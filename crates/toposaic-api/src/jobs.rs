//! Generation jobs: HTTP handlers, the blocking job runner, and progress
//! bookkeeping.

use std::{
    convert::Infallible,
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
    body::{Body, Bytes},
    extract::{Path as AxumPath, State},
    http::{HeaderMap, HeaderValue, StatusCode, header},
    response::{IntoResponse, Response},
};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use toposaic_core::{
    Artifact, FlagMarkerStyle, GenerationSpec, MarkerKind, SurfaceField, artifact_path,
    generate_marker_artifacts, generate_project_with_fields_cancellable, generate_tray_artifacts,
    height_frame_for_bounds,
};
use tracing::{error, info};
use uuid::Uuid;

use crate::{
    ApiError, AppState, api_error, cache, canonical_uuid,
    database::{find_job, insert_job, mark_job_canceled, recent_jobs, update_job},
    elevation,
    grid::{
        AdjacentGridOutputPlan, adjacent_tile_specs, copy_grid_artifact, local_artifact,
        mosaic_tray_spec, publish_grid_wall_hardware, stitch_height_fields,
    },
    internal_error, sources, surface,
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
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) failure: Option<JobFailure>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct JobFailure {
    pub(crate) title: String,
    pub(crate) message: String,
    pub(crate) technical_detail: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) control_tab: Option<JobControlTab>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) piece: Option<JobPiece>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub(crate) enum JobControlTab {
    Model,
    Surface,
    Markers,
    Mounting,
    Output,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct JobPiece {
    pub(crate) row: u32,
    pub(crate) column: u32,
}

const PREVIEW_STREAM_CONTENT_TYPE: &str = "application/x-ndjson";

#[derive(Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum PreviewStreamEvent {
    Progress {
        stage: &'static str,
        label: &'static str,
        progress: u8,
    },
    Complete {
        preview: serde_json::Value,
    },
    Error {
        error: String,
    },
    Canceled,
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
        failure: None,
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
        // One recording session per job, opened here so both the single-tile
        // and super-tile paths are covered and neither has to remember to.
        // The whole fetch phase runs on this blocking thread, which is what
        // makes a thread-local log sound; see cache::SourceLog.
        let _recording = cache::Recording::begin();
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
                Ok(Err(error)) => Some(format_job_error(&error)),
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
    headers: HeaderMap,
    Json(spec): Json<GenerationSpec>,
) -> Result<Response, (StatusCode, Json<ApiError>)> {
    spec.validate()
        .map_err(|error| api_error(StatusCode::BAD_REQUEST, error))?;
    let cancellation = begin_preview(&state).map_err(internal_error)?;
    let map_cache_dir = state.map_cache_dir.clone();
    let wants_stream = headers
        .get(header::ACCEPT)
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| value.contains(PREVIEW_STREAM_CONTENT_TYPE));

    if wants_stream {
        let (sender, receiver) = tokio::sync::mpsc::channel::<Bytes>(16);
        let worker_state = state.clone();
        let worker_cancellation = cancellation.clone();
        tokio::task::spawn_blocking(move || {
            let result = catch_live_preview(
                &spec,
                &map_cache_dir,
                &worker_cancellation,
                |stage, label, progress| {
                    send_preview_event(
                        &sender,
                        &worker_cancellation,
                        PreviewStreamEvent::Progress {
                            stage,
                            label,
                            progress,
                        },
                    )
                },
            );
            let event = match result {
                Ok(preview) => PreviewStreamEvent::Complete { preview },
                Err(_) if worker_cancellation.load(Ordering::Acquire) => {
                    PreviewStreamEvent::Canceled
                }
                Err(error) => PreviewStreamEvent::Error {
                    error: error.to_string(),
                },
            };
            let _ = send_preview_event(&sender, &worker_cancellation, event);
            finish_preview(&worker_state, &worker_cancellation);
        });
        let stream = futures_util::stream::unfold(receiver, |mut receiver| async move {
            receiver
                .recv()
                .await
                .map(|bytes| (Ok::<Bytes, Infallible>(bytes), receiver))
        });
        let mut response = Response::new(Body::from_stream(stream));
        response.headers_mut().insert(
            header::CONTENT_TYPE,
            HeaderValue::from_static(PREVIEW_STREAM_CONTENT_TYPE),
        );
        return Ok(response);
    }

    let worker_state = state.clone();
    let worker_cancellation = cancellation.clone();
    let preview = tokio::task::spawn_blocking(move || {
        let result =
            catch_live_preview(
                &spec,
                &map_cache_dir,
                &worker_cancellation,
                |_, _, _| Ok(()),
            );
        finish_preview(&worker_state, &worker_cancellation);
        result
    })
    .await
    .map_err(internal_error)?
    .map_err(internal_error)?;
    Ok(Json(preview).into_response())
}

