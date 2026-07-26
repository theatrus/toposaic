import assert from "node:assert/strict";
import test from "node:test";
import {
  MAX_TRAIL_FILE_BYTES,
  MAX_TRAIL_POINTS,
  parseTrailFile,
} from "../app/terrain/trails.ts";

const gpxTrack = `<?xml version="1.0" encoding="UTF-8"?>
<gpx xmlns="http://www.topografix.com/GPX/1/1" version="1.1" creator="unit-test">
  <trk>
    <name>Skyline &amp; Panorama</name>
    <trkseg>
      <trkpt lat="46.7852" lon="-121.7355"><ele>1650</ele></trkpt>
      <trkpt lat="46.7871" lon="-121.7332"/>
    </trkseg>
    <trkseg>
      <trkpt lat='46.7893' lon='-121.7301'/>
    </trkseg>
  </trk>
  <rte>
    <name><![CDATA[Paradise Route]]></name>
    <rtept lat="46.7800" lon="-121.7400"/>
    <rtept lat="46.7810" lon="-121.7390"/>
  </rte>
</gpx>`;

test("parses GPX tracks and routes in document order", () => {
  const { trails, downsampled } = parseTrailFile("hike.gpx", gpxTrack);
  assert.equal(downsampled.length, 0);
  assert.equal(trails.length, 2);
  // Both trkseg blocks flatten into one trail, points in document order.
  assert.equal(trails[0].name, "Skyline & Panorama");
  assert.deepEqual(trails[0].points, [
    [46.7852, -121.7355],
    [46.7871, -121.7332],
    [46.7893, -121.7301],
  ]);
  assert.equal(trails[1].name, "Paradise Route");
  assert.equal(trails[1].points.length, 2);
});

test("names unnamed GPX trails from the file name", () => {
  const unnamed = `<gpx><trk><trkseg>
    <trkpt lat="1" lon="2"/><trkpt lat="1.1" lon="2.1"/>
  </trkseg></trk></gpx>`;
  const { trails } = parseTrailFile("Wonderland Loop.gpx", unnamed);
  assert.equal(trails.length, 1);
  assert.equal(trails[0].name, "Wonderland Loop");
});

const kmlLineString = `<?xml version="1.0" encoding="UTF-8"?>
<kml xmlns="http://www.opengis.net/kml/2.2" xmlns:gx="http://www.google.com/kml/ext/2.2">
  <Document>
    <Placemark>
      <name>Ridge Walk</name>
      <LineString>
        <coordinates>
          -121.7355,46.7852,1650
          -121.7332,46.7871
          -121.7301,46.7893,1700
        </coordinates>
      </LineString>
    </Placemark>
    <Placemark>
      <name>Logged Walk</name>
      <gx:Track>
        <when>2026-07-01T10:00:00Z</when>
        <gx:coord>-121.74 46.78 1500</gx:coord>
        <gx:coord>-121.73 46.79 1510</gx:coord>
      </gx:Track>
    </Placemark>
  </Document>
</kml>`;

test("parses KML LineStrings and gx:Track logs as lat/lon", () => {
  const { trails } = parseTrailFile("routes.kml", kmlLineString);
  assert.equal(trails.length, 2);
  assert.equal(trails[0].name, "Ridge Walk");
  // KML coordinates are lon,lat[,alt]; trails store [lat, lon].
  assert.deepEqual(trails[0].points[0], [46.7852, -121.7355]);
  assert.deepEqual(trails[0].points[1], [46.7871, -121.7332]);
  assert.equal(trails[1].name, "Logged Walk");
  assert.deepEqual(trails[1].points, [
    [46.78, -121.74],
    [46.79, -121.73],
  ]);
});

test("downsamples huge tracks to the point cap by uniform stride", () => {
  const points = Array.from(
    { length: MAX_TRAIL_POINTS * 2 },
    (_, index) => `<trkpt lat="${45 + index * 1e-6}" lon="7"/>`,
  ).join("");
  const gpx = `<gpx><trk><name>Long haul</name><trkseg>${points}</trkseg></trk></gpx>`;
  const { trails, downsampled } = parseTrailFile("long.gpx", gpx);
  assert.equal(trails.length, 1);
  assert.equal(trails[0].points.length, MAX_TRAIL_POINTS);
  assert.deepEqual(downsampled, ["Long haul"]);
  // First and last points survive the stride.
  assert.equal(trails[0].points[0][0], 45);
  assert.equal(
    trails[0].points[MAX_TRAIL_POINTS - 1][0],
    45 + (MAX_TRAIL_POINTS * 2 - 1) * 1e-6,
  );
});

