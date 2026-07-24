import tauriConfig from "../src-tauri/tauri.conf.json";
import { isVersionNewer } from "./versioning";

export const APP_VERSION = tauriConfig.version;
export const GITHUB_RELEASE_API_URL =
  "https://api.github.com/repos/theatrus/toposaic/releases/latest";
export const WEBSITE_NOTICE_URL =
  "https://toposaic.com/releases/notice.json";
export const RELEASES_URL =
  "https://github.com/theatrus/toposaic/releases/latest";

export type UpdateUrgency = "normal" | "recommended" | "required";

export type AvailableUpdate = {
  version: string;
  url: string;
  summary?: string;
  urgency: UpdateUrgency;
  source: "github" | "website" | "signed";
};

type WebsiteNotice = {
  schema_version?: unknown;
  version?: unknown;
  release_url?: unknown;
  summary?: unknown;
  urgency?: unknown;
  minimum_supported_version?: unknown;
  published_at?: unknown;
};

function normalizedVersion(version: string) {
  return version.trim().replace(/^v/, "");
}

function safeReleaseUrl(value: unknown): value is string {
  if (typeof value !== "string") return false;
  try {
    const url = new URL(value);
    return (
      url.protocol === "https:" &&
      (url.hostname === "toposaic.com" ||
        (url.hostname === "github.com" &&
          url.pathname.startsWith("/theatrus/toposaic/releases/")))
    );
  } catch {
    return false;
  }
}

export function parseGithubRelease(
  value: unknown,
  currentVersion: string,
): AvailableUpdate | null {
  const release = value as {
    draft?: unknown;
    prerelease?: unknown;
    tag_name?: unknown;
    html_url?: unknown;
  };
  if (
    !release ||
    release.draft === true ||
    release.prerelease === true ||
    typeof release.tag_name !== "string" ||
    !safeReleaseUrl(release.html_url) ||
    !isVersionNewer(release.tag_name, currentVersion)
  ) {
    return null;
  }

  return {
    version: normalizedVersion(release.tag_name),
    url: release.html_url,
    urgency: "normal",
    source: "github",
  };
}

export function parseWebsiteNotice(
  value: unknown,
  currentVersion: string,
): AvailableUpdate | null {
  const notice = value as WebsiteNotice;
  if (
    !notice ||
    notice.schema_version !== 1 ||
    typeof notice.version !== "string" ||
    !safeReleaseUrl(notice.release_url) ||
    !isVersionNewer(notice.version, currentVersion)
  ) {
    return null;
  }

  const configuredUrgency: UpdateUrgency =
    notice.urgency === "recommended" || notice.urgency === "required"
      ? notice.urgency
      : "normal";
  const minimumRequiresUpdate =
    typeof notice.minimum_supported_version === "string" &&
    isVersionNewer(notice.minimum_supported_version, currentVersion);
  const summary =
    typeof notice.summary === "string"
      ? notice.summary.trim().slice(0, 240)
      : undefined;

  return {
    version: normalizedVersion(notice.version),
    url: notice.release_url,
    summary: summary || undefined,
    urgency: minimumRequiresUpdate ? "required" : configuredUrgency,
    source: "website",
  };
}

export function selectNewestUpdate(
  candidates: Array<AvailableUpdate | null>,
): AvailableUpdate | null {
  let selected: AvailableUpdate | null = null;
  for (const candidate of candidates) {
    if (!candidate) continue;
    if (
      !selected ||
      isVersionNewer(candidate.version, selected.version) ||
      (normalizedVersion(candidate.version) ===
        normalizedVersion(selected.version) &&
        candidate.source === "website")
    ) {
      selected = candidate;
    }
  }
  return selected;
}

async function fetchJson(url: string, signal: AbortSignal) {
  const response = await fetch(url, {
    cache: "no-store",
    headers: { Accept: "application/json" },
    signal,
  });
  if (!response.ok) return null;
  return response.json() as Promise<unknown>;
}

export async function fetchAvailableUpdate(
  currentVersion: string,
  signal: AbortSignal,
): Promise<AvailableUpdate | null> {
  const results = await Promise.allSettled([
    fetchJson(GITHUB_RELEASE_API_URL, signal).then((release) =>
      parseGithubRelease(release, currentVersion),
    ),
    fetchJson(WEBSITE_NOTICE_URL, signal).then((notice) =>
      parseWebsiteNotice(notice, currentVersion),
    ),
  ]);

  return selectNewestUpdate(
    results.map((result) =>
      result.status === "fulfilled" ? result.value : null,
    ),
  );
}

export function signedUpdateFallback(version: string): AvailableUpdate {
  return {
    version: normalizedVersion(version),
    url: RELEASES_URL,
    urgency: "normal",
    source: "signed",
  };
}
