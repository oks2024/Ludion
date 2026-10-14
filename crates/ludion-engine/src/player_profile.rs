//! Player profiles: a game registers one serde save-state type ([`EngineBuilder::profile`]) and gets
//! a [`Profile<P>`] resource that boot loads from `save/profile.yaml`, migrates leniently across
//! schema drift, and persists back to disk (Phase 16 / E5, `Plan/phase-16-profile.md`).
//!
//! This is the singleton analog of the [`DocKind`](crate::docs)/[`DocStore`](crate::DocStore)
//! machinery: where a data-doc kind loads a *collection* of authored `*.yaml` under the content root
//! into a `DocStore<T>`, a profile loads *one* `save/profile.yaml` into a `Profile<P>` resource that
//! the sim mutates. The two diverge in one crucial way — a data-doc is authored content an agent must
//! fix, so [`ludion_scene::check_doc`] hard-rejects any drift; a profile is a *player's* accumulated
//! save, so it must **migrate**, not reject. The lenient load ([`ProfileKind::load`]) fills fields the
//! game added from serde defaults, drops fields the game removed, and, when the file cannot be honored
//! at all (a bad envelope, a retyped field, a newer-than-engine `format`), **backs it up** and starts
//! a fresh default — never a crash, never a hard reject, never a destroyed save. The changes are
//! returned as a [`ProfileMigration`] the host prints and tests inspect.
//!
//! Like [`DocKind`](crate::docs), a [`ProfileKind`] is `(schema + monomorphized fn pointers)` held
//! persistently on the [`Engine`](crate::Engine) and re-run after every world reset (the
//! `install_audio_queue` mold), so `ludion-scene`/the engine core never name the concrete `P` — this
//! module owns the one place `P`'s fields are known.
//!
//! [`EngineBuilder::profile`]: crate::EngineBuilder::profile

use std::path::{Path, PathBuf};

use ludion_core::time::FrameIndex;
use ludion_ecs::World;
use ludion_protocol::profile::{ProfileMigration, ProfileMigrationEntry};
use schemars::JsonSchema;
use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::Value;

/// The profile document `format:` version this engine build reads and writes. A file declaring a
/// **newer** format than this is defaulted, and backed up by a persisting engine (an older build must
/// not silently downgrade a save a newer build wrote).
const PROFILE_FORMAT: u64 = 1;

/// The default save location, resolved relative to the save root (default: the project root — the
/// scene's directory). Overridable with
/// [`EngineBuilder::profile_path`](crate::EngineBuilder::profile_path).
const DEFAULT_PROFILE_PATH: &str = "save/profile.yaml";

/// The default profile path (`save/profile.yaml`) as a [`PathBuf`], for the builder to fall back on.
pub(crate) fn default_profile_rel_path() -> PathBuf {
    PathBuf::from(DEFAULT_PROFILE_PATH)
}

/// A game's persistent player save state, held as a world resource: the `P` value plus a `dirty`
/// flag a game sets by calling [`save`](Profile::save).
///
/// A resource, not a component: like [`DocStore`](crate::DocStore) it never enters the `WorldHash`
/// and trips no drift gate — its numbers reach the hash only through the components a game's systems
/// derive from it (an unlock spawning a different starting entity), which is exactly why the
/// profile-at-start must travel in a replay for reproducibility. A game reads it with
/// `world.resource::<Profile<P>>()` and mutates it with `world.resource_mut::<Profile<P>>()`.
///
/// The `dirty` flag is the save request: a system that changes the profile and wants it persisted
/// calls [`save`](Profile::save); the engine drains the flag at the frame boundary and (when
/// persistence is enabled) writes the file atomically.
///
/// # Example
/// ```
/// use ludion_engine::{Engine, Profile};
/// use schemars::JsonSchema;
/// use serde::{Deserialize, Serialize};
///
/// #[derive(Serialize, Deserialize, JsonSchema, Default)]
/// struct Save {
///     unlocked_crossbow: bool,
///     runs: u32,
/// }
///
/// # let dir = std::env::temp_dir().join(format!("ludion-profile-doc-{}", std::process::id()));
/// # let _ = std::fs::remove_dir_all(&dir);
/// let mut engine = Engine::builder()
///     .profile::<Save>()
///     .project_root(&dir)
///     .build()
///     .unwrap();
///
/// // No file on disk yet → the profile starts from `Default`, and the load is clean.
/// let save = engine.world().resource::<Profile<Save>>().unwrap();
/// assert_eq!(save.get().runs, 0);
/// assert!(engine.profile_migration().is_clean());
/// # let _ = std::fs::remove_dir_all(&dir);
/// ```
pub struct Profile<P> {
    value: P,
    /// Whether a game system has requested this profile be saved since the last drain. Taken and
    /// cleared by the engine's frame-boundary save drain.
    pub(crate) dirty: bool,
}

