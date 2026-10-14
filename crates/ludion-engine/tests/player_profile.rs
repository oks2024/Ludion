//! Player-profile boot tests (Phase 16 / E5, Wave 2): `EngineBuilder::profile::<P>` loads
//! `save/profile.yaml` from the project root into a `Profile<P>` resource at every world build,
//! migrating leniently across schema drift — filling added fields from defaults, dropping removed
//! ones, and backing up + defaulting a save it cannot honor (an `L1121` reset) — never crashing and
//! never destroying a player's bytes. The resource survives `scene.load` and reaches the `WorldHash`
//! only through the components a game derives from it.

use ludion_engine::{Engine, EngineBuilder, Profile};
use ludion_protocol::profile::ProfileMigrationEntry;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// The v1 profile type: two fields.
#[derive(Serialize, Deserialize, JsonSchema, Default, Clone, Debug, PartialEq)]
struct SaveV1 {
    unlocked_crossbow: bool,
    runs: u32,
}

/// A v2 with a field **added** (`deaths`) relative to v1 — a v1 file lacks it, so it fills from
/// default. `#[serde(default)]` is the growable-profile contract (decision 3): a field a game adds
/// carries a serde default so an old save that lacks it fills rather than failing to deserialize.
#[derive(Serialize, Deserialize, JsonSchema, Default, Clone, Debug, PartialEq)]
#[serde(default)]
struct SaveAddedField {
    unlocked_crossbow: bool,
    runs: u32,
    deaths: u32,
}

/// A v2 with a field **removed** (`runs` gone) relative to v1 — a v1 file's `runs` is dropped.
#[derive(Serialize, Deserialize, JsonSchema, Default, Clone, Debug, PartialEq)]
struct SaveRemovedField {
    unlocked_crossbow: bool,
}

/// A v2 with a field **retyped** (`runs: u32` → `runs: String`) relative to v1 — a v1 file's numeric
/// `runs` no longer deserializes, so the whole save is backed up and defaulted.
#[derive(Serialize, Deserialize, JsonSchema, Default, Clone, Debug, PartialEq)]
struct SaveRetypedField {
    unlocked_crossbow: bool,
    runs: String,
}

/// A fresh, empty temp directory unique to this process + `name` (removed first, so a re-run starts
/// clean). The project root a builder is pointed at.
fn fresh_dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("ludion-profile-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// Writes `contents` to `root`/`rel`, creating parent directories.
fn write(root: &Path, rel: &str, contents: &str) {
    let path = root.join(rel);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, contents).unwrap();
}

/// A canonical v1 profile with `unlocked_crossbow: true`, `runs: 7`.
const UNLOCKED_V1: &str = "format: 1\nkind: profile\nunlocked_crossbow: true\nruns: 7\n";

#[test]
fn boots_a_valid_file_into_the_profile() {
    let dir = fresh_dir("valid");
    write(&dir, "save/profile.yaml", UNLOCKED_V1);

    let engine = Engine::builder()
        .profile::<SaveV1>()
        .project_root(&dir)
        .build()
        .expect("a valid profile boots");

    let profile = engine.world().resource::<Profile<SaveV1>>().unwrap();
    assert_eq!(
        profile.get(),
        &SaveV1 {
            unlocked_crossbow: true,
            runs: 7,
        }
    );
    assert!(
        engine.profile_migration().is_clean(),
        "a file matching the type exactly loads cleanly: {:?}",
        engine.profile_migration()
    );
}

#[test]
fn a_missing_file_defaults_silently_and_creates_no_save_dir() {
    let dir = fresh_dir("missing");
    // No save/profile.yaml on disk.

    let engine = Engine::builder()
        .profile::<SaveV1>()
        .project_root(&dir)
        .build()
        .expect("a missing profile is not an error");

    let profile = engine.world().resource::<Profile<SaveV1>>().unwrap();
    assert_eq!(profile.get(), &SaveV1::default());
    assert!(
        engine.profile_migration().is_clean(),
        "a first boot with no file is a clean default, not a reset: {:?}",
        engine.profile_migration()
    );
    assert!(
        !dir.join("save").exists(),
        "a load-or-default must not write anything, not even the save/ directory"
    );
}

