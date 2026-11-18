//! Everything a game needs, in one glob import: `use ludion::prelude::*;`.
//!
//! What is here, and what is deliberately not:
//!
//! - **Engine construction** — [`Engine`], [`EngineBuilder`], and [`builtin_registry`] (build a
//!   game's registry on top of the built-ins).
//! - **Game data-docs** — [`DocStore`], the resource a system reads registered `kind:` docs from
//!   (`world.resource::<DocStore<T>>()`); register a kind with `EngineBuilder::doc_kind::<T>`.
//!   For tests and tools, [`parse_doc`] reads a doc into its type with the boot loader's checks,
//!   [`check_doc`]/[`emit_doc`] are the lower-level validate and canonical-write steps, and
//!   [`code`] names the `L####` diagnostic codes.
//! - **Player profile** — [`Profile`], the persistent save-state resource a game registers with
//!   `EngineBuilder::profile::<P>` and mutates via `world.resource_mut::<Profile<P>>()`; call
//!   `profile.save()` to request persistence.
//! - **Checkpoints** — [`Checkpoints`] (`EngineBuilder::checkpoints`): `capture`/`restore`/`discard`
//!   a named in-memory slot at the frame boundary; the frame, input, profile and recording continue.
//! - **Suspended run** — [`RunSave`] (`EngineBuilder::run_save`): `suspend()` writes the running
//!   game to `save/suspend.json`; `pending()` reports [`SuspendInfo`] read at boot, stamped with a
//!   [`BuildStamp`]; `resume()` restores it in place (strict, held input cleared) and
//!   [`ResumeOutcome`] reports the result.
//! - **Run recording** — [`RunRecording`] (`EngineBuilder::run_recordings`): `start(name)` restarts
//!   the run from the scene's text and records it to `<dir>/<name>.lrp`; `stop()` publishes it.
//! - **Replay playback** — [`ReplayPlayback`] (`EngineBuilder::replay_playback`): `play(name, return_to)`
//!   plays a recording in place and returns to a scene; `refresh()` / `delete(name)` manage the
//!   [`ReplayEntry`] list; [`PlaybackOutcome`] reports how it ended. While one plays, live input
//!   goes to a [`PlaybackInput`] resource (presentation systems only) and
//!   `EngineBuilder::playback_controls` maps keys to [`PlaybackControls`] (pause, [`PlaybackSpeed`], stop).
//! - **Built-in visual components** — [`Transform`], [`Sprite`] (+ [`SpriteBlend`], its `alpha` /
//!   `additive` compositing flag), [`Mesh`] (+ [`MeshSource`]), [`PointLight`],
//!   [`SpotLight`], [`ReflectionProbe`] (+ [`ProbeShape`]), [`DirectionalLight`] (the sun), [`Camera`] (+ [`Projection`]), [`Text`],
//!   [`Tilemap`] (+ [`TileAppearance`]), [`MeshGrid`], [`GroundMarker`] (+ [`GroundMarkerShape`], [`GroundMarkerStyle`]) — the flat
//!   ground-anchored telegraph a ranged or area attack draws under its target — [`DropShadow`],
//!   the projected shadow under a character, and [`Decal`] (+ [`DecalMap`], [`DecalBlend`],
//!   [`DecalReceiverSet`], [`MapWrap`]), the projected box that paints textures onto surfaces. A scene mixes these
//!   with game components.
//! - **Debug draw** — [`debug_draw`]`(world, "motor")` returns a [`DebugPen`] that draws world-space
//!   lines under a named category; the host enables categories ([`DebugCategories`]), and the
//!   drawing is never hashed, saved or recorded.
//! - **Animated voxel assemblies** — [`Animation`]: which clip of a `Mesh.shape: assembly` model an
//!   entity is playing, and since when. A movement system writes `clip: "walk"`, a combat system
//!   `clip: "attack"`; playback is visual-only (the pose is evaluated at render time from the tick
//!   count), so the sim, the world hash, and a replay are untouched by it. Write it only when the
//!   *choice* changes — re-stamping `started` every frame restarts the clip every frame.
//! - **Sky & lighting environment** — [`Environment`] (an opt-in singleton: authored-gradient sky,
//!   flat ambient, and the bloom/IBL knobs) and [`Fade`] (a per-instance screen-door dither for
//!   occlusion plumbing). A scene with no `Environment` keeps the engine's historical pipeline.
//!   [`PostProcess`] is the singleton post chain — bloom, exposure, tonemap, grade, fog. Authored in
//!   scene text and writable at runtime, so a game can drive a screen effect from sim state.
//!   [`GiDynamic`] is the field-less marker a game inserts on every entity a system moves,
//!   recolours or spawns, so its motion reaches neither the GI bake's cache key nor the
//!   voxelization's — the one GI component a game writes from code (`GiVolume` is authored in
//!   scene text and needs no Rust name).
//! - **Screen-space UI** — [`UiRect`] (a colored/textured screen-space quad), [`UiText`] (screen-
//!   space bitmap text), and [`UiList`] (+ [`UiListItem`]) — the selectable menu: a titled stack of
//!   text choices with one cursor, its own auto-sized panel, and one line per item. Each is anchored
//!   to one of nine canvas points by [`Anchor`]. Resolution-independent HUD/menu geometry, distinct
//!   from world-space `Text`. A game drives the cursor itself: build a [`UiNav`] (usually
//!   `UiNav::from_input`) and call `UiList::navigate`, which returns the activated index — exactly
//!   one list per frame, whichever one the game's phase state says is up. See
//!   [`math::resolve_ui_rect_of`] for the matching mouse-hit-test helper (or [`math::list_hit`] for a
//!   `UiList`), and the engine guide's
//!   "Screen-space UI" chapter (`ludion docs screen-ui`) and its `UiList` spoke (`ludion docs ui-list`).
//! - **ECS surface** — [`World`] (and its `query`/`get`/`spawn` methods), [`Registry`], [`Schedule`],
//!   [`SystemCtx`], [`Parent`], and the [`Component`] marker. A [`Transform`] is local under a
//!   [`Parent`]: [`world_transform`] reads the composed [`WorldPose`] (never stored, never hashed)
//!   and [`reparent`] (`world, child, parent, `[`Keep::World`] or [`Keep::Local`]) re-points it
//!   (`ludion docs hierarchy`). [`spawn_prefab`] spawns a `kind: prefab` document from a system
//!   ([`prepare_prefab`] reads one ahead, spawning nothing; [`prewarm_prefab`] also keeps its GPU assets resident), each part carrying a [`PrefabInstance`]; [`spawn_map`] stamps a `kind: map` the same way, with a
//!   [`MapSpawn`] kit, seed and [`Mirror`] ([`prepare_map`] compiles one ahead, spawning nothing); [`despawn_instance`] removes one by its root; [`revert_prefab`] and [`unpack_prefab`] edit one.
//!   [`SingleExt`] adds `world.single::<T>()` / `world.single_get::<T>()`: the first entity carrying a
//!   singleton component, and its value. [`Phase`] is the phase a system runs in, which the world
//!   enforces the sim/presentation boundary by; [`SemanticValidate`] is the trait a component
//!   implements for cross-field invariants JSON Schema cannot express.
//! - **Sim/input/time** — [`InputState`], [`Key`], [`InputEvent`]; the mouse/gamepad devices
//!   [`MouseState`]/[`GamepadState`] (reached via `input.mouse()`/`input.gamepad()`) and their
//!   name enums [`MouseButton`], [`GamepadButton`], [`GamepadAxis`]; the const-constructible key-or-button
//!   binding [`Action`], the data-driven action map [`Actions`] read by [`ActionName`]
//!   (`Engine::builder().input_actions()`, `ludion docs input`) and the standard radial stick deadzone [`STICK_DEADZONE`] and the trigger press level [`TRIGGER_THRESHOLD`]; [`SimTime`], [`FrameIndex`],
//!   [`TickRate`]; [`DeterministicRng`] and [`WorldSeed`] are the seeded randomness a system draws from. [`GameClock`] is the gameplay half of that time: the resource a game calls
//!   `hold()` on every frame it is frozen (`world.resource_mut::<GameClock>().unwrap().hold()`), so
//!   that `ctx.game_time` / `ctx.game_frame` — what every closed-form evaluator reads, from a
//!   pattern timeline to a particle emitter — stand still while the sim keeps ticking. Held nothing
//!   by default, so a game that never freezes never names it; `scale(s)` slows it the same way, and [`GameTime`] is the exact fractional gameplay time (`ctx.game_instant`).
//! - **Render extent** — [`RenderExtent`], the resource carrying the pixel extent the session
//!   renders and streams at (`world.resource::<RenderExtent>()`). It is the `viewport` every
//!   pointer mapping takes — [`math::list_hit`], [`math::camera_ray`] — so a game never names an
//!   extent of its own and a resized host maps a cursor correctly. Host state: never hashed, never
//!   snapshotted.
//! - **Fonts** — [`FontLibrary`], the resource resolving a component's `font` name to the face every
//!   measurement helper takes (`world.resource::<FontLibrary>().cloned().unwrap_or_default()`).
//! - **Aim math** — the [`math`] module: [`math::camera_ray`] turns a render-target pixel into a
//!   world [`Ray`](crate::math::Ray), and [`math::ray_ground_plane`] intersects it with the `y = 0`
//!   ground for cursor/stick aim. It also re-exports [`ludion_core::math`]'s deterministic transcendentals.
//!   (Re-exported as a module — like [`spatial`] — so its `Projection` does not clash with the
//!   camera component's; write `math::Projection`.)
//! - **Deterministic parallelism** — the [`par`] module: `par::par_chunks`/`par_map_chunks`/
//!   `ordered_reduce` are the engine's only sanctioned way onto more than one sim thread; results are
//!   thread-count invariant by construction. Re-exported as a module, like [`math`] and [`spatial`].
//! - **Collision / spatial queries** — [`Collider`] (+ [`Shape`], [`Body`]) and the [`spatial`]
//!   module: a game registers `spatial::rebuild::<Transform, TransformHierarchy>` as an ordinary system at an explicit
//!   schedule position, then calls `spatial::{overlaps, raycast, k_nearest, move_and_slide}`.
//!   [`GroundPosition`] is the seam a custom position component implements to feed the index
//!   (the engine already implements it for [`Transform`]).
//! - **3D spatial queries** — [`SpatialVolume`] (+ [`VolumeShape`]) and the [`spatial3d`] module: a
//!   game registers `spatial3d::rebuild::<Transform, TransformHierarchy>` as a system, then queries
//!   the `Index3d` resource (`overlap_sphere`, `overlap_aabb`, `k_nearest`, `cast`) with a layer
//!   `Filter`. [`WorldPosition`] is the seam a custom position component implements.
//! - **Projectiles** — the opt-in pool of swept 3D shots: `Engine::builder().projectiles(`
//!   [`ProjectileConfig`]`)`, [`projectiles_step`] scheduled after `spatial3d_rebuild`, shots fired
//!   with [`spawn_projectile`] (a [`projectiles::ProjectileDesc`]), and the tick's
//!   [`projectiles::HitEvent`]s read from the [`Projectiles`] resource. Re-exported as a module so
//!   its `Response` and `HitEvent` do not clash.
//! - **Light exposure** — [`light_exposure`] answers how lit 1–16 points are from sim state, with the
//!   renderer's falloff, cone and sky formulas and physics-ray shadows for casting lights
//!   ([`ExposureQuery`], [`Exposure`]); `ludion docs light-exposure`.
//! - **Perception** — sight for guards: `Engine::builder().perception(`[`PerceptionConfig`]`)` and [`perception_update`] scheduled after whatever moves them; a [`Perceiver`] with [`SightZone`]s sees every [`SightTarget`] (sample [`SightPoint`]s, a [`Require`] rule) and the tick's [`SightSample`]s are read from [`PerceptionResults`]; `memory:` keeps a [`StimulusRecord`] ghost per source, fed by sight and [`report_stimulus`]; hearing: a [`Perceiver`]'s `hearing:` and [`emit_noise`] queue a [`Noise`] that [`perception_update`] delivers as [`HeardNoise`], scaled by [`NoiseZone`] volumes (`ludion docs perception`).
//! - **Dialogue** — `Engine::builder().dialogue(`[`DialogueConfig`]`)`, [`dialogue_update`] scheduled after the systems that `say` a line (`ludion::dialogue`), [`DialogueEvents`] and the [`Subtitles`] on air (`ludion docs dialogue`).
//! - **Navigation** — the opt-in walkability grid built from the static physics colliders:
//!   `Engine::builder().nav(`[`NavConfig`]`)` and [`nav_update`] scheduled before whatever reads the
//!   [`Nav`] resource. A [`NavAgent`] entity gets its desired velocity in [`NavSteering`] from [`nav_steer`]
//!   (scheduled after `nav_update`, before the game's mover): a flow-field seek for a walker, a 3D seek for a
//!   flyer, plus separation from `spatial3d` neighbours; with a [`NavGoal`] a walker follows a planned
//!   [`NavRoute`] instead, and [`nav_path`] answers one-off [`PathRequest`]s. A [`NavLink`] adds an off-mesh edge (a ladder): a routed walker stops `at_link` until the game calls [`nav_link_done`]. A guard with a [`PatrolFollower`] walks a `kind: patrol` document as nav legs when [`patrol_update`] is scheduled between `nav_update` and `nav_steer`; [`patrol_motion`] reads its facing. The `ludion::nav` module (not in the prelude, which would clash with a game's own `nav`) holds the grid and flow-field types (`NavGrid`,
//!   `FlowCache`, `FlowField`).
//! - **Rigid-body dynamics** — the second tier of the same [`Collider`] surface: a 3D [`Shape`] or
//!   `body: rigid` (with [`RigidBody`] and its [`AxisLock`]; a static or kinematic body takes a
//!   [`ContactMaterial`]) is simulated rather than indexed, and
//!   [`Velocity`] is both how a game sets a velocity and where the resolved velocity lands. A scene opts
//!   in with a top-level `physics:` block (`ludion docs scene-format`); its presence places the
//!   order-critical write/step/mirror triple after every game system, in the space the block names:
//!   `free` for a 3D game, which ignores the [`Plane`] an engine always installs, or `planar` for a
//!   2D one, which takes the plane's axis locks on every rigid body. [`PhysicsWorld`] is the world
//!   resource it maintains: it queues impulses (at a body's centre, at a point, or a radial [`Blast`]
//!   with a [`Falloff`]) and answers body counts, sleep state and its four read-only queries
//!   ([`QueryFilter`], [`QueryHit`], [`PointHit`]), which report a bad argument as [`PhysicsError`]. An entity
//!   belongs to one tier or the other, never both.
//! - **Physics events** — [`ContactEvent`] (a contact beginning or ending between two bodies; `ludion
//!   docs physics`, `ludion docs collision-layers`), [`SensorEvent`] (a body entering or leaving a
//!   sensor volume; `ludion docs physics-sensors`) and [`JointBroken`] (a carrier's joint breaking;
//!   `ludion docs physics-joints`), each read off [`PhysicsWorld`] as one step's sorted list.
//! - **Animation events** — [`animation_events`] is a sim system a game registers after whatever
//!   writes `Animation`; each tick it rewrites the [`AnimationEvents`] resource with the
//!   [`ClipEvent`]s the clip clock crossed (authored events, plus `end` for a one-shot), in a
//!   deterministic order. Only the incoming clip of a crossfade emits. [`set_clip_speed`] changes a
//!   playing clip's rate without a jump in pose or events. [`AnimationLayers`] stacks up to four
//!   masked override or additive clips over the base pose of a skinned character; [`fade_layer`] and
//!   [`play_layer`] steer a layer by name (`ludion docs character-animation`); a layer may name a
//!   blend, steered by [`set_layer_params`], and fire events with `events: true`. [`blend_clocks`]
//!   keeps the [`BlendClock`] phase (and each layer's [`LayerPhase`]) of a character whose
//!   `Animation.clip` or layer clip names a blend space, so its footstep events fire once per
//!   stride at any speed (`ludion docs animation-layers`).
//! - **Ragdolls** — [`switch_to_ragdoll`] promotes a posed `Mesh.shape: assembly` entity into one
//!   `rigid` body entity per part, joined by [`Joint`]s (fixed to its parent unless the assembly's
//!   `joints:` binds it to something else); the root entity survives, stripped of its drawing/physics
//!   components. [`AssemblyData`] (+ [`load_assembly`]) is the meshed model the switch reads —
//!   whatever a game's asset cache already holds for the entity's `assembly` path.
//!   [`switch_skin_to_ragdoll`] does the same for a skinned mesh: it spawns a parts prefab of
//!   [`SkinBone`] bodies bound to the live skin (the prefab's last part, `$skin`, stands for it) at the
//!   rest pose, with the skin's velocity; a game finds a skin's parts by their [`SkinBone`].
//! - **Projectile patterns** — [`Volley`] is one fired volley, a closed-form bullet field on the
//!   entity that carries it; [`PatternPlayer`] plays a `kind: pattern` document's timeline, standing
//!   a [`Volley`] up on each entry's frame. [`play_patterns`] and [`advance_volleys`] are the
//!   systems that fire and reap them; the `bullet_*` readers ([`bullet_position`], [`bullet_age`],
//!   [`bullet_is_alive`]) and [`volley_hits`] answer where a bullet is and what it touches at a
//!   gameplay time (`ludion docs patterns`).
//! - **Camera rig** — [`CameraRig`] on the camera entity follows a target with springs and look-ahead
//!   in its [`TrackRig`] mode, sits rigidly on it with game-written look in [`FirstPersonRig`] mode,
//!   orbits it on a boom in [`ChaseRig`] (look-driven) or [`FollowRig`] (fixed angles) mode
//!   ([`add_look`], [`set_look`], [`CameraRig::aim`]) and post-aim recoil, roll and FOV channels
//!   ([`add_kick`], [`add_fov_kick`]); a game writes its `trauma` with [`add_trauma`]
//!   and cuts to the desired pose with [`snap_rig`] (`ludion docs camera`).
//! - **Paths** — [`PathFollower`] moves an entity along a `kind: path` document, [`path_follow`] is the
//!   system that does it, and [`PathSampler`] is the arc-length access to a path's positions and
//!   directions (`ludion docs path`).
//! - **Character motor** — [`CharacterMotor`] gives an entity a kinematic capsule whose origin is the
//!   character's feet; [`move_character`] (or [`move_character_with`], taking an explicit
//!   constraint) moves it through the world, and [`move_characters`] moves a crowd with parallel casts. The `MOTOR_*` constants are the colours of the `motor`
//!   debug-draw category (`ludion docs motor`). [`LaneConstraint`] holds a motor to a `kind: path`
//!   lane; the `LANE_*` constants are the colours (and [`LANE_ERROR_FULL`], the error that reaches
//!   the full off-lane colour) of the `lane` debug-draw category (`ludion docs lane`).
//! - **Audio** — [`AudioQueue`] (the resource a system pushes into) and [`AudioEvent`] (a sound
//!   path and a gain). Draining and playback are the engine's job; a system only pushes.
//!   [`play_sound`] queues a plain one-shot in one call; positional and loop calls stay on
//!   [`AudioQueue`]. `AudioQueue::play_on` plays a one-shot that follows an entity and returns a
//!   [`VoiceId`] for `voice_stop`.
//!   [`AudioEmitter`] (with [`AudioCategory`]) is the declarative half: a looping sound placed in
//!   the world, kept in sync with the listener's distance by the engine, so a discoverable bell or
//!   chant needs no system at all. [`CategoryGains`] is the mixdown half — the master gain each
//!   category's bus is mixed through, authored once on the builder
//!   ([`EngineBuilder::category_gains`](ludion_engine::EngineBuilder::category_gains)) and read off
//!   the engine by whichever host runs it. It is mixdown, not world state: no value in it reaches a
//!   `WorldHash`, a replay or the audio log. Two more builder knobs shape positional sound:
//!   [`AudioPan`] (`audio_pan`, default centred) places it across the stereo field, and
//!   [`AudioDistance`] (`audio_distance`, default planar) lets height attenuate it. [`Bus`] and
//!   [`MixLayer`] address the runtime mix (`AudioQueue::bus_gain`/`bus_pause`: `ludion docs audio-mix`).
//!   [`AudioListener`] (with [`ListenerOrientation`]) moves the ear off the camera onto any entity:
//!   `ludion docs audio-space`.
//! - **Particles** — [`ParticleEmitter`] (with [`EmitShape`] and [`ParticleBlend`]): a visual effect
//!   described by numbers, evaluated analytically at the render boundary. No system, no pool, and no
//!   entity per point — a burst is one component write stamping `started` with the current frame.
//! - **Scene transitions** — [`SceneSwitch`] (the resource a system sets via
//!   `request(scene_path, spawn_anchor, carry)` to change district mid-run, or `restart(carry)` to
//!   rebuild the current scene from the text it holds) and its
//!   [`SwitchTo`] payload. The engine performs the switch at the frame boundary, continuing the run.
//!   [`Residue`] is its mirror image: the field-less marker on an entity whose state must **stay**
//!   with its scene, captured on the way out and re-applied when the run comes back this session.
//!   [`ResidueStore`] is the readout: a read-only mirror of what *every* visited scene kept, so a
//!   system can show a district's state before the run enters it. Its one write is
//!   `ResidueStore::clear()` — the verb a run restarting in place uses to forget the city.
//!   [`Name`] rides along with them: a residue record's identity is its entity name, so a game that
//!   spawns a record rather than authoring it has to write one.
//! - **Quitting** — [`Shutdown`]: the resource a system sets with `request()` to end the process, the
//!   [`SceneSwitch`] mold one frame shorter. The engine honors it at the frame boundary through the
//!   same path the wire `engine.shutdown` takes, so a title screen's back binding closes a `--window`
//!   session exactly as the window button does.
//! - **Probe refreeze** — [`ProbeRefreeze`]: the resource a system sets with `request()` on the tick
//!   it swaps a room, so probes spawned at runtime capture the world that holds them from the next tick.
//!   `EngineBuilder::probe_bake_worlds` bakes those rooms; `Engine::probe_coverage` returns a
//!   [`ProbeCoverage`] whose `check()` fails with `L1279` for a probe the store lacks.
//! - **Anti-aliasing** — [`RenderSettings`]: the resource an options-menu system sets with
//!   `request_anti_aliasing(`[`AntiAliasing`]`)`, beating the host's choice and the builder's
//!   `anti_aliasing` default from the next frame. Write-only: a system cannot read the effective mode.
//! - **Player settings** — [`Settings`]: the resource an options menu sets (`request_bus_volume`,
//!   `request_window_mode`, `request_quality`, `request_anti_aliasing`, `save()`); [`SettingsView`] reads
//!   the values in force; [`WindowMode`] and [`QualityPreset`] name the choices. `ludion docs settings`.
//! - **Pointer lock** — [`CursorLock`], which a system sets with `lock()`/`release()`; the in-process
//!   window grabs and hides the pointer, and focus loss frees it. `CursorMode::Locked` starts locked.
//! - **Rumble** — [`Rumble`], which a system fills with `play(`[`RumbleEffect`]`)`/`stop_all()`; the
//!   in-process window plays it on the primary pad (Windows) and stops it on focus loss.
//! - **Diagnostics & ids** — [`Diagnostic`], [`DiagnosticCode`], [`EntityId`], and the channel a
//!   *system* reports through: [`report_diagnostic`]`(world, code, key, message)` (or the
//!   [`Diagnostics`] resource behind it); [`report_full_diagnostic`]`(world, key, diagnostic)` takes a
//!   whole [`Diagnostic`] with its path and help. The engine drains it at the frame boundary onto the
//!   `log` channel and stderr, once per `(code, key)` per session, at the severity's level — so a
//!   check that runs every tick prints one line, and `key` (a doc name, a scene name) keeps two
//!   offenders distinct.
//! - **Marking a field** — [`EntityRefSchema`] / [`EntityRefSelfSchema`] and [`AssetPathSchema`]:
//!   `#[schemars(with = "EntityRefSchema")]` on an [`EntityId`] field (or `Option<…>` of it) makes
//!   scene load range-check it and `scene.save` remap it; `with = "AssetPathSchema"` on a `String`
//!   makes it a content-root path. A list or map of references is refused (`L1392`).
//! - **Wire surface** — [`Command`] and the [`command`] params module, for driving an engine
//!   directly in tests.
//!
//! **Not re-exported: the `serde` and `schemars` derive macros.** A game's components derive
//! `Serialize`, `Deserialize`, and `JsonSchema`, so a game adds `serde` and `schemars` to its own
//! `Cargo.toml` — they are the game's dependencies, not something the facade injects.

