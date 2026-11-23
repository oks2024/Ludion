//! The engine snapshot document: capture (`engine.snapshot`) and restore (`engine.restore`).
//!
//! A snapshot is a self-describing JSON document carrying everything needed to reconstruct the exact
//! sim state (architecture §8): the canonical world serialization (live entities, each with its
//! `(index, generation)` and its registered components in registration order), the RNG state, the
//! frame index, the run-state, the speed, and the scene reference and seed. It doubles as the replay
//! keyframe format.
//!
//! **Restore is exact for the observable world:** the reconstructed world enumerates and hashes
//! identically to the one snapshotted (same live `(index, generation)` set, same components), and the
//! RNG continues from the same state. Held input **is** captured (the additive `input` field:
//! keys, buttons, cursor, sticks), and so are the **pending future-frame injected inputs** (the
//! additive `pending_input` field, the script's unapplied tail), so an `input.inject` addressed to a
//! later frame still fires after a restore (see
//! [`SimDriver::set_pending_input`](ludion_sim::SimDriver::set_pending_input)). The generational allocator **is** restored
//! with full fidelity — dead-slot generations and free-list order — via the additive `allocator`
//! field, but only when the live-entity set alone cannot reproduce it (a world with recycled or gap
//! slots); a clean snapshot omits the field and is byte-identical to before. This keeps id allocation
//! deterministic across a restore (a stale handle stays a reported error, and a replay seek's
//! post-restore spawns mint the same ids the uninterrupted run would). The allocator state is invisible
//! to the world hash.
//!
//! The session's live **data-doc reload log** rides the same additive precedent (the `doc_reloads`
//! field): a restore adopts it and re-applies each reload off-disk over the freshly-walked stores (the
//! way a scene switch does), so a replay-seek keyframe landing after a live reload — or a scene switch
//! performed after a restore — reproduces the reloaded docs instead of rebuilding from possibly-changed
//! disk (audit finding 2.1). The log holds one entry per document (latest text, reload count) and the
//! restore reproduces each store's `generation()`. A reload-free session omits the field and is byte-identical to before;
//! an older snapshot without it restores with the pre-fix clean-disk rebuild. Invisible to the world
//! hash (the `DocStore` it feeds is a resource).
//!
//! The **render extent** rides along (the `render_extent` field): it is sim input, so a restore, and
//! a replay seek landing on a keyframe, runs at the extent the capture had, bypassing the host's
//! render-scale check. An older snapshot without it leaves the host's extent alone.
//!
//! The **rigid-body solver state** rides the same precedent (the `physics` field), with one
//! difference: it is opaque text — base64 of a binary codec — because a structured JSON value cannot
//! hold every field the solver keeps, and `serde_json` would write raw bytes as an array of numbers.
//! A session with no physics world omits it and serializes byte-identically to before the field
//! existed; a restore into a session that *had* one, from a document without it, drops the world
//! rather than continuing with stale bodies. Invisible to the world hash — the solver is a resource,
//! and what reaches the hash is the pose and `Velocity` the mirror writes back into components.
//!
//! The session's **scene residue** — what each visited scene kept behind — rides the same precedent
//! (the `residue` field): a restore adopts it, so a return, or a replay-seek keyframe landing after
//! one, re-applies exactly what the snapshotted session had captured instead of arriving at a scene
//! that has forgotten every earlier visit. A game that marks nothing keeps an empty map, omits the
//! field, and serializes byte-identically to before; an older snapshot without it restores as no
//! residue. Invisible to the world hash — residue is session state, not world content.
//!
//! The `arrived_by_restart` fact of the `SceneSwitch` resource rides along (the field of that name):
//! a snapshot taken on the boundary a restart landed would otherwise restore it false, and a seek
//! would then run the next tick differently from the live session. Omitted when false, so existing
//! snapshots serialize byte-identically.
//!
//! The loaded scene's **`physics:` block** rides the same precedent (the `scene_physics` field,
//! additive and distinct from the solver-bytes `physics` field above): a restore adopts it and
//! rebuilds the schedule with it, so a `ludion dev` restart into a fresh engine that never loaded
//! the scene still steps physics, and a scene switch performed after a restore governs by the
//! arriving district's own block. A block-free session omits it and serializes byte-identically to
//! before the field existed; an older snapshot without it restores as no block. `SNAPSHOT_FORMAT`
//! stays 1.
//!
//! The live **spatial index** (`spatial_index`) rides it too, as its filed entries: a system scheduled
//! before `spatial_rebuild` reads the index the previous tick filed, from poses the snapshot no
//! longer holds, so only carrying the entries makes the first tick after a restore read what the
//! uninterrupted run read. A game that never ran the rebuild omits it; an older snapshot restores
//! with no index, which reads empty until the first rebuild. An entry whose center or shape is
//! non-finite (a game wrote a NaN pose onto a collider) is not carried: JSON has no such number,
//! and the whole restore would fail on it. A resource, invisible to the world hash.
//!
//! The **animation-event walk record** (`animation_walk`) rides the same precedent: where the last
//! `animation_events` walk stopped, so a restore resumes it instead of re-deriving a window. A
//! session that never ran the system omits it; an older snapshot restores with no record, which is
//! the walk's "already running when first seen" rule; one captured on an early starter's start tick
//! fires that clip's `t = 0` event again on the first walk after the restore. A resource,
//! invisible to the world hash.
//!
//! The **follower motions** (`follower_motions`) ride it too: a `speed: 0` path follower picks its
//! facing from last tick's `s`, so carrying the last tick's motions makes the first tick after a
//! restore face as the uninterrupted run did. A session with no posed follower omits it and
//! serializes byte-identically to before the field existed; an older snapshot restores with none.
//! A resource, invisible to the world hash.
//!
//! The **projectile pool** (`projectiles`) and the 3D **spatial index** (`spatial_index_3d`) ride it
//! the same way: the pool as its config plus a base64 blob of the pool and its last events (so a
//! reader scheduled before `projectiles_step` sees the same previous-tick events), the index as its
//! built items. A game without a pool, or that never ran `spatial3d_rebuild`, omits the field and
//! serializes byte-identically to before it existed. The pool is hashed (`WorldHash` rule 6); the
//! index is a resource, invisible to the hash.
//!
//! The **checkpoint store** (`checkpoints`) rides it as each held slot's own snapshot document plus
//! the last restore's outcome, so a replay keyframe, an `engine.restore` and a run-save resume
//! reinstall the slots a later restore asks for. A session restore replaces the engine's store with
//! it; a store-free session omits the field. The slot digest in the `WorldHash` covers the slot
//! metadata, not these documents.
//!
//! **Presentation entities** (`Plan/presentation-state.md`) ride the same way: the `presentation`
//! entity list and, only when the live set cannot reproduce it, `presentation_allocator`. A world
//! without presentation entities omits both and serializes byte-identically to before.

use ludion_core::codes::code;
use ludion_core::diagnostics::Diagnostic;
use ludion_core::ids::EntityId;
use ludion_core::rng::DeterministicRng;
use ludion_core::time::{FrameIndex, GameClock};
use ludion_ecs::{ColumnCopy, Registry};
use ludion_physics::PhysicsWorld;
use ludion_protocol::command::RenderSizeParams;
use ludion_protocol::replay::{ReplayExtent, SceneRef};
use ludion_protocol::restore::{RestoreEntry, RestoreReport};
use ludion_scene::{SceneLayer, ScenePhysics, ScenePhysicsSpace, validate_physics};
use ludion_sim::{DriverRestore, InputScript, InputState, RunState, Speed};
use ludion_spatial::SpatialIndex;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use std::collections::BTreeMap;
use std::sync::{Arc, OnceLock};

use crate::animation_events::AnimationWalk;
use crate::engine::{Engine, RestoredWorld};
use crate::path_follow::{FollowerMotionEntry, FollowerMotions};

/// The snapshot document format version (bumped on any breaking shape change).
const SNAPSHOT_FORMAT: u32 = 1;

