//! OpenStreetMap Overpass fetches and reusable geographic tile caching.

use std::{
    collections::HashMap,
    env, fs,
    path::{Path, PathBuf},
    sync::{
        Mutex,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
    thread,
    time::Duration,
};

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use tracing::warn;

use crate::{
    cache,
    geo::{GeoBounds, normalize_longitude},
    http,
};

const DEFAULT_OVERPASS_URL: &str = "https://overpass-api.de/api/interpreter";
const FALLBACK_OVERPASS_URL: &str = "https://maps.mail.ru/osm/tools/overpass/api/interpreter";
const OVERPASS_ATTEMPTS: usize = 2;
const OVERPASS_RETRY_DELAY: Duration = Duration::from_millis(750);
const MAX_OVERPASS_TILES_PER_QUERY: usize = 16;
const WEB_MERCATOR_MAX_LATITUDE: f64 = 85.051_128_78;

static OVERPASS_REQUEST_LOCK: Mutex<()> = Mutex::new(());
static PREFERRED_OVERPASS_ENDPOINT: AtomicUsize = AtomicUsize::new(0);

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct OverpassResponse {
    #[serde(default)]
    pub(crate) elements: Vec<OverpassWay>,
    #[serde(default)]
    pub(crate) remark: Option<String>,
}

#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub(crate) struct OverpassWay {
    #[serde(default)]
    pub(crate) id: u64,
    #[serde(rename = "type", default)]
    pub(crate) element_type: String,
    #[serde(default)]
    pub(crate) tags: HashMap<String, String>,
    #[serde(default)]
    pub(crate) geometry: Vec<OverpassPoint>,
    #[serde(default)]
    pub(crate) members: Vec<OverpassMember>,
}

#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub(crate) struct OverpassMember {
    #[serde(rename = "ref", default)]
    pub(crate) reference: u64,
    #[serde(default)]
    pub(crate) role: String,
    #[serde(rename = "type", default)]
    pub(crate) member_type: String,
    #[serde(default)]
    pub(crate) geometry: Vec<OverpassPoint>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub(crate) struct OverpassPoint {
    pub(crate) lat: f64,
    pub(crate) lon: f64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) struct TileLayer<'a> {
    pub(crate) namespace: &'a str,
    pub(crate) zoom: u8,
}

#[derive(Clone, Copy)]
pub(crate) struct LegacyCache<'a> {
    prefixes: &'a [&'a str],
    accepts: fn(&OverpassWay) -> bool,
}

impl<'a> LegacyCache<'a> {
    pub(crate) const fn new(prefixes: &'a [&'a str]) -> Self {
        Self {
            prefixes,
            accepts: |_| true,
        }
    }

    pub(crate) const fn filtered(
        prefixes: &'a [&'a str],
        accepts: fn(&OverpassWay) -> bool,
    ) -> Self {
        Self { prefixes, accepts }
    }
}

#[derive(Debug, Deserialize, Serialize)]
struct LegacyCacheMetadata {
    version: u8,
    south: f64,
    north: f64,
    west: f64,
    east: f64,
    query_variant_hash: u64,
    response_file: String,
}

impl LegacyCacheMetadata {
    fn bounds(&self) -> GeoBounds {
        GeoBounds {
            south: self.south,
            north: self.north,
            west: self.west,
            east: self.east,
        }
    }
}

