import tauriConfig from "../src-tauri/tauri.conf.json";
import { isVersionNewer } from "./versioning";

export const APP_VERSION = tauriConfig.version;
export const LATEST_RELEASE_API_URL =
  "https://api.github.com/repos/theatrus/toposaic/releases/latest";

export type AvailableUpdate = {
  version: string;
  url: string;
};

export async function fetchAvailableUpdate(
  currentVersion: string,
  signal: AbortSignal,
): Promise<AvailableUpdate | null> {
  const response = await fetch(LATEST_RELEASE_API_URL, {
    cache: "no-store",
    signal,
  });
  if (!response.ok) return null;

  const release = (await response.json()) as {
    draft?: unknown;
    prerelease?: unknown;
    tag_name?: unknown;
    html_url?: unknown;
  };
  if (
    release.draft === true ||
    release.prerelease === true ||
    typeof release.tag_name !== "string" ||
    typeof release.html_url !== "string" ||
    !release.html_url.startsWith(
      "https://github.com/theatrus/toposaic/releases/",
    ) ||
    !isVersionNewer(release.tag_name, currentVersion)
  ) {
    return null;
  }

  return {
    version: release.tag_name.replace(/^v/, ""),
    url: release.html_url,
  };
}
