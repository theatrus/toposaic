//! Printable shell and snap-fit frame prototype.

mod detail;
mod ground;
mod landmarks;
mod layout;
mod parts;
mod region;
mod world;

use std::fs::{self, File};
use std::io::{self, BufWriter, Write};
use std::path::Path;

use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};
use zip::{ZipWriter, write::SimpleFileOptions};

use self::ground::{CLASSES, Ground};
use self::layout::{Face, Pose, Vec3, cross, dot, unit};
use self::region::Region;
use crate::analysis::analyze_mesh_views;
use crate::export::{ThreeMfWriter, write_binary_stl};
use crate::mesh::Mesh;
use crate::project::{Artifact, ProjectManifest, file_artifact};
use crate::spec::{GenerationSpec, MATERIAL_SLOTS, PaintedClasses};
use crate::{HeightField, SurfaceField};

pub use landmarks::{GlobeLandmark, GlobeLandmarkShape};

pub use detail::{EARTH_DETAIL_BLOCK_SIDE, EarthDetail, EarthDetailBlock, earth_detail_blocks};

const REFERENCE_DIAMETER_MM: f32 = 200.0;
const BACKING_MM: f32 = 8.0;
const MAX_SUBDIVISION: u8 = 24;
const MAX_BATCH_TILES: usize = 80;

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
    WholeEarth,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EarthGrid {
    #[default]
    HalfDegree,
    QuarterDegree,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct GlobePrototypeSpec {
    pub exterior: GlobeExterior,
    pub source: GlobeSource,
    pub earth_grid: EarthGrid,
    pub wrap_degrees: f32,
    pub relief_mm: f32,
    pub full_wrap: bool,
    pub surface_colors: bool,
    pub texture_mm: f32,
    pub diameter_mm: f32,
    pub bed_mm: f32,
    /// Zero picks the lowest subdivision frequency that fits the bed.
    pub subdivision: u8,
    pub batch: Option<GlobeBatch>,
    pub samples_per_edge: u8,
    pub whole_earth_detail: bool,
    pub clip_fit_mm: f32,
    pub frame_depth_mm: f32,
    pub locator_clearance_mm: f32,
    pub full_shell: bool,
    /// Combined inset from the two tiles, measured in their back planes.
    pub seam_gap_mm: f32,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct GlobeBatch {
    pub first_tile: usize,
    pub tile_count: usize,
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
            "globe relief must be between 0 and 12 mm"
        );
        ensure!(
            (160.0..=12_000.0).contains(&self.diameter_mm),
            "globe diameter must be between 160 and 12000 mm"
        );
        ensure!(
            (100.0..=420.0).contains(&self.bed_mm),
            "globe bed size must be between 100 and 420 mm"
        );
        ensure!(
            (0.15..=0.6).contains(&self.clip_fit_mm),
            "clip interference must be between 0.15 and 0.60 mm"
        );
        ensure!(
            (0.1..=0.6).contains(&self.locator_clearance_mm),
            "locator clearance must be between 0.10 and 0.60 mm per side"
        );
        ensure!(
            (0.0..=0.9).contains(&self.texture_mm),
            "globe surface texture must be between 0 and 0.9 mm"
        );
        ensure!(
            self.subdivision <= MAX_SUBDIVISION,
            "globe subdivision must be between 0 (automatic) and {MAX_SUBDIVISION}"
        );
        ensure!(
            (16..=96).contains(&self.samples_per_edge),
            "globe mesh detail must be between 16 and 96 samples per edge"
        );
        ensure!(
            !self.whole_earth_detail
                || (self.source == GlobeSource::WholeEarth && self.batch.is_some()),
            "detailed Earth elevation requires a numbered Whole Earth batch"
        );
        ensure!(
            (4.5..=12.0).contains(&self.frame_depth_mm),
            "globe frame depth must be between 4.5 and 12 mm"
        );
        if let Some(batch) = self.batch {
            ensure!(
                batch.first_tile > 0 && (1..=MAX_BATCH_TILES).contains(&batch.tile_count),
                "select a positive first tile and between 1 and {MAX_BATCH_TILES} tiles per batch"
            );
        }
        Ok(())
    }

    fn preview_samples(self, tiles: usize) -> u8 {
        // Keep the original detail for a normal globe or a small batch.
        // For a large batch, bound the total top-surface triangles sent to Studio.
        const PREVIEW_TRIANGLES: f32 = 20.0 * 96.0 * 96.0;
        self.samples_per_edge
            .min((PREVIEW_TRIANGLES / tiles.max(1) as f32).sqrt() as u8)
    }

    pub fn validate_layout(self) -> Result<()> {
        Layout::new(&self).map(|_| ())
    }

    fn layout(self) -> Result<Vec<Face>> {
        let radius = self.diameter_mm / 2.0;
        // Include radial height headroom so a change of terrain or relief
        // does not invalidate the existing frame and its seat IDs.
        let limits = if self.subdivision == 0 {
            1..=MAX_SUBDIVISION
        } else {
            self.subdivision..=self.subdivision
        };
        for frequency in limits {
            let faces = Face::all(radius, BACKING_MM, frequency);
            let edge = faces.iter().map(Face::edge_width).fold(0.0, f32::max);
            let headroom = 2.0 * 13.0 * edge / radius;
            if edge + headroom <= self.bed_mm {
                ensure!(
                    faces.iter().all(|face| face.inradius() >= 20.0),
                    "frame seats are too small for the snap and fasteners; use fewer subdivisions, a larger globe, or a larger bed"
                );
                return Ok(faces);
            }
        }
        anyhow::bail!(
            "the selected globe parts exceed the printer bed; increase subdivision, reduce diameter, or increase bed size"
        )
    }
}