impl<P> Profile<P> {
    /// A fresh, not-yet-dirty profile wrapping `value`.
    pub(crate) fn new(value: P) -> Self {
        Profile {
            value,
            dirty: false,
        }
    }

    /// The saved state, for reading.
    pub fn get(&self) -> &P {
        &self.value
    }

    /// The saved state, for mutation. Changing it does **not** by itself request a save — call
    /// [`save`](Profile::save) to mark the profile for persistence at the next frame boundary.
    pub fn get_mut(&mut self) -> &mut P {
        &mut self.value
    }

    /// Requests that this profile be persisted: sets the `dirty` flag the engine drains at the frame
    /// boundary. Idempotent within a frame — several `save()` calls collapse into one write.
    pub fn save(&mut self) {
        self.dirty = true;
    }
}

/// One drained profile-save request, stamped with the frame during which a game system called
/// [`Profile::save`].
///
/// The engine's [profile save log](crate::Engine::profile_save_log) is a `Vec` of these, in request
/// order — the [`AudioLogEntry`](crate::AudioLogEntry) mold. Like audio, a save request is a pure
/// function of the sim, so a replay regenerates the identical stream and it is never recorded; several
/// `save()` calls within one frame collapse into a single entry (one drain per frame boundary).
#[derive(Clone, Debug, PartialEq)]
pub struct ProfileSaveEntry {
    /// The frame whose systems requested the save.
    pub frame: FrameIndex,
}

/// What the non-generic file resolution decided the monomorphized installer should do — the seam
/// between the P-agnostic IO ([`resolve_profile_source`]) and the P-typed install
/// ([`install_profile_resource`]).
enum ProfileSource {
    /// No file on disk (first boot) — load `P::default()`, a clean report. Not an error, not a reset.
    Default,
    /// The file existed and its `format`/`kind` envelope parsed with an acceptable version — hand its
    /// stripped field map to serde. A `from_value` failure here (a retyped field) triggers the
    /// installer's own backup.
    Fields(Value),
    /// The file could not be honored at all (a bad envelope, an unreadable file, or a newer-than-
    /// engine `format`) and has already been backed up (or, on a non-persisting engine, left in
    /// place with an empty `backup`) — load `P::default()` and report the reset.
    Reset { backup: String, reason: String },
}

/// A profile kind's monomorphized installer: build a `Profile<P>` from the resolved source (or
/// `P::default()`), insert it into the world, and return the migration report. The `bool` is
/// whether an unloadable file may be backed up (a persisting engine).
type ProfileInstaller = fn(&mut World, &Path, ProfileSource, bool) -> ProfileMigration;

/// A profile kind's monomorphized install-from-text path: parse engine-emitted canonical profile
/// `text` and build a `Profile<P>` from it (or `P::default()` on any failure), never touching disk —
/// the replay-header / snapshot-keyframe continuation seam ([`install_profile_from_text_resource`]).
type ProfileTextInstaller = fn(&mut World, &str) -> ProfileMigration;

/// A profile kind's monomorphized emitter: read the live `Profile<P>` and render it to canonical text
/// via [`ludion_scene::emit_doc`] with `kind: profile`. The frame-boundary save drain's renderer.
type ProfileEmitter = fn(&World, &Value) -> Option<String>;

