# Globe mode implementation plan

Globe mode makes a complete printable globe from numbered, flat-backed terrain tiles. The tiles snap onto a generated internal frame. The same construction supports **Whole Earth** and **Small World**, with either a rounded or faceted exterior.

## Run the mechanical prototype

In Studio, open **Model → Globe**. Choose the three-tile fit test or 20-tile globe, select a rounded or faceted exterior, and adjust the seam clearance. The preview provides assembled, frame-only, exploded, and single-tile views. **Generate** builds a ZIP with the individual STLs and 3MFs, assembly guide, and assembled OBJ. The prototype uses a plain surface and a 200 mm reference diameter.

Choose **Small World** as the globe source to use the elevation from the map selection. Zoom to set the selection size, then set how far the corners wrap and how tall the relief can print. Elevation fades near the selection edges into the rest of the globe. Small World uses the same back, frame, and snap hardware as the plain prototype, and its job can export an elevation source bundle.

For command-line generation, run:

```sh
cargo run -p toposaic-core --example globe_prototype -- target/globe-fixture
```

Use an empty output directory. The command generates tiles 02, 05, and 01, their frame modules, two joint brackets, and one snap clip model to print three times. The STLs and 3MFs are in print orientation. `ASSEMBLY.md` gives the hardware and install order, and `assembly.obj` shows the assembled parts. The current brackets use low-profile M3 screws and nuts to lock the frame modules at the generated angle.

The fixture uses a 200 mm reference globe with a plain exterior for checking fit. Pass `--rounded` for curved tiles, `--gap MM` to adjust the combined inset at the tile backs, or `--full` to generate the 20-tile mechanical shell. Digital checks cover mesh closure, print orientation, and the tile insertion axis. The physical print establishes the clip fit and clearances.

## Model options

| Option | Behavior |
| --- | --- |
| Whole Earth | Use global elevation and land cover, with a single sea-level reference and relief multiplier. Begin with a smooth ocean at sea level. Offer bathymetry once recessed ocean terrain passes the same thickness and fit checks. |
| Small World | Use the area selected on the existing map. Start with a regional globe that centers the selection on the sphere, lets the user set how far it wraps, and blends to a smooth neutral surface outside it. Show the extent and transition in the preview. Give the region one height frame and a relief control in millimeters. |
| Small World full wrap | Map the selected region around the entire globe. Blend elevation and color where the east and west edges meet and where the north and south boundaries converge at the poles. Show those joins and the polar distortion in the preview. |
| Rounded | Place the reference surface on a sphere. The printer still gets a planar back on each tile. |
| Faceted | Place the reference surface on the triangular faces of an icosahedron or its geodesic subdivision, with relief applied over each face. |

