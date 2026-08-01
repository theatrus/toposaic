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
import { IS_TAURI, terrainApi, type PreviewProgress } from "./api";
import { ExternalLink } from "./external-link";
import {
  DEFAULT_DOT_MARKER_STYLE,
  DEFAULT_FLAG_MARKER_STYLE,
  DEFAULT_MAP_LABEL_STYLE,
  MAX_ASSEMBLED_SAMPLES,
  MAX_SUPER_TILE_SIDE,
  deriveHeightFrame,
  exaggerationForHeight,
  heightForExaggeration,
  initialSpec,
  isMapLabel,
  markerNeedsSurfaceData,
  limitPlaceName,
  mergeSpecDefaults,
  normalizeMappedWidthCap,
  randomPuzzleSeed,
} from "./config";
import type {
  Artifact,
  ArtifactFeedback,
  GenerationControlTab,
  GenerationSpec,
  Job,
  MarkerKind,
  PlaceResult,
  PreviewData,
  SavedSetup,
  SetupVersion,
  SourceBundleSummary,
  TrailRoute,
} from "./contracts";
import { describeJobFailure } from "./generation-failure";
import { specHasDrifted } from "./setup-drift";
import {
  MAX_TRAILS,
  MAX_TRAIL_FILE_BYTES,
  MAX_TRAIL_POINTS,
  parseTrailFile,
} from "./trails";
import {
  type AdjacentDirection,
  mapFrameForSpec,
  matchingTileCenter,
} from "./geo";
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
const CONTROL_TAB_LABELS: Record<GenerationControlTab, string> = {
  model: "Model",
  surface: "Surface",
  buildings: "Buildings",
  markers: "Markers",
  colors: "Colors",
  mounting: "Mounting",
  output: "Output",
};
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
  const [activeSection, setActiveSection] =
    useState<GenerationControlTab>("model");
  const [markerPlacementKind, setMarkerPlacementKind] =
    useState<MarkerKind | null>(null);
  const [movingMarkerIndex, setMovingMarkerIndex] = useState<number | null>(
    null,
  );
  const showSection = useCallback((section: GenerationControlTab) => {
    setActiveSection(section);
    if (section === "markers") return;
    setMarkerPlacementKind(null);
    setMovingMarkerIndex(null);
  }, []);
  const [job, setJob] = useState<Job | null>(null);
  const [generatedPreview, setGeneratedPreview] = useState<PreviewData | null>(
    null,
  );
  const [elevationPreview, setElevationPreview] = useState<PreviewData | null>(
    null,
  );
  const [previewedSpecKey, setPreviewedSpecKey] = useState<string | null>(null);
  const [previewActivity, setPreviewActivity] = useState<
    (PreviewProgress & { specKey: string }) | null
  >(null);
  const previewRequestIdRef = useRef(0);
  const previewAbortRef = useRef<AbortController | null>(null);
  const [previewCanceledSpecKey, setPreviewCanceledSpecKey] = useState<
    string | null
  >(null);
  const [submitting, setSubmitting] = useState(false);
  const [canceling, setCanceling] = useState(false);
  const [message, setMessage] = useState<string | null>(null);
  const [artifactFeedback, setArtifactFeedback] =
    useState<ArtifactFeedback | null>(null);
  // Kept with the job it describes rather than cleared when the job changes:
  // a stale summary is then simply one that does not match, and the effect
  // below never has to setState synchronously to wipe it.
  const [sourceBundle, setSourceBundle] = useState<{
    jobId: string;
    summary: SourceBundleSummary;
  } | null>(null);
  const [buildingSourceBundle, setBuildingSourceBundle] = useState(false);
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
  // Which row's history is open, and what it holds. One row at a time: the
  // menu is already a dense list.
  const [historyFor, setHistoryFor] = useState<string | null>(null);
  const [historyVersions, setHistoryVersions] = useState<SetupVersion[]>([]);
  const [historyBusy, setHistoryBusy] = useState(false);
  // Bumped on every recall, so the map can drop linked zoom each time —
  // including a second recall of the setup already showing.
  const [setupRecallCount, setSetupRecallCount] = useState(0);
  // Which version is one click from being rolled back to. Rolling back
  // loads the old spec over the model on screen, so it asks first — the
  // same in-row confirm the delete action uses.
  const [confirmingRollbackId, setConfirmingRollbackId] = useState<
    string | null
  >(null);
  // The row whose versions the panel is currently waiting for, so a slower
  // answer for an earlier row can be dropped instead of overwriting it.
  const historyRequestRef = useRef<string | null>(null);
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
  const sourceImportRef = useRef<HTMLInputElement>(null);
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

  // Moving to different ground drops a pinned ground palette along with the
  // pinned map frame, and for the same reason: both describe one footprint.
  // A palette arrives pinned by importing a source bundle, and keeping it
  // would match the new area's imagery against colors found somewhere else —
  // print Fuji in Rainier's greys. Stepping through a super-tile is not this
  // case and keeps its palette; that is the whole point of the lock.
  const movedToNewGround = (
    current: GenerationSpec,
  ): Partial<GenerationSpec> =>
    current.color_output.locked_ground_palette
      ? {
          map_frame: null,
          color_output: {
            ...current.color_output,
            locked_ground_palette: undefined,
          },
        }
      : { map_frame: null };

  const update = useCallback(
    <Key extends keyof GenerationSpec>(
      key: Key,
      value: GenerationSpec[Key],
    ) => {
      setGeneratedPreview(null);
      setSpec((current) => {
        if (
          key === "center_lat" ||
          key === "center_lon" ||
          key === "ground_span_km" ||
          key === "terrain_rotation_degrees" ||
          key === "puzzle_tile_column" ||
          key === "puzzle_tile_row"
        ) {
          return { ...current, [key]: value, ...movedToNewGround(current) };
        }
        if (key === "model_outline") {
          const outline = value as GenerationSpec["model_outline"];
          return {
            ...current,
            model_outline: outline,
            tray:
              outline.shape === "rectangle"
                ? current.tray
                : { ...current.tray, label_enabled: false },
          };
        }
        if (
          (key === "adjacent_columns" || key === "adjacent_rows") &&
          Number(value) > 1 &&
          current.model_outline.shape !== "rectangle"
        ) {
          return {
            ...current,
            [key]: value,
            model_outline: { ...current.model_outline, shape: "rectangle" },
          };
        }
        if (key !== "base_mm") return { ...current, [key]: value };
        const baseMm = value as number;
        return {
          ...current,
          [key]: value,
          markers: current.markers.map((marker) =>
            marker.kind === "flag_hole" || marker.kind === "flag_label"
              ? {
                  ...marker,
                  flag_style: {
                    ...(marker.flag_style ?? DEFAULT_FLAG_MARKER_STYLE),
                    hole_depth_mm: Math.min(
                      (marker.flag_style ?? DEFAULT_FLAG_MARKER_STYLE)
                        .hole_depth_mm,
                      Math.max(0.6, baseMm - 0.4),
                    ),
                  },
                }
              : marker,
          ),
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
  // The sampled area, for translating between the two ways of naming a
  // vertical scale and for showing what the current one comes to.
  const heightSample = useMemo(() => {
    const sampled = generatedPreview ?? elevationPreview;
    if (
      sampled?.minimum_elevation_m === undefined ||
      sampled.maximum_elevation_m === undefined
    ) {
      return null;
    }
    return {
      minimum_elevation_m: sampled.minimum_elevation_m,
      maximum_elevation_m: sampled.maximum_elevation_m,
    };
  }, [generatedPreview, elevationPreview]);

  const heightScaleReadout = useMemo(() => {
    if (!heightSample) return null;
    return {
      exaggeration: exaggerationForHeight(spec, heightSample),
      height: heightForExaggeration(
        spec,
        heightSample,
        spec.vertical_exaggeration,
      ),
    };
  }, [spec, heightSample]);

  // Switching modes must not move the model: the incoming field is filled
  // with whatever the outgoing one already amounts to. Without a sampled
  // area there is nothing to translate from, so the mode changes alone.
  const setHeightMode = useCallback(
    (mode: GenerationSpec["height_mode"]) => {
      setGeneratedPreview(null);
      setSpec((current) => {
        if (current.height_mode === mode) return current;
        if (!heightSample) return { ...current, height_mode: mode };
        if (mode === "multiplier") {
          return {
            ...current,
            height_mode: mode,
            // Rounded for a readable field, not clamped to a slider:
            // narrowing here would silently flatten every low-relief area
            // the moment the mode changed.
            vertical_exaggeration: Math.min(
              1_000_000,
              Math.max(
                0.0001,
                Number(
                  exaggerationForHeight(current, heightSample).toPrecision(4),
                ),
              ),
            ),
          };
        }
        return {
          ...current,
          height_mode: mode,
          relief_mm: Math.min(
            80,
            Math.max(
              3,
              Number(
                heightForExaggeration(
                  current,
                  heightSample,
                  current.vertical_exaggeration,
                ).toFixed(1),
              ),
            ),
          ),
        };
      });
    },
    [heightSample],
  );

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
  const updateMarine = useCallback(
    <Key extends keyof GenerationSpec["marine"]>(
      key: Key,
      value: GenerationSpec["marine"][Key],
    ) => {
      setGeneratedPreview(null);
      setSpec((current) => ({
        ...current,
        marine: { ...current.marine, [key]: value },
      }));
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
      setSpec((current) => ({
        ...current,
        marker_settings: { ...current.marker_settings, [key]: value },
      }));
    },
    [],
  );
  const placeMarker = useCallback(
    (longitude: number, latitude: number) => {
      if (movingMarkerIndex !== null) {
        setGeneratedPreview(null);
        setSpec((current) => ({
          ...current,
          markers: current.markers.map((marker, index) =>
            index === movingMarkerIndex
              ? { ...marker, longitude, latitude }
              : marker,
          ),
        }));
        setMovingMarkerIndex(null);
        return;
      }
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
          markers: [
            ...current.markers,
            {
              kind: markerPlacementKind,
              latitude,
              longitude,
              name: `${label} ${number}`,
              label_height_mm: 4,
              rotation_degrees: 0,
              ...(markerPlacementKind === "dot"
                ? { dot_style: { ...DEFAULT_DOT_MARKER_STYLE } }
                : {}),
              ...(markerPlacementKind === "flag_hole" ||
              markerPlacementKind === "flag_label"
                ? {
                    flag_style: {
                      ...DEFAULT_FLAG_MARKER_STYLE,
                      hole_depth_mm: Math.min(
                        DEFAULT_FLAG_MARKER_STYLE.hole_depth_mm,
                        Math.max(0.6, current.base_mm - 0.4),
                      ),
                    },
                  }
                : {}),
              ...(isMapLabel(markerPlacementKind)
                ? {
                    label_style: { ...DEFAULT_MAP_LABEL_STYLE },
                  }
                : {}),
            },
          ].slice(0, 50),
        };
      });
      setMarkerPlacementKind(null);
    },
    [markerPlacementKind, movingMarkerIndex],
  );
  const chooseMarkerPlacementKind = useCallback((kind: MarkerKind | null) => {
    setMovingMarkerIndex(null);
    setMarkerPlacementKind(kind);
  }, []);
  const moveMarker = useCallback((index: number) => {
    setMarkerPlacementKind(null);
    setMovingMarkerIndex((current) => (current === index ? null : index));
  }, []);
  const updateMarker = useCallback(
    (index: number, patch: Partial<GenerationSpec["markers"][number]>) => {
      setGeneratedPreview(null);
      setSpec((current) => ({
        ...current,
        buildings:
          patch.kind === "building"
            ? { ...current.buildings, enabled: true }
            : current.buildings,
        markers: current.markers.map((marker, position) => {
          if (position !== index) return marker;
          const next = { ...marker, ...patch };
          if (next.kind === "dot" && !next.dot_style) {
            next.dot_style = { ...DEFAULT_DOT_MARKER_STYLE };
          }
          if (
            (next.kind === "flag_hole" || next.kind === "flag_label") &&
            !next.flag_style
          ) {
            next.flag_style = {
              ...DEFAULT_FLAG_MARKER_STYLE,
              hole_depth_mm: Math.min(
                DEFAULT_FLAG_MARKER_STYLE.hole_depth_mm,
                Math.max(0.6, current.base_mm - 0.4),
              ),
            };
          }
          if (isMapLabel(next.kind) && !next.label_style) {
            next.label_style = { ...DEFAULT_MAP_LABEL_STYLE };
          }
          return next;
        }),
      }));
    },
    [],
  );
  const removeMarker = useCallback((index: number) => {
    setGeneratedPreview(null);
    setMovingMarkerIndex((current) => {
      if (current === null || current < index) return current;
      return current === index ? null : current - 1;
    });
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
      ...movedToNewGround(current),
    }));
  }, []);

  const lockHeightFrame = useCallback(() => {
    const sampled = generatedPreview ?? elevationPreview;
    if (
      sampled?.minimum_elevation_m === undefined ||
      sampled.maximum_elevation_m === undefined
    ) {
      setAdjacentMessage(
        "Wait for the elevation sample, then lock the height frame.",
      );
      return false;
    }
    const { datum, metresPerMm } = deriveHeightFrame(spec, {
      minimum_elevation_m: sampled.minimum_elevation_m,
      maximum_elevation_m: sampled.maximum_elevation_m,
    });
    setSpec((current) => ({
      ...current,
      elevation_datum_m: datum,
      elevation_m_per_mm: Number(metresPerMm.toFixed(4)),
    }));
    setAdjacentMessage(
      `Height frame locked at ${datum.toFixed(1)} m with ${metresPerMm.toFixed(1)} m/mm.`,
    );
    return true;
  }, [elevationPreview, generatedPreview, spec]);

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
        const derived = deriveHeightFrame(spec, {
          minimum_elevation_m: sampled.minimum_elevation_m,
          maximum_elevation_m: sampled.maximum_elevation_m,
        });
        datum = derived.datum;
        metresPerMm = Number(derived.metresPerMm.toFixed(4));
      }
      const frame = mapFrameForSpec(spec);
      const tileColumn =
        spec.puzzle_tile_column +
        (direction === "east" ? 1 : direction === "west" ? -1 : 0);
      const tileRow =
        spec.puzzle_tile_row +
        (direction === "south" ? 1 : direction === "north" ? -1 : 0);
      const next = matchingTileCenter(spec, tileColumn, tileRow);
      setGeneratedPreview(null);
      setElevationPreview(null);
      setSpec((current) => ({
        ...current,
        center_lat: next.latitude,
        center_lon: next.longitude,
        map_frame: frame,
        elevation_datum_m: datum,
        elevation_m_per_mm: metresPerMm,
        puzzle_tile_column: tileColumn,
        puzzle_tile_row: tileRow,
      }));
      setAdjacentMessage(
        `Moved ${direction} by one tile. The shared height frame stays locked.`,
      );
    },
    [elevationPreview, generatedPreview, spec],
  );

  useEffect(() => {
    const controller = new AbortController();
    previewAbortRef.current = controller;
    const previewSpecKey = JSON.stringify(spec);
    const requestId = ++previewRequestIdRef.current;
    const timer = window.setTimeout(async () => {
      if (
        controller.signal.aborted ||
        previewRequestIdRef.current !== requestId
      ) {
        return;
      }
      setPreviewCanceledSpecKey(null);
      setPreviewActivity({
        specKey: previewSpecKey,
        stage: "elevation",
        label: "Starting preview",
        progress: 2,
      });
      try {
        const nextPreview = await terrainApi.preview(
          spec,
          controller.signal,
          (progress) => {
            if (
              controller.signal.aborted ||
              previewRequestIdRef.current !== requestId
            ) {
              return;
            }
            setPreviewActivity({ ...progress, specKey: previewSpecKey });
          },
        );
        if (
          controller.signal.aborted ||
          previewRequestIdRef.current !== requestId
        ) {
          return;
        }
        setPreviewedSpecKey(previewSpecKey);
        setElevationPreview(nextPreview);
      } catch (error) {
        // Keep the last good mesh in view when a background refresh fails.
        // Generate still reports its own error with the full job context.
        if (error instanceof DOMException && error.name === "AbortError") return;
      } finally {
        if (
          !controller.signal.aborted &&
          previewRequestIdRef.current === requestId
        ) {
          setPreviewActivity(null);
        }
        if (previewAbortRef.current === controller) {
          previewAbortRef.current = null;
        }
      }
    }, 350);
    return () => {
      window.clearTimeout(timer);
      controller.abort();
      if (previewAbortRef.current === controller) {
        previewAbortRef.current = null;
      }
    };
  }, [spec]);

  const cancelPreview = useCallback(() => {
    previewRequestIdRef.current += 1;
    previewAbortRef.current?.abort();
    previewAbortRef.current = null;
    setPreviewCanceledSpecKey(JSON.stringify(spec));
    setPreviewActivity(null);
  }, [spec]);

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
    setConfirmingRollbackId(null);
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

  // Whether the model has moved since the recalled setup was stored. Only
  // the recalled one can drift: another setup is not what this model came
  // from, so "different" says nothing about it.
  //
  // Compared against the setup as RECALLED, not as stored. Recall fills a
  // setup through mergeSpecDefaults, so one saved before a field existed
  // gains that field on the way in; comparing against the stored spec would
  // report drift the moment it was loaded, with nothing touched.
  const selectedSetupDrifted = useMemo(
    () =>
      specHasDrifted(
        spec,
        selectedSetup ? mergeSpecDefaults(selectedSetup.spec) : undefined,
      ),
    [spec, selectedSetup],
  );

  const openHistory = async (setup: SavedSetup) => {
    setConfirmingRollbackId(null);
    if (historyFor === setup.id) {
      setHistoryFor(null);
      return;
    }
    // Drop the row's versions before showing another row's panel. Held on
    // to, they render under the setup now open — a list of times that
    // belong to a different setup, each offering to roll THIS one back.
    setHistoryVersions([]);
    setHistoryBusy(true);
    setHistoryFor(setup.id);
    historyRequestRef.current = setup.id;
    try {
      const versions = await terrainApi.listSetupVersions(setup.id);
      // A slower request for an earlier row must not land on top of this
      // one; only the newest asked-for row may fill the panel.
      if (historyRequestRef.current !== setup.id) return;
      setHistoryVersions(versions);
    } catch (error) {
      if (historyRequestRef.current !== setup.id) return;
      setHistoryFor(null);
      setSetupStatus(
        error instanceof Error ? error.message : "No earlier versions loaded.",
      );
    } finally {
      if (historyRequestRef.current === setup.id) setHistoryBusy(false);
    }
  };

  // Puts an earlier spec back into the setup AND loads it, which is what
  // rolling back means. It therefore replaces whatever is on screen, so the
  // first click only arms the button — the same in-row confirm the delete
  // action uses — and the label says what is about to be lost.
  const restoreSetupVersion = async (
    setup: SavedSetup,
    version: SetupVersion,
  ) => {
    if (confirmingRollbackId !== version.id) {
      setConfirmingSetupDeleteId(null);
      setConfirmingRollbackId(version.id);
      return;
    }
    setConfirmingRollbackId(null);
    setHistoryBusy(true);
    try {
      const restored = await terrainApi.restoreSetupVersion(
        setup.id,
        version.id,
      );
      await refreshSetups();
      setHistoryVersions(await terrainApi.listSetupVersions(setup.id));
      recallSetup(restored);
      setSetupStatus(`Rolled “${restored.name}” back to an earlier version.`);
    } catch (error) {
      setSetupStatus(
        error instanceof Error ? error.message : "The version was not restored.",
      );
    } finally {
      setHistoryBusy(false);
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
    setSetupRecallCount((count) => count + 1);
    // Merge over the client defaults so setups saved before a field existed
    // still get a value, then drop stale generated output like a place change.
    setSpec(mergeSpecDefaults(setup.spec));
    setMarkerPlacementKind(null);
    setMovingMarkerIndex(null);
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
        const nextJob = await terrainApi.getJob(polledJobId, controller.signal);
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
      showSection("output");
    } catch (error) {
      showSection("output");
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
    showSection("output");
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

  // One folder picker instead of a save dialog per file. The caller says
  // which files: the print set, or that plus the per-piece STLs.
  const saveDesktopArtifactSet = async (
    key: string,
    artifactNames: string[],
  ) => {
    if (!job || !IS_TAURI || artifactNames.length === 0) return;
    setArtifactFeedback({ name: key, state: "saving" });
    try {
      const { invoke } = await import("@tauri-apps/api/core");
      const saved = await invoke<{ directory: string; files: number } | null>(
        "save_all_artifacts",
        {
          jobId: job.id,
          folderName: job.spec.place_name.trim() || "toposaic",
          artifactNames,
        },
      );
      if (saved === null) {
        setArtifactFeedback(null);
        setMessage("Save canceled.");
        return;
      }
      setArtifactFeedback({ name: key, state: "saved" });
      setMessage(
        `Saved ${saved.files} ${saved.files === 1 ? "file" : "files"} to ${saved.directory}.`,
      );
    } catch (error) {
      setArtifactFeedback(null);
      setMessage(
        error instanceof Error
          ? error.message
          : typeof error === "string"
            ? error
            : "Could not save the print files.",
      );
    }
  };

  // Asked for once a job finishes, so the Output tab can say how large the
  // source data is before anyone commits to packing it.
  useEffect(() => {
    if (job?.status !== "complete") return;
    const jobId = job.id;
    const controller = new AbortController();
    terrainApi
      .sourceBundle(jobId, controller.signal)
      .then((summary) => setSourceBundle({ jobId, summary }))
      // A job from before this feature, or a service that does not know the
      // route, simply has no bundle to offer. Nothing to report.
      .catch(() => {});
    return () => controller.abort();
  }, [job?.id, job?.status]);

  const jobSourceBundle =
    job && sourceBundle?.jobId === job.id ? sourceBundle.summary : null;

  const buildSourceBundle = async () => {
    if (!job) return;
    setBuildingSourceBundle(true);
    try {
      const built = await terrainApi.buildSourceBundle(job.id);
      setSourceBundle((current) =>
        current
          ? {
              ...current,
              summary: { ...current.summary, built_bytes: built.bytes },
            }
          : current,
      );
      setMessage(
        `Packed ${(built.bytes / 1024 / 1024).toFixed(1)} MB of source data.`,
      );
    } catch (error) {
      setMessage(
        error instanceof Error
          ? error.message
          : "Could not pack the source data.",
      );
    } finally {
      setBuildingSourceBundle(false);
    }
  };

  // Reports through the setups status line, not the Output tab's message:
  // this is started from the setups menu, and a reply the user has to change
  // tabs to read is no reply at all.
  const importSourceBundle = async (file: File) => {
    setSetupStatus(`Importing ${file.name}…`);
    try {
      const { report, spec: bundled } =
        await terrainApi.importSourceBundle(file);
      setSpec(mergeSpecDefaults(bundled));
      const kept = report.already_present
        ? `, ${report.already_present} already cached`
        : "";
      const refused = report.rejected ? `, ${report.rejected} refused` : "";
      setSetupStatus(
        `Loaded ${report.place_name}: ${report.added} source files added${kept}${refused}. Generating now needs no network.`,
      );
    } catch (error) {
      setSetupStatus(
        error instanceof Error
          ? error.message
          : "Could not import that source bundle.",
      );
    }
  };

  const noteWebDownload = (artifact: Artifact) => {
    setArtifactFeedback({ name: artifact.name, state: "sent" });
    setMessage(`Sent ${artifact.name} to your browser downloads.`);
  };

  // The ground palette to show in the Colors tab. A setup that already
  // carries a locked palette knows its colors before anything runs;
  // otherwise the last generated preview reports what the job discovered.
  const discoveredGround = useMemo(
    () =>
      spec.color_output.locked_ground_palette ??
      generatedPreview?.ground_palette ??
      [],
    [spec.color_output.locked_ground_palette, generatedPreview?.ground_palette],
  );

  const generationFailure = useMemo(() => describeJobFailure(job), [job]);
  const statusLabel = useMemo(() => {
    if (!job) return null;
    if (job.status === "complete") return "Your print files are ready.";
    if (job.status === "failed") {
      return generationFailure?.title ?? "Generation failed.";
    }
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
  }, [generationFailure, job]);

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
      {
        key: "geometry",
        label: "Geometry",
        start: hasSurface ? 65 : 40,
        end: 99,
      },
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
        state: done
          ? "done"
          : stopped
            ? "stopped"
            : active
              ? "active"
              : "pending",
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

  const preview = useMemo(() => {
    if (!generatedPreview) return elevationPreview;
    return {
      ...elevationPreview,
      ...generatedPreview,
      // The finished preview has denser sampled colors and heights, while
      // the background pass owns the draft export geometry. Keep both.
      model_meshes: elevationPreview?.model_meshes,
      model_bounds_mm: elevationPreview?.model_bounds_mm,
      model_preview_detail: elevationPreview?.model_preview_detail,
      model_preview_error: elevationPreview?.model_preview_error,
    };
  }, [elevationPreview, generatedPreview]);
  const currentSpecKey = JSON.stringify(spec);
  const previewPaused = previewCanceledSpecKey === currentSpecKey;
  const previewStale =
    elevationPreview !== null &&
    previewedSpecKey !== currentSpecKey &&
    !previewPaused;
  const currentPreviewActivity =
    previewActivity?.specKey === currentSpecKey ? previewActivity : null;
  const heightFrameLocked =
    spec.elevation_datum_m !== null && spec.elevation_m_per_mm !== null;
  const heightFrameCompatible = preview?.height_frame_compatible !== false;
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
  // One decision, not two: whether this row is showing a release summary
  // has to follow from the same branch that chose the text, or the clipping
  // and the tooltip end up describing a line that is not there.
  const updateLine: { text: string; isSummary: boolean } = (() => {
    const plain = (text: string) => ({ text, isSummary: false });
    switch (updateInstallState.phase) {
      case "checking":
        return plain("Checking signed package…");
      case "downloading":
        return plain(
          updateInstallState.percent === null
            ? "Downloading update…"
            : `Downloading ${updateInstallState.percent}%…`,
        );
      case "installing":
        return plain("Installing and restarting…");
      case "error":
        return plain(updateInstallState.message);
      default:
        break;
    }
    if (availableUpdate?.urgency === "required") {
      return plain("This version is no longer supported.");
    }
    // What the release says about itself, when the notice carried a line.
    // Otherwise the running version, which is what this said before there
    // was anything better to show.
    return availableUpdate?.summary
      ? { text: availableUpdate.summary, isSummary: true }
      : plain(`Current ${displayVersion(appVersion)}`);
  })();
  // A summary is clipped to the one top-bar row it has, so the whole of it
  // goes in the tooltip — with the running version, which it displaced.
  const updateStatusTitle = updateLine.isSummary
    ? `${updateLine.text} — current ${displayVersion(appVersion)}`
    : undefined;
  const previewState = generatedPreview
    ? "generated"
    : previewPaused && elevationPreview
      ? "paused"
      : elevationPreview && (currentPreviewActivity || previewStale)
        ? "updating"
        : elevationPreview
          ? "live"
          : currentPreviewActivity
            ? "loading"
            : "shape";
  const previewProgress =
    previewState === "updating" || previewState === "loading"
      ? currentPreviewActivity ?? {
          stage: "elevation" as const,
          label: "Waiting for changes to settle",
          progress: 1,
        }
      : null;

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
                        const confirming = confirmingSetupDeleteId === setup.id;
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
                              {setup.id === selectedSetupId && (
                                <button
                                  aria-label={
                                    selectedSetupDrifted
                                      ? `Save changes to ${setup.name}`
                                      : `${setup.name} has no unsaved changes`
                                  }
                                  className={
                                    selectedSetupDrifted ? "setup-row-save" : ""
                                  }
                                  disabled={savingSetup || !selectedSetupDrifted}
                                  onClick={() => void saveSetupAs(setup.name)}
                                  role="menuitem"
                                  type="button"
                                >
                                  {selectedSetupDrifted ? "Save" : "Saved"}
                                </button>
                              )}
                              <button
                                aria-label={
                                  historyFor === setup.id
                                    ? `Hide earlier versions of ${setup.name}`
                                    : `Earlier versions of ${setup.name}`
                                }
                                onClick={() => void openHistory(setup)}
                                role="menuitem"
                                type="button"
                              >
                                History
                              </button>
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
                            {historyFor === setup.id && (
                              <div className="setup-row-history">
                                {historyBusy && historyVersions.length === 0 ? (
                                  <small>Loading earlier versions…</small>
                                ) : historyVersions.length === 0 ? (
                                  <small>
                                    No earlier versions yet. One is kept each
                                    time this setup is saved over.
                                  </small>
                                ) : (
                                  <ul role="none">
                                    {historyVersions.map((version) => (
                                      <li key={version.id} role="none">
                                        <span>
                                          {new Date(
                                            version.saved_at,
                                          ).toLocaleString()}
                                        </span>
                                        <button
                                          aria-label={
                                            confirmingRollbackId === version.id
                                              ? `Confirm loading ${setup.name} from ${new Date(
                                                  version.saved_at,
                                                ).toLocaleString()}, replacing the model on screen`
                                              : `Roll ${setup.name} back to the version from ${new Date(
                                                  version.saved_at,
                                                ).toLocaleString()}`
                                          }
                                          className={
                                            confirmingRollbackId === version.id
                                              ? "confirm-delete"
                                              : ""
                                          }
                                          disabled={historyBusy}
                                          onClick={() =>
                                            void restoreSetupVersion(
                                              setup,
                                              version,
                                            )
                                          }
                                          role="menuitem"
                                          type="button"
                                        >
                                          {confirmingRollbackId === version.id
                                            ? "Confirm · replaces model"
                                            : "Roll back"}
                                        </button>
                                      </li>
                                    ))}
                                  </ul>
                                )}
                              </div>
                            )}
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
                    <button
                      onClick={() => {
                        sourceImportRef.current?.click();
                        closeSetupMenu(true);
                      }}
                      role="menuitem"
                      type="button"
                    >
                      Import source data
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
            <input
              accept="application/zip,.zip"
              aria-label="Import source data bundle"
              hidden
              onChange={(event) => {
                const file = event.target.files?.[0];
                event.target.value = "";
                if (file) void importSourceBundle(file);
              }}
              ref={sourceImportRef}
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
                <small
                  className={updateLine.isSummary ? "summary" : undefined}
                  title={updateStatusTitle}
                >
                  {updateLine.text}
                </small>
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
                  <ExternalLink href={availableUpdate.url || RELEASES_URL}>
                    Notes
                  </ExternalLink>
                </>
              ) : (
                <ExternalLink href={availableUpdate.url || RELEASES_URL}>
                  Download
                </ExternalLink>
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
            markerPlacementMode={
              movingMarkerIndex !== null
                ? "move"
                : markerPlacementKind
                  ? "place"
                  : null
            }
            onPlaceMarker={placeMarker}
            onCenterChange={onCenterChange}
            onGroundSpanChange={(groundSpanKm) =>
              update("ground_span_km", groundSpanKm)
            }
            onOutlineChange={(points) =>
              update("model_outline", {
                shape: "polygon",
                points,
              })
            }
            recallCount={setupRecallCount}
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
              progress={previewProgress}
              onCancelPreview={previewProgress ? cancelPreview : undefined}
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
                onClick={() => showSection(key)}
              >
                {label}
                {key === "output" && job && (
                  <span className={`tab-status ${job.status}`} />
                )}
              </button>
            ))}
          </div>

          {generationFailure && activeSection !== "output" && (
            <section className="generation-error-banner" role="alert">
              <span className="status-dot" aria-hidden="true" />
              <div className="generation-error-copy">
                <strong>{generationFailure.title}</strong>
                <span>{generationFailure.message}</span>
              </div>
              <div className="generation-error-actions">
                {generationFailure.control_tab &&
                  generationFailure.control_tab !== activeSection &&
                  generationFailure.control_tab !== "output" && (
                    <button
                      type="button"
                      onClick={() =>
                        showSection(generationFailure.control_tab ?? "output")
                      }
                    >
                      Open {CONTROL_TAB_LABELS[generationFailure.control_tab]}
                    </button>
                  )}
                <button type="button" onClick={() => showSection("output")}>
                  Technical details
                </button>
              </div>
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
            heightScaleReadout={heightScaleReadout}
            setHeightMode={setHeightMode}
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
            updateMarine={updateMarine}
          />

          <BuildingsPanel
            hidden={activeSection !== "buildings"}
            spec={spec}
            updateBuildings={updateBuildings}
          />

          <MarkersPanel
            hidden={activeSection !== "markers"}
            movingMarkerIndex={movingMarkerIndex}
            moveMarker={moveMarker}
            placementKind={markerPlacementKind}
            removeMarker={removeMarker}
            setPlacementKind={chooseMarkerPlacementKind}
            spec={spec}
            updateMarker={updateMarker}
          />

          <ColorsPanel
            discoveredGround={discoveredGround}
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
            buildSourceBundle={() => void buildSourceBundle()}
            buildingSourceBundle={buildingSourceBundle}
            failure={generationFailure}
            generationStages={generationStages}
            hidden={activeSection !== "output"}
            job={job}
            message={message}
            noteWebDownload={noteWebDownload}
            saveDesktopArtifactSet={saveDesktopArtifactSet}
            saveDesktopArtifact={saveDesktopArtifact}
            sourceBundle={jobSourceBundle}
            spec={spec}
            statusLabel={statusLabel}
            updateColor={updateColor}
          />
        </form>
      </div>
    </main>
  );
}
