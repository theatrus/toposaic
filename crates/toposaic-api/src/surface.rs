use std::{
    collections::HashMap,
    env, fs,
    path::{Path, PathBuf},
    sync::{
        Mutex,
        atomic::{AtomicUsize, Ordering},
    },
    thread,
    time::Duration,
};

use anyhow::{Context, Result, bail};
use geotiff_reader::GeoTiffFile;
use serde::Deserialize;
use toposaic_core::{GenerationSpec, HeightField, RoadDetail, SurfaceClass, SurfaceField};
use tracing::warn;

use crate::{
    cache,
    geo::{GeoBounds, normalize_longitude},
    http,
};

const WORLD_COVER_BASE_URL: &str =
    "https://esa-worldcover.s3.eu-central-1.amazonaws.com/v200/2021/map";
const WORLD_COVER_INFO_URL: &str = "https://worldcover2021.esa.int/download";
const WORLD_COVER_ATTRIBUTION: &str = "© ESA WorldCover project / Contains modified Copernicus Sentinel data (2021) processed by ESA WorldCover consortium";
const DEFAULT_OVERPASS_URL: &str = "https://overpass-api.de/api/interpreter";
const FALLBACK_OVERPASS_URL: &str = "https://maps.mail.ru/osm/tools/overpass/api/interpreter";
const OPENSTREETMAP_COPYRIGHT_URL: &str = "https://www.openstreetmap.org/copyright";
const MAJOR_HIGHWAYS: &str =
    "motorway|motorway_link|trunk|trunk_link|primary|primary_link|secondary|secondary_link";
const MINOR_HIGHWAYS: &str = "motorway|motorway_link|trunk|trunk_link|primary|primary_link|secondary|secondary_link|tertiary|tertiary_link|unclassified";
const STREET_HIGHWAYS: &str = "motorway|motorway_link|trunk|trunk_link|primary|primary_link|secondary|secondary_link|tertiary|tertiary_link|unclassified|residential|living_street|service|pedestrian|road";
const ALL_ROUTE_HIGHWAYS: &str = "motorway|motorway_link|trunk|trunk_link|primary|primary_link|secondary|secondary_link|tertiary|tertiary_link|unclassified|residential|living_street|service|pedestrian|road|track|cycleway|path|footway|bridleway|steps";
const PATH_HIGHWAYS: &str = "path|footway|bridleway|track|cycleway|steps";
const WATERWAYS: &str = "river|stream|canal";
const OVERPASS_ATTEMPTS: usize = 2;
const OVERPASS_RETRY_DELAY: Duration = Duration::from_millis(750);
static OVERPASS_REQUEST_LOCK: Mutex<()> = Mutex::new(());
static PREFERRED_OVERPASS_ENDPOINT: AtomicUsize = AtomicUsize::new(0);

#[derive(Debug)]
struct SamplePoint {
    output_index: usize,
    longitude: f64,
    latitude: f64,
}

#[derive(Debug, Deserialize)]
struct OverpassResponse {
    #[serde(default)]
    elements: Vec<OverpassWay>,
    #[serde(default)]
    remark: Option<String>,
}

#[derive(Debug, Deserialize)]
struct OverpassWay {
    #[serde(default)]
    tags: HashMap<String, String>,
    #[serde(default)]
    geometry: Vec<OverpassPoint>,
}

#[derive(Debug, Deserialize)]
struct OverpassPoint {
    lat: f64,
    lon: f64,
}

#[derive(Debug)]
struct RouteCounts {
    roads: usize,
    trails: usize,
    bridges: usize,
    detail: RoadDetail,
    highway_filter: &'static str,
    fallback: bool,
}

#[derive(Debug, Default)]
struct WaterCounts {
    lines: usize,
    available_lines: usize,
    areas: usize,
}

struct RouteFeature {
    points: Vec<[f32; 2]>,
    width_scale: f32,
    path_or_trail: bool,
    bridge_elevations_m: Option<[f32; 2]>,
}

struct WaterwayFeature {
    points: Vec<[f32; 2]>,
    width_scale: f32,
    major: bool,
}

