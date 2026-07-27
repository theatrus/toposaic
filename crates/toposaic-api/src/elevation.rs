use std::{
    collections::{BTreeSet, HashMap, HashSet},
    fs,
    path::{Path, PathBuf},
    time::Duration,
};

use anyhow::{Context, Result, bail};
use image::{ImageFormat, RgbImage};
use reqwest::{StatusCode, blocking::Client};
use toposaic_core::{DespikeReport, ElevationSource, GenerationSpec, HeightField};
use tracing::warn;

use crate::{cache, geo::GeoTransform, http};

#[cfg(test)]
use crate::geo::normalize_longitude;

const EARTH_CIRCUMFERENCE_M: f64 = 40_075_016.686;
const SOURCE_SAMPLES_PER_MESH_INTERVAL: f64 = 2.0;
const FINE_DEM_TARGET_RESOLUTION_M: f64 = 0.25;
const DETAIL_SAMPLE_STEP: u32 = 8;

#[derive(Debug, Clone, Copy)]
struct ElevationProvider {
    source: ElevationSource,
    name: &'static str,
    base_url: &'static str,
    extension: &'static str,
    image_format: ImageFormat,
    tile_size: u32,
    minimum_zoom: u8,
    maximum_zoom: u8,
    attribution_url: &'static str,
}

impl ElevationProvider {
    fn for_source(source: ElevationSource) -> Self {
        match source {
            ElevationSource::Mapzen => Self {
                source,
                name: "Mapzen Terrain Tiles on AWS",
                base_url: "https://s3.amazonaws.com/elevation-tiles-prod/terrarium",
                extension: "png",
                image_format: ImageFormat::Png,
                tile_size: 256,
                minimum_zoom: 5,
                maximum_zoom: 14,
                attribution_url: "https://github.com/tilezen/joerd/blob/master/docs/attribution.md",
            },
            ElevationSource::Mapterhorn => Self {
                source,
                name: "Mapterhorn",
                base_url: "https://tiles.mapterhorn.com",
                extension: "webp",
                image_format: ImageFormat::WebP,
                tile_size: 512,
                minimum_zoom: 0,
                maximum_zoom: 17,
                attribution_url: "https://mapterhorn.com/attribution",
            },
        }
    }

    fn allows_parent_fallback(self) -> bool {
        self.source == ElevationSource::Mapterhorn
    }

    fn tile_url(self, zoom: u8, x: u32, y: u32) -> String {
        format!("{}/{zoom}/{x}/{y}.{}", self.base_url, self.extension)
    }

    fn source_description(self, requested_zoom: u8, used_zooms: &BTreeSet<u8>) -> String {
        if !self.allows_parent_fallback() {
            return format!(
                "{}, Terrarium z{requested_zoom}; attribution: {}",
                self.name, self.attribution_url
            );
        }
        let used = match (used_zooms.first(), used_zooms.last()) {
            (Some(first), Some(last)) if first == last => format!("z{first}"),
            (Some(first), Some(last)) => format!("z{first}-z{last}"),
            _ => "no tiles".into(),
        };
        format!(
            "{}, Terrarium requested z{requested_zoom}, used {used} with lower-zoom Mapterhorn \
             fallback outside regional coverage; attribution: {}",
            self.name, self.attribution_url
        )
    }
}

pub fn fetch_height_field_with_progress(
    spec: &GenerationSpec,
    cache_dir: &Path,
    on_progress: impl FnMut(f32) -> Result<()>,
) -> Result<HeightField> {
    let samples = available_samples_per_piece(spec, cache_dir)?;
    let (sample_width, sample_height) = spec.sample_grid_dimensions(samples);
    fetch_height_field_at_size(spec, cache_dir, sample_width, sample_height, on_progress)
}

pub fn fetch_preview_height_field(
    spec: &GenerationSpec,
    cache_dir: &Path,
    size: usize,
) -> Result<HeightField> {
    let size = size.clamp(32, 128);
    fetch_height_field_at_size(spec, cache_dir, size, size, |_| Ok(()))
}

