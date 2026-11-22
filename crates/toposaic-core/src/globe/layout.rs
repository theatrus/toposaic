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

    pub(super) fn to_globe(self, [x, y, z]: Vec3) -> Vec3 {
        add(
            self.origin,
            add(
                scale(self.x_axis, x),
                add(scale(self.y_axis, y), scale(self.z_axis, z)),
            ),
        )
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

pub(super) struct Face {
    pub(super) id: usize,
    pub(super) vertices: [usize; 3],
    pub(super) pose: Pose,
    pub(super) chord_radius: f32,
    pub(super) radius: f32,
    pub(super) triangle: [[f32; 2]; 3],
}

impl Face {
    pub(super) fn all(radius: f32, backing: f32) -> Vec<Self> {
        let vertices = VERTICES.map(|point| scale(unit(point), radius));
        FACES
            .iter()
            .enumerate()
            .map(|(id, &indices)| {
                let [a, b, c] = indices.map(|i| vertices[i]);
                let along = unit(sub(b, a));
                let normal = unit(cross(along, sub(c, a)));
                let chord_radius = dot(normal, a);
                let pose = Pose::new(scale(normal, chord_radius - backing), along, normal);
                let triangle = [a, b, c].map(|point| {
                    let [x, y, _] = pose.to_local(point);
                    [x, y]
                });
                Self {
                    id: id + 1,
                    vertices: indices,
                    pose,
                    chord_radius,
                    radius,
                    triangle,
                }
            })
            .collect()
    }

    pub(super) fn outline_at(&self, z: f32, inset: f32) -> [[f32; 2]; 3] {
        let plane_radius = dot(self.pose.origin, self.pose.z_axis) + z;
        // The faces are equilateral. A homothetic inset moves each edge by
        // the same distance because the origin is the triangle's incenter.
        let [a, b, _] = self.triangle;
        let inradius = ((a[0] - b[0]).hypot(a[1] - b[1])) / (2.0 * 3.0_f32.sqrt());
        let factor = plane_radius / self.chord_radius - inset / inradius;
        self.triangle.map(|point| point.map(|value| value * factor))
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
