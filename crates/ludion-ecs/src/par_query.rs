//! Deterministic parallel iteration over a query's rows.
//!
//! [`World::par_for_each`] and [`World::par_join_fold`] are the ECS-level entry points onto
//! [`ludion_core::par`] — the engine's only sanctioned way to put more than one thread on the sim
//! path (see the `determinism` docs topic). They spread **one system's** per-entity work across
//! chunks; the schedule itself stays strictly sequential.
//!
//! # The region model
//!
//! A [`par_for_each`](World::par_for_each) call opens a *region*:
//!
//! 1. The join runs **sequentially first**, producing a row list — one entry per entity that has
//!    every named component, in ascending entity index. That is exactly the row set and order
//!    [`World::query`] yields; the sorted-merge is the same algorithm.
//! 2. Each `&mut` column's storage is lifted **out of the world** for the duration. Its dense value
//!    column is then handed to the chunks as disjoint `&mut` sub-slices (progressive
//!    [`split_at_mut`](slice::split_at_mut), no `unsafe`), because columns are ascending and a
//!    chunk's rows therefore occupy one contiguous position range per column.
//! 3. Each `&` column stays in the world and is read as a shared slice.
//! 4. Every chunk closure receives a [`ParCtx`] carrying `&World` — so a row can read *other*
//!    entities, other components, and resources while it writes its own.
//!
//! **The consequence to internalize: inside the region, the `&mut` columns are absent from the
//! world.** `ctx.world.get::<T>(other)` for a `T` the query took exclusively is an `L0026` error
//! (`has`, `entity_indices_with` and `par_join_fold` panic with it), not the live value. That is
//! the price of handing out disjoint `&mut` slices without `unsafe`, and it is deliberate: the
//! alternative is aliasing. Everything *not* named `&mut` by the query reads normally.
//!
//! # What makes it deterministic
//!
//! - Row order is the join's order, fixed before any thread starts.
//! - Chunk boundaries come from `chunk_rows` alone (fixed partitioning), so a chunk owns the same
//!   rows at any thread count. Per [`ludion_core::par`], **chunk size is part of the determinism
//!   contract**; changing it at a call site is a hash-moving edit.
//! - Results concatenate in ascending chunk order, so the returned `Vec<R>` is in row order.
//! - No intra-chunk fold happens here, so `par_for_each`'s result is invariant under `chunk_rows`
//!   too; `par_join_fold`'s is not, by design (it folds per-chunk values, like
//!   [`ludion_core::par::ordered_reduce`]).
//! - Panics propagate by `par`'s rule (lowest chunk index wins), and the lifted columns go back into
//!   the world before the unwind continues — a system that panics mid-region leaves a structurally
//!   whole world for the schedule's boundary to hand to the host.
//!
//! The [`ParQuery`], [`ParReadQuery`], and [`ParParam`] traits plus the per-arity task structs are
//! `#[doc(hidden)]` implementation detail, exposed only because the method signatures must name
//! them.

use std::any::TypeId;
use std::panic::{self, AssertUnwindSafe};

use ludion_core::ids::EntityId;
use ludion_core::par;

use crate::Component;
use crate::World;
use crate::query::{
    KeyWindow, QueryElemShape, assert_unique_query_types, seek, window_slices, window_slices_mut,
};
use crate::storage::{AnyStorage, Storage};

// --- The chunk context ------------------------------------------------------------------------

/// What a [`par_for_each`](World::par_for_each) closure sees of the world beyond its own row.
///
/// `world` is the live world **minus the query's `&mut` columns**, which are lifted out for the
/// duration of the region (see the [module docs](self)). Reads of anything else — other components,
/// other entities, resources — behave exactly as they do on the sim thread, recording included.
pub struct ParCtx<'c> {
    /// The world as the region sees it: every component except the ones this query took `&mut`.
    pub world: &'c World,
    /// Ascending entity indices of the joined rows, for [`in_join`](ParCtx::in_join).
    joined: &'c [u32],
}

