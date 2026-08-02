//! Overlay shells: marker dots, road and bridge-deck ribbons, imported
//! trails, railways, and the generic footprint-to-shell machinery they share.

use std::{
    collections::HashMap,
    sync::atomic::{AtomicU64, AtomicUsize, Ordering},
    time::Instant,
};

use anyhow::Result;
use geo::orient::Direction;
use geo::{
    Area, BooleanOps, Buffer, Centroid, Coord, LineString, MultiPolygon, Orient, Polygon, Simplify,
    unary_union,
};
use rayon::prelude::*;
use spade::{ConstrainedDelaunayTriangulation, Point2, Triangulation};

use crate::heightfield::{HeightField, normalized_height};
use crate::mesh::{
    Mesh, MeshBuilder, distance_squared, point_in_polygon, quantize_export_coordinate,
    triangulate_constraints,
};
use crate::planar_mesh::polygon_from_outline as geo_polygon;
use crate::spec::{BridgeStructure, GenerationSpec, MarkerKind, SurfaceClass};
use crate::surface::{
    ROAD_VECTOR_STEP_MM, VectorSurfaceLine, surface_area_bounds, surface_line_progress,
};

use super::{
    MINIMUM_OVERLAY_AREA_MM2, OVERLAY_SEPARATION_MM, OVERLAY_TERRAIN_EMBED_MM, SurfaceField,
    bounds_overlap, elapsed_us, multi_polygon_bounds, repair_classification_pinches,
    sanitize_footprint_group, simplify_closed_ring, terrain_z_at,
};

#[derive(Debug, Clone, Default)]
pub(super) struct RoadGeometryTiming {
    pub line_count: usize,
    pub aviation_component_count: usize,
    pub ribbon_clip_count: usize,
    pub selection_us: u64,
    pub obstacle_us: u64,
    /// Sum of elapsed clipping work across Rayon workers. This can be larger
    /// than wall time when clips run in parallel.
    pub ribbon_clip_work_us: u64,
    /// The share of `ribbon_clip_work_us` spent subtracting building areas.
    pub building_cutback_work_us: u64,
    pub regular_and_bridge_us: u64,
    pub secondary_overlay_us: u64,
    pub aviation_us: u64,
    pub total_us: u64,
}

fn elapsed_ns(started: Instant) -> u64 {
    started.elapsed().as_nanos().min(u128::from(u64::MAX)) as u64
}

/// One common 0.2 mm print layer keeps marker dots distinct from the terrain
/// without turning them into pegs.
const DOT_OVERLAY_HEIGHT_MM: f32 = 0.2;
const MARKER_CIRCLE_SEGMENTS: usize = 64;

fn marker_circle(center: [f32; 2], radius: f32) -> Polygon<f64> {
    let points = (0..MARKER_CIRCLE_SEGMENTS)
        .map(|index| {
            let angle = index as f32 / MARKER_CIRCLE_SEGMENTS as f32 * std::f32::consts::TAU;
            [
                center[0] + angle.cos() * radius,
                center[1] + angle.sin() * radius,
            ]
        })
        .collect::<Vec<_>>();
    geo_polygon(&points)
}

/// Builds smooth terrain-following discs for dot markers. Dots are vector
/// shells rather than surface-grid paint, so their roundness no longer
/// depends on DEM or overlay sample spacing.
#[allow(clippy::too_many_arguments)]
pub(super) fn append_dot_geometry(
    mesh: &mut Mesh,
    spec: &GenerationSpec,
    height_field: Option<&HeightField>,
    height_range: Option<(f32, f32)>,
    piece_outline: &[[f32; 2]],
    origin_x: f32,
    origin_y: f32,
    assembled_width: f32,
    assembled_height: f32,
    building_union: Option<&MultiPolygon<f64>>,
) -> Result<()> {
    let piece_polygon = geo_polygon(piece_outline);
    let footprints = spec
        .markers
        .iter()
        .filter(|marker| marker.kind == MarkerKind::Dot)
        .map(|marker| {
            let uv = spec.normalized_map_point(marker.latitude, marker.longitude);
            let radius = marker.dot_style().diameter_mm * 0.5;
            marker_circle(
                [
                    uv[0] * assembled_width - origin_x,
                    uv[1] * assembled_height - origin_y,
                ],
                radius,
            )
            .intersection(&piece_polygon)
        })
        .collect::<Vec<_>>();
    let mut dot_area = unary_union(footprints.iter());
    if let Some(buildings) = building_union.filter(|union| !union.0.is_empty()) {
        let buffered = buildings
            .0
            .iter()
            .map(|polygon| polygon.buffer(OVERLAY_SEPARATION_MM))
            .collect::<Vec<_>>();
        dot_area = dot_area.difference(&unary_union(buffered.iter()));
    }
    let flag_areas = spec
        .markers
        .iter()
        .filter(|marker| marker.kind.is_flag())
        .map(|marker| {
            let uv = spec.normalized_map_point(marker.latitude, marker.longitude);
            marker_circle(
                [
                    uv[0] * assembled_width - origin_x,
                    uv[1] * assembled_height - origin_y,
                ],
                marker.flag_style().hole_diameter_mm * 0.5 + OVERLAY_SEPARATION_MM as f32,
            )
            .intersection(&piece_polygon)
        })
        .collect::<Vec<_>>();
    if !flag_areas.is_empty() {
        dot_area = dot_area.difference(&unary_union(flag_areas.iter()));
    }
    let dot_area = sanitize_footprint_group(dot_area, true);
    let surface_z = |point: [f32; 2]| {
        let u = ((point[0] + origin_x) / assembled_width).clamp(0.0, 1.0);
        let v = ((point[1] + origin_y) / assembled_height).clamp(0.0, 1.0);
        terrain_z_at(spec, height_field, height_range, u, v)
    };
    let shells = dot_area
        .0
        .par_iter()
        .filter(|polygon| polygon.unsigned_area() > MINIMUM_OVERLAY_AREA_MM2)
        .map(|polygon| {
            build_polygon_shell(
                polygon,
                |point| surface_z(point) - OVERLAY_TERRAIN_EMBED_MM,
                |point| surface_z(point) + DOT_OVERLAY_HEIGHT_MM,
                None,
                None,
                SurfaceClass::Marker,
                "triangulate vector marker dot",
            )
        })
        .collect::<Result<Vec<_>>>()?;
    for shell in shells {
        mesh.append_isolated(shell);
    }
    Ok(())
}

