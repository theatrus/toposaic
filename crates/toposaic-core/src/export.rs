use std::{
    fs::File,
    io::{BufWriter, Write},
    path::Path,
};

use anyhow::{Context, Result};
use rayon::prelude::*;
use zip::{ZipWriter, write::SimpleFileOptions};

use crate::mesh::Mesh;
use crate::spec::GenerationSpec;

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
                        if uses_color {
                            let index = material.material_index();
                            let paint_color = ORCA_PAINT_CODES[index as usize];
                            writeln!(
                                buffer,
                                "      <triangle v1=\"{}\" v2=\"{}\" v3=\"{}\" pid=\"{COLOR_GROUP_ID}\" p1=\"{index}\" p2=\"{index}\" p3=\"{index}\" paint_color=\"{paint_color}\"/>",
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
        if self.spec.uses_color_materials() {
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
