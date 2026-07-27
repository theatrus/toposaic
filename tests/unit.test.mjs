import assert from "node:assert/strict";
import test from "node:test";
import {
  previewInitialCameraPosition,
  previewWorldX,
} from "../app/terrain/preview-orientation.ts";
import {
  aerialLineClass,
  assembledMeshSamples,
  effectiveMeshSamples,
  formatBytes,
  groundMeshSpacing,
  initialSpec,
  limitMarkerName,
  limitPlaceName,
  mergeSpecDefaults,
  minimumMappedWidthCap,
  normalizeMappedWidthCap,
  railLineClass,
  terrainSamplesAcross,
} from "../app/terrain/config.ts";
import {
  maximumCleatWidth,
  maximumMountDepth,
  maximumRetentionHeight,
  maximumWallPlateThickness,
  wallMountTargetWidth,
  wallHardwareQuantity,
} from "../app/terrain/mounting.ts";
import { isVersionNewer } from "../app/updates/version.ts";
import { describeJobFailure } from "../app/terrain/generation-failure.ts";
import { normalizedMapPoint } from "../app/terrain/geo.ts";

test("maps vector markers into the model frame across the date line", () => {
  const center = normalizedMapPoint(
    { center_lat: 46.8523, center_lon: -121.7603, ground_span_km: 18 },
    46.8523,
    -121.7603,
  );
  assert.ok(Math.abs(center.u - 0.5) < 1e-12);
  assert.ok(Math.abs(center.v - 0.5) < 1e-12);
  const wrapped = normalizedMapPoint(
    { center_lat: 0, center_lon: 179.99, ground_span_km: 18 },
    0,
    -179.99,
  );
  assert.ok(wrapped.u > 0.5 && wrapped.u < 1);
  assert.equal(wrapped.v, 0.5);
});

test("compares stable and prerelease app versions", () => {
  assert.equal(isVersionNewer("v0.2.0", "0.1.9"), true);
  assert.equal(isVersionNewer("v0.1.10", "0.1.9"), true);
  assert.equal(isVersionNewer("v0.1.0", "0.1.0-beta.2"), true);
  assert.equal(isVersionNewer("v0.1.0-beta.2", "0.1.0"), false);
  assert.equal(isVersionNewer("not-a-version", "0.1.0"), false);
});

test("keeps old job errors readable when structured failure data is absent", () => {
  const failure = describeJobFailure({
    status: "failed",
    error: "build piece 6, 7: triangulate terrain outline",
  });
  assert.equal(failure.title, "Could not build puzzle piece 6,7");
  assert.equal(failure.control_tab, "model");
  assert.deepEqual(failure.piece, { row: 6, column: 7 });
  assert.match(failure.technical_detail, /triangulate terrain outline/);
});

test("limits place names by Unicode characters without splitting a pair", () => {
  const limited = limitPlaceName(`${"山".repeat(47)}𠮷余`);
  assert.equal(Array.from(limited).length, 48);
  assert.equal(limited.endsWith("𠮷"), true);
  assert.equal(limited.includes("�"), false);
});

test("limits marker label names by Unicode characters", () => {
  const limited = limitMarkerName(`${"川".repeat(79)}𠮷余`);
  assert.equal(Array.from(limited).length, 80);
  assert.equal(limited.endsWith("𠮷"), true);
});

test("defaults the 3MF style to the embedded-settings project output", () => {
  assert.equal(initialSpec.color_output.threemf_style, "project");
  // Setups saved before the field existed recall with the same default, so
  // existing users keep today's one-click color behavior.
  const oldColorOutput = { ...initialSpec.color_output };
  delete oldColorOutput.threemf_style;
  const merged = mergeSpecDefaults({ color_output: oldColorOutput });
  assert.equal(merged.color_output.threemf_style, "project");
});

test("marker settings keep only the shared material color", () => {
  const merged = mergeSpecDefaults({
    marker_settings: undefined,
    markers: undefined,
  });
  assert.deepEqual(merged.markers, []);
  assert.deepEqual(merged.marker_settings, { color: "#E24A33" });
});

