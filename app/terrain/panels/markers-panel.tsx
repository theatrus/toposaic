import type { GenerationSpec, MarkerKind } from "../contracts";
import { isFlagMarker, limitMarkerName } from "../config";
import { RangeField } from "./range-field";

const kindLabel: Record<MarkerKind, string> = {
  building: "Highlight building",
  dot: "Color dot",
  flag_hole: "Blank flag",
  flag_label: "Named flag",
};

function MarkerCoordinateInput({
  label,
  max,
  min,
  onCommit,
  value,
}: {
  label: string;
  max: number;
  min: number;
  onCommit: (value: number) => void;
  value: number;
}) {
  return (
    <label>
      <span>{label}</span>
      <input
        aria-label={label}
        inputMode="decimal"
        max={max}
        min={min}
        onChange={(event) => {
          const next = event.target.valueAsNumber;
          if (Number.isFinite(next) && next >= min && next <= max) {
            onCommit(next);
          }
        }}
        onKeyDown={(event) => {
          if (event.key === "Enter") {
            event.preventDefault();
            event.currentTarget.blur();
          }
        }}
        step="any"
        type="number"
        value={value}
      />
    </label>
  );
}

export function MarkersPanel({
  hidden,
  placementKind,
  removeMarker,
  setPlacementKind,
  spec,
  updateMarker,
  updateMarkerSettings,
}: {
  hidden: boolean;
  placementKind: MarkerKind | null;
  removeMarker: (index: number) => void;
  setPlacementKind: (kind: MarkerKind | null) => void;
  spec: GenerationSpec;
  updateMarker: (
    index: number,
    patch: Partial<GenerationSpec["markers"][number]>,
  ) => void;
  updateMarkerSettings: <Key extends keyof GenerationSpec["marker_settings"]>(
    key: Key,
    value: GenerationSpec["marker_settings"][Key],
  ) => void;
}) {
  const showFlagControls =
    (placementKind !== null && isFlagMarker(placementKind)) ||
    spec.markers.some((marker) => isFlagMarker(marker.kind));

  return (
    <fieldset
      aria-label="Map markers"
      className="control-section marker-controls"
      hidden={hidden}
    >
      <div className="color-heading">
        <div>
          <strong className="color-title">Map markers</strong>
          <p>Choose a marker, then click its place on the map.</p>
        </div>
      </div>

      <div className="marker-kind-buttons" role="group" aria-label="Marker type">
        {(Object.keys(kindLabel) as MarkerKind[]).map((kind) => (
          <button
            aria-pressed={placementKind === kind}
            className={placementKind === kind ? "active" : ""}
            key={kind}
            onClick={() =>
              setPlacementKind(placementKind === kind ? null : kind)
            }
            type="button"
          >
            {kindLabel[kind]}
          </button>
        ))}
      </div>
      <p className="color-note" role="status">
        {placementKind
          ? `Click the map to place a ${kindLabel[placementKind].toLowerCase()}.`
          : "Building markers color the footprint under the point. Dots print flush with the terrain. Flag holes use a blank flag; flag labels print the editable marker name."}
      </p>

      {spec.markers.length > 0 && (
        <ul className="marker-list">
          {spec.markers.map((marker, index) => (
            <li key={`${marker.latitude}:${marker.longitude}:${index}`}>
              <input
                aria-label={`Marker ${index + 1} name`}
                onChange={(event) =>
                  updateMarker(index, {
                    name: limitMarkerName(event.target.value),
                  })
                }
                value={marker.name}
              />
              {marker.kind === "flag_label" && (
                <small>This name prints on the flag.</small>
              )}
              <select
                aria-label={`Marker ${index + 1} type`}
                onChange={(event) =>
                  updateMarker(index, {
                    kind: event.target.value as MarkerKind,
                  })
                }
                value={marker.kind}
              >
                {(Object.keys(kindLabel) as MarkerKind[]).map((kind) => (
                  <option key={kind} value={kind}>
                    {kindLabel[kind]}
                  </option>
                ))}
              </select>
              <div className="marker-coordinates">
                <MarkerCoordinateInput
                  label={`Marker ${index + 1} latitude`}
                  max={90}
                  min={-90}
                  onCommit={(latitude) => updateMarker(index, { latitude })}
                  value={marker.latitude}
                />
                <MarkerCoordinateInput
                  label={`Marker ${index + 1} longitude`}
                  max={180}
                  min={-180}
                  onCommit={(longitude) => updateMarker(index, { longitude })}
                  value={marker.longitude}
                />
              </div>
              <button
                aria-label={`Remove marker ${marker.name}`}
                onClick={() => removeMarker(index)}
                type="button"
              >
                Remove
              </button>
            </li>
          ))}
        </ul>
      )}

      <RangeField
        label="Dot diameter"
        max={10}
        min={1}
        onChange={(value) => updateMarkerSettings("dot_diameter_mm", value)}
        step={0.2}
        unit="mm"
        value={spec.marker_settings.dot_diameter_mm}
      />
      {showFlagControls && (
        <>
          <RangeField
            label="Flag-hole diameter"
            max={6}
            min={1.2}
            onChange={(value) =>
              updateMarkerSettings("hole_diameter_mm", value)
            }
            step={0.2}
            unit="mm"
            value={spec.marker_settings.hole_diameter_mm}
          />
          <RangeField
            label="Flag-hole depth"
            max={Math.min(6, Math.max(0.6, spec.base_mm - 0.4))}
            min={0.6}
            onChange={(value) =>
              updateMarkerSettings("hole_depth_mm", value)
            }
            step={0.2}
            unit="mm"
            value={Math.min(
              spec.marker_settings.hole_depth_mm,
              Math.max(0.6, spec.base_mm - 0.4),
            )}
          />
          <RangeField
            label="Flag fit clearance"
            max={Math.min(
              0.6,
              Math.max(0.1, spec.marker_settings.hole_diameter_mm - 0.9),
            )}
            min={0.1}
            onChange={(value) =>
              updateMarkerSettings("flag_clearance_mm", value)
            }
            step={0.05}
            unit="mm"
            value={spec.marker_settings.flag_clearance_mm}
          />
          <label className="marker-flag-font-field">
            <span>Flag label font</span>
            <select
              aria-label="Flag label font"
              onChange={(event) =>
                updateMarkerSettings(
                  "flag_label_font",
                  event.target
                    .value as GenerationSpec["marker_settings"]["flag_label_font"],
                )
              }
              value={spec.marker_settings.flag_label_font}
            >
              <option value="atkinson_hyperlegible">
                Atkinson Hyperlegible
              </option>
              <option value="noto_sans">Noto Sans</option>
              <option value="b612_mono">B612 Mono</option>
            </select>
          </label>
          <RangeField
            label="Flag width"
            max={80}
            min={12}
            onChange={(value) =>
              updateMarkerSettings("flag_width_mm", value)
            }
            step={1}
            unit="mm"
            value={spec.marker_settings.flag_width_mm}
          />
          <RangeField
            label="Flag height"
            max={30}
            min={6}
            onChange={(value) =>
              updateMarkerSettings("flag_height_mm", value)
            }
            step={1}
            unit="mm"
            value={spec.marker_settings.flag_height_mm}
          />
          <RangeField
            label="Flag label height"
            max={Math.min(10, spec.marker_settings.flag_height_mm - 2)}
            min={1.5}
            note="Long names shrink to fit the banner."
            onChange={(value) =>
              updateMarkerSettings("flag_label_height_mm", value)
            }
            step={0.1}
            unit="mm"
            value={Math.min(
              spec.marker_settings.flag_label_height_mm,
              spec.marker_settings.flag_height_mm - 2,
            )}
          />
          <label className="marker-template-toggle">
            <input
              checked={spec.marker_settings.export_flag_template}
              onChange={(event) =>
                updateMarkerSettings(
                  "export_flag_template",
                  event.target.checked,
                )
              }
              type="checkbox"
            />
            Export printable blank and named flags
          </label>
        </>
      )}
    </fieldset>
  );
}
