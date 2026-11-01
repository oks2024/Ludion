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
//! - **Built-in visual components** — [`Transform`], [`Sprite`] (+ [`SpriteBlend`], its `alpha` /
//!   `additive` compositing flag), [`Mesh`] (+ [`MeshSource`]), [`PointLight`],
//!   [`SpotLight`], [`ReflectionProbe`] (+ [`ProbeShape`]), [`DirectionalLight`] (the sun), [`Camera`] (+ [`Projection`]), [`Text`],
//!   [`Tilemap`] (+ [`TileAppearance`]), [`MeshGrid`], [`GroundMarker`] (+ [`GroundMarkerShape`]) — the flat
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
//!   [`math::resolve_ui_rect`] for the matching mouse-hit-test helper (or [`math::list_hit`] for a
//!   `UiList`), and the engine guide's
//!   "Screen-space UI" chapter (`ludion docs screen-ui`) and its `UiList` spoke (`ludion docs ui-list`).
//! - **ECS surface** — [`World`] (and its `query`/`get`/`spawn` methods), [`Registry`], [`Schedule`],
//!   [`SystemCtx`], [`Parent`], and the [`Component`] marker. A [`Transform`] is local under a
//!   [`Parent`]: [`world_transform`] reads the composed [`WorldPose`] (never stored, never hashed)
//!   and [`reparent`] (`world, child, parent, `[`Keep::World`] or [`Keep::Local`]) re-points it
//!   (`ludion docs hierarchy`). [`spawn_prefab`] spawns a `kind: prefab` document from a system,
//!   each part carrying a [`PrefabInstance`]; [`revert_prefab`] and [`unpack_prefab`] edit one.
//!   [`SingleExt`] adds `world.single::<T>()` / `world.single_get::<T>()`: the first entity carrying a
//!   singleton component, and its value.
//! - **Sim/input/time** — [`InputState`], [`Key`], [`InputEvent`]; the mouse/gamepad devices
//!   [`MouseState`]/[`GamepadState`] (reached via `input.mouse()`/`input.gamepad()`) and their
//!   name enums [`MouseButton`], [`GamepadButton`], [`GamepadAxis`]; the const-constructible key-or-button
//!   binding [`Action`] and the standard radial stick deadzone [`STICK_DEADZONE`] and the trigger press level [`TRIGGER_THRESHOLD`]; [`SimTime`], [`FrameIndex`],
//!   [`TickRate`]. [`GameClock`] is the gameplay half of that time: the resource a game calls
//!   `hold()` on every frame it is frozen (`world.resource_mut::<GameClock>().unwrap().hold()`), so
//!   that `ctx.game_time` / `ctx.game_frame` — what every closed-form evaluator reads, from a
//!   pattern timeline to a particle emitter — stand still while the sim keeps ticking. Held nothing
//!   by default, so a game that never freezes never names it.
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
//!   module: a game registers `spatial::rebuild::<Transform>` as an ordinary system at an explicit
//!   schedule position, then calls `spatial::{overlaps, raycast, k_nearest, move_and_slide}`.
//!   [`GroundPosition`] is the seam a custom position component implements to feed the index
//!   (the engine already implements it for [`Transform`]).
//! - **Rigid-body dynamics** — the second tier of the same [`Collider`] surface: a 3D [`Shape`] or
//!   `body: rigid` (with [`RigidBody`] and its [`AxisLock`]; a static or kinematic body takes a
//!   [`ContactMaterial`]) is simulated rather than indexed, and
//!   [`Velocity`] is both how a game sets a velocity and where the resolved velocity lands. A scene opts
//!   in with a top-level `physics:` block (`ludion docs scene-format`); its presence places the
//!   order-critical write/step/mirror triple after every game system, in the space the block names:
//!   `free` for a 3D game, which ignores the [`Plane`] an engine always installs, or `planar` for a
//!   2D one, which takes the plane's axis locks on every rigid body. [`PhysicsWorld`] is the world
//!   resource it maintains: it queues impulses (at a body's centre, at a point, or a radial [`Blast`]
//!   with a [`Falloff`]) and answers body counts, sleep state and its three read-only queries
//!   ([`QueryFilter`], [`QueryHit`]), which report a bad argument as [`PhysicsError`]. An entity
//!   belongs to one tier or the other, never both.
//! - **Animation events** — [`animation_events`] is a sim system a game registers after whatever
//!   writes `Animation`; each tick it rewrites the [`AnimationEvents`] resource with the
//!   [`ClipEvent`]s the clip clock crossed (authored events, plus `end` for a one-shot), in a
//!   deterministic order. Only the incoming clip of a crossfade emits. [`set_speed`] changes a
//!   playing clip's rate without a jump in pose or events.
//! - **Ragdolls** — [`switch_to_ragdoll`] promotes a posed `Mesh.shape: assembly` entity into one
//!   `rigid` body entity per part, joined by [`Joint`]s (fixed to its parent unless the assembly's
//!   `joints:` binds it to something else); the root entity survives, stripped of its drawing/physics
//!   components. [`AssemblyData`] (+ [`load_assembly`]) is the meshed model the switch reads —
//!   whatever a game's asset cache already holds for the entity's `assembly` path.
//! - **Audio** — [`AudioQueue`] (the resource a system pushes into) and [`AudioEvent`] (a sound
//!   path and a gain). Draining and playback are the engine's job; a system only pushes.
//!   [`AudioEmitter`] (with [`AudioCategory`]) is the declarative half: a looping sound placed in
//!   the world, kept in sync with the listener's distance by the engine, so a discoverable bell or
//!   chant needs no system at all. [`CategoryGains`] is the mixdown half — the master gain each
//!   category's bus is mixed through, authored once on the builder
//!   ([`EngineBuilder::category_gains`](ludion_engine::EngineBuilder::category_gains)) and read off
//!   the engine by whichever host runs it. It is mixdown, not world state: no value in it reaches a
//!   `WorldHash`, a replay or the audio log.
//! - **Particles** — [`ParticleEmitter`] (with [`EmitShape`] and [`ParticleBlend`]): a visual effect
//!   described by numbers, evaluated analytically at the render boundary. No system, no pool, and no
//!   entity per point — a burst is one component write stamping `started` with the current frame.
//! - **Scene transitions** — [`SceneSwitch`] (the resource a system sets via
//!   `request(scene_path, spawn_anchor, carry)` to change district mid-run) and its
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
//! - **Anti-aliasing** — [`RenderSettings`]: the resource an options-menu system sets with
//!   `request_anti_aliasing(`[`AntiAliasing`]`)`, beating the host's choice and the builder's
//!   `anti_aliasing` default from the next frame. Write-only: a system cannot read the effective mode.
//! - **Diagnostics & ids** — [`Diagnostic`], [`DiagnosticCode`], [`EntityId`], and the channel a
//!   *system* reports through: [`report_diagnostic`]`(world, code, key, message)` (or the
//!   [`Diagnostics`] resource behind it); [`report_full_diagnostic`]`(world, key, diagnostic)` takes a
//!   whole [`Diagnostic`] with its path and help. The engine drains it at the frame boundary onto the
//!   `log` channel and stderr, once per `(code, key)` per session, at the severity's level — so a
//!   check that runs every tick prints one line, and `key` (a doc name, a scene name) keeps two
//!   offenders distinct.
//! - **Wire surface** — [`Command`] and the [`command`] params module, for driving an engine
//!   directly in tests.
//!
//! **Not re-exported: the `serde` and `schemars` derive macros.** A game's components derive
//! `Serialize`, `Deserialize`, and `JsonSchema`, so a game adds `serde` and `schemars` to its own
//! `Cargo.toml` — they are the game's dependencies, not something the facade injects.

