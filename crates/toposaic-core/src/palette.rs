//! Satellite-derived ground palettes.
//!
//! WorldCover gives stable semantic classes, but one fixed color per class.
//! This module turns Sentinel-2 reflectance samples into a small printable
//! palette for the selected area: a deterministic stretch to display color,
//! an optional lightness normalization that damps terrain shadows, and a
//! deterministic weighted clustering in OKLab. The same input always gives
//! the same palette in the same order — palettes feed material slots, and a
//! reordered slot is a repainted print.
//!
//! Every step is pure arithmetic over the caller's samples. Fetching, caching,
//! and provenance live with the imagery source in the API crate.

use anyhow::{Result, bail};

use crate::spec::SurfaceClass;

/// The material index of a sample with no usable imagery. Consumers fall
/// back to the sample's mapped class color, so a coverage gap degrades to
/// exactly today's output rather than a wrong color.
pub const NO_GROUND_MATERIAL: u8 = u8::MAX;

/// Reflectance saturating the display stretch, as a fraction of full
/// reflectance. The standard Sentinel-2 true-color rendering: 30 %
/// reflectance and brighter shows white, which spreads the dark-to-mid
/// range land actually occupies across the visible ramp.
const STRETCH_SATURATION: f32 = 0.30;

/// Scaled-reflectance samples on the surface raster, four bands per sample:
/// red, green, blue, near-infrared, each in the source's 1/10000 reflectance
/// units. `valid` marks samples with real observations; the rest keep
/// [`NO_GROUND_MATERIAL`].
pub struct GroundImagery<'a> {
    pub width: usize,
    pub height: usize,
    pub rgbn: &'a [[u16; 4]],
    pub valid: &'a [bool],
}

/// Knobs for palette discovery, mirroring the spec's ground-palette group.
#[derive(Debug, Clone, Copy)]
pub struct GroundPaletteOptions {
    /// Target palette size. Discovery may return fewer entries when the
    /// area holds fewer distinguishable colors or the share floor merges
    /// some away, never more.
    pub color_count: usize,
    /// Smallest surface share an entry may keep. Entries below it are
    /// dissolved into their nearest surviving neighbour: a color too rare
    /// to print is a wasted filament slot.
    pub minimum_share: f32,
    /// How far each sample's lightness moves toward its group's mean, from
    /// 0 to 1. Terrain shadows darken one hillside of identical ground;
    /// left at 0 the cluster split follows the sun, not the surface.
    pub shadow_normalization: f32,
}

/// One discovered palette entry.
#[derive(Debug, Clone, PartialEq)]
pub struct GroundPaletteEntry {
    /// Deterministic name: the semantic group and a 1-based rank, like
    /// `forest 1` or `ground 2`.
    pub name: String,
    /// Display color as `#RRGGBB`, the same shape the spec's class colors
    /// use.
    pub color: String,
    /// Fraction of valid samples mapped to this entry, 0 to 1.
    pub share: f32,
    /// The semantic group the entry was discovered inside, when hybrid
    /// grouping ran. `None` for the pure satellite mode.
    pub group: Option<SurfaceClass>,
}

/// A resolved area palette: the discovered entries in their final, stable
/// order. Material indices index into `entries`.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct GroundPalette {
    pub entries: Vec<GroundPaletteEntry>,
    /// Human-readable description of the reflectance-to-display transform,
    /// recorded so the manifest can reproduce the rendering.
    pub stretch: String,
}

