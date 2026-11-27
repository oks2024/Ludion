//! Animation events: the per-tick `(entity, clip, event)` crossings a game's logic reacts to
//! (`Plan/animation-clips.md` decisions 11, 14 and 15).
//!
//! A clip's authored events (an assembly or prefab clip's own, a character's effective events with its
//! speed overlay) are named instants inside the clip. [`animation_events`] is a sim system a game
//! registers in its own schedule, after whatever writes `Animation` and before whatever reads the
//! result, the [`play_patterns`](crate::play_patterns) precedent. Each tick it rewrites the
//! [`AnimationEvents`] resource with everything the clock crossed. A scene whose game never
//! registers it is untouched.
//!
//! **The crossing rule.** Clip time is the unwrapped `T(n)` the renderer evaluates
//! (`clip_elapsed`, so a [`set_clip_speed`](crate::set_clip_speed) anchor counts) times the binding speed,
//! read at gameplay time. A walk fires what lies in `(T_prev, T(n)]`, and an event at `t = 0` is in
//! the first window, which is closed at zero. A `started` in the future fires nothing, and speed `0`
//! fires nothing after the start. A one-shot fires each event once, then an implicit `end` event
//! when `T` reaches its duration (after any authored event at that instant); a loop fires each
//! event once per cycle, several times in one tick when a tick spans cycles, and has no `end`. A
//! tick walks at most [`MAX_CYCLES_PER_WALK`] (64) loop cycles, the latest, so an absurd speed or
//! anchor never panics or stalls. `end` is a reserved name (`L1198` on an authored one).
//!
//! **Where a walk starts.** `T_prev` is, in order: the clip time the last walk reached
//! (`AnimationWalk`, when it ran last tick for the same `(entity, clip, started)`), so a speed
//! change before or after a walk, a raw write to `speed` included, can neither skip nor repeat (a
//! lowered speed fires nothing until clip time passes the point already walked); or, for a clip
//! started this tick or stamped after last tick's walk, nothing (window `[0, T(n)]`); or `T(n−1)`
//! from the clock, for a clip already running when first seen (a load, a switch, a restore from an
//! older snapshot). So a clip a later-registered system starts (the react-on-`end` pattern) fires
//! its `t = 0` event one tick late, with its first tick of clip time, and an earlier-registered one
//! on the start tick; never lost, never twice. The record rides a snapshot. The one exception: a
//! snapshot with no record (older or hand-built) taken on an earlier-registered starter's start tick
//! restores as "already running", so that clip's `t = 0` event fires again after the restore.
//!
//! **Crossfade.** Only the incoming `clip` emits. The outgoing `from` side never does, its `end`
//! included: [`crossfade`](crate::crossfade) has already handed the choice to the new clip, so a
//! fading-out run's footsteps would double the incoming clip's and a stale `end` would release a
//! choice the game has already made.
//!
//! **Layers.** A skinned character's, voxel assembly's or prefab instance root's `AnimationLayers` layer with
//! `events: true` fires its clip's events too, only while its weight at the gameplay instant is above 0, with `ClipEvent.layer` set to
//! the layer's name (base events carry an empty `layer`) and `clip` to the layer's clip or blend. A
//! layer keeps no walk record: a plain clip is walked statelessly over `(T(i − step), T(i)]`, `T`
//! being the layer's clock times the binding speed, `i` the exact gameplay instant and `step` this
//! tick's gameplay step, so the windows tile exactly under a scaled clock. The window is `[0, T(i)]`
//! when the layer's `started` lies after `floor(i − step)`; a layer a system registered after
//! `animation_events` starts therefore loses only an event at exactly `t = 0`, so register
//! `play_layer` callers before it. A raw `speed` write mid-clip may skip or repeat one crossing that
//! tick. A layer naming a blend fires its dominant point's events over its `BlendClock.layers` window,
//! on a character, an assembly or a prefab root.
//! An entity's base events come first, then its layers' in list order.
//!
//! **Held frames.** A frame the game held crosses nothing: the resource is cleared and the walk
//! skipped, so an event is never delivered twice for one gameplay frame.
//!
//! **Order.** Entities in ascending id order, then crossing time, then authored order, then `end`.
//! The walk is sequential, so the log is thread-count invariant by construction.
//!
//! **Prefab instances.** An `Animation` on the root part of a prefab instance (`PrefabInstance.root`
//! is the entity itself) plays the prefab's `clips:` and `blends:`, and fires their events: base
//! blend events from its `BlendClock`, and its `events: true` layers', with or without a base clip.
//! A root that also has an assembly or character `Mesh` uses the mesh's model. The root need not
//! carry a `Mesh`.
//!
//! **Where timing comes from.** No sim-side resource holds assembly, character or prefab data (the
//! asset cache is render-only), so [`AnimationLibrary`] loads each model once, lazily, through the same
//! validated loaders extraction uses and keeps only the clip timing. Like a
//! [`DocStore`](crate::DocStore) it is engine configuration derived from committed content, never
//! hashed, and rebuilt from disk at every load, restore and switch. **`asset.reload` does not
//! invalidate it**: a retimed clip reaches events at the next load, restore or switch, which keeps
//! a mid-session reload replay-safe. A model that fails to load still emits nothing, but raises
//! `L1210` once per entity naming the model; `check-scene` and scene load already report a broken
//! document.
//!
//! A runtime `L1209` rides the same walk: a `Sprite` drawn from an atlas whose `Animation` carries a
//! `fade` hard-cuts, so the fade would silently do nothing.

