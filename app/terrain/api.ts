import type {
  GenerationSpec,
  Job,
  PlaceResult,
  PreviewData,
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
};

async function requestJson<T>(
  path: string,
  init?: RequestInit,
): Promise<T> {
  const response = await fetch(`${API_URL}${path}`, init);
  const payload = (await response.json().catch(() => null)) as
    | ApiErrorPayload
    | T
    | null;
  if (!response.ok) {
    const message =
      payload &&
      typeof payload === "object" &&
      "error" in payload &&
      typeof payload.error === "string"
        ? payload.error
        : `TopoSaic service returned ${response.status}.`;
    throw new Error(message);
  }
  if (payload === null) {
    throw new Error("TopoSaic service returned an empty response.");
  }
  return payload as T;
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
  getJob(id: string) {
    return requestJson<Job>(`/api/jobs/${encodeURIComponent(id)}`);
  },
  cancelJob(id: string) {
    return requestJson<Job>(`/api/jobs/${encodeURIComponent(id)}`, {
      method: "DELETE",
    });
  },
  generatedPreview(id: string) {
    return requestJson<PreviewData>(
      `/api/jobs/${encodeURIComponent(id)}/downloads/preview.json`,
    );
  },
  artifactUrl(id: string, name: string) {
    return `${API_URL}/api/jobs/${encodeURIComponent(id)}/downloads/${encodeURIComponent(name)}`;
  },
};
