"use client";

import {
  type KeyboardEvent as ReactKeyboardEvent,
  useEffect,
  useRef,
  useState,
} from "react";
import {
  ACESFilmicToneMapping,
  BoxGeometry,
  BufferGeometry,
  Color,
  DirectionalLight,
  DoubleSide,
  Float32BufferAttribute,
  HemisphereLight,
  Line,
  LineBasicMaterial,
  Mesh,
  MeshStandardMaterial,
  PerspectiveCamera,
  Scene,
  SRGBColorSpace,
  Vector3,
  WebGLRenderer,
} from "three";
import { OrbitControls } from "three/examples/jsm/controls/OrbitControls.js";

import {
  previewInitialCameraPosition,
  previewWorldX,
} from "./preview-orientation";
import {
  aerialLineClass,
  assembledMeshSamples,
  effectiveMeshSamples,
  railLineClass,
} from "./config";
import type { GenerationSpec, PreviewData } from "./contracts";

// The terrain color the mesh renders when color output is off but trails
// or buildings still force color materials. The legend must show the same
// value, not the palette rock color.
const NEUTRAL_TERRAIN_COLOR = "#74846B";

// Palette keys in SurfaceClass::ALL order, so the index a preview reports
// is the index into this list. See crates/toposaic-core/src/spec.rs.
const CLASS_KEYS = [
  "rock",
  "forest",
  "snow",
  "water",
  "road",
  "building",
  "trail",
  "rail",
  "aerialway",
  "marker",
  "route_trail",
] as const;

function cubicBezier(
  start: [number, number],
  controlA: [number, number],
  controlB: [number, number],
  end: [number, number],
  t: number,
) {
  const inverse = 1 - t;
  const weights = [
    inverse ** 3,
    3 * inverse ** 2 * t,
    3 * inverse * t ** 2,
    t ** 3,
  ];
  return {
    along:
      start[0] * weights[0] +
      controlA[0] * weights[1] +
      controlB[0] * weights[2] +
      end[0] * weights[3],
    offset:
      start[1] * weights[0] +
      controlA[1] * weights[1] +
      controlB[1] * weights[2] +
      end[1] * weights[3],
  };
}

type EdgePattern = {
  center: number;
  radiusAlong: number;
  depthScale: number;
  skew: number;
};

function edgeNoise(seed: bigint, lane: bigint) {
  let value = BigInt.asUintN(
    64,
    seed ^ BigInt.asUintN(64, lane * 0xd6e8feb86659fd93n),
  );
  value ^= value >> 30n;
  value = BigInt.asUintN(64, value * 0xbf58476d1ce4e5b9n);
  value ^= value >> 27n;
  value = BigInt.asUintN(64, value * 0x94d049bb133111ebn);
  value ^= value >> 31n;
  return Number(value >> 40n) / 16777215;
}

function sharedEdgePattern(
  puzzleSeed: number,
  orientation: number,
  line: number,
  segment: number,
): EdgePattern {
  const seed =
    BigInt.asUintN(64, BigInt(puzzleSeed) * 0xd6e8feb86659fd93n) ^
    BigInt.asUintN(64, BigInt(orientation) * 0x9e3779b97f4a7c15n) ^
    BigInt.asUintN(64, BigInt(line) * 0xbf58476d1ce4e5b9n) ^
    BigInt.asUintN(64, BigInt(segment) * 0x94d049bb133111ebn);
  return {
    center: 0.43 + edgeNoise(seed, 2n) * 0.14,
    radiusAlong: 0.11 + edgeNoise(seed, 3n) * 0.035,
    depthScale: 0.88 + edgeNoise(seed, 4n) * 0.24,
    skew: (edgeNoise(seed, 5n) - 0.5) * 0.05,
  };
}

type PuzzleGridSpec = Pick<
  GenerationSpec,
  | "width_mm"
  | "rows"
  | "columns"
  | "straight_piece_sides"
  | "puzzle_seed"
  | "puzzle_tile_column"
  | "puzzle_tile_row"
>;

