use std::{
    collections::HashMap,
    fs,
    panic::{AssertUnwindSafe, catch_unwind},
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex as StdMutex,
        atomic::{AtomicBool, AtomicI64, Ordering},
    },
    time::{Duration, Instant},
};

use anyhow::{Context, Result, bail};
use axum::{
    Json, Router,
    body::Body,
    extract::{Path as AxumPath, State},
    http::{HeaderValue, StatusCode, header},
    response::Response,
    routing::get,
};
use chrono::{DateTime, Utc};
use reqwest::Client;
use rusqlite::Connection;
use serde::{Deserialize, Serialize};
use tokio::{net::TcpListener, sync::Mutex as AsyncMutex};
use toposaic_core::{
    Artifact, GenerationSpec, HeightField, SuperTileAnchor, artifact_path,
    generate_project_with_fields_cancellable, generate_tray_artifacts,
};
use tower_http::trace::TraceLayer;
use tracing::{error, info};
use uuid::Uuid;

mod cache;
mod database;
mod elevation;
mod geo;
mod geocoding;
mod http;
mod settings;
mod surface;

use database::{find_job, insert_job, mark_job_canceled, migrate, recent_jobs, update_job};
use geocoding::search_places;

#[derive(Clone)]
struct AppState {
    db: Arc<StdMutex<Connection>>,
    jobs_dir: Arc<PathBuf>,
    map_cache_dir: Arc<PathBuf>,
    geocoder: Client,
    geocoder_base_url: Arc<String>,
    last_geocode_request: Arc<AsyncMutex<Instant>>,
    active_jobs: Arc<StdMutex<HashMap<String, Arc<AtomicBool>>>>,
}

#[derive(Debug, Serialize)]
struct Health {
    status: &'static str,
    storage: &'static str,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct Job {
    id: String,
    status: String,
    progress: i64,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
    spec: GenerationSpec,
    artifacts: Vec<Artifact>,
    error: Option<String>,
}

#[derive(Debug, Serialize)]
struct ApiError {
    error: String,
}

pub async fn run() -> Result<()> {
    run_with(settings::data_dir(), settings::bind_address()).await
}

pub async fn run_with(data_dir: PathBuf, address: String) -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "toposaic_api=info,tower_http=info".into()),
        )
        .try_init()
        .ok();

    let jobs_dir = data_dir.join("jobs");
    let map_cache_dir = cache::root()?;
    std::fs::create_dir_all(&jobs_dir)
        .with_context(|| format!("create jobs directory {}", jobs_dir.display()))?;
    std::fs::create_dir_all(&map_cache_dir)
        .with_context(|| format!("create map cache directory {}", map_cache_dir.display()))?;
    let connection = Connection::open(data_dir.join("toposaic.sqlite3"))?;
    migrate(&connection)?;
    let geocoder = http::async_client(Duration::from_secs(12))?;

    let state = AppState {
        db: Arc::new(StdMutex::new(connection)),
        jobs_dir: Arc::new(jobs_dir),
        map_cache_dir: Arc::new(map_cache_dir.clone()),
        geocoder,
        geocoder_base_url: Arc::new(settings::geocoder_base_url()),
        last_geocode_request: Arc::new(AsyncMutex::new(Instant::now() - Duration::from_secs(1))),
        active_jobs: Arc::new(StdMutex::new(HashMap::new())),
    };

    let app = Router::new()
        .route("/api/health", get(health))
        .route("/api/places", get(search_places))
        .route("/api/preview", axum::routing::post(create_preview))
        .route("/api/jobs", get(list_jobs).post(create_job))
        .route("/api/jobs/{id}", get(get_job).delete(cancel_job))
        .route("/api/jobs/{id}/downloads/{name}", get(download))
        .layer(http::cors_layer(settings::allowed_origins()))
        .layer(TraceLayer::new_for_http())
        .with_state(state);

    let listener = TcpListener::bind(&address).await?;
    info!(
        %address,
        data_dir = %data_dir.display(),
        map_cache_dir = %map_cache_dir.display(),
        "terrain api ready"
    );
    axum::serve(listener, app).await?;
    Ok(())
}

async fn health() -> Json<Health> {
    Json(Health {
        status: "ok",
        storage: "sqlite",
    })
}

