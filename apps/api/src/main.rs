use std::{
    env,
    path::PathBuf,
    sync::{Arc, Mutex},
};

use anyhow::{Context, Result};
use axum::{
    Json, Router,
    body::Body,
    extract::{Path as AxumPath, State},
    http::{HeaderValue, StatusCode, header},
    response::Response,
    routing::get,
};
use chrono::{DateTime, Utc};
use rusqlite::{Connection, params};
use serde::{Deserialize, Serialize};
use terrain_core::{Artifact, GenerationSpec, artifact_path};
use tokio::net::TcpListener;
use tower_http::{
    cors::{Any, CorsLayer},
    trace::TraceLayer,
};
use tracing::{error, info};
use uuid::Uuid;

mod elevation;

#[derive(Clone)]
struct AppState {
    db: Arc<Mutex<Connection>>,
    jobs_dir: Arc<PathBuf>,
    dem_cache_dir: Arc<PathBuf>,
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

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "terrain_api=info,tower_http=info".into()),
        )
        .init();

    let data_dir = PathBuf::from(env::var("TERRAIN_DATA_DIR").unwrap_or_else(|_| "data".into()));
    let jobs_dir = data_dir.join("jobs");
    std::fs::create_dir_all(&jobs_dir)
        .with_context(|| format!("create jobs directory {}", jobs_dir.display()))?;
    let connection = Connection::open(data_dir.join("terrain-puzzle.sqlite3"))?;
    migrate(&connection)?;

    let state = AppState {
        db: Arc::new(Mutex::new(connection)),
        jobs_dir: Arc::new(jobs_dir),
        dem_cache_dir: Arc::new(data_dir.join("dem-cache")),
    };

    let app = Router::new()
        .route("/api/health", get(health))
        .route("/api/jobs", get(list_jobs).post(create_job))
        .route("/api/jobs/{id}", get(get_job))
        .route("/api/jobs/{id}/downloads/{name}", get(download))
        .layer(
            CorsLayer::new()
                .allow_origin(Any)
                .allow_methods(Any)
                .allow_headers(Any),
        )
        .layer(TraceLayer::new_for_http())
        .with_state(state);

    let address = env::var("TERRAIN_BIND").unwrap_or_else(|_| "127.0.0.1:8787".into());
    let listener = TcpListener::bind(&address).await?;
    info!(%address, "terrain api ready");
    axum::serve(listener, app).await?;
    Ok(())
}

