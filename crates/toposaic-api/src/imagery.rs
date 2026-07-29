//! Sentinel-2 ground imagery for satellite-derived palettes.
//!
//! The ESA WorldCover project publishes annual Sentinel-2 composites next
//! to the land-cover map itself: cloud-masked RGB plus near-infrared at
//! 10 m, on exactly the land-cover lattice, as public Cloud-Optimized
//! GeoTIFFs. The tiles are 1 degree square and hundreds of megabytes, so
//! unlike the land-cover map this module never downloads whole tiles: it
//! reads just the needed windows over HTTP range requests and caches the
//! sampled raster per setup instead of the source tiles.

use std::{path::Path, time::Duration};

use anyhow::{Context, Result, bail};
use geotiff_reader::cog::{HttpGeoTiffFile, HttpOpenOptions};
use tracing::warn;

use crate::{
    cache,
    geo::{GeoTransform, normalize_longitude},
    http,
    surface::select_sampling_overview,
};

const IMAGERY_BASE_URL: &str = "https://esa-worldcover-s2.s3.eu-central-1.amazonaws.com/rgbnir";
const IMAGERY_YEAR: &str = "2021";
const IMAGERY_VERSION: &str = "v200";
pub(crate) const IMAGERY_ATTRIBUTION: &str =
    "Contains modified Copernicus Sentinel data (2021) processed by ESA WorldCover consortium";
/// Sentinel-2 composite tiles are 1 degree square at 12000 pixels, the same
/// 10 m lattice the 3 degree land-cover tiles use.
const IMAGERY_TILE_DEGREES: f64 = 1.0;
const IMAGERY_TILE_PIXELS: u32 = 12_000;
/// Version stamp of the sampled-raster cache format and sampling scheme.
/// Bump it whenever either changes, so stale files miss instead of parse.
const CACHE_MAGIC: &[u8; 4] = b"TSI1";

/// The sampled imagery on the surface raster: four bands of scaled
/// reflectance per sample and a validity flag. `tiles` is every tile the
/// footprint touches; `missing_tiles` the subset that could not be read,
/// whose samples stay invalid.
pub(crate) struct ImageryRaster {
    pub width: usize,
    pub height: usize,
    pub rgbn: Vec<[u16; 4]>,
    pub valid: Vec<bool>,
    pub tiles: Vec<String>,
    pub missing_tiles: Vec<String>,
}

struct SamplePoint {
    output_index: usize,
    longitude: f64,
    latitude: f64,
}

/// Fetches the Sentinel-2 composite reflectance for every sample of a
/// `width` by `height` raster over the transform's footprint. Serves the
/// sampled raster from cache when an identical footprint was sampled
/// before; only a fully covered result is cached, so a transient tile
/// failure is retried next generation instead of freezing a gap in.
pub(crate) fn fetch_ground_imagery(
    transform: &GeoTransform,
    width: usize,
    height: usize,
    cache_dir: &Path,
) -> Result<ImageryRaster> {
    if width < 2 || height < 2 {
        bail!("imagery raster must be at least 2 by 2");
    }
    let mut tiles: Vec<(String, Vec<SamplePoint>)> = Vec::new();
    for row in 0..height {
        let v = row as f64 / (height - 1) as f64;
        for column in 0..width {
            let u = column as f64 / (width - 1) as f64;
            let (latitude, longitude) = transform.coordinate_at_uv(u, v);
            let longitude = normalize_longitude(longitude);
            let name = imagery_tile(longitude, latitude);
            let point = SamplePoint {
                output_index: row * width + column,
                longitude,
                latitude,
            };
            match tiles.iter_mut().find(|(tile, _)| *tile == name) {
                Some((_, points)) => points.push(point),
                None => tiles.push((name, vec![point])),
            }
        }
    }
    tiles.sort_by(|a, b| a.0.cmp(&b.0));
    let tile_names = tiles
        .iter()
        .map(|(name, _)| name.clone())
        .collect::<Vec<_>>();

    let cache_path = cache_dir.join(cache_file_name(transform, width, height));
    if let Some((rgbn, valid)) = load_cached_raster(&cache_path, width, height) {
        return Ok(ImageryRaster {
            width,
            height,
            rgbn,
            valid,
            tiles: tile_names,
            missing_tiles: Vec::new(),
        });
    }

    let mut rgbn = vec![[0u16; 4]; width * height];
    let mut valid = vec![false; width * height];
    let mut missing_tiles = Vec::new();
    // One client for every tile: a wide footprint reads dozens of tiles,
    // and each fresh client pays TLS setup to the same host again.
    let client =
        http::blocking_client(Duration::from_secs(120)).context("build Sentinel-2 client")?;
    for (tile_name, points) in &tiles {
        if let Err(error) = sample_imagery_tile(
            tile_name, points, width, height, &client, &mut rgbn, &mut valid,
        ) {
            warn!(%error, tile = %tile_name, "Sentinel-2 composite tile unavailable");
            missing_tiles.push(tile_name.clone());
        }
    }
    if missing_tiles.is_empty()
        && let Err(error) = store_cached_raster(&cache_path, width, height, &rgbn, &valid)
    {
        warn!(%error, "could not cache the sampled imagery raster");
    }
    Ok(ImageryRaster {
        width,
        height,
        rgbn,
        valid,
        tiles: tile_names,
        missing_tiles,
    })
}