impl<'a> TileLayer<'a> {
    pub(crate) const fn new(namespace: &'a str, zoom: u8) -> Self {
        Self { namespace, zoom }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub(crate) struct Tile {
    pub(crate) zoom: u8,
    pub(crate) x: u32,
    pub(crate) y: u32,
}

/// Reads fixed geographic tiles, downloads only gaps, and returns data
/// clipped to the requested view. Empty tiles are cached too.
pub(crate) fn fetch_tiled_response(
    cache_dir: &Path,
    layer: TileLayer<'_>,
    legacy: LegacyCache<'_>,
    requested_bounds: GeoBounds,
    query_for_bounds: impl Fn(&[GeoBounds]) -> String,
    cancellation: Option<&AtomicBool>,
) -> Result<OverpassResponse> {
    ensure_active(cancellation)?;
    fs::create_dir_all(cache_dir)
        .with_context(|| format!("create OpenStreetMap cache {}", cache_dir.display()))?;
    let tiles = tiles_for_bounds(layer.zoom, requested_bounds);
    let mut responses = Vec::with_capacity(tiles.len());
    let mut missing = Vec::new();
    for tile in tiles {
        let path = tile_cache_path(cache_dir, layer, tile);
        match read_cached_response(&path, layer.namespace)? {
            Some(response) => responses.push(response),
            None => missing.push(tile),
        }
    }
    if missing.is_empty() {
        return Ok(filter_response(
            merge_responses(responses),
            requested_bounds,
        ));
    }

    // Source bundles made before the tile cache carry one exact-bounds
    // response plus coverage metadata. Use it when it covers this request so
    // those bundles still rebuild offline. New bundles carry version 2 and
    // tiled paths, while the importer keeps reading version 1 for this path.
    if let Some(response) = read_covering_legacy_response(cache_dir, legacy, requested_bounds)? {
        return Ok(response);
    }

    // Recheck after the pacing lock. Concurrent jobs asking for an
    // overlapping tile then share the first job's download.
    let _request_guard = loop {
        ensure_active(cancellation)?;
        match OVERPASS_REQUEST_LOCK.try_lock() {
            Ok(guard) => break guard,
            Err(std::sync::TryLockError::Poisoned(poisoned)) => {
                break poisoned.into_inner();
            }
            Err(std::sync::TryLockError::WouldBlock) => {
                thread::sleep(Duration::from_millis(50));
            }
        }
    };
    ensure_active(cancellation)?;
    let mut still_missing = Vec::new();
    for tile in missing {
        let path = tile_cache_path(cache_dir, layer, tile);
        match read_cached_response(&path, layer.namespace)? {
            Some(response) => responses.push(response),
            None => still_missing.push(tile),
        }
    }

    for tile_batch in still_missing.chunks(MAX_OVERPASS_TILES_PER_QUERY) {
        ensure_active(cancellation)?;
        let tile_bounds = tile_batch
            .iter()
            .map(|tile| tile.bounds())
            .collect::<Vec<_>>();
        let query = query_for_bounds(&tile_bounds);
        let downloaded = download_response(layer.namespace, &query, cancellation)?;
        responses.extend(cache_downloaded_tiles(
            cache_dir,
            layer,
            tile_batch,
            &tile_bounds,
            &downloaded,
            cancellation,
        )?);
    }

    Ok(filter_response(
        merge_responses(responses),
        requested_bounds,
    ))
}

fn cache_downloaded_tiles(
    cache_dir: &Path,
    layer: TileLayer<'_>,
    tiles: &[Tile],
    bounds: &[GeoBounds],
    downloaded: &OverpassResponse,
    cancellation: Option<&AtomicBool>,
) -> Result<Vec<OverpassResponse>> {
    ensure_active(cancellation)?;
    let mut responses = Vec::with_capacity(tiles.len());
    for (&tile, &bounds) in tiles.iter().zip(bounds) {
        ensure_active(cancellation)?;
        let tile_response = filtered_response(downloaded, bounds);
        let path = tile_cache_path(cache_dir, layer, tile);
        let bytes = serde_json::to_vec(&tile_response)
            .with_context(|| format!("serialize OpenStreetMap {} tile", layer.namespace))?;
        if let Err(error) = cache::store(&path, &bytes) {
            warn!(
                %error,
                path = %path.display(),
                "could not cache OpenStreetMap tile; using downloaded data"
            );
        }
        responses.push(tile_response);
    }
    Ok(responses)
}

fn read_covering_legacy_response(
    cache_dir: &Path,
    legacy: LegacyCache<'_>,
    requested_bounds: GeoBounds,
) -> Result<Option<OverpassResponse>> {
    if legacy.prefixes.is_empty() {
        return Ok(None);
    }
    let entries = match fs::read_dir(cache_dir) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => {
            return Err(error).with_context(|| {
                format!("scan legacy OpenStreetMap cache {}", cache_dir.display())
            });
        }
    };
    let mut candidates = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
            continue;
        };
        if !name.ends_with(".json.meta") {
            continue;
        }
        let bytes = match fs::read(&path) {
            Ok(bytes) => bytes,
            Err(error) => {
                warn!(%error, path = %path.display(), "could not read legacy OpenStreetMap cache metadata");
                continue;
            }
        };
        let metadata: LegacyCacheMetadata = match serde_json::from_slice(&bytes) {
            Ok(metadata) => metadata,
            Err(error) => {
                warn!(%error, path = %path.display(), "ignoring invalid legacy OpenStreetMap cache metadata");
                continue;
            }
        };
        if metadata.version != 1 || !bounds_cover(metadata.bounds(), requested_bounds) {
            continue;
        }
        let response_name = Path::new(&metadata.response_file);
        if response_name.file_name() != Some(response_name.as_os_str())
            || !legacy.prefixes.iter().any(|prefix| {
                metadata
                    .response_file
                    .strip_prefix(prefix)
                    .is_some_and(|suffix| suffix.starts_with('-') && suffix.ends_with(".json"))
            })
        {
            continue;
        }
        candidates.push((
            bounds_area(metadata.bounds()),
            cache_dir.join(response_name),
            path,
        ));
    }
    candidates.sort_by(|left, right| left.0.total_cmp(&right.0));
    for (_, response_path, metadata_path) in candidates {
        match read_cached_response(&response_path, "legacy")? {
            Some(mut response) => {
                cache::note(&metadata_path);
                response.elements.retain(|element| {
                    (legacy.accepts)(element)
                        && element_intersects_bounds(element, requested_bounds)
                });
                response.remark = None;
                return Ok(Some(response));
            }
            None => {
                let _ = fs::remove_file(metadata_path);
            }
        }
    }
    Ok(None)
}

