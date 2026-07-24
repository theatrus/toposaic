# TopoSaic

*Terrain Puzzle*

The name TopoSaic is a portmanteau of *topographic mosaic*.

TopoSaic is a local-first topographic puzzle generator. A Rust service
samples worldwide elevation data, builds watertight pieces with round jigsaw
tabs and sockets, and stores job state in SQLite. The web app lets you choose a
place and tune the printable model, including the mesh detail and surface
colors.

## Download

The current desktop release is
[TopoSaic v0.2.0](https://github.com/theatrus/toposaic/releases/tag/v0.2.0).

| Platform | Downloads |
| --- | --- |
| Windows x64 | [Setup `.exe`](https://github.com/theatrus/toposaic/releases/download/v0.2.0/TopoSaic-0.2.0-windows-x64.exe) · [`.msi` installer](https://github.com/theatrus/toposaic/releases/download/v0.2.0/TopoSaic-0.2.0-windows-x64.msi) |
| macOS Apple silicon | [`.dmg` disk image](https://github.com/theatrus/toposaic/releases/download/v0.2.0/TopoSaic-0.2.0-macos-aarch64.dmg) · [`.app.zip` archive](https://github.com/theatrus/toposaic/releases/download/v0.2.0/TopoSaic-0.2.0-macos-aarch64.app.zip) |
| Linux x86-64 | [Portable `.AppImage`](https://github.com/theatrus/toposaic/releases/download/v0.2.0/TopoSaic-0.2.0-linux-x86_64.AppImage) |

macOS releases use a Developer ID signature, Apple notarization, and stapled
tickets. Windows installers use the GUI subsystem and do not open a terminal
window. On Linux, make the AppImage executable with `chmod +x` before opening
it. See [all releases](https://github.com/theatrus/toposaic/releases) for older
builds.

## Screenshots

![TopoSaic light workspace showing the Mount Rainier map, interactive 3D terrain puzzle preview, and model controls](docs/images/toposaic-studio.png)

*Choose a place beside a live, rotatable preview. The lower workspace starts
larger and can be resized with the horizontal divider.*

![TopoSaic model controls showing compact model choices and a four-by-three super-tile grid](docs/images/toposaic-model-controls.png)

*Move by one tile or export a straight 4×3 mosaic under one shared height
frame. Optional tabs join both terrain tiles and matching tray sections.*

![TopoSaic light workspace showing terrain colors, mapped water and road controls, and bridge structure choices](docs/images/toposaic-surface-controls.png)

*Tune print colors, mapped water, road width and height, dense-network thinning,
and floating or supported bridges in place.*

![A generated Mount Rainier color 3MF opened in Bambu Studio](docs/images/toposaic-3mf-bambu-studio.jpg)

*The exported 3MF keeps the puzzle pieces and their forest, rock, snow, water,
road, and building materials ready for a color print.*

Version 0.2 adds signed update notices, Mapterhorn elevation tiles, resizable
map and preview space, synchronized map zoom and ground span, adjacent `N×M`
super-tile exports, shared height frames, and matching split trays.

An optional shallow tray exports as its own watertight STL and color 3MF. Its
flat well shows smooth, continuous equal-height contour lines as fine color
inlays. Raised text on the front top lip shows the chosen place name, latitude,
and longitude in smooth vector letterforms. Controls set the tray clearance,
rim, floor, line count, and three print colors. The bundled Atkinson
Hyperlegible font keeps the label shape the same on every OS and remains under
its included SIL Open Font License.

Solid terrain mode exports the same mapped relief as one watertight STL and 3MF
model with a straight outer edge and no puzzle seams. It keeps the full source
sampling grid while limiting the single mesh to a safe detail level.

Piece layouts range from 2×2 to 16×16. The default 10×10 layout makes 100
pieces with narrow-necked, round puzzle knobs like a standard jigsaw.
The model controls also set the minimum solid thickness under the lowest
terrain point.

## Super-tile mode

Super-tile mode makes terrain sets larger than one printer's build plate. The
map draws the full grid before export. The chosen point can mark the top-left
tile or the center tile; center anchoring uses odd row and column counts so one
real tile stays at the chosen point. A grid can contain up to 12×12 print
passes. Each terrain tile gets its own color 3MF, while every tile uses the same
elevation datum and vertical scale. Straight tile bounds keep the grid aligned.
Optional external tabs and sockets join shared edges, and the full set keeps a
flat outside border.

The tray follows the same grid. TopoSaic makes one outer frame, then splits it
into matching printable tray parts. Joined inner edges have no walls. Each part
exports as its own STL and color 3MF, with optional matching tabs and sockets.
The separate-trays option instead gives each terrain tile its own complete
framed tray.

North, south, east, and west buttons move the selection by one full tile. The
first move locks the elevation datum and vertical scale, so the same real
elevation prints at the same Z height on each tile. If a later tile drops below
that datum, TopoSaic warns that the shared datum must move down and that earlier
tiles must be regenerated.

The elevation provider reads Mapzen Terrarium tiles by default. A Mapterhorn
option uses 512 px WebP Terrarium tiles with regional elevation data up to zoom
17 and falls back to lower-zoom Mapterhorn tiles outside that coverage. The
service caches elevation, ESA WorldCover, and OpenStreetMap input
under the operating system's user cache directory. OpenStreetMap entries keep
the raw response, so width, density, color, and visibility changes reuse the
same download.

For uncached requests, the service tries a second public Overpass instance when
the first rejects or cannot serve the request. If both fail, generation
continues without that OSM layer. WorldCover water and terrain output remain
available. Concurrent jobs share each cache fill, and the service tries the last
working public instance first on its next request. It retries a failed fetch
once and rejects HTTP 200 responses that contain an Overpass timeout remark, so
it never caches a partial building set. Set `OVERPASS_BASE_URL` to use one
specific Overpass instance.

Color mode reads 10 m ESA WorldCover 2021 data through HTTP range requests. It
maps tree cover, bare ground, snow or ice, and permanent water to editable
forest, rock, snow, and water colors. It also reads prominent roads from
OpenStreetMap through Overpass, then draws motorway, trunk, primary, and
secondary roads as smooth, print-safe vector lines. If none cross the selected
area, it draws paths, footways, bridleways, tracks, and cycleways as a trail
fallback. Rivers, streams, canals, and mapped water areas use the same vector
path so they stay smooth and flush with the terrain. Building footprints keep
their straight mapped edges, with dense local mesh detail along each wall
instead of a blocky whole-map sampling edge. The 3MF stores standard triangle
color properties.
Roads also rise by one configurable print-layer height, which defaults to 0.2
mm. Road width starts at 0.7 mm and can thin automatically in dense road
networks. Roads tagged as bridges in OpenStreetMap interpolate a deck between
their DEM-height abutments instead of dropping into the ravine or water below.
Untagged roads still follow the terrain, and `layer=*` is not treated as a
height. OpenStreetMap water can be disabled without hiding WorldCover water.
The waterway coverage cutoff always keeps rivers and canals, then keeps the
longest streams until their estimated printed area reaches the chosen share of
the model. Set it to 0% for major waterways only or 100% for every mapped
stream. Mapped water areas do not use this cutoff.
STL files stay single-color but retain the raised road geometry.

Overlay detail is separate from the base terrain setting. It defaults to 112
samples per piece and can rise to 192, giving roads, buildings, water, snow,
forest, and rock boundaries a finer mesh without forcing the same setting on
plain terrain jobs. Generated browser previews use up to 384 samples across the
assembled map.

Building mode reads OpenStreetMap footprints and raises them above the terrain.
It uses tagged height first, then floor count, then an 8 m default. Its own Z
scale controls vertical exaggeration against the map's plan scale. Buildings
can run with or without surface color output. In color output, roofs and walls
use their own editable building material instead of inheriting the land-cover
color beneath each footprint.

Place search uses explicit, user-submitted OpenStreetMap Nominatim queries
through the Rust service. Results are cached in SQLite and outbound requests
are limited to one per second. Set `NOMINATIM_BASE_URL` to use another
compatible service. Review the
[public service policy](https://operations.osmfoundation.org/policies/nominatim/)
before wider or commercial use.

The preview asks for a 64×64 real elevation sample after the location or ground
span has been still for 450 ms. This gives the relief pane useful terrain before
a full mesh job starts. It uses the same tile cache as generation. A completed
job replaces it with the detailed generated preview. The preview is a lit 3D
height mesh: drag or use the arrow keys to orbit, and scroll, pinch, or use the
plus and minus keys to zoom.

Mesh generation uses Rayon to build separate puzzle pieces and their STL files
in parallel. It keeps 3MF archive writes, downloads, cache writes, and SQLite
work in order. No more than eight piece meshes stay in memory at once. Set
`RAYON_NUM_THREADS` to cap CPU use. A repeatable release-mode mesh check is:

```bash
cargo run --release -p terrain-core --example profile_generation -- 6 6 96
```

## Requirements

- Rust 1.96 or newer
- Node.js 22.13 or newer
- Windows 10 or 11 for the Windows desktop bundle
- A 64-bit Linux system for the Linux AppImage

## Run

Start the Rust API:

```bash
cargo run -p terrain-api
```

In a second terminal, start the website:

```bash
npm install
npm run dev
```

Open `http://127.0.0.1:3100`. The Rust API listens on
`http://127.0.0.1:8787`.

### Desktop app

The Tauri app uses the same React controls and starts the Rust engine inside the
app process, so it does not need a second terminal:

```bash
npm install
npm run tauri dev
```

Build the desktop app with:

```bash
npm run tauri build
```

The desktop app keeps SQLite and generated jobs in its standard application
data directory. Downloaded map inputs still use the shared OS cache described
below. Each generated file opens a native Save As dialog, so the app does not
drop files into Downloads without asking.

The header shows the installed app version. On launch, desktop builds compare
the latest stable GitHub release with the release notice at `toposaic.com`.
They show the newest valid notice and ignore malformed or older responses. A
matching signed update can be installed in the app; otherwise the notice links
to the normal release download. The checks send no project or location data.

Tagged releases provide five desktop files: Windows `.msi` and `.exe`
installers, macOS `.app.zip` and `.dmg` bundles, and a Linux `.AppImage`. They
also provide signed Tauri update payloads and the public `updater.json` and
`notice.json` feeds. The tag must match the version in
`src-tauri/tauri.conf.json`.

On Linux, make the downloaded AppImage executable before opening it:

```bash
chmod +x TopoSaic-*-linux-x86_64.AppImage
./TopoSaic-*-linux-x86_64.AppImage
```

Windows builds use the Universal CRT that Windows 10 and 11 include and service.
CI checks each executable's DLL imports and fails if it adds a `VCRUNTIME`,
`MSVCP`, or `CONCRT` dependency that would need a Visual C++ Redistributable
install. It also checks that release executables use the Windows GUI subsystem,
so the app does not open a console window. The installers download Microsoft's
WebView2 bootstrapper only when the system does not already have WebView2.

## Storage

SQLite and generated jobs live under `data/`, which Git ignores. Set
`TERRAIN_DATA_DIR` to use another directory.

Downloaded map inputs use the standard per-user cache path:

- macOS: `~/Library/Caches/com.theatrus.toposaic`
- Linux: `$XDG_CACHE_HOME/toposaic` or `~/.cache/toposaic`
- Windows: `%LOCALAPPDATA%\theatrus\toposaic\cache`

Set `TERRAIN_CACHE_DIR` to override that path. The cache keeps Mapzen elevation
PNGs, Mapterhorn elevation WebPs, full ESA WorldCover GeoTIFF tiles, and
OpenStreetMap route responses. Writes use a temporary file and an atomic rename,
so a stopped download does not leave a valid-looking partial tile.

The browser uses `NEXT_PUBLIC_TERRAIN_API_URL` when set. See `.env.example`.

## Check

```bash
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
npm test
npm run test:ui
```

## Project shape

- `crates/terrain-core`: puzzle edges, terrain surface, watertight meshes,
  binary STL, and standards-based 3MF
- `apps/api`: global elevation provider, Axum API, SQLite jobs, background
  generation, ESA WorldCover sampling, and downloads
- `app`: WebGL-free map, color relief preview, print controls, and job downloads
- `desktop` and `src-tauri`: shared React entry point and native Tauri shell

See [the color output plan](docs/color-output-plan.md) for the design and print
checks behind the rock–forest–snow–water–road 3MF workflow.

## Terrain data

Mapzen Terrain Tiles combine several regional and global public elevation
sources:

<https://github.com/tilezen/joerd/blob/master/docs/attribution.md>

Mapterhorn provides a 30 m global layer and higher-detail regional sources. Its
tiles and source-specific credits are listed here:

<https://mapterhorn.com/data-access/>

<https://mapterhorn.com/attribution/>

Generated manifests record the selected source, requested and used zooms,
fallback policy, and attribution link.

Color manifests also record the ESA WorldCover tile and attribution:

<https://esa-worldcover.org/en/data-access>

When OpenStreetMap overlays are on, manifests also record their source and
attribution. Overpass responses use the same OS cache:

<https://www.openstreetmap.org/copyright>

Publicly shared prints, images, and generated files must retain the data-source
credits recorded in their manifest or place those credits near the work. See
[third-party licenses and data](THIRD_PARTY_NOTICES.md).

## License

TopoSaic source code and documentation are licensed under the
[Apache License 2.0](LICENSE). Third-party software, the bundled font, and map
data keep their own licenses; see [THIRD_PARTY_NOTICES.md](THIRD_PARTY_NOTICES.md)
and [assets/fonts/OFL.txt](assets/fonts/OFL.txt).
