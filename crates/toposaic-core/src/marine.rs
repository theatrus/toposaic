//! Marine water levels: issue #71's flat sea.
//!
//! The elevation provider samples whatever its source holds under the sea —
//! coarse ETOPO1 bathymetry with Mapzen, artificial surfaces elsewhere —
//! and the draped output paints water straight over it. A real sea is
//! level. This module resolves which level, finds the terrain that sea
//! covers, and flattens the height field there, so the print shows a water
//! surface instead of seabed noise.
//!
//! Only the manual and provider-zero modes resolve here. Regional tidal
//! datums (MLLW, MHHW via NOAA VDatum) come later; until then the low and
//! high presets resolve to mean sea level with a recorded warning instead
//! of a guessed regional value.

use crate::heightfield::{HeightField, VerticalReference};
use crate::spec::{MarineLevel, MarineSpec, SurfaceClass};
use crate::surface::SurfaceField;

/// A marine level made concrete: which datum it claims, where it sits in
/// the elevation provider's own reference, and what the claim rests on.
/// Everything here goes into the data sources, so an export can be traced
/// back to the level it printed.
#[derive(Debug, Clone, PartialEq)]
pub struct ResolvedMarineLevel {
    /// The datum the level claims, like `MSL` or `custom`.
    pub datum: &'static str,
    /// The level in metres above the elevation provider's zero.
    pub elevation_m: f32,
    /// Where the number comes from, naming the vertical reference.
    pub source: String,
    /// What keeps this from being a verified regional level, when
    /// something does.
    pub warning: Option<String>,
}

/// Resolves the spec's marine level against what the elevation source
/// knows about its own zero. Never guesses: a level that cannot be
/// verified resolves anyway — refusing to generate would help no one —
/// but carries the warning that says exactly what it is.
pub fn resolve_marine_level(
    spec: &MarineSpec,
    reference: VerticalReference,
) -> ResolvedMarineLevel {
    let unknown_reference = (reference == VerticalReference::Unknown).then(|| {
        "the elevation source's vertical reference is unknown; this level is the provider's \
         zero, not a verified sea level"
            .to_string()
    });
    match spec.level {
        MarineLevel::Msl => ResolvedMarineLevel {
            datum: "MSL",
            elevation_m: 0.0,
            source: format!("provider zero ({})", reference.label()),
            warning: unknown_reference,
        },
        MarineLevel::Custom => ResolvedMarineLevel {
            datum: "custom",
            // The offset already IS the level; the source names only the
            // zero it offsets from, so notes never state the number twice.
            elevation_m: spec.custom_offset_m,
            source: format!("provider zero ({})", reference.label()),
            warning: unknown_reference,
        },
        MarineLevel::LowTide | MarineLevel::HighTide => {
            let (datum, name) = if spec.level == MarineLevel::LowTide {
                ("MLLW (requested)", "low tide")
            } else {
                ("MHHW (requested)", "high tide")
            };
            ResolvedMarineLevel {
                datum,
                elevation_m: 0.0,
                source: format!("provider zero ({})", reference.label()),
                warning: Some(format!(
                    "no regional tidal datum source is wired in yet; {name} resolves to mean \
                     sea level"
                )),
            }
        }
    }
}

/// What the flat surface did, for the data sources.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct MarineOutcome {
    /// Height samples set to the water plane.
    pub flattened_heights: usize,
    /// Water the slope gates had demoted on the printed seabed wall, given
    /// back: the flat sea removes the wall the gate answered to.
    pub restored_water: usize,
    /// Water-classed samples above a lowered plane turned to ground:
    /// intertidal terrain the low water reveals.
    pub exposed_seabed: usize,
    /// Land below a raised plane turned to water: terrain the high water
    /// covers.
    pub flooded_land: usize,
}

impl MarineOutcome {
    pub fn is_no_op(&self) -> bool {
        *self == Self::default()
    }
}

