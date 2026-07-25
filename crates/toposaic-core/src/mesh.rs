use std::collections::HashMap;

use anyhow::{Context, Result};
use spade::{ConstrainedDelaunayTriangulation, Point2};

use crate::spec::SurfaceClass;

#[derive(Debug, Clone)]
pub(crate) struct Mesh {
    pub(crate) name: String,
    pub(crate) vertices: Vec<[f32; 3]>,
    pub(crate) triangles: Vec<[u32; 3]>,
    pub(crate) materials: Vec<SurfaceClass>,
    /// Diagnostic trail only: `MeshBuilder::vertex` calls whose 1e-5
    /// quantization key matched an already-stored vertex at a *different*
    /// position (kept position, dropped position). Geometry is unchanged;
    /// the manifold analyzer reads this to attribute weld collisions.
    pub(crate) quantization_collisions: Vec<([f32; 3], [f32; 3])>,
}

#[derive(Default)]
pub(crate) struct MeshBuilder {
    vertices: Vec<[f32; 3]>,
    triangles: Vec<[u32; 3]>,
    materials: Vec<SurfaceClass>,
    indices: HashMap<(i64, i64, i64), u32>,
    collisions: Vec<([f32; 3], [f32; 3])>,
}

impl MeshBuilder {
    fn vertex(&mut self, point: [f32; 3]) -> u32 {
        let key = (
            (point[0] * 100_000.0).round() as i64,
            (point[1] * 100_000.0).round() as i64,
            (point[2] * 100_000.0).round() as i64,
        );
        if let Some(&index) = self.indices.get(&key) {
            let kept = self.vertices[index as usize];
            if kept != point {
                self.collisions.push((kept, point));
            }
            return index;
        }
        let index = self.vertices.len() as u32;
        self.vertices.push(point);
        self.indices.insert(key, index);
        index
    }

    pub(crate) fn triangle(
        &mut self,
        a: [f32; 3],
        b: [f32; 3],
        c: [f32; 3],
        material: SurfaceClass,
    ) {
        let triangle = [self.vertex(a), self.vertex(b), self.vertex(c)];
        self.triangles.push(triangle);
        self.materials.push(material);
    }

    pub(crate) fn quad(
        &mut self,
        a: [f32; 3],
        b: [f32; 3],
        c: [f32; 3],
        d: [f32; 3],
        material: SurfaceClass,
    ) {
        self.triangle(a, b, c, material);
        self.triangle(a, c, d, material);
    }

    pub(crate) fn finish(self, name: impl Into<String>) -> Mesh {
        Mesh {
            name: name.into(),
            vertices: self.vertices,
            triangles: self.triangles,
            materials: self.materials,
            quantization_collisions: self.collisions,
        }
    }

    pub(crate) fn append_isolated(&mut self, other: MeshBuilder) {
        append_isolated_parts(
            &mut self.vertices,
            &mut self.triangles,
            &mut self.materials,
            &mut self.collisions,
            other,
        );
    }
}

impl Mesh {
    pub(crate) fn append_isolated(&mut self, other: MeshBuilder) {
        append_isolated_parts(
            &mut self.vertices,
            &mut self.triangles,
            &mut self.materials,
            &mut self.quantization_collisions,
            other,
        );
    }
}

fn append_isolated_parts(
    vertices: &mut Vec<[f32; 3]>,
    triangles: &mut Vec<[u32; 3]>,
    materials: &mut Vec<SurfaceClass>,
    collisions: &mut Vec<([f32; 3], [f32; 3])>,
    other: MeshBuilder,
) {
    let offset = vertices.len() as u32;
    vertices.extend(other.vertices);
    triangles.extend(
        other
            .triangles
            .into_iter()
            .map(|triangle| triangle.map(|index| index + offset)),
    );
    materials.extend(other.materials);
    collisions.extend(other.collisions);
}

