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

    /// Replaces isolated wild readings with their neighbourhood median.
    ///
    /// Published elevation tiles carry occasional bad pixels, and they cluster
    /// on the seams of the source mosaic and on the boundary between a flat
    /// water surface and the land around it. One of them is enough to ruin a
    /// print: relief is stretched over the field's whole range, so a single
    /// reading thousands of metres out squeezes every real hill into a
    /// fraction of the intended height, and punches a needle hole through the
    /// base on the way.
    ///
    /// See [`is_spike`] for what counts as a bad reading, and
    /// [`SPIKE_NEIGHBOUR_ALLOWANCE`] for what the pass can and cannot reach.
    /// Each pass replaces the readings it flags with the median of their eight
    /// neighbours, and passes repeat until nothing is flagged: a reading with
    /// too many bad neighbours cannot be judged until some of them are healed,
    /// so clearing the end of a run brings the next sample within reach.
    pub fn despike(&mut self, sample_spacing_m: f32) -> DespikeReport {
        let threshold_m = SPIKE_SLOPE_FACTOR * sample_spacing_m.max(f32::MIN_POSITIVE);
        let mut report = DespikeReport {
            threshold_m,
            ..DespikeReport::default()
        };
        for _ in 0..SPIKE_MAX_PASSES {
            let replacements: Vec<(usize, f32, f32)> = (0..self.values_m.len())
                .into_par_iter()
                .filter_map(|index| {
                    let ring = self.neighbour_ring(index % self.width, index / self.width);
                    let value = self.values_m[index];
                    is_spike(value, ring, threshold_m)
                        .map(|distance| (index, ring_median(ring), distance))
                })
                .collect();
            if replacements.is_empty() {
                break;
            }
            report.passes += 1;
            report.replaced += replacements.len();
            for (index, median, distance) in replacements {
                self.values_m[index] = median;
                report.widest_distance_m = report.widest_distance_m.max(distance);
            }
        }
        report
    }

    /// The eight readings around one sample, sorted from lowest to highest.
    ///
    /// The border is folded back rather than skipped or held flat. Skipping it
    /// is not an option: a seam in the source mosaic runs straight off the edge
    /// of the field, and leaving the outermost row unchecked keeps the whole
    /// fault alive. Holding the edge flat is worse than useless, because a
    /// sample on the border would then find itself among its own neighbours
    /// and vouch for its own reading. Folding back reflects across the edge,
    /// so a border sample is judged against real ground one step in.
    fn neighbour_ring(&self, column: usize, row: usize) -> [f32; 8] {
        let mut ring = [0.0f32; 8];
        let mut index = 0;
        for row_offset in -1i64..=1 {
            for column_offset in -1i64..=1 {
                if row_offset == 0 && column_offset == 0 {
                    continue;
                }
                let neighbour_row = fold_back(row as i64 + row_offset, self.height);
                let neighbour_column = fold_back(column as i64 + column_offset, self.width);
                ring[index] = self.values_m[neighbour_row * self.width + neighbour_column];
                index += 1;
            }
        }
        ring.sort_by(f32::total_cmp);
        ring
    }

    pub fn samples_per_piece(&self, spec: &GenerationSpec) -> usize {
        if spec.solid_model {
            return (self.width - 1).min(self.height - 1);
        }
        ((self.width - 1) / spec.columns.max(1) as usize)
            .min((self.height - 1) / spec.rows.max(1) as usize)
    }
}

/// How far a reading has to stand clear of its neighbours before it counts as
/// bad, as a multiple of the distance between samples.
///
/// Measuring against the sample spacing rather than a fixed number of metres
/// is what keeps the pass honest across scales. A drop of 150 m between
/// neighbouring samples is a sheer cliff on a 20 km model and an ordinary
/// hillside on a 160 km one, so a fixed limit that catches the fault up close
/// would shave real valleys on the wide view. At this factor the pass only
/// touches readings standing off at better than 80 degrees.
///
/// The factor is set well clear of real ground rather than at the edge of it.
/// Measured over four fields — a 20 km view holding a known bad seam and three
/// clean controls at 2 m, 20 m and 156 m sample spacing — the clean fields
/// first give up a single sample at a factor of about one. Eight leaves that
/// margin untouched and still catches every reading deep enough to matter.
const SPIKE_SLOPE_FACTOR: f32 = 8.0;

/// Reflects a coordinate that stepped off the edge back inside, so the sample
/// on the border is never counted among its own neighbours.
fn fold_back(coordinate: i64, length: usize) -> usize {
    let last = length as i64 - 1;
    if coordinate < 0 {
        (-coordinate).min(last) as usize
    } else if coordinate > last {
        (2 * last - coordinate).max(0) as usize
    } else {
        coordinate as usize
    }
}

