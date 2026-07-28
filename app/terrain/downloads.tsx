import type { Artifact, ArtifactFeedback, Job } from "./contracts";
import { terrainApi } from "./api";

// The feedback slot the "save everything" button uses. Artifact names are
// file names, so this cannot collide with one.
export const SAVE_ALL_KEY = "*all*";

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

export function ArtifactDownloads({
  job,
  feedback,
  isDesktop,
  onSave,
  onSaveAll,
  onWebDownload,
}: {
  job: Job;
  feedback: ArtifactFeedback | null;
  isDesktop: boolean;
  onSave: (artifact: Artifact) => void;
  onSaveAll: () => void;
  onWebDownload: (artifact: Artifact) => void;
}) {
  const printFiles = job.artifacts.filter(
    (artifact) =>
      artifact.name.endsWith(".3mf") || artifact.name === "manifest.json",
  );
  const stlFiles = job.artifacts.filter((artifact) =>
    artifact.name.endsWith(".stl"),
  );
  const saveAllState =
    feedback?.name === SAVE_ALL_KEY ? feedback.state : null;
  const totalBytes = job.artifacts.reduce(
    (sum, artifact) => sum + artifact.bytes,
    0,
  );
  return (
    <div className="downloads">
      {isDesktop && job.artifacts.length > 1 && (
        <button
          className="save-all"
          disabled={saveAllState === "saving"}
          onClick={onSaveAll}
          type="button"
        >
          <span>
            Save all {job.artifacts.length} files to a folder
          </span>
          <small aria-live="polite">
            {saveAllState === "saving"
              ? "Choosing a folder…"
              : saveAllState === "saved"
                ? "Saved"
                : `${(totalBytes / 1024 / 1024).toFixed(1)} MB`}
          </small>
        </button>
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
