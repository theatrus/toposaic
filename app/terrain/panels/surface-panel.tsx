import { automaticRoadDetail } from "../config";
import type { GenerationSpec } from "../contracts";
import { RangeField } from "./range-field";

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
  updateColor: <Key extends keyof GenerationSpec["color_output"]>(
    key: Key,
    value: GenerationSpec["color_output"][Key],
  ) => void;
}) {
  return (
    <fieldset
      className="color-controls control-section surface-controls"
      aria-label="Surface colors"
      hidden={hidden}
    >
      <div className="color-heading">
        <div>
          <strong className="color-title">Surface colors</strong>
          <p>Paint the 3MF from mapped land cover and routes.</p>
        </div>
        <label className="color-toggle">
          <input
            type="checkbox"
            checked={spec.color_output.enabled}
            onChange={(event) =>
              updateColor("enabled", event.target.checked)
            }
          />
          <span>{spec.color_output.enabled ? "On" : "Off"}</span>
        </label>
      </div>
      {spec.color_output.enabled && (
        <>
          <div className="color-swatches">
            {(
              [
                ["Forest", "forest_color"],
                ["Rock", "rock_color"],
                ["Snow", "snow_color"],
                ["Water", "water_color"],
                ["Route", "road_color"],
              ] as const
            ).map(([label, key]) => (
              <label key={key}>
                <input
                  type="color"
                  value={spec.color_output[key]}
                  onChange={(event) => updateColor(key, event.target.value)}
                />
                <span>{label}</span>
                <code>{spec.color_output[key].toUpperCase()}</code>
              </label>
            ))}
          </div>
          <RangeField
            label="Smallest color patch"
            value={spec.color_output.minimum_patch_mm}
            unit=" mm"
            min={0.4}
            max={4}
            step={0.2}
            onChange={(value) => updateColor("minimum_patch_mm", value)}
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
              Smoothing bends forest, rock, and water borders into curves
              using the surrounding data. It engages on its own at close
              views, where single 10 m cells are visible; wider views keep
              the source data untouched either way.
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
                note="Higher damping smooths staircase artifacts but blurs single-cell features."
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
            <small>
              WorldCover bleeds tree cover onto cliff faces. This demotes
              forest to rock above the slope limit.
            </small>
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
                  Snow keeps demoted forest white above the snowline,
                  estimated from the mapped snowcap. Below it, or without
                  a snowcap, demoted forest still prints as rock.
                </small>
              </label>
            </>
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
            <small>
              WorldCover bleeds snow onto cliff walls. This demotes snow
              to rock above the slope limit.
            </small>
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
            <small>
              Adds smooth rivers, streams, canals, and mapped water areas
            </small>
          </div>
          {spec.color_output.osm_water_enabled && (
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
          )}
          {spec.color_output.osm_water_enabled && (
            <p className="control-hint">
              Keeps rivers and canals, then adds the longest streams up to
              this share of the print surface. Set 0% for major waterways
              only or 100% for every mapped stream. Lakes are unchanged.
            </p>
          )}
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
            <small>Falls back to trails when no roads cross the map</small>
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
                  <option value="automatic">
                    Automatic for map span
                  </option>
                  <option value="major">Major roads only</option>
                  <option value="minor">
                    Major and minor roads
                  </option>
                  <option value="streets">
                    Roads and local streets
                  </option>
                  <option value="all">
                    Streets, paths, and trails
                  </option>
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
                label="Route print width"
                value={spec.color_output.road_width_mm}
                unit=" mm"
                min={0.4}
                max={4}
                step={0.1}
                onChange={(value) => updateColor("road_width_mm", value)}
              />
              <div className="road-options">
                <label className="color-toggle">
                  <input
                    type="checkbox"
                    checked={spec.color_output.adaptive_road_widths}
                    onChange={(event) =>
                      updateColor(
                        "adaptive_road_widths",
                        event.target.checked,
                      )
                    }
                  />
                  <span>Thin dense road networks</span>
                </label>
                <small>
                  Reduces route width as mapped road coverage rises. It
                  does not remove road classes.
                </small>
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
                    checked={
                      spec.color_output.bridge_structure === "floating"
                    }
                    onChange={() =>
                      updateColor("bridge_structure", "floating")
                    }
                  />
                  <span>Floating</span>
                </label>
                <small>Uses a thick deck between the abutments</small>
                <label className="color-toggle">
                  <input
                    type="radio"
                    name="bridge-structure"
                    checked={
                      spec.color_output.bridge_structure === "supported"
                    }
                    onChange={() =>
                      updateColor("bridge_structure", "supported")
                    }
                  />
                  <span>Fully supported</span>
                </label>
                <small>
                  Fills from the deck down to the mapped ground or water
                </small>
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
          <p className="color-note">
            WorldCover supplies permanent water. OpenStreetMap waterways
            add smooth lakes, rivers, streams, and canals when enabled.
            Routes come from OpenStreetMap. The generator uses prominent
            roads first, then trails only when no roads cross the model.
            Tagged bridges can use thick floating decks or solid support
            down to mapped ground or water. Untagged routes follow the
            terrain. Tunnels stay hidden. The road layer height controls
            the colored top surface, not bridge thickness. Snow is not
            live. Sides and bottoms use the rock color.
          </p>
        </>
      )}
      <div
        className="road-options trail-import"
        role="group"
        aria-label="Imported trails"
      >
        <strong>Imported trails</strong>
        <label className="trail-import-input">
          Import GPX or KML files
          <input
            aria-label="Import trail files"
            type="file"
            accept=".gpx,.kml"
            multiple
            onChange={(event) => {
              const files = Array.from(event.target.files ?? []);
              event.target.value = "";
              void importTrailFiles(files);
            }}
          />
        </label>
        {spec.trails.length > 0 && (
          <ul className="trail-list">
            {spec.trails.map((trail, index) => (
              // Content-based keys stay stable when a trail is removed
              // from the middle of the list.
              <li
                key={`${trail.name}:${trail.points.length}:${
                  trail.points[0]?.join(",") ?? ""
                }`}
              >
                <span>{trail.name}</span>
                <small>
                  {trail.points.length.toLocaleString()} points
                </small>
                <button
                  type="button"
                  aria-label={`Remove trail ${trail.name}`}
                  onClick={() => removeTrail(index)}
                >
                  Remove
                </button>
              </li>
            ))}
          </ul>
        )}
        {trailNotice && (
          <small aria-live="polite" className="trail-notice" role="status">
            {trailNotice}
          </small>
        )}
        {spec.trails.length > 0 && (
          <>
            <div className="color-swatches trail-color-swatch">
              <label>
                <input
                  aria-label="Trail color"
                  type="color"
                  value={spec.color_output.trail_color}
                  onChange={(event) =>
                    updateColor("trail_color", event.target.value)
                  }
                />
                <span>Trail color</span>
                <code>
                  {spec.color_output.trail_color.toUpperCase()}
                </code>
              </label>
            </div>
            <RangeField
              label="Trail print width"
              value={spec.color_output.trail_width_mm}
              unit=" mm"
              min={0.4}
              max={4}
              step={0.1}
              onChange={(value) => updateColor("trail_width_mm", value)}
            />
          </>
        )}
        <small>
          Each hiking route from a GPX or KML file prints as a raised
          line in its own trail color, like roads. Trails live in the
          model spec, so saved setups and exported setup files carry
          them.
        </small>
      </div>
    </fieldset>
  );
}
