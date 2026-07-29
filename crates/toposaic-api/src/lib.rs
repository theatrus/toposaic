use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::{Arc, Mutex as StdMutex, atomic::AtomicBool},
    time::{Duration, Instant},
};

use anyhow::{Context, Result};
use axum::{Json, Router, http::StatusCode, routing::get};
use reqwest::Client;
use rusqlite::Connection;
use serde::Serialize;
use tokio::{net::TcpListener, sync::Mutex as AsyncMutex};
use tower_http::trace::TraceLayer;
use tracing::{info, warn};

mod cache;
mod database;
mod elevation;
mod geo;
mod geocoding;
mod grid;
mod http;
mod jobs;
mod settings;
mod setups;
mod surface;

/// Internal hooks for the `manifold_report_real` diagnostics example only;
/// not a stable API.
#[doc(hidden)]
pub mod diagnostics {
    pub use crate::cache::root as map_cache_root;
    pub use crate::elevation::fetch_height_field_with_progress;
    pub use crate::surface::{apply_marine_water, fetch_surface_field};
}

use database::migrate;
use geocoding::search_places;
pub(crate) use jobs::Job;
pub(crate) use setups::{SavedSetup, SetupVersion};

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
    sweep_legacy_osm_cache(&map_cache_dir);
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
        .route("/api/cache", get(cache::cache_summary))
        .route("/api/cache/clear", axum::routing::post(cache::clear_cache))
        .route("/api/places", get(search_places))
        .route("/api/preview", axum::routing::post(jobs::create_preview))
        .route("/api/jobs", get(jobs::list_jobs).post(jobs::create_job))
        .route(
            "/api/jobs/{id}",
            get(jobs::get_job).delete(jobs::cancel_job),
        )
        .route("/api/jobs/{id}/downloads/{name}", get(jobs::download))
        .route(
            "/api/setups",
            get(setups::list_setups).post(setups::save_setup),
        )
        .route(
            "/api/setups/{id}",
            axum::routing::delete(setups::delete_setup).patch(setups::rename_setup),
        )
        .route("/api/setups/{id}/versions", get(setups::list_versions))
        .route(
            "/api/setups/{id}/versions/{version_id}/restore",
            axum::routing::post(setups::restore_version),
        )
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

/// The road cache prefixes moved to roads-v2-*, and the rail ones to
/// rail-v2-* when railways and aerialways split into separate fetches — a
/// rail-v1 response carried both key families, so it can never answer a
/// railway-only request. Files with any of the retired prefixes can never be
/// read again, so drop them once at startup instead of letting them sit in
/// the cache forever.
fn sweep_legacy_osm_cache(map_cache_dir: &Path) {
    let Ok(entries) = std::fs::read_dir(map_cache_dir.join("osm")) else {
        return;
    };
    for entry in entries.flatten() {
        let name = entry.file_name();
        let Some(name) = name.to_str() else { continue };
        let legacy = (name.starts_with("roads-") && !name.starts_with("roads-v2-"))
            || name.starts_with("trails-")
            || name.starts_with("rail-v1-");
        if legacy && let Err(error) = std::fs::remove_file(entry.path()) {
            warn!(
                %error,
                file = %entry.path().display(),
                "could not remove a legacy OpenStreetMap cache entry"
            );
        }
    }
}

async fn health() -> Json<Health> {
    Json(Health {
        status: "ok",
        storage: "sqlite",
    })
}

#[cfg(test)]
pub(crate) fn test_state() -> AppState {
    let connection = Connection::open_in_memory().unwrap();
    migrate(&connection).unwrap();
    let data_dir = std::env::temp_dir().join(format!("toposaic-api-test-{}", std::process::id()));
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

/// Parses a client-supplied id into its canonical hyphenated UUID form.
/// Jobs and setups both key their rows (and, for jobs, their artifact
/// directories) by this form, so path escapes like `../data` parse to
/// `None` and map to a 404.
pub(crate) fn canonical_uuid(id: &str) -> Option<String> {
    uuid::Uuid::parse_str(id)
        .ok()
        .map(|value| value.hyphenated().to_string())
}