/// The full engine snapshot document (serde shape of the `snapshot` payload).
#[derive(Clone, Serialize, Deserialize, JsonSchema)]
struct SnapshotDoc {
    /// Format version marker ([`SNAPSHOT_FORMAT`]).
    format: u32,
    /// The engine build that produced the snapshot.
    engine_version: String,
    /// The frame the snapshot was taken at.
    frame: u64,
    /// The run-state at snapshot time.
    run_state: RunState,
    /// The wall-clock pacing factor at snapshot time.
    speed: f64,
    /// The render extent at snapshot time. Sim input (a game reads it as `RenderExtent`), so a
    /// restore, and a replay seek landing on a keyframe, runs at the extent the capture had. Optional
    /// and additive: a snapshot without it leaves the host's extent alone.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    render_extent: Option<ReplayExtent>,
    /// The seed the world was instantiated with.
    seed: u64,
    /// The scene reference (path or inline), when one is loaded.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    scene: Option<SceneRef>,
    /// The loaded scene's `name:` field (authoring metadata; not world content). Optional and
    /// additive, so a snapshot from a build that predates it still restores (as no scene name).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    scene_name: Option<String>,
    /// The player profile at snapshot time, as canonical `kind: profile` text, or `None` if the game
    /// registered no profile. Optional and additive (the `scene_name` precedent), so a profile-free
    /// snapshot serializes byte-identically to before this field existed. On restore the profile is
    /// installed from **this** text, not re-read from disk — a restore is an exact continuation, so a
    /// `ludion dev` snapshot→restart→restore preserves unsaved in-memory changes and a replay seek
    /// (which reconstructs from a keyframe) installs the same profile-at-start (Phase 16 / E5,
    /// decision 7).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    profile: Option<String>,
    /// The rigid-body solver state at snapshot time — base64 of the binary codec
    /// [`PhysicsWorld::to_bytes`] writes — or `None` when the session has no physics world. Optional
    /// and additive (the `profile` precedent), so a physics-free snapshot is byte-identical to what
    /// it was before this field existed and an older document restores as no solver.
    ///
    /// Opaque text rather than a structured field because the solver's state is closed by its own
    /// type: building a JSON value out of it fails on the narrow phase's 128-bit masks. That makes
    /// the blob readable only by a build with the same solver pin and codec major version, which is
    /// what `L3033` reports and why a pin move is a snapshot-format event
    /// (`Plan/physics.md` W1).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    physics: Option<String>,
    /// The loaded scene's `physics:` block at snapshot time, or `None` for a bare engine or a
    /// scene that never declared one. Optional and additive (the `physics`/`profile` precedent), so
    /// a block-free snapshot is byte-identical to before this field existed, and an older document
    /// restores as no block — matching a scene that never opted in.
    ///
    /// Carried because restore rebuilds a world **without** re-reading scene text (`snapshot`/
    /// `restore` are text-free, unlike `scene.load`): the block is session configuration, and this
    /// is the only place it can ride across a restore, including a `ludion dev` restart into a
    /// fresh engine that never loaded the scene.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    scene_physics: Option<ScenePhysicsDoc>,
    /// Where the last `animation_events` walk stopped, or `None` for a session that never ran it.
    /// Optional and additive (the `physics` precedent): a restore resumes the same walk, so a `t: 0`
    /// event is neither lost nor fired twice across it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    animation_walk: Option<AnimationWalk>,
    /// The spatial index's filed entries, or `None` for a session that never ran
    /// `spatial_rebuild`. Optional and additive (the `animation_walk` precedent): a restore refiles
    /// them, so a reader scheduled before the rebuild sees the index it saw uninterrupted.
    /// `Some(vec![])` stays distinct from `None` — the unrun-collider report keys on the resource.
    /// An entry with a non-finite center or shape is left out (JSON has no such number, so it
    /// would fail the whole restore); the next `spatial_rebuild` files it again.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    spatial_index: Option<Vec<ludion_spatial::Entry>>,
    /// The last `path_follow` tick's per-follower motions, or `None` when no follower has been
    /// posed. Optional and additive (the `animation_walk` precedent): a `speed: 0` follower picks
    /// its facing from last tick's `s`, so the first tick after a restore needs it to match the
    /// uninterrupted run. An entry with a non-finite float is left out.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    follower_motions: Option<Vec<FollowerMotionEntry>>,
    /// The projectile pool, or `None` for a game that configured none. Optional and additive (the
    /// `physics` precedent): a restore re-creates the pool slot for slot, so in-flight shots, their
    /// generations and the free list continue as in the uninterrupted run.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    projectiles: Option<ProjectilesDoc>,
    /// The `spatial3d` index's built items, or `None` for a session that never ran
    /// `spatial3d_rebuild`. Optional and additive (the `spatial_index` precedent): a reader
    /// scheduled before the rebuild sees the index it saw uninterrupted. Items are finite by
    /// construction, so none is filtered.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    spatial_index_3d: Option<Vec<ludion_spatial::spatial3d::Item>>,
    /// The RNG state — restored verbatim so the sequence continues identically.
    rng: DeterministicRng,
    /// The gameplay clock — how much of the frame count the game had held out of its own time, and
    /// whether the captured frame itself was held. Restored verbatim (the `rng` precedent): a
    /// session captured mid-freeze resumes frozen, with gameplay time exactly where it stopped.
    /// Omitted from the document entirely when it holds nothing, so every snapshot of a game that
    /// never freezes — which is every snapshot written before this field existed — is byte-identical
    /// to what it was, and an older document restores as an unheld clock.
    #[serde(default, skip_serializing_if = "GameClock::is_zero")]
    game_clock: GameClock,
    /// The held input at capture — keys, buttons, cursor and sticks. Restored verbatim (the
    /// `game_clock` precedent), so a restore or seek continues with the input the capture had
    /// instead of releasing everything. Omitted when idle, so a snapshot of an untouched session —
    /// every snapshot written before this field existed — is byte-identical to what it was, and an
    /// older document restores idle. Never hashed: the state is a resource.
    #[serde(default, skip_serializing_if = "InputState::is_idle")]
    input: InputState,
    /// The injected events addressed to frames the snapshot has not run — the driver's script past
    /// its cursor. Restored verbatim (the `input` precedent), so an `input.inject` for a later frame
    /// survives a restore or a `ludion dev` restart. Omitted when empty, so every snapshot written
    /// before this field existed is byte-identical to what it was, and an older document restores
    /// with no pending events. Never hashed.
    #[serde(default, skip_serializing_if = "InputScript::is_empty")]
    pending_input: InputScript,
    /// The live entities, in ascending slot-index order, with their components.
    entities: Vec<EntityDoc>,
    /// The full generational-allocator state (dead-slot generations + free-list order), present
    /// **only when** the live-entity list cannot reproduce it — i.e. a world with recycled or gap
    /// slots. Optional and additive (the `scene_name`/`profile` precedent): a clean snapshot omits it
    /// and serializes byte-identically to before this field existed, and an older snapshot that
    /// predates it restores via the live-set fallback ([`ludion_ecs::World::restore_entities`]).
    ///
    /// This is what makes id allocation deterministic across a restore: without it, a slot recycled
    /// before the snapshot comes back re-issuable at generation 0, so the next spawn mints an
    /// `EntityId` bit-identical to a pre-restore stale handle (a silent alias). With it, a replay
    /// seek's post-restore spawns mint exactly the ids the uninterrupted run would. Excluded from
    /// `WorldHash` by construction (the hash visits only live slots).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    allocator: Option<ludion_ecs::AllocatorState>,
    /// The live sim slot indices, earliest spawn first, present **only when** that order differs
    /// from slot order (a slot reused after a despawn). Optional and additive (the `allocator`
    /// precedent): a clean snapshot omits it, and an older snapshot restores in slot order. Physics
    /// feeds its solver in this order, so it must survive a restore. Never hashed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    spawn_order: Option<Vec<u32>>,
    /// The live presentation entities (`Plan/presentation-state.md`), in ascending slot-index
    /// order, with their components. Optional and additive (the `doc_reloads` precedent): a world
    /// with none omits it and serializes byte-identically to before this field existed, and an
    /// older snapshot restores with no presentation entities.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    presentation: Vec<EntityDoc>,
    /// The presentation allocator's exact state, present only when `presentation` cannot reproduce
    /// it (the `allocator` precedent), in indices relative to the presentation range.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    presentation_allocator: Option<ludion_ecs::AllocatorState>,
    /// This session's game data-doc reloads applied before the snapshot, one entry per document in
    /// first-appearance order with its latest text and reload count. Carried
    /// so a restore re-applies them off-disk (the way a scene switch does) rather than rebuilding the
    /// `DocStore`s from possibly-changed disk: a replay-seek keyframe landing after a live reload, or a
    /// scene switch after a restore, then reproduces the reloaded docs cross-machine (audit finding
    /// 2.1). Optional and additive (the `allocator`/`profile` precedent): a session with no live reload
    /// omits it and serializes byte-identically to before this field existed, and an older snapshot
    /// without it restores with the pre-fix behavior (a clean disk rebuild). Never hashed — the
    /// `DocStore` it feeds is a resource.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    doc_reloads: Vec<DocReloadDoc>,
    /// What each scene visited this session kept behind, in ascending scene-key order — carried so a
    /// restore (or a replay-seek keyframe) landing *after* a return reproduces the residue the
    /// snapshotted session had captured, rather than returning to a scene that has forgotten every
    /// earlier visit. Optional and additive (the `doc_reloads` precedent): a residue-free session — a
    /// game that marks nothing — omits it and serializes byte-identically to before this field
    /// existed, and an older snapshot without it restores as empty residue. Never hashed: residue is
    /// session state, not world content (the arriving world it rebuilds *is* hashed).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    residue: Vec<SceneResidueDoc>,
    /// Whether the world was rebuilt by a scene restart on the boundary this snapshot was taken at
    /// (`SceneSwitch::arrived_by_restart`). Additive (the `residue` precedent): omitted when false.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    arrived_by_restart: bool,
    /// The checkpoint store: every held slot as its own snapshot document, and what the last
    /// restore did. Additive (the `residue` precedent): omitted with no slot and no last restore,
    /// and never present inside a slot's own document. A session restore installs it over the
    /// engine's store; an older snapshot without it restores as an empty store.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    checkpoints: Option<crate::checkpoints::CheckpointsDoc>,
    /// Top-level fields this build does not define, kept only so a restore can report them. Never
    /// written: capture leaves it empty, so the document stays byte-identical.
    #[serde(flatten, skip_serializing_if = "BTreeMap::is_empty")]
    #[schemars(skip)]
    unknown: BTreeMap<String, Value>,
}