fn catch_live_preview(
    spec: &GenerationSpec,
    map_cache_dir: &std::path::Path,
    cancellation: &AtomicBool,
    on_progress: impl FnMut(&'static str, &'static str, u8) -> Result<()>,
) -> Result<serde_json::Value> {
    match catch_unwind(AssertUnwindSafe(|| {
        run_live_preview(spec, map_cache_dir, cancellation, on_progress)
    })) {
        Ok(result) => result,
        Err(payload) => Err(anyhow::anyhow!(panic_message(payload))),
    }
}

fn run_live_preview(
    spec: &GenerationSpec,
    map_cache_dir: &std::path::Path,
    cancellation: &AtomicBool,
    mut on_progress: impl FnMut(&'static str, &'static str, u8) -> Result<()>,
) -> Result<serde_json::Value> {
    let mut preview_spec = toposaic_core::model_preview_spec(spec);
    let samples = 128;
    let mut last_elevation_progress = 0;
    on_progress("elevation", "Loading elevation tiles", 4)?;
    let mut height_field = elevation::fetch_preview_height_field_with_progress(
        &preview_spec,
        &map_cache_dir.join("elevation"),
        samples,
        |fraction| {
            ensure_preview_active(cancellation)?;
            let progress = (4.0 + fraction * 24.0).round() as u8;
            if progress > last_elevation_progress {
                on_progress("elevation", "Loading elevation tiles", progress)?;
                last_elevation_progress = progress;
            }
            Ok(())
        },
    )?;
    ensure_preview_active(cancellation)?;

    let needs_surface = preview_spec.color_output.enabled
        || preview_spec.buildings.enabled
        || preview_spec.uses_trails()
        || preview_spec.uses_building_markers();
    let mut surface_field = if needs_surface {
        Some(surface::fetch_surface_field_with_progress(
            &preview_spec,
            &height_field,
            map_cache_dir,
            |label, fraction| {
                ensure_preview_active(cancellation)?;
                on_progress("surface", label, (30.0 + fraction * 42.0).round() as u8)
            },
        )?)
    } else {
        on_progress("surface", "No map overlays needed", 72)?;
        None
    };
    ensure_preview_active(cancellation)?;
    if let Some(field) = surface_field.as_mut() {
        on_progress("surface", "Applying water levels", 74)?;
        surface::apply_marine_water(&preview_spec, &mut height_field, field, map_cache_dir);
    }
    ensure_preview_active(cancellation)?;
    preview_spec = locked_ground_spec(&preview_spec, surface_field.as_ref());
    on_progress("model", "Building draft model", 78)?;
    let preview = toposaic_core::build_model_preview(
        &preview_spec,
        &height_field,
        surface_field.as_ref(),
        samples,
    )?;
    ensure_preview_active(cancellation)?;
    on_progress("model", "Preparing 3D scene", 96)?;
    Ok(preview)
}

fn begin_preview(state: &AppState) -> Result<Arc<AtomicBool>> {
    let cancellation = Arc::new(AtomicBool::new(false));
    let previous = state
        .active_preview
        .lock()
        .map_err(|_| anyhow::anyhow!("active preview lock failed"))?
        .replace(cancellation.clone());
    if let Some(previous) = previous {
        previous.store(true, Ordering::Release);
    }
    Ok(cancellation)
}

fn finish_preview(state: &AppState, cancellation: &Arc<AtomicBool>) {
    let Ok(mut active) = state.active_preview.lock() else {
        return;
    };
    if active
        .as_ref()
        .is_some_and(|current| Arc::ptr_eq(current, cancellation))
    {
        active.take();
    }
}

fn ensure_preview_active(cancellation: &AtomicBool) -> Result<()> {
    if cancellation.load(Ordering::Acquire) {
        anyhow::bail!("preview superseded by newer settings");
    }
    Ok(())
}

