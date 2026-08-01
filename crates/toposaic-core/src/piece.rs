use std::collections::{HashMap, HashSet};

use anyhow::{Context, Result, bail};
use geo::{Area, Buffer, Contains, Coord, LineString, MultiPolygon, Point, Polygon};
use spade::{ConstrainedDelaunayTriangulation, Point2, Triangulation};

#[cfg(test)]
use crate::heightfield::height_range_for_spec;
use crate::heightfield::{HeightField, normalized_height};
use crate::jigsaw::{EdgePattern, edge_noise, edge_sign, puzzle_edge_point, shared_edge_pattern};
use crate::mesh::{
    Mesh, PolygonStripIndex, distance_squared, point_in_polygon, point_line_distance,
    quantize_export_coordinate, unit_vector, weld_export_mesh,
};
use crate::mount::{
    mount_bottom, mount_bottom_across_outline, retention_bottom, split_outline_at_mount,
};
use crate::mount_layout::retention_centers_local;
use crate::outline::{clip_piece_outline, model_outline_mm};
use crate::planar_mesh::polygon_from_outline;
use crate::spec::{GenerationSpec, PrintMaterial, SurfaceClass};
use crate::surface::{SurfaceField, surface_area_bounds};
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
/// Keeps an automatically fitted flag socket clear of a puzzle edge after
/// the circle and outline are rounded onto the export coordinate grid.
const FLAG_EDGE_GAP_MM: f32 = 0.02;
/// Overlay footprint fragments below this area (mm^2) are unprintable dust
/// left over from boolean clipping and are dropped before shelling.
const MINIMUM_OVERLAY_AREA_MM2: f64 = 0.000_01;

struct FlagCavity {
    indices: Vec<usize>,
    ring: Vec<[f32; 2]>,
    depth_mm: f32,
}

mod buildings;
mod labels;
mod overlays;

