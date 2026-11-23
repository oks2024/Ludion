//! The player's persisted input rebinds: `persist_input_bindings()` reads `save/input-bindings.yaml`
//! leniently at build and writes it on `Actions::save()`; without the switch the file is never
//! touched. A recording carries the overrides at start in its header (`save_docs`), a replay
//! installs them from there and never reads or writes the file, and a restore reverts to the
//! at-start overrides.

use ludion_ecs::{SystemCtx, World};
use ludion_engine::{Engine, EngineBuilder, ResumeOutcome, RunSave, replay_hashes_with};
use ludion_protocol::command::{EngineRestoreParams, InputInjectParams};
use ludion_protocol::replay::ReplayReader;
use ludion_protocol::{Command, ResponseBody};
use ludion_sim::rebind::{CaptureOptions, CaptureResult, CaptureSlot, Resolve};
use ludion_sim::{ActionName, Actions, Key};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::path::{Path, PathBuf};

const SCAN: ActionName = ActionName::new("scan");
const FILE: &str = "save/input-bindings.yaml";

const MAP: &str = "\
format: 1
kind: input-actions
actions:
  scan:
    kind: button
    bindings:
      - key: M
      - pad: right_shoulder
  jump:
    kind: button
    bindings:
      - key: Space
";

const SCENE: &str = "\
format: 1
seed: 1
entities:
  - name: hero
    components:
      Scans:
        count: 0
";

/// Counts the ticks `scan` was pressed, so the rebinding is visible to the `WorldHash`.
#[derive(Serialize, Deserialize, JsonSchema, Clone, Copy, Debug, PartialEq, Default)]
struct Scans {
    count: u32,
}

fn count_scans(world: &mut World, _ctx: &SystemCtx) {
    let pressed = world
        .resource::<Actions>()
        .is_some_and(|actions| actions.pressed(SCAN));
    if pressed {
        for (scans,) in world.query::<(&mut Scans,)>() {
            scans.count += 1;
        }
    }
}

/// The player's controls screen, scripted: capture a keyboard binding for `scan` at frame 0, save
/// once the capture is bound.
fn rebinder(world: &mut World, ctx: &SystemCtx) {
    let Some(actions) = world.resource_mut::<Actions>() else {
        return;
    };
    if ctx.frame.as_u64() == 0 {
        actions
            .capture("scan", CaptureSlot::Keyboard, CaptureOptions::default())
            .unwrap();
    }
    if ctx.frame.as_u64() == 6 && *actions.capture_result() == CaptureResult::Bound {
        actions.save();
    }
}

fn fresh_dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "ludion-input-bindings-{}-{name}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("content")).unwrap();
    dir
}

fn write_file(root: &Path, text: &str) {
    let path = root.join(FILE);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, text).unwrap();
}

fn file_text(root: &Path) -> String {
    std::fs::read_to_string(root.join(FILE)).unwrap()
}

fn overrides_file(action_lines: &str) -> String {
    format!("format: 1\nkind: input-bindings\nactions:\n{action_lines}")
}

fn builder(root: &Path, persist: bool, with_rebinder: bool) -> EngineBuilder {
    let mut builder = Engine::builder()
        .components(|reg| reg.register::<Scans>("Scans"))
        .system("count_scans", count_scans)
        .input_actions()
        .doc_text("input/actions.yaml", MAP)
        .project_root(root.join("content"))
        .save_root(root);
    if with_rebinder {
        builder = builder.system("rebinder", rebinder);
    }
    if persist {
        builder = builder.persist_input_bindings();
    }
    builder
}

fn booted(builder: EngineBuilder) -> Engine {
    let mut engine = builder.build().expect("boots");
    engine.load_inline_scene(SCENE).unwrap();
    engine
}

fn inject(engine: &mut Engine, frame: Option<u64>, event: Value) {
    engine
        .execute(Command::InputInject(InputInjectParams {
            frame: frame.map(ludion_core::time::FrameIndex::new),
            event,
        }))
        .unwrap();
}

fn tap(engine: &mut Engine, key: &str) {
    inject(engine, None, json!({ "kind": "key_down", "key": key }));
    engine.advance_live();
    inject(engine, None, json!({ "kind": "key_up", "key": key }));
    engine.advance_live();
}

fn scans(engine: &Engine) -> u32 {
    let world = engine.world();
    let hero = world
        .entities()
        .find(|&id| matches!(world.get::<Scans>(id), Ok(Some(_))))
        .expect("one hero");
    world.get::<Scans>(hero).unwrap().unwrap().count
}