fn bounds_cover(cached: GeoBounds, requested: GeoBounds) -> bool {
    let cached_parts = cached.split_at_antimeridian();
    requested.split_at_antimeridian().iter().all(|requested| {
        cached_parts.iter().any(|cached| {
            cached.south <= requested.south
                && cached.north >= requested.north
                && cached.west <= requested.west
                && cached.east >= requested.east
        })
    })
}

fn bounds_area(bounds: GeoBounds) -> f64 {
    bounds
        .split_at_antimeridian()
        .iter()
        .map(|part| (part.north - part.south) * (part.east - part.west))
        .sum()
}

pub(crate) fn merge_responses(responses: Vec<OverpassResponse>) -> OverpassResponse {
    let mut elements: Vec<OverpassWay> = Vec::new();
    let mut positions: HashMap<(String, u64), usize> = HashMap::new();
    for response in responses {
        for element in response.elements {
            if element.id == 0 {
                elements.push(element);
                continue;
            }
            let element_type = if element.element_type.is_empty() {
                "way".to_owned()
            } else {
                element.element_type.clone()
            };
            let key = (element_type, element.id);
            match positions.get(&key).copied() {
                Some(index) if richness(&element) > richness(&elements[index]) => {
                    elements[index] = element;
                }
                Some(_) => {}
                None => {
                    positions.insert(key, elements.len());
                    elements.push(element);
                }
            }
        }
    }
    elements.sort_by(|left, right| {
        left.element_type
            .cmp(&right.element_type)
            .then_with(|| left.id.cmp(&right.id))
    });
    OverpassResponse {
        elements,
        remark: None,
    }
}