/// The snapshot's `projectiles` field: the pool's config and its bytes.
#[derive(Clone, Serialize, Deserialize, JsonSchema)]
struct ProjectilesDoc {
    config: crate::ProjectileConfig,
    /// Base64 of the pool's codec bytes followed by the last tick's events' bytes.
    state: String,
}

/// The snapshot's `scene_physics` field: the serde shape of a loaded scene's `physics:` block
/// ([`ScenePhysics`]), which carries no serde derive of its own (`ludion-scene` boundary rule —
/// see the crate docs).
#[derive(Clone, Serialize, Deserialize, JsonSchema)]
struct ScenePhysicsDoc {
    space: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    gravity: Option<[f64; 3]>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    solver_iterations: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    stabilization_iterations: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    sleep_after_ticks: Option<u32>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    layers: Vec<SceneLayerDoc>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    surfaces: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    default_surface: Option<String>,
}

/// One `physics.layers` entry in the snapshot's `scene_physics` field.
#[derive(Clone, Serialize, Deserialize, JsonSchema)]
struct SceneLayerDoc {
    name: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    ignores: Vec<String>,
}

impl From<&ScenePhysics> for ScenePhysicsDoc {
    fn from(physics: &ScenePhysics) -> Self {
        ScenePhysicsDoc {
            space: physics.space.as_str().to_string(),
            gravity: physics.gravity,
            solver_iterations: physics.solver_iterations,
            stabilization_iterations: physics.stabilization_iterations,
            sleep_after_ticks: physics.sleep_after_ticks,
            layers: physics
                .layers
                .iter()
                .map(|layer| SceneLayerDoc {
                    name: layer.name.clone(),
                    ignores: layer.ignores.clone(),
                })
                .collect(),
            surfaces: physics.surfaces.clone(),
            default_surface: physics.default_surface.clone(),
        }
    }
}

impl TryFrom<ScenePhysicsDoc> for ScenePhysics {
    type Error = Diagnostic;

    fn try_from(doc: ScenePhysicsDoc) -> Result<Self, Diagnostic> {
        let space = match doc.space.as_str() {
            "free" => ScenePhysicsSpace::Free,
            "planar" => ScenePhysicsSpace::Planar,
            other => {
                return Err(Diagnostic::new(
                    code::PROTOCOL_SNAPSHOT_INVALID,
                    format!("snapshot `scene_physics.space` `{other}` is not `free` or `planar`"),
                ));
            }
        };
        Ok(ScenePhysics {
            space,
            gravity: doc.gravity,
            solver_iterations: doc.solver_iterations,
            stabilization_iterations: doc.stabilization_iterations,
            sleep_after_ticks: doc.sleep_after_ticks,
            layers: doc
                .layers
                .into_iter()
                .map(|layer| SceneLayer {
                    name: layer.name,
                    ignores: layer.ignores,
                })
                .collect(),
            surfaces: doc.surfaces,
            default_surface: doc.default_surface,
        })
    }
}

/// One data-doc of the reload log carried in a snapshot: the registered `kind`, the store `key`
/// (slash-normalized content-root-relative path), the latest canonical document text, and how many
/// times it was reloaded. `applied` is omitted when 1, so a session with no repeated reload
/// serializes as it always did; an older snapshot may repeat a document, which the read folds
/// (summed counts, last text).
#[derive(Clone, Serialize, Deserialize, JsonSchema)]
struct DocReloadDoc {
    kind: String,
    key: String,
    document: String,
    #[serde(default = "one", skip_serializing_if = "is_one")]
    applied: u64,
}

fn one() -> u64 {
    1
}

fn is_one(n: &u64) -> bool {
    *n == 1
}

/// One scene's residue carried in a snapshot: the scene's `name:` key and the records captured the
/// last time the session left it — the serde shape of one entry in the session's residue map.
#[derive(Clone, Serialize, Deserialize, JsonSchema)]
struct SceneResidueDoc {
    scene: String,
    /// The frame the session last left this scene on — the eviction key
    /// ([`ludion_engine::RESIDUE_MAX_SCENES`](crate::RESIDUE_MAX_SCENES)), carried so a restore
    /// evicts the same scene the uninterrupted run would. Optional and additive (the `doc_reloads`
    /// precedent): an older snapshot that predates the field restores every scene at frame 0 — all
    /// equally old, so eviction falls back on the ascending-scene-key tie break.
    #[serde(default, skip_serializing_if = "is_frame_zero")]
    last_visit: u64,
    records: Vec<ResidueRecordDoc>,
}

/// `skip_serializing_if` for [`SceneResidueDoc::last_visit`]: frame 0 is what `#[serde(default)]`
/// reads a document written before the field back as, so writing it is writing the absence.
fn is_frame_zero(frame: &u64) -> bool {
    *frame == 0
}

/// One captured entity inside a [`SceneResidueDoc`]: its `Name` (the identity a return matches on) or
/// none for an anonymous record, and its component bag as `(registered name, value)` minus `Name` —
/// the same pair [`crate::transition`]'s `ResidueRecord` holds.
#[derive(Clone, Serialize, Deserialize, JsonSchema)]
struct ResidueRecordDoc {
    /// Absent for an anonymous record, so those serialize lean.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    name: Option<String>,
    /// Whether the scene document authored the entity this record was captured from — what tells a
    /// return that a missing counterpart was *renamed* (drop the record) rather than never authored
    /// (respawn it). Optional and additive (the `last_visit` precedent), and absent for the common
    /// `false`: a snapshot written before the field restores every record as code-spawned, which
    /// respawns exactly as it did before the field existed.
    #[serde(default, skip_serializing_if = "is_false")]
    authored: bool,
    bag: Vec<(String, Value)>,
    /// The entity references in `bag`, by the identity each is resolved by on return. Optional and
    /// additive (the `authored` precedent), absent when empty: a snapshot written before the field
    /// restores with no refs, so its bags apply verbatim as they did before.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    refs: Vec<crate::transition::ResidueRef>,
}

/// `skip_serializing_if` for [`ResidueRecordDoc::authored`]: `false` is what `#[serde(default)]` reads
/// a document written before the field back as, so writing it is writing the absence.
fn is_false(flag: &bool) -> bool {
    !*flag
}

