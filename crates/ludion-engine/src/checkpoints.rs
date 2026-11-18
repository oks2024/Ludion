//! In-memory checkpoints: a game system captures the whole world into a named slot and later
//! restores it, both at the frame boundary.
//!
//! A slot is the [`engine.snapshot`](crate::Engine) document plus a little metadata. A restore is a
//! sim event inside the session: the world rolls back to the slot, while the sim frame, held input,
//! the live profile, `carry_resource` resources and an open [`RunRecording`](crate::RunRecording)
//! continue, and the [`GameClock`](ludion_core::time::GameClock) is rebased so every `started` stamp
//! in the slot stays valid. Requests come from hashed state, so a replay regenerates the slots and
//! needs no record kind of its own (`Plan/checkpoints.md`).

use std::collections::BTreeMap;
use std::sync::Arc;

use ludion_core::codes::code;
use ludion_core::collections::StableSet;
use ludion_core::diagnostics::Diagnostic;
use ludion_core::time::{FrameIndex, GameClock};
use ludion_protocol::event::{Event, SceneChangeKind, SceneChangedData};
use serde_json::Value;

use crate::engine::{Engine, PreparedWorld, Rebuild, RestoredWorld};

/// The most slots a [`CheckpointConfig`] may ask for.
const MAX_SLOTS_LIMIT: u32 = 64;

/// The longest checkpoint name.
const MAX_NAME_LEN: usize = 64;

/// What [`EngineBuilder::checkpoints`](crate::EngineBuilder::checkpoints) configures.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CheckpointConfig {
    /// How many named slots are held at once, 1 to 64. A capture of a new name past it is refused
    /// (`L4201`); the oldest slot is never evicted silently.
    pub max_slots: u32,
}

impl Default for CheckpointConfig {
    fn default() -> Self {
        CheckpointConfig { max_slots: 4 }
    }
}

impl CheckpointConfig {
    /// # Errors
    /// `L1560` when `max_slots` is 0 or above 64.
    pub(crate) fn validate(&self) -> Result<(), Diagnostic> {
        if self.max_slots == 0 || self.max_slots > MAX_SLOTS_LIMIT {
            return Err(Diagnostic::new(
                code::CHECKPOINT_CONFIG_INVALID,
                format!(
                    "checkpoints: `max_slots` is {}, outside 1..={MAX_SLOTS_LIMIT}",
                    self.max_slots
                ),
            )
            .with_help("use a slot count from 1 to 64; the default is 4"));
        }
        Ok(())
    }
}

/// What a game can read about one held slot.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CheckpointInfo {
    /// The slot's name.
    pub name: String,
    /// The sim frame the slot was captured at.
    pub frame: FrameIndex,
    /// The same frame in gameplay time (the slot's [`GameClock`] applied).
    pub game_frame: FrameIndex,
    /// The captured scene's `name:`, when it has one.
    pub scene: Option<String>,
}

/// What the last [`Checkpoints::restore`] did.
#[non_exhaustive]
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RestoreOutcome {
    /// The world became the slot's.
    Restored {
        /// The slot restored.
        name: String,
        /// The sim frame the slot was captured at (the live frame did not move back).
        frame: FrameIndex,
    },
    /// The restore was refused and the world is untouched; `reason` is readable.
    Refused {
        /// The slot asked for.
        name: String,
        /// Why nothing was restored.
        reason: String,
    },
}

/// One held slot. The document is shared so a restore does not copy it.
#[derive(Clone)]
pub(crate) struct Slot {
    pub(crate) info: CheckpointInfo,
    pub(crate) doc: Arc<Value>,
    pub(crate) clock: GameClock,
    pub(crate) scene_path: Option<String>,
    pub(crate) authored_names: StableSet<String>,
}

/// The engine's persistent checkpoint state: it outlives every world rebuild, and the
/// [`Checkpoints`] resource is a mirror of it reinstalled after each one.
pub(crate) struct CheckpointState {
    pub(crate) config: CheckpointConfig,
    pub(crate) slots: BTreeMap<String, Slot>,
    pub(crate) last_restore: Option<RestoreOutcome>,
    /// The restore requested this boundary, between the two drains.
    pub(crate) pending_restore: Option<String>,
}

