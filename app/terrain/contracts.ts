export type TrailRoute = {
  name: string;
  /** Ordered [latitude, longitude] pairs in degrees. */
  points: [number, number][];
};

export type MarkerKind =
  | "building"
  | "dot"
  | "flag_hole"
  | "flag_label"
  | "surface_label"
  | "plaque_label";

export type LabelFont = "atkinson_hyperlegible" | "noto_sans" | "b612_mono";

export type MapMarker = {
  name: string;
  latitude: number;
  longitude: number;
  kind: MarkerKind;
  label_height_mm: number;
  /** Clockwise rotation on the north-up map. */
  rotation_degrees: number;
  dot_style?: {
    diameter_mm: number;
  } | null;
  flag_style?: {
    hole_diameter_mm: number;
    hole_depth_mm: number;
    fit_clearance_mm: number;
    label_font: LabelFont;
    label_height_mm: number;
    width_mm: number;
    height_mm: number;
    export_template: boolean;
  } | null;
  label_style?: {
    label_font: LabelFont;
    relief_mm: number;
    plaque_padding_mm: number;
    plaque_thickness_mm: number;
  } | null;
};

// The backend's surface classes, spelled the way its serializer does.
export type SurfaceClassKey =
  | "rock"
  | "forest"
  | "snow"
  | "water"
  | "road"
  | "building"
  | "trail"
  | "rail"
  | "aerial"
  | "ferry"
  | "marker"
  | "route_trail"
  | "aviation";

