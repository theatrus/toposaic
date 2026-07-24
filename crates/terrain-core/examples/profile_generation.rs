use std::{
    env, fs,
    time::{Instant, SystemTime, UNIX_EPOCH},
};

use terrain_core::{GenerationSpec, HeightField, generate_project_with_height_field};

fn argument(index: usize, default: u32) -> u32 {
    env::args()
        .nth(index)
        .and_then(|value| value.parse().ok())
        .unwrap_or(default)
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let rows = argument(1, 6);
    let columns = argument(2, rows);
    let samples_per_piece = argument(3, 96);
    let field_width = (columns * samples_per_piece + 1) as usize;
    let field_height = (rows * samples_per_piece + 1) as usize;
    let values_m = (0..field_height)
        .flat_map(|y| {
            (0..field_width).map(move |x| {
                let u = x as f32 / (field_width - 1) as f32;
                let v = y as f32 / (field_height - 1) as f32;
                1_200.0
                    + 900.0 * (u * std::f32::consts::TAU * 2.0).sin()
                    + 650.0 * (v * std::f32::consts::TAU * 1.5).cos()
                    + 300.0 * ((u + v) * std::f32::consts::TAU * 3.0).sin()
            })
        })
        .collect();
    let height_field = HeightField::new(
        field_width,
        field_height,
        values_m,
        "synthetic profile surface",
    )?;
    let spec = GenerationSpec {
        rows,
        columns,
        samples_per_piece,
        tray: Default::default(),
        ..GenerationSpec::default()
    };
    let unique = SystemTime::now().duration_since(UNIX_EPOCH)?.as_nanos();
    let output_dir =
        env::temp_dir().join(format!("toposaic-profile-{}-{unique}", std::process::id()));

    let started = Instant::now();
    let result = generate_project_with_height_field(&spec, &height_field, &output_dir);
    let elapsed = started.elapsed();
    let cleanup = fs::remove_dir_all(&output_dir);
    result?;
    cleanup?;

    println!(
        "{}x{} pieces at {} samples: {:.3}s",
        rows,
        columns,
        samples_per_piece,
        elapsed.as_secs_f64()
    );
    Ok(())
}
