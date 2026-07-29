//! Ocean extent from OpenStreetMap coastlines.
//!
//! The marine flood fill needs to know which border water is really the
//! sea. Land-cover raster water cannot say: the Salton Sea sits below sea
//! level, touches a map edge, and is not ocean. OpenStreetMap's
//! `natural=coastline` ways can say — they separate the sea from
//! everything else, drawn with water on the RIGHT of the way's direction —
//! so this module assembles the ways crossing the map into ocean polygons
//! and the flood fill checks its seeds against them.
//!
//! The assembly is the classic bounded-coastline closure: clip every way
//! to the map square, stitch ways that share endpoints, then close each
//! open chain along the square's boundary walking with the water — that
//! is, keeping the interior on the right. Coastline rings that close
//! inside the square are islands: holes in the sea. Anything that breaks
//! the topology — a coastline way ending mid-square — makes the whole
//! answer `None`, and the caller falls back to trusting the raster rather
//! than trusting a guess.
//!
//! The model's UV frame has u growing east and v growing north — the same
//! handedness as geographic coordinates — so "water on the right" survives
//! the projection unchanged.

use crate::mesh::point_in_polygon;

/// Where the sea is, as polygons in the model's unit square: outer rings
/// enclosing ocean, island rings cut out of it.
#[derive(Debug, Clone, PartialEq)]
pub struct OceanExtent {
    pub outers: Vec<Vec<[f32; 2]>>,
    pub islands: Vec<Vec<[f32; 2]>>,
}

impl OceanExtent {
    /// Whether a UV point lies in the sea.
    pub fn contains(&self, point: [f32; 2]) -> bool {
        self.outers
            .iter()
            .any(|outer| point_in_polygon(point, outer))
            && !self
                .islands
                .iter()
                .any(|island| point_in_polygon(point, island))
    }
}

/// Endpoints closer than this in UV are the same coastline node. Ways
/// sharing an OpenStreetMap node project to identical coordinates, so the
/// epsilon only has to absorb float noise, not data gaps.
const JOIN_EPSILON: f64 = 1e-5;
/// A point this close to the unit square's edge counts as on it.
const BOUNDARY_EPSILON: f64 = 1e-4;

/// Assembles ocean polygons from `natural=coastline` polylines in UV
/// space. Returns `None` when the area holds no coastline at all — sea
/// everywhere and land everywhere look identical then — or when the data
/// breaks topology (a way ending mid-square), so the caller can fall back
/// instead of acting on a guess.
pub fn assemble_ocean(polylines: &[Vec<[f32; 2]>]) -> Option<OceanExtent> {
    let chains = join_chains(polylines);
    let mut open = Vec::new();
    let mut islands = Vec::new();
    for chain in chains {
        let closed = chain.len() > 3 && distance(chain[0], chain[chain.len() - 1]) < JOIN_EPSILON;
        for clipped in clip_to_unit_square(&chain, closed) {
            let closed_after_clip = clipped.len() > 3
                && distance(clipped[0], clipped[clipped.len() - 1]) < JOIN_EPSILON;
            if closed_after_clip {
                islands.push(clipped);
            } else {
                // An open chain must enter and leave through the boundary;
                // an end mid-square is a data gap the closure cannot
                // bridge honestly.
                if boundary_parameter(clipped[0]).is_none()
                    || boundary_parameter(clipped[clipped.len() - 1]).is_none()
                {
                    return None;
                }
                open.push(clipped);
            }
        }
    }
    if open.is_empty() && islands.is_empty() {
        return None;
    }
    let outers = if open.is_empty() {
        // Only islands in view: everything around them is sea.
        vec![vec![[0.0, 0.0], [0.0, 1.0], [1.0, 1.0], [1.0, 0.0]]]
    } else {
        close_chains_along_boundary(open)?
    };
    Some(OceanExtent {
        outers: outers
            .into_iter()
            .map(|ring| {
                ring.into_iter()
                    .map(|p| [p[0] as f32, p[1] as f32])
                    .collect()
            })
            .collect(),
        islands: islands
            .into_iter()
            .map(|ring| {
                ring.into_iter()
                    .map(|p| [p[0] as f32, p[1] as f32])
                    .collect()
            })
            .collect(),
    })
}

fn distance(a: [f64; 2], b: [f64; 2]) -> f64 {
    (a[0] - b[0]).hypot(a[1] - b[1])
}

