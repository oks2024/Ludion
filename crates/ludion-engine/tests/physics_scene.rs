//! `Plan/physics.md` W2: a scene's `physics:` block is the only opt-in there is. Present, every
//! host that builds a world from the scene steps rigid bodies; absent, exactly as before. Mirrors
//! `physics_snapshot.rs`'s `fresh_dir`/inline-scene style, a handful of boxes.

use std::path::PathBuf;
use std::time::Duration;

use crossbeam_channel::{bounded, unbounded};
use ludion_ecs::{SystemCtx, World};
use ludion_engine::live::{StampedCommand, run_live, shutdown_channel};
use ludion_engine::{Egress, EgressChannels, Engine, SceneSwitch, Transform};
use ludion_physics::PhysicsWorld;
use ludion_protocol::command::{
    ComponentSetParams, EngineRestoreParams, SimStepParams, StreamSubscribeParams, SubscribeMode,
};
use ludion_protocol::event::{Event, LogEvent};
use ludion_protocol::frame::FrameFormat;
use ludion_protocol::{Channel, Command, ResponseBody};
use ludion_spatial::Collider;
use serde_json::Value;

/// An empty project root, so the engine's content walk finds nothing and the scene is the whole
/// input. One per test name, wiped on entry, so a re-run never inherits a previous one.
fn fresh_dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "ludion-physics-scene-{}-{name}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn bare_engine(name: &str) -> Engine {
    Engine::builder()
        .project_root(fresh_dir(name))
        .build()
        .expect("boots")
}

