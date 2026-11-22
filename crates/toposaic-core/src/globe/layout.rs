use std::collections::HashMap;

use serde::Serialize;

pub(super) type Vec3 = [f32; 3];
pub(super) type Loop = Vec<[f32; 2]>;

const VERTICES: [Vec3; 12] = [
    [-1.0, PHI, 0.0],
    [1.0, PHI, 0.0],
    [-1.0, -PHI, 0.0],
    [1.0, -PHI, 0.0],
    [0.0, -1.0, PHI],
    [0.0, 1.0, PHI],
    [0.0, -1.0, -PHI],
    [0.0, 1.0, -PHI],
    [PHI, 0.0, -1.0],
    [PHI, 0.0, 1.0],
    [-PHI, 0.0, -1.0],
    [-PHI, 0.0, 1.0],
];
const PHI: f32 = 1.618_034;
const FACES: [[usize; 3]; 20] = [
    [0, 11, 5],
    [0, 5, 1],
    [0, 1, 7],
    [0, 7, 10],
    [0, 10, 11],
    [1, 5, 9],
    [5, 11, 4],
    [11, 10, 2],
    [10, 7, 6],
    [7, 1, 8],
    [3, 9, 4],
    [3, 4, 2],
    [3, 2, 6],
    [3, 6, 8],
    [3, 8, 9],
    [4, 9, 5],
    [2, 4, 11],
    [6, 2, 10],
    [8, 6, 7],
    [9, 8, 1],
];

pub(super) fn add(a: Vec3, b: Vec3) -> Vec3 {
    std::array::from_fn(|i| a[i] + b[i])
}

pub(super) fn sub(a: Vec3, b: Vec3) -> Vec3 {
    std::array::from_fn(|i| a[i] - b[i])
}

pub(super) fn scale(a: Vec3, factor: f32) -> Vec3 {
    a.map(|value| value * factor)
}

pub(super) fn dot(a: Vec3, b: Vec3) -> f32 {
    a.into_iter().zip(b).map(|(a, b)| a * b).sum()
}

pub(super) fn cross(a: Vec3, b: Vec3) -> Vec3 {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}

pub(super) fn unit(a: Vec3) -> Vec3 {
    scale(a, dot(a, a).sqrt().recip())
}

#[derive(Clone, Copy, Serialize)]
pub(super) struct Pose {
    pub(super) origin: Vec3,
    pub(super) x_axis: Vec3,
    pub(super) y_axis: Vec3,
    pub(super) z_axis: Vec3,
}

impl Pose {
    pub(super) fn new(origin: Vec3, x_axis: Vec3, z_axis: Vec3) -> Self {
        Self {
            origin,
            x_axis,
            y_axis: cross(z_axis, x_axis),
            z_axis,
        }
    }

    pub(super) fn rotate(self, [x, y, z]: Vec3) -> Vec3 {
        add(
            scale(self.x_axis, x),
            add(scale(self.y_axis, y), scale(self.z_axis, z)),
        )
    }

    pub(super) fn to_globe(self, point: Vec3) -> Vec3 {
        add(self.origin, self.rotate(point))
    }

    pub(super) fn to_local(self, point: Vec3) -> Vec3 {
        let delta = sub(point, self.origin);
        [
            dot(delta, self.x_axis),
            dot(delta, self.y_axis),
            dot(delta, self.z_axis),
        ]
    }
}

pub(super) const ICOSAHEDRON_CHORD_RATIO: f32 = 0.794_654_5;

pub(super) struct Face {
    pub(super) id: usize,
    pub(super) vertices: [usize; 3],
    pub(super) pose: Pose,
    pub(super) chord_radius: f32,
    pub(super) radius: f32,
    pub(super) triangle: [[f32; 2]; 3],
    incenter: [f32; 2],
    inradius: f32,
}

