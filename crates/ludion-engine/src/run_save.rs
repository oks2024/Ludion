//! The suspended run: `save/suspend.json`, the whole running game written so a later boot can offer
//! to continue it.
//!
//! A game system calls [`RunSave::suspend`]; the engine drains the request at the frame boundary
//! (beside the [profile save](crate::Profile::save)) and, in a live host only, writes the
//! [`engine.snapshot`](crate::Engine) document inside an envelope carrying a [`BuildStamp`]. The
//! stamp lives in the envelope, never in the snapshot document, so the replay content hash does not
//! move. [`RunSave::pending`] reports the file read at boot.

use std::path::Path;

use ludion_core::time::FrameIndex;
use ludion_ecs::Registry;
use ludion_protocol::hash::WireHash;
use ludion_protocol::schema;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::docs::DocKind;

/// The suspended run's path under the save root.
pub(crate) const RUN_SAVE_PATH: &str = "save/suspend.json";

/// The envelope's `format`; a file with a larger one is left alone and reads as no suspended run.
const RUN_SAVE_FORMAT: u32 = 1;

/// What built the run that was saved: the engine version and a hash of the game's component and
/// data-doc schemas (prose stripped). A saved run is only offered back to the build that wrote it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct BuildStamp {
    /// The engine version that wrote the file.
    pub engine_version: String,
    /// The hash of the game's registered component and data-doc schemas.
    pub game_schema: WireHash,
}

/// The hash of every registered component schema and data-doc kind, in registration order, with
/// doc-comment prose stripped so editing a `///` line does not invalidate saved runs.
pub(crate) fn game_schema_hash(registry: &Registry, doc_kinds: &[DocKind]) -> WireHash {
    let components: Vec<Value> = registry
        .iter()
        .map(|info| json!({ "name": info.name(), "schema": info.schema() }))
        .collect();
    let docs: Vec<Value> = doc_kinds
        .iter()
        .map(|dk| json!({ "kind": dk.kind, "singleton": dk.singleton, "schema": dk.schema }))
        .collect();
    schema::canonical_hash(&schema::strip_prose(
        json!({ "components": components, "docs": docs }),
    ))
}

/// `None` when `saved` was written by this build; otherwise a player-readable reason, the engine
/// version first.
#[allow(
    dead_code,
    reason = "the resume path compares stamps; unit tests cover it"
)]
pub(crate) fn stamp_mismatch(saved: &BuildStamp, current: &BuildStamp) -> Option<String> {
    if saved.engine_version != current.engine_version {
        return Some(format!(
            "saved by engine {}; this is {}",
            saved.engine_version, current.engine_version
        ));
    }
    if saved.game_schema != current.game_schema {
        return Some(
            "the game's component or data schemas changed since this run was saved".to_string(),
        );
    }
    None
}

/// A suspended run found on disk (or just written): enough for a menu to offer it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SuspendInfo {
    /// The frame the run was suspended at.
    pub frame: FrameIndex,
    /// The suspended scene's `name:`, when it has one.
    pub scene: Option<String>,
    /// What built the run that was saved.
    pub saved_at_build: BuildStamp,
}

/// Why a resume did not happen.
#[non_exhaustive]
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ResumeOutcome {
    /// The saved run was refused; `reason` is player-readable.
    Refused {
        /// Why the run was not resumed.
        reason: String,
    },
}

/// One run-save request a game system made.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RunSaveRequest {
    /// [`RunSave::suspend`].
    Suspend,
    /// [`RunSave::discard`].
    Discard,
}

/// One drained run-save request, stamped with the frame during which a system made it — the
/// [`ProfileSaveEntry`](crate::ProfileSaveEntry) mold, in the engine's
/// [run-save log](crate::Engine::run_save_log).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RunSaveEntry {
    /// The frame whose systems made the request.
    pub frame: FrameIndex,
    /// What was requested.
    pub request: RunSaveRequest,
}

