//! Saved setups: named generation specs users store and recall later.

use axum::{
    Json,
    extract::{Path as AxumPath, State},
    http::StatusCode,
};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use toposaic_core::GenerationSpec;
use uuid::Uuid;

use crate::{
    ApiError, AppState, api_error, canonical_uuid,
    database::{
        RenameOutcome, delete_saved_setup, find_saved_setup_by_name, list_saved_setups,
        rename_saved_setup, upsert_saved_setup,
    },
    internal_error,
};

const MAX_SETUP_NAME_CHARS: usize = 120;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct SavedSetup {
    pub(crate) id: String,
    pub(crate) name: String,
    pub(crate) created_at: DateTime<Utc>,
    pub(crate) updated_at: DateTime<Utc>,
    pub(crate) spec: GenerationSpec,
}

#[derive(Debug, Deserialize)]
pub(crate) struct SaveSetupRequest {
    pub(crate) name: String,
    pub(crate) spec: GenerationSpec,
}

#[derive(Debug, Deserialize)]
pub(crate) struct RenameSetupRequest {
    pub(crate) name: String,
}

pub(crate) async fn list_setups(
    State(state): State<AppState>,
) -> Result<Json<Vec<SavedSetup>>, (StatusCode, Json<ApiError>)> {
    list_saved_setups(&state).map(Json).map_err(internal_error)
}

pub(crate) async fn save_setup(
    State(state): State<AppState>,
    Json(request): Json<SaveSetupRequest>,
) -> Result<Json<SavedSetup>, (StatusCode, Json<ApiError>)> {
    let name = validated_setup_name(&request.name)?;
    request
        .spec
        .validate()
        .map_err(|error| api_error(StatusCode::BAD_REQUEST, error))?;

    // One atomic INSERT ... ON CONFLICT(name) both creates and replaces, so
    // two concurrent saves of the same new name cannot race a check into a
    // UNIQUE violation; the loser simply updates the winner's row.
    let now = Utc::now();
    let setup = SavedSetup {
        id: Uuid::new_v4().to_string(),
        name: name.to_string(),
        created_at: now,
        updated_at: now,
        spec: request.spec,
    };
    upsert_saved_setup(&state, &setup)
        .map(Json)
        .map_err(internal_error)
}

pub(crate) async fn rename_setup(
    State(state): State<AppState>,
    AxumPath(id): AxumPath<String>,
    Json(request): Json<RenameSetupRequest>,
) -> Result<Json<SavedSetup>, (StatusCode, Json<ApiError>)> {
    let id =
        canonical_uuid(&id).ok_or_else(|| api_error(StatusCode::NOT_FOUND, "setup not found"))?;
    let name = validated_setup_name(&request.name)?;
    // The pre-check keeps renaming a setup to its own name a no-op (no
    // updated_at bump); the atomic rename below still maps a lost name race
    // to the same conflict instead of a 500.
    if let Some(existing) = find_saved_setup_by_name(&state, name).map_err(internal_error)?
        && existing.id == id
    {
        return Ok(Json(existing));
    }
    match rename_saved_setup(&state, &id, name, Utc::now()).map_err(internal_error)? {
        RenameOutcome::Renamed(setup) => Ok(Json(*setup)),
        RenameOutcome::NameTaken => Err(api_error(
            StatusCode::CONFLICT,
            format!("a setup named \"{name}\" already exists"),
        )),
        RenameOutcome::NotFound => Err(api_error(StatusCode::NOT_FOUND, "setup not found")),
    }
}

pub(crate) async fn delete_setup(
    State(state): State<AppState>,
    AxumPath(id): AxumPath<String>,
) -> Result<StatusCode, (StatusCode, Json<ApiError>)> {
    let id =
        canonical_uuid(&id).ok_or_else(|| api_error(StatusCode::NOT_FOUND, "setup not found"))?;
    if delete_saved_setup(&state, &id).map_err(internal_error)? {
        Ok(StatusCode::NO_CONTENT)
    } else {
        Err(api_error(StatusCode::NOT_FOUND, "setup not found"))
    }
}

