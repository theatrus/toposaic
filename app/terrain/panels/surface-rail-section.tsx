import type { GenerationSpec } from "../contracts";
import { RangeField } from "./range-field";
import { SurfaceSection } from "./surface-section";
import type { UpdateColor } from "./surface-types";

export function SurfaceRailSection({
  spec,
  updateColor,
}: {
  spec: GenerationSpec;
  updateColor: UpdateColor;
}) {
  return (
    <SurfaceSection
      name="Railways and lifts"
      description="Draw ground rail and cable lifts apart from roads, or fold them into another color layer."
    >
      <div className="road-options">
        <label className="color-toggle">
          <input
            type="checkbox"
            checked={spec.color_output.rail_enabled}
            onChange={(event) =>
              updateColor("rail_enabled", event.target.checked)
            }
          />
          <span>Render railways</span>
        </label>
        <small>Includes trains, trams, metros, narrow gauge, monorails, and funiculars. Tunnels are skipped.</small>
      </div>
      {spec.color_output.rail_enabled && (
        <>
          <label className="road-detail-field">
            Railway style
            <select
              aria-label="Railway style"
              value={spec.color_output.rail_style}
              onChange={(event) =>
                updateColor(
                  "rail_style",
                  event.target
                    .value as GenerationSpec["color_output"]["rail_style"],
                )
              }
            >
              <option value="separate">Own color</option>
              <option value="with_roads">Draw with roads</option>
            </select>
            <small>
              A separate color uses a filament slot only where the map has a
              railway. Drawing with roads adds no slot.
            </small>
          </label>
          {spec.color_output.rail_style === "separate" && (
            <>
              <div className="color-swatches rail-color-swatch">
                <label>
                  <input
                    aria-label="Railway color"
                    type="color"
                    value={spec.color_output.rail_color}
                    onChange={(event) =>
                      updateColor("rail_color", event.target.value)
                    }
                  />
                  <span>Railway color</span>
                  <code>{spec.color_output.rail_color.toUpperCase()}</code>
                </label>
              </div>
              <RangeField
                label="Railway print width"
                value={spec.color_output.rail_width_mm}
                unit=" mm"
                min={0.4}
                max={4}
                step={0.1}
                onChange={(value) => updateColor("rail_width_mm", value)}
              />
            </>
          )}
        </>
      )}
      <div className="road-options">
        <label className="color-toggle">
          <input
            type="checkbox"
            checked={spec.color_output.aerial_enabled}
            onChange={(event) =>
              updateColor("aerial_enabled", event.target.checked)
            }
          />
          <span>Render aerial lifts</span>
        </label>
        <small>
          Includes cable cars, gondolas, chair lifts, drag lifts, and rope
          tows. Funiculars run on the ground, so they count as railways.
        </small>
      </div>
      {spec.color_output.aerial_enabled && (
        <>
          <label className="road-detail-field">
            Aerial lift style
            <select
              aria-label="Aerial lift style"
              value={spec.color_output.aerial_style}
              onChange={(event) =>
                updateColor(
                  "aerial_style",
                  event.target
                    .value as GenerationSpec["color_output"]["aerial_style"],
                )
              }
            >
              <option value="separate">Own color</option>
              <option value="with_rail">Draw with railways</option>
              <option value="with_roads">Draw with roads</option>
            </select>
            <small>
              {spec.color_output.aerial_style === "with_rail"
                ? spec.color_output.rail_enabled
                  ? "Lifts follow the railway style."
                  : "Railways are off, so lifts use the road color until railways return."
                : "A separate color uses a slot only where mapped lifts exist; sharing a layer adds no slot."}
            </small>
          </label>
          {spec.color_output.aerial_style === "separate" && (
            <>
              <div className="color-swatches aerial-color-swatch">
                <label>
                  <input
                    aria-label="Aerial lift color"
                    type="color"
                    value={spec.color_output.aerial_color}
                    onChange={(event) =>
                      updateColor("aerial_color", event.target.value)
                    }
                  />
                  <span>Aerial lift color</span>
                  <code>{spec.color_output.aerial_color.toUpperCase()}</code>
                </label>
              </div>
              <RangeField
                label="Aerial lift print width"
                value={spec.color_output.aerial_width_mm}
                unit=" mm"
                min={0.4}
                max={4}
                step={0.1}
                onChange={(value) => updateColor("aerial_width_mm", value)}
              />
            </>
          )}
        </>
      )}
      {(spec.color_output.rail_enabled || spec.color_output.aerial_enabled) && (
        <label className="road-detail-field">
          Railway and lift history
          <select
            aria-label="Railway and lift history"
            value={spec.color_output.rail_lifecycle}
            onChange={(event) =>
              updateColor(
                "rail_lifecycle",
                event.target
                  .value as GenerationSpec["color_output"]["rail_lifecycle"],
              )
            }
          >
            <option value="operational">In service only</option>
            <option value="disused">Add disused · track and cables remain</option>
            <option value="abandoned">Add abandoned · formation visible</option>
          </select>
          <small>
            Razed, dismantled, demolished, proposed, and planned lines never print.
          </small>
        </label>
      )}
    </SurfaceSection>
  );
}