/// One live entity in a snapshot: its identity and its components (registry order).
#[derive(Clone, Serialize, Deserialize, JsonSchema)]
struct EntityDoc {
    /// The entity's storage slot index.
    index: u32,
    /// The entity's generation.
    generation: u32,
    /// Component name → value, in registration order (`preserve_order` keeps insertion order).
    components: Map<String, Value>,
}

/// The JSON Schema of the snapshot document, which a replay header's content hash covers.
pub(crate) fn snapshot_schema() -> Value {
    ludion_protocol::schema::schema_of::<SnapshotDoc>()
}

/// Captures the engine's full state as a snapshot document.
///
/// # Errors
/// Propagates a component serialization failure (`L0007`) or a missing RNG resource (`L3008`).
pub(crate) fn capture(engine: &Engine) -> Result<Value, Diagnostic> {
    let mut doc = capture_doc(engine, true)?;
    doc.checkpoints = engine.checkpoints_doc()?;
    snapshot_value(&doc)
}

/// The engine's state as a typed document, without the checkpoint store: a slot never nests
/// another. With `components` false each entity list holds only the ids (`index`, `generation`) and
/// an empty component map: the component values ride a column copy instead
/// ([`capture_structured`]).
fn capture_doc(engine: &Engine, components: bool) -> Result<SnapshotDoc, Diagnostic> {
    let (entities, presentation) = if components {
        (
            entity_docs(engine.registry(), engine.world(), engine.world().entities())?,
            entity_docs(
                engine.registry(),
                engine.world(),
                engine.world().presentation_entities(),
            )?,
        )
    } else {
        (
            id_docs(engine.world().entities()),
            id_docs(engine.world().presentation_entities()),
        )
    };

    let doc = SnapshotDoc {
        format: SNAPSHOT_FORMAT,
        engine_version: crate::engine::ENGINE_VERSION.to_string(),
        frame: engine.frame().as_u64(),
        run_state: engine.run_state(),
        speed: engine.speed().factor(),
        render_extent: Some({
            let (width, height) = engine.render_size();
            ReplayExtent { width, height }
        }),
        seed: engine.seed(),
        scene: engine.scene_ref().cloned(),
        scene_name: engine.scene_name().map(str::to_string),
        profile: engine.emit_profile_text(),
        physics: capture_physics(engine)?,
        scene_physics: engine.scene_physics().map(ScenePhysicsDoc::from),
        animation_walk: engine.world().resource::<AnimationWalk>().cloned(),
        spatial_index: engine.world().resource::<SpatialIndex>().map(|index| {
            let mut entries = index.filed_entries();
            entries.retain(round_trips);
            entries
        }),
        follower_motions: engine
            .world()
            .resource::<FollowerMotions>()
            .map(FollowerMotions::entries)
            .filter(|entries| !entries.is_empty()),
        projectiles: engine
            .world()
            .resource::<crate::Projectiles>()
            .map(capture_projectiles),
        spatial_index_3d: engine
            .world()
            .resource::<ludion_spatial::spatial3d::Index3d>()
            .map(|index| index.items().to_vec()),
        rng: *engine.rng()?,
        game_clock: engine
            .world()
            .resource::<GameClock>()
            .copied()
            .unwrap_or_default(),
        input: engine
            .world()
            .resource::<InputState>()
            .cloned()
            .unwrap_or_default(),
        pending_input: engine.pending_input(),
        entities,
        allocator: engine.world().allocator_state(),
        spawn_order: engine.world().spawn_order(),
        presentation,
        presentation_allocator: engine.world().presentation_allocator_state(),
        doc_reloads: engine
            .session_doc_reloads()
            .iter()
            .map(|reload| DocReloadDoc {
                kind: reload.kind.clone(),
                key: reload.key.clone(),
                document: reload.document.clone(),
                applied: reload.applied,
            })
            .collect(),
        // The session map is sorted by scene key, so the emitted order is deterministic for free.
        residue: engine
            .session_residue()
            .iter()
            .map(|(scene, residue)| SceneResidueDoc {
                scene: scene.clone(),
                last_visit: residue.last_visit.as_u64(),
                records: residue
                    .records
                    .iter()
                    .map(|record| ResidueRecordDoc {
                        name: record.name.clone(),
                        authored: record.authored,
                        bag: record.bag.clone(),
                        refs: record.refs.clone(),
                    })
                    .collect(),
            })
            .collect(),
        arrived_by_restart: engine
            .world()
            .resource::<crate::SceneSwitch>()
            .is_some_and(crate::SceneSwitch::arrived_by_restart),
        checkpoints: None,
        unknown: BTreeMap::new(),
    };
    Ok(doc)
}

fn snapshot_value(doc: &SnapshotDoc) -> Result<Value, Diagnostic> {
    serde_json::to_value(doc).map_err(|error| {
        Diagnostic::new(
            code::PROTOCOL_SNAPSHOT_INVALID,
            format!("could not serialize the engine snapshot: {error}"),
        )
    })
}

/// An in-memory checkpoint body: the typed document with id-only entity lists, plus one column copy
/// per registered component, aligned with `registry.iter()`. Nothing in it is JSON except the
/// opaque solver and projectile bytes already inside the document.
pub(crate) struct StructuredSnapshot {
    doc: SnapshotDoc,
    columns: Vec<Option<ColumnCopy>>,
    /// The document [`structured_document`] renders, kept once it succeeds: a recording renders
    /// each slot once, not at every keyframe.
    rendered: OnceLock<Arc<Value>>,
}

impl StructuredSnapshot {
    /// The cached rendered document, or `render`'s result, cached on success.
    pub(crate) fn rendered_or_else(
        &self,
        render: impl FnOnce() -> Result<Arc<Value>, Diagnostic>,
    ) -> Result<Arc<Value>, Diagnostic> {
        if let Some(doc) = self.rendered.get() {
            return Ok(Arc::clone(doc));
        }
        let doc = render()?;
        Ok(Arc::clone(self.rendered.get_or_init(|| doc)))
    }
}

impl std::fmt::Debug for StructuredSnapshot {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("StructuredSnapshot")
            .field("frame", &self.doc.frame)
            .field("columns", &self.columns.iter().flatten().count())
            .finish()
    }
}

/// Captures the engine's state as a [`StructuredSnapshot`]. No component is serialized, so no value
/// is inspected: a non-finite float surfaces when the slot is rendered as a document.
///
/// # Errors
/// A missing RNG resource (`L3008`) or a solver or projectile encode failure (`L3033`).
pub(crate) fn capture_structured(engine: &Engine) -> Result<StructuredSnapshot, Diagnostic> {
    let doc = capture_doc(engine, false)?;
    let columns = engine
        .registry()
        .iter()
        .map(|info| info.copy_column(engine.world()))
        .collect();
    Ok(StructuredSnapshot {
        doc,
        columns,
        rendered: OnceLock::new(),
    })
}

/// The snapshot document `snapshot.capture` would have produced at the moment `s` was captured,
/// byte for byte: the ids rebuild a scratch world, the columns install into it, and the components
/// are serialized from there.
///
/// # Errors
/// As [`capture`], plus the `L0007` for a component holding a non-finite float, and the
/// allocator errors of a restore.
pub(crate) fn structured_document(
    engine: &Engine,
    s: &StructuredSnapshot,
) -> Result<Value, Diagnostic> {
    let registry = engine.registry();
    let mut world = rebuild_ids(&s.doc)?;
    install_columns(registry, &mut world, &s.columns)?;
    let mut doc = s.doc.clone();
    doc.entities = entity_docs(registry, &world, world.entities())?;
    doc.presentation = entity_docs(registry, &world, world.presentation_entities())?;
    snapshot_value(&doc)
}

/// Installs every present column into `world`, whose entities are already rebuilt.
fn install_columns(
    registry: &Registry,
    world: &mut ludion_ecs::World,
    columns: &[Option<ColumnCopy>],
) -> Result<(), Diagnostic> {
    if columns.len() != registry.len() {
        return Err(Diagnostic::new(
            code::PROTOCOL_SNAPSHOT_INVALID,
            format!(
                "a checkpoint holds {} component columns but the registry has {} components",
                columns.len(),
                registry.len()
            ),
        ));
    }
    for (info, column) in registry.iter().zip(columns) {
        if let Some(column) = column {
            info.install_column(world, column)?;
        }
    }
    Ok(())
}

