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

// --- resume -------------------------------------------------------------------------------------

use ludion_core::rng::DeterministicRng;
use ludion_engine::ResumeOutcome;
use ludion_protocol::command::InputInjectParams;
use ludion_sim::{InputState, Key};
use serde_json::json;

/// A component whose value depends on the RNG and on input edges, so a wrong resume diverges.
#[derive(Serialize, Deserialize, JsonSchema, Clone, Debug, PartialEq)]
struct Tally {
    sum: u64,
    edges: u32,
    held: u32,
}

const TALLY_HUB: &str = "format: 1\nname: hub\nseed: 5\nentities:\n  - name: t\n    components:\n      Tally:\n        sum: 0\n        edges: 0\n        held: 0\n";

/// Frame at which the suspending run requests the save; the snapshot lands at the next frame.
const SUSPEND_FRAME: u64 = 5;

fn tally(world: &mut World, _ctx: &SystemCtx) {
    let roll = world
        .resource_mut::<DeterministicRng>()
        .map(DeterministicRng::next_u32)
        .unwrap_or(0);
    let (held, edge) = {
        let input = world.resource::<InputState>().unwrap();
        (input.is_held(Key::W), input.was_pressed(Key::W))
    };
    let ids: Vec<_> = world.entities().collect();
    for id in ids {
        if let Ok(Some(t)) = world.get_mut::<Tally>(id) {
            t.sum = t.sum.wrapping_add(u64::from(roll));
            t.edges += u32::from(edge);
            t.held += u32::from(held);
        }
    }
}

fn suspend_at_five(world: &mut World, ctx: &SystemCtx) {
    if ctx.frame.as_u64() == SUSPEND_FRAME
        && let Some(run) = world.resource_mut::<RunSave>()
    {
        run.suspend();
    }
}

fn tally_game(dir: &Path, persist: bool) -> EngineBuilder {
    game(dir, persist)
        .components(|registry| {
            registry.register::<Tally>("Tally")?;
            Ok(())
        })
        .system("tally", tally)
}

fn boot_tally(builder: EngineBuilder) -> Engine {
    let mut engine = builder.build().expect("boots");
    engine.load_inline_scene(TALLY_HUB).unwrap();
    engine
}

fn inject(engine: &mut Engine, frame: Option<u64>, kind: &str) {
    engine
        .execute(Command::InputInject(InputInjectParams {
            frame: frame.map(FrameIndex::new),
            event: json!({ "kind": kind, "key": "W" }),
        }))
        .unwrap();
}

fn request_resume(engine: &mut Engine) {
    engine
        .world_mut()
        .resource_mut::<RunSave>()
        .unwrap()
        .resume();
}

fn last_resume(engine: &Engine) -> Option<ResumeOutcome> {
    engine
        .world()
        .resource::<RunSave>()
        .unwrap()
        .last_resume()
        .cloned()
}

fn backups(dir: &Path) -> usize {
    std::fs::read_dir(dir.join("save"))
        .unwrap()
        .filter(|entry| {
            entry
                .as_ref()
                .unwrap()
                .file_name()
                .to_string_lossy()
                .starts_with("suspend.json.bak.")
        })
        .count()
}

/// Runs the suspending game to the frame after `SUSPEND_FRAME`; `hold` leaves W down across it.
fn suspended_run(dir: &Path, hold: bool) -> Engine {
    let mut a = boot_tally(tally_game(dir, true).system("suspend", suspend_at_five));
    inject(&mut a, Some(1), "key_down");
    if !hold {
        inject(&mut a, Some(3), "key_up");
    }
    for _ in 0..=SUSPEND_FRAME {
        a.advance_live();
    }
    a
}

#[test]
fn a_resumed_run_continues_the_uninterrupted_trace() {
    let dir = fresh_dir("resume-trace");
    let mut a = suspended_run(&dir, false);

    let mut u = boot_tally(tally_game(&fresh_dir("resume-trace-u"), false));
    inject(&mut u, Some(1), "key_down");
    inject(&mut u, Some(3), "key_up");
    for _ in 0..=SUSPEND_FRAME {
        u.advance_live();
    }
    assert_eq!(a.world_hash().unwrap(), u.world_hash().unwrap());

    let mut b = boot_tally(tally_game(&dir, true));
    for _ in 0..3 {
        b.advance_live();
    }
    request_resume(&mut b);
    b.advance_live();
    let n1 = SUSPEND_FRAME + 1;
    assert_eq!(b.frame().as_u64(), n1);
    assert_eq!(b.world_hash().unwrap(), u.world_hash().unwrap());
    assert_eq!(
        last_resume(&b),
        Some(ResumeOutcome::Resumed {
            frame: FrameIndex::new(n1)
        })
    );
    assert_eq!(
        b.run_save_log().back().unwrap().request,
        RunSaveRequest::Resume
    );
    assert!(b.run_save_error().is_none());

    for engine in [&mut b, &mut u] {
        inject(engine, Some(10), "key_down");
        inject(engine, Some(200), "key_up");
    }
    for tick in 0..300 {
        b.advance_live();
        u.advance_live();
        assert_eq!(
            b.world_hash().unwrap(),
            u.world_hash().unwrap(),
            "diverged {tick} ticks after the resume"
        );
    }
}

