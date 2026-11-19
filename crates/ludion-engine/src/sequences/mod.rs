//! Sequences: linear timelines of keys — camera, animation, ui, audio, dialogue and events — played
//! over roles bound to entities (`ludion docs sequences`).
//!
//! A game opts in with [`EngineBuilder::sequences`](crate::EngineBuilder::sequences), schedules
//! [`sequence_update`] **before** `dialogue_update` and before any system that reads
//! [`SequenceEvents`], and drives a beat through the free functions here. **The API only marks
//! state** — [`play`] spawns a pending [`SequencePlayer`], [`skip`] and [`cancel`] set a flag — and
//! the system applies every key and emits every event. [`release`] clears a `wait: release` key,
//! [`holding`] reports a `holds_gameplay` beat holding the gameplay clock.
//!
//! State is entities, so it rides hashing, snapshots and checkpoints: one [`SequencePlayer`] per
//! playing beat (its handle *is* the entity) and one [`SequenceBinding`] per bound role. A game that
//! never plays one hashes as if it had not opted in.

mod docs;
mod events;
mod update;

use ludion_camera::RigConfig;
use ludion_core::codes::code;
use ludion_core::diagnostics::Diagnostic;
use ludion_core::ids::EntityId;
use ludion_core::time::FrameIndex;
use ludion_ecs::World;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

pub use docs::{
    AnimationKey, AudioKey, CameraKey, DialogueKey, DialogueWait, EventKey, EventWait, OnSkip,
    SequenceClock, SequenceDoc, SequenceTracks, UiKey,
};
pub use events::{SequenceEnd, SequenceEvent, SequenceEvents, SequenceTrack};
pub use update::sequence_update;

pub(crate) use docs::{SEQUENCE_KIND, validate_sequence_docs};

/// Where a playing sequence stands.
#[derive(Serialize, Deserialize, JsonSchema, Clone, Copy, Debug, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum PlayerState {
    /// Spawned by [`play`]; the next [`sequence_update`] stamps its start.
    #[default]
    Pending,
    /// Running.
    Playing,
}

/// One playing sequence: everything the system needs, so a restored world keeps the beat. A runtime
/// entity, hashed and snapshotted like any other; the engine despawns it (and its bindings) when the
/// sequence ends. Read through [`is_playing`] and [`SequenceEvents`] rather than writing it.
///
/// ```
/// use ludion_engine::sequences::SequencePlayer;
///
/// let player: SequencePlayer = serde_json::from_value(serde_json::json!({
///     "sequence": "sequences/intro.yaml",
/// }))
/// .unwrap();
/// assert_eq!(player.applied_through, None);
/// ```
#[derive(Serialize, Deserialize, JsonSchema, Clone, Debug, PartialEq)]
pub struct SequencePlayer {
    /// The document's content-root path.
    pub sequence: String,
    /// The clock the playhead counts on, copied from the document at [`play`].
    #[serde(default)]
    pub clock: SequenceClock,
    /// Where it stands.
    #[serde(default)]
    pub state: PlayerState,
    /// The clock frame the first update stamped.
    #[serde(default = "frame_zero")]
    #[schemars(with = "u64")]
    pub started: FrameIndex,
    /// The playhead through which keys have applied; none yet.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub applied_through: Option<u32>,
    /// A skip waiting for the next [`sequence_update`].
    #[serde(default, skip_serializing_if = "is_false")]
    pub skip: bool,
    /// A cancel waiting for the next [`sequence_update`].
    #[serde(default, skip_serializing_if = "is_false")]
    pub cancel: bool,
    /// The `seq` of every dialogue line it said, so a skip or cancel can interrupt them.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub said: Vec<u64>,
    /// Clock frames the playhead spent parked on waits; the playhead is `now - started - waited`.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub waited: u64,
    /// The wait the playhead is parked on.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub wait: Option<PendingWait>,
    /// A [`release`] latch: clears the current `wait: release`, or the next one reached.
    #[serde(default, skip_serializing_if = "is_false")]
    pub released: bool,
}

/// A wait the playhead is parked on.
#[derive(Serialize, Deserialize, JsonSchema, Clone, Debug, PartialEq, Eq)]
pub struct PendingWait {
    /// The `at` of the key that set it; the playhead stays here.
    pub at: u32,
    /// What clears it.
    pub kind: WaitKind,
}