/// The per-entity component documents of `ids`, in the order given.
///
/// # Errors
/// Propagates a component serialization failure (`L0007`), including a component holding a NaN or
/// infinite float: JSON writes it as `null`, which restore would drop along with the whole component.
fn entity_docs(
    registry: &Registry,
    world: &ludion_ecs::World,
    ids: impl Iterator<Item = EntityId>,
) -> Result<Vec<EntityDoc>, Diagnostic> {
    let mut docs = Vec::new();
    for id in ids {
        let mut components = Map::new();
        for info in registry.iter() {
            let Some(erased) = info.serialize_erased(world, id)? else {
                continue;
            };
            let non_finite = ludion_sim::non_finite_count(erased)
                .map_err(|error| info.serialize_failed(id, &error))?;
            if non_finite > 0 {
                return Err(Diagnostic::new(
                    code::COMPONENT_SERIALIZE_FAILED,
                    format!(
                        "component `{}` on entity {id} holds {non_finite} non-finite float(s) \
                         (NaN or ±inf), which a snapshot cannot store",
                        info.name()
                    ),
                )
                .with_help(
                    "find the system that wrote the NaN or infinity and fix it (`ludion explain L4024`)",
                ));
            }
            if let Some(value) = info.serialize(world, id)? {
                components.insert(info.name().to_string(), value);
            }
        }
        docs.push(EntityDoc {
            index: id.index(),
            generation: id.generation(),
            components,
        });
    }
    Ok(docs)
}

/// The entity documents of `ids` with no components (see [`capture_doc`]).
fn id_docs(ids: impl Iterator<Item = EntityId>) -> Vec<EntityDoc> {
    ids.map(|id| EntityDoc {
        index: id.index(),
        generation: id.generation(),
        components: Map::new(),
    })
    .collect()
}

/// Whether a filed entry survives a JSON round trip: a non-finite float serializes as `null`, which
/// the entry's `f32` fields refuse on restore.
fn round_trips(entry: &ludion_spatial::Entry) -> bool {
    let finite = |v: &f32| v.is_finite();
    entry.center.iter().all(finite)
        && match &entry.shape {
            ludion_spatial::Shape::Circle { radius } => radius.is_finite(),
            ludion_spatial::Shape::Aabb { half_extents } => half_extents.iter().all(finite),
            _ => true,
        }
}

/// The live solver state as the snapshot's `physics` field: base64 of [`PhysicsWorld::to_bytes`], or
/// `None` when the session has no physics world (which is every session until the loaded scene
/// declares a `physics:` block and a rigid body claims something).
///
/// # Errors
/// `L3033` if the solver state cannot be encoded.
fn capture_physics(engine: &Engine) -> Result<Option<String>, Diagnostic> {
    let Some(physics) = engine.world().resource::<PhysicsWorld>() else {
        return Ok(None);
    };
    let bytes = physics.to_bytes().map_err(|error| {
        Diagnostic::new(
            code::PHYSICS_SNAPSHOT_INVALID,
            format!("physics snapshot could not be encoded: {error}"),
        )
    })?;
    Ok(Some(base64::encode(bytes)))
}

/// The live pool as the snapshot's `projectiles` field.
fn capture_projectiles(projectiles: &crate::Projectiles) -> ProjectilesDoc {
    let mut bytes = Vec::new();
    projectiles.pool().encode(&mut bytes);
    projectiles.events().encode(&mut bytes);
    ProjectilesDoc {
        config: projectiles.config().clone(),
        state: base64::encode(bytes),
    }
}

/// What a snapshot's `projectiles` field restores into.
enum RestoredPool {
    /// The pool, under the builder's config.
    Pool(Box<crate::Projectiles>),
    /// The builder configured no pool: nothing is restored.
    Dropped,
    /// The builder's capacity is below the snapshot's high-water mark: nothing is restored (the
    /// engine installs a fresh builder pool) and this warning is reported.
    Refused(Diagnostic),
}

/// Rebuilds the pool resource from a snapshot's `projectiles` field under the **builder's**
/// `ProjectileConfig`: the snapshot's own config is validated but not used, so a restart that
/// changed gravity, capacity or targeting takes the new settings (the pool is resized).
///
/// # Errors
/// `L3008` if the config is invalid or the blob does not decode.
fn restore_projectiles(
    doc: &ProjectilesDoc,
    builder: Option<&crate::ProjectileConfig>,
) -> Result<RestoredPool, Diagnostic> {
    let invalid = |why: String| {
        Diagnostic::new(
            code::PROTOCOL_SNAPSHOT_INVALID,
            format!("snapshot `projectiles` is invalid: {why}"),
        )
        .with_path("/projectiles")
        .with_help("record the snapshot again with the current engine")
    };
    doc.config.validate().map_err(|d| invalid(d.message))?;
    let bytes = base64::decode(&doc.state).map_err(|error| invalid(format!("`state`: {error}")))?;
    let mut cursor = bytes.as_slice();
    let mut pool = ludion_projectiles::ProjectilePool::decode(&mut cursor)
        .map_err(|error| invalid(error.to_string()))?;
    let events = ludion_projectiles::StepEvents::decode(&mut cursor)
        .map_err(|error| invalid(error.to_string()))?;
    if !cursor.is_empty() {
        return Err(invalid("trailing bytes after the events".to_string()));
    }
    if pool.capacity() != doc.config.capacity as usize {
        return Err(invalid("pool capacity differs from the config".to_string()));
    }
    let Some(builder) = builder else {
        return Ok(RestoredPool::Dropped);
    };
    if !pool.resize(builder.capacity as usize) {
        return Ok(RestoredPool::Refused(
            Diagnostic::new(
                code::PROJECTILE_RESTORE_CAPACITY,
                format!(
                    "the snapshot's projectile pool has used {} slots but the build's \
                     `ProjectileConfig::capacity` is {}; the pool was not restored and starts empty",
                    pool.high_water(),
                    builder.capacity
                ),
            )
            .with_path("/projectiles")
            .with_help(format!(
                "raise `ProjectileConfig::capacity` to at least {}, or restore with the old build",
                pool.high_water()
            )),
        ));
    }
    Ok(RestoredPool::Pool(Box::new(crate::Projectiles::with_pool(
        builder.clone(),
        pool,
        events,
    ))))
}

/// Decodes a snapshot's `physics` field back into a solver state at `tick_rate`.
///
/// The caller inserts the result into the freshly rebuilt world; an absent field inserts nothing,
/// and since restore builds a new world from scratch, a session that *had* a solver and restores
/// from a document without one is left with no bodies rather than stale ones.
///
/// # Errors
/// `L3033` if the text is not base64, or the bytes are not a state this build can read.
fn restore_physics(
    blob: &str,
    tick_rate: ludion_core::time::TickRate,
) -> Result<PhysicsWorld, Diagnostic> {
    let bytes = base64::decode(blob).map_err(|error| {
        Diagnostic::new(
            code::PHYSICS_SNAPSHOT_INVALID,
            format!("physics snapshot could not be decoded: {error}"),
        )
        .with_help(concat!(
            "the snapshot was written by a different rapier/bincode version or is corrupt; ",
            "re-record it"
        ))
    })?;
    PhysicsWorld::from_bytes(&bytes, tick_rate).map_err(|error| {
        Diagnostic::new(
            code::PHYSICS_SNAPSHOT_INVALID,
            format!("physics snapshot could not be decoded: {error}"),
        )
        .with_help(concat!(
            "the snapshot was written by a different rapier/bincode version or is corrupt; ",
            "re-record it"
        ))
    })
}

