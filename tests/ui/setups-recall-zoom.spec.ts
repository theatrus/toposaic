import { expect, test } from "@playwright/test";

import { mockSetupsService } from "./helpers";

// Linked zoom resizes the selected area, so leaving it on after a recall
// means one scroll silently redraws the ground the setup named.
test("recalling a setup leaves map zoom unlinked", async ({ page }) => {
  await mockSetupsService(page, [
    { id: "11111111-1111-4111-8111-111111111111", name: "Alps",
      created_at: "2026-07-01T00:00:00Z", updated_at: "2026-07-02T00:00:00Z",
      spec: { ground_span_km: 6 } },
  ]);
  await page.setViewportSize({ width: 1500, height: 1000 });
  await page.goto("/");
  const mode = page.getByRole("button", { name: /map zoom|selected area with map zoom/i });
  await expect(mode).toHaveText("Linked");

  await page.locator(".setup-menu-button").click();
  await page
    .getByRole("menu", { name: "Saved setups" })
    .getByRole("menuitem", { name: "Alps", exact: true })
    .click();
  await expect(mode).toHaveText("Map only");
});
