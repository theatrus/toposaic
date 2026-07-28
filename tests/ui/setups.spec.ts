import { expect, test } from "@playwright/test";
import { readFileSync } from "node:fs";

import { mockSetupsService } from "./helpers";

test("lists saved setups and recalls one over the generated preview", async ({
  page,
}) => {
  await mockSetupsService(page, [
    {
      id: "setup-alps",
      name: "Alps close-up",
      created_at: "2026-07-01T00:00:00Z",
      updated_at: "2026-07-02T00:00:00Z",
      spec: {
        place_name: "Alps close-up",
        ground_span_km: 3,
        center_lat: 46.2,
        center_lon: 7.9,
      },
    },
  ]);
  await page.goto("/");

  const trigger = page.locator(".setup-menu-button");
  await expect(trigger).toHaveText(/Saved setups/);

  await page.getByRole("button", { name: /^Generate/ }).click();
  await expect(page.getByText("Generated terrain").first()).toBeVisible({
    timeout: 15_000,
  });

  await trigger.click();
  const menu = page.getByRole("menu", { name: "Saved setups" });
  await menu.getByRole("menuitem", { name: "Alps close-up", exact: true }).click();
  await expect(page.getByText(/Recalled .Alps close-up/)).toBeVisible();
  await expect(menu).toBeHidden();
  await expect(trigger).toHaveText(/Alps close-up/);
  await expect(trigger).toBeFocused();
  await expect(page.getByText("Generated terrain")).toBeHidden();
  await page.getByRole("tab", { name: "Model" }).click();
  await expect(page.getByRole("slider", { name: "Ground span" })).toHaveValue(
    "3",
  );
  await expect(page.locator(".map-selection")).toHaveAttribute(
    "data-ground-span-km",
    "3",
  );

  // Reopening marks the recalled setup in the list.
  await trigger.click();
  await expect(
    menu.getByRole("menuitem", { name: "Alps close-up", exact: true }),
  ).toHaveAttribute("aria-current", "true");
});

test("repairs a recalled wall plate below its shown minimum", async ({
  page,
}) => {
  await mockSetupsService(page, [
    {
      id: "setup-old-wall-mount",
      name: "Old wall mount",
      created_at: "2026-07-01T00:00:00Z",
      updated_at: "2026-07-02T00:00:00Z",
      spec: {
        base_mm: 10,
        wall_mount: {
          style: "straight_pin",
          target: "terrain",
          thickness_mm: 1.2,
          wall_offset_mm: 4,
        },
      },
    },
  ]);
  await page.goto("/");

  await page.locator(".setup-menu-button").click();
  await page
    .getByRole("menu", { name: "Saved setups" })
    .getByRole("menuitem", { name: "Old wall mount", exact: true })
    .click();
  await page.getByRole("tab", { name: "Mounting" }).click();

  const wallPlateThickness = page.getByRole("slider", {
    name: "Wall plate thickness",
  });
  await expect(wallPlateThickness).toHaveAttribute("min", "4.4");
  await expect(wallPlateThickness).toHaveValue("4.4");
});

