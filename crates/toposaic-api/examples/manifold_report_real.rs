//! Manifold-defect report over real generations (network data).
//!
//! Fetches elevation, land cover, and OpenStreetMap overlays exactly as the
//! API server's job runner does, builds every piece of several representative
//! specs, and prints the defect summary from `toposaic_core::analysis`.
//! Requires network access (or a warm map cache).
//!
//! Usage: manifold_report_real [scenario-name ...]
//! With no arguments every scenario runs.

use std::env;

use toposaic_api::diagnostics::{
    fetch_height_field_with_progress, fetch_surface_field, map_cache_root,
};
use toposaic_core::analysis::{analyze_project, summarize};
use toposaic_core::{BridgeStructure, BuildingSpec, ColorOutputSpec, GenerationSpec, TraySpec};

fn town_spec(color: bool) -> GenerationSpec {
    GenerationSpec {
        center_lat: 47.6,
        center_lon: -122.33,
        ground_span_km: 8.0,
        rows: 10,
        columns: 10,
        width_mm: 220.0,
        place_name: "Seattle".into(),
        buildings: BuildingSpec {
            enabled: color,
            ..BuildingSpec::default()
        },
        color_output: ColorOutputSpec {
            enabled: color,
            roads_enabled: color,
            ..ColorOutputSpec::default()
        },
        ..GenerationSpec::default()
    }
}

fn scenarios() -> Vec<(&'static str, GenerationSpec)> {
    let mut list = vec![
        ("seattle-color-10x10", town_spec(true)),
        ("seattle-plain-10x10", town_spec(false)),
        ("rainier-default-3x3", GenerationSpec::default()),
        (
            "rainier-solid-color",
            GenerationSpec {
                solid_model: true,
                color_output: ColorOutputSpec {
                    enabled: true,
                    roads_enabled: true,
                    ..ColorOutputSpec::default()
                },
                buildings: BuildingSpec {
                    enabled: true,
                    ..BuildingSpec::default()
                },
                ..GenerationSpec::default()
            },
        ),
    ];
    for structure in [BridgeStructure::Floating, BridgeStructure::Supported] {
        list.push((
            match structure {
                BridgeStructure::Floating => "portland-bridges-floating",
                BridgeStructure::Supported => "portland-bridges-supported",
            },
            GenerationSpec {
                center_lat: 45.518,
                center_lon: -122.672,
                ground_span_km: 6.0,
                rows: 4,
                columns: 4,
                place_name: "Portland".into(),
                buildings: BuildingSpec {
                    enabled: true,
                    ..BuildingSpec::default()
                },
                color_output: ColorOutputSpec {
                    enabled: true,
                    roads_enabled: true,
                    bridge_structure: structure,
                    ..ColorOutputSpec::default()
                },
                ..GenerationSpec::default()
            },
        ));
    }
    list.push((
        "rainier-tray-interlocks",
        GenerationSpec {
            tray: TraySpec {
                enabled: true,
                segment_columns: 2,
                segment_rows: 1,
                ..TraySpec::default()
            },
            ..GenerationSpec::default()
        },
    ));
    list
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let selected = env::args().skip(1).collect::<Vec<_>>();
    let cache_dir = map_cache_root()?;
    for (name, spec) in scenarios() {
        if !selected.is_empty() && !selected.iter().any(|argument| argument == name) {
            continue;
        }
        spec.validate()?;
        eprintln!("fetching data for {name}...");
        let height_field =
            fetch_height_field_with_progress(&spec, &cache_dir.join("elevation"), |_| Ok(()))?;
        let surface_field = if spec.needs_surface_field() {
            Some(fetch_surface_field(&spec, &height_field, &cache_dir)?)
        } else {
            None
        };
        eprintln!("building and analyzing {name}...");
        let reports = analyze_project(&spec, Some(&height_field), surface_field.as_ref())?;
        print!("{}", summarize(name, &reports));
        println!();
    }
    Ok(())
}