impl ParCtx<'_> {
    /// Whether `entity` is one of the rows this region is iterating.
    ///
    /// A binary search over the precomputed joined key list: it answers "does this other entity
    /// match the same query?" without touching a storage — which matters inside a region, where the
    /// `&mut` columns are not in the world to be probed. Like [`World::has`] it reads presence only
    /// and records no access, and it is total: a stale or dead handle is simply `false` (rows exist
    /// only for live entities, so a matching index paired with a stale generation is rejected).
    pub fn in_join(&self, entity: EntityId) -> bool {
        self.world.is_alive(entity) && self.joined.binary_search(&entity.index).is_ok()
    }
}

// --- The sequential join ----------------------------------------------------------------------

/// The precomputed row list of one region: for each joined entity, its id and its position in every
/// column. Built sequentially before any parallelism, so the partitioning below is over a fixed
/// list.
struct JoinRows {
    /// One live [`EntityId`] per row, ascending by slot index.
    ids: Vec<EntityId>,
    /// The same rows' bare slot indices, for [`ParCtx::in_join`]'s binary search.
    keys: Vec<u32>,
    /// Row-major `rows × arity` positions: `positions[row * arity + column]` is the row's index in
    /// that column's dense slices. Strictly increasing down each column, which is what makes a
    /// chunk's rows a contiguous position range.
    positions: Vec<usize>,
}

/// Runs the sorted-merge intersection over `key_slices` (each ascending, one per query element).
///
/// This is the same merge the sequential [`Query`](crate::Query) join performs — highest front key
/// is the candidate, trailing columns gallop forward, a row is emitted only when every column agrees —
/// so `par_for_each` visits exactly the rows `query` yields, in exactly the same ascending order.
fn join_rows(world: &World, key_slices: &[&[u32]]) -> JoinRows {
    let arity = key_slices.len();
    let mut cursors = vec![0usize; arity];
    let mut rows = JoinRows {
        ids: Vec::new(),
        keys: Vec::new(),
        positions: Vec::new(),
    };
    'merge: loop {
        // Highest front key across all columns is the next candidate index.
        let mut target: u32 = 0;
        for (column, keys) in key_slices.iter().enumerate() {
            match keys.get(cursors[column]) {
                None => break 'merge,
                Some(&key) => target = target.max(key),
            }
        }
        // Advance every column up to `target`; note if any overshoots it.
        let mut aligned = true;
        for (column, keys) in key_slices.iter().enumerate() {
            let front = match keys.get(cursors[column]) {
                None => break 'merge,
                Some(&key) if key >= target => key,
                Some(_) => {
                    cursors[column] = seek(keys, cursors[column], target);
                    match keys.get(cursors[column]) {
                        None => break 'merge,
                        Some(&key) => key,
                    }
                }
            };
            if front != target {
                aligned = false;
            }
        }
        if aligned {
            rows.keys.push(target);
            rows.ids.push(world.id_at(target));
            rows.positions.extend_from_slice(&cursors);
            for cursor in &mut cursors {
                *cursor += 1;
            }
        }
        // Otherwise a column overshot; loop to recompute a higher target.
    }
    rows
}

// --- Query elements ---------------------------------------------------------------------------

/// One element of a parallel query tuple: `&T` (shared, stays in the world) or `&mut T` (exclusive,
/// lifted out and chunked).
///
/// Hidden implementation detail; it exists so one macro can generate every arity without caring
/// which elements are exclusive.
#[doc(hidden)]
pub trait ParParam {
    /// The reference a single row gets. Borrows the chunk for the duration of one closure call, not
    /// for the region — which is what lets successive rows hand out successive `&mut`s safely.
    type Item<'a>;
    /// One chunk's view of this column: an exclusive sub-slice, or the shared slice for its rows.
    /// `Send` because a chunk task crosses to a worker thread.
    type Chunk<'a>: Send;
    /// Whether this element is `&mut T` (a write) rather than `&T` (a read).
    const MUTABLE: bool;
    /// The `TypeId` of the underlying component.
    fn type_id() -> TypeId;
    /// The Rust type name of the underlying component, for diagnostics.
    fn type_name() -> &'static str;
    /// Lifts this element's storage out of `world` — `Some` for an exclusive element, `None` for a
    /// shared one (which reads in place).
    fn take(world: &mut World) -> Option<Box<dyn AnyStorage>>;
    /// This element's key slice and its whole value source, clamped to `window` (the phase's view
    /// of the id ranges): from the lifted box for an exclusive element, from `world` for a shared
    /// one.
    fn acquire<'a>(
        world: &'a World,
        taken: &'a mut Option<Box<dyn AnyStorage>>,
        window: &KeyWindow,
    ) -> (&'a [u32], Self::Chunk<'a>);
    /// Splits `source` into its leading `take` values and the remainder. Called once per chunk in
    /// ascending chunk order, threading the remainder forward, so the pieces are disjoint by
    /// construction — for an exclusive column that is one `split_at_mut` per chunk.
    fn split<'a>(source: Self::Chunk<'a>, take: usize) -> (Self::Chunk<'a>, Self::Chunk<'a>)
    where
        Self: 'a;
    /// The reference for one row, at `offset` within this chunk's slice.
    fn item<'a, 'b>(chunk: &'a mut Self::Chunk<'b>, offset: usize) -> Self::Item<'a>
    where
        'b: 'a;
}

