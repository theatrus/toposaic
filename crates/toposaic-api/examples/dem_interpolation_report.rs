//! Source-pixel grid artifact report over real elevation data (network data).
//!
//! Fetches a close-view height field exactly as the API server's job runner
//! does, then measures how much of the surface curvature sits exactly on the
//! source DEM's pixel boundaries. Two numbers name the two ways the sampler
//! can print a grid across the terrain:
//!
//! * The share of the total second difference that lands on pixel edges. A
//!   straight-line blend is flat inside every source pixel and bends only at
//!   its edges, so the curvature comes out as a comb standing on the tile
//!   grid. Even coverage puts that share near the share of edge samples.
//! * The share of neighbouring samples that read back exactly equal. A run of
//!   equal readings is a flat step, which is what interpolating on a finer
//!   lattice than the tiles really hold produces.
//!
//! Usage: dem_interpolation_report [case-name ...]
//! With no arguments every case runs.

use std::{path::Path, time::Instant};

use toposaic_api::diagnostics::{fetch_height_field_with_progress, map_cache_root};
use toposaic_core::{ElevationSource, GenerationSpec, HeightField};

const EARTH_CIRCUMFERENCE_M: f64 = 40_075_016.686;

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
        ("mapzen-2km", close(ElevationSource::Mapzen)),
        ("mapterhorn-2km", close(ElevationSource::Mapterhorn)),
        (
            "mapterhorn-2km-fine",
            GenerationSpec {
                fine_dem_detail: true,
                ..close(ElevationSource::Mapterhorn)
            },
        ),
    ]
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let selected = std::env::args().skip(1).collect::<Vec<_>>();
    let cache_dir = map_cache_root()?.join("elevation");
    for (name, spec) in cases() {
        if !selected.is_empty() && !selected.iter().any(|argument| argument == name) {
            continue;
        }
        spec.validate()?;
        report_case(name, &spec, &cache_dir)?;
    }
    Ok(())
}

fn report_case(
    name: &str,
    spec: &GenerationSpec,
    cache_dir: &Path,
) -> Result<(), Box<dyn std::error::Error>> {
    eprintln!("fetching {name}...");
    let started = Instant::now();
    let field = fetch_height_field_with_progress(spec, cache_dir, |_| Ok(()))?;
    let fetch_seconds = started.elapsed().as_secs_f64();
    let tile_size = match spec.elevation_source {
        ElevationSource::Mapzen => 256.0,
        ElevationSource::Mapterhorn => 512.0,
    };
    let requested_zoom = parse_zoom(&field.source, "requested z")
        .or_else(|| parse_zoom(&field.source, "Terrarium z"))
        .unwrap_or(0);
    // The lattice the surface really lives on: with parent fallback the tiles
    // that answered can be coarser than the zoom the sampler asked for.
    let zoom = parse_zoom(&field.source, "used z").unwrap_or(requested_zoom);
    let source_pixel_m = EARTH_CIRCUMFERENCE_M * spec.center_lat.to_radians().cos().abs()
        / (tile_size * 2f64.powi(zoom as i32));
    let sample_spacing_m = spec.ground_span_km * 1_000.0 / (field.width - 1) as f64;

    println!("== {name} ==");
    println!("  source: {}", field.source);
    println!("  grid: {}x{}", field.width, field.height);
    println!("  requested zoom z{requested_zoom}, data lattice z{zoom}");
    println!("  fetch+sample seconds (cache state as found): {fetch_seconds:.2}");
    println!("  source pixel: {source_pixel_m:.2} m, mesh sample: {sample_spacing_m:.2} m");
    println!(
        "  samples per source pixel: {:.2}",
        source_pixel_m / sample_spacing_m
    );

    let x_boundaries = column_boundaries(spec, &field, tile_size as u32, zoom);
    let y_boundaries = row_boundaries(spec, &field, tile_size as u32, zoom);
    report_axis("rows (east-west)", &field, &x_boundaries, Axis::X);
    report_axis("columns (north-south)", &field, &y_boundaries, Axis::Y);
    println!();
    Ok(())
}

#[derive(Clone, Copy)]
enum Axis {
    X,
    Y,
}