use ludion_assets::AssetRoot;
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use ludion_core::codes::code;
use ludion_core::diagnostics::Diagnostic;
use ludion_core::ids::EntityId;
use ludion_core::time::{FrameIndex, GameTime};
use ludion_ecs::{SystemCtx, World};
use ludion_scene::{AnimationEvent, Keyframe, ROOT_TRACK};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::blend_clock::blend_crossings;
use crate::blend_space::BlendSpaceData;
use crate::components::{
    Animation, AnimationLayers, BlendClock, Mesh, MeshSource, Sprite, layer_weight_at,
};
use crate::extract::{clock_elapsed, is_prefab_player};
use crate::prefab::PrefabInstance;
use crate::sequences::bound::{self, entity_clock};
use ludion_components::animation_layers::MAX_ANIMATION_LAYERS;

/// The reserved name of the implicit event a one-shot clip fires when it finishes.
const END: &str = "end";

/// The most loop cycles one walk visits, the latest ones: a tick that spans more (an absurd speed
/// or anchor) neither stalls nor overflows the cycle arithmetic.
const MAX_CYCLES_PER_WALK: i64 = 64;

/// One clip's timing as [`animation_events`] reads it.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct ClipTiming {
    /// The clip length in seconds.
    pub duration: f64,
    /// Whether the clip loops.
    pub looped: bool,
    /// The binding's playback speed multiplier (`1.0` for an assembly clip).
    pub speed: f64,
    /// The clip's effective events, time-ascending.
    pub events: Vec<AnimationEvent>,
}

/// One clip's `$root` track as [`root_motion_delta`](crate::root_motion_delta) reads it.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct RootClip {
    /// The clip length in seconds.
    pub duration: f64,
    /// Whether the clip loops.
    pub looped: bool,
    /// The `$root` translation channel (the model's units; `v[0]` and `v[2]` are read).
    pub translation: Vec<Keyframe>,
    /// The `$root` rotation channel (Euler degrees; `v[1]` is read).
    pub rotation: Vec<Keyframe>,
    /// The binding's playback speed multiplier on a plain clip's time (`1.0` for an assembly or
    /// prefab clip). A blend point's is already inside the blend's rate.
    pub speed: f64,
}

/// A model's clips that carry a `$root` track, keyed by clip short name.
pub(crate) type RootClips = BTreeMap<String, RootClip>;

/// What `root_motion_delta` reads of one model: its `$root` clips and the blend spaces that may
/// weight them.
#[derive(Debug, Default)]
pub(crate) struct RootModel {
    pub clips: RootClips,
    pub blends: Vec<BlendSpaceData>,
}

/// The per-model clip timings [`animation_events`] reads, loaded lazily from `root`. Engine
/// configuration installed at every world rebuild, never hashed; see the module docs.
pub(crate) struct AnimationLibrary {
    root: PathBuf,
    /// Keyed by model kind and path; `Err` holds the loader's diagnostics for a model that failed
    /// to load.
    models: BTreeMap<Model, Result<BTreeMap<String, ClipTiming>, Vec<Diagnostic>>>,
    /// The blend spaces of each loaded model (`blend_clock`).
    blends: BTreeMap<Model, Vec<BlendSpaceData>>,
    /// The [`RootModel`] of each model. Interior-mutable so the sim read `root_motion_delta` works
    /// through `&World`; the lock is held only to get or insert.
    roots: Mutex<BTreeMap<Model, Arc<RootModel>>>,
}

/// A model that binds clips, with its path: the `Mesh.shape` of an assembly or character, or an
/// instanced prefab.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum Model {
    Assembly(String),
    Character(String),
    Prefab(String),
}

impl Model {
    fn path(&self) -> &str {
        match self {
            Model::Assembly(rel) | Model::Character(rel) | Model::Prefab(rel) => rel,
        }
    }
}

impl AnimationLibrary {
    /// An empty library resolving model paths against `root`.
    pub(crate) fn new(root: PathBuf) -> Self {
        AnimationLibrary {
            root,
            models: BTreeMap::new(),
            blends: BTreeMap::new(),
            roots: Mutex::new(BTreeMap::new()),
        }
    }