fn richness(element: &OverpassWay) -> usize {
    element.geometry.len()
        + element
            .members
            .iter()
            .map(|member| member.geometry.len())
            .sum::<usize>()
        + element.tags.len()
}

pub(crate) fn tile_cache_path(cache_dir: &Path, layer: TileLayer<'_>, tile: Tile) -> PathBuf {
    debug_assert_eq!(layer.zoom, tile.zoom);
    cache_dir
        .join("tiles")
        .join(layer.namespace)
        .join(tile.zoom.to_string())
        .join(tile.x.to_string())
        .join(format!("{}.json", tile.y))
}

pub(crate) fn tiles_for_bounds(zoom: u8, bounds: GeoBounds) -> Vec<Tile> {
    let mut tiles = Vec::new();
    for part in bounds.split_at_antimeridian() {
        let x_start = tile_x(part.west, zoom);
        let x_end = tile_x(part.east, zoom);
        let y_start = tile_y(part.north, zoom);
        let y_end = tile_y(part.south, zoom);
        for x in x_start.min(x_end)..=x_start.max(x_end) {
            for y in y_start.min(y_end)..=y_start.max(y_end) {
                tiles.push(Tile { zoom, x, y });
            }
        }
    }
    tiles.sort_unstable();
    tiles.dedup();
    tiles
}

fn tile_x(longitude: f64, zoom: u8) -> u32 {
    let count = 1_u32 << zoom;
    let longitude = if longitude >= 180.0 {
        180.0 - 1e-10
    } else if longitude <= -180.0 {
        -180.0
    } else {
        normalize_longitude(longitude)
    };
    let x = ((longitude + 180.0) / 360.0 * f64::from(count)).floor();
    (x as i64).clamp(0, i64::from(count - 1)) as u32
}

fn tile_y(latitude: f64, zoom: u8) -> u32 {
    let count = 1_u32 << zoom;
    let latitude = latitude.clamp(-WEB_MERCATOR_MAX_LATITUDE, WEB_MERCATOR_MAX_LATITUDE);
    let radians = latitude.to_radians();
    let y = (1.0 - (radians.tan() + 1.0 / radians.cos()).ln() / std::f64::consts::PI)
        * 0.5
        * f64::from(count);
    (y.floor() as i64).clamp(0, i64::from(count - 1)) as u32
}

impl Tile {
    fn bounds(self) -> GeoBounds {
        let count = f64::from(1_u32 << self.zoom);
        GeoBounds {
            west: f64::from(self.x) / count * 360.0 - 180.0,
            east: f64::from(self.x + 1) / count * 360.0 - 180.0,
            north: tile_latitude(f64::from(self.y), count),
            south: tile_latitude(f64::from(self.y + 1), count),
        }
    }
}

fn tile_latitude(y: f64, tile_count: f64) -> f64 {
    let mercator = std::f64::consts::PI * (1.0 - 2.0 * y / tile_count);
    mercator.sinh().atan().to_degrees()
}

fn filter_response(
    mut response: OverpassResponse,
    requested_bounds: GeoBounds,
) -> OverpassResponse {
    response
        .elements
        .retain(|element| element_intersects_bounds(element, requested_bounds));
    response
}

fn filtered_response(response: &OverpassResponse, requested_bounds: GeoBounds) -> OverpassResponse {
    OverpassResponse {
        elements: response
            .elements
            .iter()
            .filter(|element| element_intersects_bounds(element, requested_bounds))
            .cloned()
            .collect(),
        remark: None,
    }
}

fn element_intersects_bounds(element: &OverpassWay, requested_bounds: GeoBounds) -> bool {
    geometry_intersects_bounds(&element.geometry, requested_bounds)
        || element
            .members
            .iter()
            .any(|member| geometry_intersects_bounds(&member.geometry, requested_bounds))
}

