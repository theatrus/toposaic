use anyhow::Result;

use crate::mesh::{Mesh, MeshBuilder, weld_export_mesh};
use crate::planar_mesh::{add_horizontal_polygons, polygon_from_outline};
use crate::spec::{MarkerSpec, SurfaceClass};

/// A flat-print flag blank sized to the configured socket. The stem and
/// banner form one watertight profile, so users can add text or symbols in a
/// slicer or CAD tool without first repairing the template.
pub(crate) fn build_flag_template(settings: &MarkerSpec) -> Result<Mesh> {
    let post_diameter = settings.hole_diameter_mm - settings.flag_clearance_mm;
    let thickness = 0.8_f32.min(post_diameter * 0.7);
    let stem_width = (post_diameter * post_diameter - thickness * thickness).sqrt();
    let half_stem = stem_width * 0.5;
    let outline = [
        [-half_stem, 0.0],
        [half_stem, 0.0],
        [half_stem, 10.0],
        [half_stem + 12.0, 10.0],
        [half_stem + 12.0, 18.0],
        [-half_stem, 18.0],
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
    let mut mesh = builder.finish("Marker Flag Template");
    weld_export_mesh(&mut mesh);
    Ok(mesh)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mesh::assert_watertight;

    #[test]
    fn flag_post_uses_the_requested_round_socket_clearance() {
        let settings = MarkerSpec::default();
        let mesh = build_flag_template(&settings).unwrap();
        assert_watertight(&mesh);
        let stem = mesh
            .vertices
            .iter()
            .filter(|vertex| vertex[1] < 10.0)
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
}
