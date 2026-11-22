import type { GenerationSpec, GlobeSpec } from "../contracts";

/** Latitude spans offered for the 360° band. Each wraps the middle strip of
 * the map, as tall as this share of a full turn. */
export const BAND_DEGREES = [144, 120, 90, 60] as const;

/** Common heat-set inserts: thread, hole diameter, and length in mm. */
const INSERT_PRESETS = [
  ["M2", 2, 3.2, 3],
  ["M2.5", 2.5, 3.6, 3.5],
  ["M3", 3, 4, 4],
  ["M4", 4, 5.6, 5],
] as const;

export function ModelTypePanel({
  hidden,
  globeLayoutSize,
  globeBatchLimit,
  onModelTypeChange,
  setPieceLayout,
  spec,
  update,
}: {
  hidden: boolean;
  globeLayoutSize?: number;
  globeBatchLimit?: number;
  onModelTypeChange: (type: "puzzle" | "solid" | "globe") => void;
  setPieceLayout: (count: number) => void;
  spec: GenerationSpec;
  update: <Key extends keyof GenerationSpec>(
    key: Key,
    value: GenerationSpec[Key],
  ) => void;
}) {
  return (
    <>
      <div
        className="model-mode"
        role="group"
        aria-label="Model type"
        hidden={hidden}
      >
        <strong className="model-mode-label">Model type</strong>
        <button
          type="button"
          className={!spec.globe && !spec.solid_model ? "active" : ""}
          aria-pressed={!spec.globe && !spec.solid_model}
          onClick={() => onModelTypeChange("puzzle")}
        >
          <span className="mode-mark puzzle-mark" aria-hidden="true">
            <i />
            <i />
            <i />
            <i />
          </span>
          <span>
            <strong>Jigsaw puzzle</strong>
            <small>
              {spec.puzzle_tabs
                ? "Separate interlocking pieces"
                : "Separate pieces with plain cuts"}
            </small>
          </span>
        </button>
        <button
          type="button"
          className={!spec.globe && spec.solid_model ? "active" : ""}
          aria-pressed={!spec.globe && spec.solid_model}
          onClick={() => onModelTypeChange("solid")}
        >
          <span className="mode-mark solid-mark" aria-hidden="true" />
          <span>
            <strong>Solid terrain</strong>
            <small>One watertight model, no cuts</small>
          </span>
        </button>
        <button
          type="button"
          className={spec.globe ? "active" : ""}
          aria-pressed={Boolean(spec.globe)}
          onClick={() => onModelTypeChange("globe")}
        >
          <span className="mode-mark globe-mark" aria-hidden="true" />
          <span>
            <strong>Globe</strong>
            <small>Snap-on tiles and backing frame</small>
          </span>
        </button>
      </div>

      {spec.globe && (
        <GlobeControls
          hidden={hidden}
          globe={spec.globe}
          colors={spec.color_output}
          layoutSize={globeLayoutSize}
          batchLimit={globeBatchLimit}
          update={(globe) => update("globe", globe)}
          updateColors={(colors) => update("color_output", colors)}
        />
      )}

      {!spec.globe && !spec.solid_model && (
        <fieldset className="piece-grid" hidden={hidden}>
          <legend>Piece layout</legend>
          {[4, 6, 8, 10, 12].map((count) => (
            <button
              type="button"
              className={
                spec.rows === count && spec.columns === count
                  ? "active"
                  : ""
              }
              key={count}
              onClick={() => setPieceLayout(count)}
            >
              <span
                className="mini-grid"
                style={{
                  gridTemplateColumns: `repeat(${count}, 1fr)`,
                }}
              >
                {Array.from({ length: count * count }).map((_, index) => (
                  <i key={index} />
                ))}
              </span>
              <span>
                {count}×{count}
              </span>
              <small>{count * count} pieces</small>
            </button>
          ))}
          <div className="piece-custom">
            <label>
              Columns
              <select
                value={spec.columns}
                onChange={(event) =>
                  update("columns", Number(event.target.value))
                }
              >
                {Array.from({ length: 15 }, (_, index) => index + 2).map(
                  (count) => (
                    <option key={count} value={count}>
                      {count}
                    </option>
                  ),
                )}
              </select>
            </label>
            <label>
              Rows
              <select
                value={spec.rows}
                onChange={(event) =>
                  update("rows", Number(event.target.value))
                }
              >
                {Array.from({ length: 15 }, (_, index) => index + 2).map(
                  (count) => (
                    <option key={count} value={count}>
                      {count}
                    </option>
                  ),
                )}
              </select>
            </label>
            <div>
              <strong>{spec.rows * spec.columns} pieces</strong>
              <small>
                About {(spec.width_mm / spec.columns).toFixed(1)} mm wide
                each
              </small>
            </div>
          </div>
          <div
            className="piece-shape-options"
            role="group"
            aria-label="Piece shape"
          >
            <label>
              <input
                type="checkbox"
                checked={spec.straight_piece_sides}
                onChange={(event) =>
                  update("straight_piece_sides", event.target.checked)
                }
              />
              <span>
                <strong>Straight piece sides</strong>
                <small>Align each cut instead of warping the grid</small>
              </span>
            </label>
            <label>
              <input
                type="checkbox"
                checked={spec.puzzle_tabs}
                onChange={(event) =>
                  update("puzzle_tabs", event.target.checked)
                }
              />
              <span>
                <strong>Interlocking tabs</strong>
                <small>Turn off for tab-less pieces with plain cuts</small>
              </span>
            </label>
          </div>
          {spec.width_mm / spec.columns < 10 && (
            <p className="piece-warning">
              These pieces are under 10 mm wide. Increase print width for
              stronger pieces and easier handling.
            </p>
          )}
        </fieldset>
      )}
    </>
  );
}