/// How many bad neighbours a sample may have and still be recognised.
///
/// The test compares against the third-lowest of the eight neighbours, not the
/// lowest and not the median. Against the lowest, a bad reading with a bad
/// neighbour hides: the pair vouch for each other and nothing is ever flagged,
/// so a seam one sample wide — where every reading has a bad neighbour above
/// and below — survives untouched. Against the median, half the ring may be
/// bad before the test notices, which is loose enough to start flagging the
/// floors of real valleys. Third-lowest tolerates the seam and the shoreline
/// curve, both of which leave a sample with two bad neighbours, and where a
/// junction of the two leaves more, healing the arms drops the rest within
/// reach on the following pass.
///
/// What this deliberately gives up: a solid block of bad readings three or more
/// samples wide. Every sample along its edge has five bad neighbours, so none is
/// ever flagged and no later pass can work inward. Reaching those would mean
/// judging against the median, and against a clean 2 m field that starts
/// flagging real ground at a bar six times lower — too near the setting here to
/// trade for the reach. Damage that broad is a hole in the source rather than a
/// stray reading, and filling it with the median of its own bad neighbours would
/// not recover the ground anyway.
///
/// In practice that limit is met by sampling the model finer than the tiles: a
/// close view spaces its samples below the width of a source pixel, so one bad
/// pixel covers several samples at once and reads as a block. A 4 km view over a
/// damaged shoreline recovers only part of the way for this reason. Raising the
/// allowance is not the answer — measured, it grinds such a cluster down by
/// repeated smearing, rewriting tens of thousands of samples and still falling
/// short of the real ground. Reaching that case properly means repairing tile
/// pixels before they are interpolated, where a stray reading is one pixel wide
/// whatever the model asks for.
const SPIKE_NEIGHBOUR_ALLOWANCE: usize = 2;

/// A cap so a pathological field cannot spin. Real ones settle in a few
/// passes; the fault this pass was written for settles in two.
const SPIKE_MAX_PASSES: usize = 32;

/// How far `value` stands clear of its neighbours, if far enough to be bad.
///
/// Symmetric on purpose. A reading far too high is the same kind of fault as
/// one far too low and does the same damage from the other end of the range,
/// stretching the relief downward instead of upward.
fn is_spike(value: f32, ring: [f32; 8], threshold_m: f32) -> Option<f32> {
    let low = ring[SPIKE_NEIGHBOUR_ALLOWANCE];
    let high = ring[ring.len() - 1 - SPIKE_NEIGHBOUR_ALLOWANCE];
    let distance = (low - value).max(value - high);
    (distance >= threshold_m).then_some(distance)
}

/// The middle of the eight neighbours. Taken from a sorted ring, so a bad
/// neighbour that survived this pass cannot drag the replacement with it the
/// way an average would.
fn ring_median(ring: [f32; 8]) -> f32 {
    (ring[3] + ring[4]) / 2.0
}

/// What [`HeightField::despike`] changed.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct DespikeReport {
    /// How many readings were replaced, counted across every pass.
    pub replaced: usize,
    /// How many passes ran before the field settled.
    pub passes: usize,
    /// The widest gap between a replaced reading and its neighbours.
    pub widest_distance_m: f32,
    /// How far a reading had to stand clear of its neighbours to be replaced.
    /// Carried here so callers can report the bar without repeating how it is
    /// worked out.
    pub threshold_m: f32,
}