#[test]
fn a_field_added_to_the_type_fills_from_default_and_reports_it() {
    let dir = fresh_dir("added");
    // A v1 file lacks `deaths`, which the current type declares.
    write(&dir, "save/profile.yaml", UNLOCKED_V1);

    let engine = Engine::builder()
        .profile::<SaveAddedField>()
        .project_root(&dir)
        .build()
        .expect("a drifted-but-loadable profile boots");

    let profile = engine
        .world()
        .resource::<Profile<SaveAddedField>>()
        .unwrap();
    assert_eq!(
        profile.get(),
        &SaveAddedField {
            unlocked_crossbow: true,
            runs: 7,
            deaths: 0,
        }
    );
    let entries = &engine.profile_migration().entries;
    assert_eq!(
        entries.as_slice(),
        [ProfileMigrationEntry::FieldsDefaulted {
            fields: vec!["deaths".to_string()],
        }],
        "the added field is reported as filled-from-default"
    );
}

#[test]
fn a_field_removed_from_the_type_is_dropped_and_reported() {
    let dir = fresh_dir("removed");
    // A v1 file has `runs`, which the current type no longer declares.
    write(&dir, "save/profile.yaml", UNLOCKED_V1);

    let engine = Engine::builder()
        .profile::<SaveRemovedField>()
        .project_root(&dir)
        .build()
        .expect("a shrunk-type profile boots");

    let profile = engine
        .world()
        .resource::<Profile<SaveRemovedField>>()
        .unwrap();
    assert_eq!(
        profile.get(),
        &SaveRemovedField {
            unlocked_crossbow: true,
        }
    );
    let entries = &engine.profile_migration().entries;
    assert_eq!(
        entries.as_slice(),
        [ProfileMigrationEntry::FieldsDropped {
            fields: vec!["runs".to_string()],
        }],
        "the removed field is reported as dropped"
    );
}

#[test]
fn a_retyped_field_backs_up_the_original_and_defaults() {
    let dir = fresh_dir("retyped");
    // A v1 file's numeric `runs` no longer deserializes into `runs: String`.
    write(&dir, "save/profile.yaml", UNLOCKED_V1);

    let engine = Engine::builder()
        .profile::<SaveRetypedField>()
        .project_root(&dir)
        .persist_profile()
        .build()
        .expect("an unloadable profile never fails the boot");

    // The profile started from default.
    let profile = engine
        .world()
        .resource::<Profile<SaveRetypedField>>()
        .unwrap();
    assert_eq!(profile.get(), &SaveRetypedField::default());

    // The report is a single L1121 Reset naming the backup.
    let entries = &engine.profile_migration().entries;
    assert_eq!(entries.len(), 1, "{entries:?}");
    let ProfileMigrationEntry::Reset { backup, .. } = &entries[0] else {
        panic!("expected a Reset entry, got {:?}", entries[0]);
    };
    assert_eq!(entries[0].code(), Some("L1121"));

    // The original file was moved aside (not overwritten in place) and holds the ORIGINAL bytes.
    assert!(
        !dir.join("save/profile.yaml").exists(),
        "the unloadable save was renamed away, not left where a new default would clobber it"
    );
    let backup_bytes = std::fs::read_to_string(dir.join("save").join(backup))
        .expect("the backup file exists at the reported name");
    assert_eq!(
        backup_bytes, UNLOCKED_V1,
        "the backup preserves the player's exact original bytes"
    );
}

#[test]
fn corrupt_yaml_backs_up_and_defaults_without_panicking() {
    let dir = fresh_dir("corrupt");
    // Not a valid profile envelope at all.
    let garbage = "this: is: not: valid: profile\n\t bad indentation\n";
    write(&dir, "save/profile.yaml", garbage);

    let engine = Engine::builder()
        .profile::<SaveV1>()
        .project_root(&dir)
        .persist_profile()
        .build()
        .expect("a corrupt profile never crashes the boot");

    assert_eq!(
        engine.world().resource::<Profile<SaveV1>>().unwrap().get(),
        &SaveV1::default()
    );
    let entries = &engine.profile_migration().entries;
    assert_eq!(entries.len(), 1, "{entries:?}");
    assert_eq!(entries[0].code(), Some("L1121"));
    assert!(
        !dir.join("save/profile.yaml").exists(),
        "the corrupt save was backed up, not left in place"
    );
}

