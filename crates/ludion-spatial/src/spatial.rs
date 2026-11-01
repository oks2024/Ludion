//! The spatial index, its per-frame rebuild system, the pinned-order queries, and the kinematic
//! [`move_and_slide`].
//!
//! [`SpatialIndex`] is a uniform sparse grid of colliders on the two in-plane axes ([`Plane`]; x/z
//! by default), rebuilt each frame by
//! [`rebuild`] (an explicitly-scheduled system — no magic, order stays visible in the game's
//! schedule) and read by [`overlaps`] / [`raycast`] / [`k_nearest`] / [`move_and_slide`]. It is a
//! **derived resource**:
//! outside the world hash, rebuilt from world content every frame, and carried by snapshots as its
//! [`filed_entries`](SpatialIndex::filed_entries) so a restore continues exactly (a system scheduled
//! before the rebuild reads the index the previous tick filed, which the restored poses alone cannot
//! reproduce). Determinism comes from `rebuild` being a pure function of world content visited in
//! `EntityId` order, and from every query walking candidates in a pinned total order (sorted cell
//! key, then `EntityId`).
//!
//! # Pruning
//!
//! [`overlaps`] and [`k_nearest`] visit only the **candidate cells** their probe can reach: the
//! probe's world-space AABB — inflated for [`overlaps`], since entries are filed by *center* —
//! clamped to the occupied cell bounds. Inflation is kept small by two partitions of the entries: a
//! [`Body::Static`] half and a [`Body::Kinematic`] half, so a body-filtered probe is never widened
//! by the other half, and within each half an `oversize` grid holding the map-spanning colliders,
//! so no probe is widened past one cell (those few entries are scanned in full instead). Both
//! partitions are recombined into the one pinned total order — lexicographic `(cx, cz)` cell key,
//! then `EntityId` ascending — by a merge over the four streams, so pruning and the partitions
//! change *which* cells are visited, never the result order (`SpatialIndex::candidates` carries the
//! conservatism and order proofs; asserted against a full scan by the `pruned_*_match_a_full_scan`
//! tests and the conformance suite's brute-force oracle). [`raycast`] deliberately stays a full
//! scan: raycasts are a handful per frame, not hundreds, and at these entry counts the scan is
//! single-digit microseconds — a conservative ray cell-walk is the option if a profile shows the
//! scan. [`CELL_SIZE`] was tuned by the spatial microbench (`tests/e6_bench.rs`).
//!
//! # Frozen query API
//!
//! The public shapes of [`overlaps`] / [`raycast`] / [`k_nearest`] — names, parameters, return
//! types, and the optional [`Body`] filter — are **frozen**, validated by the spatial microbench
//! (`tests/e6_bench.rs`) and locked by the conformance suite (`tests/conformance.rs`): double-run
//! bit-identity, a brute-force O(n²) oracle for membership *and* order, and explicit tie-break
//! cases. [`RayHit`] and [`Entry`] are `#[non_exhaustive]` per the freeze convention for
//! library-built, consumer-read result types; [`Collider`] and its enums stay exhaustively
//! constructible — games build them as literals, exactly like the engine's other components.
//!
//! [`k_nearest_entries`] and [`move_and_slide_at`] are sanctioned additions to the
//! frozen surface (`Plan/deterministic-job-system.md`): the frozen surface bars signature
//! changes, not new entry points. Both are one core implementation shared with the frozen fn they
//! sit beside ([`k_nearest`] delegates to [`k_nearest_entries`]; [`move_and_slide`] delegates to
//! [`move_and_slide_at`]), so the frozen fn's observable behavior is unchanged by construction.

use std::any::TypeId;
use std::iter::Peekable;
use std::slice;

use ludion_core::collections::StableMap;
use ludion_core::ids::EntityId;
use ludion_ecs::{Component, Parent, SystemCtx, World};
use serde::{Deserialize, Serialize};

use crate::collider::{Body, Collider, Shape};
use crate::{GroundPosition, ParentPose, Plane};

/// The uniform grid's cell size, in world units.
///
/// A collider is filed under the single cell containing its center,
/// `(floor(x / CELL_SIZE), floor(z / CELL_SIZE))`. Tuned by the spatial microbench
/// (`tests/e6_bench.rs`) on the pinned machine: the sweep 1→2→3→4→6→8→12→16→24 fell monotonically
/// to 16 and turned back up (0.79 → 0.63 → 0.57 → 0.50 → 0.44 → 0.41 → 0.38 → **0.37** → 0.39 ms
/// frame sum). Contrary to the "cell near the query radius" prior, coarse cells win at the benched
/// density: the `StableMap`'s node traversal costs more than the extra narrow-phase tests a fat
/// cell's linear (cache-friendly) entry scan adds. One named constant so `rebuild` and every query
/// agree on the grid by construction.
pub const CELL_SIZE: f32 = 16.0;

/// The per-axis reach above which a collider is filed in its half's `oversize` grid instead of the
/// cell grid, and scanned by every query of that half rather than pruned against it.
///
/// One cell: a collider whose reach exceeds the grid's own resolution is the map-spanning-wall
/// class, and filing it in the cell grid would raise the whole half's `max_reach` to *its* reach —
/// inflating every probe rectangle of that half by a map, not by a body. Scanning a handful of such
/// entries in full is strictly cheaper than that, and it caps every probe's inflation at one cell.
const OVERSIZE_REACH: f32 = CELL_SIZE;

/// One collider's snapshot in the [`SpatialIndex`]: its id, in-plane center, shape, and body.
///
/// The center is baked in at rebuild time so queries return `EntityId`s (which a raw `World::query`
/// cannot) without re-reading the position component.
///
/// `#[non_exhaustive]` (the freeze convention for library-built, consumer-read result
/// types): only [`rebuild`] constructs entries, downstream code reads fields — so a future added
/// field is a non-breaking change.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct Entry {
    /// The collider's entity.
    pub entity: EntityId,
    /// Center on the two in-plane axes ([`Plane`]; x/z by default), from the entity's
    /// [`GroundPosition`] at rebuild time.
    pub center: [f32; 2],
    /// The collision shape (always [`Shape::Circle`] or [`Shape::Aabb`] — [`rebuild`] skips every
    /// other variant).
    pub shape: Shape,
    /// Static or kinematic (never [`Body::Rigid`] — [`rebuild`] skips it).
    pub body: Body,
}

/// A per-frame broadphase over in-plane colliders: a uniform sparse grid keyed by cell.
///
/// Scratch derived state, rebuilt each frame by [`rebuild`] and read by the queries.
///
/// # Four streams: two halves, each grid plus oversize
///
/// Reach inflation is necessarily global to whatever set it covers — entries are filed by center
/// cell, so a probe must widen its rectangle by the largest reach in the set it walks, and one
/// outlier drags every probe of that set with it. A single map-spanning wall (`half_extents`
/// `[2.0, 70.0]`) in one shared set widens *every* probe by 70 world units until each visits nearly
/// the whole index: the O(entries-per-probe) class that makes a horde quadratic. The index answers
/// that twice over.
///
/// **By body.** [`Body::Static`] and [`Body::Kinematic`] entries live in two independent halves, each
/// with its own cells, reach, and bounds, so a body-filtered probe never pays for the other half —
/// the dynamics-only separation probe cannot be widened by a wall it could never match.
///
/// **By size.** Within a half, an entry whose reach exceeds `OVERSIZE_REACH` is filed in a
/// separate `oversize` grid that queries scan in full, and it contributes to neither `max_reach`
/// nor `bounds`. That caps *every* probe's inflation at one cell — including the statics-admitting
/// probes ([`move_and_slide`], an unfiltered [`overlaps`]) that the body split alone cannot help,
/// since the walls are exactly what those want to find.
///
/// The design is invisible to results: the four streams are recombined into one pinned total order
/// — sorted cell key, then `EntityId` ascending — by `MergedEntries`, and the candidate set stays a
/// conservative superset (`SpatialIndex::candidates` carries the proof).
#[derive(Clone, Debug, Default)]
pub struct SpatialIndex {
    /// The [`Body::Static`] entries.
    statics: HalfIndex,
    /// The [`Body::Kinematic`] entries.
    dynamics: HalfIndex,
}

impl SpatialIndex {
    /// The number of colliders in the index.
    pub fn len(&self) -> usize {
        self.statics.len() + self.dynamics.len()
    }

    /// Whether the index holds no colliders.
    pub fn is_empty(&self) -> bool {
        self.statics.is_empty() && self.dynamics.is_empty()
    }