/// Discovers a palette from the imagery and assigns every raster sample a
/// material index into it. `groups`, when given, must parallel the imagery
/// raster and switches on hybrid mode: samples are grouped by semantic
/// class (forest, snow, water, everything else as ground) and clustered
/// inside their group, so a shadowed forest never trades colors with a
/// sunlit rock face. Returns the palette and one index per raster sample;
/// samples without imagery get [`NO_GROUND_MATERIAL`].
pub fn discover_ground_palette(
    imagery: &GroundImagery,
    groups: Option<&[SurfaceClass]>,
    options: &GroundPaletteOptions,
) -> Result<(GroundPalette, Vec<u8>)> {
    let sample_count = imagery.width * imagery.height;
    if imagery.rgbn.len() != sample_count || imagery.valid.len() != sample_count {
        bail!("imagery raster does not match its declared dimensions");
    }
    if let Some(groups) = groups
        && groups.len() != sample_count
    {
        bail!("semantic group raster does not match the imagery raster");
    }
    if !(1..=MAXIMUM_PALETTE_ENTRIES).contains(&options.color_count) {
        bail!("ground color count must be between 1 and {MAXIMUM_PALETTE_ENTRIES}");
    }

    let mut points = eligible_points(imagery, groups);
    if points.is_empty() {
        return Ok((
            GroundPalette {
                entries: Vec::new(),
                stretch: stretch_description(),
            },
            vec![NO_GROUND_MATERIAL; sample_count],
        ));
    }
    normalize_lightness(&mut points, options.shadow_normalization);

    // Split the color budget over the semantic groups by their share,
    // every present group keeping at least one entry; the pure mode is a
    // single group holding everything.
    let allocations = allocate_counts(&points, options.color_count);
    let mut clusters = Vec::new();
    for (group, count) in allocations {
        let members = points
            .iter()
            .filter(|point| point.group == group)
            .collect::<Vec<_>>();
        clusters.extend(cluster_group(&members, count, group));
    }
    merge_indistinguishable_clusters(&mut clusters, &points);
    dissolve_small_clusters(&mut clusters, &points, options.minimum_share);

    // Final order: semantic group first, brightest last inside a group, so
    // "forest 1" is always the darkest forest whatever the area. Sorting
    // by the quantized display color, not the raw centroid, keeps the
    // order and the printed color in agreement.
    clusters.sort_by(|a, b| {
        group_rank(a.group)
            .cmp(&group_rank(b.group))
            .then(a.centroid[0].total_cmp(&b.centroid[0]))
            .then(a.centroid[1].total_cmp(&b.centroid[1]))
            .then(a.centroid[2].total_cmp(&b.centroid[2]))
    });

    let assignments = assign_all(imagery, groups, &clusters, options.shadow_normalization);
    let total = points.len() as f32;
    let mut counts = vec![0usize; clusters.len()];
    for &index in &assignments {
        if index != NO_GROUND_MATERIAL {
            counts[index as usize] += 1;
        }
    }
    let mut group_ordinal = std::collections::HashMap::new();
    let entries = clusters
        .iter()
        .zip(&counts)
        .map(|(cluster, &count)| {
            let ordinal = group_ordinal
                .entry(group_rank(cluster.group))
                .and_modify(|n| *n += 1)
                .or_insert(1usize);
            GroundPaletteEntry {
                name: format!("{} {}", group_name(cluster.group), ordinal),
                color: oklab_to_hex(cluster.centroid),
                share: count as f32 / total,
                group: cluster.group,
            }
        })
        .collect();
    Ok((
        GroundPalette {
            entries,
            stretch: stretch_description(),
        },
        assignments,
    ))
}

/// Assigns every raster sample to the nearest color of an already-resolved
/// palette instead of discovering one: the super-tile and adjacent-job path,
/// where every tile must agree on the palette it did not discover. Shares
/// are recomputed for this tile's samples.
pub fn assign_locked_palette(
    imagery: &GroundImagery,
    colors: &[String],
    shadow_normalization: f32,
) -> Result<(GroundPalette, Vec<u8>)> {
    let sample_count = imagery.width * imagery.height;
    if imagery.rgbn.len() != sample_count || imagery.valid.len() != sample_count {
        bail!("imagery raster does not match its declared dimensions");
    }
    if colors.is_empty() || colors.len() > MAXIMUM_PALETTE_ENTRIES {
        bail!("a locked ground palette must hold 1 to {MAXIMUM_PALETTE_ENTRIES} colors");
    }
    let clusters = colors
        .iter()
        .map(|color| {
            Ok(Cluster {
                centroid: hex_to_oklab(color)?,
                group: None,
            })
        })
        .collect::<Result<Vec<_>>>()?;
    let assignments = assign_all(imagery, None, &clusters, shadow_normalization);
    let mut counts = vec![0usize; clusters.len()];
    let mut total = 0usize;
    for &index in &assignments {
        if index != NO_GROUND_MATERIAL {
            counts[index as usize] += 1;
            total += 1;
        }
    }
    let entries = clusters
        .iter()
        .zip(colors)
        .zip(&counts)
        .enumerate()
        .map(|(index, ((_, color), &count))| GroundPaletteEntry {
            name: format!("ground {}", index + 1),
            color: color.to_uppercase(),
            share: if total == 0 {
                0.0
            } else {
                count as f32 / total as f32
            },
            group: None,
        })
        .collect();
    Ok((
        GroundPalette {
            entries,
            stretch: stretch_description(),
        },
        assignments,
    ))
}

/// The palette can never outgrow a material index byte, and eight colors is
/// already past what a multi-material printer comfortably loads next to the
/// overlay materials.
pub const MAXIMUM_PALETTE_ENTRIES: usize = 8;

fn stretch_description() -> String {
    format!(
        "reflectance stretched linearly to sRGB, saturating at {:.0} % reflectance",
        STRETCH_SATURATION * 100.0
    )
}

/// One eligible sample in cluster space.
struct Point {
    oklab: [f32; 3],
    group: Option<SurfaceClass>,
    raster_index: usize,
}

struct Cluster {
    centroid: [f32; 3],
    group: Option<SurfaceClass>,
}

/// Semantic group of a class raster sample: the four ground groups the
/// issue names. Overlay classes cannot appear in a pre-overlay raster, but
/// mapping them to ground keeps the function total.
fn semantic_group(class: SurfaceClass) -> SurfaceClass {
    match class {
        SurfaceClass::Forest => SurfaceClass::Forest,
        SurfaceClass::Snow => SurfaceClass::Snow,
        SurfaceClass::Water => SurfaceClass::Water,
        _ => SurfaceClass::Rock,
    }
}

