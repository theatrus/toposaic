import { expect, test } from "@playwright/test";

const JOB_ID = "b52f1d34-3a6e-4c0f-9a17-2f4d8c1e7b90";

/** A finished job whose sources endpoint answers however the test wants. */
async function routeFinishedJob(
  page: import("@playwright/test").Page,
  sources: (built: boolean) => unknown,
  onBuild?: () => void,
) {
  let built = false;
  let jobSpec: Record<string, unknown> = {};
  await page.route("http://127.0.0.1:8787/api/**", async (route) => {
    const request = route.request();
    const url = new URL(request.url());
    if (url.pathname === "/api/preview") {
      await route.fulfill({
        json: { width: 2, height: 2, values: [0, 0.3, 0.7, 1] },
      });
      return;
    }
    if (url.pathname === "/api/jobs" && request.method() === "POST") {
      jobSpec = request.postDataJSON();
      await route.fulfill({
        status: 202,
        json: {
          id: JOB_ID,
          status: "queued",
          progress: 0,
          artifacts: [],
          spec: jobSpec,
        },
      });
      return;
    }
    if (url.pathname === `/api/jobs/${JOB_ID}` && request.method() === "GET") {
      await route.fulfill({
        json: {
          id: JOB_ID,
          status: "complete",
          progress: 100,
          artifacts: [
            { name: "terrain.3mf", media_type: "model/3mf", bytes: 1_048_576 },
          ],
          spec: jobSpec,
        },
      });
      return;
    }
    if (url.pathname === `/api/jobs/${JOB_ID}/downloads/preview.json`) {
      await route.fulfill({
        json: { width: 2, height: 2, values: [0.2, 0.4, 0.6, 0.8] },
      });
      return;
    }
    if (url.pathname === `/api/jobs/${JOB_ID}/sources/build`) {
      built = true;
      onBuild?.();
      await route.fulfill({
        json: { name: "toposaic-sources.zip", bytes: 68_127_427 },
      });
      return;
    }
    if (url.pathname === `/api/jobs/${JOB_ID}/sources`) {
      await route.fulfill({ json: sources(built) });
      return;
    }
    await route.abort();
  });
}

async function generateAndOpenOutput(page: import("@playwright/test").Page) {
  await page.goto("/");
  await page.getByRole("button", { name: /^Generate/ }).click();
  await page.getByRole("tab", { name: "Output" }).click();
}

test("packs the source data and then offers it as a file", async ({ page }) => {
  let buildRequests = 0;
  await routeFinishedJob(
    page,
    (built) => ({
      available: true,
      files: 32,
      bytes: 68_113_034,
      name: "toposaic-sources.zip",
      built_bytes: built ? 68_127_427 : null,
    }),
    () => {
      buildRequests += 1;
    },
  );
  await generateAndOpenOutput(page);

  const section = page.locator("details").filter({ hasText: "Source data" });
  await expect(section).toBeVisible();
  await section.locator("summary").click();
  // The size is stated before anything is packed, so the choice is informed.
  await expect(section).toContainText("32 elevation, land-cover, and map");
  await expect(section).toContainText("65.0 MB");

  const pack = page.getByRole("button", { name: /Pack the source data/ });
  await expect(pack).toBeVisible();
  await pack.click();

  // Once packed it is an ordinary job file, download link and all.
  const download = page.getByRole("link", { name: /toposaic-sources\.zip/ });
  await expect(download).toBeVisible();
  await expect(download).toHaveAttribute(
    "href",
    /\/api\/jobs\/.+\/downloads\/toposaic-sources\.zip$/,
  );
  await expect(pack).toHaveCount(0);
  expect(buildRequests).toBe(1);
});

test("offers nothing for a job generated before source bundles existed", async ({
  page,
}) => {
  await routeFinishedJob(page, () => ({ available: false }));
  await generateAndOpenOutput(page);

  await expect(page.getByText("terrain.3mf")).toBeVisible();
  await expect(
    page.locator("details").filter({ hasText: "Source data" }),
  ).toHaveCount(0);
});

test("imports a bundle and loads the setup it carried", async ({ page }) => {
  await page.route("http://127.0.0.1:8787/api/**", async (route) => {
    const request = route.request();
    const url = new URL(request.url());
    if (url.pathname === "/api/preview") {
      await route.fulfill({
        json: { width: 2, height: 2, values: [0, 0.3, 0.7, 1] },
      });
      return;
    }
    if (url.pathname === "/api/sources/import") {
      await route.fulfill({
        json: {
          report: {
            place_name: "Grand Canyon",
            added: 32,
            added_bytes: 68_113_034,
            already_present: 2,
            rejected: 0,
          },
          spec: {
            center_lat: 36.0544,
            center_lon: -112.1401,
            place_name: "Grand Canyon",
          },
        },
      });
      return;
    }
    await route.abort();
  });

  await page.goto("/");
  await page.getByRole("button", { name: "Setups" }).click();
  await page
    .getByRole("menuitem", { name: "Import source data" })
    .click();
  await page
    .getByLabel("Import source data bundle")
    .setInputFiles({
      name: "grand-canyon-sources.zip",
      mimeType: "application/zip",
      buffer: Buffer.from("PK pretend archive"),
    });

  // The report says what landed, including what the cache already had, and
  // the setup that came with it is now the live one.
  await expect(
    page.getByText(/Loaded Grand Canyon: 32 source files added/),
  ).toBeVisible();
  await expect(page.getByText(/2 already cached/)).toBeVisible();
  await expect(page.getByLabel("Place name")).toHaveValue("Grand Canyon");
});