/// Mean absolute second difference at samples that straddle a source pixel
/// boundary versus samples strictly inside a source pixel.
fn report_axis(label: &str, field: &HeightField, boundary: &[bool], axis: Axis) {
    let (line_length, line_count) = match axis {
        Axis::X => (field.width, field.height),
        Axis::Y => (field.height, field.width),
    };
    let at = |line: usize, index: usize| match axis {
        Axis::X => field.values_m[line * field.width + index],
        Axis::Y => field.values_m[index * field.width + line],
    };
    let mut boundary_sum = 0.0f64;
    let mut boundary_count = 0usize;
    let mut interior_sum = 0.0f64;
    let mut interior_count = 0usize;
    let mut boundary_peak = 0.0f64;
    let mut interior_peak = 0.0f64;
    for line in 0..line_count {
        for (index, on_boundary) in boundary.iter().enumerate().take(line_length - 1).skip(1) {
            let second_difference =
                f64::from(at(line, index - 1) - 2.0 * at(line, index) + at(line, index + 1)).abs();
            if *on_boundary {
                boundary_sum += second_difference;
                boundary_count += 1;
                boundary_peak = boundary_peak.max(second_difference);
            } else {
                interior_sum += second_difference;
                interior_count += 1;
                interior_peak = interior_peak.max(second_difference);
            }
        }
    }
    let mut equal_pairs = 0usize;
    let mut pairs = 0usize;
    let mut run_histogram = [0usize; 12];
    for line in 0..line_count {
        let mut run = 1usize;
        for index in 1..line_length {
            pairs += 1;
            if at(line, index) == at(line, index - 1) {
                equal_pairs += 1;
                run += 1;
            } else {
                run_histogram[run.min(11)] += 1;
                run = 1;
            }
        }
        run_histogram[run.min(11)] += 1;
    }
    let boundary_mean = boundary_sum / boundary_count.max(1) as f64;
    let interior_mean = interior_sum / interior_count.max(1) as f64;
    let spacings = boundary_spacings(boundary);
    println!("  {label}:");
    println!(
        "    boundary samples {boundary_count} ({:.1}%), interior {interior_count}",
        100.0 * boundary_count as f64 / (boundary_count + interior_count).max(1) as f64
    );
    println!(
        "    mean |d2| boundary {boundary_mean:.4} m, interior {interior_mean:.4} m, ratio {:.1}x",
        boundary_mean / interior_mean.max(1e-9)
    );
    println!("    peak |d2| boundary {boundary_peak:.3} m, interior {interior_peak:.3} m");
    println!(
        "    curvature share on boundaries: {:.1}% of total |d2|",
        100.0 * boundary_sum / (boundary_sum + interior_sum).max(1e-12)
    );
    if let Some((minimum, maximum, mean)) = spacings {
        println!("    boundary spacing: min {minimum}, max {maximum}, mean {mean:.2} samples");
    }
    println!(
        "    exactly-equal neighbours (flat plateaus): {:.1}% of {pairs} pairs",
        100.0 * equal_pairs as f64 / pairs.max(1) as f64
    );
    let runs = run_histogram
        .iter()
        .enumerate()
        .skip(1)
        .map(|(length, count)| format!("{length}:{count}"))
        .collect::<Vec<_>>()
        .join(" ");
    println!("    equal-value run lengths (11 = 11 or more): {runs}");
}

fn boundary_spacings(boundary: &[bool]) -> Option<(usize, usize, f64)> {
    let positions = boundary
        .iter()
        .enumerate()
        .filter(|(_, flag)| **flag)
        .map(|(index, _)| index)
        .collect::<Vec<_>>();
    if positions.len() < 3 {
        return None;
    }
    let gaps = positions
        .windows(2)
        .map(|pair| pair[1] - pair[0])
        .filter(|gap| *gap > 1)
        .collect::<Vec<_>>();
    if gaps.is_empty() {
        return None;
    }
    let minimum = *gaps.iter().min()?;
    let maximum = *gaps.iter().max()?;
    let mean = gaps.iter().sum::<usize>() as f64 / gaps.len() as f64;
    Some((minimum, maximum, mean))
}

/// True where the sample's own pixel differs from either neighbour's pixel,
/// i.e. where a bilinear surface is allowed to bend.
fn column_boundaries(
    spec: &GenerationSpec,
    field: &HeightField,
    tile_size: u32,
    zoom: u8,
) -> Vec<bool> {
    // Read positions along the model's middle row; for a rotated model this
    // follows the rotated axis, matching what the sampler actually fetched.
    let transform = spec.geo_transform();
    let pixels = (0..field.width)
        .map(|column| {
            let u = column as f64 / (field.width - 1) as f64;
            let (_, longitude) = transform.coordinate_at_uv(u, 0.5);
            let x = (longitude + 180.0) / 360.0 * f64::from(1_u32 << zoom) * f64::from(tile_size);
            (x - 0.5).floor() as i64
        })
        .collect::<Vec<_>>();
    flag_boundaries(&pixels)
}

fn row_boundaries(
    spec: &GenerationSpec,
    field: &HeightField,
    tile_size: u32,
    zoom: u8,
) -> Vec<bool> {
    let transform = spec.geo_transform();
    let pixels = (0..field.height)
        .map(|row| {
            let v = row as f64 / (field.height - 1) as f64;
            let (latitude, _) = transform.coordinate_at_uv(0.5, v);
            let latitude = latitude.clamp(-85.051_128_78, 85.051_128_78).to_radians();
            let y = (1.0 - (latitude.tan() + 1.0 / latitude.cos()).ln() / std::f64::consts::PI)
                / 2.0
                * f64::from(1_u32 << zoom)
                * f64::from(tile_size);
            (y - 0.5).floor() as i64
        })
        .collect::<Vec<_>>();
    flag_boundaries(&pixels)
}

fn flag_boundaries(pixels: &[i64]) -> Vec<bool> {
    (0..pixels.len())
        .map(|index| {
            let previous = pixels[index.saturating_sub(1)];
            let next = pixels[(index + 1).min(pixels.len() - 1)];
            previous != pixels[index] || next != pixels[index]
        })
        .collect()
}

fn parse_zoom(source: &str, marker: &str) -> Option<u8> {
    let rest = source.split(marker).nth(1)?;
    let digits = rest
        .chars()
        .take_while(char::is_ascii_digit)
        .collect::<String>();
    digits.parse().ok()
}
