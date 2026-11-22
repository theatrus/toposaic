use std::io::{BufRead, BufReader, Read};
use std::path::Path;
use std::time::Duration;

use anyhow::{Context, Result, ensure};
use toposaic_core::globe::{
    EARTH_DETAIL_BLOCK_SIDE, EarthDetail, EarthDetailBlock, GlobePrototypeSpec, earth_detail_blocks,
};

use crate::{cache, http};

const BASE_URL: &str = "https://oceanwatch.pifsc.noaa.gov/erddap/griddap/ETOPO_2022_v1_60s.csv";
const BLOCK_BYTES: usize = 2 * EARTH_DETAIL_BLOCK_SIDE * EARTH_DETAIL_BLOCK_SIDE;

pub(crate) fn fetch_detail(
    spec: &GlobePrototypeSpec,
    map_cache_dir: &Path,
    mut on_progress: impl FnMut(f32) -> Result<()>,
) -> Result<EarthDetail> {
    let blocks = earth_detail_blocks(spec)?;
    let client = http::blocking_client(Duration::from_secs(50))?;
    let mut detail = EarthDetail::default();
    for (index, &block) in blocks.iter().enumerate() {
        on_progress(index as f32 / blocks.len() as f32)?;
        let path = map_cache_dir
            .join("elevation")
            .join("etopo-2022-5m")
            .join(format!("{}-{}.i16le", block.column, block.row));
        let elevations = if let Ok(bytes) = cache::read(&path)
            && bytes.len() == BLOCK_BYTES
        {
            bytes
                .chunks_exact(2)
                .map(|chunk| i16::from_le_bytes([chunk[0], chunk[1]]))
                .collect()
        } else {
            let index = |block_index: usize| {
                let start = 2 + block_index * EARTH_DETAIL_BLOCK_SIDE * 5;
                (start, start + (EARTH_DETAIL_BLOCK_SIDE - 1) * 5)
            };
            let (x0, x1) = index(block.column);
            let (y0, y1) = index(block.row);
            let url = format!("{BASE_URL}?z%5B{y0}:5:{y1}%5D%5B{x0}:5:{x1}%5D");
            let response = client
                .get(url)
                .send()?
                .error_for_status()
                .with_context(|| {
                    format!(
                        "download NOAA elevation block {},{}",
                        block.column, block.row
                    )
                })?;
            let elevations = read_block(BufReader::new(response.take(2_000_000)), block)?;
            let bytes = elevations
                .iter()
                .flat_map(|value| value.to_le_bytes())
                .collect::<Vec<_>>();
            cache::store(&path, &bytes)?;
            elevations
        };
        detail.insert(block, elevations)?;
    }
    on_progress(1.0)?;
    Ok(detail)
}

fn read_block(reader: impl BufRead, block: EarthDetailBlock) -> Result<Vec<i16>> {
    let mut lines = reader.lines();
    ensure!(
        lines.next().transpose()?.as_deref() == Some("latitude,longitude,z"),
        "unexpected NOAA elevation columns"
    );
    lines
        .next()
        .transpose()?
        .context("missing NOAA elevation units")?;
    let mut elevations = Vec::with_capacity(BLOCK_BYTES / 2);
    for (i, line) in lines.enumerate() {
        ensure!(
            i < BLOCK_BYTES / 2,
            "NOAA elevation block has excess samples"
        );
        let line = line?;
        let mut numbers = line.split(',').map(str::parse::<f64>);
        let latitude = numbers
            .next()
            .context("NOAA elevation missing latitude")??;
        let longitude = numbers
            .next()
            .context("NOAA elevation missing longitude")??;
        let elevation = numbers.next().context("NOAA elevation missing value")??;
        let expected_lat = -90.0
            + (block.row * EARTH_DETAIL_BLOCK_SIDE + i / EARTH_DETAIL_BLOCK_SIDE) as f64 / 12.0
            + 1.0 / 24.0;
        let expected_lon =
            (block.column * EARTH_DETAIL_BLOCK_SIDE + i % EARTH_DETAIL_BLOCK_SIDE) as f64 / 12.0
                + 1.0 / 24.0;
        ensure!(
            (latitude - expected_lat).abs() < 1.0e-5
                && (longitude - expected_lon).abs() < 1.0e-5
                && (-12_000.0..=10_000.0).contains(&elevation),
            "invalid NOAA elevation sample in block {},{}",
            block.column,
            block.row
        );
        elevations.push(elevation.round() as i16);
    }
    ensure!(
        elevations.len() == BLOCK_BYTES / 2,
        "NOAA elevation block is incomplete"
    );
    Ok(elevations)
}
