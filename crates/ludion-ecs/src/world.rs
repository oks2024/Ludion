//! The [`World`]: entities, their components, and typed resources.
//!
//! # Determinism contracts
//!
//! - Entity allocation follows a LIFO free-list (see [`crate::entity`]).
//! - Component storages iterate in ascending entity-index order (see [`crate::storage`]); queries
//!   inherit that order.
//! - Resources are keyed by [`TypeId`] and accessed **by type only** — they are never iterated, so
//!   the non-deterministic ordering of `TypeId`s is never observable.
//!
//! Presentation entities live in a second id range with their own allocator ([`PRESENTATION_BASE`]),
//! and the accessors enforce the sim/presentation boundary by the world's [`Phase`] (`L0028`).
//!
//! Stale-handle safety: every component and lifecycle method that takes an [`EntityId`] first checks
//! it is alive and returns an `L0002` [`Diagnostic`] otherwise, so a despawned handle can never
//! silently read or write the entity that later inherits its slot.

use std::any::{Any, TypeId};
use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Mutex, PoisonError};

use ludion_core::codes::code;
use ludion_core::diagnostics::Diagnostic;
use ludion_core::ids::EntityId;

use crate::Component;
use crate::entity::{
    AllocatorState, EntityAllocator, PRESENTATION_BASE, Phase, is_presentation, resolve_id,
};
use crate::hierarchy::{ChildMap, Parent};
use crate::query::{FULL_WINDOW, KeyWindow, Query, QueryIds};
use crate::storage::{AnyStorage, Storage};

/// One observed query shape: the sorted set of `(component TypeId, exclusive?)` pairs a
/// [`World::query`] touched — or, for a per-entity accessor, the single pair that accessor named. A
/// `Vec` (kept sorted and deduped) rather than a set so it is cheap to compare and hashable-by-value
/// as a `BTreeSet` key.
type ObservedShape = Vec<(TypeId, bool)>;

/// The opt-in query-recording state (see [`World::begin_query_recording`]). Absent by default so a
/// non-recording sim pays only a single `Option` check per recorded access. Never enters the
/// `WorldHash`, a snapshot, or a replay — it purely observes.
#[derive(Default)]
struct QueryRecording {
    /// The system whose queries are currently attributed — set by the [`Schedule`](crate::Schedule)
    /// before each system runs. `None` outside a scheduled system, where a query is not attributed.
    current_system: Option<&'static str>,
    /// Per system name, the distinct query shapes its body actually ran.
    observed: BTreeMap<&'static str, BTreeSet<ObservedShape>>,
    /// The Rust name of every component type in `observed`, for messages about unregistered types.
    type_names: BTreeMap<TypeId, &'static str>,
}

/// The per-system component accesses observed during a recorded run (see
/// [`World::begin_query_recording`]).
///
/// Each system name maps to the **set** of distinct shapes its body accessed; a shape is the set of
/// `(component TypeId, exclusive?)` pairs a query named (tuple order does not matter — it is
/// normalized to a sorted vec), or the single pair a per-entity accessor named. A query-drift
/// verifier compares these against a system's *declared* shapes; the editor's Game Designer view
/// answers "which systems iterate this entity?" from the declared side of the same data.
#[derive(Debug, Default, Clone)]
pub struct QueryObservations {
    per_system: BTreeMap<&'static str, BTreeSet<ObservedShape>>,
    type_names: BTreeMap<TypeId, &'static str>,
}

impl QueryObservations {
    /// The Rust type name of an observed component type, for a message about a type the registry
    /// does not know. `None` for a `TypeId` no observation named.
    pub fn type_name(&self, id: TypeId) -> Option<&'static str> {
        self.type_names.get(&id).copied()
    }

    /// The system names that ran at least one query, in name order.
    pub fn systems(&self) -> impl Iterator<Item = &str> + '_ {
        self.per_system.keys().copied()
    }

    /// The distinct shapes `system` was observed running, each a sorted slice of
    /// `(component TypeId, exclusive?)`. Empty for a system that never queried (or never ran).
    pub fn shapes(&self, system: &str) -> impl Iterator<Item = &[(TypeId, bool)]> + '_ {
        self.per_system
            .get(system)
            .into_iter()
            .flat_map(|set| set.iter().map(Vec::as_slice))
    }

    /// Folds `other`'s observations into `self` (union per system) — how the engine accumulates
    /// observations across scene loads and district switches, each of which builds a fresh world.
    pub fn merge(&mut self, other: QueryObservations) {
        for (system, shapes) in other.per_system {
            self.per_system.entry(system).or_default().extend(shapes);
        }
        self.type_names.extend(other.type_names);
    }
}

/// A container of entities, their components, and typed resources.
///
/// The `World` is the sole owner of entity and component state, and **one sim thread solely owns the
/// `World`** (architecture doc §3) — it is never wrapped in an `Arc<Mutex<_>>`, and every read and
/// write still flows through that one owner.
///
/// # Why it is `Sync`
///
/// `World` is `Send + Sync`. `Sync` is not an invitation to share it between threads: it exists so
/// that the *one* sanctioned fan-out — [`par_for_each`](World::par_for_each) and
/// [`par_join_fold`](World::par_join_fold), which spread one system's per-entity work across
/// [`ludion_core::par`] chunks — can hand each chunk a `&World` for shared reads while it writes only
/// its own disjoint slice. The fan-out begins and ends inside a single system call on the owning
/// thread; no `&World` outlives it.
///
/// Interior mutability is limited to two host-observation cells, each a `Mutex` so that `&self`
/// accessors like [`get`](World::get) can observe themselves — from a worker chunk as readily as from
/// the owning thread: the opt-in access recorder
/// ([`begin_query_recording`](World::begin_query_recording)), locked only while recording is armed
/// (a single `Option` check otherwise), and the `L0028` outbox
/// ([`take_boundary_crossings`](World::take_boundary_crossings)), locked only on a refused access.
/// Neither holds sim state — nothing hashed, snapshotted, or replayed — so the sim-state contract is
/// unaffected.
///
/// # Example
///
/// ```
/// use ludion_ecs::World;
/// use serde::{Deserialize, Serialize};
/// use schemars::JsonSchema;
///
/// #[derive(Serialize, Deserialize, JsonSchema, Clone, Copy, PartialEq, Debug)]
/// struct Position {
///     x: i32,
///     y: i32,
/// }
///
/// let mut world = World::new();
/// let e = world.spawn();
/// world.insert(e, Position { x: 1, y: 2 }).unwrap();
/// assert_eq!(world.get::<Position>(e).unwrap(), Some(&Position { x: 1, y: 2 }));
///
/// world.despawn(e).unwrap();
/// // The handle is now permanently invalid: access is a reported error, not a silent hit.
/// assert!(world.get::<Position>(e).is_err());
/// ```
#[derive(Default)]
pub struct World {
    entities: EntityAllocator,
    /// The running system's phase; only the [`Schedule`](crate::Schedule) sets it.
    phase: Phase,
    storages: BTreeMap<TypeId, Box<dyn AnyStorage>>,
    // `+ Send + Sync` so the whole `World` is `Send + Sync`: the live driver
    // (engine-architecture.md §3) moves the engine — and thus the world — onto a dedicated sim
    // thread that solely owns it, and `par_for_each` hands `&World` to its chunks. Component
    // storages are already `Send + Sync` (see `AnyStorage`); requiring the same of resources keeps
    // the world shareable without any `Arc`/`Mutex`. Resources are all plain sim state (the seeded
    // RNG, input edges), which qualify already.
    resources: BTreeMap<TypeId, Box<dyn Any + Send + Sync>>,
    /// Opt-in component-access recording (default `None` — a non-recording sim pays one `Option`
    /// check per recorded access). Host/test observation only: never hashed, snapshotted, or
    /// replayed, and never branches sim logic. The `Mutex` is what lets the `&self` accessors
    /// ([`get`](World::get)) record — including from a `par_for_each` worker chunk; it is the
    /// world's only interior mutability besides `crossings`, and carries no sim state.
    recording: Option<Mutex<QueryRecording>>,
    /// The `L0028` refusals not yet drained by the host (see
    /// [`take_boundary_crossings`](World::take_boundary_crossings)), one per distinct key. Host
    /// observation only, like `recording`: never hashed, snapshotted, or replayed, and never
    /// branches sim logic. A `Mutex` so the `&self` readers can record, including from a
    /// `par_for_each` chunk; touched only on the cold refusal path.
    crossings: Mutex<Vec<BoundaryCrossing>>,
    /// The component types a `par_for_each` region currently has lifted out of `storages` — empty
    /// outside a region. Lets the `&self` readers tell "lifted" from "no such component"; the name
    /// is for the `L0026` message. Never hashed or snapshotted.
    lifted: Vec<(TypeId, &'static str)>,
    /// The presentation id range's allocator, in indices relative to [`PRESENTATION_BASE`]: every
    /// boundary adds or subtracts the base. Never hashed.
    presentation: EntityAllocator,
    /// The resource types a presentation system may write (outbox resources).
    presentation_writable: BTreeSet<TypeId>,
    /// The running system's name, set with the phase; names the system in `L0028` messages. Not the
    /// recorder's attribution, which only acts while recording is armed.
    current_system: Option<&'static str>,
    /// Reads an entity's display name from a label component, for `L0028` messages (see
    /// [`set_entity_label`](World::set_entity_label)). Survives restores.
    entity_label: Option<fn(&World, u32) -> Option<String>>,
}

impl World {
    /// Creates an empty world.
    pub fn new() -> Self {
        Self::default()
    }

