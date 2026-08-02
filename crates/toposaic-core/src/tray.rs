use std::collections::HashMap;

use anyhow::{Context, Result, bail};
use geo::{
    Area, BooleanOps, BoundingRect, Buffer, Contains, LineString, Point, Polygon, Translate,
};

use crate::heightfield::{HeightField, height_range_for_spec, normalized_height};
use crate::jigsaw::{edge_sign, puzzle_edge_point, shared_edge_pattern};
use crate::mesh::{Mesh, MeshBuilder, distance_squared, unit_vector, weld_export_mesh};
use crate::mount::{circle_points, mount_bottom, mount_bottom_polygons};
use crate::mount_layout::retention_centers_local;
use crate::outline::model_outline_mm;
use crate::piece::{local_piece_outline, printable_piece_positions, solid_outline};
use crate::planar_mesh::{
    add_horizontal_polygons, closed_ring, polygon_from_outline as geo_polygon,
};
use crate::spec::{GenerationSpec, SurfaceClass, TrayLabelPosition};
use crate::text::{EmbossedLabel, embossing_fonts, text_metrics};

const TRAY_CONTOUR_WIDTH_MM: f32 = 0.45;
const TRAY_CONTOUR_INLAY_MM: f32 = 0.2;
const TRAY_CONTOUR_SURFACE_OFFSET_MM: f32 = 0.01;
const TRAY_SURFACE_SPACING_MM: f32 = 0.35;
const PREVIEW_TRAY_SURFACE_SPACING_MM: f32 = 1.5;
/// How far inside a segment outline a contour centreline point must sit to
/// be kept: the ribbon half-width plus the miter allowance
/// (`add_contour_ribbon` caps a miter at twice the half-width), so no
/// ribbon vertex can protrude past the segment's cut walls. Fit wins over
/// contour reach: a contour hugging a cut therefore ends up to this far
/// from the wall instead of poking through it.
const TRAY_CONTOUR_CLIP_INSET_MM: f32 = TRAY_CONTOUR_WIDTH_MM;

/// The tray's inner-frame geometry. The one-piece tray, tray segments, and
/// contour tracing all derive from this one place so segments always match
/// the one-piece tray.
#[derive(Clone, Copy)]
struct TrayFrame {
    inner_width: f32,
    inner_height: f32,
    outer_width: f32,
    outer_height: f32,
    inner_x0: f32,
    inner_y0: f32,
    inner_x1: f32,
    inner_y1: f32,
    floor_z: f32,
    rim_z: f32,
}

impl TrayFrame {
    fn from_spec(spec: &GenerationSpec) -> Self {
        let tray = &spec.tray;
        let inner_width = spec.width_mm + tray.clearance_mm * 2.0;
        let inner_height = spec.height_mm() + tray.clearance_mm * 2.0;
        let inner_x0 = tray.rim_width_mm;
        let inner_y0 = tray.rim_width_mm;
        Self {
            inner_width,
            inner_height,
            outer_width: inner_width + tray.rim_width_mm * 2.0,
            outer_height: inner_height + tray.rim_width_mm * 2.0,
            inner_x0,
            inner_y0,
            inner_x1: inner_x0 + inner_width,
            inner_y1: inner_y0 + inner_height,
            floor_z: tray.floor_mm,
            rim_z: tray.floor_mm + tray.rim_height_mm,
        }
    }
}

#[cfg(test)]
fn build_tray(spec: &GenerationSpec, height_field: Option<&HeightField>) -> Result<Mesh> {
    build_tray_with_spacing(spec, height_field, TRAY_SURFACE_SPACING_MM)
}

fn build_tray_with_spacing(
    spec: &GenerationSpec,
    height_field: Option<&HeightField>,
    surface_spacing_mm: f32,
) -> Result<Mesh> {
    if spec.model_outline.shape != crate::spec::OutlineShape::Rectangle {
        return build_shaped_tray(spec, height_field, surface_spacing_mm);
    }
    let tray = &spec.tray;
    let TrayFrame {
        inner_width,
        inner_height,
        outer_width,
        outer_height,
        inner_x0,
        inner_y0,
        inner_x1,
        inner_y1,
        floor_z,
        rim_z,
    } = TrayFrame::from_spec(spec);
    let mut x_coordinates = regular_coordinates(0.0, outer_width, surface_spacing_mm);
    let mut y_coordinates = regular_coordinates(0.0, outer_height, surface_spacing_mm);
    insert_coordinate(&mut x_coordinates, inner_x0);
    insert_coordinate(&mut x_coordinates, inner_x1);
    insert_coordinate(&mut y_coordinates, inner_y0);
    insert_coordinate(&mut y_coordinates, inner_y1);
    let inner_x = x_coordinates
        .iter()
        .copied()
        .filter(|x| *x >= inner_x0 && *x <= inner_x1)
        .collect::<Vec<_>>();
    let inner_y = y_coordinates
        .iter()
        .copied()
        .filter(|y| *y >= inner_y0 && *y <= inner_y1)
        .collect::<Vec<_>>();
    let left_rim_x = x_coordinates
        .iter()
        .copied()
        .filter(|x| *x <= inner_x0)
        .collect::<Vec<_>>();
    let right_rim_x = x_coordinates
        .iter()
        .copied()
        .filter(|x| *x >= inner_x1)
        .collect::<Vec<_>>();
    let front_rim_y = y_coordinates
        .iter()
        .copied()
        .filter(|y| *y <= inner_y0)
        .collect::<Vec<_>>();
    let back_rim_y = y_coordinates
        .iter()
        .copied()
        .filter(|y| *y >= inner_y1)
        .collect::<Vec<_>>();
    let label = if tray.label_enabled {
        Some(tray_label(spec, outer_width, tray.rim_width_mm)?)
    } else {
        None
    };
    let z_coordinates = [0.0, rim_z];

    let height_range = height_range_for_spec(spec, height_field);
    let contour_paths = if tray.contours_enabled {
        trace_tray_contours(
            spec,
            height_field,
            height_range,
            &inner_x,
            &inner_y,
            inner_x0,
            inner_y0,
            inner_width,
            inner_height,
        )
    } else {
        Vec::new()
    };
    let retention_centers = if spec.puzzle_retention.active(spec.tray.enabled) {
        tray_retention_centers(spec)?
    } else {
        Vec::new()
    };
    let mut mesh = MeshBuilder::default();

    if spec.puzzle_retention.active(spec.tray.enabled) {
        add_floor_with_retention_pins(
            &mut mesh,
            &[rectangle_polygon(inner_x0, inner_y0, inner_x1, inner_y1)],
            floor_z,
            &retention_centers,
            spec,
        )?;
    } else {
        for y in inner_y.windows(2) {
            for x in inner_x.windows(2) {
                mesh.quad(
                    [x[0], y[0], floor_z],
                    [x[1], y[0], floor_z],
                    [x[1], y[1], floor_z],
                    [x[0], y[1], floor_z],
                    SurfaceClass::Rock,
                );
            }
        }
    }

    for x in x_coordinates.windows(2) {
        for y in front_rim_y.windows(2) {
            mesh.quad(
                [x[0], y[0], rim_z],
                [x[1], y[0], rim_z],
                [x[1], y[1], rim_z],
                [x[0], y[1], rim_z],
                SurfaceClass::Rock,
            );
        }
        for y in back_rim_y.windows(2) {
            mesh.quad(
                [x[0], y[0], rim_z],
                [x[1], y[0], rim_z],
                [x[1], y[1], rim_z],
                [x[0], y[1], rim_z],
                SurfaceClass::Rock,
            );
        }
    }
    for y in inner_y.windows(2) {
        for x in left_rim_x.windows(2) {
            mesh.quad(
                [x[0], y[0], rim_z],
                [x[1], y[0], rim_z],
                [x[1], y[1], rim_z],
                [x[0], y[1], rim_z],
                SurfaceClass::Rock,
            );
        }
        for x in right_rim_x.windows(2) {
            mesh.quad(
                [x[0], y[0], rim_z],
                [x[1], y[0], rim_z],
                [x[1], y[1], rim_z],
                [x[0], y[1], rim_z],
                SurfaceClass::Rock,
            );
        }
    }

    // The cavity walls wind with their normals facing INTO the cavity (away
    // from the rim solid), consistent with the up-facing floor and rim tops:
    // shared edges are then traversed once per direction, which the manifold
    // analyzer's misoriented-edge counter verifies.
    for x in inner_x.windows(2) {
        mesh.quad(
            [x[1], inner_y0, floor_z],
            [x[0], inner_y0, floor_z],
            [x[0], inner_y0, rim_z],
            [x[1], inner_y0, rim_z],
            SurfaceClass::Rock,
        );
        mesh.quad(
            [x[0], inner_y1, floor_z],
            [x[1], inner_y1, floor_z],
            [x[1], inner_y1, rim_z],
            [x[0], inner_y1, rim_z],
            SurfaceClass::Rock,
        );
    }
    for y in inner_y.windows(2) {
        mesh.quad(
            [inner_x0, y[0], floor_z],
            [inner_x0, y[1], floor_z],
            [inner_x0, y[1], rim_z],
            [inner_x0, y[0], rim_z],
            SurfaceClass::Rock,
        );
        mesh.quad(
            [inner_x1, y[1], floor_z],
            [inner_x1, y[0], floor_z],
            [inner_x1, y[0], rim_z],
            [inner_x1, y[1], rim_z],
            SurfaceClass::Rock,
        );
    }

    for z in z_coordinates.windows(2) {
        for x in x_coordinates.windows(2) {
            mesh.quad(
                [x[0], 0.0, z[0]],
                [x[1], 0.0, z[0]],
                [x[1], 0.0, z[1]],
                [x[0], 0.0, z[1]],
                SurfaceClass::Rock,
            );
            mesh.quad(
                [x[1], outer_height, z[0]],
                [x[0], outer_height, z[0]],
                [x[0], outer_height, z[1]],
                [x[1], outer_height, z[1]],
                SurfaceClass::Rock,
            );
        }
        for y in y_coordinates.windows(2) {
            mesh.quad(
                [0.0, y[1], z[0]],
                [0.0, y[0], z[0]],
                [0.0, y[0], z[1]],
                [0.0, y[1], z[1]],
                SurfaceClass::Rock,
            );
            mesh.quad(
                [outer_width, y[0], z[0]],
                [outer_width, y[1], z[0]],
                [outer_width, y[1], z[1]],
                [outer_width, y[0], z[1]],
                SurfaceClass::Rock,
            );
        }
    }

    if spec.wall_mount.cuts_tray() {
        let mut bottom_outline = Vec::new();
        bottom_outline.extend(x_coordinates.iter().map(|x| [*x, 0.0]));
        bottom_outline.extend(y_coordinates.iter().skip(1).map(|y| [outer_width, *y]));
        bottom_outline.extend(
            x_coordinates
                .iter()
                .rev()
                .skip(1)
                .map(|x| [*x, outer_height]),
        );
        bottom_outline.extend(
            y_coordinates
                .iter()
                .rev()
                .skip(1)
                .take(y_coordinates.len().saturating_sub(2))
                .map(|y| [0.0, *y]),
        );
        mesh.append_isolated(mount_bottom(
            &bottom_outline,
            &spec.wall_mount,
            [0.0, 0.0, outer_width, outer_height],
        )?);
    } else {
        let center = [outer_width * 0.5, outer_height * 0.5, 0.0];
        let mut boundary = Vec::new();
        boundary.extend(x_coordinates.iter().map(|x| [*x, 0.0, 0.0]));
        boundary.extend(y_coordinates.iter().skip(1).map(|y| [outer_width, *y, 0.0]));
        boundary.extend(
            x_coordinates
                .iter()
                .rev()
                .skip(1)
                .map(|x| [*x, outer_height, 0.0]),
        );
        boundary.extend(
            y_coordinates
                .iter()
                .rev()
                .skip(1)
                .take(y_coordinates.len().saturating_sub(2))
                .map(|y| [0.0, *y, 0.0]),
        );
        for index in 0..boundary.len() {
            let current = boundary[index];
            let next = boundary[(index + 1) % boundary.len()];
            mesh.triangle(center, next, current, SurfaceClass::Rock);
        }
    }
    for path in &contour_paths {
        for printable_path in contour_paths_around_retention_pins(path, &retention_centers, spec) {
            add_contour_ribbon(
                &mut mesh,
                &printable_path,
                floor_z - TRAY_CONTOUR_INLAY_MM,
                floor_z + TRAY_CONTOUR_SURFACE_OFFSET_MM,
            );
        }
    }
    if let Some(label) = label {
        label.add_embossed_shapes(&mut mesh, rim_z)?;
    }

    Ok(mesh.finish("terrain-tray"))
}

