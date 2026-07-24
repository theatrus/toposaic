import assert from "node:assert/strict";
import { mkdtemp, writeFile } from "node:fs/promises";
import os from "node:os";
import path from "node:path";
import test from "node:test";
import { buildReleaseFeeds } from "../scripts/release-feeds.mjs";

test("builds Tauri and website feeds from inline signatures", async () => {
  const directory = await mkdtemp(path.join(os.tmpdir(), "toposaic-feeds-"));
  const version = "1.2.3";
  const signatures = {
    "windows-x86_64": `TopoSaic-${version}-windows-x64.exe`,
    "linux-x86_64": `TopoSaic-${version}-linux-x86_64.AppImage`,
    "darwin-aarch64": `TopoSaic-${version}-macos-aarch64.app.tar.gz`,
  };
  for (const [target, fileName] of Object.entries(signatures)) {
    await writeFile(path.join(directory, `${fileName}.sig`), `sig-${target}\n`);
  }

  const feeds = await buildReleaseFeeds({
    artifactDirectory: directory,
    version,
    tag: `v${version}`,
    publishedAt: "2026-07-24T18:00:00Z",
    summary: "Terrain updates.",
  });

  assert.equal(feeds.updater.version, version);
  assert.equal(
    feeds.updater.platforms["darwin-aarch64"].signature,
    "sig-darwin-aarch64",
  );
  assert.equal(
    feeds.updater.platforms["windows-x86_64"].url,
    "https://github.com/theatrus/toposaic/releases/download/v1.2.3/TopoSaic-1.2.3-windows-x64.exe",
  );
  assert.deepEqual(feeds.notice, {
    schema_version: 1,
    version,
    release_url:
      "https://github.com/theatrus/toposaic/releases/tag/v1.2.3",
    summary: "Terrain updates.",
    urgency: "normal",
    minimum_supported_version: "0.1.0",
    published_at: "2026-07-24T18:00:00Z",
  });
});

test("rejects a tag that does not match the app version", async () => {
  await assert.rejects(
    buildReleaseFeeds({
      artifactDirectory: os.tmpdir(),
      version: "1.2.3",
      tag: "v1.2.4",
      publishedAt: "2026-07-24T18:00:00Z",
    }),
    /does not match/,
  );
});
