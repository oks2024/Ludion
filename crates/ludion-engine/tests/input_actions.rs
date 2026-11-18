//! `Engine::builder().input_actions()`: the `kind: input-actions` document becomes an `Actions`
//! resource the game reads. Gestures reach a hashed `Stance` component, a bad document refuses the
//! build under its re-pathed code, an unknown name reports `L4270` once, a restore latches held
//! inputs, a scene switch carries a hold, and the hash is thread-count invariant.

use ludion_core::codes::code;
use ludion_core::par;
use ludion_ecs::{SystemCtx, World};
use ludion_engine::diagnostics::Diagnostics;
use ludion_engine::{Engine, EngineBuilder, SceneSwitch};
use ludion_protocol::command::{EngineRestoreParams, InputInjectParams};
use ludion_protocol::{Command, ResponseBody};
use ludion_sim::{ActionName, Actions};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::path::PathBuf;

const CROUCH: ActionName = ActionName::new("crouch");
const PRONE: ActionName = ActionName::new("prone");

const STAND: u32 = 0;
const CROUCHING: u32 = 1;
const PRONE_STANCE: u32 = 2;

/// At 60 Hz the default `hold` is 21 ticks: down on tick `d` fires the hold on tick `d + 20`.
const HOLD_TICKS: usize = 20;

const MAP: &str = "\
format: 1
kind: input-actions
actions:
  crouch:
    kind: button
    bindings:
      - key: C
        gesture: tap
      - pad: east
        gesture: tap
  prone:
    kind: button
    bindings:
      - key: Z
        gesture: hold
      - pad: east
        gesture: hold
";

/// What the game's own system makes of the actions: the last gesture to fire wins.
#[derive(Serialize, Deserialize, JsonSchema, Clone, Copy, Debug, PartialEq, Default)]
struct Stance {
    value: u32,
}

fn stance(world: &mut World, _ctx: &SystemCtx) {
    let Some(actions) = world.resource::<Actions>() else {
        return;
    };
    let next = if actions.pressed(PRONE) {
        Some(PRONE_STANCE)
    } else if actions.pressed(CROUCH) {
        Some(CROUCHING)
    } else {
        None
    };
    if let Some(next) = next {
        for (s,) in world.query::<(&mut Stance,)>() {
            s.value = next;
        }
    }
}

const SCENE: &str = "\
format: 1
seed: 1
entities:
  - name: hero
    components:
      Stance:
        value: 0
";

fn fresh_dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "ludion-input-actions-{}-{name}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn game(name: &str, map: &str) -> EngineBuilder {
    Engine::builder()
        .components(|reg| reg.register::<Stance>("Stance"))
        .system("stance", stance)
        .input_actions()
        .doc_text("input/actions.yaml", map)
        .project_root(fresh_dir(name))
}

