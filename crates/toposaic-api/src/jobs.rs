//! Generation jobs: HTTP handlers, the blocking job runner, and progress
//! bookkeeping.

use std::{
    fs,
    panic::{AssertUnwindSafe, catch_unwind},
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicI64, Ordering},
    },
    time::Instant,
};

use anyhow::{Context, Result};
use axum::{
    Json,
    body::Body,
    extract::{Path as AxumPath, State},
    http::{HeaderValue, StatusCode, header},
    response::Response,
};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use toposaic_core::{
    Artifact, GenerationSpec, artifact_path, generate_project_with_fields_cancellable,
    generate_tray_artifacts,
};
use tracing::{error, info};
use uuid::Uuid;

use crate::{
    ApiError, AppState, api_error,
    database::{find_job, insert_job, mark_job_canceled, recent_jobs, update_job},
    elevation,
    grid::{
        adjacent_tile_specs, copy_grid_artifact, local_artifact, mosaic_tray_spec,
        stitch_height_fields,
    },
    internal_error, surface,
};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct Job {
    pub(crate) id: String,
    pub(crate) status: String,
    pub(crate) progress: i64,
    pub(crate) created_at: DateTime<Utc>,
    pub(crate) updated_at: DateTime<Utc>,
    pub(crate) spec: GenerationSpec,
    pub(crate) artifacts: Vec<Artifact>,
    pub(crate) error: Option<String>,
}

pub(crate) async fn create_job(
    State(state): State<AppState>,
    Json(spec): Json<GenerationSpec>,
) -> Result<(StatusCode, Json<Job>), (StatusCode, Json<ApiError>)> {
    spec.validate()
        .map_err(|error| api_error(StatusCode::BAD_REQUEST, error))?;

    let id = Uuid::new_v4().to_string();
    let now = Utc::now();
    let job = Job {
        id: id.clone(),
        status: "queued".into(),
        progress: 0,
        created_at: now,
        updated_at: now,
        spec: spec.clone(),
        artifacts: Vec::new(),
        error: None,
    };
    insert_job(&state, &job).map_err(internal_error)?;

    let cancellation = Arc::new(AtomicBool::new(false));
    state
        .active_jobs
        .lock()
        .map_err(|_| internal_error("active job lock failed"))?
        .insert(id.clone(), cancellation.clone());
    let worker_state = state.clone();
    tokio::task::spawn_blocking(move || {
        let result = catch_unwind(AssertUnwindSafe(|| {
            run_job(&worker_state, &id, &spec, &cancellation)
        }));
        if cancellation.load(Ordering::Acquire) {
            let output_dir = worker_state.jobs_dir.join(&id);
            if let Err(cleanup_error) = std::fs::remove_dir_all(&output_dir)
                && cleanup_error.kind() != std::io::ErrorKind::NotFound
            {
                error!(job_id = %id, error = %cleanup_error, "cancel cleanup failed");
            }
        } else {
            let failure = match result {
                Ok(Ok(())) => None,
                Ok(Err(error)) => Some(error.to_string()),
                Err(payload) => Some(panic_message(payload)),
            };
            if let Some(failure) = failure {
                error!(job_id = %id, error = %failure, "generation failed");
                let progress = find_job(&worker_state, &id)
                    .ok()
                    .flatten()
                    .map(|job| job.progress)
                    .unwrap_or(0);
                let _ = update_job(&worker_state, &id, "failed", progress, &[], Some(&failure));
            }
        }
        if let Ok(mut active_jobs) = worker_state.active_jobs.lock() {
            active_jobs.remove(&id);
        }
    });

    Ok((StatusCode::ACCEPTED, Json(job)))
}

pub(crate) async fn create_preview(
    State(state): State<AppState>,
    Json(spec): Json<GenerationSpec>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<ApiError>)> {
    spec.validate()
        .map_err(|error| api_error(StatusCode::BAD_REQUEST, error))?;
    let cache_dir = state.map_cache_dir.join("elevation");
    let preview = tokio::task::spawn_blocking(move || {
        let samples = spec.terrain_samples_per_piece().clamp(64, 128) as usize;
        let height_field = elevation::fetch_preview_height_field(&spec, &cache_dir, samples)?;
        toposaic_core::build_height_preview(&spec, &height_field, samples)
    })
    .await
    .map_err(internal_error)?
    .map_err(internal_error)?;
    Ok(Json(preview))
}

