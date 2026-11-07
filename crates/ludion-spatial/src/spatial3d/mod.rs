//! A generic, bit-exact 3D broadphase: a bounding-volume hierarchy over caller-fed spheres and
//! boxes, rebuilt from scratch each tick.
//!
//! The caller [`push`](Index3d::push)es [`Item`]s (an opaque [`Handle`], a [`Volume`] and a layer
//! mask) and calls [`build`](Index3d::build). The build sorts the items into a pinned order (Morton
//! code, then handle) and splits the sorted range at its midpoint, so the tree and every query
//! result are a pure function of the item *set*, never of push order or thread count. Queries are
//! `&self`: [`overlap_sphere`](Index3d::overlap_sphere), [`overlap_aabb`](Index3d::overlap_aabb),
//! [`k_nearest`](Index3d::k_nearest) and the first-hit [`cast`](Index3d::cast) (a segment at
//! radius 0, a swept sphere above). Each takes a [`Filter`] and fills a caller buffer without
//! allocating once the buffer has grown.
//!
//! ```
//! use ludion_spatial::spatial3d::{Filter, Handle, Index3d, Item, Volume};
//!
//! let mut index = Index3d::new();
//! index.push(Item {
//!     handle: Handle::slot(0, 0),
//!     volume: Volume::Sphere { center: [5.0, 0.0, 0.0], radius: 1.0 },
//!     layers: 1,
//! });
//! index.build();
//! let hit = index.cast([0.0; 3], [10.0, 0.0, 0.0], 0.0, &Filter::new(1)).unwrap();
//! assert_eq!(hit.handle, Handle::slot(0, 0));
//! assert_eq!(hit.t, 0.4);
//! assert_eq!(hit.normal, [-1.0, 0.0, 0.0]);
//! ```

mod narrow;
mod query;
#[cfg(test)]
mod tests;

use ludion_core::ids::EntityId;
use ludion_core::par;
use serde::{Deserialize, Serialize};

use narrow::V3;

/// Items per leaf at most.
const LEAF_SIZE: usize = 4;
/// Items per parallel key chunk; fixed so results never depend on the thread count.
const KEY_CHUNK: usize = 1024;
/// Traversal stack depth: median splits give a depth of at most `ceil(log2 n)` ≤ 32.
const STACK: usize = 64;

/// What a [`Handle`] names.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum HandleKind {
    /// An ECS entity. Sorts before [`HandleKind::Slot`].
    Entity,
    /// A slot in a caller-owned pool (a projectile pool, say).
    Slot,
}

/// An opaque, totally ordered item identity: kind, then index, then generation. Handles must be
/// unique within one build; ties in every query break by handle ascending.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct Handle {
    /// What the handle names.
    pub kind: HandleKind,
    /// Entity or pool-slot index.
    pub index: u32,
    /// Entity or pool-slot generation.
    pub generation: u32,
}

impl Handle {
    /// The handle of an ECS entity.
    pub fn entity(id: EntityId) -> Self {
        Self {
            kind: HandleKind::Entity,
            index: id.index,
            generation: id.generation,
        }
    }

    /// The handle of a pool slot.
    pub fn slot(index: u32, generation: u32) -> Self {
        Self {
            kind: HandleKind::Slot,
            index,
            generation,
        }
    }

    /// The entity this names, or `None` for a pool slot.
    pub fn as_entity(&self) -> Option<EntityId> {
        (self.kind == HandleKind::Entity).then_some(EntityId {
            index: self.index,
            generation: self.generation,
        })
    }
}

/// The shape an [`Item`] occupies, in world space.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub enum Volume {
    /// A sphere; `radius` must be finite and non-negative.
    Sphere {
        /// Centre.
        center: [f32; 3],
        /// Radius.
        radius: f32,
    },
    /// An axis-aligned box; `min <= max` on every axis.
    Aabb {
        /// Minimum corner.
        min: [f32; 3],
        /// Maximum corner.
        max: [f32; 3],
    },
}

