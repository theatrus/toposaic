"use client";

import {
  type CSSProperties,
  type KeyboardEvent as ReactKeyboardEvent,
  type PointerEvent as ReactPointerEvent,
  useCallback,
  useEffect,
  useMemo,
  useRef,
  useState,
} from "react";

import type { GenerationSpec } from "./contracts";
import { ExternalLink } from "./external-link";
import {
  DEFAULT_MAP_LABEL_STYLE,
  isMapLabel,
  MAX_GROUND_SPAN_KM,
  MIN_GROUND_SPAN_KM,
} from "./config";
import { superTileCorners } from "./geo";

const TILE_SIZE = 256;
const MAX_MERCATOR_LATITUDE = 85.05112878;
const MIN_MAP_ZOOM = 2;
const MAX_MAP_ZOOM = 17;
// Arrow keys pan the focused map by a share of the current ground span.
const KEYBOARD_PAN_SHARE = 0.1;
const KEYBOARD_PAN_SHARE_SHIFT = 0.5;

type MapInteractionMode = "pan" | "move" | "select";

type SelectionDraft = {
  left: number;
  top: number;
  width: number;
  height: number;
  cellSize: number;
};

function selectionDraft(
  startX: number,
  startY: number,
  currentX: number,
  currentY: number,
  columns: number,
  rows: number,
): SelectionDraft {
  const deltaX = currentX - startX;
  const deltaY = currentY - startY;
  const cellSize = Math.max(
    Math.abs(deltaX) / Math.max(1, columns),
    Math.abs(deltaY) / Math.max(1, rows),
  );
  const width = cellSize * Math.max(1, columns);
  const height = cellSize * Math.max(1, rows);
  const endX = startX + (deltaX < 0 ? -width : width);
  const endY = startY + (deltaY < 0 ? -height : height);
  return {
    left: Math.min(startX, endX),
    top: Math.min(startY, endY),
    width,
    height,
    cellSize,
  };
}

function projectToWorld(longitude: number, latitude: number, zoom: number) {
  const scale = TILE_SIZE * 2 ** zoom;
  const clampedLatitude = Math.max(
    -MAX_MERCATOR_LATITUDE,
    Math.min(MAX_MERCATOR_LATITUDE, latitude),
  );
  const sine = Math.sin((clampedLatitude * Math.PI) / 180);
  return {
    x: ((longitude + 180) / 360) * scale,
    y: (0.5 - Math.log((1 + sine) / (1 - sine)) / (4 * Math.PI)) * scale,
  };
}

function unprojectFromWorld(x: number, y: number, zoom: number) {
  const scale = TILE_SIZE * 2 ** zoom;
  const longitude = (((((x / scale) * 360) % 360) + 360) % 360) - 180;
  const mercatorY = Math.PI * (1 - (2 * y) / scale);
  const latitude = (Math.atan(Math.sinh(mercatorY)) * 180) / Math.PI;
  return {
    longitude,
    latitude: Math.max(
      -MAX_MERCATOR_LATITUDE,
      Math.min(MAX_MERCATOR_LATITUDE, latitude),
    ),
  };
}

function metresPerPixelAtLatitude(latitude: number, zoom: number) {
  return (
    (156543.03392 *
      Math.max(0.1, Math.cos((latitude * Math.PI) / 180))) /
    2 ** zoom
  );
}