test("saves the current spec under a typed name and overwrites it", async ({
  page,
}) => {
  const state = await mockSetupsService(page, []);
  await page.goto("/");

  const trigger = page.locator(".setup-menu-button");
  await expect(trigger).toHaveText(/Saved setups/);
  await trigger.click();
  const menu = page.getByRole("menu", { name: "Saved setups" });
  await expect(menu.getByText("No saved setups yet")).toBeVisible();
  await expect(menu.getByRole("menuitem", { name: "Export" })).toBeDisabled();
  await menu.getByRole("menuitem", { name: "Save current setup" }).click();

  const setupName = page.getByLabel("Setup name");
  await expect(setupName).toHaveValue("Mount Rainier");
  await setupName.fill("My ridge");
  await setupName.press("Enter");

  await expect(page.getByText(/Saved .My ridge/)).toBeVisible();
  await expect(menu).toBeHidden();
  await expect(trigger).toBeFocused();
  await expect(trigger).toHaveText(/My ridge/);
  expect(state.saved).toHaveLength(1);
  expect(state.saved[0].name).toBe("My ridge");
  expect(state.saved[0].spec.place_name).toBe("Mount Rainier");
  expect(state.saved[0].spec.ground_span_km).toBe(18);

  // With the fresh save recalled, the name row prefills it and saving
  // under the same name overwrites. The status line says so: the service
  // answers 200 for an overwrite instead of the fresh save's 201.
  await trigger.click();
  await expect(
    menu.getByRole("menuitem", { name: "My ridge", exact: true }),
  ).toHaveAttribute("aria-current", "true");
  await menu.getByRole("menuitem", { name: "Save current setup" }).click();
  await expect(setupName).toHaveValue("My ridge");
  await setupName.press("Enter");
  await expect(page.getByText(/Replaced .My ridge/)).toBeVisible();
  await expect(menu).toBeHidden();
  await expect.poll(() => state.saved.length).toBe(2);
  expect(state.saved[1].name).toBe("My ridge");
});

test("duplicates a setup under a free derived name and starts a rename", async ({
  page,
}) => {
  const state = await mockSetupsService(page, [
    {
      id: "setup-alps",
      name: "Alps close-up",
      created_at: "2026-07-01T00:00:00Z",
      updated_at: "2026-07-02T00:00:00Z",
      spec: { place_name: "Alps close-up", ground_span_km: 3 },
    },
    {
      id: "setup-alps-2",
      name: "Alps close-up (2)",
      created_at: "2026-07-03T00:00:00Z",
      updated_at: "2026-07-03T00:00:00Z",
      spec: { place_name: "Alps close-up", ground_span_km: 3 },
    },
  ]);
  await page.goto("/");

  const trigger = page.locator(".setup-menu-button");
  await trigger.click();
  const menu = page.getByRole("menu", { name: "Saved setups" });
  await menu
    .getByRole("menuitem", { name: "Duplicate Alps close-up", exact: true })
    .click();

  // The copy skips taken names and lands in rename mode right away.
  await expect(
    page.getByText(/Duplicated .Alps close-up. as .Alps close-up \(3\)/),
  ).toBeVisible();
  const nameInput = page.getByLabel("New name for Alps close-up (3)");
  await expect(nameInput).toBeVisible();
  await expect(nameInput).toHaveValue("Alps close-up (3)");
  await expect(nameInput).toBeFocused();
  expect(state.saved).toHaveLength(1);
  expect(state.saved[0].name).toBe("Alps close-up (3)");
  expect(state.saved[0].spec.ground_span_km).toBe(3);

  await nameInput.fill("Alps fork");
  await nameInput.press("Enter");
  await expect(page.getByText(/Renamed to .Alps fork/)).toBeVisible();
  await expect(menu.getByRole("menuitem", { name: "Alps fork", exact: true })).toBeVisible();
  await expect(
    menu.getByRole("menuitem", { name: "Alps close-up", exact: true }),
  ).toBeVisible();
});

test("deletes a setup after an in-row confirmation", async ({ page }) => {
  await mockSetupsService(page, [
    {
      id: "setup-alps",
      name: "Alps close-up",
      created_at: "2026-07-01T00:00:00Z",
      updated_at: "2026-07-02T00:00:00Z",
      spec: { place_name: "Alps close-up", ground_span_km: 3 },
    },
    {
      id: "setup-rainier",
      name: "Rainier tray",
      created_at: "2026-06-01T00:00:00Z",
      updated_at: "2026-06-02T00:00:00Z",
      spec: { place_name: "Mount Rainier", width_mm: 240 },
    },
  ]);
  await page.goto("/");

  await page.locator(".setup-menu-button").click();
  const menu = page.getByRole("menu", { name: "Saved setups" });
  await menu.getByRole("menuitem", { name: "Delete Alps close-up" }).click();
  const confirm = menu.getByRole("menuitem", {
    name: "Confirm deleting Alps close-up",
  });
  await expect(confirm).toBeVisible();
  await confirm.click();

  await expect(page.getByText(/Deleted .Alps close-up/)).toBeVisible();
  await expect(menu).toBeVisible();
  await expect(
    menu.getByRole("menuitem", { name: "Alps close-up", exact: true }),
  ).toHaveCount(0);
  await expect(
    menu.getByRole("menuitem", { name: "Rainier tray", exact: true }),
  ).toBeVisible();
});

