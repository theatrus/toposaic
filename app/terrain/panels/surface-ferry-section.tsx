import type { GenerationSpec } from "../contracts";
import { RangeField } from "./range-field";
import { SurfaceSection } from "./surface-section";
import type { UpdateColor } from "./surface-types";

export function SurfaceFerrySection({
  spec,
  updateColor,
}: {
  spec: GenerationSpec;
  updateColor: UpdateColor;
}) {
  return (
    <SurfaceSection
      name="Ferries"
      description="Draw mapped ferry crossings across the water, in their own color or with the roads."
    >
      <div className="road-options">
        <label className="color-toggle">
          <input
            type="checkbox"
            checked={spec.color_output.ferry_enabled}
            onChange={(event) =>
              updateColor("ferry_enabled", event.target.checked)
            }
          />
          <span>Render ferries</span>
        </label>
        <small>
          Every route OpenStreetMap tags as a ferry crossing, drawn as a
          raised line over the water like a road.
        </small>
      </div>
      {spec.color_output.ferry_enabled && (
        <>
          <label className="road-detail-field">
            Ferry style
            <select
              aria-label="Ferry style"
              value={spec.color_output.ferry_style}
              onChange={(event) =>
                updateColor(
                  "ferry_style",
                  event.target
                    .value as GenerationSpec["color_output"]["ferry_style"],
                )
              }
            >
              <option value="separate">Own color</option>
              <option value="with_roads">Draw with roads</option>
            </select>
            <small>
              A separate color uses a filament slot only where the map has a
              crossing. Drawing with roads adds no slot.
            </small>
          </label>
          {spec.color_output.ferry_style === "separate" && (
            <RangeField
              label="Ferry print width"
              value={spec.color_output.ferry_width_mm}
              unit=" mm"
              min={0.4}
              max={4}
              step={0.1}
              onChange={(value) => updateColor("ferry_width_mm", value)}
            />
          )}
        </>
      )}
    </SurfaceSection>
  );
}