function puzzleGridPoint(spec: PuzzleGridSpec, row: number, column: number) {
  const pieceWidth = spec.width_mm / spec.columns;
  const pieceHeight = (spec.width_mm * spec.rows) / spec.columns / spec.rows;
  if (spec.straight_piece_sides) {
    return {
      x: column === spec.columns ? spec.width_mm : column * pieceWidth,
      y:
        row === spec.rows
          ? (spec.width_mm * spec.rows) / spec.columns
          : row * pieceHeight,
    };
  }
  const globalRow = spec.puzzle_tile_row * spec.rows + row;
  const globalColumn = spec.puzzle_tile_column * spec.columns + column;
  const gridKey =
    (BigInt.asUintN(32, BigInt(globalRow)) << 32n) |
    BigInt.asUintN(32, BigInt(globalColumn));
  const seed =
    gridKey ^
    BigInt.asUintN(64, BigInt(spec.puzzle_seed) * 0xd6e8feb86659fd93n);
  const x =
    column === 0
      ? 0
      : column === spec.columns
        ? spec.width_mm
        : column * pieceWidth +
          (edgeNoise(seed, 0n) - 0.5) * pieceWidth * 0.18;
  const modelHeight = (spec.width_mm * spec.rows) / spec.columns;
  const y =
    row === 0
      ? 0
      : row === spec.rows
        ? modelHeight
        : row * pieceHeight +
          (edgeNoise(seed, 1n) - 0.5) * pieceHeight * 0.18;
  return { x, y };
}

function edgeSign(
  puzzleSeed: number,
  orientation: number,
  segment: number,
  line: number,
) {
  const seed =
    BigInt.asUintN(64, BigInt(puzzleSeed) * 0xd6e8feb86659fd93n) ^
    BigInt.asUintN(64, BigInt(orientation) * 0xa24baed4963ee407n) ^
    BigInt.asUintN(64, BigInt(line) * 0x9fb21c651e98df25n) ^
    BigInt.asUintN(64, BigInt(segment) * 0xc13fa9a902a6328fn);
  return edgeNoise(seed, 7n) < 0.5 ? -1 : 1;
}

function jigsawEdge(t: number, pattern: EdgePattern) {
  const radius = pattern.radiusAlong;
  const neck = radius * 0.46;
  const shoulderStart = pattern.center - radius - 0.085;
  const shoulderEnd = pattern.center + radius + 0.085;
  const neckLeft: [number, number] = [pattern.center - neck, 0.18];
  const neckRight: [number, number] = [pattern.center + neck, 0.18];
  const headLeft: [number, number] = [pattern.center - radius, 0.58];
  const headRight: [number, number] = [pattern.center + radius, 0.58];
  const quarterCircle = 0.5522848;
  let point;
  if (t < 0.26) {
    point = { along: (t / 0.26) * shoulderStart, offset: 0 };
  } else if (t < 0.34) {
    point = cubicBezier(
      [shoulderStart, 0],
      [shoulderStart + 0.045, -0.01],
      [neckLeft[0] - 0.025, 0.04],
      neckLeft,
      (t - 0.26) / 0.08,
    );
  } else if (t < 0.42) {
    point = cubicBezier(
      neckLeft,
      [neckLeft[0] + 0.012, 0.34],
      [headLeft[0], 0.45],
      headLeft,
      (t - 0.34) / 0.08,
    );
  } else if (t < 0.5) {
    point = cubicBezier(
      headLeft,
      [
        headLeft[0],
        headLeft[1] + (1 - headLeft[1]) * quarterCircle,
      ],
      [pattern.center - radius * quarterCircle, 1],
      [pattern.center, 1],
      (t - 0.42) / 0.08,
    );
  } else if (t < 0.58) {
    point = cubicBezier(
      [pattern.center, 1],
      [pattern.center + radius * quarterCircle, 1],
      [
        headRight[0],
        headRight[1] + (1 - headRight[1]) * quarterCircle,
      ],
      headRight,
      (t - 0.5) / 0.08,
    );
  } else if (t < 0.66) {
    point = cubicBezier(
      headRight,
      [headRight[0], 0.45],
      [neckRight[0] - 0.012, 0.34],
      neckRight,
      (t - 0.58) / 0.08,
    );
  } else if (t < 0.74) {
    point = cubicBezier(
      neckRight,
      [neckRight[0] + 0.025, 0.04],
      [shoulderEnd - 0.045, -0.01],
      [shoulderEnd, 0],
      (t - 0.66) / 0.08,
    );
  } else {
    point = {
      along: shoulderEnd + ((t - 0.74) / 0.26) * (1 - shoulderEnd),
      offset: 0,
    };
  }
  return {
    along: point.along + pattern.skew * point.offset,
    offset: point.offset,
  };
}

