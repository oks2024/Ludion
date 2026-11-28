//! Blend-space phase: the `blend_clocks` sim system and the event crossings a blend emits
//! (`Plan/animation-layers.md` decisions 13-19).
//!
//! An entity whose `Animation.clip` names a blend of its model (a skinned or voxel `kind: assembly`,
//! or the `kind: prefab` its instance root plays) plays every
//! point clip on one shared normalized phase. [`blend_clocks`] integrates that phase, in unwrapped
//! cycles, into a hashed [`BlendClock`] on the entity; `extract` poses from it and
//! `animation_events` fires the dominant point's events on its crossings. The rate is
//! `Animation.speed * sum(weight * binding_speed / duration)` per gameplay second, so a cadence
//! follows the parameters without a pose jump. With no clock (the system is not registered) extract
//! falls back to the stateless `elapsed * rate(params now)`.
//!
//! Only an entity whose clip (or the outgoing side of a live crossfade) names a blend ever carries a
//! `BlendClock`, so a world with no blends hashes exactly as before.

use ludion_core::ids::EntityId;
use ludion_core::time::{FrameIndex, GameTime};
use ludion_ecs::{SystemCtx, World};

use crate::animation_events::{AnimationLibrary, Model};
use crate::blend_space::BlendSpaceData;
use crate::components::{
    Animation, AnimationLayer, AnimationLayers, BlendClock, LayerPhase, Mesh, MeshSource,
};
use crate::extract::{clip_elapsed, clock_elapsed, fade_weight, is_prefab_player};
use crate::prefab::PrefabInstance;
use crate::sequences::entity_clock;
use ludion_components::animation_layers::MAX_ANIMATION_LAYERS;

/// The most cycles one tick's crossings visit, the latest: a tick that spans more (an absurd rate)
/// neither stalls nor overflows.
const MAX_CYCLES_PER_TICK: i64 = 64;

/// The phase rate of `blend` at `animation.params`, in cycles per second before `Animation.speed`.
pub(crate) fn blend_rate(blend: &BlendSpaceData, params: [f32; 2]) -> f64 {
    blend.rate(&blend.weights(params))
}

/// The stateless phase of a blend at `now`: `elapsed * rate(params now)`. What extract reads when no
/// [`BlendClock`] matches the animation, and where a fresh clock starts.
pub(crate) fn fallback_phase(blend: &BlendSpaceData, animation: &Animation, now: GameTime) -> f64 {
    clip_elapsed(animation, now) * blend_rate(blend, animation.params)
}

/// The stateless phase of a layer's blend at `now`: `elapsed * rate(layer params now)`. What extract
/// reads when no [`BlendClock`] layer entry matches, and where a fresh entry starts.
pub(crate) fn layer_fallback_phase(
    blend: &BlendSpaceData,
    layer: &AnimationLayer,
    now: GameTime,
) -> f64 {
    clock_elapsed(now, layer.started, layer.speed, None) * blend_rate(blend, layer.params)
}

