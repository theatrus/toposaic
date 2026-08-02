//! Small process-wide cache for height-independent planar geometry.
//!
//! Live preview and export rebuild their surface fields from the same cached
//! source tiles. The vector fingerprint and exact piece frame let those two
//! passes share expensive polygon clips without ever sharing terrain samples,
//! roof heights, bridge heights, or final mesh triangles.

use std::{
    collections::{HashMap, VecDeque},
    sync::{Arc, Mutex, OnceLock},
};

use geo::{MultiPolygon, Polygon};

use crate::surface::SurfaceField;

const MAX_CACHE_BYTES: usize = 256 * 1024 * 1024;
const ORDER_COMPACTION_MULTIPLIER: usize = 4;
const ORDER_COMPACTION_SLACK: usize = 64;
const FNV_OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
const FNV_PRIME: u64 = 0x0000_0100_0000_01b3;

#[derive(Debug, Clone)]
pub(super) struct PreparedBuildingClip {
    pub area_index: usize,
    pub footprint: MultiPolygon<f64>,
    pub bounds: [f32; 4],
}

#[derive(Debug, Clone)]
pub(super) struct PreparedBuildings {
    pub candidate_count: usize,
    pub clips: Vec<PreparedBuildingClip>,
    pub union: MultiPolygon<f64>,
    pub overlay_obstacles: MultiPolygon<f64>,
}

#[derive(Debug, Clone)]
pub(super) struct PreparedRibbons {
    pub clips: HashMap<u64, MultiPolygon<f64>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(super) struct PieceCacheKey {
    surface: u64,
    piece: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum CacheKey {
    Buildings(PieceCacheKey, u32),
    Ribbons(PieceCacheKey),
}

enum CacheValue {
    Buildings(Arc<PreparedBuildings>),
    Ribbons(Arc<PreparedRibbons>),
}

struct CacheEntry {
    value: CacheValue,
    bytes: usize,
    touched: u64,
}

#[derive(Default)]
struct PreparedGeometryCache {
    entries: HashMap<CacheKey, CacheEntry>,
    order: VecDeque<(u64, CacheKey)>,
    bytes: usize,
    clock: u64,
}

impl PreparedGeometryCache {
    fn compact_order_if_needed(&mut self) {
        let maximum_records = self
            .entries
            .len()
            .saturating_mul(ORDER_COMPACTION_MULTIPLIER)
            .saturating_add(ORDER_COMPACTION_SLACK);
        if self.order.len() <= maximum_records {
            return;
        }
        let entries = &self.entries;
        self.order.retain(|(stamp, key)| {
            entries
                .get(key)
                .is_some_and(|entry| entry.touched == *stamp)
        });
    }

    fn touch(&mut self, key: CacheKey) {
        self.clock = self.clock.wrapping_add(1);
        let touched = self.clock;
        if let Some(entry) = self.entries.get_mut(&key) {
            entry.touched = touched;
            self.order.push_back((touched, key));
        }
        self.compact_order_if_needed();
    }

    fn insert(&mut self, key: CacheKey, value: CacheValue, bytes: usize) {
        if bytes > MAX_CACHE_BYTES {
            return;
        }
        if let Some(old) = self.entries.remove(&key) {
            self.bytes = self.bytes.saturating_sub(old.bytes);
        }
        self.clock = self.clock.wrapping_add(1);
        let touched = self.clock;
        self.bytes = self.bytes.saturating_add(bytes);
        self.entries.insert(
            key,
            CacheEntry {
                value,
                bytes,
                touched,
            },
        );
        self.order.push_back((touched, key));
        self.compact_order_if_needed();
        while self.bytes > MAX_CACHE_BYTES {
            let Some((stamp, oldest)) = self.order.pop_front() else {
                break;
            };
            if self
                .entries
                .get(&oldest)
                .is_some_and(|entry| entry.touched == stamp)
                && let Some(entry) = self.entries.remove(&oldest)
            {
                self.bytes = self.bytes.saturating_sub(entry.bytes);
            }
        }
    }
}

fn cache() -> &'static Mutex<PreparedGeometryCache> {
    static CACHE: OnceLock<Mutex<PreparedGeometryCache>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(PreparedGeometryCache::default()))
}

pub(super) fn piece_cache_key(
    surface: &SurfaceField,
    outline: &[[f32; 2]],
    origin_x: f32,
    origin_y: f32,
    assembled_width: f32,
    assembled_height: f32,
) -> PieceCacheKey {
    let mut piece = FNV_OFFSET;
    for value in [origin_x, origin_y, assembled_width, assembled_height] {
        hash_u32(&mut piece, value.to_bits());
    }
    hash_u64(&mut piece, outline.len() as u64);
    for point in outline {
        hash_u32(&mut piece, point[0].to_bits());
        hash_u32(&mut piece, point[1].to_bits());
    }
    PieceCacheKey {
        surface: surface.vector_fingerprint(),
        piece,
    }
}

pub(super) fn get_buildings(
    key: PieceCacheKey,
    minimum_span_mm: Option<f32>,
) -> Option<Arc<PreparedBuildings>> {
    let key = CacheKey::Buildings(key, minimum_span_mm.map_or(u32::MAX, f32::to_bits));
    let mut cache = cache()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let value = match cache.entries.get(&key) {
        Some(CacheEntry {
            value: CacheValue::Buildings(value),
            ..
        }) => Some(Arc::clone(value)),
        _ => None,
    };
    if value.is_some() {
        cache.touch(key);
    }
    value
}