test("exports saved setups as a version-1 JSON download", async ({ page }) => {
  await mockSetupsService(page, [
    {
      id: "setup-alps",
      name: "Alps close-up",
      created_at: "2026-07-01T00:00:00Z",
      updated_at: "2026-07-02T00:00:00Z",
      spec: { place_name: "Alps close-up", ground_span_km: 3 },
    },
    {
      id: "setup-rainier",
      name: "Rainier tray",
      created_at: "2026-06-01T00:00:00Z",
      updated_at: "2026-06-02T00:00:00Z",
      spec: { place_name: "Mount Rainier", width_mm: 240 },
    },
  ]);
  await page.goto("/");

  await page.locator(".setup-menu-button").click();
  const menu = page.getByRole("menu", { name: "Saved setups" });
  await expect(
    menu.getByRole("menuitem", { name: "Rainier tray", exact: true }),
  ).toBeVisible();
  const downloadPromise = page.waitForEvent("download");
  await menu.getByRole("menuitem", { name: "Export" }).click();
  const download = await downloadPromise;
  expect(download.suggestedFilename()).toBe("toposaic-setups.json");
  const downloadPath = await download.path();
  const payload = JSON.parse(readFileSync(downloadPath, "utf8"));
  expect(payload.version).toBe(1);
  expect(payload.setups).toHaveLength(2);
  expect(payload.setups.map((entry: { name: string }) => entry.name)).toEqual([
    "Alps close-up",
    "Rainier tray",
  ]);
  expect(payload.setups[0].spec.ground_span_km).toBe(3);
  await expect(page.getByText("Exported 2 setups.")).toBeVisible();
});

test("imports setups from a JSON file and skips invalid entries", async ({
  page,
}) => {
  const state = await mockSetupsService(page, []);
  await page.goto("/");
  await expect(page.locator(".setup-menu-button")).toBeVisible();

  const payload = {
    version: 1,
    setups: [
      { name: "Alps close-up", spec: { ground_span_km: 2 } },
      { name: "Rainier tray", spec: { width_mm: 120 } },
      { name: "", spec: {} },
      "junk",
    ],
  };
  await page.getByLabel("Import setups file").setInputFiles({
    name: "toposaic-setups.json",
    mimeType: "application/json",
    buffer: Buffer.from(JSON.stringify(payload)),
  });

  await expect(page.getByText("Imported 2, skipped 2 invalid.")).toBeVisible();
  expect(state.saved.map((entry) => entry.name)).toEqual([
    "Alps close-up",
    "Rainier tray",
  ]);
  expect(state.saved[0].spec.ground_span_km).toBe(2);
  expect(state.saved[0].spec.width_mm).toBe(180);
  await page.locator(".setup-menu-button").click();
  await expect(
    page
      .getByRole("menu", { name: "Saved setups" })
      .getByRole("menuitem", { name: "Alps close-up", exact: true }),
  ).toBeVisible();
});

