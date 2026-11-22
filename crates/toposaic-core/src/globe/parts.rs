use anyhow::Result;
use geo::Polygon;

use super::layout::{Face, Loop, Pose, Vec3, add, dot, sub, unit};
use super::region::Region;
use super::{GlobeExterior, GlobePrototypeSpec};
use crate::mesh::{Mesh, MeshBuilder, weld_export_mesh};
use crate::planar_mesh::{add_horizontal_polygons, closed_ring, polygon_from_outline};
use crate::spec::SurfaceClass;

const FRAME_TOP: f32 = -1.4;
const FRAME_BOTTOM: f32 = -6.4;
const FRAME_EDGE_GAP: f32 = 0.4;
const BRACKET_DEPTH: f32 = 3.8;
const BOLT_RADIUS: f32 = 1.75;
const HEAD_RADIUS: f32 = 3.1;
const HARDWARE_Z: f32 = -4.1;
const SOCKET_Y: f32 = 2.6;
const SOCKET_DEPTH: f32 = 5.7;
const TILE_THROAT_X: f32 = 3.95;
const FRAME_THROAT_X: f32 = 3.8;
const LOCATOR_Y: f32 = 8.0;
const MATERIAL: SurfaceClass = SurfaceClass::Rock;

fn lift([x, y]: [f32; 2], z: f32) -> Vec3 {
    [x, y, z]
}

fn rectangle(x: f32, y: f32, half_x: f32, half_y: f32) -> Loop {
    vec![
        [x - half_x, y - half_y],
        [x + half_x, y - half_y],
        [x + half_x, y + half_y],
        [x - half_x, y + half_y],
    ]
}

fn circle(center: [f32; 2], radius: f32) -> Loop {
    (0..20)
        .map(|i| {
            let angle = std::f32::consts::TAU * i as f32 / 20.0;
            [
                center[0] + angle.cos() * radius,
                center[1] + angle.sin() * radius,
            ]
        })
        .collect()
}

fn cap(
    mesh: &mut MeshBuilder,
    outer: &[[f32; 2]],
    holes: &[Loop],
    z: f32,
    down: bool,
) -> Result<()> {
    let polygon = Polygon::new(
        closed_ring(outer),
        holes.iter().map(|ring| closed_ring(ring)).collect(),
    );
    add_horizontal_polygons(mesh, &[polygon], z, MATERIAL, down)
}

fn wall(
    mesh: &mut MeshBuilder,
    low: &[[f32; 2]],
    low_z: f32,
    high: &[[f32; 2]],
    high_z: f32,
    inside: bool,
) {
    for i in 0..low.len() {
        let next = (i + 1) % low.len();
        let [a, b, c, d] = [
            lift(low[i], low_z),
            lift(low[next], low_z),
            lift(high[next], high_z),
            lift(high[i], high_z),
        ];
        if inside {
            mesh.quad(b, a, d, c, MATERIAL);
        } else {
            mesh.quad(a, b, c, d, MATERIAL);
        }
    }
}

fn cavity(mesh: &mut MeshBuilder, levels: &[(Loop, f32)]) {
    for pair in levels.windows(2) {
        let [(low, low_z), (high, high_z)] = pair else {
            unreachable!()
        };
        wall(mesh, low, *low_z, high, *high_z, true);
    }
}

fn weights(corners: [[f32; 2]; 3], weights: [f32; 3]) -> [f32; 2] {
    std::array::from_fn(|axis| corners.iter().zip(weights).map(|(p, w)| p[axis] * w).sum())
}