    // --- Entities -----------------------------------------------------------------------------

    /// Spawns a new entity and returns its handle.
    ///
    /// Reuses the most recently despawned slot (LIFO) if one is free; the returned handle carries a
    /// fresh generation so it never collides with any previously-returned handle.
    ///
    /// # Panics
    /// When [`MAX_ENTITY_SLOTS`](crate::MAX_ENTITY_SLOTS) entities are alive at once — an engine
    /// limit. With the `L0028` text in [`Presentation`](Phase::Presentation) phase: a presentation
    /// system spawns with [`spawn_presentation`](World::spawn_presentation).
    pub fn spawn(&mut self) -> EntityId {
        if self.phase == Phase::Presentation {
            panic!(
                "{}",
                self.call_crossing(
                    "World::spawn",
                    "sim ids belong to the sim",
                    "use `World::spawn_presentation`",
                )
            );
        }
        self.entities.alloc()
    }

    /// Spawns a presentation entity from the second id range and returns its handle.
    ///
    /// Presentation entities are drawn and inspectable but never hashed, and spawning or despawning
    /// one never moves a sim id. Ids reuse freed slots LIFO inside the range, like sim ids.
    ///
    /// # Panics
    /// With the `L0028` text in [`Sim`](Phase::Sim) phase: sim code cannot create state it may not
    /// see. When [`MAX_ENTITY_SLOTS`](crate::MAX_ENTITY_SLOTS) presentation entities are alive.
    pub fn spawn_presentation(&mut self) -> EntityId {
        if self.phase == Phase::Sim {
            panic!(
                "{}",
                self.call_crossing(
                    "World::spawn_presentation",
                    "the sim never holds presentation ids",
                    "move the spawn into a presentation system that reads the sim state",
                )
            );
        }
        let id = self.presentation.alloc();
        EntityId::new(PRESENTATION_BASE + id.index(), id.generation())
    }

    /// Despawns `entity` and its descendants (see [`ChildMap`]), removing all of their components.
    ///
    /// The cascade frees `entity` first, then its descendants in ascending slot index, so the free
    /// list reuses the highest descendant slot first. A sim entity takes its presentation
    /// descendants with it even in [`Sim`](Phase::Sim) phase (the presentation allocator is never
    /// hashed); a presentation entity never takes a sim child.
    ///
    /// # Errors
    /// Returns an `L0002` diagnostic if `entity` is not alive (already despawned, never existed, or
    /// a stale handle), and `L0028` if it lies across the phase boundary (see [`Phase`]).
    /// An `L0028` is also recorded for the engine to report once (see
    /// [`take_boundary_crossings`](World::take_boundary_crossings)).
    pub fn despawn(&mut self, entity: EntityId) -> Result<(), Diagnostic> {
        self.ensure_access(entity, true, "despawn", no_component)?;
        if !self.doomed_has_children(&[entity]) {
            for storage in self.storages.values_mut() {
                storage.remove_entity(entity.index);
            }
            self.free_slot(entity);
            return Ok(());
        }
        let mut doomed = vec![entity];
        doomed.extend(ChildMap::build_full(self).descendants(entity));
        let mut indices = Vec::with_capacity(doomed.len());
        for id in doomed {
            self.free_slot(id);
            indices.push(id.index);
        }
        for storage in self.storages.values_mut() {
            storage.remove_entities(&indices);
        }
        Ok(())
    }

    /// Whether any `Parent` row points at one of `ids`; the gate of the cascade. Without a hit there
    /// is no descendant, so the child map is never built.
    pub(crate) fn doomed_has_children(&self, ids: &[EntityId]) -> bool {
        let Some(storage) = self.storage_opt::<Parent>() else {
            return false;
        };
        let (_, parents) = storage.as_slices();
        match ids {
            [] => false,
            [only] => parents.iter().any(|parent| parent.0 == *only),
            _ => {
                let doomed: BTreeSet<EntityId> = ids.iter().copied().collect();
                parents.iter().any(|parent| doomed.contains(&parent.0))
            }
        }
    }

    /// `(child, parent)` for every `Parent` row whose entity index lies in `window`, ascending by
    /// child. A row exists only for a live entity, so every child resolves to its live handle.
    pub(crate) fn parent_links(&self, window: KeyWindow) -> Vec<(EntityId, EntityId)> {
        let Some(storage) = self.storage_opt::<Parent>() else {
            return Vec::new();
        };
        let (keys, parents) = storage.as_slices();
        let lo = keys.partition_point(|&key| key < window.start);
        let hi = keys.partition_point(|&key| key < window.end);
        (lo..hi)
            .map(|row| {
                let child = resolve_id(&self.entities, &self.presentation, keys[row]);
                (child, parents[row].0)
            })
            .collect()
    }

    /// Despawns every id in `ids`, compacting each component storage once.
    ///
    /// Exactly equivalent to calling [`despawn`](World::despawn) on each id in slice order,
    /// cascade included: an id that is not alive contributes its `L0002` diagnostic (in slice
    /// order) and the rest still despawn; a repeated id is reported the second time as a recycled
    /// slot, and so is an id an earlier id's cascade already killed; slots reach the free list in
    /// slice order, so the next spawns are the same ones a loop would produce; and the storages end
    /// identical. An empty result means every id was despawned.
    ///
    /// Cost is O(rows of each storage holding a doomed id + k), where a `despawn` loop pays the
    /// row count once per id. Prefer it whenever the ids are already collected.
    ///
    /// # Example
    /// ```
    /// use ludion_ecs::World;
    ///
    /// let mut world = World::new();
    /// let ids: Vec<_> = (0..4).map(|_| world.spawn()).collect();
    /// let doomed = [ids[1], ids[3], ids[1]];
    ///
    /// let errors = world.despawn_many(&doomed);
    /// assert_eq!(errors.len(), 1, "the repeated id is reported once");
    /// assert_eq!(world.entities().collect::<Vec<_>>(), vec![ids[0], ids[2]]);
    /// ```
    #[must_use = "the diagnostics name the ids that were not alive"]
    pub fn despawn_many(&mut self, ids: &[EntityId]) -> Vec<Diagnostic> {
        let mut errors = Vec::new();
        let mut doomed = Vec::with_capacity(ids.len());
        // Despawns only remove rows, so one map serves every id; a descendant freed earlier in the
        // call fails `is_alive` (its generation moved).
        let children = self
            .doomed_has_children(ids)
            .then(|| ChildMap::build_full(self));
        for &id in ids {
            match self.ensure_access(id, true, "despawn_many", no_component) {
                Ok(()) => {
                    self.free_slot(id);
                    doomed.push(id.index);
                    for d in children.iter().flat_map(|map| map.descendants(id)) {
                        if self.is_alive(d) {
                            self.free_slot(d);
                            doomed.push(d.index);
                        }
                    }
                }
                Err(diagnostic) => errors.push(diagnostic),
            }
        }
        if !doomed.is_empty() {
            for storage in self.storages.values_mut() {
                storage.remove_entities(&doomed);
            }
        }
        errors
    }

    /// Whether `entity` names the entity currently occupying its slot, in either id range. A pure
    /// liveness check, not subject to the phase guard.
    #[inline]
    pub fn is_alive(&self, entity: EntityId) -> bool {
        self.entities.is_alive(entity) || self.presentation_alive_cold(entity)
    }

    /// Whether `entity` is a live presentation entity. Out of line so the live-sim-entity hit path
    /// stays one allocator lookup: a presentation index is past the sim slots, so that lookup is
    /// simply `false` for it.
    #[cold]
    #[inline(never)]
    fn presentation_alive_cold(&self, entity: EntityId) -> bool {
        is_presentation(entity) && self.presentation.is_alive(relative(entity))
    }

    /// Whether `entity` is a live presentation entity the current phase may see (all but `Sim`).
    #[cold]
    #[inline(never)]
    fn presentation_visible_cold(&self, entity: EntityId) -> bool {
        self.phase != Phase::Sim && self.presentation_alive_cold(entity)
    }

    /// The number of currently-live sim entities.
    pub fn entity_count(&self) -> usize {
        self.entities.alive_count()
    }

    /// Iterates the currently-live **sim** entities in **ascending slot-index order**.
    ///
    /// Presentation entities are not listed (see
    /// [`presentation_entities`](World::presentation_entities)): this is the enumeration the hash,
    /// snapshots and bisection walk. It is the canonical entity enumeration order used by
    /// whole-world serialization — the world hash and snapshots visit entities in exactly this
    /// order. It is a pure function of the current world state (live slots and their generations),
    /// independent of spawn order.
    ///
    /// # Example
    /// ```
    /// use ludion_ecs::World;
    ///
    /// let mut world = World::new();
    /// let a = world.spawn();
    /// let b = world.spawn();
    /// let c = world.spawn();
    /// world.despawn(b).unwrap();
    ///
    /// let live: Vec<_> = world.entities().collect();
    /// assert_eq!(live, vec![a, c]);
    /// ```
    pub fn entities(&self) -> impl Iterator<Item = EntityId> + '_ {
        self.entities.iter_alive()
    }

