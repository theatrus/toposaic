use std::collections::HashMap;

use anyhow::{Context, Result};
use rayon::prelude::*;
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

/// Snaps one coordinate toward the 3MF export grid: the nearest `f32` to
/// the 5-decimal value the `{:.5}` formatter in `export.rs` prints.
/// Wherever one `f32` step is below the 1e-5 grid spacing (magnitudes up to
/// about 168 mm) the formatter prints exactly that decimal back; above
/// that, neighbouring grid cells share representable floats and the printed
/// decimal can sit one grid step away from the rounding target. The export
/// weld therefore keys on the snapped BIT PATTERN, not the decimal, so
/// welding stays consistent either way. Negative values that round to zero
/// return positive zero so the formatted text never distinguishes
/// `-0.00000` from `0.00000`.
pub(crate) fn quantize_export_coordinate(value: f32) -> f32 {
    let snapped = (f64::from(value) * 100_000.0).round() / 100_000.0;
    if snapped == 0.0 { 0.0 } else { snapped as f32 }
}

/// Final deterministic weld and cleanup for a finished export mesh.
///
/// Every vertex snaps to the exact grid the 3MF writer's `{:.5}` formatting
/// emits, vertices landing on the same snapped position merge (the first
/// occurrence wins), triangles whose corners collapse together drop, extra
/// same-winding copies of a face drop (the first stays), and unused
/// vertices compact away in first-use order. The pass is stable over
/// triangle order, and afterwards the in-memory index topology, an STL
/// bit-exact vertex weld, and a 3MF five-decimal weld all reconstruct the
/// same mesh — a slicer sees exactly what the generator validated.
pub(crate) fn weld_export_mesh(mesh: &mut Mesh) {
    // Weld by the snapped bit pattern. Keying on bits (not on the decimal
    // grid index) matters: above 168 mm one f32 step exceeds the grid, so
    // two grid cells can share one representable float.
    let quantized_input = mesh
        .vertices
        .par_iter()
        .map(|vertex| vertex.map(quantize_export_coordinate))
        .collect::<Vec<_>>();
    let mut canonical = HashMap::<u128, u32, BuildKeyHasher>::with_capacity_and_hasher(
        mesh.vertices.len(),
        BuildKeyHasher::default(),
    );
    let mut quantized = Vec::<[f32; 3]>::with_capacity(mesh.vertices.len());
    let mut welded = Vec::<u32>::with_capacity(mesh.vertices.len());
    for snapped in quantized_input {
        let bits = snapped.map(f32::to_bits);
        let key = u128::from(bits[0]) << 64 | u128::from(bits[1]) << 32 | u128::from(bits[2]);
        let next = quantized.len() as u32;
        let index = *canonical.entry(key).or_insert(next);
        if index == next {
            quantized.push(snapped);
        }
        welded.push(index);
    }

    let mut kept_triangles = Vec::with_capacity(mesh.triangles.len());
    let mut kept_materials = Vec::with_capacity(mesh.materials.len());
    // A welded vertex set has at most two distinct cyclic windings, so the
    // first-seen rotation plus an opposite-winding flag captures every
    // duplicate without per-face allocation.
    let mut face_windings =
        HashMap::<u128, ([u32; 3], bool), BuildKeyHasher>::with_capacity_and_hasher(
            mesh.triangles.len(),
            BuildKeyHasher::default(),
        );
    for (triangle, material) in mesh.triangles.iter().zip(&mesh.materials) {
        let mapped = triangle.map(|index| welded[index as usize]);
        if mapped[0] == mapped[1] || mapped[1] == mapped[2] || mapped[2] == mapped[0] {
            continue;
        }
        // Rotate the smallest index first; two triangles on one vertex set
        // share this form exactly when they have the same cyclic winding.
        let smallest = (0..3)
            .min_by_key(|position| mapped[*position])
            .expect("triangle has three corners");
        let rotation = [
            mapped[smallest],
            mapped[(smallest + 1) % 3],
            mapped[(smallest + 2) % 3],
        ];
        let mut sorted = mapped;
        sorted.sort_unstable();
        let key = u128::from(sorted[0]) << 64 | u128::from(sorted[1]) << 32 | u128::from(sorted[2]);
        match face_windings.entry(key) {
            std::collections::hash_map::Entry::Occupied(mut entry) => {
                let (first, opposite_seen) = entry.get_mut();
                if *first == rotation || *opposite_seen {
                    continue;
                }
                *opposite_seen = true;
            }
            std::collections::hash_map::Entry::Vacant(entry) => {
                entry.insert((rotation, false));
            }
        }
        kept_triangles.push(mapped);
        kept_materials.push(*material);
    }

    let mut compacted = vec![u32::MAX; quantized.len()];
    let mut vertices = Vec::with_capacity(quantized.len());
    for triangle in &mut kept_triangles {
        for index in triangle {
            let slot = &mut compacted[*index as usize];
            if *slot == u32::MAX {
                *slot = vertices.len() as u32;
                vertices.push(quantized[*index as usize]);
            }
            *index = *slot;
        }
    }
    mesh.vertices = vertices;
    mesh.triangles = kept_triangles;
    mesh.materials = kept_materials;
}

