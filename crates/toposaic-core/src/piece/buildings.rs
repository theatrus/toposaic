//! Building shells: the per-piece building union, roof partitioning, and
//! the step walls between roof levels.

use std::collections::HashMap;

use anyhow::Result;
use geo::{
    Area, BooleanOps, Centroid, Contains, InteriorPoint, MultiPolygon, Point, Polygon, unary_union,
};
use rayon::prelude::*;
use spade::{ConstrainedDelaunayTriangulation, Point2, Triangulation};

use crate::heightfield::HeightField;
use crate::mesh::{
    Mesh, MeshBuilder, point_in_polygon, quantize_export_coordinate, triangulate_constraints,
};
use crate::spec::{GenerationSpec, SurfaceClass};
use crate::surface::{SurfaceField, VectorSurfaceArea, surface_area_bounds};

use super::{
    BUILDING_GROUND_STEP_MM, MINIMUM_OVERLAY_AREA_MM2, OVERLAY_TERRAIN_EMBED_MM, bounds_overlap,
    geo_polygon, multi_polygon_bounds, polygon_from_rings, repair_classification_pinches,
    retract_pinch_point, ring_signed_area, sanitize_footprint_group, scaled_building_height_mm,
    snapped_open_ring, terrain_z_at, triangulation_face_areas,
};

/// One building footprint clipped to the current piece, with the roof level
/// the whole footprint shares.
struct ClippedBuilding {
    /// Clipped footprint in local piece millimeters.
    footprint: MultiPolygon<f64>,
    /// Local-mm bounding box of the clipped footprint.
    bounds: [f32; 4],
    roof_z: f32,
}

