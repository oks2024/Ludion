//! Typed queries over component storages.
//!
//! [`World::query`](crate::World::query)`::<(&A, &mut B)>()` yields one tuple of references per
//! entity that has **all** of the named components. The join is a **sorted-merge intersection**:
//! every column is a cursor over one storage's dense `(keys, values)` slices — ascending by entity
//! index, which is the storage's invariant — and the iterator gallops each trailing column forward
//! to the candidate index until all agree, then yields. Ascending columns are a *correctness*
//! requirement, not just a determinism one — the merge skips forward and never backtracks, so an
//! unsorted column would make it drop matching rows. This has three properties that matter:
//!
//! - **Order is ascending entity index**, always, regardless of spawn order — conformance-tested.
//! - **No `unsafe`.** A mutable column drives its values with `slice::IterMut`, which hands out
//!   non-aliasing `&mut` by construction; a shared column indexes its slice. Disjoint storages are
//!   borrowed disjointly up front, so several columns can be live at once without aliasing.
//! - **Duplicate component types panic.** Naming the same type twice (`(&mut T, &T)`) would alias a
//!   single storage, so the builder rejects it.
//!
//! The [`Query`], [`QueryParam`], and [`Column`] traits plus the `JoinN` iterators are exposed only
//! because the tuple impls and `World::query`'s return type must name them; they are `#[doc(hidden)]`
//! implementation detail, not a stable extension surface.

use std::any::TypeId;
use std::ops::Range;
use std::slice::IterMut;

use ludion_core::ids::EntityId;

use crate::Component;
use crate::World;
use crate::entity::{EntityAllocator, resolve_id};
use crate::journal::WriteJournal;
use crate::storage::{AnyStorage, Storage};

/// One element of a query's declared *shape*: the underlying component's `TypeId` and Rust type name,
/// plus whether the access is exclusive (`&mut T`, a write) rather than shared (`&T`, a read).
///
/// Extracted from a [`Query`] tuple by [`Query::shape`] **without building an iterator or borrowing a
/// world** — it is lifetime-free data. It is the seam a *declaration* site (an editor's Game Designer
/// view, the query-drift verifier) reads to learn what a query would touch: `type_id`/`write` are the
/// comparison key, `type_name` is what a diagnostic prints when the type is not a registered
/// component.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct QueryElemShape {
    /// The `TypeId` of the underlying component (the reference target, not the reference).
    pub type_id: TypeId,
    /// The Rust type name of the underlying component (`std::any::type_name`), for diagnostics.
    pub type_name: &'static str,
    /// Whether the element is `&mut T` (a write). `false` for `&T` (a read).
    pub write: bool,
}

