# TopoSaic architecture

TopoSaic has one terrain engine, one local API, and two React entry points. The
web and desktop apps share the same studio UI.

## Dependency flow

```text
web entry ─────┐
               v
desktop entry -> app/terrain/studio.tsx -> local HTTP API
                                              |
src-tauri ------------------------------------+
                                              v
                                  crates/terrain-core
```

- `crates/terrain-core` owns print geometry and file formats. It has no network,
  database, UI, or Tauri code.
- `apps/api` owns data downloads, the cache, SQLite jobs, and HTTP routes. It
  turns remote map data into inputs for `terrain-core`.
- `app/terrain` owns the shared studio, map, preview, downloads, API client,
  browser-side contracts, and quality rules.
- `app/updates` owns release notices, version checks, and desktop updates.
- `desktop` starts the shared React UI for Vite.
- `src-tauri` starts the local API and wraps the desktop build.

## Start here

- Change the main controls in `app/terrain/studio.tsx`.
- Change the interactive map in `app/terrain/map.tsx`.
- Change the 3D view in `app/terrain/preview.tsx`.
- Change browser-to-service calls in `app/terrain/api.ts`.
- Change API startup and routes in `apps/api/src/lib.rs`.
- Change job storage in `apps/api/src/database.rs`.
- Change place search in `apps/api/src/geocoding.rs`.
- Change downloaded elevation or surface data in `apps/api/src/elevation.rs`
  or `apps/api/src/surface.rs`.
- Change generated mesh or 3MF output in `crates/terrain-core/src/lib.rs`.

## Folder rules

| Change | Put it here |
| --- | --- |
| Puzzle, tray, mesh, or 3MF logic | `crates/terrain-core` |
| Elevation, OpenStreetMap, cache, job, or download logic | `apps/api` |
| Terrain request and response types used by React | `app/terrain/contracts.ts` |
| UI defaults and quality calculations | `app/terrain/config.ts` |
| Calls from React to the local API | `app/terrain/api.ts` |
| Map projection and super-tile coordinate rules | `app/terrain/geo.ts` |
| Interactive map picker | `app/terrain/map.tsx` |
| 3D terrain preview | `app/terrain/preview.tsx` |
| Artifact download controls | `app/terrain/downloads.tsx` |
| Shared studio controls | `app/terrain/studio.tsx` |
| Update checks and install flow | `app/updates` |
| SQLite schema and job storage | `apps/api/src/database.rs` |
| Place search and its cache | `apps/api/src/geocoding.rs` |
| Runtime settings | `apps/api/src/settings.rs` |
| Web-only setup | `app/layout.tsx` or the root web build files |
| Desktop-only setup | `desktop` or `src-tauri` |
| Release feed tools | `scripts` |
| Product and design notes | `docs` |

Keep `app`, `desktop`, and `src-tauri` at the repository root. Next/Vinext,
Vite, and Tauri use those paths in their build files. Moving them would add
configuration without making ownership clearer.

## Shared rules

- Use `apps/api/src/geo.rs` for latitude, longitude, bounds, and date-line math.
- Use `app/terrain/geo.ts` for the matching browser-side tile placement rules.
- Use `apps/api/src/http.rs` for outbound HTTP clients and local API origin
  checks.
- Use `app/terrain/api.ts` for API paths, JSON parsing, and service errors.
- Validate each `GenerationSpec` at the API edge before fetching data or making
  geometry.
- Keep downloaded source data in the OS cache path. Keep job output and SQLite
  state in the app data path.
- Add small unit tests beside pure rules. Use the UI tests for user flows and
  the workspace tests for mesh and API behavior.

## Current large files

`crates/terrain-core/src/lib.rs`, `app/terrain/studio.tsx`, and
`app/globals.css` remain large. Split them by stable behavior, not by line
count. Good next seams are mesh output, tray geometry, groups of studio
controls, and matching style groups. Keep each split in a focused pull request
with mesh, browser, or visual checks that fit the change.