/// The capture/restore/discard handle a game system holds, installed by
/// [`EngineBuilder::checkpoints`](crate::EngineBuilder::checkpoints).
///
/// Requests are drained at the frame boundary, never mid-tick, so no system observes a half-restored
/// world. Within one boundary: every `discard` runs first, then every `capture` in call order, then
/// the last `restore` called. A capture and a restore of one name in one frame restore what was
/// just captured. A restore wins over a scene switch requested the same frame (`L4205`). Like
/// [`RunSave`](crate::RunSave) it is a resource with no digest, invisible to the `WorldHash`.
///
/// # Example
/// ```
/// use ludion_engine::{CheckpointConfig, Checkpoints, Engine};
///
/// let engine = Engine::builder()
///     .checkpoints(CheckpointConfig::default())
///     .build()
///     .unwrap();
///
/// let checkpoints = engine.world().resource::<Checkpoints>().unwrap();
/// assert!(checkpoints.slots().is_empty());
/// assert!(checkpoints.last_restore().is_none());
/// ```
pub struct Checkpoints {
    slots: Vec<CheckpointInfo>,
    last_restore: Option<RestoreOutcome>,
    pub(crate) captures: Vec<String>,
    pub(crate) discards: Vec<String>,
    pub(crate) restore: Option<String>,
}

impl Checkpoints {
    /// Requests that the world be captured into slot `name` at the frame boundary, replacing the
    /// slot's previous capture. A name that is empty, over 64 characters or outside
    /// `[A-Za-z0-9_.-]` is refused (`L4202`); a new name past `max_slots` is refused (`L4201`).
    pub fn capture(&mut self, name: impl Into<String>) {
        self.captures.push(name.into());
    }

    /// Requests that the world become slot `name`'s at the frame boundary. The slot is kept, so the
    /// same checkpoint restores any number of times. An unknown name is refused (`L4200`) and
    /// [`last_restore`](Checkpoints::last_restore) reports it. The sim frame, held input, the live
    /// profile, `carry_resource` resources and an open recording continue; the RNG, the scene and
    /// its residue rewind. A later call in the same frame replaces this one.
    pub fn restore(&mut self, name: impl Into<String>) {
        self.restore = Some(name.into());
    }

    /// Requests that slot `name` be dropped at the frame boundary, before any capture. An unknown
    /// name is ignored.
    pub fn discard(&mut self, name: impl Into<String>) {
        self.discards.push(name.into());
    }

    /// The held slots, sorted by name.
    pub fn slots(&self) -> &[CheckpointInfo] {
        &self.slots
    }

    /// The slot named `name`, if held.
    pub fn slot(&self, name: &str) -> Option<&CheckpointInfo> {
        self.slots.iter().find(|info| info.name == name)
    }

    /// The outcome of the last restore request, or `None` before the first one (and after a scene
    /// load or an `engine.restore`, which start a fresh session).
    pub fn last_restore(&self) -> Option<&RestoreOutcome> {
        self.last_restore.as_ref()
    }

    pub(crate) fn mirror(state: &CheckpointState) -> Self {
        Checkpoints {
            slots: state.slots.values().map(|slot| slot.info.clone()).collect(),
            last_restore: state.last_restore.clone(),
            captures: Vec::new(),
            discards: Vec::new(),
            restore: None,
        }
    }
}

/// `Ok` for a usable checkpoint name, else the reason.
fn validate_name(name: &str) -> Result<(), String> {
    if name.is_empty() {
        return Err("the name is empty".to_string());
    }
    if name.chars().count() > MAX_NAME_LEN {
        return Err(format!("the name is longer than {MAX_NAME_LEN} characters"));
    }
    if let Some(bad) = name
        .chars()
        .find(|c| !(c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | '-')))
    {
        return Err(format!(
            "the name holds `{bad}`; use only letters, digits, `_`, `.` and `-`"
        ));
    }
    Ok(())
}

