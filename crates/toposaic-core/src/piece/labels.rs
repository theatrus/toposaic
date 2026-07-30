//! Manual vector labels placed on the terrain or on a flat raised plaque.

use anyhow::Result;
use geo::{
    Area, BooleanOps, BoundingRect, Contains, Coord, InteriorPoint, LineString, MultiPolygon,
    Point, Polygon, Translate,
};

use crate::heightfield::{HeightField, normalized_height};
use crate::mesh::{Mesh, MeshBuilder};
use crate::spec::{GenerationSpec, MapMarker, MarkerKind, SurfaceClass};
use crate::text::{EmbossedLabel, embossing_fonts, text_metrics};

use super::overlays::build_polygon_shell;
use super::{MINIMUM_OVERLAY_AREA_MM2, OVERLAY_TERRAIN_EMBED_MM, sanitize_footprint_group};

const LABEL_TERRAIN_STEP_MM: f32 = 0.5;
const MAX_PLAQUE_HEIGHT_SAMPLES: usize = 128;

struct PreparedLabel {
    text: MultiPolygon<f64>,
    plaque: Polygon<f64>,
}

#[allow(clippy::too_many_arguments)]
pub(super) fn append_label_geometry(
    mesh: &mut Mesh,
    spec: &GenerationSpec,
    height_field: Option<&HeightField>,
    height_range: Option<(f32, f32)>,
    piece_outline: &[[f32; 2]],
    origin_x: f32,
    origin_y: f32,
    assembled_width: f32,
    assembled_height: f32,
) -> Result<()> {
    let piece_polygon = polygon_from_points(piece_outline);
    for marker in spec
        .markers
        .iter()
        .filter(|marker| marker.kind.is_map_label())
    {
        let uv = spec.normalized_map_point(marker.latitude, marker.longitude);
        let center = [uv[0] * assembled_width, uv[1] * assembled_height];
        let label_style = marker.label_style();
        let prepared = prepare_label(marker, center, spec.terrain_rotation_degrees as f32)?;
        let local_text = prepared
            .text
            .translate(-f64::from(origin_x), -f64::from(origin_y));
        let text_area = sanitize_footprint_group(local_text.intersection(&piece_polygon), false);
        let terrain_z = |point: [f32; 2]| {
            let u = ((point[0] + origin_x) / assembled_width).clamp(0.0, 1.0);
            let v = ((point[1] + origin_y) / assembled_height).clamp(0.0, 1.0);
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
        };

        if marker.kind == MarkerKind::PlaqueLabel {
            let plaque_top = plaque_top_z(
                spec,
                height_field,
                height_range,
                &prepared.plaque,
                assembled_width,
                assembled_height,
                label_style.plaque_thickness_mm,
            );
            let local_plaque = prepared
                .plaque
                .translate(-f64::from(origin_x), -f64::from(origin_y));
            let plaque_area =
                sanitize_footprint_group(local_plaque.intersection(&piece_polygon), false);
            append_shells(
                mesh,
                &plaque_area,
                |point| terrain_z(point) - OVERLAY_TERRAIN_EMBED_MM,
                |_| plaque_top,
                Some(LABEL_TERRAIN_STEP_MM),
                SurfaceClass::Marker,
                "triangulate map label plaque",
            )?;
            if !text_area.0.is_empty() {
                append_shells(
                    mesh,
                    &text_area,
                    |_| plaque_top - OVERLAY_TERRAIN_EMBED_MM,
                    |_| plaque_top + label_style.relief_mm,
                    None,
                    SurfaceClass::Snow,
                    "triangulate plaque label text",
                )?;
            }
        } else if !text_area.0.is_empty() {
            append_shells(
                mesh,
                &text_area,
                |point| terrain_z(point) - OVERLAY_TERRAIN_EMBED_MM,
                |point| terrain_z(point) + label_style.relief_mm,
                None,
                SurfaceClass::Marker,
                "triangulate surface label text",
            )?;
        }
    }
    Ok(())
}

