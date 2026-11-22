use std::collections::{BTreeMap, BTreeSet};

use anyhow::{Result, ensure};

use super::layout::{Face, Vec3, unit};
use super::{GlobePrototypeSpec, Layout};

pub const EARTH_DETAIL_BLOCK_SIDE: usize = 120;
const WIDTH: usize = 4320;
const HEIGHT: usize = 2160;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct EarthDetailBlock {
    pub column: usize,
    pub row: usize,
}

impl EarthDetailBlock {
    fn for_node(x: usize, y: usize) -> Self {
        Self {
            column: x / EARTH_DETAIL_BLOCK_SIDE,
            row: y / EARTH_DETAIL_BLOCK_SIDE,
        }
    }
}

#[derive(Default)]
pub struct EarthDetail {
    blocks: BTreeMap<EarthDetailBlock, Vec<i16>>,
}

impl EarthDetail {
    pub fn insert(&mut self, block: EarthDetailBlock, elevations: Vec<i16>) -> Result<()> {
        ensure!(
            block.column < WIDTH / EARTH_DETAIL_BLOCK_SIDE
                && block.row < HEIGHT / EARTH_DETAIL_BLOCK_SIDE,
            "invalid Earth elevation block"
        );
        ensure!(
            elevations.len() == EARTH_DETAIL_BLOCK_SIDE * EARTH_DETAIL_BLOCK_SIDE,
            "Earth elevation block is incomplete"
        );
        self.blocks.insert(block, elevations);
        Ok(())
    }

    pub(super) fn elevation(&self, direction: Vec3) -> Option<f32> {
        let (nodes, [u, v]) = coordinates(direction)?;
        let mut elevations = [0.0; 4];
        for (value, [x, y]) in elevations.iter_mut().zip(nodes) {
            let block = self.blocks.get(&EarthDetailBlock::for_node(x, y))?;
            let i = (y % EARTH_DETAIL_BLOCK_SIDE) * EARTH_DETAIL_BLOCK_SIDE
                + x % EARTH_DETAIL_BLOCK_SIDE;
            *value = f32::from(block[i]);
        }
        let [sw, se, nw, ne] = elevations;
        Some((sw * (1.0 - u) + se * u) * (1.0 - v) + (nw * (1.0 - u) + ne * u) * v)
    }
}

fn coordinates([x, y, z]: Vec3) -> Option<([[usize; 2]; 4], [f32; 2])> {
    let longitude = y.atan2(x).to_degrees();
    let latitude = z.clamp(-1.0, 1.0).asin().to_degrees();
    // Use the bundled polar cap at both poles, feathering into it in World.
    if latitude.abs() >= 89.75 {
        return None;
    }
    let u = (longitude * 12.0 - 0.5).rem_euclid(WIDTH as f32);
    let v = ((latitude + 90.0) * 12.0 - 0.5).clamp(0.0, (HEIGHT - 1) as f32);
    let (x0, y0) = (u.floor() as usize % WIDTH, v.floor() as usize);
    let (x1, y1) = ((x0 + 1) % WIDTH, (y0 + 1).min(HEIGHT - 1));
    Some((
        [[x0, y0], [x1, y0], [x0, y1], [x1, y1]],
        [u.fract(), v.fract()],
    ))
}

pub fn earth_detail_blocks(spec: &GlobePrototypeSpec) -> Result<Vec<EarthDetailBlock>> {
    let layout = Layout::new(spec)?;
    let mut blocks = BTreeSet::new();
    for index in layout.selected {
        let face = &layout.faces[index];
        request_face(face, usize::from(spec.samples_per_edge), &mut blocks);
        // Draft preview and final export can use different sample lattices.
        request_face(
            face,
            usize::from(spec.samples_per_edge.min(32)),
            &mut blocks,
        );
    }
    ensure!(
        blocks.len() <= 96,
        "this batch needs {} elevation blocks; reduce the number of tiles in the batch",
        blocks.len()
    );
    Ok(blocks.into_iter().collect())
}

fn request_face(face: &Face, segments: usize, blocks: &mut BTreeSet<EarthDetailBlock>) {
    let corners = face.outline_at(0.0, 0.0);
    for i in 0..=segments {
        for j in 0..=segments - i {
            let u = i as f32 / segments as f32;
            let v = j as f32 / segments as f32;
            let [x, y] = std::array::from_fn(|axis| {
                corners[0][axis] * (1.0 - u - v) + corners[1][axis] * u + corners[2][axis] * v
            });
            if let Some((nodes, _)) = coordinates(unit(face.pose.to_globe([x, y, 0.0]))) {
                blocks.extend(nodes.map(|[x, y]| EarthDetailBlock::for_node(x, y)));
            }
        }
    }
}