/// Builds all building prisms of one piece as one welded shell per connected
/// group of footprints and returns the unioned footprint area so the road
/// pass can keep clear of it.
///
/// Real map data abuts and overlaps building footprints constantly. Shelling
/// each footprint separately (the previous approach) leaves coincident
/// bottoms and wall quads that a slicer's vertex weld fuses into non-manifold
/// edges and duplicate faces. Unioning the footprints first and shelling each
/// connected component once eliminates every coincident face while keeping
/// each building's own roof height: the component is triangulated with the
/// individual footprint boundaries as interior constraints, every triangle
/// takes the roof of the building containing it (the tallest, where
/// footprints overlap), and interior edges between different roof levels grow
/// vertical step walls, exactly the silhouette the separate shells drew.
#[allow(clippy::too_many_arguments)]
pub(super) fn append_building_geometry(
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
) -> Result<MultiPolygon<f64>> {
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
                bounds[0].min((point[0] + origin_x) / assembled_width),
                bounds[1].min((point[1] + origin_y) / assembled_height),
                bounds[2].max((point[0] + origin_x) / assembled_width),
                bounds[3].max((point[1] + origin_y) / assembled_height),
            ]
        },
    );
    let candidates = surface_field
        .vector_areas
        .iter()
        .filter(|area| area.building_height_m > 0.0 && area.points.len() >= 3)
        .filter(|area| bounds_overlap(surface_area_bounds(&area.points), piece_bounds))
        .collect::<Vec<_>>();
    let clipped_buildings = candidates
        .par_iter()
        .map(|building| {
            let local_points = building
                .points
                .iter()
                .map(|point| {
                    [
                        point[0] * assembled_width - origin_x,
                        point[1] * assembled_height - origin_y,
                    ]
                })
                .collect::<Vec<_>>();
            let clipped = geo_polygon(&local_points).intersection(&piece_polygon);
            let footprint = MultiPolygon(
                clipped
                    .0
                    .into_iter()
                    .filter(|polygon| polygon.unsigned_area() > MINIMUM_OVERLAY_AREA_MM2)
                    .collect::<Vec<_>>(),
            );
            if footprint.0.is_empty() {
                return None;
            }
            let bounds = multi_polygon_bounds(&footprint);
            let roof_z = building_roof_z(
                spec,
                building,
                height_field,
                height_range,
                assembled_width,
                assembled_height,
            );
            Some(ClippedBuilding {
                footprint,
                bounds,
                roof_z,
            })
        })
        .collect::<Vec<_>>()
        .into_iter()
        .flatten()
        .collect::<Vec<_>>();
    if clipped_buildings.is_empty() {
        return Ok(MultiPolygon(Vec::new()));
    }
    let footprint_union = sanitize_footprint_group(
        unary_union(clipped_buildings.iter().map(|building| &building.footprint)),
        false,
    );
    let bottom = |point: [f32; 2]| {
        terrain_z_at(
            spec,
            height_field,
            height_range,
            (point[0] + origin_x) / assembled_width,
            (point[1] + origin_y) / assembled_height,
        ) - OVERLAY_TERRAIN_EMBED_MM
    };
    // Every building belongs to exactly one union component. Exclusive
    // membership matters: a component's shell keeps only faces covered by
    // its own members, so no two components can ever emit geometry over the
    // same spot even when coordinate rounding nudges their outlines.
    let mut component_members: Vec<Vec<&ClippedBuilding>> =
        vec![Vec::new(); footprint_union.0.len()];
    let component_bounds = footprint_union
        .0
        .iter()
        .map(polygon_bounds)
        .collect::<Vec<_>>();
    for building in &clipped_buildings {
        let anchor = building.footprint.interior_point();
        let component = anchor.and_then(|anchor| {
            let anchor_bounds = [anchor.x() as f32, anchor.y() as f32];
            footprint_union.0.iter().position(|component| {
                point_in_bounds(anchor_bounds, polygon_bounds(component))
                    && component.contains(&anchor)
            })
        });
        match component {
            Some(component) => component_members[component].push(building),
            None => {
                // The component this building fed vanished or shrank in
                // sanitizing; give the building to every nearby component
                // rather than dropping it.
                for (index, bounds) in component_bounds.iter().enumerate() {
                    if bounds_overlap(building.bounds, *bounds) {
                        component_members[index].push(building);
                    }
                }
            }
        }
    }
    // Components are independent, so shell them in parallel and append in
    // the union's stable output order.
    let shells = footprint_union
        .0
        .par_iter()
        .zip(&component_members)
        .map(|(component, members)| -> Result<MeshBuilder> {
            if component.unsigned_area() <= MINIMUM_OVERLAY_AREA_MM2 || members.is_empty() {
                return Ok(MeshBuilder::default());
            }
            let component_rings = std::iter::once(component.exterior())
                .chain(component.interiors())
                .map(snapped_open_ring)
                .filter(|ring| ring.len() >= 3)
                .collect::<Vec<_>>();
            let Some(snapped_component) = polygon_from_rings(&component_rings) else {
                return Ok(MeshBuilder::default());
            };
            let mut constraint_rings = component_rings;
            let outline_ring_count = constraint_rings.len();
            for member in members.iter() {
                for polygon in &member.footprint.0 {
                    constraint_rings.extend(
                        std::iter::once(polygon.exterior())
                            .chain(polygon.interiors())
                            .map(snapped_open_ring)
                            .filter(|ring| ring.len() >= 3),
                    );
                }
            }
            retract_isolated_member_contacts(&mut constraint_rings, outline_ring_count);
            build_building_union_shell(&snapped_component, constraint_rings, members, &bottom)
        })
        .collect::<Result<Vec<_>>>()?;
    for shell in shells {
        mesh.append_isolated(shell);
    }
    Ok(footprint_union)
}

/// Separates member footprints that touch each other (or the union outline)
/// at a single point, like two towers meeting corner-to-corner on a shared
/// podium. Around such a point the roof partition alternates high/low, which
/// stacks four step walls on one vertical edge. A repeated point whose two
/// incident segments are shared with another ring is a normal abutting wall
/// and stays; a repeated point with unshared segments is an isolated contact
/// and retracts a few microns into its own footprint, so the tops separate
/// through a sliver at the surrounding roof level.
fn retract_isolated_member_contacts(
    constraint_rings: &mut [Vec<[f32; 2]>],
    outline_ring_count: usize,
) {
    let point_key = |point: [f32; 2]| [point[0].to_bits(), point[1].to_bits()];
    let segment_key = |a: [f32; 2], b: [f32; 2]| {
        let (a, b) = (point_key(a), point_key(b));
        if a <= b { (a, b) } else { (b, a) }
    };
    let mut point_counts = HashMap::<[u32; 2], u32>::new();
    let mut segment_counts = HashMap::<([u32; 2], [u32; 2]), u32>::new();
    for ring in constraint_rings.iter() {
        for (index, point) in ring.iter().enumerate() {
            *point_counts.entry(point_key(*point)).or_default() += 1;
            let next = ring[(index + 1) % ring.len()];
            *segment_counts.entry(segment_key(*point, next)).or_default() += 1;
        }
    }
    for ring in constraint_rings.iter_mut().skip(outline_ring_count) {
        let orientation = ring_signed_area(ring).signum() as f32;
        let original = ring.clone();
        for (index, point) in original.iter().enumerate() {
            if point_counts[&point_key(*point)] < 2 {
                continue;
            }
            let previous = original[(index + original.len() - 1) % original.len()];
            let next = original[(index + 1) % original.len()];
            if segment_counts[&segment_key(previous, *point)] > 1
                || segment_counts[&segment_key(*point, next)] > 1
            {
                continue;
            }
            if let Some(moved) = retract_pinch_point(*point, previous, next, orientation) {
                ring[index] = moved;
            }
        }
    }
}

