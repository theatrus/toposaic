use std::time::Instant;

use crate::surface::SurfaceField;

use super::{buildings, overlays};

#[derive(Debug, Clone, Default)]
pub(crate) struct PieceGeometryTiming {
    pub(super) row: u32,
    pub(super) column: u32,
    pub(super) samples: usize,
    pub(super) terrain_us: u64,
    pub(super) buildings: Option<buildings::BuildingGeometryTiming>,
    pub(super) roads: Option<overlays::RoadGeometryTiming>,
    pub(super) markers_us: u64,
    pub(super) labels_us: u64,
    pub(super) weld_us: u64,
    pub(super) total_us: u64,
    pub(super) vertices: usize,
    pub(super) triangles: usize,
}

#[derive(Debug, Clone, serde::Serialize)]
struct SlowPieceGeometryTiming {
    /// One-based display row.
    row: u32,
    /// One-based display column.
    column: u32,
    total_ms: f64,
    terrain_ms: f64,
    buildings_ms: f64,
    roads_ms: f64,
    weld_ms: f64,
}

#[derive(Debug, Clone, serde::Serialize)]
pub(crate) struct GeometryTimingSummary {
    /// Wall time reported by the caller for its parallel piece phase. Preview
    /// calls measure mesh construction only; exports also include per-piece STL
    /// writes from the same worker tasks.
    piece_phase_wall_ms: f64,
    /// Sum of each piece's elapsed time. This can exceed
    /// `piece_phase_wall_ms` because pieces build in parallel.
    piece_work_ms: f64,
    terrain_work_ms: f64,
    buildings_work_ms: f64,
    building_selection_work_ms: f64,
    building_clipping_work_ms: f64,
    building_union_work_ms: f64,
    building_obstacle_work_ms: f64,
    building_assignment_work_ms: f64,
    building_shell_work_ms: f64,
    building_cache_hits: usize,
    roads_work_ms: f64,
    road_obstacle_work_ms: f64,
    road_ribbon_clip_work_ms: f64,
    road_building_cutback_work_ms: f64,
    road_cache_hits: usize,
    aviation_work_ms: f64,
    markers_work_ms: f64,
    labels_work_ms: f64,
    weld_work_ms: f64,
    tray_ms: f64,
    serialization_ms: f64,
    piece_count: usize,
    minimum_samples_per_piece: usize,
    maximum_samples_per_piece: usize,
    source_building_count: usize,
    source_line_count: usize,
    source_area_count: usize,
    /// Candidate counts sum over pieces, so a feature crossing a seam can
    /// count more than once.
    building_candidate_count: usize,
    clipped_building_count: usize,
    building_component_count: usize,
    line_candidate_count: usize,
    ribbon_clip_count: usize,
    obstacle_cutback_count: usize,
    vertices: usize,
    triangles: usize,
    slowest_pieces: Vec<SlowPieceGeometryTiming>,
}

fn microseconds_as_milliseconds(value: u64) -> f64 {
    value as f64 / 1_000.0
}

