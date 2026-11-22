//! The per-entity clock domain: while a `holds_gameplay` sequence plays, the entities bound to its
//! roles (and their `Parent` descendants) run on their own gameplay instant, so the held beat
//! animates its actors while every unbound entity stays frozen (`ludion docs sequences`).

use std::collections::BTreeSet;

use ludion_camera::CameraRig;
use ludion_components::{Animation, AnimationLayers, BlendClock, ClipAnchor};
use ludion_core::ids::EntityId;
use ludion_core::time::{FrameIndex, GameClock, GameTime, Q16_PER_TICK};
use ludion_ecs::{SystemCtx, World};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::{SequenceBinding, SequencePlayer, find_doc};
use crate::hierarchy::parent_of;

/// Marks an entity that runs on its own gameplay instant while a `holds_gameplay` sequence holds the
/// game clock. Engine-written when the beat starts, removed the tick after it ends; hashed and
/// snapshotted like any component while it exists. Read the entity's time through [`entity_clock`].
///
/// Its instant is `bound_game + (sim - bound_sim)` frames.
///
/// ```
/// use ludion_engine::sequences::SequenceBound;
///
/// let bound: SequenceBound = serde_json::from_value(serde_json::json!({
///     "player": { "index": 1, "generation": 0 }, "bound_sim": 12, "bound_game": 786432,
/// }))
/// .unwrap();
/// assert_eq!(bound.bound_game, 12 * 65536);
/// ```
#[derive(Serialize, Deserialize, JsonSchema, Clone, Debug, PartialEq)]
pub struct SequenceBound {
    /// The [`SequencePlayer`] entity that bound it.
    #[schemars(with = "ludion_ecs::EntityRefSchema")]
    pub player: EntityId,
    /// The sim frame the entity was bound at.
    #[schemars(with = "u64")]
    pub bound_sim: FrameIndex,
    /// The gameplay instant the entity was bound at, in Q16 (1/65536 frame).
    pub bound_game: u64,
}

/// One entity's gameplay time as a system reads it: the global `ctx.game_*` for an unbound entity,
/// its own instant for a bound one.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct EntityClock {
    /// The exact gameplay instant.
    pub instant: GameTime,
    /// `instant`'s floor in whole frames.
    pub frame: FrameIndex,
    /// The gameplay timestep: `ctx.dt` for a bound entity, `ctx.game_dt` otherwise.
    pub dt: f32,
    /// The fraction of a tick gameplay time advanced by: `1.0` for a bound entity.
    pub scale: f32,
    /// Whether this entity's time advanced into this tick.
    pub advanced: bool,
}

fn bound_of(world: &World, id: EntityId) -> Option<SequenceBound> {
    if world.component_count::<SequenceBound>() == 0 || !world.has::<SequenceBound>(id) {
        return None;
    }
    // Unrecorded: an engine-optional component must not demand a game's query declaration.
    world
        .get_unrecorded::<SequenceBound>(id)
        .ok()
        .flatten()
        .cloned()
}

/// The Q16 instant of `bound` at sim frame `frame`.
fn bound_q16(bound: &SequenceBound, frame: FrameIndex) -> u64 {
    let since = frame.as_u64().saturating_sub(bound.bound_sim.as_u64());
    bound
        .bound_game
        .saturating_add(since.saturating_mul(Q16_PER_TICK))
}

/// Whether `id` runs on its own clock.
pub(crate) fn is_bound(world: &World, id: EntityId) -> bool {
    bound_of(world, id).is_some()
}

/// Whether any entity is bound; a cheap gate for walks that only matter then.
pub(crate) fn any_bound(world: &World) -> bool {
    world.component_count::<SequenceBound>() > 0
}