/// Builds the road, trail, and railway shells of one piece.
///
/// Ordinary roads all share one terrain-following surface, so their clipped
/// ribbons are unioned into a single footprint per piece and each connected
/// component is shelled once — abutting or overlapping ribbons can therefore
/// never leave coincident faces for a slicer weld to fuse. Bridge ribbons
/// keep a shell per line because their deck height comes from the line's own
/// elevation profile. Every road footprint also keeps
/// [`OVERLAY_SEPARATION_MM`] clear of the building union so road and
/// building shells never share welded vertices.
///
/// Imported trails and separately-styled railways follow through
/// [`append_overlay_geometry`], each yielding to the layers already placed,
/// so adding a layer never disturbs the ones before it.
#[allow(clippy::too_many_arguments)]
pub(super) fn append_road_geometry(
    mesh: &mut Mesh,
    spec: &GenerationSpec,
    surface_field: &SurfaceField,
    height_field: Option<&HeightField>,
    height_range: Option<(f32, f32)>,
    piece_outline: &[[f32; 2]],
    origin_x: f32,
    origin_y: f32,
    assembled_width: f32,
    assembled_height: f32,
    building_union: Option<&MultiPolygon<f64>>,
) -> Result<RoadGeometryTiming> {
    let total_started = Instant::now();
    let selection_started = Instant::now();
    let mut timing = RoadGeometryTiming::default();
    let piece_polygon = geo_polygon(piece_outline);
    let piece_bounds = piece_outline.iter().fold(
        [
            f32::INFINITY,
            f32::INFINITY,
            f32::NEG_INFINITY,
            f32::NEG_INFINITY,
        ],
        |bounds, point| {
            [
                bounds[0].min(point[0] + origin_x),
                bounds[1].min(point[1] + origin_y),
                bounds[2].max(point[0] + origin_x),
                bounds[3].max(point[1] + origin_y),
            ]
        },
    );
    let overlaps_piece = |line: &&VectorSurfaceLine| {
        let half_width = line.width_mm * 0.5;
        let line_bounds = line.points_mm.iter().fold(
            [
                f32::INFINITY,
                f32::INFINITY,
                f32::NEG_INFINITY,
                f32::NEG_INFINITY,
            ],
            |bounds, point| {
                [
                    bounds[0].min(point[0] - half_width),
                    bounds[1].min(point[1] - half_width),
                    bounds[2].max(point[0] + half_width),
                    bounds[3].max(point[1] + half_width),
                ]
            },
        );
        bounds_overlap(piece_bounds, line_bounds) && line.points_mm.len() >= 2
    };
    // Roads and separately-styled railways and aerialways share the bridge
    // pipeline: a viaduct is a road bridge in another color. Their
    // terrain-following ribbons stay apart, because each must yield to the
    // layers already placed the way trails do. Under the default styles
    // neither a Rail nor an Aerial line exists and this walks the exact
    // road-only path.
    let road_and_rail = surface_field
        .vector_lines
        .iter()
        .filter(|line| {
            matches!(
                line.class,
                SurfaceClass::Road
                    | SurfaceClass::Rail
                    | SurfaceClass::Aerial
                    | SurfaceClass::Ferry
                    | SurfaceClass::RouteTrail
                    | SurfaceClass::Aviation
            )
        })
        .filter(overlaps_piece)
        .collect::<Vec<_>>();
    // Imported trails are Trail-class lines; they only exist when the spec
    // carries trails, so specs without trails walk the exact road-only path.
    let trail_lines = surface_field
        .vector_lines
        .iter()
        .filter(|line| line.class == SurfaceClass::Trail)
        .filter(overlaps_piece)
        .collect::<Vec<_>>();
    // Airport outlines are areas, not lines, so a field holding nothing but
    // aprons and helipads still has geometry to place. Computed before the
    // guard for exactly that reason: an airport whose runways are switched
    // off is still an airport.
    let aviation_outlines = aviation_area_footprint(
        surface_field,
        &piece_polygon,
        origin_x,
        origin_y,
        assembled_width,
        assembled_height,
    );
    timing.line_count = road_and_rail.len() + trail_lines.len();
    timing.aviation_component_count = aviation_outlines.0.len();
    timing.selection_us = elapsed_us(selection_started);
    if road_and_rail.is_empty() && trail_lines.is_empty() && aviation_outlines.0.is_empty() {
        timing.total_us = elapsed_us(total_started);
        return Ok(timing);
    }
    // Buildings the roads must keep clear of, grown by the separation gap.
    let obstacle_started = Instant::now();
    let obstacles = building_union
        .filter(|union| !union.0.is_empty())
        .map(|union| {
            let buffered = union
                .0
                .iter()
                .map(|polygon| polygon.buffer(OVERLAY_SEPARATION_MM))
                .collect::<Vec<_>>();
            unary_union(buffered.iter())
        });
    let marker_areas = spec
        .markers
        .iter()
        .filter_map(|marker| {
            let radius = f64::from(match marker.kind {
                MarkerKind::Dot => marker.dot_style().diameter_mm * 0.5,
                MarkerKind::FlagHole | MarkerKind::FlagLabel => {
                    marker.flag_style().hole_diameter_mm * 0.5
                }
                MarkerKind::Building | MarkerKind::SurfaceLabel | MarkerKind::PlaqueLabel => {
                    return None;
                }
            }) + OVERLAY_SEPARATION_MM;
            let point = spec.normalized_map_point(marker.latitude, marker.longitude);
            let clipped = marker_circle(
                [
                    point[0] * assembled_width - origin_x,
                    point[1] * assembled_height - origin_y,
                ],
                radius as f32,
            )
            .intersection(&piece_polygon);
            (!clipped.0.is_empty()).then_some(clipped)
        })
        .collect::<Vec<_>>();
    let marker_obstacles = (!marker_areas.is_empty()).then(|| unary_union(marker_areas.iter()));
    timing.obstacle_us = elapsed_us(obstacle_started);
    let ribbon_clip_work_ns = AtomicU64::new(0);
    let building_cutback_work_ns = AtomicU64::new(0);
    let ribbon_clip_count = AtomicUsize::new(0);
    let clip_ribbon = |line: &VectorSurfaceLine| {
        let clip_started = Instant::now();
        ribbon_clip_count.fetch_add(1, Ordering::Relaxed);
        let local_points = line
            .points_mm
            .iter()
            .map(|point| Coord {
                x: f64::from(point[0] - origin_x),
                y: f64::from(point[1] - origin_y),
            })
            .collect::<Vec<_>>();
        let ribbon = LineString::new(local_points).buffer(f64::from(line.width_mm) * 0.5);
        let mut clipped = ribbon.intersection(&piece_polygon);
        if let Some(obstacles) = &obstacles {
            let cutback_started = Instant::now();
            clipped = clipped.difference(obstacles);
            building_cutback_work_ns.fetch_add(elapsed_ns(cutback_started), Ordering::Relaxed);
        }
        if let Some(marker_obstacles) = &marker_obstacles {
            clipped = clipped.difference(marker_obstacles);
        }
        ribbon_clip_work_ns.fetch_add(elapsed_ns(clip_started), Ordering::Relaxed);
        clipped
    };
    let (bridges, regular): (Vec<_>, Vec<_>) = road_and_rail
        .into_iter()
        .partition(|line| line.bridge_elevations_m.is_some());
    // Terrain-following railways and aerialways are separate unions from the
    // roads', so they can be cut back against the road union below.
    let (rail_regular, regular): (Vec<_>, Vec<_>) = regular
        .into_iter()
        .partition(|line| line.class == SurfaceClass::Rail);
    let (aerial_regular, regular): (Vec<_>, Vec<_>) = regular
        .into_iter()
        .partition(|line| line.class == SurfaceClass::Aerial);
    let (ferry_regular, regular): (Vec<_>, Vec<_>) = regular
        .into_iter()
        .partition(|line| line.class == SurfaceClass::Ferry);
    let (aviation_regular, regular): (Vec<_>, Vec<_>) = regular
        .into_iter()
        .partition(|line| line.class == SurfaceClass::Aviation);
    let (route_trail_regular, regular): (Vec<_>, Vec<_>) = regular
        .into_iter()
        .partition(|line| line.class == SurfaceClass::RouteTrail);
    // Ordinary ribbons are clipped in parallel and unioned; the union is
    // shelled per connected component further below, once the bridge decks
    // it must keep clear of are known.
    let regular_started = Instant::now();
    let regular_areas = regular
        .par_iter()
        .map(|line| clip_ribbon(line))
        .collect::<Vec<_>>();
    let mut road_area = unary_union(regular_areas.iter());
    // Bridge decks follow their own elevation profile, so they cannot join
    // the terrain-following union. But one physical bridge arrives as many
    // lines — chained segments that share endpoints (whose round buffer
    // caps coincide exactly) and parallel carriageways — and separate
    // shells over those overlaps leave coincident deck and wall faces. So
    // bridge lines whose clipped ribbons overlap at (nearly) the same deck
    // height merge into one group, each group unions into one footprint,
    // and every group vertex takes its height from the nearest line of the
    // group. Crossings at different heights stay separate shells, exactly
    // as flyovers must.
    let bridge_areas = bridges
        .par_iter()
        .map(|line| clip_ribbon(line))
        .collect::<Vec<_>>();
    let bridge_bounds = bridge_areas
        .iter()
        .map(multi_polygon_bounds)
        .collect::<Vec<_>>();
    let mut parent = (0..bridges.len()).collect::<Vec<_>>();
    fn root(parent: &mut [usize], mut index: usize) -> usize {
        while parent[index] != index {
            parent[index] = parent[parent[index]];
            index = parent[index];
        }
        index
    }
    for first in 0..bridges.len() {
        for second in first + 1..bridges.len() {
            if !bounds_overlap(bridge_bounds[first], bridge_bounds[second]) {
                continue;
            }
            let overlap = bridge_areas[first].intersection(&bridge_areas[second]);
            let Some(largest) = overlap
                .0
                .iter()
                .max_by(|a, b| a.unsigned_area().total_cmp(&b.unsigned_area()))
                .filter(|polygon| polygon.unsigned_area() > MINIMUM_OVERLAY_AREA_MM2)
            else {
                continue;
            };
            let Some(sample) = largest.centroid() else {
                continue;
            };
            let sample = [sample.x() as f32, sample.y() as f32];
            let deck_z = |line: &VectorSurfaceLine| {
                bridge_line_z(
                    spec,
                    line,
                    height_field,
                    height_range,
                    ((sample[0] + origin_x) / assembled_width).clamp(0.0, 1.0),
                    ((sample[1] + origin_y) / assembled_height).clamp(0.0, 1.0),
                )
            };
            if (deck_z(bridges[first]) - deck_z(bridges[second])).abs() <= BRIDGE_DECK_JOIN_MM {
                let left = root(&mut parent, first);
                let right = root(&mut parent, second);
                if left != right {
                    parent[left.max(right)] = left.min(right);
                }
            }
        }
    }
    let mut groups: Vec<(Vec<&VectorSurfaceLine>, Vec<&MultiPolygon<f64>>)> = Vec::new();
    let mut group_of_root = HashMap::<usize, usize>::new();
    for index in 0..bridges.len() {
        let group_root = root(&mut parent, index);
        let group = *group_of_root.entry(group_root).or_insert_with(|| {
            groups.push((Vec::new(), Vec::new()));
            groups.len() - 1
        });
        groups[group].0.push(bridges[index]);
        groups[group].1.push(&bridge_areas[index]);
    }
    let decks = groups
        .into_iter()
        .map(|(group_lines, group_areas)| (group_lines, unary_union(group_areas)))
        .collect::<Vec<_>>();
    // Where a deck touches down — the same OSM way continues as a plain road
    // from the bridge's end node, so both ribbons carry the identical round
    // buffer cap there — the two shells would share exact boundary faces.
    // The plain road yields: every overlap where the deck sits at road level
    // is cut out of the road union with the separation gap.
    for (group_lines, deck_area) in &decks {
        for overlap in road_area.intersection(deck_area).0 {
            if overlap.unsigned_area() <= MINIMUM_OVERLAY_AREA_MM2 {
                continue;
            }
            let Some(sample) = overlap.centroid() else {
                continue;
            };
            let assembled = [sample.x() as f32 + origin_x, sample.y() as f32 + origin_y];
            let u = (assembled[0] / assembled_width).clamp(0.0, 1.0);
            let v = (assembled[1] / assembled_height).clamp(0.0, 1.0);
            let road_level = terrain_z_at(spec, height_field, height_range, u, v);
            let deck_level = nearest_deck_line(group_lines, assembled)
                .map(|line| bridge_line_z(spec, line, height_field, height_range, u, v))
                .unwrap_or(road_level);
            if (deck_level - road_level).abs() <= BRIDGE_DECK_JOIN_MM {
                road_area = road_area.difference(&overlap.buffer(OVERLAY_SEPARATION_MM));
            }
        }
    }
    // Shell the plain roads per connected component; the stable union output
    // order keeps the emitted bytes reproducible.
    let road_area = sanitize_footprint_group(road_area, true);
    let regular_shells = road_area
        .0
        .par_iter()
        .filter(|polygon| polygon.unsigned_area() > MINIMUM_OVERLAY_AREA_MM2)
        .map(|polygon| {
            build_road_polygon_shell(
                polygon,
                spec,
                &[],
                height_field,
                height_range,
                origin_x,
                origin_y,
                assembled_width,
                assembled_height,
            )
        })
        .collect::<Result<Vec<_>>>()?;
    for shell in regular_shells {
        mesh.append_isolated(shell);
    }
    for (ordinal, (group_lines, deck_area)) in decks.iter().enumerate() {
        // Deck groups get staggered embed depths. Supported decks of
        // *different* groups follow the same terrain-hugging bottom, and
        // where two groups meet at a shared road node their buffered end
        // caps coincide exactly — distinct embed depths keep those bottoms
        // from welding into one non-manifold sheet. The depths CYCLE every
        // 64 groups (the offset must stay far below print resolution, so it
        // cannot grow without bound): two groups 64 apart do share a depth,
        // which is only a problem if those two groups also touch — a piece
        // would need 65 mutually touching same-level deck groups to hit
        // that, and the level-join pass above merges touching same-level
        // groups first.
        let embed_mm = quantize_export_coordinate(
            OVERLAY_TERRAIN_EMBED_MM + ((ordinal % 64) as f32 + 1.0) * 0.000_05,
        );
        // Groups merge purely on overlap and deck level, never on class, so
        // that two same-level ribbons can never leave coincident faces. The
        // shell stays whole, while each face takes the material of its
        // nearest bridge line. A rail viaduct crossing a road bridge can
        // therefore keep its color without making two overlapping shells.
        let deck_area = sanitize_footprint_group(deck_area.clone(), true);
        let group_shells = deck_area
            .0
            .par_iter()
            .filter(|polygon| polygon.unsigned_area() > MINIMUM_OVERLAY_AREA_MM2)
            .map(|polygon| {
                build_road_polygon_shell_with_embed(
                    polygon,
                    spec,
                    group_lines,
                    height_field,
                    height_range,
                    origin_x,
                    origin_y,
                    assembled_width,
                    assembled_height,
                    embed_mm,
                    SurfaceClass::Road,
                )
            })
            .collect::<Result<Vec<_>>>()?;
        for shell in group_shells {
            mesh.append_isolated(shell);
        }
    }
    timing.regular_and_bridge_us = elapsed_us(regular_started);
    // Trails yield to the roads and their decks; railways yield to those and
    // to the trails. Each layer only ever cedes ground to the layers added
    // before it, so adding railways leaves trail geometry untouched.
    let secondary_started = Instant::now();
    let mut claimed = vec![road_area];
    if !route_trail_regular.is_empty() {
        let route_trail_area = append_overlay_geometry(
            mesh,
            spec,
            SurfaceClass::RouteTrail,
            "triangulate mapped trail ribbon",
            &route_trail_regular,
            &clip_ribbon,
            &claimed,
            &decks,
            height_field,
            height_range,
            origin_x,
            origin_y,
            assembled_width,
            assembled_height,
        )?;
        claimed.push(route_trail_area);
    }
    if !trail_lines.is_empty() {
        let trail_area = append_overlay_geometry(
            mesh,
            spec,
            SurfaceClass::Trail,
            "triangulate imported trail ribbon",
            &trail_lines,
            &clip_ribbon,
            &claimed,
            &decks,
            height_field,
            height_range,
            origin_x,
            origin_y,
            assembled_width,
            assembled_height,
        )?;
        claimed.push(trail_area);
    }
    if !rail_regular.is_empty() {
        let rail_area = append_overlay_geometry(
            mesh,
            spec,
            SurfaceClass::Rail,
            "triangulate railway ribbon",
            &rail_regular,
            &clip_ribbon,
            &claimed,
            &decks,
            height_field,
            height_range,
            origin_x,
            origin_y,
            assembled_width,
            assembled_height,
        )?;
        claimed.push(rail_area);
    }
    // Aerialways next, so switching the lift layer on can never move a
    // road, trail, or railway triangle that was already there.
    if !aerial_regular.is_empty() {
        let aerial_area = append_overlay_geometry(
            mesh,
            spec,
            SurfaceClass::Aerial,
            "triangulate aerialway ribbon",
            &aerial_regular,
            &clip_ribbon,
            &claimed,
            &decks,
            height_field,
            height_range,
            origin_x,
            origin_y,
            assembled_width,
            assembled_height,
        )?;
        claimed.push(aerial_area);
    }
    // Ferries go last for the same reason: they are the newest layer, so
    // they yield to every overlay that could already be there.
    if !ferry_regular.is_empty() {
        append_overlay_geometry(
            mesh,
            spec,
            SurfaceClass::Ferry,
            "triangulate ferry ribbon",
            &ferry_regular,
            &clip_ribbon,
            &claimed,
            &decks,
            height_field,
            height_range,
            origin_x,
            origin_y,
            assembled_width,
            assembled_height,
        )?;
    }
    timing.secondary_overlay_us = elapsed_us(secondary_started);
    // Airport pavement is placed after every other overlay, so a road, rail
    // line, or crossing over it keeps the ground it already had. Within the
    // layer the strips go down before the aprons, which is why a runway
    // drawn across an apron reads as runway.
    // Grading is shared by the ribbons and the outlines: one height
    // function over the whole layer, so a taxiway meeting a runway and an
    // apron meeting both arrive at the same z where they touch, and the
    // joins cannot crack.
    let aviation_started = Instant::now();
    let profiles = aviation_profiles(
        spec,
        &aviation_regular,
        height_field,
        height_range,
        assembled_width,
        assembled_height,
    );
    let aviation_base = |point: [f32; 2]| {
        let assembled = [point[0] + origin_x, point[1] + origin_y];
        let u = (assembled[0] / assembled_width).clamp(0.0, 1.0);
        let v = (assembled[1] / assembled_height).clamp(0.0, 1.0);
        let terrain = terrain_z_at(spec, height_field, height_range, u, v);
        // The nearest graded line owns the point, but only near itself:
        // inside its own ribbon the grading is the whole answer, and its
        // say fades to nothing over the band beyond. Out in an apron away
        // from every strip, the ground is what is left.
        let mut nearest = None::<(f32, f32, f32)>;
        for profile in &profiles {
            if assembled[0] < profile.reach[0]
                || assembled[0] > profile.reach[2]
                || assembled[1] < profile.reach[1]
                || assembled[1] > profile.reach[3]
            {
                continue;
            }
            let distance = polyline_distance_squared(&profile.line.points_mm, assembled);
            if nearest.is_none_or(|(best, _, _)| distance < best) {
                nearest = Some((distance, profile.z_at(u, v), profile.line.width_mm * 0.5));
            }
        }
        let Some((distance_squared, graded, half_width)) = nearest else {
            return terrain;
        };
        let distance = distance_squared.sqrt();
        let fade_end = half_width + AVIATION_GRADE_FADE_MM;
        let share = if distance <= half_width {
            1.0
        } else if distance >= fade_end {
            0.0
        } else {
            1.0 - (distance - half_width) / AVIATION_GRADE_FADE_MM
        };
        // Smoothstep, so the join carries no crease for a slicer to find.
        let share = share * share * (3.0 - 2.0 * share);
        // Never below the ground at this exact point, whatever the profile
        // says. Reading the high side across a strip only clears the ground
        // under that strip: where strips run close together, as they do all
        // over a real airfield, the nearest centre line is often not the one
        // whose ribbon the point is in, and its profile can sit under the
        // ground here. The same goes for terrain noise between two stations,
        // which on a flat airport at full relief is millimetres tall.
        //
        // So the clearance is a floor rather than an outcome: the surface is
        // flat where the profile stands above the ground and follows the
        // ground where it would not. Buried pavement is a hole in the model,
        // and no amount of sampling makes that acceptable.
        terrain + ((graded - terrain) * share).max(0.0)
    };
    if !aviation_regular.is_empty() {
        let aviation_area = append_overlay_geometry_at_height(
            mesh,
            spec,
            SurfaceClass::Aviation,
            "triangulate airport surface ribbon",
            &aviation_regular,
            spec.color_output.aviation.aviation_height_mm,
            Some(aviation_interior_step_mm(spec)),
            &aviation_base,
            &clip_ribbon,
            &claimed,
            &decks,
            height_field,
            height_range,
            origin_x,
            origin_y,
            assembled_width,
            assembled_height,
        )?;
        claimed.push(aviation_area);
    }
    // Outlines keep clear of buildings and markers exactly as ribbons do.
    // An apron is often drawn right up to the terminal standing in it, and
    // where the apron's ring and the building's outline share coordinates
    // the two shells leave coincident faces for a slicer's weld to fuse
    // into non-manifold edges.
    let mut aviation_outlines = aviation_outlines;
    if let Some(obstacles) = &obstacles {
        aviation_outlines = aviation_outlines.difference(obstacles);
    }
    if let Some(marker_obstacles) = &marker_obstacles {
        aviation_outlines = aviation_outlines.difference(marker_obstacles);
    }
    if !aviation_outlines.0.is_empty() {
        append_overlay_footprint(
            mesh,
            spec,
            SurfaceClass::Aviation,
            "triangulate airport surface outline",
            aviation_outlines,
            spec.color_output.aviation.aviation_height_mm,
            Some(aviation_interior_step_mm(spec)),
            &aviation_base,
            &claimed,
            &decks,
            height_field,
            height_range,
            origin_x,
            origin_y,
            assembled_width,
            assembled_height,
        )?;
    }
    timing.aviation_us = elapsed_us(aviation_started);
    timing.ribbon_clip_count = ribbon_clip_count.load(Ordering::Relaxed);
    timing.ribbon_clip_work_us = ribbon_clip_work_ns.load(Ordering::Relaxed) / 1_000;
    timing.building_cutback_work_us = building_cutback_work_ns.load(Ordering::Relaxed) / 1_000;
    timing.total_us = elapsed_us(total_started);
    Ok(timing)
}