test("defaults close-view line scaling on and recalls old setups", () => {
  assert.equal(initialSpec.color_output.scale_line_widths_by_span, true);
  assert.equal(initialSpec.color_output.close_view_width_multiplier, 2);
  assert.equal(initialSpec.color_output.maximum_mapped_width_mm, 4);
  const oldColorOutput = { ...initialSpec.color_output };
  delete oldColorOutput.scale_line_widths_by_span;
  delete oldColorOutput.close_view_width_multiplier;
  delete oldColorOutput.maximum_mapped_width_mm;
  const merged = mergeSpecDefaults({ color_output: oldColorOutput });
  assert.equal(merged.color_output.scale_line_widths_by_span, true);
  assert.equal(merged.color_output.close_view_width_multiplier, 2);
  assert.equal(merged.color_output.maximum_mapped_width_mm, 4);
});

test("keeps the mapped-width cap above active route and railway floors", () => {
  const wideRoads = {
    ...initialSpec.color_output,
    road_width_mm: 4,
    maximum_mapped_width_mm: 1,
  };
  assert.equal(minimumMappedWidthCap(wideRoads), 5.6);
  assert.equal(normalizeMappedWidthCap(wideRoads).maximum_mapped_width_mm, 5.6);

  const recalled = mergeSpecDefaults({
    color_output: {
      ...initialSpec.color_output,
      road_width_mm: 4,
      maximum_mapped_width_mm: 1,
    },
  });
  assert.equal(recalled.color_output.maximum_mapped_width_mm, 5.6);

  const railwayOnly = {
    ...wideRoads,
    roads_enabled: false,
    rail_enabled: true,
    rail_width_mm: 3.2,
  };
  assert.equal(minimumMappedWidthCap(railwayOnly), 3.2);
});

test("defaults railways on in their own color and recalls old setups", () => {
  // Mirrors ColorOutputSpec::default in crates/toposaic-core/src/spec.rs.
  assert.equal(initialSpec.color_output.rail_enabled, true);
  assert.equal(initialSpec.color_output.rail_color, "#C43D3D");
  assert.equal(initialSpec.color_output.rail_width_mm, 0.7);
  // Picked out in their own color, which is the point of drawing them. The
  // slot is only spent where the mapped data holds railways.
  assert.equal(initialSpec.color_output.rail_style, "separate");
  // Setups saved before the railway layer existed recall with the same
  // defaults the backend applies to them.
  const oldColorOutput = { ...initialSpec.color_output };
  delete oldColorOutput.rail_enabled;
  delete oldColorOutput.rail_color;
  delete oldColorOutput.rail_width_mm;
  delete oldColorOutput.rail_style;
  const merged = mergeSpecDefaults({ color_output: oldColorOutput });
  assert.equal(merged.color_output.rail_enabled, true);
  assert.equal(merged.color_output.rail_style, "separate");
  assert.equal(merged.color_output.rail_color, "#C43D3D");
  // A setup that folded railways into the roads keeps that choice.
  const mergedRail = mergeSpecDefaults({
    color_output: {
      ...initialSpec.color_output,
      rail_enabled: false,
      rail_style: "with_roads",
    },
  });
  assert.equal(mergedRail.color_output.rail_enabled, false);
  assert.equal(mergedRail.color_output.rail_style, "with_roads");
});

test("defaults aerial lifts on in their own color, in service only", () => {
  // Mirrors ColorOutputSpec::default in crates/toposaic-core/src/spec.rs.
  assert.equal(initialSpec.color_output.aerial_enabled, true);
  assert.equal(initialSpec.color_output.aerial_color, "#6C4CB6");
  assert.equal(initialSpec.color_output.aerial_width_mm, 0.7);
  // A chair lift is neither a road nor a railway, so it says so.
  assert.equal(initialSpec.color_output.aerial_style, "separate");
  assert.equal(initialSpec.color_output.rail_lifecycle, "operational");
  // Setups saved before the split recall with lifts in their own color and
  // running lines only.
  const oldColorOutput = { ...initialSpec.color_output };
  for (const field of [
    "rail_lifecycle",
    "aerial_enabled",
    "aerial_color",
    "aerial_width_mm",
    "aerial_style",
  ]) {
    delete oldColorOutput[field];
  }
  const merged = mergeSpecDefaults({ color_output: oldColorOutput });
  assert.equal(merged.color_output.aerial_style, "separate");
  assert.equal(merged.color_output.aerial_enabled, true);
  assert.equal(merged.color_output.rail_lifecycle, "operational");
  // A setup that folded lifts into the railways and asked for abandoned
  // formations keeps both choices.
  const folded = mergeSpecDefaults({
    color_output: {
      ...initialSpec.color_output,
      aerial_style: "with_rail",
      rail_lifecycle: "abandoned",
    },
  });
  assert.equal(folded.color_output.aerial_style, "with_rail");
  assert.equal(folded.color_output.rail_lifecycle, "abandoned");
});