fn build_shaped_tray(
    spec: &GenerationSpec,
    height_field: Option<&HeightField>,
    surface_spacing_mm: f32,
) -> Result<Mesh> {
    let tray = &spec.tray;
    let terrain = geo_polygon(&model_outline_mm(spec, 96));
    let cavity_unshifted = largest_polygon(terrain.buffer(f64::from(tray.clearance_mm)))?;
    let outer_unshifted = largest_polygon(cavity_unshifted.buffer(f64::from(tray.rim_width_mm)))?;
    let bounds = outer_unshifted
        .bounding_rect()
        .context("shaped tray has no printable bounds")?;
    let shift_x = -bounds.min().x;
    let shift_y = -bounds.min().y;
    let cavity = cavity_unshifted.translate(shift_x, shift_y);
    let outer = outer_unshifted.translate(shift_x, shift_y);
    let rim = vec![Polygon::new(
        outer.exterior().clone(),
        vec![cavity.exterior().clone()],
    )];
    let floor_z = tray.floor_mm;
    let rim_z = tray.floor_mm + tray.rim_height_mm;
    let retention_centers = if spec.puzzle_retention.active(spec.tray.enabled) {
        tray_retention_centers_at(spec, [shift_x as f32, shift_y as f32])?
    } else {
        Vec::new()
    };
    let mut mesh = MeshBuilder::default();

    if spec.puzzle_retention.active(spec.tray.enabled) {
        add_floor_with_retention_pins(
            &mut mesh,
            std::slice::from_ref(&cavity),
            floor_z,
            &retention_centers,
            spec,
        )?;
    } else {
        add_horizontal_polygons(
            &mut mesh,
            std::slice::from_ref(&cavity),
            floor_z,
            SurfaceClass::Rock,
            false,
        )?;
    }
    add_horizontal_polygons(&mut mesh, &rim, rim_z, SurfaceClass::Rock, false)?;
    if spec.wall_mount.cuts_tray() {
        mesh.append_isolated(mount_bottom_polygons(
            std::slice::from_ref(&outer),
            &spec.wall_mount,
            [
                0.0,
                0.0,
                (bounds.max().x - bounds.min().x) as f32,
                (bounds.max().y - bounds.min().y) as f32,
            ],
        )?);
    } else {
        add_horizontal_polygons(
            &mut mesh,
            std::slice::from_ref(&outer),
            0.0,
            SurfaceClass::Rock,
            true,
        )?;
    }
    add_ring_walls(&mut mesh, outer.exterior(), 0.0, rim_z, false);
    add_ring_walls(&mut mesh, cavity.exterior(), floor_z, rim_z, true);

    if tray.contours_enabled {
        let rectangular_origin = tray.rim_width_mm + tray.clearance_mm;
        let contour_shift = [
            shift_x as f32 - rectangular_origin,
            shift_y as f32 - rectangular_origin,
        ];
        for mut path in tray_contour_paths_with_spacing(spec, height_field, surface_spacing_mm) {
            for point in &mut path.points {
                point[0] += contour_shift[0];
                point[1] += contour_shift[1];
            }
            for clipped in clip_contour_path(&path, &cavity) {
                for printable in
                    contour_paths_around_retention_pins(&clipped, &retention_centers, spec)
                {
                    add_contour_ribbon(
                        &mut mesh,
                        &printable,
                        floor_z - TRAY_CONTOUR_INLAY_MM,
                        floor_z + TRAY_CONTOUR_SURFACE_OFFSET_MM,
                    );
                }
            }
        }
    }
    Ok(mesh.finish("terrain-tray"))
}

fn largest_polygon(polygons: geo::MultiPolygon<f64>) -> Result<Polygon<f64>> {
    polygons
        .0
        .into_iter()
        .max_by(|first, second| first.unsigned_area().total_cmp(&second.unsigned_area()))
        .context("tray clearance removed the model outline")
}

fn add_ring_walls(
    mesh: &mut MeshBuilder,
    ring: &LineString<f64>,
    lower_z: f32,
    upper_z: f32,
    reverse: bool,
) {
    for edge in ring.0.windows(2) {
        let a = [edge[0].x as f32, edge[0].y as f32];
        let b = [edge[1].x as f32, edge[1].y as f32];
        if reverse {
            mesh.quad(
                [b[0], b[1], lower_z],
                [a[0], a[1], lower_z],
                [a[0], a[1], upper_z],
                [b[0], b[1], upper_z],
                SurfaceClass::Rock,
            );
        } else {
            mesh.quad(
                [a[0], a[1], lower_z],
                [b[0], b[1], lower_z],
                [b[0], b[1], upper_z],
                [a[0], a[1], upper_z],
                SurfaceClass::Rock,
            );
        }
    }
}

pub(crate) fn build_tray_segments(
    spec: &GenerationSpec,
    height_field: Option<&HeightField>,
) -> Result<Vec<Mesh>> {
    build_tray_segments_with_spacing(spec, height_field, TRAY_SURFACE_SPACING_MM)
}

pub(crate) fn build_preview_tray_segments(
    spec: &GenerationSpec,
    height_field: Option<&HeightField>,
) -> Result<Vec<Mesh>> {
    build_tray_segments_with_spacing(spec, height_field, PREVIEW_TRAY_SURFACE_SPACING_MM)
}

fn build_tray_segments_with_spacing(
    spec: &GenerationSpec,
    height_field: Option<&HeightField>,
    surface_spacing_mm: f32,
) -> Result<Vec<Mesh>> {
    let mut segments = if spec.tray.segment_columns == 1 && spec.tray.segment_rows == 1 {
        vec![build_tray_with_spacing(
            spec,
            height_field,
            surface_spacing_mm,
        )?]
    } else {
        let contour_paths = if spec.tray.contours_enabled {
            tray_contour_paths_with_spacing(spec, height_field, surface_spacing_mm)
        } else {
            Vec::new()
        };
        let mut segments =
            Vec::with_capacity((spec.tray.segment_columns * spec.tray.segment_rows) as usize);
        for row in 0..spec.tray.segment_rows {
            for column in 0..spec.tray.segment_columns {
                segments.push(
                    build_tray_segment(spec, &contour_paths, row, column).with_context(|| {
                        format!(
                            "build display-base segment row {} column {}",
                            row + 1,
                            column + 1
                        )
                    })?,
                );
            }
        }
        segments
    };
    for segment in &mut segments {
        weld_export_mesh(segment);
    }
    Ok(segments)
}

/// Where each exported tray mesh sits in the assembled display base.
///
/// Segmented tray meshes are shifted to their own local origin for printing.
/// The live preview puts them back so their seams, walls, and interlocks match
/// the completed base. A one-piece tray already uses the assembled frame.
pub(crate) fn tray_segment_origins(spec: &GenerationSpec) -> Vec<[f32; 2]> {
    if spec.tray.segment_columns == 1 && spec.tray.segment_rows == 1 {
        return vec![[0.0, 0.0]];
    }
    let grid = tray_segment_grid(spec);
    let mut origins =
        Vec::with_capacity((spec.tray.segment_columns * spec.tray.segment_rows) as usize);
    for row in 0..spec.tray.segment_rows {
        for column in 0..spec.tray.segment_columns {
            let outline = tray_segment_outline(grid, row, column);
            origins.push([
                outline
                    .iter()
                    .map(|point| point[0])
                    .fold(f32::INFINITY, f32::min),
                outline
                    .iter()
                    .map(|point| point[1])
                    .fold(f32::INFINITY, f32::min),
            ]);
        }
    }
    origins
}

/// The assembled terrain's lower-left corner inside its fitted tray.
pub(crate) fn terrain_origin_in_tray(spec: &GenerationSpec) -> Result<[f32; 2]> {
    if spec.model_outline.shape == crate::spec::OutlineShape::Rectangle {
        let inset = spec.tray.rim_width_mm + spec.tray.clearance_mm;
        return Ok([inset, inset]);
    }
    let terrain = geo_polygon(&model_outline_mm(spec, 96));
    let cavity = largest_polygon(terrain.buffer(f64::from(spec.tray.clearance_mm)))?;
    let outer = largest_polygon(cavity.buffer(f64::from(spec.tray.rim_width_mm)))?;
    let bounds = outer
        .bounding_rect()
        .context("shaped tray has no printable bounds")?;
    Ok([-bounds.min().x as f32, -bounds.min().y as f32])
}

