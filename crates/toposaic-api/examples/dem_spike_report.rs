//! Outlier report over real elevation data (network data).
//!
//! Fetches a height field exactly as the API server's job runner does, with the
//! repair pass turned off, and lists the samples that sit far below their own
//! neighbourhood. A single source pixel carrying a wild reading punches a spike
//! clean through the relief, so this names where those readings are, how deep
//! they go, and what the repair pass wins back.
//!
//! Usage: dem_spike_report [case-name ...]
//! With no arguments every case runs.

use std::{path::Path, time::Instant};

use toposaic_api::diagnostics::{fetch_height_field_with_progress, map_cache_root};
use toposaic_core::{GenerationSpec, HeightField};

const KILOMETRES_PER_LATITUDE_DEGREE: f64 = 110.574;
const KILOMETRES_PER_LONGITUDE_DEGREE: f64 = 111.32;

fn cases() -> Vec<(&'static str, GenerationSpec)> {
    vec![
        (
            "hakone-20km",
            GenerationSpec {
                center_lat: 35.24943,
                center_lon: 139.0474,
                ground_span_km: 20.0,
                solid_model: true,
                mesh_samples_across: Some(1024),
                place_name: "Gora".into(),
                despike_terrain: false,
                ..GenerationSpec::default()
            },
        ),
        (
            "hakone-20km-mapterhorn",
            GenerationSpec {
                center_lat: 35.24943,
                center_lon: 139.0474,
                ground_span_km: 20.0,
                solid_model: true,
                mesh_samples_across: Some(1024),
                elevation_source: toposaic_core::ElevationSource::Mapterhorn,
                place_name: "Gora".into(),
                despike_terrain: false,
                ..GenerationSpec::default()
            },
        ),
        (
            "rainier-2km",
            GenerationSpec {
                center_lat: 46.8523,
                center_lon: -121.7603,
                ground_span_km: 2.0,
                solid_model: true,
                mesh_samples_across: Some(1024),
                place_name: "Rainier".into(),
                despike_terrain: false,
                ..GenerationSpec::default()
            },
        ),
        (
            "alps-160km",
            GenerationSpec {
                center_lat: 46.0,
                center_lon: 8.0,
                ground_span_km: 160.0,
                solid_model: true,
                mesh_samples_across: Some(1024),
                place_name: "Alps".into(),
                despike_terrain: false,
                ..GenerationSpec::default()
            },
        ),
        (
            "hakone-lake-4km",
            GenerationSpec {
                center_lat: 35.2065,
                center_lon: 138.9985,
                ground_span_km: 4.0,
                solid_model: true,
                mesh_samples_across: Some(1024),
                place_name: "Ashinoko".into(),
                despike_terrain: false,
                ..GenerationSpec::default()
            },
        ),
    ]
}

/// How far below the median of its ring a sample has to sit before it counts
/// as a spike. Real cliffs fall fast but they fall together; a lone pixel
/// dropping this far below every neighbour is a bad reading.
const SPIKE_DROP_M: f32 = 100.0;

struct Spike {
    column: usize,
    row: usize,
    value_m: f32,
    ring_median_m: f32,
}

fn main() -> anyhow::Result<()> {
    let requested: Vec<String> = std::env::args().skip(1).collect();
    let cache = map_cache_root()?;
    for (name, spec) in cases() {
        if !requested.is_empty() && !requested.iter().any(|want| want == name) {
            continue;
        }
        report(name, &spec, &cache)?;
    }
    Ok(())
}

