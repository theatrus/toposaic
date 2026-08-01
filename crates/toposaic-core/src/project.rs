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

use crate::export::{ThreeMfWriter, write_binary_stl, write_single_mesh_3mf};
use crate::heightfield::{HeightField, height_range_for_spec, validate_height_frame};
use crate::marker::build_flag_template;
use crate::mesh::Mesh;
use crate::mount::{build_wall_alignment_spacer, build_wall_hardware};
use crate::piece::{build_piece_with_height_range, printable_piece_positions};
use crate::preview::{build_preview, preview_sample_count};
use crate::spec::{GenerationSpec, MapMarker, MarkerKind, PaintedClasses, WallMountStyle};
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
    tray_spec.color_output.route_trail_color = Some(spec.tray.tray_color.clone());
    tray_spec.color_output.trail_color = spec.tray.tray_color.clone();
    tray_spec.color_output.rail_color = spec.tray.tray_color.clone();
    tray_spec.color_output.aerial_color = spec.tray.tray_color.clone();
    // Trays never draw trails, railways, or lifts; dropping all three keeps
    // the terrain model's settings from reaching the tray at all.
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
        // A tray prints in three colors at most — its rim, its contours, and
        // its label — whatever the terrain model is set to.
        write_single_mesh_3mf(&tray_spec, tray_mesh, &tray_3mf_path)?;
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
    let hardware = build_wall_hardware(&spec.wall_mount, spec.wall_mount_target_size()[0])?;
    let stl_path = output_dir.join("wall-mount-hardware.stl");
    write_binary_stl(&hardware, &stl_path)?;

    let mut hardware_spec = spec.clone();
    hardware_spec.solid_model = true;
    hardware_spec.color_output.enabled = false;
    hardware_spec.buildings.enabled = false;
    hardware_spec.trails.clear();
    let three_mf_path = output_dir.join("wall-mount-hardware.3mf");
    write_single_mesh_3mf(&hardware_spec, &hardware, &three_mf_path)?;
    let mut artifacts = vec![
        file_artifact(&stl_path, "model/stl")?,
        file_artifact(&three_mf_path, "model/3mf")?,
    ];
    if spec.wall_mount.style == WallMountStyle::FrenchCleat {
        let spacer = build_wall_alignment_spacer(spec)?;
        let spacer_stl_path = output_dir.join("wall-mount-alignment-spacer.stl");
        write_binary_stl(&spacer, &spacer_stl_path)?;
        artifacts.push(file_artifact(&spacer_stl_path, "model/stl")?);

        let spacer_3mf_path = output_dir.join("wall-mount-alignment-spacer.3mf");
        write_single_mesh_3mf(&hardware_spec, &spacer, &spacer_3mf_path)?;
        artifacts.push(file_artifact(&spacer_3mf_path, "model/3mf")?);
    }
    Ok(artifacts)
}

pub fn generate_marker_artifacts(
    spec: &GenerationSpec,
    output_dir: &Path,
) -> Result<Vec<Artifact>> {
    let export_flags = spec
        .markers
        .iter()
        .filter(|marker| marker.kind.is_flag())
        .filter(|marker| marker.flag_style().export_template)
        .collect::<Vec<_>>();
    if export_flags.is_empty() {
        return Ok(Vec::new());
    }
    let mut flag_spec = spec.clone();
    flag_spec.color_output.enabled = false;
    flag_spec.buildings.enabled = false;
    flag_spec.trails.clear();
    if !flag_spec.uses_colored_markers() {
        flag_spec.markers.push(MapMarker {
            name: "Flag template".into(),
            latitude: flag_spec.center_lat,
            longitude: flag_spec.center_lon,
            kind: MarkerKind::Dot,
            label_height_mm: 4.0,
            rotation_degrees: 0.0,
            dot_style: None,
            flag_style: None,
            label_style: None,
        });
    }
    let mut artifacts = Vec::new();
    let blank_flags = export_flags
        .iter()
        .copied()
        .filter(|marker| marker.kind == MarkerKind::FlagHole)
        .collect::<Vec<_>>();
    for (index, marker) in blank_flags.iter().enumerate() {
        let style = marker.flag_style();
        let blank = build_flag_template(&style, None)?;
        let stem = if blank_flags.len() == 1 {
            "marker-flag-template".into()
        } else {
            format!(
                "marker-flag-blank-{:02}-{}",
                index + 1,
                artifact_slug(&marker.name)
            )
        };
        artifacts.extend(write_flag_artifacts(&flag_spec, output_dir, &stem, &blank)?);
    }
    for (index, marker) in export_flags
        .iter()
        .copied()
        .filter(|marker| marker.kind == MarkerKind::FlagLabel)
        .enumerate()
    {
        let style = marker.flag_style();
        let flag = build_flag_template(&style, Some(marker.name.trim()))?;
        let stem = format!(
            "marker-flag-{:02}-{}",
            index + 1,
            artifact_slug(&marker.name)
        );
        artifacts.extend(write_flag_artifacts(&flag_spec, output_dir, &stem, &flag)?);
    }
    Ok(artifacts)
}

