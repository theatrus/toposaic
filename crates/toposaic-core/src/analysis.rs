//! Manifold diagnostics for generated meshes.
//!
//! This module measures and classifies mesh defects; it never repairs
//! anything. It exists for the `manifold_report` examples and is hidden from
//! the documented public API.
//!
//! Every mesh is analyzed in three views:
//!
//! * `indexed` — the in-memory index topology exactly as generation built it
//!   (what `assert_watertight` sees in tests).
//! * `stl-weld` — vertices welded by exact f32 bit equality, which is what a
//!   slicer reconstructs after loading the binary STL triangle soup.
//! * `3mf-weld` — vertices welded by the `{:.5}` decimal formatting the 3MF
//!   writer emits, which is what a slicer reconstructs from `toposaic.3mf`.
//!
//! The `slicer_edge_defects` number in the stl/3mf views is the count a
//! PrusaSlicer/Bambu-style checker reports as "non-manifold edges": undirected
//! edges whose use count is not exactly 2.
//!
//! # What this module does NOT detect
//!
//! Honest limits, so a clean report is not over-read:
//!
//! * **Unwelded overlaps.** Two shells that pass through each other without
//!   sharing any (welded) vertex — for example a ribbon poking through a
//!   wall — produce no shared edges and count as clean. In particular, tray
//!   contour ribbons embed into the tray floor BY DESIGN; that intersection
//!   is intentional and invisible to every counter here.
//! * **Global inversion.** A mesh wound consistently inside-out has every
//!   edge used once per direction and reports clean; only *local* winding
//!   disagreements show up (see `misoriented_edges`).
//! * **Sliver thresholds.** `near_zero_area` uses a fixed 1e-8 mm^2 cut.
//!   Slicers apply their own epsilons, so a triangle can pass here and
//!   still collapse inside a particular slicer, or the reverse.

use std::collections::{BTreeMap, HashMap};

use anyhow::Result;
use serde::Serialize;

use crate::heightfield::{HeightField, height_range_for_spec};
use crate::mesh::Mesh;
use crate::piece::build_piece_with_height_range;
use crate::spec::{GenerationSpec, SurfaceClass};
use crate::surface::SurfaceField;
use crate::tray::build_tray_segments;

/// Cap on stored example strings per defect list, to keep reports readable.
const EXAMPLE_CAP: usize = 8;

#[derive(Debug, Clone, Serialize)]
pub struct ViewReport {
    pub view: String,
    /// Triangles whose three welded indices are not distinct.
    pub degenerate_repeated_index: usize,
    /// Non-degenerate triangles with 3D area below 1e-8 mm^2.
    pub near_zero_area: usize,
    /// Extra faces on an index triple with the same cyclic winding.
    pub duplicate_same_winding: usize,
    /// Extra faces on an index triple with opposite winding.
    pub duplicate_opposite_winding: usize,
    /// Undirected edges used exactly once (open boundary / hole).
    pub open_edges: usize,
    /// Undirected edges used three or more times (fin / tee).
    pub overused_edges: usize,
    /// open_edges + overused_edges: what a slicer reports as
    /// "non-manifold edges".
    pub slicer_edge_defects: usize,
    /// Undirected edges some direction of which is traversed by two or more
    /// faces: neighbouring faces that disagree on winding. Such an edge can
    /// still count as used exactly twice, so the other edge counters miss
    /// it.
    pub misoriented_edges: usize,
    /// Vertices whose incident faces form more than one edge-connected fan.
    pub nonmanifold_vertices: usize,
    /// Feature attribution histogram over every defective triangle and every
    /// triangle incident to a defective edge.
    pub defect_features: BTreeMap<String, usize>,
    pub degenerate_examples: Vec<String>,
    pub duplicate_examples: Vec<String>,
    pub edge_examples: Vec<String>,
}