fn send_preview_event(
    sender: &tokio::sync::mpsc::Sender<Bytes>,
    cancellation: &AtomicBool,
    event: PreviewStreamEvent,
) -> Result<()> {
    let mut line = serde_json::to_vec(&event)?;
    line.push(b'\n');
    if sender.blocking_send(Bytes::from(line)).is_err() {
        cancellation.store(true, Ordering::Release);
        anyhow::bail!("preview listener closed");
    }
    Ok(())
}

/// The spec with its ground palette pinned to what discovery resolved, or
/// the spec unchanged when no palette was discovered. Cloning is cheap
/// against the generation that follows, and leaves the caller's spec — the
/// one the job row stores — alone.
fn locked_ground_spec(spec: &GenerationSpec, field: Option<&SurfaceField>) -> GenerationSpec {
    let Some(colors) = field.and_then(surface::resolved_ground_colors) else {
        return spec.clone();
    };
    let mut locked = spec.clone();
    locked.color_output.ground_palette.locked_ground_palette = Some(colors);
    locked
}

/// Reports what a finished job's source bundle would hold, so the app can
/// offer the download with a real size instead of a promise.
pub(crate) async fn source_summary(
    State(state): State<AppState>,
    AxumPath(id): AxumPath<String>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<ApiError>)> {
    let (_, output_dir) = finished_job(&state, &id)?;
    let read_dir = output_dir.clone();
    let list = tokio::task::spawn_blocking(move || sources::read_source_list(&read_dir))
        .await
        .map_err(internal_error)?;
    // A job from before this feature has no list. That is not an error the
    // app should show as one; it is a download that is simply not on offer.
    let Ok(list) = list else {
        return Ok(Json(serde_json::json!({ "available": false })));
    };
    let built = output_dir.join(sources::BUNDLE_ARTIFACT_NAME);
    Ok(Json(serde_json::json!({
        "available": !list.files.is_empty(),
        "files": list.files.len(),
        "bytes": list.total_bytes(),
        "name": sources::BUNDLE_ARTIFACT_NAME,
        "built_bytes": fs::metadata(&built).ok().map(|data| data.len()),
    })))
}

/// Builds the source bundle into the job's own directory, where it becomes
/// one of the job's files.
///
/// Written to disk rather than streamed straight back so that every path
/// that already saves a job's files works on it unchanged — the browser
/// download, and the desktop app's native save dialog, which copies from
/// this directory.
///
/// Built on request rather than at generation time: the files it packs can
/// run to hundreds of megabytes, and most jobs never want one.
pub(crate) async fn build_sources(
    State(state): State<AppState>,
    AxumPath(id): AxumPath<String>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<ApiError>)> {
    let (job, output_dir) = finished_job(&state, &id)?;
    let map_cache_dir = state.map_cache_dir.as_ref().clone();
    let bytes = tokio::task::spawn_blocking(move || -> Result<u64> {
        let list = sources::read_source_list(&output_dir)?;
        // The recorded spec is what generation ran; the job row's is what
        // the client asked for, which lacks any palette discovered on the way.
        let spec = list.spec.clone().unwrap_or_else(|| job.spec.clone());
        let bundle = sources::build_bundle(&list, &spec, &map_cache_dir)?;
        let path = output_dir.join(sources::BUNDLE_ARTIFACT_NAME);
        fs::write(&path, &bundle)
            .with_context(|| format!("write the source bundle {}", path.display()))?;
        Ok(bundle.len() as u64)
    })
    .await
    .map_err(internal_error)?
    .map_err(|error| api_error(StatusCode::NOT_FOUND, format!("{error:#}")))?;
    Ok(Json(serde_json::json!({
        "name": sources::BUNDLE_ARTIFACT_NAME,
        "bytes": bytes,
    })))
}

