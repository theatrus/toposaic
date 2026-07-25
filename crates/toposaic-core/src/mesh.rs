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
}

#[derive(Default)]
pub(crate) struct MeshBuilder {
    vertices: Vec<[f32; 3]>,
    triangles: Vec<[u32; 3]>,
    materials: Vec<SurfaceClass>,
    indices: HashMap<(i64, i64, i64), u32>,
}

impl MeshBuilder {
    fn vertex(&mut self, point: [f32; 3]) -> u32 {
        let key = (
            (point[0] * 100_000.0).round() as i64,
            (point[1] * 100_000.0).round() as i64,
            (point[2] * 100_000.0).round() as i64,
        );
        *self.indices.entry(key).or_insert_with(|| {
            let index = self.vertices.len() as u32;
            self.vertices.push(point);
            index
        })
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
        }
    }

    pub(crate) fn append_isolated(&mut self, other: MeshBuilder) {
        append_isolated_parts(
            &mut self.vertices,
            &mut self.triangles,
            &mut self.materials,
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
            other,
        );
    }
}

fn append_isolated_parts(
    vertices: &mut Vec<[f32; 3]>,
    triangles: &mut Vec<[u32; 3]>,
    materials: &mut Vec<SurfaceClass>,
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
