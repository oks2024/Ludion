//! The opt-in write journal behind [`World::watch_with`](crate::World::watch_with).
//!
//! A *marker* component type `M` selects which entities are watched: those carrying `M`. Every
//! mutable access to a watched entity sets a dirty bit for that marker; a drain takes the bits.
//!
//! Invariant: `watched` and `dirty` bits only ever name live slots. A despawn or a restore moves
//! the bits into the marker's `gone` list as the old handle, generation included, so a consumer
//! can evict an entity whose slot was recycled. Nothing here is hashed, snapshotted or replayed.

use std::any::TypeId;

use ludion_core::ids::EntityId;

use crate::entity::PRESENTATION_BASE;

const RANGE_SHIFT: u32 = PRESENTATION_BASE.trailing_zeros();
const OFFSET_MASK: u32 = PRESENTATION_BASE - 1;

/// A bit per entity index, one word array per id range (sim, presentation) like the storages'
/// slot tables, so a presentation index never sizes the sim array to 2^24 bits.
#[derive(Default)]
pub(crate) struct SlotBits {
    ranges: [Vec<u64>; 2],
}

impl SlotBits {
    #[inline]
    fn locate(index: u32) -> (usize, usize, u64) {
        let offset = index & OFFSET_MASK;
        (
            (index >> RANGE_SHIFT) as usize,
            (offset / 64) as usize,
            1u64 << (offset % 64),
        )
    }

    /// Whether `index` is set; an index past what was ever set (or past both ranges) is `false`.
    #[inline]
    pub(crate) fn test(&self, index: u32) -> bool {
        let (range, word, mask) = Self::locate(index);
        self.ranges
            .get(range)
            .and_then(|words| words.get(word))
            .is_some_and(|bits| bits & mask != 0)
    }

    pub(crate) fn set(&mut self, index: u32) {
        let (range, word, mask) = Self::locate(index);
        let words = &mut self.ranges[range];
        if words.len() <= word {
            words.resize(word + 1, 0);
        }
        words[word] |= mask;
    }

    pub(crate) fn clear(&mut self, index: u32) {
        let (range, word, mask) = Self::locate(index);
        if let Some(bits) = self.ranges.get_mut(range).and_then(|w| w.get_mut(word)) {
            *bits &= !mask;
        }
    }

    pub(crate) fn take(&mut self) -> Self {
        std::mem::take(self)
    }

    /// The set indices in ascending absolute order.
    pub(crate) fn ones(&self) -> impl Iterator<Item = u32> + '_ {
        self.ranges.iter().enumerate().flat_map(|(range, words)| {
            words.iter().enumerate().flat_map(move |(word, &bits)| {
                let base = ((range as u32) << RANGE_SHIFT) + word as u32 * 64;
                let mut rest = bits;
                std::iter::from_fn(move || {
                    if rest == 0 {
                        return None;
                    }
                    let bit = rest.trailing_zeros();
                    rest &= rest - 1;
                    Some(base + bit)
                })
            })
        })
    }
}

/// One watched marker type's state.
struct Marker {
    type_id: TypeId,
    /// The live entities carrying the marker.
    watched: SlotBits,
    /// Watched (or just-unwatched) live entities written since the last drain.
    dirty: SlotBits,
    /// Handles that died or were forgotten by a restore while watched or dirty.
    gone: Vec<EntityId>,
}

/// Every marker's state, plus the union of their `watched` bits so the hot path is one test.
#[derive(Default)]
pub(crate) struct WriteJournal {
    any: SlotBits,
    markers: Vec<Marker>,
}

impl WriteJournal {
    #[inline]
    pub(crate) fn is_armed(&self) -> bool {
        !self.markers.is_empty()
    }

    /// Whether any marker watches the entity at `index`: the one-bit hot-path test.
    #[inline]
    pub(crate) fn watches_any(&self, index: u32) -> bool {
        self.any.test(index)
    }

    /// Whether the marker `type_id` currently watches the entity at `index`.
    pub(crate) fn watches(&self, type_id: TypeId, index: u32) -> bool {
        self.markers
            .iter()
            .any(|m| m.type_id == type_id && m.watched.test(index))
    }

