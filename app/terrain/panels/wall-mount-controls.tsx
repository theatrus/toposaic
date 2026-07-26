import { useEffect } from "react";

import type { GenerationSpec } from "../contracts";
import { RangeField } from "./range-field";

export function WallMountControls({
  spec,
  updateWallMount,
}: {
  spec: GenerationSpec;
  updateWallMount: <Key extends keyof GenerationSpec["wall_mount"]>(
    key: Key,
    value: GenerationSpec["wall_mount"][Key],
  ) => void;
}) {
  const mountEnabled = spec.wall_mount.style !== "none";
  const mountThickness =
    spec.wall_mount.target === "tray" ? spec.tray.floor_mm : spec.base_mm;
  const maximumMountDepth = Math.max(0.4, Math.min(3, mountThickness - 0.4));

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

  return (
    <>
      <div className="wall-mount-heading">
        <div>
          <strong className="color-title">Wall mounting</strong>
          <p>Cut a blind mount into the flat back.</p>
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
              <option value="terrain">Terrain pieces or solid</option>
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
            label={
              spec.wall_mount.style === "french_cleat"
                ? "Cleat slot height"
                : "Pin diameter"
            }
            value={spec.wall_mount.pin_diameter_mm}
            unit=" mm"
            min={2}
            max={10}
            step={0.5}
            onChange={(value) => updateWallMount("pin_diameter_mm", value)}
          />
          <p className="color-note">
            Each chosen output gets its own mount. Angled pin sockets and
            cleat receivers rise toward the map north edge. The cut keeps
            at least 0.4 mm below the terrain or tray face.
          </p>
        </>
      )}
    </>
  );
}