pub(crate) fn summarize_geometry_timing(
    pieces: &[PieceGeometryTiming],
    surface_field: Option<&SurfaceField>,
    wall_us: u64,
    tray_us: u64,
    serialization_us: u64,
) -> GeometryTimingSummary {
    let sum = |read: &dyn Fn(&PieceGeometryTiming) -> u64| pieces.iter().map(read).sum::<u64>();
    let building_sum = |read: &dyn Fn(&buildings::BuildingGeometryTiming) -> u64| {
        pieces
            .iter()
            .filter_map(|piece| piece.buildings.as_ref())
            .map(read)
            .sum::<u64>()
    };
    let road_sum = |read: &dyn Fn(&overlays::RoadGeometryTiming) -> u64| {
        pieces
            .iter()
            .filter_map(|piece| piece.roads.as_ref())
            .map(read)
            .sum::<u64>()
    };
    let mut slowest = pieces.iter().collect::<Vec<_>>();
    slowest.sort_unstable_by_key(|piece| std::cmp::Reverse(piece.total_us));
    let slowest_pieces = slowest
        .into_iter()
        .take(8)
        .map(|piece| SlowPieceGeometryTiming {
            row: piece.row + 1,
            column: piece.column + 1,
            total_ms: microseconds_as_milliseconds(piece.total_us),
            terrain_ms: microseconds_as_milliseconds(piece.terrain_us),
            buildings_ms: microseconds_as_milliseconds(
                piece.buildings.as_ref().map_or(0, |timing| timing.total_us),
            ),
            roads_ms: microseconds_as_milliseconds(
                piece.roads.as_ref().map_or(0, |timing| timing.total_us),
            ),
            weld_ms: microseconds_as_milliseconds(piece.weld_us),
        })
        .collect();
    let source_building_count = surface_field.map_or(0, |field| {
        field
            .vector_areas
            .iter()
            .filter(|area| area.building_height_m > 0.0)
            .count()
    });
    GeometryTimingSummary {
        piece_phase_wall_ms: microseconds_as_milliseconds(wall_us),
        piece_work_ms: microseconds_as_milliseconds(sum(&|piece| piece.total_us)),
        terrain_work_ms: microseconds_as_milliseconds(sum(&|piece| piece.terrain_us)),
        buildings_work_ms: microseconds_as_milliseconds(building_sum(&|timing| timing.total_us)),
        building_selection_work_ms: microseconds_as_milliseconds(building_sum(&|timing| {
            timing.selection_us
        })),
        building_clipping_work_ms: microseconds_as_milliseconds(building_sum(&|timing| {
            timing.clipping_us
        })),
        building_union_work_ms: microseconds_as_milliseconds(building_sum(&|timing| {
            timing.union_us
        })),
        building_obstacle_work_ms: microseconds_as_milliseconds(building_sum(&|timing| {
            timing.obstacle_us
        })),
        building_assignment_work_ms: microseconds_as_milliseconds(building_sum(&|timing| {
            timing.assignment_us
        })),
        building_shell_work_ms: microseconds_as_milliseconds(building_sum(&|timing| {
            timing.shell_us
        })),
        building_cache_hits: pieces
            .iter()
            .filter_map(|piece| piece.buildings.as_ref())
            .filter(|timing| timing.cache_hit)
            .count(),
        roads_work_ms: microseconds_as_milliseconds(road_sum(&|timing| timing.total_us)),
        road_obstacle_work_ms: microseconds_as_milliseconds(road_sum(&|timing| timing.obstacle_us)),
        road_ribbon_clip_work_ms: microseconds_as_milliseconds(road_sum(&|timing| {
            timing.ribbon_clip_work_us
        })),
        road_building_cutback_work_ms: microseconds_as_milliseconds(road_sum(&|timing| {
            timing.building_cutback_work_us
        })),
        road_cache_hits: pieces
            .iter()
            .filter_map(|piece| piece.roads.as_ref())
            .filter(|timing| timing.cache_hit)
            .count(),
        aviation_work_ms: microseconds_as_milliseconds(road_sum(&|timing| timing.aviation_us)),
        markers_work_ms: microseconds_as_milliseconds(sum(&|piece| piece.markers_us)),
        labels_work_ms: microseconds_as_milliseconds(sum(&|piece| piece.labels_us)),
        weld_work_ms: microseconds_as_milliseconds(sum(&|piece| piece.weld_us)),
        tray_ms: microseconds_as_milliseconds(tray_us),
        serialization_ms: microseconds_as_milliseconds(serialization_us),
        piece_count: pieces.len(),
        minimum_samples_per_piece: pieces.iter().map(|piece| piece.samples).min().unwrap_or(0),
        maximum_samples_per_piece: pieces.iter().map(|piece| piece.samples).max().unwrap_or(0),
        source_building_count,
        source_line_count: surface_field.map_or(0, |field| field.vector_lines.len()),
        source_area_count: surface_field.map_or(0, |field| field.vector_areas.len()),
        building_candidate_count: pieces
            .iter()
            .filter_map(|piece| piece.buildings.as_ref())
            .map(|timing| timing.candidate_count)
            .sum(),
        clipped_building_count: pieces
            .iter()
            .filter_map(|piece| piece.buildings.as_ref())
            .map(|timing| timing.clipped_count)
            .sum(),
        building_component_count: pieces
            .iter()
            .filter_map(|piece| piece.buildings.as_ref())
            .map(|timing| timing.component_count)
            .sum(),
        line_candidate_count: pieces
            .iter()
            .filter_map(|piece| piece.roads.as_ref())
            .map(|timing| timing.line_count)
            .sum(),
        ribbon_clip_count: pieces
            .iter()
            .filter_map(|piece| piece.roads.as_ref())
            .map(|timing| timing.ribbon_clip_count)
            .sum(),
        obstacle_cutback_count: pieces
            .iter()
            .filter_map(|piece| piece.roads.as_ref())
            .map(|timing| timing.obstacle_cutback_count)
            .sum(),
        vertices: pieces.iter().map(|piece| piece.vertices).sum(),
        triangles: pieces.iter().map(|piece| piece.triangles).sum(),
        slowest_pieces,
    }
}

pub(crate) fn elapsed_us(started: Instant) -> u64 {
    started.elapsed().as_micros().min(u128::from(u64::MAX)) as u64
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sums_parallel_work_and_sorts_slow_pieces() {
        let pieces = vec![
            PieceGeometryTiming {
                row: 0,
                column: 1,
                samples: 32,
                terrain_us: 1_000,
                buildings: Some(buildings::BuildingGeometryTiming {
                    candidate_count: 4,
                    clipped_count: 3,
                    component_count: 2,
                    total_us: 3_000,
                    ..buildings::BuildingGeometryTiming::default()
                }),
                roads: Some(overlays::RoadGeometryTiming {
                    line_count: 6,
                    ribbon_clip_count: 5,
                    total_us: 4_000,
                    ..overlays::RoadGeometryTiming::default()
                }),
                total_us: 9_000,
                vertices: 100,
                triangles: 200,
                ..PieceGeometryTiming::default()
            },
            PieceGeometryTiming {
                row: 2,
                column: 3,
                samples: 48,
                terrain_us: 2_000,
                total_us: 12_000,
                vertices: 300,
                triangles: 500,
                ..PieceGeometryTiming::default()
            },
        ];

        let summary = summarize_geometry_timing(&pieces, None, 10_000, 2_000, 500);

        assert_eq!(summary.piece_phase_wall_ms, 10.0);
        assert_eq!(summary.piece_work_ms, 21.0);
        assert_eq!(summary.terrain_work_ms, 3.0);
        assert_eq!(summary.buildings_work_ms, 3.0);
        assert_eq!(summary.roads_work_ms, 4.0);
        assert_eq!(summary.minimum_samples_per_piece, 32);
        assert_eq!(summary.maximum_samples_per_piece, 48);
        assert_eq!(summary.building_candidate_count, 4);
        assert_eq!(summary.line_candidate_count, 6);
        assert_eq!(summary.ribbon_clip_count, 5);
        assert_eq!(summary.vertices, 400);
        assert_eq!(summary.triangles, 700);
        assert_eq!(summary.slowest_pieces[0].row, 3);
        assert_eq!(summary.slowest_pieces[0].column, 4);
    }
}
