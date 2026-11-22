use anyhow::Result;
use geo::{Intersects, Polygon};

use super::layout::{Face, Loop, Pose, Vec3, add, dot, scale, sub, unit};
use super::{GlobeExterior, GlobePrototypeSpec, GlobeSource, Terrain};
use crate::mesh::{Mesh, MeshBuilder, weld_export_mesh};
use crate::planar_mesh::{add_horizontal_polygons, closed_ring, polygon_from_outline};
use crate::spec::SurfaceClass;

const FRAME_TOP: f32 = -1.4;
const FRAME_EDGE_GAP: f32 = 0.4;
const BRACKET_DEPTH: f32 = 3.8;
const BOLT_RADIUS: f32 = 1.75;
const HEAD_RADIUS: f32 = 3.1;
const HARDWARE_Z: f32 = -4.1;
const SOCKET_Y: f32 = 2.6;
const SOCKET_DEPTH: f32 = 5.7;
const CLIP_BARBS_X: f32 = 4.35;
const FRAME_THROAT_X: f32 = 3.8;
const LOCATOR_Y: f32 = 8.0;
const MATERIAL: SurfaceClass = SurfaceClass::Rock;

fn frame_bottom(face: &Face, spec: &GlobePrototypeSpec) -> f32 {
    (FRAME_TOP - spec.frame_depth_mm) * face.depth_scale()
}

pub(super) fn screw_length(spec: &GlobePrototypeSpec, max_depth_scale: f32) -> u32 {
    let stack = (spec.frame_depth_mm - FRAME_TOP + BRACKET_DEPTH) * max_depth_scale + HARDWARE_Z;
    // Include a 2.4 mm M3 nut and enough projection to start the threads.
    [10, 12, 14, 16, 18, 20, 22, 25]
        .into_iter()
        .find(|&length| length as f32 >= stack + 3.4)
        .unwrap_or(25)
}

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

pub(super) fn tile_segments(spec: &GlobePrototypeSpec) -> usize {
    if spec.source != GlobeSource::FitTest {
        usize::from(spec.samples_per_edge)
    } else if spec.exterior == GlobeExterior::Rounded {
        24
    } else {
        1
    }
}