fn fetch_height_field_at_size(
    spec: &GenerationSpec,
    cache_dir: &Path,
    sample_width: usize,
    sample_height: usize,
    mut on_progress: impl FnMut(f32) -> Result<()>,
) -> Result<HeightField> {
    let provider = ElevationProvider::for_source(spec.elevation_source);
    let requested_zoom = choose_zoom(spec, sample_width.max(sample_height), provider);
    let client = elevation_client()?;
    let mut tiles = HashMap::new();
    let mut missing_tiles = HashSet::new();
    let transform = GeoTransform::new(
        spec.center_lat,
        spec.center_lon,
        spec.ground_span_km,
        spec.terrain_rotation_degrees,
    );
    let mut values_m = Vec::with_capacity(sample_width * sample_height);
    let mut sampler = ElevationSampler {
        client: &client,
        cache_dir,
        tiles: &mut tiles,
        missing_tiles: &mut missing_tiles,
        provider,
        used_zooms: BTreeSet::new(),
        repair_tiles: spec.despike_terrain,
        repairs: TileRepairTally::default(),
    };

    for row in 0..sample_height {
        let v = row as f64 / (sample_height - 1) as f64;
        for column in 0..sample_width {
            let u = column as f64 / (sample_width - 1) as f64;
            let (latitude, longitude) = transform.coordinate_at_uv(u, v);
            values_m.push(sampler.sample(requested_zoom, longitude, latitude)?);
        }
        on_progress((row + 1) as f32 / sample_height as f32)?;
    }

    let source = provider.source_description(requested_zoom, &sampler.used_zooms);
    let repairs = sampler.repairs;
    let mut field = HeightField::new(sample_width, sample_height, values_m, source)?;
    if !repairs.is_empty() {
        field.source.push_str(&describe_tile_repairs(&repairs));
    }
    // A second pass over the finished grid, as a backstop. Tiles are repaired at
    // their own resolution, which is where a stray reading is one pixel wide and
    // can be recognised, so this should find nothing. It still earns its place
    // against damage too broad to judge pixel by pixel.
    if spec.despike_terrain {
        let spacing_m = sample_spacing_m(spec, sample_width, sample_height);
        let report = field.despike(spacing_m);
        if !report.is_empty() {
            field.source.push_str(&describe_despike(&report));
        }
    }
    Ok(field)
}

/// The distance on the ground between neighbouring samples, taken on the
/// coarser axis.
///
/// Erring coarse raises the bar for calling a reading bad, which is the safe
/// direction: the pass should let a doubtful reading through rather than
/// reshape real ground.
fn sample_spacing_m(spec: &GenerationSpec, sample_width: usize, sample_height: usize) -> f32 {
    let span_m = (spec.ground_span_km * 1_000.0) as f32;
    let across = (sample_width.max(2) - 1) as f32;
    let down = (sample_height.max(2) - 1) as f32;
    (span_m / across).max(span_m / down)
}

fn describe_tile_repairs(tally: &TileRepairTally) -> String {
    format!(
        "; repaired {} stray tile {} across {} {} at source resolution, the worst standing {:.0} m \
         clear of its neighbours",
        tally.readings,
        if tally.readings == 1 {
            "reading"
        } else {
            "readings"
        },
        tally.tiles,
        if tally.tiles == 1 { "tile" } else { "tiles" },
        tally.widest_distance_m,
    )
}

fn describe_despike(report: &DespikeReport) -> String {
    format!(
        "; despiked {} isolated elevation {} over {} {}, the worst standing {:.0} m clear of its \
         neighbours against a {:.0} m bar",
        report.replaced,
        if report.replaced == 1 {
            "reading"
        } else {
            "readings"
        },
        report.passes,
        if report.passes == 1 { "pass" } else { "passes" },
        report.widest_distance_m,
        report.threshold_m,
    )
}

fn elevation_client() -> Result<Client> {
    http::blocking_client(Duration::from_secs(20)).context("build elevation HTTP client")
}

fn available_samples_per_piece(spec: &GenerationSpec, cache_dir: &Path) -> Result<u32> {
    let requested = spec.effective_samples_per_piece();
    if !spec.fine_dem_detail_active() {
        return Ok(requested);
    }

    let provider = ElevationProvider::for_source(spec.elevation_source);
    let client = elevation_client()?;
    let used_zoom = highest_available_zoom_at_center(spec, cache_dir, &client, provider)?;
    Ok(samples_per_piece_for_available_zoom(
        spec, requested, used_zoom, provider,
    ))
}

fn samples_per_piece_for_available_zoom(
    spec: &GenerationSpec,
    requested: u32,
    used_zoom: u8,
    provider: ElevationProvider,
) -> u32 {
    let delivered_resolution_m = source_resolution_m(spec.center_lat, used_zoom, provider);
    let useful_resolution_m = delivered_resolution_m.max(FINE_DEM_TARGET_RESOLUTION_M);
    let useful_total = (spec.ground_span_km * 1_000.0 / useful_resolution_m).ceil() as u32;
    let piece_count = if spec.solid_model {
        1
    } else {
        spec.rows.max(spec.columns)
    };
    let useful_per_piece = useful_total
        .div_ceil(piece_count)
        .div_ceil(DETAIL_SAMPLE_STEP)
        .saturating_mul(DETAIL_SAMPLE_STEP);
    let mut standard_spec = spec.clone();
    standard_spec.fine_dem_detail = false;
    let standard = standard_spec.effective_samples_per_piece();
    requested.min(useful_per_piece.max(standard))
}