    /// Registers `type_id` (idempotent), seeding every existing carrier (`existing_keys`) as
    /// watched and dirty, so the first drain returns all of them.
    pub(crate) fn watch(&mut self, type_id: TypeId, existing_keys: &[u32]) {
        if self.markers.iter().any(|m| m.type_id == type_id) {
            return;
        }
        let mut marker = Marker {
            type_id,
            watched: SlotBits::default(),
            dirty: SlotBits::default(),
            gone: Vec::new(),
        };
        for &key in existing_keys {
            marker.watched.set(key);
            marker.dirty.set(key);
            self.any.set(key);
        }
        self.markers.push(marker);
    }

    /// Marks the entity at `index` dirty for every marker watching it.
    #[inline]
    pub(crate) fn touch(&mut self, index: u32) {
        if self.any.test(index) {
            self.touch_watched(index);
        }
    }

    fn touch_watched(&mut self, index: u32) {
        for marker in &mut self.markers {
            if marker.watched.test(index) {
                marker.dirty.set(index);
            }
        }
    }

    /// Appends to `out` the indices of `keys` that some marker watches.
    pub(crate) fn collect_watched(&self, keys: &[u32], out: &mut Vec<u32>) {
        out.extend(keys.iter().copied().filter(|&k| self.any.test(k)));
    }

    pub(crate) fn touch_all(&mut self, indices: &[u32]) {
        for &index in indices {
            self.touch(index);
        }
    }

    /// A component of type `type_id` was inserted at `index`; the marker itself starts watching.
    pub(crate) fn on_insert(&mut self, type_id: TypeId, index: u32) {
        if let Some(marker) = self.markers.iter_mut().find(|m| m.type_id == type_id) {
            marker.watched.set(index);
            self.any.set(index);
        }
        self.touch(index);
    }

    /// A component of type `type_id` was removed at `index`; losing the marker stops watching
    /// after the removal itself is journaled.
    pub(crate) fn on_remove(&mut self, type_id: TypeId, index: u32) {
        self.touch(index);
        let Some(marker) = self.markers.iter_mut().find(|m| m.type_id == type_id) else {
            return;
        };
        marker.watched.clear(index);
        let still = self.markers.iter().any(|m| m.watched.test(index));
        if !still {
            self.any.clear(index);
        }
    }

    /// The live entity `id` was freed.
    pub(crate) fn on_despawn(&mut self, id: EntityId) {
        for marker in &mut self.markers {
            if marker.watched.test(id.index) || marker.dirty.test(id.index) {
                marker.gone.push(id);
                marker.watched.clear(id.index);
                marker.dirty.clear(id.index);
            }
        }
        self.any.clear(id.index);
    }

    /// A restore is about to replace the allocators: every watched or dirty slot becomes a `gone`
    /// handle (resolved while the old allocators are still in place). Registrations survive.
    pub(crate) fn forget_live(&mut self, resolve: impl Fn(u32) -> EntityId) {
        for marker in &mut self.markers {
            let dirty_only = marker.dirty.ones().filter(|&i| !marker.watched.test(i));
            let ids: Vec<EntityId> = marker
                .watched
                .ones()
                .chain(dirty_only)
                .map(&resolve)
                .collect();
            marker.gone.extend(ids);
            marker.watched = SlotBits::default();
            marker.dirty = SlotBits::default();
        }
        self.any = SlotBits::default();
    }

    /// Takes the dirty bits and the `gone` list of `type_id`; `None` if it is not watched.
    pub(crate) fn take_marker(&mut self, type_id: TypeId) -> Option<(SlotBits, Vec<EntityId>)> {
        let marker = self.markers.iter_mut().find(|m| m.type_id == type_id)?;
        Some((marker.dirty.take(), std::mem::take(&mut marker.gone)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bits_span_both_ranges_and_never_grow_on_test() {
        let mut bits = SlotBits::default();
        assert!(bits.ones().next().is_none());
        let far = PRESENTATION_BASE + 130;
        assert!(!bits.test(far));
        assert!(bits.ranges[1].is_empty(), "a test must not grow the table");
        for index in [0, 63, 64, 5000, PRESENTATION_BASE, far] {
            bits.set(index);
            assert!(bits.test(index));
        }
        assert_eq!(
            bits.ones().collect::<Vec<_>>(),
            vec![0, 63, 64, 5000, PRESENTATION_BASE, far]
        );
        bits.clear(64);
        bits.clear(far);
        assert!(!bits.test(64) && !bits.test(far));
        let taken = bits.take();
        assert!(bits.ones().next().is_none() && taken.ones().next().is_some());
        assert!(!bits.test(5000));
    }
}
