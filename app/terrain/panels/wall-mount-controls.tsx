import { useEffect } from "react";

import type { GenerationSpec } from "../contracts";
import {
  maximumCleatWidth,
  maximumMountDepth,
  wallHardwareQuantity,
} from "../mounting";
import type { UpdateWallMount } from "./mounting-types";
import { RangeField } from "./range-field";

export function WallMountControls({
  spec,
  updateWallMount,
}: {
  spec: GenerationSpec;
  updateWallMount: UpdateWallMount;
}) {
  const mountEnabled = spec.wall_mount.style !== "none";
  const maximumDepth = maximumMountDepth(spec);
  const maximumWidth = maximumCleatWidth(spec);
  const hardwareQuantity = wallHardwareQuantity(spec);

  useEffect(() => {
    if (mountEnabled && spec.wall_mount.depth_mm > maximumDepth) {
      updateWallMount("depth_mm", maximumDepth);
    }
  }, [
    maximumDepth,
    mountEnabled,
    spec.wall_mount.depth_mm,
    updateWallMount,
  ]);

  useEffect(() => {
    if (
      spec.wall_mount.style === "french_cleat" &&
      spec.wall_mount.cleat_width_mm > maximumWidth
    ) {
      updateWallMount("cleat_width_mm", maximumWidth);
    }
  }, [
    maximumWidth,
    spec.wall_mount.cleat_width_mm,
    spec.wall_mount.style,
    updateWallMount,
  ]);

  return (
    <>
      <div className="mounting-section-heading">
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
            label={
              spec.wall_mount.style === "french_cleat"
                ? "Receiver pocket depth"
                : "Mount cut depth"
            }
            value={spec.wall_mount.depth_mm}
            unit=" mm"
            min={0.4}
            max={maximumDepth}
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
              max={maximumWidth}
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
          <label className="option-toggle">
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
                label="Wall stand-off"
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
            Each chosen output gets its own mount. The French cleat uses a
            flush back pocket with a lower entry box, then slides toward the
            map north edge to lock. Pocket depth sets its grip; wall stand-off
            leaves room for an uneven wall. The cut keeps at least 0.4 mm below
            the terrain or display-base face.
            {spec.wall_mount.export_hardware &&
              ` Print ${hardwareQuantity} ${hardwareQuantity === 1 ? "copy" : "copies"} of the wall-side hardware.${spec.wall_mount.style === "french_cleat" ? ` The job also includes a flat alignment spacer; print ${hardwareQuantity} ${hardwareQuantity === 1 ? "copy" : "copies"} and place their outer edges together to set the cleat grid.` : ""}`}
          </p>
        </>
      )}
    </>
  );
}
