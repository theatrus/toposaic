"use client";

import maplibregl, {
  type GeoJSONSource,
  type Map as MapLibreMap,
} from "maplibre-gl";
import "maplibre-gl/dist/maplibre-gl.css";
import {
  FormEvent,
  useCallback,
  useEffect,
  useMemo,
  useRef,
  useState,
} from "react";

type GenerationSpec = {
  center_lat: number;
  center_lon: number;
  ground_span_km: number;
  width_mm: number;
  rows: number;
  columns: number;
  base_mm: number;
  relief_mm: number;
  clearance_mm: number;
  samples_per_piece: number;
};

type Artifact = {
  name: string;
  media_type: string;
  bytes: number;
};

type Job = {
  id: string;
  status: "queued" | "running" | "complete" | "failed";
  progress: number;
  artifacts: Artifact[];
  error?: string | null;
};

type PreviewData = {
  width: number;
  height: number;
  values: number[];
  rows: number;
  columns: number;
};

const API_URL =
  process.env.NEXT_PUBLIC_TERRAIN_API_URL ?? "http://127.0.0.1:8787";

const initialSpec: GenerationSpec = {
  center_lat: 46.8523,
  center_lon: -121.7603,
  ground_span_km: 18,
  width_mm: 180,
  rows: 3,
  columns: 3,
  base_mm: 2.4,
  relief_mm: 14,
  clearance_mm: 0.22,
  samples_per_piece: 28,
};

function selectionPolygon(spec: GenerationSpec) {
  const halfLat = spec.ground_span_km / 2 / 110.574;
  const longitudeScale = Math.max(
    20,
    111.32 * Math.cos((spec.center_lat * Math.PI) / 180),
  );
  const halfLon = spec.ground_span_km / 2 / longitudeScale;
  return {
    type: "Feature" as const,
    properties: {},
    geometry: {
      type: "Polygon" as const,
      coordinates: [
        [
          [spec.center_lon - halfLon, spec.center_lat - halfLat],
          [spec.center_lon + halfLon, spec.center_lat - halfLat],
          [spec.center_lon + halfLon, spec.center_lat + halfLat],
          [spec.center_lon - halfLon, spec.center_lat + halfLat],
          [spec.center_lon - halfLon, spec.center_lat - halfLat],
        ],
      ],
    },
  };
}

function TerrainMap({
  spec,
  onCenterChange,
}: {
  spec: GenerationSpec;
  onCenterChange: (longitude: number, latitude: number) => void;
}) {
  const containerRef = useRef<HTMLDivElement>(null);
  const mapRef = useRef<MapLibreMap | null>(null);
  const callbackRef = useRef(onCenterChange);

  useEffect(() => {
    callbackRef.current = onCenterChange;
  }, [onCenterChange]);

  useEffect(() => {
    if (!containerRef.current || mapRef.current) return;
    const map = new maplibregl.Map({
      container: containerRef.current,
      center: [spec.center_lon, spec.center_lat],
      zoom: 9,
      attributionControl: false,
      style: {
        version: 8,
        sources: {
          osm: {
            type: "raster",
            tiles: ["https://tile.openstreetmap.org/{z}/{x}/{y}.png"],
            tileSize: 256,
            attribution: "© OpenStreetMap contributors",
          },
        },
        layers: [{ id: "osm", type: "raster", source: "osm" }],
      },
    });
    map.addControl(
      new maplibregl.NavigationControl({ showCompass: false }),
      "top-right",
    );
    map.addControl(
      new maplibregl.AttributionControl({ compact: true }),
      "bottom-right",
    );
    map.on("load", () => {
      map.addSource("selection", {
        type: "geojson",
        data: selectionPolygon(spec),
      });
      map.addLayer({
        id: "selection-fill",
        type: "fill",
        source: "selection",
        paint: {
          "fill-color": "#d8fb72",
          "fill-opacity": 0.18,
        },
      });
      map.addLayer({
        id: "selection-outline",
        type: "line",
        source: "selection",
        paint: {
          "line-color": "#d8fb72",
          "line-width": 3,
          "line-dasharray": [2, 1.5],
        },
      });
    });
    map.on("moveend", () => {
      const center = map.getCenter();
      callbackRef.current(center.lng, center.lat);
    });
    mapRef.current = map;
    return () => {
      map.remove();
      mapRef.current = null;
    };
    // The map owns its center after setup.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  useEffect(() => {
    const source = mapRef.current?.getSource("selection") as
      | GeoJSONSource
      | undefined;
    source?.setData(selectionPolygon(spec));
  }, [spec]);

  return (
    <div className="map-shell">
      <div ref={containerRef} className="map-canvas" aria-label="Terrain map" />
      <div className="map-crosshair" aria-hidden="true">
        <span />
        <span />
      </div>
      <div className="map-instruction">Move the map to choose a place</div>
    </div>
  );
}

