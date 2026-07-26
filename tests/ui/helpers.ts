import type { Page } from "@playwright/test";
import { readFileSync } from "node:fs";

export const appVersion = JSON.parse(
  readFileSync(
    new URL("../../src-tauri/tauri.conf.json", import.meta.url),
    "utf8",
  ),
).version as string;
export const [appMajor, appMinor] = appVersion.split(".").map(Number);
export const newerVersion = `${appMajor}.${appMinor + 1}.0`;

export type StoredSetup = {
  id: string;
  name: string;
  created_at: string;
  updated_at: string;
  spec: Record<string, unknown>;
};

export type StoredCacheCategory = {
  key: "elevation" | "world_cover" | "osm" | "places";
  bytes: number;
  entries: number;
};

// 50 MB + 10 MB + 1 MB + 2 KB = 63,965,184 bytes in total.
export const defaultCacheCategories: StoredCacheCategory[] = [
  { key: "elevation", bytes: 52_428_800, entries: 120 },
  { key: "world_cover", bytes: 10_485_760, entries: 8 },
  { key: "osm", bytes: 1_048_576, entries: 30 },
  { key: "places", bytes: 2_048, entries: 5 },
];

// Serve a fake /api/setups store on both the web (8787) and desktop (38787)
// API ports, plus quiet /api/preview and /api/jobs endpoints so the studio
// settles and can finish a generation, and a /api/cache pair for the
// settings pane. Clearing by age drops the OSM category; clearing all
// empties every category.
export async function mockSetupsService(page: Page, setups: StoredSetup[]) {
  const state = {
    setups,
    saved: [] as Array<{ name: string; spec: Record<string, unknown> }>,
    renamed: [] as Array<{ id: string; name: string }>,
    cacheCategories: defaultCacheCategories.map((category) => ({
      ...category,
    })),
    cleared: [] as Array<number | null>,
  };
  let nextId = setups.length + 1;
  let jobSpec: Record<string, unknown> = {};
  const jobId = "saved-setup-job";
  const handler = async (route: import("@playwright/test").Route) => {
    const request = route.request();
    const url = new URL(request.url());
    if (url.pathname === "/api/preview") {
      await route.fulfill({
        json: { width: 2, height: 2, values: [0, 0.3, 0.7, 1] },
      });
      return;
    }
    if (url.pathname === "/api/jobs" && request.method() === "POST") {
      jobSpec = request.postDataJSON() as Record<string, unknown>;
      await route.fulfill({
        status: 202,
        json: {
          id: jobId,
          status: "queued",
          progress: 0,
          artifacts: [],
          spec: jobSpec,
        },
      });
      return;
    }
    if (url.pathname === `/api/jobs/${jobId}` && request.method() === "GET") {
      await route.fulfill({
        json: {
          id: jobId,
          status: "complete",
          progress: 100,
          artifacts: [],
          spec: jobSpec,
        },
      });
      return;
    }
    if (url.pathname === `/api/jobs/${jobId}/downloads/preview.json`) {
      await route.fulfill({
        json: { width: 2, height: 2, values: [0.2, 0.4, 0.6, 0.8] },
      });
      return;
    }
    if (url.pathname === "/api/cache" && request.method() === "GET") {
      await route.fulfill({
        json: {
          total_bytes: state.cacheCategories.reduce(
            (sum, category) => sum + category.bytes,
            0,
          ),
          categories: state.cacheCategories,
        },
      });
      return;
    }
    if (url.pathname === "/api/cache/clear" && request.method() === "POST") {
      const body = request.postDataJSON() as {
        older_than_days: number | null;
      };
      state.cleared.push(body.older_than_days);
      const removable =
        body.older_than_days === null
          ? state.cacheCategories
          : state.cacheCategories.filter((category) => category.key === "osm");
      const removed_bytes = removable.reduce(
        (sum, category) => sum + category.bytes,
        0,
      );
      const removed_entries = removable.reduce(
        (sum, category) => sum + category.entries,
        0,
      );
      for (const category of removable) {
        category.bytes = 0;
        category.entries = 0;
      }
      await route.fulfill({ json: { removed_bytes, removed_entries } });
      return;
    }
    if (url.pathname === "/api/setups" && request.method() === "GET") {
      await route.fulfill({ json: state.setups });
      return;
    }
    if (url.pathname === "/api/setups" && request.method() === "POST") {
      const body = request.postDataJSON() as {
        name: string;
        spec: Record<string, unknown>;
      };
      state.saved.push(body);
      const now = new Date().toISOString();
      let setup = state.setups.find((entry) => entry.name === body.name);
      // The real service answers 201 for a new setup and 200 for an
      // overwrite; the studio words its status line off the difference.
      let status = 200;
      if (setup) {
        setup.spec = body.spec;
        setup.updated_at = now;
      } else {
        setup = {
          id: `setup-${nextId++}`,
          name: body.name,
          created_at: now,
          updated_at: now,
          spec: body.spec,
        };
        state.setups = [setup, ...state.setups];
        status = 201;
      }
      await route.fulfill({ json: setup, status });
      return;
    }
    const setupMatch = url.pathname.match(/^\/api\/setups\/([^/]+)$/);
    if (setupMatch && request.method() === "DELETE") {
      state.setups = state.setups.filter(
        (entry) => entry.id !== decodeURIComponent(setupMatch[1]),
      );
      await route.fulfill({ status: 204, body: "" });
      return;
    }
    if (setupMatch && request.method() === "PATCH") {
      const id = decodeURIComponent(setupMatch[1]);
      const body = request.postDataJSON() as { name?: unknown };
      const name = typeof body.name === "string" ? body.name.trim() : "";
      const setup = state.setups.find((entry) => entry.id === id);
      if (!setup) {
        await route.fulfill({ status: 404, json: { error: "Unknown setup." } });
        return;
      }
      if (name === "") {
        await route.fulfill({
          status: 400,
          json: { error: "Setup names cannot be empty." },
        });
        return;
      }
      if (
        state.setups.some((entry) => entry.id !== id && entry.name === name)
      ) {
        await route.fulfill({
          status: 409,
          json: { error: `A setup named “${name}” already exists.` },
        });
        return;
      }
      state.renamed.push({ id, name });
      setup.name = name;
      setup.updated_at = new Date().toISOString();
      await route.fulfill({ json: setup });
      return;
    }
    await route.abort();
  };
  for (const port of [8787, 38787]) {
    await page.route(`http://127.0.0.1:${port}/api/**`, handler);
  }
  return state;
}
