import { type ReactNode, useState } from "react";

import type { GenerationSpec } from "../contracts";
import { filamentSlotEntries, moveFilamentClass } from "../filament-slots";
import type { UpdateTray } from "./mounting-types";
import type { UpdateColor } from "./surface-types";

const HEX_COLOR = /^#[0-9A-F]{6}$/i;

function HexColorField({
  label,
  onChange,
  value,
}: {
  label: string;
  onChange: (value: string) => void;
  value: string;
}) {
  const normalizedValue = value.toUpperCase();
  const [draft, setDraft] = useState<{
    sourceValue: string;
    value: string;
  } | null>(null);
  const [copied, setCopied] = useState(false);
  const displayedValue =
    draft?.sourceValue === normalizedValue ? draft.value : normalizedValue;

  const commit = (candidate: string) => {
    const normalized = candidate.toUpperCase();
    setDraft({ sourceValue: normalizedValue, value: normalized });
    if (HEX_COLOR.test(normalized)) {
      onChange(normalized);
      return true;
    }
    return false;
  };

  return (
    <div className="palette-color">
      <label className="palette-color-heading">
        <input
          aria-label={`${label} swatch`}
          onChange={(event) => {
            setDraft(null);
            onChange(event.target.value.toUpperCase());
          }}
          type="color"
          value={value}
        />
        <strong>{label}</strong>
      </label>
      <div className="palette-color-value">
        <input
          aria-invalid={!HEX_COLOR.test(displayedValue)}
          aria-label={`${label} color`}
          autoCapitalize="characters"
          maxLength={7}
          onBlur={() => {
            commit(displayedValue);
            setDraft(null);
          }}
          onChange={(event) => commit(event.target.value)}
          onKeyDown={(event) => {
            if (event.key === "Enter") {
              event.preventDefault();
              event.currentTarget.blur();
            }
          }}
          spellCheck={false}
          type="text"
          value={displayedValue}
        />
        <button
          aria-label={`Copy ${label} color`}
          onClick={() => {
            void navigator.clipboard.writeText(normalizedValue).then(() => {
              setCopied(true);
              window.setTimeout(() => setCopied(false), 1200);
            }).catch(() => undefined);
          }}
          type="button"
        >
          {copied ? "Copied" : "Copy"}
        </button>
      </div>
    </div>
  );
}

function ColorGroup({
  children,
  description,
  title,
}: {
  children: ReactNode;
  description: string;
  title: string;
}) {
  return (
    <section className="palette-group">
      <div className="palette-group-heading">
        <strong>{title}</strong>
        <p>{description}</p>
      </div>
      <div className="palette-grid">{children}</div>
    </section>
  );
}

