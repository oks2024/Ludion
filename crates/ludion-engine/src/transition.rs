//! In-run scene transitions (Phase 17 / E7): the [`SceneSwitch`] resource a game system sets to
//! request a district change, and `perform` (crate-private), the frame-boundary rebuild the engine
//! runs to carry it out.
//!
//! A scene switch is `scene.load`'s twin, not its clone: it rebuilds the world from a target scene
//! like a load, but where a load *starts a session* (frame 0, reseed from the scene's `seed:`, fresh
//! logs) a switch *continues* one — the frame counter keeps ticking, the RNG stream keeps flowing, the
//! session logs keep accumulating, the profile carries in-memory, and an explicit list of entities
//! (typically the player) travels along, serialized out of the old world and respawned at a `Name`d
//! anchor in the new one. The target scene's `seed:` is deliberately **ignored**: the run has one
//! uninterrupted seeded RNG stream (the determinism pillar), so reseeding mid-run would fork it.
//!
//! The switch is requested through a resource (the [`AudioQueue`](crate::AudioQueue) mold) and
//! performed by the host at the frame boundary in [`drain_scene_switch`](crate::Engine) — no wire
//! method, no new component. It is **transactional**: the target district instantiates into a local
//! world, and the live world is swapped only on full success. Any failure (target won't load →
//! `L3018`, anchor missing → `L3019`, anchor without a `Transform` → `L3029`) rejects the switch,
//! leaves the live world untouched, and lets the sim run on in the current district — the failure is a
//! non-fatal warning stored on the engine ([`Engine::last_transition_error`](crate::Engine)) and
//! printed once on the `log` channel and stderr, never a panic.
//!
//! # Scene residue: what stays behind
//!
//! Carry answers "what travels with you"; **residue** answers "what the district keeps while you are
//! away". An entity marked [`Residue`] has its full component bag captured out of the
//! departing world and re-applied when the run returns to that scene in the same session — so a spawner
//! destroyed on the first visit is still destroyed on the second. It is the switch's own machinery
//! pointed the other way: the same registry-driven serialize/deserialize pair, keyed per scene.
//!
//! A few rules make it predictable:
//!
//! - **The scene's `name:` is the key.** It is the one identity that survives a recording — a replay
//!   loads its boot scene from inline text, where no file path exists — so keying on it is what makes
//!   a live run and its replay capture into the same slot. A scene that declares no `name:` therefore
//!   accumulates no residue, live and under replay alike.
//! - **A record's identity is the entity's `Name`, and `Name` is never in the bag.** A named record
//!   replaces its counterpart *in place* (`EntityId`s do not move); an unnamed one is anonymous and
//!   comes back as a fresh entity. A named record with no counterpart on return depends on where the
//!   entity came from: one a *system* spawned respawns (its state has no other home), one the scene
//!   *authored* was renamed and is dropped and reported (`L3031`) — spawning it would duplicate the
//!   entity it is.
//! - **A record is migrated, not trusted.** A snapshot carries residue across a rebuild, so a bag can
//!   outlive the shape of its components; one that no longer reads is dropped whole and reported
//!   (`L3030`), the way a snapshot's world entity is (`L3015`/`L3016`).
//! - **Upsert only.** Capture reads the world that exists, so an entity despawned before the switch
//!   leaves nothing behind and the target scene's authored version returns. State that must survive a
//!   visit keeps its entity alive and says "spent" in a component.
//!
//! A game that marks nothing pays nothing: the residue map stays empty and every path through
//! `perform` is the one that existed before residue did.
//!
//! Residue is bounded on both axes — [`RESIDUE_MAX_SCENES`] scenes per session and
//! [`RESIDUE_MAX_RECORDS_PER_SCENE`] records per scene — and overflow is loud: the least recently
//! visited scene is evicted, or the tail of an oversized capture is dropped, and either is reported as
//! `L3028`. The choice is sim state (the last-visit frame is a frame index), so it is identical on
//! every machine and in every replay.

use std::any::TypeId;

use ludion_core::codes::code;
use ludion_core::collections::{StableMap, StableSet};
use ludion_core::diagnostics::Diagnostic;
use ludion_core::ids::EntityId;
use ludion_core::time::{FrameIndex, TickRate};
use ludion_ecs::{ChildMap, Parent, Registry, is_presentation};
use ludion_protocol::replay::SceneRef;
use ludion_sim::{DriverRestore, SimDriver};
use serde_json::Value;

use crate::components::{Name, Residue, Transform};
use crate::engine::{Engine, Instantiated, PreparedWorld, Rebuild, instantiate_named_world};

/// An ECS resource: a pending in-run district change a game system requests.
///
/// The engine installs one (empty) into every world it builds, so a game system can always
/// `world.resource_mut::<SceneSwitch>()` it and call [`request`](SceneSwitch::request). The engine
/// drains it once per tick at the frame boundary — after the audio and profile drains — and, if a
/// switch is pending, performs it (the crate-private `perform`). Like [`AudioQueue`](crate::AudioQueue) it is a
/// plain resource, excluded from the `WorldHash` and from snapshots, so it registers no component and
/// trips no drift gate.
#[derive(Default, Debug)]
pub struct SceneSwitch {
    pending: Option<SwitchTo>,
}

/// The target of a requested district change: the scene to load, the anchor to drop the carried
/// entities at, and the entities that travel.
#[derive(Clone, Debug)]
pub struct SwitchTo {
    /// The target district's scene file, resolved relative to the project root (the directory of the
    /// scene the engine was started with).
    pub scene_path: String,
    /// The `name:` of the entity in the target district whose `Transform.translation` is the drop
    /// point for the carried entities.
    pub spawn_anchor: String,
    /// The entities to carry across the switch (typically `[player]`), serialized out of the current
    /// world and respawned at the anchor. Carried in this order.
    pub carry: Vec<EntityId>,
}

