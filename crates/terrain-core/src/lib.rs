use std::{
    fs::{self, File},
    io::{BufWriter, Write},
    path::{Path, PathBuf},
};

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use zip::{ZipWriter, write::SimpleFileOptions};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct GenerationSpec {
    pub center_lat: f64,
    pub center_lon: f64,
    pub ground_span_km: f64,
    pub width_mm: f32,
    pub rows: u32,
    pub columns: u32,
    pub base_mm: f32,
    pub relief_mm: f32,
    pub clearance_mm: f32,
    pub samples_per_piece: u32,
}

impl Default for GenerationSpec {
    fn default() -> Self {
        Self {
            center_lat: 46.8523,
            center_lon: -121.7603,
            ground_span_km: 18.0,
            width_mm: 180.0,
            rows: 3,
            columns: 3,
            base_mm: 2.4,
            relief_mm: 14.0,
            clearance_mm: 0.22,
            samples_per_piece: 28,
        }
    }
}

impl GenerationSpec {
    pub fn validate(&self) -> Result<()> {
        if !(-85.0..=85.0).contains(&self.center_lat) {
            bail!("center latitude must be between -85 and 85 degrees");
        }
        if !(-180.0..=180.0).contains(&self.center_lon) {
            bail!("center longitude must be between -180 and 180 degrees");
        }
        if !(0.5..=250.0).contains(&self.ground_span_km) {
            bail!("ground span must be between 0.5 and 250 km");
        }
        if !(60.0..=500.0).contains(&self.width_mm) {
            bail!("model width must be between 60 and 500 mm");
        }
        if !(2..=8).contains(&self.rows) || !(2..=8).contains(&self.columns) {
            bail!("piece rows and columns must each be between 2 and 8");
        }
        if !(1.0..=12.0).contains(&self.base_mm) {
            bail!("base depth must be between 1 and 12 mm");
        }
        if !(1.0..=80.0).contains(&self.relief_mm) {
            bail!("relief must be between 1 and 80 mm");
        }
        if !(0.0..=0.8).contains(&self.clearance_mm) {
            bail!("clearance must be between 0 and 0.8 mm");
        }
        if !(8..=96).contains(&self.samples_per_piece) {
            bail!("samples per piece must be between 8 and 96");
        }
        Ok(())
    }

    pub fn height_mm(&self) -> f32 {
        self.width_mm * self.rows as f32 / self.columns as f32
    }
}

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
    pub spec: GenerationSpec,
    pub artifacts: Vec<Artifact>,
}

#[derive(Debug, Clone)]
pub struct HeightField {
    pub width: usize,
    pub height: usize,
    pub values_m: Vec<f32>,
    pub source: String,
}

impl HeightField {
    pub fn new(
        width: usize,
        height: usize,
        values_m: Vec<f32>,
        source: impl Into<String>,
    ) -> Result<Self> {
        if width < 2 || height < 2 {
            bail!("height field must be at least 2 by 2");
        }
        if values_m.len() != width * height {
            bail!("height field dimensions do not match its values");
        }
        if values_m.iter().any(|value| !value.is_finite()) {
            bail!("height field contains a non-finite value");
        }
        Ok(Self {
            width,
            height,
            values_m,
            source: source.into(),
        })
    }

    fn normalized_at(&self, u: f32, v: f32, minimum: f32, range: f32) -> f32 {
        let x = u.clamp(0.0, 1.0) * (self.width - 1) as f32;
        let y = v.clamp(0.0, 1.0) * (self.height - 1) as f32;
        let x0 = x.floor() as usize;
        let y0 = y.floor() as usize;
        let x1 = (x0 + 1).min(self.width - 1);
        let y1 = (y0 + 1).min(self.height - 1);
        let tx = x - x0 as f32;
        let ty = y - y0 as f32;
        let sample =
            |sample_x: usize, sample_y: usize| self.values_m[sample_y * self.width + sample_x];
        let bottom = sample(x0, y0) * (1.0 - tx) + sample(x1, y0) * tx;
        let top = sample(x0, y1) * (1.0 - tx) + sample(x1, y1) * tx;
        ((bottom * (1.0 - ty) + top * ty - minimum) / range).clamp(0.0, 1.0)
    }

