import {
  FINE_DEM_MAX_SPAN_KM,
  MAX_ASSEMBLED_SAMPLES,
  MESH_QUALITY_OPTIONS,
  assembledMeshSamples,
  formatGroundSpacing,
  groundMeshSpacing,
  randomPuzzleSeed,
} from "../config";
import type { GenerationSpec, PlaceResult } from "../contracts";
import type { AdjacentDirection } from "../geo";
import { RangeField } from "./range-field";

export function ModelPanel({
  adjacentMessage,
  choosePlace,
  heightFrameCompatible,
  heightFrameLocked,
  heightScaleReadout,
  hidden,
  lockHeightFrame,
  moveToAdjacentTile,
  placeMessage,
  placeQuery,
  placeResults,
  searchPlaces,
  searchingPlaces,
  setMeshQuality,
  setPlaceQuery,
  setHeightMode,
  setSuperTileAnchor,
  spec,
  superTileGridSizes,
  unlockHeightFrame,
  update,
}: {
  adjacentMessage: string | null;
  choosePlace: (place: PlaceResult) => void;
  heightFrameCompatible: boolean;
  heightFrameLocked: boolean;
  hidden: boolean;
  lockHeightFrame: () => boolean;
  moveToAdjacentTile: (direction: AdjacentDirection) => void;
  placeMessage: string | null;
  placeQuery: string;
  placeResults: PlaceResult[];
  searchPlaces: () => Promise<void>;
  searchingPlaces: boolean;
  setMeshQuality: (samples: number) => void;
  setPlaceQuery: (value: string) => void;
  setHeightMode: (mode: GenerationSpec["height_mode"]) => void;
  setSuperTileAnchor: (anchor: GenerationSpec["super_tile_anchor"]) => void;
  spec: GenerationSpec;
  /** Sampled elevation bounds, for the derived scale readout. */
  heightScaleReadout: { exaggeration: number; height: number } | null;
  superTileGridSizes: number[];
  unlockHeightFrame: () => void;
  update: <Key extends keyof GenerationSpec>(
    key: Key,
    value: GenerationSpec[Key],
  ) => void;
}) {
  return (
    <section
      className="control-section model-controls"
      hidden={hidden}
    >
      <div className="place-search">
        <label htmlFor="place-search-input">Find a place</label>
        <div className="place-search-row">
          <input
            id="place-search-input"
            type="search"
            value={placeQuery}
            placeholder="Mountain, park, city…"
            onChange={(event) => setPlaceQuery(event.target.value)}
            onKeyDown={(event) => {
              if (event.key === "Enter") {
                event.preventDefault();
                void searchPlaces();
              }
            }}
          />
          <button
            type="button"
            disabled={searchingPlaces}
            onClick={() => void searchPlaces()}
          >
            {searchingPlaces ? "Searching…" : "Search"}
          </button>
        </div>
        {placeMessage && (
          <p className="place-search-message" role="status">
            {placeMessage}
          </p>
        )}
        {placeResults.length > 0 && (
          <ul className="place-results" aria-label="Place search results">
            {placeResults.map((place) => (
              <li
                key={`${place.latitude}-${place.longitude}-${place.display_name}`}
              >
                <button type="button" onClick={() => choosePlace(place)}>
                  <span>{place.display_name}</span>
                  <small>
                    {place.category} · {place.kind.replaceAll("_", " ")}
                  </small>
                </button>
              </li>
            ))}
          </ul>
        )}
        <p className="place-search-note">
          Submit-only search sends public place names to{" "}
          <a
            href="https://www.openstreetmap.org/copyright"
            target="_blank"
            rel="noreferrer"
          >
            OpenStreetMap
          </a>
          . Do not enter private information.
        </p>
      </div>

      <div className="coordinate-adjacent-row">
        <div
          className="coordinate-box"
          role="group"
          aria-label="Map position"
        >
          <strong>Map position</strong>
          <div className="coordinate-row">
            <label>
              Latitude
              <input
                type="number"
                step="0.00001"
                value={spec.center_lat}
                onChange={(event) => {
                  const latitude = Number(event.target.value);
                  if (
                    event.target.value.trim() === "" ||
                    !Number.isFinite(latitude)
                  ) {
                    return;
                  }
                  update("center_lat", latitude);
                }}
              />
            </label>
            <label>
              Longitude
              <input
                type="number"
                step="0.00001"
                value={spec.center_lon}
                onChange={(event) => {
                  const longitude = Number(event.target.value);
                  if (
                    event.target.value.trim() === "" ||
                    !Number.isFinite(longitude)
                  ) {
                    return;
                  }
                  update("center_lon", longitude);
                }}
              />
            </label>
          </div>
          <div className="tile-position-block">
            <div className="tile-position-heading">
              <span>
                <strong>Matching tile position</strong>
                <small>
                  Move one tile while keeping every shared edge aligned.
                </small>
              </span>
              <div className="adjacent-actions" aria-label="Move one tile">
                {(["north", "west", "east", "south"] as const).map(
                  (direction) => (
                    <button
                      type="button"
                      key={direction}
                      aria-label={`Move ${direction} one tile`}
                      title={`Move ${direction} one tile`}
                      onClick={() => moveToAdjacentTile(direction)}
                    >
                      <span aria-hidden="true">
                        {direction === "north"
                          ? "↑"
                          : direction === "south"
                            ? "↓"
                            : direction === "east"
                              ? "→"
                              : "←"}
                      </span>
                    </button>
                  ),
                )}
              </div>
            </div>
            <div className="coordinate-row">
              <label>
                Tile column
                <input
                  type="number"
                  min="-1000000"
                  max="1000000"
                  step="1"
                  value={spec.puzzle_tile_column}
                  onChange={(event) => {
                    const value = Number(event.target.value);
                    if (
                      Number.isInteger(value) &&
                      Math.abs(value) <= 1_000_000
                    ) {
                      update("puzzle_tile_column", value);
                    }
                  }}
                />
              </label>
              <label>
                Tile row
                <input
                  type="number"
                  min="-1000000"
                  max="1000000"
                  step="1"
                  value={spec.puzzle_tile_row}
                  onChange={(event) => {
                    const value = Number(event.target.value);
                    if (
                      Number.isInteger(value) &&
                      Math.abs(value) <= 1_000_000
                    ) {
                      update("puzzle_tile_row", value);
                    }
                  }}
                />
              </label>
            </div>
          </div>
        </div>
        <div
          className="adjacent-tiles"
          role="group"
          aria-label="Super-tile mode"
        >
          <div className="adjacent-heading">
            <strong>Super-tile mode</strong>
            <button
              type="button"
              onClick={
                heightFrameLocked ? unlockHeightFrame : lockHeightFrame
              }
            >
              {heightFrameLocked ? "Unlock height" : "Lock height"}
            </button>
          </div>
          <div className="adjacent-compact-row">
            <div className="adjacent-grid" aria-label="Super-tile grid">
              <span>Grid</span>
              <label>
                Across
                <select
                  value={spec.adjacent_columns}
                  onChange={(event) =>
                    update("adjacent_columns", Number(event.target.value))
                  }
                >
                  {superTileGridSizes.map((value) => (
                    <option key={value}>{value}</option>
                  ))}
                </select>
              </label>
              <span aria-hidden="true">×</span>
              <label>
                Down
                <select
                  value={spec.adjacent_rows}
                  onChange={(event) =>
                    update("adjacent_rows", Number(event.target.value))
                  }
                >
                  {superTileGridSizes.map((value) => (
                    <option key={value}>{value}</option>
                  ))}
                </select>
              </label>
            </div>
          </div>
          <div
            className="super-tile-anchor"
            role="radiogroup"
            aria-label="Super-tile anchor"
          >
            <span>Anchor</span>
            <label>
              <input
                type="radio"
                name="super-tile-anchor"
                checked={spec.super_tile_anchor === "top_left"}
                onChange={() => setSuperTileAnchor("top_left")}
              />
              <span>Top-left tile</span>
            </label>
            <label>
              <input
                type="radio"
                name="super-tile-anchor"
                checked={spec.super_tile_anchor === "center"}
                onChange={() => setSuperTileAnchor("center")}
              />
              <span>Center tile</span>
            </label>
          </div>
          {(spec.adjacent_columns > 1 || spec.adjacent_rows > 1) && (
            <>
              <label className="adjacent-interlock-toggle">
                <input
                  type="checkbox"
                  checked={spec.adjacent_interlocks}
                  onChange={(event) =>
                    update("adjacent_interlocks", event.target.checked)
                  }
                />
                Interlock super-tile and tray joins
              </label>
              <label className="adjacent-interlock-toggle">
                <input
                  type="checkbox"
                  checked={spec.outer_edge_interlocks}
                  onChange={(event) =>
                    update("outer_edge_interlocks", event.target.checked)
                  }
                />
                Add notches to outer super-tile edges
              </label>
            </>
          )}
          <p
            className={`height-frame-status${
              heightFrameLocked && !heightFrameCompatible
                ? " warning"
                : ""
            }`}
            role={
              heightFrameLocked && !heightFrameCompatible
                ? "alert"
                : "status"
            }
          >
            {heightFrameLocked && !heightFrameCompatible
              ? `Shared datum ${spec.elevation_datum_m?.toFixed(
                  1,
                )} m · ${spec.elevation_m_per_mm?.toFixed(
                  1,
                )} m/mm. This tile drops below the shared ${spec.elevation_datum_m?.toFixed(
                  1,
                )} m datum. Lower the datum and regenerate earlier tiles.`
              : heightFrameLocked
                ? `Shared datum ${spec.elevation_datum_m?.toFixed(
                    1,
                  )} m · ${spec.elevation_m_per_mm?.toFixed(1)} m/mm`
                : spec.adjacent_columns > 1 || spec.adjacent_rows > 1
                  ? `${spec.adjacent_columns * spec.adjacent_rows} terrain 3MF files; current tile is the ${
                      spec.super_tile_anchor === "center"
                        ? "grid center"
                        : "top-left tile"
                    }. The super-tile shares one height frame.`
                  : spec.height_mode === "multiplier"
                    ? "A fixed multiplier already matches separately generated neighbors."
                    : "Auto height fits one tile; manual neighbors may form a step. A multiplier, or a locked height, keeps them level."}
          </p>
          {adjacentMessage && (
            <p className="adjacent-message">{adjacentMessage}</p>
          )}
        </div>
      </div>
      <details className="puzzle-seed-advanced">
        <summary>Advanced puzzle identity</summary>
        <div className="puzzle-seed-row" aria-label="Puzzle identity">
          <label>
            Puzzle seed
            <input
              type="number"
              min="0"
              max="4294967295"
              step="1"
              value={spec.puzzle_seed}
              onChange={(event) => {
                const value = Number(event.target.value);
                if (
                  Number.isInteger(value) &&
                  value >= 0 &&
                  value <= 4_294_967_295
                ) {
                  update("puzzle_seed", value);
                }
              }}
            />
          </label>
          <button
            type="button"
            onClick={() => update("puzzle_seed", randomPuzzleSeed())}
          >
            Generate new seed
          </button>
          <p>
            This seed controls jigsaw edges for single tiles and super-tiles.
            Changing it changes every edge. Keep it with the tile row, tile
            column, and piece grid when making matching tiles later.
          </p>
        </div>
      </details>
      <label className="elevation-source-field">
        Elevation tiles
        <select
          value={spec.elevation_source}
          onChange={(event) =>
            update(
              "elevation_source",
              event.target.value as GenerationSpec["elevation_source"],
            )
          }
        >
          <option value="mapzen">Mapzen · global coverage</option>
          <option value="mapterhorn">
            Mapterhorn · higher detail where available
          </option>
        </select>
        <small>
          Mapterhorn uses 512 px Terrarium tiles and falls back to its
          global layer outside regional high-detail coverage.
        </small>
      </label>
      {spec.elevation_source === "mapterhorn" && (
        <label className="adjacent-interlock-toggle fine-dem-toggle">
          <input
            type="checkbox"
            checked={spec.fine_dem_detail}
            disabled={
              spec.ground_span_km > FINE_DEM_MAX_SPAN_KM ||
              spec.mesh_samples_across === MAX_ASSEMBLED_SAMPLES
            }
            onChange={(event) =>
              update("fine_dem_detail", event.target.checked)
            }
          />
          Use finest available DEM detail
          <small>
            {spec.mesh_samples_across === MAX_ASSEMBLED_SAMPLES
              ? "Ultra already uses the maximum 2,048-sample budget."
              : spec.ground_span_km > FINE_DEM_MAX_SPAN_KM
                ? "Zoom to 2 km or less to enable the fine-detail budget."
                : "Increase the selected budget toward a 0.25 m target when Mapterhorn has finer tiles, up to 2,048 samples."}
          </small>
        </label>
      )}

      <label className="adjacent-interlock-toggle fine-dem-toggle">
        <input
          type="checkbox"
          checked={spec.despike_terrain}
          onChange={(event) => update("despike_terrain", event.target.checked)}
        />
        Repair stray elevation readings
        <small>
          Published tiles carry the odd bad pixel, often along a coastline or
          lake shore. Left in, one reading thousands of metres out flattens the
          whole model. Turn off to build the elevation data exactly as supplied.
        </small>
      </label>

      <RangeField
        label="Ground span"
        value={spec.ground_span_km}
        unit=" km"
        min={0.25}
        max={80}
        step={0.25}
        onChange={(value) => update("ground_span_km", value)}
      />
      <RangeField
        label="Terrain rotation"
        ariaLabel="Terrain rotation clockwise from north"
        value={spec.terrain_rotation_degrees}
        unit="°"
        min={-180}
        max={180}
        step={0.1}
        onChange={(value) => update("terrain_rotation_degrees", value)}
        note="Clockwise from north. This rotates the sampled terrain and every geographic overlay before mesh generation."
      />
      <RangeField
        label="Print width"
        value={spec.width_mm}
        unit=" mm"
        min={80}
        max={300}
        step={5}
        onChange={(value) => update("width_mm", value)}
      />
      <label className="road-detail-field">
        Vertical scale
        <select
          value={spec.height_mode}
          onChange={(event) =>
            setHeightMode(
              event.target.value as GenerationSpec["height_mode"],
            )
          }
        >
          <option value="overall_height">Overall height · fit this area</option>
          <option value="multiplier">Multiplier · fixed exaggeration</option>
        </select>
        <small>
          Fitting the height fills the model whatever the terrain does, so
          two areas print the same height. A multiplier holds one vertical
          scale instead and lets the height follow the ground, so separate
          areas — and separately generated tiles — stay comparable.
        </small>
      </label>
      {spec.height_mode === "overall_height" ? (
        <RangeField
          label="Terrain height"
          value={spec.relief_mm}
          unit=" mm"
          min={3}
          max={80}
          step={1}
          onChange={(value) => update("relief_mm", value)}
          note={
            heightScaleReadout
              ? `About ${heightScaleReadout.exaggeration.toFixed(1)}× vertical exaggeration here.`
              : undefined
          }
        />
      ) : (
        <RangeField
          label="Vertical multiplier"
          value={spec.vertical_exaggeration}
          unit="×"
          min={0.05}
          max={200}
          step={0.05}
          onChange={(value) => update("vertical_exaggeration", value)}
          note={
            heightScaleReadout
              ? `This area prints about ${heightScaleReadout.height.toFixed(1)} mm tall.${
                  heightScaleReadout.height > 80
                    ? " Taller than the usual 80 mm limit — check it fits your printer."
                    : ""
                }`
              : "The model's height follows the terrain; the height slider does not apply."
          }
        />
      )}
      <label className="road-detail-field">
        Height measured from
        <select
          value={spec.datum_reference}
          onChange={(event) =>
            update(
              "datum_reference",
              event.target.value as GenerationSpec["datum_reference"],
            )
          }
        >
          <option value="area_minimum">This area&apos;s lowest ground</option>
          <option value="sea_level">Sea level</option>
          <option value="custom">A set elevation</option>
        </select>
        <small>
          Where the print&apos;s zero sits. The area&apos;s lowest ground
          spends the whole height on the relief that is there; sea level is
          shared by every model without coordinating. A super-tile measures
          from the lowest ground across all its tiles.
        </small>
      </label>
      {spec.datum_reference === "custom" && (
        <RangeField
          label="Datum elevation"
          value={spec.custom_datum_m}
          unit=" m"
          min={-500}
          max={5000}
          step={10}
          onChange={(value) => update("custom_datum_m", value)}
          note="Ground below this keeps its relief — the datum drops to the real minimum rather than cutting terrain off under the base."
        />
      )}
      <RangeField
        label="Minimum piece height"
        value={spec.base_mm}
        unit=" mm"
        min={1}
        max={20}
        step={0.2}
        onChange={(value) => update("base_mm", value)}
      />
      <fieldset className="mesh-quality">
        <legend>
          Mesh detail
          <span>
            {assembledMeshSamples(spec)} across · about{" "}
            {formatGroundSpacing(groundMeshSpacing(spec))} m ground
            spacing
          </span>
        </legend>
        <div role="radiogroup" aria-label="Mesh detail">
          {MESH_QUALITY_OPTIONS.map((option) => (
            <button
              key={option.samples}
              type="button"
              role="radio"
              aria-checked={
                spec.mesh_samples_across === option.samples
              }
              className={
                spec.mesh_samples_across === option.samples
                  ? "active"
                  : ""
              }
              onClick={() => setMeshQuality(option.samples)}
            >
              <strong>{option.label}</strong>
              <span>{option.samples}</span>
              <small>{option.note}</small>
            </button>
          ))}
        </div>
        {spec.mesh_samples_across === MAX_ASSEMBLED_SAMPLES && (
          <p>
            Ultra produces about four times as many surface triangles as
            High and can make large 3MF files.
          </p>
        )}
      </fieldset>
      {!spec.solid_model && (
        <RangeField
          label="Fit clearance"
          value={spec.clearance_mm}
          unit=" mm"
          min={0}
          max={0.4}
          step={0.02}
          onChange={(value) => update("clearance_mm", value)}
        />
      )}
    </section>
  );
}