/// The provenance line for the manifest: dataset, tiles, and the required
/// Copernicus notice.
pub(crate) fn imagery_source_note(raster: &ImageryRaster, stretch: &str) -> String {
    let mut note = format!(
        "ground imagery: Sentinel-2 RGBNIR annual composite {IMAGERY_YEAR} {IMAGERY_VERSION} (ESA WorldCover), 10 m, tiles {}; {stretch}; {IMAGERY_ATTRIBUTION}",
        raster.tiles.join(", ")
    );
    if !raster.missing_tiles.is_empty() {
        note.push_str(&format!(
            "; tiles {} unavailable, their samples fall back to mapped class colors",
            raster.missing_tiles.join(", ")
        ));
    }
    note
}

/// Samples one tile's contribution through a ranged read of the remote COG,
/// choosing the overview that matches the sampling density like the
/// land-cover path does.
fn sample_imagery_tile(
    tile_name: &str,
    points: &[SamplePoint],
    target_width: usize,
    target_height: usize,
    client: &reqwest::blocking::Client,
    rgbn: &mut [[u16; 4]],
    valid: &mut [bool],
) -> Result<()> {
    let url = format!(
        "{IMAGERY_BASE_URL}/{IMAGERY_YEAR}/{}/ESA_WorldCover_10m_{IMAGERY_YEAR}_{IMAGERY_VERSION}_{tile_name}_S2RGBNIR.tif",
        latitude_token(tile_name)
    );
    let options = HttpOpenOptions {
        client: Some(client.clone()),
        ..HttpOpenOptions::default()
    };
    let file = HttpGeoTiffFile::open_with_options(&url, options)
        .with_context(|| format!("open Sentinel-2 composite tile {tile_name}"))?;
    let geotiff = file.inner();
    if geotiff.epsg() != Some(4326) {
        bail!(
            "Sentinel-2 composite tile {tile_name} uses unexpected CRS {:?}",
            geotiff.epsg()
        );
    }
    if geotiff.width() != IMAGERY_TILE_PIXELS
        || geotiff.height() != IMAGERY_TILE_PIXELS
        || geotiff.band_count() < 4
    {
        bail!(
            "Sentinel-2 composite tile {tile_name} is {}x{}x{}, not the expected {IMAGERY_TILE_PIXELS} square of 4 bands",
            geotiff.width(),
            geotiff.height(),
            geotiff.band_count()
        );
    }

    let base_pixels = points
        .iter()
        .map(|point| {
            geotiff
                .geo_to_pixel(point.longitude, point.latitude)
                .with_context(|| format!("map a coordinate into tile {tile_name}"))
        })
        .collect::<Result<Vec<_>>>()?;
    let bounds = |values: &[(f64, f64)], pick: fn(&(f64, f64)) -> f64, limit: u32| {
        let low = values
            .iter()
            .map(|value| pick(value).floor().max(0.0) as usize)
            .min()
            .unwrap_or(0)
            .min(limit.saturating_sub(1) as usize);
        let high = values
            .iter()
            .map(|value| pick(value).ceil().max(0.0) as usize)
            .max()
            .unwrap_or(low)
            .min(limit.saturating_sub(1) as usize);
        (low, high)
    };
    let (base_col_min, base_col_max) = bounds(&base_pixels, |p| p.0, geotiff.width());
    let (base_row_min, base_row_max) = bounds(&base_pixels, |p| p.1, geotiff.height());
    let overviews = (0..geotiff.overview_count())
        .filter_map(|index| {
            let ifd = geotiff.overview_ifd(index).ok()?;
            Some((index, ifd.width(), ifd.height()))
        })
        .collect::<Vec<_>>();
    let overview = select_sampling_overview(
        &overviews,
        (geotiff.width(), geotiff.height()),
        (
            base_col_max - base_col_min + 1,
            base_row_max - base_row_min + 1,
        ),
        (target_width, target_height),
    );
    let (raster_width, raster_height) = overview
        .map(|(_, width, height)| (width, height))
        .unwrap_or((geotiff.width(), geotiff.height()));
    let scale_x = raster_width as f64 / geotiff.width() as f64;
    let scale_y = raster_height as f64 / geotiff.height() as f64;
    let pixels = base_pixels
        .into_iter()
        .map(|(column, row)| (column * scale_x, row * scale_y))
        .collect::<Vec<_>>();
    let (col_min, col_max) = bounds(&pixels, |p| p.0, raster_width);
    let (row_min, row_max) = bounds(&pixels, |p| p.1, raster_height);
    let rows = row_max - row_min + 1;
    let columns = col_max - col_min + 1;
    let windows = (0..4)
        .map(|band| {
            match overview {
                Some((index, _, _)) => geotiff
                    .read_overview_band_window::<u16>(index, band, row_min, col_min, rows, columns),
                None => geotiff.read_band_window::<u16>(band, row_min, col_min, rows, columns),
            }
            .with_context(|| format!("read Sentinel-2 composite tile {tile_name} band {band}"))
        })
        .collect::<Result<Vec<_>>>()?;

    for (point, (column, row)) in points.iter().zip(pixels) {
        let column = (column.round() as isize).clamp(col_min as isize, col_max as isize) as usize;
        let row = (row.round() as isize).clamp(row_min as isize, row_max as isize) as usize;
        let mut sample = [0u16; 4];
        let mut complete = true;
        for (band, window) in windows.iter().enumerate() {
            // Pure defense: the window bounds are clamped in-range above
            // and the reader errors rather than clips, so this is never
            // `None` today. If a reader change ever shortens a window, a
            // missing pixel degrades to nodata instead of a panic.
            match window.get([row - row_min, column - col_min]) {
                Some(&value) => sample[band] = value,
                None => complete = false,
            }
        }
        // The tiles declare nodata 0; a pixel dark in every band at once is
        // the fill, not a real observation.
        if complete && sample.iter().any(|&value| value != 0) {
            rgbn[point.output_index] = sample;
            valid[point.output_index] = true;
        }
    }
    Ok(())
}