impl SceneSwitch {
    /// Requests a district change, to be performed at the next frame boundary.
    ///
    /// Last request wins: a second `request` in the same tick simply replaces the first (a game
    /// triggers at most one switch per frame; there is no queue).
    pub fn request(
        &mut self,
        scene_path: impl Into<String>,
        spawn_anchor: impl Into<String>,
        carry: Vec<EntityId>,
    ) {
        self.pending = Some(SwitchTo {
            scene_path: scene_path.into(),
            spawn_anchor: spawn_anchor.into(),
            carry,
        });
    }

    /// Whether a switch is currently pending.
    pub fn is_pending(&self) -> bool {
        self.pending.is_some()
    }

    /// Removes and returns the pending switch, leaving none (the engine's drain path).
    pub(crate) fn take(&mut self) -> Option<SwitchTo> {
        self.pending.take()
    }
}

/// One [`Residue`]-marked entity's state, captured out of a scene the run left.
///
/// `name` is the entity's [`Name`] (its identity on return) or `None` for an anonymous record — one
/// that is never matched and always respawned. `bag` is every registered component the entity carried
/// **except** `Name`, serialized in registration order: identity is the key, never the payload, so a
/// record cannot rename the entity it lands on.
#[derive(Clone, Debug)]
pub(crate) struct ResidueRecord {
    /// The entity's `Name`, or `None` for an anonymous record.
    pub(crate) name: Option<String>,
    /// Whether the scene document the departing world was built from authored an entity of this
    /// `Name` — the one thing that says what a record with no counterpart on return *means*. An
    /// authored record whose counterpart vanished was renamed (drop it, `L3031`); a code-spawned one
    /// never had a counterpart and must respawn, which is how a game keeps state it created itself.
    /// Always `false` for an anonymous record, and for anything captured out of a world with no
    /// document behind it (a restored snapshot), where respawning is the safe reading.
    pub(crate) authored: bool,
    /// The captured components as `(registered name, value)`, in registration order, minus `Name`.
    pub(crate) bag: Vec<(String, Value)>,
}

/// Everything one scene kept behind: the records captured the last time the run left it.
///
/// Held per scene key in the session's residue map. Capture replaces a scene's records wholesale, so
/// the map always describes the departing world as it actually was — there are no tombstones to
/// reconcile, and an entity that was despawned during the visit is simply absent from the new set.
#[derive(Clone, Debug)]
pub(crate) struct SceneResidue {
    /// The captured records, in departing-world entity order.
    pub(crate) records: Vec<ResidueRecord>,
    /// The frame the run last left this scene on — the switch frame its records were captured at.
    /// The eviction key: sim state, so the scene the session drops at [`RESIDUE_MAX_SCENES`] is the
    /// same one on every machine and under replay.
    pub(crate) last_visit: FrameIndex,
}

impl Default for SceneResidue {
    /// A scene that has kept nothing, never left. Hand-written because [`FrameIndex`] has no
    /// `Default` of its own; frame zero is the value an older snapshot's entries adopt.
    fn default() -> Self {
        SceneResidue {
            records: Vec::new(),
            last_visit: FrameIndex::ZERO,
        }
    }
}

/// How many scenes one session's residue map holds — twice the sector count the city is designed
/// around, so a run that stays inside its design never reaches it. Past it, the least recently visited
/// scene is evicted and `L3028` is reported.
pub const RESIDUE_MAX_SCENES: usize = 32;

/// How many [`Residue`] records one scene keeps. Past it the capture keeps the first
/// `RESIDUE_MAX_RECORDS_PER_SCENE` in capture order, drops the rest, and reports `L3028` — the bound
/// that stops a long visit dropping unnamed records forever.
pub const RESIDUE_MAX_RECORDS_PER_SCENE: usize = 4096;

