import { useEffect } from "react";

import type { GenerationSpec } from "../contracts";
import { RangeField } from "./range-field";

export function WallMountControls({
  spec,
  updatePuzzleRetention,
  updateWallMount,
}: {
  spec: GenerationSpec;
  updatePuzzleRetention: <Key extends keyof GenerationSpec["puzzle_retention"]>(
    key: Key,
    value: GenerationSpec["puzzle_retention"][Key],
  ) => void;
  updateWallMount: <Key extends keyof GenerationSpec["wall_mount"]>(
    key: Key,
    value: GenerationSpec["wall_mount"][Key],
  ) => void;
}) {
  const mountEnabled = spec.wall_mount.style !== "none";
  const mountThickness =
    spec.wall_mount.target === "tray" ? spec.tray.floor_mm : spec.base_mm;
  const maximumMountDepth = Math.max(0.4, Math.min(3, mountThickness - 0.4));
  const maximumRetentionHeight = Math.max(
    0.4,
    Math.min(3, spec.base_mm - spec.puzzle_retention.clearance_mm - 0.4),
  );
  const superTileCount = spec.adjacent_columns * spec.adjacent_rows;
  const hardwareQuantity =
    spec.wall_mount.target === "tray"
      ? superTileCount > 1
        ? superTileCount
        : spec.tray.segment_columns * spec.tray.segment_rows
      : superTileCount * (spec.solid_model ? 1 : spec.rows * spec.columns);

  useEffect(() => {
    if (mountEnabled && spec.wall_mount.depth_mm > maximumMountDepth) {
      updateWallMount("depth_mm", maximumMountDepth);
    }
  }, [
    maximumMountDepth,
    mountEnabled,
    spec.wall_mount.depth_mm,
    updateWallMount,
  ]);

  useEffect(() => {
    if (
      spec.puzzle_retention.enabled &&
      spec.puzzle_retention.pin_height_mm > maximumRetentionHeight
    ) {
      updatePuzzleRetention("pin_height_mm", maximumRetentionHeight);
    }
  }, [
    maximumRetentionHeight,
    spec.puzzle_retention.enabled,
    spec.puzzle_retention.pin_height_mm,
    updatePuzzleRetention,
  ]);

  useEffect(() => {
    if (
      spec.puzzle_retention.enabled &&
      mountEnabled &&
      spec.wall_mount.target === "terrain"
    ) {
      updateWallMount("target", "tray");
    }
  }, [
    mountEnabled,
    spec.puzzle_retention.enabled,
    spec.wall_mount.target,
    updateWallMount,
  ]);

  return (
    <>
      <div className="wall-mount-heading">
        <div>
          <strong className="color-title">Puzzle retention</strong>
          <p>Pin the terrain into the tray for an upright display.</p>
        </div>
      </div>
      <label className="tray-chunk-toggle">
        <input
          aria-label="Pin puzzle into tray"
          type="checkbox"
          checked={spec.puzzle_retention.enabled}
          disabled={!spec.tray.enabled}
          onChange={(event) => {
            updatePuzzleRetention("enabled", event.target.checked);
            if (event.target.checked && mountEnabled) {
              updateWallMount("target", "tray");
            }
          }}
        />
        <span>
          <strong>Retention pins</strong>
          <small>
            Add pins to the tray floor and loose-fit sockets to the terrain.
          </small>
        </span>
      </label>
      {!spec.tray.enabled && (
        <p className="color-note">Turn on the tray to use retention pins.</p>
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
            max={maximumRetentionHeight}
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
      <div className="wall-mount-heading">
        <div>
          <strong className="color-title">Wall mounting</strong>
          <p>Cut a receiver into the back and print its wall-side hardware.</p>
        </div>
      </div>
      <label className="road-detail-field">
        Mount style
        <select
          aria-label="Wall mount style"
          value={spec.wall_mount.style}
          onChange={(event) =>
            updateWallMount(
              "style",
              event.target.value as GenerationSpec["wall_mount"]["style"],
            )
          }
        >
          <option value="none">None</option>
          <option value="straight_pin">Straight pin socket</option>
          <option value="angled_pin">Angled pin socket</option>
          <option value="french_cleat">French cleat receiver</option>
        </select>
      </label>
      {mountEnabled && (
        <>
          <label className="road-detail-field">
            Cut into
            <select
              aria-label="Wall mount target"
              value={spec.wall_mount.target}
              onChange={(event) =>
                updateWallMount(
                  "target",
                  event.target.value as GenerationSpec["wall_mount"]["target"],
                )
              }
            >
              <option
                value="terrain"
                disabled={spec.puzzle_retention.enabled}
              >
                Terrain pieces or solid
              </option>
              <option value="tray">Tray or tray sections</option>
            </select>
            {spec.wall_mount.target === "tray" && !spec.tray.enabled && (
              <small>Turn on the tray to export this mount.</small>
            )}
          </label>
          <RangeField
            label="Mount cut depth"
            value={spec.wall_mount.depth_mm}
            unit=" mm"
            min={0.4}
            max={maximumMountDepth}
            step={0.2}
            onChange={(value) => updateWallMount("depth_mm", value)}
          />
          <RangeField
            label={spec.wall_mount.style === "french_cleat" ? "Cleat slot height" : "Pin diameter"}
            value={spec.wall_mount.pin_diameter_mm}
            unit=" mm"
            min={2}
            max={10}
            step={0.5}
            onChange={(value) => updateWallMount("pin_diameter_mm", value)}
          />
          {spec.wall_mount.style === "french_cleat" ? (
            <RangeField
              label="Cleat width"
              value={spec.wall_mount.cleat_width_mm}
              unit=" mm"
              min={8}
              max={100}
              step={1}
              onChange={(value) => updateWallMount("cleat_width_mm", value)}
            />
          ) : (
            <>
              <label className="road-detail-field">
                Pin count
                <select
                  aria-label="Wall mount pin count"
                  value={spec.wall_mount.pin_count}
                  onChange={(event) =>
                    updateWallMount("pin_count", Number(event.target.value))
                  }
                >
                  <option value={1}>One</option>
                  <option value={2}>Two</option>
                </select>
              </label>
              {spec.wall_mount.pin_count === 2 && (
                <RangeField
                  label="Pin spacing"
                  value={spec.wall_mount.pin_spacing_mm}
                  unit=" mm"
                  min={12}
                  max={100}
                  step={1}
                  onChange={(value) => updateWallMount("pin_spacing_mm", value)}
                />
              )}
            </>
          )}
          <label className="tray-chunk-toggle">
            <input
              aria-label="Export matching wall hardware"
              type="checkbox"
              checked={spec.wall_mount.export_hardware}
              onChange={(event) =>
                updateWallMount("export_hardware", event.target.checked)
              }
            />
            <span>
              <strong>Wall-side hardware</strong>
              <small>Export a matching peg or cleat on a screw-on spacer.</small>
            </span>
          </label>
          {spec.wall_mount.export_hardware && (
            <>
              <RangeField
                label="Hardware fit clearance"
                value={spec.wall_mount.fit_clearance_mm}
                unit=" mm"
                min={0.1}
                max={0.8}
                step={0.05}
                onChange={(value) => updateWallMount("fit_clearance_mm", value)}
              />
              <RangeField
                label="Wall spacer depth"
                value={spec.wall_mount.spacer_depth_mm}
                unit=" mm"
                min={1.2}
                max={10}
                step={0.2}
                onChange={(value) => updateWallMount("spacer_depth_mm", value)}
              />
              <RangeField
                label="Screw-hole diameter"
                value={spec.wall_mount.screw_hole_diameter_mm}
                unit=" mm"
                min={2}
                max={6}
                step={0.5}
                onChange={(value) =>
                  updateWallMount("screw_hole_diameter_mm", value)
                }
              />
            </>
          )}
          <p className="color-note">
            Each chosen output gets its own mount. Angled pin sockets and
            cleat receivers rise toward the map north edge. The cut keeps
            at least 0.4 mm below the terrain or tray face.
            {spec.wall_mount.export_hardware &&
              ` Print ${hardwareQuantity} ${hardwareQuantity === 1 ? "copy" : "copies"} of the wall-side hardware.`}
          </p>
        </>
      )}
    </>
  );
}
