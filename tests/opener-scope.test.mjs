import assert from "node:assert/strict";
import { readdir, readFile } from "node:fs/promises";
import test from "node:test";

// The desktop build opens external links through the Tauri opener plugin,
// which refuses any URL outside the scope in the capability file. Granting
// `opener:allow-open-url` on its own enables the command with an EMPTY
// scope, and the plugin's check ends in `allowed.iter().any(..)` — false for
// every URL. That is what made the update notice's links dead in the desktop
// app while working perfectly in the browser, where no scope applies.
//
// So the scope and the app's own idea of a safe release URL have to agree,
// and neither file mentions the other. This test is the link between them.

async function openerScope() {
  const capability = JSON.parse(
    await readFile(
      new URL("../src-tauri/capabilities/default.json", import.meta.url),
      "utf8",
    ),
  );
  const entry = capability.permissions.find(
    (permission) =>
      typeof permission === "object" &&
      permission.identifier === "opener:allow-open-url",
  );
  assert.ok(
    entry,
    "opener:allow-open-url must carry a scope; the bare string grants the command with none, which forbids every URL",
  );
  return entry.allow.map((rule) => rule.url);
}

/**
 * Models the plugin's matcher, which is `glob::Pattern` at its defaults —
 * `require_literal_separator: false`, so `*` crosses `/` and one pattern
 * covers a nested download path, and `case_sensitive: true`. Checked against
 * the crate rather than assumed, because a `*` that stopped at `/` would
 * quietly leave every release asset outside the scope.
 */
function scopeAllows(patterns, url) {
  return patterns.some((pattern) => {
    const expression = pattern
      .split("*")
      .map((part) => part.replace(/[.+?^${}()|[\]\\]/g, "\\$&"))
      .join(".*");
    return new RegExp(`^${expression}$`).test(url);
  });
}

test("the opener scope covers every release URL the app will try to open", async () => {
  const patterns = await openerScope();

  // Exactly what `safeReleaseUrl` in app/updates/releases.ts admits, plus
  // the constant the notice falls back to when a feed carries no URL.
  for (const url of [
    "https://github.com/theatrus/toposaic/releases/latest",
    "https://github.com/theatrus/toposaic/releases/tag/v0.7.0",
    "https://github.com/theatrus/toposaic/releases/download/v0.7.0/TopoSaic-0.7.0-macos-aarch64.dmg",
    "https://toposaic.com/",
    "https://toposaic.com/changelog/",
    // Attribution. OpenStreetMap's and ESA's licences want the terms
    // reachable, so a dead link here is a licence problem rather than an
    // annoyance — and these were dead in the desktop build for the same
    // reason the update notice was.
    "https://www.openstreetmap.org/copyright",
    "https://worldcover2021.esa.int/download",
  ]) {
    assert.ok(
      scopeAllows(patterns, url),
      `the desktop app would refuse to open ${url}`,
    );
  }
});

test("the opener scope does not open the rest of the web", async () => {
  const patterns = await openerScope();

  // A release URL arrives from a notice fetched over the network, so the
  // scope is the last line between a compromised feed and the user's
  // browser. `safeReleaseUrl` screens these out too; the scope must not be
  // the weaker of the two checks.
  for (const url of [
    "https://example.com/",
    "http://toposaic.com/",
    "https://toposaic.com.evil.test/",
    "https://github.com/someone-else/toposaic/releases/latest",
    "https://github.com/theatrus/toposaic/issues/51",
  ]) {
    assert.ok(
      !scopeAllows(patterns, url),
      `the desktop app should not open ${url}`,
    );
  }
});

/** Every .tsx under app/, so a new link cannot hide from these checks. */
async function componentSources(directory = new URL("../app/", import.meta.url)) {
  const found = [];
  for (const entry of await readdir(directory, { withFileTypes: true })) {
    const child = new URL(
      entry.name + (entry.isDirectory() ? "/" : ""),
      directory,
    );
    if (entry.isDirectory()) {
      found.push(...(await componentSources(child)));
    } else if (entry.name.endsWith(".tsx")) {
      found.push({ path: child.pathname, text: await readFile(child, "utf8") });
    }
  }
  return found;
}

// Twice now a link has been added as a plain anchor and been dead in the
// desktop app — the update notice in #51, then the attribution links and the
// elevation source found reviewing its fix. Grepping for them by hand missed
// the one whose href is a conditional rather than a literal, so this looks
// for the shape instead of the string.
test("external links go through ExternalLink, never a plain anchor", async () => {
  const offenders = [];
  for (const { path, text } of await componentSources()) {
    if (path.endsWith("external-link.tsx")) continue;
    // An <a ...> whose attributes reach an http(s) URL, literal or not.
    for (const tag of text.matchAll(/<a\s[^>]*?>/gs)) {
      if (/https?:\/\//.test(tag[0])) {
        offenders.push(`${path.split("/app/")[1]}: ${tag[0].replace(/\s+/g, " ").slice(0, 80)}`);
      }
    }
    // And the conditional form, where the URL sits in the expression body.
    for (const tag of text.matchAll(/<a\s(?:[^<>]|\{[^{}]*\})*?>/gs)) {
      if (/https?:\/\//.test(tag[0]) && !offenders.some((o) => o.includes(tag[0].slice(0, 40)))) {
        offenders.push(`${path.split("/app/")[1]}: ${tag[0].replace(/\s+/g, " ").slice(0, 80)}`);
      }
    }
  }
  assert.deepEqual(
    offenders,
    [],
    "a plain <a> to an external URL does nothing in the desktop app; use ExternalLink",
  );
});

test("every fixed URL the app links to is inside the opener scope", async () => {
  const patterns = await openerScope();
  const missing = [];
  for (const { path, text } of await componentSources()) {
    for (const match of text.matchAll(/"(https:\/\/[^"\s]+)"/g)) {
      const url = match[1];
      if (!scopeAllows(patterns, url)) {
        missing.push(`${path.split("/app/")[1]}: ${url}`);
      }
    }
  }
  assert.deepEqual(
    missing,
    [],
    "the desktop app would refuse to open these; add them to src-tauri/capabilities/default.json",
  );
});
