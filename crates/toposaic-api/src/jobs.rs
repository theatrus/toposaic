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
    ApiError, AppState, api_error, canonical_uuid,
    database::{find_job, insert_job, mark_job_canceled, recent_jobs, update_job},
    elevation,
    grid::{
        AdjacentGridOutputPlan, adjacent_tile_specs, copy_grid_artifact, local_artifact,
        mosaic_tray_spec, publish_grid_wall_hardware, stitch_height_fields,
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
        if job_confirmed_canceled(&worker_state, &id, &cancellation) {
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
                record_job_failure(&worker_state, &id, progress, &failure);
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

/// A set cancellation flag alone does not prove the job was canceled: the
/// cancel handler sets the flag before the database mark, and clears it
/// again when the mark loses the race with completion. Only a "canceled"
/// row status makes it safe to delete the artifact directory.
fn job_confirmed_canceled(state: &AppState, id: &str, cancellation: &AtomicBool) -> bool {
    cancellation.load(Ordering::Acquire)
        && find_job(state, id)
            .ok()
            .flatten()
            .is_some_and(|job| job.status == "canceled")
}

/// Writes the failed status, logging and retrying once on error: silently
/// dropping this write would leave the job stuck in "running" with no error
/// message.
fn record_job_failure(state: &AppState, id: &str, progress: i64, failure: &str) {
    for attempt in 0..2_u32 {
        match update_job(state, id, "failed", progress, &[], Some(failure)) {
            Ok(()) => return,
            Err(error) => {
                error!(job_id = %id, %error, attempt, "could not record the job failure");
            }
        }
    }
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
        canonical_uuid(&id).ok_or_else(|| api_error(StatusCode::NOT_FOUND, "job not found"))?;
    // Set the worker's flag before the database flips to canceled: the worker
    // only removes the artifact directory when it sees the flag, so the
    // reverse order can strand artifacts if the job finishes in between.
    // The compare-exchange records whether THIS request flipped the flag —
    // when the database mark then loses its race, only the flipper may clear
    // the flag again, so a duplicate cancel can never clear a flag a
    // concurrent (winning) cancel still relies on.
    let cancellation = state
        .active_jobs
        .lock()
        .map_err(|_| internal_error("active job lock failed"))?
        .get(&id)
        .cloned();
    let this_request_set_flag = cancellation.as_ref().is_some_and(|cancellation| {
        cancellation
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_ok()
    });
    // The database mark is the one authoritative gate: it only flips rows
    // that are still queued or running, so completed and already-canceled
    // jobs land here whatever interleaving got them there.
    if !mark_job_canceled(&state, &id).map_err(internal_error)? {
        if this_request_set_flag && let Some(cancellation) = &cancellation {
            cancellation.store(false, Ordering::Release);
        }
        return if find_job(&state, &id).map_err(internal_error)?.is_some() {
            Err(api_error(StatusCode::CONFLICT, "job is no longer running"))
        } else {
            Err(api_error(StatusCode::NOT_FOUND, "job not found"))
        };
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
    let id = canonical_uuid(&id)
        .ok_or_else(|| api_error(StatusCode::NOT_FOUND, "artifact not found"))?;
    // Artifact names are predictable, so an existing file is not enough: a
    // running job's directory holds half-written outputs. Stream a file only
    // once the job is complete, or once the job's stored artifact list
    // already names it.
    let job = find_job(&state, &id)
        .map_err(internal_error)?
        .ok_or_else(|| api_error(StatusCode::NOT_FOUND, "artifact not found"))?;
    let published =
        job.status == "complete" || job.artifacts.iter().any(|artifact| artifact.name == name);
    if !published {
        return Err(api_error(StatusCode::NOT_FOUND, "artifact not found"));
    }
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
    let surface_field = if spec.color_output.enabled || spec.buildings.enabled || spec.uses_trails()
    {
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
    let output_plan = AdjacentGridOutputPlan::new(spec);
    let tile_count = tiles.len();
    debug_assert_eq!(tile_count, output_plan.tiles.len());
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

    for (index, ((tile_spec, height_field), tile_output)) in tiles
        .iter()
        .zip(height_fields.iter())
        .zip(&output_plan.tiles)
        .enumerate()
    {
        ensure_job_active(cancellation)?;
        let row = tile_output.row;
        let column = tile_output.column;
        let tile_dir = output_dir.join(&tile_output.temporary_directory);
        let surface_field = if tile_spec.color_output.enabled
            || tile_spec.buildings.enabled
            || tile_spec.uses_trails()
        {
            Some(surface::fetch_surface_field(
                tile_spec,
                height_field,
                &state.map_cache_dir,
            )?)
        } else {
            None
        };
        let terrain_spec = output_plan.terrain_spec(tile_spec);
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

        let terrain_name = &tile_output.terrain_name;
        copy_grid_artifact(
            &tile_dir.join(tile_output.terrain_source),
            &output_dir.join(terrain_name),
            terrain_name,
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

    if output_plan.mosaic_tray {
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

    ensure_job_active(cancellation)?;
    let wall_hardware_names =
        publish_grid_wall_hardware(&output_plan, spec, &output_dir, &mut artifacts)?;

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
            "wall_hardware": wall_hardware_names,
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
            canonical_uuid("395481ef-0e39-4d94-9d94-2c39fea86000").as_deref(),
            Some("395481ef-0e39-4d94-9d94-2c39fea86000")
        );
        assert_eq!(canonical_uuid(".."), None);
        assert_eq!(canonical_uuid("../data"), None);
        assert_eq!(canonical_uuid("not-a-job"), None);
    }

    fn stored_job(state: &AppState, id: &str, status: &str, artifacts: Vec<Artifact>) -> Job {
        let now = Utc::now();
        let job = Job {
            id: id.into(),
            status: status.into(),
            progress: 40,
            created_at: now,
            updated_at: now,
            spec: GenerationSpec::default(),
            artifacts,
            error: None,
        };
        insert_job(state, &job).unwrap();
        job
    }

    #[tokio::test]
    async fn a_lost_cancel_race_clears_only_its_own_flag() {
        let state = test_state();
        let id = "395481ef-0e39-4d94-9d94-2c39fea86000";
        stored_job(&state, id, "complete", Vec::new());

        // This request finds a clear flag, sets it, loses the database mark
        // (the job already completed), and must clear it again.
        let own_flag = Arc::new(AtomicBool::new(false));
        state
            .active_jobs
            .lock()
            .unwrap()
            .insert(id.into(), own_flag.clone());
        let (status, _) = cancel_job(State(state.clone()), AxumPath(id.into()))
            .await
            .unwrap_err();
        assert_eq!(status, StatusCode::CONFLICT);
        assert!(!own_flag.load(Ordering::Acquire));

        // A flag another in-flight cancel already set is not this request's
        // to clear: losing the mark must leave it set.
        let foreign_flag = Arc::new(AtomicBool::new(true));
        state
            .active_jobs
            .lock()
            .unwrap()
            .insert(id.into(), foreign_flag.clone());
        let (status, _) = cancel_job(State(state.clone()), AxumPath(id.into()))
            .await
            .unwrap_err();
        assert_eq!(status, StatusCode::CONFLICT);
        assert!(foreign_flag.load(Ordering::Acquire));
    }

    #[tokio::test]
    async fn canceling_a_running_job_flips_the_flag_and_the_row() {
        let state = test_state();
        let id = "395481ef-0e39-4d94-9d94-2c39fea86001";
        stored_job(&state, id, "running", Vec::new());
        let flag = Arc::new(AtomicBool::new(false));
        state
            .active_jobs
            .lock()
            .unwrap()
            .insert(id.into(), flag.clone());

        let canceled = cancel_job(State(state.clone()), AxumPath(id.into()))
            .await
            .unwrap()
            .0;
        assert_eq!(canceled.status, "canceled");
        assert!(flag.load(Ordering::Acquire));

        let (status, _) = cancel_job(State(state.clone()), AxumPath("missing".into()))
            .await
            .unwrap_err();
        assert_eq!(status, StatusCode::NOT_FOUND);
        let (status, _) = cancel_job(
            State(state.clone()),
            AxumPath("395481ef-0e39-4d94-9d94-2c39fea86999".into()),
        )
        .await
        .unwrap_err();
        assert_eq!(status, StatusCode::NOT_FOUND);
    }

    #[test]
    fn artifact_cleanup_requires_the_canceled_row_status_not_just_the_flag() {
        let state = test_state();
        let flag = AtomicBool::new(true);

        // Flag set but the row completed first: the race window where
        // deleting would destroy a finished job's artifacts.
        stored_job(
            &state,
            "395481ef-0e39-4d94-9d94-2c39fea86002",
            "complete",
            Vec::new(),
        );
        assert!(!job_confirmed_canceled(
            &state,
            "395481ef-0e39-4d94-9d94-2c39fea86002",
            &flag
        ));

        // Flag set and the row really canceled: cleanup may proceed.
        stored_job(
            &state,
            "395481ef-0e39-4d94-9d94-2c39fea86003",
            "canceled",
            Vec::new(),
        );
        assert!(job_confirmed_canceled(
            &state,
            "395481ef-0e39-4d94-9d94-2c39fea86003",
            &flag
        ));

        // A clear flag never triggers cleanup, whatever the row says.
        let clear = AtomicBool::new(false);
        assert!(!job_confirmed_canceled(
            &state,
            "395481ef-0e39-4d94-9d94-2c39fea86003",
            &clear
        ));
    }

    #[tokio::test]
    async fn downloads_stream_only_published_artifacts() {
        let state = test_state();
        let id = "395481ef-0e39-4d94-9d94-2c39fea86004";
        let output_dir = state.jobs_dir.join(id);
        fs::create_dir_all(&output_dir).unwrap();
        fs::write(output_dir.join("listed.json"), b"{}").unwrap();
        fs::write(output_dir.join("half-written.3mf"), b"partial").unwrap();
        stored_job(
            &state,
            id,
            "running",
            vec![Artifact {
                name: "listed.json".into(),
                media_type: "application/json".into(),
                bytes: 2,
            }],
        );

        // Running: only the artifact list's own names stream.
        let listed = download(
            State(state.clone()),
            AxumPath((id.into(), "listed.json".into())),
        )
        .await
        .unwrap();
        assert_eq!(listed.status(), StatusCode::OK);
        let (status, _) = download(
            State(state.clone()),
            AxumPath((id.into(), "half-written.3mf".into())),
        )
        .await
        .unwrap_err();
        assert_eq!(status, StatusCode::NOT_FOUND);

        // Complete: any file in the job directory streams.
        update_job(&state, id, "complete", 100, &[], None).unwrap();
        let finished = download(
            State(state.clone()),
            AxumPath((id.into(), "half-written.3mf".into())),
        )
        .await
        .unwrap();
        assert_eq!(finished.status(), StatusCode::OK);

        // Unknown jobs 404 before touching the filesystem.
        let (status, _) = download(
            State(state.clone()),
            AxumPath((
                "395481ef-0e39-4d94-9d94-2c39fea86999".into(),
                "listed.json".into(),
            )),
        )
        .await
        .unwrap_err();
        assert_eq!(status, StatusCode::NOT_FOUND);

        fs::remove_dir_all(&output_dir).unwrap();
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
