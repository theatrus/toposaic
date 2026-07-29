//! Runs the marine water pipeline over a saved setup and shows its work:
//! the source notes, an east-west elevation transect before and after the
//! flatten, and a PPM of the final classes.
//!
//! Usage: marine_probe <setup.json> <out.ppm> [level_m] [v]

use std::{env, fs};

use toposaic_api::diagnostics::{
    apply_marine_water, fetch_height_field_with_progress, fetch_surface_field, map_cache_root,
};
use toposaic_core::{GenerationSpec, MarineGeometry, MarineLevel, SurfaceClass};

fn class_color(class: SurfaceClass) -> [u8; 3] {
    match class {
        SurfaceClass::Rock => [124, 116, 104],
        SurfaceClass::Forest => [40, 84, 58],
        SurfaceClass::Snow => [244, 243, 236],
        SurfaceClass::Water => [47, 118, 181],
        SurfaceClass::Road => [216, 163, 60],
        SurfaceClass::Building => [184, 168, 144],
        other => match format!("{other:?}").as_str() {
            "Trail" => [214, 51, 108],
            "Rail" => [196, 61, 61],
            "Aerial" => [108, 76, 182],
            "Ferry" => [15, 140, 140],
            "RouteTrail" => [255, 120, 60],
            "Aviation" => [30, 32, 36],
            _ => [255, 0, 255],
        },
    }
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let setup = env::args().nth(1).expect("setup json path");
    let output = env::args().nth(2).expect("output ppm path");
    let level_m = env::args()
        .nth(3)
        .map(|value| value.parse::<f32>().expect("level in metres"));
    let v = env::args()
        .nth(4)
        .map(|value| value.parse::<f32>().expect("transect v"))
        .unwrap_or(0.5);
    let mut spec: GenerationSpec = serde_json::from_str(&fs::read_to_string(setup)?)?;
    spec.marine.geometry = MarineGeometry::FlatSurface;
    if let Some(level_m) = level_m {
        spec.marine.level = MarineLevel::Custom;
        spec.marine.custom_offset_m = level_m;
    }
    spec.validate()?;
    let cache_dir = map_cache_root()?;
    let mut height_field =
        fetch_height_field_with_progress(&spec, &cache_dir.join("elevation"), |_| Ok(()))?;
    let mut field = fetch_surface_field(&spec, &height_field, &cache_dir)?;
    let before = height_field.clone();
    apply_marine_water(&spec, &mut height_field, &mut field, &cache_dir);

    println!("--- marine notes ---");
    for note in field.source.split("; ") {
        if [
            "marine",
            "WARNING",
            "flood fill",
            "coastline",
            "samples",
            "plane",
        ]
        .iter()
        .any(|key| note.contains(key))
        {
            println!("  {note}");
        }
    }
    println!("--- transect at v={v} (u, before m, after m, class) ---");
    for step in 0..24 {
        let u = step as f32 / 23.0;
        println!(
            "  u={u:.3}  {:8.2} -> {:8.2}  {:?}",
            before.elevation_m_at(u, v),
            height_field.elevation_m_at(u, v),
            field.class_at(u, v)
        );
    }

    let size = 1400usize;
    let mut pixels = Vec::with_capacity(size * size * 3);
    for y in 0..size {
        let v = y as f32 / (size - 1) as f32;
        for x in 0..size {
            let u = x as f32 / (size - 1) as f32;
            pixels.extend(class_color(field.class_at(u, v)));
        }
    }
    let mut ppm = format!("P6\n{size} {size}\n255\n").into_bytes();
    ppm.extend(pixels);
    fs::write(&output, ppm)?;
    eprintln!("wrote {output}");
    Ok(())
}
