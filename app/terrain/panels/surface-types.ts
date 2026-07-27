import type { GenerationSpec } from "../contracts";

export type UpdateColor = <Key extends keyof GenerationSpec["color_output"]>(
  key: Key,
  value: GenerationSpec["color_output"][Key],
) => void;
