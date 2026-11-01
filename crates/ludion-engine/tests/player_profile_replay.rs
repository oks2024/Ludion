//! Player-profile reproducibility tests (Phase 16 / E5, Wave 4): the profile-at-start rides the
//! replay header and the snapshot keyframe, so a recorded session reproduces its `WorldHash` trace
//! from its own bytes on a machine whose on-disk `save/profile.yaml` differs or is absent, and a
//! `ludion dev`-style snapshot→restore preserves the exact in-memory profile.
//!
//! The profile reaches the hash only through the components a game derives from it, so every test
//! registers a `project_profile` system that copies the profile into a `Progress` component each tick
//! (the doc-store `apply_weapon` mold). A mid-run `profile.save()` exercises the regenerated (never
//! recorded) save path.

use ludion_engine::{Engine, EngineBuilder, Profile, replay_hashes_with};
use ludion_protocol::command::{EngineRestoreParams, SimStepParams};
use ludion_protocol::{Command, ResponseBody};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

/// The game's persistent save: an unlock flag and a run counter.
#[derive(Serialize, Deserialize, JsonSchema, Default, Clone, Debug, PartialEq)]
struct Save {
    unlocked: bool,
    runs: u32,
}

/// The sim-visible projection of the profile — the component the `WorldHash` actually covers.
#[derive(Serialize, Deserialize, JsonSchema, Clone, Copy, Default)]
struct Progress {
    runs: u32,
    unlocked: u32,
}

/// Copies the profile into `Progress` every tick and, on frame 2, bumps `runs` and requests a save
/// (a mid-run `profile.save()`). Both the projection and the mutation are pure sim logic, so a replay
/// regenerates them identically — the save is never recorded, and the profile-at-start (embedded in
/// the header) drives the projected value from the first tick.
fn project_profile(world: &mut ludion_ecs::World, ctx: &ludion_ecs::SystemCtx) {
    if ctx.frame.as_u64() == 2
        && let Some(profile) = world.resource_mut::<Profile<Save>>()
    {
        profile.get_mut().runs += 1;
        profile.save();
    }
    let (runs, unlocked) = world
        .resource::<Profile<Save>>()
        .map(|p| (p.get().runs, p.get().unlocked as u32))
        .unwrap_or((0, 0));
    for (progress,) in world.query::<(&mut Progress,)>() {
        progress.runs = runs;
        progress.unlocked = unlocked;
    }
}

fn game(root: &Path) -> EngineBuilder {
    Engine::builder()
        .components(|reg| reg.register::<Progress>("Progress"))
        .system("project_profile", project_profile)
        .profile::<Save>()
        .project_root(root)
}

const SCENE: &str = "format: 1\nseed: 1\nentities:\n  - components:\n      Progress:\n        runs: 0\n        unlocked: 0\n";

/// A non-default profile-at-start: unlocked, 5 runs.
const UNLOCKED: &str = "format: 1\nkind: profile\nunlocked: true\nruns: 5\n";
/// A *different* on-disk profile a replay must ignore in favor of its embedded one.
const OTHER: &str = "format: 1\nkind: profile\nunlocked: false\nruns: 99\n";

fn fresh_dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "ludion-profile-replay-{}-{name}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn write(root: &Path, rel: &str, contents: &str) {
    let path = root.join(rel);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, contents).unwrap();
}

/// A recording sink whose bytes the test reads back (the `docs_reload.rs` mold).
#[derive(Clone, Default)]
struct SharedSink(Arc<Mutex<Vec<u8>>>);

impl SharedSink {
    fn bytes(&self) -> Vec<u8> {
        self.0.lock().unwrap().clone()
    }
}

