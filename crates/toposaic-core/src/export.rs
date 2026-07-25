use std::{
    fs::File,
    io::{BufWriter, Write},
    path::Path,
};

use anyhow::{Context, Result};
use rayon::prelude::*;
use zip::{ZipWriter, write::SimpleFileOptions};

use crate::mesh::Mesh;
use crate::spec::{GenerationSpec, ThreeMfStyle};

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
    object_count: usize,
}

const COLOR_GROUP_ID: u32 = 1000;
/// Elements formatted per rayon task when writing 3MF XML bodies.
const FORMAT_CHUNK_ELEMENTS: usize = 64 * 1024;
/// Elements formatted per in-memory batch; keeps peak buffered XML text to
/// a few tens of megabytes even for meshes with millions of triangles.
const WRITE_BATCH_ELEMENTS: usize = 1024 * 1024;
// OrcaSlicer and Bambu Studio use these face-paint values for extruders 1–6.
// Keep the standard 3MF color properties too, for consumers that support them.
const ORCA_PAINT_CODES: [&str; 6] = ["4", "8", "0C", "1C", "2C", "3C"];

impl<'a> ThreeMfWriter<'a> {
    pub(crate) fn new(spec: &'a GenerationSpec, path: &Path) -> Result<Self> {
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
        if spec.uses_color_materials() {
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
        if spec.uses_color_materials() {
            writeln!(
                zip,
                "    <m:colorgroup id=\"{COLOR_GROUP_ID}\">\n      <m:color color=\"{}FF\"/>\n      <m:color color=\"{}FF\"/>\n      <m:color color=\"{}FF\"/>\n      <m:color color=\"{}FF\"/>\n      <m:color color=\"{}FF\"/>\n      <m:color color=\"{}FF\"/>\n    </m:colorgroup>",
                spec.color_output.rock_color,
                spec.color_output.forest_color,
                spec.color_output.snow_color,
                spec.color_output.water_color,
                spec.color_output.road_color,
                spec.color_output.building_color,
            )?;
        }
        Ok(Self {
            zip,
            spec,
            object_count: 0,
        })
    }

    pub(crate) fn write_mesh(&mut self, mesh: &Mesh) -> Result<()> {
        debug_assert_eq!(mesh.triangles.len(), mesh.materials.len());
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
        // `Geometry` style keeps the core-spec color group references but
        // drops the OrcaSlicer/Bambu `paint_color` vendor attribute. The
        // painted-triangle branch below is the exact pre-style code path, so
        // `Painted` and `Project` archives keep their previous bytes.
        let paints = self.spec.color_output.threemf_style != ThreeMfStyle::Geometry;
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
                        if uses_color && paints {
                            let index = material.material_index();
                            let paint_color = ORCA_PAINT_CODES[index as usize];
                            writeln!(
                                buffer,
                                "      <triangle v1=\"{}\" v2=\"{}\" v3=\"{}\" pid=\"{COLOR_GROUP_ID}\" p1=\"{index}\" p2=\"{index}\" p3=\"{index}\" paint_color=\"{paint_color}\"/>",
                                triangle[0], triangle[1], triangle[2],
                            )
                            .expect("writing to a Vec cannot fail");
                        } else if uses_color {
                            let index = material.material_index();
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
            let colors = [
                self.spec.color_output.rock_color.as_str(),
                self.spec.color_output.forest_color.as_str(),
                self.spec.color_output.snow_color.as_str(),
                self.spec.color_output.water_color.as_str(),
                self.spec.color_output.road_color.as_str(),
                self.spec.color_output.building_color.as_str(),
            ];
            let flush_volumes_matrix = (0..colors.len())
                .flat_map(|row| {
                    (0..colors.len()).map(move |column| if row == column { "0" } else { "280" })
                })
                .collect::<Vec<_>>();
            let project_settings = serde_json::json!({
                "default_filament_colour": colors,
                "filament_colour": colors,
                "filament_settings_id": ["", "", "", "", "", ""],
                "filament_type": ["PLA", "PLA", "PLA", "PLA", "PLA", "PLA"],
                "filament_vendor": [
                    "(Undefined)",
                    "(Undefined)",
                    "(Undefined)",
                    "(Undefined)",
                    "(Undefined)",
                    "(Undefined)"
                ],
                "flush_volumes_matrix": flush_volumes_matrix,
                "flush_volumes_vector": [
                    "140", "140", "140", "140", "140", "140",
                    "140", "140", "140", "140", "140", "140"
                ],
            });
            serde_json::to_writer_pretty(&mut self.zip, &project_settings)?;
        }
        self.zip.finish()?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use std::io::Read;

    use super::*;
    use crate::spec::{ColorOutputSpec, SurfaceClass};

    fn fixture_spec(style: ThreeMfStyle) -> GenerationSpec {
        GenerationSpec {
            rows: 2,
            columns: 2,
            color_output: ColorOutputSpec {
                enabled: true,
                threemf_style: style,
                ..ColorOutputSpec::default()
            },
            ..GenerationSpec::default()
        }
    }

    /// Two small meshes whose triangles cover every surface class. The
    /// project-style golden fixture was generated from these exact meshes
    /// with the pre-style writer, so do not change them.
    fn fixture_meshes() -> Vec<Mesh> {
        (0..2u32)
            .map(|piece| {
                let mut vertices = Vec::new();
                let mut triangles = Vec::new();
                let mut materials = Vec::new();
                for (index, class) in SurfaceClass::ALL.into_iter().enumerate() {
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

    fn write_fixture(style: ThreeMfStyle) -> Vec<u8> {
        static NEXT_FIXTURE: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
        let unique = NEXT_FIXTURE.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "toposaic-3mf-style-{style:?}-{}-{unique}.3mf",
            std::process::id()
        ));
        let spec = fixture_spec(style);
        let mut writer = ThreeMfWriter::new(&spec, &path).unwrap();
        for mesh in fixture_meshes() {
            writer.write_mesh(&mesh).unwrap();
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

    fn model_xml(bytes: &[u8]) -> String {
        let mut archive = zip::ZipArchive::new(std::io::Cursor::new(bytes)).unwrap();
        let mut model = String::new();
        archive
            .by_name("3D/3dmodel.model")
            .unwrap()
            .read_to_string(&mut model)
            .unwrap();
        model
    }

    /// The default `Project` style must keep producing the exact archive the
    /// pre-style writer produced. The golden fixture was written by the
    /// writer as it stood before `ThreeMfStyle` existed (commit 6e4b1a0),
    /// from `fixture_spec`/`fixture_meshes` above; deterministic zip
    /// timestamps (a constant 1980 date without the `time` feature) and the
    /// pure-Rust zlib-rs deflate make whole-archive comparison stable.
    #[test]
    fn project_style_output_is_byte_identical_to_pre_style_writer() {
        let golden = include_bytes!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/testdata/project-style-golden.3mf"
        ));
        assert_eq!(write_fixture(ThreeMfStyle::Project), golden.as_slice());
        // The default style is `Project`, so an untouched spec gets the
        // same bytes too.
        assert_eq!(
            fixture_spec(ThreeMfStyle::default())
                .color_output
                .threemf_style,
            ThreeMfStyle::Project
        );
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

    #[test]
    fn painted_and_project_styles_keep_paint_codes_and_geometry_drops_them() {
        for style in [ThreeMfStyle::Painted, ThreeMfStyle::Project] {
            let model = model_xml(&write_fixture(style));
            for code in ORCA_PAINT_CODES {
                assert!(
                    model.contains(&format!(" paint_color=\"{code}\"/>")),
                    "{style:?} should carry paint code {code}"
                );
            }
            assert!(model.contains("<m:colorgroup id=\"1000\">"));
            assert!(model.contains("pid=\"1000\" p1=\"5\" p2=\"5\" p3=\"5\""));
        }

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
    fn painted_style_differs_from_project_only_by_the_settings_file() {
        // Same model XML, different archive: the paint codes stay, only the
        // embedded slicer settings go away.
        let painted = write_fixture(ThreeMfStyle::Painted);
        let project = write_fixture(ThreeMfStyle::Project);
        assert_eq!(model_xml(&painted), model_xml(&project));
        assert_ne!(painted, project);
    }
}
