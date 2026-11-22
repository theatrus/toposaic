# Globe mode implementation plan

Globe mode makes a complete printable globe from numbered, flat-backed terrain tiles. The tiles snap onto a generated internal frame. The same construction supports **Whole Earth** and **Small World**, with either a rounded or faceted exterior.

## Run the mechanical prototype

In Studio, open **Model → Globe**. Choose the three-tile fit test or full globe, select a rounded or faceted exterior, and adjust the seam clearance. The preview provides assembled, frame-only, exploded, and single-tile views. **Generate** builds a ZIP with the individual STLs and 3MFs, assembly guide, and assembled OBJ. The plain fit test starts at a 200 mm reference diameter.

Choose **Small World** as the globe source to use the elevation from the map selection. Zoom to set the selection size, then set how far the corners wrap and how tall the relief can print. Elevation fades near the selection edges into the rest of the globe. Small World uses the same back, frame, and snap hardware as the plain prototype, and its job can export an elevation source bundle.

## Print a mapped globe

- **Whole Earth** uses bundled NOAA ETOPO 2022 ice-surface elevations and a print palette derived from Natural Earth I. New Whole Earth setups use a quarter-degree grid and 96 mesh samples per edge. The half-degree map remains available for earlier print sets. **Detailed Earth elevation** downloads and caches five-arc-minute NOAA blocks for the selected tile batch, with a transition to the bundled data very close to the poles. [Data and rebuild instructions](../assets/globe/README.md)
- **Small World → Full wrap** stretches the map selection around the globe. Elevation and color blend across the longitude join and converge at the poles. Small World uses the configured regional elevation source and ESA WorldCover for mapped forest, open vegetation, bare ground, water, and snow or ice.
- **Mapped surface colors** applies one editable five-color filament palette across every 3MF and every batch made with the same settings. **Tactile texture** changes the geometry in both STL and 3MF. Rock, forest, grass, and snow use different patterns and amplitudes. Water stays smooth away from shores.

## Print a superglobe in batches

Set the globe diameter and the shorter usable side of the print bed. Automatic subdivision picks the lowest frequency that fits while allowing space for elevation. You can also choose a subdivision explicitly. The maximum frequency is 24, giving up to 11,520 numbered tiles. Feasible sizes depend on the bed and the room needed for the screws and clips on each seat.

For a large globe, choose **Numbered tile batch**, select the first tile and how many tiles to print, and set the terrain mesh detail. Use the whole-globe layout preview to see the selected batch, then generate a range-named ZIP. **Next batch** advances by the current batch size. Keep the source, dimensions, subdivision, fit, mesh detail, and colors identical across batches. Changing the batch only changes the exported tile range. Each tile has its own frame module, and the lower-numbered tile owns the bracket joining a pair of modules. This rule avoids missing or duplicate brackets at batch boundaries. Terrain and frame geometry are built only for the requested batch, while the overview shows the rest of the nominal layout. The live preview retains full detail for a 20-tile globe and reduces the preview mesh for larger batches as needed. Export uses the selected mesh detail.

Use **Frame and snap fit** to set frame depth, clip interference, and locator clearance. The seam control adjusts the gap between tiles. The assembly guide lists the screw length for the chosen frame. The three-tile fixture uses the chosen layout and fit settings, so it can test the same hardware before a larger print. The current frame design and its adjustable fit still need physical validation.

## Command-line fit test

Generate the default three-tile fixture with the Rust example.

```sh
cargo run -p toposaic-core --example globe_prototype -- target/globe-fixture
```

Use an empty output directory. The command generates tiles 02, 05, and 01, their frame modules, two joint brackets, and one snap clip model to print three times. The STLs and 3MFs are in print orientation. `ASSEMBLY.md` gives the hardware and install order, and `assembly.obj` shows the assembled parts. The current brackets use low-profile M3 screws and nuts to lock the frame modules at the generated angle.

The fixture uses a 200 mm reference globe with a plain exterior for checking fit. Pass `--rounded` for curved tiles, `--gap MM` to adjust the combined inset at the tile backs, or `--full` to generate the 20-tile mechanical shell. Digital checks cover mesh closure, print orientation, and the tile insertion axis. The physical print establishes the clip fit and clearances.

## Model options

| Option | Behavior |
| --- | --- |
| Whole Earth | Use global elevation and map colors, with one sea-level reference and a relief setting in millimeters. Ocean surfaces stay at sea level. Bathymetry is a follow-up option. |
| Small World | Use the area selected on the existing map. Start with a regional globe that centers the selection on the sphere, lets the user set how far it wraps, and blends to a smooth neutral surface outside it. Show the extent and transition in the preview. Give the region one height frame and a relief control in millimeters. |
| Small World full wrap | Map the selected region around the entire globe. Blend elevation and color where the east and west edges meet and where the north and south boundaries converge at the poles. Inspect the blended result in the assembled preview. |
| Rounded | Place the reference surface on a sphere. The printer still gets a planar back on each tile. |
| Faceted | Place the reference surface on the triangular faces of an icosahedron or its geodesic subdivision, with relief applied over each face. |

