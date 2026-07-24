import { writeFile } from "node:fs/promises";

const outputPath = process.argv[2];
const publicKey = process.env.TOPOSAIC_UPDATER_PUBLIC_KEY?.trim();

if (!outputPath) {
  throw new Error("Usage: node scripts/write-updater-config.mjs OUTPUT_PATH");
}
if (!publicKey) {
  throw new Error("TOPOSAIC_UPDATER_PUBLIC_KEY is not set.");
}

const config = {
  bundle: {
    createUpdaterArtifacts: true,
  },
  plugins: {
    updater: {
      pubkey: publicKey,
      endpoints: [
        "https://toposaic.com/releases/updater.json",
        "https://github.com/theatrus/toposaic/releases/latest/download/updater.json",
      ],
      windows: {
        installMode: "passive",
      },
    },
  },
};

await writeFile(outputPath, `${JSON.stringify(config, null, 2)}\n`, {
  mode: 0o600,
});