/// A tuple of `&T` / `&mut T` that can be queried from a [`World`].
///
/// Implemented for tuples of arity 1–4. This is a hidden implementation detail; use it only through
/// [`World::query`](crate::World::query).
#[doc(hidden)]
pub trait Query<'w> {
    /// The tuple of references yielded per matching entity.
    type Item;
    /// The concrete join iterator. It also exposes [`IndexedJoin`], so the id-yielding
    /// [`World::query_ids`](crate::World::query_ids) can recover each row's entity index without a
    /// second, divergent join.
    type Iter: IndexedJoin<Item = Self::Item>;
    /// Builds the iterator, taking disjoint storage borrows from `world`.
    fn build(world: &'w mut World) -> Self::Iter;
    /// Builds the id-yielding iterator: the same join, wrapped so each row carries the live
    /// [`EntityId`] of the entity it belongs to. Borrows the entity allocator (for generations)
    /// disjointly from the storage borrows.
    fn build_ids(world: &'w mut World) -> QueryIds<'w, Self>
    where
        Self: Sized;
    /// The tuple's declared shape — one [`QueryElemShape`] per element, in tuple order. Reads only
    /// each element's lifetime-free `type_id`/`type_name`/`MUTABLE`, so it names no world and builds
    /// no iterator: a declaration site can record what a query *would* touch without running it.
    fn shape() -> Vec<QueryElemShape>;
}

/// A join iterator that can also surface the **entity index** of each row it yields.
///
/// Every `JoinN` computes the aligned index as it advances the sorted-merge; this trait exposes it.
/// [`Iterator::next`] on a join is defined in terms of [`next_indexed`](IndexedJoin::next_indexed) —
/// one merge implementation drives both — so an id-yielding query is guaranteed to visit exactly the
/// same rows in the same ascending-index order as the plain query, differing only in what it returns.
#[doc(hidden)]
pub trait IndexedJoin: Iterator {
    /// Like [`Iterator::next`], but also returns the aligned slot index the row belongs to.
    /// [`World::query_ids`](crate::World::query_ids) resolves that index to a live [`EntityId`].
    fn next_indexed(&mut self) -> Option<(u32, Self::Item)>;
}

/// An id-yielding query iterator: wraps a join and pairs each row with the live [`EntityId`] of the
/// entity it came from.
///
/// Returned by [`World::query_ids`](crate::World::query_ids). It borrows the world's entity allocator
/// (shared, for generations) disjointly from the join's mutable storage borrows, so no `unsafe` and no
/// index→id snapshot map is needed — the id is read straight from the live slot as each row streams.
#[doc(hidden)]
pub struct QueryIds<'w, Q: Query<'w>> {
    join: Q::Iter,
    sim: &'w EntityAllocator,
    presentation: &'w EntityAllocator,
}

impl<'w, Q: Query<'w>> Iterator for QueryIds<'w, Q> {
    type Item = (EntityId, Q::Item);

    fn next(&mut self) -> Option<Self::Item> {
        let (index, row) = self.join.next_indexed()?;
        Some((resolve_id(self.sim, self.presentation, index), row))
    }
}

/// One element of a query tuple: `&T` (shared) or `&mut T` (exclusive).
#[doc(hidden)]
pub trait QueryParam<'w> {
    /// The reference yielded for this element.
    type Item;
    /// The column iterator backing this element.
    type Col: Column<'w, Item = Self::Item>;
    /// Whether this element is an exclusive (`&mut T`) borrow — a *write* — rather than a shared
    /// (`&T`) *read*. Captured so a declared/observed query shape records access, not just identity.
    const MUTABLE: bool;
    /// The `TypeId` of the underlying component, for duplicate detection and storage lookup.
    fn type_id() -> TypeId;
    /// The Rust type name of the underlying component (not the reference), for diagnostics.
    fn type_name() -> &'static str;
    /// Builds the column from a disjoint storage borrow (`None` ⇒ empty column).
    fn make_col(storage: Option<&'w mut dyn AnyStorage>, window: &KeyWindow) -> Self::Col;
}

/// An ascending-by-entity-index column of a query, supporting the sorted-merge join.
#[doc(hidden)]
pub trait Column<'w> {
    /// The reference this column yields.
    type Item;
    /// The next entity index this column would yield, without consuming it.
    fn peek_key(&mut self) -> Option<u32>;
    /// Consumes and returns the next value. Only called once the key is confirmed aligned.
    fn take(&mut self) -> Self::Item;
    /// Advances past every entry whose key is below `target`, returning the key it stops at.
    fn seek(&mut self, target: u32) -> Option<u32>;
}

/// A shared column: `&T` values in ascending index order.
///
/// A cursor over one storage's `(keys, values)` slices: `pos` is the next row, so `peek_key` is a
/// slice read with no iterator state to keep in step. A column with no storage behind it is the
/// empty pair of slices — the join then simply finds no key and stops.
#[doc(hidden)]
pub struct RefCol<'w, T> {
    keys: &'w [u32],
    values: &'w [T],
    pos: usize,
}

/// An exclusive column: `&mut T` values in ascending index order.
///
/// The keys are read through a shared slice as in [`RefCol`], but the values are handed out by
/// `slice::IterMut` — the safe way for a cursor to yield successive `&'w mut T` that do not alias.
/// `pos` and the value iterator advance together, by `take` and by `seek`.
#[doc(hidden)]
pub struct MutCol<'w, T> {
    keys: &'w [u32],
    values: IterMut<'w, T>,
    pos: usize,
}

