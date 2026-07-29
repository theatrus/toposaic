import type { GenerationSpec } from "../contracts";
import { RangeField } from "./range-field";
import { SurfaceSection } from "./surface-section";
import type { UpdateColor } from "./surface-types";

export function SurfaceTerrainSection({
  spec,
  updateColor,
}: {
  spec: GenerationSpec;
  updateColor: UpdateColor;
}) {
  return (
    <SurfaceSection
      name="Terrain appearance"
      description="Set patch size and how mapped class borders follow the terrain. Colors are together in the Colors tab."
    >
      <RangeField
        label="Smallest color patch"
        value={spec.color_output.minimum_patch_mm}
        unit=" mm"
        min={0.4}
        max={4}
        step={0.2}
        onChange={(value) => updateColor("minimum_patch_mm", value)}
      />
      <RangeField
        label="Edge color bleed"
        value={spec.color_output.edge_bleed_mm}
        unit=" mm"
        min={0}
        max={2}
        step={0.1}
        onChange={(value) => updateColor("edge_bleed_mm", value)}
        note="How far the surface color carries down a piece's side wall before the rock cut face takes over. Without it every piece wears a grey outline seen from an angle. Set it to a whole number of print layers; zero turns it off."
      />
      <label className="road-detail-field">
        Terrain class borders
        <select
          value={spec.color_output.class_borders}
          onChange={(event) =>
            updateColor(
              "class_borders",
              event.target
                .value as GenerationSpec["color_output"]["class_borders"],
            )
          }
        >
          <option value="smooth">Smoothed where 10 m cells show</option>
          <option value="blocky">Blocky · raw 10 m cells</option>
        </select>
        <small>
          Smoothing bends forest, rock, and water borders into curves using
          the surrounding data. It starts at close views where single 10 m
          cells show; wider views keep the source data untouched.
        </small>
      </label>
      {spec.color_output.class_borders === "smooth" && (
        <>
          <RangeField
            label="Border bend range"
            value={spec.color_output.border_smoothing_range_cells}
            unit=" cells"
            min={1}
            max={8}
            step={0.5}
            onChange={(value) =>
              updateColor("border_smoothing_range_cells", value)
            }
            note="How far borders bend to follow nearby data, in 10 m land-cover cells."
          />
          <RangeField
            label="Border noise damping"
            value={spec.color_output.border_smoothing_nugget}
            unit=""
            min={0}
            max={0.5}
            step={0.01}
            onChange={(value) =>
              updateColor("border_smoothing_nugget", value)
            }
            note="Higher damping smooths stair steps but blurs single-cell features."
          />
        </>
      )}
      <div className="road-options">
        <label className="color-toggle">
          <input
            type="checkbox"
            checked={spec.color_output.forest_slope_gate}
            onChange={(event) =>
              updateColor("forest_slope_gate", event.target.checked)
            }
          />
          <span>Keep forest off steep rock</span>
        </label>
        <small>Demotes forest to rock above the slope limit.</small>
      </div>
      {spec.color_output.forest_slope_gate && (
        <>
          <RangeField
            label="Forest slope limit"
            value={spec.color_output.forest_slope_limit_degrees}
            unit="°"
            min={30}
            max={85}
            step={1}
            onChange={(value) =>
              updateColor("forest_slope_limit_degrees", value)
            }
          />
          <label className="road-detail-field">
            Steep forest becomes
            <select
              value={spec.color_output.steep_forest_target}
              onChange={(event) =>
                updateColor(
                  "steep_forest_target",
                  event.target
                    .value as GenerationSpec["color_output"]["steep_forest_target"],
                )
              }
            >
              <option value="rock">Rock</option>
              <option value="snow">Snow above the snowline</option>
            </select>
            <small>
              Snow keeps steep ground white above the mapped snowline. Below
              it, or without a snowcap, the ground becomes rock.
            </small>
          </label>
        </>
      )}
      <div className="road-options">
        <label className="color-toggle">
          <input
            type="checkbox"
            checked={spec.color_output.water_slope_gate}
            onChange={(event) =>
              updateColor("water_slope_gate", event.target.checked)
            }
          />
          <span>Keep water off cliffs</span>
        </label>
        <small>
          A sea is level and a lake surface is flat, so land-cover water on a
          steep face is a shoreline bleeding up a seawall. Mapped rivers and
          waterfalls are untouched — those really do run downhill.
        </small>
      </div>
      {spec.color_output.water_slope_gate && (
        <RangeField
          label="Water slope limit"
          value={spec.color_output.water_slope_limit_degrees}
          unit="°"
          min={5}
          max={85}
          step={1}
          onChange={(value) => updateColor("water_slope_limit_degrees", value)}
        />
      )}
      <div className="road-options">
        <label className="color-toggle">
          <input
            type="checkbox"
            checked={spec.color_output.snow_slope_gate}
            onChange={(event) =>
              updateColor("snow_slope_gate", event.target.checked)
            }
          />
          <span>Keep snow off sheer faces</span>
        </label>
        <small>Demotes snow to rock above the slope limit.</small>
      </div>
      {spec.color_output.snow_slope_gate && (
        <RangeField
          label="Snow slope limit"
          value={spec.color_output.snow_slope_limit_degrees}
          unit="°"
          min={30}
          max={85}
          step={1}
          onChange={(value) =>
            updateColor("snow_slope_limit_degrees", value)
          }
        />
      )}
    </SurfaceSection>
  );
}