#[test]
fn a_newer_format_backs_up_and_defaults() {
    let dir = fresh_dir("newer-format");
    // A well-formed profile but declaring a format this build does not read.
    write(
        &dir,
        "save/profile.yaml",
        "format: 2\nkind: profile\nunlocked_crossbow: true\nruns: 7\n",
    );

    let engine = Engine::builder()
        .profile::<SaveV1>()
        .project_root(&dir)
        .persist_profile()
        .build()
        .expect("a newer-format profile never crashes the boot");

    assert_eq!(
        engine.world().resource::<Profile<SaveV1>>().unwrap().get(),
        &SaveV1::default(),
        "an older build must not silently downgrade a newer save"
    );
    let entries = &engine.profile_migration().entries;
    assert_eq!(entries.len(), 1, "{entries:?}");
    assert_eq!(entries[0].code(), Some("L1121"));
}

#[test]
fn the_profile_survives_scene_load() {
    let dir = fresh_dir("scene-load");
    write(&dir, "save/profile.yaml", UNLOCKED_V1);

    let mut engine = Engine::builder()
        .profile::<SaveV1>()
        .project_root(&dir)
        .build()
        .expect("boots");
    assert_eq!(
        engine
            .world()
            .resource::<Profile<SaveV1>>()
            .unwrap()
            .get()
            .runs,
        7
    );

    // A `scene.load` resets the world (wiping resources); the profile must be carried from the build.
    engine
        .load_inline_scene("format: 1\nentities: []\n")
        .expect("scene loads");

    assert_eq!(
        engine.world().resource::<Profile<SaveV1>>().unwrap().get(),
        &SaveV1 {
            unlocked_crossbow: true,
            runs: 7,
        },
        "the profile is rebuilt after scene.load, not lost to the resource wipe"
    );
}

#[test]
fn an_unloadable_save_is_left_in_place_without_persistence() {
    let dir = fresh_dir("no-persist-corrupt");
    let garbage = "this: is: not: valid: profile\n\t bad indentation\n";
    write(&dir, "save/profile.yaml", garbage);

    let engine = Engine::builder()
        .profile::<SaveV1>()
        .project_root(&dir)
        .build()
        .expect("a corrupt profile never crashes the boot");

    assert_eq!(
        engine.world().resource::<Profile<SaveV1>>().unwrap().get(),
        &SaveV1::default()
    );
    let entries = &engine.profile_migration().entries;
    assert_eq!(entries.len(), 1, "{entries:?}");
    assert_eq!(entries[0].code(), Some("L1121"));
    let ProfileMigrationEntry::Reset { backup, .. } = &entries[0] else {
        panic!("expected a Reset entry, got {:?}", entries[0]);
    };
    assert!(backup.is_empty(), "nothing was renamed, so no backup name");
    assert_eq!(
        std::fs::read_to_string(dir.join("save/profile.yaml")).unwrap(),
        garbage,
        "a non-persisting engine leaves the developer's save untouched"
    );
    let siblings: Vec<_> = std::fs::read_dir(dir.join("save"))
        .unwrap()
        .map(|entry| entry.unwrap().file_name())
        .collect();
    assert_eq!(siblings, ["profile.yaml"], "no `.bak` sibling was created");
}

#[test]
fn a_boot_reset_survives_the_boot_scene_load() {
    let dir = fresh_dir("boot-reset");
    write(&dir, "save/profile.yaml", "this: is: not: valid: profile\n");
    write(&dir, "main.yaml", EMPTY_SCENE);

    // The `ludion::run` path: build (which backs the save up) then the boot `scene.load`.
    let engine = Engine::builder()
        .profile::<SaveV1>()
        .persist_profile()
        .load_scene_file(&dir.join("main.yaml"))
        .expect("the corrupt profile never fails the boot");

    let entries = &engine.profile_migration().entries;
    assert_eq!(entries.len(), 1, "{entries:?}");
    let ProfileMigrationEntry::Reset { backup, .. } = &entries[0] else {
        panic!("expected a Reset entry, got {:?}", entries[0]);
    };
    assert_eq!(entries[0].code(), Some("L1121"));
    assert!(!backup.is_empty(), "the reset names the backup it made");
    assert!(dir.join("save").join(backup).exists());
}

