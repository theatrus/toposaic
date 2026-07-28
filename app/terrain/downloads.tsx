import type { Artifact, ArtifactFeedback, Job } from "./contracts";
import { terrainApi } from "./api";

// Feedback slots for the two folder saves. Artifact names are file names,
// so neither can collide with one.
export const SAVE_PRINT_KEY = "*print*";
export const SAVE_STL_KEY = "*stl*";

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

export function ArtifactDownloads({
  job,
  feedback,
  isDesktop,
  onSave,
  onSaveSet,
  onWebDownload,
}: {
  job: Job;
  feedback: ArtifactFeedback | null;
  isDesktop: boolean;
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
    </div>
  );
}