#[test]
fn one_suspend_gives_one_resume() {
    let dir = fresh_dir("resume-once");
    suspended_run(&dir, false);

    let mut b = boot_tally(tally_game(&dir, true));
    assert!(pending(&b).is_some());
    request_resume(&mut b);
    b.advance_live();
    assert!(!dir.join("save/suspend.json").exists());
    assert!(pending(&b).is_none());

    let c = boot_tally(tally_game(&dir, true));
    assert!(pending(&c).is_none());

    request_resume(&mut b);
    b.advance_live();
    assert!(matches!(
        last_resume(&b),
        Some(ResumeOutcome::Refused { .. })
    ));
    assert_eq!(b.run_save_error().unwrap().code.as_str(), "L1401");
    assert_eq!(backups(&dir), 0, "there was no file to back up");
}

#[test]
fn held_input_is_clear_after_resume() {
    let dir = fresh_dir("resume-input");
    let a = suspended_run(&dir, true);
    assert!(a.world().resource::<InputState>().unwrap().is_held(Key::W));

    let mut b = boot_tally(tally_game(&dir, true));
    request_resume(&mut b);
    b.advance_live();
    let idle = b.world().resource::<InputState>().unwrap();
    assert!(!idle.is_held(Key::W) && !idle.was_pressed(Key::W));

    inject(&mut b, None, "key_down");
    b.advance_live();
    let input = b.world().resource::<InputState>().unwrap();
    assert!(input.is_held(Key::W) && input.was_pressed(Key::W));
    let edges: u32 = b
        .world()
        .entities()
        .filter_map(|id| b.world().get::<Tally>(id).ok().flatten().map(|t| t.edges))
        .sum();
    assert_eq!(
        edges, 2,
        "A's own press, then the fresh one after the resume"
    );
}

/// Boots B with a refusal-causing file already in place and checks every refusal invariant against
/// a twin that never asked to resume.
fn assert_refused(dir: &Path, reason_has: &str, mut b: Engine) {
    let twin_dir = fresh_dir("resume-twin");
    let mut twin = boot_tally(tally_game(&twin_dir, false));
    for _ in 0..3 {
        b.advance_live();
        twin.advance_live();
    }
    request_resume(&mut b);
    b.advance_live();
    twin.advance_live();

    let Some(ResumeOutcome::Refused { reason }) = last_resume(&b) else {
        panic!("expected a refusal, got {:?}", last_resume(&b));
    };
    assert!(reason.contains(reason_has), "{reason}");
    assert_eq!(b.run_save_error().unwrap().code.as_str(), "L1401");
    assert!(!dir.join("save/suspend.json").exists());
    assert_eq!(backups(dir), 1);
    assert!(pending(&b).is_none());
    assert_eq!(b.frame(), twin.frame());
    assert_eq!(b.world_hash().unwrap(), twin.world_hash().unwrap());
}

#[test]
fn a_changed_component_schema_refuses_with_l1401_and_a_backup() {
    #[derive(Serialize, Deserialize, JsonSchema, Clone)]
    struct Gear {
        hp: u32,
    }
    #[derive(Serialize, Deserialize, JsonSchema, Clone)]
    struct WiderGear {
        hp: u32,
        mana: u32,
    }

    let dir = fresh_dir("resume-schema");
    let mut a = boot_tally(
        tally_game(&dir, true)
            .components(|registry| {
                registry.register::<Gear>("Gear")?;
                Ok(())
            })
            .system("suspend", suspend_at_five),
    );
    for _ in 0..=SUSPEND_FRAME {
        a.advance_live();
    }
    assert!(dir.join("save/suspend.json").exists());

    let b = boot_tally(tally_game(&dir, true).components(|registry| {
        registry.register::<WiderGear>("Gear")?;
        Ok(())
    }));
    assert_refused(&dir, "schemas changed", b);
}

