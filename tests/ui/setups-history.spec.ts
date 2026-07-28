import { expect, test } from "@playwright/test";

import { mockSetupsService } from "./helpers";

test("switching rows never shows the other setup's versions", async ({ page }) => {
  await mockSetupsService(page, [
    { id: "11111111-1111-4111-8111-111111111111", name: "Alps",
      created_at: "2026-07-01T00:00:00Z", updated_at: "2026-07-02T00:00:00Z", spec: { relief_mm: 28 } },
    { id: "33333333-3333-4333-8333-333333333333", name: "Rainier",
      created_at: "2026-07-01T00:00:00Z", updated_at: "2026-07-01T00:00:00Z", spec: { relief_mm: 30 } },
  ]);
  // Alps answers at once; Rainier is slow, which is when a stale list shows.
  await page.route("**/api/setups/*/versions", async (route) => {
    const url = route.request().url();
    if (url.includes("11111111")) {
      await route.fulfill({ json: [
        { id: "22222222-2222-4222-8222-222222222222", saved_at: "2026-07-02T09:15:00Z", spec: { relief_mm: 20 } },
      ] });
      return;
    }
    await new Promise((resolve) => setTimeout(resolve, 1500));
    await route.fulfill({ json: [] });
  });
  await page.setViewportSize({ width: 1500, height: 1000 });
  await page.goto("/");
  await page.locator(".setup-menu-button").click();
  const menu = page.getByRole("menu", { name: "Saved setups" });

  await menu.getByRole("menuitem", { name: "Earlier versions of Alps" }).click();
  await expect(menu.getByRole("menuitem", { name: /Roll Alps back/ })).toBeVisible();

  // Now open Rainier's history. While its request is in flight, Alps's
  // versions must not be on show under Rainier.
  await menu.getByRole("menuitem", { name: "Earlier versions of Rainier" }).click();
  await page.waitForTimeout(300);
  // Rainier has no versions, and its request is still in flight, so no
  // roll-back may be on offer under it — any that is belongs to Alps.
  // Counted at this instant, NOT with expect's retry: a retrying assertion
  // would simply wait out the slow response and pass on the empty list.
  expect(
    await menu.getByRole("menuitem", { name: /Roll .* back/ }).count(),
  ).toBe(0);
});