    /// The [`RootModel`] of `model`, loaded on first use (or already filled by
    /// [`Self::timings`]); empty for a model that fails to load, which `animation_events` reports.
    pub(crate) fn root_model(&self, model: &Model) -> Arc<RootModel> {
        let lock = || self.roots.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(found) = lock().get(model) {
            return Arc::clone(found);
        }
        let root = AssetRoot::content(&self.root);
        let loaded = match model {
            Model::Assembly(rel) => {
                crate::assembly_mesh::load(root, rel).map(|data| root_model_of_assembly(&data))
            }
            Model::Character(rel) => {
                crate::character::load(root, rel).map(|data| root_model_of_character(&data))
            }
            Model::Prefab(rel) => crate::prefab_clips::load_with_sources(root, rel)
                .map(|(data, _)| root_model_of_prefab(&data)),
        };
        let loaded = Arc::new(loaded.unwrap_or_default());
        Arc::clone(lock().entry(model.clone()).or_insert(loaded))
    }

    fn fill_root(&self, model: &Model, root: RootModel) {
        self.roots
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(model.clone(), Arc::new(root));
    }

    /// The clip timings of `model`, loading it on first use, or the loader's diagnostics for a
    /// model that failed to load.
    fn timings(&mut self, model: &Model) -> Result<&BTreeMap<String, ClipTiming>, &[Diagnostic]> {
        if !self.models.contains_key(model) {
            let rel = model.path();
            let timing = |name: &str,
                          clip: &ludion_scene::AnimationDocument,
                          speed,
                          events: &[AnimationEvent]| {
                (
                    name.to_string(),
                    ClipTiming {
                        duration: clip.duration,
                        looped: clip.looped,
                        speed,
                        events: events.to_vec(),
                    },
                )
            };
            let loaded = match model {
                Model::Assembly(_) => {
                    crate::assembly_mesh::load(AssetRoot::content(&self.root), rel).map(|data| {
                        self.blends.insert(model.clone(), data.blends.clone());
                        self.fill_root(model, root_model_of_assembly(&data));
                        data.clips
                            .iter()
                            .map(|b| timing(&b.name, &b.clip, 1.0, &b.clip.events))
                            .collect()
                    })
                }
                Model::Character(_) => crate::character::load(AssetRoot::content(&self.root), rel)
                    .map(|data| {
                        self.blends.insert(model.clone(), data.blends.clone());
                        self.fill_root(model, root_model_of_character(&data));
                        data.clips
                            .iter()
                            .map(|b| timing(&b.name, &b.clip, b.speed, &b.events))
                            .collect()
                    }),
                Model::Prefab(_) => {
                    crate::prefab_clips::load_with_sources(AssetRoot::content(&self.root), rel).map(
                        |(data, _)| {
                            self.blends.insert(model.clone(), data.blends.clone());
                            self.fill_root(model, root_model_of_prefab(&data));
                            data.clips
                                .iter()
                                .map(|b| timing(&b.name, &b.clip, 1.0, &b.clip.events))
                                .collect()
                        },
                    )
                }
            };
            self.models.insert(model.clone(), loaded);
        }
        match &self.models[model] {
            Ok(timings) => Ok(timings),
            Err(error) => Err(error),
        }
    }
}

/// `clip`'s `$root` track, if it carries one.
pub(crate) fn root_clip(clip: &ludion_scene::AnimationDocument) -> Option<RootClip> {
    let track = ludion_scene::clip_track(clip, ROOT_TRACK)?;
    Some(RootClip {
        duration: clip.duration,
        looped: clip.looped,
        translation: track.translation.clone(),
        rotation: track.rotation.clone(),
        speed: 1.0,
    })
}

fn root_clips<'a>(
    clips: impl Iterator<Item = (&'a String, &'a ludion_scene::AnimationDocument, f64)>,
) -> RootClips {
    clips
        .filter_map(|(name, clip, speed)| {
            let rooted = root_clip(clip)?;
            Some((name.clone(), RootClip { speed, ..rooted }))
        })
        .collect()
}

fn root_model_of_assembly(data: &crate::assembly_mesh::AssemblyData) -> RootModel {
    RootModel {
        clips: root_clips(data.clips.iter().map(|b| (&b.name, &b.clip, 1.0))),
        blends: data.blends.clone(),
    }
}

fn root_model_of_character(data: &crate::character::CharacterData) -> RootModel {
    RootModel {
        clips: root_clips(data.clips.iter().map(|b| (&b.name, &b.clip, b.speed))),
        blends: data.blends.clone(),
    }
}

/// Root motion reads a prefab's plain clips only, so its blend list is left empty.
fn root_model_of_prefab(data: &crate::prefab_clips::PrefabClipData) -> RootModel {
    RootModel {
        clips: root_clips(data.clips.iter().map(|b| (&b.name, &b.clip, 1.0))),
        blends: Vec::new(),
    }
}

impl AnimationLibrary {
    /// The blend spaces of `model`, loading it on first use; empty for a model that failed to load
    /// or defines none.
    pub(crate) fn blends(&mut self, model: &Model) -> &[BlendSpaceData] {
        let _ = self.timings(model);
        self.blends.get(model).map_or(&[], Vec::as_slice)
    }
}

/// Where the last [`animation_events`] walk stopped, per playing clip, so the next walk resumes
/// exactly there (module docs, "Where a walk starts"). A world resource, never hashed; it rides a
/// snapshot so a restore resumes the same walk.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub(crate) struct AnimationWalk {
    /// The gameplay tick the walk ran at.
    pub tick: u64,
    /// The clips walked, in ascending entity order.
    pub clips: Vec<WalkedClip>,
}