pub(super) fn build_tile(
    face: &Face,
    spec: &GlobePrototypeSpec,
    terrain: &Terrain,
    face_count: usize,
) -> Result<Mesh> {
    let outline = face.outline_at(0.0, spec.seam_gap_mm / 2.0);
    let nominal_outline = face.outline_at(0.0, 0.0);
    let mapped = spec.source != GlobeSource::FitTest;
    let segments = tile_segments(spec);
    let mut mesh = MeshBuilder::default();
    let base_radius = dot(face.pose.origin, face.pose.z_axis);
    let direction = |u, v| {
        let [x, y] = weights(nominal_outline, [1.0 - u - v, u, v]);
        unit(face.pose.to_globe([x, y, 0.0]))
    };
    let surface_positions = (0..=segments)
        .map(|i| {
            (0..=segments - i)
                .map(|j| {
                    let u = i as f32 / segments as f32;
                    let v = j as f32 / segments as f32;
                    let [x, y] = weights(outline, [1.0 - u - v, u, v]);
                    let length = x.hypot(y).hypot(base_radius);
                    let mut factor = match spec.exterior {
                        GlobeExterior::Faceted => face.chord_radius / base_radius,
                        GlobeExterior::Rounded => face.radius / length,
                    };
                    if mapped {
                        // Sample the uncut triangle before applying the seam clearance.
                        let direction = direction(u, v);
                        let ground = terrain.sample(direction);
                        let frequency = (face_count as f32 / 20.0).sqrt();
                        let wavelength = (spec.diameter_mm / (20.0 * frequency)).max(4.0);
                        let texture =
                            ground.texture(direction, face.radius, wavelength, spec.texture_mm);
                        factor += (ground.height + texture) / length;
                    }
                    [x * factor, y * factor, base_radius * (factor - 1.0)]
                })
                .collect::<Vec<_>>()
        })
        .collect::<Vec<_>>();
    let surface = |i: usize, j: usize| surface_positions[i][j];
    let material = |u, v| {
        if spec.surface_colors && mapped {
            terrain
                .sample(direction(u / segments as f32, v / segments as f32))
                .class()
        } else {
            MATERIAL
        }
    };
    for i in 0..segments {
        for j in 0..segments - i {
            let a = surface(i, j);
            let b = surface(i + 1, j);
            let c = surface(i, j + 1);
            mesh.triangle(
                a,
                b,
                c,
                material(i as f32 + 1.0 / 3.0, j as f32 + 1.0 / 3.0),
            );
            if i + j + 1 < segments {
                mesh.triangle(
                    b,
                    surface(i + 1, j + 1),
                    c,
                    material(i as f32 + 2.0 / 3.0, j as f32 + 2.0 / 3.0),
                );
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
        (
            rectangle(0.0, 0.0, CLIP_BARBS_X - spec.clip_fit_mm, SOCKET_Y),
            0.0,
        ),
        (
            rectangle(0.0, 0.0, CLIP_BARBS_X - spec.clip_fit_mm, SOCKET_Y),
            2.25,
        ),
        (rectangle(0.0, 0.0, 4.55, SOCKET_Y), 3.25),
        (rectangle(0.0, 0.0, 4.55, SOCKET_Y), SOCKET_DEPTH),
    ];
    let clearance = spec.locator_clearance_mm;
    let locator = rectangle(0.0, LOCATOR_Y, 2.0 + clearance, 1.6 + clearance);
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
    fn new(face: &Face, edge: usize, spec: &GlobePrototypeSpec) -> Self {
        let [a, b, c] = face.outline_at(frame_bottom(face, spec), 0.0);
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

pub(super) fn build_frame(
    face: &Face,
    connected_edges: &[usize],
    spec: &GlobePrototypeSpec,
) -> Result<(Mesh, Pose)> {
    let bottom = frame_bottom(face, spec);
    let outer = face.outline_at(bottom, FRAME_EDGE_GAP);
    let window_corners = (0..3).map(|corner| {
        [[0.78, 0.11, 0.11], [0.55, 0.34, 0.11], [0.55, 0.11, 0.34]].map(|row| {
            weights(
                [
                    outer[corner],
                    outer[(corner + 1) % 3],
                    outer[(corner + 2) % 3],
                ],
                row,
            )
        })
    });
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
        .flat_map(|&edge| FrameEdge::new(face, edge, spec).bolts())
        .collect();
    let window_keepouts = bolts
        .iter()
        .map(|&center| circle(center, HEAD_RADIUS + 1.0))
        .chain(pads.iter().cloned())
        .chain([
            rectangle(0.0, 0.0, 5.55, SOCKET_Y + 1.0),
            rectangle(0.0, LOCATOR_Y, 3.0, 2.6),
        ])
        .map(|outline| polygon_from_outline(&outline))
        .collect::<Vec<_>>();
    let windows: Vec<_> = window_corners
        .filter_map(|corners| {
            let center = weights(corners, [1.0 / 3.0; 3]);
            [1.0, 0.75, 0.5].into_iter().find_map(|scale| {
                let window = corners.map(|point| {
                    std::array::from_fn(|axis| center[axis] + (point[axis] - center[axis]) * scale)
                });
                let polygon = polygon_from_outline(&window);
                window_keepouts
                    .iter()
                    .all(|keepout| !polygon.intersects(keepout))
                    .then(|| window.to_vec())
            })
        })
        .collect();
    let frame_throat = FRAME_THROAT_X.min(CLIP_BARBS_X - spec.clip_fit_mm - 0.05);
    let clip_slot = [
        (rectangle(0.0, 0.0, 4.55, SOCKET_Y), bottom),
        (rectangle(0.0, 0.0, 4.55, SOCKET_Y), FRAME_TOP - 3.25),
        (
            rectangle(0.0, 0.0, frame_throat, SOCKET_Y),
            FRAME_TOP - 2.25,
        ),
        (rectangle(0.0, 0.0, frame_throat, SOCKET_Y), FRAME_TOP),
    ];
    let mut mesh = MeshBuilder::default();
    let mut bottom_holes = windows.clone();
    let mut top_holes = windows.clone();
    bottom_holes.push(clip_slot[0].0.clone());
    top_holes.push(clip_slot[3].0.clone());
    for center in bolts {
        let bore = circle(center, BOLT_RADIUS);
        let head = circle(center, HEAD_RADIUS);
        wall(&mut mesh, &bore, bottom, &bore, HARDWARE_Z, true);
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
        wall(&mut mesh, window, bottom, window, FRAME_TOP, true);
    }
    wall(&mut mesh, &locator, FRAME_TOP, &locator, 2.0, false);
    cap(&mut mesh, &locator, &[], 2.0, false)?;
    top_holes.extend(pads);
    top_holes.push(locator);
    cavity(&mut mesh, &clip_slot);
    wall(&mut mesh, &outer, bottom, &outer, FRAME_TOP, false);
    cap(&mut mesh, &outer, &bottom_holes, bottom, true)?;
    cap(&mut mesh, &outer, &top_holes, FRAME_TOP, false)?;
    let mut mesh = mesh.finish(format!("frame-{:02}", face.id));
    for point in &mut mesh.vertices {
        point[2] -= bottom;
    }
    let pose = Pose {
        origin: face.pose.to_globe([0.0, 0.0, bottom]),
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

pub(super) fn build_bracket(a: &Face, b: &Face, spec: &GlobePrototypeSpec) -> Result<(Mesh, Pose)> {
    let edge = FrameEdge::new(a, a.edge_with(b).expect("adjacent bracket faces"), spec);
    let top_a = frame_bottom(a, spec);
    let ends = [-10.0, 10.0].map(|offset| a.pose.to_globe(lift(edge.at(offset, 0.0), top_a)));
    let normal = unit(add(a.pose.z_axis, b.pose.z_axis));
    let print_z = unit(sub(ends[1], ends[0]));
    let pose = Pose::new(ends[0], normal, print_z);
    let reduction = BRACKET_DEPTH * a.depth_scale() / (dot(a.pose.origin, a.pose.z_axis) + top_a);
    let seam_offset = scale(add(ends[0], ends[1]), -0.5 * reduction);
    let mut combined = MeshBuilder::default();
    for (face, neighbor, ends) in [(a, b, ends), (b, a, [ends[1], ends[0]])] {
        let top_z = frame_bottom(face, spec);
        let edge = FrameEdge::new(
            face,
            face.edge_with(neighbor).expect("adjacent bracket faces"),
            spec,
        );
        let [start, end] = ends.map(|point| {
            let [x, y, _] = face.pose.to_local(point);
            [x, y]
        });
        let upper = [start, end, edge.at(10.0, 13.0), edge.at(-10.0, 13.0)];
        let lower = upper.map(|point| {
            let along = (point[0] - edge.midpoint[0]) * edge.along[0]
                + (point[1] - edge.midpoint[1]) * edge.along[1];
            std::array::from_fn(|i| point[i] - reduction * (point[i] - along * edge.along[i]))
        });
        let lower_z = top_z - BRACKET_DEPTH * face.depth_scale();
        let bores = edge.bolts().map(|center| circle(center, BOLT_RADIUS));
        let mut half = MeshBuilder::default();
        cap(&mut half, &upper, &bores, top_z, false)?;
        cap(&mut half, &lower, &bores, lower_z, true)?;
        for hole in &bores {
            wall(&mut half, hole, lower_z, hole, top_z, true);
        }
        for i in 1..4 {
            let next = (i + 1) % 4;
            half.quad(
                lift(lower[i], lower_z),
                lift(lower[next], lower_z),
                lift(upper[next], top_z),
                lift(upper[i], top_z),
                MATERIAL,
            );
        }
        // Use the same four global seam vertices in both halves before welding.
        let seam_vertices = (0..2)
            .flat_map(|i| {
                [
                    (lift(upper[i], top_z), ends[i]),
                    (lift(lower[i], lower_z), add(ends[i], seam_offset)),
                ]
            })
            .collect::<Vec<_>>();
        let half = half.finish("bracket-half");
        for triangle in half.triangles {
            let [p, q, r] = triangle.map(|index| {
                let local = half.vertices[index as usize];
                let global = seam_vertices
                    .iter()
                    .find(|(point, _)| *point == local)
                    .map_or_else(|| face.pose.to_globe(local), |(_, global)| *global);
                let mut point = pose.to_local(global);
                if point[2].abs() < 0.005 {
                    point[2] = 0.0;
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
