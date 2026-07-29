use std::{
    ffi::OsStr,
    fs,
    fs::OpenOptions,
    io::{Read, Write},
    path::{Path, PathBuf},
    time::{Duration, SystemTime},
};

use anyhow::{Context, Result, anyhow};
use axum::{Json, extract::State, http::StatusCode};
use chrono::{DateTime, TimeDelta, Utc};
use directories::ProjectDirs;
use rusqlite::params;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::{ApiError, AppState, internal_error, settings};

pub fn root() -> Result<PathBuf> {
    if let Some(path) = settings::cache_dir_override() {
        return Ok(path);
    }
    ProjectDirs::from("com", "theatrus", "toposaic")
        .map(|directories| directories.cache_dir().to_path_buf())
        .context("find the OS cache directory; set TOPOSAIC_CACHE_DIR to choose one")
}

pub fn store(path: &Path, bytes: &[u8]) -> Result<()> {
    store_reader(path, bytes)
}

pub fn store_reader(path: &Path, mut reader: impl Read) -> Result<()> {
    if path.is_file() {
        return Ok(());
    }
    let parent = path
        .parent()
        .context("cached input path has no parent directory")?;
    fs::create_dir_all(parent)
        .with_context(|| format!("create input cache directory {}", parent.display()))?;
    let file_name = path
        .file_name()
        .and_then(OsStr::to_str)
        .context("cached input path has no file name")?;
    let temporary = parent.join(format!(".{file_name}.{}.part", Uuid::new_v4()));
    let result = (|| -> Result<()> {
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temporary)
            .with_context(|| format!("create temporary cache file {}", temporary.display()))?;
        std::io::copy(&mut reader, &mut file)
            .with_context(|| format!("write temporary cache file {}", temporary.display()))?;
        file.flush()?;
        file.sync_all()?;
        if path.is_file() {
            return Ok(());
        }
        match fs::rename(&temporary, path) {
            Ok(()) => Ok(()),
            Err(_) if path.is_file() => Ok(()),
            Err(error) => {
                Err(error).with_context(|| format!("publish cached input {}", path.display()))
            }
        }
    })();
    if temporary.exists() {
        fs::remove_file(&temporary)
            .with_context(|| format!("remove temporary cache file {}", temporary.display()))?;
    }
    result
}

/// File-backed cache categories: the response key and the subdirectory of
/// the map cache root it measures. The walks are recursive, so the
/// elevation category covers its mapterhorn subdirectory. The jobs
/// directory and the jobs table are user data, not cache, and are never in
/// scope here.
const FILE_CATEGORIES: [(&str, &str); 4] = [
    ("elevation", "elevation"),
    ("world_cover", "world-cover"),
    ("osm", "osm"),
    ("datum", "datum"),
];

#[derive(Debug, Serialize)]
pub(crate) struct CacheCategory {
    key: &'static str,
    bytes: u64,
    entries: u64,
}