/// One clip [`AnimationWalk`] reached: `t` is the binding-scaled clip time the walk ended at.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub(crate) struct WalkedClip {
    pub entity: EntityId,
    pub clip: String,
    pub started: FrameIndex,
    pub t: f64,
}

/// One fired event: `name` crossed on `clip`, the clip `entity` is playing.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ClipEvent {
    /// The entity playing the clip.
    pub entity: EntityId,
    /// The clip's short name, as in `Animation.clip`.
    pub clip: String,
    /// The event's authored name, or `end` for a one-shot's implicit end.
    pub name: String,
    /// The `AnimationLayers` layer that fired it; empty for the base clip. `clip` then names the
    /// layer's clip or blend.
    pub layer: String,
}

/// Every [`ClipEvent`] the last [`animation_events`] run fired, in deterministic order (see the
/// module docs). Rewritten on every run: a consumer reads it in the same tick, after that system.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct AnimationEvents {
    pub(crate) fired: Vec<ClipEvent>,
}

impl AnimationEvents {
    /// Appends one event after the run's own: how `paired_update` adds its `paired:<action>` events.
    pub(crate) fn push(&mut self, event: ClipEvent) {
        self.fired.push(event);
    }

    /// The events fired this tick, in firing order.
    pub fn iter(&self) -> impl Iterator<Item = &ClipEvent> {
        self.fired.iter()
    }

    /// Whether `name` fired on `entity`'s `clip` this tick.
    pub fn fired(&self, entity: EntityId, clip: &str, name: &str) -> bool {
        self.fired
            .iter()
            .any(|event| event.entity == entity && event.clip == clip && event.name == name)
    }
}

/// Rewrites the [`AnimationEvents`] resource with every event the gameplay clock crossed this tick
/// on an entity playing a clip of its `Mesh.shape` assembly or character (the crossing rule,
/// crossfade and held-frame behavior are in the module docs), and raises `L1209` for a sprite
/// crossfade, and `L1210` for a model that failed to load.
///
/// A game registers it in its own schedule, after the systems that write `Animation` and before the
/// ones that read the events, and declares its reads:
///
/// ```
/// use ludion_engine::{
///     Animation, AnimationEvents, Engine, Mesh, PrefabInstance, Sprite, animation_events,
/// };
/// use ludion_protocol::{Command, command};
///
/// let mut engine = Engine::builder()
///     .system("animation_events", animation_events)
///     .query::<(&Animation, &Mesh)>()
///     .query::<(&Animation, &Sprite)>()
///     .query::<(&Animation, &PrefabInstance)>()
///     .build()
///     .unwrap();
/// engine
///     .execute(Command::SimStep(command::SimStepParams { ticks: 1, hash: false }))
///     .unwrap();
/// // A consumer registered after the system reads what fired this tick.
/// let events = engine.world().resource::<AnimationEvents>().unwrap();
/// assert_eq!(events.iter().count(), 0);
/// ```
///
/// Reading a specific entity's event is `events.fired(entity, "attack", "end")`.
pub fn animation_events(world: &mut World, ctx: &SystemCtx) {
    let mut fired = Vec::new();
    if ctx.game_advanced() || bound::any_bound(world) {
        walk(world, ctx, &mut fired);
    }
    match world.resource_mut::<AnimationEvents>() {
        Some(events) => events.fired = fired,
        None => {
            world.insert_resource(AnimationEvents { fired });
        }
    }
}

