//! The entity allocator: generational slots with a deterministic free-list.
//!
//! # Determinism contract
//!
//! The free-list is a **LIFO stack**: [`EntityAllocator::alloc`] reuses the *most recently freed*
//! slot first. This choice is deliberate and conformance-tested — a FIFO queue would iterate and
//! recycle in a different, equally-valid order, so exactly one order is fixed here so that spawn
//! sequences are reproducible across runs, processes, and platforms.
//!
//! Generations are bumped on *free*, so a despawned handle can never match its slot again, even
//! before the slot is reused. That is what makes a stale [`EntityId`] a reported error rather than a
//! silent hit on the entity that later inherits the slot.

use ludion_core::codes::code;
use ludion_core::diagnostics::Diagnostic;
use ludion_core::ids::EntityId;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// The most entity slots a world can hold — an engine limit, enforced by `EntityAllocator::alloc`
/// and on every restore. The LIFO free list reuses despawned slots, so the slot count is the peak
/// number of *concurrent* live entities, never the total ever spawned; the largest measured worlds
/// hold about 3,000. The cap is over 3,000× that, and a restore at the cap allocates about 200 MB
/// rather than the tens of GB a hand-edited index would ask for.
///
/// The cap applies to each id range separately: sim entities take indices below
/// [`PRESENTATION_BASE`], presentation entities the same number of slots above it.
pub const MAX_ENTITY_SLOTS: u32 = 1 << 24;

/// The first entity index of the presentation range. A sim entity has `index < PRESENTATION_BASE`; a
/// presentation entity has `PRESENTATION_BASE <= index < 2 * PRESENTATION_BASE`. The two ranges have
/// separate allocators, so spawning a presentation entity never moves a sim id.
pub const PRESENTATION_BASE: u32 = MAX_ENTITY_SLOTS;

/// Whether `id` lies in the presentation range (see [`PRESENTATION_BASE`]).
pub fn is_presentation(id: EntityId) -> bool {
    id.index() >= PRESENTATION_BASE
}

/// The live id of slot `index`, drawn from the allocator of the range the index lies in. A
/// presentation slot is never looked up in the sim allocator.
pub(crate) fn resolve_id(
    sim: &EntityAllocator,
    presentation: &EntityAllocator,
    index: u32,
) -> EntityId {
    if index >= PRESENTATION_BASE {
        let id = presentation.id_at(index - PRESENTATION_BASE);
        EntityId::new(index, id.generation())
    } else {
        sim.id_at(index)
    }
}

/// The phase a system runs in; the world enforces the sim/presentation boundary by it (`L0028`).
///
/// Only the [`Schedule`](crate::Schedule) sets it, around each system body. Everything outside a
/// system — command application, extract, hashing, scene loading — runs in [`Host`](Phase::Host),
/// which has no restriction.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Phase {
    /// Outside any system: unrestricted.
    #[default]
    Host,
    /// A sim system: presentation entities are invisible.
    Sim,
    /// A presentation system: sim entities are read-only and sim resources are read-only.
    Presentation,
}

/// One entity slot: the generation currently (or most recently) occupying it, and whether it is
/// live right now.
///
/// `rank` is the spawn rank of the current occupancy (see [`EntityAllocator::alloc`]). It is not part
/// of equality: ranks are never hashed and a snapshot reproduces them separately, so the "can the
/// live set alone rebuild this allocator" test must ignore them.
#[derive(Clone, Copy, Debug)]
struct Slot {
    generation: u32,
    alive: bool,
    rank: u64,
}

impl PartialEq for Slot {
    fn eq(&self, other: &Self) -> bool {
        self.generation == other.generation && self.alive == other.alive
    }
}
impl Eq for Slot {}

/// A serializable snapshot of the **entire** generational allocator — every slot's generation plus
/// the free-list stack order — sufficient to reconstruct it exactly, including the generations of
/// dead slots and the free-list order that the live-entity set alone cannot convey.
///
/// This is the "full allocator fidelity" that a live-set-only restore ([`World::restore_entities`])
/// lacks: it makes id allocation deterministic across a snapshot→restore, so a recycled slot comes
/// back at its true generation (never a stale handle's earlier value) and post-restore spawns mint
/// exactly the ids an uninterrupted run would. A slot is alive **iff** its index is absent from
/// `free`, so aliveness is carried without a redundant per-slot flag.
///
/// Deliberately **excluded from the world hash** (which visits only live slots): the free-list shape
/// is not observable world state, per the determinism contract.
///
/// [`World::restore_entities`]: crate::World::restore_entities
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct AllocatorState {
    /// Generation of each slot, indexed by slot; length is the total slot count (including trailing
    /// free slots that the live set would otherwise drop).
    generations: Vec<u32>,
    /// Free slot indices in LIFO order (the last is handed out first). A slot is dead iff it appears
    /// here.
    free: Vec<u32>,
}

