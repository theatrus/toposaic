use std::collections::VecDeque;

use anyhow::{Result, bail};
use rayon::prelude::*;

use crate::mesh::{distance_squared, point_in_polygon};
use crate::spec::SurfaceClass;

const VECTOR_BUCKET_COLUMNS: usize = 32;
const VECTOR_BUCKET_COUNT: usize = VECTOR_BUCKET_COLUMNS * VECTOR_BUCKET_COLUMNS;
pub(crate) const ROAD_VECTOR_STEP_MM: f32 = 0.25;

#[derive(Debug, Clone)]
pub struct SurfaceField {
    pub width: usize,
    pub height: usize,
    pub classes: Vec<SurfaceClass>,
    pub source: String,
    pub(crate) base_classes: Vec<SurfaceClass>,
    pub(crate) vector_lines: Vec<VectorSurfaceLine>,
    pub(crate) vector_areas: Vec<VectorSurfaceArea>,
    vector_line_buckets: Vec<Vec<usize>>,
    vector_area_buckets: Vec<Vec<usize>>,
}

#[derive(Debug, Clone)]
pub(crate) struct VectorSurfaceLine {
    pub(crate) points_mm: Vec<[f32; 2]>,
    pub(crate) width_mm: f32,
    model_width_mm: f32,
    model_height_mm: f32,
    pub(crate) class: SurfaceClass,
    pub(crate) bridge_elevations_m: Option<[f32; 2]>,
    length_mm: f32,
}

#[derive(Debug, Clone)]
pub(crate) struct VectorSurfaceArea {
    pub(crate) points: Vec<[f32; 2]>,
    class: Option<SurfaceClass>,
    pub(crate) building_height_m: f32,
}

#[derive(Clone, Copy)]
pub(crate) struct SurfaceSample {
    pub(crate) class: SurfaceClass,
    pub(crate) building_height_m: f32,
}

impl SurfaceField {
    pub fn new(
        width: usize,
        height: usize,
        classes: Vec<SurfaceClass>,
        source: impl Into<String>,
    ) -> Result<Self> {
        if width < 2 || height < 2 {
            bail!("surface field must be at least 2 by 2");
        }
        if classes.len() != width * height {
            bail!("surface field dimensions do not match its values");
        }
        Ok(Self {
            width,
            height,
            base_classes: classes.clone(),
            classes,
            source: source.into(),
            vector_lines: Vec::new(),
            vector_areas: Vec::new(),
            vector_line_buckets: vec![Vec::new(); VECTOR_BUCKET_COUNT],
            vector_area_buckets: vec![Vec::new(); VECTOR_BUCKET_COUNT],
        })
    }

    pub fn filter_small_patches(&mut self, print_width_mm: f32, minimum_patch_mm: f32) {
        let cells_across =
            minimum_patch_mm / print_width_mm.max(f32::EPSILON) * (self.width - 1) as f32;
        let minimum_cells = (std::f32::consts::PI * (cells_across * 0.5).powi(2))
            .ceil()
            .max(2.0) as usize;
        for _ in 0..2 {
            self.filter_components_smaller_than(minimum_cells);
        }
        self.base_classes.clone_from(&self.classes);
    }

    pub fn paint_polyline(
        &mut self,
        points: &[[f32; 2]],
        print_width_mm: f32,
        line_width_mm: f32,
        class: SurfaceClass,
    ) {
        self.paint_polyline_with_bridge(points, print_width_mm, line_width_mm, class, None);
    }

    pub fn paint_bridge_polyline(
        &mut self,
        points: &[[f32; 2]],
        print_width_mm: f32,
        line_width_mm: f32,
        elevations_m: [f32; 2],
    ) {
        if elevations_m.iter().all(|value| value.is_finite()) {
            self.paint_polyline_with_bridge(
                points,
                print_width_mm,
                line_width_mm,
                SurfaceClass::Road,
                Some(elevations_m),
            );
        }
    }