fn booted(name: &str) -> Engine {
    let mut engine = game(name, MAP).build().expect("boots");
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

fn key(kind: &str, key: &str) -> Value {
    json!({ "kind": kind, "key": key })
}

fn pad(kind: &str, button: &str) -> Value {
    json!({ "kind": kind, "button": button })
}

fn stance_of(engine: &Engine) -> u32 {
    let world = engine.world();
    let hero = world
        .entities()
        .find(|&id| matches!(world.get::<Stance>(id), Ok(Some(_))))
        .expect("one hero");
    world.get::<Stance>(hero).unwrap().unwrap().value
}

/// Runs `ticks` ticks with `events` addressed to their frames; the stance after each tick.
fn trace(name: &str, events: &[(u64, Value)], ticks: usize) -> Vec<u32> {
    let mut engine = booted(name);
    for (frame, event) in events {
        inject(&mut engine, Some(*frame), event.clone());
    }
    (0..ticks)
        .map(|_| {
            engine.advance_live();
            stance_of(&engine)
        })
        .collect()
}

fn first_change(trace: &[u32]) -> Option<usize> {
    trace.iter().position(|&s| s != STAND)
}

fn tap_pad() -> Vec<(u64, Value)> {
    vec![
        (3, pad("gamepad_button_down", "east")),
        (6, pad("gamepad_button_up", "east")),
    ]
}

fn hold_pad(release: u64) -> Vec<(u64, Value)> {
    vec![
        (3, pad("gamepad_button_down", "east")),
        (release, pad("gamepad_button_up", "east")),
    ]
}

#[test]
fn a_pad_tap_crouches_on_release() {
    let t = trace("tap", &tap_pad(), 20);
    assert_eq!(first_change(&t), Some(6), "{t:?}");
    assert!(t[6..].iter().all(|&s| s == CROUCHING), "{t:?}");
}

#[test]
fn a_pad_hold_goes_prone_on_exactly_the_hold_tick_and_never_crouches() {
    let release = 3 + HOLD_TICKS as u64 + 6;
    let t = trace("hold", &hold_pad(release), 50);
    assert_eq!(first_change(&t), Some(3 + HOLD_TICKS), "{t:?}");
    assert!(
        t[3 + HOLD_TICKS..].iter().all(|&s| s == PRONE_STANCE),
        "no crouch on release: {t:?}"
    );
}

#[test]
fn the_keyboard_route_gives_the_same_stance_trace() {
    let tap_keys = vec![(3, key("key_down", "C")), (6, key("key_up", "C"))];
    assert_eq!(
        trace("tap-keys", &tap_keys, 20),
        trace("tap-pad", &tap_pad(), 20)
    );
    let release = 3 + HOLD_TICKS as u64 + 6;
    let hold_keys = vec![(3, key("key_down", "Z")), (release, key("key_up", "Z"))];
    assert_eq!(
        trace("hold-keys", &hold_keys, 50),
        trace("hold-pad", &hold_pad(release), 50)
    );
}

fn build_err(builder: EngineBuilder) -> String {
    match builder.build() {
        Ok(_) => panic!("the build should have failed"),
        Err(err) => format!("{err:?}"),
    }
}

#[test]
fn a_bad_document_refuses_the_build_at_its_repathed_pointer() {
    let bad = "\
format: 1
kind: input-actions
actions:
  move:
    kind: axis2
    bindings:
      - stick: left
        gesture: hold
";
    let err = build_err(game("bad", bad));
    assert!(err.contains("L1632"), "{err}");
    assert!(
        err.contains("input-actions/input/actions.yaml/actions/move/bindings/0/gesture"),
        "{err}"
    );
}

#[test]
fn no_document_is_the_singleton_count_error() {
    let builder = Engine::builder()
        .input_actions()
        .project_root(fresh_dir("none"));
    let err = build_err(builder);
    assert!(err.contains("L1315"), "{err}");
}

fn read_misspelt(world: &mut World, _ctx: &SystemCtx) {
    if let Some(actions) = world.resource::<Actions>() {
        let _ = actions.pressed(ActionName::new("crouhc"));
    }
}

#[test]
fn an_unknown_action_name_reports_l4270_once() {
    let mut engine = Engine::builder()
        .components(|reg| reg.register::<Stance>("Stance"))
        .system("read_misspelt", read_misspelt)
        .input_actions()
        .doc_text("input/actions.yaml", MAP)
        .project_root(fresh_dir("unknown"))
        .build()
        .expect("boots");
    engine.load_inline_scene(SCENE).unwrap();
    for _ in 0..10 {
        engine.advance_live();
    }
    let diagnostics = engine.world().resource::<Diagnostics>().unwrap();
    let keys: Vec<&str> = diagnostics.reported(code::INPUT_ACTION_UNKNOWN).collect();
    assert_eq!(keys, ["input-actions/crouhc"]);
}

fn snapshot_of(engine: &mut Engine) -> Value {
    match engine.execute(Command::EngineSnapshot).unwrap().body {
        ResponseBody::Snapshot { snapshot } => snapshot,
        other => panic!("expected a snapshot, got {other:?}"),
    }
}

fn restore(engine: &mut Engine, snapshot: Value) {
    match engine
        .execute(Command::EngineRestore(EngineRestoreParams { snapshot }))
        .unwrap()
        .body
    {
        ResponseBody::Restored { .. } => {}
        other => panic!("expected a restore, got {other:?}"),
    }
}

/// The documented latch, not continuation: an input held across a restore fires nothing until it
/// is released and pressed again.
#[test]
fn a_restore_latches_a_held_input_until_a_fresh_press() {
    let mut engine = booted("restore");
    inject(&mut engine, Some(1), pad("gamepad_button_down", "east"));
    for _ in 0..4 {
        engine.advance_live();
    }
    let snapshot = snapshot_of(&mut engine);
    restore(&mut engine, snapshot);

    // Held well past the hold threshold: an uninterrupted run would have gone prone by now.
    for _ in 0..HOLD_TICKS + 10 {
        engine.advance_live();
        assert_eq!(stance_of(&engine), STAND);
    }
    inject(&mut engine, None, pad("gamepad_button_up", "east"));
    engine.advance_live();
    assert_eq!(stance_of(&engine), STAND, "the latched release is silent");

    inject(&mut engine, None, pad("gamepad_button_down", "east"));
    engine.advance_live();
    inject(&mut engine, None, pad("gamepad_button_up", "east"));
    engine.advance_live();
    assert_eq!(stance_of(&engine), CROUCHING, "a fresh tap fires");
}

const THREAD_COUNTS: [usize; 3] = [1, 2, 8];
const TICKS: usize = 60;

fn thread_trace(threads: usize) -> Vec<u64> {
    let mut engine = booted(&format!("threads-{threads}"));
    inject(&mut engine, Some(3), pad("gamepad_button_down", "east"));
    inject(&mut engine, Some(10), pad("gamepad_button_up", "east"));
    inject(&mut engine, Some(20), key("key_down", "Z"));
    (0..TICKS)
        .map(|_| {
            engine.advance_live();
            engine.world_hash().unwrap().as_u64()
        })
        .collect()
}

#[test]
fn the_hash_trace_is_identical_at_every_thread_count() {
    #[allow(clippy::disallowed_methods)]
    let [one, two, eight] = std::thread::scope(|scope| {
        THREAD_COUNTS
            .map(|n| scope.spawn(move || par::with_threads(n, || thread_trace(n))))
            .map(|handle| {
                handle
                    .join()
                    .unwrap_or_else(|payload| std::panic::resume_unwind(payload))
            })
    });
    let distinct: std::collections::BTreeSet<u64> = one.iter().copied().collect();
    assert!(distinct.len() > 1, "the stance never reached the hash");
    assert_eq!(one, two, "the 1-thread and 2-thread traces diverge");
    assert_eq!(one, eight, "the 1-thread and 8-thread traces diverge");
}

const NEXT: &str = "\
format: 1
seed: 2
entities:
  - name: hero
    components:
      Transform:
        translation:
          - 0.0
          - 0.0
          - 0.0
      Stance:
        value: 0
";

fn switch_on_frame_two(world: &mut World, ctx: &SystemCtx) {
    if ctx.frame.as_u64() != 2 {
        return;
    }
    if let Some(switch) = world.resource_mut::<SceneSwitch>() {
        switch.request("next.yaml".to_string(), "hero".to_string(), Vec::new());
    }
}

/// A scene switch carries `Actions`, so a hold that began before it fires on the frame it would
/// have without it.
#[test]
fn a_switch_mid_hold_carries_the_hold() {
    let dir = fresh_dir("switch");
    std::fs::write(dir.join("next.yaml"), NEXT).unwrap();
    let mut engine = Engine::builder()
        .components(|reg| reg.register::<Stance>("Stance"))
        .system("stance", stance)
        .system("switch_on_frame_two", switch_on_frame_two)
        .input_actions()
        .doc_text("input/actions.yaml", MAP)
        .project_root(&dir)
        .build()
        .expect("boots");
    engine.load_inline_scene(SCENE).unwrap();
    inject(&mut engine, Some(0), key("key_down", "Z"));
    let trace: Vec<u32> = (0..30)
        .map(|_| {
            engine.advance_live();
            stance_of(&engine)
        })
        .collect();
    assert!(engine.last_transition_error().is_none());
    assert_eq!(first_change(&trace), Some(HOLD_TICKS), "{trace:?}");
}

/// The YAML sample in `docs/input.md` is a loadable document, not just illustration.
#[test]
fn the_documented_sample_loads() {
    let docs = include_str!("../../../docs/input.md");
    let start = docs.find("```yaml\ngestures:").expect("the sample") + "```yaml\n".len();
    let sample = &docs[start..start + docs[start..].find("```").unwrap()];
    let text = format!("format: 1\nkind: input-actions\n{sample}");
    let engine = game("doc-sample", &text).build();
    assert!(engine.is_ok(), "{:?}", engine.err());
}
