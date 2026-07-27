use std::collections::{HashMap, HashSet};

use anyhow::{Context, Result, bail};
use geo::{Area, Buffer, Coord, LineString, MultiPolygon, Polygon};
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
use crate::spec::{GenerationSpec, SurfaceClass};
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
/// Overlay footprint fragments below this area (mm^2) are unprintable dust
/// left over from boolean clipping and are dropped before shelling.
const MINIMUM_OVERLAY_AREA_MM2: f64 = 0.000_01;

mod buildings;
mod overlays;

use buildings::append_building_geometry;
use overlays::append_road_geometry;

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
    let mut flag_cavities = Vec::<(Vec<usize>, Vec<[f32; 2]>)>::new();
    for marker in spec
        .markers
        .iter()
        .filter(|marker| marker.kind == crate::spec::MarkerKind::FlagHole)
    {
        let uv = spec.normalized_map_point(marker.latitude, marker.longitude);
        let center = [
            uv[0] * assembled_width - origin_x,
            uv[1] * assembled_height - origin_y,
        ];
        if !point_in_polygon(center, &outline) {
            continue;
        }
        let radius = spec.marker_settings.hole_diameter_mm * 0.5;
        let ring = (0..32)
            .map(|index| {
                let angle = index as f32 / 32.0 * std::f32::consts::TAU;
                [
                    center[0] + radius * angle.cos(),
                    center[1] + radius * angle.sin(),
                ]
            })
            .collect::<Vec<_>>();
        if ring.iter().any(|point| !point_in_polygon(*point, &outline)) {
            bail!(
                "flag marker '{}' is too close to a terrain or puzzle-piece edge",
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
        flag_cavities.push((indices, ring));
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
    for (indices, ring) in &mut flag_cavities {
        *indices = ring
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
    for face in triangulation.inner_faces() {
        let face_vertices = face.vertices();
        let positions = face_vertices.map(|vertex| vertex.position());
        let centroid = [
            ((positions[0].x + positions[1].x + positions[2].x) / 3.0) as f32,
            ((positions[0].y + positions[1].y + positions[2].y) / 3.0) as f32,
        ];
        if !outline_index.contains(centroid)
            || flag_cavities
                .iter()
                .any(|(_, points)| point_in_polygon(centroid, points))
        {
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
            materials.push(SurfaceClass::Rock);
        }
    }
    // HashMap iteration order is randomized per process; sort the boundary
    // edges so the emitted mesh (and every artifact hashed from it) is
    // byte-for-byte reproducible across runs.
    let flag_edges = flag_cavities
        .iter()
        .flat_map(|(ring, _)| ring.iter().zip(ring.iter().cycle().skip(1)))
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
        .filter(|(uses, _)| *uses == 1)
        .map(|(_, edge)| edge)
        .filter(|edge| {
            let key = if edge[0] < edge[1] {
                (edge[0], edge[1])
            } else {
                (edge[1], edge[0])
            };
            !flag_edges.contains(&key)
        })
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
    append_flag_cavities(
        &mut mesh,
        &flag_cavities,
        top_count,
        spec.marker_settings.hole_depth_mm,
        !rebuilt_back,
    );
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
        || spec.uses_rail_or_aerial())
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

fn append_flag_cavities(
    mesh: &mut Mesh,
    cavities: &[(Vec<usize>, Vec<[f32; 2]>)],
    top_count: usize,
    depth_mm: f32,
    close_bottom: bool,
) {
    for (ring, _) in cavities {
        if ring.len() < 3 {
            continue;
        }
        let floor_z = ring
            .iter()
            .map(|index| mesh.vertices[*index][2])
            .fold(f32::INFINITY, f32::min)
            - depth_mm;
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
            mesh.materials.push(SurfaceClass::Rock);
            mesh.triangles.push([top_a, floor_b, floor_a]);
            mesh.materials.push(SurfaceClass::Rock);
            mesh.triangles.push([floor_a, floor_b, center_index]);
            mesh.materials.push(SurfaceClass::Rock);
            if let Some(bottom_center_index) = bottom_center_index {
                let bottom_a = top_a + top_count as u32;
                let bottom_b = top_b + top_count as u32;
                mesh.triangles
                    .push([bottom_a, bottom_center_index, bottom_b]);
                mesh.materials.push(SurfaceClass::Rock);
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

pub(crate) fn local_piece_outline(
    spec: &GenerationSpec,
    row: u32,
    column: u32,
) -> Result<Vec<[f32; 2]>> {
    let piece_width = spec.width_mm / spec.columns as f32;
    let piece_height = spec.height_mm() / spec.rows as f32;
    let origin_x = column as f32 * piece_width;
    let origin_y = row as f32 * piece_height;
    Ok(piece_outline(spec, row, column, false)?
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

pub(crate) fn scaled_building_height_mm(spec: &GenerationSpec, height_m: f32) -> f32 {
    if !spec.buildings.enabled && !spec.uses_building_markers() {
        return 0.0;
    }
    height_m * spec.width_mm / (spec.ground_span_km as f32 * 1_000.0) * spec.buildings.z_scale
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    use crate::mesh::assert_watertight;
    use crate::spec::{
        MapMarker, MarkerKind, PuzzleRetentionSpec, TraySpec, WallMountSpec, WallMountStyle,
        WallMountTarget,
    };

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
