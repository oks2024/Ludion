//! Per-component storage.
//!
//! Each component type `T` gets its own [`Storage<T>`]: two parallel dense `Vec`s — `keys` (entity
//! indices) and `values` — held under one invariant, **`keys` is strictly ascending and the same
//! length as `values`**. Row `i` is `(keys[i], values[i])`. Because the keys are sorted, *iterating a
//! storage visits entities in ascending index order for free*: both the determinism contract and the
//! sorted merge in [`query`](crate::query) depend on it, and the merge reads the two columns as plain
//! slices rather than driving a tree cursor. There is no archetype table and no `unsafe`.
//!
//! Random-key lookup does not search the columns: a third `Vec`, `slots`, indexes them directly —
//! `slots[entity_index]` is the row holding that entity, or `SENTINEL` when it has none. Entity
//! indices are dense within each id range (the allocator reuses freed slots LIFO, so an index space
//! is bounded by peak live entities), which makes the table one predictable load instead of a binary
//! search's ~log n scattered probes — the difference between an O(1) and an O(log n) `get` shows up
//! directly in per-entity systems and the world-hash walk, which do millions of random lookups per
//! frame. The table is split per id range (sim, presentation — [`PRESENTATION_BASE`]), so a
//! presentation entity never makes a type pay for the gap below its range. The cost is 4 bytes per
//! peak entity index *of its range* per component type.
//!
//! Insert appends when the key is above the last one (the ascending order scene load and spawning
//! produce, so bulk construction is O(1) amortized per row) and otherwise splices into position;
//! remove splices out. A splice shifts rows, so it also repairs the shifted keys' slot entries —
//! the same O(n) class the memmove already pays. That memmove is why the layout is worth it
//! everywhere else: reads and iteration touch contiguous memory.
//!
//! Storages are held type-erased in the [`World`](crate::World) as `Box<dyn AnyStorage>` keyed by
//! `TypeId`; the [`AnyStorage`] trait exposes just enough to downcast back to the concrete
//! `Storage<T>` and to drop an entity's row on despawn without knowing `T`.

use crate::entity::PRESENTATION_BASE;
use std::any::Any;

/// Type-erased handle to a component storage.
///
/// This is a hidden implementation detail exposed only so the macro-generated query machinery in a
/// sibling module can name it; it is not part of the stable public surface.
#[doc(hidden)]
pub trait AnyStorage: Any + Send + Sync {
    /// Upcast to `&dyn Any` for downcasting to the concrete `Storage<T>`.
    fn as_any(&self) -> &dyn Any;
    /// Upcast to `&mut dyn Any` for downcasting to the concrete `Storage<T>`.
    fn as_any_mut(&mut self) -> &mut dyn Any;
    /// Removes the row for `entity_index` if present; used by despawn, which does not know `T`.
    fn remove_entity(&mut self, entity_index: u32) -> bool;
    /// Removes the rows of all `entity_indices` in one pass; used by batch despawn.
    fn remove_entities(&mut self, entity_indices: &[u32]);
    /// The entity indices holding a row, in ascending order (the storage's iteration contract).
    fn entity_indices(&self) -> Box<dyn Iterator<Item = u32> + '_>;
    /// The Rust type name of the stored component, for diagnostics.
    fn type_name(&self) -> &'static str;
    /// Whether the storage holds no rows (it persists after its last row is removed).
    fn is_empty(&self) -> bool;
}

/// Row position standing for "this entity index has no row".
const SENTINEL: u32 = u32::MAX;

/// Bits of an entity index below its range number; the shift that splits an index into
/// `(range, offset)`.
const RANGE_SHIFT: u32 = PRESENTATION_BASE.trailing_zeros();
const _: () = assert!(PRESENTATION_BASE.is_power_of_two());

/// The slot-table entry of `entity_index`, if its table reaches that far. Indices beyond the two id
/// ranges never exist and read as absent.
#[inline]
fn slot_of(slots: &[Vec<u32>; 2], entity_index: u32) -> Option<&u32> {
    slots
        .get((entity_index >> RANGE_SHIFT) as usize)?
        .get((entity_index & (PRESENTATION_BASE - 1)) as usize)
}

/// The slot-table entry of a row that exists (its table already covers it).
#[inline]
fn slot_mut(slots: &mut [Vec<u32>; 2], entity_index: u32) -> &mut u32 {
    &mut slots[(entity_index >> RANGE_SHIFT) as usize]
        [(entity_index & (PRESENTATION_BASE - 1)) as usize]
}

