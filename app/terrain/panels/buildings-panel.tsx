import type { GenerationSpec } from "../contracts";
import { RangeField } from "./range-field";

export function BuildingsPanel({
  hidden,
  spec,
  updateBuildings,
  updateColor,
}: {
  hidden: boolean;
  spec: GenerationSpec;
  updateBuildings: <Key extends keyof GenerationSpec["buildings"]>(
    key: Key,
    value: GenerationSpec["buildings"][Key],
  ) => void;
  updateColor: <Key extends keyof GenerationSpec["color_output"]>(
    key: Key,
    value: GenerationSpec["color_output"][Key],
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
        <label className="color-toggle">
          <input
            type="checkbox"
            checked={spec.buildings.enabled}
            onChange={(event) =>
              updateBuildings("enabled", event.target.checked)
            }
          />
          <span>{spec.buildings.enabled ? "On" : "Off"}</span>
        </label>
      </div>
      {spec.buildings.enabled && (
        <>
          <div className="color-swatches building-color-swatch">
            <label>
              <input
                aria-label="Building color"
                type="color"
                value={spec.color_output.building_color}
                onChange={(event) =>
                  updateColor("building_color", event.target.value)
                }
              />
              <span>Building color</span>
              <code>{spec.color_output.building_color.toUpperCase()}</code>
            </label>
          </div>
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
            vertical walls, and their own 3MF color material. 1× keeps
            true height against the map width. Higher values make small
            buildings easier to print. Tagged heights are used first,
            then floor count, then an 8 m default.
          </p>
        </>
      )}
    </fieldset>
  );
}