fn panic_message(payload: Box<dyn std::any::Any + Send>) -> String {
    if let Some(message) = payload.downcast_ref::<&str>() {
        format!("mesh generation panicked: {message}")
    } else if let Some(message) = payload.downcast_ref::<String>() {
        format!("mesh generation panicked: {message}")
    } else {
        "mesh generation panicked".into()
    }
}

pub(crate) async fn get_job(
    State(state): State<AppState>,
    AxumPath(id): AxumPath<String>,
) -> Result<Json<Job>, (StatusCode, Json<ApiError>)> {
    find_job(&state, &id)
        .map_err(internal_error)?
        .map(Json)
        .ok_or_else(|| api_error(StatusCode::NOT_FOUND, "job not found"))
}

pub(crate) async fn cancel_job(
    State(state): State<AppState>,
    AxumPath(id): AxumPath<String>,
) -> Result<Json<Job>, (StatusCode, Json<ApiError>)> {
    let id =
        canonical_job_id(&id).ok_or_else(|| api_error(StatusCode::NOT_FOUND, "job not found"))?;
    let job = find_job(&state, &id)
        .map_err(internal_error)?
        .ok_or_else(|| api_error(StatusCode::NOT_FOUND, "job not found"))?;
    if !matches!(job.status.as_str(), "queued" | "running") {
        return Err(api_error(StatusCode::CONFLICT, "job is no longer running"));
    }

    // Set the worker's flag before the database flips to canceled: the worker
    // only removes the artifact directory when it sees the flag, so the
    // reverse order can strand artifacts if the job finishes in between.
    let cancellation = state
        .active_jobs
        .lock()
        .map_err(|_| internal_error("active job lock failed"))?
        .get(&id)
        .cloned();
    if let Some(cancellation) = &cancellation {
        cancellation.store(true, Ordering::Release);
    }
    if !mark_job_canceled(&state, &id).map_err(internal_error)? {
        if let Some(cancellation) = &cancellation {
            cancellation.store(false, Ordering::Release);
        }
        return Err(api_error(StatusCode::CONFLICT, "job is no longer running"));
    }
    find_job(&state, &id)
        .map_err(internal_error)?
        .map(Json)
        .ok_or_else(|| api_error(StatusCode::NOT_FOUND, "job not found"))
}

pub(crate) async fn list_jobs(
    State(state): State<AppState>,
) -> Result<Json<Vec<Job>>, (StatusCode, Json<ApiError>)> {
    recent_jobs(&state, 20).map(Json).map_err(internal_error)
}

pub(crate) async fn download(
    State(state): State<AppState>,
    AxumPath((id, name)): AxumPath<(String, String)>,
) -> Result<Response, (StatusCode, Json<ApiError>)> {
    let id = canonical_job_id(&id)
        .ok_or_else(|| api_error(StatusCode::NOT_FOUND, "artifact not found"))?;
    let output_dir = state.jobs_dir.join(id);
    let path = artifact_path(&output_dir, &name)
        .ok_or_else(|| api_error(StatusCode::NOT_FOUND, "artifact not found"))?;
    // Solid-model artifacts can run to hundreds of megabytes; stream them
    // instead of buffering whole files per request.
    let file = tokio::fs::File::open(&path).await.map_err(internal_error)?;
    let content_length = file.metadata().await.map_err(internal_error)?.len();
    let content_type = match path.extension().and_then(|value| value.to_str()) {
        Some("stl") => "model/stl",
        Some("3mf") => "model/3mf",
        Some("json") => "application/json",
        _ => "application/octet-stream",
    };
    let mut response = Response::new(Body::from_stream(tokio_util::io::ReaderStream::new(file)));
    response
        .headers_mut()
        .insert(header::CONTENT_TYPE, HeaderValue::from_static(content_type));
    response.headers_mut().insert(
        header::CONTENT_LENGTH,
        HeaderValue::from_str(&content_length.to_string()).map_err(internal_error)?,
    );
    response.headers_mut().insert(
        header::CONTENT_DISPOSITION,
        HeaderValue::from_str(&format!("attachment; filename=\"{name}\""))
            .map_err(internal_error)?,
    );
    Ok(response)
}

fn canonical_job_id(id: &str) -> Option<String> {
    Uuid::parse_str(id)
        .ok()
        .map(|value| value.hyphenated().to_string())
}

