import type { PreviewData } from "./contracts";

// Old saved previews used 13 fixed classes. New previews explicitly report
// where their discovered ground colors begin so added classes cannot shift them.
export function previewGroundColor(preview: PreviewData | null | undefined, material?: number) {
  if (material === undefined) return undefined;
  const offset = preview?.ground_material_offset ?? 13;
  if (material < offset) return undefined;
  return preview?.ground_palette?.[material - offset];
}
