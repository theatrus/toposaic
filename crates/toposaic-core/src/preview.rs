use anyhow::{Context, Result, bail};
use rayon::prelude::*;
use serde::{Deserialize, Serialize};

use crate::heightfield::{HeightField, height_range_for_spec, normalized_height};
use crate::mesh::Mesh;
use crate::piece::{
    PieceGeometryTiming, build_piece_with_height_range_timed, elapsed_us,
    printable_piece_positions, resolved_piece_samples, scaled_building_height_mm,
    summarize_geometry_timing,
};
use crate::spec::{GenerationSpec, PrintMaterial, SurfaceClass};
use crate::surface::SurfaceField;
use crate::tray::{build_preview_tray_segments, terrain_origin_in_tray, tray_segment_origins};

const MODEL_PREVIEW_TERRAIN_SAMPLES_PER_PIECE: u32 = 16;
const MODEL_PREVIEW_OVERLAY_SAMPLES_PER_PIECE: u32 = 32;
const DETAILED_MODEL_PREVIEW_TERRAIN_SAMPLES_PER_PIECE: u32 = 32;
const DETAILED_MODEL_PREVIEW_OVERLAY_SAMPLES_PER_PIECE: u32 = 64;
const HIGH_MODEL_PREVIEW_TERRAIN_SAMPLES_PER_PIECE: u32 = 48;
const HIGH_MODEL_PREVIEW_OVERLAY_SAMPLES_PER_PIECE: u32 = 96;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PreviewDetail {
    Fast,
    Detailed,
    High,
}

impl PreviewDetail {
    pub const fn sample_grid(self) -> usize {
        match self {
            Self::Fast => 128,
            Self::Detailed => 192,
            Self::High => 256,
        }
    }
}

#[derive(Serialize)]
struct ModelPreviewMesh {
    kind: &'static str,
    name: String,
    vertices: Vec<[f32; 3]>,
    triangles: Vec<[u32; 3]>,
    materials: Vec<u32>,
}

struct ModelGeometryBuild {
    meshes: Vec<ModelPreviewMesh>,
    bounds: [f32; 6],
    terrain_bounds: [f32; 4],
    piece_timings: Vec<PieceGeometryTiming>,
    piece_phase_us: u64,
    tray_us: u64,
}

impl ModelPreviewMesh {
    fn from_mesh(mesh: Mesh, kind: &'static str, offset: [f32; 3]) -> Self {
        Self {
            kind,
            name: mesh.name,
            vertices: mesh
                .vertices
                .into_iter()
                .map(|point| {
                    [
                        point[0] + offset[0],
                        point[1] + offset[1],
                        point[2] + offset[2],
                    ]
                })
                .collect(),
            triangles: mesh.triangles,
            materials: mesh
                .materials
                .into_iter()
                .map(PrintMaterial::material_index)
                .collect(),
        }
    }
}

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

/// Keeps every model choice while using the chosen bounded sample budget for
/// the live background pass. Data fetching and mesh construction must use the
/// same draft spec or thin overlays can vanish between the two grids.
pub fn model_preview_spec(spec: &GenerationSpec, detail: PreviewDetail) -> GenerationSpec {
    let mut draft = spec.clone();
    match detail {
        PreviewDetail::Fast => {
            draft.samples_per_piece = MODEL_PREVIEW_TERRAIN_SAMPLES_PER_PIECE;
            draft.overlay_samples_per_piece = MODEL_PREVIEW_OVERLAY_SAMPLES_PER_PIECE;
            draft.mesh_samples_across = None;
            draft.overlay_samples_across = None;
            draft.fine_dem_detail = false;
        }
        PreviewDetail::Detailed => {
            draft.samples_per_piece = DETAILED_MODEL_PREVIEW_TERRAIN_SAMPLES_PER_PIECE;
            draft.overlay_samples_per_piece = DETAILED_MODEL_PREVIEW_OVERLAY_SAMPLES_PER_PIECE;
            draft.mesh_samples_across = None;
            draft.overlay_samples_across = None;
            draft.fine_dem_detail = false;
        }
        PreviewDetail::High => {
            draft.samples_per_piece = HIGH_MODEL_PREVIEW_TERRAIN_SAMPLES_PER_PIECE;
            draft.overlay_samples_per_piece = HIGH_MODEL_PREVIEW_OVERLAY_SAMPLES_PER_PIECE;
            draft.mesh_samples_across = None;
            draft.overlay_samples_across = None;
            draft.fine_dem_detail = false;
        }
    }
    draft
}