/// Performs a requested district switch against `engine`, given the target district's canonical scene
/// `target_text` (already resolved from disk by the drain — [`perform`] itself does no disk IO, so a
/// replay can feed it off-record text unchanged).
///
/// The switch continues the session: it forks `scene.load`'s check → instantiate → `Name`-projection
/// front half but builds the driver via [`SimDriver::restore`], carrying the live `(rng, frame,
/// run_state, speed)`, and leaves the session logs intact. It is transactional — the target
/// instantiates into a local world and the live world is swapped only after every check passes:
///
/// 1. copy the live RNG and read the frame / run-state / speed / profile **before** any mutation;
/// 2. capture each carried entity's component bag, with its whole subtree, from the live world (a carry
///    id that is not live is skipped, carrying nothing, rather than aborting the switch; a carried child
///    whose parent stays behind is un-parented keeping its world pose, `L3043`; an entity reference
///    to a carried entity is remapped to its respawned id, one to an entity left behind arrives empty
///    or drops its component, `L3043`);
/// 3. capture the departing scene's **residue** — every `Residue`-marked entity's bag (`L3026` on a
///    duplicate `Name`, `L3027` on an entity that is both carried and marked), held locally so a
///    rejected switch leaves the session's residue map untouched;
/// 4. instantiate the target text into a **local** world (`L3018` on a load/validation failure);
/// 5. re-apply the target scene's residue over that local world — named records in ascending `Name`
///    order, then anonymous records in capture order (`L3030` a record this build can no longer read,
///    `L3031` a named record with no counterpart left, `L3026` a name two entities share);
/// 6. locate the anchor by `Name` in that local world (`L3019` if none matches, `L3029` if the one
///    that does carries no `Transform`);
/// 7. spawn each carried bag into the local world at the anchor's translation;
/// 8. prepare the target's data-docs on that local world — the content-root walk, the game's
///    validators and the session's doc-reload log (a doc gone bad on disk fails here, as `L3018`-class
///    rejection);
/// 9. commit the departing scene's residue into the session, evicting to stay inside the two bounds
///    (`L3028`), then build the continued driver and swap it in with [`Engine::commit_world`] — the
///    point of no return, which cannot fail — reinstalling the engine-owned resources and carrying the
///    in-memory profile.
///
/// Steps 5–7 run in that order so the carried entities are spawned **last** and hold the highest
/// `EntityId`s: a game that resolves a carried-versus-authored duplicate by keeping the highest id
/// keeps resolving it in the carried entity's favor. **That is a contract, not an accident** — steps
/// 4 and 5 only spawn, never despawn, so every carried id is strictly greater than every instantiated
/// and residue-spawned one; `transition_residue.rs`'s
/// `carried_entities_hold_the_highest_ids_after_a_switch` pins it, and anything that starts despawning
/// inside instantiation breaks every game that reads the highest id as the live one.
///
/// Returns `Ok(None)` on a fully-clean switch, `Ok(Some(warning))` when the switch succeeded but
/// something was reported along the way (`L3020` a dead carry id, `L3043` an orphaned carried child or a reference left behind, `L3027` a carried-and-marked entity,
/// `L3026` a duplicated residue name, `L3030` an unreadable residue record, `L3031` a residue record
/// that matched nothing, `L3028` a bound reached — the first is returned with the rest attached as
/// `related`), and
/// `Err` when the switch was rejected (the failure diagnostic wins over any warning).
///
/// # Errors
/// `L3018` if the target district cannot be loaded or validated; `L3019` if the anchor is missing;
/// `L3029` if the anchor carries no `Transform`; the data-doc walk's diagnostics (`L1101`,
/// `L1119`/`L1120`) or a registered validator's error, collapsed to one, if the target's docs no longer
/// load. On any of them, the local world is discarded and the session's residue map and the live driver
/// are untouched — the sim runs on in the current district.
pub(crate) fn perform(
    engine: &mut Engine,
    switch: SwitchTo,
    target_text: &str,
) -> Result<Option<Diagnostic>, Diagnostic> {
    // (a) Snapshot the run's continuity out of the live world before anything changes: the RNG is
    //     carried verbatim (never reseeded), the frame keeps counting, and the profile is carried
    //     in-memory (the target scene's `seed:` is ignored — one run, one uninterrupted RNG stream).
    let rng = *engine.rng()?;
    let frame = engine.frame();
    let run_state = engine.run_state();
    let speed = engine.speed();
    let profile_text = engine.emit_profile_text();

    // (b) Capture each carried entity's component bag from the live world, in registration order (the
    //     `snapshot::capture` mold). A carry id not alive at drain time carries nothing and is skipped
    //     (recorded for the `L3020` warning) — a game bug, but never a reason to abort the switch and
    //     strand the player mid-run.
    //     Each carried entity travels with its whole subtree: the expanded list holds every carried
    //     id then its descendants (ascending slot), once, never a presentation entity.
    let registry = engine.registry();
    let child_map = ChildMap::build(engine.world());
    let mut expanded: Vec<EntityId> = Vec::new();
    let mut skipped: Vec<EntityId> = Vec::new();
    for &id in &switch.carry {
        if !engine.world().is_alive(id) {
            skipped.push(id);
            continue;
        }
        for member in std::iter::once(id).chain(child_map.descendants(id)) {
            if !is_presentation(member) && !expanded.contains(&member) {
                expanded.push(member);
            }
        }
    }
    let mut bags: Vec<Vec<(String, Value)>> = Vec::new();
    // Per bag: the components holding a reference to a carried entity, which can be written only once
    // every respawned id is known, with each such field's pointer and the index in `expanded` it names.
    let mut deferred_bags: Vec<Vec<DeferredComponent>> = Vec::new();
    // Per bag: the index in `expanded` of its `Parent`, when that parent travels too.
    let mut carried_parents: Vec<Option<usize>> = Vec::new();
    let mut orphaned: Vec<EntityId> = Vec::new();
    let mut left_behind: Vec<String> = Vec::new();
    for &id in &expanded {
        let mut bag = Vec::new();
        let mut deferred = Vec::new();
        for info in registry.iter() {
            if info.name() == "Parent" {
                continue;
            }
            if let Some(mut value) = info.serialize(engine.world(), id)? {
                let Some(pending) =
                    remap_carried(info, &mut value, id, &expanded, &mut left_behind)
                else {
                    continue;
                };
                if pending.is_empty() {
                    bag.push((info.name().to_string(), value));
                } else {
                    deferred.push(DeferredComponent {
                        name: info.name().to_string(),
                        value,
                        pending,
                    });
                }
            }
        }
        deferred_bags.push(deferred);
        let parent = engine
            .world()
            .get::<Parent>(id)
            .ok()
            .flatten()
            .map(|Parent(parent)| *parent);
        let carried_parent = parent.and_then(|parent| expanded.iter().position(|e| *e == parent));
        if carried_parent.is_none()
            && parent.is_some_and(|parent| engine.world().is_alive(parent))
            && let Some(pose) = crate::hierarchy::world_pose_transform(engine.world(), id)
        {
            // Un-parented keeping its world pose: the local value it carried is meaningless alone.
            let value = serde_json::to_value(pose)
                .map_err(|e| Diagnostic::new(code::COMPONENT_SERIALIZE_FAILED, e.to_string()))?;
            if let Some(slot) = bag.iter_mut().find(|(name, _)| name == "Transform") {
                slot.1 = value;
            }
            orphaned.push(id);
        }
        bags.push(bag);
        carried_parents.push(carried_parent);
    }
    // Every non-fatal report this switch produces, in the order the steps that raise them run. The
    // first is returned and the rest ride it as `related`, because the engine surfaces one warning.
    let mut warnings: Vec<Diagnostic> = Vec::new();
    if !skipped.is_empty() {
        warnings.push(carry_dead(&skipped));
    }
    if !orphaned.is_empty() {
        warnings.push(carry_orphaned(&orphaned));
    }
    if !left_behind.is_empty() {
        warnings.push(carry_left_behind(&left_behind));
    }

    // (b′) Capture the departing scene's residue: every `Residue`-marked entity's bag, keyed by the
    //      scene's `name:` (the identity that survives a recording, so a replay captures into the same
    //      slot a live run does). Held locally until the switch is past every fallible check, so a
    //      rejected switch leaves the session's residue map exactly as it was.
    let departing_key = engine.scene_name().map(str::to_string);
    let mut departing_residue = None;
    if let Some(key) = departing_key {
        let (records, residue_warnings) =
            capture_residue(engine.world(), registry, &expanded, engine.authored_names())?;
        warnings.extend(residue_warnings);
        departing_residue = Some((key, records));
    }

    // (c) Instantiate the target district into a LOCAL world (the `scene.load` front half, shared via
    //     `instantiate_named_world`). A load/validation failure discards this local value — the live
    //     driver is never touched — so the sim runs on in the current district.
    let mut reads = engine.load_reads();
    let Instantiated {
        source,
        scene,
        mut world,
        placements: prefab_placements,
    } = instantiate_named_world(
        target_text,
        engine.registry(),
        engine.project_root(),
        &mut |path| reads.resolve(path),
    )
    .map_err(|diagnostics| target_unresolved(&switch.scene_path, diagnostics))?;

    // (c′′) Ready the target's `preload:` prefabs into its library, still before anything on
    //      the session changes: a bad entry fails the switch like a bad scene.
    let prefab_library = engine
        .prepare_prefab_library(&source, scene.physics.clone(), frame, reads)
        .map_err(|diagnostics| target_unresolved(&switch.scene_path, diagnostics))?;

    // (c′) Re-apply what the target scene kept from an earlier visit, over the freshly authored world.
    //      Switching into the scene you are leaving is the one case the session map cannot answer —
    //      its entry is one step stale — so the capture taken moments ago wins there.
    if let Some(arriving_key) = scene.name.as_deref() {
        let same_scene = departing_residue
            .as_ref()
            .is_some_and(|(key, _)| key == arriving_key);
        let records = if same_scene {
            departing_residue.as_ref().map(|(_, records)| &records[..])
        } else {
            engine
                .session_residue()
                .as_inner()
                .get(arriving_key)
                .map(|residue| &residue.records[..])
        };
        if let Some(records) = records {
            warnings.extend(apply_residue(&mut world, engine.registry(), records)?);
        }
    }

    // (d) Locate the anchor by `Name` in the fresh world; its translation is the drop point. An
    //     anchor that carries no `Transform` names no drop point, so it is refused like a missing one
    //     rather than dropping the run at the world origin.
    let anchor_translation = match anchor_translation(&world, &switch.spawn_anchor) {
        Anchor::At(translation) => translation,
        Anchor::NoTransform => {
            return Err(anchor_without_transform(
                &switch.scene_path,
                &switch.spawn_anchor,
            ));
        }
        Anchor::Missing => {
            return Err(anchor_missing(&switch.scene_path, &switch.spawn_anchor));
        }
    };

    // (e) Respawn each carried bag into the local world, overriding only `Transform.translation` to
    //     the anchor's (rotation/scale and every other component travel verbatim — decision 8).
    //     Only carried roots take the anchor; a carried child keeps its local `Transform`. Once every
    //     bag is spawned, its `Parent` and each component holding an entity reference to a carried
    //     entity are written against the respawned ids.
    let mut new_ids: Vec<EntityId> = Vec::with_capacity(bags.len());
    for (bag, carried_parent) in bags.iter().zip(&carried_parents) {
        let id = world.spawn();
        for (name, value) in bag {
            engine
                .registry()
                .deserialize_insert(name, value, &mut world, id)?;
        }
        if carried_parent.is_none()
            && let Ok(Some(transform)) = world.get::<Transform>(id)
        {
            let mut transform = *transform;
            transform.translation = anchor_translation;
            world
                .insert(id, transform)
                .expect("the entity was just spawned and is alive");
        }
        new_ids.push(id);
    }
    for ((&id, carried_parent), deferred) in new_ids.iter().zip(&carried_parents).zip(deferred_bags)
    {
        if let Some(parent) = carried_parent {
            world
                .insert(id, Parent(new_ids[*parent]))
                .expect("the entity was just spawned and is alive");
        }
        for DeferredComponent {
            name,
            mut value,
            pending,
        } in deferred
        {
            for (suffix, target) in pending {
                let new_id = serde_json::to_value(new_ids[target]).map_err(|e| {
                    Diagnostic::new(code::COMPONENT_SERIALIZE_FAILED, e.to_string())
                })?;
                if suffix.is_empty() {
                    value = new_id;
                } else if let Some(slot) = value.pointer_mut(&suffix) {
                    *slot = new_id;
                }
            }
            engine
                .registry()
                .deserialize_insert(&name, &value, &mut world, id)?;
        }
    }

    // (e′) Prepare the target's data-docs on the local world: the content-root walk, the game's
    //     validators and this session's doc-reload log. A doc gone bad on disk since boot fails
    //     here, with the residue map and the live driver both still untouched.
    let docs = engine
        .prepare_docs_for(&mut world, engine.session_doc_reloads())
        .map_err(crate::commands::collapse)?;

    // (f) Commit the departing scene's residue now that every fallible check has passed. An empty
    //     capture drops the scene's entry rather than storing an empty one, so a game that marks
    //     nothing keeps an empty map — and a scene whose last marked entity was despawned stops
    //     claiming to remember anything. Both bounds are held here, reported (`L3028`) and never
    //     fatal: the switch has already happened by every other measure.
    if let Some((key, mut records)) = departing_residue {
        if records.len() > RESIDUE_MAX_RECORDS_PER_SCENE {
            warnings.push(residue_records_capped(&key, records.len()));
            records.truncate(RESIDUE_MAX_RECORDS_PER_SCENE);
        }
        let residue = engine.session_residue_mut();
        if records.is_empty() {
            residue.remove(&key);
        } else {
            residue.insert(
                key.clone(),
                SceneResidue {
                    records,
                    last_visit: frame,
                },
            );
            // The scene just committed is never the eviction candidate: it is the most recently
            // visited by construction, and dropping it would make leaving a scene forget it.
            while residue.len() > RESIDUE_MAX_SCENES
                && let Some(evicted) = least_recently_visited(residue, &key)
            {
                let dropped = residue
                    .remove(&evicted)
                    .expect("the key was just read out of this map");
                warnings.push(residue_evicted(&evicted, dropped.last_visit));
            }
        }
    }

    // (g) Build the continued driver (the `SimDriver::restore` mold — carries the RNG, frame,
    //     run-state, and speed, unlike `scene.load`'s `SimDriver::new`, which would reset them).
    let schedule =
        engine.build_schedule(scene.physics.as_ref(), crate::path_follow::present(&world));
    let driver = SimDriver::restore(
        world,
        schedule,
        DriverRestore {
            rng,
            // One run, one seed: the target district's `seed:` is ignored exactly as its RNG is.
            world_seed: engine.seed(),
            frame,
            run_state,
            speed,
            tick_rate: TickRate::DEFAULT,
        },
    );

    // Swap the new district in and reinstall the engine-owned resources, carrying the in-memory
    // profile off `profile_text`. The session logs are left intact — a switch is mid-session, the
    // opposite of `scene.load`. This is the point of no return, and the commit cannot fail: every
    // fallible check is above it.
    let scene_ref = SceneRef::Inline {
        document: target_text.to_string(),
    };
    let seed = engine.seed();
    engine.commit_world(PreparedWorld {
        driver,
        seed,
        scene: Some(scene_ref),
        scene_name: scene.name.clone(),
        scene_physics: scene.physics.clone(),
        docs,
        prefab_library: Some(prefab_library),
        prefab_placements,
        kind: Rebuild::Switch {
            authored: crate::engine::authored_names(&scene),
            profile_text,
        },
    });

    // The switch succeeded. Anything reported along the way rides back as one warning — the switch
    // happened, but the game named a stale entity or asked one to both leave and stay behind.
    Ok(fold_warnings(warnings))
}