/// splitmix64's finalizer: a fast, well-mixing hash step for fixed-width
/// keys.
fn mix_bits(mut value: u64) -> u64 {
    value ^= value >> 30;
    value = value.wrapping_mul(0xBF58_476D_1CE4_E5B9);
    value ^= value >> 27;
    value = value.wrapping_mul(0x94D0_49BB_1331_11EB);
    value ^= value >> 31;
    value
}

/// Hasher for the weld pass's packed `u128` keys: two splitmix rounds over
/// the halves. Far faster than SipHash for these fixed-width keys and
/// mixing enough for hash-table bucketing; key equality stays exact.
#[derive(Default)]
struct KeyHasher(u64);

impl std::hash::Hasher for KeyHasher {
    fn finish(&self) -> u64 {
        self.0
    }

    fn write(&mut self, bytes: &[u8]) {
        for chunk in bytes.chunks(8) {
            let mut word = [0_u8; 8];
            word[..chunk.len()].copy_from_slice(chunk);
            self.0 = mix_bits(self.0 ^ u64::from_le_bytes(word));
        }
    }

    fn write_u128(&mut self, value: u128) {
        self.0 = mix_bits(mix_bits(value as u64) ^ (value >> 64) as u64);
    }
}

type BuildKeyHasher = std::hash::BuildHasherDefault<KeyHasher>;

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
/// Asserts a mesh survives every vertex weld a consumer applies: in the
/// in-memory index topology, after an STL bit-exact weld, and after a 3MF
/// five-decimal weld, every undirected edge must be used exactly twice with
/// no collapsed triangles and no same-winding duplicate faces. Any edge
/// used four or more times after welding means two pieces of geometry
/// genuinely overlap and is always a hard failure.
///
/// This is edge/vertex topology only, not a full manifold proof: shells
/// that overlap in space without sharing welded vertices (tray contour
/// ribbons embed into the floor by design), a globally inverted winding,
/// and slivers above the analyzer's area epsilon all pass. See the
/// `analysis` module docs for the full list of out-of-scope defect
/// classes.
pub(crate) fn assert_watertight(mesh: &Mesh) {
    let report = crate::analysis::analyze_mesh_views(mesh);
    for view in &report.views {
        assert_eq!(
            view.slicer_edge_defects, 0,
            "{}: {} view has {} open and {} overused edges: {:?}",
            mesh.name, view.view, view.open_edges, view.overused_edges, view.edge_examples,
        );
        assert_eq!(
            view.degenerate_repeated_index, 0,
            "{}: {} view has collapsed triangles: {:?}",
            mesh.name, view.view, view.degenerate_examples,
        );
        assert_eq!(
            view.duplicate_same_winding, 0,
            "{}: {} view has same-winding duplicate faces: {:?}",
            mesh.name, view.view, view.duplicate_examples,
        );
        assert_eq!(
            view.misoriented_edges, 0,
            "{}: {} view has edges traversed twice in one direction \
             (neighbouring faces disagree on winding)",
            mesh.name, view.view,
        );
    }
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
