use anyhow::{Result, bail};
use geo::{Area, Contains, Coord, LineString, Point, Polygon};
use spade::{Point2, Triangulation};

use crate::mesh::{MeshBuilder, triangulate_constraints};
use crate::spec::{SurfaceClass, WallMountSpec, WallMountStyle};

const CIRCLE_SAMPLES: usize = 32;
const ANGLED_PIN_DEGREES: f32 = 25.0;
const FRENCH_CLEAT_DEGREES: f32 = 35.0;

struct MountCavity {
    opening: Vec<[f32; 2]>,
    ceiling: Vec<[f32; 2]>,
}

/// Builds the flat back of one solid with blind mounting cuts.
///
/// The caller keeps its existing outer walls and omits only its old flat
/// bottom. This builder supplies the remaining downward face, each cut's
/// ceiling, and the walls between them. All cut edges therefore join by
/// coordinates at the final export weld; no 3D boolean or overlapping solid
/// reaches the slicer.
pub(crate) fn mount_bottom(outline: &[[f32; 2]], mount: &WallMountSpec) -> Result<MeshBuilder> {
    let outline_polygon = polygon(outline);
    mount_bottom_polygons(&[outline_polygon], mount)
}

/// The split tray keeps its floor and rim as separate bottom polygons because
/// their shared frame adds exact vertices to the outer wall. Retaining those
/// splits stops a long bottom edge from passing through a wall vertex as a
/// T-junction. The mount cavity itself still gets built once.
pub(crate) fn mount_bottom_polygons(
    base_polygons: &[Polygon<f64>],
    mount: &WallMountSpec,
) -> Result<MeshBuilder> {
    let (mount_region_index, mount_region) = base_polygons
        .iter()
        .enumerate()
        .max_by(|left, right| left.1.unsigned_area().total_cmp(&right.1.unsigned_area()))
        .ok_or_else(|| anyhow::anyhow!("wall-mount feature needs a non-empty back face"))?;
    let mount_outline = mount_region
        .exterior()
        .0
        .iter()
        .take(mount_region.exterior().0.len().saturating_sub(1))
        .map(|coordinate| [coordinate.x as f32, coordinate.y as f32])
        .collect::<Vec<_>>();
    let cavities = cavities_for_outline(&mount_outline, mount_region, mount)?;
    let bottom = base_polygons
        .iter()
        .enumerate()
        .map(|(index, base)| {
            if index != mount_region_index {
                return base.clone();
            }
            let mut holes = base.interiors().to_vec();
            holes.extend(cavities.iter().map(|cavity| ring(&cavity.opening)));
            Polygon::new(base.exterior().clone(), holes)
        })
        .collect::<Vec<_>>();

    let mut mesh = MeshBuilder::default();
    add_horizontal_polygons(&mut mesh, &bottom, 0.0, SurfaceClass::Rock, true)?;
    for cavity in cavities {
        add_horizontal_polygons(
            &mut mesh,
            &[polygon(&cavity.ceiling)],
            mount.depth_mm,
            SurfaceClass::Rock,
            true,
        )?;
        for index in 0..cavity.opening.len() {
            let next = (index + 1) % cavity.opening.len();
            let opening_a = cavity.opening[index];
            let opening_b = cavity.opening[next];
            let ceiling_a = cavity.ceiling[index];
            let ceiling_b = cavity.ceiling[next];
            // The opening ring runs counter-clockwise. Reverse it here so
            // the wall normals face into the empty socket, away from the
            // solid material.
            mesh.quad(
                [opening_b[0], opening_b[1], 0.0],
                [opening_a[0], opening_a[1], 0.0],
                [ceiling_a[0], ceiling_a[1], mount.depth_mm],
                [ceiling_b[0], ceiling_b[1], mount.depth_mm],
                SurfaceClass::Rock,
            );
        }
    }
    Ok(mesh)
}

fn cavities_for_outline(
    outline: &[[f32; 2]],
    outline_polygon: &Polygon<f64>,
    mount: &WallMountSpec,
) -> Result<Vec<MountCavity>> {
    let [minimum_x, minimum_y, maximum_x, maximum_y] = bounds(outline);
    let width = maximum_x - minimum_x;
    let height = maximum_y - minimum_y;
    let center_y = minimum_y + height * 0.62;
    let shift = match mount.style {
        WallMountStyle::AngledPin => mount.depth_mm * ANGLED_PIN_DEGREES.to_radians().tan(),
        WallMountStyle::FrenchCleat => mount.depth_mm * FRENCH_CLEAT_DEGREES.to_radians().tan(),
        WallMountStyle::None | WallMountStyle::StraightPin => 0.0,
    };

    let cavities = match mount.style {
        WallMountStyle::None => Vec::new(),
        WallMountStyle::StraightPin | WallMountStyle::AngledPin => {
            let radius = mount.pin_diameter_mm * 0.5;
            let centers = if width >= 60.0 {
                vec![minimum_x + width * 0.33, minimum_x + width * 0.67]
            } else {
                vec![minimum_x + width * 0.5]
            };
            centers
                .into_iter()
                .map(|center_x| MountCavity {
                    opening: circle([center_x, center_y], radius),
                    ceiling: circle([center_x, center_y + shift], radius),
                })
                .collect()
        }
        WallMountStyle::FrenchCleat => {
            let half_width = (width * 0.32).min(70.0);
            let half_height = mount.pin_diameter_mm * 0.5;
            vec![MountCavity {
                opening: rectangle(minimum_x + width * 0.5, center_y, half_width, half_height),
                ceiling: rectangle(
                    minimum_x + width * 0.5,
                    center_y + shift,
                    half_width,
                    half_height,
                ),
            }]
        }
    };

    for cavity in &cavities {
        if cavity.opening.iter().chain(&cavity.ceiling).any(|point| {
            !outline_polygon.contains(&Point::new(f64::from(point[0]), f64::from(point[1])))
        }) {
            bail!(
                "wall-mount feature does not fit this part; reduce the pin size or use fewer pieces"
            );
        }
    }
    Ok(cavities)
}

fn circle(center: [f32; 2], radius: f32) -> Vec<[f32; 2]> {
    (0..CIRCLE_SAMPLES)
        .map(|index| {
            let angle = std::f32::consts::TAU * index as f32 / CIRCLE_SAMPLES as f32;
            [
                center[0] + radius * angle.cos(),
                center[1] + radius * angle.sin(),
            ]
        })
        .collect()
}

fn rectangle(center_x: f32, center_y: f32, half_width: f32, half_height: f32) -> Vec<[f32; 2]> {
    vec![
        [center_x - half_width, center_y - half_height],
        [center_x + half_width, center_y - half_height],
        [center_x + half_width, center_y + half_height],
        [center_x - half_width, center_y + half_height],
    ]
}

fn bounds(outline: &[[f32; 2]]) -> [f32; 4] {
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

fn polygon(points: &[[f32; 2]]) -> Polygon<f64> {
    Polygon::new(ring(points), vec![])
}

fn ring(points: &[[f32; 2]]) -> LineString<f64> {
    let mut coordinates = points
        .iter()
        .map(|point| Coord {
            x: f64::from(point[0]),
            y: f64::from(point[1]),
        })
        .collect::<Vec<_>>();
    if let Some(first) = coordinates.first().copied() {
        coordinates.push(first);
    }
    LineString::new(coordinates)
}

fn add_horizontal_polygons(
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
            triangulate_constraints(points, constraints, "triangulate wall-mount cut")?;
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
