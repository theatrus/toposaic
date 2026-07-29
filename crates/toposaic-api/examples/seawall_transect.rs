//! Prints elevation and printed slope along one east-west transect of a
//! saved setup, to see what the DEM actually does at a shoreline.
//! Usage: seawall_transect <setup.json> <v> [u0 u1]

use std::{env, fs};

use toposaic_api::diagnostics::{
    fetch_height_field_with_progress, fetch_surface_field, map_cache_root,
};
use toposaic_core::GenerationSpec;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let setup = env::args().nth(1).expect("setup json path");
    let v: f32 = env::args().nth(2).expect("v").parse()?;
    let u0: f32 = env::args()
        .nth(3)
        .map(|a| a.parse().unwrap())
        .unwrap_or(0.0);
    let u1: f32 = env::args()
        .nth(4)
        .map(|a| a.parse().unwrap())
        .unwrap_or(1.0);
    let spec: GenerationSpec = serde_json::from_str(&fs::read_to_string(setup)?)?;
    spec.validate()?;
    let cache = map_cache_root()?;
    let field = fetch_height_field_with_progress(&spec, &cache.join("elevation"), |_| Ok(()))?;
    let surface = fetch_surface_field(&spec, &field, &cache)?;
    for line in surface.source.split(';') {
        eprintln!("source: {}", line.trim());
    }
    let span_m = (spec.ground_span_km * 1_000.0) as f32;
    let (low, high) = field
        .values_m
        .iter()
        .filter(|value| value.is_finite())
        .fold((f32::INFINITY, f32::NEG_INFINITY), |(low, high), value| {
            (low.min(*value), high.max(*value))
        });
    let vertical = spec.relief_mm / (high - low);
    let horizontal = spec.width_mm / span_m;
    let exaggeration = vertical / horizontal;
    println!("range {low:.2}..{high:.2} m, exaggeration {exaggeration:.1}");
    // Sample at the surface-raster pitch the demote pass uses.
    let samples = 1024usize;
    let du = 1.0 / (samples - 1) as f32;
    let mut previous = f32::NAN;
    for index in 0..samples {
        let u = index as f32 / (samples - 1) as f32;
        if u < u0 || u > u1 {
            continue;
        }
        let e = field.elevation_m_at(u, v);
        let ua = (u - du).max(0.0);
        let ub = (u + du).min(1.0);
        let gradient =
            (field.elevation_m_at(ub, v) - field.elevation_m_at(ua, v)) / ((ub - ua) * span_m);
        let printed = (gradient * exaggeration).atan().to_degrees();
        if (e - previous).abs() > 0.15 || printed.abs() > 20.0 {
            println!(
                "u={u:.4}  {e:8.2} m  ground {:5.2}  printed {printed:6.1} deg  class {:?}  terrain {:?}",
                gradient.atan().to_degrees(),
                surface.class_at(u, v),
                surface.terrain_class_at(u, v),
            );
        }
        previous = e;
    }
    Ok(())
}
