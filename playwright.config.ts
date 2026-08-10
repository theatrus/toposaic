import { defineConfig } from "@playwright/test";

export default defineConfig({
  testDir: "./tests/ui",
  fullyParallel: true,
  forbidOnly: Boolean(process.env.CI),
  retries: process.env.CI ? 2 : 0,
  // Playwright's default is 30s per test, which the longer controls.spec
  // cases sit close to: ~20 interactions each carrying an actionability wait,
  // run fullyParallel. On a release build, where the runner is also compiling
  // Rust and Tauri, one of them tipped over and failed all three attempts with
  // "Test timeout of 30000ms exceeded" -- inside uncheck(), but the budget was
  // what ran out, not the locator. It passed on a rerun and passes on pushes
  // to main. Nothing asserts the app is broken, so the honest fix is a
  // realistic budget rather than reshaping a correct test to run faster.
  timeout: 60_000,
  reporter: "line",
  use: {
    baseURL: "http://127.0.0.1:1420",
    headless: true,
  },
  webServer: {
    command:
      "npm run desktop:build && npx vite preview --config vite.desktop.config.ts --host 127.0.0.1 --port 1420",
    url: "http://127.0.0.1:1420",
    reuseExistingServer: !process.env.CI,
    timeout: 120_000,
  },
});
