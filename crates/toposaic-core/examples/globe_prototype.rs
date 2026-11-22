//! Usage: cargo run -p toposaic-core --example globe_prototype -- OUTPUT [--batch FIRST COUNT] [--rounded] [--gap MM]
//!
//! Writes the plain 20-tile shell, or a numbered batch of its tiles.

use std::env;
use std::path::PathBuf;

use anyhow::{Context, Result, bail};
use toposaic_core::globe::{GlobeBatch, GlobeExterior, GlobePrototypeSpec, export_globe_prototype};

fn main() -> Result<()> {
    let mut args = env::args_os().skip(1);
    let output = PathBuf::from(args.next().context("provide an output directory")?);
    let mut spec = GlobePrototypeSpec::default();
    while let Some(option) = args.next() {
        match option.to_str() {
            Some("--batch") => {
                let mut number = |label: &str| -> Result<usize> {
                    let value = args
                        .next()
                        .with_context(|| format!("provide the {label}"))?;
                    Ok(value.to_str().context("invalid batch")?.parse()?)
                };
                spec.batch = Some(GlobeBatch {
                    first_tile: number("first tile")?,
                    tile_count: number("tile count")?,
                });
            }
            Some("--rounded") => spec.exterior = GlobeExterior::Rounded,
            Some("--gap") => {
                let gap = args.next().context("provide a seam gap in millimeters")?;
                spec.seam_gap_mm = gap.to_str().context("invalid seam gap")?.parse()?;
            }
            _ => bail!("unrecognized option {option:?}"),
        }
    }
    export_globe_prototype(&spec, &output)?;
    println!("Wrote globe prototype to {}", output.display());
    Ok(())
}