impl Face {
    pub(super) fn all(radius: f32, backing: f32, frequency: u8) -> Vec<Self> {
        let corners = VERTICES.map(|point| scale(unit(point), radius));
        let mut vertices = Vec::new();
        let mut vertex_ids = HashMap::new();
        let divisions = usize::from(frequency);
        let faces = FACES
            .iter()
            .flat_map(|&ids| {
                let mut point = |i, j| {
                    let weights = [divisions - i - j, i, j];
                    let mut key = [(0, 0); 3];
                    for (index, (id, weight)) in ids
                        .into_iter()
                        .zip(weights)
                        .filter(|(_, weight)| *weight != 0)
                        .enumerate()
                    {
                        key[index] = (id + 1, weight);
                    }
                    key.sort_unstable();
                    *vertex_ids.entry(key).or_insert_with(|| {
                        let sum = key
                            .into_iter()
                            .filter(|&(id, _)| id > 0)
                            .fold([0.0; 3], |sum, (id, weight)| {
                                add(sum, scale(corners[id - 1], weight as f32))
                            });
                        vertices.push(scale(unit(sum), radius));
                        vertices.len() - 1
                    })
                };
                let mut triangles = Vec::with_capacity(divisions * divisions);
                for i in 0..divisions {
                    for j in 0..divisions - i {
                        triangles.push([point(i, j), point(i + 1, j), point(i, j + 1)]);
                        if i + j + 1 < divisions {
                            triangles.push([point(i + 1, j), point(i + 1, j + 1), point(i, j + 1)]);
                        }
                    }
                }
                triangles
            })
            .collect::<Vec<_>>();
        faces
            .iter()
            .enumerate()
            .map(|(id, &indices)| {
                let [a, b, c] = indices.map(|i| vertices[i]);
                let along = unit(sub(b, a));
                let normal = unit(cross(along, sub(c, a)));
                let chord_radius = dot(normal, a);
                let pose = Pose::new(
                    scale(
                        normal,
                        chord_radius - backing * chord_radius / (radius * ICOSAHEDRON_CHORD_RATIO),
                    ),
                    along,
                    normal,
                );
                let triangle = [a, b, c].map(|point| {
                    let [x, y, _] = pose.to_local(point);
                    [x, y]
                });
                let opposite = [sub(c, b), sub(a, c), sub(b, a)].map(|edge| dot(edge, edge).sqrt());
                let perimeter: f32 = opposite.iter().sum();
                let incenter = std::array::from_fn(|axis| {
                    triangle
                        .iter()
                        .zip(opposite)
                        .map(|(p, length)| p[axis] * length / perimeter)
                        .sum()
                });
                let double_area = dot(cross(sub(b, a), sub(c, a)), normal);
                Self {
                    id: id + 1,
                    vertices: indices,
                    pose,
                    chord_radius,
                    radius,
                    triangle,
                    incenter,
                    inradius: double_area / perimeter,
                }
            })
            .collect()
    }

    pub(super) fn outline_at(&self, z: f32, inset: f32) -> [[f32; 2]; 3] {
        let plane_radius = dot(self.pose.origin, self.pose.z_axis) + z;
        self.triangle.map(|point| {
            std::array::from_fn(|axis| {
                point[axis] * plane_radius / self.chord_radius
                    + (self.incenter[axis] - point[axis]) * inset / self.inradius
            })
        })
    }

    pub(super) fn depth_scale(&self) -> f32 {
        self.chord_radius / (self.radius * ICOSAHEDRON_CHORD_RATIO)
    }

    pub(super) fn inradius(&self) -> f32 {
        self.inradius
    }

    pub(super) fn edge_width(&self) -> f32 {
        (0..3)
            .map(|i| {
                let [a, b] = [self.triangle[i], self.triangle[(i + 1) % 3]];
                (a[0] - b[0]).hypot(a[1] - b[1])
            })
            .fold(0.0, f32::max)
    }

    pub(super) fn adjacent(&self, other: &Self) -> bool {
        self.edge_with(other).is_some()
    }

    pub(super) fn edge_with(&self, other: &Self) -> Option<usize> {
        if self.id == other.id {
            return None;
        }
        self.vertices.iter().enumerate().position(|(i, vertex)| {
            other.vertices.contains(vertex) && other.vertices.contains(&self.vertices[(i + 1) % 3])
        })
    }
}
