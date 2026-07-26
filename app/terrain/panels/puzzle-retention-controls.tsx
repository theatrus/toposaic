import { useEffect } from "react";

import type { GenerationSpec } from "../contracts";
import { maximumRetentionHeight } from "../mounting";
import type { UpdatePuzzleRetention } from "./mounting-types";
import { RangeField } from "./range-field";

export function PuzzleRetentionControls({
  spec,
  updatePuzzleRetention,
}: {
  spec: GenerationSpec;
  updatePuzzleRetention: UpdatePuzzleRetention;
}) {
  const maximumHeight = maximumRetentionHeight(spec);

  useEffect(() => {
    if (
      spec.puzzle_retention.enabled &&
      spec.puzzle_retention.pin_height_mm > maximumHeight
    ) {
      updatePuzzleRetention("pin_height_mm", maximumHeight);
    }
  }, [
    maximumHeight,
    spec.puzzle_retention.enabled,
    spec.puzzle_retention.pin_height_mm,
    updatePuzzleRetention,
  ]);

  return (
    <>
      <div className="mounting-section-heading">
        <div>
          <strong className="color-title">Puzzle retention</strong>
          <p>Pin the terrain into the tray for an upright display.</p>
        </div>
      </div>
      <label className="option-toggle">
        <input
          aria-label="Pin puzzle into tray"
          type="checkbox"
          checked={spec.puzzle_retention.enabled}
          disabled={!spec.tray.enabled}
          onChange={(event) =>
            updatePuzzleRetention("enabled", event.target.checked)
          }
        />
        <span>
          <strong>Retention pins</strong>
          <small>
            Add pins to the tray floor and loose-fit sockets to the terrain.
          </small>
        </span>
      </label>
      {!spec.tray.enabled && (
        <p className="color-note">Turn on the display base to use retention pins.</p>
      )}
      {spec.puzzle_retention.enabled && spec.tray.enabled && (
        <>
          <RangeField
            label="Retention pin diameter"
            value={spec.puzzle_retention.pin_diameter_mm}
            unit=" mm"
            min={2}
            max={8}
            step={0.5}
            onChange={(value) =>
              updatePuzzleRetention("pin_diameter_mm", value)
            }
          />
          <RangeField
            label="Retention pin height"
            value={spec.puzzle_retention.pin_height_mm}
            unit=" mm"
            min={0.4}
            max={maximumHeight}
            step={0.2}
            onChange={(value) => updatePuzzleRetention("pin_height_mm", value)}
          />
          <RangeField
            label="Retention fit clearance"
            value={spec.puzzle_retention.clearance_mm}
            unit=" mm"
            min={0.1}
            max={0.6}
            step={0.05}
            onChange={(value) => updatePuzzleRetention("clearance_mm", value)}
          />
        </>
      )}
    </>
  );
}