impl Volume {
    /// The axis-aligned bounds `(min, max)`.
    pub fn bounds(&self) -> ([f32; 3], [f32; 3]) {
        match *self {
            Volume::Sphere {
                center: c,
                radius: r,
            } => (
                [c[0] - r, c[1] - r, c[2] - r],
                [c[0] + r, c[1] + r, c[2] + r],
            ),
            Volume::Aabb { min, max } => (min, max),
        }
    }

    /// The centre: a sphere's centre, a box's `(min + max) * 0.5`. [`Index3d::k_nearest`] measures
    /// to this point.
    pub fn center(&self) -> [f32; 3] {
        match *self {
            Volume::Sphere { center, .. } => center,
            Volume::Aabb { min, max } => [
                (min[0] + max[0]) * 0.5,
                (min[1] + max[1]) * 0.5,
                (min[2] + max[2]) * 0.5,
            ],
        }
    }

    fn is_valid(&self) -> bool {
        match *self {
            Volume::Sphere { center, radius } => {
                narrow::is_finite3(center) && radius.is_finite() && radius >= 0.0
            }
            Volume::Aabb { min, max } => {
                narrow::is_finite3(min)
                    && narrow::is_finite3(max)
                    && (0..3).all(|i| min[i] <= max[i])
            }
        }
    }

    /// Turns every `-0.0` into `+0.0` (`min`/`max` with signed zeros is unspecified).
    fn canonical(self) -> Self {
        match self {
            Volume::Sphere { center, radius } => Volume::Sphere {
                center: zero3(center),
                radius: radius + 0.0,
            },
            Volume::Aabb { min, max } => Volume::Aabb {
                min: zero3(min),
                max: zero3(max),
            },
        }
    }

    /// The content as raw bits, the last tie-break of the build sort.
    fn bits(&self) -> [u32; 7] {
        match *self {
            Volume::Sphere { center: c, radius } => [
                0,
                c[0].to_bits(),
                c[1].to_bits(),
                c[2].to_bits(),
                radius.to_bits(),
                0,
                0,
            ],
            Volume::Aabb { min, max } => [
                1,
                min[0].to_bits(),
                min[1].to_bits(),
                min[2].to_bits(),
                max[0].to_bits(),
                max[1].to_bits(),
                max[2].to_bits(),
            ],
        }
    }
}

fn zero3(v: V3) -> V3 {
    [v[0] + 0.0, v[1] + 0.0, v[2] + 0.0]
}

/// One thing in the index: an identity, a shape and the layers it lives on.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Item {
    /// Identity; unique within a build.
    pub handle: Handle,
    /// The shape.
    pub volume: Volume,
    /// Layer bits; a query sees the item when `layers & filter.layers != 0`.
    pub layers: u32,
}

/// Which items a query sees: a layer mask plus up to two excluded handles.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Filter {
    /// Layer mask; an item is seen when `item.layers & layers != 0`.
    pub layers: u32,
    /// An item never seen (a shooter, say).
    pub exclude: Option<Handle>,
    /// A second never-seen item (the shooter's owner, say).
    pub owner: Option<Handle>,
}

impl Filter {
    /// Sees every item on any of `layers`.
    pub fn new(layers: u32) -> Self {
        Self {
            layers,
            exclude: None,
            owner: None,
        }
    }

    /// Also hides `handle`.
    pub fn excluding(mut self, handle: Handle) -> Self {
        self.exclude = Some(handle);
        self
    }

    /// Also hides `handle` as the owner.
    pub fn owned_by(mut self, handle: Handle) -> Self {
        self.owner = Some(handle);
        self
    }

    fn passes(&self, item: &Item) -> bool {
        item.layers & self.layers != 0
            && self.exclude != Some(item.handle)
            && self.owner != Some(item.handle)
    }
}

/// The first hit of [`Index3d::cast`].
#[derive(Clone, Copy, Debug, PartialEq)]
#[non_exhaustive]
pub struct CastHit {
    /// The item hit.
    pub handle: Handle,
    /// Fraction of `delta` travelled, in `[0, 1]`; `0` when the probe starts overlapping.
    pub t: f32,
    /// The item's surface point nearest the probe centre at `t`.
    pub point: [f32; 3],
    /// Unit normal pointing from the item toward the probe. Exactly an axis for a box face.
    pub normal: [f32; 3],
}