/// Name of the 1 degree composite tile containing a coordinate, like
/// `N37W123`: the same naming the 3 degree land-cover tiles use, on a
/// finer grid.
fn imagery_tile(longitude: f64, latitude: f64) -> String {
    let south = (latitude / IMAGERY_TILE_DEGREES).floor() as i32;
    let west = (longitude / IMAGERY_TILE_DEGREES).floor() as i32;
    format!(
        "{}{:02}{}{:03}",
        if south < 0 { 'S' } else { 'N' },
        south.unsigned_abs(),
        if west < 0 { 'W' } else { 'E' },
        west.unsigned_abs(),
    )
}

/// The bucket groups tiles by their latitude band directory, `N37W123`
/// under `N37/`.
fn latitude_token(tile_name: &str) -> &str {
    &tile_name[..3]
}

/// Cache file for one sampled footprint. The four sample-grid corners pin
/// the footprint including rotation, so any change of area, span, angle,
/// or resolution misses instead of serving the wrong raster.
fn cache_file_name(transform: &GeoTransform, width: usize, height: usize) -> String {
    let mut canonical = format!("s2rgbnir-{IMAGERY_YEAR}-{IMAGERY_VERSION}|{width}x{height}");
    for (u, v) in [(0.0, 0.0), (1.0, 0.0), (0.0, 1.0), (1.0, 1.0)] {
        let (latitude, longitude) = transform.coordinate_at_uv(u, v);
        canonical.push_str(&format!("|{latitude:.7},{longitude:.7}"));
    }
    format!(
        "s2rgbnir-{IMAGERY_YEAR}-{IMAGERY_VERSION}-{width}x{height}-{:016x}.bin",
        fnv1a_64(canonical.as_bytes())
    )
}