    fn range(&self) -> (f32, f32) {
        let minimum = self.values_m.iter().copied().fold(f32::INFINITY, f32::min);
        let maximum = self
            .values_m
            .iter()
            .copied()
            .fold(f32::NEG_INFINITY, f32::max);
        (minimum, (maximum - minimum).max(1.0))
    }
}

#[derive(Debug, Clone)]
struct Mesh {
    name: String,
    vertices: Vec<[f32; 3]>,
    triangles: Vec<[u32; 3]>,
}

pub fn generate_project(spec: &GenerationSpec, output_dir: &Path) -> Result<ProjectManifest> {
    generate_project_inner(spec, None, output_dir)
}

pub fn generate_project_with_height_field(
    spec: &GenerationSpec,
    height_field: &HeightField,
    output_dir: &Path,
) -> Result<ProjectManifest> {
    generate_project_inner(spec, Some(height_field), output_dir)
}

fn generate_project_inner(
    spec: &GenerationSpec,
    height_field: Option<&HeightField>,
    output_dir: &Path,
) -> Result<ProjectManifest> {
    spec.validate()?;
    fs::create_dir_all(output_dir)
        .with_context(|| format!("create output directory {}", output_dir.display()))?;

    let mut meshes = Vec::with_capacity((spec.rows * spec.columns) as usize);
    for row in 0..spec.rows {
        for column in 0..spec.columns {
            meshes.push(build_piece(spec, height_field, row, column));
        }
    }

    let mut artifacts = Vec::new();
    for (index, mesh) in meshes.iter().enumerate() {
        let row = index as u32 / spec.columns + 1;
        let column = index as u32 % spec.columns + 1;
        let name = format!("piece-{row}-{column}.stl");
        let path = output_dir.join(&name);
        write_binary_stl(mesh, &path)?;
        artifacts.push(file_artifact(&path, "model/stl")?);
    }

    let project_path = output_dir.join("terrain-puzzle.3mf");
    write_3mf(spec, &meshes, &project_path)?;
    artifacts.push(file_artifact(&project_path, "model/3mf")?);

    let preview_path = output_dir.join("preview.json");
    let preview = build_preview(spec, height_field, 42);
    fs::write(&preview_path, serde_json::to_vec(&preview)?)
        .with_context(|| format!("write {}", preview_path.display()))?;
    artifacts.push(file_artifact(&preview_path, "application/json")?);

    let manifest = ProjectManifest {
        generator: format!("terrain-puzzle/{}", env!("CARGO_PKG_VERSION")),
        terrain_source: height_field
            .map(|field| field.source.clone())
            .unwrap_or_else(|| "deterministic-preview-surface".into()),
        spec: spec.clone(),
        artifacts,
    };
    let manifest_path = output_dir.join("manifest.json");
    fs::write(&manifest_path, serde_json::to_vec_pretty(&manifest)?)
        .with_context(|| format!("write {}", manifest_path.display()))?;

    let mut complete = manifest;
    complete
        .artifacts
        .push(file_artifact(&manifest_path, "application/json")?);
    Ok(complete)
}