fn build_tray_segment(
    spec: &GenerationSpec,
    contour_paths: &[ContourPath],
    row: u32,
    column: u32,
) -> Result<Mesh> {
    let tray = &spec.tray;
    let TrayFrame {
        inner_width: _,
        inner_height: _,
        outer_width: _,
        outer_height: _,
        inner_x0,
        inner_y0,
        inner_x1,
        inner_y1,
        floor_z,
        rim_z,
    } = TrayFrame::from_spec(spec);
    let segment_grid = tray_segment_grid(spec);
    let outline = tray_segment_outline(segment_grid, row, column);
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
    let segment_polygon = geo_polygon(&outline);
    let inner_polygon = rectangle_polygon(inner_x0, inner_y0, inner_x1, inner_y1);
    let floor_polygons = segment_polygon.intersection(&inner_polygon).0;
    let rim_polygons = segment_polygon.difference(&inner_polygon).0;
    let retention_centers = if spec.puzzle_retention.active(spec.tray.enabled) {
        tray_retention_centers(spec)?
    } else {
        Vec::new()
    };
    let mut mesh = MeshBuilder::default();

    if spec.puzzle_retention.active(spec.tray.enabled) {
        add_floor_with_retention_pins(
            &mut mesh,
            &floor_polygons,
            floor_z,
            &retention_centers,
            spec,
        )?;
    } else {
        add_horizontal_polygons(
            &mut mesh,
            &floor_polygons,
            floor_z,
            SurfaceClass::Rock,
            false,
        )?;
    }
    add_horizontal_polygons(&mut mesh, &rim_polygons, rim_z, SurfaceClass::Rock, false)?;
    if spec.wall_mount.cuts_tray() {
        let bottom_polygons = floor_polygons
            .iter()
            .chain(&rim_polygons)
            .cloned()
            .collect::<Vec<_>>();
        let [terrain_x0, terrain_y0, terrain_x1, terrain_y1] = segment_grid.terrain_bounds;
        let mount_frame = [
            terrain_x0 + (terrain_x1 - terrain_x0) * column as f32 / tray.segment_columns as f32,
            terrain_y0 + (terrain_y1 - terrain_y0) * row as f32 / tray.segment_rows as f32,
            terrain_x0
                + (terrain_x1 - terrain_x0) * (column + 1) as f32 / tray.segment_columns as f32,
            terrain_y0 + (terrain_y1 - terrain_y0) * (row + 1) as f32 / tray.segment_rows as f32,
        ];
        mesh.append_isolated(mount_bottom_polygons(
            &bottom_polygons,
            &spec.wall_mount,
            mount_frame,
        )?);
    } else {
        add_horizontal_polygons(&mut mesh, &floor_polygons, 0.0, SurfaceClass::Rock, true)?;
        add_horizontal_polygons(&mut mesh, &rim_polygons, 0.0, SurfaceClass::Rock, true)?;
    }

    let inner_frame = [inner_x0, inner_y0, inner_x1, inner_y1];
    add_segment_walls(
        &mut mesh,
        &floor_polygons,
        inner_frame,
        SegmentWallSide::Outer,
        0.0,
        floor_z,
    );
    add_segment_walls(
        &mut mesh,
        &rim_polygons,
        inner_frame,
        SegmentWallSide::Outer,
        0.0,
        floor_z,
    );
    add_segment_walls(
        &mut mesh,
        &rim_polygons,
        inner_frame,
        SegmentWallSide::Outer,
        floor_z,
        rim_z,
    );
    add_segment_walls(
        &mut mesh,
        &floor_polygons,
        inner_frame,
        SegmentWallSide::Inner,
        floor_z,
        rim_z,
    );

    for path in contour_paths {
        for clipped in clip_contour_path(path, &segment_polygon) {
            for printable_path in
                contour_paths_around_retention_pins(&clipped, &retention_centers, spec)
            {
                add_contour_ribbon(
                    &mut mesh,
                    &printable_path,
                    floor_z - TRAY_CONTOUR_INLAY_MM,
                    floor_z + TRAY_CONTOUR_SURFACE_OFFSET_MM,
                );
            }
        }
    }

    if row == 0 && tray.label_enabled {
        let segment_width = maximum_x - minimum_x;
        let label_margin = 8.0_f32.min(segment_width * 0.2);
        let mut label = tray_label(
            spec,
            (segment_width - label_margin * 2.0).max(12.0),
            tray.rim_width_mm,
        )?;
        label.origin_x += minimum_x + label_margin;
        label.add_embossed_shapes(&mut mesh, rim_z)?;
    }

    let mut result = mesh.finish(format!("terrain-tray-r{}-c{}", row + 1, column + 1));
    for vertex in &mut result.vertices {
        vertex[0] -= minimum_x;
        vertex[1] -= minimum_y;
    }
    Ok(result)
}

fn tray_contour_paths_with_spacing(
    spec: &GenerationSpec,
    height_field: Option<&HeightField>,
    surface_spacing_mm: f32,
) -> Vec<ContourPath> {
    let TrayFrame {
        inner_width,
        inner_height,
        inner_x0,
        inner_y0,
        inner_x1,
        inner_y1,
        ..
    } = TrayFrame::from_spec(spec);
    let x_coordinates = regular_coordinates(inner_x0, inner_x1, surface_spacing_mm);
    let y_coordinates = regular_coordinates(inner_y0, inner_y1, surface_spacing_mm);
    trace_tray_contours(
        spec,
        height_field,
        height_range_for_spec(spec, height_field),
        &x_coordinates,
        &y_coordinates,
        inner_x0,
        inner_y0,
        inner_width,
        inner_height,
    )
}

#[derive(Clone, Copy)]
struct TraySegmentGrid {
    size: [f32; 2],
    terrain_bounds: [f32; 4],
    rows: u32,
    columns: u32,
    puzzle_seed: u32,
    interlocks: bool,
    clearance_mm: f32,
}

fn tray_segment_grid(spec: &GenerationSpec) -> TraySegmentGrid {
    let tray = &spec.tray;
    let frame = TrayFrame::from_spec(spec);
    TraySegmentGrid {
        size: [frame.outer_width, frame.outer_height],
        terrain_bounds: [
            frame.inner_x0 + tray.clearance_mm,
            frame.inner_y0 + tray.clearance_mm,
            frame.inner_x1 - tray.clearance_mm,
            frame.inner_y1 - tray.clearance_mm,
        ],
        rows: tray.segment_rows,
        columns: tray.segment_columns,
        puzzle_seed: spec.puzzle_seed,
        interlocks: spec.adjacent_interlocks,
        clearance_mm: if spec.adjacent_interlocks {
            spec.clearance_mm
        } else {
            0.0
        },
    }
}

fn tray_segment_outline(grid: TraySegmentGrid, row: u32, column: u32) -> Vec<[f32; 2]> {
    let [width, height] = grid.size;
    let [terrain_x0, terrain_y0, terrain_x1, terrain_y1] = grid.terrain_bounds;
    let rows = grid.rows;
    let columns = grid.columns;
    let x0 = if column == 0 {
        0.0
    } else {
        terrain_x0 + (terrain_x1 - terrain_x0) * column as f32 / columns as f32
    };
    let x1 = if column + 1 == columns {
        width
    } else {
        terrain_x0 + (terrain_x1 - terrain_x0) * (column + 1) as f32 / columns as f32
    };
    let y0 = if row == 0 {
        0.0
    } else {
        terrain_y0 + (terrain_y1 - terrain_y0) * row as f32 / rows as f32
    };
    let y1 = if row + 1 == rows {
        height
    } else {
        terrain_y0 + (terrain_y1 - terrain_y0) * (row + 1) as f32 / rows as f32
    };
    let corners = [[x0, y0], [x1, y0], [x1, y1], [x0, y1]];
    let nominal_size = ((x1 - x0).min(y1 - y0)).max(1.0);
    let base_depth = nominal_size * 0.12;
    let samples = 96;
    let edges = [
        (
            corners[0],
            corners[1],
            shared_edge_pattern(grid.puzzle_seed, 0, i64::from(row), i64::from(column)),
            if grid.interlocks && row > 0 {
                edge_sign(grid.puzzle_seed, 0, i64::from(column), i64::from(row))
            } else {
                0.0
            },
            false,
            if row > 0 {
                [0.0, grid.clearance_mm * 0.5]
            } else {
                [0.0, 0.0]
            },
        ),
        (
            corners[1],
            corners[2],
            shared_edge_pattern(grid.puzzle_seed, 1, i64::from(column + 1), i64::from(row)),
            if grid.interlocks && column + 1 < columns {
                edge_sign(grid.puzzle_seed, 1, i64::from(row), i64::from(column + 1))
            } else {
                0.0
            },
            false,
            if column + 1 < columns {
                [-grid.clearance_mm * 0.5, 0.0]
            } else {
                [0.0, 0.0]
            },
        ),
        (
            corners[3],
            corners[2],
            shared_edge_pattern(grid.puzzle_seed, 0, i64::from(row + 1), i64::from(column)),
            if grid.interlocks && row + 1 < rows {
                edge_sign(grid.puzzle_seed, 0, i64::from(column), i64::from(row + 1))
            } else {
                0.0
            },
            true,
            if row + 1 < rows {
                [0.0, -grid.clearance_mm * 0.5]
            } else {
                [0.0, 0.0]
            },
        ),
        (
            corners[0],
            corners[3],
            shared_edge_pattern(grid.puzzle_seed, 1, i64::from(column), i64::from(row)),
            if grid.interlocks && column > 0 {
                edge_sign(grid.puzzle_seed, 1, i64::from(row), i64::from(column))
            } else {
                0.0
            },
            true,
            if column > 0 {
                [grid.clearance_mm * 0.5, 0.0]
            } else {
                [0.0, 0.0]
            },
        ),
    ];
    let mut outline = Vec::with_capacity(samples * 4);
    for (start, end, pattern, sign, reverse, clearance_shift) in edges {
        for index in 0..samples {
            let t = index as f32 / samples as f32;
            let mut point = puzzle_edge_point(
                start,
                end,
                pattern,
                sign,
                if reverse { 1.0 - t } else { t },
                base_depth,
            );
            point[0] += clearance_shift[0];
            point[1] += clearance_shift[1];
            outline.push(point);
        }
    }
    outline
}