test("skips malformed points and drops trails shorter than two points", () => {
  const gpx = `<gpx><trk><trkseg>
    <trkpt lat="not-a-number" lon="7"/>
    <trkpt lat="95" lon="7"/>
    <trkpt lat="45" lon="181"/>
    <trkpt lat="45.5" lon="7.5"/>
  </trkseg></trk></gpx>`;
  const { trails } = parseTrailFile("broken.gpx", gpx);
  // Only one valid point remains, so no trail survives.
  assert.equal(trails.length, 0);
});

test("slices long emoji names by code points, never mid-surrogate", () => {
  // 100 mountain emoji (each an astral code point, two UTF-16 units) trim
  // to exactly 80 whole emoji — a .slice(0, 80) on the string would cut
  // the 41st emoji in half and leave a lone surrogate.
  const name = "🏔".repeat(100);
  const gpx = `<gpx><trk><name>${name}</name><trkseg>
    <trkpt lat="1" lon="2"/><trkpt lat="1.1" lon="2.1"/>
  </trkseg></trk></gpx>`;
  const { trails } = parseTrailFile("emoji.gpx", gpx);
  assert.equal(trails[0].name, "🏔".repeat(80));
  assert.equal(trails[0].name.isWellFormed?.() ?? true, true);
});

test("decodes surrogate and out-of-range character references safely", () => {
  // &#xD800; is a lone surrogate half and &#x110000; is past Unicode;
  // String.fromCodePoint throws on both. They decode to U+FFFD instead.
  const gpx = `<gpx><trk><name>A&#xD800;B&#x110000;C</name><trkseg>
    <trkpt lat="1" lon="2"/><trkpt lat="1.1" lon="2.1"/>
  </trkseg></trk></gpx>`;
  const { trails } = parseTrailFile("weird.gpx", gpx);
  assert.equal(trails[0].name, "A�B�C");
});

test("scrubs control characters from file-name fallback names", () => {
  const unnamed = `<gpx><trk><trkseg>
    <trkpt lat="1" lon="2"/><trkpt lat="1.1" lon="2.1"/>
  </trkseg></trk></gpx>`;
  // Control and format characters strip out, like parsed names.
  const scrubbed = parseTrailFile("Ev\u0007il\u200B walk.gpx", unnamed);
  assert.equal(scrubbed.trails[0].name, "Evil walk");
  // A name that is nothing but control characters falls back.
  const empty = parseTrailFile("\u0001\u0002.gpx", unnamed);
  assert.equal(empty.trails[0].name, "Imported trail");
});

test("ignores point tags hidden inside CDATA sections", () => {
  // CDATA is character data, never markup: the fake trkpt in the
  // description must not become a trail point, while the CDATA name
  // still decodes as text.
  const gpx = `<gpx><trk>
    <name><![CDATA[Real & Trail]]></name>
    <desc><![CDATA[<trkpt lat="0" lon="0"/><trkpt lat="0.1" lon="0.1"/>]]></desc>
    <trkseg>
      <trkpt lat="46.1" lon="7.1"/>
      <trkpt lat="46.2" lon="7.2"/>
    </trkseg>
  </trk></gpx>`;
  const { trails } = parseTrailFile("cdata.gpx", gpx);
  assert.equal(trails.length, 1);
  assert.equal(trails[0].name, "Real & Trail");
  assert.deepEqual(trails[0].points, [
    [46.1, 7.1],
    [46.2, 7.2],
  ]);
});

test("refuses trail files past the 32 MB import limit", () => {
  const huge = "x".repeat(MAX_TRAIL_FILE_BYTES + 1);
  assert.throws(
    () => parseTrailFile("huge.gpx", huge),
    /32 MB trail import limit/,
  );
});

test("keeps the route name when rtept points carry their own names", () => {
  // Only text before the first point can name the trail; rtept-level
  // <name> children must not shadow it — or name an unnamed route.
  const gpx = `<gpx>
    <rte>
      <name>Summit Route</name>
      <rtept lat="46.1" lon="7.1"><name>Waypoint 1</name></rtept>
      <rtept lat="46.2" lon="7.2"><name>Waypoint 2</name></rtept>
    </rte>
    <rte>
      <rtept lat="47.1" lon="8.1"><name>Sneaky waypoint</name></rtept>
      <rtept lat="47.2" lon="8.2"/>
    </rte>
  </gpx>`;
  const { trails } = parseTrailFile("Routes.gpx", gpx);
  assert.equal(trails.length, 2);
  assert.equal(trails[0].name, "Summit Route");
  // The unnamed route falls back to the file name, not a point name.
  assert.equal(trails[1].name, "Routes 2");
});

test("returns nothing for files with no recognizable trails", () => {
  assert.deepEqual(parseTrailFile("note.gpx", "just some text"), {
    trails: [],
    downsampled: [],
  });
  assert.deepEqual(
    parseTrailFile("empty.kml", "<kml><Document/></kml>").trails,
    [],
  );
  // Unknown extensions try GPX first, then KML.
  const { trails } = parseTrailFile("export.xml", kmlLineString);
  assert.equal(trails.length, 2);
});