/// Folds the switch's non-fatal reports into the single warning the engine surfaces: the first, with
/// every later one attached as a `related` sub-diagnostic. `None` for a fully-clean switch.
fn fold_warnings(warnings: Vec<Diagnostic>) -> Option<Diagnostic> {
    let mut warnings = warnings.into_iter();
    let first = warnings.next()?;
    Some(warnings.fold(first, Diagnostic::with_related))
}

/// The eviction candidate: the scene whose last departure sits on the lowest frame, skipping `keep`
/// (the scene the switch just committed). `None` when `keep` is the only entry.
///
/// The map iterates in ascending key order and `min_by_key` returns the *first* minimum, so equal
/// last-visit frames break toward the lower scene name — the tie a session that left two scenes on the
/// same frame boundary needs answered identically everywhere.
fn least_recently_visited(residue: &StableMap<String, SceneResidue>, keep: &str) -> Option<String> {
    residue
        .iter()
        .filter(|(key, _)| key.as_str() != keep)
        .min_by_key(|(_, entry)| entry.last_visit)
        .map(|(key, _)| key.clone())
}

/// Captures the residue of the world a switch is leaving: one [`ResidueRecord`] per `Residue`-marked
/// entity, in entity order, each bag holding every registered component except `Name`.
///
/// Two shapes are reported and skipped rather than guessed at: an entity that is also in `carry` left
/// *and* stayed behind, so the carry wins (`L3027`); and a `Name` a second marked entity repeats
/// cannot address two records, so the first in entity order wins (`L3026`). Both are warnings — the
/// switch itself is unaffected.
///
/// `authored` is the set of `Name`s the departing world's scene document names; it decides each
/// record's [`authored`](ResidueRecord::authored) flag, and so what a missing counterpart means on
/// return. An empty set (a world rebuilt from a snapshot rather than a document) marks every record
/// code-spawned, which respawns — the reading that never loses state.
///
/// # Errors
/// `L0007` if a marked entity's component fails to serialize.
fn capture_residue(
    world: &ludion_ecs::World,
    registry: &Registry,
    carry: &[EntityId],
    authored: &StableSet<String>,
) -> Result<(Vec<ResidueRecord>, Vec<Diagnostic>), Diagnostic> {
    let mut records = Vec::new();
    let mut named = StableSet::new();
    let mut duplicates: Vec<String> = Vec::new();
    let mut conflicts: Vec<EntityId> = Vec::new();
    for id in world.entities() {
        if !world.has::<Residue>(id) {
            continue;
        }
        if carry.contains(&id) {
            conflicts.push(id);
            continue;
        }
        let name = world
            .get::<Name>(id)
            .ok()
            .flatten()
            .map(|name| name.0.clone());
        if let Some(name) = &name
            && !named.insert(name.clone())
        {
            duplicates.push(name.clone());
            continue;
        }
        let mut bag = Vec::new();
        for info in registry.iter() {
            if info.type_id() == TypeId::of::<Name>() {
                continue;
            }
            if let Some(value) = info.serialize(world, id)? {
                bag.push((info.name().to_string(), value));
            }
        }
        let authored = name.as_ref().is_some_and(|name| authored.contains(name));
        records.push(ResidueRecord {
            name,
            authored,
            bag,
        });
    }
    let mut warnings = Vec::new();
    if !conflicts.is_empty() {
        warnings.push(residue_carry_conflict(&conflicts));
    }
    if !duplicates.is_empty() {
        warnings.push(residue_duplicate_name(&duplicates));
    }
    Ok((records, warnings))
}