#[test]
fn a_later_scene_load_rereads_the_save() {
    let dir = fresh_dir("later-load");
    write(&dir, "save/profile.yaml", UNLOCKED_V1);

    let mut engine = Engine::builder()
        .profile::<SaveV1>()
        .project_root(&dir)
        .build()
        .expect("boots");
    engine.load_inline_scene(EMPTY_SCENE).expect("boot load");

    write(
        &dir,
        "save/profile.yaml",
        "format: 1\nkind: profile\nunlocked_crossbow: true\nruns: 99\n",
    );
    engine.load_inline_scene(EMPTY_SCENE).expect("later load");

    assert_eq!(
        engine
            .world()
            .resource::<Profile<SaveV1>>()
            .unwrap()
            .get()
            .runs,
        99,
        "only the boot load carries the build's profile; a later load reads disk"
    );
}

/// A system that projects the profile's `runs` into a component so the profile-at-start reaches the
/// `WorldHash`. `Progress` is a game component; the profile itself never enters the hash directly.
#[derive(Serialize, Deserialize, JsonSchema, Clone, Copy, Default)]
struct Progress {
    runs: u32,
}

fn project_profile(world: &mut ludion_ecs::World, _ctx: &ludion_ecs::SystemCtx) {
    // Read the resource out first (ending its borrow) before mutating components.
    let runs = world
        .resource::<Profile<SaveV1>>()
        .map(|p| p.get().runs)
        .unwrap_or(0);
    for (progress,) in world.query::<(&mut Progress,)>() {
        progress.runs = runs;
    }
}

fn progress_game(root: &Path) -> EngineBuilder {
    Engine::builder()
        .components(|reg| reg.register::<Progress>("Progress"))
        .system("project_profile", project_profile)
        .profile::<SaveV1>()
        .project_root(root)
}

const PROGRESS_SCENE: &str =
    "format: 1\nentities:\n  - components:\n      Progress:\n        runs: 0\n";

#[test]
fn two_boots_from_the_same_file_are_hash_identical() {
    let dir = fresh_dir("determinism");
    write(&dir, "save/profile.yaml", UNLOCKED_V1);

    let mut a = progress_game(&dir).build().expect("boots");
    a.load_inline_scene(PROGRESS_SCENE).unwrap();
    let mut b = progress_game(&dir).build().expect("boots");
    b.load_inline_scene(PROGRESS_SCENE).unwrap();

    // Run a tick on each so the projecting system copies the profile into the component.
    a.advance_live();
    b.advance_live();

    assert_eq!(
        a.world_hash().unwrap(),
        b.world_hash().unwrap(),
        "two boots from the same profile file produce an identical WorldHash"
    );
}

#[test]
fn the_profile_at_start_drives_the_hash() {
    let unlocked_dir = fresh_dir("hash-unlocked");
    write(&unlocked_dir, "save/profile.yaml", UNLOCKED_V1);
    let default_dir = fresh_dir("hash-default");
    // No profile file → default (runs = 0).

    let mut unlocked = progress_game(&unlocked_dir).build().expect("boots");
    unlocked.load_inline_scene(PROGRESS_SCENE).unwrap();
    unlocked.advance_live();

    let mut default = progress_game(&default_dir).build().expect("boots");
    default.load_inline_scene(PROGRESS_SCENE).unwrap();
    default.advance_live();

    assert_ne!(
        unlocked.world_hash().unwrap(),
        default.world_hash().unwrap(),
        "a run from a profile with runs=7 diverges from a default-profile run (runs=0)"
    );
}

