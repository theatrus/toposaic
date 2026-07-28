use std::sync::MutexGuard;

use anyhow::{Result, anyhow};
use chrono::{DateTime, Utc};
use rusqlite::{Connection, params};
use toposaic_core::Artifact;

use uuid::Uuid;

use crate::{AppState, Job, SavedSetup, SetupVersion, jobs::classify_job_error};

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
        -- What a setup held before each overwrite, newest first, trimmed to
        -- SAVED_SETUP_VERSION_LIMIT. Deleting a setup takes its history with
        -- it; foreign keys are ON above, so the cascade is enforced.
        CREATE TABLE IF NOT EXISTS saved_setup_versions (
            id TEXT PRIMARY KEY,
            setup_id TEXT NOT NULL REFERENCES saved_setups(id) ON DELETE CASCADE,
            saved_at TEXT NOT NULL,
            spec_json TEXT NOT NULL
        );
        CREATE INDEX IF NOT EXISTS saved_setup_versions_setup_idx
            ON saved_setup_versions(setup_id, saved_at DESC);
        CREATE TABLE IF NOT EXISTS place_search_cache (
            query TEXT PRIMARY KEY,
            response_json TEXT NOT NULL,
            fetched_at TEXT NOT NULL
        );
        UPDATE jobs
        SET status = 'failed',
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

/// Inserts the setup, or — when a setup with that name already exists —
/// replaces its spec and bumps `updated_at` in one atomic statement, keeping
/// the existing row's id and `created_at`. The single `INSERT ... ON
/// CONFLICT` cannot lose a check-then-write race the way a separate
/// find-by-name followed by an insert can. Returns the stored row straight
/// from `RETURNING`, plus whether this call created it: the row carries the
/// caller's freshly minted id only when the insert arm won, so the flag is
/// as race-free as the upsert itself.
pub fn upsert_saved_setup(state: &AppState, setup: &SavedSetup) -> Result<(SavedSetup, bool)> {
    let mut guard = connection(state)?;
    // One unit of work: a version filed without the write it stands for
    // would be a lie about what the setup used to hold.
    let transaction = guard.transaction()?;
    // Keep what the name held before this write, so an overwrite can be
    // undone. Only when the spec actually moves: saving a setup twice
    // without touching the model should not push its real history out.
    if let Some(previous) = read_saved_setup_by_name(&transaction, &setup.name)?
        && serde_json::to_string(&previous.spec)? != serde_json::to_string(&setup.spec)?
    {
        archive_setup_version(&transaction, &previous, setup.updated_at)?;
    }
    let mut statement = transaction.prepare(
        "INSERT INTO saved_setups (id, name, created_at, updated_at, spec_json)
         VALUES (?1, ?2, ?3, ?4, ?5)
         ON CONFLICT(name) DO UPDATE SET
             spec_json = excluded.spec_json,
             updated_at = excluded.updated_at
         RETURNING id, name, created_at, updated_at, spec_json",
    )?;
    let stored = statement.query_row(
        params![
            setup.id,
            setup.name,
            setup.created_at.to_rfc3339(),
            setup.updated_at.to_rfc3339(),
            serde_json::to_string(&setup.spec)?,
        ],
        row_to_saved_setup,
    )?;
    let created = stored.id == setup.id;
    drop(statement);
    transaction.commit()?;
    Ok((stored, created))
}

/// Outcome of a rename attempt, so the handler can map a lost name race to
/// the same conflict response as its own pre-check.
#[derive(Debug)]
pub enum RenameOutcome {
    Renamed(Box<SavedSetup>),
    NameTaken,
    NotFound,
}

/// Renames the setup. A concurrent writer can claim the target name between
/// any pre-check and this update; the UNIQUE violation that raises is
/// reported as `NameTaken` instead of surfacing as a plain error.
pub fn rename_saved_setup(
    state: &AppState,
    id: &str,
    name: &str,
    updated_at: DateTime<Utc>,
) -> Result<RenameOutcome> {
    let connection = connection(state)?;
    let updated = match connection.execute(
        "UPDATE saved_setups SET name = ?2, updated_at = ?3 WHERE id = ?1",
        params![id, name, updated_at.to_rfc3339()],
    ) {
        Ok(updated) => updated,
        Err(error) if is_unique_violation(&error) => return Ok(RenameOutcome::NameTaken),
        Err(error) => return Err(error.into()),
    };
    if updated != 1 {
        return Ok(RenameOutcome::NotFound);
    }
    let mut statement = connection.prepare(
        "SELECT id, name, created_at, updated_at, spec_json
         FROM saved_setups WHERE id = ?1",
    )?;
    let mut rows = statement.query([id])?;
    Ok(rows
        .next()?
        .map(row_to_saved_setup)
        .transpose()?
        .map(|setup| RenameOutcome::Renamed(Box::new(setup)))
        .unwrap_or(RenameOutcome::NotFound))
}