    /// The live sim entity in slot `index`: `None` for a free slot, an out-of-range index or a
    /// presentation index. It pairs with [`entity_indices_with`](Self::entity_indices_with).
    ///
    /// # Example
    /// ```
    /// use ludion_ecs::World;
    ///
    /// let mut world = World::new();
    /// let a = world.spawn();
    /// assert_eq!(world.live_id_at(a.index()), Some(a));
    /// world.despawn(a).unwrap();
    /// assert_eq!(world.live_id_at(a.index()), None);
    /// ```
    pub fn live_id_at(&self, index: u32) -> Option<EntityId> {
        self.entities.live_id_at(index)
    }

    /// Iterates the currently-live presentation entities in ascending index order. Empty in
    /// [`Sim`](Phase::Sim) phase: the sim never sees them.
    pub fn presentation_entities(&self) -> impl Iterator<Item = EntityId> + '_ {
        let visible = self.phase != Phase::Sim;
        self.presentation
            .iter_alive()
            .take(if visible { usize::MAX } else { 0 })
            .map(|id| EntityId::new(PRESENTATION_BASE + id.index(), id.generation()))
    }

    /// The phase of the running system, [`Host`](Phase::Host) outside a scheduled system.
    pub fn phase(&self) -> Phase {
        self.phase
    }

    /// Sets the phase and the system running in it; only the [`Schedule`](crate::Schedule) calls
    /// it, around a system body.
    pub(crate) fn set_phase(&mut self, phase: Phase, system: Option<&'static str>) {
        self.phase = phase;
        self.current_system = system;
    }

    /// Installs the component `T` whose text names an entity in `L0028` messages (the engine
    /// installs `Name`). Read straight from the storage, so it neither re-enters the phase guard nor
    /// records an access.
    pub fn set_entity_label<T: Component + AsRef<str>>(&mut self) {
        self.entity_label = Some(read_label::<T>);
    }

    /// Lets presentation systems write the resource type `T` — for **outbox resources only**
    /// (diagnostics, an audio queue): state the sim never reads back. Any other resource write from
    /// a presentation system panics with `L0028`.
    pub fn allow_presentation_writes<T: 'static>(&mut self) {
        self.presentation_writable.insert(TypeId::of::<T>());
    }

    /// Clears the world and reconstructs exactly the given live entities at their `(index,
    /// generation)`.
    ///
    /// After this call, [`entities`](World::entities) yields precisely `live` (in ascending
    /// slot-index order) and every id in `live` is [`alive`](World::is_alive); all previous
    /// components and resources are dropped, so components must be re-inserted afterwards. Every
    /// slot index below the highest live index that is not itself in `live` becomes a free,
    /// recyclable slot at generation 0. This is the entity half of snapshot restore
    /// (`ludion-engine`'s `engine.restore`): a world rebuilt this way enumerates and hashes
    /// identically to the one that produced the snapshot.
    ///
    /// # Errors
    /// Returns an `L0025` diagnostic if `live` repeats a slot index or names one at or past
    /// [`MAX_ENTITY_SLOTS`](crate::MAX_ENTITY_SLOTS); the world is left untouched.
    ///
    /// # Example
    /// ```
    /// use ludion_ecs::World;
    /// use ludion_core::ids::EntityId;
    ///
    /// let mut world = World::new();
    /// world.restore_entities(&[EntityId::new(0, 0), EntityId::new(2, 3)]).unwrap();
    /// let live: Vec<_> = world.entities().collect();
    /// assert_eq!(live, vec![EntityId::new(0, 0), EntityId::new(2, 3)]);
    /// // The gap slot (index 1) is free and recycled first by the next spawn.
    /// assert_eq!(world.spawn(), EntityId::new(1, 0));
    /// ```
    pub fn restore_entities(&mut self, live: &[EntityId]) -> Result<(), Diagnostic> {
        let entities = EntityAllocator::from_live(live)?;
        self.storages.clear();
        self.resources.clear();
        self.entities = entities;
        self.presentation = EntityAllocator::default();
        Ok(())
    }

    /// The exact allocator state needed to reproduce id allocation across a snapshot→restore, or
    /// `None` when [`restore_entities`](World::restore_entities) over the live set alone already
    /// reconstructs it (a clean world with no recycled or trailing free slots).
    ///
    /// `restore_entities` rebuilds every non-live slot at generation 0, which is lossy: a slot that
    /// was recycled before the snapshot comes back re-issuable at generation 0, so the next spawn can
    /// mint an [`EntityId`] bit-identical to a pre-snapshot stale handle — a silent alias. When this
    /// returns `Some`, the carried state preserves dead-slot generations and free-list order, so a
    /// world rebuilt with [`restore_allocator`](World::restore_allocator) keeps stale handles reported
    /// as errors and spawns the same ids an uninterrupted run would (id determinism for replay seek).
    ///
    /// Returning `None` for the common (never-recycled) case keeps a clean snapshot byte-identical.
    /// This state is deliberately **excluded from `WorldHash`**, which observes only live slots.
    pub fn allocator_state(&self) -> Option<AllocatorState> {
        self.entities.state_if_irreducible()
    }

    /// Clears the world and reconstructs the allocator from an exact [`AllocatorState`] — the
    /// full-fidelity counterpart to [`restore_entities`](World::restore_entities), additionally
    /// restoring dead-slot generations and free-list order.
    ///
    /// After this call the live entities are exactly the slots absent from the state's free list, at
    /// their stored generations; all previous components and resources are dropped, so components must
    /// be re-inserted afterwards (as with `restore_entities`).
    ///
    /// # Errors
    /// Returns an `L0025` diagnostic if the state's free list repeats an index or names a slot past
    /// the generation table, or if the table holds more than
    /// [`MAX_ENTITY_SLOTS`](crate::MAX_ENTITY_SLOTS) slots; the world is left untouched.
    pub fn restore_allocator(&mut self, state: &AllocatorState) -> Result<(), Diagnostic> {
        let entities = EntityAllocator::from_state(state)?;
        self.storages.clear();
        self.resources.clear();
        self.entities = entities;
        self.presentation = EntityAllocator::default();
        Ok(())
    }

    /// The presentation allocator's exact state, mirroring [`allocator_state`](World::allocator_state)
    /// in indices relative to [`PRESENTATION_BASE`]; `None` when the live set alone reproduces it.
    pub fn presentation_allocator_state(&self) -> Option<AllocatorState> {
        self.presentation.state_if_irreducible()
    }

    /// Rebuilds the presentation allocator: from `state` when given, else from the live set `live`
    /// alone (each id at its own generation, gaps free). The presentation half of snapshot restore,
    /// called after [`restore_entities`](World::restore_entities) or
    /// [`restore_allocator`](World::restore_allocator), which empty it. It does **not** clear
    /// storages: the caller inserts the components afterwards.
    ///
    /// # Errors
    /// `L0025` at `/<position>` if an id of `live` is not in the presentation range or repeats a
    /// slot, and the state errors of [`restore_allocator`](World::restore_allocator); the world is
    /// left untouched.
    pub fn restore_presentation(
        &mut self,
        live: &[EntityId],
        state: Option<&AllocatorState>,
    ) -> Result<(), Diagnostic> {
        if let Some(position) = live.iter().position(|&id| !is_presentation(id)) {
            let entity = live[position];
            return Err(Diagnostic::new(
                code::ENTITY_ALLOCATOR_STATE_INVALID,
                format!(
                    "presentation entity {entity} has slot index {}, below the presentation range starting at {PRESENTATION_BASE}",
                    entity.index
                ),
            )
            .with_path(format!("/{position}"))
            .with_help("re-take the snapshot rather than editing it"));
        }
        let allocator = match state {
            Some(state) => EntityAllocator::from_state(state)?,
            None => {
                let rebased: Vec<EntityId> = live.iter().map(|&id| relative(id)).collect();
                EntityAllocator::from_live(&rebased)?
            }
        };
        self.presentation = allocator;
        Ok(())
    }

    // --- Components ---------------------------------------------------------------------------

    /// Inserts (or replaces) component `T` on `entity`, returning the previous value if there was
    /// one.
    ///
    /// Records a `(T, write)` access while recording is armed (see
    /// [`begin_query_recording`](World::begin_query_recording)).
    ///
    /// # Errors
    /// Returns an `L0002` diagnostic if `entity` is not alive, and `L0028` across the phase boundary
    /// (see [`Phase`]).
    /// An `L0028` is also recorded for the engine to report once (see
    /// [`take_boundary_crossings`](World::take_boundary_crossings)).
    pub fn insert<T: Component>(
        &mut self,
        entity: EntityId,
        value: T,
    ) -> Result<Option<T>, Diagnostic> {
        self.record_component_access::<T>(true);
        self.ensure_access(entity, true, "insert", std::any::type_name::<T>)?;
        Ok(self.storage_mut::<T>().insert(entity.index, value))
    }

    /// Removes component `T` from `entity`, returning it if present.
    ///
    /// Records a `(T, write)` access while recording is armed (see
    /// [`begin_query_recording`](World::begin_query_recording)).
    ///
    /// # Errors
    /// Returns an `L0002` diagnostic if `entity` is not alive, and `L0028` across the phase boundary
    /// (see [`Phase`]).
    /// An `L0028` is also recorded for the engine to report once (see
    /// [`take_boundary_crossings`](World::take_boundary_crossings)).
    pub fn remove<T: Component>(&mut self, entity: EntityId) -> Result<Option<T>, Diagnostic> {
        self.record_component_access::<T>(true);
        self.ensure_access(entity, true, "remove", std::any::type_name::<T>)?;
        Ok(self
            .storage_opt_mut::<T>()
            .and_then(|s| s.remove(entity.index)))
    }

    /// Gets a shared reference to component `T` on `entity`.
    ///
    /// Returns `Ok(None)` if the entity is alive but has no `T`. Records a `(T, read)` access while
    /// recording is armed (see [`begin_query_recording`](World::begin_query_recording)).
    ///
    /// # Errors
    /// Returns an `L0002` diagnostic if `entity` is not alive, and an `L0026` diagnostic if `T` is
    /// a column the enclosing [`par_for_each`](World::par_for_each) took `&mut` (that column is
    /// lent to the region, so it cannot be read through the world). `L0028` if a sim system names a
    /// presentation entity (see [`Phase`]); that `L0028` is also recorded for the engine to report
    /// once (see [`take_boundary_crossings`](World::take_boundary_crossings)).
    pub fn get<T: Component>(&self, entity: EntityId) -> Result<Option<&T>, Diagnostic> {
        self.record_component_access::<T>(false);
        self.get_unrecorded(entity)
    }

    /// [`get`](World::get) without the query-recorder entry, for engine walks (the hierarchy fold)
    /// that the query-drift gate must not attribute to the calling system. Game code uses `get`.
    ///
    /// # Errors
    /// The same as [`get`](World::get).
    ///
    /// # Example
    /// ```
    /// use ludion_ecs::World;
    /// use serde::{Deserialize, Serialize};
    /// use schemars::JsonSchema;
    ///
    /// #[derive(Serialize, Deserialize, JsonSchema)]
    /// struct Health(u32);
    ///
    /// let mut world = World::new();
    /// let e = world.spawn();
    /// world.insert(e, Health(3)).unwrap();
    ///
    /// world.begin_query_recording();
    /// world.set_recording_system("regen");
    /// assert_eq!(world.get_unrecorded::<Health>(e).unwrap().unwrap().0, 3);
    ///
    /// let observed = world.take_query_recording().unwrap();
    /// assert_eq!(observed.shapes("regen").count(), 0);
    /// ```
    pub fn get_unrecorded<T: Component>(&self, entity: EntityId) -> Result<Option<&T>, Diagnostic> {
        self.ensure_access(entity, false, "get", std::any::type_name::<T>)?;
        match self.storage_opt::<T>() {
            Some(s) => Ok(s.get(entity.index)),
            None => match self.lifted_name(TypeId::of::<T>()) {
                Some(name) => Err(Self::lifted_read_diagnostic(name)),
                None => Ok(None),
            },
        }
    }

    /// Resolves the column of component `T` once, for an engine read walk over many ids (extract).
    /// [`ReadColumn::get`] then skips the per-call type-map probe that [`get_unrecorded`](World::get_unrecorded)
    /// pays. Like `get_unrecorded` it skips the query recorder, so game systems use
    /// [`get`](World::get) or a query instead. It borrows the world, so it lives for one frame at most.
    ///
    /// # Panics
    /// With the `L0026` text if `T` is a column the enclosing [`par_for_each`](World::par_for_each)
    /// took `&mut`, like [`has`](World::has).
    ///
    /// # Example
    /// ```
    /// use ludion_ecs::World;
    /// use serde::{Deserialize, Serialize};
    /// use schemars::JsonSchema;
    ///
    /// #[derive(Serialize, Deserialize, JsonSchema)]
    /// struct Health(u32);
    ///
    /// let mut world = World::new();
    /// let (a, b, gone) = (world.spawn(), world.spawn(), world.spawn());
    /// world.insert(a, Health(3)).unwrap();
    /// world.insert(gone, Health(9)).unwrap();
    /// world.despawn(gone).unwrap();
    ///
    /// let health = world.read_column::<Health>();
    /// assert_eq!(health.get(a).unwrap().0, 3);
    /// assert!(health.get(b).is_none());
    /// assert!(health.get(gone).is_none());
    /// ```
    pub fn read_column<T: Component>(&self) -> ReadColumn<'_, T> {
        let storage = self.storage_opt::<T>();
        if storage.is_none() {
            self.panic_if_lifted(TypeId::of::<T>());
        }
        ReadColumn {
            world: self,
            storage,
        }
    }

    /// Gets an exclusive reference to component `T` on `entity`.
    ///
    /// Returns `Ok(None)` if the entity is alive but has no `T`. Records a `(T, write)` access while
    /// recording is armed (see [`begin_query_recording`](World::begin_query_recording)).
    ///
    /// # Errors
    /// Returns an `L0002` diagnostic if `entity` is not alive, and `L0028` across the phase boundary
    /// (see [`Phase`]).
    /// An `L0028` is also recorded for the engine to report once (see
    /// [`take_boundary_crossings`](World::take_boundary_crossings)).
    pub fn get_mut<T: Component>(
        &mut self,
        entity: EntityId,
    ) -> Result<Option<&mut T>, Diagnostic> {
        self.record_component_access::<T>(true);
        self.ensure_access(entity, true, "get_mut", std::any::type_name::<T>)?;
        Ok(self
            .storage_opt_mut::<T>()
            .and_then(|s| s.get_mut(entity.index)))
    }

    /// The everyday read: `T` on `entity`, or `None` when the id is dead, `T` is absent, or the id
    /// lies across the phase boundary. Use [`get`](World::get) only when you must tell a dead id
    /// from an absent component. A crossing is still recorded (see
    /// [`take_boundary_crossings`](World::take_boundary_crossings)); a dead id is not.
    ///
    /// # Panics
    /// With the `L0026` text if `T` is a column the enclosing
    /// [`par_for_each`](World::par_for_each) took `&mut`, like [`has`](World::has): an `Option`
    /// cannot carry the diagnostic, and answering `None` would be wrong.
    ///
    /// # Example
    /// ```
    /// use ludion_ecs::World;
    /// use serde::{Deserialize, Serialize};
    /// use schemars::JsonSchema;
    ///
    /// #[derive(Serialize, Deserialize, JsonSchema)]
    /// struct Health(u32);
    /// #[derive(Serialize, Deserialize, JsonSchema)]
    /// struct Armor(u32);
    ///
    /// let mut world = World::new();
    /// let e = world.spawn();
    /// world.insert(e, Health(3)).unwrap();
    /// assert_eq!(world.component::<Health>(e).unwrap().0, 3);
    /// assert!(world.component::<Armor>(e).is_none());
    ///
    /// world.despawn(e).unwrap();
    /// assert!(world.component::<Health>(e).is_none());
    /// assert!(world.take_boundary_crossings().is_empty());
    /// ```
    pub fn component<T: Component>(&self, entity: EntityId) -> Option<&T> {
        match self.get(entity) {
            Ok(value) => value,
            Err(diagnostic) if diagnostic.code == code::LIFTED_COMPONENT_READ => {
                panic!("{diagnostic}")
            }
            Err(_) => None,
        }
    }

    /// [`component`](World::component) for writing: `None` when the id is dead, `T` is absent, or
    /// the id lies across the phase boundary (recorded like [`get_mut`](World::get_mut)).
    ///
    /// # Example
    /// ```
    /// use ludion_ecs::World;
    /// use serde::{Deserialize, Serialize};
    /// use schemars::JsonSchema;
    ///
    /// #[derive(Serialize, Deserialize, JsonSchema)]
    /// struct Health(u32);
    /// #[derive(Serialize, Deserialize, JsonSchema)]
    /// struct Armor(u32);
    ///
    /// let mut world = World::new();
    /// let e = world.spawn();
    /// world.insert(e, Health(3)).unwrap();
    /// if let Some(health) = world.component_mut::<Health>(e) {
    ///     health.0 += 1;
    /// }
    /// assert_eq!(world.component::<Health>(e).unwrap().0, 4);
    /// assert!(world.component_mut::<Armor>(e).is_none());
    ///
    /// world.despawn(e).unwrap();
    /// assert!(world.component_mut::<Health>(e).is_none());
    /// assert!(world.take_boundary_crossings().is_empty());
    /// ```
    pub fn component_mut<T: Component>(&mut self, entity: EntityId) -> Option<&mut T> {
        self.get_mut(entity).ok().flatten()
    }

    /// How many rows the `T` column holds, presentation rows included; `0` when none was ever
    /// stored. O(1): a cheap "did this column change size" signal.
    pub fn component_count<T: Component>(&self) -> usize {
        self.storage_opt::<T>()
            .map_or(0, |storage| storage.as_slices().0.len())
    }

    /// Whether any `Parent` row points at `id`. O(rows of `Parent`), no allocation: a caller gates a
    /// [`ChildMap`](crate::ChildMap) build on it.
    pub fn has_children(&self, id: EntityId) -> bool {
        self.doomed_has_children(&[id])
    }

    /// Whether `entity` currently has component `T`.
    ///
    /// This is a total predicate: it returns `false` for a dead handle (a dead entity has no
    /// components) rather than erroring, so it is safe to call as a cheap guard. It reads presence,
    /// never a value, so it records no access (see
    /// [`begin_query_recording`](World::begin_query_recording)).
    ///
    /// # Panics
    /// Panics with the `L0026` text if `T` is a column the enclosing
    /// [`par_for_each`](World::par_for_each) took `&mut` — a `bool` cannot carry the diagnostic
    /// [`get`](World::get) returns, and answering `false` would be wrong. Answers `false` for a
    /// presentation entity in [`Sim`](Phase::Sim) phase: it is invisible there.
    pub fn has<T: Component>(&self, entity: EntityId) -> bool {
        if !self.entities.is_alive(entity) && !self.presentation_visible_cold(entity) {
            return false;
        }
        match self.storage_opt::<T>() {
            Some(s) => s.contains(entity.index),
            None => {
                self.panic_if_lifted(TypeId::of::<T>());
                false
            }
        }
    }

    /// The entity indices carrying the component type `type_id`, in **ascending index order** —
    /// empty when no value of that type has ever been stored.
    ///
    /// The erased, read-only counterpart to [`has`](World::has): it answers "which entities have
    /// this component?" for a type known only as a [`TypeId`], which is how the component
    /// [`Registry`](crate::Registry) knows its entries. Whole-world walks that visit every
    /// registered component of every entity (the `WorldHash`) advance one such cursor per component
    /// in step with [`entities`](World::entities), rather than probing every storage once per
    /// entity — turning an `entities × registered types` map-probe scan into one integer comparison
    /// per pair.
    ///
    /// Rows exist only for live entities (despawn drops them), so the yielded indices are live
    /// slots; pairing an index with a generation still means going through
    /// [`entities`](World::entities).
    ///
    /// # Panics
    /// Panics with the `L0026` text if `type_id` is a column the enclosing
    /// [`par_for_each`](World::par_for_each) took `&mut` — an iterator cannot carry the diagnostic,
    /// and answering empty would be wrong. In [`Sim`](Phase::Sim) phase the walk stops before
    /// [`PRESENTATION_BASE`].
    ///
    /// # Example
    /// ```
    /// use std::any::TypeId;
    /// use ludion_ecs::World;
    /// use serde::{Deserialize, Serialize};
    /// use schemars::JsonSchema;
    ///
    /// #[derive(Serialize, Deserialize, JsonSchema)]
    /// struct Health(u32);
    ///
    /// let mut world = World::new();
    /// let a = world.spawn();
    /// let b = world.spawn();
    /// world.insert(b, Health(3)).unwrap();
    /// assert_eq!(
    ///     world.entity_indices_with(TypeId::of::<Health>()).collect::<Vec<_>>(),
    ///     vec![b.index()],
    /// );
    /// assert_eq!(world.entity_indices_with(TypeId::of::<u32>()).count(), 0);
    /// let _ = a;
    /// ```
    pub fn entity_indices_with(&self, type_id: TypeId) -> Box<dyn Iterator<Item = u32> + '_> {
        match self.storages.get(&type_id) {
            Some(storage) if self.phase == Phase::Sim => Box::new(
                storage
                    .entity_indices()
                    .take_while(|&index| index < PRESENTATION_BASE),
            ),
            Some(storage) => storage.entity_indices(),
            None => {
                self.panic_if_lifted(type_id);
                Box::new(std::iter::empty())
            }
        }
    }

    /// The `(TypeId, Rust type name)` of every component type holding at least one row, in
    /// **arbitrary order** (`TypeId` order differs between builds) — sort before anything
    /// observable. How the engine finds values the registry cannot see.
    pub fn component_types(&self) -> impl Iterator<Item = (TypeId, &'static str)> + '_ {
        self.storages
            .iter()
            .filter(|(_, storage)| !storage.is_empty())
            .map(|(id, storage)| (*id, storage.type_name()))
    }

    // --- Queries ------------------------------------------------------------------------------

    /// Iterates the entities that have **all** the components named by `Q`, yielding one tuple of
    /// references per matching entity.
    ///
    /// `Q` is a tuple of `&T` / `&mut T` of arity 1–4, e.g. `(&Transform, &mut Velocity)`.
    ///
    /// # Iteration order
    /// Ascending entity index — the intersection of the columns' (ascending) key sets. The order is
    /// independent of spawn order and is conformance-tested.
    ///
    /// # Duplicate component types
    /// A query tuple must not name the same component type twice (e.g. `(&mut T, &T)`); doing so
    /// **panics**, because it would alias a single storage.
    ///
    /// # Why `&mut self`
    /// The query takes disjoint mutable borrows of the underlying storages (that is how mutable
    /// access is made sound without `unsafe`), so it needs `&mut World` even for read-only tuples.
    ///
    /// # Example
    /// ```
    /// use ludion_ecs::World;
    /// use serde::{Deserialize, Serialize};
    /// use schemars::JsonSchema;
    ///
    /// #[derive(Serialize, Deserialize, JsonSchema)]
    /// struct Position { x: i32 }
    /// #[derive(Serialize, Deserialize, JsonSchema)]
    /// struct Velocity { dx: i32 }
    ///
    /// let mut world = World::new();
    /// let e = world.spawn();
    /// world.insert(e, Position { x: 0 }).unwrap();
    /// world.insert(e, Velocity { dx: 3 }).unwrap();
    ///
    /// for (pos, vel) in world.query::<(&mut Position, &Velocity)>() {
    ///     pos.x += vel.dx;
    /// }
    /// assert_eq!(world.get::<Position>(e).unwrap().unwrap().x, 3);
    /// ```
    ///
    /// # Phase
    /// A [`Sim`](Phase::Sim) system's query never reaches a presentation row. A
    /// [`Presentation`](Phase::Presentation) system's query that names any `&mut` column joins over
    /// presentation rows only; a read-only one sees every row.
    pub fn query<'w, Q: Query<'w>>(&'w mut self) -> Q::Iter {
        // Recording is off in every shipping/hashed run, so this is a single `Option` check on the
        // hot path. When on, the shape is observed *before* the join borrows the world; observation
        // never changes what the query yields (determinism: recording only watches).
        if self.recording.is_some() {
            self.record_query_shape(Q::shape());
        }
        Q::build(self)
    }

    /// Like [`query`](World::query), but yields the matching entity's [`EntityId`] alongside its
    /// component tuple: `(EntityId, Q::Item)` per row.
    ///
    /// Same join, same rows, same ascending-entity-index order as [`query`](World::query) — the id
    /// is the only addition. Reach for it when the row needs the entity's identity, not just its
    /// components: staging a deferred despawn, recording a spatial-lookup key, or referencing another
    /// entity. The yielded id carries the slot's live generation, so it is a valid handle to pass
    /// straight back into [`get`](World::get) / [`despawn`](World::despawn).
    ///
    /// # Shape recording
    /// An id-yielding query is the **same shape** as the plain one — it records `Q::shape()`
    /// identically, so the query-drift verifier and `engine.describe` see no difference.
    ///
    /// # Duplicate component types
    /// Naming the same type twice **panics**, exactly as for [`query`](World::query).
    ///
    /// # Example
    /// ```
    /// use ludion_ecs::World;
    /// use serde::{Deserialize, Serialize};
    /// use schemars::JsonSchema;
    ///
    /// #[derive(Serialize, Deserialize, JsonSchema)]
    /// struct Health { hp: i32 }
    ///
    /// let mut world = World::new();
    /// let a = world.spawn();
    /// let b = world.spawn();
    /// world.insert(a, Health { hp: 0 }).unwrap();
    /// world.insert(b, Health { hp: 7 }).unwrap();
    ///
    /// // Collect the ids of entities that are out of health, then despawn them.
    /// let dead: Vec<_> = world
    ///     .query_ids::<(&Health,)>()
    ///     .filter(|(_, (h,))| h.hp <= 0)
    ///     .map(|(id, _)| id)
    ///     .collect();
    /// assert!(world.despawn_many(&dead).is_empty());
    /// assert!(!world.is_alive(a));
    /// assert!(world.is_alive(b));
    /// ```
    pub fn query_ids<'w, Q: Query<'w>>(&'w mut self) -> QueryIds<'w, Q> {
        // Identical recording semantics to `query`: an id-yielding query observes the same shape.
        if self.recording.is_some() {
            self.record_query_shape(Q::shape());
        }
        Q::build_ids(self)
    }

    /// Enables opt-in component-access recording (host/test only). While enabled, the world observes
    /// the full read/write set of each system the [`Schedule`](crate::Schedule) tags via
    /// [`set_recording_system`](World::set_recording_system):
    ///
    /// - every [`query`](World::query) / [`query_ids`](World::query_ids) records its component shape;
    /// - every per-entity accessor records the one-component shape it names — [`get`](World::get) a
    ///   read, [`get_mut`](World::get_mut) / [`insert`](World::insert) / [`remove`](World::remove) a
    ///   write — so the snapshot-then-`get_mut` idiom the `&mut self` borrow model forces is visible
    ///   too.
    ///
    /// **A missed access still records.** A `get` on a dead handle or on an entity without the
    /// component records the same shape as a hit: what the observation is for is the system's access
    /// *intent*, which does not depend on whether this run's data happened to satisfy it.
    /// [`has`](World::has), [`spawn`](World::spawn), [`despawn`](World::despawn), and the resource
    /// accessors record nothing — they name no component value.
    ///
    /// Idempotent; starting again clears prior observations. Recording never enters the `WorldHash`,
    /// a snapshot, or a replay.
    ///
    /// # Example
    /// ```
    /// use std::any::TypeId;
    /// use ludion_ecs::World;
    /// use serde::{Deserialize, Serialize};
    /// use schemars::JsonSchema;
    ///
    /// #[derive(Serialize, Deserialize, JsonSchema)]
    /// struct Health(u32);
    ///
    /// let mut world = World::new();
    /// let e = world.spawn();
    /// world.insert(e, Health(3)).unwrap();
    ///
    /// world.begin_query_recording();
    /// world.set_recording_system("regen");
    /// // A per-entity read observes the same one-component shape a `(&Health,)` query would.
    /// let _ = world.get::<Health>(e).unwrap();
    /// let observed = world.take_query_recording().unwrap();
    ///
    /// let shapes: Vec<_> = observed.shapes("regen").map(<[_]>::to_vec).collect();
    /// assert_eq!(shapes, vec![vec![(TypeId::of::<Health>(), false)]]);
    /// ```
    pub fn begin_query_recording(&mut self) {
        self.recording = Some(Mutex::new(QueryRecording::default()));
    }

    /// Stops recording and returns the accumulated per-system observations, or `None` if recording was
    /// not enabled. Leaves the world non-recording.
    pub fn take_query_recording(&mut self) -> Option<QueryObservations> {
        self.recording.take().map(|rec| {
            let rec = unpoison(rec.into_inner());
            QueryObservations {
                per_system: rec.observed,
                type_names: rec.type_names,
            }
        })
    }

    /// Tags the system whose accesses the recorder attributes from here on — called by the
    /// [`Schedule`](crate::Schedule) before each system runs. A no-op unless recording is enabled, so
    /// the world stays name-agnostic outside its recording state (the schedule may call it every tick).
    pub fn set_recording_system(&mut self, name: &'static str) {
        if let Some(rec) = self.recording.as_mut() {
            unpoison(rec.get_mut()).current_system = Some(name);
        }
    }

    /// Drops the current attribution — the closing bracket of
    /// [`set_recording_system`](World::set_recording_system), called by the
    /// [`Schedule`](crate::Schedule) once its systems have run. Accesses made afterwards (the host's
    /// extract, world hash, command application, and scene loading all read and write components)
    /// belong to no system and are recorded nowhere. A no-op unless recording is enabled.
    ///
    /// # Example
    /// ```
    /// use ludion_ecs::World;
    /// use serde::{Deserialize, Serialize};
    /// use schemars::JsonSchema;
    ///
    /// #[derive(Serialize, Deserialize, JsonSchema)]
    /// struct Health(u32);
    ///
    /// let mut world = World::new();
    /// let e = world.spawn();
    ///
    /// world.begin_query_recording();
    /// world.set_recording_system("regen");
    /// world.clear_recording_system();
    /// world.insert(e, Health(3)).unwrap(); // host-side, not a system: unattributed
    ///
    /// let observed = world.take_query_recording().unwrap();
    /// assert_eq!(observed.shapes("regen").count(), 0);
    /// ```
    pub fn clear_recording_system(&mut self) {
        if let Some(rec) = self.recording.as_mut() {
            unpoison(rec.get_mut()).current_system = None;
        }
    }

    /// Whether the opt-in access recorder is armed. The guard callers put in front of the
    /// shape-normalizing [`record_query_shape`](World::record_query_shape) so a non-recording run
    /// pays one `Option` check and no allocation.
    pub(crate) fn is_recording(&self) -> bool {
        self.recording.is_some()
    }

    /// Records one query's shape under the currently-tagged system.
    pub(crate) fn record_query_shape(&self, shape: Vec<crate::query::QueryElemShape>) {
        // Normalize to a sorted, deduped set of (component, access) so tuple order is irrelevant and
        // equal shapes compare equal (the drift verdict treats a query as a set).
        let mut normalized: ObservedShape = shape.iter().map(|e| (e.type_id, e.write)).collect();
        normalized.sort_unstable();
        normalized.dedup();
        self.record_shape(normalized, shape.iter().map(|e| (e.type_id, e.type_name)));
    }

    /// Records one per-entity accessor's single-component shape under the currently-tagged system.
    /// Inlined-away to the `Option` discriminant check when recording is off, which is what keeps
    /// [`get`](World::get) — the sim's hottest path — free of recording cost.
    #[inline]
    fn record_component_access<T: Component>(&self, write: bool) {
        if self.recording.is_some() {
            self.record_shape(
                vec![(TypeId::of::<T>(), write)],
                [(TypeId::of::<T>(), std::any::type_name::<T>())],
            );
        }
    }

    /// Files a normalized shape under the currently-tagged system, remembering the Rust names of
    /// its component types. Skips accesses made outside a scheduled system (no attribution target)
    /// and outside recording.
    fn record_shape(
        &self,
        normalized: ObservedShape,
        names: impl IntoIterator<Item = (TypeId, &'static str)>,
    ) {
        let Some(rec) = self.recording.as_ref() else {
            return;
        };
        // No recorded accessor re-enters the recorder, so the lock is held only for the insert. Any
        // number of `par_for_each` worker chunks may reach it at once; the observation is a *set*,
        // so what order they arrive in is not observable.
        let mut rec = unpoison(rec.lock());
        let Some(system) = rec.current_system else {
            return;
        };
        rec.observed.entry(system).or_default().insert(normalized);
        rec.type_names.extend(names);
    }

    // --- Resources ----------------------------------------------------------------------------

    /// Inserts (or replaces) the resource of type `T`, returning the previous value if any.
    ///
    /// Resources are singletons keyed by type — the canonical home for things like the one seeded
    /// `DeterministicRng`. They are accessed by type and never iterated. `T: Send + Sync` so the
    /// world stays `Send + Sync`: `Send` for the sim thread that owns it, `Sync` so a
    /// [`par_for_each`](World::par_for_each) chunk can read resources through its `&World` (see the
    /// `resources` field).
    ///
    /// # Panics
    /// With the `L0028` text in [`Presentation`](Phase::Presentation) phase unless `T` is allowed by
    /// [`allow_presentation_writes`](World::allow_presentation_writes); likewise
    /// [`resource_mut`](World::resource_mut) and [`remove_resource`](World::remove_resource).
    pub fn insert_resource<T: 'static + Send + Sync>(&mut self, value: T) -> Option<T> {
        self.guard_resource_write::<T>("insert_resource");
        self.resources
            .insert(TypeId::of::<T>(), Box::new(value))
            .map(|boxed| {
                *boxed
                    .downcast::<T>()
                    .expect("resource TypeId matches stored type")
            })
    }

    /// Gets a shared reference to the resource of type `T`, if present.
    pub fn resource<T: 'static>(&self) -> Option<&T> {
        self.resources.get(&TypeId::of::<T>()).map(|boxed| {
            boxed
                .downcast_ref::<T>()
                .expect("resource TypeId matches stored type")
        })
    }

    /// Gets an exclusive reference to the resource of type `T`, if present.
    pub fn resource_mut<T: 'static>(&mut self) -> Option<&mut T> {
        self.guard_resource_write::<T>("resource_mut");
        self.resources.get_mut(&TypeId::of::<T>()).map(|boxed| {
            boxed
                .downcast_mut::<T>()
                .expect("resource TypeId matches stored type")
        })
    }

    /// Removes and returns the resource of type `T`, if present.
    pub fn remove_resource<T: 'static>(&mut self) -> Option<T> {
        self.guard_resource_write::<T>("remove_resource");
        self.resources.remove(&TypeId::of::<T>()).map(|boxed| {
            *boxed
                .downcast::<T>()
                .expect("resource TypeId matches stored type")
        })
    }

    // --- Internal helpers ---------------------------------------------------------------------

    /// Returns `Ok(())` if `entity` is alive, else an `L0002` diagnostic naming the reason.
    pub(crate) fn ensure_alive(&self, entity: EntityId) -> Result<(), Diagnostic> {
        if self.is_alive(entity) {
            Ok(())
        } else {
            Err(self.dead_entity_diagnostic(entity))
        }
    }

    /// The phase guard, then liveness: a crossing reports `L0028`, not `L0002`. A live sim entity —
    /// the hit path — is one allocator lookup and, for a write, one phase compare; every other id
    /// takes the cold path. `op` and `component` (the accessor's name and a thunk for the component
    /// type's name) reach only the cold message builder. A dead sim id is `L0002` in every phase.
    #[inline]
    fn ensure_access(
        &self,
        entity: EntityId,
        write: bool,
        op: &'static str,
        component: fn() -> &'static str,
    ) -> Result<(), Diagnostic> {
        if self.entities.is_alive(entity) {
            if write && self.phase == Phase::Presentation {
                return Err(self.refuse(entity, op, component));
            }
            return Ok(());
        }
        self.access_cold(entity, op, component)
    }

    /// [`ensure_access`](World::ensure_access) for an id that is not a live sim entity: a presentation
    /// id (refused in `Sim`, live-checked otherwise) or a dead id (`L0002`).
    #[cold]
    #[inline(never)]
    fn access_cold(
        &self,
        entity: EntityId,
        op: &'static str,
        component: fn() -> &'static str,
    ) -> Result<(), Diagnostic> {
        if is_presentation(entity) && self.phase == Phase::Sim {
            return Err(self.refuse(entity, op, component));
        }
        self.ensure_alive(entity)
    }

    /// The `L0028` for a refused accessor, queued for the host unless its key is already queued
    /// (so a world nobody drains stays bounded by the distinct keys).
    #[cold]
    #[inline(never)]
    fn refuse(
        &self,
        entity: EntityId,
        op: &'static str,
        component: fn() -> &'static str,
    ) -> Diagnostic {
        let name = component();
        let system = self.current_system.unwrap_or("-");
        let key = if name.is_empty() {
            format!("{system}/{op}")
        } else {
            format!("{system}/{op}/{}", short_name(name))
        };
        let diagnostic = self.entity_crossing(entity, op, name);
        let mut queued = unpoison(self.crossings.lock());
        if queued.iter().all(|c| c.key != key) {
            queued.push(BoundaryCrossing {
                key,
                diagnostic: diagnostic.clone(),
            });
        }
        diagnostic
    }

    /// Drains the queued `L0028` refusals. Every `get`/`get_mut`/`insert`/`remove`/`despawn`/
    /// `despawn_many` that crossed the phase boundary is recorded here whatever the caller did with
    /// its `Result`, one entry per distinct `"{system}/{op}/{Component}"` key since the last drain
    /// (`L0002` is never recorded). The engine drains this at the frame boundary into its
    /// diagnostics; a bare driver world only grows to the distinct keys. Entries arrive in
    /// nondeterministic order when a refusal comes from a `par_for_each` chunk — sort by `key`.
    pub fn take_boundary_crossings(&mut self) -> Vec<BoundaryCrossing> {
        std::mem::take(unpoison(self.crossings.get_mut()))
    }

    /// The keys of the refusals [`take_boundary_crossings`](World::take_boundary_crossings) would
    /// drain, without draining them.
    pub fn pending_boundary_crossings(&self) -> Vec<String> {
        unpoison(self.crossings.lock())
            .iter()
            .map(|c| c.key.clone())
            .collect()
    }

    /// The running system as `sim system `name``, or just `sim system` outside a named one.
    #[cold]
    #[inline(never)]
    fn who(&self) -> String {
        let kind = if self.phase == Phase::Sim {
            "sim"
        } else {
            "presentation"
        };
        match self.current_system {
            Some(name) => format!("{kind} system `{name}`"),
            None => format!("{kind} system"),
        }
    }

    /// `e5v0 (`ball`)` when a label is installed and present, else the bare id.
    #[cold]
    #[inline(never)]
    fn entity_text(&self, entity: EntityId) -> String {
        match self
            .entity_label
            .and_then(|read| read(self, entity.index))
            .filter(|label| !label.is_empty())
        {
            Some(label) => format!("{entity} (`{label}`)"),
            None => entity.to_string(),
        }
    }

    /// The `L0028` for an accessor that named an entity across the boundary.
    #[cold]
    #[inline(never)]
    fn entity_crossing(
        &self,
        entity: EntityId,
        op: &'static str,
        component: &'static str,
    ) -> Diagnostic {
        let through = if component.is_empty() {
            format!("`{op}`")
        } else {
            format!("`{op}::<{}>`", short_name(component))
        };
        let who = self.who();
        let text = self.entity_text(entity);
        if self.phase == Phase::Sim {
            let verb = if op == "get" { "read" } else { "wrote" };
            Diagnostic::new(
                code::PRESENTATION_BOUNDARY,
                format!(
                    "{who} {verb} presentation entity {text} through {through}; the sim never sees presentation state"
                ),
            )
            .with_help(
                "if the sim must read it, make it a sim entity (spawn it with `World::spawn`, or drop \
                 `presentation: true`); a camera the sim unprojects through must be a sim entity. \
                 Otherwise move this logic into a presentation system",
            )
        } else {
            Diagnostic::new(
                code::PRESENTATION_BOUNDARY,
                format!(
                    "{who} wrote sim entity {text} through {through}; sim state is read-only to presentation"
                ),
            )
            .with_help(
                "do the write in a sim system, or write the derived value onto a presentation entity",
            )
        }
    }

    /// The `L0028` for a call that is refused outright (`spawn`, `spawn_presentation`).
    #[cold]
    #[inline(never)]
    fn call_crossing(&self, call: &str, why: &str, help: &str) -> Diagnostic {
        Diagnostic::new(
            code::PRESENTATION_BOUNDARY,
            format!("{} called `{call}`; {why}", self.who()),
        )
        .with_help(help.to_string())
    }

    /// Panics with the `L0028` text when a presentation system writes a resource that is not an
    /// allowlisted outbox.
    #[inline]
    fn guard_resource_write<T: 'static>(&self, op: &'static str) {
        if self.phase == Phase::Presentation
            && !self.presentation_writable.contains(&TypeId::of::<T>())
        {
            self.resource_crossing(op, std::any::type_name::<T>());
        }
    }

    #[cold]
    #[inline(never)]
    fn resource_crossing(&self, op: &'static str, resource: &'static str) -> ! {
        panic!(
            "{}",
            Diagnostic::new(
                code::PRESENTATION_BOUNDARY,
                format!(
                    "{} wrote resource `{}` through `{op}`; sim resources are read-only to presentation",
                    self.who(),
                    short_name(resource)
                ),
            )
            .with_help(
                "write only through an allowlisted outbox (`World::allow_presentation_writes`); \
                 for randomness, use `DeterministicRng::derived` from `WorldSeed` and the frame",
            )
        )
    }

    /// Frees `entity`'s slot in the allocator of its range. The caller verified it is alive.
    fn free_slot(&mut self, entity: EntityId) {
        if is_presentation(entity) {
            self.presentation.free(relative(entity));
        } else {
            self.entities.free(entity);
        }
    }

    /// The key window a query sees in the current phase (see [`World::query`]); `write` is whether
    /// the query names any `&mut` column.
    pub(crate) fn query_window(&self, write: bool) -> KeyWindow {
        match self.phase {
            Phase::Sim => 0..PRESENTATION_BASE,
            Phase::Presentation if write => PRESENTATION_BASE..u32::MAX,
            _ => FULL_WINDOW,
        }
    }

    /// Builds the `L0002` diagnostic for a non-alive handle.
    pub(crate) fn dead_entity_diagnostic(&self, entity: EntityId) -> Diagnostic {
        Diagnostic::new(
            code::DEAD_ENTITY,
            format!(
                "entity {entity} is not alive: {}",
                if is_presentation(entity) {
                    self.presentation.not_alive_reason(relative(entity))
                } else {
                    self.entities.not_alive_reason(entity)
                }
            ),
        )
        .with_help(
            "only use EntityIds returned by the current World::spawn; a handle is invalid once its \
             entity is despawned",
        )
    }

    /// Builds the `L0026` diagnostic for a read of a column the running region lifted.
    fn lifted_read_diagnostic(type_name: &str) -> Diagnostic {
        Diagnostic::new(
            code::LIFTED_COMPONENT_READ,
            format!(
                "`{type_name}` was read through the world inside a `par_for_each` that took it \
                 `&mut`: its column is lent to the region"
            ),
        )
        .with_help(
            "read it through the row item the closure receives, read a different component, or \
             make the system sequential",
        )
    }

    /// The Rust type name of `type_id` if a running `par_for_each` region has that column lifted.
    fn lifted_name(&self, type_id: TypeId) -> Option<&'static str> {
        self.lifted
            .iter()
            .find(|(lifted, _)| *lifted == type_id)
            .map(|&(_, name)| name)
    }

    /// Panics with the `L0026` text when `type_id` is a column the running region lifted. Called on
    /// the storage-absent arm of readers that cannot return the diagnostic.
    pub(crate) fn panic_if_lifted(&self, type_id: TypeId) {
        if let Some(name) = self.lifted_name(type_id) {
            panic!("{}", Self::lifted_read_diagnostic(name));
        }
    }

    /// Gets the storage for `T`, creating it if absent.
    fn storage_mut<T: Component>(&mut self) -> &mut Storage<T> {
        self.storages
            .entry(TypeId::of::<T>())
            .or_insert_with(|| Box::new(Storage::<T>::new()))
            .as_any_mut()
            .downcast_mut::<Storage<T>>()
            .expect("storage TypeId matches component type")
    }

    /// Gets the storage for `T` if it has ever been created.
    pub(crate) fn storage_opt<T: Component>(&self) -> Option<&Storage<T>> {
        self.storages.get(&TypeId::of::<T>()).map(|boxed| {
            boxed
                .as_any()
                .downcast_ref::<Storage<T>>()
                .expect("storage TypeId matches component type")
        })
    }

    /// Gets the storage for `T` mutably if it has ever been created.
    fn storage_opt_mut<T: Component>(&mut self) -> Option<&mut Storage<T>> {
        self.storages.get_mut(&TypeId::of::<T>()).map(|boxed| {
            boxed
                .as_any_mut()
                .downcast_mut::<Storage<T>>()
                .expect("storage TypeId matches component type")
        })
    }

    /// Returns disjoint mutable borrows of the storages named by `ids`, in `ids` order (`None` for
    /// a type that has no storage yet). `ids` must be duplicate-free; the query builder guarantees
    /// this before calling. Used only by the query machinery.
    pub(crate) fn many_storages_mut(&mut self, ids: &[TypeId]) -> Vec<Option<&mut dyn AnyStorage>> {
        collect_storages(&mut self.storages, ids)
    }

    /// The id-yielding counterpart of [`many_storages_mut`](World::many_storages_mut): the storages
    /// named by `ids` (mutable) **and** a shared borrow of the entity allocator, taken disjointly.
    /// `entities` and `storages` are separate fields, so the mutable storage borrows and the shared
    /// allocator borrows (one per id range) coexist without `unsafe`. The allocators supply the
    /// live generation for each joined row's slot index. `ids` must be duplicate-free; used only by
    /// [`Query::build_ids`].
    ///
    /// [`Query::build_ids`]: crate::query::Query::build_ids
    pub(crate) fn split_storages_and_entities(
        &mut self,
        ids: &[TypeId],
    ) -> (
        &EntityAllocator,
        &EntityAllocator,
        Vec<Option<&mut dyn AnyStorage>>,
    ) {
        let storages = collect_storages(&mut self.storages, ids);
        (&self.entities, &self.presentation, storages)
    }

    /// Lifts the whole storage box for `type_id` **out** of the world, leaving that component type
    /// absent until [`put_storage_erased`](World::put_storage_erased) puts it back. The exclusive
    /// half of [`par_for_each`](World::par_for_each)'s region model: an owned box is what lets a
    /// chunk hold a `&mut` sub-slice of a column while other chunks hold theirs and every closure
    /// holds a `&World`.
    pub(crate) fn take_storage_erased(&mut self, type_id: TypeId) -> Option<Box<dyn AnyStorage>> {
        let storage = self.storages.remove(&type_id);
        if let Some(storage) = &storage {
            self.lifted.push((type_id, storage.type_name()));
        }
        storage
    }

    /// Puts a box lifted by [`take_storage_erased`](World::take_storage_erased) back under its
    /// `TypeId`, closing the region.
    pub(crate) fn put_storage_erased(&mut self, type_id: TypeId, storage: Box<dyn AnyStorage>) {
        self.lifted.retain(|(t, _)| *t != type_id);
        self.storages.insert(type_id, storage);
    }

    /// The live [`EntityId`] occupying slot `index`. Only ever called with an index a column key
    /// yielded, and rows exist only for live entities, so the slot is alive and this reads its true
    /// generation (the same resolution [`query_ids`](World::query_ids) does per row).
    pub(crate) fn id_at(&self, index: u32) -> EntityId {
        resolve_id(&self.entities, &self.presentation, index)
    }
}

