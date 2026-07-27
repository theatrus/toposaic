"use client";

import {
  type CSSProperties,
  type KeyboardEvent as ReactKeyboardEvent,
  type FormEvent,
  type PointerEvent as ReactPointerEvent,
  lazy,
  Suspense,
  useCallback,
  useEffect,
  useMemo,
  useRef,
  useState,
} from "react";
import {
  APP_VERSION,
  RELEASES_URL,
  type AvailableUpdate,
  fetchAvailableUpdate,
  signedUpdateFallback,
} from "../updates/releases";
import {
  checkSignedUpdateVersion,
  downloadAndInstallSignedUpdate,
} from "../updates/desktop";
import { IS_TAURI, terrainApi } from "./api";
import { ExternalLink } from "./external-link";
import {
  MAX_ASSEMBLED_SAMPLES,
  MAX_SUPER_TILE_SIDE,
  deriveHeightFrame,
  initialSpec,
  markerNeedsSurfaceData,
  limitPlaceName,
  mergeSpecDefaults,
  normalizeMappedWidthCap,
  randomPuzzleSeed,
} from "./config";
import type {
  Artifact,
  ArtifactFeedback,
  GenerationSpec,
  Job,
  MarkerKind,
  PlaceResult,
  PreviewData,
  SavedSetup,
  TrailRoute,
} from "./contracts";
import {
  MAX_TRAILS,
  MAX_TRAIL_FILE_BYTES,
  MAX_TRAIL_POINTS,
  parseTrailFile,
} from "./trails";
import { type AdjacentDirection, adjacentCenter } from "./geo";
import { TerrainMap } from "./map";
import { SettingsMenu } from "./settings-menu";
import { useOutsideDismiss } from "./use-outside-dismiss";
import { BuildingsPanel } from "./panels/buildings-panel";
import { ColorsPanel } from "./panels/colors-panel";
import { ModelPanel } from "./panels/model-panel";
import { ModelTypePanel } from "./panels/model-type-panel";
import { MountingPanel } from "./panels/mounting-panel";
import { MarkersPanel } from "./panels/markers-panel";
import { OutputPanel } from "./panels/output-panel";
import { SurfacePanel } from "./panels/surface-panel";
import { displayVersion, isVersionNewer } from "../updates/version";

const ReliefPreview = lazy(() =>
  import("./preview").then(({ ReliefPreview: component }) => ({
    default: component,
  })),
);

const GENERATOR_STALLED_MESSAGE =
  "The generator stopped responding. The job is safe in SQLite.";

const DEFAULT_VISUAL_HEIGHT_PERCENT = 37;
const MIN_VISUAL_HEIGHT_PERCENT = 28;
const MAX_VISUAL_HEIGHT_PERCENT = 76;
const VISUAL_HEIGHT_KEYBOARD_STEP = 4;
const WORKSPACE_RESIZER_HEIGHT_PX = 14;

const DEFAULT_MAP_SHARE_PERCENT = 50;
const MIN_MAP_SHARE_PERCENT = 25;
const MAX_MAP_SHARE_PERCENT = 75;
const MAP_SHARE_KEYBOARD_STEP = 4;
const VISUAL_RESIZER_WIDTH_PX = 14;

const SETUPS_EXPORT_VERSION = 1;

type SetupsExport = {
  version: number;
  setups: Array<{ name: string; spec: GenerationSpec }>;
};

function parseSetupsExport(text: string) {
  let parsed: unknown;
  try {
    parsed = JSON.parse(text);
  } catch {
    return null;
  }
  if (
    typeof parsed !== "object" ||
    parsed === null ||
    (parsed as { version?: unknown }).version !== SETUPS_EXPORT_VERSION ||
    !Array.isArray((parsed as { setups?: unknown }).setups)
  ) {
    return null;
  }
  return (parsed as { setups: unknown[] }).setups;
}

function isImportableSetup(
  entry: unknown,
): entry is { name: string; spec: Partial<GenerationSpec> } {
  if (typeof entry !== "object" || entry === null) return false;
  const candidate = entry as { name?: unknown; spec?: unknown };
  return (
    typeof candidate.name === "string" &&
    candidate.name.trim() !== "" &&
    typeof candidate.spec === "object" &&
    candidate.spec !== null &&
    !Array.isArray(candidate.spec)
  );
}

const ADJACENT_GRID_SIZES = Array.from(
  { length: MAX_SUPER_TILE_SIDE },
  (_, index) => index + 1,
);

function oddSuperTileSize(value: number) {
  if (value % 2 === 1) return value;
  return value >= MAX_SUPER_TILE_SIDE ? MAX_SUPER_TILE_SIDE - 1 : value + 1;
}