fn geometry_intersects_bounds(points: &[OverpassPoint], bounds: GeoBounds) -> bool {
    if points.is_empty() {
        return false;
    }
    let south = points
        .iter()
        .map(|point| point.lat)
        .fold(f64::INFINITY, f64::min);
    let north = points
        .iter()
        .map(|point| point.lat)
        .fold(f64::NEG_INFINITY, f64::max);
    bounds.split_at_antimeridian().iter().any(|part| {
        if north < part.south || south > part.north {
            return false;
        }
        // Unwrap every longitude around this part. A way from 179.9 to
        // -179.9 then spans 179.9..180.1 for the eastern part and
        // -180.1..-179.9 for the western one, instead of looking world-wide.
        let center = (part.west + part.east) * 0.5;
        if points
            .iter()
            .any(|point| point_inside_bounds(unwrapped_point(*point, center), *part))
            || points.windows(2).any(|segment| {
                segment_intersects_bounds(
                    unwrapped_point(segment[0], center),
                    unwrapped_point(segment[1], center),
                    *part,
                )
            })
        {
            return true;
        }
        let closed = points.len() >= 4
            && points
                .first()
                .zip(points.last())
                .is_some_and(|(first, last)| first.lat == last.lat && first.lon == last.lon);
        closed && ring_contains_point(points, [center, (part.south + part.north) * 0.5], center)
    })
}

fn unwrapped_point(point: OverpassPoint, center: f64) -> [f64; 2] {
    [center + normalize_longitude(point.lon - center), point.lat]
}

fn point_inside_bounds(point: [f64; 2], bounds: GeoBounds) -> bool {
    point[0] >= bounds.west
        && point[0] <= bounds.east
        && point[1] >= bounds.south
        && point[1] <= bounds.north
}

fn segment_intersects_bounds(start: [f64; 2], end: [f64; 2], bounds: GeoBounds) -> bool {
    let delta = [end[0] - start[0], end[1] - start[1]];
    let mut enter: f64 = 0.0;
    let mut exit: f64 = 1.0;
    for (direction, distance) in [
        (-delta[0], start[0] - bounds.west),
        (delta[0], bounds.east - start[0]),
        (-delta[1], start[1] - bounds.south),
        (delta[1], bounds.north - start[1]),
    ] {
        if direction == 0.0 {
            if distance < 0.0 {
                return false;
            }
            continue;
        }
        let ratio = distance / direction;
        if direction < 0.0 {
            enter = enter.max(ratio);
        } else {
            exit = exit.min(ratio);
        }
        if enter > exit {
            return false;
        }
    }
    true
}

fn ring_contains_point(ring: &[OverpassPoint], point: [f64; 2], center: f64) -> bool {
    let mut inside = false;
    let mut previous = ring.len() - 1;
    for current in 0..ring.len() {
        let (a, b) = (
            unwrapped_point(ring[current], center),
            unwrapped_point(ring[previous], center),
        );
        if (a[1] > point[1]) != (b[1] > point[1]) {
            let longitude = a[0] + (point[1] - a[1]) / (b[1] - a[1]) * (b[0] - a[0]);
            if point[0] < longitude {
                inside = !inside;
            }
        }
        previous = current;
    }
    inside
}

fn read_cached_response(path: &Path, cache_prefix: &str) -> Result<Option<OverpassResponse>> {
    match cache::read(path) {
        Ok(bytes) => match parse_response(&bytes, cache_prefix) {
            Ok(response) => Ok(Some(response)),
            Err(error) => {
                warn!(
                    %error,
                    path = %path.display(),
                    "removing incomplete OpenStreetMap cache entry"
                );
                if let Err(remove_error) = fs::remove_file(path)
                    && remove_error.kind() != std::io::ErrorKind::NotFound
                {
                    warn!(
                        error = %remove_error,
                        path = %path.display(),
                        "could not remove incomplete OpenStreetMap cache entry"
                    );
                }
                Ok(None)
            }
        },
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => {
            Err(error).with_context(|| format!("read OpenStreetMap cache {}", path.display()))
        }
    }
}

