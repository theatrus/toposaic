import type { GenerationSpec } from "./contracts";

// A spec written so two of them can be compared for sameness. JSON key
// order follows insertion, and two specs that mean the same thing can be
// built in different orders — a recalled setup is merged through
// mergeSpecDefaults, a live one is edited field by field — so the keys are
// sorted before comparing.
function canonicalSpec(value: unknown): unknown {
  if (Array.isArray(value)) {
    return value.map(canonicalSpec);
  }
  if (value && typeof value === "object") {
    return Object.fromEntries(
      Object.entries(value as Record<string, unknown>)
        .sort(([left], [right]) => (left < right ? -1 : left > right ? 1 : 0))
        .map(([key, entry]) => [key, canonicalSpec(entry)]),
    );
  }
  return value;
}

// Whether the model has moved away from the setup it was recalled from.
// Used to offer a save only when there is something to save.
export function specHasDrifted(
  current: GenerationSpec,
  saved: GenerationSpec | undefined,
): boolean {
  if (!saved) return false;
  return (
    JSON.stringify(canonicalSpec(current)) !==
    JSON.stringify(canonicalSpec(saved))
  );
}