/// The piece's share of every aeroway area, holes kept.
///
/// Outlines arrive in normalized map space and in whatever winding their
/// mapper drew; the piece wants millimetres in its own frame, so the ring
/// order is normalized here rather than trusted. Clipping against the piece
/// is what keeps an apron that crosses a seam watertight on both sides.
fn aviation_area_footprint(
    field: &SurfaceField,
    piece_polygon: &Polygon<f64>,
    origin_x: f32,
    origin_y: f32,
    assembled_width: f32,
    assembled_height: f32,
) -> MultiPolygon<f64> {
    let ring = |points: &[[f32; 2]]| {
        let mut coords = points
            .iter()
            .map(|point| Coord {
                x: f64::from(point[0] * assembled_width - origin_x),
                y: f64::from(point[1] * assembled_height - origin_y),
            })
            .collect::<Vec<_>>();
        if let Some(first) = coords.first().copied() {
            coords.push(first);
        }
        LineString::new(coords)
    };
    // Only the aprons this piece could touch. Unioning every one of them
    // for every piece is work a hundred-piece puzzle does a hundred times
    // over, and the bounds test is the same one buildings use.
    let piece_bounds = {
        let bounds = piece_polygon.exterior();
        bounds.coords().fold(
            [
                f32::INFINITY,
                f32::INFINITY,
                f32::NEG_INFINITY,
                f32::NEG_INFINITY,
            ],
            |box_, point| {
                let u = (point.x as f32 + origin_x) / assembled_width;
                let v = (point.y as f32 + origin_y) / assembled_height;
                [
                    box_[0].min(u),
                    box_[1].min(v),
                    box_[2].max(u),
                    box_[3].max(v),
                ]
            },
        )
    };
    let outlines = field
        .vector_areas
        .iter()
        .filter(|area| area.class == Some(SurfaceClass::Aviation) && area.points.len() >= 3)
        .filter(|area| bounds_overlap(surface_area_bounds(&area.points), piece_bounds))
        .map(|area| {
            Polygon::new(
                ring(&area.points),
                area.holes.iter().map(|hole| ring(hole)).collect(),
            )
            .orient(Direction::Default)
        })
        .collect::<Vec<_>>();
    if outlines.is_empty() {
        return MultiPolygon::new(Vec::new());
    }
    unary_union(outlines.iter()).intersection(piece_polygon)
}

/// Builds one secondary overlay's shells for a piece: terrain-following
/// ribbons raised by the road layer height, in that overlay's own material.
/// Imported trails and separately-styled railways both come through here.
///
/// Footprints are clipped and unioned exactly like plain roads and
/// additionally keep [`OVERLAY_SEPARATION_MM`] clear of every already
/// `claimed` area and of every bridge deck, so an overlay crossing a road
/// never leaves coincident top or bottom faces for a slicer weld to fuse
/// into non-manifold edges. Returns the finished footprint so a later
/// overlay can claim against it in turn.
#[allow(clippy::too_many_arguments)]
fn append_overlay_geometry(
    mesh: &mut Mesh,
    spec: &GenerationSpec,
    material: SurfaceClass,
    error_context: &'static str,
    lines: &[&VectorSurfaceLine],
    clip_ribbon: &(impl Fn(&VectorSurfaceLine) -> MultiPolygon<f64> + Sync),
    claimed: &[MultiPolygon<f64>],
    decks: &[(Vec<&VectorSurfaceLine>, MultiPolygon<f64>)],
    height_field: Option<&HeightField>,
    height_range: Option<(f32, f32)>,
    origin_x: f32,
    origin_y: f32,
    assembled_width: f32,
    assembled_height: f32,
) -> Result<MultiPolygon<f64>> {
    let terrain_base = |point: [f32; 2]| {
        let u = ((point[0] + origin_x) / assembled_width).clamp(0.0, 1.0);
        let v = ((point[1] + origin_y) / assembled_height).clamp(0.0, 1.0);
        terrain_z_at(spec, height_field, height_range, u, v)
    };
    append_overlay_geometry_at_height(
        mesh,
        spec,
        material,
        error_context,
        lines,
        spec.color_output.road_height_mm,
        None,
        &terrain_base,
        clip_ribbon,
        claimed,
        decks,
        height_field,
        height_range,
        origin_x,
        origin_y,
        assembled_width,
        assembled_height,
    )
}

/// [`append_overlay_geometry`] for a layer that stands at its own height
/// rather than the road layer's.
#[allow(clippy::too_many_arguments)]
fn append_overlay_geometry_at_height(
    mesh: &mut Mesh,
    spec: &GenerationSpec,
    material: SurfaceClass,
    error_context: &'static str,
    lines: &[&VectorSurfaceLine],
    height_mm: f32,
    interior_step_mm: Option<f32>,
    base_z: &(impl Fn([f32; 2]) -> f32 + Sync),
    clip_ribbon: &(impl Fn(&VectorSurfaceLine) -> MultiPolygon<f64> + Sync),
    claimed: &[MultiPolygon<f64>],
    decks: &[(Vec<&VectorSurfaceLine>, MultiPolygon<f64>)],
    height_field: Option<&HeightField>,
    height_range: Option<(f32, f32)>,
    origin_x: f32,
    origin_y: f32,
    assembled_width: f32,
    assembled_height: f32,
) -> Result<MultiPolygon<f64>> {
    let clips = lines
        .par_iter()
        .map(|line| clip_ribbon(line))
        .collect::<Vec<_>>();
    append_overlay_footprint(
        mesh,
        spec,
        material,
        error_context,
        unary_union(clips.iter()),
        height_mm,
        interior_step_mm,
        base_z,
        claimed,
        decks,
        height_field,
        height_range,
        origin_x,
        origin_y,
        assembled_width,
        assembled_height,
    )
}

