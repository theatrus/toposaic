//! Placement shared by terrain sockets and their matching tray pins.

use crate::planar_mesh::outline_bounds;
use crate::spec::GenerationSpec;

pub(crate) fn retention_centers_local(
    spec: &GenerationSpec,
    row: u32,
    column: u32,
    outline: &[[f32; 2]],
) -> Vec<[f32; 2]> {
    if spec.solid_model {
        return (0..spec.tray.segment_rows)
            .flat_map(|segment_row| {
                (0..spec.tray.segment_columns).map(move |segment_column| {
                    [
                        spec.width_mm * (segment_column as f32 + 0.5)
                            / spec.tray.segment_columns as f32,
                        spec.height_mm() * (segment_row as f32 + 0.5)
                            / spec.tray.segment_rows as f32,
                    ]
                })
            })
            .collect();
    }

    let [minimum_x, minimum_y, maximum_x, maximum_y] = outline_bounds(outline);
    let mut center = [
        minimum_x + (maximum_x - minimum_x) * 0.5,
        minimum_y + (maximum_y - minimum_y) * 0.5,
    ];
    let radius = spec.puzzle_retention.socket_diameter_mm() * 0.5;
    move_center_off_segment_seams(
        &mut center[0],
        column as f32 * spec.width_mm / spec.columns as f32,
        spec.width_mm,
        spec.tray.segment_columns,
        minimum_x,
        maximum_x,
        radius,
    );
    move_center_off_segment_seams(
        &mut center[1],
        row as f32 * spec.height_mm() / spec.rows as f32,
        spec.height_mm(),
        spec.tray.segment_rows,
        minimum_y,
        maximum_y,
        radius,
    );
    vec![center]
}

#[allow(clippy::too_many_arguments)]
fn move_center_off_segment_seams(
    center: &mut f32,
    piece_origin: f32,
    assembled_size: f32,
    segment_count: u32,
    minimum: f32,
    maximum: f32,
    radius: f32,
) {
    let margin = radius + 0.35;
    for segment in 1..segment_count {
        let seam = assembled_size * segment as f32 / segment_count as f32 - piece_origin;
        if (*center - seam).abs() >= margin {
            continue;
        }
        let left = seam - margin;
        let right = seam + margin;
        *center = if left - minimum >= maximum - right {
            left
        } else {
            right
        };
    }
}