/// Storage for a single component type: sorted dense columns from entity index to value, plus a
/// direct slot table from entity index to row.
///
/// Invariants, all preserved by every method here — nothing outside this module can break them,
/// because the fields are private and the slice accessors hand out the columns without the means to
/// reorder them:
///
/// - `keys` is strictly ascending and `keys.len() == values.len()`.
/// - `slots[range][offset] == r` for every row `r` whose key splits into `(range, offset)`, and
///   every other entry is [`SENTINEL`].
pub(crate) struct Storage<T> {
    keys: Vec<u32>,
    values: Vec<T>,
    /// Per id range (sim, presentation), the entity's row in `keys`/`values` at its offset in the
    /// range, or [`SENTINEL`] when absent. Each table is sized to the highest offset ever inserted
    /// in its range, so 4 bytes per peak entity index of that range.
    slots: [Vec<u32>; 2],
}

impl<T> Storage<T> {
    /// Creates an empty storage.
    pub(crate) fn new() -> Self {
        Storage {
            keys: Vec::new(),
            values: Vec::new(),
            slots: [Vec::new(), Vec::new()],
        }
    }

    /// The row index of `entity_index`, if it has one — one load, no search.
    #[inline]
    fn row(&self, entity_index: u32) -> Option<usize> {
        match slot_of(&self.slots, entity_index) {
            Some(&row) if row != SENTINEL => Some(row as usize),
            _ => None,
        }
    }

    /// Points `entity_index` at `row`, growing the table with sentinels to cover it.
    #[inline]
    fn set_row(&mut self, entity_index: u32, row: usize) {
        let table = &mut self.slots[(entity_index >> RANGE_SHIFT) as usize];
        let offset = (entity_index & (PRESENTATION_BASE - 1)) as usize;
        if offset >= table.len() {
            table.resize(offset + 1, SENTINEL);
        }
        table[offset] = row as u32;
    }

    /// Repairs the slot entries of rows `from..`, whose positions a splice has just shifted.
    fn repair_rows_from(&mut self, from: usize) {
        for row in from..self.keys.len() {
            *slot_mut(&mut self.slots, self.keys[row]) = row as u32;
        }
    }

    /// The value for `entity_index`, if it has a row.
    #[inline]
    pub(crate) fn get(&self, entity_index: u32) -> Option<&T> {
        self.row(entity_index).map(|i| &self.values[i])
    }

    /// The value for `entity_index` mutably, if it has a row.
    #[inline]
    pub(crate) fn get_mut(&mut self, entity_index: u32) -> Option<&mut T> {
        match self.row(entity_index) {
            Some(i) => Some(&mut self.values[i]),
            None => None,
        }
    }

    /// Whether `entity_index` has a row.
    #[inline]
    pub(crate) fn contains(&self, entity_index: u32) -> bool {
        self.row(entity_index).is_some()
    }

    /// Inserts (or replaces) the value for `entity_index`, returning the previous one if there was
    /// a row. A key above every existing one appends — the path a scene load and ascending spawn
    /// order take, which keeps bulk construction linear.
    pub(crate) fn insert(&mut self, entity_index: u32, value: T) -> Option<T> {
        if let Some(i) = self.row(entity_index) {
            return Some(std::mem::replace(&mut self.values[i], value));
        }
        if self.keys.last().is_none_or(|&last| entity_index > last) {
            let row = self.keys.len();
            self.keys.push(entity_index);
            self.values.push(value);
            self.set_row(entity_index, row);
            return None;
        }
        let i = self.keys.partition_point(|&key| key < entity_index);
        self.keys.insert(i, entity_index);
        self.values.insert(i, value);
        self.set_row(entity_index, i);
        self.repair_rows_from(i + 1);
        None
    }

    /// Removes `entity_index`'s row, returning its value if there was one.
    pub(crate) fn remove(&mut self, entity_index: u32) -> Option<T> {
        let i = self.row(entity_index)?;
        self.keys.remove(i);
        let value = self.values.remove(i);
        *slot_mut(&mut self.slots, entity_index) = SENTINEL;
        self.repair_rows_from(i);
        Some(value)
    }

    /// Removes the rows of every index in `entity_indices` (absent and repeated indices are
    /// ignored) in one compaction pass — O(rows + k) where `remove` in a loop is O(k · rows).
    pub(crate) fn remove_many(&mut self, entity_indices: &[u32]) {
        let mut first_removed_row = usize::MAX;
        for &index in entity_indices {
            if let Some(row) = self.row(index) {
                first_removed_row = first_removed_row.min(row);
                *slot_mut(&mut self.slots, index) = SENTINEL;
            }
        }
        if first_removed_row == usize::MAX {
            return;
        }
        // The removed keys' slots are SENTINEL now, so each column keeps exactly the rows whose key
        // still has a slot. `retain` visits in order, so `values` walks `keys` in step.
        let slots = &self.slots;
        let has_row =
            |slots: &[Vec<u32>; 2], key| slot_of(slots, key).is_some_and(|&s| s != SENTINEL);
        let mut keys = self.keys.iter();
        self.values
            .retain(|_| has_row(slots, *keys.next().expect("columns are aligned")));
        self.keys.retain(|&key| has_row(slots, key));
        self.repair_rows_from(first_removed_row);
    }

