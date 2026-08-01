import type { GenerationSpec, MarkerKind } from "../contracts";
import {
  DEFAULT_DOT_MARKER_STYLE,
  DEFAULT_FLAG_MARKER_STYLE,
  DEFAULT_MAP_LABEL_STYLE,
  isFlagMarker,
  isMapLabel,
  limitMarkerName,
} from "../config";
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

type MarkerGroupKey = "building" | "dot" | "flag" | "label";

const markerGroups: Array<{
  key: MarkerGroupKey;
  title: string;
  note: string;
  includes: (kind: MarkerKind) => boolean;
}> = [
  {
    key: "building",
    title: "Building highlights",
    note: "Color selected OpenStreetMap building footprints.",
    includes: (kind) => kind === "building",
  },
  {
    key: "dot",
    title: "Surface dots",
    note: "Print smooth, terrain-following dots one layer above the surface.",
    includes: (kind) => kind === "dot",
  },
  {
    key: "flag",
    title: "Flags",
    note: "Use blank flags or print an editable name on the banner.",
    includes: isFlagMarker,
  },
  {
    key: "label",
    title: "Terrain labels",
    note: "Follow the terrain or add a flat raised plaque.",
    includes: isMapLabel,
  },
];

function resolvedLabelStyle(
  marker: GenerationSpec["markers"][number],
): MapLabelStyle {
  return marker.label_style ?? DEFAULT_MAP_LABEL_STYLE;
}

function resolvedDotStyle(marker: GenerationSpec["markers"][number]) {
  return marker.dot_style ?? DEFAULT_DOT_MARKER_STYLE;
}

