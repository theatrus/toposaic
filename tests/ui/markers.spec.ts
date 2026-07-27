import { expect, test } from "@playwright/test";

test("places map markers and submits their print modes", async ({ page }) => {
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
          id: "marker-job",
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
  await page.getByRole("tab", { name: "Markers" }).click();
  const controls = page.getByRole("group", { name: "Map markers" });
  const map = page.getByRole("application", { name: /Terrain map/ });

  await controls.getByRole("button", { name: "Highlight building" }).click();
  await map.click({ position: { x: 220, y: 150 } });
  await expect(page.locator(".map-marker.building")).toHaveCount(1);
  await controls.getByLabel("Marker 1 name").fill("Home");
  await controls.getByLabel("Marker 1 latitude").fill("46.900001");
  await controls.getByLabel("Marker 1 latitude").press("Enter");

  await controls.getByRole("button", { name: "Color dot" }).click();
  await map.click({ position: { x: 260, y: 180 } });
  await controls.getByRole("button", { name: "Flag hole" }).click();
  await map.click({ position: { x: 300, y: 210 } });

  await expect(page.locator(".map-marker")).toHaveCount(3);
  await expect(page.locator(".map-marker.dot")).toHaveCount(1);
  await expect(page.locator(".map-marker.flag_hole")).toHaveCount(1);
  await expect(controls.getByLabel("Marker color")).toHaveValue("#e24a33");
  await expect(
    controls.getByRole("checkbox", {
      name: "Export a printable flag blank with flag-hole jobs",
    }),
  ).toBeChecked();

  await page.getByRole("button", { name: /^Generate/ }).click();
  await expect.poll(() => (jobSpec.markers as unknown[] | undefined)?.length).toBe(3);
  const markers = jobSpec.markers as Array<{
    name: string;
    kind: string;
    latitude: number;
    longitude: number;
  }>;
  expect(markers.map((marker) => marker.kind)).toEqual([
    "building",
    "dot",
    "flag_hole",
  ]);
  expect(markers[0].name).toBe("Home");
  expect(markers[0].latitude).toBe(46.900001);
  expect(markers.every((marker) => Number.isFinite(marker.latitude))).toBe(true);
  expect(markers.every((marker) => Number.isFinite(marker.longitude))).toBe(true);
  expect((jobSpec.buildings as { enabled: boolean }).enabled).toBe(true);
  expect((jobSpec.marker_settings as { hole_diameter_mm: number }).hole_diameter_mm).toBe(2.4);
});
