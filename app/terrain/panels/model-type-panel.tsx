import type { GenerationSpec, GlobeSpec } from "../contracts";

export function ModelTypePanel({
  hidden,
  onModelTypeChange,
  setPieceLayout,
  spec,
  update,
}: {
  hidden: boolean;
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

      {spec.globe && <GlobeControls hidden={hidden} globe={spec.globe} update={(globe) => update("globe", globe)} />}

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

function GlobeControls({ hidden, globe, update }: {
  hidden: boolean;
  globe: GlobeSpec;
  update: (globe: GlobeSpec) => void;
}) {
  return (
    <fieldset className="control-section globe-controls" hidden={hidden}>
      <legend>Globe settings</legend>
      <label className="road-detail-field">
        Globe source
        <select aria-label="Globe source" value={globe.source} onChange={(event) => {
          const source = event.target.value === "small_world" ? "small_world" : "fit_test";
          update({ ...globe, source, full_shell: source === "small_world" || globe.full_shell });
        }}>
          <option value="fit_test">Plain fit test</option>
          <option value="small_world">Small World</option>
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
        <select aria-label="Print layout" value={globe.full_shell ? "full" : "fixture"} onChange={(event) => update({ ...globe, full_shell: event.target.value === "full" })}>
          <option value="fixture">3-tile fit test</option>
          <option value="full">20-tile globe</option>
        </select>
      </label>
      <label className="road-detail-field">
        Seam clearance · {globe.seam_gap_mm.toFixed(2)} mm
        <input aria-label="Seam clearance" type="range" min="0.2" max="1.2" step="0.05" value={globe.seam_gap_mm} onChange={(event) => update({ ...globe, seam_gap_mm: event.target.valueAsNumber })} />
      </label>
      {globe.source === "small_world" ? (
        <>
          <label className="road-detail-field">
            Regional wrap · {globe.wrap_degrees}°
            <input aria-label="Regional wrap" type="range" min="45" max="140" step="5" value={globe.wrap_degrees} onChange={(event) => update({ ...globe, wrap_degrees: event.target.valueAsNumber })} />
            <small>Angle from the center of the region to a map corner.</small>
          </label>
          <label className="road-detail-field">
            Terrain relief · {globe.relief_mm.toFixed(1)} mm
            <input aria-label="Terrain relief" type="range" min="0" max="12" step="0.5" value={globe.relief_mm} onChange={(event) => update({ ...globe, relief_mm: event.target.valueAsNumber })} />
          </label>
          <p>Select the region on the map. Its elevation fades at the edges into the 200 mm globe.</p>
        </>
      ) : <p>200 mm reference diameter. The plain surface is for testing the clip and frame fit.</p>}
    </fieldset>
  );
}
