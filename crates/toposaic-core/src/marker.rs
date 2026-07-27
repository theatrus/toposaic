use anyhow::Result;

use crate::mesh::{Mesh, MeshBuilder, weld_export_mesh};
use crate::planar_mesh::{add_horizontal_polygons, polygon_from_outline};
use crate::spec::{MarkerSpec, SurfaceClass};
use crate::text::{EmbossedLabel, embossing_fonts, text_metrics};

const FLAG_POST_LENGTH_MM: f32 = 10.0;
const FLAG_LABEL_MARGIN_MM: f32 = 1.0;

/// A flat-print flag sized to the configured socket. The stem and banner form
/// one watertight profile. A named flag adds fitted vector text to its top;
/// a blank stays ready for symbols or later edits in a slicer or CAD tool.
pub(crate) fn build_flag_template(settings: &MarkerSpec, label: Option<&str>) -> Result<Mesh> {
    let post_diameter = settings.hole_diameter_mm - settings.flag_clearance_mm;
    let thickness = 0.8_f32.min(post_diameter * 0.7);
    let stem_width = (post_diameter * post_diameter - thickness * thickness).sqrt();
    let half_stem = stem_width * 0.5;
    let banner_right = half_stem + settings.flag_width_mm;
    let banner_top = FLAG_POST_LENGTH_MM + settings.flag_height_mm;
    let outline = [
        [-half_stem, 0.0],
        [half_stem, 0.0],
        [half_stem, FLAG_POST_LENGTH_MM],
        [banner_right, FLAG_POST_LENGTH_MM],
        [banner_right, banner_top],
        [-half_stem, banner_top],
    ];
    let polygon = polygon_from_outline(&outline);
    let mut builder = MeshBuilder::default();
    add_horizontal_polygons(
        &mut builder,
        std::slice::from_ref(&polygon),
        0.0,
        SurfaceClass::Marker,
        true,
    )?;
    add_horizontal_polygons(
        &mut builder,
        &[polygon],
        thickness,
        SurfaceClass::Marker,
        false,
    )?;
    for (start, end) in outline.iter().zip(outline.iter().cycle().skip(1)) {
        builder.quad(
            [start[0], start[1], 0.0],
            [end[0], end[1], 0.0],
            [end[0], end[1], thickness],
            [start[0], start[1], thickness],
            SurfaceClass::Marker,
        );
    }
    if let Some(label) = label {
        add_flag_label(&mut builder, settings, label, half_stem, thickness)?;
    }
    let mut mesh = builder.finish("Marker Flag Template");
    weld_export_mesh(&mut mesh);
    Ok(mesh)
}

fn add_flag_label(
    builder: &mut MeshBuilder,
    settings: &MarkerSpec,
    label: &str,
    half_stem: f32,
    surface_z: f32,
) -> Result<()> {
    let text = label.split_whitespace().collect::<Vec<_>>().join(" ");
    let fonts = embossing_fonts(settings.label_font)?;
    let metrics = text_metrics(&fonts, &text)?;
    let available_width = (settings.flag_width_mm - FLAG_LABEL_MARGIN_MM * 2.0).max(1.0);
    let available_height = (settings.flag_height_mm - FLAG_LABEL_MARGIN_MM * 2.0).max(1.0);
    let scale = (settings.flag_label_height_mm / metrics.height)
        .min(available_width / metrics.width)
        .min(available_height / metrics.height);
    let text_width = metrics.width * scale;
    let text_height = metrics.height * scale;
    let origin_x = half_stem + FLAG_LABEL_MARGIN_MM + (available_width - text_width) * 0.5
        - metrics.minimum_x * scale;
    let baseline_y =
        FLAG_POST_LENGTH_MM + FLAG_LABEL_MARGIN_MM + (available_height - text_height) * 0.5
            - metrics.minimum_y * scale;
    EmbossedLabel {
        text,
        font: settings.label_font,
        origin_x,
        baseline_y,
        scale,
    }
    .add_embossed_shapes(builder, surface_z)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mesh::assert_watertight;

    #[test]
    fn flag_post_uses_the_requested_round_socket_clearance() {
        let settings = MarkerSpec::default();
        let mesh = build_flag_template(&settings, None).unwrap();
        assert_watertight(&mesh);
        let stem = mesh
            .vertices
            .iter()
            .filter(|vertex| vertex[1] < FLAG_POST_LENGTH_MM)
            .collect::<Vec<_>>();
        let minimum_x = stem
            .iter()
            .map(|point| point[0])
            .fold(f32::INFINITY, f32::min);
        let maximum_x = stem
            .iter()
            .map(|point| point[0])
            .fold(f32::NEG_INFINITY, f32::max);
        let minimum_z = stem
            .iter()
            .map(|point| point[2])
            .fold(f32::INFINITY, f32::min);
        let maximum_z = stem
            .iter()
            .map(|point| point[2])
            .fold(f32::NEG_INFINITY, f32::max);
        let post_diagonal = (maximum_x - minimum_x).hypot(maximum_z - minimum_z);
        assert!(
            (post_diagonal - (settings.hole_diameter_mm - settings.flag_clearance_mm)).abs()
                < 0.001
        );
    }

    #[test]
    fn named_flags_use_the_selected_font_and_stay_watertight() {
        for font in [
            crate::spec::LabelFont::AtkinsonHyperlegible,
            crate::spec::LabelFont::NotoSans,
            crate::spec::LabelFont::B612Mono,
        ] {
            let settings = MarkerSpec {
                label_font: font,
                flag_width_mm: 36.0,
                ..MarkerSpec::default()
            };
            let mesh = build_flag_template(&settings, Some("富士山 Mount Fuji")).unwrap();
            assert_watertight(&mesh);
            assert!(
                mesh.vertices.iter().any(|point| point[2] > 0.8),
                "the printed name must rise above the flag face"
            );
            let maximum_x = mesh
                .vertices
                .iter()
                .map(|point| point[0])
                .fold(f32::NEG_INFINITY, f32::max);
            assert!(maximum_x > 36.0);
        }
    }
}
