use anyhow::{Result, bail};
use serde::{Deserialize, Serialize};

const MAX_ADJACENT_GRID_SIDE: u32 = 12;
const AUTO_DETAIL_REFERENCE_SPAN_KM: f64 = 18.0;
const MAX_TERRAIN_SAMPLES_PER_PIECE: u32 = 160;
const MAX_OVERLAY_SAMPLES_PER_PIECE: u32 = 192;
const MIN_ASSEMBLED_SAMPLES: u32 = 256;
const MAX_STANDARD_ASSEMBLED_SAMPLES: u32 = 1_024;
const MAX_ASSEMBLED_SAMPLES: u32 = 2_048;
const MAX_FINE_DEM_ASSEMBLED_SAMPLES: u32 = 2_048;
const FINE_DEM_TARGET_RESOLUTION_M: f64 = 0.25;
const FINE_DEM_MAX_SPAN_KM: f64 = 2.0;
const DETAIL_SAMPLE_STEP: u32 = 8;
/// Caps for imported hiker trails. A GPX track logger easily emits tens of
/// thousands of points; 20k per trail keeps memory and paint time bounded
/// while exceeding one-second GPS logs of any printable route.
const MAX_TRAILS: usize = 20;
const MAX_TRAIL_POINTS: usize = 20_000;
const MAX_TRAIL_NAME_CHARS: usize = 80;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct GenerationSpec {
    pub center_lat: f64,
    pub center_lon: f64,
    pub elevation_source: ElevationSource,
    pub ground_span_km: f64,
    pub width_mm: f32,
    pub rows: u32,
    pub columns: u32,
    pub base_mm: f32,
    pub relief_mm: f32,
    pub elevation_datum_m: Option<f32>,
    pub elevation_m_per_mm: Option<f32>,
    pub adjacent_columns: u32,
    pub adjacent_rows: u32,
    pub super_tile_anchor: SuperTileAnchor,
    pub adjacent_interlocks: bool,
    pub adjacent_tile_column: u32,
    pub adjacent_tile_row: u32,
    pub clearance_mm: f32,
    pub samples_per_piece: u32,
    pub overlay_samples_per_piece: u32,
    pub mesh_samples_across: Option<u32>,
    pub overlay_samples_across: Option<u32>,
    pub fine_dem_detail: bool,
    pub solid_model: bool,
    pub straight_piece_sides: bool,
    pub puzzle_tabs: bool,
    pub place_name: String,
    pub tray: TraySpec,
    pub buildings: BuildingSpec,
    pub color_output: ColorOutputSpec,
    /// Imported hiker trails (GPX/KML routes) drawn on the model in the
    /// dedicated trail color. Empty for every spec saved before the field
    /// existed, so old projects regenerate byte-identically.
    pub trails: Vec<TrailRoute>,
}

impl Default for GenerationSpec {
    fn default() -> Self {
        Self {
            center_lat: 46.8523,
            center_lon: -121.7603,
            elevation_source: ElevationSource::Mapzen,
            ground_span_km: 18.0,
            width_mm: 180.0,
            rows: 3,
            columns: 3,
            base_mm: 2.4,
            relief_mm: 28.0,
            elevation_datum_m: None,
            elevation_m_per_mm: None,
            adjacent_columns: 1,
            adjacent_rows: 1,
            super_tile_anchor: SuperTileAnchor::TopLeft,
            adjacent_interlocks: false,
            adjacent_tile_column: 0,
            adjacent_tile_row: 0,
            clearance_mm: 0.14,
            samples_per_piece: 64,
            overlay_samples_per_piece: 112,
            mesh_samples_across: None,
            overlay_samples_across: None,
            fine_dem_detail: false,
            solid_model: false,
            straight_piece_sides: false,
            puzzle_tabs: true,
            place_name: "Mount Rainier".into(),
            tray: TraySpec::default(),
            buildings: BuildingSpec::default(),
            color_output: ColorOutputSpec::default(),
            trails: Vec::new(),
        }
    }
}

impl GenerationSpec {
    pub fn validate(&self) -> Result<()> {
        if !(-85.0..=85.0).contains(&self.center_lat) {
            bail!("center latitude must be between -85 and 85 degrees");
        }
        if !(-180.0..=180.0).contains(&self.center_lon) {
            bail!("center longitude must be between -180 and 180 degrees");
        }
        if !(0.25..=250.0).contains(&self.ground_span_km) {
            bail!("ground span must be between 0.25 and 250 km");
        }
        if !(60.0..=500.0).contains(&self.width_mm) {
            bail!("model width must be between 60 and 500 mm");
        }
        if !(2..=16).contains(&self.rows) || !(2..=16).contains(&self.columns) {
            bail!("piece rows and columns must each be between 2 and 16");
        }
        if !(1.0..=12.0).contains(&self.base_mm) {
            bail!("base depth must be between 1 and 12 mm");
        }
        if !(1.0..=80.0).contains(&self.relief_mm) {
            bail!("relief must be between 1 and 80 mm");
        }
        match (self.elevation_datum_m, self.elevation_m_per_mm) {
            (Some(datum), Some(metres_per_mm)) => {
                if !(-12_000.0..=12_000.0).contains(&datum) {
                    bail!("elevation datum must be between -12000 and 12000 m");
                }
                if !(0.1..=2_000.0).contains(&metres_per_mm) {
                    bail!("elevation scale must be between 0.1 and 2000 m/mm");
                }
            }
            (None, None) => {}
            _ => bail!("elevation datum and scale must be set together"),
        }
        if !(1..=MAX_ADJACENT_GRID_SIDE).contains(&self.adjacent_columns)
            || !(1..=MAX_ADJACENT_GRID_SIDE).contains(&self.adjacent_rows)
        {
            bail!(
                "super-tile grid columns and rows must each be between 1 and {MAX_ADJACENT_GRID_SIDE}"
            );
        }
        if self.adjacent_tile_column >= self.adjacent_columns
            || self.adjacent_tile_row >= self.adjacent_rows
        {
            bail!("super-tile position must be inside its grid");
        }
        if self.super_tile_anchor == SuperTileAnchor::Center
            && (self.adjacent_columns.is_multiple_of(2) || self.adjacent_rows.is_multiple_of(2))
        {
            bail!("center-anchored super-tile grids require odd column and row counts");
        }
        if !(0.0..=0.8).contains(&self.clearance_mm) {
            bail!("clearance must be between 0 and 0.8 mm");
        }
        if !(16..=MAX_TERRAIN_SAMPLES_PER_PIECE).contains(&self.samples_per_piece) {
            bail!("samples per piece must be between 16 and {MAX_TERRAIN_SAMPLES_PER_PIECE}");
        }
        if !(32..=MAX_OVERLAY_SAMPLES_PER_PIECE).contains(&self.overlay_samples_per_piece) {
            bail!(
                "overlay samples per piece must be between 32 and {MAX_OVERLAY_SAMPLES_PER_PIECE}"
            );
        }
        if self.mesh_samples_across.is_some_and(|samples| {
            !(MIN_ASSEMBLED_SAMPLES..=MAX_ASSEMBLED_SAMPLES).contains(&samples)
        }) {
            bail!(
                "assembled mesh samples must be between {MIN_ASSEMBLED_SAMPLES} and {MAX_ASSEMBLED_SAMPLES}"
            );
        }
        if self.overlay_samples_across.is_some_and(|samples| {
            !(MIN_ASSEMBLED_SAMPLES..=MAX_ASSEMBLED_SAMPLES).contains(&samples)
        }) {
            bail!(
                "assembled overlay samples must be between {MIN_ASSEMBLED_SAMPLES} and {MAX_ASSEMBLED_SAMPLES}"
            );
        }
        if self.place_name.trim().is_empty() || self.place_name.chars().count() > 48 {
            bail!("place label must contain between 1 and 48 characters");
        }
        if self.place_name.chars().any(char::is_control) {
            bail!("place label cannot contain control characters");
        }
        self.tray.validate()?;
        self.buildings.validate()?;
        self.color_output.validate()?;
        if self.trails.len() > MAX_TRAILS {
            bail!("imported trail count must be at most {MAX_TRAILS}");
        }
        for trail in &self.trails {
            trail.validate()?;
        }
        Ok(())
    }

