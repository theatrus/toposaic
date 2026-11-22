//! Printable shell and snap-fit frame prototype.

mod layout;
mod parts;
mod region;

use std::fs::{self, File};
use std::io::{self, BufWriter, Write};
use std::path::Path;

use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};
use zip::{ZipWriter, write::SimpleFileOptions};

use self::layout::{Face, Pose, cross, dot, unit};
use self::region::Region;
use crate::HeightField;
use crate::analysis::analyze_mesh_views;
use crate::export::{write_binary_stl, write_single_mesh_3mf};
use crate::mesh::Mesh;
use crate::project::{Artifact, ProjectManifest, file_artifact};
use crate::spec::GenerationSpec;

const DIAMETER_MM: f32 = 200.0;
const BACKING_MM: f32 = 8.0;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GlobeExterior {
    Faceted,
    Rounded,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GlobeSource {
    #[default]
    FitTest,
    SmallWorld,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct GlobePrototypeSpec {
    pub exterior: GlobeExterior,
    pub source: GlobeSource,
    pub wrap_degrees: f32,
    pub relief_mm: f32,
    pub full_shell: bool,
    /// Combined inset from the two tiles, measured in their back planes.
    pub seam_gap_mm: f32,
}

impl GlobePrototypeSpec {
    pub(crate) fn validate(self) -> Result<()> {
        ensure!(
            (0.2..=1.2).contains(&self.seam_gap_mm),
            "prototype seam gap must be between 0.2 and 1.2 mm"
        );
        ensure!(
            (45.0..=140.0).contains(&self.wrap_degrees),
            "Small World wrap must be between 45 and 140 degrees"
        );
        ensure!(
            (0.0..=12.0).contains(&self.relief_mm),
            "Small World relief must be between 0 and 12 mm"
        );
        Ok(())
    }
}

impl Default for GlobePrototypeSpec {
    fn default() -> Self {
        Self {
            exterior: GlobeExterior::Faceted,
            source: GlobeSource::FitTest,
            wrap_degrees: 110.0,
            relief_mm: 8.0,
            full_shell: false,
            seam_gap_mm: 0.4,
        }
    }
}

struct Part {
    mesh: Mesh,
    poses: Vec<Pose>,
}

impl Part {
    fn single(mesh: Mesh, pose: Pose) -> Self {
        Self {
            mesh,
            poses: vec![pose],
        }
    }
}

#[derive(Serialize)]
struct PartManifest<'a> {
    name: &'a str,
    stl: String,
    three_mf: String,
    print_bounds_mm: [f32; 6],
    assembled_poses: &'a [Pose],
}

fn globe_parts(
    spec: &GlobePrototypeSpec,
    field: Option<&HeightField>,
) -> Result<(Vec<Part>, Vec<usize>)> {
    spec.validate()?;
    let faces = Face::all(DIAMETER_MM / 2.0, BACKING_MM);
    let region = match spec.source {
        GlobeSource::FitTest => None,
        GlobeSource::SmallWorld => Some(Region::new(
            field.context("Small World requires a regional elevation field")?,
            &faces[0],
            spec.wrap_degrees,
            spec.relief_mm,
        )),
    };
    let selected = if spec.full_shell {
        faces.iter().collect::<Vec<_>>()
    } else {
        let middle = &faces[0];
        let mut neighbors: Vec<_> = faces
            .iter()
            .filter(|other| middle.adjacent(other))
            .take(2)
            .collect();
        neighbors.push(middle);
        neighbors
    };
    let mut parts = Vec::new();
    let (clip, clip_in_face) = parts::build_clip()?;
    let mut clip_poses = Vec::new();
    for (i, face) in selected.iter().enumerate() {
        let edges: Vec<_> = selected
            .iter()
            .filter_map(|other| face.edge_with(other))
            .collect();
        let tile = parts::build_tile(face, spec, region.as_ref())?;
        verify_tile_insertion(face, &tile)?;
        parts.push(Part::single(tile, face.pose));
        let (frame, pose) = parts::build_frame(face, &edges)?;
        parts.push(Part::single(frame, pose));
        let origin = face.pose.to_globe(clip_in_face.origin);
        let axis = |axis| layout::sub(face.pose.to_globe(axis), face.pose.origin);
        clip_poses.push(Pose::new(
            origin,
            axis(clip_in_face.x_axis),
            axis(clip_in_face.z_axis),
        ));
        for other in &selected[..i] {
            if face.adjacent(other) {
                let (joint, pose) = parts::build_bracket(other, face)?;
                parts.push(Part::single(joint, pose));
            }
        }
    }
    parts.push(Part {
        mesh: clip,
        poses: clip_poses,
    });
    Ok((parts, selected.iter().map(|face| face.id).collect()))
}

fn verify_tile_insertion(face: &Face, tile: &Mesh) -> Result<()> {
    // The neighboring tiles sit in other convex cones. Show that translating
    // this whole tile outward along its mounting axis can never cross a seam.
    let rays = face
        .triangle
        .map(|[x, y]| face.pose.to_globe([x, y, BACKING_MM]));
    for i in 0..3 {
        let side = unit(cross(rays[i], rays[(i + 1) % 3]));
        ensure!(
            dot(side, face.pose.z_axis) > 0.0,
            "{} has no clear insertion axis",
            tile.name
        );
        ensure!(
            tile.vertices
                .iter()
                .all(|&point| dot(side, face.pose.to_globe(point)) > 0.0),
            "{} crosses its seam plane",
            tile.name
        );
    }
    Ok(())
}

fn validate_part(mesh: &Mesh) -> Result<[f32; 6]> {
    const BED_TOLERANCE_MM: f32 = 0.000_1;
    let report = analyze_mesh_views(mesh);
    for view in &report.views {
        ensure!(view.is_clean(), "{}: invalid mesh view {view:?}", mesh.name);
    }
    ensure!(
        report.signed_volume_mm3 > 0.0 && report.bounds_mm[2].abs() < BED_TOLERANCE_MM,
        "{} has invalid volume or print orientation: {:?}",
        mesh.name,
        report.bounds_mm
    );
    ensure!(
        mesh.triangles.iter().any(|triangle| triangle
            .iter()
            .all(|&index| { mesh.vertices[index as usize][2].abs() < BED_TOLERANCE_MM })),
        "{} has no flat bed face",
        mesh.name
    );
    Ok(report.bounds_mm)
}

/// Export the synthetic mechanical fixture or full shell. The numbered parts
/// are ready to slice, and the OBJ and manifest describe the assembled poses.
pub fn export_globe_prototype(spec: &GlobePrototypeSpec, output: &Path) -> Result<()> {
    let (parts, install_order) = globe_parts(spec, None)?;
    write_parts(spec, output, &parts, &install_order, |_| Ok(()))?;
    Ok(())
}

fn write_parts(
    spec: &GlobePrototypeSpec,
    output: &Path,
    parts: &[Part],
    install_order: &[usize],
    mut on_progress: impl FnMut(f32) -> Result<()>,
) -> Result<Vec<Artifact>> {
    fs::create_dir_all(output).with_context(|| format!("create {}", output.display()))?;
    ensure!(
        output.read_dir()?.next().is_none(),
        "prototype output directory must be empty"
    );
    let mut entries = Vec::new();
    let print_spec = GenerationSpec::default();
    let mut artifacts = Vec::new();
    for (index, part) in parts.iter().enumerate() {
        on_progress(index as f32 / parts.len() as f32)?;
        let print_bounds_mm = validate_part(&part.mesh)?;
        let stl = format!("{}.stl", part.mesh.name);
        let three_mf = format!("{}.3mf", part.mesh.name);
        let stl_path = output.join(&stl);
        let three_mf_path = output.join(&three_mf);
        write_binary_stl(&part.mesh, &stl_path)?;
        write_single_mesh_3mf(&print_spec, &part.mesh, &three_mf_path)?;
        artifacts.push(file_artifact(&stl_path, "model/stl")?);
        artifacts.push(file_artifact(&three_mf_path, "model/3mf")?);
        entries.push(PartManifest {
            name: &part.mesh.name,
            stl,
            three_mf,
            print_bounds_mm,
            assembled_poses: &part.poses,
        });
    }
    write_assembled_obj(
        parts,
        BufWriter::new(File::create(output.join("assembly.obj"))?),
    )?;
    let manifest = serde_json::json!({
        "prototype": spec,
        "diameter_mm": DIAMETER_MM,
        "install_order": install_order,
        "parts": entries,
        "joint_hardware_per_bracket": "4 low-profile M3 × 12 mm screws and 4 matching nuts",
        "clip_quantity": install_order.len(),
    });
    let mut file = BufWriter::new(File::create(output.join("assembly.json"))?);
    serde_json::to_writer_pretty(&mut file, &manifest)?;
    writeln!(file)?;
    file.flush()?;
    let order = install_order
        .iter()
        .map(|id| format!("{id:02}"))
        .collect::<Vec<_>>()
        .join(", ");
    let guide = format!(
        "# TopoSaic globe prototype\n\n\
         Print one copy of each tile, frame module, and joint bracket, plus {} copies of the snap clip. \
         Choose either the STL files or the 3MF files. Mark the frame and tile IDs after printing. \
         Use assembly.obj to inspect the assembled positions.\n\n\
         1. Join the frame modules with the numbered brackets on the inside. \
         Each bracket uses four low-profile M3 × 12 mm screws and four M3 nuts. \
         Insert the screws from the recessed side of the frame.\n\
         2. Press a clip into each frame seat from the outside until its collar sits against the frame.\n\
         3. Match each tile to its seat and align the locator before pressing it straight on. \
         Install the tiles in this order. {order}.\n\n\
         Record seam gaps, seating repeatability, insertion and removal force, and clip wear.\n",
        install_order.len(),
    );
    fs::write(output.join("ASSEMBLY.md"), guide)?;
    for (name, mime) in [
        ("ASSEMBLY.md", "text/markdown"),
        ("assembly.obj", "model/obj"),
        ("assembly.json", "application/json"),
    ] {
        artifacts.push(file_artifact(&output.join(name), mime)?);
    }
    on_progress(1.0)?;
    Ok(artifacts)
}

pub fn build_globe_preview(
    spec: &GlobePrototypeSpec,
    field: Option<&HeightField>,
) -> Result<serde_json::Value> {
    let (parts, _) = globe_parts(spec, field)?;
    preview_with_parts(&parts)
}

fn preview_with_parts(parts: &[Part]) -> Result<serde_json::Value> {
    #[derive(Serialize)]
    struct PreviewPart<'a> {
        name: String,
        kind: &'static str,
        outward: Vec<[f32; 3]>,
        vertices: &'a [[f32; 3]],
        triangles: &'a [[u32; 3]],
        poses: &'a [Pose],
    }
    let meshes = parts
        .iter()
        .map(|part| {
            let kind = if part.mesh.name.starts_with("tile-") {
                "tile"
            } else if part.mesh.name.starts_with("frame-") {
                "frame"
            } else if part.mesh.name.starts_with("joint-") {
                "joint"
            } else {
                "clip"
            };
            PreviewPart {
                name: part.mesh.name.clone(),
                kind,
                outward: part.poses.iter().map(|pose| unit(pose.origin)).collect(),
                vertices: &part.mesh.vertices,
                triangles: &part.mesh.triangles,
                poses: &part.poses,
            }
        })
        .collect::<Vec<_>>();
    Ok(serde_json::json!({
        "width": 0, "height": 0, "values": [], "rows": 0, "columns": 0,
        "globe_meshes": meshes,
    }))
}