    fn paint_polyline_with_bridge(
        &mut self,
        points: &[[f32; 2]],
        print_width_mm: f32,
        line_width_mm: f32,
        class: SurfaceClass,
        bridge_elevations_m: Option<[f32; 2]>,
    ) {
        if points.len() < 2 {
            return;
        }
        let print_height_mm = print_width_mm * (self.height - 1) as f32 / (self.width - 1) as f32;
        let smooth_points = resample_surface_line(
            &smooth_surface_line(
                &points
                    .iter()
                    .map(|point| [point[0] * print_width_mm, point[1] * print_height_mm])
                    .collect::<Vec<_>>(),
            ),
            ROAD_VECTOR_STEP_MM,
        );
        let length_mm = smooth_points
            .windows(2)
            .map(|segment| (segment[1][0] - segment[0][0]).hypot(segment[1][1] - segment[0][1]))
            .sum();
        let line = VectorSurfaceLine {
            points_mm: smooth_points.clone(),
            width_mm: line_width_mm,
            model_width_mm: print_width_mm,
            model_height_mm: print_height_mm,
            class,
            bridge_elevations_m,
            length_mm,
        };
        let half_width = line_width_mm * 0.5;
        let bounds = line.points_mm.iter().fold(
            [
                f32::INFINITY,
                f32::INFINITY,
                f32::NEG_INFINITY,
                f32::NEG_INFINITY,
            ],
            |bounds, point| {
                [
                    bounds[0].min((point[0] - half_width) / print_width_mm),
                    bounds[1].min((point[1] - half_width) / print_height_mm),
                    bounds[2].max((point[0] + half_width) / print_width_mm),
                    bounds[3].max((point[1] + half_width) / print_height_mm),
                ]
            },
        );
        let line_index = self.vector_lines.len();
        self.vector_lines.push(line);
        add_to_vector_buckets(&mut self.vector_line_buckets, bounds, line_index);
    }

    pub fn paint_building(&mut self, points: &[[f32; 2]], height_m: f32) {
        if points.len() < 3 || !height_m.is_finite() || height_m <= 0.0 {
            return;
        }
        let area = VectorSurfaceArea {
            points: points.to_vec(),
            class: None,
            building_height_m: height_m,
        };
        let area_index = self.vector_areas.len();
        add_to_vector_buckets(
            &mut self.vector_area_buckets,
            surface_area_bounds(&area.points),
            area_index,
        );
        self.vector_areas.push(area);
    }

    pub fn paint_surface_area(&mut self, points: &[[f32; 2]], class: SurfaceClass) {
        if points.len() < 3 {
            return;
        }
        let area = VectorSurfaceArea {
            points: points.to_vec(),
            class: Some(class),
            building_height_m: 0.0,
        };
        let area_index = self.vector_areas.len();
        add_to_vector_buckets(
            &mut self.vector_area_buckets,
            surface_area_bounds(&area.points),
            area_index,
        );
        self.vector_areas.push(area);
        self.rasterize_area(points, class);
    }

    fn rasterize_area(&mut self, points: &[[f32; 2]], class: SurfaceClass) {
        let pixels = points
            .iter()
            .map(|point| {
                [
                    point[0] * (self.width - 1) as f32,
                    point[1] * (self.height - 1) as f32,
                ]
            })
            .collect::<Vec<_>>();
        let polygon_min_x = pixels
            .iter()
            .map(|point| point[0])
            .fold(f32::INFINITY, f32::min);
        let polygon_max_x = pixels
            .iter()
            .map(|point| point[0])
            .fold(f32::NEG_INFINITY, f32::max);
        let polygon_min_y = pixels
            .iter()
            .map(|point| point[1])
            .fold(f32::INFINITY, f32::min);
        let polygon_max_y = pixels
            .iter()
            .map(|point| point[1])
            .fold(f32::NEG_INFINITY, f32::max);
        if polygon_max_x < 0.0
            || polygon_min_x > (self.width - 1) as f32
            || polygon_max_y < 0.0
            || polygon_min_y > (self.height - 1) as f32
        {
            return;
        }
        let min_x = polygon_min_x.floor().max(0.0) as usize;
        let max_x = polygon_max_x.ceil().min((self.width - 1) as f32) as usize;
        let min_y = polygon_min_y.floor().max(0.0) as usize;
        let max_y = polygon_max_y.ceil().min((self.height - 1) as f32) as usize;
        for y in min_y..=max_y {
            for x in min_x..=max_x {
                if point_in_polygon([x as f32, y as f32], &pixels) {
                    self.classes[y * self.width + x] = class;
                }
            }
        }
    }

