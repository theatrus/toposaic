use anyhow::{Result, bail};
use geo::{
    Area, BooleanOps, Contains, ConvexHull, LineString, MultiPoint, MultiPolygon, Point, Polygon,
};

use crate::mesh::{Mesh, MeshBuilder, weld_export_mesh};
use crate::planar_mesh::{
    add_horizontal_polygons, closed_ring as ring, outline_bounds as bounds,
    polygon_from_outline as polygon,
};
use crate::spec::{
    GenerationSpec, PuzzleRetentionSpec, SurfaceClass, WallMountSpec, WallMountStyle,
};

const CIRCLE_SAMPLES: usize = 32;
const ANGLED_PIN_DEGREES: f32 = 25.0;
const FRENCH_CLEAT_DEGREES: f32 = 35.0;
const FRENCH_CLEAT_MIN_SLIDE_MM: f32 = 2.0;
const WALL_PLATE_MARGIN_MM: f32 = 1.2;
const WALL_PLATE_SCREW_GAP_MM: f32 = 0.4;
const ALIGNMENT_FRAME_BAND_MM: f32 = 1.6;
const ALIGNMENT_FRAME_THICKNESS_MM: f32 = 1.2;
const ALIGNMENT_RAIL_HALF_WIDTH_MM: f32 = 0.8;

#[derive(Clone)]
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
pub(crate) fn mount_bottom(
    outline: &[[f32; 2]],
    mount: &WallMountSpec,
    mount_frame: [f32; 4],
) -> Result<MeshBuilder> {
    let outline_polygon = polygon(outline);
    mount_bottom_polygons(&[outline_polygon], mount, mount_frame)
}

pub(crate) fn validate_wall_mount_frame(
    mount: &WallMountSpec,
    width: f32,
    height: f32,
) -> Result<()> {
    let frame = [0.0, 0.0, width, height];
    let frame_polygon = polygon(&rectangle(
        width * 0.5,
        height * 0.5,
        width * 0.5,
        height * 0.5,
    ));
    let pocket = polygon(&wall_plate_pocket(mount, frame));
    let screw_head_reliefs = screw_head_sweep_polygons(mount, frame);
    let fit_error = || {
        anyhow::anyhow!(
            "wall mount does not fit the full terrain tile or display base; reduce its width, height, travel, or screw-hole size"
        )
    };
    let receivers = cavities_for_frame(mount, frame).map_err(|_| fit_error())?;
    if !frame_polygon.contains(&pocket)
        || receivers.iter().any(|receiver| {
            !frame_polygon.contains(&polygon(&receiver.opening))
                || !frame_polygon.contains(&polygon(&receiver.ceiling))
        })
        || screw_head_reliefs
            .iter()
            .any(|relief| !frame_polygon.contains(relief))
    {
        return Err(fit_error());
    }
    Ok(())
}

/// Cuts one full-model mount through a single puzzle-piece outline.
///
/// A jigsaw split is a later XY partition of the mounted terrain. The plate
/// pocket and receiver therefore stay in full-model coordinates, while each
/// piece gets only the slice that crosses its outline. Layered lower walls
/// leave shared piece edges open wherever the wall hardware must pass.
pub(crate) fn mount_bottom_across_outline(
    outline: &[[f32; 2]],
    mount: &WallMountSpec,
    mount_frame: [f32; 4],
) -> Result<MeshBuilder> {
    let base = polygon(outline);
    let pocket = polygon(&wall_plate_pocket(mount, mount_frame));
    let receiver_sweeps = receiver_sweep_polygons(mount, mount_frame)?;
    let screw_head_reliefs = screw_head_sweep_polygons(mount, mount_frame);
    let cavity_union = receiver_sweeps.iter().chain(&screw_head_reliefs).fold(
        MultiPolygon(Vec::new()),
        |union, cavity| {
            if union.0.is_empty() {
                MultiPolygon(vec![cavity.clone()])
            } else {
                union.union(cavity)
            }
        },
    );

    let bottom = stabilize_mount_polygons(base.difference(&pocket), &base);
    let pocket_floor =
        stabilize_mount_polygons(base.intersection(&pocket).difference(&cavity_union), &base);
    let upper_layer = stabilize_mount_polygons(base.difference(&cavity_union), &base);
    let cavity_ceiling = stabilize_mount_polygons(base.intersection(&cavity_union), &base);

    let pocket_depth = mount.pocket_depth_mm();
    let ceiling = mount.embedded_depth_mm();
    let mut mesh = MeshBuilder::default();
    add_horizontal_polygons(&mut mesh, &bottom.0, 0.0, SurfaceClass::Rock, true)?;
    add_horizontal_polygons(
        &mut mesh,
        &pocket_floor.0,
        pocket_depth,
        SurfaceClass::Rock,
        true,
    )?;
    add_horizontal_polygons(
        &mut mesh,
        &cavity_ceiling.0,
        ceiling,
        SurfaceClass::Rock,
        true,
    )?;
    add_polygon_walls(&mut mesh, &bottom.0, 0.0, pocket_depth);
    add_polygon_walls(&mut mesh, &upper_layer.0, pocket_depth, ceiling);
    Ok(mesh)
}

