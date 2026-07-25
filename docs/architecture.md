# TopoSaic architecture

TopoSaic has one terrain engine, one local API, and two React entry points. The
web and desktop apps share the same studio UI.

## Dependency flow

```text
app/terrain-studio.tsx
        |
        v
apps/api  --->  crates/terrain-core
        ^
        |
src-tauri
```

- `crates/terrain-core` owns print geometry and file formats. It has no network,
  database, UI, or Tauri code.
- `apps/api` owns data downloads, the cache, SQLite jobs, and HTTP routes. It
  turns remote map data into inputs for `terrain-core`.
- `app` owns the shared React UI. `app/terrain` holds the UI-side terrain
  contract and quality rules.
- `desktop` starts the shared React UI for Vite.
- `src-tauri` starts the local API and wraps the desktop build.

## Folder rules

| Change | Put it here |
| --- | --- |
| Puzzle, tray, mesh, or 3MF logic | `crates/terrain-core` |
| Elevation, OpenStreetMap, cache, job, or download logic | `apps/api` |
| Terrain request and response types used by React | `app/terrain/contracts.ts` |
| UI defaults and quality calculations | `app/terrain/config.ts` |
| Map projection and super-tile coordinate rules | `app/terrain/geo.ts` |
| Interactive map picker | `app/terrain/map.tsx` |
| Shared studio controls, map, and preview | `app` |
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
- Use `apps/api/src/http.rs` for outbound HTTP clients so every provider gets
  the current version, repository URL, and timeout policy.
- Validate each `GenerationSpec` at the API edge before fetching data or making
  geometry.
- Keep downloaded source data in the OS cache path. Keep job output and SQLite
  state in the app data path.
- Add small unit tests beside pure rules. Use the UI tests for user flows and
  the workspace tests for mesh and API behavior.

## Current large files

`crates/terrain-core/src/lib.rs`, `app/terrain-studio.tsx`, and
`app/globals.css` remain large. Split them by stable behavior, not by line
count. Good next seams are mesh output, tray geometry, the map picker, and the
3D preview. Avoid moving all three at once: each has broad regression risk and
needs its own focused pull request.