    fn filter_components_smaller_than(&mut self, minimum_cells: usize) {
        let original = self.classes.clone();
        let mut visited = vec![false; original.len()];
        for start in 0..original.len() {
            if visited[start] {
                continue;
            }
            let class = original[start];
            let mut queue = VecDeque::from([start]);
            let mut component = Vec::new();
            let mut neighbours = [0_usize; 6];
            visited[start] = true;
            while let Some(index) = queue.pop_front() {
                component.push(index);
                let x = index % self.width;
                let y = index / self.width;
                for neighbour in [
                    x.checked_sub(1).map(|value| y * self.width + value),
                    (x + 1 < self.width).then_some(y * self.width + x + 1),
                    y.checked_sub(1).map(|value| value * self.width + x),
                    (y + 1 < self.height).then_some((y + 1) * self.width + x),
                ]
                .into_iter()
                .flatten()
                {
                    let neighbour_class = original[neighbour];
                    if neighbour_class == class {
                        if !visited[neighbour] {
                            visited[neighbour] = true;
                            queue.push_back(neighbour);
                        }
                    } else {
                        neighbours[neighbour_class.material_index() as usize] += 1;
                    }
                }
            }
            if component.len() < minimum_cells {
                let replacement = neighbours
                    .into_iter()
                    .enumerate()
                    .max_by_key(|(index, count)| (*count, usize::MAX - *index))
                    .map(|(index, _)| match index {
                        1 => SurfaceClass::Forest,
                        2 => SurfaceClass::Snow,
                        3 => SurfaceClass::Water,
                        4 => SurfaceClass::Road,
                        5 => SurfaceClass::Building,
                        _ => SurfaceClass::Rock,
                    })
                    .unwrap_or(SurfaceClass::Rock);
                for index in component {
                    self.classes[index] = replacement;
                }
            }
        }
    }

    fn at(&self, u: f32, v: f32) -> SurfaceClass {
        self.sample(u, v).class
    }

    pub(crate) fn terrain_at(&self, u: f32, v: f32) -> SurfaceClass {
        self.terrain_sample(u, v).class
    }

    pub(crate) fn sample(&self, u: f32, v: f32) -> SurfaceSample {
        self.sample_with_overlays(u, v, true, true)
    }

    fn terrain_sample(&self, u: f32, v: f32) -> SurfaceSample {
        self.sample_with_overlays(u, v, false, false)
    }

    fn interpolated_base_class(&self, u: f32, v: f32) -> SurfaceClass {
        let x = u.clamp(0.0, 1.0) * (self.width - 1) as f32;
        let y = v.clamp(0.0, 1.0) * (self.height - 1) as f32;
        let x0 = x.floor() as usize;
        let y0 = y.floor() as usize;
        let x1 = (x0 + 1).min(self.width - 1);
        let y1 = (y0 + 1).min(self.height - 1);
        let tx = x - x0 as f32;
        let ty = y - y0 as f32;
        let corners = [
            (
                self.base_classes[y0 * self.width + x0],
                (1.0 - tx) * (1.0 - ty),
            ),
            (self.base_classes[y0 * self.width + x1], tx * (1.0 - ty)),
            (self.base_classes[y1 * self.width + x1], tx * ty),
            (self.base_classes[y1 * self.width + x0], (1.0 - tx) * ty),
        ];
        [
            SurfaceClass::Rock,
            SurfaceClass::Forest,
            SurfaceClass::Snow,
            SurfaceClass::Water,
            SurfaceClass::Road,
            SurfaceClass::Building,
        ]
        .into_iter()
        .map(|class| {
            let weight = corners
                .iter()
                .filter(|(corner_class, _)| *corner_class == class)
                .map(|(_, weight)| weight)
                .sum::<f32>();
            (class, weight)
        })
        .max_by(|first, second| first.1.total_cmp(&second.1))
        .map(|(class, _)| class)
        .unwrap_or(SurfaceClass::Rock)
    }