/// The engine's persistent run-save state: it outlives every world rebuild, and the [`RunSave`]
/// resource is a mirror of it reinstalled after each one.
pub(crate) struct RunSaveState {
    pub(crate) stamp: BuildStamp,
    pub(crate) pending: Option<SuspendInfo>,
    pub(crate) last_resume: Option<ResumeOutcome>,
}

/// The suspend/discard handle a game system holds, installed by
/// [`EngineBuilder::run_save`](crate::EngineBuilder::run_save).
///
/// Requests are flags the engine drains at the frame boundary, so several calls in one frame
/// collapse (a discard runs before a suspend). Like [`Profile`](crate::Profile) it is a resource
/// with no digest, invisible to the `WorldHash`. Only a live host writes the file; a test or replay
/// fills the [log](crate::Engine::run_save_log) and updates [`pending`](RunSave::pending) without
/// touching disk.
///
/// # Example
/// ```
/// use ludion_engine::{Engine, RunSave};
///
/// # let dir = std::env::temp_dir().join(format!("ludion-runsave-doc-{}", std::process::id()));
/// # let _ = std::fs::remove_dir_all(&dir);
/// let engine = Engine::builder()
///     .run_save()
///     .project_root(&dir)
///     .build()
///     .unwrap();
///
/// // No file on disk → no suspended run to offer.
/// assert!(engine.world().resource::<RunSave>().unwrap().pending().is_none());
/// # let _ = std::fs::remove_dir_all(&dir);
/// ```
pub struct RunSave {
    pending: Option<SuspendInfo>,
    last_resume: Option<ResumeOutcome>,
    pub(crate) suspend: bool,
    pub(crate) discard: bool,
}

impl RunSave {
    /// Requests that the running game be written to `save/suspend.json` at the frame boundary.
    pub fn suspend(&mut self) {
        self.suspend = true;
    }

    /// Requests that the suspended run be deleted at the frame boundary.
    pub fn discard(&mut self) {
        self.discard = true;
    }

    /// The suspended run read at boot, or the one the last [`suspend`](RunSave::suspend) wrote.
    pub fn pending(&self) -> Option<&SuspendInfo> {
        self.pending.as_ref()
    }

    /// Why the last resume did not happen, if it did not.
    pub fn last_resume(&self) -> Option<&ResumeOutcome> {
        self.last_resume.as_ref()
    }

    pub(crate) fn mirror(state: &RunSaveState) -> Self {
        RunSave {
            pending: state.pending.clone(),
            last_resume: state.last_resume.clone(),
            suspend: false,
            discard: false,
        }
    }

    /// Updates the mirrored pending run in place.
    pub(crate) fn set_pending(&mut self, pending: Option<SuspendInfo>) {
        self.pending = pending;
    }
}

/// The on-disk envelope. `snapshot` is the untouched `engine.snapshot` document.
#[derive(Serialize, Deserialize)]
struct RunSaveFile {
    format: u32,
    build: BuildStamp,
    frame: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    scene: Option<String>,
    snapshot: Value,
}

/// The envelope text for a captured `snapshot` (trailing newline).
pub(crate) fn encode(info: &SuspendInfo, snapshot: Value) -> String {
    let file = RunSaveFile {
        format: RUN_SAVE_FORMAT,
        build: info.saved_at_build.clone(),
        frame: info.frame.as_u64(),
        scene: info.scene.clone(),
        snapshot,
    };
    let mut text = serde_json::to_string(&file).expect("a JSON value always serializes");
    text.push('\n');
    text
}

/// Writes the envelope crash-durably; the error is the `L1400` message.
pub(crate) fn write(path: &Path, info: &SuspendInfo, snapshot: Value) -> Result<(), String> {
    ludion_core::atomic::write_durable(path, encode(info, snapshot)).map_err(|e| {
        format!(
            "the suspended run could not be written to `{}`: {e}",
            path.display()
        )
    })
}