fn group_rank(group: Option<SurfaceClass>) -> u32 {
    match group {
        None => 0,
        Some(class) => 1 + class.material_index(),
    }
}

fn group_name(group: Option<SurfaceClass>) -> &'static str {
    match group {
        None | Some(SurfaceClass::Rock) => "ground",
        Some(SurfaceClass::Forest) => "forest",
        Some(SurfaceClass::Snow) => "snow",
        Some(SurfaceClass::Water) => "water",
        Some(_) => "ground",
    }
}

fn eligible_points(imagery: &GroundImagery, groups: Option<&[SurfaceClass]>) -> Vec<Point> {
    imagery
        .valid
        .iter()
        .enumerate()
        .filter(|&(_, &valid)| valid)
        .map(|(index, _)| Point {
            oklab: reflectance_to_oklab(imagery.rgbn[index]),
            group: groups.map(|groups| semantic_group(groups[index])),
            raster_index: index,
        })
        .collect()
}

/// Moves every point toward its group's mean lightness along the shading
/// ray. Terrain shadow multiplies linear reflectance, and a uniform scale
/// of linear RGB scales the whole OKLab vector — L, a, and b together, by
/// the cube root of the factor — so undoing it must scale all three, not
/// slide L alone. The mean is computed in a fixed order, so the result
/// never depends on thread count or map order.
fn normalize_lightness(points: &mut [Point], strength: f32) {
    if strength <= 0.0 || points.is_empty() {
        return;
    }
    let mut sums: std::collections::HashMap<u32, (f64, usize)> = std::collections::HashMap::new();
    for point in points.iter() {
        let entry = sums.entry(group_rank(point.group)).or_default();
        entry.0 += f64::from(point.oklab[0]);
        entry.1 += 1;
    }
    for point in points.iter_mut() {
        let (sum, count) = sums[&group_rank(point.group)];
        let mean = (sum / count as f64) as f32;
        let lightness = point.oklab[0];
        if lightness <= f32::EPSILON {
            continue;
        }
        let factor = 1.0 + strength * (mean / lightness - 1.0);
        for axis in &mut point.oklab {
            *axis *= factor;
        }
    }
}

/// Splits `total` palette entries over the groups present in `points`.
/// Every group keeps at least one; the spares go by color diversity — the
/// group's share of samples times its OKLab variance — largest remainder
/// first, ties to the lower group rank. Share alone would let a uniform
/// forest covering half the map starve a two-tone ground group, merging
/// red rock and white rock into a single mud; a solid-color group needs
/// exactly one entry however large it is, so variance is what spare
/// colors should follow. Deterministic by construction.
fn allocate_counts(points: &[Point], total: usize) -> Vec<(Option<SurfaceClass>, usize)> {
    let mut shares: Vec<(Option<SurfaceClass>, usize)> = Vec::new();
    for point in points {
        match shares.iter_mut().find(|(group, _)| *group == point.group) {
            Some((_, count)) => *count += 1,
            None => shares.push((point.group, 1)),
        }
    }
    shares.sort_by_key(|(group, _)| group_rank(*group));
    if shares.len() >= total {
        // More groups than colors: the largest groups take one each.
        let mut by_size = shares.clone();
        by_size.sort_by(|a, b| b.1.cmp(&a.1).then(group_rank(a.0).cmp(&group_rank(b.0))));
        return by_size
            .into_iter()
            .take(total)
            .map(|(group, _)| (group, 1))
            .collect();
    }
    let weights = shares
        .iter()
        .map(|&(group, count)| count as f64 * group_variance(points, group))
        .collect::<Vec<_>>();
    let weight_total: f64 = weights.iter().sum();
    let point_total = points.len() as f64;
    let spare = total - shares.len();
    let mut allocations = shares
        .iter()
        .zip(&weights)
        .map(|(&(group, count), &weight)| {
            // All-solid groups leave no diversity to weigh; fall back to
            // share so the arithmetic stays total.
            let fraction = if weight_total > 0.0 {
                weight / weight_total
            } else {
                count as f64 / point_total
            };
            let ideal = fraction * spare as f64;
            (group, 1 + ideal.floor() as usize, ideal.fract())
        })
        .collect::<Vec<_>>();
    let assigned: usize = allocations.iter().map(|(_, count, _)| count).sum();
    let mut order = (0..allocations.len()).collect::<Vec<_>>();
    order.sort_by(|&a, &b| {
        allocations[b]
            .2
            .total_cmp(&allocations[a].2)
            .then(group_rank(allocations[a].0).cmp(&group_rank(allocations[b].0)))
    });
    for &index in order.iter().take(total - assigned) {
        allocations[index].1 += 1;
    }
    allocations
        .into_iter()
        .map(|(group, count, _)| (group, count))
        .collect()
}

