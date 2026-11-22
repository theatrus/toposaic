//! Small World map wrapping.
//!
//! A Small World globe is the ordinary terrain model of the selected map,
//! bent onto the sphere. The map is laid out in millimetres at the globe's
//! own scale, so road widths, building heights, and marker sizes mean the
//! same thing they do on a flat print. Each tile traces its outline onto the
//! map, the terrain piece builder fills that outline with terrain, roads,
//! buildings, markers, and colors, and the result is moved onto the tile.
//!
//! Two projections place the map on the sphere:
//!
//! - **Regional** is azimuthal equidistant around the center tile. The map
//!   is square, and its corners reach the chosen wrap angle. Distances from
//!   the center are true to scale.
//! - **360° band** is equirectangular. The map wraps once around the globe,
//!   and its middle strip covers the chosen band of latitude. The map stays
//!   square, as every terrain map is, so the strip keeps true proportions.
//!   The east and west edges blend into each other, and the band fades into
//!   plain polar caps.
//!
//! Tiles that contain a point the projection cannot unfold (a pole of the
//! band, the far side of a regional map) or that cross the band's join keep
//! the sampled tile surface, read from the same faded map so their heights
//! and colors agree with their neighbors.

use std::collections::HashMap;
use std::f32::consts::{SQRT_2, TAU};

use anyhow::{Result, ensure};

use super::ground::{CLASSES, Ground, smoothstep};
use super::layout::{Face, Pose, Vec3, add, dot, scale, unit};
use super::{GlobePrototypeSpec, GlobeSource};
use crate::heightfield::{height_range_for_spec, normalized_height};
use crate::mesh::Mesh;
use crate::piece::{OutlinePiece, build_outline_piece};
use crate::spec::{
    GenerationSpec, ModelOutlineSpec, PrintMaterial, PuzzleRetentionSpec, SurfaceClass,
    WallMountSpec,
};
use crate::{HeightField, SurfaceField};

/// Latitude spans offered for the 360° band.
pub const BAND_DEGREES: [f32; 4] = [144.0, 120.0, 90.0, 60.0];

/// Thickness under zero relief in the flat map model. It only parameterizes
/// the wall between the tile back and the terrain; the tile's real thickness
/// comes from the globe layout.
const MAP_BASE_MM: f32 = 4.0;
/// Fraction of a regional map, or of the band's height, in from each edge,
/// over which the map fades to plain ground.
const EDGE_FADE: f32 = 0.08;
/// Fraction of the band's width, in from the east and west edges, over which
/// the two edges blend into one another.
const JOIN_BLEND: f32 = 0.06;
/// Tracing step along each tile edge, in the tile's back plane.
const TRACE_STEP_MM: f32 = 0.75;

pub(super) fn validate_band(degrees: f32) -> Result<()> {
    ensure!(
        BAND_DEGREES.iter().any(|span| (span - degrees).abs() < 0.5),
        "360° band height must be one of 144, 120, 90, or 60 degrees"
    );
    Ok(())
}

/// The flat terrain model a Small World globe is cut from: the selected
/// map, laid out at the globe's scale, as one solid model with no tray,
/// mount, or puzzle cuts. The API fetches elevation and map data for this
/// spec, and the globe builds its tiles from it.
pub fn small_world_map_spec(spec: &GenerationSpec) -> Option<GenerationSpec> {
    let globe = spec.globe?;
    if globe.source != GlobeSource::SmallWorld {
        return None;
    }
    let radius = globe.diameter_mm / 2.0;
    let mut map = spec.clone();
    map.globe = None;
    map.globe_landmarks.clear();
    map.solid_model = true;
    map.map_frame = None;
    map.model_outline = ModelOutlineSpec::default();
    map.adjacent_rows = 1;
    map.adjacent_columns = 1;
    map.adjacent_tile_row = 0;
    map.adjacent_tile_column = 0;
    map.tray.enabled = false;
    map.wall_mount = WallMountSpec::default();
    map.puzzle_retention = PuzzleRetentionSpec::default();
    map.export_piece_stls = false;
    // Never thinner than the user's base: flag sockets are sized against it,
    // and anything at or below zero becomes the tile back.
    map.base_mm = MAP_BASE_MM.max(spec.base_mm);
    map.relief_mm = globe.relief_mm;
    map.height_scale = Default::default();
    map.elevation_datum_m = None;
    map.elevation_m_per_mm = None;
    map.rows = 1;
    map.columns = 1;
    map.width_mm = if globe.full_wrap {
        // One trip around the equator. The band is the middle strip.
        TAU * radius
    } else {
        // The corners sit at the wrap angle, so the half diagonal is the
        // arc from the center to a corner.
        SQRT_2 * radius * globe.wrap_degrees.to_radians()
    };
    Some(map)
}

