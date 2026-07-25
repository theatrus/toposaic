import assert from "node:assert/strict";
import test from "node:test";
import {
  previewInitialCameraPosition,
  previewWorldX,
} from "../app/terrain/preview-orientation.ts";
import { initialSpec, mergeSpecDefaults } from "../app/terrain/config.ts";
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

test("keeps east and west in the expected preview positions", () => {
  assert.ok(previewWorldX(1) < previewWorldX(0));
});

test("starts the preview camera south of the terrain", () => {
  const [, cameraY, cameraZ] = previewInitialCameraPosition(2, 0.4);
  assert.ok(cameraY > 0.4);
  assert.ok(cameraZ < 0);
});