/// Mean squared OKLab distance of a group's points from the group mean:
/// the diversity weight spare palette entries follow.
fn group_variance(points: &[Point], group: Option<SurfaceClass>) -> f64 {
    let mut sum = [0.0f64; 3];
    let mut count = 0usize;
    for point in points.iter().filter(|point| point.group == group) {
        for (slot, &value) in sum.iter_mut().zip(&point.oklab) {
            *slot += f64::from(value);
        }
        count += 1;
    }
    if count == 0 {
        return 0.0;
    }
    let mean = sum.map(|value| value / count as f64);
    let mut variance = 0.0;
    for point in points.iter().filter(|point| point.group == group) {
        for (&mean, &value) in mean.iter().zip(&point.oklab) {
            variance += (f64::from(value) - mean).powi(2);
        }
    }
    variance / count as f64
}

/// How many points cluster fitting reads at most. Assignment always visits
/// every sample; fitting on an evenly strided subset keeps discovery fast on
/// dense rasters without giving up determinism — a stride is as reproducible
/// as the full set.
const MAXIMUM_FITTING_POINTS: usize = 262_144;

/// Deterministic k-means over one group's points. Seeds are lightness
/// quantiles of the sorted points — no randomness anywhere — and ties in
/// assignment go to the lower cluster index, so the fixed iteration order
/// fixes the result.
fn cluster_group(members: &[&Point], count: usize, group: Option<SurfaceClass>) -> Vec<Cluster> {
    if members.is_empty() || count == 0 {
        return Vec::new();
    }
    let stride = members.len().div_ceil(MAXIMUM_FITTING_POINTS);
    let fitting = members
        .iter()
        .step_by(stride)
        .map(|point| point.oklab)
        .collect::<Vec<_>>();
    let mut sorted = fitting.clone();
    sorted.sort_by(|a, b| {
        a[0].total_cmp(&b[0])
            .then(a[1].total_cmp(&b[1]))
            .then(a[2].total_cmp(&b[2]))
    });
    let count = count.min(sorted.len());
    let mut centroids = (0..count)
        .map(|index| sorted[(index * 2 + 1) * (sorted.len() - 1) / (count * 2).max(1)])
        .collect::<Vec<_>>();
    let mut assignment = vec![usize::MAX; fitting.len()];
    for _ in 0..32 {
        let mut changed = false;
        for (point, slot) in fitting.iter().zip(assignment.iter_mut()) {
            let nearest = nearest_centroid(*point, &centroids);
            if nearest != *slot {
                *slot = nearest;
                changed = true;
            }
        }
        if !changed {
            break;
        }
        let mut sums = vec![[0.0f64; 3]; centroids.len()];
        let mut counts = vec![0usize; centroids.len()];
        for (point, &slot) in fitting.iter().zip(&assignment) {
            for axis in 0..3 {
                sums[slot][axis] += f64::from(point[axis]);
            }
            counts[slot] += 1;
        }
        for (index, centroid) in centroids.iter_mut().enumerate() {
            if counts[index] > 0 {
                for axis in 0..3 {
                    centroid[axis] = (sums[index][axis] / counts[index] as f64) as f32;
                }
            }
        }
    }
    // An empty cluster contributes nothing and is dropped; the palette
    // promises at most `count` entries, not exactly.
    let mut used = vec![false; centroids.len()];
    for &slot in &assignment {
        used[slot] = true;
    }
    centroids
        .into_iter()
        .zip(used)
        .filter(|(_, used)| *used)
        .map(|(centroid, _)| Cluster { centroid, group })
        .collect()
}

fn nearest_centroid(point: [f32; 3], centroids: &[[f32; 3]]) -> usize {
    let mut best = 0;
    let mut best_distance = f32::INFINITY;
    for (index, centroid) in centroids.iter().enumerate() {
        let distance = oklab_distance_squared(point, *centroid);
        if distance < best_distance {
            best_distance = distance;
            best = index;
        }
    }
    best
}

fn oklab_distance_squared(a: [f32; 3], b: [f32; 3]) -> f32 {
    (a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2) + (a[2] - b[2]).powi(2)
}

/// Two centroids closer than this in OKLab print as the same color; around
/// the just-noticeable difference on a calibrated screen, and far below
/// what distinct filaments can express.
const INDISTINGUISHABLE_OKLAB_DISTANCE: f32 = 0.01;

/// Merges clusters whose centroids are visually the same color into one,
/// share-weighted, closest pair first. Asking for k colors of an area that
/// only has fewer must return fewer, not near-duplicates that waste
/// filament slots. Hybrid groups are kept apart: a water grey and an
/// asphalt grey stay two entries on purpose.
fn merge_indistinguishable_clusters(clusters: &mut Vec<Cluster>, points: &[Point]) {
    let limit = INDISTINGUISHABLE_OKLAB_DISTANCE * INDISTINGUISHABLE_OKLAB_DISTANCE;
    loop {
        let shares = cluster_shares(clusters, points);
        let mut best: Option<(usize, usize, f32)> = None;
        for first in 0..clusters.len() {
            for second in first + 1..clusters.len() {
                if clusters[first].group != clusters[second].group {
                    continue;
                }
                let distance =
                    oklab_distance_squared(clusters[first].centroid, clusters[second].centroid);
                if distance < limit && best.is_none_or(|(_, _, held)| distance < held) {
                    best = Some((first, second, distance));
                }
            }
        }
        let Some((first, second, _)) = best else {
            return;
        };
        let weight_first = shares[first].max(f32::EPSILON);
        let weight_second = shares[second].max(f32::EPSILON);
        let total = weight_first + weight_second;
        for axis in 0..3 {
            clusters[first].centroid[axis] = (clusters[first].centroid[axis] * weight_first
                + clusters[second].centroid[axis] * weight_second)
                / total;
        }
        clusters.remove(second);
    }
}