impl<'w, T> Column<'w> for RefCol<'w, T> {
    type Item = &'w T;
    #[inline(always)]
    fn peek_key(&mut self) -> Option<u32> {
        self.keys.get(self.pos).copied()
    }
    #[inline(always)]
    fn take(&mut self) -> &'w T {
        // Copy the slice reference out so the yielded borrow lives for `'w`, not for this `&mut self`.
        let values: &'w [T] = self.values;
        let value = values.get(self.pos).expect("take on exhausted column");
        self.pos += 1;
        value
    }
    fn seek(&mut self, target: u32) -> Option<u32> {
        self.pos = seek(self.keys, self.pos, target);
        self.keys.get(self.pos).copied()
    }
}

impl<'w, T> Column<'w> for MutCol<'w, T> {
    type Item = &'w mut T;
    #[inline(always)]
    fn peek_key(&mut self) -> Option<u32> {
        self.keys.get(self.pos).copied()
    }
    #[inline(always)]
    fn take(&mut self) -> &'w mut T {
        self.pos += 1;
        self.values.next().expect("take on exhausted column")
    }
    fn seek(&mut self, target: u32) -> Option<u32> {
        let to = seek(self.keys, self.pos, target);
        if to > self.pos {
            self.values.nth(to - self.pos - 1);
            self.pos = to;
        }
        self.keys.get(to).copied()
    }
}

impl<'w, T: Component> QueryParam<'w> for &'w T {
    type Item = &'w T;
    type Col = RefCol<'w, T>;
    const MUTABLE: bool = false;
    fn type_id() -> TypeId {
        TypeId::of::<T>()
    }
    fn type_name() -> &'static str {
        std::any::type_name::<T>()
    }
    fn make_col(storage: Option<&'w mut dyn AnyStorage>, window: &KeyWindow) -> RefCol<'w, T> {
        match storage {
            Some(s) => {
                let concrete = s
                    .as_any_mut()
                    .downcast_mut::<Storage<T>>()
                    .expect("storage TypeId matches component type");
                let shared: &'w Storage<T> = &*concrete;
                let (keys, values) = window_slices(shared.as_slices(), window);
                RefCol {
                    keys,
                    values,
                    pos: 0,
                }
            }
            None => RefCol {
                keys: &[],
                values: &[],
                pos: 0,
            },
        }
    }
}

impl<'w, T: Component> QueryParam<'w> for &'w mut T {
    type Item = &'w mut T;
    type Col = MutCol<'w, T>;
    const MUTABLE: bool = true;
    fn type_id() -> TypeId {
        TypeId::of::<T>()
    }
    fn type_name() -> &'static str {
        std::any::type_name::<T>()
    }
    fn make_col(storage: Option<&'w mut dyn AnyStorage>, window: &KeyWindow) -> MutCol<'w, T> {
        match storage {
            Some(s) => {
                let concrete = s
                    .as_any_mut()
                    .downcast_mut::<Storage<T>>()
                    .expect("storage TypeId matches component type");
                let (keys, values) = window_slices_mut(concrete.as_slices_mut(), window);
                MutCol {
                    keys,
                    values: values.iter_mut(),
                    pos: 0,
                }
            }
            None => MutCol {
                keys: &[],
                values: <&mut [T]>::default().iter_mut(),
                pos: 0,
            },
        }
    }
}

/// Panics if a query tuple names the same component type twice — it would alias a single storage.
/// Shared by both build paths ([`Query::build`] and [`Query::build_ids`]) and by the parallel join
/// in [`crate::par_query`], so the guard is identical everywhere.
pub(crate) fn assert_unique_query_types(ids: &[TypeId]) {
    for i in 0..ids.len() {
        for j in (i + 1)..ids.len() {
            assert!(
                ids[i] != ids[j],
                "ludion-ecs: a query tuple names the same component type twice; each component may \
                 appear at most once per query (it would alias a storage)",
            );
        }
    }
}

/// The half-open range of entity indices a query may see — the phase rule of the sim/presentation
/// boundary, applied once per column as one `partition_point` (never per row).
pub(crate) type KeyWindow = Range<u32>;

/// The window that admits every row.
pub(crate) const FULL_WINDOW: KeyWindow = 0..u32::MAX;

