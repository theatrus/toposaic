import type { GenerationFailure, Job } from "./contracts";

export function describeJobFailure(job: Job | null): GenerationFailure | null {
  if (job?.status !== "failed") return null;
  if (job.failure) return job.failure;

  const technicalDetail = job.error?.trim() || "No technical detail was recorded.";
  const piece = parsePiece(technicalDetail);
  return {
    title: piece
      ? `Could not build puzzle piece ${piece.row},${piece.column}`
      : "Generation failed",
    message: piece
      ? "TopoSaic could not finish this piece. Open the technical details to see which geometry step failed."
      : "TopoSaic stopped before it could finish the model. Try again or include the technical details in a bug report.",
    technical_detail: technicalDetail,
    control_tab: piece ? "model" : undefined,
    piece,
  };
}

function parsePiece(error: string) {
  const match = error.match(/build piece\s+(\d+)\s*,\s*(\d+)/i);
  if (!match) return undefined;
  return { row: Number(match[1]), column: Number(match[2]) };
}