fn walk(world: &mut World, ctx: &SystemCtx, fired: &mut Vec<ClipEvent>) {
    let sprite_fades: Vec<EntityId> = world
        .query_ids::<(&Animation, &Sprite)>()
        .filter(|(_, (animation, sprite))| sprite.atlas.is_some() && animation.fade > 0.0)
        .map(|(id, _)| id)
        .collect();
    for id in sprite_fades {
        let key = format!(
            "{}/Animation/fade",
            crate::diagnostics::offender_key(world, id)
        );
        crate::diagnostics::report(
            world,
            code::ANIMATION_SPRITE_CROSSFADE,
            key,
            format!(
                "a sprite flipbook cannot crossfade: `Animation.fade` on {id} is ignored and the \
                 sprite hard-cuts to its `clip`"
            ),
        );
    }

    // Both resources come out of the world for the walk, so the query can iterate the world
    // directly: no per-tick copy of the record, the shapes or the clip names.
    let Some(mut library) = world.remove_resource::<AnimationLibrary>() else {
        return;
    };
    let mut record = world.remove_resource::<AnimationWalk>();

    // The record's tick is the world's gameplay tick; each entity walks on its own clock.
    let record_tick = ctx.game_instant.ticks_floor();
    let mut failures = Vec::new();
    let mut walked = Vec::new();
    // Entities in ascending id order: a mesh model, else the instance's prefab for a root part.
    let mut playing: BTreeMap<EntityId, (Animation, Model)> = BTreeMap::new();
    // An entity with no base clip still walks its event-firing layers.
    let mut baseless = Vec::new();
    let roots: Vec<(EntityId, Animation, PrefabInstance)> = world
        .query_ids::<(&Animation, &PrefabInstance)>()
        .filter(|(id, (_, instance))| instance.root == *id)
        .map(|(id, (animation, instance))| (id, animation.clone(), instance.clone()))
        .collect();
    for (id, animation, instance) in roots {
        if is_prefab_player(world, id, &instance) {
            let model = Model::Prefab(instance.source);
            if animation.clip.is_empty() {
                baseless.push((id, animation, model));
            } else {
                playing.insert(id, (animation, model));
            }
        }
    }
    for (id, (animation, mesh)) in world.query_ids::<(&Animation, &Mesh)>() {
        let model = match &mesh.shape {
            MeshSource::Assembly(rel) => Model::Assembly(rel.clone()),
            MeshSource::Character(rel) => Model::Character(rel.clone()),
            _ => continue,
        };
        if animation.clip.is_empty() {
            baseless.push((id, animation.clone(), model));
        } else {
            playing.insert(id, (animation.clone(), model));
        }
    }
    for (id, animation, model) in baseless {
        // Unrecorded: engine-optional components must not demand a game's query declaration.
        let layered = world
            .get_unrecorded::<AnimationLayers>(id)
            .ok()
            .flatten()
            .is_some_and(|stack| stack.layers.iter().any(|layer| layer.events));
        if layered {
            playing.insert(id, (animation, model));
        }
    }
    for (id, (animation, model)) in playing {
        let own = entity_clock(world, id, ctx);
        if !own.advanced {
            // Not walked this tick: its record stays where it stopped.
            let kept = record.as_ref().and_then(|record| {
                let index = record.clips.binary_search_by_key(&id, |c| c.entity).ok()?;
                Some(record.clips[index].clone())
            });
            walked.extend(kept);
            continue;
        }
        let (now, ticks) = (own.instant, own.frame.as_u64());
        let bound = bound::is_bound(world, id);
        let animation = &animation;
        let clip = &animation.clip;
        'base: {
            if clip.is_empty() {
                break 'base;
            }
            let timing = match library.timings(&model) {
                Ok(timings) => timings.get(clip),
                Err(error) => {
                    failures.push((id, clip.clone(), model.path().to_string(), error.to_vec()));
                    None
                }
            };
            let Some(timing) = timing else {
                if let Some(clock) = world.get_unrecorded::<BlendClock>(id).ok().flatten()
                    && let Some(blend) = library
                        .blends(&model)
                        .iter()
                        .find(|blend| blend.name == *clip)
                    && clock.blend == *clip
                {
                    let mut names = Vec::new();
                    blend_crossings(
                        blend,
                        animation.params,
                        (clock.prev, clock.phase),
                        &mut names,
                    );
                    fired.extend(names.into_iter().map(|name| ClipEvent {
                        entity: id,
                        clip: clip.clone(),
                        name,
                        layer: String::new(),
                    }));
                }
                break 'base;
            };
            let started = animation.started.as_u64();
            if ticks < started {
                break 'base;
            }
            let frame = FrameIndex::new(started);
            let (speed, anchor) = (animation.speed, animation.anchor);
            let at =
                |time: GameTime| clock_elapsed(time, frame, speed, anchor.as_ref()) * timing.speed;
            let t_now = at(now);
            let old = record.as_mut().and_then(|record| {
                let index = record.clips.binary_search_by_key(&id, |c| c.entity).ok()?;
                Some((record.tick, &mut record.clips[index]))
            });
            let resumed = old
                .as_ref()
                // A scaled clock repeats a gameplay tick across sim ticks, so the same tick resumes too.
                .filter(|(tick, last)| {
                    // A bound entity advances every tick, so its record is always the previous walk's.
                    (bound || *tick == ticks || tick + 1 == ticks)
                        && last.clip == *clip
                        && last.started == frame
                })
                .map(|(_, last)| last.t);
            let prev = match resumed {
                Some(t) => Some(t),
                // Started this tick, or stamped after last tick's walk: nothing has been walked yet.
                None if started + 1 >= ticks => None,
                // Already running when first seen (a load, a switch, a restore from an older snapshot).
                None => Some(at(GameTime::from_ticks(ticks - 1, now.rate()))),
            };
            // A raw `speed` write that lowers the speed pulls clip time back below the point already
            // walked; keeping the furthest point fires nothing until the clock passes it again.
            let reached = resumed.map_or(t_now, |p| t_now.max(p));
            let walked_clip = match old {
                Some((_, last)) if last.clip == *clip => std::mem::take(&mut last.clip),
                _ => clip.clone(),
            };
            walked.push(WalkedClip {
                entity: id,
                clip: walked_clip,
                started: frame,
                t: reached,
            });
            let mut names = Vec::new();
            crossings(timing, prev, t_now, &mut names);
            fired.extend(names.into_iter().map(|name| ClipEvent {
                entity: id,
                clip: clip.clone(),
                name: name.to_string(),
                layer: String::new(),
            }));
        }
        walk_layers(world, &mut library, ctx, id, &model, fired);
    }
    world.insert_resource(library);
    world.insert_resource(AnimationWalk {
        tick: record_tick,
        clips: walked,
    });
    for (id, clip, rel, causes) in failures {
        let diagnostic = Diagnostic::new(
            code::ANIMATION_MODEL_UNLOADED,
            format!(
                "{id} plays `{clip}` but its model `{rel}` failed to load, so it fires no \
                 animation events (`end` included)"
            ),
        );
        let diagnostic = causes
            .into_iter()
            .fold(diagnostic, Diagnostic::with_related);
        let key = format!(
            "{}/Animation/clip",
            crate::diagnostics::offender_key(world, id)
        );
        crate::diagnostics::report_full(world, key, diagnostic);
    }
}