impl Engine {
    /// Installs the checkpoint state; called once at build when
    /// [`checkpoints`](crate::EngineBuilder::checkpoints) is on.
    pub(crate) fn init_checkpoints(&mut self, config: CheckpointConfig) {
        self.checkpoints = Some(CheckpointState {
            config,
            slots: BTreeMap::new(),
            last_restore: None,
            pending_restore: None,
        });
        self.install_checkpoints(false);
    }

    /// (Re)installs the [`Checkpoints`] resource from the engine's state; a no-op without
    /// [`checkpoints`](crate::EngineBuilder::checkpoints). Called after every world (re)build.
    /// `clear` drops the slots first: a scene load and an `engine.restore` start a fresh session.
    pub(crate) fn install_checkpoints(&mut self, clear: bool) {
        let Some(state) = self.checkpoints.as_mut() else {
            return;
        };
        if clear {
            state.slots.clear();
            state.last_restore = None;
        }
        let mirror = Checkpoints::mirror(state);
        self.world_mut().insert_resource(mirror);
    }

    /// Re-syncs the mirrored slot list and outcome into the live resource, keeping its requests.
    fn refresh_checkpoints_mirror(&mut self) {
        let Some(state) = self.checkpoints.as_ref() else {
            return;
        };
        let fresh = Checkpoints::mirror(state);
        if let Some(resource) = self.world_mut().resource_mut::<Checkpoints>() {
            resource.slots = fresh.slots;
            resource.last_restore = fresh.last_restore;
        }
    }

    /// Reports a checkpoint warning on both sinks now, latched on `key`: the next world swap would
    /// drop it from a queue.
    fn report_checkpoint(&mut self, key: String, diagnostic: Diagnostic) {
        crate::diagnostics::report_full(self.world_mut(), key, diagnostic);
        self.emit_pending_diagnostics();
    }

    /// Drains the capture and discard requests and stashes the last restore, right after the
    /// profile drain. Every request is taken off the resource first.
    pub(crate) fn drain_checkpoints(&mut self, frame: FrameIndex) {
        let Some(state) = self.checkpoints.as_mut() else {
            return;
        };
        state.pending_restore = None;
        let Some(resource) = self.world_mut().resource_mut::<Checkpoints>() else {
            return;
        };
        let discards = std::mem::take(&mut resource.discards);
        let captures = std::mem::take(&mut resource.captures);
        let restore = resource.restore.take();
        if discards.is_empty() && captures.is_empty() && restore.is_none() {
            return;
        }
        for name in discards {
            if let Some(state) = self.checkpoints.as_mut() {
                state.slots.remove(&name);
            }
        }
        for name in captures {
            self.capture_slot(&name, frame);
        }
        if let Some(name) = restore {
            match validate_name(&name) {
                Ok(()) => {
                    if let Some(state) = self.checkpoints.as_mut() {
                        state.pending_restore = Some(name);
                    }
                }
                Err(reason) => self.invalid_name(&name, &reason, frame),
            }
        }
        self.refresh_checkpoints_mirror();
    }

    fn invalid_name(&mut self, name: &str, reason: &str, frame: FrameIndex) {
        let diagnostic = Diagnostic::new(
            code::CHECKPOINT_NAME_INVALID,
            format!("checkpoint name {name:?} is invalid: {reason}"),
        )
        .with_help("use a short name such as `b_to_c`: 1 to 64 of letters, digits, `_`, `.`, `-`");
        self.report_checkpoint(format!("{name}@{}", frame.as_u64()), diagnostic);
    }

