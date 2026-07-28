import { expect, test } from "@playwright/test";

test("the base label can be switched off", async ({ page }) => {
  await page.route("http://127.0.0.1:8787/api/**", async (route) => {
    const url = new URL(route.request().url());
    if (url.pathname === "/api/preview") { await route.fulfill({ json: { width: 2, height: 2, values: [0, 0.3, 0.7, 1] } }); return; }
    await route.abort();
  });
  await page.setViewportSize({ width: 1500, height: 1100 });
  await page.goto("/");
  await page.getByRole("tab", { name: "Mounting" }).click();
  await page.getByRole("checkbox", { name: "Generate display tray" }).check();

  const label = page.getByRole("checkbox", { name: "Emboss the label on the base" });
  await expect(label).toBeChecked();
  await expect(page.getByLabel("Label position")).toBeVisible();
  await label.uncheck();
  await expect(page.getByLabel("Label position")).toHaveCount(0);
  // Turning it off also stops the panel promising the coordinates.
  await expect(page.getByText(/raised shapes on the top front lip/)).toHaveCount(
    0,
  );
  await label.check();
  await expect(
    page.getByText(/raised shapes on the top front lip/),
  ).toBeVisible();
});
