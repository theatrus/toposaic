use std::io::{Cursor, Read};

use anyhow::{Result, ensure};

use super::detail::EarthDetail;
use super::ground::{Ground, smoothstep};
use super::layout::Vec3;

pub(super) const ELEVATION_SOURCE: &str = "NOAA NCEI ETOPO 2022 (ice surface), half-degree sample; https://www.ncei.noaa.gov/products/etopo-global-relief-model";
pub(super) const COLOR_SOURCE: &str = "Natural Earth I at 1:50 million, simplified print categories from its shaded land and water raster; https://www.naturalearthdata.com/downloads/50m-raster-data/50m-natural-earth-1/";

const WIDTH: usize = 720;
const HEIGHT: usize = 360;
const DATA: &[u8] = include_bytes!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../assets/globe/earth-half-degree.zip"
));

pub(super) struct World<'a> {
    cells: Vec<u8>,
    poles: [Ground; 2],
    relief: f32,
    detail: Option<&'a EarthDetail>,
}

impl<'a> World<'a> {
    pub(super) fn new(relief: f32, detail: Option<&'a EarthDetail>) -> Result<Self> {
        let mut archive = zip::ZipArchive::new(Cursor::new(DATA))?;
        let mut cells = Vec::with_capacity(WIDTH * HEIGHT * 3);
        archive.by_name("earth.bin")?.read_to_end(&mut cells)?;
        ensure!(
            cells.len() == WIDTH * HEIGHT * 3,
            "bundled Whole Earth data has the wrong dimensions"
        );
        let mut world = Self {
            cells,
            relief,
            detail,
            poles: [Ground::ZERO; 2],
        };
        for (index, row) in [0, HEIGHT - 1].into_iter().enumerate() {
            world.poles[index] = (0..WIDTH).fold(Ground::ZERO, |sum, x| {
                sum.plus(world.cell(x, row), 1.0 / WIDTH as f32)
            });
        }
        Ok(world)
    }

    fn cell(&self, x: usize, y: usize) -> Ground {
        let index = 3 * (y * WIDTH + x);
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
        let u = ((longitude - 0.241_666_66) * 2.0).rem_euclid(WIDTH as f32);
        let v = ((latitude + 89.758_33) * 2.0).clamp(0.0, (HEIGHT - 1) as f32);
        let (x0, y0) = (u.floor() as usize % WIDTH, v.floor() as usize);
        let (x1, y1) = ((x0 + 1) % WIDTH, (y0 + 1).min(HEIGHT - 1));
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
