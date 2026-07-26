import type { GenerationSpec } from "./contracts";

// Client default for both sample totals; also stands in when a spec carries
// an explicit null ("backend picks"), so label math never divides by zero.
export const DEFAULT_SAMPLES_ACROSS = 640;

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
  mesh_samples_across: DEFAULT_SAMPLES_ACROSS,
  overlay_samples_across: DEFAULT_SAMPLES_ACROSS,
  fine_dem_detail: false,
  despike_terrain: true,
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
    contours_enabled: true,
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
  wall_mount: {
    style: "none",
    target: "terrain",
    depth_mm: 0.8,
    pin_diameter_mm: 4,
  },
  color_output: {
    enabled: true,
    // "project" keeps today's output — embedded filament colors and purge
    // settings for one-click Bambu color setups. mergeSpecDefaults fills it
    // into setups saved before the field existed.
    threemf_style: "project",
    forest_color: "#28543A",
    rock_color: "#7C7468",
    snow_color: "#F4F3EC",
    water_color: "#2F76B5",
    road_color: "#D8A33C",
    building_color: "#B8A890",
    // High-vis raspberry magenta, clearly apart from the gold route color.
    trail_color: "#D6336C",
    trail_width_mm: 0.7,
    roads_enabled: true,
    // Railways switch on apart from roads, and are on by default: a map
    // that drops the rail network is simply wrong. The default style draws
    // them in the road color, so no project gains a filament slot. Mirrors
    // ColorOutputSpec in crates/toposaic-core/src/spec.rs — change both
    // together.
    rail_enabled: true,
    // Slate blue-grey: steel against the gold roads and raspberry trails.
    rail_color: "#4A5568",
    rail_width_mm: 0.7,
    // Picked out in their own color, which is the point of drawing them.
    // The 3MF packs its palette from the mapped data, so the slot costs
    // nothing in an area with no railways.
    rail_style: "separate",
    // Running lines only, which is what every model drew before the
    // setting existed. One setting governs railways and aerial lifts.
    rail_lifecycle: "operational",
    aerial_enabled: true,
    // Signal violet, apart from the steel railways and gold roads.
    aerial_color: "#6C4CB6",
    aerial_width_mm: 0.7,
    // Lifts get their own color too: a chair lift is neither a road nor a
    // railway, and the map is worth more when it says so.
    aerial_style: "separate",
    road_detail: "automatic",
    adaptive_road_widths: true,
    osm_water_enabled: true,
    waterway_coverage_percent: 12,
    road_width_mm: 0.7,
    road_height_mm: 0.2,
    bridge_structure: "floating",
    bridge_thickness_mm: 1.2,
    minimum_patch_mm: 1.2,
    class_borders: "smooth",
    border_smoothing_range_cells: 2.5,
    border_smoothing_nugget: 0.05,
    forest_slope_gate: true,
    forest_slope_limit_degrees: 55,
    steep_forest_target: "rock",
    snow_slope_gate: true,
    snow_slope_limit_degrees: 65,
  },
  trails: [],
};

// Fill any field a saved spec is missing with the client default, so setups
// saved before a field existed still recall cleanly.
export function mergeSpecDefaults(saved: Partial<GenerationSpec>): GenerationSpec {
  return {
    ...initialSpec,
    ...saved,
    // The wire type is Option<u32>: an explicit null means "backend picks".
    // Spreading keeps nulls, so coalesce them to the client defaults here.
    mesh_samples_across:
      saved.mesh_samples_across ?? initialSpec.mesh_samples_across,
    overlay_samples_across:
      saved.overlay_samples_across ?? initialSpec.overlay_samples_across,
    buildings: { ...initialSpec.buildings, ...saved.buildings },
    tray: { ...initialSpec.tray, ...saved.tray },
    wall_mount: { ...initialSpec.wall_mount, ...saved.wall_mount },
    color_output: { ...initialSpec.color_output, ...saved.color_output },
    trails: saved.trails ?? [],
  };
}

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