fn codes(engine: &Engine) -> Vec<&str> {
    engine
        .boot_warnings()
        .iter()
        .map(|d| d.code.as_str())
        .collect()
}

fn backups(root: &Path) -> usize {
    std::fs::read_dir(root.join("save"))
        .map(|entries| {
            entries
                .flatten()
                .filter(|e| {
                    e.file_name()
                        .to_string_lossy()
                        .starts_with("input-bindings.yaml.bak.")
                })
                .count()
        })
        .unwrap_or(0)
}

#[test]
fn a_captured_rebind_saves_and_a_rebuilt_engine_loads_it() {
    let root = fresh_dir("round-trip");
    let mut engine = booted(builder(&root, true, true));
    for _ in 0..2 {
        engine.advance_live();
    }
    inject(&mut engine, None, json!({ "kind": "key_down", "key": "F" }));
    for _ in 0..3 {
        engine.advance_live();
    }
    inject(&mut engine, None, json!({ "kind": "key_up", "key": "F" }));
    for _ in 0..6 {
        engine.advance_live();
    }
    assert!(engine.input_bindings_save_error().is_none());
    let text = file_text(&root);
    assert!(
        text.starts_with("format: 1\nkind: input-bindings\n"),
        "{text}"
    );
    assert!(text.contains("scan:"), "{text}");
    assert!(!text.contains("jump"), "only the overridden action: {text}");

    let mut rebuilt = booted(builder(&root, true, false));
    assert!(rebuilt.boot_warnings().is_empty());
    tap(&mut rebuilt, "M");
    assert_eq!(scans(&rebuilt), 0, "the old key no longer scans");
    tap(&mut rebuilt, "F");
    assert_eq!(scans(&rebuilt), 1, "the saved key scans");
}

#[test]
fn a_swapped_conflict_saves_both_actions_and_reloads_them() {
    let root = fresh_dir("swap");
    let mut engine = booted(builder(&root, true, false));
    engine
        .world_mut()
        .resource_mut::<Actions>()
        .unwrap()
        .capture("scan", CaptureSlot::Keyboard, CaptureOptions::default())
        .unwrap();
    // `Space` is `jump`'s key, so binding it to `scan` conflicts.
    inject(
        &mut engine,
        None,
        json!({ "kind": "key_down", "key": "Space" }),
    );
    engine.advance_live();
    let actions = engine.world_mut().resource_mut::<Actions>().unwrap();
    assert!(
        matches!(actions.capture_result(), CaptureResult::Conflict { .. }),
        "{:?}",
        actions.capture_result()
    );
    actions.resolve_conflict(Resolve::Swap).unwrap();
    actions.save();
    engine.advance_live();
    assert!(engine.input_bindings_save_error().is_none());

    let rebuilt = booted(builder(&root, true, false));
    assert!(rebuilt.boot_warnings().is_empty());
    let actions = rebuilt.world().resource::<Actions>().unwrap();
    assert_eq!(actions.bindings("scan").unwrap()[0].key, Some(Key::Space));
    assert_eq!(
        actions.bindings("jump").unwrap()[0].key,
        Some(Key::M),
        "the other action took the replaced key"
    );
}

#[test]
fn a_lenient_boot_keeps_what_it_can() {
    let root = fresh_dir("lenient");
    write_file(
        &root,
        &overrides_file(
            "  scan:\n    - key: F\n  ghost:\n    - key: G\n  jump:\n    - key: Space\n      mouse: left\n",
        ),
    );
    let mut engine = booted(builder(&root, true, false));
    let mut warned = codes(&engine);
    warned.sort_unstable();
    assert_eq!(warned, ["L4273", "L4274"]);
    tap(&mut engine, "F");
    assert_eq!(scans(&engine), 1, "the good override applied");
    let actions = engine.world().resource::<Actions>().unwrap();
    assert_eq!(
        actions.bindings("jump").unwrap()[0].key,
        Some(Key::Space),
        "the bad entry left its action at the default"
    );
}

#[test]
fn a_malformed_file_is_backed_up_and_the_engine_boots_with_defaults() {
    let root = fresh_dir("malformed");
    write_file(&root, "format: 1\nkind: input-bindings\n  actions: [\n");
    let mut engine = booted(builder(&root, true, false));
    assert_eq!(codes(&engine), ["L4277"]);
    assert_eq!(backups(&root), 1);
    assert!(!root.join(FILE).exists());
    tap(&mut engine, "M");
    assert_eq!(scans(&engine), 1, "the defaults apply");
}