    fn capture_slot(&mut self, name: &str, frame: FrameIndex) {
        if let Err(reason) = validate_name(name) {
            self.invalid_name(name, &reason, frame);
            return;
        }
        let Some(state) = self.checkpoints.as_ref() else {
            return;
        };
        if !state.slots.contains_key(name) && state.slots.len() >= state.config.max_slots as usize {
            let max = state.config.max_slots;
            let diagnostic = Diagnostic::new(
                code::CHECKPOINT_SLOTS_FULL,
                format!("checkpoint `{name}` was not captured: all {max} slots are held"),
            )
            .with_help(
                "`discard` a slot first, or raise `CheckpointConfig::max_slots` (at most 64)",
            );
            self.report_checkpoint(format!("{name}@{}", frame.as_u64()), diagnostic);
            return;
        }
        match self.build_slot(name) {
            Ok(slot) => {
                if let Some(state) = self.checkpoints.as_mut() {
                    state.slots.insert(name.to_string(), slot);
                }
            }
            Err(error) => {
                let diagnostic = Diagnostic::new(
                    code::CHECKPOINT_CAPTURE_FAILED,
                    format!(
                        "checkpoint `{name}` was not captured ({}): {}",
                        error.code, error.message
                    ),
                )
                .with_help("the previous slot of that name, if any, is kept");
                self.report_checkpoint(format!("{name}@{}", frame.as_u64()), diagnostic);
            }
        }
    }

    fn build_slot(&self, name: &str) -> Result<Slot, Diagnostic> {
        let doc = crate::snapshot::capture(self)?;
        let frame = self.frame();
        let clock = self
            .world()
            .resource::<GameClock>()
            .copied()
            .unwrap_or_default();
        Ok(Slot {
            info: CheckpointInfo {
                name: name.to_string(),
                frame,
                game_frame: clock.game_frame(frame),
                scene: self.scene_name().map(str::to_string),
            },
            doc: Arc::new(doc),
            clock,
            scene_path: self.current_scene_path().map(str::to_string),
            authored_names: self.authored_names().into_owned(),
        })
    }

    /// Performs the restore stashed by [`drain_checkpoints`](Engine::drain_checkpoints), or refuses
    /// it. Returns whether the world was replaced, so the boundary skips the scene switch: a restore
    /// wins over one requested the same frame.
    pub(crate) fn drain_checkpoint_restore(&mut self, frame: FrameIndex) -> bool {
        let Some(name) = self
            .checkpoints
            .as_mut()
            .and_then(|state| state.pending_restore.take())
        else {
            return false;
        };
        let key = format!("{name}@{}", frame.as_u64());
        let slot = self
            .checkpoints
            .as_ref()
            .and_then(|state| state.slots.get(&name))
            .cloned();
        let Some(slot) = slot else {
            let diagnostic = Diagnostic::new(
                code::CHECKPOINT_NO_SLOT,
                format!("checkpoint `{name}` cannot be restored: no such slot"),
            )
            .with_help("capture it first; a scene load, `engine.restore` or run-save resume clears every slot");
            let reason = format!("no slot named `{name}`");
            self.refuse_restore(name, reason);
            self.report_checkpoint(key, diagnostic);
            return false;
        };
        let switch_pending = self
            .world()
            .resource::<crate::transition::SceneSwitch>()
            .is_some_and(crate::transition::SceneSwitch::is_pending);
        // Before the rebuild: it mirrors this outcome into the fresh world.
        if let Some(state) = self.checkpoints.as_mut() {
            state.last_restore = Some(RestoreOutcome::Restored {
                name: name.clone(),
                frame: slot.info.frame,
            });
        }
        if let Err(error) = crate::snapshot::restore_checkpoint(self, &slot) {
            let diagnostic = Diagnostic::new(
                code::CHECKPOINT_RESTORE_FAILED,
                format!(
                    "checkpoint `{name}` could not be restored ({}): {}",
                    error.code, error.message
                ),
            )
            .with_help("the world is untouched");
            self.refuse_restore(name, error.message);
            self.report_checkpoint(key, diagnostic);
            return false;
        }
        self.set_current_scene_path(slot.scene_path.clone());
        self.push_event(Event::SceneChanged(SceneChangedData {
            kind: SceneChangeKind::Restore,
            path: slot.scene_path.clone(),
            request_id: self.current_request(),
        }));
        if switch_pending {
            let diagnostic = Diagnostic::new(
                code::CHECKPOINT_SWITCH_DROPPED,
                format!("a scene switch requested with the restore of `{name}` was dropped"),
            )
            .with_help("request the switch on a later frame, or restore without switching");
            self.report_checkpoint(key, diagnostic);
        }
        true
    }