/// Stitches polylines end-to-start where they share a node. Coastline ways
/// run in a consistent direction (water right), so consecutive ways
/// connect one's end to the next one's start.
fn join_chains(polylines: &[Vec<[f32; 2]>]) -> Vec<Vec<[f64; 2]>> {
    let mut chains = polylines
        .iter()
        .filter(|points| points.len() >= 2)
        .map(|points| {
            points
                .iter()
                .map(|p| [f64::from(p[0]), f64::from(p[1])])
                .collect::<Vec<_>>()
        })
        .collect::<Vec<_>>();
    loop {
        let mut merged_any = false;
        'outer: for first in 0..chains.len() {
            for second in 0..chains.len() {
                if first == second {
                    continue;
                }
                let end = chains[first][chains[first].len() - 1];
                let start = chains[second][0];
                if distance(end, start) < JOIN_EPSILON {
                    let mut tail = chains.remove(second);
                    let first = if second < first { first - 1 } else { first };
                    tail.remove(0);
                    chains[first].extend(tail);
                    merged_any = true;
                    break 'outer;
                }
            }
        }
        if !merged_any {
            return chains;
        }
    }
}

/// Clips one chain to the unit square, splitting it into the sub-chains
/// that lie inside and inserting exact boundary crossings. `closed` wraps
/// the last segment back to the first point.
fn clip_to_unit_square(chain: &[[f64; 2]], closed: bool) -> Vec<Vec<[f64; 2]>> {
    let mut pieces = Vec::new();
    let mut current: Vec<[f64; 2]> = Vec::new();
    let segments = if closed { chain.len() } else { chain.len() - 1 };
    for index in 0..segments {
        let from = chain[index];
        let to = chain[(index + 1) % chain.len()];
        if let Some((entry, exit)) = clip_segment(from, to) {
            if current.is_empty() || distance(current[current.len() - 1], entry) > JOIN_EPSILON {
                if !current.is_empty() {
                    pieces.push(std::mem::take(&mut current));
                }
                current.push(entry);
            }
            current.push(exit);
        } else if !current.is_empty() {
            pieces.push(std::mem::take(&mut current));
        }
    }
    if !current.is_empty() {
        pieces.push(current);
    }
    // A closed ring clipped into nothing but one unbroken run is still the
    // ring; a run that starts and ends mid-square only because the ring
    // wrapped there is stitched back together by the caller's closed-ring
    // check, which compares first against last.
    if closed && pieces.len() > 1 {
        let first_start = pieces[0][0];
        let last = pieces.len() - 1;
        let last_end = pieces[last][pieces[last].len() - 1];
        if distance(first_start, last_end) < JOIN_EPSILON {
            let mut tail = pieces.remove(last);
            tail.pop();
            tail.extend(pieces[0].iter().copied());
            pieces[0] = tail;
        }
    }
    pieces
}

/// Liang-Barsky segment clip against the unit square. Returns the clipped
/// endpoints, or `None` when the segment misses the square.
fn clip_segment(from: [f64; 2], to: [f64; 2]) -> Option<([f64; 2], [f64; 2])> {
    let delta = [to[0] - from[0], to[1] - from[1]];
    let mut t0 = 0.0f64;
    let mut t1 = 1.0f64;
    for (p, q) in [
        (-delta[0], from[0]),
        (delta[0], 1.0 - from[0]),
        (-delta[1], from[1]),
        (delta[1], 1.0 - from[1]),
    ] {
        if p.abs() < f64::EPSILON {
            if q < 0.0 {
                return None;
            }
            continue;
        }
        let r = q / p;
        if p < 0.0 {
            if r > t1 {
                return None;
            }
            t0 = t0.max(r);
        } else {
            if r < t0 {
                return None;
            }
            t1 = t1.min(r);
        }
    }
    if t0 > t1 {
        return None;
    }
    let at = |t: f64| [from[0] + delta[0] * t, from[1] + delta[1] * t];
    Some((at(t0), at(t1)))
}