/// One recorded `L0028` refusal: the dedupe `key` (`"{system}/{op}/{Component}"`, or
/// `"{system}/despawn"`; `system` is `-` outside a named system) and the diagnostic the caller got.
#[derive(Debug, Clone)]
pub struct BoundaryCrossing {
    /// The once-per-session latch key.
    pub key: String,
    /// The refusal as the accessor returned it.
    pub diagnostic: Diagnostic,
}

/// The component thunk of an accessor that names no component type.
fn no_component() -> &'static str {
    ""
}

/// The last path segment of a type name: `game::fx::Popup` → `Popup`.
fn short_name(type_name: &str) -> &str {
    type_name.rsplit("::").next().unwrap_or(type_name)
}

/// Reads label component `T` of slot `index` straight from its storage — no phase guard, no
/// recorded access.
#[cold]
#[inline(never)]
fn read_label<T: Component + AsRef<str>>(world: &World, index: u32) -> Option<String> {
    world
        .storage_opt::<T>()?
        .get(index)
        .map(|label| label.as_ref().to_owned())
}

/// `entity` with its index made relative to [`PRESENTATION_BASE`], for the presentation allocator.
fn relative(entity: EntityId) -> EntityId {
    EntityId::new(entity.index() - PRESENTATION_BASE, entity.generation())
}