    /// Whether the spec carries imported hiker trails. Every trail-only
    /// behavior — the seventh color slot, its paint code, the extra
    /// filament settings — hangs off this, so specs without trails produce
    /// byte-identical artifacts to builds that predate trails.
    pub fn uses_trails(&self) -> bool {
        !self.trails.is_empty()
    }

    pub fn height_mm(&self) -> f32 {
        self.width_mm * self.rows as f32 / self.columns as f32
    }

    pub fn effective_samples_per_piece(&self) -> u32 {
        if self.uses_color_materials() {
            self.terrain_samples_per_piece()
                .max(self.overlay_samples_per_piece())
        } else {
            self.terrain_samples_per_piece()
        }
    }

    pub fn terrain_samples_per_piece(&self) -> u32 {
        let piece_count = self.mesh_piece_count();
        let mut total = if let Some(samples) = self.mesh_samples_across {
            samples
        } else {
            let base_total = if self.solid_model {
                self.samples_per_piece.saturating_mul(4)
            } else {
                self.samples_per_piece.saturating_mul(piece_count)
            };
            scale_detail_samples(
                base_total,
                self.ground_span_km,
                MAX_STANDARD_ASSEMBLED_SAMPLES,
            )
        };
        if self.fine_dem_detail_active() {
            let fine_total =
                (self.ground_span_km * 1_000.0 / FINE_DEM_TARGET_RESOLUTION_M).ceil() as u32;
            total = total.max(fine_total.min(MAX_FINE_DEM_ASSEMBLED_SAMPLES));
        }
        samples_per_piece_for_total(total, piece_count)
    }

    pub fn overlay_samples_per_piece(&self) -> u32 {
        let piece_count = self.mesh_piece_count();
        let total = if let Some(samples) = self.overlay_samples_across {
            samples
        } else {
            let base_total = if self.solid_model {
                self.overlay_samples_per_piece
            } else {
                self.overlay_samples_per_piece.saturating_mul(piece_count)
            };
            scale_detail_samples(
                base_total,
                self.ground_span_km,
                MAX_STANDARD_ASSEMBLED_SAMPLES,
            )
        };
        samples_per_piece_for_total(total, piece_count)
    }

    pub fn assembled_terrain_samples(&self) -> u32 {
        self.terrain_samples_per_piece()
            .saturating_mul(self.mesh_piece_count())
    }

    pub fn assembled_overlay_samples(&self) -> u32 {
        self.overlay_samples_per_piece()
            .saturating_mul(self.mesh_piece_count())
    }

    pub fn sample_grid_dimensions(&self, samples_per_piece: u32) -> (usize, usize) {
        let columns = if self.solid_model { 1 } else { self.columns };
        let rows = if self.solid_model { 1 } else { self.rows };
        (
            (columns * samples_per_piece + 1) as usize,
            (rows * samples_per_piece + 1) as usize,
        )
    }

    pub fn fine_dem_detail_active(&self) -> bool {
        self.fine_dem_detail
            && self.elevation_source == ElevationSource::Mapterhorn
            && self.ground_span_km <= FINE_DEM_MAX_SPAN_KM
            && self.mesh_samples_across != Some(MAX_ASSEMBLED_SAMPLES)
    }

    pub(crate) fn uses_color_materials(&self) -> bool {
        self.color_output.enabled || self.buildings.enabled || self.uses_trails()
    }

    /// The color of every emitted material slot, ordered by
    /// `SurfaceClass::material_index`. The trail slot exists only when the
    /// spec carries trails, so slot count and colors stay exactly as before
    /// for every spec without them.
    pub(crate) fn material_colors(&self) -> Vec<&str> {
        let mut colors = vec![
            self.color_output.rock_color.as_str(),
            self.color_output.forest_color.as_str(),
            self.color_output.snow_color.as_str(),
            self.color_output.water_color.as_str(),
            self.color_output.road_color.as_str(),
            self.color_output.building_color.as_str(),
        ];
        if self.uses_trails() {
            colors.push(self.color_output.trail_color.as_str());
        }
        colors
    }

    fn mesh_piece_count(&self) -> u32 {
        if self.solid_model {
            1
        } else {
            self.rows.max(self.columns)
        }
    }
}

