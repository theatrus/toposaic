import type { GenerationSpec } from "../contracts";
import { RangeField } from "./range-field";
import { SurfaceSection } from "./surface-section";
import type { UpdateColor } from "./surface-types";

export function SurfaceTrailSection({
  importTrailFiles,
  removeTrail,
  spec,
  trailNotice,
  updateColor,
}: {
  importTrailFiles: (files: File[]) => Promise<void>;
  removeTrail: (index: number) => void;
  spec: GenerationSpec;
  trailNotice: string | null;
  updateColor: UpdateColor;
}) {
  return (
    <SurfaceSection
      name="Imported trails"
      description="Add your own hiking routes from GPX or KML files."
    >
      <div className="trail-import">
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
              <li
                key={`${trail.name}:${trail.points.length}:${trail.points[0]?.join(",") ?? ""}`}
              >
                <span>{trail.name}</span>
                <small>{trail.points.length.toLocaleString()} points</small>
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
      </div>
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
              <code>{spec.color_output.trail_color.toUpperCase()}</code>
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
      <small className="control-hint">
        Imported routes print as raised lines in their own color. Saved setups carry them.
      </small>
    </SurfaceSection>
  );
}
