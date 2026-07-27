import { useEffect } from "react";

import type { GenerationSpec } from "../contracts";
import {
  maximumCleatWidth,
  maximumMountDepth,
  maximumWallPlateThickness,
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
  const maximumThickness = maximumWallPlateThickness(spec);
  const minimumThickness = Number(
    (spec.wall_mount.wall_offset_mm + 0.4).toFixed(2),
  );
  const maximumWidth = maximumCleatWidth(spec);
  const hardwareQuantity = wallHardwareQuantity(spec);
  const backThickness =
    spec.wall_mount.target === "tray" ? spec.tray.floor_mm : spec.base_mm;
  const backThicknessLabel =
    spec.wall_mount.target === "tray"
      ? "display-base floor"
      : "minimum piece height";
  const maximumCountersinkDepth = Math.max(
    0,
    Math.min(3, Number((spec.wall_mount.thickness_mm - 0.4).toFixed(2))),
  );
  const embeddedDepth =
    spec.wall_mount.thickness_mm -
    spec.wall_mount.wall_offset_mm +
    Math.max(
      spec.wall_mount.depth_mm,
      spec.wall_mount.screw_head_clearance_mm,
    );
  const depthViolation = mountEnabled && embeddedDepth > backThickness - 0.4;
  const requiredBackThickness = embeddedDepth + 0.4;

  useEffect(() => {
    if (mountEnabled && spec.wall_mount.thickness_mm < minimumThickness) {
      updateWallMount("thickness_mm", minimumThickness);
    }
  }, [
    minimumThickness,
    mountEnabled,
    spec.wall_mount.thickness_mm,
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

  useEffect(() => {
    if (
      mountEnabled &&
      spec.wall_mount.screw_countersink_depth_mm > maximumCountersinkDepth
    ) {
      updateWallMount("screw_countersink_depth_mm", maximumCountersinkDepth);
    }
  }, [
    maximumCountersinkDepth,
    mountEnabled,
    spec.wall_mount.screw_countersink_depth_mm,
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
                Full terrain tile
              </option>
              <option value="tray">Tray or tray sections</option>
            </select>
            {spec.wall_mount.target === "tray" && !spec.tray.enabled && (
              <small>Turn on the tray to export this mount.</small>
            )}
          </label>
          <RangeField
            label="Mount position from top"
            value={Number(
              (spec.wall_mount.vertical_position_ratio * 100).toFixed(1),
            )}
            unit="%"
            min={16.7}
            max={83.3}
            step={0.1}
            onChange={(value) =>
              updateWallMount(
                "vertical_position_ratio",
                Number((value / 100).toFixed(4)),
              )
            }
          />
          <RangeField
            label={
              spec.wall_mount.style === "french_cleat"
                ? "Cleat engagement depth"
                : "Pin engagement depth"
            }
            value={spec.wall_mount.depth_mm}
            unit=" mm"
            min={0.4}
            max={maximumDepth}
            step={0.2}
            onChange={(value) => updateWallMount("depth_mm", value)}
          />
          <RangeField
            label="Wall plate thickness"
            value={spec.wall_mount.thickness_mm}
            unit=" mm"
            min={minimumThickness}
            max={Math.max(minimumThickness, maximumThickness)}
            step={0.2}
            onChange={(value) => updateWallMount("thickness_mm", value)}
          />
          {depthViolation && (
            <p className="color-note" role="alert">
              The {backThickness.toFixed(1)} mm {backThicknessLabel} is too thin
              for this mount. Raise it to at least{" "}
              {requiredBackThickness.toFixed(1)} mm, or reduce the engagement
              depth, screw-head clearance, or plate thickness, or raise the wall
              offset. TopoSaic will not add height for you.
            </p>
          )}
          <RangeField
            label="Wall offset"
            value={spec.wall_mount.wall_offset_mm}
            unit=" mm"
            min={0}
            max={Math.min(10, spec.wall_mount.thickness_mm - 0.4)}
            step={0.2}
            onChange={(value) => updateWallMount("wall_offset_mm", value)}
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
              <small>Export a matching peg or cleat on a screw-on plate.</small>
            </span>
          </label>
          <RangeField
            label="Pocket and hardware fit clearance"
            value={spec.wall_mount.fit_clearance_mm}
            unit=" mm"
            min={0.1}
            max={0.8}
            step={0.05}
            onChange={(value) => updateWallMount("fit_clearance_mm", value)}
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
          <RangeField
            label="Screw countersink depth"
            value={spec.wall_mount.screw_countersink_depth_mm}
            unit=" mm"
            min={0}
            max={maximumCountersinkDepth}
            step={0.2}
            onChange={(value) =>
              updateWallMount("screw_countersink_depth_mm", value)
            }
          />
          <RangeField
            label="Screw-head pocket clearance"
            value={spec.wall_mount.screw_head_clearance_mm}
            unit=" mm"
            min={0}
            max={3}
            step={0.2}
            onChange={(value) =>
              updateWallMount("screw_head_clearance_mm", value)
            }
          />
          <label className="option-toggle">
            <input
              aria-label="Add edge screw holes on wide mounts"
              type="checkbox"
              checked={spec.wall_mount.wide_edge_screws}
              onChange={(event) =>
                updateWallMount("wide_edge_screws", event.target.checked)
              }
            />
            <span>
              <strong>Wide-mount edge screws</strong>
              <small>
                Add a screw near each end on mounts at least 40 mm wide when
                the target has room. The alignment jig uses the same holes.
              </small>
            </span>
          </label>
          <p className="color-note">
            Engagement depth sets how far the pin or cleat enters the model for
            rigidity. Wall plate thickness is the full plate from its wall face
            to its model face. Wall offset is the finished gap for an uneven
            wall. TopoSaic derives the hidden pocket from plate thickness minus
            wall offset. Every cut keeps at least 0.4 mm below the terrain or
            display-base face. Terrain mounts use one full-tile layout; jigsaw
            pieces receive the sections that cross them, while tray-retention
            pins remain per piece. The pocket covers the full wall plate at
            entry and at lock. A 90° countersink keeps flat screw heads in the
            printed plate; set its depth to zero for plain holes. Screw-head
            pocket clearance adds local depth behind each swept head without
            changing wall offset. Mount position measures down from the map
            north edge. Wide mounts can add end screws when the target fits
            them; the printed hardware, pocket, and alignment jig share one
            screw layout. French cleat travel grows with slot height.
            French cleats and angled pins slide toward the map north edge to
            lock.
            {spec.wall_mount.export_hardware &&
              ` Print ${hardwareQuantity} ${hardwareQuantity === 1 ? "copy" : "copies"} of the wall-side hardware.${spec.wall_mount.style === "french_cleat" ? ` The job also includes a flat alignment spacer; print ${hardwareQuantity} ${hardwareQuantity === 1 ? "copy" : "copies"} and place their outer edges together to set the cleat grid.` : ""}`}
          </p>
        </>
      )}
    </>
  );
}
