//! Printable shell and snap-fit frame prototype.

mod detail;
mod ground;
mod landmarks;
mod layout;
mod parts;
mod world;
mod wrap;

use std::fs::{self, File};
use std::io::{self, BufWriter, Write};
use std::path::Path;

use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};
use zip::{ZipWriter, write::SimpleFileOptions};

use self::ground::{CLASSES, Ground};
use self::layout::{Face, Pose, Vec3, cross, dot, unit};
use self::wrap::{MapWrap, TilePlan, WrappedMap};
use crate::analysis::analyze_mesh_views;
use crate::export::{ThreeMfWriter, write_binary_stl};
use crate::mesh::Mesh;
use crate::project::{Artifact, ProjectManifest, file_artifact};
use crate::spec::{GenerationSpec, MATERIAL_SLOTS, PaintedClasses, PrintMaterial};
use crate::{HeightField, SurfaceField};

pub use landmarks::{GlobeLandmark, GlobeLandmarkShape};
pub use wrap::{BAND_DEGREES, small_world_map_spec};

pub use detail::{EARTH_DETAIL_BLOCK_SIDE, EarthDetail, EarthDetailBlock, earth_detail_blocks};

const REFERENCE_DIAMETER_MM: f32 = 200.0;
const BACKING_MM: f32 = 8.0;
const MAX_SUBDIVISION: u8 = 24;
const MAX_BATCH_TILES: usize = 80;
/// Terrain triangles one globe job may hold in memory.
const JOB_TRIANGLE_BUDGET: f32 = 1_500_000.0;

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
    /// Small World wraps 360° around the globe as a band of latitude
    /// instead of a regional cap.
    pub full_wrap: bool,
    /// Latitude span of the 360° band, from [`BAND_DEGREES`].
    pub band_degrees: f32,
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
    /// Kept so older saved setups still load. Every unbatched globe is now
    /// the full shell; the three-tile fixture is gone.
    pub full_shell: bool,
    /// Combined inset from the two tiles, measured in their back planes.
    pub seam_gap_mm: f32,
    /// Play between each dovetail key and its slots, per side.
    pub key_clearance_mm: f32,
    /// Add screwed brackets under each frame joint, behind the dovetail key.
    pub screw_brackets: bool,
    /// Screw each tile to its frame module from inside the globe.
    pub tile_screws: bool,
    /// The heat-set inserts the brackets and tiles take.
    pub insert: HeatSetInsert,
}

/// A heat-set threaded insert and the screw that goes into it.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct HeatSetInsert {
    /// Nominal thread diameter: 3 for M3.
    pub screw_mm: f32,
    /// The hole the insert is pressed into.
    pub hole_diameter_mm: f32,
    pub length_mm: f32,
}

impl Default for HeatSetInsert {
    fn default() -> Self {
        Self {
            screw_mm: 3.0,
            hole_diameter_mm: 4.0,
            length_mm: 4.0,
        }
    }
}

impl HeatSetInsert {
    fn validate(self) -> Result<()> {
        ensure!(
            (2.0..=5.0).contains(&self.screw_mm),
            "heat-set insert screws must be between M2 and M5"
        );
        ensure!(
            self.hole_diameter_mm > self.screw_mm + 0.3
                && self.hole_diameter_mm <= self.screw_mm + 3.5,
            "heat-set insert holes must be wider than the screw and at most 3.5 mm wider"
        );
        ensure!(
            (2.5..=6.5).contains(&self.length_mm),
            "heat-set inserts must be between 2.5 and 6.5 mm long"
        );
        Ok(())
    }
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
        if self.full_wrap {
            wrap::validate_band(self.band_degrees)?;
        }
        ensure!(
            (0.05..=0.4).contains(&self.key_clearance_mm),
            "dovetail key clearance must be between 0.05 and 0.40 mm per side"
        );
        if self.screw_brackets || self.tile_screws {
            self.insert.validate()?;
        }
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

