# Terrain Puzzle

Terrain Puzzle is a local-first topographic puzzle generator. A Rust service
samples worldwide elevation data, builds watertight interlocking pieces, and
stores job state in SQLite. The web app lets you choose a place and tune the
printable model.

The elevation provider reads Mapzen Terrarium tiles from the AWS Open Data
registry and keeps a local tile cache under `data/dem-cache`.

## Requirements

- Rust 1.96 or newer
- Node.js 22.13 or newer

## Run

Start the Rust API:

```bash
cargo run -p terrain-api
```

In a second terminal, start the website:

```bash
npm install
npm run dev
```

Open `http://127.0.0.1:3100`. The Rust API listens on
`http://127.0.0.1:8787`.

## Storage

SQLite and generated jobs live under `data/`, which Git ignores. Set
`TERRAIN_DATA_DIR` to use another directory.

The browser uses `NEXT_PUBLIC_TERRAIN_API_URL` when set. See `.env.example`.

## Check

```bash
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
npm test
```

## Project shape

- `crates/terrain-core`: puzzle edges, terrain surface, watertight meshes,
  binary STL, and standards-based 3MF
- `apps/api`: global elevation provider, Axum API, SQLite jobs, background
  generation, and downloads
- `app`: map, relief preview, print controls, and job downloads

## Terrain data

Mapzen Terrain Tiles combine several regional and global public elevation
sources. Generated manifests record the source and link to the required
attribution notices:

<https://github.com/tilezen/joerd/blob/master/docs/attribution.md>
