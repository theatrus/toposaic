use std::collections::BTreeMap;

use anyhow::{Result, ensure};
use serde::{Deserialize, Serialize};

use super::GlobePrototypeSpec;
use super::layout::{Face, Loop, Pose, Vec3, add, dot, scale, sub, unit};
use crate::mesh::Mesh;

const LIMIT: usize = 24;
const MAX_OFFSET_MM: f32 = 14.0;
pub(super) const PEG_DEPTH: f32 = 1.8;
const CLEARANCE: f32 = 0.20;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GlobeLandmarkShape {
    Tower,
    Pyramid,
    CityMarker,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GlobeLandmark {
    pub id: u32,
    pub name: String,
    pub shape: GlobeLandmarkShape,
    pub latitude: f64,
    pub longitude: f64,
    pub width_mm: f32,
    pub height_mm: f32,
}

impl GlobeLandmark {
    pub(crate) fn validate_list(landmarks: &[Self]) -> Result<()> {
        ensure!(
            landmarks.len() <= LIMIT,
            "a globe can have up to {LIMIT} landmarks"
        );
        let mut ids = std::collections::BTreeSet::new();
        for landmark in landmarks {
            ensure!(
                landmark.id > 0 && ids.insert(landmark.id),
                "landmark numbers must be unique and positive"
            );
            ensure!(
                !landmark.name.trim().is_empty()
                    && landmark.name.chars().count() <= 60
                    && !landmark.name.chars().any(char::is_control),
                "landmark names must have 1 to 60 characters and fit on one line"
            );
            ensure!(
                (-90.0..=90.0).contains(&landmark.latitude)
                    && (-180.0..=180.0).contains(&landmark.longitude),
                "landmark coordinates are out of range"
            );
            ensure!(
                (5.0..=14.0).contains(&landmark.width_mm)
                    && (4.0..=20.0).contains(&landmark.height_mm),
                "landmark width must be 5 to 14 mm and height 4 to 20 mm"
            );
        }
        Ok(())
    }

    fn direction(&self) -> Vec3 {
        let (north, horizontal) = self.latitude.to_radians().sin_cos();
        let (east, prime) = self.longitude.to_radians().sin_cos();
        [
            (horizontal * prime) as f32,
            (horizontal * east) as f32,
            north as f32,
        ]
    }

    pub(super) fn peg_half(&self) -> f32 {
        (0.20 * self.width_mm).clamp(1.3, 2.5)
    }
}

#[derive(Clone, Serialize)]
pub(super) struct Placement {
    #[serde(flatten)]
    pub(super) landmark: GlobeLandmark,
    pub(super) tile: usize,
    pub(super) seat: Option<Seat>,
    pub(super) requested_position: Vec3,
    pub(super) insert_position: Option<Vec3>,
}

#[derive(Clone, Serialize)]
pub(super) struct Seat {
    pub(super) latitude: f64,
    pub(super) longitude: f64,
    pub(super) offset_mm: f32,
    #[serde(skip)]
    pub(super) uv: [f32; 2],
    #[serde(skip)]
    pub(super) rim: Loop,
}

pub(super) struct Insert {
    pub(super) mesh: Mesh,
    pub(super) pose: Pose,
    pub(super) location: Vec3,
}

pub(super) fn keyed_ring([x, y]: [f32; 2], half: f32) -> Loop {
    [
        [-1.0, -1.0],
        [1.0, -1.0],
        [1.0, 0.35],
        [0.35, 1.0],
        [-1.0, 1.0],
    ]
    .map(|[a, b]| [x + a * half, y + b * half])
    .to_vec()
}

pub(super) fn socket_half(landmark: &GlobeLandmark) -> f32 {
    landmark.peg_half() + CLEARANCE
}

pub(super) fn uv_for(triangle: [[f32; 2]; 3], point: [f32; 2]) -> [f32; 2] {
    let [[ax, ay], [bx, by], [cx, cy]] = triangle;
    let [x, y] = [point[0] - ax, point[1] - ay];
    let [a, b, c, d] = [bx - ax, by - ay, cx - ax, cy - ay];
    let determinant = a * d - b * c;
    [(x * d - y * c) / determinant, (a * y - b * x) / determinant]
}

fn interior([u, v]: [f32; 2]) -> f32 {
    u.min(v).min(1.0 - u - v)
}

pub(super) fn place(
    landmarks: &[GlobeLandmark],
    faces: &[Face],
    spec: &GlobePrototypeSpec,
) -> Vec<Placement> {
    let mut ordered = landmarks.iter().collect::<Vec<_>>();
    ordered.sort_unstable_by_key(|landmark| landmark.id);
    let mut occupied = BTreeMap::<usize, Vec<([f32; 2], f32)>>::new();
    let mut offsets = (-(MAX_OFFSET_MM as i32)..=MAX_OFFSET_MM as i32)
        .flat_map(|x| {
            (-(MAX_OFFSET_MM as i32)..=MAX_OFFSET_MM as i32).map(move |y| (x * x + y * y, x, y))
        })
        .filter(|&(d, _, _)| d as f32 <= MAX_OFFSET_MM * MAX_OFFSET_MM)
        .collect::<Vec<_>>();
    offsets.sort_unstable();
    ordered
        .into_iter()
        .map(|landmark| {
            let direction = landmark.direction();
            let (face, requested, _) = faces
                .iter()
                .filter_map(|face| {
                    let axis = dot(direction, face.pose.z_axis);
                    if axis <= 0.0 {
                        return None;
                    }
                    let projected = face.pose.to_local(scale(
                        direction,
                        dot(face.pose.origin, face.pose.z_axis) / axis,
                    ));
                    let point = [projected[0], projected[1]];
                    let margin = interior(uv_for(face.outline_at(0.0, 0.0), point));
                    Some((face, point, margin))
                })
                .max_by(|(a, _, am), (b, _, bm)| am.total_cmp(bm).then_with(|| b.id.cmp(&a.id)))
                .expect("a globe has outward faces");
            let trim = face.outline_at(0.0, spec.seam_gap_mm / 2.0);
            let visited = occupied.entry(face.id).or_default();
            let half = landmark.width_mm * 0.50 + 2.2;
            let keepout = socket_half(landmark) + 0.9;
            let seat = offsets.iter().find_map(|&(_, dx, dy)| {
                let center = [requested[0] + dx as f32, requested[1] + dy as f32];
                // The recess and its registration terrace have to clear the
                // back-side clip, locator, and tile-screw insert.
                let insert = spec.insert.hole_diameter_mm / 2.0 + 0.8;
                let screw = super::parts::tile_screw_center(spec);
                if (center[0].abs() < 4.6 + keepout && center[1].abs() < 2.6 + keepout)
                    || (center[0].abs() < 2.7 + keepout && (center[1] - 8.0).abs() < 2.3 + keepout)
                    || (spec.tile_screws
                        && (center[0] - screw[0]).abs() < insert + keepout
                        && (center[1] - screw[1]).abs() < insert + keepout)
                    || visited.iter().any(|&(other, radius)| {
                        (center[0] - other[0])
                            .abs()
                            .max((center[1] - other[1]).abs())
                            < half + radius + 0.4
                    })
                {
                    return None;
                }
                let rim = keyed_ring(center, half)
                    .into_iter()
                    .map(|p| uv_for(trim, p))
                    .collect::<Loop>();
                if rim.iter().any(|&p| interior(p) <= 0.001) {
                    return None;
                }
                let rim = keyed_ring(center, keepout + 0.75)
                    .into_iter()
                    .map(|p| uv_for(trim, p))
                    .collect();
                visited.push((center, half));
                let placed = unit(face.pose.to_globe([center[0], center[1], 0.0]));
                let distance = norm(sub(direction, placed)) * face.radius;
                Some(Seat {
                    latitude: f64::from(placed[2]).clamp(-1.0, 1.0).asin().to_degrees(),
                    longitude: f64::from(placed[1])
                        .atan2(f64::from(placed[0]))
                        .to_degrees(),
                    offset_mm: distance,
                    uv: uv_for(trim, center),
                    rim,
                })
            });
            let nominal_radius = if spec.exterior == super::GlobeExterior::Rounded {
                face.radius
            } else {
                face.chord_radius / dot(direction, face.pose.z_axis)
            };
            Placement {
                landmark: landmark.clone(),
                tile: face.id,
                seat,
                requested_position: scale(direction, nominal_radius),
                insert_position: None,
            }
        })
        .collect()
}

fn norm(a: Vec3) -> f32 {
    dot(a, a).sqrt()
}

pub(super) fn insert_pose(face: &Face, center: [f32; 2], seat_z: f32) -> Pose {
    let origin = face.pose.to_globe([center[0], center[1], seat_z]);
    Pose {
        origin: add(origin, scale(face.pose.z_axis, -PEG_DEPTH)),
        ..face.pose
    }
}
