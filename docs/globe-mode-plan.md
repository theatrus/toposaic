# Globe mode implementation plan

Globe mode makes a complete printable globe from numbered, flat-backed terrain tiles. The tiles snap onto a generated internal frame. The same construction supports **Whole Earth** and **Small World**, with either a rounded or faceted exterior.

## Run the mechanical prototype

In Studio, open **Model → Globe**. Choose the full globe or a numbered tile batch, select a rounded or faceted exterior, and adjust the seam clearance. The preview provides assembled, frame-only, exploded, and single-tile views. **Generate** builds a ZIP with the individual STLs and 3MFs, assembly guide, and assembled OBJ. The plain globe starts at a 200 mm reference diameter. To test the fit, print a small batch of neighbouring tiles.

Choose **Small World** as the globe source to wrap the map selection onto the globe. Studio keeps the map, place search, and the Surface, Buildings, Markers, and Colors sections, so roads, rail, trails, water, buildings, markers, and filament colors work as they do on a flat model. Zoom to set the selection size, then choose how the map covers the globe and how tall the relief can print. Small World uses the same back, frame, and snap hardware as the plain prototype, and its job can export a source bundle.

- **Regional with smooth back** centers the selection on the globe. The wrap angle sets how far its corners reach. The terrain fades into plain ground at the selection edges.
- **360° band with polar caps** wraps the map once around the globe. Its middle strip, outlined on the map, forms a band of ±72°, ±60°, ±45°, or ±30° latitude. The east and west edges blend where they meet, and the band fades into plain caps at both poles.

## Print a mapped globe

- **Whole Earth** uses bundled NOAA ETOPO 2022 ice-surface elevations and a print palette derived from Natural Earth I. New Whole Earth setups use a quarter-degree grid and 96 mesh samples per edge. The half-degree map remains available for earlier print sets. **Detailed Earth elevation** downloads and caches five-arc-minute NOAA blocks for the selected tile batch, with a transition to the bundled data very close to the poles. [Data and rebuild instructions](../assets/globe/README.md)
- **Small World** loads the same elevation, map, water, and ground-color data as a flat model of the selection. Its colors and filament palette come from the Colors section.
- **Mapped surface colors** applies one editable five-color filament palette across every 3MF and every batch made with the same settings. **Tactile texture** changes the geometry in both STL and 3MF. Rock, forest, grass, and snow use different patterns and amplitudes. Water stays smooth away from shores.

## Add oversized landmarks