/// Compile-time proof of the contract in [`World`]'s docs: the world is `Send` (the sim thread owns
/// it) **and** `Sync` (a [`par_for_each`](World::par_for_each) chunk holds `&World`). Both properties
/// are load-bearing, and both are one field away from being lost silently — a resource type or an
/// interior-mutability cell that is not `Sync` would break the parallel entry points at their call
/// sites, far from the cause. This fails here instead.
const _: fn() = || {
    fn assert_send_sync<T: Send + Sync>() {}
    assert_send_sync::<World>();
};

/// Unwraps a lock result, ignoring poisoning.
///
/// The recorder's lock is held only across a `BTreeSet` insert that cannot panic, so a poisoned lock
/// can only mean a panic elsewhere while a guard happened to be live. The observations behind it are
/// intact either way, and observation must never be the thing that fails a run.
fn unpoison<T>(result: Result<T, PoisonError<T>>) -> T {
    result.unwrap_or_else(PoisonError::into_inner)
}

/// Collects disjoint mutable borrows of the storages named by `ids`, in `ids` order (`None` for a
/// type with no storage yet). Free function over the `storages` field alone, so a caller can hold a
/// disjoint borrow of another `World` field at the same time (that is how
/// [`World::split_storages_and_entities`] borrows the entity allocator alongside the storages).
fn collect_storages<'a>(
    storages: &'a mut BTreeMap<TypeId, Box<dyn AnyStorage>>,
    ids: &[TypeId],
) -> Vec<Option<&'a mut dyn AnyStorage>> {
    let mut out: Vec<Option<&mut dyn AnyStorage>> = (0..ids.len()).map(|_| None).collect();
    for (tid, storage) in storages.iter_mut() {
        if let Some(pos) = ids.iter().position(|t| t == tid) {
            out[pos] = Some(&mut **storage);
        }
    }
    out
}