pub(super) fn build_tile(
    face: &Face,
    spec: &GlobePrototypeSpec,
    region: Option<&Region>,
) -> Result<Mesh> {
    let outline = face.outline_at(0.0, spec.seam_gap_mm / 2.0);
    let segments = if region.is_some() {
        40
    } else if spec.exterior == GlobeExterior::Rounded {
        24
    } else {
        1
    };
    let mut mesh = MeshBuilder::default();
    let base_radius = dot(face.pose.origin, face.pose.z_axis);
    let nominal_outline = face.outline_at(0.0, 0.0);
    let surface = |i, j| {
        let u = i as f32 / segments as f32;
        let v = j as f32 / segments as f32;
        let [x, y] = weights(outline, [1.0 - u - v, u, v]);
        let mut factor = match spec.exterior {
            GlobeExterior::Faceted => face.chord_radius / base_radius,
            GlobeExterior::Rounded => face.radius / x.hypot(y).hypot(base_radius),
        };
        if let Some(region) = region {
            // Sample the uncut triangle, so both sides of a seam receive
            // the same terrain before the physical clearance is applied.
            let [sample_x, sample_y] = weights(nominal_outline, [1.0 - u - v, u, v]);
            let direction = unit(face.pose.to_globe([sample_x, sample_y, 0.0]));
            factor += region.height(direction) / x.hypot(y).hypot(base_radius);
        }
        [x * factor, y * factor, base_radius * (factor - 1.0)]
    };
    for i in 0..segments {
        for j in 0..segments - i {
            let a = surface(i, j);
            let b = surface(i + 1, j);
            let c = surface(i, j + 1);
            mesh.triangle(a, b, c, MATERIAL);
            if i + j + 1 < segments {
                mesh.triangle(b, surface(i + 1, j + 1), c, MATERIAL);
            }
        }
    }
    let mut bottom = Vec::with_capacity(segments * 3);
    for (edge, index) in (0..3).flat_map(|edge| (0..segments).map(move |i| (edge, i))) {
        let lattice = |i| match edge {
            0 => (i, 0),
            1 => (segments - i, i),
            _ => (0, segments - i),
        };
        let (i, j) = lattice(index);
        let (next_i, next_j) = lattice(index + 1);
        let base = |i, j| {
            let u = i as f32 / segments as f32;
            let v = j as f32 / segments as f32;
            weights(outline, [1.0 - u - v, u, v])
        };
        let a = base(i, j);
        bottom.push(a);
        mesh.quad(
            lift(a, 0.0),
            lift(base(next_i, next_j), 0.0),
            surface(next_i, next_j),
            surface(i, j),
            MATERIAL,
        );
    }

    // A short throat deflects the clip. The flare inside the back acts as a
    // release ramp, and the small bridge closes the blind socket.
    let socket = [
        (rectangle(0.0, 0.0, TILE_THROAT_X, SOCKET_Y), 0.0),
        (rectangle(0.0, 0.0, TILE_THROAT_X, SOCKET_Y), 2.25),
        (rectangle(0.0, 0.0, 4.55, SOCKET_Y), 3.25),
        (rectangle(0.0, 0.0, 4.55, SOCKET_Y), SOCKET_DEPTH),
    ];
    let locator = rectangle(0.0, LOCATOR_Y, 2.25, 1.85);
    cap(
        &mut mesh,
        &bottom,
        &[socket[0].0.clone(), locator.clone()],
        0.0,
        true,
    )?;
    cavity(&mut mesh, &socket);
    cap(&mut mesh, &socket[3].0, &[], SOCKET_DEPTH, true)?;
    wall(&mut mesh, &locator, 0.0, &locator, 2.5, true);
    cap(&mut mesh, &locator, &[], 2.5, true)?;
    Ok(mesh.finish(format!("tile-{:02}", face.id)))
}

struct FrameEdge {
    midpoint: [f32; 2],
    along: [f32; 2],
}

impl FrameEdge {
    fn new(face: &Face, edge: usize) -> Self {
        let [a, b, c] = face.outline_at(FRAME_BOTTOM, 0.0);
        let [a, b] = [[a, b], [b, c], [c, a]][edge];
        let length = (a[0] - b[0]).hypot(a[1] - b[1]);
        Self {
            midpoint: [(a[0] + b[0]) / 2.0, (a[1] + b[1]) / 2.0],
            along: [(b[0] - a[0]) / length, (b[1] - a[1]) / length],
        }
    }

    fn at(&self, along: f32, inward: f32) -> [f32; 2] {
        let [x, y] = self.along;
        [
            self.midpoint[0] + x * along - y * inward,
            self.midpoint[1] + y * along + x * inward,
        ]
    }

    fn bolts(&self) -> [[f32; 2]; 2] {
        [-4.6, 4.6].map(|offset| self.at(offset, 7.0))
    }
}

