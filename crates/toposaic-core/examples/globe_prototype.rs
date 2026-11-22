//! Usage: cargo run -p toposaic-core --example globe_prototype -- OUTPUT [--full] [--rounded] [--gap MM]

use std::env;
use std::path::PathBuf;

use anyhow::{Context, Result, bail};
use toposaic_core::globe::{GlobeExterior, GlobePrototypeSpec, export_globe_prototype};

fn main() -> Result<()> {
    let mut args = env::args_os().skip(1);
    let output = PathBuf::from(args.next().context("provide an output directory")?);
    let mut spec = GlobePrototypeSpec::default();
    while let Some(option) = args.next() {
        match option.to_str() {
            Some("--full") => spec.full_shell = true,
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
