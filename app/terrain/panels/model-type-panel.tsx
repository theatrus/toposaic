import type { GenerationSpec, GlobeSpec } from "../contracts";

export function ModelTypePanel({
  hidden,
  globeLayoutSize,
  onModelTypeChange,
  setPieceLayout,
  spec,
  update,
}: {
  hidden: boolean;
  globeLayoutSize?: number;
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

function GlobeControls({ hidden, globe, colors, layoutSize, update, updateColors }: {
  hidden: boolean;
  globe: GlobeSpec;
  colors: GenerationSpec["color_output"];
  layoutSize?: number;
  update: (globe: GlobeSpec) => void;
  updateColors: (colors: GenerationSpec["color_output"]) => void;
}) {
  const mapped = globe.source !== "fit_test";
  const printLayout = globe.batch ? "batch" : globe.full_shell || globe.source === "whole_earth" ? "full" : "fixture";
  const batch = globe.batch;
  const setLayout = (layout: string) => update({
    ...globe,
    full_shell: layout !== "fixture",
    batch: layout === "batch" ? batch ?? { first_tile: 1, tile_count: 8 } : null,
    whole_earth_detail: layout === "batch" && globe.whole_earth_detail,
  });
  const range = (key: "seam_gap_mm" | "wrap_degrees" | "relief_mm" | "texture_mm" | "diameter_mm" | "bed_mm" | "clip_fit_mm" | "locator_clearance_mm" | "frame_depth_mm", label: string, min: number, max: number, step: number, decimals = 0, unit = " mm") => (
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
          update({ ...globe, source, full_shell: withTerrain || globe.full_shell,
            whole_earth_detail: source === "whole_earth" && globe.whole_earth_detail,
            surface_colors: globe.source === "fit_test" && withTerrain ? true : globe.surface_colors });
        }}>
          <option value="fit_test">Plain fit test</option>
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
          {globe.source !== "whole_earth" && <option value="fixture">3-tile fit test</option>}
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
            <input aria-label="Tiles in batch" type="number" min="1" max="80" value={batch.tile_count} onChange={(event) => {
              if (Number.isInteger(event.target.valueAsNumber) && event.target.valueAsNumber >= 1 && event.target.valueAsNumber <= 80) update({ ...globe, batch: { ...batch, tile_count: event.target.valueAsNumber } });
            }} />
            <small>Each kit includes matching frame modules and the joints owned by its tiles.</small>
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
              <option value="full">Full wrap</option>
            </select>
            {globe.full_wrap && <small>The east and west edges blend together. The top and bottom edges converge at the poles.</small>}
          </label>
          {!globe.full_wrap && range("wrap_degrees", "Regional wrap", 45, 140, 5, 0, "°")}
        </>
      )}
      {mapped && (
        <>
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
          {range("texture_mm", "Tactile texture", 0, 0.9, 0.05, 2)}
          <label className="option-toggle feature-enable-toggle globe-color-toggle">
            <input type="checkbox" checked={globe.surface_colors} onChange={(event) => update({ ...globe, surface_colors: event.target.checked })} />
            <span><strong>Mapped surface colors</strong><small>Paint the tiles in 3MF with a shared filament palette.</small></span>
          </label>
        </>
      )}
      <details className="globe-detail">
        <summary>Frame and snap fit</summary>
        <div className="globe-detail-fields">
          {range("frame_depth_mm", "Frame depth", 4.5, 12, 0.5, 1)}
          {range("clip_fit_mm", "Clip interference", 0.15, 0.6, 0.05, 2)}
          {range("locator_clearance_mm", "Locator clearance per side", 0.1, 0.6, 0.05, 2)}
        </div>
      </details>
      {mapped && globe.surface_colors && (
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