impl std::io::Write for SharedSink {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(buf);
        Ok(buf.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

/// The commands a recorded/replayed session drives: six single-tick steps.
fn session() -> Vec<Command> {
    vec![Command::SimStep(SimStepParams {
        ticks: 6,
        hash: false,
    })]
}

/// Records a session in `dir` (booting on the `UNLOCKED` profile), returning `(live_trace, bytes)`.
fn record_unlocked_session(
    dir: &Path,
) -> (
    Vec<(ludion_core::time::FrameIndex, ludion_sim::WorldHash)>,
    Vec<u8>,
) {
    write(dir, "save/profile.yaml", UNLOCKED);
    let sink = SharedSink::default();
    let mut engine = game(dir).build().unwrap();
    engine.load_inline_scene(SCENE).unwrap();
    engine.set_recording(sink.clone()).unwrap();
    let live = ludion_engine::drive_hashes(&mut engine, session()).unwrap();
    engine.stop_recording().unwrap();
    (live, sink.bytes())
}

#[test]
fn a_recorded_session_reproduces_its_trace_twice() {
    let dir = fresh_dir("reproduce");
    let (live, bytes) = record_unlocked_session(&dir);

    let first = replay_hashes_with(game(&dir), &bytes, &dir).unwrap();
    let second = replay_hashes_with(game(&dir), &bytes, &dir).unwrap();
    assert_eq!(live, first, "the replay reproduces the live trace");
    assert_eq!(first, second, "replaying the same bytes twice is identical");
}

#[test]
fn the_embedded_profile_wins_over_a_missing_or_different_disk_save() {
    let dir = fresh_dir("cross-machine");
    let (live, bytes) = record_unlocked_session(&dir);

    // Cross-machine A: the on-disk save is gone entirely. The header's embedded profile-at-start must
    // still drive the trace to the recording's.
    std::fs::remove_file(dir.join("save/profile.yaml")).unwrap();
    let without_disk = replay_hashes_with(game(&dir), &bytes, &dir).unwrap();
    assert_eq!(
        live, without_disk,
        "with no disk profile the embedded profile-at-start reproduces the recording"
    );

    // Cross-machine B: the on-disk save holds *different* values (unlocked:false, runs:99). If the
    // replay re-read disk it would boot on runs=99 and diverge; the embedded runs=5 must win.
    write(&dir, "save/profile.yaml", OTHER);
    let with_other_disk = replay_hashes_with(game(&dir), &bytes, &dir).unwrap();
    assert_eq!(
        live, with_other_disk,
        "a different on-disk profile is ignored — the embedded profile-at-start wins"
    );
}

#[test]
fn an_unlocked_profile_diverges_from_a_default_run() {
    let unlocked_dir = fresh_dir("diverge-unlocked");
    write(&unlocked_dir, "save/profile.yaml", UNLOCKED);
    let default_dir = fresh_dir("diverge-default");
    // No profile file → default (unlocked:false, runs:0).

    let mut unlocked = game(&unlocked_dir).build().unwrap();
    unlocked.load_inline_scene(SCENE).unwrap();
    let unlocked_trace = ludion_engine::drive_hashes(&mut unlocked, session()).unwrap();

    let mut default = game(&default_dir).build().unwrap();
    default.load_inline_scene(SCENE).unwrap();
    let default_trace = ludion_engine::drive_hashes(&mut default, session()).unwrap();

    // Frame 0 (before any tick) is identical (both scenes start Progress at 0); the projecting system
    // then moves the hash apart from the first tick on.
    assert_eq!(unlocked_trace[0].1, default_trace[0].1, "frame 0 matches");
    assert_ne!(
        unlocked_trace[1].1, default_trace[1].1,
        "the profile-at-start moves the hash from the first tick"
    );
    assert_ne!(unlocked_trace, default_trace);
}

/// Snapshots `engine` via the `engine.snapshot` command, returning the raw document value.
fn snapshot(engine: &mut Engine) -> serde_json::Value {
    match engine.execute(Command::EngineSnapshot).unwrap().body {
        ResponseBody::Snapshot { snapshot } => snapshot,
        other => panic!("expected a snapshot body, got {other:?}"),
    }
}

/// Restores `snapshot` into `engine` via the `engine.restore` command.
fn restore(engine: &mut Engine, snapshot: serde_json::Value) {
    engine
        .execute(Command::EngineRestore(EngineRestoreParams { snapshot }))
        .unwrap();
}

#[test]
fn a_snapshot_carries_the_profile_and_restore_installs_it_off_disk() {
    let source_dir = fresh_dir("snap-source");
    write(&source_dir, "save/profile.yaml", UNLOCKED);
    let mut source = game(&source_dir).build().unwrap();
    source.load_inline_scene(SCENE).unwrap();
    // Tick to frame 3 so the mid-run save (frame 2) has fired and `runs` was bumped to 6 in memory —
    // but NOT written to disk (persistence is off in a test), an unsaved in-memory change.
    for _ in 0..3 {
        source.advance_live();
    }
    assert_eq!(
        source.world().resource::<Profile<Save>>().unwrap().get(),
        &Save {
            unlocked: true,
            runs: 6,
        },
        "the in-memory profile carries the unsaved mid-run bump"
    );
    let snap = snapshot(&mut source);

    // Restore into a FRESH engine whose on-disk save differs (runs=99). The restored in-memory
    // profile must equal the snapshot's embedded value (runs=6), not the disk's — a dev-reload
    // continuation preserves unsaved state.
    let restore_dir = fresh_dir("snap-restore");
    write(&restore_dir, "save/profile.yaml", OTHER);
    let mut restored = game(&restore_dir).build().unwrap();
    restored.load_inline_scene(SCENE).unwrap();
    restore(&mut restored, snap);
    assert_eq!(
        restored.world().resource::<Profile<Save>>().unwrap().get(),
        &Save {
            unlocked: true,
            runs: 6,
        },
        "restore installs the snapshot's embedded profile bit-exactly, ignoring the different disk save"
    );
    assert!(restored.profile_migration().is_clean());
}

#[test]
fn a_profile_free_snapshot_has_no_profile_key() {
    let dir = fresh_dir("snap-free");
    // A profile-free game: no `.profile::<P>()` registration.
    let mut engine = Engine::builder()
        .components(|reg| reg.register::<Progress>("Progress"))
        .project_root(&dir)
        .build()
        .unwrap();
    engine.load_inline_scene(SCENE).unwrap();
    let snap = snapshot(&mut engine);
    assert!(
        snap.get("profile").is_none(),
        "a profile-free snapshot must omit the additive `profile` key (byte-identical to before E5): {snap}"
    );
    // A restore of it is still clean (no profile to install, falls back to the disk load-or-default).
    let restored = engine
        .execute(Command::EngineRestore(EngineRestoreParams {
            snapshot: snap,
        }))
        .unwrap()
        .body;
    assert!(matches!(restored, ResponseBody::Restored { report } if report.is_clean()));
}

/// Projects the profile without the mid-run bump, so a profile-free recording and a profile-bearing
/// replay of it trace identically exactly when the replay installs the default profile.
fn project_only(world: &mut ludion_ecs::World, _ctx: &ludion_ecs::SystemCtx) {
    let (runs, unlocked) = world
        .resource::<Profile<Save>>()
        .map(|p| (p.get().runs, p.get().unlocked as u32))
        .unwrap_or((0, 0));
    for (progress,) in world.query::<(&mut Progress,)>() {
        progress.runs = runs;
        progress.unlocked = unlocked;
    }
}

#[test]
fn a_replay_without_a_header_profile_installs_the_default_never_the_disk_save() {
    let dir = fresh_dir("no-header-profile");
    let sink = SharedSink::default();
    // A profile-free recorder: its header carries no profile text.
    let mut recorder = Engine::builder()
        .components(|reg| reg.register::<Progress>("Progress"))
        .system("project_only", project_only)
        .project_root(&dir)
        .build()
        .unwrap();
    recorder.load_inline_scene(SCENE).unwrap();
    recorder.set_recording(sink.clone()).unwrap();
    let live = ludion_engine::drive_hashes(&mut recorder, session()).unwrap();
    recorder.stop_recording().unwrap();

    // The replaying build registers the profile and the developer's save holds non-default values.
    write(&dir, "save/profile.yaml", OTHER);
    let replaying = Engine::builder()
        .components(|reg| reg.register::<Progress>("Progress"))
        .system("project_only", project_only)
        .profile::<Save>()
        .project_root(&dir);
    let replayed = replay_hashes_with(replaying, &sink.bytes(), &dir).unwrap();
    assert_eq!(
        live, replayed,
        "the replay booted on the default, not runs=99"
    );
    assert_eq!(
        std::fs::read_to_string(dir.join("save/profile.yaml")).unwrap(),
        OTHER,
        "the save file is untouched"
    );
}
