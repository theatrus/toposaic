import { expect, test } from "@playwright/test";

import { mockSetupsService } from "./helpers";

test("existing setups opt into automatic surfaces and submit their category colors", async ({ page }) => {
  await mockSetupsService(page, []);
  await page.goto("/");
  await page.getByRole("tab", { name: "Surface", exact: true }).click();
  const surfaces = page.getByRole("checkbox", { name: "Distinguish neighborhood surfaces" });
  await expect(surfaces).not.toBeChecked();
  await surfaces.check();
  await page.getByRole("tab", { name: "Colors", exact: true }).click();
  await expect(page.getByLabel("Grass / lawns color", { exact: true })).toHaveValue("#76A542");
  await expect(page.getByLabel("Sports surfaces color", { exact: true })).toHaveValue("#2F76B5");
  await expect(page.getByLabel("Playgrounds color", { exact: true })).toHaveValue("#9A6C42");
  await page.getByLabel("Paved areas color", { exact: true }).fill("#112233");
  const requestPromise = page.waitForRequest((request) =>
    request.url().endsWith("/api/jobs") && request.method() === "POST",
  );
  await page.getByRole("button", { name: /^Generate/ }).click();
  const spec = (await requestPromise).postDataJSON();
  expect(spec.color_output.neighborhood_surfaces).toEqual({
    enabled: true,
    grass_color: "#76A542",
    pavement_color: "#112233",
    sport_color: "#2F76B5",
    playground_color: "#9A6C42",
  });
  await page.getByRole("tab", { name: "Surface", exact: true }).click();
  await surfaces.uncheck();
  await page.getByRole("tab", { name: "Colors", exact: true }).click();
  await expect(page.getByLabel("Grass / lawns color", { exact: true })).toHaveCount(0);
  await expect(page.getByLabel("Paved areas color", { exact: true })).toHaveCount(0);
});