#[derive(Debug, Serialize)]
pub(crate) struct CacheSummary {
    total_bytes: u64,
    categories: Vec<CacheCategory>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct ClearCacheRequest {
    older_than_days: Option<u32>,
}

#[derive(Debug, Serialize)]
pub(crate) struct ClearCacheResponse {
    removed_bytes: u64,
    removed_entries: u64,
}

pub(crate) async fn cache_summary(
    State(state): State<AppState>,
) -> Result<Json<CacheSummary>, (StatusCode, Json<ApiError>)> {
    let map_cache_dir = state.map_cache_dir.clone();
    // The walks touch potentially thousands of files, so they run off the
    // async workers.
    let mut categories = tokio::task::spawn_blocking(move || {
        Vec::from(FILE_CATEGORIES.map(|(key, directory)| {
            let (bytes, entries) = directory_stats(&map_cache_dir.join(directory));
            CacheCategory {
                key,
                bytes,
                entries,
            }
        }))
    })
    .await
    .map_err(internal_error)?;
    let (bytes, entries) = places_cache_stats(&state).map_err(internal_error)?;
    categories.push(CacheCategory {
        key: "places",
        bytes,
        entries,
    });
    let total_bytes = categories.iter().map(|category| category.bytes).sum();
    Ok(Json(CacheSummary {
        total_bytes,
        categories,
    }))
}

pub(crate) async fn clear_cache(
    State(state): State<AppState>,
    Json(request): Json<ClearCacheRequest>,
) -> Result<Json<ClearCacheResponse>, (StatusCode, Json<ApiError>)> {
    let file_cutoff = request.older_than_days.map(file_cutoff);
    let map_cache_dir = state.map_cache_dir.clone();
    let (mut removed_bytes, mut removed_entries) = tokio::task::spawn_blocking(move || {
        let mut bytes = 0;
        let mut entries = 0;
        for (_, directory) in FILE_CATEGORIES {
            let (removed_bytes, removed_entries) =
                clear_directory(&map_cache_dir.join(directory), file_cutoff);
            bytes += removed_bytes;
            entries += removed_entries;
        }
        (bytes, entries)
    })
    .await
    .map_err(internal_error)?;
    let (bytes, entries) = clear_places_cache(&state, request.older_than_days.map(row_cutoff))
        .map_err(internal_error)?;
    removed_bytes += bytes;
    removed_entries += entries;
    Ok(Json(ClearCacheResponse {
        removed_bytes,
        removed_entries,
    }))
}

/// Sums file sizes and counts files under `directory`, recursively. Entries
/// that vanish or cannot be read mid-walk are skipped: the summary is a
/// best-effort display, not an audit.
fn directory_stats(directory: &Path) -> (u64, u64) {
    let mut bytes = 0;
    let mut entries = 0;
    let Ok(directory_entries) = fs::read_dir(directory) else {
        return (0, 0);
    };
    for entry in directory_entries.flatten() {
        let Ok(file_type) = entry.file_type() else {
            continue;
        };
        if file_type.is_dir() {
            let (child_bytes, child_entries) = directory_stats(&entry.path());
            bytes += child_bytes;
            entries += child_entries;
        } else if let Ok(metadata) = entry.metadata() {
            bytes += metadata.len();
            entries += 1;
        }
    }
    (bytes, entries)
}

/// Sizes the place-search cache. `SUM(LENGTH(response_json))` counts the
/// cached payload text — an honest approximation that ignores SQLite's
/// per-row overhead, which is small next to the JSON bodies.
fn places_cache_stats(state: &AppState) -> Result<(u64, u64)> {
    let connection = state
        .db
        .lock()
        .map_err(|_| anyhow!("database lock failed"))?;
    let (bytes, entries) = connection.query_row(
        "SELECT COALESCE(SUM(LENGTH(response_json)), 0), COUNT(*) FROM place_search_cache",
        [],
        |row| Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?)),
    )?;
    Ok((bytes.max(0) as u64, entries.max(0) as u64))
}

/// The moment before which files count as "older than `days`". A cutoff
/// that would fall before the epoch clamps to the epoch and removes
/// nothing, since no file is older.
fn file_cutoff(days: u32) -> SystemTime {
    SystemTime::now()
        .checked_sub(Duration::from_secs(u64::from(days) * 86_400))
        .unwrap_or(SystemTime::UNIX_EPOCH)
}

fn row_cutoff(days: u32) -> DateTime<Utc> {
    Utc::now()
        .checked_sub_signed(TimeDelta::days(i64::from(days)))
        .unwrap_or(DateTime::UNIX_EPOCH)
}

/// The age rule for one cache file: a clear-all (`cutoff` None) removes
/// every file; an age-filtered clear removes only files modified before the
/// cutoff and skips files whose modified time cannot be read rather than
/// guessing.
fn should_remove(modified: Option<SystemTime>, cutoff: Option<SystemTime>) -> bool {
    match cutoff {
        None => true,
        Some(cutoff) => modified.is_some_and(|modified| modified < cutoff),
    }
}

/// Removes in-scope files under `directory`, then prunes directories the
/// removals left empty — including `directory` itself, which `store`
/// re-creates on demand.
///
/// Safety against a concurrently running generation: on POSIX, unlinking a
/// published cache file a generator holds open leaves the generator reading
/// the old inode; the next generation simply re-downloads it. The age rule
/// applies to `store_reader`'s `.part` temporaries like any other file, so
/// a fresh in-flight temporary survives an "older than N days" clear on its
/// own mtime. A clear-all may unlink an in-flight temporary, and that is
/// still safe: `store_reader` writes through `create_new` + `rename`, so
/// the writer keeps filling the unlinked inode, its `rename` fails, and
/// that one store reports an error without ever publishing a partial file —
/// the data is fetched again later.
fn clear_directory(directory: &Path, cutoff: Option<SystemTime>) -> (u64, u64) {
    let mut bytes = 0;
    let mut entries = 0;
    let Ok(directory_entries) = fs::read_dir(directory) else {
        return (0, 0);
    };
    for entry in directory_entries.flatten() {
        let Ok(file_type) = entry.file_type() else {
            continue;
        };
        if file_type.is_dir() {
            let (child_bytes, child_entries) = clear_directory(&entry.path(), cutoff);
            bytes += child_bytes;
            entries += child_entries;
            continue;
        }
        let Ok(metadata) = entry.metadata() else {
            continue;
        };
        if should_remove(metadata.modified().ok(), cutoff) && fs::remove_file(entry.path()).is_ok()
        {
            bytes += metadata.len();
            entries += 1;
        }
    }
    // remove_dir refuses a non-empty directory, so a failure here is the
    // keep signal, not an error.
    let _ = fs::remove_dir(directory);
    (bytes, entries)
}

