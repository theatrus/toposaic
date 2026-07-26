import type { TrailRoute } from "./contracts";

// Mirror the caps in crates/toposaic-core/src/spec.rs so the client never
// submits a spec the backend rejects.
export const MAX_TRAILS = 20;
export const MAX_TRAIL_POINTS = 20000;
const MAX_TRAIL_NAME_CHARS = 80;
/** Refuse trail files past this size; real GPX/KML logs stay far under it. */
export const MAX_TRAIL_FILE_BYTES = 32 * 1024 * 1024;

export type ParsedTrailFile = {
  trails: TrailRoute[];
  /** Names of trails thinned to the point cap by uniform stride. */
  downsampled: string[];
};

// The extraction below is a small, dependency-free XML scan instead of a
// DOMParser walk so the exact production code also runs under `node --test`
// (Node has no DOMParser and the project takes no new dependencies). It
// handles the subset real GPX/KML files use: namespace prefixes, either
// attribute quote style, XML comments, CDATA sections, and entities.
// The Playwright suite exercises the same code in a real browser.

function stripComments(text: string) {
  return text.replace(/<!--[\s\S]*?-->/g, "");
}

// CDATA is character data, never markup, so a `<trkpt>` inside a CDATA
// section must not reach the tag scan. Escaping the section's specials
// turns it into plain entity-encoded text: names in CDATA still decode
// correctly through decodeXmlText, and fake tags stay inert. Comments are
// stripped BEFORE this runs (see parseTrailFile) so a comment cannot hide
// or splice a CDATA opener; keep that order.
function escapeCdataSections(text: string) {
  return text.replace(/<!\[CDATA\[([\s\S]*?)\]\]>/g, (_, inner: string) =>
    inner
      .replace(/&/g, "&amp;")
      .replace(/</g, "&lt;")
      .replace(/>/g, "&gt;"),
  );
}

// Surrogate halves (0xD800-0xDFFF) and codes past 0x10FFFF are not XML
// characters; String.fromCodePoint would throw on them. Decode to the
// replacement character instead, the way browsers render bad references.
function decodedCodePoint(code: number) {
  if (
    !Number.isInteger(code) ||
    code > 0x10ffff ||
    (code >= 0xd800 && code <= 0xdfff)
  ) {
    return "�";
  }
  return String.fromCodePoint(code);
}

function decodeXmlText(value: string) {
  return value
    .replace(/&#x([0-9a-fA-F]+);/g, (_, hex: string) =>
      decodedCodePoint(Number.parseInt(hex, 16)),
    )
    .replace(/&#([0-9]+);/g, (_, decimal: string) =>
      decodedCodePoint(Number.parseInt(decimal, 10)),
    )
    .replace(/&quot;/g, '"')
    .replace(/&apos;/g, "'")
    .replace(/&lt;/g, "<")
    .replace(/&gt;/g, ">")
    .replace(/&amp;/g, "&");
}

/** Inner content of every `<tag>...</tag>` block, prefix-agnostic. */
function innerBlocks(text: string, tag: string) {
  const pattern = new RegExp(
    `<(?:[A-Za-z0-9_.-]+:)?${tag}(?:\\s[^>]*)?>([\\s\\S]*?)</(?:[A-Za-z0-9_.-]+:)?${tag}\\s*>`,
    "g",
  );
  const blocks: string[] = [];
  for (const match of text.matchAll(pattern)) {
    blocks.push(match[1]);
  }
  return blocks;
}

/** The attribute strings of every `<tag ...>` or `<tag ... />` in order. */
function openTagAttributes(text: string, tag: string) {
  const pattern = new RegExp(
    `<(?:[A-Za-z0-9_.-]+:)?${tag}((?:\\s[^>]*)?)/?>`,
    "g",
  );
  const attributes: string[] = [];
  for (const match of text.matchAll(pattern)) {
    attributes.push(match[1]);
  }
  return attributes;
}

function attributeValue(attributes: string, name: string) {
  const match = new RegExp(
    `(?:^|\\s)${name}\\s*=\\s*(?:"([^"]*)"|'([^']*)')`,
  ).exec(attributes);
  return match ? (match[1] ?? match[2]) : undefined;
}

function validPoint(latitude: number, longitude: number) {
  return (
    Number.isFinite(latitude) &&
    Number.isFinite(longitude) &&
    Math.abs(latitude) <= 90 &&
    Math.abs(longitude) <= 180
  );
}

/** Strips control and format characters; every trail name passes through. */
function scrubName(name: string) {
  return name.replace(/[\p{Cc}\p{Cf}]/gu, "").trim();
}

/** The text of `text` before the first `<tag …>`, or all of it. */
function textBeforeFirstTag(text: string, tag: string) {
  const match = new RegExp(`<(?:[A-Za-z0-9_.-]+:)?${tag}[\\s/>]`).exec(text);
  return match ? text.slice(0, match.index) : text;
}

