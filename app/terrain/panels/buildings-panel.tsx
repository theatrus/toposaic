import type { GenerationSpec } from "../contracts";
import { RangeField } from "./range-field";

export function BuildingsPanel({
  hidden,
  spec,
  updateBuildings,
}: {
  hidden: boolean;
  spec: GenerationSpec;
  updateBuildings: <Key extends keyof GenerationSpec["buildings"]>(
    key: Key,
    value: GenerationSpec["buildings"][Key],
  ) => void;
}) {
  return (
    <fieldset
      className="color-controls building-controls control-section"
      aria-label="Mapped buildings"
      hidden={hidden}
    >
      <div className="color-heading">
        <div>
          <strong className="color-title">Mapped buildings</strong>
          <p>Raise OpenStreetMap building footprints above the terrain.</p>
        </div>
      </div>
      <label className="option-toggle feature-enable-toggle">
        <input
          aria-label="Enable mapped buildings"
          type="checkbox"
          checked={spec.buildings.enabled}
          onChange={(event) =>
            updateBuildings("enabled", event.target.checked)
          }
        />
        <span>
          <strong>Enable mapped buildings</strong>
          <small>
            {spec.buildings.enabled
              ? "On · building footprints will be fetched and raised."
              : "Off · no mapped building geometry will be added."}
          </small>
        </span>
      </label>
      {spec.buildings.enabled && (
        <>
          <RangeField
            label="Building Z scale"
            value={spec.buildings.z_scale}
            unit="×"
            min={0.5}
            max={30}
            step={0.5}
            onChange={(value) => updateBuildings("z_scale", value)}
          />
          <p className="color-note">
            Buildings use exact mapped footprints, flat roofs, straight
            vertical walls, and their color from the Colors tab. 1× keeps true
            height against the map width. Higher values make small buildings
            easier to print. Tagged heights are used first, then floor count,
            then an 8 m default.
          </p>
        </>
      )}
    </fieldset>
  );
}