/// Re-applies a scene's captured residue over the world freshly instantiated from that scene's text,
/// returning what it had to report along the way (all warnings — a record the engine refuses to guess
/// at is dropped, never a reason to reject the switch).
///
/// Named records go first, in ascending `Name` order, each **replacing its counterpart in place**:
/// every registered component but `Name` is removed from the matched entity and the record's bag
/// inserted, so the entity keeps the `EntityId` the target scene minted and no id after it shifts. A
/// named record with **no** counterpart splits on [`authored`](ResidueRecord::authored): a
/// code-spawned one respawns carrying its `Name` (a game that spawns its own marked entity keeps it
/// across a visit — that state has no other home), an authored one is dropped and reported (`L3031`),
/// because the scene renamed the entity it belongs to and a spawn would duplicate it. Anonymous
/// records then spawn in capture order.
///
/// Two further shapes are reported: a bag this build can no longer read (`L3030` — residue outlives a
/// component change through a snapshot, so it is migrated exactly like a snapshot's world entities,
/// and the check runs *before* the world is touched because the in-place branch strips its entity
/// first — that record is dropped); and a name two entities of the target scene share (`L3026` — the
/// record lands on the first in entity order, as it always did, and both ids are named).
///
/// Both spawning passes run before the anchor lookup and the carried respawn, which is what keeps the
/// carried entities on the highest ids.
///
/// # Errors
/// `L0002`/`L0006` if a captured value no longer deserializes into its component — unreachable, since
/// the migration check above rejects exactly those bags first.
fn apply_residue(
    world: &mut ludion_ecs::World,
    registry: &Registry,
    records: &[ResidueRecord],
) -> Result<Vec<Diagnostic>, Diagnostic> {
    let mut warnings = Vec::new();
    // One throwaway world for the migration probe: a bag is deserialized into it to learn whether it
    // still reads, so nothing half-applied ever reaches the world being built.
    let mut probe = ludion_ecs::World::new();
    let mut named: Vec<&ResidueRecord> = records
        .iter()
        .filter(|record| record.name.is_some())
        .collect();
    named.sort_by(|a, b| a.name.cmp(&b.name));
    for record in named {
        let name = record.name.as_deref().expect("named records carry a name");
        if !record_reads(registry, &mut probe, record, &mut warnings) {
            continue;
        }
        let matches = entities_named(world, name);
        let Some(&id) = matches.first() else {
            // No counterpart. What that means depends on where the entity came from: the scene
            // renamed one it authored (drop it — a spawn would duplicate the renamed entity), or a
            // system spawned it and the scene never named it (respawn — that state has no other home).
            if record.authored {
                warnings.push(residue_name_unmatched(name));
            } else {
                let id = world.spawn();
                world
                    .insert(id, Name(name.to_string()))
                    .expect("the entity was just spawned and is alive");
                insert_bag(world, registry, id, record)?;
            }
            continue;
        };
        if matches.len() > 1 {
            warnings.push(residue_name_ambiguous(name, &matches));
        }
        for info in registry.iter() {
            if info.type_id() == TypeId::of::<Name>() {
                continue;
            }
            if info.has(world, id) {
                info.remove(world, id)?;
            }
        }
        insert_bag(world, registry, id, record)?;
    }
    for record in records.iter().filter(|record| record.name.is_none()) {
        if !record_reads(registry, &mut probe, record, &mut warnings) {
            continue;
        }
        let id = world.spawn();
        insert_bag(world, registry, id, record)?;
    }
    Ok(warnings)
}