export type GenerationSpec = {
  center_lat: number;
  center_lon: number;
  elevation_source: "mapzen" | "mapterhorn";
  ground_span_km: number;
  /** Clockwise rotation of the model's top edge from true north. */
  terrain_rotation_degrees: number;
  map_frame: MapFrame | null;
  width_mm: number;
  model_outline: {
    shape: "rectangle" | "circle" | "ellipse" | "polygon";
    /** Points in the normalized, rotated model frame. */
    points: [number, number][];
  };
  rows: number;
  columns: number;
  base_mm: number;
  relief_mm: number;
  elevation_datum_m: number | null;
  elevation_m_per_mm: number | null;
  // How the vertical scale is chosen. "overall_height" fits the area's
  // relief into relief_mm — the default, and what relief_mm has always
  // meant. "multiplier" holds a fixed exaggeration instead and lets the
  // model's height follow the terrain, so separate areas and separately
  // generated tiles print comparably. A locked elevation_datum_m +
  // elevation_m_per_mm pair still overrides both.
  height_mode: "overall_height" | "multiplier";
  vertical_exaggeration: number;
  datum_reference: "area_minimum" | "sea_level" | "custom";
  custom_datum_m: number;
  adjacent_columns: number;
  adjacent_rows: number;
  super_tile_anchor: "top_left" | "center";
  adjacent_interlocks: boolean;
  outer_edge_interlocks: boolean;
  adjacent_tile_column: number;
  adjacent_tile_row: number;
  puzzle_seed: number;
  puzzle_tile_column: number;
  puzzle_tile_row: number;
  clearance_mm: number;
  samples_per_piece: number;
  overlay_samples_per_piece: number;
  // Option<u32> on the wire: null means "let the backend pick". Specs that
  // arrive from the service or from setup files can carry explicit nulls.
  mesh_samples_across: number | null;
  overlay_samples_across: number | null;
  fine_dem_detail: boolean;
  // Replace isolated wild elevation readings with their neighbourhood
  // median. On by default; see the model panel for why.
  despike_terrain: boolean;
  solid_model: boolean;
  straight_piece_sides: boolean;
  puzzle_tabs: boolean;
  place_name: string;
  buildings: {
    enabled: boolean;
    z_scale: number;
  };
  // Marine water: what the sea's printed surface is, and at which level.
  // bathymetric_relief — the draped output every setup has today — is the
  // default; the flat sea is opt-in. low_tide/high_tide resolve to msl
  // with a recorded warning until a regional tidal datum source lands.
  marine: {
    geometry: "flat_surface" | "bathymetric_relief";
    level: "msl" | "low_tide" | "high_tide" | "custom";
    custom_offset_m: number;
  };
  marker_settings: {
    color: string;
  };
  tray: {
    enabled: boolean;
    individual_tiles: boolean;
    contours_enabled: boolean;
    label_enabled: boolean;
    tray_color: string;
    contour_color: string;
    label_color: string;
    label_font: LabelFont;
    label_height_mm: number;
    label_position: "left" | "center" | "right";
    clearance_mm: number;
    rim_width_mm: number;
    floor_mm: number;
    rim_height_mm: number;
    contour_count: number;
    segment_columns: number;
    segment_rows: number;
  };
  puzzle_retention: {
    enabled: boolean;
    pin_diameter_mm: number;
    pin_height_mm: number;
    clearance_mm: number;
  };
  wall_mount: {
    style: "none" | "straight_pin" | "angled_pin" | "french_cleat";
    target: "terrain" | "tray";
    vertical_position_ratio: number;
    depth_mm: number;
    thickness_mm: number;
    wall_offset_mm: number;
    pin_diameter_mm: number;
    pin_count: number;
    pin_spacing_mm: number;
    cleat_width_mm: number;
    export_hardware: boolean;
    fit_clearance_mm: number;
    screw_hole_diameter_mm: number;
    screw_countersink_depth_mm: number;
    screw_head_clearance_mm: number;
    wide_edge_screws: boolean;
  };
  color_output: {
    enabled: boolean;
    threemf_style: "painted" | "project" | "geometry";
    // The slicer filament preset every slot of a "project" 3MF names. Left
    // unnamed, OrcaSlicer and Bambu Studio pick a material themselves.
    filament_profile:
      | "generic_pla"
      | "bambu_pla_basic"
      | "polylite_pla"
      | "polyterra_pla";
    // The order surface classes take filament slots. Classes left out
    // follow in the backend's fixed class order; empty means that fixed
    // order alone. Changes slot numbers only, never which classes print.
    filament_order: SurfaceClassKey[];
    forest_color: string;
    rock_color: string;
    snow_color: string;
    water_color: string;
    road_color: string;
    building_color: string;
    route_trail_color: string;
    trail_color: string;
    trail_width_mm: number;
    roads_enabled: boolean;
    // Ground track: trains, trams, metros, monorails, funiculars. The
    // layer switches on its own, apart from roads; "with_roads" paints it
    // in the road color, so the print needs no slot of its own.
    rail_enabled: boolean;
    rail_color: string;
    rail_width_mm: number;
    rail_style: "separate" | "with_roads";
    // Which lifecycle states either rail-family layer draws, cumulative.
    // One setting governs railways and aerialways together.
    rail_lifecycle: "operational" | "disused" | "abandoned";
    // Lines that hang from cables: cable cars, gondolas, chair lifts, drag
    // lifts, rope tows. "with_rail" follows the railway layer, but only
    // while railways are enabled; otherwise it falls through to roads.
    aerial_enabled: boolean;
    aerial_color: string;
    aerial_width_mm: number;
    aerial_style: "separate" | "with_rail" | "with_roads";
    // Ways OpenStreetMap tags route=ferry. No lifecycle setting: there is
    // no disused-ferry convention the way there is for track.
    ferry_enabled: boolean;
    ferry_color: string;
    ferry_width_mm: number;
    ferry_style: "separate" | "with_roads";
    road_detail: "automatic" | "major" | "minor" | "streets" | "all";
    adaptive_road_widths: boolean;
    scale_line_widths_by_span: boolean;
    close_view_width_multiplier: number;
    maximum_mapped_width_mm: number;
    osm_water_enabled: boolean;
    waterway_coverage_percent: number;
    road_width_mm: number;
    road_height_mm: number;
    bridge_structure: "floating" | "supported";
    bridge_thickness_mm: number;
    minimum_patch_mm: number;
    edge_bleed_mm: number;
    aviation_enabled: boolean;
    aviation_runways_enabled: boolean;
    aviation_taxiways_enabled: boolean;
    aviation_aprons_enabled: boolean;
    aviation_helipads_enabled: boolean;
    aviation_style: "separate" | "follow_roads";
    aviation_color: string;
    aviation_height_mm: number;
    maximum_aviation_width_mm: number;
    aviation_detail_span_km: number;
    class_borders: "blocky" | "smooth";
    border_smoothing_range_cells: number;
    border_smoothing_nugget: number;
    forest_slope_gate: boolean;
    forest_slope_limit_degrees: number;
    steep_forest_target: "rock" | "snow";
    snow_slope_gate: boolean;
    water_slope_gate: boolean;
    water_slope_limit_degrees: number;
    osm_water_slope_gate: boolean;
    osm_water_slope_limit_degrees: number;
    snow_slope_limit_degrees: number;
    ground_colors: "mapped" | "satellite" | "hybrid";
    ground_color_count: number;
    ground_color_minimum_share: number;
    ground_shadow_normalization: number;
    locked_ground_palette?: string[];
  };
  trails: TrailRoute[];
  markers: MapMarker[];
};