/// Integrates the [`BlendClock`] of every character, assembly or prefab instance root whose
/// `Animation.clip` (or, while a crossfade is live, `from`) or whose `AnimationLayers` layer `clip`
/// names a blend of its model, and removes the clock from one that no longer does.
///
/// Entities run in ascending id order. A held gameplay frame only parks `prev` on `phase`. A changed
/// `(clip, started)` restarts the phase at the stateless value for the new clock and, when the
/// outgoing clip was the blend the clock held, carries that phase for the crossfade's outgoing side.
/// Each layer naming a blend keeps a `BlendClock.layers` entry integrated the same way at the layer's
/// `params` and `speed`, restarted when its `(blend, started)` changes; a clock with no base blend
/// keeps `blend` empty and its base fields zero.
///
/// A game registers it after whatever writes `Animation` or `AnimationLayers` and before
/// `animation_events`, and declares its accesses (the layers are read by component lookup, so they
/// need no query line):
///
/// ```
/// use ludion_engine::{
///     Animation, BlendClock, Engine, Mesh, PrefabInstance, animation_events, blend_clocks,
/// };
/// use ludion_protocol::{Command, command};
///
/// let mut engine = Engine::builder()
///     .system("blend_clocks", blend_clocks)
///     .query::<(&Animation, &Mesh)>()
///     .query::<(&Animation, &PrefabInstance)>()
///     .query::<(&mut BlendClock,)>()
///     .system("animation_events", animation_events)
///     .query::<(&Animation, &Mesh)>()
///     .query::<(&Animation, &PrefabInstance)>()
///     .query::<(&BlendClock,)>()
///     .build()
///     .unwrap();
/// engine
///     .execute(Command::SimStep(command::SimStepParams { ticks: 1, hash: false }))
///     .unwrap();
/// ```
pub fn blend_clocks(world: &mut World, ctx: &SystemCtx) {
    let Some(mut library) = world.remove_resource::<AnimationLibrary>() else {
        return;
    };
    let mut candidates: Vec<(EntityId, Animation, Model)> = world
        .query_ids::<(&Animation, &Mesh)>()
        .filter_map(|(id, (animation, mesh))| match &mesh.shape {
            MeshSource::Assembly(rel) | MeshSource::Character(rel) => {
                Some((id, animation.clone(), Model::Assembly(rel.clone())))
            }
            _ => None,
        })
        .collect();
    // A prefab player never has a mesh model, so no entity is listed twice.
    let roots: Vec<(EntityId, Animation, PrefabInstance)> = world
        .query_ids::<(&Animation, &PrefabInstance)>()
        .filter(|(id, (_, instance))| instance.root == *id)
        .map(|(id, (animation, instance))| (id, animation.clone(), instance.clone()))
        .collect();
    for (id, animation, instance) in roots {
        if is_prefab_player(world, id, &instance) {
            candidates.push((id, animation, Model::Prefab(instance.source)));
        }
    }
    candidates.sort_by_key(|(id, _, _)| *id);
    for (id, animation, model) in candidates {
        let own = entity_clock(world, id, ctx);
        let ticks = own.frame.as_u64();
        let seconds = f64::from(own.dt);
        let blends = library.blends(&model);
        let blend_named = |name: &str| blends.iter().find(|blend| blend.name == name);
        let incoming = (!animation.clip.is_empty())
            .then(|| blend_named(&animation.clip))
            .flatten();
        let fading = animation.fade > 0.0
            && !animation.from.is_empty()
            && fade_weight(&animation, own.instant).is_some();
        let outgoing = fading.then(|| blend_named(&animation.from)).flatten();
        // Unrecorded: engine-optional components must not demand a game's query declaration.
        let layer_blends: Vec<(&AnimationLayer, &BlendSpaceData)> = world
            .get_unrecorded::<AnimationLayers>(id)
            .ok()
            .flatten()
            .map(|stack| {
                stack
                    .layers
                    .iter()
                    .take(MAX_ANIMATION_LAYERS)
                    .filter_map(|layer| Some((layer, blend_named(&layer.clip)?)))
                    .collect()
            })
            .unwrap_or_default();
        let had = world
            .get_unrecorded::<BlendClock>(id)
            .ok()
            .flatten()
            .is_some();
        let base_blends = incoming.is_some() || outgoing.is_some();
        if !base_blends && layer_blends.is_empty() {
            if had {
                let _ = world.remove::<BlendClock>(id);
            }
            continue;
        }
        let name = incoming.map_or_else(String::new, |blend| blend.name.clone());
        let mut clock = world
            .get_unrecorded::<BlendClock>(id)
            .ok()
            .flatten()
            .cloned()
            .unwrap_or_default();
        let fresh = !had;
        if !own.advanced {
            if fresh {
                continue;
            }
            clock.prev = clock.phase;
            for entry in &mut clock.layers {
                entry.prev = entry.phase;
            }
            let _ = world.insert(id, clock);
            continue;
        }
        let old_layers = std::mem::take(&mut clock.layers);
        clock.layers = layer_blends
            .iter()
            .map(|&(layer, blend)| {
                let old = old_layers.iter().find(|entry| {
                    entry.name == layer.name
                        && entry.blend == blend.name
                        && entry.started == layer.started
                });
                match old {
                    None => LayerPhase {
                        name: layer.name.clone(),
                        blend: blend.name.clone(),
                        started: layer.started,
                        phase: layer_fallback_phase(blend, layer, own.instant),
                        prev: -1.0,
                    },
                    Some(entry) => LayerPhase {
                        prev: entry.phase,
                        phase: if ticks < layer.started.as_u64() {
                            0.0
                        } else {
                            entry.phase
                                + blend_rate(blend, layer.params) * f64::from(layer.speed) * seconds
                        },
                        ..entry.clone()
                    },
                }
            })
            .collect();
        if !base_blends {
            clock.blend = String::new();
            clock.started = FrameIndex::ZERO;
            clock.reset = FrameIndex::ZERO;
            (clock.phase, clock.prev) = (0.0, 0.0);
            (clock.from_blend, clock.from_phase) = (String::new(), 0.0);
            let _ = world.insert(id, clock);
            continue;
        }
        let restarts = fresh || clock.blend != name || clock.started != animation.started;
        if restarts {
            let carries =
                outgoing.is_some() && !clock.blend.is_empty() && clock.blend == animation.from;
            (clock.from_blend, clock.from_phase) = if carries {
                (clock.blend.clone(), clock.phase)
            } else {
                (String::new(), 0.0)
            };
            clock.blend = name;
            clock.started = animation.started;
            clock.reset = own.frame;
            clock.phase =
                incoming.map_or(0.0, |blend| fallback_phase(blend, &animation, own.instant));
            clock.prev = -1.0;
        } else {
            clock.prev = clock.phase;
            if let Some(blend) = incoming {
                clock.phase = if ticks < animation.started.as_u64() {
                    0.0
                } else {
                    clock.phase
                        + blend_rate(blend, animation.params) * f64::from(animation.speed) * seconds
                };
            }
            match outgoing {
                Some(blend) if !clock.from_blend.is_empty() => {
                    clock.from_phase += blend_rate(blend, animation.params)
                        * f64::from(animation.from_speed)
                        * seconds;
                }
                _ => (clock.from_blend, clock.from_phase) = (String::new(), 0.0),
            }
        }
        let _ = world.insert(id, clock);
    }
    world.insert_resource(library);
}

