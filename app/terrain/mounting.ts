import type { GenerationSpec } from "./contracts";

export function maximumMountDepth(spec: GenerationSpec) {
  const thickness =
    spec.wall_mount.target === "tray" ? spec.tray.floor_mm : spec.base_mm;
  const pocketDepth =
    spec.wall_mount.thickness_mm - spec.wall_mount.wall_offset_mm;
  return Math.max(0.4, Math.min(3, thickness - pocketDepth - 0.4));
}

export function maximumWallPlateThickness(spec: GenerationSpec) {
  const thickness =
    spec.wall_mount.target === "tray" ? spec.tray.floor_mm : spec.base_mm;
  return Math.min(
    13,
    spec.wall_mount.wall_offset_mm +
      thickness -
      Math.max(
        spec.wall_mount.depth_mm,
        spec.wall_mount.screw_head_clearance_mm,
      ) -
      0.4,
  );
}

export function maximumRetentionHeight(spec: GenerationSpec) {
  return Math.max(
    0.4,
    Math.min(3, spec.base_mm - spec.puzzle_retention.clearance_mm - 0.4),
  );
}

export function wallMountTargetWidth(spec: GenerationSpec) {
  if (spec.wall_mount.target === "terrain") {
    return spec.width_mm;
  }

  const extra = (spec.tray.clearance_mm + spec.tray.rim_width_mm) * 2;
  const tileWidth = spec.width_mm + extra;
  if (spec.adjacent_columns > 1 || spec.adjacent_rows > 1) {
    return spec.tray.individual_tiles ? tileWidth : spec.width_mm;
  }
  return spec.tray.segment_columns > 1 || spec.tray.segment_rows > 1
    ? spec.width_mm / spec.tray.segment_columns
    : tileWidth;
}

export function maximumCleatWidth(spec: GenerationSpec) {
  return Math.max(8, Math.min(400, wallMountTargetWidth(spec) - 4));
}

export function wallHardwareQuantity(spec: GenerationSpec) {
  const superTileCount = spec.adjacent_columns * spec.adjacent_rows;
  if (spec.wall_mount.target === "tray") {
    return superTileCount > 1
      ? superTileCount
      : spec.tray.segment_columns * spec.tray.segment_rows;
  }
  return superTileCount;
}