/// Roof level for one triangulated footprint face: the tallest member
/// building whose footprint contains the face centroid, or `None` when no
/// member covers it — such a face lies outside this component's buildings
/// and stays out of the shell.
fn face_roof_z(members: &[&ClippedBuilding], centroid: [f32; 2]) -> Option<f32> {
    let centroid_point = Point::new(f64::from(centroid[0]), f64::from(centroid[1]));
    let mut roof: Option<f32> = None;
    for member in members {
        if point_in_bounds(centroid, member.bounds) && member.footprint.contains(&centroid_point) {
            roof = Some(roof.map_or(member.roof_z, |best| best.max(member.roof_z)));
        }
    }
    roof
}

/// Removes pinches from the roof partition of one building union shell.
///
/// Around every triangulation vertex, the walls that will exist there are
/// known: a step wall between each pair of neighboring kept faces with
/// different roofs, and a full wall wherever a kept face meets a dropped
/// one. When those wall spans stack more than two deep on the vertex's
/// vertical line — roofs alternating high/low around the vertex, as when
/// two footprints of different heights meet corner-to-corner inside the
/// union — the shell would be non-manifold there. The smallest offending
/// roof run merges into the neighboring run with the nearest roof level,
/// sweeping until stable; only sliver-scale corner triangles change height.
fn smooth_roof_partition(
    triangulation: &ConstrainedDelaunayTriangulation<Point2<f64>>,
    inside: &[bool],
    face_roofs: &mut [f32],
    bottom: &impl Fn([f32; 2]) -> f32,
) {
    let areas = triangulation_face_areas(triangulation);
    for _sweep in 0..8 {
        let mut changed = false;
        for vertex in triangulation.vertices() {
            let faces = vertex
                .out_edges()
                .map(|edge| edge.face().fix().index())
                .collect::<Vec<_>>();
            if faces.len() < 3 {
                continue;
            }
            let position = vertex.position();
            let floor = bottom([position.x as f32, position.y as f32]);
            // Wall spans on this vertex's vertical line.
            let mut spans = Vec::<(f32, f32)>::new();
            for index in 0..faces.len() {
                let current = faces[index];
                let next = faces[(index + 1) % faces.len()];
                match (inside[current], inside[next]) {
                    (true, true) => {
                        let (low, high) = (
                            face_roofs[current].min(face_roofs[next]),
                            face_roofs[current].max(face_roofs[next]),
                        );
                        if low < high {
                            spans.push((low, high));
                        }
                    }
                    (true, false) => spans.push((floor, face_roofs[current])),
                    (false, true) => spans.push((floor, face_roofs[next])),
                    (false, false) => {}
                }
            }
            if spans.len() <= 2 {
                continue;
            }
            // Sweep the spans; more than two walls over any height is a
            // pinch.
            let mut events = Vec::with_capacity(spans.len() * 2);
            for (low, high) in &spans {
                events.push((*low, 1));
                events.push((*high, -1));
            }
            events.sort_by(|(left, left_step), (right, right_step)| {
                left.total_cmp(right).then(left_step.cmp(right_step))
            });
            let mut depth = 0;
            let mut pinched = false;
            for (_, step) in &events {
                depth += step;
                if depth > 2 {
                    pinched = true;
                    break;
                }
            }
            if !pinched {
                continue;
            }
            // Roof runs around the vertex: contiguous kept faces sharing a
            // roof, broken at dropped faces. Merge the smallest run into
            // the neighboring run with the nearest roof.
            let start = (0..faces.len())
                .find(|index| {
                    let previous = faces[(index + faces.len() - 1) % faces.len()];
                    let current = faces[*index];
                    inside[current]
                        && (!inside[previous] || face_roofs[previous] != face_roofs[current])
                })
                .unwrap_or(0);
            let mut runs: Vec<Option<(f64, f32, Vec<usize>)>> = Vec::new();
            for offset in 0..faces.len() {
                let face = faces[(start + offset) % faces.len()];
                if !inside[face] {
                    if !matches!(runs.last(), Some(None) | None) {
                        runs.push(None);
                    }
                    continue;
                }
                match runs.last_mut() {
                    Some(Some((area, roof, members))) if *roof == face_roofs[face] => {
                        *area += areas[face];
                        members.push(face);
                    }
                    _ => runs.push(Some((areas[face], face_roofs[face], vec![face]))),
                }
            }
            let run_count = runs.len();
            let smallest = runs
                .iter()
                .enumerate()
                .filter_map(|(index, run)| run.as_ref().map(|run| (index, run)))
                .filter(|(index, (_, roof, _))| {
                    // Only merge runs that have a kept neighbor run to
                    // merge into.
                    let neighbors = [
                        (*index + run_count - 1) % run_count,
                        (*index + 1) % run_count,
                    ];
                    neighbors.iter().any(|neighbor| {
                        *neighbor != *index
                            && runs[*neighbor]
                                .as_ref()
                                .is_some_and(|(_, other, _)| other != roof)
                    })
                })
                .min_by(|(_, (left, ..)), (_, (right, ..))| left.total_cmp(right))
                .map(|(index, _)| index);
            let Some(smallest) = smallest else {
                continue;
            };
            let (_, roof, members) = runs[smallest].clone().expect("smallest run is kept");
            let neighbor_roofs = [
                (smallest + run_count - 1) % run_count,
                (smallest + 1) % run_count,
            ]
            .into_iter()
            .filter(|neighbor| *neighbor != smallest)
            .filter_map(|neighbor| runs[neighbor].as_ref())
            .map(|(_, other, _)| *other)
            .filter(|other| *other != roof)
            .collect::<Vec<_>>();
            let Some(new_roof) = neighbor_roofs.into_iter().min_by(|left, right| {
                (left - roof)
                    .abs()
                    .total_cmp(&(right - roof).abs())
                    .then(left.total_cmp(right))
            }) else {
                continue;
            };
            for face in members {
                face_roofs[face] = new_roof;
            }
            changed = true;
        }
        if !changed {
            break;
        }
    }
}

