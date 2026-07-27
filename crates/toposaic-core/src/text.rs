//! Vector text embossing: parses the bundled font, flattens glyph outlines
//! into contours, and extrudes them onto a mesh. The tray label is the first
//! use; anything that needs raised text on a mesh can reuse this.

use anyhow::{Result, anyhow};
use spade::{Point2, Triangulation};
use ttf_parser::{Face, GlyphId, OutlineBuilder};

use crate::mesh::{
    MeshBuilder, distance_squared, point_in_polygon, point_line_distance, triangulate_constraints,
};
use crate::spec::SurfaceClass;

const LATIN_FONT: &[u8] = include_bytes!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../assets/fonts/AtkinsonHyperlegible-Regular.ttf"
));
const CJK_FONT: &[u8] = include_bytes!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../assets/fonts/NotoSansJP-Regular.otf"
));

pub(crate) struct EmbossingFonts {
    latin: Face<'static>,
    cjk: Face<'static>,
}

impl EmbossingFonts {
    fn glyph(&self, character: char) -> Option<(&Face<'static>, GlyphId)> {
        self.latin
            .glyph_index(character)
            .map(|glyph_id| (&self.latin, glyph_id))
            .or_else(|| {
                self.cjk
                    .glyph_index(character)
                    .map(|glyph_id| (&self.cjk, glyph_id))
            })
    }
}

pub(crate) fn embossing_fonts() -> Result<EmbossingFonts> {
    let latin = Face::parse(LATIN_FONT, 0)
        .map_err(|error| anyhow!("parse bundled Latin tray font: {error:?}"))?;
    let cjk = Face::parse(CJK_FONT, 0)
        .map_err(|error| anyhow!("parse bundled Japanese tray font: {error:?}"))?;
    Ok(EmbossingFonts { latin, cjk })
}

pub(crate) struct TextMetrics {
    pub(crate) minimum_x: f32,
    pub(crate) minimum_y: f32,
    pub(crate) width: f32,
    pub(crate) height: f32,
}

pub(crate) fn text_metrics(fonts: &EmbossingFonts, text: &str) -> Result<TextMetrics> {
    let mut pen_x = 0.0_f32;
    let mut minimum_x = 0.0_f32;
    let mut maximum_x = 0.0_f32;
    let mut minimum_y = f32::INFINITY;
    let mut maximum_y = f32::NEG_INFINITY;
    let mut missing = Vec::new();

    for character in text.chars() {
        let Some((face, glyph_id)) = fonts.glyph(character) else {
            if !missing.contains(&character) {
                missing.push(character);
            }
            continue;
        };
        let Some(advance) = face.glyph_hor_advance(glyph_id) else {
            if !missing.contains(&character) {
                missing.push(character);
            }
            continue;
        };
        let units = 1_000.0 / f32::from(face.units_per_em());
        if let Some(bounds) = face.glyph_bounding_box(glyph_id) {
            minimum_x = minimum_x.min(pen_x + f32::from(bounds.x_min) * units);
            maximum_x = maximum_x.max(pen_x + f32::from(bounds.x_max) * units);
            minimum_y = minimum_y.min(f32::from(bounds.y_min) * units);
            maximum_y = maximum_y.max(f32::from(bounds.y_max) * units);
        }
        pen_x += f32::from(advance) * units;
        maximum_x = maximum_x.max(pen_x);
    }

    if !missing.is_empty() {
        let characters = missing
            .into_iter()
            .map(|character| format!("{character:?}"))
            .collect::<Vec<_>>()
            .join(", ");
        return Err(anyhow!(
            "the bundled tray font cannot render {characters}; use a place name written with supported Japanese, Latin, Cyrillic, or Vietnamese characters"
        ));
    }
    if !minimum_y.is_finite() || !maximum_y.is_finite() {
        return Err(anyhow!("tray label contains no printable characters"));
    }

    Ok(TextMetrics {
        minimum_x,
        minimum_y,
        width: (maximum_x - minimum_x).max(1.0),
        height: (maximum_y - minimum_y).max(1.0),
    })
}

pub(crate) fn validate_embossing_text(text: &str) -> Result<()> {
    let fonts = embossing_fonts()?;
    text_metrics(&fonts, text).map(|_| ())
}