    /// Every entry, in pinned order: sorted cell key, then insertion (`EntityId` ascending) order.
    fn entries<'a>(
        &'a self,
    ) -> MergedEntries<'a, impl Iterator<Item = CellSlice<'a>>, impl Iterator<Item = CellSlice<'a>>>
    {
        MergedEntries::new(
            [
                all_cells(&self.statics.cells, true),
                all_cells(&self.dynamics.cells, true),
            ],
            [
                all_cells(&self.statics.oversize, true),
                all_cells(&self.dynamics.oversize, true),
            ],
        )
    }

    /// Files one collider into the half its [`Body`] belongs to. The whole of [`rebuild`]'s
    /// per-entity work, in one place.
    fn file(&mut self, entity: EntityId, collider: Collider, center: [f32; 2]) {
        self.file_entry(Entry {
            entity,
            center,
            shape: collider.shape,
            body: collider.body,
        });
    }

    fn file_entry(&mut self, entry: Entry) {
        match entry.body {
            Body::Static(_) => self.statics.file(entry),
            Body::Kinematic(_) => self.dynamics.file(entry),
            // `rebuild` skips every `Rigid` collider before ever calling `file` (its doc comment
            // carries the rule); a no-op here is the defensive twin of that filter, never reached.
            Body::Rigid(_) => {}
        }
    }

    /// Every filed entry, sorted by `EntityId` — the form a snapshot carries. The index is a pure
    /// function of its entries (each cell's vec is `EntityId`-ascending because [`rebuild`] files in
    /// that order), so [`from_entries`](SpatialIndex::from_entries) over this list reproduces it.
    pub fn filed_entries(&self) -> Vec<Entry> {
        let mut entries: Vec<Entry> = self.entries().cloned().collect();
        entries.sort_by_key(|entry| entry.entity);
        entries
    }

    /// Files `entries` in the order given, skipping a [`Body::Rigid`] one as [`rebuild`] does. Pass
    /// [`filed_entries`](SpatialIndex::filed_entries)' order to reproduce an index exactly.
    pub fn from_entries(entries: impl IntoIterator<Item = Entry>) -> SpatialIndex {
        let mut index = SpatialIndex::default();
        for entry in entries {
            index.file_entry(entry);
        }
        index
    }

    /// Every entry that could match a probe covering the world-space rectangle `[min, max]`, in the
    /// pinned order a full scan yields, restricted to the half `body` admits (`None` walks both).
    ///
    /// Callers pass the **tight** rectangle — the probe shape's own AABB. `pad_by_reach` asks each
    /// admitted half to inflate it by *that half's* `max_reach` (what an overlap test needs, since
    /// entries are filed by center cell); a query that measures to entry *centers* passes `false`
    /// and gets the exact rectangle. Each half then clamps to its own occupied bounds, so the walk
    /// is bounded by the world, not the probe (`f32 as i32` saturates on overflow/infinities, and
    /// the clamp then reins the range in). Each admitted half's `oversize` grid is scanned in full
    /// on top of that, unpruned.
    ///
    /// # Why the candidate set is conservative
    ///
    /// Two shapes overlap only if, per axis, their center gap is at most the sum of their reaches
    /// (true for all three closed-form cases). For a *regular* entry that bound reads
    /// `gap <= probe reach + entry reach <= probe reach + that set's max_reach`, because a set's
    /// `max_reach` is the maximum over exactly the entries filed in its grid (the oversize ones are
    /// excluded from it, and from the grid). So a set's rectangle, inflated by its own `max_reach`,
    /// still contains the center cell of every *regular* entry of that set which can overlap the
    /// probe — the pruning guarantee, per set. Nothing else is lost: an entry a narrower rectangle
    /// no longer reaches is either in the other half (walked under its own inflation) or oversize
    /// (walked in full, never pruned). When `body` excludes a half entirely, the entries dropped are
    /// exactly those the caller's body predicate would have rejected. Survivor set identical.
    ///
    /// # Why the order is the pinned order
    ///
    /// Each of the four streams is sorted by cell key, then by `EntityId` within a cell (every set
    /// is filed from the same ascending-`EntityId` join). An `EntityId` appears in exactly one
    /// stream — one body, one size class — so a merge by (cell key, `EntityId`) over the four is a
    /// total order with no ties, and it is by definition the order a full scan of the union yields.
    /// `MergedEntries` performs exactly that merge.
    fn candidates<'a>(
        &'a self,
        min: [f32; 2],
        max: [f32; 2],
        body: Option<Body>,
        pad_by_reach: bool,
    ) -> MergedEntries<'a, impl Iterator<Item = CellSlice<'a>>, impl Iterator<Item = CellSlice<'a>>>
    {
        let rect = |admitted: bool, half: &HalfIndex| {
            admitted.then(|| {
                let pad = if pad_by_reach { half.max_reach } else { 0.0 };
                ([min[0] - pad, min[1] - pad], [max[0] + pad, max[1] + pad])
            })
        };
        let wants_statics = body.is_none_or(|wanted| matches!(wanted, Body::Static(_)));
        let wants_dynamics = body.is_none_or(|wanted| matches!(wanted, Body::Kinematic(_)));
        MergedEntries::new(
            [
                self.statics
                    .candidate_cells(rect(wants_statics, &self.statics)),
                self.dynamics
                    .candidate_cells(rect(wants_dynamics, &self.dynamics)),
            ],
            [
                all_cells(&self.statics.oversize, wants_statics),
                all_cells(&self.dynamics.oversize, wants_dynamics),
            ],
        )
    }
}

/// One [`Body`] class's half of a [`SpatialIndex`]: a pruned grid, its reach and bounds, and the
/// oversize grid that is scanned in full.
#[derive(Clone, Debug, Default)]
struct HalfIndex {
    /// This half's entries of reach `<=` [`OVERSIZE_REACH`], by cell key. Cells iterate in sorted
    /// key order ([`StableMap`]); within a cell, entries are in insertion order, which is
    /// `world.entities()` order = `EntityId` ascending.
    cells: StableMap<(i32, i32), Vec<Entry>>,
    /// This half's entries of reach `>` [`OVERSIZE_REACH`], filed and ordered exactly like `cells`
    /// but kept out of `max_reach` and `bounds`: queries scan this grid whole rather than let one
    /// map-spanning collider widen every probe of the half.
    oversize: StableMap<(i32, i32), Vec<Entry>>,
    /// The largest per-axis reach (center-to-edge distance) over the entries in `cells` — a
    /// circle's radius, an AABB's larger half-extent; `0.0` when empty, and never more than
    /// [`OVERSIZE_REACH`]. Entries are filed by *center* cell only, so a query must inflate its
    /// search rectangle by this to catch a large collider whose center sits in a far cell.
    max_reach: f32,
    /// Occupied cell-key bounds of `cells`, `((min_cx, min_cz), (max_cx, max_cz))`, `None` when
    /// empty. Clamps a candidate rectangle so a huge probe radius never walks empty cells outside
    /// the world.
    bounds: Option<((i32, i32), (i32, i32))>,
}

impl HalfIndex {
    /// The number of colliders in this half.
    fn len(&self) -> usize {
        let count =
            |map: &StableMap<(i32, i32), Vec<Entry>>| map.values().map(Vec::len).sum::<usize>();
        count(&self.cells) + count(&self.oversize)
    }

    /// Whether this half holds no colliders.
    fn is_empty(&self) -> bool {
        let empty = |map: &StableMap<(i32, i32), Vec<Entry>>| {
            map.iter().all(|(_, entries)| entries.is_empty())
        };
        empty(&self.cells) && empty(&self.oversize)
    }

    /// Files one collider under its center's cell — in `oversize` when its reach exceeds
    /// [`OVERSIZE_REACH`], otherwise in `cells`, where it also feeds `max_reach` and `bounds`.
    fn file(&mut self, entry: Entry) {
        let cell = cell_of(entry.center);
        let reach = shape_reach(&entry.shape);
        if reach[0].max(reach[1]) > OVERSIZE_REACH {
            self.oversize.entry(cell).or_default().push(entry);
            return;
        }
        self.cells.entry(cell).or_default().push(entry);
        self.max_reach = self.max_reach.max(reach[0]).max(reach[1]);
        self.bounds = Some(match self.bounds {
            None => (cell, cell),
            Some((lo, hi)) => (
                (lo.0.min(cell.0), lo.1.min(cell.1)),
                (hi.0.max(cell.0), hi.1.max(cell.1)),
            ),
        });
    }

    /// This half's `cells` inside the world-space rectangle `rect`, in sorted key order, clamped to
    /// this half's occupied bounds. `None` — the half is excluded by a body filter — yields nothing.
    ///
    /// `rect` is threaded through the same `Option` chain the bounds check already walks, so the
    /// excluded case is the *same* opaque iterator type as the admitted one: the merge's two type
    /// parameters each stay bound to one call of this method, no `Either` and no boxing on a path
    /// that runs per entry.
    fn candidate_cells<'a>(
        &'a self,
        rect: Option<([f32; 2], [f32; 2])>,
    ) -> impl Iterator<Item = CellSlice<'a>> {
        let range = rect
            .zip(self.bounds)
            .and_then(|((min, max), (lo_bound, hi_bound))| {
                let lo = cell_of(min);
                let hi = cell_of(max);
                let cx0 = lo.0.max(lo_bound.0);
                let cx1 = hi.0.min(hi_bound.0);
                let cz0 = lo.1.max(lo_bound.1);
                let cz1 = hi.1.min(hi_bound.1);
                (cx0 <= cx1 && cz0 <= cz1).then_some((cx0..=cx1, cz0, cz1))
            });
        range.into_iter().flat_map(move |(columns, cz0, cz1)| {
            columns.flat_map(move |cx| {
                self.cells
                    .as_inner()
                    .range((cx, cz0)..=(cx, cz1))
                    .map(|(key, entries)| (*key, entries.as_slice()))
            })
        })
    }
}