fn prepare_label(
    marker: &MapMarker,
    center: [f32; 2],
    terrain_rotation_degrees: f32,
) -> Result<PreparedLabel> {
    let text = marker.name.split_whitespace().collect::<Vec<_>>().join(" ");
    let label_style = marker.label_style();
    let fonts = embossing_fonts(label_style.label_font)?;
    let metrics = text_metrics(&fonts, &text)?;
    let scale = marker.label_height_mm / metrics.height;
    let text_width = metrics.width * scale;
    let origin_x = center[0] - text_width * 0.5 - metrics.minimum_x * scale;
    let baseline_y = center[1] - marker.label_height_mm * 0.5 - metrics.minimum_y * scale;
    let angle = -(marker.rotation_degrees - terrain_rotation_degrees).to_radians();
    let contours = EmbossedLabel {
        text,
        font: label_style.label_font,
        origin_x,
        baseline_y,
        scale,
    }
    .contours()?
    .into_iter()
    .map(|contour| {
        contour
            .into_iter()
            .map(|point| rotate_about(point, center, angle))
            .collect()
    })
    .collect::<Vec<Vec<[f32; 2]>>>();
    let padding = label_style.plaque_padding_mm;
    let half_width = text_width * 0.5 + padding;
    let half_height = marker.label_height_mm * 0.5 + padding;
    let plaque = polygon_from_points(
        &[
            [-half_width, -half_height],
            [half_width, -half_height],
            [half_width, half_height],
            [-half_width, half_height],
        ]
        .map(|point| {
            let point = [point[0] + center[0], point[1] + center[1]];
            rotate_about(point, center, angle)
        }),
    );
    Ok(PreparedLabel {
        text: contours_to_multipolygon(contours),
        plaque,
    })
}

fn rotate_about(point: [f32; 2], center: [f32; 2], angle: f32) -> [f32; 2] {
    let offset = [point[0] - center[0], point[1] - center[1]];
    let (sin, cos) = angle.sin_cos();
    [
        center[0] + offset[0] * cos - offset[1] * sin,
        center[1] + offset[0] * sin + offset[1] * cos,
    ]
}

fn polygon_from_points(points: &[[f32; 2]]) -> Polygon<f64> {
    let mut coordinates = points
        .iter()
        .map(|point| Coord {
            x: f64::from(point[0]),
            y: f64::from(point[1]),
        })
        .collect::<Vec<_>>();
    if let Some(first) = coordinates.first().copied() {
        coordinates.push(first);
    }
    Polygon::new(LineString::new(coordinates), Vec::new())
}

fn contours_to_multipolygon(contours: Vec<Vec<[f32; 2]>>) -> MultiPolygon<f64> {
    let rings = contours
        .into_iter()
        .filter(|contour| contour.len() >= 3)
        .map(|contour| polygon_from_points(&contour))
        .filter(|polygon| polygon.unsigned_area() > MINIMUM_OVERLAY_AREA_MM2)
        .collect::<Vec<_>>();
    let points = rings
        .iter()
        .map(|ring| {
            ring.interior_point()
                .unwrap_or_else(|| Point::new(0.0, 0.0))
        })
        .collect::<Vec<_>>();
    let areas = rings.iter().map(Polygon::unsigned_area).collect::<Vec<_>>();
    let parents = rings
        .iter()
        .enumerate()
        .map(|(index, _)| {
            rings
                .iter()
                .enumerate()
                .filter(|(other, ring)| {
                    *other != index && areas[*other] > areas[index] && ring.contains(&points[index])
                })
                .min_by(|(left, _), (right, _)| areas[*left].total_cmp(&areas[*right]))
                .map(|(parent, _)| parent)
        })
        .collect::<Vec<_>>();
    let depths = (0..rings.len())
        .map(|mut index| {
            let mut depth = 0;
            while let Some(parent) = parents[index] {
                depth += 1;
                index = parent;
            }
            depth
        })
        .collect::<Vec<_>>();
    MultiPolygon(
        rings
            .iter()
            .enumerate()
            .filter(|(index, _)| depths[*index] % 2 == 0)
            .map(|(index, ring)| {
                let holes = rings
                    .iter()
                    .enumerate()
                    .filter(|(hole, _)| depths[*hole] % 2 == 1 && parents[*hole] == Some(index))
                    .map(|(_, hole)| hole.exterior().clone())
                    .collect();
                Polygon::new(ring.exterior().clone(), holes)
            })
            .collect(),
    )
}