fn tray_retention_centers(spec: &GenerationSpec) -> Result<Vec<[f32; 2]>> {
    let frame = TrayFrame::from_spec(spec);
    tray_retention_centers_at(
        spec,
        [
            frame.inner_x0 + spec.tray.clearance_mm,
            frame.inner_y0 + spec.tray.clearance_mm,
        ],
    )
}

fn tray_retention_centers_at(
    spec: &GenerationSpec,
    terrain_origin: [f32; 2],
) -> Result<Vec<[f32; 2]>> {
    if spec.solid_model {
        let outline = solid_outline(spec, 64)?;
        return Ok(retention_centers_local(spec, 0, 0, &outline)
            .into_iter()
            .map(|center| [terrain_origin[0] + center[0], terrain_origin[1] + center[1]])
            .collect());
    }

    let piece_width = spec.width_mm / spec.columns as f32;
    let piece_height = spec.height_mm() / spec.rows as f32;
    let mut centers = Vec::with_capacity((spec.rows * spec.columns) as usize);
    for (row, column) in printable_piece_positions(spec)? {
        let outline = local_piece_outline(spec, row, column)?;
        centers.extend(
            retention_centers_local(spec, row, column, &outline)
                .into_iter()
                .map(|center| {
                    [
                        terrain_origin[0] + column as f32 * piece_width + center[0],
                        terrain_origin[1] + row as f32 * piece_height + center[1],
                    ]
                }),
        );
    }
    Ok(centers)
}

fn add_floor_with_retention_pins(
    mesh: &mut MeshBuilder,
    floor_polygons: &[Polygon<f64>],
    floor_z: f32,
    centers: &[[f32; 2]],
    spec: &GenerationSpec,
) -> Result<()> {
    let radius = spec.puzzle_retention.pin_diameter_mm * 0.5;
    let pin_rings = centers
        .iter()
        .map(|center| circle_points(*center, radius))
        .collect::<Vec<_>>();
    let mut holes = vec![Vec::<LineString<f64>>::new(); floor_polygons.len()];
    let mut retained_pins = Vec::new();
    for (center, pin_ring) in centers.iter().zip(&pin_rings) {
        let center_point = Point::new(f64::from(center[0]), f64::from(center[1]));
        if let Some(index) = floor_polygons
            .iter()
            .position(|polygon| polygon.contains(&center_point))
        {
            if !pin_ring.iter().all(|point| {
                floor_polygons[index]
                    .contains(&Point::new(f64::from(point[0]), f64::from(point[1])))
            }) {
                bail!(
                    "a tray-retention pin crosses a tray-section join; reduce the pin size or change the tray split"
                );
            }
            holes[index].push(closed_ring(pin_ring));
            retained_pins.push((*center, pin_ring));
        }
    }
    let surfaces = floor_polygons
        .iter()
        .enumerate()
        .map(|(index, polygon)| {
            let mut interiors = polygon.interiors().to_vec();
            interiors.append(&mut holes[index]);
            Polygon::new(polygon.exterior().clone(), interiors)
        })
        .collect::<Vec<_>>();
    add_horizontal_polygons(mesh, &surfaces, floor_z, SurfaceClass::Rock, false)?;

    let top_z = floor_z + spec.puzzle_retention.pin_height_mm;
    for (center, pin_ring) in retained_pins {
        for index in 0..pin_ring.len() {
            let next = (index + 1) % pin_ring.len();
            let a = pin_ring[index];
            let b = pin_ring[next];
            mesh.quad(
                [a[0], a[1], floor_z],
                [b[0], b[1], floor_z],
                [b[0], b[1], top_z],
                [a[0], a[1], top_z],
                SurfaceClass::Rock,
            );
            mesh.triangle(
                [center[0], center[1], top_z],
                [a[0], a[1], top_z],
                [b[0], b[1], top_z],
                SurfaceClass::Rock,
            );
        }
    }
    Ok(())
}

fn rectangle_polygon(x0: f32, y0: f32, x1: f32, y1: f32) -> Polygon<f64> {
    geo_polygon(&[[x0, y0], [x1, y0], [x1, y1], [x0, y1]])
}

#[derive(Clone, Copy, PartialEq)]
enum SegmentWallSide {
    Inner,
    Outer,
}

fn add_segment_walls(
    mesh: &mut MeshBuilder,
    polygons: &[Polygon<f64>],
    inner: [f32; 4],
    side: SegmentWallSide,
    lower_z: f32,
    upper_z: f32,
) {
    let [x0, y0, x1, y1] = inner;
    let on_inner_boundary = |a: [f32; 2], b: [f32; 2]| {
        ((a[0] - x0).abs() < 0.0001 && (b[0] - x0).abs() < 0.0001)
            || ((a[0] - x1).abs() < 0.0001 && (b[0] - x1).abs() < 0.0001)
            || ((a[1] - y0).abs() < 0.0001 && (b[1] - y0).abs() < 0.0001)
            || ((a[1] - y1).abs() < 0.0001 && (b[1] - y1).abs() < 0.0001)
    };
    for polygon in polygons {
        for ring in std::iter::once(polygon.exterior()).chain(polygon.interiors()) {
            for edge in ring.0.windows(2) {
                let a = [edge[0].x as f32, edge[0].y as f32];
                let b = [edge[1].x as f32, edge[1].y as f32];
                let inner_edge = on_inner_boundary(a, b);
                if inner_edge == (side == SegmentWallSide::Inner) {
                    // Outer walls skirt the solid UNDER the polygon, inner
                    // (cavity) walls rise against the rim solid OUTSIDE it,
                    // so the two sides wind in opposite ring directions to
                    // both face away from their solid.
                    if side == SegmentWallSide::Inner {
                        mesh.quad(
                            [b[0], b[1], lower_z],
                            [a[0], a[1], lower_z],
                            [a[0], a[1], upper_z],
                            [b[0], b[1], upper_z],
                            SurfaceClass::Rock,
                        );
                    } else {
                        mesh.quad(
                            [a[0], a[1], lower_z],
                            [b[0], b[1], lower_z],
                            [b[0], b[1], upper_z],
                            [a[0], a[1], upper_z],
                            SurfaceClass::Rock,
                        );
                    }
                }
            }
        }
    }
}

/// Splits a contour path into the runs that fit inside one tray segment.
/// A point is kept only when it sits inside the outline AND at least
/// [`TRAY_CONTOUR_CLIP_INSET_MM`] away from it, so the ribbon built around
/// the centreline (half-width plus miter) can never protrude past the
/// segment's cut walls.
fn clip_contour_path(path: &ContourPath, segment: &Polygon<f64>) -> Vec<ContourPath> {
    let keep = |point: &[f32; 2]| {
        segment.contains(&Point::new(f64::from(point[0]), f64::from(point[1])))
            && polygon_boundary_distance(segment, *point) >= TRAY_CONTOUR_CLIP_INSET_MM
    };
    let mut paths = Vec::new();
    let mut current = Vec::new();
    let first_point_kept = path.points.first().is_some_and(&keep);
    for point in &path.points {
        if keep(point) {
            current.push(*point);
        } else if current.len() >= 2 {
            paths.push(ContourPath {
                points: std::mem::take(&mut current),
                closed: false,
            });
        } else {
            current.clear();
        }
    }
    if current.len() >= 2 {
        if path.closed && first_point_kept && !paths.is_empty() {
            // A closed contour partially inside the segment can wrap the
            // point-array seam: its first run (starting at index 0) and its
            // final run are one continuous arc of the contour. Join them so
            // the arc comes out as one open path instead of two butted at
            // the seam.
            let mut joined = current;
            joined.extend(paths[0].points.iter().copied());
            paths[0] = ContourPath {
                points: joined,
                closed: false,
            };
        } else {
            // Only a fully surviving loop stays closed; any dropped point
            // leaves an arc, and closing an arc would bridge the gap.
            let closed = path.closed && paths.is_empty() && current.len() == path.points.len();
            paths.push(ContourPath {
                points: current,
                closed,
            });
        }
    }
    paths
}

fn contour_paths_around_retention_pins(
    path: &ContourPath,
    centers: &[[f32; 2]],
    spec: &GenerationSpec,
) -> Vec<ContourPath> {
    if centers.is_empty() {
        return vec![path.clone()];
    }
    let exclusion = spec.puzzle_retention.pin_diameter_mm * 0.5 + TRAY_CONTOUR_WIDTH_MM * 1.5 + 0.1;
    let keep = |point: &[f32; 2]| {
        centers
            .iter()
            .all(|center| distance_squared(*point, *center) >= exclusion * exclusion)
    };
    if path.points.iter().all(&keep) {
        return vec![path.clone()];
    }

    let mut paths = Vec::new();
    let mut current = Vec::new();
    for point in &path.points {
        if keep(point) {
            current.push(*point);
        } else if current.len() >= 2 {
            paths.push(ContourPath {
                points: std::mem::take(&mut current),
                closed: false,
            });
        } else {
            current.clear();
        }
    }
    if current.len() >= 2 {
        paths.push(ContourPath {
            points: current,
            closed: false,
        });
    }
    paths
}

