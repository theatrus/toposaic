import type { GenerationSpec, MapFrame } from "./contracts";

const KILOMETRES_PER_LATITUDE_DEGREE = 110.574;
const KILOMETRES_PER_LONGITUDE_DEGREE = 111.32;
const MINIMUM_LONGITUDE_SCALE = 20;
const MAX_MODEL_LATITUDE = 85;

export type AdjacentDirection = "north" | "south" | "east" | "west";

type GeographicFrame = Pick<
  GenerationSpec,
  "center_lat" | "center_lon" | "ground_span_km"
> &
  Partial<Pick<GenerationSpec, "terrain_rotation_degrees" | "map_frame">>;

export function normalizedMapPoint(
  spec: GeographicFrame,
  latitude: number,
  longitude: number,
) {
  const rotation = canonicalRotation(spec.terrain_rotation_degrees ?? 0);
  const halfLatitude =
    spec.ground_span_km / (2 * KILOMETRES_PER_LATITUDE_DEGREE);
  const scale = longitudeScale(
    spec.map_frame?.origin_lat ?? spec.center_lat,
  );
  const halfLongitude = spec.ground_span_km / (2 * scale);
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
      (unwrappedLongitude - spec.center_lon) * scale;
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

export function coordinateAtNormalizedPoint(
  spec: GeographicFrame,
  u: number,
  v: number,
) {
  const rotation = canonicalRotation(spec.terrain_rotation_degrees ?? 0);
  if (rotation !== 0) {
    return coordinateAtLocalOffset(
      spec.center_lat,
      spec.center_lon,
      (u - 0.5) * spec.ground_span_km,
      (v - 0.5) * spec.ground_span_km,
      rotation,
      spec.map_frame?.origin_lat ?? spec.center_lat,
    );
  }
  const halfLatitude =
    spec.ground_span_km / (2 * KILOMETRES_PER_LATITUDE_DEGREE);
  const halfLongitude = spec.ground_span_km / (2 * longitudeScale(spec.center_lat));
  const south = Math.max(-MAX_MODEL_LATITUDE, spec.center_lat - halfLatitude);
  const north = Math.min(MAX_MODEL_LATITUDE, spec.center_lat + halfLatitude);
  const unwrappedLongitude =
    spec.center_lon - halfLongitude + 2 * halfLongitude * u;
  const longitude = ((unwrappedLongitude + 180) % 360 + 360) % 360 - 180;
  return {
    latitude: south + (north - south) * v,
    longitude,
  };
}

function longitudeScale(latitude: number) {
  return Math.max(
    MINIMUM_LONGITUDE_SCALE,
    KILOMETRES_PER_LONGITUDE_DEGREE *
      Math.abs(Math.cos((latitude * Math.PI) / 180)),
  );
}

export function coordinateAtLocalOffset(
  latitude: number,
  longitude: number,
  localEastKm: number,
  localNorthKm: number,
  rotationDegrees: number,
  referenceLatitude = latitude,
) {
  const angle = (canonicalRotation(rotationDegrees) * Math.PI) / 180;
  const sine = Math.sin(angle);
  const cosine = Math.cos(angle);
  const eastKm = localEastKm * cosine + localNorthKm * sine;
  const northKm = -localEastKm * sine + localNorthKm * cosine;
  const scale = longitudeScale(referenceLatitude);
  return {
    latitude: latitude + northKm / KILOMETRES_PER_LATITUDE_DEGREE,
    longitude:
      (((longitude + eastKm / scale + 180) % 360) + 360) % 360 -
      180,
  };
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
  referenceLatitude = latitude,
) {
  return coordinateAtLocalOffset(
    latitude,
    longitude,
    direction === "east"
      ? groundSpanKm
      : direction === "west"
        ? -groundSpanKm
        : 0,
    direction === "north"
      ? groundSpanKm
      : direction === "south"
        ? -groundSpanKm
        : 0,
    rotationDegrees,
    referenceLatitude,
  );
}

export function mapFrameForSpec(
  spec: Pick<
    GenerationSpec,
    | "center_lat"
    | "center_lon"
    | "puzzle_tile_column"
    | "puzzle_tile_row"
    | "map_frame"
  >,
): MapFrame {
  return (
    spec.map_frame ?? {
      origin_lat: spec.center_lat,
      origin_lon: spec.center_lon,
      origin_tile_column: spec.puzzle_tile_column,
      origin_tile_row: spec.puzzle_tile_row,
    }
  );
}

export function matchingTileCenter(
  spec: Pick<
    GenerationSpec,
    | "center_lat"
    | "center_lon"
    | "ground_span_km"
    | "terrain_rotation_degrees"
    | "puzzle_tile_column"
    | "puzzle_tile_row"
    | "map_frame"
  >,
  tileColumn: number,
  tileRow: number,
) {
  const frame = mapFrameForSpec(spec);
  return coordinateAtLocalOffset(
    frame.origin_lat,
    frame.origin_lon,
    (tileColumn - frame.origin_tile_column) * spec.ground_span_km,
    -(tileRow - frame.origin_tile_row) * spec.ground_span_km,
    spec.terrain_rotation_degrees,
    frame.origin_lat,
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
  referenceLatitude = latitude,
) {
  const rowAnchor = anchor === "center" ? (rows - 1) / 2 : 0;
  const columnAnchor = anchor === "center" ? (columns - 1) / 2 : 0;
  return coordinateAtLocalOffset(
    latitude,
    longitude,
    (column - columnAnchor) * groundSpanKm,
    -(row - rowAnchor) * groundSpanKm,
    rotationDegrees,
    referenceLatitude,
  );
}

export function superTileCorners(
  spec: Pick<
    GenerationSpec,
    | "center_lat"
    | "center_lon"
    | "ground_span_km"
    | "terrain_rotation_degrees"
    | "adjacent_rows"
    | "adjacent_columns"
    | "super_tile_anchor"
    | "map_frame"
  >,
  row: number,
  column: number,
) {
  const rowAnchor =
    spec.super_tile_anchor === "center" ? (spec.adjacent_rows - 1) / 2 : 0;
  const columnAnchor =
    spec.super_tile_anchor === "center"
      ? (spec.adjacent_columns - 1) / 2
      : 0;
  const centerEast = (column - columnAnchor) * spec.ground_span_km;
  const centerNorth = -(row - rowAnchor) * spec.ground_span_km;
  const half = spec.ground_span_km / 2;
  const referenceLatitude = spec.map_frame?.origin_lat ?? spec.center_lat;
  return [
    [-half, half],
    [half, half],
    [half, -half],
    [-half, -half],
  ].map(([east, north]) =>
    coordinateAtLocalOffset(
      spec.center_lat,
      spec.center_lon,
      centerEast + east,
      centerNorth + north,
      spec.terrain_rotation_degrees,
      referenceLatitude,
    ),
  );
}
