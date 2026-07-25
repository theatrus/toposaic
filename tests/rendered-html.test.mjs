import assert from "node:assert/strict";
import { access, readFile } from "node:fs/promises";
import test from "node:test";

const projectRoot = new URL("../", import.meta.url);

async function render() {
  const workerUrl = new URL("../dist/server/index.js", import.meta.url);
  workerUrl.searchParams.set("test", `${process.pid}-${Date.now()}`);
  const { default: worker } = await import(workerUrl.href);

  return worker.fetch(
    new Request("http://localhost/", {
      headers: { accept: "text/html" },
    }),
    {
      ASSETS: {
        fetch: async () => new Response("Not found", { status: 404 }),
      },
    },
    {
      waitUntil() {},
      passThroughOnException() {},
    },
  );
}

test("server-renders TopoSaic", async () => {
  const appConfig = JSON.parse(
    await readFile(
      new URL("../src-tauri/tauri.conf.json", import.meta.url),
      "utf8",
    ),
  );
  const response = await render();
  assert.equal(response.status, 200);
  assert.match(response.headers.get("content-type") ?? "", /^text\/html\b/i);

  const html = await response.text();
  assert.match(html, /<title>TopoSaic — Terrain Puzzle<\/title>/i);
  assert.match(html, />TopoSaic</);
  assert.match(
    html,
    new RegExp(
      `Terrain Puzzle ·.*v${appConfig.version.replaceAll(".", "\\.")}`,
      "s",
    ),
  );
  assert.match(html, /Shape your terrain/);
  assert.match(html, /role="tablist"/);
  assert.match(html, />Model</);
  assert.match(html, />Surface</);
  assert.match(html, />Buildings</);
  assert.match(html, />Tray</);
  assert.match(html, />Output</);
  assert.match(html, /id="terrain-controls"/);
  assert.match(html, /Solid terrain/);
  assert.match(html, /Straight piece sides/);
  assert.match(html, /Interlocking tabs/);
  assert.match(html, /tab-less pieces with plain cuts/);
  assert.match(html, /Shallow terrain tray/);
  assert.match(html, /Map center/);
  assert.match(html, /Place name/);
  assert.match(html, /Selected terrain area: 18 km square/);
  assert.match(html, /Mapped buildings/);
  assert.match(html, /Building color/);
  assert.match(html, /Render roads/);
  assert.match(html, /OpenStreetMap waterways/);
  assert.match(html, /Maximum waterway coverage/);
  assert.match(html, /major waterways only/);
  assert.match(html, /Terrain class borders/);
  assert.match(html, /Blocky \(native 10 m data\)/);
  assert.match(html, /Smoothed borders/);
  assert.match(html, /3MF style/);
  assert.match(html, /Color project · filament colors and purge settings/);
  assert.match(html, /Painted colors · no embedded presets/);
  assert.match(html, /Geometry only · plain colors/);
  // The server renders the default style selected: the embedded-settings
  // project output existing users already get.
  assert.match(html, /<option value="project" selected="">/);
  assert.match(html, /OrcaSlicer and Bambu Studio import the file as a project/);
  // The border smoothing sliders only render once "Smoothed borders" is
  // selected; the server renders the blocky default without them.
  assert.doesNotMatch(html, /Border bend range/);
  assert.doesNotMatch(html, /Border noise damping/);
  assert.match(html, /Keep forest off steep rock/);
  assert.match(html, /demotes forest to rock above the slope limit/);
  assert.match(html, /Forest slope limit/);
  // The demotion target renders with the slope gate, which defaults on.
  assert.match(html, /Steep forest becomes/);
  assert.match(html, /Snow above the snowline/);
  assert.match(html, /Keep snow off sheer faces/);
  assert.match(html, /demotes snow\s+to rock above the slope limit/);
  // The snow slope limit renders with its own gate, which defaults on.
  assert.match(html, /Snow slope limit/);
  assert.match(html, /Imported trails/);
  assert.match(html, /Import GPX or KML files/);
  assert.match(html, /aria-label="Import trail files"/);
  assert.match(html, /saved setups and exported setup files carry\s+them/);
  // Trail color and width controls only render once a trail is imported.
  assert.doesNotMatch(html, /Trail print width/);
  assert.doesNotMatch(html, /Trail color/);
  assert.match(html, /Route detail/);
  assert.match(html, /Automatic for map span/);
  assert.match(html, /Streets, paths, and trails/);
  assert.match(html, /Thin dense road networks/);
  assert.match(html, /does not remove road classes/);
  assert.match(html, /Mesh detail/);
  assert.match(html, /Standard/);
  assert.match(html, /Ultra/);
  assert.match(html, /Road layer height/);
  assert.match(html, /Bridge structure/);
  assert.match(html, /Floating bridge thickness/);
  assert.match(html, /Fully supported/);
  assert.match(
    html,
    /Tagged bridges can use thick floating decks or solid support/,
  );
  assert.match(html, /#B8A890/i);
  assert.match(html, /class="setup-menu-button"/);
  assert.match(html, /Saved setups/);
  assert.match(html, /aria-haspopup="menu"/);
  assert.match(html, /aria-label="Import setups file"/);
  assert.doesNotMatch(html, /Local engine/);
  assert.doesNotMatch(html, /None saved yet/);
  assert.match(html, /Resize map and preview panes/);
  assert.doesNotMatch(html, /codex-preview|Your site is taking shape/i);
});

test("removes starter-only files and metadata", async () => {
  const [page, layout, packageJson] = await Promise.all([
    readFile(new URL("../app/page.tsx", import.meta.url), "utf8"),
    readFile(new URL("../app/layout.tsx", import.meta.url), "utf8"),
    readFile(new URL("../package.json", import.meta.url), "utf8"),
  ]);

  assert.match(page, /TerrainStudio/);
  assert.match(layout, /TopoSaic — Terrain Puzzle/);
  assert.doesNotMatch(packageJson, /react-loading-skeleton|drizzle/);
  await assert.rejects(access(new URL("../app/_sites-preview", projectRoot)));
});
