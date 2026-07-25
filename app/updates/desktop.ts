export type SignedUpdateProgress =
  | { phase: "downloading"; percent: number | null }
  | { phase: "installing"; percent: 100 };

export async function checkSignedUpdateVersion(): Promise<string | null> {
  const { check } = await import("@tauri-apps/plugin-updater");
  const update = await check({ timeout: 15_000 });
  if (!update) return null;
  try {
    return update.version;
  } finally {
    await update.close();
  }
}

export async function downloadAndInstallSignedUpdate(
  onProgress: (progress: SignedUpdateProgress) => void,
): Promise<string | null> {
  const [{ check }, { relaunch }] = await Promise.all([
    import("@tauri-apps/plugin-updater"),
    import("@tauri-apps/plugin-process"),
  ]);
  const update = await check({ timeout: 30_000 });
  if (!update) return null;

  let downloaded = 0;
  let total: number | undefined;
  await update.downloadAndInstall(
    (event) => {
      if (event.event === "Started") {
        total = event.data.contentLength;
        onProgress({ phase: "downloading", percent: total ? 0 : null });
      } else if (event.event === "Progress") {
        downloaded += event.data.chunkLength;
        onProgress({
          phase: "downloading",
          percent: total
            ? Math.min(99, Math.round((downloaded / total) * 100))
            : null,
        });
      } else {
        onProgress({ phase: "installing", percent: 100 });
      }
    },
    { timeout: 10 * 60_000 },
  );
  const version = update.version;
  await relaunch();
  return version;
}
