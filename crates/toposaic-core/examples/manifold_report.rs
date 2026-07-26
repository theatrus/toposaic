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
    AerialStyle, BridgeStructure, BuildingSpec, ColorOutputSpec, GenerationSpec, HeightField,
    RailStyle, SurfaceClass, SurfaceField, TrailRoute, TraySpec,
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

/// Winding imported-trail polylines that cross the synthetic road grid,
/// buildings, and each other — the worst weld case trails can pose.
fn paint_synthetic_trails(spec: &GenerationSpec, field: &mut SurfaceField) {
    for trail in 0..3_u32 {
        let across = 0.2 + trail as f32 * 0.3;
        let points = (0..32)
            .map(|index| {
                let along = index as f32 / 31.0;
                let wobble =
                    0.22 * (along * std::f32::consts::TAU * (1.0 + trail as f32 * 0.8)).cos();
                [(across + wobble).clamp(0.0, 1.0), along]
            })
            .collect::<Vec<_>>();
        field.paint_polyline(
            &points,
            spec.width_mm,
            spec.color_output.trail_width_mm,
            SurfaceClass::Trail,
        );
    }
}

/// Railway lines that cut across the synthetic road grid, the buildings, and
/// the trails at shallow angles, plus one viaduct: the worst weld case a
/// separately-styled rail layer can pose.
fn paint_synthetic_rail(spec: &GenerationSpec, field: &mut SurfaceField) {
    for line in 0..4_u32 {
        let along = 0.15 + line as f32 * 0.22;
        let points = (0..28)
            .map(|index| {
                let progress = index as f32 / 27.0;
                let drift = 0.3 * (progress * std::f32::consts::TAU * 0.5).sin();
                [progress, (along + drift).clamp(0.0, 1.0)]
            })
            .collect::<Vec<_>>();
        field.paint_polyline(
            &points,
            spec.width_mm,
            spec.color_output.rail_width_mm,
            SurfaceClass::Rail,
        );
    }
    // A viaduct high over the terrain, so trails and roads must keep running
    // beneath it while its deck stays a shell of its own.
    field.paint_bridge_polyline_as(
        &[[0.02, 0.34], [0.98, 0.66]],
        spec.width_mm,
        spec.color_output.rail_width_mm * 1.5,
        [1_700.0, 1_700.0],
        SurfaceClass::Rail,
    );
}

/// Aerialway lines drawn across everything else at yet another angle: the
/// lift layer is the last one placed, so it has the most to yield to.
fn paint_synthetic_aerial(spec: &GenerationSpec, field: &mut SurfaceField) {
    for line in 0..3_u32 {
        let along = 0.2 + line as f32 * 0.3;
        let points = (0..24)
            .map(|index| {
                let progress = index as f32 / 23.0;
                [(along + progress * 0.55).clamp(0.0, 1.0), progress]
            })
            .collect::<Vec<_>>();
        field.paint_polyline(
            &points,
            spec.width_mm,
            spec.color_output.aerial_width_mm,
            SurfaceClass::Aerial,
        );
    }
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
    let surface_field = color.then(|| {
        let mut field = synthetic_surface_field(spec, width.max(64), with_bridge);
        if !spec.trails.is_empty() {
            paint_synthetic_trails(spec, &mut field);
        }
        if spec.uses_separate_rail() {
            paint_synthetic_rail(spec, &mut field);
        }
        if spec.uses_separate_aerial() {
            paint_synthetic_aerial(spec, &mut field);
        }
        field
    });
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

    // The rail-family layers default to their own color, so the scenarios
    // that are not about them fold both into the roads. Each scenario then
    // paints exactly the layers its name claims, and the rail and aerialway
    // scenarios below opt in one at a time.
    let color = GenerationSpec {
        buildings: BuildingSpec {
            enabled: true,
            ..BuildingSpec::default()
        },
        color_output: ColorOutputSpec {
            enabled: true,
            roads_enabled: true,
            rail_style: RailStyle::WithRoads,
            aerial_style: AerialStyle::WithRoads,
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

    let trails = GenerationSpec {
        trails: vec![
            TrailRoute {
                name: "Synthetic Loop".into(),
                points: vec![[46.8, -121.8], [46.9, -121.7]],
            },
            TrailRoute {
                name: "Crossing Track".into(),
                points: vec![[46.9, -121.8], [46.8, -121.7]],
            },
            TrailRoute {
                name: "Third Route".into(),
                points: vec![[46.85, -121.8], [46.85, -121.7]],
            },
        ],
        ..color.clone()
    };
    scenario(
        "color 3x3 puzzle with imported trails",
        &trails,
        true,
        false,
        json,
    )?;

    // Roads, buildings, imported trails, and a separately-styled rail layer
    // with a viaduct, all crossing each other.
    let rail = GenerationSpec {
        color_output: ColorOutputSpec {
            rail_enabled: true,
            rail_style: RailStyle::Separate,
            ..trails.color_output.clone()
        },
        ..trails.clone()
    };
    scenario(
        "color 3x3 puzzle with trails and separate railways",
        &rail,
        true,
        false,
        json,
    )?;

    // The full overlay stack: roads, buildings, trails, a separately-styled
    // rail layer with a viaduct, AND a separately-styled aerialway layer, so
    // every yield in the chain has something to yield to.
    let rail_and_aerial = GenerationSpec {
        color_output: ColorOutputSpec {
            aerial_enabled: true,
            aerial_style: AerialStyle::Separate,
            ..rail.color_output.clone()
        },
        ..rail.clone()
    };
    scenario(
        "color 3x3 puzzle with trails, separate railways, and separate aerialways",
        &rail_and_aerial,
        true,
        false,
        json,
    )?;

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
