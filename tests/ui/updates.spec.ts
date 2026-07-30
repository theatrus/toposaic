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
    "https://updates.toposaic.com/notice.json",
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
    "https://updates.toposaic.com/notice.json",
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

  // The notice's own line is what the release says about itself. Before
  // this it was parsed, capped, and then never shown, so anything written
  // in notice.json went nowhere.
  const line = notice.locator("small");
  await expect(line).toHaveText("New terrain tools.");
  // It is clipped to one top-bar row, so hovering gives the whole of it —
  // and the running version, which the summary displaced.
  await expect(line).toHaveAttribute(
    "title",
    `New terrain tools. — current v${appVersion}`,
  );
});

test("falls back to the running version when a notice carries no summary", async ({
  page,
}) => {
  const siteVersion = `${appMajor}.${appMinor + 2}.0`;
  await page.addInitScript(() => {
    Object.defineProperty(window, "__TAURI_INTERNALS__", {
      configurable: true,
      value: {},
    });
  });
  await page.route(
    "https://api.github.com/repos/theatrus/toposaic/releases/latest",
    async (route) => await route.abort(),
  );
  await page.route(
    "https://updates.toposaic.com/notice.json",
    async (route) => {
      await route.fulfill({
        contentType: "application/json",
        body: JSON.stringify({
          schema_version: 1,
          version: siteVersion,
          release_url:
            "https://github.com/theatrus/toposaic/releases/tag/" +
            `v${siteVersion}`,
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
  const line = notice.locator("small");
  await expect(line).toHaveText(`Current v${appVersion}`);
  // Nothing to expand, so nothing to hover.
  await expect(line).not.toHaveAttribute("title", /./);
});