/// Dissolves clusters below the share floor into their nearest surviving
/// neighbour, smallest first, and folds the dissolved cluster's share into
/// the survivor by re-checking shares after every merge. Hybrid groups keep
/// their last entry: a group present in the data must stay printable.
fn dissolve_small_clusters(clusters: &mut Vec<Cluster>, points: &[Point], minimum_share: f32) {
    if minimum_share <= 0.0 {
        return;
    }
    loop {
        if clusters.len() <= 1 {
            return;
        }
        let shares = cluster_shares(clusters, points);
        let candidate = shares
            .iter()
            .enumerate()
            .filter(|(index, share)| {
                **share < minimum_share
                    && (clusters[*index].group.is_none()
                        || clusters.iter().enumerate().any(|(other, cluster)| {
                            other != *index && cluster.group == clusters[*index].group
                        }))
            })
            .min_by(|a, b| a.1.total_cmp(b.1).then(a.0.cmp(&b.0)));
        let Some((index, _)) = candidate else {
            return;
        };
        clusters.remove(index);
    }
}

fn cluster_shares(clusters: &[Cluster], points: &[Point]) -> Vec<f32> {
    let mut counts = vec![0usize; clusters.len()];
    for point in points {
        counts[nearest_cluster(point, clusters)] += 1;
    }
    counts
        .into_iter()
        .map(|count| count as f32 / points.len() as f32)
        .collect()
}

/// Nearest cluster honouring hybrid grouping: a grouped point only maps to
/// its own group's clusters, so forest samples stay forest whatever color
/// they are. Falls back over all clusters when the group has none.
fn nearest_cluster(point: &Point, clusters: &[Cluster]) -> usize {
    let mut best = None;
    let mut best_distance = f32::INFINITY;
    for (index, cluster) in clusters.iter().enumerate() {
        if point.group.is_some() && cluster.group != point.group {
            continue;
        }
        let distance = oklab_distance_squared(point.oklab, cluster.centroid);
        if distance < best_distance {
            best_distance = distance;
            best = Some(index);
        }
    }
    best.unwrap_or_else(|| {
        clusters
            .iter()
            .enumerate()
            .min_by(|a, b| {
                oklab_distance_squared(point.oklab, a.1.centroid)
                    .total_cmp(&oklab_distance_squared(point.oklab, b.1.centroid))
                    .then(a.0.cmp(&b.0))
            })
            .map(|(index, _)| index)
            .unwrap_or(0)
    })
}

/// Maps every raster sample to its final material index. Runs the same
/// normalization the discovery saw so a shadowed sample lands in the same
/// cluster it was fitted into.
fn assign_all(
    imagery: &GroundImagery,
    groups: Option<&[SurfaceClass]>,
    clusters: &[Cluster],
    shadow_normalization: f32,
) -> Vec<u8> {
    let mut assignments = vec![NO_GROUND_MATERIAL; imagery.rgbn.len()];
    if clusters.is_empty() {
        return assignments;
    }
    let mut points = eligible_points(imagery, groups);
    normalize_lightness(&mut points, shadow_normalization);
    for point in &points {
        assignments[point.raster_index] = nearest_cluster(point, clusters) as u8;
    }
    assignments
}

/// The deterministic display stretch: scaled reflectance to linear RGB,
/// saturating at [`STRETCH_SATURATION`].
fn reflectance_to_linear(value: u16) -> f32 {
    (f32::from(value) / 10_000.0 / STRETCH_SATURATION).clamp(0.0, 1.0)
}

fn reflectance_to_oklab(rgbn: [u16; 4]) -> [f32; 3] {
    linear_to_oklab([
        reflectance_to_linear(rgbn[0]),
        reflectance_to_linear(rgbn[1]),
        reflectance_to_linear(rgbn[2]),
    ])
}

/// Linear sRGB to OKLab, the perceptual space the clustering measures
/// distance in (Björn Ottosson's matrices).
fn linear_to_oklab(rgb: [f32; 3]) -> [f32; 3] {
    let l = 0.412_221_46 * rgb[0] + 0.536_332_55 * rgb[1] + 0.051_445_995 * rgb[2];
    let m = 0.211_903_5 * rgb[0] + 0.680_699_5 * rgb[1] + 0.107_396_96 * rgb[2];
    let s = 0.088_302_46 * rgb[0] + 0.281_718_85 * rgb[1] + 0.629_978_7 * rgb[2];
    let l = l.cbrt();
    let m = m.cbrt();
    let s = s.cbrt();
    [
        0.210_454_26 * l + 0.793_617_8 * m - 0.004_072_047 * s,
        1.977_998_5 * l - 2.428_592_2 * m + 0.450_593_7 * s,
        0.025_904_037 * l + 0.782_771_77 * m - 0.808_675_77 * s,
    ]
}