#[test]
fn without_the_switch_the_file_is_neither_read_nor_written() {
    let root = fresh_dir("off");
    let text = overrides_file("  scan:\n    - key: F\n");
    write_file(&root, &text);
    let mut engine = booted(builder(&root, false, true));
    assert!(engine.boot_warnings().is_empty());
    for _ in 0..8 {
        engine.advance_live();
    }
    tap(&mut engine, "M");
    assert_eq!(
        scans(&engine),
        1,
        "the defaults apply: the file was not read"
    );
    engine.world_mut().resource_mut::<Actions>().unwrap().save();
    engine.advance_live();
    assert_eq!(file_text(&root), text, "the file was not written");
    assert_eq!(backups(&root), 0);
}

fn record(engine: &mut Engine, path: &Path) {
    engine
        .set_recording(std::fs::File::create(path).unwrap())
        .unwrap();
    for _ in 0..3 {
        engine.advance_live();
    }
    for key in ["F", "M", "F"] {
        tap(engine, key);
    }
    engine.stop_recording().unwrap();
}

fn trace_of(engine: &mut Engine, ticks: usize) -> Vec<u64> {
    (0..ticks)
        .map(|_| {
            engine.advance_live();
            engine.world_hash().unwrap().as_u64()
        })
        .collect()
}

#[test]
fn the_header_wins_over_the_replaying_machines_file() {
    let a = fresh_dir("header-a");
    let b = fresh_dir("header-b");
    write_file(&a, &overrides_file("  scan:\n    - key: F\n"));
    let b_text = "format: 1\nkind: input-bindings\n  actions: [\n";
    write_file(&b, b_text);
    let path = a.join("session.lrp");

    let mut engine = booted(builder(&a, true, false));
    record(&mut engine, &path);
    let live_scans = scans(&engine);
    assert_eq!(live_scans, 2, "two F taps scan, the M tap does not");
    let bytes = std::fs::read(&path).unwrap();
    let header = ReplayReader::new(&bytes).unwrap().header().clone();
    let carried = header.save_docs.get("input-bindings").expect("carried");
    assert!(carried.contains("key: F"), "{carried}");

    let replayed = ludion_engine::replay_with(
        builder(&b, true, false),
        &bytes,
        b.join("content"),
        |_, _| Ok(std::ops::ControlFlow::Continue(())),
    )
    .unwrap();
    assert_eq!(scans(&replayed), live_scans, "the replay used the header");
    assert_eq!(file_text(&b), b_text, "root B's file bytes are unchanged");
    assert_eq!(backups(&b), 0, "a replay never backs the file up either");

    let live = replay_hashes_with(builder(&a, true, false), &bytes, a.join("content")).unwrap();
    let elsewhere =
        replay_hashes_with(builder(&b, true, false), &bytes, b.join("content")).unwrap();
    assert_eq!(live, elsewhere, "the hash trace is identical");
    let empty = fresh_dir("header-c");
    let bare =
        replay_hashes_with(builder(&empty, true, false), &bytes, empty.join("content")).unwrap();
    assert_eq!(live, bare, "no file at all reproduces too");
    assert!(!empty.join(FILE).exists(), "and none is created");
}

#[test]
fn a_recording_without_overrides_has_no_save_docs() {
    let root = fresh_dir("no-overrides");
    let path = root.join("session.lrp");
    let mut engine = booted(builder(&root, true, false));
    record(&mut engine, &path);
    let bytes = std::fs::read(&path).unwrap();
    let header = ReplayReader::new(&bytes).unwrap().header().clone();
    assert!(header.save_docs.is_empty());
    assert!(
        !serde_json::to_string(&header)
            .unwrap()
            .contains("save_docs")
    );
}

fn scan_key(engine: &Engine) -> Option<Key> {
    engine
        .world()
        .resource::<Actions>()
        .unwrap()
        .bindings("scan")
        .unwrap()[0]
        .key
}

fn snapshot_of(engine: &mut Engine) -> Value {
    match engine.execute(Command::EngineSnapshot).unwrap().body {
        ResponseBody::Snapshot { snapshot } => snapshot,
        other => panic!("expected a snapshot, got {other:?}"),
    }
}