/// Allocates and recycles [`EntityId`]s with generational safety and a deterministic free-list.
#[derive(Debug, Default)]
pub(crate) struct EntityAllocator {
    /// Indexed by slot index; grows and never shrinks.
    slots: Vec<Slot>,
    /// Indices of currently-free slots, used as a LIFO stack (`push`/`pop` at the end).
    free: Vec<u32>,
    /// The rank the next `alloc` stamps; strictly above every live slot's rank. Not in equality.
    next_rank: u64,
}

impl PartialEq for EntityAllocator {
    fn eq(&self, other: &Self) -> bool {
        self.slots == other.slots && self.free == other.free
    }
}
impl Eq for EntityAllocator {}

impl EntityAllocator {
    /// Reconstructs an allocator in which exactly `live` are alive, each at its own generation.
    ///
    /// Every slot index below the highest live index that is not itself in `live` becomes a free,
    /// recyclable slot at generation 0. This is the allocator half of snapshot restore: a world
    /// rebuilt from it enumerates (`iter_alive`) and hashes identically to the one that was
    /// snapshotted.
    ///
    /// # Errors
    /// `L0025` if two entries of `live` share a slot index, with the JSON Pointer `/<position>` of
    /// the second — the snapshot is a set of live entities, and merging duplicates would silently
    /// keep one generation. `L0025` too, at `/<position>`, if an entry's slot index is at or past
    /// [`MAX_ENTITY_SLOTS`]; that check runs before anything is allocated.
    pub(crate) fn from_live(live: &[EntityId]) -> Result<Self, Diagnostic> {
        if let Some(position) = live.iter().position(|e| e.index >= MAX_ENTITY_SLOTS) {
            let entity = live[position];
            return Err(Diagnostic::new(
                code::ENTITY_ALLOCATOR_STATE_INVALID,
                format!(
                    "live entity {entity} has slot index {}, past the engine limit of {MAX_ENTITY_SLOTS} slots",
                    entity.index
                ),
            )
            .with_path(format!("/{position}"))
            .with_help("re-take the snapshot rather than editing it"));
        }
        let len = live.iter().map(|e| e.index as usize + 1).max().unwrap_or(0);
        let mut slots = vec![
            Slot {
                generation: 0,
                alive: false,
                rank: 0,
            };
            len
        ];
        for (position, entity) in live.iter().enumerate() {
            let slot = &mut slots[entity.index as usize];
            if slot.alive {
                let first = live
                    .iter()
                    .find(|other| other.index == entity.index)
                    .expect("the slot was marked alive by an earlier entry");
                return Err(Diagnostic::new(
                    code::ENTITY_ALLOCATOR_STATE_INVALID,
                    format!(
                        "live entities {first} and {entity} share slot index {}",
                        entity.index
                    ),
                )
                .with_path(format!("/{position}"))
                .with_help(
                    "a slot holds one entity; re-take the snapshot rather than editing it",
                ));
            }
            *slot = Slot {
                generation: entity.generation,
                alive: true,
                rank: u64::from(entity.index),
            };
        }
        // Push free indices high-to-low so the LIFO `pop` hands out the lowest free index first —
        // the same "prefer low slots" feel a fresh allocator has.
        let free: Vec<u32> = (0..len as u32)
            .rev()
            .filter(|&i| !slots[i as usize].alive)
            .collect();
        let next_rank = len as u64;
        Ok(EntityAllocator {
            slots,
            free,
            next_rank,
        })
    }

    /// Serializes the full allocator — every slot's generation and the free-list order — into an
    /// [`AllocatorState`] that reconstructs it exactly via [`from_state`](Self::from_state).
    fn to_state(&self) -> AllocatorState {
        AllocatorState {
            generations: self.slots.iter().map(|slot| slot.generation).collect(),
            free: self.free.clone(),
        }
    }