/// One rigid box entity above a static floor, tilted and given initial angular velocity so a
/// contact-solver setting (like `solver_iterations`) has something chaotic to diverge over —
/// straight free fall alone never touches the solver's iteration count. `physics_block` is
/// inserted verbatim after `format: 1\n` (including its own trailing newline; empty for none).
/// `rigid_extra` is inserted verbatim inside `crate0`'s `rigid:` mapping (its own field lines,
/// correctly indented; empty for none).
fn scene(physics_block: &str, rigid_extra: &str) -> String {
    format!(
        "format: 1\n\
         {physics_block}\
         entities:\n\
         \x20\x20- name: floor\n\
         \x20\x20\x20\x20components:\n\
         \x20\x20\x20\x20\x20\x20Transform:\n\
         \x20\x20\x20\x20\x20\x20\x20\x20translation:\n\
         \x20\x20\x20\x20\x20\x20\x20\x20\x20\x20- 0.0\n\
         \x20\x20\x20\x20\x20\x20\x20\x20\x20\x20- -0.5\n\
         \x20\x20\x20\x20\x20\x20\x20\x20\x20\x20- 0.0\n\
         \x20\x20\x20\x20\x20\x20Collider:\n\
         \x20\x20\x20\x20\x20\x20\x20\x20shape:\n\
         \x20\x20\x20\x20\x20\x20\x20\x20\x20\x20box:\n\
         \x20\x20\x20\x20\x20\x20\x20\x20\x20\x20\x20\x20half_extents:\n\
         \x20\x20\x20\x20\x20\x20\x20\x20\x20\x20\x20\x20\x20\x20- 10.0\n\
         \x20\x20\x20\x20\x20\x20\x20\x20\x20\x20\x20\x20\x20\x20- 0.5\n\
         \x20\x20\x20\x20\x20\x20\x20\x20\x20\x20\x20\x20\x20\x20- 10.0\n\
         \x20\x20\x20\x20\x20\x20\x20\x20body: static\n\
         \x20\x20- name: crate0\n\
         \x20\x20\x20\x20components:\n\
         \x20\x20\x20\x20\x20\x20Transform:\n\
         \x20\x20\x20\x20\x20\x20\x20\x20translation:\n\
         \x20\x20\x20\x20\x20\x20\x20\x20\x20\x20- 0.0\n\
         \x20\x20\x20\x20\x20\x20\x20\x20\x20\x20- 5.0\n\
         \x20\x20\x20\x20\x20\x20\x20\x20\x20\x20- 0.0\n\
         \x20\x20\x20\x20\x20\x20\x20\x20rotation:\n\
         \x20\x20\x20\x20\x20\x20\x20\x20\x20\x20- 15.0\n\
         \x20\x20\x20\x20\x20\x20\x20\x20\x20\x20- 30.0\n\
         \x20\x20\x20\x20\x20\x20\x20\x20\x20\x20- 7.5\n\
         \x20\x20\x20\x20\x20\x20Collider:\n\
         \x20\x20\x20\x20\x20\x20\x20\x20shape:\n\
         \x20\x20\x20\x20\x20\x20\x20\x20\x20\x20box:\n\
         \x20\x20\x20\x20\x20\x20\x20\x20\x20\x20\x20\x20half_extents:\n\
         \x20\x20\x20\x20\x20\x20\x20\x20\x20\x20\x20\x20\x20\x20- 0.5\n\
         \x20\x20\x20\x20\x20\x20\x20\x20\x20\x20\x20\x20\x20\x20- 0.5\n\
         \x20\x20\x20\x20\x20\x20\x20\x20\x20\x20\x20\x20\x20\x20- 0.5\n\
         \x20\x20\x20\x20\x20\x20\x20\x20body:\n\
         \x20\x20\x20\x20\x20\x20\x20\x20\x20\x20rigid:\n\
         \x20\x20\x20\x20\x20\x20\x20\x20\x20\x20\x20\x20mass: 1.0\n\
         \x20\x20\x20\x20\x20\x20\x20\x20\x20\x20\x20\x20friction: 0.5\n\
         \x20\x20\x20\x20\x20\x20\x20\x20\x20\x20\x20\x20restitution: 0.2\n\
         {rigid_extra}\
         \x20\x20\x20\x20\x20\x20Velocity:\n\
         \x20\x20\x20\x20\x20\x20\x20\x20angular:\n\
         \x20\x20\x20\x20\x20\x20\x20\x20\x20\x20- 1.0\n\
         \x20\x20\x20\x20\x20\x20\x20\x20\x20\x20- 0.5\n\
         \x20\x20\x20\x20\x20\x20\x20\x20\x20\x20- 0.25\n"
    )
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

/// The `log`-channel lines observed after `commands` run against a live engine, followed by one
/// forced tick — the public seam onto `Diagnostics`, since its own drain (`take()`) is
/// `pub(crate)` and unreachable from this integration test. Mirrors `patterns.rs`'s `log_lines`.
fn log_lines_after(mut engine: Engine, commands: Vec<Command>) -> Vec<LogEvent> {
    engine
        .execute(Command::SimPause)
        .expect("the engine pauses");
    let (intake, intake_rx) = unbounded();
    let (shutdown, shutdown_rx) = shutdown_channel();
    let (frames, _frames_rx) = bounded::<Egress>(1);
    let (events, events_rx) = unbounded::<Egress>();
    let handle = std::thread::spawn(move || {
        run_live(
            engine,
            intake_rx,
            shutdown_rx,
            Some(EgressChannels { frames, events }),
        )
    });

    let send = |command: Command| {
        let (stamped, reply) = StampedCommand::new(command);
        intake.send(stamped).unwrap();
        reply
            .recv_timeout(Duration::from_secs(5))
            .expect("the sim thread replies")
            .expect("the command succeeds");
    };
    send(Command::StreamSubscribe(StreamSubscribeParams {
        channel: Channel::Log,
        mode: SubscribeMode::LatestWins { max_fps: None },
        format: FrameFormat::Qoi,
    }));
    for command in commands {
        send(command);
    }
    send(Command::SimStep(SimStepParams {
        ticks: 1,
        hash: false,
    }));

    let mut lines = Vec::new();
    let mut wait = Duration::from_secs(5);
    while let Ok(Egress::Event(event)) = events_rx.recv_timeout(wait) {
        if let Event::Log(log) = event.message.event {
            lines.push(log);
        }
        wait = Duration::from_millis(300);
    }
    shutdown.signal();
    let _ = handle.join();
    lines
}

fn advance_trace(engine: &mut Engine, n: usize) -> Vec<u64> {
    (0..n)
        .map(|_| {
            engine.advance_live();
            engine.world_hash().unwrap().as_u64()
        })
        .collect()
}

/// The `y` of `crate0`'s mirrored `Transform`.
fn crate_height(engine: &Engine) -> f32 {
    let world = engine.world();
    let entity = world
        .entities()
        .find(|&entity| {
            matches!(world.get::<ludion_engine::Name>(entity), Ok(Some(name)) if name.0 == "crate0")
        })
        .expect("the scene authors one crate");
    world
        .get::<Transform>(entity)
        .expect("a live entity")
        .expect("its transform")
        .translation[1]
}

#[test]
fn a_scene_without_the_block_never_steps() {
    let mut engine = bare_engine("no-block");
    engine.load_inline_scene(&scene("", "")).unwrap();
    let start = crate_height(&engine);
    for _ in 0..30 {
        engine.advance_live();
    }
    assert_eq!(
        crate_height(&engine),
        start,
        "with no physics: block the box's Transform never moves"
    );
    assert!(
        engine.world().resource::<PhysicsWorld>().is_none(),
        "no physics: block means no solver resource at all"
    );
}

#[test]
fn the_block_steps_under_a_bare_engine() {
    let mut engine = bare_engine("free-block");
    engine
        .load_inline_scene(&scene("physics:\n  space: free\n", ""))
        .unwrap();
    let start = crate_height(&engine);
    for _ in 0..30 {
        engine.advance_live();
    }
    assert!(
        crate_height(&engine) < start,
        "the scene's own physics: block is the opt-in; the box must fall under a plain builder"
    );
}

#[test]
fn solver_iterations_reach_the_solver() {
    let mut with_default = bare_engine("solver-default");
    with_default
        .load_inline_scene(&scene("physics:\n  space: free\n", ""))
        .unwrap();
    let default_trace = advance_trace(&mut with_default, 120);

    let mut with_one = bare_engine("solver-one");
    with_one
        .load_inline_scene(&scene(
            "physics:\n  space: free\n  solver_iterations: 1\n",
            "",
        ))
        .unwrap();
    with_one.advance_live();
    assert_eq!(
        with_one
            .world()
            .resource::<PhysicsWorld>()
            .expect("a claimed rigid body installs the solver")
            .solver_iterations(),
        1,
        "the scene's solver_iterations must reach the solver"
    );
    let one_trace: Vec<u64> = std::iter::once(with_one.world_hash().unwrap().as_u64())
        .chain((1..120).map(|_| {
            with_one.advance_live();
            with_one.world_hash().unwrap().as_u64()
        }))
        .collect();

    assert_ne!(
        default_trace, one_trace,
        "a different solver_iterations count must move the contact-resolution trace"
    );
}

/// Two rigid boxes, no floor: `crate0` omits its own `sleep_after_ticks`, `crate1` authors `90`.
/// A minimal scene for one focused assertion, not the falling/tumbling one [`scene`] builds.
fn sleep_scene() -> String {
    "format: 1\n\
     physics:\n\
     \x20\x20space: free\n\
     \x20\x20sleep_after_ticks: 7\n\
     entities:\n\
     \x20\x20- name: crate0\n\
     \x20\x20\x20\x20components:\n\
     \x20\x20\x20\x20\x20\x20Transform:\n\
     \x20\x20\x20\x20\x20\x20\x20\x20translation:\n\
     \x20\x20\x20\x20\x20\x20\x20\x20\x20\x20- 0.0\n\
     \x20\x20\x20\x20\x20\x20\x20\x20\x20\x20- 5.0\n\
     \x20\x20\x20\x20\x20\x20\x20\x20\x20\x20- 0.0\n\
     \x20\x20\x20\x20\x20\x20Collider:\n\
     \x20\x20\x20\x20\x20\x20\x20\x20shape:\n\
     \x20\x20\x20\x20\x20\x20\x20\x20\x20\x20box:\n\
     \x20\x20\x20\x20\x20\x20\x20\x20\x20\x20\x20\x20half_extents:\n\
     \x20\x20\x20\x20\x20\x20\x20\x20\x20\x20\x20\x20\x20\x20- 0.5\n\
     \x20\x20\x20\x20\x20\x20\x20\x20\x20\x20\x20\x20\x20\x20- 0.5\n\
     \x20\x20\x20\x20\x20\x20\x20\x20\x20\x20\x20\x20\x20\x20- 0.5\n\
     \x20\x20\x20\x20\x20\x20\x20\x20body:\n\
     \x20\x20\x20\x20\x20\x20\x20\x20\x20\x20rigid: {}\n\
     \x20\x20- name: crate1\n\
     \x20\x20\x20\x20components:\n\
     \x20\x20\x20\x20\x20\x20Transform:\n\
     \x20\x20\x20\x20\x20\x20\x20\x20translation:\n\
     \x20\x20\x20\x20\x20\x20\x20\x20\x20\x20- 3.0\n\
     \x20\x20\x20\x20\x20\x20\x20\x20\x20\x20- 5.0\n\
     \x20\x20\x20\x20\x20\x20\x20\x20\x20\x20- 0.0\n\
     \x20\x20\x20\x20\x20\x20Collider:\n\
     \x20\x20\x20\x20\x20\x20\x20\x20shape:\n\
     \x20\x20\x20\x20\x20\x20\x20\x20\x20\x20box:\n\
     \x20\x20\x20\x20\x20\x20\x20\x20\x20\x20\x20\x20half_extents:\n\
     \x20\x20\x20\x20\x20\x20\x20\x20\x20\x20\x20\x20\x20\x20- 0.5\n\
     \x20\x20\x20\x20\x20\x20\x20\x20\x20\x20\x20\x20\x20\x20- 0.5\n\
     \x20\x20\x20\x20\x20\x20\x20\x20\x20\x20\x20\x20\x20\x20- 0.5\n\
     \x20\x20\x20\x20\x20\x20\x20\x20body:\n\
     \x20\x20\x20\x20\x20\x20\x20\x20\x20\x20rigid:\n\
     \x20\x20\x20\x20\x20\x20\x20\x20\x20\x20\x20\x20sleep_after_ticks: 90\n"
        .to_string()
}

fn rigid_body_of(engine: &Engine, name: &str) -> ludion_spatial::RigidBody {
    let entity = engine
        .world()
        .entities()
        .find(|&entity| {
            matches!(
                engine.world().get::<ludion_engine::Name>(entity),
                Ok(Some(found)) if found.0 == name
            )
        })
        .unwrap_or_else(|| panic!("the scene authors a `{name}` entity"));
    let collider = engine
        .world()
        .get::<Collider>(entity)
        .expect("a live entity")
        .expect("its collider");
    let ludion_spatial::Body::Rigid(rigid) = collider.body else {
        panic!("`{name}` must be a rigid body");
    };
    rigid
}

#[test]
fn sleep_default_resolves_in_the_solver_not_the_component() {
    let mut engine = bare_engine("sleep-default");
    engine.load_inline_scene(&sleep_scene()).unwrap();
    engine.advance_live();

    assert_eq!(
        engine
            .world()
            .resource::<PhysicsWorld>()
            .expect("a claimed rigid body installs the solver")
            .sleep_after_ticks(),
        7,
        "the scene block's sleep_after_ticks resolves into the solver"
    );

    assert_eq!(
        rigid_body_of(&engine, "crate0").sleep_after_ticks,
        None,
        "an omitted sleep_after_ticks is never rewritten with the resolved value"
    );
    assert_eq!(
        rigid_body_of(&engine, "crate1").sleep_after_ticks,
        Some(90),
        "an authored sleep_after_ticks is never rewritten by the resolution"
    );

    // A round trip through the world->scene writer proves the omission survives a save, not just
    // the live component.
    let text = engine.scene_text().unwrap();
    let crate0 = text
        .split("name: crate0")
        .nth(1)
        .expect("crate0 is in the saved text");
    let crate0 = &crate0[..crate0.find("name: crate1").unwrap_or(crate0.len())];
    assert!(
        !crate0.contains("sleep_after_ticks"),
        "crate0 omitted its own sleep_after_ticks; the saved text must not resolve it in: {crate0}"
    );
    assert!(
        text.contains("sleep_after_ticks: 90"),
        "crate1's authored sleep_after_ticks must survive the round trip; saved text:\n{text}"
    );
}

/// A `Transform` body stores Euler degrees, so the pose the mirror writes reads back with different
/// bits than the solver held. The mirror's baseline is the read-back, so that difference is not an
/// edit: the tumbled crate is not re-posed and woken every tick, and it falls asleep.
#[test]
fn a_tumbled_transform_body_falls_asleep() {
    let mut engine = bare_engine("tumbled-sleeps");
    engine
        .load_inline_scene(
            &scene("physics:\n  space: free\n  sleep_after_ticks: 10\n", "")
                .replace("- 30.0\n", "- 37.0\n"),
        )
        .unwrap();
    let entity = engine
        .world()
        .entities()
        .find(|&entity| {
            matches!(
                engine.world().get::<ludion_engine::Name>(entity),
                Ok(Some(found)) if found.0 == "crate0"
            )
        })
        .expect("the scene authors a `crate0` entity");

    let mut asleep = false;
    for _ in 0..600 {
        engine.advance_live();
        let physics = engine.world().resource::<PhysicsWorld>().expect("a solver");
        if physics.is_sleeping(entity) == Some(true) {
            asleep = true;
            break;
        }
    }
    assert!(
        asleep,
        "a resting Transform body must fall asleep within 600 ticks"
    );
}

/// The `ludion dev` restart case: a snapshot restored into a fresh engine that never loaded the
/// scene still steps physics, because the scene's `physics:` block rides the snapshot.
#[test]
fn restore_into_a_fresh_engine_keeps_physics() {
    let text = scene("physics:\n  space: free\n", "");

    let mut uninterrupted = bare_engine("restore-reference");
    uninterrupted.load_inline_scene(&text).unwrap();
    let reference = advance_trace(&mut uninterrupted, 80);

    let mut captured = bare_engine("restore-captured");
    captured.load_inline_scene(&text).unwrap();
    let first_half = advance_trace(&mut captured, 40);
    assert_eq!(first_half, reference[..40]);
    let document = snapshot_of(&mut captured);
    assert!(
        document.get("scene_physics").is_some(),
        "a physics scene's snapshot must carry the scene_physics field"
    );

    // A fresh engine that never loaded any scene — the `ludion dev` restart shape.
    let mut fresh = bare_engine("restore-fresh");
    restore(&mut fresh, document);
    let second_half = advance_trace(&mut fresh, 40);
    assert_eq!(
        second_half,
        reference[40..],
        "a restore into a scene-free fresh engine must still step physics"
    );
}

/// A restored snapshot that carries solver state (`physics`) but no scene physics settings
/// (`scene_physics`) — a hand-edited document, or one from a build old enough to predate the
/// field — reports `L3034` once at the next tick boundary, since a restore replaces the engine's
/// physics settings with the document's own and there is then nothing to install the
/// write/step/mirror triple from.
#[test]
fn a_snapshot_missing_scene_physics_reports_l3034_on_restore() {
    let text = scene("physics:\n  space: free\n", "");
    let mut captured = bare_engine("missing-scene-physics-source");
    captured.load_inline_scene(&text).unwrap();
    for _ in 0..5 {
        captured.advance_live();
    }
    let mut document = snapshot_of(&mut captured);
    assert!(
        document.get("physics").is_some(),
        "a physics scene's snapshot must carry solver state"
    );
    document
        .as_object_mut()
        .expect("a snapshot document is a JSON object")
        .remove("scene_physics");

    let mut target = bare_engine("missing-scene-physics-target");
    target.load_inline_scene(&text).unwrap();
    let lines = log_lines_after(
        target,
        vec![Command::EngineRestore(EngineRestoreParams {
            snapshot: document,
        })],
    );
    assert!(
        lines.iter().any(|line| code_is(line, "L3034")),
        "expected an L3034 warning after a restore missing scene_physics: {lines:?}"
    );
}

#[test]
fn a_normal_restore_reports_no_l3034() {
    let text = scene("physics:\n  space: free\n", "");
    let mut captured = bare_engine("normal-restore-source");
    captured.load_inline_scene(&text).unwrap();
    for _ in 0..5 {
        captured.advance_live();
    }
    let document = snapshot_of(&mut captured);

    let mut target = bare_engine("normal-restore-target");
    target.load_inline_scene(&text).unwrap();
    let lines = log_lines_after(
        target,
        vec![Command::EngineRestore(EngineRestoreParams {
            snapshot: document,
        })],
    );
    assert!(
        !lines.iter().any(|line| code_is(line, "L3034")),
        "a normal restore must not report L3034: {lines:?}"
    );
}

/// Restore re-runs the scene parser's own value checks over the decoded `scene_physics` — a
/// hand-edited snapshot with an out-of-range `solver_iterations` never reaches the schedule or
/// the world, exactly as loading such a scene from a file would fail at `L1033`.
#[test]
fn a_snapshots_out_of_range_solver_iterations_fails_restore_at_l1033() {
    let text = scene("physics:\n  space: free\n", "");
    let mut engine = bare_engine("bad-scene-physics-source");
    engine.load_inline_scene(&text).unwrap();
    engine.advance_live();
    let mut document = snapshot_of(&mut engine);
    document["scene_physics"]["solver_iterations"] = Value::from(0);

    let mut target = bare_engine("bad-scene-physics-target");
    target.load_inline_scene(&text).unwrap();
    let diagnostic = target
        .execute(Command::EngineRestore(EngineRestoreParams {
            snapshot: document,
        }))
        .expect_err("an out-of-range solver_iterations must fail the restore");
    assert_eq!(diagnostic.code.as_str(), "L1033");
    assert_eq!(
        diagnostic.path.as_deref(),
        Some("/scene_physics/solver_iterations")
    );
}

const LAYERED_BLOCK: &str = "physics:\n  space: free\n  layers:\n    - name: dummy\n      ignores:\n        - dummy\n        - sling\n    - name: sling\n";

/// The declared layers ride the snapshot's `scene_physics` and come back through a restore into a
/// fresh engine (a re-snapshot reproduces the field), so the layer table survives a `ludion dev`
/// restart.
#[test]
fn a_layered_scene_snapshot_carries_and_restores_its_layers() {
    let text = scene(LAYERED_BLOCK, "");
    let mut engine = bare_engine("layers-source");
    engine.load_inline_scene(&text).unwrap();
    engine.advance_live();
    let document = snapshot_of(&mut engine);
    let layers = &document["scene_physics"]["layers"];
    assert_eq!(layers[0]["name"], "dummy");
    assert_eq!(layers[0]["ignores"], serde_json::json!(["dummy", "sling"]));
    assert_eq!(layers[1]["name"], "sling");
    assert!(
        layers[1].get("ignores").is_none(),
        "an empty ignores list is omitted"
    );

    let mut fresh = bare_engine("layers-fresh");
    restore(&mut fresh, document.clone());
    assert_eq!(
        snapshot_of(&mut fresh)["scene_physics"],
        document["scene_physics"]
    );
}

/// Restore re-validates the layers: a snapshot hand-edited to declare a layer twice is refused at
/// the duplicate's pointer.
#[test]
fn a_snapshot_with_a_duplicate_layer_fails_restore_at_l1033() {
    let text = scene(LAYERED_BLOCK, "");
    let mut engine = bare_engine("bad-layers-source");
    engine.load_inline_scene(&text).unwrap();
    engine.advance_live();
    let mut document = snapshot_of(&mut engine);
    document["scene_physics"]["layers"][0]["ignores"] = serde_json::json!(["dummy"]);
    document["scene_physics"]["layers"][1]["name"] = Value::from("dummy");

    let mut target = bare_engine("bad-layers-target");
    target.load_inline_scene(&text).unwrap();
    let diagnostic = target
        .execute(Command::EngineRestore(EngineRestoreParams {
            snapshot: document,
        }))
        .expect_err("a duplicate layer must fail the restore");
    assert_eq!(diagnostic.code.as_str(), "L1033");
    assert_eq!(
        diagnostic.path.as_deref(),
        Some("/scene_physics/layers/1/name")
    );
}

#[test]
fn a_block_free_snapshot_has_no_scene_physics_field() {
    let mut engine = bare_engine("no-block-snapshot");
    engine.load_inline_scene(&scene("", "")).unwrap();
    for _ in 0..10 {
        engine.advance_live();
    }
    let document = snapshot_of(&mut engine);
    assert!(document.get("scene_physics").is_none());
    assert!(document.get("physics").is_none());
}

/// On frame 2, requests a switch into `physics-target.yaml`'s `floor` anchor, carrying nothing —
/// the test only cares whether the *target* district's `physics:` block steps afterward. Mirrors
/// `transition.rs`'s `request_switch`, trimmed to no carry.
fn request_switch_to_physics(world: &mut World, ctx: &SystemCtx) {
    if ctx.frame.as_u64() != 2 {
        return;
    }
    if let Some(switch) = world.resource_mut::<SceneSwitch>() {
        switch.request("physics-target.yaml", "floor", vec![]);
    }
}

/// A switch is `scene.load`'s twin: the source district has no `physics:` block, so a switch into
/// one that does must start stepping it, the same way a fresh load would.
#[test]
fn a_switch_into_a_physics_scene_steps_it() {
    let dir = fresh_dir("switch-into-physics");
    std::fs::write(
        dir.join("physics-target.yaml"),
        scene("physics:\n  space: free\n", ""),
    )
    .unwrap();

    let mut engine = Engine::builder()
        .project_root(dir)
        .system("request_switch_to_physics", request_switch_to_physics)
        .build()
        .expect("boots");
    engine.load_inline_scene(&scene("", "")).unwrap();

    engine.advance_live(); // frame 0
    engine.advance_live(); // frame 1
    engine.advance_live(); // frame 2 -> switch performed, driver resumes at frame 3

    assert!(
        engine.last_transition_error().is_none(),
        "a valid switch into a physics scene reports no error: {:?}",
        engine.last_transition_error()
    );

    let start = crate_height(&engine);
    for _ in 0..30 {
        engine.advance_live();
    }
    assert!(
        crate_height(&engine) < start,
        "a switch into a scene with a physics: block must step it afterward"
    );
}

#[test]
fn saving_a_physics_scene_keeps_the_block() {
    let mut engine = bare_engine("save-keeps-block");
    engine
        .load_inline_scene(&scene("physics:\n  space: free\n", ""))
        .unwrap();
    let text = engine.scene_text().unwrap();
    assert!(
        text.contains("physics:\n  space: free"),
        "the saved scene must keep its physics: block; saved text:\n{text}"
    );
}

/// The scene block's `gravity` reaches the solver: zero gravity keeps the crate from falling, both
/// live and after a snapshot restore into a fresh engine (mirrors
/// `restore_into_a_fresh_engine_keeps_physics`).
#[test]
fn the_blocks_gravity_reaches_the_solver_and_survives_a_restore() {
    let text = scene(
        "physics:\n  space: free\n  gravity:\n    - 0.0\n    - 0.0\n    - 0.0\n",
        "",
    );

    let mut engine = bare_engine("zero-gravity");
    engine.load_inline_scene(&text).unwrap();
    let start = crate_height(&engine);
    for _ in 0..60 {
        engine.advance_live();
    }
    assert_eq!(
        crate_height(&engine),
        start,
        "the scene's zero gravity must keep the crate from falling"
    );

    let document = snapshot_of(&mut engine);
    let mut fresh = bare_engine("zero-gravity-restored");
    restore(&mut fresh, document);
    for _ in 0..60 {
        fresh.advance_live();
    }
    assert_eq!(
        crate_height(&fresh),
        start,
        "a restored zero-gravity scene must still keep the crate from falling"
    );
}

/// A `component.set` on a live body's `Collider` is not read once and forgotten — it reaches the
/// solver through a rebuild.
#[test]
fn component_set_on_a_live_collider_reaches_the_solver() {
    let mut engine = bare_engine("component-set-collider");
    engine
        .load_inline_scene(&scene("physics:\n  space: free\n", ""))
        .unwrap();
    for _ in 0..10 {
        engine.advance_live();
    }

    let entity = engine
        .world()
        .entities()
        .find(|&entity| {
            matches!(
                engine.world().get::<ludion_engine::Name>(entity),
                Ok(Some(name)) if name.0 == "crate0"
            )
        })
        .expect("the scene authors a crate0 entity");
    let mut collider = engine
        .world()
        .get::<Collider>(entity)
        .expect("a live entity")
        .expect("its collider")
        .clone();
    let ludion_spatial::Body::Rigid(ref mut rigid) = collider.body else {
        panic!("crate0 must be a rigid body");
    };
    rigid.mass = 9.0;

    engine
        .execute(Command::ComponentSet(ComponentSetParams {
            entity,
            component: "Collider".into(),
            value: serde_json::to_value(&collider).unwrap(),
        }))
        .unwrap();
    engine.advance_live();

    assert_eq!(
        engine
            .world()
            .resource::<PhysicsWorld>()
            .expect("a claimed rigid body installs the solver")
            .mass(entity),
        Some(9.0),
        "a component.set on a live Collider must reach the solver through a rebuild"
    );
}

/// Whether a log record carries the diagnostic `code` (the structured field, not the rendered text).
fn code_is(log: &LogEvent, code: &str) -> bool {
    log.diagnostic
        .as_ref()
        .is_some_and(|d| d.code.as_str() == code)
}