test("resolves which class each rail-family layer paints in", () => {
  // Mirrors rail_line_style and aerial_line_style in
  // crates/toposaic-core/src/spec.rs.
  const resolve = (overrides) => {
    const colorOutput = { ...initialSpec.color_output, ...overrides };
    return [railLineClass(colorOutput), aerialLineClass(colorOutput)];
  };

  // Railway styling answers "how would railways look", so it ignores the
  // railway toggle; only the lift chain consults it.
  assert.deepEqual(resolve({ rail_style: "with_roads" })[0], "road");
  assert.deepEqual(resolve({ rail_style: "separate" })[0], "rail");
  assert.deepEqual(
    resolve({ rail_enabled: false, rail_style: "separate" })[0],
    "rail",
  );

  // Lifts following railways land wherever railways land.
  assert.equal(
    resolve({ aerial_style: "with_rail", rail_style: "with_roads" })[1],
    "road",
  );
  assert.equal(
    resolve({ aerial_style: "with_rail", rail_style: "separate" })[1],
    "rail",
  );
  // With railways off there is no railway style to follow, so the chain
  // falls through to roads rather than drawing nothing or borrowing a
  // rail color the model never emits.
  assert.equal(
    resolve({
      aerial_style: "with_rail",
      rail_style: "separate",
      rail_enabled: false,
    })[1],
    "road",
  );
  // The other two styles ignore railways entirely.
  assert.equal(
    resolve({ aerial_style: "separate", rail_style: "with_roads" })[1],
    "aerialway",
  );
  assert.equal(
    resolve({ aerial_style: "with_roads", rail_style: "separate" })[1],
    "road",
  );
});

test("defaults imported trails to none and recalls old setups cleanly", () => {
  assert.deepEqual(initialSpec.trails, []);
  assert.equal(initialSpec.color_output.trail_color, "#D6336C");
  assert.equal(initialSpec.color_output.trail_width_mm, 0.7);
  // Setups saved before trails existed recall with no trails and the new
  // defaults filled in.
  const oldSpec = { ...initialSpec };
  delete oldSpec.trails;
  const merged = mergeSpecDefaults(oldSpec);
  assert.deepEqual(merged.trails, []);
  // Setups saved with trails keep them.
  const withTrail = mergeSpecDefaults({
    trails: [
      {
        name: "Loop",
        points: [
          [46.8, -121.7],
          [46.9, -121.6],
        ],
      },
    ],
  });
  assert.equal(withTrail.trails.length, 1);
  assert.equal(withTrail.trails[0].name, "Loop");
});

test("mapped trails follow the saved route color until split", () => {
  assert.equal(
    initialSpec.color_output.route_trail_color,
    initialSpec.color_output.road_color,
  );
  const oldColorOutput = { ...initialSpec.color_output, road_color: "#123456" };
  delete oldColorOutput.route_trail_color;
  const merged = mergeSpecDefaults({ color_output: oldColorOutput });
  assert.equal(merged.color_output.route_trail_color, "#123456");

  const split = mergeSpecDefaults({
    color_output: { ...oldColorOutput, route_trail_color: "#654321" },
  });
  assert.equal(split.color_output.route_trail_color, "#654321");
});

test("old setups gain a stable puzzle identity without changing their old cuts", () => {
  const oldSpec = { ...initialSpec };
  delete oldSpec.puzzle_seed;
  delete oldSpec.puzzle_tile_column;
  delete oldSpec.puzzle_tile_row;
  delete oldSpec.outer_edge_interlocks;
  const merged = mergeSpecDefaults(oldSpec);
  assert.equal(merged.puzzle_seed, 0);
  assert.equal(merged.puzzle_tile_column, 0);
  assert.equal(merged.puzzle_tile_row, 0);
  assert.equal(merged.outer_edge_interlocks, false);
});