/// One [`Index3d::k_nearest`] result.
#[derive(Clone, Copy, Debug, PartialEq)]
#[non_exhaustive]
pub struct Neighbor {
    /// The item.
    pub handle: Handle,
    /// Squared distance from the query point to the item's [`Volume::center`].
    pub distance_sq: f32,
    /// The item's [`Volume::center`], so a caller inside a parallel region needs no world read.
    pub center: [f32; 3],
}

/// A flat pre-order BVH node. A leaf has `count > 0` (items `first..first + count`); an interior
/// node's left child is the next node and its right child is `first`.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Node {
    min: V3,
    max: V3,
    layers: u32,
    first: u32,
    count: u32,
}

/// The 3D broadphase. Stage with [`push`](Self::push), [`build`](Self::build), then query.
///
/// `Send + Sync`: queries are `&self`, so callers may fan them out over `ludion_core::par`.
#[derive(Clone, Debug, Default)]
pub struct Index3d {
    staging: Vec<Item>,
    keyed: Vec<(u32, Item)>,
    items: Vec<Item>,
    nodes: Vec<Node>,
    scratch_handles: Vec<Handle>,
    scratch_dups: Vec<Handle>,
    scratch_seen: Vec<bool>,
    rejected: usize,
    skipped: usize,
}

impl Index3d {
    /// An empty index.
    pub fn new() -> Self {
        Self::default()
    }

    /// Drops the staged items. The built tree stays queryable until the next [`build`](Self::build).
    pub fn clear(&mut self) {
        self.staging.clear();
        self.rejected = 0;
    }

    /// Stages an item. A non-finite coordinate, a negative or non-finite radius or an inverted box
    /// is skipped and counted in [`skipped`](Self::skipped), identically in debug and release.
    pub fn push(&mut self, item: Item) {
        let volume = item.volume.canonical();
        if !volume.is_valid() {
            self.rejected += 1;
            return;
        }
        self.staging.push(Item { volume, ..item });
    }

    /// Builds the tree from the staged items and clears the staging (capacity kept). A duplicate
    /// handle keeps only the first item in sorted order and counts the rest as skipped.
    pub fn build(&mut self) {
        self.items.clear();
        self.nodes.clear();
        self.skipped = std::mem::take(&mut self.rejected);
        if self.staging.is_empty() {
            return;
        }

        // Quantize each centre over the staged centres' bounds, sequentially: min/max of
        // non-NaN, sign-canonical values is independent of order.
        let mut lo = [f32::INFINITY; 3];
        let mut hi = [f32::NEG_INFINITY; 3];
        for item in &self.staging {
            let c = item.volume.center();
            for i in 0..3 {
                lo[i] = lo[i].min(c[i]);
                hi[i] = hi[i].max(c[i]);
            }
        }
        self.keyed.clear();
        self.keyed
            .extend(self.staging.iter().map(|item| (0, *item)));
        par::par_chunks(&mut self.keyed, KEY_CHUNK, |_, chunk| {
            for entry in chunk {
                entry.0 = morton(entry.1.volume.center(), lo, hi);
            }
        });
        self.staging.clear();
        self.keyed.sort_unstable_by(|a, b| {
            a.0.cmp(&b.0)
                .then(a.1.handle.cmp(&b.1.handle))
                .then(a.1.volume.bits().cmp(&b.1.volume.bits()))
                .then(a.1.layers.cmp(&b.1.layers))
        });
        self.items.extend(self.keyed.iter().map(|(_, item)| *item));
        self.dedup_handles();

        self.nodes.reserve(self.items.len().div_ceil(LEAF_SIZE) * 2);
        self.build_node(0, self.items.len());
    }

