import type { GenerationSpec } from "../contracts";
import { RangeField } from "./range-field";
import { SurfaceSection } from "./surface-section";
import type { UpdateColor, UpdateMarine } from "./surface-types";

export function SurfaceWaterSection({
  spec,
  updateColor,
  updateMarine,
}: {
  spec: GenerationSpec;
  updateColor: UpdateColor;
  updateMarine: UpdateMarine;
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
      <label className="road-detail-field">
        Sea surface
        <select
          value={spec.marine.geometry}
          onChange={(event) =>
            updateMarine(
              "geometry",
              event.target.value as GenerationSpec["marine"]["geometry"],
            )
          }
        >
          <option value="bathymetric_relief">
            Bathymetric relief · draped source depths
          </option>
          <option value="flat_surface">Flat surface at a water level</option>
        </select>
        <small>
          A real sea is level, but the elevation source holds seabed under
          it. Flat finds the water connected to the open sea at the map edge
          — checked against OpenStreetMap coastlines — and levels it. Lakes
          keep their own height.
        </small>
      </label>
      {spec.marine.geometry === "flat_surface" && (
        <>
          <label className="road-detail-field">
            Water level
            <select
              value={spec.marine.level}
              onChange={(event) =>
                updateMarine(
                  "level",
                  event.target.value as GenerationSpec["marine"]["level"],
                )
              }
            >
              <option value="msl">Mean sea level</option>
              <option value="low_tide">Low tide (MLLW)</option>
              <option value="high_tide">High tide (MHHW)</option>
              <option value="custom">Custom offset</option>
            </select>
            <small>
              Low and high tide use the nearest NOAA tide station's datums
              (MLLW and MHHW), which cover United States coasts. Elsewhere
              they fall back to mean sea level, and the data sources name
              the station or say why not.
            </small>
          </label>
          {spec.marine.level === "custom" && (
            <RangeField
              label="Custom level"
              value={spec.marine.custom_offset_m}
              unit=" m"
              min={-10}
              max={10}
              step={0.5}
              onChange={(value) => updateMarine("custom_offset_m", value)}
              note="Metres above the elevation source's zero. Below zero dries out the foreshore down to the level; above zero covers sea-connected land under it."
            />
          )}
        </>
      )}
    </SurfaceSection>
  );
}
