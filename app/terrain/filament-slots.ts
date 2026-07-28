import type { GenerationSpec, SurfaceClassKey } from "./contracts";

// The backend's fixed class order — `SurfaceClass::ALL` in
// crates/toposaic-core/src/spec.rs. Classes missing from
// `filament_order` follow in this order.
export const DEFAULT_CLASS_ORDER: readonly SurfaceClassKey[] = [
  "rock",
  "forest",
  "snow",
  "water",
  "road",
  "building",
  "trail",
  "rail",
  "aerial",
  "marker",
  "route_trail",
];

const CLASS_LABELS: Record<SurfaceClassKey, string> = {
  rock: "Rock",
  forest: "Forest",
  snow: "Snow",
  water: "Water",
  road: "Route",
  building: "Building",
  trail: "Imported trail",
  rail: "Railway",
  aerial: "Aerial lift",
  marker: "Map marker",
  route_trail: "Trail",
};

export type FilamentSlotEntry = {
  classKey: SurfaceClassKey;
  label: string;
  color: string;
  // 1-based filament number, matching the slicer's list. Classes sharing
  // a color share a number.
  filament: number;
};

// The classes the current settings put in the 3MF, in slot order. Mirrors
// the backend's palette for display: it assumes the map contains every
// enabled layer, so a layer the map turns out to lack gives its number up
// and later ones move down.
export function filamentSlotEntries(spec: GenerationSpec): FilamentSlotEntry[] {
  const colors: Record<SurfaceClassKey, string> = {
    rock: spec.color_output.rock_color,
    forest: spec.color_output.forest_color,
    snow: spec.color_output.snow_color,
    water: spec.color_output.water_color,
    road: spec.color_output.road_color,
    building: spec.color_output.building_color,
    trail: spec.color_output.trail_color,
    rail: spec.color_output.rail_color,
    aerial: spec.color_output.aerial_color,
    marker: spec.marker_settings.color,
    route_trail: spec.color_output.route_trail_color,
  };
  const emits: Record<SurfaceClassKey, boolean> = {
    rock: true,
    forest: true,
    snow: true,
    water: true,
    road: true,
    building: true,
    trail: spec.trails.length > 0,
    rail:
      spec.color_output.rail_enabled &&
      spec.color_output.rail_style === "separate",
    aerial:
      spec.color_output.aerial_enabled &&
      spec.color_output.aerial_style === "separate",
    marker: spec.markers.some(
      (marker) => marker.kind !== "flag_hole" && marker.kind !== "flag_label",
    ),
    route_trail: spec.color_output.roads_enabled,
  };
  const slotByColor = new Map<string, number>();
  const entries: FilamentSlotEntry[] = [];
  for (const classKey of effectiveClassOrder(spec)) {
    if (!emits[classKey]) {
      continue;
    }
    const color = colors[classKey].toUpperCase();
    let slot = slotByColor.get(color);
    if (slot === undefined) {
      slot = slotByColor.size;
      slotByColor.set(color, slot);
    }
    entries.push({
      classKey,
      label: CLASS_LABELS[classKey],
      color,
      filament: slot + 1,
    });
  }
  return entries;
}

// Every class once: the saved order first, then the rest in default order.
export function effectiveClassOrder(spec: GenerationSpec): SurfaceClassKey[] {
  const seen = new Set<SurfaceClassKey>();
  const order: SurfaceClassKey[] = [];
  for (const classKey of [
    ...spec.color_output.filament_order,
    ...DEFAULT_CLASS_ORDER,
  ]) {
    if (DEFAULT_CLASS_ORDER.includes(classKey) && !seen.has(classKey)) {
      seen.add(classKey);
      order.push(classKey);
    }
  }
  return order;
}

// The saved order after moving one displayed class a step earlier or later
// among the displayed classes. Returns null when the move falls off either
// end. Classes not displayed keep following in default order.
export function moveFilamentClass(
  spec: GenerationSpec,
  classKey: SurfaceClassKey,
  direction: "earlier" | "later",
): SurfaceClassKey[] | null {
  const displayed = filamentSlotEntries(spec).map((entry) => entry.classKey);
  const from = displayed.indexOf(classKey);
  const to = direction === "earlier" ? from - 1 : from + 1;
  if (from < 0 || to < 0 || to >= displayed.length) {
    return null;
  }
  [displayed[from], displayed[to]] = [displayed[to], displayed[from]];
  return displayed;
}