function resolvedFlagStyle(marker: GenerationSpec["markers"][number]) {
  return marker.flag_style ?? DEFAULT_FLAG_MARKER_STYLE;
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
}) {
  const groupedMarkers = markerGroups
    .map((group) => ({
      ...group,
      markers: spec.markers
        .map((marker, index) => ({ marker, index }))
        .filter(({ marker }) => group.includes(marker.kind)),
    }))
    .filter((group) => group.markers.length > 0);

  return (
    <fieldset
      aria-label="Map markers"
      className="control-section marker-controls"
      hidden={hidden}
    >
      <div className="color-heading">
        <div>
          <strong className="color-title">Map markers</strong>
          <p>Choose a marker, then click its place on the map or 3D terrain.</p>
        </div>
      </div>

      <div
        className="marker-kind-buttons"
        role="group"
        aria-label="Marker type"
      >
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
          ? `Click the map or 3D terrain to move ${spec.markers[movingMarkerIndex]?.name ?? "the marker"}.`
          : placementKind
            ? `Click the map or 3D terrain to place a ${kindLabel[placementKind].toLowerCase()}.`
            : "Building markers color a footprint. Dots print as smooth one-layer overlays. Flags can stay blank or use an editable name. Surface labels follow the terrain; plaques make a flat raised base."}
      </p>
      {groupedMarkers.length > 0 && (
        <div className="marker-type-groups">
          {groupedMarkers.map((group) => (
            <section
              aria-labelledby={`marker-group-${group.key}`}
              className={`marker-type-group marker-type-${group.key}`}
              key={group.key}
            >
              <header className="marker-type-heading">
                <div>
                  <strong id={`marker-group-${group.key}`}>
                    {group.title}
                  </strong>
                  <small>{group.note}</small>
                </div>
                <span>{group.markers.length}</span>
              </header>

              <ul className="marker-list">
                {group.markers.map(({ marker, index }) => (
                  <li key={`${marker.latitude}:${marker.longitude}:${index}`}>
                    <span aria-hidden="true" className="marker-row-index">
                      {index + 1}
                    </span>
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
                      {(marker.kind === "flag_label" ||
                        isMapLabel(marker.kind)) && (
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
                    {marker.kind === "dot" && (
                      <div className="marker-instance-settings">
                        <RangeField
                          ariaLabel={`Marker ${index + 1} dot diameter`}
                          label="Dot diameter"
                          max={10}
                          min={1}
                          onChange={(diameter_mm) =>
                            updateMarker(index, {
                              dot_style: { diameter_mm },
                            })
                          }
                          step={0.2}
                          unit="mm"
                          value={resolvedDotStyle(marker).diameter_mm}
                        />
                      </div>
                    )}
                    {isFlagMarker(marker.kind) && (
                      <div className="marker-instance-settings marker-flag-settings">
                        <RangeField
                          ariaLabel={`Marker ${index + 1} flag-hole diameter`}
                          label="Flag-hole diameter"
                          max={6}
                          min={1.2}
                          onChange={(hole_diameter_mm) => {
                            const style = resolvedFlagStyle(marker);
                            updateMarker(index, {
                              flag_style: {
                                ...style,
                                hole_diameter_mm,
                                fit_clearance_mm: Math.min(
                                  style.fit_clearance_mm,
                                  Math.max(0.1, hole_diameter_mm - 0.9),
                                ),
                              },
                            });
                          }}
                          step={0.2}
                          unit="mm"
                          value={resolvedFlagStyle(marker).hole_diameter_mm}
                        />
                        <RangeField
                          ariaLabel={`Marker ${index + 1} flag-hole depth`}
                          label="Flag-hole depth"
                          max={Math.min(6, Math.max(0.6, spec.base_mm - 0.4))}
                          min={0.6}
                          onChange={(hole_depth_mm) =>
                            updateMarker(index, {
                              flag_style: {
                                ...resolvedFlagStyle(marker),
                                hole_depth_mm,
                              },
                            })
                          }
                          step={0.2}
                          unit="mm"
                          value={Math.min(
                            resolvedFlagStyle(marker).hole_depth_mm,
                            Math.max(0.6, spec.base_mm - 0.4),
                          )}
                        />
                        <RangeField
                          ariaLabel={`Marker ${index + 1} flag fit clearance`}
                          label="Flag fit clearance"
                          max={Math.min(
                            0.6,
                            Math.max(
                              0.1,
                              resolvedFlagStyle(marker).hole_diameter_mm - 0.9,
                            ),
                          )}
                          min={0.1}
                          onChange={(fit_clearance_mm) =>
                            updateMarker(index, {
                              flag_style: {
                                ...resolvedFlagStyle(marker),
                                fit_clearance_mm,
                              },
                            })
                          }
                          step={0.05}
                          unit="mm"
                          value={resolvedFlagStyle(marker).fit_clearance_mm}
                        />
                        <RangeField
                          ariaLabel={`Marker ${index + 1} flag width`}
                          label="Flag width"
                          max={80}
                          min={12}
                          onChange={(width_mm) =>
                            updateMarker(index, {
                              flag_style: {
                                ...resolvedFlagStyle(marker),
                                width_mm,
                              },
                            })
                          }
                          step={1}
                          unit="mm"
                          value={resolvedFlagStyle(marker).width_mm}
                        />
                        <RangeField
                          ariaLabel={`Marker ${index + 1} flag height`}
                          label="Flag height"
                          max={30}
                          min={6}
                          onChange={(height_mm) => {
                            const style = resolvedFlagStyle(marker);
                            updateMarker(index, {
                              flag_style: {
                                ...style,
                                height_mm,
                                label_height_mm: Math.min(
                                  style.label_height_mm,
                                  Math.max(1.5, height_mm - 2),
                                ),
                              },
                            });
                          }}
                          step={1}
                          unit="mm"
                          value={resolvedFlagStyle(marker).height_mm}
                        />
                        {marker.kind === "flag_label" && (
                          <>
                            <LabelFontSelect
                              ariaLabel={`Marker ${index + 1} label font`}
                              onChange={(label_font) =>
                                updateMarker(index, {
                                  flag_style: {
                                    ...resolvedFlagStyle(marker),
                                    label_font,
                                  },
                                })
                              }
                              value={resolvedFlagStyle(marker).label_font}
                            />
                            <RangeField
                              ariaLabel={`Marker ${index + 1} flag label height`}
                              label="Flag label height"
                              max={Math.min(
                                10,
                                resolvedFlagStyle(marker).height_mm - 2,
                              )}
                              min={1.5}
                              note="Long names shrink to fit the banner."
                              onChange={(label_height_mm) =>
                                updateMarker(index, {
                                  flag_style: {
                                    ...resolvedFlagStyle(marker),
                                    label_height_mm,
                                  },
                                })
                              }
                              step={0.1}
                              unit="mm"
                              value={Math.min(
                                resolvedFlagStyle(marker).label_height_mm,
                                resolvedFlagStyle(marker).height_mm - 2,
                              )}
                            />
                          </>
                        )}
                        <label className="marker-template-toggle">
                          <input
                            aria-label={`Marker ${index + 1} export printable flag`}
                            checked={resolvedFlagStyle(marker).export_template}
                            onChange={(event) =>
                              updateMarker(index, {
                                flag_style: {
                                  ...resolvedFlagStyle(marker),
                                  export_template: event.target.checked,
                                },
                              })
                            }
                            type="checkbox"
                          />
                          Export this printable flag
                        </label>
                        <p className="marker-type-note">
                          The round socket lets this flag rotate after
                          insertion. A socket that crosses a puzzle seam shifts
                          only far enough to fit inside its owning piece.
                        </p>
                      </div>
                    )}
                    {isMapLabel(marker.kind) && (
                      <div className="marker-label-fields">
                        <LabelFontSelect
                          ariaLabel={`Marker ${index + 1} label font`}
                          onChange={(label_font) =>
                            updateMarker(index, {
                              label_style: {
                                ...resolvedLabelStyle(marker),
                                label_font,
                              },
                            })
                          }
                          value={resolvedLabelStyle(marker).label_font}
                        />
                        <RangeField
                          ariaLabel={`Marker ${index + 1} text height`}
                          label="Text height"
                          max={12}
                          min={1.5}
                          onChange={(label_height_mm) =>
                            updateMarker(index, { label_height_mm })
                          }
                          step={0.1}
                          unit="mm"
                          value={marker.label_height_mm}
                        />
                        <label>
                          <span>Rotation</span>
                          <input
                            aria-label={`Marker ${index + 1} rotation`}
                            max={180}
                            min={-180}
                            onChange={(event) => {
                              const value = event.target.valueAsNumber;
                              if (Number.isFinite(value)) {
                                updateMarker(index, {
                                  rotation_degrees: value,
                                });
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
                                    ...resolvedLabelStyle(marker),
                                    relief_mm: value,
                                  },
                                });
                              }
                            }}
                            step={0.1}
                            type="number"
                            value={resolvedLabelStyle(marker).relief_mm}
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
                                        ...resolvedLabelStyle(marker),
                                        plaque_padding_mm: value,
                                      },
                                    });
                                  }
                                }}
                                step={0.1}
                                type="number"
                                value={
                                  resolvedLabelStyle(marker).plaque_padding_mm
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
                                        ...resolvedLabelStyle(marker),
                                        plaque_thickness_mm: value,
                                      },
                                    });
                                  }
                                }}
                                step={0.1}
                                type="number"
                                value={
                                  resolvedLabelStyle(marker).plaque_thickness_mm
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
                        onCommit={(latitude) =>
                          updateMarker(index, { latitude })
                        }
                        value={marker.latitude}
                      />
                      <MarkerCoordinateInput
                        label={`Marker ${index + 1} longitude`}
                        max={180}
                        min={-180}
                        onCommit={(longitude) =>
                          updateMarker(index, { longitude })
                        }
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
            </section>
          ))}
        </div>
      )}
    </fieldset>
  );
}