/// Publish a globe job through the same artifact and saved-setup contracts as terrain.
pub fn generate_globe_project(
    spec: &GenerationSpec,
    field: Option<&HeightField>,
    output: &Path,
    mut on_progress: impl FnMut(f32) -> Result<()>,
) -> Result<ProjectManifest> {
    spec.validate()?;
    let globe = spec
        .globe
        .context("globe project requires globe settings")?;
    on_progress(0.0)?;
    let (parts, order) = globe_parts(&globe, field)?;
    let files = write_parts(&globe, output, &parts, &order, |fraction| {
        on_progress(0.85 * fraction)
    })?;
    let preview_path = output.join("preview.json");
    fs::write(
        &preview_path,
        serde_json::to_vec(&preview_with_parts(&parts)?)?,
    )?;

    let archive_path = output.join("globe-print-kit.zip");
    let mut archive = ZipWriter::new(File::create(&archive_path)?);
    let options = SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);
    for (index, file) in files.iter().enumerate() {
        on_progress(0.85 + 0.14 * index as f32 / files.len() as f32)?;
        archive.start_file(&file.name, options)?;
        io::copy(&mut File::open(output.join(&file.name))?, &mut archive)?;
    }
    archive.finish()?;
    let manifest_path = output.join("manifest.json");
    let mut manifest = ProjectManifest {
        generator: format!("toposaic/{}", env!("CARGO_PKG_VERSION")),
        terrain_source: field
            .filter(|_| globe.source == GlobeSource::SmallWorld)
            .map_or_else(
                || "Synthetic globe fit geometry".into(),
                |field| field.source.clone(),
            ),
        surface_source: None,
        spec: spec.clone(),
        artifacts: vec![file_artifact(&archive_path, "application/zip")?],
    };
    manifest.artifacts.extend(files);
    manifest
        .artifacts
        .push(file_artifact(&preview_path, "application/json")?);
    fs::write(&manifest_path, serde_json::to_vec_pretty(&manifest)?)?;
    manifest
        .artifacts
        .push(file_artifact(&manifest_path, "application/json")?);
    on_progress(1.0)?;
    Ok(manifest)
}