/// One cell's key and its entries — the unit every cell stream yields.
type CellSlice<'a> = ((i32, i32), &'a [Entry]);

/// The empty entry slice a drained or excluded stream stands in with.
const NO_ENTRIES: &[Entry] = &[];

/// The index's four cell streams — each half's pruned grid and its unpruned `oversize` grid —
/// merged into one entry stream in the pinned total order: ascending cell key, then ascending
/// `EntityId` within a cell.
///
/// Each stream is walked under its own rules (a regular one pruned to its set's inflated rectangle
/// and clamped to its set's bounds, an oversize one in full), so a cell key can appear in any
/// subset of them. The merge takes the smallest pending key, opens every stream holding it, and
/// drains those cells by ascending `entity`. Each set is filed from the same ascending-`EntityId`
/// join, so every cell is already sorted, and an `EntityId` appears in exactly one stream — one
/// [`Body`], one size class — so the merge is a total order with no ties and reproduces exactly
/// what a full scan of the union yields.
///
/// The common case is one stream per key (a body-filtered probe, an empty `oversize`); `Cursor`
/// keeps that a plain slice drain with no per-entry comparison.
struct MergedEntries<'a, A: Iterator, B: Iterator> {
    /// The two halves' pruned grid streams, statics first.
    regular: [Peekable<A>; 2],
    /// The two halves' `oversize` streams, statics first.
    oversize: [Peekable<B>; 2],
    cursor: Cursor<'a>,
}

/// The cells under one key that a `MergedEntries` is currently draining.
enum Cursor<'a> {
    /// Nothing in hand: the next `next` refills from the cell streams.
    Empty,
    /// Exactly one stream held this cell key — drain it in order, no per-entry comparison.
    One(slice::Iter<'a, Entry>),
    /// Several streams held this cell key: the first `usize` of the cursors, merged by ascending
    /// `entity`.
    ///
    /// Deliberately raw [`slice::Iter`]s, not `Peekable`s: this is the only per-*entry* comparison
    /// on the candidate path, and `slice::Iter::as_slice().first()` peeks with a load where
    /// `Peekable` maintains an `Option<Option<&Entry>>` it must write on every peek.
    Many([slice::Iter<'a, Entry>; 4], usize),
}

/// Every occupied cell of `map`, in sorted key order; `admitted == false` yields nothing.
///
/// The gate rides an `Option` layer rather than branching to a different iterator, so an excluded
/// stream is the *same* opaque type as an admitted one — each of the merge's type parameters stays
/// bound to one call site, no `Either` and no boxing on a path that runs per entry.
fn all_cells(
    map: &StableMap<(i32, i32), Vec<Entry>>,
    admitted: bool,
) -> impl Iterator<Item = CellSlice<'_>> {
    admitted
        .then_some(map)
        .into_iter()
        .flat_map(|map| map.iter().map(|(key, entries)| (*key, entries.as_slice())))
}

/// The entries of a cell a stream yielded, or an empty iterator when the stream is exhausted (which
/// the merge never asks for: it only takes a cell it has peeked).
fn cell_entries(cell: Option<CellSlice<'_>>) -> slice::Iter<'_, Entry> {
    cell.map_or(NO_ENTRIES.iter(), |(_, entries)| entries.iter())
}

impl<'a, A, B> MergedEntries<'a, A, B>
where
    A: Iterator<Item = CellSlice<'a>>,
    B: Iterator<Item = CellSlice<'a>>,
{
    /// Merges the two halves' pruned grid streams with their two `oversize` streams, statics first
    /// in each pair. Every stream must be in ascending cell-key order, with each cell's entries in
    /// ascending `EntityId` order.
    fn new(regular: [A; 2], oversize: [B; 2]) -> Self {
        Self {
            regular: regular.map(Iterator::peekable),
            oversize: oversize.map(Iterator::peekable),
            cursor: Cursor::Empty,
        }
    }

    /// Opens every stream holding the smallest pending cell key, or reports all four exhausted.
    fn refill(&mut self) -> Option<()> {
        let mut key: Option<(i32, i32)> = None;
        let mut smallest = |peeked: Option<&CellSlice<'a>>| {
            if let Some((pending, _)) = peeked {
                key = Some(key.map_or(*pending, |best| best.min(*pending)));
            }
        };
        for stream in &mut self.regular {
            smallest(stream.peek());
        }
        for stream in &mut self.oversize {
            smallest(stream.peek());
        }
        let key = key?;

        let mut cells = std::array::from_fn(|_| NO_ENTRIES.iter());
        let mut open = 0;
        for stream in &mut self.regular {
            if stream.peek().is_some_and(|(pending, _)| *pending == key) {
                cells[open] = cell_entries(stream.next());
                open += 1;
            }
        }
        for stream in &mut self.oversize {
            if stream.peek().is_some_and(|(pending, _)| *pending == key) {
                cells[open] = cell_entries(stream.next());
                open += 1;
            }
        }
        self.cursor = if open == 1 {
            // The common case by far — a body-filtered probe, or an empty `oversize` — and the one
            // that must stay a plain slice drain with no per-entry comparison.
            Cursor::One(cells[0].clone())
        } else {
            Cursor::Many(cells, open)
        };
        Some(())
    }
}

impl<'a, A, B> Iterator for MergedEntries<'a, A, B>
where
    A: Iterator<Item = CellSlice<'a>>,
    B: Iterator<Item = CellSlice<'a>>,
{
    type Item = &'a Entry;

    fn next(&mut self) -> Option<&'a Entry> {
        loop {
            match &mut self.cursor {
                Cursor::Empty => {}
                Cursor::One(entries) => {
                    if let Some(entry) = entries.next() {
                        return Some(entry);
                    }
                }
                Cursor::Many(cells, open) => {
                    // `as_slice` borrows from the *slice*, not from the iterator, so every peek
                    // stays live while one of the cursors is advanced.
                    let mut lowest: Option<(usize, EntityId)> = None;
                    for (slot, cell) in cells[..*open].iter().enumerate() {
                        if let Some(entry) = cell.as_slice().first()
                            && lowest.is_none_or(|(_, id)| entry.entity < id)
                        {
                            lowest = Some((slot, entry.entity));
                        }
                    }
                    if let Some((slot, _)) = lowest {
                        return cells[slot].next();
                    }
                }
            }
            self.refill()?;
        }
    }
}

/// The per-axis reach (center-to-edge distance) of a shape, clamped non-negative so a degenerate
/// probe still yields a conservative (never inverted) search rectangle.
///
/// Only [`Shape::Circle`] and [`Shape::Aabb`] ever reach the 2D index or a query's probe shape in
/// practice (`rebuild` skips every 3D shape); a 3D shape here is defensive-only and reaches zero, the
/// same conservative answer an empty collider would.
fn shape_reach(shape: &Shape) -> [f32; 2] {
    match shape {
        Shape::Circle { radius } => [radius.max(0.0); 2],
        Shape::Aabb { half_extents } => [half_extents[0].max(0.0), half_extents[1].max(0.0)],
        Shape::Sphere { .. }
        | Shape::Box { .. }
        | Shape::Capsule { .. }
        | Shape::Cylinder { .. }
        | Shape::Convex { .. }
        | Shape::Heightfield { .. }
        | Shape::Compound { .. }
        | Shape::Mesh { .. } => [0.0, 0.0],
    }
}

/// The cell a ground-plane point falls in.
fn cell_of(point: [f32; 2]) -> (i32, i32) {
    (
        (point[0] / CELL_SIZE).floor() as i32,
        (point[1] / CELL_SIZE).floor() as i32,
    )
}

/// The nearest in-plane hit of a ray against a collider.
///
/// `#[non_exhaustive]` (the freeze convention for library-built, consumer-read result
/// types): only [`raycast`] constructs hits, downstream code reads fields — so a future added field
/// (e.g. a surface normal) is a non-breaking change.
#[derive(Clone, Copy, Debug, PartialEq)]
#[non_exhaustive]
pub struct RayHit {
    /// The collider that was hit.
    pub entity: EntityId,
    /// The ray parameter of the hit: the hit point is `origin + t * dir`.
    pub t: f32,
    /// The world-space hit point on the two in-plane axes ([`Plane`]; x/z by default).
    pub point: [f32; 2],
}

/// `entity`'s ground center from the resolver's world pose, `None` for a root.
fn parent_ground<R: ParentPose>(world: &World, entity: EntityId, plane: Plane) -> Option<[f32; 2]> {
    let axes = plane.axes();
    R::world_pose(world, entity).map(|(t, _)| [t[axes[0]], t[axes[1]]])
}