fn build_piece(
    spec: &GenerationSpec,
    height_field: Option<&HeightField>,
    row: u32,
    column: u32,
) -> Mesh {
    let samples = spec.samples_per_piece as usize;
    let stride = samples + 1;
    let top_count = stride * stride;
    let piece_width = spec.width_mm / spec.columns as f32;
    let piece_height = spec.height_mm() / spec.rows as f32;
    let origin_x = column as f32 * piece_width;
    let origin_y = row as f32 * piece_height;
    let assembled_width = spec.width_mm;
    let assembled_height = spec.height_mm();
    let height_range = height_field.map(HeightField::range);

    let mut vertices = Vec::with_capacity(top_count * 2);
    for layer in 0..2 {
        for y in 0..=samples {
            let v = y as f32 / samples as f32;
            for x in 0..=samples {
                let u = x as f32 / samples as f32;
                let [assembled_x, assembled_y] = map_piece_point(spec, row, column, u, v, false);
                let local_x = assembled_x - origin_x;
                let local_y = assembled_y - origin_y;
                let z = if layer == 0 {
                    spec.base_mm
                        + spec.relief_mm
                            * normalized_height(
                                height_field,
                                height_range,
                                assembled_x / assembled_width,
                                assembled_y / assembled_height,
                                spec.center_lat,
                                spec.center_lon,
                            )
                } else {
                    0.0
                };
                vertices.push([local_x, local_y, z]);
            }
        }
    }

    let mut triangles = Vec::with_capacity(samples * samples * 4 + samples * 8);
    for y in 0..samples {
        for x in 0..samples {
            let a = (y * stride + x) as u32;
            let b = a + 1;
            let c = a + stride as u32;
            let d = c + 1;
            triangles.push([a, b, d]);
            triangles.push([a, d, c]);

            let offset = top_count as u32;
            triangles.push([offset + a, offset + d, offset + b]);
            triangles.push([offset + a, offset + c, offset + d]);
        }
    }

    let mut add_side = |a: usize, b: usize| {
        let top_a = a as u32;
        let top_b = b as u32;
        let bottom_a = (top_count + a) as u32;
        let bottom_b = (top_count + b) as u32;
        triangles.push([top_a, bottom_b, top_b]);
        triangles.push([top_a, bottom_a, bottom_b]);
    };

    for x in 0..samples {
        add_side(x + 1, x);
        let top_row = samples * stride;
        add_side(top_row + x, top_row + x + 1);
    }
    for y in 0..samples {
        add_side(y * stride, (y + 1) * stride);
        add_side((y + 1) * stride + samples, y * stride + samples);
    }

    Mesh {
        name: format!("Piece {}-{}", row + 1, column + 1),
        vertices,
        triangles,
    }
}

fn map_piece_point(
    spec: &GenerationSpec,
    row: u32,
    column: u32,
    u: f32,
    v: f32,
    exact_shared_edge: bool,
) -> [f32; 2] {
    let piece_width = spec.width_mm / spec.columns as f32;
    let piece_height = spec.height_mm() / spec.rows as f32;
    let x0 = column as f32 * piece_width;
    let x1 = x0 + piece_width;
    let y0 = row as f32 * piece_height;
    let y1 = y0 + piece_height;
    let tab_depth = piece_width.min(piece_height) * 0.13;

    let left = [
        x0 + vertical_edge_offset(row, column, spec.columns, v, tab_depth),
        y0 + v * piece_height,
    ];
    let right = [
        x1 + vertical_edge_offset(row, column + 1, spec.columns, v, tab_depth),
        y0 + v * piece_height,
    ];
    let bottom = [
        x0 + u * piece_width,
        y0 + horizontal_edge_offset(column, row, spec.rows, u, tab_depth),
    ];
    let top = [
        x0 + u * piece_width,
        y1 + horizontal_edge_offset(column, row + 1, spec.rows, u, tab_depth),
    ];

    let bilinear = [x0 + u * piece_width, y0 + v * piece_height];
    let mut point = [
        (1.0 - u) * left[0] + u * right[0] + (1.0 - v) * bottom[0] + v * top[0] - bilinear[0],
        (1.0 - u) * left[1] + u * right[1] + (1.0 - v) * bottom[1] + v * top[1] - bilinear[1],
    ];

    if !exact_shared_edge && spec.clearance_mm > 0.0 {
        let center_x = (x0 + x1) * 0.5;
        let center_y = (y0 + y1) * 0.5;
        let scale_x = ((piece_width - spec.clearance_mm) / piece_width).max(0.95);
        let scale_y = ((piece_height - spec.clearance_mm) / piece_height).max(0.95);
        point[0] = center_x + (point[0] - center_x) * scale_x;
        point[1] = center_y + (point[1] - center_y) * scale_y;
    }
    point
}