/// `id`'s gameplay clock this tick: its own instant when a holding sequence bound it, else exactly
/// the context's global `game_*` fields. Reads without recording, so an opted-out game demands no
/// query declaration.
///
/// ```
/// use ludion_core::ids::EntityId;
/// use ludion_core::time::{FrameIndex, TickRate};
/// use ludion_ecs::{SystemCtx, World};
/// use ludion_engine::sequences::entity_clock;
///
/// let mut world = World::new();
/// let e = world.spawn();
/// let ctx = SystemCtx::new(FrameIndex::new(7), TickRate::DEFAULT);
/// let clock = entity_clock(&world, e, &ctx);
/// assert_eq!(clock.frame, ctx.game_frame);
/// assert!(clock.advanced);
/// ```
pub fn entity_clock(world: &World, id: EntityId, ctx: &SystemCtx) -> EntityClock {
    match bound_of(world, id) {
        Some(bound) => {
            let q = bound_q16(&bound, ctx.frame);
            let instant = GameTime::from_q16(q, ctx.time.rate());
            EntityClock {
                instant,
                frame: FrameIndex::new(instant.ticks_floor()),
                dt: ctx.dt,
                scale: 1.0,
                advanced: true,
            }
        }
        None => EntityClock {
            instant: ctx.game_instant,
            frame: ctx.game_frame,
            dt: ctx.game_dt,
            scale: ctx.game_scale(),
            advanced: ctx.game_advanced(),
        },
    }
}

/// `game_now` (the world's gameplay instant, read after the tick) rebased to `id`'s own instant. The
/// sim instant is recovered exactly from the same clock state that produced `game_now`.
pub(crate) fn rebased_instant(world: &World, id: EntityId, game_now: GameTime) -> GameTime {
    let Some(bound) = bound_of(world, id) else {
        return game_now;
    };
    let excluded = world
        .resource::<GameClock>()
        .map_or(0, |clock| clock.excluded_q16());
    let sim = game_now.as_q16().saturating_add(excluded);
    let since = sim.saturating_sub(bound.bound_sim.as_u64().saturating_mul(Q16_PER_TICK));
    GameTime::from_q16(bound.bound_game.saturating_add(since), game_now.rate())
}

/// Whether `entity` is `root` or has it among its `Parent` ancestors.
fn under(world: &World, entity: EntityId, root: EntityId) -> bool {
    let mut current = entity;
    // A `Parent` loop is reported elsewhere; the bound keeps the walk finite.
    for _ in 0..256 {
        if current == root {
            return true;
        }
        match parent_of(world, current) {
            Some(parent) => current = parent.0,
            None => return false,
        }
    }
    false
}

/// Binds every live role entity and its `Parent` descendants to `player`. An entity that already
/// carries a binding keeps it; a descendant spawned later is not bound.
pub(crate) fn bind(world: &mut World, player: EntityId, roles: &[EntityId], ctx: &SystemCtx) {
    let roots: Vec<EntityId> = roles
        .iter()
        .copied()
        .filter(|&entity| world.is_alive(entity))
        .collect();
    if roots.is_empty() {
        return;
    }
    let mut targets: BTreeSet<EntityId> = roots.iter().copied().collect();
    for entity in world.entities() {
        if roots.iter().any(|&root| under(world, entity, root)) {
            targets.insert(entity);
        }
    }
    for entity in targets {
        if world.has::<SequenceBound>(entity) {
            continue;
        }
        let _ = world.insert(
            entity,
            SequenceBound {
                player,
                bound_sim: ctx.frame,
                bound_game: ctx.game_instant.as_q16(),
            },
        );
    }
}