/// One component type's column, resolved once by [`World::read_column`]. [`get`](ReadColumn::get)
/// answers exactly what `world.get_unrecorded::<T>(id).ok().flatten()` does, without the per-call
/// type-map probe.
pub struct ReadColumn<'w, T> {
    world: &'w World,
    storage: Option<&'w Storage<T>>,
}

impl<'w, T: Component> ReadColumn<'w, T> {
    /// `T` on `entity`, or `None` when the id is dead, `T` is absent, or the id lies across the
    /// phase boundary (that crossing is queued like [`World::get_unrecorded`]'s). A recycled slot
    /// never answers for the stale id.
    #[inline]
    pub fn get(&self, entity: EntityId) -> Option<&'w T> {
        if self.world.entities.is_alive(entity) {
            return self.storage?.get(entity.index);
        }
        self.world.get_unrecorded::<T>(entity).ok().flatten()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_cascade_probe_fires_only_for_a_doomed_parent() {
        let mut world = World::new();
        let root = world.spawn();
        let child = world.spawn();
        let other = world.spawn();
        assert!(!world.doomed_has_children(&[root]));
        world.insert(child, Parent(root)).unwrap();
        assert!(world.doomed_has_children(&[root]));
        assert!(world.doomed_has_children(&[other, root]));
        assert!(!world.doomed_has_children(&[child]));
        assert!(!world.doomed_has_children(&[child, other]));
        assert!(!world.doomed_has_children(&[]));
    }