/// A run of text embossed onto a mesh at a position and scale.
#[derive(Debug)]
pub(crate) struct EmbossedLabel {
    pub(crate) text: String,
    pub(crate) origin_x: f32,
    pub(crate) baseline_y: f32,
    pub(crate) scale: f32,
}

impl EmbossedLabel {
    pub(crate) fn add_embossed_shapes(&self, mesh: &mut MeshBuilder, rim_z: f32) -> Result<()> {
        let fonts = embossing_fonts()?;
        let mut pen_x = 0.0;
        for character in self.text.chars() {
            let (face, glyph_id) = fonts
                .glyph(character)
                .ok_or_else(|| anyhow!("tray font has no glyph for {character:?}"))?;
            let advance = face
                .glyph_hor_advance(glyph_id)
                .ok_or_else(|| anyhow!("tray font has no advance for {character:?}"))?
                as f32;
            let units = 1_000.0 / f32::from(face.units_per_em());
            let mut outline = GlyphOutline::default();
            if face.outline_glyph(glyph_id, &mut outline).is_some() {
                outline.finish_contour();
                let contours = outline
                    .contours
                    .into_iter()
                    .map(|contour| {
                        contour
                            .into_iter()
                            .map(|point| {
                                [
                                    self.origin_x + (pen_x + point[0] * units) * self.scale,
                                    self.baseline_y + point[1] * units * self.scale,
                                ]
                            })
                            .collect::<Vec<_>>()
                    })
                    .collect::<Vec<_>>();
                add_extruded_contours(
                    mesh,
                    &contours,
                    rim_z - 0.02,
                    rim_z + 0.56,
                    SurfaceClass::Snow,
                )?;
            }
            pen_x += advance * units;
        }
        Ok(())
    }
}

#[derive(Default)]
struct GlyphOutline {
    contours: Vec<Vec<[f32; 2]>>,
    current: Vec<[f32; 2]>,
}

impl GlyphOutline {
    fn push_point(&mut self, point: [f32; 2]) {
        if self
            .current
            .last()
            .is_none_or(|last| distance_squared(*last, point) > 0.000_001)
        {
            self.current.push(point);
        }
    }

    fn finish_contour(&mut self) {
        if self.current.len() > 2 {
            if distance_squared(self.current[0], *self.current.last().unwrap()) < 0.000_001 {
                self.current.pop();
            }
            if self.current.len() > 2 {
                self.contours.push(std::mem::take(&mut self.current));
                return;
            }
        }
        self.current.clear();
    }
}

impl OutlineBuilder for GlyphOutline {
    fn move_to(&mut self, x: f32, y: f32) {
        self.finish_contour();
        self.push_point([x, y]);
    }

    fn line_to(&mut self, x: f32, y: f32) {
        self.push_point([x, y]);
    }

    fn quad_to(&mut self, x1: f32, y1: f32, x: f32, y: f32) {
        let start = *self.current.last().unwrap_or(&[x, y]);
        flatten_quadratic(start, [x1, y1], [x, y], 0, &mut self.current);
    }

    fn curve_to(&mut self, x1: f32, y1: f32, x2: f32, y2: f32, x: f32, y: f32) {
        let start = *self.current.last().unwrap_or(&[x, y]);
        flatten_cubic(start, [x1, y1], [x2, y2], [x, y], 0, &mut self.current);
    }

    fn close(&mut self) {
        self.finish_contour();
    }
}

fn flatten_quadratic(
    start: [f32; 2],
    control: [f32; 2],
    end: [f32; 2],
    depth: u8,
    output: &mut Vec<[f32; 2]>,
) {
    if depth >= 10 || point_line_distance(control, start, end) <= 2.0 {
        output.push(end);
        return;
    }
    let start_control = midpoint(start, control);
    let control_end = midpoint(control, end);
    let middle = midpoint(start_control, control_end);
    flatten_quadratic(start, start_control, middle, depth + 1, output);
    flatten_quadratic(middle, control_end, end, depth + 1, output);
}

