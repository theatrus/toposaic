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
import {
  FINE_DEM_MAX_SPAN_KM,
  MAX_ASSEMBLED_SAMPLES,
  MAX_SUPER_TILE_SIDE,
  MESH_QUALITY_OPTIONS,
  assembledMeshSamples,
  automaticRoadDetail,
  deriveHeightFrame,
  formatGroundSpacing,
  groundMeshSpacing,
  initialSpec,
} from "./config";
import type {
  Artifact,
  ArtifactFeedback,
  GenerationSpec,
  Job,
  PlaceResult,
  PreviewData,
} from "./contracts";
import { type AdjacentDirection, adjacentCenter } from "./geo";
import { ArtifactDownloads } from "./downloads";
import { TerrainMap } from "./map";
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

const ADJACENT_GRID_SIZES = Array.from(
  { length: MAX_SUPER_TILE_SIDE },
  (_, index) => index + 1,
);

function oddSuperTileSize(value: number) {
  if (value % 2 === 1) return value;
  return value >= MAX_SUPER_TILE_SIDE ? MAX_SUPER_TILE_SIDE - 1 : value + 1;
}

function RangeField({
  label,
  value,
  unit,
  displayValue,
  min,
  max,
  step,
  onChange,
}: {
  label: string;
  value: number;
  unit: string;
  displayValue?: string;
  min: number;
  max: number;
  step: number;
  onChange: (value: number) => void;
}) {
  return (
    <label className="range-field">
      <span>
        {label}
        <output>{displayValue ?? `${value}${unit}`}</output>
      </span>
      <input
        aria-label={label}
        type="range"
        min={min}
        max={max}
        step={step}
        value={value}
        onChange={(event) => onChange(Number(event.target.value))}
      />
    </label>
  );
}