/// Triangulates one connected component of the building union — with every
/// member footprint boundary as an interior constraint — and closes it into a
/// single watertight shell: per-face roofs on top, terrain-following bottom,
/// full-height walls on the outline, and vertical step walls where two roof
/// levels meet inside the component.
fn build_building_union_shell(
    component: &Polygon<f64>,
    constraint_rings: Vec<Vec<[f32; 2]>>,
    members: &[&ClippedBuilding],
    bottom: &impl Fn([f32; 2]) -> f32,
) -> Result<MeshBuilder> {
    let mut points = Vec::new();
    let mut constraints = Vec::new();
    for ring in &constraint_rings {
        let start = points.len();
        points.extend(
            ring.iter()
                .map(|point| Point2::new(f64::from(point[0]), f64::from(point[1]))),
        );
        constraints
            .extend((0..ring.len()).map(|index| [start + index, start + (index + 1) % ring.len()]));
    }
    if points.len() < 3 {
        return Ok(MeshBuilder::default());
    }
    let triangulation =
        triangulate_constraints(points, constraints, "triangulate building union footprint")?;
    // Classify: a face belongs to the shell when its centroid sits in the
    // component. Repair any classification pinch, then assign each kept
    // face its roof and smooth away pinches in the roof partition itself.
    let mut inside = vec![false; triangulation.num_all_faces()];
    let mut face_roofs = vec![f32::NAN; triangulation.num_all_faces()];
    for face in triangulation.inner_faces() {
        let positions = face.vertices().map(|vertex| vertex.position());
        let centroid = Point::new(
            (positions[0].x + positions[1].x + positions[2].x) / 3.0,
            (positions[0].y + positions[1].y + positions[2].y) / 3.0,
        );
        if !component.contains(&centroid) {
            continue;
        }
        let Some(roof) = face_roof_z(members, [centroid.x() as f32, centroid.y() as f32]) else {
            continue;
        };
        let index = face.fix().index();
        inside[index] = true;
        face_roofs[index] = roof;
    }
    repair_classification_pinches(&triangulation, &mut inside, false);
    smooth_roof_partition(&triangulation, &inside, &mut face_roofs, bottom);
    let mut output = MeshBuilder::default();
    let mut edge_faces = HashMap::<(usize, usize), Vec<([usize; 2], f32)>>::new();
    let mut vertex_positions = HashMap::<usize, [f32; 2]>::new();
    // Every roof level that appears on a vertex. Wall quads sharing that
    // vertex's vertical line must subdivide at these levels, or a short
    // neighbor's wall corner becomes a T-junction (an open edge after weld)
    // against a taller neighbor's unbroken vertical side.
    let mut vertex_roofs = HashMap::<usize, Vec<f32>>::new();
    for face in triangulation.inner_faces() {
        if !inside[face.fix().index()] {
            continue;
        }
        let face_vertices = face.vertices();
        let face_points = face_vertices.map(|vertex| {
            let point = vertex.position();
            [point.x as f32, point.y as f32]
        });
        let roof = face_roofs[face.fix().index()];
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
            let roofs = vertex_roofs.entry(index).or_default();
            if !roofs.contains(&roof) {
                roofs.push(roof);
            }
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
            edge_faces.entry(key).or_default().push((directed, roof));
        }
        output.triangle(
            [ordered[0][0], ordered[0][1], roof],
            [ordered[1][0], ordered[1][1], roof],
            [ordered[2][0], ordered[2][1], roof],
            SurfaceClass::Building,
        );
        output.triangle(
            [ordered[0][0], ordered[0][1], bottom(ordered[0])],
            [ordered[2][0], ordered[2][1], bottom(ordered[2])],
            [ordered[1][0], ordered[1][1], bottom(ordered[1])],
            SurfaceClass::Building,
        );
    }
    for roofs in vertex_roofs.values_mut() {
        roofs.sort_by(f32::total_cmp);
    }
    // Emits a wall face for the directed footprint edge `from -> to`
    // spanning `low_z(vertex)..high_z` on both vertical sides, splitting
    // each side at every other roof level its vertex carries so neighboring
    // walls of different heights share complete edges instead of forming
    // T-junctions.
    let emit_wall = |output: &mut MeshBuilder,
                     from: usize,
                     to: usize,
                     low_z: &dyn Fn([f32; 2]) -> f32,
                     high_z: f32| {
        let side = |index: usize| {
            let point = vertex_positions[&index];
            let floor = low_z(point);
            let mut levels = vec![floor];
            levels.extend(
                vertex_roofs
                    .get(&index)
                    .into_iter()
                    .flatten()
                    .copied()
                    .filter(|level| *level > floor && *level < high_z),
            );
            levels.push(high_z);
            (point, levels)
        };
        let (start, left) = side(from);
        let (end, right) = side(to);
        // Ladder triangulation between the two ascending vertical sides,
        // oriented like the plain wall quad it generalizes.
        let mut i = 0;
        let mut j = 0;
        while i + 1 < left.len() || j + 1 < right.len() {
            let advance_right =
                j + 1 < right.len() && (i + 1 >= left.len() || right[j + 1] <= left[i + 1]);
            if advance_right {
                output.triangle(
                    [start[0], start[1], left[i]],
                    [end[0], end[1], right[j]],
                    [end[0], end[1], right[j + 1]],
                    SurfaceClass::Building,
                );
                j += 1;
            } else {
                output.triangle(
                    [start[0], start[1], left[i]],
                    [end[0], end[1], right[j]],
                    [start[0], start[1], left[i + 1]],
                    SurfaceClass::Building,
                );
                i += 1;
            }
        }
    };
    // Sorted for the same run-to-run reproducibility as the terrain walls.
    let mut edges = edge_faces.into_iter().collect::<Vec<_>>();
    edges.sort_unstable_by_key(|(key, _)| *key);
    for (_, faces) in edges {
        match faces.as_slice() {
            [([from, to], roof)] => {
                emit_wall(&mut output, *from, *to, &bottom, *roof);
            }
            [first, second] => {
                if first.1 == second.1 {
                    continue;
                }
                let (high, low) = if first.1 > second.1 {
                    (first, second)
                } else {
                    (second, first)
                };
                let [from, to] = high.0;
                let low_roof = low.1;
                emit_wall(&mut output, from, to, &move |_point| low_roof, high.1);
            }
            _ => {}
        }
    }
    Ok(output)
}

