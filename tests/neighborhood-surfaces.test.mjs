import assert from "node:assert/strict";
import test from "node:test";

import { initialSpec, mergeSpecDefaults } from "../app/terrain/config.ts";
import { filamentSlotEntries } from "../app/terrain/filament-slots.ts";
import { previewGroundColor } from "../app/terrain/preview-ground-color.ts";

test("old and partial saved setups receive opt-in neighborhood defaults", () => {
  assert.equal(initialSpec.color_output.neighborhood_surfaces.enabled, false);
  const old = mergeSpecDefaults({ color_output: { rock_color: "#123456" } });
  assert.equal(old.color_output.neighborhood_surfaces.enabled, false);
  assert.equal(old.color_output.rock_color, "#123456");
  const partial = mergeSpecDefaults({ color_output: {
    neighborhood_surfaces: { enabled: true, sport_color: "#112233" },
  } });
  assert.equal(partial.color_output.neighborhood_surfaces.enabled, true);
  assert.equal(partial.color_output.neighborhood_surfaces.sport_color, "#112233");
  assert.equal(partial.color_output.neighborhood_surfaces.grass_color, "#76A542");
  assert.equal(initialSpec.color_output.neighborhood_surfaces.sport_color, "#2F76B5");
});

test("neighborhood surfaces share matching filament slots and disappear when disabled", () => {
  const spec = structuredClone(initialSpec);
  const classes = ["grass", "pavement", "sport", "playground"];
  const surfaces = () => filamentSlotEntries(spec).filter((entry) => classes.includes(entry.classKey));
  assert.deepEqual(surfaces(), []);
  spec.color_output.neighborhood_surfaces.enabled = true;
  assert.deepEqual(surfaces().map((entry) => entry.classKey), classes);
  spec.color_output.neighborhood_surfaces.pavement_color = spec.color_output.road_color.toLowerCase();
  const entries = filamentSlotEntries(spec);
  assert.equal(
    entries.find((entry) => entry.classKey === "pavement").filament,
    entries.find((entry) => entry.classKey === "road").filament,
  );
  assert.equal(
    entries.find((entry) => entry.classKey === "sport").filament,
    entries.find((entry) => entry.classKey === "water").filament,
  );
  spec.color_output.neighborhood_surfaces.enabled = false;
  assert.deepEqual(surfaces(), []);
  spec.color_output.neighborhood_surfaces.enabled = true;
  spec.color_output.enabled = false;
  assert.deepEqual(surfaces(), []);
});

test("saved satellite previews keep their old material offset", () => {
  const old = { ground_palette: ["#112233", "#445566"] };
  assert.equal(previewGroundColor(old, 13), "#112233");
  assert.equal(previewGroundColor(old, 14), "#445566");
  const current = { ...old, ground_material_offset: 17 };
  assert.equal(previewGroundColor(current, 13), undefined);
  assert.equal(previewGroundColor(current, 17), "#112233");
  assert.equal(previewGroundColor(current, 18), "#445566");
  assert.equal(previewGroundColor(current, 25), undefined);
  assert.equal(previewGroundColor(null, 17), undefined);
  assert.equal(previewGroundColor(current), undefined);
});
