use super::layout::Vec3;
use crate::SurfaceClass;

pub(super) const CLASSES: [SurfaceClass; 5] = [
    SurfaceClass::Rock,
    SurfaceClass::Forest,
    SurfaceClass::Snow,
    SurfaceClass::Water,
    SurfaceClass::Grass,
];

#[derive(Clone, Copy)]
pub(super) struct Ground {
    pub(super) height: f32,
    cover: [f32; 5],
    texture_fade: f32,
}

impl Ground {
    pub(super) const ZERO: Self = Self {
        height: 0.0,
        cover: [0.0; 5],
        texture_fade: 0.0,
    };
    pub(super) const NEUTRAL: Self = Self {
        height: 0.0,
        cover: [1.0, 0.0, 0.0, 0.0, 0.0],
        texture_fade: 0.0,
    };

    pub(super) fn solid(height: f32, class: usize) -> Self {
        let mut sample = Self {
            height,
            texture_fade: 1.0,
            ..Self::ZERO
        };
        sample.cover[class] = 1.0;
        sample
    }

    pub(super) fn plus(self, other: Self, weight: f32) -> Self {
        Self {
            height: self.height + other.height * weight,
            cover: std::array::from_fn(|index| self.cover[index] + other.cover[index] * weight),
            texture_fade: self.texture_fade + other.texture_fade * weight,
        }
    }

    pub(super) fn blend(self, other: Self, toward: f32) -> Self {
        Self::ZERO.plus(self, 1.0 - toward).plus(other, toward)
    }

    pub(super) fn class(self) -> SurfaceClass {
        let index = (1..self.cover.len()).fold(0, |best, index| {
            if self.cover[index] > self.cover[best] {
                index
            } else {
                best
            }
        });
        CLASSES[index]
    }

    /// Texture is a fixed field in assembled globe coordinates. Each cover
    /// has a distinct mix of ridges and bumps, with smooth amplitude changes
    /// as the raster classes blend. Water contributes exactly zero texture.
    pub(super) fn texture(
        self,
        direction: Vec3,
        radius_mm: f32,
        wavelength_mm: f32,
        amplitude: f32,
    ) -> f32 {
        let [x, y, z] = direction
            .map(|coordinate| coordinate * radius_mm * std::f32::consts::TAU / wavelength_mm);
        let rocks = (x.sin() * y.sin() * z.cos()).abs();
        let bumps = (1.0 + (x.cos() + (y + 0.37 * x).cos() + (z - 0.4 * y).cos()) / 3.0) * 0.5;
        let ridges = (1.0 + (0.56 * x + 0.64 * y + 0.52 * z).sin()) * 0.5;
        let patterns = [rocks, bumps, bumps * 0.12, 0.0, ridges * 0.55];
        self.cover
            .into_iter()
            .zip(patterns)
            .map(|(weight, pattern)| weight * pattern)
            .sum::<f32>()
            * amplitude
            * self.texture_fade
    }
}

pub(super) fn smoothstep(t: f32) -> f32 {
    let t = t.clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}
