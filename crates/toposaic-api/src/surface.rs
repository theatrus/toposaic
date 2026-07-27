use std::{
    collections::{HashMap, HashSet},
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
use toposaic_core::{
    ClassBorders, GenerationSpec, HeightField, LineStyle, MarkerKind, NativeClassGrid,
    RailLifecycle, ResolvedRoadDetail, SlopeGates, SurfaceClass, SurfaceField,
};
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
const WORLD_COVER_RESOLUTION_M: f32 = 10.0;
const WORLD_COVER_TILE_DEGREES: f64 = 3.0;
const WORLD_COVER_TILE_PIXELS: i64 = 36_000;
const WORLD_COVER_PIXELS_PER_DEGREE: f64 =
    WORLD_COVER_TILE_PIXELS as f64 / WORLD_COVER_TILE_DEGREES;
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
/// In-service `railway=*` values worth drawing. Every lifecycle value —
/// `abandoned`, `disused`, `razed`, `dismantled`, `demolished`, `removed`,
/// `proposed`, `construction` — is absent by construction, so the whitelist
/// itself is the first lifecycle filter.
const RAILWAYS: &str =
    "rail|light_rail|subway|tram|narrow_gauge|funicular|monorail|miniature|preserved";
/// In-service `aerialway=*` values: cable cars, gondolas, and every tow.
/// Station and pylon values are left out; they are point furniture, not line
/// features.
const AERIALWAYS: &str =
    "cable_car|gondola|chair_lift|mixed_lift|drag_lift|t-bar|j-bar|platter|rope_tow|magic_carpet";
/// Bare lifecycle keys that can mark an otherwise in-service `railway=*` or
/// `aerialway=*` way as out of use, in the order the Overpass negation list
/// writes them. A way tagged `railway=rail` plus `disused=yes` is a rusting
/// siding, not a railway; whether it prints is up to
/// [`RailLifecycle`], which decides which of these keys the query negates
/// and which it lets through.
///
/// `dismantled` is not in the list, and deliberately so: the namespaced form
/// `dismantled:railway=*` never reaches the parser because no query asks for
/// that key, and adding the bare form would change what the default setting
/// draws. It belongs with `razed` and is a small known gap in the bare-key
/// coverage, not a state this setting can reach.
const RAIL_LIFECYCLE_KEYS: [&str; 7] = [
    "disused",
    "abandoned",
    "razed",
    "demolished",
    "removed",
    "proposed",
    "construction",
];
/// Bare lifecycle keys that mean nothing is left on the ground, or nothing is
/// there yet. No [`RailLifecycle`] setting accepts them; see that type for
/// the argument.
const GONE_LIFECYCLE_KEYS: [&str; 5] =
    ["razed", "demolished", "removed", "proposed", "construction"];
/// Key PREFIXES for the same states, as in `razed:railway=rail` or
/// `construction:aerialway=gondola`. Such a way carries no plain
/// `railway`/`aerialway` key at all, so no query asks for it; the check stays
/// as a guard for ways carrying both encodings.
const GONE_LIFECYCLE_PREFIXES: [&str; 6] = [
    "razed:",
    "demolished:",
    "removed:",
    "proposed:",
    "construction:",
    "historic:",
];
/// Narrowest line any overlay prints. Below roughly one nozzle width a line
/// stops being reliably extruded, so every width scale bottoms out here.
const MINIMUM_LINE_WIDTH_MM: f32 = 0.4;
/// Working width for a railway whose OSM way has no explicit `width=*`.
/// Standard gauge is 1.435 m; a representative 3.15 m loading envelope adds
/// room for the vehicle around it. This is a print-width estimate, not a
/// claim about the full formation, ballast, or right of way.
const DEFAULT_RAILWAY_WIDTH_M: f32 = 1.435 + 3.15;
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
    /// OpenStreetMap way id. Overpass returns each query's ways in ascending
    /// id order, so this is what lets two separate fetches be merged back
    /// into the order one combined query would have produced.
    #[serde(default)]
    id: u64,
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
    detail: ResolvedRoadDetail,
    highway_filter: &'static str,
    fallback: bool,
}

#[derive(Debug, Default, Clone, Copy)]
struct RailCounts {
    lines: usize,
    bridges: usize,
    lifecycle_skipped: usize,
    tunnel_skipped: usize,
}

/// One of the two rail-family layers. They share a painting pass, a
/// lifecycle setting, and a tag grammar, and differ only in which key they
/// read, which values they accept, and where they cache.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RailKind {
    Railway,
    Aerialway,
}

impl RailKind {
    /// Both layers, in the order they fetch and paint.
    const ALL: [Self; 2] = [Self::Railway, Self::Aerialway];

    fn index(self) -> usize {
        match self {
            Self::Railway => 0,
            Self::Aerialway => 1,
        }
    }

    /// The plain tag key, then the lifecycle-namespaced spellings a
    /// non-default [`RailLifecycle`] can ask for.
    fn tag_keys(self) -> [&'static str; 3] {
        match self {
            Self::Railway => ["railway", "disused:railway", "abandoned:railway"],
            Self::Aerialway => ["aerialway", "disused:aerialway", "abandoned:aerialway"],
        }
    }

    /// The accepted values of that key.
    fn values(self) -> &'static str {
        match self {
            Self::Railway => RAILWAYS,
            Self::Aerialway => AERIALWAYS,
        }
    }

    /// Cache-prefix stem. The railway stem is at v2 because the v1 query
    /// fetched aerialways in the same response; a v1 entry would answer a
    /// railway-only request with lift lines mixed in.
    fn cache_stem(self) -> &'static str {
        match self {
            Self::Railway => "rail-v2",
            Self::Aerialway => "aerial-v1",
        }
    }

    fn note_name(self) -> &'static str {
        match self {
            Self::Railway => "railways",
            Self::Aerialway => "aerialways",
        }
    }

    fn width_scale(self, value: &str) -> Option<f32> {
        match self {
            Self::Railway => railway_width_scale(value),
            Self::Aerialway => aerialway_width_scale(value),
        }
    }
}