/// Flattens the marine surface to `level_m`: finds every raster sample the
/// sea covers, sets the height field to the plane there, and squares the
/// classes with the new surface — flooding land a raised plane covers,
/// exposing seabed a lowered plane reveals, and restoring water the slope
/// gates demoted on walls the flat sea no longer has.
///
/// Marine means connected to the open water at the map's edge. The flood
/// fill starts from border samples that are water at or below the plane
/// and never leaps over land above it, so an inland lake or depression
/// keeps its own level whatever the sea does. A model with no border
/// water — landlocked terrain — is untouched.
///
/// `freeze_edge_ring` is the super-tile contract: ring samples then join
/// the sea only by the border rule (water at or below the plane), never
/// by interior connectivity, so both sides of a shared edge decide each
/// ring sample from shared data alone and seams stay equal.
pub fn apply_flat_marine_surface(
    field: &mut SurfaceField,
    heights: &mut HeightField,
    level_m: f32,
    freeze_edge_ring: bool,
) -> MarineOutcome {
    let width = field.width;
    let height = field.height;
    let uv = |x: usize, y: usize| {
        (
            x as f32 / (width - 1) as f32,
            y as f32 / (height - 1) as f32,
        )
    };
    let elevations = (0..width * height)
        .map(|index| {
            let (u, v) = uv(index % width, index / width);
            heights.elevation_m_at(u, v)
        })
        .collect::<Vec<_>>();
    let is_water = |field: &SurfaceField, index: usize| {
        field.classes[index] == SurfaceClass::Water
            || field.base_classes[index] == SurfaceClass::Water
    };
    let on_ring = |index: usize| -> bool {
        let (x, y) = (index % width, index / width);
        x == 0 || y == 0 || x == width - 1 || y == height - 1
    };
    // The sea enters at the border: water at or below the plane. Land
    // below a raised plane spreads the flood inland but never starts it —
    // a dry border depression is not the ocean.
    let seed = |field: &SurfaceField, index: usize| {
        on_ring(index) && is_water(field, index) && elevations[index] <= level_m
    };
    let spreadable = |field: &SurfaceField, index: usize| {
        elevations[index] <= level_m && (is_water(field, index) || level_m > 0.0)
    };

    let mut marine = vec![false; width * height];
    let mut queue = Vec::new();
    for (index, masked) in marine.iter_mut().enumerate() {
        if seed(field, index) {
            *masked = true;
            queue.push(index);
        }
    }
    while let Some(index) = queue.pop() {
        let (x, y) = (index % width, index / width);
        for (dx, dy) in [(0i64, -1i64), (0, 1), (-1, 0), (1, 0)] {
            let (nx, ny) = (x as i64 + dx, y as i64 + dy);
            if nx < 0 || ny < 0 || nx >= width as i64 || ny >= height as i64 {
                continue;
            }
            let neighbour = ny as usize * width + nx as usize;
            if marine[neighbour] || !spreadable(field, neighbour) {
                continue;
            }
            // Under the super-tile contract a ring sample joins only as a
            // seed; interior connectivity must not decide what a shared
            // edge shows.
            if freeze_edge_ring && on_ring(neighbour) {
                continue;
            }
            marine[neighbour] = true;
            queue.push(neighbour);
        }
    }

    let mut outcome = MarineOutcome::default();

    // Square the classes with the flood. A masked land sample is covered;
    // masked water the gates had turned to rock comes back.
    for (index, &masked) in marine.iter().enumerate() {
        if !masked {
            continue;
        }
        if field.classes[index] != SurfaceClass::Water {
            if field.base_classes[index] == SurfaceClass::Water {
                outcome.restored_water += 1;
            } else {
                outcome.flooded_land += 1;
            }
            field.classes[index] = SurfaceClass::Water;
            field.base_classes[index] = SurfaceClass::Water;
        }
    }

    // A lowered plane reveals intertidal ground: water above the plane but
    // no deeper than provider zero, touching the sea. Growing through the
    // candidates lets a whole tidal flat dry out, while a river — water
    // above provider zero — never qualifies, and an inland lake is never
    // adjacent to the sea. Ring samples keep their class under the
    // super-tile contract for the same reason they only seed.
    if level_m < 0.0 {
        let exposable = |field: &SurfaceField, index: usize| {
            !marine[index]
                && is_water(field, index)
                && elevations[index] > level_m
                && elevations[index] <= 0.0
                && !(freeze_edge_ring && on_ring(index))
        };
        let mut exposed = vec![false; width * height];
        let mut queue = Vec::new();
        for (index, starts_dry) in exposed.iter_mut().enumerate() {
            if !exposable(field, index) {
                continue;
            }
            let (x, y) = (index % width, index / width);
            let touches_sea = [(0i64, -1i64), (0, 1), (-1, 0), (1, 0)]
                .iter()
                .any(|(dx, dy)| {
                    let (nx, ny) = (x as i64 + dx, y as i64 + dy);
                    nx >= 0
                        && ny >= 0
                        && nx < width as i64
                        && ny < height as i64
                        && marine[ny as usize * width + nx as usize]
                });
            if touches_sea {
                *starts_dry = true;
                queue.push(index);
            }
        }
        while let Some(index) = queue.pop() {
            let (x, y) = (index % width, index / width);
            for (dx, dy) in [(0i64, -1i64), (0, 1), (-1, 0), (1, 0)] {
                let (nx, ny) = (x as i64 + dx, y as i64 + dy);
                if nx < 0 || ny < 0 || nx >= width as i64 || ny >= height as i64 {
                    continue;
                }
                let neighbour = ny as usize * width + nx as usize;
                if !exposed[neighbour] && exposable(field, neighbour) {
                    exposed[neighbour] = true;
                    queue.push(neighbour);
                }
            }
        }
        for (index, &dried) in exposed.iter().enumerate() {
            if dried {
                field.classes[index] = SurfaceClass::Rock;
                field.base_classes[index] = SurfaceClass::Rock;
                outcome.exposed_seabed += 1;
            }
        }
    }

    // The plane itself: every height sample over a marine surface sample
    // becomes the level. Nearest-sample lookup matches how the sampler
    // reads classes, so the flat area and the water color agree to the
    // raster's own resolution.
    for hy in 0..heights.height {
        let v = hy as f32 / (heights.height - 1) as f32;
        let y = (v * (height - 1) as f32).round() as usize;
        for hx in 0..heights.width {
            let u = hx as f32 / (heights.width - 1) as f32;
            let x = (u * (width - 1) as f32).round() as usize;
            if marine[y * width + x] {
                heights.values_m[hy * heights.width + hx] = level_m;
                outcome.flattened_heights += 1;
            }
        }
    }
    outcome
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A coast rising west to east from -8 m to +8 m, water classed
    /// wherever the ground sits below zero, with an inland lake basin in
    /// the north-east corner: land at +4 around a water pocket at -3.
    fn coast(size: usize) -> (SurfaceField, HeightField) {
        let elevation = |x: usize| -> f32 { -8.0 + 16.0 * x as f32 / (size - 1) as f32 };
        let mut values = Vec::with_capacity(size * size);
        let mut classes = Vec::with_capacity(size * size);
        for y in 0..size {
            for x in 0..size {
                let lake = (10..=12).contains(&x) && (2..=4).contains(&y);
                let lake_rim = (9..=13).contains(&x) && (1..=5).contains(&y) && !lake;
                if lake {
                    values.push(-3.0);
                    classes.push(SurfaceClass::Water);
                } else if lake_rim {
                    values.push(4.0);
                    classes.push(SurfaceClass::Rock);
                } else {
                    values.push(elevation(x));
                    classes.push(if elevation(x) < 0.0 {
                        SurfaceClass::Water
                    } else {
                        SurfaceClass::Rock
                    });
                }
            }
        }
        let field = SurfaceField::new(size, size, classes, "test").unwrap();
        let heights = HeightField::new(size, size, values, "test").unwrap();
        (field, heights)
    }

    fn water_count(field: &SurfaceField) -> usize {
        field
            .classes
            .iter()
            .filter(|&&class| class == SurfaceClass::Water)
            .count()
    }

    #[test]
    fn the_sea_flattens_to_the_plane_and_the_lake_keeps_its_own_level() {
        let (mut field, mut heights) = coast(17);
        let outcome = apply_flat_marine_surface(&mut field, &mut heights, 0.0, false);
        assert!(outcome.flattened_heights > 0);
        // Sea samples sit exactly on the plane; the lake keeps its basin.
        assert_eq!(heights.values_m[8 * 17], 0.0, "west edge sea");
        assert_eq!(heights.values_m[3 * 17 + 11], -3.0, "lake basin");
        assert_eq!(field.classes[3 * 17 + 11], SurfaceClass::Water);
        // Land keeps its slope.
        assert!(heights.values_m[8 * 17 + 16] > 7.0);
    }

    #[test]
    fn low_water_never_covers_more_than_msl_and_reveals_the_foreshore() {
        let (mut msl_field, mut msl_heights) = coast(17);
        apply_flat_marine_surface(&mut msl_field, &mut msl_heights, 0.0, false);
        let (mut low_field, mut low_heights) = coast(17);
        let outcome = apply_flat_marine_surface(&mut low_field, &mut low_heights, -3.0, false);
        // The acceptance line: low tide never covers more coast than MSL.
        assert!(water_count(&low_field) < water_count(&msl_field));
        // The strip between -3 and 0 dried out and reads as ground.
        assert!(outcome.exposed_seabed > 0);
        let still_sea = 8 * 17 + 3; // elevation -5, below the -3 plane
        assert_eq!(low_field.classes[still_sea], SurfaceClass::Water);
        let dried = 8 * 17 + 6; // elevation -2, between the planes
        assert_eq!(low_field.classes[dried], SurfaceClass::Rock);
        assert_eq!(low_heights.values_m[8 * 17], -3.0);
        // The lake at -3 is untouched by a marine low.
        assert_eq!(low_field.classes[3 * 17 + 11], SurfaceClass::Water);
    }

    #[test]
    fn high_water_floods_connected_land_but_not_inland_depressions() {
        let size = 17;
        let (mut field, mut heights) = coast(size);
        // Dig an inland depression at +1 … below the +2 plane but ringed by
        // higher land, so the sea cannot reach it.
        let basin = 14 * size + 14;
        heights.values_m[basin] = 1.0;
        let outcome = apply_flat_marine_surface(&mut field, &mut heights, 2.0, false);
        assert!(outcome.flooded_land > 0);
        // Coastal land below +2 is covered and sits on the plane.
        let flooded = 8 * size + 9; // elevation -8+9=+1, sea-connected
        assert_eq!(field.classes[flooded], SurfaceClass::Water);
        assert_eq!(heights.values_m[flooded], 2.0);
        // The ringed depression stays dry land.
        assert_eq!(field.classes[basin], SurfaceClass::Rock);
        assert_eq!(heights.values_m[basin], 1.0);
        // MSL covers less than high water: the other half of the
        // acceptance line.
        let (mut msl_field, mut msl_heights) = coast(size);
        apply_flat_marine_surface(&mut msl_field, &mut msl_heights, 0.0, false);
        assert!(water_count(&msl_field) < water_count(&field));
    }

    #[test]
    fn gate_demoted_shoreline_water_comes_back_under_the_flat_sea() {
        let (mut field, mut heights) = coast(17);
        // The slope gates demoted a below-zero shoreline sample to rock;
        // its base class still says water. The flat sea removes the wall
        // the gate answered to, so the sample returns to water.
        let demoted = 8 * 17 + 5; // elevation -3, sea-connected
        field.classes[demoted] = SurfaceClass::Rock;
        let outcome = apply_flat_marine_surface(&mut field, &mut heights, 0.0, false);
        assert_eq!(outcome.restored_water, 1);
        assert_eq!(field.classes[demoted], SurfaceClass::Water);
    }

    #[test]
    fn a_landlocked_model_is_untouched() {
        let size = 9;
        // A mountain lake basin: all land except an interior water pocket.
        let mut classes = vec![SurfaceClass::Rock; size * size];
        classes[4 * size + 4] = SurfaceClass::Water;
        let mut values = vec![100.0f32; size * size];
        values[4 * size + 4] = -5.0;
        let mut field = SurfaceField::new(size, size, classes, "test").unwrap();
        let mut heights = HeightField::new(size, size, values, "test").unwrap();
        let outcome = apply_flat_marine_surface(&mut field, &mut heights, 0.0, false);
        assert!(outcome.is_no_op());
        assert_eq!(heights.values_m[4 * size + 4], -5.0);
    }

    #[test]
    fn the_frozen_ring_joins_by_the_border_rule_alone() {
        let size = 17;
        // High water: interior flooding would reach ring land below the
        // plane, but under the super-tile contract a ring sample joins
        // only as border water — shared data both tiles read alike.
        let (mut field, mut heights) = coast(size);
        let ring_land = 9; // top row, elevation -8+9=+1, below the +2 plane
        assert_eq!(field.classes[ring_land], SurfaceClass::Rock);
        apply_flat_marine_surface(&mut field, &mut heights, 2.0, true);
        assert_eq!(
            field.classes[ring_land],
            SurfaceClass::Rock,
            "ring land was flooded by interior connectivity"
        );
        // Ring water below the plane still flattens: the border rule.
        assert_eq!(heights.values_m[4], 2.0, "top-row sea sample");
        // Without the freeze the same sample floods.
        let (mut field, mut heights) = coast(size);
        apply_flat_marine_surface(&mut field, &mut heights, 2.0, false);
        assert_eq!(field.classes[ring_land], SurfaceClass::Water);
    }

    #[test]
    fn levels_resolve_with_honest_provenance() {
        let msl = resolve_marine_level(&MarineSpec::default(), VerticalReference::Egm2008);
        assert_eq!(msl.datum, "MSL");
        assert_eq!(msl.elevation_m, 0.0);
        assert!(msl.source.contains("EGM2008"));
        assert!(msl.warning.is_none());

        let unknown = resolve_marine_level(&MarineSpec::default(), VerticalReference::Unknown);
        assert!(unknown.warning.as_deref().unwrap().contains("unknown"));

        let custom = MarineSpec {
            level: MarineLevel::Custom,
            custom_offset_m: -2.5,
            ..MarineSpec::default()
        };
        let custom = resolve_marine_level(&custom, VerticalReference::Egm96);
        assert_eq!(custom.elevation_m, -2.5);
        assert!(custom.source.contains("EGM96"));
        // The note layer prints the level itself; the source must not
        // repeat it, or the manifest reads "-1.50 m from -1.50 m from".
        assert!(!custom.source.contains("-2.5"));

        let low = MarineSpec {
            level: MarineLevel::LowTide,
            ..MarineSpec::default()
        };
        let low = resolve_marine_level(&low, VerticalReference::Egm96);
        assert_eq!(low.datum, "MLLW (requested)");
        assert_eq!(low.elevation_m, 0.0);
        assert!(low.warning.as_deref().unwrap().contains("low tide"));
    }
}