async fn create_job(
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

async fn create_preview(
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

async fn get_job(
    State(state): State<AppState>,
    AxumPath(id): AxumPath<String>,
) -> Result<Json<Job>, (StatusCode, Json<ApiError>)> {
    find_job(&state, &id)
        .map_err(internal_error)?
        .map(Json)
        .ok_or_else(|| api_error(StatusCode::NOT_FOUND, "job not found"))
}

async fn cancel_job(
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

    if !mark_job_canceled(&state, &id).map_err(internal_error)? {
        return Err(api_error(StatusCode::CONFLICT, "job is no longer running"));
    }
    if let Some(cancellation) = state
        .active_jobs
        .lock()
        .map_err(|_| internal_error("active job lock failed"))?
        .get(&id)
    {
        cancellation.store(true, Ordering::Release);
    }
    find_job(&state, &id)
        .map_err(internal_error)?
        .map(Json)
        .ok_or_else(|| api_error(StatusCode::NOT_FOUND, "job not found"))
}

async fn list_jobs(
    State(state): State<AppState>,
) -> Result<Json<Vec<Job>>, (StatusCode, Json<ApiError>)> {
    recent_jobs(&state, 20).map(Json).map_err(internal_error)
}

async fn download(
    State(state): State<AppState>,
    AxumPath((id, name)): AxumPath<(String, String)>,
) -> Result<Response, (StatusCode, Json<ApiError>)> {
    let id = canonical_job_id(&id)
        .ok_or_else(|| api_error(StatusCode::NOT_FOUND, "artifact not found"))?;
    let output_dir = state.jobs_dir.join(id);
    let path = artifact_path(&output_dir, &name)
        .ok_or_else(|| api_error(StatusCode::NOT_FOUND, "artifact not found"))?;
    let bytes = tokio::fs::read(&path).await.map_err(internal_error)?;
    let content_type = match path.extension().and_then(|value| value.to_str()) {
        Some("stl") => "model/stl",
        Some("3mf") => "model/3mf",
        Some("json") => "application/json",
        _ => "application/octet-stream",
    };
    let mut response = Response::new(Body::from(bytes));
    response
        .headers_mut()
        .insert(header::CONTENT_TYPE, HeaderValue::from_static(content_type));
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

fn adjacent_tile_specs(spec: &GenerationSpec) -> Vec<GenerationSpec> {
    let row_anchor = match spec.super_tile_anchor {
        SuperTileAnchor::TopLeft => 0.0,
        SuperTileAnchor::Center => (f64::from(spec.adjacent_rows) - 1.0) / 2.0,
    };
    let column_anchor = match spec.super_tile_anchor {
        SuperTileAnchor::TopLeft => 0.0,
        SuperTileAnchor::Center => (f64::from(spec.adjacent_columns) - 1.0) / 2.0,
    };
    (0..spec.adjacent_rows)
        .flat_map(|row| {
            (0..spec.adjacent_columns).map(move |column| {
                let mut tile = spec.clone();
                let row_offset = f64::from(row) - row_anchor;
                let column_offset = f64::from(column) - column_anchor;
                (tile.center_lat, tile.center_lon) = geo::offset_coordinates(
                    spec.center_lat,
                    spec.center_lon,
                    -row_offset * spec.ground_span_km,
                    column_offset * spec.ground_span_km,
                );
                tile.adjacent_tile_column = column;
                tile.adjacent_tile_row = row;
                tile
            })
        })
        .collect()
}

fn mosaic_tray_spec(spec: &GenerationSpec) -> GenerationSpec {
    let mut mosaic = spec.clone();
    mosaic.width_mm *= spec.adjacent_columns as f32;
    mosaic.rows *= spec.adjacent_rows;
    mosaic.columns *= spec.adjacent_columns;
    mosaic.ground_span_km *= spec.adjacent_columns as f64;
    mosaic.adjacent_tile_column = 0;
    mosaic.adjacent_tile_row = 0;
    mosaic.tray.individual_tiles = false;
    mosaic.tray.segment_columns = spec.adjacent_columns;
    mosaic.tray.segment_rows = spec.adjacent_rows;
    mosaic
}

fn stitch_height_fields(fields: &[HeightField], rows: u32, columns: u32) -> Result<HeightField> {
    if rows == 0 || columns == 0 || fields.len() != (rows * columns) as usize {
        bail!("height fields do not match the adjacent tray grid");
    }
    let tile_width = fields[0].width;
    let tile_height = fields[0].height;
    if fields
        .iter()
        .any(|field| field.width != tile_width || field.height != tile_height)
    {
        bail!("adjacent height fields must use matching sample dimensions");
    }
    let width = columns as usize * (tile_width - 1) + 1;
    let height = rows as usize * (tile_height - 1) + 1;
    let mut sums = vec![0.0_f32; width * height];
    let mut counts = vec![0_u8; width * height];
    for (tile_index, field) in fields.iter().enumerate() {
        let tile_row = tile_index / columns as usize;
        let tile_column = tile_index % columns as usize;
        let x_offset = tile_column * (tile_width - 1);
        let y_offset = tile_row * (tile_height - 1);
        for y in 0..tile_height {
            for x in 0..tile_width {
                let output = (y_offset + y) * width + x_offset + x;
                sums[output] += field.values_m[y * tile_width + x];
                counts[output] += 1;
            }
        }
    }
    for (value, count) in sums.iter_mut().zip(counts) {
        *value /= f32::from(count);
    }
    HeightField::new(width, height, sums, "stitched adjacent elevation grid")
}

fn copy_grid_artifact(
    source: &Path,
    destination: &Path,
    name: &str,
    media_type: &str,
    artifacts: &mut Vec<Artifact>,
) -> Result<()> {
    fs::copy(source, destination)
        .with_context(|| format!("copy {} to {}", source.display(), destination.display()))?;
    artifacts.push(local_artifact(destination, name, media_type)?);
    Ok(())
}

fn local_artifact(path: &Path, name: &str, media_type: &str) -> Result<Artifact> {
    Ok(Artifact {
        name: name.to_owned(),
        media_type: media_type.to_owned(),
        bytes: fs::metadata(path)?.len(),
    })
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

fn api_error(status: StatusCode, message: impl ToString) -> (StatusCode, Json<ApiError>) {
    (
        status,
        Json(ApiError {
            error: message.to_string(),
        }),
    )
}

fn internal_error(error: impl std::fmt::Display) -> (StatusCode, Json<ApiError>) {
    api_error(StatusCode::INTERNAL_SERVER_ERROR, error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_state() -> AppState {
        let connection = Connection::open_in_memory().unwrap();
        migrate(&connection).unwrap();
        let data_dir =
            std::env::temp_dir().join(format!("toposaic-api-test-{}", std::process::id()));
        AppState {
            db: Arc::new(StdMutex::new(connection)),
            jobs_dir: Arc::new(data_dir.join("jobs")),
            map_cache_dir: Arc::new(data_dir.join("cache")),
            geocoder: Client::new(),
            geocoder_base_url: Arc::new("https://example.invalid".into()),
            last_geocode_request: Arc::new(AsyncMutex::new(Instant::now())),
            active_jobs: Arc::new(StdMutex::new(HashMap::new())),
        }
    }

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

    #[test]
    fn super_tile_grid_uses_the_current_tile_as_its_top_left_anchor() {
        let spec = GenerationSpec {
            center_lat: 46.0,
            center_lon: -121.0,
            ground_span_km: 10.0,
            adjacent_columns: 3,
            adjacent_rows: 2,
            ..GenerationSpec::default()
        };
        let tiles = adjacent_tile_specs(&spec);

        assert_eq!(tiles.len(), 6);
        assert_eq!(tiles[0].center_lat, spec.center_lat);
        assert_eq!(tiles[0].center_lon, spec.center_lon);
        assert!(tiles[1].center_lon > tiles[0].center_lon);
        assert!(tiles[3].center_lat < tiles[0].center_lat);
        assert_eq!(tiles[5].adjacent_tile_column, 2);
        assert_eq!(tiles[5].adjacent_tile_row, 1);
    }

    #[test]
    fn super_tile_grid_can_use_the_selected_point_as_its_center() {
        let spec = GenerationSpec {
            center_lat: 46.0,
            center_lon: -121.0,
            ground_span_km: 10.0,
            adjacent_columns: 5,
            adjacent_rows: 3,
            super_tile_anchor: SuperTileAnchor::Center,
            ..GenerationSpec::default()
        };
        let tiles = adjacent_tile_specs(&spec);
        let top_left = &tiles[0];
        let center = &tiles[7];
        let bottom_right = &tiles[14];

        assert_eq!(tiles.len(), 15);
        assert!(top_left.center_lat > spec.center_lat);
        assert!(top_left.center_lon < spec.center_lon);
        assert_eq!(center.center_lat, spec.center_lat);
        assert_eq!(center.center_lon, spec.center_lon);
        assert!(bottom_right.center_lat < spec.center_lat);
        assert!(bottom_right.center_lon > spec.center_lon);
        assert!(
            ((top_left.center_lat + bottom_right.center_lat) / 2.0 - spec.center_lat).abs() < 1e-9
        );
        assert!(
            ((top_left.center_lon + bottom_right.center_lon) / 2.0 - spec.center_lon).abs() < 1e-9
        );
    }

    #[test]
    fn mosaic_tray_follows_the_adjacent_tile_grid() {
        let spec = GenerationSpec {
            width_mm: 100.0,
            rows: 4,
            columns: 5,
            adjacent_columns: 3,
            adjacent_rows: 2,
            adjacent_interlocks: true,
            ..GenerationSpec::default()
        };
        let tray = mosaic_tray_spec(&spec);

        assert_eq!(tray.width_mm, 300.0);
        assert_eq!(tray.rows, 8);
        assert_eq!(tray.columns, 15);
        assert_eq!(tray.tray.segment_rows, 2);
        assert_eq!(tray.tray.segment_columns, 3);
        assert!(tray.adjacent_interlocks);
    }

    #[test]
    fn stitched_tray_height_field_averages_shared_samples() {
        let left = HeightField::new(2, 2, vec![1.0, 2.0, 3.0, 4.0], "left").unwrap();
        let right = HeightField::new(2, 2, vec![4.0, 5.0, 6.0, 7.0], "right").unwrap();
        let stitched = stitch_height_fields(&[left, right], 1, 2).unwrap();

        assert_eq!((stitched.width, stitched.height), (3, 2));
        assert_eq!(stitched.values_m, vec![1.0, 3.0, 5.0, 3.0, 5.0, 7.0]);
    }
}