pub fn fetch_surface_field(
    spec: &GenerationSpec,
    height_field: &HeightField,
    map_cache_dir: &Path,
) -> Result<SurfaceField> {
    let samples = spec
        .effective_samples_per_piece()
        .min(height_field.samples_per_piece(spec) as u32)
        .max(16);
    let (width, height) = spec.sample_grid_dimensions(samples);
    let bounds = bounds_for(spec);
    let mut classes = vec![SurfaceClass::Rock; width * height];
    let mut source = String::new();

    if spec.color_output.enabled {
        let mut tiles = HashMap::<String, Vec<SamplePoint>>::new();
        for row in 0..height {
            let v = row as f64 / (height - 1) as f64;
            let latitude = bounds.south + (bounds.north - bounds.south) * v;
            for column in 0..width {
                let u = column as f64 / (width - 1) as f64;
                let longitude = normalize_longitude(bounds.west + (bounds.east - bounds.west) * u);
                tiles
                    .entry(world_cover_tile(longitude, latitude))
                    .or_default()
                    .push(SamplePoint {
                        output_index: row * width + column,
                        longitude,
                        latitude,
                    });
            }
        }
        let mut tile_names = tiles.keys().cloned().collect::<Vec<_>>();
        tile_names.sort();
        for tile_name in &tile_names {
            let points = tiles
                .remove(tile_name)
                .context("land-cover tile group disappeared")?;
            sample_tile(
                tile_name,
                &points,
                width,
                height,
                &mut classes,
                &map_cache_dir.join("world-cover"),
            )?;
        }
        source = format!(
            "ESA WorldCover 2021 v200, 10 m, EPSG:4326, tiles {}; CC BY 4.0; source: {WORLD_COVER_INFO_URL}; {WORLD_COVER_ATTRIBUTION}",
            tile_names.join(", ")
        );
    }

    let mut field = SurfaceField::new(width, height, classes, source)?;
    if spec.color_output.enabled {
        field.filter_small_patches(spec.width_mm, spec.color_output.minimum_patch_mm);
        if spec.color_output.osm_water_enabled {
            match paint_water(spec, bounds, &map_cache_dir.join("osm"), &mut field) {
                Ok(counts) => append_source(
                    &mut field.source,
                    format!(
                        "waterways: {} of {} lines after {:.0}% coverage cutoff and {} water areas from OpenStreetMap via Overpass API; © OpenStreetMap contributors, ODbL; {OPENSTREETMAP_COPYRIGHT_URL}",
                        counts.lines,
                        counts.available_lines,
                        spec.color_output.waterway_coverage_percent,
                        counts.areas
                    ),
                ),
                Err(error) => {
                    warn!(%error, "OpenStreetMap water unavailable; using WorldCover water");
                    append_source(
                        &mut field.source,
                        "OpenStreetMap water unavailable; used WorldCover water only",
                    );
                }
            }
        }
    }
    if spec.color_output.enabled && spec.color_output.roads_enabled {
        match paint_roads_or_trails(
            spec,
            height_field,
            bounds,
            &map_cache_dir.join("osm"),
            &mut field,
        ) {
            Ok(counts) => {
                let fallback = if counts.fallback {
                    " (trail fallback)"
                } else {
                    ""
                };
                append_source(
                    &mut field.source,
                    format!(
                        "routes{fallback}: {} roads and streets, {} paths and trails, and {} tagged bridges from OpenStreetMap via Overpass API; detail={}; highway={}; © OpenStreetMap contributors, ODbL; {OPENSTREETMAP_COPYRIGHT_URL}",
                        counts.roads,
                        counts.trails,
                        counts.bridges,
                        road_detail_name(counts.detail),
                        counts.highway_filter,
                    ),
                );
            }
            Err(error) => {
                warn!(%error, "OpenStreetMap roads unavailable; omitting route overlay");
                append_source(
                    &mut field.source,
                    "OpenStreetMap roads unavailable; route overlay omitted",
                );
            }
        }
    }
    if spec.buildings.enabled {
        match paint_buildings(spec, bounds, &map_cache_dir.join("osm"), &mut field) {
            Ok(count) => append_source(
                &mut field.source,
                format!(
                    "buildings: {count} OpenStreetMap footprints via Overpass API; © OpenStreetMap contributors, ODbL; {OPENSTREETMAP_COPYRIGHT_URL}"
                ),
            ),
            Err(error) => {
                warn!(%error, "OpenStreetMap buildings unavailable; omitting buildings");
                append_source(
                    &mut field.source,
                    "OpenStreetMap buildings unavailable; building overlay omitted",
                );
            }
        }
    }
    Ok(field)
}

fn append_source(source: &mut String, addition: impl AsRef<str>) {
    if !source.is_empty() {
        source.push_str("; ");
    }
    source.push_str(addition.as_ref());
}

fn normalized_osm_points(
    way: &OverpassWay,
    spec: &GenerationSpec,
    bounds: GeoBounds,
) -> Vec<[f32; 2]> {
    way.geometry
        .iter()
        .map(|point| {
            let longitude = unwrap_longitude(point.lon, spec.center_lon);
            [
                ((longitude - bounds.west) / (bounds.east - bounds.west)) as f32,
                ((point.lat - bounds.south) / (bounds.north - bounds.south)) as f32,
            ]
        })
        .collect()
}

fn paint_water(
    spec: &GenerationSpec,
    bounds: GeoBounds,
    cache_dir: &Path,
    field: &mut SurfaceField,
) -> Result<WaterCounts> {
    let water = fetch_osm_response(spec, cache_dir, "water", water_query(bounds))?;
    let mut counts = WaterCounts::default();
    let mut lines = Vec::new();
    for way in water.elements {
        if is_water_area(&way.tags) {
            if way.geometry.len() >= 3 {
                field.paint_surface_area(
                    &normalized_osm_points(&way, spec, bounds),
                    SurfaceClass::Water,
                );
                counts.areas += 1;
            }
            continue;
        }
        if way.geometry.len() < 2 || is_tunnel(&way.tags) {
            continue;
        }
        let Some(width_scale) = waterway_width_scale(&way.tags) else {
            continue;
        };
        lines.push(WaterwayFeature {
            points: normalized_osm_points(&way, spec, bounds),
            width_scale,
            major: is_major_waterway(&way.tags),
        });
    }
    counts.available_lines = lines.len();
    let lines = select_waterway_features(spec, lines);
    counts.lines = lines.len();
    for line in lines {
        field.paint_polyline(
            &line.points,
            spec.width_mm,
            waterway_print_width(spec, &line),
            SurfaceClass::Water,
        );
    }
    Ok(counts)
}