/// The positions of `keys` (ascending) that fall inside `window`. O(1) when the whole column is
/// inside; otherwise one `partition_point` per bound that is not trivially open.
fn window_range(keys: &[u32], window: &KeyWindow) -> Range<usize> {
    let (Some(&first), Some(&last)) = (keys.first(), keys.last()) else {
        return 0..0;
    };
    if window.start <= first && last < window.end {
        return 0..keys.len();
    }
    let from = if window.start == 0 {
        0
    } else {
        keys.partition_point(|&k| k < window.start)
    };
    let to = if window.end == u32::MAX {
        keys.len()
    } else {
        keys.partition_point(|&k| k < window.end)
    };
    from..to
}

/// `(keys, values)` clamped to `window`, keys and values cut at the same positions.
pub(crate) fn window_slices<'a, T>(
    (keys, values): (&'a [u32], &'a [T]),
    window: &KeyWindow,
) -> (&'a [u32], &'a [T]) {
    let range = window_range(keys, window);
    (&keys[range.clone()], &values[range])
}

/// The exclusive counterpart of [`window_slices`].
pub(crate) fn window_slices_mut<'a, T>(
    (keys, values): (&'a [u32], &'a mut [T]),
    window: &KeyWindow,
) -> (&'a [u32], &'a mut [T]) {
    let range = window_range(keys, window);
    (&keys[range.clone()], &mut values[range])
}

/// First position `p >= from` with `keys[p] >= target`, or `keys.len()`. `keys` ascending.
///
/// Gallops: probes at doubling distances, then binary-searches the bracket, so a far jump costs
/// `O(log gap)` and a jump of one costs a compare or two. Stops exactly where a row-by-row scan
/// for the first key `>= target` would.
#[inline]
pub(crate) fn seek(keys: &[u32], from: usize, target: u32) -> usize {
    match keys.get(from) {
        None => return keys.len(),
        Some(&k) if k >= target => return from,
        _ => {}
    }
    gallop(keys, from, target)
}

/// The far-jump half of [`seek`], kept out of line so the aligned-column check inlines into the join.
#[cold]
#[inline(never)]
fn gallop(keys: &[u32], from: usize, target: u32) -> usize {
    // Invariant: keys[from + lo] < target.
    let (mut lo, mut step) = (0usize, 1usize);
    while let Some(&k) = keys.get(from + step) {
        if k >= target {
            break;
        }
        lo = step;
        step *= 2;
    }
    let start = from + lo + 1;
    let end = (from + step + 1).min(keys.len());
    start + keys[start..end].partition_point(|&k| k < target)
}