#[test]
fn a_garbled_file_is_refused_and_backed_up() {
    let dir = fresh_dir("resume-garbled");
    write(&dir, "save/suspend.json", "{ not json");
    let b = boot_tally(tally_game(&dir, true));
    assert_refused(&dir, "unreadable", b);
}

#[test]
fn a_non_persisting_resume_only_logs() {
    let dir = fresh_dir("resume-log");
    let mut b = boot_tally(tally_game(&dir, false));
    let mut twin = boot_tally(tally_game(&fresh_dir("resume-log-twin"), false));
    for _ in 0..3 {
        b.advance_live();
        twin.advance_live();
    }
    request_resume(&mut b);
    b.advance_live();
    twin.advance_live();
    assert_eq!(
        b.run_save_log().back().unwrap().request,
        RunSaveRequest::Resume
    );
    assert_eq!(b.world_hash().unwrap(), twin.world_hash().unwrap());
    assert_eq!(last_resume(&b), None);
    assert!(!dir.join("save").exists());
}

// --- the host's pacing and the consumed file ------------------------------------------------------

use ludion_protocol::command::SimStepParams;
use ludion_sim::RunState;

fn resume_on_two(world: &mut World, ctx: &SystemCtx) {
    if ctx.frame.as_u64() == 2
        && let Some(run) = world.resource_mut::<RunSave>()
    {
        run.resume();
    }
}

fn sim_step(engine: &mut Engine, ticks: u64) {
    engine
        .execute(Command::SimStep(SimStepParams { ticks, hash: false }))
        .unwrap();
}

#[test]
fn a_resume_inside_sim_step_keeps_the_host_paused() {
    let dir = fresh_dir("resume-paused");
    suspended_run(&dir, false);

    let mut b = boot_tally(tally_game(&dir, true).system("resume", resume_on_two));
    b.execute(Command::SimPause).unwrap();
    sim_step(&mut b, 5);

    // Ticks 0..=2 ran before the resume rewound to the suspended frame; the 2 left ran after it.
    assert_eq!(b.frame().as_u64(), SUSPEND_FRAME + 1 + 2);
    assert_eq!(b.run_state(), RunState::Paused);
    assert!(matches!(
        last_resume(&b),
        Some(ResumeOutcome::Resumed { .. })
    ));
}

#[test]
fn a_suspend_taken_mid_step_resumes_unstepped() {
    let dir = fresh_dir("resume-mid-step");
    let mut a = boot_tally(tally_game(&dir, true).system("suspend", suspend_on_two));
    a.execute(Command::SimPause).unwrap();
    sim_step(&mut a, 6);
    assert!(dir.join("save/suspend.json").exists());
    let stored = read_envelope(&dir);
    assert_eq!(stored["snapshot"]["run_state"]["state"], "stepping");

    let mut b = boot_tally(tally_game(&dir, true));
    b.advance_live();
    request_resume(&mut b);
    b.advance_live();
    assert!(matches!(
        last_resume(&b),
        Some(ResumeOutcome::Resumed { .. })
    ));
    assert_eq!(b.run_state(), RunState::Running);
    let frame = b.frame();
    b.advance_live();
    assert_eq!(b.frame().as_u64(), frame.as_u64() + 1);
}

#[test]
fn a_resume_consumes_the_file_and_a_crashed_resume_is_backed_up_at_boot() {
    let dir = fresh_dir("resume-consumed");
    suspended_run(&dir, false);
    let mut b = boot_tally(tally_game(&dir, true));
    request_resume(&mut b);
    b.advance_live();
    assert!(!dir.join("save/suspend.json").exists());
    assert!(!dir.join("save/suspend.json.resuming").exists());
    assert!(b.run_save_error().is_none());

    // A resume that died between the rename and the delete left its parked file behind.
    write(&dir, "save/suspend.json.resuming", "parked");
    let c = boot_tally(tally_game(&dir, true));
    assert!(pending(&c).is_none());
    assert!(!dir.join("save/suspend.json.resuming").exists());
    let kept = std::fs::read_dir(dir.join("save"))
        .unwrap()
        .filter(|entry| {
            let name = entry.as_ref().unwrap().file_name();
            name.to_string_lossy()
                .starts_with("suspend.json.resuming.bak.")
        })
        .count();
    assert_eq!(kept, 1);
}