impl Default for GlobePrototypeSpec {
    fn default() -> Self {
        Self {
            exterior: GlobeExterior::Faceted,
            source: GlobeSource::FitTest,
            earth_grid: EarthGrid::HalfDegree,
            wrap_degrees: 110.0,
            relief_mm: 8.0,
            full_wrap: false,
            surface_colors: false,
            texture_mm: 0.0,
            diameter_mm: REFERENCE_DIAMETER_MM,
            bed_mm: 180.0,
            subdivision: 0,
            batch: None,
            samples_per_edge: 48,
            whole_earth_detail: false,
            clip_fit_mm: 0.4,
            frame_depth_mm: 5.0,
            locator_clearance_mm: 0.25,
            full_shell: false,
            seam_gap_mm: 0.4,
        }
    }
}

struct Part {
    mesh: Mesh,
    poses: Vec<Pose>,
    top_triangles: usize,
}

impl Part {
    fn single(mesh: Mesh, pose: Pose) -> Self {
        Self {
            mesh,
            poses: vec![pose],
            top_triangles: 0,
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

enum Terrain<'a> {
    Plain,
    Regional(Region<'a>),
    Earth(world::World<'a>),
}

impl Terrain<'_> {
    fn sample(&self, direction: Vec3) -> Ground {
        match self {
            Self::Plain => Ground::NEUTRAL,
            Self::Regional(region) => region.sample(direction),
            Self::Earth(world) => world.sample(direction),
        }
    }
}

struct Layout {
    faces: Vec<Face>,
    selected: Vec<usize>,
    fixture: bool,
}

impl Layout {
    fn new(spec: &GlobePrototypeSpec) -> Result<Self> {
        spec.validate()?;
        let faces = spec.layout()?;
        let fixture =
            spec.batch.is_none() && !spec.full_shell && spec.source != GlobeSource::WholeEarth;
        let selected = if let Some(batch) = spec.batch {
            ensure!(
                batch.first_tile <= faces.len(),
                "first tile exceeds the {}-tile globe layout",
                faces.len()
            );
            let start = batch.first_tile - 1;
            (start..(start + batch.tile_count).min(faces.len())).collect()
        } else if fixture {
            let mut neighbors: Vec<_> = (1..faces.len())
                .filter(|&i| faces[0].adjacent(&faces[i]))
                .take(2)
                .collect();
            neighbors.push(0);
            neighbors
        } else {
            ensure!(
                faces.len() <= MAX_BATCH_TILES,
                "this globe has {} tiles; choose a numbered print batch to limit memory",
                faces.len()
            );
            (0..faces.len()).collect()
        };
        Ok(Self {
            faces,
            selected,
            fixture,
        })
    }
}

#[derive(Clone, Copy, Default)]
pub struct GlobeData<'a> {
    pub height: Option<&'a HeightField>,
    pub cover: Option<&'a SurfaceField>,
    pub earth_detail: Option<&'a EarthDetail>,
}

struct GlobeBuild {
    parts: Vec<Part>,
    order: Vec<usize>,
    layout_size: usize,
    landmarks: Vec<landmarks::Placement>,
}

#[cfg(test)]
fn globe_parts(
    spec: &GlobePrototypeSpec,
    data: GlobeData,
) -> Result<(Vec<Part>, Vec<usize>, usize)> {
    let build = build_parts(spec, data, &[], |_| Ok(()))?;
    Ok((build.parts, build.order, build.layout_size))
}

fn build_parts(
    spec: &GlobePrototypeSpec,
    data: GlobeData,
    landmarks: &[GlobeLandmark],
    mut on_progress: impl FnMut(f32) -> Result<()>,
) -> Result<GlobeBuild> {
    let layout = Layout::new(spec)?;
    let mapping_center = Face::all(spec.diameter_mm / 2.0, BACKING_MM, 1);
    let terrain = match spec.source {
        GlobeSource::FitTest => Terrain::Plain,
        GlobeSource::WholeEarth => {
            ensure!(
                !spec.whole_earth_detail || data.earth_detail.is_some(),
                "missing detailed Earth elevation for this batch"
            );
            Terrain::Earth(world::World::new(
                spec.earth_grid,
                spec.relief_mm,
                data.earth_detail,
            )?)
        }
        GlobeSource::SmallWorld => Terrain::Regional(Region::new(
            data.height
                .context("Small World requires a regional elevation field")?,
            data.cover,
            &mapping_center[0],
            spec.wrap_degrees,
            spec.relief_mm,
            spec.full_wrap,
        )),
    };
    let mut placements = if spec.source == GlobeSource::WholeEarth {
        landmarks::place(landmarks, &layout.faces, spec)
    } else {
        Vec::new()
    };
    let mut parts = Vec::new();
    let (clip, clip_in_face) = parts::build_clip()?;
    let mut clip_poses = Vec::new();
    for (position, &index) in layout.selected.iter().enumerate() {
        on_progress(position as f32 / layout.selected.len() as f32)?;
        let face = &layout.faces[index];
        let neighbors: Vec<_> = layout
            .faces
            .iter()
            .filter(|candidate| {
                face.adjacent(candidate)
                    && (!layout.fixture || layout.selected.contains(&(candidate.id - 1)))
            })
            .collect();
        let edges: Vec<_> = neighbors
            .iter()
            .filter_map(|other| face.edge_with(other))
            .collect();
        let mut sites = placements
            .iter_mut()
            .filter(|placement| placement.tile == face.id)
            .collect::<Vec<_>>();
        let tile = parts::build_tile(face, spec, &terrain, layout.faces.len(), &mut sites)?;
        verify_tile_insertion(face, &tile.mesh)?;
        parts.push(Part {
            mesh: tile.mesh,
            poses: vec![face.pose],
            top_triangles: tile.top_triangles,
        });
        parts.extend(
            tile.inserts
                .into_iter()
                .map(|insert| Part::single(insert.mesh, insert.pose)),
        );
        let (frame, pose) = parts::build_frame(face, &edges, spec)?;
        parts.push(Part::single(frame, pose));
        let origin = face.pose.to_globe(clip_in_face.origin);
        clip_poses.push(Pose::new(
            origin,
            face.pose.rotate(clip_in_face.x_axis),
            face.pose.rotate(clip_in_face.z_axis),
        ));
        // The lower tile ID owns each joint, including those that connect to
        // a seat printed in a different job. Frame bolt holes use every neighbor.
        for other in neighbors.into_iter().filter(|other| face.id < other.id) {
            let (joint, pose) = parts::build_bracket(face, other, spec)?;
            parts.push(Part::single(joint, pose));
        }
    }
    parts.push(Part {
        mesh: clip,
        poses: clip_poses,
        top_triangles: 0,
    });
    on_progress(1.0)?;
    Ok(GlobeBuild {
        parts,
        order: layout
            .selected
            .iter()
            .map(|i| layout.faces[*i].id)
            .collect(),
        layout_size: layout.faces.len(),
        landmarks: placements,
    })
}

fn verify_tile_insertion(face: &Face, tile: &Mesh) -> Result<()> {
    // The neighboring tiles sit in other convex cones. Show that translating
    // this whole tile outward along its mounting axis can never cross a seam.
    let rays = face
        .triangle
        .map(|[x, y]| face.pose.to_globe([x, y, BACKING_MM * face.depth_scale()]));
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
    let build = build_parts(spec, GlobeData::default(), &[], |_| Ok(()))?;
    write_parts(spec, &GenerationSpec::default(), output, &build, |_| Ok(()))?;
    Ok(())
}

fn write_parts(
    spec: &GlobePrototypeSpec,
    generation: &GenerationSpec,
    output: &Path,
    build: &GlobeBuild,
    mut on_progress: impl FnMut(f32) -> Result<()>,
) -> Result<Vec<Artifact>> {
    let GlobeBuild {
        parts,
        order: install_order,
        layout_size,
        landmarks,
    } = build;
    let layout_size = *layout_size;
    let included = landmarks
        .iter()
        .filter(|site| install_order.contains(&site.tile))
        .collect::<Vec<_>>();
    fs::create_dir_all(output).with_context(|| format!("create {}", output.display()))?;
    ensure!(
        output.read_dir()?.next().is_none(),
        "prototype output directory must be empty"
    );
    let mut entries = Vec::new();
    let print_spec = print_spec(generation, spec);
    let plain_print = GenerationSpec::default();
    let mut palette = [false; MATERIAL_SLOTS];
    for class in CLASSES {
        palette[class.material_index() as usize] = true;
    }
    let palette = PaintedClasses::Exact(palette);
    let mut artifacts = Vec::new();
    for (index, part) in parts.iter().enumerate() {
        on_progress(index as f32 / parts.len() as f32)?;
        let print_bounds_mm = validate_part(&part.mesh)?;
        ensure!(
            (0..2).all(|axis| print_bounds_mm[axis + 3] - print_bounds_mm[axis] <= spec.bed_mm),
            "{} exceeds the selected {} mm print bed",
            part.mesh.name,
            spec.bed_mm
        );
        let stl = format!("{}.stl", part.mesh.name);
        let three_mf = format!("{}.3mf", part.mesh.name);
        let stl_path = output.join(&stl);
        let three_mf_path = output.join(&three_mf);
        write_binary_stl(&part.mesh, &stl_path)?;
        let mut writer = if part.mesh.name.starts_with("landmark-") {
            ThreeMfWriter::new(
                &plain_print,
                PaintedClasses::of_mesh(&part.mesh),
                &three_mf_path,
            )?
        } else {
            ThreeMfWriter::new(&print_spec, palette, &three_mf_path)?
        };
        writer.write_mesh(&part.mesh)?;
        writer.finish()?;
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
    let max_depth_scale = if layout_size > 20 {
        1.0 / layout::ICOSAHEDRON_CHORD_RATIO
    } else {
        1.0
    };
    let screw_length = parts::screw_length(spec, max_depth_scale);
    let manifest = serde_json::json!({
        "prototype": spec,
        "diameter_mm": spec.diameter_mm,
        "install_order": install_order,
        "layout_tile_count": layout_size,
        "joint_owner": "lower tile ID",
        "parts": entries,
        "landmarks": included,
        "joint_hardware_per_bracket": format!("4 low-profile M3 × {screw_length} mm screws and 4 matching nuts"),
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
    let batch_note = if spec.batch.is_some() {
        format!(
            "This kit covers tiles {order} of {layout_size}. Keep the same settings across batches. Each joint is included with the lower of the two tile IDs. Collect all frame modules and joint brackets before assembling the globe.\n\n"
        )
    } else {
        String::new()
    };
    let landmark_note = if included.is_empty() {
        ""
    } else {
        "See LANDMARKS.md for the numbered inserts and their tile locations.\n\n"
    };
    let guide = format!(
        "# TopoSaic globe print kit\n\n\
         {batch_note}Print one copy of each tile, frame module, and joint bracket, plus {} copies of the snap clip. \
         Choose either the STL files or the 3MF files. Mark the frame and tile IDs after printing. \
         Use assembly.obj to inspect the assembled positions.\n\n\
         1. Join the frame modules with the numbered brackets on the inside. \
         Each bracket uses four low-profile M3 × {screw_length} mm screws and four M3 nuts. \
         Insert the screws from the recessed side of the frame.\n\
         2. Press a clip into each frame seat from the outside until its collar sits against the frame.\n\
         3. Match each tile to its seat and align the locator before pressing it straight on. \
         Install the tiles in this order. {order}.\n\n\
         {landmark_note}Record seam gaps, seating repeatability, insertion and removal force, and clip wear.\n",
        install_order.len(),
    );
    fs::write(output.join("ASSEMBLY.md"), guide)?;
    if !included.is_empty() {
        let path = output.join("LANDMARKS.md");
        let mut guide = String::from(
            "# Globe landmark key\n\nPrint each insert separately in your choice of filament. Match the clipped corner of its peg to the socket, then press the insert in from the outside.\n\n",
        );
        for site in included {
            use std::fmt::Write as _;
            let name = site
                .landmark
                .name
                .replace(['<', '>', '[', ']', '`', '*'], "");
            writeln!(
                guide,
                "- **{:02} · {}** on tile {:02} · site {:.4}°, {:.4}°",
                site.landmark.id, name, site.tile, site.landmark.latitude, site.landmark.longitude
            )?;
            if let Some(seat) = &site.seat {
                writeln!(
                    guide,
                    "  Seat {:.4}°, {:.4}° · {:.1} mm from the site",
                    seat.latitude, seat.longitude, seat.offset_mm
                )?;
            } else {
                writeln!(
                    guide,
                    "  No insert in this kit. The socket could not clear the seam or frame attachment."
                )?;
            }
        }
        fs::write(&path, guide)?;
        artifacts.push(file_artifact(&path, "text/markdown")?);
    }
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
    generation: &GenerationSpec,
    data: GlobeData,
    on_progress: impl FnMut(f32) -> Result<()>,
) -> Result<serde_json::Value> {
    let spec = generation
        .globe
        .context("globe preview requires globe settings")?;
    let count = Layout::new(&spec)?.selected.len();
    let draft = GlobePrototypeSpec {
        samples_per_edge: spec.preview_samples(count),
        ..spec
    };
    let build = build_parts(&draft, data, &generation.globe_landmarks, on_progress)?;
    let mut preview = preview_with_parts(&build, &print_spec(generation, &spec), draft)?;
    if spec.batch.is_some() {
        // The overview needs only three nominal points per seat, even for a
        // large job. Actual terrain meshes stay limited to the chosen batch.
        let outline = spec
            .layout()?
            .into_iter()
            .map(|face| {
                face.triangle
                    .map(|[x, y]| face.pose.to_globe([x, y, BACKING_MM * face.depth_scale()]))
            })
            .collect::<Vec<_>>();
        preview["globe_overview"] = serde_json::to_value(outline)?;
    }
    Ok(preview)
}

fn print_spec(generation: &GenerationSpec, globe: &GlobePrototypeSpec) -> GenerationSpec {
    let mut print = GenerationSpec {
        color_output: generation.color_output.clone(),
        ..GenerationSpec::default()
    };
    print.color_output.enabled = globe.surface_colors && globe.source != GlobeSource::FitTest;
    print.color_output.neighborhood_surfaces.enabled = print.color_output.enabled;
    print.color_output.ground_palette = Default::default();
    print
}

fn preview_with_parts(
    build: &GlobeBuild,
    print: &GenerationSpec,
    draft: GlobePrototypeSpec,
) -> Result<serde_json::Value> {
    #[derive(Serialize)]
    struct PreviewPart<'a> {
        name: String,
        kind: &'static str,
        top_triangles: usize,
        outward: Vec<[f32; 3]>,
        vertices: &'a [[f32; 3]],
        triangles: &'a [[u32; 3]],
        poses: &'a [Pose],
        #[serde(skip_serializing_if = "Vec::is_empty")]
        materials: Vec<u32>,
    }
    let meshes = build
        .parts
        .iter()
        .map(|part| {
            let kind = if part.mesh.name.starts_with("tile-") {
                "tile"
            } else if part.mesh.name.starts_with("landmark-") {
                "landmark"
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
                top_triangles: part.top_triangles,
                outward: part.poses.iter().map(|pose| unit(pose.origin)).collect(),
                vertices: &part.mesh.vertices,
                triangles: &part.mesh.triangles,
                poses: &part.poses,
                materials: if print.color_output.enabled && kind == "tile" {
                    part.mesh
                        .materials
                        .iter()
                        .map(|material| material.material_index())
                        .collect()
                } else {
                    Vec::new()
                },
            }
        })
        .collect::<Vec<_>>();
    let palette = if print.color_output.enabled {
        CLASSES
            .map(|class| (class.material_index(), print.class_color(class)))
            .into_iter()
            .collect::<std::collections::BTreeMap<_, _>>()
    } else {
        Default::default()
    };
    Ok(serde_json::json!({
        "globe_palette": palette,
        "globe_surface_samples": parts::tile_segments(&draft),
        "globe_exterior": draft.exterior,
        "globe_source": draft.source,
        "globe_tiles": build.order,
        "globe_layout_size": build.layout_size,
        "globe_landmarks": build.landmarks,
        "width": 0, "height": 0, "values": [], "rows": 0, "columns": 0,
        "globe_meshes": meshes,
    }))
}

/// Publish a globe job through the same artifact and saved-setup contracts as terrain.
pub fn generate_globe_project(
    spec: &GenerationSpec,
    data: GlobeData,
    output: &Path,
    mut on_progress: impl FnMut(f32) -> Result<()>,
) -> Result<ProjectManifest> {
    spec.validate()?;
    let globe = spec
        .globe
        .context("globe project requires globe settings")?;
    on_progress(0.0)?;
    let build = build_parts(&globe, data, &spec.globe_landmarks, |fraction| {
        on_progress(0.3 * fraction)
    })?;
    let mut files = write_parts(&globe, spec, output, &build, |fraction| {
        on_progress(0.3 + 0.55 * fraction)
    })?;
    let order = build.order;
    drop(build.parts);
    let preview_path = output.join("preview.json");
    fs::write(
        &preview_path,
        serde_json::to_vec(&build_globe_preview(spec, data, |_| on_progress(0.85))?)?,
    )?;

    let terrain_source = match globe.source {
        GlobeSource::WholeEarth => {
            if globe.whole_earth_detail {
                format!(
                    "{} with NOAA five-arc-minute elevation blocks",
                    world::elevation_source(globe.earth_grid)
                )
            } else {
                world::elevation_source(globe.earth_grid)
            }
        }
        GlobeSource::SmallWorld => data
            .height
            .context("missing Small World elevation")?
            .source
            .clone(),
        GlobeSource::FitTest => "Synthetic globe fit geometry".into(),
    };
    let surface_source: Option<String> = match globe.source {
        GlobeSource::WholeEarth => Some(world::COLOR_SOURCE.into()),
        GlobeSource::SmallWorld => data.cover.map(|field| field.source.clone()),
        GlobeSource::FitTest => None,
    };
    if globe.source != GlobeSource::FitTest {
        let source_path = output.join("SOURCES.md");
        let mut sources = format!("# Globe data sources\n\n{terrain_source}\n");
        if let Some(cover) = &surface_source {
            sources.push_str(&format!("\n{cover}\n"));
        }
        fs::write(&source_path, sources)?;
        files.push(file_artifact(&source_path, "text/markdown")?);
    }

    let archive_name = if globe.batch.is_some() {
        format!(
            "globe-tiles-{:04}-{:04}.zip",
            order[0],
            order[order.len() - 1]
        )
    } else {
        "globe-print-kit.zip".into()
    };
    let archive_path = output.join(archive_name);
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
        terrain_source,
        surface_source,
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
    fn landmarks_cut_watertight_sockets_and_keep_their_tile_in_batches() {
        let [x, y, z] = Face::all(100.0, BACKING_MM, 1)[0].pose.z_axis;
        let landmarks = [
            (
                1,
                "Eiffel Tower",
                GlobeLandmarkShape::Tower,
                48.8583,
                2.2945,
                7.0,
                13.0,
            ),
            (
                2,
                "Great Pyramid of Giza",
                GlobeLandmarkShape::Pyramid,
                29.9792,
                31.1342,
                9.0,
                5.0,
            ),
            (
                3,
                "Sydney",
                GlobeLandmarkShape::CityMarker,
                -33.8672,
                151.2075,
                6.0,
                4.0,
            ),
            (
                4,
                "Frame clearance",
                GlobeLandmarkShape::CityMarker,
                f64::from(z).asin().to_degrees(),
                f64::from(y).atan2(f64::from(x)).to_degrees(),
                6.0,
                4.0,
            ),
        ]
        .map(
            |(id, name, shape, latitude, longitude, width_mm, height_mm)| GlobeLandmark {
                id,
                name: name.into(),
                shape,
                latitude,
                longitude,
                width_mm,
                height_mm,
            },
        );
        for exterior in [GlobeExterior::Faceted, GlobeExterior::Rounded] {
            let mut spec = GlobePrototypeSpec {
                source: GlobeSource::WholeEarth,
                full_shell: true,
                exterior,
                earth_grid: EarthGrid::QuarterDegree,
                samples_per_edge: 48,
                ..GlobePrototypeSpec::default()
            };
            let full = build_parts(&spec, GlobeData::default(), &landmarks, |_| Ok(())).unwrap();
            assert_eq!(full.landmarks.len(), landmarks.len());
            for placement in &full.landmarks {
                assert!(
                    placement.insert_position.is_some(),
                    "{}",
                    placement.landmark.name
                );
                let id = placement.landmark.id;
                let owner = placement.tile;
                for name in [format!("tile-{owner:02}"), format!("landmark-{id:02}")] {
                    let part = full
                        .parts
                        .iter()
                        .find(|part| part.mesh.name == name)
                        .unwrap();
                    validate_part(&part.mesh).unwrap();
                }
            }
            assert!(full.landmarks[3].seat.as_ref().unwrap().offset_mm > 0.5);

            let owner = full.landmarks[0].tile;
            spec.batch = Some(GlobeBatch {
                first_tile: owner,
                tile_count: 1,
            });
            let batch = build_parts(&spec, GlobeData::default(), &landmarks, |_| Ok(())).unwrap();
            assert_eq!(batch.order, [owner]);
            for (whole, partial) in full.landmarks.iter().zip(&batch.landmarks) {
                assert_eq!(whole.tile, partial.tile);
                assert_eq!(
                    whole.seat.as_ref().unwrap().uv,
                    partial.seat.as_ref().unwrap().uv
                );
                assert_eq!(partial.insert_position.is_some(), partial.tile == owner);
            }
            for part in batch.parts.iter().filter(|part| {
                part.mesh.name.starts_with("landmark-") || part.mesh.name.starts_with("tile-")
            }) {
                let original = full
                    .parts
                    .iter()
                    .find(|whole| whole.mesh.name == part.mesh.name)
                    .unwrap();
                assert_eq!(part.mesh.vertices, original.mesh.vertices);
                assert_eq!(part.mesh.triangles, original.mesh.triangles);
            }
        }
    }

    #[test]
    fn whole_earth_preview_keeps_print_detail_for_twenty_tiles() {
        let old: GlobePrototypeSpec = serde_json::from_str(r#"{"source":"whole_earth"}"#).unwrap();
        assert_eq!(old.earth_grid, EarthGrid::HalfDegree);
        let fine = GlobePrototypeSpec {
            earth_grid: EarthGrid::QuarterDegree,
            samples_per_edge: 96,
            ..old
        };
        assert_eq!(fine.preview_samples(20), 96);
        assert_eq!(fine.preview_samples(80), 48);
        let spec = GenerationSpec {
            globe: Some(fine),
            ..GenerationSpec::default()
        };
        let preview = build_globe_preview(&spec, GlobeData::default(), |_| Ok(())).unwrap();
        assert_eq!(preview["globe_surface_samples"], 96);
        assert_eq!(preview["globe_layout_size"], 20);
    }

    #[test]
    fn fixture_contains_two_joints_and_installs_the_middle_tile_last() {
        let (parts, order, _) =
            globe_parts(&GlobePrototypeSpec::default(), GlobeData::default()).unwrap();
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
            let (parts, order, _) = globe_parts(&spec, GlobeData::default()).unwrap();
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
        let (blank_parts, _, _) = globe_parts(&blank, GlobeData::default()).unwrap();
        let spec = GlobePrototypeSpec {
            source: GlobeSource::SmallWorld,
            ..blank
        };
        assert!(globe_parts(&spec, GlobeData::default()).is_err());
        let (mapped, _, _) = globe_parts(
            &spec,
            GlobeData {
                height: Some(&field),
                ..GlobeData::default()
            },
        )
        .unwrap();
        for (mapped, blank) in mapped.iter().zip(&blank_parts) {
            validate_part(&mapped.mesh).unwrap();
            if mapped.mesh.name.starts_with("tile-") {
                assert!(mapped.mesh.vertices.len() > blank.mesh.vertices.len());
            } else {
                assert_eq!(mapped.mesh.vertices, blank.mesh.vertices);
            }
        }
        let faces = Face::all(REFERENCE_DIAMETER_MM / 2.0, BACKING_MM, 1);
        let region = Region::new(
            &field,
            None,
            &faces[0],
            spec.wrap_degrees,
            spec.relief_mm,
            false,
        );
        assert!((region.sample(faces[0].pose.z_axis).height - spec.relief_mm).abs() < 1.0e-5);
        assert_eq!(
            region
                .sample(layout::scale(faces[0].pose.z_axis, -1.0))
                .height,
            0.0
        );
    }

    #[test]
    fn superglobe_batches_keep_frame_ids_and_assign_border_joints_once() {
        let mut spec = GlobePrototypeSpec {
            diameter_mm: 1_800.0,
            subdivision: 8,
            batch: Some(GlobeBatch {
                first_tile: 1,
                tile_count: 2,
            }),
            ..GlobePrototypeSpec::default()
        };
        let (first, order, size) = globe_parts(&spec, GlobeData::default()).unwrap();
        assert_eq!((order, size), (vec![1, 2], 1280));
        spec.batch = Some(GlobeBatch {
            first_tile: 2,
            tile_count: 2,
        });
        let (second, order, size) = globe_parts(&spec, GlobeData::default()).unwrap();
        assert_eq!((order, size), (vec![2, 3], 1280));
        let shared = |parts: &[Part]| {
            parts
                .iter()
                .find(|part| part.mesh.name == "frame-02")
                .unwrap()
                .mesh
                .vertices
                .clone()
        };
        assert_eq!(shared(&first), shared(&second));
        assert!(
            first
                .iter()
                .any(|part| part.mesh.name.starts_with("joint-01-"))
        );
        assert!(
            second
                .iter()
                .all(|part| !part.mesh.name.starts_with("joint-01-"))
        );
        for part in first.iter().chain(&second) {
            let bounds = validate_part(&part.mesh).unwrap();
            assert!(bounds[3] - bounds[0] <= spec.bed_mm && bounds[4] - bounds[1] <= spec.bed_mm);
        }
    }

    #[test]
    fn thick_frame_still_clears_the_clip_and_print_bed_at_both_scales() {
        let fixture = GlobePrototypeSpec {
            diameter_mm: 160.0,
            frame_depth_mm: 12.0,
            ..GlobePrototypeSpec::default()
        };
        let high_order = GlobePrototypeSpec {
            diameter_mm: 9_000.0,
            subdivision: 20,
            bed_mm: 420.0,
            batch: Some(GlobeBatch {
                first_tile: 4132,
                tile_count: 2,
            }),
            ..fixture
        };
        for spec in [fixture, high_order] {
            let (parts, _, _) = globe_parts(&spec, GlobeData::default()).unwrap();
            for part in parts {
                let bounds = validate_part(&part.mesh).unwrap();
                assert!(
                    bounds[3] - bounds[0] <= spec.bed_mm && bounds[4] - bounds[1] <= spec.bed_mm
                );
            }
        }
    }

    #[test]
    fn earth_colors_use_bundled_geography_and_full_wrap_matches_at_the_join() {
        let earth = world::World::new(EarthGrid::QuarterDegree, 8.0, None).unwrap();
        let direction = |latitude: f32, longitude: f32| {
            let (sin_lat, cos_lat) = latitude.to_radians().sin_cos();
            let (sin_lon, cos_lon) = longitude.to_radians().sin_cos();
            [cos_lon * cos_lat, sin_lon * cos_lat, sin_lat]
        };
        assert_eq!(
            earth.sample(direction(5.0, -125.0)).class(),
            crate::SurfaceClass::Water
        );
        assert_eq!(
            earth.sample(direction(-82.0, 25.0)).class(),
            crate::SurfaceClass::Snow
        );
        assert_eq!(
            earth.sample(direction(-8.0, -62.0)).class(),
            crate::SurfaceClass::Forest
        );
        let field = HeightField::new(2, 2, vec![100.0, 400.0, 0.0, 200.0], "test slope").unwrap();
        let faces = Face::all(100.0, BACKING_MM, 1);
        let pose = faces[0].pose;
        let region = Region::new(&field, None, &faces[0], 110.0, 8.0, true);
        let west = unit(layout::add(
            layout::scale(pose.z_axis, -1.0),
            layout::scale(pose.x_axis, -0.0001),
        ));
        let east = unit(layout::add(
            layout::scale(pose.z_axis, -1.0),
            layout::scale(pose.x_axis, 0.0001),
        ));
        assert!((region.sample(west).height - region.sample(east).height).abs() < 0.01);
    }
}