/// What a [`PendingWait`] is waiting for.
#[derive(Serialize, Deserialize, JsonSchema, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum WaitKind {
    /// The dialogue line with this `seq` ends.
    LineEnd {
        /// The line's `DialogueLine.seq`.
        seq: u64,
    },
    /// The game calls [`release`].
    Release,
}

/// One role bound to one entity: its own entity because a list of entity refs is refused (`L1392`).
///
/// ```
/// use ludion_engine::sequences::SequenceBinding;
///
/// let binding: SequenceBinding = serde_json::from_value(serde_json::json!({
///     "player": { "index": 1, "generation": 0 }, "role": "camera",
///     "entity": { "index": 2, "generation": 0 },
/// }))
/// .unwrap();
/// assert!(binding.saved_rig.is_none());
/// ```
#[derive(Serialize, Deserialize, JsonSchema, Clone, Debug, PartialEq)]
pub struct SequenceBinding {
    /// The [`SequencePlayer`] entity.
    #[schemars(with = "ludion_ecs::EntityRefSchema")]
    pub player: EntityId,
    /// The role name.
    pub role: String,
    /// The bound entity.
    #[schemars(with = "ludion_ecs::EntityRefSchema")]
    pub entity: EntityId,
    /// The `CameraRig.rig` the first applied camera key replaced, for `restore`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub saved_rig: Option<RigConfig>,
}

fn frame_zero() -> FrameIndex {
    FrameIndex::ZERO
}

fn is_false(value: &bool) -> bool {
    !*value
}

fn is_zero(value: &u64) -> bool {
    *value == 0
}

/// A playing sequence's handle: the entity that carries it, valid until it ends.
///
/// ```
/// use ludion_engine::sequences::SequenceHandle;
/// use ludion_core::ids::EntityId;
///
/// let handle = SequenceHandle(EntityId::new(3, 0));
/// assert_eq!(handle.0, EntityId::new(3, 0));
/// ```
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct SequenceHandle(pub EntityId);

fn not_enabled() -> Diagnostic {
    Diagnostic::new(
        code::SEQUENCES_NOT_ENABLED,
        "a sequence call was made, but the game did not opt in",
    )
    .with_help("add `.sequences()` to the engine builder")
}

fn find_doc(world: &World, key: &str) -> Option<SequenceDoc> {
    world
        .resource::<crate::DocStore<SequenceDoc>>()?
        .get(key)
        .cloned()
}

/// Starts the sequence `key` (the document's content-root path) with `bindings` mapping each role to
/// an entity. The first [`sequence_update`] after this stamps the start and applies the keys at 0.
///
/// # Errors
/// `L4354` without `.sequences()`, `L4350` for an unknown document, `L4351` for a role the document
/// does not declare or one bound twice (nothing is started). A declared role left unbound is not an
/// error: its keys are inert and `L4352` warns once.
///
/// # Example
/// ```
/// use ludion_engine::{Engine, sequences};
///
/// let mut engine = Engine::builder().sequences().build().unwrap();
/// // No document is loaded, so the sequence is unknown.
/// let refused = sequences::play(engine.world_mut(), "sequences/intro.yaml", &[]).unwrap_err();
/// assert_eq!(refused.code.as_str(), "L4350");
/// ```
pub fn play(
    world: &mut World,
    key: &str,
    bindings: &[(&str, EntityId)],
) -> Result<SequenceHandle, Diagnostic> {
    if world.resource::<SequenceEvents>().is_none() {
        return Err(not_enabled());
    }
    let doc = find_doc(world, key).ok_or_else(|| {
        Diagnostic::new(
            code::SEQUENCE_UNKNOWN,
            format!("no `kind: sequence` document `{key}` is loaded"),
        )
        .with_help("pass the document's path under the content root, e.g. `sequences/intro.yaml`")
    })?;
    for (n, (role, _)) in bindings.iter().enumerate() {
        let declared = doc.roles.iter().any(|r| r == role);
        let twice = bindings[..n].iter().any(|(earlier, _)| earlier == role);
        if !declared || twice {
            return Err(Diagnostic::new(
                code::SEQUENCE_BIND_INVALID,
                format!(
                    "role `{role}` is {} for `{key}`",
                    if declared {
                        "bound twice"
                    } else {
                        "not declared under `roles:`"
                    }
                ),
            )
            .with_help("bind each declared role at most once"));
        }
    }
    let player = world.spawn();
    let _ = world.insert(
        player,
        SequencePlayer {
            sequence: key.to_owned(),
            clock: doc.effective_clock(),
            state: PlayerState::Pending,
            started: FrameIndex::ZERO,
            applied_through: None,
            skip: false,
            cancel: false,
            said: Vec::new(),
            waited: 0,
            wait: None,
            released: false,
        },
    );
    for (role, entity) in bindings {
        let binding = world.spawn();
        let _ = world.insert(
            binding,
            SequenceBinding {
                player,
                role: (*role).to_owned(),
                entity: *entity,
                saved_rig: None,
            },
        );
    }
    Ok(SequenceHandle(player))
}