/// How many superseded versions of a setup are kept. A few, so a wrong
/// overwrite can be walked back, without the store growing without bound.
pub const SAVED_SETUP_VERSION_LIMIT: i64 = 5;

fn read_saved_setup_by_name(connection: &Connection, name: &str) -> Result<Option<SavedSetup>> {
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

/// Files one superseded spec and drops whatever falls past the limit.
fn archive_setup_version(
    connection: &Connection,
    previous: &SavedSetup,
    saved_at: DateTime<Utc>,
) -> Result<()> {
    connection.execute(
        "INSERT INTO saved_setup_versions (id, setup_id, saved_at, spec_json)
         VALUES (?1, ?2, ?3, ?4)",
        params![
            Uuid::new_v4().to_string(),
            previous.id,
            saved_at.to_rfc3339(),
            serde_json::to_string(&previous.spec)?,
        ],
    )?;
    connection.execute(
        "DELETE FROM saved_setup_versions
         WHERE setup_id = ?1
           AND id NOT IN (
               SELECT id FROM saved_setup_versions
               WHERE setup_id = ?1
               ORDER BY saved_at DESC, rowid DESC
               LIMIT ?2
           )",
        params![previous.id, SAVED_SETUP_VERSION_LIMIT],
    )?;
    Ok(())
}

/// One superseded spec, newest first.
pub fn list_setup_versions(state: &AppState, setup_id: &str) -> Result<Vec<SetupVersion>> {
    let connection = connection(state)?;
    let mut statement = connection.prepare(
        "SELECT id, saved_at, spec_json
         FROM saved_setup_versions
         WHERE setup_id = ?1
         ORDER BY saved_at DESC, rowid DESC",
    )?;
    let rows = statement.query_map([setup_id], |row| {
        let saved_at: String = row.get(1)?;
        let spec_json: String = row.get(2)?;
        Ok(SetupVersion {
            id: row.get(0)?,
            saved_at: saved_at.parse().map_err(sql_conversion_error)?,
            spec: serde_json::from_str(&spec_json).map_err(sql_conversion_error)?,
        })
    })?;
    rows.collect::<rusqlite::Result<Vec<_>>>()
        .map_err(Into::into)
}

/// Puts a superseded spec back, filing the current one as a version of its
/// own so the restore can itself be walked back. Returns the updated setup,
/// or `None` when the version is not that setup's.
pub fn restore_setup_version(
    state: &AppState,
    setup_id: &str,
    version_id: &str,
    now: DateTime<Utc>,
) -> Result<Option<SavedSetup>> {
    let mut guard = connection(state)?;
    // One unit of work: the four writes below leave the store telling a
    // different story about this setup if any of them lands without the
    // rest.
    let transaction = guard.transaction()?;
    let mut statement = transaction
        .prepare("SELECT spec_json FROM saved_setup_versions WHERE id = ?1 AND setup_id = ?2")?;
    let mut rows = statement.query(params![version_id, setup_id])?;
    let Some(row) = rows.next()? else {
        return Ok(None);
    };
    let spec: toposaic_core::GenerationSpec = serde_json::from_str(&row.get::<_, String>(0)?)?;
    drop(rows);
    drop(statement);

    let mut statement = transaction.prepare(
        "SELECT id, name, created_at, updated_at, spec_json
         FROM saved_setups WHERE id = ?1",
    )?;
    let mut rows = statement.query([setup_id])?;
    let Some(current) = rows.next()?.map(row_to_saved_setup).transpose()? else {
        return Ok(None);
    };
    drop(rows);
    drop(statement);
    archive_setup_version(&transaction, &current, now)?;
    transaction.execute(
        "DELETE FROM saved_setup_versions WHERE id = ?1",
        params![version_id],
    )?;

    let mut statement = transaction.prepare(
        "UPDATE saved_setups SET spec_json = ?2, updated_at = ?3 WHERE id = ?1
         RETURNING id, name, created_at, updated_at, spec_json",
    )?;
    let restored = statement.query_row(
        params![setup_id, serde_json::to_string(&spec)?, now.to_rfc3339()],
        row_to_saved_setup,
    )?;
    drop(statement);
    transaction.commit()?;
    Ok(Some(restored))
}

fn is_unique_violation(error: &rusqlite::Error) -> bool {
    matches!(
        error,
        rusqlite::Error::SqliteFailure(failure, _)
            if failure.extended_code == rusqlite::ffi::SQLITE_CONSTRAINT_UNIQUE
    )
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
    let error: Option<String> = row.get(7)?;
    let failure = error.as_deref().map(classify_job_error);
    Ok(Job {
        id: row.get(0)?,
        status: row.get(1)?,
        progress: row.get(2)?,
        created_at: created_at.parse().map_err(sql_conversion_error)?,
        updated_at: updated_at.parse().map_err(sql_conversion_error)?,
        spec: serde_json::from_str(&spec_json).map_err(sql_conversion_error)?,
        artifacts: serde_json::from_str(&artifacts_json).map_err(sql_conversion_error)?,
        error,
        failure,
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

#[cfg(test)]
mod tests {
    use toposaic_core::GenerationSpec;

    use super::*;
    use crate::test_state;

    #[test]
    fn migrate_fails_interrupted_jobs_but_keeps_their_progress() {
        let state = test_state();
        {
            let connection = state.db.lock().unwrap();
            let now = Utc::now().to_rfc3339();
            for (id, status, progress) in [
                ("job-running", "running", 40_i64),
                ("job-queued", "queued", 0),
                ("job-done", "complete", 100),
            ] {
                connection
                    .execute(
                        "INSERT INTO jobs
                         (id, status, progress, created_at, updated_at, spec_json,
                          artifacts_json, error)
                         VALUES (?1, ?2, ?3, ?4, ?4, '{}', '[]', NULL)",
                        params![id, status, progress, now],
                    )
                    .unwrap();
            }
            migrate(&connection).unwrap();

            let mut statement = connection
                .prepare("SELECT id, status, progress FROM jobs ORDER BY id")
                .unwrap();
            let rows = statement
                .query_map([], |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, i64>(2)?,
                    ))
                })
                .unwrap()
                .collect::<rusqlite::Result<Vec<_>>>()
                .unwrap();
            assert_eq!(
                rows,
                [
                    ("job-done".into(), "complete".into(), 100),
                    ("job-queued".into(), "failed".into(), 0),
                    ("job-running".into(), "failed".into(), 40),
                ]
            );
        }
    }

    fn setup(name: &str) -> SavedSetup {
        let now = Utc::now();
        SavedSetup {
            id: uuid::Uuid::new_v4().to_string(),
            name: name.into(),
            created_at: now,
            updated_at: now,
            spec: GenerationSpec::default(),
        }
    }

    #[test]
    fn upsert_keeps_the_winning_row_when_a_name_race_is_lost() {
        let state = test_state();
        let (winner, created) = upsert_saved_setup(&state, &setup("Alps")).unwrap();
        assert!(created);

        // A racing save built its own row for the same name before the
        // winner landed; the atomic upsert must fold it into the winner's
        // row instead of failing on the UNIQUE index.
        let mut loser = setup("Alps");
        loser.spec.ground_span_km = 30.0;
        let (stored, created) = upsert_saved_setup(&state, &loser).unwrap();
        assert!(!created, "an overwrite is not a create");

        assert_eq!(stored.id, winner.id);
        assert_eq!(stored.created_at, winner.created_at);
        assert_eq!(stored.updated_at, loser.updated_at);
        assert_eq!(stored.spec.ground_span_km, 30.0);
        assert_eq!(list_saved_setups(&state).unwrap().len(), 1);
    }

    #[test]
    fn rename_reports_a_lost_name_race_instead_of_erroring() {
        let state = test_state();
        let (first, _) = upsert_saved_setup(&state, &setup("Alps")).unwrap();
        let (second, _) = upsert_saved_setup(&state, &setup("Rockies")).unwrap();

        // The UNIQUE violation a lost race raises maps to NameTaken.
        assert!(matches!(
            rename_saved_setup(&state, &second.id, "Alps", Utc::now()).unwrap(),
            RenameOutcome::NameTaken
        ));
        assert!(matches!(
            rename_saved_setup(&state, "no-such-id", "Baker", Utc::now()).unwrap(),
            RenameOutcome::NotFound
        ));
        match rename_saved_setup(&state, &second.id, "Cascades", Utc::now()).unwrap() {
            RenameOutcome::Renamed(renamed) => {
                assert_eq!(renamed.id, second.id);
                assert_eq!(renamed.name, "Cascades");
            }
            other => panic!("expected a rename, got {other:?}"),
        }
        assert_eq!(
            find_saved_setup_by_name(&state, "Alps")
                .unwrap()
                .unwrap()
                .id,
            first.id
        );
    }
}