function ReliefPreview({
  spec,
  preview,
}: {
  spec: GenerationSpec;
  preview: PreviewData | null;
}) {
  const canvasRef = useRef<HTMLCanvasElement>(null);

  useEffect(() => {
    const canvas = canvasRef.current;
    if (!canvas) return;
    const ratio = Math.min(window.devicePixelRatio || 1, 2);
    const width = canvas.clientWidth;
    const height = canvas.clientHeight;
    canvas.width = width * ratio;
    canvas.height = height * ratio;
    const context = canvas.getContext("2d");
    if (!context) return;
    context.scale(ratio, ratio);
    context.clearRect(0, 0, width, height);

    const samples = preview?.width ?? 32;
    const points: { x: number; y: number; z: number }[][] = [];
    const seedA = Math.sin((spec.center_lat * Math.PI) / 180) * 1.7;
    const seedB = Math.cos((spec.center_lon * Math.PI) / 180) * 1.3;
    for (let y = 0; y < samples; y += 1) {
      const row = [];
      for (let x = 0; x < samples; x += 1) {
        const u = x / (samples - 1);
        const v = y / (samples - 1);
        const z =
          preview?.values[y * samples + x] ??
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
          })();
        row.push({
          x: width * 0.5 + (u - v) * width * 0.38,
          y:
            height * 0.2 +
            (u + v) * height * 0.27 -
            z * Math.min(92, spec.relief_mm * 5),
          z,
        });
      }
      points.push(row);
    }

    for (let y = samples - 2; y >= 0; y -= 1) {
      for (let x = 0; x < samples - 1; x += 1) {
        const a = points[y][x];
        const b = points[y][x + 1];
        const c = points[y + 1][x + 1];
        const d = points[y + 1][x];
        const shade = Math.round(46 + ((a.z + b.z + c.z + d.z) / 4) * 72);
        context.beginPath();
        context.moveTo(a.x, a.y);
        context.lineTo(b.x, b.y);
        context.lineTo(c.x, c.y);
        context.lineTo(d.x, d.y);
        context.closePath();
        context.fillStyle = `hsl(75 28% ${shade}%)`;
        context.fill();
      }
    }

    context.strokeStyle = "rgba(15, 25, 23, 0.72)";
    context.lineWidth = 1.4;
    for (let column = 1; column < spec.columns; column += 1) {
      const u = column / spec.columns;
      context.beginPath();
      for (let y = 0; y < samples; y += 1) {
        const point = points[y][Math.round(u * (samples - 1))];
        if (y === 0) context.moveTo(point.x, point.y);
        else context.lineTo(point.x, point.y);
      }
      context.stroke();
    }
    for (let row = 1; row < spec.rows; row += 1) {
      const v = row / spec.rows;
      context.beginPath();
      for (let x = 0; x < samples; x += 1) {
        const point = points[Math.round(v * (samples - 1))][x];
        if (x === 0) context.moveTo(point.x, point.y);
        else context.lineTo(point.x, point.y);
      }
      context.stroke();
    }
  }, [preview, spec]);

  return (
    <div className="relief-shell">
      <canvas ref={canvasRef} className="relief-canvas" />
      <div className="preview-label">
        <span>{preview ? "Generated terrain" : "Fast shape preview"}</span>
        <strong>
          {spec.columns} × {spec.rows} pieces
        </strong>
      </div>
    </div>
  );
}