#[derive(Clone, Copy)]
pub(super) struct MapWrap {
    axes: Pose,
    radius: f32,
    width: f32,
    height: f32,
    band: bool,
    /// Half the band's height on the map, in millimetres.
    band_half: f32,
}

impl MapWrap {
    pub(super) fn new(
        axes: Pose,
        radius: f32,
        map: &GenerationSpec,
        globe: &GlobePrototypeSpec,
    ) -> Self {
        Self {
            axes,
            radius,
            width: map.width_mm,
            height: map.height_mm(),
            band: globe.full_wrap,
            band_half: radius * globe.band_degrees.to_radians() / 2.0,
        }
    }

    /// The unit direction a map point lands on.
    pub(super) fn direction(&self, [x, y]: [f32; 2]) -> Vec3 {
        let dx = (x - self.width / 2.0) / self.radius;
        let dy = (y - self.height / 2.0) / self.radius;
        let Pose {
            x_axis,
            y_axis,
            z_axis,
            ..
        } = self.axes;
        if self.band {
            let (sin_lon, cos_lon) = dx.sin_cos();
            let (sin_lat, cos_lat) = dy.sin_cos();
            return add(
                scale(x_axis, cos_lat * sin_lon),
                add(scale(y_axis, sin_lat), scale(z_axis, cos_lat * cos_lon)),
            );
        }
        let angle = dx.hypot(dy);
        if angle < 1.0e-9 {
            return z_axis;
        }
        let (sin, cos) = angle.sin_cos();
        add(
            scale(z_axis, cos),
            add(
                scale(x_axis, sin * dx / angle),
                scale(y_axis, sin * dy / angle),
            ),
        )
    }

    /// The map point, in millimetres, under a direction. Points off the map
    /// are returned as they are, outside its rectangle.
    pub(super) fn map_point(&self, direction: Vec3) -> [f32; 2] {
        let x = dot(direction, self.axes.x_axis);
        let y = dot(direction, self.axes.y_axis);
        let z = dot(direction, self.axes.z_axis);
        if self.band {
            let longitude = x.atan2(z);
            let latitude = y.clamp(-1.0, 1.0).asin();
            return [
                self.width / 2.0 + longitude * self.radius,
                self.height / 2.0 + latitude * self.radius,
            ];
        }
        let tangent = x.hypot(y);
        let angle = tangent.atan2(z);
        let factor = if tangent > 1.0e-9 {
            angle / tangent
        } else {
            1.0
        };
        [
            self.width / 2.0 + x * factor * self.radius,
            self.height / 2.0 + y * factor * self.radius,
        ]
    }

    fn uv(&self, [x, y]: [f32; 2]) -> [f32; 2] {
        [x / self.width, y / self.height]
    }

    /// Position across the printed area: the whole map for a regional
    /// globe, the band's strip for a 360° wrap.
    fn area_uv(&self, [x, y]: [f32; 2]) -> [f32; 2] {
        if self.band {
            let bottom = self.height / 2.0 - self.band_half;
            [x / self.width, (y - bottom) / (2.0 * self.band_half)]
        } else {
            self.uv([x, y])
        }
    }

    /// Whether a map point lies on the printed area, with a margin in
    /// fractions of that area.
    fn on_area(&self, point: [f32; 2], margin: f32) -> bool {
        let [u, v] = self.area_uv(point);
        (-margin..=1.0 + margin).contains(&u) && (-margin..=1.0 + margin).contains(&v)
    }

    /// One inside the printed area, falling to zero at and beyond its faded
    /// edges. The band fades only at its top and bottom; its east and west
    /// edges join instead.
    fn fade(&self, point: [f32; 2]) -> f32 {
        let [u, v] = self.area_uv(point);
        let edge = if self.band {
            v.min(1.0 - v)
        } else {
            u.min(v).min(1.0 - u).min(1.0 - v)
        };
        smoothstep(edge / EDGE_FADE)
    }