fn point_in_bounds(point: [f32; 2], bounds: [f32; 4]) -> bool {
    point[0] >= bounds[0] && point[0] <= bounds[2] && point[1] >= bounds[1] && point[1] <= bounds[3]
}

fn polygon_bounds(polygon: &Polygon<f64>) -> [f32; 4] {
    let mut bounds = [
        f32::INFINITY,
        f32::INFINITY,
        f32::NEG_INFINITY,
        f32::NEG_INFINITY,
    ];
    for point in &polygon.exterior().0 {
        bounds[0] = bounds[0].min(point.x as f32);
        bounds[1] = bounds[1].min(point.y as f32);
        bounds[2] = bounds[2].max(point.x as f32);
        bounds[3] = bounds[3].max(point.y as f32);
    }
    bounds
}

fn building_roof_z(
    spec: &GenerationSpec,
    building: &VectorSurfaceArea,
    height_field: Option<&HeightField>,
    height_range: Option<(f32, f32)>,
    assembled_width: f32,
    assembled_height: f32,
) -> f32 {
    let centroid = geo_polygon(&building.points)
        .centroid()
        .map(|point| [point.x() as f32, point.y() as f32])
        .unwrap_or(building.points[0]);
    let mut ground_z = terrain_z_at(spec, height_field, height_range, centroid[0], centroid[1]);
    for (start, end) in building
        .points
        .iter()
        .zip(building.points.iter().cycle().skip(1))
    {
        let length_mm =
            ((end[0] - start[0]) * assembled_width).hypot((end[1] - start[1]) * assembled_height);
        let sample_count = (length_mm / BUILDING_GROUND_STEP_MM).ceil().max(1.0) as usize;
        for sample in 0..sample_count {
            let amount = sample as f32 / sample_count as f32;
            let point = [
                start[0] + (end[0] - start[0]) * amount,
                start[1] + (end[1] - start[1]) * amount,
            ];
            ground_z = ground_z.max(terrain_z_at(
                spec,
                height_field,
                height_range,
                point[0],
                point[1],
            ));
        }
    }
    if let Some(height_field) = height_field {
        let bounds = surface_area_bounds(&building.points);
        let minimum_x =
            (bounds[0].clamp(0.0, 1.0) * (height_field.width - 1) as f32).floor() as usize;
        let maximum_x =
            (bounds[2].clamp(0.0, 1.0) * (height_field.width - 1) as f32).ceil() as usize;
        let minimum_y =
            (bounds[1].clamp(0.0, 1.0) * (height_field.height - 1) as f32).floor() as usize;
        let maximum_y =
            (bounds[3].clamp(0.0, 1.0) * (height_field.height - 1) as f32).ceil() as usize;
        for y in minimum_y..=maximum_y {
            for x in minimum_x..=maximum_x {
                let point = [
                    x as f32 / (height_field.width - 1) as f32,
                    y as f32 / (height_field.height - 1) as f32,
                ];
                if point_in_polygon(point, &building.points) {
                    ground_z = ground_z.max(terrain_z_at(
                        spec,
                        Some(height_field),
                        height_range,
                        point[0],
                        point[1],
                    ));
                }
            }
        }
    }
    // Snapped to the export grid: two roofs closer than the grid would emit
    // step walls thinner than the vertex weld can represent.
    quantize_export_coordinate(
        ground_z + scaled_building_height_mm(spec, building.building_height_m),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::{HashMap, HashSet};

    use crate::mesh::assert_watertight;
    use crate::piece::build_piece;
    use crate::spec::{BuildingSpec, ColorOutputSpec};

    #[test]
    fn building_solids_keep_exact_straight_walls_and_flat_roofs() {
        let mut field = SurfaceField::new(5, 5, vec![SurfaceClass::Rock; 25], "buildings").unwrap();
        field.paint_building(&[[0.4, 0.4], [0.6, 0.4], [0.6, 0.6], [0.4, 0.6]], 12.0);
        let height = HeightField::new(
            3,
            3,
            vec![0.0, 0.0, 0.0, 0.0, 100.0, 0.0, 0.0, 0.0, 0.0],
            "peak",
        )
        .unwrap();
        let spec = GenerationSpec {
            width_mm: 100.0,
            rows: 1,
            columns: 1,
            samples_per_piece: 32,
            overlay_samples_per_piece: 32,
            solid_model: true,
            buildings: BuildingSpec {
                enabled: true,
                z_scale: 2.0,
            },
            ..GenerationSpec::default()
        };
        let mesh = build_piece(&spec, Some(&height), Some(&field), 0, 0).unwrap();
        let building_indices = mesh
            .triangles
            .iter()
            .zip(&mesh.materials)
            .filter(|(_, material)| **material == SurfaceClass::Building)
            .flat_map(|(triangle, _)| triangle)
            .copied()
            .collect::<HashSet<_>>();
        let terrain_indices = mesh
            .triangles
            .iter()
            .zip(&mesh.materials)
            .filter(|(_, material)| **material != SurfaceClass::Building)
            .flat_map(|(triangle, _)| triangle)
            .copied()
            .collect::<HashSet<_>>();
        assert!(!building_indices.is_empty());
        assert!(building_indices.is_disjoint(&terrain_indices));

        let mut wall_levels = HashMap::<(i32, i32), Vec<f32>>::new();
        for index in building_indices {
            let vertex = mesh.vertices[index as usize];
            assert!(
                (vertex[0] - 40.0).abs() < 0.001
                    || (vertex[0] - 60.0).abs() < 0.001
                    || (vertex[1] - 40.0).abs() < 0.001
                    || (vertex[1] - 60.0).abs() < 0.001,
                "building vertex left its exact footprint: {vertex:?}"
            );
            wall_levels
                .entry((
                    (vertex[0] * 1_000.0).round() as i32,
                    (vertex[1] * 1_000.0).round() as i32,
                ))
                .or_default()
                .push(vertex[2]);
        }
        let mut roof_levels = Vec::new();
        for levels in wall_levels.values_mut() {
            levels.sort_by(f32::total_cmp);
            levels.dedup_by(|left, right| (*left - *right).abs() < 0.000_1);
            assert_eq!(levels.len(), 2, "wall vertex did not form a vertical pair");
            roof_levels.push(levels[1]);
        }
        assert!(
            roof_levels
                .windows(2)
                .all(|pair| (pair[0] - pair[1]).abs() < 0.000_1)
        );
        let expected_roof = spec.base_mm + spec.relief_mm + scaled_building_height_mm(&spec, 12.0);
        assert!((roof_levels[0] - expected_roof).abs() < 0.000_1);
        assert_watertight(&mesh);
    }

    #[test]
    fn building_solids_clip_cleanly_at_piece_edges() {
        let mut field = SurfaceField::new(5, 5, vec![SurfaceClass::Rock; 25], "buildings").unwrap();
        field.paint_building(&[[0.45, 0.1], [0.55, 0.1], [0.55, 0.9], [0.45, 0.9]], 24.0);
        let flat_height = HeightField {
            width: 2,
            height: 2,
            values_m: vec![0.0; 4],
            source: "flat".into(),
        };
        let spec = GenerationSpec {
            width_mm: 100.0,
            ground_span_km: 1.0,
            rows: 2,
            columns: 2,
            samples_per_piece: 32,
            overlay_samples_per_piece: 32,
            buildings: BuildingSpec {
                enabled: true,
                z_scale: 2.0,
            },
            ..GenerationSpec::default()
        };

        for row in 0..spec.rows {
            for column in 0..spec.columns {
                let mesh =
                    build_piece(&spec, Some(&flat_height), Some(&field), row, column).unwrap();
                let building_indices = mesh
                    .triangles
                    .iter()
                    .zip(&mesh.materials)
                    .filter(|(_, material)| **material == SurfaceClass::Building)
                    .flat_map(|(triangle, _)| triangle)
                    .copied()
                    .collect::<HashSet<_>>();
                let terrain_indices = mesh
                    .triangles
                    .iter()
                    .zip(&mesh.materials)
                    .filter(|(_, material)| **material != SurfaceClass::Building)
                    .flat_map(|(triangle, _)| triangle)
                    .copied()
                    .collect::<HashSet<_>>();
                assert!(
                    !building_indices.is_empty(),
                    "missing building in piece {row}-{column}"
                );
                assert!(building_indices.is_disjoint(&terrain_indices));
                let roof_z = building_indices
                    .iter()
                    .map(|index| mesh.vertices[*index as usize][2])
                    .fold(f32::NEG_INFINITY, f32::max);
                let roof_vertices = building_indices
                    .iter()
                    .filter(|index| (mesh.vertices[**index as usize][2] - roof_z).abs() < 0.000_1)
                    .count();
                assert!(roof_vertices >= 3);
                assert_watertight(&mesh);
            }
        }
    }

    #[test]
    fn abutting_buildings_and_clipped_roads_stay_manifold_after_welding() {
        // Regression for the coincident-shell defect family: buildings that
        // share a wall (equal and unequal heights), a building pair meeting
        // corner-to-corner, and a road clipped against those outlines used
        // to emit per-feature shells whose identical bottoms and wall quads
        // fused into 4-use edges and duplicate faces once a slicer welded
        // vertices.
        let mut field = SurfaceField::new(5, 5, vec![SurfaceClass::Rock; 25], "abutting").unwrap();
        // Two buildings sharing the x = 0.5 wall at different heights.
        field.paint_building(&[[0.3, 0.3], [0.5, 0.3], [0.5, 0.5], [0.3, 0.5]], 30.0);
        field.paint_building(&[[0.5, 0.3], [0.7, 0.3], [0.7, 0.5], [0.5, 0.5]], 12.0);
        // Two buildings sharing a wall at the same height.
        field.paint_building(&[[0.3, 0.6], [0.4, 0.6], [0.4, 0.7], [0.3, 0.7]], 18.0);
        field.paint_building(&[[0.4, 0.6], [0.5, 0.6], [0.5, 0.7], [0.4, 0.7]], 18.0);
        // Two buildings meeting at a single corner point.
        field.paint_building(&[[0.6, 0.6], [0.65, 0.6], [0.65, 0.65], [0.6, 0.65]], 24.0);
        field.paint_building(&[[0.65, 0.65], [0.7, 0.65], [0.7, 0.7], [0.65, 0.7]], 9.0);
        // A road running straight through the buildings, so its ribbon is
        // clipped against their outlines.
        field.paint_polyline(&[[0.1, 0.4], [0.9, 0.4]], 60.0, 1.5, SurfaceClass::Road);
        let height = HeightField::new(2, 2, vec![0.0; 4], "flat").unwrap();
        let spec = GenerationSpec {
            width_mm: 60.0,
            ground_span_km: 1.0,
            solid_model: true,
            buildings: BuildingSpec {
                enabled: true,
                z_scale: 2.0,
            },
            color_output: ColorOutputSpec {
                enabled: true,
                roads_enabled: true,
                ..ColorOutputSpec::default()
            },
            ..GenerationSpec::default()
        };

        let mesh = build_piece(&spec, Some(&height), Some(&field), 0, 0).unwrap();
        assert!(mesh.materials.contains(&SurfaceClass::Building));
        assert!(mesh.materials.contains(&SurfaceClass::Road));
        // Both roof levels of the unequal pair survive the union.
        let building_tops = mesh
            .triangles
            .iter()
            .zip(&mesh.materials)
            .filter(|(_, material)| **material == SurfaceClass::Building)
            .flat_map(|(triangle, _)| triangle)
            .map(|index| (mesh.vertices[*index as usize][2] * 1_000.0).round() as i32)
            .collect::<HashSet<_>>();
        for height_m in [30.0_f32, 12.0] {
            let expected = spec.base_mm + scaled_building_height_mm(&spec, height_m);
            assert!(
                building_tops.contains(&((expected * 1_000.0).round() as i32)),
                "missing roof level for {height_m} m building"
            );
        }
        assert_watertight(&mesh);
    }

    #[test]
    fn buildings_raise_the_printed_mesh() {
        let mut field = SurfaceField::new(3, 3, vec![SurfaceClass::Rock; 9], "buildings").unwrap();
        field.paint_building(&[[0.2, 0.2], [0.8, 0.2], [0.8, 0.8], [0.2, 0.8]], 12.0);
        let height = HeightField::new(2, 2, vec![0.0; 4], "flat").unwrap();
        let spec = GenerationSpec {
            width_mm: 60.0,
            rows: 2,
            columns: 2,
            samples_per_piece: 16,
            ground_span_km: 1.0,
            solid_model: true,
            buildings: BuildingSpec {
                enabled: true,
                z_scale: 2.0,
            },
            ..GenerationSpec::default()
        };
        let raised = build_piece(&spec, Some(&height), Some(&field), 0, 0).unwrap();
        assert!(raised.materials.contains(&SurfaceClass::Building));
        let flat = build_piece(
            &GenerationSpec {
                buildings: BuildingSpec {
                    enabled: false,
                    ..spec.buildings.clone()
                },
                ..spec.clone()
            },
            Some(&height),
            Some(&field),
            0,
            0,
        )
        .unwrap();
        let raised_top = raised
            .vertices
            .iter()
            .map(|vertex| vertex[2])
            .fold(f32::NEG_INFINITY, f32::max);
        let flat_top = flat
            .vertices
            .iter()
            .map(|vertex| vertex[2])
            .fold(f32::NEG_INFINITY, f32::max);
        assert!((raised_top - flat_top - 1.44).abs() < 0.001);
    }
}
