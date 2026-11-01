//! Render-side cuts for display-rate pose blending.
//!
//! A windowed host draws between two ticks' poses; an entity that jumps (a teleport, a camera cut)
//! must not slide across the gap. [`NoInterpolate`] is the marker a game writes; the engine's own
//! snap sites write the crate-private [`PresentCuts`] resource instead, so a game system that calls
//! `teleport_character` gains no undeclared component write.

use std::collections::BTreeSet;

use ludion_ecs::{Parent, World};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use ludion_core::ids::EntityId;

use crate::hierarchy::MAX_HIERARCHY_DEPTH;

/// A render-only, one-tick marker: the entity (and its `Parent` descendants) take their newer pose
/// unblended on the next presented frame. The engine removes it before the next tick runs, so insert
/// it in the tick that moved the entity.
///
/// It covers a hand-written `Transform` teleport. It is never registered, so it is never hashed,
/// snapshotted, saved or sent over the wire.
///
/// # Example
/// ```
/// use ludion_engine::NoInterpolate;
/// let marker = NoInterpolate;
/// assert_eq!(marker, NoInterpolate::default());
/// ```
#[derive(Serialize, Deserialize, JsonSchema, Clone, Copy, Debug, Default, PartialEq)]
pub struct NoInterpolate;

/// Entities the engine itself cut this tick (teleport, camera rig cut).
#[derive(Default)]
pub(crate) struct PresentCuts(Vec<EntityId>);

/// Marks `id` as cut for the next presented frame.
pub(crate) fn cut_presentation(world: &mut World, id: EntityId) {
    match world.resource_mut::<PresentCuts>() {
        Some(cuts) => cuts.0.push(id),
        None => {
            world.insert_resource(PresentCuts(vec![id]));
        }
    }
}

/// Forgets every cut: removes each [`NoInterpolate`] and empties [`PresentCuts`].
pub(crate) fn clear_cuts(world: &mut World) {
    let marked: Vec<EntityId> = world
        .query_ids::<(&NoInterpolate,)>()
        .map(|(id, _)| id)
        .collect();
    for id in marked {
        let _ = world.remove::<NoInterpolate>(id);
    }
    if let Some(cuts) = world.resource_mut::<PresentCuts>() {
        cuts.0.clear();
    }
}

/// The keys cut this tick, ascending and unique: each key whose own id, or any `Parent` ancestor's,
/// carries [`NoInterpolate`] or is in [`PresentCuts`].
pub(crate) fn snap_set(world: &World, keys: impl Iterator<Item = EntityId>) -> Vec<EntityId> {
    let cuts: BTreeSet<EntityId> = world
        .resource::<PresentCuts>()
        .map_or_else(BTreeSet::new, |cuts| cuts.0.iter().copied().collect());
    let cut = |id: EntityId| cuts.contains(&id) || world.has::<NoInterpolate>(id);
    let mut out = BTreeSet::new();
    for key in keys {
        let mut current = key;
        for _ in 0..=MAX_HIERARCHY_DEPTH {
            if cut(current) {
                out.insert(key);
                break;
            }
            if !world.has::<Parent>(current) {
                break;
            }
            match world.get::<Parent>(current) {
                Ok(Some(Parent(parent))) => current = *parent,
                _ => break,
            }
        }
    }
    out.into_iter().collect()
}

#[cfg(test)]
mod tests {
    use super::NoInterpolate;
    use crate::Engine;
    use ludion_ecs::{SystemCtx, World};
    use ludion_protocol::event::Event;

    const SCENE: &str = "format: 1\nseed: 3\nentities:\n  - name: box\n    components:\n      Transform: {}\n      Mesh:\n        shape: cube\n";

    #[test]
    fn the_scene_is_identical_with_and_without_blend_collection() {
        let mut engine = Engine::builder().build().expect("the game builds");
        engine.load_inline_scene(SCENE).expect("the scene loads");
        let (width, height) = engine.render_size();
        let plain = engine
            .extract_render_scene_through(width, height, true)
            .expect("extracts");
        let (with_blend, data) = engine.extract_present_scene().expect("extracts");
        assert_eq!(plain, with_blend);
        assert_eq!(data.meshes.len(), with_blend.meshes.len());
    }

    fn mark(world: &mut World, _ctx: &SystemCtx) {
        let id = world.entities().next().expect("the box");
        world.insert(id, NoInterpolate).unwrap();
    }

    #[test]
    fn the_marker_is_exempt_from_the_unregistered_component_warning() {
        let mut engine = Engine::builder()
            .system("mark", mark)
            .build()
            .expect("the game builds");
        engine.load_inline_scene(SCENE).expect("the scene loads");
        for _ in 0..3 {
            engine.advance_live();
        }
        let unregistered = engine.take_events().into_iter().any(|event| match event {
            Event::Log(log) => log.diagnostic.is_some_and(|d| d.code.as_str() == "L0027"),
            _ => false,
        });
        assert!(!unregistered, "NoInterpolate must not raise L0027");
    }
}
