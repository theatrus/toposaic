import type { GenerationSpec } from "./contracts";

export const initialSpec: GenerationSpec = {
  center_lat: 46.8523,
  center_lon: -121.7603,
  elevation_source: "mapzen",
  ground_span_km: 18,
  width_mm: 180,
  rows: 10,
  columns: 10,
  base_mm: 2.4,
  relief_mm: 28,
  elevation_datum_m: null,
  elevation_m_per_mm: null,
  adjacent_columns: 1,
  adjacent_rows: 1,
  super_tile_anchor: "top_left",
  adjacent_interlocks: false,
  adjacent_tile_column: 0,
  adjacent_tile_row: 0,
  clearance_mm: 0.14,
  samples_per_piece: 64,
  overlay_samples_per_piece: 112,
  mesh_samples_across: 640,
  overlay_samples_across: 640,
  fine_dem_detail: false,
  solid_model: false,
  straight_piece_sides: false,
  puzzle_tabs: true,
  place_name: "Mount Rainier",
  buildings: {
    enabled: false,
    z_scale: 5,
  },
  tray: {
    enabled: true,
    individual_tiles: false,
    tray_color: "#252822",
    contour_color: "#E7E4D8",
    label_color: "#F4F3EC",
    clearance_mm: 0.6,
    rim_width_mm: 8,
    floor_mm: 1.6,
    rim_height_mm: 3.2,
    contour_count: 18,
    segment_columns: 1,
    segment_rows: 1,
  },
  color_output: {
    enabled: true,
    forest_color: "#28543A",
    rock_color: "#7C7468",
    snow_color: "#F4F3EC",
    water_color: "#2F76B5",
    road_color: "#D8A33C",
    building_color: "#B8A890",
    roads_enabled: true,
    road_detail: "automatic",
    adaptive_road_widths: true,
    osm_water_enabled: true,
    waterway_coverage_percent: 12,
    road_width_mm: 0.7,
    road_height_mm: 0.2,
    bridge_structure: "floating",
    bridge_thickness_mm: 1.2,
    minimum_patch_mm: 1.2,
  },
};

export const MIN_GROUND_SPAN_KM = 0.25;
export const MAX_GROUND_SPAN_KM = 80;
export const MAX_SUPER_TILE_SIDE = 12;
export const MAX_ASSEMBLED_SAMPLES = 2048;
const FINE_DEM_TARGET_RESOLUTION_M = 0.25;
export const FINE_DEM_MAX_SPAN_KM = 2;

export const MESH_QUALITY_OPTIONS = [
  { label: "Draft", samples: 384, note: "Fast export" },
  { label: "Standard", samples: 640, note: "Most prints" },
  { label: "High", samples: 1024, note: "Fine FDM" },
  {
    label: "Ultra",
    samples: MAX_ASSEMBLED_SAMPLES,
    note: "0.2 mm or resin",
  },
] as const;

// Mirrors RoadDetail::resolve in crates/toposaic-core/src/spec.rs; the backend
// owns the real decision, so change the spans in both places together.
export function automaticRoadDetail(groundSpanKm: number) {
  if (groundSpanKm <= 2) return "all streets, paths, and trails";
  if (groundSpanKm <= 8) return "local streets";
  if (groundSpanKm <= 20) return "minor roads";
  return "major roads";
}

function meshPieceCount(spec: GenerationSpec) {
  return spec.solid_model ? 1 : Math.max(spec.rows, spec.columns);
}

function samplesPerPieceForTotal(total: number, pieceCount: number) {
  return Math.ceil(total / Math.max(1, pieceCount));
}

export function terrainSamplesAcross(spec: GenerationSpec) {
  let total = spec.mesh_samples_across;
  if (
    spec.fine_dem_detail &&
    spec.elevation_source === "mapterhorn" &&
    spec.ground_span_km <= FINE_DEM_MAX_SPAN_KM
  ) {
    total = Math.max(
      total,
      Math.min(
        MAX_ASSEMBLED_SAMPLES,
        Math.ceil(
          (spec.ground_span_km * 1000) / FINE_DEM_TARGET_RESOLUTION_M,
        ),
      ),
    );
  }
  return total;
}

export function terrainSamplesPerPiece(spec: GenerationSpec) {
  return samplesPerPieceForTotal(
    terrainSamplesAcross(spec),
    meshPieceCount(spec),
  );
}

export function overlaySamplesPerPiece(spec: GenerationSpec) {
  const pieceCount = meshPieceCount(spec);
  return samplesPerPieceForTotal(spec.overlay_samples_across, pieceCount);
}

export function effectiveMeshSamples(spec: GenerationSpec) {
  const terrain = terrainSamplesPerPiece(spec);
  const overlay =
    spec.color_output.enabled || spec.buildings.enabled
      ? overlaySamplesPerPiece(spec)
      : 0;
  return Math.max(terrain, overlay);
}

export function assembledMeshSamples(spec: GenerationSpec) {
  const overlays =
    spec.color_output.enabled || spec.buildings.enabled
      ? spec.overlay_samples_across
      : 0;
  return Math.max(terrainSamplesAcross(spec), overlays);
}

export function groundMeshSpacing(spec: GenerationSpec) {
  return (spec.ground_span_km * 1000) / assembledMeshSamples(spec);
}

export function formatGroundSpacing(metres: number) {
  return metres < 1 ? metres.toFixed(2) : metres.toFixed(1);
}

export function deriveHeightFrame(
  sampled: { minimum_elevation_m: number; maximum_elevation_m: number },
  reliefMm: number,
) {
  const sampledRange = Math.max(
    1,
    sampled.maximum_elevation_m - sampled.minimum_elevation_m,
  );
  const margin = Math.max(2, sampledRange * 0.02);
  const datum = Math.floor((sampled.minimum_elevation_m - margin) * 10) / 10;
  const metresPerMm = Math.max(
    0.1,
    (sampled.maximum_elevation_m - datum) / reliefMm,
  );
  return { datum, metresPerMm };
}
