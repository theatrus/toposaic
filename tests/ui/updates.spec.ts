import { expect, test } from "@playwright/test";

import { appMajor, appMinor, appVersion, newerVersion } from "./helpers";

test("shows and dismisses a newer desktop release notice", async ({ page }) => {
  await page.addInitScript(() => {
    Object.defineProperty(window, "__TAURI_INTERNALS__", {
      configurable: true,
      value: {},
    });
  });
  await page.route(
    "https://api.github.com/repos/theatrus/toposaic/releases/latest",
    async (route) => {
      await route.fulfill({
        contentType: "application/json",
        body: JSON.stringify({
          draft: false,
          prerelease: false,
          tag_name: `v${newerVersion}`,
          html_url: `https://github.com/theatrus/toposaic/releases/tag/v${newerVersion}`,
        }),
      });
    },
  );
  await page.route(
    "https://toposaic.com/releases/notice.json",
    async (route) => {
      await route.fulfill({
        contentType: "application/json",
        body: JSON.stringify({
          schema_version: 1,
          version: appVersion,
          release_url:
            "https://github.com/theatrus/toposaic/releases/tag/" +
            `v${appVersion}`,
          urgency: "normal",
        }),
      });
    },
  );
  await page.route("http://127.0.0.1:38787/api/preview", async (route) => {
    await route.abort();
  });

  await page.goto("/");

  const notice = page
    .getByRole("status")
    .filter({ hasText: `v${newerVersion} available` });
  await expect(notice).toContainText(`Current v${appVersion}`);
  await expect(notice.getByRole("link", { name: "Download" })).toHaveAttribute(
    "href",
    `https://github.com/theatrus/toposaic/releases/tag/v${newerVersion}`,
  );
  await notice
    .getByRole("button", {
      name: `Dismiss v${newerVersion} update notice`,
    })
    .click();
  await expect(notice).toBeHidden();
});

test("prefers a newer valid TopoSaic site notice", async ({ page }) => {
  const siteVersion = `${appMajor}.${appMinor + 2}.0`;
  await page.addInitScript(() => {
    Object.defineProperty(window, "__TAURI_INTERNALS__", {
      configurable: true,
      value: {},
    });
  });
  await page.route(
    "https://api.github.com/repos/theatrus/toposaic/releases/latest",
    async (route) => {
      await route.fulfill({
        contentType: "application/json",
        body: JSON.stringify({
          draft: false,
          prerelease: false,
          tag_name: `v${newerVersion}`,
          html_url:
            "https://github.com/theatrus/toposaic/releases/tag/" +
            `v${newerVersion}`,
        }),
      });
    },
  );
  await page.route(
    "https://toposaic.com/releases/notice.json",
    async (route) => {
      await route.fulfill({
        contentType: "application/json",
        body: JSON.stringify({
          schema_version: 1,
          version: siteVersion,
          release_url:
            "https://github.com/theatrus/toposaic/releases/tag/" +
            `v${siteVersion}`,
          summary: "New terrain tools.",
          urgency: "recommended",
          minimum_supported_version: appVersion,
          published_at: "2026-07-24T18:00:00Z",
        }),
      });
    },
  );
  await page.route("http://127.0.0.1:38787/api/preview", async (route) => {
    await route.abort();
  });

  await page.goto("/");

  const notice = page
    .getByRole("status")
    .filter({ hasText: `v${siteVersion} available` });
  await expect(notice).toBeVisible();
  await expect(notice.getByRole("link", { name: "Download" })).toHaveAttribute(
    "href",
    `https://github.com/theatrus/toposaic/releases/tag/v${siteVersion}`,
  );
});
