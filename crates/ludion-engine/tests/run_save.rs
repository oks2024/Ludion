//! Run-save suspend tests: `EngineBuilder::run_save` installs a `RunSave` resource; a system's
//! `suspend()` is drained at the frame boundary into a frame-stamped log and — only in a persisting
//! host — written crash-durably to `save/suspend.json` as an envelope around the `engine.snapshot`
//! document, stamped with the build.

use ludion_core::time::FrameIndex;
use ludion_ecs::{SystemCtx, World};
use ludion_engine::{
    Engine, EngineBuilder, Profile, RunSave, RunSaveRequest, SceneSwitch, SuspendInfo,
};
use ludion_protocol::{Command, ResponseBody};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::path::{Path, PathBuf};

#[derive(Serialize, Deserialize, JsonSchema, Default, Clone, Debug, PartialEq)]
struct Save {
    runs: u32,
}

/// A scene with a `name:` so the envelope's `scene` is checked.
const HUB: &str = "format: 1\nname: hub\nseed: 5\nentities:\n  - name: a\n    components:\n      Transform:\n        translation:\n          - 1.0\n          - 2.0\n          - 3.0\n";

/// The switch target, with the anchor a switch needs.
const NEXT: &str = "format: 1\nname: arrival\nentities:\n  - name: gate\n    components:\n      Transform:\n        translation:\n          - 4.0\n          - 0.0\n          - 0.0\n";

fn fresh_dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("ludion-runsave-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn write(root: &Path, rel: &str, contents: &str) {
    let path = root.join(rel);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, contents).unwrap();
}

/// Suspends on frame 2 only.
fn suspend_on_two(world: &mut World, ctx: &SystemCtx) {
    if ctx.frame.as_u64() == 2
        && let Some(run) = world.resource_mut::<RunSave>()
    {
        run.suspend();
    }
}

fn suspend_every_frame(world: &mut World, _ctx: &SystemCtx) {
    if let Some(run) = world.resource_mut::<RunSave>() {
        run.suspend();
    }
}

fn discard_then_suspend(world: &mut World, ctx: &SystemCtx) {
    if ctx.frame.as_u64() == 1
        && let Some(run) = world.resource_mut::<RunSave>()
    {
        // Called suspend-first on purpose: the drain order is discard, then suspend.
        run.suspend();
        run.discard();
    }
}

fn save_suspend_and_switch(world: &mut World, ctx: &SystemCtx) {
    if ctx.frame.as_u64() != 1 {
        return;
    }
    if let Some(profile) = world.resource_mut::<Profile<Save>>() {
        profile.get_mut().runs += 1;
        profile.save();
    }
    if let Some(run) = world.resource_mut::<RunSave>() {
        run.suspend();
    }
    if let Some(switch) = world.resource_mut::<SceneSwitch>() {
        switch.request("next.yaml".to_string(), "gate".to_string(), Vec::new());
    }
}

fn game(dir: &Path, persist: bool) -> EngineBuilder {
    let mut builder = Engine::builder()
        .profile::<Save>()
        .run_save()
        .project_root(dir);
    if persist {
        builder = builder.persist_profile();
    }
    builder
}

fn boot(builder: EngineBuilder) -> Engine {
    let mut engine = builder.build().expect("boots");
    engine.load_inline_scene(HUB).unwrap();
    engine
}

fn read_envelope(dir: &Path) -> Value {
    let text = std::fs::read_to_string(dir.join("save/suspend.json")).unwrap();
    serde_json::from_str(&text).unwrap()
}

fn pending(engine: &Engine) -> Option<SuspendInfo> {
    engine
        .world()
        .resource::<RunSave>()
        .unwrap()
        .pending()
        .cloned()
}

fn snapshot_of(engine: &mut Engine) -> Value {
    match engine.execute(Command::EngineSnapshot).unwrap().body {
        ResponseBody::Snapshot { snapshot } => snapshot,
        other => panic!("expected a snapshot, got {other:?}"),
    }
}