/// What OpenStreetMap says is left of a rail-family way, ordered by how much
/// of it a visitor would still find on the ground. Comparing against the
/// spec's [`RailLifecycle`] is then a single test.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum WayLifecycle {
    InService,
    Disused,
    Abandoned,
    Gone,
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
    mapped_width_m: Option<f32>,
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
        // A tile that fails (missing over open ocean, outside coverage, or a
        // download error) degrades to the default Rock class instead of
        // failing the whole generation, matching the other overlays.
        let mut missing_tiles = Vec::new();
        for tile_name in &tile_names {
            let points = tiles
                .remove(tile_name)
                .context("land-cover tile group disappeared")?;
            if let Err(error) = sample_tile(
                tile_name,
                &points,
                width,
                height,
                &mut classes,
                &map_cache_dir.join("world-cover"),
            ) {
                warn!(%error, tile = %tile_name, "ESA WorldCover tile unavailable; using rock");
                missing_tiles.push(tile_name.clone());
            }
        }
        source = format!(
            "ESA WorldCover 2021 v200, 10 m, EPSG:4326, tiles {}; CC BY 4.0; source: {WORLD_COVER_INFO_URL}; {WORLD_COVER_ATTRIBUTION}",
            tile_names.join(", ")
        );
        if !missing_tiles.is_empty() {
            append_source(
                &mut source,
                format!(
                    "WorldCover unavailable for tiles {}; defaulted to rock",
                    missing_tiles.join(", ")
                ),
            );
        }
    }

    let mut field = SurfaceField::new(width, height, classes, source)?;
    if spec.color_output.enabled {
        let ground_span_m = (spec.ground_span_km * 1_000.0) as f32;
        let gates = &spec.color_output.slope_gates;
        if gates.forest_slope_gate || gates.snow_slope_gate {
            // One call gates both classes so the slope per sample is
            // computed once, whichever gates are on.
            let demoted = field.demote_steep_classes(
                height_field,
                ground_span_m,
                SlopeGates {
                    forest_limit_degrees: gates
                        .forest_slope_gate
                        .then_some(gates.forest_slope_limit_degrees),
                    steep_forest_target: gates.steep_forest_target,
                    snow_limit_degrees: gates
                        .snow_slope_gate
                        .then_some(gates.snow_slope_limit_degrees),
                },
            );
            if demoted.total() > 0 {
                let mut parts = Vec::new();
                if demoted.forest_to_rock > 0 {
                    parts.push(format!(
                        "{} forest samples steeper than {:.0} degrees reclassified as rock",
                        demoted.forest_to_rock, gates.forest_slope_limit_degrees
                    ));
                }
                if demoted.forest_to_snow > 0 {
                    parts.push(format!(
                        "{} forest samples steeper than {:.0} degrees reclassified as snow above the snowline",
                        demoted.forest_to_snow, gates.forest_slope_limit_degrees
                    ));
                }
                if demoted.snow_to_rock > 0 {
                    parts.push(format!(
                        "{} snow samples steeper than {:.0} degrees reclassified as rock",
                        demoted.snow_to_rock, gates.snow_slope_limit_degrees
                    ));
                }
                append_source(
                    &mut field.source,
                    format!("steep-slope gates: {}", parts.join("; ")),
                );
            }
        }
        field.filter_small_patches(spec.width_mm, spec.color_output.minimum_patch_mm);
        if spec.color_output.borders.class_borders == ClassBorders::Smooth {
            // Smoothing is the default, so the scale gate decides whether it
            // runs: only a raster that samples each 10 m cell often enough
            // has borders to bend. Wide views fall short, and there the
            // native window is not worth reading either.
            if field.class_border_smoothing_applies(WORLD_COVER_RESOLUTION_M, ground_span_m) {
                let smoothed_native = match fetch_native_class_grid(
                    bounds,
                    width,
                    height,
                    &map_cache_dir.join("world-cover"),
                ) {
                    Ok(native) => {
                        field.smooth_class_borders_with_native(
                            &native,
                            spec.color_output.borders.border_smoothing_range_cells,
                            spec.color_output.borders.border_smoothing_nugget,
                        );
                        true
                    }
                    Err(error) => {
                        warn!(
                            %error,
                            "native land-cover window unavailable; smoothing the recovered grid"
                        );
                        field.smooth_class_borders(
                            WORLD_COVER_RESOLUTION_M,
                            ground_span_m,
                            spec.color_output.borders.border_smoothing_range_cells,
                            spec.color_output.borders.border_smoothing_nugget,
                        );
                        false
                    }
                };
                append_source(
                    &mut field.source,
                    format!(
                        "class borders smoothed by indicator kriging of the 10 m land-cover grid ({} lattice, range {:.1} cells, nugget {:.2})",
                        if smoothed_native {
                            "native"
                        } else {
                            "recovered"
                        },
                        spec.color_output.borders.border_smoothing_range_cells,
                        spec.color_output.borders.border_smoothing_nugget
                    ),
                );
            } else {
                // Say so rather than stay silent: the same setting produces
                // smoothed borders at close views and untouched ones here,
                // and the difference should be readable in the sources.
                append_source(
                    &mut field.source,
                    "class borders kept at source resolution; smoothing needs finer sampling than 10 m cells",
                );
            }
        }
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
                        counts.detail.name(),
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
    if spec.uses_rail_or_aerial() {
        let (drawn, failures) = paint_rail_family(
            spec,
            height_field,
            bounds,
            &map_cache_dir.join("osm"),
            &mut field,
        );
        for kind in RailKind::ALL {
            let Some(counts) = drawn[kind.index()] else {
                continue;
            };
            let style = match kind {
                RailKind::Railway => spec.rail_line_style(),
                RailKind::Aerialway => spec.aerial_line_style(),
            };
            append_source(
                &mut field.source,
                format!(
                    "{}: {} lines ({} tagged bridges) from OpenStreetMap via Overpass API; \
                     drawn in the {} color; lifecycle={}; skipped {} out-of-service and \
                     {} tunnelled ways; {}={}; \
                     © OpenStreetMap contributors, ODbL; {OPENSTREETMAP_COPYRIGHT_URL}",
                    kind.note_name(),
                    counts.lines,
                    counts.bridges,
                    line_style_color_name(style),
                    spec.color_output.rail_lifecycle.name(),
                    counts.lifecycle_skipped,
                    counts.tunnel_skipped,
                    kind.tag_keys()[0],
                    kind.values(),
                ),
            );
        }
        for failure in failures {
            append_source(&mut field.source, failure);
        }
    }
    if !spec.trails.is_empty() {
        let painted = paint_imported_trails(spec, bounds, &mut field);
        append_source(
            &mut field.source,
            format!(
                "imported trails: {painted} of {} drawn on the model in the trail color",
                spec.trails.len()
            ),
        );
    }
    let marker_dots = paint_marker_dots(spec, &mut field);
    if marker_dots > 0 {
        append_source(
            &mut field.source,
            format!("map markers: {marker_dots} colored dots drawn on the terrain"),
        );
    }
    if spec.buildings.enabled || spec.uses_building_markers() {
        match paint_buildings(spec, bounds, &map_cache_dir.join("osm"), &mut field) {
            Ok(count) => append_source(
                &mut field.source,
                format!(
                    "buildings: {count} OpenStreetMap footprints via Overpass API; © OpenStreetMap contributors, ODbL; {OPENSTREETMAP_COPYRIGHT_URL}"
                ),
            ),
            Err(error) => {
                if spec.uses_building_markers() {
                    return Err(error).context("map building markers require OpenStreetMap data");
                }
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

/// Which configured color a resolved line style actually paints in, for the
/// data-source note.
fn line_style_color_name(style: toposaic_core::LineStyle) -> &'static str {
    match style.class {
        SurfaceClass::Rail => "rail",
        SurfaceClass::Aerial => "aerialway",
        _ => "road",
    }
}

fn append_source(source: &mut String, addition: impl AsRef<str>) {
    if !source.is_empty() {
        source.push_str("; ");
    }
    source.push_str(addition.as_ref());
}

/// Maps one latitude/longitude pair into the model's normalized UV square,
/// unwrapping the longitude around the date line first. Every overlay —
/// OpenStreetMap ways and imported trails alike — must share this mapping so
/// their features land on the same spot of the model.
fn normalized_map_point(
    latitude: f64,
    longitude: f64,
    spec: &GenerationSpec,
    bounds: GeoBounds,
) -> [f32; 2] {
    let longitude = unwrap_longitude(longitude, spec.center_lon);
    [
        ((longitude - bounds.west) / (bounds.east - bounds.west)) as f32,
        ((latitude - bounds.south) / (bounds.north - bounds.south)) as f32,
    ]
}

fn normalized_osm_points(
    way: &OverpassWay,
    spec: &GenerationSpec,
    bounds: GeoBounds,
) -> Vec<[f32; 2]> {
    way.geometry
        .iter()
        .map(|point| normalized_map_point(point.lat, point.lon, spec, bounds))
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
    if road_count + trail_count > 0 || detail == ResolvedRoadDetail::All {
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
            mapped_width_m: osm_width_m(&way.tags),
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
        let line_width = road_line_width_mm(spec, feature, density_scale);
        let class = if feature.path_or_trail {
            SurfaceClass::RouteTrail
        } else {
            SurfaceClass::Road
        };
        if let Some(elevations_m) = feature.bridge_elevations_m {
            field.paint_bridge_polyline_as(
                &feature.points,
                spec.width_mm,
                line_width,
                elevations_m,
                class,
            );
        } else {
            field.paint_polyline(&feature.points, spec.width_mm, line_width, class);
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

/// Fetches and draws the railway and aerialway layers.
///
/// Each layer has its OWN Overpass query and its own cache entry, so
/// switching one on or off never re-downloads the other, and a ski map that
/// wants lifts without trains never pays to download a city's rail network.
/// The price is a second request when both layers are on — serialized behind
/// the global request lock, with its own retry ladder. That is the trade
/// taken deliberately: rail networks and lift networks differ in size by
/// orders of magnitude, so a combined fetch would make the common
/// one-layer-only case pay for the other layer every time, and any shared
/// download would have to be re-fetched whenever either layer's settings
/// moved. Roads already spend up to two requests through their path
/// fallback, so a second one here is within the established budget.
///
/// Both layers then paint in ONE pass over the union of their ways, ordered
/// by OpenStreetMap way id — the order a single combined query returned
/// before the split. So when both layers are on and resolve to the same
/// class and width, which is exactly what the defaults do, the drawn result
/// is the pre-split result.
///
/// Rail networks are sparse next to street grids, so these lines skip the
/// adaptive road-width thinning: a thinned single-track line reads as noise.
fn paint_rail_family(
    spec: &GenerationSpec,
    height_field: &HeightField,
    bounds: GeoBounds,
    cache_dir: &Path,
    field: &mut SurfaceField,
) -> ([Option<RailCounts>; 2], Vec<String>) {
    let mut ways = Vec::new();
    let mut drawn = [None, None];
    let mut failures = Vec::new();
    for kind in RailKind::ALL {
        let enabled = match kind {
            RailKind::Railway => spec.uses_rail(),
            RailKind::Aerialway => spec.uses_aerial(),
        };
        if !enabled {
            continue;
        }
        match fetch_rail_ways(spec, bounds, cache_dir, kind) {
            Ok(fetched) => {
                ways.extend(fetched.into_iter().map(|way| (kind, way)));
                drawn[kind.index()] = Some(RailCounts::default());
            }
            Err(error) => {
                let name = kind.note_name();
                warn!(%error, layer = name, "OpenStreetMap rail layer unavailable; omitting it");
                failures.push(format!(
                    "OpenStreetMap {name} unavailable; {name} overlay omitted"
                ));
            }
        }
    }
    // A stable sort over two already-ascending lists merges them back into
    // one ascending list, which is the order a combined query returned.
    ways.sort_by_key(|(_, way)| way.id);
    let counts = paint_rail_ways(spec, height_field, bounds, field, ways);
    for (index, layer) in drawn.iter_mut().enumerate() {
        if let Some(layer) = layer {
            *layer = counts[index];
        }
    }
    (drawn, failures)
}

/// Fetches one layer's ways. The lifecycle setting is part of the cache key,
/// not just the query: a download made with out-of-service lines filtered out
/// must never answer a request that asked for them.
fn fetch_rail_ways(
    spec: &GenerationSpec,
    bounds: GeoBounds,
    cache_dir: &Path,
    kind: RailKind,
) -> Result<Vec<OverpassWay>> {
    let lifecycle = spec.color_output.rail_lifecycle;
    let cache_prefix = rail_cache_prefix(kind, lifecycle);
    let response = fetch_osm_response(
        spec,
        cache_dir,
        &cache_prefix,
        rail_query(bounds, kind, lifecycle),
    )?;
    Ok(response.elements)
}

fn rail_cache_prefix(kind: RailKind, lifecycle: RailLifecycle) -> String {
    format!("{}-{}", kind.cache_stem(), lifecycle.name())
}

/// Draws one merged batch of fetched rail-family ways, counting per layer
/// what each drew and skipped.
///
/// A way paints into whatever class and width its layer's style resolved to,
/// so under the defaults — aerial following rail, rail following roads —
/// both layers land in the Road class at the road width, which is what keeps
/// any extra filament slot out of the archive.
fn paint_rail_ways(
    spec: &GenerationSpec,
    height_field: &HeightField,
    bounds: GeoBounds,
    field: &mut SurfaceField,
    ways: Vec<(RailKind, OverpassWay)>,
) -> [RailCounts; 2] {
    let styles = [spec.rail_line_style(), spec.aerial_line_style()];
    let lifecycle = spec.color_output.rail_lifecycle;
    let mut counts = [RailCounts::default(), RailCounts::default()];
    for (kind, way) in ways {
        let counts = &mut counts[kind.index()];
        if way.geometry.len() < 2 {
            continue;
        }
        let state = way_lifecycle(&way.tags);
        if !lifecycle_accepts(lifecycle, state) {
            counts.lifecycle_skipped += 1;
            continue;
        }
        if is_tunnel(&way.tags) {
            counts.tunnel_skipped += 1;
            continue;
        }
        let Some(scale) =
            rail_family_value(&way.tags, kind).and_then(|value| kind.width_scale(value))
        else {
            continue;
        };
        let style = styles[kind.index()];
        let points = normalized_osm_points(&way, spec, bounds);
        let line_width = rail_family_line_width_mm(spec, &way.tags, kind, style, scale, state);
        if is_bridge(&way.tags) {
            let first = points[0];
            let last = points[points.len() - 1];
            field.paint_bridge_polyline_as(
                &points,
                spec.width_mm,
                line_width,
                [
                    height_field.elevation_m_at(first[0], first[1]),
                    height_field.elevation_m_at(last[0], last[1]),
                ],
                style.class,
            );
            counts.bridges += 1;
        } else {
            field.paint_polyline(&points, spec.width_mm, line_width, style.class);
        }
        counts.lines += 1;
    }
    counts
}

/// The `railway`/`aerialway` value of a way, wherever its lifecycle
/// namespace put it. An `abandoned:railway=rail` way carries no plain
/// `railway` key, so the type has to be read from the namespaced spelling.
fn rail_family_value(tags: &HashMap<String, String>, kind: RailKind) -> Option<&str> {
    kind.tag_keys()
        .into_iter()
        .find_map(|key| tags.get(key))
        .map(String::as_str)
}

/// Relative print widths per railway type, against the configured width.
/// The ordering is the ground truth of the structures: a mainline is a
/// double-track formation tens of metres wide and a tram shares a street.
/// Values are deliberately compressed — even the narrowest must stay
/// printable, so the range is 1.0 down to 0.35 rather than the true ratio.
fn railway_width_scale(value: &str) -> Option<f32> {
    match value {
        // Mainline formations, the widest thing on the map after a motorway.
        "rail" => Some(1.0),
        // Preserved lines are mainline formations kept in service.
        "preserved" => Some(0.8),
        "light_rail" => Some(0.7),
        // Mostly tunnelled, so this rarely applies; where a metro runs on
        // the surface it is a light-rail-sized formation.
        "subway" => Some(0.65),
        "narrow_gauge" => Some(0.6),
        "monorail" => Some(0.55),
        "tram" => Some(0.5),
        // A mountain funicular is single track on a narrow bench.
        "funicular" => Some(0.5),
        "miniature" => Some(0.35),
        _ => None,
    }
}

/// Relative print widths per aerialway type. Every one of these is a cable,
/// so the whole family sits below the railway range.
fn aerialway_width_scale(value: &str) -> Option<f32> {
    match value {
        // Cabin lifts: the largest aerial structures, but still cables.
        "cable_car" => Some(0.5),
        "gondola" => Some(0.45),
        "mixed_lift" => Some(0.45),
        "chair_lift" => Some(0.4),
        // Surface tows are a cable and a track in the snow.
        "drag_lift" | "t-bar" | "j-bar" | "platter" => Some(0.35),
        "rope_tow" | "magic_carpet" => Some(0.3),
        _ => None,
    }
}

/// How much narrower an out-of-service line prints than a running one.
///
/// Printed width is a legibility signal, not a measurement. A line still in
/// service is what a reader navigates by; an out-of-service one is landscape
/// texture, and thinning it keeps the map's hierarchy readable while still
/// showing the feature. Disused track keeps its rails and ballast and a
/// disused lift its cable and pylons, so it loses little. An abandoned line
/// has had its rails lifted: what is left is a scar in the ground, and it
/// prints like one. Widths still bottom out at the printability floor, so
/// the thinning can never produce a line too fine to come out of a nozzle.
fn lifecycle_width_scale(state: WayLifecycle) -> f32 {
    match state {
        WayLifecycle::InService => 1.0,
        WayLifecycle::Disused => 0.85,
        WayLifecycle::Abandoned => 0.6,
        // Never drawn; the filter rejects it first.
        WayLifecycle::Gone => 0.0,
    }
}

/// What state a way OpenStreetMap tags as a railway or aerialway is in.
///
/// Two encodings matter. A bare lifecycle key — `disused=yes`,
/// `abandoned=yes`, `razed=yes` — sits alongside the in-service tag. A
/// lifecycle-namespaced key — `disused:railway=rail` — replaces it. Both are
/// read here, and the more-gone state wins when a way carries several, so a
/// way tagged both `disused=yes` and `razed=yes` counts as gone. Explicit
/// negatives (`disused=no`) are not lifecycle states.
fn way_lifecycle(tags: &HashMap<String, String>) -> WayLifecycle {
    let bare = |key: &str| {
        tags.get(key)
            .is_some_and(|value| value != "no" && value != "false" && !value.is_empty())
    };
    let namespaced = |prefix: &str| tags.keys().any(|key| key.starts_with(prefix));
    if GONE_LIFECYCLE_KEYS.iter().copied().any(bare)
        || GONE_LIFECYCLE_PREFIXES.iter().copied().any(namespaced)
    {
        return WayLifecycle::Gone;
    }
    if bare("abandoned") || namespaced("abandoned:") {
        return WayLifecycle::Abandoned;
    }
    if bare("disused") || namespaced("disused:") {
        return WayLifecycle::Disused;
    }
    WayLifecycle::InService
}

/// Whether a lifecycle setting draws a way in this state. The settings are
/// cumulative, and nothing that has left the ground is ever drawn.
fn lifecycle_accepts(filter: RailLifecycle, state: WayLifecycle) -> bool {
    match state {
        WayLifecycle::InService => true,
        WayLifecycle::Disused => filter >= RailLifecycle::Disused,
        WayLifecycle::Abandoned => filter >= RailLifecycle::Abandoned,
        WayLifecycle::Gone => false,
    }
}

/// The lifecycle namespaces a setting asks Overpass for, beyond the plain
/// key. A `disused:railway=rail` way has no plain `railway` key, so it only
/// ever arrives if the query names its key.
fn accepted_namespaces(lifecycle: RailLifecycle) -> &'static [&'static str] {
    match lifecycle {
        RailLifecycle::Operational => &[],
        RailLifecycle::Disused => &["disused"],
        RailLifecycle::Abandoned => &["disused", "abandoned"],
    }
}

fn rail_query(bounds: GeoBounds, kind: RailKind, lifecycle: RailLifecycle) -> String {
    let namespaces = accepted_namespaces(lifecycle);
    // Drop the bare lifecycle keys this setting rejects server-side too, so
    // an out-of-service freight yard never reaches the parser. Filtering the
    // one ordered constant keeps the negation order fixed, so the default
    // setting still writes the exact negation list it always has.
    let negations = RAIL_LIFECYCLE_KEYS
        .iter()
        .filter(|key| !namespaces.contains(key))
        .map(|key| format!("[\"{key}\"!~\".\"]"))
        .collect::<String>();
    let values = kind.values();
    let ways = bounds
        .split_at_antimeridian()
        .iter()
        .map(|bounds| {
            let box_filter = format!(
                "({south:.7},{west:.7},{north:.7},{east:.7})",
                south = bounds.south,
                west = bounds.west,
                north = bounds.north,
                east = bounds.east,
            );
            kind.tag_keys()
                .into_iter()
                .take(1 + namespaces.len())
                .map(|tag| {
                    format!(
                        "way[\"{tag}\"~\"^({values})$\"]{negations}[\"area\"!=\"yes\"]{box_filter};"
                    )
                })
                .collect::<String>()
        })
        .collect::<String>();
    format!("[out:json][timeout:30];({ways});out tags geom;")
}

fn road_highway_filter(detail: ResolvedRoadDetail) -> &'static str {
    match detail {
        ResolvedRoadDetail::Major => MAJOR_HIGHWAYS,
        ResolvedRoadDetail::Minor => MINOR_HIGHWAYS,
        ResolvedRoadDetail::Streets => STREET_HIGHWAYS,
        ResolvedRoadDetail::All => ALL_ROUTE_HIGHWAYS,
    }
}

fn road_cache_prefix(detail: ResolvedRoadDetail) -> &'static str {
    match detail {
        ResolvedRoadDetail::Major => "roads-v2-major",
        ResolvedRoadDetail::Minor => "roads-v2-minor",
        ResolvedRoadDetail::Streets => "roads-v2-streets",
        ResolvedRoadDetail::All => "roads-v2-all",
    }
}

// The per-segment length sums here and in `waterway_printed_area` look
// alike but stay separate on purpose: each applies its own width factor and
// serves a different rule (waterways budget printed area against a coverage
// percentage; routes derive one global thinning scale).
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
                * provisional_road_width_mm(spec, feature)
        })
        .sum::<f32>();
    let model_area = spec.width_mm * spec.height_mm();
    let estimated_coverage = printed_length / model_area.max(f32::EPSILON);
    (0.06 / estimated_coverage.max(0.06)).clamp(0.35, 1.0)
}

