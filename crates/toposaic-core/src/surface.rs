use std::collections::VecDeque;

use anyhow::{Result, bail};
use rayon::prelude::*;

use crate::heightfield::HeightField;
use crate::mesh::{distance_squared, point_in_polygon};
use crate::spec::{SteepForestTarget, SurfaceClass};

const VECTOR_BUCKET_COLUMNS: usize = 32;
const VECTOR_BUCKET_COUNT: usize = VECTOR_BUCKET_COLUMNS * VECTOR_BUCKET_COLUMNS;
pub(crate) const ROAD_VECTOR_STEP_MM: f32 = 0.25;

// Indicator-kriging border smoothing. The estimator interpolates per-class
// 0/1 indicators from the recovered native-resolution land-cover grid with
// local ordinary kriging (a 4 by 4 node neighbourhood and a spherical
// variogram), then keeps the class with the largest estimate. On a regular
// grid the kriging weights depend only on the fractional position inside a
// native cell, so they are solved once per quantized offset and applied as
// fixed stencils.
/// Nodes per axis in the kriging neighbourhood.
const KRIGING_NEIGHBORHOOD: usize = 4;
/// Quantization steps per axis for the fractional-offset weight table.
const KRIGING_OFFSET_STEPS: usize = 16;
/// Below this many samples per native cell the raster already resolves the
/// source borders and smoothing would only blur real data.
const MINIMUM_NATIVE_CELL_SAMPLES: f32 = 1.5;
/// Below this share of snow samples a scene has no snowcap, so there is no
/// snowline to estimate.
const SNOWLINE_MINIMUM_SNOW_FRACTION: f32 = 0.01;
/// Snowline percentile of the snow-sample elevations. A low percentile
/// rather than the minimum keeps the estimate robust: a few misclassified
/// low-altitude snow pixels (a white roof, a glacier terminus in shadow)
/// would drag a minimum-based snowline far below the real one, while the
/// 10th percentile ignores them and still sits near the bottom of the
/// genuine snowcap.
const SNOWLINE_PERCENTILE: usize = 10;

#[derive(Debug, Clone)]
pub struct SurfaceField {
    pub width: usize,
    pub height: usize,
    pub classes: Vec<SurfaceClass>,
    pub source: String,
    pub(crate) base_classes: Vec<SurfaceClass>,
    pub(crate) vector_lines: Vec<VectorSurfaceLine>,
    pub(crate) vector_areas: Vec<VectorSurfaceArea>,
    vector_line_buckets: Vec<Vec<LineBucketEntry>>,
    vector_area_buckets: Vec<Vec<usize>>,
}

/// One line's presence in one bucket: only the segments whose padded
/// bounding box touches the bucket, as ranges into `points_mm` windows.
/// Long resampled lines have hundreds of segments but only a handful near
/// any one bucket, so queries walk a short subset instead of the whole
/// polyline.
#[derive(Debug, Clone)]
struct LineBucketEntry {
    line_index: usize,
    /// Half-open ranges of segment indices; segment `i` spans
    /// `points_mm[i]..=points_mm[i + 1]`.
    segment_ranges: Vec<(u32, u32)>,
}

#[derive(Debug, Clone)]
pub(crate) struct VectorSurfaceLine {
    pub(crate) points_mm: Vec<[f32; 2]>,
    pub(crate) width_mm: f32,
    model_width_mm: f32,
    model_height_mm: f32,
    pub(crate) class: SurfaceClass,
    pub(crate) bridge_elevations_m: Option<[f32; 2]>,
    length_mm: f32,
}

#[derive(Debug, Clone)]
pub(crate) struct VectorSurfaceArea {
    pub(crate) points: Vec<[f32; 2]>,
    pub(crate) class: Option<SurfaceClass>,
    pub(crate) building_height_m: f32,
}

#[derive(Clone, Copy)]
pub(crate) struct SurfaceSample {
    pub(crate) class: SurfaceClass,
    pub(crate) building_height_m: f32,
}

/// Per-class steep-slope gate configuration for `demote_steep_classes`.
/// A `None` limit turns that class's gate off.
#[derive(Debug, Clone, Copy)]
pub struct SlopeGates {
    /// Demote forest steeper than this many degrees.
    pub forest_limit_degrees: Option<f32>,
    /// What demoted steep forest becomes: rock everywhere, or snow above
    /// the snowline. Ignored when the forest gate is off.
    pub steep_forest_target: SteepForestTarget,
    /// Demote snow steeper than this many degrees to rock. Applies after
    /// the forest gate, so it also gates forest just demoted to snow.
    pub snow_limit_degrees: Option<f32>,
}

/// How many samples the steep-slope gates reclassified, split by the class
/// they left and the class they became.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct SlopeGateDemotion {
    pub forest_to_rock: usize,
    pub forest_to_snow: usize,
    pub snow_to_rock: usize,
}

impl SlopeGateDemotion {
    pub fn total(self) -> usize {
        self.forest_to_rock + self.forest_to_snow + self.snow_to_rock
    }

    fn add(self, other: Self) -> Self {
        Self {
            forest_to_rock: self.forest_to_rock + other.forest_to_rock,
            forest_to_snow: self.forest_to_snow + other.forest_to_snow,
            snow_to_rock: self.snow_to_rock + other.snow_to_rock,
        }
    }
}

/// Land-cover classes on the source's own pixel lattice, plus the affine
/// map from sample-raster pixel indices to native grid coordinates:
/// `grid = sample_index * scale + offset`, with node centres at integer
/// grid coordinates. Row 0 is the southmost row, matching the sample
/// raster. Unlike the grid `smooth_class_borders` recovers from the
/// upsampled raster, this carries the true geographic phase of the source
/// pixels, so borders land where the source drew them instead of up to half
/// a cell away.
#[derive(Debug, Clone)]
pub struct NativeClassGrid {
    width: usize,
    height: usize,
    classes: Vec<SurfaceClass>,
    x_scale: f64,
    x_offset: f64,
    y_scale: f64,
    y_offset: f64,
}

impl NativeClassGrid {
    pub fn new(
        width: usize,
        height: usize,
        classes: Vec<SurfaceClass>,
        x_scale: f64,
        x_offset: f64,
        y_scale: f64,
        y_offset: f64,
    ) -> Result<Self> {
        if width < 2 || height < 2 {
            bail!("native class grid must be at least 2 by 2");
        }
        if classes.len() != width * height {
            bail!("native class grid dimensions do not match its values");
        }
        if !(x_scale.is_finite()
            && x_offset.is_finite()
            && y_scale.is_finite()
            && y_offset.is_finite())
            || x_scale <= 0.0
            || y_scale <= 0.0
        {
            bail!("native class grid mapping must be finite with positive scales");
        }
        Ok(Self {
            width,
            height,
            classes,
            x_scale,
            x_offset,
            y_scale,
            y_offset,
        })
    }

    /// Samples per native cell along the denser axis, as implied by the
    /// mapping; the counterpart of the ratio `smooth_class_borders` derives
    /// from the ground span.
    fn cell_samples(&self) -> f64 {
        (1.0 / self.x_scale).max(1.0 / self.y_scale)
    }
}

impl SurfaceField {
    pub fn new(
        width: usize,
        height: usize,
        classes: Vec<SurfaceClass>,
        source: impl Into<String>,
    ) -> Result<Self> {
        if width < 2 || height < 2 {
            bail!("surface field must be at least 2 by 2");
        }
        if classes.len() != width * height {
            bail!("surface field dimensions do not match its values");
        }
        Ok(Self {
            width,
            height,
            base_classes: classes.clone(),
            classes,
            source: source.into(),
            vector_lines: Vec::new(),
            vector_areas: Vec::new(),
            vector_line_buckets: vec![Vec::new(); VECTOR_BUCKET_COUNT],
            vector_area_buckets: vec![Vec::new(); VECTOR_BUCKET_COUNT],
        })
    }

    /// Captures the outer raster ring in clockwise order. Vector overlays are
    /// not included because their geographic paths already cross tile seams.
    pub fn raster_edge_classes(&self) -> Vec<SurfaceClass> {
        let mut edges = Vec::with_capacity(self.width * 2 + self.height.saturating_sub(2) * 2);
        edges.extend_from_slice(&self.classes[..self.width]);
        for row in 1..self.height - 1 {
            edges.push(self.classes[row * self.width + self.width - 1]);
        }
        edges.extend(
            self.classes[(self.height - 1) * self.width..]
                .iter()
                .rev()
                .copied(),
        );
        for row in (1..self.height - 1).rev() {
            edges.push(self.classes[row * self.width]);
        }
        edges
    }

    /// Restores a captured raster ring after tile-local filtering. Adjacent
    /// tiles sample the same raw ring, so this keeps their final material seam
    /// equal while allowing all interior smoothing and slope gates to run.
    pub fn restore_raster_edge_classes(&mut self, edges: &[SurfaceClass]) -> Result<()> {
        let expected = self.width * 2 + self.height.saturating_sub(2) * 2;
        if edges.len() != expected {
            bail!("surface edge ring does not match field dimensions");
        }
        let mut cursor = 0;
        for column in 0..self.width {
            self.set_raster_class(column, 0, edges[cursor]);
            cursor += 1;
        }
        for row in 1..self.height - 1 {
            self.set_raster_class(self.width - 1, row, edges[cursor]);
            cursor += 1;
        }
        for column in (0..self.width).rev() {
            self.set_raster_class(column, self.height - 1, edges[cursor]);
            cursor += 1;
        }
        for row in (1..self.height - 1).rev() {
            self.set_raster_class(0, row, edges[cursor]);
            cursor += 1;
        }
        Ok(())
    }

    fn set_raster_class(&mut self, column: usize, row: usize, class: SurfaceClass) {
        let index = row * self.width + column;
        self.classes[index] = class;
        self.base_classes[index] = class;
    }

    pub fn filter_small_patches(&mut self, print_width_mm: f32, minimum_patch_mm: f32) {
        let cells_across =
            minimum_patch_mm / print_width_mm.max(f32::EPSILON) * (self.width - 1) as f32;
        let minimum_cells = (std::f32::consts::PI * (cells_across * 0.5).powi(2))
            .ceil()
            .max(2.0) as usize;
        for _ in 0..2 {
            self.filter_components_smaller_than(minimum_cells);
        }
        self.base_classes.clone_from(&self.classes);
    }