/// The boundary parameter of a point on the square's edge, walked
/// counterclockwise: bottom 0..1, right 1..2, top 2..3, left 3..4.
/// `None` when the point is not on the boundary.
fn boundary_parameter(point: [f64; 2]) -> Option<f64> {
    let [u, v] = point;
    if v.abs() < BOUNDARY_EPSILON {
        Some(u.clamp(0.0, 1.0))
    } else if (u - 1.0).abs() < BOUNDARY_EPSILON {
        Some(1.0 + v.clamp(0.0, 1.0))
    } else if (v - 1.0).abs() < BOUNDARY_EPSILON {
        Some(2.0 + (1.0 - u.clamp(0.0, 1.0)))
    } else if u.abs() < BOUNDARY_EPSILON {
        Some(3.0 + (1.0 - v.clamp(0.0, 1.0)))
    } else {
        None
    }
}

/// The square's corner at an integer boundary parameter.
fn corner(parameter: usize) -> [f64; 2] {
    match parameter {
        0 => [0.0, 0.0],
        1 => [1.0, 0.0],
        2 => [1.0, 1.0],
        _ => [0.0, 1.0],
    }
}

/// Closes open coastline chains into ocean rings along the square's
/// boundary. Water sits on the right of each chain, so the ocean interior
/// stays on the right: from a chain's exit the walk follows the boundary
/// CLOCKWISE — decreasing parameter — to the next chain entry, inserting
/// the corners it passes.
fn close_chains_along_boundary(chains: Vec<Vec<[f64; 2]>>) -> Option<Vec<Vec<[f64; 2]>>> {
    struct Open {
        points: Vec<[f64; 2]>,
        entry: f64,
        exit: f64,
        used: bool,
    }
    let mut open = chains
        .into_iter()
        .map(|points| {
            let entry = boundary_parameter(points[0])?;
            let exit = boundary_parameter(points[points.len() - 1])?;
            Some(Open {
                points,
                entry,
                exit,
                used: false,
            })
        })
        .collect::<Option<Vec<_>>>()?;

    // Walking clockwise from `from`, how far to `to`: the decreasing-t
    // distance, wrapped.
    let clockwise_gap = |from: f64, to: f64| -> f64 { (from - to).rem_euclid(4.0) };

    let mut rings = Vec::new();
    for start in 0..open.len() {
        if open[start].used {
            continue;
        }
        let mut ring: Vec<[f64; 2]> = Vec::new();
        let mut current = start;
        // Each iteration consumes one chain, so the loop is bounded.
        for _ in 0..=open.len() {
            open[current].used = true;
            ring.extend(open[current].points.iter().copied());
            let exit = open[current].exit;
            // The next entry clockwise from this exit — the start chain's
            // entry competes too, closing the ring.
            let mut next: Option<(usize, f64)> = None;
            for (index, candidate) in open.iter().enumerate() {
                if candidate.used && index != start {
                    continue;
                }
                if index == current && ring.len() > open[current].points.len() {
                    continue;
                }
                let gap = clockwise_gap(exit, candidate.entry);
                if next.is_none_or(|(_, best)| gap < best) {
                    next = Some((index, gap));
                }
            }
            let (next_index, gap) = next?;
            // Corners passed while walking clockwise across `gap`.
            let mut walked = 0.0;
            let mut t = exit;
            while walked < gap {
                let to_next_corner = {
                    let fractional = t.rem_euclid(1.0);
                    if fractional < f64::EPSILON {
                        1.0
                    } else {
                        fractional
                    }
                };
                if walked + to_next_corner >= gap {
                    break;
                }
                walked += to_next_corner;
                t = (t - to_next_corner).rem_euclid(4.0);
                ring.push(corner(t as usize % 4));
            }
            if next_index == start {
                break;
            }
            current = next_index;
        }
        if ring.len() < 3 {
            return None;
        }
        rings.push(ring);
    }
    Some(rings)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A north-south coastline down the middle, water on the right of its
    /// southward direction: the west half is sea.
    #[test]
    fn a_single_crossing_coastline_splits_the_square() {
        let coast = vec![vec![[0.5, 1.2], [0.5, 0.5], [0.5, -0.2]]];
        let ocean = assemble_ocean(&coast).unwrap();
        assert_eq!(ocean.outers.len(), 1);
        assert!(ocean.contains([0.1, 0.5]), "west of the coast is sea");
        assert!(ocean.contains([0.4, 0.9]));
        assert!(!ocean.contains([0.6, 0.5]), "east of the coast is land");
        assert!(!ocean.contains([0.9, 0.1]));
    }

    /// The same coastline drawn the other way puts the sea on the east.
    #[test]
    fn the_way_direction_decides_which_side_is_sea() {
        let coast = vec![vec![[0.5, -0.2], [0.5, 0.5], [0.5, 1.2]]];
        let ocean = assemble_ocean(&coast).unwrap();
        assert!(!ocean.contains([0.1, 0.5]));
        assert!(ocean.contains([0.9, 0.5]));
    }

    /// Ways split at arbitrary nodes stitch back into one coastline.
    #[test]
    fn split_ways_join_before_assembly() {
        let coast = vec![
            vec![[0.5, 1.2], [0.5, 0.8]],
            vec![[0.5, 0.4], [0.5, -0.2]],
            vec![[0.5, 0.8], [0.5, 0.4]],
        ];
        let ocean = assemble_ocean(&coast).unwrap();
        assert!(ocean.contains([0.2, 0.5]));
        assert!(!ocean.contains([0.8, 0.5]));
    }

    /// An island ring inside the view: sea everywhere around it, land on
    /// it. Coastline rings run with water on the right, so an island is
    /// drawn counterclockwise.
    #[test]
    fn an_island_is_a_hole_in_the_sea() {
        let island = vec![vec![
            [0.4, 0.4],
            [0.6, 0.4],
            [0.6, 0.6],
            [0.4, 0.6],
            [0.4, 0.4],
        ]];
        let ocean = assemble_ocean(&island).unwrap();
        assert!(ocean.contains([0.1, 0.1]), "around the island is sea");
        assert!(ocean.contains([0.9, 0.9]));
        assert!(!ocean.contains([0.5, 0.5]), "the island is land");
    }

    /// A bay: the coastline enters the east edge, loops through the middle
    /// and leaves the east edge again; the sea is the pocket it wraps.
    #[test]
    fn a_bay_wraps_its_own_pocket_of_sea() {
        // Entering at (1, 0.2), west, north, back east to exit (1, 0.8):
        // the right hand of that walk always points into the hook, so the
        // pocket is the sea.
        let coast = vec![vec![[1.2, 0.2], [0.4, 0.2], [0.4, 0.8], [1.2, 0.8]]];
        let ocean = assemble_ocean(&coast).unwrap();
        assert!(ocean.contains([0.7, 0.5]), "inside the hook is sea");
        assert!(!ocean.contains([0.2, 0.5]), "west of the hook is land");
        assert!(!ocean.contains([0.7, 0.9]), "north of the hook is land");
        assert!(!ocean.contains([0.7, 0.1]), "south of the hook is land");

        // The same hook walked the other way wraps land instead.
        let reversed = vec![vec![[1.2, 0.8], [0.4, 0.8], [0.4, 0.2], [1.2, 0.2]]];
        let ocean = assemble_ocean(&reversed).unwrap();
        assert!(!ocean.contains([0.7, 0.5]), "the hook wraps land");
        assert!(ocean.contains([0.2, 0.5]), "the sea is everything else");
    }

    /// No coastline at all cannot distinguish all-sea from all-land.
    #[test]
    fn no_coastline_means_no_answer() {
        assert!(assemble_ocean(&[]).is_none());
        // Fully outside the square is the same as absent.
        let far_away = vec![vec![[3.0, 3.0], [4.0, 4.0]]];
        assert!(assemble_ocean(&far_away).is_none());
    }

    /// A coastline way that ends mid-square is a data gap; guessing how to
    /// close it could flood the wrong half of the map.
    #[test]
    fn a_broken_coastline_refuses_to_answer() {
        let broken = vec![vec![[0.5, 1.2], [0.5, 0.5]]];
        assert!(assemble_ocean(&broken).is_none());
    }

    /// Two separate coastlines make two seas: a strait's shores.
    #[test]
    fn two_coastlines_bound_a_strait() {
        // West coast: northward at u=0.2 (water right = east of it? no:
        // northward travel puts the right hand east). East coast:
        // southward at u=0.8, water right = west of it. Sea is the band
        // between them.
        let coast = vec![vec![[0.2, -0.2], [0.2, 1.2]], vec![[0.8, 1.2], [0.8, -0.2]]];
        let ocean = assemble_ocean(&coast).unwrap();
        assert!(ocean.contains([0.5, 0.5]), "the strait is sea");
        assert!(!ocean.contains([0.1, 0.5]), "west shore is land");
        assert!(!ocean.contains([0.9, 0.5]), "east shore is land");
    }
}