/// Builds a draft of the printable scene with the same mesh code as export.
///
/// Height values remain in the response as a cheap browser fallback. The
/// mesh list adds the outcomes a height map cannot show: vertical building
/// walls, overlay shells and bridges, labels, mount pockets, puzzle bodies,
/// retention features, and the fitted or segmented display base.
pub fn build_model_preview(
    spec: &GenerationSpec,
    height_field: &HeightField,
    surface_field: Option<&SurfaceField>,
    size: usize,
    detail: PreviewDetail,
) -> Result<serde_json::Value> {
    build_model_preview_cancellable(spec, height_field, surface_field, size, detail, &|| false)
}

pub fn build_model_preview_cancellable(
    spec: &GenerationSpec,
    height_field: &HeightField,
    surface_field: Option<&SurfaceField>,
    size: usize,
    detail: PreviewDetail,
    is_cancelled: &(dyn Fn() -> bool + Sync),
) -> Result<serde_json::Value> {
    spec.validate()?;
    let draft = model_preview_spec(spec, detail);
    ensure_model_preview_active(is_cancelled)?;

    let mut preview = build_preview(
        &draft,
        Some(height_field),
        surface_field,
        size.clamp(32, detail.sample_grid()),
    );
    let piece_samples = resolved_piece_samples(&draft, Some(height_field));
    let assembled_samples = if draft.solid_model {
        piece_samples
    } else {
        piece_samples * draft.rows.max(draft.columns) as usize
    };
    preview["model_mesh_samples_across"] = serde_json::json!(assembled_samples);
    let model_geometry = (|| -> Result<ModelGeometryBuild> {
        ensure_model_preview_active(is_cancelled)?;
        let height_range = height_range_for_spec(&draft, Some(height_field));
        let terrain_in_tray = if draft.tray.enabled {
            terrain_origin_in_tray(&draft)?
        } else {
            [0.0, 0.0]
        };
        let terrain_z = if draft.tray.enabled {
            draft.tray.floor_mm
        } else {
            0.0
        };
        let piece_width = draft.width_mm / draft.columns.max(1) as f32;
        let piece_height = draft.height_mm() / draft.rows.max(1) as f32;
        let positions = printable_piece_positions(&draft)?;
        let piece_phase_started = std::time::Instant::now();
        let built = positions
            .into_par_iter()
            .map(|(row, column)| {
                ensure_model_preview_active(is_cancelled)?;
                let (mesh, timing) = build_piece_with_height_range_timed(
                    &draft,
                    Some(height_field),
                    height_range,
                    surface_field,
                    row,
                    column,
                )
                .with_context(|| format!("build preview piece {}, {}", row + 1, column + 1))?;
                ensure_model_preview_active(is_cancelled)?;
                let piece_offset = if draft.solid_model {
                    [0.0, 0.0]
                } else {
                    [column as f32 * piece_width, row as f32 * piece_height]
                };
                Ok((
                    ModelPreviewMesh::from_mesh(
                        mesh,
                        "terrain",
                        [
                            terrain_in_tray[0] + piece_offset[0],
                            terrain_in_tray[1] + piece_offset[1],
                            terrain_z,
                        ],
                    ),
                    timing,
                ))
            })
            .collect::<Result<Vec<_>>>()?;
        let piece_phase_us = elapsed_us(piece_phase_started);
        let (mut meshes, piece_timings): (Vec<_>, Vec<_>) = built.into_iter().unzip();

        let tray_started = std::time::Instant::now();
        if draft.tray.enabled {
            ensure_model_preview_active(is_cancelled)?;
            let tray_origins = tray_segment_origins(&draft);
            let tray_meshes = build_preview_tray_segments(&draft, Some(height_field))?;
            for (mesh, origin) in tray_meshes.into_iter().zip(tray_origins) {
                meshes.push(ModelPreviewMesh::from_mesh(
                    mesh,
                    "tray",
                    [origin[0], origin[1], 0.0],
                ));
            }
        }
        let tray_us = elapsed_us(tray_started);

        let mut bounds = [
            f32::INFINITY,
            f32::INFINITY,
            f32::INFINITY,
            f32::NEG_INFINITY,
            f32::NEG_INFINITY,
            f32::NEG_INFINITY,
        ];
        for point in meshes.iter().flat_map(|mesh| mesh.vertices.iter()) {
            bounds[0] = bounds[0].min(point[0]);
            bounds[1] = bounds[1].min(point[1]);
            bounds[2] = bounds[2].min(point[2]);
            bounds[3] = bounds[3].max(point[0]);
            bounds[4] = bounds[4].max(point[1]);
            bounds[5] = bounds[5].max(point[2]);
        }
        let terrain_bounds = [
            terrain_in_tray[0],
            terrain_in_tray[1],
            terrain_in_tray[0] + draft.width_mm,
            terrain_in_tray[1] + draft.height_mm(),
        ];
        Ok(ModelGeometryBuild {
            meshes,
            bounds,
            terrain_bounds,
            piece_timings,
            piece_phase_us,
            tray_us,
        })
    })();
    match model_geometry {
        Ok(ModelGeometryBuild {
            meshes,
            bounds,
            terrain_bounds,
            piece_timings,
            piece_phase_us,
            tray_us,
        }) => {
            let serialization_started = std::time::Instant::now();
            preview["model_meshes"] = serde_json::to_value(meshes)?;
            let serialization_us = elapsed_us(serialization_started);
            preview["model_geometry_timing"] = serde_json::to_value(summarize_geometry_timing(
                &piece_timings,
                surface_field,
                piece_phase_us,
                tray_us,
                serialization_us,
            ))?;
            preview["model_bounds_mm"] = serde_json::json!(bounds);
            preview["model_terrain_bounds_mm"] = serde_json::json!(terrain_bounds);
            preview["model_preview_detail"] = serde_json::json!("draft export geometry");
        }
        Err(error) => {
            preview["model_preview_error"] = serde_json::json!(error.to_string());
        }
    }
    Ok(preview)
}