impl DespikeReport {
    pub fn is_empty(&self) -> bool {
        self.replaced == 0
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

    /// A field of gentle ground at the given size, tilted so no two samples
    /// read alike and nothing is a plateau the pass could mistake for a fault.
    fn rolling_ground(width: usize, height: usize) -> HeightField {
        let values = (0..width * height)
            .map(|index| {
                let column = (index % width) as f32;
                let row = (index / width) as f32;
                500.0 + column * 0.5 + row * 0.25
            })
            .collect();
        HeightField::new(width, height, values, "test").unwrap()
    }

    #[test]
    fn despiking_replaces_a_lone_bad_reading_and_leaves_the_rest_alone() {
        let mut field = rolling_ground(9, 9);
        let untouched = field.values_m.clone();
        let index = 4 * 9 + 4;
        field.values_m[index] = -6827.9;

        let report = field.despike(20.0);

        assert_eq!(report.replaced, 1);
        assert_eq!(report.passes, 1);
        assert_eq!(report.threshold_m, 160.0);
        assert!(report.widest_distance_m > 7000.0);
        // The replacement is its neighbours' median, and no other sample moved.
        assert!((field.values_m[index] - untouched[index]).abs() < 1.0);
        for (position, (after, before)) in field.values_m.iter().zip(&untouched).enumerate() {
            if position != index {
                assert_eq!(after, before, "sample {position} moved");
            }
        }
    }

    /// The fault this pass was written for: a seam one sample wide running the
    /// full height of the field, so every bad reading has a bad neighbour above
    /// and below it. A test against the lowest neighbour would clear the whole
    /// column, since each member vouches for the next.
    #[test]
    fn despiking_clears_a_seam_one_sample_wide() {
        let mut field = rolling_ground(9, 9);
        for row in 0..9 {
            field.values_m[row * 9 + 3] = -5000.0;
        }

        let report = field.despike(20.0);

        assert_eq!(report.replaced, 9);
        let (minimum, _) = field.elevation_bounds();
        assert!(minimum > 400.0, "seam survived, floor is {minimum} m");
    }

    /// Where a seam grows a stub, the two samples at the join each have three
    /// bad neighbours and cannot be judged on the first pass. Healing the ends
    /// brings them within reach on the second, which is why the pass repeats.
    #[test]
    fn despiking_works_inward_over_repeated_passes() {
        let mut field = rolling_ground(11, 11);
        for column in 4..7 {
            field.values_m[5 * 11 + column] = -4000.0;
        }
        field.values_m[4 * 11 + 5] = -4000.0;

        let report = field.despike(20.0);

        assert_eq!(report.replaced, 4);
        assert!(report.passes >= 2, "settled in {} passes", report.passes);
        let (minimum, _) = field.elevation_bounds();
        assert!(minimum > 400.0, "readings survived, floor is {minimum} m");
    }

    /// Pins the limit named on [`SPIKE_NEIGHBOUR_ALLOWANCE`]: a solid block
    /// three samples wide is left alone, because every sample on its edge has
    /// five bad neighbours. Should a future change reach these, this test is the
    /// place to record it.
    #[test]
    fn despiking_leaves_a_solid_wide_block_alone() {
        let mut field = rolling_ground(11, 11);
        for row in 3..8 {
            for column in 4..7 {
                field.values_m[row * 11 + column] = -4000.0;
            }
        }
        let before = field.values_m.clone();

        let report = field.despike(20.0);

        assert!(report.is_empty());
        assert_eq!(field.values_m, before);
    }

    /// A bad reading on the outermost row still has to be caught: the seam that
    /// prompted this pass runs straight off the edge of the field, and leaving
    /// the border out kept the fault alive.
    #[test]
    fn despiking_reaches_the_border() {
        let mut field = rolling_ground(9, 9);
        field.values_m[0] = -3000.0;
        let last = field.values_m.len() - 1;
        field.values_m[last] = -3000.0;

        let report = field.despike(20.0);

        assert_eq!(report.replaced, 2);
        let (minimum, _) = field.elevation_bounds();
        assert!(
            minimum > 400.0,
            "border reading survived, floor is {minimum} m"
        );
    }

    /// Symmetric: a reading far too high does the same damage from the other
    /// end of the range.
    #[test]
    fn despiking_replaces_a_reading_far_too_high() {
        let mut field = rolling_ground(9, 9);
        field.values_m[4 * 9 + 4] = 9000.0;

        let report = field.despike(20.0);

        assert_eq!(report.replaced, 1);
        let (_, maximum) = field.elevation_bounds();
        assert!(
            maximum < 600.0,
            "high reading survived, ceiling is {maximum} m"
        );
    }

    /// The bar scales with the distance between samples, so the same drop is a
    /// fault on a close view and ordinary ground on a wide one. Without this a
    /// fixed limit would shave real valleys off the wide view.
    #[test]
    fn the_bar_follows_the_sample_spacing() {
        let steep = |spacing_m: f32| {
            let mut field = rolling_ground(9, 9);
            field.values_m[4 * 9 + 4] -= 300.0;
            field.despike(spacing_m).replaced
        };

        // 300 m between neighbouring samples 20 m apart is a fault.
        assert_eq!(steep(20.0), 1);
        // The same 300 m across 160 m of ground is a hillside.
        assert_eq!(steep(160.0), 0);
    }

    #[test]
    fn despiking_leaves_ordinary_ground_untouched() {
        let mut field = rolling_ground(16, 16);
        let untouched = field.values_m.clone();

        let report = field.despike(20.0);

        assert!(report.is_empty());
        assert_eq!(report.passes, 0);
        assert_eq!(field.values_m, untouched);
    }
}
