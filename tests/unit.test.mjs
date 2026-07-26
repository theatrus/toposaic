import assert from "node:assert/strict";
import test from "node:test";
import {
  previewInitialCameraPosition,
  previewWorldX,
} from "../app/terrain/preview-orientation.ts";
import {
  assembledMeshSamples,
  groundMeshSpacing,
  initialSpec,
  mergeSpecDefaults,
  terrainSamplesAcross,
} from "../app/terrain/config.ts";
import { isVersionNewer } from "../app/updates/version.ts";

test("compares stable and prerelease app versions", () => {
  assert.equal(isVersionNewer("v0.2.0", "0.1.9"), true);
  assert.equal(isVersionNewer("v0.1.10", "0.1.9"), true);
  assert.equal(isVersionNewer("v0.1.0", "0.1.0-beta.2"), true);
  assert.equal(isVersionNewer("v0.1.0-beta.2", "0.1.0"), false);
  assert.equal(isVersionNewer("not-a-version", "0.1.0"), false);
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
    trails: [{ name: "Loop", points: [[46.8, -121.7], [46.9, -121.6]] }],
  });
  assert.equal(withTrail.trails.length, 1);
  assert.equal(withTrail.trails[0].name, "Loop");
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
  assert.equal(
    assembledMeshSamples({ ...ultra, solid_model: true }),
    2048,
  );
  // Even totals divide cleanly and stay put.
  assert.equal(assembledMeshSamples(initialSpec), 640);
});

test("keeps east and west in the expected preview positions", () => {
  assert.ok(previewWorldX(1) < previewWorldX(0));
});

test("starts the preview camera south of the terrain", () => {
  const [, cameraY, cameraZ] = previewInitialCameraPosition(2, 0.4);
  assert.ok(cameraY > 0.4);
  assert.ok(cameraZ < 0);
});