    /// How far a band sample mixes toward its mirror across the join. Both
    /// edges reach one half, so they print the same value where they meet.
    fn join(&self, u: f32) -> f32 {
        if !self.band {
            return 0.0;
        }
        0.5 * (1.0 - smoothstep(u.min(1.0 - u) / JOIN_BLEND))
    }

    /// Directions the projection cannot unfold onto a flat map.
    fn singular(&self) -> Vec<Vec3> {
        if self.band {
            vec![self.axes.y_axis, scale(self.axes.y_axis, -1.0)]
        } else {
            vec![scale(self.axes.z_axis, -1.0)]
        }
    }
}

/// The selected map with its edges faded and, for a band, its join blended.
/// Both the flat-built tiles and the sampled ones read from here.
pub(super) struct WrappedMap<'a> {
    pub(super) wrap: MapWrap,
    pub(super) spec: &'a GenerationSpec,
    field: HeightField,
    range: Option<(f32, f32)>,
    cover: Option<&'a SurfaceField>,
}

pub(super) enum TilePlan {
    /// The tile's outline on the map, in map millimetres.
    Map(Vec<[f32; 2]>),
    /// Build the tile from sampled heights and colors.
    Sampled,
}

impl<'a> WrappedMap<'a> {
    pub(super) fn new(
        wrap: MapWrap,
        spec: &'a GenerationSpec,
        height: &HeightField,
        cover: Option<&'a SurfaceField>,
    ) -> Self {
        let range = height_range_for_spec(spec, Some(height));
        let datum = range.map_or(0.0, |(minimum, _)| minimum);
        let mut field = height.clone();
        let (width, rows) = (height.width, height.height);
        for row in 0..rows {
            let v = row as f32 / (rows - 1).max(1) as f32;
            for column in 0..width {
                let u = column as f32 / (width - 1).max(1) as f32;
                let mut value = height.values_m[row * width + column];
                let join = wrap.join(u);
                if join > 0.0 {
                    let mirror = height.values_m[row * width + (width - 1 - column)];
                    value = value * (1.0 - join) + mirror * join;
                }
                let point = [u * wrap.width, v * wrap.height];
                field.values_m[row * width + column] = datum + (value - datum) * wrap.fade(point);
            }
        }
        Self {
            wrap,
            spec,
            field,
            range,
            cover,
        }
    }

    /// Relief above the globe's zero surface, in millimetres.
    fn height_at(&self, [u, v]: [f32; 2]) -> f32 {
        self.spec.relief_mm
            * normalized_height(
                Some(&self.field),
                self.range,
                u,
                v,
                self.spec.center_lat,
                self.spec.center_lon,
            )
    }

    pub(super) fn sample(&self, direction: Vec3) -> Ground {
        let point = self.wrap.map_point(direction);
        let uv = self.wrap.uv(point);
        let [u, v] = uv;
        let mut ground = match self.cover {
            None => Ground::solid(0.0, 0),
            Some(cover) => {
                let at = |u: f32| {
                    let class = cover.terrain_class_at(u.clamp(0.0, 1.0), v.clamp(0.0, 1.0));
                    let index = CLASSES.iter().position(|item| *item == class).unwrap_or(0);
                    Ground::solid(0.0, index)
                };
                at(u).blend(at(1.0 - u), self.wrap.join(u))
            }
        };
        ground = Ground::NEUTRAL.blend(ground, self.wrap.fade(point));
        ground.height = self.height_at(uv);
        ground
    }

    pub(super) fn material(&self, direction: Vec3) -> PrintMaterial {
        let point = self.wrap.map_point(direction);
        let [u, v] = self.wrap.uv(point);
        match self.cover {
            // The same test the map-built tiles use, so seams match in color.
            Some(cover) if self.wrap.on_area(point, 0.0) => cover.print_material_at(u, v),
            _ => PrintMaterial::Class(SurfaceClass::Rock),
        }
    }