    /// Reclassifies forest and snow wherever the local ground slope exceeds
    /// their per-class limits, using central differences on the height
    /// field over the real ground spacing. Steep forest becomes rock, or —
    /// with `SteepForestTarget::Snow` — snow when it sits above the
    /// snowline estimated from the samples already classed snow (and rock
    /// below it, or everywhere when the scene has no snowcap). Steep snow
    /// becomes rock; the snow gate runs after the forest gate, so a face
    /// steeper than both limits ends as rock even with the snow target.
    /// The snowline always comes from the pre-demotion snow samples, and
    /// each sample's slope is computed once and shared by both gates.
    /// Applies to both the working and the base class rasters, so call it
    /// before painting vector overlays. Returns how many samples were
    /// reclassified, split by the class they left and became.
    pub fn demote_steep_classes(
        &mut self,
        height_field: &HeightField,
        ground_span_m: f32,
        gates: SlopeGates,
    ) -> SlopeGateDemotion {
        if !ground_span_m.is_finite() || ground_span_m <= 0.0 {
            return SlopeGateDemotion::default();
        }
        if gates.forest_limit_degrees.is_none() && gates.snow_limit_degrees.is_none() {
            return SlopeGateDemotion::default();
        }
        // The snowline comes from the classes before any demotion, so the
        // demoted samples themselves cannot move it.
        let snowline_m = (gates.forest_limit_degrees.is_some()
            && gates.steep_forest_target == SteepForestTarget::Snow)
            .then(|| self.snowline_m(height_field))
            .flatten();
        let tan_forest_limit = gates
            .forest_limit_degrees
            .map(|degrees| degrees.to_radians().tan());
        let tan_snow_limit = gates
            .snow_limit_degrees
            .map(|degrees| degrees.to_radians().tan());
        let width = self.width;
        let du = 1.0 / (width - 1) as f32;
        let dv = 1.0 / (self.height - 1) as f32;
        let demoted = self
            .classes
            .par_chunks_mut(width)
            .enumerate()
            .map(|(y, row)| {
                let v = y as f32 * dv;
                let v0 = (v - dv).max(0.0);
                let v1 = (v + dv).min(1.0);
                let mut demoted = SlopeGateDemotion::default();
                for (x, class) in row.iter_mut().enumerate() {
                    // Only forest under the forest gate and snow under the
                    // snow gate need a slope; everything else keeps its
                    // class without touching the height field.
                    let gated = match *class {
                        SurfaceClass::Forest => tan_forest_limit.is_some(),
                        SurfaceClass::Snow => tan_snow_limit.is_some(),
                        _ => false,
                    };
                    if !gated {
                        continue;
                    }
                    let u = x as f32 * du;
                    let u0 = (u - du).max(0.0);
                    let u1 = (u + du).min(1.0);
                    let rise_x =
                        height_field.elevation_m_at(u1, v) - height_field.elevation_m_at(u0, v);
                    let rise_y =
                        height_field.elevation_m_at(u, v1) - height_field.elevation_m_at(u, v0);
                    let gradient_x = rise_x / ((u1 - u0) * ground_span_m);
                    let gradient_y = rise_y / ((v1 - v0) * ground_span_m);
                    let gradient = gradient_x.hypot(gradient_y);
                    // Forest pass first: its snow output feeds the snow
                    // gate below, exactly as if the passes ran in
                    // sequence over the whole raster.
                    if *class == SurfaceClass::Forest
                        && tan_forest_limit.is_some_and(|limit| gradient > limit)
                    {
                        *class = match snowline_m {
                            Some(snowline) if height_field.elevation_m_at(u, v) >= snowline => {
                                demoted.forest_to_snow += 1;
                                SurfaceClass::Snow
                            }
                            _ => {
                                demoted.forest_to_rock += 1;
                                SurfaceClass::Rock
                            }
                        };
                    }
                    if *class == SurfaceClass::Snow
                        && tan_snow_limit.is_some_and(|limit| gradient > limit)
                    {
                        demoted.snow_to_rock += 1;
                        *class = SurfaceClass::Rock;
                    }
                }
                demoted
            })
            .reduce(SlopeGateDemotion::default, SlopeGateDemotion::add);
        if demoted.total() > 0 {
            self.base_classes.clone_from(&self.classes);
        }
        demoted
    }

    /// Estimates the snowline as the `SNOWLINE_PERCENTILE`th percentile of
    /// the elevations of samples already classed snow. `None` when snow
    /// covers less than `SNOWLINE_MINIMUM_SNOW_FRACTION` of the raster: a
    /// handful of stray white pixels is not a snowcap and defines no line.
    fn snowline_m(&self, height_field: &HeightField) -> Option<f32> {
        let width = self.width;
        let mut elevations = self
            .classes
            .par_iter()
            .enumerate()
            .filter(|(_, class)| **class == SurfaceClass::Snow)
            .map(|(index, _)| {
                let u = (index % width) as f32 / (width - 1) as f32;
                let v = (index / width) as f32 / (self.height - 1) as f32;
                height_field.elevation_m_at(u, v)
            })
            .collect::<Vec<_>>();
        if (elevations.len() as f32) < self.classes.len() as f32 * SNOWLINE_MINIMUM_SNOW_FRACTION {
            return None;
        }
        elevations.sort_unstable_by(f32::total_cmp);
        Some(elevations[(elevations.len() - 1) * SNOWLINE_PERCENTILE / 100])
    }

    /// Whether `smooth_class_borders` would actually redraw this raster:
    /// finite positive inputs and a raster meaningfully finer than the
    /// source. Callers use this to skip fetching native-resolution
    /// land-cover data when smoothing would no-op anyway (wide spans).
    pub fn class_border_smoothing_applies(
        &self,
        native_resolution_m: f32,
        ground_span_m: f32,
    ) -> bool {
        if !native_resolution_m.is_finite()
            || !ground_span_m.is_finite()
            || native_resolution_m <= 0.0
            || ground_span_m <= 0.0
        {
            return false;
        }
        let cell_samples_x = native_resolution_m * (self.width - 1) as f32 / ground_span_m;
        let cell_samples_y = native_resolution_m * (self.height - 1) as f32 / ground_span_m;
        cell_samples_x.max(cell_samples_y) >= MINIMUM_NATIVE_CELL_SAMPLES
    }

    /// Redraws raster class borders by ordinary kriging of per-class
    /// indicators. `native_resolution_m` is the ground resolution of the
    /// land-cover source (10 m for ESA WorldCover) and `ground_span_m` the
    /// ground distance the raster covers, so the method can recover the
    /// native grid that nearest-neighbour sampling upscaled.
    /// `range_cells` is the variogram range in native cells: how far a
    /// border bends to follow surrounding data. `nugget` is the variogram
    /// nugget as a fraction of the sill: higher values damp staircase phase
    /// noise but blur single-cell features. A no-op when the raster is not
    /// meaningfully finer than the source. Call before painting vector
    /// overlays; both class rasters are replaced.
    ///
    /// The recovered grid can sit up to half a cell off the true source
    /// lattice; prefer `smooth_class_borders_with_native` when the true
    /// native window is available, and keep this as the fallback when it is
    /// not (for example when the source data cannot be re-read).
    pub fn smooth_class_borders(
        &mut self,
        native_resolution_m: f32,
        ground_span_m: f32,
        range_cells: f32,
        nugget: f32,
    ) {
        if !self.class_border_smoothing_applies(native_resolution_m, ground_span_m)
            || !range_cells.is_finite()
            || range_cells <= 0.0
            || !nugget.is_finite()
            || nugget < 0.0
        {
            return;
        }
        let cell_samples_x = native_resolution_m * (self.width - 1) as f32 / ground_span_m;
        let cell_samples_y = native_resolution_m * (self.height - 1) as f32 / ground_span_m;
        let (native_width, native_height) =
            recovered_native_dimensions(self.width, self.height, cell_samples_x, cell_samples_y);
        // Nearest-sample downsampling recovers the native land-cover values
        // up to half a cell of phase, because the raster itself is a
        // nearest-neighbour upsample of those values.
        let native = (0..native_height)
            .flat_map(|node_y| {
                let y = ((node_y * (self.height - 1)) as f32 / (native_height - 1) as f32).round()
                    as usize;
                (0..native_width).map(move |node_x| (node_x, y))
            })
            .map(|(node_x, y)| {
                let x = ((node_x * (self.width - 1)) as f32 / (native_width - 1) as f32).round()
                    as usize;
                self.base_classes[y * self.width + x]
            })
            .collect::<Vec<_>>();
        // The recovered nodes span the raster exactly, so the mapping from
        // sample indices to grid coordinates is a pure scale with no phase.
        self.krige_class_borders(
            &native,
            native_width,
            native_height,
            (native_width - 1) as f64 / (self.width - 1) as f64,
            0.0,
            (native_height - 1) as f64 / (self.height - 1) as f64,
            0.0,
            range_cells,
            nugget,
        );
    }

    /// Like `smooth_class_borders`, but kriges from the source's true pixel
    /// lattice instead of a grid recovered from the upsampled raster, so
    /// borders keep the source's geographic phase. A no-op under the same
    /// conditions: invalid parameters, or fewer than
    /// `MINIMUM_NATIVE_CELL_SAMPLES` samples per native cell (as implied by
    /// the grid's mapping).
    pub fn smooth_class_borders_with_native(
        &mut self,
        native: &NativeClassGrid,
        range_cells: f32,
        nugget: f32,
    ) {
        if !range_cells.is_finite()
            || range_cells <= 0.0
            || !nugget.is_finite()
            || nugget < 0.0
            || native.cell_samples() < f64::from(MINIMUM_NATIVE_CELL_SAMPLES)
        {
            return;
        }
        self.krige_class_borders(
            &native.classes,
            native.width,
            native.height,
            native.x_scale,
            native.x_offset,
            native.y_scale,
            native.y_offset,
            range_cells,
            nugget,
        );
    }