/// Rebuilds the [`SpatialIndex`] resource from every entity carrying a [`Collider`] and a position
/// component `P`.
///
/// A game registers a concrete instantiation directly as a system —
/// `.system("spatial_rebuild", spatial::rebuild::<Transform, TransformHierarchy>)` — at a chosen schedule position (the
/// monomorphized function has the exact `fn(&mut World, &SystemCtx)` shape a system requires).
/// The `(Collider, P)` join is visited in ascending `EntityId` order, so each cell's entries end up
/// `EntityId`-ascending, and the whole rebuild is a pure function of world content.
///
/// **Deliberately sequential.** A [`World::par_join_fold`] region measured slower:
/// the join is exactly the right shape for chunking, but at ~0.4 ms the per-chunk partial indices
/// and their merge cost more than the parallelism returns (measured 0.399 ms at one thread against
/// 0.848 ms at eight — `Plan/deterministic-job-system.md`). The typed join is what survived: it
/// rides the dense column slices instead of probing two storages per live entity the way the
/// original `entities()` walk did.
pub fn rebuild<P: Component + GroundPosition, R: ParentPose>(world: &mut World, _ctx: &SystemCtx) {
    // Read once per rebuild, not per entity: the plane is a game-lifetime constant.
    let plane = Plane::of(world);
    // Hoisted like the plane: a world with no `Parent` pays one probe per rebuild, nothing per entity.
    let parented = world
        .entity_indices_with(TypeId::of::<Parent>())
        .next()
        .is_some();
    let mut index = SpatialIndex::default();
    if parented {
        // `query_ids` borrows the world mutably, so the world poses are read in a second pass.
        let mut filed: Vec<(EntityId, Collider, [f32; 2])> = Vec::new();
        for (entity, (collider, position)) in world.query_ids::<(&Collider, &P)>() {
            if collider.is_solver_owned() {
                continue;
            }
            filed.push((entity, collider.clone(), position.ground_position(plane)));
        }
        for (entity, collider, center) in filed {
            let center = parent_ground::<R>(world, entity, plane).unwrap_or(center);
            index.file(entity, collider, center);
        }
    } else {
        for (entity, (collider, position)) in world.query_ids::<(&Collider, &P)>() {
            // A physics scene: a 3D shape or a rigid body never enters the 2D index, so it indexes
            // nothing for that entity rather than indexing a lie (`ludion-physics` owns it instead).
            if collider.is_solver_owned() {
                continue;
            }
            index.file(entity, collider.clone(), position.ground_position(plane));
        }
    }
    world.insert_resource(index);
}

/// Whether `body` (an optional filter) admits `entry`; the contact material is ignored.
///
/// The pruned queries select halves instead, so this is the filter for the walks that see both
/// halves interleaved: [`raycast`]'s full scan, and the in-crate full-scan test oracles.
fn body_admits(filter: Option<Body>, entry: &Entry) -> bool {
    filter
        .is_none_or(|wanted| std::mem::discriminant(&wanted) == std::mem::discriminant(&entry.body))
}

/// Reads the index resource, or `None` before the first [`rebuild`].
fn index(world: &World) -> Option<&SpatialIndex> {
    world.resource::<SpatialIndex>()
}

/// Every collider whose shape overlaps the probe (`shape` at `center`), in pinned order (cell key,
/// then `EntityId`). Optionally filtered to one [`Body`] kind (the contact material is ignored).
///
/// Returns empty before the first [`rebuild`] (a legitimate "before first rebuild" state, not an
/// error). This is the projectile hit-test and the general overlap primitive.
pub fn overlaps(
    world: &World,
    shape: Shape,
    center: [f32; 2],
    body: Option<Body>,
) -> Vec<EntityId> {
    let Some(index) = index(world) else {
        return Vec::new();
    };
    // The tight probe AABB; each admitted half inflates it by its own largest entry reach, which is
    // conservative for that half (the proof is on `SpatialIndex::candidates`). The half selection is
    // the body filter, so only the narrow-phase test remains here.
    let reach = shape_reach(&shape);
    index
        .candidates(
            [center[0] - reach[0], center[1] - reach[1]],
            [center[0] + reach[0], center[1] + reach[1]],
            body,
            true,
        )
        .filter(|entry| shapes_overlap(&shape, center, &entry.shape, entry.center))
        .map(|entry| entry.entity)
        .collect()
}

/// Whether [`overlaps`] would be non-empty, without allocating.
pub fn overlaps_any(world: &World, shape: Shape, center: [f32; 2], body: Option<Body>) -> bool {
    let Some(index) = index(world) else {
        return false;
    };
    let reach = shape_reach(&shape);
    index
        .candidates(
            [center[0] - reach[0], center[1] - reach[1]],
            [center[0] + reach[0], center[1] + reach[1]],
            body,
            true,
        )
        .any(|entry| shapes_overlap(&shape, center, &entry.shape, entry.center))
}

/// The nearest in-plane hit of the ray `origin + t * dir` (`t >= 0`) against any collider, ties
/// broken by `EntityId`. Optionally filtered to one [`Body`]. `None` if nothing is hit (or before
/// the first [`rebuild`]).
///
/// Deliberately a full scan (no candidate-cell pruning): raycasts are not on the benched hot
/// path — a handful of line-of-sight tests per frame, not hundreds — and at these entry counts
/// the scan measures in single-digit microseconds. A conservative ray cell-walk (DDA inflated by
/// `max_reach`) is the option if a profile demands one; the result contract would not change.
pub fn raycast(
    world: &World,
    origin: [f32; 2],
    dir: [f32; 2],
    body: Option<Body>,
) -> Option<RayHit> {
    let index = index(world)?;
    let mut best: Option<RayHit> = None;
    for entry in index.entries().filter(|entry| body_admits(body, entry)) {
        let Some(t) = ray_shape_t(origin, dir, &entry.shape, entry.center) else {
            continue;
        };
        // Nearest hit, ties broken by EntityId (cell order alone is not global EntityId order).
        // `total_cmp` gives Equal only on bit-identical `t`, which is exactly the tie case.
        let closer = match best {
            None => true,
            Some(current) => match t.total_cmp(&current.t) {
                std::cmp::Ordering::Less => true,
                std::cmp::Ordering::Equal => entry.entity < current.entity,
                std::cmp::Ordering::Greater => false,
            },
        };
        if closer {
            best = Some(RayHit {
                entity: entry.entity,
                t,
                point: [origin[0] + t * dir[0], origin[1] + t * dir[1]],
            });
        }
    }
    best
}

/// The `k` colliders whose centers are closest to `center` and within `max_radius`, ordered by
/// (squared distance, `EntityId`). Optionally filtered to one [`Body`]. Fewer than `k` if fewer
/// qualify; empty before the first [`rebuild`]. This is separation, aura pickups, and nearest-enemy
/// targeting.
///
/// Delegates to [`k_nearest_entries`] and maps to ids — one implementation, two entry points.
pub fn k_nearest(
    world: &World,
    center: [f32; 2],
    k: usize,
    max_radius: f32,
    body: Option<Body>,
) -> Vec<EntityId> {
    k_nearest_entries(world, center, k, max_radius, body)
        .into_iter()
        .map(|entry| entry.entity)
        .collect()
}

/// Same query as [`k_nearest`], returning the full [`Entry`] (id, center, shape, body) instead of
/// just the id, in exactly the same (squared distance, `EntityId`) order.
///
/// The center on each returned `Entry` is the **frozen rebuild-time position** baked in by
/// [`rebuild`] — that's the point: a caller in a parallel region can consume a neighbor's
/// position without re-reading it from the world, where the neighbor's own position column may be
/// checked out under a disjoint `&mut` chunk. Callers that need a live position must still read it
/// from the world themselves.
///
/// `#[inline]` for the same reason as [`move_and_slide_at`]: non-generic and on the per-entity hot
/// path of every caller, it must not become an opaque cross-crate call.
#[inline]
pub fn k_nearest_entries(
    world: &World,
    center: [f32; 2],
    k: usize,
    max_radius: f32,
    body: Option<Body>,
) -> Vec<Entry> {
    let Some(index) = index(world) else {
        return Vec::new();
    };
    let max_sq = max_radius * max_radius;
    // k_nearest measures distance to entry *centers*, so the candidate rectangle needs no
    // max-reach inflation — only cells within `max_radius` of the probe can hold a qualifying
    // center. `abs()` mirrors the squared-distance test, which is sign-blind on `max_radius`.
    let reach = max_radius.abs();
    // The sort runs over slim keys, not over `Entry` values: this is the hottest inner loop in the
    // game (the horde's separation probe), and an `Entry` is ~4x the payload of the two fields the
    // comparator actually reads. Qualifying entries land in `found` in candidate order; a key
    // carries the slot that addresses one. Selection and order are unchanged — the comparator is
    // still (squared distance, `EntityId`), never the slot, because candidate order is cell order,
    // not `EntityId` order.
    let mut found: Vec<Entry> = Vec::new();
    let mut keys: Vec<(f32, EntityId, u32)> = Vec::new();
    for entry in index.candidates(
        [center[0] - reach, center[1] - reach],
        [center[0] + reach, center[1] + reach],
        body,
        false,
    ) {
        let sq = distance_squared(center, entry.center);
        if sq <= max_sq {
            keys.push((sq, entry.entity, found.len() as u32));
            found.push(entry.clone());
        }
    }
    // Squared distance is deterministic in f32 (not exact); total_cmp gives a total order (never a
    // NaN-unwrap) and, for finite values, matches numeric order. Ties break by EntityId.
    keys.sort_by(|a, b| a.0.total_cmp(&b.0).then(a.1.cmp(&b.1)));
    keys.into_iter()
        .take(k)
        .map(|(_, _, slot)| found[slot as usize].clone())
        .collect()
}