    /// The most tiles one job may build. Mapped tiles carry far more
    /// geometry than plain ones, so they get fewer per job; a larger globe
    /// prints in numbered batches.
    pub fn tile_limit(self) -> usize {
        let samples = f32::from(self.samples_per_edge);
        let per_tile = match self.source {
            GlobeSource::FitTest => 2.0 * 24.0 * 24.0,
            GlobeSource::WholeEarth => samples * samples,
            // Flat map triangles (three per lattice step along an edge),
            // doubled for roads, buildings, and other overlays.
            GlobeSource::SmallWorld => {
                2.0 * EXPORT_MAP_DETAIL * EXPORT_MAP_DETAIL * samples * samples
            }
        };
        ((JOB_TRIANGLE_BUDGET / per_tile) as usize).clamp(1, MAX_BATCH_TILES)
    }

    fn preview_samples(self, tiles: usize) -> u8 {
        // Keep the original detail for a normal globe or a small batch.
        // For a large batch, bound the total top-surface triangles sent to Studio.
        const PREVIEW_TRIANGLES: f32 = 20.0 * 96.0 * 96.0;
        self.samples_per_edge
            .min((PREVIEW_TRIANGLES / tiles.max(1) as f32).sqrt() as u8)
    }

    /// Check that the tiles fit the bed and their seats fit the hardware.
    /// A globe too large for one job still previews as a layout.
    pub fn validate_layout(self) -> Result<()> {
        self.validate()?;
        self.layout().map(|_| ())
    }

    /// Whether the unbatched globe has more tiles than one job may build.
    pub fn needs_batch(self) -> Result<bool> {
        Ok(self.batch.is_none() && self.layout()?.len() > self.tile_limit())
    }

