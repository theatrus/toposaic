import { IS_TAURI } from "../api";
import type {
  Artifact,
  ArtifactFeedback,
  GenerationFailure,
  GenerationSpec,
  Job,
} from "../contracts";
import { ArtifactDownloads } from "../downloads";

export function OutputPanel({
  artifactFeedback,
  failure,
  generationStages,
  hidden,
  job,
  message,
  noteWebDownload,
  saveDesktopArtifact,
  saveDesktopArtifactSet,
  spec,
  statusLabel,
  updateColor,
}: {
  artifactFeedback: ArtifactFeedback | null;
  failure: GenerationFailure | null;
  generationStages: Array<{
    key: string;
    label: string;
    state: string;
    detail: string;
  }>;
  hidden: boolean;
  job: Job | null;
  message: string | null;
  noteWebDownload: (artifact: Artifact) => void;
  saveDesktopArtifact: (artifact: Artifact) => Promise<void>;
  saveDesktopArtifactSet: (key: string, names: string[]) => Promise<void>;
  spec: GenerationSpec;
  statusLabel: string | null;
  updateColor: <Key extends keyof GenerationSpec["color_output"]>(
    key: Key,
    value: GenerationSpec["color_output"][Key],
  ) => void;
}) {
  return (
    <>
      <div className="output-intro" hidden={hidden}>
        <strong>{job ? statusLabel : "No generation job yet."}</strong>
        <p>
          Generate a model to collect its color 3MF, tray, manifest, and
          optional STL files here.
        </p>
      </div>

      <fieldset
        className="control-section"
        aria-label="3MF export style"
        hidden={hidden}
      >
        <label className="road-detail-field">
          3MF style
          <select
            value={spec.color_output.threemf_style}
            onChange={(event) =>
              updateColor(
                "threemf_style",
                event.target
                  .value as GenerationSpec["color_output"]["threemf_style"],
              )
            }
          >
            <option value="project">
              Color project · filament colors and purge settings
            </option>
            <option value="painted">
              Painted colors (for Orca) · paint only, no presets
            </option>
            <option value="geometry">Geometry only · standard 3MF colors</option>
          </select>
          <small>
            Color project carries its colors for both slicers. Bambu Studio
            never applies an embedded filament list; its import dialog reads
            the file&apos;s color group instead, and Color match puts the
            palette on the filaments already loaded — use it rather than
            Append, which copies your last filament once per color and piles
            the copies up across imports. OrcaSlicer applies the embedded
            settings, so the filament list becomes the Colors tab palette.
            Painted colors is a plain pre-painted model: triangles carry
            extruder assignments 1..N, colors come from the filaments
            already loaded, and no presets are touched. Geometry only writes
            a plain standards-based 3MF for other tools.
          </small>
        </label>
      </fieldset>

      <div className="engine-note" hidden={hidden}>
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
          spec.buildings.enabled ||
          spec.markers.some((marker) => marker.kind === "building")) && (
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
          hidden={hidden}
        >
          <div>
            <span className="status-dot" />
            <strong>{message ?? statusLabel}</strong>
          </div>
          {job?.status === "failed" && failure && (
            <div className="generation-failure-detail">
              <p>{failure.message}</p>
              {failure.piece && (
                <p>
                  Affected piece: row {failure.piece.row}, column {failure.piece.column}
                </p>
              )}
              <details>
                <summary>Technical details</summary>
                <code>{failure.technical_detail}</code>
              </details>
            </div>
          )}
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
              onSaveSet={(key, names) => void saveDesktopArtifactSet(key, names)}
              onWebDownload={noteWebDownload}
            />
          )}
        </section>
      )}
    </>
  );
}