/// A profile kind's monomorphized dirty-flag drain: clear the live `Profile<P>`'s save-request flag
/// and report whether a save was requested since the last drain (false if the resource is absent).
/// The seam the engine's [`drain_profile`](crate::Engine) reads without naming `P`.
type ProfileDirtyTaker = fn(&mut World) -> bool;

/// The one registered profile type, held persistently on the [`Engine`](crate::Engine): its captured
/// schema and monomorphized `fn` pointers. The singleton analog of [`DocKind`](crate::docs::DocKind)
/// — a `fn` pointer is `Copy`, so the registration survives the `SimDriver` resource wipe and the
/// engine re-runs it after every world reset.
///
/// A profile type with a top-level `format` or `kind` field is refused when the engine is built
/// (`L1257`): it would collide with the envelope.
pub(crate) struct ProfileKind {
    /// The captured `schema_for!(P)`, used by [`emit`](ProfileKind::emit) to write fields in schema
    /// order.
    schema: Value,
    install: ProfileInstaller,
    install_text: ProfileTextInstaller,
    emit: ProfileEmitter,
    take_dirty: ProfileDirtyTaker,
}

impl ProfileKind {
    /// The captured `schema_for!(P)`.
    pub(crate) fn schema(&self) -> &Value {
        &self.schema
    }

    /// Captures a profile type's `(schema, monomorphized installer/emitter)` — the value
    /// [`EngineBuilder::profile`](crate::EngineBuilder::profile) records.
    pub(crate) fn new<P>() -> Self
    where
        P: Serialize + DeserializeOwned + JsonSchema + Default + Send + Sync + 'static,
    {
        ProfileKind {
            schema: serde_json::to_value(schemars::schema_for!(P))
                .expect("a schemars JSON Schema always serializes to a JSON value"),
            install: install_profile_resource::<P>,
            install_text: install_profile_from_text_resource::<P>,
            emit: emit_profile::<P>,
            take_dirty: take_profile_dirty::<P>,
        }
    }

    /// Loads the profile at `path` into `world`, returning the lenient migration report. A missing
    /// file is a silent load-or-default (first boot, clean report); an unloadable one is defaulted (an
    /// `L1121` [`Reset`](ProfileMigrationEntry::Reset) entry). A *persisting* engine (`backup`) also
    /// renames the file aside; any other engine leaves it in place and reports an empty `backup`, so
    /// a test or replay never moves the developer's save. Never panics on file contents.
    pub(crate) fn load(&self, path: &Path, world: &mut World, backup: bool) -> ProfileMigration {
        let source = resolve_profile_source(path, backup);
        (self.install)(world, path, source, backup)
    }

    /// Installs the profile directly from engine-emitted canonical `text` (a replay header's or a
    /// snapshot keyframe's embedded profile-at-start), never reading disk. The text normally
    /// round-trips exactly, so the report is usually clean; a build whose `P` drifted from the embedded
    /// text still migrates leniently (filled/dropped fields reported), and unparseable text falls back
    /// to `P::default()` with an `L1121` [`Reset`](ProfileMigrationEntry::Reset) carrying an empty
    /// `backup` (there is no on-disk file to preserve). Never panics on the text's contents.
    pub(crate) fn load_text(&self, text: &str, world: &mut World) -> ProfileMigration {
        (self.install_text)(world, text)
    }

    /// Renders the live `Profile<P>` to canonical profile text (`kind: profile`), or `None` if no
    /// profile resource is installed. The frame-boundary save drain's renderer.
    pub(crate) fn emit(&self, world: &World) -> Option<String> {
        (self.emit)(world, &self.schema)
    }

    /// Clears the live `Profile<P>`'s save-request flag, returning whether a save was requested since
    /// the last drain (false with no profile resource). The frame-boundary save drain's seam.
    pub(crate) fn take_dirty(&self, world: &mut World) -> bool {
        (self.take_dirty)(world)
    }
}

