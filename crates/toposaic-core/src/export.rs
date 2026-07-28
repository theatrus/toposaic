use std::{
    fs::File,
    io::{BufWriter, Write},
    path::Path,
};

use anyhow::{Context, Result};
use rayon::prelude::*;
use zip::{ZipWriter, write::SimpleFileOptions};

use crate::mesh::Mesh;
use crate::spec::{GenerationSpec, MaterialPalette, PaintedClasses, SurfaceClass, ThreeMfStyle};

pub(crate) fn write_binary_stl(mesh: &Mesh, path: &Path) -> Result<()> {
    let mut writer = BufWriter::new(
        File::create(path).with_context(|| format!("create STL {}", path.display()))?,
    );
    let mut header = [0_u8; 80];
    let label = format!("TopoSaic — {}", mesh.name);
    let bytes = label.as_bytes();
    header[..bytes.len().min(80)].copy_from_slice(&bytes[..bytes.len().min(80)]);
    writer.write_all(&header)?;
    writer.write_all(&(mesh.triangles.len() as u32).to_le_bytes())?;

    for triangle in &mesh.triangles {
        let a = mesh.vertices[triangle[0] as usize];
        let b = mesh.vertices[triangle[1] as usize];
        let c = mesh.vertices[triangle[2] as usize];
        let normal = face_normal(a, b, c);
        for value in normal.into_iter().chain(a).chain(b).chain(c) {
            writer.write_all(&value.to_le_bytes())?;
        }
        writer.write_all(&0_u16.to_le_bytes())?;
    }
    writer.flush()?;
    Ok(())
}

fn face_normal(a: [f32; 3], b: [f32; 3], c: [f32; 3]) -> [f32; 3] {
    let ab = [b[0] - a[0], b[1] - a[1], b[2] - a[2]];
    let ac = [c[0] - a[0], c[1] - a[1], c[2] - a[2]];
    let cross = [
        ab[1] * ac[2] - ab[2] * ac[1],
        ab[2] * ac[0] - ab[0] * ac[2],
        ab[0] * ac[1] - ab[1] * ac[0],
    ];
    let length = (cross[0] * cross[0] + cross[1] * cross[1] + cross[2] * cross[2])
        .sqrt()
        .max(f32::EPSILON);
    [cross[0] / length, cross[1] / length, cross[2] / length]
}

/// Writes pre-formatted XML one line at a time so the BufWriter in front of
/// the zip compressor flushes at (nearly) the same points as the previous
/// line-by-line serial writer.
fn write_lines(output: &mut impl Write, buffers: &[Vec<u8>]) -> std::io::Result<()> {
    for buffer in buffers {
        for line in buffer.split_inclusive(|byte| *byte == b'\n') {
            output.write_all(line)?;
        }
    }
    Ok(())
}

pub(crate) struct ThreeMfWriter<'a> {
    zip: ZipWriter<File>,
    spec: &'a GenerationSpec,
    /// The archive's dense filament palette, built once from the spec and
    /// the surface data that together govern every mesh in the file. The
    /// color group, the per-triangle property indices, the paint codes, and
    /// the project-settings arrays all take their slot numbers from it, so
    /// they cannot disagree.
    palette: MaterialPalette<'a>,
    object_count: usize,
}

const COLOR_GROUP_ID: u32 = 1000;

/// Whether a style carries the core-spec color group and the per-triangle
/// `pid`/`p1`/`p2`/`p3` references into it. `Painted` alone does not.
///
/// Bambu Studio has two import flows and they read different parts of a
/// third-party file. Opening AS A PROJECT loads the embedded
/// `project_settings.config`, so the filament list becomes exactly this
/// archive's palette. IMPORTING GEOMETRY into an existing project skips the
/// embedded settings by design; there, the color group is what carries the
/// colors — Bambu collects the per-triangle `pid` references of a file it
/// did not generate and opens its "Standard 3mf Import color" dialog, whose
/// Color match maps this palette onto the filaments already loaded.
/// `Project` therefore carries the group so its colors survive BOTH flows.
/// (OrcaSlicer reads only the settings and the `paint_color` codes; it has
/// no such dialog, and only ever maps a color group through an object-level
/// `pid`, which these archives do not use.)
///
/// `Painted` is the plain pre-painted model: `paint_color` codes assign
/// extruders 1..N and the colors come from whatever filaments the project
/// has. No group, no settings, no dialogs, no presets touched — in either
/// slicer.
///
/// `Geometry` is the standards flavor: the color group is the one channel
/// other 3MF consumers read, so it is the one channel that style writes.
fn carries_color_group(style: ThreeMfStyle) -> bool {
    style != ThreeMfStyle::Painted
}
/// Elements formatted per rayon task when writing 3MF XML bodies.
const FORMAT_CHUNK_ELEMENTS: usize = 64 * 1024;
/// Elements formatted per in-memory batch; keeps peak buffered XML text to
/// a few tens of megabytes even for meshes with millions of triangles.
const WRITE_BATCH_ELEMENTS: usize = 1024 * 1024;
// OrcaSlicer and Bambu Studio face-paint values for extruders 1–9, from
// PrusaSlicer's TriangleSelector serialization. An unsplit painted triangle
// stores its extruder number n as a nibble stream: n = 1 or 2 fits one
// nibble, hex(n << 2) — "4", "8". From n = 3 up the state nibble saturates
// at 0xC and an extension nibble carries n - 3, written before the marker —
// "0C" through "9C" for extruders 3 to 12.
// Keep the standard 3MF color properties too, for consumers that support
// them.
//
// The index here is the archive's DENSE filament slot, not the surface
// class: extruder number = slot + 1. A spec that emits six classes only ever
// reaches "3C" whichever six they are.
const ORCA_PAINT_CODES: [&str; 12] = [
    "4", "8", "0C", "1C", "2C", "3C", "4C", "5C", "6C", "7C", "8C", "9C",
];
const _: () = assert!(
    ORCA_PAINT_CODES.len() == crate::spec::SurfaceClass::ALL.len(),
    "every surface class needs a face-paint code"
);

impl<'a> ThreeMfWriter<'a> {
    /// Opens an archive. `painted` is what the meshes to come can paint, and
    /// it is what lets the filament palette leave out a layer the settings
    /// enable but this archive never draws.
    pub(crate) fn new(
        spec: &'a GenerationSpec,
        painted: PaintedClasses,
        path: &Path,
    ) -> Result<Self> {
        let file = File::create(path).with_context(|| format!("create 3MF {}", path.display()))?;
        let mut zip = ZipWriter::new(file);
        let options = SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Deflated)
            .compression_level(Some(3));

        zip.start_file("[Content_Types].xml", options)?;
        zip.write_all(
            br#"<?xml version="1.0" encoding="UTF-8"?>
<Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types">
  <Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/>
  <Default Extension="model" ContentType="application/vnd.ms-package.3dmanufacturing-3dmodel+xml"/>
</Types>"#,
        )?;

        zip.add_directory("_rels/", options)?;
        zip.start_file("_rels/.rels", options)?;
        zip.write_all(
        br#"<?xml version="1.0" encoding="UTF-8"?>
<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">
  <Relationship Target="/3D/3dmodel.model" Id="rel-1" Type="http://schemas.microsoft.com/3dmanufacturing/2013/01/3dmodel"/>
</Relationships>"#,
    )?;

