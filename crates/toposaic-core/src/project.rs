use std::{
    fs,
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
};

use anyhow::{Context, Result, bail};
use rayon::prelude::*;
use serde::{Deserialize, Serialize};

use crate::export::{ThreeMfWriter, write_binary_stl};
use crate::heightfield::{HeightField, height_range_for_spec, validate_height_frame};
use crate::mesh::Mesh;
use crate::mount::build_wall_hardware;
use crate::piece::build_piece_with_height_range;
use crate::preview::{build_preview, preview_sample_count};
use crate::spec::GenerationSpec;
use crate::surface::SurfaceField;
use crate::tray::build_tray_segments;

const MAX_PARALLEL_PIECES: usize = 8;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Artifact {
    pub name: String,
    pub media_type: String,
    pub bytes: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProjectManifest {
    pub generator: String,
    pub terrain_source: String,
    pub surface_source: Option<String>,
    pub spec: GenerationSpec,
    pub artifacts: Vec<Artifact>,
}

pub fn generate_project(spec: &GenerationSpec, output_dir: &Path) -> Result<ProjectManifest> {
    generate_project_inner(spec, None, None, output_dir, &|| false, &|_| Ok(()))
}

pub fn generate_project_with_height_field(
    spec: &GenerationSpec,
    height_field: &HeightField,
    output_dir: &Path,
) -> Result<ProjectManifest> {
    generate_project_inner(
        spec,
        Some(height_field),
        None,
        output_dir,
        &|| false,
        &|_| Ok(()),
    )
}

pub fn generate_project_with_fields(
    spec: &GenerationSpec,
    height_field: &HeightField,
    surface_field: Option<&SurfaceField>,
    output_dir: &Path,
) -> Result<ProjectManifest> {
    generate_project_inner(
        spec,
        Some(height_field),
        surface_field,
        output_dir,
        &|| false,
        &|_| Ok(()),
    )
}

pub fn generate_project_with_fields_cancellable(
    spec: &GenerationSpec,
    height_field: &HeightField,
    surface_field: Option<&SurfaceField>,
    output_dir: &Path,
    is_cancelled: &(dyn Fn() -> bool + Sync),
    on_progress: &(dyn Fn(f32) -> Result<()> + Sync),
) -> Result<ProjectManifest> {
    generate_project_inner(
        spec,
        Some(height_field),
        surface_field,
        output_dir,
        is_cancelled,
        on_progress,
    )
}

pub fn generate_tray_artifacts(
    spec: &GenerationSpec,
    height_field: Option<&HeightField>,
    output_dir: &Path,
) -> Result<Vec<Artifact>> {
    if !spec.tray.enabled {
        return Ok(Vec::new());
    }
    spec.tray.validate()?;
    fs::create_dir_all(output_dir)
        .with_context(|| format!("create tray output directory {}", output_dir.display()))?;

    let mut tray_spec = spec.clone();
    tray_spec.solid_model = true;
    tray_spec.color_output.enabled = true;
    tray_spec.color_output.rock_color = spec.tray.tray_color.clone();
    tray_spec.color_output.forest_color = spec.tray.contour_color.clone();
    tray_spec.color_output.snow_color = spec.tray.label_color.clone();
    tray_spec.color_output.water_color = spec.tray.tray_color.clone();
    tray_spec.color_output.road_color = spec.tray.tray_color.clone();
    tray_spec.color_output.building_color = spec.tray.tray_color.clone();
    tray_spec.color_output.trail_color = spec.tray.tray_color.clone();
    tray_spec.color_output.rail_color = spec.tray.tray_color.clone();
    tray_spec.color_output.aerial_color = spec.tray.tray_color.clone();
    // Trays never draw trails, railways, or lifts; dropping all three keeps
    // the tray 3MF at its six-slot layout however the terrain model is
    // configured.
    tray_spec.trails = Vec::new();
    tray_spec.color_output.rail_enabled = false;
    tray_spec.color_output.aerial_enabled = false;

    let tray_meshes = build_tray_segments(spec, height_field)?;
    let mut artifacts = Vec::with_capacity(tray_meshes.len() * 2);
    for (index, tray_mesh) in tray_meshes.iter().enumerate() {
        let row = index as u32 / spec.tray.segment_columns;
        let column = index as u32 % spec.tray.segment_columns;
        let suffix = if tray_meshes.len() == 1 {
            String::new()
        } else {
            format!("-r{:02}-c{:02}", row + 1, column + 1)
        };
        let tray_stl_path = output_dir.join(format!("terrain-tray{suffix}.stl"));
        write_binary_stl(tray_mesh, &tray_stl_path)?;
        artifacts.push(file_artifact(&tray_stl_path, "model/stl")?);

        let tray_3mf_path = output_dir.join(format!("terrain-tray{suffix}.3mf"));
        // Trays carry no surface data, so their palette falls back to the
        // settings alone — the base six, exactly as before.
        let mut tray_writer = ThreeMfWriter::new(&tray_spec, None, &tray_3mf_path)?;
        tray_writer.write_mesh(tray_mesh)?;
        tray_writer.finish()?;
        artifacts.push(file_artifact(&tray_3mf_path, "model/3mf")?);
    }
    Ok(artifacts)
}

/// Writes the printable wall-side half of an enabled mount.
///
/// This is public for the API's super-tile job, which publishes one shared
/// hardware pair after its temporary terrain folders have been removed.
pub fn generate_wall_mount_artifacts(
    spec: &GenerationSpec,
    output_dir: &Path,
) -> Result<Vec<Artifact>> {
    if spec.wall_mount.style == crate::spec::WallMountStyle::None
        || !spec.wall_mount.export_hardware
    {
        return Ok(Vec::new());
    }
    spec.validate()?;
    fs::create_dir_all(output_dir).with_context(|| {
        format!(
            "create wall-mount output directory {}",
            output_dir.display()
        )
    })?;
    let hardware = build_wall_hardware(&spec.wall_mount)?;
    let stl_path = output_dir.join("wall-mount-hardware.stl");
    write_binary_stl(&hardware, &stl_path)?;

    let mut hardware_spec = spec.clone();
    hardware_spec.solid_model = true;
    hardware_spec.color_output.enabled = false;
    hardware_spec.buildings.enabled = false;
    hardware_spec.trails.clear();
    let three_mf_path = output_dir.join("wall-mount-hardware.3mf");
    let mut writer = ThreeMfWriter::new(&hardware_spec, None, &three_mf_path)?;
    writer.write_mesh(&hardware)?;
    writer.finish()?;
    Ok(vec![
        file_artifact(&stl_path, "model/stl")?,
        file_artifact(&three_mf_path, "model/3mf")?,
    ])
}

fn generate_project_inner(
    spec: &GenerationSpec,
    height_field: Option<&HeightField>,
    surface_field: Option<&SurfaceField>,
    output_dir: &Path,
    is_cancelled: &(dyn Fn() -> bool + Sync),
    on_progress: &(dyn Fn(f32) -> Result<()> + Sync),
) -> Result<ProjectManifest> {
    spec.validate()?;
    ensure_generation_active(is_cancelled)?;
    if spec.color_output.enabled && surface_field.is_none() {
        bail!("color output requires ESA WorldCover surface data");
    }
    if spec.buildings.enabled && surface_field.is_none() {
        bail!("building output requires OpenStreetMap building data");
    }
    if spec.uses_trails() && surface_field.is_none() {
        bail!("imported trails require surface data to draw on");
    }
    fs::create_dir_all(output_dir)
        .with_context(|| format!("create output directory {}", output_dir.display()))?;

    let object_count = if spec.solid_model {
        1
    } else {
        (spec.rows * spec.columns) as usize
    };

    let mut artifacts = Vec::new();
    validate_height_frame(spec, height_field)?;
    let height_range = height_range_for_spec(spec, height_field);
    let project_path = output_dir.join(if spec.solid_model {
        "terrain-solid.3mf"
    } else {
        "toposaic.3mf"
    });
    let piece_batch_size = object_count
        .min(rayon::current_num_threads())
        .clamp(1, MAX_PARALLEL_PIECES);
    // The 3MF write is serial (one deflate stream), so run it on its own
    // thread and feed it finished meshes in index order over a bounded
    // channel: batch k's 3MF write overlaps batch k+1's builds. The writer
    // consumes meshes in send order, so the file bytes are unchanged.
    //
    // The writer finalizes the archive only when the build side explicitly
    // declared success first. The flag starts false, so a build error, a
    // cancel, AND a panic unwinding out of a piece builder (which drops the
    // sender mid-stream without ever reaching the success store) all leave
    // it unset — a truncated archive can never be finished into one that
    // looks complete.
    let build_completed = AtomicBool::new(false);
    let (mesh_sender, mesh_receiver) = mpsc::sync_channel::<Mesh>(piece_batch_size);
    // However this function exits before the disarm below — error, cancel,
    // or panic — the partial, unfinished project archive is removed.
    let mut partial_archive_guard = RemoveFileOnDrop::new(&project_path);
    std::thread::scope(|scope| -> Result<()> {
        let completed_flag = &build_completed;
        let writer_path = &project_path;
        let writer = scope.spawn(move || -> Result<()> {
            // The surface field is finished before any mesh is built, so
            // the palette can be sized from the data the meshes will sample
            // without buffering a single mesh.
            let mut project_writer = ThreeMfWriter::new(spec, surface_field, writer_path)?;
            for mesh in mesh_receiver {
                project_writer.write_mesh(&mesh)?;
            }
            if !completed_flag.load(Ordering::Acquire) {
                // The building side failed, was canceled, or panicked; skip
                // finalizing the archive, its error is reported instead.
                return Ok(());
            }
            project_writer.finish()
        });
        let build_result = (|| -> Result<()> {
            for batch_start in (0..object_count).step_by(piece_batch_size) {
                ensure_generation_active(is_cancelled)?;
                let batch_end = (batch_start + piece_batch_size).min(object_count);
                let pieces = (batch_start..batch_end)
                    .into_par_iter()
                    .map(|index| -> Result<(Mesh, Artifact)> {
                        ensure_generation_active(is_cancelled)?;
                        let row = if spec.solid_model {
                            0
                        } else {
                            index as u32 / spec.columns
                        };
                        let column = if spec.solid_model {
                            0
                        } else {
                            index as u32 % spec.columns
                        };
                        let mesh = build_piece_with_height_range(
                            spec,
                            height_field,
                            height_range,
                            surface_field,
                            row,
                            column,
                        )
                        .with_context(|| format!("build piece {}, {}", row + 1, column + 1))?;
                        ensure_generation_active(is_cancelled)?;
                        let name = if spec.solid_model {
                            "terrain-solid.stl".into()
                        } else {
                            format!("piece-{}-{}.stl", row + 1, column + 1)
                        };
                        let path = output_dir.join(&name);
                        write_binary_stl(&mesh, &path)?;
                        let artifact = file_artifact(&path, "model/stl")?;
                        Ok((mesh, artifact))
                    })
                    .collect::<Vec<_>>();
                for piece in pieces {
                    ensure_generation_active(is_cancelled)?;
                    let (mesh, artifact) = piece?;
                    artifacts.push(artifact);
                    if mesh_sender.send(mesh).is_err() {
                        // The writer thread dropped the receiver after an
                        // error; the join below reports it.
                        return Ok(());
                    }
                }
                on_progress(batch_end as f32 / object_count as f32 * 0.9)?;
            }
            ensure_generation_active(is_cancelled)
        })();
        if build_result.is_ok() {
            build_completed.store(true, Ordering::Release);
        }
        drop(mesh_sender);
        // Re-raise the writer thread's panic with its original message and
        // location; `expect` here would report only `Any { .. }`.
        let write_result = match writer.join() {
            Ok(result) => result,
            Err(payload) => std::panic::resume_unwind(payload),
        };
        build_result?;
        write_result
    })?;
    partial_archive_guard.disarm();
    artifacts.push(file_artifact(&project_path, "model/3mf")?);

    if spec.tray.enabled {
        ensure_generation_active(is_cancelled)?;
        artifacts.extend(generate_tray_artifacts(spec, height_field, output_dir)?);
    }
    ensure_generation_active(is_cancelled)?;
    artifacts.extend(generate_wall_mount_artifacts(spec, output_dir)?);
    on_progress(0.95)?;

    ensure_generation_active(is_cancelled)?;
    let preview_path = output_dir.join("preview.json");
    let preview_size = preview_sample_count(spec);
    let preview = build_preview(spec, height_field, surface_field, preview_size);
    fs::write(&preview_path, serde_json::to_vec(&preview)?)
        .with_context(|| format!("write {}", preview_path.display()))?;
    artifacts.push(file_artifact(&preview_path, "application/json")?);
    on_progress(0.98)?;

    let manifest = ProjectManifest {
        generator: format!("toposaic/{}", env!("CARGO_PKG_VERSION")),
        terrain_source: height_field
            .map(|field| field.source.clone())
            .unwrap_or_else(|| "deterministic-preview-surface".into()),
        surface_source: surface_field.map(|field| field.source.clone()),
        spec: spec.clone(),
        artifacts,
    };
    let manifest_path = output_dir.join("manifest.json");
    ensure_generation_active(is_cancelled)?;
    fs::write(&manifest_path, serde_json::to_vec_pretty(&manifest)?)
        .with_context(|| format!("write {}", manifest_path.display()))?;

    let mut complete = manifest;
    complete
        .artifacts
        .push(file_artifact(&manifest_path, "application/json")?);
    on_progress(1.0)?;
    Ok(complete)
}

/// Removes a file when dropped unless disarmed: keeps a partially written
/// project archive from surviving an error, a cancel, or a panic.
struct RemoveFileOnDrop<'a> {
    path: &'a Path,
    armed: bool,
}