/// Places one already-clipped overlay footprint: cuts it back against the
/// layers claimed before it, drops the parts coincident with a terrain-level
/// deck, and shells what is left onto the terrain.
///
/// Shared by every terrain-following overlay. Ribbons reach it as the union
/// of their buffered centre lines; airport pavement drawn as an outline
/// reaches it as that outline. Beyond how the footprint was arrived at
/// there is nothing different to do, and having one path is what keeps a
/// runway's edge behaving like a road's.
#[allow(clippy::too_many_arguments)]
fn append_overlay_footprint(
    mesh: &mut Mesh,
    spec: &GenerationSpec,
    material: SurfaceClass,
    error_context: &'static str,
    footprint: MultiPolygon<f64>,
    height_mm: f32,
    // Spacing of the points scattered inside the footprint so its top can
    // follow the ground rather than span it.
    interior_step_mm: Option<f32>,
    // The surface the overlay is laid on, given a piece-local point. Every
    // layer but airport pavement passes the terrain itself; a runway passes
    // its own graded profile, which is what makes its cross-section level
    // instead of draped over two separate DEM samples.
    base_z: &(impl Fn([f32; 2]) -> f32 + Sync),
    claimed: &[MultiPolygon<f64>],
    decks: &[(Vec<&VectorSurfaceLine>, MultiPolygon<f64>)],
    height_field: Option<&HeightField>,
    height_range: Option<(f32, f32)>,
    origin_x: f32,
    origin_y: f32,
    assembled_width: f32,
    assembled_height: f32,
) -> Result<MultiPolygon<f64>> {
    let mut overlay_area = footprint;
    let grown = |area: &MultiPolygon<f64>| {
        let buffered = area
            .0
            .iter()
            .map(|polygon| polygon.buffer(OVERLAY_SEPARATION_MM))
            .collect::<Vec<_>>();
        unary_union(buffered.iter())
    };
    for area in claimed {
        if !area.0.is_empty() {
            overlay_area = overlay_area.difference(&grown(area));
        }
    }
    // Decks are cut out only where they sit at overlay (terrain) level — the
    // same [`BRIDGE_DECK_JOIN_MM`] gate the road union uses. An elevated
    // deck shares no faces with a terrain-following ribbon, so a trail or a
    // railway under a flyover keeps running instead of getting a gap.
    for (group_lines, deck_area) in decks {
        for overlap in overlay_area.intersection(deck_area).0 {
            if overlap.unsigned_area() <= MINIMUM_OVERLAY_AREA_MM2 {
                continue;
            }
            let Some(sample) = overlap.centroid() else {
                continue;
            };
            let assembled = [sample.x() as f32 + origin_x, sample.y() as f32 + origin_y];
            let u = (assembled[0] / assembled_width).clamp(0.0, 1.0);
            let v = (assembled[1] / assembled_height).clamp(0.0, 1.0);
            let overlay_level = terrain_z_at(spec, height_field, height_range, u, v);
            let deck_level = nearest_deck_line(group_lines, assembled)
                .map(|line| bridge_line_z(spec, line, height_field, height_range, u, v))
                .unwrap_or(overlay_level);
            if (deck_level - overlay_level).abs() <= BRIDGE_DECK_JOIN_MM {
                overlay_area = overlay_area.difference(&overlap.buffer(OVERLAY_SEPARATION_MM));
            }
        }
    }
    // The differences above leave hair-thin slivers where an overlay border
    // runs nearly tangent to a road border: boundary vertices one export
    // quantum apart that triangulate into zero-area faces. A Douglas-Peucker
    // pass at a tenth of the separation gap removes those vertices without
    // visibly moving the outline; sanitizing then handles exact duplicates.
    let overlay_area =
        sanitize_footprint_group(overlay_area.simplify(OVERLAY_SEPARATION_MM * 0.1), true);
    let surface_z = |point: [f32; 2]| {
        let u = ((point[0] + origin_x) / assembled_width).clamp(0.0, 1.0);
        let v = ((point[1] + origin_y) / assembled_height).clamp(0.0, 1.0);
        terrain_z_at(spec, height_field, height_range, u, v)
    };
    let shells = overlay_area
        .0
        .par_iter()
        .filter(|polygon| polygon.unsigned_area() > MINIMUM_OVERLAY_AREA_MM2)
        .map(|polygon| {
            build_polygon_shell(
                polygon,
                // Where a graded surface cuts below the ground it crosses,
                // the shell's bottom follows the ground instead, or the
                // solid would be inside out.
                |point| base_z(point).min(surface_z(point)) - OVERLAY_TERRAIN_EMBED_MM,
                |point| base_z(point) + height_mm,
                // The outline is densified to match. A dense interior
                // beside a sparse edge is not enough: the band between the
                // last row of interior points and the outline is still
                // spanned by triangles reaching between whatever boundary
                // vertices happen to exist, and the ground rises through
                // those exactly as it did before.
                interior_step_mm,
                interior_step_mm,
                material,
                error_context,
            )
        })
        .collect::<Result<Vec<_>>>()?;
    for shell in shells {
        mesh.append_isolated(shell);
    }
    Ok(overlay_area)
}

/// Deck heights within this tolerance where two bridge ribbons overlap mean
/// one physical deck (chained segments, parallel carriageways); a larger gap
/// means a flyover crossing that must keep its own shell.
const BRIDGE_DECK_JOIN_MM: f32 = 0.05;

/// Print height of a bridge line's deck surface at one map position.
fn bridge_line_z(
    spec: &GenerationSpec,
    line: &VectorSurfaceLine,
    height_field: Option<&HeightField>,
    height_range: Option<(f32, f32)>,
    u: f32,
    v: f32,
) -> f32 {
    if let (Some([start, end]), Some((minimum, span))) = (line.bridge_elevations_m, height_range) {
        let progress = surface_line_progress(line, u, v);
        let elevation = start + (end - start) * progress;
        spec.base_mm + spec.relief_mm * ((elevation - minimum) / span).max(0.0)
    } else {
        terrain_z_at(spec, height_field, height_range, u, v)
    }
}

/// Stations sampled along an aeroway centre line when following it. Dense
/// enough that the ground between two of them cannot rise through the
/// pavement laid across them.
const AVIATION_PROFILE_STATIONS: usize = 256;
/// How far beyond its own ribbon a centre-line profile keeps any say over
/// the height, in mm of print.
///
/// A profile is measured along one strip and is only true near it. Letting
/// the nearest one own the whole layer puts an apron a kilometre away at
/// the runway's height, which on sloping ground sinks it into the hill or
/// floats it over one. Easing back to the ground over a short band keeps
/// the height function continuous, so an apron abutting a taxiway still
/// meets it without a step.
const AVIATION_GRADE_FADE_MM: f32 = 4.0;

/// Spacing of the points scattered inside airport pavement, in mm of
/// print, matched to the terrain's own sample spacing. Finer buys nothing,
/// because the ground carries no more detail than that; coarser lets the
/// ground rise through the surface between them.
fn aviation_interior_step_mm(spec: &GenerationSpec) -> f32 {
    let samples = spec.assembled_overlay_samples().max(1) as f32;
    (spec.width_mm / samples).clamp(0.1, 2.0)
}

/// How many samples across the ribbon each station takes to find the
/// ground it must clear.
const AVIATION_CROSS_SAMPLES: usize = 5;

/// The elevation along one aeroway centre line.
///
/// A runway is flat across its width and not along its length: it rises and
/// falls with the ground it is laid on, but a section cut across it is
/// level. Sampling the terrain per mesh vertex gives the opposite — the
/// ribbon tilts side to side wherever two coarse samples disagree.
///
/// So the ground is read along the centre line, and every point of the
/// ribbon takes the height of its own station. Points across the width
/// share a station, which is what makes the cross-section level; stations
/// along the length follow the terrain, which is what stops the pavement
/// burying itself in a hill it should be climbing.
///
/// A station takes the HIGHEST ground across its own width, not the height
/// under the centre line. A level surface set to the middle of a cross
/// slope buries its uphill edge, and buried pavement is not pavement — it
/// is a hole in the model where a runway should be. Taking the high side
/// fills instead of cutting, which is what an airfield does to the ground
/// anyway.
struct AviationProfile<'lines> {
    line: &'lines VectorSurfaceLine,
    /// Print z at evenly spaced stations from the line's start to its end.
    stations: Vec<f32>,
    /// Assembled-mm box beyond which this profile has no say, being its
    /// ribbon grown by the fade band. Every point of the layer asks every
    /// profile how far away it is, so a busy airport would otherwise walk
    /// hundreds of polylines per vertex. Skipping a profile whose box
    /// misses the point changes no answer — past the fade its share is
    /// zero — it just stops asking.
    reach: [f32; 4],
}

impl AviationProfile<'_> {
    /// Print z where an assembled-mm point projects onto this line.
    fn z_at(&self, u: f32, v: f32) -> f32 {
        let progress = surface_line_progress(self.line, u, v).clamp(0.0, 1.0);
        let last = self.stations.len() - 1;
        let position = progress * last as f32;
        let index = (position.floor() as usize).min(last);
        let next = (index + 1).min(last);
        let fraction = position - index as f32;
        self.stations[index] + (self.stations[next] - self.stations[index]) * fraction
    }
}

/// Grades every aeroway line: reads the terrain along it, then smooths.
fn aviation_profiles<'lines>(
    spec: &GenerationSpec,
    lines: &[&'lines VectorSurfaceLine],
    height_field: Option<&HeightField>,
    height_range: Option<(f32, f32)>,
    assembled_width: f32,
    assembled_height: f32,
) -> Vec<AviationProfile<'lines>> {
    lines
        .par_iter()
        .map(|line| {
            let half_width = line.width_mm * 0.5;
            let stations = (0..=AVIATION_PROFILE_STATIONS)
                .map(|station| {
                    let progress = station as f32 / AVIATION_PROFILE_STATIONS as f32;
                    let point = polyline_point_at_progress(&line.points_mm, progress);
                    let along = polyline_direction_at_progress(&line.points_mm, progress);
                    // Perpendicular to the strip, so the samples cross it.
                    let across = [-along[1], along[0]];
                    (0..AVIATION_CROSS_SAMPLES)
                        .map(|sample| {
                            let offset = if AVIATION_CROSS_SAMPLES <= 1 {
                                0.0
                            } else {
                                (sample as f32 / (AVIATION_CROSS_SAMPLES - 1) as f32 - 0.5) * 2.0
                            } * half_width;
                            let at = [point[0] + across[0] * offset, point[1] + across[1] * offset];
                            let u = (at[0] / assembled_width).clamp(0.0, 1.0);
                            let v = (at[1] / assembled_height).clamp(0.0, 1.0);
                            terrain_z_at(spec, height_field, height_range, u, v)
                        })
                        .fold(f32::NEG_INFINITY, f32::max)
                })
                .collect::<Vec<_>>();
            let margin = line.width_mm * 0.5 + AVIATION_GRADE_FADE_MM;
            let mut reach = [
                f32::INFINITY,
                f32::INFINITY,
                f32::NEG_INFINITY,
                f32::NEG_INFINITY,
            ];
            for point in &line.points_mm {
                reach[0] = reach[0].min(point[0] - margin);
                reach[1] = reach[1].min(point[1] - margin);
                reach[2] = reach[2].max(point[0] + margin);
                reach[3] = reach[3].max(point[1] + margin);
            }
            AviationProfile {
                line,
                stations,
                reach,
            }
        })
        .collect()
}

/// The unit direction of a polyline a fraction of the way along it.
fn polyline_direction_at_progress(points: &[[f32; 2]], progress: f32) -> [f32; 2] {
    let ahead = polyline_point_at_progress(points, (progress + 0.005).min(1.0));
    let behind = polyline_point_at_progress(points, (progress - 0.005).max(0.0));
    let delta = [ahead[0] - behind[0], ahead[1] - behind[1]];
    let length = delta[0].hypot(delta[1]);
    if length <= f32::EPSILON {
        [1.0, 0.0]
    } else {
        [delta[0] / length, delta[1] / length]
    }
}

/// The point a fraction of the way along a polyline, by arc length.
fn polyline_point_at_progress(points: &[[f32; 2]], progress: f32) -> [f32; 2] {
    let Some(first) = points.first().copied() else {
        return [0.0, 0.0];
    };
    let total: f32 = points
        .windows(2)
        .map(|segment| distance(segment[0], segment[1]))
        .sum();
    if total <= f32::EPSILON {
        return first;
    }
    let target = progress.clamp(0.0, 1.0) * total;
    let mut travelled = 0.0;
    for segment in points.windows(2) {
        let length = distance(segment[0], segment[1]);
        if travelled + length >= target {
            let fraction = if length <= f32::EPSILON {
                0.0
            } else {
                (target - travelled) / length
            };
            return [
                segment[0][0] + (segment[1][0] - segment[0][0]) * fraction,
                segment[0][1] + (segment[1][1] - segment[0][1]) * fraction,
            ];
        }
        travelled += length;
    }
    points.last().copied().unwrap_or(first)
}