/// Resolves the on-disk profile at `path` into the installer's [`ProfileSource`] — the P-agnostic
/// half of the load. A missing file is [`Default`](ProfileSource::Default) (first boot); a bad
/// envelope, an unreadable file, or a newer-than-engine `format` is backed up (when `backup`) and
/// reported as a [`Reset`](ProfileSource::Reset); a well-formed envelope hands its stripped fields to
/// the installer.
fn resolve_profile_source(path: &Path, backup: bool) -> ProfileSource {
    let text = match std::fs::read_to_string(path) {
        Ok(text) => text,
        // A missing file is the common first-boot case: load-or-default silently, no reset.
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return ProfileSource::Default;
        }
        // Present but unreadable (permissions, invalid UTF-8): back it up and default.
        Err(error) => {
            return reset_by_backup(
                path,
                format!("the profile file could not be read: {error}"),
                backup,
            );
        }
    };
    match ludion_scene::open_profile(&text) {
        Err(diagnostics) => reset_by_backup(path, join_reasons(&diagnostics), backup),
        Ok(envelope) if envelope.format > PROFILE_FORMAT => reset_by_backup(
            path,
            format!(
                "the profile declares `format: {}`, newer than this build reads (format {PROFILE_FORMAT}); \
                 an older build must not overwrite a newer save",
                envelope.format
            ),
            backup,
        ),
        Ok(envelope) => ProfileSource::Fields(envelope.fields),
    }
}

/// The [`Reset`](ProfileSource::Reset) source for an unloadable file at `path`: see [`back_up_or_keep`].
fn reset_by_backup(path: &Path, reason: String, backup: bool) -> ProfileSource {
    let (backup, reason) = back_up_or_keep(path, reason, backup);
    ProfileSource::Reset { backup, reason }
}

/// Backs up the file at `path` when `backup` is set, returning the `(backup name, reason)` pair a
/// [`Reset`](ProfileMigrationEntry::Reset) carries. Without `backup` (an engine that does not persist)
/// the file is left in place with an empty backup name. If the rename itself fails the original is
/// also left in place (the rename never happened) and the failure is folded into the reason — the game
/// still boots on a default profile, and the player's bytes are untouched.
fn back_up_or_keep(path: &Path, reason: String, backup: bool) -> (String, String) {
    if !backup {
        return (
            String::new(),
            format!("{reason}; profile persistence is off, so the file was left in place"),
        );
    }
    match backup_file(path) {
        Ok(name) => (name, reason),
        Err(error) => (
            String::new(),
            format!(
                "{reason}; the original could not be backed up ({error}), so it was left in place"
            ),
        ),
    }
}

/// Renames `path` to a content-hashed sibling `<name>.bak.<hash>` (e.g.
/// `profile.yaml.bak.3f9a…`) and returns the backup file name. The hash is
/// [`ludion_assets::content_hash`] (xxh3) of the file's bytes, so the same corrupt save always backs
/// up to the same name (no timestamp non-determinism) and two distinct corrupt saves never collide.
///
/// Never overwrites or deletes the player's bytes: it is a pure rename. `std::fs::rename` replaces
/// atomically on both Windows and POSIX.
fn backup_file(path: &Path) -> std::io::Result<String> {
    let bytes = std::fs::read(path)?;
    let hash = ludion_assets::content_hash(&bytes);
    let file_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("profile.yaml");
    let backup_name = format!("{file_name}.bak.{hash:016x}");
    std::fs::rename(path, path.with_file_name(&backup_name))?;
    Ok(backup_name)
}

/// Collapses a `Vec<Diagnostic>` (an `open_profile` envelope failure) into one reason string for a
/// [`Reset`](ProfileMigrationEntry::Reset) entry, joining each diagnostic's message.
fn join_reasons(diagnostics: &[ludion_core::diagnostics::Diagnostic]) -> String {
    diagnostics
        .iter()
        .map(|diagnostic| diagnostic.message.clone())
        .collect::<Vec<_>>()
        .join("; ")
}