pub use ludion_engine::{
    Anchor, Animation, AnimationEvents, AntiAliasing, AssemblyData, AudioCategory, AudioEmitter,
    AudioEvent, AudioQueue, BlendCurve, BlendIn, BulletKind, Camera, CameraRig, CategoryGains,
    ClipAnchor, ClipEvent, DebugCategories, DebugPen, Decal, DecalBlend, DecalMap,
    DecalReceiverSet, Diagnostics, DirectionalLight, DocStore, DropShadow, EmitShape, Engine,
    EngineBuilder, Environment, Fade, FollowerMotion, FontLibrary, GiDynamic, GroundMarker,
    GroundMarkerShape, Keep, LANE_ANCHOR, LANE_ERROR_FULL, LANE_FORWARD, LANE_LATERAL, LANE_OFF,
    LANE_ON, LANE_PATH, LaneConstraint, LaneEnds, LaneInput, LookAhead, MOTOR_AIRBORNE,
    MOTOR_CARRY, MOTOR_CEILING, MOTOR_GROUND_NORMAL, MOTOR_GROUNDED, MOTOR_PROBE, MOTOR_STUCK,
    MOTOR_WALL, MapWrap, Mesh, MeshGrid, MeshSource, MotionSegment, Name,
    PARTICLE_MAX_LIVE_PER_EMITTER, ParticleBlend, ParticleEmitter, PathDocument, PathFollower,
    PathFrame, PathInterpolation, PathMetric, PathMode, PathOrient, PathSampler, PatternDoc,
    PatternPlayer, PointLight, PostProcess, PrefabApplied, PrefabInstance, PrefabReverted,
    ProbeShape, Profile, Projection, RampSpec, ReflectionProbe, RenderExtent, RenderSettings,
    Residue, ResidueStore, RigConfig, RigMode, RigState, SceneSwitch, ShakeConfig, Shutdown,
    SpotLight, Springs, Sprite, SpriteBlend, SwitchTo, Text, TileAppearance, Tilemap,
    TimelineEntry, TrackRig, Transform, TurnSpec, UiList, UiListItem, UiRect, UiText,
    VOLLEY_MAX_BULLETS, VOLLEY_SALT, VerticalAnchor, Volley, VolleyCenter, VolleyShape, WorldPose,
    add_trauma, advance_volleys, animation_events, builtin_registry, bullet_age, bullet_is_alive,
    bullet_position, camera_relative, camera_relative_yaw, check_path, crossfade, debug_draw,
    fits_character, lane_error_colour, lane_frame, lane_relative, load_assembly, move_character,
    move_character_with, path_follow, path_follower_motion, play_patterns, reparent,
    report_diagnostic, report_full_diagnostic, resize_character, revert_prefab, set_speed,
    snap_rig, spawn_prefab, switch_to_ragdoll, teleport_character, unpack_prefab,
    validate_pattern_docs, volley_hits, world_transform,
};