export function TerrainMap({
  spec,
  markerPlacementMode,
  onPlaceMarker,
  onCenterChange,
  onGroundSpanChange,
  recallCount,
}: {
  spec: GenerationSpec;
  markerPlacementMode: "place" | "move" | null;
  onPlaceMarker: (longitude: number, latitude: number) => void;
  onCenterChange: (longitude: number, latitude: number) => void;
  onGroundSpanChange: (groundSpanKm: number) => void;
  // Bumped each time a saved setup is recalled.
  recallCount: number;
}) {
  const containerRef = useRef<HTMLDivElement>(null);
  const dragRef = useRef<{
    pointerId: number;
    mode: MapInteractionMode | "marker";
    startX: number;
    startY: number;
    localStartX: number;
    localStartY: number;
    worldX: number;
    worldY: number;
  } | null>(null);
  const [zoom, setZoom] = useState(9);
  const [mapOnlyZoom, setMapOnlyZoom] = useState<number | null>(null);
  // The fitted zoom as it stands, for the recall effect below to read
  // without re-running every time the view moves.
  const fittedMapZoomRef = useRef(0);
  const [interactionMode, setInteractionMode] =
    useState<MapInteractionMode>("pan");
  const [viewCenter, setViewCenter] = useState<{
    longitude: number;
    latitude: number;
  } | null>(null);
  const [draft, setDraft] = useState<SelectionDraft | null>(null);
  const [size, setSize] = useState({ width: 0, height: 0 });
  const [tilesLoaded, setTilesLoaded] = useState(false);
  const superTileColumns = Math.max(1, spec.adjacent_columns);
  const superTileRows = Math.max(1, spec.adjacent_rows);
  const superTileActive = superTileColumns > 1 || superTileRows > 1;
  const terrainRotationDegrees = spec.terrain_rotation_degrees;
  const terrainRotationRadians = (terrainRotationDegrees * Math.PI) / 180;
  const rotationSine = Math.sin(terrainRotationRadians);
  const rotationCosine = Math.cos(terrainRotationRadians);

  useEffect(() => {
    const container = containerRef.current;
    if (!container) return;
    const observer = new ResizeObserver(([entry]) => {
      setSize({
        width: Math.round(entry.contentRect.width),
        height: Math.round(entry.contentRect.height),
      });
    });
    observer.observe(container);
    return () => observer.disconnect();
  }, []);

  const superTileGeography = useMemo(() => {
    const cells = [];
    for (let row = 0; row < superTileRows; row += 1) {
      for (let column = 0; column < superTileColumns; column += 1) {
        cells.push({ row, column, corners: superTileCorners(spec, row, column) });
      }
    }
    return cells;
  }, [spec, superTileColumns, superTileRows]);
  const baseAnchorWorld = projectToWorld(spec.center_lon, spec.center_lat, zoom);
  const baseWorldScale = TILE_SIZE * 2 ** zoom;
  const baseFootprintPoints = superTileGeography.flatMap((cell) =>
    cell.corners.map((corner) => {
      const point = projectToWorld(corner.longitude, corner.latitude, zoom);
      let deltaX = point.x - baseAnchorWorld.x;
      if (deltaX > baseWorldScale / 2) deltaX -= baseWorldScale;
      if (deltaX < -baseWorldScale / 2) deltaX += baseWorldScale;
      return { x: baseAnchorWorld.x + deltaX, y: point.y };
    }),
  );
  const baseFootprintBounds = baseFootprintPoints.reduce(
    (bounds, point) => ({
      left: Math.min(bounds.left, point.x),
      right: Math.max(bounds.right, point.x),
      top: Math.min(bounds.top, point.y),
      bottom: Math.max(bounds.bottom, point.y),
    }),
    {
      left: baseAnchorWorld.x,
      right: baseAnchorWorld.x,
      top: baseAnchorWorld.y,
      bottom: baseAnchorWorld.y,
    },
  );
  const projectedFootprintWidth =
    baseFootprintBounds.right - baseFootprintBounds.left;
  const projectedFootprintHeight =
    baseFootprintBounds.bottom - baseFootprintBounds.top;
  const fitScale = Math.max(
    1,
    size.width ? projectedFootprintWidth / (size.width * 0.88) : 1,
    size.height ? projectedFootprintHeight / (size.height * 0.82) : 1,
  );
  const fittedMapZoom = Math.max(
    MIN_MAP_ZOOM,
    zoom - Math.max(0, Math.ceil(Math.log2(fitScale))),
  );
  const mapZoom = mapOnlyZoom ?? fittedMapZoom;
  const zoomLinked = mapOnlyZoom === null;

  // Declared before the recall effect below so the ref it reads already
  // holds this render's value.
  useEffect(() => {
    fittedMapZoomRef.current = fittedMapZoom;
  }, [fittedMapZoom]);

  // A recalled setup names the ground it covers, and linked zoom resizes
  // exactly that: one scroll over the map and the area just recalled is a
  // different area. So recall leaves the zoom unlinked, at the zoom the
  // recalled span fits, and the view stays put until asked to move.
  //
  // Keyed on the recall count alone. The fitted zoom is read through a ref
  // because including it would re-pin the view on every zoom, which is the
  // opposite of leaving it alone.
  useEffect(() => {
    if (recallCount === 0) return;
    setMapOnlyZoom(fittedMapZoomRef.current);
  }, [recallCount]);
  const anchorWorld = useMemo(
    () => projectToWorld(spec.center_lon, spec.center_lat, mapZoom),
    [mapZoom, spec.center_lat, spec.center_lon],
  );
  const superTileCells = useMemo(() => {
    const worldScale = TILE_SIZE * 2 ** mapZoom;
    return superTileGeography.map((cell) => {
      const corners = cell.corners.map((corner) => {
        const projected = projectToWorld(
          corner.longitude,
          corner.latitude,
          mapZoom,
        );
        let deltaX = projected.x - anchorWorld.x;
        if (deltaX > worldScale / 2) deltaX -= worldScale;
        if (deltaX < -worldScale / 2) deltaX += worldScale;
        return { x: anchorWorld.x + deltaX, y: projected.y };
      });
      return {
        ...cell,
        corners,
      };
    });
  }, [
    anchorWorld.x,
    mapZoom,
    superTileGeography,
  ]);
  const selectionWorldCenter = useMemo(() => {
    const points = superTileCells.flatMap((cell) => cell.corners);
    if (points.length === 0) return anchorWorld;
    const bounds = points.reduce(
      (current, point) => ({
        left: Math.min(current.left, point.x),
        right: Math.max(current.right, point.x),
        top: Math.min(current.top, point.y),
        bottom: Math.max(current.bottom, point.y),
      }),
      {
        left: points[0].x,
        right: points[0].x,
        top: points[0].y,
        bottom: points[0].y,
      },
    );
    return {
      x: (bounds.left + bounds.right) / 2,
      y: (bounds.top + bounds.bottom) / 2,
    };
  }, [anchorWorld, superTileCells]);
  const viewWorldCenter = useMemo(
    () =>
      viewCenter
        ? projectToWorld(viewCenter.longitude, viewCenter.latitude, mapZoom)
        : selectionWorldCenter,
    [mapZoom, selectionWorldCenter, viewCenter],
  );
  const tiles = useMemo(() => {
    if (!size.width || !size.height) return [];
    const firstX =
      Math.floor((viewWorldCenter.x - size.width / 2) / TILE_SIZE) - 1;
    const lastX =
      Math.floor((viewWorldCenter.x + size.width / 2) / TILE_SIZE) + 1;
    const firstY =
      Math.floor((viewWorldCenter.y - size.height / 2) / TILE_SIZE) - 1;
    const lastY =
      Math.floor((viewWorldCenter.y + size.height / 2) / TILE_SIZE) + 1;
    const tileCount = 2 ** mapZoom;
    const visibleTiles = [];
    for (let tileY = firstY; tileY <= lastY; tileY += 1) {
      if (tileY < 0 || tileY >= tileCount) continue;
      for (let tileX = firstX; tileX <= lastX; tileX += 1) {
        const wrappedX = ((tileX % tileCount) + tileCount) % tileCount;
        visibleTiles.push({
          key: `${mapZoom}/${tileX}/${tileY}`,
          url: `https://tile.openstreetmap.org/${mapZoom}/${wrappedX}/${tileY}.png`,
          left: tileX * TILE_SIZE - viewWorldCenter.x + size.width / 2,
          top: tileY * TILE_SIZE - viewWorldCenter.y + size.height / 2,
        });
      }
    }
    return visibleTiles;
  }, [mapZoom, size, viewWorldCenter]);

  // Imported trails as screen-space SVG polylines, projected with the same
  // Mercator math as the selection boxes. Long tracks are thinned for
  // rendering only; the spec keeps the full resolution.
  const trailPaths = useMemo(() => {
    if (!size.width || !size.height || spec.trails.length === 0) return [];
    const worldScale = TILE_SIZE * 2 ** mapZoom;
    const toScreen = (latitude: number, longitude: number) => {
      const projected = projectToWorld(longitude, latitude, mapZoom);
      let deltaX = projected.x - anchorWorld.x;
      if (deltaX > worldScale / 2) deltaX -= worldScale;
      if (deltaX < -worldScale / 2) deltaX += worldScale;
      const x = anchorWorld.x + deltaX - viewWorldCenter.x + size.width / 2;
      const y = projected.y - viewWorldCenter.y + size.height / 2;
      return `${x.toFixed(1)},${y.toFixed(1)}`;
    };
    // Hand-edited setup imports can carry empty or single-point trails;
    // those have nothing to draw and would otherwise crash the projection.
    return spec.trails
      .filter((trail) => trail.points.length >= 2)
      .map((trail) => {
        const stride = Math.max(1, Math.ceil(trail.points.length / 400));
        const points = [];
        for (let index = 0; index < trail.points.length; index += stride) {
          points.push(toScreen(trail.points[index][0], trail.points[index][1]));
        }
        const last = trail.points[trail.points.length - 1];
        const lastScreen = toScreen(last[0], last[1]);
        if (points[points.length - 1] !== lastScreen) {
          points.push(lastScreen);
        }
        return points.join(" ");
      });
  }, [anchorWorld.x, mapZoom, size, spec.trails, viewWorldCenter]);

  const markerPositions = useMemo(() => {
    if (!size.width || !size.height) return [];
    const worldScale = TILE_SIZE * 2 ** mapZoom;
    return spec.markers.map((marker, index) => {
      const projected = projectToWorld(
        marker.longitude,
        marker.latitude,
        mapZoom,
      );
      let deltaX = projected.x - anchorWorld.x;
      if (deltaX > worldScale / 2) deltaX -= worldScale;
      if (deltaX < -worldScale / 2) deltaX += worldScale;
      return {
        ...marker,
        index,
        x: anchorWorld.x + deltaX - viewWorldCenter.x + size.width / 2,
        y: projected.y - viewWorldCenter.y + size.height / 2,
      };
    });
  }, [anchorWorld.x, mapZoom, size, spec.markers, viewWorldCenter]);

  const groundSpanLabel = Number.isInteger(spec.ground_span_km)
    ? spec.ground_span_km.toFixed(0)
    : spec.ground_span_km.toFixed(2).replace(/0$/, "");
  const anchorRow =
    spec.super_tile_anchor === "center" ? Math.floor(superTileRows / 2) : 0;
  const anchorColumn =
    spec.super_tile_anchor === "center" ? Math.floor(superTileColumns / 2) : 0;
  const selectedCell = superTileCells.find(
    (cell) => cell.row === anchorRow && cell.column === anchorColumn,
  );
  const projectedSelectionWidth = selectedCell
    ? Math.hypot(
        selectedCell.corners[1].x - selectedCell.corners[0].x,
        selectedCell.corners[1].y - selectedCell.corners[0].y,
      )
    : 8;
  const selectionSize = Math.max(
    8,
    Math.min(Math.min(size.width, size.height) * 0.94, projectedSelectionWidth),
  );
  const anchorDescription =
    spec.super_tile_anchor === "center" ? "center tile" : "top-left tile";

  const moveToWorld = useCallback(
    (worldX: number, worldY: number) =>
      unprojectFromWorld(worldX, worldY, mapZoom),
    [mapZoom],
  );

  const pointerDown = (event: ReactPointerEvent<HTMLDivElement>) => {
    event.currentTarget.setPointerCapture(event.pointerId);
    const bounds = event.currentTarget.getBoundingClientRect();
    const mode = markerPlacementMode ? "marker" : interactionMode;
    dragRef.current = {
      pointerId: event.pointerId,
      mode,
      startX: event.clientX,
      startY: event.clientY,
      localStartX: event.clientX - bounds.left,
      localStartY: event.clientY - bounds.top,
      worldX: mode === "move" ? anchorWorld.x : viewWorldCenter.x,
      worldY: mode === "move" ? anchorWorld.y : viewWorldCenter.y,
    };
  };

  const pointerMove = (event: ReactPointerEvent<HTMLDivElement>) => {
    const drag = dragRef.current;
    if (!drag || drag.pointerId !== event.pointerId) return;
    if (drag.mode === "marker") return;
    if (drag.mode === "select") {
      const bounds = event.currentTarget.getBoundingClientRect();
      setDraft(
        selectionDraft(
          drag.localStartX,
          drag.localStartY,
          event.clientX - bounds.left,
          event.clientY - bounds.top,
          superTileColumns,
          superTileRows,
        ),
      );
      return;
    }
    const next = moveToWorld(
      drag.worldX - (event.clientX - drag.startX),
      drag.worldY - (event.clientY - drag.startY),
    );
    if (drag.mode === "move") {
      onCenterChange(next.longitude, next.latitude);
    } else {
      setViewCenter(next);
    }
  };

  const pointerUp = (event: ReactPointerEvent<HTMLDivElement>) => {
    const drag = dragRef.current;
    if (!drag || drag.pointerId !== event.pointerId) return;
    dragRef.current = null;
    if (drag.mode === "marker") {
      if (
        Math.hypot(event.clientX - drag.startX, event.clientY - drag.startY) <=
        6
      ) {
        const bounds = event.currentTarget.getBoundingClientRect();
        const point = moveToWorld(
          viewWorldCenter.x + event.clientX - bounds.left - size.width / 2,
          viewWorldCenter.y + event.clientY - bounds.top - size.height / 2,
        );
        onPlaceMarker(point.longitude, point.latitude);
      }
      return;
    }
    if (drag.mode === "select") {
      const bounds = event.currentTarget.getBoundingClientRect();
      const nextDraft = selectionDraft(
        drag.localStartX,
        drag.localStartY,
        event.clientX - bounds.left,
        event.clientY - bounds.top,
        superTileColumns,
        superTileRows,
      );
      setDraft(null);
      if (nextDraft.cellSize < 8) return;
      const draftCenter = moveToWorld(
        viewWorldCenter.x +
          nextDraft.left +
          nextDraft.width * 0.5 -
          size.width * 0.5,
        viewWorldCenter.y +
          nextDraft.top +
          nextDraft.height * 0.5 -
          size.height * 0.5,
      );
      const draftMetresPerPixel = metresPerPixelAtLatitude(
        draftCenter.latitude,
        mapZoom,
      );
      const nextGroundSpan = Math.max(
        MIN_GROUND_SPAN_KM,
        Math.min(
          MAX_GROUND_SPAN_KM,
          Math.round(((nextDraft.cellSize * draftMetresPerPixel) / 1000) * 4) /
            4,
        ),
      );
      const anchorX =
        spec.super_tile_anchor === "center"
          ? nextDraft.left + nextDraft.width * 0.5
          : nextDraft.left + nextDraft.width * 0.5 +
            (-nextDraft.width * 0.5 + nextDraft.cellSize * 0.5) *
              rotationCosine -
            (-nextDraft.height * 0.5 + nextDraft.cellSize * 0.5) *
              rotationSine;
      const anchorY =
        spec.super_tile_anchor === "center"
          ? nextDraft.top + nextDraft.height * 0.5
          : nextDraft.top + nextDraft.height * 0.5 +
            (-nextDraft.width * 0.5 + nextDraft.cellSize * 0.5) *
              rotationSine +
            (-nextDraft.height * 0.5 + nextDraft.cellSize * 0.5) *
              rotationCosine;
      const nextAnchor = moveToWorld(
        viewWorldCenter.x + anchorX - size.width * 0.5,
        viewWorldCenter.y + anchorY - size.height * 0.5,
      );
      setViewCenter(moveToWorld(viewWorldCenter.x, viewWorldCenter.y));
      onGroundSpanChange(nextGroundSpan);
      onCenterChange(nextAnchor.longitude, nextAnchor.latitude);
      return;
    }
    const next = moveToWorld(
      drag.worldX - (event.clientX - drag.startX),
      drag.worldY - (event.clientY - drag.startY),
    );
    if (drag.mode === "move") {
      onCenterChange(next.longitude, next.latitude);
    } else {
      setViewCenter(next);
    }
  };

  const keyDown = (event: ReactKeyboardEvent<HTMLDivElement>) => {
    // Only the map surface itself pans; keys aimed at inner controls
    // (like the zoom buttons) keep their own behavior.
    if (event.target !== event.currentTarget) return;
    let east = 0;
    let north = 0;
    if (event.key === "ArrowLeft") east = -1;
    else if (event.key === "ArrowRight") east = 1;
    else if (event.key === "ArrowUp") north = 1;
    else if (event.key === "ArrowDown") north = -1;
    else return;
    // preventDefault only for handled keys, so Tab and the rest still work
    // and the page never scrolls under an arrow press.
    event.preventDefault();
    const share = event.shiftKey
      ? KEYBOARD_PAN_SHARE_SHIFT
      : KEYBOARD_PAN_SHARE;
    const keyboardOrigin =
      interactionMode === "move" ? anchorWorld : viewWorldCenter;
    const viewPosition = moveToWorld(keyboardOrigin.x, keyboardOrigin.y);
    const viewMetresPerPixel = metresPerPixelAtLatitude(
      viewPosition.latitude,
      mapZoom,
    );
    const panPixels =
      (spec.ground_span_km * 1000 * share) / viewMetresPerPixel;
    // moveToWorld runs the same unproject as drags, so latitude clamps to
    // the Mercator range and longitude wraps at the antimeridian.
    const next = moveToWorld(
      keyboardOrigin.x + east * panPixels,
      keyboardOrigin.y - north * panPixels,
    );
    if (interactionMode === "move") {
      onCenterChange(next.longitude, next.latitude);
    } else {
      setViewCenter(next);
    }
  };

  const changeZoom = useCallback(
    (delta: number) => {
      if (!zoomLinked) {
        setMapOnlyZoom((current) =>
          Math.max(
            MIN_MAP_ZOOM,
            Math.min(MAX_MAP_ZOOM, (current ?? mapZoom) + delta),
          ),
        );
        return;
      }
      const nextZoom = Math.max(
        MIN_MAP_ZOOM,
        Math.min(MAX_MAP_ZOOM, zoom + delta),
      );
      if (nextZoom === zoom) return;
      if (zoomLinked) {
        const nextGroundSpan = spec.ground_span_km * 2 ** (zoom - nextZoom);
        if (
          nextGroundSpan < MIN_GROUND_SPAN_KM ||
          nextGroundSpan > MAX_GROUND_SPAN_KM
        ) {
          return;
        }
        onGroundSpanChange(Math.round(nextGroundSpan * 4) / 4);
      }
      setZoom(nextZoom);
    },
    [mapZoom, onGroundSpanChange, spec.ground_span_km, zoom, zoomLinked],
  );

  // React's onWheel registers a passive listener, so preventDefault is a
  // no-op there; a native non-passive listener keeps the page from scrolling.
  useEffect(() => {
    const container = containerRef.current;
    if (!container) return;
    const wheel = (event: WheelEvent) => {
      if (event.deltaY === 0) return;
      event.preventDefault();
      changeZoom(event.deltaY < 0 ? 1 : -1);
    };
    container.addEventListener("wheel", wheel, { passive: false });
    return () => container.removeEventListener("wheel", wheel);
  }, [changeZoom]);

  const canZoomIn =
    mapZoom < MAX_MAP_ZOOM &&
    (!zoomLinked || spec.ground_span_km / 2 >= MIN_GROUND_SPAN_KM);
  const canZoomOut =
    mapZoom > MIN_MAP_ZOOM &&
    (!zoomLinked || spec.ground_span_km * 2 <= MAX_GROUND_SPAN_KM);
  const displayedViewCenter =
    viewCenter ?? moveToWorld(selectionWorldCenter.x, selectionWorldCenter.y);

  return (
    <div className="map-shell">
      <div
        ref={containerRef}
        className={`map-canvas map-${interactionMode}${markerPlacementMode ? " placing-marker" : ""}`}
        data-interaction-mode={markerPlacementMode ? "marker" : interactionMode}
        data-view-latitude={displayedViewCenter.latitude.toFixed(8)}
        data-view-longitude={displayedViewCenter.longitude.toFixed(8)}
        aria-keyshortcuts="ArrowUp ArrowDown ArrowLeft ArrowRight"
        aria-label="Terrain map. Pan the map, move the terrain area, or draw a new area. Use the mouse wheel to zoom or focus the map and pan with the arrow keys."
        onKeyDown={keyDown}
        onPointerDown={pointerDown}
        onPointerMove={pointerMove}
        onPointerUp={pointerUp}
        onPointerCancel={() => {
          dragRef.current = null;
          setDraft(null);
        }}
        role="application"
        tabIndex={0}
        title="Pan, move, or draw · Scroll to zoom · Arrow keys pan"
      >
        <div className="map-tiles" aria-hidden="true">
          {tiles.map((tile) => (
            // Map tiles must load from their source without image optimization.
            // eslint-disable-next-line @next/next/no-img-element
            <img
              alt=""
              draggable={false}
              key={tile.key}
              onLoad={() => setTilesLoaded(true)}
              src={tile.url}
              style={{ left: tile.left, top: tile.top }}
            />
          ))}
        </div>
        <svg
          aria-label={`Super-tile map: ${superTileColumns} across by ${superTileRows} down, ${
            terrainRotationDegrees === 0
              ? ""
              : `rotated ${terrainRotationDegrees} degrees, `
          }anchored at ${anchorDescription}`}
          className="map-super-tile-grid"
          data-super-tile-columns={superTileColumns}
          data-super-tile-rows={superTileRows}
          height={size.height}
          role="group"
          width={size.width}
        >
          {superTileCells.map((cell) => {
            const current =
              cell.row === anchorRow && cell.column === anchorColumn;
            const corners = cell.corners.map((point) => ({
              x: point.x - viewWorldCenter.x + size.width / 2,
              y: point.y - viewWorldCenter.y + size.height / 2,
            }));
            const points = corners
              .map((point) => `${point.x.toFixed(3)},${point.y.toFixed(3)}`)
              .join(" ");
            const labelX = (corners[0].x + corners[1].x) / 2;
            const labelY = (corners[0].y + corners[1].y) / 2 - 6;
            return (
              <g key={`${cell.row}-${cell.column}`}>
                <polygon
                  aria-label={
                    current
                      ? `Selected terrain area: ${groundSpanLabel} km square`
                      : `Super-tile row ${cell.row + 1}, column ${cell.column + 1}`
                  }
                  className={`map-selection${current ? " current" : ""}`}
                  data-ground-span-km={spec.ground_span_km}
                  data-map-zoom={mapZoom}
                  data-super-tile-column={cell.column + 1}
                  data-super-tile-row={cell.row + 1}
                  points={points}
                  role="img"
                />
                {current && (
                  <text
                    className="map-selection-label"
                    textAnchor="middle"
                    x={labelX}
                    y={labelY}
                  >
                    {groundSpanLabel} km
                  </text>
                )}
              </g>
            );
          })}
        </svg>
        {draft && (
          <div
            aria-label={`New terrain area: ${superTileColumns} across by ${superTileRows} down`}
            className="map-selection-draft"
            data-cell-size={draft.cellSize}
            role="img"
            style={
              {
                height: draft.height,
                left: draft.left,
                top: draft.top,
                width: draft.width,
                "--draft-columns": superTileColumns,
                "--draft-rows": superTileRows,
                transform: `rotate(${terrainRotationDegrees}deg)`,
              } as CSSProperties
            }
          >
            <span>
              {superTileColumns} × {superTileRows}
            </span>
          </div>
        )}
        {trailPaths.length > 0 && (
          <svg
            aria-hidden="true"
            className="map-trails"
            height={size.height}
            style={{
              inset: 0,
              pointerEvents: "none",
              position: "absolute",
            }}
            viewBox={`0 0 ${size.width} ${size.height}`}
            width={size.width}
          >
            {trailPaths.map((path, index) => (
              <polyline
                fill="none"
                key={index}
                opacity={0.9}
                points={path}
                stroke={spec.color_output.trail_color}
                strokeLinecap="round"
                strokeLinejoin="round"
                strokeWidth={2.5}
              />
            ))}
          </svg>
        )}
        {markerPositions.length > 0 && (
          <div aria-hidden="true" className="map-markers">
            {markerPositions.map((marker) => (
              // Convert print dimensions to the current map scale. Keep only a
              // small legibility floor so text-size changes and map-only zoom
              // remain visible in the preview.
              <span
                className={`map-marker ${marker.kind}`}
                key={`${marker.latitude}:${marker.longitude}:${marker.index}`}
                style={
                  {
                    left: marker.x,
                    top: marker.y,
                    "--marker-color": spec.marker_settings.color,
                    "--marker-rotation": `${marker.rotation_degrees}deg`,
                  } as CSSProperties
                }
                title={marker.name}
              >
                {marker.kind === "flag_label" && (
                  <span className="map-marker-name">{marker.name}</span>
                )}
                {isMapLabel(marker.kind) && (
                  <span
                    className="map-feature-label-text"
                    data-label-height-mm={marker.label_height_mm}
                    style={{
                      fontSize: Math.max(
                        3,
                        (marker.label_height_mm / spec.width_mm) *
                          selectionSize,
                      ),
                      padding:
                        marker.kind === "plaque_label"
                          ? Math.max(
                              2,
                              ((marker.label_style?.plaque_padding_mm ??
                                DEFAULT_MAP_LABEL_STYLE.plaque_padding_mm) /
                                spec.width_mm) *
                                selectionSize,
                            )
                          : 0,
                    }}
                  >
                    {marker.name}
                  </span>
                )}
              </span>
            ))}
          </div>
        )}
      </div>
      <div
        className="map-mode-tools"
        aria-label="Map interaction"
        role="toolbar"
      >
        <button
          aria-label="Pan map without moving terrain area"
          aria-pressed={interactionMode === "pan"}
          onClick={() => {
            setDraft(null);
            setInteractionMode("pan");
          }}
          type="button"
        >
          Pan
        </button>
        <button
          aria-label="Move terrain area with map"
          aria-pressed={interactionMode === "move"}
          onClick={() => {
            setDraft(null);
            setViewCenter(null);
            setInteractionMode("move");
          }}
          type="button"
        >
          Move area
        </button>
        <button
          aria-label="Draw terrain area"
          aria-pressed={interactionMode === "select"}
          onClick={() => setInteractionMode("select")}
          type="button"
        >
          Draw area
        </button>
        <button
          aria-label="Center map on terrain area"
          disabled={viewCenter === null}
          onClick={() => setViewCenter(null)}
          type="button"
        >
          Center
        </button>
        <span className="map-instruction" role="status">
          {tilesLoaded
            ? markerPlacementMode
              ? markerPlacementMode === "move"
                ? "Click the map to move the marker"
                : "Click the map to place the marker"
              : interactionMode === "pan"
                ? "Drag to pan"
                : interactionMode === "move"
                  ? "Drag to move area"
                : superTileActive
                  ? `Drag ${superTileColumns} × ${superTileRows} area`
                  : "Drag a terrain area"
            : "Loading map…"}
        </span>
      </div>
      <div className="map-zoom" aria-label="Map zoom">
        <button
          type="button"
          aria-label="Resize selected area with map zoom"
          aria-pressed={zoomLinked}
          className="map-zoom-mode"
          onClick={() =>
            setMapOnlyZoom((current) => (current === null ? mapZoom : null))
          }
          title={
            zoomLinked
              ? "Zoom changes the selected area"
              : "Zoom changes the map view only"
          }
        >
          {zoomLinked ? "Linked" : "Map only"}
        </button>
        <button
          type="button"
          aria-label="Zoom in"
          disabled={!canZoomIn}
          onClick={() => changeZoom(1)}
        >
          +
        </button>
        <button
          type="button"
          aria-label="Zoom out"
          disabled={!canZoomOut}
          onClick={() => changeZoom(-1)}
        >
          −
        </button>
      </div>
      <div
        className="map-crosshair"
        aria-hidden="true"
        style={{
          left: anchorWorld.x - viewWorldCenter.x + size.width / 2,
          top: anchorWorld.y - viewWorldCenter.y + size.height / 2,
        }}
      >
        <span />
        <span />
      </div>
      <ExternalLink
        className="map-attribution"
        href="https://www.openstreetmap.org/copyright"
      >
        © OpenStreetMap
      </ExternalLink>
    </div>
  );
}