    /// Reconstructs the allocator from an exact [`AllocatorState`]. A slot is alive iff its index is
    /// absent from the state's free list; generations come straight from the state.
    ///
    /// # Errors
    /// `L0025` if a free-list index is past the generation table or appears twice, with the JSON
    /// Pointer `/free/<position>` — either would alias two entities or panic on the next spawn.
    /// `L0025` at `/generations` if the table holds more than [`MAX_ENTITY_SLOTS`] slots.
    pub(crate) fn from_state(state: &AllocatorState) -> Result<Self, Diagnostic> {
        if state.generations.len() > MAX_ENTITY_SLOTS as usize {
            return Err(Diagnostic::new(
                code::ENTITY_ALLOCATOR_STATE_INVALID,
                format!(
                    "the generation table holds {} slots, past the engine limit of {MAX_ENTITY_SLOTS}",
                    state.generations.len()
                ),
            )
            .with_path("/generations")
            .with_help("re-take the snapshot rather than editing the allocator state"));
        }
        // O(n), deterministic, no hashing: mark the dead slots, everything else is alive.
        let slot_count = state.generations.len();
        let mut dead = vec![false; slot_count];
        for (position, &index) in state.free.iter().enumerate() {
            let invalid = |message: String| {
                Diagnostic::new(code::ENTITY_ALLOCATOR_STATE_INVALID, message)
                    .with_path(format!("/free/{position}"))
                    .with_help("re-take the snapshot rather than editing the allocator state")
            };
            match dead.get_mut(index as usize) {
                None => {
                    return Err(invalid(format!(
                        "free slot index {index} is past the end of the generation table ({slot_count} slots)"
                    )));
                }
                Some(flag) if *flag => {
                    return Err(invalid(format!(
                        "free slot index {index} appears twice in the free list"
                    )));
                }
                Some(flag) => *flag = true,
            }
        }
        let slots = state
            .generations
            .iter()
            .zip(dead)
            .enumerate()
            .map(|(index, (&generation, is_dead))| Slot {
                generation,
                alive: !is_dead,
                rank: index as u64,
            })
            .collect();
        Ok(EntityAllocator {
            slots,
            free: state.free.clone(),
            next_rank: slot_count as u64,
        })
    }

    /// The full allocator state **iff** the live-entity set alone cannot reproduce it — i.e. when
    /// [`from_live`](Self::from_live) over the current live entities would rebuild a *different*
    /// allocator (recycled slots carry a non-zero generation, trailing free slots exist, or the
    /// free-list order differs). Returns `None` for a clean allocator, so a snapshot of an
    /// un-recycled world carries no extra state and stays byte-identical.
    pub(crate) fn state_if_irreducible(&self) -> Option<AllocatorState> {
        let live: Vec<EntityId> = self.iter_alive().collect();
        let rebuilt = EntityAllocator::from_live(&live).expect("iter_alive yields unique indices");
        if *self == rebuilt {
            None
        } else {
            Some(self.to_state())
        }
    }

    /// Allocates an entity, reusing the most recently freed slot (LIFO) if any, otherwise appending
    /// a fresh slot at generation 0. Each occupancy is stamped with the next spawn rank, so relative
    /// rank is spawn-call order whatever slots the free list hands out.
    ///
    /// # Panics
    /// When every one of [`MAX_ENTITY_SLOTS`] slots is live.
    pub(crate) fn alloc(&mut self) -> EntityId {
        let rank = self.next_rank;
        self.next_rank += 1;
        if let Some(index) = self.free.pop() {
            let slot = &mut self.slots[index as usize];
            slot.alive = true;
            slot.rank = rank;
            EntityId::new(index, slot.generation)
        } else {
            let index = u32::try_from(self.slots.len())
                .ok()
                .filter(|&index| index < MAX_ENTITY_SLOTS)
                .unwrap_or_else(|| {
                    panic!(
                        "entity slot limit MAX_ENTITY_SLOTS ({MAX_ENTITY_SLOTS} concurrent entities) reached"
                    )
                });
            self.slots.push(Slot {
                generation: 0,
                alive: true,
                rank,
            });
            EntityId::new(index, 0)
        }
    }

