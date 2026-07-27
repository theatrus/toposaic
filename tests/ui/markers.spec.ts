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
  await controls.getByLabel("Marker 2 dot diameter").fill("5");
  await controls.getByRole("button", { name: "Blank flag" }).click();
  await map.click({ position: { x: 300, y: 210 } });
  await controls.getByLabel("Marker 3 flag-hole diameter").fill("3.2");
  await controls.getByLabel("Marker 3 flag width").fill("34");
  await controls.getByLabel("Marker 3 export printable flag").uncheck();
  await controls.getByRole("button", { name: "Named flag" }).click();
  await map.click({ position: { x: 180, y: 100 } });
  await controls.getByLabel("Marker 4 name").fill("富士山 Mount Fuji");
  await controls.getByLabel("Marker 4 label font").selectOption("noto_sans");
  await controls.getByLabel("Marker 4 flag width").fill("42");
  await controls.getByLabel("Marker 4 flag height").fill("14");
  await controls.getByLabel("Marker 4 flag label height").fill("5");
  await expect(page.locator(".map-marker-name")).toHaveText(
    "富士山 Mount Fuji",
  );
  await controls.getByRole("button", { name: "Surface label" }).click();
  await map.click({ position: { x: 340, y: 120 } });
  await controls.getByLabel("Marker 5 name").fill("North Fork");
  await controls.getByLabel("Marker 5 text height").fill("5.5");
  await controls.getByLabel("Marker 5 rotation").fill("35");
  await controls.getByLabel("Marker 5 relief").fill("0.7");
  await controls.getByLabel("Marker 5 label font").selectOption("b612_mono");
  await controls.getByRole("button", { name: "Raised plaque" }).click();
  await map.click({ position: { x: 380, y: 80 } });
  await controls.getByLabel("Marker 6 name").fill("Mirror Lake");
  await controls.getByLabel("Marker 6 text height").fill("8");
  await controls.getByLabel("Marker 6 rotation").fill("-20");
  await controls.getByLabel("Marker 6 relief").fill("1");
  await controls.getByLabel("Marker 6 plaque padding").fill("2.5");
  await controls.getByLabel("Marker 6 plaque base height").fill("1.4");

  await expect(page.locator(".map-marker")).toHaveCount(6);
  await expect(page.locator(".map-marker.dot")).toHaveCount(1);
  await expect(
    page.getByLabel("Interactive 3D terrain preview"),
  ).toHaveAttribute("data-vector-dot-count", "1");
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
  await expect(
    page.locator(".map-marker.surface_label .map-feature-label-text"),
  ).toHaveAttribute("data-label-height-mm", "5.5");
  await expect(
    page.locator(".map-marker.plaque_label .map-feature-label-text"),
  ).toHaveAttribute("data-label-height-mm", "8");
  const surfaceLabel = page.locator(
    ".map-marker.surface_label .map-feature-label-text",
  );
  const fontSizeBeforeZoom = await surfaceLabel.evaluate((element) =>
    Number.parseFloat(getComputedStyle(element).fontSize),
  );
  await page
    .getByRole("button", { name: "Resize selected area with map zoom" })
    .click();
  await page.getByRole("button", { name: "Zoom in" }).click();
  await expect
    .poll(() =>
      surfaceLabel.evaluate((element) =>
        Number.parseFloat(getComputedStyle(element).fontSize),
      ),
    )
    .toBeGreaterThan(fontSizeBeforeZoom);
  await expect(
    controls.getByRole("button", { name: /^Move marker/ }),
  ).toHaveCount(6);
  const moveHome = controls.getByRole("button", {
    name: "Move marker Home",
    exact: true,
  });
  await moveHome.click();
  await expect(
    controls.getByRole("button", { name: "Cancel moving marker Home" }),
  ).toHaveAttribute("aria-pressed", "true");
  await expect(page.locator(".map-instruction")).toContainText(
    "Click the map to move the marker",
  );
  await map.click({ position: { x: 440, y: 40 } });
  await expect(controls.getByLabel("Marker 1 latitude")).not.toHaveValue(
    "46.900001",
  );
  await expect(moveHome).toHaveAttribute("aria-pressed", "false");

  const moveFlag = controls.getByRole("button", {
    name: "Move marker 富士山 Mount Fuji",
    exact: true,
  });
  await moveFlag.click();
  await controls
    .getByRole("button", { name: "Cancel moving marker 富士山 Mount Fuji" })
    .click();
  await expect(moveFlag).toHaveAttribute("aria-pressed", "false");
  await expect(
    controls.getByLabel("Marker 4 export printable flag"),
  ).toBeChecked();
  await page.getByRole("tab", { name: "Colors" }).click();
  await expect(
    page.getByRole("textbox", { name: "Map marker color" }),
  ).toHaveValue("#E24A33");

  await page.getByRole("button", { name: /^Generate/ }).click();
  await expect
    .poll(() => (jobSpec.markers as unknown[] | undefined)?.length)
    .toBe(6);
  const markers = jobSpec.markers as Array<{
    name: string;
    kind: string;
    latitude: number;
    longitude: number;
    label_height_mm: number;
    rotation_degrees: number;
    dot_style?: { diameter_mm: number };
    flag_style?: {
      hole_diameter_mm: number;
      width_mm: number;
      height_mm: number;
      label_height_mm: number;
      label_font: string;
      export_template: boolean;
    };
    label_style: {
      label_font: string;
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
  expect(markers[0].latitude).not.toBe(46.900001);
  expect(markers.every((marker) => Number.isFinite(marker.latitude))).toBe(
    true,
  );
  expect(markers.every((marker) => Number.isFinite(marker.longitude))).toBe(
    true,
  );
  expect(markers[3].name).toBe("富士山 Mount Fuji");
  expect(markers[1].dot_style).toEqual({ diameter_mm: 5 });
  expect(markers[2].flag_style).toMatchObject({
    hole_diameter_mm: 3.2,
    width_mm: 34,
    export_template: false,
  });
  expect(markers[3].flag_style).toMatchObject({
    label_font: "noto_sans",
    label_height_mm: 5,
    width_mm: 42,
    height_mm: 14,
    export_template: true,
  });
  expect(markers[4]).toMatchObject({
    name: "North Fork",
    label_height_mm: 5.5,
    rotation_degrees: 35,
    label_style: { label_font: "b612_mono", relief_mm: 0.7 },
  });
  expect(markers[5]).toMatchObject({
    name: "Mirror Lake",
    label_height_mm: 8,
    rotation_degrees: -20,
    label_style: {
      relief_mm: 1,
      plaque_padding_mm: 2.5,
      plaque_thickness_mm: 1.4,
    },
  });
  expect((jobSpec.buildings as { enabled: boolean }).enabled).toBe(true);
  expect(jobSpec.marker_settings).toEqual({ color: "#E24A33" });
});
