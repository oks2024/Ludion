//! The suspended run: `save/suspend.json`, the whole running game written so a later boot can offer
//! to continue it.
//!
//! A game system calls [`RunSave::suspend`]; the engine drains the request at the frame boundary
//! (beside the [profile save](crate::Profile::save)) and, in a live host only, writes the
//! [`engine.snapshot`](crate::Engine) document inside an envelope carrying a [`BuildStamp`]. The
//! stamp lives in the envelope, never in the snapshot document, so the replay content hash does not
//! move. [`RunSave::pending`] reports the file read at boot.

use std::path::{Path, PathBuf};

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
    /// The hash of the builder's `ProjectileConfig`, `None` without a pool. A file without the
    /// field reads as `None`, so a game that has a pool refuses it.
    #[serde(default)]
    pub projectiles: Option<WireHash>,
}

/// The hash of the builder's projectile pool config, for the [`BuildStamp`].
pub(crate) fn projectile_config_hash(config: &crate::ProjectileConfig) -> WireHash {
    WireHash::of_bytes(&serde_json::to_vec(config).expect("a config always serializes"))
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
    if saved.projectiles != current.projectiles {
        return Some(
            "the game's projectile pool settings changed since this run was saved".to_string(),
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

/// What the last [`RunSave::resume`] did.
#[non_exhaustive]
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ResumeOutcome {
    /// The saved run was restored in place.
    Resumed {
        /// The restored snapshot's frame, the frame the resumed run continues from.
        frame: FrameIndex,
    },
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
    /// [`RunSave::resume`].
    Resume,
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

/// The suspend/discard/resume handle a game system holds, installed by
/// [`EngineBuilder::run_save`](crate::EngineBuilder::run_save).
///
/// Requests are flags the engine drains at the frame boundary, so several calls in one frame
/// collapse (discard, then suspend, then resume). Like [`Profile`](crate::Profile) it is a resource
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
    pub(crate) resume: bool,
}

impl RunSave {
    /// Requests that the running game be written to `save/suspend.json` at the frame boundary.
    /// A successful persisting suspend also finalizes an open
    /// [`RunRecording`](crate::RunRecording) as a normal `stop()`: the file covers the run up to
    /// the suspend frame, and the resumed rest of the run is not recorded.
    pub fn suspend(&mut self) {
        self.suspend = true;
    }

    /// Requests that the suspended run be deleted at the frame boundary.
    pub fn discard(&mut self) {
        self.discard = true;
    }

    /// Requests that the suspended run be restored at the frame boundary, after any
    /// [`discard`](RunSave::discard) and [`suspend`](RunSave::suspend). A persisting host restores
    /// the world in place with held input cleared and deletes the file; a refusal is reported by
    /// [`last_resume`](RunSave::last_resume). Without persistence the request is only logged.
    /// While a [`RunRecording`](crate::RunRecording) is open the resume is refused (`L1401`) with
    /// the save kept: `stop()` the recording first.
    pub fn resume(&mut self) {
        self.resume = true;
    }

    /// The suspended run read at boot, or the one the last [`suspend`](RunSave::suspend) wrote.
    ///
    /// This is disk input a replay never sees: a replay, and an in-place playback, starts with
    /// `None` whatever file is pending live. Read it before
    /// [`RunRecording::start`](crate::RunRecording::start) (on the title screen); a system that
    /// branches on it during a recorded run diverges on replay.
    pub fn pending(&self) -> Option<&SuspendInfo> {
        self.pending.as_ref()
    }

    /// The outcome of the last resume, if there was one.
    pub fn last_resume(&self) -> Option<&ResumeOutcome> {
        self.last_resume.as_ref()
    }

    /// `replayed`: the world a replay runs in sees no disk state, as under `--replay-verify`.
    pub(crate) fn mirror(state: &RunSaveState, replayed: bool) -> Self {
        RunSave {
            pending: state.pending.clone().filter(|_| !replayed),
            last_resume: state.last_resume.clone().filter(|_| !replayed),
            suspend: false,
            discard: false,
            resume: false,
        }
    }

    /// Updates the mirrored pending run in place.
    pub(crate) fn set_pending(&mut self, pending: Option<SuspendInfo>) {
        self.pending = pending;
    }

    /// Updates the mirrored resume outcome in place.
    pub(crate) fn set_last_resume(&mut self, outcome: Option<ResumeOutcome>) {
        self.last_resume = outcome;
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

/// Where a resume parks `path` while it restores: a sibling `suspend.json.resuming`.
pub(crate) fn resuming_path(path: &Path) -> PathBuf {
    let mut name = path.file_name().unwrap_or_default().to_os_string();
    name.push(".resuming");
    path.with_file_name(name)
}

/// Consumes the suspended run: renames `path` to its [`resuming_path`] so a failed delete later
/// cannot give a second resume. `Ok(None)` when there is no file; the `Err` is a refusal reason.
pub(crate) fn consume(path: &Path) -> Result<Option<PathBuf>, String> {
    let parked = resuming_path(path);
    match std::fs::rename(path, &parked) {
        Ok(()) => Ok(Some(parked)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(format!(
            "the suspended run could not be set aside for the resume: {e}"
        )),
    }
}

/// Keeps a `suspend.json.resuming` a crashed resume left behind as a backup, so a run the player
/// may still want is never deleted. Called at a persisting boot.
pub(crate) fn backup_stale_resuming(path: &Path) {
    let parked = resuming_path(path);
    if parked.exists() {
        let _ = crate::player_profile::backup_file(&parked);
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

/// Reads the suspended run for a resume: the snapshot to restore (with held and pending input
/// removed, so the restore installs an idle input) and its frame. The `Err` is the player-readable
/// refusal reason.
pub(crate) fn read_resume(
    path: &Path,
    current: &BuildStamp,
) -> Result<(FrameIndex, Value), String> {
    let text = match std::fs::read_to_string(path) {
        Ok(text) => text,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return Err("there is no suspended run".to_string());
        }
        Err(e) => return Err(format!("the suspended run file is unreadable: {e}")),
    };
    let file: RunSaveFile = serde_json::from_str(&text)
        .map_err(|e| format!("the suspended run file is unreadable: {e}"))?;
    if file.format > RUN_SAVE_FORMAT {
        return Err("the suspended run was written by a newer version".to_string());
    }
    if let Some(reason) = stamp_mismatch(&file.build, current) {
        return Err(reason);
    }
    let mut snapshot = file.snapshot;
    let frame = snapshot
        .get("frame")
        .and_then(Value::as_u64)
        .ok_or_else(|| {
            "the suspended run file is unreadable: its snapshot has no frame".to_string()
        })?;
    if let Some(object) = snapshot.as_object_mut() {
        object.remove("input");
        object.remove("pending_input");
    }
    Ok((FrameIndex::new(frame), snapshot))
}

#[cfg(test)]
mod tests {
    use super::*;
    use schemars::JsonSchema;

    fn stamp(version: &str, schema: u64) -> BuildStamp {
        BuildStamp {
            engine_version: version.to_string(),
            game_schema: WireHash::of_bytes(&schema.to_le_bytes()),
            projectiles: None,
        }
    }

    #[test]
    fn the_projectile_config_is_part_of_the_stamp() {
        let a = stamp("0.1.0", 1);
        let mut pooled = a.clone();
        pooled.projectiles = Some(projectile_config_hash(&crate::ProjectileConfig::default()));
        let reason = stamp_mismatch(&a, &pooled).unwrap();
        assert!(reason.contains("projectile"), "{reason}");
        assert_eq!(stamp_mismatch(&pooled, &pooled.clone()), None);

        // An old file without the field reads as `None`, never a panic.
        let old = r#"{"engine_version":"0.1.0","game_schema":"0000000000000000"}"#;
        let parsed: BuildStamp = serde_json::from_str(old).unwrap();
        assert_eq!(parsed.projectiles, None);
        assert!(stamp_mismatch(&parsed, &pooled).is_some());
    }

    #[test]
    fn consume_parks_the_file_and_a_second_consume_finds_none() {
        let dir =
            std::env::temp_dir().join(format!("ludion-runsave-consume-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("suspend.json");
        assert_eq!(consume(&path), Ok(None));
        std::fs::write(&path, "x").unwrap();
        let parked = consume(&path).unwrap().unwrap();
        assert!(!path.exists());
        assert_eq!(parked, dir.join("suspend.json.resuming"));
        assert_eq!(std::fs::read_to_string(&parked).unwrap(), "x");
        assert!(read_pending(&path).is_none());
        assert_eq!(consume(&path), Ok(None));
        let _ = std::fs::remove_dir_all(&dir);
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

    #[test]
    fn read_resume_refuses_each_unreadable_or_foreign_file() {
        let dir =
            std::env::temp_dir().join(format!("ludion-runsave-resume-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("suspend.json");
        let _ = std::fs::remove_file(&path);
        let here = stamp("0.1.0", 1);
        let err = read_resume(&path, &here).unwrap_err();
        assert!(err.contains("no suspended run"), "{err}");

        std::fs::write(&path, "not json").unwrap();
        let err = read_resume(&path, &here).unwrap_err();
        assert!(err.contains("unreadable"), "{err}");

        let info = SuspendInfo {
            frame: FrameIndex::new(7),
            scene: None,
            saved_at_build: here.clone(),
        };
        let snapshot = json!({ "frame": 8, "input": {"held": 1}, "pending_input": [1], "rng": 3 });
        std::fs::write(&path, encode(&info, snapshot.clone())).unwrap();
        let (frame, value) = read_resume(&path, &here).unwrap();
        assert_eq!(frame, FrameIndex::new(8));
        assert_eq!(value, json!({ "frame": 8, "rng": 3 }));

        let err = read_resume(&path, &stamp("0.2.0", 1)).unwrap_err();
        assert!(err.contains("saved by engine"), "{err}");

        std::fs::write(&path, encode(&info, json!({ "rng": 3 }))).unwrap();
        assert!(read_resume(&path, &here).unwrap_err().contains("no frame"));

        let newer = encode(&info, snapshot).replacen("\"format\":1", "\"format\":2", 1);
        std::fs::write(&path, newer).unwrap();
        let err = read_resume(&path, &here).unwrap_err();
        assert!(err.contains("newer version"), "{err}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A restore under `Refuse` into `target` fails with `L1401` and leaves the engine as it was.
    fn assert_refused_untouched(target: &mut crate::Engine, snapshot: &Value, why: &str) {
        use crate::snapshot::{DriftPolicy, restore_into};

        let hash = target.world_hash().unwrap();
        let frame = target.frame();
        let error = restore_into(target, snapshot, DriftPolicy::Refuse).unwrap_err();
        assert_eq!(error.code.as_str(), "L1401");
        assert!(error.message.contains(why), "{}", error.message);
        assert_eq!(target.world_hash().unwrap(), hash);
        assert_eq!(target.frame(), frame);
    }

    fn pooled_source(capacity: u32) -> crate::Engine {
        let mut source = crate::Engine::builder()
            .projectiles(crate::ProjectileConfig {
                capacity,
                ..crate::ProjectileConfig::default()
            })
            .build()
            .unwrap();
        source.load_inline_scene("format: 1\nseed: 1\n").unwrap();
        for _ in 0..4 {
            let desc = ludion_projectiles::ProjectileDesc::new([0.0; 3], [1.0, 0.0, 0.0]);
            crate::spawn_projectile(source.world_mut(), desc).unwrap();
        }
        source
    }

    #[test]
    fn a_dropped_projectile_pool_refuses_under_refuse() {
        let snapshot = crate::snapshot::capture(&pooled_source(16)).unwrap();
        let mut bare = crate::Engine::new();
        assert_refused_untouched(&mut bare, &snapshot, "no projectile pool");
    }

    #[test]
    fn a_pool_too_small_for_the_snapshot_refuses_under_refuse() {
        let snapshot = crate::snapshot::capture(&pooled_source(16)).unwrap();
        let mut small = crate::Engine::builder()
            .projectiles(crate::ProjectileConfig {
                capacity: 2,
                ..crate::ProjectileConfig::default()
            })
            .build()
            .unwrap();
        assert_refused_untouched(&mut small, &snapshot, "lose the projectiles");
    }

    #[test]
    fn solver_state_without_settings_refuses_under_refuse() {
        let mut source = crate::Engine::new();
        source
            .load_inline_scene("format: 1\nphysics:\n  space: free\nentities:\n  - name: floor\n    components:\n      Transform:\n        translation:\n          - 0.0\n          - -0.5\n          - 0.0\n      Collider:\n        shape:\n          box:\n            half_extents:\n              - 10.0\n              - 0.5\n              - 10.0\n        body: static\n")
            .unwrap();
        source.advance_live();
        let mut snapshot = crate::snapshot::capture(&source).unwrap();
        assert!(snapshot.get("physics").is_some());
        snapshot.as_object_mut().unwrap().remove("scene_physics");
        let mut target = crate::Engine::new();
        assert_refused_untouched(&mut target, &snapshot, "lose physics");
    }

    #[test]
    fn refusing_drift_leaves_the_engine_untouched() {
        use crate::snapshot::{DriftPolicy, capture, restore_into};

        let mut source = crate::Engine::builder()
            .components(|registry| {
                registry.register::<Plain>("Gear")?;
                Ok(())
            })
            .build()
            .unwrap();
        source
            .load_inline_scene(
                "format: 1\nentities:\n  - name: g\n    components:\n      Gear:\n        hp: 3\n",
            )
            .unwrap();
        let snapshot = capture(&source).unwrap();

        let mut target = crate::Engine::new();
        let hash = target.world_hash().unwrap();
        let frame = target.frame();
        let error = restore_into(&mut target, &snapshot, DriftPolicy::Refuse).unwrap_err();
        assert_eq!(error.code.as_str(), "L1401");
        assert!(error.message.contains("Gear"), "{}", error.message);
        assert_eq!(target.world_hash().unwrap(), hash);
        assert_eq!(target.frame(), frame);

        let report = restore_into(&mut target, &snapshot, DriftPolicy::Migrate).unwrap();
        assert_eq!(report.entries.len(), 1);
    }
}