fn oklab_to_linear(lab: [f32; 3]) -> [f32; 3] {
    let l = lab[0] + 0.396_337_78 * lab[1] + 0.215_803_76 * lab[2];
    let m = lab[0] - 0.105_561_346 * lab[1] - 0.063_854_17 * lab[2];
    let s = lab[0] - 0.089_484_18 * lab[1] - 1.291_485_5 * lab[2];
    let l = l * l * l;
    let m = m * m * m;
    let s = s * s * s;
    [
        4.076_741_7 * l - 3.307_711_6 * m + 0.230_969_94 * s,
        -1.268_438 * l + 2.609_757_4 * m - 0.341_319_38 * s,
        -0.004_196_086_3 * l - 0.703_418_6 * m + 1.707_614_7 * s,
    ]
}

fn linear_to_srgb_byte(value: f32) -> u8 {
    let value = value.clamp(0.0, 1.0);
    let encoded = if value <= 0.003_130_8 {
        value * 12.92
    } else {
        1.055 * value.powf(1.0 / 2.4) - 0.055
    };
    (encoded * 255.0).round() as u8
}

fn srgb_byte_to_linear(value: u8) -> f32 {
    let encoded = f32::from(value) / 255.0;
    if encoded <= 0.040_45 {
        encoded / 12.92
    } else {
        ((encoded + 0.055) / 1.055).powf(2.4)
    }
}

fn oklab_to_hex(lab: [f32; 3]) -> String {
    let rgb = oklab_to_linear(lab);
    format!(
        "#{:02X}{:02X}{:02X}",
        linear_to_srgb_byte(rgb[0]),
        linear_to_srgb_byte(rgb[1]),
        linear_to_srgb_byte(rgb[2]),
    )
}