test("recalls old setups with tray contours, retention, and wall hardware defaults", () => {
  const oldSpec = structuredClone(initialSpec);
  delete oldSpec.tray.contours_enabled;
  delete oldSpec.tray.label_font;
  delete oldSpec.tray.label_height_mm;
  delete oldSpec.tray.label_position;
  delete oldSpec.puzzle_retention;
  delete oldSpec.wall_mount;
  const merged = mergeSpecDefaults(oldSpec);
  assert.equal(merged.tray.contours_enabled, true);
  assert.equal(merged.tray.label_font, "atkinson_hyperlegible");
  assert.equal(merged.tray.label_height_mm, 4);
  assert.equal(merged.tray.label_position, "center");
  assert.deepEqual(merged.puzzle_retention, {
    enabled: false,
    pin_diameter_mm: 3,
    pin_height_mm: 1,
    clearance_mm: 0.2,
  });
  assert.deepEqual(merged.wall_mount, {
    style: "none",
    target: "terrain",
    vertical_position_ratio: 0.28,
    depth_mm: 1.6,
    thickness_mm: 1.2,
    wall_offset_mm: 0.8,
    pin_diameter_mm: 4,
    pin_count: 1,
    pin_spacing_mm: 32,
    cleat_width_mm: 12,
    export_hardware: true,
    fit_clearance_mm: 0.2,
    screw_hole_diameter_mm: 3.5,
    screw_countersink_depth_mm: 0.8,
    screw_head_clearance_mm: 0.4,
    wide_edge_screws: true,
  });
});

test("migrates an old wall pocket to full plate thickness", () => {
  const oldSpec = structuredClone(initialSpec);
  delete oldSpec.wall_mount.thickness_mm;
  oldSpec.wall_mount.depth_mm = 1.2;
  oldSpec.wall_mount.pocket_depth_mm = 0.6;
  oldSpec.wall_mount.wall_offset_mm = 1;

  const merged = mergeSpecDefaults(oldSpec);
  assert.equal(merged.wall_mount.thickness_mm, 1.6);
  assert.equal(merged.wall_mount.depth_mm, 1.2);
  assert.equal(merged.wall_mount.wall_offset_mm, 1);
  assert.equal("pocket_depth_mm" in merged.wall_mount, false);
});

test("derives mounting limits and wall hardware counts from the full model", () => {
  const puzzleGrid = structuredClone(initialSpec);
  puzzleGrid.adjacent_columns = 3;
  puzzleGrid.adjacent_rows = 2;
  assert.equal(wallHardwareQuantity(puzzleGrid), 6);

  puzzleGrid.wall_mount.target = "tray";
  assert.equal(wallHardwareQuantity(puzzleGrid), 6);
  puzzleGrid.adjacent_columns = 1;
  puzzleGrid.adjacent_rows = 1;
  puzzleGrid.tray.segment_columns = 2;
  puzzleGrid.tray.segment_rows = 3;
  assert.equal(wallHardwareQuantity(puzzleGrid), 6);

  puzzleGrid.wall_mount.target = "terrain";
  puzzleGrid.solid_model = true;
  assert.equal(wallHardwareQuantity(puzzleGrid), 1);
  assert.ok(Math.abs(maximumMountDepth(puzzleGrid) - 2.4) < 1e-9);
  assert.ok(Math.abs(maximumWallPlateThickness(puzzleGrid) - 2) < 1e-9);
  assert.ok(Math.abs(maximumRetentionHeight(puzzleGrid) - 2.6) < 1e-9);

  puzzleGrid.width_mm = 320;
  assert.equal(wallMountTargetWidth(puzzleGrid), 320);
  assert.equal(maximumCleatWidth(puzzleGrid), 316);
  puzzleGrid.solid_model = false;
  puzzleGrid.columns = 4;
  assert.equal(wallMountTargetWidth(puzzleGrid), 320);
  assert.equal(maximumCleatWidth(puzzleGrid), 316);

  puzzleGrid.wall_mount.target = "tray";
  puzzleGrid.tray.segment_columns = 1;
  puzzleGrid.tray.segment_rows = 2;
  assert.equal(wallMountTargetWidth(puzzleGrid), 320);
  puzzleGrid.tray.segment_columns = 2;
  assert.equal(wallMountTargetWidth(puzzleGrid), 160);
});