/// Handles every [`SequenceBound`] whose player is no longer a live `holds_gameplay` player: it
/// re-points to another live holding player that binds the entity, else it is released and the
/// entity's gameplay stamps shift back by the lead, so nothing jumps.
pub(crate) fn release_orphans(world: &mut World, ctx: &SystemCtx) {
    if !any_bound(world) {
        return;
    }
    let mut bounds: Vec<(EntityId, SequenceBound)> = world
        .query_ids::<(&SequenceBound,)>()
        .map(|(id, (bound,))| (id, bound.clone()))
        .collect();
    bounds.sort_by_key(|(id, _)| *id);
    let players: Vec<(EntityId, String)> = world
        .query_ids::<(&SequencePlayer,)>()
        .map(|(id, (player,))| (id, player.sequence.clone()))
        .collect();
    let mut holding: Vec<EntityId> = players
        .into_iter()
        .filter(|(_, key)| find_doc(world, key).is_some_and(|doc| doc.holds_gameplay))
        .map(|(id, _)| id)
        .collect();
    holding.sort();
    let orphans: Vec<(EntityId, SequenceBound)> = bounds
        .into_iter()
        .filter(|(_, bound)| holding.binary_search(&bound.player).is_err())
        .collect();
    if orphans.is_empty() {
        return;
    }
    let mut roles: Vec<(EntityId, EntityId)> = world
        .query_ids::<(&SequenceBinding,)>()
        .filter(|(_, (binding,))| holding.binary_search(&binding.player).is_ok())
        .map(|(_, (binding,))| (binding.player, binding.entity))
        .collect();
    roles.sort();
    for (id, bound) in orphans {
        let taker = roles
            .iter()
            .find(|(_, role)| world.is_alive(*role) && under(world, id, *role))
            .map(|(player, _)| *player);
        if let Some(player) = taker {
            if let Ok(Some(stored)) = world.get_mut::<SequenceBound>(id) {
                stored.player = player;
            }
            continue;
        }
        let lead_q = bound_q16(&bound, ctx.frame).saturating_sub(ctx.game_instant.as_q16());
        let lead = (lead_q + Q16_PER_TICK / 2) / Q16_PER_TICK;
        shift_stamps(world, id, lead);
        let _ = world.remove::<SequenceBound>(id);
    }
}

fn back(frame: FrameIndex, lead: u64) -> FrameIndex {
    FrameIndex::new(frame.as_u64().saturating_sub(lead))
}

fn back_anchor(anchor: &mut Option<ClipAnchor>, lead: u64) {
    if let Some(anchor) = anchor {
        anchor.started = back(anchor.started, lead);
        anchor.frame = back(anchor.frame, lead);
    }
}

/// Shifts every gameplay stamp of `id` that reads the entity clock back by `lead` frames.
pub(crate) fn shift_stamps(world: &mut World, id: EntityId, lead: u64) {
    if lead == 0 {
        return;
    }
    if let Ok(Some(animation)) = world.get_mut::<Animation>(id) {
        animation.started = back(animation.started, lead);
        animation.from_started = back(animation.from_started, lead);
        animation.fade_started = back(animation.fade_started, lead);
        back_anchor(&mut animation.anchor, lead);
        back_anchor(&mut animation.from_anchor, lead);
    }
    if let Ok(Some(stack)) = world.get_mut::<AnimationLayers>(id) {
        for layer in &mut stack.layers {
            layer.started = back(layer.started, lead);
            layer.changed = back(layer.changed, lead);
        }
    }
    if let Ok(Some(clock)) = world.get_mut::<BlendClock>(id) {
        clock.started = back(clock.started, lead);
        clock.reset = back(clock.reset, lead);
        for layer in &mut clock.layers {
            layer.started = back(layer.started, lead);
        }
    }
    if let Ok(Some(rig)) = world.get_mut::<CameraRig>(id)
        && let Some(blend) = &mut rig.state.blend
    {
        blend.started = back(blend.started, lead);
    }
}

#[cfg(test)]
mod tests {
    use ludion_core::time::TickRate;
    use ludion_ecs::Parent;

    use super::*;

    fn bound(player: EntityId, sim: u64, game_ticks: u64) -> SequenceBound {
        SequenceBound {
            player,
            bound_sim: FrameIndex::new(sim),
            bound_game: game_ticks * Q16_PER_TICK,
        }
    }

    #[test]
    fn an_unbound_entity_clock_equals_the_context_field_for_field() {
        let mut world = World::new();
        let e = world.spawn();
        let mut clock = GameClock::default();
        clock.scale(0.5);
        let advanced = clock.advance();
        let ctx =
            SystemCtx::new(FrameIndex::new(9), TickRate::DEFAULT).with_clock(&clock, advanced);
        let own = entity_clock(&world, e, &ctx);
        assert_eq!(own.instant, ctx.game_instant);
        assert_eq!(own.frame, ctx.game_frame);
        assert_eq!(own.dt, ctx.game_dt);
        assert_eq!(own.scale, ctx.game_scale());
        assert_eq!(own.advanced, ctx.game_advanced());
    }

