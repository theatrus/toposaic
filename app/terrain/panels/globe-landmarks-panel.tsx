import type { GlobeLandmark, GlobeLandmarkPlacement } from "../contracts";

const MAX_LANDMARKS = 24;
const EXAMPLES: Omit<GlobeLandmark, "id">[] = [
  { name: "Eiffel Tower", shape: "tower", latitude: 48.8583, longitude: 2.2945, width_mm: 7, height_mm: 13 },
  { name: "Great Pyramid of Giza", shape: "pyramid", latitude: 29.9792, longitude: 31.1342, width_mm: 9, height_mm: 5 },
  { name: "Sydney", shape: "city_marker", latitude: -33.8672, longitude: 151.2075, width_mm: 6, height_mm: 4 },
];

export function GlobeLandmarksPanel({ hidden, landmarks, placements, update }: {
  hidden: boolean;
  landmarks: GlobeLandmark[];
  placements?: GlobeLandmarkPlacement[];
  update: (landmarks: GlobeLandmark[]) => void;
}) {
  const nextId = Math.max(0, ...landmarks.map((landmark) => landmark.id)) + 1;
  const add = (items: Omit<GlobeLandmark, "id">[]) => update([
    ...landmarks,
    ...items.slice(0, MAX_LANDMARKS - landmarks.length).map((item, index) => ({ ...item, id: nextId + index })),
  ]);
  return (
    <fieldset className="control-section globe-landmarks" hidden={hidden}>
      <legend>Landmarks</legend>
      <div className="globe-landmark-actions">
        <button type="button" disabled={landmarks.length >= MAX_LANDMARKS} onClick={() => add(EXAMPLES.filter((item) => !landmarks.some((landmark) => landmark.name === item.name)))}>Add example landmarks</button>
        <button type="button" disabled={landmarks.length >= MAX_LANDMARKS} onClick={() => add([{ name: "City marker", shape: "city_marker", latitude: 0, longitude: 0, width_mm: 6, height_mm: 4 }])}>Add custom landmark</button>
      </div>
      {landmarks.map((landmark) => {
        const placement = placements?.find((site) => site.id === landmark.id && site.name === landmark.name
          && site.latitude === landmark.latitude && site.longitude === landmark.longitude && site.shape === landmark.shape
          && Math.abs(site.width_mm - landmark.width_mm) < 0.001 && Math.abs(site.height_mm - landmark.height_mm) < 0.001);
        const change = (fields: Partial<GlobeLandmark>) => {
          if (Object.entries(fields).some(([key, value]) => landmark[key as keyof GlobeLandmark] !== value)) {
            update(landmarks.map((item) => item.id === landmark.id ? { ...item, ...fields } : item));
          }
        };
        return (
          <details className="globe-detail" key={landmark.id}>
            <summary>
              {String(landmark.id).padStart(2, "0")} · {landmark.name} {placement && `· Tile ${placement.tile}`}
            </summary>
            <div className="globe-detail-fields">
              <label className="road-detail-field">
                Name
                <input aria-label={`Landmark ${landmark.id} name`} key={landmark.name} defaultValue={landmark.name} maxLength={60} onBlur={(event) => {
                  const name = event.target.value.trim();
                  if (name) change({ name });
                  else event.target.value = landmark.name;
                }} onKeyDown={finishEdit} />
              </label>
              <label className="road-detail-field">
                Shape
                <select aria-label={`Landmark ${landmark.id} shape`} value={landmark.shape} onChange={(event) => change({ shape: event.target.value as GlobeLandmark["shape"] })}>
                  <option value="tower">Tower</option>
                  <option value="pyramid">Pyramid</option>
                  <option value="city_marker">City marker</option>
                </select>
              </label>
              <NumberInput label="Latitude" id={landmark.id} value={landmark.latitude} min={-90} max={90} step={0.0001} onChange={(latitude) => change({ latitude })} />
              <NumberInput label="Longitude" id={landmark.id} value={landmark.longitude} min={-180} max={180} step={0.0001} onChange={(longitude) => change({ longitude })} />
              <NumberInput label="Width (mm)" id={landmark.id} value={landmark.width_mm} min={5} max={14} step={0.1} onChange={(width_mm) => change({ width_mm })} />
              <NumberInput label="Figure height (mm)" id={landmark.id} value={landmark.height_mm} min={4} max={20} step={0.1} onChange={(height_mm) => change({ height_mm })} />
              <button type="button" aria-label={`Remove ${landmark.name}`} onClick={() => update(landmarks.filter((item) => item.id !== landmark.id))}>Remove landmark</button>
            </div>
            {placement && (placement.seat ? (
              <p>Site {landmark.latitude.toFixed(4)}°, {landmark.longitude.toFixed(4)}°{placement.seat.offset_mm > 0.3 && (<>. Seat {placement.seat.latitude.toFixed(4)}°, {placement.seat.longitude.toFixed(4)}° · {placement.seat.offset_mm.toFixed(1)} mm offset</>)}.</p>
            ) : <p>The socket could not clear the tile seam or frame attachment. Try a smaller width or move the site.</p>)}
          </details>
        );
      })}
    </fieldset>
  );
}

function finishEdit(event: React.KeyboardEvent<HTMLInputElement>) {
  if (event.key === "Enter") {
    event.preventDefault();
    event.currentTarget.blur();
  }
}

function NumberInput({ label, id, value, min, max, step, onChange }: {
  label: string;
  id: number;
  value: number;
  min: number;
  max: number;
  step: number;
  onChange: (value: number) => void;
}) {
  return (
    <label className="road-detail-field">
      {label}
      <input aria-label={`Landmark ${id} ${label.toLowerCase()}`} type="number" key={value} defaultValue={value} min={min} max={max} step={step} onKeyDown={finishEdit} onBlur={(event) => {
        const next = event.target.valueAsNumber;
        if (next >= min && next <= max) onChange(next);
        else event.target.value = String(value);
      }} />
    </label>
  );
}
