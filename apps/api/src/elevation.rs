use std::{
    collections::{BTreeSet, HashMap, HashSet},
    fs,
    path::{Path, PathBuf},
    time::Duration,
};

use anyhow::{Context, Result, bail};
use image::{ImageFormat, RgbImage};
use reqwest::{StatusCode, blocking::Client};
use terrain_core::{ElevationSource, GenerationSpec, HeightField};

use crate::cache;

const EARTH_CIRCUMFERENCE_M: f64 = 40_075_016.686;
const SOURCE_SAMPLES_PER_MESH_INTERVAL: f64 = 2.0;

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
    let samples = spec.effective_samples_per_piece();
    let sample_width = (spec.columns * samples + 1) as usize;
    let sample_height = (spec.rows * samples + 1) as usize;
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
    let client = Client::builder()
        .user_agent("toposaic/0.1 (+local terrain mesh generator)")
        .timeout(Duration::from_secs(20))
        .build()?;
    let mut tiles = HashMap::new();
    let mut missing_tiles = HashSet::new();
    let half_lat = spec.ground_span_km / 2.0 / 110.574;
    let longitude_scale = (111.32 * spec.center_lat.to_radians().cos().abs()).max(20.0);
    let half_lon = spec.ground_span_km / 2.0 / longitude_scale;
    let south = (spec.center_lat - half_lat).max(-85.0);
    let north = (spec.center_lat + half_lat).min(85.0);
    let west = spec.center_lon - half_lon;
    let east = spec.center_lon + half_lon;
    let mut values_m = Vec::with_capacity(sample_width * sample_height);
    let mut sampler = ElevationSampler {
        client: &client,
        cache_dir,
        tiles: &mut tiles,
        missing_tiles: &mut missing_tiles,
        provider,
        used_zooms: BTreeSet::new(),
    };

    for row in 0..sample_height {
        let v = row as f64 / (sample_height - 1) as f64;
        let latitude = south + (north - south) * v;
        for column in 0..sample_width {
            let u = column as f64 / (sample_width - 1) as f64;
            let longitude = normalize_longitude(west + (east - west) * u);
            values_m.push(sampler.sample(requested_zoom, longitude, latitude)?);
        }
        on_progress((row + 1) as f32 / sample_height as f32)?;
    }

    let source = provider.source_description(requested_zoom, &sampler.used_zooms);
    HeightField::new(sample_width, sample_height, values_m, source)
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

struct ElevationSampler<'a> {
    client: &'a Client,
    cache_dir: &'a Path,
    tiles: &'a mut HashMap<(u8, u32, u32), RgbImage>,
    missing_tiles: &'a mut HashSet<(u8, u32, u32)>,
    provider: ElevationProvider,
    used_zooms: BTreeSet<u8>,
}

impl ElevationSampler<'_> {
    fn sample(&mut self, requested_zoom: u8, longitude: f64, latitude: f64) -> Result<f32> {
        let (global_x, global_y) =
            global_pixel_position(self.provider.tile_size, requested_zoom, longitude, latitude);
        let centered_x = global_x - 0.5;
        let centered_y = global_y - 0.5;
        let x0 = centered_x.floor() as i64;
        let y0 = centered_y.floor() as i64;
        let tx = (centered_x - x0 as f64) as f32;
        let ty = (centered_y - y0 as f64) as f32;
        let bottom_left = self.sample_global_pixel(requested_zoom, x0, y0)?;
        let bottom_right = self.sample_global_pixel(requested_zoom, x0 + 1, y0)?;
        let top_left = self.sample_global_pixel(requested_zoom, x0, y0 + 1)?;
        let top_right = self.sample_global_pixel(requested_zoom, x0 + 1, y0 + 1)?;
        Ok(bilinear_elevation(
            [bottom_left, bottom_right, top_left, top_right],
            tx,
            ty,
        ))
    }

    fn sample_global_pixel(
        &mut self,
        requested_zoom: u8,
        global_x: i64,
        global_y: i64,
    ) -> Result<f32> {
        let minimum_zoom = if self.provider.allows_parent_fallback() {
            self.provider.minimum_zoom
        } else {
            requested_zoom
        };
        let requested_total_pixels = i64::from(self.provider.tile_size) * (1_i64 << requested_zoom);
        let global_x = global_x.rem_euclid(requested_total_pixels);
        let global_y = global_y.clamp(0, requested_total_pixels - 1);
        for zoom in (minimum_zoom..=requested_zoom).rev() {
            let scale = 1_i64 << (requested_zoom - zoom);
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
            if let std::collections::hash_map::Entry::Vacant(entry) = self.tiles.entry(key) {
                match load_tile(
                    self.client,
                    self.cache_dir,
                    self.provider,
                    zoom,
                    location.tile_x,
                    location.tile_y,
                )? {
                    Some(tile) => {
                        entry.insert(tile);
                    }
                    None => {
                        self.missing_tiles.insert(key);
                        continue;
                    }
                }
            }
            let pixel = self
                .tiles
                .get(&key)
                .context("elevation tile cache lost a tile")?
                .get_pixel(location.pixel_x, location.pixel_y);
            self.used_zooms.insert(zoom);
            return Ok(decode_terrarium_pixel(pixel.0));
        }
        bail!(
            "{} has no elevation tile for this point at or below z{requested_zoom}",
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

#[cfg(test)]
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

fn bilinear_elevation(corners: [f32; 4], tx: f32, ty: f32) -> f32 {
    let bottom = corners[0] * (1.0 - tx) + corners[1] * tx;
    let top = corners[2] * (1.0 - tx) + corners[3] * tx;
    bottom * (1.0 - ty) + top * ty
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
    let bytes = if path.is_file() {
        fs::read(&path).with_context(|| format!("read cached tile {}", path.display()))?
    } else {
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
        cache::store(&path, &bytes)
            .with_context(|| format!("cache elevation tile {}", path.display()))?;
        bytes
    };

    let image = image::load_from_memory_with_format(&bytes, provider.image_format)
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
    Ok(Some(image))
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

fn normalize_longitude(longitude: f64) -> f64 {
    (longitude + 180.0).rem_euclid(360.0) - 180.0
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
    fn elevation_pixels_blend_in_both_axes() {
        assert!((bilinear_elevation([0.0, 10.0, 20.0, 30.0], 0.5, 0.5) - 15.0).abs() < 1e-6);
        assert!((bilinear_elevation([0.0, 10.0, 20.0, 30.0], 0.25, 0.75) - 17.5).abs() < 1e-6);
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