pub(super) fn build_frame(face: &Face, connected_edges: &[usize]) -> Result<(Mesh, Pose)> {
    let outer = face.outline_at(FRAME_BOTTOM, FRAME_EDGE_GAP);
    let windows: Vec<_> = (0..3)
        .map(|corner| {
            [[0.78, 0.11, 0.11], [0.55, 0.34, 0.11], [0.55, 0.11, 0.34]]
                .map(|row| {
                    weights(
                        [
                            outer[corner],
                            outer[(corner + 1) % 3],
                            outer[(corner + 2) % 3],
                        ],
                        row,
                    )
                })
                .to_vec()
        })
        .collect();
    let pads: Vec<_> = (0..3)
        .map(|corner| {
            let mut barycentric = [0.07; 3];
            barycentric[corner] = 0.86;
            circle(weights(outer, barycentric), 2.5)
        })
        .collect();
    let locator = rectangle(0.0, LOCATOR_Y, 2.0, 1.6);
    let bolts: Vec<_> = connected_edges
        .iter()
        .flat_map(|&edge| FrameEdge::new(face, edge).bolts())
        .collect();
    let clip_slot = [
        (rectangle(0.0, 0.0, 4.55, SOCKET_Y), FRAME_BOTTOM),
        (rectangle(0.0, 0.0, 4.55, SOCKET_Y), FRAME_TOP - 3.25),
        (
            rectangle(0.0, 0.0, FRAME_THROAT_X, SOCKET_Y),
            FRAME_TOP - 2.25,
        ),
        (rectangle(0.0, 0.0, FRAME_THROAT_X, SOCKET_Y), FRAME_TOP),
    ];
    let mut mesh = MeshBuilder::default();
    let mut bottom_holes = windows.clone();
    let mut top_holes = windows.clone();
    bottom_holes.push(clip_slot[0].0.clone());
    top_holes.push(clip_slot[3].0.clone());
    for center in bolts {
        let bore = circle(center, BOLT_RADIUS);
        let head = circle(center, HEAD_RADIUS);
        wall(&mut mesh, &bore, FRAME_BOTTOM, &bore, HARDWARE_Z, true);
        wall(&mut mesh, &head, HARDWARE_Z, &head, FRAME_TOP, true);
        cap(
            &mut mesh,
            &head,
            std::slice::from_ref(&bore),
            HARDWARE_Z,
            false,
        )?;
        bottom_holes.push(bore);
        top_holes.push(head);
    }
    for pad in &pads {
        wall(&mut mesh, pad, FRAME_TOP, pad, 0.0, false);
        cap(&mut mesh, pad, &[], 0.0, false)?;
    }
    for window in &windows {
        wall(&mut mesh, window, FRAME_BOTTOM, window, FRAME_TOP, true);
    }
    wall(&mut mesh, &locator, FRAME_TOP, &locator, 2.0, false);
    cap(&mut mesh, &locator, &[], 2.0, false)?;
    top_holes.extend(pads);
    top_holes.push(locator);
    cavity(&mut mesh, &clip_slot);
    wall(&mut mesh, &outer, FRAME_BOTTOM, &outer, FRAME_TOP, false);
    cap(&mut mesh, &outer, &bottom_holes, FRAME_BOTTOM, true)?;
    cap(&mut mesh, &outer, &top_holes, FRAME_TOP, false)?;
    let mut mesh = mesh.finish(format!("frame-{:02}", face.id));
    for point in &mut mesh.vertices {
        point[2] -= FRAME_BOTTOM;
    }
    let pose = Pose {
        origin: face.pose.to_globe([0.0, 0.0, FRAME_BOTTOM]),
        ..face.pose
    };
    Ok((mesh, pose))
}