fn highest_available_zoom_at_center(
    spec: &GenerationSpec,
    cache_dir: &Path,
    client: &Client,
    provider: ElevationProvider,
) -> Result<u8> {
    for zoom in (provider.minimum_zoom..=provider.maximum_zoom).rev() {
        let location = tile_location(provider.tile_size, zoom, spec.center_lon, spec.center_lat);
        if load_tile(
            client,
            cache_dir,
            provider,
            zoom,
            location.tile_x,
            location.tile_y,
        )?
        .is_some()
        {
            return Ok(zoom);
        }
    }
    bail!(
        "{} has no elevation tile at the selected center",
        provider.name
    )
}

fn source_resolution_m(latitude: f64, zoom: u8, provider: ElevationProvider) -> f64 {
    EARTH_CIRCUMFERENCE_M * latitude.to_radians().cos().abs().max(0.1)
        / (f64::from(provider.tile_size) * f64::from(1_u32 << zoom))
}

fn choose_zoom(spec: &GenerationSpec, samples: usize, provider: ElevationProvider) -> u8 {
    let target_resolution_m =
        spec.ground_span_km * 1_000.0 / (samples.saturating_sub(1).max(1)) as f64;
    let source_resolution_m = target_resolution_m / SOURCE_SAMPLES_PER_MESH_INTERVAL;
    let latitude_scale = spec.center_lat.to_radians().cos().abs().max(0.1);
    let desired = (EARTH_CIRCUMFERENCE_M * latitude_scale
        / (f64::from(provider.tile_size) * source_resolution_m.max(0.1)))
    .log2()
    .ceil() as i32;
    desired.clamp(
        i32::from(provider.minimum_zoom),
        i32::from(provider.maximum_zoom),
    ) as u8
}

/// One elevation tile, decoded to metres once and repaired at its own
/// resolution.
///
/// Repairing here rather than on the finished model is what reaches a close
/// view. A stray reading is one pixel wide in the tile whatever the model asks
/// for, but a model spacing its samples below the width of a pixel spreads that
/// one reading over several samples, where it reads as a block too wide to
/// judge. Doing it once per tile also drops the per-sample decode: a model reads
/// every pixel many times over.
struct ElevationTile {
    size: u32,
    values_m: Vec<f32>,
}

impl ElevationTile {
    fn prepare(image: &RgbImage, repair_spacing_m: Option<f32>) -> Result<(Self, DespikeReport)> {
        let values_m = image
            .pixels()
            .map(|pixel| decode_terrarium_pixel(pixel.0))
            .collect();
        let mut grid = HeightField::new(
            image.width() as usize,
            image.height() as usize,
            values_m,
            "tile",
        )
        .context("read an elevation tile as a grid of metres")?;
        let report = match repair_spacing_m {
            Some(spacing_m) => grid.despike(spacing_m),
            None => DespikeReport::default(),
        };
        Ok((
            Self {
                size: image.width(),
                values_m: grid.values_m,
            },
            report,
        ))
    }

    fn elevation_m(&self, x: u32, y: u32) -> f32 {
        self.values_m[(y * self.size + x) as usize]
    }
}

/// What repairing tiles changed, gathered over every tile a model reads.
#[derive(Debug, Clone, Copy, Default)]
struct TileRepairTally {
    tiles: usize,
    readings: usize,
    widest_distance_m: f32,
}

impl TileRepairTally {
    fn add(&mut self, report: &DespikeReport) {
        if report.is_empty() {
            return;
        }
        self.tiles += 1;
        self.readings += report.replaced;
        self.widest_distance_m = self.widest_distance_m.max(report.widest_distance_m);
    }

    fn is_empty(&self) -> bool {
        self.readings == 0
    }
}

/// The latitude through the middle of a tile row, for sizing one of its pixels
/// on the ground.
fn tile_row_latitude(tile_size: u32, zoom: u8, tile_y: u32) -> f64 {
    let total_pixels = f64::from(tile_size) * f64::from(1_u32 << zoom);
    let centre = (f64::from(tile_y) + 0.5) * f64::from(tile_size);
    let scaled = 1.0 - 2.0 * (centre / total_pixels);
    (std::f64::consts::PI * scaled).sinh().atan().to_degrees()
}