    #[test]
    fn despawning_a_leaf_or_nothing_leaves_siblings_alone() {
        let mut world = World::new();
        let root = world.spawn();
        let (a, b) = (world.spawn(), world.spawn());
        world.insert(a, Parent(root)).unwrap();
        world.insert(b, Parent(root)).unwrap();
        assert!(world.despawn_many(&[]).is_empty());
        assert!(world.despawn_many(&[a]).is_empty());
        assert!(world.is_alive(root) && world.is_alive(b) && !world.is_alive(a));
        world.despawn(root).unwrap();
        assert!(!world.is_alive(b));
    }

    #[derive(serde::Serialize, serde::Deserialize, schemars::JsonSchema, Debug, PartialEq)]
    struct Probe(u64);

    #[derive(serde::Serialize, serde::Deserialize, schemars::JsonSchema)]
    struct Never;

    fn as_ptr<T>(r: Option<&T>) -> Option<*const T> {
        r.map(|r| r as *const T)
    }

    #[test]
    fn read_column_matches_get_unrecorded_in_every_phase() {
        use ludion_core::rng::DeterministicRng;

        let mut rng = DeterministicRng::new(0x5EAD_C0DE);
        let mut world = World::new();
        let mut ids = Vec::new();
        for round in 0..6 {
            for _ in 0..40 {
                let sim = world.spawn();
                let pres = world.spawn_presentation();
                ids.extend([sim, pres]);
            }
            // Despawn a random subset (stale ids stay in `ids`), so later rounds recycle slots.
            for &id in &ids {
                if rng.gen_range_u32(0..4) == 0 && world.is_alive(id) {
                    world.despawn(id).ok();
                }
            }
            for &id in &ids {
                if world.is_alive(id) && rng.gen_range_u32(0..2) == 0 {
                    world.insert(id, Probe(rng.next_u64() ^ round)).unwrap();
                }
            }
        }

        for phase in [Phase::Host, Phase::Sim, Phase::Presentation] {
            world.set_phase(phase, Some("oracle"));
            let col = world.read_column::<Probe>();
            let got: Vec<_> = ids.iter().map(|&id| as_ptr(col.get(id))).collect();
            let got_crossings = world.take_boundary_crossings();
            let want: Vec<_> = ids
                .iter()
                .map(|&id| as_ptr(world.get_unrecorded::<Probe>(id).ok().flatten()))
                .collect();
            let want_crossings = world.take_boundary_crossings();
            assert_eq!(got, want, "values differ in {phase:?}");
            assert_eq!(
                got_crossings.iter().map(|c| &c.key).collect::<Vec<_>>(),
                want_crossings.iter().map(|c| &c.key).collect::<Vec<_>>(),
                "crossings differ in {phase:?}"
            );
            assert!(
                got.iter().any(Option::is_some),
                "oracle must hit in {phase:?}"
            );
        }
        world.set_phase(Phase::Host, None);
    }

    #[test]
    fn read_column_of_a_never_created_column_is_none() {
        let mut world = World::new();
        let e = world.spawn();
        let col = world.read_column::<Never>();
        assert!(col.get(e).is_none());
    }
}