function puzzleEdgePoint(
  start: { x: number; y: number },
  end: { x: number; y: number },
  pattern: EdgePattern,
  sign: number,
  t: number,
  baseDepth: number,
) {
  const deltaX = end.x - start.x;
  const deltaY = end.y - start.y;
  const length = Math.max(Number.EPSILON, Math.hypot(deltaX, deltaY));
  const edge = sign === 0 ? { along: t, offset: 0 } : jigsawEdge(t, pattern);
  const depth = baseDepth * pattern.depthScale;
  return {
    x:
      start.x +
      deltaX * edge.along -
      (deltaY / length) * sign * depth * edge.offset,
    y:
      start.y +
      deltaY * edge.along +
      (deltaX / length) * sign * depth * edge.offset,
  };
}

export function ReliefPreview({
  spec,
  preview,
  previewState,
}: {
  spec: GenerationSpec;
  preview: PreviewData | null;
  previewState: "shape" | "loading" | "elevation" | "generated";
}) {
  const canvasRef = useRef<HTMLCanvasElement>(null);
  const renderErrorRef = useRef<HTMLParagraphElement>(null);
  const controlsRef = useRef<OrbitControls | null>(null);
  const viewRef = useRef<{
    position: [number, number, number];
    target: [number, number, number];
  } | null>(null);
  const resetViewRef = useRef(false);
  const [resetViewKey, setResetViewKey] = useState(0);

  // The scene effect reads these fields alone, so depending on them (rather
  // than the whole spec object) avoids rebuilding the scene on unrelated
  // spec changes.
  const {
    relief_mm,
    width_mm,
    base_mm,
    center_lat,
    center_lon,
    rows,
    columns,
    solid_model,
    puzzle_tabs,
    straight_piece_sides,
    puzzle_seed,
    puzzle_tile_column,
    puzzle_tile_row,
  } = spec;
  const {
    enabled: colorOutputEnabled,
    rock_color,
    forest_color,
    snow_color,
    water_color,
    road_color,
    building_color,
    route_trail_color,
    trail_color,
    rail_color,
    aerial_color,
  } = spec.color_output;
  const markerColor = spec.marker_settings.color;
  const coloredMarkersPresent = spec.markers.some(
    (marker) => marker.kind !== "flag_hole",
  );
  const buildingsEnabled =
    spec.buildings.enabled ||
    spec.markers.some((marker) => marker.kind === "building");
  const trailsPresent = spec.trails.length > 0;
  // Which class each rail-family layer's lines actually land in, resolved
  // the way the backend resolves it. A layer only earns its own legend
  // entry when it lands in its own class; otherwise its geometry appears
  // under the entry for the class it borrowed, and that entry has to show
  // even when the borrowed layer is itself switched off.
  const railClass = railLineClass(spec.color_output);
  const aerialClass = aerialLineClass(spec.color_output);
  const railDrawn = spec.color_output.rail_enabled;
  const aerialDrawn = spec.color_output.aerial_enabled;
  // Lifts following a separately-colored railway layer paint in the rail
  // class, so the Rail entry names their color too.
  const railClassDrawn =
    (railDrawn && railClass === "rail") ||
    (aerialDrawn && aerialClass === "rail");
  const aerialClassDrawn = aerialDrawn && aerialClass === "aerialway";
  const roadClassDrawn =
    spec.color_output.roads_enabled ||
    (railDrawn && railClass === "road") ||
    (aerialDrawn && aerialClass === "road");

  useEffect(() => {
    const canvas = canvasRef.current;
    if (!canvas) return;
    if (renderErrorRef.current) renderErrorRef.current.hidden = true;
    let renderer: WebGLRenderer;
    try {
      renderer = new WebGLRenderer({
        canvas,
        antialias: true,
        alpha: true,
        powerPreference: "high-performance",
      });
    } catch {
      if (renderErrorRef.current) renderErrorRef.current.hidden = false;
      return;
    }

    const scene = new Scene();
    const sampleWidth = preview?.width ?? 32;
    const sampleHeight = preview?.height ?? sampleWidth;
    const heightScale = relief_mm / width_mm;
    const baseDepth = base_mm / width_mm;
    canvas.dataset.heightScale = heightScale.toFixed(4);
    canvas.dataset.baseScale = baseDepth.toFixed(4);
    const seedA = Math.sin((center_lat * Math.PI) / 180) * 1.7;
    const seedB = Math.cos((center_lon * Math.PI) / 180) * 1.3;
    const heightValues = Array.from(
      { length: sampleWidth * sampleHeight },
      (_, index) => {
        const x = index % sampleWidth;
        const y = Math.floor(index / sampleWidth);
        const u = x / Math.max(1, sampleWidth - 1);
        const v = y / Math.max(1, sampleHeight - 1);
        return (
          preview?.values[index] ??
          (() => {
            const ridge =
              Math.sin((u * 9.2 + seedA) * 1.2) * 0.19 +
              Math.cos((v * 7.1 - seedB) * 1.4) * 0.14;
            const folds =
              Math.abs(Math.sin((u * 3.8 + v * 5.6 + seedB) * Math.PI)) *
              0.17;
            const dx = u - (0.54 + seedB * 0.05);
            const dy = v - (0.48 + seedA * 0.05);
            const peak = Math.exp(-(dx * dx * 5.5 + dy * dy * 7)) * 0.63;
            return Math.max(0.03, Math.min(1, 0.12 + ridge + folds + peak));
          })()
        );
      },
    );

    const heightAt = (u: number, v: number) => {
      const sampleX = Math.max(
        0,
        Math.min(sampleWidth - 1, u * (sampleWidth - 1)),
      );
      const sampleY = Math.max(
        0,
        Math.min(sampleHeight - 1, v * (sampleHeight - 1)),
      );
      const x0 = Math.floor(sampleX);
      const y0 = Math.floor(sampleY);
      const x1 = Math.min(sampleWidth - 1, x0 + 1);
      const y1 = Math.min(sampleHeight - 1, y0 + 1);
      const tx = sampleX - x0;
      const ty = sampleY - y0;
      const bottom =
        heightValues[y0 * sampleWidth + x0] * (1 - tx) +
        heightValues[y0 * sampleWidth + x1] * tx;
      const top =
        heightValues[y1 * sampleWidth + x0] * (1 - tx) +
        heightValues[y1 * sampleWidth + x1] * tx;
      return bottom * (1 - ty) + top * ty;
    };

    const palette = {
      rock: preview?.surface_palette?.rock ?? rock_color,
      forest: preview?.surface_palette?.forest ?? forest_color,
      snow: preview?.surface_palette?.snow ?? snow_color,
      water: preview?.surface_palette?.water ?? water_color,
      road: preview?.surface_palette?.road ?? road_color,
      building: preview?.surface_palette?.building ?? building_color,
      route_trail:
        preview?.surface_palette?.route_trail ?? route_trail_color,
      trail: preview?.surface_palette?.trail ?? trail_color,
      rail: preview?.surface_palette?.rail ?? rail_color,
      aerialway: preview?.surface_palette?.aerialway ?? aerial_color,
      marker: preview?.surface_palette?.marker ?? markerColor,
    };
    const classColor = (surfaceClass?: number) => {
      // Preview classes are raw SurfaceClass material indices, in the
      // order SurfaceClass::ALL declares in
      // crates/toposaic-core/src/spec.rs. They stay raw — the 3MF packs
      // its filament slots, the preview does not — so this lookup must
      // keep that order exactly. Rail (7) and Aerial (8) only ever arrive
      // when their layer is drawn in its own color.
      const key = surfaceClass === undefined ? undefined : CLASS_KEYS[surfaceClass];
      // With color output off the mesh shows neutral terrain, not rock.
      if (key === undefined || (key === "rock" && !colorOutputEnabled)) {
        return colorOutputEnabled ? palette.rock : NEUTRAL_TERRAIN_COLOR;
      }
      return palette[key];
    };
    const positions: number[] = [];
    const colors: number[] = [];
    const normals: number[] = [];
    const normalAt = (x: number, y: number) => {
      const left = Math.max(0, x - 1);
      const right = Math.min(sampleWidth - 1, x + 1);
      const down = Math.max(0, y - 1);
      const up = Math.min(sampleHeight - 1, y + 1);
      const spanX = Math.max(1, right - left) / Math.max(1, sampleWidth - 1);
      const spanY = Math.max(1, up - down) / Math.max(1, sampleHeight - 1);
      const slopeX =
        ((heightValues[y * sampleWidth + right] -
          heightValues[y * sampleWidth + left]) *
          heightScale) /
        spanX;
      const slopeY =
        ((heightValues[up * sampleWidth + x] -
          heightValues[down * sampleWidth + x]) *
          heightScale) /
        spanY;
      return new Vector3(-slopeX, 1, -slopeY).normalize();
    };
    const addVertex = (
      x: number,
      y: number,
      color: Color,
    ) => {
      const u = x / Math.max(1, sampleWidth - 1);
      const v = y / Math.max(1, sampleHeight - 1);
      positions.push(
        previewWorldX(u),
        heightValues[y * sampleWidth + x] * heightScale,
        v - 0.5,
      );
      colors.push(color.r, color.g, color.b);
      const normal = normalAt(x, y);
      normals.push(-normal.x, normal.y, normal.z);
    };
    for (let y = 0; y < sampleHeight - 1; y += 1) {
      for (let x = 0; x < sampleWidth - 1; x += 1) {
        const surfaceClass =
          colorOutputEnabled ||
          buildingsEnabled ||
          trailsPresent ||
          coloredMarkersPresent
            ? preview?.surface_classes?.[y * sampleWidth + x]
            : undefined;
        const color = new Color(classColor(surfaceClass));
        addVertex(x, y, color);
        addVertex(x + 1, y, color);
        addVertex(x, y + 1, color);
        addVertex(x + 1, y, color);
        addVertex(x + 1, y + 1, color);
        addVertex(x, y + 1, color);
      }
    }

    const terrainGeometry = new BufferGeometry();
    terrainGeometry.setAttribute(
      "position",
      new Float32BufferAttribute(positions, 3),
    );
    terrainGeometry.setAttribute(
      "color",
      new Float32BufferAttribute(colors, 3),
    );
    terrainGeometry.setAttribute(
      "normal",
      new Float32BufferAttribute(normals, 3),
    );
    const terrainMaterial = new MeshStandardMaterial({
      color: 0xffffff,
      metalness: 0,
      roughness: 0.86,
      side: DoubleSide,
      vertexColors: true,
    });
    const terrainMesh = new Mesh(terrainGeometry, terrainMaterial);
    scene.add(terrainMesh);

    const baseGeometry = new BoxGeometry(1, baseDepth, 1);
    const baseMaterial = new MeshStandardMaterial({
      color: new Color(palette.rock).multiplyScalar(0.68),
      metalness: 0,
      roughness: 0.92,
    });
    const baseMesh = new Mesh(baseGeometry, baseMaterial);
    baseMesh.position.y = -baseDepth / 2;
    scene.add(baseMesh);

    const lineMaterial = new LineBasicMaterial({
      color: 0x14201d,
      opacity: 0.72,
      transparent: true,
    });
    const pointOnTerrain = (u: number, v: number) =>
      new Vector3(
        previewWorldX(u),
        heightAt(u, v) * heightScale + 0.0025,
        v - 0.5,
      );
    const perimeter = [
      ...Array.from({ length: sampleWidth }, (_, x) =>
        pointOnTerrain(x / Math.max(1, sampleWidth - 1), 0),
      ),
      ...Array.from({ length: sampleHeight - 1 }, (_, y) =>
        pointOnTerrain(1, (y + 1) / Math.max(1, sampleHeight - 1)),
      ),
      ...Array.from({ length: sampleWidth - 1 }, (_, x) =>
        pointOnTerrain(
          (sampleWidth - 2 - x) / Math.max(1, sampleWidth - 1),
          1,
        ),
      ),
      ...Array.from({ length: sampleHeight - 2 }, (_, y) =>
        pointOnTerrain(
          0,
          (sampleHeight - 2 - y) / Math.max(1, sampleHeight - 1),
        ),
      ),
    ];
    perimeter.push(perimeter[0].clone());
    scene.add(
      new Line(
        new BufferGeometry().setFromPoints(perimeter),
        lineMaterial,
      ),
    );

    const gridSpec = {
      width_mm,
      rows,
      columns,
      straight_piece_sides,
      puzzle_seed,
      puzzle_tile_column,
      puzzle_tile_row,
    };
    const modelHeight = (width_mm * rows) / columns;
    const puzzleTabDepth =
      Math.min(width_mm / columns, modelHeight / rows) * 0.17;
    if (!solid_model) {
      for (let edgeColumn = 1; edgeColumn < columns; edgeColumn += 1) {
        for (let row = 0; row < rows; row += 1) {
          const start = puzzleGridPoint(gridSpec, row, edgeColumn);
          const end = puzzleGridPoint(gridSpec, row + 1, edgeColumn);
          const globalLine = puzzle_tile_column * columns + edgeColumn;
          const globalSegment = puzzle_tile_row * rows + row;
          const pattern = sharedEdgePattern(
            puzzle_seed,
            1,
            globalLine,
            globalSegment,
          );
          const sign = puzzle_tabs
            ? edgeSign(puzzle_seed, 1, globalSegment, globalLine)
            : 0;
          const points = [];
          for (let step = 0; step <= 48; step += 1) {
            const t = step / 48;
            const edgePoint = puzzleEdgePoint(
              start,
              end,
              pattern,
              sign,
              t,
              puzzleTabDepth,
            );
            points.push(
              pointOnTerrain(
                edgePoint.x / width_mm,
                edgePoint.y / modelHeight,
              ),
            );
          }
          scene.add(
            new Line(
              new BufferGeometry().setFromPoints(points),
              lineMaterial,
            ),
          );
        }
      }
      for (let edgeRow = 1; edgeRow < rows; edgeRow += 1) {
        for (let column = 0; column < columns; column += 1) {
          const start = puzzleGridPoint(gridSpec, edgeRow, column);
          const end = puzzleGridPoint(gridSpec, edgeRow, column + 1);
          const globalLine = puzzle_tile_row * rows + edgeRow;
          const globalSegment = puzzle_tile_column * columns + column;
          const pattern = sharedEdgePattern(
            puzzle_seed,
            0,
            globalLine,
            globalSegment,
          );
          const sign = puzzle_tabs
            ? edgeSign(puzzle_seed, 0, globalSegment, globalLine)
            : 0;
          const points = [];
          for (let step = 0; step <= 48; step += 1) {
            const t = step / 48;
            const edgePoint = puzzleEdgePoint(
              start,
              end,
              pattern,
              sign,
              t,
              puzzleTabDepth,
            );
            points.push(
              pointOnTerrain(
                edgePoint.x / width_mm,
                edgePoint.y / modelHeight,
              ),
            );
          }
          scene.add(
            new Line(
              new BufferGeometry().setFromPoints(points),
              lineMaterial,
            ),
          );
        }
      }
    }

    scene.add(new HemisphereLight(0xffffff, 0x39433c, 1.8));
    const keyLight = new DirectionalLight(0xfff8df, 2.7);
    keyLight.position.set(-1.4, 2.1, 1.5);
    scene.add(keyLight);
    const fillLight = new DirectionalLight(0xb9d8ff, 0.8);
    fillLight.position.set(1.3, 0.7, -1);
    scene.add(fillLight);

    const camera = new PerspectiveCamera(36, 1, 0.01, 20);
    const cameraScale = Math.max(1, heightScale * 1.5);
    const defaultTarget: [number, number, number] = [
      0,
      heightScale * 0.35,
      0,
    ];
    const savedView = resetViewRef.current ? null : viewRef.current;
    if (savedView) {
      camera.position.fromArray(savedView.position);
    } else {
      camera.position.fromArray(
        previewInitialCameraPosition(cameraScale, defaultTarget[1]),
      );
    }
    const controls = new OrbitControls(camera, canvas);
    controls.target.fromArray(savedView?.target ?? defaultTarget);
    controls.enableDamping = false;
    controls.enablePan = false;
    controls.minDistance = 0.72;
    controls.maxDistance = 3.1 * cameraScale;
    controls.minPolarAngle = 0.12;
    controls.maxPolarAngle = Math.PI / 2 - 0.025;
    controls.rotateSpeed = 0.72;
    controls.zoomSpeed = 0.85;
    controls.zoomToCursor = true;
    controls.update();
    controlsRef.current = controls;
    canvas.dataset.cameraMoved = savedView ? "true" : "false";
    resetViewRef.current = false;
    const initialPosition = camera.position.clone();
    const initialTarget = controls.target.clone();

    renderer.setPixelRatio(Math.min(window.devicePixelRatio || 1, 2));
    renderer.outputColorSpace = SRGBColorSpace;
    renderer.toneMapping = ACESFilmicToneMapping;
    renderer.toneMappingExposure = 1.05;

    const render = () => renderer.render(scene, camera);
    const resize = () => {
      const width = Math.max(1, canvas.clientWidth);
      const height = Math.max(1, canvas.clientHeight);
      renderer.setSize(width, height, false);
      camera.aspect = width / height;
      camera.updateProjectionMatrix();
      render();
    };
    const onViewChange = () => {
      const positionDelta =
        camera.position.distanceToSquared(initialPosition);
      const targetDelta = controls.target.distanceToSquared(initialTarget);
      const cameraMoved = positionDelta > 1e-12 || targetDelta > 1e-12;
      if (cameraMoved) {
        canvas.dataset.cameraMoved = "true";
        viewRef.current = {
          position: camera.position.toArray(),
          target: controls.target.toArray(),
        };
      }
      render();
    };
    controls.addEventListener("change", onViewChange);
    const observer = new ResizeObserver(resize);
    observer.observe(canvas);
    resize();

    return () => {
      if (!resetViewRef.current && canvas.dataset.cameraMoved === "true") {
        viewRef.current = {
          position: camera.position.toArray(),
          target: controls.target.toArray(),
        };
      }
      observer.disconnect();
      controls.removeEventListener("change", onViewChange);
      controls.dispose();
      controlsRef.current = null;
      scene.traverse((object) => {
        if (object instanceof Mesh || object instanceof Line) {
          object.geometry.dispose();
          const materials = Array.isArray(object.material)
            ? object.material
            : [object.material];
          materials.forEach((material) => material.dispose());
        }
      });
      renderer.dispose();
    };
  }, [
    preview,
    resetViewKey,
    relief_mm,
    width_mm,
    base_mm,
    center_lat,
    center_lon,
    rows,
    columns,
    solid_model,
    puzzle_tabs,
    straight_piece_sides,
    puzzle_seed,
    puzzle_tile_column,
    puzzle_tile_row,
    colorOutputEnabled,
    buildingsEnabled,
    trailsPresent,
    coloredMarkersPresent,
    rock_color,
    forest_color,
    snow_color,
    water_color,
    road_color,
    building_color,
    trail_color,
    route_trail_color,
    rail_color,
    aerial_color,
    markerColor,
  ]);

  const keyboardOrbit = (event: ReactKeyboardEvent<HTMLCanvasElement>) => {
    const controls = controlsRef.current;
    if (!controls) return;
    switch (event.key) {
      case "ArrowLeft":
        controls.rotateLeft(Math.PI / 18);
        break;
      case "ArrowRight":
        controls.rotateLeft(-Math.PI / 18);
        break;
      case "ArrowUp":
        controls.rotateUp(Math.PI / 24);
        break;
      case "ArrowDown":
        controls.rotateUp(-Math.PI / 24);
        break;
      case "+":
      case "=":
        controls.dollyIn(1.12);
        break;
      case "-":
      case "_":
        controls.dollyOut(1.12);
        break;
      default:
        return;
    }
    event.preventDefault();
    controls.update();
  };

  return (
    <div className="relief-shell">
      <canvas
        ref={canvasRef}
        className="relief-canvas"
        aria-label="Interactive 3D terrain preview"
        data-camera-moved="false"
        data-puzzle-tabs={spec.puzzle_tabs}
        data-straight-piece-sides={spec.straight_piece_sides}
        onKeyDown={keyboardOrbit}
        tabIndex={0}
      />
      <p ref={renderErrorRef} className="preview-render-error" hidden>
        This system could not start the 3D preview.
      </p>
      <div className="preview-orbit-controls" aria-label="3D preview controls">
        <span>Drag to rotate · Scroll or pinch to zoom</span>
        <button
          type="button"
          onClick={() => {
            resetViewRef.current = true;
            viewRef.current = null;
            setResetViewKey((current) => current + 1);
          }}
        >
          Reset view
        </button>
      </div>
      {(spec.color_output.enabled ||
        spec.buildings.enabled ||
        trailsPresent) && (
        <div className="color-legend" aria-label="Surface color legend">
          {(
            [
              ["Forest", "forest", spec.color_output.forest_color],
              ["Rock", "rock", spec.color_output.rock_color],
              ["Snow", "snow", spec.color_output.snow_color],
              ["Water", "water", spec.color_output.water_color],
              ["Route", "road", spec.color_output.road_color],
              ["Trail", "route_trail", spec.color_output.route_trail_color],
              ["Building", "building", spec.color_output.building_color],
              ["Imported trail", "trail", spec.color_output.trail_color],
              ["Rail", "rail", spec.color_output.rail_color],
              ["Aerial", "aerialway", spec.color_output.aerial_color],
              ["Marker", "marker", spec.marker_settings.color],
            ] as const
          )
            .filter(
              ([, key]) => {
                if (key === "trail") {
                  return trailsPresent;
                }
                if (key === "route_trail") {
                  return spec.color_output.enabled && spec.color_output.roads_enabled;
                }
                if (key === "marker") {
                  return coloredMarkersPresent;
                }
                if (!spec.color_output.enabled) {
                  return (
                    key === "rock" ||
                    (key === "building" && buildingsEnabled)
                  );
                }
                // Every color the model shows carries exactly one name, so
                // each line entry asks whether its class is drawn at all —
                // by its own layer or by one borrowing it — rather than
                // whether its own toggle happens to be on.
                return (
                  (key !== "road" || roadClassDrawn) &&
                  (key !== "building" || buildingsEnabled) &&
                  (key !== "rail" || railClassDrawn) &&
                  (key !== "aerialway" || aerialClassDrawn)
                );
              },
            )
            .map(([label, key, color]) => (
              <span key={key}>
                <i
                  style={{
                    background:
                      // With color output off, the mesh renders the neutral
                      // terrain color for rock; the legend must match it.
                      key === "rock" && !spec.color_output.enabled
                        ? NEUTRAL_TERRAIN_COLOR
                        : (preview?.surface_palette?.[key] ?? color),
                  }}
                />
                {label}
                {preview?.surface_coverage && (
                  <small>
                    {(preview.surface_coverage[key] ?? 0).toFixed(0)}%
                  </small>
                )}
              </span>
            ))}
        </div>
      )}
      <div className={`preview-label ${previewState}`}>
        <span>
          {previewState === "generated"
            ? "Generated terrain"
            : previewState === "elevation"
              ? "Live elevation preview"
              : previewState === "loading"
                ? "Sampling preview elevation"
                : "Fast shape preview"}{" "}
          ·{" "}
          {spec.solid_model
            ? `${effectiveMeshSamples(spec)} mesh samples`
            : `${assembledMeshSamples(spec)} mesh samples across model`}
        </span>
        <strong>
          {spec.solid_model
            ? "One solid terrain model"
            : `${spec.columns} × ${spec.rows} pieces`}
        </strong>
      </div>
    </div>
  );
}
