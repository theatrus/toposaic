import assert from "node:assert/strict";
import test from "node:test";

import { elapsedLabel } from "../app/terrain/time.ts";

test("formats short and long timing values without roll-over errors", () => {
  assert.equal(elapsedLabel(288.4), "288 ms");
  assert.equal(elapsedLabel(9_800), "10 s");
  assert.equal(elapsedLabel(60_500), "1m 1s");
  assert.equal(elapsedLabel(3_661_000), "1h 1m");
  assert.equal(elapsedLabel(-10), "0 ms");
});