fn road_line_width_mm(spec: &GenerationSpec, feature: &RouteFeature, density_scale: f32) -> f32 {
    let provisional_width = provisional_road_width_mm(spec, feature);
    if feature.mapped_width_m.is_some() {
        // A mapped physical width already expresses the road's scale. It
        // contributes to the density budget, but thinning it would make the
        // printed result cease to be the mapped width.
        provisional_width
    } else {
        (provisional_width * density_scale).max(MINIMUM_LINE_WIDTH_MM)
    }
}

fn provisional_road_width_mm(spec: &GenerationSpec, feature: &RouteFeature) -> f32 {
    let class_width = spec.color_output.road_width_mm * feature.width_scale;
    let minimum_width = class_width.max(MINIMUM_LINE_WIDTH_MM);
    feature
        .mapped_width_m
        .map(|width_m| mapped_line_width_mm(spec, minimum_width, width_m))
        .unwrap_or_else(|| {
            (class_width * road_close_view_scale(spec, feature.width_scale))
                .max(MINIMUM_LINE_WIDTH_MM)
        })
}

fn rail_family_line_width_mm(
    spec: &GenerationSpec,
    tags: &HashMap<String, String>,
    kind: RailKind,
    style: LineStyle,
    width_scale: f32,
    state: WayLifecycle,
) -> f32 {
    if kind == RailKind::Aerialway {
        return (style.width_mm
            * width_scale
            * lifecycle_width_scale(state)
            * spec.close_view_line_scale())
        .max(MINIMUM_LINE_WIDTH_MM);
    }

    let minimum_width =
        (style.width_mm * width_scale * lifecycle_width_scale(state)).max(MINIMUM_LINE_WIDTH_MM);
    let physical_width_m = osm_width_m(tags).unwrap_or(DEFAULT_RAILWAY_WIDTH_M);
    mapped_line_width_mm(spec, minimum_width, physical_width_m)
}

/// Converts an OSM ground width into print millimetres without changing the
/// non-zoom class minimum. Between that floor and the user's safety cap, the
/// line stays at its real scale on the model.
fn mapped_line_width_mm(
    spec: &GenerationSpec,
    minimum_width_mm: f32,
    physical_width_m: f32,
) -> f32 {
    let mapped_width_mm = physical_width_m * spec.width_mm / (spec.ground_span_km as f32 * 1_000.0);
    mapped_width_mm.clamp(
        minimum_width_mm,
        spec.color_output
            .line_scaling
            .maximum_mapped_width_mm
            .max(minimum_width_mm),
    )
}

/// Reads the common OSM width forms. Bare values and `m` use metres; feet,
/// inches, centimetres, and kilometres carry an explicit suffix. OSM also
/// uses `est_width=*` when a mapper estimated rather than measured a width.
fn osm_width_m(tags: &HashMap<String, String>) -> Option<f32> {
    tags.get("width")
        .and_then(|value| parse_osm_length_m(value))
        .or_else(|| {
            tags.get("est_width")
                .and_then(|value| parse_osm_length_m(value))
        })
}

fn parse_osm_length_m(value: &str) -> Option<f32> {
    let mut value = value.trim().to_ascii_lowercase();
    for prefix in ["~", "approx.", "approx", "ca.", "ca"] {
        if let Some(rest) = value.strip_prefix(prefix) {
            value = rest.trim().to_owned();
            break;
        }
    }
    if value.contains(';') {
        return None;
    }

    if let Some(feet_end) = value.find('\'') {
        let feet = value[..feet_end].trim().parse::<f32>().ok()?;
        let inches = value[feet_end + 1..]
            .trim()
            .trim_end_matches(['"', 'i', 'n'])
            .trim()
            .parse::<f32>()
            .unwrap_or(0.0);
        return positive_finite_metres(feet * 0.3048 + inches * 0.0254);
    }

    let units = [
        ("kilometres", 1_000.0),
        ("kilometers", 1_000.0),
        ("km", 1_000.0),
        ("centimetres", 0.01),
        ("centimeters", 0.01),
        ("cm", 0.01),
        ("metres", 1.0),
        ("meters", 1.0),
        ("meter", 1.0),
        ("metre", 1.0),
        ("m", 1.0),
        ("feet", 0.3048),
        ("foot", 0.3048),
        ("ft", 0.3048),
        ("inches", 0.0254),
        ("inch", 0.0254),
        ("in", 0.0254),
    ];
    for (suffix, scale) in units {
        if let Some(number) = value.strip_suffix(suffix) {
            return positive_finite_metres(number.trim().parse::<f32>().ok()? * scale);
        }
    }
    positive_finite_metres(value.parse::<f32>().ok()?)
}

fn positive_finite_metres(value: f32) -> Option<f32> {
    (value.is_finite() && value > 0.0).then_some(value)
}

/// Major roads get the full close-view boost. Smaller road classes get a
/// smaller share, so local streets gain detail without filling an urban map.
fn road_close_view_scale(spec: &GenerationSpec, road_class_scale: f32) -> f32 {
    1.0 + (spec.close_view_line_scale() - 1.0) * road_class_scale.clamp(0.0, 1.0)
}

/// How far outside the model square a trail keeps painting, per axis, in
/// normalized map units. Each margin only has to cover half the trail
/// line's width on that axis, so clipped ends never show a gap at the model
/// border. The axes differ on non-square models: the same millimetres are a
/// larger normalized margin along the shorter side, so a fixed constant
/// sized for the width under-covers the v axis of a short, wide model.
fn trail_clip_margins(spec: &GenerationSpec) -> [f32; 2] {
    let half_line_mm = spec.color_output.trail_width_mm * 0.5;
    [
        half_line_mm / spec.width_mm.max(f32::EPSILON),
        half_line_mm / spec.height_mm().max(f32::EPSILON),
    ]
}