/// Adds exact mount-boundary crossings to a jigsaw outline. The terrain side
/// wall and each clipped mount layer then share the same seam vertices instead
/// of meeting at a T-junction.
pub(crate) fn split_outline_at_mount(
    outline: &[[f32; 2]],
    mount: &WallMountSpec,
    mount_frame: [f32; 4],
) -> Result<Vec<[f32; 2]>> {
    let mut cut_rings = vec![wall_plate_pocket(mount, mount_frame)];
    cut_rings.extend(
        receiver_sweep_polygons(mount, mount_frame)?
            .into_iter()
            .map(|receiver| {
                receiver
                    .exterior()
                    .0
                    .iter()
                    .take(receiver.exterior().0.len().saturating_sub(1))
                    .map(|point| [point.x as f32, point.y as f32])
                    .collect()
            }),
    );
    cut_rings.extend(
        screw_head_sweep_polygons(mount, mount_frame)
            .into_iter()
            .map(|relief| {
                relief
                    .exterior()
                    .0
                    .iter()
                    .take(relief.exterior().0.len().saturating_sub(1))
                    .map(|point| [point.x as f32, point.y as f32])
                    .collect()
            }),
    );

    let mut split = Vec::with_capacity(outline.len() + cut_rings.len() * 2);
    for (start, end) in outline.iter().zip(outline.iter().cycle().skip(1)) {
        let mut cuts = vec![0.0_f32];
        for ring in &cut_rings {
            for (cut_start, cut_end) in ring.iter().zip(ring.iter().cycle().skip(1)) {
                if let Some(t) = segment_crossing(*start, *end, *cut_start, *cut_end)
                    && t > 0.000_01
                    && t < 0.999_99
                {
                    cuts.push(t);
                }
            }
        }
        cuts.sort_by(f32::total_cmp);
        cuts.dedup_by(|left, right| (*left - *right).abs() < 0.000_01);
        split.extend(cuts.into_iter().map(|t| {
            [
                start[0] + (end[0] - start[0]) * t,
                start[1] + (end[1] - start[1]) * t,
            ]
        }));
    }
    Ok(split)
}

fn segment_crossing(
    start: [f32; 2],
    end: [f32; 2],
    cut_start: [f32; 2],
    cut_end: [f32; 2],
) -> Option<f32> {
    let segment = [end[0] - start[0], end[1] - start[1]];
    let cut = [cut_end[0] - cut_start[0], cut_end[1] - cut_start[1]];
    let offset = [cut_start[0] - start[0], cut_start[1] - start[1]];
    let denominator = segment[0] * cut[1] - segment[1] * cut[0];
    if denominator.abs() < 0.000_001 {
        return None;
    }
    let t = (offset[0] * cut[1] - offset[1] * cut[0]) / denominator;
    let u = (offset[0] * segment[1] - offset[1] * segment[0]) / denominator;
    ((0.0..=1.0).contains(&t) && (0.0..=1.0).contains(&u)).then_some(t)
}

/// The split tray keeps its floor and rim as separate bottom polygons because
/// their shared frame adds exact vertices to the outer wall. Retaining those
/// splits stops a long bottom edge from passing through a wall vertex as a
/// T-junction. The mount cavity itself still gets built once.
pub(crate) fn mount_bottom_polygons(
    base_polygons: &[Polygon<f64>],
    mount: &WallMountSpec,
    mount_frame: [f32; 4],
) -> Result<MeshBuilder> {
    let [center_x, center_y] = wall_mount_center(mount, mount_frame);
    let mount_center = Point::new(f64::from(center_x), f64::from(center_y));
    let (mount_region_index, mount_region) = base_polygons
        .iter()
        .enumerate()
        .filter(|(_, polygon)| polygon.contains(&mount_center))
        .max_by(|left, right| left.1.unsigned_area().total_cmp(&right.1.unsigned_area()))
        .ok_or_else(|| anyhow::anyhow!("wall-mount feature needs a back face below its center"))?;
    let cavities = cavities_for_outline(mount_region, mount, mount_frame)?;
    if mount.style != WallMountStyle::None {
        return bottom_with_wall_plate_pocket(
            base_polygons,
            mount_region_index,
            mount_region,
            &cavities,
            mount,
            mount_frame,
        );
    }
    bottom_with_cavities(
        base_polygons,
        mount_region_index,
        &cavities,
        mount.engagement_depth_mm(),
    )
}

pub(crate) fn retention_bottom(
    outline: &[[f32; 2]],
    centers: &[[f32; 2]],
    retention: &PuzzleRetentionSpec,
) -> Result<MeshBuilder> {
    let outline_polygon = polygon(outline);
    let radius = retention.socket_diameter_mm() * 0.5;
    let cavities = centers
        .iter()
        .map(|center| MountCavity {
            opening: circle(*center, radius),
            ceiling: circle(*center, radius),
        })
        .collect::<Vec<_>>();
    if cavities.iter().any(|cavity| {
        cavity
            .opening
            .iter()
            .any(|point| !outline_polygon.contains(&Point::new(point[0].into(), point[1].into())))
    }) {
        bail!(
            "tray-retention socket does not fit this piece; reduce the pin size or use fewer pieces"
        );
    }
    bottom_with_cavities(
        &[outline_polygon],
        0,
        &cavities,
        retention.socket_depth_mm(),
    )
}

fn bottom_with_cavities(
    base_polygons: &[Polygon<f64>],
    mount_region_index: usize,
    cavities: &[MountCavity],
    depth_mm: f32,
) -> Result<MeshBuilder> {
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
            depth_mm,
            SurfaceClass::Rock,
            true,
        )?;
        add_cavity_wall(&mut mesh, &cavity.opening, 0.0, &cavity.ceiling, depth_mm);
    }
    Ok(mesh)
}

