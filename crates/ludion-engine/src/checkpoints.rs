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
use std::path::Path;
use std::sync::Arc;

use ludion_core::codes::code;
use ludion_core::collections::StableSet;
use ludion_core::diagnostics::Diagnostic;
use ludion_core::time::{FrameIndex, GameClock};
use ludion_protocol::event::{Event, LogEvent, SceneChangeKind, SceneChangedData};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::engine::{Engine, PreparedWorld, Rebuild, RestoredWorld};
use crate::run_save::BuildStamp;

/// The disk slot's path under the save root.
pub(crate) const CHECKPOINT_PATH: &str = "save/checkpoint.json";

/// The envelope's `format`; a file with a larger one is refused on a load and reads as no disk slot.
const CHECKPOINT_FORMAT: u32 = 1;

/// The name a refusal reports when the disk slot's own name is unknown. Not a valid slot name.
const DISK_REFUSAL_NAME: &str = "<disk>";

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
#[derive(Clone, Debug)]
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
    pub(crate) pending_restore: Option<RestoreRequest>,
    /// The disk slot read at a persisting boot or last written.
    pub(crate) disk: Option<CheckpointInfo>,
    /// The last disk write or load failure.
    pub(crate) disk_error: Option<Diagnostic>,
    pub(crate) stamp: BuildStamp,
}

/// What a restore request asks for; the last one in a frame wins.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum RestoreRequest {
    /// [`Checkpoints::restore`].
    Slot(String),
    /// [`Checkpoints::load_disk`].
    Disk,
}

/// The capture/restore/discard handle a game system holds, installed by
/// [`EngineBuilder::checkpoints`](crate::EngineBuilder::checkpoints).
///
/// Requests are drained at the frame boundary, never mid-tick, so no system observes a half-restored
/// world. Within one boundary: every `discard` runs first, then every `capture` in call order, then
/// the last `restore` called. A capture and a restore of one name in one frame restore what was
/// just captured. A restore wins over a scene switch requested the same frame (`L4205`). The disk
/// slot rides the same boundary: `discard_disk` runs before `persist`, and `persist` after the
/// captures, so it writes this frame's capture. `load_disk` is a restore request and shares the
/// last-wins rule. Like [`RunSave`](crate::RunSave) it is a resource with no digest, invisible to
/// the `WorldHash`.
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
    pub(crate) restore: Option<RestoreRequest>,
    disk: Option<CheckpointInfo>,
    disk_error: Option<Diagnostic>,
    pub(crate) persist: Option<String>,
    pub(crate) discard_disk: bool,
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
        self.restore = Some(RestoreRequest::Slot(name.into()));
    }

    /// Requests that held slot `name` be written to `save/checkpoint.json` at the frame boundary,
    /// after this frame's captures; the last call in a frame wins. An unknown name is refused
    /// (`L4200`) and nothing is written. A persisting host writes the file crash-durably (a failure
    /// is `L4208`, the previous file stays); any other host only updates [`disk`](Checkpoints::disk).
    /// Persisting is output, so it is safe under a recording.
    pub fn persist(&mut self, name: impl Into<String>) {
        self.persist = Some(name.into());
    }

    /// Requests that the disk slot be deleted at the frame boundary, before any
    /// [`persist`](Checkpoints::persist). No file is success.
    pub fn discard_disk(&mut self) {
        self.discard_disk = true;
    }

    /// Requests a Continue: the world becomes the disk slot's at the frame boundary, as
    /// [`restore`](Checkpoints::restore) does, and the file is kept. The slot also lands in the
    /// store, so a later `restore(name)` works. The frame never moves back: a slot ahead of the live
    /// frame advances it. [`last_restore`](Checkpoints::last_restore) reports it. A refusal leaves
    /// the world untouched: a host that keeps no saves, a missing file, a store full of other
    /// names (`L4201`), and while a recording is open (`L4209`, the file kept). A garbled or newer
    /// file, one from another build, or one that no longer restores cleanly is `L4209` too, and the
    /// file is renamed to `checkpoint.json.bak.<hash>`. The last `restore`/`load_disk` in a frame
    /// wins.
    pub fn load_disk(&mut self) {
        self.restore = Some(RestoreRequest::Disk);
    }

    /// The disk slot read at a persisting boot or last written, or `None`: no file, a refused one,
    /// and always in a replay or playback.
    ///
    /// This is disk input a replay never sees, the [`RunSave::pending`](crate::RunSave::pending)
    /// rule: read it on the title screen, before a recording starts.
    pub fn disk(&self) -> Option<&CheckpointInfo> {
        self.disk.as_ref()
    }

    /// The last disk write or load failure (`L4208`, `L4209`), until a request succeeds.
    pub fn disk_error(&self) -> Option<&Diagnostic> {
        self.disk_error.as_ref()
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

    /// `replayed`: the world a replay runs in sees no disk state.
    pub(crate) fn mirror(state: &CheckpointState, replayed: bool) -> Self {
        Checkpoints {
            slots: state.slots.values().map(|slot| slot.info.clone()).collect(),
            last_restore: state.last_restore.clone(),
            captures: Vec::new(),
            discards: Vec::new(),
            restore: None,
            disk: state.disk.clone().filter(|_| !replayed),
            disk_error: state.disk_error.clone().filter(|_| !replayed),
            persist: None,
            discard_disk: false,
        }
    }
}

