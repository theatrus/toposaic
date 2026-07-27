use anyhow::{Result, bail};
use serde::{Deserialize, Serialize};

use crate::surface::SurfaceField;

const MAX_ADJACENT_GRID_SIDE: u32 = 12;
const MAX_PUZZLE_TILE_COORDINATE: i32 = 1_000_000;
const AUTO_DETAIL_REFERENCE_SPAN_KM: f64 = 18.0;
const LINE_SCALE_WIDE_SPAN_KM: f64 = 18.0;
const LINE_SCALE_CLOSE_SPAN_KM: f64 = 2.0;
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
const MAX_MAP_MARKERS: usize = 50;
const MAX_MARKER_NAME_CHARS: usize = 80;
const KILOMETRES_PER_LATITUDE_DEGREE: f64 = 110.574;
const KILOMETRES_PER_LONGITUDE_DEGREE: f64 = 111.32;
const MINIMUM_LONGITUDE_SCALE: f64 = 20.0;
const MAX_MODEL_LATITUDE: f64 = 85.0;

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
    pub outer_edge_interlocks: bool,
    pub adjacent_tile_column: u32,
    pub adjacent_tile_row: u32,
    /// Stable input for all puzzle edge choices. Keep this with the setup so
    /// later runs can reproduce the same cuts.
    pub puzzle_seed: u32,
    /// Signed position in the unbounded puzzle grid. These coordinates let
    /// separate jobs agree on the edge between them.
    pub puzzle_tile_column: i32,
    pub puzzle_tile_row: i32,
    pub clearance_mm: f32,
    pub samples_per_piece: u32,
    pub overlay_samples_per_piece: u32,
    pub mesh_samples_across: Option<u32>,
    pub overlay_samples_across: Option<u32>,
    pub fine_dem_detail: bool,
    /// Replace isolated wild elevation readings with their neighbourhood
    /// median before the model is built. On by default: published tiles carry
    /// bad pixels here and there, and one of them squeezes the whole relief.
    /// `#[serde(default)]` on the struct fills this from `Default`, so setups
    /// saved before the field existed get the pass too.
    pub despike_terrain: bool,
    pub solid_model: bool,
    pub straight_piece_sides: bool,
    pub puzzle_tabs: bool,
    pub place_name: String,
    pub tray: TraySpec,
    pub puzzle_retention: PuzzleRetentionSpec,
    pub wall_mount: WallMountSpec,
    pub buildings: BuildingSpec,
    pub marker_settings: MarkerSpec,
    pub color_output: ColorOutputSpec,
    /// Imported hiker trails (GPX/KML routes) drawn on the model in the
    /// dedicated trail color. Empty for every spec saved before the field
    /// existed, so old projects regenerate byte-identically.
    pub trails: Vec<TrailRoute>,
    /// User-placed points that mark a building, paint a dot, or cut a flag
    /// socket. Empty for setups saved before map markers existed.
    pub markers: Vec<MapMarker>,
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
            base_mm: 3.2,
            relief_mm: 28.0,
            elevation_datum_m: None,
            elevation_m_per_mm: None,
            adjacent_columns: 1,
            adjacent_rows: 1,
            super_tile_anchor: SuperTileAnchor::TopLeft,
            adjacent_interlocks: false,
            outer_edge_interlocks: false,
            adjacent_tile_column: 0,
            adjacent_tile_row: 0,
            puzzle_seed: 0,
            puzzle_tile_column: 0,
            puzzle_tile_row: 0,
            clearance_mm: 0.14,
            samples_per_piece: 64,
            overlay_samples_per_piece: 112,
            mesh_samples_across: None,
            overlay_samples_across: None,
            fine_dem_detail: false,
            despike_terrain: true,
            solid_model: false,
            straight_piece_sides: false,
            puzzle_tabs: true,
            place_name: "Mount Rainier".into(),
            tray: TraySpec::default(),
            puzzle_retention: PuzzleRetentionSpec::default(),
            wall_mount: WallMountSpec::default(),
            buildings: BuildingSpec::default(),
            marker_settings: MarkerSpec::default(),
            color_output: ColorOutputSpec::default(),
            trails: Vec::new(),
            markers: Vec::new(),
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
        if !(1.0..=20.0).contains(&self.base_mm) {
            bail!("minimum piece height must be between 1 and 20 mm");
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
        if self.puzzle_tile_column.unsigned_abs() > MAX_PUZZLE_TILE_COORDINATE as u32
            || self.puzzle_tile_row.unsigned_abs() > MAX_PUZZLE_TILE_COORDINATE as u32
        {
            bail!(
                "puzzle tile row and column must each be between -{MAX_PUZZLE_TILE_COORDINATE} and {MAX_PUZZLE_TILE_COORDINATE}"
            );
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
        if self.tray.enabled {
            crate::text::validate_embossing_text(&self.place_name, self.tray.label_font)?;
        }
        self.puzzle_retention
            .validate(self.base_mm, self.tray.enabled)?;
        let wall_mount_target = self.wall_mount_target_size();
        self.wall_mount
            .validate(self.base_mm, self.tray.floor_mm, wall_mount_target[0])?;
        if self.wall_mount.style != WallMountStyle::None {
            crate::mount::validate_wall_mount_frame(
                &self.wall_mount,
                wall_mount_target[0],
                wall_mount_target[1],
            )?;
        }
        if self.wall_mount.cuts_tray() && !self.tray.enabled {
            bail!("tray wall mounting needs an enabled tray");
        }
        if self.puzzle_retention.active(self.tray.enabled) && self.wall_mount.cuts_terrain() {
            bail!(
                "puzzle retention cannot share the terrain back with wall mounting; mount the tray instead"
            );
        }
        self.buildings.validate()?;
        self.color_output.validate()?;
        if self.trails.len() > MAX_TRAILS {
            bail!("imported trail count must be at most {MAX_TRAILS}");
        }
        for trail in &self.trails {
            trail.validate()?;
        }
        self.marker_settings.validate(self)?;
        if self.markers.len() > MAX_MAP_MARKERS {
            bail!("map marker count must be at most {MAX_MAP_MARKERS}");
        }
        for marker in &self.markers {
            marker.validate()?;
        }
        let flag_holes = self
            .markers
            .iter()
            .filter(|marker| marker.kind == MarkerKind::FlagHole)
            .map(|marker| {
                (
                    marker,
                    self.normalized_map_point(marker.latitude, marker.longitude),
                )
            })
            .filter(|(_, point)| (0.0..=1.0).contains(&point[0]) && (0.0..=1.0).contains(&point[1]))
            .collect::<Vec<_>>();
        for (index, (first, first_point)) in flag_holes.iter().enumerate() {
            for (second, second_point) in &flag_holes[index + 1..] {
                let distance_mm = ((first_point[0] - second_point[0]) * self.width_mm)
                    .hypot((first_point[1] - second_point[1]) * self.height_mm());
                if distance_mm < self.marker_settings.hole_diameter_mm {
                    bail!(
                        "flag markers '{}' and '{}' overlap; move them farther apart",
                        first.name,
                        second.name
                    );
                }
            }
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

    pub fn uses_markers(&self) -> bool {
        !self.markers.is_empty()
    }

    pub fn uses_colored_markers(&self) -> bool {
        self.markers
            .iter()
            .any(|marker| marker.kind != MarkerKind::FlagHole)
    }

    pub fn uses_building_markers(&self) -> bool {
        self.markers
            .iter()
            .any(|marker| marker.kind == MarkerKind::Building)
    }

    pub fn uses_flag_holes(&self) -> bool {
        self.markers
            .iter()
            .any(|marker| marker.kind == MarkerKind::FlagHole)
    }

    /// Maps a geographic point into this tile's normalized model square.
    /// The API uses this same helper for OSM data, so markers, trails, and
    /// fetched features cannot drift apart at high latitude or the date line.
    pub fn normalized_map_point(&self, latitude: f64, longitude: f64) -> [f32; 2] {
        let half_latitude = self.ground_span_km / (2.0 * KILOMETRES_PER_LATITUDE_DEGREE);
        let longitude_scale = (KILOMETRES_PER_LONGITUDE_DEGREE
            * self.center_lat.to_radians().cos().abs())
        .max(MINIMUM_LONGITUDE_SCALE);
        let half_longitude = self.ground_span_km / (2.0 * longitude_scale);
        let longitude =
            self.center_lon + (longitude - self.center_lon + 180.0).rem_euclid(360.0) - 180.0;
        let south = (self.center_lat - half_latitude).max(-MAX_MODEL_LATITUDE);
        let north = (self.center_lat + half_latitude).min(MAX_MODEL_LATITUDE);
        [
            ((longitude - (self.center_lon - half_longitude)) / (2.0 * half_longitude)) as f32,
            ((latitude - south) / (north - south)) as f32,
        ]
    }

    /// Whether the railway layer — `railway=*`: trains, trams, metros,
    /// funiculars, monorails — is drawn at all. Like roads it rides on color
    /// output, because it comes from the same OpenStreetMap fetch the color
    /// pipeline runs.
    pub fn uses_rail(&self) -> bool {
        self.color_output.enabled && self.color_output.rail_enabled
    }

    /// Whether the aerialway layer — `aerialway=*`: cable cars, gondolas,
    /// chair lifts, drag lifts, rope tows — is drawn at all. It switches
    /// independently of the railway layer: a chairlift up a ski slope and a
    /// mainline railway are different features, and a map may want one
    /// without the other.
    pub fn uses_aerial(&self) -> bool {
        self.color_output.enabled && self.color_output.aerial_enabled
    }

    /// Whether either rail-family layer is drawn. The two share a fetch
    /// pipeline, a lifecycle setting, and the piece-level overlay gate.
    pub fn uses_rail_or_aerial(&self) -> bool {
        self.uses_rail() || self.uses_aerial()
    }

    /// Where the railway layer paints: `Separate` gives it the Rail class,
    /// color, and filament slot; `WithRoads` paints it as a road, in the
    /// road color at the road width.
    ///
    /// This answers only "how would railways look", so it ignores
    /// `rail_enabled`; [`Self::uses_rail`] decides whether they are drawn.
    pub fn rail_line_style(&self) -> LineStyle {
        match self.color_output.rail_style {
            RailStyle::Separate => LineStyle {
                class: SurfaceClass::Rail,
                width_mm: self.color_output.rail_width_mm,
            },
            RailStyle::WithRoads => self.road_line_style(),
        }
    }

    /// Where the aerialway layer paints.
    ///
    /// `WithRail` follows whatever the railway layer resolves to, so lifts
    /// and trains share one spool and one look when a user asks for that.
    ///
    /// The chain is total. When the railway layer is switched OFF there is
    /// no rail styling to follow, and `with_rail` falls through to the
    /// railway layer's own default, `with_roads`. It deliberately does not
    /// draw nothing — an enabled layer that vanished because an unrelated
    /// toggle moved would be a trap — and it deliberately does not borrow
    /// the Rail class, because a switched-off railway layer must not put a
    /// rail color into the archive.
    pub fn aerial_line_style(&self) -> LineStyle {
        match self.color_output.aerial_style {
            AerialStyle::Separate => LineStyle {
                class: SurfaceClass::Aerial,
                width_mm: self.color_output.aerial_width_mm,
            },
            AerialStyle::WithRoads => self.road_line_style(),
            AerialStyle::WithRail if self.color_output.rail_enabled => self.rail_line_style(),
            AerialStyle::WithRail => self.road_line_style(),
        }
    }

    fn road_line_style(&self) -> LineStyle {
        LineStyle {
            class: SurfaceClass::Road,
            width_mm: self.color_output.road_width_mm,
        }
    }

    /// Width multiplier for roads without a usable mapped width and for
    /// aerial lifts at the current ground span. The logarithmic fade matches
    /// how map zoom feels: each halving of the span adds an equal share of
    /// the close-view boost.
    pub fn close_view_line_scale(&self) -> f32 {
        let settings = &self.color_output.line_scaling;
        if !settings.scale_line_widths_by_span {
            return 1.0;
        }
        let span = self
            .ground_span_km
            .clamp(LINE_SCALE_CLOSE_SPAN_KM, LINE_SCALE_WIDE_SPAN_KM);
        let progress = (LINE_SCALE_WIDE_SPAN_KM / span).ln()
            / (LINE_SCALE_WIDE_SPAN_KM / LINE_SCALE_CLOSE_SPAN_KM).ln();
        1.0 + (settings.close_view_width_multiplier - 1.0) * progress as f32
    }

    /// Whether drawn railways get their own class, color, and filament slot.
    pub fn uses_separate_rail(&self) -> bool {
        self.uses_rail() && self.color_output.rail_style == RailStyle::Separate
    }

    /// Whether drawn aerialways get their own class, color, and filament
    /// slot. Note that an aerial layer set to `with_rail` over a
    /// separately-styled railway layer paints in the RAIL class, so this
    /// stays false while the Rail slot covers both.
    pub fn uses_separate_aerial(&self) -> bool {
        self.uses_aerial() && self.aerial_line_style().class == SurfaceClass::Aerial
    }

    pub fn height_mm(&self) -> f32 {
        self.width_mm * self.rows as f32 / self.columns as f32
    }

    pub(crate) fn wall_mount_target_size(&self) -> [f32; 2] {
        if self.wall_mount.target == WallMountTarget::Terrain {
            return [self.width_mm, self.height_mm()];
        }

        let extra = (self.tray.clearance_mm + self.tray.rim_width_mm) * 2.0;
        let tile_size = [self.width_mm + extra, self.height_mm() + extra];
        if self.adjacent_columns > 1 || self.adjacent_rows > 1 {
            if self.tray.individual_tiles {
                tile_size
            } else if self.tray.segment_columns > 1 || self.tray.segment_rows > 1 {
                [
                    self.width_mm / self.tray.segment_columns as f32,
                    self.height_mm() / self.tray.segment_rows as f32,
                ]
            } else {
                [self.width_mm, self.height_mm()]
            }
        } else if self.tray.segment_columns > 1 || self.tray.segment_rows > 1 {
            [
                self.width_mm / self.tray.segment_columns as f32,
                self.height_mm() / self.tray.segment_rows as f32,
            ]
        } else {
            tile_size
        }
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
        self.color_output.enabled
            || self.buildings.enabled
            || self.uses_trails()
            || self.uses_colored_markers()
    }

    /// The filament palette of one archive: every surface class it can
    /// emit, in `SurfaceClass::ALL` order, packed into consecutive slots.
    ///
    /// The palette is DENSE. A class the archive never paints takes no slot
    /// and the classes after it move up, so a separately-styled rail layer
    /// without imported trails is seven colors with rail in slot seven — not
    /// eight with an unreferenced trail placeholder. It filters
    /// `SurfaceClass::ALL` rather than reordering it, so the same feature
    /// always lands in the same relative position, and it is computed once
    /// per archive, so every mesh in the file agrees on it.
    ///
    /// Membership takes TWO tests, and a class needs both.
    ///
    /// The spec test asks whether the settings draw the class at all. The
    /// `surface` test asks whether the drawn data actually contains one —
    /// because with both rail-family layers coloring themselves by default,
    /// a settings-only palette would charge a spool for cable cars in a city
    /// that has none, and the whole point of packing the palette is not to
    /// bill for what is not there. Passing `None` (a tray, or any archive
    /// with no surface data) skips the second test, which can only ever make
    /// the palette larger.
    ///
    /// The result is a SUPERSET of what any mesh in the archive paints, and
    /// that direction is the one that matters:
    ///
    /// - The base six are unconditional. Terrain tops sample the field's
    ///   base classes; every wall, floor, and underside is Rock; building
    ///   shells are Building; road ribbons and every bridge deck default to
    ///   Road; and tray meshes, which carry no field at all, use Rock,
    ///   Forest, and Snow. Holding all six by construction covers each of
    ///   those without a per-source special case — and keeps every archive
    ///   written before the palette became dense byte-for-byte unchanged.
    /// - Trails, railways, and aerialways only ever reach a mesh as vector
    ///   lines of the surface field, which is exactly what
    ///   [`SurfaceField::contained_classes`] reports.
    ///
    /// So a triangle without a slot is unreachable, and the export can never
    /// be refused over a thin line the palette missed. The cost is that the
    /// palette may be slightly LOOSE — a line in the field that no piece
    /// happens to sample still takes a slot. That is the right way round.
    pub(crate) fn material_palette<'spec>(
        &'spec self,
        surface: Option<&SurfaceField>,
    ) -> MaterialPalette<'spec> {
        let contained = surface.map(SurfaceField::contained_classes);
        let mut palette = MaterialPalette::default();
        for class in SurfaceClass::ALL {
            // Only the optional layers consult the data; see above for why
            // the base six are unconditional.
            let in_data = matches!(
                class,
                SurfaceClass::Rock
                    | SurfaceClass::Forest
                    | SurfaceClass::Snow
                    | SurfaceClass::Water
                    | SurfaceClass::Road
                    | SurfaceClass::Building
            ) || contained
                .is_none_or(|present| present[class.material_index() as usize]);
            if !self.emits_class(class) || !in_data {
                continue;
            }
            palette.slots[class.material_index() as usize] = Some(palette.colors.len() as u32);
            palette.colors.push(self.class_color(class));
        }
        palette
    }

    /// Whether a spec's SETTINGS draw this class at all.
    fn emits_class(&self, class: SurfaceClass) -> bool {
        match class {
            SurfaceClass::Rock
            | SurfaceClass::Forest
            | SurfaceClass::Snow
            | SurfaceClass::Water
            | SurfaceClass::Road
            | SurfaceClass::Building => true,
            SurfaceClass::Trail => self.uses_trails(),
            SurfaceClass::Rail => self.uses_separate_rail(),
            SurfaceClass::Aerial => self.uses_separate_aerial(),
            SurfaceClass::Marker => self.uses_colored_markers(),
        }
    }

    fn class_color(&self, class: SurfaceClass) -> &str {
        match class {
            SurfaceClass::Rock => &self.color_output.rock_color,
            SurfaceClass::Forest => &self.color_output.forest_color,
            SurfaceClass::Snow => &self.color_output.snow_color,
            SurfaceClass::Water => &self.color_output.water_color,
            SurfaceClass::Road => &self.color_output.road_color,
            SurfaceClass::Building => &self.color_output.building_color,
            SurfaceClass::Trail => &self.color_output.trail_color,
            SurfaceClass::Rail => &self.color_output.rail_color,
            SurfaceClass::Aerial => &self.color_output.aerial_color,
            SurfaceClass::Marker => &self.marker_settings.color,
        }
    }

    fn mesh_piece_count(&self) -> u32 {
        if self.solid_model {
            1
        } else {
            self.rows.max(self.columns)
        }
    }
}