fn migrate(connection: &Connection) -> Result<()> {
    connection.execute_batch(
        r#"
        PRAGMA journal_mode = WAL;
        PRAGMA foreign_keys = ON;
        CREATE TABLE IF NOT EXISTS jobs (
            id TEXT PRIMARY KEY,
            status TEXT NOT NULL,
            progress INTEGER NOT NULL,
            created_at TEXT NOT NULL,
            updated_at TEXT NOT NULL,
            spec_json TEXT NOT NULL,
            artifacts_json TEXT NOT NULL DEFAULT '[]',
            error TEXT
        );
        CREATE INDEX IF NOT EXISTS jobs_created_at_idx ON jobs(created_at DESC);
        UPDATE jobs
        SET status = 'failed',
            progress = 100,
            updated_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now'),
            error = 'Generation was interrupted by a service restart.'
        WHERE status IN ('queued', 'running');
        "#,
    )?;
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

    let worker_state = state.clone();
    tokio::task::spawn_blocking(move || {
        if let Err(error) = run_job(&worker_state, &id, &spec) {
            error!(job_id = %id, %error, "generation failed");
            let _ = update_job(
                &worker_state,
                &id,
                "failed",
                100,
                &[],
                Some(&error.to_string()),
            );
        }
    });

    Ok((StatusCode::ACCEPTED, Json(job)))
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

async fn list_jobs(
    State(state): State<AppState>,
) -> Result<Json<Vec<Job>>, (StatusCode, Json<ApiError>)> {
    let connection = state
        .db
        .lock()
        .map_err(|_| api_error(StatusCode::INTERNAL_SERVER_ERROR, "database lock failed"))?;
    let mut statement = connection
        .prepare(
            "SELECT id, status, progress, created_at, updated_at, spec_json, artifacts_json, error
             FROM jobs ORDER BY created_at DESC LIMIT 20",
        )
        .map_err(internal_error)?;
    let rows = statement
        .query_map([], row_to_job)
        .map_err(internal_error)?;
    let jobs = rows
        .collect::<rusqlite::Result<Vec<_>>>()
        .map_err(internal_error)?;
    Ok(Json(jobs))
}

async fn download(
    State(state): State<AppState>,
    AxumPath((id, name)): AxumPath<(String, String)>,
) -> Result<Response, (StatusCode, Json<ApiError>)> {
    let output_dir = state.jobs_dir.join(&id);
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

fn run_job(state: &AppState, id: &str, spec: &GenerationSpec) -> Result<()> {
    update_job(state, id, "running", 10, &[], None)?;
    let height_field = elevation::fetch_height_field(spec, &state.dem_cache_dir)?;
    update_job(state, id, "running", 55, &[], None)?;
    let output_dir = state.jobs_dir.join(id);
    let manifest =
        terrain_core::generate_project_with_height_field(spec, &height_field, &output_dir)?;
    update_job(state, id, "complete", 100, &manifest.artifacts, None)?;
    Ok(())
}

fn insert_job(state: &AppState, job: &Job) -> Result<()> {
    let connection = state
        .db
        .lock()
        .map_err(|_| anyhow::anyhow!("database lock failed"))?;
    connection.execute(
        "INSERT INTO jobs
         (id, status, progress, created_at, updated_at, spec_json, artifacts_json, error)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
        params![
            job.id,
            job.status,
            job.progress,
            job.created_at.to_rfc3339(),
            job.updated_at.to_rfc3339(),
            serde_json::to_string(&job.spec)?,
            serde_json::to_string(&job.artifacts)?,
            job.error,
        ],
    )?;
    Ok(())
}

fn update_job(
    state: &AppState,
    id: &str,
    status: &str,
    progress: i64,
    artifacts: &[Artifact],
    error: Option<&str>,
) -> Result<()> {
    let connection = state
        .db
        .lock()
        .map_err(|_| anyhow::anyhow!("database lock failed"))?;
    connection.execute(
        "UPDATE jobs SET status = ?2, progress = ?3, updated_at = ?4,
         artifacts_json = ?5, error = ?6 WHERE id = ?1",
        params![
            id,
            status,
            progress,
            Utc::now().to_rfc3339(),
            serde_json::to_string(artifacts)?,
            error,
        ],
    )?;
    Ok(())
}

fn find_job(state: &AppState, id: &str) -> Result<Option<Job>> {
    let connection = state
        .db
        .lock()
        .map_err(|_| anyhow::anyhow!("database lock failed"))?;
    let mut statement = connection.prepare(
        "SELECT id, status, progress, created_at, updated_at, spec_json, artifacts_json, error
         FROM jobs WHERE id = ?1",
    )?;
    let mut rows = statement.query([id])?;
    rows.next()?.map(row_to_job).transpose().map_err(Into::into)
}

fn row_to_job(row: &rusqlite::Row<'_>) -> rusqlite::Result<Job> {
    let created_at: String = row.get(3)?;
    let updated_at: String = row.get(4)?;
    let spec_json: String = row.get(5)?;
    let artifacts_json: String = row.get(6)?;
    Ok(Job {
        id: row.get(0)?,
        status: row.get(1)?,
        progress: row.get(2)?,
        created_at: created_at.parse().map_err(sql_conversion_error)?,
        updated_at: updated_at.parse().map_err(sql_conversion_error)?,
        spec: serde_json::from_str(&spec_json).map_err(sql_conversion_error)?,
        artifacts: serde_json::from_str(&artifacts_json).map_err(sql_conversion_error)?,
        error: row.get(7)?,
    })
}

fn sql_conversion_error(error: impl std::error::Error + Send + Sync + 'static) -> rusqlite::Error {
    rusqlite::Error::FromSqlConversionFailure(0, rusqlite::types::Type::Text, Box::new(error))
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