        zip.add_directory("3D/", options)?;
        zip.start_file("3D/3dmodel.model", options)?;
        let palette = spec.material_palette(painted);
        // The material namespace is declared as a REQUIRED extension, so it
        // may only appear when the color group it exists for does.
        let writes_color_group = spec.uses_color_materials()
            && carries_color_group(spec.color_output.threemf_style)
            && !palette.is_empty();
        if writes_color_group {
            zip.write_all(
            r#"<?xml version="1.0" encoding="UTF-8"?>
<model unit="millimeter" xml:lang="en-US" xmlns="http://schemas.microsoft.com/3dmanufacturing/core/2015/02" xmlns:m="http://schemas.microsoft.com/3dmanufacturing/material/2015/02" requiredextensions="m">
  <metadata name="Title">TopoSaic</metadata>
  <metadata name="Designer">TopoSaic Terrain Puzzle Generator</metadata>
  <resources>
"#
                .as_bytes(),
            )?;
        } else {
            zip.write_all(
            r#"<?xml version="1.0" encoding="UTF-8"?>
<model unit="millimeter" xml:lang="en-US" xmlns="http://schemas.microsoft.com/3dmanufacturing/core/2015/02">
  <metadata name="Title">TopoSaic</metadata>
  <metadata name="Designer">TopoSaic Terrain Puzzle Generator</metadata>
  <resources>
"#
                .as_bytes(),
            )?;
        }
        if writes_color_group {
            // Only colors this archive prints, one slot each.
            let mut colors = String::new();
            for color in palette.colors() {
                colors.push_str(&format!("      <m:color color=\"{color}FF\"/>\n"));
            }
            writeln!(
                zip,
                "    <m:colorgroup id=\"{COLOR_GROUP_ID}\">\n{colors}    </m:colorgroup>",
            )?;
        }
        Ok(Self {
            zip,
            spec,
            palette,
            object_count: 0,
        })
    }

    pub(crate) fn write_mesh(&mut self, mesh: &Mesh) -> Result<()> {
        // A real check, not a debug assertion: the triangle loop below zips
        // the two lists, so in release a short material list would silently
        // truncate the mesh out of the archive.
        anyhow::ensure!(
            mesh.triangles.len() == mesh.materials.len(),
            "mesh {} has {} triangles but {} materials",
            mesh.name,
            mesh.triangles.len(),
            mesh.materials.len()
        );
        // The palette is built from the spec, so a mesh carrying a class the
        // spec says it never draws would silently take another class's
        // filament. Fail instead: a mis-colored print is worse than a
        // refused one, and this can only fire on a generator bug.
        if self.spec.uses_color_materials() {
            let mut seen = [false; SurfaceClass::ALL.len()];
            for material in &mesh.materials {
                seen[material.material_index() as usize] = true;
            }
            for class in SurfaceClass::ALL {
                anyhow::ensure!(
                    !seen[class.material_index() as usize] || self.palette.slot(class).is_some(),
                    "mesh {} paints {class:?} triangles, which this spec's filament palette \
                     does not carry",
                    mesh.name,
                );
            }
        }
        // Slot per class, flattened for the parallel formatters below. Every
        // class the mesh actually uses is in the palette by the check above,
        // so the fallback can never be reached.
        let slots = SurfaceClass::ALL.map(|class| self.palette.slot(class).unwrap_or(0));
        let object_id = self.object_count + 1;
        // Formatting millions of decimal numbers dominates the serial 3MF
        // write, so format fixed-size ranges in parallel — with the exact
        // same per-element write pattern as before — and hand the buffers
        // to the zip writer in order. Batching bounds the amount of
        // formatted text held in memory at once. The buffered text is fed
        // to the compressor in small writes through the same 64 KiB
        // BufWriter the serial code used: deflate's block framing depends
        // on how its input is chunked, so keeping the write pattern keeps
        // the archive bytes identical, not just the decompressed stream.
        let mut output = BufWriter::with_capacity(64 * 1024, &mut self.zip);
        // The name is interpolated into XML without escaping. That is safe
        // only because every mesh name is generator-made ("piece-2-3",
        // "terrain-tray-r1-c2", "terrain-solid") and never carries XML
        // metacharacters or user text; escape here first if that changes.
        writeln!(
            output,
            "    <object id=\"{object_id}\" name=\"{}\" type=\"model\"><mesh><vertices>",
            mesh.name
        )?;
        for batch in mesh.vertices.chunks(WRITE_BATCH_ELEMENTS) {
            let buffers = batch
                .par_chunks(FORMAT_CHUNK_ELEMENTS)
                .map(|chunk| {
                    let mut buffer = Vec::with_capacity(chunk.len() * 56);
                    for vertex in chunk {
                        writeln!(
                            buffer,
                            "      <vertex x=\"{:.5}\" y=\"{:.5}\" z=\"{:.5}\"/>",
                            vertex[0], vertex[1], vertex[2]
                        )
                        .expect("writing to a Vec cannot fail");
                    }
                    buffer
                })
                .collect::<Vec<_>>();
            write_lines(&mut output, &buffers)?;
        }
        output.write_all(b"    </vertices><triangles>\n")?;
        let uses_color = self.spec.uses_color_materials();
        // Which color channels each triangle carries — see
        // `carries_color_group` for why they differ by style. `Project`
        // carries both and is the exact pre-style code path, so its archives
        // keep their previous bytes.
        let style = self.spec.color_output.threemf_style;
        let groups = carries_color_group(style);
        let paints = style != ThreeMfStyle::Geometry;
        for (triangles, materials) in mesh
            .triangles
            .chunks(WRITE_BATCH_ELEMENTS)
            .zip(mesh.materials.chunks(WRITE_BATCH_ELEMENTS))
        {
            let buffers = triangles
                .par_chunks(FORMAT_CHUNK_ELEMENTS)
                .zip(materials.par_chunks(FORMAT_CHUNK_ELEMENTS))
                .map(|(triangles, materials)| {
                    let mut buffer = Vec::with_capacity(triangles.len() * 128);
                    for (triangle, material) in triangles.iter().zip(materials) {
                        if uses_color && groups && paints {
                            let index = slots[material.material_index() as usize];
                            let paint_color = ORCA_PAINT_CODES[index as usize];
                            writeln!(
                                buffer,
                                "      <triangle v1=\"{}\" v2=\"{}\" v3=\"{}\" pid=\"{COLOR_GROUP_ID}\" p1=\"{index}\" p2=\"{index}\" p3=\"{index}\" paint_color=\"{paint_color}\"/>",
                                triangle[0], triangle[1], triangle[2],
                            )
                            .expect("writing to a Vec cannot fail");
                        } else if uses_color && paints {
                            let index = slots[material.material_index() as usize];
                            let paint_color = ORCA_PAINT_CODES[index as usize];
                            writeln!(
                                buffer,
                                "      <triangle v1=\"{}\" v2=\"{}\" v3=\"{}\" paint_color=\"{paint_color}\"/>",
                                triangle[0], triangle[1], triangle[2],
                            )
                            .expect("writing to a Vec cannot fail");
                        } else if uses_color {
                            let index = slots[material.material_index() as usize];
                            writeln!(
                                buffer,
                                "      <triangle v1=\"{}\" v2=\"{}\" v3=\"{}\" pid=\"{COLOR_GROUP_ID}\" p1=\"{index}\" p2=\"{index}\" p3=\"{index}\"/>",
                                triangle[0], triangle[1], triangle[2],
                            )
                            .expect("writing to a Vec cannot fail");
                        } else {
                            writeln!(
                                buffer,
                                "      <triangle v1=\"{}\" v2=\"{}\" v3=\"{}\"/>",
                                triangle[0], triangle[1], triangle[2]
                            )
                            .expect("writing to a Vec cannot fail");
                        }
                    }
                    buffer
                })
                .collect::<Vec<_>>();
            write_lines(&mut output, &buffers)?;
        }
        output.write_all(b"    </triangles></mesh></object>\n")?;
        output.flush()?;
        drop(output);
        self.object_count += 1;
        Ok(())
    }

    pub(crate) fn finish(mut self) -> Result<()> {
        self.zip.write_all(b"  </resources>\n  <build>\n")?;
        let piece_width = self.spec.width_mm / self.spec.columns as f32;
        let piece_height = self.spec.height_mm() / self.spec.rows as f32;
        let spacing = piece_width.min(piece_height) * 0.3;
        for index in 0..self.object_count {
            let row = if self.spec.solid_model {
                0
            } else {
                index as u32 / self.spec.columns
            };
            let column = if self.spec.solid_model {
                0
            } else {
                index as u32 % self.spec.columns
            };
            let tx = column as f32 * (piece_width + spacing);
            let ty = row as f32 * (piece_height + spacing);
            writeln!(
                self.zip,
                "    <item objectid=\"{}\" transform=\"1 0 0 0 1 0 0 0 1 {:.5} {:.5} 0\"/>",
                index + 1,
                tx,
                ty
            )?;
        }
        self.zip.write_all(b"  </build>\n</model>")?;
        // Only the `Project` style embeds Metadata/project_settings.config.
        // Its presence makes OrcaSlicer and Bambu Studio import the archive
        // as a full PROJECT — filament colours and purge volumes, but also
        // printer, material, and process preset state. That one-click color
        // setup is what `Project` (the default) is for; `Painted` and
        // `Geometry` skip the file so opening the model never touches the
        // user's presets. The per-triangle `paint_color` codes are separate
        // MMU-painting metadata and do not trigger the project prompt.
        if self.spec.uses_color_materials()
            && self.spec.color_output.threemf_style == ThreeMfStyle::Project
        {
            let options = SimpleFileOptions::default()
                .compression_method(zip::CompressionMethod::Deflated)
                .compression_level(Some(3));
            self.zip.add_directory("Metadata/", options)?;
            self.zip
                .start_file("Metadata/project_settings.config", options)?;
            // Every per-filament array, and the flush matrix's side length,
            // size themselves from the same dense palette the color group
            // and the paint codes used, so slot n means one filament
            // throughout the archive.
            //
            // `filament_settings_id` names the slicer preset each slot
            // loads. Left empty, OrcaSlicer and Bambu Studio have nothing to
            // match and fall back to whichever preset they please — which is
            // how six terrain colors arrived as Generic TPU. Naming a real
            // preset, with the vendor it belongs to, makes them import as
            // the PLA the `filament_type` beside them already says they are.
            let colors = self.palette.colors();
            let profile = self.spec.color_output.filament_profile;
            let (preset, vendor) = profile.preset();
            let flush_volumes_matrix = (0..self.palette.len())
                .flat_map(|row| {
                    (0..self.palette.len())
                        .map(move |column| if row == column { "0" } else { "280" })
                })
                .collect::<Vec<_>>();
            let project_settings = serde_json::json!({
                "default_filament_colour": colors,
                "filament_colour": colors,
                "filament_settings_id": vec![preset; colors.len()],
                "filament_type": vec![profile.material(); colors.len()],
                "filament_vendor": vec![vendor; colors.len()],
                "flush_volumes_matrix": flush_volumes_matrix,
                "flush_volumes_vector": vec!["140"; colors.len() * 2],
            });
            serde_json::to_writer_pretty(&mut self.zip, &project_settings)?;
        }
        self.zip.finish()?;
        Ok(())
    }
}

