import type { GenerationSpec } from "../contracts";

export function ModelTypePanel({
  hidden,
  setPieceLayout,
  spec,
  update,
}: {
  hidden: boolean;
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
          className={!spec.solid_model ? "active" : ""}
          onClick={() => update("solid_model", false)}
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
          className={spec.solid_model ? "active" : ""}
          onClick={() => update("solid_model", true)}
        >
          <span className="mode-mark solid-mark" aria-hidden="true" />
          <span>
            <strong>Solid terrain</strong>
            <small>One watertight model, no cuts</small>
          </span>
        </button>
      </div>

      {!spec.solid_model && (
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
