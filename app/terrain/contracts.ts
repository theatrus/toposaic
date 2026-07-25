export type TrailRoute = {
  name: string;
  /** Ordered [latitude, longitude] pairs in degrees. */
  points: [number, number][];
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
  adjacent_tile_column: number;
  adjacent_tile_row: number;
  clearance_mm: number;
  samples_per_piece: number;
  overlay_samples_per_piece: number;
  mesh_samples_across: number;
  overlay_samples_across: number;
  fine_dem_detail: boolean;
  solid_model: boolean;
  straight_piece_sides: boolean;
  puzzle_tabs: boolean;
  place_name: string;
  buildings: {
    enabled: boolean;
    z_scale: number;
  };
  tray: {
    enabled: boolean;
    individual_tiles: boolean;
    tray_color: string;
    contour_color: string;
    label_color: string;
    clearance_mm: number;
    rim_width_mm: number;
    floor_mm: number;
    rim_height_mm: number;
    contour_count: number;
    segment_columns: number;
    segment_rows: number;
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
    road_detail: "automatic" | "major" | "minor" | "streets" | "all";
    adaptive_road_widths: boolean;
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
  };
  surface_coverage?: {
    rock: number;
    forest: number;
    snow: number;
    water: number;
    road: number;
    building: number;
    trail?: number;
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
