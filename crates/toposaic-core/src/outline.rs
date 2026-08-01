use anyhow::{Result, bail};
use geo::{Area, BooleanOps, Polygon};

use crate::planar_mesh::polygon_from_outline;
use crate::spec::{GenerationSpec, OutlineShape};

const MAX_POLYGON_POINTS: usize = 128;
const MIN_NORMALIZED_AREA: f32 = 0.0025;
const MIN_PRINT_COMPONENT_AREA_MM2: f64 = 0.05;

pub(crate) fn validate_normalized_polygon(points: &[[f32; 2]]) -> Result<()> {
    if !(3..=MAX_POLYGON_POINTS).contains(&points.len()) {
        bail!("a custom outline needs between 3 and {MAX_POLYGON_POINTS} points");
    }
    for point in points {
        if !point[0].is_finite()
            || !point[1].is_finite()
            || !(0.0..=1.0).contains(&point[0])
            || !(0.0..=1.0).contains(&point[1])
        {
            bail!("custom outline points must stay inside the selected map area");
        }
    }
    for index in 0..points.len() {
        let next = (index + 1) % points.len();
        if distance_squared(points[index], points[next]) < 1.0e-8 {
            bail!("custom outline has two points in the same place");
        }
    }
    for first in 0..points.len() {
        let first_next = (first + 1) % points.len();
        for second in first + 1..points.len() {
            let second_next = (second + 1) % points.len();
            if first == second
                || first_next == second
                || second_next == first
                || (first == 0 && second_next == 0)
            {
                continue;
            }
            if segments_intersect(
                points[first],
                points[first_next],
                points[second],
                points[second_next],
            ) {
                bail!("custom outline edges cannot cross");
            }
        }
    }
    if signed_area(points).abs() < MIN_NORMALIZED_AREA {
        bail!("custom outline is too small");
    }
    Ok(())
}

pub(crate) fn normalized_outline(spec: &GenerationSpec, samples: usize) -> Vec<[f32; 2]> {
    let aspect = spec.height_mm() / spec.width_mm;
    match spec.model_outline.shape {
        OutlineShape::Rectangle => densify_ring(
            &[[0.0, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 1.0]],
            samples.max(4) * 4,
        ),
        OutlineShape::Circle => {
            let radius_x = 0.5_f32.min(aspect * 0.5);
            let radius_y = (0.5 / aspect).min(0.5);
            ellipse_points(radius_x, radius_y, samples.max(32) * 4)
        }
        OutlineShape::Ellipse => ellipse_points(0.5, 0.5, samples.max(32) * 4),
        OutlineShape::Polygon => densify_ring(&spec.model_outline.points, samples.max(8) * 4),
    }
}

pub(crate) fn model_outline_mm(spec: &GenerationSpec, samples: usize) -> Vec<[f32; 2]> {
    normalized_outline(spec, samples)
        .into_iter()
        .map(|[u, v]| [u * spec.width_mm, v * spec.height_mm()])
        .collect()
}

/// Intersects a jigsaw cell with the model boundary. A single print file may
/// not contain two loose islands, so a polygon that splits one cell gets a
/// direct error rather than a file whose parts can be mistaken for one piece.
pub(crate) fn clip_piece_outline(
    spec: &GenerationSpec,
    piece: &[[f32; 2]],
) -> Result<Option<Vec<[f32; 2]>>> {
    if spec.model_outline.shape == OutlineShape::Rectangle {
        return Ok(Some(piece.to_vec()));
    }
    let boundary = model_outline_mm(spec, spec.samples_per_piece as usize);
    let intersection = polygon_from_outline(piece).intersection(&polygon_from_outline(&boundary));
    let mut components = intersection
        .0
        .into_iter()
        .filter(|polygon| polygon.unsigned_area() >= MIN_PRINT_COMPONENT_AREA_MM2)
        .collect::<Vec<_>>();
    if components.is_empty() {
        return Ok(None);
    }
    components.sort_by(|first, second| second.unsigned_area().total_cmp(&first.unsigned_area()));
    if components.len() > 1 {
        if spec.model_outline.shape == OutlineShape::Polygon {
            bail!(
                "the custom outline splits a puzzle cell into separate islands; move an outline edge or use fewer pieces"
            );
        }
        bail!(
            "the shaped outline splits a puzzle cell into separate islands; try another puzzle seed or use fewer pieces"
        );
    }
    let polygon = components.remove(0);
    if !polygon.interiors().is_empty() {
        bail!("the custom outline makes a hole inside one puzzle piece");
    }
    Ok(Some(exterior_points(&polygon)?))
}