use buildings::append_building_geometry;
use labels::append_label_geometry;
use overlays::{append_dot_geometry, append_road_geometry};

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
        local_piece_outline(spec, row, column)?
            .into_iter()
            .map(|[x, y]| [x + origin_x, y + origin_y])
            .collect()
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
    let mounted_piece_back = spec.wall_mount.cuts_terrain() && !spec.solid_model;
    let mount_frame = [
        -origin_x,
        -origin_y,
        assembled_width - origin_x,
        assembled_height - origin_y,
    ];
    let outline = if mounted_piece_back {
        split_outline_at_mount(&outline, &spec.wall_mount, mount_frame)?
    } else {
        outline
    };
    let mut points = outline
        .iter()
        .map(|point| Point2::new(point[0] as f64, point[1] as f64))
        .collect::<Vec<_>>();
    let mut point_keys = outline
        .iter()
        .enumerate()
        .map(|(index, point)| (triangulation_point_key(*point), index))
        .collect::<HashMap<_, _>>();
    let mut constraints = (0..outline.len())
        .map(|index| [index, (index + 1) % outline.len()])
        .collect::<Vec<_>>();
    let mut flag_cavities = Vec::<FlagCavity>::new();
    for marker in spec.markers.iter().filter(|marker| marker.kind.is_flag()) {
        let uv = spec.normalized_map_point(marker.latitude, marker.longitude);
        let requested_center = [
            uv[0] * assembled_width - origin_x,
            uv[1] * assembled_height - origin_y,
        ];
        let nominal_owner = nominal_flag_marker_piece(spec, uv);
        if !spec.solid_model
            && (row.abs_diff(nominal_owner.0) > 1 || column.abs_diff(nominal_owner.1) > 1)
        {
            continue;
        }
        if flag_marker_owner(spec, uv)? != (row, column) {
            continue;
        }
        let flag_style = marker.flag_style();
        let radius = flag_style.hole_diameter_mm * 0.5;
        let center =
            fit_flag_cavity_center(requested_center, radius, &outline).with_context(|| {
                format!("fit flag marker '{}' within its puzzle piece", marker.name)
            })?;
        let ring = flag_cavity_ring(center, radius);
        if ring.iter().any(|point| !point_in_polygon(*point, &outline)) {
            bail!(
                "flag marker '{}' could not fit clear of its terrain or puzzle-piece edge",
                marker.name
            );
        }
        let indices = ring
            .iter()
            .copied()
            .map(|point| push_unique_triangulation_point(&mut points, &mut point_keys, point))
            .collect::<Vec<_>>();
        constraints.extend(
            indices
                .iter()
                .zip(indices.iter().cycle().skip(1))
                .map(|(start, end)| [*start, *end]),
        );
        flag_cavities.push(FlagCavity {
            indices,
            ring,
            depth_mm: flag_style.hole_depth_mm,
        });
    }

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
    let outline_index = PolygonStripIndex::new(&outline, grid_rows.max(1))?;
    let shaped_outline = (spec.model_outline.shape != crate::spec::OutlineShape::Rectangle)
        .then(|| polygon_from_outline(&outline));
    let contains_outline = |point: [f32; 2]| {
        shaped_outline.as_ref().map_or_else(
            || outline_index.contains(point),
            |polygon| polygon.contains(&Point::new(f64::from(point[0]), f64::from(point[1]))),
        )
    };
    for grid_y in 0..grid_rows {
        let y = minimum_y + (grid_y as f32 + 0.5) * terrain_spacing;
        for grid_x in 0..grid_columns {
            let x = minimum_x + (grid_x as f32 + 0.5) * terrain_spacing;
            if contains_outline([x, y]) {
                push_unique_triangulation_point(&mut points, &mut point_keys, [x, y]);
            }
        }
    }
    let triangulation =
        ConstrainedDelaunayTriangulation::<Point2<f64>>::bulk_load_cdt(points, constraints)
            .context("triangulate terrain outline")?;
    let triangulation_indices = triangulation
        .vertices()
        .map(|vertex| {
            let point = vertex.position();
            (
                triangulation_point_key([point.x as f32, point.y as f32]),
                vertex.fix().index(),
            )
        })
        .collect::<HashMap<_, _>>();
    for cavity in &mut flag_cavities {
        cavity.indices = cavity
            .ring
            .iter()
            .map(|point| triangulation_indices[&triangulation_point_key(*point)])
            .collect();
    }
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
    let lower_side_z = if mounted_piece_back {
        spec.wall_mount.embedded_depth_mm()
    } else {
        0.0
    };
    for vertex in triangulation.vertices() {
        let position = vertex.position();
        vertices.push([position.x as f32, position.y as f32, lower_side_z]);
    }

    let mut top_triangles = Vec::with_capacity(triangulation.num_inner_faces());
    let mut top_materials = Vec::with_capacity(triangulation.num_inner_faces());
    let mut kept_faces = vec![false; triangulation.num_all_faces()];
    for face in triangulation.inner_faces() {
        let positions = face.vertices().map(|vertex| vertex.position());
        let centroid = [
            ((positions[0].x + positions[1].x + positions[2].x) / 3.0) as f32,
            ((positions[0].y + positions[1].y + positions[2].y) / 3.0) as f32,
        ];
        kept_faces[face.fix().index()] = contains_outline(centroid)
            && !flag_cavities
                .iter()
                .any(|cavity| point_in_polygon(centroid, &cavity.ring));
    }
    if shaped_outline.is_some() {
        // Dense curve intersections can leave a sliver at one constrained
        // vertex whose incident faces alternate inside and outside. Shed
        // only the smallest kept fan so the terrain wall remains manifold.
        repair_classification_pinches(&triangulation, &mut kept_faces, false);
    }
    for face in triangulation.inner_faces() {
        if !kept_faces[face.fix().index()] {
            continue;
        }
        let face_vertices = face.vertices();
        let positions = face_vertices.map(|vertex| vertex.position());
        let centroid = [
            ((positions[0].x + positions[1].x + positions[2].x) / 3.0) as f32,
            ((positions[0].y + positions[1].y + positions[2].y) / 3.0) as f32,
        ];
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
                    field.print_material_at(
                        (centroid[0] + origin_x) / assembled_width,
                        (centroid[1] + origin_y) / assembled_height,
                    )
                })
                .unwrap_or(PrintMaterial::Class(SurfaceClass::Rock)),
        );
    }

    // The surface class travels with the edge so a boundary edge — used by
    // exactly one triangle — can hand its own land cover to the wall below it.
    let mut edge_uses = HashMap::<(u32, u32), (u32, [u32; 2], PrintMaterial)>::new();
    for (triangle, material) in top_triangles.iter().zip(&top_materials) {
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
            let entry = edge_uses.entry(key).or_insert((0, directed, *material));
            entry.0 += 1;
        }
    }

    // Four wall triangles per boundary edge now: the bleed band and the cut
    // face below it.
    let mut triangles = Vec::with_capacity(top_triangles.len() * 2 + edge_uses.len() * 4);
    let mut materials = Vec::with_capacity(triangles.capacity());
    let retained_back = spec.puzzle_retention.active(spec.tray.enabled);
    let mounted_back = spec.wall_mount.cuts_terrain();
    let rebuilt_back = mounted_back || retained_back;
    for (top, material) in top_triangles.into_iter().zip(top_materials) {
        triangles.push(top);
        materials.push(material);
        if !rebuilt_back {
            triangles.push([
                top[0] + top_count as u32,
                top[2] + top_count as u32,
                top[1] + top_count as u32,
            ]);
            materials.push(SurfaceClass::Rock.into());
        }
    }
    // HashMap iteration order is randomized per process; sort the boundary
    // edges so the emitted mesh (and every artifact hashed from it) is
    // byte-for-byte reproducible across runs.
    let flag_edges = flag_cavities
        .iter()
        .flat_map(|cavity| {
            cavity
                .indices
                .iter()
                .zip(cavity.indices.iter().cycle().skip(1))
        })
        .map(|(start, end)| {
            let (start, end) = (*start as u32, *end as u32);
            if start < end {
                (start, end)
            } else {
                (end, start)
            }
        })
        .collect::<HashSet<_>>();
    let mut boundary_edges = edge_uses
        .into_values()
        .filter(|(uses, _, _)| *uses == 1)
        .map(|(_, edge, material)| (edge, material))
        .filter(|(edge, _)| {
            let key = if edge[0] < edge[1] {
                (edge[0], edge[1])
            } else {
                (edge[1], edge[0])
            };
            !flag_edges.contains(&key)
        })
        .collect::<Vec<_>>();
    boundary_edges.sort_unstable_by_key(|(edge, _)| *edge);
    let edge_bleed_mm = spec.color_output.edge_bleed_mm;
    // One bleed vertex per boundary vertex, not per boundary edge: two edges
    // meeting at a corner must land on the same point, or the wall gains a
    // T-junction and stops being watertight.
    let mut bleed_vertices = HashMap::<u32, u32>::new();
    for (edge, material) in boundary_edges {
        let [from, to] = edge;
        let from_bottom = from + top_count as u32;
        let to_bottom = to + top_count as u32;
        let from_bleed = bleed_vertex(
            &mut vertices,
            &mut bleed_vertices,
            from,
            top_count,
            lower_side_z,
            edge_bleed_mm,
        );
        let to_bleed = bleed_vertex(
            &mut vertices,
            &mut bleed_vertices,
            to,
            top_count,
            lower_side_z,
            edge_bleed_mm,
        );
        // The band under the rim, carrying the terrain color over the edge.
        triangles.push([from, to_bleed, to]);
        materials.push(material);
        triangles.push([from, from_bleed, to_bleed]);
        materials.push(material);
        // The cut face below it. Either end can bottom out against a wall
        // shorter than the bleed, which collapses that side of the quad.
        if to_bleed != to_bottom {
            triangles.push([from_bleed, to_bottom, to_bleed]);
            materials.push(SurfaceClass::Rock.into());
        }
        if from_bleed != from_bottom {
            triangles.push([from_bleed, from_bottom, to_bottom]);
            materials.push(SurfaceClass::Rock.into());
        }
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
    append_flag_cavities(&mut mesh, &flag_cavities, top_count, !rebuilt_back);
    if mounted_back {
        let bottom = if spec.solid_model {
            mount_bottom(
                &outline,
                &spec.wall_mount,
                [0.0, 0.0, piece_width, piece_height],
            )?
        } else {
            mount_bottom_across_outline(&outline, &spec.wall_mount, mount_frame)?
        };
        mesh.append_isolated(bottom);
    } else if retained_back {
        mesh.append_isolated(retention_bottom(
            &outline,
            &retention_centers_local(spec, row, column, &outline),
            &spec.puzzle_retention,
        )?);
    }
    let mut building_union = None;
    if (spec.buildings.enabled || spec.uses_building_markers())
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
    if ((spec.color_output.enabled && spec.color_output.roads_enabled)
        || spec.uses_trails()
        || spec.uses_rail_or_aerial()
        // Airport pavement is drawn by the same pass, and an airfield with
        // its roads switched off is still an airfield.
        || spec.uses_aviation())
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
    if spec.uses_dot_markers() {
        append_dot_geometry(
            &mut mesh,
            spec,
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
    if spec.uses_map_labels() {
        append_label_geometry(
            &mut mesh,
            spec,
            height_field,
            height_range,
            &outline,
            origin_x,
            origin_y,
            assembled_width,
            assembled_height,
        )?;
    }
    weld_export_mesh(&mut mesh);
    Ok(mesh)
}

/// The vertex where a piece's terrain color stops bleeding down the side wall
/// and the rock cut face takes over, directly below the top vertex `top`.
///
/// Returns the wall's own bottom vertex where the wall is shorter than the
/// bleed, so the whole of a short wall carries the surface color rather than
/// the bleed sinking below the model. Results are cached per top vertex: two
/// boundary edges meeting at a corner must share this point.
fn bleed_vertex(
    vertices: &mut Vec<[f32; 3]>,
    cache: &mut HashMap<u32, u32>,
    top: u32,
    top_count: usize,
    lower_side_z: f32,
    bleed_mm: f32,
) -> u32 {
    let point = vertices[top as usize];
    if bleed_mm <= 0.0 || point[2] - bleed_mm <= lower_side_z {
        return top + top_count as u32;
    }
    if let Some(index) = cache.get(&top) {
        return *index;
    }
    let index = vertices.len() as u32;
    vertices.push([point[0], point[1], point[2] - bleed_mm]);
    cache.insert(top, index);
    index
}

fn append_flag_cavities(
    mesh: &mut Mesh,
    cavities: &[FlagCavity],
    top_count: usize,
    close_bottom: bool,
) {
    for cavity in cavities {
        let ring = &cavity.indices;
        if ring.len() < 3 {
            continue;
        }
        let floor_z = ring
            .iter()
            .map(|index| mesh.vertices[*index][2])
            .fold(f32::INFINITY, f32::min)
            - cavity.depth_mm;
        let floor_start = mesh.vertices.len() as u32;
        let floor_vertices = ring
            .iter()
            .map(|index| {
                let point = mesh.vertices[*index];
                [point[0], point[1], floor_z]
            })
            .collect::<Vec<_>>();
        mesh.vertices.extend(floor_vertices);
        let center_index = mesh.vertices.len() as u32;
        let center = ring.iter().fold([0.0_f32; 2], |sum, index| {
            [
                sum[0] + mesh.vertices[*index][0],
                sum[1] + mesh.vertices[*index][1],
            ]
        });
        mesh.vertices.push([
            center[0] / ring.len() as f32,
            center[1] / ring.len() as f32,
            floor_z,
        ]);
        let bottom_center_index = if close_bottom {
            let index = mesh.vertices.len() as u32;
            mesh.vertices.push([
                center[0] / ring.len() as f32,
                center[1] / ring.len() as f32,
                0.0,
            ]);
            Some(index)
        } else {
            None
        };
        for index in 0..ring.len() {
            let next = (index + 1) % ring.len();
            let top_a = ring[index] as u32;
            let top_b = ring[next] as u32;
            let floor_a = floor_start + index as u32;
            let floor_b = floor_start + next as u32;
            mesh.triangles.push([top_a, top_b, floor_b]);
            mesh.materials.push(SurfaceClass::Rock.into());
            mesh.triangles.push([top_a, floor_b, floor_a]);
            mesh.materials.push(SurfaceClass::Rock.into());
            mesh.triangles.push([floor_a, floor_b, center_index]);
            mesh.materials.push(SurfaceClass::Rock.into());
            if let Some(bottom_center_index) = bottom_center_index {
                let bottom_a = top_a + top_count as u32;
                let bottom_b = top_b + top_count as u32;
                mesh.triangles
                    .push([bottom_a, bottom_center_index, bottom_b]);
                mesh.materials.push(SurfaceClass::Rock.into());
            }
        }
    }
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

/// Drops near-collinear ring points, one at a time, each judged against the
/// neighbors it has at the moment it is dropped. An earlier version removed
/// every qualifying point of a sweep at once, each judged against original
/// neighbors that were themselves being removed: on a densely sampled smooth
/// arc every interior point qualifies simultaneously, so whole arcs collapsed
/// into single chords millimetres from the boundary they replaced — a bridge
/// loop ramp became a filled quarter-disc. Removing sequentially keeps every
/// drop within tolerance of the edge that actually replaces it: once removals
/// stretch a chord far enough that its midpoint deviates past the tolerance,
/// the survivors stay.
fn simplify_closed_ring(mut points: Vec<[f32; 2]>) -> Vec<[f32; 2]> {
    let mut index = 0;
    let mut kept_in_a_row = 0;
    while points.len() > 3 && kept_in_a_row < points.len() {
        let count = points.len();
        let previous = points[(index + count - 1) % count];
        let point = points[index];
        let next = points[(index + 1) % count];
        let incoming = [point[0] - previous[0], point[1] - previous[1]];
        let outgoing = [next[0] - point[0], next[1] - point[1]];
        let continues_forward = incoming[0] * outgoing[0] + incoming[1] * outgoing[1] > 0.0;
        if continues_forward && point_line_distance(point, previous, next) <= 0.000_1 {
            points.remove(index);
            // The removal changed the neighbors of the points on either
            // side, so both get judged again before the sweep can settle.
            kept_in_a_row = 0;
            if index >= points.len() {
                index = 0;
            }
        } else {
            kept_in_a_row += 1;
            index = (index + 1) % points.len();
        }
    }
    points
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
    if spec.model_outline.shape != crate::spec::OutlineShape::Rectangle {
        return Ok(model_outline_mm(spec, edge_samples));
    }
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
    let model_boundary = (spec.model_outline.shape != crate::spec::OutlineShape::Rectangle)
        .then(|| polygon_from_outline(&model_outline_mm(spec, edge_samples)));

    let bottom_pattern = piece_edge_pattern(spec, 0, column, row);
    let bottom_sign = boundary_safe_edge_sign(
        model_boundary.as_ref(),
        bottom_left,
        bottom_right,
        bottom_pattern,
        puzzle_edge_sign(spec, 0, column, row, spec.rows),
        base_depth,
    );
    for index in 0..edge_samples {
        let t = index as f32 / edge_samples as f32;
        outline.push(puzzle_edge_point(
            bottom_left,
            bottom_right,
            bottom_pattern,
            bottom_sign,
            t,
            base_depth,
        ));
    }
    let right_pattern = piece_edge_pattern(spec, 1, row, column + 1);
    let right_sign = boundary_safe_edge_sign(
        model_boundary.as_ref(),
        bottom_right,
        top_right,
        right_pattern,
        puzzle_edge_sign(spec, 1, row, column + 1, spec.columns),
        base_depth,
    );
    for index in 0..edge_samples {
        let t = index as f32 / edge_samples as f32;
        outline.push(puzzle_edge_point(
            bottom_right,
            top_right,
            right_pattern,
            right_sign,
            t,
            base_depth,
        ));
    }
    let top_pattern = piece_edge_pattern(spec, 0, column, row + 1);
    let top_sign = boundary_safe_edge_sign(
        model_boundary.as_ref(),
        top_left,
        top_right,
        top_pattern,
        puzzle_edge_sign(spec, 0, column, row + 1, spec.rows),
        base_depth,
    );
    for index in 0..edge_samples {
        let t = 1.0 - index as f32 / edge_samples as f32;
        outline.push(puzzle_edge_point(
            top_left,
            top_right,
            top_pattern,
            top_sign,
            t,
            base_depth,
        ));
    }
    let left_pattern = piece_edge_pattern(spec, 1, row, column);
    let left_sign = boundary_safe_edge_sign(
        model_boundary.as_ref(),
        bottom_left,
        top_left,
        left_pattern,
        puzzle_edge_sign(spec, 1, row, column, spec.columns),
        base_depth,
    );
    for index in 0..edge_samples {
        let t = 1.0 - index as f32 / edge_samples as f32;
        outline.push(puzzle_edge_point(
            bottom_left,
            top_left,
            left_pattern,
            left_sign,
            t,
            base_depth,
        ));
    }

    if !exact_shared_edge && spec.clearance_mm > 0.0 {
        outline = inset_outline(&outline, spec.clearance_mm * 0.5)?;
    }
    Ok(outline)
}

fn boundary_safe_edge_sign(
    model_boundary: Option<&Polygon<f64>>,
    start: [f32; 2],
    end: [f32; 2],
    pattern: crate::jigsaw::EdgePattern,
    sign: f32,
    depth: f32,
) -> f32 {
    let Some(model_boundary) = model_boundary else {
        return sign;
    };
    if sign == 0.0 {
        return sign;
    }
    // A shaped boundary can cut a tab head away from its neck. Keep tabs
    // whose full shared curve lies inside the model; use a straight shared
    // seam near the outer cut so every exported puzzle piece stays whole.
    let stays_inside = (0..=32).all(|index| {
        let point = puzzle_edge_point(start, end, pattern, sign, index as f32 / 32.0, depth);
        model_boundary.contains(&Point::new(f64::from(point[0]), f64::from(point[1])))
    });
    if stays_inside { sign } else { 0.0 }
}

pub(crate) fn clipped_piece_outline(
    spec: &GenerationSpec,
    row: u32,
    column: u32,
    exact_shared_edge: bool,
) -> Result<Option<Vec<[f32; 2]>>> {
    if spec.model_outline.shape == crate::spec::OutlineShape::Rectangle {
        return piece_outline(spec, row, column, exact_shared_edge).map(Some);
    }
    let piece = piece_outline(spec, row, column, exact_shared_edge)?;
    let Some(clipped) = clip_piece_outline(spec, &piece)? else {
        return Ok(None);
    };
    Ok(Some(clipped))
}

pub(crate) fn printable_piece_positions(spec: &GenerationSpec) -> Result<Vec<(u32, u32)>> {
    if spec.solid_model {
        return Ok(vec![(0, 0)]);
    }
    let mut positions = Vec::new();
    for row in 0..spec.rows {
        for column in 0..spec.columns {
            if clipped_piece_outline(spec, row, column, false)?.is_some() {
                positions.push((row, column));
            }
        }
    }
    if positions.is_empty() {
        bail!("the model outline does not contain any printable puzzle pieces");
    }
    Ok(positions)
}

pub(crate) fn local_piece_outline(
    spec: &GenerationSpec,
    row: u32,
    column: u32,
) -> Result<Vec<[f32; 2]>> {
    let piece_width = spec.width_mm / spec.columns as f32;
    let piece_height = spec.height_mm() / spec.rows as f32;
    let origin_x = column as f32 * piece_width;
    let origin_y = row as f32 * piece_height;
    Ok(clipped_piece_outline(spec, row, column, false)?
        .with_context(|| {
            format!(
                "puzzle piece {}, {} lies outside the model outline",
                row + 1,
                column + 1
            )
        })?
        .into_iter()
        .map(|[x, y]| [x - origin_x, y - origin_y])
        .collect())
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
    let global_row = i64::from(spec.puzzle_tile_row) * i64::from(spec.rows) + i64::from(row);
    let global_column =
        i64::from(spec.puzzle_tile_column) * i64::from(spec.columns) + i64::from(column);
    let grid_key = ((global_row as u32 as u64) << 32) | global_column as u32 as u64;
    let seed = grid_key ^ (spec.puzzle_seed as u64).wrapping_mul(0xD6E8_FEB8_6659_FD93);
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
    let tile_edge = line == 0 || line == line_count;
    let inside_super_tile = tile_edge && super_tile_edge_has_neighbor(spec, orientation, line);
    let enabled = if !tile_edge {
        spec.puzzle_tabs
    } else if inside_super_tile {
        spec.adjacent_interlocks
    } else {
        spec.outer_edge_interlocks
    };
    if !enabled {
        return 0.0;
    }
    let (global_segment, global_line) = global_edge_key(spec, orientation, segment, line);
    edge_sign(spec.puzzle_seed, orientation, global_segment, global_line)
}

fn super_tile_edge_has_neighbor(spec: &GenerationSpec, orientation: u64, line: u32) -> bool {
    if orientation == 0 {
        (line == 0 && spec.adjacent_tile_row > 0)
            || (line == spec.rows && spec.adjacent_tile_row + 1 < spec.adjacent_rows)
    } else {
        (line == 0 && spec.adjacent_tile_column > 0)
            || (line == spec.columns && spec.adjacent_tile_column + 1 < spec.adjacent_columns)
    }
}

fn piece_edge_pattern(
    spec: &GenerationSpec,
    orientation: u64,
    segment: u32,
    line: u32,
) -> EdgePattern {
    let (global_segment, global_line) = global_edge_key(spec, orientation, segment, line);
    shared_edge_pattern(spec.puzzle_seed, orientation, global_line, global_segment)
}

fn global_edge_key(spec: &GenerationSpec, orientation: u64, segment: u32, line: u32) -> (i64, i64) {
    if orientation == 0 {
        (
            i64::from(spec.puzzle_tile_column) * i64::from(spec.columns) + i64::from(segment),
            i64::from(spec.puzzle_tile_row) * i64::from(spec.rows) + i64::from(line),
        )
    } else {
        (
            i64::from(spec.puzzle_tile_row) * i64::from(spec.rows) + i64::from(segment),
            i64::from(spec.puzzle_tile_column) * i64::from(spec.columns) + i64::from(line),
        )
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

fn flag_cavity_ring(center: [f32; 2], radius: f32) -> Vec<[f32; 2]> {
    (0..32)
        .map(|index| {
            let angle = index as f32 / 32.0 * std::f32::consts::TAU;
            [
                center[0] + radius * angle.cos(),
                center[1] + radius * angle.sin(),
            ]
        })
        .collect()
}

fn flag_marker_owner(spec: &GenerationSpec, uv: [f32; 2]) -> Result<(u32, u32)> {
    if spec.solid_model {
        return Ok((0, 0));
    }
    let point = [uv[0] * spec.width_mm, uv[1] * spec.height_mm()];
    let (nominal_row, nominal_column) = nominal_flag_marker_piece(spec, uv);
    let first_row = nominal_row.saturating_sub(1);
    let last_row = (nominal_row + 1).min(spec.rows - 1);
    let first_column = nominal_column.saturating_sub(1);
    let last_column = (nominal_column + 1).min(spec.columns - 1);
    for row in first_row..=last_row {
        for column in first_column..=last_column {
            if clipped_piece_outline(spec, row, column, true)?
                .is_some_and(|outline| point_in_polygon(point, &outline))
            {
                return Ok((row, column));
            }
        }
    }

    // A point exactly on a shared polygon edge can fall outside both sides
    // under floating-point containment. Give it one stable owner so the
    // socket cannot disappear into the fit-clearance gap.
    Ok((nominal_row, nominal_column))
}

fn nominal_flag_marker_piece(spec: &GenerationSpec, uv: [f32; 2]) -> (u32, u32) {
    let row = (uv[1].clamp(0.0, 1.0 - f32::EPSILON) * spec.rows as f32) as u32;
    let column = (uv[0].clamp(0.0, 1.0 - f32::EPSILON) * spec.columns as f32) as u32;
    (row, column)
}

/// Moves a flag socket only when its requested circle would cross the edge of
/// its owning piece. The closest point on an eroded copy of the piece is the
/// smallest deterministic correction that leaves the complete socket inside.
fn fit_flag_cavity_center(
    requested: [f32; 2],
    radius: f32,
    outline: &[[f32; 2]],
) -> Result<[f32; 2]> {
    let requested_ring = flag_cavity_ring(requested, radius);
    if requested_ring
        .iter()
        .all(|point| point_in_polygon(*point, outline))
    {
        return Ok(requested);
    }

    let safe_centers = inset_outline(outline, radius + FLAG_EDGE_GAP_MM)
        .context("this puzzle piece is too small for the flag socket")?;
    let mut closest = None::<([f32; 2], f32)>;
    for (start, end) in safe_centers.iter().zip(safe_centers.iter().cycle().skip(1)) {
        let segment = [end[0] - start[0], end[1] - start[1]];
        let length_squared = segment[0] * segment[0] + segment[1] * segment[1];
        if length_squared <= f32::EPSILON {
            continue;
        }
        let t = (((requested[0] - start[0]) * segment[0] + (requested[1] - start[1]) * segment[1])
            / length_squared)
            .clamp(0.0, 1.0);
        let candidate = [start[0] + segment[0] * t, start[1] + segment[1] * t];
        let distance_squared =
            (candidate[0] - requested[0]).powi(2) + (candidate[1] - requested[1]).powi(2);
        if closest.is_none_or(|(_, best)| distance_squared < best) {
            closest = Some((candidate, distance_squared));
        }
    }
    closest
        .map(|(point, _)| point)
        .context("this puzzle piece has no valid flag socket position")
}

pub(crate) fn scaled_building_height_mm(spec: &GenerationSpec, height_m: f32) -> f32 {
    if !spec.buildings.enabled && !spec.uses_building_markers() {
        return 0.0;
    }
    height_m * spec.width_mm / (spec.ground_span_km as f32 * 1_000.0) * spec.building_height_scale()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    use crate::mesh::assert_watertight;
    use crate::spec::{
        FlagMarkerStyle, MapMarker, MarkerKind, OutlineShape, PuzzleRetentionSpec, TraySpec,
        WallMountSpec, WallMountStyle, WallMountTarget,
    };

    #[test]
    fn a_solid_ellipse_is_watertight() {
        let mut spec = GenerationSpec {
            solid_model: true,
            width_mm: 80.0,
            rows: 2,
            columns: 3,
            samples_per_piece: 24,
            ..GenerationSpec::default()
        };
        spec.model_outline.shape = OutlineShape::Ellipse;
        let mesh = build_piece(&spec, None, None, 0, 0).unwrap();
        assert_watertight(&mesh);
    }

    #[test]
    fn a_concave_custom_solid_outline_is_watertight() {
        let mut spec = GenerationSpec {
            solid_model: true,
            width_mm: 80.0,
            rows: 4,
            columns: 4,
            samples_per_piece: 24,
            ..GenerationSpec::default()
        };
        spec.model_outline.shape = OutlineShape::Polygon;
        spec.model_outline.points = vec![
            [0.1, 0.1],
            [0.9, 0.1],
            [0.9, 0.42],
            [0.58, 0.42],
            [0.58, 0.9],
            [0.1, 0.9],
        ];
        spec.validate().unwrap();
        let mesh = build_piece(&spec, None, None, 0, 0).unwrap();
        assert_watertight(&mesh);
    }

    #[test]
    fn a_circle_omits_cells_that_fall_outside_its_boundary() {
        let mut spec = GenerationSpec {
            width_mm: 100.0,
            rows: 10,
            columns: 10,
            ..GenerationSpec::default()
        };
        spec.model_outline.shape = OutlineShape::Circle;
        let positions = printable_piece_positions(&spec).unwrap();
        assert!(positions.len() < 100);
        assert!(!positions.contains(&(0, 0)));
        assert!(positions.contains(&(5, 5)));
        let boundary = positions
            .into_iter()
            .find(|(row, column)| *row == 0 || *column == 0)
            .expect("circle should retain a boundary piece");
        let outline = local_piece_outline(&spec, boundary.0, boundary.1).unwrap();
        let unique_points = outline
            .iter()
            .map(|point| triangulation_point_key(*point))
            .collect::<HashSet<_>>();
        assert_eq!(
            unique_points.len(),
            outline.len(),
            "clipped outline repeats vertices"
        );
        let mut edge_counts = HashMap::new();
        for (start, end) in outline.iter().zip(outline.iter().cycle().skip(1)) {
            let mut edge = [
                triangulation_point_key(*start),
                triangulation_point_key(*end),
            ];
            edge.sort();
            *edge_counts.entry(edge).or_insert(0usize) += 1;
        }
        let repeated = edge_counts.values().filter(|count| **count > 1).count();
        assert_eq!(repeated, 0, "clipped outline repeats {repeated} edges");
        let mesh = build_piece(&spec, None, None, boundary.0, boundary.1).unwrap();
        assert_watertight(&mesh);
    }

    #[test]
    fn a_custom_polygon_clips_jigsaw_pieces_without_loose_parts() {
        let mut spec = GenerationSpec {
            width_mm: 90.0,
            rows: 5,
            columns: 5,
            samples_per_piece: 16,
            ..GenerationSpec::default()
        };
        spec.model_outline.shape = OutlineShape::Polygon;
        spec.model_outline.points = vec![
            [0.12, 0.2],
            [0.52, 0.08],
            [0.9, 0.3],
            [0.82, 0.84],
            [0.3, 0.92],
            [0.08, 0.58],
        ];
        spec.validate().unwrap();
        let positions = printable_piece_positions(&spec).unwrap();
        assert!(positions.len() < (spec.rows * spec.columns) as usize);
        assert!(!positions.is_empty());
        for (row, column) in positions {
            let mesh = build_piece(&spec, None, None, row, column).unwrap();
            assert_watertight(&mesh);
        }
    }

    /// Airport pavement reaches the mesh both ways — a ribbon down a runway
    /// centre line and an outline around an apron — and the piece stays
    /// closed with either or both on it.
    #[test]
    fn airport_pavement_meshes_watertight_as_lines_and_areas() {
        let mut spec = GenerationSpec {
            width_mm: 60.0,
            rows: 2,
            columns: 2,
            samples_per_piece: 24,
            color_output: crate::spec::ColorOutputSpec {
                enabled: true,
                ..crate::spec::ColorOutputSpec::default()
            },
            ..GenerationSpec::default()
        };
        spec.color_output.aviation.aviation_enabled = true;
        spec.validate().unwrap();

        let mut field = SurfaceField::new(3, 3, vec![SurfaceClass::Rock; 9], "airport").unwrap();
        // An apron with a terminal cut out of it, and a runway across it.
        field.paint_surface_area_with_holes(
            &[[0.1, 0.1], [0.9, 0.1], [0.9, 0.9], [0.1, 0.9]],
            &[vec![[0.4, 0.4], [0.6, 0.4], [0.6, 0.6], [0.4, 0.6]]],
            SurfaceClass::Aviation,
        );
        field.paint_polyline(&[[0.0, 0.5], [1.0, 0.5]], 60.0, 1.2, SurfaceClass::Aviation);

        let mesh = build_piece(&spec, None, Some(&field), 0, 0).unwrap();
        assert_watertight(&mesh);

        let pavement = mesh
            .materials
            .iter()
            .filter(|material| **material == SurfaceClass::Aviation)
            .count();
        assert!(pavement > 0, "no airport pavement reached the mesh");

        // Whether any pavement triangle actually covers a point, rather
        // than merely passing near it. Asking for vertices "close to" a
        // point proves nothing: a coarse triangle can straddle it with
        // every corner far away.
        let covered_by_pavement = |point: [f32; 2]| {
            mesh.triangles
                .iter()
                .zip(&mesh.materials)
                .filter(|(_, material)| **material == SurfaceClass::Aviation)
                .any(|(triangle, _)| {
                    let corners = triangle.map(|index| {
                        let vertex = mesh.vertices[index as usize];
                        [vertex[0], vertex[1]]
                    });
                    let side = |a: [f32; 2], b: [f32; 2]| {
                        (b[0] - a[0]) * (point[1] - a[1]) - (b[1] - a[1]) * (point[0] - a[0])
                    };
                    let (first, second, third) = (
                        side(corners[0], corners[1]),
                        side(corners[1], corners[2]),
                        side(corners[2], corners[0]),
                    );
                    (first >= 0.0 && second >= 0.0 && third >= 0.0)
                        || (first <= 0.0 && second <= 0.0 && third <= 0.0)
                })
        };
        // Piece (0, 0) of a 2x2 covers the map's first quarter, so both
        // sample points fall inside it: one on the apron, one in the hole
        // the terminal stands in.
        let at = |u: f32, v: f32| [u * spec.width_mm, v * spec.height_mm()];
        assert!(
            covered_by_pavement(at(0.25, 0.25)),
            "the apron itself should be paved"
        );
        assert!(
            !covered_by_pavement(at(0.45, 0.45)),
            "pavement covered the hole in the apron"
        );
    }

    /// A densely sampled smooth arc must survive simplification as an arc.
    /// An earlier sweep judged every point against neighbors that were
    /// themselves being removed in the same pass, so a whole arc could
    /// vanish at once: at SFO a bridge loop ramp's boundary collapsed into
    /// one 16 mm chord and the ground the loop enclosed flooded with deck.
    /// The sharp notch here plays the ramp's junction — the survivor the
    /// old sweep drew its chord from.
    #[test]
    fn simplifying_a_dense_arc_never_replaces_it_with_a_chord() {
        let radius = 4.0_f32;
        let step = 0.02_f32;
        let count = (radius * std::f32::consts::TAU / step) as usize;
        let mut ring = vec![[3.0, 0.0]];
        ring.extend((1..count).map(|index| {
            let angle = index as f32 / count as f32 * std::f32::consts::TAU;
            [radius * angle.cos(), radius * angle.sin()]
        }));
        let area_before = ring_signed_area(&ring).abs();
        let simplified = simplify_closed_ring(ring);
        // Chord-collapse does not merely roughen the outline — it walks the
        // boundary somewhere else entirely, so the enclosed area is the
        // honest measure. The old sweep left a sliver of the disc.
        let area_after = ring_signed_area(&simplified).abs();
        assert!(
            (area_before - area_after).abs() < 0.05,
            "simplification moved the ring's area from {area_before} to {area_after} mm2"
        );
    }

    /// Terminals and hangars carry `building=*` and go through the building
    /// pipeline. The aviation query never asks for them, so the only way
    /// they could be duplicated is by the apron swallowing them — and an
    /// apron is pavement everywhere except where a building stands.
    #[test]
    fn airport_buildings_keep_their_own_material_under_an_apron() {
        let mut spec = GenerationSpec {
            width_mm: 60.0,
            rows: 2,
            columns: 2,
            samples_per_piece: 24,
            buildings: crate::spec::BuildingSpec {
                enabled: true,
                z_scale: 1.0,
            },
            color_output: crate::spec::ColorOutputSpec {
                enabled: true,
                ..crate::spec::ColorOutputSpec::default()
            },
            ..GenerationSpec::default()
        };
        spec.color_output.aviation.aviation_enabled = true;
        spec.validate().unwrap();

        let mut field = SurfaceField::new(9, 9, vec![SurfaceClass::Rock; 81], "airport").unwrap();
        // An apron with the terminal's footprint cut out of it, and the
        // terminal itself standing in that gap.
        let terminal = [[0.15, 0.15], [0.3, 0.15], [0.3, 0.3], [0.15, 0.3]];
        field.paint_surface_area_with_holes(
            &[[0.05, 0.05], [0.45, 0.05], [0.45, 0.45], [0.05, 0.45]],
            &[terminal.to_vec()],
            SurfaceClass::Aviation,
        );
        field.paint_building(&terminal, 14.0);

        let mesh = build_piece(&spec, None, Some(&field), 0, 0).unwrap();
        assert_watertight(&mesh);

        let building_faces = mesh
            .materials
            .iter()
            .filter(|material| **material == SurfaceClass::Building)
            .count();
        assert!(
            building_faces > 0,
            "the terminal lost its building material to the pavement"
        );
        assert!(
            mesh.materials.contains(&SurfaceClass::Aviation.into()),
            "the apron itself should still be there"
        );
    }

    /// Every tile of a super-tile grid must treat the same pavement the same
    /// way: clipped to its own edge, watertight on both sides of the seam,
    /// and drawn in the same material.
    #[test]
    fn every_super_tile_part_clips_the_same_pavement_the_same_way() {
        let mut spec = GenerationSpec {
            width_mm: 60.0,
            rows: 2,
            columns: 2,
            samples_per_piece: 24,
            color_output: crate::spec::ColorOutputSpec {
                enabled: true,
                ..crate::spec::ColorOutputSpec::default()
            },
            ..GenerationSpec::default()
        };
        spec.color_output.aviation.aviation_enabled = true;
        spec.validate().unwrap();

        let mut field = SurfaceField::new(9, 9, vec![SurfaceClass::Rock; 81], "airport").unwrap();
        // A runway straight across the middle, so it crosses every seam.
        field.paint_polyline(&[[0.0, 0.5], [1.0, 0.5]], 60.0, 2.0, SurfaceClass::Aviation);
        // And an apron spanning the same seam.
        field.paint_surface_area(
            &[[0.3, 0.35], [0.7, 0.35], [0.7, 0.65], [0.3, 0.65]],
            SurfaceClass::Aviation,
        );

        let mut paved_parts = 0;
        for row in 0..spec.rows {
            for column in 0..spec.columns {
                let mesh = build_piece(&spec, None, Some(&field), row, column).unwrap();
                assert_watertight(&mesh);
                let paved = mesh
                    .materials
                    .iter()
                    .filter(|material| **material == SurfaceClass::Aviation)
                    .count();
                if paved > 0 {
                    paved_parts += 1;
                }
                // Nothing may hang outside the part it belongs to.
                let width = spec.width_mm / spec.columns as f32;
                let height = spec.height_mm() / spec.rows as f32;
                for vertex in &mesh.vertices {
                    assert!(
                        vertex[0] >= -width * 0.6 && vertex[0] <= width * 1.6,
                        "a vertex escaped its tile in x: {vertex:?}"
                    );
                    assert!(
                        vertex[1] >= -height * 0.6 && vertex[1] <= height * 1.6,
                        "a vertex escaped its tile in y: {vertex:?}"
                    );
                }
            }
        }
        assert_eq!(
            paved_parts, 4,
            "the runway and apron cross every tile, so every tile is paved"
        );
    }

    /// An airport can be all apron: a helipad on a hospital roof, a small
    /// field with unpaved strips OSM never drew as ways. The overlay pass
    /// used to return early whenever a piece held no road, rail, or trail
    /// LINE, which skipped areas with it — so that airport rendered nothing
    /// at all.
    #[test]
    fn an_airport_with_no_lines_still_draws_its_aprons() {
        let mut spec = GenerationSpec {
            width_mm: 60.0,
            rows: 2,
            columns: 2,
            samples_per_piece: 24,
            color_output: crate::spec::ColorOutputSpec {
                enabled: true,
                // No roads either, so the piece really has no lines at all.
                roads_enabled: false,
                rail_enabled: false,
                aerial_enabled: false,
                ferry_enabled: false,
                ..crate::spec::ColorOutputSpec::default()
            },
            ..GenerationSpec::default()
        };
        spec.color_output.aviation.aviation_enabled = true;
        spec.validate().unwrap();

        let mut field = SurfaceField::new(9, 9, vec![SurfaceClass::Rock; 81], "airport").unwrap();
        field.paint_surface_area(
            &[[0.05, 0.05], [0.45, 0.05], [0.45, 0.45], [0.05, 0.45]],
            SurfaceClass::Aviation,
        );

        let mesh = build_piece(&spec, None, Some(&field), 0, 0).unwrap();
        assert_watertight(&mesh);
        assert!(
            mesh.materials.contains(&SurfaceClass::Aviation.into()),
            "an apron with no line beside it still has to reach the mesh"
        );
    }

    /// A runway is flat across its width and not along its length. It
    /// rises and falls with the ground it is laid on — a runway that held
    /// one height would bury itself in the first hill it crossed — but a
    /// section cut across it is level, rather than draped over whichever
    /// two coarse samples happen to sit either side of it.
    #[test]
    fn a_runway_is_level_across_its_width_and_follows_the_ground_along_it() {
        // Ground that ripples along the strip and falls away across it.
        let samples = 32;
        let values_m = (0..samples)
            .flat_map(|y| {
                (0..samples).map(move |x| {
                    let u = x as f32 / (samples - 1) as f32;
                    let v = y as f32 / (samples - 1) as f32;
                    500.0 * (u * std::f32::consts::TAU * 1.5).sin() + 400.0 * v
                })
            })
            .collect();
        let height_field = HeightField::new(samples, samples, values_m, "rolling").unwrap();

        let mut spec = GenerationSpec {
            width_mm: 120.0,
            rows: 2,
            columns: 2,
            samples_per_piece: 48,
            color_output: crate::spec::ColorOutputSpec {
                enabled: true,
                ..crate::spec::ColorOutputSpec::default()
            },
            ..GenerationSpec::default()
        };
        spec.color_output.aviation.aviation_enabled = true;
        spec.validate().unwrap();

        let mut field = SurfaceField::new(3, 3, vec![SurfaceClass::Rock; 9], "airport").unwrap();
        // Well inside the first piece, so the whole width is on one tile.
        field.paint_polyline(
            &[[0.0, 0.25], [1.0, 0.25]],
            120.0,
            4.0,
            SurfaceClass::Aviation,
        );

        let mesh = build_piece(&spec, Some(&height_field), Some(&field), 0, 0).unwrap();
        assert_watertight(&mesh);
        let range = crate::heightfield::height_range_for_spec(&spec, Some(&height_field));

        // Only the top of the shell is laid flat; the bottom follows the
        // ground so the solid is never inside out.
        let mut columns = HashMap::<(i32, i32), f32>::new();
        for vertex in mesh
            .triangles
            .iter()
            .zip(&mesh.materials)
            .filter(|(_, material)| **material == SurfaceClass::Aviation)
            .flat_map(|(triangle, _)| triangle.iter().map(|i| mesh.vertices[*i as usize]))
        {
            let key = (
                (vertex[0] * 500.0).round() as i32,
                (vertex[1] * 500.0).round() as i32,
            );
            let top = columns.entry(key).or_insert(f32::NEG_INFINITY);
            *top = top.max(vertex[2]);
        }
        let mut tops = columns
            .into_iter()
            .map(|((x, y), z)| [x as f32 / 500.0, y as f32 / 500.0, z])
            .collect::<Vec<_>>();
        tops.sort_by(|a, b| a[0].total_cmp(&b[0]));
        assert!(
            tops.len() > 8,
            "not enough pavement to judge: {}",
            tops.len()
        );

        // Across the width, every point at one station shares a height.
        let mut worst_tilt = 0.0_f32;
        for window in tops.chunk_by(|a, b| (a[0] - b[0]).abs() < 0.002) {
            if window.len() < 2 {
                continue;
            }
            let high = window
                .iter()
                .map(|v| v[2])
                .fold(f32::NEG_INFINITY, f32::max);
            let low = window.iter().map(|v| v[2]).fold(f32::INFINITY, f32::min);
            worst_tilt = worst_tilt.max(high - low);
        }
        assert!(
            worst_tilt < 0.02,
            "cross-section should be level, tilts by {worst_tilt} mm"
        );

        // Along the length it tracks the ground, one surface height above
        // it, rather than holding a level of its own. A triangulated ribbon
        // carries its vertices on its edges rather than down the middle, so
        // each is judged against the ground at the centre line for its own
        // station — which is exactly the height a flat cross-section takes.
        let centre_y = spec.height_mm() * 0.25;
        let mut worst_drift = 0.0_f32;
        for top in &tops {
            let ground = terrain_z_at(
                &spec,
                Some(&height_field),
                range,
                top[0] / spec.width_mm,
                centre_y / spec.height_mm(),
            ) + spec.color_output.aviation.aviation_height_mm;
            worst_drift = worst_drift.max((top[2] - ground).abs());
        }
        assert!(
            worst_drift < 0.2,
            "the runway drifts {worst_drift} mm from the ground under its centre line"
        );

        // And it really does rise and fall: a runway pinned at one height
        // would pass the drift check only if the ground were flat, which
        // this one is not.
        let high = tops.iter().map(|t| t[2]).fold(f32::NEG_INFINITY, f32::max);
        let low = tops.iter().map(|t| t[2]).fold(f32::INFINITY, f32::min);
        assert!(
            high - low > 2.0,
            "the runway held one level across rolling ground: {low} to {high}"
        );
    }

    /// An apron far from any runway must sit on its own ground. The graded
    /// profile belongs to the strip it was measured along; letting it own
    /// the whole map floats a distant apron above the terrain, or sinks it
    /// under, by whatever the elevation differs by.
    #[test]
    fn a_distant_apron_follows_its_own_ground_not_a_far_off_runway() {
        // Ground that climbs hard from west to east.
        let samples = 32;
        let values_m = (0..samples)
            .flat_map(|y| {
                (0..samples).map(move |x| {
                    let _ = y;
                    x as f32 / (samples - 1) as f32 * 600.0
                })
            })
            .collect();
        let height_field = HeightField::new(samples, samples, values_m, "slope").unwrap();

        let mut spec = GenerationSpec {
            width_mm: 120.0,
            rows: 2,
            columns: 2,
            samples_per_piece: 32,
            color_output: crate::spec::ColorOutputSpec {
                enabled: true,
                ..crate::spec::ColorOutputSpec::default()
            },
            ..GenerationSpec::default()
        };
        spec.color_output.aviation.aviation_enabled = true;
        spec.validate().unwrap();

        let mut field = SurfaceField::new(9, 9, vec![SurfaceClass::Rock; 81], "airport").unwrap();
        // A short runway hard against the low western edge...
        field.paint_polyline(
            &[[0.02, 0.1], [0.02, 0.4]],
            120.0,
            2.0,
            SurfaceClass::Aviation,
        );
        // ...and an apron away east, where the ground is hundreds of
        // metres higher.
        field.paint_surface_area(
            &[[0.30, 0.10], [0.45, 0.10], [0.45, 0.40], [0.30, 0.40]],
            SurfaceClass::Aviation,
        );

        let mesh = build_piece(&spec, Some(&height_field), Some(&field), 0, 0).unwrap();
        assert_watertight(&mesh);

        // The ground climbs across the apron, so every point is judged
        // against the terrain under it rather than one figure for the lot.
        let range = crate::heightfield::height_range_for_spec(&spec, Some(&height_field));
        let mut columns = HashMap::<(i32, i32), f32>::new();
        for vertex in mesh
            .triangles
            .iter()
            .zip(&mesh.materials)
            .filter(|(_, material)| **material == SurfaceClass::Aviation)
            .flat_map(|(triangle, _)| triangle.iter().map(|i| mesh.vertices[*i as usize]))
            // Only the apron, well east of the runway.
            .filter(|vertex| vertex[0] > spec.width_mm * 0.25)
        {
            let key = (
                (vertex[0] * 50.0).round() as i32,
                (vertex[1] * 50.0).round() as i32,
            );
            let top = columns.entry(key).or_insert(f32::NEG_INFINITY);
            *top = top.max(vertex[2]);
        }
        assert!(
            !columns.is_empty(),
            "the apron never reached the mesh, so this proves nothing"
        );

        let mut worst = 0.0_f32;
        let mut worst_at = (0.0, 0.0);
        for ((x, y), top) in &columns {
            let point = [*x as f32 / 50.0, *y as f32 / 50.0];
            let ground = terrain_z_at(
                &spec,
                Some(&height_field),
                range,
                point[0] / spec.width_mm,
                point[1] / spec.height_mm(),
            ) + spec.color_output.aviation.aviation_height_mm;
            if (top - ground).abs() > worst {
                worst = (top - ground).abs();
                worst_at = (*top, ground);
            }
        }
        assert!(
            worst < 0.5,
            "the apron stands {worst} mm off its own ground ({} against {}) — it \
             took a far-off runway's graded height instead",
            worst_at.0,
            worst_at.1
        );
    }

    /// Guards the cost of grading. Every mesh vertex of the layer asks every
    /// aeroway line how far away it is, so a busy airport multiplies out. A
    /// real one — Heathrow has on the order of a hundred taxiway ways — must
    /// not turn a piece into a stall.
    #[test]
    fn grading_a_busy_airport_stays_quick() {
        let mut spec = GenerationSpec {
            width_mm: 120.0,
            rows: 2,
            columns: 2,
            samples_per_piece: 48,
            color_output: crate::spec::ColorOutputSpec {
                enabled: true,
                ..crate::spec::ColorOutputSpec::default()
            },
            ..GenerationSpec::default()
        };
        spec.color_output.aviation.aviation_enabled = true;
        spec.validate().unwrap();

        let mut field = SurfaceField::new(9, 9, vec![SurfaceClass::Rock; 81], "airport").unwrap();
        // A hundred taxiways in a grid, plus two runways across them.
        for index in 0..100 {
            let t = index as f32 / 99.0;
            field.paint_polyline(
                &[[0.05, 0.05 + t * 0.9], [0.95, 0.05 + t * 0.9]],
                120.0,
                0.6,
                SurfaceClass::Aviation,
            );
        }
        field.paint_polyline(
            &[[0.1, 0.3], [0.9, 0.3]],
            120.0,
            3.0,
            SurfaceClass::Aviation,
        );
        field.paint_polyline(
            &[[0.1, 0.7], [0.9, 0.7]],
            120.0,
            3.0,
            SurfaceClass::Aviation,
        );

        let started = std::time::Instant::now();
        let mesh = build_piece(&spec, None, Some(&field), 0, 0).unwrap();
        let elapsed = started.elapsed();
        assert_watertight(&mesh);
        assert!(
            mesh.materials.contains(&SurfaceClass::Aviation.into()),
            "the airport should have reached the mesh"
        );
        // Generous: this runs in a debug build on shared CI. It is here to
        // catch an order-of-magnitude regression, not to hold a budget.
        assert!(
            elapsed.as_secs() < 60,
            "grading a hundred-way airport took {elapsed:?}"
        );
        println!("grading a 102-way airport: {elapsed:?}");
    }

    /// A runway is not a flat slab dropped on a hill: real ones follow the
    /// ground. Any pavement whose top sits below the terrain under it has
    /// been buried — it clips into the tile and simply is not there to see.
    #[test]
    fn airport_pavement_never_sinks_into_the_terrain() {
        let samples = 32;
        let values_m = (0..samples)
            .flat_map(|y| {
                (0..samples).map(move |x| {
                    let u = x as f32 / (samples - 1) as f32;
                    let v = y as f32 / (samples - 1) as f32;
                    // A ridge across the middle, so any surface that does
                    // not follow the ground dives straight through it.
                    400.0 * (u * std::f32::consts::TAU).sin() + 300.0 * (v - 0.5).abs()
                })
            })
            .collect();
        let height_field = HeightField::new(samples, samples, values_m, "ridge").unwrap();

        let mut spec = GenerationSpec {
            width_mm: 120.0,
            rows: 2,
            columns: 2,
            samples_per_piece: 40,
            color_output: crate::spec::ColorOutputSpec {
                enabled: true,
                ..crate::spec::ColorOutputSpec::default()
            },
            ..GenerationSpec::default()
        };
        spec.color_output.aviation.aviation_enabled = true;
        spec.validate().unwrap();

        let mut field = SurfaceField::new(9, 9, vec![SurfaceClass::Rock; 81], "airport").unwrap();
        field.paint_polyline(
            &[[0.0, 0.5], [1.0, 0.5]],
            120.0,
            3.0,
            SurfaceClass::Aviation,
        );
        field.paint_surface_area(
            &[[0.10, 0.10], [0.40, 0.10], [0.40, 0.35], [0.10, 0.35]],
            SurfaceClass::Aviation,
        );

        let mesh = build_piece(&spec, Some(&height_field), Some(&field), 0, 0).unwrap();
        assert_watertight(&mesh);
        let range = crate::heightfield::height_range_for_spec(&spec, Some(&height_field));

        // The highest pavement in each column, against the ground there.
        let mut columns = HashMap::<(i32, i32), f32>::new();
        for vertex in mesh
            .triangles
            .iter()
            .zip(&mesh.materials)
            .filter(|(_, material)| **material == SurfaceClass::Aviation)
            .flat_map(|(triangle, _)| triangle.iter().map(|i| mesh.vertices[*i as usize]))
        {
            let key = (
                (vertex[0] * 20.0).round() as i32,
                (vertex[1] * 20.0).round() as i32,
            );
            let top = columns.entry(key).or_insert(f32::NEG_INFINITY);
            *top = top.max(vertex[2]);
        }
        assert!(!columns.is_empty(), "no pavement to judge");

        let mut worst_buried = 0.0_f32;
        for ((x, y), top) in &columns {
            let point = [*x as f32 / 20.0, *y as f32 / 20.0];
            let ground = terrain_z_at(
                &spec,
                Some(&height_field),
                range,
                point[0] / spec.width_mm,
                point[1] / spec.height_mm(),
            );
            worst_buried = worst_buried.max(ground - top);
        }
        assert!(
            worst_buried < 0.05,
            "pavement is buried {worst_buried} mm under the terrain it crosses"
        );
    }

    /// Buried pavement is not pavement — it is a hole in the model where a
    /// runway should be. A strip laid flat across a side slope buries its
    /// uphill edge unless the level is taken from the high side, so this
    /// runs a runway and a taxiway straight along a hillside, across the
    /// fall line, which is the case that breaks it.
    #[test]
    fn nothing_airside_is_ever_buried_on_a_cross_slope() {
        let samples = 32;
        let values_m = (0..samples)
            .flat_map(|y| {
                (0..samples).map(move |x| {
                    let _ = x;
                    // Ground that falls steadily across the strips.
                    y as f32 / (samples - 1) as f32 * 900.0
                })
            })
            .collect();
        let height_field = HeightField::new(samples, samples, values_m, "side slope").unwrap();

        let mut spec = GenerationSpec {
            width_mm: 120.0,
            rows: 2,
            columns: 2,
            samples_per_piece: 48,
            color_output: crate::spec::ColorOutputSpec {
                enabled: true,
                ..crate::spec::ColorOutputSpec::default()
            },
            ..GenerationSpec::default()
        };
        spec.color_output.aviation.aviation_enabled = true;
        spec.validate().unwrap();

        let mut field = SurfaceField::new(3, 3, vec![SurfaceClass::Rock; 9], "airport").unwrap();
        // A wide runway and a narrow taxiway, both along the contour, so
        // every point of each has ground rising on one side of it.
        field.paint_polyline(
            &[[0.0, 0.2], [1.0, 0.2]],
            120.0,
            4.0,
            SurfaceClass::Aviation,
        );
        field.paint_polyline(
            &[[0.0, 0.35], [1.0, 0.35]],
            120.0,
            0.8,
            SurfaceClass::Aviation,
        );

        let mesh = build_piece(&spec, Some(&height_field), Some(&field), 0, 0).unwrap();
        assert_watertight(&mesh);
        let range = crate::heightfield::height_range_for_spec(&spec, Some(&height_field));

        let mut columns = HashMap::<(i32, i32), f32>::new();
        for vertex in mesh
            .triangles
            .iter()
            .zip(&mesh.materials)
            .filter(|(_, material)| **material == SurfaceClass::Aviation)
            .flat_map(|(triangle, _)| triangle.iter().map(|i| mesh.vertices[*i as usize]))
        {
            let key = (
                (vertex[0] * 100.0).round() as i32,
                (vertex[1] * 100.0).round() as i32,
            );
            let top = columns.entry(key).or_insert(f32::NEG_INFINITY);
            *top = top.max(vertex[2]);
        }
        assert!(columns.len() > 8, "no pavement to judge");

        let mut worst_buried = 0.0_f32;
        for ((x, y), top) in &columns {
            let ground = terrain_z_at(
                &spec,
                Some(&height_field),
                range,
                *x as f32 / 100.0 / spec.width_mm,
                *y as f32 / 100.0 / spec.height_mm(),
            );
            worst_buried = worst_buried.max(ground - top);
        }
        assert!(
            worst_buried < 0.02,
            "pavement is buried {worst_buried} mm under the hillside it crosses"
        );
    }

    /// The case a real airport is: flat ground, a small elevation range,
    /// and the full relief height spent on it, so a metre of DEM noise
    /// becomes millimetres of print — many times the pavement's own 0.2 mm.
    /// Strips run close together too, so the nearest centre line to a point
    /// is often not the one whose ribbon it lies in. Neither may bury it.
    ///
    /// Numbers taken from the saved San Francisco setup: 4.5 km across
    /// 180 mm, 28 mm of relief over ground that barely moves.
    /// The case a real airport is: flat ground, a small elevation range, and
    /// the full relief height spent on it, so a metre of DEM noise becomes
    /// millimetres of print — many times the pavement's own 0.2 mm. Ground
    /// like this swallows a surface that cannot follow it closely.
    ///
    /// Numbers from the saved San Francisco setup: 4.5 km across 180 mm,
    /// 28 mm of relief over ground that barely moves.
    #[test]
    fn a_flat_noisy_airfield_at_full_relief_buries_nothing() {
        let samples = 64;
        let values_m = (0..samples)
            .flat_map(|y| {
                (0..samples).map(move |x| {
                    // Airfield ground: a couple of metres of relief, and
                    // sample-to-sample noise of about a metre on top.
                    let drift = (x as f32 / samples as f32) * 3.0;
                    let noise = if (x * 7 + y * 13) % 5 < 2 { 1.2 } else { 0.0 };
                    drift + noise
                })
            })
            .collect();
        let height_field = HeightField::new(samples, samples, values_m, "airfield").unwrap();

        let mut spec = GenerationSpec {
            center_lat: 37.61847,
            center_lon: -122.37651,
            ground_span_km: 4.5,
            width_mm: 180.0,
            relief_mm: 28.0,
            base_mm: 3.2,
            rows: 2,
            columns: 2,
            samples_per_piece: 64,
            despike_terrain: true,
            color_output: crate::spec::ColorOutputSpec {
                enabled: true,
                ..crate::spec::ColorOutputSpec::default()
            },
            ..GenerationSpec::default()
        };
        spec.color_output.aviation.aviation_enabled = true;
        spec.validate().unwrap();

        let mut field = SurfaceField::new(3, 3, vec![SurfaceClass::Rock; 9], "airport").unwrap();
        // Parallel strips close together, wide and narrow, the way an
        // airfield lays them out, plus a taxiway crossing them all.
        for (offset, width) in [(0.20, 3.0), (0.26, 0.8), (0.32, 3.0), (0.36, 0.8)] {
            field.paint_polyline(
                &[[0.02, offset], [0.98, offset]],
                180.0,
                width,
                SurfaceClass::Aviation,
            );
        }
        field.paint_polyline(
            &[[0.50, 0.15], [0.50, 0.42]],
            180.0,
            0.8,
            SurfaceClass::Aviation,
        );
        field.paint_surface_area(
            &[[0.60, 0.16], [0.90, 0.16], [0.90, 0.40], [0.60, 0.40]],
            SurfaceClass::Aviation,
        );

        let mesh = build_piece(&spec, Some(&height_field), Some(&field), 0, 0).unwrap();
        assert_watertight(&mesh);
        let range = crate::heightfield::height_range_for_spec(&spec, Some(&height_field));

        let mut columns = HashMap::<(i32, i32), f32>::new();
        for vertex in mesh
            .triangles
            .iter()
            .zip(&mesh.materials)
            .filter(|(_, material)| **material == SurfaceClass::Aviation)
            .flat_map(|(triangle, _)| triangle.iter().map(|i| mesh.vertices[*i as usize]))
        {
            let key = (
                (vertex[0] * 200.0).round() as i32,
                (vertex[1] * 200.0).round() as i32,
            );
            let top = columns.entry(key).or_insert(f32::NEG_INFINITY);
            *top = top.max(vertex[2]);
        }
        assert!(columns.len() > 50, "not enough pavement: {}", columns.len());

        // Sampling the pavement's own vertices proves nothing: every one of
        // them clears the ground by construction. The ground rises through
        // the SURFACE BETWEEN them, so the surface is what has to be asked.
        let faces = mesh
            .triangles
            .iter()
            .zip(&mesh.materials)
            .filter(|(_, material)| **material == SurfaceClass::Aviation)
            .map(|(triangle, _)| triangle.map(|index| mesh.vertices[index as usize]))
            .collect::<Vec<_>>();
        // Height of the pavement's upper surface at a point, or None where
        // there is no pavement.
        let pavement_top_at = |point: [f32; 2]| {
            let mut best = None::<f32>;
            for face in &faces {
                let side = |a: [f32; 3], b: [f32; 3]| {
                    (b[0] - a[0]) * (point[1] - a[1]) - (b[1] - a[1]) * (point[0] - a[0])
                };
                let (first, second, third) = (
                    side(face[0], face[1]),
                    side(face[1], face[2]),
                    side(face[2], face[0]),
                );
                let inside = (first >= 0.0 && second >= 0.0 && third >= 0.0)
                    || (first <= 0.0 && second <= 0.0 && third <= 0.0);
                if !inside {
                    continue;
                }
                let total = first + second + third;
                if total.abs() <= f32::EPSILON {
                    continue;
                }
                let z = (second * face[0][2] + third * face[1][2] + first * face[2][2]) / total;
                best = Some(best.map_or(z, |current: f32| current.max(z)));
            }
            best
        };

        let mut worst_buried = 0.0_f32;
        let mut checked = 0;
        let steps = 160;
        for row in 0..=steps {
            for column in 0..=steps {
                let point = [
                    column as f32 / steps as f32 * spec.width_mm * 0.5,
                    row as f32 / steps as f32 * spec.height_mm() * 0.5,
                ];
                let Some(top) = pavement_top_at(point) else {
                    continue;
                };
                let ground = terrain_z_at(
                    &spec,
                    Some(&height_field),
                    range,
                    point[0] / spec.width_mm,
                    point[1] / spec.height_mm(),
                );
                worst_buried = worst_buried.max(ground - top);
                checked += 1;
            }
        }
        assert!(checked > 200, "only {checked} samples landed on pavement");
        // A tenth of the height the pavement stands proud. Not zero: the
        // surface is triangulated, so the ground can still cross it by a
        // fraction of one triangle's own span.
        assert!(
            worst_buried < 0.1,
            "the ground rises {worst_buried} mm through the pavement between its \
             own vertices, over {checked} samples"
        );
    }

    /// The pavement stands at its own height, not the road layer's.
    ///
    /// Measured as the difference between two runs rather than against the
    /// terrain's maximum: a graded runway deliberately does not follow the
    /// ground under it, so "higher than the tallest terrain" would be
    /// asking the wrong question.
    #[test]
    fn airport_pavement_uses_its_own_surface_height() {
        let pavement_top = |aviation_height_mm: f32| {
            let mut spec = GenerationSpec {
                width_mm: 60.0,
                rows: 2,
                columns: 2,
                samples_per_piece: 24,
                color_output: crate::spec::ColorOutputSpec {
                    enabled: true,
                    road_height_mm: 0.2,
                    ..crate::spec::ColorOutputSpec::default()
                },
                ..GenerationSpec::default()
            };
            spec.color_output.aviation.aviation_enabled = true;
            spec.color_output.aviation.aviation_height_mm = aviation_height_mm;
            spec.validate().unwrap();

            let mut field =
                SurfaceField::new(3, 3, vec![SurfaceClass::Rock; 9], "airport").unwrap();
            field.paint_polyline(&[[0.0, 0.5], [1.0, 0.5]], 60.0, 1.2, SurfaceClass::Aviation);
            let mesh = build_piece(&spec, None, Some(&field), 0, 0).unwrap();
            assert_watertight(&mesh);
            mesh.triangles
                .iter()
                .zip(&mesh.materials)
                .filter(|(_, material)| **material == SurfaceClass::Aviation)
                .flat_map(|(triangle, _)| triangle.iter().map(|i| mesh.vertices[*i as usize][2]))
                .fold(f32::NEG_INFINITY, f32::max)
        };

        let thin = pavement_top(0.2);
        let thick = pavement_top(0.6);
        assert!(
            (thick - thin - 0.4).abs() < 0.01,
            "0.4 mm more pavement height should raise the top 0.4 mm; \
             {thin} became {thick}"
        );
    }

    #[test]
    fn terrain_color_bleeds_over_the_piece_edge_before_the_rock_cut_face() {
        // Rolling terrain, so the piece's walls vary from shallow to steep
        // rather than all standing at one height.
        let samples = 16;
        let values_m = (0..samples)
            .flat_map(|y| {
                (0..samples).map(move |x| {
                    let u = x as f32 / (samples - 1) as f32;
                    let v = y as f32 / (samples - 1) as f32;
                    900.0
                        + 600.0 * (u * std::f32::consts::TAU).sin()
                        + 400.0 * (v * std::f32::consts::TAU * 1.5).cos()
                })
            })
            .collect();
        let height_field = HeightField::new(samples, samples, values_m, "hills").unwrap();
        let field = SurfaceField::new(3, 3, vec![SurfaceClass::Forest; 9], "forest").unwrap();
        let spec = GenerationSpec {
            width_mm: 60.0,
            rows: 2,
            columns: 2,
            samples_per_piece: 16,
            color_output: crate::spec::ColorOutputSpec {
                enabled: true,
                ..crate::spec::ColorOutputSpec::default()
            },
            ..GenerationSpec::default()
        };
        spec.validate().unwrap();
        let mesh = build_piece(&spec, Some(&height_field), Some(&field), 0, 0).unwrap();
        assert_watertight(&mesh);

        let bleed = spec.color_output.edge_bleed_mm;
        // Every (x, y) column of the mesh is topped by its terrain vertex, so
        // the highest vertex at each position is the visible surface. This
        // holds whatever the terrain does underneath.
        let mut column_top = HashMap::<(i32, i32), f32>::new();
        for vertex in &mesh.vertices {
            let key = (
                (vertex[0] * 1_000.0).round() as i32,
                (vertex[1] * 1_000.0).round() as i32,
            );
            let entry = column_top.entry(key).or_insert(f32::NEG_INFINITY);
            *entry = entry.max(vertex[2]);
        }
        let is_surface = |index: &u32| {
            let vertex = mesh.vertices[*index as usize];
            let key = (
                (vertex[0] * 1_000.0).round() as i32,
                (vertex[1] * 1_000.0).round() as i32,
            );
            (column_top[&key] - vertex[2]).abs() < 0.0005
        };

        // The bug: rock reaching the surface draws a grey outline around the
        // piece as soon as it is seen from an angle.
        let rock_at_the_rim = mesh
            .triangles
            .iter()
            .zip(&mesh.materials)
            .filter(|(triangle, material)| {
                **material == SurfaceClass::Rock && triangle.iter().any(is_surface)
            })
            .count();
        assert_eq!(rock_at_the_rim, 0, "rock still shows at the piece edge");

        // The bleed is a band, not a repaint: the cut face below it stays rock.
        let rock_wall = mesh
            .triangles
            .iter()
            .zip(&mesh.materials)
            .filter(|(triangle, material)| {
                **material == SurfaceClass::Rock
                    && triangle
                        .iter()
                        .any(|index| mesh.vertices[*index as usize][2] > 0.001)
            })
            .count();
        assert!(rock_wall > 0, "the cut face lost its rock");

        // And the band really is on the wall, not just the top face. Every
        // bleed vertex sits exactly one band below the surface above it.
        let forest_wall = mesh
            .triangles
            .iter()
            .zip(&mesh.materials)
            .filter(|(triangle, material)| {
                **material == SurfaceClass::Forest
                    && triangle.iter().any(|index| !is_surface(index))
            })
            .count();
        assert!(forest_wall > 0, "the surface color never left the top face");
        let bleed_vertices = mesh
            .vertices
            .iter()
            .filter(|vertex| {
                let key = (
                    (vertex[0] * 1_000.0).round() as i32,
                    (vertex[1] * 1_000.0).round() as i32,
                );
                (column_top[&key] - vertex[2] - bleed).abs() < 0.0005
            })
            .count();
        assert!(bleed_vertices > 0, "no vertex sits at the bleed depth");
    }

    /// A mounted back raises the wall's floor, and the mount check only
    /// guarantees 0.4 mm of wall under the cut, while the bleed goes to 2 mm.
    /// So a flat shoreline under a deep cleat really does run out of wall,
    /// and the clamp in `bleed_vertex` is reachable, not defensive.
    ///
    /// All three regimes have to stay closed: wall to spare, a wall shorter
    /// than the bleed everywhere, and the mixed case where one end of a wall
    /// clamps and the other does not.
    #[test]
    fn a_wall_squeezed_to_the_bleed_depth_still_closes() {
        let field = SurfaceField::new(3, 3, vec![SurfaceClass::Forest; 9], "forest").unwrap();
        let flat = HeightField::new(3, 3, vec![0.0; 9], "shoreline").unwrap();
        // Relief low enough that the terrain crosses the bleed threshold part
        // way up, so a single wall has clamped and unclamped ends.
        let rolling = HeightField::new(
            3,
            3,
            vec![0.0, 40.0, 0.0, 40.0, 80.0, 40.0, 0.0, 40.0, 0.0],
            "low rise",
        )
        .unwrap();
        let mut regimes = Vec::new();
        for (depth_mm, height_field, relief_mm) in [
            (0.4_f32, &flat, 28.0_f32),
            (2.4, &flat, 28.0),
            (2.4, &rolling, 1.0),
        ] {
            let spec = GenerationSpec {
                width_mm: 80.0,
                rows: 2,
                columns: 2,
                relief_mm,
                wall_mount: WallMountSpec {
                    style: WallMountStyle::FrenchCleat,
                    target: WallMountTarget::Terrain,
                    depth_mm,
                    thickness_mm: 1.2,
                    wall_offset_mm: 0.8,
                    ..WallMountSpec::default()
                },
                color_output: crate::spec::ColorOutputSpec {
                    enabled: true,
                    // Twice the default, so the bleed outruns the wall a
                    // mounted back leaves behind.
                    edge_bleed_mm: 0.8,
                    ..crate::spec::ColorOutputSpec::default()
                },
                ..GenerationSpec::default()
            };
            spec.validate().unwrap();
            let mesh = build_piece(&spec, Some(height_field), Some(&field), 0, 0).unwrap();
            assert_watertight(&mesh);

            // Which regime this actually was, so the test cannot quietly stop
            // reaching the clamp if the mount bounds ever move.
            let bleed = spec.color_output.edge_bleed_mm;
            let floor = spec.wall_mount.embedded_depth_mm();
            let mut column_top = HashMap::<(i32, i32), f32>::new();
            for vertex in &mesh.vertices {
                let key = (
                    (vertex[0] * 1_000.0).round() as i32,
                    (vertex[1] * 1_000.0).round() as i32,
                );
                let entry = column_top.entry(key).or_insert(f32::NEG_INFINITY);
                *entry = entry.max(vertex[2]);
            }
            let clamped = column_top
                .values()
                .filter(|top| **top - floor <= bleed)
                .count();
            regimes.push((clamped, column_top.len() - clamped));
        }
        assert!(regimes[0].0 == 0, "expected no clamping, got {regimes:?}");
        assert!(regimes[1].1 == 0, "expected full clamping, got {regimes:?}");
        assert!(
            regimes[2].0 > 0 && regimes[2].1 > 0,
            "expected a mix of clamped and unclamped walls, got {regimes:?}"
        );
    }

    #[test]
    fn flag_marker_cuts_a_watertight_blind_socket() {
        let spec = GenerationSpec {
            solid_model: true,
            samples_per_piece: 32,
            markers: vec![MapMarker {
                name: "Home".into(),
                latitude: 46.8523,
                longitude: -121.7603,
                kind: MarkerKind::FlagHole,
                label_height_mm: 4.0,
                rotation_degrees: 0.0,
                dot_style: None,
                flag_style: Some(FlagMarkerStyle {
                    hole_diameter_mm: 3.6,
                    hole_depth_mm: 1.2,
                    ..FlagMarkerStyle::default()
                }),
                label_style: None,
            }],
            ..GenerationSpec::default()
        };
        spec.validate().unwrap();
        let mesh = build_piece(&spec, None, None, 0, 0).unwrap();
        assert_watertight(&mesh);
        assert!(mesh.vertices.iter().any(|vertex| {
            (vertex[0] - spec.width_mm * 0.5).abs() < 0.001
                && (vertex[1] - spec.height_mm() * 0.5).abs() < 0.001
                && vertex[2] > 0.4
                && vertex[2] < spec.base_mm + spec.relief_mm
        }));
        let cavity_edge_vertices = mesh
            .vertices
            .iter()
            .filter(|vertex| {
                let radius =
                    (vertex[0] - spec.width_mm * 0.5).hypot(vertex[1] - spec.height_mm() * 0.5);
                (radius - 1.8).abs() < 0.001 && vertex[2] > 0.4
            })
            .collect::<Vec<_>>();
        assert!(cavity_edge_vertices.iter().any(|top| {
            cavity_edge_vertices.iter().any(|bottom| {
                (top[0] - bottom[0]).abs() < 0.001
                    && (top[1] - bottom[1]).abs() < 0.001
                    && (top[2] - bottom[2] - 1.2).abs() < 0.001
            })
        }));
    }

    #[test]
    fn flag_marker_from_failed_piece_6_7_builds() {
        let spec = GenerationSpec {
            center_lat: 46.8523,
            center_lon: -121.7603,
            ground_span_km: 2.25,
            width_mm: 180.0,
            rows: 10,
            columns: 10,
            samples_per_piece: 64,
            mesh_samples_across: Some(640),
            puzzle_seed: 3_372_996_238,
            markers: vec![MapMarker {
                name: "Flag 1".into(),
                latitude: 46.853_696_811_101_67,
                longitude: -121.756_909_687_805_2,
                kind: MarkerKind::FlagHole,
                label_height_mm: 4.0,
                rotation_degrees: 0.0,
                dot_style: None,
                flag_style: None,
                label_style: None,
            }],
            ..GenerationSpec::default()
        };

        spec.validate().unwrap();
        let uv = spec.normalized_map_point(spec.markers[0].latitude, spec.markers[0].longitude);
        assert_eq!(flag_marker_owner(&spec, uv).unwrap(), (5, 6));
        let piece_width = spec.width_mm / spec.columns as f32;
        let piece_height = spec.height_mm() / spec.rows as f32;
        let requested = [
            uv[0] * spec.width_mm - 6.0 * piece_width,
            uv[1] * spec.height_mm() - 5.0 * piece_height,
        ];
        let outline = local_piece_outline(&spec, 5, 6).unwrap();
        let radius = spec.markers[0].flag_style().hole_diameter_mm * 0.5;
        let fitted = fit_flag_cavity_center(requested, radius, &outline).unwrap();
        let shift = (fitted[0] - requested[0]).hypot(fitted[1] - requested[1]);
        assert!(shift > 0.0 && shift < spec.markers[0].flag_style().hole_diameter_mm);
        assert!(
            flag_cavity_ring(fitted, radius)
                .iter()
                .all(|point| point_in_polygon(*point, &outline))
        );
        let mesh = build_piece(&spec, None, None, 5, 6).unwrap();
        assert_watertight(&mesh);
    }

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
            adjacent_columns: 2,
            adjacent_rows: 1,
            adjacent_interlocks: true,
            adjacent_tile_column: 0,
            puzzle_seed: 0x1234_5678,
            puzzle_tile_column: -3,
            puzzle_tile_row: 5,
            ..GenerationSpec::default()
        };
        let right_spec = GenerationSpec {
            adjacent_tile_column: 1,
            puzzle_tile_column: left_spec.puzzle_tile_column + 1,
            ..left_spec.clone()
        };
        let row = 1;
        let left = piece_outline(&left_spec, row, left_spec.columns - 1, true).unwrap();
        let right = piece_outline(&right_spec, row, 0, true)
            .unwrap()
            .into_iter()
            .map(|point| [point[0] + left_spec.width_mm, point[1]])
            .collect::<Vec<_>>();

        let edge_samples = left_spec.samples_per_piece as usize;
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
        let outer_plain = piece_outline(&left_spec, 0, 0, true).unwrap();
        assert!(
            outer_plain[..edge_samples]
                .iter()
                .all(|point| point[1].abs() < 0.0001)
        );
        let outer_notched = piece_outline(
            &GenerationSpec {
                outer_edge_interlocks: true,
                ..left_spec.clone()
            },
            0,
            0,
            true,
        )
        .unwrap();
        assert!(
            outer_notched[..edge_samples]
                .iter()
                .any(|point| point[1].abs() > left_spec.height_mm() * 0.01)
        );
        let plain = piece_outline(
            &GenerationSpec {
                adjacent_interlocks: false,
                ..left_spec
            },
            row,
            right_spec.columns - 1,
            true,
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
    fn every_wall_mount_style_cuts_a_watertight_piece_back() {
        for style in [
            WallMountStyle::StraightPin,
            WallMountStyle::AngledPin,
            WallMountStyle::FrenchCleat,
        ] {
            let spec = GenerationSpec {
                width_mm: 80.0,
                rows: 2,
                columns: 2,
                wall_mount: WallMountSpec {
                    style,
                    target: WallMountTarget::Terrain,
                    pin_diameter_mm: 4.0,
                    ..WallMountSpec::default()
                },
                ..GenerationSpec::default()
            };
            let mesh = build_piece(&spec, None, None, 0, 0).unwrap();
            assert_watertight(&mesh);
            assert!(mesh.vertices.iter().any(|vertex| {
                (vertex[2] - spec.wall_mount.pocket_depth_mm()).abs() < 0.000_01
            }));
            assert!(mesh.vertices.iter().any(|vertex| {
                (vertex[2] - spec.wall_mount.embedded_depth_mm()).abs() < 0.000_01
            }));
        }
    }

    #[test]
    fn jigsaw_wall_mount_uses_one_full_model_layout_across_piece_seams() {
        let spec = GenerationSpec {
            width_mm: 180.0,
            rows: 10,
            columns: 10,
            wall_mount: WallMountSpec {
                style: WallMountStyle::FrenchCleat,
                target: WallMountTarget::Terrain,
                ..WallMountSpec::default()
            },
            ..GenerationSpec::default()
        };
        spec.validate().unwrap();

        for (row, column) in [(0, 0), (4, 4), (4, 5), (5, 4), (5, 5)] {
            let mesh = build_piece(&spec, None, None, row, column).unwrap_or_else(|error| {
                panic!("piece {}, {} failed: {error:#}", row + 1, column + 1)
            });
            assert_watertight(&mesh);
        }
    }

    #[test]
    fn tray_retention_adds_a_watertight_piece_socket() {
        let spec = GenerationSpec {
            width_mm: 80.0,
            rows: 2,
            columns: 2,
            tray: TraySpec {
                enabled: true,
                ..TraySpec::default()
            },
            puzzle_retention: PuzzleRetentionSpec {
                enabled: true,
                ..PuzzleRetentionSpec::default()
            },
            ..GenerationSpec::default()
        };
        let mesh = build_piece(&spec, None, None, 0, 0).unwrap();
        assert_watertight(&mesh);
        assert!(mesh.vertices.iter().any(|vertex| {
            (vertex[2] - spec.puzzle_retention.socket_depth_mm()).abs() < 0.000_01
        }));

        let mut changed_wall_pocket = spec.clone();
        changed_wall_pocket.wall_mount.thickness_mm = 9.0;
        changed_wall_pocket.wall_mount.wall_offset_mm = 8.0;
        let unchanged_retention = build_piece(&changed_wall_pocket, None, None, 0, 0).unwrap();
        assert_eq!(mesh.vertices, unchanged_retention.vertices);
        assert_eq!(mesh.triangles, unchanged_retention.triangles);
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
