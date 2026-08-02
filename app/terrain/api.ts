import type {
  CacheClearResult,
  CacheStats,
  GenerationSpec,
  Job,
  PlaceResult,
  PreviewData,
  SavedSetup,
  SetupVersion,
  SourceBundleSummary,
  SourceImportResult,
} from "./contracts";

export const IS_TAURI =
  typeof window !== "undefined" && "__TAURI_INTERNALS__" in window;

const configuredApiUrl =
  typeof process !== "undefined"
    ? process.env.NEXT_PUBLIC_TOPOSAIC_API_URL ??
      process.env.NEXT_PUBLIC_TERRAIN_API_URL
    : undefined;

export const API_URL = (
  IS_TAURI ? "http://127.0.0.1:38787" : configuredApiUrl ?? "http://127.0.0.1:8787"
).replace(/\/+$/, "");

type ApiErrorPayload = {
  error?: unknown;
  message?: unknown;
};

function rethrowAbort(error: unknown) {
  if (error instanceof DOMException && error.name === "AbortError") {
    throw error;
  }
}

/** The error detail from a failed response's body, if it carries one. */
async function errorDetail(response: Response): Promise<string | null> {
  let body: string;
  try {
    body = await response.text();
  } catch (error) {
    rethrowAbort(error);
    return null;
  }
  try {
    const payload = JSON.parse(body) as ApiErrorPayload;
    if (typeof payload.error === "string") return payload.error;
    if (typeof payload.message === "string") return payload.message;
  } catch {
    // The body was not JSON, so fall back to the status.
  }
  return null;
}

// The status matters to callers that treat 200 and 201 differently, such
// as saveSetup's created-versus-replaced report. Everyone else goes through
// requestJson below and keeps its plain-body shape.
async function requestJsonWithStatus<T>(
  path: string,
  init?: RequestInit,
): Promise<{ status: number; body: T }> {
  const response = await fetch(`${API_URL}${path}`, init);
  if (!response.ok) {
    const detail = await errorDetail(response);
    throw new Error(detail ?? `TopoSaic service returned ${response.status}.`);
  }
  try {
    return { status: response.status, body: (await response.json()) as T };
  } catch (error) {
    rethrowAbort(error);
    // A 200 with an unreadable body: keep the friendly message instead of
    // leaking a raw SyntaxError to the interface.
    throw new Error(
      `TopoSaic service returned ${response.status}, but the reply was unreadable.`,
    );
  }
}

async function requestJson<T>(
  path: string,
  init?: RequestInit,
): Promise<T> {
  return (await requestJsonWithStatus<T>(path, init)).body;
}

function jsonBody(value: unknown, signal?: AbortSignal): RequestInit {
  return {
    method: "POST",
    headers: { "content-type": "application/json" },
    body: JSON.stringify(value),
    signal,
  };
}

export type PreviewProgress = {
  stage: "elevation" | "surface" | "model";
  label: string;
  progress: number;
};

export type PreviewDetail = "fast" | "detailed" | "high";
export type PreviewTile = { row: number; column: number };

type PreviewStreamEvent =
  | ({ type: "progress" } & PreviewProgress)
  | { type: "complete"; preview: PreviewData }
  | { type: "error"; error: string }
  | { type: "canceled" };

async function previewRequest(
  spec: GenerationSpec,
  signal?: AbortSignal,
  onProgress?: (progress: PreviewProgress) => void,
  detail: PreviewDetail = "fast",
  requestId?: string,
  tile?: PreviewTile,
) {
  const response = await fetch(`${API_URL}/api/preview`, {
    method: "POST",
    headers: {
      accept: "application/x-ndjson",
      "content-type": "application/json",
      "x-toposaic-preview-detail": detail,
      ...(requestId ? { "x-toposaic-preview-id": requestId } : {}),
      ...(tile
        ? {
            "x-toposaic-preview-tile-row": String(tile.row),
            "x-toposaic-preview-tile-column": String(tile.column),
          }
        : {}),
    },
    body: JSON.stringify(spec),
    signal,
  });
  if (!response.ok) {
    const detail = await errorDetail(response);
    throw new Error(detail ?? `TopoSaic service returned ${response.status}.`);
  }
  if (!response.headers.get("content-type")?.includes("application/x-ndjson")) {
    try {
      return (await response.json()) as PreviewData;
    } catch (error) {
      rethrowAbort(error);
      throw new Error(
        `TopoSaic service returned ${response.status}, but the reply was unreadable.`,
      );
    }
  }
  if (!response.body) {
    throw new Error("TopoSaic service returned an empty preview stream.");
  }

  const reader = response.body.getReader();
  const decoder = new TextDecoder();
  let buffered = "";
  let completed: PreviewData | null = null;
  const readEvent = (line: string) => {
    const event = JSON.parse(line) as PreviewStreamEvent;
    if (event.type === "progress") {
      onProgress?.({
        stage: event.stage,
        label: event.label,
        progress: Math.max(0, Math.min(100, event.progress)),
      });
    } else if (event.type === "complete") {
      completed = event.preview;
    } else if (event.type === "error") {
      throw new Error(event.error);
    } else if (event.type === "canceled") {
      throw new DOMException("Preview replaced by newer settings.", "AbortError");
    }
  };

  while (true) {
    const { done, value } = await reader.read();
    buffered += decoder.decode(value, { stream: !done });
    let newline = buffered.indexOf("\n");
    while (newline >= 0) {
      const line = buffered.slice(0, newline).trim();
      buffered = buffered.slice(newline + 1);
      if (line) readEvent(line);
      newline = buffered.indexOf("\n");
    }
    if (done) break;
  }
  if (buffered.trim()) readEvent(buffered.trim());
  if (!completed) {
    throw new Error("TopoSaic preview ended before the model was ready.");
  }
  return completed;
}

