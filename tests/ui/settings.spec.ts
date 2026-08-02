import { expect, test } from "@playwright/test";

import { mockSetupsService } from "./helpers";

// The mock cache holds 50 MB of elevation, 10 MB of land cover, 1 MB of
// OSM, and 2 KB of place search — 61 MB in total.

test("chooses and keeps a higher live preview detail", async ({ page }) => {
  const state = await mockSetupsService(page, []);
  await page.goto("/");

  await page.getByRole("button", { name: "Settings" }).click();
  const pane = page.getByRole("dialog", { name: "Settings" });
  const detail = pane.getByLabel("Preview detail");
  await expect(detail).toHaveValue("fast");
  await detail.selectOption("detailed");
  await expect(detail).toHaveValue("detailed");
  await expect
    .poll(() => state.previewDetails.at(-1))
    .toBe("detailed");

  await page.reload();
  await page.getByRole("button", { name: "Settings" }).click();
  await expect(
    page.getByRole("dialog", { name: "Settings" }).getByLabel("Preview detail"),
  ).toHaveValue("detailed");
});

test("opens the settings pane, lists cache sizes, and closes cleanly", async ({
  page,
}) => {
  await mockSetupsService(page, []);
  await page.goto("/");

  const settingsButton = page.getByRole("button", { name: "Settings" });
  await expect(settingsButton).toHaveAttribute("aria-haspopup", "dialog");
  await settingsButton.click();

  const pane = page.getByRole("dialog", { name: "Settings" });
  await expect(pane).toBeVisible();
  await expect(settingsButton).toHaveAttribute("aria-expanded", "true");
  await expect(
    pane.getByRole("heading", { name: "Map data cache" }),
  ).toBeVisible();

  const rows = pane.locator(".settings-cache-rows li");
  await expect(rows.filter({ hasText: "Elevation tiles" })).toContainText(
    "50 MB",
  );
  await expect(rows.filter({ hasText: "Elevation tiles" })).toContainText(
    "120 entries",
  );
  await expect(rows.filter({ hasText: "Land cover" })).toContainText("10 MB");
  await expect(rows.filter({ hasText: "OpenStreetMap" })).toContainText(
    "1.0 MB",
  );
  await expect(rows.filter({ hasText: "Place search" })).toContainText(
    "2.0 KB",
  );
  await expect(rows.filter({ hasText: "Total" })).toContainText("61 MB");
  await expect(pane.getByLabel("Older than")).toHaveValue("30");
  await expect(pane.getByText(/Clearing is always manual/)).toBeVisible();

  // Escape closes the pane and hands focus back to the gear button.
  await page.keyboard.press("Escape");
  await expect(pane).toBeHidden();
  await expect(settingsButton).toBeFocused();
  await expect(settingsButton).toHaveAttribute("aria-expanded", "false");

  // An outside click closes it too.
  await settingsButton.click();
  await expect(pane).toBeVisible();
  await page.getByRole("heading", { name: "Shape your terrain" }).click();
  await expect(pane).toBeHidden();
});

test("clears cache entries older than the selected age", async ({ page }) => {
  const state = await mockSetupsService(page, []);
  await page.goto("/");

  await page.getByRole("button", { name: "Settings" }).click();
  const pane = page.getByRole("dialog", { name: "Settings" });
  await expect(pane.getByText(/Total/)).toBeVisible();

  await pane.getByLabel("Older than").selectOption("7");
  await pane.getByRole("button", { name: "Clear older" }).click();

  await expect(page.getByText("Removed 1.0 MB (30 entries).")).toBeVisible();
  expect(state.cleared).toEqual([7]);

  // The sizes refresh after the clear: OSM is empty and the total shrinks.
  const rows = pane.locator(".settings-cache-rows li");
  await expect(rows.filter({ hasText: "OpenStreetMap" })).toContainText("0 B");
  await expect(rows.filter({ hasText: "Total" })).toContainText("60 MB");
});

test("clears the whole cache only after an inline confirm", async ({
  page,
}) => {
  const state = await mockSetupsService(page, []);
  await page.goto("/");

  await page.getByRole("button", { name: "Settings" }).click();
  const pane = page.getByRole("dialog", { name: "Settings" });
  await expect(pane.getByText(/Total/)).toBeVisible();

  // The first click only arms the confirm step; nothing is cleared yet.
  await pane.getByRole("button", { name: "Clear all cached map data" }).click();
  const confirm = pane.getByRole("button", {
    name: "Confirm clearing the whole cache",
  });
  await expect(confirm).toBeVisible();
  await expect(confirm).toHaveText("Confirm");
  expect(state.cleared).toEqual([]);

  await confirm.click();
  await expect(page.getByText("Removed 61 MB (163 entries).")).toBeVisible();
  expect(state.cleared).toEqual([null]);
  const rows = pane.locator(".settings-cache-rows li");
  await expect(rows.filter({ hasText: "Total" })).toContainText("0 B");
  await expect(rows.filter({ hasText: "Elevation tiles" })).toContainText(
    "0 B",
  );
});