/// Writes a one-mesh 3MF — a tray segment, a piece of wall-mount hardware, a
/// flag template. The mesh is in hand before the archive opens, so its
/// palette is exactly the colors that mesh paints and no others.
pub(crate) fn write_single_mesh_3mf(spec: &GenerationSpec, mesh: &Mesh, path: &Path) -> Result<()> {
    let mut writer = ThreeMfWriter::new(spec, PaintedClasses::of_mesh(mesh), path)?;
    writer.write_mesh(mesh)?;
    writer.finish()
}

#[cfg(test)]
mod tests {
    use std::io::Read;

    use super::*;
    use crate::spec::{ColorOutputSpec, SurfaceClass};
    use crate::surface::SurfaceField;

    /// The six-slot spec the golden fixture was written from.
    ///
    /// The rail, aerial, and ferry layers all default to their own color,
    /// so they are pinned here to the MERGED styles — the fallback a user
    /// picks to save the spools. That is the configuration whose archive still has to
    /// match the pre-trail six-slot output byte for byte, and pinning it
    /// here is what keeps the golden fixture reachable.
    fn fixture_spec(style: ThreeMfStyle) -> GenerationSpec {
        GenerationSpec {
            rows: 2,
            columns: 2,
            color_output: ColorOutputSpec {
                enabled: true,
                threemf_style: style,
                rail_style: crate::spec::RailStyle::WithRoads,
                aerial_style: crate::spec::AerialStyle::WithRoads,
                ferry_style: crate::spec::FerryStyle::WithRoads,
                ..ColorOutputSpec::default()
            },
            ..GenerationSpec::default()
        }
    }

    /// The six surface classes that existed when the project-style golden
    /// fixture was generated. `SurfaceClass::ALL` has since gained `Trail`
    /// and `Rail`, neither of which a spec without trails and without a
    /// separately-styled rail layer ever emits, so the fixture meshes stay
    /// pinned to the original six.
    const PRE_TRAIL_CLASSES: [SurfaceClass; 6] = [
        SurfaceClass::Rock,
        SurfaceClass::Forest,
        SurfaceClass::Snow,
        SurfaceClass::Water,
        SurfaceClass::Road,
        SurfaceClass::Building,
    ];

    /// Two small meshes whose triangles cover every pre-trail surface
    /// class. The project-style golden fixture was generated from these
    /// exact meshes with the pre-style writer, so do not change them.
    fn fixture_meshes() -> Vec<Mesh> {
        (0..2u32)
            .map(|piece| {
                let mut vertices = Vec::new();
                let mut triangles = Vec::new();
                let mut materials = Vec::new();
                for (index, class) in PRE_TRAIL_CLASSES.into_iter().enumerate() {
                    let base = vertices.len() as u32;
                    let x = piece as f32 * 40.0 + index as f32 * 3.25;
                    vertices.push([x, 0.0, 0.5 + index as f32 * 0.125]);
                    vertices.push([x + 2.5, 0.75, 1.0 + piece as f32]);
                    vertices.push([x + 1.25, 3.125, 2.0 + index as f32]);
                    triangles.push([base, base + 1, base + 2]);
                    materials.push(class);
                }
                Mesh {
                    name: format!("piece-{}", piece + 1),
                    vertices,
                    triangles,
                    materials,
                    quantization_collisions: Vec::new(),
                }
            })
            .collect()
    }

    /// The classes a set of finished meshes paints, as every caller holding
    /// its meshes hands them to the writer.
    fn painted_by(meshes: &[Mesh]) -> PaintedClasses {
        let mut present = [false; SurfaceClass::ALL.len()];
        for material in meshes.iter().flat_map(|mesh| &mesh.materials) {
            present[material.material_index() as usize] = true;
        }
        PaintedClasses::Exact(present)
    }

    /// Nothing ruled out by the data, so the settings alone decide the
    /// palette. Used to test what a spec CAN emit, apart from any map.
    fn any_class() -> PaintedClasses {
        PaintedClasses::Exact([true; SurfaceClass::ALL.len()])
    }

    fn write_fixture(style: ThreeMfStyle) -> Vec<u8> {
        write_spec_fixture(&fixture_spec(style))
    }

    /// Writes the fixture meshes under an arbitrary spec, so two specs can
    /// be compared archive against archive.
    fn write_spec_fixture(spec: &GenerationSpec) -> Vec<u8> {
        let meshes = fixture_meshes();
        write_spec_meshes(spec, &meshes, painted_by(&meshes))
    }

    fn write_spec_meshes(
        spec: &GenerationSpec,
        meshes: &[Mesh],
        painted: PaintedClasses,
    ) -> Vec<u8> {
        static NEXT_FIXTURE: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
        let unique = NEXT_FIXTURE.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let style = spec.color_output.threemf_style;
        let path = std::env::temp_dir().join(format!(
            "toposaic-3mf-style-{style:?}-{}-{unique}.3mf",
            std::process::id()
        ));
        let mut writer = ThreeMfWriter::new(spec, painted, &path).unwrap();
        for mesh in meshes {
            writer.write_mesh(mesh).unwrap();
        }
        writer.finish().unwrap();
        let bytes = std::fs::read(&path).unwrap();
        std::fs::remove_file(path).unwrap();
        bytes
    }

    fn archive_names(bytes: &[u8]) -> Vec<String> {
        let archive = zip::ZipArchive::new(std::io::Cursor::new(bytes)).unwrap();
        archive.file_names().map(str::to_owned).collect()
    }

    fn archive_entry(bytes: &[u8], name: &str) -> String {
        let mut archive = zip::ZipArchive::new(std::io::Cursor::new(bytes)).unwrap();
        let mut contents = String::new();
        archive
            .by_name(name)
            .unwrap()
            .read_to_string(&mut contents)
            .unwrap();
        contents
    }

    fn model_xml(bytes: &[u8]) -> String {
        archive_entry(bytes, "3D/3dmodel.model")
    }

    fn project_settings(bytes: &[u8]) -> String {
        archive_entry(bytes, "Metadata/project_settings.config")
    }