export const terrainApi = {
  preview(
    spec: GenerationSpec,
    signal?: AbortSignal,
    onProgress?: (progress: PreviewProgress) => void,
    detail: PreviewDetail = "fast",
    requestId?: string,
    tile?: PreviewTile,
  ) {
    return previewRequest(spec, signal, onProgress, detail, requestId, tile);
  },
  cancelPreview(requestId: string) {
    return requestJson<{ canceled: boolean }>(
      `/api/preview/${encodeURIComponent(requestId)}`,
      { method: "DELETE" },
    );
  },
  searchPlaces(query: string) {
    return requestJson<PlaceResult[]>(
      `/api/places?q=${encodeURIComponent(query)}`,
    );
  },
  createJob(spec: GenerationSpec) {
    return requestJson<Job>("/api/jobs", jsonBody(spec));
  },
  getJob(id: string, signal?: AbortSignal) {
    return requestJson<Job>(`/api/jobs/${encodeURIComponent(id)}`, { signal });
  },
  cancelJob(id: string) {
    return requestJson<Job>(`/api/jobs/${encodeURIComponent(id)}`, {
      method: "DELETE",
    });
  },
  generatedPreview(id: string, signal?: AbortSignal) {
    return requestJson<PreviewData>(
      `/api/jobs/${encodeURIComponent(id)}/downloads/preview.json`,
      { signal },
    );
  },
  artifactUrl(id: string, name: string) {
    return `${API_URL}/api/jobs/${encodeURIComponent(id)}/downloads/${encodeURIComponent(name)}`;
  },
  sourceBundle(id: string, signal?: AbortSignal) {
    return requestJson<SourceBundleSummary>(
      `/api/jobs/${encodeURIComponent(id)}/sources`,
      { signal },
    );
  },
  // Builds the bundle into the job's directory, where it becomes one of the
  // job's files — so the ordinary download and save paths carry it.
  buildSourceBundle(id: string, signal?: AbortSignal) {
    return requestJson<{ name: string; bytes: number }>(
      `/api/jobs/${encodeURIComponent(id)}/sources/build`,
      { method: "POST", signal },
    );
  },
  // Sent as raw bytes rather than multipart: the service streams the body
  // straight to a file, and a bundle can run to hundreds of megabytes.
  importSourceBundle(file: Blob, signal?: AbortSignal) {
    return requestJson<SourceImportResult>("/api/sources/import", {
      method: "POST",
      headers: { "content-type": "application/zip" },
      body: file,
      signal,
    });
  },
  listSetups(signal?: AbortSignal) {
    return requestJson<SavedSetup[]>("/api/setups", { signal });
  },
  // The service answers 201 for a new setup and 200 for an overwrite; the
  // studio words its status line off that difference.
  async saveSetup(
    name: string,
    spec: GenerationSpec,
  ): Promise<{ setup: SavedSetup; created: boolean }> {
    const { status, body } = await requestJsonWithStatus<SavedSetup>(
      "/api/setups",
      jsonBody({ name, spec }),
    );
    return { setup: body, created: status === 201 };
  },
  listSetupVersions(id: string, signal?: AbortSignal) {
    return requestJson<SetupVersion[]>(
      `/api/setups/${encodeURIComponent(id)}/versions`,
      { signal },
    );
  },
  restoreSetupVersion(id: string, versionId: string, signal?: AbortSignal) {
    return requestJson<SavedSetup>(
      `/api/setups/${encodeURIComponent(id)}/versions/${encodeURIComponent(versionId)}/restore`,
      { method: "POST", signal },
    );
  },
  renameSetup(id: string, name: string, signal?: AbortSignal) {
    return requestJson<SavedSetup>(`/api/setups/${encodeURIComponent(id)}`, {
      method: "PATCH",
      headers: { "content-type": "application/json" },
      body: JSON.stringify({ name }),
      signal,
    });
  },
  cacheStats(signal?: AbortSignal) {
    return requestJson<CacheStats>("/api/cache", { signal });
  },
  clearCache(olderThanDays: number | null) {
    return requestJson<CacheClearResult>(
      "/api/cache/clear",
      jsonBody({ older_than_days: olderThanDays }),
    );
  },
  // Separate from requestJson because a successful delete has no body.
  async deleteSetup(id: string, signal?: AbortSignal) {
    const response = await fetch(
      `${API_URL}/api/setups/${encodeURIComponent(id)}`,
      { method: "DELETE", signal },
    );
    if (!response.ok) {
      const detail = await errorDetail(response);
      throw new Error(
        detail ?? `TopoSaic service returned ${response.status}.`,
      );
    }
  },
};