export type MapFrame = {
  origin_lat: number;
  origin_lon: number;
  origin_tile_column: number;
  origin_tile_row: number;
};

export type CacheCategoryKey =
  | "elevation"
  | "world_cover"
  | "osm"
  | "imagery"
  | "datum"
  | "places";

export type CacheCategory = {
  key: CacheCategoryKey;
  bytes: number;
  entries: number;
};

export type CacheStats = {
  total_bytes: number;
  categories: CacheCategory[];
};

export type CacheClearResult = {
  removed_bytes: number;
  removed_entries: number;
};

// What a finished job's source bundle would hold. Jobs generated before
// source bundles existed report available: false and nothing else, so the
// download is simply not offered for them.
export type SourceBundleSummary = {
  available: boolean;
  files?: number;
  bytes?: number;
  name?: string;
  // Set once the bundle has been built for this job, so a reload can offer
  // the file itself rather than the build step again.
  built_bytes?: number | null;
};

export type SourceImportResult = {
  report: {
    place_name: string;
    added: number;
    added_bytes: number;
    already_present: number;
    rejected: number;
  };
  spec: GenerationSpec;
};

// One superseded spec of a setup, kept so an overwrite can be walked back.
export type SetupVersion = {
  id: string;
  // When the spec it replaced was written.
  saved_at: string;
  spec: GenerationSpec;
};

export type SavedSetup = {
  id: string;
  name: string;
  created_at: string;
  updated_at: string;
  spec: GenerationSpec;
};

export type Artifact = {
  name: string;
  media_type: string;
  bytes: number;
};

export type ArtifactFeedback = {
  name: string;
  state: "saving" | "saved" | "sent";
};

export type GenerationControlTab =
  | "model"
  | "surface"
  | "buildings"
  | "markers"
  | "colors"
  | "mounting"
  | "output";

export type GenerationFailure = {
  title: string;
  message: string;
  technical_detail: string;
  control_tab?: GenerationControlTab;
  piece?: {
    row: number;
    column: number;
  };
};

export type Job = {
  id: string;
  status: "queued" | "running" | "complete" | "failed" | "canceled";
  progress: number;
  artifacts: Artifact[];
  error?: string | null;
  failure?: GenerationFailure | null;
  spec: GenerationSpec;
};

export type PreviewData = {
  width: number;
  height: number;
  values: number[];
  rows: number;
  columns: number;
  solid_model?: boolean;
  surface_classes?: number[];
  // Satellite-discovered ground colors, in palette order. A surface_classes
  // index at or past the fixed class count names one of these.
  ground_palette?: string[];
  surface_palette?: {
    rock: string;
    forest: string;
    snow: string;
    water: string;
    road: string;
    building: string;
    route_trail?: string;
    trail?: string;
    rail?: string;
    aerialway?: string;
    ferry?: string;
    aviation?: string;
    marker?: string;
  };
  surface_coverage?: {
    rock: number;
    forest: number;
    snow: number;
    water: number;
    road: number;
    building: number;
    route_trail?: number;
    trail?: number;
    rail?: number;
    aerialway?: number;
    ferry?: number;
    aviation?: number;
    marker?: number;
  };
  surface_source?: string;
  minimum_elevation_m?: number;
  maximum_elevation_m?: number;
  height_frame_compatible?: boolean;
  /** Draft meshes made by the same Rust builders used for export. */
  model_meshes?: Array<{
    kind: "terrain" | "tray";
    name: string;
    vertices: [number, number, number][];
    triangles: [number, number, number][];
    /** Raw PrintMaterial indices, one per triangle. */
    materials: number[];
  }>;
  /** min x, min y, min z, max x, max y, max z in assembled print mm. */
  model_bounds_mm?: [number, number, number, number, number, number];
  /** Exact terrain footprint inside the assembled preview: min x/y, max x/y. */
  model_terrain_bounds_mm?: [number, number, number, number];
  model_preview_detail?: string;
  model_preview_error?: string;
};

export type PlaceResult = {
  display_name: string;
  latitude: number;
  longitude: number;
  category: string;
  kind: string;
};
