import type { GenerationSpec } from "./contracts";

const KILOMETRES_PER_LATITUDE_DEGREE = 110.574;
const KILOMETRES_PER_LONGITUDE_DEGREE = 111.32;
const MINIMUM_LONGITUDE_SCALE = 20;
const MAX_MODEL_LATITUDE = 85;

export type AdjacentDirection = "north" | "south" | "east" | "west";

type GeographicFrame = Pick<
  GenerationSpec,
  "center_lat" | "center_lon" | "ground_span_km"
> &
  Partial<Pick<GenerationSpec, "terrain_rotation_degrees">>;

export function normalizedMapPoint(
  spec: GeographicFrame,
  latitude: number,
  longitude: number,
) {
  const rotation = canonicalRotation(spec.terrain_rotation_degrees ?? 0);
  const halfLatitude =
    spec.ground_span_km / (2 * KILOMETRES_PER_LATITUDE_DEGREE);
  const longitudeScale = Math.max(
    MINIMUM_LONGITUDE_SCALE,
    KILOMETRES_PER_LONGITUDE_DEGREE *
      Math.abs(Math.cos((spec.center_lat * Math.PI) / 180)),
  );
  const halfLongitude = spec.ground_span_km / (2 * longitudeScale);
  const unwrappedLongitude =
    spec.center_lon +
    ((((longitude - spec.center_lon + 180) % 360) + 360) % 360) -
    180;
  const south = Math.max(
    -MAX_MODEL_LATITUDE,
    spec.center_lat - halfLatitude,
  );
  const north = Math.min(
    MAX_MODEL_LATITUDE,
    spec.center_lat + halfLatitude,
  );
  if (rotation !== 0) {
    const worldEastKm =
      (unwrappedLongitude - spec.center_lon) * longitudeScale;
    const worldNorthKm =
      (latitude - spec.center_lat) * KILOMETRES_PER_LATITUDE_DEGREE;
    const angle = (rotation * Math.PI) / 180;
    const sine = Math.sin(angle);
    const cosine = Math.cos(angle);
    const localEastKm = worldEastKm * cosine - worldNorthKm * sine;
    const localNorthKm = worldEastKm * sine + worldNorthKm * cosine;
    return {
      u: localEastKm / spec.ground_span_km + 0.5,
      v: localNorthKm / spec.ground_span_km + 0.5,
    };
  }
  return {
    u:
      (unwrappedLongitude - (spec.center_lon - halfLongitude)) /
      (2 * halfLongitude),
    v: (latitude - south) / (north - south),
  };
}

function offsetCoordinates(
  latitude: number,
  longitude: number,
  northKm: number,
  eastKm: number,
) {
  const longitudeScale = Math.max(
    MINIMUM_LONGITUDE_SCALE,
    KILOMETRES_PER_LONGITUDE_DEGREE *
      Math.abs(Math.cos((latitude * Math.PI) / 180)),
  );
  return {
    latitude: Math.max(
      -MAX_MODEL_LATITUDE,
      Math.min(
        MAX_MODEL_LATITUDE,
        latitude + northKm / KILOMETRES_PER_LATITUDE_DEGREE,
      ),
    ),
    longitude:
      (((longitude + eastKm / longitudeScale + 180) % 360) + 360) % 360 -
      180,
  };
}

function offsetRotatedCoordinates(
  latitude: number,
  longitude: number,
  localNorthKm: number,
  localEastKm: number,
  rotationDegrees: number,
) {
  const angle = (canonicalRotation(rotationDegrees) * Math.PI) / 180;
  const sine = Math.sin(angle);
  const cosine = Math.cos(angle);
  return offsetCoordinates(
    latitude,
    longitude,
    -localEastKm * sine + localNorthKm * cosine,
    localEastKm * cosine + localNorthKm * sine,
  );
}

function canonicalRotation(rotationDegrees: number) {
  const rotation = ((((rotationDegrees + 180) % 360) + 360) % 360) - 180;
  return Math.abs(rotation) < Number.EPSILON ? 0 : rotation;
}

export function adjacentCenter(
  latitude: number,
  longitude: number,
  groundSpanKm: number,
  direction: AdjacentDirection,
  rotationDegrees = 0,
) {
  return offsetRotatedCoordinates(
    latitude,
    longitude,
    direction === "north"
      ? groundSpanKm
      : direction === "south"
        ? -groundSpanKm
        : 0,
    direction === "east"
      ? groundSpanKm
      : direction === "west"
        ? -groundSpanKm
        : 0,
    rotationDegrees,
  );
}

export function superTileCenter(
  latitude: number,
  longitude: number,
  groundSpanKm: number,
  row: number,
  column: number,
  rows: number,
  columns: number,
  anchor: GenerationSpec["super_tile_anchor"],
  rotationDegrees = 0,
) {
  const rowAnchor = anchor === "center" ? (rows - 1) / 2 : 0;
  const columnAnchor = anchor === "center" ? (columns - 1) / 2 : 0;
  return offsetRotatedCoordinates(
    latitude,
    longitude,
    -(row - rowAnchor) * groundSpanKm,
    (column - columnAnchor) * groundSpanKm,
    rotationDegrees,
  );
}
