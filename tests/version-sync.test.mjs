import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import test from "node:test";

test("package.json, Cargo.toml, and tauri.conf.json agree on the version", async () => {
  const packageVersion = JSON.parse(
    await readFile(new URL("../package.json", import.meta.url), "utf8"),
  ).version;

  const tauriVersion = JSON.parse(
    await readFile(
      new URL("../src-tauri/tauri.conf.json", import.meta.url),
      "utf8",
    ),
  ).version;

  const cargoToml = await readFile(
    new URL("../Cargo.toml", import.meta.url),
    "utf8",
  );
  const workspacePackage = cargoToml.match(
    /\[workspace\.package\][^[]*?^version = "([^"]+)"/ms,
  );
  assert.ok(workspacePackage, "Cargo.toml has a [workspace.package] version");
  const cargoVersion = workspacePackage[1];

  assert.equal(cargoVersion, packageVersion);
  assert.equal(tauriVersion, packageVersion);
});