fn validated_setup_name(name: &str) -> Result<&str, (StatusCode, Json<ApiError>)> {
    let name = name.trim();
    if name.is_empty() {
        return Err(api_error(
            StatusCode::BAD_REQUEST,
            "setup name must not be blank",
        ));
    }
    if name.chars().count() > MAX_SETUP_NAME_CHARS {
        return Err(api_error(
            StatusCode::BAD_REQUEST,
            format!("setup name must be at most {MAX_SETUP_NAME_CHARS} characters"),
        ));
    }
    Ok(name)
}

#[cfg(test)]
mod tests {
    use rusqlite::params;

    use super::*;
    use crate::test_state;

    async fn save(
        state: &AppState,
        name: &str,
        spec: GenerationSpec,
    ) -> Result<SavedSetup, StatusCode> {
        let request = SaveSetupRequest {
            name: name.into(),
            spec,
        };
        save_setup(State(state.clone()), Json(request))
            .await
            .map(|json| json.0)
            .map_err(|(status, _)| status)
    }

    async fn list(state: &AppState) -> Vec<SavedSetup> {
        list_setups(State(state.clone())).await.unwrap().0
    }

    async fn delete(state: &AppState, id: &str) -> Result<StatusCode, StatusCode> {
        delete_setup(State(state.clone()), AxumPath(id.into()))
            .await
            .map_err(|(status, _)| status)
    }

    async fn rename(state: &AppState, id: &str, name: &str) -> Result<SavedSetup, StatusCode> {
        let request = RenameSetupRequest { name: name.into() };
        rename_setup(State(state.clone()), AxumPath(id.into()), Json(request))
            .await
            .map(|json| json.0)
            .map_err(|(status, _)| status)
    }

    #[tokio::test]
    async fn setups_round_trip_through_save_list_and_delete() {
        let state = test_state();
        let first = save(&state, "  Mount Rainier  ", GenerationSpec::default())
            .await
            .unwrap();
        assert_eq!(first.name, "Mount Rainier");
        std::thread::sleep(std::time::Duration::from_millis(5));
        let second = save(&state, "Mount Baker", GenerationSpec::default())
            .await
            .unwrap();

        let listed = list(&state).await;
        assert_eq!(listed.len(), 2);
        assert_eq!(listed[0].id, second.id, "newest update lists first");
        assert_eq!(listed[1].id, first.id);
        assert_eq!(listed[1].created_at, first.created_at);
        assert_eq!(listed[1].spec.place_name, first.spec.place_name);

        assert_eq!(delete(&state, &first.id).await, Ok(StatusCode::NO_CONTENT));
        assert_eq!(list(&state).await.len(), 1);
        assert_eq!(delete(&state, &first.id).await, Err(StatusCode::NOT_FOUND));
        assert_eq!(
            delete(&state, "../escape").await,
            Err(StatusCode::NOT_FOUND)
        );
    }

