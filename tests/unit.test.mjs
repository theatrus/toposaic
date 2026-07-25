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

test("keeps east and west in the expected preview positions", () => {
  assert.ok(previewWorldX(1) < previewWorldX(0));
});

test("starts the preview camera south of the terrain", () => {
  const [, cameraY, cameraZ] = previewInitialCameraPosition(2, 0.4);
  assert.ok(cameraY > 0.4);
  assert.ok(cameraZ < 0);
});