/// Paints the spec's imported trails as Trail-class vector polylines, using
/// the same lat/lon-to-UV normalization as OpenStreetMap routes. Each trail
/// is clipped to the (margin-expanded) model square first — GPX files often
/// wander far beyond the mapped area, and painting the whole track would
/// resample kilometres of invisible line. Returns how many trails put at
/// least one segment on the model.
fn paint_imported_trails(
    spec: &GenerationSpec,
    bounds: GeoBounds,
    field: &mut SurfaceField,
) -> usize {
    let mut painted = 0;
    let margins = trail_clip_margins(spec);
    for trail in &spec.trails {
        let normalized = trail
            .points
            .iter()
            .map(|point| normalized_map_point(point[0], point[1], spec, bounds))
            .collect::<Vec<_>>();
        let mut on_model = false;
        for chain in clip_polyline_to_unit_box(&normalized, margins) {
            if chain.len() >= 2 {
                field.paint_polyline(
                    &chain,
                    spec.width_mm,
                    spec.color_output.trail_width_mm,
                    SurfaceClass::Trail,
                );
                on_model = true;
            }
        }
        if on_model {
            painted += 1;
        }
    }
    painted
}

/// Clips a polyline to the unit square expanded by the per-axis `margins`,
/// splitting it into the chains that cross the box. Segments are clipped by
/// Liang-Barsky; consecutive surviving segments whose endpoints meet stay
/// in one chain, and every exit from the box starts a new one.
fn clip_polyline_to_unit_box(points: &[[f32; 2]], margins: [f32; 2]) -> Vec<Vec<[f32; 2]>> {
    let low = [-margins[0], -margins[1]];
    let high = [1.0 + margins[0], 1.0 + margins[1]];
    let mut chains = Vec::new();
    let mut current: Vec<[f32; 2]> = Vec::new();
    let mut flush = |chain: &mut Vec<[f32; 2]>| {
        if chain.len() >= 2 {
            chains.push(std::mem::take(chain));
        } else {
            chain.clear();
        }
    };
    for pair in points.windows(2) {
        let Some((start, end)) = clip_segment_to_box(pair[0], pair[1], low, high) else {
            flush(&mut current);
            continue;
        };
        let continues = current.last().is_some_and(|last| {
            (last[0] - start[0]).abs() <= 1e-6 && (last[1] - start[1]).abs() <= 1e-6
        });
        if !continues {
            flush(&mut current);
            current.push(start);
        }
        if current.last() != Some(&end) {
            current.push(end);
        }
    }
    flush(&mut current);
    chains
}

/// Liang-Barsky clip of one segment against an axis-aligned box; `None`
/// when the segment misses the box entirely.
fn clip_segment_to_box(
    start: [f32; 2],
    end: [f32; 2],
    low: [f32; 2],
    high: [f32; 2],
) -> Option<([f32; 2], [f32; 2])> {
    let delta = [end[0] - start[0], end[1] - start[1]];
    let mut enter = 0.0_f32;
    let mut exit = 1.0_f32;
    for (direction, distance) in [
        (-delta[0], start[0] - low[0]),
        (delta[0], high[0] - start[0]),
        (-delta[1], start[1] - low[1]),
        (delta[1], high[1] - start[1]),
    ] {
        if direction == 0.0 {
            if distance < 0.0 {
                return None;
            }
            continue;
        }
        let ratio = distance / direction;
        if direction < 0.0 {
            if ratio > exit {
                return None;
            }
            enter = enter.max(ratio);
        } else {
            if ratio < enter {
                return None;
            }
            exit = exit.min(ratio);
        }
    }
    if enter > exit {
        return None;
    }
    Some((
        [start[0] + delta[0] * enter, start[1] + delta[1] * enter],
        [start[0] + delta[0] * exit, start[1] + delta[1] * exit],
    ))
}

fn paint_buildings(
    spec: &GenerationSpec,
    bounds: GeoBounds,
    cache_dir: &Path,
    field: &mut SurfaceField,
) -> Result<usize> {
    let response = fetch_osm_response(spec, cache_dir, "buildings", building_query(bounds))?;
    let building_markers = spec
        .markers
        .iter()
        .enumerate()
        .filter(|(_, marker)| marker.kind == MarkerKind::Building)
        .map(|(index, marker)| {
            (
                index,
                marker,
                spec.normalized_map_point(marker.latitude, marker.longitude),
            )
        })
        .filter(|(_, _, point)| (0.0..=1.0).contains(&point[0]) && (0.0..=1.0).contains(&point[1]))
        .collect::<Vec<_>>();
    let mut matched_markers = HashSet::new();
    let mut painted = 0;
    for building in response.elements {
        if building.geometry.len() < 3 {
            continue;
        }
        let points = normalized_osm_points(&building, spec, bounds);
        let mut highlighted = false;
        for (index, _, point) in &building_markers {
            if point_in_polygon(*point, &points) {
                matched_markers.insert(*index);
                highlighted = true;
            }
        }
        field.paint_building_with_class(
            &points,
            building_height_m(&building.tags),
            if highlighted {
                SurfaceClass::Marker
            } else {
                SurfaceClass::Building
            },
        );
        painted += 1;
    }
    if let Some((_, marker, _)) = building_markers
        .iter()
        .find(|(index, _, _)| !matched_markers.contains(index))
    {
        bail!(
            "building marker '{}' does not fall inside an OpenStreetMap building footprint",
            marker.name
        );
    }
    Ok(painted)
}

fn paint_marker_dots(spec: &GenerationSpec, field: &mut SurfaceField) -> usize {
    let radius_mm = spec.marker_settings.dot_diameter_mm * 0.5;
    let mut painted = 0;
    for marker in spec
        .markers
        .iter()
        .filter(|marker| marker.kind == MarkerKind::Dot)
    {
        let center = spec.normalized_map_point(marker.latitude, marker.longitude);
        if !(0.0..=1.0).contains(&center[0]) || !(0.0..=1.0).contains(&center[1]) {
            continue;
        }
        let points = (0..32)
            .map(|index| {
                let angle = index as f32 / 32.0 * std::f32::consts::TAU;
                [
                    center[0] + angle.cos() * radius_mm / spec.width_mm,
                    center[1] + angle.sin() * radius_mm / spec.height_mm(),
                ]
            })
            .collect::<Vec<_>>();
        field.paint_surface_area(&points, SurfaceClass::Marker);
        painted += 1;
    }
    painted
}

fn point_in_polygon(point: [f32; 2], polygon: &[[f32; 2]]) -> bool {
    let mut inside = false;
    for (start, end) in polygon.iter().zip(polygon.iter().cycle().skip(1)) {
        if (start[1] > point[1]) != (end[1] > point[1])
            && point[0]
                < (end[0] - start[0]) * (point[1] - start[1]) / (end[1] - start[1]) + start[0]
        {
            inside = !inside;
        }
    }
    inside
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
    // A panic while holding the lock poisons it, but the lock only guards
    // request pacing; recovering the guard costs nothing, while treating the
    // poison as fatal would disable OSM overlays for the process's lifetime.
    let _request_guard = OVERPASS_REQUEST_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
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
                    "removing incomplete OpenStreetMap cache entry"
                );
                // cache::store never overwrites, so the bad entry must go or
                // the fresh download can never replace it.
                if let Err(remove_error) = fs::remove_file(cache_path)
                    && remove_error.kind() != std::io::ErrorKind::NotFound
                {
                    warn!(
                        error = %remove_error,
                        path = %cache_path.display(),
                        "could not remove incomplete OpenStreetMap cache entry"
                    );
                }
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

/// Continuous WorldCover grid column of a longitude, on the global lattice
/// implied by the tiling: 3 degree tiles of 36000 by 36000 pixels anchored
/// at integer multiples of 3 degrees. Integer values sit on pixel centres.
/// Longitudes may be unwrapped past the antimeridian; the lattice continues
/// linearly and the per-tile reads normalize.
fn world_cover_grid_column(longitude: f64) -> f64 {
    (longitude + 180.0) * WORLD_COVER_PIXELS_PER_DEGREE - 0.5
}

/// Continuous WorldCover grid row of a latitude; rows grow southward like
/// the tiles themselves, with integer values on pixel centres.
fn world_cover_grid_row(latitude: f64) -> f64 {
    (90.0 - latitude) * WORLD_COVER_PIXELS_PER_DEGREE - 0.5
}

/// One contiguous window read from one WorldCover tile: where it starts in
/// the tile's own pixel coordinates and on the global lattice.
#[derive(Debug, Clone, PartialEq, Eq)]
struct NativeTileRead {
    tile_name: String,
    tile_row: usize,
    tile_column: usize,
    rows: usize,
    columns: usize,
    global_row: i64,
    global_column: i64,
}

/// The native-resolution window that covers the model bounds plus the
/// kriging neighbourhood, the affine map from sample pixels to window grid
/// coordinates, and the per-tile reads that fill it. Splitting the plan
/// from the reads keeps the stitching arithmetic testable without tiles.
#[derive(Debug)]
struct NativeWindowPlan {
    column_start: i64,
    row_start: i64,
    width: usize,
    height: usize,
    x_scale: f64,
    x_offset: f64,
    y_scale: f64,
    y_offset: f64,
    reads: Vec<NativeTileRead>,
}

/// Plans the native WorldCover window for a sample raster of
/// `sample_width` by `sample_height` covering `bounds`. The window pads one
/// pixel beyond the outermost kriging cells so the 4 by 4 neighbourhood
/// never clamps for interior samples, and the mapping puts window row 0 at
/// the southmost row to match the sample raster's row order.
fn plan_native_window(
    bounds: GeoBounds,
    sample_width: usize,
    sample_height: usize,
) -> NativeWindowPlan {
    let column_start = world_cover_grid_column(bounds.west).floor() as i64 - 1;
    let column_end = world_cover_grid_column(bounds.east).floor() as i64 + 2;
    let row_start = world_cover_grid_row(bounds.north).floor() as i64 - 1;
    let row_end = world_cover_grid_row(bounds.south).floor() as i64 + 2;
    let mut reads = Vec::new();
    for tile_row in
        row_start.div_euclid(WORLD_COVER_TILE_PIXELS)..=row_end.div_euclid(WORLD_COVER_TILE_PIXELS)
    {
        let clipped_rows = row_start.max(tile_row * WORLD_COVER_TILE_PIXELS)
            ..=row_end.min((tile_row + 1) * WORLD_COVER_TILE_PIXELS - 1);
        for tile_column in column_start.div_euclid(WORLD_COVER_TILE_PIXELS)
            ..=column_end.div_euclid(WORLD_COVER_TILE_PIXELS)
        {
            let clipped_columns = column_start.max(tile_column * WORLD_COVER_TILE_PIXELS)
                ..=column_end.min((tile_column + 1) * WORLD_COVER_TILE_PIXELS - 1);
            let tile_center_longitude =
                normalize_longitude((tile_column as f64 + 0.5) * WORLD_COVER_TILE_DEGREES - 180.0);
            let tile_center_latitude = 90.0 - (tile_row as f64 + 0.5) * WORLD_COVER_TILE_DEGREES;
            reads.push(NativeTileRead {
                tile_name: world_cover_tile(tile_center_longitude, tile_center_latitude),
                tile_row: (clipped_rows.start() - tile_row * WORLD_COVER_TILE_PIXELS) as usize,
                tile_column: (clipped_columns.start() - tile_column * WORLD_COVER_TILE_PIXELS)
                    as usize,
                rows: (clipped_rows.end() - clipped_rows.start() + 1) as usize,
                columns: (clipped_columns.end() - clipped_columns.start() + 1) as usize,
                global_row: *clipped_rows.start(),
                global_column: *clipped_columns.start(),
            });
        }
    }
    NativeWindowPlan {
        column_start,
        row_start,
        width: (column_end - column_start + 1) as usize,
        height: (row_end - row_start + 1) as usize,
        // Sample x maps to longitude west..east, and grid coordinates are
        // window-relative pixel centres; the y map also flips rows so the
        // window matches the raster's south-first row order.
        x_scale: (bounds.east - bounds.west) * WORLD_COVER_PIXELS_PER_DEGREE
            / (sample_width - 1) as f64,
        x_offset: world_cover_grid_column(bounds.west) - column_start as f64,
        y_scale: (bounds.north - bounds.south) * WORLD_COVER_PIXELS_PER_DEGREE
            / (sample_height - 1) as f64,
        y_offset: row_end as f64 - world_cover_grid_row(bounds.south),
        reads,
    }
}

