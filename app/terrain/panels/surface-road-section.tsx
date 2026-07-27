import {
  LINE_SCALE_CLOSE_SPAN_KM,
  LINE_SCALE_WIDE_SPAN_KM,
  automaticRoadDetail,
  closeViewLineScale,
  minimumMappedWidthCap,
} from "../config";
import type { GenerationSpec } from "../contracts";
import { RangeField } from "./range-field";
import { SurfaceSection } from "./surface-section";
import type { UpdateColor } from "./surface-types";

export function SurfaceRoadSection({
  spec,
  updateColor,
}: {
  spec: GenerationSpec;
  updateColor: UpdateColor;
}) {
  const currentScale = closeViewLineScale(
    spec.ground_span_km,
    spec.color_output.scale_line_widths_by_span,
    spec.color_output.close_view_width_multiplier,
  );
  const mappedWidthFloor = minimumMappedWidthCap(spec.color_output);

  return (
    <SurfaceSection
      name="Roads and bridges"
      description="Choose which routes print, their base size, and how bridges cross the terrain."
    >
      <div className="road-options">
        <label className="color-toggle">
          <input
            type="checkbox"
            checked={spec.color_output.roads_enabled}
            onChange={(event) =>
              updateColor("roads_enabled", event.target.checked)
            }
          />
          <span>Render roads</span>
        </label>
        <small>Falls back to trails when no roads cross the map.</small>
      </div>
      {spec.color_output.roads_enabled && (
        <>
          <label className="road-detail-field">
            Route detail
            <select
              value={spec.color_output.road_detail}
              onChange={(event) =>
                updateColor(
                  "road_detail",
                  event.target
                    .value as GenerationSpec["color_output"]["road_detail"],
                )
              }
            >
              <option value="automatic">Automatic for map span</option>
              <option value="major">Major roads only</option>
              <option value="minor">Major and minor roads</option>
              <option value="streets">Roads and local streets</option>
              <option value="all">Streets, paths, and trails</option>
            </select>
            <small>
              {spec.color_output.road_detail === "automatic"
                ? `At ${spec.ground_span_km.toLocaleString()} km, automatic mode includes ${automaticRoadDetail(
                    spec.ground_span_km,
                  )}.`
                : "The chosen detail applies at every map span."}
            </small>
          </label>
          <RangeField
            label="Route minimum width"
            value={spec.color_output.road_width_mm}
            unit=" mm"
            min={0.4}
            max={4}
            step={0.1}
            onChange={(value) => updateColor("road_width_mm", value)}
          />
        </>
      )}
      <div className="road-options">
        <label className="color-toggle">
          <input
            type="checkbox"
            checked={spec.color_output.scale_line_widths_by_span}
            onChange={(event) =>
              updateColor("scale_line_widths_by_span", event.target.checked)
            }
          />
          <span>Boost lines without mapped widths at close views</span>
        </label>
        <small>Strengthens roads with no usable width tag and aerial lifts as the mapped area shrinks.</small>
      </div>
      {spec.color_output.scale_line_widths_by_span && (
        <RangeField
          label="Close-view line boost"
          value={spec.color_output.close_view_width_multiplier}
          unit="×"
          min={1}
          max={3}
          step={0.1}
          onChange={(value) =>
            updateColor("close_view_width_multiplier", value)
          }
          note={`Major roads without a mapped width and aerial lifts use ${currentScale.toFixed(2)}× at ${spec.ground_span_km.toLocaleString()} km. Full boost applies at ${LINE_SCALE_CLOSE_SPAN_KM} km and below; it fades to 1× at ${LINE_SCALE_WIDE_SPAN_KM} km.`}
        />
      )}
      <RangeField
        label="Maximum mapped width"
        value={spec.color_output.maximum_mapped_width_mm}
        unit=" mm"
        min={mappedWidthFloor}
        max={8}
        step={0.1}
        onChange={(value) => updateColor("maximum_mapped_width_mm", value)}
        note={`Caps physical OSM widths after scaling. Active route and railway minimums set a ${mappedWidthFloor.toFixed(1)} mm floor; the app raises this cap when those minimums grow.`}
      />
      {spec.color_output.roads_enabled && (
        <>
          <div className="road-options">
            <label className="color-toggle">
              <input
                type="checkbox"
                checked={spec.color_output.adaptive_road_widths}
                onChange={(event) =>
                  updateColor("adaptive_road_widths", event.target.checked)
                }
              />
              <span>Thin estimated widths in dense networks</span>
            </label>
            <small>Mapped physical widths stay at real scale and count toward the budget; estimated widths shrink as coverage rises.</small>
          </div>
          <RangeField
            label="Road layer height"
            value={spec.color_output.road_height_mm}
            unit=" mm"
            min={0.08}
            max={0.4}
            step={0.02}
            onChange={(value) => updateColor("road_height_mm", value)}
          />
          <div
            className="road-options bridge-options"
            role="group"
            aria-label="Bridge structure"
          >
            <strong>Bridge structure</strong>
            <label className="color-toggle">
              <input
                type="radio"
                name="bridge-structure"
                checked={spec.color_output.bridge_structure === "floating"}
                onChange={() => updateColor("bridge_structure", "floating")}
              />
              <span>Floating</span>
            </label>
            <small>Uses a thick deck between the abutments.</small>
            <label className="color-toggle">
              <input
                type="radio"
                name="bridge-structure"
                checked={spec.color_output.bridge_structure === "supported"}
                onChange={() => updateColor("bridge_structure", "supported")}
              />
              <span>Fully supported</span>
            </label>
            <small>Fills from the deck down to the mapped ground or water.</small>
          </div>
          {spec.color_output.bridge_structure === "floating" && (
            <RangeField
              label="Floating bridge thickness"
              value={spec.color_output.bridge_thickness_mm}
              unit=" mm"
              min={0.4}
              max={6}
              step={0.2}
              onChange={(value) =>
                updateColor("bridge_thickness_mm", value)
              }
            />
          )}
        </>
      )}
    </SurfaceSection>
  );
}