fn select_waterway_features(
    spec: &GenerationSpec,
    features: Vec<WaterwayFeature>,
) -> Vec<WaterwayFeature> {
    if spec.color_output.waterway_coverage_percent >= 100.0 {
        return features;
    }
    let coverage_budget =
        spec.width_mm * spec.height_mm() * spec.color_output.waterway_coverage_percent / 100.0;
    let (mut major, mut minor): (Vec<_>, Vec<_>) =
        features.into_iter().partition(|feature| feature.major);
    major.sort_by(|left, right| {
        waterway_printed_area(spec, right).total_cmp(&waterway_printed_area(spec, left))
    });
    minor.sort_by(|left, right| {
        waterway_printed_area(spec, right).total_cmp(&waterway_printed_area(spec, left))
    });
    let mut used_area = major
        .iter()
        .map(|feature| waterway_printed_area(spec, feature))
        .sum::<f32>();
    for feature in minor {
        let area = waterway_printed_area(spec, &feature);
        if used_area + area <= coverage_budget {
            used_area += area;
            major.push(feature);
        }
    }
    major
}

fn waterway_printed_area(spec: &GenerationSpec, feature: &WaterwayFeature) -> f32 {
    feature
        .points
        .windows(2)
        .map(|points| {
            let width = (points[1][0] - points[0][0]) * spec.width_mm;
            let height = (points[1][1] - points[0][1]) * spec.height_mm();
            width.hypot(height)
        })
        .sum::<f32>()
        * waterway_print_width(spec, feature)
}

fn waterway_print_width(spec: &GenerationSpec, feature: &WaterwayFeature) -> f32 {
    (spec.color_output.road_width_mm * feature.width_scale).max(0.6)
}

fn bounds_for(spec: &GenerationSpec) -> GeoBounds {
    GeoBounds::around(spec.center_lat, spec.center_lon, spec.ground_span_km)
}

fn paint_roads_or_trails(
    spec: &GenerationSpec,
    height_field: &HeightField,
    bounds: GeoBounds,
    cache_dir: &Path,
    field: &mut SurfaceField,
) -> Result<RouteCounts> {
    let detail = spec.color_output.road_detail.resolve(spec.ground_span_km);
    let highway_filter = road_highway_filter(detail);
    let cache_prefix = road_cache_prefix(detail);
    let routes = fetch_osm_ways(spec, bounds, cache_dir, cache_prefix, highway_filter)?;
    let (road_count, trail_count, bridge_count) =
        paint_osm_ways(spec, height_field, bounds, field, routes);
    if road_count + trail_count > 0 || detail == RoadDetail::All {
        return Ok(RouteCounts {
            roads: road_count,
            trails: trail_count,
            bridges: bridge_count,
            detail,
            highway_filter,
            fallback: false,
        });
    }
    let trails = fetch_osm_ways(
        spec,
        bounds,
        cache_dir,
        "roads-v2-path-fallback",
        PATH_HIGHWAYS,
    )?;
    let (road_count, trail_count, bridge_count) =
        paint_osm_ways(spec, height_field, bounds, field, trails);
    Ok(RouteCounts {
        roads: road_count,
        trails: trail_count,
        bridges: bridge_count,
        detail,
        highway_filter: PATH_HIGHWAYS,
        fallback: true,
    })
}

fn paint_osm_ways(
    spec: &GenerationSpec,
    height_field: &HeightField,
    bounds: GeoBounds,
    field: &mut SurfaceField,
    response: OverpassResponse,
) -> (usize, usize, usize) {
    let mut features = Vec::new();
    for way in response.elements {
        if way.geometry.len() < 2 || is_tunnel(&way.tags) {
            continue;
        }
        let Some(scale) = road_width_scale(&way.tags) else {
            continue;
        };
        let points = normalized_osm_points(&way, spec, bounds);
        let bridge_elevations_m = is_bridge(&way.tags).then(|| {
            let first = points[0];
            let last = points[points.len() - 1];
            [
                height_field.elevation_m_at(first[0], first[1]),
                height_field.elevation_m_at(last[0], last[1]),
            ]
        });
        features.push(RouteFeature {
            points,
            width_scale: scale,
            path_or_trail: is_path_or_trail(&way.tags),
            bridge_elevations_m,
        });
    }
    let density_scale = if spec.color_output.adaptive_road_widths {
        route_density_scale(spec, &features)
    } else {
        1.0
    };
    for feature in &features {
        let line_width =
            (spec.color_output.road_width_mm * feature.width_scale * density_scale).max(0.4);
        if let Some(elevations_m) = feature.bridge_elevations_m {
            field.paint_bridge_polyline(&feature.points, spec.width_mm, line_width, elevations_m);
        } else {
            field.paint_polyline(
                &feature.points,
                spec.width_mm,
                line_width,
                SurfaceClass::Road,
            );
        }
    }
    let trail_count = features
        .iter()
        .filter(|feature| feature.path_or_trail)
        .count();
    (
        features.len() - trail_count,
        trail_count,
        features
            .iter()
            .filter(|feature| feature.bridge_elevations_m.is_some())
            .count(),
    )
}

fn road_highway_filter(detail: RoadDetail) -> &'static str {
    match detail {
        RoadDetail::Automatic => unreachable!("automatic road detail must be resolved"),
        RoadDetail::Major => MAJOR_HIGHWAYS,
        RoadDetail::Minor => MINOR_HIGHWAYS,
        RoadDetail::Streets => STREET_HIGHWAYS,
        RoadDetail::All => ALL_ROUTE_HIGHWAYS,
    }
}

