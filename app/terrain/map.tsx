"use client";

import {
  type PointerEvent as ReactPointerEvent,
  type WheelEvent as ReactWheelEvent,
  useCallback,
  useEffect,
  useMemo,
  useRef,
  useState,
} from "react";

import type { GenerationSpec } from "./contracts";
import { superTileCenter } from "./geo";

const TILE_SIZE = 256;
const MAX_MERCATOR_LATITUDE = 85.05112878;
const MIN_MAP_ZOOM = 2;
const MAX_MAP_ZOOM = 17;
const MIN_GROUND_SPAN_KM = 0.25;
const MAX_GROUND_SPAN_KM = 80;

function projectToWorld(longitude: number, latitude: number, zoom: number) {
  const scale = TILE_SIZE * 2 ** zoom;
  const clampedLatitude = Math.max(
    -MAX_MERCATOR_LATITUDE,
    Math.min(MAX_MERCATOR_LATITUDE, latitude),
  );
  const sine = Math.sin((clampedLatitude * Math.PI) / 180);
  return {
    x: ((longitude + 180) / 360) * scale,
    y:
      (0.5 - Math.log((1 + sine) / (1 - sine)) / (4 * Math.PI)) *
      scale,
  };
}

function unprojectFromWorld(x: number, y: number, zoom: number) {
  const scale = TILE_SIZE * 2 ** zoom;
  const longitude = ((((x / scale) * 360) % 360) + 360) % 360 - 180;
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
  onCenterChange,
  onGroundSpanChange,
}: {
  spec: GenerationSpec;
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
  const baseSelectionSize =
    (spec.ground_span_km * 1000) / baseMetresPerPixel;
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
    spec.super_tile_anchor === "center"
      ? Math.floor(superTileRows / 2)
      : 0;
  const anchorColumn =
    spec.super_tile_anchor === "center"
      ? Math.floor(superTileColumns / 2)
      : 0;
  const anchorDescription =
    spec.super_tile_anchor === "center"
      ? "center tile"
      : "top-left tile";

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
    const next = moveToWorld(
      drag.worldX - (event.clientX - drag.startX),
      drag.worldY - (event.clientY - drag.startY),
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
      const nextGroundSpan = spec.ground_span_km * 2 ** (zoom - nextZoom);
      if (
        nextGroundSpan < MIN_GROUND_SPAN_KM ||
        nextGroundSpan > MAX_GROUND_SPAN_KM
      ) {
        return;
      }
      setZoom(nextZoom);
      onGroundSpanChange(Math.round(nextGroundSpan * 4) / 4);
    },
    [onGroundSpanChange, spec.ground_span_km, zoom],
  );

  const wheel = (event: ReactWheelEvent<HTMLDivElement>) => {
    event.preventDefault();
    changeZoom(event.deltaY < 0 ? 1 : -1);
  };

  const canZoomIn =
    zoom < MAX_MAP_ZOOM &&
    spec.ground_span_km / 2 >= MIN_GROUND_SPAN_KM;
  const canZoomOut =
    zoom > MIN_MAP_ZOOM &&
    spec.ground_span_km * 2 <= MAX_GROUND_SPAN_KM;

  return (
    <div className="map-shell">
      <div
        ref={containerRef}
        className="map-canvas"
        aria-label="Terrain map. Drag to choose a place."
        onPointerDown={pointerDown}
        onPointerMove={pointerMove}
        onPointerUp={pointerUp}
        onPointerCancel={() => {
          dragRef.current = null;
        }}
        onWheel={wheel}
        role="application"
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
                  left:
                    cell.worldX - viewWorldCenter.x + size.width / 2,
                  top:
                    cell.worldY - viewWorldCenter.y + size.height / 2,
                  width: selectionSize,
                }}
              >
                {current && <span>{groundSpanLabel} km</span>}
              </div>
            );
          })}
        </div>
      </div>
      <div className="map-zoom" aria-label="Map zoom">
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
        {tilesLoaded
          ? superTileActive
            ? `Super-tile mode · ${superTileColumns} × ${superTileRows} · current tile is ${anchorDescription}`
            : "Drag the map to choose a place"
          : "Loading map tiles…"}
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
