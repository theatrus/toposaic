import { expect, test } from "@playwright/test";

test("imports a GPX trail, shows its controls, and submits it in the spec", async ({
  page,
}) => {
  const gpx = `<?xml version="1.0" encoding="UTF-8"?>
<gpx xmlns="http://www.topografix.com/GPX/1/1" version="1.1" creator="test">
  <trk>
    <name>Skyline Loop</name>
    <trkseg>
      <trkpt lat="46.7852" lon="-121.7355"/>
      <trkpt lat="46.7871" lon="-121.7332"/>
      <trkpt lat="46.7893" lon="-121.7301"/>
    </trkseg>
  </trk>
</gpx>`;
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
          id: "trail-job",
          status: "running",
          progress: 10,
          artifacts: [],
          spec: jobSpec,
        },
      });
      return;
    }
    await route.abort();
  });

  await page.goto("/");
  await page.getByRole("tab", { name: "Surface" }).click();
  const trailsGroup = page.getByRole("group", { name: "Imported trails" });
  await expect(trailsGroup).toBeVisible();
  await expect(
    trailsGroup.getByRole("slider", { name: "Trail print width" }),
  ).toBeHidden();

  await trailsGroup.getByLabel("Import trail files").setInputFiles({
    name: "skyline.gpx",
    mimeType: "application/gpx+xml",
    buffer: Buffer.from(gpx, "utf8"),
  });
  await expect(trailsGroup.getByText("Skyline Loop")).toBeVisible();
  await expect(trailsGroup.getByText("3 points")).toBeVisible();
  await expect(trailsGroup.getByText("Imported 1 trail.")).toBeVisible();
  await expect(trailsGroup.getByLabel("Trail color")).toBeVisible();
  const width = trailsGroup.getByRole("slider", { name: "Trail print width" });
  await expect(width).toHaveValue("0.7");

  // The map draws the imported trail as a polyline overlay.
  await expect(page.locator(".map-trails polyline")).toHaveCount(1);
  // The 3D preview legend gains a Trail entry.
  await expect(
    page.getByLabel("Surface color legend").getByText("Trail", { exact: true }),
  ).toBeVisible();

  await page.getByRole("button", { name: /^Generate/ }).click();
  await expect
    .poll(() => (jobSpec.trails as unknown[] | undefined)?.length)
    .toBe(1);
  const submitted = jobSpec.trails as Array<{
    name: string;
    points: [number, number][];
  }>;
  expect(submitted[0].name).toBe("Skyline Loop");
  expect(submitted[0].points).toEqual([
    [46.7852, -121.7355],
    [46.7871, -121.7332],
    [46.7893, -121.7301],
  ]);
  expect(
    (jobSpec.color_output as Record<string, unknown>).trail_color,
  ).toBe("#D6336C");
  expect(
    (jobSpec.color_output as Record<string, unknown>).trail_width_mm,
  ).toBe(0.7);

  // Generating jumps to the Output tab; return to Surface to edit trails.
  await page.getByRole("tab", { name: "Surface" }).click();
  // Removing the trail clears the list and hides the trail controls.
  await trailsGroup
    .getByRole("button", { name: "Remove trail Skyline Loop" })
    .click();
  await expect(trailsGroup.getByText("Skyline Loop")).toBeHidden();
  await expect(width).toBeHidden();
  await expect(page.locator(".map-trails polyline")).toHaveCount(0);
});
