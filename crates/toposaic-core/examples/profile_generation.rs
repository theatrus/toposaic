//! Wall-time profile of the full generation path with synthetic inputs.
//!
//! Usage:
//!   profile_generation [solid|puzzle] [samples_across] \
//!       [plain|color|color-smooth] [rows] [columns] [out_dir]
//!
//! `color-smooth` matches `color` but uses a 1 km ground span, builds the
//! surface raster at the full sample grid, and reports the extra cost of
//! the steep-slope forest gate and the kriged border smoothing.
//!
//! Defaults: puzzle 1024 plain 6 6, artifacts in a temp dir that is removed
//! after the run. Pass an out_dir to keep the artifacts (for hashing).

use std::{
    env, fs,
    path::PathBuf,
    time::{Instant, SystemTime, UNIX_EPOCH},
};

use toposaic_core::{
    BuildingSpec, ColorOutputSpec, GenerationSpec, HeightField, NativeClassGrid, SteepForestTarget,
    SurfaceClass, SurfaceField, generate_project_with_fields,
};

fn argument(index: usize, default: &str) -> String {
    env::args().nth(index).unwrap_or_else(|| default.into())
}

/// Small deterministic generator so synthetic roads and buildings are stable
/// across runs and across code changes.
struct Lcg(u64);

impl Lcg {
    fn next_f32(&mut self) -> f32 {
        self.0 = self
            .0
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        (self.0 >> 40) as f32 / (1_u64 << 24) as f32
    }
}

fn synthetic_height_field(width: usize, height: usize) -> HeightField {
    let values_m = (0..height)
        .flat_map(|y| {
            (0..width).map(move |x| {
                let u = x as f32 / (width - 1) as f32;
                let v = y as f32 / (height - 1) as f32;
                1_200.0
                    + 900.0 * (u * std::f32::consts::TAU * 2.0).sin()
                    + 650.0 * (v * std::f32::consts::TAU * 1.5).cos()
                    + 300.0 * ((u + v) * std::f32::consts::TAU * 3.0).sin()
            })
        })
        .collect();
    HeightField::new(width, height, values_m, "synthetic profile surface").unwrap()
}

fn synthetic_surface_field(spec: &GenerationSpec, size: usize) -> SurfaceField {
    let classes = (0..size)
        .flat_map(|y| {
            (0..size).map(move |x| {
                let u = x as f32 / (size - 1) as f32;
                let v = y as f32 / (size - 1) as f32;
                let bands = (u * std::f32::consts::TAU * 2.0).sin()
                    + (v * std::f32::consts::TAU * 1.5).cos();
                let patches = (u * 23.0).sin() * (v * 19.0).cos();
                if bands < -1.1 {
                    SurfaceClass::Water
                } else if bands > 1.3 {
                    SurfaceClass::Snow
                } else if patches > 0.25 {
                    SurfaceClass::Forest
                } else {
                    SurfaceClass::Rock
                }
            })
        })
        .collect();
    let mut field = SurfaceField::new(size, size, classes, "synthetic profile classes").unwrap();

    let mut rng = Lcg(0x5eed_cafe_f00d_0001);
    for road in 0..14_u32 {
        let across = (road as f32 + 0.5) / 14.0;
        let frequency = 1.5 + (road % 3) as f32 * 0.7;
        let points = (0..24)
            .map(|index| {
                let along = index as f32 / 23.0;
                let wobble = 0.18 * (along * std::f32::consts::TAU * frequency + road as f32).sin()
                    + (rng.next_f32() - 0.5) * 0.01;
                let coordinates = [along, (across + wobble).clamp(0.0, 1.0)];
                if road % 2 == 0 {
                    coordinates
                } else {
                    [coordinates[1], coordinates[0]]
                }
            })
            .collect::<Vec<_>>();
        field.paint_polyline(
            &points,
            spec.width_mm,
            spec.color_output.road_width_mm,
            SurfaceClass::Road,
        );
    }
    for _ in 0..120 {
        let center = [0.05 + rng.next_f32() * 0.9, 0.05 + rng.next_f32() * 0.9];
        let half = [
            0.002 + rng.next_f32() * 0.004,
            0.002 + rng.next_f32() * 0.004,
        ];
        field.paint_building(
            &[
                [center[0] - half[0], center[1] - half[1]],
                [center[0] + half[0], center[1] - half[1]],
                [center[0] + half[0], center[1] + half[1]],
                [center[0] - half[0], center[1] + half[1]],
            ],
            6.0 + rng.next_f32() * 34.0,
        );
    }
    field
}