/// Deletes in-scope place-search rows and reports what went away, in one
/// statement so the tally always matches the rows removed. `fetched_at`
/// stores RFC 3339 UTC strings, which order lexicographically — the same
/// idiom the jobs table uses for `created_at`.
fn clear_places_cache(state: &AppState, cutoff: Option<DateTime<Utc>>) -> Result<(u64, u64)> {
    let connection = state
        .db
        .lock()
        .map_err(|_| anyhow!("database lock failed"))?;
    let cutoff = cutoff.map(|cutoff| cutoff.to_rfc3339());
    let mut statement = connection.prepare(
        "DELETE FROM place_search_cache
         WHERE ?1 IS NULL OR fetched_at < ?1
         RETURNING LENGTH(response_json)",
    )?;
    let lengths = statement.query_map(params![cutoff], |row| row.get::<_, i64>(0))?;
    let mut bytes = 0;
    let mut entries = 0;
    for length in lengths {
        bytes += length?.max(0) as u64;
        entries += 1;
    }
    Ok((bytes, entries))
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;
    use crate::test_state;

    #[test]
    fn atomically_stores_cached_input() {
        let directory =
            std::env::temp_dir().join(format!("toposaic-cache-test-{}", Uuid::new_v4()));
        let path = directory.join("tiles").join("sample.bin");
        store(&path, b"first").unwrap();
        store(&path, b"second").unwrap();
        assert_eq!(fs::read(&path).unwrap(), b"first");
        fs::remove_dir_all(directory).unwrap();
    }

    /// `test_state` shares one per-process directory; these tests delete
    /// files, so each takes its own.
    fn isolated_state() -> (AppState, PathBuf) {
        let mut state = test_state();
        let directory =
            std::env::temp_dir().join(format!("toposaic-cache-admin-test-{}", Uuid::new_v4()));
        state.map_cache_dir = Arc::new(directory.join("cache"));
        state.jobs_dir = Arc::new(directory.join("jobs"));
        (state, directory)
    }

    fn write_file(path: &Path, bytes: &[u8]) {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, bytes).unwrap();
    }

    fn backdate(path: &Path, days: u64) {
        let modified = SystemTime::now() - Duration::from_secs(days * 86_400);
        let file = OpenOptions::new().append(true).open(path).unwrap();
        file.set_times(fs::FileTimes::new().set_modified(modified))
            .unwrap();
    }

    fn insert_place_row(state: &AppState, query: &str, response: &str, fetched_at: DateTime<Utc>) {
        state
            .db
            .lock()
            .unwrap()
            .execute(
                "INSERT INTO place_search_cache (query, response_json, fetched_at)
                 VALUES (?1, ?2, ?3)",
                params![query, response, fetched_at.to_rfc3339()],
            )
            .unwrap();
    }

    fn place_queries(state: &AppState) -> Vec<String> {
        let connection = state.db.lock().unwrap();
        let mut statement = connection
            .prepare("SELECT query FROM place_search_cache ORDER BY query")
            .unwrap();
        let rows = statement
            .query_map([], |row| row.get::<_, String>(0))
            .unwrap();
        rows.collect::<rusqlite::Result<Vec<_>>>().unwrap()
    }

    async fn summary(state: &AppState) -> CacheSummary {
        cache_summary(State(state.clone())).await.unwrap().0
    }

    async fn clear(state: &AppState, older_than_days: Option<u32>) -> ClearCacheResponse {
        clear_cache(
            State(state.clone()),
            Json(ClearCacheRequest { older_than_days }),
        )
        .await
        .unwrap()
        .0
    }

    fn category<'a>(summary: &'a CacheSummary, key: &str) -> &'a CacheCategory {
        summary
            .categories
            .iter()
            .find(|category| category.key == key)
            .unwrap()
    }

    #[test]
    fn clear_all_removes_every_file_and_age_filter_spares_fresh_and_unreadable() {
        let old = Some(SystemTime::now() - Duration::from_secs(10 * 86_400));
        let fresh = Some(SystemTime::now());
        let cutoff = Some(SystemTime::now() - Duration::from_secs(5 * 86_400));

        assert!(should_remove(old, None));
        assert!(should_remove(fresh, None));
        assert!(should_remove(None, None));
        assert!(should_remove(old, cutoff));
        assert!(!should_remove(fresh, cutoff));
        assert!(!should_remove(None, cutoff), "unreadable mtime is skipped");
    }

    #[tokio::test]
    async fn summary_totals_each_category_including_places_rows() {
        let (state, directory) = isolated_state();
        let cache_dir = state.map_cache_dir.as_ref().clone();
        write_file(&cache_dir.join("elevation/8/1/2.png"), &[0; 10]);
        write_file(&cache_dir.join("elevation/mapterhorn/8/1/2.webp"), &[0; 20]);
        write_file(&cache_dir.join("world-cover/tile-a.tif"), &[0; 7]);
        write_file(&cache_dir.join("osm/roads-v2-a.json"), &[0; 3]);
        write_file(&cache_dir.join("osm/water-a.json"), &[0; 4]);
        // Out of scope: never counted, never cleared.
        write_file(&state.jobs_dir.join("job-1/model.3mf"), &[0; 999]);
        insert_place_row(&state, "rainier", "12345678", Utc::now());

        let summary = summary(&state).await;
        assert_eq!(category(&summary, "elevation").bytes, 30);
        assert_eq!(category(&summary, "elevation").entries, 2);
        assert_eq!(category(&summary, "world_cover").bytes, 7);
        assert_eq!(category(&summary, "world_cover").entries, 1);
        assert_eq!(category(&summary, "osm").bytes, 7);
        assert_eq!(category(&summary, "osm").entries, 2);
        assert_eq!(category(&summary, "places").bytes, 8);
        assert_eq!(category(&summary, "places").entries, 1);
        assert_eq!(summary.total_bytes, 30 + 7 + 7 + 8);

        fs::remove_dir_all(directory).unwrap();
    }

    #[tokio::test]
    async fn missing_cache_directories_report_empty() {
        let (state, _directory) = isolated_state();
        let summary = summary(&state).await;
        assert_eq!(summary.total_bytes, 0);
        assert_eq!(summary.categories.len(), 5);
        assert!(
            summary
                .categories
                .iter()
                .all(|category| category.bytes == 0)
        );
    }

    #[tokio::test]
    async fn clear_all_empties_the_cache_but_never_touches_jobs() {
        let (state, directory) = isolated_state();
        let cache_dir = state.map_cache_dir.as_ref().clone();
        write_file(&cache_dir.join("elevation/8/1/2.png"), &[0; 10]);
        write_file(&cache_dir.join("elevation/mapterhorn/8/1/2.webp"), &[0; 20]);
        write_file(&cache_dir.join("world-cover/tile-a.tif"), &[0; 7]);
        write_file(&cache_dir.join("osm/.roads-v2-a.json.abc.part"), &[0; 5]);
        write_file(&cache_dir.join("datum/coops-stations-v1.json"), &[0; 9]);
        let job_file = state.jobs_dir.join("job-1/model.3mf");
        write_file(&job_file, &[0; 999]);
        insert_place_row(&state, "rainier", "12345678", Utc::now());

        let cleared = clear(&state, None).await;
        assert_eq!(cleared.removed_bytes, 10 + 20 + 7 + 5 + 9 + 8);
        assert_eq!(cleared.removed_entries, 6);
        assert!(!cache_dir.join("elevation").exists(), "empty dirs pruned");
        assert!(!cache_dir.join("world-cover").exists());
        assert!(!cache_dir.join("osm").exists());
        assert!(!cache_dir.join("datum").exists());
        assert!(job_file.exists(), "jobs are user data, not cache");
        assert!(place_queries(&state).is_empty());
        assert_eq!(summary(&state).await.total_bytes, 0);

        fs::remove_dir_all(directory).unwrap();
    }

    #[tokio::test]
    async fn clear_old_removes_only_backdated_files_and_rows() {
        let (state, directory) = isolated_state();
        let cache_dir = state.map_cache_dir.as_ref().clone();
        let old_tile = cache_dir.join("elevation/8/1/2.png");
        let fresh_tile = cache_dir.join("elevation/8/1/3.png");
        let old_temporary = cache_dir.join("osm/.roads-v2-a.json.abc.part");
        let fresh_temporary = cache_dir.join("osm/.roads-v2-b.json.def.part");
        write_file(&old_tile, &[0; 10]);
        write_file(&fresh_tile, &[0; 20]);
        write_file(&old_temporary, &[0; 5]);
        write_file(&fresh_temporary, &[0; 6]);
        backdate(&old_tile, 10);
        backdate(&old_temporary, 10);
        insert_place_row(&state, "old", "1234", Utc::now() - TimeDelta::days(10));
        insert_place_row(&state, "fresh", "123456", Utc::now());

        let cleared = clear(&state, Some(5)).await;
        assert_eq!(cleared.removed_bytes, 10 + 5 + 4);
        assert_eq!(cleared.removed_entries, 3);
        assert!(!old_tile.exists());
        assert!(fresh_tile.exists());
        assert!(!old_temporary.exists());
        assert!(fresh_temporary.exists(), "in-flight temporaries stay");
        assert_eq!(place_queries(&state), ["fresh"]);

        // Pruning stops at the first directory a fresh file keeps alive.
        assert!(cache_dir.join("elevation/8/1").exists());

        fs::remove_dir_all(directory).unwrap();
    }
}
