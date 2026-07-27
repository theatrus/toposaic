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
import {
  DEFAULT_MAP_LABEL_STYLE,
  isMapLabel,
  MAX_GROUND_SPAN_KM,
  MIN_GROUND_SPAN_KM,
} from "./config";
import { superTileCenter } from "./geo";

const TILE_SIZE = 256;
const MAX_MERCATOR_LATITUDE = 85.05112878;
const MIN_MAP_ZOOM = 2;
const MAX_MAP_ZOOM = 17;
// Arrow keys pan the focused map by a share of the current ground span.
const KEYBOARD_PAN_SHARE = 0.1;
const KEYBOARD_PAN_SHARE_SHIFT = 0.5;
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

export function TerrainMap({
  spec,
  markerPlacementMode,
  onPlaceMarker,
  onCenterChange,
  onGroundSpanChange,
}: {
  spec: GenerationSpec;
  markerPlacementMode: "place" | "move" | null;
  onPlaceMarker: (longitude: number, latitude: number) => void;
  onCenterChange: (longitude: number, latitude: number) => void;
  onGroundSpanChange: (groundSpanKm: number) => void;
}) {
  const containerRef = useRef<HTMLDivElement>(null);
  const dragRef = useRef<{
    pointerId: number;
    startX: number;
    startY: number;
    worldX: number;
    worldY: number;
  } | null>(null);
  const [zoom, setZoom] = useState(9);
  const [zoomLinked, setZoomLinked] = useState(true);
  const [size, setSize] = useState({ width: 0, height: 0 });
  const [tilesLoaded, setTilesLoaded] = useState(false);
  const superTileColumns = Math.max(1, spec.adjacent_columns);
  const superTileRows = Math.max(1, spec.adjacent_rows);
  const superTileActive = superTileColumns > 1 || superTileRows > 1;

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

  const baseMetresPerPixel =
    (156543.03392 *
      Math.max(0.1, Math.cos((spec.center_lat * Math.PI) / 180))) /
    2 ** zoom;
  const baseSelectionSize = (spec.ground_span_km * 1000) / baseMetresPerPixel;
  const fitScale = Math.max(
    1,
    size.width
      ? (baseSelectionSize * superTileColumns) / (size.width * 0.88)
      : 1,
    size.height
      ? (baseSelectionSize * superTileRows) / (size.height * 0.82)
      : 1,
  );
  const mapZoom = Math.max(
    MIN_MAP_ZOOM,
    zoom - Math.max(0, Math.ceil(Math.log2(fitScale))),
  );
  const anchorWorld = useMemo(
    () => projectToWorld(spec.center_lon, spec.center_lat, mapZoom),
    [mapZoom, spec.center_lat, spec.center_lon],
  );
  const superTileCells = useMemo(() => {
    const worldScale = TILE_SIZE * 2 ** mapZoom;
    const cells = [];
    for (let row = 0; row < superTileRows; row += 1) {
      for (let column = 0; column < superTileColumns; column += 1) {
        const center = superTileCenter(
          spec.center_lat,
          spec.center_lon,
          spec.ground_span_km,
          row,
          column,
          superTileRows,
          superTileColumns,
          spec.super_tile_anchor,
        );
        const projected = projectToWorld(
          center.longitude,
          center.latitude,
          mapZoom,
        );
        let deltaX = projected.x - anchorWorld.x;
        if (deltaX > worldScale / 2) deltaX -= worldScale;
        if (deltaX < -worldScale / 2) deltaX += worldScale;
        cells.push({
          row,
          column,
          worldX: anchorWorld.x + deltaX,
          worldY: projected.y,
        });
      }
    }
    return cells;
  }, [
    anchorWorld.x,
    mapZoom,
    spec.center_lat,
    spec.center_lon,
    spec.ground_span_km,
    spec.super_tile_anchor,
    superTileColumns,
    superTileRows,
  ]);
  const viewWorldCenter = useMemo(() => {
    const firstCell = superTileCells[0];
    const lastCell = superTileCells.at(-1);
    if (!firstCell || !lastCell) return anchorWorld;
    return {
      x: (firstCell.worldX + lastCell.worldX) / 2,
      y: (firstCell.worldY + lastCell.worldY) / 2,
    };
  }, [anchorWorld, superTileCells]);
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

  const metresPerPixel =
    (156543.03392 *
      Math.max(0.1, Math.cos((spec.center_lat * Math.PI) / 180))) /
    2 ** mapZoom;
  const selectionSize = Math.max(
    8,
    Math.min(
      Math.min(size.width, size.height) * 0.94,
      (spec.ground_span_km * 1000) / metresPerPixel,
    ),
  );
  const groundSpanLabel = Number.isInteger(spec.ground_span_km)
    ? spec.ground_span_km.toFixed(0)
    : spec.ground_span_km.toFixed(2).replace(/0$/, "");
  const anchorRow =
    spec.super_tile_anchor === "center" ? Math.floor(superTileRows / 2) : 0;
  const anchorColumn =
    spec.super_tile_anchor === "center" ? Math.floor(superTileColumns / 2) : 0;
  const anchorDescription =
    spec.super_tile_anchor === "center" ? "center tile" : "top-left tile";

  const moveToWorld = useCallback(
    (worldX: number, worldY: number) =>
      unprojectFromWorld(worldX, worldY, mapZoom),
    [mapZoom],
  );

  const pointerDown = (event: ReactPointerEvent<HTMLDivElement>) => {
    event.currentTarget.setPointerCapture(event.pointerId);
    dragRef.current = {
      pointerId: event.pointerId,
      startX: event.clientX,
      startY: event.clientY,
      worldX: anchorWorld.x,
      worldY: anchorWorld.y,
    };
  };

  const pointerMove = (event: ReactPointerEvent<HTMLDivElement>) => {
    const drag = dragRef.current;
    if (!drag || drag.pointerId !== event.pointerId) return;
    if (markerPlacementMode) return;
    const next = moveToWorld(
      drag.worldX - (event.clientX - drag.startX),
      drag.worldY - (event.clientY - drag.startY),
    );
    onCenterChange(next.longitude, next.latitude);
  };

  const pointerUp = (event: ReactPointerEvent<HTMLDivElement>) => {
    const drag = dragRef.current;
    if (!drag || drag.pointerId !== event.pointerId) return;
    dragRef.current = null;
    if (markerPlacementMode) {
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
    const next = moveToWorld(
      drag.worldX - (event.clientX - drag.startX),
      drag.worldY - (event.clientY - drag.startY),
    );
    onCenterChange(next.longitude, next.latitude);
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
    const panPixels = (spec.ground_span_km * 1000 * share) / metresPerPixel;
    // moveToWorld runs the same unproject as drags, so latitude clamps to
    // the Mercator range and longitude wraps at the antimeridian.
    const next = moveToWorld(
      anchorWorld.x + east * panPixels,
      anchorWorld.y - north * panPixels,
    );
    onCenterChange(next.longitude, next.latitude);
  };

  const changeZoom = useCallback(
    (delta: number) => {
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
    [onGroundSpanChange, spec.ground_span_km, zoom, zoomLinked],
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
    zoom < MAX_MAP_ZOOM &&
    (!zoomLinked || spec.ground_span_km / 2 >= MIN_GROUND_SPAN_KM);
  const canZoomOut =
    zoom > MIN_MAP_ZOOM &&
    (!zoomLinked || spec.ground_span_km * 2 <= MAX_GROUND_SPAN_KM);

  return (
    <div className="map-shell">
      <div
        ref={containerRef}
        className={`map-canvas${markerPlacementMode ? " placing-marker" : ""}`}
        aria-keyshortcuts="ArrowUp ArrowDown ArrowLeft ArrowRight"
        aria-label="Terrain map. Drag to choose a place, use the mouse wheel to zoom, or focus the map and pan with the arrow keys."
        onKeyDown={keyDown}
        onPointerDown={pointerDown}
        onPointerMove={pointerMove}
        onPointerUp={pointerUp}
        onPointerCancel={() => {
          dragRef.current = null;
        }}
        role="application"
        tabIndex={0}
        title="Scroll to zoom · Arrow keys pan · Shift for bigger steps"
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
        <div
          aria-label={`Super-tile map: ${superTileColumns} across by ${superTileRows} down, anchored at ${anchorDescription}`}
          className="map-super-tile-grid"
          data-super-tile-columns={superTileColumns}
          data-super-tile-rows={superTileRows}
          role="group"
        >
          {superTileCells.map((cell) => {
            const current =
              cell.row === anchorRow && cell.column === anchorColumn;
            return (
              <div
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
                key={`${cell.row}-${cell.column}`}
                role="img"
                style={{
                  height: selectionSize,
                  left: cell.worldX - viewWorldCenter.x + size.width / 2,
                  top: cell.worldY - viewWorldCenter.y + size.height / 2,
                  width: selectionSize,
                }}
              >
                {current && <span>{groundSpanLabel} km</span>}
              </div>
            );
          })}
        </div>
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
      <div className="map-zoom" aria-label="Map zoom">
        <button
          type="button"
          aria-label="Resize selected area with map zoom"
          aria-pressed={zoomLinked}
          className="map-zoom-mode"
          onClick={() => setZoomLinked((linked) => !linked)}
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
      <div className="map-instruction">
        {tilesLoaded ? (
          <>
            {markerPlacementMode
              ? markerPlacementMode === "move"
                ? "Click the map to move the marker"
                : "Click the map to place the marker"
              : superTileActive
                ? `Super-tile mode · ${superTileColumns} × ${superTileRows} · current tile is ${anchorDescription}`
                : "Drag the map to choose a place"}
            <small>
              {zoomLinked ? "Linked zoom" : "Map-only zoom"} · Scroll to zoom ·
              Arrow keys pan · Shift for bigger steps
            </small>
          </>
        ) : (
          "Loading map tiles…"
        )}
      </div>
      <a
        className="map-attribution"
        href="https://www.openstreetmap.org/copyright"
        target="_blank"
        rel="noreferrer"
      >
        © OpenStreetMap
      </a>
    </div>
  );
}
