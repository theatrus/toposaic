import type {
  Artifact,
  ArtifactFeedback,
  Job,
  SourceBundleSummary,
} from "./contracts";
import { terrainApi } from "./api";

// Feedback slots for the two folder saves. Artifact names are file names,
// so neither can collide with one.
const SAVE_PRINT_KEY = "*print*";
const SAVE_STL_KEY = "*stl*";

type ArtifactActionProps = {
  artifact: Artifact;
  feedback: ArtifactFeedback | null;
  isDesktop: boolean;
  jobId: string;
  desktopFallback: string;
  webFallback: string;
  onSave: (artifact: Artifact) => void;
  onWebDownload: (artifact: Artifact) => void;
};

function feedbackLabel(
  artifact: Artifact,
  feedback: ArtifactFeedback | null,
  fallback: string,
) {
  if (feedback?.name !== artifact.name) return fallback;
  if (feedback.state === "saving") return "Choosing & saving…";
  if (feedback.state === "saved") return "Saved";
  return "Sent to browser";
}

function ArtifactAction({
  artifact,
  feedback,
  isDesktop,
  jobId,
  desktopFallback,
  webFallback,
  onSave,
  onWebDownload,
}: ArtifactActionProps) {
  if (isDesktop) {
    return (
      <button
        type="button"
        disabled={feedback?.state === "saving"}
        onClick={() => onSave(artifact)}
      >
        <span>{artifact.name}</span>
        <small aria-live="polite">
          {feedbackLabel(artifact, feedback, desktopFallback)}
        </small>
      </button>
    );
  }
  return (
    <a
      href={terrainApi.artifactUrl(jobId, artifact.name)}
      download={artifact.name}
      onClick={() => onWebDownload(artifact)}
    >
      <span>{artifact.name}</span>
      <small aria-live="polite">
        {feedbackLabel(artifact, feedback, webFallback)}
      </small>
    </a>
  );
}

/// One folder picker for a set of a job's files.
function SaveSetButton({
  artifacts,
  feedback,
  label,
  onSaveSet,
  saveKey,
}: {
  artifacts: Artifact[];
  feedback: ArtifactFeedback | null;
  label: string;
  onSaveSet: (key: string, names: string[]) => void;
  saveKey: string;
}) {
  const state = feedback?.name === saveKey ? feedback.state : null;
  const megabytes = artifacts.reduce((sum, a) => sum + a.bytes, 0) / 1024 / 1024;
  return (
    <button
      className="save-all"
      disabled={state === "saving"}
      onClick={() => onSaveSet(saveKey, artifacts.map((a) => a.name))}
      type="button"
    >
      <span>{label}</span>
      <small aria-live="polite">
        {state === "saving"
          ? "Choosing a folder…"
          : state === "saved"
            ? "Saved"
            : `${megabytes.toFixed(1)} MB`}
      </small>
    </button>
  );
}

/// The map data this job read, packed so the same model can be built again
/// with no network. Built on request: it is far larger than the print files,
/// and most jobs never need one.
function SourceBundleSection({
  bundle,
  building,
  feedback,
  isDesktop,
  job,
  onBuild,
  onSave,
  onWebDownload,
}: {
  bundle: SourceBundleSummary | null;
  building: boolean;
  feedback: ArtifactFeedback | null;
  isDesktop: boolean;
  job: Job;
  onBuild: () => void;
  onSave: (artifact: Artifact) => void;
  onWebDownload: (artifact: Artifact) => void;
}) {
  if (!bundle?.available || !bundle.name) return null;
  const megabytes = ((bundle.bytes ?? 0) / 1024 / 1024).toFixed(1);
  const built = typeof bundle.built_bytes === "number";
  const artifact: Artifact = {
    name: bundle.name,
    bytes: bundle.built_bytes ?? bundle.bytes ?? 0,
    media_type: "application/zip",
  };
  return (
    <details>
      <summary>Source data</summary>
      <div>
        <p className="control-hint">
          The {bundle.files} elevation, land-cover, and map files this model
          read, about {megabytes} MB. Import one on another machine to build
          the same model with no network, or keep it as an archive of the data
          the print came from.
        </p>
        {built ? (
          <ArtifactAction
            artifact={artifact}
            desktopFallback={`${(artifact.bytes / 1024 / 1024).toFixed(1)} MB`}
            feedback={feedback}
            isDesktop={isDesktop}
            jobId={job.id}
            onSave={onSave}
            onWebDownload={onWebDownload}
            webFallback={`${(artifact.bytes / 1024 / 1024).toFixed(1)} MB`}
          />
        ) : (
          <button
            className="save-all"
            disabled={building}
            onClick={onBuild}
            type="button"
          >
            <span>Pack the source data</span>
            <small aria-live="polite">
              {building ? "Packing…" : `About ${megabytes} MB`}
            </small>
          </button>
        )}
      </div>
    </details>
  );
}

export function ArtifactDownloads({
  job,
  bundle,
  building,
  feedback,
  isDesktop,
  onBuildBundle,
  onSave,
  onSaveSet,
  onWebDownload,
}: {
  job: Job;
  bundle: SourceBundleSummary | null;
  building: boolean;
  feedback: ArtifactFeedback | null;
  isDesktop: boolean;
  onBuildBundle: () => void;
  onSave: (artifact: Artifact) => void;
  onSaveSet: (key: string, names: string[]) => void;
  onWebDownload: (artifact: Artifact) => void;
}) {
  const printFiles = job.artifacts.filter(
    (artifact) =>
      artifact.name.endsWith(".3mf") || artifact.name === "manifest.json",
  );
  const stlFiles = job.artifacts.filter((artifact) =>
    artifact.name.endsWith(".stl"),
  );
  return (
    <div className="downloads">
      {isDesktop && printFiles.length > 1 && (
        <SaveSetButton
          artifacts={printFiles}
          feedback={feedback}
          label={`Save all ${printFiles.length} print files to a folder`}
          onSaveSet={onSaveSet}
          saveKey={SAVE_PRINT_KEY}
        />
      )}
      {printFiles.map((artifact) => {
        const size = `${(artifact.bytes / 1024 / 1024).toFixed(1)} MB`;
        return (
          <ArtifactAction
            artifact={artifact}
            desktopFallback={size}
            feedback={feedback}
            isDesktop={isDesktop}
            jobId={job.id}
            key={artifact.name}
            onSave={onSave}
            onWebDownload={onWebDownload}
            webFallback={size}
          />
        );
      })}
      {stlFiles.length > 0 && (
        <details>
          <summary>STL models</summary>
          <div>
            {isDesktop && stlFiles.length > 1 && (
              <SaveSetButton
                artifacts={stlFiles}
                feedback={feedback}
                label={`Save all ${stlFiles.length} STL files to a folder`}
                onSaveSet={onSaveSet}
                saveKey={SAVE_STL_KEY}
              />
            )}
            {stlFiles.map((artifact) => (
              <ArtifactAction
                artifact={artifact}
                desktopFallback="Save STL"
                feedback={feedback}
                isDesktop={isDesktop}
                jobId={job.id}
                key={artifact.name}
                onSave={onSave}
                onWebDownload={onWebDownload}
                webFallback="Download STL"
              />
            ))}
          </div>
        </details>
      )}
      <SourceBundleSection
        building={building}
        bundle={bundle}
        feedback={feedback}
        isDesktop={isDesktop}
        job={job}
        onBuild={onBuildBundle}
        onSave={onSave}
        onWebDownload={onWebDownload}
      />
    </div>
  );
}