Subdivision frequencies 1 and 2 give 20 and 80 tiles. Higher frequencies provide smaller parts for a given globe diameter. A rounded globe can have a smooth exterior with 20 tiles. The assembly manifest records printable bounds for every exported part. [Icosahedral subdivision](https://journals.ametsoc.org/view/journals/mwre/130/2/1520-0493_2002_130_0319_togihg_2.0.co_2.xml)

## Tiles and snap-on frame

1. Define the globe layout once. Give every face a stable ID, neighbor list, and assembled pose. Sample each shared surface edge once. Use the same globe-center directions for adjacent tiles and apply relief along those directions, including on the faceted globe.
2. Build each tile as a closed solid with a terrain surface, side walls, planar back, and recessed snap sockets. Derive the back planes and frame seats from the same inner reference geometry. Reserve enough material under the lowest terrain for the sockets. Account for intentional seam clearance after computing the shared terrain edges.
3. Generate an open rib lattice with one numbered seat per tile. Split it into keyed, bed-sized modules. Assemble the complete frame before installing the tiles so all the frame joints are accessible. Use three pads at each seat to set the tile height and tilt. A keyed locator sets orientation and resists sideways motion. Replaceable spring clips on the frame engage the sockets and hold the tile against the pads.
4. Seat the tiles from the outside. Check that a tile clears its neighbors over the full insertion and removal paths. The final tile should snap into a fully surrounded seat. Use a controlled pull-off fit so a damaged tile or clip can be replaced.

Tie frame fit to the globe diameter, subdivision, and fit settings. Derive the attachment geometry independently of the terrain, source, and mesh detail. Validate relief and protrusions against the reserved thickness and assembly clearances so changing terrain can reuse the same frame. Keep tile IDs in the assembly guide and print-file names. Add matching molded or engraved IDs in a later hardware revision.

## Elevation, color, and surface texture

- **Whole Earth.** NOAA supplies ice-surface elevation, and the Natural Earth raster supplies the print classes. The sampler wraps longitude, and both polar caps converge to one elevation and class mix each. The ocean mask keeps deep ocean terrain at sea level. [Data and rebuild instructions](../assets/globe/README.md)
- **Small World.** Reuse the configured regional elevation source and ESA WorldCover. A cap has a smooth transition at its outer edges. Full wrap applies the same join and pole transitions to heights and class mixes.
- **Color and texture.** Top triangles use one editable palette across all 3MFs and batches. Sample and blend nearby cover cells before assigning printed classes and texture amplitudes. Tactile patterns use assembled globe coordinates so neighboring tiles share the pattern phase. [Existing color output plan](color-output-plan.md)

## Studio, jobs, and exports

- Add Globe to the shared studio with its own versioned settings. Expose source, diameter, exterior, subdivision, relief, and fit controls. Show assembled, frame-only, exploded, and individual-tile views. The assembled preview should use the same attachment poses as export.
- Put globe topology, seats, and watertight mesh generation in `toposaic-core`. Dispatch to the globe path before loading terrain in `toposaic-api`. Reuse cancellable jobs, the cache, saved setups, and output handling. Record the resolved source versions, palette, layout, and fit settings so a replacement tile reproduces the original interfaces. Extend cache and source-bundle handling for the globe data. [Repository architecture](architecture.md)
- Export bed-oriented terrain tiles, frame modules, and clips as separate print parts. Provide individual bed-sized 3MFs with consistent filament assignments, individual STLs, and a numbered assembly guide. Keep assembled poses in the manifest and let the user regenerate a single tile by ID.

## Build and verification milestones

- [ ] **Three-tile snap-fit print.** Generate a fixture with a real frame module joint and install the middle tile last. Measure seam gaps, seating repeatability, insertion and removal force, and clip wear after repeated removal. Choose the clip shape, clearance, and socket depth from the print results.
- [ ] **Complete 20-tile faceted globe.** Print and assemble the monochrome 200 mm shell and frame. Confirm that the frame closes, the final tile seats with all neighbors installed, and a removed tile can be replaced. Check each exported solid for manifold geometry, minimum thickness, and a flat print base. Check the sliced overhangs and socket bridges.
- [ ] **Rounded finish, Whole Earth color, and 80-tile layout.** Verify continuous elevation and class boundaries at a coastline crossing a tile seam, the antimeridian, and both poles. Check that every part fits the selected bed and that 3MF imports retain consistent filament slots across print groups. Physically test at least one higher-frequency seat and its frame joint before committing to a full production print.
- [ ] **Small World regional mapping, then full wrap.** Print a coastal mountain selection with both a coastline and a ridge crossing tile seams. Verify the fade outside the regional selection, the full-wrap join and pole transitions, and that tiles from a different source can reuse the same frame when the layout and fit settings match. Reprint a single tile from a saved setup and verify its fit.
- [ ] **Tactile texture trial.** Slice and print a sample with adjoining material classes and a seam crossing them. Compare the result with the plain relief and use the print to set texture amplitudes and printable feature sizes.