fn plaque_top_z(
    spec: &GenerationSpec,
    height_field: Option<&HeightField>,
    height_range: Option<(f32, f32)>,
    plaque: &Polygon<f64>,
    assembled_width: f32,
    assembled_height: f32,
    plaque_thickness_mm: f32,
) -> f32 {
    let model = polygon_from_points(&[
        [0.0, 0.0],
        [assembled_width, 0.0],
        [assembled_width, assembled_height],
        [0.0, assembled_height],
    ]);
    let clipped = plaque.intersection(&model);
    let terrain_z = |point: [f32; 2]| {
        spec.base_mm
            + spec.relief_mm
                * normalized_height(
                    height_field,
                    height_range,
                    point[0] / assembled_width,
                    point[1] / assembled_height,
                    spec.center_lat,
                    spec.center_lon,
                )
    };
    let mut maximum = spec.base_mm;
    for polygon in clipped.0 {
        for coordinate in &polygon.exterior().0 {
            maximum = maximum.max(terrain_z([coordinate.x as f32, coordinate.y as f32]));
        }
        if let Some(bounds) = polygon.bounding_rect() {
            let width = bounds.width() as f32;
            let height = bounds.height() as f32;
            let step = LABEL_TERRAIN_STEP_MM
                .max(width / MAX_PLAQUE_HEIGHT_SAMPLES as f32)
                .max(height / MAX_PLAQUE_HEIGHT_SAMPLES as f32);
            let columns = (width / step).ceil() as usize;
            let rows = (height / step).ceil() as usize;
            for row in 0..=rows {
                for column in 0..=columns {
                    let point = Point::new(
                        bounds.min().x + f64::from((column as f32 * step).min(width)),
                        bounds.min().y + f64::from((row as f32 * step).min(height)),
                    );
                    if polygon.contains(&point) {
                        maximum = maximum.max(terrain_z([point.x() as f32, point.y() as f32]));
                    }
                }
            }
        }
    }
    maximum + plaque_thickness_mm
}