/// Boolean clipping can repeat a vertex or overlap constraints at a dense
/// line junction, and spade's strict loader panics on both. Duplicate
/// constraints are dropped up front and overlaps are rejected instead;
/// callers filter faces by containment, so a rejected overlap is safe.
pub(crate) fn triangulate_constraints(
    points: Vec<Point2<f64>>,
    mut constraints: Vec<[usize; 2]>,
    error_context: &'static str,
) -> Result<ConstrainedDelaunayTriangulation<Point2<f64>>> {
    let mut canonical_positions = HashMap::<(u64, u64), usize>::new();
    let canonical_indices = points
        .iter()
        .enumerate()
        .map(|(index, point)| {
            *canonical_positions
                .entry((point.x.to_bits(), point.y.to_bits()))
                .or_insert(index)
        })
        .collect::<Vec<_>>();
    constraints.retain(|[from, to]| canonical_indices[*from] != canonical_indices[*to]);
    ConstrainedDelaunayTriangulation::<Point2<f64>>::try_bulk_load_cdt(points, constraints, |_| {})
        .context(error_context)
}

/// Snaps one coordinate to the 3MF export grid: the value the `{:.5}`
/// formatter in `export.rs` will print is exactly the decimal this rounds to.
/// Negative values that round to zero return positive zero so the formatted
/// text never distinguishes `-0.00000` from `0.00000`.
pub(crate) fn quantize_export_coordinate(value: f32) -> f32 {
    let snapped = (f64::from(value) * 100_000.0).round() / 100_000.0;
    if snapped == 0.0 {
        0.0
    } else {
        snapped as f32
    }
}

pub(crate) fn unit_vector(vector: [f32; 2]) -> [f32; 2] {
    let length = vector[0].hypot(vector[1]);
    if length <= f32::EPSILON {
        [0.0, 0.0]
    } else {
        [vector[0] / length, vector[1] / length]
    }
}

pub(crate) fn distance_squared(a: [f32; 2], b: [f32; 2]) -> f32 {
    (a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2)
}

pub(crate) fn point_line_distance(point: [f32; 2], start: [f32; 2], end: [f32; 2]) -> f32 {
    let length_squared = distance_squared(start, end);
    if length_squared <= f32::EPSILON {
        return distance_squared(point, start).sqrt();
    }
    let cross =
        (end[0] - start[0]) * (start[1] - point[1]) - (start[0] - point[0]) * (end[1] - start[1]);
    cross.abs() / length_squared.sqrt()
}

pub(crate) fn point_in_polygon(point: [f32; 2], polygon: &[[f32; 2]]) -> bool {
    let mut inside = false;
    let mut previous = polygon.len() - 1;
    for current in 0..polygon.len() {
        let a = polygon[current];
        let b = polygon[previous];
        let crosses = (a[1] > point[1]) != (b[1] > point[1])
            && point[0] < (b[0] - a[0]) * (point[1] - a[1]) / (b[1] - a[1]) + a[0];
        if crosses {
            inside = !inside;
        }
        previous = current;
    }
    inside
}

/// Even-odd point-in-polygon queries accelerated by a strip index over y.
///
/// Edges are bucketed by the strips their y-span covers; a query walks only
/// the edges in its own strip and runs the exact crossing test from
/// [`point_in_polygon`] on each. An edge whose y-span excludes the query
/// point contributes no crossing there (both endpoint comparisons agree), so
/// walking the strip's superset of candidate edges returns exactly the same
/// answer as walking the whole outline.
pub(crate) struct PolygonStripIndex<'a> {
    polygon: &'a [[f32; 2]],
    minimum_y: f32,
    maximum_y: f32,
    strips_per_unit: f32,
    strips: Vec<Vec<u32>>,
}