In **Whole Earth → Landmarks**, add the Eiffel Tower, Great Pyramid of Giza, and Sydney examples, or make a custom landmark from latitude and longitude. Choose a stylized tower, pyramid, or city marker and set its printed width and figure height in millimeters. The inserts stay readable when the diameter or tile subdivision changes. [Eiffel Tower site](https://www.wikidata.org/wiki/Q243) · [Great Pyramid survey](https://www.giza-pyramids.com/documents/pdf/GPP_K-TheGPSLocationOfTheGreatPyramid.pdf) · [Sydney city centre](https://www.wikidata.org/wiki/Q1852577)

The owning tile has a shallow, keyed socket. If the requested position is too close to a tile seam or a frame attachment, Studio looks for room on the same tile within 14 mm and shows both coordinates and the displacement. A white dot marks a displaced landmark's requested site in the 3D preview. Choose a landmark in the preview's **View** menu to inspect it on its tile. If there is no room, the landmark is listed in the guide so you can adjust the site or width.

Print each numbered insert separately in the filament of your choice. See `LANDMARKS.md` in the kit for its tile, orientation, and coordinates. A landmark STL and 3MF are included with the tile that has its socket. Keep the same landmark settings across batches so you can reprint that tile and its insert together. The keyed peg clearance still needs a physical print test.

## Print a superglobe in batches

Set the globe diameter and the shorter usable side of the print bed. Automatic subdivision picks the lowest frequency that fits while allowing space for elevation. You can also choose a subdivision explicitly. The maximum frequency is 24, giving up to 11,520 numbered tiles. Feasible sizes depend on the bed and the room needed for the screws and clips on each seat.

One job holds a limited number of tiles, set by the source and mesh detail, so memory stays bounded. Mapped tiles carry far more geometry than plain ones. When the full globe is over the limit, the preview shows the whole layout and asks for a batch. For a large globe, choose **Numbered tile batch**, select the first tile and how many tiles to print, and set the terrain mesh detail. Use the whole-globe layout preview to see the selected batch, then generate a range-named ZIP. **Next batch** advances by the current batch size. Keep the source, dimensions, subdivision, fit, mesh detail, and colors identical across batches. Changing the batch only changes the exported tile range. Each tile has its own frame module, and the lower-numbered tile owns the bracket joining a pair of modules. This rule avoids missing or duplicate brackets at batch boundaries. Terrain and frame geometry are built only for the requested batch, while the overview shows the rest of the nominal layout. The live preview retains full detail for a 20-tile globe and reduces the preview mesh for larger batches as needed. Export uses the selected mesh detail.

Use **Frame and snap fit** to set frame depth, clip interference, locator clearance, and dovetail key clearance. The seam control adjusts the gap between tiles.

Neighbouring frame modules lock together with printed dovetail keys, one per joint. A key drops into keyhole slots in the tops of both modules and slides along their shared edge under the dovetail lips. All of this happens from outside, so the last module goes in the same way as the first. Two options add screws, both into heat-set inserts with a configurable thread, hole diameter, and length:

- **Screw brackets** add a bracket under each joint. Its inserts take screws driven down from outside through recessed holes in the frame modules.
- **Tile screws** add an insert hole in each tile back and a counterbored hole in its frame module. Each tile is screwed from inside as it goes on. The last few tiles cannot be reached, so their clips hold them.

The assembly guide lists the inserts, screws, and screw lengths for the chosen frame. A small batch uses the chosen layout and fit settings, so it can test the same hardware before a larger print. The current frame design and its adjustable fit still need physical validation.

## Command-line prototype

Generate the plain 20-tile mechanical shell with the Rust example.

```sh
cargo run -p toposaic-core --example globe_prototype -- target/globe-shell
```

Use an empty output directory. The command writes every tile, frame module, and dovetail key, plus one snap clip model to print once per tile. The STLs and 3MFs are in print orientation. `ASSEMBLY.md` gives the install order, and `assembly.obj` shows the assembled parts.

The shell uses a 200 mm reference globe with a plain exterior. Pass `--batch FIRST COUNT` to write a numbered batch, `--rounded` for curved tiles, or `--gap MM` to adjust the combined inset at the tile backs. Digital checks cover mesh closure, print orientation, the tile insertion axis, and that each installed key sits inside both of its slots. A physical print establishes the clip and key fit and clearances.

## Model options

| Option | Behavior |
| --- | --- |
| Whole Earth | Use global elevation and map colors, with one sea-level reference and a relief setting in millimeters. Ocean surfaces stay at sea level. Bathymetry is a follow-up option. |
| Small World regional | Center the selected area on the sphere with an azimuthal equidistant projection, so distances from the center stay true. The user sets how far the corners wrap. Outside the selection the globe is plain ground. |
| Small World 360° band | Wrap the map once around the globe with an equirectangular projection. The band's height is a share of the square map, so the land keeps its proportions at the equator. Blend the east and west edges, and fade the band into plain polar caps. |
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
- **Small World.** Build the flat terrain model of the selection at the globe's scale, so a millimetre on the map is a millimetre of arc at the map center. Each tile traces its seam-inset outline onto the map, and the terrain piece builder fills that outline with terrain, roads, bridges, buildings, markers, and colors. The piece then bends onto the tile: the map's zero-relief surface lands on the sphere or facet, relief rises along globe-center directions, and the piece's base becomes the tile back with its clip socket. A few tiles cannot unfold onto the map: those holding a pole of the band, those across the band's east-west join, and those around the far side of a regional map. They sample heights and colors from the same faded map, without roads or buildings.
- **Color and texture.** Top triangles use one editable palette across all 3MFs and batches. Sample and blend nearby cover cells before assigning printed classes and texture amplitudes. Tactile patterns use assembled globe coordinates so neighboring tiles share the pattern phase. [Existing color output plan](color-output-plan.md)

## Studio, jobs, and exports

- Add Globe to the shared studio with its own versioned settings. Expose source, diameter, exterior, subdivision, relief, and fit controls. Show assembled, frame-only, exploded, and individual-tile views. The assembled preview should use the same attachment poses as export.
- Put globe topology, seats, and watertight mesh generation in `toposaic-core`. Dispatch to the globe path before loading terrain in `toposaic-api`. Reuse cancellable jobs, the cache, saved setups, and output handling. Record the resolved source versions, palette, layout, and fit settings so a replacement tile reproduces the original interfaces. Extend cache and source-bundle handling for the globe data. [Repository architecture](architecture.md)
- Export bed-oriented terrain tiles, frame modules, and clips as separate print parts. Provide individual bed-sized 3MFs with consistent filament assignments, individual STLs, and a numbered assembly guide. Keep assembled poses in the manifest and let the user regenerate a single tile by ID.

## Build and verification milestones

- [ ] **Small-batch snap-fit print.** Print a batch of neighbouring tiles with their frame modules and keys, and install the middle tile last. Measure seam gaps, key fit and slide force, seating repeatability, insertion and removal force, and clip wear after repeated removal. Choose the clip shape, key clearance, and socket depth from the print results. Repeat with screw brackets and tile screws to check the insert holes.
- [ ] **Complete 20-tile faceted globe.** Print and assemble the monochrome 200 mm shell and frame. Confirm that the frame closes, the final tile seats with all neighbors installed, and a removed tile can be replaced. Check each exported solid for manifold geometry, minimum thickness, and a flat print base. Check the sliced overhangs and socket bridges.
- [ ] **Rounded finish, Whole Earth color, and 80-tile layout.** Verify continuous elevation and class boundaries at a coastline crossing a tile seam, the antimeridian, and both poles. Check that every part fits the selected bed and that 3MF imports retain consistent filament slots across print groups. Physically test at least one higher-frequency seat and its frame joint before committing to a full production print.
- [ ] **Small World regional mapping, then the 360° band.** Print a coastal mountain selection with a coastline, a ridge, and a road crossing tile seams. Verify the fade outside the regional selection, the band's east-west join and polar caps, and that tiles from a different source can reuse the same frame when the layout and fit settings match. Reprint a single tile from a saved setup and verify its fit.
- [ ] **Small World detail across the band.** The band reads its data from the square map around it, so it loads more area than it prints. Measure load time on large spans and decide whether the map data should follow the band's own shape.
- [ ] **Tactile texture trial.** Slice and print a sample with adjoining material classes and a seam crossing them. Compare the result with the plain relief and use the print to set texture amplitudes and printable feature sizes.