fn exterior_points(polygon: &Polygon<f64>) -> Result<Vec<[f32; 2]>> {
    let mut points = polygon
        .exterior()
        .0
        .iter()
        .take(polygon.exterior().0.len().saturating_sub(1))
        .map(|point| [point.x as f32, point.y as f32])
        .collect::<Vec<_>>();
    points.dedup_by(|first, second| distance_squared(*first, *second) < 1.0e-10);
    if points.len() < 3 {
        return Err(anyhow::anyhow!(
            "outline clipping left fewer than three points"
        ));
    }
    if signed_area(&points) < 0.0 {
        points.reverse();
    }
    Ok(points)
}

fn ellipse_points(radius_x: f32, radius_y: f32, samples: usize) -> Vec<[f32; 2]> {
    let count = samples.clamp(64, 768);
    (0..count)
        .map(|index| {
            // Start at the bottom and wind counter-clockwise, like the
            // rectangle and jigsaw outlines used by the wall builder.
            let angle =
                -std::f32::consts::FRAC_PI_2 + index as f32 / count as f32 * std::f32::consts::TAU;
            [0.5 + radius_x * angle.cos(), 0.5 + radius_y * angle.sin()]
        })
        .collect()
}

fn densify_ring(points: &[[f32; 2]], target: usize) -> Vec<[f32; 2]> {
    if points.len() < 3 {
        return points.to_vec();
    }
    let lengths = (0..points.len())
        .map(|index| {
            let next = points[(index + 1) % points.len()];
            distance_squared(points[index], next).sqrt()
        })
        .collect::<Vec<_>>();
    let perimeter = lengths.iter().sum::<f32>().max(f32::EPSILON);
    let mut result = Vec::with_capacity(target.max(points.len()));
    for (index, length) in lengths.into_iter().enumerate() {
        let count = ((target as f32 * length / perimeter).round() as usize).max(1);
        let start = points[index];
        let end = points[(index + 1) % points.len()];
        for step in 0..count {
            let t = step as f32 / count as f32;
            result.push([
                start[0] + (end[0] - start[0]) * t,
                start[1] + (end[1] - start[1]) * t,
            ]);
        }
    }
    result
}

fn signed_area(points: &[[f32; 2]]) -> f32 {
    (0..points.len())
        .map(|index| {
            let next = points[(index + 1) % points.len()];
            points[index][0] * next[1] - next[0] * points[index][1]
        })
        .sum::<f32>()
        * 0.5
}

fn distance_squared(first: [f32; 2], second: [f32; 2]) -> f32 {
    (first[0] - second[0]).powi(2) + (first[1] - second[1]).powi(2)
}

fn orientation(first: [f32; 2], second: [f32; 2], third: [f32; 2]) -> f32 {
    (second[0] - first[0]) * (third[1] - first[1]) - (second[1] - first[1]) * (third[0] - first[0])
}

fn on_segment(first: [f32; 2], point: [f32; 2], second: [f32; 2]) -> bool {
    point[0] >= first[0].min(second[0]) - 1.0e-6
        && point[0] <= first[0].max(second[0]) + 1.0e-6
        && point[1] >= first[1].min(second[1]) - 1.0e-6
        && point[1] <= first[1].max(second[1]) + 1.0e-6
}

fn segments_intersect(a: [f32; 2], b: [f32; 2], c: [f32; 2], d: [f32; 2]) -> bool {
    let ab_c = orientation(a, b, c);
    let ab_d = orientation(a, b, d);
    let cd_a = orientation(c, d, a);
    let cd_b = orientation(c, d, b);
    if ab_c * ab_d < 0.0 && cd_a * cd_b < 0.0 {
        return true;
    }
    (ab_c.abs() <= 1.0e-6 && on_segment(a, c, b))
        || (ab_d.abs() <= 1.0e-6 && on_segment(a, d, b))
        || (cd_a.abs() <= 1.0e-6 && on_segment(c, a, d))
        || (cd_b.abs() <= 1.0e-6 && on_segment(c, b, d))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_crossed_polygon() {
        let error = validate_normalized_polygon(&[[0.1, 0.1], [0.9, 0.9], [0.1, 0.9], [0.9, 0.1]])
            .unwrap_err();
        assert!(error.to_string().contains("edges cannot cross"));
    }

    #[test]
    fn a_circle_is_round_in_print_space() {
        let mut spec = GenerationSpec {
            width_mm: 180.0,
            rows: 2,
            columns: 3,
            ..GenerationSpec::default()
        };
        spec.model_outline.shape = OutlineShape::Circle;
        let points = model_outline_mm(&spec, 32);
        let x = points
            .iter()
            .map(|point| point[0])
            .fold(f32::INFINITY, f32::min);
        let x1 = points
            .iter()
            .map(|point| point[0])
            .fold(f32::NEG_INFINITY, f32::max);
        let y = points
            .iter()
            .map(|point| point[1])
            .fold(f32::INFINITY, f32::min);
        let y1 = points
            .iter()
            .map(|point| point[1])
            .fold(f32::NEG_INFINITY, f32::max);
        assert!(((x1 - x) - (y1 - y)).abs() < 0.001);
    }
}
