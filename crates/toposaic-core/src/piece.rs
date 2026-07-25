use std::collections::HashMap;

use anyhow::{Context, Result, bail};
use geo::{
    Area, BooleanOps, Buffer, Centroid, Contains, Coord, InteriorPoint, LineString, MultiPolygon,
    Point, Polygon, unary_union,
};
use rayon::prelude::*;
use spade::{ConstrainedDelaunayTriangulation, Point2, Triangulation};

#[cfg(test)]
use crate::heightfield::height_range_for_spec;
use crate::heightfield::{HeightField, normalized_height};
use crate::jigsaw::{EdgePattern, edge_noise, edge_sign, puzzle_edge_point, shared_edge_pattern};
use crate::mesh::{
    Mesh, MeshBuilder, PolygonStripIndex, distance_squared, point_in_polygon, point_line_distance,
    quantize_export_coordinate, triangulate_constraints, unit_vector, weld_export_mesh,
};
use crate::spec::{BridgeStructure, GenerationSpec, SurfaceClass};
use crate::surface::{
    ROAD_VECTOR_STEP_MM, SurfaceField, VectorSurfaceArea, VectorSurfaceLine, surface_area_bounds,
    surface_line_progress,
};
use crate::tray::{add_triangle_contour_segment, smooth_contour_path, stitch_contour_segments};

const OVERLAY_TERRAIN_EMBED_MM: f32 = 0.02;
const BUILDING_GROUND_STEP_MM: f32 = 0.25;
/// Clearance kept between road shells and building shells. Without it a road
/// clipped against a building outline shares that outline's exact coordinates
/// with the building shell, and both shells' embedded bottoms sit at the same
/// depth, so a slicer's vertex weld fuses the two solids along those edges
/// into non-manifold four-face edges. Five microns is far below print
/// resolution but far above the 1e-5 mm export grid.
const OVERLAY_SEPARATION_MM: f64 = 0.005;
/// Overlay footprint fragments below this area (mm^2) are unprintable dust
/// left over from boolean clipping and are dropped before shelling.
const MINIMUM_OVERLAY_AREA_MM2: f64 = 0.000_01;

#[allow(clippy::too_many_arguments)]
fn add_forest_boundary_points(
    points: &mut Vec<Point2<f64>>,
    point_keys: &mut HashMap<(i64, i64), usize>,
    field: &SurfaceField,
    outline: &[[f32; 2]],
    origin_x: f32,
    origin_y: f32,
    assembled_width: f32,
    assembled_height: f32,
    spacing: f32,
) -> usize {
    let bounds = surface_area_bounds(outline);
    let minimum_u = ((bounds[0] + origin_x) / assembled_width).clamp(0.0, 1.0);
    let maximum_u = ((bounds[2] + origin_x) / assembled_width).clamp(0.0, 1.0);
    let minimum_v = ((bounds[1] + origin_y) / assembled_height).clamp(0.0, 1.0);
    let maximum_v = ((bounds[3] + origin_y) / assembled_height).clamp(0.0, 1.0);
    let first_column = (minimum_u * (field.width - 1) as f32).floor().max(0.0) as usize;
    let last_column = (maximum_u * (field.width - 1) as f32)
        .ceil()
        .min((field.width - 1) as f32) as usize;
    let first_row = (minimum_v * (field.height - 1) as f32).floor().max(0.0) as usize;
    let last_row = (maximum_v * (field.height - 1) as f32)
        .ceil()
        .min((field.height - 1) as f32) as usize;
    let mut segments = Vec::new();

    for row in first_row..last_row {
        for column in first_column..last_column {
            let uv = [
                [
                    column as f32 / (field.width - 1) as f32,
                    row as f32 / (field.height - 1) as f32,
                ],
                [
                    (column + 1) as f32 / (field.width - 1) as f32,
                    row as f32 / (field.height - 1) as f32,
                ],
                [
                    (column + 1) as f32 / (field.width - 1) as f32,
                    (row + 1) as f32 / (field.height - 1) as f32,
                ],
                [
                    column as f32 / (field.width - 1) as f32,
                    (row + 1) as f32 / (field.height - 1) as f32,
                ],
            ];
            let cell_points = uv.map(|point| {
                [
                    point[0] * assembled_width - origin_x,
                    point[1] * assembled_height - origin_y,
                ]
            });
            let cell_values = [
                field.base_classes[row * field.width + column],
                field.base_classes[row * field.width + column + 1],
                field.base_classes[(row + 1) * field.width + column + 1],
                field.base_classes[(row + 1) * field.width + column],
            ]
            .map(|class| f32::from(class == SurfaceClass::Forest));
            if cell_values.iter().all(|value| *value == cell_values[0]) {
                continue;
            }
            add_triangle_contour_segment(
                [cell_points[0], cell_points[1], cell_points[2]],
                [cell_values[0], cell_values[1], cell_values[2]],
                0.5,
                &mut segments,
            );
            add_triangle_contour_segment(
                [cell_points[0], cell_points[2], cell_points[3]],
                [cell_values[0], cell_values[2], cell_values[3]],
                0.5,
                &mut segments,
            );
        }
    }

    let offset = (spacing * 0.28).clamp(0.04, 0.35);
    let before = points.len();
    for path in stitch_contour_segments(segments)
        .into_iter()
        .filter(|path| path.points.len() > 2)
        .map(smooth_contour_path)
    {
        for index in 0..path.points.len() {
            let point = path.points[index];
            let previous = if index > 0 {
                path.points[index - 1]
            } else if path.closed {
                *path.points.last().unwrap()
            } else {
                point
            };
            let next = if index + 1 < path.points.len() {
                path.points[index + 1]
            } else if path.closed {
                path.points[0]
            } else {
                point
            };
            let normal = unit_vector([previous[1] - next[1], next[0] - previous[0]]);
            for candidate in [
                point,
                [point[0] + normal[0] * offset, point[1] + normal[1] * offset],
                [point[0] - normal[0] * offset, point[1] - normal[1] * offset],
            ] {
                if point_in_polygon(candidate, outline) {
                    push_unique_triangulation_point(points, point_keys, candidate);
                }
            }
        }
    }
    points.len() - before
}

fn triangulation_point_key(point: [f32; 2]) -> (i64, i64) {
    (
        (point[0] * 100_000.0).round() as i64,
        (point[1] * 100_000.0).round() as i64,
    )
}