impl<T: Component> ParParam for &'static T {
    type Item<'a> = &'a T;
    type Chunk<'a> = &'a [T];
    const MUTABLE: bool = false;

    fn type_id() -> TypeId {
        TypeId::of::<T>()
    }

    fn type_name() -> &'static str {
        std::any::type_name::<T>()
    }

    fn take(_world: &mut World) -> Option<Box<dyn AnyStorage>> {
        None
    }

    fn acquire<'a>(
        world: &'a World,
        _taken: &'a mut Option<Box<dyn AnyStorage>>,
        window: &KeyWindow,
    ) -> (&'a [u32], &'a [T]) {
        match world.storage_opt::<T>() {
            Some(storage) => window_slices(storage.as_slices(), window),
            None => (&[], &[]),
        }
    }

    fn split<'a>(source: &'a [T], take: usize) -> (&'a [T], &'a [T])
    where
        Self: 'a,
    {
        source.split_at(take)
    }

    fn item<'a, 'b>(chunk: &'a mut &'b [T], offset: usize) -> &'a T
    where
        'b: 'a,
    {
        &chunk[offset]
    }
}

impl<T: Component> ParParam for &'static mut T {
    type Item<'a> = &'a mut T;
    type Chunk<'a> = &'a mut [T];
    const MUTABLE: bool = true;

    fn type_id() -> TypeId {
        TypeId::of::<T>()
    }

    fn type_name() -> &'static str {
        std::any::type_name::<T>()
    }

    fn take(world: &mut World) -> Option<Box<dyn AnyStorage>> {
        world.take_storage_erased(TypeId::of::<T>())
    }

    fn acquire<'a>(
        _world: &'a World,
        taken: &'a mut Option<Box<dyn AnyStorage>>,
        window: &KeyWindow,
    ) -> (&'a [u32], &'a mut [T]) {
        match taken {
            Some(boxed) => window_slices_mut(
                boxed
                    .as_any_mut()
                    .downcast_mut::<Storage<T>>()
                    .expect("storage TypeId matches component type")
                    .as_slices_mut(),
                window,
            ),
            None => (&[], <&mut [T]>::default()),
        }
    }

    fn split<'a>(source: &'a mut [T], take: usize) -> (&'a mut [T], &'a mut [T])
    where
        Self: 'a,
    {
        source.split_at_mut(take)
    }

    fn item<'a, 'b>(chunk: &'a mut &'b mut [T], offset: usize) -> &'a mut T
    where
        'b: 'a,
    {
        &mut chunk[offset]
    }
}

// --- The exclusive (write) query ---------------------------------------------------------------

/// A tuple of `&T` / `&mut T` that [`World::par_for_each`] can iterate in parallel.
///
/// Implemented for tuples of arity 1–4. Hidden implementation detail; use it only through
/// [`World::par_for_each`].
#[doc(hidden)]
pub trait ParQuery {
    /// The tuple of references handed to the closure for one row. A GAT rather than a plain
    /// associated type because each row's references live for that one call, not for the region.
    type Item<'c>;

    /// The tuple's declared shape — the same lifetime-free data [`crate::Query::shape`] produces, so
    /// a region records exactly what the equivalent sequential query would.
    fn shape() -> Vec<QueryElemShape>;

    /// Opens the region, runs the chunks, closes it, and returns the per-row results in row order.
    fn run_region<R, F>(world: &mut World, chunk_rows: usize, f: &F) -> Vec<R>
    where
        R: Send,
        F: for<'c> Fn(&ParCtx<'c>, EntityId, Self::Item<'c>) -> R + Sync;
}