/// The on-disk envelope; `slot` is the untouched capture document.
#[derive(Serialize, Deserialize)]
struct CheckpointFile<S> {
    format: u32,
    build: BuildStamp,
    name: String,
    frame: u64,
    game_frame: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    scene: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    scene_path: Option<String>,
    authored_names: Vec<String>,
    slot: S,
}

/// The envelope text for `slot` (trailing newline).
pub(crate) fn encode(slot: &Slot, build: &BuildStamp) -> String {
    let file = CheckpointFile {
        format: CHECKPOINT_FORMAT,
        build: build.clone(),
        name: slot.info.name.clone(),
        frame: slot.info.frame.as_u64(),
        game_frame: slot.info.game_frame.as_u64(),
        scene: slot.info.scene.clone(),
        scene_path: slot.scene_path.clone(),
        authored_names: slot.authored_names.iter().cloned().collect(),
        slot: &*slot.doc,
    };
    let mut text = serde_json::to_string(&file).expect("a JSON value always serializes");
    text.push('\n');
    text
}

/// Writes the envelope crash-durably; the error is the `L4208` message.
pub(crate) fn write(path: &Path, slot: &Slot, build: &BuildStamp) -> Result<(), String> {
    ludion_core::atomic::write_durable(path, encode(slot, build)).map_err(|e| {
        format!(
            "checkpoint `{}` could not be written to `{}`: {e}",
            slot.info.name,
            path.display()
        )
    })
}

/// Deletes the disk slot; a missing file is success, any other error is the `L4208` message.
pub(crate) fn remove_file(path: &Path) -> Result<(), String> {
    match std::fs::remove_file(path) {
        Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(format!(
            "the disk checkpoint `{}` could not be removed: {e}",
            path.display()
        )),
        _ => Ok(()),
    }
}

/// Reads the disk slot's metadata at a boot. A missing, unreadable or unparsable file, or one
/// written by a newer format, reads as `None` and is left in place.
pub(crate) fn read_info(path: &Path) -> Option<CheckpointInfo> {
    let text = std::fs::read_to_string(path).ok()?;
    let file: CheckpointFile<Value> = serde_json::from_str(&text).ok()?;
    if file.format > CHECKPOINT_FORMAT {
        return None;
    }
    Some(CheckpointInfo {
        name: file.name,
        frame: FrameIndex::new(file.frame),
        game_frame: FrameIndex::new(file.game_frame),
        scene: file.scene,
    })
}