pub use ludion_engine::ui::UiNav;

pub use ludion_ecs::{
    Component, Parent, Phase, Registry, Schedule, SemanticValidate, SystemCtx, World,
};

pub use ludion_sim::{
    Action, GamepadAxis, GamepadButton, GamepadState, InputEvent, InputState, Key, MouseButton,
    MouseState, STICK_DEADZONE, TRIGGER_THRESHOLD,
};

pub use crate::math;

mod single;
pub use single::SingleExt;

pub use ludion_physics::{
    Blast, BlastHit, CharacterMotor, CharacterMotorState, ContactEvent, ContactPhase, ExitCause,
    Falloff, GroundHit, JointBroken, LaneEnd, LaneFrame, LaneReport, MotorError, MoveConstraint,
    MoveReport, PhysicsError, PhysicsWorld, PlatformCarry, PlatformPose, QueryFilter, QueryHit,
    ResizeReport, SensorEvent, SensorPhase, SurfaceHit, SurfaceVelocity, height_in_force,
    teleport_kinematic,
};
pub use ludion_spatial::{
    AxisLock, Body, Collider, CombineRule, CompoundPart, ContactMaterial, GroundPosition, Joint,
    JointKind, JointMotor, Plane, RigidBody, Shape, Velocity, spatial,
};

pub use ludion_core::codes::code;
pub use ludion_core::diagnostics::{Diagnostic, DiagnosticCode};
pub use ludion_core::ids::EntityId;
pub use ludion_core::par;
pub use ludion_core::time::{FrameIndex, GameClock, SimTime, TickRate};
pub use ludion_sim::WorldHash;

pub use ludion_protocol::{Command, command};
pub use ludion_scene::{check_doc, emit_doc, parse_doc};
