import { readFile, writeFile } from "node:fs/promises";
import path from "node:path";
import { pathToFileURL } from "node:url";

export const NOTICE_SCHEMA_VERSION = 1;

const platformAssets = (version) => ({
  "windows-x86_64": `TopoSaic-${version}-windows-x64.exe`,
  "linux-x86_64": `TopoSaic-${version}-linux-x86_64.AppImage`,
  "darwin-aarch64": `TopoSaic-${version}-macos-aarch64.app.tar.gz`,
});

function validVersion(version) {
  return /^\d+\.\d+\.\d+(?:-[0-9A-Za-z.-]+)?$/.test(version);
}

function releaseAssetUrl(repository, tag, fileName) {
  return `https://github.com/${repository}/releases/download/${encodeURIComponent(
    tag,
  )}/${encodeURIComponent(fileName)}`;
}

export async function buildReleaseFeeds({
  artifactDirectory,
  version,
  tag,
  repository = "theatrus/toposaic",
  publishedAt,
  summary = "A new TopoSaic release is ready.",
  minimumSupportedVersion = "0.1.0",
}) {
  if (!validVersion(version)) {
    throw new Error(`Invalid release version: ${version}`);
  }
  if (tag !== `v${version}`) {
    throw new Error(`Release tag ${tag} does not match v${version}.`);
  }
  if (Number.isNaN(Date.parse(publishedAt))) {
    throw new Error(`Invalid release date: ${publishedAt}`);
  }

  const platforms = {};
  for (const [target, fileName] of Object.entries(platformAssets(version))) {
    const signature = (
      await readFile(path.join(artifactDirectory, `${fileName}.sig`), "utf8")
    ).trim();
    if (!signature) {
      throw new Error(`The updater signature for ${fileName} is empty.`);
    }
    platforms[target] = {
      signature,
      url: releaseAssetUrl(repository, tag, fileName),
    };
  }

  const releaseUrl = `https://github.com/${repository}/releases/tag/${encodeURIComponent(
    tag,
  )}`;
  return {
    updater: {
      version,
      notes: summary,
      pub_date: publishedAt,
      platforms,
    },
    notice: {
      schema_version: NOTICE_SCHEMA_VERSION,
      version,
      release_url: releaseUrl,
      summary,
      urgency: "normal",
      minimum_supported_version: minimumSupportedVersion,
      published_at: publishedAt,
    },
  };
}

async function main() {
  const [artifactDirectory, version, tag, publishedAt] = process.argv.slice(2);
  if (!artifactDirectory || !version || !tag || !publishedAt) {
    throw new Error(
      "Usage: node scripts/release-feeds.mjs ARTIFACT_DIR VERSION TAG PUBLISHED_AT",
    );
  }
  const feeds = await buildReleaseFeeds({
    artifactDirectory,
    version,
    tag,
    publishedAt,
  });
  await Promise.all([
    writeFile(
      path.join(artifactDirectory, "updater.json"),
      `${JSON.stringify(feeds.updater, null, 2)}\n`,
    ),
    writeFile(
      path.join(artifactDirectory, "notice.json"),
      `${JSON.stringify(feeds.notice, null, 2)}\n`,
    ),
  ]);
}

if (import.meta.url === pathToFileURL(process.argv[1]).href) {
  await main();
}
