export type TrailRoute = {
  name: string;
  /** Ordered [latitude, longitude] pairs in degrees. */
  points: [number, number][];
};

export type MarkerKind = "building" | "dot" | "flag_hole";

export type MapMarker = {
  name: string;
  latitude: number;
  longitude: number;
  kind: MarkerKind;
};

export type GenerationSpec = {
  center_lat: number;
  center_lon: number;
  elevation_source: "mapzen" | "mapterhorn";
  ground_span_km: number;
  width_mm: number;
  rows: number;
  columns: number;
  base_mm: number;
  relief_mm: number;
  elevation_datum_m: number | null;
  elevation_m_per_mm: number | null;
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
  marker_settings: {
    color: string;
    dot_diameter_mm: number;
    hole_diameter_mm: number;
    hole_depth_mm: number;
    flag_clearance_mm: number;
    export_flag_template: boolean;
  };
  tray: {
    enabled: boolean;
    individual_tiles: boolean;
    contours_enabled: boolean;
    tray_color: string;
    contour_color: string;
    label_color: string;
    label_font: "atkinson_hyperlegible" | "noto_sans" | "b612_mono";
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
    forest_color: string;
    rock_color: string;
    snow_color: string;
    water_color: string;
    road_color: string;
    building_color: string;
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
    class_borders: "blocky" | "smooth";
    border_smoothing_range_cells: number;
    border_smoothing_nugget: number;
    forest_slope_gate: boolean;
    forest_slope_limit_degrees: number;
    steep_forest_target: "rock" | "snow";
    snow_slope_gate: boolean;
    snow_slope_limit_degrees: number;
  };
  trails: TrailRoute[];
  markers: MapMarker[];
};

export type CacheCategoryKey = "elevation" | "world_cover" | "osm" | "places";

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

export type Job = {
  id: string;
  status: "queued" | "running" | "complete" | "failed" | "canceled";
  progress: number;
  artifacts: Artifact[];
  error?: string | null;
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
  surface_palette?: {
    rock: string;
    forest: string;
    snow: string;
    water: string;
    road: string;
    building: string;
    trail?: string;
    rail?: string;
    aerialway?: string;
    marker?: string;
  };
  surface_coverage?: {
    rock: number;
    forest: number;
    snow: number;
    water: number;
    road: number;
    building: number;
    trail?: number;
    rail?: number;
    aerialway?: number;
    marker?: number;
  };
  surface_source?: string;
  minimum_elevation_m?: number;
  maximum_elevation_m?: number;
  height_frame_compatible?: boolean;
};

export type PlaceResult = {
  display_name: string;
  latitude: number;
  longitude: number;
  category: string;
  kind: string;
};