fn ensure_model_preview_active(is_cancelled: &(dyn Fn() -> bool + Sync)) -> Result<()> {
    if is_cancelled() {
        bail!("preview superseded by newer settings");
    }
    Ok(())
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
                // The same material the mesh will paint here, so the preview
                // shows the discovered ground colors rather than the mapped
                // class colors the print will not use. Built from the sample
                // already taken, overlays and all — re-sampling the terrain
                // here would drop every road and runway from the preview.
                surface_sample.zip(surface_field).map(|(sample, field)| {
                    field
                        .print_material_for(sample.class, u, v)
                        .material_index()
                }),
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
        // Indices past the fixed classes name a discovered ground color, in
        // this order. Absent when the ground colors are the mapped ones.
        if let Some(palette) = field.ground_palette() {
            preview["ground_palette"] = serde_json::json!(
                palette
                    .entries
                    .iter()
                    .map(|entry| entry.color.clone())
                    .collect::<Vec<_>>()
            );
        }
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
    use crate::spec::{BuildingSpec, MapMarker, MarkerKind};

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
    fn live_model_preview_uses_export_meshes_for_buildings() {
        let mut spec = GenerationSpec {
            width_mm: 60.0,
            solid_model: true,
            buildings: BuildingSpec {
                enabled: true,
                ..BuildingSpec::default()
            },
            color_output: crate::spec::ColorOutputSpec {
                enabled: true,
                roads_enabled: true,
                ..crate::spec::ColorOutputSpec::default()
            },
            ..GenerationSpec::default()
        };
        spec.color_output.rail_enabled = false;
        spec.color_output.aerial_enabled = false;
        spec.color_output.ferry_enabled = false;
        let height = HeightField::new(33, 33, vec![0.0; 33 * 33], "height").unwrap();
        let mut surface =
            SurfaceField::new(33, 33, vec![SurfaceClass::Rock; 33 * 33], "surface").unwrap();
        surface.paint_building(&[[0.3, 0.3], [0.7, 0.3], [0.7, 0.7], [0.3, 0.7]], 12.0);
        surface.paint_polyline(
            &[[0.1, 0.5], [0.9, 0.5]],
            spec.width_mm,
            0.7,
            SurfaceClass::Road,
        );

        let preview =
            build_model_preview(&spec, &height, Some(&surface), 64, PreviewDetail::Fast).unwrap();
        let meshes = preview["model_meshes"].as_array().unwrap();
        assert_eq!(meshes.len(), 1);
        assert_eq!(meshes[0]["kind"], "terrain");
        assert!(
            meshes[0]["materials"]
                .as_array()
                .unwrap()
                .iter()
                .any(|material| {
                    material.as_u64() == Some(u64::from(SurfaceClass::Building.material_index()))
                }),
            "the draft must carry the building shell, not a raised raster cell"
        );
        let timing = &preview["model_geometry_timing"];
        assert_eq!(timing["piece_count"], 1);
        assert_eq!(timing["source_building_count"], 1);
        assert_eq!(timing["source_line_count"], 1);
        assert_eq!(timing["building_candidate_count"], 1);
        assert_eq!(timing["clipped_building_count"], 1);
        assert_eq!(timing["line_candidate_count"], 1);
        assert_eq!(timing["ribbon_clip_count"], 1);
        assert_eq!(timing["slowest_pieces"][0]["row"], 1);
        assert_eq!(timing["slowest_pieces"][0]["column"], 1);
        assert!(timing["piece_phase_wall_ms"].is_number());
        assert!(timing["road_building_cutback_work_ms"].is_number());
    }

    #[test]
    fn live_model_preview_keeps_highlighted_buildings_in_the_marker_material() {
        let spec = GenerationSpec {
            width_mm: 60.0,
            solid_model: true,
            buildings: BuildingSpec {
                enabled: true,
                ..BuildingSpec::default()
            },
            markers: vec![MapMarker {
                name: "Terminal".into(),
                latitude: 0.0,
                longitude: 0.0,
                kind: MarkerKind::Building,
                label_height_mm: 4.0,
                rotation_degrees: 0.0,
                dot_style: None,
                flag_style: None,
                label_style: None,
            }],
            ..GenerationSpec::default()
        };
        let height = HeightField::new(33, 33, vec![0.0; 33 * 33], "height").unwrap();
        let mut surface =
            SurfaceField::new(33, 33, vec![SurfaceClass::Rock; 33 * 33], "surface").unwrap();
        surface.paint_building_with_class(
            &[[0.3, 0.3], [0.7, 0.3], [0.7, 0.7], [0.3, 0.7]],
            12.0,
            SurfaceClass::Marker,
        );

        let preview =
            build_model_preview(&spec, &height, Some(&surface), 64, PreviewDetail::Fast).unwrap();
        let materials = preview["model_meshes"][0]["materials"].as_array().unwrap();
        assert!(materials.iter().any(|material| {
            material.as_u64() == Some(u64::from(SurfaceClass::Marker.material_index()))
        }));
        assert!(
            preview["surface_classes"]
                .as_array()
                .unwrap()
                .iter()
                .any(|class| {
                    class.as_u64() == Some(u64::from(SurfaceClass::Marker.material_index()))
                })
        );
    }

    #[test]
    fn live_model_preview_assembles_the_enabled_tray() {
        let mut spec = GenerationSpec {
            width_mm: 60.0,
            solid_model: true,
            ..GenerationSpec::default()
        };
        spec.tray.enabled = true;
        spec.tray.label_enabled = false;
        spec.tray.contours_enabled = false;
        let height = HeightField::new(33, 33, vec![0.0; 33 * 33], "height").unwrap();

        let preview = build_model_preview(&spec, &height, None, 64, PreviewDetail::Fast).unwrap();
        let meshes = preview["model_meshes"].as_array().unwrap();
        assert!(meshes.iter().any(|mesh| mesh["kind"] == "terrain"));
        assert!(meshes.iter().any(|mesh| mesh["kind"] == "tray"));
        let bounds = preview["model_bounds_mm"].as_array().unwrap();
        assert!(bounds[3].as_f64().unwrap() > f64::from(spec.width_mm));
        assert!(bounds[4].as_f64().unwrap() > f64::from(spec.height_mm()));
        let terrain_bounds = preview["model_terrain_bounds_mm"].as_array().unwrap();
        let terrain_width =
            terrain_bounds[2].as_f64().unwrap() - terrain_bounds[0].as_f64().unwrap();
        let terrain_height =
            terrain_bounds[3].as_f64().unwrap() - terrain_bounds[1].as_f64().unwrap();
        assert!((terrain_width - f64::from(spec.width_mm)).abs() < 1e-4);
        assert!((terrain_height - f64::from(spec.height_mm())).abs() < 1e-4);
    }

    #[test]
    fn model_preview_spec_keeps_features_and_lowers_only_detail() {
        let mut spec = GenerationSpec::default();
        spec.color_output.enabled = true;
        spec.color_output.roads_enabled = true;
        spec.buildings.enabled = true;
        spec.tray.enabled = true;
        spec.mesh_samples_across = Some(2_048);
        spec.overlay_samples_across = Some(2_048);
        spec.fine_dem_detail = true;

        let draft = model_preview_spec(&spec, PreviewDetail::Fast);
        assert!(draft.color_output.enabled);
        assert!(draft.color_output.roads_enabled);
        assert!(draft.buildings.enabled);
        assert!(draft.tray.enabled);
        assert_eq!(draft.samples_per_piece, 16);
        assert_eq!(draft.overlay_samples_per_piece, 32);
        assert_eq!(draft.mesh_samples_across, None);
        assert_eq!(draft.overlay_samples_across, None);
        assert!(!draft.fine_dem_detail);

        let detailed = model_preview_spec(&spec, PreviewDetail::Detailed);
        assert_eq!(detailed.samples_per_piece, 32);
        assert_eq!(detailed.overlay_samples_per_piece, 64);
        assert_eq!(detailed.mesh_samples_across, None);
        assert_eq!(detailed.overlay_samples_across, None);
        assert!(!detailed.fine_dem_detail);

        let high = model_preview_spec(&spec, PreviewDetail::High);
        assert_eq!(high.samples_per_piece, 48);
        assert_eq!(high.overlay_samples_per_piece, 96);
        assert_eq!(high.mesh_samples_across, None);
        assert_eq!(high.overlay_samples_across, None);
        assert!(!high.fine_dem_detail);
        assert_eq!(PreviewDetail::Fast.sample_grid(), 128);
        assert_eq!(PreviewDetail::Detailed.sample_grid(), 192);
        assert_eq!(PreviewDetail::High.sample_grid(), 256);
    }

    #[test]
    fn live_model_preview_stays_bounded_for_the_default_app_layout() {
        let mut spec = GenerationSpec {
            rows: 10,
            columns: 10,
            ..GenerationSpec::default()
        };
        spec.tray.enabled = true;
        let height = HeightField::new(129, 129, vec![0.0; 129 * 129], "height").unwrap();

        let preview = build_model_preview(&spec, &height, None, 128, PreviewDetail::Fast).unwrap();
        assert_eq!(preview["model_meshes"].as_array().unwrap().len(), 101);
        let bytes = serde_json::to_vec(&preview).unwrap().len();
        assert!(
            bytes < 16 * 1024 * 1024,
            "default live preview grew to {:.1} MiB",
            bytes as f64 / (1024.0 * 1024.0)
        );
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
