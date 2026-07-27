//! Shared helpers for turning 2D outlines into mesh surfaces.

use anyhow::Result;
use geo::{Contains, Coord, LineString, Point, Polygon};
use spade::{Point2, Triangulation};

use crate::mesh::{MeshBuilder, triangulate_constraints};
use crate::spec::SurfaceClass;

pub(crate) fn outline_bounds(outline: &[[f32; 2]]) -> [f32; 4] {
    outline.iter().fold(
        [
            f32::INFINITY,
            f32::INFINITY,
            f32::NEG_INFINITY,
            f32::NEG_INFINITY,
        ],
        |mut bounds, point| {
            bounds[0] = bounds[0].min(point[0]);
            bounds[1] = bounds[1].min(point[1]);
            bounds[2] = bounds[2].max(point[0]);
            bounds[3] = bounds[3].max(point[1]);
            bounds
        },
    )
}

pub(crate) fn closed_ring(points: &[[f32; 2]]) -> LineString<f64> {
    let mut coordinates = points
        .iter()
        .map(|point| Coord {
            x: f64::from(point[0]),
            y: f64::from(point[1]),
        })
        .collect::<Vec<_>>();
    if coordinates.first() != coordinates.last()
        && let Some(first) = coordinates.first().copied()
    {
        coordinates.push(first);
    }
    LineString::new(coordinates)
}

pub(crate) fn polygon_from_outline(points: &[[f32; 2]]) -> Polygon<f64> {
    Polygon::new(closed_ring(points), vec![])
}

pub(crate) fn add_horizontal_polygons(
    mesh: &mut MeshBuilder,
    polygons: &[Polygon<f64>],
    z: f32,
    material: SurfaceClass,
    reverse: bool,
) -> Result<()> {
    for polygon in polygons {
        let mut points = Vec::new();
        let mut constraints = Vec::new();
        for ring in std::iter::once(polygon.exterior()).chain(polygon.interiors()) {
            let start = points.len();
            for coordinate in ring.0.iter().take(ring.0.len().saturating_sub(1)) {
                points.push(Point2::new(coordinate.x, coordinate.y));
            }
            let count = points.len() - start;
            for index in 0..count {
                constraints.push([start + index, start + (index + 1) % count]);
            }
        }
        if points.len() < 3 {
            continue;
        }
        let triangulation =
            triangulate_constraints(points, constraints, "triangulate planar surface")?;
        for face in triangulation.inner_faces() {
            let vertices = face.vertices();
            let center = vertices.iter().fold([0.0, 0.0], |sum, vertex| {
                let point = vertex.position();
                [sum[0] + point.x / 3.0, sum[1] + point.y / 3.0]
            });
            if !polygon.contains(&Point::new(center[0], center[1])) {
                continue;
            }
            let points = vertices.map(|vertex| {
                let point = vertex.position();
                [point.x as f32, point.y as f32, z]
            });
            if reverse {
                mesh.triangle(points[0], points[2], points[1], material);
            } else {
                mesh.triangle(points[0], points[1], points[2], material);
            }
        }
    }
    Ok(())
}