/// Reads the native-resolution WorldCover classes for the planned window,
/// stitching across tile borders, and returns them with the sample-to-grid
/// mapping for `SurfaceField::smooth_class_borders_with_native`. Any error
/// (a missing tile, an unexpected layout) is returned so the caller can
/// fall back to the recovered-grid smoothing path.
fn fetch_native_class_grid(
    bounds: GeoBounds,
    sample_width: usize,
    sample_height: usize,
    cache_dir: &Path,
) -> Result<NativeClassGrid> {
    let plan = plan_native_window(bounds, sample_width, sample_height);
    let mut classes = vec![SurfaceClass::Rock; plan.width * plan.height];
    for read in &plan.reads {
        let geotiff = open_world_cover_tile(&read.tile_name, cache_dir)?;
        if i64::from(geotiff.width()) != WORLD_COVER_TILE_PIXELS
            || i64::from(geotiff.height()) != WORLD_COVER_TILE_PIXELS
        {
            bail!(
                "ESA WorldCover tile {} is {}x{}, not the expected {WORLD_COVER_TILE_PIXELS} square",
                read.tile_name,
                geotiff.width(),
                geotiff.height()
            );
        }
        // Cross-check the analytic lattice against the tile's own
        // geotransform before trusting the phase it implies.
        let (expected_column, expected_row) = geotiff
            .geo_to_pixel(
                normalize_longitude(
                    -180.0 + (read.global_column as f64 + 0.5) / WORLD_COVER_PIXELS_PER_DEGREE,
                ),
                90.0 - (read.global_row as f64 + 0.5) / WORLD_COVER_PIXELS_PER_DEGREE,
            )
            .with_context(|| format!("map the window into tile {}", read.tile_name))?;
        if (expected_column - (read.tile_column as f64 + 0.5)).abs() > 0.05
            || (expected_row - (read.tile_row as f64 + 0.5)).abs() > 0.05
        {
            bail!(
                "ESA WorldCover tile {} is not on the expected 1/{WORLD_COVER_PIXELS_PER_DEGREE} degree lattice",
                read.tile_name
            );
        }
        let window = geotiff
            .read_band_window::<u8>(0, read.tile_row, read.tile_column, read.rows, read.columns)
            .with_context(|| format!("read ESA WorldCover tile {}", read.tile_name))?;
        for row in 0..read.rows {
            let global_row = read.global_row + row as i64;
            let flipped_row = (plan.row_start + plan.height as i64 - 1 - global_row) as usize;
            for column in 0..read.columns {
                // The reader can hand back a shorter window than requested
                // at a clipped image edge, so trust its shape, not ours.
                let Some(&value) = window.get([row, column]) else {
                    continue;
                };
                // Nodata keeps the default rock, like the sample path.
                if value == 0 {
                    continue;
                }
                let window_column =
                    (read.global_column + column as i64 - plan.column_start) as usize;
                classes[flipped_row * plan.width + window_column] = classify_world_cover(value);
            }
        }
    }
    NativeClassGrid::new(
        plan.width,
        plan.height,
        classes,
        plan.x_scale,
        plan.x_offset,
        plan.y_scale,
        plan.y_offset,
    )
}

/// Opens a cached WorldCover tile, refetching once when the cached copy is
/// unreadable: a corrupt cached tile must not fail its area forever.
fn open_world_cover_tile(tile_name: &str, cache_dir: &Path) -> Result<GeoTiffFile> {
    let path = cached_world_cover_tile(tile_name, cache_dir)?;
    let geotiff = match GeoTiffFile::open(&path) {
        Ok(geotiff) => geotiff,
        Err(error) => {
            warn!(
                %error,
                tile = %path.display(),
                "cached ESA WorldCover tile is unreadable; refetching"
            );
            fs::remove_file(&path)
                .with_context(|| format!("remove corrupt WorldCover tile {}", path.display()))?;
            let path = cached_world_cover_tile(tile_name, cache_dir)?;
            GeoTiffFile::open(&path)
                .with_context(|| format!("open cached ESA WorldCover tile {}", path.display()))?
        }
    };
    if geotiff.epsg() != Some(4326) {
        bail!(
            "ESA WorldCover tile {tile_name} uses unexpected CRS {:?}",
            geotiff.epsg()
        );
    }
    Ok(geotiff)
}

/// Picks the raster level for a sampling read: the largest overview whose
/// scaled read window stays within twice the target sample grid, or — when
/// even the smallest overview reads more than that — the smallest overview,
/// since any overview is a cheaper read than the full-resolution base image
/// (a whole WorldCover tile is 36000 by 36000 pixels). `None`, meaning the
/// base image, only when the file carries no readable overview at all.
/// Entries are `(overview index, width, height)`.
fn select_sampling_overview(
    overviews: &[(usize, u32, u32)],
    base: (u32, u32),
    base_window: (usize, usize),
    target: (usize, usize),
) -> Option<(usize, u32, u32)> {
    let pixels = |width: u32, height: u32| u64::from(width) * u64::from(height);
    overviews
        .iter()
        .filter(|(_, width, height)| {
            let scale_x = f64::from(*width) / f64::from(base.0);
            let scale_y = f64::from(*height) / f64::from(base.1);
            let window_width = (base_window.0 as f64 * scale_x).ceil() as usize;
            let window_height = (base_window.1 as f64 * scale_y).ceil() as usize;
            window_width <= target.0 * 2 && window_height <= target.1 * 2
        })
        .max_by_key(|(_, width, height)| pixels(*width, *height))
        .or_else(|| {
            overviews
                .iter()
                .min_by_key(|(_, width, height)| pixels(*width, *height))
        })
        .copied()
}

