import type { GenerationSpec } from "../contracts";
import { RangeField } from "./range-field";
import { WallMountControls } from "./wall-mount-controls";

export function TrayPanel({
  hidden,
  spec,
  update,
  updateTray,
  updateWallMount,
}: {
  hidden: boolean;
  spec: GenerationSpec;
  update: <Key extends keyof GenerationSpec>(
    key: Key,
    value: GenerationSpec[Key],
  ) => void;
  updateTray: <Key extends keyof GenerationSpec["tray"]>(
    key: Key,
    value: GenerationSpec["tray"][Key],
  ) => void;
  updateWallMount: <Key extends keyof GenerationSpec["wall_mount"]>(
    key: Key,
    value: GenerationSpec["wall_mount"][Key],
  ) => void;
}) {
  return (
    <fieldset
      className="color-controls tray-controls control-section"
      aria-label="Shallow terrain tray"
      hidden={hidden}
    >
      <div className="color-heading">
        <div>
          <strong className="color-title">Shallow terrain tray</strong>
          <p>A fitted base for the terrain or puzzle pieces.</p>
        </div>
        <label className="color-toggle">
          <input
            type="checkbox"
            checked={spec.tray.enabled}
            onChange={(event) =>
              updateTray("enabled", event.target.checked)
            }
          />
          <span>{spec.tray.enabled ? "On" : "Off"}</span>
        </label>
      </div>
      <label className="place-label-field">
        Place name
        <input
          type="text"
          maxLength={48}
          required
          value={spec.place_name}
          onChange={(event) => update("place_name", event.target.value)}
        />
        <small>The tray adds the coordinates after this name.</small>
      </label>
      {spec.tray.enabled && (
        <>
          <div className="color-swatches">
            {(
              [
                ["Tray", "tray_color"],
                ["Contours", "contour_color"],
                ["Label", "label_color"],
              ] as const
            ).map(([label, key]) => (
              <label key={key}>
                <input
                  type="color"
                  value={spec.tray[key]}
                  onChange={(event) =>
                    updateTray(key, event.target.value)
                  }
                />
                <span>{label}</span>
                <code>{String(spec.tray[key]).toUpperCase()}</code>
              </label>
            ))}
          </div>
          <label className="tray-chunk-toggle">
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
            max={4}
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
            <label className="tray-chunk-toggle">
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
      <WallMountControls spec={spec} updateWallMount={updateWallMount} />
    </fieldset>
  );
}