fn distance(a: [f32; 2], b: [f32; 2]) -> f32 {
    ((b[0] - a[0]).powi(2) + (b[1] - a[1]).powi(2)).sqrt()
}

/// Line of a merged deck group nearest to an assembled-mm point.
fn nearest_deck_line<'lines>(
    deck_lines: &[&'lines VectorSurfaceLine],
    assembled: [f32; 2],
) -> Option<&'lines VectorSurfaceLine> {
    let mut nearest = None::<(f32, &VectorSurfaceLine)>;
    for line in deck_lines {
        let distance = polyline_distance_squared(&line.points_mm, assembled);
        if nearest.is_none_or(|(best, _)| distance < best) {
            nearest = Some((distance, line));
        }
    }
    nearest.map(|(_, line)| line)
}

/// Squared distance from an assembled-mm point to a polyline.
fn polyline_distance_squared(points: &[[f32; 2]], point: [f32; 2]) -> f32 {
    let mut best = f32::INFINITY;
    for segment in points.windows(2) {
        let [start, end] = [segment[0], segment[1]];
        let direction = [end[0] - start[0], end[1] - start[1]];
        let length_squared = direction[0] * direction[0] + direction[1] * direction[1];
        let t = if length_squared <= f32::EPSILON {
            0.0
        } else {
            (((point[0] - start[0]) * direction[0] + (point[1] - start[1]) * direction[1])
                / length_squared)
                .clamp(0.0, 1.0)
        };
        let nearest = [start[0] + direction[0] * t, start[1] + direction[1] * t];
        best = best.min(distance_squared(point, nearest));
    }
    best
}

#[allow(clippy::too_many_arguments)]
fn build_road_polygon_shell(
    polygon: &Polygon<f64>,
    spec: &GenerationSpec,
    deck_lines: &[&VectorSurfaceLine],
    height_field: Option<&HeightField>,
    height_range: Option<(f32, f32)>,
    origin_x: f32,
    origin_y: f32,
    assembled_width: f32,
    assembled_height: f32,
) -> Result<MeshBuilder> {
    build_road_polygon_shell_with_embed(
        polygon,
        spec,
        deck_lines,
        height_field,
        height_range,
        origin_x,
        origin_y,
        assembled_width,
        assembled_height,
        OVERLAY_TERRAIN_EMBED_MM,
        SurfaceClass::Road,
    )
}

#[allow(clippy::too_many_arguments)]
fn build_road_polygon_shell_with_embed(
    polygon: &Polygon<f64>,
    spec: &GenerationSpec,
    deck_lines: &[&VectorSurfaceLine],
    height_field: Option<&HeightField>,
    height_range: Option<(f32, f32)>,
    origin_x: f32,
    origin_y: f32,
    assembled_width: f32,
    assembled_height: f32,
    embed_mm: f32,
    material: SurfaceClass,
) -> Result<MeshBuilder> {
    let road_z = |point: [f32; 2]| {
        let assembled = [point[0] + origin_x, point[1] + origin_y];
        let u = (assembled[0] / assembled_width).clamp(0.0, 1.0);
        let v = (assembled[1] / assembled_height).clamp(0.0, 1.0);
        // A merged deck takes its height from the nearest of its lines, so
        // a chained elevation profile carries across the whole group.
        if let Some(line) = nearest_deck_line(deck_lines, assembled) {
            bridge_line_z(spec, line, height_field, height_range, u, v)
        } else {
            spec.base_mm
                + spec.relief_mm
                    * normalized_height(
                        height_field,
                        height_range,
                        u,
                        v,
                        spec.center_lat,
                        spec.center_lon,
                    )
        }
    };
    let top = |point: [f32; 2]| road_z(point) + spec.color_output.road_height_mm;
    let is_bridge = !deck_lines.is_empty();
    let bottom = |point: [f32; 2]| {
        if !is_bridge {
            return road_z(point) - embed_mm;
        }
        match spec.color_output.bridge_structure {
            BridgeStructure::Floating => top(point) - spec.color_output.bridge_thickness_mm,
            BridgeStructure::Supported => {
                let u = ((point[0] + origin_x) / assembled_width).clamp(0.0, 1.0);
                let v = ((point[1] + origin_y) / assembled_height).clamp(0.0, 1.0);
                (terrain_z_at(spec, height_field, height_range, u, v) - embed_mm)
                    .min(top(point) - embed_mm)
            }
        }
    };
    let boundary_step_mm = (is_bridge
        && spec.color_output.bridge_structure == BridgeStructure::Supported)
        .then_some(ROAD_VECTOR_STEP_MM);
    build_polygon_shell_with_material(
        polygon,
        bottom,
        top,
        boundary_step_mm,
        None,
        |point| {
            let assembled = [point[0] + origin_x, point[1] + origin_y];
            nearest_deck_line(deck_lines, assembled).map_or(material, |line| line.class)
        },
        "triangulate vector road ribbon",
    )
}

/// Distance from a point to the nearest edge of a closed ring.
fn ring_clearance(ring: &[[f32; 2]], point: [f32; 2]) -> f32 {
    let open = polyline_distance_squared(ring, point);
    let closing = match (ring.first(), ring.last()) {
        (Some(first), Some(last)) => polyline_distance_squared(&[*last, *first], point),
        _ => f32::INFINITY,
    };
    open.min(closing).sqrt()
}

pub(super) fn build_polygon_shell(
    polygon: &Polygon<f64>,
    bottom: impl Fn([f32; 2]) -> f32,
    top: impl Fn([f32; 2]) -> f32,
    boundary_step_mm: Option<f32>,
    // Spacing of extra points scattered through the inside of the
    // footprint. `None` triangulates from the outline alone, which is what
    // every thin overlay has always done and is right for them.
    //
    // A wide footprint needs more. Triangulated from its outline, a runway
    // is a handful of triangles stretched over its whole area, so the
    // height computed for it only lands at the corners and the ground in
    // between rises straight through the surface. Points inside give that
    // surface somewhere to follow the ground.
    interior_step_mm: Option<f32>,
    material: SurfaceClass,
    error_context: &'static str,
) -> Result<MeshBuilder> {
    build_polygon_shell_with_material(
        polygon,
        bottom,
        top,
        boundary_step_mm,
        interior_step_mm,
        |_| material,
        error_context,
    )
}

fn build_polygon_shell_with_material(
    polygon: &Polygon<f64>,
    bottom: impl Fn([f32; 2]) -> f32,
    top: impl Fn([f32; 2]) -> f32,
    boundary_step_mm: Option<f32>,
    interior_step_mm: Option<f32>,
    material_at: impl Fn([f32; 2]) -> SurfaceClass,
    error_context: &'static str,
) -> Result<MeshBuilder> {
    let rings = std::iter::once(polygon.exterior())
        .chain(polygon.interiors())
        .map(open_ring_points)
        .map(|ring| {
            boundary_step_mm
                .map(|step| {
                    // Densified midpoints land off the export grid; snap them
                    // back so triangulation and the vertex weld agree.
                    let mut dense = densify_closed_ring(&ring, step)
                        .into_iter()
                        .map(|point| {
                            [
                                quantize_export_coordinate(point[0]),
                                quantize_export_coordinate(point[1]),
                            ]
                        })
                        .collect::<Vec<_>>();
                    dense.dedup();
                    while dense.len() > 1 && dense.first() == dense.last() {
                        dense.pop();
                    }
                    dense
                })
                .unwrap_or(ring)
        })
        .filter(|ring| ring.len() >= 3)
        .collect::<Vec<_>>();
    let mut points = Vec::new();
    let mut constraints = Vec::new();
    if let Some(step) = interior_step_mm.filter(|step| *step > 0.0) {
        let bounds = rings.iter().flatten().fold(
            [
                f32::INFINITY,
                f32::INFINITY,
                f32::NEG_INFINITY,
                f32::NEG_INFINITY,
            ],
            |box_, point| {
                [
                    box_[0].min(point[0]),
                    box_[1].min(point[1]),
                    box_[2].max(point[0]),
                    box_[3].max(point[1]),
                ]
            },
        );
        let columns = (((bounds[2] - bounds[0]) / step).ceil() as usize).min(2048);
        let rows = (((bounds[3] - bounds[1]) / step).ceil() as usize).min(2048);
        for row in 1..rows {
            for column in 1..columns {
                let point = [
                    quantize_export_coordinate(bounds[0] + column as f32 * step),
                    quantize_export_coordinate(bounds[1] + row as f32 * step),
                ];
                // Inside the outline, outside every hole, and clear of both:
                // a point landing on a constraint edge splits it and leaves
                // a crack down the seam.
                let inside = point_in_polygon(point, &rings[0])
                    && !rings[1..].iter().any(|hole| point_in_polygon(point, hole));
                if inside
                    && rings
                        .iter()
                        .all(|ring| ring_clearance(ring, point) > step * 0.25)
                {
                    points.push(Point2::new(f64::from(point[0]), f64::from(point[1])));
                }
            }
        }
    }
    for ring in &rings {
        let start = points.len();
        points.extend(
            ring.iter()
                .map(|point| Point2::new(point[0] as f64, point[1] as f64)),
        );
        constraints
            .extend((0..ring.len()).map(|index| [start + index, start + (index + 1) % ring.len()]));
    }
    if points.len() < 3 {
        return Ok(MeshBuilder::default());
    }
    let triangulation = triangulate_constraints(points, constraints, error_context)?;
    let mut inside = interior_faces_by_parity(&triangulation);
    repair_classification_pinches(&triangulation, &mut inside, true);
    let mut output = MeshBuilder::default();
    let mut edge_uses = HashMap::<(usize, usize), (u32, [usize; 2])>::new();
    let mut vertex_positions = HashMap::<usize, [f32; 2]>::new();
    for face in triangulation.inner_faces() {
        if !inside[face.fix().index()] {
            continue;
        }
        let face_vertices = face.vertices();
        let face_points = face_vertices.map(|vertex| {
            let point = vertex.position();
            [point.x as f32, point.y as f32]
        });
        let mut ordered = face_points;
        let mut ordered_indices = face_vertices.map(|vertex| vertex.fix().index());
        let area = (ordered[1][0] - ordered[0][0]) * (ordered[2][1] - ordered[0][1])
            - (ordered[1][1] - ordered[0][1]) * (ordered[2][0] - ordered[0][0]);
        if area < 0.0 {
            ordered.swap(1, 2);
            ordered_indices.swap(1, 2);
        }
        for (index, point) in ordered_indices.into_iter().zip(ordered) {
            vertex_positions.insert(index, point);
        }
        for directed in [
            [ordered_indices[0], ordered_indices[1]],
            [ordered_indices[1], ordered_indices[2]],
            [ordered_indices[2], ordered_indices[0]],
        ] {
            let key = if directed[0] < directed[1] {
                (directed[0], directed[1])
            } else {
                (directed[1], directed[0])
            };
            let entry = edge_uses.entry(key).or_insert((0, directed));
            entry.0 += 1;
        }
        let centroid = [
            (ordered[0][0] + ordered[1][0] + ordered[2][0]) / 3.0,
            (ordered[0][1] + ordered[1][1] + ordered[2][1]) / 3.0,
        ];
        let material = material_at(centroid);
        output.triangle(
            [ordered[0][0], ordered[0][1], top(ordered[0])],
            [ordered[1][0], ordered[1][1], top(ordered[1])],
            [ordered[2][0], ordered[2][1], top(ordered[2])],
            material,
        );
        output.triangle(
            [ordered[0][0], ordered[0][1], bottom(ordered[0])],
            [ordered[2][0], ordered[2][1], bottom(ordered[2])],
            [ordered[1][0], ordered[1][1], bottom(ordered[1])],
            material,
        );
    }
    // Sorted for the same run-to-run reproducibility as the terrain walls.
    let mut boundary_edges = edge_uses
        .into_values()
        .filter(|(uses, _)| *uses == 1)
        .map(|(_, edge)| edge)
        .collect::<Vec<_>>();
    boundary_edges.sort_unstable();
    for [from, to] in boundary_edges {
        let start = vertex_positions[&from];
        let end = vertex_positions[&to];
        let material = material_at([(start[0] + end[0]) * 0.5, (start[1] + end[1]) * 0.5]);
        output.quad(
            [start[0], start[1], bottom(start)],
            [end[0], end[1], bottom(end)],
            [end[0], end[1], top(end)],
            [start[0], start[1], top(start)],
            material,
        );
    }
    Ok(output)
}