test("renames a saved setup and surfaces name conflicts", async ({ page }) => {
  const state = await mockSetupsService(page, [
    {
      id: "setup-alps",
      name: "Alps close-up",
      created_at: "2026-07-01T00:00:00Z",
      updated_at: "2026-07-02T00:00:00Z",
      spec: { place_name: "Alps close-up", ground_span_km: 3 },
    },
    {
      id: "setup-rainier",
      name: "Rainier tray",
      created_at: "2026-06-01T00:00:00Z",
      updated_at: "2026-06-02T00:00:00Z",
      spec: { place_name: "Mount Rainier", width_mm: 240 },
    },
  ]);
  await page.goto("/");

  await page.locator(".setup-menu-button").click();
  const menu = page.getByRole("menu", { name: "Saved setups" });
  await menu.getByRole("menuitem", { name: "Rename Alps close-up" }).click();

  const nameInput = page.getByLabel("New name for Alps close-up");
  await expect(nameInput).toHaveValue("Alps close-up");
  await nameInput.fill("Rainier tray");
  await nameInput.press("Enter");
  await expect(
    page.getByText("A setup named “Rainier tray” already exists."),
  ).toBeVisible();
  // The conflict keeps the input open for a corrected name.
  await expect(nameInput).toBeVisible();

  await nameInput.fill("Alps wide");
  await nameInput.press("Enter");
  await expect(page.getByText(/Renamed to .Alps wide/)).toBeVisible();
  expect(state.renamed).toEqual([{ id: "setup-alps", name: "Alps wide" }]);
  const renamedRow = menu.getByRole("menuitem", { name: "Alps wide", exact: true });
  await expect(renamedRow).toBeVisible();
  await expect(renamedRow).toBeFocused();
  await expect(
    menu.getByRole("menuitem", { name: "Alps close-up", exact: true }),
  ).toHaveCount(0);

  // Escape cancels an open rename and returns focus to the row.
  await menu.getByRole("menuitem", { name: "Rename Rainier tray" }).click();
  await expect(page.getByLabel("New name for Rainier tray")).toBeVisible();
  await page.keyboard.press("Escape");
  await expect(menu).toBeVisible();
  await expect(
    menu.getByRole("menuitem", { name: "Rainier tray", exact: true }),
  ).toBeFocused();
});

test("drives the setups menu from the keyboard", async ({ page }) => {
  await mockSetupsService(page, [
    {
      id: "setup-alps",
      name: "Alps close-up",
      created_at: "2026-07-01T00:00:00Z",
      updated_at: "2026-07-02T00:00:00Z",
      spec: { place_name: "Alps close-up", ground_span_km: 3 },
    },
  ]);
  await page.goto("/");

  const menuButton = page.locator(".setup-menu-button");
  await expect(menuButton).toHaveAttribute("aria-haspopup", "menu");
  await menuButton.click();
  const menu = page.getByRole("menu", { name: "Saved setups" });
  await expect(menu).toBeVisible();
  await expect(menuButton).toHaveAttribute("aria-expanded", "true");

  const row = menu.getByRole("menuitem", { name: "Alps close-up", exact: true });
  // The row's actions in order: History, then Rename, Duplicate, Delete.
  // (A recalled setup also carries Save ahead of these; this one is not
  // recalled, so its row starts at History.)
  const history = menu.getByRole("menuitem", {
    name: "Earlier versions of Alps close-up",
  });
  const rename = menu.getByRole("menuitem", { name: "Rename Alps close-up" });
  await expect(row).toBeFocused();
  await page.keyboard.press("ArrowDown");
  await expect(history).toBeFocused();
  await page.keyboard.press("ArrowDown");
  await expect(rename).toBeFocused();
  await page.keyboard.press("ArrowUp");
  await expect(history).toBeFocused();
  await page.keyboard.press("ArrowUp");
  await expect(row).toBeFocused();
  await page.keyboard.press("ArrowUp");
  await expect(menu.getByRole("menuitem", { name: "Import" })).toBeFocused();
  await page.keyboard.press("Home");
  await expect(row).toBeFocused();

  await page.keyboard.press("Escape");
  await expect(menu).toBeHidden();
  await expect(menuButton).toBeFocused();
  await expect(menuButton).toHaveAttribute("aria-expanded", "false");

  await menuButton.click();
  await expect(menu).toBeVisible();
  await page.getByRole("heading", { name: "Shape your terrain" }).click();
  await expect(menu).toBeHidden();
});
