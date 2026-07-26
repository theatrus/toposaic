import { expect, test } from "@playwright/test";

import { mockSetupsService } from "./helpers";

test("pans the focused map with the arrow keys", async ({ page }) => {
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
  const initialLatitude = Number(await latitude.inputValue());
  const initialLongitude = Number(await longitude.inputValue());

  await map.focus();
  await expect(map).toBeFocused();

  // One press moves the center north by 10% of the ground span.
  await page.keyboard.press("ArrowUp");
  await expect
    .poll(async () => Number(await latitude.inputValue()))
    .toBeGreaterThan(initialLatitude);
  const afterStep = Number(await latitude.inputValue());
  const plainStep = afterStep - initialLatitude;

  // Shift raises the step to 50% of the span — five times the plain step.
  await page.keyboard.press("Shift+ArrowUp");
  await expect
    .poll(async () => Number(await latitude.inputValue()))
    .toBeGreaterThan(afterStep + plainStep * 3);

  // East-west panning works too, and longitude grows eastward.
  await page.keyboard.press("ArrowRight");
  await expect
    .poll(async () => Number(await longitude.inputValue()))
    .toBeGreaterThan(initialLongitude);
  await page.keyboard.press("Shift+ArrowLeft");
  await expect
    .poll(async () => Number(await longitude.inputValue()))
    .toBeLessThan(initialLongitude);

  // Handled keys never scroll the page under the map.
  expect(
    await page.evaluate(() => window.scrollX + window.scrollY),
  ).toBe(0);
});