/// Generates the per-chunk task struct plus the [`ParQuery`] impl for an N-tuple of [`ParParam`]s.
macro_rules! impl_par_query {
    ($task:ident, $arity:literal, $( ($col:ident, $keys:ident, $src:ident, $lift:ident, $P:ident, $i:tt) ),+ ) => {
        /// One chunk's work: the rows it owns, and its disjoint slice of every column.
        #[doc(hidden)]
        pub struct $task<'c, $($P: ParParam),+> {
            /// First joined row this chunk owns.
            start_row: usize,
            /// One past the last joined row this chunk owns.
            end_row: usize,
            /// Absolute start position of this chunk's slice in each column, so a row's absolute
            /// position converts to an offset within the slice.
            bases: [usize; $arity],
            $( $col: $P::Chunk<'c>, )+
        }

        impl<$($P: ParParam),+> ParQuery for ($($P,)+) {
            type Item<'c> = ($($P::Item<'c>,)+);

            fn shape() -> Vec<QueryElemShape> {
                vec![ $( QueryElemShape {
                    type_id: $P::type_id(),
                    type_name: $P::type_name(),
                    write: $P::MUTABLE,
                } ),+ ]
            }

            fn run_region<R, F>(world: &mut World, chunk_rows: usize, f: &F) -> Vec<R>
            where
                R: Send,
                F: for<'c> Fn(&ParCtx<'c>, EntityId, Self::Item<'c>) -> R + Sync,
            {
                assert!(chunk_rows > 0, "par_for_each: chunk_rows must be non-zero");
                assert_unique_query_types(&[ $( $P::type_id() ),+ ]);
                // The region declares the same shape the equivalent `query` would, before it opens:
                // observation must not depend on what the data happened to contain.
                if world.is_recording() {
                    world.record_query_shape(Self::shape());
                }

                // Clamp every column to the phase's window before the join and the chunking, so rows
                // and chunk positions index the clamped slices.
                let window = world.query_window(false $(|| $P::MUTABLE)+);
                $( let mut $lift = $P::take(world); )+
                // The lifted boxes are owned locals, so the world can be re-borrowed *shared* for
                // the whole region — that shared borrow is what every chunk's `ParCtx` carries.
                let outcome = {
                    let shared: &World = &*world;
                    panic::catch_unwind(AssertUnwindSafe(|| {
                        $( let ($keys, mut $src) = $P::acquire(shared, &mut $lift, &window); )+
                        let join = join_rows(shared, &[ $($keys),+ ]);
                        let rows = join.ids.len();
                        if rows == 0 {
                            return Vec::new();
                        }
                        let lengths = [ $( $keys.len() ),+ ];
                        let chunk_count = rows.div_ceil(chunk_rows);

                        // `boundaries[k * arity + c]` = absolute start of chunk k in column c. Chunk
                        // 0 starts at 0 (so leading unjoined values ride along in it), chunk k at
                        // its first row's position, and the sentinel row is each column's length —
                        // every value is in exactly one chunk's slice.
                        let mut boundaries = Vec::with_capacity((chunk_count + 1) * $arity);
                        boundaries.extend([0usize; $arity]);
                        for k in 1..chunk_count {
                            let row = k * chunk_rows;
                            boundaries.extend_from_slice(&join.positions[row * $arity..][..$arity]);
                        }
                        boundaries.extend_from_slice(&lengths);

                        let mut tasks: Vec<($task<$($P),+>, Vec<R>)> =
                            Vec::with_capacity(chunk_count);
                        for k in 0..chunk_count {
                            let here: [usize; $arity] = boundaries[k * $arity..][..$arity]
                                .try_into()
                                .expect("one boundary per column");
                            let next: [usize; $arity] = boundaries[(k + 1) * $arity..][..$arity]
                                .try_into()
                                .expect("one boundary per column");
                            let task = $task {
                                start_row: k * chunk_rows,
                                end_row: ((k + 1) * chunk_rows).min(rows),
                                bases: here,
                                $( $col: {
                                    let (piece, rest) = $P::split($src, next[$i] - here[$i]);
                                    $src = rest;
                                    piece
                                }, )+
                            };
                            tasks.push((task, Vec::new()));
                        }

                        let ctx = ParCtx { world: shared, joined: &join.keys };
                        // One task per `par` chunk: the row partitioning is already fixed above, so
                        // `par` only has to hand whole tasks to workers and propagate the
                        // lowest-index panic.
                        par::par_chunks(&mut tasks, 1, |_, slot| {
                            let (task, out) = &mut slot[0];
                            let mut results = Vec::with_capacity(task.end_row - task.start_row);
                            for row in task.start_row..task.end_row {
                                let positions = &join.positions[row * $arity..][..$arity];
                                let item = ( $(
                                    $P::item(&mut task.$col, positions[$i] - task.bases[$i]),
                                )+ );
                                results.push(f(&ctx, join.ids[row], item));
                            }
                            *out = results;
                        });
                        tasks.into_iter().flat_map(|(_, out)| out).collect()
                    }))
                };

                // The columns go back before anything else happens — including before a panic
                // resumes — so the world a caught panic leaves is structurally whole.
                $( if let Some(boxed) = $lift {
                    world.put_storage_erased($P::type_id(), boxed);
                } )+

                match outcome {
                    Ok(results) => results,
                    Err(payload) => panic::resume_unwind(payload),
                }
            }
        }
    };
}

