use std::collections::HashMap;

use anyhow::{Context, Result, bail};
use geo::{Area, BooleanOps, Buffer, Centroid, Contains, Coord, LineString, Point, Polygon};
use spade::{ConstrainedDelaunayTriangulation, Point2, Triangulation};

#[cfg(test)]
use crate::heightfield::height_range_for_spec;
use crate::heightfield::{HeightField, normalized_height};
use crate::jigsaw::{EdgePattern, edge_noise, edge_sign, puzzle_edge_point, shared_edge_pattern};
use crate::mesh::{
    Mesh, MeshBuilder, distance_squared, point_in_polygon, point_line_distance,
    triangulate_constraints, unit_vector,
};
use crate::spec::{BridgeStructure, GenerationSpec, SurfaceClass};
use crate::surface::{
    ROAD_VECTOR_STEP_MM, SurfaceField, VectorSurfaceArea, VectorSurfaceLine, surface_area_bounds,
    surface_line_progress,
};
use crate::tray::{add_triangle_contour_segment, smooth_contour_path, stitch_contour_segments};

const OVERLAY_TERRAIN_EMBED_MM: f32 = 0.02;
const BUILDING_GROUND_STEP_MM: f32 = 0.25;

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
    for grid_y in 0..grid_rows {
        let y = minimum_y + (grid_y as f32 + 0.5) * terrain_spacing;
        for grid_x in 0..grid_columns {
            let x = minimum_x + (grid_x as f32 + 0.5) * terrain_spacing;
            if point_in_polygon([x, y], &outline) {
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
        if !point_in_polygon(centroid, &outline) {
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
    };
    if spec.buildings.enabled
        && let Some(field) = surface_field
    {
        append_building_geometry(
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
        )?;
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
        )?;
    }
    Ok(mesh)
}

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
                bounds[0].min((point[0] + origin_x) / assembled_width),
                bounds[1].min((point[1] + origin_y) / assembled_height),
                bounds[2].max((point[0] + origin_x) / assembled_width),
                bounds[3].max((point[1] + origin_y) / assembled_height),
            ]
        },
    );
    for building in surface_field
        .vector_areas
        .iter()
        .filter(|area| area.building_height_m > 0.0 && area.points.len() >= 3)
        .filter(|area| bounds_overlap(surface_area_bounds(&area.points), piece_bounds))
    {
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
        let roof_z = building_roof_z(
            spec,
            building,
            height_field,
            height_range,
            assembled_width,
            assembled_height,
        );
        for polygon in clipped
            .0
            .iter()
            .filter(|polygon| polygon.unsigned_area() > 0.000_01)
        {
            let bottom = |point: [f32; 2]| {
                terrain_z_at(
                    spec,
                    height_field,
                    height_range,
                    (point[0] + origin_x) / assembled_width,
                    (point[1] + origin_y) / assembled_height,
                ) - OVERLAY_TERRAIN_EMBED_MM
            };
            let top = |_point: [f32; 2]| roof_z;
            mesh.append_isolated(build_polygon_shell(
                polygon,
                bottom,
                top,
                None,
                SurfaceClass::Building,
                "triangulate vector building footprint",
            )?);
        }
    }
    Ok(())
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
    ground_z + scaled_building_height_mm(spec, building.building_height_m)
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
    for line in surface_field
        .vector_lines
        .iter()
        .filter(|line| line.class == SurfaceClass::Road)
    {
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
        if !bounds_overlap(piece_bounds, line_bounds) {
            continue;
        }
        let local_points = line
            .points_mm
            .iter()
            .map(|point| Coord {
                x: (point[0] - origin_x) as f64,
                y: (point[1] - origin_y) as f64,
            })
            .collect::<Vec<_>>();
        if local_points.len() < 2 {
            continue;
        }
        let road_area = LineString::new(local_points).buffer(line.width_mm as f64 * 0.5);
        let mut clipped = road_area.intersection(&piece_polygon);
        if spec.buildings.enabled {
            for building in surface_field
                .vector_areas
                .iter()
                .filter(|area| area.building_height_m > 0.0 && area.points.len() >= 3)
                .filter(|area| {
                    let bounds = surface_area_bounds(&area.points);
                    let assembled_bounds = [
                        bounds[0] * assembled_width,
                        bounds[1] * assembled_height,
                        bounds[2] * assembled_width,
                        bounds[3] * assembled_height,
                    ];
                    bounds_overlap(piece_bounds, assembled_bounds)
                        && bounds_overlap(line_bounds, assembled_bounds)
                })
            {
                let local_building = building
                    .points
                    .iter()
                    .map(|point| {
                        [
                            point[0] * assembled_width - origin_x,
                            point[1] * assembled_height - origin_y,
                        ]
                    })
                    .collect::<Vec<_>>();
                clipped = clipped.difference(&geo_polygon(&local_building));
            }
        }
        for polygon in clipped
            .0
            .iter()
            .filter(|polygon| polygon.unsigned_area() > 0.000_01)
        {
            let road_mesh = build_road_polygon_shell(
                polygon,
                spec,
                line,
                height_field,
                height_range,
                origin_x,
                origin_y,
                assembled_width,
                assembled_height,
            )?;
            mesh.append_isolated(road_mesh);
        }
    }
    Ok(())
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
    line: &VectorSurfaceLine,
    height_field: Option<&HeightField>,
    height_range: Option<(f32, f32)>,
    origin_x: f32,
    origin_y: f32,
    assembled_width: f32,
    assembled_height: f32,
) -> Result<MeshBuilder> {
    let road_z = |point: [f32; 2]| {
        let u = ((point[0] + origin_x) / assembled_width).clamp(0.0, 1.0);
        let v = ((point[1] + origin_y) / assembled_height).clamp(0.0, 1.0);
        if let (Some([start, end]), Some((minimum, span))) =
            (line.bridge_elevations_m, height_range)
        {
            let progress = surface_line_progress(line, u, v);
            let elevation = start + (end - start) * progress;
            spec.base_mm + spec.relief_mm * ((elevation - minimum) / span).max(0.0)
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
    let is_bridge = line.bridge_elevations_m.is_some();
    let bottom = |point: [f32; 2]| {
        if !is_bridge {
            return road_z(point) - OVERLAY_TERRAIN_EMBED_MM;
        }
        match spec.color_output.bridge_structure {
            BridgeStructure::Floating => top(point) - spec.color_output.bridge_thickness_mm,
            BridgeStructure::Supported => {
                let u = ((point[0] + origin_x) / assembled_width).clamp(0.0, 1.0);
                let v = ((point[1] + origin_y) / assembled_height).clamp(0.0, 1.0);
                (terrain_z_at(spec, height_field, height_range, u, v) - OVERLAY_TERRAIN_EMBED_MM)
                    .min(top(point) - OVERLAY_TERRAIN_EMBED_MM)
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
                .map(|step| densify_closed_ring(&ring, step))
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
    let mut output = MeshBuilder::default();
    let mut edge_uses = HashMap::<(usize, usize), (u32, [usize; 2])>::new();
    let mut vertex_positions = HashMap::<usize, [f32; 2]>::new();
    for face in triangulation.inner_faces() {
        let face_vertices = face.vertices();
        let face_points = face_vertices.map(|vertex| {
            let point = vertex.position();
            [point.x as f32, point.y as f32]
        });
        let centroid = Point::new(
            face_points.iter().map(|point| point[0] as f64).sum::<f64>() / 3.0,
            face_points.iter().map(|point| point[1] as f64).sum::<f64>() / 3.0,
        );
        if !polygon.contains(&centroid) {
            continue;
        }
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
        .map(|point| [point.x as f32, point.y as f32])
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
