//! Hillshade pictures of a real close-view height field (network data).
//!
//! Fetches the height field exactly as the API server's job runner does, then
//! draws two pictures of it:
//!
//! * A hillshade. Surface normals come from the height field at true ground
//!   spacing, and the light sits low (315 degrees, 30 degrees up). Grazing
//!   light is the point: a slope that jumps at every source pixel edge throws
//!   a hard line, so the stair-step grid prints as a comb across the terrain.
//!   Overhead light would wash exactly that away.
//! * A bend map. Each sample carries the angle between its own normal and its
//!   neighbours' normals, on a heat scale. Terrain that bends evenly reads
//!   dark; a crease standing on the source lattice reads as a bright ruled
//!   grid.
//!
//! Usage: dem_hillshade --tag <name> [--out <dir>] [--crop <samples>]
//!                      [--zoom <n>] [--shift <east> <north>]
//!                      [--stretch <low> <high>] [--bend-cap <degrees>]
//!                      [case-name ...]
//! With no case names every case runs. `--tag` names the sampler in the output
//! file names, so a before and an after run can sit side by side. `--zoom`
//! repeats each sample n times across and down, which is how a source lattice
//! only a few samples wide becomes visible. `--shift` moves the window off
//! the centre of the field, in samples east and north. `--stretch` opens up a
//! narrow band of shading, which a slope facing the light needs.
//!
//! The pictures under `docs/images` came from these two runs, once with the
//! sampler in `elevation.rs` and once with the earlier straight-line blend
//! checked out in its place:
//!
//! ```text
//! dem_hillshade --tag <sampler> --out docs/images --crop 400 --zoom 2
//! dem_hillshade --tag <sampler>-detail --out docs/images --crop 64 --zoom 12 \
//!     --shift -150 149 --stretch 0.82 0.93 mapzen
//! ```

use std::{error::Error, f64::consts::PI, fs, path::PathBuf};

use image::{GrayImage, Luma, Rgb, RgbImage};
use toposaic_api::diagnostics::{fetch_height_field_with_progress, map_cache_root};
use toposaic_core::{ElevationSource, GenerationSpec, HeightField};

/// Where the light comes from, clockwise from north.
const LIGHT_AZIMUTH_DEGREES: f64 = 315.0;
/// How far the light sits above the horizon. Low on purpose.
const LIGHT_ALTITUDE_DEGREES: f64 = 30.0;
/// Lifts the shading of a low sun into a readable range. Applied identically
/// to every picture, so a pair stays comparable.
const SHADE_GAMMA: f64 = 1.5;

struct Options {
    tag: String,
    out_dir: PathBuf,
    crop: usize,
    zoom: usize,
    shift: (i64, i64),
    stretch: (f64, f64),
    bend_cap_degrees: f64,
    cases: Vec<String>,
}

fn cases() -> Vec<(&'static str, GenerationSpec)> {
    let close = |source: ElevationSource| GenerationSpec {
        center_lat: 46.8523,
        center_lon: -121.7603,
        ground_span_km: 2.0,
        solid_model: true,
        place_name: "Rainier".into(),
        elevation_source: source,
        ..GenerationSpec::default()
    };
    vec![
        ("mapzen", close(ElevationSource::Mapzen)),
        ("mapterhorn", close(ElevationSource::Mapterhorn)),
    ]
}

fn main() -> Result<(), Box<dyn Error>> {
    let options = parse_options()?;
    fs::create_dir_all(&options.out_dir)?;
    let cache_dir = map_cache_root()?.join("elevation");
    for (name, spec) in cases() {
        if !options.cases.is_empty() && !options.cases.iter().any(|wanted| wanted == name) {
            continue;
        }
        spec.validate()?;
        render_case(name, &spec, &cache_dir, &options)?;
    }
    Ok(())
}

fn parse_options() -> Result<Options, Box<dyn Error>> {
    let mut tag = None;
    let mut out_dir = PathBuf::from("dem-images");
    let mut crop = 800usize;
    let mut zoom = 1usize;
    let mut shift = (0i64, 0i64);
    let mut stretch = (0.0f64, 1.0f64);
    let mut bend_cap_degrees = 20.0f64;
    let mut cases = Vec::new();
    let mut arguments = std::env::args().skip(1);
    while let Some(argument) = arguments.next() {
        match argument.as_str() {
            "--tag" => tag = Some(arguments.next().ok_or("--tag needs a value")?),
            "--out" => out_dir = PathBuf::from(arguments.next().ok_or("--out needs a value")?),
            "--crop" => crop = arguments.next().ok_or("--crop needs a value")?.parse()?,
            "--zoom" => zoom = arguments.next().ok_or("--zoom needs a value")?.parse()?,
            "--shift" => {
                shift = (
                    arguments
                        .next()
                        .ok_or("--shift needs two values")?
                        .parse()?,
                    arguments
                        .next()
                        .ok_or("--shift needs two values")?
                        .parse()?,
                );
            }
            "--stretch" => {
                stretch = (
                    arguments
                        .next()
                        .ok_or("--stretch needs two values")?
                        .parse()?,
                    arguments
                        .next()
                        .ok_or("--stretch needs two values")?
                        .parse()?,
                );
            }
            "--bend-cap" => {
                bend_cap_degrees = arguments
                    .next()
                    .ok_or("--bend-cap needs a value")?
                    .parse()?;
            }
            other => cases.push(other.to_string()),
        }
    }
    Ok(Options {
        tag: tag.ok_or("--tag is required, for example --tag bilinear")?,
        out_dir,
        crop,
        zoom: zoom.max(1),
        shift,
        stretch,
        bend_cap_degrees,
        cases,
    })
}