fn write_flag_artifacts(
    spec: &GenerationSpec,
    output_dir: &Path,
    stem: &str,
    flag: &Mesh,
) -> Result<Vec<Artifact>> {
    let stl_path = output_dir.join(format!("{stem}.stl"));
    write_binary_stl(flag, &stl_path)?;
    let three_mf_path = output_dir.join(format!("{stem}.3mf"));
    write_single_mesh_3mf(spec, flag, &three_mf_path)?;
    Ok(vec![
        file_artifact(&stl_path, "model/stl")?,
        file_artifact(&three_mf_path, "model/3mf")?,
    ])
}

fn artifact_slug(name: &str) -> String {
    let mut slug = String::new();
    let mut separator = false;
    for character in name.chars() {
        if character.is_ascii_alphanumeric() {
            if separator && !slug.is_empty() {
                slug.push('-');
            }
            slug.push(character.to_ascii_lowercase());
            separator = false;
        } else {
            separator = true;
        }
    }
    if slug.is_empty() {
        "label".into()
    } else {
        slug.truncate(40);
        slug.trim_end_matches('-').to_string()
    }
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
    if spec.uses_building_markers() && surface_field.is_none() {
        bail!("building markers require OpenStreetMap building data");
    }
    fs::create_dir_all(output_dir)
        .with_context(|| format!("create output directory {}", output_dir.display()))?;

    let piece_positions = printable_piece_positions(spec)?;
    let object_count = piece_positions.len();

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
            let mut project_writer =
                ThreeMfWriter::new(spec, PaintedClasses::sampled(surface_field), writer_path)?;
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
                        let (row, column) = piece_positions[index];
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
    artifacts.extend(generate_marker_artifacts(spec, output_dir)?);
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
        BuildingSpec, ColorOutputSpec, OutlineShape, SurfaceClass, WallMountSpec, WallMountStyle,
        WallMountTarget,
    };

    fn colors_in(path: &Path) -> Vec<String> {
        let mut archive = zip::ZipArchive::new(File::open(path).unwrap()).unwrap();
        let mut model = String::new();
        archive
            .by_name("3D/3dmodel.model")
            .unwrap()
            .read_to_string(&mut model)
            .unwrap();
        model
            .match_indices("<m:color color=\"")
            .map(|(index, marker)| model[index + marker.len()..][..7].to_owned())
            .collect()
    }

    /// A tray prints in three colors: rim, contours, and label. It used to
    /// ask for six, four of them the same tray color, because the tray spec
    /// points every unused class at the tray color and nothing merged them.
    #[test]
    fn tray_archives_ask_for_the_three_tray_colors_only() {
        let output_dir =
            std::env::temp_dir().join(format!("toposaic-tray-colors-{}", std::process::id()));
        if output_dir.exists() {
            std::fs::remove_dir_all(&output_dir).unwrap();
        }
        let mut spec = GenerationSpec {
            rows: 2,
            columns: 2,
            samples_per_piece: 16,
            ..GenerationSpec::default()
        };
        spec.tray.enabled = true;
        generate_tray_artifacts(&spec, None, &output_dir).unwrap();

        assert_eq!(
            colors_in(&output_dir.join("terrain-tray.3mf")),
            [
                spec.tray.tray_color.as_str(),
                spec.tray.contour_color.as_str(),
                spec.tray.label_color.as_str(),
            ]
        );
        std::fs::remove_dir_all(output_dir).unwrap();
    }

    /// A wall-mount bracket is one solid color and a flag template is two.
    /// Neither has any business asking for the terrain palette.
    #[test]
    fn hardware_and_flag_archives_do_not_carry_the_terrain_palette() {
        let output_dir =
            std::env::temp_dir().join(format!("toposaic-hardware-colors-{}", std::process::id()));
        if output_dir.exists() {
            std::fs::remove_dir_all(&output_dir).unwrap();
        }
        let spec = GenerationSpec {
            rows: 2,
            columns: 2,
            samples_per_piece: 16,
            wall_mount: WallMountSpec {
                style: WallMountStyle::StraightPin,
                ..WallMountSpec::default()
            },
            markers: vec![MapMarker {
                name: "Summit".into(),
                latitude: 46.8523,
                longitude: -121.7603,
                kind: MarkerKind::FlagLabel,
                label_height_mm: 4.0,
                rotation_degrees: 0.0,
                dot_style: None,
                flag_style: None,
                label_style: None,
            }],
            ..GenerationSpec::default()
        };
        generate_wall_mount_artifacts(&spec, &output_dir).unwrap();
        let flags = generate_marker_artifacts(&spec, &output_dir).unwrap();

        // The bracket is rock all over, so it needs one filament — and the
        // terrain colors it never paints must not follow it into the file.
        let hardware = colors_in(&output_dir.join("wall-mount-hardware.3mf"));
        assert!(hardware.len() <= 1, "{hardware:?}");
        assert!(!hardware.contains(&spec.color_output.water_color));

        let flag_archives = flags
            .iter()
            .filter(|artifact| artifact.name.ends_with(".3mf"))
            .collect::<Vec<_>>();
        assert_eq!(flag_archives.len(), 1, "one labeled flag template");
        for artifact in flag_archives {
            // The banner and the name cut into it: two colors, not seven.
            let colors = colors_in(&output_dir.join(&artifact.name));
            assert_eq!(colors.len(), 2, "{} carries {colors:?}", artifact.name);
            for unused in [
                &spec.color_output.forest_color,
                &spec.color_output.water_color,
                &spec.color_output.road_color,
                &spec.color_output.building_color,
            ] {
                assert!(
                    !colors.contains(unused),
                    "{} carries {unused}",
                    artifact.name
                );
            }
        }
        std::fs::remove_dir_all(output_dir).unwrap();
    }

    /// A map with no water, no roads, and no buildings pays for none of the
    /// three. Only the classes its own data holds reach the color group.
    #[test]
    fn wilderness_maps_do_not_pay_for_water_roads_or_buildings() {
        let output_dir =
            std::env::temp_dir().join(format!("toposaic-wilderness-{}", std::process::id()));
        if output_dir.exists() {
            std::fs::remove_dir_all(&output_dir).unwrap();
        }
        let spec = GenerationSpec {
            rows: 2,
            columns: 2,
            samples_per_piece: 16,
            color_output: ColorOutputSpec {
                enabled: true,
                ..ColorOutputSpec::default()
            },
            ..GenerationSpec::default()
        };
        let height = HeightField::new(5, 5, vec![0.0; 25], "test").unwrap();
        let surface = SurfaceField::new(
            3,
            3,
            vec![
                SurfaceClass::Rock,
                SurfaceClass::Forest,
                SurfaceClass::Rock,
                SurfaceClass::Forest,
                SurfaceClass::Rock,
                SurfaceClass::Forest,
                SurfaceClass::Rock,
                SurfaceClass::Forest,
                SurfaceClass::Rock,
            ],
            "wilderness",
        )
        .unwrap();
        generate_project_with_fields(&spec, &height, Some(&surface), &output_dir).unwrap();

        assert_eq!(
            colors_in(&output_dir.join("toposaic.3mf")),
            [
                spec.color_output.rock_color.as_str(),
                spec.color_output.forest_color.as_str(),
            ]
        );
        std::fs::remove_dir_all(output_dir).unwrap();
    }

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
    fn shaped_projects_omit_cells_outside_the_outline() {
        let output_dir = std::env::temp_dir().join(format!(
            "toposaic-shaped-project-test-{}",
            std::process::id()
        ));
        if output_dir.exists() {
            std::fs::remove_dir_all(&output_dir).unwrap();
        }
        let mut spec = GenerationSpec {
            width_mm: 120.0,
            rows: 2,
            columns: 6,
            samples_per_piece: 16,
            ..GenerationSpec::default()
        };
        spec.model_outline.shape = OutlineShape::Circle;
        let manifest = generate_project(&spec, &output_dir).unwrap();

        assert!(output_dir.join("toposaic.3mf").is_file());
        assert!(!output_dir.join("piece-1-1.stl").exists());
        assert!(output_dir.join("piece-1-3.stl").is_file());
        assert!(
            manifest
                .artifacts
                .iter()
                .all(|artifact| artifact.name != "piece-1-1.stl")
        );

        std::fs::remove_dir_all(output_dir).unwrap();
    }

    #[test]
    fn puzzle_wall_mount_jobs_export_full_tile_hardware_and_alignment_spacer() {
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
            wall_mount: WallMountSpec {
                style: WallMountStyle::FrenchCleat,
                target: WallMountTarget::Terrain,
                export_hardware: true,
                ..WallMountSpec::default()
            },
            ..GenerationSpec::default()
        };
        let manifest = generate_project(&spec, &output_dir).unwrap();
        for name in [
            "wall-mount-hardware.stl",
            "wall-mount-hardware.3mf",
            "wall-mount-alignment-spacer.stl",
            "wall-mount-alignment-spacer.3mf",
        ] {
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
    fn flag_jobs_honor_each_markers_export_choice() {
        let output_dir =
            std::env::temp_dir().join(format!("toposaic-marker-flag-test-{}", std::process::id()));
        if output_dir.exists() {
            std::fs::remove_dir_all(&output_dir).unwrap();
        }
        let spec = GenerationSpec {
            solid_model: true,
            samples_per_piece: 24,
            markers: vec![
                crate::spec::MapMarker {
                    name: "Blank".into(),
                    latitude: 46.8523,
                    longitude: -121.7603,
                    kind: crate::spec::MarkerKind::FlagHole,
                    label_height_mm: 4.0,
                    rotation_degrees: 0.0,
                    dot_style: None,
                    flag_style: Some(crate::spec::FlagMarkerStyle {
                        export_template: false,
                        ..crate::spec::FlagMarkerStyle::default()
                    }),
                    label_style: None,
                },
                crate::spec::MapMarker {
                    name: "Mount Fuji 富士山".into(),
                    latitude: 46.8523,
                    longitude: -121.7503,
                    kind: crate::spec::MarkerKind::FlagLabel,
                    label_height_mm: 4.0,
                    rotation_degrees: 0.0,
                    dot_style: None,
                    flag_style: None,
                    label_style: None,
                },
            ],
            ..GenerationSpec::default()
        };
        let manifest = generate_project(&spec, &output_dir).unwrap();
        for name in [
            "marker-flag-01-mount-fuji.stl",
            "marker-flag-01-mount-fuji.3mf",
        ] {
            assert!(output_dir.join(name).is_file());
            assert!(
                manifest
                    .artifacts
                    .iter()
                    .any(|artifact| artifact.name == name)
            );
        }
        assert!(!output_dir.join("marker-flag-template.stl").exists());
        assert!(!output_dir.join("marker-flag-template.3mf").exists());
        std::fs::remove_dir_all(output_dir).unwrap();
    }

    #[test]
    fn vector_markers_generate_from_elevation_without_surface_downloads() {
        let output_dir =
            std::env::temp_dir().join(format!("toposaic-map-label-test-{}", std::process::id()));
        if output_dir.exists() {
            std::fs::remove_dir_all(&output_dir).unwrap();
        }
        let defaults = GenerationSpec::default();
        let spec = GenerationSpec {
            solid_model: true,
            samples_per_piece: 24,
            markers: vec![
                crate::spec::MapMarker {
                    name: "Mirror Lake".into(),
                    latitude: defaults.center_lat,
                    longitude: defaults.center_lon,
                    kind: crate::spec::MarkerKind::PlaqueLabel,
                    label_height_mm: 4.0,
                    rotation_degrees: 25.0,
                    dot_style: None,
                    flag_style: None,
                    label_style: None,
                },
                crate::spec::MapMarker {
                    name: "Trailhead".into(),
                    latitude: defaults.center_lat,
                    longitude: defaults.center_lon + 0.01,
                    kind: crate::spec::MarkerKind::Dot,
                    label_height_mm: 4.0,
                    rotation_degrees: 0.0,
                    dot_style: None,
                    flag_style: None,
                    label_style: None,
                },
            ],
            ..defaults
        };
        let height = HeightField::new(5, 5, vec![100.0; 25], "test").unwrap();
        let manifest = generate_project_with_height_field(&spec, &height, &output_dir).unwrap();
        assert!(manifest.surface_source.is_none());
        assert!(output_dir.join("terrain-solid.stl").is_file());
        assert!(output_dir.join("terrain-solid.3mf").is_file());
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
        // Two filaments, not the old fixed six. This model prints rock and
        // buildings; with surface colors switched off it never paints the
        // forest, snow, water, or road colors, so they take no slot and the
        // buildings pack into slot two.
        assert_eq!(model.matches("<m:color ").count(), 2);
        assert!(model.contains("p1=\"1\""));

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