/// Checks that the rebuilt allocator's live slots are exactly the snapshot's `entities` list, so a
/// hand-edited `allocator` field cannot leave component rows on slots that are free (or live slots
/// with no row). The list is compared sorted by slot index, as a full sequence.
///
/// # Errors
/// `L3041` naming the first differing position.
fn check_live_set(
    live: &[EntityId],
    actual: &[EntityId],
    list_field: &str,
    allocator_field: &str,
) -> Result<(), Diagnostic> {
    let mut expected = live.to_vec();
    expected.sort_by_key(|entity| entity.index);
    let describe =
        |entity: Option<&EntityId>| entity.map_or("none".to_string(), ToString::to_string);
    let Some(position) = (0..expected.len().max(actual.len()))
        .find(|&position| expected.get(position) != actual.get(position))
    else {
        return Ok(());
    };
    Err(Diagnostic::new(
        code::RESTORE_ENTITIES_MISMATCH,
        format!(
            "snapshot `{list_field}` lists {} but its `{allocator_field}` state has {} live at position {position} (slot order)",
            describe(expected.get(position)),
            describe(actual.get(position)),
        ),
    )
    .with_path(format!("/{list_field}"))
    .with_help(format!(
        "re-take the snapshot with engine.snapshot, or delete the `{allocator_field}` field to restore from `{list_field}` alone"
    )))
}

/// The highest frame a restore accepts: headroom for the `+ 1` of every tick and for
/// `last_keyframe + keyframe_interval`, which a frame near `u64::MAX` would overflow.
const MAX_RESTORE_FRAME: u64 = u64::MAX / 2;

/// What [`restore_into`] does with component-schema drift.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum DriftPolicy {
    /// Migrate over it and report it (the dev loop's `engine.restore`).
    Migrate,
    /// Refuse a snapshot that would restore only with drift (a resumed run).
    Refuse,
}

/// A fresh world holding exactly the entities `doc` lists, in both id ranges: the allocator state
/// when the document carries it, the live-set fallback otherwise, then the spawn order and the
/// presentation range. Holds no component.
fn rebuild_ids(doc: &SnapshotDoc) -> Result<ludion_ecs::World, Diagnostic> {
    let live: Vec<EntityId> = doc
        .entities
        .iter()
        .map(|entity| EntityId::new(entity.index, entity.generation))
        .collect();
    let mut world = ludion_ecs::World::new();
    // Full allocator fidelity when the snapshot carries it (dead-slot generations + free-list order,
    // so recycled slots never re-issue a stale handle and post-restore spawns stay deterministic);
    // otherwise the live-set fallback, which also covers snapshots that predate the `allocator` field.
    let (rebuilt, root) = match &doc.allocator {
        Some(state) => (world.restore_allocator(state), "/allocator"),
        None => (world.restore_entities(&live), "/entities"),
    };
    rebuilt.map_err(|mut diagnostic| {
        diagnostic.path = Some(format!(
            "{root}{}",
            diagnostic.path.as_deref().unwrap_or_default()
        ));
        diagnostic
    })?;
    check_live_set(
        &live,
        &world.entities().collect::<Vec<_>>(),
        "entities",
        "allocator",
    )?;
    if let Some(order) = &doc.spawn_order {
        world.restore_spawn_order(order).map_err(|mut diagnostic| {
            diagnostic.path = Some(format!(
                "/spawn_order{}",
                diagnostic.path.as_deref().unwrap_or_default()
            ));
            diagnostic
        })?;
    }
    let presentation_live: Vec<EntityId> = doc
        .presentation
        .iter()
        .map(|entity| EntityId::new(entity.index, entity.generation))
        .collect();
    let presentation_root = if doc.presentation_allocator.is_some() {
        "/presentation_allocator"
    } else {
        "/presentation"
    };
    world
        .restore_presentation(&presentation_live, doc.presentation_allocator.as_ref())
        .map_err(|mut diagnostic| {
            diagnostic.path = Some(format!(
                "{presentation_root}{}",
                diagnostic.path.as_deref().unwrap_or_default()
            ));
            diagnostic
        })?;
    check_live_set(
        &presentation_live,
        &world.presentation_entities().collect::<Vec<_>>(),
        "presentation",
        "presentation_allocator",
    )?;
    Ok(world)
}

/// Rebuilds the engine's sim state from a snapshot document, installing it into `engine`, and returns
/// a [`RestoreReport`] of every component-schema drift the migration had to work around.
///
/// Restore is a **migration**, not a strict round-trip (architecture §8): the dev loop restores a
/// snapshot taken from one build into a freshly recompiled binary, so component drift is normal.
/// Rather than fail on it, this reconstructs the world as faithfully as it can, per entity per
/// component:
///
/// - a component this build no longer registers is **dropped** (report entry `L3015`);
/// - a value that no longer deserializes into its (still-registered) type is **dropped**, honestly —
///   default-filling a whole undeserializable component would hand a system invented state (entry
///   `L3016`, carrying the serde error);
/// - added fields fill from serde defaults inside deserialization; the filled/removed *field paths*
///   (dotted for nested fields) are computed by re-serializing the inserted value and diffing keys
///   against the snapshot's stored value, and reported as an informational entry;
/// - a value that deserializes but fails the component's schema (`L1014`) or semantic validator is
///   **kept**, exactly as captured — Rust code may legitimately hold a value outside its authored
///   range, and a restore must reproduce the live world — and each failure is reported into
///   `Diagnostics` with a path under `/entities/<i>/components/<Name>` (or `/presentation/<i>/…`).
///
/// An empty report means a clean, lossless restore. Under [`DriftPolicy::Refuse`] any report entry
/// is instead an `L1401` error, raised before the engine is touched.
///
/// # Errors
/// `L3008` for a genuinely malformed snapshot document — a wrong envelope shape, an unparseable
/// body, an unsupported `format`, an invalid speed, a `frame` past `MAX_RESTORE_FRAME`, or a
/// `pending_input` event addressed to a frame before the snapshot's; `L0025` for an entity allocator state or a `spawn_order` that
/// is inconsistent; `L3041` when the `allocator` field's live slots differ from the `entities`
/// list. All are raised before the engine is touched. Per-component drift never errors; it lands in
/// the returned report.
pub(crate) fn restore_into(
    engine: &mut Engine,
    value: &Value,
    policy: DriftPolicy,
) -> Result<RestoreReport, Diagnostic> {
    restore_with(engine, value, RestoreMode::Session(policy))
}

/// How [`restore_with`] installs the world it decoded.
enum RestoreMode<'a> {
    /// An exact continuation of a snapshotted session ([`restore_into`]).
    Session(DriftPolicy),
    /// A checkpoint slot inside the live session: the driver is built at the engine's own frame, run
    /// state and speed, and the world goes in through [`Engine::install_checkpoint`]. The slot's
    /// frame, input, profile and render extent are never read. Strict, like a run-save resume.
    Checkpoint(&'a crate::checkpoints::Slot),
}

/// Restores a checkpoint `slot` into the live session; the engine is untouched on `Err`.
///
/// # Errors
/// The same document errors as [`restore_into`], plus `L1401` for drift (a slot is this build's own
/// capture, so none is expected).
pub(crate) fn restore_checkpoint(
    engine: &mut Engine,
    slot: &crate::checkpoints::Slot,
) -> Result<(), Diagnostic> {
    match &slot.body {
        crate::checkpoints::SlotBody::Document(value) => {
            restore_with(engine, value, RestoreMode::Checkpoint(slot)).map(|_| ())
        }
        crate::checkpoints::SlotBody::Structured(s) => restore_decoded(
            engine,
            s.doc.clone(),
            Some(&s.columns),
            RestoreMode::Checkpoint(slot),
        )
        .map(|_| ()),
    }
}

fn restore_with(
    engine: &mut Engine,
    value: &Value,
    mode: RestoreMode<'_>,
) -> Result<RestoreReport, Diagnostic> {
    let doc: SnapshotDoc = serde_json::from_value(value.clone()).map_err(|error| {
        Diagnostic::new(
            code::PROTOCOL_SNAPSHOT_INVALID,
            format!("snapshot document is not a valid engine snapshot: {error}"),
        )
        .with_help("restore only from a document produced by this engine build's engine.snapshot")
    })?;
    restore_decoded(engine, doc, None, mode)
}