#[test]
fn a_game_with_both_a_doc_kind_and_a_profile_boots_clean() {
    // The skip-kind proof: a save/profile.yaml under the content root must not fail the data-doc walk
    // that a registered doc kind triggers (it would, as an unregistered `kind: profile`, without the
    // `is_engine_kind` skip). Boots clean, loads both.
    #[derive(Serialize, Deserialize, JsonSchema, Clone, Debug, PartialEq)]
    struct Weapon {
        damage: f32,
    }

    let dir = fresh_dir("doc-plus-profile");
    write(
        &dir,
        "weapons/crossbow.yaml",
        "format: 1\nkind: weapon\ndamage: 12.0\n",
    );
    write(&dir, "save/profile.yaml", UNLOCKED_V1);

    let engine = Engine::builder()
        .doc_kind::<Weapon>("weapon")
        .profile::<SaveV1>()
        .project_root(&dir)
        .build()
        .expect("a game registering both a doc kind and a profile boots clean");

    // The doc walk skipped save/profile.yaml (no L1119); the profile loaded via its own loader.
    assert_eq!(
        engine
            .world()
            .resource::<ludion_engine::DocStore<Weapon>>()
            .unwrap()
            .len(),
        1
    );
    assert_eq!(
        engine
            .world()
            .resource::<Profile<SaveV1>>()
            .unwrap()
            .get()
            .runs,
        7
    );
    assert!(engine.profile_migration().is_clean());
}

#[test]
fn no_profile_registered_installs_no_resource() {
    let dir = fresh_dir("no-profile");
    // A save file present but never registered — the loader never runs, byte-identical to before.
    write(&dir, "save/profile.yaml", UNLOCKED_V1);

    let engine = Engine::builder()
        .project_root(&dir)
        .build()
        .expect("a profile-free game ignores the save file");
    assert!(
        engine.world().resource::<Profile<SaveV1>>().is_none(),
        "no profile registered means no resource and no IO"
    );
    assert!(engine.profile_migration().is_clean());
}

// --- Wave 3: save drain + atomic write + persistence gate + save log --------------------------

const EMPTY_SCENE: &str = "format: 1\nentities: []\n";

/// A system that requests a save only on frame 0, mutating the profile first, and calls `save()`
/// twice — to prove several requests within a frame collapse into one save log entry.
fn save_on_frame_zero(world: &mut ludion_ecs::World, ctx: &ludion_ecs::SystemCtx) {
    if ctx.frame.as_u64() == 0
        && let Some(profile) = world.resource_mut::<Profile<SaveV1>>()
    {
        profile.get_mut().runs += 1;
        profile.save();
        profile.save();
    }
}

/// A system that writes a fixed profile value and requests a save every tick — for the deterministic,
/// byte-idempotent persistence tests (the same value each frame produces identical canonical bytes).
fn save_fixed(world: &mut ludion_ecs::World, _ctx: &ludion_ecs::SystemCtx) {
    if let Some(profile) = world.resource_mut::<Profile<SaveV1>>() {
        let save = profile.get_mut();
        save.unlocked_crossbow = true;
        save.runs = 42;
        profile.save();
    }
}

#[test]
fn a_save_request_produces_one_stamped_entry_and_quiet_frames_produce_none() {
    let dir = fresh_dir("save-log");
    let mut engine = Engine::builder()
        .profile::<SaveV1>()
        .system("save_on_frame_zero", save_on_frame_zero)
        .project_root(&dir)
        .build()
        .expect("boots");
    engine.load_inline_scene(EMPTY_SCENE).unwrap();

    for _ in 0..3 {
        engine.advance_live();
    }

    let log = engine.profile_save_log();
    assert_eq!(
        log.len(),
        1,
        "two save() calls in frame 0 collapse to one entry"
    );
    assert_eq!(
        log[0].frame.as_u64(),
        0,
        "the entry is stamped with the requesting frame"
    );
}

#[test]
fn persistence_enabled_writes_canonical_idempotent_bytes_that_round_trip() {
    let dir = fresh_dir("persist-write");
    let mut engine = Engine::builder()
        .profile::<SaveV1>()
        .system("save_fixed", save_fixed)
        .project_root(&dir)
        .persist_profile()
        .build()
        .expect("boots");
    engine.load_inline_scene(EMPTY_SCENE).unwrap();

    engine.advance_live();
    let path = dir.join("save/profile.yaml");
    let first = std::fs::read_to_string(&path).expect("persistence wrote the file");
    assert_eq!(
        first, "format: 1\nkind: profile\nunlocked_crossbow: true\nruns: 42\n",
        "the file holds canonical profile bytes (envelope + schema-order fields)"
    );

    // Writing the same value again produces byte-identical output (emit is deterministic).
    engine.advance_live();
    let second = std::fs::read_to_string(&path).unwrap();
    assert_eq!(first, second, "an unchanged profile writes identical bytes");
    assert_eq!(engine.profile_save_log().len(), 2);
    assert!(engine.profile_save_error().is_none());

    // A fresh boot reads the written file back to the same value (emit_doc -> open_profile round-trip).
    let engine2 = Engine::builder()
        .profile::<SaveV1>()
        .project_root(&dir)
        .build()
        .expect("boots from the written profile");
    assert_eq!(
        engine2.world().resource::<Profile<SaveV1>>().unwrap().get(),
        &SaveV1 {
            unlocked_crossbow: true,
            runs: 42,
        },
        "the round-tripped profile equals what was written"
    );
    assert!(engine2.profile_migration().is_clean());
}

