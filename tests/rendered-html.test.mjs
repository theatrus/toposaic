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
  assert.match(html, />Colors</);
  assert.match(html, />Buildings</);
  assert.match(html, />Markers</);
  assert.match(html, />Mounting</);
  assert.match(html, />Output</);
  assert.match(html, /id="terrain-controls"/);
  assert.match(html, /Solid terrain/);
  assert.match(html, /Straight piece sides/);
  assert.match(html, /Interlocking tabs/);
  assert.match(html, /tab-less pieces with plain cuts/);
  assert.match(html, /Display base/);
  assert.match(html, /Map position/);
  assert.match(html, /Advanced puzzle identity/);
  assert.match(html, /Place name/);
  assert.match(html, /Selected terrain area: 18 km square/);
  assert.match(html, /Mapped buildings/);
  assert.match(html, /Enable mapped buildings/);
  // All color targets render in one tab, even when their layers are off.
  assert.match(html, /Print colors/);
  assert.match(html, /Building color/);
  assert.match(html, /Render roads/);
  // Railways and aerial lifts default on with their style pickers
  // showing, since each layer switches apart from roads and from the
  // other.
  assert.match(html, /Render railways/);
  assert.match(html, /trains, trams, metros, narrow gauge/);
  assert.match(html, /Tunnels are skipped/);
  assert.match(html, /Railway style/);
  assert.match(html, /Render aerial lifts/);
  assert.match(html, /Aerial lift style/);
  assert.match(html, /cable cars, gondolas, chair/);
  // Funiculars run on the ground, so the copy sends people to railways.
  assert.match(html, /Funiculars run on the\s+ground/);
  assert.match(html, /Draw with roads/);
  assert.match(html, /Draw with railways/);
  assert.match(html, /Own color/);
  // Both layers default to their own color — picking them out is the point
  // of drawing them — so the server renders both selects on "separate" and
  // neither folded into another layer.
  assert.equal(
    (html.match(/<option value="separate" selected="">/g) ?? []).length,
    2,
  );
  assert.doesNotMatch(html, /<option value="with_roads" selected="">/);
  assert.doesNotMatch(html, /<option value="with_rail" selected="">/);
  // Both colors render in the Colors tab, and both width sliders render in
  // Surface.
  assert.match(html, /Railway color/);
  assert.match(html, /Railway minimum width/);
  assert.match(html, /Aerial lift color/);
  assert.match(html, /Aerial lift print width/);
  assert.match(html, /#C43D3D/i);
  assert.match(html, /#6C4CB6/i);
  // A layer in its own color only spends a filament where the mapped data
  // actually holds those features.
  assert.match(html, /uses a filament slot only where the map has a\s+railway/);
  assert.match(html, /slot only where mapped lifts exist/);
  // One lifecycle setting serves both layers, and renders with them.
  assert.match(html, /Railway and lift history/);
  assert.match(html, /In service only/);
  assert.match(html, /track and cables remain/);
  assert.match(html, /formation visible/);
  assert.match(html, /<option value="operational" selected="">/);
  assert.match(html, /Razed, dismantled, demolished/);
  assert.match(html, /OpenStreetMap waterways/);
  assert.match(html, /Maximum waterway coverage/);
  assert.match(html, /major waterways only/);
  assert.match(html, /Terrain class borders/);
  assert.match(html, /Smoothed where 10 m cells show/);
  assert.match(html, /Blocky · raw 10 m cells/);
  // Smoothing is the default, and it gates itself by scale.
  assert.match(html, /<option value="smooth" selected="">/);
  assert.match(html, /It starts at close views/);
  assert.match(html, /3MF style/);
  assert.match(
    html,
    /Color project · filament colors and purge settings/,
  );
  assert.match(html, /Painted colors \(for Orca\) · paint only, no presets/);
  assert.match(html, /Geometry only · standard 3MF colors/);
  // The server renders the default style selected: the embedded-settings
  // project output existing users already get.
  assert.match(html, /<option value="project" selected="">/);
  assert.match(html, /carries its colors for both slicers/);
  // The border smoothing sliders render with the smoothed default.
  assert.match(html, /Border bend range/);
  assert.match(html, /Border noise damping/);
  assert.match(html, /Keep forest off steep rock/);
  assert.match(html, /Demotes forest to rock above the slope limit/);
  assert.match(html, /Forest slope limit/);
  // The demotion target renders with the slope gate, which defaults on.
  assert.match(html, /Steep forest becomes/);
  assert.match(html, /Snow above the snowline/);
  assert.match(html, /Keep snow off sheer faces/);
  assert.match(html, /Demotes snow to rock above the slope limit/);
  // The snow slope limit renders with its own gate, which defaults on.
  assert.match(html, /Snow slope limit/);
  assert.match(html, /Imported trails/);
  assert.match(html, /Import GPX or KML files/);
  assert.match(html, /aria-label="Import trail files"/);
  assert.match(html, /Saved setups carry them/);
  // Trail width renders after an import. Its saved color stays in Colors.
  assert.doesNotMatch(html, /Trail print width/);
  assert.match(html, /Imported trail color/);
  assert.match(html, /Trail color/);
  assert.match(html, /Route detail/);
  assert.match(html, /Automatic for map span/);
  assert.match(html, /Streets, paths, and trails/);
  assert.match(html, /Thin estimated widths in dense networks/);
  assert.match(html, /Mapped physical widths stay at real scale/);
  assert.match(html, /Mesh detail/);
  assert.match(html, /Standard/);
  assert.match(html, /Ultra/);
  assert.match(html, /Road layer height/);
  assert.match(html, /Bridge structure/);
  assert.match(html, /Floating bridge thickness/);
  assert.match(html, /Fully supported/);
  assert.match(html, /Uses a thick deck between the abutments/);
  assert.match(html, /Fills from the deck down to the mapped ground or water/);
  assert.match(html, /#B8A890/i);
  assert.match(html, /class="setup-menu-button"/);
  assert.match(html, /Saved setups/);
  assert.match(html, /aria-haspopup="menu"/);
  assert.match(html, /aria-label="Import setups file"/);
  // The settings gear renders on the server; its pane stays closed, so no
  // cache contents appear until it opens in the browser.
  assert.match(html, /aria-label="Settings"/);
  assert.match(html, /class="settings-button"/);
  assert.doesNotMatch(html, /Map data cache/);
  // The map advertises its keyboard panning and can take focus.
  assert.match(
    html,
    /aria-keyshortcuts="ArrowUp ArrowDown ArrowLeft ArrowRight"/,
  );
  assert.match(html, /Arrow keys pan/);
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