    /// The two columns as slices: entity indices (ascending) and their values, aligned by position.
    /// The shared half of the query join's cursor.
    #[inline]
    pub(crate) fn as_slices(&self) -> (&[u32], &[T]) {
        (&self.keys, &self.values)
    }

    /// The two columns with the values borrowed exclusively — the keys stay shared, since a column
    /// cursor only reads them. Disjoint fields, so one `&mut self` yields both.
    #[inline]
    pub(crate) fn as_slices_mut(&mut self) -> (&[u32], &mut [T]) {
        (&self.keys, &mut self.values)
    }
}

impl<T: 'static + Send + Sync> AnyStorage for Storage<T> {
    fn as_any(&self) -> &dyn Any {
        self
    }

    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }

    fn remove_entity(&mut self, entity_index: u32) -> bool {
        self.remove(entity_index).is_some()
    }

    fn remove_entities(&mut self, entity_indices: &[u32]) {
        self.remove_many(entity_indices);
    }

    fn entity_indices(&self) -> Box<dyn Iterator<Item = u32> + '_> {
        Box::new(self.keys.iter().copied())
    }

    fn type_name(&self) -> &'static str {
        std::any::type_name::<T>()
    }

    fn is_empty(&self) -> bool {
        self.keys.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ludion_core::rng::DeterministicRng;
    use std::collections::BTreeMap;

    #[test]
    fn the_erased_handle_reports_its_type_name_and_emptiness() {
        let mut storage = Storage::<u64>::new();
        let erased: &dyn AnyStorage = &storage;
        assert_eq!(erased.type_name(), "u64");
        assert!(erased.is_empty());

        storage.insert(3, 7);
        assert!(!AnyStorage::is_empty(&storage));
        storage.remove(3);
        assert!(
            AnyStorage::is_empty(&storage),
            "a storage that lost its last row is empty again"
        );
    }

    /// Key range the randomized tests draw from — small enough that collisions, splices, and
    /// removals of both present and absent keys are frequent.
    const UNIVERSE: u32 = 64;

    /// Checks every invariant documented on [`Storage`] against a `BTreeMap` oracle: dense columns
    /// ascending and equal to the oracle's contents in order, `get`/`contains` agreeing across the
    /// whole key universe, and the slot table pointing every present key at its row and every absent
    /// key at [`SENTINEL`].
    fn assert_matches_oracle(storage: &Storage<u64>, oracle: &BTreeMap<u32, u64>) {
        let (keys, values) = storage.as_slices();
        assert!(
            keys.windows(2).all(|w| w[0] < w[1]),
            "keys not strictly ascending: {keys:?}"
        );
        let oracle_keys: Vec<u32> = oracle.keys().copied().collect();
        let oracle_values: Vec<u64> = oracle.values().copied().collect();
        assert_eq!(
            keys,
            oracle_keys.as_slice(),
            "dense keys diverged from oracle"
        );
        assert_eq!(
            values,
            oracle_values.as_slice(),
            "dense values diverged from oracle"
        );

        for key in 0..UNIVERSE {
            assert_eq!(
                storage.get(key).copied(),
                oracle.get(&key).copied(),
                "get mismatch at key {key}"
            );
            assert_eq!(
                storage.contains(key),
                oracle.contains_key(&key),
                "contains mismatch at key {key}"
            );

            let slot = slot_of(&storage.slots, key).copied().unwrap_or(SENTINEL);
            match oracle_keys.iter().position(|&k| k == key) {
                Some(row) => assert_eq!(slot, row as u32, "slots[{key}] should point at row {row}"),
                None => assert_eq!(slot, SENTINEL, "slots[{key}] should be SENTINEL (absent)"),
            }
        }
    }

    #[test]
    fn slot_table_matches_oracle_under_randomized_interleaving() {
        let mut storage: Storage<u64> = Storage::new();
        let mut oracle: BTreeMap<u32, u64> = BTreeMap::new();
        let mut rng = DeterministicRng::new(0xC0FF_EE42);

        for _ in 0..5000 {
            let key = rng.gen_range_u32(0..UNIVERSE);
            match rng.gen_range_u32(0..3) {
                // Insert: lands on a fresh key or an existing one depending on the oracle's current
                // state, exercising both the append and splice paths as the universe fills up.
                0 => {
                    let value = rng.next_u64();
                    assert_eq!(
                        storage.insert(key, value),
                        oracle.insert(key, value),
                        "insert return mismatch at key {key}"
                    );
                }
                // Remove: present or absent, again decided by whatever the oracle currently holds.
                1 => {
                    assert_eq!(
                        storage.remove(key),
                        oracle.remove(&key),
                        "remove return mismatch at key {key}"
                    );
                }
                // Get: read-only, present or absent.
                _ => {
                    assert_eq!(
                        storage.get(key).copied(),
                        oracle.get(&key).copied(),
                        "get mismatch at key {key}"
                    );
                }
            }
            assert_matches_oracle(&storage, &oracle);
        }
    }

    #[test]
    fn remove_many_matches_oracle_under_randomized_batches() {
        let mut storage: Storage<u64> = Storage::new();
        let mut oracle: BTreeMap<u32, u64> = BTreeMap::new();
        let mut rng = DeterministicRng::new(0xBA7C_4001);

        for _ in 0..600 {
            // Repopulate so remove -> re-insert -> remove rounds are covered.
            for _ in 0..rng.gen_range_u32(0..40) {
                let key = rng.gen_range_u32(0..UNIVERSE);
                let value = rng.next_u64();
                assert_eq!(storage.insert(key, value), oracle.insert(key, value));
            }
            // Absent keys and duplicates are drawn freely from the whole universe.
            let batch: Vec<u32> = (0..rng.gen_range_u32(0..24))
                .map(|_| rng.gen_range_u32(0..UNIVERSE))
                .collect();
            storage.remove_many(&batch);
            for key in &batch {
                oracle.remove(key);
            }
            assert_matches_oracle(&storage, &oracle);
        }
    }

    #[test]
    fn slot_table_survives_append_then_splice_then_remove_from_middle() {
        let mut storage: Storage<u64> = Storage::new();
        let mut oracle: BTreeMap<u32, u64> = BTreeMap::new();

        // Append ascending keys — the fast path that never splices.
        for key in [0u32, 2, 4, 6, 8] {
            oracle.insert(key, key as u64);
            storage.insert(key, key as u64);
            assert_matches_oracle(&storage, &oracle);
        }

        // Splice one into the middle, shifting every row after it.
        oracle.insert(5, 50);
        storage.insert(5, 50);
        assert_matches_oracle(&storage, &oracle);

        // Remove from the middle, shifting every row after it back down.
        oracle.remove(&4);
        storage.remove(4);
        assert_matches_oracle(&storage, &oracle);
    }

    #[test]
    fn a_presentation_insert_allocates_only_its_own_range() {
        let mut storage: Storage<u64> = Storage::new();
        storage.insert(PRESENTATION_BASE, 1);
        assert!(storage.slots[0].is_empty(), "the sim table stays empty");
        assert_eq!(storage.slots[1].len(), 1);
        assert_eq!(storage.get(PRESENTATION_BASE), Some(&1));
        assert_eq!(storage.get(0), None);
        assert_eq!(
            storage.get(2 * PRESENTATION_BASE),
            None,
            "beyond both ranges is absent"
        );
        assert_eq!(storage.get(u32::MAX), None);
    }

    #[test]
    fn slot_invariant_holds_across_interleaved_sim_and_presentation_keys() {
        let keys = [
            3,
            PRESENTATION_BASE + 1,
            7,
            PRESENTATION_BASE,
            1,
            PRESENTATION_BASE + 5,
            5,
        ];
        let mut storage: Storage<u64> = Storage::new();
        for &key in &keys {
            storage.insert(key, u64::from(key));
        }
        let check = |storage: &Storage<u64>| {
            let (rows, values) = storage.as_slices();
            for (r, &key) in rows.iter().enumerate() {
                assert_eq!(storage.row(key), Some(r), "slot of key {key}");
                assert_eq!(values[r], u64::from(key));
            }
        };
        check(&storage);
        assert_eq!(storage.remove(7), Some(7));
        assert_eq!(
            storage.remove(PRESENTATION_BASE),
            Some(u64::from(PRESENTATION_BASE))
        );
        check(&storage);
        storage.remove_many(&[1, PRESENTATION_BASE + 5, 99, 1]);
        check(&storage);
        assert_eq!(storage.as_slices().0, &[3, 5, PRESENTATION_BASE + 1]);
        assert!(!storage.contains(PRESENTATION_BASE + 5));
    }
}
