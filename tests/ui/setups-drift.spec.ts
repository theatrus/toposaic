import { expect, test } from "@playwright/test";

import { mockSetupsService } from "./helpers";

test("offers a save only once the model has drifted, and rolls back", async ({
  page,
}) => {
  await mockSetupsService(page, [
    { id: "11111111-1111-4111-8111-111111111111", name: "Rainier 18 km",
      created_at: "2026-07-01T00:00:00Z", updated_at: "2026-07-02T00:00:00Z",
      spec: { relief_mm: 28 } },
  ]);
  await page.route("**/api/setups/*/versions", async (route) => {
    await route.fulfill({ json: [
      { id: "22222222-2222-4222-8222-222222222222", saved_at: "2026-07-02T09:15:00Z", spec: { relief_mm: 20 } },
    ] });
  });
  await page.setViewportSize({ width: 1500, height: 1000 });
  await page.goto("/");
  await page.locator(".setup-menu-button").click();
  const menu = page.getByRole("menu", { name: "Saved setups" });

  // Nothing recalled yet, so no Save offer at all.
  await expect(menu.getByRole("menuitem", { name: /^Save changes to/ })).toHaveCount(0);

  await menu.getByRole("menuitem", { name: "Rainier 18 km", exact: true }).click();
  await page.locator(".setup-menu-button").click();
  // Freshly recalled: nothing to save.
  await expect(
    menu.getByRole("menuitem", { name: /has no unsaved changes/ }),
  ).toBeDisabled();

  await menu.getByRole("menuitem", { name: /Earlier versions of/ }).click();
  await expect(menu.getByRole("menuitem", { name: /Roll .* back to the version/ })).toBeVisible();

  // Move the model and the offer to save appears.
  await page.keyboard.press("Escape");
  await page.getByRole("slider", { name: "Terrain height" }).fill("55");
  await page.locator(".setup-menu-button").click();
  await expect(
    menu.getByRole("menuitem", { name: /^Save changes to/ }),
  ).toBeEnabled();
});