    /// Decide how to build a tile, tracing its seam-inset outline onto the
    /// map when the projection can unfold it.
    pub(super) fn plan(&self, face: &Face, seam_gap_mm: f32) -> TilePlan {
        let nominal = face.outline_at(0.0, 0.0);
        let margin = -0.08 * face.inradius();
        let expanded = face.outline_at(0.0, margin);
        let base_radius = dot(face.pose.origin, face.pose.z_axis);
        for point in self.wrap.singular() {
            let local = [
                dot(point, face.pose.x_axis),
                dot(point, face.pose.y_axis),
                dot(point, face.pose.z_axis),
            ];
            if local[2] <= 0.0 {
                continue;
            }
            let plane = [
                local[0] * base_radius / local[2],
                local[1] * base_radius / local[2],
            ];
            if in_triangle(plane, expanded) {
                return TilePlan::Sampled;
            }
        }
        let direction = |[x, y]: [f32; 2]| unit(face.pose.to_globe([x, y, 0.0]));
        // A tile wholly off the map needs no map geometry.
        let touches_map = (0..=12).any(|i| {
            (0..=12 - i).any(|j| {
                let weights = [(12 - i - j) as f32, i as f32, j as f32].map(|w| w / 12.0);
                let point = std::array::from_fn(|axis| {
                    nominal
                        .iter()
                        .zip(weights)
                        .map(|(corner, weight)| corner[axis] * weight)
                        .sum()
                });
                self.wrap
                    .on_area(self.wrap.map_point(direction(point)), 0.02)
            })
        });
        if !touches_map {
            return TilePlan::Sampled;
        }
        let outline = face.outline_at(0.0, seam_gap_mm / 2.0);
        let mut traced = Vec::new();
        for edge in 0..3 {
            let [start, end] = [outline[edge], outline[(edge + 1) % 3]];
            let length = (end[0] - start[0]).hypot(end[1] - start[1]);
            let steps = (length / TRACE_STEP_MM).ceil().max(1.0) as usize;
            for step in 0..steps {
                let t = step as f32 / steps as f32;
                let point = [
                    start[0] + (end[0] - start[0]) * t,
                    start[1] + (end[1] - start[1]) * t,
                ];
                traced.push(self.wrap.map_point(direction(point)));
            }
        }
        if self.wrap.band {
            // A tile across the join would unfold into two pieces.
            let half = self.wrap.width / 2.0;
            let crosses = traced
                .iter()
                .zip(traced.iter().cycle().skip(1))
                .any(|(a, b)| (a[0] - b[0]).abs() > half);
            if crosses {
                return TilePlan::Sampled;
            }
        }
        TilePlan::Map(traced)
    }

    /// Build the flat terrain piece for a traced tile outline.
    pub(super) fn build_piece(
        &self,
        outline: &[[f32; 2]],
        spacing_mm: f32,
        name: String,
    ) -> Result<Mesh> {
        build_outline_piece(
            self.spec,
            Some(&self.field),
            self.range,
            self.cover,
            &OutlinePiece {
                outline,
                spacing_mm,
                name,
            },
        )
    }
}

fn in_triangle(point: [f32; 2], triangle: [[f32; 2]; 3]) -> bool {
    let side = |a: [f32; 2], b: [f32; 2]| {
        (b[0] - a[0]) * (point[1] - a[1]) - (b[1] - a[1]) * (point[0] - a[0])
    };
    let signs = [
        side(triangle[0], triangle[1]),
        side(triangle[1], triangle[2]),
        side(triangle[2], triangle[0]),
    ];
    signs.iter().all(|s| *s >= 0.0) || signs.iter().all(|s| *s <= 0.0)
}

/// A flat map piece moved onto its globe tile, in the tile's own frame.
pub(super) struct WrappedPiece {
    /// Terrain and overlay triangles, upward-facing ones first.
    pub(super) triangles: Vec<([Vec3; 3], PrintMaterial)>,
    pub(super) upward: usize,
    /// The closed loop where the tile walls meet the tile back, in the back
    /// plane.
    pub(super) back: Vec<[f32; 2]>,
}