#[test]
fn a_restore_keeps_the_mid_session_overrides() {
    let root = fresh_dir("restore");
    write_file(&root, &overrides_file("  scan:\n    - key: F\n"));
    let mut engine = booted(builder(&root, true, false));
    engine.advance_live();
    assert!(
        snapshot_of(&mut engine).get("input_bindings").is_none(),
        "the at-start overrides are not carried"
    );
    engine
        .world_mut()
        .resource_mut::<Actions>()
        .unwrap()
        .reset("scan")
        .unwrap();
    tap(&mut engine, "M");
    assert_eq!(scans(&engine), 1, "mid-session the player is back on M");

    let snapshot = snapshot_of(&mut engine);
    assert!(snapshot.get("input_bindings").is_some());
    engine
        .execute(Command::EngineRestore(EngineRestoreParams { snapshot }))
        .unwrap();
    assert_eq!(scan_key(&engine), Some(Key::M), "the rebind survives");
    tap(&mut engine, "M");
    assert_eq!(scans(&engine), 2, "and M still scans");
}

#[test]
fn a_restore_of_an_older_snapshot_returns_to_the_at_start_overrides() {
    let root = fresh_dir("restore-at-start");
    write_file(&root, &overrides_file("  scan:\n    - key: F\n"));
    let mut engine = booted(builder(&root, true, false));
    engine.advance_live();
    let snapshot = snapshot_of(&mut engine);
    engine
        .world_mut()
        .resource_mut::<Actions>()
        .unwrap()
        .reset("scan")
        .unwrap();
    assert_eq!(scan_key(&engine), Some(Key::M));
    engine
        .execute(Command::EngineRestore(EngineRestoreParams { snapshot }))
        .unwrap();
    assert_eq!(
        scan_key(&engine),
        Some(Key::F),
        "the snapshot had no override text"
    );
    let _ = trace_of(&mut engine, 1);
}

#[derive(Serialize, Deserialize, JsonSchema, Default, Clone, Debug, PartialEq)]
struct Save {
    runs: u32,
}

#[test]
fn a_run_save_resume_keeps_the_machines_overrides() {
    let root = fresh_dir("resume");
    write_file(&root, &overrides_file("  scan:\n    - key: F\n"));
    let game = |root: &Path| {
        builder(root, true, false)
            .profile::<Save>()
            .persist_profile()
            .run_save()
    };
    let mut a = booted(game(&root));
    a.advance_live();
    a.world_mut()
        .resource_mut::<Actions>()
        .unwrap()
        .reset("scan")
        .unwrap();
    a.world_mut().resource_mut::<RunSave>().unwrap().suspend();
    a.advance_live();
    assert!(
        root.join("save/suspend.json").exists(),
        "the run was suspended"
    );

    // The machine's own file says F; the suspended run was on M.
    let mut b = booted(game(&root));
    for _ in 0..2 {
        b.advance_live();
    }
    b.world_mut().resource_mut::<RunSave>().unwrap().resume();
    b.advance_live();
    assert_eq!(
        b.world()
            .resource::<RunSave>()
            .unwrap()
            .last_resume()
            .cloned(),
        Some(ResumeOutcome::Resumed {
            frame: ludion_core::time::FrameIndex::new(2)
        })
    );
    assert_eq!(scan_key(&b), Some(Key::F), "the machine's overrides stay");
}

#[test]
fn a_recording_armed_after_a_rebind_carries_the_effective_overrides() {
    let a = fresh_dir("armed-a");
    let b = fresh_dir("armed-b");
    let path = a.join("session.lrp");
    let mut engine = booted(builder(&a, true, false));
    // No file at boot: the player rebinds `scan` to F before the recording is armed (arming is
    // only possible at frame 0), and nothing is saved.
    engine
        .world_mut()
        .resource_mut::<Actions>()
        .unwrap()
        .rebind(
            "scan",
            0,
            ludion_sim::actions::BindingDoc {
                key: Some(Key::F),
                ..Default::default()
            },
        )
        .unwrap();
    assert!(!a.join(FILE).exists());
    record(&mut engine, &path);
    let live_scans = scans(&engine);
    let bytes = std::fs::read(&path).unwrap();
    let header = ReplayReader::new(&bytes).unwrap().header().clone();
    let carried = header.save_docs.get("input-bindings").expect("carried");
    assert!(carried.contains("key: F"), "{carried}");

    let replayed = ludion_engine::replay_with(
        builder(&b, true, false),
        &bytes,
        b.join("content"),
        |_, _| Ok(std::ops::ControlFlow::Continue(())),
    )
    .unwrap();
    assert_eq!(scans(&replayed), live_scans, "the replay used the header");
    assert!(!b.join(FILE).exists(), "a replay never writes the file");
}
