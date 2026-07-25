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
    ApiError, AppState, api_error,
    database::{
        delete_saved_setup, find_saved_setup_by_name, insert_saved_setup, list_saved_setups,
        update_saved_setup,
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

pub(crate) async fn list_setups(
    State(state): State<AppState>,
) -> Result<Json<Vec<SavedSetup>>, (StatusCode, Json<ApiError>)> {
    list_saved_setups(&state).map(Json).map_err(internal_error)
}

pub(crate) async fn save_setup(
    State(state): State<AppState>,
    Json(request): Json<SaveSetupRequest>,
) -> Result<Json<SavedSetup>, (StatusCode, Json<ApiError>)> {
    let name = request.name.trim();
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
    request
        .spec
        .validate()
        .map_err(|error| api_error(StatusCode::BAD_REQUEST, error))?;

    let now = Utc::now();
    let setup = match find_saved_setup_by_name(&state, name).map_err(internal_error)? {
        Some(existing) => {
            let setup = SavedSetup {
                updated_at: now,
                spec: request.spec,
                ..existing
            };
            update_saved_setup(&state, &setup).map_err(internal_error)?;
            setup
        }
        None => {
            let setup = SavedSetup {
                id: Uuid::new_v4().to_string(),
                name: name.to_string(),
                created_at: now,
                updated_at: now,
                spec: request.spec,
            };
            insert_saved_setup(&state, &setup).map_err(internal_error)?;
            setup
        }
    };
    Ok(Json(setup))
}

pub(crate) async fn delete_setup(
    State(state): State<AppState>,
    AxumPath(id): AxumPath<String>,
) -> Result<StatusCode, (StatusCode, Json<ApiError>)> {
    let id = canonical_setup_id(&id)
        .ok_or_else(|| api_error(StatusCode::NOT_FOUND, "setup not found"))?;
    if delete_saved_setup(&state, &id).map_err(internal_error)? {
        Ok(StatusCode::NO_CONTENT)
    } else {
        Err(api_error(StatusCode::NOT_FOUND, "setup not found"))
    }
}

fn canonical_setup_id(id: &str) -> Option<String> {
    Uuid::parse_str(id)
        .ok()
        .map(|value| value.hyphenated().to_string())
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