fn run_job(
    state: &AppState,
    id: &str,
    spec: &GenerationSpec,
    cancellation: &AtomicBool,
) -> Result<()> {
    if spec.adjacent_columns > 1 || spec.adjacent_rows > 1 {
        return run_adjacent_grid_job(state, id, spec, cancellation);
    }
    let job_started = Instant::now();
    ensure_job_active(cancellation)?;
    update_job(state, id, "running", 8, &[], None)?;
    let phase_started = Instant::now();
    let mut last_elevation_progress = 8;
    let height_field = elevation::fetch_height_field_with_progress(
        spec,
        &state.map_cache_dir.join("elevation"),
        |fraction| {
            ensure_job_active(cancellation)?;
            let progress = elevation_job_progress(fraction);
            if progress > last_elevation_progress {
                update_job(state, id, "running", progress, &[], None)?;
                last_elevation_progress = progress;
            }
            Ok(())
        },
    )?;
    ensure_job_active(cancellation)?;
    info!(
        job_id = %id,
        phase = "elevation",
        elapsed_ms = phase_started.elapsed().as_millis() as u64,
        "generation phase complete"
    );
    update_job(state, id, "running", 40, &[], None)?;
    let surface_field = if spec.color_output.enabled || spec.buildings.enabled {
        update_job(state, id, "running", 42, &[], None)?;
        let phase_started = Instant::now();
        let field = surface::fetch_surface_field(spec, &height_field, &state.map_cache_dir)?;
        ensure_job_active(cancellation)?;
        info!(
            job_id = %id,
            phase = "surface",
            elapsed_ms = phase_started.elapsed().as_millis() as u64,
            "generation phase complete"
        );
        Some(field)
    } else {
        None
    };
    update_job(state, id, "running", 65, &[], None)?;
    let output_dir = state.jobs_dir.join(id);
    let phase_started = Instant::now();
    let mesh_progress = AtomicI64::new(65);
    let manifest = generate_project_with_fields_cancellable(
        spec,
        &height_field,
        surface_field.as_ref(),
        &output_dir,
        &|| cancellation.load(Ordering::Acquire),
        &|fraction| {
            ensure_job_active(cancellation)?;
            let progress = mesh_job_progress(fraction);
            let previous = mesh_progress.fetch_max(progress, Ordering::AcqRel);
            if progress > previous {
                update_job(state, id, "running", progress, &[], None)?;
            }
            Ok(())
        },
    )?;
    ensure_job_active(cancellation)?;
    info!(
        job_id = %id,
        phase = "mesh",
        elapsed_ms = phase_started.elapsed().as_millis() as u64,
        "generation phase complete"
    );
    update_job(state, id, "complete", 100, &manifest.artifacts, None)?;
    info!(
        job_id = %id,
        elapsed_ms = job_started.elapsed().as_millis() as u64,
        "generation complete"
    );
    Ok(())
}

