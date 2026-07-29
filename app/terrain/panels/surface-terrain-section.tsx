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
      <label className="road-detail-field">
        Ground colors
        <select
          value={spec.color_output.ground_colors}
          onChange={(event) =>
            updateColor(
              "ground_colors",
              event.target
                .value as GenerationSpec["color_output"]["ground_colors"],
            )
          }
        >
          <option value="mapped">Mapped classes · fixed colors</option>
          <option value="hybrid">Satellite shades inside classes</option>
          <option value="satellite">Satellite palette · imagery alone</option>
        </select>
        <small>
          The mapped classes each print one fixed color. The satellite modes
          discover a small palette from Sentinel-2 imagery of this area:
          hybrid keeps forest, ground, snow, and water apart and finds local
          shades inside each; pure satellite clusters the imagery alone. The
          discovered swatches appear in the generated job&apos;s data sources.
        </small>
      </label>
      {spec.color_output.ground_colors !== "mapped" && (
        <>
          <RangeField
            label="Ground color count"
            value={spec.color_output.ground_color_count}
            unit=" colors"
            min={2}
            max={8}
            step={1}
            onChange={(value) => updateColor("ground_color_count", value)}
            note="How many ground colors discovery may keep. Hybrid guarantees every present class an entry, so four classes can exceed a request of two or three."
          />
          <RangeField
            label="Smallest color share"
            value={spec.color_output.ground_color_minimum_share * 100}
            unit="%"
            min={0}
            max={25}
            step={1}
            onChange={(value) =>
              updateColor("ground_color_minimum_share", value / 100)
            }
            note="Colors covering less than this merge into the nearest one instead of taking a filament slot."
          />
          <RangeField
            label="Shadow flattening"
            value={spec.color_output.ground_shadow_normalization}
            unit=""
            min={0}
            max={1}
            step={0.05}
            onChange={(value) =>
              updateColor("ground_shadow_normalization", value)
            }
            note="How strongly terrain shadows are evened out before colors are chosen. At zero a shadowed hillside can become its own darker color."
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
          <span>Keep land-cover water off walls</span>
        </label>
        <small>Takes satellite water off printed slopes it cannot hold.</small>
      </div>
      {spec.color_output.water_slope_gate && (
        <RangeField
          label="Land-cover water slope limit"
          value={spec.color_output.water_slope_limit_degrees}
          unit="°"
          min={5}
          max={85}
          step={1}
          onChange={(value) => updateColor("water_slope_limit_degrees", value)}
          note="Printed degrees, not ground degrees: a sea is level, so water climbing a face is a shoreline bleeding up a seawall, and vertical exaggeration is what builds the face it climbs. Rivers and waterfalls are untouched — those really do run downhill."
        />
      )}
      <div className="road-options">
        <label className="color-toggle">
          <input
            type="checkbox"
            checked={spec.color_output.osm_water_slope_gate}
            onChange={(event) =>
              updateColor("osm_water_slope_gate", event.target.checked)
            }
          />
          <span>Keep mapped water off walls</span>
        </label>
        <small>The same physics for OpenStreetMap seas and lakes.</small>
      </div>
      {spec.color_output.osm_water_slope_gate && (
        <RangeField
          label="Mapped water slope limit"
          value={spec.color_output.osm_water_slope_limit_degrees}
          unit="°"
          min={5}
          max={85}
          step={1}
          onChange={(value) =>
            updateColor("osm_water_slope_limit_degrees", value)
          }
          note="Printed degrees, for the water polygons most coastlines are made of. Its own switch because the sources err differently: a mapped shoreline is usually exact, and climbs only where the elevation data and the mapping disagree."
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
