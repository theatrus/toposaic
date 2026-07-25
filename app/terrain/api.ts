import type {
  GenerationSpec,
  Job,
  PlaceResult,
  PreviewData,
  SavedSetup,
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

async function requestJson<T>(
  path: string,
  init?: RequestInit,
): Promise<T> {
  const response = await fetch(`${API_URL}${path}`, init);
  if (!response.ok) {
    let detail: string | null = null;
    try {
      const payload = (await response.json()) as ApiErrorPayload;
      if (typeof payload.error === "string") {
        detail = payload.error;
      } else if (typeof payload.message === "string") {
        detail = payload.message;
      }
    } catch (error) {
      if (error instanceof DOMException && error.name === "AbortError") {
        throw error;
      }
      // The body was not JSON, so fall back to the status.
    }
    throw new Error(detail ?? `TopoSaic service returned ${response.status}.`);
  }
  return (await response.json()) as T;
}

function jsonBody(value: unknown, signal?: AbortSignal): RequestInit {
  return {
    method: "POST",
    headers: { "content-type": "application/json" },
    body: JSON.stringify(value),
    signal,
  };
}

export const terrainApi = {
  preview(spec: GenerationSpec, signal?: AbortSignal) {
    return requestJson<PreviewData>("/api/preview", jsonBody(spec, signal));
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
  listSetups(signal?: AbortSignal) {
    return requestJson<SavedSetup[]>("/api/setups", { signal });
  },
  saveSetup(name: string, spec: GenerationSpec) {
    return requestJson<SavedSetup>("/api/setups", jsonBody({ name, spec }));
  },
  renameSetup(id: string, name: string, signal?: AbortSignal) {
    return requestJson<SavedSetup>(`/api/setups/${encodeURIComponent(id)}`, {
      method: "PATCH",
      headers: { "content-type": "application/json" },
      body: JSON.stringify({ name }),
      signal,
    });
  },
  async deleteSetup(id: string) {
    const response = await fetch(
      `${API_URL}/api/setups/${encodeURIComponent(id)}`,
      { method: "DELETE" },
    );
    if (!response.ok) {
      throw new Error(`TopoSaic service returned ${response.status}.`);
    }
  },
};