impl<'a> RemoveFileOnDrop<'a> {
    fn new(path: &'a Path) -> Self {
        Self { path, armed: true }
    }

    fn disarm(&mut self) {
        self.armed = false;
    }
}

impl Drop for RemoveFileOnDrop<'_> {
    fn drop(&mut self) {
        if self.armed {
            let _ = fs::remove_file(self.path);
        }
    }
}

fn ensure_generation_active(is_cancelled: &(dyn Fn() -> bool + Sync)) -> Result<()> {
    if is_cancelled() {
        bail!("generation canceled");
    }
    Ok(())
}

fn file_artifact(path: &Path, media_type: &str) -> Result<Artifact> {
    Ok(Artifact {
        name: path
            .file_name()
            .and_then(|name| name.to_str())
            .context("artifact has no file name")?
            .to_owned(),
        media_type: media_type.to_owned(),
        bytes: fs::metadata(path)?.len(),
    })
}

pub fn artifact_path(output_dir: &Path, name: &str) -> Option<PathBuf> {
    let candidate = Path::new(name);
    if candidate.components().count() != 1 {
        return None;
    }
    let path = output_dir.join(candidate);
    path.is_file().then_some(path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{collections::HashMap, fs::File, io::Read};

    use crate::piece::{build_piece, solid_outline};
    use crate::spec::{
        BuildingSpec, ColorOutputSpec, SurfaceClass, WallMountSpec, WallMountStyle, WallMountTarget,
    };

    #[test]
    fn project_writes_print_artifacts() {
        let output_dir =
            std::env::temp_dir().join(format!("toposaic-core-test-{}", std::process::id()));
        if output_dir.exists() {
            std::fs::remove_dir_all(&output_dir).unwrap();
        }

        let spec = GenerationSpec {
            rows: 2,
            columns: 2,
            samples_per_piece: 16,
            ..GenerationSpec::default()
        };
        let progress = std::sync::Mutex::new(Vec::new());
        let manifest =
            generate_project_inner(&spec, None, None, &output_dir, &|| false, &|value| {
                progress.lock().unwrap().push(value);
                Ok(())
            })
            .unwrap();
        let progress = progress.into_inner().unwrap();

        assert!(output_dir.join("toposaic.3mf").is_file());
        assert!(output_dir.join("piece-1-1.stl").is_file());
        assert!(output_dir.join("preview.json").is_file());
        assert_eq!(
            manifest
                .artifacts
                .iter()
                .filter(|artifact| artifact.name.ends_with(".stl"))
                .map(|artifact| artifact.name.as_str())
                .collect::<Vec<_>>(),
            [
                "piece-1-1.stl",
                "piece-1-2.stl",
                "piece-2-1.stl",
                "piece-2-2.stl",
            ]
        );
        assert!(progress.windows(2).all(|values| values[0] <= values[1]));
        assert_eq!(progress.last().copied(), Some(1.0));

        std::fs::remove_dir_all(output_dir).unwrap();
    }

    #[test]
    fn wall_mount_jobs_export_matching_printable_hardware() {
        let output_dir = std::env::temp_dir().join(format!(
            "toposaic-wall-hardware-test-{}",
            std::process::id()
        ));
        if output_dir.exists() {
            std::fs::remove_dir_all(&output_dir).unwrap();
        }
        let spec = GenerationSpec {
            rows: 2,
            columns: 2,
            samples_per_piece: 16,
            solid_model: true,
            wall_mount: WallMountSpec {
                style: WallMountStyle::FrenchCleat,
                target: WallMountTarget::Terrain,
                export_hardware: true,
                ..WallMountSpec::default()
            },
            ..GenerationSpec::default()
        };
        let manifest = generate_project(&spec, &output_dir).unwrap();
        for name in ["wall-mount-hardware.stl", "wall-mount-hardware.3mf"] {
            assert!(output_dir.join(name).is_file());
            assert!(
                manifest
                    .artifacts
                    .iter()
                    .any(|artifact| artifact.name == name)
            );
        }
        std::fs::remove_dir_all(output_dir).unwrap();
    }

    #[test]
    fn solid_mode_writes_one_plain_watertight_model() {
        let output_dir =
            std::env::temp_dir().join(format!("terrain-solid-core-test-{}", std::process::id()));
        if output_dir.exists() {
            std::fs::remove_dir_all(&output_dir).unwrap();
        }
        let spec = GenerationSpec {
            rows: 2,
            columns: 2,
            samples_per_piece: 16,
            solid_model: true,
            ..GenerationSpec::default()
        };
        let outline = solid_outline(&spec, 32).unwrap();
        assert!(outline.iter().all(|point| {
            point[0] == 0.0
                || point[0] == spec.width_mm
                || point[1] == 0.0
                || point[1] == spec.height_mm()
        }));

        let manifest = generate_project(&spec, &output_dir).unwrap();
        assert!(output_dir.join("terrain-solid.stl").is_file());
        assert!(output_dir.join("terrain-solid.3mf").is_file());
        assert!(!output_dir.join("toposaic.3mf").exists());
        assert!(!output_dir.join("piece-1-1.stl").exists());
        assert_eq!(
            manifest
                .artifacts
                .iter()
                .filter(|artifact| artifact.name.ends_with(".stl"))
                .count(),
            1
        );

        let mesh = build_piece(&spec, None, None, 0, 0).unwrap();
        let mut edges: HashMap<(u32, u32), u32> = HashMap::new();
        for triangle in &mesh.triangles {
            for edge in [
                (triangle[0], triangle[1]),
                (triangle[1], triangle[2]),
                (triangle[2], triangle[0]),
            ] {
                let ordered = if edge.0 < edge.1 {
                    edge
                } else {
                    (edge.1, edge.0)
                };
                *edges.entry(ordered).or_default() += 1;
            }
        }
        assert!(edges.values().all(|uses| *uses == 2));

        std::fs::remove_dir_all(output_dir).unwrap();
    }

    #[test]
    fn color_project_writes_standard_3mf_properties_and_preview() {
        let output_dir =
            std::env::temp_dir().join(format!("toposaic-color-test-{}", std::process::id()));
        if output_dir.exists() {
            std::fs::remove_dir_all(&output_dir).unwrap();
        }
        let spec = GenerationSpec {
            rows: 2,
            columns: 2,
            samples_per_piece: 16,
            buildings: BuildingSpec {
                enabled: true,
                ..BuildingSpec::default()
            },
            color_output: ColorOutputSpec {
                enabled: true,
                ..ColorOutputSpec::default()
            },
            ..GenerationSpec::default()
        };
        let height =
            HeightField::new(5, 5, (0..25).map(|value| value as f32).collect(), "test").unwrap();
        let mut surface = SurfaceField::new(
            5,
            5,
            (0..25)
                .map(|index| match index % 5 {
                    1 => SurfaceClass::Forest,
                    2 => SurfaceClass::Snow,
                    3 => SurfaceClass::Water,
                    4 => SurfaceClass::Road,
                    _ => SurfaceClass::Rock,
                })
                .collect(),
            "test surface",
        )
        .unwrap();
        surface.paint_building(
            &[[0.35, 0.35], [0.65, 0.35], [0.65, 0.65], [0.35, 0.65]],
            12.0,
        );

        generate_project_with_fields(&spec, &height, Some(&surface), &output_dir).unwrap();

        let file = File::open(output_dir.join("toposaic.3mf")).unwrap();
        let mut archive = zip::ZipArchive::new(file).unwrap();
        let mut model = String::new();
        archive
            .by_name("3D/3dmodel.model")
            .unwrap()
            .read_to_string(&mut model)
            .unwrap();
        assert!(
            model.contains(
                "xmlns:m=\"http://schemas.microsoft.com/3dmanufacturing/material/2015/02\""
            )
        );
        assert!(model.contains("<m:colorgroup id=\"1000\">"));
        assert!(model.contains("color=\"#28543AFF\""));
        assert!(model.contains("color=\"#2F76B5FF\""));
        assert!(model.contains("color=\"#D8A33CFF\""));
        assert!(model.contains("color=\"#B8A890FF\""));
        assert!(model.contains("pid=\"1000\""));
        assert!(model.contains("p1=\"1\""));
        assert!(model.contains("p1=\"2\""));
        assert!(model.contains("p1=\"3\""));
        assert!(model.contains("p1=\"4\""));
        assert!(model.contains("p1=\"5\""));
        assert!(model.contains("paint_color=\"4\""));
        assert!(model.contains("paint_color=\"8\""));
        assert!(model.contains("paint_color=\"0C\""));
        assert!(model.contains("paint_color=\"1C\""));
        assert!(model.contains("paint_color=\"2C\""));
        assert!(model.contains("paint_color=\"3C\""));
        assert_eq!(model.matches("<object id=").count(), 4);
        assert_eq!(model.matches("<item objectid=").count(), 4);

        let mut project_settings = String::new();
        archive
            .by_name("Metadata/project_settings.config")
            .unwrap()
            .read_to_string(&mut project_settings)
            .unwrap();
        let project_settings: serde_json::Value = serde_json::from_str(&project_settings).unwrap();
        assert_eq!(
            project_settings["filament_colour"],
            serde_json::json!([
                "#7C7468", "#28543A", "#F4F3EC", "#2F76B5", "#D8A33C", "#B8A890"
            ])
        );
        assert_eq!(
            project_settings["filament_settings_id"]
                .as_array()
                .unwrap()
                .len(),
            6
        );

        let preview: serde_json::Value =
            serde_json::from_slice(&std::fs::read(output_dir.join("preview.json")).unwrap())
                .unwrap();
        assert!(preview["surface_classes"].is_array());
        assert_eq!(preview["surface_palette"]["rock"], "#7C7468");
        assert_eq!(preview["surface_palette"]["water"], "#2F76B5");
        assert_eq!(preview["surface_palette"]["road"], "#D8A33C");
        assert_eq!(preview["surface_palette"]["building"], "#B8A890");
        assert!(preview["surface_coverage"]["building"].as_f64().unwrap() > 0.0);
        assert_eq!(preview["surface_source"], "test surface");

        std::fs::remove_dir_all(output_dir).unwrap();
    }

    #[test]
    fn building_project_keeps_its_color_without_surface_colors() {
        let output_dir = std::env::temp_dir().join(format!(
            "toposaic-building-color-test-{}",
            std::process::id()
        ));
        if output_dir.exists() {
            std::fs::remove_dir_all(&output_dir).unwrap();
        }
        let spec = GenerationSpec {
            rows: 2,
            columns: 2,
            samples_per_piece: 16,
            buildings: BuildingSpec {
                enabled: true,
                ..BuildingSpec::default()
            },
            color_output: ColorOutputSpec {
                enabled: false,
                building_color: "#8A5B3D".into(),
                ..ColorOutputSpec::default()
            },
            ..GenerationSpec::default()
        };
        let height = HeightField::new(5, 5, vec![0.0; 25], "test").unwrap();
        let mut surface =
            SurfaceField::new(5, 5, vec![SurfaceClass::Rock; 25], "buildings").unwrap();
        surface.paint_building(&[[0.2, 0.2], [0.8, 0.2], [0.8, 0.8], [0.2, 0.8]], 12.0);

        generate_project_with_fields(&spec, &height, Some(&surface), &output_dir).unwrap();

        let file = File::open(output_dir.join("toposaic.3mf")).unwrap();
        let mut archive = zip::ZipArchive::new(file).unwrap();
        let mut model = String::new();
        archive
            .by_name("3D/3dmodel.model")
            .unwrap()
            .read_to_string(&mut model)
            .unwrap();
        assert!(model.contains("<m:colorgroup id=\"1000\">"));
        assert!(model.contains("color=\"#8A5B3DFF\""));
        assert!(model.contains("pid=\"1000\""));
        assert!(model.contains("p1=\"5\""));

        std::fs::remove_dir_all(output_dir).unwrap();
    }

    #[test]
    fn mid_build_cancel_removes_the_partial_project_archive() {
        let output_dir = std::env::temp_dir().join(format!(
            "toposaic-midcancel-core-test-{}",
            std::process::id()
        ));
        if output_dir.exists() {
            std::fs::remove_dir_all(&output_dir).unwrap();
        }
        let spec = GenerationSpec {
            rows: 2,
            columns: 2,
            samples_per_piece: 16,
            ..GenerationSpec::default()
        };
        // Let the first batch build and stream into the archive, then
        // cancel: the writer must not finalize what it already wrote.
        let cancel = std::sync::atomic::AtomicBool::new(false);
        let result = generate_project_inner(
            &spec,
            None,
            None,
            &output_dir,
            &|| cancel.load(std::sync::atomic::Ordering::Acquire),
            &|_| {
                cancel.store(true, std::sync::atomic::Ordering::Release);
                Ok(())
            },
        );

        assert_eq!(result.unwrap_err().to_string(), "generation canceled");
        assert!(
            !output_dir.join("toposaic.3mf").exists(),
            "a canceled build must not leave a partial project archive"
        );
        std::fs::remove_dir_all(&output_dir).unwrap();
    }

    #[test]
    fn a_panicking_build_leaves_no_complete_looking_archive() {
        let output_dir =
            std::env::temp_dir().join(format!("toposaic-panic-core-test-{}", std::process::id()));
        if output_dir.exists() {
            std::fs::remove_dir_all(&output_dir).unwrap();
        }
        let spec = GenerationSpec {
            rows: 2,
            columns: 2,
            samples_per_piece: 16,
            ..GenerationSpec::default()
        };
        // A panic after the first batch drops the mesh sender mid-stream.
        // Without the completed-normally flag the writer would still
        // finalize a valid-looking archive holding a subset of the pieces.
        let unwound = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            generate_project_inner(&spec, None, None, &output_dir, &|| false, &|_| {
                panic!("injected build panic")
            })
        }));

        assert!(unwound.is_err(), "the injected panic must propagate");
        assert!(
            !output_dir.join("toposaic.3mf").exists(),
            "a panicking build must not leave a project archive behind"
        );
        std::fs::remove_dir_all(&output_dir).unwrap();
    }

    #[test]
    fn canceled_generation_stops_before_writing_output() {
        let output_dir = std::env::temp_dir().join(format!(
            "toposaic-canceled-core-test-{}",
            std::process::id()
        ));
        let result = generate_project_inner(
            &GenerationSpec::default(),
            None,
            None,
            &output_dir,
            &|| true,
            &|_| Ok(()),
        );

        assert_eq!(result.unwrap_err().to_string(), "generation canceled");
        assert!(!output_dir.exists());
    }
}