test("coalesces explicit null sample counts to the client defaults", () => {
  // The wire type is Option<u32>: the service and setup files can send
  // explicit nulls, which a plain spread would keep.
  const merged = mergeSpecDefaults({
    ...initialSpec,
    mesh_samples_across: null,
    overlay_samples_across: null,
  });
  assert.equal(merged.mesh_samples_across, initialSpec.mesh_samples_across);
  assert.equal(
    merged.overlay_samples_across,
    initialSpec.overlay_samples_across,
  );
});

test("treats null sample counts as unset in the label math", () => {
  // Specs straight off the wire skip mergeSpecDefaults, so the consumers
  // guard nulls themselves: no Math.max(null, …) = 0 in the preview label
  // and no division by zero in the ground spacing.
  const wireSpec = {
    ...initialSpec,
    mesh_samples_across: null,
    overlay_samples_across: null,
  };
  assert.equal(terrainSamplesAcross(wireSpec), 640);
  assert.equal(assembledMeshSamples(wireSpec), 640);
  assert.equal(groundMeshSpacing(wireSpec), 18000 / 640);
});

test("matches the backend's per-piece round-up in the assembled label", () => {
  // 2048 across 10 pieces rounds up to 205 per piece, so the assembled
  // model carries 2050 samples — the same number the backend reports.
  const ultra = {
    ...initialSpec,
    mesh_samples_across: 2048,
    overlay_samples_across: 2048,
  };
  assert.equal(assembledMeshSamples(ultra), 2050);
  // A solid model is a single piece, so nothing rounds.
  assert.equal(assembledMeshSamples({ ...ultra, solid_model: true }), 2048);
  // Even totals divide cleanly and stay put.
  assert.equal(assembledMeshSamples(initialSpec), 640);
});

test("counts trails alone toward the overlay sampling like the backend", () => {
  // uses_color_materials in crates/toposaic-core/src/spec.rs is true for
  // color output, buildings, OR imported trails, so a trails-only plain
  // model samples at the overlay density and the label must match.
  const trailsOnly = {
    ...initialSpec,
    mesh_samples_across: 384,
    overlay_samples_across: 1024,
    color_output: { ...initialSpec.color_output, enabled: false },
    buildings: { ...initialSpec.buildings, enabled: false },
    trails: [
      {
        name: "Loop",
        points: [
          [46.8, -121.7],
          [46.9, -121.6],
        ],
      },
    ],
  };
  // 1024 across 10 pieces rounds up to 103 per piece, 1030 assembled.
  assert.equal(effectiveMeshSamples(trailsOnly), 103);
  assert.equal(assembledMeshSamples(trailsOnly), 1030);
  // Without trails the plain model keeps its terrain sampling.
  const plain = { ...trailsOnly, trails: [] };
  assert.equal(effectiveMeshSamples(plain), 39);
  assert.equal(assembledMeshSamples(plain), 390);
});

test("formats cache sizes as B, KB, MB, and GB", () => {
  assert.equal(formatBytes(0), "0 B");
  assert.equal(formatBytes(512), "512 B");
  assert.equal(formatBytes(1023), "1023 B");
  assert.equal(formatBytes(1024), "1.0 KB");
  assert.equal(formatBytes(2048), "2.0 KB");
  assert.equal(formatBytes(1_048_576), "1.0 MB");
  assert.equal(formatBytes(52_428_800), "50 MB");
  assert.equal(formatBytes(63_965_184), "61 MB");
  assert.equal(formatBytes(3_650_722_202), "3.4 GB");
  // Nothing below zero: a bad reply still renders a sane size.
  assert.equal(formatBytes(-5), "0 B");
});

test("keeps east and west in the expected preview positions", () => {
  assert.ok(previewWorldX(1) < previewWorldX(0));
});

test("starts the preview camera south of the terrain", () => {
  const [, cameraY, cameraZ] = previewInitialCameraPosition(2, 0.4);
  assert.ok(cameraY > 0.4);
  assert.ok(cameraZ < 0);
});