fn run_adjacent_grid_job(
    state: &AppState,
    id: &str,
    spec: &GenerationSpec,
    cancellation: &AtomicBool,
) -> Result<()> {
    let job_started = Instant::now();
    let mut tiles = adjacent_tile_specs(spec);
    let tile_count = tiles.len();
    ensure_job_active(cancellation)?;
    update_job(state, id, "running", 8, &[], None)?;

    let mut height_fields = Vec::with_capacity(tile_count);
    let mut last_elevation_progress = 8;
    for (index, tile_spec) in tiles.iter().enumerate() {
        let height_field = elevation::fetch_height_field_with_progress(
            tile_spec,
            &state.map_cache_dir.join("elevation"),
            |fraction| {
                ensure_job_active(cancellation)?;
                let combined = (index as f32 + fraction) / tile_count as f32;
                let progress = elevation_job_progress(combined);
                if progress > last_elevation_progress {
                    update_job(state, id, "running", progress, &[], None)?;
                    last_elevation_progress = progress;
                }
                Ok(())
            },
        )?;
        height_fields.push(height_field);
    }

    if spec.elevation_datum_m.is_none() {
        let (minimum, maximum) = height_fields.iter().fold(
            (f32::INFINITY, f32::NEG_INFINITY),
            |(minimum, maximum), field| {
                let (field_minimum, field_maximum) = field.elevation_bounds();
                (minimum.min(field_minimum), maximum.max(field_maximum))
            },
        );
        let metres_per_mm = (maximum - minimum).max(1.0) / spec.relief_mm;
        for tile in &mut tiles {
            tile.elevation_datum_m = Some(minimum);
            tile.elevation_m_per_mm = Some(metres_per_mm);
        }
    }

    let output_dir = state.jobs_dir.join(id);
    fs::create_dir_all(&output_dir)
        .with_context(|| format!("create output directory {}", output_dir.display()))?;
    let mut artifacts = Vec::new();
    let mut tile_manifest = Vec::with_capacity(tile_count);
    let mut mosaic_tray_names = Vec::new();
    let mesh_progress = AtomicI64::new(40);

    for (index, (tile_spec, height_field)) in tiles.iter().zip(height_fields.iter()).enumerate() {
        ensure_job_active(cancellation)?;
        let row = index as u32 / spec.adjacent_columns;
        let column = index as u32 % spec.adjacent_columns;
        let tile_dir = output_dir.join(format!(".tile-{}-{}", row + 1, column + 1));
        let surface_field = if tile_spec.color_output.enabled || tile_spec.buildings.enabled {
            Some(surface::fetch_surface_field(
                tile_spec,
                height_field,
                &state.map_cache_dir,
            )?)
        } else {
            None
        };
        let mut terrain_spec = tile_spec.clone();
        if !spec.tray.individual_tiles {
            terrain_spec.tray.enabled = false;
        } else {
            terrain_spec.tray.segment_columns = 1;
            terrain_spec.tray.segment_rows = 1;
        }
        let manifest = generate_project_with_fields_cancellable(
            &terrain_spec,
            height_field,
            surface_field.as_ref(),
            &tile_dir,
            &|| cancellation.load(Ordering::Acquire),
            &|fraction| {
                ensure_job_active(cancellation)?;
                let combined = (index as f32 + fraction) / tile_count as f32;
                let progress = (40.0 + combined * 49.0).round() as i64;
                let previous = mesh_progress.fetch_max(progress, Ordering::AcqRel);
                if progress > previous {
                    update_job(state, id, "running", progress, &[], None)?;
                }
                Ok(())
            },
        )?;

        let terrain_source = if tile_spec.solid_model {
            "terrain-solid.3mf"
        } else {
            "toposaic.3mf"
        };
        let terrain_name = format!("terrain-r{:02}-c{:02}.3mf", row + 1, column + 1);
        copy_grid_artifact(
            &tile_dir.join(terrain_source),
            &output_dir.join(&terrain_name),
            &terrain_name,
            "model/3mf",
            &mut artifacts,
        )?;

        let mut tray_names = Vec::new();
        for tray_artifact in manifest
            .artifacts
            .iter()
            .filter(|artifact| artifact.name.starts_with("terrain-tray"))
        {
            let segment = tray_artifact
                .name
                .strip_prefix("terrain-tray")
                .unwrap_or_default();
            let name = format!("tray-tile-r{:02}-c{:02}{segment}", row + 1, column + 1);
            copy_grid_artifact(
                &tile_dir.join(&tray_artifact.name),
                &output_dir.join(&name),
                &name,
                &tray_artifact.media_type,
                &mut artifacts,
            )?;
            tray_names.push(name);
        }
        if index == 0 {
            copy_grid_artifact(
                &tile_dir.join("preview.json"),
                &output_dir.join("preview.json"),
                "preview.json",
                "application/json",
                &mut artifacts,
            )?;
        }
        tile_manifest.push(serde_json::json!({
            "row": row + 1,
            "column": column + 1,
            "center_lat": tile_spec.center_lat,
            "center_lon": tile_spec.center_lon,
            "terrain": terrain_name,
            "trays": tray_names,
            "source": manifest.terrain_source,
        }));
        fs::remove_dir_all(&tile_dir)
            .with_context(|| format!("remove temporary tile directory {}", tile_dir.display()))?;
    }

    if spec.tray.enabled && !spec.tray.individual_tiles {
        ensure_job_active(cancellation)?;
        update_job(state, id, "running", 90, &[], None)?;
        let mosaic_height =
            stitch_height_fields(&height_fields, spec.adjacent_rows, spec.adjacent_columns)?;
        let mosaic_spec = mosaic_tray_spec(spec);
        let tray_dir = output_dir.join(".mosaic-tray");
        for tray_artifact in generate_tray_artifacts(&mosaic_spec, Some(&mosaic_height), &tray_dir)?
        {
            let name = tray_artifact
                .name
                .replacen("terrain-tray", "mosaic-tray", 1);
            copy_grid_artifact(
                &tray_dir.join(&tray_artifact.name),
                &output_dir.join(&name),
                &name,
                &tray_artifact.media_type,
                &mut artifacts,
            )?;
            mosaic_tray_names.push(name);
        }
        fs::remove_dir_all(&tray_dir)
            .with_context(|| format!("remove temporary tray directory {}", tray_dir.display()))?;
    }

    let manifest_name = "manifest.json";
    let manifest_path = output_dir.join(manifest_name);
    fs::write(
        &manifest_path,
        serde_json::to_vec_pretty(&serde_json::json!({
            "generator": format!("toposaic/{}", env!("CARGO_PKG_VERSION")),
            "layout": "north-west anchor, rows run south and columns run east",
            "spec": spec,
            "shared_height_frame": {
                "elevation_datum_m": tiles[0].elevation_datum_m,
                "elevation_m_per_mm": tiles[0].elevation_m_per_mm,
            },
            "tiles": tile_manifest,
            "mosaic_trays": mosaic_tray_names,
        }))?,
    )?;
    artifacts.push(local_artifact(
        &manifest_path,
        manifest_name,
        "application/json",
    )?);
    update_job(state, id, "complete", 100, &artifacts, None)?;
    info!(
        job_id = %id,
        tiles = tile_count,
        elapsed_ms = job_started.elapsed().as_millis() as u64,
        "super-tile grid generation complete"
    );
    Ok(())
}

