use std::io::{Cursor, Read};

use anyhow::{Result, ensure};

use super::EarthGrid;
use super::detail::EarthDetail;
use super::ground::{Ground, smoothstep};
use super::layout::Vec3;

pub(super) fn elevation_source(grid: EarthGrid) -> String {
    let step = match grid {
        EarthGrid::HalfDegree => "half-degree",
        EarthGrid::QuarterDegree => "quarter-degree",
    };
    format!(
        "NOAA NCEI ETOPO 2022 (ice surface), {step} sample; https://www.ncei.noaa.gov/products/etopo-global-relief-model"
    )
}
pub(super) const COLOR_SOURCE: &str = "Natural Earth I at 1:50 million, simplified print categories from its shaded land and water raster; https://www.naturalearthdata.com/downloads/50m-raster-data/50m-natural-earth-1/";

const HALF_DEGREE: &[u8] = include_bytes!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../assets/globe/earth-half-degree.zip"
));
const QUARTER_DEGREE: &[u8] = include_bytes!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../assets/globe/earth-quarter-degree.zip"
));

pub(super) struct World<'a> {
    width: usize,
    height: usize,
    west_degrees: f32,
    south_degrees: f32,
    cells: Vec<u8>,
    poles: [Ground; 2],
    relief: f32,
    detail: Option<&'a EarthDetail>,
}

impl<'a> World<'a> {
    pub(super) fn new(
        grid: EarthGrid,
        relief: f32,
        detail: Option<&'a EarthDetail>,
    ) -> Result<Self> {
        let (width, height, west_degrees, south_degrees, data) = match grid {
            EarthGrid::HalfDegree => (720, 360, 0.241_666_66, -89.758_33, HALF_DEGREE),
            EarthGrid::QuarterDegree => (1440, 720, 0.125, -89.875, QUARTER_DEGREE),
        };
        let mut archive = zip::ZipArchive::new(Cursor::new(data))?;
        let mut cells = Vec::with_capacity(width * height * 3);
        archive.by_name("earth.bin")?.read_to_end(&mut cells)?;
        ensure!(
            cells.len() == width * height * 3,
            "bundled Whole Earth data has the wrong dimensions"
        );
        let mut world = Self {
            width,
            height,
            west_degrees,
            south_degrees,
            cells,
            relief,
            detail,
            poles: [Ground::ZERO; 2],
        };
        for (index, row) in [0, height - 1].into_iter().enumerate() {
            world.poles[index] = (0..width).fold(Ground::ZERO, |sum, x| {
                sum.plus(world.cell(x, row), 1.0 / width as f32)
            });
        }
        Ok(world)
    }

    fn cell(&self, x: usize, y: usize) -> Ground {
        let index = 3 * (y * self.width + x);
        let class = usize::from(self.cells[index + 2]);
        let elevation = i16::from_le_bytes([self.cells[index], self.cells[index + 1]]);
        let height = if class == 3 {
            0.0
        } else {
            (f32::from(elevation) / 8_848.0).clamp(0.0, 1.0) * self.relief
        };
        Ground::solid(height, class.min(4))
    }

    pub(super) fn sample(&self, direction: Vec3) -> Ground {
        let [x, y, z] = direction;
        let longitude = y.atan2(x).to_degrees();
        let latitude = z.clamp(-1.0, 1.0).asin().to_degrees();
        let cells_per_degree = self.width as f32 / 360.0;
        let u = ((longitude - self.west_degrees) * cells_per_degree).rem_euclid(self.width as f32);
        let v = ((latitude - self.south_degrees) * cells_per_degree)
            .clamp(0.0, (self.height - 1) as f32);
        let (x0, y0) = (u.floor() as usize % self.width, v.floor() as usize);
        let (x1, y1) = ((x0 + 1) % self.width, (y0 + 1).min(self.height - 1));
        let south = self.cell(x0, y0).blend(self.cell(x1, y0), u.fract());
        let north = self.cell(x0, y1).blend(self.cell(x1, y1), u.fract());
        let sample = south.blend(north, v.fract());
        let polar_weight = ((latitude.abs() - 89.5) * 2.0).clamp(0.0, 1.0);
        let mut sample = sample.blend(self.poles[usize::from(latitude >= 0.0)], polar_weight);
        if let Some(elevation) = self.detail.and_then(|detail| detail.elevation(direction)) {
            let blend = 1.0 - smoothstep((latitude.abs() - 89.0) / 0.75);
            let high_detail = if sample.class() == crate::SurfaceClass::Water {
                0.0
            } else {
                (elevation / 8_848.0).clamp(0.0, 1.0) * self.relief
            };
            sample.height += (high_detail - sample.height) * blend;
        }
        sample
    }
}