Start with 20 and 80 tiles, corresponding to subdivision frequencies 1 and 2. Keep tile count independent of mesh detail. A rounded globe can have a smooth exterior with 20 tiles. Higher frequencies provide smaller parts for a given globe diameter. Start the full-shell prototype at a reference diameter of 200 mm. The preview and export should report actual dimensions, including relief and the largest tile and frame module. [Icosahedral subdivision](https://journals.ametsoc.org/view/journals/mwre/130/2/1520-0493_2002_130_0319_togihg_2.0.co_2.xml)

## Tiles and snap-on frame

1. Define the globe layout once. Give every face a stable ID, neighbor list, and assembled pose. Sample each shared surface edge once. Use the same globe-center directions for adjacent tiles and apply relief along those directions, including on the faceted globe.
2. Build each tile as a closed solid with a terrain surface, side walls, planar back, and recessed snap sockets. Derive the back planes and frame seats from the same inner reference geometry. Reserve enough material under the lowest terrain for the sockets. Account for intentional seam clearance after computing the shared terrain edges.
3. Generate an open rib lattice with one numbered seat per tile. Split it into keyed, bed-sized modules. Assemble the complete frame before installing the tiles so all the frame joints are accessible. Use three pads at each seat to set the tile height and tilt. A keyed locator sets orientation and resists sideways motion. Replaceable spring clips on the frame engage the sockets and hold the tile against the pads.
4. Seat the tiles from the outside. Check that a tile clears its neighbors over the full insertion and removal paths. The final tile should snap into a fully surrounded seat. Use a controlled pull-off fit so a damaged tile or clip can be replaced.

Tie frame fit to the globe diameter, subdivision, and fit settings. Derive the attachment geometry independently of the terrain, source, and mesh detail. Validate relief and protrusions against the reserved thickness and assembly clearances so changing terrain can reuse the same frame. Keep tile IDs visible in the assembly guide and on the inside of printed parts.

## Elevation, color, and surface texture

- **Whole Earth.** Start with NOAA ETOPO 2022 ice-surface elevation, Natural Earth coastline and ocean data, and a pinned NASA MODIS land-cover year. Use a globe sampler that wraps longitude and includes both poles. The current regional elevation reader clamps latitude at about 85°. Treat ocean membership as a separate input from elevation so below-sea-level land stays land. Start testing relief at 10× for the 200 mm globe. [NOAA ETOPO](https://www.ncei.noaa.gov/products/etopo-global-relief-model) · [Natural Earth physical vectors](https://www.naturalearthdata.com/downloads/50m-physical-vectors/) · [NASA MODIS land cover](https://ladsweb.modaps.eosdis.nasa.gov/missions-and-measurements/products/MCD12C1/) · [Regional elevation sampler](../crates/toposaic-api/src/elevation.rs)
- **Small World.** Reuse regional elevation and land-cover loading, with satellite-derived colors where available. Map the geographic selection onto the globe before deciding which details survive at print scale. For full wrap, apply the same transition to elevation and material assignments.
- **Color.** Resolve one editable filament palette per globe. Distinguish ocean and inland water, forest, grassland and scrub, desert and exposed rock, and permanent snow and ice as the filament budget allows. Filter small patches by their printed size on the assembled surface before assigning materials per tile. Check that a coast or land-cover boundary can cross a seam without a visible color jump. [Existing color output plan](color-output-plan.md)
- **Tactile texture.** After the color and fit checks pass, try subtle class-dependent relief with a smooth ocean. Anchor any pattern in globe coordinates and fade it at class transitions where needed. Keep its amplitude subordinate to the terrain relief and verify that it survives slicing without obscuring coastlines or changing the fit.

## Studio, jobs, and exports

- Add Globe to the shared studio with its own versioned settings. Expose source, diameter, exterior, subdivision, relief, and fit controls. Show assembled, frame-only, exploded, and individual-tile views. The assembled preview should use the same attachment poses as export.
- Put globe topology, seats, and watertight mesh generation in `toposaic-core`. Dispatch to the globe path before loading terrain in `toposaic-api`. Reuse cancellable jobs, the cache, saved setups, and output handling. Record the resolved source versions, palette, layout, and fit settings so a replacement tile reproduces the original interfaces. Extend cache and source-bundle handling for the globe data. [Repository architecture](architecture.md)
- Export bed-oriented terrain tiles, frame modules, and clips as separate print parts. Provide bed-sized color 3MF groups with consistent filament assignments, plus individual STLs and a numbered assembly guide. Keep assembled poses in the manifest and let the user regenerate a single tile by ID.

## Build and verification milestones

- [ ] **Three-tile snap-fit print.** Generate a fixture with a real frame module joint and install the middle tile last. Measure seam gaps, seating repeatability, insertion and removal force, and clip wear after repeated removal. Choose the clip shape, clearance, and socket depth from the print results.
- [ ] **Complete 20-tile faceted globe.** Print and assemble the monochrome 200 mm shell and frame. Confirm that the frame closes, the final tile seats with all neighbors installed, and a removed tile can be replaced. Check each exported solid for manifold geometry, minimum thickness, and a flat print base. Check the sliced overhangs and socket bridges.
- [ ] **Rounded finish, Whole Earth color, and 80-tile layout.** Verify continuous elevation and class boundaries at a coastline crossing a tile seam, the antimeridian, and both poles. Check that every part fits the selected bed and that 3MF imports retain consistent filament slots across print groups. Physically test at least one higher-frequency seat and its frame joint before enabling the 80-tile preset.
- [ ] **Small World regional mapping, then full wrap.** Print a coastal mountain selection with both a coastline and a ridge crossing tile seams. Verify the fade outside the regional selection, the full-wrap join and pole transitions, and that tiles from a different source can reuse the same frame when the layout and fit settings match. Reprint a single tile from a saved setup and verify its fit.
- [ ] **Tactile texture trial.** Slice and print a sample with adjoining material classes and a seam crossing them. Compare the result with the plain relief and use the print to set texture amplitudes and printable feature sizes.
