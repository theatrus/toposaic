import type { GenerationSpec } from "./contracts";

export function maximumMountDepth(spec: GenerationSpec) {
  const thickness =
    spec.wall_mount.target === "tray" ? spec.tray.floor_mm : spec.base_mm;
  return Math.max(0.4, Math.min(3, thickness - 0.4));
}

export function maximumRetentionHeight(spec: GenerationSpec) {
  return Math.max(
    0.4,
    Math.min(3, spec.base_mm - spec.puzzle_retention.clearance_mm - 0.4),
  );
}

export function wallHardwareQuantity(spec: GenerationSpec) {
  const superTileCount = spec.adjacent_columns * spec.adjacent_rows;
  if (spec.wall_mount.target === "tray") {
    return superTileCount > 1
      ? superTileCount
      : spec.tray.segment_columns * spec.tray.segment_rows;
  }
  return superTileCount * (spec.solid_model ? 1 : spec.rows * spec.columns);
}