fn write_assembled_obj(parts: &[Part], mut writer: impl Write) -> Result<()> {
    let mut index_offset = 1;
    for part in parts {
        for (i, &pose) in part.poses.iter().enumerate() {
            writeln!(writer, "o {}-{}", part.mesh.name, i + 1)?;
            for &point in &part.mesh.vertices {
                let [x, y, z] = pose.to_globe(point);
                writeln!(writer, "v {x:.5} {y:.5} {z:.5}")?;
            }
            for triangle in &part.mesh.triangles {
                let [a, b, c] = triangle.map(|id| id as usize + index_offset);
                writeln!(writer, "f {a} {b} {c}")?;
            }
            index_offset += part.mesh.vertices.len();
        }
    }
    writer.flush()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fixture_contains_two_joints_and_installs_the_middle_tile_last() {
        let (parts, order) = globe_parts(&GlobePrototypeSpec::default(), None).unwrap();
        assert_eq!(order, [2, 5, 1]);
        assert_eq!(parts.len(), 9);
        assert_eq!(parts.last().unwrap().poses.len(), 3);
        for part in parts {
            validate_part(&part.mesh).unwrap();
        }
    }

    #[test]
    fn both_full_shells_export_watertight_tiles_and_frame_parts() {
        for exterior in [GlobeExterior::Faceted, GlobeExterior::Rounded] {
            let spec = GlobePrototypeSpec {
                exterior,
                full_shell: true,
                ..GlobePrototypeSpec::default()
            };
            let (parts, order) = globe_parts(&spec, None).unwrap();
            assert_eq!(order.len(), 20);
            assert_eq!(parts.len(), 20 + 20 + 30 + 1);
            assert_eq!(parts.last().unwrap().poses.len(), 20);
            for part in parts {
                validate_part(&part.mesh).unwrap();
            }
        }
    }

    #[test]
    fn small_world_adds_regional_heights_above_the_same_frame() {
        let field = HeightField::new(
            3,
            3,
            vec![0.0, 200.0, 0.0, 400.0, 1_000.0, 200.0, 0.0, 100.0, 0.0],
            "test ridge",
        )
        .unwrap();
        let blank = GlobePrototypeSpec::default();
        let (blank_parts, _) = globe_parts(&blank, None).unwrap();
        let spec = GlobePrototypeSpec {
            source: GlobeSource::SmallWorld,
            ..blank
        };
        assert!(globe_parts(&spec, None).is_err());
        let (mapped, _) = globe_parts(&spec, Some(&field)).unwrap();
        for (mapped, blank) in mapped.iter().zip(&blank_parts) {
            validate_part(&mapped.mesh).unwrap();
            if mapped.mesh.name.starts_with("tile-") {
                assert!(mapped.mesh.vertices.len() > blank.mesh.vertices.len());
            } else {
                assert_eq!(mapped.mesh.vertices, blank.mesh.vertices);
            }
        }
        let faces = Face::all(DIAMETER_MM / 2.0, BACKING_MM);
        let region = Region::new(&field, &faces[0], spec.wrap_degrees, spec.relief_mm);
        assert!((region.height(faces[0].pose.z_axis) - spec.relief_mm).abs() < 1.0e-5);
        assert_eq!(
            region.height(layout::scale(faces[0].pose.z_axis, -1.0)),
            0.0
        );
    }
}
