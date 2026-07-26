import { expect, test } from "@playwright/test";

test("turns Generate into Cancel while a job is active", async ({ page }) => {
  const jobId = "8b4165dc-9b47-4fa2-9f75-2ea36b9dff45";
  let cancelRequested = false;
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
          id: jobId,
          status: "running",
          progress: 24,
          artifacts: [],
          spec: jobSpec,
        },
      });
      return;
    }
    if (
      url.pathname === `/api/jobs/${jobId}` &&
      request.method() === "DELETE"
    ) {
      cancelRequested = true;
      await route.fulfill({
        json: {
          id: jobId,
          status: "canceled",
          progress: 24,
          artifacts: [],
          spec: jobSpec,
        },
      });
      return;
    }
    await route.abort();
  });

  await page.goto("/");
  await page.getByRole("button", { name: /^Generate/ }).click();

  const cancel = page.getByRole("button", { name: /^Cancel$/ });
  await expect(cancel).toBeVisible();
  await expect(cancel).toHaveClass(/cancel/);
  await expect(
    page.getByText("Sampling elevation and fetching source tiles…").first(),
  ).toBeVisible();
  const steps = page.getByRole("list", { name: "Generation progress" });
  await expect(steps).toContainText("Elevation");
  await expect(steps).toContainText("60%");
  await expect(page.locator(".job-progress output")).toHaveText("24%");
  await cancel.click();

  await expect(page.getByRole("button", { name: /^Generate/ })).toBeVisible();
  await expect(page.getByText("Generation canceled.").first()).toBeVisible();
  expect(cancelRequested).toBe(true);
});

test("shows the generated preview after a polled job completes", async ({
  page,
}) => {
  const jobId = "37c1f0aa-52d7-4f8e-9a41-6a0b0f5f7f21";
  let jobSpec: Record<string, unknown> = {};
  let statusReads = 0;

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
      statusReads += 1;
      const complete = statusReads > 1;
      await route.fulfill({
        json: {
          id: jobId,
          status: complete ? "complete" : "running",
          progress: complete ? 100 : 55,
          artifacts: complete
            ? [
                {
                  name: "terrain.3mf",
                  media_type: "model/3mf",
                  bytes: 1_048_576,
                },
              ]
            : [],
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
    await route.abort();
  });

  await page.goto("/");
  await page.getByRole("button", { name: /^Generate/ }).click();

  await expect(page.getByText("Generated terrain").first()).toBeVisible({
    timeout: 15_000,
  });
  await expect(
    page.getByRole("link", { name: /terrain\.3mf/ }),
  ).toBeVisible();
});

test("keeps direct artifact downloads in the web app", async ({ page }) => {
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
      await route.fulfill({
        json: {
          id: "e2ba221e-a689-4b59-9d5f-ae9b883596a1",
          status: "complete",
          progress: 100,
          artifacts: [
            {
              name: "terrain.3mf",
              media_type: "model/3mf",
              bytes: 1_048_576,
            },
            {
              name: "manifest.json",
              media_type: "application/json",
              bytes: 1024,
            },
            {
              name: "piece-01.stl",
              media_type: "model/stl",
              bytes: 2048,
            },
          ],
          spec: request.postDataJSON(),
        },
      });
      return;
    }
    if (
      url.pathname.endsWith("/downloads/terrain.3mf") &&
      request.method() === "GET"
    ) {
      await route.fulfill({
        body: "3mf data",
        headers: {
          "content-disposition": 'attachment; filename="terrain.3mf"',
          "content-type": "model/3mf",
        },
      });
      return;
    }
    await route.abort();
  });

  await page.goto("/");
  await page.getByRole("button", { name: /^Generate/ }).click();

  const model = page.getByRole("link", { name: /terrain\.3mf/ });
  await expect(model).toBeVisible();
  await expect(model).toHaveAttribute(
    "href",
    "http://127.0.0.1:8787/api/jobs/e2ba221e-a689-4b59-9d5f-ae9b883596a1/downloads/terrain.3mf",
  );
  const completedSteps = page.getByRole("list", {
    name: "Generation progress",
  });
  await expect(completedSteps).toContainText("Print files");
  await expect(completedSteps).toContainText("Ready");

  const download = page.waitForEvent("download");
  await model.click();
  await expect(model).toContainText("Sent to browser");
  expect((await download).suggestedFilename()).toBe("terrain.3mf");
  await expect(
    page.getByText("Sent terrain.3mf to your browser downloads."),
  ).toBeVisible();

  await page.getByText("STL models").click();
  await expect(
    page.getByRole("link", { name: /piece-01\.stl/ }),
  ).toHaveAttribute("href", /\/downloads\/piece-01\.stl$/);
});
