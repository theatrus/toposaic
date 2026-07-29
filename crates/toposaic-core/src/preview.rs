use anyhow::Result;
use rayon::prelude::*;

use crate::heightfield::{HeightField, height_range_for_spec, normalized_height};
use crate::piece::scaled_building_height_mm;
use crate::spec::{GenerationSpec, SurfaceClass};
use crate::surface::SurfaceField;

pub fn build_height_preview(
    spec: &GenerationSpec,
    height_field: &HeightField,
    size: usize,
) -> Result<serde_json::Value> {
    spec.validate()?;
    Ok(build_preview(
        spec,
        Some(height_field),
        None,
        size.clamp(32, 128),
    ))
}

pub(crate) fn preview_sample_count(spec: &GenerationSpec) -> usize {
    (spec.rows.max(spec.columns) * spec.effective_samples_per_piece() + 1).clamp(96, 384) as usize
}

pub(crate) fn build_preview(
    spec: &GenerationSpec,
    height_field: Option<&HeightField>,
    surface_field: Option<&SurfaceField>,
    size: usize,
) -> serde_json::Value {
    let range = height_range_for_spec(spec, height_field);
    let samples = (0..size * size)
        .into_par_iter()
        .map(|index| {
            let x = index % size;
            let y = index / size;
            let u = x as f32 / (size - 1) as f32;
            let v = y as f32 / (size - 1) as f32;
            let surface_sample = surface_field.map(|field| field.sample(u, v));
            let terrain =
                normalized_height(height_field, range, u, v, spec.center_lat, spec.center_lon);
            let building = scaled_building_height_mm(
                spec,
                surface_sample
                    .map(|sample| sample.building_height_m)
                    .unwrap_or(0.0),
            ) / spec.relief_mm.max(f32::EPSILON);
            let road = surface_sample
                .filter(|sample| {
                    // Railways, aerialways, and ferries can all paint as
                    // Road-class samples under a merged style, so they must
                    // raise the preview even when the road layer is off.
                    (spec.color_output.enabled
                        && (spec.color_output.roads_enabled
                            || spec.color_output.rail_enabled
                            || spec.color_output.aerial_enabled
                            || spec.color_output.ferry_enabled
                            || spec.color_output.aviation.aviation_enabled)
                        && sample.class == SurfaceClass::Road)
                        || (spec.color_output.enabled
                            && spec.color_output.roads_enabled
                            && sample.class == SurfaceClass::RouteTrail)
                        || (spec.uses_trails() && sample.class == SurfaceClass::Trail)
                        || (spec.uses_rail_or_aerial() && sample.class == SurfaceClass::Rail)
                        || (spec.uses_aerial() && sample.class == SurfaceClass::Aerial)
                        || (spec.uses_ferry() && sample.class == SurfaceClass::Ferry)
                        || (spec.uses_aviation() && sample.class == SurfaceClass::Aviation)
                })
                .map(|_| spec.color_output.road_height_mm)
                .unwrap_or(0.0)
                / spec.relief_mm.max(f32::EPSILON);
            (
                terrain + building + road,
                surface_sample.map(|sample| sample.class.material_index()),
            )
        })
        .collect::<Vec<_>>();
    let mut heights = Vec::with_capacity(samples.len());
    let mut surface_classes = surface_field.map(|_| Vec::with_capacity(samples.len()));
    for (height, surface_class) in samples {
        heights.push(height);
        if let (Some(class), Some(classes)) = (surface_class, surface_classes.as_mut()) {
            classes.push(class);
        }
    }
    let mut preview = serde_json::json!({
        "width": size,
        "height": size,
        "values": heights,
        "rows": spec.rows,
        "columns": spec.columns,
        "solid_model": spec.solid_model,
    });
    if let Some(field) = height_field {
        let (minimum, maximum) = field.elevation_bounds();
        preview["minimum_elevation_m"] = serde_json::json!(minimum);
        preview["maximum_elevation_m"] = serde_json::json!(maximum);
        preview["height_frame_compatible"] = serde_json::json!(
            spec.elevation_datum_m
                .map(|datum| minimum + 0.01 >= datum)
                .unwrap_or(true)
        );
    }
    if let (Some(field), Some(classes)) = (surface_field, surface_classes) {
        let coverage = field.coverage();
        preview["surface_classes"] = serde_json::json!(classes);
        preview["surface_palette"] = serde_json::json!({
            "rock": spec.color_output.rock_color,
            "forest": spec.color_output.forest_color,
            "snow": spec.color_output.snow_color,
            "water": spec.color_output.water_color,
            "road": spec.color_output.road_color,
            "building": spec.color_output.building_color,
        });
        preview["surface_coverage"] = serde_json::json!({
            "rock": coverage[0],
            "forest": coverage[1],
            "snow": coverage[2],
            "water": coverage[3],
            "road": coverage[4],
            "building": coverage[5],
        });
        // The trail keys appear only when the spec carries trails, so
        // preview.json stays byte-identical for every existing project.
        if spec.uses_trails() {
            preview["surface_palette"]["trail"] = serde_json::json!(spec.color_output.trail_color);
            preview["surface_coverage"]["trail"] =
                serde_json::json!(coverage[SurfaceClass::Trail.material_index() as usize]);
        }
        if field.contained_classes()[SurfaceClass::RouteTrail.material_index() as usize] {
            preview["surface_palette"]["route_trail"] = serde_json::json!(
                spec.color_output
                    .route_trail_color
                    .as_deref()
                    .unwrap_or(&spec.color_output.road_color)
            );
            preview["surface_coverage"]["route_trail"] =
                serde_json::json!(coverage[SurfaceClass::RouteTrail.material_index() as usize]);
        }
        if spec.uses_colored_markers() {
            preview["surface_palette"]["marker"] = serde_json::json!(spec.marker_settings.color);
            preview["surface_coverage"]["marker"] =
                serde_json::json!(coverage[SurfaceClass::Marker.material_index() as usize]);
        }
        // Railways and aerialways only get their own preview entries under
        // the `separate` style; otherwise they paint in a class already
        // counted here, so preview.json keeps its existing shape.
        //
        // These indices are the RAW `material_index`, not the archive's
        // dense filament slot. The preview names a feature class; the 3MF
        // slot names a spool. Keeping them apart means the preview does not
        // shift under the frontend when a color layer is switched on.
        if spec.uses_separate_rail() {
            preview["surface_palette"]["rail"] = serde_json::json!(spec.color_output.rail_color);
            preview["surface_coverage"]["rail"] =
                serde_json::json!(coverage[SurfaceClass::Rail.material_index() as usize]);
        }
        if spec.uses_separate_aerial() {
            preview["surface_palette"]["aerialway"] =
                serde_json::json!(spec.color_output.aerial_color);
            preview["surface_coverage"]["aerialway"] =
                serde_json::json!(coverage[SurfaceClass::Aerial.material_index() as usize]);
        }
        if spec.uses_separate_ferry() {
            preview["surface_palette"]["ferry"] = serde_json::json!(spec.color_output.ferry_color);
            preview["surface_coverage"]["ferry"] =
                serde_json::json!(coverage[SurfaceClass::Ferry.material_index() as usize]);
        }
        if spec.uses_separate_aviation() {
            preview["surface_palette"]["aviation"] =
                serde_json::json!(spec.color_output.aviation.aviation_color);
            preview["surface_coverage"]["aviation"] =
                serde_json::json!(coverage[SurfaceClass::Aviation.material_index() as usize]);
        }
        preview["surface_source"] = serde_json::json!(field.source);
    }
    preview
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::spec::BuildingSpec;

    /// The 3D preview has to show the pavement the generated model will
    /// carry — same class, same color, and standing proud like a road —
    /// or the preview is telling a different story from the print.
    #[test]
    fn the_preview_shows_airport_pavement_the_model_will_print() {
        let mut spec = GenerationSpec {
            width_mm: 60.0,
            color_output: crate::spec::ColorOutputSpec {
                enabled: true,
                ..crate::spec::ColorOutputSpec::default()
            },
            ..GenerationSpec::default()
        };
        spec.color_output.aviation.aviation_enabled = true;
        spec.color_output.aviation.aviation_color = "#334455".into();

        let mut field = SurfaceField::new(9, 9, vec![SurfaceClass::Rock; 81], "airport").unwrap();
        field.paint_polyline(&[[0.0, 0.5], [1.0, 0.5]], 60.0, 4.0, SurfaceClass::Aviation);

        let preview = build_preview(&spec, None, Some(&field), 16);
        assert_eq!(preview["surface_palette"]["aviation"], "#334455");
        assert!(
            preview["surface_coverage"]["aviation"].as_f64().unwrap() > 0.0,
            "the legend needs coverage to show the layer"
        );

        // The class indices the preview reports are SurfaceClass::ALL
        // positions, so the runway must read as the aviation class.
        let classes = preview["surface_classes"].as_array().unwrap();
        let aviation_index = SurfaceClass::Aviation.material_index() as u64;
        assert!(
            classes
                .iter()
                .any(|class| class.as_u64() == Some(aviation_index)),
            "no pavement in the preview classes"
        );

        // And it stands above the terrain, the way roads do.
        let values = preview["values"].as_array().unwrap();
        let raised = classes
            .iter()
            .zip(values)
            .filter(|(class, _)| class.as_u64() == Some(aviation_index))
            .map(|(_, value)| value.as_f64().unwrap())
            .fold(f64::NEG_INFINITY, f64::max);
        let flat = classes
            .iter()
            .zip(values)
            .filter(|(class, _)| class.as_u64() != Some(aviation_index))
            .map(|(_, value)| value.as_f64().unwrap())
            .fold(f64::NEG_INFINITY, f64::max);
        assert!(
            raised > flat,
            "pavement should stand proud in the preview: {raised} vs {flat}"
        );
    }

    /// Following the roads spends no filament slot, so the preview must not
    /// offer an airport entry to a legend that will have no such color.
    #[test]
    fn the_preview_offers_no_airport_color_when_it_follows_the_roads() {
        let mut spec = GenerationSpec {
            color_output: crate::spec::ColorOutputSpec {
                enabled: true,
                ..crate::spec::ColorOutputSpec::default()
            },
            ..GenerationSpec::default()
        };
        spec.color_output.aviation.aviation_enabled = true;
        spec.color_output.aviation.aviation_style = crate::spec::AviationStyle::FollowRoads;

        let field = SurfaceField::new(9, 9, vec![SurfaceClass::Rock; 81], "airport").unwrap();
        let preview = build_preview(&spec, None, Some(&field), 16);
        assert!(preview["surface_palette"]["aviation"].is_null());
    }

    #[test]
    fn height_preview_reports_elevation_bounds_and_frame_fit() {
        let spec = GenerationSpec {
            elevation_datum_m: Some(100.0),
            elevation_m_per_mm: Some(10.0),
            ..GenerationSpec::default()
        };
        let height = HeightField::new(2, 2, vec![90.0, 110.0, 120.0, 130.0], "test").unwrap();
        let preview = build_height_preview(&spec, &height, 32).unwrap();

        assert_eq!(preview["minimum_elevation_m"], 90.0);
        assert_eq!(preview["maximum_elevation_m"], 130.0);
        assert_eq!(preview["height_frame_compatible"], false);
    }

    #[test]
    fn assembled_preview_keeps_more_overlay_detail() {
        let spec = GenerationSpec {
            rows: 4,
            columns: 4,
            buildings: BuildingSpec {
                enabled: true,
                ..BuildingSpec::default()
            },
            ..GenerationSpec::default()
        };
        assert_eq!(preview_sample_count(&spec), 384);
    }

    #[test]
    fn fast_height_preview_uses_real_samples_and_caps_its_size() {
        let field =
            HeightField::new(2, 2, vec![100.0, 200.0, 300.0, 400.0], "preview-test").unwrap();
        let preview = build_height_preview(&GenerationSpec::default(), &field, 512).unwrap();
        let values = preview["values"].as_array().unwrap();

        assert_eq!(preview["width"], 128);
        assert_eq!(preview["height"], 128);
        assert_eq!(values.len(), 128 * 128);
        assert_eq!(
            values.first().and_then(serde_json::Value::as_f64),
            Some(0.0)
        );
        assert_eq!(values.last().and_then(serde_json::Value::as_f64), Some(1.0));
        assert!(preview.get("surface_classes").is_none());
    }

    #[test]
    fn parallel_preview_keeps_stable_sample_order() {
        let spec = GenerationSpec::default();
        let height =
            HeightField::new(3, 3, (0..9).map(|value| value as f32).collect(), "height").unwrap();
        let surface = SurfaceField::new(
            3,
            3,
            [
                SurfaceClass::Rock,
                SurfaceClass::Forest,
                SurfaceClass::Snow,
                SurfaceClass::Water,
                SurfaceClass::Road,
                SurfaceClass::Building,
                SurfaceClass::Snow,
                SurfaceClass::Forest,
                SurfaceClass::Rock,
            ]
            .to_vec(),
            "surface",
        )
        .unwrap();
        let single_threaded = rayon::ThreadPoolBuilder::new()
            .num_threads(1)
            .build()
            .unwrap()
            .install(|| build_preview(&spec, Some(&height), Some(&surface), 64));
        let parallel = rayon::ThreadPoolBuilder::new()
            .num_threads(4)
            .build()
            .unwrap()
            .install(|| build_preview(&spec, Some(&height), Some(&surface), 64));

        assert_eq!(single_threaded, parallel);
    }
}