#[test]
fn a_suspend_request_is_logged_and_writes_nothing_without_persistence() {
    let dir = fresh_dir("no-persist");
    let mut engine = boot(game(&dir, false).system("suspend", suspend_on_two));
    for _ in 0..4 {
        engine.advance_live();
    }

    let log = engine.run_save_log();
    assert_eq!(log.len(), 1, "{log:?}");
    assert_eq!(log[0].frame, FrameIndex::new(2));
    assert_eq!(log[0].request, RunSaveRequest::Suspend);
    let info = pending(&engine).expect("pending reflects the request");
    assert_eq!(info.frame, FrameIndex::new(2));
    assert_eq!(info.scene.as_deref(), Some("hub"));
    assert!(!dir.join("save").exists(), "no IO without persistence");
    assert!(engine.run_save_error().is_none());
}

#[test]
fn a_persisting_suspend_writes_the_envelope() {
    let dir = fresh_dir("envelope");
    let mut engine = boot(game(&dir, true).system("suspend", suspend_on_two));
    for _ in 0..3 {
        engine.advance_live();
    }
    // Frame 2 has been drained; nothing has run since, so a direct snapshot sees the same world.
    let direct = snapshot_of(&mut engine);

    let file = read_envelope(&dir);
    assert_eq!(file["format"], 1);
    assert_eq!(file["scene"], "hub");
    // The envelope carries the request frame; the snapshot is taken after the counter advanced.
    assert_eq!(file["frame"], 2);
    assert_eq!(file["snapshot"]["frame"], direct["frame"]);
    assert_eq!(file["snapshot"]["scene_name"], "hub");
    assert_eq!(file["snapshot"]["entities"], direct["entities"]);
    assert_eq!(file["snapshot"]["seed"], direct["seed"]);
    assert!(
        file["snapshot"].get("build").is_none(),
        "the stamp stays in the envelope"
    );
    let info = pending(&engine).unwrap();
    assert_eq!(
        file["build"]["engine_version"],
        info.saved_at_build.engine_version
    );
    assert_eq!(
        file["build"]["game_schema"],
        serde_json::to_value(info.saved_at_build.game_schema).unwrap()
    );
    assert!(engine.run_save_error().is_none());
}

#[test]
fn pending_reads_the_file_at_boot_and_discard_deletes_it() {
    let dir = fresh_dir("pending");
    {
        let mut a = boot(game(&dir, true).system("suspend", suspend_on_two));
        for _ in 0..3 {
            a.advance_live();
        }
    }
    let written = read_envelope(&dir);

    // A non-persisting boot sees nothing and leaves the file.
    let c = boot(game(&dir, false));
    assert!(pending(&c).is_none());
    assert!(dir.join("save/suspend.json").exists());

    let mut b = boot(
        game(&dir, true).system("discard", |world: &mut World, ctx: &SystemCtx| {
            if ctx.frame.as_u64() == 0
                && let Some(run) = world.resource_mut::<RunSave>()
            {
                run.discard();
            }
        }),
    );
    let info = pending(&b).expect("B reads A's file");
    assert_eq!(info.frame, FrameIndex::new(2));
    assert_eq!(info.scene.as_deref(), Some("hub"));
    assert_eq!(
        serde_json::to_value(info.saved_at_build.game_schema).unwrap(),
        written["build"]["game_schema"]
    );

    b.advance_live();
    assert!(!dir.join("save/suspend.json").exists());
    assert!(pending(&b).is_none());
    assert_eq!(b.run_save_log()[0].request, RunSaveRequest::Discard);
}

#[test]
fn a_suspend_io_failure_surfaces_l1400_and_keeps_the_previous_file() {
    let dir = fresh_dir("io-fail");
    let mut engine = boot(game(&dir, true).system("suspend", suspend_every_frame));
    engine.advance_live();
    assert!(engine.run_save_error().is_none());
    let before = std::fs::read(dir.join("save/suspend.json")).unwrap();

    // Make the temp-sibling creation fail: `save` becomes a file, so nothing can be staged beside
    // the target. Move the whole directory aside to keep the earlier bytes comparable.
    std::fs::rename(dir.join("save"), dir.join("save-old")).unwrap();
    std::fs::write(dir.join("save"), "not a directory").unwrap();

    assert!(engine.advance_live(), "the sim keeps ticking");
    let error = engine.run_save_error().expect("surfaced");
    assert_eq!(error.code.as_str(), "L1400");
    assert_eq!(
        engine.run_save_log().len(),
        2,
        "the request is still logged"
    );
    assert_eq!(
        std::fs::read(dir.join("save-old/suspend.json")).unwrap(),
        before
    );
    engine.advance_live();
    assert_eq!(engine.frame().as_u64(), 3);
}