fn ensure_job_active(cancellation: &AtomicBool) -> Result<()> {
    if cancellation.load(Ordering::Acquire) {
        anyhow::bail!("generation canceled");
    }
    Ok(())
}

fn elevation_job_progress(fraction: f32) -> i64 {
    (8.0 + fraction.clamp(0.0, 1.0) * 31.0).round() as i64
}

fn mesh_job_progress(fraction: f32) -> i64 {
    (65.0 + fraction.clamp(0.0, 1.0) * 34.0).round() as i64
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_state;

    #[test]
    fn panic_payload_becomes_a_job_error() {
        assert_eq!(
            panic_message(Box::new("triangulation failed")),
            "mesh generation panicked: triangulation failed"
        );
    }

    #[test]
    fn artifact_downloads_require_uuid_job_directories() {
        assert_eq!(
            canonical_job_id("395481ef-0e39-4d94-9d94-2c39fea86000").as_deref(),
            Some("395481ef-0e39-4d94-9d94-2c39fea86000")
        );
        assert_eq!(canonical_job_id(".."), None);
        assert_eq!(canonical_job_id("../data"), None);
        assert_eq!(canonical_job_id("not-a-job"), None);
    }

    #[test]
    fn canceled_jobs_cannot_return_to_running_or_complete() {
        let state = test_state();
        let now = Utc::now();
        let job = Job {
            id: "395481ef-0e39-4d94-9d94-2c39fea86000".into(),
            status: "running".into(),
            progress: 40,
            created_at: now,
            updated_at: now,
            spec: GenerationSpec::default(),
            artifacts: Vec::new(),
            error: None,
        };
        insert_job(&state, &job).unwrap();

        assert!(mark_job_canceled(&state, &job.id).unwrap());
        update_job(&state, &job.id, "complete", 100, &[], None).unwrap();

        let canceled = find_job(&state, &job.id).unwrap().unwrap();
        assert_eq!(canceled.status, "canceled");
        assert_eq!(canceled.progress, 40);
        assert!(canceled.artifacts.is_empty());
        assert!(!mark_job_canceled(&state, &job.id).unwrap());
    }

    #[test]
    fn maps_real_phase_progress_into_the_job_range() {
        assert_eq!(elevation_job_progress(0.0), 8);
        assert_eq!(elevation_job_progress(0.5), 24);
        assert_eq!(elevation_job_progress(1.0), 39);
        assert_eq!(mesh_job_progress(0.0), 65);
        assert_eq!(mesh_job_progress(0.5), 82);
        assert_eq!(mesh_job_progress(1.0), 99);
    }
}
