//! Repeats a real saved setup against the normal map cache.
//!
//! The first run warms source and prepared-geometry caches. The reported
//! value is the median of the measured release runs, not the warm-up.

use std::{env, fs, path::PathBuf, time::Instant};

use toposaic_api::diagnostics::{
    apply_marine_water, fetch_height_field_with_progress, fetch_surface_field, map_cache_root,
};
use toposaic_core::{GenerationSpec, generate_project_with_fields};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "toposaic_core::geometry=info,toposaic_api=info".into()),
        )
        .try_init()
        .ok();
    let fixture = env::args()
        .nth(1)
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("benchmarks/slow-tacoma.json"));
    let measured_runs = env::args()
        .nth(2)
        .map(|value| value.parse::<usize>())
        .transpose()?
        .unwrap_or(3)
        .max(1);
    let spec: GenerationSpec = serde_json::from_slice(&fs::read(&fixture)?)?;
    spec.validate()?;
    let cache = map_cache_root()?;
    let root = env::temp_dir().join(format!("toposaic-real-benchmark-{}", std::process::id()));
    if root.exists() {
        fs::remove_dir_all(&root)?;
    }
    fs::create_dir_all(&root)?;

    let mut elapsed = Vec::with_capacity(measured_runs);
    for run in 0..=measured_runs {
        let output = root.join(format!("run-{run}"));
        let started = Instant::now();
        let mut height =
            fetch_height_field_with_progress(&spec, &cache.join("elevation"), |_| Ok(()))?;
        let mut surface = fetch_surface_field(&spec, &height, &cache)?;
        apply_marine_water(&spec, &mut height, &mut surface, &cache);
        generate_project_with_fields(&spec, &height, Some(&surface), &output)?;
        let duration = started.elapsed();
        println!(
            "{} run {run}: {:.3}s",
            if run == 0 { "warm-up" } else { "measured" },
            duration.as_secs_f64()
        );
        fs::remove_dir_all(&output)?;
        if run > 0 {
            elapsed.push(duration);
        }
    }
    elapsed.sort_unstable();
    let median = elapsed[elapsed.len() / 2];
    println!(
        "median of {measured_runs} measured runs: {:.3}s",
        median.as_secs_f64()
    );
    fs::remove_dir_all(root)?;
    Ok(())
}