fn flatten_cubic(
    start: [f32; 2],
    control_a: [f32; 2],
    control_b: [f32; 2],
    end: [f32; 2],
    depth: u8,
    output: &mut Vec<[f32; 2]>,
) {
    let flatness =
        point_line_distance(control_a, start, end).max(point_line_distance(control_b, start, end));
    if depth >= 10 || flatness <= 2.0 {
        output.push(end);
        return;
    }
    let start_a = midpoint(start, control_a);
    let a_b = midpoint(control_a, control_b);
    let b_end = midpoint(control_b, end);
    let first_middle = midpoint(start_a, a_b);
    let second_middle = midpoint(a_b, b_end);
    let middle = midpoint(first_middle, second_middle);
    flatten_cubic(start, start_a, first_middle, middle, depth + 1, output);
    flatten_cubic(middle, second_middle, b_end, end, depth + 1, output);
}

fn midpoint(a: [f32; 2], b: [f32; 2]) -> [f32; 2] {
    [(a[0] + b[0]) * 0.5, (a[1] + b[1]) * 0.5]
}

fn point_in_contours(point: [f32; 2], contours: &[Vec<[f32; 2]>]) -> bool {
    contours
        .iter()
        .filter(|contour| point_in_polygon(point, contour))
        .count()
        % 2
        == 1
}

fn add_extruded_contours(
    mesh: &mut MeshBuilder,
    contours: &[Vec<[f32; 2]>],
    bottom_z: f32,
    top_z: f32,
    material: SurfaceClass,
) -> Result<()> {
    let mut points = Vec::new();
    let mut constraints = Vec::new();
    for contour in contours.iter().filter(|contour| contour.len() > 2) {
        let start = points.len();
        points.extend(
            contour
                .iter()
                .map(|point| Point2::new(f64::from(point[0]), f64::from(point[1]))),
        );
        constraints.extend(
            (0..contour.len()).map(|index| [start + index, start + (index + 1) % contour.len()]),
        );
    }
    if points.len() < 3 {
        return Ok(());
    }

    let triangulation =
        triangulate_constraints(points, constraints, "triangulate vector tray label")?;
    for face in triangulation.inner_faces() {
        let positions = face.vertices().map(|vertex| vertex.position());
        let centroid = [
            ((positions[0].x + positions[1].x + positions[2].x) / 3.0) as f32,
            ((positions[0].y + positions[1].y + positions[2].y) / 3.0) as f32,
        ];
        if !point_in_contours(centroid, contours) {
            continue;
        }
        let mut triangle = positions.map(|point| [point.x as f32, point.y as f32]);
        let area = (triangle[1][0] - triangle[0][0]) * (triangle[2][1] - triangle[0][1])
            - (triangle[1][1] - triangle[0][1]) * (triangle[2][0] - triangle[0][0]);
        if area < 0.0 {
            triangle.swap(1, 2);
        }
        mesh.triangle(
            [triangle[0][0], triangle[0][1], top_z],
            [triangle[1][0], triangle[1][1], top_z],
            [triangle[2][0], triangle[2][1], top_z],
            material,
        );
        mesh.triangle(
            [triangle[2][0], triangle[2][1], bottom_z],
            [triangle[1][0], triangle[1][1], bottom_z],
            [triangle[0][0], triangle[0][1], bottom_z],
            material,
        );
    }

    for contour in contours.iter().filter(|contour| contour.len() > 2) {
        for index in 0..contour.len() {
            let a = contour[index];
            let b = contour[(index + 1) % contour.len()];
            let edge = [b[0] - a[0], b[1] - a[1]];
            let edge_length = (edge[0].powi(2) + edge[1].powi(2)).sqrt();
            if edge_length <= f32::EPSILON {
                continue;
            }
            let middle = midpoint(a, b);
            let probe = [
                middle[0] - edge[1] / edge_length * 0.002,
                middle[1] + edge[0] / edge_length * 0.002,
            ];
            if point_in_contours(probe, contours) {
                mesh.quad(
                    [a[0], a[1], bottom_z],
                    [b[0], b[1], bottom_z],
                    [b[0], b[1], top_z],
                    [a[0], a[1], top_z],
                    material,
                );
            } else {
                mesh.quad(
                    [b[0], b[1], bottom_z],
                    [a[0], a[1], bottom_z],
                    [a[0], a[1], top_z],
                    [b[0], b[1], top_z],
                    material,
                );
            }
        }
    }
    Ok(())
}
