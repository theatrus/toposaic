use std::sync::MutexGuard;

use anyhow::{Result, anyhow};
use chrono::{DateTime, Utc};
use rusqlite::{Connection, params};
use toposaic_core::Artifact;

use crate::{AppState, Job, SavedSetup};

pub fn migrate(connection: &Connection) -> Result<()> {
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
        CREATE TABLE IF NOT EXISTS saved_setups (
            id TEXT PRIMARY KEY,
            name TEXT NOT NULL UNIQUE,
            created_at TEXT NOT NULL,
            updated_at TEXT NOT NULL,
            spec_json TEXT NOT NULL
        );
        CREATE INDEX IF NOT EXISTS saved_setups_updated_at_idx ON saved_setups(updated_at DESC);
        CREATE TABLE IF NOT EXISTS place_search_cache (
            query TEXT PRIMARY KEY,
            response_json TEXT NOT NULL,
            fetched_at TEXT NOT NULL
        );
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

pub fn insert_job(state: &AppState, job: &Job) -> Result<()> {
    connection(state)?.execute(
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

pub fn update_job(
    state: &AppState,
    id: &str,
    status: &str,
    progress: i64,
    artifacts: &[Artifact],
    error: Option<&str>,
) -> Result<()> {
    connection(state)?.execute(
        "UPDATE jobs SET status = ?2, progress = ?3, updated_at = ?4,
         artifacts_json = ?5, error = ?6
         WHERE id = ?1 AND status != 'canceled'",
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

pub fn mark_job_canceled(state: &AppState, id: &str) -> Result<bool> {
    let updated = connection(state)?.execute(
        "UPDATE jobs
         SET status = 'canceled', updated_at = ?2, artifacts_json = '[]',
             error = NULL
         WHERE id = ?1 AND status IN ('queued', 'running')",
        params![id, Utc::now().to_rfc3339()],
    )?;
    Ok(updated == 1)
}

pub fn find_job(state: &AppState, id: &str) -> Result<Option<Job>> {
    let connection = connection(state)?;
    let mut statement = connection.prepare(
        "SELECT id, status, progress, created_at, updated_at, spec_json, artifacts_json, error
         FROM jobs WHERE id = ?1",
    )?;
    let mut rows = statement.query([id])?;
    rows.next()?.map(row_to_job).transpose().map_err(Into::into)
}

pub fn recent_jobs(state: &AppState, limit: usize) -> Result<Vec<Job>> {
    let connection = connection(state)?;
    let mut statement = connection.prepare(
        "SELECT id, status, progress, created_at, updated_at, spec_json, artifacts_json, error
         FROM jobs ORDER BY created_at DESC LIMIT ?1",
    )?;
    let rows = statement.query_map([limit as i64], row_to_job)?;
    rows.collect::<rusqlite::Result<Vec<_>>>()
        .map_err(Into::into)
}

pub fn insert_saved_setup(state: &AppState, setup: &SavedSetup) -> Result<()> {
    connection(state)?.execute(
        "INSERT INTO saved_setups (id, name, created_at, updated_at, spec_json)
         VALUES (?1, ?2, ?3, ?4, ?5)",
        params![
            setup.id,
            setup.name,
            setup.created_at.to_rfc3339(),
            setup.updated_at.to_rfc3339(),
            serde_json::to_string(&setup.spec)?,
        ],
    )?;
    Ok(())
}

pub fn update_saved_setup(state: &AppState, setup: &SavedSetup) -> Result<()> {
    connection(state)?.execute(
        "UPDATE saved_setups SET spec_json = ?2, updated_at = ?3 WHERE id = ?1",
        params![
            setup.id,
            serde_json::to_string(&setup.spec)?,
            setup.updated_at.to_rfc3339(),
        ],
    )?;
    Ok(())
}

/// Renames the setup and returns the updated row, or `None` when no setup
/// has the given id.
pub fn rename_saved_setup(
    state: &AppState,
    id: &str,
    name: &str,
    updated_at: DateTime<Utc>,
) -> Result<Option<SavedSetup>> {
    let connection = connection(state)?;
    let updated = connection.execute(
        "UPDATE saved_setups SET name = ?2, updated_at = ?3 WHERE id = ?1",
        params![id, name, updated_at.to_rfc3339()],
    )?;
    if updated != 1 {
        return Ok(None);
    }
    let mut statement = connection.prepare(
        "SELECT id, name, created_at, updated_at, spec_json
         FROM saved_setups WHERE id = ?1",
    )?;
    let mut rows = statement.query([id])?;
    rows.next()?
        .map(row_to_saved_setup)
        .transpose()
        .map_err(Into::into)
}

pub fn find_saved_setup_by_name(state: &AppState, name: &str) -> Result<Option<SavedSetup>> {
    let connection = connection(state)?;
    let mut statement = connection.prepare(
        "SELECT id, name, created_at, updated_at, spec_json
         FROM saved_setups WHERE name = ?1",
    )?;
    let mut rows = statement.query([name])?;
    rows.next()?
        .map(row_to_saved_setup)
        .transpose()
        .map_err(Into::into)
}

pub fn list_saved_setups(state: &AppState) -> Result<Vec<SavedSetup>> {
    let connection = connection(state)?;
    let mut statement = connection.prepare(
        "SELECT id, name, created_at, updated_at, spec_json
         FROM saved_setups ORDER BY updated_at DESC",
    )?;
    let rows = statement.query_map([], row_to_saved_setup)?;
    rows.collect::<rusqlite::Result<Vec<_>>>()
        .map_err(Into::into)
}

pub fn delete_saved_setup(state: &AppState, id: &str) -> Result<bool> {
    let deleted =
        connection(state)?.execute("DELETE FROM saved_setups WHERE id = ?1", params![id])?;
    Ok(deleted == 1)
}

fn connection(state: &AppState) -> Result<MutexGuard<'_, Connection>> {
    state.db.lock().map_err(|_| anyhow!("database lock failed"))
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

fn row_to_saved_setup(row: &rusqlite::Row<'_>) -> rusqlite::Result<SavedSetup> {
    let created_at: String = row.get(2)?;
    let updated_at: String = row.get(3)?;
    let spec_json: String = row.get(4)?;
    Ok(SavedSetup {
        id: row.get(0)?,
        name: row.get(1)?,
        created_at: created_at.parse().map_err(sql_conversion_error)?,
        updated_at: updated_at.parse().map_err(sql_conversion_error)?,
        spec: serde_json::from_str(&spec_json).map_err(sql_conversion_error)?,
    })
}

pub fn sql_conversion_error(
    error: impl std::error::Error + Send + Sync + 'static,
) -> rusqlite::Error {
    rusqlite::Error::FromSqlConversionFailure(0, rusqlite::types::Type::Text, Box::new(error))
}