/// Generates a `JoinN` iterator plus the `Query` impl for an N-tuple of [`QueryParam`]s.
macro_rules! impl_query {
    ($join:ident, $( ($col:ident, $P:ident) ),+ ) => {
        #[doc(hidden)]
        #[allow(non_snake_case)]
        pub struct $join<'w, $($P: QueryParam<'w>),+> {
            $( $col: $P::Col, )+
            /// Armed only for a query with a `&mut` column while a marker is watched: every
            /// yielded row is journaled.
            journal: Option<&'w mut WriteJournal>,
        }

        impl<'w, $($P: QueryParam<'w>),+> Query<'w> for ($($P,)+) {
            type Item = ($($P::Item,)+);
            type Iter = $join<'w, $($P),+>;

            fn build(world: &'w mut World) -> Self::Iter {
                let ids = [ $( $P::type_id() ),+ ];
                assert_unique_query_types(&ids);
                let write = false $(|| $P::MUTABLE)+;
                let window = world.query_window(write);
                let (_, _, journal, storages) = world.split_for_query(&ids, write);
                let mut drain = storages.into_iter();
                $( let $col = $P::make_col(drain.next().expect("one storage slot per query element"), &window); )+
                $join { $($col,)+ journal }
            }

            fn build_ids(world: &'w mut World) -> QueryIds<'w, Self> {
                let ids = [ $( $P::type_id() ),+ ];
                assert_unique_query_types(&ids);
                // Split the world's borrow: the entity allocator (shared, for generations) and the
                // named storages (mutable) are disjoint fields, so both can be live at once.
                let write = false $(|| $P::MUTABLE)+;
                let window = world.query_window(write);
                let (sim, presentation, journal, storages) = world.split_for_query(&ids, write);
                let mut drain = storages.into_iter();
                $( let $col = $P::make_col(drain.next().expect("one storage slot per query element"), &window); )+
                QueryIds { join: $join { $($col,)+ journal }, sim, presentation }
            }

            fn shape() -> Vec<QueryElemShape> {
                vec![ $( QueryElemShape {
                    type_id: $P::type_id(),
                    type_name: $P::type_name(),
                    write: $P::MUTABLE,
                } ),+ ]
            }
        }

        impl<'w, $($P: QueryParam<'w>),+> IndexedJoin for $join<'w, $($P),+> {
            fn next_indexed(&mut self) -> Option<(u32, Self::Item)> {
                loop {
                    // Highest front key across all columns is the next candidate index. Entity
                    // indices are `u32`, so 0 is a safe identity for the running maximum.
                    let mut target: u32 = 0;
                    $(
                        match self.$col.peek_key() {
                            None => return None,
                            Some(k) => target = target.max(k),
                        }
                    )+
                    // Advance every column up to `target`; note if any overshoots it.
                    let mut aligned = true;
                    $(
                        let front = match self.$col.peek_key() {
                            None => return None,
                            Some(k) if k >= target => k,
                            Some(_) => match self.$col.seek(target) {
                                None => return None,
                                Some(k) => k,
                            },
                        };
                        if front != target {
                            aligned = false;
                        }
                    )+
                    if aligned {
                        if let Some(journal) = self.journal.as_deref_mut() {
                            journal.touch(target);
                        }
                        return Some((target, ( $( self.$col.take(), )+ )));
                    }
                    // Otherwise a column overshot; loop to recompute a higher target.
                }
            }
        }

        impl<'w, $($P: QueryParam<'w>),+> Iterator for $join<'w, $($P),+> {
            type Item = ($($P::Item,)+);

            // Defined in terms of the indexed merge so `query` and `query_ids` share one join and one
            // order; discarding the index is a no-op move the optimizer removes, so the plain path
            // keeps its original cost.
            fn next(&mut self) -> Option<Self::Item> {
                self.next_indexed().map(|(_, row)| row)
            }
        }
    };
}

impl_query!(Join1, (c0, P0));
impl_query!(Join2, (c0, P0), (c1, P1));
impl_query!(Join3, (c0, P0), (c1, P1), (c2, P2));
impl_query!(Join4, (c0, P0), (c1, P1), (c2, P2), (c3, P3));

#[cfg(test)]
mod tests {
    use ludion_core::rng::DeterministicRng;

    use super::seek;

    fn linear(keys: &[u32], from: usize, target: u32) -> usize {
        let mut p = from;
        while p < keys.len() && keys[p] < target {
            p += 1;
        }
        p
    }

    #[test]
    fn seek_edge_cases_match_a_linear_scan() {
        let keys = [2u32, 4, 5, 9, 10, 11, 30];
        assert_eq!(seek(&[], 0, 7), 0);
        for from in 0..=keys.len() {
            for target in 0..=32 {
                assert_eq!(
                    seek(&keys, from, target),
                    linear(&keys, from, target),
                    "from {from} target {target}",
                );
            }
        }
        // Above every key, at the end, and at or below the front key.
        assert_eq!(seek(&keys, 0, 1000), keys.len());
        assert_eq!(seek(&keys, keys.len(), 0), keys.len());
        assert_eq!(seek(&keys, 3, 0), 3);
    }

    #[test]
    fn seek_matches_a_linear_scan_on_random_slices() {
        let mut rng = DeterministicRng::new(0x5EE4);
        for _ in 0..2000 {
            let len = rng.gen_range_usize(0..300);
            let mut key = 0u32;
            let keys: Vec<u32> = (0..len)
                .map(|_| {
                    key += rng.gen_range_u32(1..40);
                    key
                })
                .collect();
            let from = rng.gen_range_usize(0..len + 1);
            let target = rng.gen_range_u32(0..key + 50);
            assert_eq!(seek(&keys, from, target), linear(&keys, from, target));
        }
    }
}