    /// The shared kriging pass: re-estimates every sample from the node
    /// classes of a regular grid, where `grid = sample_index * scale +
    /// offset` maps sample pixels onto grid coordinates with node centres
    /// at integers. Fractional positions are quantized to the
    /// `KRIGING_OFFSET_STEPS` stencil table; with the true lattice the
    /// offsets come from real geography, so they are clamped into the cell
    /// before quantizing (edge samples clamped to the border cell can
    /// otherwise fall outside it). One table step is 1/16 of a native cell
    /// — about 0.6 m of ground for 10 m sources — well under both the
    /// sample spacing wherever smoothing runs and any visible border
    /// movement, so the existing table resolution needs no extension.
    #[expect(clippy::too_many_arguments)]
    fn krige_class_borders(
        &mut self,
        native: &[SurfaceClass],
        native_width: usize,
        native_height: usize,
        x_scale: f64,
        x_offset: f64,
        y_scale: f64,
        y_offset: f64,
        range_cells: f32,
        nugget: f32,
    ) {
        let weights = kriging_weight_table(range_cells as f64, nugget as f64);
        let width = self.width;
        let mut smoothed = vec![SurfaceClass::Rock; width * self.height];
        smoothed
            .par_chunks_mut(width)
            .enumerate()
            .for_each(|(y, row)| {
                let grid_y = y as f64 * y_scale + y_offset;
                let cell_y = (grid_y.floor() as isize).clamp(0, native_height as isize - 2);
                let offset_y = ((grid_y - cell_y as f64).clamp(0.0, 1.0)
                    * KRIGING_OFFSET_STEPS as f64)
                    .round() as usize;
                for (x, sample) in row.iter_mut().enumerate() {
                    let grid_x = x as f64 * x_scale + x_offset;
                    let cell_x = (grid_x.floor() as isize).clamp(0, native_width as isize - 2);
                    let offset_x = ((grid_x - cell_x as f64).clamp(0.0, 1.0)
                        * KRIGING_OFFSET_STEPS as f64)
                        .round() as usize;
                    let stencil = &weights[offset_y * (KRIGING_OFFSET_STEPS + 1) + offset_x];
                    let mut scores = [0.0_f32; SurfaceClass::ALL.len()];
                    for node_y in 0..KRIGING_NEIGHBORHOOD {
                        let native_y = (cell_y + node_y as isize - 1)
                            .clamp(0, native_height as isize - 1)
                            as usize;
                        for node_x in 0..KRIGING_NEIGHBORHOOD {
                            let native_x = (cell_x + node_x as isize - 1)
                                .clamp(0, native_width as isize - 1)
                                as usize;
                            let class = native[native_y * native_width + native_x];
                            scores[class.material_index() as usize] +=
                                stencil[node_y * KRIGING_NEIGHBORHOOD + node_x];
                        }
                    }
                    *sample = SurfaceClass::ALL
                        .into_iter()
                        .zip(scores)
                        .max_by(|first, second| first.1.total_cmp(&second.1))
                        .map(|(class, _)| class)
                        .unwrap_or(SurfaceClass::Rock);
                }
            });
        self.base_classes.clone_from(&smoothed);
        self.classes = smoothed;
    }

    pub fn paint_polyline(
        &mut self,
        points: &[[f32; 2]],
        print_width_mm: f32,
        line_width_mm: f32,
        class: SurfaceClass,
    ) {
        self.paint_polyline_with_bridge(points, print_width_mm, line_width_mm, class, None);
    }

    pub fn paint_bridge_polyline(
        &mut self,
        points: &[[f32; 2]],
        print_width_mm: f32,
        line_width_mm: f32,
        elevations_m: [f32; 2],
    ) {
        self.paint_bridge_polyline_as(
            points,
            print_width_mm,
            line_width_mm,
            elevations_m,
            SurfaceClass::Road,
        );
    }

    /// A tagged bridge in an explicit class, for overlays that carry their
    /// own material — a railway viaduct is structurally a road bridge with
    /// a different color.
    pub fn paint_bridge_polyline_as(
        &mut self,
        points: &[[f32; 2]],
        print_width_mm: f32,
        line_width_mm: f32,
        elevations_m: [f32; 2],
        class: SurfaceClass,
    ) {
        if elevations_m.iter().all(|value| value.is_finite()) {
            self.paint_polyline_with_bridge(
                points,
                print_width_mm,
                line_width_mm,
                class,
                Some(elevations_m),
            );
        }
    }

    fn paint_polyline_with_bridge(
        &mut self,
        points: &[[f32; 2]],
        print_width_mm: f32,
        line_width_mm: f32,
        class: SurfaceClass,
        bridge_elevations_m: Option<[f32; 2]>,
    ) {
        if points.len() < 2 {
            return;
        }
        let print_height_mm = print_width_mm * (self.height - 1) as f32 / (self.width - 1) as f32;
        let smooth_points = resample_surface_line(
            &smooth_surface_line(
                &points
                    .iter()
                    .map(|point| [point[0] * print_width_mm, point[1] * print_height_mm])
                    .collect::<Vec<_>>(),
            ),
            ROAD_VECTOR_STEP_MM,
        );
        let length_mm = smooth_points
            .windows(2)
            .map(|segment| (segment[1][0] - segment[0][0]).hypot(segment[1][1] - segment[0][1]))
            .sum();
        let line = VectorSurfaceLine {
            points_mm: smooth_points.clone(),
            width_mm: line_width_mm,
            model_width_mm: print_width_mm,
            model_height_mm: print_height_mm,
            class,
            bridge_elevations_m,
            length_mm,
        };
        let half_width = line_width_mm * 0.5;
        let bounds = line.points_mm.iter().fold(
            [
                f32::INFINITY,
                f32::INFINITY,
                f32::NEG_INFINITY,
                f32::NEG_INFINITY,
            ],
            |bounds, point| {
                [
                    bounds[0].min((point[0] - half_width) / print_width_mm),
                    bounds[1].min((point[1] - half_width) / print_height_mm),
                    bounds[2].max((point[0] + half_width) / print_width_mm),
                    bounds[3].max((point[1] + half_width) / print_height_mm),
                ]
            },
        );
        // Whole-line bucket rectangle from the padded bounds, exactly as the
        // per-line index used; per-segment rectangles never reach outside it.
        let line_rect = [
            vector_bucket_coordinate(bounds[0]),
            vector_bucket_coordinate(bounds[1]),
            vector_bucket_coordinate(bounds[2]),
            vector_bucket_coordinate(bounds[3]),
        ];
        let line_index = self.vector_lines.len();
        for (segment, pair) in line.points_mm.windows(2).enumerate() {
            // A segment must be indexed in every bucket whose queries it can
            // satisfy. A query q (in mm, after the same clamp the distance
            // test applies) is within half_width of the segment only if q
            // lies inside the segment's bounding box padded by half_width,
            // so index the buckets that padded box touches. The padding math
            // matches the whole-line bounds fold above; on top of that, one
            // extra ring of buckets absorbs any f32 rounding slack in the
            // subtract/divide/quantize chain — rounding is ULP-scale while a
            // bucket spans 1/32 of the model, so containment answers at
            // bucket borders are exactly those of the full-line walk.
            let first_x = vector_bucket_coordinate(
                (pair[0][0].min(pair[1][0]) - half_width) / print_width_mm,
            )
            .saturating_sub(1)
            .max(line_rect[0]);
            let first_y = vector_bucket_coordinate(
                (pair[0][1].min(pair[1][1]) - half_width) / print_height_mm,
            )
            .saturating_sub(1)
            .max(line_rect[1]);
            let last_x = (vector_bucket_coordinate(
                (pair[0][0].max(pair[1][0]) + half_width) / print_width_mm,
            ) + 1)
                .min(line_rect[2]);
            let last_y = (vector_bucket_coordinate(
                (pair[0][1].max(pair[1][1]) + half_width) / print_height_mm,
            ) + 1)
                .min(line_rect[3]);
            for y in first_y..=last_y {
                for x in first_x..=last_x {
                    add_segment_to_bucket(
                        &mut self.vector_line_buckets[y * VECTOR_BUCKET_COLUMNS + x],
                        line_index,
                        segment as u32,
                    );
                }
            }
        }
        self.vector_lines.push(line);
    }

    pub fn paint_building(&mut self, points: &[[f32; 2]], height_m: f32) {
        self.paint_building_with_class(points, height_m, SurfaceClass::Building);
    }

    pub fn paint_building_with_class(
        &mut self,
        points: &[[f32; 2]],
        height_m: f32,
        class: SurfaceClass,
    ) {
        if points.len() < 3 || !height_m.is_finite() || height_m <= 0.0 {
            return;
        }
        let area = VectorSurfaceArea {
            points: points.to_vec(),
            class: (class != SurfaceClass::Building).then_some(class),
            building_height_m: height_m,
        };
        let area_index = self.vector_areas.len();
        add_to_vector_buckets(
            &mut self.vector_area_buckets,
            surface_area_bounds(&area.points),
            area_index,
        );
        self.vector_areas.push(area);
    }

    pub fn paint_surface_area(&mut self, points: &[[f32; 2]], class: SurfaceClass) {
        if points.len() < 3 {
            return;
        }
        let area = VectorSurfaceArea {
            points: points.to_vec(),
            class: Some(class),
            building_height_m: 0.0,
        };
        let area_index = self.vector_areas.len();
        add_to_vector_buckets(
            &mut self.vector_area_buckets,
            surface_area_bounds(&area.points),
            area_index,
        );
        self.vector_areas.push(area);
        self.rasterize_area(points, class);
    }

    fn rasterize_area(&mut self, points: &[[f32; 2]], class: SurfaceClass) {
        let pixels = points
            .iter()
            .map(|point| {
                [
                    point[0] * (self.width - 1) as f32,
                    point[1] * (self.height - 1) as f32,
                ]
            })
            .collect::<Vec<_>>();
        let polygon_min_x = pixels
            .iter()
            .map(|point| point[0])
            .fold(f32::INFINITY, f32::min);
        let polygon_max_x = pixels
            .iter()
            .map(|point| point[0])
            .fold(f32::NEG_INFINITY, f32::max);
        let polygon_min_y = pixels
            .iter()
            .map(|point| point[1])
            .fold(f32::INFINITY, f32::min);
        let polygon_max_y = pixels
            .iter()
            .map(|point| point[1])
            .fold(f32::NEG_INFINITY, f32::max);
        if polygon_max_x < 0.0
            || polygon_min_x > (self.width - 1) as f32
            || polygon_max_y < 0.0
            || polygon_min_y > (self.height - 1) as f32
        {
            return;
        }
        let min_x = polygon_min_x.floor().max(0.0) as usize;
        let max_x = polygon_max_x.ceil().min((self.width - 1) as f32) as usize;
        let min_y = polygon_min_y.floor().max(0.0) as usize;
        let max_y = polygon_max_y.ceil().min((self.height - 1) as f32) as usize;
        for y in min_y..=max_y {
            for x in min_x..=max_x {
                if point_in_polygon([x as f32, y as f32], &pixels) {
                    self.classes[y * self.width + x] = class;
                }
            }
        }
    }

    fn filter_components_smaller_than(&mut self, minimum_cells: usize) {
        let original = self.classes.clone();
        let mut visited = vec![false; original.len()];
        for start in 0..original.len() {
            if visited[start] {
                continue;
            }
            let class = original[start];
            let mut queue = VecDeque::from([start]);
            let mut component = Vec::new();
            let mut neighbours = [0_usize; SurfaceClass::ALL.len()];
            visited[start] = true;
            while let Some(index) = queue.pop_front() {
                component.push(index);
                let x = index % self.width;
                let y = index / self.width;
                for neighbour in [
                    x.checked_sub(1).map(|value| y * self.width + value),
                    (x + 1 < self.width).then_some(y * self.width + x + 1),
                    y.checked_sub(1).map(|value| value * self.width + x),
                    (y + 1 < self.height).then_some((y + 1) * self.width + x),
                ]
                .into_iter()
                .flatten()
                {
                    let neighbour_class = original[neighbour];
                    if neighbour_class == class {
                        if !visited[neighbour] {
                            visited[neighbour] = true;
                            queue.push_back(neighbour);
                        }
                    } else {
                        neighbours[neighbour_class.material_index() as usize] += 1;
                    }
                }
            }
            if component.len() < minimum_cells {
                // `neighbours` is indexed by material_index, so ALL (which
                // is ordered by material_index) inverts it directly.
                let replacement = neighbours
                    .into_iter()
                    .enumerate()
                    .max_by_key(|(index, count)| (*count, usize::MAX - *index))
                    .map(|(index, _)| SurfaceClass::ALL[index])
                    .unwrap_or(SurfaceClass::Rock);
                for index in component {
                    self.classes[index] = replacement;
                }
            }
        }
    }