// Which class a drawn layer's lines land in. The surface classes the
// preview reports are raw material indices, and several layers can share
// one, so the legend has to resolve the same chain the backend does.
export type LineClass = "road" | "rail" | "aerialway";

// Mirrors GenerationSpec::rail_line_style in
// crates/toposaic-core/src/spec.rs. It answers "how would railways look",
// so it ignores rail_enabled, exactly as the Rust does.
export function railLineClass(
  colorOutput: GenerationSpec["color_output"],
): LineClass {
  return colorOutput.rail_style === "separate" ? "rail" : "road";
}

// Mirrors GenerationSpec::aerial_line_style. The chain is total: with
// railways switched off, "follow railways" falls through to roads rather
// than drawing nothing or borrowing a rail color the model never emits.
export function aerialLineClass(
  colorOutput: GenerationSpec["color_output"],
): LineClass {
  if (colorOutput.aerial_style === "separate") return "aerialway";
  if (colorOutput.aerial_style === "with_roads") return "road";
  return colorOutput.rail_enabled ? railLineClass(colorOutput) : "road";
}

function meshPieceCount(spec: GenerationSpec) {
  return spec.solid_model ? 1 : Math.max(spec.rows, spec.columns);
}

function samplesPerPieceForTotal(total: number, pieceCount: number) {
  return Math.ceil(total / Math.max(1, pieceCount));
}

export function terrainSamplesAcross(spec: GenerationSpec) {
  // Specs straight off the wire can carry null; treat it as unset.
  let total = spec.mesh_samples_across ?? DEFAULT_SAMPLES_ACROSS;
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
  return samplesPerPieceForTotal(
    // Specs straight off the wire can carry null; treat it as unset.
    spec.overlay_samples_across ?? DEFAULT_SAMPLES_ACROSS,
    pieceCount,
  );
}

// Mirrors uses_color_materials in crates/toposaic-core/src/spec.rs: the
// backend raises sampling to the overlay grid whenever color output,
// buildings, or imported trails are in play — trails alone count too.
function usesColorMaterials(spec: GenerationSpec) {
  return (
    spec.color_output.enabled ||
    spec.buildings.enabled ||
    spec.trails.length > 0
  );
}

export function effectiveMeshSamples(spec: GenerationSpec) {
  const terrain = terrainSamplesPerPiece(spec);
  const overlay = usesColorMaterials(spec) ? overlaySamplesPerPiece(spec) : 0;
  return Math.max(terrain, overlay);
}

export function assembledMeshSamples(spec: GenerationSpec) {
  // Match the Rust side (assembled_terrain_samples in
  // crates/toposaic-core/src/spec.rs): the total rounds up to whole
  // samples per piece, so the assembled figure is per-piece × piece count.
  const pieceCount = meshPieceCount(spec);
  const terrain = terrainSamplesPerPiece(spec) * pieceCount;
  const overlays = usesColorMaterials(spec)
    ? overlaySamplesPerPiece(spec) * pieceCount
    : 0;
  return Math.max(terrain, overlays);
}

export function groundMeshSpacing(spec: GenerationSpec) {
  return (spec.ground_span_km * 1000) / assembledMeshSamples(spec);
}

export function formatGroundSpacing(metres: number) {
  return metres < 1 ? metres.toFixed(2) : metres.toFixed(1);
}

const BYTE_UNITS = ["B", "KB", "MB", "GB"] as const;

// Human cache sizes for the settings pane: whole bytes, then one decimal
// under ten units, then whole units (1.4 KB, 50 MB, 2.0 GB).
export function formatBytes(bytes: number) {
  let value = Math.max(0, bytes);
  let unit = 0;
  while (value >= 1024 && unit < BYTE_UNITS.length - 1) {
    value /= 1024;
    unit += 1;
  }
  const text =
    unit === 0
      ? `${Math.round(value)}`
      : value >= 10
        ? value.toFixed(0)
        : value.toFixed(1);
  return `${text} ${BYTE_UNITS[unit]}`;
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