    /// Frees a slot, bumping its generation so the handle can never match again. The caller must
    /// have already verified the entity is alive (via [`is_alive`](Self::is_alive)).
    pub(crate) fn free(&mut self, entity: EntityId) {
        let slot = &mut self.slots[entity.index as usize];
        slot.alive = false;
        slot.generation = slot.generation.wrapping_add(1);
        self.free.push(entity.index);
    }

    /// Whether `entity` names the entity currently occupying its slot.
    pub(crate) fn is_alive(&self, entity: EntityId) -> bool {
        self.slots
            .get(entity.index as usize)
            .is_some_and(|slot| slot.alive && slot.generation == entity.generation)
    }

    /// The number of currently-live entities.
    pub(crate) fn alive_count(&self) -> usize {
        self.slots.iter().filter(|slot| slot.alive).count()
    }

    /// Iterates the live entities in **ascending slot-index order**.
    ///
    /// This is the canonical enumeration order for whole-world serialization (snapshots, the world
    /// hash): it depends only on the current world state, never on spawn history beyond which slots
    /// are live and at which generation.
    pub(crate) fn iter_alive(&self) -> impl Iterator<Item = EntityId> + '_ {
        self.slots
            .iter()
            .enumerate()
            .filter(|(_, slot)| slot.alive)
            .map(|(index, slot)| {
                let index = u32::try_from(index).expect("slot indices always fit in u32");
                EntityId::new(index, slot.generation)
            })
    }

    /// The live [`EntityId`] occupying slot `index` — its index paired with the slot's current
    /// generation.
    ///
    /// Used by [`World::query_ids`](crate::World::query_ids) to attach an id to each joined row: the
    /// join only ever yields the index of an entity that carries components, and components live only
    /// on live entities, so the slot is guaranteed alive and this reads its true generation. `index`
    /// must be in range (a live slot); an out-of-range index is a caller bug and panics.
    pub(crate) fn id_at(&self, index: u32) -> EntityId {
        let slot = self.slots[index as usize];
        debug_assert!(slot.alive, "id_at on a non-live slot {index}");
        EntityId::new(index, slot.generation)
    }

    /// The live [`EntityId`] in slot `index`; `None` for a free or out-of-range slot.
    pub(crate) fn live_id_at(&self, index: u32) -> Option<EntityId> {
        let slot = self.slots.get(index as usize)?;
        slot.alive.then(|| EntityId::new(index, slot.generation))
    }

    /// The spawn rank of the live entity in slot `index`; `None` for a free or out-of-range slot.
    pub(crate) fn rank_at(&self, index: u32) -> Option<u64> {
        let slot = self.slots.get(index as usize)?;
        slot.alive.then_some(slot.rank)
    }

    /// The live slot indices in ascending spawn-rank order, or `None` when that is ascending slot
    /// order (the common case, which a snapshot omits).
    pub(crate) fn spawn_order(&self) -> Option<Vec<u32>> {
        let mut order: Vec<u32> = self.iter_alive().map(|id| id.index).collect();
        if order
            .windows(2)
            .all(|w| self.slots[w[0] as usize].rank < self.slots[w[1] as usize].rank)
        {
            return None;
        }
        order.sort_by_key(|&index| self.slots[index as usize].rank);
        Some(order)
    }

    /// Re-ranks the live slots to `order` (rebased to `0..n`; only relative order is observable).
    ///
    /// # Errors
    /// `L0025` at `/<position>` if `order` names a slot that is not live or repeats one, or if it
    /// omits a live slot (position = its length); the allocator is left untouched.
    pub(crate) fn restore_order(&mut self, order: &[u32]) -> Result<(), Diagnostic> {
        let invalid = |position: usize, message: String| {
            Diagnostic::new(code::ENTITY_ALLOCATOR_STATE_INVALID, message)
                .with_path(format!("/{position}"))
                .with_help("re-take the snapshot rather than editing the spawn order")
        };
        let mut seen = vec![false; self.slots.len()];
        for (position, &index) in order.iter().enumerate() {
            match seen.get_mut(index as usize) {
                Some(flag) if self.slots[index as usize].alive && !*flag => *flag = true,
                Some(flag) if *flag => {
                    return Err(invalid(
                        position,
                        format!("spawn order repeats slot {index}"),
                    ));
                }
                _ => {
                    return Err(invalid(
                        position,
                        format!("spawn order names slot {index}, which is not live"),
                    ));
                }
            }
        }
        if order.len() != self.alive_count() {
            return Err(invalid(
                order.len(),
                format!(
                    "spawn order lists {} slots but {} are live",
                    order.len(),
                    self.alive_count()
                ),
            ));
        }
        for (rank, &index) in order.iter().enumerate() {
            self.slots[index as usize].rank = rank as u64;
        }
        self.next_rank = order.len() as u64;
        Ok(())
    }

    /// A human-readable reason a handle is not alive, for diagnostics. Only meaningful when
    /// [`is_alive`](Self::is_alive) is `false`.
    pub(crate) fn not_alive_reason(&self, entity: EntityId) -> &'static str {
        match self.slots.get(entity.index as usize) {
            None => "no entity has ever occupied that slot",
            Some(slot) if slot.generation != entity.generation => {
                "the slot was recycled: this handle refers to an entity that has been despawned"
            }
            Some(_) => "the entity in that slot has been despawned",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn from_live_reconstructs_exactly_the_live_set() {
        let live = [
            EntityId::new(0, 0),
            EntityId::new(2, 3),
            EntityId::new(3, 1),
        ];
        let allocator = EntityAllocator::from_live(&live).unwrap();

        // Enumeration matches the live set, in ascending index order, with exact generations.
        let enumerated: Vec<_> = allocator.iter_alive().collect();
        assert_eq!(enumerated, live);
        assert_eq!(allocator.alive_count(), 3);
        for &id in &live {
            assert!(allocator.is_alive(id));
        }

        // A stale handle to a live slot's earlier generation is (still) not alive.
        assert!(!allocator.is_alive(EntityId::new(2, 0)));
    }

    #[test]
    fn from_live_frees_gap_slots_lowest_first() {
        // Live at 0 and 3; slots 1 and 2 are gaps → free, recycled lowest-first.
        let mut allocator =
            EntityAllocator::from_live(&[EntityId::new(0, 0), EntityId::new(3, 0)]).unwrap();
        assert_eq!(allocator.alloc(), EntityId::new(1, 0));
        assert_eq!(allocator.alloc(), EntityId::new(2, 0));
        // Both gaps consumed; the next spawn appends a fresh slot past the max index.
        assert_eq!(allocator.alloc(), EntityId::new(4, 0));
    }

    #[test]
    fn from_live_empty_is_a_fresh_allocator() {
        let mut allocator = EntityAllocator::from_live(&[]).unwrap();
        assert_eq!(allocator.alive_count(), 0);
        assert_eq!(allocator.alloc(), EntityId::new(0, 0));
    }

    /// Builds an allocator with a recycled-but-still-free gap slot: spawn seven, despawn slot 5. Slot
    /// 5 is now free at generation 1 (its original handle was gen 0). This is the shape that exposed
    /// the restore-aliasing bug.
    fn recycled_gap() -> (EntityAllocator, EntityId) {
        let mut allocator = EntityAllocator::default();
        let ids: Vec<EntityId> = (0..7).map(|_| allocator.alloc()).collect();
        let stale = ids[5]; // EntityId(5, 0)
        allocator.free(stale); // slot 5 → generation 1, free
        (allocator, stale)
    }

    #[test]
    fn state_round_trips_a_recycled_allocator_exactly() {
        let (allocator, _) = recycled_gap();
        let restored = EntityAllocator::from_state(&allocator.to_state()).unwrap();
        assert_eq!(
            restored, allocator,
            "to_state/from_state is an exact round-trip"
        );
    }

    #[test]
    fn a_clean_allocator_is_reducible_but_a_recycled_one_is_not() {
        // No despawns → the live set alone reconstructs it → no extra state needed.
        let mut clean = EntityAllocator::default();
        for _ in 0..4 {
            clean.alloc();
        }
        assert!(clean.state_if_irreducible().is_none());

        // A recycled gap slot (generation 1, free) cannot be reproduced by `from_live` (which would
        // give generation 0), so the state must be carried.
        let (recycled, _) = recycled_gap();
        assert!(recycled.state_if_irreducible().is_some());
    }

    #[test]
    fn restore_via_state_does_not_reissue_a_stale_handle() {
        let (allocator, stale) = recycled_gap();
        let state = allocator
            .state_if_irreducible()
            .expect("a recycled allocator is irreducible");

        // The pre-fix path: `from_live` rebuilds slot 5 at generation 0 and the next spawn re-mints
        // the exact stale handle — the aliasing bug.
        let mut buggy =
            EntityAllocator::from_live(&allocator.iter_alive().collect::<Vec<_>>()).unwrap();
        assert_eq!(
            buggy.alloc(),
            stale,
            "the bug: the live-set-only restore re-issues the stale handle"
        );

        // The fix: restoring from full allocator state hands out generation 1, and the stale handle
        // stays a reported error rather than a silent hit on the fresh entity.
        let mut fixed = EntityAllocator::from_state(&state).unwrap();
        let fresh = fixed.alloc();
        assert_eq!(fresh, EntityId::new(5, 1));
        assert_ne!(fresh, stale, "the fresh id must not equal the stale handle");
        assert!(!fixed.is_alive(stale), "the stale handle must not be alive");
        assert!(
            fixed.not_alive_reason(stale).contains("recycled"),
            "the stale handle reports the recycled-slot diagnostic"
        );
    }

    #[test]
    fn restore_via_state_matches_an_uninterrupted_spawn_sequence() {
        // An uninterrupted run: recycle a gap, then spawn a handful more.
        let (mut uninterrupted, _) = recycled_gap();
        let direct: Vec<EntityId> = (0..5).map(|_| uninterrupted.alloc()).collect();

        // A restored run: snapshot the allocator right after the recycle, restore, then spawn the
        // same count. The id sequence must be identical (id-allocation determinism across restore).
        let (snapshotted, _) = recycled_gap();
        let mut restored =
            EntityAllocator::from_state(&snapshotted.state_if_irreducible().expect("irreducible"))
                .unwrap();
        let after_restore: Vec<EntityId> = (0..5).map(|_| restored.alloc()).collect();

        assert_eq!(
            after_restore, direct,
            "post-restore spawns match the uninterrupted sequence exactly"
        );
    }

    fn state(generations: &[u32], free: &[u32]) -> AllocatorState {
        serde_json::from_value(serde_json::json!({"generations": generations, "free": free}))
            .expect("well-formed allocator state")
    }

    #[test]
    fn from_state_rejects_a_duplicate_free_entry() {
        let error = EntityAllocator::from_state(&state(&[0, 0, 0], &[1, 2, 1])).unwrap_err();
        assert_eq!(error.code, code::ENTITY_ALLOCATOR_STATE_INVALID);
        assert_eq!(error.path.as_deref(), Some("/free/2"));
        assert!(error.message.contains("twice"), "{}", error.message);
    }

    #[test]
    fn from_state_rejects_an_out_of_range_free_entry() {
        let error = EntityAllocator::from_state(&state(&[0, 0], &[0, 999])).unwrap_err();
        assert_eq!(error.code, code::ENTITY_ALLOCATOR_STATE_INVALID);
        assert_eq!(error.path.as_deref(), Some("/free/1"));
        assert!(error.message.contains("999"), "{}", error.message);
    }

    #[test]
    fn from_state_rejects_a_table_past_the_slot_limit() {
        let state = AllocatorState {
            generations: vec![0; MAX_ENTITY_SLOTS as usize + 1],
            free: vec![],
        };
        let error = EntityAllocator::from_state(&state).unwrap_err();
        assert_eq!(error.code, code::ENTITY_ALLOCATOR_STATE_INVALID);
        assert_eq!(error.path.as_deref(), Some("/generations"));
    }

    #[test]
    fn from_live_rejects_an_index_past_the_slot_limit() {
        let error =
            EntityAllocator::from_live(&[EntityId::new(0, 0), EntityId::new(MAX_ENTITY_SLOTS, 0)])
                .unwrap_err();
        assert_eq!(error.code, code::ENTITY_ALLOCATOR_STATE_INVALID);
        assert_eq!(error.path.as_deref(), Some("/1"));
    }

    #[test]
    fn from_live_rejects_a_duplicate_live_index() {
        let error =
            EntityAllocator::from_live(&[EntityId::new(1, 0), EntityId::new(1, 5)]).unwrap_err();
        assert_eq!(error.code, code::ENTITY_ALLOCATOR_STATE_INVALID);
        assert_eq!(error.path.as_deref(), Some("/1"));
    }

    #[test]
    fn a_valid_state_still_round_trips() {
        let (allocator, _) = recycled_gap();
        let rebuilt = EntityAllocator::from_state(&allocator.to_state()).unwrap();
        assert_eq!(rebuilt, allocator);
    }
}