    /// Check everything a generation job needs, including that its tiles
    /// fit in one job.
    pub fn validate_job(self) -> Result<()> {
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
            band_degrees: 120.0,
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
            full_shell: true,
            seam_gap_mm: 0.4,
            key_clearance_mm: 0.15,
            screw_brackets: false,
            tile_screws: false,
            insert: HeatSetInsert::default(),
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
    Map(WrappedMap<'a>),
    Earth(world::World<'a>),
}

impl Terrain<'_> {
    fn sample(&self, direction: Vec3) -> Ground {
        match self {
            Self::Plain => Ground::NEUTRAL,
            Self::Map(map) => map.sample(direction),
            Self::Earth(world) => world.sample(direction),
        }
    }

    fn material(&self, direction: Vec3) -> PrintMaterial {
        match self {
            Self::Map(map) => map.material(direction),
            _ => self.sample(direction).class().into(),
        }
    }
}

struct Layout {
    faces: Vec<Face>,
    selected: Vec<usize>,
}

impl Layout {
    fn new(spec: &GlobePrototypeSpec) -> Result<Self> {
        spec.validate()?;
        let faces = spec.layout()?;
        let limit = spec.tile_limit();
        let selected = if let Some(batch) = spec.batch {
            ensure!(
                batch.first_tile <= faces.len(),
                "first tile exceeds the {}-tile globe layout",
                faces.len()
            );
            ensure!(
                batch.tile_count <= limit,
                "batches of this globe hold at most {limit} tiles at this mesh detail; choose fewer tiles per batch"
            );
            let start = batch.first_tile - 1;
            (start..(start + batch.tile_count).min(faces.len())).collect()
        } else {
            ensure!(
                faces.len() <= limit,
                "this globe has {} tiles, more than one job can hold at this mesh detail; choose a numbered tile batch of up to {limit} tiles",
                faces.len()
            );
            (0..faces.len()).collect()
        };
        Ok(Self { faces, selected })
    }
}

#[derive(Clone, Copy, Default)]
pub struct GlobeData<'a> {
    /// The flat map a Small World globe is cut from, from
    /// [`small_world_map_spec`] with its ground palette locked.
    pub map_spec: Option<&'a GenerationSpec>,
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
    let build = build_parts(spec, data, &[], EXPORT_MAP_DETAIL, |_| Ok(()))?;
    Ok((build.parts, build.order, build.layout_size))
}

/// Flat map triangles per lattice sample along a tile edge.
const EXPORT_MAP_DETAIL: f32 = 3.0;
const PREVIEW_MAP_DETAIL: f32 = 1.5;

fn build_parts(
    spec: &GlobePrototypeSpec,
    data: GlobeData,
    landmarks: &[GlobeLandmark],
    map_detail: f32,
    mut on_progress: impl FnMut(f32) -> Result<()>,
) -> Result<GlobeBuild> {
    // Small World colors and print detail come from the map's own settings.
    let adjusted = match (spec.source, data.map_spec) {
        (GlobeSource::SmallWorld, Some(map)) => GlobePrototypeSpec {
            surface_colors: map.color_output.enabled,
            texture_mm: 0.0,
            ..*spec
        },
        _ => *spec,
    };
    let spec = &adjusted;
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
        GlobeSource::SmallWorld => {
            let map = data
                .map_spec
                .context("Small World requires its map settings")?;
            let height = data
                .height
                .context("Small World requires a regional elevation field")?;
            let wrap = MapWrap::new(mapping_center[0].pose, spec.diameter_mm / 2.0, map, spec);
            Terrain::Map(WrappedMap::new(wrap, map, height, data.cover))
        }
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
            .filter(|candidate| face.adjacent(candidate))
            .collect();
        let edges: Vec<_> = neighbors
            .iter()
            .filter_map(|other| face.edge_with(other))
            .collect();
        let mut sites = placements
            .iter_mut()
            .filter(|placement| placement.tile == face.id)
            .collect::<Vec<_>>();
        let tile = match &terrain {
            Terrain::Map(map) => match map.plan(face, spec.seam_gap_mm) {
                TilePlan::Map(outline) => {
                    let spacing = (face.edge_width()
                        / (f32::from(spec.samples_per_edge) * map_detail))
                        .clamp(0.25, 4.0);
                    parts::build_map_tile(face, spec, map, &outline, spacing)
                        .with_context(|| format!("build Small World tile {:02}", face.id))?
                }
                TilePlan::Sampled => {
                    parts::build_tile(face, spec, &terrain, layout.faces.len(), &mut sites)?
                }
            },
            _ => parts::build_tile(face, spec, &terrain, layout.faces.len(), &mut sites)?,
        };
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
        // a seat printed in a different job. Every module has a key slot, and
        // bolt holes when brackets are on, for each neighbor.
        for other in neighbors.into_iter().filter(|other| face.id < other.id) {
            let (key, pose) = parts::build_key(face, other, spec)?;
            parts.push(Part::single(key, pose));
            if spec.screw_brackets {
                let (joint, pose) = parts::build_bracket(face, other, spec)?;
                parts.push(Part::single(joint, pose));
            }
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
    let build = build_parts(spec, GlobeData::default(), &[], EXPORT_MAP_DETAIL, |_| {
        Ok(())
    })?;
    write_parts(
        spec,
        &GenerationSpec::default(),
        None,
        output,
        &build,
        |_| Ok(()),
    )?;
    Ok(())
}

fn write_parts(
    spec: &GlobePrototypeSpec,
    generation: &GenerationSpec,
    map: Option<&GenerationSpec>,
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
    let print_spec = print_spec(generation, spec, map);
    let exact_palette = map.is_some();
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
        } else if exact_palette {
            ThreeMfWriter::new(
                &print_spec,
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
    let count = |prefix: &str| {
        parts
            .iter()
            .filter(|part| part.mesh.name.starts_with(prefix))
            .count()
    };
    let (keys, brackets) = (count("key-"), count("joint-"));
    let thread = {
        let size = spec.insert.screw_mm;
        if size.fract() == 0.0 {
            format!("M{size:.0}")
        } else {
            format!("M{size}")
        }
    };
    let insert = format!(
        "{thread} heat-set inserts ({:.1} mm hole, {:.1} mm long)",
        spec.insert.hole_diameter_mm, spec.insert.length_mm
    );
    let bracket_screw = parts::bracket_screw_length(spec, max_depth_scale);
    let tile_screw = parts::tile_screw_length(spec, max_depth_scale);
    let mut hardware = Vec::new();
    if brackets > 0 {
        hardware.push(format!(
            "{} {insert} and {} {thread} × {bracket_screw} mm screws for the brackets",
            4 * brackets,
            4 * brackets
        ));
    }
    if spec.tile_screws {
        hardware.push(format!(
            "{} {insert} and {} {thread} × {tile_screw} mm screws for the tiles",
            install_order.len(),
            install_order.len()
        ));
    }
    let manifest = serde_json::json!({
        "prototype": spec,
        "diameter_mm": spec.diameter_mm,
        "install_order": install_order,
        "layout_tile_count": layout_size,
        "joint_owner": "lower tile ID",
        "parts": entries,
        "landmarks": included,
        "dovetail_keys": keys,
        "screw_brackets": brackets,
        "hardware": hardware,
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
            "This kit covers tiles {order} of {layout_size}. Keep the same settings across batches. Each joint's key and bracket come with the lower of the two tile IDs. Collect every frame module, key, and bracket before you assemble the globe.\n\n"
        )
    } else {
        String::new()
    };
    let landmark_note = if included.is_empty() {
        ""
    } else {
        "See LANDMARKS.md for the numbered inserts and their tile locations.\n\n"
    };
    let hardware_note = if hardware.is_empty() {
        "No screws are needed.\n\n".to_owned()
    } else {
        format!("Hardware: {}.\n\n", hardware.join("; "))
    };
    let bracket_step = if brackets > 0 {
        format!(
            "2. Press the heat-set inserts into the tops of the brackets with a soldering iron. Hold each bracket under its joint and drive the {thread} × {bracket_screw} mm screws down through the recessed holes in the frame modules.\n"
        )
    } else {
        String::new()
    };
    let tile_step = if spec.tile_screws {
        format!(
            " Press an insert into the hole in each tile's back first. As you seat each tile, drive a {thread} × {tile_screw} mm screw up through the counterbore under its frame module. The last few tiles cannot be reached from inside, so their clips hold them."
        )
    } else {
        String::new()
    };
    let clip_step = if brackets > 0 { 3 } else { 2 };
    let guide = format!(
        "# TopoSaic globe print kit\n\n\
         {batch_note}Print one copy of each tile, frame module, dovetail key{}, plus {} copies of the snap clip. \
         Print the keys standing on their flat ends. \
         Choose either the STL files or the 3MF files. Mark the frame and tile IDs after printing. \
         Use assembly.obj to inspect the assembled positions.\n\n\
         {hardware_note}\
         1. Lay two neighbouring frame modules edge to edge and line up the slots near their shared edge. \
         Drop the matching key into the open ends of both slots, then slide it along the edge until it stops. \
         The dovetails lock the modules together. Join every module this way from the outside.\n\
         {bracket_step}\
         {clip_step}. Press a clip into each frame seat from the outside until its collar sits against the frame.\n\
         {}. Match each tile to its seat and align the locator before pressing it straight on.{tile_step} \
         Install the tiles in this order. {order}.\n\n\
         {landmark_note}Record seam gaps, key fit, seating repeatability, insertion and removal force, and clip wear.\n",
        if brackets > 0 {
            ", and joint bracket"
        } else {
            ""
        },
        install_order.len(),
        clip_step + 1,
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
    let overview = |spec: &GlobePrototypeSpec| -> Result<serde_json::Value> {
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
        Ok(serde_json::to_value(outline)?)
    };
    if spec.batch.is_none() {
        spec.validate()?;
        let size = spec.layout()?.len();
        if spec.needs_batch()? {
            // Too large for one job: show where the tiles go and ask for a batch.
            return Ok(serde_json::json!({
                "globe_batch_required": true,
                "globe_batch_limit": spec.tile_limit(),
                "globe_layout_size": size,
                "globe_overview": overview(&spec)?,
                "globe_source": spec.source,
                "globe_exterior": spec.exterior,
                "globe_tiles": [],
                "globe_meshes": [],
                "width": 0, "height": 0, "values": [], "rows": 0, "columns": 0,
            }));
        }
    }
    let count = Layout::new(&spec)?.selected.len();
    let draft = GlobePrototypeSpec {
        samples_per_edge: spec.preview_samples(count),
        ..spec
    };
    let build = build_parts(
        &draft,
        data,
        &generation.globe_landmarks,
        PREVIEW_MAP_DETAIL,
        on_progress,
    )?;
    let print = print_spec(generation, &spec, data.map_spec);
    let mut preview = preview_with_parts(&build, &print, data.map_spec.is_some(), draft)?;
    preview["globe_batch_limit"] = spec.tile_limit().into();
    if spec.batch.is_some() {
        preview["globe_overview"] = overview(&spec)?;
    }
    Ok(preview)
}

fn print_spec(
    generation: &GenerationSpec,
    globe: &GlobePrototypeSpec,
    map: Option<&GenerationSpec>,
) -> GenerationSpec {
    if globe.source == GlobeSource::SmallWorld
        && let Some(map) = map
    {
        // A Small World prints every color its map can carry.
        return map.clone();
    }
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
    mapped_palette: bool,
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
            } else if part.mesh.name.starts_with("key-") {
                "key"
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
    let palette = if print.color_output.enabled && mapped_palette {
        // A Small World tile can carry any map color: list the ones in use.
        let ground = print.locked_ground_colors();
        let mut palette = std::collections::BTreeMap::new();
        for part in build
            .parts
            .iter()
            .filter(|part| part.mesh.name.starts_with("tile-"))
        {
            for material in &part.mesh.materials {
                let color = match *material {
                    PrintMaterial::Class(class) => Some(print.class_color(class)),
                    PrintMaterial::Ground(entry) => {
                        ground.get(usize::from(entry)).map(String::as_str)
                    }
                };
                if let Some(color) = color {
                    palette
                        .entry(material.material_index())
                        .or_insert_with(|| color.to_owned());
                }
            }
        }
        palette
    } else if print.color_output.enabled {
        CLASSES
            .map(|class| (class.material_index(), print.class_color(class).to_owned()))
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
    let build = build_parts(
        &globe,
        data,
        &spec.globe_landmarks,
        EXPORT_MAP_DETAIL,
        |fraction| on_progress(0.3 * fraction),
    )?;
    let mut files = write_parts(&globe, spec, data.map_spec, output, &build, |fraction| {
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
            let full = build_parts(
                &spec,
                GlobeData::default(),
                &landmarks,
                EXPORT_MAP_DETAIL,
                |_| Ok(()),
            )
            .unwrap();
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
            let batch = build_parts(
                &spec,
                GlobeData::default(),
                &landmarks,
                EXPORT_MAP_DETAIL,
                |_| Ok(()),
            )
            .unwrap();
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
    fn large_mapped_globes_require_batches_within_the_tile_limit() {
        let mapped = GlobePrototypeSpec {
            source: GlobeSource::SmallWorld,
            diameter_mm: 1_000.0,
            subdivision: 4,
            ..GlobePrototypeSpec::default()
        };
        let limit = mapped.tile_limit();
        assert!((20..320).contains(&limit));
        let error = Layout::new(&mapped).err().unwrap().to_string();
        assert!(error.contains("numbered tile batch"), "{error}");
        let too_many = GlobePrototypeSpec {
            batch: Some(GlobeBatch {
                first_tile: 1,
                tile_count: limit + 1,
            }),
            ..mapped
        };
        assert!(Layout::new(&too_many).is_err());
        let within = GlobePrototypeSpec {
            batch: Some(GlobeBatch {
                first_tile: 1,
                tile_count: limit,
            }),
            ..mapped
        };
        assert_eq!(Layout::new(&within).unwrap().selected.len(), limit);
        let generation = GenerationSpec {
            globe: Some(mapped),
            ..GenerationSpec::default()
        };
        let preview = build_globe_preview(&generation, GlobeData::default(), |_| Ok(())).unwrap();
        assert_eq!(preview["globe_batch_required"], true);
        assert_eq!(preview["globe_layout_size"], 320);
        assert_eq!(preview["globe_overview"].as_array().unwrap().len(), 320);
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
        let generation = GenerationSpec {
            globe: Some(spec),
            ..GenerationSpec::default()
        };
        let map = small_world_map_spec(&generation).unwrap();
        assert!(globe_parts(&spec, GlobeData::default()).is_err());
        let (mapped, _, _) = globe_parts(
            &spec,
            GlobeData {
                map_spec: Some(&map),
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
        let wrap = MapWrap::new(faces[0].pose, REFERENCE_DIAMETER_MM / 2.0, &map, &spec);
        let region = WrappedMap::new(wrap, &map, &field, None);
        assert!((region.sample(faces[0].pose.z_axis).height - spec.relief_mm).abs() < 1.0e-5);
        assert_eq!(
            region
                .sample(layout::scale(faces[0].pose.z_axis, -1.0))
                .height,
            0.0
        );
    }

    #[test]
    fn small_world_tiles_carry_map_roads_and_close_watertight() {
        let field = HeightField::new(
            5,
            5,
            (0..25).map(|i| ((i * 37) % 11) as f32 * 90.0).collect(),
            "test hills",
        )
        .unwrap();
        for (full_wrap, exterior) in [
            (false, GlobeExterior::Rounded),
            (true, GlobeExterior::Rounded),
            (true, GlobeExterior::Faceted),
        ] {
            let globe = GlobePrototypeSpec {
                source: GlobeSource::SmallWorld,
                full_wrap,
                exterior,
                full_shell: true,
                samples_per_edge: 16,
                ..GlobePrototypeSpec::default()
            };
            let mut generation = GenerationSpec {
                globe: Some(globe),
                ..GenerationSpec::default()
            };
            generation.color_output.enabled = true;
            let map = small_world_map_spec(&generation).unwrap();
            let mut cover =
                SurfaceField::new(9, 9, vec![crate::SurfaceClass::Forest; 81], "test").unwrap();
            // A road through the map center, crossing tile seams.
            cover.paint_polyline(
                &[[0.1, 0.5], [0.9, 0.52]],
                map.width_mm,
                1.2,
                crate::SurfaceClass::Road,
            );
            let (parts, order, _) = globe_parts(
                &globe,
                GlobeData {
                    map_spec: Some(&map),
                    height: Some(&field),
                    cover: Some(&cover),
                    ..GlobeData::default()
                },
            )
            .unwrap();
            assert_eq!(order.len(), 20);
            let mut road_tiles = 0;
            for part in &parts {
                validate_part(&part.mesh)
                    .unwrap_or_else(|error| panic!("{}: {error:#}", part.mesh.name));
                if part.mesh.name.starts_with("tile-")
                    && part
                        .mesh
                        .materials
                        .contains(&PrintMaterial::Class(crate::SurfaceClass::Road))
                {
                    road_tiles += 1;
                }
            }
            assert!(
                road_tiles >= 2,
                "the road should cross at least one seam ({full_wrap}, {exterior:?})"
            );
        }
    }

    #[test]
    fn screw_brackets_and_tile_inserts_stay_watertight_and_list_their_hardware() {
        for exterior in [GlobeExterior::Faceted, GlobeExterior::Rounded] {
            let spec = GlobePrototypeSpec {
                exterior,
                screw_brackets: true,
                tile_screws: true,
                insert: HeatSetInsert {
                    screw_mm: 3.0,
                    hole_diameter_mm: 4.2,
                    length_mm: 5.0,
                },
                ..GlobePrototypeSpec::default()
            };
            let (parts, order, _) = globe_parts(&spec, GlobeData::default()).unwrap();
            assert_eq!(order.len(), 20);
            // Tiles, frame modules, keys, brackets, and the clip.
            assert_eq!(parts.len(), 20 + 20 + 30 + 30 + 1);
            for part in &parts {
                validate_part(&part.mesh)
                    .unwrap_or_else(|error| panic!("{}: {error:#}", part.mesh.name));
            }
        }
        let output =
            std::env::temp_dir().join(format!("toposaic-globe-inserts-{}", std::process::id()));
        let _ = fs::remove_dir_all(&output);
        let spec = GlobePrototypeSpec {
            screw_brackets: true,
            tile_screws: true,
            ..GlobePrototypeSpec::default()
        };
        export_globe_prototype(&spec, &output).unwrap();
        let guide = fs::read_to_string(output.join("ASSEMBLY.md")).unwrap();
        assert!(guide.contains("120 M3 heat-set inserts"), "{guide}");
        assert!(guide.contains("20 M3 heat-set inserts"), "{guide}");
        assert!(guide.contains("dovetail key"), "{guide}");
        fs::remove_dir_all(&output).unwrap();
    }

    #[test]
    fn smallest_seats_fit_m3_screws_beside_brackets_and_refuse_m5() {
        // Both screw options on the smallest seat any layout allows. M3
        // hardware must clear itself; M5 needs more room and is refused.
        let m3 = GlobePrototypeSpec {
            diameter_mm: 1_000.0,
            screw_brackets: true,
            tile_screws: true,
            ..GlobePrototypeSpec::default()
        };
        let (subdivision, tile) = (1..=MAX_SUBDIVISION)
            .filter_map(|subdivision| {
                let faces = GlobePrototypeSpec { subdivision, ..m3 }.layout().ok()?;
                let face = faces
                    .iter()
                    .min_by(|a, b| a.inradius().total_cmp(&b.inradius()))?;
                Some((face.inradius(), subdivision, face.id))
            })
            .min_by(|a, b| a.0.total_cmp(&b.0))
            .map(|(_, subdivision, tile)| (subdivision, tile))
            .expect("a valid layout");
        let m3 = GlobePrototypeSpec {
            subdivision,
            batch: Some(GlobeBatch {
                first_tile: tile,
                tile_count: 1,
            }),
            ..m3
        };
        for part in globe_parts(&m3, GlobeData::default()).unwrap().0 {
            validate_part(&part.mesh).unwrap();
        }
        let m5 = GlobePrototypeSpec {
            insert: HeatSetInsert {
                screw_mm: 5.0,
                hole_diameter_mm: 6.4,
                length_mm: 6.0,
            },
            ..m3
        };
        let error = globe_parts(&m5, GlobeData::default())
            .err()
            .unwrap()
            .to_string();
        assert!(error.contains("too small for both"), "{error}");
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
                .any(|part| part.mesh.name.starts_with("key-01-"))
        );
        assert!(
            second
                .iter()
                .all(|part| !part.mesh.name.starts_with("key-01-"))
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
        let band = GlobePrototypeSpec {
            source: GlobeSource::SmallWorld,
            full_wrap: true,
            ..GlobePrototypeSpec::default()
        };
        let generation = GenerationSpec {
            globe: Some(band),
            ..GenerationSpec::default()
        };
        let map = small_world_map_spec(&generation).unwrap();
        let region = WrappedMap::new(MapWrap::new(pose, 100.0, &map, &band), &map, &field, None);
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