function blockName(block: string, pointTag?: string) {
  // Only text before the first point can name the trail; points such as
  // rtept carry their own <name> children, which must not shadow it.
  const scope = pointTag ? textBeforeFirstTag(block, pointTag) : block;
  const name = innerBlocks(scope, "name")[0];
  if (!name) return undefined;
  const decoded = scrubName(decodeXmlText(name));
  return decoded === "" ? undefined : decoded;
}

function trimName(name: string) {
  // Slice by code points, never inside a surrogate pair.
  const codePoints = Array.from(name);
  return codePoints.length > MAX_TRAIL_NAME_CHARS
    ? codePoints.slice(0, MAX_TRAIL_NAME_CHARS).join("").trimEnd()
    : name;
}

type RawTrail = { name: string | undefined; points: [number, number][] };

function gpxTrails(text: string): RawTrail[] {
  const trails: RawTrail[] = [];
  for (const [container, pointTag] of [
    ["trk", "trkpt"],
    ["rte", "rtept"],
  ] as const) {
    for (const block of innerBlocks(text, container)) {
      const points: [number, number][] = [];
      // trkpt order inside every trkseg follows document order, so one
      // scan flattens a multi-segment track into one trail.
      for (const attributes of openTagAttributes(block, pointTag)) {
        const latitude = Number.parseFloat(
          attributeValue(attributes, "lat") ?? "",
        );
        const longitude = Number.parseFloat(
          attributeValue(attributes, "lon") ?? "",
        );
        if (validPoint(latitude, longitude)) {
          points.push([latitude, longitude]);
        }
      }
      trails.push({ name: blockName(block, pointTag), points });
    }
  }
  return trails;
}

/** Parses "lon,lat[,alt]" tuples separated by whitespace. */
function kmlCoordinatePoints(text: string) {
  const points: [number, number][] = [];
  for (const tuple of decodeXmlText(text).trim().split(/\s+/)) {
    const [longitude, latitude] = tuple.split(",").map(Number.parseFloat);
    if (validPoint(latitude, longitude)) {
      points.push([latitude, longitude]);
    }
  }
  return points;
}

function kmlTrails(text: string): RawTrail[] {
  const trails: RawTrail[] = [];
  const placemarks = innerBlocks(text, "Placemark");
  // LineStrings outside any Placemark still parse (rare hand-made files).
  const scopes = placemarks.length > 0 ? placemarks : [text];
  for (const scope of scopes) {
    const name = blockName(scope);
    for (const lineString of innerBlocks(scope, "LineString")) {
      const coordinates = innerBlocks(lineString, "coordinates")[0];
      if (coordinates === undefined) continue;
      trails.push({ name, points: kmlCoordinatePoints(coordinates) });
    }
    // gx:Track stores one point per <gx:coord> as "lon lat alt".
    for (const track of innerBlocks(scope, "Track")) {
      const points: [number, number][] = [];
      for (const coord of innerBlocks(track, "coord")) {
        const [longitude, latitude] = decodeXmlText(coord)
          .trim()
          .split(/\s+/)
          .map(Number.parseFloat);
        if (validPoint(latitude, longitude)) {
          points.push([latitude, longitude]);
        }
      }
      trails.push({ name, points });
    }
  }
  return trails;
}

function downsample(points: [number, number][]): [number, number][] {
  if (points.length <= MAX_TRAIL_POINTS) return points;
  const last = points.length - 1;
  return Array.from(
    { length: MAX_TRAIL_POINTS },
    (_, index) => points[Math.round((index * last) / (MAX_TRAIL_POINTS - 1))],
  );
}

/**
 * Parses one imported .gpx or .kml file into trails. Every trk, rte,
 * LineString, and gx:Track becomes one trail, named from its <name> or the
 * file name. Tracks longer than the point cap are thinned by uniform
 * stride and reported in `downsampled`.
 */
export function parseTrailFile(fileName: string, text: string): ParsedTrailFile {
  if (text.length > MAX_TRAIL_FILE_BYTES) {
    throw new Error(
      `${fileName} is larger than the 32 MB trail import limit.`,
    );
  }
  // Order matters: strip comments first so a comment cannot hide or splice
  // a CDATA opener, then neutralize CDATA so tags inside it stay inert.
  const clean = escapeCdataSections(stripComments(text));
  const extension = fileName.toLowerCase().split(".").pop();
  let raw: RawTrail[];
  if (extension === "gpx") {
    raw = gpxTrails(clean);
  } else if (extension === "kml") {
    raw = kmlTrails(clean);
  } else {
    raw = gpxTrails(clean);
    if (raw.length === 0) raw = kmlTrails(clean);
  }
  // File names pass through the same scrub as parsed names: strip control
  // and format characters, trim, and never end up empty.
  const fallbackName =
    scrubName(fileName.replace(/\.[^.]+$/, "")) || "Imported trail";
  const usable = raw.filter((trail) => trail.points.length >= 2);
  const downsampled: string[] = [];
  const trails = usable.map((trail, index) => {
    const name = trimName(
      trail.name ??
        (usable.length > 1 ? `${fallbackName} ${index + 1}` : fallbackName),
    );
    const points = downsample(trail.points);
    if (points.length < trail.points.length) {
      downsampled.push(name);
    }
    return { name, points };
  });
  return { trails, downsampled };
}