pub(super) fn build_clip() -> Result<(Mesh, Pose)> {
    // Print on the side so the two double-ended spring beams lie in XY.
    // The broad collar stops against the top of the frame and sets engagement.
    let right = [
        [2.25, -0.2],
        [2.05, 5.2],
        [3.15, 5.2],
        [4.35, 3.65],
        [3.55, 2.55],
        [3.35, -0.2],
        [5.7, -0.2],
        [5.7, FRAME_TOP],
        [3.35, FRAME_TOP],
        [3.55, FRAME_TOP - 2.75],
        [4.35, FRAME_TOP - 3.85],
        [3.15, FRAME_TOP - 5.4],
        [2.05, FRAME_TOP - 5.4],
        [2.25, FRAME_TOP],
    ];
    let mut outline: Loop = right
        .into_iter()
        .chain(right.into_iter().rev().map(|[x, z]| [-x, z]))
        .collect();
    outline.reverse();
    let mut mesh = MeshBuilder::default();
    let polygon = polygon_from_outline(&outline);
    add_horizontal_polygons(
        &mut mesh,
        std::slice::from_ref(&polygon),
        0.0,
        MATERIAL,
        true,
    )?;
    add_horizontal_polygons(&mut mesh, &[polygon], 4.6, MATERIAL, false)?;
    wall(&mut mesh, &outline, 0.0, &outline, 4.6, false);
    let in_face = Pose::new([0.0, 2.3, 0.0], [1.0, 0.0, 0.0], [0.0, -1.0, 0.0]);
    Ok((mesh.finish("snap-clip"), in_face))
}

pub(super) fn build_bracket(a: &Face, b: &Face) -> Result<(Mesh, Pose)> {
    let edge = FrameEdge::new(a, a.edge_with(b).expect("adjacent bracket faces"));
    let ends =
        [-10.0, 10.0].map(|offset| a.pose.to_globe(lift(edge.at(offset, 0.0), FRAME_BOTTOM)));
    let normal = unit(add(a.pose.z_axis, b.pose.z_axis));
    let print_z = unit(sub(ends[1], ends[0]));
    let pose = Pose::new(ends[0], normal, print_z);
    let seam_bottom_x = -BRACKET_DEPTH / dot(normal, a.pose.z_axis);
    let mut combined = MeshBuilder::default();
    for (face, neighbor, ends) in [(a, b, ends), (b, a, [ends[1], ends[0]])] {
        let edge = FrameEdge::new(
            face,
            face.edge_with(neighbor).expect("adjacent bracket faces"),
        );
        let [start, end] = ends.map(|point| {
            let [x, y, _] = face.pose.to_local(point);
            [x, y]
        });
        let upper = [start, end, edge.at(10.0, 13.0), edge.at(-10.0, 13.0)];
        let base_radius = dot(face.pose.origin, face.pose.z_axis) + FRAME_BOTTOM;
        let reduction = BRACKET_DEPTH / base_radius;
        let lower = upper.map(|point| {
            let along = (point[0] - edge.midpoint[0]) * edge.along[0]
                + (point[1] - edge.midpoint[1]) * edge.along[1];
            std::array::from_fn(|i| point[i] - reduction * (point[i] - along * edge.along[i]))
        });
        let lower_z = FRAME_BOTTOM - BRACKET_DEPTH;
        let bores = edge.bolts().map(|center| circle(center, BOLT_RADIUS));
        let mut half = MeshBuilder::default();
        cap(&mut half, &upper, &bores, FRAME_BOTTOM, false)?;
        cap(&mut half, &lower, &bores, lower_z, true)?;
        for hole in &bores {
            wall(&mut half, hole, lower_z, hole, FRAME_BOTTOM, true);
        }
        for i in 1..4 {
            let next = (i + 1) % 4;
            half.quad(
                lift(lower[i], lower_z),
                lift(lower[next], lower_z),
                lift(upper[next], FRAME_BOTTOM),
                lift(upper[i], FRAME_BOTTOM),
                MATERIAL,
            );
        }
        let half = half.finish("bracket-half");
        for triangle in half.triangles {
            let [p, q, r] = triangle.map(|i| {
                let mut point = pose.to_local(face.pose.to_globe(half.vertices[i as usize]));
                for value in &mut point {
                    if value.abs() < 0.000_02 {
                        *value = 0.0;
                    }
                }
                if point[1] == 0.0 {
                    point[0] = if point[0] < -1.0 { seam_bottom_x } else { 0.0 };
                    point[2] = if point[2] < 10.0 { 0.0 } else { 20.0 };
                }
                point
            });
            combined.triangle(p, q, r, MATERIAL);
        }
    }
    let mut mesh = combined.finish(format!("joint-{:02}-{:02}", a.id, b.id));
    weld_export_mesh(&mut mesh);
    Ok((mesh, pose))
}
