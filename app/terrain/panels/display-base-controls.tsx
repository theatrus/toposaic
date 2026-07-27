import type { GenerationSpec } from "../contracts";
import { limitPlaceName } from "../config";
import type {
  UpdateGenerationSpec,
  UpdateTray,
} from "./mounting-types";
import { LabelFontSelect } from "./label-font-select";
import { RangeField } from "./range-field";

export function DisplayBaseControls({
  spec,
  update,
  updateTray,
  setTrayEnabled,
}: {
  spec: GenerationSpec;
  update: UpdateGenerationSpec;
  updateTray: UpdateTray;
  setTrayEnabled: (enabled: boolean) => void;
}) {
  return (
    <>
      <div className="color-heading">
        <div>
          <strong className="color-title">Display base</strong>
          <p>A shallow fitted tray for the terrain or puzzle pieces.</p>
        </div>
        <label className="color-toggle">
          <input
            aria-label="Generate display tray"
            type="checkbox"
            checked={spec.tray.enabled}
            onChange={(event) => setTrayEnabled(event.target.checked)}
          />
          <span>{spec.tray.enabled ? "On" : "Off"}</span>
        </label>
      </div>
      {spec.tray.enabled && (
        <>
          <label className="place-label-field">
            Place name
            <input
              type="text"
              required
              value={spec.place_name}
              onChange={(event) =>
                update("place_name", limitPlaceName(event.target.value))
              }
            />
            <small>
              Up to 48 characters. The tray adds the coordinates after this
              name. Letter case and Japanese text are preserved.
            </small>
          </label>
          <LabelFontSelect
            note="Bundled fonts keep the result the same on every OS."
            onChange={(font) => updateTray("label_font", font)}
            value={spec.tray.label_font}
          />
          <RangeField
            label="Label height"
            value={spec.tray.label_height_mm}
            unit=" mm"
            min={1.5}
            max={10}
            step={0.1}
            onChange={(value) => updateTray("label_height_mm", value)}
            note="Long labels shrink to fit the front lip."
          />
          <label className="tray-label-position-field">
            <span>Label position</span>
            <select
              aria-label="Label position"
              value={spec.tray.label_position}
              onChange={(event) =>
                updateTray(
                  "label_position",
                  event.target.value as GenerationSpec["tray"]["label_position"],
                )
              }
            >
              <option value="left">Left</option>
              <option value="center">Center</option>
              <option value="right">Right</option>
            </select>
            <small>Place the full name and coordinate line on the lip.</small>
          </label>
          <label className="option-toggle">
            <input
              aria-label="Draw contour lines on tray"
              type="checkbox"
              checked={spec.tray.contours_enabled}
              onChange={(event) =>
                updateTray("contours_enabled", event.target.checked)
              }
            />
            <span>
              <strong>Contour lines</strong>
              <small>Draw terrain contours on the tray floor.</small>
            </span>
          </label>
          <RangeField
            label="Tray clearance"
            value={spec.tray.clearance_mm}
            unit=" mm"
            min={0.2}
            max={2}
            step={0.1}
            onChange={(value) => updateTray("clearance_mm", value)}
          />
          <RangeField
            label="Rim width"
            value={spec.tray.rim_width_mm}
            unit=" mm"
            min={5}
            max={16}
            step={0.5}
            onChange={(value) => updateTray("rim_width_mm", value)}
          />
          <RangeField
            label="Floor thickness"
            value={spec.tray.floor_mm}
            unit=" mm"
            min={1}
            max={20}
            step={0.2}
            onChange={(value) => updateTray("floor_mm", value)}
          />
          <RangeField
            label="Rim height above floor"
            value={spec.tray.rim_height_mm}
            unit=" mm"
            min={2}
            max={8}
            step={0.2}
            onChange={(value) => updateTray("rim_height_mm", value)}
          />
          {spec.tray.contours_enabled && (
            <RangeField
              label="Contour line count"
              value={spec.tray.contour_count}
              unit=""
              min={5}
              max={60}
              step={1}
              onChange={(value) => updateTray("contour_count", value)}
            />
          )}
          {(spec.adjacent_columns > 1 || spec.adjacent_rows > 1) && (
            <label className="option-toggle">
              <input
                type="checkbox"
                checked={spec.tray.individual_tiles}
                onChange={(event) =>
                  updateTray("individual_tiles", event.target.checked)
                }
              />
              <span>
                <strong>Separate framed trays</strong>
                <small>
                  Make one complete tray per terrain tile instead of one
                  joined mosaic tray.
                </small>
              </span>
            </label>
          )}
          <p className="color-note">
            The color 3MF prints the chosen tray details and the place name,
            latitude, and longitude as raised shapes on the top front lip.
            Mosaic trays follow the terrain grid and its shared-edge setting.
            The job also includes a plain STL.
          </p>
        </>
      )}
    </>
  );
}