/// One archive's dense filament palette: the colors it emits, in slot order,
/// and the slot each surface class was packed into.
///
/// Built once per 3MF by [`GenerationSpec::material_palette`] and used by
/// every part of the write that has to agree on a slot number — the color
/// group, the per-triangle `p1`/`p2`/`p3` indices, the OrcaSlicer paint
/// codes, and the per-filament project-settings arrays. A slot number used
/// inconsistently between any two of those would mis-color the print, so
/// they all read it from here.
#[derive(Debug, Default)]
pub(crate) struct MaterialPalette<'spec> {
    colors: Vec<&'spec str>,
    slots: [Option<u32>; SurfaceClass::ALL.len()],
}

impl<'spec> MaterialPalette<'spec> {
    /// The emitted colors, indexed by slot.
    pub(crate) fn colors(&self) -> &[&'spec str] {
        &self.colors
    }

    pub(crate) fn len(&self) -> usize {
        self.colors.len()
    }

    /// The slot a class paints into, or `None` when the palette does not
    /// carry it. Callers that must not fail check membership first.
    pub(crate) fn slot(&self, class: SurfaceClass) -> Option<u32> {
        self.slots[class.material_index() as usize]
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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MarkerKind {
    Building,
    Dot,
    FlagHole,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MapMarker {
    pub name: String,
    pub latitude: f64,
    pub longitude: f64,
    pub kind: MarkerKind,
}

impl MapMarker {
    fn validate(&self) -> Result<()> {
        if self.name.trim().is_empty() || self.name.chars().count() > MAX_MARKER_NAME_CHARS {
            bail!("marker names must contain between 1 and {MAX_MARKER_NAME_CHARS} characters");
        }
        if self.name.chars().any(char::is_control) {
            bail!("marker names cannot contain control characters");
        }
        if !self.latitude.is_finite() || !(-90.0..=90.0).contains(&self.latitude) {
            bail!("marker latitudes must be between -90 and 90 degrees");
        }
        if !self.longitude.is_finite() || !(-180.0..=180.0).contains(&self.longitude) {
            bail!("marker longitudes must be between -180 and 180 degrees");
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct MarkerSpec {
    pub color: String,
    pub dot_diameter_mm: f32,
    pub hole_diameter_mm: f32,
    pub hole_depth_mm: f32,
    pub flag_clearance_mm: f32,
    pub export_flag_template: bool,
}

impl Default for MarkerSpec {
    fn default() -> Self {
        Self {
            color: "#E24A33".into(),
            dot_diameter_mm: 3.0,
            hole_diameter_mm: 2.4,
            hole_depth_mm: 2.0,
            flag_clearance_mm: 0.2,
            export_flag_template: true,
        }
    }
}

impl MarkerSpec {
    fn validate(&self, spec: &GenerationSpec) -> Result<()> {
        if !valid_hex_color(&self.color) {
            bail!("marker color must be a #RRGGBB value");
        }
        if !(1.0..=10.0).contains(&self.dot_diameter_mm) {
            bail!("marker dot diameter must be between 1 and 10 mm");
        }
        if !(1.2..=6.0).contains(&self.hole_diameter_mm) {
            bail!("marker flag-hole diameter must be between 1.2 and 6 mm");
        }
        if !(0.6..=6.0).contains(&self.hole_depth_mm) {
            bail!("marker flag-hole depth must be between 0.6 and 6 mm");
        }
        if !(0.1..=0.6).contains(&self.flag_clearance_mm) {
            bail!("marker flag clearance must be between 0.1 and 0.6 mm");
        }
        if self.hole_diameter_mm - self.flag_clearance_mm < 0.9 {
            bail!("marker flag clearance must leave at least a 0.9 mm flag post");
        }
        if spec.uses_flag_holes() && self.hole_depth_mm > spec.base_mm - 0.4 {
            bail!("marker flag holes must leave at least 0.4 mm of terrain base");
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
    pub contours_enabled: bool,
    pub tray_color: String,
    pub contour_color: String,
    pub label_color: String,
    pub label_font: TrayLabelFont,
    pub label_height_mm: f32,
    pub label_position: TrayLabelPosition,
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
            contours_enabled: true,
            tray_color: "#252822".into(),
            contour_color: "#E7E4D8".into(),
            label_color: "#F4F3EC".into(),
            label_font: TrayLabelFont::AtkinsonHyperlegible,
            label_height_mm: 4.0,
            label_position: TrayLabelPosition::Center,
            clearance_mm: 0.6,
            rim_width_mm: 8.0,
            floor_mm: 2.4,
            rim_height_mm: 3.2,
            contour_count: 18,
            segment_columns: 1,
            segment_rows: 1,
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TrayLabelFont {
    #[default]
    AtkinsonHyperlegible,
    NotoSans,
    B612Mono,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TrayLabelPosition {
    Left,
    #[default]
    Center,
    Right,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WallMountStyle {
    #[default]
    None,
    StraightPin,
    AngledPin,
    FrenchCleat,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WallMountTarget {
    #[default]
    Terrain,
    Tray,
}

/// Mating pins in the tray floor and blind sockets in the terrain back.
///
/// The printed pin diameter is the nominal size. `clearance_mm` widens the
/// matching socket and deepens it by the same amount, so the peg never holds
/// the terrain above the tray floor.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct PuzzleRetentionSpec {
    pub enabled: bool,
    pub pin_diameter_mm: f32,
    pub pin_height_mm: f32,
    pub clearance_mm: f32,
}

impl Default for PuzzleRetentionSpec {
    fn default() -> Self {
        Self {
            enabled: false,
            pin_diameter_mm: 3.0,
            pin_height_mm: 1.0,
            clearance_mm: 0.2,
        }
    }
}

impl PuzzleRetentionSpec {
    pub(crate) fn active(&self, tray_enabled: bool) -> bool {
        self.enabled && tray_enabled
    }

    pub(crate) fn socket_depth_mm(&self) -> f32 {
        self.pin_height_mm + self.clearance_mm
    }

    pub(crate) fn socket_diameter_mm(&self) -> f32 {
        self.pin_diameter_mm + self.clearance_mm
    }

    fn validate(&self, base_mm: f32, tray_enabled: bool) -> Result<()> {
        if !self.enabled {
            return Ok(());
        }
        if !tray_enabled {
            bail!("puzzle retention needs an enabled tray");
        }
        if !(2.0..=8.0).contains(&self.pin_diameter_mm) {
            bail!("tray-retention pin diameter must be between 2 and 8 mm");
        }
        if !(0.4..=3.0).contains(&self.pin_height_mm) {
            bail!("tray-retention pin height must be between 0.4 and 3 mm");
        }
        if !(0.1..=0.6).contains(&self.clearance_mm) {
            bail!("tray-retention fit clearance must be between 0.1 and 0.6 mm");
        }
        if self.socket_depth_mm() > base_mm - 0.4 {
            bail!("tray-retention socket must leave at least 0.4 mm of terrain base");
        }
        Ok(())
    }
}

/// Blind mounting cuts in the flat back of the terrain or tray.
///
/// Keeping the feature in one top-level spec makes the two mounting targets
/// exclusive. A saved setup cannot ask for two overlapping sets of cuts by
/// accident, and old setups deserialize to `None` through `#[serde(default)]`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct WallMountSpec {
    pub style: WallMountStyle,
    pub target: WallMountTarget,
    pub vertical_position_ratio: f32,
    pub depth_mm: f32,
    pub thickness_mm: f32,
    pub wall_offset_mm: f32,
    pub pin_diameter_mm: f32,
    pub pin_count: u32,
    pub pin_spacing_mm: f32,
    pub cleat_width_mm: f32,
    pub export_hardware: bool,
    pub fit_clearance_mm: f32,
    pub screw_hole_diameter_mm: f32,
    pub screw_countersink_depth_mm: f32,
    pub screw_head_clearance_mm: f32,
    pub wide_edge_screws: bool,
}

impl Default for WallMountSpec {
    fn default() -> Self {
        Self {
            style: WallMountStyle::None,
            target: WallMountTarget::Terrain,
            vertical_position_ratio: 0.28,
            depth_mm: 1.6,
            thickness_mm: 1.2,
            wall_offset_mm: 0.8,
            pin_diameter_mm: 4.0,
            pin_count: 1,
            pin_spacing_mm: 32.0,
            cleat_width_mm: 12.0,
            export_hardware: true,
            fit_clearance_mm: 0.2,
            screw_hole_diameter_mm: 3.5,
            screw_countersink_depth_mm: 0.8,
            screw_head_clearance_mm: 0.4,
            wide_edge_screws: true,
        }
    }
}

impl WallMountSpec {
    pub(crate) fn embedded_depth_mm(&self) -> f32 {
        self.pocket_depth_mm() + self.engagement_depth_mm().max(self.screw_head_clearance_mm)
    }

    pub(crate) fn pocket_depth_mm(&self) -> f32 {
        self.thickness_mm - self.wall_offset_mm
    }

    pub(crate) fn engagement_depth_mm(&self) -> f32 {
        self.depth_mm
    }

    pub(crate) fn cuts_terrain(&self) -> bool {
        self.style != WallMountStyle::None && self.target == WallMountTarget::Terrain
    }

    pub(crate) fn cuts_tray(&self) -> bool {
        self.style != WallMountStyle::None && self.target == WallMountTarget::Tray
    }

    fn validate(&self, base_mm: f32, tray_floor_mm: f32, target_width_mm: f32) -> Result<()> {
        if self.style == WallMountStyle::None {
            return Ok(());
        }
        if !((1.0 / 6.0)..=(5.0 / 6.0)).contains(&self.vertical_position_ratio) {
            bail!("wall-mount position must be between one-sixth and five-sixths from the top");
        }
        if !(0.0..=10.0).contains(&self.wall_offset_mm) {
            bail!("wall offset must be between 0 and 10 mm");
        }
        if !(0.4..=3.0).contains(&self.depth_mm) {
            bail!("wall-mount engagement depth must be between 0.4 and 3 mm");
        }
        if !(0.4..=13.0).contains(&self.thickness_mm) {
            bail!("wall-plate thickness must be between 0.4 and 13 mm");
        }
        if self.pocket_depth_mm() + 0.000_01 < 0.4 {
            bail!("wall-plate thickness must be at least 0.4 mm greater than its wall offset");
        }
        if !(2.0..=10.0).contains(&self.pin_diameter_mm) {
            bail!("wall-mount pin diameter must be between 2 and 10 mm");
        }
        if !(1..=2).contains(&self.pin_count) {
            bail!("wall-mount pin count must be one or two");
        }
        if !(12.0..=100.0).contains(&self.pin_spacing_mm) {
            bail!("wall-mount pin spacing must be between 12 and 100 mm");
        }
        if !(8.0..=400.0).contains(&self.cleat_width_mm) {
            bail!("wall-mount cleat width must be between 8 and 400 mm");
        }
        if self.style == WallMountStyle::FrenchCleat && self.cleat_width_mm > target_width_mm - 4.0
        {
            bail!(
                "wall-mount cleat must leave at least 2 mm on each side of its piece, solid, or tray section"
            );
        }
        if !(0.1..=0.8).contains(&self.fit_clearance_mm)
            || self.fit_clearance_mm >= self.pin_diameter_mm - 0.8
        {
            bail!(
                "wall-mount hardware clearance must be between 0.1 and 0.8 mm and leave a printable pin"
            );
        }
        if !(2.0..=6.0).contains(&self.screw_hole_diameter_mm) {
            bail!("wall-mount screw-hole diameter must be between 2 and 6 mm");
        }
        if !(0.0..=3.0).contains(&self.screw_countersink_depth_mm) {
            bail!("wall-mount screw countersink depth must be between 0 and 3 mm");
        }
        if self.screw_countersink_depth_mm > self.thickness_mm - 0.4 + 0.000_01 {
            bail!("wall-mount screw countersink must leave at least 0.4 mm of straight screw bore");
        }
        if !(0.0..=3.0).contains(&self.screw_head_clearance_mm) {
            bail!("wall-mount screw-head pocket clearance must be between 0 and 3 mm");
        }
        let available = match self.target {
            WallMountTarget::Terrain => base_mm,
            WallMountTarget::Tray => tray_floor_mm,
        };
        let total_cut_depth = self.embedded_depth_mm();
        if total_cut_depth > available - 0.4 + 0.000_01 {
            let target = match self.target {
                WallMountTarget::Terrain => "minimum piece height",
                WallMountTarget::Tray => "display-base floor",
            };
            bail!(
                "the chosen {target} is too thin for this wall mount; raise it to at least {:.1} mm or reduce the mount cut depth",
                total_cut_depth + 0.4
            );
        }
        Ok(())
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
        if !(1.5..=10.0).contains(&self.label_height_mm) {
            bail!("tray label height must be between 1.5 and 10 mm");
        }
        if !(5.0..=16.0).contains(&self.rim_width_mm) {
            bail!("tray rim width must be between 5 and 16 mm");
        }
        if !(1.0..=20.0).contains(&self.floor_mm) {
            bail!("display-base floor thickness must be between 1 and 20 mm");
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

/// How drawn railways are colored.
///
/// `Separate` is the default: a railway is not a road, and a map that paints
/// it as one has thrown away the distinction the reader came for. Picking it
/// out in steel is the point of drawing it at all. Because the 3MF emits only
/// the colors a model actually uses, that costs exactly one filament slot and
/// nothing else.
///
/// `WithRoads` stays available for anyone who would rather spend the spool
/// elsewhere: it paints railways in the road class, in the road color, at the
/// road width, so they still show up without adding a filament.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RailStyle {
    /// Railways get their own class, color, and filament slot.
    #[default]
    Separate,
    /// Railways paint as roads, in the road color and at the road width.
    WithRoads,
}

/// How drawn aerialways — cable cars, gondolas, chair lifts, drag lifts,
/// rope tows — are colored.
///
/// `Separate` is the default, for the same reason railways default to their
/// own color: a chair lift up a ski slope is not a road and not a railway,
/// and the map is worth more when it says so. It costs one filament slot.
///
/// `WithRail` folds lifts into the railway layer, so the two rail-family
/// layers share one spool and one look; `WithRoads` folds them into the
/// roads. [`GenerationSpec::aerial_line_style`] resolves the chain, including
/// the case where the railway layer `WithRail` names is switched off.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AerialStyle {
    /// Aerialways get their own class, color, and filament slot.
    #[default]
    Separate,
    /// Aerialways follow the railway layer's style.
    WithRail,
    /// Aerialways paint as roads, in the road color and at the road width.
    WithRoads,
}

/// Which lifecycle states of railway and aerialway line the map draws.
///
/// The settings are CUMULATIVE: each keeps everything the one before it
/// kept and adds a state. OpenStreetMap writes a line's state either as a
/// bare key beside the in-service tag (`railway=rail` plus `disused=yes`) or
/// as a namespace that replaces it (`disused:railway=rail`); both forms mean
/// the same thing here and are read the same way.
///
/// The distinction the settings turn on is what is left on the ground.
/// `disused:` track still has its rails, ties, and ballast in place and a
/// disused lift still has its cable and pylons — the feature is intact, just
/// not running. `abandoned:` track has had the rails lifted, but the
/// formation is still there and often the most legible line in the
/// landscape: embankments, cuttings, a dead-straight trackbed, the cleared
/// swath of an old lift line. Both are worth printing if the user wants
/// them, and neither is worth printing by default, so the default is
/// `Operational` and today's output does not move.
///
/// Two groups stay excluded at EVERY setting, because a terrain model is a
/// record of what stands on the ground:
///
/// - `razed:`, `dismantled:`, `demolished:`, `removed:`, and `historic:`
///   mean the structure and its earthworks are gone; the way survives in
///   OpenStreetMap as a record of where something used to be. Printing it
///   would raise a ridge across ground that is flat.
/// - `proposed:` and `construction:` are the mirror image: nothing is there
///   yet. `construction:` is the arguable one, since a line being built
///   usually does have a graded formation — but the tag covers everything
///   from a surveyed alignment on paper to rails going down this week, so it
///   cannot be read as "there is earthwork here". A model that drew it would
///   show a feature a visitor would not find.
///
/// One setting covers both rail-family layers. The states are one physical
/// question, OpenStreetMap encodes them identically for `railway` and
/// `aerialway`, and splitting them would double the fetch-cache matrix to
/// buy the ability to ask for running trains beside derelict ski lifts.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RailLifecycle {
    /// In service only. Today's behavior, and the default.
    #[default]
    Operational,
    /// Adds track and lift lines still in place but out of use.
    Disused,
    /// Adds `disused`, plus lifted track whose formation is still visible.
    Abandoned,
}

impl RailLifecycle {
    /// Stable short name, used in the fetch cache key and the data-source
    /// note. It is part of the CACHE KEY because it changes the Overpass
    /// query: a download made with out-of-service lines filtered out must
    /// never be served to a request that asked for them.
    pub fn name(self) -> &'static str {
        match self {
            Self::Operational => "operational",
            Self::Disused => "disused",
            Self::Abandoned => "abandoned",
        }
    }
}

/// Where a drawn line layer lands: the surface class its geometry carries,
/// and the print width its per-type scales multiply.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LineStyle {
    pub class: SurfaceClass,
    pub width_mm: f32,
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
    /// Draw railways — `railway=*`, so trains, trams, metros, funiculars,
    /// monorails — from OpenStreetMap. On by default: a map that drops the
    /// rail network is simply wrong, and the default `rail_style` draws it
    /// without costing a filament slot.
    pub rail_enabled: bool,
    /// Color of railways when `rail_style` is `separate`. Ignored — and
    /// never emitted into any artifact — under the default `with_roads`
    /// style.
    pub rail_color: String,
    /// Print width of railway lines when `rail_style` is `separate`; under
    /// `with_roads` the road width applies instead.
    pub rail_width_mm: f32,
    /// Whether railways get their own color and filament slot (the default)
    /// or paint as roads.
    pub rail_style: RailStyle,
    /// Which lifecycle states of line to draw. Governs the railway AND the
    /// aerialway layer; see [`RailLifecycle`] for why they share it and for
    /// what stays excluded whatever it is set to.
    pub rail_lifecycle: RailLifecycle,
    /// Draw aerialways — `aerialway=*`, so cable cars, gondolas, chair
    /// lifts, drag lifts, rope tows — from OpenStreetMap. On by default and
    /// switchable apart from railways: a ski map wants the lifts without the
    /// mainline, a city map the mainline without the lifts.
    pub aerial_enabled: bool,
    /// Color of aerialways when `aerial_style` is `separate`. Ignored — and
    /// never emitted into any artifact — otherwise.
    pub aerial_color: String,
    /// Print width of aerialway lines when `aerial_style` is `separate`;
    /// otherwise the width of whichever layer they follow applies.
    pub aerial_width_mm: f32,
    /// Whether aerialways get their own color and filament slot (the
    /// default), follow the railway layer, or paint as roads.
    pub aerial_style: AerialStyle,
    pub roads_enabled: bool,
    pub road_detail: RoadDetail,
    pub adaptive_road_widths: bool,
    /// Map-span-aware width scaling for roads, railways, and aerial lifts.
    /// Flattened to keep saved setup files compatible and easy to read.
    #[serde(flatten)]
    pub line_scaling: LineScaleSpec,
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

/// Controls how mapped transport lines change width as the view closes in.
///
/// Serialized flat inside [`ColorOutputSpec`].
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct LineScaleSpec {
    pub scale_line_widths_by_span: bool,
    pub close_view_width_multiplier: f32,
    /// Upper print-width limit for roads and railways derived from a mapped
    /// physical width. Their configured class width remains the lower limit.
    pub maximum_mapped_width_mm: f32,
}

impl Default for LineScaleSpec {
    fn default() -> Self {
        Self {
            scale_line_widths_by_span: true,
            close_view_width_multiplier: 2.0,
            maximum_mapped_width_mm: 4.0,
        }
    }
}

impl LineScaleSpec {
    fn validate(&self) -> Result<()> {
        if !(1.0..=3.0).contains(&self.close_view_width_multiplier) {
            bail!("close-view line width multiplier must be between 1 and 3");
        }
        if !(0.4..=8.0).contains(&self.maximum_mapped_width_mm) {
            bail!("maximum mapped line width must be between 0.4 and 8 mm");
        }
        Ok(())
    }
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
            rail_enabled: true,
            // Clear red against the gold roads and raspberry trails, and
            // apart from the water blue.
            rail_color: "#C43D3D".into(),
            rail_width_mm: 0.7,
            rail_style: RailStyle::default(),
            rail_lifecycle: RailLifecycle::default(),
            aerial_enabled: true,
            // Signal violet: apart from the raspberry trail color, which is
            // pink-red, and from every terrain color and the water blue.
            aerial_color: "#6C4CB6".into(),
            aerial_width_mm: 0.7,
            aerial_style: AerialStyle::default(),
            roads_enabled: true,
            road_detail: RoadDetail::Automatic,
            adaptive_road_widths: true,
            line_scaling: LineScaleSpec::default(),
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
            ("rail", &self.rail_color),
            ("aerialway", &self.aerial_color),
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
        if !(0.4..=5.0).contains(&self.rail_width_mm) {
            bail!("rail line width must be between 0.4 and 5 mm");
        }
        if !(0.4..=5.0).contains(&self.aerial_width_mm) {
            bail!("aerialway line width must be between 0.4 and 5 mm");
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
        self.line_scaling.validate()?;
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
    /// output when a spec carries trails.
    Trail,
    /// Railways drawn in their own color, and the aerialways that follow
    /// them. The class only ever appears when `rail_style` is `separate`.
    Rail,
    /// Aerialways drawn in their own color. The class only ever appears when
    /// `aerial_style` is `separate`.
    Aerial,
    /// User-placed colored dots and highlighted buildings.
    Marker,
}

impl SurfaceClass {
    /// Every class, in `material_index` order.
    ///
    /// This order is the CLASS identity used by the preview, the coverage
    /// histogram, and the mesh materials. It is not a filament slot: the
    /// 3MF packs whichever of these classes a spec actually emits into
    /// consecutive slots, so a class that never appears costs nothing. See
    /// [`GenerationSpec::material_palette`].
    pub(crate) const ALL: [Self; 10] = [
        Self::Rock,
        Self::Forest,
        Self::Snow,
        Self::Water,
        Self::Road,
        Self::Building,
        Self::Trail,
        Self::Rail,
        Self::Aerial,
        Self::Marker,
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
            Self::Rail => 7,
            Self::Aerial => 8,
            Self::Marker => 9,
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
        let expected = r##"{"center_lat":46.8523,"center_lon":-121.7603,"elevation_source":"mapzen","ground_span_km":18.0,"width_mm":180.0,"rows":3,"columns":3,"base_mm":3.2,"relief_mm":28.0,"elevation_datum_m":null,"elevation_m_per_mm":null,"adjacent_columns":1,"adjacent_rows":1,"super_tile_anchor":"top_left","adjacent_interlocks":false,"adjacent_tile_column":0,"adjacent_tile_row":0,"clearance_mm":0.14,"samples_per_piece":64,"overlay_samples_per_piece":112,"mesh_samples_across":null,"overlay_samples_across":null,"fine_dem_detail":false,"despike_terrain":true,"solid_model":false,"straight_piece_sides":false,"puzzle_tabs":true,"place_name":"Mount Rainier","tray":{"enabled":false,"individual_tiles":false,"contours_enabled":true,"tray_color":"#252822","contour_color":"#E7E4D8","label_color":"#F4F3EC","label_font":"atkinson_hyperlegible","label_height_mm":4.0,"label_position":"center","clearance_mm":0.6,"rim_width_mm":8.0,"floor_mm":2.4,"rim_height_mm":3.2,"contour_count":18,"segment_columns":1,"segment_rows":1},"puzzle_retention":{"enabled":false,"pin_diameter_mm":3.0,"pin_height_mm":1.0,"clearance_mm":0.2},"wall_mount":{"style":"none","target":"terrain","vertical_position_ratio":0.28,"depth_mm":1.6,"thickness_mm":1.2,"wall_offset_mm":0.8,"pin_diameter_mm":4.0,"pin_count":1,"pin_spacing_mm":32.0,"cleat_width_mm":12.0,"export_hardware":true,"fit_clearance_mm":0.2,"screw_hole_diameter_mm":3.5,"screw_countersink_depth_mm":0.8,"screw_head_clearance_mm":0.4,"wide_edge_screws":true},"buildings":{"enabled":false,"z_scale":5.0},"color_output":{"enabled":false,"threemf_style":"project","forest_color":"#28543A","rock_color":"#7C7468","snow_color":"#F4F3EC","water_color":"#2F76B5","road_color":"#D8A33C","building_color":"#B8A890","trail_color":"#D6336C","trail_width_mm":0.7,"rail_enabled":true,"rail_color":"#C43D3D","rail_width_mm":0.7,"rail_style":"separate","rail_lifecycle":"operational","aerial_enabled":true,"aerial_color":"#6C4CB6","aerial_width_mm":0.7,"aerial_style":"separate","roads_enabled":true,"road_detail":"automatic","adaptive_road_widths":true,"scale_line_widths_by_span":true,"close_view_width_multiplier":2.0,"maximum_mapped_width_mm":4.0,"osm_water_enabled":true,"waterway_coverage_percent":12.0,"road_width_mm":0.7,"road_height_mm":0.2,"bridge_structure":"floating","bridge_thickness_mm":1.2,"minimum_patch_mm":1.2,"class_borders":"smooth","border_smoothing_range_cells":2.5,"border_smoothing_nugget":0.05,"forest_slope_gate":true,"forest_slope_limit_degrees":55.0,"steep_forest_target":"rock","snow_slope_gate":true,"snow_slope_limit_degrees":65.0},"trails":[]}"##;
        let expected = expected.replace(
            "\"buildings\":{\"enabled\":false,\"z_scale\":5.0},",
            "\"buildings\":{\"enabled\":false,\"z_scale\":5.0},\"marker_settings\":{\"color\":\"#E24A33\",\"dot_diameter_mm\":3.0,\"hole_diameter_mm\":2.4,\"hole_depth_mm\":2.0,\"flag_clearance_mm\":0.2,\"export_flag_template\":true},",
        );
        let expected = expected.replace("\"trails\":[]}", "\"trails\":[],\"markers\":[]}");
        let expected = expected.replace(
            "\"adjacent_interlocks\":false,",
            "\"adjacent_interlocks\":false,\"outer_edge_interlocks\":false,",
        );
        let expected = expected.replace(
            "\"adjacent_tile_row\":0,",
            "\"adjacent_tile_row\":0,\"puzzle_seed\":0,\"puzzle_tile_column\":0,\"puzzle_tile_row\":0,",
        );
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
            "outer_edge_interlocks": true,
            "adjacent_tile_column": 1,
            "adjacent_tile_row": 2,
            "puzzle_seed": 305419896,
            "puzzle_tile_column": -4,
            "puzzle_tile_row": 7,
            "clearance_mm": 0.25,
            "samples_per_piece": 96,
            "overlay_samples_per_piece": 128,
            "mesh_samples_across": 512,
            "overlay_samples_across": 640,
            "fine_dem_detail": true,
            "despike_terrain": true,
            "solid_model": true,
            "straight_piece_sides": true,
            "puzzle_tabs": false,
            "place_name": "Yellowstone",
            "tray": {
                "enabled": true,
                "individual_tiles": true,
                "contours_enabled": false,
                "tray_color": "#111111",
                "contour_color": "#222222",
                "label_color": "#333333",
                "label_font": "b612_mono",
                "label_height_mm": 5.5,
                "label_position": "right",
                "clearance_mm": 0.5,
                "rim_width_mm": 9.0,
                "floor_mm": 2.0,
                "rim_height_mm": 4.0,
                "contour_count": 24,
                "segment_columns": 2,
                "segment_rows": 3
            },
            "puzzle_retention": {
                "enabled": false,
                "pin_diameter_mm": 3.0,
                "pin_height_mm": 1.0,
                "clearance_mm": 0.25
            },
            "wall_mount": {
                "style": "angled_pin",
                "target": "tray",
                "vertical_position_ratio": 0.5,
                "depth_mm": 1.0,
                "thickness_mm": 2.0,
                "wall_offset_mm": 1.5,
                "pin_diameter_mm": 5.0,
                "pin_count": 2,
                "pin_spacing_mm": 32.0,
                "cleat_width_mm": 24.0,
                "export_hardware": true,
                "fit_clearance_mm": 0.25,
                "screw_hole_diameter_mm": 3.5,
                "screw_countersink_depth_mm": 0.75,
                "screw_head_clearance_mm": 0.5,
                "wide_edge_screws": false
            },
            "buildings": { "enabled": true, "z_scale": 2.0 },
            "marker_settings": {
                "color": "#E24A33",
                "dot_diameter_mm": 4.0,
                "hole_diameter_mm": 3.0,
                "hole_depth_mm": 2.0,
                "flag_clearance_mm": 0.25,
                "export_flag_template": true
            },
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
                "rail_enabled": false,
                "rail_color": "#334455",
                "rail_width_mm": 1.25,
                "rail_style": "separate",
                "rail_lifecycle": "abandoned",
                "aerial_enabled": false,
                "aerial_color": "#5533AA",
                "aerial_width_mm": 1.75,
                "aerial_style": "separate",
                "roads_enabled": false,
                "road_detail": "streets",
                "adaptive_road_widths": false,
                "scale_line_widths_by_span": false,
                "close_view_width_multiplier": 2.5,
                "maximum_mapped_width_mm": 5.5,
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
            ],
            "markers": [
                {
                    "name": "Old Faithful",
                    "latitude": 44.5,
                    "longitude": -110.5,
                    "kind": "dot"
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
    fn map_markers_validate_and_share_the_model_coordinate_frame() {
        let mut spec = GenerationSpec {
            markers: vec![MapMarker {
                name: "Home".into(),
                latitude: 46.8523,
                longitude: -121.7603,
                kind: MarkerKind::Dot,
            }],
            ..GenerationSpec::default()
        };
        assert!(spec.validate().is_ok());
        assert!(spec.uses_colored_markers());
        let center = spec.normalized_map_point(46.8523, -121.7603);
        assert!((center[0] - 0.5).abs() < 0.000_001);
        assert!((center[1] - 0.5).abs() < 0.000_001);

        spec.markers[0].kind = MarkerKind::FlagHole;
        spec.marker_settings.hole_depth_mm = spec.base_mm;
        assert!(spec.validate().is_err());
        spec.marker_settings.hole_depth_mm = 2.0;
        spec.markers[0].latitude = 91.0;
        assert!(spec.validate().is_err());

        let mut overlapping = GenerationSpec {
            width_mm: 100.0,
            rows: 2,
            columns: 2,
            markers: vec![
                MapMarker {
                    name: "First".into(),
                    latitude: 46.8523,
                    longitude: -121.7603,
                    kind: MarkerKind::FlagHole,
                },
                MapMarker {
                    name: "Second".into(),
                    latitude: 46.8523,
                    longitude: -121.7603,
                    kind: MarkerKind::FlagHole,
                },
            ],
            ..GenerationSpec::default()
        };
        assert!(overlapping.validate().is_err());
        overlapping.markers[1].longitude += 0.01;
        overlapping.validate().unwrap();
    }

    #[test]
    fn wall_mounts_default_off_and_preserve_a_printable_skin() {
        let old: GenerationSpec = serde_json::from_value(serde_json::json!({
            "tray": { "enabled": true }
        }))
        .unwrap();
        assert!(old.tray.contours_enabled);
        assert_eq!(old.tray.label_font, TrayLabelFont::AtkinsonHyperlegible);
        assert_eq!(old.tray.label_height_mm, 4.0);
        assert_eq!(old.tray.label_position, TrayLabelPosition::Center);
        assert!(!old.puzzle_retention.enabled);
        assert_eq!(old.wall_mount.style, WallMountStyle::None);
        assert!(old.wall_mount.export_hardware);
        assert_eq!(old.wall_mount.vertical_position_ratio, 0.28);
        assert_eq!(old.wall_mount.screw_countersink_depth_mm, 0.8);
        assert_eq!(old.wall_mount.screw_head_clearance_mm, 0.4);
        assert!(old.wall_mount.wide_edge_screws);

        let mut misplaced = GenerationSpec::default();
        misplaced.wall_mount.style = WallMountStyle::StraightPin;
        misplaced.wall_mount.vertical_position_ratio = 0.1;
        assert!(
            misplaced
                .validate()
                .unwrap_err()
                .to_string()
                .contains("between one-sixth and five-sixths")
        );

        let mut spec = GenerationSpec::default();
        spec.wall_mount.style = WallMountStyle::AngledPin;
        spec.wall_mount.target = WallMountTarget::Terrain;
        spec.wall_mount.thickness_mm =
            spec.wall_mount.wall_offset_mm + spec.base_mm - spec.wall_mount.depth_mm - 0.4;
        assert!(spec.validate().is_ok());
        let minimum_height = spec.base_mm;
        spec.wall_mount.thickness_mm += 0.01;
        assert!(
            spec.validate()
                .unwrap_err()
                .to_string()
                .contains("minimum piece height is too thin")
        );
        assert_eq!(spec.base_mm, minimum_height);

        spec.wall_mount.target = WallMountTarget::Tray;
        spec.tray.enabled = true;
        spec.wall_mount.thickness_mm =
            spec.wall_mount.wall_offset_mm + spec.tray.floor_mm - spec.wall_mount.depth_mm - 0.4;
        assert!(spec.validate().is_ok());
        let floor_height = spec.tray.floor_mm;
        spec.wall_mount.thickness_mm += 0.01;
        assert!(
            spec.validate()
                .unwrap_err()
                .to_string()
                .contains("display-base floor is too thin")
        );
        assert_eq!(spec.tray.floor_mm, floor_height);

        spec.wall_mount.target = WallMountTarget::Terrain;
        spec.wall_mount.thickness_mm = 1.2;
        spec.wall_mount.wall_offset_mm = 0.8;
        spec.wall_mount.depth_mm = 0.8;
        spec.wall_mount.screw_countersink_depth_mm = 0.81;
        assert!(
            spec.validate()
                .unwrap_err()
                .to_string()
                .contains("straight screw bore")
        );
        spec.wall_mount.screw_countersink_depth_mm = 0.8;
        spec.wall_mount.screw_head_clearance_mm = 3.0;
        assert!(
            spec.validate()
                .unwrap_err()
                .to_string()
                .contains("minimum piece height is too thin")
        );
    }

    #[test]
    fn tray_label_controls_parse_and_validate() {
        let mut spec: GenerationSpec = serde_json::from_value(serde_json::json!({
            "tray": {
                "enabled": true,
                "label_font": "b612_mono",
                "label_height_mm": 6.5,
                "label_position": "right"
            }
        }))
        .unwrap();
        assert_eq!(spec.tray.label_font, TrayLabelFont::B612Mono);
        assert_eq!(spec.tray.label_height_mm, 6.5);
        assert_eq!(spec.tray.label_position, TrayLabelPosition::Right);
        spec.place_name = "富士山".into();
        assert!(spec.validate().is_ok());

        spec.tray.label_height_mm = 1.4;
        assert!(
            spec.validate()
                .unwrap_err()
                .to_string()
                .contains("tray label height")
        );

        spec.tray.label_height_mm = 4.0;
        spec.place_name = "Fuji 🗻".into();
        assert!(
            spec.validate()
                .unwrap_err()
                .to_string()
                .contains("cannot render")
        );
    }

    #[test]
    fn thick_terrain_backs_and_tray_floors_have_a_usable_range() {
        let mut spec = GenerationSpec {
            base_mm: 20.0,
            tray: TraySpec {
                floor_mm: 20.0,
                ..TraySpec::default()
            },
            ..GenerationSpec::default()
        };
        assert!(spec.validate().is_ok());

        spec.base_mm = 20.01;
        assert!(spec.validate().unwrap_err().to_string().contains("20 mm"));
        spec.base_mm = 20.0;
        spec.tray.floor_mm = 20.01;
        assert!(spec.validate().unwrap_err().to_string().contains("20 mm"));
    }

    #[test]
    fn french_cleats_can_span_large_targets_but_keep_side_walls() {
        let mut spec = GenerationSpec {
            width_mm: 320.0,
            solid_model: true,
            wall_mount: WallMountSpec {
                style: WallMountStyle::FrenchCleat,
                cleat_width_mm: 300.0,
                ..WallMountSpec::default()
            },
            ..GenerationSpec::default()
        };
        assert!(spec.validate().is_ok());

        spec.solid_model = false;
        spec.rows = 4;
        spec.columns = 16;
        spec.wall_mount.cleat_width_mm = 316.0;
        assert!(spec.validate().is_ok());
        spec.wall_mount.cleat_width_mm = 316.01;
        assert!(
            spec.validate()
                .unwrap_err()
                .to_string()
                .contains("2 mm on each side")
        );
    }

    #[test]
    fn wall_mount_preflight_checks_the_full_tile_in_both_axes() {
        let mut spec = GenerationSpec {
            width_mm: 60.0,
            rows: 2,
            columns: 16,
            wall_mount: WallMountSpec {
                style: WallMountStyle::FrenchCleat,
                target: WallMountTarget::Terrain,
                ..WallMountSpec::default()
            },
            ..GenerationSpec::default()
        };
        let error = spec.validate().unwrap_err().to_string();
        assert!(error.contains("full terrain tile or display base"));

        spec.rows = 16;
        spec.columns = 2;
        assert!(spec.validate().is_ok());
    }

    #[test]
    fn tray_retention_requires_a_tray_and_keeps_wall_mounting_separate() {
        let mut spec = GenerationSpec::default();
        spec.puzzle_retention.enabled = true;
        assert!(
            spec.validate()
                .unwrap_err()
                .to_string()
                .contains("needs an enabled tray")
        );

        spec.tray.enabled = true;
        assert!(spec.validate().is_ok());
        spec.wall_mount.style = WallMountStyle::StraightPin;
        spec.wall_mount.target = WallMountTarget::Terrain;
        assert!(
            spec.validate()
                .unwrap_err()
                .to_string()
                .contains("mount the tray instead")
        );
        spec.wall_mount.target = WallMountTarget::Tray;
        assert!(spec.validate().is_ok());
    }

    #[test]
    fn tray_wall_mounts_require_a_tray() {
        let mut spec = GenerationSpec::default();
        spec.wall_mount.style = WallMountStyle::StraightPin;
        spec.wall_mount.target = WallMountTarget::Tray;
        assert!(
            spec.validate()
                .unwrap_err()
                .to_string()
                .contains("needs an enabled tray")
        );
        spec.tray.enabled = true;
        assert!(spec.validate().is_ok());
    }

    #[test]
    fn trails_activate_color_materials_on_their_own() {
        let mut spec = GenerationSpec::default();
        spec.color_output.enabled = false;
        assert!(!spec.uses_color_materials());
        spec.trails = vec![trail(vec![[46.85, -121.76], [46.86, -121.75]])];
        assert!(spec.uses_color_materials());
    }

    /// Both rail-family layers draw in their OWN color out of the box: that
    /// is what was asked for, and it is what a map is for. Each costs
    /// exactly one filament slot, no more, because the palette is dense.
    #[test]
    fn rail_defaults_draw_both_layers_in_their_own_colors() {
        let spec: GenerationSpec = serde_json::from_value(serde_json::json!({
            "color_output": { "enabled": true }
        }))
        .unwrap();
        assert!(spec.color_output.rail_enabled);
        assert!(spec.color_output.aerial_enabled);
        assert_eq!(spec.color_output.rail_style, RailStyle::Separate);
        assert_eq!(spec.color_output.aerial_style, AerialStyle::Separate);
        assert_eq!(spec.color_output.rail_lifecycle, RailLifecycle::Operational);
        assert_eq!(spec.color_output.rail_color, "#C43D3D");
        assert_eq!(spec.color_output.aerial_color, "#6C4CB6");
        assert_eq!(spec.color_output.rail_width_mm, 0.7);
        assert_eq!(spec.color_output.aerial_width_mm, 0.7);
        assert!(spec.uses_rail());
        assert!(spec.uses_aerial());
        assert!(spec.uses_separate_rail());
        assert!(spec.uses_separate_aerial());
        assert_eq!(spec.rail_line_style().class, SurfaceClass::Rail);
        assert_eq!(spec.aerial_line_style().class, SurfaceClass::Aerial);
        assert_eq!(
            spec.rail_line_style().width_mm,
            spec.color_output.rail_width_mm
        );

        // Eight slots: the base six plus one each, and no trail placeholder
        // between them.
        let palette = spec.material_palette(None);
        assert_eq!(palette.len(), 8);
        assert_eq!(palette.slot(SurfaceClass::Trail), None);
        assert_eq!(palette.slot(SurfaceClass::Rail), Some(6));
        assert_eq!(palette.slot(SurfaceClass::Aerial), Some(7));

        // Both layers ride on color output, like roads, so a model with
        // color output off pays nothing for either.
        let mut off = GenerationSpec::default();
        assert!(!off.color_output.enabled);
        assert!(!off.uses_rail());
        assert!(!off.uses_aerial());
        assert_eq!(off.material_palette(None).len(), 6);
        off.color_output.enabled = true;
        assert!(off.uses_rail_or_aerial());
    }

    /// The merged styles are the offered fallback for anyone who would
    /// rather not spend the spools: choosing them puts the layers back in
    /// the road class and the palette back to six.
    #[test]
    fn merged_styles_fold_both_layers_back_into_the_roads() {
        let mut spec = GenerationSpec::default();
        spec.color_output.enabled = true;
        spec.color_output.rail_style = RailStyle::WithRoads;
        spec.color_output.aerial_style = AerialStyle::WithRail;
        assert!(spec.uses_rail());
        assert!(spec.uses_aerial());
        assert!(!spec.uses_separate_rail());
        assert!(!spec.uses_separate_aerial());
        assert_eq!(spec.rail_line_style(), spec.aerial_line_style());
        assert_eq!(spec.rail_line_style().class, SurfaceClass::Road);
        assert_eq!(
            spec.rail_line_style().width_mm,
            spec.color_output.road_width_mm
        );
        assert_eq!(spec.material_palette(None).len(), 6);

        spec.color_output.aerial_style = AerialStyle::WithRoads;
        assert_eq!(spec.aerial_line_style().class, SurfaceClass::Road);
        assert_eq!(spec.material_palette(None).len(), 6);
    }

    /// The two layers switch independently, and each separate style costs
    /// exactly one slot — no placeholder rides along behind it.
    #[test]
    fn each_rail_layer_toggles_and_colors_on_its_own() {
        let mut spec = GenerationSpec::default();
        spec.color_output.enabled = true;
        // Start from the merged styles so each slot below is one this test
        // switched on deliberately.
        spec.color_output.rail_style = RailStyle::WithRoads;
        spec.color_output.aerial_style = AerialStyle::WithRail;

        spec.color_output.rail_enabled = false;
        assert!(!spec.uses_rail());
        assert!(spec.uses_aerial(), "lifts survive switching off trains");
        spec.color_output.rail_enabled = true;
        spec.color_output.aerial_enabled = false;
        assert!(spec.uses_rail(), "trains survive switching off lifts");
        assert!(!spec.uses_aerial());
        spec.color_output.aerial_enabled = true;

        // Separate rail alone: seven colors, rail in slot seven, and no
        // unreferenced trail placeholder in front of it.
        spec.color_output.rail_style = RailStyle::Separate;
        assert!(spec.uses_separate_rail());
        assert!(
            !spec.uses_separate_aerial(),
            "an aerial layer left on with_rail shares the rail slot"
        );
        let palette = spec.material_palette(None);
        assert_eq!(palette.len(), 7);
        assert_eq!(palette.colors()[6], "#C43D3D");
        assert_eq!(palette.slot(SurfaceClass::Rail), Some(6));
        assert_eq!(palette.slot(SurfaceClass::Trail), None);
        assert_eq!(palette.slot(SurfaceClass::Building), Some(5));

        // Separate aerial alongside it: eight colors, aerial last.
        spec.color_output.aerial_style = AerialStyle::Separate;
        assert!(spec.uses_separate_aerial());
        let palette = spec.material_palette(None);
        assert_eq!(palette.len(), 8);
        assert_eq!(palette.colors()[7], "#6C4CB6");
        assert_eq!(palette.slot(SurfaceClass::Aerial), Some(7));
        assert_eq!(palette.slot(SurfaceClass::Trail), None);

        // Separate aerial WITHOUT rail: seven colors, aerial in slot seven.
        spec.color_output.rail_style = RailStyle::WithRoads;
        let palette = spec.material_palette(None);
        assert_eq!(palette.len(), 7);
        assert_eq!(palette.colors()[6], "#6C4CB6");
        assert_eq!(palette.slot(SurfaceClass::Rail), None);
        assert_eq!(palette.slot(SurfaceClass::Aerial), Some(6));

        // Trails ahead of it push it along, and the order never reorders.
        spec.trails = vec![trail(vec![[46.85, -121.76], [46.86, -121.75]])];
        let palette = spec.material_palette(None);
        assert_eq!(palette.len(), 8);
        assert_eq!(palette.slot(SurfaceClass::Trail), Some(6));
        assert_eq!(palette.slot(SurfaceClass::Aerial), Some(7));

        // Switching both layers off drops the extra slots again.
        spec.trails.clear();
        spec.color_output.aerial_enabled = false;
        spec.color_output.rail_enabled = false;
        assert_eq!(spec.material_palette(None).len(), 6);
    }

    /// The aerial style chain is total, including when the railway layer it
    /// names is switched off.
    #[test]
    fn aerial_style_resolves_through_the_rail_layer() {
        let mut spec = GenerationSpec::default();
        spec.color_output.enabled = true;
        spec.color_output.rail_style = RailStyle::WithRoads;
        spec.color_output.aerial_style = AerialStyle::WithRail;
        spec.color_output.road_width_mm = 1.0;
        spec.color_output.rail_width_mm = 2.0;
        spec.color_output.aerial_width_mm = 3.0;
        let road = LineStyle {
            class: SurfaceClass::Road,
            width_mm: 1.0,
        };
        let rail = LineStyle {
            class: SurfaceClass::Rail,
            width_mm: 2.0,
        };
        let aerial = LineStyle {
            class: SurfaceClass::Aerial,
            width_mm: 3.0,
        };

        // with_rail over a rail layer set to with_roads.
        assert_eq!(spec.aerial_line_style(), road);

        // with_rail over a separately-styled rail layer: the rail class and
        // the RAIL width, so the two layers really are one look.
        spec.color_output.rail_style = RailStyle::Separate;
        assert_eq!(spec.aerial_line_style(), rail);

        // with_rail with the rail layer switched OFF falls through to roads.
        // It cannot draw nothing — the aerial layer's own switch decides
        // that — and it cannot borrow a rail color the archive never emits.
        spec.color_output.rail_enabled = false;
        assert_eq!(spec.aerial_line_style(), road);
        assert!(!spec.uses_separate_aerial());
        assert_eq!(spec.material_palette(None).slot(SurfaceClass::Rail), None);
        spec.color_output.rail_enabled = true;

        // The explicit styles ignore the rail layer entirely.
        spec.color_output.aerial_style = AerialStyle::WithRoads;
        assert_eq!(spec.aerial_line_style(), road);
        spec.color_output.rail_enabled = false;
        assert_eq!(spec.aerial_line_style(), road);
        spec.color_output.aerial_style = AerialStyle::Separate;
        assert_eq!(spec.aerial_line_style(), aerial);
        spec.color_output.rail_enabled = true;
        assert_eq!(spec.aerial_line_style(), aerial);
    }

    #[test]
    fn lifecycle_settings_are_cumulative_and_default_to_operational() {
        assert_eq!(RailLifecycle::default(), RailLifecycle::Operational);
        assert!(RailLifecycle::Disused > RailLifecycle::Operational);
        assert!(RailLifecycle::Abandoned > RailLifecycle::Disused);
        assert_eq!(RailLifecycle::Operational.name(), "operational");
        assert_eq!(RailLifecycle::Disused.name(), "disused");
        assert_eq!(RailLifecycle::Abandoned.name(), "abandoned");

        let spec: GenerationSpec = serde_json::from_value(serde_json::json!({
            "color_output": { "rail_lifecycle": "abandoned" }
        }))
        .unwrap();
        assert_eq!(
            spec.color_output.rail_lifecycle,
            RailLifecycle::Abandoned,
            "one setting covers both rail-family layers"
        );
        assert_eq!(
            serde_json::to_value(RailLifecycle::Operational).unwrap(),
            serde_json::json!("operational")
        );
    }

    #[test]
    fn rail_styles_parse_as_snake_case_and_rail_settings_validate() {
        let spec: GenerationSpec = serde_json::from_value(serde_json::json!({
            "color_output": { "rail_style": "with_roads" }
        }))
        .unwrap();
        assert_eq!(spec.color_output.rail_style, RailStyle::WithRoads);
        assert_eq!(
            serde_json::to_value(RailStyle::WithRoads).unwrap(),
            serde_json::json!("with_roads")
        );
        assert_eq!(
            serde_json::to_value(RailStyle::Separate).unwrap(),
            serde_json::json!("separate")
        );

        let spec: GenerationSpec = serde_json::from_value(serde_json::json!({
            "color_output": { "aerial_style": "with_roads" }
        }))
        .unwrap();
        assert_eq!(spec.color_output.aerial_style, AerialStyle::WithRoads);
        for (style, name) in [
            (AerialStyle::Separate, "separate"),
            (AerialStyle::WithRail, "with_rail"),
            (AerialStyle::WithRoads, "with_roads"),
        ] {
            assert_eq!(
                serde_json::to_value(style).unwrap(),
                serde_json::json!(name)
            );
        }

        let mut spec = GenerationSpec::default();
        spec.color_output.rail_width_mm = 0.2;
        let error = spec.validate().unwrap_err().to_string();
        assert!(error.contains("rail line width"));
        spec.color_output.rail_width_mm = 5.0;
        assert!(spec.validate().is_ok());
        spec.color_output.rail_width_mm = 5.1;
        assert!(spec.validate().is_err());

        spec.color_output.rail_width_mm = 0.7;
        spec.color_output.aerial_width_mm = 0.2;
        let error = spec.validate().unwrap_err().to_string();
        assert!(error.contains("aerialway line width"));
        spec.color_output.aerial_width_mm = 0.7;

        spec.color_output.rail_color = "steel".into();
        let error = spec.validate().unwrap_err().to_string();
        assert!(error.contains("rail color"));
        spec.color_output.rail_color = "#C43D3D".into();
        spec.color_output.aerial_color = "violet".into();
        let error = spec.validate().unwrap_err().to_string();
        assert!(error.contains("aerialway color"));
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
    fn close_view_line_scale_fades_with_map_span_and_validates() {
        let mut spec = GenerationSpec::default();
        assert_eq!(spec.close_view_line_scale(), 1.0);

        spec.ground_span_km = 2.0;
        assert_eq!(spec.close_view_line_scale(), 2.0);
        spec.ground_span_km = 0.25;
        assert_eq!(spec.close_view_line_scale(), 2.0);
        spec.ground_span_km = 6.0;
        assert!((1.0..2.0).contains(&spec.close_view_line_scale()));

        spec.color_output.line_scaling.scale_line_widths_by_span = false;
        assert_eq!(spec.close_view_line_scale(), 1.0);
        spec.color_output.line_scaling.close_view_width_multiplier = 3.1;
        assert!(spec.validate().is_err());
        spec.color_output.line_scaling.close_view_width_multiplier = 1.0;
        assert!(spec.validate().is_ok());
        spec.color_output.line_scaling.maximum_mapped_width_mm = 8.1;
        assert!(spec.validate().is_err());
        spec.color_output.line_scaling.maximum_mapped_width_mm = 0.4;
        assert!(spec.validate().is_ok());
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