impl ViewReport {
    pub fn is_clean(&self) -> bool {
        self.degenerate_repeated_index == 0
            && self.near_zero_area == 0
            && self.duplicate_same_winding == 0
            && self.duplicate_opposite_winding == 0
            && self.slicer_edge_defects == 0
            && self.misoriented_edges == 0
            && self.nonmanifold_vertices == 0
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct MeshReport {
    pub name: String,
    pub triangles: usize,
    pub vertices: usize,
    /// `MeshBuilder::vertex` calls that hit an existing 1e-5 quantization key
    /// holding a different position (only overlay shells and trays run
    /// through `MeshBuilder`; the terrain body does not).
    pub quantization_collisions: usize,
    pub collision_examples: Vec<String>,
    pub views: Vec<ViewReport>,
}

impl MeshReport {
    pub fn view(&self, name: &str) -> &ViewReport {
        self.views
            .iter()
            .find(|view| view.view == name)
            .expect("all three views are always present")
    }
}

/// Classifies which generated feature a triangle belongs to, from its
/// material and geometry alone. The terrain body uses z == 0 for the floor
/// and z > 0 for the relief top, and every wall face is (near) vertical, so
/// the face normal and the z range separate the parts reliably.
fn triangle_feature(mesh: &Mesh, triangle_index: usize) -> String {
    let material = mesh.materials[triangle_index];
    let corners = mesh.triangles[triangle_index].map(|index| mesh.vertices[index as usize]);
    let normal_z = face_normal(corners)[2];
    let orientation = if normal_z > 0.5 {
        "top"
    } else if normal_z < -0.5 {
        "bottom"
    } else {
        "wall"
    };
    match material {
        SurfaceClass::Building => format!("building-shell {orientation}"),
        SurfaceClass::Road => format!("road-shell {orientation}"),
        SurfaceClass::Trail => format!("trail-shell {orientation}"),
        SurfaceClass::Rail => format!("rail-shell {orientation}"),
        SurfaceClass::Aerial => format!("aerialway-shell {orientation}"),
        SurfaceClass::Ferry => format!("ferry-shell {orientation}"),
        SurfaceClass::Marker => format!("marker {orientation}"),
        SurfaceClass::RouteTrail => format!("mapped-trail-shell {orientation}"),
        _ => {
            let zeroes = corners
                .iter()
                .filter(|corner| corner[2].abs() <= 1e-6)
                .count();
            let part = if zeroes == 3 {
                "floor"
            } else if orientation == "wall" {
                "wall"
            } else if mesh.name.contains("tray") {
                "tray-surface"
            } else {
                "terrain-top"
            };
            if mesh.name.contains("tray") {
                format!("tray {part} ({material:?})")
            } else {
                format!("terrain {part}")
            }
        }
    }
}

fn face_normal(corners: [[f32; 3]; 3]) -> [f32; 3] {
    let ab = [
        corners[1][0] - corners[0][0],
        corners[1][1] - corners[0][1],
        corners[1][2] - corners[0][2],
    ];
    let ac = [
        corners[2][0] - corners[0][0],
        corners[2][1] - corners[0][1],
        corners[2][2] - corners[0][2],
    ];
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

fn triangle_area(corners: [[f32; 3]; 3]) -> f64 {
    let ab = [
        (corners[1][0] - corners[0][0]) as f64,
        (corners[1][1] - corners[0][1]) as f64,
        (corners[1][2] - corners[0][2]) as f64,
    ];
    let ac = [
        (corners[2][0] - corners[0][0]) as f64,
        (corners[2][1] - corners[0][1]) as f64,
        (corners[2][2] - corners[0][2]) as f64,
    ];
    let cross = [
        ab[1] * ac[2] - ab[2] * ac[1],
        ab[2] * ac[0] - ab[0] * ac[2],
        ab[0] * ac[1] - ab[1] * ac[0],
    ];
    0.5 * (cross[0] * cross[0] + cross[1] * cross[1] + cross[2] * cross[2]).sqrt()
}

/// Rounds exactly like the 3MF writer's `{:.5}` formatting, so the welded
/// view matches what a slicer parses back out of `3D/3dmodel.model`.
fn three_mf_key(vertex: [f32; 3]) -> String {
    format!("{:.5},{:.5},{:.5}", vertex[0], vertex[1], vertex[2])
}

fn identity_map(mesh: &Mesh) -> Vec<u32> {
    (0..mesh.vertices.len() as u32).collect()
}

fn stl_weld_map(mesh: &Mesh) -> Vec<u32> {
    let mut canonical = HashMap::<[u32; 3], u32>::with_capacity(mesh.vertices.len());
    mesh.vertices
        .iter()
        .enumerate()
        .map(|(index, vertex)| {
            let bits = [
                vertex[0].to_bits(),
                vertex[1].to_bits(),
                vertex[2].to_bits(),
            ];
            *canonical.entry(bits).or_insert(index as u32)
        })
        .collect()
}

fn three_mf_weld_map(mesh: &Mesh) -> Vec<u32> {
    let mut canonical = HashMap::<String, u32>::with_capacity(mesh.vertices.len());
    mesh.vertices
        .iter()
        .enumerate()
        .map(|(index, vertex)| {
            *canonical
                .entry(three_mf_key(*vertex))
                .or_insert(index as u32)
        })
        .collect()
}

/// Rotates a triangle so its smallest index comes first, preserving winding.
/// Two triangles on the same vertex set share this canonical form exactly
/// when they have the same cyclic winding.
fn canonical_rotation(triangle: [u32; 3]) -> [u32; 3] {
    let smallest = (0..3)
        .min_by_key(|position| triangle[*position])
        .expect("triangle has three corners");
    [
        triangle[smallest],
        triangle[(smallest + 1) % 3],
        triangle[(smallest + 2) % 3],
    ]
}

fn analyze_view(mesh: &Mesh, view_name: &str, vertex_map: &[u32]) -> ViewReport {
    let mut report = ViewReport {
        view: view_name.to_owned(),
        degenerate_repeated_index: 0,
        near_zero_area: 0,
        duplicate_same_winding: 0,
        duplicate_opposite_winding: 0,
        open_edges: 0,
        overused_edges: 0,
        slicer_edge_defects: 0,
        misoriented_edges: 0,
        nonmanifold_vertices: 0,
        defect_features: BTreeMap::new(),
        degenerate_examples: Vec::new(),
        duplicate_examples: Vec::new(),
        edge_examples: Vec::new(),
    };
    let attribute = |features: &mut BTreeMap<String, usize>, triangle_index: usize| {
        *features
            .entry(triangle_feature(mesh, triangle_index))
            .or_default() += 1;
    };

    // Pass one: welded triangles, degenerates, duplicates, edge counts.
    let mut edge_uses = HashMap::<(u32, u32), u32>::new();
    let mut directed_edge_uses = HashMap::<(u32, u32), u32>::new();
    let mut face_sets = HashMap::<[u32; 3], Vec<(usize, [u32; 3])>>::new();
    let mut welded = Vec::<(usize, [u32; 3])>::with_capacity(mesh.triangles.len());
    for (triangle_index, triangle) in mesh.triangles.iter().enumerate() {
        let mapped = triangle.map(|index| vertex_map[index as usize]);
        if mapped[0] == mapped[1] || mapped[1] == mapped[2] || mapped[2] == mapped[0] {
            report.degenerate_repeated_index += 1;
            attribute(&mut report.defect_features, triangle_index);
            if report.degenerate_examples.len() < EXAMPLE_CAP {
                let corners = triangle.map(|index| mesh.vertices[index as usize]);
                report.degenerate_examples.push(format!(
                    "triangle {triangle_index} [{}] collapsed to indices {mapped:?} at {corners:?}",
                    triangle_feature(mesh, triangle_index),
                ));
            }
            continue;
        }
        let corners = mapped.map(|index| mesh.vertices[index as usize]);
        if triangle_area(corners) < 1e-8 {
            report.near_zero_area += 1;
            attribute(&mut report.defect_features, triangle_index);
            if report.degenerate_examples.len() < EXAMPLE_CAP {
                report.degenerate_examples.push(format!(
                    "triangle {triangle_index} [{}] has near-zero area at {corners:?}",
                    triangle_feature(mesh, triangle_index),
                ));
            }
        }
        let mut sorted = mapped;
        sorted.sort_unstable();
        face_sets
            .entry(sorted)
            .or_default()
            .push((triangle_index, canonical_rotation(mapped)));
        for edge in [
            (mapped[0], mapped[1]),
            (mapped[1], mapped[2]),
            (mapped[2], mapped[0]),
        ] {
            let ordered = if edge.0 < edge.1 {
                edge
            } else {
                (edge.1, edge.0)
            };
            *edge_uses.entry(ordered).or_default() += 1;
            *directed_edge_uses.entry(edge).or_default() += 1;
        }
        welded.push((triangle_index, mapped));
    }

    // Two consistently wound neighbours traverse their shared edge in
    // opposite directions, so a direction used twice means the faces
    // disagree on winding — even when the undirected count is a clean 2.
    report.misoriented_edges = directed_edge_uses
        .iter()
        .filter(|(_, uses)| **uses >= 2)
        .map(|((from, to), _)| {
            if from < to {
                (*from, *to)
            } else {
                (*to, *from)
            }
        })
        .collect::<std::collections::HashSet<_>>()
        .len();

    // Sorted by vertex triple so example emission is deterministic —
    // HashMap iteration order is not.
    let mut duplicate_sets = face_sets
        .iter()
        .filter(|(_, faces)| faces.len() >= 2)
        .collect::<Vec<_>>();
    duplicate_sets.sort_unstable_by_key(|(sorted, _)| **sorted);
    for (sorted, faces) in duplicate_sets {
        let first_rotation = faces[0].1;
        for (triangle_index, rotation) in &faces[1..] {
            if *rotation == first_rotation {
                report.duplicate_same_winding += 1;
            } else {
                report.duplicate_opposite_winding += 1;
            }
            attribute(&mut report.defect_features, *triangle_index);
            if report.duplicate_examples.len() < EXAMPLE_CAP {
                report.duplicate_examples.push(format!(
                    "triangles {:?} repeat vertex set {sorted:?} ({}) at {:?}",
                    faces.iter().map(|(index, _)| *index).collect::<Vec<_>>(),
                    if *rotation == first_rotation {
                        "same winding"
                    } else {
                        "opposite winding"
                    },
                    sorted.map(|index| mesh.vertices[index as usize]),
                ));
            }
        }
    }

    // Pass two: edge verdicts, with feature attribution for the incident
    // triangles of every defective edge.
    let mut defective_edges = edge_uses
        .iter()
        .filter(|(_, uses)| **uses != 2)
        .map(|(edge, uses)| (*edge, *uses))
        .collect::<Vec<_>>();
    defective_edges.sort_unstable();
    report.open_edges = defective_edges
        .iter()
        .filter(|(_, uses)| *uses == 1)
        .count();
    report.overused_edges = defective_edges.iter().filter(|(_, uses)| *uses > 2).count();
    report.slicer_edge_defects = report.open_edges + report.overused_edges;
    if !defective_edges.is_empty() {
        let defective_set = defective_edges
            .iter()
            .map(|(edge, uses)| (*edge, *uses))
            .collect::<HashMap<_, _>>();
        let mut edge_incident_features = HashMap::<(u32, u32), Vec<String>>::new();
        for (triangle_index, mapped) in &welded {
            for edge in [
                (mapped[0], mapped[1]),
                (mapped[1], mapped[2]),
                (mapped[2], mapped[0]),
            ] {
                let ordered = if edge.0 < edge.1 {
                    edge
                } else {
                    (edge.1, edge.0)
                };
                if defective_set.contains_key(&ordered) {
                    attribute(&mut report.defect_features, *triangle_index);
                    if std::env::var_os("TOPOSAIC_DEBUG_EDGES").is_some() {
                        eprintln!(
                            "[{}] edge {:?}-{:?}: triangle {triangle_index} {:?}",
                            view_name,
                            mesh.vertices[ordered.0 as usize],
                            mesh.vertices[ordered.1 as usize],
                            mesh.triangles[*triangle_index]
                                .map(|index| mesh.vertices[index as usize]),
                        );
                    }
                    edge_incident_features
                        .entry(ordered)
                        .or_default()
                        .push(triangle_feature(mesh, *triangle_index));
                }
            }
        }
        for (edge, uses) in defective_edges.iter().take(EXAMPLE_CAP) {
            let mut features = edge_incident_features.remove(edge).unwrap_or_default();
            features.sort();
            features.dedup();
            report.edge_examples.push(format!(
                "edge used {uses}x between {:?} and {:?} on {}",
                mesh.vertices[edge.0 as usize],
                mesh.vertices[edge.1 as usize],
                features.join(" + "),
            ));
        }
    }

    // Vertex fans: a vertex is manifold when its incident faces form one
    // edge-connected component. Grouping incident faces and joining them
    // over shared incident edges is linear in total face degree.
    let mut incident = HashMap::<u32, Vec<usize>>::new();
    for (list_index, (_, mapped)) in welded.iter().enumerate() {
        for corner in mapped {
            incident.entry(*corner).or_default().push(list_index);
        }
    }
    let mut vertices_sorted = incident.keys().copied().collect::<Vec<_>>();
    vertices_sorted.sort_unstable();
    for vertex in vertices_sorted {
        let faces = &incident[&vertex];
        if faces.len() < 2 {
            continue;
        }
        // Union faces that share an edge through this vertex.
        let mut parent = (0..faces.len()).collect::<Vec<_>>();
        fn root(parent: &mut [usize], mut index: usize) -> usize {
            while parent[index] != index {
                parent[index] = parent[parent[index]];
                index = parent[index];
            }
            index
        }
        let mut neighbor_owner = HashMap::<u32, Vec<usize>>::new();
        for (position, face) in faces.iter().enumerate() {
            let mapped = welded[*face].1;
            for corner in mapped {
                if corner != vertex {
                    neighbor_owner.entry(corner).or_default().push(position);
                }
            }
        }
        for owners in neighbor_owner.values() {
            for pair in owners.windows(2) {
                let left = root(&mut parent, pair[0]);
                let right = root(&mut parent, pair[1]);
                if left != right {
                    parent[left] = right;
                }
            }
        }
        let mut components = 0;
        for index in 0..faces.len() {
            if root(&mut parent, index) == index {
                components += 1;
            }
        }
        if components > 1 {
            report.nonmanifold_vertices += 1;
        }
    }

    report
}

pub(crate) fn analyze_mesh_views(mesh: &Mesh) -> MeshReport {
    let views = vec![
        analyze_view(mesh, "indexed", &identity_map(mesh)),
        analyze_view(mesh, "stl-weld", &stl_weld_map(mesh)),
        analyze_view(mesh, "3mf-weld", &three_mf_weld_map(mesh)),
    ];
    MeshReport {
        name: mesh.name.clone(),
        triangles: mesh.triangles.len(),
        vertices: mesh.vertices.len(),
        quantization_collisions: mesh.quantization_collisions.len(),
        collision_examples: mesh
            .quantization_collisions
            .iter()
            .take(EXAMPLE_CAP)
            .map(|(kept, dropped)| format!("kept {kept:?}, dropped {dropped:?}"))
            .collect(),
        views,
    }
}

/// Builds one piece exactly as production does and analyzes it.
///
/// Validates the spec first, exactly like the production entry points: the
/// builders assume validated ranges (for example a tray contour count of at
/// least 5) and can panic on values validation would have rejected.
pub fn analyze_piece(
    spec: &GenerationSpec,
    height_field: Option<&HeightField>,
    surface_field: Option<&SurfaceField>,
    row: u32,
    column: u32,
) -> Result<MeshReport> {
    spec.validate()?;
    let height_range = height_range_for_spec(spec, height_field);
    let mesh = build_piece_with_height_range(
        spec,
        height_field,
        height_range,
        surface_field,
        row,
        column,
    )?;
    Ok(analyze_mesh_views(&mesh))
}

/// Builds and analyzes every piece of a project (or the one solid model),
/// plus the tray segments when the spec enables a tray.
pub fn analyze_project(
    spec: &GenerationSpec,
    height_field: Option<&HeightField>,
    surface_field: Option<&SurfaceField>,
) -> Result<Vec<MeshReport>> {
    spec.validate()?;
    let mut reports = Vec::new();
    let (rows, columns) = if spec.solid_model {
        (1, 1)
    } else {
        (spec.rows, spec.columns)
    };
    for row in 0..rows {
        for column in 0..columns {
            reports.push(analyze_piece(
                spec,
                height_field,
                surface_field,
                row,
                column,
            )?);
        }
    }
    if spec.tray.enabled {
        for mesh in build_tray_segments(spec, height_field)? {
            reports.push(analyze_mesh_views(&mesh));
        }
    }
    Ok(reports)
}

/// Renders an aggregate, human-readable summary for one scenario.
pub fn summarize(scenario: &str, reports: &[MeshReport]) -> String {
    use std::fmt::Write;

    let mut output = String::new();
    let _ = writeln!(output, "=== {scenario}: {} meshes ===", reports.len());
    let _ = writeln!(
        output,
        "{:<10} {:>7} {:>12} {:>6} {:>8} {:>7} {:>9} {:>8} {:>8} {:>8} {:>8}",
        "view",
        "meshes*",
        "slicer-edges",
        "open",
        "overused",
        "degen",
        "zero-area",
        "dup-same",
        "dup-opp",
        "mis-wind",
        "nm-verts"
    );
    for view_name in ["indexed", "stl-weld", "3mf-weld"] {
        let views = reports
            .iter()
            .map(|report| report.view(view_name))
            .collect::<Vec<_>>();
        let _ = writeln!(
            output,
            "{:<10} {:>7} {:>12} {:>6} {:>8} {:>7} {:>9} {:>8} {:>8} {:>8} {:>8}",
            view_name,
            views.iter().filter(|view| !view.is_clean()).count(),
            views
                .iter()
                .map(|view| view.slicer_edge_defects)
                .sum::<usize>(),
            views.iter().map(|view| view.open_edges).sum::<usize>(),
            views.iter().map(|view| view.overused_edges).sum::<usize>(),
            views
                .iter()
                .map(|view| view.degenerate_repeated_index)
                .sum::<usize>(),
            views.iter().map(|view| view.near_zero_area).sum::<usize>(),
            views
                .iter()
                .map(|view| view.duplicate_same_winding)
                .sum::<usize>(),
            views
                .iter()
                .map(|view| view.duplicate_opposite_winding)
                .sum::<usize>(),
            views
                .iter()
                .map(|view| view.misoriented_edges)
                .sum::<usize>(),
            views
                .iter()
                .map(|view| view.nonmanifold_vertices)
                .sum::<usize>(),
        );
    }
    let _ = writeln!(output, "(* meshes with any defect in that view)");

    let collisions = reports
        .iter()
        .map(|report| report.quantization_collisions)
        .sum::<usize>();
    let _ = writeln!(
        output,
        "quantization collisions: {collisions} total across {} meshes",
        reports
            .iter()
            .filter(|report| report.quantization_collisions > 0)
            .count()
    );

    for view_name in ["stl-weld", "3mf-weld"] {
        let mut histogram = BTreeMap::<&str, usize>::new();
        for report in reports {
            for (feature, count) in &report.view(view_name).defect_features {
                *histogram.entry(feature).or_default() += count;
            }
        }
        if !histogram.is_empty() {
            let _ = writeln!(output, "defect attribution ({view_name}):");
            for (feature, count) in histogram {
                let _ = writeln!(output, "  {feature}: {count}");
            }
        }
    }

    let mut dirty = reports
        .iter()
        .filter(|report| !report.view("stl-weld").is_clean() || !report.view("3mf-weld").is_clean())
        .collect::<Vec<_>>();
    dirty.sort_by_key(|report| usize::MAX - report.view("3mf-weld").slicer_edge_defects);
    if !dirty.is_empty() {
        let _ = writeln!(output, "meshes with export-view defects:");
        for report in dirty.iter().take(20) {
            let _ = writeln!(
                output,
                "  {}: slicer-edges stl={} 3mf={}, degen stl={} 3mf={}, collisions={}",
                report.name,
                report.view("stl-weld").slicer_edge_defects,
                report.view("3mf-weld").slicer_edge_defects,
                report.view("stl-weld").degenerate_repeated_index,
                report.view("3mf-weld").degenerate_repeated_index,
                report.quantization_collisions,
            );
        }
        if let Some(worst) = dirty.first() {
            let view = worst.view("3mf-weld");
            let examples = view
                .edge_examples
                .iter()
                .chain(&view.degenerate_examples)
                .chain(&view.duplicate_examples)
                .chain(&worst.collision_examples);
            let _ = writeln!(output, "examples from {} (3mf-weld):", worst.name);
            for example in examples.take(EXAMPLE_CAP) {
                let _ = writeln!(output, "  {example}");
            }
        }
    }
    output
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::spec::GenerationSpec;

    #[test]
    fn clean_piece_reports_no_defects_in_any_view() {
        let report = analyze_piece(&GenerationSpec::default(), None, None, 0, 0).unwrap();
        for view in &report.views {
            assert!(
                view.is_clean(),
                "expected clean {} view: {view:?}",
                view.view
            );
        }
        assert_eq!(report.quantization_collisions, 0);
    }

    #[test]
    fn overlay_shell_triangles_attribute_to_their_own_features() {
        let mut mesh = Mesh {
            name: "piece".into(),
            vertices: vec![[0.0, 0.0, 1.0], [1.0, 0.0, 1.0], [0.0, 1.0, 1.0]],
            triangles: vec![[0, 1, 2]],
            materials: vec![SurfaceClass::Trail],
            quantization_collisions: Vec::new(),
        };
        assert_eq!(triangle_feature(&mesh, 0), "trail-shell top");
        mesh.materials[0] = SurfaceClass::Rail;
        assert_eq!(triangle_feature(&mesh, 0), "rail-shell top");
        // A vertical face of the same shell reads as a wall, not terrain.
        mesh.vertices[2] = [0.0, 0.0, 2.0];
        assert_eq!(triangle_feature(&mesh, 0), "rail-shell wall");
    }

    #[test]
    fn welding_views_catch_defects_the_indexed_view_hides() {
        // Two triangles that share an edge geometrically but not by index,
        // plus a sliver: indexed sees open edges, and welds must agree once
        // the duplicate positions merge.
        let mesh = Mesh {
            name: "synthetic".into(),
            vertices: vec![
                [0.0, 0.0, 0.0],
                [1.0, 0.0, 0.0],
                [0.0, 1.0, 0.0],
                [1.0, 0.0, 0.0], // duplicate of vertex 1
                [0.0, 1.0, 0.0], // duplicate of vertex 2
                [1.0, 1.0, 0.0],
            ],
            triangles: vec![[0, 1, 2], [3, 5, 4]],
            materials: vec![SurfaceClass::Rock; 2],
            quantization_collisions: Vec::new(),
        };
        let report = analyze_mesh_views(&mesh);
        assert_eq!(report.view("indexed").open_edges, 6);
        assert_eq!(report.view("stl-weld").open_edges, 4);
        assert_eq!(report.view("3mf-weld").open_edges, 4);
    }

    #[test]
    fn same_direction_shared_edges_count_as_misoriented() {
        // Two faces sharing edge 0->1 in the SAME direction: the undirected
        // count is a clean 2, so only the directed counter can see that one
        // face is wound against its neighbour.
        let mesh = Mesh {
            name: "synthetic".into(),
            vertices: vec![
                [0.0, 0.0, 0.0],
                [1.0, 0.0, 0.0],
                [0.0, 1.0, 0.0],
                [0.5, -1.0, 0.0],
            ],
            triangles: vec![[0, 1, 2], [0, 1, 3]],
            materials: vec![SurfaceClass::Rock; 2],
            quantization_collisions: Vec::new(),
        };
        let report = analyze_mesh_views(&mesh);
        for view in &report.views {
            assert_eq!(view.misoriented_edges, 1, "{}", view.view);
            assert!(!view.is_clean());
        }

        // Flipping the second face restores opposite traversal.
        let consistent = Mesh {
            triangles: vec![[0, 1, 2], [1, 0, 3]],
            ..mesh
        };
        let report = analyze_mesh_views(&consistent);
        for view in &report.views {
            assert_eq!(view.misoriented_edges, 0, "{}", view.view);
        }
    }

    #[test]
    fn invalid_specs_are_rejected_before_any_build_runs() {
        // A zero contour count reaches an arithmetic underflow in the tray
        // tracer when validation is skipped; the analyzers must validate
        // exactly like the production entry points.
        let mut spec = GenerationSpec::default();
        spec.tray.enabled = true;
        spec.tray.contour_count = 0;
        // Assert on the message, not merely on `is_err`: any other
        // validation rule firing first would satisfy `is_err` and let the
        // underflow back in unnoticed.
        for error in [
            analyze_project(&spec, None, None).unwrap_err(),
            analyze_piece(&spec, None, None, 0, 0).unwrap_err(),
        ] {
            assert!(
                error.to_string().contains("contour count"),
                "expected the contour-count rule to reject this spec, got: {error}"
            );
        }
    }

    #[test]
    fn three_mf_weld_catches_sub_precision_vertex_pairs() {
        // Two vertices 2e-6 apart survive the STL byte-exact weld but merge
        // in the 5-decimal 3MF text, collapsing the second triangle.
        let mesh = Mesh {
            name: "synthetic".into(),
            vertices: vec![
                [0.0, 0.0, 0.0],
                [1.0, 0.0, 0.0],
                [0.0, 1.0, 0.0],
                [0.000_002, 1.0, 0.0],
            ],
            triangles: vec![[0, 1, 2], [0, 2, 3]],
            materials: vec![SurfaceClass::Rock; 2],
            quantization_collisions: Vec::new(),
        };
        let report = analyze_mesh_views(&mesh);
        assert_eq!(report.view("stl-weld").degenerate_repeated_index, 0);
        assert_eq!(report.view("3mf-weld").degenerate_repeated_index, 1);
    }
}