/// The names of the events the dominant point of `blend` fires as the shared phase moves over the
/// `(prev, phase]` window of a [`BlendClock`] (or one of its layer entries); a restart parks `prev` at `-1`, which opens the window at phase
/// 0 inclusive (`[0, phase]`). Each event sits at phase position `frac(t / duration - sync)`, so a synced event
/// lands at phase 0 and fires once per cycle at any rate. At most [`MAX_CYCLES_PER_TICK`] cycles.
pub(crate) fn blend_crossings(
    blend: &BlendSpaceData,
    params: [f32; 2],
    (prev, now): (f64, f64),
    out: &mut Vec<String>,
) {
    if !(prev.is_finite() && now.is_finite()) {
        return;
    }
    let weights = blend.weights(params);
    if weights.iter().next().is_none() {
        return;
    }
    let point = &blend.points[usize::from(weights.dominant())];
    if !(point.duration > 0.0 && point.duration.is_finite()) {
        return;
    }
    if now <= prev {
        return;
    }
    let in_window = |p: f64| p > prev && p <= now;
    let lower = prev.max(0.0);
    let first = (lower.floor() as i64).saturating_sub(1).max(0);
    let last = (now.floor() as i64).saturating_add(1);
    let first = first.max((now.floor() as i64).saturating_sub(MAX_CYCLES_PER_TICK));
    for cycle in first..=last {
        for event in &point.events {
            let x = event.t / point.duration - point.sync;
            let position = x - x.floor() + cycle as f64;
            if in_window(position) {
                out.push(event.name.clone());
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use ludion_core::time::FrameIndex;
    use ludion_scene::AnimationEvent;

    use super::*;
    use crate::blend_space::BlendPointData;

    fn blend(sync: f64) -> BlendSpaceData {
        let event = |t: f64, name: &str| AnimationEvent {
            t,
            name: name.to_string(),
        };
        BlendSpaceData {
            name: "loco".into(),
            two_d: false,
            triangles: Vec::new(),
            points: vec![BlendPointData {
                name: "run".into(),
                clip: 0,
                at: [0.0, 0.0],
                duration: 1.0,
                speed: 1.0,
                sync,
                events: vec![event(0.25, "foot_l"), event(0.75, "foot_r")],
            }],
        }
    }

    fn clock(prev: f64, phase: f64) -> BlendClock {
        BlendClock {
            blend: "loco".into(),
            started: FrameIndex::ZERO,
            reset: FrameIndex::ZERO,
            phase,
            prev,
            ..BlendClock::default()
        }
    }

    fn fired(blend: &BlendSpaceData, clock: &BlendClock) -> Vec<String> {
        let mut out = Vec::new();
        blend_crossings(blend, [0.0; 2], (clock.prev, clock.phase), &mut out);
        out
    }

    #[test]
    fn a_synced_event_sits_at_phase_zero() {
        let blend = blend(0.25);
        assert_eq!(fired(&blend, &clock(0.9, 1.1)), ["foot_l"]);
        // A restart parks `prev` at -1: the window includes phase 0.
        assert_eq!(fired(&blend, &clock(-1.0, 0.0)), ["foot_l"]);
        assert_eq!(fired(&blend, &clock(0.0, 0.0)), Vec::<String>::new());
    }

    #[test]
    fn crossings_tile_without_repeats() {
        let blend = blend(0.0);
        let mut all = Vec::new();
        let mut prev = 0.0;
        for step in 1..=40 {
            let phase = f64::from(step) * 0.1;
            all.extend(fired(&blend, &clock(prev, phase)));
            prev = phase;
        }
        assert_eq!(all.len(), 8);
        assert!(all.chunks(2).all(|pair| pair == ["foot_l", "foot_r"]));
    }

    #[test]
    fn a_window_spanning_many_cycles_visits_the_latest() {
        let blend = blend(0.0);
        let out = fired(&blend, &clock(0.0, 1.0e6));
        assert_eq!(out.len(), 2 * MAX_CYCLES_PER_TICK as usize);
    }
}