/// Unpacks a source bundle into the map cache and hands back the setup it
/// carried, so the app can load it and generate with no network.
///
/// The body is streamed to a temporary file rather than buffered. A real
/// bundle runs to hundreds of megabytes — the Rainier test case is 65 MB for
/// an 8 km square, most of it one WorldCover tile — and holding that in
/// memory per request is a denial of service waiting to be found. Reading
/// the zip needs seeking anyway, so a file is the natural home.
pub(crate) async fn import_sources(
    State(state): State<AppState>,
    body: Body,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<ApiError>)> {
    let map_cache_dir = state.map_cache_dir.as_ref().clone();
    let upload = state
        .jobs_dir
        .join(format!(".import-{}.zip", Uuid::new_v4()));
    if let Some(parent) = upload.parent() {
        tokio::fs::create_dir_all(parent)
            .await
            .map_err(internal_error)?;
    }
    let written = stream_body_to_file(body, &upload, sources::MAXIMUM_UPLOAD_BYTES).await;
    let result = match written {
        Ok(()) => {
            let path = upload.clone();
            tokio::task::spawn_blocking(move || {
                let file = fs::File::open(&path).context("open the uploaded bundle")?;
                sources::import_bundle(std::io::BufReader::new(file), &map_cache_dir)
            })
            .await
            .map_err(internal_error)?
            // A bad bundle is the caller's file, not a server fault.
            .map_err(|error| api_error(StatusCode::BAD_REQUEST, format!("{error:#}")))
        }
        Err(error) => Err(api_error(StatusCode::BAD_REQUEST, format!("{error:#}"))),
    };
    // The upload is scratch either way.
    let _ = tokio::fs::remove_file(&upload).await;
    let (report, spec) = result?;
    Ok(Json(serde_json::json!({
        "report": report,
        "spec": spec,
    })))
}

/// Writes a request body to `path`, refusing it once it passes `limit` so a
/// long upload cannot fill the disk before anyone inspects it.
async fn stream_body_to_file(body: Body, path: &std::path::Path, limit: u64) -> Result<()> {
    use futures_util::StreamExt;
    use tokio::io::AsyncWriteExt;

    let mut file = tokio::fs::File::create(path)
        .await
        .context("open a temporary file for the upload")?;
    let mut stream = body.into_data_stream();
    let mut total = 0_u64;
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.context("read the uploaded bundle")?;
        total += chunk.len() as u64;
        if total > limit {
            anyhow::bail!(
                "this upload is larger than the {} GB an import accepts",
                limit / (1024 * 1024 * 1024)
            );
        }
        file.write_all(&chunk)
            .await
            .context("write the uploaded bundle")?;
    }
    file.flush().await.context("flush the uploaded bundle")?;
    Ok(())
}

/// The job and its output directory, or a 404. Only a complete job has a
/// source list worth reading.
fn finished_job(
    state: &AppState,
    id: &str,
) -> Result<(Job, std::path::PathBuf), (StatusCode, Json<ApiError>)> {
    let id = canonical_uuid(id).ok_or_else(|| api_error(StatusCode::NOT_FOUND, "job not found"))?;
    let job = find_job(state, &id)
        .map_err(internal_error)?
        .ok_or_else(|| api_error(StatusCode::NOT_FOUND, "job not found"))?;
    if job.status != "complete" {
        return Err(api_error(
            StatusCode::NOT_FOUND,
            "this job has not finished, so its sources are not settled yet",
        ));
    }
    let output_dir = state.jobs_dir.join(&id);
    Ok((job, output_dir))
}