fn render_case(
    name: &str,
    spec: &GenerationSpec,
    cache_dir: &std::path::Path,
    options: &Options,
) -> Result<(), Box<dyn Error>> {
    eprintln!("fetching {name}...");
    let field = fetch_height_field_with_progress(spec, cache_dir, |_| Ok(()))?;
    let spacing_m = spec.ground_span_km * 1_000.0 / (field.width - 1) as f64;
    println!("== {name} / {} ==", options.tag);
    println!("  source: {}", field.source);
    println!("  grid: {}x{}", field.width, field.height);
    println!("  ground spacing: {spacing_m:.2} m");
    println!("  height checksum: {:016x}", checksum(&field));
    let lowest = field.values_m.iter().copied().fold(f32::INFINITY, f32::min);
    let highest = field
        .values_m
        .iter()
        .copied()
        .fold(f32::NEG_INFINITY, f32::max);
    // A cubic that rings at a cliff shows up here as a range wider than the
    // readings the tiles hold.
    println!("  height range: {lowest:.2} m to {highest:.2} m");

    let normals = surface_normals(&field, spacing_m);
    let shade = hillshade(&normals);
    let bend = bend_degrees(&field, &normals);
    report_bend(&bend);

    let window = Window::centered(
        field.width,
        field.height,
        options.crop,
        options.zoom,
        options.shift,
    );
    println!(
        "  window: {} samples square at {}x, {} px",
        window.size,
        window.zoom,
        window.size * window.zoom
    );
    let hillshade_path = options
        .out_dir
        .join(format!("dem-{name}-{}.png", options.tag));
    report_window_shade(&field, &window, &shade);
    write_gray(&hillshade_path, &field, &window, &shade, options.stretch)?;
    let bend_path = options
        .out_dir
        .join(format!("dem-{name}-{}-bend.png", options.tag));
    write_heat(&bend_path, &field, &window, &bend, options.bend_cap_degrees)?;
    println!("  wrote {}", hillshade_path.display());
    println!("  wrote {}", bend_path.display());
    println!();
    Ok(())
}

/// The spread of shading inside the window, which is what `--stretch` needs
/// to know to open a washed-out slope up.
fn report_window_shade(field: &HeightField, window: &Window, shade: &[f64]) {
    let mut values = Vec::with_capacity(window.size * window.size);
    for y in 0..window.size as u32 {
        for x in 0..window.size as u32 {
            values.push(
                shade[window.source_index(field, x * window.zoom as u32, y * window.zoom as u32)],
            );
        }
    }
    values.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let percentile = |share: f64| values[((values.len() - 1) as f64 * share) as usize];
    println!(
        "  window shade: p1 {:.3}, p50 {:.3}, p99 {:.3}",
        percentile(0.01),
        percentile(0.5),
        percentile(0.99)
    );
}

/// A cheap fingerprint of the sampled heights, so two runs cannot be mistaken
/// for each other.
fn checksum(field: &HeightField) -> u64 {
    let mut hash = 0xcbf2_9ce4_8422_2325u64;
    for value in &field.values_m {
        for byte in value.to_bits().to_le_bytes() {
            hash ^= u64::from(byte);
            hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
        }
    }
    hash
}

/// Unit surface normals, x east, y north, z up, from central differences at
/// true ground spacing.
fn surface_normals(field: &HeightField, spacing_m: f64) -> Vec<[f64; 3]> {
    let at = |column: usize, row: usize| f64::from(field.values_m[row * field.width + column]);
    let mut normals = Vec::with_capacity(field.width * field.height);
    for row in 0..field.height {
        for column in 0..field.width {
            let west = column.saturating_sub(1);
            let east = (column + 1).min(field.width - 1);
            let south = row.saturating_sub(1);
            let north = (row + 1).min(field.height - 1);
            let run_x = (east - west) as f64 * spacing_m;
            let run_y = (north - south) as f64 * spacing_m;
            let slope_x = (at(east, row) - at(west, row)) / run_x;
            let slope_y = (at(column, north) - at(column, south)) / run_y;
            let length = (slope_x * slope_x + slope_y * slope_y + 1.0).sqrt();
            normals.push([-slope_x / length, -slope_y / length, 1.0 / length]);
        }
    }
    normals
}

