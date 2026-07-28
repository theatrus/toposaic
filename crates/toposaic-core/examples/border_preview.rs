//! Before/after images for the class-border smoother and the steep-slope
//! forest gate, on synthetic data. Writes plain PPM files.
//!
//! Usage:
//!   border_preview [out_dir]
//!
//! Defaults to a temp directory; the written paths are printed either way.

use std::{
    env, fs,
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

use toposaic_core::{HeightField, SlopeGates, SteepForestTarget, SurfaceClass, SurfaceField};

/// Sample grid: 481 samples over a 480 m span is 1 m per sample, so each
/// 10 m native cell covers 10 samples, like a zoomed-in map.
const SIZE: usize = 481;
const GROUND_SPAN_M: f32 = 480.0;
const NATIVE_RESOLUTION_M: f32 = 10.0;
const NATIVE_SIDE: usize = 48;
/// Default indicator-kriging parameters, matching `ColorOutputSpec`.
const RANGE_CELLS: f32 = 2.5;
const NUGGET: f32 = 0.05;

fn class_color(class: SurfaceClass) -> [u8; 3] {
    match class {
        SurfaceClass::Forest => [0x28, 0x54, 0x3A],
        SurfaceClass::Snow => [0xF4, 0xF3, 0xEC],
        SurfaceClass::Water => [0x2F, 0x76, 0xB5],
        SurfaceClass::Road => [0xD8, 0xA3, 0x3C],
        SurfaceClass::Building => [0xB8, 0xA8, 0x90],
        SurfaceClass::Trail => [0xD6, 0x33, 0x6C],
        SurfaceClass::Rail => [0x4A, 0x55, 0x68],
        SurfaceClass::Aerial => [0x6C, 0x4C, 0xB6],
        SurfaceClass::Marker => [0xE2, 0x4A, 0x33],
        SurfaceClass::RouteTrail => [0xD8, 0xA3, 0x3C],
        SurfaceClass::Ferry => [0x0F, 0x8C, 0x8C],
        SurfaceClass::Aviation => [0x4A, 0x4E, 0x54],
        SurfaceClass::Rock => [0x7C, 0x74, 0x68],
    }
}

fn write_ppm(path: &Path, width: usize, height: usize, classes: &[SurfaceClass]) {
    let mut bytes = format!("P6\n{width} {height}\n255\n").into_bytes();
    for class in classes {
        bytes.extend_from_slice(&class_color(*class));
    }
    fs::write(path, bytes).expect("write PPM");
    println!("{}", path.display());
}

/// Native-resolution land cover: a lake, a snow band, and forest patches
/// over rock, evaluated per 10 m cell.
fn native_class(cell_x: usize, cell_y: usize) -> SurfaceClass {
    let u = cell_x as f32 / (NATIVE_SIDE - 1) as f32;
    let v = cell_y as f32 / (NATIVE_SIDE - 1) as f32;
    let lake = ((u - 0.28).powi(2) + (v - 0.68).powi(2)).sqrt();
    if lake < 0.16 + 0.03 * ((u * 23.0).sin() + (v * 17.0).cos()) {
        return SurfaceClass::Water;
    }
    if v < 0.22 + 0.06 * (u * std::f32::consts::TAU * 1.7).sin() {
        return SurfaceClass::Snow;
    }
    let patches = (u * 9.0).sin() * (v * 7.0).cos() + 0.5 * ((u + v) * 11.0).sin();
    if patches > 0.15 {
        SurfaceClass::Forest
    } else {
        SurfaceClass::Rock
    }
}

/// Nearest-neighbour upsample of the native grid, exactly how WorldCover
/// pixels land on a fine sample grid today.
fn blocky_field() -> SurfaceField {
    let classes = (0..SIZE * SIZE)
        .map(|index| {
            let x = (index % SIZE) as f32 / (SIZE - 1) as f32;
            let y = (index / SIZE) as f32 / (SIZE - 1) as f32;
            native_class(
                (x * (NATIVE_SIDE - 1) as f32).round() as usize,
                (y * (NATIVE_SIDE - 1) as f32).round() as usize,
            )
        })
        .collect();
    SurfaceField::new(SIZE, SIZE, classes, "border preview").unwrap()
}

/// A flat plain with a sheer-sided butte, plus forest painted everywhere:
/// the Devils Tower failure case for 10 m land cover.
fn butte_scene() -> (HeightField, SurfaceField) {
    let values = (0..SIZE * SIZE)
        .map(|index| {
            let x = (index % SIZE) as f32 / (SIZE - 1) as f32;
            let y = (index / SIZE) as f32 / (SIZE - 1) as f32;
            let radius = ((x - 0.5).powi(2) + (y - 0.5).powi(2)).sqrt();
            // 250 m of rise packed into a 12 m wide rim: about 87 degrees.
            let rim = ((0.15 - radius) / 0.025).clamp(0.0, 1.0);
            250.0 * rim * rim * (3.0 - 2.0 * rim)
        })
        .collect();
    let heights = HeightField::new(SIZE, SIZE, values, "butte").unwrap();
    let field = SurfaceField::new(
        SIZE,
        SIZE,
        vec![SurfaceClass::Forest; SIZE * SIZE],
        "butte cover",
    )
    .unwrap();
    (heights, field)
}

fn main() {
    let out_dir = env::args().nth(1).map(PathBuf::from).unwrap_or_else(|| {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        env::temp_dir().join(format!("toposaic-border-preview-{unique}"))
    });
    fs::create_dir_all(&out_dir).expect("create output directory");

    let blocky = blocky_field();
    write_ppm(
        &out_dir.join("borders-before.ppm"),
        SIZE,
        SIZE,
        &blocky.classes,
    );
    let mut smoothed = blocky.clone();
    smoothed.smooth_class_borders(NATIVE_RESOLUTION_M, GROUND_SPAN_M, RANGE_CELLS, NUGGET);
    write_ppm(
        &out_dir.join("borders-after.ppm"),
        SIZE,
        SIZE,
        &smoothed.classes,
    );

    let (heights, mut cover) = butte_scene();
    write_ppm(
        &out_dir.join("slope-gate-before.ppm"),
        SIZE,
        SIZE,
        &cover.classes,
    );
    let demoted = cover
        .demote_steep_classes(
            &heights,
            GROUND_SPAN_M,
            SlopeGates {
                forest_limit_degrees: Some(55.0),
                steep_forest_target: SteepForestTarget::Rock,
                snow_limit_degrees: Some(65.0),
            },
        )
        .total();
    write_ppm(
        &out_dir.join("slope-gate-after.ppm"),
        SIZE,
        SIZE,
        &cover.classes,
    );
    println!("slope gate reclassified {demoted} samples");
}
