import { expect, test } from "@playwright/test";

test("airport surfaces are off until asked for, then reach the submitted spec", async ({
  page,
}) => {
  let jobSpec: Record<string, unknown> | null = null;
  await page.route("http://127.0.0.1:8787/api/**", async (route) => {
    const url = new URL(route.request().url());
    const request = route.request();
    if (url.pathname === "/api/preview") {
      await route.fulfill({
        json: { width: 2, height: 2, values: [0, 0.3, 0.7, 1] },
      });
      return;
    }
    if (url.pathname === "/api/jobs" && request.method() === "POST") {
      jobSpec = request.postDataJSON();
      await route.fulfill({ json: { id: "job-1", status: "queued" } });
      return;
    }
    await route.abort();
  });
  await page.setViewportSize({ width: 1500, height: 1100 });
  await page.goto("/");
  await page.getByRole("tab", { name: "Surface" }).click();

  // Off to start, and nothing but the master switch on show.
  const master = page.getByRole("checkbox", {
    name: "Render airport surfaces",
  });
  await expect(master).not.toBeChecked();
  expect(
    await page.getByRole("checkbox", { name: "Runways and airstrips" }).count(),
  ).toBe(0);

  await master.check();
  for (const group of [
    "Runways and airstrips",
    "Taxiways and taxilanes",
    "Aprons",
    "Helipads",
  ]) {
    await expect(page.getByRole("checkbox", { name: group })).toBeChecked();
  }

  // Each group switches independently.
  await page.getByRole("checkbox", { name: "Aprons" }).uncheck();
  await expect(
    page.getByRole("checkbox", { name: "Runways and airstrips" }),
  ).toBeChecked();

  // The Colors tab carries the pavement color and its filament slot.
  await page.getByRole("tab", { name: "Colors" }).click();
  await expect(
    page.getByRole("textbox", { name: "Airport surface color" }),
  ).toHaveValue("#4A4E54");

  await page.getByRole("tab", { name: "Surface" }).click();
  await page
    .getByLabel("Airport surface style")
    .selectOption("follow_roads");

  await page.getByRole("button", { name: "Generate" }).click();
  await expect.poll(() => jobSpec !== null).toBe(true);

  const colorOutput = (
    jobSpec as unknown as { color_output: Record<string, unknown> }
  ).color_output;
  expect(colorOutput.aviation_enabled).toBe(true);
  expect(colorOutput.aviation_aprons_enabled).toBe(false);
  expect(colorOutput.aviation_runways_enabled).toBe(true);
  expect(colorOutput.aviation_style).toBe("follow_roads");
});

test("the small-feature cutoff says when it is dropping helipads", async ({
  page,
}) => {
  await page.route("http://127.0.0.1:8787/api/**", async (route) => {
    const url = new URL(route.request().url());
    if (url.pathname === "/api/preview") {
      await route.fulfill({
        json: { width: 2, height: 2, values: [0, 0.3, 0.7, 1] },
      });
      return;
    }
    await route.abort();
  });
  await page.setViewportSize({ width: 1500, height: 1100 });
  await page.goto("/");
  await page.getByRole("tab", { name: "Surface" }).click();
  await page.getByRole("checkbox", { name: "Render airport surfaces" }).check();

  // The default map spans 18 km, past the 12 km cutoff, so the panel should
  // already be saying helipads are being left out.
  await expect(page.getByText(/helipads are being\s+left out/)).toBeVisible();

  // Raising the cutoff past the span stops the warning.
  await page.getByRole("slider", { name: "Small feature cutoff" }).fill("40");
  expect(await page.getByText(/helipads are being\s+left out/).count()).toBe(0);
});