fn vertical_edge_offset(row: u32, edge_column: u32, columns: u32, t: f32, depth: f32) -> f32 {
    if edge_column == 0 || edge_column == columns {
        return 0.0;
    }
    let sign = if (row + edge_column).is_multiple_of(2) {
        1.0
    } else {
        -1.0
    };
    sign * depth * tab_profile(t)
}

fn horizontal_edge_offset(column: u32, edge_row: u32, rows: u32, t: f32, depth: f32) -> f32 {
    if edge_row == 0 || edge_row == rows {
        return 0.0;
    }
    let sign = if (column + edge_row).is_multiple_of(2) {
        1.0
    } else {
        -1.0
    };
    sign * depth * tab_profile(t)
}

fn tab_profile(t: f32) -> f32 {
    const START: f32 = 0.22;
    const END: f32 = 0.78;
    if !(START..=END).contains(&t) {
        return 0.0;
    }
    let phase = (t - START) / (END - START);
    (std::f32::consts::PI * phase).sin().powi(2)
}

fn terrain_height(u: f32, v: f32, lat: f64, lon: f64) -> f32 {
    let u = u.clamp(0.0, 1.0);
    let v = v.clamp(0.0, 1.0);
    let seed_a = (lat as f32).to_radians().sin() * 1.7;
    let seed_b = (lon as f32).to_radians().cos() * 1.3;
    let ridge = ((u * 9.2 + seed_a) * 1.2).sin() * 0.19 + ((v * 7.1 - seed_b) * 1.4).cos() * 0.14;
    let folds = ((u * 3.8 + v * 5.6 + seed_b) * std::f32::consts::PI)
        .sin()
        .abs()
        * 0.17;
    let dx = u - (0.54 + seed_b * 0.05);
    let dy = v - (0.48 + seed_a * 0.05);
    let peak = (-((dx * dx * 5.5) + (dy * dy * 7.0))).exp() * 0.63;
    (0.12 + ridge + folds + peak).clamp(0.03, 1.0)
}

fn normalized_height(
    height_field: Option<&HeightField>,
    range: Option<(f32, f32)>,
    u: f32,
    v: f32,
    lat: f64,
    lon: f64,
) -> f32 {
    match (height_field, range) {
        (Some(field), Some((minimum, span))) => field.normalized_at(u, v, minimum, span),
        _ => terrain_height(u, v, lat, lon),
    }
}

fn build_preview(
    spec: &GenerationSpec,
    height_field: Option<&HeightField>,
    size: usize,
) -> serde_json::Value {
    let mut heights = Vec::with_capacity(size * size);
    let range = height_field.map(HeightField::range);
    for y in 0..size {
        for x in 0..size {
            heights.push(normalized_height(
                height_field,
                range,
                x as f32 / (size - 1) as f32,
                y as f32 / (size - 1) as f32,
                spec.center_lat,
                spec.center_lon,
            ));
        }
    }
    serde_json::json!({
        "width": size,
        "height": size,
        "values": heights,
        "rows": spec.rows,
        "columns": spec.columns,
    })
}

