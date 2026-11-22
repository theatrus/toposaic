use super::layout::{Face, Pose, Vec3, dot};
use crate::HeightField;

/// Square map selection on an azimuthal equidistant projection. The wrap
/// angle is measured from the globe's center-facing point to a map corner.
pub(super) struct Region<'a> {
    field: &'a HeightField,
    axes: Pose,
    corner_angle: f32,
    minimum: f32,
    relief_per_metre: f32,
}

impl<'a> Region<'a> {
    pub(super) fn new(
        field: &'a HeightField,
        center: &Face,
        wrap_degrees: f32,
        relief_mm: f32,
    ) -> Self {
        let (minimum, maximum) = field.elevation_bounds();
        Self {
            field,
            axes: center.pose,
            corner_angle: wrap_degrees.to_radians(),
            minimum,
            relief_per_metre: relief_mm / (maximum - minimum).max(1.0),
        }
    }

    pub(super) fn height(&self, direction: Vec3) -> f32 {
        let x = dot(direction, self.axes.x_axis);
        let y = dot(direction, self.axes.y_axis);
        let tangent = x.hypot(y);
        let angle = tangent.atan2(dot(direction, self.axes.z_axis));
        if angle >= self.corner_angle {
            return 0.0;
        }
        let projected_scale = if tangent > 1.0e-6 {
            angle * std::f32::consts::FRAC_1_SQRT_2 / (tangent * self.corner_angle)
        } else {
            0.0
        };
        let u = 0.5 + x * projected_scale;
        let v = 0.5 + y * projected_scale;
        let edge = u.min(v).min(1.0 - u).min(1.0 - v);
        if edge <= 0.0 {
            return 0.0;
        }
        let fade = (edge / 0.08).min(1.0);
        (self.field.elevation_m_at(u, v) - self.minimum)
            * self.relief_per_metre
            * fade
            * fade
            * (3.0 - 2.0 * fade)
    }
}