fn download_response(
    cache_prefix: &str,
    query: &str,
    cancellation: Option<&AtomicBool>,
) -> Result<OverpassResponse> {
    let client =
        http::blocking_client(Duration::from_secs(45)).context("build OpenStreetMap client")?;
    let configured_url = env::var("OVERPASS_BASE_URL").ok();
    let preferred_endpoint = PREFERRED_OVERPASS_ENDPOINT.load(Ordering::Relaxed);
    let urls = overpass_urls(configured_url.as_deref(), preferred_endpoint);
    let mut failures = Vec::new();
    for attempt in 0..OVERPASS_ATTEMPTS {
        if attempt > 0 {
            let slices = OVERPASS_RETRY_DELAY.as_millis().div_ceil(50) as usize;
            for _ in 0..slices {
                ensure_active(cancellation)?;
                thread::sleep(Duration::from_millis(50));
            }
        }
        for &(endpoint_index, base_url) in &urls {
            ensure_active(cancellation)?;
            match client.post(base_url).form(&[("data", query)]).send() {
                Ok(response) if response.status().is_success() => match response.bytes() {
                    Ok(response_bytes) => match parse_response(&response_bytes, cache_prefix) {
                        Ok(parsed) => {
                            if configured_url.is_none() {
                                PREFERRED_OVERPASS_ENDPOINT
                                    .store(endpoint_index, Ordering::Relaxed);
                            }
                            return Ok(parsed);
                        }
                        Err(error) => failures.push(format!("{base_url}: {error:#}")),
                    },
                    Err(error) => failures.push(format!("{base_url}: {error}")),
                },
                Ok(response) => failures.push(format!("{base_url}: HTTP {}", response.status())),
                Err(error) => failures.push(format!("{base_url}: {error}")),
            }
        }
    }
    bail!(
        "OpenStreetMap Overpass rejected the {cache_prefix} request after {OVERPASS_ATTEMPTS} attempts ({})",
        failures.join("; ")
    )
}

fn ensure_active(cancellation: Option<&AtomicBool>) -> Result<()> {
    if cancellation.is_some_and(|flag| flag.load(Ordering::Acquire)) {
        bail!("preview superseded by newer settings");
    }
    Ok(())
}

pub(crate) fn parse_response(bytes: &[u8], cache_prefix: &str) -> Result<OverpassResponse> {
    let response: OverpassResponse = serde_json::from_slice(bytes)
        .with_context(|| format!("parse OpenStreetMap Overpass {cache_prefix} response"))?;
    if let Some(remark) = response.remark.as_deref() {
        bail!("OpenStreetMap Overpass returned incomplete {cache_prefix} data: {remark}");
    }
    Ok(response)
}

pub(crate) fn overpass_urls(
    configured_url: Option<&str>,
    preferred_endpoint: usize,
) -> Vec<(usize, &str)> {
    if let Some(url) = configured_url {
        return vec![(0, url)];
    }
    let mut urls = vec![(0, DEFAULT_OVERPASS_URL), (1, FALLBACK_OVERPASS_URL)];
    let endpoint_count = urls.len();
    urls.rotate_left(preferred_endpoint % endpoint_count);
    urls
}

#[cfg(test)]
mod tests {
    use super::*;

    fn point(id: u64, lat: f64, lon: f64) -> OverpassWay {
        OverpassWay {
            id,
            element_type: "way".into(),
            geometry: vec![OverpassPoint { lat, lon }],
            ..Default::default()
        }
    }

    #[test]
    fn nearby_views_share_fixed_tiles() {
        let first = tiles_for_bounds(12, GeoBounds::around(46.8523, -121.7603, 2.0));
        let second = tiles_for_bounds(12, GeoBounds::around(46.8530, -121.7590, 2.0));
        assert_eq!(first, second);
    }

