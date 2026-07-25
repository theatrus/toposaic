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
        Ok(())
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
        self.color_output.enabled || self.buildings.enabled
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
    #[default]
    Blocky,
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

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct ColorOutputSpec {
    pub enabled: bool,
    pub forest_color: String,
    pub rock_color: String,
    pub snow_color: String,
    pub water_color: String,
    pub road_color: String,
    pub building_color: String,
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
    /// Reclassify forest as rock where the local ground slope exceeds
    /// `forest_slope_limit_degrees`. Fixes 10 m land-cover pixels that bleed
    /// tree cover onto near-vertical faces (for example the sides of Devils
    /// Tower), so it defaults on.
    pub forest_slope_gate: bool,
    pub forest_slope_limit_degrees: f32,
    /// What demoted steep forest becomes: rock everywhere, or snow above
    /// the local snowline.
    pub steep_forest_target: SteepForestTarget,
}

impl Default for ColorOutputSpec {
    fn default() -> Self {
        Self {
            enabled: false,
            forest_color: "#28543A".into(),
            rock_color: "#7C7468".into(),
            snow_color: "#F4F3EC".into(),
            water_color: "#2F76B5".into(),
            road_color: "#D8A33C".into(),
            building_color: "#B8A890".into(),
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
            class_borders: ClassBorders::default(),
            // 2.5 cells keeps the estimate local so borders track the data;
            // a 0.05 nugget keeps the estimator near-exact at native nodes
            // while still damping staircase phase noise between them.
            border_smoothing_range_cells: 2.5,
            border_smoothing_nugget: 0.05,
            forest_slope_gate: true,
            // Closed forest is rare above roughly 45 degrees and absent from
            // true cliff faces; 55 degrees keeps legitimately steep forested
            // gorge and fjord walls while catching near-vertical rock that
            // 10 m land-cover pixels paint green.
            forest_slope_limit_degrees: 55.0,
            steep_forest_target: SteepForestTarget::Rock,
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
        ] {
            if !valid_hex_color(color) {
                bail!("{name} color must use #RRGGBB");
            }
        }
        if !(0.4..=5.0).contains(&self.road_width_mm) {
            bail!("road line width must be between 0.4 and 5 mm");
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
        if !(30.0..=85.0).contains(&self.forest_slope_limit_degrees) {
            bail!("forest slope limit must be between 30 and 85 degrees");
        }
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
}

impl SurfaceClass {
    /// Every class, ordered by `material_index`.
    pub(crate) const ALL: [Self; 6] = [
        Self::Rock,
        Self::Forest,
        Self::Snow,
        Self::Water,
        Self::Road,
        Self::Building,
    ];

    pub(crate) fn material_index(self) -> u32 {
        match self {
            Self::Rock => 0,
            Self::Forest => 1,
            Self::Snow => 2,
            Self::Water => 3,
            Self::Road => 4,
            Self::Building => 5,
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
        assert_eq!(spec.elevation_source, ElevationSource::Mapzen);
        assert!(!spec.solid_model);
        assert!(!spec.straight_piece_sides);
        assert!(spec.puzzle_tabs);
        assert_eq!(spec.overlay_samples_per_piece, 112);
        assert_eq!(spec.place_name, "Mount Rainier");
        assert!(!spec.tray.enabled);
        assert!(!spec.buildings.enabled);
        assert_eq!(spec.buildings.z_scale, 5.0);
        assert_eq!(spec.color_output.water_color, "#2F76B5");
        assert_eq!(spec.color_output.road_color, "#D8A33C");
        assert_eq!(spec.color_output.building_color, "#B8A890");
        assert!(spec.color_output.roads_enabled);
        assert_eq!(spec.color_output.road_detail, RoadDetail::Automatic);
        assert!(spec.color_output.adaptive_road_widths);
        assert!(spec.color_output.osm_water_enabled);
        assert_eq!(spec.color_output.waterway_coverage_percent, 12.0);
        assert_eq!(spec.color_output.road_width_mm, 0.7);
        assert_eq!(spec.color_output.road_height_mm, 0.2);
        assert_eq!(
            spec.color_output.bridge_structure,
            BridgeStructure::Floating
        );
        assert_eq!(spec.color_output.bridge_thickness_mm, 1.2);
        assert_eq!(spec.color_output.class_borders, ClassBorders::Blocky);
        assert_eq!(spec.color_output.border_smoothing_range_cells, 2.5);
        assert_eq!(spec.color_output.border_smoothing_nugget, 0.05);
        assert!(spec.color_output.forest_slope_gate);
        assert_eq!(spec.color_output.forest_slope_limit_degrees, 55.0);
        assert_eq!(
            spec.color_output.steep_forest_target,
            SteepForestTarget::Rock
        );
    }

    #[test]
    fn steep_forest_targets_parse_as_snake_case() {
        let spec: GenerationSpec = serde_json::from_value(serde_json::json!({
            "color_output": { "steep_forest_target": "snow" }
        }))
        .unwrap();
        assert_eq!(
            spec.color_output.steep_forest_target,
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
            "color_output": { "class_borders": "smooth", "forest_slope_gate": false }
        }))
        .unwrap();
        assert_eq!(spec.color_output.class_borders, ClassBorders::Smooth);
        assert!(!spec.color_output.forest_slope_gate);
        assert_eq!(spec.color_output.border_smoothing_range_cells, 2.5);
        assert_eq!(spec.color_output.border_smoothing_nugget, 0.05);

        let mut spec = GenerationSpec::default();
        spec.color_output.forest_slope_limit_degrees = 20.0;
        assert!(spec.validate().is_err());
        spec.color_output.forest_slope_limit_degrees = 85.0;
        assert!(spec.validate().is_ok());
    }

    #[test]
    fn border_smoothing_parameters_validate() {
        let mut spec = GenerationSpec::default();
        spec.color_output.border_smoothing_range_cells = 0.5;
        let error = spec.validate().unwrap_err().to_string();
        assert!(error.contains("between 1 and 8 native cells"));
        spec.color_output.border_smoothing_range_cells = 8.0;
        assert!(spec.validate().is_ok());

        spec.color_output.border_smoothing_nugget = 0.6;
        let error = spec.validate().unwrap_err().to_string();
        assert!(error.contains("between 0 and 0.5"));
        spec.color_output.border_smoothing_nugget = 0.0;
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