export function ColorsPanel({
  discoveredGround = [],
  hidden,
  spec,
  updateColor,
  updateMarkerSettings,
  updateTray,
}: {
  // The ground palette the last finished job discovered. Empty before one
  // has run: these colors come off the imagery, not the settings.
  discoveredGround?: readonly string[];
  hidden: boolean;
  spec: GenerationSpec;
  updateColor: UpdateColor;
  updateMarkerSettings: <Key extends keyof GenerationSpec["marker_settings"]>(
    key: Key,
    value: GenerationSpec["marker_settings"][Key],
  ) => void;
  updateTray: UpdateTray;
}) {
  const palette = [
    ["Forest", spec.color_output.forest_color],
    ["Rock", spec.color_output.rock_color],
    ["Snow", spec.color_output.snow_color],
    ["Water", spec.color_output.water_color],
    ["Route", spec.color_output.road_color],
    ["Trail", spec.color_output.route_trail_color],
    ["Imported trail", spec.color_output.trail_color],
    ["Railway", spec.color_output.rail_color],
    ["Aerial lift", spec.color_output.aerial_color],
    ["Ferry", spec.color_output.ferry_color],
    ["Airport surface", spec.color_output.aviation_color],
    ["Building", spec.color_output.building_color],
    ["Map marker", spec.marker_settings.color],
    ["Display base", spec.tray.tray_color],
    ["Base contours", spec.tray.contour_color],
    ["Base label", spec.tray.label_color],
  ] as const;
  const [paletteCopied, setPaletteCopied] = useState(false);

  return (
    <fieldset
      aria-label="Print colors"
      className="color-palette-panel control-section"
      hidden={hidden}
    >
      <div className="color-heading palette-heading">
        <div>
          <strong className="color-title">Print colors</strong>
          <p>
            Copy and paste hex values to make several outputs target the same
            filament.
          </p>
        </div>
        <button
          aria-label="Copy all colors"
          onClick={() => {
            const text = palette
              .map(([label, color]) => `${label}: ${color.toUpperCase()}`)
              .join("\n");
            void navigator.clipboard
              .writeText(text)
              .then(() => {
                setPaletteCopied(true);
                window.setTimeout(() => setPaletteCopied(false), 1200);
              })
              .catch(() => undefined);
          }}
          type="button"
        >
          {paletteCopied ? "Palette copied" : "Copy palette"}
        </button>
      </div>

      <label className="road-detail-field">
        Filament preset
        <select
          value={spec.color_output.filament_profile}
          onChange={(event) =>
            updateColor(
              "filament_profile",
              event.target
                .value as GenerationSpec["color_output"]["filament_profile"],
            )
          }
        >
          <option value="generic_pla">Generic PLA</option>
          <option value="bambu_pla_basic">Bambu PLA Basic</option>
          <option value="polylite_pla">Polymaker PolyLite PLA</option>
          <option value="polyterra_pla">Polymaker PolyTerra PLA</option>
        </select>
        <small>
          The preset each filament slot asks for when the Output tab writes a
          color project. Naming one stops OrcaSlicer and Bambu Studio picking
          a material of their own. Slicer preset names also carry a printer
          suffix, which the file cannot know, so a slicer that cannot match
          the exact preset still reads the right vendor and material.
        </small>
      </label>

      <fieldset
        aria-label="Output filament order"
        className="filament-order control-subsection"
      >
        <legend>Output filament order</legend>
        <ol className="filament-order-list">
          {filamentSlotEntries(spec, discoveredGround).map((entry, index, entries) => {
            // Bound once so the handlers below narrow: a property read
            // inside a closure does not.
            const classKey = entry.classKey;
            const movable = entries.filter((other) => other.classKey).length;
            return (
            <li key={classKey ?? `discovered-${entry.label}`}>
              <span className="filament-number">F{entry.filament}</span>
              <span
                aria-hidden="true"
                className="filament-swatch"
                style={{ backgroundColor: entry.color }}
              />
              <span className="filament-label">{entry.label}</span>
              {classKey === null ? (
                // Discovered from the imagery, not a setting: there is no
                // color to edit and no place in the order to move it to.
                <span className="filament-note">from the imagery</span>
              ) : (
                <>
                  <button
                    aria-label={`Move ${entry.label} earlier`}
                    disabled={index === 0}
                    onClick={() => {
                      const order = moveFilamentClass(spec, classKey, "earlier");
                      if (order) {
                        updateColor("filament_order", order);
                      }
                    }}
                    type="button"
                  >
                    ↑
                  </button>
                  <button
                    aria-label={`Move ${entry.label} later`}
                    disabled={index === movable - 1}
                    onClick={() => {
                      const order = moveFilamentClass(spec, classKey, "later");
                      if (order) {
                        updateColor("filament_order", order);
                      }
                    }}
                    type="button"
                  >
                    ↓
                  </button>
                </>
              )}
            </li>
            );
          })}
        </ol>
        <small>
          The filament number each layer prints from, in every 3MF style.
          Two layers with one color share a number. Numbers assume each
          layer appears on the map; a layer the map lacks gives its number
          up and later ones move down.
          {spec.color_output.ground_colors !== "mapped" &&
            (discoveredGround.length > 0
              ? " The ground colors below them were read off this area's imagery, so they cannot be edited or reordered here."
              : " This model also prints satellite ground colors, which are read off the imagery — generate it once to see them listed here.")}
        </small>
      </fieldset>

      <ColorGroup
        title="Terrain and land cover"
        description="Mapped surface classes. Sides and bottoms use rock."
      >
        <HexColorField
          label="Forest"
          value={spec.color_output.forest_color}
          onChange={(value) => updateColor("forest_color", value)}
        />
        <HexColorField
          label="Rock"
          value={spec.color_output.rock_color}
          onChange={(value) => updateColor("rock_color", value)}
        />
        <HexColorField
          label="Snow"
          value={spec.color_output.snow_color}
          onChange={(value) => updateColor("snow_color", value)}
        />
        <HexColorField
          label="Water"
          value={spec.color_output.water_color}
          onChange={(value) => updateColor("water_color", value)}
        />
      </ColorGroup>

      <ColorGroup
        title="Roads and transport"
        description="Mapped routes and trails, imported tracks, railways, aerial lifts, and ferries."
      >
        <HexColorField
          label="Route"
          value={spec.color_output.road_color}
          onChange={(value) => updateColor("road_color", value)}
        />
        <HexColorField
          label="Trail"
          value={spec.color_output.route_trail_color}
          onChange={(value) => updateColor("route_trail_color", value)}
        />
        <HexColorField
          label="Imported trail"
          value={spec.color_output.trail_color}
          onChange={(value) => updateColor("trail_color", value)}
        />
        <HexColorField
          label="Railway"
          value={spec.color_output.rail_color}
          onChange={(value) => updateColor("rail_color", value)}
        />
        <HexColorField
          label="Aerial lift"
          value={spec.color_output.aerial_color}
          onChange={(value) => updateColor("aerial_color", value)}
        />
        <HexColorField
          label="Ferry"
          value={spec.color_output.ferry_color}
          onChange={(value) => updateColor("ferry_color", value)}
        />
        <HexColorField
          label="Airport surface"
          value={spec.color_output.aviation_color}
          onChange={(value) => updateColor("aviation_color", value)}
        />
      </ColorGroup>

      <ColorGroup
        title="Structures and markers"
        description="Mapped buildings and markers placed on the map."
      >
        <HexColorField
          label="Building"
          value={spec.color_output.building_color}
          onChange={(value) => updateColor("building_color", value)}
        />
        <HexColorField
          label="Map marker"
          value={spec.marker_settings.color}
          onChange={(value) => updateMarkerSettings("color", value)}
        />
      </ColorGroup>

      <ColorGroup
        title="Display base"
        description="The base body, contour inlays, and raised label."
      >
        <HexColorField
          label="Display base"
          value={spec.tray.tray_color}
          onChange={(value) => updateTray("tray_color", value)}
        />
        <HexColorField
          label="Base contours"
          value={spec.tray.contour_color}
          onChange={(value) => updateTray("contour_color", value)}
        />
        <HexColorField
          label="Base label"
          value={spec.tray.label_color}
          onChange={(value) => updateTray("label_color", value)}
        />
      </ColorGroup>
      <p className="color-note palette-note">
        Colors stay editable while a layer is off so setups can be prepared in
        advance. A color affects geometry only when its matching output is
        enabled. Optional line and marker slots are also omitted when no
        matching geometry exists.
      </p>
    </fieldset>
  );
}
