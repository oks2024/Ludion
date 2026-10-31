//! Divergence bisection over a recorded session: the keyframe-drift verifier
//! ([`verify_replay_keyframes`]) and the two-run localizer ([`bisect_divergence`]).
//!
//! The synthetic game here is deliberately two systems over two components, so a change confined to
//! one system is localizable to one component — the shape a real divergence has, minus the noise.

use std::io::Write;
use std::sync::{Arc, Mutex};

use ludion_engine::{
    Engine, EngineBuilder, RunConfig, bisect_divergence, verify_replay_keyframes, world_diff,
};
use ludion_protocol::command::{EngineRestoreParams, SimStepParams};
use ludion_protocol::replay::{ReplayReader, ReplayRecord};
use ludion_protocol::{Command, ResponseBody};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize, JsonSchema, Clone, Copy)]
struct Counter {
    n: f32,
}

#[derive(Serialize, Deserialize, JsonSchema, Clone, Copy)]
struct Tag {
    t: f32,
}

/// The first system: identical in every configuration, so a localization that names *it* is wrong.
///
/// The increments across these systems are deliberately *awkward* — accumulating them lands on
/// `f32`s whose shortest `f64` rendering is long, which is where a keyframe's JSON round trip loses
/// a ULP. A verifier that compared raw JSON instead of typed values would fail here.
fn bump(world: &mut ludion_ecs::World, _ctx: &ludion_ecs::SystemCtx) {
    for (c,) in world.query::<(&mut Counter,)>() {
        c.n += 0.1;
    }
}

/// The second system, v1 — the recorded behavior.
fn scale_v1(world: &mut ludion_ecs::World, _ctx: &ludion_ecs::SystemCtx) {
    for (t,) in world.query::<(&mut Tag,)>() {
        t.t += 1.0 / 3.0;
    }
}

/// The second system, v2 — one retuned constant, the smallest gameplay change there is.
fn scale_v2(world: &mut ludion_ecs::World, _ctx: &ludion_ecs::SystemCtx) {
    for (t,) in world.query::<(&mut Tag,)>() {
        t.t += 2.0 / 3.0;
    }
}

/// `scale_v1` until the frame passes `KEYFRAME_INTERVAL`, then a different constant: a change no
/// interval-boundary keyframe sees when the session stops soon after it.
fn scale_late(world: &mut ludion_ecs::World, ctx: &ludion_ecs::SystemCtx) {
    let step = if ctx.frame.as_u64() > ludion_engine::KEYFRAME_INTERVAL {
        2.0 / 3.0
    } else {
        1.0 / 3.0
    };
    for (t,) in world.query::<(&mut Tag,)>() {
        t.t += step;
    }
}

const SCENE: &str = "\
format: 1
seed: 5
entities:
  - name: subject
    components:
      Counter:
        n: 0.0
      Tag:
        t: 0.0
";

fn game(scale: fn(&mut ludion_ecs::World, &ludion_ecs::SystemCtx)) -> EngineBuilder {
    Engine::builder()
        .components(|r| {
            r.register::<Counter>("Counter")?;
            r.register::<Tag>("Tag")
        })
        .system("bump", bump)
        .system("scale", scale)
}

#[derive(Clone, Default)]
struct SharedSink(Arc<Mutex<Vec<u8>>>);