pub(super) fn put_buildings(
    key: PieceCacheKey,
    minimum_span_mm: Option<f32>,
    value: Arc<PreparedBuildings>,
) {
    let key = CacheKey::Buildings(key, minimum_span_mm.map_or(u32::MAX, f32::to_bits));
    let bytes = prepared_buildings_bytes(&value);
    cache()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .insert(key, CacheValue::Buildings(value), bytes);
}

pub(super) fn get_ribbons(key: PieceCacheKey) -> Option<Arc<PreparedRibbons>> {
    let key = CacheKey::Ribbons(key);
    let mut cache = cache()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let value = match cache.entries.get(&key) {
        Some(CacheEntry {
            value: CacheValue::Ribbons(value),
            ..
        }) => Some(Arc::clone(value)),
        _ => None,
    };
    if value.is_some() {
        cache.touch(key);
    }
    value
}

pub(super) fn put_ribbons(key: PieceCacheKey, value: Arc<PreparedRibbons>) {
    let bytes = value
        .clips
        .values()
        .map(multi_polygon_bytes)
        .sum::<usize>()
        .saturating_add(value.clips.len() * 48);
    cache()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .insert(key.into(), CacheValue::Ribbons(value), bytes);
}

impl From<PieceCacheKey> for CacheKey {
    fn from(value: PieceCacheKey) -> Self {
        Self::Ribbons(value)
    }
}

fn prepared_buildings_bytes(value: &PreparedBuildings) -> usize {
    value
        .clips
        .iter()
        .map(|clip| multi_polygon_bytes(&clip.footprint).saturating_add(48))
        .sum::<usize>()
        .saturating_add(multi_polygon_bytes(&value.union))
        .saturating_add(multi_polygon_bytes(&value.overlay_obstacles))
}

fn multi_polygon_bytes(value: &MultiPolygon<f64>) -> usize {
    value
        .0
        .iter()
        .map(polygon_bytes)
        .sum::<usize>()
        .saturating_add(value.0.len() * 24)
}

fn polygon_bytes(value: &Polygon<f64>) -> usize {
    let coordinates = value.exterior().0.len()
        + value
            .interiors()
            .iter()
            .map(|ring| ring.0.len())
            .sum::<usize>();
    coordinates.saturating_mul(std::mem::size_of::<geo::Coord<f64>>())
}

fn hash_u32(state: &mut u64, value: u32) {
    hash_bytes(state, &value.to_le_bytes());
}

fn hash_u64(state: &mut u64, value: u64) {
    hash_bytes(state, &value.to_le_bytes());
}

fn hash_bytes(state: &mut u64, bytes: &[u8]) {
    for byte in bytes {
        *state ^= u64::from(*byte);
        *state = state.wrapping_mul(FNV_PRIME);
    }
}

#[cfg(test)]
pub(super) fn clear_for_tests() {
    *cache()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner()) = PreparedGeometryCache::default();
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::spec::SurfaceClass;

    #[test]
    fn cached_ribbons_require_the_same_surface_and_piece_frame() {
        clear_for_tests();
        let field = SurfaceField::new(2, 2, vec![SurfaceClass::Rock; 4], "cache").unwrap();
        let outline = [[0.0, 0.0], [20.0, 0.0], [20.0, 20.0], [0.0, 20.0]];
        let key = piece_cache_key(&field, &outline, 0.0, 0.0, 20.0, 20.0);
        let value = Arc::new(PreparedRibbons {
            clips: HashMap::new(),
        });
        put_ribbons(key, Arc::clone(&value));
        assert!(get_ribbons(key).is_some());

        let moved = piece_cache_key(&field, &outline, 20.0, 0.0, 40.0, 20.0);
        assert!(get_ribbons(moved).is_none());

        let mut changed = SurfaceField::new(2, 2, vec![SurfaceClass::Rock; 4], "cache").unwrap();
        changed.paint_polyline(&[[0.0, 0.0], [1.0, 1.0]], 20.0, 0.7, SurfaceClass::Road);
        let changed = piece_cache_key(&changed, &outline, 0.0, 0.0, 20.0, 20.0);
        assert!(get_ribbons(changed).is_none());
        clear_for_tests();
    }

    #[test]
    fn repeated_cache_hits_keep_lru_metadata_bounded() {
        let field = SurfaceField::new(2, 2, vec![SurfaceClass::Rock; 4], "cache").unwrap();
        let outline = [[0.0, 0.0], [20.0, 0.0], [20.0, 20.0], [0.0, 20.0]];
        let key = piece_cache_key(&field, &outline, 0.0, 0.0, 20.0, 20.0);
        let cache_key = CacheKey::Ribbons(key);
        let mut cache = PreparedGeometryCache::default();
        cache.insert(
            cache_key,
            CacheValue::Ribbons(Arc::new(PreparedRibbons {
                clips: HashMap::new(),
            })),
            0,
        );

        for _ in 0..10_000 {
            cache.touch(cache_key);
        }

        assert!(
            cache.order.len()
                <= cache
                    .entries
                    .len()
                    .saturating_mul(ORDER_COMPACTION_MULTIPLIER)
                    .saturating_add(ORDER_COMPACTION_SLACK),
            "{} LRU records for {} live entries",
            cache.order.len(),
            cache.entries.len()
        );
    }
}