    /// Keeps the first item (in sorted order) of every duplicated handle. Duplicates need not be
    /// adjacent: their Morton codes differ when their shapes do.
    fn dedup_handles(&mut self) {
        let handles = &mut self.scratch_handles;
        handles.clear();
        handles.extend(self.items.iter().map(|item| item.handle));
        handles.sort_unstable();
        let dups = &mut self.scratch_dups;
        dups.clear();
        for pair in handles.windows(2) {
            if pair[0] == pair[1] && dups.last() != Some(&pair[0]) {
                dups.push(pair[0]);
            }
        }
        if dups.is_empty() {
            return;
        }
        let seen = &mut self.scratch_seen;
        seen.clear();
        seen.resize(dups.len(), false);
        let before = self.items.len();
        self.items
            .retain(|item| match dups.binary_search(&item.handle) {
                Ok(i) => !std::mem::replace(&mut seen[i], true),
                Err(_) => true,
            });
        self.skipped += before - self.items.len();
    }

    /// Builds the subtree over `items[lo..hi]` and returns its node index.
    fn build_node(&mut self, lo: usize, hi: usize) -> u32 {
        let index = self.nodes.len() as u32;
        if hi - lo <= LEAF_SIZE {
            let mut min = [f32::INFINITY; 3];
            let mut max = [f32::NEG_INFINITY; 3];
            let mut layers = 0;
            for item in &self.items[lo..hi] {
                let (b0, b1) = item.volume.bounds();
                for i in 0..3 {
                    min[i] = min[i].min(b0[i]);
                    max[i] = max[i].max(b1[i]);
                }
                layers |= item.layers;
            }
            // Outward padding so a rounding gap between the node test and the leaf narrow phase
            // can never prune a true hit; it changes which leaves are visited, never a result.
            for i in 0..3 {
                min[i] -= PAD * (1.0 + min[i].abs());
                max[i] += PAD * (1.0 + max[i].abs());
            }
            self.nodes.push(Node {
                min,
                max,
                layers,
                first: lo as u32,
                count: (hi - lo) as u32,
            });
            return index;
        }
        self.nodes.push(Node {
            min: [0.0; 3],
            max: [0.0; 3],
            layers: 0,
            first: 0,
            count: 0,
        });
        let mid = lo + (hi - lo) / 2;
        let left = self.build_node(lo, mid);
        let right = self.build_node(mid, hi);
        let (l, r) = (self.nodes[left as usize], self.nodes[right as usize]);
        let mut min = l.min;
        let mut max = l.max;
        for i in 0..3 {
            min[i] = min[i].min(r.min[i]);
            max[i] = max[i].max(r.max[i]);
        }
        self.nodes[index as usize] = Node {
            min,
            max,
            layers: l.layers | r.layers,
            first: right,
            count: 0,
        };
        index
    }

    /// Number of built items.
    pub fn len(&self) -> usize {
        self.items.len()
    }

    /// Whether no item is built.
    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    /// Items rejected at [`push`](Self::push) plus duplicate handles dropped, as of the last
    /// [`build`](Self::build) (pushes since then included).
    pub fn skipped(&self) -> usize {
        self.skipped + self.rejected
    }

    /// The built items in their pinned order (Morton code, then handle).
    pub fn items(&self) -> &[Item] {
        &self.items
    }

    #[cfg(test)]
    fn nodes(&self) -> &[Node] {
        &self.nodes
    }
}

/// Outward node-bound padding factor.
const PAD: f32 = 1e-5;

/// A 30-bit Morton code of `c` quantized over `[lo, hi]`.
fn morton(c: V3, lo: V3, hi: V3) -> u32 {
    let mut code = 0;
    for i in 0..3 {
        let extent = hi[i] - lo[i];
        let q = if extent > 0.0 {
            ((c[i] - lo[i]) / extent * 1023.0) as u32
        } else {
            0
        };
        code |= spread_bits(q.min(1023)) << i;
    }
    code
}

/// Spreads the low 10 bits so two zero bits separate each.
fn spread_bits(mut v: u32) -> u32 {
    v = (v | (v << 16)) & 0x0300_00FF;
    v = (v | (v << 8)) & 0x0300_F00F;
    v = (v | (v << 4)) & 0x030C_30C3;
    (v | (v << 2)) & 0x0924_9249
}