function RangeField({
  label,
  value,
  unit,
  min,
  max,
  step,
  onChange,
}: {
  label: string;
  value: number;
  unit: string;
  min: number;
  max: number;
  step: number;
  onChange: (value: number) => void;
}) {
  return (
    <label className="range-field">
      <span>
        {label}
        <output>
          {value}
          {unit}
        </output>
      </span>
      <input
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
  const [job, setJob] = useState<Job | null>(null);
  const [preview, setPreview] = useState<PreviewData | null>(null);
  const [submitting, setSubmitting] = useState(false);
  const [message, setMessage] = useState<string | null>(null);

  const update = useCallback(
    <Key extends keyof GenerationSpec>(key: Key, value: GenerationSpec[Key]) =>
      setSpec((current) => ({ ...current, [key]: value })),
    [],
  );

  const onCenterChange = useCallback((longitude: number, latitude: number) => {
    setSpec((current) => ({
      ...current,
      center_lat: Number(latitude.toFixed(5)),
      center_lon: Number(longitude.toFixed(5)),
    }));
  }, []);

  useEffect(() => {
    if (!job || !["queued", "running"].includes(job.status)) return;
    const timer = window.setInterval(async () => {
      try {
        const response = await fetch(`${API_URL}/api/jobs/${job.id}`);
        if (!response.ok) throw new Error("Could not read the job");
        const nextJob = (await response.json()) as Job;
        setJob(nextJob);
        if (nextJob.status === "complete") {
          const previewResponse = await fetch(
            `${API_URL}/api/jobs/${nextJob.id}/downloads/preview.json`,
          );
          if (previewResponse.ok) {
            setPreview((await previewResponse.json()) as PreviewData);
          }
        }
      } catch {
        setMessage("The generator stopped responding. The job is safe in SQLite.");
      }
    }, 900);
    return () => window.clearInterval(timer);
  }, [job]);

  const submit = async (event: FormEvent) => {
    event.preventDefault();
    setSubmitting(true);
    setMessage(null);
    setJob(null);
    setPreview(null);
    try {
      const response = await fetch(`${API_URL}/api/jobs`, {
        method: "POST",
        headers: { "content-type": "application/json" },
        body: JSON.stringify(spec),
      });
      const payload = await response.json();
      if (!response.ok) {
        throw new Error(payload.error ?? "Generation could not start");
      }
      setJob(payload as Job);
    } catch (error) {
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

  const statusLabel = useMemo(() => {
    if (!job) return null;
    if (job.status === "complete") return "Your print files are ready.";
    if (job.status === "failed") return job.error ?? "Generation failed.";
    if (job.status === "queued") return "Waiting for the generator…";
    return job.progress < 55
      ? "Sampling global elevation…"
      : "Building watertight pieces…";
  }, [job]);

  return (
    <main className="studio">
      <header className="topbar">
        <a className="brand" href="#" aria-label="Terrain Puzzle Studio home">
          <span className="brand-mark" aria-hidden="true">
            T6
          </span>
          <span>
            Terrain Puzzle
            <small>Rust mesh studio</small>
          </span>
        </a>
        <div className="build-state">
          <span />
          Local engine · SQLite
        </div>
      </header>

      <section className="hero">
        <div>
          <p className="eyebrow">Make a place you can hold</p>
          <h1>Turn any landscape into a puzzle.</h1>
        </div>
        <p className="hero-copy">
          Pick a place, tune the relief, then generate watertight 3MF and STL
          files for your printer.
        </p>
      </section>

      <div className="workspace">
        <section className="visual-column" aria-label="Place and model preview">
          <TerrainMap spec={spec} onCenterChange={onCenterChange} />
          <ReliefPreview spec={spec} preview={preview} />
        </section>

        <form className="controls" onSubmit={submit}>
          <div className="panel-heading">
            <span>01</span>
            <div>
              <h2>Shape your puzzle</h2>
              <p>All sizes use millimetres.</p>
            </div>
          </div>

          <div className="coordinate-row">
            <label>
              Latitude
              <input
                type="number"
                step="0.00001"
                value={spec.center_lat}
                onChange={(event) =>
                  update("center_lat", Number(event.target.value))
                }
              />
            </label>
            <label>
              Longitude
              <input
                type="number"
                step="0.00001"
                value={spec.center_lon}
                onChange={(event) =>
                  update("center_lon", Number(event.target.value))
                }
              />
            </label>
          </div>

          <RangeField
            label="Ground span"
            value={spec.ground_span_km}
            unit=" km"
            min={1}
            max={80}
            step={1}
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
            label="Terrain relief"
            value={spec.relief_mm}
            unit=" mm"
            min={3}
            max={35}
            step={1}
            onChange={(value) => update("relief_mm", value)}
          />
          <RangeField
            label="Fit clearance"
            value={spec.clearance_mm}
            unit=" mm"
            min={0}
            max={0.6}
            step={0.02}
            onChange={(value) => update("clearance_mm", value)}
          />

          <fieldset className="piece-grid">
            <legend>Piece layout</legend>
            {[2, 3, 4, 5].map((count) => (
              <button
                type="button"
                className={
                  spec.rows === count && spec.columns === count ? "active" : ""
                }
                key={count}
                onClick={() =>
                  setSpec((current) => ({
                    ...current,
                    rows: count,
                    columns: count,
                  }))
                }
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
                {count}×{count}
              </button>
            ))}
          </fieldset>

          <div className="engine-note">
            <span>Print source</span>
            <strong>
              <a
                href="https://github.com/tilezen/joerd/blob/master/docs/attribution.md"
                target="_blank"
                rel="noreferrer"
              >
                Global Mapzen elevation tiles
              </a>
            </strong>
            <p>
              The job saves source details and required notices in its manifest.
            </p>
          </div>

          <button className="generate-button" type="submit" disabled={submitting}>
            <span>{submitting ? "Starting…" : "Generate print files"}</span>
            <span aria-hidden="true">↗</span>
          </button>

          {(message || job) && (
            <section
              className={`job-card ${job?.status ?? "notice"}`}
              aria-live="polite"
            >
              <div>
                <span className="status-dot" />
                <strong>{message ?? statusLabel}</strong>
              </div>
              {job && job.status !== "failed" && (
                <div className="progress-track">
                  <span style={{ width: `${job.progress}%` }} />
                </div>
              )}
              {job?.status === "complete" && (
                <div className="downloads">
                  {job.artifacts
                    .filter(
                      (artifact) =>
                        artifact.name.endsWith(".3mf") ||
                        artifact.name === "manifest.json",
                    )
                    .map((artifact) => (
                      <a
                        key={artifact.name}
                        href={`${API_URL}/api/jobs/${job.id}/downloads/${artifact.name}`}
                      >
                        <span>{artifact.name}</span>
                        <small>
                          {(artifact.bytes / 1024 / 1024).toFixed(1)} MB
                        </small>
                      </a>
                    ))}
                  <details>
                    <summary>Separate STL pieces</summary>
                    <div>
                      {job.artifacts
                        .filter((artifact) => artifact.name.endsWith(".stl"))
                        .map((artifact) => (
                          <a
                            key={artifact.name}
                            href={`${API_URL}/api/jobs/${job.id}/downloads/${artifact.name}`}
                          >
                            {artifact.name}
                          </a>
                        ))}
                    </div>
                  </details>
                </div>
              )}
            </section>
          )}
        </form>
      </div>
    </main>
  );
}