/// Installs a decoded snapshot. `columns` carries the component values of a structured slot
/// (installed in bulk after the entities are rebuilt); `None` restores them from the document's
/// per-entity maps. Everything is checked before the engine is touched.
fn restore_decoded(
    engine: &mut Engine,
    mut doc: SnapshotDoc,
    columns: Option<&[Option<ColumnCopy>]>,
    mode: RestoreMode<'_>,
) -> Result<RestoreReport, Diagnostic> {
    let policy = match &mode {
        RestoreMode::Session(policy) => *policy,
        RestoreMode::Checkpoint(_) => DriftPolicy::Refuse,
    };

    if doc.format != SNAPSHOT_FORMAT {
        return Err(Diagnostic::new(
            code::PROTOCOL_SNAPSHOT_INVALID,
            format!(
                "snapshot format version {} is not supported (this engine reads version {SNAPSHOT_FORMAT})",
                doc.format
            ),
        ));
    }

    let speed = Speed::new(doc.speed).map_err(|_| {
        Diagnostic::new(
            code::PROTOCOL_SNAPSHOT_INVALID,
            format!("snapshot speed {} is not a valid pacing factor", doc.speed),
        )
    })?;

    let render_extent = doc
        .render_extent
        .take()
        .map(|extent| {
            crate::commands::render::requested_extent(&RenderSizeParams {
                width: Some(extent.width),
                height: Some(extent.height),
            })
            .map(|checked| checked.unwrap_or_default())
            .map_err(|diagnostic| {
                Diagnostic::new(
                    code::PROTOCOL_SNAPSHOT_INVALID,
                    format!(
                        "snapshot render extent {}x{} is not a valid extent: {}",
                        extent.width, extent.height, diagnostic.message
                    ),
                )
                .with_path("/render_extent")
                .with_help("re-take the snapshot from a real session")
            })
        })
        .transpose()?;

    if doc.frame > MAX_RESTORE_FRAME {
        return Err(Diagnostic::new(
            code::PROTOCOL_SNAPSHOT_INVALID,
            format!(
                "snapshot frame {} is past the highest restorable frame {MAX_RESTORE_FRAME}",
                doc.frame
            ),
        )
        .with_path("/frame")
        .with_help("a run never reaches this frame; re-take the snapshot from a real session"));
    }

    if let Some((i, event)) = doc
        .pending_input
        .events()
        .iter()
        .enumerate()
        .find(|(_, event)| event.frame.as_u64() < doc.frame)
    {
        return Err(Diagnostic::new(
            code::PROTOCOL_SNAPSHOT_INVALID,
            format!(
                "snapshot input event {i} is addressed to frame {}, before the snapshot's frame {}",
                event.frame.as_u64(),
                doc.frame
            ),
        )
        .with_path(format!("/pending_input/{i}/frame"))
        .with_help(
            "a pending input is addressed to a frame the snapshot has not run yet; re-take the snapshot",
        ));
    }

    // Decoded before the schedule: the schedule needs it, and a malformed `space` is the same
    // "genuinely malformed document" class as the rest of this function's up-front checks.
    let scene_physics: Option<ScenePhysics> = doc
        .scene_physics
        .take()
        .map(ScenePhysicsDoc::try_into)
        .transpose()?;

    // Re-runs the scene parser's own value checks (an out-of-range iteration count, a gravity
    // component that does not fit f32) over the decoded struct: a hand-edited or foreign snapshot
    // can carry a value the parser never validated, and restore must reject it before the schedule
    // or the world is touched, exactly as a scene load would.
    if let Some(physics) = &scene_physics
        && let Some(diagnostic) = validate_physics(physics, "/scene_physics")
            .into_iter()
            .next()
    {
        return Err(diagnostic);
    }

    // Captured before `scene_physics` moves into `RestoredWorld` below: solver state with nothing
    // to install the write/step/mirror triple from means physics silently stops stepping.
    let missing_settings = doc.physics.is_some() && scene_physics.is_none();
    if missing_settings && policy == DriftPolicy::Refuse {
        return Err(Diagnostic::new(
            code::RUN_SAVE_RESUME_REFUSED,
            "restoring would lose physics: the snapshot carries solver state but no scene physics \
             settings, so physics would not step",
        ));
    }

    // Reconstruct the world's live entities exactly, then re-insert their components, migrating each
    // rather than hard-failing. The registry borrow is disjoint from the fresh local `world`.
    let registry = engine.registry();
    let mut world = rebuild_ids(&doc)?;
    // Document-level warnings come first: the restore goes ahead either way, but a snapshot from
    // another build or carrying fields this one does not read may not restore everything it held.
    let mut entries = Vec::new();
    if doc.engine_version != crate::engine::ENGINE_VERSION {
        entries.push(RestoreEntry::EngineVersionDiffers {
            snapshot_version: doc.engine_version.clone(),
            engine_version: crate::engine::ENGINE_VERSION.to_string(),
        });
    }
    for field in doc.unknown.keys() {
        entries.push(RestoreEntry::SnapshotFieldUnknown {
            field: field.clone(),
        });
    }
    let mut invalid: Vec<Diagnostic> = Vec::new();
    let listed = doc
        .entities
        .iter()
        .enumerate()
        .map(|(i, entity)| (entity, format!("/entities/{i}")))
        .chain(
            doc.presentation
                .iter()
                .enumerate()
                .map(|(i, entity)| (entity, format!("/presentation/{i}"))),
        );
    for (entity, doc_path) in listed {
        let id = EntityId::new(entity.index, entity.generation);
        for (name, snapshot_value) in &entity.components {
            migrate_component(
                registry,
                &mut world,
                id,
                name,
                snapshot_value,
                &doc_path,
                &mut entries,
                &mut invalid,
            );
        }
    }
    if let Some(columns) = columns {
        install_columns(registry, &mut world, columns)?;
    }
    if policy == DriftPolicy::Refuse
        && let Some(first) = entries.first()
    {
        return Err(Diagnostic::new(
            code::RUN_SAVE_RESUME_REFUSED,
            format!(
                "restoring would change the run: {} schema drift(s), first: {first:?}",
                entries.len()
            ),
        ));
    }

    // The tick rate the restored driver will run at, bound once so the solver's integration step and
    // the driver's timestep cannot disagree.
    let tick_rate = ludion_core::time::TickRate::DEFAULT;
    // The solver rides as an ordinary world resource, so it goes in before the driver takes the
    // world; an absent field inserts nothing, and the world is new, so there is nothing stale to
    // remove.
    if let Some(blob) = &doc.physics {
        let mut physics = restore_physics(blob, tick_rate)?;
        // The side table the write refills each tick; a layer-table drift below can wake partners
        // before that, and the wake order follows the restored spawn ranks.
        physics.sync_spawn_ranks(&world);
        physics.sync_query_layers(&world);
        physics.sync_surface_tags(&world);
        // The decoded solver has no layer table until the next physics write; a game system that
        // moves a character before then would ignore `physics.layers`.
        if let Some(scene) = &scene_physics {
            physics.apply_settings(&crate::scene_physics::settings(scene));
        }
        world.insert_resource(physics);
    }
    if let Some(walk) = doc.animation_walk.clone() {
        world.insert_resource(walk);
    }
    if let Some(entries) = doc.spatial_index.take() {
        world.insert_resource(SpatialIndex::from_entries(entries));
    }
    if let Some(projectiles) = &doc.projectiles {
        match restore_projectiles(projectiles, engine.projectile_config.as_ref())? {
            RestoredPool::Pool(pool) => {
                world.insert_resource(*pool);
            }
            RestoredPool::Dropped if policy == DriftPolicy::Refuse => {
                return Err(Diagnostic::new(
                    code::RUN_SAVE_RESUME_REFUSED,
                    "restoring would lose the projectiles in flight: this build configures no \
                     projectile pool",
                ));
            }
            RestoredPool::Dropped => {}
            RestoredPool::Refused(warning) if policy == DriftPolicy::Refuse => {
                return Err(Diagnostic::new(
                    code::RUN_SAVE_RESUME_REFUSED,
                    format!(
                        "restoring would lose the projectiles in flight: {}",
                        warning.message
                    ),
                ));
            }
            RestoredPool::Refused(warning) => invalid.push(warning),
        }
    }
    if let Some(items) = doc.spatial_index_3d.take() {
        let mut index = ludion_spatial::spatial3d::Index3d::new();
        items.into_iter().for_each(|item| index.push(item));
        index.build();
        world.insert_resource(index);
    }
    if let Some(entries) = doc.follower_motions.take() {
        world.insert_resource(FollowerMotions::from_entries(entries));
    }

    // Rebuilt so a restored engine keeps running its systems — restore resets the world, but the
    // schedule is engine configuration that must survive it (the same fix as `scene.load`).
    let schedule = engine.build_schedule(scene_physics.as_ref());
    let (driver_frame, driver_run_state, driver_speed) = match &mode {
        RestoreMode::Session(_) => (FrameIndex::new(doc.frame), doc.run_state, speed),
        // A disk slot can be ahead of a fresh boot; the frame never moves back.
        RestoreMode::Checkpoint(slot) => (
            engine.frame().max(slot.info.frame),
            engine.run_state(),
            engine.speed(),
        ),
    };
    let driver = ludion_sim::SimDriver::restore(
        world,
        schedule,
        DriverRestore {
            rng: doc.rng,
            // The snapshot has always carried the run's seed; the `WorldSeed` resource is rebuilt
            // from it rather than from a new snapshot field, so old snapshots restore unchanged.
            world_seed: doc.seed,
            frame: driver_frame,
            run_state: driver_run_state,
            speed: driver_speed,
            tick_rate,
        },
    );

    // Adopt the snapshot's reload log so the restore re-applies each reloaded doc off-disk (audit
    // finding 2.1); an empty log — including an older snapshot that predates the field — restores the
    // pre-fix clean-disk rebuild.
    let doc_reloads =
        crate::docs::DocReloadLog::from_entries(doc.doc_reloads.into_iter().map(|reload| {
            crate::docs::DocReload {
                kind: reload.kind,
                key: reload.key,
                document: reload.document,
                applied: reload.applied,
            }
        }));

    // Adopt the snapshot's scene residue for the same reason: a return performed after this restore
    // must re-apply what the snapshotted session kept, not arrive at a scene with no memory. An empty
    // set — a game that marks nothing, or an older snapshot predating the field — adopts nothing.
    let residue = doc
        .residue
        .into_iter()
        .map(|scene| {
            (
                scene.scene,
                crate::transition::SceneResidue {
                    records: scene
                        .records
                        .into_iter()
                        .map(|record| crate::transition::ResidueRecord {
                            name: record.name,
                            authored: record.authored,
                            bag: record.bag,
                            refs: record.refs,
                        })
                        .collect(),
                    last_visit: ludion_core::time::FrameIndex::new(scene.last_visit),
                },
            )
        })
        .collect();

    // A slot's own document has no store; only a session restore installs one.
    let checkpoints = match (&mode, doc.checkpoints.take()) {
        (RestoreMode::Session(_), Some(store)) => Some(store.into_store().map_err(|reason| {
            Diagnostic::new(
                code::PROTOCOL_SNAPSHOT_INVALID,
                format!("snapshot document is not a valid engine snapshot: {reason}"),
            )
        })?),
        _ => None,
    };

    let frame = doc.frame;
    let restored = RestoredWorld {
        driver,
        seed: doc.seed,
        scene: doc.scene,
        scene_name: doc.scene_name,
        profile: doc.profile,
        scene_physics,
        doc_reloads,
        residue,
        game_clock: doc.game_clock,
        input: doc.input,
        pending_input: doc.pending_input,
        arrived_by_restart: doc.arrived_by_restart,
        render_extent,
        checkpoints,
    };
    match mode {
        RestoreMode::Session(_) => engine.install_restored(restored),
        RestoreMode::Checkpoint(slot) => engine.install_checkpoint(restored, slot),
    }
    .map_err(crate::commands::collapse)?;

    for diagnostic in invalid {
        let key = format!(
            "restore@{frame} {} {}",
            diagnostic.path.as_deref().unwrap_or_default(),
            diagnostic.code.as_str()
        );
        crate::diagnostics::report_full(engine.world_mut(), key, diagnostic);
    }

    if missing_settings {
        crate::diagnostics::report(
            engine.world_mut(),
            code::PHYSICS_SNAPSHOT_NO_SETTINGS,
            format!("restore@{frame}"),
            "the snapshot carries solver state but no scene physics settings, so physics will not \
             step after this restore; re-take the snapshot from a session whose scene declares a \
             `physics:` block",
        );
    }

    Ok(RestoreReport { entries })
}