/// FNV-1a, 64 bit: a stable file-name hash. The standard library's hasher
/// is documented as unstable across releases, and a cache key that drifts
/// with the toolchain silently splits the cache.
fn fnv1a_64(bytes: &[u8]) -> u64 {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for &byte in bytes {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash
}

fn store_cached_raster(
    path: &Path,
    width: usize,
    height: usize,
    rgbn: &[[u16; 4]],
    valid: &[bool],
) -> Result<()> {
    let mut bytes = Vec::with_capacity(4 + 8 + rgbn.len() * 8 + valid.len().div_ceil(8));
    bytes.extend_from_slice(CACHE_MAGIC);
    bytes.extend_from_slice(&(width as u32).to_le_bytes());
    bytes.extend_from_slice(&(height as u32).to_le_bytes());
    for sample in rgbn {
        for &band in sample {
            bytes.extend_from_slice(&band.to_le_bytes());
        }
    }
    let mut mask = vec![0u8; valid.len().div_ceil(8)];
    for (index, &flag) in valid.iter().enumerate() {
        if flag {
            mask[index / 8] |= 1 << (index % 8);
        }
    }
    bytes.extend_from_slice(&mask);
    cache::store(path, &bytes)
}

/// Loads a cached sampled raster, or `None` when there is none or it does
/// not parse — a corrupt cache file must mean a refetch, never an error.
fn load_cached_raster(
    path: &Path,
    width: usize,
    height: usize,
) -> Option<(Vec<[u16; 4]>, Vec<bool>)> {
    let bytes = cache::read(path).ok()?;
    let samples = width * height;
    let expected = 4 + 8 + samples * 8 + samples.div_ceil(8);
    if bytes.len() != expected || &bytes[..4] != CACHE_MAGIC {
        return None;
    }
    let stored_width = u32::from_le_bytes(bytes[4..8].try_into().ok()?);
    let stored_height = u32::from_le_bytes(bytes[8..12].try_into().ok()?);
    if stored_width as usize != width || stored_height as usize != height {
        return None;
    }
    let mut rgbn = Vec::with_capacity(samples);
    for index in 0..samples {
        let offset = 12 + index * 8;
        let mut sample = [0u16; 4];
        for (band, slot) in sample.iter_mut().enumerate() {
            let start = offset + band * 2;
            *slot = u16::from_le_bytes(bytes[start..start + 2].try_into().ok()?);
        }
        rgbn.push(sample);
    }
    let mask = &bytes[12 + samples * 8..];
    let valid = (0..samples)
        .map(|index| mask[index / 8] & (1 << (index % 8)) != 0)
        .collect();
    Some((rgbn, valid))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolves_one_degree_imagery_tile_names() {
        // The land-cover naming on the finer 1 degree grid: SFO sits in
        // N37W123 here, and in the same-named 3 degree land-cover tile only
        // by coincidence of the corner.
        assert_eq!(imagery_tile(-122.399, 37.615), "N37W123");
        assert_eq!(imagery_tile(138.7274, 35.3606), "N35E138");
        assert_eq!(imagery_tile(-0.5, -0.5), "S01W001");
        assert_eq!(imagery_tile(0.5, 0.5), "N00E000");
        assert_eq!(latitude_token("S01W001"), "S01");
    }

    #[test]
    fn the_cache_round_trips_and_rejects_mismatched_shapes() {
        let dir = std::env::temp_dir().join(format!(
            "toposaic-imagery-cache-test-{}",
            std::process::id()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("raster.bin");
        let rgbn = (0..6u16)
            .map(|i| [i, i + 1, i + 2, i + 3])
            .collect::<Vec<_>>();
        let valid = vec![true, false, true, true, false, true];
        store_cached_raster(&path, 3, 2, &rgbn, &valid).unwrap();
        let (read_rgbn, read_valid) = load_cached_raster(&path, 3, 2).unwrap();
        assert_eq!(read_rgbn, rgbn);
        assert_eq!(read_valid, valid);
        // A different shape misses rather than mis-parses.
        assert!(load_cached_raster(&path, 2, 3).is_none());
        assert!(load_cached_raster(&dir.join("absent.bin"), 3, 2).is_none());
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn cache_names_pin_the_footprint_and_resolution() {
        let transform = GeoTransform::new(37.615, -122.399, 4.5, 0.0);
        let name = cache_file_name(&transform, 128, 128);
        assert_eq!(name, cache_file_name(&transform, 128, 128));
        assert_ne!(name, cache_file_name(&transform, 256, 256));
        let rotated = GeoTransform::new(37.615, -122.399, 4.5, 30.0);
        assert_ne!(name, cache_file_name(&rotated, 128, 128));
        let elsewhere = GeoTransform::new(46.8523, -121.7603, 18.0, 0.0);
        assert_ne!(name, cache_file_name(&elsewhere, 128, 128));
    }
}
