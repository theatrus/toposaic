//! Regional tidal datums from NOAA CO-OPS.
//!
//! The low and high tide presets need real numbers: how far MLLW and MHHW
//! sit from local mean sea level. NOAA's CO-OPS metadata API publishes
//! exactly that for United States coasts — a directory of water-level
//! stations and, per station, a datum sheet with MLLW, MSL, and MHHW on a
//! published tidal epoch, all in metres against the station datum. The
//! differences MLLW−MSL and MHHW−MSL are pure tidal quantities, so they
//! need no vertical-datum transformation to ride on the model's own
//! reference.
//!
//! Only offsets leave this module. Where the sea's mean surface actually
//! sits stays the marine resolver's problem, and its notes carry the
//! approximation honestly.

use std::{path::Path, time::Duration};

use anyhow::{Context, Result, bail};
use serde::Deserialize;
use tracing::warn;

use crate::{cache, http};
use toposaic_core::TidalOffsets;

const STATIONS_URL: &str = "https://api.tidesandcurrents.noaa.gov/mdapi/prod/webapi/stations.json?type=waterlevels&units=metric";
const STATION_DATUMS_URL: &str = "https://api.tidesandcurrents.noaa.gov/mdapi/prod/webapi/stations";
/// Beyond this distance the nearest station's tides say little about the
/// map's own shore; the presets fall back to mean sea level instead of
/// stretching a station past its regime.
const MAXIMUM_STATION_KM: f64 = 100.0;
/// Beyond this distance the offsets still apply but carry a caveat: the
/// issue's rule is never to extend a station silently.
const DISTANT_STATION_KM: f64 = 50.0;

#[derive(Debug, Deserialize)]
struct StationsResponse {
    stations: Vec<Station>,
}

#[derive(Debug, Deserialize)]
struct Station {
    id: String,
    name: String,
    lat: f64,
    lng: f64,
}

#[derive(Debug, Deserialize)]
struct DatumsResponse {
    #[serde(default)]
    epoch: String,
    #[serde(default)]
    datums: Vec<DatumEntry>,
}

#[derive(Debug, Deserialize)]
struct DatumEntry {
    name: String,
    value: f64,
}

/// Fetches the tidal datum offsets for the station nearest the model
/// centre, both requests served from cache after the first fetch. `None`
/// — never an error — when no station is close enough or its sheet lacks
/// the needed datums: the marine resolver has an honest fallback and a
/// missing tide service must not fail a generation.
pub(crate) fn fetch_tidal_offsets(
    latitude: f64,
    longitude: f64,
    cache_dir: &Path,
) -> Option<TidalOffsets> {
    let stations = match fetch_stations(cache_dir) {
        Ok(stations) => stations,
        Err(error) => {
            warn!(%error, "NOAA CO-OPS station directory unavailable");
            return None;
        }
    };
    let (station, distance_km) = nearest_station(&stations, latitude, longitude)?;
    if distance_km > MAXIMUM_STATION_KM {
        return None;
    }
    let datums = match fetch_station_datums(&station.id, cache_dir) {
        Ok(datums) => datums,
        Err(error) => {
            warn!(%error, station = %station.id, "NOAA CO-OPS datum sheet unavailable");
            return None;
        }
    };
    offsets_from_datums(&datums, station, distance_km)
}

fn fetch_stations(cache_dir: &Path) -> Result<Vec<Station>> {
    let bytes = fetch_cached(cache_dir, "coops-stations-v1.json", STATIONS_URL)?;
    let response: StationsResponse =
        serde_json::from_slice(&bytes).context("parse the CO-OPS station directory")?;
    Ok(response.stations)
}

fn fetch_station_datums(station_id: &str, cache_dir: &Path) -> Result<DatumsResponse> {
    // Station ids come from NOAA's own directory, but they name a cache
    // file: keep them to the digits they always are.
    if station_id.is_empty() || !station_id.chars().all(|c| c.is_ascii_alphanumeric()) {
        bail!("station id {station_id:?} is not a plain identifier");
    }
    let url = format!("{STATION_DATUMS_URL}/{station_id}/datums.json?units=metric");
    let bytes = fetch_cached(
        cache_dir,
        &format!("coops-datums-v1-{station_id}.json"),
        &url,
    )?;
    serde_json::from_slice(&bytes).context("parse the CO-OPS datum sheet")
}

fn fetch_cached(cache_dir: &Path, file_name: &str, url: &str) -> Result<Vec<u8>> {
    let path = cache_dir.join(file_name);
    if let Ok(bytes) = std::fs::read(&path) {
        return Ok(bytes);
    }
    let response = http::blocking_client(Duration::from_secs(30))
        .context("build NOAA CO-OPS client")?
        .get(url)
        .send()
        .with_context(|| format!("fetch {url}"))?
        .error_for_status()
        .with_context(|| format!("NOAA CO-OPS rejected {url}"))?;
    let bytes = response.bytes().context("read NOAA CO-OPS response")?;
    if let Err(error) = cache::store(&path, &bytes) {
        warn!(%error, "could not cache the NOAA CO-OPS response");
    }
    Ok(bytes.to_vec())
}

/// The station nearest a coordinate, with its great-circle-ish distance:
/// equirectangular is exact enough at station-hunting ranges.
fn nearest_station(stations: &[Station], latitude: f64, longitude: f64) -> Option<(&Station, f64)> {
    stations
        .iter()
        .map(|station| {
            let dy = (station.lat - latitude) * 111.32;
            let dx = (station.lng - longitude) * 111.32 * latitude.to_radians().cos();
            (station, dx.hypot(dy))
        })
        .min_by(|a, b| a.1.total_cmp(&b.1))
}