    fn at(&self, u: f32, v: f32) -> SurfaceClass {
        self.sample(u, v).class
    }

    /// Surface class at a normalized position with every overlay applied —
    /// the class a generated artifact colors that spot.
    pub fn class_at(&self, u: f32, v: f32) -> SurfaceClass {
        self.at(u, v)
    }

    pub(crate) fn terrain_at(&self, u: f32, v: f32) -> SurfaceClass {
        self.terrain_sample(u, v).class
    }

    pub(crate) fn sample(&self, u: f32, v: f32) -> SurfaceSample {
        self.sample_with_overlays(u, v, true, true)
    }

    fn terrain_sample(&self, u: f32, v: f32) -> SurfaceSample {
        self.sample_with_overlays(u, v, false, false)
    }

    fn interpolated_base_class(&self, u: f32, v: f32) -> SurfaceClass {
        let x = u.clamp(0.0, 1.0) * (self.width - 1) as f32;
        let y = v.clamp(0.0, 1.0) * (self.height - 1) as f32;
        let x0 = x.floor() as usize;
        let y0 = y.floor() as usize;
        let x1 = (x0 + 1).min(self.width - 1);
        let y1 = (y0 + 1).min(self.height - 1);
        let tx = x - x0 as f32;
        let ty = y - y0 as f32;
        let corners = [
            (
                self.base_classes[y0 * self.width + x0],
                (1.0 - tx) * (1.0 - ty),
            ),
            (self.base_classes[y0 * self.width + x1], tx * (1.0 - ty)),
            (self.base_classes[y1 * self.width + x1], tx * ty),
            (self.base_classes[y1 * self.width + x0], (1.0 - tx) * ty),
        ];
        SurfaceClass::ALL
            .into_iter()
            .map(|class| {
                let weight = corners
                    .iter()
                    .filter(|(corner_class, _)| *corner_class == class)
                    .map(|(_, weight)| weight)
                    .sum::<f32>();
                (class, weight)
            })
            .max_by(|first, second| first.1.total_cmp(&second.1))
            .map(|(class, _)| class)
            .unwrap_or(SurfaceClass::Rock)
    }

    fn sample_with_overlays(
        &self,
        u: f32,
        v: f32,
        include_roads: bool,
        include_buildings: bool,
    ) -> SurfaceSample {
        let bucket = vector_bucket_index(u, v);
        let building_height_m = self.building_height_at_in_bucket(u, v, bucket);
        let has_marker = self.vector_area_buckets[bucket]
            .iter()
            .rev()
            .map(|index| &self.vector_areas[*index])
            .any(|area| {
                area.building_height_m == 0.0
                    && area.class == Some(SurfaceClass::Marker)
                    && point_in_polygon([u, v], &area.points)
            });
        if has_marker {
            return SurfaceSample {
                class: SurfaceClass::Marker,
                building_height_m,
            };
        }
        if include_buildings && building_height_m > 0.0 {
            return SurfaceSample {
                class: SurfaceClass::Building,
                building_height_m,
            };
        }
        let line_entries = &self.vector_line_buckets[bucket];
        // Imported trails are the user's own highlight, so they sit above
        // roads and every other overlay short of buildings. Without trails
        // no Trail line exists and this check never matches.
        let has_trail = include_roads
            && line_entries.iter().any(|entry| {
                let line = &self.vector_lines[entry.line_index];
                line.class == SurfaceClass::Trail
                    && line_segment_ranges_contain(line, &entry.segment_ranges, u, v)
            });
        if has_trail {
            return SurfaceSample {
                class: SurfaceClass::Trail,
                building_height_m,
            };
        }
        // Aerialways sit above everything they cross — a cable car really
        // does fly over the railway and the road. Under any style but
        // `separate` no Aerial line exists and this check never matches.
        let has_aerial = include_roads
            && line_entries.iter().any(|entry| {
                let line = &self.vector_lines[entry.line_index];
                line.class == SurfaceClass::Aerial
                    && line_segment_ranges_contain(line, &entry.segment_ranges, u, v)
            });
        if has_aerial {
            return SurfaceSample {
                class: SurfaceClass::Aerial,
                building_height_m,
            };
        }
        // Railways sit above roads: at a level crossing the rail line is the
        // feature worth reading. Under the default `with_roads` style no
        // Rail line exists and this check never matches.
        let has_rail = include_roads
            && line_entries.iter().any(|entry| {
                let line = &self.vector_lines[entry.line_index];
                line.class == SurfaceClass::Rail
                    && line_segment_ranges_contain(line, &entry.segment_ranges, u, v)
            });
        if has_rail {
            return SurfaceSample {
                class: SurfaceClass::Rail,
                building_height_m,
            };
        }
        // Ferries sit above roads for the reason railways do: where the two
        // meet, at a terminal apron, the crossing is the feature worth
        // reading. Under any style but `separate` no Ferry line exists and
        // this check never matches.
        //
        // Like every overlay above it this is gated on `include_roads`, so
        // the TERRAIN sampler still answers with the water a crossing runs
        // over. Without the gate the mesh would paint its terrain triangles
        // in the ferry material and then raise the ferry ribbon on top of
        // them.
        let has_ferry = include_roads
            && line_entries.iter().any(|entry| {
                let line = &self.vector_lines[entry.line_index];
                line.class == SurfaceClass::Ferry
                    && line_segment_ranges_contain(line, &entry.segment_ranges, u, v)
            });
        if has_ferry {
            return SurfaceSample {
                class: SurfaceClass::Ferry,
                building_height_m,
            };
        }
        let has_road = include_roads
            && line_entries.iter().any(|entry| {
                let line = &self.vector_lines[entry.line_index];
                line.class == SurfaceClass::Road
                    && line_segment_ranges_contain(line, &entry.segment_ranges, u, v)
            });
        if has_road {
            return SurfaceSample {
                class: SurfaceClass::Road,
                building_height_m,
            };
        }
        if let Some(class) = self.vector_area_buckets[bucket]
            .iter()
            .rev()
            .filter_map(|index| {
                let area = &self.vector_areas[*index];
                area.class.map(|class| (area, class))
            })
            .find(|(area, _)| point_in_polygon([u, v], &area.points))
            .map(|(_, class)| class)
        {
            return SurfaceSample {
                class,
                building_height_m,
            };
        }
        if let Some(class) = line_entries
            .iter()
            .rev()
            .map(|entry| (&self.vector_lines[entry.line_index], entry))
            .filter(|(line, _)| {
                !matches!(
                    line.class,
                    SurfaceClass::Road
                        | SurfaceClass::Trail
                        | SurfaceClass::Rail
                        | SurfaceClass::Aerial
                        | SurfaceClass::Ferry
                )
            })
            .find(|(line, entry)| line_segment_ranges_contain(line, &entry.segment_ranges, u, v))
            .map(|(line, _)| line.class)
        {
            return SurfaceSample {
                class,
                building_height_m,
            };
        }
        SurfaceSample {
            class: self.interpolated_base_class(u, v),
            building_height_m,
        }
    }

    #[cfg(test)]
    fn building_height_at(&self, u: f32, v: f32) -> f32 {
        self.building_height_at_in_bucket(u, v, vector_bucket_index(u, v))
    }

    fn building_height_at_in_bucket(&self, u: f32, v: f32, bucket: usize) -> f32 {
        self.vector_area_buckets[bucket]
            .iter()
            .map(|index| &self.vector_areas[*index])
            .filter(|area| area.building_height_m > 0.0)
            .filter(|area| point_in_polygon([u, v], &area.points))
            .map(|area| area.building_height_m)
            .fold(0.0, f32::max)
    }

    /// Every surface class this field holds, exactly and without sampling:
    /// the raster values it carries, the classes of its vector areas and
    /// vector lines, and Building where it holds a footprint.
    ///
    /// This is the SUPERSET a mesh built from the field can paint. Terrain
    /// tops read `terrain_at`, which returns a base-class value; vector
    /// areas and lines answer through `sample`, which can only return a
    /// class one of them carries; building shells only exist where a
    /// footprint does. So no sampling of this field can produce a class the
    /// set omits — which is what lets a 3MF size its filament palette from
    /// here without risking a triangle that has no slot.
    ///
    /// It may be LOOSE in the other direction: a class the field holds in a
    /// spot no piece happens to sample still counts. That costs at worst a
    /// slot, where the reverse error would cost the export.
    pub fn contained_classes(&self) -> [bool; SurfaceClass::ALL.len()] {
        let mut present = [false; SurfaceClass::ALL.len()];
        for class in self.classes.iter().chain(&self.base_classes) {
            present[class.material_index() as usize] = true;
        }
        for area in &self.vector_areas {
            if let Some(class) = area.class {
                present[class.material_index() as usize] = true;
            }
            if area.building_height_m > 0.0 {
                present[SurfaceClass::Building.material_index() as usize] = true;
            }
        }
        for line in &self.vector_lines {
            present[line.class.material_index() as usize] = true;
        }
        present
    }

    pub(crate) fn coverage(&self) -> [f32; SurfaceClass::ALL.len()] {
        let counts = (0..self.classes.len())
            .into_par_iter()
            .fold(
                || [0_usize; SurfaceClass::ALL.len()],
                |mut counts, index| {
                    let x = index % self.width;
                    let y = index / self.width;
                    let u = x as f32 / (self.width - 1) as f32;
                    let v = y as f32 / (self.height - 1) as f32;
                    counts[self.at(u, v).material_index() as usize] += 1;
                    counts
                },
            )
            .reduce(
                || [0_usize; SurfaceClass::ALL.len()],
                |mut total, counts| {
                    for (total, count) in total.iter_mut().zip(counts) {
                        *total += count;
                    }
                    total
                },
            );
        let total = self.classes.len() as f32;
        counts.map(|count| count as f32 * 100.0 / total)
    }
}

