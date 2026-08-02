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
mod datum;
mod elevation;
mod geo;
mod geocoding;
mod grid;
mod http;
mod imagery;
mod jobs;
mod osm;
mod settings;
mod setups;
mod sources;
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
    /// The live preview is replaceable work: a newer request cancels the
    /// older one instead of leaving a queue of OSM fetches behind it.
    active_preview: Arc<StdMutex<Option<ActivePreview>>>,
}

#[derive(Clone)]
struct ActivePreview {
    id: String,
    cancellation: Arc<AtomicBool>,
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
        active_preview: Arc::new(StdMutex::new(None)),
    };

    let app = Router::new()
        .route("/api/health", get(health))
        .route("/api/cache", get(cache::cache_summary))
        .route("/api/cache/clear", axum::routing::post(cache::clear_cache))
        .route("/api/places", get(search_places))
        .route("/api/preview", axum::routing::post(jobs::create_preview))
        .route(
            "/api/preview/{id}",
            axum::routing::delete(jobs::cancel_preview),
        )
        .route("/api/jobs", get(jobs::list_jobs).post(jobs::create_job))
        .route(
            "/api/jobs/{id}",
            get(jobs::get_job).delete(jobs::cancel_job),
        )
        .route("/api/jobs/{id}/downloads/{name}", get(jobs::download))
        .route("/api/jobs/{id}/sources", get(jobs::source_summary))
        .route(
            "/api/jobs/{id}/sources/build",
            axum::routing::post(jobs::build_sources),
        )
        .route(
            "/api/sources/import",
            axum::routing::post(jobs::import_sources)
                // The handler streams the upload to a temporary file and
                // enforces its own cap; axum's 2 MB default would refuse
                // every real bundle before the handler ever saw it.
                .layer(axum::extract::DefaultBodyLimit::disable()),
        )
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

/// Overpass data now lives below `osm/tiles`. Every old cache response was
/// keyed by an exact bounding box and sat as a file directly below `osm`, so
/// no current fetch can read it. Drop those retired files at startup rather
/// than counting dead data forever; the nested tile directory stays intact.
fn sweep_legacy_osm_cache(map_cache_dir: &Path) {
    let Ok(entries) = std::fs::read_dir(map_cache_dir.join("osm")) else {
        return;
    };
    for entry in entries.flatten() {
        let Ok(file_type) = entry.file_type() else {
            continue;
        };
        if file_type.is_file()
            && let Err(error) = std::fs::remove_file(entry.path())
        {
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
        active_preview: Arc::new(StdMutex::new(None)),
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn osm_cache_migration_removes_bbox_files_and_keeps_tiles() {
        let root = std::env::temp_dir().join(format!(
            "toposaic-osm-migration-test-{}",
            uuid::Uuid::new_v4()
        ));
        let osm = root.join("osm");
        let legacy = osm.join("roads-v2-old.json");
        let tile = osm.join("tiles/roads-v3-major/10/164/353.json");
        std::fs::create_dir_all(tile.parent().unwrap()).unwrap();
        std::fs::write(&legacy, b"old").unwrap();
        std::fs::write(&tile, b"current").unwrap();

        sweep_legacy_osm_cache(&root);

        assert!(!legacy.exists());
        assert!(tile.exists());
        std::fs::remove_dir_all(root).unwrap();
    }
}