    #[test]
    fn antimeridian_views_use_tiles_at_both_world_edges() {
        let tiles = tiles_for_bounds(8, GeoBounds::around(0.0, 179.95, 20.0));
        assert!(tiles.iter().any(|tile| tile.x == 0));
        assert!(tiles.iter().any(|tile| tile.x == 255));
        assert!(tiles.len() < 10, "date-line wrap must not select the world");
    }

    #[test]
    fn antimeridian_geometry_survives_clipping_on_both_sides() {
        let crossing = OverpassResponse {
            elements: vec![OverpassWay {
                id: 1,
                element_type: "way".into(),
                geometry: vec![
                    OverpassPoint {
                        lat: 0.0,
                        lon: 179.9,
                    },
                    OverpassPoint {
                        lat: 0.0,
                        lon: -179.9,
                    },
                ],
                ..Default::default()
            }],
            remark: None,
        };
        for bounds in GeoBounds::around(0.0, 179.95, 20.0).split_at_antimeridian() {
            assert_eq!(filter_response(crossing.clone(), bounds).elements.len(), 1);
        }
    }

    #[test]
    fn a_geometry_bounding_box_is_not_treated_as_an_intersection() {
        let bounds = GeoBounds {
            south: -0.25,
            north: 0.25,
            west: -0.25,
            east: 0.25,
        };
        let bent_around_the_view = vec![
            OverpassPoint {
                lat: -1.0,
                lon: -1.0,
            },
            OverpassPoint {
                lat: 1.0,
                lon: -1.0,
            },
            OverpassPoint { lat: 1.0, lon: 1.0 },
        ];
        assert!(!geometry_intersects_bounds(&bent_around_the_view, bounds));
    }

    #[test]
    fn a_closed_area_that_contains_the_view_is_kept() {
        let bounds = GeoBounds {
            south: -0.25,
            north: 0.25,
            west: -0.25,
            east: 0.25,
        };
        let surrounding_ring = vec![
            OverpassPoint {
                lat: -1.0,
                lon: -1.0,
            },
            OverpassPoint {
                lat: -1.0,
                lon: 1.0,
            },
            OverpassPoint { lat: 1.0, lon: 1.0 },
            OverpassPoint {
                lat: 1.0,
                lon: -1.0,
            },
            OverpassPoint {
                lat: -1.0,
                lon: -1.0,
            },
        ];
        assert!(geometry_intersects_bounds(&surrounding_ring, bounds));
    }

    #[test]
    fn merging_tiles_deduplicates_ids_and_keeps_the_richer_copy() {
        let sparse = point(42, 46.8, -121.8);
        let mut rich = sparse.clone();
        rich.geometry.push(OverpassPoint {
            lat: 46.9,
            lon: -121.7,
        });
        let merged = merge_responses(vec![
            OverpassResponse {
                elements: vec![sparse, point(7, 46.8, -121.9)],
                remark: None,
            },
            OverpassResponse {
                elements: vec![rich],
                remark: None,
            },
        ]);
        assert_eq!(merged.elements.len(), 2);
        assert_eq!(merged.elements[1].id, 42);
        assert_eq!(merged.elements[1].geometry.len(), 2);
    }