impl_par_query!(ParTask1, 1, (c0, k0, s0, l0, P0, 0));
impl_par_query!(
    ParTask2,
    2,
    (c0, k0, s0, l0, P0, 0),
    (c1, k1, s1, l1, P1, 1)
);
impl_par_query!(
    ParTask3,
    3,
    (c0, k0, s0, l0, P0, 0),
    (c1, k1, s1, l1, P1, 1),
    (c2, k2, s2, l2, P2, 2)
);
impl_par_query!(
    ParTask4,
    4,
    (c0, k0, s0, l0, P0, 0),
    (c1, k1, s1, l1, P1, 1),
    (c2, k2, s2, l2, P2, 2),
    (c3, k3, s3, l3, P3, 3)
);

// --- The read-only query ------------------------------------------------------------------------

/// A tuple of `&T` that [`World::par_join_fold`] can iterate in parallel.
///
/// Read-only, so every column stays in the world and every reference lives for the whole region —
/// which is why this needs no lifting, no chunk splitting, and no `&mut World`. Implemented for
/// arity 1–4. Hidden implementation detail.
#[doc(hidden)]
pub trait ParReadQuery {
    /// The tuple of shared references for one row.
    type Item<'c>;
    /// Every column's `(keys, values)` slices. `Copy + Send + Sync` so each chunk can take its own
    /// copy of the same shared view.
    type Cols<'c>: Copy + Send + Sync;
    /// The number of columns — the stride of [`JoinRows::positions`].
    const ARITY: usize;

    /// The tuple's declared shape (all reads).
    fn shape() -> Vec<QueryElemShape>;
    /// Borrows every column out of `world`, clamped to `window`.
    fn cols<'c>(world: &'c World, window: &KeyWindow) -> Self::Cols<'c>;
    /// The columns' key slices, in tuple order, for the join.
    fn keys<'c>(cols: &Self::Cols<'c>) -> Vec<&'c [u32]>;
    /// One row's references, given its position in each column.
    fn item<'c>(cols: &Self::Cols<'c>, positions: &[usize]) -> Self::Item<'c>;
}

/// Generates the [`ParReadQuery`] impl for an N-tuple of shared references.
macro_rules! impl_par_read_query {
    ($arity:literal, $( ($T:ident, $i:tt) ),+ ) => {
        impl<$($T: Component),+> ParReadQuery for ($(&'static $T,)+) {
            type Item<'c> = ($(&'c $T,)+);
            type Cols<'c> = ($((&'c [u32], &'c [$T]),)+);
            const ARITY: usize = $arity;

            fn shape() -> Vec<QueryElemShape> {
                vec![ $( QueryElemShape {
                    type_id: TypeId::of::<$T>(),
                    type_name: std::any::type_name::<$T>(),
                    write: false,
                } ),+ ]
            }

            fn cols<'c>(world: &'c World, window: &KeyWindow) -> Self::Cols<'c> {
                ( $(
                    match world.storage_opt::<$T>() {
                        Some(storage) => window_slices(storage.as_slices(), window),
                        None => {
                            world.panic_if_lifted(TypeId::of::<$T>());
                            (&[][..], &[][..])
                        }
                    },
                )+ )
            }

            fn keys<'c>(cols: &Self::Cols<'c>) -> Vec<&'c [u32]> {
                vec![ $( cols.$i.0 ),+ ]
            }

            fn item<'c>(cols: &Self::Cols<'c>, positions: &[usize]) -> Self::Item<'c> {
                ( $( &cols.$i.1[positions[$i]], )+ )
            }
        }
    };
}