/// Deletes the suspended run; a missing file is success, any other error is the `L1400` message.
pub(crate) fn remove_file(path: &Path) -> Result<(), String> {
    match std::fs::remove_file(path) {
        Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(format!(
            "the suspended run `{}` could not be removed: {e}",
            path.display()
        )),
        _ => Ok(()),
    }
}

/// Reads the suspended run at `path`. A missing, unreadable or unparsable file, or one written by a
/// newer format, reads as `None` and is left in place.
pub(crate) fn read_pending(path: &Path) -> Option<SuspendInfo> {
    let text = std::fs::read_to_string(path).ok()?;
    let file: RunSaveFile = serde_json::from_str(&text).ok()?;
    if file.format > RUN_SAVE_FORMAT {
        return None;
    }
    Some(SuspendInfo {
        frame: FrameIndex::new(file.frame),
        scene: file.scene,
        saved_at_build: file.build,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use schemars::JsonSchema;

    fn stamp(version: &str, schema: u64) -> BuildStamp {
        BuildStamp {
            engine_version: version.to_string(),
            game_schema: WireHash::of_bytes(&schema.to_le_bytes()),
        }
    }

    #[test]
    fn stamp_mismatch_names_the_engine_first_then_the_schema() {
        let a = stamp("0.1.0", 1);
        assert_eq!(stamp_mismatch(&a, &a.clone()), None);
        let engine = stamp_mismatch(&a, &stamp("0.2.0", 2)).unwrap();
        assert_eq!(engine, "saved by engine 0.1.0; this is 0.2.0");
        let schema = stamp_mismatch(&a, &stamp("0.1.0", 2)).unwrap();
        assert!(schema.contains("schemas changed"), "{schema}");
    }

    /// The baseline component.
    #[derive(Serialize, Deserialize, JsonSchema)]
    struct Plain {
        hp: u32,
    }

    /// Different prose on the type, and prose on the field.
    #[derive(Serialize, Deserialize, JsonSchema)]
    struct Prose {
        /// The field's own prose.
        hp: u32,
    }

    /// A field added.
    #[derive(Serialize, Deserialize, JsonSchema)]
    struct Wider {
        hp: u32,
        mana: u32,
    }

    fn hash_with<T: ludion_ecs::Component>() -> WireHash {
        let mut registry = Registry::new();
        registry.register::<T>("Gear").unwrap();
        game_schema_hash(&registry, &[])
    }

    #[test]
    fn the_schema_hash_moves_on_a_field_but_not_on_prose() {
        assert_eq!(hash_with::<Plain>(), hash_with::<Prose>());
        assert_ne!(hash_with::<Plain>(), hash_with::<Wider>());
    }

    #[test]
    fn game_schema_hash_reads_the_registry() {
        let empty = Registry::new();
        assert_eq!(game_schema_hash(&empty, &[]), game_schema_hash(&empty, &[]));
        assert_ne!(game_schema_hash(&empty, &[]), hash_with::<Plain>());
    }

    #[test]
    fn a_garbled_or_newer_file_reads_as_none() {
        let dir = std::env::temp_dir().join(format!("ludion-runsave-unit-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("suspend.json");
        assert!(read_pending(&path).is_none());
        std::fs::write(&path, "not json").unwrap();
        assert!(read_pending(&path).is_none());
        let info = SuspendInfo {
            frame: FrameIndex::new(7),
            scene: Some("hub".into()),
            saved_at_build: stamp("0.1.0", 1),
        };
        std::fs::write(&path, encode(&info, json!({}))).unwrap();
        assert_eq!(read_pending(&path), Some(info.clone()));
        let newer = encode(&info, json!({})).replacen("\"format\":1", "\"format\":2", 1);
        std::fs::write(&path, newer).unwrap();
        assert!(read_pending(&path).is_none());
        assert!(path.exists());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