fn bottom_with_wall_plate_pocket(
    base_polygons: &[Polygon<f64>],
    mount_region_index: usize,
    mount_region: &Polygon<f64>,
    receivers: &[MountCavity],
    mount: &WallMountSpec,
    mount_frame: [f32; 4],
) -> Result<MeshBuilder> {
    let pocket = wall_plate_pocket(mount, mount_frame);
    let screw_head_reliefs = screw_head_sweep_polygons(mount, mount_frame);
    if !mount_region.contains(&polygon(&pocket))
        || receivers.iter().any(|receiver| {
            !mount_region.contains(&polygon(&receiver.opening))
                || !mount_region.contains(&polygon(&receiver.ceiling))
        })
        || screw_head_reliefs
            .iter()
            .any(|relief| !mount_region.contains(relief))
    {
        bail!(
            "wall-mount plate does not fit this part; reduce the mount, pocket, or screw size, or use fewer pieces"
        );
    }

    let bottom = base_polygons
        .iter()
        .enumerate()
        .map(|(index, base)| {
            if index != mount_region_index {
                return base.clone();
            }
            let mut holes = base.interiors().to_vec();
            holes.push(ring(&pocket));
            Polygon::new(base.exterior().clone(), holes)
        })
        .collect::<Vec<_>>();
    let pocket_floor = Polygon::new(
        ring(&pocket),
        receivers
            .iter()
            .map(|receiver| ring(&receiver.opening))
            .chain(
                screw_head_reliefs
                    .iter()
                    .map(|relief| relief.exterior().clone()),
            )
            .collect(),
    );
    let pocket_depth = mount.pocket_depth_mm();
    let receiver_ceiling = mount.embedded_depth_mm();
    let screw_head_ceiling = receiver_ceiling;

    let mut mesh = MeshBuilder::default();
    add_horizontal_polygons(&mut mesh, &bottom, 0.0, SurfaceClass::Rock, true)?;
    add_horizontal_polygons(
        &mut mesh,
        &[pocket_floor],
        pocket_depth,
        SurfaceClass::Rock,
        true,
    )?;
    add_cavity_wall(&mut mesh, &pocket, 0.0, &pocket, pocket_depth);
    for receiver in receivers {
        add_horizontal_polygons(
            &mut mesh,
            &[polygon(&receiver.ceiling)],
            receiver_ceiling,
            SurfaceClass::Rock,
            true,
        )?;
        add_cavity_wall(
            &mut mesh,
            &receiver.opening,
            pocket_depth,
            &receiver.ceiling,
            receiver_ceiling,
        );
    }
    for relief in &screw_head_reliefs {
        add_horizontal_polygons(
            &mut mesh,
            std::slice::from_ref(relief),
            screw_head_ceiling,
            SurfaceClass::Rock,
            true,
        )?;
        add_line_string_wall(
            &mut mesh,
            relief.exterior(),
            pocket_depth,
            screw_head_ceiling,
            true,
        );
    }
    Ok(mesh)
}

fn add_cavity_wall(
    mesh: &mut MeshBuilder,
    opening: &[[f32; 2]],
    opening_z: f32,
    ceiling: &[[f32; 2]],
    ceiling_z: f32,
) {
    for index in 0..opening.len() {
        let next = (index + 1) % opening.len();
        let opening_a = opening[index];
        let opening_b = opening[next];
        let ceiling_a = ceiling[index];
        let ceiling_b = ceiling[next];
        mesh.quad(
            [opening_b[0], opening_b[1], opening_z],
            [opening_a[0], opening_a[1], opening_z],
            [ceiling_a[0], ceiling_a[1], ceiling_z],
            [ceiling_b[0], ceiling_b[1], ceiling_z],
            SurfaceClass::Rock,
        );
    }
}

fn wall_plate_pocket(mount: &WallMountSpec, mount_frame: [f32; 4]) -> Vec<[f32; 2]> {
    let (_, features) = wall_hardware_features(mount);
    let feature_bounds = feature_bounds(&features);
    let (plate, _) = hardware_plate_and_screw_centers(mount, feature_bounds);
    let plate_bounds = bounds(&plate);
    let slide = wall_mount_slide(mount);
    let [center_x, center_y] = wall_mount_center(mount, mount_frame);
    // Sweep the plate's true footprint from its lower entry position to its
    // locked position. Angled features make the plate slightly asymmetric
    // around the receiver, so centering a size-only box on the receiver can
    // clip one edge even when its width and height look large enough.
    let minimum_x = center_x + plate_bounds[0] - mount.fit_clearance_mm;
    let maximum_x = center_x + plate_bounds[2] + mount.fit_clearance_mm;
    let minimum_y = center_y + plate_bounds[1] - slide - mount.fit_clearance_mm;
    let maximum_y = center_y + plate_bounds[3] + mount.fit_clearance_mm;
    rectangle(
        (minimum_x + maximum_x) * 0.5,
        (minimum_y + maximum_y) * 0.5,
        (maximum_x - minimum_x) * 0.5,
        (maximum_y - minimum_y) * 0.5,
    )
}

fn wall_mount_center(mount: &WallMountSpec, mount_frame: [f32; 4]) -> [f32; 2] {
    let [minimum_x, minimum_y, maximum_x, maximum_y] = mount_frame;
    let frame_center = [(minimum_x + maximum_x) * 0.5, (minimum_y + maximum_y) * 0.5];
    if mount.style == WallMountStyle::None {
        return frame_center;
    }
    let (_, features) = wall_hardware_features(mount);
    let (plate, _) = hardware_plate_and_screw_centers(mount, feature_bounds(&features));
    let plate_bounds = bounds(&plate);
    let plate_center = [
        (plate_bounds[0] + plate_bounds[2]) * 0.5,
        (plate_bounds[1] + plate_bounds[3]) * 0.5,
    ];
    [
        frame_center[0] - plate_center[0],
        frame_center[1] - plate_center[1] + wall_mount_slide(mount) * 0.5,
    ]
}

fn wall_mount_slide(mount: &WallMountSpec) -> f32 {
    let shift = match mount.style {
        WallMountStyle::AngledPin => {
            mount.engagement_depth_mm() * ANGLED_PIN_DEGREES.to_radians().tan()
        }
        WallMountStyle::FrenchCleat => {
            mount.engagement_depth_mm() * FRENCH_CLEAT_DEGREES.to_radians().tan()
        }
        WallMountStyle::None | WallMountStyle::StraightPin => 0.0,
    };
    match mount.style {
        WallMountStyle::AngledPin => shift + mount.fit_clearance_mm,
        WallMountStyle::FrenchCleat => FRENCH_CLEAT_MIN_SLIDE_MM
            .max(mount.pin_diameter_mm * 0.75)
            .max(shift + mount.fit_clearance_mm),
        WallMountStyle::None | WallMountStyle::StraightPin => 0.0,
    }
}