/// Migrates one snapshotted component onto `entity`, pushing a [`RestoreEntry`] for any drift.
///
/// The three outcomes (drop-unregistered / drop-undeserializable / kept-with-field-diff) are exactly
/// the migration policy documented on [`restore_into`]. A component that restores byte-identically
/// pushes nothing. A kept component whose value fails its schema or semantic check also lands in
/// `invalid`, each diagnostic's path prefixed with `doc_path/components/<name>`.
#[allow(clippy::too_many_arguments)]
fn migrate_component(
    registry: &ludion_ecs::Registry,
    world: &mut ludion_ecs::World,
    entity: EntityId,
    name: &str,
    snapshot_value: &Value,
    doc_path: &str,
    entries: &mut Vec<RestoreEntry>,
    invalid: &mut Vec<Diagnostic>,
) {
    // 1. A component this build no longer registers is dropped (L3015).
    if registry.get(name).is_none() {
        entries.push(RestoreEntry::ComponentUnregistered {
            entity,
            component: name.to_string(),
        });
        return;
    }

    // 1b. The value is checked as a scene or wire value would be, but only reported: it is kept either
    //     way (see the policy on `restore_into`).
    let mut failures = ludion_scene::check_component(name, snapshot_value, registry);
    if failures.is_empty()
        && let Some(info) = registry.get(name)
    {
        failures = info.validate_semantics(snapshot_value);
    }

    // 2. A value that no longer deserializes is dropped, carrying the serde error (L3016). Added
    //    fields fill from serde defaults *inside* this deserialize; only a value that fails entirely
    //    lands here.
    if let Err(diagnostic) = registry.deserialize_insert(name, snapshot_value, world, entity) {
        entries.push(RestoreEntry::ComponentUndeserializable {
            entity,
            component: name.to_string(),
            error: diagnostic.message,
        });
        return;
    }

    // Reported only now: a value that was dropped above is not "kept", and says so already.
    for mut diagnostic in failures {
        diagnostic.path = Some(format!(
            "{doc_path}/components/{name}{}",
            diagnostic.path.as_deref().unwrap_or_default()
        ));
        invalid.push(diagnostic);
    }

    // 3. Kept — but compute the honest field diff by re-serializing the inserted value and comparing
    //    its keys against the snapshot's. Keys only the re-serialized value has were filled from
    //    defaults; keys only the snapshot had were dropped from the type. Report only a real change.
    if let Ok(Some(reserialized)) = registry.serialize(name, world, entity) {
        let (filled, removed) = crate::field_diff::diff_fields(snapshot_value, &reserialized);
        if !filled.is_empty() || !removed.is_empty() {
            entries.push(RestoreEntry::ComponentMigrated {
                entity,
                component: name.to_string(),
                filled,
                removed,
            });
        }
    }
}

#[cfg(test)]
mod surface_schema_tests {
    /// The snapshot schema carries the scene's declared surfaces.
    #[test]
    fn the_snapshot_schema_names_the_surface_fields() {
        let schema = super::snapshot_schema().to_string();
        assert!(schema.contains("\"surfaces\""), "{schema}");
        assert!(schema.contains("default_surface"), "{schema}");
    }

    /// Guards the `LayerSet` config field: `replay_content_hash` covers the prose-stripped schema,
    /// so a `$ref` or definition named `LayerSet` is what would move it and fail every committed
    /// replay; the field must stay a plain list of names.
    #[test]
    fn the_snapshot_schema_spells_projectile_ignore_layers_inline() {
        let schema = ludion_protocol::schema::strip_prose(super::snapshot_schema()).to_string();
        assert!(schema.contains("ignore_layers"), "{schema}");
        assert!(!schema.contains("LayerSet"), "{schema}");
    }
}

#[cfg(test)]
mod clock_and_store_schema_tests {
    /// The clock's scale fields and the checkpoint store are part of the typed schema a replay's
    /// content hash covers.
    #[test]
    fn the_snapshot_schema_names_the_clock_scale_fields_and_the_store() {
        let schema = super::snapshot_schema().to_string();
        for name in ["slowed_q16", "pending_q16", "step_q16", "checkpoints"] {
            assert!(schema.contains(name), "{name} is in the schema");
        }
    }
}