/// Appends the events of `id`'s event-firing `AnimationLayers` layers, in list order (the module
/// docs' "Layers" paragraph).
fn walk_layers(
    world: &World,
    library: &mut AnimationLibrary,
    ctx: &SystemCtx,
    id: EntityId,
    model: &Model,
    fired: &mut Vec<ClipEvent>,
) {
    // Unrecorded, as in the baseless scan.
    let Some(stack) = world.get_unrecorded::<AnimationLayers>(id).ok().flatten() else {
        return;
    };
    let own = entity_clock(world, id, ctx);
    let (now, ticks) = (own.instant, own.frame.as_u64());
    let step_q16 = (own.scale * 65536.0) as u64;
    let prev_instant = GameTime::from_q16(now.as_q16().saturating_sub(step_q16), now.rate());
    for layer in stack.layers.iter().take(MAX_ANIMATION_LAYERS) {
        if !layer.events || layer_weight_at(layer, now) <= 0.0 {
            continue;
        }
        let mut names: Vec<String> = Vec::new();
        let timing = library
            .timings(model)
            .ok()
            .and_then(|timings| timings.get(&layer.clip));
        if let Some(timing) = timing {
            if ticks < layer.started.as_u64() {
                continue;
            }
            let at = |time: GameTime| {
                clock_elapsed(time, layer.started, layer.speed, None) * timing.speed
            };
            let prev =
                (layer.started.as_u64() <= prev_instant.ticks_floor()).then(|| at(prev_instant));
            let mut crossed = Vec::new();
            crossings(timing, prev, at(now), &mut crossed);
            names.extend(crossed.into_iter().map(str::to_string));
        } else if let Some(blend) = library.blends(model).iter().find(|b| b.name == layer.clip)
            && let Some(entry) = world
                .get_unrecorded::<BlendClock>(id)
                .ok()
                .flatten()
                .and_then(|clock| {
                    clock.layers.iter().find(|entry| {
                        entry.name == layer.name
                            && entry.blend == layer.clip
                            && entry.started == layer.started
                    })
                })
        {
            blend_crossings(blend, layer.params, (entry.prev, entry.phase), &mut names);
        }
        fired.extend(names.into_iter().map(|name| ClipEvent {
            entity: id,
            clip: layer.clip.clone(),
            name,
            layer: layer.name.clone(),
        }));
    }
}

/// The names of the events `timing` fires as clip time moves from `prev` to `now`, in firing order
/// (`end` last). `prev` is the clip time already walked: `Some(p)` is the window `(p, now]`, `None`
/// is a clip that has not been walked yet, `[0, now]` closed at zero so a `t = 0` event fires. A
/// loop visits at most [`MAX_CYCLES_PER_WALK`] cycles, the latest.
fn crossings<'a>(timing: &'a ClipTiming, prev: Option<f64>, now: f64, out: &mut Vec<&'a str>) {
    let duration = timing.duration;
    if !(duration > 0.0
        && duration.is_finite()
        && prev.is_none_or(f64::is_finite)
        && now.is_finite())
    {
        return;
    }
    let in_window = |t: f64| match prev {
        None => t >= 0.0 && t <= now,
        Some(prev) => t > prev && t <= now,
    };
    if prev.is_some_and(|prev| now <= prev) {
        return;
    }
    if timing.looped {
        // `floor` and `fl(k·d) <= now` can disagree by a cycle, so the range is widened and the
        // window test decides; windows tile exactly, so a wider range cannot fire twice.
        let lower = prev.unwrap_or(0.0);
        let first = ((lower / duration).floor() as i64).saturating_sub(1).max(0);
        let last = ((now / duration).floor() as i64).saturating_add(1);
        let first = first.max(last.saturating_sub(MAX_CYCLES_PER_WALK));
        for cycle in first..=last {
            let offset = cycle as f64 * duration;
            for event in &timing.events {
                if in_window(event.t + offset) {
                    out.push(&event.name);
                }
            }
        }
    } else {
        for event in &timing.events {
            if in_window(event.t) {
                out.push(&event.name);
            }
        }
        if in_window(duration) {
            out.push(END);
        }
    }
}