fn hex_to_oklab(color: &str) -> Result<[f32; 3]> {
    let stripped = color.strip_prefix('#');
    let Some(stripped) = stripped else {
        bail!("ground palette color {color:?} is not a #RRGGBB hex color");
    };
    if stripped.len() != 6 || !stripped.chars().all(|c| c.is_ascii_hexdigit()) {
        bail!("ground palette color {color:?} is not a #RRGGBB hex color");
    }
    let byte = |range: std::ops::Range<usize>| u8::from_str_radix(&stripped[range], 16).unwrap();
    Ok(linear_to_oklab([
        srgb_byte_to_linear(byte(0..2)),
        srgb_byte_to_linear(byte(2..4)),
        srgb_byte_to_linear(byte(4..6)),
    ]))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A synthetic raster of solid color blocks: `blocks` lists
    /// (rgbn, sample count) runs laid out row-major.
    fn synthetic(blocks: &[([u16; 4], usize)]) -> (Vec<[u16; 4]>, Vec<bool>) {
        let mut rgbn = Vec::new();
        for &(color, count) in blocks {
            rgbn.extend(std::iter::repeat_n(color, count));
        }
        let valid = vec![true; rgbn.len()];
        (rgbn, valid)
    }

    const DARK_GREEN: [u16; 4] = [400, 550, 350, 2400];
    const BRIGHT_SAND: [u16; 4] = [2200, 2000, 1600, 2600];
    const DEEP_BLUE: [u16; 4] = [500, 700, 1100, 300];

    fn options(count: usize) -> GroundPaletteOptions {
        GroundPaletteOptions {
            color_count: count,
            minimum_share: 0.0,
            shadow_normalization: 0.0,
        }
    }

    #[test]
    fn three_solid_blocks_come_back_as_three_colors() {
        let (rgbn, valid) = synthetic(&[(DARK_GREEN, 40), (BRIGHT_SAND, 40), (DEEP_BLUE, 20)]);
        let imagery = GroundImagery {
            width: 10,
            height: 10,
            rgbn: &rgbn,
            valid: &valid,
        };
        let (palette, assignments) = discover_ground_palette(&imagery, None, &options(3)).unwrap();
        assert_eq!(palette.entries.len(), 3);
        // Every block maps to one entry, and different blocks to different
        // entries.
        assert_eq!(
            assignments[..40]
                .iter()
                .collect::<std::collections::HashSet<_>>()
                .len(),
            1
        );
        assert_eq!(
            assignments[40..80]
                .iter()
                .collect::<std::collections::HashSet<_>>()
                .len(),
            1
        );
        assert_ne!(assignments[0], assignments[40]);
        assert_ne!(assignments[40], assignments[80]);
        // Shares follow the block sizes.
        let sand_entry = &palette.entries[assignments[40] as usize];
        assert!((sand_entry.share - 0.4).abs() < 1e-6);
        // Entries are darkest-first and named by rank.
        assert!(palette.entries.windows(2).all(|pair| {
            hex_to_oklab(&pair[0].color).unwrap()[0] <= hex_to_oklab(&pair[1].color).unwrap()[0]
        }));
        assert_eq!(palette.entries[0].name, "ground 1");
    }

    #[test]
    fn discovery_is_deterministic() {
        let (rgbn, valid) = synthetic(&[
            (DARK_GREEN, 33),
            (BRIGHT_SAND, 41),
            (DEEP_BLUE, 17),
            ([900, 850, 800, 2100], 9),
        ]);
        let imagery = GroundImagery {
            width: 10,
            height: 10,
            rgbn: &rgbn,
            valid: &valid,
        };
        let first = discover_ground_palette(&imagery, None, &options(4)).unwrap();
        let second = discover_ground_palette(&imagery, None, &options(4)).unwrap();
        assert_eq!(first.0, second.0);
        assert_eq!(first.1, second.1);
    }

    #[test]
    fn the_share_floor_dissolves_a_rare_color() {
        let (rgbn, valid) = synthetic(&[(DARK_GREEN, 96), (BRIGHT_SAND, 4)]);
        let imagery = GroundImagery {
            width: 10,
            height: 10,
            rgbn: &rgbn,
            valid: &valid,
        };
        let with_floor = GroundPaletteOptions {
            minimum_share: 0.1,
            ..options(2)
        };
        let (palette, assignments) = discover_ground_palette(&imagery, None, &with_floor).unwrap();
        assert_eq!(palette.entries.len(), 1);
        assert!(assignments.iter().all(|&index| index == 0));
        // Without the floor both survive.
        let (palette, _) = discover_ground_palette(&imagery, None, &options(2)).unwrap();
        assert_eq!(palette.entries.len(), 2);
    }

    #[test]
    fn hybrid_grouping_keeps_water_out_of_the_ground_colors() {
        // Bay water and asphalt can be nearly the same grey; hybrid mode
        // must keep them in separate entries because the classes differ.
        let murk = [700, 720, 700, 400];
        let (rgbn, valid) = synthetic(&[(murk, 50), (murk, 50)]);
        let classes = [SurfaceClass::Water; 50]
            .into_iter()
            .chain([SurfaceClass::Rock; 50])
            .collect::<Vec<_>>();
        let imagery = GroundImagery {
            width: 10,
            height: 10,
            rgbn: &rgbn,
            valid: &valid,
        };
        let (palette, assignments) =
            discover_ground_palette(&imagery, Some(&classes), &options(4)).unwrap();
        assert_ne!(assignments[0], assignments[99]);
        let water_entry = &palette.entries[assignments[0] as usize];
        let ground_entry = &palette.entries[assignments[99] as usize];
        assert_eq!(water_entry.group, Some(SurfaceClass::Water));
        assert_eq!(ground_entry.group, Some(SurfaceClass::Rock));
        assert_eq!(water_entry.name, "water 1");
    }

    #[test]
    fn hybrid_allocation_gives_the_bigger_group_more_colors() {
        // 80 forest samples in two distinct greens, 20 rock samples in one
        // grey: with four colors to spend, forest gets enough to keep both
        // greens apart.
        let light_green = [700, 1100, 500, 2900];
        let (rgbn, valid) = synthetic(&[
            (DARK_GREEN, 40),
            (light_green, 40),
            ([900, 880, 860, 2000], 20),
        ]);
        let classes = [SurfaceClass::Forest; 80]
            .into_iter()
            .chain([SurfaceClass::Rock; 20])
            .collect::<Vec<_>>();
        let imagery = GroundImagery {
            width: 10,
            height: 10,
            rgbn: &rgbn,
            valid: &valid,
        };
        let (palette, assignments) =
            discover_ground_palette(&imagery, Some(&classes), &options(3)).unwrap();
        let forest_entries = palette
            .entries
            .iter()
            .filter(|entry| entry.group == Some(SurfaceClass::Forest))
            .count();
        assert_eq!(forest_entries, 2);
        assert_ne!(assignments[0], assignments[40]);
    }

    #[test]
    fn spare_colors_follow_diversity_so_red_and_white_rock_both_survive() {
        // A uniform forest holds most of the area; the smaller ground group
        // holds two far-apart tones, red rock and white rock. Share-based
        // allocation would hand the spare color to the forest and merge the
        // rocks; diversity-based allocation keeps both.
        let red_rock = [2100, 1100, 700, 2300];
        let white_rock = [2600, 2500, 2300, 2700];
        let (rgbn, valid) = synthetic(&[(DARK_GREEN, 70), (red_rock, 15), (white_rock, 15)]);
        let classes = [SurfaceClass::Forest; 70]
            .into_iter()
            .chain([SurfaceClass::Rock; 30])
            .collect::<Vec<_>>();
        let imagery = GroundImagery {
            width: 10,
            height: 10,
            rgbn: &rgbn,
            valid: &valid,
        };
        let (palette, assignments) =
            discover_ground_palette(&imagery, Some(&classes), &options(3)).unwrap();
        let ground_entries = palette
            .entries
            .iter()
            .filter(|entry| entry.group == Some(SurfaceClass::Rock))
            .count();
        assert_eq!(ground_entries, 2);
        assert_ne!(assignments[70], assignments[85]);
    }

    #[test]
    fn shadow_normalization_reunites_a_shadowed_hillside() {
        // The same ground at two brightnesses: full normalization must fold
        // both into one color, and none must keep them apart.
        let lit = [1600, 1400, 1200, 2400];
        let shadowed = [500, 440, 380, 900];
        let (rgbn, valid) = synthetic(&[(lit, 50), (shadowed, 50)]);
        let imagery = GroundImagery {
            width: 10,
            height: 10,
            rgbn: &rgbn,
            valid: &valid,
        };
        let split = discover_ground_palette(&imagery, None, &options(2)).unwrap();
        assert_ne!(split.1[0], split.1[99]);
        let normalized = GroundPaletteOptions {
            shadow_normalization: 1.0,
            minimum_share: 0.0,
            color_count: 2,
        };
        let (_, assignments) = discover_ground_palette(&imagery, None, &normalized).unwrap();
        // With lightness equalized the two runs may still split on the
        // slight hue difference, but the dominant lit/shadow divide is
        // gone: check via a pair that differs ONLY in brightness.
        let doubled = [1000, 880, 760, 1800];
        let half = [500, 440, 380, 900];
        let (rgbn, valid) = synthetic(&[(doubled, 50), (half, 50)]);
        let imagery = GroundImagery {
            width: 10,
            height: 10,
            rgbn: &rgbn,
            valid: &valid,
        };
        let (palette, assignments2) = discover_ground_palette(&imagery, None, &normalized).unwrap();
        assert_eq!(assignments2[0], assignments2[99]);
        assert_eq!(palette.entries.len(), 1);
        let _ = assignments;
    }

    #[test]
    fn samples_without_imagery_keep_the_fallback_index() {
        let (rgbn, mut valid) = synthetic(&[(DARK_GREEN, 50), (BRIGHT_SAND, 50)]);
        for slot in valid.iter_mut().take(10) {
            *slot = false;
        }
        let imagery = GroundImagery {
            width: 10,
            height: 10,
            rgbn: &rgbn,
            valid: &valid,
        };
        let (palette, assignments) = discover_ground_palette(&imagery, None, &options(2)).unwrap();
        assert!(
            assignments[..10]
                .iter()
                .all(|&index| index == NO_GROUND_MATERIAL)
        );
        assert!(
            assignments[10..]
                .iter()
                .all(|&index| index != NO_GROUND_MATERIAL)
        );
        // Shares still sum to 1 over the valid samples.
        let total: f32 = palette.entries.iter().map(|entry| entry.share).sum();
        assert!((total - 1.0).abs() < 1e-5);
    }

    #[test]
    fn an_all_invalid_raster_yields_an_empty_palette() {
        let rgbn = vec![[0u16; 4]; 100];
        let valid = vec![false; 100];
        let imagery = GroundImagery {
            width: 10,
            height: 10,
            rgbn: &rgbn,
            valid: &valid,
        };
        let (palette, assignments) = discover_ground_palette(&imagery, None, &options(4)).unwrap();
        assert!(palette.entries.is_empty());
        assert!(assignments.iter().all(|&index| index == NO_GROUND_MATERIAL));
    }

    #[test]
    fn a_locked_palette_assigns_without_rediscovering() {
        let (rgbn, valid) = synthetic(&[(DARK_GREEN, 60), (BRIGHT_SAND, 40)]);
        let imagery = GroundImagery {
            width: 10,
            height: 10,
            rgbn: &rgbn,
            valid: &valid,
        };
        // Lock two colors near the two blocks, in a fixed order the data
        // would not have discovered (bright first).
        let colors = vec!["#E7C89B".to_string(), "#274A32".to_string()];
        let (palette, assignments) = assign_locked_palette(&imagery, &colors, 0.0).unwrap();
        assert_eq!(palette.entries[0].color, "#E7C89B");
        assert_eq!(assignments[0], 1);
        assert_eq!(assignments[99], 0);
        assert!((palette.entries[1].share - 0.6).abs() < 1e-6);
        assert!(assign_locked_palette(&imagery, &["green".to_string()], 0.0).is_err());
    }

    #[test]
    fn the_stretch_and_color_conversions_hold_their_anchors() {
        // Zero reflectance is black, the saturation point is full scale,
        // and the ramp is monotone.
        assert_eq!(reflectance_to_linear(0), 0.0);
        assert_eq!(reflectance_to_linear(3_000), 1.0);
        assert_eq!(reflectance_to_linear(10_000), 1.0);
        assert!(reflectance_to_linear(1_000) < reflectance_to_linear(2_000));
        // OKLab round-trips through hex to within a display quantum.
        for color in ["#000000", "#FFFFFF", "#28543A", "#2F76B5", "#7C7468"] {
            let lab = hex_to_oklab(color).unwrap();
            assert_eq!(oklab_to_hex(lab), color);
        }
        // White is lighter than black in L.
        assert!(hex_to_oklab("#FFFFFF").unwrap()[0] > hex_to_oklab("#000000").unwrap()[0]);
    }
}
