use anyhow::{Result, bail};
use rayon::prelude::*;

use crate::spec::GenerationSpec;

#[derive(Debug, Clone)]
pub struct HeightField {
    pub width: usize,
    pub height: usize,
    pub values_m: Vec<f32>,
    pub source: String,
}

impl HeightField {
    pub fn new(
        width: usize,
        height: usize,
        values_m: Vec<f32>,
        source: impl Into<String>,
    ) -> Result<Self> {
        if width < 2 || height < 2 {
            bail!("height field must be at least 2 by 2");
        }
        if values_m.len() != width * height {
            bail!("height field dimensions do not match its values");
        }
        if values_m.iter().any(|value| !value.is_finite()) {
            bail!("height field contains a non-finite value");
        }
        Ok(Self {
            width,
            height,
            values_m,
            source: source.into(),
        })
    }

    fn normalized_at(&self, u: f32, v: f32, minimum: f32, range: f32) -> f32 {
        ((self.elevation_m_at(u, v) - minimum) / range).max(0.0)
    }

    pub fn elevation_m_at(&self, u: f32, v: f32) -> f32 {
        let x = u.clamp(0.0, 1.0) * (self.width - 1) as f32;
        let y = v.clamp(0.0, 1.0) * (self.height - 1) as f32;
        let x0 = x.floor() as usize;
        let y0 = y.floor() as usize;
        let x1 = (x0 + 1).min(self.width - 1);
        let y1 = (y0 + 1).min(self.height - 1);
        let tx = x - x0 as f32;
        let ty = y - y0 as f32;
        let sample =
            |sample_x: usize, sample_y: usize| self.values_m[sample_y * self.width + sample_x];
        let bottom = sample(x0, y0) * (1.0 - tx) + sample(x1, y0) * tx;
        let top = sample(x0, y1) * (1.0 - tx) + sample(x1, y1) * tx;
        bottom * (1.0 - ty) + top * ty
    }

    pub(crate) fn range(&self) -> (f32, f32) {
        let (minimum, maximum) = self.elevation_bounds();
        (minimum, (maximum - minimum).max(1.0))
    }

    pub fn elevation_bounds(&self) -> (f32, f32) {
        let (minimum, maximum) = self
            .values_m
            .par_iter()
            .copied()
            .fold(
                || (f32::INFINITY, f32::NEG_INFINITY),
                |(minimum, maximum), value| (minimum.min(value), maximum.max(value)),
            )
            .reduce(
                || (f32::INFINITY, f32::NEG_INFINITY),
                |(left_minimum, left_maximum), (right_minimum, right_maximum)| {
                    (
                        left_minimum.min(right_minimum),
                        left_maximum.max(right_maximum),
                    )
                },
            );
        (minimum, maximum)
    }

    pub fn samples_per_piece(&self, spec: &GenerationSpec) -> usize {
        if spec.solid_model {
            return (self.width - 1).min(self.height - 1);
        }
        ((self.width - 1) / spec.columns.max(1) as usize)
            .min((self.height - 1) / spec.rows.max(1) as usize)
    }
}

pub(crate) fn height_range_for_spec(
    spec: &GenerationSpec,
    height_field: Option<&HeightField>,
) -> Option<(f32, f32)> {
    height_field.map(|field| {
        spec.elevation_datum_m
            .zip(spec.elevation_m_per_mm)
            .map(|(datum, metres_per_mm)| (datum, metres_per_mm * spec.relief_mm))
            .unwrap_or_else(|| field.range())
    })
}

pub(crate) fn validate_height_frame(
    spec: &GenerationSpec,
    height_field: Option<&HeightField>,
) -> Result<()> {
    if let (Some(field), Some(datum)) = (height_field, spec.elevation_datum_m) {
        let (minimum, _) = field.elevation_bounds();
        if minimum + 0.01 < datum {
            bail!(
                "shared elevation datum {datum:.1} m is above this tile's minimum elevation \
                 {minimum:.1} m; lower the datum and regenerate the earlier super-tile parts"
            );
        }
    }
    Ok(())
}

fn terrain_height(u: f32, v: f32, lat: f64, lon: f64) -> f32 {
    let u = u.clamp(0.0, 1.0);
    let v = v.clamp(0.0, 1.0);
    let seed_a = (lat as f32).to_radians().sin() * 1.7;
    let seed_b = (lon as f32).to_radians().cos() * 1.3;
    let ridge = ((u * 9.2 + seed_a) * 1.2).sin() * 0.19 + ((v * 7.1 - seed_b) * 1.4).cos() * 0.14;
    let folds = ((u * 3.8 + v * 5.6 + seed_b) * std::f32::consts::PI)
        .sin()
        .abs()
        * 0.17;
    let dx = u - (0.54 + seed_b * 0.05);
    let dy = v - (0.48 + seed_a * 0.05);
    let peak = (-((dx * dx * 5.5) + (dy * dy * 7.0))).exp() * 0.63;
    (0.12 + ridge + folds + peak).clamp(0.03, 1.0)
}

pub(crate) fn normalized_height(
    height_field: Option<&HeightField>,
    range: Option<(f32, f32)>,
    u: f32,
    v: f32,
    lat: f64,
    lon: f64,
) -> f32 {
    match (height_field, range) {
        (Some(field), Some((minimum, span))) => field.normalized_at(u, v, minimum, span),
        _ => terrain_height(u, v, lat, lon),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shared_height_frame_reports_a_tile_below_its_datum() {
        let spec = GenerationSpec {
            elevation_datum_m: Some(100.0),
            elevation_m_per_mm: Some(10.0),
            ..GenerationSpec::default()
        };
        let height = HeightField::new(2, 2, vec![90.0, 110.0, 120.0, 130.0], "test").unwrap();
        let error = validate_height_frame(&spec, Some(&height))
            .unwrap_err()
            .to_string();

        assert!(error.contains("above this tile's minimum elevation 90.0 m"));
        assert!(error.contains("regenerate the earlier super-tile parts"));
    }
}