/// The resolved in-plane delta for a kinematic move of `entity`, swept against every
/// [`Body::Static`] collider in the index (never a `Kinematic` — mover-vs-mover response is game
/// logic built on [`k_nearest`], per the settled scope).
///
/// A **pure read**: nothing in the world is mutated. The game applies the returned delta to its
/// position component's two in-plane axes ([`Plane`]; `Transform.translation` x/z by default)
/// itself, keeping the only hashed write in game code, on the game's visible schedule. Like
/// [`rebuild`], the mover's position component is supplied through [`GroundPosition`] — call it as
/// `move_and_slide::<Transform, TransformHierarchy>(…)`.
///
/// # Resolution: axis-separated test-and-cancel (exact order of operations)
///
/// Each candidate end position is tested for overlap against the statics (an endpoint test — no
/// continuous sweep; at 60 Hz kinematic deltas this is the standard, obviously-correct resolution).
/// `x` and `z` below name the plane's two in-plane axes ([`Plane`]; x/z by default):
///
/// 1. the **full delta** `[dx, dz]` — free ⇒ returned unchanged (covers "no statics in range");
/// 2. **x only** `[dx, 0]` (the z axis cancelled) — free ⇒ `[dx, 0.0]`;
/// 3. **z only** `[0, dz]` (the x axis cancelled) — free ⇒ `[0.0, dz]`;
/// 4. both axes blocked (a corner) ⇒ `[0.0, 0.0]`.
///
/// The x axis is deliberately preferred when either single axis would do — a fixed, documented
/// bias, not an ordering accident. Sliding along a wall falls out of steps 2/3: the tangential
/// component survives, the normal component cancels. A mover that already starts overlapping a
/// static is **not** depenetrated: every candidate fails and the delta resolves to zero — this
/// function resolves motion, never pre-existing penetration.
///
/// # Edge cases (documented, not errors)
///
/// - `entity` has no [`Collider`] or no `P` position → the delta returns unchanged (a non-colliding
///   mover is legal).
/// - Empty index / before the first [`rebuild`] → the delta returns unchanged.
/// - The mover's **own index entry is excluded by `EntityId`** — an entity never collides with
///   itself, even when it is itself `Static` (the index snapshot may also hold a stale copy of the
///   mover at its pre-move position; identity, not position, is the correct exclusion key).
///
/// # Determinism
///
/// Statics are tested in the pinned candidate order (cell key, then `EntityId`) via the same
/// pruned candidate rectangle as [`overlaps`] — each end position's shape AABB, inflated by the
/// statics half's `max_reach`. A blocked/free answer is an any-hit, so pruning provably cannot
/// change it (the candidate set is conservative), and the same world + the same delta always
/// resolve to the same bits.
///
/// Reads the mover's center from `P` (its world center when it sits under a parent, so the returned
/// delta is in world plane coordinates), then delegates to [`move_and_slide_at`] — one
/// implementation, two entry points. A caller adding the delta to a parented mover's local
/// `Transform` must first convert it into the parent's frame (or keep movers as roots).
pub fn move_and_slide<P: Component + GroundPosition, R: ParentPose>(
    world: &World,
    entity: EntityId,
    delta: [f32; 2],
) -> [f32; 2] {
    let Ok(Some(position)) = world.get::<P>(entity) else {
        return delta;
    };
    let plane = Plane::of(world);
    let center =
        parent_ground::<R>(world, entity, plane).unwrap_or_else(|| position.ground_position(plane));
    move_and_slide_at(world, entity, center, delta)
}

/// Same resolution as [`move_and_slide`] — identical axis-separated test-and-cancel, identical
/// x-axis bias, statics only, mover excluded by `EntityId` — except `center` comes from the
/// parameter instead of being read from a position component `P`.
///
/// This is the parallel-region entry point: the mover's position column may be checked out of the
/// world under a disjoint `&mut` chunk, so the caller reads its own center once (outside the
/// world) and passes it in. The mover's [`Collider`] is still read from the world for its shape —
/// shape is immutable game data, not a per-frame chunked column.
///
/// `#[inline]` is load-bearing: unlike the generic [`move_and_slide`], this is non-generic, so
/// without it (and without LTO) the candidate scan compiles as an opaque cross-crate call and the
/// hot loop loses its call-site context — measured 2.3× slower per candidate entry at stress scale.
#[inline]
pub fn move_and_slide_at(
    world: &World,
    entity: EntityId,
    center: [f32; 2],
    delta: [f32; 2],
) -> [f32; 2] {
    let Some(index) = index(world) else {
        return delta;
    };
    let Ok(Some(collider)) = world.get::<Collider>(entity) else {
        return delta;
    };
    let shape = &collider.shape;

    let blocked = |end: [f32; 2]| -> bool {
        let reach = shape_reach(shape);
        // Statics only — asking for that half *is* the body test, and it keeps the walk off the
        // dynamics half entirely (which at horde scale is nearly the whole index).
        index
            .candidates(
                [end[0] - reach[0], end[1] - reach[1]],
                [end[0] + reach[0], end[1] + reach[1]],
                Some(Body::new_static()),
                true,
            )
            .any(|entry| {
                entry.entity != entity && shapes_penetrate(shape, end, &entry.shape, entry.center)
            })
    };

    let full = [center[0] + delta[0], center[1] + delta[1]];
    if !blocked(full) {
        return delta;
    }
    let x_only = [center[0] + delta[0], center[1]];
    if !blocked(x_only) {
        return [delta[0], 0.0];
    }
    let z_only = [center[0], center[1] + delta[1]];
    if !blocked(z_only) {
        return [0.0, delta[1]];
    }
    [0.0, 0.0]
}

/// Squared Euclidean distance between two ground-plane points: rounded `f32` arithmetic in a fixed
/// order with no fused multiply-add, so the same inputs give the same bits on every platform.
fn distance_squared(a: [f32; 2], b: [f32; 2]) -> f32 {
    let dx = a[0] - b[0];
    let dz = a[1] - b[1];
    dx * dx + dz * dz
}

/// Whether shape `a` (at `ac`) overlaps shape `b` (at `bc`) — the three closed-form cases.
///
/// Only the four `circle`/`aabb` combinations are meaningful (`rebuild` skips every 3D shape before
/// it ever reaches an index entry); any combination involving a 3D shape is defensive-only and
/// answers `false`, the conservative "does not overlap" rather than a panic.
fn shapes_overlap(a: &Shape, ac: [f32; 2], b: &Shape, bc: [f32; 2]) -> bool {
    shapes_meet(a, ac, b, bc, true)
}

/// Whether `a` and `b` interpenetrate: [`shapes_overlap`] with exact contact (touching edges or a
/// tangent circle) not counted, so a mover resting against a wall is not blocked.
fn shapes_penetrate(a: &Shape, ac: [f32; 2], b: &Shape, bc: [f32; 2]) -> bool {
    shapes_meet(a, ac, b, bc, false)
}

/// `gap <= reach` when `touching`, else `gap < reach`.
fn within(gap: f32, reach: f32, touching: bool) -> bool {
    if touching { gap <= reach } else { gap < reach }
}

fn shapes_meet(a: &Shape, ac: [f32; 2], b: &Shape, bc: [f32; 2], touching: bool) -> bool {
    match (a, b) {
        (Shape::Circle { radius: ra }, Shape::Circle { radius: rb }) => {
            within(distance_squared(ac, bc), (ra + rb) * (ra + rb), touching)
        }
        (Shape::Aabb { half_extents: ha }, Shape::Aabb { half_extents: hb }) => {
            within((ac[0] - bc[0]).abs(), ha[0] + hb[0], touching)
                && within((ac[1] - bc[1]).abs(), ha[1] + hb[1], touching)
        }
        (Shape::Circle { radius }, Shape::Aabb { half_extents }) => {
            circle_aabb_meet(ac, *radius, bc, *half_extents, touching)
        }
        (Shape::Aabb { half_extents }, Shape::Circle { radius }) => {
            circle_aabb_meet(bc, *radius, ac, *half_extents, touching)
        }
        _ => false,
    }
}

