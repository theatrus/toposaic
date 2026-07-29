//! Renders a saved setup's surface classes to a PPM image so class
//! placement can be inspected directly: every sample colored by
//! `class_at`, the class a generated artifact would color that spot.
//!
//! Usage: surface_probe <setup.json> <out.ppm> [u0 v0 u1 v1] [mode]
//! Modes: classes (default) — `class_at`, every overlay applied;
//! terrain — `terrain_class_at`, what the terrain triangles get painted.

use std::{env, fs};

use toposaic_api::diagnostics::{
    fetch_height_field_with_progress, fetch_surface_field, map_cache_root,
};
use toposaic_core::{GenerationSpec, SurfaceClass};

fn class_color(class: SurfaceClass) -> [u8; 3] {
    match class {
        SurfaceClass::Rock => [124, 116, 104],
        SurfaceClass::Forest => [40, 84, 58],
        SurfaceClass::Snow => [244, 243, 236],
        SurfaceClass::Water => [47, 118, 181],
        SurfaceClass::Road => [216, 163, 60],
        SurfaceClass::Building => [184, 168, 144],
        _ => {
            let name = format!("{class:?}");
            match name.as_str() {
                "Trail" => [214, 51, 108],
                "Rail" => [196, 61, 61],
                "Aerial" => [108, 76, 182],
                "Ferry" => [15, 140, 140],
                "RouteTrail" => [255, 120, 60],
                "Aviation" => [30, 32, 36],
                _ => [255, 0, 255],
            }
        }
    }
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let setup = env::args().nth(1).expect("setup json path");
    let output = env::args().nth(2).expect("output ppm path");
    let mut arguments = env::args().skip(3).collect::<Vec<_>>();
    let mode = match arguments.last().map(String::as_str) {
        Some(name @ ("terrain" | "conflict" | "slope")) => {
            let name = name.to_string();
            arguments.pop();
            name
        }
        _ => "classes".to_string(),
    };
    let mode = mode.as_str();
    let window = arguments
        .iter()
        .map(|value| value.parse::<f32>().unwrap())
        .collect::<Vec<_>>();
    let (u0, v0, u1, v1) = match window.as_slice() {
        [u0, v0, u1, v1] => (*u0, *v0, *u1, *v1),
        _ => (0.0, 0.0, 1.0, 1.0),
    };
    let spec: GenerationSpec = serde_json::from_str(&fs::read_to_string(setup)?)?;
    spec.validate()?;
    let cache_dir = map_cache_root()?;
    let height_field =
        fetch_height_field_with_progress(&spec, &cache_dir.join("elevation"), |_| Ok(()))?;
    let field = fetch_surface_field(&spec, &height_field, &cache_dir)?;

    let size = 1400usize;
    let mut pixels = Vec::with_capacity(size * size * 3);
    for y in 0..size {
        let v = v0 + (v1 - v0) * y as f32 / (size - 1) as f32;
        for x in 0..size {
            let u = u0 + (u1 - u0) * x as f32 / (size - 1) as f32;
            match mode {
                "terrain" => pixels.extend(class_color(field.terrain_class_at(u, v))),
                // Pavement whose ground says water, in magenta over a
                // dimmed class map: the exact spots where a runway would
                // stand in the bay.
                "conflict" => {
                    let class = field.class_at(u, v);
                    let name = format!("{class:?}");
                    let terrain = field.terrain_class_at(u, v);
                    if name == "Aviation" && terrain == SurfaceClass::Water {
                        pixels.extend([255, 0, 255]);
                    } else {
                        pixels.extend(class_color(class).map(|value| value / 2));
                    }
                }
                // Slope in degrees as brightness (white at 60), the
                // centered two-sample gradient demote_steep_classes uses;
                // water-classed samples tinted blue so the shoreline reads.
                "slope" => {
                    let du = 1.0 / (field.width - 1) as f32;
                    let dv = 1.0 / (field.height - 1) as f32;
                    let span_m = spec.ground_span_km as f32 * 1000.0;
                    let (su0, su1) = ((u - du).max(0.0), (u + du).min(1.0));
                    let (sv0, sv1) = ((v - dv).max(0.0), (v + dv).min(1.0));
                    let rise_x =
                        height_field.elevation_m_at(su1, v) - height_field.elevation_m_at(su0, v);
                    let rise_y =
                        height_field.elevation_m_at(u, sv1) - height_field.elevation_m_at(u, sv0);
                    let gradient =
                        (rise_x / ((su1 - su0) * span_m)).hypot(rise_y / ((sv1 - sv0) * span_m));
                    let degrees = gradient.atan().to_degrees();
                    let level = ((degrees / 60.0).min(1.0) * 255.0) as u8;
                    if field.class_at(u, v) == SurfaceClass::Water {
                        pixels.extend([level / 3, level / 3, 200]);
                    } else {
                        pixels.extend([level, level, level]);
                    }
                }
                _ => pixels.extend(class_color(field.class_at(u, v))),
            }
        }
    }
    let mut ppm = format!("P6\n{size} {size}\n255\n").into_bytes();
    ppm.extend(pixels);
    fs::write(&output, ppm)?;
    eprintln!("wrote {output}");
    Ok(())
}
