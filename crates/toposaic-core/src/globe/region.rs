use super::ground::{CLASSES, Ground, smoothstep};
use super::layout::{Face, Pose, Vec3, dot};
use crate::{HeightField, SurfaceField};

pub(super) struct Region<'a> {
    field: &'a HeightField,
    cover: Option<&'a SurfaceField>,
    axes: Pose,
    corner_angle: f32,
    minimum: f32,
    relief_per_metre: f32,
    full_wrap: bool,
    poles: [Ground; 2],
}

impl<'a> Region<'a> {
    pub(super) fn new(
        field: &'a HeightField,
        cover: Option<&'a SurfaceField>,
        center: &Face,
        wrap_degrees: f32,
        relief_mm: f32,
        full_wrap: bool,
    ) -> Self {
        let (minimum, maximum) = field.elevation_bounds();
        let mut region = Self {
            field,
            cover,
            axes: center.pose,
            corner_angle: wrap_degrees.to_radians(),
            minimum,
            relief_per_metre: relief_mm / (maximum - minimum).max(1.0),
            full_wrap,
            poles: [Ground::ZERO; 2],
        };
        if full_wrap {
            region.poles = [0.0, 1.0].map(|v| {
                (0..64).fold(Ground::ZERO, |sum, i| {
                    sum.plus(region.at(i as f32 / 63.0, v), 1.0 / 64.0)
                })
            });
        }
        region
    }

    fn at(&self, u: f32, v: f32) -> Ground {
        let height = (self.field.elevation_m_at(u, v) - self.minimum) * self.relief_per_metre;
        let Some(cover) = self.cover else {
            return Ground::solid(height, 0);
        };
        // Average a small neighborhood before painting the mesh. This also
        // feathers texture over the boundaries of mapped ground categories.
        let mut ground = Ground::ZERO;
        let scale = 0.008;
        for (x, y, weight) in [
            (0.0, 0.0, 0.4),
            (scale, 0.0, 0.15),
            (-scale, 0.0, 0.15),
            (0.0, scale, 0.15),
            (0.0, -scale, 0.15),
        ] {
            let class = cover.terrain_class_at(u + x, v + y);
            let class_id = CLASSES.iter().position(|item| *item == class).unwrap_or(0);
            ground = ground.plus(Ground::solid(height, class_id), weight);
        }
        ground
    }

    pub(super) fn sample(&self, direction: Vec3) -> Ground {
        let x = dot(direction, self.axes.x_axis);
        let y = dot(direction, self.axes.y_axis);
        let z = dot(direction, self.axes.z_axis);
        if self.full_wrap {
            // East and west meet halfway between the edge samples. The
            // polar rows converge to one value so the poles cannot pinch.
            let u = 0.5 + x.atan2(z) / std::f32::consts::TAU;
            let v = 0.5 + y.clamp(-1.0, 1.0).asin() / std::f32::consts::PI;
            let join = 0.5 * (1.0 - smoothstep(u.min(1.0 - u) / 0.06));
            let polar = 1.0 - smoothstep(v.min(1.0 - v) / 0.08);
            let sampled = self.at(u, v).blend(self.at(1.0 - u, v), join);
            return sampled.blend(self.poles[usize::from(v > 0.5)], polar);
        }
        // Azimuthal equidistant projection. The wrap angle runs from the
        // center of the map to each corner of the square selection.
        let tangent = x.hypot(y);
        let angle = tangent.atan2(z);
        if angle >= self.corner_angle {
            return Ground::NEUTRAL;
        }
        let projected_scale = if tangent > 1.0e-6 {
            angle * std::f32::consts::FRAC_1_SQRT_2 / (tangent * self.corner_angle)
        } else {
            0.0
        };
        let u = 0.5 + x * projected_scale;
        let v = 0.5 + y * projected_scale;
        let edge = u.min(v).min(1.0 - u).min(1.0 - v);
        Ground::NEUTRAL.blend(self.at(u, v), smoothstep(edge / 0.08))
    }
}
