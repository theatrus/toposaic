# How the slicers read a third-party color 3MF

Findings from the Bambu Studio and OrcaSlicer sources, current as of July
2026, with the Bambu behavior verified live against Bambu Studio on macOS.
They decide what each `threemf_style` carries; see `carries_color_group` in
`crates/toposaic-core/src/export.rs`.

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

Bambu never applies a third-party file's embedded settings. Whichever way
the file arrives — File > Open, double-click, or drag-drop — it announces
"The 3mf is not from Bambu Lab, load geometry data and color data only" and
keeps the open project's filament list untouched. Verified live: opening a
file whose settings carry five filaments left a 32-filament session list
exactly as it was.

The one channel that conveys colors into Bambu is the color group. Bambu
collects the per-triangle `pid` references of any file it did not generate
and opens its "Standard 3mf Import color" dialog:

- **Color match** (pre-selected when the colors are close) maps the file's
  colors onto filaments already loaded and adds nothing. Verified live: a
  five-color file matched onto five existing slots, list length unchanged.
- **Append** adds one filament per file color — and each addition is a COPY
  OF THE LAST FILAMENT in the current list (`PresetBundle::
  set_num_filaments` resizes with `filament_presets.back()`). One TPU spool
  at the end of the list turns every appended color into Generic TPU, and
  the clones persist in the project, so repeated imports pile them up. A
  32-filament list holding the same five terrain colors over and over as
  Generic TPU is this loop's signature; delete the clones once and use
  Color match.

Without a group, the paint codes silently index into whatever filaments the
project already has — extruders 1..N, whatever their colors. That is
correct behavior for a deliberately pre-painted file and wrong for one that
means to state its own palette.

The dialog is also why the palette must be deduplicated: every color in the
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

- **Project** carries colors into both slicers, through different channels:
  in Bambu the group feeds the import dialog (the settings are dead weight
  there — Bambu refuses third-party configs); in Orca the settings set the
  filament list and the group is ignored.
- **Painted** is a plain pre-painted model in both slicers: triangles carry
  extruder assignments 1..N, colors come from the filaments already loaded,
  and no dialog opens and no preset changes.
- **Geometry** carries the one channel non-slicer 3MF consumers read, and
  nothing vendor-specific. The material namespace is declared as a REQUIRED
  extension, so it appears only in the styles that write the group.