/// Reads the disk slot for a load. `Ok(None)` when there is no file; the `Err` is the
/// player-readable refusal reason (unreadable, garbled, newer format, another build, bad name).
pub(crate) fn read_slot(path: &Path, current: &BuildStamp) -> Result<Option<Slot>, String> {
    let text = match std::fs::read_to_string(path) {
        Ok(text) => text,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(format!("the disk checkpoint is unreadable: {e}")),
    };
    let file: CheckpointFile<Value> = serde_json::from_str(&text)
        .map_err(|e| format!("the disk checkpoint is unreadable: {e}"))?;
    if file.format > CHECKPOINT_FORMAT {
        return Err("the disk checkpoint was written by a newer version".to_string());
    }
    if let Some(reason) = crate::run_save::stamp_mismatch(&file.build, current) {
        return Err(reason);
    }
    validate_name(&file.name)
        .map_err(|reason| format!("the disk checkpoint has an invalid name: {reason}"))?;
    let clock = match file.slot.get("game_clock") {
        Some(value) => serde_json::from_value(value.clone())
            .map_err(|e| format!("the disk checkpoint is unreadable: its clock: {e}"))?,
        None => GameClock::default(),
    };
    let frame = FrameIndex::new(file.frame);
    Ok(Some(Slot {
        info: CheckpointInfo {
            name: file.name,
            frame,
            game_frame: clock.game_frame(frame),
            scene: file.scene,
        },
        doc: Arc::new(file.slot),
        clock,
        scene_path: file.scene_path,
        authored_names: file.authored_names.into_iter().collect(),
    }))
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
        let mut disk = None;
        if self.persisting() {
            let path = self.save_root().join(CHECKPOINT_PATH);
            crate::player_profile::sweep_temps(&path);
            disk = read_info(&path);
        }
        self.checkpoints = Some(CheckpointState {
            config,
            slots: BTreeMap::new(),
            last_restore: None,
            pending_restore: None,
            disk,
            disk_error: None,
            stamp: self.build_stamp(),
        });
        self.install_checkpoints(false);
    }

    /// (Re)installs the [`Checkpoints`] resource from the engine's state; a no-op without
    /// [`checkpoints`](crate::EngineBuilder::checkpoints). Called after every world (re)build.
    /// `clear` drops the slots first: a scene load and an `engine.restore` start a fresh session.
    pub(crate) fn install_checkpoints(&mut self, clear: bool) {
        let replayed = self.is_playback();
        let Some(state) = self.checkpoints.as_mut() else {
            return;
        };
        if clear {
            state.slots.clear();
            state.last_restore = None;
        }
        let mirror = Checkpoints::mirror(state, replayed);
        self.world_mut().insert_resource(mirror);
    }

    /// Re-syncs the mirrored slot list, outcome and disk slot into the live resource, keeping its
    /// requests.
    fn refresh_checkpoints_mirror(&mut self) {
        let replayed = self.is_playback();
        let Some(state) = self.checkpoints.as_ref() else {
            return;
        };
        let fresh = Checkpoints::mirror(state, replayed);
        if let Some(resource) = self.world_mut().resource_mut::<Checkpoints>() {
            resource.slots = fresh.slots;
            resource.last_restore = fresh.last_restore;
            resource.disk = fresh.disk;
            resource.disk_error = fresh.disk_error;
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
        let persist = resource.persist.take();
        let discard_disk = std::mem::take(&mut resource.discard_disk);
        if discards.is_empty()
            && captures.is_empty()
            && restore.is_none()
            && persist.is_none()
            && !discard_disk
        {
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
        if discard_disk {
            self.discard_disk_slot();
        }
        if let Some(name) = persist {
            self.persist_slot(&name, frame);
        }
        match restore {
            Some(RestoreRequest::Slot(name)) => match validate_name(&name) {
                Ok(()) => {
                    if let Some(state) = self.checkpoints.as_mut() {
                        state.pending_restore = Some(RestoreRequest::Slot(name));
                    }
                }
                Err(reason) => self.invalid_name(&name, &reason, frame),
            },
            Some(RestoreRequest::Disk) => {
                if let Some(state) = self.checkpoints.as_mut() {
                    state.pending_restore = Some(RestoreRequest::Disk);
                }
            }
            None => {}
        }
        self.refresh_checkpoints_mirror();
    }

    /// The disk slot's file under the save root.
    fn checkpoint_path(&self) -> std::path::PathBuf {
        self.save_root().join(CHECKPOINT_PATH)
    }

    /// Reports an `L4208`/`L4209` on both sinks (stderr and the `log` channel) and keeps it as the
    /// last disk error.
    fn report_disk(&mut self, diagnostic: Diagnostic) {
        eprintln!("{diagnostic}");
        self.push_event(Event::Log(LogEvent::from_diagnostic(diagnostic.clone())));
        if let Some(state) = self.checkpoints.as_mut() {
            state.disk_error = Some(diagnostic);
        }
    }

    fn disk_write_failed(&mut self, message: String) {
        let diagnostic = Diagnostic::new(code::CHECKPOINT_DISK_WRITE_FAILED, message).with_help(
            "the previous whole `checkpoint.json` stays on disk and the next request retries; \
             check the save location is writable",
        );
        self.report_disk(diagnostic);
    }

    /// Deletes the disk slot (a persisting host only) and clears `disk`.
    fn discard_disk_slot(&mut self) {
        if self.persisting()
            && let Err(message) = remove_file(&self.checkpoint_path())
        {
            self.disk_write_failed(message);
            return;
        }
        if let Some(state) = self.checkpoints.as_mut() {
            state.disk = None;
            state.disk_error = None;
        }
    }

    /// Writes held slot `name` to the disk slot (a persisting host only) and updates `disk`.
    fn persist_slot(&mut self, name: &str, frame: FrameIndex) {
        let Some(state) = self.checkpoints.as_ref() else {
            return;
        };
        let Some(slot) = state.slots.get(name).cloned() else {
            let diagnostic = Diagnostic::new(
                code::CHECKPOINT_NO_SLOT,
                format!("checkpoint `{name}` cannot be persisted: no such slot"),
            )
            .with_help("capture it first; nothing was written");
            self.report_checkpoint(format!("persist {name}@{}", frame.as_u64()), diagnostic);
            return;
        };
        let stamp = state.stamp.clone();
        if self.persisting()
            && let Err(message) = write(&self.checkpoint_path(), &slot, &stamp)
        {
            self.disk_write_failed(message);
            return;
        }
        if let Some(state) = self.checkpoints.as_mut() {
            state.disk = Some(slot.info);
            state.disk_error = None;
        }
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
        let Some(request) = self
            .checkpoints
            .as_mut()
            .and_then(|state| state.pending_restore.take())
        else {
            return false;
        };
        match request {
            RestoreRequest::Slot(name) => self.restore_named(name, frame),
            RestoreRequest::Disk => self.load_disk_slot(frame),
        }
    }

    /// Restores held slot `name`, or refuses it.
    fn restore_named(&mut self, name: String, frame: FrameIndex) -> bool {
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

    /// Loads the disk slot: reads it strictly, files it in the store and restores it. Every refusal
    /// leaves the world and the store as they were.
    fn load_disk_slot(&mut self, frame: FrameIndex) -> bool {
        if !self.persisting() {
            self.refuse_restore(
                self.disk_refusal_name(),
                "this host keeps no saves".to_string(),
            );
            return false;
        }
        if self.is_recording() {
            self.refuse_disk(
                "a recording is open: stop it before load_disk()".to_string(),
                false,
            );
            return false;
        }
        let Some(stamp) = self.checkpoints.as_ref().map(|state| state.stamp.clone()) else {
            return false;
        };
        let slot = match read_slot(&self.checkpoint_path(), &stamp) {
            Ok(Some(slot)) => slot,
            Ok(None) => {
                if let Some(state) = self.checkpoints.as_mut() {
                    state.disk = None;
                }
                self.refuse_restore(
                    DISK_REFUSAL_NAME.to_string(),
                    "there is no disk checkpoint".to_string(),
                );
                return false;
            }
            Err(reason) => {
                self.refuse_disk(reason, true);
                return false;
            }
        };
        let name = slot.info.name.clone();
        let Some(state) = self.checkpoints.as_mut() else {
            return false;
        };
        if !state.slots.contains_key(&name) && state.slots.len() >= state.config.max_slots as usize
        {
            let max = state.config.max_slots;
            let diagnostic = Diagnostic::new(
                code::CHECKPOINT_SLOTS_FULL,
                format!("the disk checkpoint `{name}` was not loaded: all {max} slots are held"),
            )
            .with_help(
                "`discard` a slot first, or raise `CheckpointConfig::max_slots`; the file is kept",
            );
            self.refuse_restore(name.clone(), format!("all {max} slots are held"));
            self.report_checkpoint(format!("{name}@{}", frame.as_u64()), diagnostic);
            return false;
        }
        let info = slot.info.clone();
        let replaced = state.slots.insert(name.clone(), slot);
        if self.restore_named(name.clone(), frame) {
            if let Some(state) = self.checkpoints.as_mut() {
                state.disk = Some(info);
                state.disk_error = None;
            }
            self.refresh_checkpoints_mirror();
            return true;
        }
        // The restore refused (already reported as `L4204`): the store goes back as it was.
        if let Some(state) = self.checkpoints.as_mut() {
            match replaced {
                Some(previous) => state.slots.insert(name.clone(), previous),
                None => state.slots.remove(&name),
            };
        }
        let reason = match self
            .checkpoints
            .as_ref()
            .and_then(|s| s.last_restore.as_ref())
        {
            Some(RestoreOutcome::Refused { reason, .. }) => reason.clone(),
            _ => "the checkpoint could not be restored".to_string(),
        };
        self.refuse_disk(reason, true);
        false
    }

    /// The name a disk refusal reports: the disk slot's own when known.
    fn disk_refusal_name(&self) -> String {
        self.checkpoints
            .as_ref()
            .and_then(|state| state.disk.as_ref())
            .map_or_else(|| DISK_REFUSAL_NAME.to_string(), |info| info.name.clone())
    }

    /// Refuses a disk load with an `L4209` on both sinks. With `backup` the file is renamed to
    /// `checkpoint.json.bak.<hash>` and `disk()` becomes `None`; without it the file is kept.
    fn refuse_disk(&mut self, reason: String, backup: bool) {
        let mut reason = reason;
        let name = self.disk_refusal_name();
        if backup {
            if let Err(error) = crate::player_profile::backup_file(&self.checkpoint_path()) {
                reason = format!(
                    "{reason}; the file could not be backed up ({error}), so it was left in place"
                );
            }
            if let Some(state) = self.checkpoints.as_mut() {
                state.disk = None;
            }
        }
        let diagnostic = Diagnostic::new(
            code::CHECKPOINT_DISK_LOAD_REFUSED,
            format!("the disk checkpoint was not loaded: {reason}"),
        )
        .with_help(if backup {
            "start a new game; the original is kept as `checkpoint.json.bak.<hash>` beside it"
        } else {
            "the file is kept; stop the recording, then `load_disk()` again"
        });
        self.report_disk(diagnostic);
        self.refuse_restore(name, reason);
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
        // The frame does not move back: the clock gives up the frames since the capture. A slot
        // ahead of the live frame (a disk slot at boot) lifts the driver to it, so none elapse.
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

    fn stamp(version: &str) -> BuildStamp {
        BuildStamp {
            engine_version: version.to_string(),
            game_schema: ludion_protocol::hash::WireHash::of_bytes(b"schema"),
            projectiles: None,
        }
    }

    fn slot() -> Slot {
        let clock = GameClock::default().rebased(3);
        let frame = FrameIndex::new(40);
        Slot {
            info: CheckpointInfo {
                name: "b_to_c".to_string(),
                frame,
                game_frame: clock.game_frame(frame),
                scene: Some("hub".to_string()),
            },
            doc: Arc::new(serde_json::json!({ "frame": 40, "rng": 3, "game_clock": clock })),
            clock,
            scene_path: Some("scenes/hub.yaml".to_string()),
            authored_names: ["b".to_string(), "a".to_string()].into_iter().collect(),
        }
    }

    fn temp_file(label: &str) -> std::path::PathBuf {
        let dir =
            std::env::temp_dir().join(format!("ludion-checkpoint-{label}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir.join("checkpoint.json")
    }

    #[test]
    fn the_envelope_round_trips() {
        let path = temp_file("round");
        let held = slot();
        write(&path, &held, &stamp("0.1.0")).unwrap();
        assert_eq!(read_info(&path), Some(held.info.clone()));
        let read = read_slot(&path, &stamp("0.1.0")).unwrap().unwrap();
        assert_eq!(read.info, held.info);
        assert_eq!(read.doc, held.doc);
        assert_eq!(read.scene_path, held.scene_path);
        assert_eq!(read.authored_names, held.authored_names);
        // Written sorted, so the file is canonical.
        assert!(
            std::fs::read_to_string(&path)
                .unwrap()
                .contains("\"authored_names\":[\"a\",\"b\"]")
        );
        remove_file(&path).unwrap();
        assert!(read_slot(&path, &stamp("0.1.0")).unwrap().is_none());
        remove_file(&path).unwrap();
    }

    #[test]
    fn a_garbled_file_reads_as_none_and_is_left_in_place() {
        let path = temp_file("garbled");
        assert!(read_info(&path).is_none());
        std::fs::write(&path, "not json").unwrap();
        assert!(read_info(&path).is_none());
        assert!(path.exists());
        assert!(
            read_slot(&path, &stamp("0.1.0"))
                .unwrap_err()
                .contains("unreadable")
        );
    }

    #[test]
    fn a_strict_read_refuses_a_newer_format_and_a_foreign_build() {
        let path = temp_file("strict");
        let text = encode(&slot(), &stamp("0.1.0"));
        let newer = text.replacen("\"format\":1", "\"format\":2", 1);
        std::fs::write(&path, newer).unwrap();
        assert!(read_info(&path).is_none());
        let err = read_slot(&path, &stamp("0.1.0")).unwrap_err();
        assert!(err.contains("newer version"), "{err}");

        std::fs::write(&path, text).unwrap();
        let err = read_slot(&path, &stamp("0.2.0")).unwrap_err();
        assert!(err.contains("saved by engine"), "{err}");
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