impl<'a> PolygonStripIndex<'a> {
    pub(crate) fn new(polygon: &'a [[f32; 2]], strip_count: usize) -> Self {
        debug_assert!(polygon.len() >= 3);
        let (minimum_y, maximum_y) = polygon.iter().fold(
            (f32::INFINITY, f32::NEG_INFINITY),
            |(minimum, maximum), point| (minimum.min(point[1]), maximum.max(point[1])),
        );
        let strip_count = strip_count.clamp(1, polygon.len());
        let strips_per_unit = strip_count as f32 / (maximum_y - minimum_y).max(f32::EPSILON);
        let mut strips = vec![Vec::new(); strip_count];
        let mut previous = polygon.len() - 1;
        for current in 0..polygon.len() {
            let low = polygon[current][1].min(polygon[previous][1]);
            let high = polygon[current][1].max(polygon[previous][1]);
            let first = (((low - minimum_y) * strips_per_unit) as usize).min(strip_count - 1);
            let last = (((high - minimum_y) * strips_per_unit) as usize).min(strip_count - 1);
            for strip in &mut strips[first..=last] {
                strip.push(current as u32);
            }
            previous = current;
        }
        Self {
            polygon,
            minimum_y,
            maximum_y,
            strips_per_unit,
            strips,
        }
    }

    /// Returns exactly `point_in_polygon(point, polygon)`.
    pub(crate) fn contains(&self, point: [f32; 2]) -> bool {
        // Outside the polygon's y-range no edge can cross, so the full walk
        // would return false too. This also rejects NaN.
        if !(point[1] >= self.minimum_y && point[1] <= self.maximum_y) {
            return false;
        }
        let strip = (((point[1] - self.minimum_y) * self.strips_per_unit) as usize)
            .min(self.strips.len() - 1);
        let mut inside = false;
        for &edge in &self.strips[strip] {
            let current = edge as usize;
            let previous = if current == 0 {
                self.polygon.len() - 1
            } else {
                current - 1
            };
            let a = self.polygon[current];
            let b = self.polygon[previous];
            // Keep this test identical to point_in_polygon, including the
            // comparison operators, so ties on vertices behave the same.
            let crosses = (a[1] > point[1]) != (b[1] > point[1])
                && point[0] < (b[0] - a[0]) * (point[1] - a[1]) / (b[1] - a[1]) + a[0];
            if crosses {
                inside = !inside;
            }
        }
        inside
    }
}

#[cfg(test)]
pub(crate) fn assert_watertight(mesh: &Mesh) {
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
    let bad_edges = edges
        .iter()
        .filter(|(_, uses)| **uses != 2)
        .take(12)
        .map(|(edge, uses)| {
            (
                mesh.vertices[edge.0 as usize],
                mesh.vertices[edge.1 as usize],
                *uses,
            )
        })
        .collect::<Vec<_>>();
    assert!(bad_edges.is_empty(), "non-manifold edges: {bad_edges:?}");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strip_index_matches_the_full_point_in_polygon_walk() {
        // A wavy closed outline with several local extrema per strip.
        let outline = (0..400)
            .map(|index| {
                let angle = index as f32 / 400.0 * std::f32::consts::TAU;
                let radius = 10.0 + (angle * 9.0).sin() * 3.0;
                [radius * angle.cos(), radius * angle.sin()]
            })
            .collect::<Vec<_>>();
        for strip_count in [1, 7, 64] {
            let index = PolygonStripIndex::new(&outline, strip_count);
            for y_step in -32..=32 {
                for x_step in -32..=32 {
                    let point = [x_step as f32 * 0.45, y_step as f32 * 0.45];
                    assert_eq!(
                        index.contains(point),
                        point_in_polygon(point, &outline),
                        "strips={strip_count}, point={point:?}"
                    );
                }
            }
            // Vertex ties and out-of-range queries behave identically too.
            for point in [outline[13], outline[0], [0.0, 99.0], [0.0, -99.0]] {
                assert_eq!(index.contains(point), point_in_polygon(point, &outline));
            }
        }
    }
}