    #[tokio::test]
    async fn saving_an_existing_name_replaces_the_spec_in_place() {
        let state = test_state();
        let first = save(&state, "Alps", GenerationSpec::default())
            .await
            .unwrap();
        std::thread::sleep(std::time::Duration::from_millis(5));

        let spec = GenerationSpec {
            ground_span_km: 30.0,
            ..GenerationSpec::default()
        };
        let second = save(&state, "Alps", spec).await.unwrap();

        assert_eq!(second.id, first.id);
        assert_eq!(second.created_at, first.created_at);
        assert!(second.updated_at > first.updated_at);
        assert_eq!(second.spec.ground_span_km, 30.0);

        let listed = list(&state).await;
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].id, first.id);
        assert_eq!(listed[0].created_at, first.created_at);
        assert_eq!(listed[0].updated_at, second.updated_at);
        assert_eq!(listed[0].spec.ground_span_km, 30.0);
    }

    #[tokio::test]
    async fn blank_and_oversized_names_and_invalid_specs_are_rejected() {
        let state = test_state();
        assert_eq!(
            save(&state, "   ", GenerationSpec::default()).await.err(),
            Some(StatusCode::BAD_REQUEST)
        );
        assert_eq!(
            save(&state, &"x".repeat(121), GenerationSpec::default())
                .await
                .err(),
            Some(StatusCode::BAD_REQUEST)
        );
        let spec = GenerationSpec {
            ground_span_km: 0.0,
            ..GenerationSpec::default()
        };
        assert_eq!(
            save(&state, "Bad span", spec).await.err(),
            Some(StatusCode::BAD_REQUEST)
        );
        assert!(list(&state).await.is_empty());
    }

    #[tokio::test]
    async fn renaming_changes_only_the_name_and_updated_at() {
        let state = test_state();
        let spec = GenerationSpec {
            ground_span_km: 30.0,
            ..GenerationSpec::default()
        };
        let saved = save(&state, "Alps", spec).await.unwrap();
        std::thread::sleep(std::time::Duration::from_millis(5));

        let renamed = rename(&state, &saved.id, "  Dolomites  ").await.unwrap();
        assert_eq!(renamed.name, "Dolomites");
        assert_eq!(renamed.id, saved.id);
        assert_eq!(renamed.created_at, saved.created_at);
        assert_eq!(renamed.spec.ground_span_km, 30.0);
        assert!(renamed.updated_at > saved.updated_at);

        let listed = list(&state).await;
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].name, "Dolomites");
        assert_eq!(listed[0].updated_at, renamed.updated_at);
    }

    #[tokio::test]
    async fn renaming_a_setup_to_its_own_name_succeeds() {
        let state = test_state();
        let saved = save(&state, "Alps", GenerationSpec::default())
            .await
            .unwrap();

        let renamed = rename(&state, &saved.id, " Alps ").await.unwrap();
        assert_eq!(renamed.id, saved.id);
        assert_eq!(renamed.name, "Alps");
        assert_eq!(renamed.created_at, saved.created_at);
        assert_eq!(renamed.updated_at, saved.updated_at, "rename is a no-op");
    }

    #[tokio::test]
    async fn renaming_over_another_setup_is_a_conflict() {
        let state = test_state();
        let first = save(&state, "Alps", GenerationSpec::default())
            .await
            .unwrap();
        let second = save(&state, "Rockies", GenerationSpec::default())
            .await
            .unwrap();

        assert_eq!(
            rename(&state, &second.id, " Alps ").await.err(),
            Some(StatusCode::CONFLICT)
        );

        let listed = list(&state).await;
        assert_eq!(listed.len(), 2);
        assert_eq!(listed[0].id, second.id);
        assert_eq!(listed[0].name, "Rockies");
        assert_eq!(listed[0].updated_at, second.updated_at);
        assert_eq!(listed[1].id, first.id);
        assert_eq!(listed[1].name, "Alps");
    }

    #[tokio::test]
    async fn rename_rejects_bad_names_and_unknown_ids() {
        let state = test_state();
        let saved = save(&state, "Alps", GenerationSpec::default())
            .await
            .unwrap();

        assert_eq!(
            rename(&state, &saved.id, "   ").await.err(),
            Some(StatusCode::BAD_REQUEST)
        );
        assert_eq!(
            rename(&state, &saved.id, &"x".repeat(121)).await.err(),
            Some(StatusCode::BAD_REQUEST)
        );
        assert_eq!(
            rename(&state, "395481ef-0e39-4d94-9d94-2c39fea86001", "Baker")
                .await
                .err(),
            Some(StatusCode::NOT_FOUND)
        );
        assert_eq!(
            rename(&state, "../escape", "Baker").await.err(),
            Some(StatusCode::NOT_FOUND)
        );

        let listed = list(&state).await;
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].name, "Alps");
    }

    #[tokio::test]
    async fn setups_saved_before_a_field_existed_load_with_defaults() {
        let state = test_state();
        let mut value = serde_json::to_value(GenerationSpec::default()).unwrap();
        let fields = value.as_object_mut().unwrap();
        fields.remove("puzzle_tabs");
        fields.remove("buildings");
        let now = Utc::now().to_rfc3339();
        state
            .db
            .lock()
            .unwrap()
            .execute(
                "INSERT INTO saved_setups (id, name, created_at, updated_at, spec_json)
                 VALUES (?1, ?2, ?3, ?4, ?5)",
                params![
                    "395481ef-0e39-4d94-9d94-2c39fea86001",
                    "legacy",
                    now,
                    now,
                    value.to_string(),
                ],
            )
            .unwrap();

        let listed = list(&state).await;
        assert_eq!(listed.len(), 1);
        let defaults = GenerationSpec::default();
        assert_eq!(listed[0].spec.puzzle_tabs, defaults.puzzle_tabs);
        assert_eq!(listed[0].spec.buildings.enabled, defaults.buildings.enabled);
    }
}