fn sample_tile(
    tile_name: &str,
    points: &[SamplePoint],
    target_width: usize,
    target_height: usize,
    output: &mut [SurfaceClass],
    cache_dir: &Path,
) -> Result<()> {
    let geotiff = open_world_cover_tile(tile_name, cache_dir)?;

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
    let overviews = (0..geotiff.overview_count())
        .filter_map(|index| {
            let ifd = geotiff.overview_ifd(index).ok()?;
            Some((index, ifd.width(), ifd.height()))
        })
        .collect::<Vec<_>>();
    let overview = select_sampling_overview(
        &overviews,
        (geotiff.width(), geotiff.height()),
        (base_window_width, base_window_height),
        (target_width, target_height),
    );
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
        // A clipped edge can return fewer rows or columns than requested;
        // a missing pixel means the same as nodata below.
        let Some(&value) = window.get([row - row_min, column - col_min]) else {
            continue;
        };
        // Nodata (open ocean or a coverage gap) keeps the default Rock class
        // instead of failing the whole generation.
        if value == 0 {
            continue;
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
    use toposaic_core::MapMarker;

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
    fn dot_markers_paint_the_marker_material_at_their_map_position() {
        let mut spec = GenerationSpec::default();
        spec.markers.push(MapMarker {
            name: "Centre".into(),
            latitude: spec.center_lat,
            longitude: spec.center_lon,
            kind: MarkerKind::Dot,
            label_height_mm: 4.0,
            rotation_degrees: 0.0,
            label_style: None,
        });
        let mut field =
            SurfaceField::new(33, 33, vec![SurfaceClass::Rock; 33 * 33], "marker test").unwrap();

        assert_eq!(paint_marker_dots(&spec, &mut field), 1);
        assert_eq!(field.class_at(0.5, 0.5), SurfaceClass::Marker);
    }

    #[test]
    fn building_marker_intersection_handles_points_inside_and_outside() {
        let footprint = [[0.2, 0.2], [0.8, 0.2], [0.8, 0.8], [0.2, 0.8]];
        assert!(point_in_polygon([0.5, 0.5], &footprint));
        assert!(!point_in_polygon([0.1, 0.5], &footprint));
    }

    #[test]
    fn native_window_mapping_puts_pixel_centres_on_integer_grid_coordinates() {
        let bounds = GeoBounds {
            south: 46.8,
            north: 46.9,
            west: -121.9,
            east: -121.7,
        };
        let plan = plan_native_window(bounds, 65, 65);
        assert_eq!(plan.reads.len(), 1);
        assert_eq!(plan.reads[0].tile_name, "N45W123");
        // The first sample sits one to two pixels inside the padded window,
        // so the kriging neighbourhood never leaves it.
        assert!((1.0..2.0).contains(&plan.x_offset));
        let last_x = 64.0 * plan.x_scale + plan.x_offset;
        assert!(last_x < plan.width as f64 - 1.0);
        // A sample placed exactly on a pixel centre maps to that pixel's
        // integer grid coordinate: the phase the recovered grid loses.
        let column = plan.column_start + 5;
        let longitude = -180.0 + (column as f64 + 0.5) / WORLD_COVER_PIXELS_PER_DEGREE;
        let sample_x = (longitude - bounds.west) / (bounds.east - bounds.west) * 64.0;
        assert!((sample_x * plan.x_scale + plan.x_offset - 5.0).abs() < 1e-6);
        let row = plan.row_start + 5;
        let latitude = 90.0 - (row as f64 + 0.5) / WORLD_COVER_PIXELS_PER_DEGREE;
        let sample_y = (latitude - bounds.south) / (bounds.north - bounds.south) * 64.0;
        // Window row 0 is the southmost row, so global row start + 5 lands
        // five rows below the window top.
        let expected = plan.height as f64 - 6.0;
        assert!((sample_y * plan.y_scale + plan.y_offset - expected).abs() < 1e-6);
    }

    #[test]
    fn native_window_stitches_across_tile_borders() {
        let across_longitude = GeoBounds {
            south: 46.8,
            north: 46.81,
            west: -120.005,
            east: -119.995,
        };
        let plan = plan_native_window(across_longitude, 65, 65);
        assert_eq!(plan.reads.len(), 2);
        assert_eq!(plan.reads[0].tile_name, "N45W123");
        assert_eq!(plan.reads[1].tile_name, "N45W120");
        // The two reads butt against the shared tile border and cover the
        // window exactly.
        assert_eq!(plan.reads[0].tile_column + plan.reads[0].columns, 36_000);
        assert_eq!(plan.reads[1].tile_column, 0);
        assert_eq!(
            plan.reads[0].global_column + plan.reads[0].columns as i64,
            plan.reads[1].global_column
        );
        assert_eq!(plan.reads[0].columns + plan.reads[1].columns, plan.width);

        let corner = plan_native_window(
            GeoBounds {
                south: 44.995,
                north: 45.005,
                west: -120.005,
                east: -119.995,
            },
            65,
            65,
        );
        let mut names = corner
            .reads
            .iter()
            .map(|read| read.tile_name.as_str())
            .collect::<Vec<_>>();
        names.sort_unstable();
        assert_eq!(names, ["N42W120", "N42W123", "N45W120", "N45W123"]);
        assert_eq!(
            corner
                .reads
                .iter()
                .map(|read| read.rows * read.columns)
                .sum::<usize>(),
            corner.width * corner.height
        );
    }

    #[test]
    fn native_window_follows_unwrapped_antimeridian_bounds() {
        let plan = plan_native_window(
            GeoBounds {
                south: 0.1,
                north: 0.11,
                west: 179.99,
                east: 180.01,
            },
            65,
            65,
        );
        let names = plan
            .reads
            .iter()
            .map(|read| read.tile_name.as_str())
            .collect::<Vec<_>>();
        assert_eq!(names, ["N00E177", "N00W180"]);
        assert_eq!(
            plan.reads[0].global_column + plan.reads[0].columns as i64,
            plan.reads[1].global_column
        );
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
    fn close_views_boost_major_roads_more_than_local_lines() {
        let mut spec = GenerationSpec {
            ground_span_km: 2.0,
            ..GenerationSpec::default()
        };
        assert_eq!(road_close_view_scale(&spec, 1.4), 2.0);
        assert_eq!(road_close_view_scale(&spec, 1.0), 2.0);
        assert!(road_close_view_scale(&spec, 0.56) < 2.0);
        assert!(road_close_view_scale(&spec, 0.38) < road_close_view_scale(&spec, 0.56));

        spec.ground_span_km = 18.0;
        assert_eq!(road_close_view_scale(&spec, 1.4), 1.0);
        assert_eq!(road_close_view_scale(&spec, 0.38), 1.0);
    }

    #[test]
    fn reads_common_osm_width_units_and_estimates() {
        let width = |value: &str| HashMap::from([("width".into(), value.into())]);
        assert_eq!(osm_width_m(&width("4.5")), Some(4.5));
        assert_eq!(osm_width_m(&width("250 cm")), Some(2.5));
        assert!((osm_width_m(&width("10 ft")).unwrap() - 3.048).abs() < 0.0001);
        assert!((osm_width_m(&width("12' 6\"")).unwrap() - 3.81).abs() < 0.0001);
        assert_eq!(osm_width_m(&width("~ 6 m")), Some(6.0));
        assert_eq!(osm_width_m(&width("3;4")), None);

        let estimated = HashMap::from([("est_width".into(), "7".into())]);
        assert_eq!(osm_width_m(&estimated), Some(7.0));
        let measured_wins = HashMap::from([
            ("width".into(), "5".into()),
            ("est_width".into(), "7".into()),
        ]);
        assert_eq!(osm_width_m(&measured_wins), Some(5.0));
    }

    #[test]
    fn mapped_road_widths_stay_real_between_the_print_limits() {
        let mut spec = GenerationSpec {
            ground_span_km: 2.0,
            ..GenerationSpec::default()
        };
        let feature = |mapped_width_m| RouteFeature {
            points: vec![[0.0, 0.5], [1.0, 0.5]],
            width_scale: 1.0,
            mapped_width_m,
            path_or_trail: false,
            bridge_elevations_m: None,
        };

        // Ten ground metres across a 2 km, 180 mm model is 0.9 print mm.
        assert!((road_line_width_mm(&spec, &feature(Some(10.0)), 1.0) - 0.9).abs() < 0.0001);
        assert!((road_line_width_mm(&spec, &feature(Some(10.0)), 0.35) - 0.9).abs() < 0.0001);
        // An unknown width keeps the existing 2x close-view boost.
        assert!((road_line_width_mm(&spec, &feature(None), 1.0) - 1.4).abs() < 0.0001);
        assert!((road_line_width_mm(&spec, &feature(None), 0.35) - 0.49).abs() < 0.0001);

        spec.ground_span_km = 18.0;
        // A real width below the configured class floor never makes a road
        // narrower than the old wide-area default.
        assert!((road_line_width_mm(&spec, &feature(Some(10.0)), 1.0) - 0.7).abs() < 0.0001);

        spec.ground_span_km = 0.25;
        assert_eq!(road_line_width_mm(&spec, &feature(Some(10.0)), 1.0), 4.0);
        spec.color_output.line_scaling.maximum_mapped_width_mm = 6.0;
        assert_eq!(road_line_width_mm(&spec, &feature(Some(10.0)), 1.0), 6.0);
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
            mapped_width_m: None,
            path_or_trail: false,
            bridge_elevations_m: None,
        };
        assert_eq!(route_density_scale(&spec, &[route()]), 1.0);
        let dense = (0..24).map(|_| route()).collect::<Vec<_>>();
        assert!(route_density_scale(&spec, &dense) < 0.5);

        let close_spec = GenerationSpec {
            ground_span_km: 0.25,
            ..spec
        };
        let mapped_route = || RouteFeature {
            points: vec![[0.0, 0.5], [1.0, 0.5]],
            width_scale: 1.0,
            mapped_width_m: Some(20.0),
            path_or_trail: false,
            bridge_elevations_m: None,
        };
        assert_eq!(route_density_scale(&close_spec, &[route(), route()]), 1.0);
        assert!(route_density_scale(&close_spec, &[mapped_route(), mapped_route()]) < 1.0);
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
    fn mapped_paths_use_their_own_class_while_roads_keep_the_route_class() {
        let mut spec = rail_bounds_spec();
        spec.color_output.road_width_mm = 2.0;
        let bounds = bounds_for(&spec);
        let height_field = HeightField::new(2, 2, vec![100.0; 4], "routes").unwrap();

        let mut paths = SurfaceField::new(9, 9, vec![SurfaceClass::Rock; 81], "paths").unwrap();
        let counts = paint_osm_ways(
            &spec,
            &height_field,
            bounds,
            &mut paths,
            OverpassResponse {
                elements: vec![crossing_way(bounds, &[("highway", "path")])],
                remark: None,
            },
        );
        assert_eq!(counts, (0, 1, 0));
        assert_eq!(paths.class_at(0.5, 0.5), SurfaceClass::RouteTrail);

        let mut roads = SurfaceField::new(9, 9, vec![SurfaceClass::Rock; 81], "roads").unwrap();
        let counts = paint_osm_ways(
            &spec,
            &height_field,
            bounds,
            &mut roads,
            OverpassResponse {
                elements: vec![crossing_way(bounds, &[("highway", "residential")])],
                remark: None,
            },
        );
        assert_eq!(counts, (1, 0, 0));
        assert_eq!(roads.class_at(0.5, 0.5), SurfaceClass::Road);
    }

    #[test]
    fn road_detail_controls_query_scope_and_cache_key() {
        assert_eq!(
            road_highway_filter(ResolvedRoadDetail::Major),
            MAJOR_HIGHWAYS
        );
        assert!(!road_highway_filter(ResolvedRoadDetail::Minor).contains("residential"));
        assert!(road_highway_filter(ResolvedRoadDetail::Streets).contains("residential"));
        assert!(road_highway_filter(ResolvedRoadDetail::All).contains("footway"));
        assert_ne!(
            road_cache_prefix(ResolvedRoadDetail::Major),
            road_cache_prefix(ResolvedRoadDetail::Streets)
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
                id: 1,
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

    fn rail_bounds_spec() -> GenerationSpec {
        let mut spec = GenerationSpec {
            width_mm: 60.0,
            rows: 2,
            columns: 2,
            ..GenerationSpec::default()
        };
        spec.color_output.enabled = true;
        spec
    }

    /// One west-to-east way across the middle of the model.
    fn crossing_way(bounds: GeoBounds, tags: &[(&str, &str)]) -> OverpassWay {
        crossing_way_with_id(bounds, 0, tags)
    }

    fn crossing_way_with_id(bounds: GeoBounds, id: u64, tags: &[(&str, &str)]) -> OverpassWay {
        let latitude = (bounds.south + bounds.north) * 0.5;
        OverpassWay {
            id,
            tags: tags
                .iter()
                .map(|(key, value)| ((*key).to_owned(), (*value).to_owned()))
                .collect(),
            geometry: vec![
                OverpassPoint {
                    lat: latitude,
                    lon: bounds.west,
                },
                OverpassPoint {
                    lat: latitude,
                    lon: bounds.east,
                },
            ],
        }
    }

    /// Runs the real painting body against synthetic railway ways, no
    /// network.
    fn paint_ways(
        spec: &GenerationSpec,
        bounds: GeoBounds,
        field: &mut SurfaceField,
        ways: Vec<OverpassWay>,
    ) -> RailCounts {
        paint_layer_ways(spec, bounds, field, RailKind::Railway, ways)
    }

    fn paint_layer_ways(
        spec: &GenerationSpec,
        bounds: GeoBounds,
        field: &mut SurfaceField,
        kind: RailKind,
        ways: Vec<OverpassWay>,
    ) -> RailCounts {
        let height_field = HeightField::new(2, 2, vec![100.0; 4], "rail").unwrap();
        let ways = ways.into_iter().map(|way| (kind, way)).collect();
        paint_rail_ways(spec, &height_field, bounds, field, ways)[kind.index()]
    }

    fn test_bounds() -> GeoBounds {
        GeoBounds {
            south: 46.8,
            north: 46.9,
            west: -121.9,
            east: -121.7,
        }
    }

    #[test]
    fn each_rail_layer_queries_only_its_own_key() {
        let railway = rail_query(test_bounds(), RailKind::Railway, RailLifecycle::Operational);
        let aerial = rail_query(
            test_bounds(),
            RailKind::Aerialway,
            RailLifecycle::Operational,
        );
        // Each layer asks for its own key and its own value whitelist, and
        // for nothing of the other's — that separation is what lets one
        // layer be switched without re-downloading the other.
        assert!(railway.contains("[\"railway\"~\"^(rail|light_rail|subway|tram|narrow_gauge|funicular|monorail|miniature|preserved)$\"]"));
        assert!(!railway.contains("aerialway"));
        assert!(aerial.contains("[\"aerialway\"~\"^(cable_car|gondola|chair_lift|mixed_lift|drag_lift|t-bar|j-bar|platter|rope_tow|magic_carpet)$\"]"));
        assert!(!aerial.contains("\"railway\""));
        // Lifecycle values are absent from the whitelists, so a
        // `railway=abandoned` way can never match in the first place.
        for lifecycle in ["abandoned", "razed", "dismantled"] {
            assert!(!railway.contains(&format!("|{lifecycle}|")));
            assert!(!railway.contains(&format!("({lifecycle}|")));
        }
        for query in [&railway, &aerial] {
            // Every bare lifecycle key is negated server-side by default.
            for key in RAIL_LIFECYCLE_KEYS {
                assert!(
                    query.contains(&format!("[\"{key}\"!~\".\"]")),
                    "missing {key} negation"
                );
            }
            assert!(query.contains("[\"area\"!=\"yes\"]"));
            assert!(query.contains("(46.8000000,-121.9000000,46.9000000,-121.7000000)"));
            assert!(query.ends_with("out tags geom;"));
        }
        assert_ne!(railway, overpass_query(test_bounds(), MAJOR_HIGHWAYS));
    }

    /// The lifecycle setting has to reach the WIRE — it names extra
    /// namespaced keys and stops negating the ones it now wants — and it has
    /// to reach the CACHE KEY, or a filtered download would answer a request
    /// that asked for abandoned lines.
    #[test]
    fn lifecycle_settings_change_both_the_query_and_the_cache_key() {
        let spec = GenerationSpec::default();
        let mut paths = Vec::new();
        let mut queries = Vec::new();
        for lifecycle in [
            RailLifecycle::Operational,
            RailLifecycle::Disused,
            RailLifecycle::Abandoned,
        ] {
            for kind in RailKind::ALL {
                let prefix = rail_cache_prefix(kind, lifecycle);
                assert!(prefix.ends_with(lifecycle.name()), "{prefix}");
                paths.push(osm_cache_path(&spec, Path::new("/cache"), &prefix));
                queries.push(rail_query(test_bounds(), kind, lifecycle));
            }
        }
        let unique = paths.iter().collect::<std::collections::HashSet<_>>();
        assert_eq!(unique.len(), paths.len(), "every combination caches apart");
        let unique = queries.iter().collect::<std::collections::HashSet<_>>();
        assert_eq!(unique.len(), queries.len());

        let operational = rail_query(test_bounds(), RailKind::Railway, RailLifecycle::Operational);
        let disused = rail_query(test_bounds(), RailKind::Railway, RailLifecycle::Disused);
        let abandoned = rail_query(test_bounds(), RailKind::Railway, RailLifecycle::Abandoned);
        // The operational query asks for the plain key only, and negates
        // every lifecycle key.
        assert!(!operational.contains("disused:railway"));
        assert!(!operational.contains("abandoned:railway"));
        // Each step names one more namespaced key and stops negating it.
        assert!(disused.contains("way[\"disused:railway\"~"));
        assert!(!disused.contains("[\"disused\"!~\".\"]"));
        assert!(disused.contains("[\"abandoned\"!~\".\"]"));
        assert!(!disused.contains("abandoned:railway"));
        assert!(abandoned.contains("way[\"disused:railway\"~"));
        assert!(abandoned.contains("way[\"abandoned:railway\"~"));
        assert!(!abandoned.contains("[\"abandoned\"!~\".\"]"));
        // States with nothing left on the ground stay negated throughout.
        for query in [&operational, &disused, &abandoned] {
            for key in GONE_LIFECYCLE_KEYS {
                assert!(query.contains(&format!("[\"{key}\"!~\".\"]")), "{key}");
            }
        }
    }

    #[test]
    fn rail_cache_prefixes_are_independent_of_each_other_and_of_the_roads() {
        let spec = GenerationSpec::default();
        let lifecycle = RailLifecycle::Operational;
        let railway = osm_cache_path(
            &spec,
            Path::new("/cache"),
            &rail_cache_prefix(RailKind::Railway, lifecycle),
        );
        let aerial = osm_cache_path(
            &spec,
            Path::new("/cache"),
            &rail_cache_prefix(RailKind::Aerialway, lifecycle),
        );
        assert_ne!(railway, aerial, "one layer must not evict the other");
        for detail in [
            ResolvedRoadDetail::Major,
            ResolvedRoadDetail::Minor,
            ResolvedRoadDetail::Streets,
            ResolvedRoadDetail::All,
        ] {
            let roads = osm_cache_path(&spec, Path::new("/cache"), road_cache_prefix(detail));
            assert_ne!(railway, roads);
            assert_ne!(aerial, roads);
        }
        // The railway stem is v2 because v1 responses carried aerialways.
        assert!(railway.to_string_lossy().contains("rail-v2-operational-"));
        assert!(aerial.to_string_lossy().contains("aerial-v1-operational-"));
    }

    #[test]
    fn rail_width_scales_rank_mainline_above_tram_above_lift() {
        assert!(railway_width_scale("rail") > railway_width_scale("light_rail"));
        assert!(railway_width_scale("light_rail") > railway_width_scale("tram"));
        assert!(railway_width_scale("tram") > railway_width_scale("miniature"));
        assert!(railway_width_scale("tram") >= aerialway_width_scale("cable_car"));
        assert!(aerialway_width_scale("cable_car") > aerialway_width_scale("chair_lift"));
        assert!(aerialway_width_scale("chair_lift") > aerialway_width_scale("rope_tow"));
        // Anything outside a layer's whitelist draws nothing, and neither
        // layer reads the other's values.
        assert_eq!(railway_width_scale("platform"), None);
        assert_eq!(aerialway_width_scale("station"), None);
        assert_eq!(railway_width_scale("gondola"), None);
        assert_eq!(aerialway_width_scale("rail"), None);

        // Out-of-service lines print thinner than running ones, and a
        // lifted formation thinner than track still in place.
        assert_eq!(lifecycle_width_scale(WayLifecycle::InService), 1.0);
        assert!(
            lifecycle_width_scale(WayLifecycle::InService)
                > lifecycle_width_scale(WayLifecycle::Disused)
        );
        assert!(
            lifecycle_width_scale(WayLifecycle::Disused)
                > lifecycle_width_scale(WayLifecycle::Abandoned)
        );
        assert!(lifecycle_width_scale(WayLifecycle::Abandoned) > 0.0);
    }

    #[test]
    fn railways_use_a_physical_default_but_aerialways_keep_the_boost() {
        let mut spec = GenerationSpec {
            ground_span_km: 0.5,
            ..GenerationSpec::default()
        };
        let style = LineStyle {
            class: SurfaceClass::Rail,
            width_mm: 0.7,
        };
        let tags = HashMap::new();
        let rail = rail_family_line_width_mm(
            &spec,
            &tags,
            RailKind::Railway,
            style,
            1.0,
            WayLifecycle::InService,
        );
        let expected = DEFAULT_RAILWAY_WIDTH_M * 180.0 / 500.0;
        assert!((rail - expected).abs() < 0.0001);

        let aerial = rail_family_line_width_mm(
            &spec,
            &tags,
            RailKind::Aerialway,
            style,
            1.0,
            WayLifecycle::InService,
        );
        assert_eq!(aerial, 1.4);

        spec.ground_span_km = 0.25;
        let explicit = HashMap::from([("width".into(), "20".into())]);
        assert_eq!(
            rail_family_line_width_mm(
                &spec,
                &explicit,
                RailKind::Railway,
                style,
                1.0,
                WayLifecycle::InService,
            ),
            4.0
        );
    }

    #[test]
    fn way_lifecycle_reads_bare_and_namespaced_tags() {
        let tags = |pairs: &[(&str, &str)]| {
            pairs
                .iter()
                .map(|(key, value)| ((*key).to_owned(), (*value).to_owned()))
                .collect::<HashMap<_, _>>()
        };
        assert_eq!(
            way_lifecycle(&tags(&[("railway", "rail")])),
            WayLifecycle::InService
        );
        // The default setting still rejects every lifecycle key, bare or
        // namespaced, exactly as the pre-split filter did.
        for key in RAIL_LIFECYCLE_KEYS {
            let state = way_lifecycle(&tags(&[("railway", "rail"), (key, "yes")]));
            assert_ne!(state, WayLifecycle::InService, "{key}=yes");
            assert!(
                !lifecycle_accepts(RailLifecycle::Operational, state),
                "{key}=yes should not draw by default"
            );
            // An explicit negative is not a lifecycle state.
            assert_eq!(
                way_lifecycle(&tags(&[("railway", "rail"), (key, "no")])),
                WayLifecycle::InService,
                "{key}=no should keep"
            );
        }
        assert_eq!(
            way_lifecycle(&tags(&[("disused:railway", "rail")])),
            WayLifecycle::Disused
        );
        assert_eq!(
            way_lifecycle(&tags(&[("abandoned:aerialway", "chair_lift")])),
            WayLifecycle::Abandoned
        );
        assert_eq!(
            way_lifecycle(&tags(&[("construction:aerialway", "gondola")])),
            WayLifecycle::Gone
        );
        // The more-gone state wins when a way carries several.
        assert_eq!(
            way_lifecycle(&tags(&[("disused:railway", "rail"), ("razed", "yes")])),
            WayLifecycle::Gone
        );
        assert_eq!(
            way_lifecycle(&tags(&[("railway", "rail"), ("abandoned:railway", "rail")])),
            WayLifecycle::Abandoned
        );
        // Ordinary namespaced tags that are not lifecycle states stay.
        assert_eq!(
            way_lifecycle(&tags(&[("railway", "rail"), ("railway:track_ref", "2")])),
            WayLifecycle::InService
        );

        // Cumulative acceptance, and nothing gone is ever drawn.
        for (filter, expected) in [
            (RailLifecycle::Operational, [true, false, false, false]),
            (RailLifecycle::Disused, [true, true, false, false]),
            (RailLifecycle::Abandoned, [true, true, true, false]),
        ] {
            for (state, expected) in [
                WayLifecycle::InService,
                WayLifecycle::Disused,
                WayLifecycle::Abandoned,
                WayLifecycle::Gone,
            ]
            .into_iter()
            .zip(expected)
            {
                assert_eq!(
                    lifecycle_accepts(filter, state),
                    expected,
                    "{filter:?} vs {state:?}"
                );
            }
        }
    }

    /// Turning the lifecycle setting up draws lines the default drops, and
    /// draws them thinner.
    #[test]
    fn lifecycle_settings_admit_out_of_service_lines_at_reduced_width() {
        let mut spec = rail_bounds_spec();
        spec.color_output.rail_style = toposaic_core::RailStyle::Separate;
        spec.color_output.rail_width_mm = 4.0;
        let bounds = bounds_for(&spec);
        let ways = || {
            vec![
                crossing_way(bounds, &[("railway", "rail")]),
                crossing_way(bounds, &[("railway", "rail"), ("disused", "yes")]),
                crossing_way(bounds, &[("abandoned:railway", "rail")]),
                crossing_way(bounds, &[("razed:railway", "rail")]),
            ]
        };

        let mut field = SurfaceField::new(9, 9, vec![SurfaceClass::Rock; 81], "rail").unwrap();
        let counts = paint_ways(&spec, bounds, &mut field, ways());
        assert_eq!((counts.lines, counts.lifecycle_skipped), (1, 3));

        spec.color_output.rail_lifecycle = RailLifecycle::Disused;
        let mut field = SurfaceField::new(9, 9, vec![SurfaceClass::Rock; 81], "rail").unwrap();
        let counts = paint_ways(&spec, bounds, &mut field, ways());
        assert_eq!((counts.lines, counts.lifecycle_skipped), (2, 2));

        spec.color_output.rail_lifecycle = RailLifecycle::Abandoned;
        let mut field = SurfaceField::new(9, 9, vec![SurfaceClass::Rock; 81], "rail").unwrap();
        let counts = paint_ways(&spec, bounds, &mut field, ways());
        assert_eq!(
            (counts.lines, counts.lifecycle_skipped),
            (3, 1),
            "razed track is never drawn"
        );

        // Each state prints, and prints narrower the less is left of it.
        // Measured off the model rather than off the line record: a scar
        // that is thinner only on paper would be no use.
        let painted_half_width = |way: OverpassWay| {
            let mut field = SurfaceField::new(9, 9, vec![SurfaceClass::Rock; 81], "rail").unwrap();
            assert_eq!(paint_ways(&spec, bounds, &mut field, vec![way]).lines, 1);
            (0..500)
                .map(|step| step as f32 * 0.0005)
                .take_while(|offset| field.class_at(0.5, 0.5 + offset) == SurfaceClass::Rail)
                .last()
                .expect("the line must reach its own centre")
        };
        let in_service = painted_half_width(crossing_way(bounds, &[("railway", "rail")]));
        let disused = painted_half_width(crossing_way(
            bounds,
            &[("railway", "rail"), ("disused", "yes")],
        ));
        let abandoned = painted_half_width(crossing_way(bounds, &[("abandoned:railway", "rail")]));
        assert!(in_service > disused, "{in_service} vs {disused}");
        assert!(disused > abandoned, "{disused} vs {abandoned}");
        assert!(abandoned > 0.0);
    }

    /// Each layer paints in its own resolved style, and the aerial layer
    /// follows the rail layer by default.
    #[test]
    fn the_two_layers_paint_in_their_own_resolved_styles() {
        let mut spec = rail_bounds_spec();
        spec.color_output.rail_style = toposaic_core::RailStyle::Separate;
        spec.color_output.aerial_style = toposaic_core::AerialStyle::Separate;
        spec.color_output.rail_width_mm = 2.0;
        spec.color_output.aerial_width_mm = 2.0;
        let bounds = bounds_for(&spec);

        let mut field = SurfaceField::new(9, 9, vec![SurfaceClass::Rock; 81], "rail").unwrap();
        assert_eq!(
            paint_layer_ways(
                &spec,
                bounds,
                &mut field,
                RailKind::Aerialway,
                vec![crossing_way(bounds, &[("aerialway", "cable_car")])],
            )
            .lines,
            1
        );
        assert_eq!(field.class_at(0.5, 0.5), SurfaceClass::Aerial);

        // Set to `with_rail`, lifts land in the RAIL class instead, so the
        // two layers share one filament.
        spec.color_output.aerial_style = toposaic_core::AerialStyle::WithRail;
        let mut field = SurfaceField::new(9, 9, vec![SurfaceClass::Rock; 81], "rail").unwrap();
        paint_layer_ways(
            &spec,
            bounds,
            &mut field,
            RailKind::Aerialway,
            vec![crossing_way(bounds, &[("aerialway", "cable_car")])],
        );
        assert_eq!(field.class_at(0.5, 0.5), SurfaceClass::Rail);

        // And with the rail layer switched off entirely they fall through
        // to the road class rather than vanishing.
        spec.color_output.rail_enabled = false;
        let mut field = SurfaceField::new(9, 9, vec![SurfaceClass::Rock; 81], "rail").unwrap();
        paint_layer_ways(
            &spec,
            bounds,
            &mut field,
            RailKind::Aerialway,
            vec![crossing_way(bounds, &[("aerialway", "cable_car")])],
        );
        assert_eq!(field.class_at(0.5, 0.5), SurfaceClass::Road);
    }

    /// The two layers are merged into one painting pass by way id, so the
    /// draw order is the order one combined query gave before the split.
    #[test]
    fn both_layers_paint_in_merged_way_id_order() {
        // Merged styles, so both layers land in one class and the ORDER is
        // the only thing that could differ from the pre-split output.
        let mut spec = rail_bounds_spec();
        spec.color_output.rail_style = toposaic_core::RailStyle::WithRoads;
        spec.color_output.aerial_style = toposaic_core::AerialStyle::WithRoads;
        let bounds = bounds_for(&spec);
        let height_field = HeightField::new(2, 2, vec![100.0; 4], "rail").unwrap();
        let mut ways = vec![
            (
                RailKind::Railway,
                crossing_way_with_id(bounds, 10, &[("railway", "rail")]),
            ),
            (
                RailKind::Railway,
                crossing_way_with_id(bounds, 30, &[("railway", "tram")]),
            ),
            (
                RailKind::Aerialway,
                crossing_way_with_id(bounds, 20, &[("aerialway", "gondola")]),
            ),
        ];
        ways.sort_by_key(|(_, way)| way.id);
        assert_eq!(
            ways.iter().map(|(_, way)| way.id).collect::<Vec<_>>(),
            [10, 20, 30]
        );
        let mut field = SurfaceField::new(9, 9, vec![SurfaceClass::Rock; 81], "rail").unwrap();
        let counts = paint_rail_ways(&spec, &height_field, bounds, &mut field, ways);
        // One pass, but the counts stay per layer.
        assert_eq!(counts[RailKind::Railway.index()].lines, 2);
        assert_eq!(counts[RailKind::Aerialway.index()].lines, 1);
        // Both layers land in the road class here, which is what makes the
        // merged-style output the pre-split output.
        assert_eq!(field.class_at(0.5, 0.5), SurfaceClass::Road);
    }

    #[test]
    fn separate_rail_paints_the_rail_class_and_with_roads_paints_the_road_class() {
        let mut spec = rail_bounds_spec();
        spec.color_output.rail_style = toposaic_core::RailStyle::Separate;
        spec.color_output.rail_width_mm = 2.0;
        let bounds = bounds_for(&spec);
        let ways = vec![
            crossing_way(bounds, &[("railway", "rail")]),
            // Out of service, tunnelled, and off-whitelist ways draw nothing.
            crossing_way(bounds, &[("railway", "rail"), ("disused", "yes")]),
            crossing_way(bounds, &[("railway", "subway"), ("tunnel", "yes")]),
            crossing_way(bounds, &[("railway", "platform")]),
        ];

        let mut field = SurfaceField::new(9, 9, vec![SurfaceClass::Rock; 81], "rail").unwrap();
        let counts = paint_ways(&spec, bounds, &mut field, ways);
        assert_eq!(counts.lines, 1);
        assert_eq!(counts.lifecycle_skipped, 1);
        assert_eq!(counts.tunnel_skipped, 1);
        assert_eq!(counts.bridges, 0);
        assert_eq!(field.class_at(0.5, 0.5), SurfaceClass::Rail);
        assert_eq!(field.class_at(0.5, 0.1), SurfaceClass::Rock);

        // The merged style paints the very same geometry as a road, so no
        // Rail class — and therefore no extra filament slot — appears.
        let mut with_roads = rail_bounds_spec();
        with_roads.color_output.rail_style = toposaic_core::RailStyle::WithRoads;
        with_roads.color_output.road_width_mm = 2.0;
        assert!(!with_roads.uses_separate_rail());
        let mut field = SurfaceField::new(9, 9, vec![SurfaceClass::Rock; 81], "rail").unwrap();
        let counts = paint_ways(
            &with_roads,
            bounds,
            &mut field,
            vec![crossing_way(bounds, &[("railway", "rail")])],
        );
        assert_eq!(counts.lines, 1);
        assert_eq!(field.class_at(0.5, 0.5), SurfaceClass::Road);
    }

    #[test]
    fn rail_viaducts_take_the_same_bridge_deck_treatment_as_roads() {
        let mut spec = rail_bounds_spec();
        spec.color_output.rail_style = toposaic_core::RailStyle::Separate;
        spec.color_output.rail_width_mm = 2.0;
        let bounds = bounds_for(&spec);
        // `is_bridge` keys off the `bridge` tag alone, so a railway viaduct
        // reaches the deck path exactly as a road bridge does.
        assert!(is_bridge(&HashMap::from([
            ("railway".to_owned(), "rail".to_owned()),
            ("bridge".to_owned(), "viaduct".to_owned()),
        ])));
        let mut field = SurfaceField::new(9, 9, vec![SurfaceClass::Rock; 81], "rail").unwrap();
        let counts = paint_ways(
            &spec,
            bounds,
            &mut field,
            vec![crossing_way(
                bounds,
                &[("railway", "rail"), ("bridge", "viaduct")],
            )],
        );
        assert_eq!((counts.lines, counts.bridges), (1, 1));
        assert_eq!(field.class_at(0.5, 0.5), SurfaceClass::Rail);
    }

    #[test]
    fn imported_trails_paint_in_the_trail_class_with_clipping() {
        let mut spec = GenerationSpec {
            width_mm: 60.0,
            rows: 2,
            columns: 2,
            ..GenerationSpec::default()
        };
        let bounds = bounds_for(&spec);
        let center_latitude = (bounds.south + bounds.north) * 0.5;
        // A west-to-east trail through the middle of the model that starts
        // and ends far outside it.
        spec.trails = vec![toposaic_core::TrailRoute {
            name: "Crossing".into(),
            points: vec![
                [center_latitude, bounds.west - 2.0],
                [center_latitude, bounds.east + 2.0],
            ],
        }];
        spec.color_output.trail_width_mm = 2.0;
        let mut field = SurfaceField::new(9, 9, vec![SurfaceClass::Rock; 81], "trail").unwrap();
        assert_eq!(paint_imported_trails(&spec, bounds, &mut field), 1);
        // Vector overlays answer through sampling, not the raster.
        assert_eq!(field.class_at(0.5, 0.5), SurfaceClass::Trail);
        assert_eq!(field.class_at(0.1, 0.5), SurfaceClass::Trail);
        assert_eq!(field.class_at(0.5, 0.2), SurfaceClass::Rock);

        // A trail entirely outside the model paints nothing.
        spec.trails[0].points = vec![
            [center_latitude + 5.0, bounds.west - 2.0],
            [center_latitude + 5.0, bounds.east + 2.0],
        ];
        let mut untouched = SurfaceField::new(9, 9, vec![SurfaceClass::Rock; 81], "trail").unwrap();
        assert_eq!(paint_imported_trails(&spec, bounds, &mut untouched), 0);
    }

    #[test]
    fn polyline_box_clipping_splits_reentrant_tracks() {
        // Out - in - out - in: two chains, each clipped at the box border.
        let chains = clip_polyline_to_unit_box(
            &[
                [-1.0, 0.5],
                [0.5, 0.5],
                [0.5, 2.0],
                [0.6, 2.0],
                [0.6, 0.5],
                [2.0, 0.5],
            ],
            [0.0, 0.0],
        );
        assert_eq!(chains.len(), 2);
        assert_eq!(chains[0].first(), Some(&[0.0, 0.5]));
        assert_eq!(chains[0].last(), Some(&[0.5, 1.0]));
        assert_eq!(chains[1].first(), Some(&[0.6, 1.0]));
        assert_eq!(chains[1].last(), Some(&[1.0, 0.5]));
        // A polyline that never touches the box produces nothing.
        assert!(clip_polyline_to_unit_box(&[[-2.0, -2.0], [-1.5, -2.0]], [0.0, 0.0]).is_empty());
    }

    #[test]
    fn trail_clip_margins_cover_half_the_line_width_on_each_axis() {
        // A short, wide model: 100 mm across but only 50 mm tall. The same
        // 5 mm line needs twice the normalized margin along v.
        let mut spec = GenerationSpec {
            width_mm: 100.0,
            rows: 2,
            columns: 4,
            ..GenerationSpec::default()
        };
        spec.color_output.trail_width_mm = 5.0;
        assert_eq!(spec.height_mm(), 50.0);
        let margins = trail_clip_margins(&spec);
        assert!((margins[0] - 0.025).abs() < 1e-6);
        assert!((margins[1] - 0.05).abs() < 1e-6);

        // The clip box honours each axis's own margin: a vertical chain may
        // run past the v margin but a horizontal one is cut at the u margin.
        let close = |point: &[f32; 2], expected: [f32; 2]| {
            (point[0] - expected[0]).abs() < 1e-5 && (point[1] - expected[1]).abs() < 1e-5
        };
        let chains = clip_polyline_to_unit_box(&[[0.5, -1.0], [0.5, 2.0]], margins);
        assert_eq!(chains.len(), 1);
        assert!(close(chains[0].first().unwrap(), [0.5, -0.05]));
        assert!(close(chains[0].last().unwrap(), [0.5, 1.05]));
        let chains = clip_polyline_to_unit_box(&[[-1.0, 0.5], [2.0, 0.5]], margins);
        assert!(close(chains[0].first().unwrap(), [-0.025, 0.5]));
        assert!(close(chains[0].last().unwrap(), [1.025, 0.5]));
    }

    #[test]
    fn overview_selection_never_falls_back_to_the_full_resolution_base() {
        // WorldCover-like pyramid: base 36000^2 with 2x overviews.
        let overviews = [
            (0_usize, 18_000_u32, 18_000_u32),
            (1, 9_000, 9_000),
            (2, 4_500, 4_500),
            (3, 2_250, 2_250),
        ];
        let base = (36_000, 36_000);

        // A window that fits several overviews takes the largest fitting
        // one: 1000 base pixels scale to 250 at level 1 (within 2 * 129)
        // but to 500 at level 0.
        assert_eq!(
            select_sampling_overview(&overviews, base, (1_000, 1_000), (129, 129)),
            Some((1, 9_000, 9_000))
        );
        // A huge window (a wide ground span) fits no overview at twice the
        // target; the smallest overview still beats reading the base image.
        assert_eq!(
            select_sampling_overview(&overviews, base, (30_000, 30_000), (129, 129)),
            Some((3, 2_250, 2_250))
        );
        // Only a file with no overviews at all reads the base image.
        assert_eq!(
            select_sampling_overview(&[], base, (1_000, 1_000), (129, 129)),
            None
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
        second.color_output.line_scaling.scale_line_widths_by_span = false;
        second.color_output.line_scaling.close_view_width_multiplier = 2.8;
        second.color_output.osm_water_enabled = false;
        second.color_output.waterway_coverage_percent = 3.0;
        let prefix = road_cache_prefix(ResolvedRoadDetail::Streets);
        assert_eq!(
            osm_cache_path(&first, Path::new("/cache"), prefix),
            osm_cache_path(&second, Path::new("/cache"), prefix)
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