    /// The default `Project` style must keep producing the archive it
    /// produced last time it was reviewed — geometry included, and for a
    /// six-color spec that geometry is still what the writer emitted before
    /// `ThreeMfStyle` existed (commit 6e4b1a0). The fixture is written from
    /// `fixture_spec`/`fixture_meshes` above; deterministic zip timestamps
    /// (a constant 1980 date without the `time` feature) and the pure-Rust
    /// zlib-rs deflate make whole-archive comparison stable.
    ///
    /// The embedded slicer settings have moved once since: naming the
    /// `Generic PLA` preset instead of leaving the filament id empty.
    #[test]
    fn project_style_output_is_byte_identical_to_pre_style_writer() {
        let golden = include_bytes!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/testdata/project-style-golden.3mf"
        ));
        let current = write_fixture(ThreeMfStyle::Project);
        if current != golden.as_slice() {
            // Name which entry moved before blaming compression: geometry,
            // the embedded slicer settings, or neither (a zip or zlib-rs
            // bump reframes deflate blocks without touching content, which
            // is the only one of the three safe to accept unreviewed).
            assert_eq!(
                model_xml(&current),
                model_xml(golden),
                "the 3MF MODEL CONTENT changed; if that change is intentional, \
                 regenerate the fixture with: cargo test -p toposaic-core \
                 regenerate_project_style_golden -- --ignored"
            );
            assert_eq!(
                project_settings(&current),
                project_settings(golden),
                "the EMBEDDED SLICER SETTINGS changed; if that change is \
                 intentional, regenerate the fixture with: cargo test -p \
                 toposaic-core regenerate_project_style_golden -- --ignored"
            );
            panic!(
                "the 3MF content is unchanged but the archive bytes differ — \
                 a compression dependency changed its output. Verify the \
                 decompressed entries match, then regenerate the fixture \
                 with: cargo test -p toposaic-core \
                 regenerate_project_style_golden -- --ignored"
            );
        }
        // The default style is `Project`, so an untouched spec gets the
        // same bytes too.
        assert_eq!(
            fixture_spec(ThreeMfStyle::default())
                .color_output
                .threemf_style,
            ThreeMfStyle::Project
        );
    }

    /// Rewrites the golden fixture from the CURRENT writer. Running this
    /// accepts today's output as the new baseline, which is only legitimate
    /// after an intentional, reviewed format change (or a verified
    /// compression-only dependency change). Never run it to silence a
    /// failure you don't understand — that destroys the invariant the
    /// golden test protects.
    #[test]
    #[ignore = "rewrites testdata/project-style-golden.3mf from the current writer"]
    fn regenerate_project_style_golden() {
        let path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/testdata/project-style-golden.3mf"
        );
        std::fs::write(path, write_fixture(ThreeMfStyle::Project)).unwrap();
    }

    #[test]
    fn only_the_project_style_embeds_project_settings() {
        let expected_project = [
            "[Content_Types].xml",
            "_rels/",
            "_rels/.rels",
            "3D/",
            "3D/3dmodel.model",
            "Metadata/",
            "Metadata/project_settings.config",
        ];
        assert_eq!(
            archive_names(&write_fixture(ThreeMfStyle::Project)),
            expected_project
        );
        let expected_plain = [
            "[Content_Types].xml",
            "_rels/",
            "_rels/.rels",
            "3D/",
            "3D/3dmodel.model",
        ];
        assert_eq!(
            archive_names(&write_fixture(ThreeMfStyle::Painted)),
            expected_plain
        );
        assert_eq!(
            archive_names(&write_fixture(ThreeMfStyle::Geometry)),
            expected_plain
        );
    }

    /// The channels each style carries — see `carries_color_group`.
    /// `Project` carries the group AND the paint codes, so its colors
    /// survive both of Bambu Studio's import flows; `Painted` is a plain
    /// pre-painted model, paint codes only; `Geometry` is the standards
    /// flavor, group only.
    #[test]
    fn each_style_carries_its_own_color_channels() {
        for style in [ThreeMfStyle::Painted, ThreeMfStyle::Project] {
            let model = model_xml(&write_fixture(style));
            for code in &ORCA_PAINT_CODES[..PRE_TRAIL_CLASSES.len()] {
                assert!(
                    model.contains(&format!(" paint_color=\"{code}\"/>")),
                    "{style:?} should carry paint code {code}"
                );
            }
        }

        // Project: the color group rides along with the paint codes.
        let model = model_xml(&write_fixture(ThreeMfStyle::Project));
        assert!(model.contains("<m:colorgroup id=\"1000\">"));
        assert!(model.contains("pid=\"1000\" p1=\"5\" p2=\"5\" p3=\"5\""));
        assert!(model.contains("requiredextensions=\"m\""));

        // Painted: paint codes alone, and no leftover REQUIRED extension
        // declaration for a group that is not there.
        let model = model_xml(&write_fixture(ThreeMfStyle::Painted));
        assert!(!model.contains("<m:colorgroup"));
        assert!(!model.contains("pid="));
        assert!(!model.contains("requiredextensions"));
        assert!(!model.contains("xmlns:m="));

        let model = model_xml(&write_fixture(ThreeMfStyle::Geometry));
        assert!(!model.contains("paint_color"));
        // The core-spec color group and per-triangle references stay.
        assert!(model.contains("<m:colorgroup id=\"1000\">"));
        assert!(model.contains("color=\"#28543AFF\""));
        for index in 0..6 {
            assert!(model.contains(&format!(
                "pid=\"1000\" p1=\"{index}\" p2=\"{index}\" p3=\"{index}\"/>"
            )));
        }
        assert_eq!(model.matches("<object id=").count(), 2);
    }

    #[test]
    fn mismatched_material_lists_fail_the_write_instead_of_truncating() {
        let path =
            std::env::temp_dir().join(format!("toposaic-3mf-mismatch-{}.3mf", std::process::id()));
        let spec = fixture_spec(ThreeMfStyle::Project);
        let mut writer = ThreeMfWriter::new(&spec, any_class(), &path).unwrap();
        let mut mesh = fixture_meshes().remove(0);
        mesh.materials.pop();
        let error = writer.write_mesh(&mesh).unwrap_err().to_string();
        assert!(error.contains("triangles but"), "{error}");
        drop(writer);
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn paint_code_table_matches_the_triangle_selector_encoding() {
        // Reproduce every code from the TriangleSelector rule the table
        // documents: extruder n <= 2 is one nibble hex(n << 2); n >= 3 is
        // the extension nibble hex(n - 3) followed by the saturated "C".
        for (index, expected) in ORCA_PAINT_CODES.into_iter().enumerate() {
            let extruder = index + 1;
            let derived = if extruder <= 2 {
                format!("{:X}", extruder << 2)
            } else {
                format!("{:X}C", extruder - 3)
            };
            assert_eq!(derived, expected, "extruder {extruder}");
        }
        // Spot checks across the extension-nibble range: the first code
        // that needs one, and the last slot the widest possible palette can
        // reach — every surface class emitted at once.
        assert_eq!(ORCA_PAINT_CODES[2], "0C");
        assert_eq!(ORCA_PAINT_CODES[6], "4C");
        assert_eq!(ORCA_PAINT_CODES[7], "5C");
        assert_eq!(ORCA_PAINT_CODES[8], "6C");
        assert_eq!(ORCA_PAINT_CODES.len(), SurfaceClass::ALL.len());

        // The codes are indexed by DENSE SLOT, not by class, so the code a
        // class gets moves with the palette. A separately-styled rail layer
        // alone takes slot 7 ("4C"); behind imported trails it takes slot 8.
        let mut spec = fixture_spec(ThreeMfStyle::Project);
        spec.color_output.rail_enabled = true;
        spec.color_output.rail_style = crate::spec::RailStyle::Separate;
        let slot = spec
            .material_palette(any_class())
            .slot(SurfaceClass::Rail)
            .unwrap();
        assert_eq!(ORCA_PAINT_CODES[slot as usize], "4C");
        spec.trails = vec![crate::spec::TrailRoute {
            name: "Loop".into(),
            points: vec![[46.8, -121.8], [46.9, -121.7]],
        }];
        spec.markers = vec![crate::spec::MapMarker {
            name: "Point".into(),
            latitude: 46.85,
            longitude: -121.76,
            kind: crate::spec::MarkerKind::Dot,
            label_height_mm: 4.0,
            rotation_degrees: 0.0,
            dot_style: None,
            flag_style: None,
            label_style: None,
        }];
        let slot = spec
            .material_palette(any_class())
            .slot(SurfaceClass::Rail)
            .unwrap();
        assert_eq!(ORCA_PAINT_CODES[slot as usize], "5C");
    }

    fn trail_spec(style: ThreeMfStyle) -> GenerationSpec {
        GenerationSpec {
            trails: vec![crate::spec::TrailRoute {
                name: "Skyline Loop".into(),
                points: vec![[46.78, -121.73], [46.79, -121.74]],
            }],
            ..fixture_spec(style)
        }
    }

    /// The fixture meshes plus one Trail-material triangle, as a piece with
    /// an imported trail would carry.
    fn trail_meshes() -> Vec<Mesh> {
        let mut meshes = fixture_meshes();
        let mesh = &mut meshes[0];
        let base = mesh.vertices.len() as u32;
        mesh.vertices.push([90.0, 0.0, 1.0]);
        mesh.vertices.push([92.5, 0.75, 1.5]);
        mesh.vertices.push([91.25, 3.125, 2.0]);
        mesh.triangles.push([base, base + 1, base + 2]);
        mesh.materials.push(SurfaceClass::Trail);
        meshes
    }

    fn write_trail_fixture(style: ThreeMfStyle) -> Vec<u8> {
        static NEXT_FIXTURE: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
        let unique = NEXT_FIXTURE.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "toposaic-3mf-trail-{style:?}-{}-{unique}.3mf",
            std::process::id()
        ));
        let spec = trail_spec(style);
        let meshes = trail_meshes();
        let mut writer = ThreeMfWriter::new(&spec, painted_by(&meshes), &path).unwrap();
        for mesh in &meshes {
            writer.write_mesh(mesh).unwrap();
        }
        writer.finish().unwrap();
        let bytes = std::fs::read(&path).unwrap();
        std::fs::remove_file(path).unwrap();
        bytes
    }

    #[test]
    fn specs_without_trails_never_emit_the_seventh_slot() {
        for style in [
            ThreeMfStyle::Project,
            ThreeMfStyle::Painted,
            ThreeMfStyle::Geometry,
        ] {
            let bytes = write_fixture(style);
            let model = model_xml(&bytes);
            // Painted states no colors of its own; the other two carry the
            // six in their group.
            let expected_colors = if style == ThreeMfStyle::Painted { 0 } else { 6 };
            assert_eq!(
                model.matches("<m:color ").count(),
                expected_colors,
                "{style:?} should keep six colors"
            );
            assert!(!model.contains("paint_color=\"4C\""), "{style:?}");
            assert!(!model.contains("#D6336C"), "{style:?}");
            if style == ThreeMfStyle::Project {
                let mut archive = zip::ZipArchive::new(std::io::Cursor::new(&bytes)).unwrap();
                let mut settings = String::new();
                archive
                    .by_name("Metadata/project_settings.config")
                    .unwrap()
                    .read_to_string(&mut settings)
                    .unwrap();
                let settings: serde_json::Value = serde_json::from_str(&settings).unwrap();
                assert_eq!(settings["filament_colour"].as_array().unwrap().len(), 6);
                assert_eq!(
                    settings["flush_volumes_matrix"].as_array().unwrap().len(),
                    36
                );
                assert_eq!(
                    settings["flush_volumes_vector"].as_array().unwrap().len(),
                    12
                );
            }
        }
    }

    #[test]
    fn trail_projects_emit_the_seventh_color_and_paint_code() {
        for style in [ThreeMfStyle::Project, ThreeMfStyle::Painted] {
            let model = model_xml(&write_trail_fixture(style));
            assert!(
                model.contains(" paint_color=\"4C\"/>"),
                "{style:?} should face-paint the trail triangle for extruder 7"
            );
        }
        // Project also states what the seven colors ARE, in its group and
        // its settings; Painted deliberately does not.
        let model = model_xml(&write_trail_fixture(ThreeMfStyle::Project));
        assert_eq!(model.matches("<m:color ").count(), 7);
        assert!(model.contains("color=\"#D6336CFF\""));
        assert!(model.contains("pid=\"1000\" p1=\"6\" p2=\"6\" p3=\"6\" paint_color=\"4C\"/>"));

        // Geometry drops paint codes but keeps the seventh color reference.
        let model = model_xml(&write_trail_fixture(ThreeMfStyle::Geometry));
        assert!(!model.contains("paint_color"));
        assert_eq!(model.matches("<m:color ").count(), 7);
        assert!(model.contains("color=\"#D6336CFF\""));
        assert!(model.contains("pid=\"1000\" p1=\"6\" p2=\"6\" p3=\"6\"/>"));
    }

    #[test]
    fn trail_projects_grow_seven_slot_project_settings() {
        let bytes = write_trail_fixture(ThreeMfStyle::Project);
        let mut archive = zip::ZipArchive::new(std::io::Cursor::new(&bytes)).unwrap();
        let mut settings = String::new();
        archive
            .by_name("Metadata/project_settings.config")
            .unwrap()
            .read_to_string(&mut settings)
            .unwrap();
        let settings: serde_json::Value = serde_json::from_str(&settings).unwrap();
        let colors = settings["filament_colour"].as_array().unwrap();
        assert_eq!(colors.len(), 7);
        assert_eq!(colors[6], "#D6336C");
        for key in ["filament_settings_id", "filament_type", "filament_vendor"] {
            assert_eq!(settings[key].as_array().unwrap().len(), 7, "{key}");
        }
        let matrix = settings["flush_volumes_matrix"].as_array().unwrap();
        assert_eq!(matrix.len(), 49);
        assert_eq!(matrix[0], "0");
        assert_eq!(matrix[48], "0");
        assert_eq!(matrix[1], "280");
        assert_eq!(
            settings["flush_volumes_vector"].as_array().unwrap().len(),
            14
        );

        // Painted and geometry styles keep skipping the settings file even
        // with trails present.
        for style in [ThreeMfStyle::Painted, ThreeMfStyle::Geometry] {
            let names = archive_names(&write_trail_fixture(style));
            assert!(
                !names
                    .iter()
                    .any(|name| name == "Metadata/project_settings.config"),
                "{style:?}"
            );
        }
    }

    fn rail_spec(style: ThreeMfStyle, rail_style: crate::spec::RailStyle) -> GenerationSpec {
        let mut spec = fixture_spec(style);
        spec.color_output.rail_enabled = true;
        spec.color_output.rail_style = rail_style;
        spec
    }

    /// The fixture meshes plus one Rail-material triangle, as a piece with a
    /// separately-styled rail layer would carry.
    fn rail_meshes() -> Vec<Mesh> {
        let mut meshes = fixture_meshes();
        let mesh = &mut meshes[0];
        let base = mesh.vertices.len() as u32;
        mesh.vertices.push([120.0, 0.0, 1.0]);
        mesh.vertices.push([122.5, 0.75, 1.5]);
        mesh.vertices.push([121.25, 3.125, 2.0]);
        mesh.triangles.push([base, base + 1, base + 2]);
        mesh.materials.push(SurfaceClass::Rail);
        meshes
    }

    fn write_rail_fixture(style: ThreeMfStyle, rail_style: crate::spec::RailStyle) -> Vec<u8> {
        static NEXT_FIXTURE: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
        let unique = NEXT_FIXTURE.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "toposaic-3mf-rail-{style:?}-{rail_style:?}-{}-{unique}.3mf",
            std::process::id()
        ));
        let spec = rail_spec(style, rail_style);
        let meshes = if spec.uses_separate_rail() {
            rail_meshes()
        } else {
            fixture_meshes()
        };
        let mut writer = ThreeMfWriter::new(&spec, painted_by(&meshes), &path).unwrap();
        for mesh in &meshes {
            writer.write_mesh(mesh).unwrap();
        }
        writer.finish().unwrap();
        let bytes = std::fs::read(&path).unwrap();
        std::fs::remove_file(path).unwrap();
        bytes
    }

    /// An ordinary map: every base class in the raster, a building, a road,
    /// and exactly the extra overlay lines asked for. The palette is sized
    /// from data now, so a test about optional layers needs a field that
    /// really carries the six a map normally has.
    fn field_with(lines: &[SurfaceClass]) -> SurfaceField {
        let mut field = SurfaceField::new(
            3,
            3,
            vec![
                SurfaceClass::Rock,
                SurfaceClass::Forest,
                SurfaceClass::Snow,
                SurfaceClass::Water,
                SurfaceClass::Rock,
                SurfaceClass::Forest,
                SurfaceClass::Snow,
                SurfaceClass::Water,
                SurfaceClass::Rock,
            ],
            "palette",
        )
        .unwrap();
        field.paint_building(&[[0.4, 0.02], [0.6, 0.02], [0.6, 0.1], [0.4, 0.1]], 12.0);
        for (index, class) in std::iter::once(&SurfaceClass::Road)
            .chain(lines)
            .enumerate()
        {
            let across = 0.15 + index as f32 * 0.17;
            field.paint_polyline(&[[0.05, across], [0.95, across]], 60.0, 1.0, *class);
        }
        field
    }

    fn separate_layers_spec() -> GenerationSpec {
        let mut spec = fixture_spec(ThreeMfStyle::Project);
        spec.color_output.rail_style = crate::spec::RailStyle::Separate;
        spec.color_output.aerial_style = crate::spec::AerialStyle::Separate;
        spec.color_output.ferry_style = crate::spec::FerryStyle::Separate;
        spec
    }

    /// Every slot names a real slicer preset and the vendor that ships it.
    /// An empty id is what left OrcaSlicer and Bambu Studio to guess, and
    /// what they guessed was TPU.
    #[test]
    fn every_slot_asks_for_the_chosen_filament_preset() {
        use crate::spec::FilamentProfile;

        for (profile, preset, vendor) in [
            (FilamentProfile::GenericPla, "Generic PLA", "Generic"),
            (
                FilamentProfile::BambuPlaBasic,
                "Bambu PLA Basic",
                "Bambu Lab",
            ),
            (FilamentProfile::PolyLitePla, "PolyLite PLA", "Polymaker"),
            (FilamentProfile::PolyTerraPla, "PolyTerra PLA", "Polymaker"),
        ] {
            let mut spec = fixture_spec(ThreeMfStyle::Project);
            spec.color_output.filament_profile = profile;
            let settings: serde_json::Value =
                serde_json::from_str(&project_settings(&write_spec_fixture(&spec))).unwrap();
            for (key, expected) in [
                ("filament_settings_id", preset),
                ("filament_vendor", vendor),
                ("filament_type", "PLA"),
            ] {
                let values = settings[key].as_array().unwrap();
                assert_eq!(values.len(), 6, "{profile:?} {key}");
                assert!(
                    values.iter().all(|value| value == expected),
                    "{profile:?} {key} should be {expected}, got {values:?}"
                );
            }
        }
    }

    /// Two classes printed in one color are one spool. No slicer merges
    /// them for us, so the palette must — however the two ended up matching.
    #[test]
    fn classes_sharing_a_color_share_a_slot() {
        let mut spec = fixture_spec(ThreeMfStyle::Project);
        // A wilderness map printed on one spool of grey, bar the water.
        spec.color_output.forest_color = spec.color_output.rock_color.clone();
        spec.color_output.snow_color = spec.color_output.rock_color.to_lowercase();
        spec.color_output.road_color = spec.color_output.rock_color.clone();
        spec.color_output.building_color = spec.color_output.rock_color.clone();

        let palette = spec.material_palette(any_class());
        assert_eq!(palette.len(), 2, "one grey and one blue");
        for class in [
            SurfaceClass::Rock,
            SurfaceClass::Forest,
            SurfaceClass::Snow,
            SurfaceClass::Road,
            SurfaceClass::Building,
        ] {
            assert_eq!(
                palette.slot(class),
                Some(0),
                "{class:?} prints in the grey filament"
            );
        }
        assert_eq!(palette.slot(SurfaceClass::Water), Some(1));

        // And the archive asks for the two, not six — every triangle still
        // painted, none of them pointing past the group.
        let meshes = fixture_meshes();
        let model = model_xml(&write_spec_meshes(&spec, &meshes, painted_by(&meshes)));
        assert_eq!(model.matches("<m:color ").count(), 2);
        assert!(!model.contains("p1=\"2\""));
        assert_eq!(
            model.matches("<triangle ").count(),
            meshes
                .iter()
                .map(|mesh| mesh.triangles.len())
                .sum::<usize>()
        );
    }

    /// An archive that holds its meshes before it opens — a tray, a piece of
    /// hardware, a flag — asks for exactly the colors those meshes paint.
    #[test]
    fn single_mesh_archives_ask_only_for_what_the_mesh_paints() {
        let spec = fixture_spec(ThreeMfStyle::Project);
        let mut mesh = fixture_meshes().remove(0);
        // A tray: rim, contours, label. Nothing else.
        mesh.materials = vec![
            SurfaceClass::Rock,
            SurfaceClass::Forest,
            SurfaceClass::Snow,
            SurfaceClass::Rock,
            SurfaceClass::Forest,
            SurfaceClass::Snow,
        ];

        let path =
            std::env::temp_dir().join(format!("toposaic-3mf-single-{}.3mf", std::process::id()));
        write_single_mesh_3mf(&spec, &mesh, &path).unwrap();
        let model = model_xml(&std::fs::read(&path).unwrap());
        std::fs::remove_file(path).unwrap();

        assert_eq!(model.matches("<m:color ").count(), 3);
        assert!(!model.contains("color=\"#2F76B5FF\""), "no water filament");
        assert!(!model.contains("color=\"#D8A33CFF\""), "no road filament");
        assert!(
            !model.contains("color=\"#B8A890FF\""),
            "no building filament"
        );
    }

    /// The point of sizing the palette from the data: a layer switched on
    /// but with nothing to draw costs nothing. A city map with railways and
    /// no cable cars must not bill a spool for cable cars.
    #[test]
    fn a_layer_with_no_features_in_the_data_takes_no_filament_slot() {
        let spec = separate_layers_spec();
        assert!(spec.uses_separate_rail());
        assert!(spec.uses_separate_aerial());
        // Settings alone would charge for all three separate layers —
        // rail, aerial, and ferry — on top of the base six...
        assert_eq!(spec.material_palette(any_class()).len(), 9);

        // ...but a city with railways and no lifts pays for railways only.
        let city = field_with(&[SurfaceClass::Rail]);
        let palette = spec.material_palette(PaintedClasses::sampled(Some(&city)));
        assert_eq!(palette.len(), 7);
        assert_eq!(palette.slot(SurfaceClass::Rail), Some(6));
        assert_eq!(palette.slot(SurfaceClass::Aerial), None);

        // A ski area with both pays for both, in class order.
        let resort = field_with(&[SurfaceClass::Rail, SurfaceClass::Aerial]);
        let palette = spec.material_palette(PaintedClasses::sampled(Some(&resort)));
        assert_eq!(palette.len(), 8);
        assert_eq!(palette.slot(SurfaceClass::Rail), Some(6));
        assert_eq!(palette.slot(SurfaceClass::Aerial), Some(7));

        // A valley with lifts and no railway pays for lifts only, and the
        // lift color moves up into slot seven rather than leaving a hole.
        let valley = field_with(&[SurfaceClass::Aerial]);
        let palette = spec.material_palette(PaintedClasses::sampled(Some(&valley)));
        assert_eq!(palette.len(), 7);
        assert_eq!(palette.slot(SurfaceClass::Rail), None);
        assert_eq!(palette.slot(SurfaceClass::Aerial), Some(6));

        // And it reaches the archive, not just the palette.
        let model = model_xml(&write_spec_meshes(
            &{
                let mut spec = spec.clone();
                spec.color_output.threemf_style = ThreeMfStyle::Project;
                spec
            },
            &fixture_meshes(),
            PaintedClasses::sampled(Some(&city)),
        ));
        assert_eq!(model.matches("<m:color ").count(), 7);
        assert!(model.contains("color=\"#C43D3DFF\""), "the rail color");
        assert!(
            !model.contains("color=\"#6C4CB6FF\""),
            "no lift color for a map with no lifts"
        );
    }

    #[test]
    fn vector_labels_keep_the_marker_filament_without_marker_pixels() {
        let mut spec = fixture_spec(ThreeMfStyle::Project);
        spec.markers.push(crate::spec::MapMarker {
            name: "North Fork".into(),
            latitude: spec.center_lat,
            longitude: spec.center_lon,
            kind: crate::spec::MarkerKind::SurfaceLabel,
            label_height_mm: 4.0,
            rotation_degrees: 0.0,
            dot_style: None,
            flag_style: None,
            label_style: None,
        });
        let field = field_with(&[]);
        let palette = spec.material_palette(PaintedClasses::sampled(Some(&field)));
        assert_eq!(palette.slot(SurfaceClass::Marker), Some(6));

        let mut meshes = fixture_meshes();
        meshes[0].materials[0] = SurfaceClass::Marker;
        let model = model_xml(&write_spec_meshes(
            &spec,
            &meshes,
            PaintedClasses::sampled(Some(&field)),
        ));
        assert!(model.contains("paint_color=\"4C\""));
    }

    #[test]
    fn vector_dots_keep_the_marker_filament_without_marker_pixels() {
        let mut spec = fixture_spec(ThreeMfStyle::Project);
        spec.markers.push(crate::spec::MapMarker {
            name: "Trailhead".into(),
            latitude: spec.center_lat,
            longitude: spec.center_lon,
            kind: crate::spec::MarkerKind::Dot,
            label_height_mm: 4.0,
            rotation_degrees: 0.0,
            dot_style: None,
            flag_style: None,
            label_style: None,
        });
        let field = field_with(&[]);
        assert_eq!(
            spec.material_palette(PaintedClasses::sampled(Some(&field)))
                .slot(SurfaceClass::Marker),
            Some(6)
        );
    }

    /// The property the refusal check depends on: the palette covers every
    /// class a mesh built from the field can paint. Proved end to end —
    /// real pieces built from a field carrying every class, written through
    /// the real writer, which refuses any triangle whose class has no slot.
    #[test]
    fn the_palette_covers_every_class_a_piece_can_paint() {
        use crate::heightfield::HeightField;
        use crate::piece::build_piece;

        let mut spec = separate_layers_spec();
        spec.rows = 1;
        spec.columns = 1;
        spec.solid_model = true;
        spec.width_mm = 60.0;
        spec.samples_per_piece = 16;
        spec.overlay_samples_per_piece = 32;
        spec.buildings.enabled = true;
        spec.color_output.route_trail_color = Some("#875A2C".into());
        spec.trails = vec![crate::spec::TrailRoute {
            name: "Loop".into(),
            points: vec![[46.8, -121.8], [46.9, -121.7]],
        }];
        spec.markers = vec![crate::spec::MapMarker {
            name: "Point".into(),
            latitude: 46.85,
            longitude: -121.76,
            kind: crate::spec::MarkerKind::Dot,
            label_height_mm: 4.0,
            rotation_degrees: 0.0,
            dot_style: None,
            flag_style: None,
            label_style: None,
        }];

        // Every base class in the raster, every overlay class as vectors,
        // and a building footprint.
        let mut field = SurfaceField::new(
            3,
            3,
            vec![
                SurfaceClass::Rock,
                SurfaceClass::Forest,
                SurfaceClass::Snow,
                SurfaceClass::Water,
                SurfaceClass::Rock,
                SurfaceClass::Forest,
                SurfaceClass::Snow,
                SurfaceClass::Water,
                SurfaceClass::Rock,
            ],
            "every class",
        )
        .unwrap();
        field.paint_polyline(&[[0.05, 0.2], [0.95, 0.2]], 60.0, 1.0, SurfaceClass::Road);
        field.paint_polyline(&[[0.05, 0.4], [0.95, 0.4]], 60.0, 0.8, SurfaceClass::Trail);
        field.paint_polyline(&[[0.05, 0.6], [0.95, 0.6]], 60.0, 0.8, SurfaceClass::Rail);
        field.paint_polyline(&[[0.05, 0.8], [0.95, 0.8]], 60.0, 0.8, SurfaceClass::Aerial);
        field.paint_polyline(&[[0.05, 0.7], [0.95, 0.7]], 60.0, 0.8, SurfaceClass::Ferry);
        field.paint_polyline(
            &[[0.05, 0.9], [0.95, 0.9]],
            60.0,
            0.8,
            SurfaceClass::RouteTrail,
        );
        field.paint_building(&[[0.4, 0.05], [0.6, 0.05], [0.6, 0.12], [0.4, 0.12]], 12.0);
        field.paint_surface_area(
            &[[0.45, 0.45], [0.55, 0.45], [0.55, 0.55], [0.45, 0.55]],
            SurfaceClass::Marker,
        );

        let contained = field.contained_classes();
        assert!(
            contained.iter().all(|present| *present),
            "the fixture field should hold every class"
        );
        let palette = spec.material_palette(PaintedClasses::sampled(Some(&field)));
        assert_eq!(palette.len(), SurfaceClass::ALL.len());

        let height_field = HeightField::new(
            3,
            3,
            vec![0.0, 40.0, 0.0, 40.0, 80.0, 40.0, 0.0, 40.0, 0.0],
            "relief",
        )
        .unwrap();
        let mesh = build_piece(&spec, Some(&height_field), Some(&field), 0, 0).unwrap();
        // The mesh really does exercise the optional layers, so the check
        // below is not vacuous.
        for class in [
            SurfaceClass::Road,
            SurfaceClass::Trail,
            SurfaceClass::Rail,
            SurfaceClass::Aerial,
            SurfaceClass::Ferry,
            SurfaceClass::Building,
            SurfaceClass::Marker,
            SurfaceClass::RouteTrail,
        ] {
            assert!(mesh.materials.contains(&class), "{class:?} should be built");
        }
        // Every class the mesh paints has a slot; the writer refuses
        // otherwise, so reaching the end IS the assertion.
        for material in &mesh.materials {
            assert!(
                palette.slot(*material).is_some(),
                "{material:?} has no filament slot"
            );
        }
        write_spec_meshes(&spec, &[mesh], PaintedClasses::sampled(Some(&field)));
    }

    /// A piece cuts plaque label text in the Snow class and the plaque under
    /// it in Marker, neither of which the surface data reports. Sizing the
    /// palette from data alone would leave a named peak on a snowless map
    /// with nothing to print its own name in, and the writer would refuse
    /// the archive. Proved through the real piece builder and the real
    /// writer, on a field with no snow anywhere in it.
    #[test]
    fn plaque_labels_keep_their_filaments_on_a_map_without_snow() {
        use crate::heightfield::HeightField;
        use crate::piece::build_piece;

        let mut spec = fixture_spec(ThreeMfStyle::Project);
        spec.rows = 1;
        spec.columns = 1;
        spec.solid_model = true;
        spec.width_mm = 60.0;
        spec.samples_per_piece = 16;
        spec.overlay_samples_per_piece = 32;
        spec.markers = vec![crate::spec::MapMarker {
            name: "Peak".into(),
            latitude: spec.center_lat,
            longitude: spec.center_lon,
            kind: crate::spec::MarkerKind::PlaqueLabel,
            label_height_mm: 6.0,
            rotation_degrees: 0.0,
            dot_style: None,
            flag_style: None,
            label_style: None,
        }];

        let field = SurfaceField::new(3, 3, vec![SurfaceClass::Rock; 9], "no snow").unwrap();
        assert!(
            !field.contained_classes()[SurfaceClass::Snow.material_index() as usize],
            "the fixture field must hold no snow"
        );
        let palette = spec.material_palette(PaintedClasses::sampled(Some(&field)));
        assert!(palette.slot(SurfaceClass::Snow).is_some(), "plaque text");
        assert!(palette.slot(SurfaceClass::Marker).is_some(), "the plaque");

        let height = HeightField::new(
            3,
            3,
            vec![0.0, 40.0, 0.0, 40.0, 80.0, 40.0, 0.0, 40.0, 0.0],
            "relief",
        )
        .unwrap();
        let mesh = build_piece(&spec, Some(&height), Some(&field), 0, 0).unwrap();
        for class in [SurfaceClass::Snow, SurfaceClass::Marker] {
            assert!(
                mesh.materials.contains(&class),
                "{class:?} should be built, or this test proves nothing"
            );
        }
        // The writer refuses any triangle whose class has no slot, so
        // reaching the end IS the assertion.
        write_spec_meshes(&spec, &[mesh], PaintedClasses::sampled(Some(&field)));
    }

    /// What the DEFAULT emits now: both rail-family layers in their own
    /// color, eight slots, nothing reserved for a layer that is not drawn.
    ///
    /// This is a deliberate change from the pre-split output, and it is the
    /// change that was asked for. The byte-identity guarantee moved to the
    /// merged styles below; it did not go away.
    #[test]
    fn default_rail_family_settings_emit_both_colors() {
        use crate::spec::{AerialStyle, RailStyle};

        let mut spec = fixture_spec(ThreeMfStyle::Project);
        spec.color_output.rail_style = RailStyle::default();
        spec.color_output.aerial_style = AerialStyle::default();
        assert_eq!(spec.color_output.rail_style, RailStyle::Separate);
        assert_eq!(spec.color_output.aerial_style, AerialStyle::Separate);

        let mut meshes = fixture_meshes();
        for (mesh, class) in meshes
            .iter_mut()
            .zip([SurfaceClass::Rail, SurfaceClass::Aerial])
        {
            let base = mesh.vertices.len() as u32;
            mesh.vertices.push([120.0, 0.0, 1.0]);
            mesh.vertices.push([122.5, 0.75, 1.5]);
            mesh.vertices.push([121.25, 3.125, 2.0]);
            mesh.triangles.push([base, base + 1, base + 2]);
            mesh.materials.push(class);
        }
        let model = model_xml(&write_spec_meshes(&spec, &meshes, painted_by(&meshes)));
        assert_eq!(model.matches("<m:color ").count(), 8);
        assert!(model.contains("color=\"#C43D3DFF\""), "the rail color");
        assert!(model.contains("color=\"#6C4CB6FF\""), "the aerialway color");
        assert!(
            !model.contains("color=\"#D6336CFF\""),
            "no trail placeholder between them"
        );
        // Slots seven and eight, extruders 7 and 8.
        assert!(model.contains("p1=\"6\" p2=\"6\" p3=\"6\" paint_color=\"4C\"/>"));
        assert!(model.contains("p1=\"7\" p2=\"7\" p3=\"7\" paint_color=\"5C\"/>"));
    }

    /// The byte-identity guarantee that survives the default flip: a spec
    /// that picks the MERGED styles — the offered fallback for anyone who
    /// would rather not spend two more spools — produces exactly the
    /// six-slot archive a build before the rail layers existed produced.
    /// Switching either layer off does the same.
    #[test]
    fn merged_rail_family_styles_leave_the_archive_byte_identical() {
        use crate::spec::{AerialStyle, RailLifecycle, RailStyle};

        let baseline = write_fixture(ThreeMfStyle::Project);
        // A document that spells out the merged styles and nothing else.
        let merged: GenerationSpec = serde_json::from_value(serde_json::json!({
            "rows": 2,
            "columns": 2,
            "color_output": {
                "enabled": true,
                "rail_style": "with_roads",
                "aerial_style": "with_roads",
                "ferry_style": "with_roads"
            }
        }))
        .unwrap();
        assert!(merged.color_output.rail_enabled);
        assert!(merged.color_output.aerial_enabled);
        assert_eq!(merged.material_palette(any_class()).len(), 6);
        assert_eq!(write_spec_fixture(&merged), baseline);

        // Every combination that resolves to "paint as roads" is the same
        // archive: switching a layer off, or folding it into another layer
        // that is itself folded into the roads.
        for (rail_enabled, rail_style, aerial_enabled, aerial_style) in [
            (true, RailStyle::WithRoads, true, AerialStyle::WithRail),
            (true, RailStyle::WithRoads, true, AerialStyle::WithRoads),
            (true, RailStyle::WithRoads, false, AerialStyle::Separate),
            (false, RailStyle::Separate, true, AerialStyle::WithRail),
            (false, RailStyle::Separate, false, AerialStyle::Separate),
        ] {
            let mut spec = fixture_spec(ThreeMfStyle::Project);
            spec.color_output.rail_enabled = rail_enabled;
            spec.color_output.rail_style = rail_style;
            spec.color_output.aerial_enabled = aerial_enabled;
            spec.color_output.aerial_style = aerial_style;
            assert_eq!(
                write_spec_fixture(&spec),
                baseline,
                "rail={rail_enabled} {rail_style:?} aerial={aerial_enabled} {aerial_style:?}"
            );
        }

        // The lifecycle setting only changes which ways the API fetches, so
        // it cannot move the archive on its own at any style.
        for lifecycle in [
            RailLifecycle::Operational,
            RailLifecycle::Disused,
            RailLifecycle::Abandoned,
        ] {
            let mut spec = fixture_spec(ThreeMfStyle::Project);
            spec.color_output.rail_lifecycle = lifecycle;
            assert_eq!(write_spec_fixture(&spec), baseline, "{lifecycle:?}");
        }
    }

    /// The byte guard for the seventh slot, in the style of
    /// `specs_without_trails_never_emit_the_seventh_slot`: a rail layer
    /// folded into the roads — and a disabled one — must produce exactly
    /// the archive a build without rail produced.
    #[test]
    fn specs_without_separate_rail_never_emit_an_extra_slot() {
        use crate::spec::RailStyle;

        for style in [
            ThreeMfStyle::Project,
            ThreeMfStyle::Painted,
            ThreeMfStyle::Geometry,
        ] {
            // The default style already has rail enabled; the archive must
            // match the pre-rail six-slot archive byte for byte.
            assert_eq!(
                write_rail_fixture(style, RailStyle::WithRoads),
                write_fixture(style),
                "{style:?} with_roads rail must not change the archive"
            );
            let mut disabled = fixture_spec(style);
            disabled.color_output.rail_enabled = false;
            assert!(!disabled.uses_separate_rail());
            assert_eq!(disabled.material_palette(any_class()).len(), 6);

            let model = model_xml(&write_rail_fixture(style, RailStyle::WithRoads));
            let expected_colors = if style == ThreeMfStyle::Painted { 0 } else { 6 };
            assert_eq!(
                model.matches("<m:color ").count(),
                expected_colors,
                "{style:?} should keep six colors"
            );
            assert!(!model.contains("paint_color=\"4C\""), "{style:?}");
            assert!(!model.contains("#C43D3D"), "{style:?}");
            if style == ThreeMfStyle::Project {
                let bytes = write_rail_fixture(style, RailStyle::WithRoads);
                let mut archive = zip::ZipArchive::new(std::io::Cursor::new(&bytes)).unwrap();
                let mut settings = String::new();
                archive
                    .by_name("Metadata/project_settings.config")
                    .unwrap()
                    .read_to_string(&mut settings)
                    .unwrap();
                let settings: serde_json::Value = serde_json::from_str(&settings).unwrap();
                assert_eq!(settings["filament_colour"].as_array().unwrap().len(), 6);
                assert_eq!(
                    settings["flush_volumes_matrix"].as_array().unwrap().len(),
                    36
                );
                assert_eq!(
                    settings["flush_volumes_vector"].as_array().unwrap().len(),
                    12
                );
            }
        }
    }

    /// A separately-styled rail layer costs exactly ONE slot. Before the
    /// palette became dense the rail color sat at a fixed eighth position
    /// and dragged an unreferenced trail color into slot seven with it; now
    /// it packs straight into slot seven. This is the one place compaction
    /// deliberately changes what a spec emits.
    #[test]
    fn separate_rail_projects_emit_a_seventh_color_with_no_placeholder() {
        use crate::spec::RailStyle;

        for style in [ThreeMfStyle::Project, ThreeMfStyle::Painted] {
            let model = model_xml(&write_rail_fixture(style, RailStyle::Separate));
            assert!(
                model.contains(" paint_color=\"4C\"/>"),
                "{style:?} should face-paint the rail triangle for extruder 7"
            );
            assert!(
                !model.contains("paint_color=\"5C\""),
                "{style:?} must not reach for a slot past the palette"
            );
        }
        // Project also states what the seven colors ARE; no unreferenced
        // trail placeholder among them.
        let model = model_xml(&write_rail_fixture(
            ThreeMfStyle::Project,
            RailStyle::Separate,
        ));
        assert_eq!(model.matches("<m:color ").count(), 7);
        assert!(!model.contains("color=\"#D6336CFF\""));
        assert!(model.contains("color=\"#C43D3DFF\""));
        assert!(model.contains("pid=\"1000\" p1=\"6\" p2=\"6\" p3=\"6\" paint_color=\"4C\"/>"));

        let model = model_xml(&write_rail_fixture(
            ThreeMfStyle::Geometry,
            RailStyle::Separate,
        ));
        assert!(!model.contains("paint_color"));
        assert_eq!(model.matches("<m:color ").count(), 7);
        assert!(model.contains("pid=\"1000\" p1=\"6\" p2=\"6\" p3=\"6\"/>"));
    }

    #[test]
    fn separate_rail_projects_grow_seven_slot_project_settings() {
        use crate::spec::RailStyle;

        let bytes = write_rail_fixture(ThreeMfStyle::Project, RailStyle::Separate);
        let mut archive = zip::ZipArchive::new(std::io::Cursor::new(&bytes)).unwrap();
        let mut settings = String::new();
        archive
            .by_name("Metadata/project_settings.config")
            .unwrap()
            .read_to_string(&mut settings)
            .unwrap();
        let settings: serde_json::Value = serde_json::from_str(&settings).unwrap();
        let colors = settings["filament_colour"].as_array().unwrap();
        assert_eq!(colors.len(), 7);
        assert_eq!(colors[6], "#C43D3D");
        for key in ["filament_settings_id", "filament_type", "filament_vendor"] {
            assert_eq!(settings[key].as_array().unwrap().len(), 7, "{key}");
        }
        let matrix = settings["flush_volumes_matrix"].as_array().unwrap();
        assert_eq!(matrix.len(), 49);
        assert_eq!(matrix[0], "0");
        assert_eq!(matrix[48], "0");
        assert_eq!(matrix[1], "280");
        assert_eq!(
            settings["flush_volumes_vector"].as_array().unwrap().len(),
            14
        );
    }

    /// The archive-wide invariant compaction has to hold: the color group,
    /// the per-triangle property indices, the Orca paint codes, and the
    /// project-settings arrays must all mean the same thing by slot n.
    ///
    /// The check reads the emitted XML back and, for every painted
    /// triangle, walks slot -> color -> extruder and compares against the
    /// spec's own color for that triangle's class. A dense index used
    /// inconsistently anywhere in the chain shows up here as a mis-colored
    /// triangle, which is exactly what it would be on the plate.
    #[test]
    fn every_emitted_slot_agrees_across_the_color_group_and_the_paint_codes() {
        use crate::spec::{AerialStyle, RailStyle};

        let mut spec = fixture_spec(ThreeMfStyle::Project);
        spec.color_output.rail_enabled = true;
        spec.color_output.rail_style = RailStyle::Separate;
        spec.color_output.aerial_enabled = true;
        spec.color_output.aerial_style = AerialStyle::Separate;
        spec.trails = vec![crate::spec::TrailRoute {
            name: "Loop".into(),
            points: vec![[46.8, -121.8], [46.9, -121.7]],
        }];

        // The data the palette is now sized from: every optional layer
        // present, so all nine classes are in play.
        let field = field_with(&[
            SurfaceClass::Trail,
            SurfaceClass::Rail,
            SurfaceClass::Aerial,
        ]);

        // One triangle per class, spread over two meshes so the palette has
        // to be the union across the whole archive rather than per mesh.
        let mut meshes = fixture_meshes();
        for (mesh, class) in meshes.iter_mut().zip([
            [SurfaceClass::Trail, SurfaceClass::Rail],
            [SurfaceClass::Aerial, SurfaceClass::Aerial],
        ]) {
            for class in class {
                let base = mesh.vertices.len() as u32;
                mesh.vertices.push([120.0, 0.0, 1.0]);
                mesh.vertices.push([122.5, 0.75, 1.5]);
                mesh.vertices.push([121.25, 3.125, 2.0]);
                mesh.triangles.push([base, base + 1, base + 2]);
                mesh.materials.push(class);
            }
        }

        let path =
            std::env::temp_dir().join(format!("toposaic-3mf-agree-{}.3mf", std::process::id()));
        let mut writer =
            ThreeMfWriter::new(&spec, PaintedClasses::sampled(Some(&field)), &path).unwrap();
        let emitted = meshes
            .iter()
            .flat_map(|mesh| mesh.materials.clone())
            .collect::<Vec<_>>();
        for mesh in &meshes {
            writer.write_mesh(mesh).unwrap();
        }
        writer.finish().unwrap();
        let bytes = std::fs::read(&path).unwrap();
        std::fs::remove_file(path).unwrap();
        let model = model_xml(&bytes);

        // Slot -> color, read back out of the emitted color group.
        let group = model
            .split("<m:colorgroup")
            .nth(1)
            .unwrap()
            .split("</m:colorgroup>")
            .next()
            .unwrap();
        let group_colors = group
            .match_indices("color=\"")
            .map(|(index, marker)| group[index + marker.len()..][..7].to_owned())
            .collect::<Vec<_>>();
        assert_eq!(group_colors.len(), 9, "every class this spec emits");

        // Slot -> extruder, read back out of the per-triangle attributes,
        // in the order the meshes were written.
        let painted = model
            .lines()
            .filter(|line| line.contains("<triangle "))
            .map(|line| {
                let slot = line
                    .split("p1=\"")
                    .nth(1)
                    .unwrap()
                    .split('"')
                    .next()
                    .unwrap()
                    .parse::<usize>()
                    .unwrap();
                let paint = line.split("paint_color=\"").nth(1).unwrap();
                (slot, paint.split('"').next().unwrap().to_owned())
            })
            .collect::<Vec<_>>();
        assert_eq!(painted.len(), emitted.len());
        for ((slot, paint), class) in painted.into_iter().zip(emitted) {
            let palette = spec.material_palette(PaintedClasses::sampled(Some(&field)));
            assert_eq!(
                group_colors[slot],
                palette.colors()[slot],
                "color group slot {slot}"
            );
            assert_eq!(
                group_colors[slot],
                palette.colors()[palette.slot(class).unwrap() as usize],
                "{class:?} took the wrong slot"
            );
            assert_eq!(paint, ORCA_PAINT_CODES[slot], "{class:?} paint code");
        }

        // The project settings size themselves from the same palette.
        let mut archive = zip::ZipArchive::new(std::io::Cursor::new(&bytes)).unwrap();
        let mut settings = String::new();
        archive
            .by_name("Metadata/project_settings.config")
            .unwrap()
            .read_to_string(&mut settings)
            .unwrap();
        let settings: serde_json::Value = serde_json::from_str(&settings).unwrap();
        assert_eq!(
            settings["filament_colour"].as_array().unwrap().len(),
            group_colors.len()
        );
        assert_eq!(
            settings["flush_volumes_matrix"].as_array().unwrap().len(),
            group_colors.len() * group_colors.len()
        );
    }

    /// A mesh painting a class the spec's palette does not carry is a
    /// generator bug that would silently print in another filament's color.
    /// It must fail the write instead.
    #[test]
    fn meshes_painting_a_class_outside_the_palette_fail_the_write() {
        let path = std::env::temp_dir().join(format!(
            "toposaic-3mf-offpalette-{}.3mf",
            std::process::id()
        ));
        let spec = fixture_spec(ThreeMfStyle::Project);
        assert_eq!(spec.material_palette(any_class()).len(), 6);
        let mut writer = ThreeMfWriter::new(&spec, any_class(), &path).unwrap();
        let mut mesh = fixture_meshes().remove(0);
        mesh.materials[0] = SurfaceClass::Aerial;
        let error = writer.write_mesh(&mesh).unwrap_err().to_string();
        assert!(error.contains("filament palette"), "{error}");
        drop(writer);
        let _ = std::fs::remove_file(path);
    }

    /// A painted model is a project stripped to its paint: same geometry,
    /// same per-triangle extruder assignments, none of the color statements.
    #[test]
    fn painted_style_is_the_project_stripped_to_its_paint() {
        let painted = model_xml(&write_fixture(ThreeMfStyle::Painted));
        let project = model_xml(&write_fixture(ThreeMfStyle::Project));
        // Removing the group references and the namespace from the project
        // model yields the painted model exactly, so the two can never
        // disagree on geometry or on which extruder paints a triangle.
        let stripped = project
            .replace(
                " xmlns:m=\"http://schemas.microsoft.com/3dmanufacturing/material/2015/02\" requiredextensions=\"m\"",
                "",
            )
            .lines()
            .filter(|line| !line.contains("<m:color") && !line.contains("</m:colorgroup>"))
            .map(|line| {
                let mut line = line.to_owned();
                if let (Some(start), Some(end)) = (line.find(" pid=\""), line.find(" paint_color=\""))
                {
                    line.replace_range(start..end, "");
                }
                line
            })
            .collect::<Vec<_>>()
            .join("\n");
        assert_eq!(painted, stripped.trim_end_matches('\n'));
    }
}