/// Spherical semivariogram in native-cell units with a nugget expressed as
/// a fraction of the sill. Zero at zero distance, so kriging honours the
/// data exactly at nodes.
/// Node counts of the lattice `smooth_class_borders` recovers, one axis at
/// a time from that axis's own samples-per-native-cell density. Densities
/// below one sample per cell clamp to the raster's own resolution: on a
/// non-square raster (rows unequal to columns over the square ground
/// bounds) the coarser axis carries FEWER samples than native cells, and
/// deriving more nodes than it has samples would duplicate raster rows and
/// pass them off as independent native data — the recovered grid must
/// never upsample an axis.
fn recovered_native_dimensions(
    width: usize,
    height: usize,
    cell_samples_x: f32,
    cell_samples_y: f32,
) -> (usize, usize) {
    let nodes = |samples: usize, cell_samples: f32| {
        (((samples - 1) as f32 / cell_samples.max(1.0)).round() as usize).max(1) + 1
    };
    (nodes(width, cell_samples_x), nodes(height, cell_samples_y))
}

fn spherical_variogram(distance: f64, range_cells: f64, nugget: f64) -> f64 {
    if distance <= 0.0 {
        return 0.0;
    }
    if distance >= range_cells {
        return nugget + 1.0;
    }
    let ratio = distance / range_cells;
    nugget + 1.5 * ratio - 0.5 * ratio.powi(3)
}

/// Ordinary-kriging stencils for every quantized fractional position inside
/// a native cell, indexed `offset_y * (KRIGING_OFFSET_STEPS + 1) + offset_x`.
/// Each stencil holds the weights of the 4 by 4 surrounding nodes.
fn kriging_weight_table(
    range_cells: f64,
    nugget: f64,
) -> Vec<[f32; KRIGING_NEIGHBORHOOD * KRIGING_NEIGHBORHOOD]> {
    let mut table = Vec::with_capacity((KRIGING_OFFSET_STEPS + 1) * (KRIGING_OFFSET_STEPS + 1));
    for offset_y in 0..=KRIGING_OFFSET_STEPS {
        for offset_x in 0..=KRIGING_OFFSET_STEPS {
            table.push(kriging_weights(
                offset_x as f64 / KRIGING_OFFSET_STEPS as f64,
                offset_y as f64 / KRIGING_OFFSET_STEPS as f64,
                range_cells,
                nugget,
            ));
        }
    }
    table
}

/// Solves the ordinary-kriging system for a target at fractional position
/// (`target_x`, `target_y`) inside the central cell of a 4 by 4 node
/// neighbourhood whose nodes sit at integer offsets -1..=2 on each axis.
fn kriging_weights(
    target_x: f64,
    target_y: f64,
    range_cells: f64,
    nugget: f64,
) -> [f32; KRIGING_NEIGHBORHOOD * KRIGING_NEIGHBORHOOD] {
    const NODES: usize = KRIGING_NEIGHBORHOOD * KRIGING_NEIGHBORHOOD;
    // Unknowns: one weight per node plus the Lagrange multiplier that
    // enforces the weights summing to one.
    const SIZE: usize = NODES + 1;
    let position = |node: usize| {
        [
            (node % KRIGING_NEIGHBORHOOD) as f64 - 1.0,
            (node / KRIGING_NEIGHBORHOOD) as f64 - 1.0,
        ]
    };
    let mut matrix = [[0.0_f64; SIZE + 1]; SIZE];
    for (row, row_values) in matrix.iter_mut().enumerate().take(NODES) {
        let from = position(row);
        for (column, value) in row_values.iter_mut().enumerate().take(NODES) {
            let to = position(column);
            *value = spherical_variogram(
                (from[0] - to[0]).hypot(from[1] - to[1]),
                range_cells,
                nugget,
            );
        }
        row_values[NODES] = 1.0;
        row_values[SIZE] = spherical_variogram(
            (from[0] - target_x).hypot(from[1] - target_y),
            range_cells,
            nugget,
        );
    }
    matrix[NODES][..NODES].fill(1.0);
    matrix[NODES][NODES] = 0.0;
    matrix[NODES][SIZE] = 1.0;
    // Gaussian elimination with partial pivoting; the system is small and
    // solved only (KRIGING_OFFSET_STEPS + 1)^2 times per smoothing pass.
    for pivot in 0..SIZE {
        let best = (pivot..SIZE)
            .max_by(|left, right| {
                matrix[*left][pivot]
                    .abs()
                    .total_cmp(&matrix[*right][pivot].abs())
            })
            .unwrap_or(pivot);
        matrix.swap(pivot, best);
        let pivot_value = matrix[pivot][pivot];
        if pivot_value.abs() < f64::EPSILON {
            continue;
        }
        let pivot_row = matrix[pivot];
        for (row, row_values) in matrix.iter_mut().enumerate() {
            if row == pivot {
                continue;
            }
            let factor = row_values[pivot] / pivot_value;
            if factor == 0.0 {
                continue;
            }
            for (value, pivot_value) in row_values.iter_mut().zip(pivot_row).skip(pivot) {
                *value -= factor * pivot_value;
            }
        }
    }
    let mut weights = [0.0_f32; NODES];
    for (node, weight) in weights.iter_mut().enumerate() {
        *weight = (matrix[node][SIZE] / matrix[node][node]) as f32;
    }
    weights
}

pub(crate) fn surface_area_bounds(points: &[[f32; 2]]) -> [f32; 4] {
    points.iter().fold(
        [
            f32::INFINITY,
            f32::INFINITY,
            f32::NEG_INFINITY,
            f32::NEG_INFINITY,
        ],
        |bounds, point| {
            [
                bounds[0].min(point[0]),
                bounds[1].min(point[1]),
                bounds[2].max(point[0]),
                bounds[3].max(point[1]),
            ]
        },
    )
}

fn vector_bucket_coordinate(value: f32) -> usize {
    (value.clamp(0.0, 0.999_999) * VECTOR_BUCKET_COLUMNS as f32) as usize
}

fn vector_bucket_index(u: f32, v: f32) -> usize {
    vector_bucket_coordinate(v) * VECTOR_BUCKET_COLUMNS + vector_bucket_coordinate(u)
}

/// Records one segment of one line in a bucket. Segments arrive in
/// increasing order and a whole line is indexed before the next line
/// starts, so the entry for this line is the bucket's last entry (if any)
/// and a contiguous segment extends its trailing range.
fn add_segment_to_bucket(bucket: &mut Vec<LineBucketEntry>, line_index: usize, segment: u32) {
    if let Some(entry) = bucket.last_mut()
        && entry.line_index == line_index
    {
        if let Some(range) = entry.segment_ranges.last_mut() {
            if range.1 == segment {
                range.1 = segment + 1;
                return;
            }
            if segment < range.1 {
                // Each (segment, bucket) pair is visited once, so an
                // already-covered segment cannot arrive; kept as a guard.
                return;
            }
        }
        entry.segment_ranges.push((segment, segment + 1));
        return;
    }
    bucket.push(LineBucketEntry {
        line_index,
        segment_ranges: vec![(segment, segment + 1)],
    });
}

fn add_to_vector_buckets(buckets: &mut [Vec<usize>], bounds: [f32; 4], feature_index: usize) {
    let minimum_x = vector_bucket_coordinate(bounds[0]);
    let minimum_y = vector_bucket_coordinate(bounds[1]);
    let maximum_x = vector_bucket_coordinate(bounds[2]);
    let maximum_y = vector_bucket_coordinate(bounds[3]);
    for y in minimum_y..=maximum_y {
        for x in minimum_x..=maximum_x {
            buckets[y * VECTOR_BUCKET_COLUMNS + x].push(feature_index);
        }
    }
}

fn smooth_surface_line(points: &[[f32; 2]]) -> Vec<[f32; 2]> {
    if points.len() < 3 {
        return points.to_vec();
    }
    let mut result = Vec::with_capacity((points.len() - 1) * 4 + 1);
    for index in 0..points.len() - 1 {
        let controls = [
            points[index.saturating_sub(1)],
            points[index],
            points[index + 1],
            points[(index + 2).min(points.len() - 1)],
        ];
        for sample in 0..4 {
            let t = sample as f32 / 4.0;
            let t2 = t * t;
            let t3 = t2 * t;
            result.push([
                0.5 * (2.0 * controls[1][0]
                    + (-controls[0][0] + controls[2][0]) * t
                    + (2.0 * controls[0][0] - 5.0 * controls[1][0] + 4.0 * controls[2][0]
                        - controls[3][0])
                        * t2
                    + (-controls[0][0] + 3.0 * controls[1][0] - 3.0 * controls[2][0]
                        + controls[3][0])
                        * t3),
                0.5 * (2.0 * controls[1][1]
                    + (-controls[0][1] + controls[2][1]) * t
                    + (2.0 * controls[0][1] - 5.0 * controls[1][1] + 4.0 * controls[2][1]
                        - controls[3][1])
                        * t2
                    + (-controls[0][1] + 3.0 * controls[1][1] - 3.0 * controls[2][1]
                        + controls[3][1])
                        * t3),
            ]);
        }
    }
    result.push(*points.last().unwrap());
    result
}

fn resample_surface_line(points: &[[f32; 2]], maximum_step_mm: f32) -> Vec<[f32; 2]> {
    if points.len() < 2 {
        return points.to_vec();
    }
    let mut result = Vec::new();
    for segment in points.windows(2) {
        let delta = [segment[1][0] - segment[0][0], segment[1][1] - segment[0][1]];
        let length = delta[0].hypot(delta[1]);
        let samples = (length / maximum_step_mm.max(0.01)).ceil().max(1.0) as usize;
        for sample in 0..samples {
            let t = sample as f32 / samples as f32;
            let point = [segment[0][0] + delta[0] * t, segment[0][1] + delta[1] * t];
            if result
                .last()
                .is_none_or(|previous| distance_squared(*previous, point) > 0.000_001)
            {
                result.push(point);
            }
        }
    }
    result.push(*points.last().unwrap());
    result
}

pub(crate) fn surface_line_progress(line: &VectorSurfaceLine, u: f32, v: f32) -> f32 {
    surface_line_nearest_projection(line, u, v).1
}

fn surface_line_nearest_projection(line: &VectorSurfaceLine, u: f32, v: f32) -> (f32, f32) {
    let point = [
        u.clamp(0.0, 1.0) * line.model_width_mm,
        v.clamp(0.0, 1.0) * line.model_height_mm,
    ];
    let mut traversed_mm = 0.0;
    let mut closest = (f32::INFINITY, 0.0);
    for segment in line.points_mm.windows(2) {
        let delta = [segment[1][0] - segment[0][0], segment[1][1] - segment[0][1]];
        let length_squared = delta[0].powi(2) + delta[1].powi(2);
        let length = length_squared.sqrt();
        let offset = [point[0] - segment[0][0], point[1] - segment[0][1]];
        let amount = if length_squared <= f32::EPSILON {
            0.0
        } else {
            ((offset[0] * delta[0] + offset[1] * delta[1]) / length_squared).clamp(0.0, 1.0)
        };
        let nearest = [
            segment[0][0] + delta[0] * amount,
            segment[0][1] + delta[1] * amount,
        ];
        let distance = distance_squared(point, nearest);
        if distance < closest.0 {
            closest = (
                distance,
                (traversed_mm + length * amount) / line.length_mm.max(f32::EPSILON),
            );
        }
        traversed_mm += length;
    }
    closest
}