/// Classifies every triangulation face as inside or outside the footprint
/// whose closed rings were loaded as constraints, by walking the face
/// adjacency graph from the outer face and flipping sides at every
/// constraint edge. Unlike a point-in-polygon test on face centroids, this
/// cannot misclassify the near-degenerate slivers a snapped, densified
/// boundary produces — misclassified slivers notch the kept set and leave
/// pinched, non-manifold wall verticals.
fn interior_faces_by_parity(
    triangulation: &ConstrainedDelaunayTriangulation<Point2<f64>>,
) -> Vec<bool> {
    let face_count = triangulation.num_all_faces();
    let mut adjacency: Vec<Vec<(u32, bool)>> = vec![Vec::new(); face_count];
    for edge in triangulation.undirected_edges() {
        let constraint = edge.is_constraint_edge();
        let directed = edge.as_directed();
        let left = directed.face().fix().index();
        let right = directed.rev().face().fix().index();
        adjacency[left].push((right as u32, constraint));
        adjacency[right].push((left as u32, constraint));
    }
    let mut inside = vec![false; face_count];
    let mut visited = vec![false; face_count];
    let outer = triangulation.outer_face().fix().index();
    visited[outer] = true;
    let mut queue = std::collections::VecDeque::from([outer]);
    while let Some(face) = queue.pop_front() {
        for &(neighbor, constraint) in &adjacency[face] {
            let neighbor = neighbor as usize;
            if visited[neighbor] {
                continue;
            }
            visited[neighbor] = true;
            inside[neighbor] = inside[face] != constraint;
            queue.push_back(neighbor);
        }
    }
    inside
}

fn densify_closed_ring(points: &[[f32; 2]], maximum_step: f32) -> Vec<[f32; 2]> {
    let mut dense = Vec::new();
    for (start, end) in points.iter().zip(points.iter().cycle().skip(1)) {
        let delta = [end[0] - start[0], end[1] - start[1]];
        let length = delta[0].hypot(delta[1]);
        let segments = (length / maximum_step.max(0.01)).ceil().max(1.0) as usize;
        for index in 0..segments {
            let t = index as f32 / segments as f32;
            dense.push([start[0] + delta[0] * t, start[1] + delta[1] * t]);
        }
    }
    dense
}