impl Write for SharedSink {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(buf);
        Ok(buf.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

/// Records `ticks` single-tick steps of `scale`'s game and returns the `.lrp` bytes.
fn record(scale: fn(&mut ludion_ecs::World, &ludion_ecs::SystemCtx), ticks: u64) -> Vec<u8> {
    let sink = SharedSink::default();
    let mut engine = game(scale).build().unwrap();
    engine.load_inline_scene(SCENE).unwrap();
    engine.set_recording(sink.clone()).unwrap();
    for _ in 0..ticks {
        engine
            .execute(Command::SimStep(SimStepParams {
                ticks: 1,
                hash: false,
            }))
            .unwrap();
    }
    engine.stop_recording().unwrap();
    let sink = sink.0.lock().unwrap();
    sink.clone()
}

/// (a) The frame-alignment pin: a replay verified against the code that recorded it reproduces every
/// embedded keyframe, including the ones written mid-session at the production
/// `KEYFRAME_INTERVAL` boundaries. Green here is what makes a *failure* elsewhere meaningful.
#[test]
fn unchanged_code_reproduces_every_recorded_keyframe() {
    // 601 ticks crosses two keyframe boundaries, so the boot keyframe is not the only one checked.
    let interval = ludion_engine::KEYFRAME_INTERVAL;
    let bytes = record(scale_v1, interval * 2 + 1);

    let report = verify_replay_keyframes(game(scale_v1), &bytes, ".").unwrap();
    assert!(
        report.is_clean(),
        "an unchanged build must reproduce its own keyframes exactly:\n{report}"
    );
    assert_eq!(
        report.keyframes, 4,
        "the boot keyframe, one per crossed interval boundary, and the closing one"
    );
    assert_eq!(report.verified, report.keyframes);
    assert!(report.into_result().is_ok());
}

/// The frames of the keyframes a recording holds, in file order.
fn keyframe_frames(bytes: &[u8]) -> Vec<u64> {
    ReplayReader::new(bytes)
        .unwrap()
        .records()
        .unwrap()
        .iter()
        .filter_map(|record| match record {
            ReplayRecord::Keyframe { frame, .. } => Some(frame.as_u64()),
            _ => None,
        })
        .collect()
}

/// A recording shorter than one interval is still checked at its end, by the closing keyframe.
#[test]
fn a_short_session_is_verified_at_its_close() {
    let bytes = record(scale_v1, 30);

    let report = verify_replay_keyframes(game(scale_v2), &bytes, ".").unwrap();
    assert!(!report.is_clean(), "the retuned constant must be caught");
    assert_eq!(report.divergence.as_ref().unwrap().frame.as_u64(), 30);
    assert_eq!(report.verified, 1, "the boot keyframe verified first");
    assert_eq!(report.keyframes, 2);
}

/// Drift after the last interval boundary is caught at the recording's end.
#[test]
fn drift_after_the_last_interval_boundary_is_caught() {
    let interval = ludion_engine::KEYFRAME_INTERVAL;
    let bytes = record(scale_v1, interval + 30);

    let report = verify_replay_keyframes(game(scale_late), &bytes, ".").unwrap();
    assert!(!report.is_clean(), "the late change must be caught");
    assert_eq!(
        report.divergence.as_ref().unwrap().frame.as_u64(),
        interval + 30
    );
    assert_eq!(
        report.verified, 2,
        "the boot and boundary keyframes matched"
    );
}

/// Stopping right after a keyframe adds none: a boundary stop and a zero-tick session each end on one.
#[test]
fn stopping_on_a_boundary_writes_no_duplicate_keyframe() {
    let interval = ludion_engine::KEYFRAME_INTERVAL;
    assert_eq!(keyframe_frames(&record(scale_v1, interval)), [0, interval]);
    assert_eq!(keyframe_frames(&record(scale_v1, 0)), [0]);
}

/// (b) The replay-churn answer: after a gameplay change the committed replay's keyframes are stale,
/// and the report names the frame, the entity, and the component that drifted — rather than leaving
/// a re-record as a rubber stamp.
#[test]
fn a_changed_system_is_named_at_the_first_stale_keyframe() {
    let interval = ludion_engine::KEYFRAME_INTERVAL;
    let bytes = record(scale_v1, interval + 1);

    let report = verify_replay_keyframes(game(scale_v2), &bytes, ".").unwrap();
    assert!(!report.is_clean(), "the retuned constant must be caught");

    let divergence = report.divergence.clone().unwrap();
    assert_eq!(
        divergence.frame.as_u64(),
        interval,
        "the boot keyframe still matches (nothing has ticked); the first mid-run one does not"
    );
    assert_eq!(report.verified, 1, "the boot keyframe verified first");
    assert_eq!(
        divergence.live_frame, divergence.frame,
        "stepping is intact"
    );
    assert!(!divergence.rng_differs, "neither system draws from the RNG");
    assert!(
        !divergence.allocator_differs,
        "nothing spawned or despawned"
    );

    let world = &divergence.world;
    assert!(world.missing.is_empty() && world.spawned.is_empty());
    assert_eq!(world.changed_total, 1);
    let entity = &world.changed[0];
    assert_eq!(entity.components.len(), 1, "only `Tag` drifted");
    assert_eq!(entity.components[0].component, "Tag");
    assert_eq!(entity.components[0].fields, ["t"]);

    let rendered = report.to_string();
    assert!(rendered.contains("Tag"), "the report names it:\n{rendered}");
    let diagnostic = report.into_result().unwrap_err();
    assert_eq!(diagnostic.code.as_str(), "L3024");
    assert!(diagnostic.help.is_some(), "the fix is spelled out");
}

/// (c) Two-run localization: two configurations differing in exactly one system diverge at a frame,
/// and the bisector names *that* system and *that* component — not the first system, not the whole
/// world.
#[test]
fn two_configurations_localize_to_the_system_and_component_that_differ() {
    let bytes = record(scale_v1, 5);

    let report = bisect_divergence(
        RunConfig::new("v1", || game(scale_v1)),
        RunConfig::new("v2", || game(scale_v2)),
        &bytes,
        ".",
    )
    .unwrap()
    .expect("the two configurations diverge");

    assert_eq!(
        report.frame.as_u64(),
        1,
        "the very first tick already differs"
    );
    assert_eq!(
        report.system.as_deref(),
        Some("scale"),
        "`bump` is identical on both sides, so the worlds agree at its boundary:\n{report}"
    );
    assert_eq!(report.systems_probed, 2);
    assert_eq!(report.world.changed_total, 1);
    assert_eq!(report.world.changed[0].components[0].component, "Tag");
}

/// The same bisector over two *identical* configurations finds nothing — the negative that keeps the
/// positive honest, and the shape a green parallel-sim gate has.
#[test]
fn identical_configurations_do_not_diverge() {
    let bytes = record(scale_v1, 5);

    let report = bisect_divergence(
        RunConfig::new("left", || game(scale_v1)),
        RunConfig::new("right", || game(scale_v1)),
        &bytes,
        ".",
    )
    .unwrap();
    assert!(
        report.is_none(),
        "identical builds produce identical traces"
    );
}

/// A configuration's ambient scope wraps every drive it performs — the seam a thread count or a
/// feature flag enters through. Here the scope only counts, proving it is entered once per drive and
/// that its result is what the bisector sees.
#[test]
fn a_run_configuration_scope_wraps_every_drive() {
    let bytes = record(scale_v1, 5);
    let drives = std::cell::Cell::new(0usize);

    let report = bisect_divergence(
        RunConfig::new("scoped", || game(scale_v1)).scoped(|body| {
            drives.set(drives.get() + 1);
            body()
        }),
        RunConfig::new("plain", || game(scale_v2)),
        &bytes,
        ".",
    )
    .unwrap()
    .expect("the two configurations diverge");

    assert_eq!(report.system.as_deref(), Some("scale"));
    assert_eq!(
        drives.get(),
        3,
        "one trace drive plus the localizer's two probe passes"
    );
}

/// A `scene.load` after a tick resets the sim frame, so a divergence frame names two states: the
/// bisector refuses the replay up front (`L3021`) instead of probing the wrong one.
#[test]
fn a_scene_load_after_a_tick_is_refused_by_the_localizer() {
    let dir = std::env::temp_dir().join(format!("ludion-bisect-load-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("again.yaml"), SCENE).unwrap();

    let sink = SharedSink::default();
    let mut engine = game(scale_v1).project_root(&dir).build().unwrap();
    engine.load_inline_scene(SCENE).unwrap();
    engine.set_recording(sink.clone()).unwrap();
    for command in [
        Command::SimStep(SimStepParams {
            ticks: 2,
            hash: false,
        }),
        Command::SceneLoad(ludion_protocol::command::SceneLoadParams {
            path: "again.yaml".into(),
        }),
        Command::SimStep(SimStepParams {
            ticks: 2,
            hash: false,
        }),
    ] {
        engine.execute(command).unwrap();
    }
    engine.stop_recording().unwrap();
    let bytes = sink.0.lock().unwrap().clone();

    for frame in [0, 3] {
        let error = ludion_engine::localize_divergence(
            RunConfig::new("v1", || game(scale_v1)),
            RunConfig::new("v2", || game(scale_v2)),
            &bytes,
            &dir,
            ludion_core::time::FrameIndex::new(frame),
        )
        .expect_err("the replay's frame numbers repeat");
        assert_eq!(error.code, ludion_core::codes::code::REPLAY_FRAME_WINDOW);
        assert!(error.message.contains("bisect"), "{}", error.message);
    }
}

/// `scale_v1` until the tick that starts on frame 2, then a different constant: a divergence that
/// first shows on the frame the panic test below panics on.
fn scale_from_two(world: &mut ludion_ecs::World, ctx: &ludion_ecs::SystemCtx) {
    let step = if ctx.frame.as_u64() >= PANIC_FRAME {
        2.0 / 3.0
    } else {
        1.0 / 3.0
    };
    for (t,) in world.query::<(&mut Tag,)>() {
        t.t += step;
    }
}

/// The frame whose tick panics (producing frame `PANIC_FRAME + 1`).
const PANIC_FRAME: u64 = 2;

fn boom(_world: &mut ludion_ecs::World, ctx: &ludion_ecs::SystemCtx) {
    if ctx.frame.as_u64() == PANIC_FRAME {
        panic!("boom");
    }
}

fn panicking_game(scale: fn(&mut ludion_ecs::World, &ludion_ecs::SystemCtx)) -> EngineBuilder {
    game(scale).system("boom", boom)
}

/// A recorded panic tick carries a `system_panic` marker, and the localizer's probe of that very
/// frame must accept the expected `L4007` as the replay does — not report `L3050`.
#[test]
fn the_localizer_probes_a_recorded_panic_tick() {
    let sink = SharedSink::default();
    let mut engine = panicking_game(scale_v1).build().unwrap();
    engine.load_inline_scene(SCENE).unwrap();
    engine.set_recording(sink.clone()).unwrap();
    let step = || {
        Command::SimStep(SimStepParams {
            ticks: 1,
            hash: false,
        })
    };
    for _ in 0..PANIC_FRAME {
        engine.execute(step()).unwrap();
    }
    engine.execute(step()).expect_err("the panicking tick");
    engine.stop_recording().unwrap();
    let bytes = sink.0.lock().unwrap().clone();

    let report = bisect_divergence(
        RunConfig::new("v1", || panicking_game(scale_v1)),
        RunConfig::new("v2", || panicking_game(scale_from_two)),
        &bytes,
        ".",
    )
    .expect("the probe of a recorded panic tick is not an error")
    .expect("the two configurations diverge");
    assert_eq!(report.frame.as_u64(), PANIC_FRAME + 1);
}

/// A second engine restored from the first's snapshot holds an identical world; one component edit
/// then shows up in `world_diff` by component and field.
#[test]
fn world_diff_is_empty_for_a_restored_twin_and_names_an_edit() {
    let mut a = game(scale_v1).build().unwrap();
    a.load_inline_scene(SCENE).unwrap();
    a.execute(Command::SimStep(SimStepParams {
        ticks: 3,
        hash: false,
    }))
    .unwrap();
    let snapshot = match a.execute(Command::EngineSnapshot).unwrap().body {
        ResponseBody::Snapshot { snapshot } => snapshot,
        other => panic!("expected a snapshot, got {other:?}"),
    };
    let mut b = game(scale_v1).build().unwrap();
    b.load_inline_scene(SCENE).unwrap();
    b.execute(Command::EngineRestore(EngineRestoreParams {
        snapshot: serde_json::Value::clone(&snapshot),
    }))
    .unwrap();
    assert!(world_diff(&a, &b).unwrap().is_empty());

    for (c,) in b.world_mut().query::<(&mut Counter,)>() {
        c.n += 1.0;
    }
    let diff = world_diff(&a, &b).unwrap();
    assert_eq!(diff.changed_total, 1);
    let component = &diff.changed[0].components[0];
    assert_eq!(component.component, "Counter");
    assert_eq!(component.fields, vec!["n".to_string()]);
}
