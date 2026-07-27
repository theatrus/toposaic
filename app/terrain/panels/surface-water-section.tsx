import type { GenerationSpec } from "../contracts";
import { RangeField } from "./range-field";
import { SurfaceSection } from "./surface-section";
import type { UpdateColor } from "./surface-types";

export function SurfaceWaterSection({
  spec,
  updateColor,
}: {
  spec: GenerationSpec;
  updateColor: UpdateColor;
}) {
  return (
    <SurfaceSection
      name="Water"
      description="Add mapped lakes and waterways, then limit minor streams in dense drainage networks."
    >
      <div className="road-options">
        <label className="color-toggle">
          <input
            type="checkbox"
            checked={spec.color_output.osm_water_enabled}
            onChange={(event) =>
              updateColor("osm_water_enabled", event.target.checked)
            }
          />
          <span>OpenStreetMap waterways</span>
        </label>
        <small>Adds smooth rivers, streams, canals, and mapped water areas.</small>
      </div>
      {spec.color_output.osm_water_enabled && (
        <>
          <RangeField
            label="Maximum waterway coverage"
            value={spec.color_output.waterway_coverage_percent}
            unit="%"
            min={0}
            max={100}
            step={1}
            onChange={(value) =>
              updateColor("waterway_coverage_percent", value)
            }
          />
          <p className="control-hint">
            Keeps rivers and canals, then adds the longest streams up to this
            share of the print surface. Set 0% for major waterways only or
            100% for every mapped stream. Lakes stay unchanged.
          </p>
        </>
      )}
    </SurfaceSection>
  );
}