struct ElevationSampler<'a> {
    client: &'a Client,
    cache_dir: &'a Path,
    tiles: &'a mut HashMap<(u8, u32, u32), ElevationTile>,
    missing_tiles: &'a mut HashSet<(u8, u32, u32)>,
    provider: ElevationProvider,
    used_zooms: BTreeSet<u8>,
    repair_tiles: bool,
    repairs: TileRepairTally,
}

impl ElevationSampler<'_> {
    fn sample(&mut self, requested_zoom: u8, longitude: f64, latitude: f64) -> Result<f32> {
        let zoom = self.zoom_holding_data(requested_zoom, longitude, latitude)?;
        let (global_x, global_y) =
            global_pixel_position(self.provider.tile_size, zoom, longitude, latitude);
        sample_lattice(global_x, global_y, |x, y| {
            self.sample_global_pixel(zoom, x, y).map(|(value, _)| value)
        })
    }

    /// The highest zoom at or below `requested_zoom` that really has a tile
    /// over this point.
    ///
    /// Interpolating on a finer lattice than the data actually holds is not
    /// harmless: with parent fallback every neighbour in the finer lattice
    /// collapses onto the same parent pixel, the interpolation weights cancel,
    /// and the surface comes out as flat parent-sized plateaus with hard steps
    /// between them.
    fn zoom_holding_data(
        &mut self,
        requested_zoom: u8,
        longitude: f64,
        latitude: f64,
    ) -> Result<u8> {
        if !self.provider.allows_parent_fallback() {
            return Ok(requested_zoom);
        }
        let (global_x, global_y) =
            global_pixel_position(self.provider.tile_size, requested_zoom, longitude, latitude);
        let (_, zoom) = self.sample_global_pixel(
            requested_zoom,
            global_x.floor() as i64,
            global_y.floor() as i64,
        )?;
        Ok(zoom)
    }

    /// The elevation at one pixel of the `ceiling_zoom` lattice, with the zoom
    /// that supplied it.
    fn sample_global_pixel(
        &mut self,
        ceiling_zoom: u8,
        global_x: i64,
        global_y: i64,
    ) -> Result<(f32, u8)> {
        let minimum_zoom = if self.provider.allows_parent_fallback() {
            self.provider.minimum_zoom
        } else {
            ceiling_zoom
        };
        let ceiling_total_pixels = i64::from(self.provider.tile_size) * (1_i64 << ceiling_zoom);
        let global_x = global_x.rem_euclid(ceiling_total_pixels);
        let global_y = global_y.clamp(0, ceiling_total_pixels - 1);
        for zoom in (minimum_zoom..=ceiling_zoom).rev() {
            let scale = 1_i64 << (ceiling_zoom - zoom);
            let pixel_x = global_x / scale;
            let pixel_y = global_y / scale;
            let tile_size = i64::from(self.provider.tile_size);
            let location = TileLocation {
                tile_x: (pixel_x / tile_size) as u32,
                tile_y: (pixel_y / tile_size) as u32,
                pixel_x: (pixel_x % tile_size) as u32,
                pixel_y: (pixel_y % tile_size) as u32,
            };
            let key = (zoom, location.tile_x, location.tile_y);
            if self.missing_tiles.contains(&key) {
                continue;
            }
            // Sized on this tile's own row, since a pixel covers less ground the
            // further from the equator it sits.
            let repair_spacing_m = self.repair_tiles.then(|| {
                source_resolution_m(
                    tile_row_latitude(self.provider.tile_size, zoom, location.tile_y),
                    zoom,
                    self.provider,
                ) as f32
            });
            let mut fresh_repairs = None;
            if let std::collections::hash_map::Entry::Vacant(entry) = self.tiles.entry(key) {
                match load_tile(
                    self.client,
                    self.cache_dir,
                    self.provider,
                    zoom,
                    location.tile_x,
                    location.tile_y,
                )? {
                    Some(image) => {
                        let (tile, report) = ElevationTile::prepare(&image, repair_spacing_m)?;
                        entry.insert(tile);
                        fresh_repairs = Some(report);
                    }
                    None => {
                        self.missing_tiles.insert(key);
                        continue;
                    }
                }
            }
            if let Some(report) = fresh_repairs {
                self.repairs.add(&report);
            }
            let elevation_m = self
                .tiles
                .get(&key)
                .context("elevation tile cache lost a tile")?
                .elevation_m(location.pixel_x, location.pixel_y);
            self.used_zooms.insert(zoom);
            return Ok((elevation_m, zoom));
        }
        bail!(
            "{} has no elevation tile for this point at or below z{ceiling_zoom}",
            self.provider.name
        )
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct TileLocation {
    tile_x: u32,
    tile_y: u32,
    pixel_x: u32,
    pixel_y: u32,
}

fn tile_location(tile_size: u32, zoom: u8, longitude: f64, latitude: f64) -> TileLocation {
    let (global_x, global_y) = global_pixel_position(tile_size, zoom, longitude, latitude);
    let tile_count = 1_u32 << zoom;
    let total_pixels = f64::from(tile_size) * f64::from(tile_count);
    let global_x = global_x.floor().rem_euclid(total_pixels) as u32;
    let global_y = global_y.floor().clamp(0.0, total_pixels - 1.0) as u32;
    TileLocation {
        tile_x: global_x / tile_size,
        tile_y: global_y / tile_size,
        pixel_x: global_x % tile_size,
        pixel_y: global_y % tile_size,
    }
}

fn global_pixel_position(tile_size: u32, zoom: u8, longitude: f64, latitude: f64) -> (f64, f64) {
    let tile_count = 1_u32 << zoom;
    let x = (longitude + 180.0) / 360.0 * tile_count as f64;
    let latitude_radians = latitude.clamp(-85.051_128_78, 85.051_128_78).to_radians();
    let y = (1.0
        - (latitude_radians.tan() + 1.0 / latitude_radians.cos()).ln() / std::f64::consts::PI)
        / 2.0
        * tile_count as f64;
    (x * f64::from(tile_size), y * f64::from(tile_size))
}

fn decode_terrarium_pixel(pixel: [u8; 3]) -> f32 {
    pixel[0] as f32 * 256.0 + pixel[1] as f32 + pixel[2] as f32 / 256.0 - 32_768.0
}

/// Reads the 4 by 4 source pixels around `(x, y)` and interpolates between
/// them.
///
/// `x` and `y` are pixel coordinates on the source lattice, where whole
/// numbers fall on pixel edges and pixel centres sit at the half-way points.
fn sample_lattice(x: f64, y: f64, mut source: impl FnMut(i64, i64) -> Result<f32>) -> Result<f32> {
    let centered_x = x - 0.5;
    let centered_y = y - 0.5;
    let x0 = centered_x.floor() as i64;
    let y0 = centered_y.floor() as i64;
    let tx = (centered_x - x0 as f64) as f32;
    let ty = (centered_y - y0 as f64) as f32;
    let mut neighbourhood = [[0.0f32; 4]; 4];
    for (row_index, row) in neighbourhood.iter_mut().enumerate() {
        for (column_index, value) in row.iter_mut().enumerate() {
            *value = source(x0 + column_index as i64 - 1, y0 + row_index as i64 - 1)?;
        }
    }
    Ok(catmull_rom_patch(neighbourhood, tx, ty))
}

/// Catmull-Rom over a 4 by 4 neighbourhood, clamped to that neighbourhood's
/// own range.
///
/// Bilinear interpolation is continuous but its slope is not: it is flat
/// inside every source pixel and bends only at pixel edges, which prints as a
/// rectilinear grid of creases once the model samples finer than the source.
/// Catmull-Rom passes through every source pixel and keeps its slope
/// continuous, so the grid disappears. The clamp holds the classic cubic
/// overshoot at cliffs: without it a ridge or dam edge grows a lip taller than
/// any real reading nearby.
fn catmull_rom_patch(neighbourhood: [[f32; 4]; 4], tx: f32, ty: f32) -> f32 {
    let mut rows = [0.0f32; 4];
    for (value, row) in rows.iter_mut().zip(neighbourhood.iter()) {
        *value = catmull_rom(*row, tx);
    }
    let interpolated = catmull_rom(rows, ty);
    let mut minimum = f32::INFINITY;
    let mut maximum = f32::NEG_INFINITY;
    for value in neighbourhood.iter().flatten() {
        minimum = minimum.min(*value);
        maximum = maximum.max(*value);
    }
    interpolated.clamp(minimum, maximum)
}

/// The uniform Catmull-Rom spline through `values[1]` and `values[2]`, at
/// `t` between them. It reproduces straight and quadratic runs of samples
/// exactly and joins neighbouring spans with a matching slope.
fn catmull_rom(values: [f32; 4], t: f32) -> f32 {
    let [p0, p1, p2, p3] = values;
    let a = -0.5 * p0 + 1.5 * p1 - 1.5 * p2 + 0.5 * p3;
    let b = p0 - 2.5 * p1 + 2.0 * p2 - 0.5 * p3;
    let c = -0.5 * p0 + 0.5 * p2;
    ((a * t + b) * t + c) * t + p1
}

fn load_tile(
    client: &Client,
    cache_dir: &Path,
    provider: ElevationProvider,
    zoom: u8,
    x: u32,
    y: u32,
) -> Result<Option<RgbImage>> {
    let path = cache_path(cache_dir, provider, zoom, x, y);
    if path.is_file() {
        let bytes =
            fs::read(&path).with_context(|| format!("read cached tile {}", path.display()))?;
        // A corrupt cached tile must not fail every future job in the area:
        // drop it and fall through to a fresh download.
        match decode_tile(&bytes, provider, zoom, x, y) {
            Ok(image) => return Ok(Some(image)),
            Err(error) => {
                warn!(%error, tile = %path.display(), "cached elevation tile is corrupt; refetching");
                fs::remove_file(&path)
                    .with_context(|| format!("remove corrupt tile {}", path.display()))?;
            }
        }
    }

    let response = client
        .get(provider.tile_url(zoom, x, y))
        .send()
        .with_context(|| format!("download elevation tile {zoom}/{x}/{y}"))?;
    if response.status() == StatusCode::NOT_FOUND && provider.allows_parent_fallback() {
        return Ok(None);
    }
    if !response.status().is_success() {
        bail!(
            "{} elevation tile {zoom}/{x}/{y} returned {}",
            provider.name,
            response.status()
        );
    }
    let bytes = response.bytes()?.to_vec();
    let image = decode_tile(&bytes, provider, zoom, x, y)?;
    cache::store(&path, &bytes)
        .with_context(|| format!("cache elevation tile {}", path.display()))?;
    Ok(Some(image))
}

fn decode_tile(
    bytes: &[u8],
    provider: ElevationProvider,
    zoom: u8,
    x: u32,
    y: u32,
) -> Result<RgbImage> {
    let image = image::load_from_memory_with_format(bytes, provider.image_format)
        .with_context(|| format!("decode elevation tile {zoom}/{x}/{y}"))?
        .to_rgb8();
    if image.width() != provider.tile_size || image.height() != provider.tile_size {
        bail!(
            "{} elevation tile {zoom}/{x}/{y} has unexpected size {}x{}",
            provider.name,
            image.width(),
            image.height()
        );
    }
    Ok(image)
}

fn cache_path(cache_dir: &Path, provider: ElevationProvider, zoom: u8, x: u32, y: u32) -> PathBuf {
    let source_dir = match provider.source {
        ElevationSource::Mapzen => cache_dir.to_path_buf(),
        ElevationSource::Mapterhorn => cache_dir.join("mapterhorn"),
    };
    source_dir
        .join(zoom.to_string())
        .join(x.to_string())
        .join(format!("{y}.{}", provider.extension))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn source_options_use_their_native_tile_formats_and_limits() {
        let mapzen = ElevationProvider::for_source(ElevationSource::Mapzen);
        let mapterhorn = ElevationProvider::for_source(ElevationSource::Mapterhorn);

        assert_eq!(mapzen.tile_size, 256);
        assert_eq!(mapzen.maximum_zoom, 14);
        assert_eq!(mapzen.extension, "png");
        assert_eq!(mapterhorn.tile_size, 512);
        assert_eq!(mapterhorn.maximum_zoom, 17);
        assert_eq!(mapterhorn.extension, "webp");
        assert!(mapterhorn.allows_parent_fallback());
        let used_zooms = BTreeSet::from([13]);
        assert!(
            mapterhorn
                .source_description(16, &used_zooms)
                .contains("Mapterhorn")
        );
        assert!(
            mapterhorn
                .source_description(16, &used_zooms)
                .contains("requested z16, used z13")
        );
        assert!(
            mapterhorn
                .source_description(16, &used_zooms)
                .contains("https://mapterhorn.com/attribution")
        );
    }

    /// Encodes metres back into a Terrarium pixel, so a synthetic tile can be
    /// built the way a real one arrives.
    fn terrarium_pixel(elevation_m: f32) -> [u8; 3] {
        let raw = ((elevation_m + 32_768.0) * 256.0).round().max(0.0) as u32;
        [
            ((raw >> 16) & 0xFF) as u8,
            ((raw >> 8) & 0xFF) as u8,
            (raw & 0xFF) as u8,
        ]
    }

    fn tile_with_reading_at(size: u32, x: u32, y: u32, elevation_m: f32) -> RgbImage {
        RgbImage::from_fn(size, size, |column, row| {
            let value = if (column, row) == (x, y) {
                elevation_m
            } else {
                500.0 + column as f32 * 0.5 + row as f32 * 0.25
            };
            image::Rgb(terrarium_pixel(value))
        })
    }

    #[test]
    fn tiles_are_repaired_at_their_own_resolution() {
        let image = tile_with_reading_at(32, 10, 12, -6827.9);

        let (tile, report) = ElevationTile::prepare(&image, Some(7.8)).unwrap();

        assert_eq!(report.replaced, 1);
        // The repaired reading sits with its neighbours, not thousands below.
        assert!(
            (tile.elevation_m(10, 12) - 508.0).abs() < 2.0,
            "repaired to {}",
            tile.elevation_m(10, 12)
        );
        // Everything else is left as supplied.
        assert!((tile.elevation_m(9, 12) - 507.5).abs() < 0.1);
    }

    /// The point of repairing tiles rather than the finished model: the fault is
    /// one pixel wide here whatever the model later asks for.
    #[test]
    fn tiles_are_left_alone_when_the_repair_is_off() {
        let image = tile_with_reading_at(32, 10, 12, -6827.9);

        let (tile, report) = ElevationTile::prepare(&image, None).unwrap();

        assert!(report.is_empty());
        assert!(tile.elevation_m(10, 12) < -6000.0);
    }

    #[test]
    fn a_tile_pixel_shrinks_with_latitude() {
        let provider = ElevationProvider::for_source(ElevationSource::Mapzen);
        // Hakone sits in this z14 tile row; a pixel there covers about 7.8 m.
        let latitude = tile_row_latitude(provider.tile_size, 14, 6477);
        assert!(
            (latitude - 35.24).abs() < 0.2,
            "tile row centre came out at {latitude}"
        );
        let metres = source_resolution_m(latitude, 14, provider);
        assert!((metres - 7.8).abs() < 0.2, "pixel came out {metres} m");
        // Nearer the pole the same pixel covers less ground.
        let polar = source_resolution_m(
            tile_row_latitude(provider.tile_size, 14, 2000),
            14,
            provider,
        );
        assert!(polar < metres, "{polar} m should be under {metres} m");
    }

    #[test]
    fn zoom_stays_in_each_source_range() {
        let spec = GenerationSpec::default();
        for source in [ElevationSource::Mapzen, ElevationSource::Mapterhorn] {
            let provider = ElevationProvider::for_source(source);
            let zoom = choose_zoom(&spec, 85, provider);
            assert!((provider.minimum_zoom..=provider.maximum_zoom).contains(&zoom));
        }
    }

    #[test]
    fn closer_views_request_finer_source_tiles() {
        let provider = ElevationProvider::for_source(ElevationSource::Mapterhorn);
        let wide = GenerationSpec::default();
        let close = GenerationSpec {
            ground_span_km: wide.ground_span_km / 4.0,
            ..wide.clone()
        };

        let wide_zoom = choose_zoom(&wide, 128, provider);
        let close_zoom = choose_zoom(&close, 128, provider);

        assert_eq!(close_zoom, wide_zoom + 2);
    }

    #[test]
    fn source_zoom_oversamples_mesh_intervals() {
        let provider = ElevationProvider::for_source(ElevationSource::Mapterhorn);
        let spec = GenerationSpec::default();
        let samples = 128;
        let zoom = choose_zoom(&spec, samples, provider);
        let mesh_interval_m = spec.ground_span_km * 1_000.0 / (samples - 1) as f64;
        let source_interval_m = EARTH_CIRCUMFERENCE_M * spec.center_lat.to_radians().cos().abs()
            / (f64::from(provider.tile_size) * f64::from(1_u32 << zoom));

        assert!(source_interval_m <= mesh_interval_m / SOURCE_SAMPLES_PER_MESH_INTERVAL);
    }

    #[test]
    fn fine_detail_tracks_the_available_tile_grid_without_exceeding_quarter_metre_target() {
        let provider = ElevationProvider::for_source(ElevationSource::Mapterhorn);
        let mut spec = GenerationSpec {
            center_lat: 75.0,
            ground_span_km: 0.5,
            elevation_source: ElevationSource::Mapterhorn,
            fine_dem_detail: true,
            solid_model: true,
            ..GenerationSpec::default()
        };
        let requested = spec.effective_samples_per_piece();
        assert_eq!(requested, 2_000);
        assert_eq!(
            samples_per_piece_for_available_zoom(&spec, requested, 17, provider),
            2_000
        );

        spec.center_lat = 46.8523;
        let rainier = samples_per_piece_for_available_zoom(&spec, requested, 17, provider);
        assert!((1_200..2_000).contains(&rainier));

        let fallback = samples_per_piece_for_available_zoom(&spec, requested, 13, provider);
        assert_eq!(fallback, 1_024);
    }

    /// Reads a synthetic source lattice the way the sampler reads tiles.
    fn lattice(field: impl Fn(i64, i64) -> f32, x: f64, y: f64) -> f32 {
        sample_lattice(x, y, |pixel_x, pixel_y| Ok(field(pixel_x, pixel_y))).unwrap()
    }

    #[test]
    fn elevation_pixels_blend_in_both_axes() {
        // A plane through the pixel centres comes back exactly, in both axes.
        let plane = |x: i64, y: i64| 3.0 * x as f32 + 7.0 * y as f32;
        for (x, y) in [(4.5, 6.5), (4.75, 6.25), (5.0, 7.0), (5.5, 6.9)] {
            let expected = 3.0 * (x as f32 - 0.5) + 7.0 * (y as f32 - 0.5);
            assert!((lattice(plane, x, y) - expected).abs() < 1e-3);
        }
    }

    /// The stair-step grid the user sees is a slope that jumps at every source
    /// pixel edge. Sampling a curved surface far finer than the source must
    /// return an evenly curved profile, not a run of flats hinged at the pixel
    /// edges. Bilinear interpolation fails this: it reads back zero curvature
    /// inside each pixel and a spike of about 0.25 m at every edge.
    #[test]
    fn sampling_finer_than_the_source_keeps_the_slope_continuous() {
        let bowl = |x: i64, y: i64| 0.5 * (x * x) as f32 + 0.25 * (y * y) as f32;
        let steps_per_pixel = 8;
        let step = 1.0 / f64::from(steps_per_pixel);
        let profile = (0..steps_per_pixel * 4)
            .map(|index| lattice(bowl, 3.0 + f64::from(index) * step, 5.25))
            .collect::<Vec<_>>();
        // A quadratic sampled at spacing h has the same second difference
        // everywhere: curvature * h^2.
        let expected = 1.0 * (step * step) as f32;

        for window in profile.windows(3) {
            let second_difference = window[0] - 2.0 * window[1] + window[2];
            assert!(
                (second_difference - expected).abs() < 1e-4,
                "second difference {second_difference} strays from {expected}",
            );
        }
    }

    #[test]
    fn cliffs_do_not_grow_a_lip_above_the_source_readings() {
        let cliff = |x: i64, _: i64| if x < 4 { 0.0 } else { 100.0 };
        // Unclamped, the cubic rings above the plateau it has just climbed on
        // to: the span after the step overshoots by several metres.
        assert!(catmull_rom([0.0, 100.0, 100.0, 100.0], 0.5) > 105.0);

        for index in 0..64 {
            let value = lattice(cliff, 3.0 + f64::from(index) / 16.0, 5.5);
            assert!((0.0..=100.0).contains(&value), "cliff sample {value}");
        }
    }

    #[test]
    fn flat_ground_stays_flat() {
        assert_eq!(lattice(|_, _| 12.5, 4.3, 9.8), 12.5);
    }

    #[test]
    fn mapterhorn_uses_512_pixel_coordinates() {
        let location = tile_location(512, 12, 0.0, 0.0);
        assert_eq!(
            location,
            TileLocation {
                tile_x: 2_048,
                tile_y: 2_048,
                pixel_x: 0,
                pixel_y: 0,
            }
        );
    }

    #[test]
    fn source_caches_do_not_overlap() {
        let root = Path::new("/cache/elevation");
        let mapzen = cache_path(
            root,
            ElevationProvider::for_source(ElevationSource::Mapzen),
            8,
            1,
            2,
        );
        let mapterhorn = cache_path(
            root,
            ElevationProvider::for_source(ElevationSource::Mapterhorn),
            8,
            1,
            2,
        );

        assert_eq!(mapzen, root.join("8/1/2.png"));
        assert_eq!(mapterhorn, root.join("mapterhorn/8/1/2.webp"));
    }

    #[test]
    fn longitude_wraps() {
        assert!((normalize_longitude(181.0) + 179.0).abs() < f64::EPSILON);
        assert!((normalize_longitude(-181.0) - 179.0).abs() < f64::EPSILON);
    }
}