/// Whether `record`'s whole bag still deserializes into this build's components, pushing the `L3030`
/// warning naming the components that do not when it does not.
///
/// The migration is `snapshot::migrate_component`'s policy at record granularity: a bag captured
/// before a component changed shape (or was unregistered) is dropped whole, because a residue record
/// *is* one entity's state and half of it is not a lesser version of it.
fn record_reads(
    registry: &Registry,
    probe: &mut ludion_ecs::World,
    record: &ResidueRecord,
    warnings: &mut Vec<Diagnostic>,
) -> bool {
    let entity = probe.spawn();
    let failed: Vec<&str> = record
        .bag
        .iter()
        .filter(|(name, value)| {
            registry
                .deserialize_insert(name, value, probe, entity)
                .is_err()
        })
        .map(|(name, _)| name.as_str())
        .collect();
    if failed.is_empty() {
        return true;
    }
    warnings.push(residue_record_unmigratable(record.name.as_deref(), &failed));
    false
}

/// Inserts every component of `record`'s bag onto `entity`, in capture (registration) order.
fn insert_bag(
    world: &mut ludion_ecs::World,
    registry: &Registry,
    entity: EntityId,
    record: &ResidueRecord,
) -> Result<(), Diagnostic> {
    for (name, value) in &record.bag {
        registry.deserialize_insert(name, value, world, entity)?;
    }
    Ok(())
}

/// Every entity in `world` carrying `Name(name)`, in entity order. More than one is an authoring
/// collision a residue record cannot resolve, so the caller reports it rather than the lookup hiding
/// it.
fn entities_named(world: &ludion_ecs::World, name: &str) -> Vec<EntityId> {
    world
        .entities()
        .filter(|&id| {
            world
                .get::<Name>(id)
                .ok()
                .flatten()
                .is_some_and(|found| found.0 == name)
        })
        .collect()
}

/// How a switch's spawn anchor resolved in the target world.
enum Anchor {
    /// The drop point: the anchor entity's `Transform.translation`.
    At([f32; 3]),
    /// An entity carries the anchor's `Name` but no `Transform`, so it names no drop point (`L3029`).
    NoTransform,
    /// No entity carries the anchor's `Name` (`L3019`).
    Missing,
}

/// Resolves `anchor` against the first entity in `world` carrying that `Name`.
///
/// A transform-less anchor is distinguished from a missing one rather than defaulting to the origin:
/// the origin is a plausible-looking place, so dropping a run there reads as a physics bug several
/// systems away from the scene file that caused it.
fn anchor_translation(world: &ludion_ecs::World, anchor: &str) -> Anchor {
    for id in world.entities() {
        if let Ok(Some(name)) = world.get::<Name>(id)
            && name.0 == anchor
        {
            return match crate::hierarchy::world_pose_transform(world, id) {
                Some(transform) => Anchor::At(transform.translation),
                None => Anchor::NoTransform,
            };
        }
    }
    Anchor::Missing
}

