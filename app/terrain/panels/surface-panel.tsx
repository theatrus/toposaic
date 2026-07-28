import type { GenerationSpec } from "../contracts";
import { SurfaceFerrySection } from "./surface-ferry-section";
import { SurfaceRailSection } from "./surface-rail-section";
import { SurfaceRoadSection } from "./surface-road-section";
import { SurfaceTerrainSection } from "./surface-terrain-section";
import { SurfaceTrailSection } from "./surface-trail-section";
import type { UpdateColor } from "./surface-types";
import { SurfaceWaterSection } from "./surface-water-section";

export function SurfacePanel({
  hidden,
  importTrailFiles,
  removeTrail,
  spec,
  trailNotice,
  updateColor,
}: {
  hidden: boolean;
  importTrailFiles: (files: File[]) => Promise<void>;
  removeTrail: (index: number) => void;
  spec: GenerationSpec;
  trailNotice: string | null;
  updateColor: UpdateColor;
}) {
  return (
    <fieldset
      className="color-controls control-section surface-controls"
      aria-label="Surface settings"
      hidden={hidden}
    >
      <div className="color-heading">
        <div>
          <strong className="color-title">Mapped surface</strong>
          <p>Paint the 3MF from mapped land cover, water, and routes.</p>
        </div>
        <label className="color-toggle">
          <input
            type="checkbox"
            checked={spec.color_output.enabled}
            onChange={(event) => updateColor("enabled", event.target.checked)}
          />
          <span>{spec.color_output.enabled ? "On" : "Off"}</span>
        </label>
      </div>
      {spec.color_output.enabled && (
        <>
          <SurfaceTerrainSection spec={spec} updateColor={updateColor} />
          <SurfaceWaterSection spec={spec} updateColor={updateColor} />
          <SurfaceRoadSection spec={spec} updateColor={updateColor} />
          <SurfaceRailSection spec={spec} updateColor={updateColor} />
          <SurfaceFerrySection spec={spec} updateColor={updateColor} />
          <p className="color-note">
            WorldCover supplies land cover and permanent water. OpenStreetMap
            supplies waterways and transport lines. Snow is not live. Sides
            and bottoms use the rock color.
          </p>
        </>
      )}
      <SurfaceTrailSection
        importTrailFiles={importTrailFiles}
        removeTrail={removeTrail}
        spec={spec}
        trailNotice={trailNotice}
        updateColor={updateColor}
      />
    </fieldset>
  );
}
