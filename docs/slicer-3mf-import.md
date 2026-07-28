# How the slicers read a third-party color 3MF

Findings from the Bambu Studio and OrcaSlicer sources, current as of July
2026. They decide what each `threemf_style` carries; see
`carries_color_group` in `crates/toposaic-core/src/export.rs`.

## The three color channels

A 3MF can state its colors three ways, and the slicers read them
differently:

1. **Core-spec color group** — `<m:colorgroup>` plus per-triangle
   `pid`/`p1`/`p2`/`p3` references. The standard channel.
2. **Face-paint codes** — the `paint_color` triangle attribute, a
   PrusaSlicer-lineage encoding of "this face prints on extruder n".
   Vendor-specific, shared by Bambu Studio and OrcaSlicer.
3. **Embedded project settings** — `Metadata/project_settings.config`, a
   JSON config with `filament_colour`, `filament_settings_id`, and flush
   volumes.

## Who reads what

Both slicers classify a file by its `Application` metadata. Bambu Studio
treats only `BambuStudio-*` as its own; OrcaSlicer accepts `BambuStudio-*`,
`OrcaSlicer-*`, and its own `OrcaSlicer` version tag. Everything else — us —
is a third-party file.

### Bambu Studio (`src/slic3r/GUI/Plater.cpp`, `Format/bbs_3mf.cpp`)

Bambu has two import flows for a third-party file, and they read different
channels:

- **Open as project** (double-click, File > Open, drag-drop → "Open as
  project"): the embedded settings load into the preset bundle, so the
  filament list becomes exactly the file's `filament_colour` array and each
  slot selects the preset `filament_settings_id` names. The paint codes then
  bind each triangle to those slots.
- **Import geometry** (drag-drop → "Import geometry only", the usual way to
  add a model to a plate in progress): embedded settings are skipped BY
  DESIGN — `load_config` is off, and nothing from the file may touch the
  open project's config. In this flow the only channel that conveys colors
  is the color group: Bambu collects the per-triangle `pid` references of
  any file it did not generate and opens its "Standard 3mf Import color"
  dialog, where Color match maps the file's colors onto the loaded filaments
  and Append adds new ones. Without a group, the paint codes silently index
  into whatever filaments the project already has.

So a file that wants its colors to survive both flows must carry the
settings AND the group — which is what the Project style does. The dialog's
Append is also why the palette must be deduplicated: every color in the
group is offered as a potential new filament.

### OrcaSlicer (`src/slic3r/GUI/Plater.cpp`, `Format/bbs_3mf.cpp`)

- Loads a third-party file's embedded settings silently when they exist
  (`From_Other` with a non-empty config) — filament colors just apply.
- Reads the paint codes unconditionally.
- Has no import-color dialog; the Bambu code for it does not exist in the
  fork. The color group is read into one color per group (each `<m:color>`
  overwrites the last) and used only to map an **object**-level `pid` to an
  extruder. Our references are per-triangle, so Orca ignores them entirely.

### Filament presets

`filament_settings_id` names must match a preset the slicer ships or the
import falls back to guessing a material — an empty id is how terrain colors
once imported as Generic TPU. Both slicers ship `Generic PLA`, `Bambu PLA
Basic`, `PolyLite PLA`, and `PolyTerra PLA` (Bambu under `resources/
profiles/BBL/filament`, Orca in its filament library). Their real preset
names carry a printer suffix (`PolyTerra PLA @BBL A1`) the file cannot know;
the base name still resolves the vendor and material.

## What each style carries

| Style | Color group | Paint codes | Embedded settings |
|---|---|---|---|
| Color project (for Bambu) | yes | yes | yes |
| Painted colors (for Orca) | — | yes | — |
| Geometry only | yes | — | — |

- **Project** survives both Bambu flows: settings for open-as-project, the
  group for import-geometry. In Orca the settings apply and the group is
  ignored.
- **Painted** is a plain pre-painted model in both slicers: triangles carry
  extruder assignments 1..N, colors come from the filaments already loaded,
  and no dialog opens and no preset changes.
- **Geometry** carries the one channel non-slicer 3MF consumers read, and
  nothing vendor-specific. The material namespace is declared as a REQUIRED
  extension, so it appears only in the styles that write the group.