fn push_unique_triangulation_point(
    points: &mut Vec<Point2<f64>>,
    point_keys: &mut HashMap<(i64, i64), usize>,
    point: [f32; 2],
) -> usize {
    let key = triangulation_point_key(point);
    if let Some(index) = point_keys.get(&key) {
        return *index;
    }
    let index = points.len();
    points.push(Point2::new(f64::from(point[0]), f64::from(point[1])));
    point_keys.insert(key, index);
    index
}

#[cfg(test)]
pub(crate) fn build_piece(
    spec: &GenerationSpec,
    height_field: Option<&HeightField>,
    surface_field: Option<&SurfaceField>,
    row: u32,
    column: u32,
) -> Result<Mesh> {
    let height_range = height_range_for_spec(spec, height_field);
    build_piece_with_height_range(spec, height_field, height_range, surface_field, row, column)
}

pub(crate) fn build_piece_with_height_range(
    spec: &GenerationSpec,
    height_field: Option<&HeightField>,
    height_range: Option<(f32, f32)>,
    surface_field: Option<&SurfaceField>,
    row: u32,
    column: u32,
) -> Result<Mesh> {
    let base_samples = spec.terrain_samples_per_piece() as usize;
    let requested_samples = base_samples.max(spec.effective_samples_per_piece() as usize);
    let samples = height_field
        .map(|field| requested_samples.min(field.samples_per_piece(spec)))
        .unwrap_or(requested_samples)
        .max(16);
    let piece_width = if spec.solid_model {
        spec.width_mm
    } else {
        spec.width_mm / spec.columns as f32
    };
    let piece_height = if spec.solid_model {
        spec.height_mm()
    } else {
        spec.height_mm() / spec.rows as f32
    };
    let origin_x = if spec.solid_model {
        0.0
    } else {
        column as f32 * piece_width
    };
    let origin_y = if spec.solid_model {
        0.0
    } else {
        row as f32 * piece_height
    };
    let assembled_width = spec.width_mm;
    let assembled_height = spec.height_mm();
    let outline = if spec.solid_model {
        solid_outline(spec, samples)?
    } else {
        piece_outline(spec, row, column, false)?
    }
    .into_iter()
    .map(|[x, y]| [x - origin_x, y - origin_y])
    .collect::<Vec<_>>();
    let outline_samples = if spec.solid_model {
        samples
    } else {
        spec.samples_per_piece as usize
    };
    let terrain_spacing = piece_width.min(piece_height) / samples as f32;
    let boundary_spacing = piece_width.min(piece_height) / outline_samples as f32;
    let outline = densify_outline_for_triangulation(&outline, boundary_spacing);
    let mut points = outline
        .iter()
        .map(|point| Point2::new(point[0] as f64, point[1] as f64))
        .collect::<Vec<_>>();
    let mut point_keys = outline
        .iter()
        .enumerate()
        .map(|(index, point)| (triangulation_point_key(*point), index))
        .collect::<HashMap<_, _>>();
    let constraints = (0..outline.len())
        .map(|index| [index, (index + 1) % outline.len()])
        .collect::<Vec<_>>();

    let minimum_x = outline
        .iter()
        .map(|point| point[0])
        .fold(f32::INFINITY, f32::min);
    let maximum_x = outline
        .iter()
        .map(|point| point[0])
        .fold(f32::NEG_INFINITY, f32::max);
    let minimum_y = outline
        .iter()
        .map(|point| point[1])
        .fold(f32::INFINITY, f32::min);
    let maximum_y = outline
        .iter()
        .map(|point| point[1])
        .fold(f32::NEG_INFINITY, f32::max);
    if spec.color_output.enabled
        && let Some(field) = surface_field
    {
        add_forest_boundary_points(
            &mut points,
            &mut point_keys,
            field,
            &outline,
            origin_x,
            origin_y,
            assembled_width,
            assembled_height,
            terrain_spacing,
        );
    }
    let grid_columns = ((maximum_x - minimum_x) / terrain_spacing).ceil() as usize;
    let grid_rows = ((maximum_y - minimum_y) / terrain_spacing).ceil() as usize;
    // The densified outline reaches thousands of points at high detail, and
    // both the grid seeding below and the face filter after triangulation
    // run one containment query per sample. The strip index answers each
    // query from roughly one grid row's worth of edges instead of the whole
    // outline while returning exactly what point_in_polygon would.
    let outline_index = PolygonStripIndex::new(&outline, grid_rows.max(1));
    for grid_y in 0..grid_rows {
        let y = minimum_y + (grid_y as f32 + 0.5) * terrain_spacing;
        for grid_x in 0..grid_columns {
            let x = minimum_x + (grid_x as f32 + 0.5) * terrain_spacing;
            if outline_index.contains([x, y]) {
                push_unique_triangulation_point(&mut points, &mut point_keys, [x, y]);
            }
        }
    }
    let triangulation =
        ConstrainedDelaunayTriangulation::<Point2<f64>>::bulk_load_cdt(points, constraints)
            .context("triangulate terrain outline")?;
    let top_count = triangulation.num_vertices();
    let mut vertices = Vec::with_capacity(top_count * 2);
    for vertex in triangulation.vertices() {
        let position = vertex.position();
        let assembled_x = position.x as f32 + origin_x;
        let assembled_y = position.y as f32 + origin_y;
        let u = assembled_x / assembled_width;
        let v = assembled_y / assembled_height;
        let terrain = normalized_height(
            height_field,
            height_range,
            u,
            v,
            spec.center_lat,
            spec.center_lon,
        );
        let z = spec.base_mm + spec.relief_mm * terrain;
        vertices.push([position.x as f32, position.y as f32, z]);
    }
    for vertex in triangulation.vertices() {
        let position = vertex.position();
        vertices.push([position.x as f32, position.y as f32, 0.0]);
    }

    let mut top_triangles = Vec::with_capacity(triangulation.num_inner_faces());
    let mut top_materials = Vec::with_capacity(triangulation.num_inner_faces());
    for face in triangulation.inner_faces() {
        let face_vertices = face.vertices();
        let positions = face_vertices.map(|vertex| vertex.position());
        let centroid = [
            ((positions[0].x + positions[1].x + positions[2].x) / 3.0) as f32,
            ((positions[0].y + positions[1].y + positions[2].y) / 3.0) as f32,
        ];
        if !outline_index.contains(centroid) {
            continue;
        }
        let face_indices = face_vertices.map(|vertex| vertex.fix().index());
        let mut top = face_indices.map(|index| index as u32);
        let area = (positions[1].x - positions[0].x) * (positions[2].y - positions[0].y)
            - (positions[1].y - positions[0].y) * (positions[2].x - positions[0].x);
        if area < 0.0 {
            top.swap(1, 2);
        }
        top_triangles.push(top);
        top_materials.push(
            surface_field
                .map(|field| {
                    field.terrain_at(
                        (centroid[0] + origin_x) / assembled_width,
                        (centroid[1] + origin_y) / assembled_height,
                    )
                })
                .unwrap_or(SurfaceClass::Rock),
        );
    }

    let mut edge_uses = HashMap::<(u32, u32), (u32, [u32; 2])>::new();
    for triangle in &top_triangles {
        for directed in [
            [triangle[0], triangle[1]],
            [triangle[1], triangle[2]],
            [triangle[2], triangle[0]],
        ] {
            let key = if directed[0] < directed[1] {
                (directed[0], directed[1])
            } else {
                (directed[1], directed[0])
            };
            let entry = edge_uses.entry(key).or_insert((0, directed));
            entry.0 += 1;
        }
    }

    let mut triangles = Vec::with_capacity(top_triangles.len() * 2 + edge_uses.len() * 2);
    let mut materials = Vec::with_capacity(triangles.capacity());
    for (top, material) in top_triangles.into_iter().zip(top_materials) {
        triangles.push(top);
        materials.push(material);
        triangles.push([
            top[0] + top_count as u32,
            top[2] + top_count as u32,
            top[1] + top_count as u32,
        ]);
        materials.push(SurfaceClass::Rock);
    }
    // HashMap iteration order is randomized per process; sort the boundary
    // edges so the emitted mesh (and every artifact hashed from it) is
    // byte-for-byte reproducible across runs.
    let mut boundary_edges = edge_uses
        .into_values()
        .filter(|(uses, _)| *uses == 1)
        .map(|(_, edge)| edge)
        .collect::<Vec<_>>();
    boundary_edges.sort_unstable();
    for [from, to] in boundary_edges {
        triangles.push([from, to + top_count as u32, to]);
        materials.push(SurfaceClass::Rock);
        triangles.push([from, from + top_count as u32, to + top_count as u32]);
        materials.push(SurfaceClass::Rock);
    }

    let mut mesh = Mesh {
        name: if spec.solid_model {
            "Solid Terrain".into()
        } else {
            format!("Piece {}-{}", row + 1, column + 1)
        },
        vertices,
        triangles,
        materials,
        quantization_collisions: Vec::new(),
    };
    let mut building_union = None;
    if spec.buildings.enabled
        && let Some(field) = surface_field
    {
        building_union = Some(append_building_geometry(
            &mut mesh,
            spec,
            field,
            height_field,
            height_range,
            &outline,
            origin_x,
            origin_y,
            assembled_width,
            assembled_height,
        )?);
    }
    if spec.color_output.enabled
        && spec.color_output.roads_enabled
        && let Some(field) = surface_field
    {
        append_road_geometry(
            &mut mesh,
            spec,
            field,
            height_field,
            height_range,
            &outline,
            origin_x,
            origin_y,
            assembled_width,
            assembled_height,
            building_union.as_ref(),
        )?;
    }
    weld_export_mesh(&mut mesh);
    Ok(mesh)
}

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
fn append_building_geometry(
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

fn multi_polygon_bounds(multi_polygon: &MultiPolygon<f64>) -> [f32; 4] {
    let mut bounds = [
        f32::INFINITY,
        f32::INFINITY,
        f32::NEG_INFINITY,
        f32::NEG_INFINITY,
    ];
    for polygon in &multi_polygon.0 {
        for point in &polygon.exterior().0 {
            bounds[0] = bounds[0].min(point.x as f32);
            bounds[1] = bounds[1].min(point.y as f32);
            bounds[2] = bounds[2].max(point.x as f32);
            bounds[3] = bounds[3].max(point.y as f32);
        }
    }
    bounds
}

/// Rebuilds a polygon (first ring exterior, rest holes) from snapped open
/// rings, for containment tests that agree with the triangulated geometry.
fn polygon_from_rings(rings: &[Vec<[f32; 2]>]) -> Option<Polygon<f64>> {
    let mut ring_strings = rings.iter().map(|ring| {
        let mut coordinates = ring
            .iter()
            .map(|point| Coord {
                x: f64::from(point[0]),
                y: f64::from(point[1]),
            })
            .collect::<Vec<_>>();
        if let Some(first) = coordinates.first().copied() {
            coordinates.push(first);
        }
        LineString::new(coordinates)
    });
    let exterior = ring_strings.next()?;
    Some(Polygon::new(exterior, ring_strings.collect()))
}

/// Converts one closed ring into snapped open points: every coordinate lands
/// on the 1e-5 export grid, and consecutive duplicates created by the snap
/// are removed. No collinear simplification happens here — the union outline
/// and the member footprint constraints must keep their shared segments
/// exactly identical so the triangulation dedupes them.
fn snapped_open_ring(ring: &LineString<f64>) -> Vec<[f32; 2]> {
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
    points.dedup();
    while points.len() > 1 && points.first() == points.last() {
        points.pop();
    }
    points
}

fn ring_signed_area(points: &[[f32; 2]]) -> f64 {
    let mut area = 0.0;
    for index in 0..points.len() {
        let a = points[index];
        let b = points[(index + 1) % points.len()];
        area += f64::from(a[0]) * f64::from(b[1]) - f64::from(b[0]) * f64::from(a[1]);
    }
    area * 0.5
}

/// Cleans the footprint rings of one overlay material group before shelling.
///
/// * Every coordinate snaps to the 1e-5 export grid and consecutive
///   duplicates disappear, so triangulation never sees the sub-grid slivers
///   that used to collapse into degenerate triangles in `MeshBuilder`.
/// * Degenerate rings (under three points or under the minimum area) drop
///   out; a degenerate exterior drops its whole polygon.
/// * A snapped point that occurs twice anywhere in the group — a ring
///   touching itself, a hole touching its exterior, or two polygons meeting
///   at a point — is a pinch: the shells built from those rings would stack
///   four wall quads on one vertical edge. Every occurrence after the first
///   retracts a few microns toward its own lobe (or is removed when its
///   edges are too short to retract along), which separates the lobes
///   without visibly changing the outline.
fn sanitize_footprint_group(group: MultiPolygon<f64>, simplify: bool) -> MultiPolygon<f64> {
    let mut polygons = Vec::new();
    for polygon in group.0 {
        let mut rings = Vec::new();
        for (ring_index, ring) in std::iter::once(polygon.exterior())
            .chain(polygon.interiors())
            .enumerate()
        {
            let mut points = snapped_open_ring(ring);
            if simplify {
                points = simplify_closed_ring(points);
            }
            if points.len() < 3 || ring_signed_area(&points).abs() <= MINIMUM_OVERLAY_AREA_MM2 {
                if ring_index == 0 {
                    rings.clear();
                    break;
                }
                continue;
            }
            rings.push(points);
        }
        if !rings.is_empty() {
            polygons.push(rings);
        }
    }

    let mut seen = HashMap::<[u32; 2], u32>::new();
    for rings in &mut polygons {
        for ring in rings.iter_mut() {
            let orientation = ring_signed_area(ring).signum() as f32;
            let original = ring.clone();
            let mut removed = vec![false; original.len()];
            for index in 0..original.len() {
                let point = original[index];
                let key = [point[0].to_bits(), point[1].to_bits()];
                let occurrences = seen.entry(key).or_insert(0);
                *occurrences += 1;
                if *occurrences == 1 {
                    continue;
                }
                let previous = original[(index + original.len() - 1) % original.len()];
                let next = original[(index + 1) % original.len()];
                let Some(moved) = retract_pinch_point(point, previous, next, orientation) else {
                    removed[index] = true;
                    continue;
                };
                ring[index] = moved;
                *seen
                    .entry([moved[0].to_bits(), moved[1].to_bits()])
                    .or_insert(0) += 1;
            }
            if removed.iter().any(|flag| *flag) {
                *ring = ring
                    .iter()
                    .zip(&removed)
                    .filter(|(_, removed)| !**removed)
                    .map(|(point, _)| *point)
                    .collect();
            }
        }
        rings.retain(|ring| {
            ring.len() >= 3 && ring_signed_area(ring).abs() > MINIMUM_OVERLAY_AREA_MM2
        });
    }

    MultiPolygon(
        polygons
            .into_iter()
            .filter_map(|rings| polygon_from_rings(&rings))
            .collect(),
    )
}

/// Moves a repeated ring point a few microns into its own lobe: along the
/// corner's angle bisector, which points into the enclosed wedge for the
/// sharp corners pinches form. Falls back to the ring's enclosed side when
/// the corner is collinear. Returns `None` when the neighboring edges are
/// too short to retract along, meaning the point should be removed instead.
fn retract_pinch_point(
    point: [f32; 2],
    previous: [f32; 2],
    next: [f32; 2],
    orientation: f32,
) -> Option<[f32; 2]> {
    let incoming_length = distance_squared(previous, point).sqrt();
    let outgoing_length = distance_squared(next, point).sqrt();
    let shortest = incoming_length.min(outgoing_length);
    let epsilon = (0.25 * shortest).min(OVERLAY_SEPARATION_MM as f32);
    if epsilon < 0.000_03 {
        return None;
    }
    let to_previous = unit_vector([previous[0] - point[0], previous[1] - point[1]]);
    let to_next = unit_vector([next[0] - point[0], next[1] - point[1]]);
    let bisector = [to_previous[0] + to_next[0], to_previous[1] + to_next[1]];
    let direction = if bisector[0].hypot(bisector[1]) > 0.001 {
        unit_vector(bisector)
    } else {
        let tangent = unit_vector([next[0] - previous[0], next[1] - previous[1]]);
        [-tangent[1] * orientation, tangent[0] * orientation]
    };
    let moved = [
        quantize_export_coordinate(point[0] + direction[0] * epsilon),
        quantize_export_coordinate(point[1] + direction[1] * epsilon),
    ];
    if moved == point { None } else { Some(moved) }
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

fn terrain_z_at(
    spec: &GenerationSpec,
    height_field: Option<&HeightField>,
    height_range: Option<(f32, f32)>,
    u: f32,
    v: f32,
) -> f32 {
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

/// Builds the road shells of one piece.
///
/// Ordinary roads all share one terrain-following surface, so their clipped
/// ribbons are unioned into a single footprint per piece and each connected
/// component is shelled once — abutting or overlapping ribbons can therefore
/// never leave coincident faces for a slicer weld to fuse. Bridge ribbons
/// keep a shell per line because their deck height comes from the line's own
/// elevation profile. Every road footprint also keeps
/// [`OVERLAY_SEPARATION_MM`] clear of the building union so road and
/// building shells never share welded vertices.
#[allow(clippy::too_many_arguments)]
fn append_road_geometry(
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
) -> Result<()> {
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
    let roads = surface_field
        .vector_lines
        .iter()
        .filter(|line| line.class == SurfaceClass::Road)
        .filter(|line| {
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
        })
        .collect::<Vec<_>>();
    if roads.is_empty() {
        return Ok(());
    }
    // Buildings the roads must keep clear of, grown by the separation gap.
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
    let clip_ribbon = |line: &VectorSurfaceLine| {
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
            clipped = clipped.difference(obstacles);
        }
        clipped
    };
    let (bridges, regular): (Vec<_>, Vec<_>) = roads
        .into_iter()
        .partition(|line| line.bridge_elevations_m.is_some());
    // Ordinary ribbons are clipped in parallel and unioned; the union is
    // shelled per connected component further below, once the bridge decks
    // it must keep clear of are known.
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
    for (ordinal, (group_lines, deck_area)) in decks.into_iter().enumerate() {
        // Each deck group embeds a fraction deeper than the last. Supported
        // decks of *different* groups follow the same terrain-hugging
        // bottom, and where two groups meet at a shared road node their
        // buffered end caps coincide exactly — distinct embed depths keep
        // those bottoms from welding into one non-manifold sheet. The
        // offsets stay far below print resolution, hidden inside terrain.
        let embed_mm = quantize_export_coordinate(
            OVERLAY_TERRAIN_EMBED_MM + ((ordinal % 64) as f32 + 1.0) * 0.000_05,
        );
        let deck_area = sanitize_footprint_group(deck_area, true);
        let group_shells = deck_area
            .0
            .par_iter()
            .filter(|polygon| polygon.unsigned_area() > MINIMUM_OVERLAY_AREA_MM2)
            .map(|polygon| {
                build_road_polygon_shell_with_embed(
                    polygon,
                    spec,
                    &group_lines,
                    height_field,
                    height_range,
                    origin_x,
                    origin_y,
                    assembled_width,
                    assembled_height,
                    embed_mm,
                )
            })
            .collect::<Result<Vec<_>>>()?;
        for shell in group_shells {
            mesh.append_isolated(shell);
        }
    }
    Ok(())
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

pub(crate) fn geo_polygon(points: &[[f32; 2]]) -> Polygon<f64> {
    let mut coordinates = points
        .iter()
        .map(|point| Coord {
            x: point[0] as f64,
            y: point[1] as f64,
        })
        .collect::<Vec<_>>();
    if coordinates.first() != coordinates.last()
        && let Some(first) = coordinates.first().copied()
    {
        coordinates.push(first);
    }
    Polygon::new(LineString::new(coordinates), vec![])
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
    build_polygon_shell(
        polygon,
        bottom,
        top,
        boundary_step_mm,
        SurfaceClass::Road,
        "triangulate vector road ribbon",
    )
}

fn build_polygon_shell(
    polygon: &Polygon<f64>,
    bottom: impl Fn([f32; 2]) -> f32,
    top: impl Fn([f32; 2]) -> f32,
    boundary_step_mm: Option<f32>,
    material: SurfaceClass,
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

/// Areas of every triangulation face, indexed like the all-faces domain
/// (the outer face keeps zero).
fn triangulation_face_areas(
    triangulation: &ConstrainedDelaunayTriangulation<Point2<f64>>,
) -> Vec<f64> {
    let mut areas = vec![0.0; triangulation.num_all_faces()];
    for face in triangulation.inner_faces() {
        let positions = face.vertices().map(|vertex| vertex.position());
        let area = 0.5
            * ((positions[1].x - positions[0].x) * (positions[2].y - positions[0].y)
                - (positions[1].y - positions[0].y) * (positions[2].x - positions[0].x))
                .abs();
        areas[face.fix().index()] = area;
    }
    areas
}

/// Repairs pinches in a kept-face set: a vertex whose incident faces
/// alternate kept/dropped more than twice stacks four or more wall quads on
/// one vertical edge. Ring coordinates are rounded, so two rings can cross
/// by a sliver, which drops a conflicting constraint and lets the
/// classification leak into (or out of) a tiny pocket. Flipping the
/// smallest alternating fan at each pinched vertex — sweeping until stable —
/// dissolves those pockets while never touching more than sliver-scale area.
/// `allow_fill` controls whether dropped fans may flip to kept. Road shells
/// allow it (a dropped pocket strictly inside the ring is always road).
/// Building components must not: their triangulation spans neighboring
/// components' footprints too, and filling would grow the shell into a
/// neighbor's area — shedding the smaller kept fan resolves the pinch
/// instead.
fn repair_classification_pinches(
    triangulation: &ConstrainedDelaunayTriangulation<Point2<f64>>,
    inside: &mut [bool],
    allow_fill: bool,
) {
    let outer = triangulation.outer_face().fix().index();
    let areas = triangulation_face_areas(triangulation);
    for _sweep in 0..8 {
        let mut changed = false;
        for vertex in triangulation.vertices() {
            let faces = vertex
                .out_edges()
                .map(|edge| edge.face().fix().index())
                .collect::<Vec<_>>();
            if faces.len() < 4 {
                continue;
            }
            let transitions = (0..faces.len())
                .filter(|index| inside[faces[*index]] != inside[faces[(index + 1) % faces.len()]])
                .count();
            if transitions <= 2 {
                continue;
            }
            // Cyclic runs of equal classification; flip the smallest one
            // that does not contain the outer face.
            let start = (0..faces.len())
                .find(|index| {
                    inside[faces[(index + faces.len() - 1) % faces.len()]] != inside[faces[*index]]
                })
                .unwrap_or(0);
            let mut runs: Vec<(f64, bool, Vec<usize>)> = Vec::new();
            for offset in 0..faces.len() {
                let face = faces[(start + offset) % faces.len()];
                match runs.last_mut() {
                    Some((area, kept, members)) if *kept == inside[face] => {
                        *area += areas[face];
                        members.push(face);
                    }
                    _ => runs.push((areas[face], inside[face], vec![face])),
                }
            }
            let smallest = runs
                .iter()
                .enumerate()
                .filter(|(_, (_, kept, members))| {
                    (allow_fill || *kept) && !members.contains(&outer)
                })
                .min_by(|(_, (left, ..)), (_, (right, ..))| left.total_cmp(right));
            if let Some((_, (_, _, members))) = smallest {
                for face in members {
                    inside[*face] = !inside[*face];
                }
                changed = true;
            }
        }
        if !changed {
            break;
        }
    }
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

fn simplify_closed_ring(mut points: Vec<[f32; 2]>) -> Vec<[f32; 2]> {
    loop {
        if points.len() <= 3 {
            return points;
        }
        let count = points.len();
        let mut simplified = Vec::with_capacity(count);
        for index in 0..count {
            let previous = points[(index + count - 1) % count];
            let point = points[index];
            let next = points[(index + 1) % count];
            let incoming = [point[0] - previous[0], point[1] - previous[1]];
            let outgoing = [next[0] - point[0], next[1] - point[1]];
            let continues_forward = incoming[0] * outgoing[0] + incoming[1] * outgoing[1] > 0.0;
            if !continues_forward || point_line_distance(point, previous, next) > 0.000_1 {
                simplified.push(point);
            }
        }
        if simplified.len() == points.len() || simplified.len() < 3 {
            return points;
        }
        points = simplified;
    }
}

fn bounds_overlap(left: [f32; 4], right: [f32; 4]) -> bool {
    left[0] <= right[2] && left[2] >= right[0] && left[1] <= right[3] && left[3] >= right[1]
}

fn densify_outline_for_triangulation(outline: &[[f32; 2]], maximum_step: f32) -> Vec<[f32; 2]> {
    if outline.len() < 3 {
        return outline.to_vec();
    }
    let signed_area = outline
        .iter()
        .zip(outline.iter().cycle().skip(1))
        .map(|(start, end)| start[0] * end[1] - end[0] * start[1])
        .sum::<f32>();
    let inward_sign = if signed_area >= 0.0 { 1.0 } else { -1.0 };
    let mut dense = Vec::with_capacity(outline.len());
    for (start, end) in outline.iter().zip(outline.iter().cycle().skip(1)) {
        let delta = [end[0] - start[0], end[1] - start[1]];
        let length = delta[0].hypot(delta[1]);
        let segments = (length / maximum_step.max(0.01)).ceil().max(1.0) as usize;
        let inward = if length <= f32::EPSILON {
            [0.0, 0.0]
        } else {
            [
                -delta[1] / length * inward_sign,
                delta[0] / length * inward_sign,
            ]
        };
        for index in 0..segments {
            let t = index as f32 / segments as f32;
            let offset = if index % 2 == 1 { 0.001 } else { 0.0 };
            dense.push([
                start[0] + delta[0] * t + inward[0] * offset,
                start[1] + delta[1] * t + inward[1] * offset,
            ]);
        }
    }
    let unshifted = dense.clone();
    let point_count = dense.len();
    for index in (1..point_count).step_by(2) {
        let previous = unshifted[(index + point_count - 1) % point_count];
        let point = unshifted[index];
        let next = unshifted[(index + 1) % point_count];
        if point_line_distance(point, previous, next) > 0.000_01 {
            continue;
        }
        let tangent = [next[0] - previous[0], next[1] - previous[1]];
        let length = tangent[0].hypot(tangent[1]);
        if length > f32::EPSILON {
            dense[index][0] += -tangent[1] / length * inward_sign * 0.001;
            dense[index][1] += tangent[0] / length * inward_sign * 0.001;
        }
    }
    dense
}

pub(crate) fn solid_outline(spec: &GenerationSpec, edge_samples: usize) -> Result<Vec<[f32; 2]>> {
    if spec.adjacent_interlocks && (spec.adjacent_columns > 1 || spec.adjacent_rows > 1) {
        let mut tile = spec.clone();
        tile.rows = 1;
        tile.columns = 1;
        tile.clearance_mm = 0.0;
        return piece_outline(&tile, 0, 0, true);
    }
    let corners = [
        [0.0, 0.0],
        [spec.width_mm, 0.0],
        [spec.width_mm, spec.height_mm()],
        [0.0, spec.height_mm()],
    ];
    let mut outline = Vec::with_capacity(edge_samples * 4);
    for edge in 0..4 {
        let start = corners[edge];
        let end = corners[(edge + 1) % corners.len()];
        for index in 0..edge_samples {
            let t = index as f32 / edge_samples as f32;
            outline.push([
                start[0] + (end[0] - start[0]) * t,
                start[1] + (end[1] - start[1]) * t,
            ]);
        }
    }
    Ok(outline)
}

fn piece_outline(
    spec: &GenerationSpec,
    row: u32,
    column: u32,
    exact_shared_edge: bool,
) -> Result<Vec<[f32; 2]>> {
    let bottom_left = puzzle_grid_point(spec, row, column);
    let bottom_right = puzzle_grid_point(spec, row, column + 1);
    let top_right = puzzle_grid_point(spec, row + 1, column + 1);
    let top_left = puzzle_grid_point(spec, row + 1, column);
    let nominal_piece_size =
        (spec.width_mm / spec.columns as f32).min(spec.height_mm() / spec.rows as f32);
    let base_depth = nominal_piece_size * 0.17;
    let edge_samples = spec.samples_per_piece.clamp(64, 128) as usize;
    let mut outline = Vec::with_capacity(edge_samples * 4);

    for index in 0..edge_samples {
        let t = index as f32 / edge_samples as f32;
        outline.push(puzzle_edge_point(
            bottom_left,
            bottom_right,
            piece_edge_pattern(spec, 0, column, row),
            puzzle_edge_sign(spec, 0, column, row, spec.rows),
            t,
            base_depth,
        ));
    }
    for index in 0..edge_samples {
        let t = index as f32 / edge_samples as f32;
        outline.push(puzzle_edge_point(
            bottom_right,
            top_right,
            piece_edge_pattern(spec, 1, row, column + 1),
            puzzle_edge_sign(spec, 1, row, column + 1, spec.columns),
            t,
            base_depth,
        ));
    }
    for index in 0..edge_samples {
        let t = 1.0 - index as f32 / edge_samples as f32;
        outline.push(puzzle_edge_point(
            top_left,
            top_right,
            piece_edge_pattern(spec, 0, column, row + 1),
            puzzle_edge_sign(spec, 0, column, row + 1, spec.rows),
            t,
            base_depth,
        ));
    }
    for index in 0..edge_samples {
        let t = 1.0 - index as f32 / edge_samples as f32;
        outline.push(puzzle_edge_point(
            bottom_left,
            top_left,
            piece_edge_pattern(spec, 1, row, column),
            puzzle_edge_sign(spec, 1, row, column, spec.columns),
            t,
            base_depth,
        ));
    }

    if !exact_shared_edge && spec.clearance_mm > 0.0 {
        outline = inset_outline(&outline, spec.clearance_mm * 0.5)?;
    }
    Ok(outline)
}

fn puzzle_grid_point(spec: &GenerationSpec, row: u32, column: u32) -> [f32; 2] {
    let piece_width = spec.width_mm / spec.columns as f32;
    let piece_height = spec.height_mm() / spec.rows as f32;
    if spec.straight_piece_sides {
        let x = if column == spec.columns {
            spec.width_mm
        } else {
            column as f32 * piece_width
        };
        let y = if row == spec.rows {
            spec.height_mm()
        } else {
            row as f32 * piece_height
        };
        return [x, y];
    }
    let seed = ((row as u64) << 32) | column as u64;
    let x = if column == 0 {
        0.0
    } else if column == spec.columns {
        spec.width_mm
    } else {
        column as f32 * piece_width + (edge_noise(seed, 0) - 0.5) * piece_width * 0.18
    };
    let y = if row == 0 {
        0.0
    } else if row == spec.rows {
        spec.height_mm()
    } else {
        row as f32 * piece_height + (edge_noise(seed, 1) - 0.5) * piece_height * 0.18
    };
    [x, y]
}

fn puzzle_edge_sign(
    spec: &GenerationSpec,
    orientation: u64,
    segment: u32,
    line: u32,
    line_count: u32,
) -> f32 {
    if let Some((global_segment, global_line, global_line_count)) =
        adjacent_edge_key(spec, orientation, segment, line, line_count)
    {
        edge_sign(orientation, global_segment, global_line, global_line_count)
    } else if spec.puzzle_tabs {
        edge_sign(orientation, segment, line, line_count)
    } else {
        0.0
    }
}

fn piece_edge_pattern(
    spec: &GenerationSpec,
    orientation: u64,
    segment: u32,
    line: u32,
) -> EdgePattern {
    adjacent_edge_key(
        spec,
        orientation,
        segment,
        line,
        if orientation == 0 {
            spec.rows
        } else {
            spec.columns
        },
    )
    .map(|(global_segment, global_line, _)| {
        shared_edge_pattern(orientation, global_line, global_segment)
    })
    .unwrap_or_else(|| shared_edge_pattern(orientation, line, segment))
}

fn adjacent_edge_key(
    spec: &GenerationSpec,
    orientation: u64,
    segment: u32,
    line: u32,
    line_count: u32,
) -> Option<(u32, u32, u32)> {
    if !spec.adjacent_interlocks || (line != 0 && line != line_count) {
        return None;
    }
    if orientation == 0 {
        let global_line = if line == 0 {
            spec.adjacent_tile_row + 1
        } else {
            spec.adjacent_tile_row
        };
        Some((
            spec.adjacent_tile_column * spec.columns + segment,
            global_line,
            spec.adjacent_rows,
        ))
    } else {
        let global_line = if line == 0 {
            spec.adjacent_tile_column
        } else {
            spec.adjacent_tile_column + 1
        };
        Some((
            spec.adjacent_tile_row * spec.rows + segment,
            global_line,
            spec.adjacent_columns,
        ))
    }
}

fn inset_outline(outline: &[[f32; 2]], distance: f32) -> Result<Vec<[f32; 2]>> {
    let mut coordinates = outline
        .iter()
        .map(|point| Coord {
            x: point[0] as f64,
            y: point[1] as f64,
        })
        .collect::<Vec<_>>();
    coordinates.push(coordinates[0]);

    let inset = Polygon::new(LineString::new(coordinates), vec![]).buffer(-(distance as f64));
    let polygon = inset
        .0
        .into_iter()
        .max_by(|first, second| first.unsigned_area().total_cmp(&second.unsigned_area()))
        .context("clearance removed the puzzle-piece outline")?;
    if !polygon.interiors().is_empty() {
        bail!("clearance produced holes in the puzzle-piece outline");
    }

    let mut result = Vec::<[f32; 2]>::new();
    for point in &polygon.exterior().0 {
        let candidate = [point.x as f32, point.y as f32];
        let is_duplicate = result.last().is_some_and(|previous| {
            (previous[0] - candidate[0]).hypot(previous[1] - candidate[1]) < 0.000_01
        });
        if !is_duplicate {
            result.push(candidate);
        }
    }
    if result.len() > 1
        && (result[0][0] - result[result.len() - 1][0])
            .hypot(result[0][1] - result[result.len() - 1][1])
            < 0.000_01
    {
        result.pop();
    }
    Ok(result)
}

pub(crate) fn scaled_building_height_mm(spec: &GenerationSpec, height_m: f32) -> f32 {
    if !spec.buildings.enabled {
        return 0.0;
    }
    height_m * spec.width_mm / (spec.ground_span_km as f32 * 1_000.0) * spec.buildings.z_scale
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::{HashMap, HashSet};

    use crate::mesh::assert_watertight;
    use crate::preview::build_preview;
    use crate::spec::{BuildingSpec, ColorOutputSpec};

    #[test]
    fn shared_height_frame_keeps_absolute_elevations_at_the_same_height() {
        let spec = GenerationSpec {
            elevation_datum_m: Some(50.0),
            elevation_m_per_mm: Some(10.0),
            ..GenerationSpec::default()
        };
        let first = HeightField::new(2, 2, vec![100.0, 200.0, 100.0, 200.0], "first").unwrap();
        let second = HeightField::new(2, 2, vec![0.0, 200.0, 300.0, 400.0], "second").unwrap();

        let first_z = terrain_z_at(
            &spec,
            Some(&first),
            height_range_for_spec(&spec, Some(&first)),
            1.0,
            0.0,
        );
        let second_z = terrain_z_at(
            &spec,
            Some(&second),
            height_range_for_spec(&spec, Some(&second)),
            1.0,
            0.0,
        );

        assert!((first_z - second_z).abs() < 0.0001);
        assert!((first_z - (spec.base_mm + 15.0)).abs() < 0.0001);
    }

    #[test]
    fn shared_edges_are_identical_before_clearance() {
        let spec = GenerationSpec::default();
        let edge_samples = spec.samples_per_piece as usize;
        let left_piece = piece_outline(&spec, 1, 1, true).unwrap();
        let right_piece = piece_outline(&spec, 1, 2, true).unwrap();
        for point in &left_piece[edge_samples..edge_samples * 2] {
            let matching_distance = right_piece
                .iter()
                .map(|candidate| (candidate[0] - point[0]).hypot(candidate[1] - point[1]))
                .fold(f32::INFINITY, f32::min);
            assert!(matching_distance < 0.0001);
        }
    }

    #[test]
    fn optional_adjacent_tile_edges_interlock_without_warping_the_grid() {
        let left_spec = GenerationSpec {
            solid_model: true,
            adjacent_columns: 2,
            adjacent_rows: 1,
            adjacent_interlocks: true,
            adjacent_tile_column: 0,
            ..GenerationSpec::default()
        };
        let right_spec = GenerationSpec {
            adjacent_tile_column: 1,
            ..left_spec.clone()
        };
        let left = solid_outline(&left_spec, 96).unwrap();
        let right = solid_outline(&right_spec, 96)
            .unwrap()
            .into_iter()
            .map(|point| [point[0] + left_spec.width_mm, point[1]])
            .collect::<Vec<_>>();

        let edge_samples = left.len() / 4;
        let left_shared = left[edge_samples..edge_samples * 2]
            .iter()
            .collect::<Vec<_>>();
        let right_shared = right[edge_samples * 3..edge_samples * 4]
            .iter()
            .collect::<Vec<_>>();
        assert!(
            left_shared
                .iter()
                .any(|point| { (point[0] - left_spec.width_mm).abs() > left_spec.width_mm * 0.01 })
        );
        for point in left_shared.into_iter().skip(1) {
            let distance = right_shared
                .iter()
                .map(|candidate| (point[0] - candidate[0]).hypot(point[1] - candidate[1]))
                .fold(f32::INFINITY, f32::min);
            assert!(distance < 0.001);
        }
        assert!(
            left.iter()
                .filter(|point| point[1] < 0.001)
                .all(|point| point[1].abs() < 0.001)
        );

        let plain = solid_outline(
            &GenerationSpec {
                adjacent_interlocks: false,
                ..left_spec
            },
            96,
        )
        .unwrap();
        assert!(
            plain[plain.len() / 4..plain.len() / 2]
                .iter()
                .all(|point| (point[0] - right_spec.width_mm).abs() < 0.0001)
        );
    }

    #[test]
    fn shared_seam_keeps_the_requested_minimum_clearance() {
        for straight_piece_sides in [false, true] {
            for puzzle_tabs in [false, true] {
                let spec = GenerationSpec {
                    straight_piece_sides,
                    puzzle_tabs,
                    ..GenerationSpec::default()
                };
                let fitted_left = piece_outline(&spec, 1, 1, false).unwrap();
                let fitted_right = piece_outline(&spec, 1, 2, false).unwrap();

                let gap = fitted_left
                    .iter()
                    .map(|point| point_outline_distance(*point, &fitted_right))
                    .chain(
                        fitted_right
                            .iter()
                            .map(|point| point_outline_distance(*point, &fitted_left)),
                    )
                    .fold(f32::INFINITY, f32::min);
                assert!(
                    (gap - spec.clearance_mm).abs() < 0.015,
                    "straight={straight_piece_sides}, tabs={puzzle_tabs}: minimum shared clearance was {gap} mm"
                );
            }
        }
    }

    #[test]
    fn straight_tabless_pieces_use_plain_rectangular_cuts() {
        let spec = GenerationSpec {
            straight_piece_sides: true,
            puzzle_tabs: false,
            ..GenerationSpec::default()
        };
        let piece_width = spec.width_mm / spec.columns as f32;
        let piece_height = spec.height_mm() / spec.rows as f32;
        let outline = piece_outline(&spec, 1, 1, true).unwrap();

        for point in outline {
            let on_vertical_edge = (point[0] - piece_width).abs() < 0.0001
                || (point[0] - piece_width * 2.0).abs() < 0.0001;
            let on_horizontal_edge = (point[1] - piece_height).abs() < 0.0001
                || (point[1] - piece_height * 2.0).abs() < 0.0001;
            assert!(on_vertical_edge || on_horizontal_edge, "{point:?}");
        }
    }

    #[test]
    fn every_piece_shape_mode_is_watertight() {
        for straight_piece_sides in [false, true] {
            for puzzle_tabs in [false, true] {
                let spec = GenerationSpec {
                    straight_piece_sides,
                    puzzle_tabs,
                    ..GenerationSpec::default()
                };
                let mesh = build_piece(&spec, None, None, 1, 1).unwrap();
                assert_watertight(&mesh);
            }
        }
    }

    #[test]
    fn generated_piece_is_watertight() {
        let mesh = build_piece(&GenerationSpec::default(), None, None, 0, 0).unwrap();
        assert_watertight(&mesh);
    }

    #[test]
    fn solid_mode_supports_maximum_detail() {
        let spec = GenerationSpec {
            samples_per_piece: 128,
            solid_model: true,
            ..GenerationSpec::default()
        };
        let mesh = build_piece(&spec, None, None, 0, 0).unwrap();
        assert!(mesh.vertices.len() > 100_000);
    }

    #[test]
    fn forest_edges_add_targeted_smooth_mesh_points() {
        let classes = (0..7)
            .flat_map(|row| {
                (0..7).map(move |column| {
                    if column + row / 2 < 4 {
                        SurfaceClass::Forest
                    } else {
                        SurfaceClass::Rock
                    }
                })
            })
            .collect();
        let field = SurfaceField::new(7, 7, classes, "test").unwrap();
        let outline = vec![[0.0, 0.0], [60.0, 0.0], [60.0, 60.0], [0.0, 60.0]];
        let mut points = outline
            .iter()
            .map(|point| Point2::new(f64::from(point[0]), f64::from(point[1])))
            .collect::<Vec<_>>();
        let mut point_keys = outline
            .iter()
            .enumerate()
            .map(|(index, point)| (triangulation_point_key(*point), index))
            .collect::<HashMap<_, _>>();

        let added = add_forest_boundary_points(
            &mut points,
            &mut point_keys,
            &field,
            &outline,
            0.0,
            0.0,
            60.0,
            60.0,
            4.0,
        );

        assert!(added > 24, "expected dense boundary points, got {added}");
        assert!(points.iter().skip(4).any(|point| {
            let on_source_grid = (point.x / 10.0 - (point.x / 10.0).round()).abs() < 0.000_01
                && (point.y / 10.0 - (point.y / 10.0).round()).abs() < 0.000_01;
            !on_source_grid
        }));
    }

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
        let flat = build_piece(
            &GenerationSpec {
                color_output: ColorOutputSpec {
                    roads_enabled: false,
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
        assert!(!flat.materials.contains(&SurfaceClass::Road));
        assert_watertight(&raised);
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

    #[test]
    fn puzzle_grid_points_vary() {
        let spec = GenerationSpec::default();
        let nominal = spec.width_mm / spec.columns as f32;
        let interior = puzzle_grid_point(&spec, 1, 1);
        assert!((interior[0] - nominal).abs() > 0.01);
        assert!((interior[1] - nominal).abs() > 0.01);
    }

    #[test]
    fn all_supported_detail_levels_triangulate() {
        for samples_per_piece in [64, 88, 104, 112, 128, 160] {
            let spec = GenerationSpec {
                samples_per_piece,
                ..GenerationSpec::default()
            };
            for row in 0..spec.rows {
                for column in 0..spec.columns {
                    build_piece(&spec, None, None, row, column).unwrap_or_else(|error| {
                        panic!("detail {samples_per_piece}, piece {row}-{column} failed: {error}")
                    });
                }
            }
        }
    }

    #[test]
    fn high_detail_outlines_work_for_every_grid_size() {
        for grid_size in [2, 4, 8, 12, 16] {
            let spec = GenerationSpec {
                rows: grid_size,
                columns: grid_size,
                samples_per_piece: 160,
                ..GenerationSpec::default()
            };
            for row in 0..spec.rows {
                for column in 0..spec.columns {
                    let outline = piece_outline(&spec, row, column, false).unwrap();
                    let points = outline
                        .iter()
                        .map(|point| Point2::new(point[0] as f64, point[1] as f64))
                        .collect::<Vec<_>>();
                    let constraints = (0..outline.len())
                        .map(|index| [index, (index + 1) % outline.len()])
                        .collect::<Vec<_>>();
                    ConstrainedDelaunayTriangulation::<Point2<f64>>::bulk_load_cdt(
                        points,
                        constraints,
                    )
                    .unwrap_or_else(|error| {
                        panic!("grid {grid_size}, piece {row}-{column} failed: {error}")
                    });
                }
            }
        }
    }

    fn point_segment_distance(point: [f32; 2], start: [f32; 2], end: [f32; 2]) -> f32 {
        let segment = [end[0] - start[0], end[1] - start[1]];
        let length_squared = segment[0] * segment[0] + segment[1] * segment[1];
        let t = (((point[0] - start[0]) * segment[0] + (point[1] - start[1]) * segment[1])
            / length_squared.max(f32::EPSILON))
        .clamp(0.0, 1.0);
        (point[0] - start[0] - t * segment[0]).hypot(point[1] - start[1] - t * segment[1])
    }

    fn point_outline_distance(point: [f32; 2], outline: &[[f32; 2]]) -> f32 {
        (0..outline.len())
            .map(|index| {
                point_segment_distance(point, outline[index], outline[(index + 1) % outline.len()])
            })
            .fold(f32::INFINITY, f32::min)
    }
}
