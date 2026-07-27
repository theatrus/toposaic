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
  await controls.getByRole("button", { name: "Blank flag" }).click();
  await map.click({ position: { x: 300, y: 210 } });
  await controls.getByRole("button", { name: "Named flag" }).click();
  await map.click({ position: { x: 180, y: 100 } });
  await controls.getByLabel("Marker 4 name").fill("富士山 Mount Fuji");
  await expect(page.locator(".map-marker-name")).toHaveText("富士山 Mount Fuji");
  await controls.getByRole("button", { name: "Surface label" }).click();
  await map.click({ position: { x: 340, y: 120 } });
  await controls.getByLabel("Marker 5 name").fill("North Fork");
  await controls.getByLabel("Marker 5 text height").fill("5.5");
  await controls.getByLabel("Marker 5 rotation").fill("35");
  await controls.getByLabel("Marker 5 relief").fill("0.7");
  await controls.getByRole("button", { name: "Raised plaque" }).click();
  await map.click({ position: { x: 380, y: 80 } });
  await controls.getByLabel("Marker 6 name").fill("Mirror Lake");
  await controls.getByLabel("Marker 6 rotation").fill("-20");
  await controls.getByLabel("Marker 6 relief").fill("1");
  await controls.getByLabel("Marker 6 plaque padding").fill("2.5");
  await controls.getByLabel("Marker 6 plaque base height").fill("1.4");

  await expect(page.locator(".map-marker")).toHaveCount(6);
  await expect(page.locator(".map-marker.dot")).toHaveCount(1);
  await expect(page.locator(".map-marker.flag_hole")).toHaveCount(1);
  await expect(page.locator(".map-marker.flag_label")).toHaveCount(1);
  await expect(page.locator(".map-marker.surface_label")).toHaveText(
    "North Fork",
  );
  await expect(page.locator(".map-marker.surface_label")).toHaveAttribute(
    "style",
    /--marker-rotation: 35deg/,
  );
  await expect(page.locator(".map-marker.plaque_label")).toHaveText(
    "Mirror Lake",
  );
  await expect(controls.getByLabel("Label font")).toHaveValue(
    "atkinson_hyperlegible",
  );
  await controls.getByLabel("Label font").selectOption("noto_sans");
  await controls.getByRole("slider", { name: "Flag width" }).fill("42");
  await controls.getByRole("slider", { name: "Flag height" }).fill("14");
  await controls.getByRole("slider", { name: "Flag label height" }).fill("5");
  await expect(
    controls.getByRole("checkbox", {
      name: "Export printable blank and named flags",
    }),
  ).toBeChecked();

  await page.getByRole("tab", { name: "Colors" }).click();
  await expect(
    page.getByRole("textbox", { name: "Map marker color" }),
  ).toHaveValue("#E24A33");

  await page.getByRole("button", { name: /^Generate/ }).click();
  await expect.poll(() => (jobSpec.markers as unknown[] | undefined)?.length).toBe(6);
  const markers = jobSpec.markers as Array<{
    name: string;
    kind: string;
    latitude: number;
    longitude: number;
    label_height_mm: number;
    rotation_degrees: number;
    label_style: {
      relief_mm: number;
      plaque_padding_mm: number;
      plaque_thickness_mm: number;
    };
  }>;
  expect(markers.map((marker) => marker.kind)).toEqual([
    "building",
    "dot",
    "flag_hole",
    "flag_label",
    "surface_label",
    "plaque_label",
  ]);
  expect(markers[0].name).toBe("Home");
  expect(markers[0].latitude).toBe(46.900001);
  expect(markers.every((marker) => Number.isFinite(marker.latitude))).toBe(true);
  expect(markers.every((marker) => Number.isFinite(marker.longitude))).toBe(true);
  expect(markers[3].name).toBe("富士山 Mount Fuji");
  expect(markers[4]).toMatchObject({
    name: "North Fork",
    label_height_mm: 5.5,
    rotation_degrees: 35,
    label_style: { relief_mm: 0.7 },
  });
  expect(markers[5]).toMatchObject({
    name: "Mirror Lake",
    rotation_degrees: -20,
    label_style: {
      relief_mm: 1,
      plaque_padding_mm: 2.5,
      plaque_thickness_mm: 1.4,
    },
  });
  expect((jobSpec.buildings as { enabled: boolean }).enabled).toBe(true);
  const markerSettings = jobSpec.marker_settings as {
    flag_height_mm: number;
    label_font: string;
    flag_label_height_mm: number;
    flag_width_mm: number;
    hole_diameter_mm: number;
  };
  expect(markerSettings.hole_diameter_mm).toBe(2.4);
  expect(markerSettings.label_font).toBe("noto_sans");
  expect(markerSettings.flag_label_height_mm).toBe(5);
  expect(markerSettings.flag_width_mm).toBe(42);
  expect(markerSettings.flag_height_mm).toBe(14);
});