pub use ludion_engine::{
    Aim, Anchor, Animation, AnimationEvents, AnimationLayer, AnimationLayers, AntiAliasing,
    AssemblyData, AudioCategory, AudioDistance, AudioEmitter, AudioEvent, AudioListener, AudioPan,
    AudioQueue, BlendClock, BlendCurve, BlendIn, BuildStamp, BulletKind, Bus, Camera, CameraOffset,
    CameraRig, CameraZone, CategoryGains, ChaseRig, CheckpointConfig, CheckpointInfo, Checkpoints,
    ClipAnchor, ClipEvent, CursorLock, CursorMode, DebugCategories, DebugPen, Decal, DecalBlend,
    DecalMap, DecalReceiverSet, Diagnostics, DialogueConfig, DialogueEvents, DirectionalLight,
    DocStore, DropShadow, EmitShape, Engine, EngineBuilder, Environment, Fade, FirstPersonRig,
    FollowRig, FollowerMotion, FontLibrary, Framing, GameIdentity, GiDynamic, GroundMarker,
    GroundMarkerGrid, GroundMarkerShape, GroundMarkerStyle, HeardNoise, Hearing, Keep, KickConfig,
    LANE_ANCHOR, LANE_ERROR_FULL, LANE_FORWARD, LANE_LATERAL, LANE_OFF, LANE_ON, LANE_PATH,
    LaneConstraint, LaneEnds, LaneInput, LayerMode, LayerPhase, ListenerOrientation, Look,
    LookAhead, MOTOR_AIRBORNE, MOTOR_CARRY, MOTOR_CEILING, MOTOR_GROUND_NORMAL, MOTOR_GROUNDED,
    MOTOR_PROBE, MOTOR_STUCK, MOTOR_WALL, MapSpawn, MapWrap, Mesh, MeshGrid, MeshSource, Mirror,
    MixLayer, MotionSegment, Name, Nav, NavAgent, NavConfig, NavGoal, NavLink, NavPath, NavReplan,
    NavRoute, NavRouteStatus, NavSteering, NoInterpolate, Noise, NoiseId, NoiseRecord, NoiseZone,
    NoiseZoneShape, PARTICLE_MAX_LIVE_PER_EMITTER, ParentJoint, ParticleBlend, ParticleEmitter,
    PathDocument, PathFollower, PathFrame, PathInterpolation, PathMetric, PathMode, PathOrient,
    PathRequest, PathSampler, PathStatus, PatternDoc, PatternPlayer, PendingNoise, Perceiver,
    PerceiverMemory, PerceiverStatus, PerceptionConfig, PerceptionResults, PerceptionState,
    PlaybackControls, PlaybackInput, PlaybackOutcome, PlaybackSpeed, PointLight, PostProcess,
    PrefabApplied, PrefabInstance, PrefabPrewarm, PrefabReverted, ProbeCoverage,
    ProbeCoverageEntry, ProbeRefreeze, ProbeShape, Profile, ProjectileConfig, ProjectileLook,
    ProjectileVisual, ProjectileVisuals, Projectiles, Projection, QualityPreset, RampSpec,
    Recenter, ReflectionProbe, RenderExtent, RenderSettings, ReplayEntry, ReplayPlayback,
    ReplayPlaybackConfig, Require, Residue, ResidueStore, RestoreOutcome, ResumeOutcome,
    RigCollision, RigCollisionConfig, RigConfig, RigMode, RigState, Rumble, RumbleEffect,
    RunRecording, RunRecordingConfig, RunSave, SceneSwitch, Sense, Settings, SettingsView,
    ShakeConfig, Shutdown, SightPoint, SightSample, SightSampleRecord, SightTarget, SightZone,
    SkinBone, SpotLight, Springs, Sprite, SpriteBlend, Static, Stimulus, StimulusRecord, Subtitles,
    SurfaceNoise, SuspendInfo, SweepShape, SwitchTo, Text, TileAppearance, Tilemap, TimelineEntry,
    TrackRig, Transform, TransformHierarchy, TurnSpec, UiAnchorBehind, UiAnchorShow, UiClock,
    UiEase, UiGroup, UiList, UiListItem, UiRect, UiText, UiTween, UiTweenField, UiTweenRepeat,
    UiTweenTrack, UiWorldAnchor, VOLLEY_MAX_BULLETS, VOLLEY_SALT, VerticalAnchor, Viewmodel,
    ViewmodelLens, Visibility, Volley, VolleyCenter, VolleyShape, WindowMode, WorldPose, ZoneSweep,
    add_fov_kick, add_kick, add_look, add_look_counts, add_trauma, advance_volleys,
    animation_events, blend_clocks, builtin_registry, bullet_age, bullet_is_alive, bullet_position,
    camera_occluders, camera_relative, camera_relative_yaw, character_fit, check_path, crossfade,
    debug_draw, despawn_instance, dialogue_update, emit_noise, fade_layer, first_person_eye,
    fits_character, forget, forget_all, lane_error_colour, lane_frame, lane_relative,
    load_assembly, move_character, move_character_with, move_characters, nav_distance,
    nav_link_done, nav_path, nav_project, nav_reachable, nav_steer, nav_update, orbit_eye,
    path_follow, path_follower_motion, perception_update, play_layer, play_patterns, play_sound,
    posed_joint_world, prepare_map, prepare_prefab, prewarm_prefab, probe_character_ground,
    projectiles_step, reparent, report_diagnostic, report_full_diagnostic, report_stimulus,
    resize_character, resume_character, revert_prefab, set_character_posture, set_clip_speed,
    set_layer_params, set_look, snap_rig, spawn_map, spawn_prefab, spawn_projectile, stimuli,
    suspend_character, switch_skin_to_ragdoll, switch_to_ragdoll, teleport_character,
    turn_character, unpack_prefab, validate_pattern_docs, volley_hits, world_transform,
};