impl_par_read_query!(1, (A, 0));
impl_par_read_query!(2, (A, 0), (B, 1));
impl_par_read_query!(3, (A, 0), (B, 1), (C, 2));
impl_par_read_query!(4, (A, 0), (B, 1), (C, 2), (D, 3));

/// The rows of one [`par_join_fold`](World::par_join_fold) chunk: `(EntityId, Q::Item)` in ascending
/// entity index, exactly the slice of the join this chunk owns.
#[doc(hidden)]
pub struct ParRows<'c, Q: ParReadQuery> {
    cols: Q::Cols<'c>,
    join: &'c JoinRows,
    row: usize,
    end: usize,
}

impl<'c, Q: ParReadQuery> Iterator for ParRows<'c, Q> {
    type Item = (EntityId, Q::Item<'c>);

    fn next(&mut self) -> Option<Self::Item> {
        if self.row >= self.end {
            return None;
        }
        let row = self.row;
        self.row += 1;
        let positions = &self.join.positions[row * Q::ARITY..][..Q::ARITY];
        Some((self.join.ids[row], Q::item(&self.cols, positions)))
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        let remaining = self.end - self.row;
        (remaining, Some(remaining))
    }
}

impl<Q: ParReadQuery> ExactSizeIterator for ParRows<'_, Q> {}

// --- The World entry points ---------------------------------------------------------------------

impl World {
    /// Iterates the entities matching `Q` in parallel, running `f` once per row and returning the
    /// results **in row order**.
    ///
    /// `Q` is a tuple of `&T` / `&mut T` of arity 1–4 — the same spelling [`query`](World::query)
    /// takes — and the rows are the same rows in the same ascending-entity-index order. `chunk_rows`
    /// is how many joined rows one chunk owns.
    ///
    /// # The `&mut` columns are absent from `ctx.world`
    ///
    /// For the duration of the call, every component type the tuple names `&mut` is lifted out of
    /// the world so its dense column can be split into disjoint `&mut` chunks without `unsafe`. A
    /// `ctx.world.get::<T>(other)` for such a `T` therefore is an `L0026` `Err`, and `has`,
    /// `entity_indices_with` and `par_join_fold` over it panic — it is *not* a peek at another
    /// row's value. Components the query does not name `&mut` (including its own `&T`
    /// columns), and every resource, read normally. See the [module docs](crate::par_query) for the
    /// full region model.
    ///
    /// # Determinism
    ///
    /// The result and the resulting world state are **bit-identical at any thread count**. They do
    /// depend on `chunk_rows` only to the extent the closure makes them: `par_for_each` performs no
    /// reduction of its own, so a closure that only writes its own row is `chunk_rows`-invariant
    /// too. `chunk_rows` is still part of the determinism contract at the call site
    /// ([`ludion_core::par`]) — treat a change to it as hash-moving.
    ///
    /// # Panics
    ///
    /// Panics if `chunk_rows` is zero, or if the tuple names the same component type twice (it would
    /// alias one storage), exactly as [`query`](World::query) does. A panic *inside* `f` propagates
    /// with the lowest-index chunk's payload once every chunk has run and been joined, and the
    /// lifted columns are back in the world before it does.
    ///
    /// # Example
    /// ```
    /// use ludion_ecs::World;
    /// use serde::{Deserialize, Serialize};
    /// use schemars::JsonSchema;
    ///
    /// #[derive(Serialize, Deserialize, JsonSchema)]
    /// struct Position { x: f32 }
    /// #[derive(Serialize, Deserialize, JsonSchema)]
    /// struct Velocity { dx: f32 }
    ///
    /// let mut world = World::new();
    /// for i in 0..8 {
    ///     let e = world.spawn();
    ///     world.insert(e, Position { x: 0.0 }).unwrap();
    ///     world.insert(e, Velocity { dx: i as f32 }).unwrap();
    /// }
    ///
    /// // 4 rows per chunk; each row writes only its own Position.
    /// let moved: Vec<f32> = world.par_for_each::<(&mut Position, &Velocity), _>(
    ///     4,
    ///     |_ctx, _entity, (pos, vel)| {
    ///         pos.x += vel.dx;
    ///         pos.x
    ///     },
    /// );
    /// assert_eq!(moved, vec![0.0, 1.0, 2.0, 3.0, 4.0, 5.0, 6.0, 7.0]);
    /// ```
    pub fn par_for_each<Q: ParQuery, R: Send>(
        &mut self,
        chunk_rows: usize,
        f: impl for<'c> Fn(&ParCtx<'c>, EntityId, Q::Item<'c>) -> R + Sync,
    ) -> Vec<R> {
        Q::run_region(self, chunk_rows, &f)
    }