/// The monomorphized body of every [`ProfileKind::install`]: build a `Profile<P>` from `source` and
/// insert it, returning the migration report. On a [`Fields`](ProfileSource::Fields) source whose
/// value no longer deserializes into `P` (a retyped field), the file is backed up here and a fresh
/// `P::default()` starts — the one place the from-value outcome (which needs `P`) meets the backup.
fn install_profile_resource<P>(
    world: &mut World,
    path: &Path,
    source: ProfileSource,
    backup: bool,
) -> ProfileMigration
where
    P: Serialize + DeserializeOwned + Default + Send + Sync + 'static,
{
    match source {
        ProfileSource::Default => {
            world.insert_resource(Profile::new(P::default()));
            ProfileMigration::default()
        }
        ProfileSource::Reset { backup, reason } => {
            world.insert_resource(Profile::new(P::default()));
            reset_report(backup, reason)
        }
        ProfileSource::Fields(fields) => match serde_json::from_value::<P>(fields.clone()) {
            Ok(value) => {
                // Diff the file's keys against the reserialized value's: keys only in the reserialized
                // form were filled from defaults (the type grew); keys only in the file were dropped
                // (the type shrank). `field_diff::diff_fields`.
                let reserialized = serde_json::to_value(&value).unwrap_or(Value::Null);
                let report = field_drift_report(&fields, &reserialized);
                world.insert_resource(Profile::new(value));
                report
            }
            // A retyped field: serde is all-or-nothing on a struct, so the whole value is unloadable.
            // Back it up (preserving the player's bytes) and start a fresh default.
            Err(error) => {
                world.insert_resource(Profile::new(P::default()));
                let (backup, reason) = back_up_or_keep(
                    path,
                    format!(
                        "the saved profile no longer deserializes into the profile type: {error}"
                    ),
                    backup,
                );
                reset_report(backup, reason)
            }
        },
    }
}

/// The monomorphized body of every [`ProfileKind::load_text`]: parse engine-emitted canonical profile
/// `text`, build a `Profile<P>` from its stripped fields, and insert it — **never touching disk**.
///
/// The text is normally this build's own `emit_profile` output, so it round-trips exactly and the
/// report is clean; the lenient path is still honored for the dev-reload case (a snapshot from a build
/// whose `P` differs): added fields fill from defaults and removed fields drop, both reported. Any
/// failure to honor the text at all — a bad envelope, a value that no longer deserializes, or a
/// newer-than-engine `format` — backs off to `P::default()` with an `L1121` reset whose `backup` is
/// empty (there is no file to back up; the reason names the embedded-text failure).
fn install_profile_from_text_resource<P>(world: &mut World, text: &str) -> ProfileMigration
where
    P: Serialize + DeserializeOwned + Default + Send + Sync + 'static,
{
    match ludion_scene::open_profile(text) {
        Ok(envelope) if envelope.format <= PROFILE_FORMAT => {
            match serde_json::from_value::<P>(envelope.fields.clone()) {
                Ok(value) => {
                    let reserialized = serde_json::to_value(&value).unwrap_or(Value::Null);
                    let report = field_drift_report(&envelope.fields, &reserialized);
                    world.insert_resource(Profile::new(value));
                    report
                }
                Err(error) => reset_to_default::<P>(
                    world,
                    format!(
                        "the embedded profile no longer deserializes into the profile type: {error}"
                    ),
                ),
            }
        }
        Ok(envelope) => reset_to_default::<P>(
            world,
            format!(
                "the embedded profile declares `format: {}`, newer than this build reads (format \
                 {PROFILE_FORMAT})",
                envelope.format
            ),
        ),
        Err(diagnostics) => reset_to_default::<P>(world, join_reasons(&diagnostics)),
    }
}

/// Inserts a fresh `P::default()` profile and returns a single-entry `L1121`
/// [`Reset`](ProfileMigrationEntry::Reset) with an **empty** backup — the failure shape for the
/// off-disk install path, where there is no file to preserve (unlike the disk load, which backs the
/// file up first).
fn reset_to_default<P>(world: &mut World, reason: String) -> ProfileMigration
where
    P: Default + Send + Sync + 'static,
{
    world.insert_resource(Profile::new(P::default()));
    reset_report(String::new(), reason)
}