    /// Records a refused restore and re-syncs the resource.
    fn refuse_restore(&mut self, name: String, reason: String) {
        if let Some(state) = self.checkpoints.as_mut() {
            state.last_restore = Some(RestoreOutcome::Refused { name, reason });
        }
        self.refresh_checkpoints_mirror();
    }

    /// Installs a world rebuilt by [`crate::snapshot::restore_checkpoint`]: the
    /// [`install_restored`](Engine::install_restored) twin that keeps the live session.
    ///
    /// # Errors
    /// Every diagnostic the content-root doc walk collected; the engine is untouched on failure.
    pub(crate) fn install_checkpoint(
        &mut self,
        mut restored: RestoredWorld,
        slot: &Slot,
    ) -> Result<(), Vec<Diagnostic>> {
        let doc_reloads = self.session_doc_reloads().clone();
        let docs = self.prepare_docs_for(restored.driver.world_mut(), &doc_reloads)?;
        // The frame does not move back: the clock gives up the frames since the capture.
        let elapsed = self
            .frame()
            .as_u64()
            .saturating_sub(slot.info.frame.as_u64());
        let game_clock = slot.clock.rebased(elapsed);
        let profile_text = self.emit_profile_text();
        self.commit_world(PreparedWorld {
            driver: restored.driver,
            seed: restored.seed,
            scene: restored.scene,
            scene_name: restored.scene_name,
            scene_physics: restored.scene_physics,
            docs,
            prefab_library: None,
            prefab_placements: Vec::new(),
            kind: Rebuild::Checkpoint {
                residue: restored.residue,
                authored_names: slot.authored_names.clone(),
                game_clock,
                arrived_by_restart: restored.arrived_by_restart,
                profile_text,
            },
        });
        Ok(())
    }

    /// Whether any slot is held.
    fn holds_checkpoints(&self) -> bool {
        self.checkpoints
            .as_ref()
            .is_some_and(|state| !state.slots.is_empty())
    }

    /// The `L4206` a keyframe is skipped with while slots are held. A snapshot cannot carry slots
    /// yet, so a keyframe taken now could not reproduce a later restore. The message is constant so
    /// the diagnostics latch reports it once per session.
    pub(crate) fn checkpoint_keyframe_skip(&self) -> Option<Diagnostic> {
        self.holds_checkpoints().then(|| {
            Diagnostic::new(
                code::CHECKPOINT_KEYFRAME_SKIP,
                "replay keyframes are skipped while checkpoint slots are held; a seek lands on the \
                 last keyframe before the first capture",
            )
            .with_help("the replay itself is unaffected; `discard` every slot to resume keyframes")
        })
    }

    /// The `L4207` that refuses arming a recording while slots are held.
    pub(crate) fn checkpoint_arm_refusal(&self) -> Option<Diagnostic> {
        self.holds_checkpoints().then(|| {
            Diagnostic::new(
                code::CHECKPOINT_RECORDING_ARM_REFUSED,
                "a recording was not armed: checkpoint slots are held and a snapshot cannot carry \
                 them yet",
            )
            .with_help(
                "start the recording before the first capture, or `discard` every slot first",
            )
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_validate() {
        for ok in ["b_to_c", "A.1-x", &"a".repeat(64)] {
            assert!(validate_name(ok).is_ok(), "{ok}");
        }
        for bad in ["", "a b", "a/b", "é", &"a".repeat(65)] {
            assert!(validate_name(bad).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn config_bounds() {
        assert!(CheckpointConfig::default().validate().is_ok());
        for max_slots in [0, 65] {
            let error = CheckpointConfig { max_slots }.validate().unwrap_err();
            assert_eq!(error.code, code::CHECKPOINT_CONFIG_INVALID);
        }
        assert!(CheckpointConfig { max_slots: 64 }.validate().is_ok());
    }
}