fn report(name: &str, spec: &GenerationSpec, cache: &Path) -> anyhow::Result<()> {
    let raw_started = Instant::now();
    let field = fetch_height_field_with_progress(spec, cache, |_| Ok(()))?;
    let raw_elapsed = raw_started.elapsed();
    let (width, height) = (field.width, field.height);
    let (minimum, maximum) = field.elevation_bounds();
    println!("== {name} ==");
    println!("  grid {width} x {height}");
    println!("  elevation {minimum:.1} m .. {maximum:.1} m");

    let spikes = find_spikes(&field);
    println!(
        "  spikes below neighbourhood by {SPIKE_DROP_M} m: {}",
        spikes.len()
    );

    // What the pass recovers, measured by running the real thing: the same
    // spec with the repair turned back on, fetched through the job runner's own
    // code path rather than a copy of it here.
    let repaired_started = Instant::now();
    let repaired = fetch_height_field_with_progress(
        &GenerationSpec {
            despike_terrain: true,
            ..spec.clone()
        },
        cache,
        |_| Ok(()),
    )?;
    let repaired_elapsed = repaired_started.elapsed();
    println!(
        "  fetch {:.2}s as supplied, {:.2}s repairing",
        raw_elapsed.as_secs_f64(),
        repaired_elapsed.as_secs_f64()
    );
    println!("  manifest records: {}", repaired.source);
    let (repaired_minimum, repaired_maximum) = repaired.elevation_bounds();
    println!("  repaired elevation {repaired_minimum:.1} m .. {repaired_maximum:.1} m");
    // Relief is stretched over the field's whole range, so the real ground only
    // gets the share of it that the bad readings leave behind.
    let raw_span = maximum - minimum;
    let real_span = repaired_maximum - repaired_minimum;
    if raw_span > 0.0 && real_span > 0.0 {
        println!(
            "  relief: real ground takes {:.2} mm of the {:.1} mm as fetched, {:.2} mm repaired",
            spec.relief_mm * real_span / raw_span,
            spec.relief_mm,
            spec.relief_mm * real_span / real_span,
        );
    }

    // A seam in the source mosaic lands on one meridian, so a column that
    // collects many spikes is worth naming separately from scattered noise.
    let mut per_column: std::collections::HashMap<usize, usize> = std::collections::HashMap::new();
    for spike in &spikes {
        *per_column.entry(spike.column).or_default() += 1;
    }
    let mut columns: Vec<(usize, usize)> = per_column.into_iter().collect();
    columns.sort_by_key(|(column, count)| (std::cmp::Reverse(*count), *column));
    print!("  worst columns:");
    for (column, count) in columns.iter().take(5) {
        let (_, lon) = sample_position(spec, &field, *column, 0);
        print!(" col {column} ({lon:.5}) x{count};");
    }
    println!();
    let deepest = spikes
        .iter()
        .min_by(|left, right| left.value_m.total_cmp(&right.value_m));
    if let Some(spike) = deepest {
        let (lat, lon) = sample_position(spec, &field, spike.column, spike.row);
        println!(
            "  deepest {:.1} m at column {} row {} ({lat:.5}, {lon:.5}); ring median {:.1} m",
            spike.value_m, spike.column, spike.row, spike.ring_median_m
        );
    }
    for spike in spikes.iter().take(12) {
        let (lat, lon) = sample_position(spec, &field, spike.column, spike.row);
        println!(
            "    column {:4} row {:4} ({lat:.5}, {lon:.5}) {:9.1} m against ring median {:7.1} m",
            spike.column, spike.row, spike.value_m, spike.ring_median_m
        );
    }
    if spikes.len() > 12 {
        println!("    ... {} more", spikes.len() - 12);
    }
    if let Ok(directory) = std::env::var("SPIKE_CSV_DIR") {
        let mut csv = String::from("column,row,latitude,longitude,value_m,ring_median_m\n");
        for spike in &spikes {
            let (lat, lon) = sample_position(spec, &field, spike.column, spike.row);
            csv.push_str(&format!(
                "{},{},{lat:.6},{lon:.6},{:.2},{:.2}\n",
                spike.column, spike.row, spike.value_m, spike.ring_median_m
            ));
        }
        let path = Path::new(&directory).join(format!("{name}-spikes.csv"));
        std::fs::write(&path, csv)?;
        let mut grid = String::new();
        for row in 0..field.height {
            for column in 0..field.width {
                if column > 0 {
                    grid.push(',');
                }
                grid.push_str(&format!(
                    "{:.1}",
                    field.values_m[row * field.width + column]
                ));
            }
            grid.push('\n');
        }
        std::fs::write(
            Path::new(&directory).join(format!("{name}-field.csv")),
            grid,
        )?;
        println!("  wrote {}", path.display());
    }
    println!();
    Ok(())
}

/// Every sample that sits `SPIKE_DROP_M` below the median of the eight around
/// it. The median ignores a second bad reading in the same ring, which a mean
/// would not.
fn find_spikes(field: &HeightField) -> Vec<Spike> {
    let (width, height) = (field.width, field.height);
    let mut spikes = Vec::new();
    // The ring clamps at the border rather than skipping it: the source seam
    // that carries most of these readings runs right off the edge of the field,
    // so an edge row left unchecked keeps the whole fault alive.
    for row in 0..height {
        for column in 0..width {
            let value = field.values_m[row * width + column];
            let mut ring = [0.0f32; 8];
            let mut index = 0;
            for row_offset in -1i64..=1 {
                for column_offset in -1i64..=1 {
                    if row_offset == 0 && column_offset == 0 {
                        continue;
                    }
                    let neighbour_row =
                        (row as i64 + row_offset).clamp(0, height as i64 - 1) as usize;
                    let neighbour_column =
                        (column as i64 + column_offset).clamp(0, width as i64 - 1) as usize;
                    ring[index] = field.values_m[neighbour_row * width + neighbour_column];
                    index += 1;
                }
            }
            ring.sort_by(f32::total_cmp);
            let median = (ring[3] + ring[4]) / 2.0;
            if median - value >= SPIKE_DROP_M {
                spikes.push(Spike {
                    column,
                    row,
                    value_m: value,
                    ring_median_m: median,
                });
            }
        }
    }
    spikes.sort_by(|left, right| left.value_m.total_cmp(&right.value_m));
    spikes
}

fn sample_position(
    spec: &GenerationSpec,
    field: &HeightField,
    column: usize,
    row: usize,
) -> (f64, f64) {
    let half_latitude = spec.ground_span_km / 2.0 / KILOMETRES_PER_LATITUDE_DEGREE;
    let scale =
        (KILOMETRES_PER_LONGITUDE_DEGREE * spec.center_lat.to_radians().cos().abs()).max(20.0);
    let half_longitude = spec.ground_span_km / 2.0 / scale;
    let u = column as f64 / (field.width - 1) as f64;
    let v = row as f64 / (field.height - 1) as f64;
    let latitude = spec.center_lat - half_latitude + 2.0 * half_latitude * v;
    let longitude = spec.center_lon - half_longitude + 2.0 * half_longitude * u;
    (latitude, longitude)
}