fn light_vector() -> [f64; 3] {
    let azimuth = LIGHT_AZIMUTH_DEGREES * PI / 180.0;
    let altitude = LIGHT_ALTITUDE_DEGREES * PI / 180.0;
    [
        altitude.cos() * azimuth.sin(),
        altitude.cos() * azimuth.cos(),
        altitude.sin(),
    ]
}

fn hillshade(normals: &[[f64; 3]]) -> Vec<f64> {
    let light = light_vector();
    normals
        .iter()
        .map(|normal| {
            let lambert = dot(*normal, light).clamp(0.0, 1.0);
            lambert.powf(1.0 / SHADE_GAMMA)
        })
        .collect()
}

/// The largest angle, in degrees, between a sample's normal and either of its
/// forward neighbours. A crease shows here even where the light happens to
/// face it edge on.
fn bend_degrees(field: &HeightField, normals: &[[f64; 3]]) -> Vec<f64> {
    let mut bends = vec![0.0f64; normals.len()];
    for row in 0..field.height {
        for column in 0..field.width {
            let index = row * field.width + column;
            let mut worst = 0.0f64;
            if column + 1 < field.width {
                worst = worst.max(angle_between(normals[index], normals[index + 1]));
            }
            if row + 1 < field.height {
                worst = worst.max(angle_between(normals[index], normals[index + field.width]));
            }
            bends[index] = worst * 180.0 / PI;
        }
    }
    bends
}

fn report_bend(bend: &[f64]) {
    let mut sorted = bend.to_vec();
    sorted.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let percentile = |share: f64| sorted[((sorted.len() - 1) as f64 * share) as usize];
    let mean = sorted.iter().sum::<f64>() / sorted.len() as f64;
    println!(
        "  facet bend degrees: mean {mean:.2}, p50 {:.2}, p90 {:.2}, p99 {:.2}, max {:.2}",
        percentile(0.5),
        percentile(0.9),
        percentile(0.99),
        sorted[sorted.len() - 1]
    );
}

fn angle_between(a: [f64; 3], b: [f64; 3]) -> f64 {
    dot(a, b).clamp(-1.0, 1.0).acos()
}

fn dot(a: [f64; 3], b: [f64; 3]) -> f64 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

/// A square of samples, drawn whole pixels to a sample so nothing is
/// resampled away.
struct Window {
    x0: usize,
    y0: usize,
    size: usize,
    zoom: usize,
}

impl Window {
    fn centered(width: usize, height: usize, size: usize, zoom: usize, shift: (i64, i64)) -> Self {
        let size = size.min(width).min(height);
        let place = |span: usize, offset: i64| {
            (((span - size) / 2) as i64 + offset).clamp(0, (span - size) as i64) as usize
        };
        Self {
            x0: place(width, shift.0),
            y0: place(height, shift.1),
            size,
            zoom,
        }
    }

    fn pixels(&self) -> u32 {
        (self.size * self.zoom) as u32
    }

    /// Height field rows run south to north; image rows run north to south.
    fn source_index(&self, field: &HeightField, x: u32, y: u32) -> usize {
        let row = self.y0 + (self.size - 1 - y as usize / self.zoom);
        let column = self.x0 + x as usize / self.zoom;
        row * field.width + column
    }
}

fn write_gray(
    path: &std::path::Path,
    field: &HeightField,
    window: &Window,
    values: &[f64],
    stretch: (f64, f64),
) -> Result<(), Box<dyn Error>> {
    let mut image = GrayImage::new(window.pixels(), window.pixels());
    let span = (stretch.1 - stretch.0).max(1e-6);
    for y in 0..window.pixels() {
        for x in 0..window.pixels() {
            let value = (values[window.source_index(field, x, y)] - stretch.0) / span;
            let level = (value.clamp(0.0, 1.0) * 255.0).round() as u8;
            image.put_pixel(x, y, Luma([level]));
        }
    }
    image.save(path)?;
    Ok(())
}

fn write_heat(
    path: &std::path::Path,
    field: &HeightField,
    window: &Window,
    values: &[f64],
    cap: f64,
) -> Result<(), Box<dyn Error>> {
    let mut image = RgbImage::new(window.pixels(), window.pixels());
    for y in 0..window.pixels() {
        for x in 0..window.pixels() {
            let value = values[window.source_index(field, x, y)] / cap.max(1e-6);
            image.put_pixel(x, y, Rgb(heat(value.clamp(0.0, 1.0))));
        }
    }
    image.save(path)?;
    Ok(())
}

/// Black to red to yellow to white.
fn heat(t: f64) -> [u8; 3] {
    let level = |value: f64| (value.clamp(0.0, 1.0) * 255.0).round() as u8;
    if t < 1.0 / 3.0 {
        [level(t * 3.0), 0, 0]
    } else if t < 2.0 / 3.0 {
        [255, level((t - 1.0 / 3.0) * 3.0), 0]
    } else {
        [255, 255, level((t - 2.0 / 3.0) * 3.0)]
    }
}