fn road_cache_prefix(detail: RoadDetail) -> &'static str {
    match detail {
        RoadDetail::Automatic => unreachable!("automatic road detail must be resolved"),
        RoadDetail::Major => "roads-v2-major",
        RoadDetail::Minor => "roads-v2-minor",
        RoadDetail::Streets => "roads-v2-streets",
        RoadDetail::All => "roads-v2-all",
    }
}

fn road_detail_name(detail: RoadDetail) -> &'static str {
    match detail {
        RoadDetail::Automatic => "automatic",
        RoadDetail::Major => "major",
        RoadDetail::Minor => "minor",
        RoadDetail::Streets => "streets",
        RoadDetail::All => "all",
    }
}

fn route_density_scale(spec: &GenerationSpec, features: &[RouteFeature]) -> f32 {
    let printed_length = features
        .iter()
        .map(|feature| {
            feature
                .points
                .windows(2)
                .map(|points| {
                    let width = (points[1][0] - points[0][0]) * spec.width_mm;
                    let height = (points[1][1] - points[0][1]) * spec.height_mm();
                    width.hypot(height)
                })
                .sum::<f32>()
                * spec.color_output.road_width_mm
                * feature.width_scale
        })
        .sum::<f32>();
    let model_area = spec.width_mm * spec.height_mm();
    let estimated_coverage = printed_length / model_area.max(f32::EPSILON);
    (0.06 / estimated_coverage.max(0.06)).clamp(0.35, 1.0)
}

fn paint_buildings(
    spec: &GenerationSpec,
    bounds: GeoBounds,
    cache_dir: &Path,
    field: &mut SurfaceField,
) -> Result<usize> {
    let response = fetch_osm_response(spec, cache_dir, "buildings", building_query(bounds))?;
    let mut painted = 0;
    for building in response.elements {
        if building.geometry.len() < 3 {
            continue;
        }
        let points = normalized_osm_points(&building, spec, bounds);
        field.paint_building(&points, building_height_m(&building.tags));
        painted += 1;
    }
    Ok(painted)
}

fn building_height_m(tags: &HashMap<String, String>) -> f32 {
    tags.get("height")
        .and_then(|value| first_number(value))
        .or_else(|| {
            tags.get("building:levels")
                .and_then(|value| first_number(value))
                .map(|levels| levels * 3.0)
        })
        .unwrap_or(8.0)
        .clamp(2.5, 200.0)
}

fn first_number(value: &str) -> Option<f32> {
    let number = value
        .trim()
        .chars()
        .take_while(|character| character.is_ascii_digit() || *character == '.')
        .collect::<String>();
    number.parse().ok()
}

fn fetch_osm_ways(
    spec: &GenerationSpec,
    bounds: GeoBounds,
    cache_dir: &Path,
    cache_prefix: &str,
    highway_filter: &str,
) -> Result<OverpassResponse> {
    fetch_osm_response(
        spec,
        cache_dir,
        cache_prefix,
        overpass_query(bounds, highway_filter),
    )
}