fn scale_detail_samples(base: u32, ground_span_km: f64, maximum: u32) -> u32 {
    let scale = (AUTO_DETAIL_REFERENCE_SPAN_KM / ground_span_km.max(0.5)).max(1.0);
    let scaled = (f64::from(base) * scale).ceil() as u32;
    scaled
        .div_ceil(DETAIL_SAMPLE_STEP)
        .saturating_mul(DETAIL_SAMPLE_STEP)
        .max(base)
        .min(maximum)
}

fn samples_per_piece_for_total(total: u32, piece_count: u32) -> u32 {
    total.div_ceil(piece_count.max(1))
}

/// One imported hiker trail: an ordered polyline of geographic positions.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TrailRoute {
    pub name: String,
    /// Ordered (latitude, longitude) pairs in degrees.
    pub points: Vec<[f64; 2]>,
}

impl TrailRoute {
    fn validate(&self) -> Result<()> {
        if self.name.trim().is_empty() || self.name.chars().count() > MAX_TRAIL_NAME_CHARS {
            bail!("trail names must contain between 1 and {MAX_TRAIL_NAME_CHARS} characters");
        }
        if self.name.chars().any(char::is_control) {
            bail!("trail names cannot contain control characters");
        }
        if self.points.len() < 2 {
            bail!("trail point counts must be between 2 and {MAX_TRAIL_POINTS}");
        }
        if self.points.len() > MAX_TRAIL_POINTS {
            bail!("trail point counts must be between 2 and {MAX_TRAIL_POINTS}");
        }
        for point in &self.points {
            let [latitude, longitude] = *point;
            if !latitude.is_finite() || !longitude.is_finite() {
                bail!("trail coordinates must be finite");
            }
            if !(-90.0..=90.0).contains(&latitude) {
                bail!("trail latitudes must be between -90 and 90 degrees");
            }
            if !(-180.0..=180.0).contains(&longitude) {
                bail!("trail longitudes must be between -180 and 180 degrees");
            }
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ElevationSource {
    #[default]
    Mapzen,
    Mapterhorn,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SuperTileAnchor {
    #[default]
    TopLeft,
    Center,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct BuildingSpec {
    pub enabled: bool,
    pub z_scale: f32,
}

impl Default for BuildingSpec {
    fn default() -> Self {
        Self {
            enabled: false,
            z_scale: 5.0,
        }
    }
}

impl BuildingSpec {
    fn validate(&self) -> Result<()> {
        if !(0.5..=30.0).contains(&self.z_scale) {
            bail!("building Z scale must be between 0.5 and 30");
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct TraySpec {
    pub enabled: bool,
    pub individual_tiles: bool,
    pub tray_color: String,
    pub contour_color: String,
    pub label_color: String,
    pub clearance_mm: f32,
    pub rim_width_mm: f32,
    pub floor_mm: f32,
    pub rim_height_mm: f32,
    pub contour_count: u32,
    pub segment_columns: u32,
    pub segment_rows: u32,
}

impl Default for TraySpec {
    fn default() -> Self {
        Self {
            enabled: false,
            individual_tiles: false,
            tray_color: "#252822".into(),
            contour_color: "#E7E4D8".into(),
            label_color: "#F4F3EC".into(),
            clearance_mm: 0.6,
            rim_width_mm: 8.0,
            floor_mm: 1.6,
            rim_height_mm: 3.2,
            contour_count: 18,
            segment_columns: 1,
            segment_rows: 1,
        }
    }
}

impl TraySpec {
    pub(crate) fn validate(&self) -> Result<()> {
        for (name, color) in [
            ("tray", &self.tray_color),
            ("contour", &self.contour_color),
            ("tray label", &self.label_color),
        ] {
            if !valid_hex_color(color) {
                bail!("{name} color must use #RRGGBB");
            }
        }
        if !(0.2..=2.0).contains(&self.clearance_mm) {
            bail!("tray clearance must be between 0.2 and 2 mm");
        }
        if !(5.0..=16.0).contains(&self.rim_width_mm) {
            bail!("tray rim width must be between 5 and 16 mm");
        }
        if !(1.0..=4.0).contains(&self.floor_mm) {
            bail!("tray floor must be between 1 and 4 mm");
        }
        if !(2.0..=8.0).contains(&self.rim_height_mm) {
            bail!("tray rim height must be between 2 and 8 mm");
        }
        if !(5..=60).contains(&self.contour_count) {
            bail!("tray contour count must be between 5 and 60");
        }
        if !(1..=MAX_ADJACENT_GRID_SIDE).contains(&self.segment_columns)
            || !(1..=MAX_ADJACENT_GRID_SIDE).contains(&self.segment_rows)
        {
            bail!(
                "tray segment columns and rows must each be between 1 and {MAX_ADJACENT_GRID_SIDE}"
            );
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BridgeStructure {
    #[default]
    Floating,
    Supported,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RoadDetail {
    #[default]
    Automatic,
    Major,
    Minor,
    Streets,
    All,
}

/// A road detail level with `Automatic` already resolved against the map
/// span, so consumers never have to handle the automatic case.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResolvedRoadDetail {
    Major,
    Minor,
    Streets,
    All,
}

impl RoadDetail {
    pub fn resolve(self, ground_span_km: f64) -> ResolvedRoadDetail {
        match self {
            Self::Major => ResolvedRoadDetail::Major,
            Self::Minor => ResolvedRoadDetail::Minor,
            Self::Streets => ResolvedRoadDetail::Streets,
            Self::All => ResolvedRoadDetail::All,
            // These span thresholds are mirrored by automaticRoadDetail in
            // app/terrain/config.ts for the UI hint; change both together.
            Self::Automatic => {
                if ground_span_km <= 2.0 {
                    ResolvedRoadDetail::All
                } else if ground_span_km <= 8.0 {
                    ResolvedRoadDetail::Streets
                } else if ground_span_km <= 20.0 {
                    ResolvedRoadDetail::Minor
                } else {
                    ResolvedRoadDetail::Major
                }
            }
        }
    }
}

impl ResolvedRoadDetail {
    pub fn name(self) -> &'static str {
        match self {
            Self::Major => "major",
            Self::Minor => "minor",
            Self::Streets => "streets",
            Self::All => "all",
        }
    }
}

/// How raster land-cover class borders are drawn. `Blocky` keeps the
/// nearest-sample staircase borders of the 10 m source pixels; `Smooth`
/// re-estimates every sample by ordinary kriging of per-class indicators on
/// the recovered native-resolution grid, which bends borders into curves
/// that still honour the source pixels.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ClassBorders {
    Blocky,
    #[default]
    Smooth,
}

/// What steep forest demoted by the slope gate becomes. `Rock` recolors
/// every demoted sample; `Snow` recolors demoted samples above the local
/// snowline as snow instead, so shaded couloirs and icefields WorldCover
/// paints green print white like their surroundings. Below the snowline (or
/// when the scene has no snow) `Snow` still falls back to rock.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SteepForestTarget {
    #[default]
    Rock,
    Snow,
}

/// What the color 3MF carries beyond core-spec geometry and its color group.
///
/// `Project` stays the default so existing users — above all Bambu Studio
/// users who rely on one-click color setups — keep getting exactly what they
/// get today: OrcaSlicer/Bambu face-paint codes plus an embedded
/// `Metadata/project_settings.config` with filament colours and purge
/// volumes. Slicers treat that archive as a full project, so importing it
/// also pulls printer, material, and process preset state. `Painted` keeps
/// the per-triangle paint codes but drops the embedded settings for users
/// who don't want the import touching their presets; `Geometry` drops the
/// paint codes too and leaves a plain standards-based 3MF.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ThreeMfStyle {
    /// Color group plus slicer face-paint codes; no embedded settings.
    Painted,
    /// Today's full output: paint codes plus embedded project settings.
    #[default]
    Project,
    /// Core-spec color group only; cleanest for interchange.
    Geometry,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct ColorOutputSpec {
    pub enabled: bool,
    /// Which 3MF flavour to write. `#[serde(default)]` on the struct fills
    /// this with `Project` for specs saved before the field existed, so
    /// existing users keep today's one-click color behavior unchanged.
    pub threemf_style: ThreeMfStyle,
    pub forest_color: String,
    pub rock_color: String,
    pub snow_color: String,
    pub water_color: String,
    pub road_color: String,
    pub building_color: String,
    /// Color of imported hiker trails. Only emitted into artifacts when the
    /// spec actually carries trails, so the default never changes existing
    /// output.
    pub trail_color: String,
    /// Print width of imported trail lines, like `road_width_mm` for roads.
    pub trail_width_mm: f32,
    pub roads_enabled: bool,
    pub road_detail: RoadDetail,
    pub adaptive_road_widths: bool,
    pub osm_water_enabled: bool,
    pub waterway_coverage_percent: f32,
    pub road_width_mm: f32,
    pub road_height_mm: f32,
    pub bridge_structure: BridgeStructure,
    pub bridge_thickness_mm: f32,
    pub minimum_patch_mm: f32,
    /// Class border drawing and smoothing. Flattened, so the JSON keys stay
    /// at the `color_output` level exactly as before the grouping.
    #[serde(flatten)]
    pub borders: BorderSpec,
    /// Steep-slope reclassification gates. Flattened like `borders`.
    #[serde(flatten)]
    pub slope_gates: SlopeGateSpec,
}

/// How land-cover class borders are drawn and smoothed.
///
/// Serialized flattened into [`ColorOutputSpec`], so this grouping never
/// shows up on the wire.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct BorderSpec {
    /// Defaults to [`ClassBorders::Smooth`], because the smoother carries
    /// its own scale gate: it only redraws a raster that samples each 10 m
    /// land-cover cell at least one and a half times, which is the close
    /// view where single source cells span several print samples and read
    /// as blocks. Wider views sample the land cover more coarsely than the
    /// source itself, so smoothing there would only blur real data; the
    /// setting does nothing and the map keeps the source resolution
    /// untouched.
    pub class_borders: ClassBorders,
    /// How far smoothed borders bend: the indicator-kriging variogram range
    /// in native (10 m) land-cover cells. Larger values bend borders over a
    /// wider window; smaller values keep them tight to the source pixels.
    pub border_smoothing_range_cells: f32,
    /// Indicator-kriging variogram nugget as a fraction of the sill. Higher
    /// values damp the staircase phase noise that nearest-neighbour
    /// upsampling introduces, at the cost of fidelity to single-cell
    /// features at the native nodes.
    pub border_smoothing_nugget: f32,
}

impl Default for BorderSpec {
    fn default() -> Self {
        Self {
            class_borders: ClassBorders::default(),
            // 2.5 cells keeps the estimate local so borders track the data;
            // a 0.05 nugget keeps the estimator near-exact at native nodes
            // while still damping staircase phase noise between them.
            border_smoothing_range_cells: 2.5,
            border_smoothing_nugget: 0.05,
        }
    }
}

impl BorderSpec {
    fn validate(&self) -> Result<()> {
        // Below 1 cell the 4 by 4 kriging neighbourhood barely overlaps the
        // variogram range and borders stay blocky; above 8 cells borders
        // detach from the data the stencil can see.
        if !(1.0..=8.0).contains(&self.border_smoothing_range_cells) {
            bail!("border smoothing range must be between 1 and 8 native cells");
        }
        // Past half the sill the nugget flattens the stencil into a blur
        // that erases single-cell features.
        if !(0.0..=0.5).contains(&self.border_smoothing_nugget) {
            bail!("border smoothing nugget must be between 0 and 0.5");
        }
        Ok(())
    }
}

/// Steep-slope gates that reclassify land cover the 10 m source raster
/// bleeds onto near-vertical faces.
///
/// Serialized flattened into [`ColorOutputSpec`], so this grouping never
/// shows up on the wire.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct SlopeGateSpec {
    /// Reclassify forest as rock where the local ground slope exceeds
    /// `forest_slope_limit_degrees`. Fixes 10 m land-cover pixels that bleed
    /// tree cover onto near-vertical faces (for example the sides of Devils
    /// Tower), so it defaults on.
    pub forest_slope_gate: bool,
    pub forest_slope_limit_degrees: f32,
    /// What demoted steep forest becomes: rock everywhere, or snow above
    /// the local snowline.
    pub steep_forest_target: SteepForestTarget,
    /// Reclassify snow as rock where the local ground slope exceeds
    /// `snow_slope_limit_degrees`. Like the forest gate this only removes a
    /// physical impossibility — snow does not hold on near-vertical faces,
    /// but WorldCover bleeds snow onto cliff walls exactly like tree cover
    /// — so it defaults on. Runs after the forest gate, so forest the
    /// forest gate demoted to snow is gated too.
    pub snow_slope_gate: bool,
    pub snow_slope_limit_degrees: f32,
}

impl Default for SlopeGateSpec {
    fn default() -> Self {
        Self {
            forest_slope_gate: true,
            // Closed forest is rare above roughly 45 degrees and absent from
            // true cliff faces; 55 degrees keeps legitimately steep forested
            // gorge and fjord walls while catching near-vertical rock that
            // 10 m land-cover pixels paint green.
            forest_slope_limit_degrees: 55.0,
            steep_forest_target: SteepForestTarget::Rock,
            snow_slope_gate: true,
            // Snow patches persist on steeper ground than closed forest:
            // couloirs and clingy north faces hold snow past 55 degrees,
            // but faces past roughly 65 degrees shed. DEM smoothing
            // understates slope, so the default errs high.
            snow_slope_limit_degrees: 65.0,
        }
    }
}

impl SlopeGateSpec {
    fn validate(&self) -> Result<()> {
        if !(30.0..=85.0).contains(&self.forest_slope_limit_degrees) {
            bail!("forest slope limit must be between 30 and 85 degrees");
        }
        if !(30.0..=85.0).contains(&self.snow_slope_limit_degrees) {
            bail!("snow slope limit must be between 30 and 85 degrees");
        }
        Ok(())
    }
}

impl Default for ColorOutputSpec {
    fn default() -> Self {
        Self {
            enabled: false,
            threemf_style: ThreeMfStyle::default(),
            forest_color: "#28543A".into(),
            rock_color: "#7C7468".into(),
            snow_color: "#F4F3EC".into(),
            water_color: "#2F76B5".into(),
            road_color: "#D8A33C".into(),
            building_color: "#B8A890".into(),
            // High-vis raspberry magenta: reads on every default terrain
            // color and stays clearly apart from the gold route color.
            trail_color: "#D6336C".into(),
            trail_width_mm: 0.7,
            roads_enabled: true,
            road_detail: RoadDetail::Automatic,
            adaptive_road_widths: true,
            osm_water_enabled: true,
            waterway_coverage_percent: 12.0,
            road_width_mm: 0.7,
            road_height_mm: 0.2,
            bridge_structure: BridgeStructure::Floating,
            bridge_thickness_mm: 1.2,
            minimum_patch_mm: 1.2,
            borders: BorderSpec::default(),
            slope_gates: SlopeGateSpec::default(),
        }
    }
}

impl ColorOutputSpec {
    fn validate(&self) -> Result<()> {
        for (name, color) in [
            ("forest", &self.forest_color),
            ("rock", &self.rock_color),
            ("snow", &self.snow_color),
            ("water", &self.water_color),
            ("road", &self.road_color),
            ("building", &self.building_color),
            ("trail", &self.trail_color),
        ] {
            if !valid_hex_color(color) {
                bail!("{name} color must use #RRGGBB");
            }
        }
        if !(0.4..=5.0).contains(&self.road_width_mm) {
            bail!("road line width must be between 0.4 and 5 mm");
        }
        if !(0.4..=5.0).contains(&self.trail_width_mm) {
            bail!("trail line width must be between 0.4 and 5 mm");
        }
        if !(0.0..=100.0).contains(&self.waterway_coverage_percent) {
            bail!("waterway coverage cutoff must be between 0 and 100 percent");
        }
        if !(0.08..=0.4).contains(&self.road_height_mm) {
            bail!("road layer height must be between 0.08 and 0.4 mm");
        }
        if !(0.4..=6.0).contains(&self.bridge_thickness_mm) {
            bail!("floating bridge thickness must be between 0.4 and 6 mm");
        }
        if !(0.4..=8.0).contains(&self.minimum_patch_mm) {
            bail!("minimum color patch must be between 0.4 and 8 mm");
        }
        self.slope_gates.validate()?;
        self.borders.validate()?;
        Ok(())
    }
}

fn valid_hex_color(color: &str) -> bool {
    color.len() == 7
        && color.starts_with('#')
        && color[1..].bytes().all(|byte| byte.is_ascii_hexdigit())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SurfaceClass {
    Rock,
    Forest,
    Snow,
    Water,
    Road,
    Building,
    /// Imported hiker trails. The class only ever appears in generated
    /// output when a spec carries trails, so its seventh material slot is
    /// invisible to every existing project.
    Trail,
}

impl SurfaceClass {
    /// Every class, ordered by `material_index`.
    pub(crate) const ALL: [Self; 7] = [
        Self::Rock,
        Self::Forest,
        Self::Snow,
        Self::Water,
        Self::Road,
        Self::Building,
        Self::Trail,
    ];

    pub(crate) fn material_index(self) -> u32 {
        match self {
            Self::Rock => 0,
            Self::Forest => 1,
            Self::Snow => 2,
            Self::Water => 3,
            Self::Road => 4,
            Self::Building => 5,
            Self::Trail => 6,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_the_full_relief_range() {
        let mut spec = GenerationSpec {
            relief_mm: 80.0,
            ..GenerationSpec::default()
        };
        assert!(spec.validate().is_ok());

        spec.relief_mm = 80.1;
        assert!(spec.validate().is_err());
    }

    #[test]
    fn shared_height_frame_requires_a_datum_and_scale() {
        let mut spec = GenerationSpec {
            elevation_datum_m: Some(100.0),
            ..GenerationSpec::default()
        };
        assert!(spec.validate().is_err());

        spec.elevation_m_per_mm = Some(25.0);
        assert!(spec.validate().is_ok());
    }

    #[test]
    fn accepts_super_tile_grids_up_to_twelve_by_twelve() {
        let mut spec = GenerationSpec {
            adjacent_columns: 12,
            adjacent_rows: 12,
            adjacent_tile_column: 11,
            adjacent_tile_row: 11,
            ..GenerationSpec::default()
        };
        assert!(spec.validate().is_ok());

        spec.adjacent_columns = 13;
        assert!(spec.validate().is_err());
    }

    #[test]
    fn center_anchored_super_tiles_require_odd_dimensions() {
        let mut spec = GenerationSpec {
            adjacent_columns: 5,
            adjacent_rows: 3,
            super_tile_anchor: SuperTileAnchor::Center,
            ..GenerationSpec::default()
        };
        assert!(spec.validate().is_ok());

        spec.adjacent_columns = 4;
        let error = spec.validate().unwrap_err().to_string();
        assert!(error.contains("require odd column and row counts"));
    }

    #[test]
    fn old_color_specs_gain_new_default_colors() {
        // An empty document is the default spec, field for field.
        let empty: GenerationSpec = serde_json::from_str("{}").unwrap();
        assert_eq!(
            serde_json::to_value(&empty).unwrap(),
            serde_json::to_value(GenerationSpec::default()).unwrap()
        );

        // A pre-color-era document that already sets some color_output keys
        // keeps them and gains the defaults for every field it omits.
        let spec: GenerationSpec = serde_json::from_value(serde_json::json!({
            "color_output": {
                "enabled": true,
                "forest_color": "#28543A",
                "rock_color": "#7C7468",
                "snow_color": "#F4F3EC",
                "minimum_patch_mm": 1.2
            }
        }))
        .unwrap();
        let mut expected = GenerationSpec::default();
        expected.color_output.enabled = true;
        assert_eq!(
            serde_json::to_value(&spec).unwrap(),
            serde_json::to_value(&expected).unwrap()
        );

        // Spot checks on the defaults whose stability matters most. Specs
        // saved before the 3MF style existed keep today's project output,
        // embedded slicer settings included; specs saved before trails
        // existed carry none and stay off the trail-only code paths.
        //
        // Class borders are the deliberate exception: old saved setups
        // regenerate with smoothed borders, because smoothing now defaults
        // on and gates itself by scale. Close views change; wide views,
        // where the gate declines, come out byte for byte as before.
        assert_eq!(spec.place_name, "Mount Rainier");
        assert_eq!(spec.color_output.threemf_style, ThreeMfStyle::Project);
        assert_eq!(
            spec.color_output.borders.class_borders,
            ClassBorders::Smooth
        );
        assert!(spec.trails.is_empty());
        assert!(!spec.uses_trails());
        assert_eq!(spec.color_output.trail_color, "#D6336C");
    }

    /// The exact serialization of `GenerationSpec::default()` captured
    /// before `ColorOutputSpec` grew its flattened sub-structs. Byte
    /// equality proves the grouping never touched the wire format: every
    /// key flat, every key in the old order.
    #[test]
    fn default_spec_serializes_to_the_exact_flat_wire_format() {
        let expected = r##"{"center_lat":46.8523,"center_lon":-121.7603,"elevation_source":"mapzen","ground_span_km":18.0,"width_mm":180.0,"rows":3,"columns":3,"base_mm":2.4,"relief_mm":28.0,"elevation_datum_m":null,"elevation_m_per_mm":null,"adjacent_columns":1,"adjacent_rows":1,"super_tile_anchor":"top_left","adjacent_interlocks":false,"adjacent_tile_column":0,"adjacent_tile_row":0,"clearance_mm":0.14,"samples_per_piece":64,"overlay_samples_per_piece":112,"mesh_samples_across":null,"overlay_samples_across":null,"fine_dem_detail":false,"solid_model":false,"straight_piece_sides":false,"puzzle_tabs":true,"place_name":"Mount Rainier","tray":{"enabled":false,"individual_tiles":false,"tray_color":"#252822","contour_color":"#E7E4D8","label_color":"#F4F3EC","clearance_mm":0.6,"rim_width_mm":8.0,"floor_mm":1.6,"rim_height_mm":3.2,"contour_count":18,"segment_columns":1,"segment_rows":1},"buildings":{"enabled":false,"z_scale":5.0},"color_output":{"enabled":false,"threemf_style":"project","forest_color":"#28543A","rock_color":"#7C7468","snow_color":"#F4F3EC","water_color":"#2F76B5","road_color":"#D8A33C","building_color":"#B8A890","trail_color":"#D6336C","trail_width_mm":0.7,"roads_enabled":true,"road_detail":"automatic","adaptive_road_widths":true,"osm_water_enabled":true,"waterway_coverage_percent":12.0,"road_width_mm":0.7,"road_height_mm":0.2,"bridge_structure":"floating","bridge_thickness_mm":1.2,"minimum_patch_mm":1.2,"class_borders":"smooth","border_smoothing_range_cells":2.5,"border_smoothing_nugget":0.05,"forest_slope_gate":true,"forest_slope_limit_degrees":55.0,"steep_forest_target":"rock","snow_slope_gate":true,"snow_slope_limit_degrees":65.0},"trails":[]}"##;
        let serialized = serde_json::to_string(&GenerationSpec::default()).unwrap();
        assert_eq!(serialized, expected);
    }

    /// A JSON document that sets every spec field, flat `color_output` keys
    /// included, must survive a full round-trip unchanged.
    #[test]
    fn full_flat_documents_round_trip_unchanged() {
        let doc: serde_json::Value = serde_json::from_str(
            r##"{
            "center_lat": 44.5,
            "center_lon": -110.5,
            "elevation_source": "mapterhorn",
            "ground_span_km": 6.0,
            "width_mm": 240.0,
            "rows": 5,
            "columns": 5,
            "base_mm": 3.0,
            "relief_mm": 20.0,
            "elevation_datum_m": 1200.0,
            "elevation_m_per_mm": 40.0,
            "adjacent_columns": 3,
            "adjacent_rows": 3,
            "super_tile_anchor": "center",
            "adjacent_interlocks": true,
            "adjacent_tile_column": 1,
            "adjacent_tile_row": 2,
            "clearance_mm": 0.25,
            "samples_per_piece": 96,
            "overlay_samples_per_piece": 128,
            "mesh_samples_across": 512,
            "overlay_samples_across": 640,
            "fine_dem_detail": true,
            "solid_model": true,
            "straight_piece_sides": true,
            "puzzle_tabs": false,
            "place_name": "Yellowstone",
            "tray": {
                "enabled": true,
                "individual_tiles": true,
                "tray_color": "#111111",
                "contour_color": "#222222",
                "label_color": "#333333",
                "clearance_mm": 0.5,
                "rim_width_mm": 9.0,
                "floor_mm": 2.0,
                "rim_height_mm": 4.0,
                "contour_count": 24,
                "segment_columns": 2,
                "segment_rows": 3
            },
            "buildings": { "enabled": true, "z_scale": 2.0 },
            "color_output": {
                "enabled": true,
                "threemf_style": "painted",
                "forest_color": "#014421",
                "rock_color": "#6E6E6E",
                "snow_color": "#FFFFFF",
                "water_color": "#0055AA",
                "road_color": "#AA8800",
                "building_color": "#997755",
                "trail_color": "#CC2266",
                "trail_width_mm": 1.5,
                "roads_enabled": false,
                "road_detail": "streets",
                "adaptive_road_widths": false,
                "osm_water_enabled": false,
                "waterway_coverage_percent": 25.0,
                "road_width_mm": 1.0,
                "road_height_mm": 0.25,
                "bridge_structure": "supported",
                "bridge_thickness_mm": 2.0,
                "minimum_patch_mm": 2.5,
                "class_borders": "smooth",
                "border_smoothing_range_cells": 4.0,
                "border_smoothing_nugget": 0.25,
                "forest_slope_gate": false,
                "forest_slope_limit_degrees": 60.0,
                "steep_forest_target": "snow",
                "snow_slope_gate": false,
                "snow_slope_limit_degrees": 70.0
            },
            "trails": [
                {
                    "name": "Loop",
                    "points": [[44.5, -110.5], [44.6, -110.25]]
                }
            ]
        }"##,
        )
        .unwrap();
        let spec: GenerationSpec = serde_json::from_value(doc.clone()).unwrap();
        spec.validate().unwrap();
        assert_eq!(
            spec.color_output.borders.class_borders,
            ClassBorders::Smooth
        );
        assert_eq!(
            spec.color_output.slope_gates.steep_forest_target,
            SteepForestTarget::Snow
        );
        assert_eq!(serde_json::to_value(&spec).unwrap(), doc);
    }

    fn trail(points: Vec<[f64; 2]>) -> TrailRoute {
        TrailRoute {
            name: "Wonderland Trail".into(),
            points,
        }
    }

    #[test]
    fn trail_specs_validate_coordinates_and_caps() {
        let mut spec = GenerationSpec {
            trails: vec![trail(vec![[46.85, -121.76], [46.86, -121.75]])],
            ..GenerationSpec::default()
        };
        assert!(spec.validate().is_ok());
        assert!(spec.uses_trails());

        spec.trails[0].points = vec![[46.85, -121.76]];
        let error = spec.validate().unwrap_err().to_string();
        assert!(error.contains("must be between 2 and"));

        spec.trails[0].points = vec![[91.0, 0.0], [0.0, 0.0]];
        let error = spec.validate().unwrap_err().to_string();
        assert!(error.contains("latitudes"));

        spec.trails[0].points = vec![[0.0, -181.0], [0.0, 0.0]];
        let error = spec.validate().unwrap_err().to_string();
        assert!(error.contains("longitudes"));

        spec.trails[0].points = vec![[f64::NAN, 0.0], [0.0, 0.0]];
        let error = spec.validate().unwrap_err().to_string();
        assert!(error.contains("finite"));

        spec.trails[0].points = vec![[0.0, 0.0]; 20_001];
        let error = spec.validate().unwrap_err().to_string();
        assert!(error.contains("must be between 2 and 20000"));

        spec.trails = vec![trail(vec![[46.85, -121.76], [46.86, -121.75]]); 21];
        let error = spec.validate().unwrap_err().to_string();
        assert!(error.contains("must be at most 20"));

        spec.trails.truncate(20);
        assert!(spec.validate().is_ok());

        spec.trails.truncate(1);
        spec.trails[0].name = "  ".into();
        let error = spec.validate().unwrap_err().to_string();
        assert!(error.contains("trail names"));

        spec.trails[0].name = "OK".into();
        spec.color_output.trail_width_mm = 0.2;
        let error = spec.validate().unwrap_err().to_string();
        assert!(error.contains("trail line width"));

        spec.color_output.trail_width_mm = 0.7;
        spec.color_output.trail_color = "magenta".into();
        let error = spec.validate().unwrap_err().to_string();
        assert!(error.contains("trail color"));
    }

    #[test]
    fn trails_activate_color_materials_on_their_own() {
        let mut spec = GenerationSpec::default();
        spec.color_output.enabled = false;
        assert!(!spec.uses_color_materials());
        spec.trails = vec![trail(vec![[46.85, -121.76], [46.86, -121.75]])];
        assert!(spec.uses_color_materials());
    }

    #[test]
    fn threemf_style_defaults_to_project_and_parses_as_snake_case() {
        assert_eq!(
            ColorOutputSpec::default().threemf_style,
            ThreeMfStyle::Project
        );
        let spec: GenerationSpec = serde_json::from_value(serde_json::json!({
            "color_output": { "threemf_style": "painted" }
        }))
        .unwrap();
        assert_eq!(spec.color_output.threemf_style, ThreeMfStyle::Painted);
        let spec: GenerationSpec = serde_json::from_value(serde_json::json!({
            "color_output": { "threemf_style": "geometry" }
        }))
        .unwrap();
        assert_eq!(spec.color_output.threemf_style, ThreeMfStyle::Geometry);
        assert_eq!(
            serde_json::to_value(ThreeMfStyle::Project).unwrap(),
            serde_json::json!("project")
        );
    }

    #[test]
    fn steep_forest_targets_parse_as_snake_case() {
        let spec: GenerationSpec = serde_json::from_value(serde_json::json!({
            "color_output": { "steep_forest_target": "snow" }
        }))
        .unwrap();
        assert_eq!(
            spec.color_output.slope_gates.steep_forest_target,
            SteepForestTarget::Snow
        );
        assert_eq!(
            serde_json::to_value(SteepForestTarget::Rock).unwrap(),
            serde_json::json!("rock")
        );
    }

    #[test]
    fn class_border_modes_parse_and_slope_limits_validate() {
        let spec: GenerationSpec = serde_json::from_value(serde_json::json!({
            "color_output": {
                "class_borders": "smooth",
                "forest_slope_gate": false,
                "snow_slope_gate": false
            }
        }))
        .unwrap();
        assert_eq!(
            spec.color_output.borders.class_borders,
            ClassBorders::Smooth
        );
        assert!(!spec.color_output.slope_gates.forest_slope_gate);
        assert!(!spec.color_output.slope_gates.snow_slope_gate);
        assert_eq!(spec.color_output.borders.border_smoothing_range_cells, 2.5);
        assert_eq!(spec.color_output.borders.border_smoothing_nugget, 0.05);

        let mut spec = GenerationSpec::default();
        spec.color_output.slope_gates.forest_slope_limit_degrees = 20.0;
        assert!(spec.validate().is_err());
        spec.color_output.slope_gates.forest_slope_limit_degrees = 85.0;
        assert!(spec.validate().is_ok());

        spec.color_output.slope_gates.snow_slope_limit_degrees = 89.0;
        let error = spec.validate().unwrap_err().to_string();
        assert!(error.contains("snow slope limit"));
        spec.color_output.slope_gates.snow_slope_limit_degrees = 30.0;
        assert!(spec.validate().is_ok());
    }

    #[test]
    fn border_smoothing_parameters_validate() {
        let mut spec = GenerationSpec::default();
        spec.color_output.borders.border_smoothing_range_cells = 0.5;
        let error = spec.validate().unwrap_err().to_string();
        assert!(error.contains("between 1 and 8 native cells"));
        spec.color_output.borders.border_smoothing_range_cells = 8.0;
        assert!(spec.validate().is_ok());

        spec.color_output.borders.border_smoothing_nugget = 0.6;
        let error = spec.validate().unwrap_err().to_string();
        assert!(error.contains("between 0 and 0.5"));
        spec.color_output.borders.border_smoothing_nugget = 0.0;
        assert!(spec.validate().is_ok());
    }

    #[test]
    fn automatic_road_detail_tracks_ground_span() {
        assert_eq!(RoadDetail::Automatic.resolve(1.0), ResolvedRoadDetail::All);
        assert_eq!(
            RoadDetail::Automatic.resolve(4.0),
            ResolvedRoadDetail::Streets
        );
        assert_eq!(
            RoadDetail::Automatic.resolve(18.0),
            ResolvedRoadDetail::Minor
        );
        assert_eq!(
            RoadDetail::Automatic.resolve(40.0),
            ResolvedRoadDetail::Major
        );
        assert_eq!(
            RoadDetail::Streets.resolve(40.0),
            ResolvedRoadDetail::Streets
        );
    }

    #[test]
    fn overlays_use_their_independent_detail_level() {
        let mut spec = GenerationSpec::default();
        assert_eq!(spec.effective_samples_per_piece(), 64);
        spec.color_output.enabled = true;
        assert_eq!(spec.effective_samples_per_piece(), 112);
        spec.overlay_samples_per_piece = 48;
        assert_eq!(spec.effective_samples_per_piece(), 64);
    }

    #[test]
    fn close_views_raise_mesh_detail_with_safe_caps() {
        let mut spec = GenerationSpec::default();
        assert_eq!(spec.terrain_samples_per_piece(), 64);
        assert_eq!(spec.overlay_samples_per_piece(), 112);

        spec.ground_span_km = 9.0;
        assert_eq!(spec.terrain_samples_per_piece(), 128);
        assert_eq!(spec.overlay_samples_per_piece(), 224);

        spec.ground_span_km = 4.5;
        assert_eq!(spec.terrain_samples_per_piece(), 256);
        assert_eq!(spec.overlay_samples_per_piece(), 342);

        spec.ground_span_km = 36.0;
        assert_eq!(spec.terrain_samples_per_piece(), 64);
        assert_eq!(spec.overlay_samples_per_piece(), 112);
    }

    #[test]
    fn assembled_detail_cap_does_not_grow_with_piece_count() {
        let spec = GenerationSpec {
            rows: 10,
            columns: 10,
            ground_span_km: 9.0,
            color_output: ColorOutputSpec {
                enabled: true,
                ..ColorOutputSpec::default()
            },
            mesh_samples_across: Some(1_024),
            overlay_samples_across: Some(1_024),
            ..GenerationSpec::default()
        };

        assert_eq!(spec.terrain_samples_per_piece(), 103);
        assert_eq!(spec.overlay_samples_per_piece(), 103);
        assert_eq!(spec.assembled_terrain_samples(), 1_030);
        assert_eq!(spec.effective_samples_per_piece(), 103);
    }

    #[test]
    fn solid_and_fine_dem_modes_get_explicit_total_budgets() {
        let mut solid = GenerationSpec {
            solid_model: true,
            ground_span_km: 9.0,
            mesh_samples_across: Some(640),
            overlay_samples_across: Some(640),
            ..GenerationSpec::default()
        };
        assert_eq!(solid.assembled_terrain_samples(), 640);
        assert_eq!(solid.sample_grid_dimensions(640), (641, 641));

        solid.ground_span_km = 0.5;
        solid.elevation_source = ElevationSource::Mapterhorn;
        solid.fine_dem_detail = true;
        assert_eq!(solid.assembled_terrain_samples(), 2_000);

        solid.elevation_source = ElevationSource::Mapzen;
        assert_eq!(solid.assembled_terrain_samples(), 640);

        solid.elevation_source = ElevationSource::Mapterhorn;
        solid.ground_span_km = 3.0;
        assert_eq!(solid.assembled_terrain_samples(), 640);

        let puzzle = GenerationSpec {
            rows: 8,
            columns: 10,
            mesh_samples_across: Some(640),
            overlay_samples_across: Some(640),
            ..GenerationSpec::default()
        };
        assert_eq!(puzzle.sample_grid_dimensions(64), (641, 513));
        assert_eq!(puzzle.assembled_terrain_samples(), 640);

        let ultra = GenerationSpec {
            mesh_samples_across: Some(2_048),
            overlay_samples_across: Some(2_048),
            ..GenerationSpec::default()
        };
        ultra.validate().unwrap();
    }
}
