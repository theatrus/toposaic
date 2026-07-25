import { readFile, writeFile } from "node:fs/promises";

const outputPath = process.argv[2];
const publicKey = process.env.TOPOSAIC_UPDATER_PUBLIC_KEY?.trim();

if (!outputPath) {
  throw new Error("Usage: node scripts/write-updater-config.mjs OUTPUT_PATH");
}
if (!publicKey) {
  throw new Error("TOPOSAIC_UPDATER_PUBLIC_KEY is not set.");
}

// Endpoints and install mode come from the checked-in Tauri config so the
// signed release build cannot drift from it. Only the pubkey is overridden.
const { updater } = JSON.parse(
  await readFile(
    new URL("../src-tauri/tauri.conf.json", import.meta.url),
    "utf8",
  ),
).plugins;

const config = {
  bundle: {
    createUpdaterArtifacts: true,
  },
  plugins: {
    updater: {
      pubkey: publicKey,
      endpoints: updater.endpoints,
      windows: {
        installMode: updater.windows.installMode,
      },
    },
  },
};

await writeFile(outputPath, `${JSON.stringify(config, null, 2)}\n`, {
  mode: 0o600,
});