/// Whether a circle (center `cc`, radius `r`) meets an AABB (center `bc`, half-extents `he`):
/// the circle center's distance to the box's nearest point is within the radius.
fn circle_aabb_meet(cc: [f32; 2], r: f32, bc: [f32; 2], he: [f32; 2], touching: bool) -> bool {
    let nearest_x = cc[0].clamp(bc[0] - he[0], bc[0] + he[0]);
    let nearest_z = cc[1].clamp(bc[1] - he[1], bc[1] + he[1]);
    within(
        distance_squared(cc, [nearest_x, nearest_z]),
        r * r,
        touching,
    )
}

/// The smallest non-negative ray parameter `t` at which `origin + t * dir` meets the shape, or
/// `None` if the ray misses. A ray that starts inside the shape hits at `t = 0`.
///
/// A 3D shape never reaches an index entry (`rebuild` skips it); `None` (a miss) is this defensive
/// case's answer.
fn ray_shape_t(origin: [f32; 2], dir: [f32; 2], shape: &Shape, center: [f32; 2]) -> Option<f32> {
    match shape {
        Shape::Circle { radius } => ray_circle_t(origin, dir, center, *radius),
        Shape::Aabb { half_extents } => ray_aabb_t(origin, dir, center, *half_extents),
        Shape::Sphere { .. }
        | Shape::Box { .. }
        | Shape::Capsule { .. }
        | Shape::Cylinder { .. }
        | Shape::Convex { .. }
        | Shape::Heightfield { .. }
        | Shape::Compound { .. }
        | Shape::Mesh { .. } => None,
    }
}

/// Smallest non-negative `t` where the ray meets a circle (quadratic in `t`).
fn ray_circle_t(origin: [f32; 2], dir: [f32; 2], center: [f32; 2], radius: f32) -> Option<f32> {
    let ox = origin[0] - center[0];
    let oz = origin[1] - center[1];
    let a = dir[0] * dir[0] + dir[1] * dir[1];
    if a == 0.0 {
        // Degenerate (zero-length) ray: a hit only if the origin is already inside the circle.
        return (ox * ox + oz * oz <= radius * radius).then_some(0.0);
    }
    let b = 2.0 * (ox * dir[0] + oz * dir[1]);
    let c = ox * ox + oz * oz - radius * radius;
    let disc = b * b - 4.0 * a * c;
    if disc < 0.0 {
        return None;
    }
    let sqrt_disc = ludion_core::math::sqrt(disc);
    let t0 = (-b - sqrt_disc) / (2.0 * a);
    let t1 = (-b + sqrt_disc) / (2.0 * a);
    // Entry point first; if the ray starts inside (t0 < 0 <= t1), clamp to 0.
    if t0 >= 0.0 {
        Some(t0)
    } else if t1 >= 0.0 {
        Some(0.0)
    } else {
        None
    }
}