#[allow(clippy::too_many_arguments)]
fn append_shells(
    mesh: &mut Mesh,
    area: &MultiPolygon<f64>,
    bottom: impl Fn([f32; 2]) -> f32 + Copy,
    top: impl Fn([f32; 2]) -> f32 + Copy,
    boundary_step_mm: Option<f32>,
    material: SurfaceClass,
    context: &'static str,
) -> Result<()> {
    for polygon in area
        .0
        .iter()
        .filter(|polygon| polygon.unsigned_area() > MINIMUM_OVERLAY_AREA_MM2)
    {
        let shell: MeshBuilder = build_polygon_shell(
            polygon,
            bottom,
            top,
            boundary_step_mm,
            None,
            material,
            context,
        )?;
        mesh.append_isolated(shell);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::heightfield::HeightField;
    use crate::mesh::assert_watertight;
    use crate::piece::build_piece;
    use crate::spec::MapLabelStyle;

    fn label_spec(kind: MarkerKind, rotation_degrees: f32) -> GenerationSpec {
        let defaults = GenerationSpec::default();
        GenerationSpec {
            solid_model: true,
            samples_per_piece: 32,
            markers: vec![MapMarker {
                name: "River Bend".into(),
                latitude: defaults.center_lat,
                longitude: defaults.center_lon,
                kind,
                label_height_mm: 5.0,
                rotation_degrees,
                dot_style: None,
                flag_style: None,
                label_style: None,
            }],
            ..defaults
        }
    }

    fn rolling_height() -> HeightField {
        let size = 33;
        HeightField::new(
            size,
            size,
            (0..size * size)
                .map(|index| (index % size) as f32 + (index / size) as f32 * 0.35)
                .collect(),
            "label slope",
        )
        .unwrap()
    }

    #[test]
    fn geographic_label_rotation_enters_the_model_frame() {
        let spec = label_spec(MarkerKind::SurfaceLabel, 0.0);
        let north_up = prepare_label(&spec.markers[0], [50.0, 50.0], 0.0).unwrap();
        let quarter_turn = prepare_label(&spec.markers[0], [50.0, 50.0], 90.0).unwrap();
        let north_bounds = north_up.text.bounding_rect().unwrap();
        let rotated_bounds = quarter_turn.text.bounding_rect().unwrap();

        assert!(north_bounds.width() > north_bounds.height());
        assert!(rotated_bounds.width() < rotated_bounds.height());
    }

    fn material_bounds(mesh: &Mesh, material: SurfaceClass) -> [f32; 4] {
        mesh.triangles
            .iter()
            .zip(&mesh.materials)
            .filter(|(_, class)| **class == material)
            .flat_map(|(triangle, _)| triangle.iter().map(|index| mesh.vertices[*index as usize]))
            .fold(
                [
                    f32::INFINITY,
                    f32::INFINITY,
                    f32::NEG_INFINITY,
                    f32::NEG_INFINITY,
                ],
                |bounds, point| {
                    [
                        bounds[0].min(point[0]),
                        bounds[1].min(point[1]),
                        bounds[2].max(point[0]),
                        bounds[3].max(point[1]),
                    ]
                },
            )
    }

    #[test]
    fn surface_labels_follow_sloped_terrain_and_rotate_on_the_map() {
        let height = rolling_height();
        let horizontal = build_piece(
            &label_spec(MarkerKind::SurfaceLabel, 0.0),
            Some(&height),
            None,
            0,
            0,
        )
        .unwrap();
        let vertical = build_piece(
            &label_spec(MarkerKind::SurfaceLabel, 90.0),
            Some(&height),
            None,
            0,
            0,
        )
        .unwrap();
        assert_watertight(&horizontal);
        assert_watertight(&vertical);
        let horizontal_bounds = material_bounds(&horizontal, SurfaceClass::Marker);
        let vertical_bounds = material_bounds(&vertical, SurfaceClass::Marker);
        assert!(
            horizontal_bounds[2] - horizontal_bounds[0]
                > horizontal_bounds[3] - horizontal_bounds[1]
        );
        assert!(vertical_bounds[3] - vertical_bounds[1] > vertical_bounds[2] - vertical_bounds[0]);
    }

    #[test]
    fn raised_plaque_has_a_flat_backing_and_contrasting_text() {
        let height = rolling_height();
        let mesh = build_piece(
            &label_spec(MarkerKind::PlaqueLabel, -28.0),
            Some(&height),
            None,
            0,
            0,
        )
        .unwrap();
        assert_watertight(&mesh);
        assert!(mesh.materials.contains(&SurfaceClass::Marker.into()));
        assert!(mesh.materials.contains(&SurfaceClass::Snow.into()));
        let snow_heights = mesh
            .triangles
            .iter()
            .zip(&mesh.materials)
            .filter(|(_, material)| **material == SurfaceClass::Snow)
            .flat_map(|(triangle, _)| {
                triangle
                    .iter()
                    .map(|index| mesh.vertices[*index as usize][2])
            })
            .collect::<Vec<_>>();
        assert!(
            snow_heights
                .iter()
                .copied()
                .fold(f32::NEG_INFINITY, f32::max)
                - snow_heights.iter().copied().fold(f32::INFINITY, f32::min)
                >= 0.39
        );
    }

    #[test]
    fn each_plaque_uses_its_own_relief_padding_and_base_height() {
        let mut spec = label_spec(MarkerKind::PlaqueLabel, 0.0);
        spec.markers[0].label_style = Some(MapLabelStyle {
            label_font: crate::spec::LabelFont::AtkinsonHyperlegible,
            relief_mm: 0.9,
            plaque_padding_mm: 2.6,
            plaque_thickness_mm: 1.7,
        });
        let mesh = build_piece(&spec, None, None, 0, 0).unwrap();
        assert_watertight(&mesh);

        let marker_bounds = material_bounds(&mesh, SurfaceClass::Marker);
        assert!(marker_bounds[2] - marker_bounds[0] > 20.0);
        let uv = spec.normalized_map_point(spec.markers[0].latitude, spec.markers[0].longitude);
        let prepared = prepare_label(
            &spec.markers[0],
            [uv[0] * spec.width_mm, uv[1] * spec.height_mm()],
            spec.terrain_rotation_degrees as f32,
        )
        .unwrap();
        let expected_plaque_top = plaque_top_z(
            &spec,
            None,
            None,
            &prepared.plaque,
            spec.width_mm,
            spec.height_mm(),
            1.7,
        );
        let maximum_marker_z = mesh
            .triangles
            .iter()
            .zip(&mesh.materials)
            .filter(|(_, material)| **material == SurfaceClass::Marker)
            .flat_map(|(triangle, _)| {
                triangle
                    .iter()
                    .map(|index| mesh.vertices[*index as usize][2])
            })
            .fold(f32::NEG_INFINITY, f32::max);
        let maximum_text_z = mesh
            .triangles
            .iter()
            .zip(&mesh.materials)
            .filter(|(_, material)| **material == SurfaceClass::Snow)
            .flat_map(|(triangle, _)| {
                triangle
                    .iter()
                    .map(|index| mesh.vertices[*index as usize][2])
            })
            .fold(f32::NEG_INFINITY, f32::max);
        assert!((maximum_marker_z - expected_plaque_top).abs() < 0.01);
        assert!((maximum_text_z - (expected_plaque_top + 0.9)).abs() < 0.01);
    }

    #[test]
    fn plaques_clip_into_watertight_parts_across_a_piece_seam() {
        let defaults = GenerationSpec::default();
        let spec = GenerationSpec {
            rows: 1,
            columns: 2,
            straight_piece_sides: true,
            puzzle_tabs: false,
            samples_per_piece: 32,
            markers: vec![MapMarker {
                name: "Lake Crossing".into(),
                latitude: defaults.center_lat,
                longitude: defaults.center_lon,
                kind: MarkerKind::PlaqueLabel,
                label_height_mm: 5.0,
                rotation_degrees: 0.0,
                dot_style: None,
                flag_style: None,
                label_style: None,
            }],
            ..defaults
        };
        for column in 0..2 {
            let mesh = build_piece(&spec, None, None, 0, column).unwrap();
            assert_watertight(&mesh);
            assert!(mesh.materials.contains(&SurfaceClass::Marker.into()));
        }
    }
}