/// The grid's spec with whatever ground palette its tiles shared, for the
/// source list. Without it a rebuilt grid would rediscover per tile.
fn bundled_grid_spec(spec: &GenerationSpec, locked: Option<&[String]>) -> GenerationSpec {
    let mut bundled = spec.clone();
    if let Some(colors) = locked {
        bundled.color_output.ground_palette.locked_ground_palette = Some(colors.to_vec());
    }
    bundled
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

fn format_job_error(error: &anyhow::Error) -> String {
    format!("{error:#}")
}

pub(crate) fn classify_job_error(error: &str) -> JobFailure {
    let lower = error.to_ascii_lowercase();
    let piece = parse_piece_context(error);
    let technical_detail = error.to_owned();

    if lower.contains("flag marker") || lower.contains("flag socket") {
        return JobFailure {
            title: piece
                .map(|piece| format!("Flag socket did not fit piece {},{}", piece.row, piece.column))
                .unwrap_or_else(|| "Flag socket did not fit".into()),
            message: "Reduce the flag-hole diameter or move the flag farther from a puzzle seam, then generate again.".into(),
            technical_detail,
            control_tab: Some(JobControlTab::Markers),
            piece,
        };
    }
    if lower.contains("wall mount")
        || lower.contains("wall plate")
        || lower.contains("cleat")
        || lower.contains("mount pocket")
    {
        return JobFailure {
            title: piece
                .map(|piece| {
                    format!(
                        "Wall mount did not fit piece {},{}",
                        piece.row, piece.column
                    )
                })
                .unwrap_or_else(|| "Wall mount did not fit".into()),
            message:
                "Increase the minimum piece height or reduce the mount depth, then generate again."
                    .into(),
            technical_detail,
            control_tab: Some(JobControlTab::Mounting),
            piece,
        };
    }
    if lower.contains("openstreetmap") || lower.contains("overpass") {
        return JobFailure {
            title: "OpenStreetMap did not return the requested map details".into(),
            message:
                "Try again. TopoSaic will reuse cached inputs and can try another Overpass server."
                    .into(),
            technical_detail,
            control_tab: Some(JobControlTab::Surface),
            piece,
        };
    }
    if lower.contains("elevation") || lower.contains("mapzen") || lower.contains("mapterhorn") {
        return JobFailure {
            title: "Elevation data could not be loaded".into(),
            message: "Try again or choose another elevation source in Model.".into(),
            technical_detail,
            control_tab: Some(JobControlTab::Model),
            piece,
        };
    }
    if (lower.contains("shaped outline") || lower.contains("shaped tray"))
        && (lower.contains("tray") || lower.contains("top-lip"))
    {
        return JobFailure {
            title: "The tray settings do not fit this outline".into(),
            message: "Use one tray segment and turn off its top-lip label, then generate again."
                .into(),
            technical_detail,
            control_tab: Some(JobControlTab::Mounting),
            piece,
        };
    }
    if piece.is_none() && (lower.contains("outline") || lower.contains("super-tile mode")) {
        return JobFailure {
            title: "The model outline could not be built".into(),
            message: "Check the outline in Model. Custom edges must not cross or split one puzzle piece into separate parts."
                .into(),
            technical_detail,
            control_tab: Some(JobControlTab::Model),
            piece,
        };
    }
    if let Some(piece) = piece {
        let conflicting_edge = lower.contains("conflicting edge");
        return JobFailure {
            title: format!("Could not build puzzle piece {},{}", piece.row, piece.column),
            message: if conflicting_edge {
                "Try another puzzle seed or lower the mesh detail, then generate again."
            } else {
                "TopoSaic could not finish this piece. The technical details name the geometry step that failed."
            }
            .into(),
            technical_detail,
            control_tab: Some(JobControlTab::Model),
            piece: Some(piece),
        };
    }
    if lower.contains("write")
        || lower.contains("output directory")
        || lower.contains("3mf")
        || lower.contains("stl")
    {
        return JobFailure {
            title: "Print files could not be written".into(),
            message: "Check free disk space and the destination folder, then generate again."
                .into(),
            technical_detail,
            control_tab: Some(JobControlTab::Output),
            piece: None,
        };
    }
    JobFailure {
        title: "Generation failed".into(),
        message: "TopoSaic stopped before it could finish the model. Open the technical details when reporting this error.".into(),
        technical_detail,
        control_tab: None,
        piece: None,
    }
}

fn parse_piece_context(error: &str) -> Option<JobPiece> {
    let context = error.split("build piece ").nth(1)?;
    let coordinates = context.split(':').next()?.trim();
    let (row, column) = coordinates.split_once(',')?;
    Some(JobPiece {
        row: row.trim().parse().ok()?,
        column: column.trim().parse().ok()?,
    })
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
    let mut height_field = elevation::fetch_height_field_with_progress(
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
    let mut surface_field = if spec.color_output.enabled
        || spec.buildings.enabled
        || spec.uses_trails()
        || spec.uses_building_markers()
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
    if let Some(field) = surface_field.as_mut() {
        surface::apply_marine_water(spec, &mut height_field, field, &state.map_cache_dir);
    }
    // Discovery happened against the imagery; the export builds its filament
    // palette from the spec. Writing the resolved colors back is what joins
    // the two, and it makes the recorded setup reproduce this exact palette
    // rather than rediscover one.
    let spec = &locked_ground_spec(spec, surface_field.as_ref());
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
    // Written before the job is marked complete, so a client that asks for
    // the source bundle the moment it sees "complete" finds the list there.
    sources::write_source_list(
        &output_dir,
        &state.map_cache_dir,
        &manifest_data_sources(&manifest),
        spec,
    );
    update_job(state, id, "complete", 100, &manifest.artifacts, None)?;
    info!(
        job_id = %id,
        elapsed_ms = job_started.elapsed().as_millis() as u64,
        "generation complete"
    );
    Ok(())
}

/// The provider notes a finished manifest carries, for the source list.
fn manifest_data_sources(manifest: &toposaic_core::ProjectManifest) -> Vec<String> {
    let mut notes = vec![manifest.terrain_source.clone()];
    notes.extend(manifest.surface_source.clone());
    notes
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

    // Every part of a super-tile prints on one frame, so it resolves here
    // across the whole footprint rather than per tile. The datum reference
    // and height mode decide it as they do for a lone tile; "the area"
    // just means all the tiles together.
    if spec.elevation_datum_m.is_none() {
        let (minimum, maximum) = height_fields.iter().fold(
            (f32::INFINITY, f32::NEG_INFINITY),
            |(minimum, maximum), field| {
                let (field_minimum, field_maximum) = field.elevation_bounds();
                (minimum.min(field_minimum), maximum.max(field_maximum))
            },
        );
        let frame = height_frame_for_bounds(spec, minimum, maximum);
        for tile in &mut tiles {
            tile.elevation_datum_m = Some(frame.datum_m);
            tile.elevation_m_per_mm = Some(frame.metres_per_mm);
        }
    }

    let output_dir = state.jobs_dir.join(id);
    fs::create_dir_all(&output_dir)
        .with_context(|| format!("create output directory {}", output_dir.display()))?;
    let mut artifacts = Vec::new();
    let mut tile_manifest = Vec::with_capacity(tile_count);
    let mut mosaic_tray_names = Vec::new();
    // Set from the first tile's discovery and handed to every tile after it,
    // so the whole grid prints from one ground palette instead of each tile
    // finding its own and changing filament at the seams.
    let mut ground_palette_lock: Option<Vec<String>> = None;
    // Every tile reads the same providers, so the first tile's notes stand
    // for the set.
    let mut data_sources = Vec::new();
    let mesh_progress = AtomicI64::new(40);

    for (index, ((tile_spec, height_field), tile_output)) in tiles
        .iter()
        .zip(height_fields.iter_mut())
        .zip(&output_plan.tiles)
        .enumerate()
    {
        ensure_job_active(cancellation)?;
        let row = tile_output.row;
        let column = tile_output.column;
        let tile_dir = output_dir.join(&tile_output.temporary_directory);
        // Tile 0 discovers; every later tile is assigned to what it found.
        let locked_tile_spec;
        let tile_spec = match &ground_palette_lock {
            Some(colors) => {
                let mut locked = tile_spec.clone();
                locked.color_output.ground_palette.locked_ground_palette = Some(colors.clone());
                locked_tile_spec = locked;
                &locked_tile_spec
            }
            None => tile_spec,
        };
        let mut surface_field = if tile_spec.color_output.enabled
            || tile_spec.buildings.enabled
            || tile_spec.uses_trails()
            || tile_spec.uses_building_markers()
        {
            Some(surface::fetch_surface_field(
                tile_spec,
                height_field,
                &state.map_cache_dir,
            )?)
        } else {
            None
        };
        // The plane is spec-constant, so every tile flattens to the same
        // level; the frozen ring rule inside keeps the shared edges equal.
        if let Some(field) = surface_field.as_mut() {
            surface::apply_marine_water(tile_spec, height_field, field, &state.map_cache_dir);
        }
        // The first tile discovers a palette; the rest are assigned to it.
        // Discovery per tile would give each its own colors, and a seam
        // where two tiles meet would change filament for no reason on the
        // ground. Written into every later tile's spec before its surface
        // is fetched, which is why this loop sets it on `tiles` rather than
        // on the tile spec in hand.
        if index == 0
            && let Some(colors) = surface_field
                .as_ref()
                .and_then(surface::resolved_ground_colors)
        {
            ground_palette_lock = Some(colors);
        }
        let mut terrain_spec = output_plan.terrain_spec(tile_spec);
        // A super-tile exports each requested flag once. Per-tile generation
        // would build and discard the same files in every temporary folder.
        for marker in terrain_spec
            .markers
            .iter_mut()
            .filter(|marker| matches!(marker.kind, MarkerKind::FlagHole | MarkerKind::FlagLabel))
        {
            let mut style = marker.flag_style.unwrap_or_else(FlagMarkerStyle::default);
            style.export_template = false;
            marker.flag_style = Some(style);
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

        if index == 0 {
            data_sources = manifest_data_sources(&manifest);
        }

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
    artifacts.extend(generate_marker_artifacts(spec, &output_dir)?);

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
    // The first tile's spec carries the palette the whole grid printed
    // from; `spec` here is still the caller's, which does not.
    sources::write_source_list(
        &output_dir,
        &state.map_cache_dir,
        &data_sources,
        &bundled_grid_spec(spec, ground_palette_lock.as_deref()),
    );
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
    fn job_errors_keep_their_context_chain() {
        let error = anyhow::anyhow!("socket crosses a puzzle seam").context("build piece 6, 7");
        assert_eq!(
            format_job_error(&error),
            "build piece 6, 7: socket crosses a puzzle seam"
        );
    }

    #[test]
    fn piece_failures_identify_the_piece_control_and_recovery() {
        let failure = classify_job_error(
            "build piece 6, 7: fit flag marker 'Flag 1' within its puzzle piece: this puzzle piece is too small for the flag socket",
        );
        assert_eq!(failure.title, "Flag socket did not fit piece 6,7");
        assert_eq!(failure.control_tab, Some(JobControlTab::Markers));
        assert_eq!(failure.piece, Some(JobPiece { row: 6, column: 7 }));
        assert!(failure.message.contains("flag-hole diameter"));
        assert!(failure.technical_detail.contains("Flag 1"));

        let failure = classify_job_error(
            "build piece 2, 3: triangulate terrain outline: Conflicting edge encountered",
        );
        assert_eq!(failure.title, "Could not build puzzle piece 2,3");
        assert_eq!(failure.control_tab, Some(JobControlTab::Model));
        assert!(failure.message.contains("puzzle seed"));
    }

    #[test]
    fn download_failures_point_to_the_matching_control() {
        let failure = classify_job_error(
            "OpenStreetMap Overpass rejected the water request: server timed out",
        );
        assert_eq!(failure.control_tab, Some(JobControlTab::Surface));
        assert!(failure.title.contains("OpenStreetMap"));

        let failure = classify_job_error("Mapterhorn elevation tile returned HTTP 503");
        assert_eq!(failure.control_tab, Some(JobControlTab::Model));
        assert!(failure.title.contains("Elevation"));
    }

    #[test]
    fn shaped_outline_failures_point_to_the_matching_control() {
        let failure = classify_job_error("custom outline edges cannot cross");
        assert_eq!(failure.control_tab, Some(JobControlTab::Model));
        assert!(failure.title.contains("outline"));

        let failure = classify_job_error(
            "shaped outlines need a one-piece tray; tray splitting is not yet available",
        );
        assert_eq!(failure.control_tab, Some(JobControlTab::Mounting));
        assert!(failure.title.contains("tray"));

        let failure = classify_job_error(
            "top-lip labels are not yet available on shaped trays; turn off the tray label",
        );
        assert_eq!(failure.control_tab, Some(JobControlTab::Mounting));
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
            failure: None,
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
            failure: None,
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
    fn a_new_preview_cancels_only_the_preview_it_replaced() {
        let state = test_state();
        let first = begin_preview(&state).unwrap();
        let second = begin_preview(&state).unwrap();

        assert!(first.load(Ordering::Acquire));
        assert!(!second.load(Ordering::Acquire));
        finish_preview(&state, &first);
        assert!(
            state
                .active_preview
                .lock()
                .unwrap()
                .as_ref()
                .is_some_and(|active| Arc::ptr_eq(active, &second))
        );
        finish_preview(&state, &second);
        assert!(state.active_preview.lock().unwrap().is_none());
    }

    #[test]
    fn a_closed_preview_stream_cancels_its_worker() {
        let (sender, receiver) = tokio::sync::mpsc::channel(1);
        drop(receiver);
        let cancellation = AtomicBool::new(false);

        assert!(send_preview_event(&sender, &cancellation, PreviewStreamEvent::Canceled).is_err());
        assert!(cancellation.load(Ordering::Acquire));
    }
}
