import type { GenerationSpec, MarkerKind } from "../contracts";
import { isFlagMarker, isMapLabel, limitMarkerName } from "../config";
import { LabelFontSelect } from "./label-font-select";
import { RangeField } from "./range-field";

const kindLabel: Record<MarkerKind, string> = {
  building: "Highlight building",
  dot: "Color dot",
  flag_hole: "Blank flag",
  flag_label: "Named flag",
  surface_label: "Surface label",
  plaque_label: "Raised plaque",
};

type MapLabelStyle = NonNullable<
  GenerationSpec["markers"][number]["label_style"]
>;

function resolvedLabelStyle(
  marker: GenerationSpec["markers"][number],
  defaults: GenerationSpec["marker_settings"],
): MapLabelStyle {
  return (
    marker.label_style ?? {
      relief_mm: defaults.map_label_relief_mm,
      plaque_padding_mm: defaults.plaque_padding_mm,
      plaque_thickness_mm: defaults.plaque_thickness_mm,
    }
  );
}

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
  movingMarkerIndex,
  moveMarker,
  placementKind,
  removeMarker,
  setPlacementKind,
  spec,
  updateMarker,
  updateMarkerSettings,
}: {
  hidden: boolean;
  movingMarkerIndex: number | null;
  moveMarker: (index: number) => void;
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
  const showMapLabelControls =
    (placementKind !== null && isMapLabel(placementKind)) ||
    spec.markers.some((marker) => isMapLabel(marker.kind));
  const showTextControls =
    placementKind === "flag_label" ||
    showMapLabelControls ||
    spec.markers.some((marker) => marker.kind === "flag_label");

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
        {movingMarkerIndex !== null
          ? `Click the map to move ${spec.markers[movingMarkerIndex]?.name ?? "the marker"}.`
          : placementKind
          ? `Click the map to place a ${kindLabel[placementKind].toLowerCase()}.`
          : "Building markers color a footprint. Dots print as smooth one-layer overlays. Flags can stay blank or use an editable name. Surface labels follow the terrain; plaques make a flat raised base."}
      </p>
      {showFlagControls && (
        <p className="color-note">
          Round flag sockets let flags rotate after insertion. A socket that
          crosses a puzzle seam shifts only far enough to fit inside its
          owning piece.
        </p>
      )}

      {spec.markers.length > 0 && (
        <ul className="marker-list">
          {spec.markers.map((marker, index) => (
            <li key={`${marker.latitude}:${marker.longitude}:${index}`}>
              <div className="marker-name-field">
                <input
                  aria-label={`Marker ${index + 1} name`}
                  onChange={(event) =>
                    updateMarker(index, {
                      name: limitMarkerName(event.target.value),
                    })
                  }
                  value={marker.name}
                />
                {(marker.kind === "flag_label" || isMapLabel(marker.kind)) && (
                  <small>This name prints on the model.</small>
                )}
              </div>
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
              {isMapLabel(marker.kind) && (
                <div className="marker-label-fields">
                  <label>
                    <span>Text height</span>
                    <input
                      aria-label={`Marker ${index + 1} text height`}
                      max={12}
                      min={1.5}
                      onChange={(event) => {
                        const value = event.target.valueAsNumber;
                        if (Number.isFinite(value)) {
                          updateMarker(index, { label_height_mm: value });
                        }
                      }}
                      step={0.1}
                      type="number"
                      value={marker.label_height_mm}
                    />
                    <small>mm</small>
                  </label>
                  <label>
                    <span>Rotation</span>
                    <input
                      aria-label={`Marker ${index + 1} rotation`}
                      max={180}
                      min={-180}
                      onChange={(event) => {
                        const value = event.target.valueAsNumber;
                        if (Number.isFinite(value)) {
                          updateMarker(index, { rotation_degrees: value });
                        }
                      }}
                      step={1}
                      type="number"
                      value={marker.rotation_degrees}
                    />
                    <small>° clockwise</small>
                  </label>
                  <label>
                    <span>Relief</span>
                    <input
                      aria-label={`Marker ${index + 1} relief`}
                      max={1.2}
                      min={0.2}
                      onChange={(event) => {
                        const value = event.target.valueAsNumber;
                        if (Number.isFinite(value)) {
                          updateMarker(index, {
                            label_style: {
                              ...resolvedLabelStyle(
                                marker,
                                spec.marker_settings,
                              ),
                              relief_mm: value,
                            },
                          });
                        }
                      }}
                      step={0.1}
                      type="number"
                      value={
                        resolvedLabelStyle(marker, spec.marker_settings)
                          .relief_mm
                      }
                    />
                    <small>mm raised</small>
                  </label>
                  {marker.kind === "plaque_label" && (
                    <>
                      <label>
                        <span>Padding</span>
                        <input
                          aria-label={`Marker ${index + 1} plaque padding`}
                          max={5}
                          min={0.5}
                          onChange={(event) => {
                            const value = event.target.valueAsNumber;
                            if (Number.isFinite(value)) {
                              updateMarker(index, {
                                label_style: {
                                  ...resolvedLabelStyle(
                                    marker,
                                    spec.marker_settings,
                                  ),
                                  plaque_padding_mm: value,
                                },
                              });
                            }
                          }}
                          step={0.1}
                          type="number"
                          value={
                            resolvedLabelStyle(marker, spec.marker_settings)
                              .plaque_padding_mm
                          }
                        />
                        <small>mm</small>
                      </label>
                      <label>
                        <span>Base height</span>
                        <input
                          aria-label={`Marker ${index + 1} plaque base height`}
                          max={3}
                          min={0.4}
                          onChange={(event) => {
                            const value = event.target.valueAsNumber;
                            if (Number.isFinite(value)) {
                              updateMarker(index, {
                                label_style: {
                                  ...resolvedLabelStyle(
                                    marker,
                                    spec.marker_settings,
                                  ),
                                  plaque_thickness_mm: value,
                                },
                              });
                            }
                          }}
                          step={0.1}
                          type="number"
                          value={
                            resolvedLabelStyle(marker, spec.marker_settings)
                              .plaque_thickness_mm
                          }
                        />
                        <small>mm above terrain</small>
                      </label>
                    </>
                  )}
                </div>
              )}
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
              <div className="marker-actions">
                <button
                  aria-label={
                    movingMarkerIndex === index
                      ? `Cancel moving marker ${marker.name}`
                      : `Move marker ${marker.name}`
                  }
                  aria-pressed={movingMarkerIndex === index}
                  className={movingMarkerIndex === index ? "active" : ""}
                  onClick={() => moveMarker(index)}
                  type="button"
                >
                  {movingMarkerIndex === index ? "Cancel" : "Move"}
                </button>
                <button
                  aria-label={`Remove marker ${marker.name}`}
                  onClick={() => removeMarker(index)}
                  type="button"
                >
                  Remove
                </button>
              </div>
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
      {showTextControls && (
        <LabelFontSelect
          onChange={(font) => updateMarkerSettings("label_font", font)}
          value={spec.marker_settings.label_font}
        />
      )}
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