fn other_pid() -> u32 {
    std::process::id() + 1
}

#[test]
fn a_persisting_boot_sweeps_foreign_suspend_temps_only() {
    let dir = fresh_dir("sweep-on");
    let foreign = format!("save/.suspend.json.{}.0.tmp", other_pid());
    let own = format!("save/.suspend.json.{}.0.tmp", std::process::id());
    write(&dir, &foreign, "debris");
    write(&dir, &own, "own");
    boot(game(&dir, true));
    assert!(!dir.join(&foreign).exists());
    assert!(dir.join(&own).exists());
}

#[test]
fn a_non_persisting_boot_leaves_suspend_temps_alone() {
    let dir = fresh_dir("sweep-off");
    let foreign = format!("save/.suspend.json.{}.0.tmp", other_pid());
    write(&dir, &foreign, "debris");
    boot(game(&dir, false));
    assert!(dir.join(&foreign).exists());
}

#[test]
fn save_suspend_and_switch_in_one_frame_all_land() {
    let dir = fresh_dir("switch");
    write(&dir, "next.yaml", NEXT);
    let mut engine = boot(game(&dir, true).system("all", save_suspend_and_switch));
    for _ in 0..4 {
        engine.advance_live();
    }

    assert!(engine.last_transition_error().is_none());
    assert_eq!(engine.profile_save_log().len(), 1);
    assert_eq!(engine.profile_save_log()[0].frame, FrameIndex::new(1));
    assert_eq!(engine.run_save_log().len(), 1);
    assert_eq!(engine.run_save_log()[0].frame, FrameIndex::new(1));

    let file = read_envelope(&dir);
    assert_eq!(
        file["snapshot"]["scene_name"], "hub",
        "the departing scene is what was written"
    );
    assert_eq!(snapshot_of(&mut engine)["scene_name"], "arrival");
    let info = pending(&engine).expect("the arriving world's RunSave reports the new pending");
    assert_eq!(info.scene.as_deref(), Some("hub"));
    assert_eq!(info.frame, FrameIndex::new(1));
}

#[test]
fn discard_then_suspend_in_one_frame_leaves_the_new_file() {
    let dir = fresh_dir("discard-suspend");
    write(
        &dir,
        "save/suspend.json",
        "{\"format\":1,\"build\":{\"engine_version\":\"x\",\"game_schema\":\"0000000000000000\"},\"frame\":99,\"snapshot\":{}}\n",
    );
    let mut engine = boot(game(&dir, true).system("both", discard_then_suspend));
    engine.advance_live();
    engine.advance_live();

    let log: Vec<_> = engine.run_save_log().iter().map(|e| e.request).collect();
    assert_eq!(log, [RunSaveRequest::Discard, RunSaveRequest::Suspend]);
    let file = read_envelope(&dir);
    assert_eq!(file["frame"], 1, "the new run's file, not the old one");
    assert_eq!(pending(&engine).unwrap().frame, FrameIndex::new(1));
}

#[test]
fn a_suspend_does_not_move_the_world_hash() {
    let trace = |suspending: bool| {
        let dir = fresh_dir(if suspending { "hash-on" } else { "hash-off" });
        let mut builder = game(&dir, suspending);
        if suspending {
            builder = builder.system("suspend", suspend_every_frame);
        }
        let mut engine = boot(builder);
        (0..40)
            .map(|_| {
                engine.advance_live();
                engine.world_hash().unwrap().as_u64()
            })
            .collect::<Vec<_>>()
    };
    assert_eq!(trace(false), trace(true));
}