/// Bend a flat map piece onto a tile. The map's zero-relief surface lands on
/// the globe's surface, relief rises radially, and the flat bottom of the
/// piece becomes the tile's back plane. The bottom itself is dropped: the
/// tile closes its back with the clip socket and locator.
pub(super) fn wrap_piece(
    piece: &Mesh,
    face: &Face,
    globe: &GlobePrototypeSpec,
    map: &WrappedMap,
) -> Result<WrappedPiece> {
    use super::GlobeExterior;
    let base_radius = dot(face.pose.origin, face.pose.z_axis);
    let base = map.spec.base_mm;
    let warped = piece
        .vertices
        .iter()
        .map(|&[x, y, z]| {
            let direction = map.wrap.direction([x, y]);
            let local = [
                dot(direction, face.pose.x_axis),
                dot(direction, face.pose.y_axis),
                dot(direction, face.pose.z_axis),
            ];
            ensure!(
                local[2] > 1.0e-3,
                "map point falls outside tile {:02}",
                face.id
            );
            let back = [
                local[0] * base_radius / local[2],
                local[1] * base_radius / local[2],
            ];
            if z <= 1.0e-6 {
                return Ok([back[0], back[1], 0.0]);
            }
            let length = back[0].hypot(back[1]).hypot(base_radius);
            let zero = match globe.exterior {
                GlobeExterior::Faceted => face.chord_radius / base_radius,
                GlobeExterior::Rounded => face.radius / length,
            };
            let factor = if z < base {
                1.0 + (zero - 1.0) * z / base
            } else {
                zero + (z - base) / length
            };
            Ok([
                back[0] * factor,
                back[1] * factor,
                base_radius * (factor - 1.0),
            ])
        })
        .collect::<Result<Vec<_>>>()?;
    let on_back = |index: u32| piece.vertices[index as usize][2] <= 1.0e-6;
    let mut upward = Vec::new();
    let mut other = Vec::new();
    let mut edge_uses = HashMap::<(u32, u32), (u32, [u32; 2])>::new();
    for (triangle, material) in piece.triangles.iter().zip(&piece.materials) {
        if triangle.iter().all(|&index| on_back(index)) {
            continue;
        }
        for directed in [
            [triangle[0], triangle[1]],
            [triangle[1], triangle[2]],
            [triangle[2], triangle[0]],
        ] {
            let key = (directed[0].min(directed[1]), directed[0].max(directed[1]));
            edge_uses.entry(key).or_insert((0, directed)).0 += 1;
        }
        let [a, b, c] = triangle.map(|index| piece.vertices[index as usize]);
        let normal_z = (b[0] - a[0]) * (c[1] - a[1]) - (b[1] - a[1]) * (c[0] - a[0]);
        let area = {
            let ab = [b[0] - a[0], b[1] - a[1], b[2] - a[2]];
            let ac = [c[0] - a[0], c[1] - a[1], c[2] - a[2]];
            let cross = [
                ab[1] * ac[2] - ab[2] * ac[1],
                ab[2] * ac[0] - ab[0] * ac[2],
                ab[0] * ac[1] - ab[1] * ac[0],
            ];
            (cross[0] * cross[0] + cross[1] * cross[1] + cross[2] * cross[2]).sqrt()
        };
        let centroid = [(a[0] + b[0] + c[0]) / 3.0, (a[1] + b[1] + c[1]) / 3.0];
        // Off the printed area the terrain is plain ground.
        let material = if map.wrap.on_area(centroid, 0.0) {
            *material
        } else {
            PrintMaterial::Class(SurfaceClass::Rock)
        };
        let item = (triangle.map(|index| warped[index as usize]), material);
        if area > 0.0 && normal_z / area > 0.5 {
            upward.push(item);
        } else {
            other.push(item);
        }
    }
    // The wall bottoms are the only open edges left once the piece bottom
    // is gone. Walk them into the loop that the tile back closes.
    let mut next = HashMap::new();
    for (uses, [from, to]) in edge_uses.into_values() {
        if uses == 1 {
            ensure!(
                on_back(from) && on_back(to),
                "tile {:02} map surface has an open edge above its back",
                face.id
            );
            ensure!(
                next.insert(from, to).is_none(),
                "tile {:02} back outline branches",
                face.id
            );
        }
    }
    ensure!(
        next.len() >= 3,
        "tile {:02} map piece has no back outline",
        face.id
    );
    let start = *next.keys().min().expect("non-empty");
    let mut back = Vec::with_capacity(next.len());
    let mut current = start;
    loop {
        let point = warped[current as usize];
        back.push([point[0], point[1]]);
        current = *next
            .get(&current)
            .ok_or_else(|| anyhow::anyhow!("tile {:02} back outline is open", face.id))?;
        if current == start {
            break;
        }
        ensure!(
            back.len() <= next.len(),
            "tile {:02} back outline does not close",
            face.id
        );
    }
    ensure!(
        back.len() == next.len(),
        "tile {:02} back outline has more than one loop",
        face.id
    );
    let count = upward.len();
    upward.extend(other);
    Ok(WrappedPiece {
        triangles: upward,
        upward: count,
        back,
    })
}