    fn sample_with_overlays(
        &self,
        u: f32,
        v: f32,
        include_roads: bool,
        include_buildings: bool,
    ) -> SurfaceSample {
        let bucket = vector_bucket_index(u, v);
        let building_height_m = self.building_height_at_in_bucket(u, v, bucket);
        if include_buildings && building_height_m > 0.0 {
            return SurfaceSample {
                class: SurfaceClass::Building,
                building_height_m,
            };
        }
        let line_indices = &self.vector_line_buckets[bucket];
        let has_road = include_roads
            && line_indices.iter().any(|index| {
                let line = &self.vector_lines[*index];
                line.class == SurfaceClass::Road && surface_line_contains(line, u, v)
            });
        if has_road {
            return SurfaceSample {
                class: SurfaceClass::Road,
                building_height_m,
            };
        }
        if let Some(class) = self.vector_area_buckets[bucket]
            .iter()
            .rev()
            .filter_map(|index| {
                let area = &self.vector_areas[*index];
                area.class.map(|class| (area, class))
            })
            .find(|(area, _)| point_in_polygon([u, v], &area.points))
            .map(|(_, class)| class)
        {
            return SurfaceSample {
                class,
                building_height_m,
            };
        }
        if let Some(class) = self.vector_line_buckets[bucket]
            .iter()
            .rev()
            .map(|index| &self.vector_lines[*index])
            .filter(|line| line.class != SurfaceClass::Road)
            .find(|line| surface_line_contains(line, u, v))
            .map(|line| line.class)
        {
            return SurfaceSample {
                class,
                building_height_m,
            };
        }
        SurfaceSample {
            class: self.interpolated_base_class(u, v),
            building_height_m,
        }
    }

    #[cfg(test)]
    fn building_height_at(&self, u: f32, v: f32) -> f32 {
        self.building_height_at_in_bucket(u, v, vector_bucket_index(u, v))
    }

    fn building_height_at_in_bucket(&self, u: f32, v: f32, bucket: usize) -> f32 {
        self.vector_area_buckets[bucket]
            .iter()
            .map(|index| &self.vector_areas[*index])
            .filter(|area| area.building_height_m > 0.0)
            .filter(|area| point_in_polygon([u, v], &area.points))
            .map(|area| area.building_height_m)
            .fold(0.0, f32::max)
    }

    pub(crate) fn coverage(&self) -> [f32; 6] {
        let counts = (0..self.classes.len())
            .into_par_iter()
            .fold(
                || [0_usize; 6],
                |mut counts, index| {
                    let x = index % self.width;
                    let y = index / self.width;
                    let u = x as f32 / (self.width - 1) as f32;
                    let v = y as f32 / (self.height - 1) as f32;
                    counts[self.at(u, v).material_index() as usize] += 1;
                    counts
                },
            )
            .reduce(
                || [0_usize; 6],
                |mut total, counts| {
                    for (total, count) in total.iter_mut().zip(counts) {
                        *total += count;
                    }
                    total
                },
            );
        let total = self.classes.len() as f32;
        counts.map(|count| count as f32 * 100.0 / total)
    }
}

pub(crate) fn surface_area_bounds(points: &[[f32; 2]]) -> [f32; 4] {
    points.iter().fold(
        [
            f32::INFINITY,
            f32::INFINITY,
            f32::NEG_INFINITY,
            f32::NEG_INFINITY,
        ],
        |bounds, point| {
            [
                bounds[0].min(point[0]),
                bounds[1].min(point[1]),
                bounds[2].max(point[0]),
                bounds[3].max(point[1]),
            ]
        },
    )
}

fn vector_bucket_coordinate(value: f32) -> usize {
    (value.clamp(0.0, 0.999_999) * VECTOR_BUCKET_COLUMNS as f32) as usize
}

fn vector_bucket_index(u: f32, v: f32) -> usize {
    vector_bucket_coordinate(v) * VECTOR_BUCKET_COLUMNS + vector_bucket_coordinate(u)
}

fn add_to_vector_buckets(buckets: &mut [Vec<usize>], bounds: [f32; 4], feature_index: usize) {
    let minimum_x = vector_bucket_coordinate(bounds[0]);
    let minimum_y = vector_bucket_coordinate(bounds[1]);
    let maximum_x = vector_bucket_coordinate(bounds[2]);
    let maximum_y = vector_bucket_coordinate(bounds[3]);
    for y in minimum_y..=maximum_y {
        for x in minimum_x..=maximum_x {
            buckets[y * VECTOR_BUCKET_COLUMNS + x].push(feature_index);
        }
    }
}

