import type { GenerationSpec } from "../contracts";

export type UpdateGenerationSpec = <Key extends keyof GenerationSpec>(
  key: Key,
  value: GenerationSpec[Key],
) => void;

export type UpdateTray = <Key extends keyof GenerationSpec["tray"]>(
  key: Key,
  value: GenerationSpec["tray"][Key],
) => void;

export type UpdatePuzzleRetention = <
  Key extends keyof GenerationSpec["puzzle_retention"],
>(
  key: Key,
  value: GenerationSpec["puzzle_retention"][Key],
) => void;

export type UpdateWallMount = <Key extends keyof GenerationSpec["wall_mount"]>(
  key: Key,
  value: GenerationSpec["wall_mount"][Key],
) => void;
