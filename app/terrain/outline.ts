import type { GenerationSpec } from "./contracts";
import { coordinateAtLocalOffset } from "./geo";

export function normalizedOutlinePoints(
  spec: Pick<
    GenerationSpec,
    "model_outline" | "width_mm" | "rows" | "columns"
  >,
  samples = 96,
): [number, number][] {
  const shape = spec.model_outline.shape;
  if (shape === "polygon" && spec.model_outline.points.length >= 3) {
    return spec.model_outline.points;
  }
  if (shape === "rectangle") {
    return [
      [0, 0],
      [1, 0],
      [1, 1],
      [0, 1],
    ];
  }
  const heightMm = (spec.width_mm * spec.rows) / spec.columns;
  const aspect = heightMm / spec.width_mm;
  const radiusX = shape === "circle" ? Math.min(0.5, aspect * 0.5) : 0.5;
  const radiusY =
    shape === "circle" ? Math.min(0.5, 0.5 / aspect) : 0.5;
  const count = Math.max(64, samples);
  return Array.from({ length: count }, (_, index) => {
    const angle = -Math.PI / 2 + (index / count) * Math.PI * 2;
    return [
      0.5 + radiusX * Math.cos(angle),
      0.5 + radiusY * Math.sin(angle),
    ];
  });
}

export function geographicOutlinePoints(
  spec: Pick<
    GenerationSpec,
    | "model_outline"
    | "width_mm"
    | "rows"
    | "columns"
    | "center_lat"
    | "center_lon"
    | "ground_span_km"
    | "terrain_rotation_degrees"
    | "map_frame"
  >,
) {
  const referenceLatitude = spec.map_frame?.origin_lat ?? spec.center_lat;
  return normalizedOutlinePoints(spec).map(([u, v]) =>
    coordinateAtLocalOffset(
      spec.center_lat,
      spec.center_lon,
      (u - 0.5) * spec.ground_span_km,
      (v - 0.5) * spec.ground_span_km,
      spec.terrain_rotation_degrees,
      referenceLatitude,
    ),
  );
}

export function pointInNormalizedOutline(
  point: [number, number],
  outline: [number, number][],
) {
  let inside = false;
  for (let current = 0, previous = outline.length - 1; current < outline.length; previous = current++) {
    const [x, y] = outline[current];
    const [previousX, previousY] = outline[previous];
    if (
      (y > point[1]) !== (previousY > point[1]) &&
      point[0] <
        ((previousX - x) * (point[1] - y)) /
          (previousY - y || Number.EPSILON) +
          x
    ) {
      inside = !inside;
    }
  }
  return inside;
}