export function TerrainStudio() {
  const [spec, setSpec] = useState(initialSpec);
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
    "model" | "surface" | "buildings" | "tray" | "output"
  >("model");
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
  const workspaceRef = useRef<HTMLDivElement>(null);
  const resizePointerRef = useRef<number | null>(null);

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

  const update = useCallback(
    <Key extends keyof GenerationSpec>(key: Key, value: GenerationSpec[Key]) => {
      setGeneratedPreview(null);
      setSpec((current) => ({ ...current, [key]: value }));
    },
    [],
  );
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
      setSpec((current) => ({
        ...current,
        color_output: { ...current.color_output, [key]: value },
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
    update("place_name", place.display_name.split(",")[0].trim().slice(0, 48));
    setPlaceResults([]);
    setPlaceMessage(`Map moved to ${place.display_name.split(",")[0]}.`);
    setGeneratedPreview(null);
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
      (job.spec.color_output.enabled || job.spec.buildings.enabled)
    ) {
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
    const hasSurface =
      job.spec.color_output.enabled || job.spec.buildings.enabled;
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
                  <a
                    href={availableUpdate.url || RELEASES_URL}
                    target="_blank"
                    rel="noreferrer"
                  >
                    Notes
                  </a>
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
          <div className={`build-state ${job?.status ?? "idle"}`}>
            <span />
            {job ? statusLabel : "Local engine · SQLite"}
          </div>
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
        <section className="visual-column" aria-label="Place and model preview">
          <TerrainMap
            spec={spec}
            onCenterChange={onCenterChange}
            onGroundSpanChange={(groundSpanKm) =>
              update("ground_span_km", groundSpanKm)
            }
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
                ["tray", "Tray"],
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

          <section
            className="control-section model-controls"
            hidden={activeSection !== "model"}
          >
            <div className="place-search">
              <label htmlFor="place-search-input">Find a place</label>
              <div className="place-search-row">
                <input
                  id="place-search-input"
                  type="search"
                  value={placeQuery}
                  placeholder="Mountain, park, city…"
                  onChange={(event) => setPlaceQuery(event.target.value)}
                  onKeyDown={(event) => {
                    if (event.key === "Enter") {
                      event.preventDefault();
                      void searchPlaces();
                    }
                  }}
                />
                <button
                  type="button"
                  disabled={searchingPlaces}
                  onClick={() => void searchPlaces()}
                >
                  {searchingPlaces ? "Searching…" : "Search"}
                </button>
              </div>
              {placeMessage && (
                <p className="place-search-message" role="status">
                  {placeMessage}
                </p>
              )}
              {placeResults.length > 0 && (
                <ul className="place-results" aria-label="Place search results">
                  {placeResults.map((place) => (
                    <li
                      key={`${place.latitude}-${place.longitude}-${place.display_name}`}
                    >
                      <button type="button" onClick={() => choosePlace(place)}>
                        <span>{place.display_name}</span>
                        <small>
                          {place.category} · {place.kind.replaceAll("_", " ")}
                        </small>
                      </button>
                    </li>
                  ))}
                </ul>
              )}
              <p className="place-search-note">
                Submit-only search sends public place names to{" "}
                <a
                  href="https://www.openstreetmap.org/copyright"
                  target="_blank"
                  rel="noreferrer"
                >
                  OpenStreetMap
                </a>
                . Do not enter private information.
              </p>
            </div>

            <div className="coordinate-adjacent-row">
              <div
                className="coordinate-box"
                role="group"
                aria-label="Map center"
              >
                <strong>Map center</strong>
                <div className="coordinate-row">
                  <label>
                    Latitude
                    <input
                      type="number"
                      step="0.00001"
                      value={spec.center_lat}
                      onChange={(event) => {
                        const latitude = Number(event.target.value);
                        if (
                          event.target.value.trim() === "" ||
                          !Number.isFinite(latitude)
                        ) {
                          return;
                        }
                        update("center_lat", latitude);
                      }}
                    />
                  </label>
                  <label>
                    Longitude
                    <input
                      type="number"
                      step="0.00001"
                      value={spec.center_lon}
                      onChange={(event) => {
                        const longitude = Number(event.target.value);
                        if (
                          event.target.value.trim() === "" ||
                          !Number.isFinite(longitude)
                        ) {
                          return;
                        }
                        update("center_lon", longitude);
                      }}
                    />
                  </label>
                </div>
              </div>
              <div
                className="adjacent-tiles"
                role="group"
                aria-label="Super-tile mode"
              >
                <div className="adjacent-heading">
                  <strong>Super-tile mode</strong>
                  <button
                    type="button"
                    onClick={
                      heightFrameLocked ? unlockHeightFrame : lockHeightFrame
                    }
                  >
                    {heightFrameLocked ? "Unlock height" : "Lock height"}
                  </button>
                </div>
                <div className="adjacent-compact-row">
                  <div className="adjacent-actions" aria-label="Move one tile">
                    {(["north", "west", "east", "south"] as const).map(
                      (direction) => (
                        <button
                          type="button"
                          key={direction}
                          aria-label={`Move ${direction} one tile`}
                          title={`Move ${direction} one tile`}
                          onClick={() => moveToAdjacentTile(direction)}
                        >
                          <span aria-hidden="true">
                            {direction === "north"
                              ? "↑"
                              : direction === "south"
                                ? "↓"
                                : direction === "east"
                                  ? "→"
                                  : "←"}
                          </span>
                        </button>
                      ),
                    )}
                  </div>
                  <div className="adjacent-grid" aria-label="Super-tile grid">
                    <span>Grid</span>
                    <label>
                      Across
                      <select
                        value={spec.adjacent_columns}
                        onChange={(event) =>
                          update("adjacent_columns", Number(event.target.value))
                        }
                      >
                        {superTileGridSizes.map((value) => (
                          <option key={value}>{value}</option>
                        ))}
                      </select>
                    </label>
                    <span aria-hidden="true">×</span>
                    <label>
                      Down
                      <select
                        value={spec.adjacent_rows}
                        onChange={(event) =>
                          update("adjacent_rows", Number(event.target.value))
                        }
                      >
                        {superTileGridSizes.map((value) => (
                          <option key={value}>{value}</option>
                        ))}
                      </select>
                    </label>
                  </div>
                </div>
                <div
                  className="super-tile-anchor"
                  role="radiogroup"
                  aria-label="Super-tile anchor"
                >
                  <span>Anchor</span>
                  <label>
                    <input
                      type="radio"
                      name="super-tile-anchor"
                      checked={spec.super_tile_anchor === "top_left"}
                      onChange={() => setSuperTileAnchor("top_left")}
                    />
                    <span>Top-left tile</span>
                  </label>
                  <label>
                    <input
                      type="radio"
                      name="super-tile-anchor"
                      checked={spec.super_tile_anchor === "center"}
                      onChange={() => setSuperTileAnchor("center")}
                    />
                    <span>Center tile</span>
                  </label>
                </div>
                {(spec.adjacent_columns > 1 || spec.adjacent_rows > 1) && (
                  <label className="adjacent-interlock-toggle">
                    <input
                      type="checkbox"
                      checked={spec.adjacent_interlocks}
                      onChange={(event) =>
                        update("adjacent_interlocks", event.target.checked)
                      }
                    />
                    Interlock super-tile and tray edges
                  </label>
                )}
                <p
                  className={`height-frame-status${
                    heightFrameLocked && !heightFrameCompatible
                      ? " warning"
                      : ""
                  }`}
                  role={
                    heightFrameLocked && !heightFrameCompatible
                      ? "alert"
                      : "status"
                  }
                >
                  {heightFrameLocked && !heightFrameCompatible
                    ? `Shared datum ${spec.elevation_datum_m?.toFixed(
                        1,
                      )} m · ${spec.elevation_m_per_mm?.toFixed(
                        1,
                      )} m/mm. This tile drops below the shared ${spec.elevation_datum_m?.toFixed(
                        1,
                      )} m datum. Lower the datum and regenerate earlier tiles.`
                    : heightFrameLocked
                      ? `Shared datum ${spec.elevation_datum_m?.toFixed(
                          1,
                        )} m · ${spec.elevation_m_per_mm?.toFixed(1)} m/mm`
                      : spec.adjacent_columns > 1 || spec.adjacent_rows > 1
                        ? `${spec.adjacent_columns * spec.adjacent_rows} terrain 3MF files; current tile is the ${
                            spec.super_tile_anchor === "center"
                              ? "grid center"
                              : "top-left tile"
                          }. The super-tile shares one height frame.`
                        : "Auto height fits one tile; manual neighbors may form a step."}
                </p>
                {adjacentMessage && (
                  <p className="adjacent-message">{adjacentMessage}</p>
                )}
              </div>
            </div>
            <label className="elevation-source-field">
              Elevation tiles
              <select
                value={spec.elevation_source}
                onChange={(event) =>
                  update(
                    "elevation_source",
                    event.target.value as GenerationSpec["elevation_source"],
                  )
                }
              >
                <option value="mapzen">Mapzen · global coverage</option>
                <option value="mapterhorn">
                  Mapterhorn · higher detail where available
                </option>
              </select>
              <small>
                Mapterhorn uses 512 px Terrarium tiles and falls back to its
                global layer outside regional high-detail coverage.
              </small>
            </label>
            {spec.elevation_source === "mapterhorn" && (
              <label className="adjacent-interlock-toggle fine-dem-toggle">
                <input
                  type="checkbox"
                  checked={spec.fine_dem_detail}
                  disabled={
                    spec.ground_span_km > FINE_DEM_MAX_SPAN_KM ||
                    spec.mesh_samples_across === MAX_ASSEMBLED_SAMPLES
                  }
                  onChange={(event) =>
                    update("fine_dem_detail", event.target.checked)
                  }
                />
                Use finest available DEM detail
                <small>
                  {spec.mesh_samples_across === MAX_ASSEMBLED_SAMPLES
                    ? "Ultra already uses the maximum 2,048-sample budget."
                    : spec.ground_span_km > FINE_DEM_MAX_SPAN_KM
                      ? "Zoom to 2 km or less to enable the fine-detail budget."
                      : "Increase the selected budget toward a 0.25 m target when Mapterhorn has finer tiles, up to 2,048 samples."}
                </small>
              </label>
            )}

            <RangeField
              label="Ground span"
              value={spec.ground_span_km}
              unit=" km"
              min={0.25}
              max={80}
              step={0.25}
              onChange={(value) => update("ground_span_km", value)}
            />
            <RangeField
              label="Print width"
              value={spec.width_mm}
              unit=" mm"
              min={80}
              max={300}
              step={5}
              onChange={(value) => update("width_mm", value)}
            />
            <RangeField
              label="Terrain height"
              value={spec.relief_mm}
              unit=" mm"
              min={3}
              max={80}
              step={1}
              onChange={(value) => update("relief_mm", value)}
            />
            <RangeField
              label="Minimum piece height"
              value={spec.base_mm}
              unit=" mm"
              min={1}
              max={12}
              step={0.2}
              onChange={(value) => update("base_mm", value)}
            />
            <fieldset className="mesh-quality">
              <legend>
                Mesh detail
                <span>
                  {assembledMeshSamples(spec)} across · about{" "}
                  {formatGroundSpacing(groundMeshSpacing(spec))} m ground
                  spacing
                </span>
              </legend>
              <div role="radiogroup" aria-label="Mesh detail">
                {MESH_QUALITY_OPTIONS.map((option) => (
                  <button
                    key={option.samples}
                    type="button"
                    role="radio"
                    aria-checked={
                      spec.mesh_samples_across === option.samples
                    }
                    className={
                      spec.mesh_samples_across === option.samples
                        ? "active"
                        : ""
                    }
                    onClick={() => setMeshQuality(option.samples)}
                  >
                    <strong>{option.label}</strong>
                    <span>{option.samples}</span>
                    <small>{option.note}</small>
                  </button>
                ))}
              </div>
              {spec.mesh_samples_across === MAX_ASSEMBLED_SAMPLES && (
                <p>
                  Ultra produces about four times as many surface triangles as
                  High and can make large 3MF files.
                </p>
              )}
            </fieldset>
            {!spec.solid_model && (
              <RangeField
                label="Fit clearance"
                value={spec.clearance_mm}
                unit=" mm"
                min={0}
                max={0.4}
                step={0.02}
                onChange={(value) => update("clearance_mm", value)}
              />
            )}
          </section>

          <fieldset
            className="color-controls control-section surface-controls"
            aria-label="Surface colors"
            hidden={activeSection !== "surface"}
          >
            <div className="color-heading">
              <div>
                <strong className="color-title">Surface colors</strong>
                <p>Paint the 3MF from mapped land cover and routes.</p>
              </div>
              <label className="color-toggle">
                <input
                  type="checkbox"
                  checked={spec.color_output.enabled}
                  onChange={(event) =>
                    updateColor("enabled", event.target.checked)
                  }
                />
                <span>{spec.color_output.enabled ? "On" : "Off"}</span>
              </label>
            </div>
            {spec.color_output.enabled && (
              <>
                <div className="color-swatches">
                  {(
                    [
                      ["Forest", "forest_color"],
                      ["Rock", "rock_color"],
                      ["Snow", "snow_color"],
                      ["Water", "water_color"],
                      ["Route", "road_color"],
                    ] as const
                  ).map(([label, key]) => (
                    <label key={key}>
                      <input
                        type="color"
                        value={spec.color_output[key]}
                        onChange={(event) => updateColor(key, event.target.value)}
                      />
                      <span>{label}</span>
                      <code>{spec.color_output[key].toUpperCase()}</code>
                    </label>
                  ))}
                </div>
                <RangeField
                  label="Smallest color patch"
                  value={spec.color_output.minimum_patch_mm}
                  unit=" mm"
                  min={0.4}
                  max={4}
                  step={0.2}
                  onChange={(value) => updateColor("minimum_patch_mm", value)}
                />
                <div className="road-options">
                  <label className="color-toggle">
                    <input
                      type="checkbox"
                      checked={spec.color_output.osm_water_enabled}
                      onChange={(event) =>
                        updateColor("osm_water_enabled", event.target.checked)
                      }
                    />
                    <span>OpenStreetMap waterways</span>
                  </label>
                  <small>
                    Adds smooth rivers, streams, canals, and mapped water areas
                  </small>
                </div>
                {spec.color_output.osm_water_enabled && (
                  <RangeField
                    label="Maximum waterway coverage"
                    value={spec.color_output.waterway_coverage_percent}
                    unit="%"
                    min={0}
                    max={100}
                    step={1}
                    onChange={(value) =>
                      updateColor("waterway_coverage_percent", value)
                    }
                  />
                )}
                {spec.color_output.osm_water_enabled && (
                  <p className="control-hint">
                    Keeps rivers and canals, then adds the longest streams up to
                    this share of the print surface. Set 0% for major waterways
                    only or 100% for every mapped stream. Lakes are unchanged.
                  </p>
                )}
                <div className="road-options">
                  <label className="color-toggle">
                    <input
                      type="checkbox"
                      checked={spec.color_output.roads_enabled}
                      onChange={(event) =>
                        updateColor("roads_enabled", event.target.checked)
                      }
                    />
                    <span>Render roads</span>
                  </label>
                  <small>Falls back to trails when no roads cross the map</small>
                </div>
                {spec.color_output.roads_enabled && (
                  <>
                    <label className="road-detail-field">
                      Route detail
                      <select
                        value={spec.color_output.road_detail}
                        onChange={(event) =>
                          updateColor(
                            "road_detail",
                            event.target
                              .value as GenerationSpec["color_output"]["road_detail"],
                          )
                        }
                      >
                        <option value="automatic">
                          Automatic for map span
                        </option>
                        <option value="major">Major roads only</option>
                        <option value="minor">
                          Major and minor roads
                        </option>
                        <option value="streets">
                          Roads and local streets
                        </option>
                        <option value="all">
                          Streets, paths, and trails
                        </option>
                      </select>
                      <small>
                        {spec.color_output.road_detail === "automatic"
                          ? `At ${spec.ground_span_km.toLocaleString()} km, automatic mode includes ${automaticRoadDetail(
                              spec.ground_span_km,
                            )}.`
                          : "The chosen detail applies at every map span."}
                      </small>
                    </label>
                    <RangeField
                      label="Route print width"
                      value={spec.color_output.road_width_mm}
                      unit=" mm"
                      min={0.4}
                      max={4}
                      step={0.1}
                      onChange={(value) => updateColor("road_width_mm", value)}
                    />
                    <div className="road-options">
                      <label className="color-toggle">
                        <input
                          type="checkbox"
                          checked={spec.color_output.adaptive_road_widths}
                          onChange={(event) =>
                            updateColor(
                              "adaptive_road_widths",
                              event.target.checked,
                            )
                          }
                        />
                        <span>Thin dense road networks</span>
                      </label>
                      <small>
                        Reduces route width as mapped road coverage rises. It
                        does not remove road classes.
                      </small>
                    </div>
                    <RangeField
                      label="Road layer height"
                      value={spec.color_output.road_height_mm}
                      unit=" mm"
                      min={0.08}
                      max={0.4}
                      step={0.02}
                      onChange={(value) => updateColor("road_height_mm", value)}
                    />
                    <div
                      className="road-options bridge-options"
                      role="group"
                      aria-label="Bridge structure"
                    >
                      <strong>Bridge structure</strong>
                      <label className="color-toggle">
                        <input
                          type="radio"
                          name="bridge-structure"
                          checked={
                            spec.color_output.bridge_structure === "floating"
                          }
                          onChange={() =>
                            updateColor("bridge_structure", "floating")
                          }
                        />
                        <span>Floating</span>
                      </label>
                      <small>Uses a thick deck between the abutments</small>
                      <label className="color-toggle">
                        <input
                          type="radio"
                          name="bridge-structure"
                          checked={
                            spec.color_output.bridge_structure === "supported"
                          }
                          onChange={() =>
                            updateColor("bridge_structure", "supported")
                          }
                        />
                        <span>Fully supported</span>
                      </label>
                      <small>
                        Fills from the deck down to the mapped ground or water
                      </small>
                    </div>
                    {spec.color_output.bridge_structure === "floating" && (
                      <RangeField
                        label="Floating bridge thickness"
                        value={spec.color_output.bridge_thickness_mm}
                        unit=" mm"
                        min={0.4}
                        max={6}
                        step={0.2}
                        onChange={(value) =>
                          updateColor("bridge_thickness_mm", value)
                        }
                      />
                    )}
                  </>
                )}
                <p className="color-note">
                  WorldCover supplies permanent water. OpenStreetMap waterways
                  add smooth lakes, rivers, streams, and canals when enabled.
                  Routes come from OpenStreetMap. The generator uses prominent
                  roads first, then trails only when no roads cross the model.
                  Tagged bridges can use thick floating decks or solid support
                  down to mapped ground or water. Untagged routes follow the
                  terrain. Tunnels stay hidden. The road layer height controls
                  the colored top surface, not bridge thickness. Snow is not
                  live. Sides and bottoms use the rock color.
                </p>
              </>
            )}
          </fieldset>

          <fieldset
            className="color-controls building-controls control-section"
            aria-label="Mapped buildings"
            hidden={activeSection !== "buildings"}
          >
            <div className="color-heading">
              <div>
                <strong className="color-title">Mapped buildings</strong>
                <p>Raise OpenStreetMap building footprints above the terrain.</p>
              </div>
              <label className="color-toggle">
                <input
                  type="checkbox"
                  checked={spec.buildings.enabled}
                  onChange={(event) =>
                    updateBuildings("enabled", event.target.checked)
                  }
                />
                <span>{spec.buildings.enabled ? "On" : "Off"}</span>
              </label>
            </div>
            <div className="color-swatches building-color-swatch">
              <label>
                <input
                  aria-label="Building color"
                  type="color"
                  value={spec.color_output.building_color}
                  onChange={(event) =>
                    updateColor("building_color", event.target.value)
                  }
                />
                <span>Building color</span>
                <code>{spec.color_output.building_color.toUpperCase()}</code>
              </label>
            </div>
            {spec.buildings.enabled && (
              <>
                <RangeField
                  label="Building Z scale"
                  value={spec.buildings.z_scale}
                  unit="×"
                  min={0.5}
                  max={30}
                  step={0.5}
                  onChange={(value) => updateBuildings("z_scale", value)}
                />
                <p className="color-note">
                  Buildings use exact mapped footprints, flat roofs, straight
                  vertical walls, and their own 3MF color material. 1× keeps
                  true height against the map width. Higher values make small
                  buildings easier to print. Tagged heights are used first,
                  then floor count, then an 8 m default.
                </p>
              </>
            )}
          </fieldset>

          <div
            className="model-mode"
            role="group"
            aria-label="Model type"
            hidden={activeSection !== "model"}
          >
            <strong className="model-mode-label">Model type</strong>
            <button
              type="button"
              className={!spec.solid_model ? "active" : ""}
              onClick={() => update("solid_model", false)}
            >
              <span className="mode-mark puzzle-mark" aria-hidden="true">
                <i />
                <i />
                <i />
                <i />
              </span>
              <span>
                <strong>Jigsaw puzzle</strong>
                <small>
                  {spec.puzzle_tabs
                    ? "Separate interlocking pieces"
                    : "Separate pieces with plain cuts"}
                </small>
              </span>
            </button>
            <button
              type="button"
              className={spec.solid_model ? "active" : ""}
              onClick={() => update("solid_model", true)}
            >
              <span className="mode-mark solid-mark" aria-hidden="true" />
              <span>
                <strong>Solid terrain</strong>
                <small>One watertight model, no cuts</small>
              </span>
            </button>
          </div>

          {!spec.solid_model && (
            <fieldset className="piece-grid" hidden={activeSection !== "model"}>
              <legend>Piece layout</legend>
              {[4, 6, 8, 10, 12].map((count) => (
                <button
                  type="button"
                  className={
                    spec.rows === count && spec.columns === count
                      ? "active"
                      : ""
                  }
                  key={count}
                  onClick={() => {
                    setGeneratedPreview(null);
                    setSpec((current) => ({
                      ...current,
                      rows: count,
                      columns: count,
                    }));
                  }}
                >
                  <span
                    className="mini-grid"
                    style={{
                      gridTemplateColumns: `repeat(${count}, 1fr)`,
                    }}
                  >
                    {Array.from({ length: count * count }).map((_, index) => (
                      <i key={index} />
                    ))}
                  </span>
                  <span>
                    {count}×{count}
                  </span>
                  <small>{count * count} pieces</small>
                </button>
              ))}
              <div className="piece-custom">
                <label>
                  Columns
                  <select
                    value={spec.columns}
                    onChange={(event) =>
                      update("columns", Number(event.target.value))
                    }
                  >
                    {Array.from({ length: 15 }, (_, index) => index + 2).map(
                      (count) => (
                        <option key={count} value={count}>
                          {count}
                        </option>
                      ),
                    )}
                  </select>
                </label>
                <label>
                  Rows
                  <select
                    value={spec.rows}
                    onChange={(event) =>
                      update("rows", Number(event.target.value))
                    }
                  >
                    {Array.from({ length: 15 }, (_, index) => index + 2).map(
                      (count) => (
                        <option key={count} value={count}>
                          {count}
                        </option>
                      ),
                    )}
                  </select>
                </label>
                <div>
                  <strong>{spec.rows * spec.columns} pieces</strong>
                  <small>
                    About {(spec.width_mm / spec.columns).toFixed(1)} mm wide
                    each
                  </small>
                </div>
              </div>
              <div
                className="piece-shape-options"
                role="group"
                aria-label="Piece shape"
              >
                <label>
                  <input
                    type="checkbox"
                    checked={spec.straight_piece_sides}
                    onChange={(event) =>
                      update("straight_piece_sides", event.target.checked)
                    }
                  />
                  <span>
                    <strong>Straight piece sides</strong>
                    <small>Align each cut instead of warping the grid</small>
                  </span>
                </label>
                <label>
                  <input
                    type="checkbox"
                    checked={spec.puzzle_tabs}
                    onChange={(event) =>
                      update("puzzle_tabs", event.target.checked)
                    }
                  />
                  <span>
                    <strong>Interlocking tabs</strong>
                    <small>Turn off for tab-less pieces with plain cuts</small>
                  </span>
                </label>
              </div>
              {spec.width_mm / spec.columns < 10 && (
                <p className="piece-warning">
                  These pieces are under 10 mm wide. Increase print width for
                  stronger pieces and easier handling.
                </p>
              )}
            </fieldset>
          )}

          <fieldset
            className="color-controls tray-controls control-section"
            aria-label="Shallow terrain tray"
            hidden={activeSection !== "tray"}
          >
            <div className="color-heading">
              <div>
                <strong className="color-title">Shallow terrain tray</strong>
                <p>A fitted base for the terrain or puzzle pieces.</p>
              </div>
              <label className="color-toggle">
                <input
                  type="checkbox"
                  checked={spec.tray.enabled}
                  onChange={(event) =>
                    updateTray("enabled", event.target.checked)
                  }
                />
                <span>{spec.tray.enabled ? "On" : "Off"}</span>
              </label>
            </div>
            <label className="place-label-field">
              Place name
              <input
                type="text"
                maxLength={48}
                required
                value={spec.place_name}
                onChange={(event) => update("place_name", event.target.value)}
              />
              <small>The tray adds the coordinates after this name.</small>
            </label>
            {spec.tray.enabled && (
              <>
                <div className="color-swatches">
                  {(
                    [
                      ["Tray", "tray_color"],
                      ["Contours", "contour_color"],
                      ["Label", "label_color"],
                    ] as const
                  ).map(([label, key]) => (
                    <label key={key}>
                      <input
                        type="color"
                        value={spec.tray[key]}
                        onChange={(event) =>
                          updateTray(key, event.target.value)
                        }
                      />
                      <span>{label}</span>
                      <code>{String(spec.tray[key]).toUpperCase()}</code>
                    </label>
                  ))}
                </div>
                <RangeField
                  label="Tray clearance"
                  value={spec.tray.clearance_mm}
                  unit=" mm"
                  min={0.2}
                  max={2}
                  step={0.1}
                  onChange={(value) => updateTray("clearance_mm", value)}
                />
                <RangeField
                  label="Rim width"
                  value={spec.tray.rim_width_mm}
                  unit=" mm"
                  min={5}
                  max={16}
                  step={0.5}
                  onChange={(value) => updateTray("rim_width_mm", value)}
                />
                <RangeField
                  label="Floor thickness"
                  value={spec.tray.floor_mm}
                  unit=" mm"
                  min={1}
                  max={4}
                  step={0.2}
                  onChange={(value) => updateTray("floor_mm", value)}
                />
                <RangeField
                  label="Rim height above floor"
                  value={spec.tray.rim_height_mm}
                  unit=" mm"
                  min={2}
                  max={8}
                  step={0.2}
                  onChange={(value) => updateTray("rim_height_mm", value)}
                />
                <RangeField
                  label="Contour lines"
                  value={spec.tray.contour_count}
                  unit=""
                  min={5}
                  max={60}
                  step={1}
                  onChange={(value) => updateTray("contour_count", value)}
                />
                {(spec.adjacent_columns > 1 || spec.adjacent_rows > 1) && (
                  <label className="tray-chunk-toggle">
                    <input
                      type="checkbox"
                      checked={spec.tray.individual_tiles}
                      onChange={(event) =>
                        updateTray("individual_tiles", event.target.checked)
                      }
                    />
                    <span>
                      <strong>Separate framed trays</strong>
                      <small>
                        Make one complete tray per terrain tile instead of one
                        joined mosaic tray.
                      </small>
                    </span>
                  </label>
                )}
                <p className="color-note">
                  The color 3MF prints contour lines on the flat tray floor and
                  the place name, latitude, and longitude as raised shapes on
                  the top front lip. Mosaic trays follow the terrain grid and
                  its shared-edge setting. The job also includes a plain STL.
                </p>
              </>
            )}
          </fieldset>

          <div className="output-intro" hidden={activeSection !== "output"}>
            <strong>{job ? statusLabel : "No generation job yet."}</strong>
            <p>
              Generate a model to collect its color 3MF, tray, manifest, and
              optional STL files here.
            </p>
          </div>

          <div className="engine-note" hidden={activeSection !== "output"}>
            <span>Print source</span>
            <strong>
              <a
                href={
                  spec.elevation_source === "mapterhorn"
                    ? "https://mapterhorn.com/attribution"
                    : "https://github.com/tilezen/joerd/blob/master/docs/attribution.md"
                }
                target="_blank"
                rel="noreferrer"
              >
                {spec.elevation_source === "mapterhorn"
                  ? "Mapterhorn elevation tiles"
                  : "Global Mapzen elevation tiles"}
              </a>
            </strong>
            {spec.color_output.enabled && (
              <strong>
                <a
                  href="https://worldcover2021.esa.int/download"
                  target="_blank"
                  rel="noreferrer"
                >
                  ESA WorldCover 2021 surface classes
                </a>
              </strong>
            )}
            {((spec.color_output.enabled &&
              spec.color_output.roads_enabled) ||
              spec.buildings.enabled) && (
              <strong>
                <a
                  href="https://www.openstreetmap.org/copyright"
                  target="_blank"
                  rel="noreferrer"
                >
                  OpenStreetMap route and building data
                </a>
              </strong>
            )}
            <p>
              The job saves source details and required notices in its manifest.
            </p>
          </div>

          {(message || job) && (
            <section
              className={`job-card ${job?.status ?? "notice"}`}
              aria-live="polite"
              hidden={activeSection !== "output"}
            >
              <div>
                <span className="status-dot" />
                <strong>{message ?? statusLabel}</strong>
              </div>
              {job && (
                <ol
                  className="job-steps"
                  aria-label="Generation progress"
                >
                  {generationStages.map((stage) => (
                    <li key={stage.key} className={stage.state}>
                      <span aria-hidden="true" />
                      <div>
                        <strong>{stage.label}</strong>
                        <small>{stage.detail}</small>
                      </div>
                    </li>
                  ))}
                </ol>
              )}
              {job && !["failed", "canceled"].includes(job.status) && (
                <div className="job-progress">
                  <div className="progress-track">
                    <span style={{ width: `${job.progress}%` }} />
                  </div>
                  <output>{job.progress}%</output>
                </div>
              )}
              {job?.status === "complete" && (
                <ArtifactDownloads
                  feedback={artifactFeedback}
                  isDesktop={IS_TAURI}
                  job={job}
                  onSave={(artifact) => void saveDesktopArtifact(artifact)}
                  onWebDownload={noteWebDownload}
                />
              )}
            </section>
          )}
        </form>
      </div>
    </main>
  );
}