/// Cuts the sequence to its end state at the next [`sequence_update`]: the last camera, animation
/// and ui key of every role applies in its end form, unfired `on_skip: fire` events fire in order,
/// unplayed audio and dialogue keys are dropped and its lines are interrupted; then
/// `Ended { Skipped }`. `false` when the handle is not live or the document says
/// `skippable: false`.
///
/// # Example
/// ```
/// use ludion_engine::{Engine, sequences};
///
/// let mut engine = Engine::builder().sequences().build().unwrap();
/// let gone = sequences::SequenceHandle(ludion_core::ids::EntityId::new(9, 0));
/// assert!(!sequences::skip(engine.world_mut(), gone));
/// ```
pub fn skip(world: &mut World, handle: SequenceHandle) -> bool {
    let Ok(Some(player)) = world.get::<SequencePlayer>(handle.0) else {
        return false;
    };
    let skippable = find_doc(world, &player.sequence).is_none_or(|doc| doc.skippable);
    if !skippable {
        return false;
    }
    if let Ok(Some(player)) = world.get_mut::<SequencePlayer>(handle.0) {
        player.skip = true;
    }
    true
}

/// Stops the sequence where it is at the next [`sequence_update`]: no end state is applied, its
/// lines are interrupted, and it emits `Ended { Cancelled }`. `false` when the handle is not live.
///
/// # Example
/// ```
/// use ludion_engine::{Engine, sequences};
///
/// let mut engine = Engine::builder().sequences().build().unwrap();
/// let gone = sequences::SequenceHandle(ludion_core::ids::EntityId::new(9, 0));
/// assert!(!sequences::cancel(engine.world_mut(), gone));
/// ```
pub fn cancel(world: &mut World, handle: SequenceHandle) -> bool {
    let Ok(Some(player)) = world.get_mut::<SequencePlayer>(handle.0) else {
        return false;
    };
    player.cancel = true;
    true
}

/// Releases a `wait: release` key: the playhead resumes on the next clock frame. A single latch, not a
/// counter: a release before the wait is reached clears the next one, so an early release cannot
/// stall the beat. `false` when the handle is not live.
///
/// # Example
/// ```
/// use ludion_engine::{Engine, sequences};
///
/// let mut engine = Engine::builder().sequences().build().unwrap();
/// let gone = sequences::SequenceHandle(ludion_core::ids::EntityId::new(9, 0));
/// assert!(!sequences::release(engine.world_mut(), gone));
/// ```
pub fn release(world: &mut World, handle: SequenceHandle) -> bool {
    let Ok(Some(player)) = world.get_mut::<SequencePlayer>(handle.0) else {
        return false;
    };
    player.released = true;
    true
}

/// Whether a live `holds_gameplay` sequence is holding the gameplay clock.
///
/// # Example
/// ```
/// use ludion_engine::{Engine, sequences};
///
/// let mut engine = Engine::builder().sequences().build().unwrap();
/// assert!(!sequences::holding(engine.world_mut()));
/// ```
pub fn holding(world: &mut World) -> bool {
    let playing: Vec<String> = world
        .query_ids::<(&SequencePlayer,)>()
        .map(|(_, (player,))| player.sequence.clone())
        .collect();
    playing
        .iter()
        .any(|key| find_doc(world, key).is_some_and(|doc| doc.holds_gameplay))
}

/// Whether the handle names a sequence that has not ended.
///
/// # Example
/// ```
/// use ludion_engine::{Engine, sequences};
///
/// let engine = Engine::builder().sequences().build().unwrap();
/// let gone = sequences::SequenceHandle(ludion_core::ids::EntityId::new(9, 0));
/// assert!(!sequences::is_playing(engine.world(), gone));
/// ```
pub fn is_playing(world: &World, handle: SequenceHandle) -> bool {
    matches!(world.get::<SequencePlayer>(handle.0), Ok(Some(_)))
}
