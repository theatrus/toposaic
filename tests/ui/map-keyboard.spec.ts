import { expect, test } from "@playwright/test";

import { mockSetupsService } from "./helpers";

test("pans the map without moving the terrain area", async ({ page }) => {
  await mockSetupsService(page, []);
  await page.goto("/");

  const map = page.locator(".map-canvas");
  await expect(map).toHaveAttribute("tabindex", "0");
  await expect(map).toHaveAttribute("title", /Arrow keys pan/);
  await expect(map).toHaveAttribute(
    "aria-keyshortcuts",
    "ArrowUp ArrowDown ArrowLeft ArrowRight",
  );

  const latitude = page.getByLabel("Latitude");
  const longitude = page.getByLabel("Longitude");
  const initialLatitude = await latitude.inputValue();
  const initialLongitude = await longitude.inputValue();
  const initialViewLatitude = Number(
    await map.getAttribute("data-view-latitude"),
  );
  const initialViewLongitude = Number(
    await map.getAttribute("data-view-longitude"),
  );

  await expect(
    page.getByRole("button", { name: "Pan map without moving terrain area" }),
  ).toHaveAttribute("aria-pressed", "true");
  await map.focus();
  await expect(map).toBeFocused();

  // One press moves only the map view north by 10% of the ground span.
  await page.keyboard.press("ArrowUp");
  await expect
    .poll(async () => Number(await map.getAttribute("data-view-latitude")))
    .toBeGreaterThan(initialViewLatitude);
  const afterStep = Number(await map.getAttribute("data-view-latitude"));
  const plainStep = afterStep - initialViewLatitude;
  await expect(latitude).toHaveValue(initialLatitude);

  // Shift raises the step to 50% of the span — five times the plain step.
  await page.keyboard.press("Shift+ArrowUp");
  await expect
    .poll(async () => Number(await map.getAttribute("data-view-latitude")))
    .toBeGreaterThan(afterStep + plainStep * 3);

  // East-west panning works too, and longitude grows eastward.
  await page.keyboard.press("ArrowRight");
  await expect
    .poll(async () => Number(await map.getAttribute("data-view-longitude")))
    .toBeGreaterThan(initialViewLongitude);
  await page.keyboard.press("Shift+ArrowLeft");
  await expect
    .poll(async () => Number(await map.getAttribute("data-view-longitude")))
    .toBeLessThan(initialViewLongitude);
  await expect(longitude).toHaveValue(initialLongitude);

  // Handled keys never scroll the page under the map.
  expect(await page.evaluate(() => window.scrollX + window.scrollY)).toBe(0);
});

test("pans independently and draws a new terrain area", async ({ page }) => {
  await mockSetupsService(page, []);
  await page.setViewportSize({ width: 1440, height: 900 });
  await page.goto("/");

  const map = page.locator(".map-canvas");
  const selection = page.locator(".map-selection.current");
  const latitude = page.getByLabel("Latitude");
  const longitude = page.getByLabel("Longitude");
  const groundSpan = page.getByRole("slider", { name: "Ground span" });
  const mapBounds = await map.boundingBox();
  const initialSelectionBounds = await selection.boundingBox();
  expect(mapBounds).not.toBeNull();
  expect(initialSelectionBounds).not.toBeNull();
  if (!mapBounds || !initialSelectionBounds) return;

  const initialLatitude = await latitude.inputValue();
  const initialLongitude = await longitude.inputValue();
  await page.mouse.move(
    mapBounds.x + mapBounds.width * 0.5,
    mapBounds.y + mapBounds.height * 0.5,
  );
  await page.mouse.down();
  await page.mouse.move(
    mapBounds.x + mapBounds.width * 0.5 + 90,
    mapBounds.y + mapBounds.height * 0.5,
    { steps: 5 },
  );
  await page.mouse.up();

  await expect(latitude).toHaveValue(initialLatitude);
  await expect(longitude).toHaveValue(initialLongitude);
  const pannedSelectionBounds = await selection.boundingBox();
  expect(pannedSelectionBounds).not.toBeNull();
  expect(pannedSelectionBounds!.x).toBeGreaterThan(
    initialSelectionBounds.x + 70,
  );
  const centerButton = page.getByRole("button", {
    name: "Center map on terrain area",
  });
  await expect(centerButton).toBeEnabled();
  await centerButton.click();
  await expect(centerButton).toBeDisabled();
  await expect
    .poll(async () => (await selection.boundingBox())?.x ?? 0)
    .toBeCloseTo(initialSelectionBounds.x, 0);

  await page.getByRole("button", { name: "Draw terrain area" }).click();
  await expect(map).toHaveAttribute("data-interaction-mode", "select");
  const start = {
    x: mapBounds.x + mapBounds.width * 0.28,
    y: mapBounds.y + mapBounds.height * 0.3,
  };
  const end = { x: start.x + 120, y: start.y + 105 };
  await page.mouse.move(start.x, start.y);
  await page.mouse.down();
  await page.mouse.move(end.x, end.y, { steps: 6 });
  const draft = page.locator(".map-selection-draft");
  await expect(draft).toBeVisible();
  const draftBounds = await draft.boundingBox();
  expect(draftBounds).not.toBeNull();
  expect(draftBounds!.width).toBeCloseTo(draftBounds!.height, 0);
  await page.mouse.up();

  await expect(draft).toHaveCount(0);
  await expect(latitude).not.toHaveValue(initialLatitude);
  await expect(longitude).not.toHaveValue(initialLongitude);
  await expect(groundSpan).not.toHaveValue("18");
});

test("draws the full super-tile footprint at its grid ratio", async ({
  page,
}) => {
  await mockSetupsService(page, []);
  await page.setViewportSize({ width: 1440, height: 900 });
  await page.goto("/");

  const grid = page.getByLabel("Super-tile grid");
  await grid.getByLabel("Across").selectOption("3");
  await grid.getByLabel("Down").selectOption("2");
  await page.getByRole("button", { name: "Draw terrain area" }).click();
  await expect(page.locator(".map-instruction")).toHaveText(
    "Drag 3 × 2 area",
  );

  const map = page.locator(".map-canvas");
  const mapBounds = await map.boundingBox();
  expect(mapBounds).not.toBeNull();
  if (!mapBounds) return;
  const start = {
    x: mapBounds.x + mapBounds.width * 0.3,
    y: mapBounds.y + mapBounds.height * 0.3,
  };
  await page.mouse.move(start.x, start.y);
  await page.mouse.down();
  await page.mouse.move(start.x + 150, start.y + 60, { steps: 6 });
  const draft = page.locator(".map-selection-draft");
  await expect(draft).toHaveAttribute(
    "aria-label",
    "New terrain area: 3 across by 2 down",
  );
  const bounds = await draft.boundingBox();
  expect(bounds).not.toBeNull();
  expect(bounds!.width / bounds!.height).toBeCloseTo(1.5, 2);
  await page.mouse.up();
  await expect(draft).toHaveCount(0);
  await expect(page.locator(".map-selection")).toHaveCount(6);
});
