//! Manifold-defect report over synthetic (offline) generations.
//!
//! Builds every piece of several representative project specs exactly as
//! production does, then classifies mesh defects in three views: the
//! in-memory index topology, an STL-style exact-position weld, and a
//! 3MF-style 5-decimal weld (what slicers actually see). See
//! `toposaic_core::analysis` for the definitions.
//!
//! Usage: manifold_report [--json]

use std::env;

use toposaic_core::analysis::{analyze_project, summarize};
use toposaic_core::{
    BridgeStructure, BuildingSpec, ColorOutputSpec, GenerationSpec, HeightField, SurfaceClass,
    SurfaceField, TraySpec,
};

/// Small deterministic generator so synthetic roads and buildings are stable
/// across runs.
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
    HeightField::new(width, height, values_m, "synthetic manifold surface").unwrap()
}

fn synthetic_surface_field(spec: &GenerationSpec, size: usize, with_bridge: bool) -> SurfaceField {
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
    let mut field = SurfaceField::new(size, size, classes, "synthetic manifold classes").unwrap();

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
    if with_bridge {
        field.paint_bridge_polyline(
            &[[0.05, 0.48], [0.95, 0.52]],
            spec.width_mm,
            1.4,
            [1_650.0, 1_650.0],
        );
    }
    field
}

fn scenario(
    name: &str,
    spec: &GenerationSpec,
    color: bool,
    with_bridge: bool,
    json: bool,
) -> Result<(), Box<dyn std::error::Error>> {
    let (width, height) = spec.sample_grid_dimensions(spec.effective_samples_per_piece());
    let height_field = synthetic_height_field(width, height);
    let surface_field = color.then(|| synthetic_surface_field(spec, width.max(64), with_bridge));
    let reports = analyze_project(spec, Some(&height_field), surface_field.as_ref())?;
    if json {
        println!("{}", serde_json::to_string_pretty(&reports)?);
    } else {
        print!("{}", summarize(name, &reports));
        println!();
    }
    Ok(())
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let json = env::args().any(|argument| argument == "--json");

    let plain = GenerationSpec::default();
    scenario("plain 3x3 puzzle", &plain, false, false, json)?;

    let color = GenerationSpec {
        buildings: BuildingSpec {
            enabled: true,
            ..BuildingSpec::default()
        },
        color_output: ColorOutputSpec {
            enabled: true,
            roads_enabled: true,
            ..ColorOutputSpec::default()
        },
        ..GenerationSpec::default()
    };
    scenario(
        "color 3x3 puzzle with roads+buildings",
        &color,
        true,
        false,
        json,
    )?;

    let solid = GenerationSpec {
        solid_model: true,
        ..color.clone()
    };
    scenario("solid color model", &solid, true, false, json)?;

    for structure in [BridgeStructure::Floating, BridgeStructure::Supported] {
        let bridges = GenerationSpec {
            color_output: ColorOutputSpec {
                bridge_structure: structure,
                ..color.color_output.clone()
            },
            ..color.clone()
        };
        scenario(
            &format!("color 3x3 puzzle with {structure:?} bridge"),
            &bridges,
            true,
            true,
            json,
        )?;
    }

    let tray = GenerationSpec {
        tray: TraySpec {
            enabled: true,
            ..TraySpec::default()
        },
        ..GenerationSpec::default()
    };
    scenario("plain 3x3 puzzle with tray", &tray, false, false, json)?;

    Ok(())
}