export function TerrainStudio() {
  const [spec, setSpec] = useState(initialSpec);
  const seededInitialSetup = useRef(false);
  const [appVersion, setAppVersion] = useState(APP_VERSION);
  const [availableUpdate, setAvailableUpdate] =
    useState<AvailableUpdate | null>(null);
  const [signedUpdateVersion, setSignedUpdateVersion] = useState<string | null>(
    null,
  );
  const [updateInstallState, setUpdateInstallState] = useState<
    | { phase: "idle" }
    | { phase: "checking" }
    | { phase: "downloading"; percent: number | null }
    | { phase: "installing" }
    | { phase: "error"; message: string }
  >({ phase: "idle" });
  const [updateDismissed, setUpdateDismissed] = useState(false);
  const [visualHeightPercent, setVisualHeightPercent] = useState(
    DEFAULT_VISUAL_HEIGHT_PERCENT,
  );
  const [activeSection, setActiveSection] = useState<
    | "model"
    | "surface"
    | "colors"
    | "buildings"
    | "markers"
    | "mounting"
    | "output"
  >("model");
  const [markerPlacementKind, setMarkerPlacementKind] =
    useState<MarkerKind | null>(null);
  const [job, setJob] = useState<Job | null>(null);
  const [generatedPreview, setGeneratedPreview] =
    useState<PreviewData | null>(null);
  const [elevationPreview, setElevationPreview] =
    useState<PreviewData | null>(null);
  const [previewLoading, setPreviewLoading] = useState(false);
  const [submitting, setSubmitting] = useState(false);
  const [canceling, setCanceling] = useState(false);
  const [message, setMessage] = useState<string | null>(null);
  const [artifactFeedback, setArtifactFeedback] =
    useState<ArtifactFeedback | null>(null);
  const [placeQuery, setPlaceQuery] = useState("");
  const [placeResults, setPlaceResults] = useState<PlaceResult[]>([]);
  const [placeMessage, setPlaceMessage] = useState<string | null>(null);
  const [adjacentMessage, setAdjacentMessage] = useState<string | null>(null);
  const [searchingPlaces, setSearchingPlaces] = useState(false);
  const [mapSharePercent, setMapSharePercent] = useState(
    DEFAULT_MAP_SHARE_PERCENT,
  );
  const [setups, setSetups] = useState<SavedSetup[]>([]);
  const [selectedSetupId, setSelectedSetupId] = useState("");
  const [setupMenuOpen, setSetupMenuOpen] = useState(false);
  const [setupNameMode, setSetupNameMode] = useState<
    { kind: "save" } | { kind: "rename"; id: string } | null
  >(null);
  const [setupNameDraft, setSetupNameDraft] = useState("");
  const [setupStatus, setSetupStatus] = useState<string | null>(null);
  const [trailNotice, setTrailNotice] = useState<string | null>(null);
  const [savingSetup, setSavingSetup] = useState(false);
  const [confirmingSetupDeleteId, setConfirmingSetupDeleteId] = useState<
    string | null
  >(null);
  const workspaceRef = useRef<HTMLDivElement>(null);
  const resizePointerRef = useRef<number | null>(null);
  const visualColumnRef = useRef<HTMLElement>(null);
  const visualResizePointerRef = useRef<number | null>(null);
  const setupImportRef = useRef<HTMLInputElement>(null);
  const setupMenuRef = useRef<HTMLDivElement>(null);
  const setupMenuButtonRef = useRef<HTMLButtonElement>(null);
  const setupNameInputRef = useRef<HTMLInputElement>(null);
  // "__first" focuses the first enabled item; a setup id focuses that row.
  const setupFocusRef = useRef<string | null>(null);
  const skipSetupNameBlurRef = useRef(false);

  useEffect(() => {
    if (seededInitialSetup.current) return;
    seededInitialSetup.current = true;
    // Browser entropy gives each new setup its own saved seed. The mesh code
    // uses only fixed integer math after this point.
    setSpec((current) => ({ ...current, puzzle_seed: randomPuzzleSeed() }));
  }, []);

  useEffect(() => {
    if (!IS_TAURI) return;
    const controller = new AbortController();

    void (async () => {
      let installedVersion = APP_VERSION;
      try {
        const { getVersion } = await import("@tauri-apps/api/app");
        installedVersion = await getVersion();
      } catch {
        // The bundled config remains a safe fallback if the app API is unavailable.
      }
      if (controller.signal.aborted) return;
      setAppVersion(installedVersion);

      const [noticeResult, signedResult] = await Promise.allSettled([
        fetchAvailableUpdate(installedVersion, controller.signal),
        checkSignedUpdateVersion(),
      ]);
      if (controller.signal.aborted) return;

      const notice =
        noticeResult.status === "fulfilled" ? noticeResult.value : null;
      const signedVersion =
        signedResult.status === "fulfilled" ? signedResult.value : null;
      setSignedUpdateVersion(signedVersion);
      if (
        signedVersion &&
        isVersionNewer(signedVersion, installedVersion) &&
        (!notice || isVersionNewer(signedVersion, notice.version))
      ) {
        setAvailableUpdate(signedUpdateFallback(signedVersion));
      } else {
        setAvailableUpdate(notice);
      }
    })();

    return () => controller.abort();
  }, []);

  const setVisualHeightFromPointer = useCallback((clientY: number) => {
    const bounds = workspaceRef.current?.getBoundingClientRect();
    if (!bounds || bounds.height <= WORKSPACE_RESIZER_HEIGHT_PX) return;
    const usableHeight = bounds.height - WORKSPACE_RESIZER_HEIGHT_PX;
    const previewHeight =
      clientY - bounds.top - WORKSPACE_RESIZER_HEIGHT_PX / 2;
    const nextPercent = (previewHeight / usableHeight) * 100;
    setVisualHeightPercent(
      Math.min(
        MAX_VISUAL_HEIGHT_PERCENT,
        Math.max(MIN_VISUAL_HEIGHT_PERCENT, nextPercent),
      ),
    );
  }, []);

  const resizePointerDown = useCallback(
    (event: ReactPointerEvent<HTMLDivElement>) => {
      if (event.button !== 0) return;
      event.preventDefault();
      resizePointerRef.current = event.pointerId;
      event.currentTarget.setPointerCapture(event.pointerId);
      setVisualHeightFromPointer(event.clientY);
    },
    [setVisualHeightFromPointer],
  );

  const resizePointerMove = useCallback(
    (event: ReactPointerEvent<HTMLDivElement>) => {
      if (resizePointerRef.current !== event.pointerId) return;
      setVisualHeightFromPointer(event.clientY);
    },
    [setVisualHeightFromPointer],
  );

  const resizePointerUp = useCallback(
    (event: ReactPointerEvent<HTMLDivElement>) => {
      if (resizePointerRef.current !== event.pointerId) return;
      setVisualHeightFromPointer(event.clientY);
      resizePointerRef.current = null;
      if (event.currentTarget.hasPointerCapture(event.pointerId)) {
        event.currentTarget.releasePointerCapture(event.pointerId);
      }
    },
    [setVisualHeightFromPointer],
  );

  const resizeKeyDown = useCallback(
    (event: ReactKeyboardEvent<HTMLDivElement>) => {
      let nextPercent: number | null = null;
      if (event.key === "ArrowUp") {
        nextPercent = visualHeightPercent - VISUAL_HEIGHT_KEYBOARD_STEP;
      } else if (event.key === "ArrowDown") {
        nextPercent = visualHeightPercent + VISUAL_HEIGHT_KEYBOARD_STEP;
      } else if (event.key === "Home") {
        nextPercent = MIN_VISUAL_HEIGHT_PERCENT;
      } else if (event.key === "End") {
        nextPercent = MAX_VISUAL_HEIGHT_PERCENT;
      }
      if (nextPercent === null) return;
      event.preventDefault();
      setVisualHeightPercent(
        Math.min(
          MAX_VISUAL_HEIGHT_PERCENT,
          Math.max(MIN_VISUAL_HEIGHT_PERCENT, nextPercent),
        ),
      );
    },
    [visualHeightPercent],
  );

  const setMapShareFromPointer = useCallback((clientX: number) => {
    const bounds = visualColumnRef.current?.getBoundingClientRect();
    if (!bounds || bounds.width <= VISUAL_RESIZER_WIDTH_PX) return;
    const usableWidth = bounds.width - VISUAL_RESIZER_WIDTH_PX;
    const mapWidth = clientX - bounds.left - VISUAL_RESIZER_WIDTH_PX / 2;
    const nextPercent = (mapWidth / usableWidth) * 100;
    setMapSharePercent(
      Math.min(
        MAX_MAP_SHARE_PERCENT,
        Math.max(MIN_MAP_SHARE_PERCENT, nextPercent),
      ),
    );
  }, []);

  const visualResizePointerDown = useCallback(
    (event: ReactPointerEvent<HTMLDivElement>) => {
      if (event.button !== 0) return;
      event.preventDefault();
      visualResizePointerRef.current = event.pointerId;
      event.currentTarget.setPointerCapture(event.pointerId);
      setMapShareFromPointer(event.clientX);
    },
    [setMapShareFromPointer],
  );

  const visualResizePointerMove = useCallback(
    (event: ReactPointerEvent<HTMLDivElement>) => {
      if (visualResizePointerRef.current !== event.pointerId) return;
      setMapShareFromPointer(event.clientX);
    },
    [setMapShareFromPointer],
  );

  const visualResizePointerUp = useCallback(
    (event: ReactPointerEvent<HTMLDivElement>) => {
      if (visualResizePointerRef.current !== event.pointerId) return;
      setMapShareFromPointer(event.clientX);
      visualResizePointerRef.current = null;
      if (event.currentTarget.hasPointerCapture(event.pointerId)) {
        event.currentTarget.releasePointerCapture(event.pointerId);
      }
    },
    [setMapShareFromPointer],
  );

  const visualResizeKeyDown = useCallback(
    (event: ReactKeyboardEvent<HTMLDivElement>) => {
      let nextPercent: number | null = null;
      if (event.key === "ArrowLeft") {
        nextPercent = mapSharePercent - MAP_SHARE_KEYBOARD_STEP;
      } else if (event.key === "ArrowRight") {
        nextPercent = mapSharePercent + MAP_SHARE_KEYBOARD_STEP;
      } else if (event.key === "Home") {
        nextPercent = MIN_MAP_SHARE_PERCENT;
      } else if (event.key === "End") {
        nextPercent = MAX_MAP_SHARE_PERCENT;
      }
      if (nextPercent === null) return;
      event.preventDefault();
      setMapSharePercent(
        Math.min(
          MAX_MAP_SHARE_PERCENT,
          Math.max(MIN_MAP_SHARE_PERCENT, nextPercent),
        ),
      );
    },
    [mapSharePercent],
  );

  const update = useCallback(
    <Key extends keyof GenerationSpec>(key: Key, value: GenerationSpec[Key]) => {
      setGeneratedPreview(null);
      setSpec((current) => {
        if (key !== "base_mm") return { ...current, [key]: value };
        const baseMm = value as number;
        return {
          ...current,
          [key]: value,
          marker_settings: {
            ...current.marker_settings,
            hole_depth_mm: Math.min(
              current.marker_settings.hole_depth_mm,
              Math.max(0.6, baseMm - 0.4),
            ),
          },
        };
      });
    },
    [],
  );
  const setPieceLayout = useCallback((count: number) => {
    setGeneratedPreview(null);
    setSpec((current) => ({
      ...current,
      rows: count,
      columns: count,
    }));
  }, []);
  const setMeshQuality = useCallback((samples: number) => {
    setGeneratedPreview(null);
    setSpec((current) => ({
      ...current,
      mesh_samples_across: samples,
      overlay_samples_across: samples,
      fine_dem_detail:
        samples === MAX_ASSEMBLED_SAMPLES ? false : current.fine_dem_detail,
    }));
  }, []);
  const updateColor = useCallback(
    <Key extends keyof GenerationSpec["color_output"]>(
      key: Key,
      value: GenerationSpec["color_output"][Key],
    ) => {
      setGeneratedPreview(null);
      setSpec((current) => {
        const colorOutput = normalizeMappedWidthCap({
          ...current.color_output,
          [key]: value,
        });
        return { ...current, color_output: colorOutput };
      });
    },
    [],
  );
  const updateTray = useCallback(
    <Key extends keyof GenerationSpec["tray"]>(
      key: Key,
      value: GenerationSpec["tray"][Key],
    ) => {
      setGeneratedPreview(null);
      setSpec((current) => ({
        ...current,
        tray: { ...current.tray, [key]: value },
      }));
    },
    [],
  );
  const updateWallMount = useCallback(
    <Key extends keyof GenerationSpec["wall_mount"]>(
      key: Key,
      value: GenerationSpec["wall_mount"][Key],
    ) => {
      setGeneratedPreview(null);
      setSpec((current) => ({
        ...current,
        wall_mount: { ...current.wall_mount, [key]: value },
      }));
    },
    [],
  );
  const updatePuzzleRetention = useCallback(
    <Key extends keyof GenerationSpec["puzzle_retention"]>(
      key: Key,
      value: GenerationSpec["puzzle_retention"][Key],
    ) => {
      setGeneratedPreview(null);
      setSpec((current) => ({
        ...current,
        puzzle_retention: { ...current.puzzle_retention, [key]: value },
      }));
    },
    [],
  );
  const updateBuildings = useCallback(
    <Key extends keyof GenerationSpec["buildings"]>(
      key: Key,
      value: GenerationSpec["buildings"][Key],
    ) => {
      setGeneratedPreview(null);
      setSpec((current) => ({
        ...current,
        buildings: { ...current.buildings, [key]: value },
      }));
    },
    [],
  );
  const updateMarkerSettings = useCallback(
    <Key extends keyof GenerationSpec["marker_settings"]>(
      key: Key,
      value: GenerationSpec["marker_settings"][Key],
    ) => {
      setGeneratedPreview(null);
      setSpec((current) => {
        const markerSettings = { ...current.marker_settings, [key]: value };
        if (key === "hole_diameter_mm") {
          markerSettings.flag_clearance_mm = Math.min(
            markerSettings.flag_clearance_mm,
            Math.max(0.1, (value as number) - 0.9),
          );
        }
        if (key === "flag_height_mm") {
          markerSettings.flag_label_height_mm = Math.min(
            markerSettings.flag_label_height_mm,
            Math.max(1.5, (value as number) - 2),
          );
        }
        return { ...current, marker_settings: markerSettings };
      });
    },
    [],
  );
  const addMarker = useCallback(
    (longitude: number, latitude: number) => {
      if (!markerPlacementKind) return;
      setGeneratedPreview(null);
      setSpec((current) => {
        const number = current.markers.length + 1;
        const label =
          markerPlacementKind === "building"
            ? "Building"
            : markerPlacementKind === "dot"
              ? "Point"
              : markerPlacementKind === "flag_label"
                ? "Flag label"
                : markerPlacementKind === "surface_label"
                  ? "Surface label"
                  : markerPlacementKind === "plaque_label"
                    ? "Plaque label"
                    : "Flag";
        return {
          ...current,
          buildings:
            markerPlacementKind === "building"
              ? { ...current.buildings, enabled: true }
              : current.buildings,
          marker_settings: {
            ...current.marker_settings,
            hole_depth_mm: Math.min(
              current.marker_settings.hole_depth_mm,
              Math.max(0.6, current.base_mm - 0.4),
            ),
          },
          markers: [
            ...current.markers,
            {
              kind: markerPlacementKind,
              latitude,
              longitude,
              name: `${label} ${number}`,
              label_height_mm: 4,
              rotation_degrees: 0,
            },
          ].slice(0, 50),
        };
      });
      setMarkerPlacementKind(null);
    },
    [markerPlacementKind],
  );
  const updateMarker = useCallback(
    (index: number, patch: Partial<GenerationSpec["markers"][number]>) => {
      setGeneratedPreview(null);
      setSpec((current) => ({
        ...current,
        buildings:
          patch.kind === "building"
            ? { ...current.buildings, enabled: true }
            : current.buildings,
        markers: current.markers.map((marker, position) =>
          position === index ? { ...marker, ...patch } : marker,
        ),
      }));
    },
    [],
  );
  const removeMarker = useCallback((index: number) => {
    setGeneratedPreview(null);
    setSpec((current) => ({
      ...current,
      markers: current.markers.filter((_, position) => position !== index),
    }));
  }, []);
  const importTrailFiles = async (files: File[]) => {
    if (files.length === 0) return;
    const notices: string[] = [];
    const imported: TrailRoute[] = [];
    for (const file of files) {
      if (file.size > MAX_TRAIL_FILE_BYTES) {
        notices.push(
          `${file.name} is larger than the 32 MB trail import limit.`,
        );
        continue;
      }
      try {
        const parsed = parseTrailFile(file.name, await file.text());
        if (parsed.trails.length === 0) {
          notices.push(`No trails found in ${file.name}.`);
        }
        imported.push(...parsed.trails);
        for (const name of parsed.downsampled) {
          notices.push(
            `${name} was thinned to ${MAX_TRAIL_POINTS.toLocaleString()} points.`,
          );
        }
      } catch {
        notices.push(`Could not read ${file.name}.`);
      }
    }
    const kept = imported.slice(
      0,
      Math.max(0, MAX_TRAILS - spec.trails.length),
    );
    if (imported.length > kept.length) {
      notices.push(`A model holds at most ${MAX_TRAILS} trails.`);
    }
    if (kept.length > 0) {
      notices.unshift(
        `Imported ${kept.length} ${kept.length === 1 ? "trail" : "trails"}.`,
      );
    }
    if (imported.length > 0) {
      // Merge inside the updater: the awaits above span renders, so two
      // overlapping imports would otherwise each merge from a stale
      // spec.trails and one would drop the other's trails.
      setGeneratedPreview(null);
      setSpec((current) => ({
        ...current,
        trails: [...current.trails, ...imported].slice(0, MAX_TRAILS),
      }));
    }
    setTrailNotice(notices.length > 0 ? notices.join(" ") : null);
  };
  const removeTrail = (index: number) => {
    update(
      "trails",
      spec.trails.filter((_, position) => position !== index),
    );
    setTrailNotice(null);
  };

  const setSuperTileAnchor = useCallback(
    (anchor: GenerationSpec["super_tile_anchor"]) => {
      const columns =
        anchor === "center"
          ? oddSuperTileSize(spec.adjacent_columns)
          : spec.adjacent_columns;
      const rows =
        anchor === "center"
          ? oddSuperTileSize(spec.adjacent_rows)
          : spec.adjacent_rows;
      setGeneratedPreview(null);
      setSpec((current) => ({
        ...current,
        adjacent_columns: columns,
        adjacent_rows: rows,
        super_tile_anchor: anchor,
      }));
      setAdjacentMessage(
        anchor === "center"
          ? columns !== spec.adjacent_columns || rows !== spec.adjacent_rows
            ? `Center anchor needs a center tile, so the grid changed to ${columns} × ${rows}.`
            : "The selected map point is the center tile."
          : "The selected map point is the top-left tile.",
      );
    },
    [spec.adjacent_columns, spec.adjacent_rows],
  );

  const onCenterChange = useCallback((longitude: number, latitude: number) => {
    setGeneratedPreview(null);
    setAdjacentMessage(null);
    setSpec((current) => ({
      ...current,
      center_lat: Number(latitude.toFixed(5)),
      center_lon: Number(longitude.toFixed(5)),
    }));
  }, []);

  const lockHeightFrame = useCallback(() => {
    const sampled = generatedPreview ?? elevationPreview;
    if (
      sampled?.minimum_elevation_m === undefined ||
      sampled.maximum_elevation_m === undefined
    ) {
      setAdjacentMessage("Wait for the elevation sample, then lock the height frame.");
      return false;
    }
    const { datum, metresPerMm } = deriveHeightFrame(
      {
        minimum_elevation_m: sampled.minimum_elevation_m,
        maximum_elevation_m: sampled.maximum_elevation_m,
      },
      spec.relief_mm,
    );
    setSpec((current) => ({
      ...current,
      elevation_datum_m: datum,
      elevation_m_per_mm: Number(metresPerMm.toFixed(4)),
    }));
    setAdjacentMessage(
      `Height frame locked at ${datum.toFixed(1)} m with ${metresPerMm.toFixed(1)} m/mm.`,
    );
    return true;
  }, [elevationPreview, generatedPreview, spec.relief_mm]);

  const unlockHeightFrame = useCallback(() => {
    setGeneratedPreview(null);
    setSpec((current) => ({
      ...current,
      elevation_datum_m: null,
      elevation_m_per_mm: null,
    }));
    setAdjacentMessage(
      "Each tile will now use its own height range and may not meet its neighbors.",
    );
  }, []);

  const moveToAdjacentTile = useCallback(
    (direction: AdjacentDirection) => {
      const sampled = generatedPreview ?? elevationPreview;
      let datum = spec.elevation_datum_m;
      let metresPerMm = spec.elevation_m_per_mm;
      if (datum === null || metresPerMm === null) {
        if (
          sampled?.minimum_elevation_m === undefined ||
          sampled.maximum_elevation_m === undefined
        ) {
          setAdjacentMessage(
            "Wait for the elevation sample before moving so the two tiles can share a height frame.",
          );
          return;
        }
        const derived = deriveHeightFrame(
          {
            minimum_elevation_m: sampled.minimum_elevation_m,
            maximum_elevation_m: sampled.maximum_elevation_m,
          },
          spec.relief_mm,
        );
        datum = derived.datum;
        metresPerMm = Number(derived.metresPerMm.toFixed(4));
      }
      const next = adjacentCenter(
        spec.center_lat,
        spec.center_lon,
        spec.ground_span_km,
        direction,
      );
      setGeneratedPreview(null);
      setElevationPreview(null);
      setSpec((current) => ({
        ...current,
        center_lat: Number(next.latitude.toFixed(5)),
        center_lon: Number(next.longitude.toFixed(5)),
        elevation_datum_m: datum,
        elevation_m_per_mm: metresPerMm,
        puzzle_tile_column:
          current.puzzle_tile_column +
          (direction === "east" ? 1 : direction === "west" ? -1 : 0),
        puzzle_tile_row:
          current.puzzle_tile_row +
          (direction === "south" ? 1 : direction === "north" ? -1 : 0),
      }));
      setAdjacentMessage(
        `Moved ${direction} by one tile. The shared height frame stays locked.`,
      );
    },
    [
      elevationPreview,
      generatedPreview,
      spec.center_lat,
      spec.center_lon,
      spec.elevation_datum_m,
      spec.elevation_m_per_mm,
      spec.ground_span_km,
      spec.relief_mm,
    ],
  );

  useEffect(() => {
    const controller = new AbortController();
    const timer = window.setTimeout(async () => {
      setElevationPreview(null);
      setPreviewLoading(true);
      const previewSpec: GenerationSpec = {
        ...initialSpec,
        center_lat: spec.center_lat,
        center_lon: spec.center_lon,
        elevation_source: spec.elevation_source,
        ground_span_km: spec.ground_span_km,
        width_mm: spec.width_mm,
        base_mm: spec.base_mm,
        relief_mm: spec.relief_mm,
        samples_per_piece: spec.samples_per_piece,
        overlay_samples_per_piece: spec.overlay_samples_per_piece,
        mesh_samples_across: spec.mesh_samples_across,
        overlay_samples_across: spec.overlay_samples_across,
        fine_dem_detail: spec.fine_dem_detail,
        despike_terrain: spec.despike_terrain,
        elevation_datum_m: spec.elevation_datum_m,
        elevation_m_per_mm: spec.elevation_m_per_mm,
        color_output: {
          ...initialSpec.color_output,
          enabled: false,
          roads_enabled: false,
          osm_water_enabled: false,
        },
        buildings: { ...initialSpec.buildings, enabled: false },
        tray: { ...initialSpec.tray, enabled: false },
      };
      try {
        setElevationPreview(
          await terrainApi.preview(previewSpec, controller.signal),
        );
      } catch (error) {
        if (!(error instanceof DOMException && error.name === "AbortError")) {
          setElevationPreview(null);
        }
      } finally {
        if (!controller.signal.aborted) setPreviewLoading(false);
      }
    }, 450);
    return () => {
      window.clearTimeout(timer);
      controller.abort();
    };
  }, [
    spec.base_mm,
    spec.center_lat,
    spec.center_lon,
    spec.elevation_datum_m,
    spec.elevation_m_per_mm,
    spec.despike_terrain,
    spec.elevation_source,
    spec.fine_dem_detail,
    spec.ground_span_km,
    spec.mesh_samples_across,
    spec.overlay_samples_per_piece,
    spec.overlay_samples_across,
    spec.relief_mm,
    spec.samples_per_piece,
    spec.width_mm,
  ]);

  const searchPlaces = async () => {
    const query = placeQuery.trim();
    if (query.length < 2) {
      setPlaceMessage("Enter at least two characters.");
      setPlaceResults([]);
      return;
    }
    setSearchingPlaces(true);
    setPlaceMessage(null);
    try {
      const results = await terrainApi.searchPlaces(query);
      setPlaceResults(results);
      if (results.length === 0) {
        setPlaceMessage("No matching places found.");
      }
    } catch (error) {
      setPlaceResults([]);
      setPlaceMessage(
        error instanceof Error ? error.message : "Place search failed.",
      );
    } finally {
      setSearchingPlaces(false);
    }
  };

  const choosePlace = (place: PlaceResult) => {
    onCenterChange(place.longitude, place.latitude);
    setPlaceQuery(place.display_name);
    update(
      "place_name",
      limitPlaceName(place.display_name.split(",")[0].trim()),
    );
    setPlaceResults([]);
    setPlaceMessage(`Map moved to ${place.display_name.split(",")[0]}.`);
    setGeneratedPreview(null);
  };

  const refreshSetups = useCallback(async (signal?: AbortSignal) => {
    try {
      const nextSetups = await terrainApi.listSetups(signal);
      if (signal?.aborted) return;
      setSetups(nextSetups);
    } catch {
      // The picker stays as it was when the service is unreachable.
    }
  }, []);

  useEffect(() => {
    const controller = new AbortController();
    void (async () => {
      await refreshSetups(controller.signal);
    })();
    return () => controller.abort();
  }, [refreshSetups]);

  const defaultSetupName = spec.place_name.trim() || "Saved setup";
  const selectedSetup =
    setups.find((candidate) => candidate.id === selectedSetupId) ?? null;

  const closeSetupMenu = useCallback((focusButton: boolean) => {
    setSetupMenuOpen(false);
    setSetupNameMode(null);
    setConfirmingSetupDeleteId(null);
    if (focusButton) setupMenuButtonRef.current?.focus();
  }, []);

  const dismissSetupMenu = useCallback(
    () => closeSetupMenu(false),
    [closeSetupMenu],
  );
  useOutsideDismiss(setupMenuRef, setupMenuOpen, dismissSetupMenu);

  useEffect(() => {
    if (!setupMenuOpen) return;
    setupMenuRef.current
      ?.querySelector<HTMLButtonElement>('[role="menuitem"]:enabled')
      ?.focus();
  }, [setupMenuOpen]);

  useEffect(() => {
    if (setupNameMode === null) return;
    setupNameInputRef.current?.focus();
    setupNameInputRef.current?.select();
  }, [setupNameMode]);

  // Runs after every render so freshly refreshed rows can take focus.
  useEffect(() => {
    const target = setupFocusRef.current;
    if (target === null) return;
    setupFocusRef.current = null;
    const root = setupMenuRef.current;
    if (!root) return;
    const element =
      target === "__first"
        ? root.querySelector<HTMLButtonElement>('[role="menuitem"]:enabled')
        : (root.querySelector<HTMLButtonElement>(
            `[data-setup-button="${target}"]`,
          ) ??
          root.querySelector<HTMLButtonElement>('[role="menuitem"]:enabled'));
    element?.focus();
  });

  const setupMenuKeyDown = (event: ReactKeyboardEvent<HTMLDivElement>) => {
    if (!setupMenuOpen) return;
    if (event.key === "Escape") {
      event.preventDefault();
      event.stopPropagation();
      if (setupNameMode !== null) {
        skipSetupNameBlurRef.current = true;
        setupFocusRef.current =
          setupNameMode.kind === "rename" ? setupNameMode.id : "__first";
        setSetupNameMode(null);
        return;
      }
      closeSetupMenu(true);
      return;
    }
    if (event.key === "Tab") {
      closeSetupMenu(false);
      return;
    }
    if (event.target instanceof HTMLInputElement) return;
    if (!["ArrowDown", "ArrowUp", "Home", "End"].includes(event.key)) return;
    const items = Array.from(
      setupMenuRef.current?.querySelectorAll<HTMLButtonElement>(
        '[role="menuitem"]:enabled',
      ) ?? [],
    );
    if (items.length === 0) return;
    event.preventDefault();
    const activeIndex = items.findIndex(
      (item) => item === document.activeElement,
    );
    const nextIndex =
      event.key === "Home" || (event.key === "ArrowDown" && activeIndex === -1)
        ? 0
        : event.key === "End"
          ? items.length - 1
          : event.key === "ArrowDown"
            ? (activeIndex + 1) % items.length
            : activeIndex <= 0
              ? items.length - 1
              : activeIndex - 1;
    items[nextIndex]?.focus();
  };

  const openSaveRow = () => {
    setConfirmingSetupDeleteId(null);
    skipSetupNameBlurRef.current = false;
    setSetupNameDraft(selectedSetup?.name ?? spec.place_name.trim());
    setSetupNameMode({ kind: "save" });
  };

  const openRenameRow = (setup: SavedSetup) => {
    setConfirmingSetupDeleteId(null);
    skipSetupNameBlurRef.current = false;
    setSetupNameDraft(setup.name);
    setSetupNameMode({ kind: "rename", id: setup.id });
  };

  const saveSetupAs = async (name: string) => {
    setSavingSetup(true);
    try {
      const { setup: saved, created } = await terrainApi.saveSetup(name, spec);
      setSelectedSetupId(saved.id);
      setSetupStatus(
        created ? `Saved “${saved.name}”.` : `Replaced “${saved.name}”.`,
      );
      await refreshSetups();
      closeSetupMenu(true);
    } catch (error) {
      setSetupStatus(
        error instanceof Error ? error.message : "The setup was not saved.",
      );
    } finally {
      setSavingSetup(false);
    }
  };

  const renameSetup = async (id: string, name: string) => {
    setSavingSetup(true);
    try {
      const renamed = await terrainApi.renameSetup(id, name);
      setSetupStatus(`Renamed to “${renamed.name}”.`);
      await refreshSetups();
      skipSetupNameBlurRef.current = true;
      setupFocusRef.current = id;
      setSetupNameMode(null);
    } catch (error) {
      // A conflict (409) or other failure keeps the input open so the
      // name can be corrected; the status line carries the message.
      setSetupStatus(
        error instanceof Error ? error.message : "The setup was not renamed.",
      );
    } finally {
      setSavingSetup(false);
    }
  };

  const submitSetupName = async () => {
    if (savingSetup || setupNameMode === null) return;
    const trimmed = setupNameDraft.trim();
    if (setupNameMode.kind === "rename") {
      if (trimmed === "") return;
      const current = setups.find(
        (candidate) => candidate.id === setupNameMode.id,
      );
      if (current && current.name === trimmed) {
        // Nothing changed; leave the name as it is.
        skipSetupNameBlurRef.current = true;
        setupFocusRef.current = setupNameMode.id;
        setSetupNameMode(null);
        return;
      }
      await renameSetup(setupNameMode.id, trimmed);
      return;
    }
    await saveSetupAs(trimmed === "" ? defaultSetupName : trimmed);
  };

  const setupNameBlur = () => {
    if (skipSetupNameBlurRef.current) {
      skipSetupNameBlurRef.current = false;
      return;
    }
    if (setupNameMode?.kind !== "rename") return;
    void submitSetupName();
  };

  const recallSetup = (setup: SavedSetup) => {
    setSelectedSetupId(setup.id);
    // Merge over the client defaults so setups saved before a field existed
    // still get a value, then drop stale generated output like a place change.
    setSpec(mergeSpecDefaults(setup.spec));
    setGeneratedPreview(null);
    setAdjacentMessage(null);
    setSetupStatus(`Recalled “${setup.name}”.`);
    closeSetupMenu(true);
  };

  const duplicateSetup = async (setup: SavedSetup) => {
    if (savingSetup) return;
    setConfirmingSetupDeleteId(null);
    setSavingSetup(true);
    try {
      const names = new Set(setups.map((candidate) => candidate.name));
      let copy = 2;
      while (names.has(`${setup.name} (${copy})`)) copy += 1;
      const { setup: saved } = await terrainApi.saveSetup(
        `${setup.name} (${copy})`,
        setup.spec,
      );
      setSetupStatus(`Duplicated “${setup.name}” as “${saved.name}”.`);
      await refreshSetups();
      // Drop the new row straight into rename mode so it can be retitled.
      skipSetupNameBlurRef.current = false;
      setSetupNameDraft(saved.name);
      setSetupNameMode({ kind: "rename", id: saved.id });
    } catch (error) {
      setSetupStatus(
        error instanceof Error ? error.message : "The setup was not copied.",
      );
    } finally {
      setSavingSetup(false);
    }
  };

  const deleteSetup = async (setup: SavedSetup) => {
    if (confirmingSetupDeleteId !== setup.id) {
      setSetupNameMode(null);
      setConfirmingSetupDeleteId(setup.id);
      return;
    }
    setConfirmingSetupDeleteId(null);
    try {
      await terrainApi.deleteSetup(setup.id);
      if (selectedSetupId === setup.id) setSelectedSetupId("");
      setSetupStatus(`Deleted “${setup.name}”.`);
      await refreshSetups();
      setupFocusRef.current = "__first";
    } catch (error) {
      setSetupStatus(
        error instanceof Error ? error.message : "The setup was not deleted.",
      );
    }
  };

  const exportSetups = () => {
    closeSetupMenu(true);
    const payload: SetupsExport = {
      version: SETUPS_EXPORT_VERSION,
      setups: setups.map(({ name, spec: savedSpec }) => ({
        name,
        spec: savedSpec,
      })),
    };
    // A plain blob download works in browsers and in the Tauri webview; the
    // native save_artifact command only copies files a job already produced.
    const blob = new Blob([JSON.stringify(payload, null, 2)], {
      type: "application/json",
    });
    const url = URL.createObjectURL(blob);
    const anchor = document.createElement("a");
    anchor.href = url;
    anchor.download = "toposaic-setups.json";
    document.body.append(anchor);
    anchor.click();
    anchor.remove();
    URL.revokeObjectURL(url);
    setSetupStatus(
      `Exported ${setups.length} ${setups.length === 1 ? "setup" : "setups"}.`,
    );
  };

  const importSetups = async (file: File) => {
    setConfirmingSetupDeleteId(null);
    const entries = parseSetupsExport(await file.text());
    if (entries === null) {
      setSetupStatus("That file is not a version-1 TopoSaic setups export.");
      return;
    }
    let imported = 0;
    let skipped = 0;
    for (const entry of entries) {
      if (!isImportableSetup(entry)) {
        skipped += 1;
        continue;
      }
      try {
        await terrainApi.saveSetup(
          entry.name.trim(),
          mergeSpecDefaults(entry.spec),
        );
        imported += 1;
      } catch {
        skipped += 1;
      }
    }
    setSetupStatus(
      skipped > 0
        ? `Imported ${imported}, skipped ${skipped} invalid.`
        : `Imported ${imported} ${imported === 1 ? "setup" : "setups"}.`,
    );
    await refreshSetups();
  };

  useEffect(() => {
    if (!job || !["queued", "running"].includes(job.status)) return;
    const polledJobId = job.id;
    const controller = new AbortController();
    const timer = window.setInterval(async () => {
      try {
        const nextJob = await terrainApi.getJob(
          polledJobId,
          controller.signal,
        );
        if (nextJob.id !== polledJobId) return;
        // Fetch the finished preview before setJob: updating the job tears
        // this effect down and aborts the controller, which would cancel a
        // preview request started after the update.
        let previewData: PreviewData | null = null;
        if (nextJob.status === "complete") {
          previewData = await terrainApi
            .generatedPreview(nextJob.id, controller.signal)
            .catch(() => null);
          if (controller.signal.aborted) return;
        }
        setJob(nextJob);
        setMessage((current) =>
          current === GENERATOR_STALLED_MESSAGE ? null : current,
        );
        if (previewData) {
          setGeneratedPreview(previewData);
        }
      } catch (error) {
        if (!(error instanceof DOMException && error.name === "AbortError")) {
          setMessage(GENERATOR_STALLED_MESSAGE);
        }
      }
    }, 900);
    return () => {
      window.clearInterval(timer);
      controller.abort();
    };
  }, [job]);

  const submit = async (event: FormEvent) => {
    event.preventDefault();
    if (submitting || generationActive) return;
    setSubmitting(true);
    setMessage(null);
    setJob(null);
    setArtifactFeedback(null);
    setGeneratedPreview(null);
    try {
      setJob(await terrainApi.createJob(spec));
      setActiveSection("output");
    } catch (error) {
      setActiveSection("output");
      setMessage(
        error instanceof TypeError
          ? "Start the local Rust generator, then try again."
          : error instanceof Error
            ? error.message
            : "Generation could not start.",
      );
    } finally {
      setSubmitting(false);
    }
  };

  const cancelGeneration = async () => {
    setActiveSection("output");
    setMessage(null);
    if (!job || !["queued", "running"].includes(job.status)) return;

    setCanceling(true);
    try {
      setJob(await terrainApi.cancelJob(job.id));
      setGeneratedPreview(null);
      setMessage("Generation canceled.");
    } catch (error) {
      setMessage(
        error instanceof Error
          ? error.message
          : "Generation could not be canceled.",
      );
    } finally {
      setCanceling(false);
    }
  };

  const installUpdate = async () => {
    setUpdateInstallState({ phase: "checking" });
    try {
      const installedVersion = await downloadAndInstallSignedUpdate(
        (progress) => {
          setUpdateInstallState(
            progress.phase === "installing"
              ? { phase: "installing" }
              : {
                  phase: "downloading",
                  percent: progress.percent,
                },
          );
        },
      );
      if (!installedVersion) {
        throw new Error("The signed update is no longer available.");
      }
    } catch {
      setUpdateInstallState({
        phase: "error",
        message: "Install failed. You can still download the release.",
      });
    }
  };

  const saveDesktopArtifact = async (artifact: Artifact) => {
    if (!job || !IS_TAURI) return;
    setArtifactFeedback({ name: artifact.name, state: "saving" });
    try {
      const { invoke } = await import("@tauri-apps/api/core");
      const savedBytes = await invoke<number | null>("save_artifact", {
        jobId: job.id,
        artifactName: artifact.name,
      });
      if (savedBytes === null) {
        setArtifactFeedback(null);
        setMessage("Save canceled.");
        return;
      }
      setArtifactFeedback({ name: artifact.name, state: "saved" });
      setMessage(`Saved ${artifact.name}.`);
    } catch (error) {
      setArtifactFeedback(null);
      setMessage(
        error instanceof Error
          ? error.message
          : typeof error === "string"
            ? error
            : `Could not save ${artifact.name}.`,
      );
    }
  };

  const noteWebDownload = (artifact: Artifact) => {
    setArtifactFeedback({ name: artifact.name, state: "sent" });
    setMessage(`Sent ${artifact.name} to your browser downloads.`);
  };

  const statusLabel = useMemo(() => {
    if (!job) return null;
    if (job.status === "complete") return "Your print files are ready.";
    if (job.status === "failed") return job.error ?? "Generation failed.";
    if (job.status === "canceled") return "Generation canceled.";
    if (job.status === "queued") return "Waiting for the generator…";
    if (job.progress < 8) return "Preparing source data…";
    if (job.progress < 40) {
      return "Sampling elevation and fetching source tiles…";
    }
    if (
      job.progress < 65 &&
      (job.spec.color_output.enabled ||
        job.spec.buildings.enabled ||
        job.spec.trails.length > 0 ||
        job.spec.markers.some((marker) => markerNeedsSurfaceData(marker.kind)))
    ) {
      // The backend runs the surface phase for trail-only jobs too.
      if (
        !job.spec.color_output.enabled &&
        !job.spec.buildings.enabled &&
        job.spec.markers.length === 0
      ) {
        return "Mapping imported trails…";
      }
      if (job.spec.buildings.enabled && !job.spec.color_output.enabled) {
        return "Downloading and mapping building footprints…";
      }
      if (job.spec.buildings.enabled) {
        return job.spec.color_output.roads_enabled
          ? "Downloading and mapping land cover, routes, and buildings…"
          : "Downloading and mapping land cover and buildings…";
      }
      return job.spec.color_output.roads_enabled
        ? "Downloading and mapping land cover, roads, or fallback trails…"
        : "Downloading and mapping forest, rock, snow, and water…";
    }
    if (job.progress >= 96) return "Writing preview and print files…";
    return job.spec.solid_model
      ? "Building one watertight terrain model…"
      : "Building watertight pieces…";
  }, [job]);

  const generationStages = useMemo(() => {
    if (!job) return [];
    // Trail-only jobs run the surface phase too (uses_trails on the
    // backend), so they get the same "Map details" stage.
    const hasSurface =
      job.spec.color_output.enabled ||
      job.spec.buildings.enabled ||
      job.spec.trails.length > 0 ||
      job.spec.markers.some((marker) => markerNeedsSurfaceData(marker.kind));
    const stages = [
      { key: "elevation", label: "Elevation", start: 0, end: 40 },
      ...(hasSurface
        ? [{ key: "surface", label: "Map details", start: 40, end: 65 }]
        : []),
      { key: "geometry", label: "Geometry", start: hasSurface ? 65 : 40, end: 99 },
      { key: "files", label: "Print files", start: 99, end: 100 },
    ];
    return stages.map((stage) => {
      const done = job.status === "complete" || job.progress >= stage.end;
      const stopped =
        ["failed", "canceled"].includes(job.status) &&
        job.progress >= stage.start &&
        !done;
      const active =
        ["queued", "running"].includes(job.status) &&
        job.progress >= stage.start &&
        !done;
      const localProgress = Math.round(
        ((job.progress - stage.start) / (stage.end - stage.start)) * 100,
      );
      return {
        ...stage,
        state: done ? "done" : stopped ? "stopped" : active ? "active" : "pending",
        detail: done
          ? stage.key === "files"
            ? "Ready"
            : "Done"
          : active
            ? job.status === "queued"
              ? "Queued"
              : `${Math.max(0, Math.min(100, localProgress))}%`
            : stopped
              ? job.status === "canceled"
                ? "Canceled"
                : "Failed"
            : "Next",
      };
    });
  }, [job]);

  const preview = generatedPreview ?? elevationPreview;
  const heightFrameLocked =
    spec.elevation_datum_m !== null && spec.elevation_m_per_mm !== null;
  const heightFrameCompatible =
    preview?.height_frame_compatible !== false;
  const superTileGridSizes =
    spec.super_tile_anchor === "center"
      ? ADJACENT_GRID_SIZES.filter((value) => value % 2 === 1)
      : ADJACENT_GRID_SIZES;
  const generationActive =
    job !== null && ["queued", "running"].includes(job.status);
  const cancellationActive = generationActive || canceling;
  const signedUpdateReady =
    availableUpdate !== null &&
    signedUpdateVersion !== null &&
    displayVersion(availableUpdate.version) ===
      displayVersion(signedUpdateVersion);
  const updateBusy = ["checking", "downloading", "installing"].includes(
    updateInstallState.phase,
  );
  const updateStatus =
    updateInstallState.phase === "checking"
      ? "Checking signed package…"
      : updateInstallState.phase === "downloading"
        ? updateInstallState.percent === null
          ? "Downloading update…"
          : `Downloading ${updateInstallState.percent}%…`
        : updateInstallState.phase === "installing"
          ? "Installing and restarting…"
          : updateInstallState.phase === "error"
            ? updateInstallState.message
            : availableUpdate?.urgency === "required"
              ? "This version is no longer supported."
              : `Current ${displayVersion(appVersion)}`;
  const previewState = generatedPreview
    ? "generated"
    : elevationPreview
      ? "elevation"
      : previewLoading
        ? "loading"
        : "shape";

  return (
    <main className="studio">
      <header className="topbar">
        <a className="brand" href="#" aria-label="TopoSaic home">
          <span className="brand-mark" aria-hidden="true" />
          <span>
            TopoSaic
            <small>
              Terrain Puzzle · <span>{displayVersion(appVersion)}</span>
            </small>
          </span>
        </a>
        <div className="topbar-actions">
          <div className="setup-manager">
            <div
              className="setup-menu"
              onKeyDown={setupMenuKeyDown}
              ref={setupMenuRef}
            >
              <button
                aria-expanded={setupMenuOpen}
                aria-haspopup="menu"
                className="setup-menu-button"
                onClick={() => {
                  if (setupMenuOpen) closeSetupMenu(true);
                  else setSetupMenuOpen(true);
                }}
                ref={setupMenuButtonRef}
                type="button"
              >
                <span className="setup-menu-label">
                  {selectedSetup?.name ?? "Saved setups"}
                </span>
                <span aria-hidden="true">▾</span>
              </button>
              {setupMenuOpen && (
                <div
                  aria-label="Saved setups"
                  className="setup-menu-list"
                  role="menu"
                >
                  {setups.length === 0 ? (
                    <p className="setup-menu-empty">No saved setups yet</p>
                  ) : (
                    <ul className="setup-rows" role="none">
                      {setups.map((setup) => {
                        const renaming =
                          setupNameMode?.kind === "rename" &&
                          setupNameMode.id === setup.id;
                        const confirming =
                          confirmingSetupDeleteId === setup.id;
                        return (
                          <li className="setup-row" key={setup.id} role="none">
                            {renaming ? (
                              <input
                                aria-label={`New name for ${setup.name}`}
                                className="setup-row-input"
                                maxLength={48}
                                onBlur={setupNameBlur}
                                onChange={(event) =>
                                  setSetupNameDraft(event.target.value)
                                }
                                onKeyDown={(event) => {
                                  if (event.key === "Enter") {
                                    event.preventDefault();
                                    void submitSetupName();
                                  }
                                }}
                                ref={setupNameInputRef}
                                type="text"
                                value={setupNameDraft}
                              />
                            ) : (
                              <button
                                aria-current={
                                  setup.id === selectedSetupId
                                    ? "true"
                                    : undefined
                                }
                                className="setup-row-name"
                                data-setup-button={setup.id}
                                onClick={() => recallSetup(setup)}
                                role="menuitem"
                                type="button"
                              >
                                {setup.id === selectedSetupId && (
                                  <span
                                    aria-hidden="true"
                                    className="setup-row-check"
                                  >
                                    ✓{" "}
                                  </span>
                                )}
                                {setup.name}
                              </button>
                            )}
                            <span className="setup-row-actions">
                              <button
                                aria-label={`Rename ${setup.name}`}
                                disabled={savingSetup}
                                onClick={() => openRenameRow(setup)}
                                role="menuitem"
                                type="button"
                              >
                                Rename
                              </button>
                              <button
                                aria-label={`Duplicate ${setup.name}`}
                                disabled={savingSetup}
                                onClick={() => void duplicateSetup(setup)}
                                role="menuitem"
                                type="button"
                              >
                                Duplicate
                              </button>
                              <button
                                aria-label={
                                  confirming
                                    ? `Confirm deleting ${setup.name}`
                                    : `Delete ${setup.name}`
                                }
                                className={confirming ? "confirm-delete" : ""}
                                onClick={() => void deleteSetup(setup)}
                                role="menuitem"
                                type="button"
                              >
                                {confirming ? "Confirm" : "Delete"}
                              </button>
                            </span>
                          </li>
                        );
                      })}
                    </ul>
                  )}
                  <div className="setup-menu-tools">
                    {setupNameMode?.kind === "save" ? (
                      <div className="setup-menu-name-row">
                        <input
                          aria-label="Setup name"
                          maxLength={48}
                          onChange={(event) =>
                            setSetupNameDraft(event.target.value)
                          }
                          onKeyDown={(event) => {
                            if (event.key === "Enter") {
                              event.preventDefault();
                              void submitSetupName();
                            }
                          }}
                          placeholder={defaultSetupName}
                          ref={setupNameInputRef}
                          type="text"
                          value={setupNameDraft}
                        />
                        <button
                          disabled={savingSetup}
                          onClick={() => void submitSetupName()}
                          type="button"
                        >
                          {savingSetup ? "Saving…" : "Save"}
                        </button>
                      </div>
                    ) : (
                      <button
                        disabled={savingSetup}
                        onClick={openSaveRow}
                        role="menuitem"
                        type="button"
                      >
                        Save current setup
                      </button>
                    )}
                    <button
                      disabled={setups.length === 0}
                      onClick={exportSetups}
                      role="menuitem"
                      type="button"
                    >
                      Export
                    </button>
                    <button
                      onClick={() => {
                        setupImportRef.current?.click();
                        closeSetupMenu(true);
                      }}
                      role="menuitem"
                      type="button"
                    >
                      Import
                    </button>
                  </div>
                </div>
              )}
            </div>
            <small aria-live="polite" className="setup-status" role="status">
              {setupStatus}
            </small>
            <input
              accept="application/json"
              aria-label="Import setups file"
              hidden
              onChange={(event) => {
                const file = event.target.files?.[0];
                event.target.value = "";
                if (file) void importSetups(file);
              }}
              ref={setupImportRef}
              type="file"
            />
          </div>
          <SettingsMenu />
          {availableUpdate && !updateDismissed && (
            <aside
              className={`update-notice ${availableUpdate.urgency}`}
              role="status"
            >
              <span>
                <strong>
                  {displayVersion(availableUpdate.version)} available
                </strong>
                <small>{updateStatus}</small>
              </span>
              {signedUpdateReady ? (
                <>
                  <button
                    type="button"
                    disabled={updateBusy}
                    onClick={() => void installUpdate()}
                  >
                    {updateBusy ? "Working…" : "Install"}
                  </button>
                  <ExternalLink
                    href={availableUpdate.url || RELEASES_URL}
                  >
                    Notes
                  </ExternalLink>
                </>
              ) : (
                <a
                  href={availableUpdate.url || RELEASES_URL}
                  target="_blank"
                  rel="noreferrer"
                >
                  Download
                </a>
              )}
              {!updateBusy && (
                <button
                  type="button"
                  aria-label={`Dismiss ${displayVersion(
                    availableUpdate.version,
                  )} update notice`}
                  onClick={() => setUpdateDismissed(true)}
                >
                  Later
                </button>
              )}
            </aside>
          )}
          {job !== null && (
            <div className={`build-state ${job.status}`}>
              <span />
              {statusLabel}
            </div>
          )}
          <button
            className={`topbar-generate${cancellationActive ? " cancel" : ""}`}
            type={generationActive ? "button" : "submit"}
            form="terrain-controls"
            disabled={submitting || canceling}
            onClick={
              generationActive ? () => void cancelGeneration() : undefined
            }
          >
            {submitting
              ? "Starting…"
              : canceling
                ? "Canceling…"
                : generationActive
                  ? "Cancel"
                  : "Generate"}
            <span aria-hidden="true">{cancellationActive ? "×" : "↗"}</span>
          </button>
        </div>
      </header>

      <div
        className="workspace"
        ref={workspaceRef}
        style={
          {
            "--visual-share": `${visualHeightPercent}fr`,
            "--controls-share": `${100 - visualHeightPercent}fr`,
          } as CSSProperties
        }
      >
        <section
          className="visual-column"
          aria-label="Place and model preview"
          ref={visualColumnRef}
          style={
            {
              "--map-share": `${mapSharePercent}fr`,
              "--preview-share": `${100 - mapSharePercent}fr`,
            } as CSSProperties
          }
        >
          <TerrainMap
            spec={spec}
            markerPlacementKind={markerPlacementKind}
            onAddMarker={addMarker}
            onCenterChange={onCenterChange}
            onGroundSpanChange={(groundSpanKm) =>
              update("ground_span_km", groundSpanKm)
            }
          />
          <div
            aria-label="Resize map and preview panes"
            aria-orientation="vertical"
            aria-valuemax={MAX_MAP_SHARE_PERCENT}
            aria-valuemin={MIN_MAP_SHARE_PERCENT}
            aria-valuenow={Math.round(mapSharePercent)}
            aria-valuetext={`${Math.round(mapSharePercent)}% map width`}
            className="visual-resizer"
            onDoubleClick={() => setMapSharePercent(DEFAULT_MAP_SHARE_PERCENT)}
            onKeyDown={visualResizeKeyDown}
            onLostPointerCapture={() => {
              visualResizePointerRef.current = null;
            }}
            onPointerCancel={() => {
              visualResizePointerRef.current = null;
            }}
            onPointerDown={visualResizePointerDown}
            onPointerMove={visualResizePointerMove}
            onPointerUp={visualResizePointerUp}
            role="separator"
            tabIndex={0}
            title="Drag to resize the map and 3D preview panes"
          />
          <Suspense
            fallback={
              <section className="relief-shell" aria-label="3D terrain preview">
                <div className="preview-label loading">
                  <span>Loading 3D preview…</span>
                </div>
              </section>
            }
          >
            <ReliefPreview
              spec={spec}
              preview={preview}
              previewState={previewState}
            />
          </Suspense>
        </section>

        <div
          aria-label="Resize map and 3D preview"
          aria-orientation="horizontal"
          aria-valuemax={MAX_VISUAL_HEIGHT_PERCENT}
          aria-valuemin={MIN_VISUAL_HEIGHT_PERCENT}
          aria-valuenow={Math.round(visualHeightPercent)}
          aria-valuetext={`${Math.round(visualHeightPercent)}% preview height`}
          className="workspace-resizer"
          onDoubleClick={() =>
            setVisualHeightPercent(DEFAULT_VISUAL_HEIGHT_PERCENT)
          }
          onKeyDown={resizeKeyDown}
          onLostPointerCapture={() => {
            resizePointerRef.current = null;
          }}
          onPointerCancel={() => {
            resizePointerRef.current = null;
          }}
          onPointerDown={resizePointerDown}
          onPointerMove={resizePointerMove}
          onPointerUp={resizePointerUp}
          role="separator"
          tabIndex={0}
          title="Drag to resize the map and 3D preview"
        />

        <form className="controls" id="terrain-controls" onSubmit={submit}>
          <div className="panel-heading">
            <div>
              <h1>Shape your terrain</h1>
              <p>Choose a section. Generate stays within reach.</p>
            </div>
          </div>

          <div
            className="control-tabs"
            role="tablist"
            aria-label="Terrain settings"
          >
            {(
              [
                ["model", "Model"],
                ["surface", "Surface"],
                ["buildings", "Buildings"],
                ["markers", "Markers"],
                ["colors", "Colors"],
                ["mounting", "Mounting"],
                ["output", "Output"],
              ] as const
            ).map(([key, label]) => (
              <button
                key={key}
                type="button"
                role="tab"
                aria-selected={activeSection === key}
                className={activeSection === key ? "active" : ""}
                onClick={() => setActiveSection(key)}
              >
                {label}
                {key === "output" && job && (
                  <span className={`tab-status ${job.status}`} />
                )}
              </button>
            ))}
          </div>

          {job?.status === "failed" && activeSection !== "output" && (
            <section className="generation-error-banner" role="alert">
              <span className="status-dot" aria-hidden="true" />
              <strong>{job.error ?? "Generation failed."}</strong>
              <button type="button" onClick={() => setActiveSection("output")}>
                View output
              </button>
            </section>
          )}

          <ModelPanel
            adjacentMessage={adjacentMessage}
            choosePlace={choosePlace}
            heightFrameCompatible={heightFrameCompatible}
            heightFrameLocked={heightFrameLocked}
            hidden={activeSection !== "model"}
            lockHeightFrame={lockHeightFrame}
            moveToAdjacentTile={moveToAdjacentTile}
            placeMessage={placeMessage}
            placeQuery={placeQuery}
            placeResults={placeResults}
            searchPlaces={searchPlaces}
            searchingPlaces={searchingPlaces}
            setMeshQuality={setMeshQuality}
            setPlaceQuery={setPlaceQuery}
            setSuperTileAnchor={setSuperTileAnchor}
            spec={spec}
            superTileGridSizes={superTileGridSizes}
            unlockHeightFrame={unlockHeightFrame}
            update={update}
          />

          <SurfacePanel
            hidden={activeSection !== "surface"}
            importTrailFiles={importTrailFiles}
            removeTrail={removeTrail}
            spec={spec}
            trailNotice={trailNotice}
            updateColor={updateColor}
          />

          <BuildingsPanel
            hidden={activeSection !== "buildings"}
            spec={spec}
            updateBuildings={updateBuildings}
          />

          <MarkersPanel
            hidden={activeSection !== "markers"}
            placementKind={markerPlacementKind}
            removeMarker={removeMarker}
            setPlacementKind={setMarkerPlacementKind}
            spec={spec}
            updateMarker={updateMarker}
            updateMarkerSettings={updateMarkerSettings}
          />

          <ColorsPanel
            hidden={activeSection !== "colors"}
            spec={spec}
            updateColor={updateColor}
            updateMarkerSettings={updateMarkerSettings}
            updateTray={updateTray}
          />

          <ModelTypePanel
            hidden={activeSection !== "model"}
            setPieceLayout={setPieceLayout}
            spec={spec}
            update={update}
          />

          <MountingPanel
            hidden={activeSection !== "mounting"}
            spec={spec}
            update={update}
            updateTray={updateTray}
            updatePuzzleRetention={updatePuzzleRetention}
            updateWallMount={updateWallMount}
          />

          <OutputPanel
            artifactFeedback={artifactFeedback}
            generationStages={generationStages}
            hidden={activeSection !== "output"}
            job={job}
            message={message}
            noteWebDownload={noteWebDownload}
            saveDesktopArtifact={saveDesktopArtifact}
            spec={spec}
            statusLabel={statusLabel}
            updateColor={updateColor}
          />
        </form>
      </div>
    </main>
  );
}