#[cfg(test)]
mod tests {
    use ludion_core::time::{FrameIndex, SimTime, TickRate};

    use super::*;

    fn timing(duration: f64, looped: bool, events: &[(f64, &str)]) -> ClipTiming {
        ClipTiming {
            duration,
            looped,
            speed: 1.0,
            events: events
                .iter()
                .map(|(t, name)| AnimationEvent {
                    t: *t,
                    name: (*name).to_string(),
                })
                .collect(),
        }
    }

    /// The names fired on `tick` of a clip started at frame 0, at `speed`, at 60 Hz.
    fn on_tick(timing: &ClipTiming, tick: u64, speed: f32) -> Vec<&str> {
        let at = |ticks| {
            clock_elapsed(
                SimTime::from_ticks(ticks, TickRate::DEFAULT),
                FrameIndex::ZERO,
                speed,
                None,
            ) * timing.speed
        };
        let mut out = Vec::new();
        let prev = (tick > 0).then(|| at(tick - 1));
        crossings(timing, prev, at(tick), &mut out);
        out
    }

    const BODY: &str = r#"format: 1
kind: assembly
name: body
size:
  - 1
  - 1
  - 1
palette:
  A:
    color:
      - 1.0
      - 0.0
      - 0.0
      - 1.0
parts:
  - name: body
    file: cell.part.yaml
    place:
      - 0
      - 0
      - 0
clips:
  walk: walk.anim.yaml
blends:
  stride:
    points:
      - clip: walk
        at: 0.0
      - clip: walk
        at: 1.0
"#;

    const CELL: &str = r#"format: 1
kind: part
size:
  - 1
  - 1
  - 1
slices:
  -
    - "A"
"#;

    const WALK: &str = r#"format: 1
kind: animation
duration: 1.0
loop: true
tracks:
  "$root":
    translation:
      - t: 0.0
        v:
          - 0.0
          - 0.0
          - 0.0
      - t: 1.0
        v:
          - 0.0
          - 0.0
          - -8.0
"#;

