import assert from "node:assert/strict";
import test from "node:test";
import {
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