    #[test]
    fn a_bound_entity_runs_on_sim_time_from_its_bind_instant() {
        let mut world = World::new();
        let (player, e) = (world.spawn(), world.spawn());
        world.insert(e, bound(player, 10, 4)).unwrap();
        // Held for the whole stretch: the global clock still says 4.
        let mut clock = GameClock::default();
        clock.hold();
        let ctx = SystemCtx::new(FrameIndex::new(13), TickRate::DEFAULT).with_clock(&clock, false);
        let own = entity_clock(&world, e, &ctx);
        assert_eq!(own.instant.as_q16(), 7 * Q16_PER_TICK);
        assert_eq!(own.frame, FrameIndex::new(7));
        assert_eq!(own.dt, ctx.dt);
        assert_eq!(own.scale, 1.0);
        assert!(own.advanced && !ctx.game_advanced());
    }

    #[test]
    fn rebased_instant_recovers_the_sim_instant_with_a_hold_marked() {
        let mut world = World::new();
        let (player, e) = (world.spawn(), world.spawn());
        world.insert(e, bound(player, 10, 4)).unwrap();
        let mut clock = GameClock::default();
        for _ in 0..3 {
            clock.hold();
            clock.advance();
        }
        clock.hold(); // marked, not yet folded
        let now = clock.game_instant(ludion_core::time::SimTime::from_ticks(
            14,
            TickRate::DEFAULT,
        ));
        world.insert_resource(clock);
        let rebased = rebased_instant(&world, e, now);
        assert_eq!(rebased.as_q16(), (4 + 4) * Q16_PER_TICK);
        assert_eq!(rebased_instant(&world, player, now), now);
    }

    #[test]
    fn shift_stamps_moves_every_listed_field() {
        let mut world = World::new();
        let e = world.spawn();
        let f = FrameIndex::new;
        let anchor = ClipAnchor {
            started: f(20),
            frame: f(24),
            ticks: 1.0,
            grid_ticks: None,
        };
        world
            .insert(
                e,
                Animation {
                    started: f(20),
                    from_started: f(18),
                    fade_started: f(22),
                    anchor: Some(anchor),
                    from_anchor: Some(anchor),
                    ..Animation::default()
                },
            )
            .unwrap();
        shift_stamps(&mut world, e, 5);
        let animation = world.get::<Animation>(e).unwrap().unwrap();
        assert_eq!(
            (
                animation.started,
                animation.from_started,
                animation.fade_started
            ),
            (f(15), f(13), f(17))
        );
        for anchor in [animation.anchor, animation.from_anchor] {
            let anchor = anchor.unwrap();
            assert_eq!((anchor.started, anchor.frame), (f(15), f(19)));
        }
        shift_stamps(&mut world, e, 100);
        assert_eq!(world.get::<Animation>(e).unwrap().unwrap().started, f(0));
    }

    #[test]
    fn the_descendant_walk_follows_the_parent_chain() {
        let mut world = World::new();
        let (root, child, grandchild, other) =
            (world.spawn(), world.spawn(), world.spawn(), world.spawn());
        world.insert(child, Parent(root)).unwrap();
        world.insert(grandchild, Parent(child)).unwrap();
        assert!(under(&world, grandchild, root));
        assert!(under(&world, root, root));
        assert!(!under(&world, root, grandchild));
        assert!(!under(&world, other, root));
        let ctx = SystemCtx::new(FrameIndex::new(3), TickRate::DEFAULT);
        let player = world.spawn();
        let mut registry_less = world;
        // `bind` inserts through the world, so the column exists once it runs.
        bind(&mut registry_less, player, &[root], &ctx);
        for e in [root, child, grandchild] {
            assert!(registry_less.has::<SequenceBound>(e));
        }
        assert!(!registry_less.has::<SequenceBound>(other));
    }
}