fn open_ring_points(ring: &LineString<f64>) -> Vec<[f32; 2]> {
    let mut points = ring
        .0
        .iter()
        .map(|point| {
            [
                quantize_export_coordinate(point.x as f32),
                quantize_export_coordinate(point.y as f32),
            ]
        })
        .collect::<Vec<_>>();
    if points.len() > 1 && distance_squared(points[0], *points.last().unwrap()) < 0.000_000_01 {
        points.pop();
    }
    points.dedup_by(|left, right| distance_squared(*left, *right) < 0.000_000_01);
    simplify_closed_ring(points)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    use crate::heightfield::HeightField;
    use crate::mesh::assert_watertight;
    use crate::piece::build_piece;
    use crate::preview::build_preview;
    use crate::spec::{BuildingSpec, ColorOutputSpec, DotMarkerStyle, MapMarker, MarkerKind};

    #[test]
    fn dot_markers_are_smooth_vector_overlays_without_surface_data() {
        let defaults = GenerationSpec::default();
        let spec = GenerationSpec {
            width_mm: 60.0,
            solid_model: true,
            samples_per_piece: 16,
            markers: vec![MapMarker {
                name: "Centre".into(),
                latitude: defaults.center_lat,
                longitude: defaults.center_lon,
                kind: MarkerKind::Dot,
                label_height_mm: 4.0,
                rotation_degrees: 0.0,
                dot_style: Some(DotMarkerStyle { diameter_mm: 5.0 }),
                flag_style: None,
                label_style: None,
            }],
            ..defaults
        };

        let height_field = HeightField::new(3, 3, vec![0.0; 9], "flat").unwrap();
        let mesh = build_piece(&spec, Some(&height_field), None, 0, 0).unwrap();
        assert_watertight(&mesh);
        let marker_vertices = mesh
            .triangles
            .iter()
            .zip(&mesh.materials)
            .filter(|(_, material)| **material == SurfaceClass::Marker)
            .flat_map(|(triangle, _)| triangle)
            .map(|index| mesh.vertices[*index as usize])
            .collect::<Vec<_>>();
        assert!(marker_vertices.len() >= MARKER_CIRCLE_SEGMENTS * 6);
        let minimum_x = marker_vertices
            .iter()
            .map(|point| point[0])
            .fold(f32::INFINITY, f32::min);
        let maximum_x = marker_vertices
            .iter()
            .map(|point| point[0])
            .fold(f32::NEG_INFINITY, f32::max);
        let minimum_z = marker_vertices
            .iter()
            .map(|point| point[2])
            .fold(f32::INFINITY, f32::min);
        let maximum_z = marker_vertices
            .iter()
            .map(|point| point[2])
            .fold(f32::NEG_INFINITY, f32::max);
        assert!((minimum_x - 27.5).abs() < 0.001);
        assert!((maximum_x - 32.5).abs() < 0.001);
        assert!((minimum_z - (spec.base_mm - OVERLAY_TERRAIN_EMBED_MM)).abs() < 0.001);
        assert!((maximum_z - (spec.base_mm + DOT_OVERLAY_HEIGHT_MM)).abs() < 0.001);
    }

    #[test]
    fn roads_use_smooth_vector_ribbons_one_layer_above_terrain() {
        let mut road_field = SurfaceField::new(3, 3, vec![SurfaceClass::Rock; 9], "roads").unwrap();
        road_field.paint_polyline(
            &[[0.1, 0.25], [0.5, 0.75], [0.9, 0.25]],
            60.0,
            1.0,
            SurfaceClass::Road,
        );
        let height_field = HeightField::new(3, 3, vec![0.0; 9], "flat").unwrap();
        let spec = GenerationSpec {
            width_mm: 60.0,
            rows: 2,
            columns: 2,
            samples_per_piece: 16,
            overlay_samples_per_piece: 32,
            solid_model: true,
            color_output: ColorOutputSpec {
                enabled: true,
                roads_enabled: true,
                road_height_mm: 0.2,
                ..ColorOutputSpec::default()
            },
            ..GenerationSpec::default()
        };
        let raised = build_piece(&spec, Some(&height_field), Some(&road_field), 0, 0).unwrap();
        // Railways and aerialways paint as Road-class lines under their
        // default styles, so "no road overlays" means turning all three off.
        let flat = build_piece(
            &GenerationSpec {
                color_output: ColorOutputSpec {
                    roads_enabled: false,
                    rail_enabled: false,
                    aerial_enabled: false,
                    ..spec.color_output.clone()
                },
                ..spec.clone()
            },
            Some(&height_field),
            Some(&road_field),
            0,
            0,
        )
        .unwrap();
        let road_vertices = raised
            .triangles
            .iter()
            .zip(&raised.materials)
            .filter(|(_, material)| **material == SurfaceClass::Road)
            .flat_map(|(triangle, _)| triangle)
            .map(|index| raised.vertices[*index as usize])
            .collect::<Vec<_>>();
        let minimum_z = road_vertices
            .iter()
            .map(|vertex| vertex[2])
            .fold(f32::INFINITY, f32::min);
        let maximum_z = road_vertices
            .iter()
            .map(|vertex| vertex[2])
            .fold(f32::NEG_INFINITY, f32::max);
        assert!(road_field.vector_lines[0].points_mm.len() > 100);
        assert!(road_vertices.len() > 100);
        assert!((minimum_z - (spec.base_mm - OVERLAY_TERRAIN_EMBED_MM)).abs() < 0.001);
        assert!((maximum_z - (spec.base_mm + spec.color_output.road_height_mm)).abs() < 0.001);
        assert!(!flat.materials.contains(&SurfaceClass::Road.into()));
        assert_watertight(&raised);
    }

    #[test]
    fn roads_yield_to_flag_sockets() {
        let mut field = SurfaceField::new(3, 3, vec![SurfaceClass::Rock; 9], "roads").unwrap();
        field.paint_polyline(&[[0.0, 0.5], [1.0, 0.5]], 60.0, 1.0, SurfaceClass::Road);
        let spec = GenerationSpec {
            width_mm: 60.0,
            rows: 1,
            columns: 1,
            samples_per_piece: 32,
            solid_model: true,
            color_output: ColorOutputSpec {
                enabled: true,
                roads_enabled: true,
                ..ColorOutputSpec::default()
            },
            markers: vec![MapMarker {
                name: "Flag".into(),
                latitude: GenerationSpec::default().center_lat,
                longitude: GenerationSpec::default().center_lon,
                kind: MarkerKind::FlagHole,
                label_height_mm: 4.0,
                rotation_degrees: 0.0,
                dot_style: None,
                flag_style: None,
                label_style: None,
            }],
            ..GenerationSpec::default()
        };

        let mesh = build_piece(&spec, None, Some(&field), 0, 0).unwrap();
        assert_watertight(&mesh);
        let clear_radius = spec.markers[0].flag_style().hole_diameter_mm * 0.5;
        assert!(
            mesh.triangles
                .iter()
                .zip(&mesh.materials)
                .filter(|(_, material)| **material == SurfaceClass::Road)
                .flat_map(|(triangle, _)| triangle)
                .map(|index| mesh.vertices[*index as usize])
                .all(|vertex| (vertex[0] - 30.0).hypot(vertex[1] - 30.0) >= clear_radius)
        );
    }

    #[test]
    fn imported_trails_ride_the_raised_road_treatment() {
        let mut field = SurfaceField::new(3, 3, vec![SurfaceClass::Rock; 9], "trails").unwrap();
        // A road across the piece and a trail crossing it.
        field.paint_polyline(&[[0.1, 0.5], [0.9, 0.5]], 60.0, 1.0, SurfaceClass::Road);
        field.paint_polyline(&[[0.5, 0.1], [0.5, 0.9]], 60.0, 0.7, SurfaceClass::Trail);
        let height_field = HeightField::new(3, 3, vec![0.0; 9], "flat").unwrap();
        let spec = GenerationSpec {
            width_mm: 60.0,
            samples_per_piece: 16,
            overlay_samples_per_piece: 32,
            solid_model: true,
            color_output: ColorOutputSpec {
                enabled: true,
                roads_enabled: true,
                road_height_mm: 0.2,
                ..ColorOutputSpec::default()
            },
            trails: vec![crate::spec::TrailRoute {
                name: "Crossing".into(),
                points: vec![[46.8, -121.8], [46.9, -121.7]],
            }],
            ..GenerationSpec::default()
        };
        let mesh = build_piece(&spec, Some(&height_field), Some(&field), 0, 0).unwrap();
        let trail_vertices = mesh
            .triangles
            .iter()
            .zip(&mesh.materials)
            .filter(|(_, material)| **material == SurfaceClass::Trail)
            .flat_map(|(triangle, _)| triangle)
            .map(|index| mesh.vertices[*index as usize])
            .collect::<Vec<_>>();
        assert!(mesh.materials.contains(&SurfaceClass::Road.into()));
        assert!(trail_vertices.len() > 100);
        let minimum_z = trail_vertices
            .iter()
            .map(|vertex| vertex[2])
            .fold(f32::INFINITY, f32::min);
        let maximum_z = trail_vertices
            .iter()
            .map(|vertex| vertex[2])
            .fold(f32::NEG_INFINITY, f32::max);
        // Same raised layer as roads: embedded bottom, road-height top.
        assert!((minimum_z - (spec.base_mm - OVERLAY_TERRAIN_EMBED_MM)).abs() < 0.001);
        assert!((maximum_z - (spec.base_mm + spec.color_output.road_height_mm)).abs() < 0.001);
        assert_watertight(&mesh);

        // Without spec trails the Trail lines paint nothing and the piece
        // carries no Trail material — the byte-level no-op in mesh form.
        let mut no_trail_spec = spec.clone();
        no_trail_spec.trails.clear();
        let mut road_only = SurfaceField::new(3, 3, vec![SurfaceClass::Rock; 9], "trails").unwrap();
        road_only.paint_polyline(&[[0.1, 0.5], [0.9, 0.5]], 60.0, 1.0, SurfaceClass::Road);
        let plain =
            build_piece(&no_trail_spec, Some(&height_field), Some(&road_only), 0, 0).unwrap();
        assert!(!plain.materials.contains(&SurfaceClass::Trail.into()));
    }

    #[test]
    fn separate_railways_ride_the_raised_road_treatment_beside_roads_and_trails() {
        use crate::spec::RailStyle;

        let mut field = SurfaceField::new(3, 3, vec![SurfaceClass::Rock; 9], "rail").unwrap();
        // A road across the piece, a trail crossing it, and a railway
        // crossing both.
        field.paint_polyline(&[[0.1, 0.5], [0.9, 0.5]], 60.0, 1.0, SurfaceClass::Road);
        field.paint_polyline(&[[0.5, 0.1], [0.5, 0.9]], 60.0, 0.7, SurfaceClass::Trail);
        field.paint_polyline(&[[0.1, 0.2], [0.9, 0.8]], 60.0, 0.7, SurfaceClass::Rail);
        let height_field = HeightField::new(3, 3, vec![0.0; 9], "flat").unwrap();
        let spec = GenerationSpec {
            width_mm: 60.0,
            samples_per_piece: 16,
            overlay_samples_per_piece: 32,
            solid_model: true,
            color_output: ColorOutputSpec {
                enabled: true,
                roads_enabled: true,
                road_height_mm: 0.2,
                rail_enabled: true,
                rail_style: RailStyle::Separate,
                ..ColorOutputSpec::default()
            },
            trails: vec![crate::spec::TrailRoute {
                name: "Crossing".into(),
                points: vec![[46.8, -121.8], [46.9, -121.7]],
            }],
            ..GenerationSpec::default()
        };
        assert!(spec.uses_separate_rail());
        let mesh = build_piece(&spec, Some(&height_field), Some(&field), 0, 0).unwrap();
        let rail_vertices = mesh
            .triangles
            .iter()
            .zip(&mesh.materials)
            .filter(|(_, material)| **material == SurfaceClass::Rail)
            .flat_map(|(triangle, _)| triangle)
            .map(|index| mesh.vertices[*index as usize])
            .collect::<Vec<_>>();
        assert!(mesh.materials.contains(&SurfaceClass::Road.into()));
        assert!(mesh.materials.contains(&SurfaceClass::Trail.into()));
        assert!(rail_vertices.len() > 100);
        let minimum_z = rail_vertices
            .iter()
            .map(|vertex| vertex[2])
            .fold(f32::INFINITY, f32::min);
        let maximum_z = rail_vertices
            .iter()
            .map(|vertex| vertex[2])
            .fold(f32::NEG_INFINITY, f32::max);
        // Same raised layer as roads: embedded bottom, road-height top.
        assert!((minimum_z - (spec.base_mm - OVERLAY_TERRAIN_EMBED_MM)).abs() < 0.001);
        assert!((maximum_z - (spec.base_mm + spec.color_output.road_height_mm)).abs() < 0.001);
        assert_watertight(&mesh);

        // Under the default `with_roads` style no Rail line is ever painted,
        // so a piece carries no Rail material at all.
        let mut with_roads = spec.clone();
        with_roads.color_output.rail_style = RailStyle::WithRoads;
        let mut road_class = SurfaceField::new(3, 3, vec![SurfaceClass::Rock; 9], "rail").unwrap();
        road_class.paint_polyline(&[[0.1, 0.5], [0.9, 0.5]], 60.0, 1.0, SurfaceClass::Road);
        road_class.paint_polyline(&[[0.1, 0.2], [0.9, 0.8]], 60.0, 0.7, SurfaceClass::Road);
        let plain = build_piece(&with_roads, Some(&height_field), Some(&road_class), 0, 0).unwrap();
        assert!(!plain.materials.contains(&SurfaceClass::Rail.into()));
        assert!(plain.materials.contains(&SurfaceClass::Road.into()));
        assert_watertight(&plain);
    }

    /// A ferry is a raised ribbon like every other mapped line, and it is
    /// drawn last, so it yields to the road it meets at a terminal rather
    /// than moving a triangle that was already there.
    #[test]
    fn ferries_ride_the_raised_road_treatment_and_yield_to_what_is_there() {
        use crate::spec::FerryStyle;

        let mut field = SurfaceField::new(3, 3, vec![SurfaceClass::Water; 9], "ferry").unwrap();
        // A road down to the water, and a crossing leaving from it.
        field.paint_polyline(&[[0.1, 0.5], [0.5, 0.5]], 60.0, 1.0, SurfaceClass::Road);
        field.paint_polyline(&[[0.4, 0.5], [0.9, 0.5]], 60.0, 0.7, SurfaceClass::Ferry);
        let height_field = HeightField::new(3, 3, vec![0.0; 9], "flat").unwrap();
        let spec = GenerationSpec {
            width_mm: 60.0,
            samples_per_piece: 16,
            overlay_samples_per_piece: 32,
            solid_model: true,
            color_output: ColorOutputSpec {
                enabled: true,
                roads_enabled: true,
                road_height_mm: 0.2,
                ferry_enabled: true,
                ferry_style: FerryStyle::Separate,
                ..ColorOutputSpec::default()
            },
            ..GenerationSpec::default()
        };
        assert!(spec.uses_separate_ferry());
        let mesh = build_piece(&spec, Some(&height_field), Some(&field), 0, 0).unwrap();
        let ferry_vertices = mesh
            .triangles
            .iter()
            .zip(&mesh.materials)
            .filter(|(_, material)| **material == SurfaceClass::Ferry)
            .flat_map(|(triangle, _)| triangle)
            .map(|index| mesh.vertices[*index as usize])
            .collect::<Vec<_>>();
        assert!(mesh.materials.contains(&SurfaceClass::Road.into()));
        assert!(ferry_vertices.len() > 100);
        let minimum_z = ferry_vertices
            .iter()
            .map(|vertex| vertex[2])
            .fold(f32::INFINITY, f32::min);
        let maximum_z = ferry_vertices
            .iter()
            .map(|vertex| vertex[2])
            .fold(f32::NEG_INFINITY, f32::max);
        // Same raised layer as roads: embedded bottom, road-height top.
        assert!((minimum_z - (spec.base_mm - OVERLAY_TERRAIN_EMBED_MM)).abs() < 0.001);
        assert!((maximum_z - (spec.base_mm + spec.color_output.road_height_mm)).abs() < 0.001);
        assert_watertight(&mesh);

        // Folded into the roads, no Ferry material reaches the piece at all.
        let mut with_roads = spec.clone();
        with_roads.color_output.ferry_style = FerryStyle::WithRoads;
        let mut road_class =
            SurfaceField::new(3, 3, vec![SurfaceClass::Water; 9], "ferry").unwrap();
        road_class.paint_polyline(&[[0.1, 0.5], [0.9, 0.5]], 60.0, 1.0, SurfaceClass::Road);
        let plain = build_piece(&with_roads, Some(&height_field), Some(&road_class), 0, 0).unwrap();
        assert!(!plain.materials.contains(&SurfaceClass::Ferry.into()));
        assert!(plain.materials.contains(&SurfaceClass::Road.into()));
        assert_watertight(&plain);
    }

    /// Railways switch on and off independently of roads, so a model with
    /// the road layer off and railways on must still build overlay
    /// geometry. Under the default `with_roads` style those railways are
    /// Road-class lines, so nothing but the piece gate distinguishes this
    /// from a road-less model — and the gate used to close on it.
    #[test]
    fn rail_only_models_still_build_their_overlay_geometry() {
        use crate::spec::RailStyle;

        let height_field = HeightField::new(3, 3, vec![0.0; 9], "flat").unwrap();
        let spec = |roads_enabled, rail_enabled, rail_style| GenerationSpec {
            width_mm: 60.0,
            samples_per_piece: 16,
            overlay_samples_per_piece: 32,
            solid_model: true,
            color_output: ColorOutputSpec {
                enabled: true,
                roads_enabled,
                rail_enabled,
                rail_style,
                // The aerialway layer follows the railway switch here, so
                // "rail off" really means no rail-family overlay at all.
                aerial_enabled: rail_enabled,
                road_height_mm: 0.2,
                ..ColorOutputSpec::default()
            },
            ..GenerationSpec::default()
        };
        // The field the API produces for roads off, railways on, default
        // style: one Road-class line that is really a railway.
        let mut field = SurfaceField::new(3, 3, vec![SurfaceClass::Rock; 9], "rail-only").unwrap();
        field.paint_polyline(&[[0.1, 0.5], [0.9, 0.5]], 60.0, 1.0, SurfaceClass::Road);

        let rail_only = spec(false, true, RailStyle::WithRoads);
        assert!(rail_only.uses_rail());
        assert!(!rail_only.uses_separate_rail());
        let mesh = build_piece(&rail_only, Some(&height_field), Some(&field), 0, 0).unwrap();
        let raised = mesh
            .triangles
            .iter()
            .zip(&mesh.materials)
            .filter(|(_, material)| **material == SurfaceClass::Road)
            .flat_map(|(triangle, _)| triangle)
            .map(|index| mesh.vertices[*index as usize][2])
            .collect::<Vec<_>>();
        assert!(
            !raised.is_empty(),
            "a rail-only model must draw its railway"
        );
        let maximum = raised.iter().copied().fold(f32::NEG_INFINITY, f32::max);
        assert!(
            (maximum - (rail_only.base_mm + rail_only.color_output.road_height_mm)).abs() < 0.001
        );
        assert_watertight(&mesh);

        // With both layers off the same field draws nothing, which is what
        // makes the case above a real gate and not a no-op.
        let neither = build_piece(
            &spec(false, false, RailStyle::WithRoads),
            Some(&height_field),
            Some(&field),
            0,
            0,
        )
        .unwrap();
        assert!(!neither.materials.contains(&SurfaceClass::Road.into()));
        assert_watertight(&neither);
    }

    #[test]
    fn railway_viaducts_shell_as_elevated_decks_in_the_rail_material() {
        use crate::spec::RailStyle;

        let height_field = HeightField::new(
            3,
            3,
            vec![0.0, 0.0, 0.0, 100.0, 0.0, 100.0, 0.0, 0.0, 0.0],
            "viaduct",
        )
        .unwrap();
        let mut field = SurfaceField::new(3, 3, vec![SurfaceClass::Rock; 9], "viaduct").unwrap();
        field.paint_bridge_polyline_as(
            &[[0.0, 0.5], [1.0, 0.5]],
            60.0,
            1.0,
            [100.0, 100.0],
            SurfaceClass::Rail,
        );
        let spec = GenerationSpec {
            width_mm: 60.0,
            solid_model: true,
            color_output: ColorOutputSpec {
                enabled: true,
                roads_enabled: true,
                rail_enabled: true,
                rail_style: RailStyle::Separate,
                bridge_structure: BridgeStructure::Floating,
                ..ColorOutputSpec::default()
            },
            ..GenerationSpec::default()
        };
        let mesh = build_piece(&spec, Some(&height_field), Some(&field), 0, 0).unwrap();
        let rail_z = mesh
            .triangles
            .iter()
            .zip(&mesh.materials)
            .filter(|(_, material)| **material == SurfaceClass::Rail)
            .flat_map(|(triangle, _)| triangle)
            .map(|index| mesh.vertices[*index as usize][2])
            .collect::<Vec<_>>();
        assert!(!rail_z.is_empty(), "the viaduct must carry the rail color");
        assert!(!mesh.materials.contains(&SurfaceClass::Road.into()));
        let minimum = rail_z.iter().copied().fold(f32::INFINITY, f32::min);
        let maximum = rail_z.iter().copied().fold(f32::NEG_INFINITY, f32::max);
        // A floating deck is exactly its thickness tall and hangs high above
        // the valley floor, like a road bridge.
        assert!((maximum - minimum - spec.color_output.bridge_thickness_mm).abs() < 0.001);
        assert!(minimum > spec.base_mm + spec.relief_mm - 1.1);
        assert_watertight(&mesh);
    }

    #[test]
    fn touching_road_and_rail_bridges_keep_both_materials() {
        use crate::spec::RailStyle;

        let height_field = HeightField::new(3, 3, vec![0.0; 9], "crossing").unwrap();
        let mut field = SurfaceField::new(3, 3, vec![SurfaceClass::Rock; 9], "crossing").unwrap();
        // Roads paint first in the API. At SFO, later BART bridge ribbons
        // cross those road bridges at a height close enough that their
        // footprints form one manifold deck group.
        field.paint_bridge_polyline_as(
            &[[0.1, 0.5], [0.9, 0.5]],
            60.0,
            1.4,
            [0.0, 0.0],
            SurfaceClass::Road,
        );
        field.paint_bridge_polyline_as(
            &[[0.5, 0.1], [0.5, 0.9]],
            60.0,
            0.8,
            [0.0, 0.0],
            SurfaceClass::Rail,
        );
        let spec = GenerationSpec {
            width_mm: 60.0,
            solid_model: true,
            color_output: ColorOutputSpec {
                enabled: true,
                roads_enabled: true,
                rail_enabled: true,
                rail_style: RailStyle::Separate,
                bridge_structure: BridgeStructure::Floating,
                ..ColorOutputSpec::default()
            },
            ..GenerationSpec::default()
        };

        let mesh = build_piece(&spec, Some(&height_field), Some(&field), 0, 0).unwrap();
        assert!(mesh.materials.contains(&SurfaceClass::Road.into()));
        assert!(
            mesh.materials.contains(&SurfaceClass::Rail.into()),
            "the joined deck must not take only the first bridge line's color"
        );
        assert_watertight(&mesh);
    }

    #[test]
    fn trails_keep_running_under_elevated_decks_but_yield_at_touchdowns() {
        let spec = GenerationSpec {
            width_mm: 60.0,
            samples_per_piece: 16,
            overlay_samples_per_piece: 32,
            solid_model: true,
            color_output: ColorOutputSpec {
                enabled: true,
                roads_enabled: true,
                road_height_mm: 0.2,
                ..ColorOutputSpec::default()
            },
            trails: vec![crate::spec::TrailRoute {
                name: "Underpass".into(),
                points: vec![[46.8, -121.8], [46.9, -121.7]],
            }],
            ..GenerationSpec::default()
        };
        // A valley at 0 m with 100 m walls; the tagged bridge spans the
        // valley at 100 m, far above the terrain-following trail.
        let height_field = HeightField::new(
            3,
            3,
            vec![0.0, 0.0, 0.0, 100.0, 0.0, 100.0, 0.0, 0.0, 0.0],
            "flyover",
        )
        .unwrap();
        let build = |deck_elevations: [f32; 2]| {
            let mut field =
                SurfaceField::new(3, 3, vec![SurfaceClass::Rock; 9], "flyover").unwrap();
            field.paint_bridge_polyline(&[[0.0, 0.5], [1.0, 0.5]], 60.0, 1.0, deck_elevations);
            field.paint_polyline(&[[0.5, 0.1], [0.5, 0.9]], 60.0, 0.7, SurfaceClass::Trail);
            build_piece(&spec, Some(&height_field), Some(&field), 0, 0).unwrap()
        };
        // A trail cut at the deck keeps the separation gap clear of the
        // deck strip (y = 29.5..30.5 plus the buffer), so no trail triangle
        // can span y = 30 after a cut; a continuous trail must have some.
        let spans_deck_strip = |mesh: &crate::mesh::Mesh| {
            mesh.triangles
                .iter()
                .zip(&mesh.materials)
                .filter(|(_, material)| **material == SurfaceClass::Trail)
                .any(|(triangle, _)| {
                    let vertices = triangle.map(|index| mesh.vertices[index as usize]);
                    let minimum = vertices.iter().map(|v| v[1]).fold(f32::INFINITY, f32::min);
                    let maximum = vertices
                        .iter()
                        .map(|v| v[1])
                        .fold(f32::NEG_INFINITY, f32::max);
                    minimum < 29.9 && maximum > 30.1
                })
        };

        // Elevated deck (100 m over a 0 m valley): the trail keeps running
        // under the bridge — no gap in the deck strip around y = 30 mm.
        let flyover = build([100.0, 100.0]);
        assert!(
            spans_deck_strip(&flyover),
            "the trail must continue beneath an elevated deck"
        );
        assert_watertight(&flyover);

        // A deck at terrain level (a touchdown) still cuts the trail so the
        // two shells cannot leave coincident faces.
        let touchdown = build([0.0, 0.0]);
        assert!(
            !spans_deck_strip(&touchdown),
            "a deck at trail level must still cut the trail"
        );
        assert_watertight(&touchdown);
    }

    #[test]
    fn polygon_shell_tolerates_repeated_and_overlapping_boundary_edges() {
        let polygon = Polygon::new(
            LineString::new(vec![
                Coord { x: 0.0, y: 0.0 },
                Coord { x: 4.0, y: 0.0 },
                Coord { x: 4.0, y: 4.0 },
                Coord { x: 2.0, y: 4.0 },
                Coord { x: 4.0, y: 4.0 },
                Coord { x: 0.0, y: 4.0 },
                Coord { x: 0.0, y: 0.0 },
            ]),
            vec![],
        );
        let mesh = build_polygon_shell(
            &polygon,
            |_| 1.0,
            |_| 1.2,
            None,
            None,
            SurfaceClass::Road,
            "test repeated boundary",
        )
        .unwrap()
        .finish("Repeated boundary");

        assert!(!mesh.triangles.is_empty());
        assert_watertight(&mesh);
    }

    #[test]
    fn vector_roads_stop_at_enabled_building_footprints() {
        let mut field = SurfaceField::new(3, 3, vec![SurfaceClass::Rock; 9], "roads").unwrap();
        field.paint_polyline(&[[0.1, 0.5], [0.9, 0.5]], 60.0, 1.0, SurfaceClass::Road);
        field.paint_building(&[[0.4, 0.4], [0.6, 0.4], [0.6, 0.6], [0.4, 0.6]], 12.0);
        let spec = GenerationSpec {
            width_mm: 60.0,
            solid_model: true,
            buildings: BuildingSpec {
                enabled: true,
                ..BuildingSpec::default()
            },
            color_output: ColorOutputSpec {
                enabled: true,
                roads_enabled: true,
                ..ColorOutputSpec::default()
            },
            ..GenerationSpec::default()
        };

        let mesh = build_piece(&spec, None, Some(&field), 0, 0).unwrap();
        for (triangle, material) in mesh.triangles.iter().zip(&mesh.materials) {
            if *material != SurfaceClass::Road {
                continue;
            }
            let centroid = triangle
                .map(|index| mesh.vertices[index as usize])
                .iter()
                .fold([0.0, 0.0], |sum, vertex| {
                    [sum[0] + vertex[0] / 3.0, sum[1] + vertex[1] / 3.0]
                });
            assert!(
                !(centroid[0] > 24.0
                    && centroid[0] < 36.0
                    && centroid[1] > 24.0
                    && centroid[1] < 36.0),
                "road triangle entered building at {centroid:?}"
            );
        }
        assert_watertight(&mesh);
    }

    #[test]
    fn tagged_bridge_support_modes_span_a_low_crossing() {
        let height_field = HeightField::new(
            3,
            3,
            vec![0.0, 0.0, 0.0, 100.0, 0.0, 100.0, 0.0, 0.0, 0.0],
            "bridge-test",
        )
        .unwrap();
        let mut bridge_field =
            SurfaceField::new(3, 3, vec![SurfaceClass::Rock; 9], "bridge").unwrap();
        bridge_field.paint_bridge_polyline(&[[0.0, 0.5], [1.0, 0.5]], 60.0, 1.0, [100.0, 100.0]);
        let floating_spec = GenerationSpec {
            width_mm: 60.0,
            rows: 2,
            columns: 2,
            solid_model: true,
            color_output: ColorOutputSpec {
                enabled: true,
                roads_enabled: true,
                bridge_structure: BridgeStructure::Floating,
                bridge_thickness_mm: 1.2,
                ..ColorOutputSpec::default()
            },
            ..GenerationSpec::default()
        };

        let floating = build_piece(
            &floating_spec,
            Some(&height_field),
            Some(&bridge_field),
            0,
            0,
        )
        .unwrap();
        let floating_road_vertices = floating
            .triangles
            .iter()
            .zip(&floating.materials)
            .filter(|(_, material)| **material == SurfaceClass::Road)
            .flat_map(|(triangle, _)| triangle)
            .map(|index| floating.vertices[*index as usize])
            .collect::<Vec<_>>();
        let floating_minimum_z = floating_road_vertices
            .iter()
            .map(|vertex| vertex[2])
            .fold(f32::INFINITY, f32::min);
        let floating_maximum_z = floating_road_vertices
            .iter()
            .map(|vertex| vertex[2])
            .fold(f32::NEG_INFINITY, f32::max);
        assert!(!floating_road_vertices.is_empty());
        assert!(
            (floating_maximum_z
                - floating_minimum_z
                - floating_spec.color_output.bridge_thickness_mm)
                .abs()
                < 0.001
        );
        assert!(floating_minimum_z > floating_spec.base_mm + floating_spec.relief_mm - 1.1);
        assert_watertight(&floating);

        let supported_spec = GenerationSpec {
            color_output: ColorOutputSpec {
                bridge_structure: BridgeStructure::Supported,
                ..floating_spec.color_output.clone()
            },
            ..floating_spec.clone()
        };
        let supported = build_piece(
            &supported_spec,
            Some(&height_field),
            Some(&bridge_field),
            0,
            0,
        )
        .unwrap();
        let supported_road_indices = supported
            .triangles
            .iter()
            .zip(&supported.materials)
            .filter(|(_, material)| **material == SurfaceClass::Road)
            .flat_map(|(triangle, _)| triangle)
            .copied()
            .collect::<HashSet<_>>();
        let terrain_vertex_indices = supported
            .triangles
            .iter()
            .zip(&supported.materials)
            .filter(|(_, material)| **material != SurfaceClass::Road)
            .flat_map(|(triangle, _)| triangle)
            .copied()
            .collect::<HashSet<_>>();
        let supported_minimum_z = supported_road_indices
            .iter()
            .map(|index| supported.vertices[*index as usize][2])
            .fold(f32::INFINITY, f32::min);
        assert!(!supported_road_indices.is_empty());
        assert!(supported_road_indices.is_disjoint(&terrain_vertex_indices));
        assert!(
            (supported_minimum_z - (supported_spec.base_mm - OVERLAY_TERRAIN_EMBED_MM)).abs()
                < 0.01
        );
        let preview = build_preview(&supported_spec, Some(&height_field), Some(&bridge_field), 3);
        assert!(preview["values"][4].as_f64().unwrap() < 0.1);
        assert_watertight(&supported);
    }
}