function GlobeControls({ hidden, globe, colors, layoutSize, batchLimit, update, updateColors }: {
  hidden: boolean;
  globe: GlobeSpec;
  colors: GenerationSpec["color_output"];
  layoutSize?: number;
  batchLimit?: number;
  update: (globe: GlobeSpec) => void;
  updateColors: (colors: GenerationSpec["color_output"]) => void;
}) {
  const mapped = globe.source !== "fit_test";
  const printLayout = globe.batch ? "batch" : "full";
  const maxBatch = batchLimit ?? 80;
  const batch = globe.batch;
  const setLayout = (layout: string) => update({
    ...globe,
    full_shell: true,
    batch: layout === "batch" ? batch ?? { first_tile: 1, tile_count: Math.min(8, maxBatch) } : null,
    whole_earth_detail: layout === "batch" && globe.whole_earth_detail,
  });
  const insert = globe.insert ?? { screw_mm: 3, hole_diameter_mm: 4, length_mm: 4 };
  const insertRange = (key: "hole_diameter_mm" | "length_mm", label: string, min: number, max: number) => (
    <label className="road-detail-field">
      {label} · {insert[key].toFixed(1)} mm
      <input aria-label={label} type="range" min={min} max={max} step={0.1} value={insert[key]} onChange={(event) => update({ ...globe, insert: { ...insert, [key]: event.target.valueAsNumber } })} />
    </label>
  );
  const range = (key: "seam_gap_mm" | "wrap_degrees" | "relief_mm" | "texture_mm" | "diameter_mm" | "bed_mm" | "clip_fit_mm" | "locator_clearance_mm" | "frame_depth_mm" | "key_clearance_mm", label: string, min: number, max: number, step: number, decimals = 0, unit = " mm") => (
    <label className="road-detail-field">
      {label} · {globe[key].toFixed(decimals)}{unit}
      <input aria-label={label} type="range" min={min} max={max} step={step} value={globe[key]} onChange={(event) => update({ ...globe, [key]: event.target.valueAsNumber })} />
    </label>
  );
  return (
    <fieldset className="control-section globe-controls" hidden={hidden}>
      <legend>Globe settings</legend>
      <label className="road-detail-field">
        Globe source
        <select aria-label="Globe source" value={globe.source} onChange={(event) => {
          const source = event.target.value as GlobeSpec["source"];
          const withTerrain = source !== "fit_test";
          update({ ...globe, source, full_shell: true,
            whole_earth_detail: source === "whole_earth" && globe.whole_earth_detail,
            samples_per_edge: globe.source !== "whole_earth" && source === "whole_earth" ? 96 : globe.samples_per_edge,
            surface_colors: globe.source === "fit_test" && withTerrain ? true : globe.surface_colors });
        }}>
          <option value="fit_test">Plain shell</option>
          <option value="small_world">Small World</option>
          <option value="whole_earth">Whole Earth</option>
        </select>
      </label>
      <label className="road-detail-field">
        Globe exterior
        <select aria-label="Globe exterior" value={globe.exterior} onChange={(event) => update({ ...globe, exterior: event.target.value === "rounded" ? "rounded" : "faceted" })}>
          <option value="faceted">Faceted</option>
          <option value="rounded">Rounded</option>
        </select>
      </label>
      <label className="road-detail-field">
        Print layout
        <select aria-label="Print layout" value={printLayout} onChange={(event) => setLayout(event.target.value)}>
          <option value="full">Full globe</option>
          <option value="batch">Numbered tile batch</option>
        </select>
      </label>
      <label className="road-detail-field">
        Globe diameter (mm)
        <input aria-label="Globe diameter" type="number" min="160" max="12000" key={globe.diameter_mm} defaultValue={globe.diameter_mm} onBlur={(event) => {
          const diameter = event.target.valueAsNumber;
          if (diameter >= 160 && diameter <= 12000) update({ ...globe, diameter_mm: diameter });
          else event.target.value = String(globe.diameter_mm);
        }} onKeyDown={(event) => {
          if (event.key === "Enter") {
            event.preventDefault();
            event.currentTarget.blur();
          }
        }} />
      </label>
      {range("bed_mm", "Usable bed (shorter side)", 100, 420, 5)}
      <label className="road-detail-field">
        Globe subdivision
        <select aria-label="Globe subdivision" value={globe.subdivision} onChange={(event) => update({ ...globe, subdivision: Number(event.target.value) })}>
          <option value={0}>Automatic for print bed</option>
          {Array.from({ length: 24 }, (_, index) => index + 1).map((frequency) => (
            <option key={frequency} value={frequency}>{frequency} · {(20 * frequency ** 2).toLocaleString()} tiles</option>
          ))}
        </select>
      </label>
      {batch && (
        <>
          <label className="road-detail-field">
            First tile
            <input aria-label="First tile" type="number" min="1" max="11520" value={batch.first_tile} onChange={(event) => {
              if (Number.isInteger(event.target.valueAsNumber) && event.target.valueAsNumber > 0) update({ ...globe, batch: { ...batch, first_tile: event.target.valueAsNumber } });
            }} />
          </label>
          <label className="road-detail-field">
            Tiles in batch
            <input aria-label="Tiles in batch" type="number" min="1" max={maxBatch} value={batch.tile_count} onChange={(event) => {
              if (Number.isInteger(event.target.valueAsNumber) && event.target.valueAsNumber >= 1 && event.target.valueAsNumber <= maxBatch) update({ ...globe, batch: { ...batch, tile_count: event.target.valueAsNumber } });
            }} />
            <small>Up to {maxBatch} tiles per batch at this mesh detail. Each kit includes matching frame modules and the joints owned by its tiles.</small>
          </label>
          <div className="adjacent-actions globe-batch-actions" role="group" aria-label="Change globe batch">
            <button type="button" disabled={batch.first_tile === 1} onClick={() => update({ ...globe, batch: { ...batch, first_tile: Math.max(1, batch.first_tile - batch.tile_count) } })}>Previous batch</button>
            <button type="button" disabled={!layoutSize || batch.first_tile + batch.tile_count > layoutSize} onClick={() => update({ ...globe, batch: { ...batch, first_tile: batch.first_tile + batch.tile_count } })}>Next batch</button>
          </div>
        </>
      )}
      {range("seam_gap_mm", "Seam clearance", 0.2, 1.2, 0.05, 2)}
      {globe.source === "small_world" && (
        <>
          <label className="road-detail-field">
            Map coverage
            <select aria-label="Map coverage" value={globe.full_wrap ? "full" : "regional"} onChange={(event) => update({ ...globe, full_wrap: event.target.value === "full" })}>
              <option value="regional">Regional with smooth back</option>
              <option value="full">360° band with polar caps</option>
            </select>
            {globe.full_wrap && <small>The map wraps once around the globe. Its middle strip, outlined on the map, forms the band. The east and west edges blend where they meet, and the band fades into plain polar caps.</small>}
          </label>
          {globe.full_wrap ? (
            <label className="road-detail-field">
              Band height
              <select aria-label="Band height" value={globe.band_degrees ?? 120} onChange={(event) => update({ ...globe, band_degrees: Number(event.target.value) })}>
                {BAND_DEGREES.map((degrees) => (
                  <option key={degrees} value={degrees}>±{degrees / 2}° latitude</option>
                ))}
              </select>
            </label>
          ) : range("wrap_degrees", "Regional wrap", 45, 140, 5, 0, "°")}
          <p className="globe-map-note">Roads, buildings, water, markers, and colors come from the Surface, Buildings, Markers, and Colors sections, as on a flat model.</p>
        </>
      )}
      {mapped && (
        <>
          {globe.source === "whole_earth" && (
            <label className="road-detail-field">
              Bundled Earth map
              <select aria-label="Bundled Earth map" value={globe.earth_grid} onChange={(event) => update({ ...globe, earth_grid: event.target.value === "quarter_degree" ? "quarter_degree" : "half_degree" })}>
                <option value="quarter_degree">Fine · 15 arc-minute grid</option>
                <option value="half_degree">Original · 30 arc-minute grid</option>
              </select>
            </label>
          )}
          <label className="road-detail-field">
            Terrain mesh detail
            <select aria-label="Terrain mesh detail" value={globe.samples_per_edge} onChange={(event) => update({ ...globe, samples_per_edge: Number(event.target.value) })}>
              {[16, 32, 48, 64, 96].map((value) => <option key={value} value={value}>{value} samples per edge</option>)}
            </select>
          </label>
          {globe.source === "whole_earth" && batch && (
            <label className="option-toggle feature-enable-toggle globe-color-toggle">
              <input type="checkbox" checked={globe.whole_earth_detail} onChange={(event) => update({ ...globe, whole_earth_detail: event.target.checked })} />
              <span><strong>Detailed Earth elevation</strong><small>Download the 5 arc-minute elevation blocks used by this batch.</small></span>
            </label>
          )}
          {range("relief_mm", "Terrain relief", 0, 12, 0.5, 1)}
          {globe.source === "whole_earth" && range("texture_mm", "Tactile texture", 0, 0.9, 0.05, 2)}
          {globe.source === "whole_earth" && (
            <label className="option-toggle feature-enable-toggle globe-color-toggle">
              <input type="checkbox" checked={globe.surface_colors} onChange={(event) => update({ ...globe, surface_colors: event.target.checked })} />
              <span><strong>Mapped surface colors</strong><small>Paint the tiles in 3MF with a shared filament palette.</small></span>
            </label>
          )}
        </>
      )}
      <details className="globe-detail">
        <summary>Frame and snap fit</summary>
        <div className="globe-detail-fields">
          {range("frame_depth_mm", "Frame depth", 4.5, 12, 0.5, 1)}
          {range("clip_fit_mm", "Clip interference", 0.15, 0.6, 0.05, 2)}
          {range("locator_clearance_mm", "Locator clearance per side", 0.1, 0.6, 0.05, 2)}
          {range("key_clearance_mm", "Dovetail key clearance per side", 0.05, 0.4, 0.05, 2)}
          <p className="globe-map-note">Dovetail keys lock neighbouring frame modules together. Each key drops into slots in both modules and slides along their shared edge.</p>
          <label className="option-toggle feature-enable-toggle globe-color-toggle">
            <input type="checkbox" checked={globe.screw_brackets ?? false} onChange={(event) => update({ ...globe, screw_brackets: event.target.checked })} />
            <span><strong>Screw brackets</strong><small>Add a bracket under each joint, screwed down from outside into heat-set inserts.</small></span>
          </label>
          <label className="option-toggle feature-enable-toggle globe-color-toggle">
            <input type="checkbox" checked={globe.tile_screws ?? false} onChange={(event) => update({ ...globe, tile_screws: event.target.checked })} />
            <span><strong>Tile screws</strong><small>Screw each tile to its frame from inside, into a heat-set insert in the tile back. The last few tiles rely on their clips.</small></span>
          </label>
          {(globe.screw_brackets || globe.tile_screws) && (
            <>
              <label className="road-detail-field">
                Heat-set insert
                <select aria-label="Heat-set insert" value={insert.screw_mm} onChange={(event) => {
                  const preset = INSERT_PRESETS.find(([, screw]) => screw === Number(event.target.value));
                  if (preset) update({ ...globe, insert: { screw_mm: preset[1], hole_diameter_mm: preset[2], length_mm: preset[3] } });
                }}>
                  {INSERT_PRESETS.map(([label, screw]) => <option key={label} value={screw}>{label}</option>)}
                </select>
              </label>
              {insertRange("hole_diameter_mm", "Insert hole diameter", insert.screw_mm + 0.4, insert.screw_mm + 3.5)}
              {insertRange("length_mm", "Insert length", 2.5, 6.5)}
            </>
          )}
        </div>
      </details>
      {globe.source === "whole_earth" && globe.surface_colors && (
        <details className="globe-detail">
          <summary>Filament colors</summary>
          <div className="globe-detail-fields">
            {([["Rock and bare ground", "rock_color"], ["Forest", "forest_color"], ["Snow and ice", "snow_color"], ["Water", "water_color"]] as const).map(([label, key]) => (
              <label className="road-detail-field" key={key}>{label}<input type="color" value={colors[key]} onChange={(event) => updateColors({ ...colors, [key]: event.target.value })} /></label>
            ))}
            <label className="road-detail-field">Grass and open vegetation<input type="color" value={colors.neighborhood_surfaces.grass_color} onChange={(event) => updateColors({ ...colors, neighborhood_surfaces: { ...colors.neighborhood_surfaces, grass_color: event.target.value } })} /></label>
          </div>
        </details>
      )}
    </fieldset>
  );
}
