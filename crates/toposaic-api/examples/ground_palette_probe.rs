//! Runs the satellite ground-palette pipeline over a saved setup and shows
//! its work: the source notes, the discovered palette, and a PPM where
//! every sample wears its palette entry's color — the mapped class color
//! where imagery is missing or the sample is an overlay.
//!
//! Usage: ground_palette_probe <setup.json> <out.ppm> [satellite|hybrid]

use std::{env, fs};

use toposaic_api::diagnostics::{
    fetch_height_field_with_progress, fetch_surface_field, map_cache_root,
};
use toposaic_core::{GenerationSpec, GroundColorMode, SurfaceClass};

fn class_color(class: SurfaceClass) -> [u8; 3] {
    match class {
        SurfaceClass::Rock => [124, 116, 104],
        SurfaceClass::Forest => [40, 84, 58],
        SurfaceClass::Snow => [244, 243, 236],
        SurfaceClass::Water => [47, 118, 181],
        SurfaceClass::Road => [216, 163, 60],
        SurfaceClass::Building => [184, 168, 144],
        _ => [255, 0, 255],
    }
}

fn hex_bytes(color: &str) -> [u8; 3] {
    let byte = |range| u8::from_str_radix(&color[1..][range], 16).unwrap_or(255);
    [byte(0..2), byte(2..4), byte(4..6)]
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let setup = env::args().nth(1).expect("setup json path");
    let output = env::args().nth(2).expect("output ppm path");
    let mode = match env::args().nth(3).as_deref() {
        Some("satellite") => GroundColorMode::Satellite,
        _ => GroundColorMode::Hybrid,
    };
    let mut spec: GenerationSpec = serde_json::from_str(&fs::read_to_string(setup)?)?;
    spec.color_output.ground_palette.ground_colors = mode;
    spec.validate()?;
    let cache_dir = map_cache_root()?;
    let height_field =
        fetch_height_field_with_progress(&spec, &cache_dir.join("elevation"), |_| Ok(()))?;
    let field = fetch_surface_field(&spec, &height_field, &cache_dir)?;

    println!("--- source notes ---");
    for note in field.source.split("; ") {
        println!("  {note}");
    }
    println!("--- palette ---");
    let Some(palette) = field.ground_palette() else {
        println!("  none resolved");
        return Ok(());
    };
    for entry in &palette.entries {
        println!(
            "  {:<10} {}  {:5.1}%  group {:?}",
            entry.name,
            entry.color,
            entry.share * 100.0,
            entry.group
        );
    }

    let size = 1400usize;
    let mut pixels = Vec::with_capacity(size * size * 3);
    let mut painted = 0usize;
    let mut fallback = 0usize;
    for y in 0..size {
        let v = y as f32 / (size - 1) as f32;
        for x in 0..size {
            let u = x as f32 / (size - 1) as f32;
            match field.ground_material_at(u, v) {
                Some(index) => {
                    painted += 1;
                    pixels.extend(hex_bytes(&palette.entries[index as usize].color));
                }
                None => {
                    fallback += 1;
                    pixels.extend(class_color(field.terrain_class_at(u, v)));
                }
            }
        }
    }
    println!("--- coverage: {painted} palette samples, {fallback} class-color fallbacks ---");
    let mut ppm = format!("P6\n{size} {size}\n255\n").into_bytes();
    ppm.extend(pixels);
    fs::write(&output, ppm)?;
    eprintln!("wrote {output}");
    Ok(())
}
