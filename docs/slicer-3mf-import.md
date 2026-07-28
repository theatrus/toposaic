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

Bambu does not apply a third-party file's embedded settings. Verified live
through File > Open: a file whose settings carry five filaments left a
32-filament session list exactly as it was, and Bambu announced "The 3mf is
not from Bambu Lab, load geometry data and color data only". Drag-drop and
double-click were not tested and take their own path through
`Plater.cpp`'s import-action prompt; treat them as unverified.

The one channel that conveys colors into Bambu is the color group. Bambu
collects the per-triangle `pid` references of any file it did not generate
and opens its "Standard 3mf Import color" dialog. Without a group, the paint
codes silently index into whatever filaments the project already has —
extruders 1..N, whatever their colors, which is right for a deliberately
pre-painted file and wrong for one that means to state its own palette.

#### The import dialog, and why it multiplies filaments

The dialog offers two quick sets:

- **Color match** maps the file's colors onto filaments already loaded and
  adds nothing. Verified live: a five-color file matched onto five existing
  slots, list length unchanged.
- **Append** adds one filament per file color.

Its DEFAULT is append, not match. `ObjColorPanel::deal_default_strategy`
runs `deal_add_btn` first, staging one new filament per file color and
pointing every mapping row at the staged copies; the nearest-color match
runs after, with those copies as zero-distance candidates, so a row without
a clearly closer existing filament stays on its copy. OK commits every row
pointing past the current list as a new filament — so accepting the defaults
appends the whole palette without anyone clicking Append. Only a list at the
32-filament cap skips the staging, forcing a pure match.

Two things then compound it:

- Each appended filament is a COPY OF THE LAST ONE in the list
  (`PresetBundle::set_num_filaments` resizes with `filament_presets.back()`),
  so a trailing TPU spool makes every appended color Generic TPU.
- The filament list is APP state, not project state. It persists across
  sessions, so the copies appear in every later import dialog until deleted
  by hand.

A long list holding the same few terrain colors over and over as Generic TPU
is this loop's signature; delete the copies once and use Color match. It is
also why the palette must be deduplicated: every color in the group is
offered as a potential new filament.

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
