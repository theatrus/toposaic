//! Deterministic jigsaw-edge geometry. Puzzle pieces and interlocking tray
//! segments both draw their shared edges from these functions, so two
//! neighbours always agree on the same tab shape.

/// The shape parameters of one jigsaw edge, derived deterministically from
/// the edge's grid position so both neighbours compute the same pattern.
#[derive(Debug, Clone, Copy)]
pub(crate) struct EdgePattern {
    center: f32,
    radius_along: f32,
    depth_scale: f32,
    skew: f32,
}

pub(crate) fn shared_edge_pattern(orientation: u64, line: u32, segment: u32) -> EdgePattern {
    let seed = orientation.wrapping_mul(0x9E37_79B9_7F4A_7C15)
        ^ (line as u64).wrapping_mul(0xBF58_476D_1CE4_E5B9)
        ^ (segment as u64).wrapping_mul(0x94D0_49BB_1331_11EB);
    EdgePattern {
        center: 0.43 + edge_noise(seed, 2) * 0.14,
        radius_along: 0.11 + edge_noise(seed, 3) * 0.035,
        depth_scale: 0.88 + edge_noise(seed, 4) * 0.24,
        skew: (edge_noise(seed, 5) - 0.5) * 0.05,
    }
}

pub(crate) fn edge_noise(seed: u64, lane: u64) -> f32 {
    let mut value = seed ^ lane.wrapping_mul(0xD6E8_FEB8_6659_FD93);
    value ^= value >> 30;
    value = value.wrapping_mul(0xBF58_476D_1CE4_E5B9);
    value ^= value >> 27;
    value = value.wrapping_mul(0x94D0_49BB_1331_11EB);
    value ^= value >> 31;
    ((value >> 40) as u32) as f32 / 16_777_215.0
}

pub(crate) fn edge_sign(orientation: u64, segment: u32, line: u32, line_count: u32) -> f32 {
    if line == 0 || line == line_count {
        0.0
    } else {
        let seed = orientation.wrapping_mul(0xA24B_AED4_963E_E407)
            ^ (line as u64).wrapping_mul(0x9FB2_1C65_1E98_DF25)
            ^ (segment as u64).wrapping_mul(0xC13F_A9A9_02A6_328F);
        if edge_noise(seed, 7) < 0.5 { -1.0 } else { 1.0 }
    }
}

pub(crate) fn puzzle_edge_point(
    start: [f32; 2],
    end: [f32; 2],
    pattern: EdgePattern,
    sign: f32,
    t: f32,
    base_depth: f32,
) -> [f32; 2] {
    let delta = [end[0] - start[0], end[1] - start[1]];
    let length = delta[0].hypot(delta[1]).max(f32::EPSILON);
    let tangent = [delta[0] / length, delta[1] / length];
    let normal = [-tangent[1], tangent[0]];
    let [along, offset] = if sign == 0.0 {
        [t, 0.0]
    } else {
        jigsaw_edge(t, pattern)
    };
    let depth = base_depth * pattern.depth_scale;
    [
        start[0] + delta[0] * along + normal[0] * sign * depth * offset,
        start[1] + delta[1] * along + normal[1] * sign * depth * offset,
    ]
}

fn jigsaw_edge(t: f32, pattern: EdgePattern) -> [f32; 2] {
    let radius = pattern.radius_along;
    let neck = radius * 0.46;
    let shoulder_start = pattern.center - radius - 0.085;
    let shoulder_end = pattern.center + radius + 0.085;
    let neck_left = [pattern.center - neck, 0.18];
    let neck_right = [pattern.center + neck, 0.18];
    let head_left = [pattern.center - radius, 0.58];
    let head_right = [pattern.center + radius, 0.58];
    let quarter_circle = 0.552_284_8;
    let point = if t < 0.26 {
        [t / 0.26 * shoulder_start, 0.0]
    } else if t < 0.34 {
        cubic_bezier(
            [shoulder_start, 0.0],
            [shoulder_start + 0.045, -0.01],
            [neck_left[0] - 0.025, 0.04],
            neck_left,
            (t - 0.26) / 0.08,
        )
    } else if t < 0.42 {
        cubic_bezier(
            neck_left,
            [neck_left[0] + 0.012, 0.34],
            [head_left[0], 0.45],
            head_left,
            (t - 0.34) / 0.08,
        )
    } else if t < 0.5 {
        cubic_bezier(
            head_left,
            [
                head_left[0],
                head_left[1] + (1.0 - head_left[1]) * quarter_circle,
            ],
            [pattern.center - radius * quarter_circle, 1.0],
            [pattern.center, 1.0],
            (t - 0.42) / 0.08,
        )
    } else if t < 0.58 {
        cubic_bezier(
            [pattern.center, 1.0],
            [pattern.center + radius * quarter_circle, 1.0],
            [
                head_right[0],
                head_right[1] + (1.0 - head_right[1]) * quarter_circle,
            ],
            head_right,
            (t - 0.5) / 0.08,
        )
    } else if t < 0.66 {
        cubic_bezier(
            head_right,
            [head_right[0], 0.45],
            [neck_right[0] - 0.012, 0.34],
            neck_right,
            (t - 0.58) / 0.08,
        )
    } else if t < 0.74 {
        cubic_bezier(
            neck_right,
            [neck_right[0] + 0.025, 0.04],
            [shoulder_end - 0.045, -0.01],
            [shoulder_end, 0.0],
            (t - 0.66) / 0.08,
        )
    } else {
        [shoulder_end + (t - 0.74) / 0.26 * (1.0 - shoulder_end), 0.0]
    };
    [point[0] + pattern.skew * point[1], point[1]]
}

fn cubic_bezier(
    start: [f32; 2],
    control_a: [f32; 2],
    control_b: [f32; 2],
    end: [f32; 2],
    t: f32,
) -> [f32; 2] {
    let inverse = 1.0 - t;
    let weights = [
        inverse.powi(3),
        3.0 * inverse.powi(2) * t,
        3.0 * inverse * t.powi(2),
        t.powi(3),
    ];
    [
        start[0] * weights[0]
            + control_a[0] * weights[1]
            + control_b[0] * weights[2]
            + end[0] * weights[3],
        start[1] * weights[0]
            + control_a[1] * weights[1]
            + control_b[1] * weights[2]
            + end[1] * weights[3],
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn jigsaw_edge_has_overhanging_round_head() {
        let pattern = shared_edge_pattern(0, 1, 0);
        assert_eq!(jigsaw_edge(0.1, pattern)[1], 0.0);
        assert!(jigsaw_edge(0.5, pattern)[1] > 0.99);
        assert!(jigsaw_edge(0.42, pattern)[0] < jigsaw_edge(0.34, pattern)[0] - 0.03);
        assert!(jigsaw_edge(0.58, pattern)[0] > jigsaw_edge(0.66, pattern)[0] + 0.03);
        assert_eq!(jigsaw_edge(0.0, pattern)[1], 0.0);
        assert_eq!(jigsaw_edge(1.0, pattern)[1], 0.0);
    }

    #[test]
    fn edge_patterns_vary_between_segments() {
        let first = shared_edge_pattern(0, 1, 0);
        let second = shared_edge_pattern(0, 1, 1);
        assert!((first.center - second.center).abs() > 0.001);
        assert!((first.depth_scale - second.depth_scale).abs() > 0.001);
        assert!((first.skew - second.skew).abs() > 0.001);
    }
}