/// Whether the query point lies within the line's half width, tested only
/// against the segment ranges indexed for the query's bucket.
///
/// Equal to running the old full-polyline walk for every query point whose
/// bucket produced `ranges`: the minimum distance over all segments is
/// within the radius exactly when some segment's distance is, every segment
/// within reach of any point in the bucket is present in `ranges` (see the
/// indexing comment in `paint_polyline_with_bridge`), and the per-segment
/// math below matches `surface_line_nearest_projection` operation for
/// operation.
fn line_segment_ranges_contain(
    line: &VectorSurfaceLine,
    ranges: &[(u32, u32)],
    u: f32,
    v: f32,
) -> bool {
    let radius_squared = (line.width_mm * 0.5).powi(2);
    let point = [
        u.clamp(0.0, 1.0) * line.model_width_mm,
        v.clamp(0.0, 1.0) * line.model_height_mm,
    ];
    for &(first, last) in ranges {
        for segment in first as usize..last as usize {
            let start = line.points_mm[segment];
            let end = line.points_mm[segment + 1];
            let delta = [end[0] - start[0], end[1] - start[1]];
            let length_squared = delta[0].powi(2) + delta[1].powi(2);
            let offset = [point[0] - start[0], point[1] - start[1]];
            let amount = if length_squared <= f32::EPSILON {
                0.0
            } else {
                ((offset[0] * delta[0] + offset[1] * delta[1]) / length_squared).clamp(0.0, 1.0)
            };
            let nearest = [start[0] + delta[0] * amount, start[1] + delta[1] * amount];
            if distance_squared(point, nearest) <= radius_squared {
                return true;
            }
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::piece::scaled_building_height_mm;
    use crate::spec::{BuildingSpec, GenerationSpec};

    #[test]
    fn surface_filter_removes_tiny_color_islands() {
        let mut classes = vec![SurfaceClass::Forest; 25];
        classes[12] = SurfaceClass::Snow;
        let mut field = SurfaceField::new(5, 5, classes, "test").unwrap();
        field.filter_small_patches(10.0, 4.0);
        assert_eq!(field.classes[12], SurfaceClass::Forest);
    }

    #[test]
    fn restoring_raster_edges_keeps_tile_local_changes_inside() {
        let classes = (0_usize..25)
            .map(|index| {
                if index.is_multiple_of(2) {
                    SurfaceClass::Forest
                } else {
                    SurfaceClass::Rock
                }
            })
            .collect::<Vec<_>>();
        let mut field = SurfaceField::new(5, 5, classes.clone(), "test").unwrap();
        let edges = field.raster_edge_classes();
        field.classes.fill(SurfaceClass::Snow);
        field.base_classes.fill(SurfaceClass::Snow);

        field.restore_raster_edge_classes(&edges).unwrap();

        assert_eq!(field.classes[2 * 5 + 2], SurfaceClass::Snow);
        for column in 0..5 {
            assert_eq!(field.classes[column], classes[column]);
            assert_eq!(field.classes[4 * 5 + column], classes[4 * 5 + column]);
        }
        for row in 1..4 {
            assert_eq!(field.classes[row * 5], classes[row * 5]);
            assert_eq!(field.classes[row * 5 + 4], classes[row * 5 + 4]);
        }
        assert_eq!(field.classes, field.base_classes);
    }

    #[test]
    fn base_surface_classes_use_interpolated_boundaries() {
        let field = SurfaceField::new(
            2,
            2,
            vec![
                SurfaceClass::Forest,
                SurfaceClass::Rock,
                SurfaceClass::Forest,
                SurfaceClass::Forest,
            ],
            "test",
        )
        .unwrap();

        assert_eq!(field.interpolated_base_class(0.75, 0.1), SurfaceClass::Rock);
        assert_eq!(
            field.interpolated_base_class(0.75, 0.9),
            SurfaceClass::Forest
        );
    }

    #[test]
    fn surface_field_paints_print_width_aware_road_lines() {
        let mut field =
            SurfaceField::new(21, 21, vec![SurfaceClass::Forest; 21 * 21], "test").unwrap();
        field.paint_polyline(&[[0.0, 0.5], [1.0, 0.5]], 20.0, 2.0, SurfaceClass::Road);

        assert_eq!(field.at(0.5, 0.5), SurfaceClass::Road);
        assert_eq!(field.at(0.5, 0.53), SurfaceClass::Road);
        assert_eq!(field.at(0.5, 0.35), SurfaceClass::Forest);
    }

    #[test]
    fn surface_field_paints_trail_lines_above_roads_and_water() {
        let mut field =
            SurfaceField::new(21, 21, vec![SurfaceClass::Forest; 21 * 21], "test").unwrap();
        field.paint_surface_area(
            &[[0.4, 0.0], [0.6, 0.0], [0.6, 1.0], [0.4, 1.0]],
            SurfaceClass::Water,
        );
        field.paint_polyline(&[[0.0, 0.4], [1.0, 0.4]], 20.0, 2.0, SurfaceClass::Road);
        field.paint_polyline(&[[0.0, 0.5], [1.0, 0.5]], 20.0, 2.0, SurfaceClass::Trail);
        // A crossing trail wins over the road it shares samples with.
        field.paint_polyline(&[[0.0, 0.4], [1.0, 0.4]], 20.0, 2.0, SurfaceClass::Trail);

        assert_eq!(field.at(0.2, 0.5), SurfaceClass::Trail);
        assert_eq!(field.at(0.5, 0.5), SurfaceClass::Trail, "beats water areas");
        assert_eq!(field.at(0.2, 0.4), SurfaceClass::Trail, "beats roads");
        assert_eq!(field.at(0.2, 0.7), SurfaceClass::Forest);
        // The terrain raster under the raised trail keeps its base class,
        // like it does under roads.
        assert_eq!(field.terrain_at(0.2, 0.5), SurfaceClass::Forest);
    }

    #[test]
    fn vector_lines_are_smooth_and_independent_of_raster_cells() {
        let mut field =
            SurfaceField::new(11, 11, vec![SurfaceClass::Forest; 11 * 11], "test").unwrap();
        field.paint_polyline(
            &[[0.0, 0.2], [0.5, 0.8], [1.0, 0.2]],
            20.0,
            0.4,
            SurfaceClass::Road,
        );
        assert!(field.vector_lines[0].points_mm.len() > 3);
        assert_eq!(field.at(0.5, 0.8), SurfaceClass::Road);
        assert_eq!(field.at(0.5, 0.84), SurfaceClass::Forest);

        field.paint_polyline(
            &[[0.0, 0.1], [0.5, 0.15], [1.0, 0.1]],
            20.0,
            0.6,
            SurfaceClass::Water,
        );
        assert_eq!(field.at(0.5, 0.15), SurfaceClass::Water);
    }

    #[test]
    fn vector_water_areas_keep_their_exact_boundary() {
        let mut field = SurfaceField::new(5, 5, vec![SurfaceClass::Rock; 25], "test").unwrap();
        field.paint_surface_area(
            &[[0.45, 0.45], [0.55, 0.45], [0.55, 0.55], [0.45, 0.55]],
            SurfaceClass::Water,
        );
        assert_eq!(field.at(0.5, 0.5), SurfaceClass::Water);
        assert_eq!(field.at(0.6, 0.5), SurfaceClass::Rock);
    }

    /// Nearest-neighbour upsample of a 9 by 9 native diagonal split onto a
    /// 65 by 65 sample grid: the blocky staircase the smoother should bend.
    fn blocky_diagonal_field() -> SurfaceField {
        let size = 65;
        let native_side = 8.0;
        let classes = (0..size * size)
            .map(|index| {
                let x = (index % size) as f32 / (size - 1) as f32;
                let y = (index / size) as f32 / (size - 1) as f32;
                let node_x = (x * native_side).round();
                let node_y = (y * native_side).round();
                if node_x + node_y > native_side {
                    SurfaceClass::Forest
                } else {
                    SurfaceClass::Rock
                }
            })
            .collect();
        SurfaceField::new(size, size, classes, "test").unwrap()
    }

    /// The defaults `smooth_class_borders` gained when its range and nugget
    /// became parameters; passing these must reproduce the old constants'
    /// output exactly.
    const DEFAULT_RANGE_CELLS: f32 = 2.5;
    const DEFAULT_NUGGET: f32 = 0.05;

    #[test]
    fn kriging_weights_sum_to_one() {
        for stencil in kriging_weight_table(DEFAULT_RANGE_CELLS as f64, DEFAULT_NUGGET as f64) {
            let total: f32 = stencil.iter().sum();
            assert!((total - 1.0).abs() < 1e-4, "weights sum to {total}");
        }
        // At a node position the estimator is exact: all weight on the node.
        let at_node = kriging_weights(0.0, 0.0, DEFAULT_RANGE_CELLS as f64, DEFAULT_NUGGET as f64);
        assert!(at_node[KRIGING_NEIGHBORHOOD + 1] > 0.99);
    }

    #[test]
    fn border_smoothing_skips_rasters_at_native_resolution() {
        let mut field = blocky_diagonal_field();
        let original = field.base_classes.clone();
        // 64 sample steps over 1 km is 15.6 m per sample: coarser than the
        // 10 m source, so there is nothing to reconstruct.
        field.smooth_class_borders(10.0, 1_000.0, DEFAULT_RANGE_CELLS, DEFAULT_NUGGET);
        assert_eq!(field.base_classes, original);
        assert_eq!(field.classes, original);
    }

    #[test]
    fn border_smoothing_bends_staircase_borders_deterministically() {
        let mut first = blocky_diagonal_field();
        let mut second = first.clone();
        let original = first.base_classes.clone();
        // 10 m cells over an 80 m span put 8 samples in each native cell.
        first.smooth_class_borders(10.0, 80.0, DEFAULT_RANGE_CELLS, DEFAULT_NUGGET);
        second.smooth_class_borders(10.0, 80.0, DEFAULT_RANGE_CELLS, DEFAULT_NUGGET);
        assert_eq!(first.base_classes, second.base_classes);
        assert_eq!(first.classes, first.base_classes);
        let changed = first
            .base_classes
            .iter()
            .zip(&original)
            .filter(|(after, before)| after != before)
            .count();
        assert!(changed > 0, "staircase corners should move");
        assert!(
            changed < original.len() / 4,
            "smoothing should only touch the border region, changed {changed}"
        );
        // No class invented, and the far corners keep their classes.
        assert!(
            first
                .base_classes
                .iter()
                .all(|class| matches!(class, SurfaceClass::Rock | SurfaceClass::Forest))
        );
        assert_eq!(first.base_classes[2 * 65 + 2], SurfaceClass::Rock);
        assert_eq!(first.base_classes[62 * 65 + 62], SurfaceClass::Forest);
    }

    #[test]
    fn border_smoothing_range_and_nugget_change_the_result() {
        let mut default_range = blocky_diagonal_field();
        let mut wide_range = default_range.clone();
        let mut damped = default_range.clone();
        default_range.smooth_class_borders(10.0, 80.0, DEFAULT_RANGE_CELLS, DEFAULT_NUGGET);
        wide_range.smooth_class_borders(10.0, 80.0, 8.0, DEFAULT_NUGGET);
        damped.smooth_class_borders(10.0, 80.0, DEFAULT_RANGE_CELLS, 0.5);
        assert_ne!(
            default_range.base_classes, wide_range.base_classes,
            "a wider variogram range should move the smoothed borders"
        );
        assert_ne!(
            default_range.base_classes, damped.base_classes,
            "a heavier nugget should move the smoothed borders"
        );
        // Both variants still only redraw borders between the two classes.
        for field in [&wide_range, &damped] {
            assert!(
                field
                    .base_classes
                    .iter()
                    .all(|class| matches!(class, SurfaceClass::Rock | SurfaceClass::Forest))
            );
        }
    }

    /// The true 9 by 9 native diagonal that `blocky_diagonal_field` is a
    /// nearest-neighbour upsample of.
    fn diagonal_native_classes() -> Vec<SurfaceClass> {
        (0..9 * 9)
            .map(|index| {
                if index % 9 + index / 9 > 8 {
                    SurfaceClass::Forest
                } else {
                    SurfaceClass::Rock
                }
            })
            .collect()
    }

    #[test]
    fn native_smoothing_matches_the_recovered_path_when_aligned() {
        let mut recovered = blocky_diagonal_field();
        let mut native_path = recovered.clone();
        recovered.smooth_class_borders(10.0, 80.0, DEFAULT_RANGE_CELLS, DEFAULT_NUGGET);
        // The same lattice the recovered path reconstructs: 9 nodes across
        // 64 sample steps, no phase.
        let native = NativeClassGrid::new(
            9,
            9,
            diagonal_native_classes(),
            8.0 / 64.0,
            0.0,
            8.0 / 64.0,
            0.0,
        )
        .unwrap();
        native_path.smooth_class_borders_with_native(&native, DEFAULT_RANGE_CELLS, DEFAULT_NUGGET);
        assert_eq!(recovered.base_classes, native_path.base_classes);
        assert_eq!(native_path.classes, native_path.base_classes);
    }

    #[test]
    fn native_smoothing_honours_the_lattice_phase() {
        let mut aligned = blocky_diagonal_field();
        let mut shifted = aligned.clone();
        let grid = |x_offset: f64| {
            NativeClassGrid::new(
                9,
                9,
                diagonal_native_classes(),
                8.0 / 64.0,
                x_offset,
                8.0 / 64.0,
                0.0,
            )
            .unwrap()
        };
        aligned.smooth_class_borders_with_native(&grid(0.0), DEFAULT_RANGE_CELLS, DEFAULT_NUGGET);
        // The same data half a native cell further along x: every border
        // must shift with it instead of snapping back to the sample grid.
        shifted.smooth_class_borders_with_native(&grid(0.5), DEFAULT_RANGE_CELLS, DEFAULT_NUGGET);
        assert_ne!(aligned.base_classes, shifted.base_classes);
        let first_forest = |field: &SurfaceField, row: usize| {
            (0..65)
                .find(|x| field.base_classes[row * 65 + x] == SurfaceClass::Forest)
                .unwrap()
        };
        for row in [16, 32, 48] {
            let moved = first_forest(&aligned, row) as isize - first_forest(&shifted, row) as isize;
            // Half a native cell is 4 samples; allow one sample of kriging
            // slack but reject snapping (0) and whole-cell jumps (8).
            assert!(
                (3..=5).contains(&moved),
                "row {row} border moved {moved} samples"
            );
        }
    }

    #[test]
    fn native_smoothing_is_deterministic_and_respects_the_no_op_threshold() {
        let mut first = blocky_diagonal_field();
        let mut second = first.clone();
        let native = NativeClassGrid::new(
            9,
            9,
            diagonal_native_classes(),
            8.0 / 64.0,
            0.25,
            8.0 / 64.0,
            0.25,
        )
        .unwrap();
        first.smooth_class_borders_with_native(&native, DEFAULT_RANGE_CELLS, DEFAULT_NUGGET);
        second.smooth_class_borders_with_native(&native, DEFAULT_RANGE_CELLS, DEFAULT_NUGGET);
        assert_eq!(first.base_classes, second.base_classes);

        // One sample per native cell: nothing to reconstruct, so the grid
        // is ignored just as the recovered path would ignore it.
        let mut coarse = blocky_diagonal_field();
        let original = coarse.base_classes.clone();
        let unit = NativeClassGrid::new(
            65,
            65,
            vec![SurfaceClass::Rock; 65 * 65],
            1.0,
            0.0,
            1.0,
            0.0,
        )
        .unwrap();
        coarse.smooth_class_borders_with_native(&unit, DEFAULT_RANGE_CELLS, DEFAULT_NUGGET);
        assert_eq!(coarse.base_classes, original);
    }

    #[test]
    fn recovered_lattice_never_upsamples_an_axis() {
        // Oversampled on both axes: per-axis node counts follow each
        // axis's own density (128 / 8 and 4 / 2, plus one).
        assert_eq!(recovered_native_dimensions(129, 5, 8.0, 2.0), (17, 3));
        // The y axis carries FEWER samples than native cells (0.5 per
        // cell). It must clamp to the raster's own resolution instead of
        // doubling to 9 nodes of duplicated data.
        assert_eq!(recovered_native_dimensions(129, 5, 8.0, 0.5), (17, 5));
        assert_eq!(recovered_native_dimensions(5, 129, 0.25, 4.0), (5, 33));
        // Degenerate densities still never exceed the raster.
        let (native_width, native_height) = recovered_native_dimensions(65, 3, 0.01, 0.01);
        assert!(native_width <= 65 && native_height <= 3);
    }

    #[test]
    fn non_square_rasters_smooth_deterministically_without_upsampling() {
        // 65 x 5 samples over 80 m of ground: 8 samples per 10 m cell
        // along x, half a sample per cell along y — the anisotropic case
        // where the old reconstruction made the y lattice denser than the
        // raster. A blocky vertical border must still smooth and stay
        // deterministic.
        let width = 65;
        let height = 5;
        let classes = (0..height)
            .flat_map(|_| {
                (0..width).map(move |x| {
                    if (x / 8) % 2 == 0 {
                        SurfaceClass::Rock
                    } else {
                        SurfaceClass::Forest
                    }
                })
            })
            .collect::<Vec<_>>();
        let mut first = SurfaceField::new(width, height, classes, "test").unwrap();
        assert!(first.class_border_smoothing_applies(10.0, 80.0));
        let mut second = first.clone();
        first.smooth_class_borders(10.0, 80.0, DEFAULT_RANGE_CELLS, DEFAULT_NUGGET);
        second.smooth_class_borders(10.0, 80.0, DEFAULT_RANGE_CELLS, DEFAULT_NUGGET);
        assert_eq!(first.classes, second.classes);
        assert!(first.classes.contains(&SurfaceClass::Rock));
        assert!(first.classes.contains(&SurfaceClass::Forest));
    }

    #[test]
    fn smoothing_gate_tracks_the_sample_density() {
        let field = blocky_diagonal_field();
        // 64 sample steps over 80 m is 8 samples per 10 m cell; over 1 km
        // it is coarser than the source and smoothing must no-op, so
        // callers can skip fetching native data entirely.
        assert!(field.class_border_smoothing_applies(10.0, 80.0));
        assert!(!field.class_border_smoothing_applies(10.0, 1_000.0));
        assert!(!field.class_border_smoothing_applies(10.0, 0.0));
        assert!(!field.class_border_smoothing_applies(f32::NAN, 80.0));
    }

    fn forest_gate(limit_degrees: f32, target: SteepForestTarget) -> SlopeGates {
        SlopeGates {
            forest_limit_degrees: Some(limit_degrees),
            steep_forest_target: target,
            snow_limit_degrees: None,
        }
    }

    /// A 300 m wall at the grid middle: one 31.25 m step, roughly 78
    /// degrees for the central difference; the rest is dead flat.
    fn cliff_height_field(size: usize) -> HeightField {
        let values = (0..size * size)
            .map(|index| {
                if (index % size) as f32 / (size - 1) as f32 > 0.5 {
                    300.0
                } else {
                    0.0
                }
            })
            .collect();
        HeightField::new(size, size, values, "cliff").unwrap()
    }

    #[test]
    fn steep_slope_gate_demotes_forest_on_cliff_faces() {
        let size = 33;
        let height_field = cliff_height_field(size);
        let mut field =
            SurfaceField::new(size, size, vec![SurfaceClass::Forest; size * size], "test").unwrap();
        let demoted = field.demote_steep_classes(
            &height_field,
            1_000.0,
            forest_gate(55.0, SteepForestTarget::Rock),
        );
        assert!(demoted.forest_to_rock > 0);
        assert_eq!(demoted.forest_to_snow, 0);
        assert_eq!(demoted.snow_to_rock, 0);
        assert_eq!(field.terrain_at(0.5, 0.5), SurfaceClass::Rock);
        assert_eq!(field.terrain_at(0.1, 0.5), SurfaceClass::Forest);
        assert_eq!(field.terrain_at(0.9, 0.5), SurfaceClass::Forest);
        assert_eq!(field.base_classes, field.classes);
    }

    #[test]
    fn steep_slope_gate_keeps_forest_on_ordinary_slopes() {
        let size = 33;
        // A uniform 300 m rise over 1 km is under 17 degrees.
        let values = (0..size * size)
            .map(|index| (index % size) as f32 / (size - 1) as f32 * 300.0)
            .collect();
        let height_field = HeightField::new(size, size, values, "hill").unwrap();
        let mut field =
            SurfaceField::new(size, size, vec![SurfaceClass::Forest; size * size], "test").unwrap();
        assert_eq!(
            field
                .demote_steep_classes(
                    &height_field,
                    1_000.0,
                    forest_gate(55.0, SteepForestTarget::Rock),
                )
                .total(),
            0
        );
        assert!(
            field
                .classes
                .iter()
                .all(|class| *class == SurfaceClass::Forest)
        );
    }

    #[test]
    fn snow_gate_demotes_steep_snow_and_keeps_moderate_snow() {
        let size = 33;
        let height_field = cliff_height_field(size);
        let mut field =
            SurfaceField::new(size, size, vec![SurfaceClass::Snow; size * size], "test").unwrap();
        let demoted = field.demote_steep_classes(
            &height_field,
            1_000.0,
            SlopeGates {
                forest_limit_degrees: None,
                steep_forest_target: SteepForestTarget::Rock,
                snow_limit_degrees: Some(65.0),
            },
        );
        assert!(demoted.snow_to_rock > 0);
        assert_eq!(demoted.forest_to_rock, 0);
        assert_eq!(demoted.forest_to_snow, 0);
        // The wall sheds its snow; the gentle ground either side keeps it.
        assert_eq!(field.terrain_at(0.5, 0.5), SurfaceClass::Rock);
        assert_eq!(field.terrain_at(0.1, 0.5), SurfaceClass::Snow);
        assert_eq!(field.terrain_at(0.9, 0.5), SurfaceClass::Snow);
        assert_eq!(field.base_classes, field.classes);
    }

    /// A steep ramp along x (3200 m over 1 km, about 73 degrees everywhere)
    /// with a snowcap on the high side and a forested wall crossing the
    /// snowline as a horizontal band.
    fn snowline_wall() -> (HeightField, SurfaceField) {
        let size = 33;
        let elevation = |x: usize| x as f32 / (size - 1) as f32 * 3_200.0;
        let values = (0..size * size)
            .map(|index| elevation(index % size))
            .collect();
        let height_field = HeightField::new(size, size, values, "ramp").unwrap();
        let classes = (0..size * size)
            .map(|index| {
                let x = index % size;
                let v = (index / size) as f32 / (size - 1) as f32;
                if (0.4..=0.6).contains(&v) {
                    SurfaceClass::Forest
                } else if elevation(x) > 2_400.0 {
                    SurfaceClass::Snow
                } else {
                    SurfaceClass::Rock
                }
            })
            .collect();
        let field = SurfaceField::new(size, size, classes, "test").unwrap();
        (height_field, field)
    }

    #[test]
    fn snow_target_splits_the_demoted_wall_at_the_snowline() {
        let (height_field, mut field) = snowline_wall();
        let demoted = field.demote_steep_classes(
            &height_field,
            1_000.0,
            forest_gate(55.0, SteepForestTarget::Snow),
        );
        // The whole band is steeper than the limit, and the snowline (the
        // 10th percentile of snow elevations, 2500 m here) splits it.
        assert!(demoted.forest_to_snow > 0);
        assert!(demoted.forest_to_rock > 0);
        assert_eq!(demoted.snow_to_rock, 0);
        assert_eq!(field.terrain_at(1.0, 0.5), SurfaceClass::Snow);
        assert_eq!(field.terrain_at(0.9, 0.5), SurfaceClass::Snow);
        assert_eq!(field.terrain_at(0.5, 0.5), SurfaceClass::Rock);
        assert_eq!(field.terrain_at(0.1, 0.5), SurfaceClass::Rock);
        // Outside the band nothing moves.
        assert_eq!(field.terrain_at(0.1, 0.1), SurfaceClass::Rock);
        assert_eq!(field.terrain_at(1.0, 0.1), SurfaceClass::Snow);
        assert_eq!(field.base_classes, field.classes);
    }

    #[test]
    fn rock_target_keeps_the_current_behavior_on_snow_scenes() {
        let (height_field, mut field) = snowline_wall();
        let demoted = field.demote_steep_classes(
            &height_field,
            1_000.0,
            forest_gate(55.0, SteepForestTarget::Rock),
        );
        assert_eq!(demoted.forest_to_snow, 0);
        assert!(demoted.forest_to_rock > 0);
        // The demoted wall is rock everywhere, snowcap or not.
        assert_eq!(field.terrain_at(1.0, 0.5), SurfaceClass::Rock);
        assert_eq!(field.terrain_at(0.1, 0.5), SurfaceClass::Rock);
    }

    #[test]
    fn snow_target_without_a_snowcap_demotes_to_rock() {
        let (height_field, mut field) = snowline_wall();
        // Erase the snowcap down to a few stray pixels: under the 1 percent
        // threshold there is no snowline.
        for class in &mut field.classes {
            if *class == SurfaceClass::Snow {
                *class = SurfaceClass::Rock;
            }
        }
        for index in 0..5 {
            field.classes[index * 33 + 32] = SurfaceClass::Snow;
        }
        field.base_classes.clone_from(&field.classes);
        let demoted = field.demote_steep_classes(
            &height_field,
            1_000.0,
            forest_gate(55.0, SteepForestTarget::Snow),
        );
        assert_eq!(demoted.forest_to_snow, 0);
        assert!(demoted.forest_to_rock > 0);
        assert_eq!(field.terrain_at(1.0, 0.5), SurfaceClass::Rock);
    }

    #[test]
    fn snow_gate_regates_forest_the_forest_gate_turned_into_snow() {
        let (height_field, mut field) = snowline_wall();
        // The whole ramp is about 73 degrees: steeper than both limits.
        // Forest above the snowline becomes snow first, then the snow gate
        // demotes it — and the original snowcap — to rock.
        let demoted = field.demote_steep_classes(
            &height_field,
            1_000.0,
            SlopeGates {
                forest_limit_degrees: Some(55.0),
                steep_forest_target: SteepForestTarget::Snow,
                snow_limit_degrees: Some(65.0),
            },
        );
        assert!(demoted.forest_to_snow > 0);
        assert!(demoted.forest_to_rock > 0);
        // The snow gate catches the demoted band and the original snowcap.
        assert!(demoted.snow_to_rock >= demoted.forest_to_snow);
        assert_eq!(field.terrain_at(1.0, 0.5), SurfaceClass::Rock);
        assert_eq!(field.terrain_at(0.9, 0.5), SurfaceClass::Rock);
        assert_eq!(field.terrain_at(1.0, 0.1), SurfaceClass::Rock);
        assert_eq!(field.base_classes, field.classes);
    }

    #[test]
    fn snow_gate_off_keeps_the_forest_gate_result() {
        // With the snow gate off the forest gate behaves exactly as it did
        // before the snow gate existed: forest above the snowline turns
        // snow and stays snow, and the 73-degree original snowcap is never
        // touched.
        let (height_field, mut field) = snowline_wall();
        let demoted = field.demote_steep_classes(
            &height_field,
            1_000.0,
            forest_gate(55.0, SteepForestTarget::Snow),
        );
        assert_eq!(demoted.snow_to_rock, 0);
        assert!(demoted.forest_to_snow > 0);
        assert_eq!(field.terrain_at(0.9, 0.5), SurfaceClass::Snow);
        assert_eq!(field.terrain_at(1.0, 0.1), SurfaceClass::Snow);
    }

    #[test]
    fn slope_gates_with_both_gates_off_change_nothing() {
        let (height_field, mut field) = snowline_wall();
        let before = field.classes.clone();
        let demoted = field.demote_steep_classes(
            &height_field,
            1_000.0,
            SlopeGates {
                forest_limit_degrees: None,
                steep_forest_target: SteepForestTarget::Snow,
                snow_limit_degrees: None,
            },
        );
        assert_eq!(demoted, SlopeGateDemotion::default());
        assert_eq!(field.classes, before);
    }

    #[test]
    fn surface_field_paints_scaled_building_heights() {
        let mut field =
            SurfaceField::new(21, 21, vec![SurfaceClass::Rock; 21 * 21], "test").unwrap();
        field.paint_building(
            &[[0.25, 0.25], [0.75, 0.25], [0.75, 0.75], [0.25, 0.75]],
            12.0,
        );
        assert_eq!(field.at(0.5, 0.5), SurfaceClass::Building);
        assert_eq!(field.building_height_at(0.5, 0.5), 12.0);
        assert_eq!(field.building_height_at(0.76, 0.5), 0.0);
        assert_eq!(field.building_height_at(0.1, 0.1), 0.0);

        let spec = GenerationSpec {
            width_mm: 100.0,
            ground_span_km: 1.0,
            buildings: BuildingSpec {
                enabled: true,
                z_scale: 2.0,
            },
            ..GenerationSpec::default()
        };
        // A 1 km view is well inside the close reference span, so the
        // exaggeration has eased off entirely and a 12 m building prints at
        // true height against the map width: 12 m of 1 km across 100 mm.
        assert!((spec.building_height_scale() - 1.0).abs() < 1e-4);
        let expected = 12.0 * spec.width_mm / 1_000.0;
        assert!(
            (scaled_building_height_mm(&spec, field.building_height_at(0.5, 0.5)) - expected).abs()
                < 0.001
        );
    }

    #[test]
    fn marker_areas_take_priority_over_crossing_roads() {
        let mut field =
            SurfaceField::new(21, 21, vec![SurfaceClass::Rock; 21 * 21], "markers").unwrap();
        field.paint_polyline(&[[0.0, 0.5], [1.0, 0.5]], 100.0, 1.0, SurfaceClass::Road);
        field.paint_surface_area(
            &[[0.45, 0.45], [0.55, 0.45], [0.55, 0.55], [0.45, 0.55]],
            SurfaceClass::Marker,
        );

        assert_eq!(field.class_at(0.5, 0.5), SurfaceClass::Marker);
        assert_eq!(field.class_at(0.25, 0.5), SurfaceClass::Road);
    }

    /// A ferry is a raised ribbon over the water, not a recolouring of the
    /// water. The mesh takes each terrain triangle's material from
    /// `terrain_at`, so a crossing that reached it would paint the surface
    /// teal and then raise the ribbon on top of that.
    #[test]
    fn a_ferry_line_colors_its_ribbon_and_not_the_water_under_it() {
        let mut field = SurfaceField::new(3, 3, vec![SurfaceClass::Water; 9], "ferry").unwrap();
        field.paint_polyline(&[[0.0, 0.5], [1.0, 0.5]], 60.0, 2.0, SurfaceClass::Ferry);

        // The artifact colors that spot as a ferry...
        assert_eq!(field.class_at(0.5, 0.5), SurfaceClass::Ferry);
        // ...while the terrain under it stays the water it crosses.
        assert_eq!(field.terrain_at(0.5, 0.5), SurfaceClass::Water);
        // And the coverage histogram, which reads the overlay sampler, still
        // counts it, so the preview legend can show the layer.
        assert!(field.coverage()[SurfaceClass::Ferry.material_index() as usize] > 0.0);
    }

    /// Where a crossing meets the road at its terminal, the crossing is the
    /// feature worth reading — the rule railways already follow over roads.
    #[test]
    fn a_ferry_outranks_a_road_it_meets() {
        let mut field = SurfaceField::new(3, 3, vec![SurfaceClass::Water; 9], "ferry").unwrap();
        field.paint_polyline(&[[0.0, 0.5], [1.0, 0.5]], 60.0, 2.0, SurfaceClass::Road);
        field.paint_polyline(&[[0.0, 0.5], [1.0, 0.5]], 60.0, 2.0, SurfaceClass::Ferry);
        assert_eq!(field.class_at(0.5, 0.5), SurfaceClass::Ferry);
    }
}