fn smooth_surface_line(points: &[[f32; 2]]) -> Vec<[f32; 2]> {
    if points.len() < 3 {
        return points.to_vec();
    }
    let mut result = Vec::with_capacity((points.len() - 1) * 4 + 1);
    for index in 0..points.len() - 1 {
        let controls = [
            points[index.saturating_sub(1)],
            points[index],
            points[index + 1],
            points[(index + 2).min(points.len() - 1)],
        ];
        for sample in 0..4 {
            let t = sample as f32 / 4.0;
            let t2 = t * t;
            let t3 = t2 * t;
            result.push([
                0.5 * (2.0 * controls[1][0]
                    + (-controls[0][0] + controls[2][0]) * t
                    + (2.0 * controls[0][0] - 5.0 * controls[1][0] + 4.0 * controls[2][0]
                        - controls[3][0])
                        * t2
                    + (-controls[0][0] + 3.0 * controls[1][0] - 3.0 * controls[2][0]
                        + controls[3][0])
                        * t3),
                0.5 * (2.0 * controls[1][1]
                    + (-controls[0][1] + controls[2][1]) * t
                    + (2.0 * controls[0][1] - 5.0 * controls[1][1] + 4.0 * controls[2][1]
                        - controls[3][1])
                        * t2
                    + (-controls[0][1] + 3.0 * controls[1][1] - 3.0 * controls[2][1]
                        + controls[3][1])
                        * t3),
            ]);
        }
    }
    result.push(*points.last().unwrap());
    result
}

fn resample_surface_line(points: &[[f32; 2]], maximum_step_mm: f32) -> Vec<[f32; 2]> {
    if points.len() < 2 {
        return points.to_vec();
    }
    let mut result = Vec::new();
    for segment in points.windows(2) {
        let delta = [segment[1][0] - segment[0][0], segment[1][1] - segment[0][1]];
        let length = delta[0].hypot(delta[1]);
        let samples = (length / maximum_step_mm.max(0.01)).ceil().max(1.0) as usize;
        for sample in 0..samples {
            let t = sample as f32 / samples as f32;
            let point = [segment[0][0] + delta[0] * t, segment[0][1] + delta[1] * t];
            if result
                .last()
                .is_none_or(|previous| distance_squared(*previous, point) > 0.000_001)
            {
                result.push(point);
            }
        }
    }
    result.push(*points.last().unwrap());
    result
}

fn surface_line_projection(line: &VectorSurfaceLine, u: f32, v: f32) -> Option<(f32, f32)> {
    let radius_squared = (line.width_mm * 0.5).powi(2);
    let nearest = surface_line_nearest_projection(line, u, v);
    (nearest.0 <= radius_squared).then_some(nearest)
}

pub(crate) fn surface_line_progress(line: &VectorSurfaceLine, u: f32, v: f32) -> f32 {
    surface_line_nearest_projection(line, u, v).1
}

fn surface_line_nearest_projection(line: &VectorSurfaceLine, u: f32, v: f32) -> (f32, f32) {
    let point = [
        u.clamp(0.0, 1.0) * line.model_width_mm,
        v.clamp(0.0, 1.0) * line.model_height_mm,
    ];
    let mut traversed_mm = 0.0;
    let mut closest = (f32::INFINITY, 0.0);
    for segment in line.points_mm.windows(2) {
        let delta = [segment[1][0] - segment[0][0], segment[1][1] - segment[0][1]];
        let length_squared = delta[0].powi(2) + delta[1].powi(2);
        let length = length_squared.sqrt();
        let offset = [point[0] - segment[0][0], point[1] - segment[0][1]];
        let amount = if length_squared <= f32::EPSILON {
            0.0
        } else {
            ((offset[0] * delta[0] + offset[1] * delta[1]) / length_squared).clamp(0.0, 1.0)
        };
        let nearest = [
            segment[0][0] + delta[0] * amount,
            segment[0][1] + delta[1] * amount,
        ];
        let distance = distance_squared(point, nearest);
        if distance < closest.0 {
            closest = (
                distance,
                (traversed_mm + length * amount) / line.length_mm.max(f32::EPSILON),
            );
        }
        traversed_mm += length;
    }
    closest
}