    /// Maps each chunk of the read-only join `Q` to one value in parallel, then folds those values
    /// **sequentially in ascending chunk order**.
    ///
    /// The read-only counterpart of [`par_for_each`](World::par_for_each): every column stays in the
    /// world, so this takes `&self` and the whole world reads normally throughout. `map` receives an
    /// iterator over its chunk's `(EntityId, Q::Item)` rows — ascending entity index, the same rows
    /// [`query`](World::query) would yield.
    ///
    /// # Determinism
    ///
    /// The fold order is the point: float addition is not associative, so per-chunk values must
    /// combine in chunk order however the chunks finished. That makes the result independent of the
    /// thread count and dependent on `chunk_rows` — a different grouping is a different sum — which
    /// is the same contract [`ludion_core::par::ordered_reduce`] carries.
    ///
    /// # Panics
    ///
    /// Panics if `chunk_rows` is zero or if the tuple names the same component type twice; a panic
    /// inside `map` propagates with the lowest chunk index's payload. Called inside a
    /// [`par_for_each`](World::par_for_each) region, a type that region took `&mut` panics with
    /// `L0026`.
    ///
    /// # Example
    /// ```
    /// use ludion_ecs::World;
    /// use serde::{Deserialize, Serialize};
    /// use schemars::JsonSchema;
    ///
    /// #[derive(Serialize, Deserialize, JsonSchema)]
    /// struct Health { hp: f32 }
    ///
    /// let mut world = World::new();
    /// for i in 1..=10 {
    ///     let e = world.spawn();
    ///     world.insert(e, Health { hp: i as f32 }).unwrap();
    /// }
    ///
    /// // Per-chunk subtotals, folded in chunk order.
    /// let total = world.par_join_fold::<(&Health,), f32, f32>(
    ///     4,
    ///     |rows| rows.map(|(_id, (health,))| health.hp).sum(),
    ///     0.0,
    ///     |total, part| total + part,
    /// );
    /// assert_eq!(total, 55.0);
    /// ```
    pub fn par_join_fold<Q: ParReadQuery, A: Send, B>(
        &self,
        chunk_rows: usize,
        map: impl for<'c> Fn(ParRows<'c, Q>) -> A + Sync,
        init: B,
        fold: impl FnMut(B, A) -> B,
    ) -> B {
        assert!(chunk_rows > 0, "par_join_fold: chunk_rows must be non-zero");
        let shape = Q::shape();
        assert_unique_query_types(&shape.iter().map(|e| e.type_id).collect::<Vec<_>>());
        if self.is_recording() {
            self.record_query_shape(shape);
        }

        let window = self.query_window(false);
        let cols = Q::cols(self, &window);
        let join = join_rows(self, &Q::keys(&cols));
        let rows = join.ids.len();
        let ranges: Vec<(usize, usize)> = (0..rows.div_ceil(chunk_rows))
            .map(|k| (k * chunk_rows, ((k + 1) * chunk_rows).min(rows)))
            .collect();

        // `par_map_chunks` returns per-chunk values in ascending chunk order whatever order the
        // chunks finished in; the fold below is what makes the aggregate deterministic.
        par::par_map_chunks(&ranges, 1, |_, range: &[(usize, usize)]| {
            let (start, end) = range[0];
            map(ParRows::<Q> {
                cols,
                join: &join,
                row: start,
                end,
            })
        })
        .into_iter()
        .fold(init, fold)
    }
}