/// The monomorphized body of every [`ProfileKind::emit`]: read the live `Profile<P>` and render it to
/// canonical text with `kind: profile`, or `None` if the resource is absent.
fn emit_profile<P>(world: &World, schema: &Value) -> Option<String>
where
    P: Serialize + 'static,
{
    let profile = world.resource::<Profile<P>>()?;
    let value = serde_json::to_value(profile.get()).ok()?;
    Some(ludion_scene::emit_doc("profile", &value, schema))
}

/// The monomorphized body of every [`ProfileKind::take_dirty`]: clear the live `Profile<P>`'s `dirty`
/// flag and report whether a save was requested since the last drain. `false` if the resource is
/// absent (a world reset before the reinstall, or no registered profile).
fn take_profile_dirty<P>(world: &mut World) -> bool
where
    P: 'static + Send + Sync,
{
    match world.resource_mut::<Profile<P>>() {
        Some(profile) => std::mem::take(&mut profile.dirty),
        None => false,
    }
}

/// A single-entry [`ProfileMigration`] carrying a [`Reset`](ProfileMigrationEntry::Reset).
fn reset_report(backup: String, reason: String) -> ProfileMigration {
    ProfileMigration {
        entries: vec![ProfileMigrationEntry::Reset { backup, reason }],
    }
}

/// A [`ProfileMigration`] reporting the field drift between the file's `fields` and the reserialized
/// value `reserialized`: keys only in `reserialized` were filled from defaults, keys only in `fields`
/// were dropped. An empty report (no drift) is a clean load.
fn field_drift_report(fields: &Value, reserialized: &Value) -> ProfileMigration {
    let (filled, dropped) = crate::field_diff::diff_fields(fields, reserialized);
    let mut entries = Vec::new();
    if !filled.is_empty() {
        entries.push(ProfileMigrationEntry::FieldsDefaulted { fields: filled });
    }
    if !dropped.is_empty() {
        entries.push(ProfileMigrationEntry::FieldsDropped { fields: dropped });
    }
    ProfileMigration { entries }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A scratch directory unique to this process + `name`, removed first so a re-run starts clean.
    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("ludion-atomic-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn atomic_write_never_exposes_a_partial_dest() {
        // Drive `write_atomic`'s two steps by hand to observe the intermediate state a crash would
        // leave: after the temp is staged but before the rename, `dest` must still hold the OLD bytes.
        let dir = scratch("partial");
        let dest = dir.join("profile.yaml");
        std::fs::write(&dest, "OLD").unwrap();

        let tmp = ludion_core::atomic::temp_sibling(&dest);
        std::fs::write(&tmp, "NEW").unwrap();
        assert_eq!(
            std::fs::read_to_string(&dest).unwrap(),
            "OLD",
            "before the rename, the destination still holds the complete previous save"
        );
        assert!(tmp.exists());

        std::fs::rename(&tmp, &dest).unwrap();
        assert_eq!(
            std::fs::read_to_string(&dest).unwrap(),
            "NEW",
            "after the rename, the destination is the complete new save"
        );
        assert!(!tmp.exists(), "the temp is consumed by the rename");

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn write_atomic_creates_parent_dirs_and_replaces_in_place() {
        let dir = scratch("replace");
        let dest = dir.join("save").join("profile.yaml");
        ludion_core::atomic::write_atomic(&dest, "first")
            .expect("creates the missing save/ directory and writes");
        assert_eq!(std::fs::read_to_string(&dest).unwrap(), "first");

        ludion_core::atomic::write_atomic(&dest, "second").expect("replaces in place");
        assert_eq!(std::fs::read_to_string(&dest).unwrap(), "second");
        assert!(
            !ludion_core::atomic::temp_sibling(&dest).exists(),
            "no temp is left behind after a successful write"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }
}