/// A synthetic 10 m class lattice covering the sample grid with a nonzero
/// phase, standing in for the true WorldCover window production feeds the
/// native smoothing path.
fn synthetic_native_grid(sample_width: usize, ground_span_m: f32) -> NativeClassGrid {
    let cells = (ground_span_m / 10.0).round() as usize;
    let x_scale = cells as f64 / (sample_width - 1) as f64;
    let x_offset = 1.37;
    let native_size = cells + 4;
    let classes = (0..native_size * native_size)
        .map(|index| {
            let node_x = index % native_size;
            let node_y = index / native_size;
            let u = ((node_x as f64 - x_offset) / x_scale / (sample_width - 1) as f64) as f32;
            let v = ((node_y as f64 - x_offset) / x_scale / (sample_width - 1) as f64) as f32;
            let bands =
                (u * std::f32::consts::TAU * 2.0).sin() + (v * std::f32::consts::TAU * 1.5).cos();
            let patches = (u * 23.0).sin() * (v * 19.0).cos();
            if bands < -1.1 {
                SurfaceClass::Water
            } else if bands > 1.3 {
                SurfaceClass::Snow
            } else if patches > 0.25 {
                SurfaceClass::Forest
            } else {
                SurfaceClass::Rock
            }
        })
        .collect();
    NativeClassGrid::new(
        native_size,
        native_size,
        classes,
        x_scale,
        x_offset,
        x_scale,
        x_offset,
    )
    .unwrap()
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mode = argument(1, "puzzle");
    let samples_across: u32 = argument(2, "1024").parse()?;
    let surface = argument(3, "plain");
    let rows: u32 = argument(4, "6").parse()?;
    let columns: u32 = argument(5, "6").parse()?;
    let keep_dir = env::args().nth(6).map(PathBuf::from);
    let (color, smooth) = match surface.as_str() {
        "color" => (true, false),
        "color-smooth" => (true, true),
        "plain" => (false, false),
        other => return Err(format!("unknown surface mode {other}").into()),
    };

    let spec = GenerationSpec {
        // A close-in span, where 10 m land-cover pixels turn blocky and the
        // border smoother has native cells to reconstruct.
        ground_span_km: if smooth { 1.0 } else { 18.0 },
        rows,
        columns,
        solid_model: mode == "solid",
        mesh_samples_across: Some(samples_across),
        overlay_samples_across: Some(samples_across),
        tray: Default::default(),
        buildings: BuildingSpec {
            enabled: color,
            ..BuildingSpec::default()
        },
        color_output: ColorOutputSpec {
            enabled: color,
            ..ColorOutputSpec::default()
        },
        ..GenerationSpec::default()
    };
    let (field_width, field_height) =
        spec.sample_grid_dimensions(spec.effective_samples_per_piece());
    let height_field = synthetic_height_field(field_width, field_height);
    let surface_size = if smooth { field_width } else { 641 };
    let mut surface_field = color.then(|| synthetic_surface_field(&spec, surface_size));
    if let (true, Some(field)) = (smooth, surface_field.as_mut()) {
        let ground_span_m = (spec.ground_span_km * 1_000.0) as f32;
        let gate_started = Instant::now();
        let demoted = field
            .demote_steep_forest(&height_field, ground_span_m, 55.0, SteepForestTarget::Rock)
            .total();
        let gate_elapsed = gate_started.elapsed();
        // Time the recovered-grid fallback on a clone, then the true-lattice
        // path (what production uses) on the field itself.
        let mut recovered = field.clone();
        let recovered_started = Instant::now();
        recovered.smooth_class_borders(
            10.0,
            ground_span_m,
            spec.color_output.border_smoothing_range_cells,
            spec.color_output.border_smoothing_nugget,
        );
        let recovered_elapsed = recovered_started.elapsed();
        let native = synthetic_native_grid(field_width, ground_span_m);
        let native_started = Instant::now();
        field.smooth_class_borders_with_native(
            &native,
            spec.color_output.border_smoothing_range_cells,
            spec.color_output.border_smoothing_nugget,
        );
        let native_elapsed = native_started.elapsed();
        println!(
            "slope gate: {:.3}s ({demoted} demoted); border smoothing: {:.3}s recovered, {:.3}s native lattice",
            gate_elapsed.as_secs_f64(),
            recovered_elapsed.as_secs_f64(),
            native_elapsed.as_secs_f64()
        );
    }

    let output_dir = keep_dir.clone().unwrap_or_else(|| {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        env::temp_dir().join(format!("toposaic-profile-{}-{unique}", std::process::id()))
    });

    let started = Instant::now();
    let result =
        generate_project_with_fields(&spec, &height_field, surface_field.as_ref(), &output_dir);
    let elapsed = started.elapsed();
    let cleanup = if keep_dir.is_none() {
        fs::remove_dir_all(&output_dir)
    } else {
        Ok(())
    };
    result?;
    cleanup?;

    println!(
        "{mode} {}x{columns} at {samples_across} samples across ({surface}): {:.3}s",
        rows,
        elapsed.as_secs_f64()
    );
    Ok(())
}