#[test]
fn persistence_disabled_fills_the_log_but_writes_nothing() {
    let dir = fresh_dir("persist-off");
    let mut engine = Engine::builder()
        .profile::<SaveV1>()
        .system("save_fixed", save_fixed)
        .project_root(&dir)
        .build()
        .expect("boots");
    // Persistence NOT enabled (the default) — the replay/CI/test posture.
    engine.load_inline_scene(EMPTY_SCENE).unwrap();

    engine.advance_live();
    engine.advance_live();

    assert_eq!(
        engine.profile_save_log().len(),
        2,
        "the log fills regardless of persistence"
    );
    assert!(
        !dir.join("save").exists(),
        "with persistence off, not even the save/ directory is created"
    );
    assert!(engine.profile_save_error().is_none());
}

#[test]
fn a_save_io_failure_surfaces_l1122_without_stopping_the_sim() {
    let dir = fresh_dir("save-io-fail");
    let mut engine = Engine::builder()
        .profile::<SaveV1>()
        .profile_path("blocked/profile.yaml")
        .system("save_fixed", save_fixed)
        .project_root(&dir)
        .persist_profile()
        .build()
        .expect("boots");
    engine.load_inline_scene(EMPTY_SCENE).unwrap();

    // Make the profile's parent an existing FILE, so create_dir_all (and the write) fail cross-platform.
    std::fs::write(dir.join("blocked"), "not a directory").unwrap();

    let ticked = engine.advance_live();
    assert!(ticked, "the tick still runs despite the save failure");
    let error = engine
        .profile_save_error()
        .expect("a save IO failure is surfaced, not swallowed");
    assert_eq!(error.code.as_str(), "L1122");
    assert_eq!(
        engine.profile_save_log().len(),
        1,
        "the request is still logged; only the disk write failed"
    );

    // The sim keeps ticking past the failure without panicking.
    engine.advance_live();
    assert_eq!(engine.frame().as_u64(), 2);
    assert_eq!(engine.profile_save_log().len(), 2);
}

#[test]
fn two_identical_runs_produce_identical_save_streams() {
    let dir = fresh_dir("save-determinism");
    let build = || {
        let mut engine = Engine::builder()
            .profile::<SaveV1>()
            .system("save_on_frame_zero", save_on_frame_zero)
            .project_root(&dir)
            .build()
            .expect("boots");
        engine.load_inline_scene(EMPTY_SCENE).unwrap();
        for _ in 0..5 {
            engine.advance_live();
        }
        engine
    };
    let a = build();
    let b = build();
    assert_eq!(
        a.profile_save_log(),
        b.profile_save_log(),
        "two identical runs regenerate the identical save-request stream"
    );
    assert_eq!(a.profile_save_log().len(), 1);
}

/// A profile type whose own `format` field collides with the envelope.
#[derive(Serialize, Deserialize, JsonSchema, Default, Clone, Debug, PartialEq)]
struct SaveFormatField {
    format: u32,
}

#[test]
fn a_profile_type_with_a_top_level_format_field_is_refused_at_build() {
    let dir = fresh_dir("format_field");
    let Err(diagnostic) = Engine::builder()
        .profile::<SaveFormatField>()
        .project_root(&dir)
        .build()
    else {
        panic!("expected the build to fail");
    };
    assert_eq!(diagnostic.code.as_str(), "L1257");
    assert!(
        diagnostic.message.contains("`format`"),
        "{}",
        diagnostic.message
    );
}