pub use ludion_engine::VoiceId;
pub use ludion_engine::ui::UiNav;
pub use ludion_engine::{
    Combine, Exposure, ExposureError, ExposureQuery, Facing, LightContribution, LightExposure,
    LightExposureResults, LightKind, MAX_EXPOSURE_POINTS, light_exposure, light_exposure_update,
};
pub use ludion_engine::{
    PatrolFollower, PatrolMotion, PatrolPhase, PatrolResume, PatrolState, patrol_motion,
    patrol_update,
};

pub use ludion_ecs::{
    AssetPathSchema, Component, EntityRefSchema, EntityRefSelfSchema, Parent, Phase, Registry,
    Schedule, SemanticValidate, SystemCtx, World,
};

pub use ludion_sim::rebind::{CaptureOptions, CaptureResult, CaptureSlot, Resolve};
pub use ludion_sim::{
    Action, ActionName, Actions, GamepadAxis, GamepadButton, GamepadState, InputBindingsDoc,
    InputEvent, InputState, Key, MouseButton, MouseState, STICK_DEADZONE, TRIGGER_THRESHOLD,
};

pub use crate::math;

mod single;
pub use single::SingleExt;

pub use ludion_physics::{
    Blast, BlastHit, CharacterMotor, CharacterMotorState, ContactEvent, ContactPhase, ExitCause,
    Falloff, GroundHit, GroundProbe, JointBroken, LaneEnd, LaneFrame, LaneReport, LayerSet,
    MotorError, MoveConstraint, MoveReport, PhysicsError, PhysicsWorld, PlatformCarry,
    PlatformPose, PointHit, Posture, PostureReport, PostureShape, QueryFilter, QueryHit,
    ResizeReport, ResumeReport, SensorEvent, SensorPhase, SurfaceHit, SurfaceId, SurfaceOverlay,
    SurfaceTable, SurfaceTag, SurfaceVelocity, TurnReport, height_in_force, motor_capsule_segment,
    motor_in_force, teleport_kinematic,
};
pub use ludion_projectiles as projectiles;
pub use ludion_spatial::{
    AxisLock, Body, Collider, CombineRule, CompoundPart, ContactMaterial, GroundPosition, Joint,
    JointKind, JointMotor, JointPoses, NoParent, ParentPose, Plane, PoseBlend, RigidBody, Shape,
    SpatialVolume, Velocity, VolumeShape, WorldPosition, spatial, spatial3d,
};

pub use ludion_core::codes::code;
pub use ludion_core::diagnostics::{Diagnostic, DiagnosticCode};
pub use ludion_core::ids::EntityId;
pub use ludion_core::par;
pub use ludion_core::rng::{DeterministicRng, WorldSeed};
pub use ludion_core::time::{FrameIndex, GameClock, GameTime, SimTime, TickRate};
pub use ludion_sim::WorldHash;

pub use ludion_protocol::{Command, command};
pub use ludion_scene::{check_doc, emit_doc, parse_doc};