    #[test]
    fn the_root_model_is_cached_and_prefilled_by_timings() {
        let dir = std::env::temp_dir().join(format!("ludion-root-model-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let files = [
            ("body.assembly.yaml", BODY),
            ("cell.part.yaml", CELL),
            ("walk.anim.yaml", WALK),
        ];
        for (name, text) in files {
            std::fs::write(dir.join(name), text).unwrap();
        }

        let lazy = AnimationLibrary::new(dir.clone());
        let model = Model::Assembly("body.assembly.yaml".into());
        let first = lazy.root_model(&model);
        assert_eq!((first.clips.len(), first.blends.len()), (1, 1));
        assert!(Arc::ptr_eq(&first, &lazy.root_model(&model)));

        let mut prefilled = AnimationLibrary::new(dir.clone());
        assert!(prefilled.timings(&model).is_ok());
        let skinned = Model::Character(skinned_files(&dir));
        assert!(prefilled.timings(&skinned).is_ok(), "the skinned fixture");
        // With the files gone, only the pre-fill can still answer.
        std::fs::remove_dir_all(&dir).unwrap();
        let filled = prefilled.root_model(&model);
        assert_eq!((filled.clips.len(), filled.blends.len()), (1, 1));
        let filled = prefilled.root_model(&skinned);
        assert_eq!(filled.clips["walk"].speed, 2.0);
    }

    /// A one-joint skinned assembly binding the `$root` walk at speed 2, written beside it.
    fn skinned_files(dir: &std::path::Path) -> String {
        let mesh = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../examples/gallery/meshes/skin-mannequin.mesh.yaml");
        std::fs::copy(mesh, dir.join("skin.mesh.yaml")).unwrap();
        std::fs::write(
            dir.join("skin.assembly.yaml"),
            "format: 1\nkind: assembly\nmesh: skin.mesh.yaml\nclips:\n  walk:\n    clip: walk.anim.yaml\n    speed: 2.0\n",
        )
        .unwrap();
        "skin.assembly.yaml".into()
    }

    #[test]
    fn an_event_at_zero_fires_on_the_start_tick_only() {
        let clip = timing(1.0, true, &[(0.0, "go")]);
        assert_eq!(on_tick(&clip, 0, 1.0), ["go"]);
        assert!((1..30).all(|tick| on_tick(&clip, tick, 1.0).is_empty()));
    }

    #[test]
    fn a_one_shot_fires_end_once_after_the_authored_events() {
        let clip = timing(0.5, false, &[(0.25, "hit"), (0.5, "late")]);
        let fired: Vec<Vec<&str>> = (0..90).map(|tick| on_tick(&clip, tick, 1.0)).collect();
        assert_eq!(fired[15], ["hit"]);
        assert_eq!(fired[30], ["late", "end"]);
        let total: usize = fired.iter().map(Vec::len).sum();
        assert_eq!(total, 3);
    }

    #[test]
    fn a_loop_fires_each_event_once_per_cycle_and_no_end() {
        let clip = timing(0.5, true, &[(0.25, "step")]);
        let fired: usize = (0..120).map(|tick| on_tick(&clip, tick, 1.0).len()).sum();
        assert_eq!(fired, 4);
        assert!((0..120).all(|tick| !on_tick(&clip, tick, 1.0).contains(&"end")));
    }

    #[test]
    fn a_tick_spanning_two_cycles_fires_both() {
        let clip = timing(0.1, true, &[(0.05, "step")]);
        // Speed 20 at 60 Hz: one tick is 1/3 s, more than three 0.1 s cycles.
        let fired = on_tick(&clip, 1, 20.0);
        assert_eq!(fired, ["step", "step", "step"]);
    }

    #[test]
    fn speed_zero_fires_nothing_after_the_start() {
        let clip = timing(1.0, false, &[(0.0, "go"), (0.5, "mid")]);
        assert_eq!(on_tick(&clip, 0, 0.0), ["go"]);
        assert!((1..120).all(|tick| on_tick(&clip, tick, 0.0).is_empty()));
    }

    #[test]
    fn a_035_second_one_shot_ends_on_tick_21_at_60_hz() {
        let clip = timing(0.35, false, &[]);
        assert!(on_tick(&clip, 20, 1.0).is_empty());
        assert_eq!(on_tick(&clip, 21, 1.0), ["end"]);
        assert!((22..90).all(|tick| on_tick(&clip, tick, 1.0).is_empty()));
    }

    /// A loop's `t: 0` event fires once per cycle at the tick an exact integer oracle names, for
    /// every duration in 5 ms steps and four speeds: float rounding at a wrap never drops a cycle or
    /// fires one twice.
    #[test]
    fn a_loops_zero_event_fires_once_per_cycle_for_every_rounding() {
        let mut failures = Vec::new();
        for m in 10u64..=400 {
            for a in 1u64..=4 {
                let clip = timing(m as f64 / 200.0, true, &[(0.0, "go")]);
                let speed = a as f32 / 2.0;
                let ticks = 3000u64;
                let per_tick: Vec<usize> = (0..ticks)
                    .map(|tick| on_tick(&clip, tick, speed).len())
                    .collect();
                let fires: Vec<u64> = (0..ticks).filter(|&t| per_tick[t as usize] > 0).collect();
                let total: usize = per_tick.iter().sum();
                // Cycle `k` is first reached at the smallest `n` with `5·a·n >= 3·k·m`; on an exact
                // tie the float may land a tick late.
                let oracle = |k: u64| {
                    let n = (3 * k * m).div_ceil(5 * a);
                    (n, 5 * a * n == 3 * k * m && k > 0)
                };
                let last_cycle = (5 * a * (ticks - 1)) / (3 * m);

                let (mut sure, mut maybe) = (0, 0);
                let mut misplaced = None;
                for (next, k) in (0..=last_cycle).enumerate() {
                    let (n, tie) = oracle(k);
                    maybe += 1;
                    if n + 1 < ticks {
                        sure += 1;
                    }
                    let ok = fires
                        .get(next)
                        .is_some_and(|&f| f == n || (tie && f == n + 1));
                    if !ok && n + 1 < ticks {
                        misplaced = Some((k, n, fires.get(next).copied()));
                        break;
                    }
                }
                let doubled = per_tick.iter().any(|&count| count > 1);
                if misplaced.is_some() || doubled || !(sure..=maybe).contains(&total) {
                    failures.push((m, a, misplaced, doubled, total, sure));
                }
            }
        }
        assert!(
            failures.is_empty(),
            "{} cases misfired (m, a, misplaced (cycle, oracle tick, got), doubled, total, sure): {:?}",
            failures.len(),
            &failures[..failures.len().min(5)]
        );
    }

    /// Rust-built values skip validation, so a huge speed or anchor reaches `crossings`: it must
    /// neither overflow the cycle arithmetic nor walk billions of cycles.
    #[test]
    fn a_pathological_speed_or_anchor_neither_panics_nor_stalls() {
        let clip = timing(0.5, true, &[(0.1, "a"), (0.3, "b")]);
        let bound = (MAX_CYCLES_PER_WALK as usize + 1) * 2;
        for (prev, now) in [
            (Some(-1e30), 1e30),
            (Some(0.0), f64::MAX),
            (Some(1e300), 1.5e300),
            (Some(-1e30), -1e29),
        ] {
            let mut out = Vec::new();
            crossings(&clip, prev, now, &mut out);
            assert!(out.len() <= bound, "{prev:?} -> {now}: {}", out.len());
        }
        for speed in [1e21, f32::MAX, 1e9] {
            for tick in 0..4 {
                let fired = on_tick(&clip, tick, speed);
                assert!(fired.len() <= bound, "speed {speed}, tick {tick}");
            }
        }
    }

    #[test]
    fn a_degenerate_duration_fires_nothing() {
        for duration in [0.0, -1.0, f64::NAN, f64::INFINITY] {
            let clip = timing(duration, false, &[(0.0, "go")]);
            assert!(on_tick(&clip, 0, 1.0).is_empty());
        }
    }
}
