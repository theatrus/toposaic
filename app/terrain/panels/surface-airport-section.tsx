import type { GenerationSpec } from "../contracts";
import { RangeField } from "./range-field";
import { SurfaceSection } from "./surface-section";
import type { UpdateColor } from "./surface-types";

export function SurfaceAirportSection({
  spec,
  updateColor,
}: {
  spec: GenerationSpec;
  updateColor: UpdateColor;
}) {
  const aviation = spec.color_output;
  const denseDetailDropped =
    spec.ground_span_km > aviation.aviation_detail_span_km;

  return (
    <SurfaceSection
      name="Airport surfaces"
      description="Draw runways, taxiways, aprons, and helipads from OpenStreetMap as raised pavement."
    >
      <div className="road-options">
        <label className="color-toggle">
          <input
            type="checkbox"
            checked={aviation.aviation_enabled}
            onChange={(event) =>
              updateColor("aviation_enabled", event.target.checked)
            }
          />
          <span>Render airport surfaces</span>
        </label>
        <small>
          Off to start. An airport is something you go looking for, and a map
          that clips the edge of one should not sprout a runway.
        </small>
      </div>
      {aviation.aviation_enabled && (
        <>
          <div
            className="road-options"
            role="group"
            aria-label="Airport feature groups"
          >
            <strong>Draw</strong>
            <label className="color-toggle">
              <input
                type="checkbox"
                checked={aviation.aviation_runways_enabled}
                onChange={(event) =>
                  updateColor("aviation_runways_enabled", event.target.checked)
                }
              />
              <span>Runways and airstrips</span>
            </label>
            <label className="color-toggle">
              <input
                type="checkbox"
                checked={aviation.aviation_taxiways_enabled}
                onChange={(event) =>
                  updateColor("aviation_taxiways_enabled", event.target.checked)
                }
              />
              <span>Taxiways and taxilanes</span>
            </label>
            <label className="color-toggle">
              <input
                type="checkbox"
                checked={aviation.aviation_aprons_enabled}
                onChange={(event) =>
                  updateColor("aviation_aprons_enabled", event.target.checked)
                }
              />
              <span>Aprons</span>
            </label>
            <label className="color-toggle">
              <input
                type="checkbox"
                checked={aviation.aviation_helipads_enabled}
                onChange={(event) =>
                  updateColor("aviation_helipads_enabled", event.target.checked)
                }
              />
              <span>Helipads</span>
            </label>
            <small>
              Each group is fetched only when it is on, and all of them share
              one color and one filament slot.
            </small>
          </div>
          <label className="road-detail-field">
            Airport surface style
            <select
              aria-label="Airport surface style"
              value={aviation.aviation_style}
              onChange={(event) =>
                updateColor(
                  "aviation_style",
                  event.target
                    .value as GenerationSpec["color_output"]["aviation_style"],
                )
              }
            >
              <option value="separate">Own color</option>
              <option value="follow_roads">Draw with roads</option>
            </select>
            <small>
              A separate color uses a filament slot only where the map really
              has pavement. Drawing with roads adds no slot.
            </small>
          </label>
          <RangeField
            label="Airport surface height"
            value={aviation.aviation_height_mm}
            unit=" mm"
            min={0.08}
            max={1}
            step={0.02}
            onChange={(value) => updateColor("aviation_height_mm", value)}
            note="How far pavement stands above the terrain. Set it to a whole number of the layers you slice at."
          />
          <RangeField
            label="Maximum airport width"
            value={aviation.maximum_aviation_width_mm}
            unit=" mm"
            min={0.4}
            max={12}
            step={0.1}
            onChange={(value) =>
              updateColor("maximum_aviation_width_mm", value)
            }
            note="A 60 m runway close in is a correct reading of the data and still wider than anyone wants across the model."
          />
          <RangeField
            label="Small feature cutoff"
            value={aviation.aviation_detail_span_km}
            unit=" km"
            min={1}
            max={80}
            step={1}
            onChange={(value) => updateColor("aviation_detail_span_km", value)}
            note="Ground span past which helipads are dropped before they are even requested."
          />
          {denseDetailDropped && aviation.aviation_helipads_enabled && (
            <small className="road-options">
              This map spans {spec.ground_span_km} km, so helipads are being
              left out. Raise the cutoff to draw them anyway.
            </small>
          )}
        </>
      )}
    </SurfaceSection>
  );
}