fn cavities_for_outline(
    outline_polygon: &Polygon<f64>,
    mount: &WallMountSpec,
    mount_frame: [f32; 4],
) -> Result<Vec<MountCavity>> {
    let [minimum_x, _, maximum_x, _] = mount_frame;
    let width = maximum_x - minimum_x;
    let [center_x, center_y] = wall_mount_center(mount, mount_frame);
    let shift = match mount.style {
        WallMountStyle::AngledPin => {
            mount.engagement_depth_mm() * ANGLED_PIN_DEGREES.to_radians().tan()
        }
        WallMountStyle::FrenchCleat => {
            mount.engagement_depth_mm() * FRENCH_CLEAT_DEGREES.to_radians().tan()
        }
        WallMountStyle::None | WallMountStyle::StraightPin => 0.0,
    };

    let cavities = match mount.style {
        WallMountStyle::None => Vec::new(),
        WallMountStyle::StraightPin | WallMountStyle::AngledPin => {
            let radius = mount.pin_diameter_mm * 0.5;
            let centers = if mount.pin_count == 2 {
                vec![
                    minimum_x + width * 0.5 - mount.pin_spacing_mm * 0.5,
                    minimum_x + width * 0.5 + mount.pin_spacing_mm * 0.5,
                ]
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
            let half_width = mount.cleat_width_mm * 0.5;
            let half_height = mount.pin_diameter_mm * 0.5;
            let slide = wall_mount_slide(mount);
            let box_half_height = half_height + slide * 0.5;
            vec![MountCavity {
                opening: rectangle(
                    center_x,
                    center_y - slide * 0.5,
                    half_width,
                    box_half_height,
                ),
                ceiling: rectangle(
                    center_x,
                    center_y + shift - slide * 0.5,
                    half_width,
                    box_half_height,
                ),
            }]
        }
    };

    for cavity in &cavities {
        if !outline_polygon.contains(&polygon(&cavity.opening))
            || !outline_polygon.contains(&polygon(&cavity.ceiling))
        {
            bail!(
                "wall-mount feature does not fit this part; reduce the pin size or use fewer pieces"
            );
        }
    }
    Ok(cavities)
}

fn cavities_for_frame(mount: &WallMountSpec, mount_frame: [f32; 4]) -> Result<Vec<MountCavity>> {
    let frame = polygon(&rectangle(
        (mount_frame[0] + mount_frame[2]) * 0.5,
        (mount_frame[1] + mount_frame[3]) * 0.5,
        (mount_frame[2] - mount_frame[0]) * 0.5,
        (mount_frame[3] - mount_frame[1]) * 0.5,
    ));
    cavities_for_outline(&frame, mount, mount_frame)
}

fn receiver_sweep_polygons(
    mount: &WallMountSpec,
    mount_frame: [f32; 4],
) -> Result<Vec<Polygon<f64>>> {
    Ok(cavities_for_frame(mount, mount_frame)?
        .into_iter()
        .map(|receiver| {
            let points = receiver
                .opening
                .into_iter()
                .chain(receiver.ceiling)
                .map(|point| Point::new(f64::from(point[0]), f64::from(point[1])))
                .collect::<Vec<_>>();
            MultiPoint::new(points).convex_hull()
        })
        .collect())
}

/// Sweeps the screw-head footprint through the same entry-to-lock travel as
/// the wall plate. This cuts only the local head relief, so wall offset and
/// the rest of the plate pocket keep their exact requested dimensions.
fn screw_head_sweep_polygons(mount: &WallMountSpec, mount_frame: [f32; 4]) -> Vec<Polygon<f64>> {
    if mount.screw_head_clearance_mm <= 0.000_01 {
        return Vec::new();
    }
    let (_, features) = wall_hardware_features(mount);
    let (_, screw_centers) = hardware_plate_and_screw_centers(mount, feature_bounds(&features));
    let [center_x, center_y] = wall_mount_center(mount, mount_frame);
    let slide = wall_mount_slide(mount);
    let radius = screw_head_radius(mount) + mount.fit_clearance_mm;
    screw_centers
        .into_iter()
        .map(|screw_center| {
            let locked = [center_x + screw_center[0], center_y + screw_center[1]];
            let entry = [locked[0], locked[1] - slide];
            let points = circle(locked, radius)
                .into_iter()
                .chain(circle(entry, radius))
                .map(|point| Point::new(f64::from(point[0]), f64::from(point[1])))
                .collect::<Vec<_>>();
            MultiPoint::new(points).convex_hull()
        })
        .collect()
}

fn stabilize_mount_polygons(polygons: MultiPolygon<f64>, base: &Polygon<f64>) -> MultiPolygon<f64> {
    let base_points = base
        .exterior()
        .0
        .iter()
        .take(base.exterior().0.len().saturating_sub(1))
        .map(|point| [point.x as f32, point.y as f32])
        .collect::<Vec<_>>();
    MultiPolygon(
        polygons
            .0
            .into_iter()
            .map(|polygon| {
                let exterior = stabilize_mount_ring(polygon.exterior(), &base_points);
                let interiors = polygon
                    .interiors()
                    .iter()
                    .map(|interior| stabilize_mount_ring(interior, &base_points))
                    .collect();
                Polygon::new(exterior, interiors)
            })
            .collect(),
    )
}

fn stabilize_mount_ring(line: &LineString<f64>, base_points: &[[f32; 2]]) -> LineString<f64> {
    let mut points = line
        .0
        .iter()
        .take(line.0.len().saturating_sub(1))
        .map(|point| {
            let point = [point.x as f32, point.y as f32];
            base_points
                .iter()
                .copied()
                .find(|base_point| {
                    (base_point[0] - point[0]).abs() <= 0.000_06
                        && (base_point[1] - point[1]).abs() <= 0.000_06
                })
                .unwrap_or_else(|| {
                    [
                        (point[0] * 10_000.0).round() / 10_000.0,
                        (point[1] * 10_000.0).round() / 10_000.0,
                    ]
                })
        })
        .collect::<Vec<_>>();
    points.dedup_by(|left, right| left == right);
    ring(&points)
}

pub(crate) fn circle_points(center: [f32; 2], radius: f32) -> Vec<[f32; 2]> {
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

fn circle(center: [f32; 2], radius: f32) -> Vec<[f32; 2]> {
    circle_points(center, radius)
}

fn wall_hardware_features(mount: &WallMountSpec) -> (f32, Vec<MountCavity>) {
    let engagement = (mount.engagement_depth_mm() - mount.fit_clearance_mm).max(0.2);
    let shift = match mount.style {
        WallMountStyle::AngledPin => engagement * ANGLED_PIN_DEGREES.to_radians().tan(),
        WallMountStyle::FrenchCleat => engagement * FRENCH_CLEAT_DEGREES.to_radians().tan(),
        WallMountStyle::None | WallMountStyle::StraightPin => 0.0,
    };
    let features = match mount.style {
        WallMountStyle::None => Vec::new(),
        WallMountStyle::StraightPin | WallMountStyle::AngledPin => {
            let radius = (mount.pin_diameter_mm - mount.fit_clearance_mm) * 0.5;
            let centers = if mount.pin_count == 2 {
                vec![-mount.pin_spacing_mm * 0.5, mount.pin_spacing_mm * 0.5]
            } else {
                vec![0.0]
            };
            centers
                .into_iter()
                .map(|center_x| MountCavity {
                    opening: circle([center_x, 0.0], radius),
                    ceiling: circle([center_x, shift], radius),
                })
                .collect()
        }
        WallMountStyle::FrenchCleat => {
            let half_width = (mount.cleat_width_mm - mount.fit_clearance_mm) * 0.5;
            let half_height = (mount.pin_diameter_mm - mount.fit_clearance_mm) * 0.5;
            vec![MountCavity {
                opening: rectangle(0.0, 0.0, half_width, half_height),
                ceiling: rectangle(0.0, shift, half_width, half_height),
            }]
        }
    };
    (engagement, features)
}

fn feature_bounds(features: &[MountCavity]) -> [f32; 4] {
    features
        .iter()
        .flat_map(|feature| feature.opening.iter().chain(&feature.ceiling))
        .fold(
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

fn hardware_plate_and_screw_centers(
    mount: &WallMountSpec,
    feature_bounds: [f32; 4],
) -> (Vec<[f32; 2]>, Vec<[f32; 2]>) {
    let screw_radius = screw_head_radius(mount);
    let center_x = (feature_bounds[0] + feature_bounds[2]) * 0.5;
    let slide = wall_mount_slide(mount);
    // Keep both heads clear throughout engagement, not just when the peg or
    // cleat reaches its locked position.
    let lower_screw_y = feature_bounds[1] - slide - screw_radius - WALL_PLATE_SCREW_GAP_MM;
    let upper_screw_y = feature_bounds[3] + slide + screw_radius + WALL_PLATE_SCREW_GAP_MM;
    let minimum_x = feature_bounds[0] - WALL_PLATE_MARGIN_MM;
    let maximum_x = feature_bounds[2] + WALL_PLATE_MARGIN_MM;
    let minimum_y = lower_screw_y - screw_radius - WALL_PLATE_MARGIN_MM;
    let maximum_y = upper_screw_y + screw_radius + WALL_PLATE_MARGIN_MM;
    (
        rectangle(
            (minimum_x + maximum_x) * 0.5,
            (minimum_y + maximum_y) * 0.5,
            (maximum_x - minimum_x) * 0.5,
            (maximum_y - minimum_y) * 0.5,
        ),
        vec![[center_x, lower_screw_y], [center_x, upper_screw_y]],
    )
}

/// A 90-degree countersink grows its radius by one millimetre for each
/// millimetre of depth. Keeping that relation fixed needs only one control and
/// matches common metric flat-head screws.
fn screw_head_radius(mount: &WallMountSpec) -> f32 {
    mount.screw_hole_diameter_mm * 0.5 + mount.screw_countersink_depth_mm
}

fn hardware_plate_thickness(mount: &WallMountSpec) -> f32 {
    mount.thickness_mm
}

/// Builds the wall-side half of the selected mount. The flat plate is both a
/// screw flange and the requested wall spacer; its peg or cleat uses the same
/// angle as the receiver, reduced by the chosen fit clearance.
pub(crate) fn build_wall_hardware(mount: &WallMountSpec) -> Result<Mesh> {
    let (engagement, features) = wall_hardware_features(mount);
    if features.is_empty() {
        bail!("wall hardware needs an enabled mount style");
    }

    let feature_bounds = feature_bounds(&features);
    let (plate, screw_centers) = hardware_plate_and_screw_centers(mount, feature_bounds);
    let plate_thickness = hardware_plate_thickness(mount);
    let screw_radius = mount.screw_hole_diameter_mm * 0.5;
    let screw_head_radius = screw_head_radius(mount);
    let screw_holes = screw_centers
        .iter()
        .map(|center| circle(*center, screw_radius))
        .collect::<Vec<_>>();
    let screw_head_holes = screw_centers
        .iter()
        .map(|center| circle(*center, screw_head_radius))
        .collect::<Vec<_>>();
    let bottom = Polygon::new(
        ring(&plate),
        screw_holes.iter().map(|hole| ring(hole)).collect(),
    );
    let top = Polygon::new(
        ring(&plate),
        screw_head_holes
            .iter()
            .map(|hole| ring(hole))
            .chain(features.iter().map(|feature| ring(&feature.opening)))
            .collect(),
    );
    let mut mesh = MeshBuilder::default();
    add_horizontal_polygons(&mut mesh, &[bottom], 0.0, SurfaceClass::Rock, true)?;
    add_horizontal_polygons(
        &mut mesh,
        &[top],
        plate_thickness,
        SurfaceClass::Rock,
        false,
    )?;
    add_ring_wall(&mut mesh, &plate, 0.0, plate_thickness, false);
    let countersink_floor = plate_thickness - mount.screw_countersink_depth_mm;
    for (hole, head_hole) in screw_holes.iter().zip(&screw_head_holes) {
        add_ring_wall(&mut mesh, hole, 0.0, countersink_floor, true);
        if mount.screw_countersink_depth_mm > 0.000_01 {
            add_cavity_wall(
                &mut mesh,
                hole,
                countersink_floor,
                head_hole,
                plate_thickness,
            );
        }
    }
    for feature in &features {
        add_horizontal_polygons(
            &mut mesh,
            &[polygon(&feature.ceiling)],
            plate_thickness + engagement,
            SurfaceClass::Rock,
            false,
        )?;
        for index in 0..feature.opening.len() {
            let next = (index + 1) % feature.opening.len();
            let lower_a = feature.opening[index];
            let lower_b = feature.opening[next];
            let upper_a = feature.ceiling[index];
            let upper_b = feature.ceiling[next];
            mesh.quad(
                [lower_a[0], lower_a[1], plate_thickness],
                [lower_b[0], lower_b[1], plate_thickness],
                [upper_b[0], upper_b[1], plate_thickness + engagement],
                [upper_a[0], upper_a[1], plate_thickness + engagement],
                SurfaceClass::Rock,
            );
        }
    }
    let mut result = mesh.finish("Wall mount hardware");
    let [minimum_x, minimum_y, _, _] = bounds(
        &result
            .vertices
            .iter()
            .map(|point| [point[0], point[1]])
            .collect::<Vec<_>>(),
    );
    for vertex in &mut result.vertices {
        vertex[0] -= minimum_x;
        vertex[1] -= minimum_y;
    }
    weld_export_mesh(&mut result);
    Ok(result)
}

/// Builds a thin placement jig for one cleat target. Print one per target,
/// place the outer edges together, and mark or drill through the two pilot
/// holes before removing the jigs and installing the wall-side hardware.
pub(crate) fn build_wall_alignment_spacer(spec: &GenerationSpec) -> Result<Mesh> {
    if spec.wall_mount.style != WallMountStyle::FrenchCleat {
        bail!("wall alignment spacers need a French-cleat mount");
    }
    let [width, height] = spec.wall_mount_target_size();
    if width <= ALIGNMENT_FRAME_BAND_MM * 2.0 || height <= ALIGNMENT_FRAME_BAND_MM * 2.0 {
        bail!("wall alignment spacer is too small for this output");
    }

    let (_, features) = wall_hardware_features(&spec.wall_mount);
    let feature_bounds = feature_bounds(&features);
    let (plate, screw_offsets) = hardware_plate_and_screw_centers(&spec.wall_mount, feature_bounds);
    let plate_bounds = bounds(&plate);
    let mount_center = wall_mount_center(&spec.wall_mount, [0.0, 0.0, width, height]);
    if plate_bounds[0] + mount_center[0] < 0.0
        || plate_bounds[1] + mount_center[1] < 0.0
        || plate_bounds[2] + mount_center[0] > width
        || plate_bounds[3] + mount_center[1] > height
    {
        bail!(
            "French-cleat plate does not fit its alignment spacer; reduce the cleat height, width, or screw-hole size, or use fewer pieces"
        );
    }

    let outer = polygon(&rectangle(
        width * 0.5,
        height * 0.5,
        width * 0.5,
        height * 0.5,
    ));
    let inner = polygon(&rectangle(
        width * 0.5,
        height * 0.5,
        width * 0.5 - ALIGNMENT_FRAME_BAND_MM,
        height * 0.5 - ALIGNMENT_FRAME_BAND_MM,
    ));
    let mut shape = outer.difference(&inner);
    let vertical_rail = polygon(&rectangle(
        mount_center[0],
        height * 0.5,
        ALIGNMENT_RAIL_HALF_WIDTH_MM,
        height * 0.5,
    ));
    shape = shape.union(&vertical_rail);

    let screw_radius = spec.wall_mount.screw_hole_diameter_mm * 0.5;
    for offset in screw_offsets {
        let center = [mount_center[0] + offset[0], mount_center[1] + offset[1]];
        let rail = polygon(&rectangle(
            width * 0.5,
            center[1],
            width * 0.5,
            ALIGNMENT_RAIL_HALF_WIDTH_MM,
        ));
        shape = shape.union(&rail);
        shape = shape.union(&polygon(&circle(
            center,
            screw_radius + ALIGNMENT_FRAME_BAND_MM,
        )));
        shape = shape.difference(&polygon(&circle(center, screw_radius)));
    }
    // The support collars can reach the frame edge on very small pieces.
    // Keep the jig's outer dimensions exact so adjacent frames still align.
    shape = shape.intersection(&outer);

    let mut mesh = MeshBuilder::default();
    add_horizontal_polygons(&mut mesh, &shape.0, 0.0, SurfaceClass::Rock, true)?;
    add_horizontal_polygons(
        &mut mesh,
        &shape.0,
        ALIGNMENT_FRAME_THICKNESS_MM,
        SurfaceClass::Rock,
        false,
    )?;
    for polygon in &shape.0 {
        add_line_string_wall(
            &mut mesh,
            polygon.exterior(),
            0.0,
            ALIGNMENT_FRAME_THICKNESS_MM,
            false,
        );
        for interior in polygon.interiors() {
            add_line_string_wall(&mut mesh, interior, 0.0, ALIGNMENT_FRAME_THICKNESS_MM, true);
        }
    }
    let mut result = mesh.finish("Wall mount alignment spacer");
    weld_export_mesh(&mut result);
    Ok(result)
}

fn add_line_string_wall(
    mesh: &mut MeshBuilder,
    line: &LineString<f64>,
    lower_z: f32,
    upper_z: f32,
    inward: bool,
) {
    let mut points = line
        .0
        .iter()
        .take(line.0.len().saturating_sub(1))
        .map(|point| [point.x as f32, point.y as f32])
        .collect::<Vec<_>>();
    let signed_area = points
        .iter()
        .zip(points.iter().cycle().skip(1))
        .map(|(from, to)| from[0] * to[1] - to[0] * from[1])
        .sum::<f32>();
    if signed_area < 0.0 {
        points.reverse();
    }
    add_ring_wall(mesh, &points, lower_z, upper_z, inward);
}

fn add_polygon_walls(
    mesh: &mut MeshBuilder,
    polygons: &[Polygon<f64>],
    lower_z: f32,
    upper_z: f32,
) {
    for polygon in polygons {
        add_line_string_wall(mesh, polygon.exterior(), lower_z, upper_z, false);
        for interior in polygon.interiors() {
            add_line_string_wall(mesh, interior, lower_z, upper_z, true);
        }
    }
}

fn add_ring_wall(
    mesh: &mut MeshBuilder,
    points: &[[f32; 2]],
    lower_z: f32,
    upper_z: f32,
    inward: bool,
) {
    for index in 0..points.len() {
        let next = (index + 1) % points.len();
        let a = points[index];
        let b = points[next];
        if inward {
            mesh.quad(
                [b[0], b[1], lower_z],
                [a[0], a[1], lower_z],
                [a[0], a[1], upper_z],
                [b[0], b[1], upper_z],
                SurfaceClass::Rock,
            );
        } else {
            mesh.quad(
                [a[0], a[1], lower_z],
                [b[0], b[1], lower_z],
                [b[0], b[1], upper_z],
                [a[0], a[1], upper_z],
                SurfaceClass::Rock,
            );
        }
    }
}

fn rectangle(center_x: f32, center_y: f32, half_width: f32, half_height: f32) -> Vec<[f32; 2]> {
    vec![
        [center_x - half_width, center_y - half_height],
        [center_x + half_width, center_y - half_height],
        [center_x + half_width, center_y + half_height],
        [center_x - half_width, center_y + half_height],
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mesh::assert_watertight;

    #[test]
    fn every_wall_hardware_style_is_watertight() {
        for style in [
            WallMountStyle::StraightPin,
            WallMountStyle::AngledPin,
            WallMountStyle::FrenchCleat,
        ] {
            let mount = WallMountSpec {
                style,
                pin_count: 2,
                ..WallMountSpec::default()
            };
            let hardware = build_wall_hardware(&mount).unwrap();
            assert_watertight(&hardware);
            let plate_thickness = hardware_plate_thickness(&mount);
            assert!(
                hardware
                    .vertices
                    .iter()
                    .any(|vertex| vertex[2] > plate_thickness)
            );
        }
    }

    #[test]
    fn countersink_depth_shapes_a_watertight_plate_and_its_pocket() {
        let mut mount = WallMountSpec {
            style: WallMountStyle::StraightPin,
            thickness_mm: 2.0,
            screw_countersink_depth_mm: 0.8,
            ..WallMountSpec::default()
        };
        let (_, features) = wall_hardware_features(&mount);
        let deep_plate = hardware_plate_and_screw_centers(&mount, feature_bounds(&features)).0;
        let deep_bounds = bounds(&deep_plate);
        let hardware = build_wall_hardware(&mount).unwrap();
        assert_watertight(&hardware);
        assert!(hardware.vertices.iter().any(|point| {
            (point[2] - (mount.thickness_mm - mount.screw_countersink_depth_mm)).abs() < 0.000_01
        }));
        assert!(
            (screw_head_radius(&mount)
                - (mount.screw_hole_diameter_mm * 0.5 + mount.screw_countersink_depth_mm))
                .abs()
                < 0.000_01
        );

        mount.screw_countersink_depth_mm = 0.0;
        let plain_plate = hardware_plate_and_screw_centers(&mount, feature_bounds(&features)).0;
        let plain_bounds = bounds(&plain_plate);
        assert!(deep_bounds[3] - deep_bounds[1] > plain_bounds[3] - plain_bounds[1]);
        assert_watertight(&build_wall_hardware(&mount).unwrap());
    }

    #[test]
    fn screw_head_clearance_cuts_local_relief_to_the_deeper_requested_ceiling() {
        let mount = WallMountSpec {
            style: WallMountStyle::FrenchCleat,
            screw_head_clearance_mm: 1.4,
            ..WallMountSpec::default()
        };
        let outline = rectangle(40.0, 40.0, 40.0, 40.0);
        let receiver = mount_bottom(&outline, &mount, [0.0, 0.0, 80.0, 80.0])
            .unwrap()
            .finish("receiver with screw-head relief");
        assert!(receiver.vertices.iter().any(|point| {
            (point[2] - (mount.pocket_depth_mm() + mount.screw_head_clearance_mm)).abs() < 0.000_01
        }));
    }

    #[test]
    fn french_cleat_receiver_has_a_flush_entry_box_and_slide_travel() {
        let mount = WallMountSpec {
            style: WallMountStyle::FrenchCleat,
            ..WallMountSpec::default()
        };
        let outline = rectangle(20.0, 20.0, 20.0, 20.0);
        let outline_polygon = polygon(&outline);
        let cavities =
            cavities_for_outline(&outline_polygon, &mount, [0.0, 0.0, 40.0, 40.0]).unwrap();
        let cavity = &cavities[0];
        let opening = bounds(&cavity.opening);
        let ceiling = bounds(&cavity.ceiling);
        let shift = mount.engagement_depth_mm() * FRENCH_CLEAT_DEGREES.to_radians().tan();

        assert!((opening[3] - opening[1]) >= mount.pin_diameter_mm + FRENCH_CLEAT_MIN_SLIDE_MM);
        assert!(((ceiling[1] - opening[1]) - shift).abs() < 0.000_01);
        assert_eq!(opening[0], 20.0 - mount.cleat_width_mm * 0.5);
        assert_eq!(opening[2], 20.0 + mount.cleat_width_mm * 0.5);
    }

    #[test]
    fn french_cleat_alignment_spacer_is_watertight_and_matches_one_terrain_tile() {
        let spec = GenerationSpec {
            width_mm: 180.0,
            rows: 10,
            columns: 10,
            wall_mount: WallMountSpec {
                style: WallMountStyle::FrenchCleat,
                ..WallMountSpec::default()
            },
            ..GenerationSpec::default()
        };
        let spacer = build_wall_alignment_spacer(&spec).unwrap();
        assert_watertight(&spacer);
        let points = spacer
            .vertices
            .iter()
            .map(|point| [point[0], point[1]])
            .collect::<Vec<_>>();
        let spacer_bounds = bounds(&points);
        assert_eq!(spacer_bounds, [0.0, 0.0, 180.0, 180.0]);
        assert!(
            spacer
                .vertices
                .iter()
                .any(|point| { (point[2] - ALIGNMENT_FRAME_THICKNESS_MM).abs() < 0.000_01 })
        );
    }

    #[test]
    fn french_cleat_derives_its_pocket_from_thickness_and_offset() {
        let mut mount = WallMountSpec {
            style: WallMountStyle::FrenchCleat,
            thickness_mm: 1.7,
            wall_offset_mm: 0.5,
            ..WallMountSpec::default()
        };
        let close_hardware = build_wall_hardware(&mount).unwrap();
        let close_height = close_hardware
            .vertices
            .iter()
            .map(|point| point[2])
            .fold(f32::NEG_INFINITY, f32::max);

        mount.wall_offset_mm = 1.0;
        let offset_hardware = build_wall_hardware(&mount).unwrap();
        let offset_height = offset_hardware
            .vertices
            .iter()
            .map(|point| point[2])
            .fold(f32::NEG_INFINITY, f32::max);
        assert!((offset_height - close_height).abs() < 0.000_01);

        let outline = rectangle(20.0, 20.0, 20.0, 20.0);
        let pocket = wall_plate_pocket(&mount, [0.0, 0.0, 40.0, 40.0]);
        let shallow_receiver = mount_bottom(&outline, &mount, [0.0, 0.0, 40.0, 40.0])
            .unwrap()
            .finish("shallow receiver");
        for corner in pocket {
            assert!(shallow_receiver.vertices.iter().any(|point| {
                (point[0] - corner[0]).abs() < 0.000_01
                    && (point[1] - corner[1]).abs() < 0.000_01
                    && point[2].abs() < 0.000_01
            }));
        }
        assert!(
            shallow_receiver
                .vertices
                .iter()
                .any(|point| (point[2] - mount.pocket_depth_mm()).abs() < 0.000_01)
        );
        let receiver_ceiling = mount.embedded_depth_mm();
        assert!(
            shallow_receiver
                .vertices
                .iter()
                .any(|point| (point[2] - receiver_ceiling).abs() < 0.000_01)
        );

        mount.thickness_mm = 3.5;
        let deep_receiver = mount_bottom(&outline, &mount, [0.0, 0.0, 40.0, 40.0])
            .unwrap()
            .finish("deep receiver");
        assert!(
            deep_receiver
                .vertices
                .iter()
                .any(|point| (point[2] - 3.3).abs() < 0.000_01)
        );
    }

    #[test]
    fn french_cleat_pocket_sweeps_the_whole_plate_through_its_entry_travel() {
        let mount = WallMountSpec {
            style: WallMountStyle::FrenchCleat,
            depth_mm: 1.2,
            pin_diameter_mm: 6.0,
            fit_clearance_mm: 0.3,
            ..WallMountSpec::default()
        };
        let frame = [0.0, 0.0, 60.0, 50.0];
        let [center_x, center_y] = wall_mount_center(&mount, frame);
        let (_, features) = wall_hardware_features(&mount);
        let (plate, _) = hardware_plate_and_screw_centers(&mount, feature_bounds(&features));
        let plate_bounds = bounds(&plate);
        let pocket_bounds = bounds(&wall_plate_pocket(&mount, frame));
        let slide = wall_mount_slide(&mount);

        assert!((pocket_bounds[0] - (center_x + plate_bounds[0] - 0.3)).abs() < 0.000_01);
        assert!((pocket_bounds[2] - (center_x + plate_bounds[2] + 0.3)).abs() < 0.000_01);
        assert!((pocket_bounds[1] - (center_y + plate_bounds[1] - slide - 0.3)).abs() < 0.000_01);
        assert!((pocket_bounds[3] - (center_y + plate_bounds[3] + 0.3)).abs() < 0.000_01);
        assert!(slide >= mount.pin_diameter_mm * 0.75);
        assert!(
            ((pocket_bounds[3] - pocket_bounds[1])
                - (plate_bounds[3] - plate_bounds[1] + slide + 0.6))
                .abs()
                < 0.000_01
        );
    }
}