fn surface_line_contains(line: &VectorSurfaceLine, u: f32, v: f32) -> bool {
    surface_line_projection(line, u, v).is_some()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::piece::scaled_building_height_mm;
    use crate::spec::{BuildingSpec, GenerationSpec};

    #[test]
    fn surface_filter_removes_tiny_color_islands() {
        let mut classes = vec![SurfaceClass::Forest; 25];
        classes[12] = SurfaceClass::Snow;
        let mut field = SurfaceField::new(5, 5, classes, "test").unwrap();
        field.filter_small_patches(10.0, 4.0);
        assert_eq!(field.classes[12], SurfaceClass::Forest);
    }

    #[test]
    fn base_surface_classes_use_interpolated_boundaries() {
        let field = SurfaceField::new(
            2,
            2,
            vec![
                SurfaceClass::Forest,
                SurfaceClass::Rock,
                SurfaceClass::Forest,
                SurfaceClass::Forest,
            ],
            "test",
        )
        .unwrap();

        assert_eq!(field.interpolated_base_class(0.75, 0.1), SurfaceClass::Rock);
        assert_eq!(
            field.interpolated_base_class(0.75, 0.9),
            SurfaceClass::Forest
        );
    }

    #[test]
    fn surface_field_paints_print_width_aware_road_lines() {
        let mut field =
            SurfaceField::new(21, 21, vec![SurfaceClass::Forest; 21 * 21], "test").unwrap();
        field.paint_polyline(&[[0.0, 0.5], [1.0, 0.5]], 20.0, 2.0, SurfaceClass::Road);

        assert_eq!(field.at(0.5, 0.5), SurfaceClass::Road);
        assert_eq!(field.at(0.5, 0.53), SurfaceClass::Road);
        assert_eq!(field.at(0.5, 0.35), SurfaceClass::Forest);
    }

    #[test]
    fn vector_lines_are_smooth_and_independent_of_raster_cells() {
        let mut field =
            SurfaceField::new(11, 11, vec![SurfaceClass::Forest; 11 * 11], "test").unwrap();
        field.paint_polyline(
            &[[0.0, 0.2], [0.5, 0.8], [1.0, 0.2]],
            20.0,
            0.4,
            SurfaceClass::Road,
        );
        assert!(field.vector_lines[0].points_mm.len() > 3);
        assert_eq!(field.at(0.5, 0.8), SurfaceClass::Road);
        assert_eq!(field.at(0.5, 0.84), SurfaceClass::Forest);

        field.paint_polyline(
            &[[0.0, 0.1], [0.5, 0.15], [1.0, 0.1]],
            20.0,
            0.6,
            SurfaceClass::Water,
        );
        assert_eq!(field.at(0.5, 0.15), SurfaceClass::Water);
    }

    #[test]
    fn vector_water_areas_keep_their_exact_boundary() {
        let mut field = SurfaceField::new(5, 5, vec![SurfaceClass::Rock; 25], "test").unwrap();
        field.paint_surface_area(
            &[[0.45, 0.45], [0.55, 0.45], [0.55, 0.55], [0.45, 0.55]],
            SurfaceClass::Water,
        );
        assert_eq!(field.at(0.5, 0.5), SurfaceClass::Water);
        assert_eq!(field.at(0.6, 0.5), SurfaceClass::Rock);
    }

    #[test]
    fn surface_field_paints_scaled_building_heights() {
        let mut field =
            SurfaceField::new(21, 21, vec![SurfaceClass::Rock; 21 * 21], "test").unwrap();
        field.paint_building(
            &[[0.25, 0.25], [0.75, 0.25], [0.75, 0.75], [0.25, 0.75]],
            12.0,
        );
        assert_eq!(field.at(0.5, 0.5), SurfaceClass::Building);
        assert_eq!(field.building_height_at(0.5, 0.5), 12.0);
        assert_eq!(field.building_height_at(0.76, 0.5), 0.0);
        assert_eq!(field.building_height_at(0.1, 0.1), 0.0);

        let spec = GenerationSpec {
            width_mm: 100.0,
            ground_span_km: 1.0,
            buildings: BuildingSpec {
                enabled: true,
                z_scale: 2.0,
            },
            ..GenerationSpec::default()
        };
        assert!(
            (scaled_building_height_mm(&spec, field.building_height_at(0.5, 0.5)) - 2.4).abs()
                < 0.001
        );
    }
}