/// Builds the `L3018` warning for a target district that could not be loaded/validated, folding the
/// underlying scene diagnostics in as `related`.
fn target_unresolved(scene_path: &str, diagnostics: Vec<Diagnostic>) -> Diagnostic {
    Diagnostic::new(
        code::TRANSITION_TARGET_UNRESOLVED,
        format!("the target district `{scene_path}` could not be loaded for a scene switch"),
    )
    .with_help(
        "check the district's scene file exists under the project root and passes `check-scene`; \
         the switch was rejected and the sim continues in the current district",
    )
    .with_related(crate::commands::collapse(diagnostics))
}

/// Builds the `L3020` warning for carry ids that were not alive at the switch frame. The switch still
/// succeeded, carrying whichever ids were live; this names the skipped ones.
fn carry_dead(skipped: &[EntityId]) -> Diagnostic {
    let ids = skipped
        .iter()
        .map(|id| id.to_string())
        .collect::<Vec<_>>()
        .join(", ");
    Diagnostic::new(
        code::TRANSITION_CARRY_DEAD,
        format!(
            "the scene switch skipped {} carried entity that was not alive at the switch frame: {ids}",
            skipped.len()
        ),
    )
    .with_help(
        "build the carry list in the same frame the switch is requested, from live queries, so every \
         id names an entity that is still alive at the frame boundary; the switch proceeded without \
         the skipped entities",
    )
}

/// Builds the `L3043` warning for carried entities whose parent stayed behind. The switch still
/// succeeded; each arrived un-parented, keeping its world pose.
fn carry_orphaned(orphaned: &[EntityId]) -> Diagnostic {
    let ids = orphaned
        .iter()
        .map(|id| id.to_string())
        .collect::<Vec<_>>()
        .join(", ");
    Diagnostic::new(
        code::TRANSITION_CARRY_ORPHANED,
        format!(
            "the scene switch carried {} entity without its parent, which stayed behind: {ids}",
            orphaned.len()
        ),
    )
    .with_help(
        "carry the parent too (its whole subtree travels with it); the orphaned entities arrived \
         un-parented, keeping their world pose",
    )
}

/// A carried component holding entity references to carried entities: its serialized `value`, and for
/// each such field its pointer suffix and the index in the expanded carry list of the entity it names.
struct DeferredComponent {
    name: String,
    value: Value,
    pending: Vec<(String, usize)>,
}

/// Rewrites the entity references in `value`, `owner`'s serialized `info` component, for the carry.
/// A reference to a carried entity is returned as `(suffix, index in expanded)` to be written once the
/// respawned ids exist. One to an entity that stayed behind is cleared when its field is optional
/// (noted in `left_behind`); when it is required the whole component is dropped (noted too) and `None`
/// is returned.
fn remap_carried(
    info: &ludion_ecs::ComponentInfo,
    value: &mut Value,
    owner: EntityId,
    expanded: &[EntityId],
    left_behind: &mut Vec<String>,
) -> Option<Vec<(String, usize)>> {
    let mut pending = Vec::new();
    let mut cleared = Vec::new();
    for field in info.entity_refs() {
        let Some(target) = crate::engine::ref_target(value, field) else {
            continue;
        };
        if let Some(index) = expanded.iter().position(|e| *e == target) {
            pending.push((field.suffix.clone(), index));
            continue;
        }
        let item = format!("{owner} `{}`{} → {target}", info.name(), field.suffix);
        if !field.optional {
            left_behind.push(item);
            return None;
        }
        cleared.push((field.suffix.as_str(), item));
    }
    for (suffix, item) in cleared {
        if suffix.is_empty() {
            *value = Value::Null;
        } else if let Some(slot) = value.pointer_mut(suffix) {
            *slot = Value::Null;
        }
        left_behind.push(item);
    }
    Some(pending)
}

/// Builds the `L3043` warning for carried entities holding an entity reference to one that stayed
/// behind. The switch still succeeded; an optional reference arrived empty and a component whose
/// required reference stayed behind was left behind.
fn carry_left_behind(items: &[String]) -> Diagnostic {
    Diagnostic::new(
        code::TRANSITION_CARRY_ORPHANED,
        format!(
            "the scene switch carried {} reference to an entity that stayed behind: {}",
            items.len(),
            items.join(", ")
        ),
    )
    .with_help(
        "carry the referenced entity too; an optional reference arrived empty, a component whose \
         required reference stayed behind was left behind",
    )
}

/// Builds the `L3026` warning for `Name`s repeated among the departing scene's `Residue`-marked
/// entities. The first entity of each name was captured; the rest were skipped.
fn residue_duplicate_name(duplicates: &[String]) -> Diagnostic {
    let names = duplicates
        .iter()
        .map(|name| format!("`{name}`"))
        .collect::<Vec<_>>()
        .join(", ");
    Diagnostic::new(
        code::RESIDUE_DUPLICATE_NAME,
        format!(
            "the scene's residue could not capture {} `Residue`-marked entity that repeats a name \
             already captured: {names}",
            duplicates.len()
        ),
    )
    .with_help(
        "give each `Residue`-marked entity a unique `name:` in its scene, or leave an entity that \
         has no identity of its own unnamed — an unnamed record is anonymous and always respawns; \
         the switch proceeded, capturing the first entity of each name",
    )
}

/// Builds the `L3027` warning for entities named in the carry list that also carry the `Residue`
/// marker. The carry won; no residue was captured for them.
fn residue_carry_conflict(conflicts: &[EntityId]) -> Diagnostic {
    let ids = conflicts
        .iter()
        .map(|id| id.to_string())
        .collect::<Vec<_>>()
        .join(", ");
    Diagnostic::new(
        code::RESIDUE_CARRY_CONFLICT,
        format!(
            "the scene switch both carried and `Residue`-marked {} entity: {ids}",
            conflicts.len()
        ),
    )
    .with_help(
        "an entity either travels with the run (carried, no marker) or stays with its scene \
         (`Residue: {}`, never in the carry list); the switch carried it and captured no residue \
         for it",
    )
}