/// Turns a station's datum sheet into MSL-relative offsets. `None` when
/// the sheet lacks MLLW, MSL, or MHHW — subordinate stations sometimes do
/// — or the numbers are not credible tides.
fn offsets_from_datums(
    datums: &DatumsResponse,
    station: &Station,
    distance_km: f64,
) -> Option<TidalOffsets> {
    let value = |name: &str| {
        datums
            .datums
            .iter()
            .find(|entry| entry.name == name)
            .map(|entry| entry.value)
    };
    let msl = value("MSL")?;
    let low = value("MLLW")? - msl;
    let high = value("MHHW")? - msl;
    // A sheet claiming low water above mean, or a tidal range past the
    // largest on Earth, is a data problem, not a level to print.
    if !(-10.0..0.0).contains(&low) || !(0.0..10.0).contains(&high) {
        return None;
    }
    let epoch = if datums.epoch.is_empty() {
        String::new()
    } else {
        format!(", epoch {}", datums.epoch)
    };
    Some(TidalOffsets {
        low_minus_msl_m: low as f32,
        high_minus_msl_m: high as f32,
        source: format!(
            "NOAA CO-OPS station {} {} ({:.0} km away){epoch}",
            station.id, station.name, distance_km
        ),
        caveat: (distance_km > DISTANT_STATION_KM).then(|| {
            format!(
                "the nearest tide station is {distance_km:.0} km away; its tidal range may not \
                 match this shore"
            )
        }),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A trimmed real CO-OPS datum sheet: San Francisco, station 9414290,
    /// metres against the station datum, epoch 1983-2001.
    const SAN_FRANCISCO_DATUMS: &str = r#"{
        "epoch": "1983-2001",
        "datums": [
            {"name": "STND", "value": 0.0},
            {"name": "MHHW", "value": 3.602},
            {"name": "MHW", "value": 3.416},
            {"name": "MTL", "value": 2.792},
            {"name": "MSL", "value": 2.773},
            {"name": "MLW", "value": 2.168},
            {"name": "MLLW", "value": 1.822},
            {"name": "NAVD88", "value": 1.804}
        ]
    }"#;

    fn station() -> Station {
        Station {
            id: "9414290".into(),
            name: "San Francisco".into(),
            lat: 37.806,
            lng: -122.466,
        }
    }

    #[test]
    fn a_real_datum_sheet_yields_msl_relative_offsets() {
        let datums: DatumsResponse = serde_json::from_str(SAN_FRANCISCO_DATUMS).unwrap();
        let offsets = offsets_from_datums(&datums, &station(), 22.0).unwrap();
        assert!((offsets.low_minus_msl_m + 0.951).abs() < 1e-3);
        assert!((offsets.high_minus_msl_m - 0.829).abs() < 1e-3);
        assert!(offsets.source.contains("9414290"));
        assert!(offsets.source.contains("San Francisco"));
        assert!(offsets.source.contains("1983-2001"));
        assert!(offsets.caveat.is_none(), "22 km is a near station");
    }

    #[test]
    fn distant_stations_carry_their_caveat_and_absurd_sheets_are_refused() {
        let datums: DatumsResponse = serde_json::from_str(SAN_FRANCISCO_DATUMS).unwrap();
        let distant = offsets_from_datums(&datums, &station(), 87.0).unwrap();
        assert!(distant.caveat.as_deref().unwrap().contains("87 km"));

        // A sheet without MSL cannot anchor offsets.
        let no_msl: DatumsResponse = serde_json::from_str(
            r#"{"epoch": "", "datums": [{"name": "MLLW", "value": 0.0}, {"name": "MHHW", "value": 1.5}]}"#,
        )
        .unwrap();
        assert!(offsets_from_datums(&no_msl, &station(), 5.0).is_none());

        // Low water above mean is a broken sheet, not a level to print.
        let inverted: DatumsResponse = serde_json::from_str(
            r#"{"epoch": "", "datums": [{"name": "MSL", "value": 1.0}, {"name": "MLLW", "value": 1.5}, {"name": "MHHW", "value": 2.0}]}"#,
        )
        .unwrap();
        assert!(offsets_from_datums(&inverted, &station(), 5.0).is_none());
    }

    #[test]
    fn the_nearest_station_wins_and_longitude_shrinks_with_latitude() {
        let stations = vec![
            Station {
                id: "1".into(),
                name: "north".into(),
                lat: 38.0,
                lng: -122.4,
            },
            Station {
                id: "2".into(),
                name: "east".into(),
                lat: 37.6,
                lng: -122.0,
            },
        ];
        // From (37.6, -122.4): north is 0.4 deg of latitude (~44 km), east
        // is 0.4 deg of longitude (~35 km at this latitude) — east wins
        // only because longitude degrees shrink with latitude.
        let (nearest, distance) = nearest_station(&stations, 37.6, -122.4).unwrap();
        assert_eq!(nearest.id, "2");
        assert!((30.0..40.0).contains(&distance));
    }

    #[test]
    fn station_ids_that_are_not_plain_identifiers_never_touch_the_cache() {
        let dir = std::env::temp_dir().join(format!("toposaic-datum-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        assert!(fetch_station_datums("../evil", &dir).is_err());
        assert!(fetch_station_datums("", &dir).is_err());
        std::fs::remove_dir_all(&dir).ok();
    }
}