fn fetch_osm_response(
    spec: &GenerationSpec,
    cache_dir: &Path,
    cache_prefix: &str,
    query: String,
) -> Result<OverpassResponse> {
    fs::create_dir_all(cache_dir)
        .with_context(|| format!("create OpenStreetMap cache {}", cache_dir.display()))?;
    let cache_path = osm_cache_path(spec, cache_dir, cache_prefix);
    if let Some(response) = read_cached_osm_response(&cache_path, cache_prefix)? {
        return Ok(response);
    }
    let _request_guard = OVERPASS_REQUEST_LOCK
        .lock()
        .map_err(|_| anyhow::anyhow!("OpenStreetMap request lock was poisoned"))?;
    if let Some(response) = read_cached_osm_response(&cache_path, cache_prefix)? {
        return Ok(response);
    }

    let client =
        http::blocking_client(Duration::from_secs(45)).context("build OpenStreetMap client")?;
    let configured_url = env::var("OVERPASS_BASE_URL").ok();
    let preferred_endpoint = PREFERRED_OVERPASS_ENDPOINT.load(Ordering::Relaxed);
    let urls = overpass_urls(configured_url.as_deref(), preferred_endpoint);
    let mut failures = Vec::new();
    for attempt in 0..OVERPASS_ATTEMPTS {
        if attempt > 0 {
            thread::sleep(OVERPASS_RETRY_DELAY);
        }
        for &(endpoint_index, base_url) in &urls {
            match client
                .post(base_url)
                .form(&[("data", query.as_str())])
                .send()
            {
                Ok(response) if response.status().is_success() => match response.bytes() {
                    Ok(response_bytes) => {
                        let bytes = response_bytes.to_vec();
                        match parse_osm_response(&bytes, cache_prefix) {
                            Ok(parsed) => {
                                if configured_url.is_none() {
                                    PREFERRED_OVERPASS_ENDPOINT
                                        .store(endpoint_index, Ordering::Relaxed);
                                }
                                if let Err(error) = cache::store(&cache_path, &bytes) {
                                    warn!(
                                        %error,
                                        path = %cache_path.display(),
                                        "could not cache OpenStreetMap response; using downloaded data"
                                    );
                                }
                                return Ok(parsed);
                            }
                            Err(error) => failures.push(format!("{base_url}: {error:#}")),
                        }
                    }
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

fn read_cached_osm_response(
    cache_path: &Path,
    cache_prefix: &str,
) -> Result<Option<OverpassResponse>> {
    match fs::read(cache_path) {
        Ok(bytes) => match parse_osm_response(&bytes, cache_prefix) {
            Ok(response) => Ok(Some(response)),
            Err(error) => {
                warn!(
                    %error,
                    path = %cache_path.display(),
                    "ignoring incomplete OpenStreetMap cache entry"
                );
                Ok(None)
            }
        },
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => {
            Err(error).with_context(|| format!("read OpenStreetMap cache {}", cache_path.display()))
        }
    }
}

fn parse_osm_response(bytes: &[u8], cache_prefix: &str) -> Result<OverpassResponse> {
    let response: OverpassResponse = serde_json::from_slice(bytes)
        .with_context(|| format!("parse OpenStreetMap Overpass {cache_prefix} response"))?;
    if let Some(remark) = response.remark.as_deref() {
        bail!("OpenStreetMap Overpass returned incomplete {cache_prefix} data: {remark}");
    }
    Ok(response)
}

fn overpass_urls(configured_url: Option<&str>, preferred_endpoint: usize) -> Vec<(usize, &str)> {
    if let Some(url) = configured_url {
        return vec![(0, url)];
    }
    let mut urls = vec![(0, DEFAULT_OVERPASS_URL), (1, FALLBACK_OVERPASS_URL)];
    let endpoint_count = urls.len();
    urls.rotate_left(preferred_endpoint % endpoint_count);
    urls
}

fn osm_cache_path(spec: &GenerationSpec, cache_dir: &Path, cache_prefix: &str) -> PathBuf {
    cache_dir.join(format!(
        "{cache_prefix}-{:.5}-{:.5}-{:.3}.json",
        spec.center_lat, spec.center_lon, spec.ground_span_km,
    ))
}

fn overpass_query(bounds: GeoBounds, highway_filter: &str) -> String {
    let ways = bounds
        .split_at_antimeridian()
        .iter()
        .map(|bounds| {
            format!(
                "way[\"highway\"~\"^({highway_filter})$\"][\"area\"!=\"yes\"]({south:.7},{west:.7},{north:.7},{east:.7});",
                south = bounds.south,
                west = bounds.west,
                north = bounds.north,
                east = bounds.east,
            )
        })
        .collect::<String>();
    format!("[out:json][timeout:30];({ways});out tags geom;")
}

fn building_query(bounds: GeoBounds) -> String {
    let ways = bounds
        .split_at_antimeridian()
        .iter()
        .map(|bounds| {
            format!(
                "way[\"building\"][\"building\"!=\"no\"]({south:.7},{west:.7},{north:.7},{east:.7});",
                south = bounds.south,
                west = bounds.west,
                north = bounds.north,
                east = bounds.east,
            )
        })
        .collect::<String>();
    format!("[out:json][timeout:60];({ways});out tags geom;")
}

fn water_query(bounds: GeoBounds) -> String {
    let ways = bounds
        .split_at_antimeridian()
        .iter()
        .map(|bounds| {
            format!(
                "way[\"waterway\"~\"^({WATERWAYS})$\"][\"area\"!=\"yes\"]({south:.7},{west:.7},{north:.7},{east:.7});way[\"natural\"=\"water\"]({south:.7},{west:.7},{north:.7},{east:.7});way[\"waterway\"=\"riverbank\"]({south:.7},{west:.7},{north:.7},{east:.7});",
                south = bounds.south,
                west = bounds.west,
                north = bounds.north,
                east = bounds.east,
            )
        })
        .collect::<String>();
    format!("[out:json][timeout:30];({ways});out tags geom;")
}

fn road_width_scale(tags: &HashMap<String, String>) -> Option<f32> {
    match tags.get("highway")?.as_str() {
        "motorway" => Some(1.4),
        "trunk" => Some(1.25),
        "primary" => Some(1.0),
        "secondary" => Some(0.8),
        "tertiary" => Some(0.7),
        "unclassified" => Some(0.62),
        "motorway_link" | "trunk_link" => Some(0.75),
        "primary_link" | "secondary_link" => Some(0.65),
        "tertiary_link" => Some(0.58),
        "residential" => Some(0.56),
        "living_street" | "pedestrian" | "road" => Some(0.5),
        "service" => Some(0.45),
        "track" => Some(0.5),
        "cycleway" => Some(0.45),
        "bridleway" => Some(0.42),
        "path" | "footway" | "steps" => Some(0.38),
        _ => None,
    }
}

fn is_path_or_trail(tags: &HashMap<String, String>) -> bool {
    tags.get("highway").is_some_and(|highway| {
        matches!(
            highway.as_str(),
            "track" | "cycleway" | "path" | "footway" | "bridleway" | "steps"
        )
    })
}

fn waterway_width_scale(tags: &HashMap<String, String>) -> Option<f32> {
    match tags.get("waterway")?.as_str() {
        "river" => Some(1.2),
        "canal" => Some(0.9),
        "stream" => Some(0.65),
        _ => None,
    }
}

fn is_major_waterway(tags: &HashMap<String, String>) -> bool {
    tags.get("waterway")
        .is_some_and(|value| value == "river" || value == "canal")
}

fn is_water_area(tags: &HashMap<String, String>) -> bool {
    tags.get("natural").is_some_and(|value| value == "water")
        || tags
            .get("waterway")
            .is_some_and(|value| value == "riverbank")
}

fn is_tunnel(tags: &HashMap<String, String>) -> bool {
    tags.get("tunnel")
        .is_some_and(|value| value != "no" && value != "false")
}

fn is_bridge(tags: &HashMap<String, String>) -> bool {
    tags.get("bridge")
        .is_some_and(|value| value != "no" && value != "false")
}

fn unwrap_longitude(longitude: f64, center: f64) -> f64 {
    center + normalize_longitude(longitude - center)
}

fn sample_tile(
    tile_name: &str,
    points: &[SamplePoint],
    target_width: usize,
    target_height: usize,
    output: &mut [SurfaceClass],
    cache_dir: &Path,
) -> Result<()> {
    let path = cached_world_cover_tile(tile_name, cache_dir)?;
    let geotiff = GeoTiffFile::open(&path)
        .with_context(|| format!("open cached ESA WorldCover tile {}", path.display()))?;
    if geotiff.epsg() != Some(4326) {
        bail!(
            "ESA WorldCover tile {tile_name} uses unexpected CRS {:?}",
            geotiff.epsg()
        );
    }

    let base_pixels = points
        .iter()
        .map(|point| {
            geotiff
                .geo_to_pixel(point.longitude, point.latitude)
                .with_context(|| format!("map a coordinate into tile {tile_name}"))
        })
        .collect::<Result<Vec<_>>>()?;
    let base_col_min = base_pixels
        .iter()
        .map(|(column, _)| column.floor().max(0.0) as usize)
        .min()
        .unwrap_or(0);
    let base_col_max = base_pixels
        .iter()
        .map(|(column, _)| column.ceil().max(0.0) as usize)
        .max()
        .unwrap_or(base_col_min);
    let base_row_min = base_pixels
        .iter()
        .map(|(_, row)| row.floor().max(0.0) as usize)
        .min()
        .unwrap_or(0);
    let base_row_max = base_pixels
        .iter()
        .map(|(_, row)| row.ceil().max(0.0) as usize)
        .max()
        .unwrap_or(base_row_min);
    let base_window_width = base_col_max.saturating_sub(base_col_min) + 1;
    let base_window_height = base_row_max.saturating_sub(base_row_min) + 1;
    let overview =
        (0..geotiff.overview_count())
            .filter_map(|index| {
                let ifd = geotiff.overview_ifd(index).ok()?;
                let scale_x = ifd.width() as f64 / geotiff.width() as f64;
                let scale_y = ifd.height() as f64 / geotiff.height() as f64;
                let window_width = (base_window_width as f64 * scale_x).ceil() as usize;
                let window_height = (base_window_height as f64 * scale_y).ceil() as usize;
                (window_width <= target_width * 2 && window_height <= target_height * 2)
                    .then_some((index, ifd.width(), ifd.height()))
            })
            .max_by_key(|(_, width, height)| u64::from(*width) * u64::from(*height));
    let (raster_width, raster_height) = overview
        .map(|(_, width, height)| (width, height))
        .unwrap_or((geotiff.width(), geotiff.height()));
    let scale_x = raster_width as f64 / geotiff.width() as f64;
    let scale_y = raster_height as f64 / geotiff.height() as f64;
    let pixels = base_pixels
        .into_iter()
        .map(|(column, row)| (column * scale_x, row * scale_y))
        .collect::<Vec<_>>();
    let col_min = pixels
        .iter()
        .map(|(column, _)| column.floor().max(0.0) as usize)
        .min()
        .unwrap_or(0)
        .min(raster_width.saturating_sub(1) as usize);
    let col_max = pixels
        .iter()
        .map(|(column, _)| column.ceil().max(0.0) as usize)
        .max()
        .unwrap_or(col_min)
        .min(raster_width.saturating_sub(1) as usize);
    let row_min = pixels
        .iter()
        .map(|(_, row)| row.floor().max(0.0) as usize)
        .min()
        .unwrap_or(0)
        .min(raster_height.saturating_sub(1) as usize);
    let row_max = pixels
        .iter()
        .map(|(_, row)| row.ceil().max(0.0) as usize)
        .max()
        .unwrap_or(row_min)
        .min(raster_height.saturating_sub(1) as usize);
    let rows = row_max - row_min + 1;
    let columns = col_max - col_min + 1;
    let window = match overview {
        Some((index, _, _)) => {
            geotiff.read_overview_band_window::<u8>(index, 0, row_min, col_min, rows, columns)
        }
        None => geotiff.read_band_window::<u8>(0, row_min, col_min, rows, columns),
    }
    .with_context(|| format!("read ESA WorldCover tile {tile_name}"))?;

    for (point, (column, row)) in points.iter().zip(pixels) {
        let column = (column.round() as isize).clamp(col_min as isize, col_max as isize) as usize;
        let row = (row.round() as isize).clamp(row_min as isize, row_max as isize) as usize;
        let value = window[[row - row_min, column - col_min]];
        if value == 0 {
            bail!(
                "ESA WorldCover has no data at {}, {}",
                point.latitude,
                point.longitude
            );
        }
        output[point.output_index] = classify_world_cover(value);
    }
    Ok(())
}

fn cached_world_cover_tile(tile_name: &str, cache_dir: &Path) -> Result<PathBuf> {
    let file_name = format!("ESA_WorldCover_10m_2021_v200_{tile_name}_Map.tif");
    let path = cache_dir.join(&file_name);
    if path.is_file() {
        return Ok(path);
    }
    let url = format!("{WORLD_COVER_BASE_URL}/{file_name}");
    let response = http::blocking_client(Duration::from_secs(300))
        .context("build ESA WorldCover client")?
        .get(&url)
        .send()
        .with_context(|| format!("download ESA WorldCover tile {tile_name}"))?
        .error_for_status()
        .with_context(|| format!("ESA WorldCover rejected tile {tile_name}"))?;
    cache::store_reader(&path, response)
        .with_context(|| format!("cache ESA WorldCover tile {}", path.display()))?;
    Ok(path)
}

fn classify_world_cover(value: u8) -> SurfaceClass {
    match value {
        10 => SurfaceClass::Forest,
        70 => SurfaceClass::Snow,
        80 => SurfaceClass::Water,
        _ => SurfaceClass::Rock,
    }
}

fn world_cover_tile(longitude: f64, latitude: f64) -> String {
    let south = (latitude / 3.0).floor() as i32 * 3;
    let west = (longitude / 3.0).floor() as i32 * 3;
    format!(
        "{}{:02}{}{:03}",
        if south < 0 { 'S' } else { 'N' },
        south.unsigned_abs(),
        if west < 0 { 'W' } else { 'E' },
        west.unsigned_abs(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolves_world_cover_tile_names() {
        assert_eq!(world_cover_tile(-121.7603, 46.8523), "N45W123");
        assert_eq!(world_cover_tile(138.7274, 35.3606), "N33E138");
        assert_eq!(world_cover_tile(-1.0, -1.0), "S03W003");
    }

    #[test]
    fn maps_world_cover_classes_to_print_colors() {
        assert_eq!(classify_world_cover(10), SurfaceClass::Forest);
        assert_eq!(classify_world_cover(70), SurfaceClass::Snow);
        assert_eq!(classify_world_cover(80), SurfaceClass::Water);
        assert_eq!(classify_world_cover(60), SurfaceClass::Rock);
        assert_eq!(classify_world_cover(30), SurfaceClass::Rock);
    }

    #[test]
    fn builds_major_road_query_with_geometry() {
        let query = overpass_query(
            GeoBounds {
                south: 47.0,
                north: 48.0,
                west: -123.0,
                east: -122.0,
            },
            MAJOR_HIGHWAYS,
        );
        assert!(query.contains("motorway"));
        assert!(query.contains("secondary_link"));
        assert!(query.contains("[\"area\"!=\"yes\"]"));
        assert!(query.contains("(47.0000000,-123.0000000,48.0000000,-122.0000000)"));
        assert!(query.ends_with("out tags geom;"));
    }

    #[test]
    fn assigns_wider_lines_to_higher_road_classes() {
        let tags = |class: &str| HashMap::from([("highway".into(), class.into())]);
        assert!(road_width_scale(&tags("motorway")) > road_width_scale(&tags("primary")));
        assert!(road_width_scale(&tags("primary")) > road_width_scale(&tags("secondary")));
        assert!(road_width_scale(&tags("secondary")) > road_width_scale(&tags("residential")));
        assert!(road_width_scale(&tags("residential")) > road_width_scale(&tags("footway")));
    }

    #[test]
    fn thins_dense_road_networks_but_not_sparse_routes() {
        let spec = GenerationSpec {
            width_mm: 100.0,
            rows: 2,
            columns: 2,
            ..GenerationSpec::default()
        };
        let route = || RouteFeature {
            points: vec![[0.0, 0.5], [1.0, 0.5]],
            width_scale: 1.0,
            path_or_trail: false,
            bridge_elevations_m: None,
        };
        assert_eq!(route_density_scale(&spec, &[route()]), 1.0);
        let dense = (0..24).map(|_| route()).collect::<Vec<_>>();
        assert!(route_density_scale(&spec, &dense) < 0.5);
    }

    #[test]
    fn builds_full_route_query_and_classifies_paths() {
        let query = overpass_query(
            GeoBounds {
                south: 46.8,
                north: 46.9,
                west: -121.9,
                east: -121.7,
            },
            ALL_ROUTE_HIGHWAYS,
        );
        assert!(query.contains("residential"));
        assert!(query.contains("path|footway|bridleway|steps"));
        let tags = |class: &str| HashMap::from([("highway".into(), class.into())]);
        assert!(road_width_scale(&tags("track")) > road_width_scale(&tags("path")));
        assert!(is_path_or_trail(&tags("path")));
        assert!(!is_path_or_trail(&tags("residential")));
    }

    #[test]
    fn road_detail_controls_query_scope_and_cache_key() {
        assert_eq!(road_highway_filter(RoadDetail::Major), MAJOR_HIGHWAYS);
        assert!(!road_highway_filter(RoadDetail::Minor).contains("residential"));
        assert!(road_highway_filter(RoadDetail::Streets).contains("residential"));
        assert!(road_highway_filter(RoadDetail::All).contains("footway"));
        assert_ne!(
            road_cache_prefix(RoadDetail::Major),
            road_cache_prefix(RoadDetail::Streets)
        );
    }

    #[test]
    fn identifies_tagged_bridges_without_using_layer_as_height() {
        let tags = |key: &str, value: &str| HashMap::from([(key.into(), value.into())]);
        assert!(is_bridge(&tags("bridge", "yes")));
        assert!(is_bridge(&tags("bridge", "viaduct")));
        assert!(!is_bridge(&tags("bridge", "no")));
        assert!(!is_bridge(&tags("layer", "1")));
    }

    #[test]
    fn carries_bridge_tags_into_route_painting() {
        let spec = GenerationSpec {
            width_mm: 60.0,
            rows: 2,
            columns: 2,
            ..GenerationSpec::default()
        };
        let bounds = bounds_for(&spec);
        let height_field =
            HeightField::new(2, 2, vec![100.0, 100.0, 100.0, 100.0], "bridge").unwrap();
        let mut surface = SurfaceField::new(3, 3, vec![SurfaceClass::Rock; 9], "bridge").unwrap();
        let response = OverpassResponse {
            elements: vec![OverpassWay {
                tags: HashMap::from([
                    ("highway".into(), "primary".into()),
                    ("bridge".into(), "yes".into()),
                ]),
                geometry: vec![
                    OverpassPoint {
                        lat: bounds.south,
                        lon: bounds.west,
                    },
                    OverpassPoint {
                        lat: bounds.north,
                        lon: bounds.east,
                    },
                ],
            }],
            remark: None,
        };
        assert_eq!(
            paint_osm_ways(&spec, &height_field, bounds, &mut surface, response,),
            (1, 0, 1)
        );
    }

    #[test]
    fn builds_water_queries_and_widths() {
        let bounds = GeoBounds {
            south: 46.8,
            north: 46.9,
            west: -121.9,
            east: -121.7,
        };
        let query = water_query(bounds);
        assert!(query.contains("river|stream|canal"));
        assert!(query.contains("[\"area\"!=\"yes\"]"));
        assert!(query.contains("[\"natural\"=\"water\"]"));
        assert!(query.contains("[\"waterway\"=\"riverbank\"]"));

        let tags = |class: &str| HashMap::from([("waterway".into(), class.into())]);
        assert!(waterway_width_scale(&tags("river")) > waterway_width_scale(&tags("stream")));
        assert_eq!(waterway_width_scale(&tags("drain")), None);
        assert!(is_major_waterway(&tags("river")));
        assert!(is_major_waterway(&tags("canal")));
        assert!(!is_major_waterway(&tags("stream")));
        assert!(is_water_area(&HashMap::from([(
            "natural".into(),
            "water".into()
        )])));
    }

    #[test]
    fn waterway_cutoff_keeps_major_lines_and_limits_stream_coverage() {
        let features = || {
            let mut features = vec![WaterwayFeature {
                points: vec![[0.0, 0.0], [1.0, 0.0]],
                width_scale: 1.2,
                major: true,
            }];
            features.extend((0..10).map(|index| WaterwayFeature {
                points: vec![[0.0, index as f32 * 0.01], [1.0, index as f32 * 0.01]],
                width_scale: 0.65,
                major: false,
            }));
            features
        };
        let mut spec = GenerationSpec {
            width_mm: 100.0,
            ..GenerationSpec::default()
        };
        spec.color_output.waterway_coverage_percent = 0.0;
        assert_eq!(select_waterway_features(&spec, features()).len(), 1);
        spec.color_output.waterway_coverage_percent = 3.0;
        assert_eq!(select_waterway_features(&spec, features()).len(), 4);
        spec.color_output.waterway_coverage_percent = 100.0;
        assert_eq!(select_waterway_features(&spec, features()).len(), 11);
    }

    #[test]
    fn osm_cache_keys_ignore_render_settings() {
        let first = GenerationSpec::default();
        let mut second = first.clone();
        second.color_output.road_width_mm = 0.4;
        second.color_output.adaptive_road_widths = false;
        second.color_output.osm_water_enabled = false;
        second.color_output.waterway_coverage_percent = 3.0;
        assert_eq!(
            osm_cache_path(&first, Path::new("/cache"), "roads"),
            osm_cache_path(&second, Path::new("/cache"), "roads")
        );
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
        let error = parse_osm_response(partial, "buildings").unwrap_err();
        assert!(error.to_string().contains("incomplete buildings data"));
        assert!(error.to_string().contains("Query timed out"));
        assert!(parse_osm_response(br#"{"elements":[]}"#, "buildings").is_ok());
    }

    #[test]
    fn builds_building_query_and_reads_height_tags() {
        let query = building_query(GeoBounds {
            south: 46.8,
            north: 46.9,
            west: -121.9,
            east: -121.7,
        });
        assert!(query.contains("[\"building\"]"));
        assert!(query.contains("[\"building\"!=\"no\"]"));
        assert!(query.contains("out tags geom"));
        assert_eq!(
            building_height_m(&HashMap::from([("height".into(), "12.5 m".into())])),
            12.5
        );
        assert_eq!(
            building_height_m(&HashMap::from([("building:levels".into(), "4".into())])),
            12.0
        );
        assert_eq!(building_height_m(&HashMap::new()), 8.0);
    }

    #[test]
    fn unwraps_longitudes_around_the_date_line() {
        assert!((unwrap_longitude(-179.9, 179.9) - 180.1).abs() < 0.001);
        assert!((unwrap_longitude(179.9, -179.9) + 180.1).abs() < 0.001);
    }
}
