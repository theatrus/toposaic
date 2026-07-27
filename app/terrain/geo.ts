import type { GenerationSpec } from "./contracts";

const KILOMETRES_PER_LATITUDE_DEGREE = 110.574;
const KILOMETRES_PER_LONGITUDE_DEGREE = 111.32;
const MINIMUM_LONGITUDE_SCALE = 20;
const MAX_MODEL_LATITUDE = 85;

export type AdjacentDirection = "north" | "south" | "east" | "west";

export function normalizedMapPoint(
  spec: Pick<
    GenerationSpec,
    "center_lat" | "center_lon" | "ground_span_km"
  >,
  latitude: number,
  longitude: number,
) {
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

export function adjacentCenter(
  latitude: number,
  longitude: number,
  groundSpanKm: number,
  direction: AdjacentDirection,
) {
  return offsetCoordinates(
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
) {
  const rowAnchor = anchor === "center" ? (rows - 1) / 2 : 0;
  const columnAnchor = anchor === "center" ? (columns - 1) / 2 : 0;
  return offsetCoordinates(
    latitude,
    longitude,
    -(row - rowAnchor) * groundSpanKm,
    (column - columnAnchor) * groundSpanKm,
  );
}
