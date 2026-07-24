type ParsedVersion = {
  core: [number, number, number];
  prerelease: string[] | null;
};

function parseVersion(value: string): ParsedVersion | null {
  const match = value
    .trim()
    .match(
      /^v?(\d+)\.(\d+)\.(\d+)(?:-([0-9A-Za-z.-]+))?(?:\+[0-9A-Za-z.-]+)?$/,
    );
  if (!match) return null;
  return {
    core: [Number(match[1]), Number(match[2]), Number(match[3])],
    prerelease: match[4]?.split(".") ?? null,
  };
}

function comparePrerelease(
  candidate: string[] | null,
  current: string[] | null,
): number {
  if (candidate === null && current === null) return 0;
  if (candidate === null) return 1;
  if (current === null) return -1;

  for (
    let index = 0;
    index < Math.max(candidate.length, current.length);
    index += 1
  ) {
    const candidatePart = candidate[index];
    const currentPart = current[index];
    if (candidatePart === undefined) return -1;
    if (currentPart === undefined) return 1;
    if (candidatePart === currentPart) continue;

    const candidateNumber = /^\d+$/.test(candidatePart)
      ? Number(candidatePart)
      : null;
    const currentNumber = /^\d+$/.test(currentPart) ? Number(currentPart) : null;
    if (candidateNumber !== null && currentNumber !== null) {
      return candidateNumber > currentNumber ? 1 : -1;
    }
    if (candidateNumber !== null) return -1;
    if (currentNumber !== null) return 1;
    return candidatePart > currentPart ? 1 : -1;
  }
  return 0;
}

export function isVersionNewer(candidate: string, current: string): boolean {
  const parsedCandidate = parseVersion(candidate);
  const parsedCurrent = parseVersion(current);
  if (!parsedCandidate || !parsedCurrent) return false;

  for (let index = 0; index < parsedCandidate.core.length; index += 1) {
    if (parsedCandidate.core[index] === parsedCurrent.core[index]) continue;
    return parsedCandidate.core[index] > parsedCurrent.core[index];
  }
  return (
    comparePrerelease(parsedCandidate.prerelease, parsedCurrent.prerelease) > 0
  );
}

export function displayVersion(version: string): string {
  return `v${version.trim().replace(/^v/, "")}`;
}