/// Distance from a point to the nearest edge of the polygon's rings.
fn polygon_boundary_distance(polygon: &Polygon<f64>, point: [f32; 2]) -> f32 {
    let mut best = f32::INFINITY;
    for ring in std::iter::once(polygon.exterior()).chain(polygon.interiors()) {
        for edge in ring.0.windows(2) {
            let start = [edge[0].x as f32, edge[0].y as f32];
            let end = [edge[1].x as f32, edge[1].y as f32];
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
    }
    best.sqrt()
}

#[derive(Debug, Clone)]
pub(crate) struct ContourPath {
    pub(crate) points: Vec<[f32; 2]>,
    pub(crate) closed: bool,
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct ContourSegment {
    start: [f32; 2],
    end: [f32; 2],
}

#[allow(clippy::too_many_arguments)]
fn trace_tray_contours(
    spec: &GenerationSpec,
    height_field: Option<&HeightField>,
    height_range: Option<(f32, f32)>,
    x_coordinates: &[f32],
    y_coordinates: &[f32],
    origin_x: f32,
    origin_y: f32,
    width: f32,
    height: f32,
) -> Vec<ContourPath> {
    let columns = x_coordinates.len();
    let rows = y_coordinates.len();
    let values = y_coordinates
        .iter()
        .flat_map(|y| {
            x_coordinates.iter().map(move |x| {
                normalized_height(
                    height_field,
                    height_range,
                    (*x - origin_x) / width,
                    (*y - origin_y) / height,
                    spec.center_lat,
                    spec.center_lon,
                )
            })
        })
        .collect::<Vec<_>>();
    let contour_count = spec.tray.contour_count as usize;
    let mut level_segments = vec![Vec::new(); contour_count];

    for row in 0..rows - 1 {
        for column in 0..columns - 1 {
            let points = [
                [x_coordinates[column], y_coordinates[row]],
                [x_coordinates[column + 1], y_coordinates[row]],
                [x_coordinates[column + 1], y_coordinates[row + 1]],
                [x_coordinates[column], y_coordinates[row + 1]],
            ];
            let cell_values = [
                values[row * columns + column],
                values[row * columns + column + 1],
                values[(row + 1) * columns + column + 1],
                values[(row + 1) * columns + column],
            ];
            let minimum = cell_values.iter().copied().fold(f32::INFINITY, f32::min);
            let maximum = cell_values
                .iter()
                .copied()
                .fold(f32::NEG_INFINITY, f32::max);
            let first_level = ((minimum * contour_count as f32).floor() as usize).max(1);
            let last_level =
                ((maximum * contour_count as f32).ceil() as usize).min(contour_count - 1);
            if first_level > last_level {
                continue;
            }
            for (level_index, segments) in level_segments
                .iter_mut()
                .enumerate()
                .take(last_level + 1)
                .skip(first_level)
            {
                let level = level_index as f32 / contour_count as f32 + 0.000_001;
                add_triangle_contour_segment(
                    [points[0], points[1], points[2]],
                    [cell_values[0], cell_values[1], cell_values[2]],
                    level,
                    segments,
                );
                add_triangle_contour_segment(
                    [points[0], points[2], points[3]],
                    [cell_values[0], cell_values[2], cell_values[3]],
                    level,
                    segments,
                );
            }
        }
    }

    level_segments
        .into_iter()
        .flat_map(stitch_contour_segments)
        .filter(|path| path.points.len() > 2)
        .map(smooth_contour_path)
        .collect()
}

pub(crate) fn add_triangle_contour_segment(
    points: [[f32; 2]; 3],
    values: [f32; 3],
    level: f32,
    output: &mut Vec<ContourSegment>,
) {
    let mut intersections = Vec::with_capacity(2);
    for [start, end] in [[0, 1], [1, 2], [2, 0]] {
        let start_above = values[start] >= level;
        let end_above = values[end] >= level;
        if start_above == end_above {
            continue;
        }
        let amount = ((level - values[start]) / (values[end] - values[start])).clamp(0.0, 1.0);
        let point = [
            points[start][0] + (points[end][0] - points[start][0]) * amount,
            points[start][1] + (points[end][1] - points[start][1]) * amount,
        ];
        if intersections
            .last()
            .is_none_or(|last| distance_squared(*last, point) > 0.000_000_01)
        {
            intersections.push(point);
        }
    }
    if intersections.len() == 2
        && distance_squared(intersections[0], intersections[1]) > 0.000_000_01
    {
        output.push(ContourSegment {
            start: intersections[0],
            end: intersections[1],
        });
    }
}

fn contour_point_key(point: [f32; 2]) -> (i64, i64) {
    (
        (point[0] * 1_000.0).round() as i64,
        (point[1] * 1_000.0).round() as i64,
    )
}

pub(crate) fn stitch_contour_segments(segments: Vec<ContourSegment>) -> Vec<ContourPath> {
    let mut adjacency = HashMap::<(i64, i64), Vec<(usize, bool)>>::new();
    for (index, segment) in segments.iter().enumerate() {
        adjacency
            .entry(contour_point_key(segment.start))
            .or_default()
            .push((index, false));
        adjacency
            .entry(contour_point_key(segment.end))
            .or_default()
            .push((index, true));
    }
    let mut visited = vec![false; segments.len()];
    let mut result = Vec::new();

    let start_order = (0..segments.len())
        .filter(|index| {
            let segment = segments[*index];
            adjacency
                .get(&contour_point_key(segment.start))
                .is_some_and(|edges| edges.len() != 2)
                || adjacency
                    .get(&contour_point_key(segment.end))
                    .is_some_and(|edges| edges.len() != 2)
        })
        .chain(0..segments.len())
        .collect::<Vec<_>>();
    for start_index in start_order {
        if visited[start_index] {
            continue;
        }
        let segment = segments[start_index];
        let start_at_end = adjacency
            .get(&contour_point_key(segment.start))
            .is_some_and(|edges| edges.len() == 2)
            && adjacency
                .get(&contour_point_key(segment.end))
                .is_some_and(|edges| edges.len() != 2);
        let first_point = if start_at_end {
            segment.end
        } else {
            segment.start
        };
        let mut points = vec![first_point];
        let mut current_index = start_index;
        let mut enter_at_end = start_at_end;
        let mut closed = false;

        loop {
            visited[current_index] = true;
            let current = segments[current_index];
            let next_point = if enter_at_end {
                current.start
            } else {
                current.end
            };
            if contour_point_key(next_point) == contour_point_key(first_point) && points.len() > 2 {
                closed = true;
                break;
            }
            points.push(next_point);
            let next_key = contour_point_key(next_point);
            let direction = unit_vector([
                next_point[0] - points[points.len() - 2][0],
                next_point[1] - points[points.len() - 2][1],
            ]);
            let next = adjacency.get(&next_key).and_then(|edges| {
                edges
                    .iter()
                    .copied()
                    .filter(|(index, _)| !visited[*index])
                    .max_by(|first, second| {
                        let score = |candidate: &(usize, bool)| {
                            let segment = segments[candidate.0];
                            let destination = if candidate.1 {
                                segment.start
                            } else {
                                segment.end
                            };
                            let candidate_direction = unit_vector([
                                destination[0] - next_point[0],
                                destination[1] - next_point[1],
                            ]);
                            direction[0] * candidate_direction[0]
                                + direction[1] * candidate_direction[1]
                        };
                        score(first).total_cmp(&score(second))
                    })
            });
            let Some((next_index, next_at_end)) = next else {
                break;
            };
            current_index = next_index;
            enter_at_end = next_at_end;
        }
        if points.len() > 2 {
            result.push(ContourPath { points, closed });
        }
    }
    result
}

pub(crate) fn smooth_contour_path(path: ContourPath) -> ContourPath {
    if path.points.len() < 4 {
        return path;
    }
    let mut points = Vec::new();
    let segment_count = if path.closed {
        path.points.len()
    } else {
        path.points.len() - 1
    };
    for index in 0..segment_count {
        let control = |offset: isize| {
            let raw_index = index as isize + offset;
            if path.closed {
                path.points[raw_index.rem_euclid(path.points.len() as isize) as usize]
            } else {
                path.points[raw_index.clamp(0, path.points.len() as isize - 1) as usize]
            }
        };
        let controls = [control(-1), control(0), control(1), control(2)];
        for sample in 0..4 {
            let t = sample as f32 / 4.0;
            let t2 = t * t;
            let t3 = t2 * t;
            let weights = [
                (1.0 - 3.0 * t + 3.0 * t2 - t3) / 6.0,
                (4.0 - 6.0 * t2 + 3.0 * t3) / 6.0,
                (1.0 + 3.0 * t + 3.0 * t2 - 3.0 * t3) / 6.0,
                t3 / 6.0,
            ];
            points.push([
                controls
                    .iter()
                    .zip(weights)
                    .map(|(point, weight)| point[0] * weight)
                    .sum(),
                controls
                    .iter()
                    .zip(weights)
                    .map(|(point, weight)| point[1] * weight)
                    .sum(),
            ]);
        }
    }
    if !path.closed {
        points.push(*path.points.last().unwrap());
    }
    let mut spaced_points = Vec::with_capacity(points.len());
    for point in points {
        if spaced_points
            .last()
            .is_none_or(|last| distance_squared(*last, point) >= 0.000_4)
        {
            spaced_points.push(point);
        }
    }
    if path.closed
        && spaced_points.len() > 2
        && distance_squared(spaced_points[0], *spaced_points.last().unwrap()) < 0.000_4
    {
        spaced_points.pop();
    }
    ContourPath {
        points: spaced_points,
        closed: path.closed,
    }
}

fn add_contour_ribbon(output: &mut MeshBuilder, path: &ContourPath, bottom_z: f32, top_z: f32) {
    if path.points.len() < 2 {
        return;
    }
    let half_width = TRAY_CONTOUR_WIDTH_MM * 0.5;
    let mut left = Vec::with_capacity(path.points.len());
    let mut right = Vec::with_capacity(path.points.len());
    for index in 0..path.points.len() {
        let point = path.points[index];
        let previous = if index > 0 {
            path.points[index - 1]
        } else if path.closed {
            path.points[path.points.len() - 1]
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
        let incoming = unit_vector([point[0] - previous[0], point[1] - previous[1]]);
        let outgoing = unit_vector([next[0] - point[0], next[1] - point[1]]);
        let incoming = if incoming == [0.0, 0.0] {
            outgoing
        } else {
            incoming
        };
        let outgoing = if outgoing == [0.0, 0.0] {
            incoming
        } else {
            outgoing
        };
        let incoming_normal = [-incoming[1], incoming[0]];
        let outgoing_normal = [-outgoing[1], outgoing[0]];
        let normal_sum = [
            incoming_normal[0] + outgoing_normal[0],
            incoming_normal[1] + outgoing_normal[1],
        ];
        let miter = if normal_sum == [0.0, 0.0] {
            outgoing_normal
        } else {
            unit_vector(normal_sum)
        };
        let denominator = (miter[0] * outgoing_normal[0] + miter[1] * outgoing_normal[1]).abs();
        let miter_length = (half_width / denominator.max(0.25)).min(half_width * 2.0);
        let offset = [miter[0] * miter_length, miter[1] * miter_length];
        left.push([point[0] + offset[0], point[1] + offset[1]]);
        right.push([point[0] - offset[0], point[1] - offset[1]]);
    }

    let segment_count = if path.closed {
        path.points.len()
    } else {
        path.points.len() - 1
    };
    let mut mesh = MeshBuilder::default();
    for index in 0..segment_count {
        let next = (index + 1) % path.points.len();
        mesh.quad(
            [left[index][0], left[index][1], top_z],
            [right[index][0], right[index][1], top_z],
            [right[next][0], right[next][1], top_z],
            [left[next][0], left[next][1], top_z],
            SurfaceClass::Forest,
        );
        mesh.quad(
            [left[next][0], left[next][1], bottom_z],
            [right[next][0], right[next][1], bottom_z],
            [right[index][0], right[index][1], bottom_z],
            [left[index][0], left[index][1], bottom_z],
            SurfaceClass::Forest,
        );
        // Walls and caps wind so their normals face away from the ribbon
        // body, matching the top (+z) and bottom (-z) faces: every shared
        // edge is then traversed once in each direction, which the manifold
        // analyzer's misoriented-edge counter checks.
        mesh.quad(
            [left[next][0], left[next][1], bottom_z],
            [left[index][0], left[index][1], bottom_z],
            [left[index][0], left[index][1], top_z],
            [left[next][0], left[next][1], top_z],
            SurfaceClass::Forest,
        );
        mesh.quad(
            [right[index][0], right[index][1], bottom_z],
            [right[next][0], right[next][1], bottom_z],
            [right[next][0], right[next][1], top_z],
            [right[index][0], right[index][1], top_z],
            SurfaceClass::Forest,
        );
    }
    if !path.closed {
        let last = path.points.len() - 1;
        mesh.quad(
            [left[0][0], left[0][1], bottom_z],
            [right[0][0], right[0][1], bottom_z],
            [right[0][0], right[0][1], top_z],
            [left[0][0], left[0][1], top_z],
            SurfaceClass::Forest,
        );
        mesh.quad(
            [right[last][0], right[last][1], bottom_z],
            [left[last][0], left[last][1], bottom_z],
            [left[last][0], left[last][1], top_z],
            [right[last][0], right[last][1], top_z],
            SurfaceClass::Forest,
        );
    }
    output.append_isolated(mesh);
}

fn regular_coordinates(start: f32, end: f32, maximum_step: f32) -> Vec<f32> {
    let segments = (((end - start) / maximum_step).ceil().max(1.0) as usize).min(1_024);
    (0..=segments)
        .map(|index| start + (end - start) * index as f32 / segments as f32)
        .collect()
}

fn insert_coordinate(coordinates: &mut Vec<f32>, value: f32) {
    coordinates.push(value);
    coordinates.sort_by(f32::total_cmp);
    coordinates.dedup_by(|a, b| (*a - *b).abs() < 0.000_01);
}

fn tray_label(spec: &GenerationSpec, width: f32, lip_depth: f32) -> Result<EmbossedLabel> {
    let place = spec
        .place_name
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    let decimals = coordinate_decimals(spec.ground_span_km);
    let latitude = coordinate_label(spec.center_lat, decimals, 'N', 'S');
    let longitude = coordinate_label(spec.center_lon, decimals, 'E', 'W');
    let text = format!("{place}  {latitude} {longitude}");
    let fonts = embossing_fonts(spec.tray.label_font)?;
    let metrics = text_metrics(&fonts, &text)?;
    let horizontal_margin = 2.0_f32.min(width * 0.1);
    let vertical_margin = 0.8_f32.min(lip_depth * 0.15);
    let available_width = (width - horizontal_margin * 2.0).max(1.0);
    let available_height = (lip_depth - vertical_margin * 2.0).max(1.0);
    let scale = (spec.tray.label_height_mm / metrics.height)
        .min(available_width / metrics.width)
        .min(available_height / metrics.height);
    let text_width = metrics.width * scale;
    let text_height = metrics.height * scale;
    let left = match spec.tray.label_position {
        TrayLabelPosition::Left => horizontal_margin,
        TrayLabelPosition::Center => (width - text_width) * 0.5,
        TrayLabelPosition::Right => width - horizontal_margin - text_width,
    };
    Ok(EmbossedLabel {
        text,
        font: spec.tray.label_font,
        origin_x: left - metrics.minimum_x * scale,
        baseline_y: (lip_depth - text_height) * 0.5 - metrics.minimum_y * scale,
        scale,
    })
}

fn coordinate_label(value: f64, decimals: usize, positive: char, negative: char) -> String {
    format!(
        "{:.*}{}",
        decimals,
        value.abs(),
        if value >= 0.0 { positive } else { negative }
    )
}

/// How many decimals a coordinate on the base is worth printing.
///
/// A fixed four put roughly eleven metres of precision on every base,
/// whatever it showed — digits an eighty-kilometre map cannot support and
/// nobody reads off a printed rim. The label is cut to no finer than a
/// twentieth of the map's own width, which still names the centre well
/// inside the model, and rounded to whole decimal places so the number
/// stays a number a person can read back.
fn coordinate_decimals(ground_span_km: f64) -> usize {
    /// Metres per degree of latitude. Longitude narrows toward the poles,
    /// but both halves of the label share one precision: a coordinate pair
    /// written to two different widths reads as a mistake.
    const METRES_PER_DEGREE: f64 = 111_320.0;
    let tolerance_m = (ground_span_km * 1_000.0 * 0.05).max(f64::MIN_POSITIVE);
    let decimals = (METRES_PER_DEGREE / tolerance_m).log10().ceil();
    (decimals as i64).clamp(1, 4) as usize
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{fs, fs::File, io::Read};

    use crate::mesh::assert_watertight;
    use crate::project::{generate_project, generate_project_with_height_field};
    use crate::spec::{
        LabelFont, OutlineShape, PuzzleRetentionSpec, TraySpec, WallMountSpec, WallMountStyle,
        WallMountTarget,
    };

    #[test]
    fn a_shaped_tray_follows_the_terrain_outline() {
        let mut spec = GenerationSpec {
            solid_model: true,
            width_mm: 120.0,
            rows: 2,
            columns: 4,
            tray: TraySpec {
                enabled: true,
                label_enabled: false,
                contours_enabled: false,
                ..TraySpec::default()
            },
            puzzle_retention: PuzzleRetentionSpec {
                enabled: true,
                ..PuzzleRetentionSpec::default()
            },
            ..GenerationSpec::default()
        };
        spec.model_outline.shape = OutlineShape::Circle;
        spec.validate().unwrap();
        let terrain = geo_polygon(&model_outline_mm(&spec, 96));
        let cavity = largest_polygon(terrain.buffer(f64::from(spec.tray.clearance_mm))).unwrap();
        let outer = largest_polygon(cavity.buffer(f64::from(spec.tray.rim_width_mm))).unwrap();
        let outer_bounds = outer.bounding_rect().unwrap();
        let shift = [-outer_bounds.min().x as f32, -outer_bounds.min().y as f32];
        let centers = tray_retention_centers_at(&spec, shift).unwrap();
        assert_eq!(centers.len(), 1);
        assert!((centers[0][0] - 38.6).abs() < 0.2);
        assert!((centers[0][1] - 38.6).abs() < 0.2);
        let mesh = build_tray(&spec, None).unwrap();
        assert_watertight(&mesh);
        let bounds = mesh.vertices.iter().fold(
            [
                f32::INFINITY,
                f32::NEG_INFINITY,
                f32::INFINITY,
                f32::NEG_INFINITY,
            ],
            |bounds, point| {
                [
                    bounds[0].min(point[0]),
                    bounds[1].max(point[0]),
                    bounds[2].min(point[1]),
                    bounds[3].max(point[1]),
                ]
            },
        );
        assert!((bounds[1] - bounds[0] - 77.2).abs() < 0.2);
        assert!((bounds[3] - bounds[2] - 77.2).abs() < 0.2);
    }

    #[test]
    fn tray_is_watertight_and_keeps_contours_and_label_colors() {
        let spec = GenerationSpec {
            width_mm: 60.0,
            rows: 2,
            columns: 2,
            place_name: "Mount Rainier".into(),
            tray: TraySpec {
                enabled: true,
                ..TraySpec::default()
            },
            ..GenerationSpec::default()
        };
        let height = HeightField::new(
            3,
            3,
            vec![0.0, 1.0, 2.0, 1.0, 3.0, 5.0, 2.0, 5.0, 8.0],
            "test",
        )
        .unwrap();
        let mesh = build_tray(&spec, Some(&height)).unwrap();
        assert_watertight(&mesh);
        assert!(mesh.materials.contains(&SurfaceClass::Rock.into()));
        assert!(mesh.materials.contains(&SurfaceClass::Forest.into()));
        assert!(mesh.materials.contains(&SurfaceClass::Snow.into()));
        let rim_z = spec.tray.floor_mm + spec.tray.rim_height_mm;
        let raised_label = mesh
            .triangles
            .iter()
            .zip(&mesh.materials)
            .filter(|(_, material)| **material == SurfaceClass::Snow)
            .flat_map(|(triangle, _)| triangle)
            .map(|index| mesh.vertices[*index as usize])
            .collect::<Vec<_>>();
        assert!(raised_label.iter().any(|vertex| vertex[2] > rim_z));
        assert!(
            raised_label
                .iter()
                .all(|vertex| vertex[1] < spec.tray.rim_width_mm)
        );
    }

    #[test]
    fn tray_can_omit_contour_geometry() {
        let spec = GenerationSpec {
            width_mm: 60.0,
            rows: 2,
            columns: 2,
            tray: TraySpec {
                enabled: true,
                contours_enabled: false,
                ..TraySpec::default()
            },
            ..GenerationSpec::default()
        };
        let height = HeightField::new(
            3,
            3,
            vec![0.0, 1.0, 2.0, 1.0, 3.0, 5.0, 2.0, 5.0, 8.0],
            "test",
        )
        .unwrap();
        let mesh = build_tray(&spec, Some(&height)).unwrap();
        assert_watertight(&mesh);
        assert!(!mesh.materials.contains(&SurfaceClass::Forest.into()));
    }

    #[test]
    fn every_wall_mount_style_cuts_watertight_tray_sections() {
        for style in [
            WallMountStyle::StraightPin,
            WallMountStyle::AngledPin,
            WallMountStyle::FrenchCleat,
        ] {
            let spec = GenerationSpec {
                width_mm: 120.0,
                rows: 2,
                columns: 2,
                adjacent_interlocks: true,
                tray: TraySpec {
                    enabled: true,
                    contours_enabled: false,
                    floor_mm: 2.8,
                    segment_columns: 2,
                    segment_rows: 2,
                    ..TraySpec::default()
                },
                wall_mount: WallMountSpec {
                    style,
                    target: WallMountTarget::Tray,
                    vertical_position_ratio: 0.5,
                    pin_diameter_mm: 4.0,
                    ..WallMountSpec::default()
                },
                ..GenerationSpec::default()
            };
            let mut whole_spec = spec.clone();
            whole_spec.tray.segment_columns = 1;
            whole_spec.tray.segment_rows = 1;
            let whole_trays = build_tray_segments(&whole_spec, None).unwrap();
            assert_eq!(whole_trays.len(), 1);
            assert_watertight(&whole_trays[0]);
            let segments = build_tray_segments(&spec, None)
                .unwrap_or_else(|error| panic!("{style:?} split tray failed: {error:#}"));
            assert_eq!(segments.len(), 4);
            for segment in &segments {
                assert_watertight(segment);
                assert!(segment.vertices.iter().any(|vertex| {
                    (vertex[2] - spec.wall_mount.pocket_depth_mm()).abs() < 0.000_01
                }));
                assert!(segment.vertices.iter().any(|vertex| {
                    (vertex[2] - spec.wall_mount.embedded_depth_mm()).abs() < 0.000_01
                }));
            }
        }
    }

    #[test]
    fn tray_retention_pins_stay_watertight_in_whole_and_split_trays() {
        let spec = GenerationSpec {
            width_mm: 80.0,
            rows: 3,
            columns: 3,
            adjacent_interlocks: false,
            tray: TraySpec {
                enabled: true,
                contours_enabled: false,
                segment_columns: 2,
                segment_rows: 2,
                ..TraySpec::default()
            },
            puzzle_retention: PuzzleRetentionSpec {
                enabled: true,
                ..PuzzleRetentionSpec::default()
            },
            ..GenerationSpec::default()
        };
        let segments = build_tray_segments(&spec, None).unwrap();
        assert_eq!(segments.len(), 4);
        for segment in &segments {
            assert_watertight(segment);
            assert!(segment.vertices.iter().any(|vertex| {
                (vertex[2] - (spec.tray.floor_mm + spec.puzzle_retention.pin_height_mm)).abs()
                    < 0.000_01
            }));
        }

        let solid_spec = GenerationSpec {
            solid_model: true,
            ..spec
        };
        let solid = crate::piece::build_piece(&solid_spec, None, None, 0, 0).unwrap();
        assert_watertight(&solid);
        let solid_segments = build_tray_segments(&solid_spec, None).unwrap();
        assert_eq!(solid_segments.len(), 4);
        for segment in &solid_segments {
            assert_watertight(segment);
            assert!(segment.vertices.iter().any(|vertex| {
                (vertex[2] - (solid_spec.tray.floor_mm + solid_spec.puzzle_retention.pin_height_mm))
                    .abs()
                    < 0.000_01
            }));
        }
    }

    #[test]
    fn segmented_tray_exports_watertight_interlocking_parts() {
        let spec = GenerationSpec {
            width_mm: 60.0,
            rows: 2,
            columns: 2,
            place_name: "Test".into(),
            adjacent_interlocks: true,
            tray: TraySpec {
                enabled: true,
                segment_columns: 2,
                segment_rows: 2,
                contour_count: 5,
                ..TraySpec::default()
            },
            ..GenerationSpec::default()
        };
        let height = HeightField::new(
            3,
            3,
            vec![0.0, 1.0, 2.0, 1.0, 3.0, 5.0, 2.0, 5.0, 8.0],
            "test",
        )
        .unwrap();
        let segments = build_tray_segments(&spec, Some(&height)).unwrap();

        assert_eq!(segments.len(), 4);
        for segment in &segments {
            assert_watertight(segment);
            let curved_cut_walls = segment
                .triangles
                .iter()
                .filter(|triangle| {
                    let vertices = triangle.map(|index| segment.vertices[index as usize]);
                    let minimum_z = vertices
                        .iter()
                        .map(|vertex| vertex[2])
                        .fold(f32::INFINITY, f32::min);
                    let maximum_z = vertices
                        .iter()
                        .map(|vertex| vertex[2])
                        .fold(f32::NEG_INFINITY, f32::max);
                    minimum_z < 0.001
                        && (maximum_z - spec.tray.floor_mm).abs() < 0.001
                        && (0..3).any(|index| {
                            let a = vertices[index];
                            let b = vertices[(index + 1) % 3];
                            (a[0] - b[0]).abs() > 0.001 && (a[1] - b[1]).abs() > 0.001
                        })
                })
                .count();
            assert!(curved_cut_walls > 20);
        }
        let output_dir = std::env::temp_dir().join(format!(
            "toposaic-segmented-tray-test-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&output_dir);
        let mut export_spec = spec.clone();
        export_spec.solid_model = true;
        export_spec.samples_per_piece = 16;
        export_spec.overlay_samples_per_piece = 32;
        let manifest =
            generate_project_with_height_field(&export_spec, &height, &output_dir).unwrap();
        assert!(
            manifest
                .artifacts
                .iter()
                .any(|artifact| artifact.name == "terrain-tray-r01-c01.3mf")
        );
        assert!(output_dir.join("terrain-tray-r02-c02.stl").is_file());
        fs::remove_dir_all(&output_dir).unwrap();

        let tabbed_grid = TraySegmentGrid {
            size: [80.0, 80.0],
            terrain_bounds: [8.0, 8.0, 72.0, 72.0],
            rows: 2,
            columns: 2,
            puzzle_seed: 7,
            interlocks: true,
            clearance_mm: 0.14,
        };
        let first = tray_segment_outline(tabbed_grid, 0, 0);
        let second = tray_segment_outline(tabbed_grid, 0, 1);
        let first_shared = &first[96..192];
        let second_shared = &second[288..384];
        assert!(
            first_shared
                .iter()
                .any(|point| (point[0] - 40.0).abs() > 1.0)
        );
        let shared_clearance = first_shared
            .iter()
            .skip(2)
            .take(first_shared.len() - 4)
            .map(|point| {
                second_shared
                    .iter()
                    .map(|candidate| (point[0] - candidate[0]).hypot(point[1] - candidate[1]))
                    .fold(f32::INFINITY, f32::min)
            })
            .fold(f32::INFINITY, f32::min);
        assert!((0.1..=0.2).contains(&shared_clearance));

        let straight = tray_segment_outline(
            TraySegmentGrid {
                interlocks: false,
                clearance_mm: 0.0,
                ..tabbed_grid
            },
            0,
            0,
        );
        assert!(
            straight[96..192]
                .iter()
                .all(|point| (point[0] - 40.0).abs() < 0.0001)
        );

        let four_across = tray_segment_outline(
            TraySegmentGrid {
                size: [110.0, 80.0],
                terrain_bounds: [5.0, 8.0, 105.0, 72.0],
                rows: 1,
                columns: 4,
                puzzle_seed: 7,
                interlocks: false,
                clearance_mm: 0.0,
            },
            0,
            0,
        );
        assert!(
            four_across[96..192]
                .iter()
                .all(|point| (point[0] - 30.0).abs() < 0.0001)
        );
    }

    #[test]
    fn clipped_contours_keep_ribbon_clearance_from_segment_walls() {
        let segment = rectangle_polygon(0.0, 0.0, 20.0, 20.0);
        // A straight contour running 0.1 mm inside the right wall: inside
        // the polygon, but a 0.45 mm-wide mitred ribbon around it would
        // protrude through the cut. Every point must be dropped.
        let hugging = ContourPath {
            points: (0..8)
                .map(|index| [19.9, 2.0 + index as f32 * 2.0])
                .collect(),
            closed: false,
        };
        assert!(clip_contour_path(&hugging, &segment).is_empty());

        // A contour crossing the wall keeps only points with clearance for
        // the ribbon half-width plus its miter allowance.
        let crossing = ContourPath {
            points: (0..21).map(|index| [index as f32, 10.0]).collect(),
            closed: false,
        };
        let clipped = clip_contour_path(&crossing, &segment);
        assert_eq!(clipped.len(), 1);
        for point in &clipped[0].points {
            assert!(
                polygon_boundary_distance(&segment, *point) >= TRAY_CONTOUR_CLIP_INSET_MM,
                "point {point:?} too close to the segment wall"
            );
        }
    }

    #[test]
    fn closed_contours_partially_inside_join_across_the_array_seam() {
        // Only x >= 6 of this 16-point loop is inside the segment. The loop
        // starts INSIDE (index 0), leaves, and re-enters before wrapping:
        // the surviving arc crosses the point-array seam and must come out
        // as ONE open path, not two runs butted at the seam.
        let segment = rectangle_polygon(6.0, -30.0, 60.0, 30.0);
        let loop_points = (0..16)
            .map(|index| {
                let angle = index as f32 / 16.0 * std::f32::consts::TAU;
                [12.0 * angle.cos(), 12.0 * angle.sin()]
            })
            .collect::<Vec<_>>();
        let path = ContourPath {
            points: loop_points.clone(),
            closed: true,
        };
        let clipped = clip_contour_path(&path, &segment);
        assert_eq!(clipped.len(), 1, "the wrapped arc must join into one path");
        assert!(!clipped[0].closed, "a partial arc must not close");
        // The joined path runs the final run first (indices 13..) and then
        // wraps into the first run (indices 0..): consecutive original
        // ordering across the seam.
        let arc = &clipped[0].points;
        assert!(arc.len() >= 4);
        assert_eq!(arc.last(), loop_points.get(2));
        assert!(arc.contains(&loop_points[0]));
        assert!(arc.contains(&loop_points[14]));
        let seam = arc
            .windows(2)
            .position(|pair| pair[0] == loop_points[15] && pair[1] == loop_points[0]);
        assert!(seam.is_some(), "the seam neighbours must stay adjacent");

        // A fully inside loop still comes back closed and untouched.
        let wide = rectangle_polygon(-30.0, -30.0, 30.0, 30.0);
        let whole = clip_contour_path(&path, &wide);
        assert_eq!(whole.len(), 1);
        assert!(whole[0].closed);
        assert_eq!(whole[0].points, loop_points);
    }

    #[test]
    fn tray_label_uses_smooth_vector_curves() {
        let label = EmbossedLabel {
            text: "O".into(),
            font: LabelFont::AtkinsonHyperlegible,
            origin_x: 1.0,
            baseline_y: 1.0,
            scale: 0.005,
        };
        let mut builder = MeshBuilder::default();
        label.add_embossed_shapes(&mut builder, 3.0).unwrap();
        let mesh = builder.finish("vector-label");
        assert_watertight(&mesh);

        let slanted_side_edges = mesh
            .triangles
            .iter()
            .filter(|triangle| {
                let vertices = triangle.map(|index| mesh.vertices[index as usize]);
                let spans_height = vertices
                    .iter()
                    .map(|vertex| vertex[2])
                    .fold(f32::INFINITY, f32::min)
                    < vertices
                        .iter()
                        .map(|vertex| vertex[2])
                        .fold(f32::NEG_INFINITY, f32::max);
                spans_height
                    && (0..3).any(|index| {
                        let a = vertices[index];
                        let b = vertices[(index + 1) % 3];
                        (a[2] - b[2]).abs() < 0.000_01
                            && (a[0] - b[0]).abs() > 0.000_01
                            && (a[1] - b[1]).abs() > 0.000_01
                    })
            })
            .count();
        assert!(
            slanted_side_edges > 24,
            "expected a smooth O outline, found {slanted_side_edges} curved segments"
        );
    }

    #[test]
    fn tray_label_preserves_case_and_embosses_japanese() {
        let spec = GenerationSpec {
            place_name: "富士山 Mount Fuji".into(),
            tray: TraySpec {
                label_height_mm: 3.0,
                ..TraySpec::default()
            },
            ..GenerationSpec::default()
        };
        let label = tray_label(&spec, 180.0, 10.0).unwrap();
        assert!(label.text.starts_with("富士山 Mount Fuji  "));
        assert!(!label.text.contains("MOUNT FUJI"));

        let mut builder = MeshBuilder::default();
        label.add_embossed_shapes(&mut builder, 3.0).unwrap();
        let mesh = builder.finish("japanese-vector-label");
        assert_watertight(&mesh);
        assert!(mesh.triangles.len() > 100);
    }

    #[test]
    fn tray_label_fonts_emboss_multilingual_watertight_text() {
        let mut widths = Vec::new();
        for font in [
            LabelFont::AtkinsonHyperlegible,
            LabelFont::NotoSans,
            LabelFont::B612Mono,
        ] {
            let spec = GenerationSpec {
                place_name: "Hạ Long Москва 富士山".into(),
                tray: TraySpec {
                    label_font: font,
                    label_height_mm: 3.0,
                    ..TraySpec::default()
                },
                ..GenerationSpec::default()
            };
            let label = tray_label(&spec, 180.0, 10.0).unwrap();
            assert_eq!(label.font, font);
            assert!(label.text.starts_with("Hạ Long Москва 富士山  "));

            let metrics = text_metrics(&embossing_fonts(font).unwrap(), "TopoSaic 123").unwrap();
            widths.push(metrics.width);
            let mut builder = MeshBuilder::default();
            label.add_embossed_shapes(&mut builder, 3.0).unwrap();
            assert_watertight(&builder.finish("multilingual-vector-label"));
        }
        assert!(
            widths
                .windows(2)
                .all(|pair| (pair[0] - pair[1]).abs() > 1.0)
        );
    }

    #[test]
    fn tray_label_height_and_position_control_the_layout() {
        let mut spec = GenerationSpec {
            place_name: "Fuji".into(),
            tray: TraySpec {
                label_height_mm: 2.0,
                label_position: TrayLabelPosition::Left,
                ..TraySpec::default()
            },
            ..GenerationSpec::default()
        };
        let left = tray_label(&spec, 240.0, 12.0).unwrap();
        spec.tray.label_position = TrayLabelPosition::Center;
        let center = tray_label(&spec, 240.0, 12.0).unwrap();
        spec.tray.label_position = TrayLabelPosition::Right;
        let right = tray_label(&spec, 240.0, 12.0).unwrap();
        assert!(left.origin_x < center.origin_x);
        assert!(center.origin_x < right.origin_x);

        spec.tray.label_position = TrayLabelPosition::Center;
        spec.tray.label_height_mm = 4.0;
        let larger = tray_label(&spec, 240.0, 12.0).unwrap();
        assert!((larger.scale / center.scale - 2.0).abs() < 0.01);
    }

    #[test]
    fn tray_label_reports_unsupported_characters() {
        let spec = GenerationSpec {
            place_name: "Fuji 🗻".into(),
            ..GenerationSpec::default()
        };
        let error = tray_label(&spec, 180.0, 10.0).unwrap_err();
        assert!(error.to_string().contains("cannot render"));
        assert!(error.to_string().contains("🗻"));
    }

    #[test]
    fn tray_contours_are_continuous_spline_ribbons() {
        let size = 9;
        let values = (0..size)
            .flat_map(|y| {
                (0..size).map(move |x| {
                    let dx = x as f32 - 4.0;
                    let dy = y as f32 - 4.0;
                    32.0 - dx * dx - dy * dy
                })
            })
            .collect::<Vec<_>>();
        let height = HeightField::new(size, size, values, "radial-test").unwrap();
        let spec = GenerationSpec {
            width_mm: 60.0,
            tray: TraySpec {
                contour_count: 8,
                ..TraySpec::default()
            },
            ..GenerationSpec::default()
        };
        let coordinates = regular_coordinates(0.0, 60.0, 0.35);
        let paths = trace_tray_contours(
            &spec,
            Some(&height),
            Some(height.range()),
            &coordinates,
            &coordinates,
            0.0,
            0.0,
            60.0,
            60.0,
        );
        let longest_path = paths
            .iter()
            .max_by_key(|path| path.points.len())
            .expect("radial terrain should produce contour paths");
        assert!(
            paths.iter().any(|path| path.closed),
            "radial terrain should produce closed contour loops"
        );
        assert!(longest_path.points.len() > 100);
        assert!(
            longest_path
                .points
                .windows(2)
                .all(|points| { distance_squared(points[0], points[1]).sqrt() < 0.4 })
        );
        let curved_turns = longest_path
            .points
            .windows(3)
            .filter(|points| {
                let incoming =
                    unit_vector([points[1][0] - points[0][0], points[1][1] - points[0][1]]);
                let outgoing =
                    unit_vector([points[2][0] - points[1][0], points[2][1] - points[1][1]]);
                incoming[0] * outgoing[0] + incoming[1] * outgoing[1] < 0.999_99
            })
            .count();
        assert!(curved_turns > 20);

        let mut builder = MeshBuilder::default();
        for path in &paths {
            add_contour_ribbon(&mut builder, path, 1.4, 1.61);
        }
        assert_watertight(&builder.finish("spline-contours"));
    }

    #[test]
    fn tray_exports_separate_stl_and_color_3mf() {
        let output_dir =
            std::env::temp_dir().join(format!("terrain-tray-core-test-{}", std::process::id()));
        if output_dir.exists() {
            std::fs::remove_dir_all(&output_dir).unwrap();
        }
        let spec = GenerationSpec {
            width_mm: 60.0,
            rows: 2,
            columns: 2,
            samples_per_piece: 16,
            tray: TraySpec {
                enabled: true,
                ..TraySpec::default()
            },
            ..GenerationSpec::default()
        };
        let manifest = generate_project(&spec, &output_dir).unwrap();
        assert!(output_dir.join("terrain-tray.stl").is_file());
        assert!(output_dir.join("terrain-tray.3mf").is_file());
        assert!(
            manifest
                .artifacts
                .iter()
                .any(|artifact| artifact.name == "terrain-tray.3mf")
        );

        let file = File::open(output_dir.join("terrain-tray.3mf")).unwrap();
        let mut archive = zip::ZipArchive::new(file).unwrap();
        let mut model = String::new();
        archive
            .by_name("3D/3dmodel.model")
            .unwrap()
            .read_to_string(&mut model)
            .unwrap();
        assert!(model.contains("color=\"#252822FF\""));
        assert!(model.contains("color=\"#E7E4D8FF\""));
        assert!(model.contains("color=\"#F4F3ECFF\""));
        assert!(model.contains("p1=\"1\""));
        assert!(model.contains("p1=\"2\""));

        std::fs::remove_dir_all(output_dir).unwrap();
    }

    #[test]
    fn tray_contour_sampling_uses_submillimetre_steps() {
        let coordinates = regular_coordinates(0.0, 180.0, 0.35);
        let largest = coordinates
            .windows(2)
            .map(|pair| pair[1] - pair[0])
            .fold(0.0, f32::max);
        assert!(largest <= 0.351);
    }

    /// A base's label is read off a printed rim, so it carries no more
    /// precision than the map it names can support.
    #[test]
    fn base_coordinates_are_cut_to_the_scale_of_the_map() {
        // A twentieth of the map's width, in whole decimal places.
        assert_eq!(coordinate_decimals(80.0), 2);
        assert_eq!(coordinate_decimals(18.0), 3);
        assert_eq!(coordinate_decimals(2.0), 4);
        // Never finer than four, however close the view.
        assert_eq!(coordinate_decimals(0.25), 4);
        assert_eq!(coordinate_decimals(0.01), 4);
        // Never coarser as the map widens, and never below one place.
        let mut previous = 5;
        for span in [0.25, 1.0, 2.0, 6.0, 18.0, 40.0, 80.0, 500.0] {
            let decimals = coordinate_decimals(span);
            assert!(decimals <= previous, "{span} km rose to {decimals}");
            assert!(decimals >= 1, "{span} km fell to {decimals}");
            previous = decimals;
        }

        let mut spec = GenerationSpec {
            width_mm: 180.0,
            ground_span_km: 18.0,
            ..GenerationSpec::default()
        };
        spec.tray.enabled = true;
        let wide = tray_label(&spec, 180.0, 10.0).unwrap();
        assert!(wide.text.contains("46.852N"), "{}", wide.text);
        assert!(!wide.text.contains("46.8523N"), "{}", wide.text);

        spec.ground_span_km = 2.0;
        let close = tray_label(&spec, 180.0, 10.0).unwrap();
        assert!(close.text.contains("46.8523N"), "{}", close.text);
    }

    /// A base can be left plain: with the label off nothing is embossed on
    /// its rim, and the base is still watertight without it.
    #[test]
    fn a_base_can_be_printed_without_its_label() {
        let mut spec = GenerationSpec {
            rows: 2,
            columns: 2,
            samples_per_piece: 16,
            ..GenerationSpec::default()
        };
        spec.tray.enabled = true;
        // Contours are not what this is about, and they have their own
        // watertightness trouble on a synthetic height field.
        spec.tray.contours_enabled = false;
        spec.place_name = "Mount Rainier".into();

        let labelled = build_tray(&spec, None).unwrap();
        spec.tray.label_enabled = false;
        let plain = build_tray(&spec, None).unwrap();

        assert!(
            plain.triangles.len() < labelled.triangles.len(),
            "the label's geometry should be gone: {} vs {}",
            plain.triangles.len(),
            labelled.triangles.len()
        );
        assert_watertight(&plain);
    }
}
