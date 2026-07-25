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

// Serve a fake /api/setups store on both the web (8787) and desktop (38787)
// API ports, plus quiet /api/preview and /api/jobs endpoints so the studio
// settles and can finish a generation.
export async function mockSetupsService(page: Page, setups: StoredSetup[]) {
  const state = {
    setups,
    saved: [] as Array<{ name: string; spec: Record<string, unknown> }>,
    renamed: [] as Array<{ id: string; name: string }>,
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
      }
      await route.fulfill({ json: setup });
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