    #[test]
    fn empty_tiles_are_valid_cached_answers() {
        let root =
            std::env::temp_dir().join(format!("toposaic-osm-tile-test-{}", uuid::Uuid::new_v4()));
        let layer = TileLayer::new("test-v1", 10);
        let bounds = GeoBounds::around(46.8523, -121.7603, 0.25);
        for tile in tiles_for_bounds(layer.zoom, bounds) {
            let path = tile_cache_path(&root, layer, tile);
            cache::store(&path, br#"{"elements":[]}"#).unwrap();
        }
        let response = fetch_tiled_response(
            &root,
            layer,
            LegacyCache::new(&[]),
            bounds,
            |_| panic!("a complete empty cache must not reach Overpass"),
            None,
        )
        .unwrap();
        assert!(response.elements.is_empty());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn a_covering_legacy_bundle_response_still_serves_offline() {
        let root =
            std::env::temp_dir().join(format!("toposaic-osm-legacy-test-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&root).unwrap();
        let requested = GeoBounds::around(46.8523, -121.7603, 0.25);
        let cached = GeoBounds::around(46.8523, -121.7603, 2.0);
        let response_name = "roads-v2-major-deadbeef.json";
        let response_path = root.join(response_name);
        cache::store(
            &response_path,
            &serde_json::to_vec(&OverpassResponse {
                elements: vec![point(42, 46.8523, -121.7603)],
                remark: None,
            })
            .unwrap(),
        )
        .unwrap();
        cache::store(
            &root.join(format!("{response_name}.meta")),
            &serde_json::to_vec(&LegacyCacheMetadata {
                version: 1,
                south: cached.south,
                north: cached.north,
                west: cached.west,
                east: cached.east,
                query_variant_hash: 0,
                response_file: response_name.into(),
            })
            .unwrap(),
        )
        .unwrap();

        let response = fetch_tiled_response(
            &root,
            TileLayer::new("roads-v3-major", 10),
            LegacyCache::new(&["roads-v2-major"]),
            requested,
            |_| panic!("a covering legacy response must not reach Overpass"),
            None,
        )
        .unwrap();
        assert_eq!(response.elements.len(), 1);
        assert_eq!(response.elements[0].id, 42);

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn cancelled_preview_never_queues_an_overpass_request() {
        let cancelled = AtomicBool::new(true);
        let error = fetch_tiled_response(
            Path::new("/unused"),
            TileLayer::new("test-v1", 10),
            LegacyCache::new(&[]),
            GeoBounds::around(46.8523, -121.7603, 0.25),
            |_| panic!("a cancelled preview must not build a query"),
            Some(&cancelled),
        )
        .unwrap_err();
        assert!(error.to_string().contains("preview superseded"));
    }

    #[test]
    fn cancellation_after_a_download_skips_tile_splitting() {
        let root = std::env::temp_dir().join(format!(
            "toposaic-osm-cancelled-split-test-{}",
            uuid::Uuid::new_v4()
        ));
        let layer = TileLayer::new("test-v1", 10);
        let tile = Tile {
            zoom: 10,
            x: 164,
            y: 353,
        };
        let cancelled = AtomicBool::new(true);
        let error = cache_downloaded_tiles(
            &root,
            layer,
            &[tile],
            &[tile.bounds()],
            &OverpassResponse {
                elements: vec![point(42, 46.8, -121.8)],
                remark: None,
            },
            Some(&cancelled),
        )
        .unwrap_err();
        assert!(error.to_string().contains("preview superseded"));
        assert!(!tile_cache_path(&root, layer, tile).exists());
    }

    #[test]
    fn falls_back_to_a_second_overpass_instance_unless_one_is_configured() {
        assert_eq!(
            overpass_urls(None, 0),
            vec![(0, DEFAULT_OVERPASS_URL), (1, FALLBACK_OVERPASS_URL)]
        );
        assert_eq!(
            overpass_urls(None, 1),
            vec![(1, FALLBACK_OVERPASS_URL), (0, DEFAULT_OVERPASS_URL)]
        );
        assert_eq!(
            overpass_urls(Some("http://127.0.0.1:1234/api/interpreter"), 1),
            vec![(0, "http://127.0.0.1:1234/api/interpreter")]
        );
    }

    #[test]
    fn rejects_partial_overpass_responses_with_timeout_remarks() {
        let partial = br#"{"remark":"runtime error: Query timed out","elements":[{"type":"way"}]}"#;
        let error = parse_response(partial, "buildings").unwrap_err();
        assert!(error.to_string().contains("incomplete buildings data"));
        assert!(error.to_string().contains("Query timed out"));
        assert!(parse_response(br#"{"elements":[]}"#, "buildings").is_ok());
    }
}