/// Builds the `L3028` warning for a session that visited more than [`RESIDUE_MAX_SCENES`] scenes: the
/// least recently visited one was dropped to make room for the scene just left.
fn residue_evicted(evicted: &str, last_visit: FrameIndex) -> Diagnostic {
    Diagnostic::new(
        code::RESIDUE_EVICTED,
        format!(
            "the session's scene residue holds at most {RESIDUE_MAX_SCENES} scenes, so it evicted the \
             least recently visited one: `{evicted}`, last left on frame {}",
            last_visit.as_u64()
        ),
    )
    .with_help(
        "keep fewer scenes alive in one session — mark `Residue: {}` only in the districts whose \
         state has to outlive a visit — or accept the drop: the evicted scene loads fresh from its \
         text on the next return, as if the run had never been there. The switch itself happened.",
    )
}

/// Builds the `L3028` warning for a scene that captured more than [`RESIDUE_MAX_RECORDS_PER_SCENE`]
/// records: the tail was dropped in capture order.
fn residue_records_capped(scene: &str, captured: usize) -> Diagnostic {
    let dropped = captured - RESIDUE_MAX_RECORDS_PER_SCENE;
    Diagnostic::new(
        code::RESIDUE_EVICTED,
        format!(
            "the scene `{scene}` left {captured} `Residue`-marked entities behind, past the \
             {RESIDUE_MAX_RECORDS_PER_SCENE} one scene's residue holds, so the last {dropped} were \
             dropped"
        ),
    )
    .with_help(
        "mark fewer entities `Residue: {}` in that scene: an unnamed record is never matched and \
         always respawns, so a system that drops one per event accumulates across a long visit. \
         Despawn them, or count them on one named entity instead. The dropped entities come back \
         authored on the next return; the switch itself happened.",
    )
}

/// Builds the `L3026` warning for a residue record whose `Name` is carried by more than one entity of
/// the target scene. The record lands on the first in entity order; this names every candidate.
fn residue_name_ambiguous(name: &str, matches: &[EntityId]) -> Diagnostic {
    let ids = matches
        .iter()
        .map(|id| id.to_string())
        .collect::<Vec<_>>()
        .join(", ");
    Diagnostic::new(
        code::RESIDUE_DUPLICATE_NAME,
        format!(
            "the residue record `{name}` matches {} entities of the scene it returned to: {ids}",
            matches.len()
        ),
    )
    .with_help(
        "give each entity a unique `name:` in its scene — a residue record addresses one entity, so \
         a repeated name makes which one it lands on an accident of entity order; the record was \
         applied to the first",
    )
}

/// Builds the `L3031` warning for a record captured off a **scene-authored** entity whose `Name` the
/// scene no longer carries. The record is dropped for this visit rather than spawned, so the renamed
/// entity is not duplicated. A code-spawned record never reaches here — it respawns.
fn residue_name_unmatched(name: &str) -> Diagnostic {
    Diagnostic::new(
        code::RESIDUE_NAME_UNMATCHED,
        format!(
            "the residue record `{name}` was captured off an entity the scene authored, which the \
             scene no longer names, so it was dropped"
        ),
    )
    .with_help(
        "keep a `Residue`-marked entity's `name:` stable across scene edits — it is the record's \
         identity, not a label. The record is not spawned as a fresh entity, because the scene \
         authors that entity under its new name and a spawn would duplicate it.",
    )
}

/// Builds the `L3030` warning for a residue record this build can no longer read, naming the
/// components that failed. The record is dropped whole — half an entity's state is not a lesser
/// version of it.
fn residue_record_unmigratable(name: Option<&str>, failed: &[&str]) -> Diagnostic {
    let record = match name {
        Some(name) => format!("`{name}`"),
        None => "an anonymous record".to_string(),
    };
    let components = failed
        .iter()
        .map(|component| format!("`{component}`"))
        .collect::<Vec<_>>()
        .join(", ");
    Diagnostic::new(
        code::RESIDUE_RECORD_UNMIGRATABLE,
        format!(
            "the residue record {record} was dropped: {} of its captured components no longer read \
             into this build: {components}",
            failed.len()
        ),
    )
    .with_help(
        "residue outlives a component change through a snapshot (`ludion dev`'s restart, a replay \
         seek), so a bag captured by an earlier shape of the component cannot be applied; the scene \
         rebuilds that entity from its text and the switch proceeded. Start a fresh session to stop \
         carrying the stale record.",
    )
}

/// Builds the `L3029` rejection for a spawn anchor that exists in the target district but carries no
/// `Transform`, so it names no drop point.
fn anchor_without_transform(scene_path: &str, anchor: &str) -> Diagnostic {
    Diagnostic::new(
        code::TRANSITION_ANCHOR_NO_TRANSFORM,
        format!(
            "the spawn anchor `{anchor}` in the target district `{scene_path}` carries no `Transform`"
        ),
    )
    .with_help(
        "give the anchor entity a `Transform` at the point the run should arrive; the switch was \
         rejected rather than dropping the carried entities at the world origin, and the sim \
         continues in the current district",
    )
}

/// Builds the `L3019` warning for a spawn anchor missing from the (successfully loaded) target
/// district.
fn anchor_missing(scene_path: &str, anchor: &str) -> Diagnostic {
    Diagnostic::new(
        code::TRANSITION_ANCHOR_MISSING,
        format!("the spawn anchor `{anchor}` was not found in the target district `{scene_path}`"),
    )
    .with_help(
        "add an entity named `<anchor>` (the `name:` field) to the target district at the drop \
         point, or fix the anchor name in the switch request; the switch was rejected and the sim \
         continues in the current district",
    )
}
