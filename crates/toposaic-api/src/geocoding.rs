use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};
use axum::{
    Json,
    extract::{Query, State},
    http::StatusCode,
};
use chrono::Utc;
use rusqlite::params;
use serde::{Deserialize, Serialize};
use tokio::time::sleep;

use crate::{ApiError, AppState, api_error, database::sql_conversion_error, internal_error};

#[derive(Debug, Deserialize)]
pub struct PlaceSearch {
    q: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlaceResult {
    display_name: String,
    latitude: f64,
    longitude: f64,
    category: String,
    kind: String,
}

#[derive(Debug, Deserialize)]
pub struct NominatimPlace {
    display_name: String,
    lat: String,
    lon: String,
    category: String,
    #[serde(rename = "type")]
    kind: String,
}

pub async fn search_places(
    State(state): State<AppState>,
    Query(search): Query<PlaceSearch>,
) -> Result<Json<Vec<PlaceResult>>, (StatusCode, Json<ApiError>)> {
    let query = search.q.trim();
    if !valid_place_query_length(query) {
        return Err(api_error(
            StatusCode::BAD_REQUEST,
            "place search must be between 2 and 120 characters",
        ));
    }
    let normalized_query = query.to_lowercase();
    if let Some(cached) = find_cached_places(&state, &normalized_query).map_err(internal_error)? {
        return Ok(Json(cached));
    }

    fetch_places(&state, query, &normalized_query)
        .await
        .map(Json)
        .map_err(internal_error)
}

pub fn valid_place_query_length(query: &str) -> bool {
    (2..=120).contains(&query.chars().count())
}

fn find_cached_places(state: &AppState, query: &str) -> Result<Option<Vec<PlaceResult>>> {
    let connection = state
        .db
        .lock()
        .map_err(|_| anyhow::anyhow!("database lock failed"))?;
    let mut statement =
        connection.prepare("SELECT response_json FROM place_search_cache WHERE query = ?1")?;
    let mut rows = statement.query([query])?;
    rows.next()?
        .map(|row| {
            let value: String = row.get(0)?;
            serde_json::from_str(&value).map_err(sql_conversion_error)
        })
        .transpose()
        .map_err(Into::into)
}

async fn fetch_places(
    state: &AppState,
    query: &str,
    normalized_query: &str,
) -> Result<Vec<PlaceResult>> {
    {
        let mut previous = state.last_geocode_request.lock().await;
        let wait = Duration::from_secs(1).saturating_sub(previous.elapsed());
        if !wait.is_zero() {
            sleep(wait).await;
        }
        *previous = Instant::now();
    }

    let url = format!("{}/search", state.geocoder_base_url.trim_end_matches('/'));
    let response = state
        .geocoder
        .get(url)
        .query(&[
            ("q", query),
            ("format", "jsonv2"),
            ("limit", "5"),
            ("addressdetails", "0"),
        ])
        .send()
        .await
        .context("search OpenStreetMap places")?
        .error_for_status()
        .context("OpenStreetMap place search failed")?;
    let results = response
        .json::<Vec<NominatimPlace>>()
        .await?
        .into_iter()
        .map(PlaceResult::try_from)
        .collect::<Result<Vec<_>>>()?;

    let connection = state
        .db
        .lock()
        .map_err(|_| anyhow::anyhow!("database lock failed"))?;
    connection.execute(
        "INSERT INTO place_search_cache (query, response_json, fetched_at)
         VALUES (?1, ?2, ?3)
         ON CONFLICT(query) DO UPDATE SET
             response_json = excluded.response_json,
             fetched_at = excluded.fetched_at",
        params![
            normalized_query,
            serde_json::to_string(&results)?,
            Utc::now().to_rfc3339(),
        ],
    )?;
    Ok(results)
}

impl TryFrom<NominatimPlace> for PlaceResult {
    type Error = anyhow::Error;

    fn try_from(place: NominatimPlace) -> Result<Self> {
        let latitude: f64 = place.lat.parse().context("invalid place latitude")?;
        let longitude: f64 = place.lon.parse().context("invalid place longitude")?;
        if !latitude.is_finite() || !(-90.0..=90.0).contains(&latitude) {
            bail!("place latitude is outside the valid range");
        }
        if !longitude.is_finite() || !(-180.0..=180.0).contains(&longitude) {
            bail!("place longitude is outside the valid range");
        }
        Ok(Self {
            display_name: place.display_name,
            latitude,
            longitude,
            category: place.category,
            kind: place.kind,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn converts_nominatim_coordinates() {
        let place = PlaceResult::try_from(NominatimPlace {
            display_name: "Mount Rainier, Washington, United States".into(),
            lat: "46.8523".into(),
            lon: "-121.7603".into(),
            category: "natural".into(),
            kind: "peak".into(),
        })
        .unwrap();

        assert_eq!(
            place.display_name,
            "Mount Rainier, Washington, United States"
        );
        assert!((place.latitude - 46.8523).abs() < f64::EPSILON);
        assert!((place.longitude + 121.7603).abs() < f64::EPSILON);
        assert_eq!(place.kind, "peak");
    }

    #[test]
    fn rejects_invalid_nominatim_coordinates() {
        let invalid_number = PlaceResult::try_from(NominatimPlace {
            display_name: "Broken".into(),
            lat: "north".into(),
            lon: "west".into(),
            category: "place".into(),
            kind: "unknown".into(),
        });
        assert!(invalid_number.is_err());

        let out_of_range = PlaceResult::try_from(NominatimPlace {
            display_name: "Broken".into(),
            lat: "91".into(),
            lon: "181".into(),
            category: "place".into(),
            kind: "unknown".into(),
        });
        assert!(out_of_range.is_err());
    }

    #[test]
    fn place_query_limit_counts_characters_not_utf8_bytes() {
        assert!(valid_place_query_length("東京"));
        assert!(valid_place_query_length(&"é".repeat(120)));
        assert!(!valid_place_query_length("x"));
        assert!(!valid_place_query_length(&"é".repeat(121)));
    }
}