fn write_binary_stl(mesh: &Mesh, path: &Path) -> Result<()> {
    let mut writer = BufWriter::new(
        File::create(path).with_context(|| format!("create STL {}", path.display()))?,
    );
    let mut header = [0_u8; 80];
    let label = format!("Terrain Puzzle — {}", mesh.name);
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

fn write_3mf(spec: &GenerationSpec, meshes: &[Mesh], path: &Path) -> Result<()> {
    let file = File::create(path).with_context(|| format!("create 3MF {}", path.display()))?;
    let mut zip = ZipWriter::new(file);
    let options = SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);

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

    let mut model = String::from(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<model unit="millimeter" xml:lang="en-US" xmlns="http://schemas.microsoft.com/3dmanufacturing/core/2015/02">
  <metadata name="Title">Terrain Puzzle</metadata>
  <metadata name="Designer">Terrain Puzzle Generator</metadata>
  <resources>
"#,
    );

    for (index, mesh) in meshes.iter().enumerate() {
        model.push_str(&format!(
            "    <object id=\"{}\" name=\"{}\" type=\"model\"><mesh><vertices>\n",
            index + 1,
            mesh.name
        ));
        for vertex in &mesh.vertices {
            model.push_str(&format!(
                "      <vertex x=\"{:.5}\" y=\"{:.5}\" z=\"{:.5}\"/>\n",
                vertex[0], vertex[1], vertex[2]
            ));
        }
        model.push_str("    </vertices><triangles>\n");
        for triangle in &mesh.triangles {
            model.push_str(&format!(
                "      <triangle v1=\"{}\" v2=\"{}\" v3=\"{}\"/>\n",
                triangle[0], triangle[1], triangle[2]
            ));
        }
        model.push_str("    </triangles></mesh></object>\n");
    }
    model.push_str("  </resources>\n  <build>\n");

    let piece_width = spec.width_mm / spec.columns as f32;
    let piece_height = spec.height_mm() / spec.rows as f32;
    let spacing = piece_width.min(piece_height) * 0.3;
    for (index, _) in meshes.iter().enumerate() {
        let row = index as u32 / spec.columns;
        let column = index as u32 % spec.columns;
        let tx = column as f32 * (piece_width + spacing);
        let ty = row as f32 * (piece_height + spacing);
        model.push_str(&format!(
            "    <item objectid=\"{}\" transform=\"1 0 0 0 1 0 0 0 1 {:.5} {:.5} 0\"/>\n",
            index + 1,
            tx,
            ty
        ));
    }
    model.push_str("  </build>\n</model>");

    zip.add_directory("3D/", options)?;
    zip.start_file("3D/3dmodel.model", options)?;
    zip.write_all(model.as_bytes())?;
    zip.finish()?;
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
    use std::collections::HashMap;

    #[test]
    fn shared_edges_are_identical_before_clearance() {
        let spec = GenerationSpec::default();
        for sample in 0..=32 {
            let t = sample as f32 / 32.0;
            let left = map_piece_point(&spec, 1, 1, 1.0, t, true);
            let right = map_piece_point(&spec, 1, 2, 0.0, t, true);
            assert!((left[0] - right[0]).abs() < 0.0001);
            assert!((left[1] - right[1]).abs() < 0.0001);
        }
    }

    #[test]
    fn generated_piece_is_watertight() {
        let mesh = build_piece(&GenerationSpec::default(), None, 0, 0);
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
    }

    #[test]
    fn project_writes_print_artifacts() {
        let output_dir =
            std::env::temp_dir().join(format!("terrain-puzzle-core-test-{}", std::process::id()));
        if output_dir.exists() {
            std::fs::remove_dir_all(&output_dir).unwrap();
        }

        let spec = GenerationSpec {
            rows: 2,
            columns: 2,
            samples_per_piece: 8,
            ..GenerationSpec::default()
        };
        let manifest = generate_project(&spec, &output_dir).unwrap();

        assert!(output_dir.join("terrain-puzzle.3mf").is_file());
        assert!(output_dir.join("piece-1-1.stl").is_file());
        assert!(output_dir.join("preview.json").is_file());
        assert_eq!(
            manifest
                .artifacts
                .iter()
                .filter(|artifact| artifact.name.ends_with(".stl"))
                .count(),
            4
        );

        std::fs::remove_dir_all(output_dir).unwrap();
    }
}