/// Smallest non-negative `t` where the ray meets an AABB (the slab test).
fn ray_aabb_t(origin: [f32; 2], dir: [f32; 2], center: [f32; 2], he: [f32; 2]) -> Option<f32> {
    let min = [center[0] - he[0], center[1] - he[1]];
    let max = [center[0] + he[0], center[1] + he[1]];
    let mut t_enter = f32::NEG_INFINITY;
    let mut t_exit = f32::INFINITY;
    for axis in 0..2 {
        if dir[axis] == 0.0 {
            // Ray parallel to this slab: miss unless the origin is within it.
            if origin[axis] < min[axis] || origin[axis] > max[axis] {
                return None;
            }
        } else {
            let inv = 1.0 / dir[axis];
            let mut t_near = (min[axis] - origin[axis]) * inv;
            let mut t_far = (max[axis] - origin[axis]) * inv;
            if t_near > t_far {
                std::mem::swap(&mut t_near, &mut t_far);
            }
            t_enter = t_enter.max(t_near);
            t_exit = t_exit.min(t_far);
            if t_enter > t_exit {
                return None;
            }
        }
    }
    if t_exit < 0.0 {
        return None; // box is entirely behind the ray
    }
    Some(t_enter.max(0.0)) // clamp to 0 when the origin is inside the box
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::NoParent;
    use crate::collider::RigidBody;
    use ludion_core::time::{FrameIndex, TickRate};
    use schemars::JsonSchema;
    use serde::{Deserialize, Serialize};

    /// A minimal position component for testing `rebuild` without depending on `ludion-engine`.
    #[derive(Serialize, Deserialize, JsonSchema, Clone, Copy)]
    struct Pos {
        xz: [f32; 2],
    }

    impl GroundPosition for Pos {
        fn ground_position(&self, _plane: Plane) -> [f32; 2] {
            self.xz
        }
    }

    fn ctx() -> SystemCtx {
        SystemCtx::new(FrameIndex::ZERO, TickRate::DEFAULT)
    }

    fn spawn_collider(world: &mut World, xz: [f32; 2], shape: Shape, body: Body) -> EntityId {
        let e = world.spawn();
        world
            .insert(
                e,
                Collider {
                    shape,
                    body,
                    layer: None,
                    sensor: false,
                },
            )
            .unwrap();
        world.insert(e, Pos { xz }).unwrap();
        e
    }

    #[test]
    fn queries_are_empty_before_the_first_rebuild() {
        let world = World::new();
        assert!(overlaps(&world, Shape::default(), [0.0, 0.0], None).is_empty());
        assert!(k_nearest(&world, [0.0, 0.0], 4, 100.0, None).is_empty());
        assert!(raycast(&world, [0.0, 0.0], [1.0, 0.0], None).is_none());
    }

    /// Refiling `filed_entries` reproduces the index: every query answers as the rebuilt one does,
    /// with statics, kinematics, an oversize wall and entries sharing a cell in the world.
    #[test]
    fn from_entries_over_filed_entries_reproduces_the_index() {
        let mut world = World::new();
        let circle = || Shape::Circle { radius: 1.0 };
        // Spawned out of cell order so the order is the `EntityId` one, not the insertion one.
        spawn_collider(&mut world, [40.0, 40.0], circle(), Body::new_kinematic());
        spawn_collider(&mut world, [1.0, 1.0], circle(), Body::new_static());
        spawn_collider(&mut world, [1.5, 1.5], circle(), Body::new_kinematic());
        spawn_collider(&mut world, [2.0, 2.0], circle(), Body::new_static());
        spawn_collider(
            &mut world,
            [0.0, 20.0],
            Shape::Aabb {
                half_extents: [70.0, 1.0],
            },
            Body::new_static(),
        );
        spawn_collider(&mut world, [-30.0, 5.0], circle(), Body::new_static());
        rebuild::<Pos, NoParent>(&mut world, &ctx());
        let original = world.resource::<SpatialIndex>().unwrap().clone();

        let mut copy = World::new();
        copy.insert_resource(SpatialIndex::from_entries(original.filed_entries()));
        assert_eq!(
            copy.resource::<SpatialIndex>().unwrap().len(),
            original.len()
        );

        let probe = Shape::Circle { radius: 3.0 };
        for x in [-30.0, 0.0, 1.5, 2.0, 40.0] {
            for z in [0.0, 5.0, 20.0, 40.0] {
                for body in [None, Some(Body::new_static()), Some(Body::new_kinematic())] {
                    assert_eq!(
                        overlaps(&copy, probe.clone(), [x, z], body),
                        overlaps(&world, probe.clone(), [x, z], body)
                    );
                    assert_eq!(
                        k_nearest_entries(&copy, [x, z], 4, 100.0, body),
                        k_nearest_entries(&world, [x, z], 4, 100.0, body)
                    );
                    assert_eq!(
                        format!("{:?}", raycast(&copy, [x, z], [1.0, 0.5], body)),
                        format!("{:?}", raycast(&world, [x, z], [1.0, 0.5], body))
                    );
                }
            }
        }
        let all = |w: &World| {
            w.resource::<SpatialIndex>()
                .unwrap()
                .entries()
                .cloned()
                .collect::<Vec<Entry>>()
        };
        assert_eq!(all(&copy), all(&world), "the pinned walk order matches too");
    }

    #[test]
    fn rebuild_indexes_every_collider() {
        let mut world = World::new();
        spawn_collider(&mut world, [0.0, 0.0], Shape::default(), Body::new_static());
        spawn_collider(
            &mut world,
            [50.0, -30.0],
            Shape::default(),
            Body::new_kinematic(),
        );
        rebuild::<Pos, NoParent>(&mut world, &ctx());
        assert_eq!(world.resource::<SpatialIndex>().unwrap().len(), 2);
    }

    /// A physics scene indexes nothing rather than indexing a lie: a collider with a 3D shape, or
    /// one whose body is `Rigid`, is skipped even though its (2D-shaped, non-rigid) neighbors still
    /// index normally.
    #[test]
    fn rebuild_skips_3d_shapes_and_rigid_bodies() {
        let mut world = World::new();
        let ordinary = spawn_collider(&mut world, [0.0, 0.0], Shape::default(), Body::new_static());
        spawn_collider(
            &mut world,
            [1.0, 1.0],
            Shape::Sphere { radius: 0.5 },
            Body::new_static(),
        );
        spawn_collider(
            &mut world,
            [2.0, 2.0],
            Shape::Box {
                half_extents: [0.5, 0.5, 0.5],
            },
            Body::new_kinematic(),
        );
        spawn_collider(
            &mut world,
            [3.0, 3.0],
            Shape::default(),
            Body::Rigid(RigidBody::default()),
        );
        rebuild::<Pos, NoParent>(&mut world, &ctx());
        let index = world.resource::<SpatialIndex>().unwrap();
        assert_eq!(
            index.len(),
            1,
            "only the ordinary 2D, non-rigid collider is indexed"
        );
        assert_eq!(
            overlaps(&world, Shape::default(), [0.0, 0.0], None),
            vec![ordinary]
        );
    }

    #[test]
    fn overlaps_finds_only_overlapping_colliders_in_pinned_order() {
        let mut world = World::new();
        let a = spawn_collider(
            &mut world,
            [0.0, 0.0],
            Shape::Circle { radius: 1.0 },
            Body::new_static(),
        );
        let b = spawn_collider(
            &mut world,
            [1.0, 0.0],
            Shape::Circle { radius: 1.0 },
            Body::new_static(),
        );
        let _far = spawn_collider(
            &mut world,
            [40.0, 40.0],
            Shape::default(),
            Body::new_static(),
        );
        rebuild::<Pos, NoParent>(&mut world, &ctx());
        // A probe circle at the origin touches a and b (centers 0 and 1 apart, radii sum 2), not far.
        let hits = overlaps(&world, Shape::Circle { radius: 1.0 }, [0.0, 0.0], None);
        assert_eq!(hits, vec![a, b]); // ascending EntityId
    }

    /// `overlaps_any` is exactly "`overlaps` is non-empty", over a sweep of probe points, shapes and
    /// body filters against a mixed static/kinematic set, and `false` before the first rebuild.
    #[test]
    fn overlaps_any_agrees_with_overlaps() {
        let mut world = World::new();
        assert!(!overlaps_any(&world, Shape::default(), [0.0, 0.0], None));
        let circle = |r| Shape::Circle { radius: r };
        let aabb = |x, z| Shape::Aabb {
            half_extents: [x, z],
        };
        spawn_collider(&mut world, [0.0, 0.0], circle(1.0), Body::new_static());
        spawn_collider(&mut world, [6.0, 2.0], aabb(1.5, 0.5), Body::new_static());
        spawn_collider(&mut world, [-5.0, -4.0], aabb(8.0, 0.4), Body::new_static());
        spawn_collider(&mut world, [3.0, -3.0], circle(0.7), Body::new_kinematic());
        spawn_collider(
            &mut world,
            [-2.0, 5.0],
            aabb(0.6, 0.6),
            Body::new_kinematic(),
        );
        rebuild::<Pos, NoParent>(&mut world, &ctx());
        let probes = [circle(0.3), circle(0.9), circle(2.5), aabb(0.5, 1.5)];
        let bodies = [None, Some(Body::new_static()), Some(Body::new_kinematic())];
        let (mut hits, mut misses) = (0, 0);
        for ix in -14..=14 {
            for iz in -12..=12 {
                let center = [ix as f32 * 0.75, iz as f32 * 0.75];
                for probe in &probes {
                    for body in &bodies {
                        let expected = !overlaps(&world, probe.clone(), center, *body).is_empty();
                        assert_eq!(
                            overlaps_any(&world, probe.clone(), center, *body),
                            expected,
                            "{probe:?} at {center:?} body {body:?}"
                        );
                        if expected {
                            hits += 1;
                        } else {
                            misses += 1;
                        }
                    }
                }
            }
        }
        assert!(
            hits > 100 && misses > 100,
            "the sweep exercises both answers"
        );
    }

    #[test]
    fn overlaps_filters_by_body() {
        let mut world = World::new();
        let s = spawn_collider(
            &mut world,
            [0.0, 0.0],
            Shape::Circle { radius: 1.0 },
            Body::new_static(),
        );
        let _d = spawn_collider(
            &mut world,
            [0.5, 0.0],
            Shape::Circle { radius: 1.0 },
            Body::new_kinematic(),
        );
        rebuild::<Pos, NoParent>(&mut world, &ctx());
        let hits = overlaps(
            &world,
            Shape::Circle { radius: 1.0 },
            [0.0, 0.0],
            Some(Body::new_static()),
        );
        assert_eq!(hits, vec![s]);
    }

    /// The four streams recombine into the one pinned order the queries promise — cell key
    /// ascending, then `EntityId` within a cell — through every merge shape: a cell key shared by
    /// two streams, a key held by one stream only (in both directions), a key shared by three, and
    /// the case the `oversize` grid exists for, where a wall reaches the probe from a center cell
    /// no inflated rectangle covers.
    #[test]
    fn overlaps_with_no_filter_merges_halves_in_pinned_order() {
        // One shared cell, bodies interleaved: the merge must order by `EntityId`, not by half.
        let mut world = World::new();
        let small = Shape::Circle { radius: 0.25 };
        let a = spawn_collider(&mut world, [1.0, 1.0], small.clone(), Body::new_static());
        let b = spawn_collider(&mut world, [2.0, 1.0], small.clone(), Body::new_kinematic());
        let c = spawn_collider(&mut world, [3.0, 1.0], small.clone(), Body::new_static());
        let d = spawn_collider(&mut world, [4.0, 1.0], small.clone(), Body::new_kinematic());
        rebuild::<Pos, NoParent>(&mut world, &ctx());
        let hits = overlaps(&world, Shape::Circle { radius: 5.0 }, [2.5, 1.0], None);
        assert_eq!(hits, vec![a, b, c, d]);

        // Disjoint cells on both sides of the dynamic one, spawned so that cell order and
        // `EntityId` order disagree: the merge must follow cell order.
        let mut world = World::new();
        let small = Shape::Circle { radius: 1.0 };
        let mid_dynamic =
            spawn_collider(&mut world, [0.0, 0.0], small.clone(), Body::new_kinematic());
        let right_static =
            spawn_collider(&mut world, [20.0, 0.0], small.clone(), Body::new_static());
        let left_static =
            spawn_collider(&mut world, [-20.0, 0.0], small.clone(), Body::new_static());
        rebuild::<Pos, NoParent>(&mut world, &ctx());
        let wide = Shape::Aabb {
            half_extents: [30.0, 5.0],
        };
        let hits = overlaps(&world, wide, [0.0, 0.0], None);
        assert_eq!(hits, vec![left_static, mid_dynamic, right_static]);

        // Three streams under one cell key — an oversize static, a dynamic, and a regular static,
        // spawned in that order so a stream-by-stream concatenation would emit them out of
        // `EntityId` order.
        let mut world = World::new();
        let long = spawn_collider(
            &mut world,
            [8.0, 8.0],
            Shape::Aabb {
                half_extents: [20.0, 1.0],
            },
            Body::new_static(),
        );
        let dynamic = spawn_collider(
            &mut world,
            [9.0, 8.0],
            Shape::Circle { radius: 1.0 },
            Body::new_kinematic(),
        );
        let prop = spawn_collider(
            &mut world,
            [7.0, 8.0],
            Shape::Circle { radius: 1.0 },
            Body::new_static(),
        );
        rebuild::<Pos, NoParent>(&mut world, &ctx());
        let hits = overlaps(&world, Shape::Circle { radius: 3.0 }, [8.0, 8.0], None);
        assert_eq!(hits, vec![long, dynamic, prop]); // all cell (0, 0), ascending EntityId

        // The oversize trap: a wall reaching the probe from a center cell (0, 4) that no inflated
        // rectangle covers — the statics grid's bounds are pinned elsewhere by `far` and its
        // `max_reach` is a prop's, the dynamics half is a body radius. Only the full oversize scan
        // finds it, and the merge must still place it after the dynamic in cell (0, 0).
        let mut world = World::new();
        let wall = spawn_collider(
            &mut world,
            [0.0, 70.0],
            Shape::Aabb {
                half_extents: [2.0, 70.0],
            },
            Body::new_static(),
        );
        let _far = spawn_collider(
            &mut world,
            [100.0, 100.0],
            Shape::Circle { radius: 1.0 },
            Body::new_static(),
        );
        let mover = spawn_collider(
            &mut world,
            [0.5, 2.0],
            Shape::Circle { radius: 0.5 },
            Body::new_kinematic(),
        );
        rebuild::<Pos, NoParent>(&mut world, &ctx());
        let hits = overlaps(&world, Shape::Circle { radius: 1.0 }, [0.0, 2.0], None);
        assert_eq!(hits, vec![mover, wall]); // cell (0, 0) before cell (0, 4)
    }

    #[test]
    fn circle_aabb_overlap_is_symmetric() {
        let mut world = World::new();
        let box_e = spawn_collider(
            &mut world,
            [3.0, 0.0],
            Shape::Aabb {
                half_extents: [1.0, 1.0],
            },
            Body::new_static(),
        );
        rebuild::<Pos, NoParent>(&mut world, &ctx());
        // Circle probe reaching the box's near face.
        let hits = overlaps(&world, Shape::Circle { radius: 1.5 }, [0.5, 0.0], None);
        assert_eq!(hits, vec![box_e]);
        // Just out of reach.
        assert!(overlaps(&world, Shape::Circle { radius: 1.0 }, [0.0, 0.0], None).is_empty());
    }

    #[test]
    fn k_nearest_orders_by_distance_then_entity() {
        let mut world = World::new();
        let near = spawn_collider(&mut world, [1.0, 0.0], Shape::default(), Body::new_static());
        let mid = spawn_collider(&mut world, [3.0, 0.0], Shape::default(), Body::new_static());
        let far = spawn_collider(&mut world, [9.0, 0.0], Shape::default(), Body::new_static());
        rebuild::<Pos, NoParent>(&mut world, &ctx());
        let got = k_nearest(&world, [0.0, 0.0], 2, 100.0, None);
        assert_eq!(got, vec![near, mid]);
        let all = k_nearest(&world, [0.0, 0.0], 10, 100.0, None);
        assert_eq!(all, vec![near, mid, far]);
    }

    #[test]
    fn k_nearest_respects_max_radius() {
        let mut world = World::new();
        let near = spawn_collider(&mut world, [1.0, 0.0], Shape::default(), Body::new_static());
        let _far = spawn_collider(
            &mut world,
            [50.0, 0.0],
            Shape::default(),
            Body::new_static(),
        );
        rebuild::<Pos, NoParent>(&mut world, &ctx());
        assert_eq!(k_nearest(&world, [0.0, 0.0], 10, 5.0, None), vec![near]);
    }

    #[test]
    fn k_nearest_ties_break_by_entity() {
        let mut world = World::new();
        // Two colliders equidistant from the probe: lower EntityId comes first.
        let a = spawn_collider(&mut world, [2.0, 0.0], Shape::default(), Body::new_static());
        let b = spawn_collider(
            &mut world,
            [-2.0, 0.0],
            Shape::default(),
            Body::new_static(),
        );
        rebuild::<Pos, NoParent>(&mut world, &ctx());
        assert_eq!(k_nearest(&world, [0.0, 0.0], 2, 100.0, None), vec![a, b]);
    }

    #[test]
    fn raycast_returns_the_nearest_hit() {
        let mut world = World::new();
        let _far = spawn_collider(
            &mut world,
            [10.0, 0.0],
            Shape::Circle { radius: 1.0 },
            Body::new_static(),
        );
        let near = spawn_collider(
            &mut world,
            [4.0, 0.0],
            Shape::Circle { radius: 1.0 },
            Body::new_static(),
        );
        rebuild::<Pos, NoParent>(&mut world, &ctx());
        let hit = raycast(&world, [0.0, 0.0], [1.0, 0.0], None).unwrap();
        assert_eq!(hit.entity, near);
        assert!((hit.t - 3.0).abs() < 1e-5, "t was {}", hit.t);
        assert!((hit.point[0] - 3.0).abs() < 1e-5);
    }

    #[test]
    fn raycast_hits_an_aabb_via_the_slab_test() {
        let mut world = World::new();
        let box_e = spawn_collider(
            &mut world,
            [5.0, 0.0],
            Shape::Aabb {
                half_extents: [1.0, 1.0],
            },
            Body::new_static(),
        );
        rebuild::<Pos, NoParent>(&mut world, &ctx());
        let hit = raycast(&world, [0.0, 0.0], [1.0, 0.0], None).unwrap();
        assert_eq!(hit.entity, box_e);
        assert!((hit.t - 4.0).abs() < 1e-5, "t was {}", hit.t);
    }

    #[test]
    fn raycast_misses_return_none() {
        let mut world = World::new();
        spawn_collider(
            &mut world,
            [0.0, 10.0],
            Shape::Circle { radius: 1.0 },
            Body::new_static(),
        );
        rebuild::<Pos, NoParent>(&mut world, &ctx());
        // Ray along +x never reaches the collider at z = 10.
        assert!(raycast(&world, [0.0, 0.0], [1.0, 0.0], None).is_none());
    }

    /// Candidate-cell pruning is invisible: on a scattered layout (including large AABBs whose
    /// extent crosses many cells, and probes sitting exactly on cell borders), `overlaps` and
    /// `k_nearest` return exactly what a full scan over every entry returns — same set, same pinned
    /// order. The conformance suite holds the independent oracle.
    #[test]
    fn pruned_queries_match_a_full_scan_on_a_scattered_layout() {
        use ludion_core::rng::DeterministicRng;

        fn coord(rng: &mut DeterministicRng, half: f32) -> f32 {
            (rng.next_f32() - 0.5) * 2.0 * half
        }

        let mut world = World::new();
        let mut rng = DeterministicRng::new(7);
        for i in 0..200 {
            let xz = [coord(&mut rng, 40.0), coord(&mut rng, 40.0)];
            let shape = match i % 4 {
                0 => Shape::Circle {
                    radius: 0.25 + rng.next_f32() * 1.5,
                },
                1 => Shape::Aabb {
                    half_extents: [0.25 + rng.next_f32() * 1.5, 0.25 + rng.next_f32() * 1.5],
                },
                // Large AABBs spanning many cells — the case `max_reach` inflation exists for.
                2 => Shape::Aabb {
                    half_extents: [4.0 + rng.next_f32() * 8.0, 0.5 + rng.next_f32()],
                },
                _ => Shape::Circle {
                    radius: 0.1 + rng.next_f32() * 0.3,
                },
            };
            let body = if i % 3 == 0 {
                Body::new_static()
            } else {
                Body::new_kinematic()
            };
            spawn_collider(&mut world, xz, shape, body);
        }
        // The production wall shape: one static whose reach dwarfs every other entry's, so the
        // statics half's inflation is degenerate while the dynamics half's stays tight — the
        // asymmetry the two halves exist to produce, under the full-scan assertion.
        spawn_collider(
            &mut world,
            [12.0, -8.0],
            Shape::Aabb {
                half_extents: [2.0, 70.0],
            },
            Body::new_static(),
        );
        rebuild::<Pos, NoParent>(&mut world, &ctx());
        let index = world.resource::<SpatialIndex>().unwrap();

        let probes = [
            [0.0, 0.0],              // exactly on a cell corner
            [CELL_SIZE, -CELL_SIZE], // on cell borders
            [3.7, -12.2],
            [-25.0, 31.0],
            [55.0, 55.0],    // outside the populated area
            [-0.001, 0.001], // straddling the origin corner
        ];
        let shapes = [
            Shape::Circle { radius: 0.5 },
            Shape::Circle { radius: 5.0 },
            Shape::Aabb {
                half_extents: [2.0, 0.5],
            },
        ];
        let filters = [None, Some(Body::new_static()), Some(Body::new_kinematic())];
        for &center in &probes {
            for shape in &shapes {
                for &filter in &filters {
                    let pruned = overlaps(&world, shape.clone(), center, filter);
                    let full: Vec<EntityId> = index
                        .entries()
                        .filter(|entry| body_admits(filter, entry))
                        .filter(|entry| shapes_overlap(shape, center, &entry.shape, entry.center))
                        .map(|entry| entry.entity)
                        .collect();
                    assert_eq!(
                        pruned, full,
                        "overlaps diverged at {center:?} shape {shape:?} filter {filter:?}"
                    );
                }
            }
            for &(k, radius) in &[(4usize, 3.0f32), (10, 8.0), (200, 500.0)] {
                for &filter in &filters {
                    let pruned = k_nearest(&world, center, k, radius, filter);
                    let mut full: Vec<(f32, EntityId)> = index
                        .entries()
                        .filter(|entry| body_admits(filter, entry))
                        .filter_map(|entry| {
                            let sq = distance_squared(center, entry.center);
                            (sq <= radius * radius).then_some((sq, entry.entity))
                        })
                        .collect();
                    full.sort_by(|a, b| a.0.total_cmp(&b.0).then(a.1.cmp(&b.1)));
                    let full: Vec<EntityId> = full.into_iter().take(k).map(|(_, e)| e).collect();
                    assert_eq!(
                        pruned, full,
                        "k_nearest diverged at {center:?} k {k} radius {radius} filter {filter:?}"
                    );
                }
            }
        }
    }

    #[test]
    fn rebuild_is_a_pure_function_of_world_content() {
        let mut world = World::new();
        spawn_collider(
            &mut world,
            [1.0, 2.0],
            Shape::Circle { radius: 1.0 },
            Body::new_kinematic(),
        );
        spawn_collider(
            &mut world,
            [-3.0, 4.0],
            Shape::default(),
            Body::new_static(),
        );
        rebuild::<Pos, NoParent>(&mut world, &ctx());
        let first: Vec<_> = world
            .resource::<SpatialIndex>()
            .unwrap()
            .entries()
            .cloned()
            .collect();
        rebuild::<Pos, NoParent>(&mut world, &ctx());
        let second: Vec<_> = world
            .resource::<SpatialIndex>()
            .unwrap()
            .entries()
            .cloned()
            .collect();
        assert_eq!(first, second);
    }
}
