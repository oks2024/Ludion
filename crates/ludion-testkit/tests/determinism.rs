//! `assert_deterministic` over engines whose divergence the test controls with a per-test `static`
//! toggled inside `build()` — the harness alternates A, B, A, B, …, so each side reproduces.

use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};

use ludion_ecs::{SystemCtx, World};
use ludion_engine::{Engine, EngineBuilder};
use ludion_testkit::{assert_deterministic, loaded_engine, step};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize, JsonSchema, Clone, Copy)]
struct Gauge {
    level: f32,
    other: f32,
}

const SCENE: &str = "\
format: 1
seed: 3
entities:
  - name: subject
    components:
      Gauge:
        level: 0.0
        other: 0.0
";

fn steady(world: &mut World, _ctx: &SystemCtx) {
    for (g,) in world.query::<(&mut Gauge,)>() {
        g.other += 1.0;
    }
}

fn game(drift: fn(&mut World, &SystemCtx)) -> EngineBuilder {
    Engine::builder()
        .components(|r| r.register::<Gauge>("Gauge"))
        .system("steady", steady)
        .system("drift", drift)
}

fn no_drift(_: &mut World, _: &SystemCtx) {}

fn panic_text(f: impl FnOnce()) -> String {
    let payload = catch_unwind(AssertUnwindSafe(f)).expect_err("the assertion must panic");
    match payload.downcast::<String>() {
        Ok(s) => *s,
        Err(p) => (*p.downcast::<&str>().expect("a string panic")).to_string(),
    }
}

static FLIP: AtomicBool = AtomicBool::new(false);

fn drift_when_flipped(world: &mut World, ctx: &SystemCtx) {
    if FLIP.load(Ordering::SeqCst) && ctx.frame.as_u64() >= 2 {
        for (g,) in world.query::<(&mut Gauge,)>() {
            g.level += 0.5;
        }
    }
}

#[test]
fn a_divergence_names_the_frame_system_component_and_field() {
    // Only the second `build()` turns the drift on, so run B alone adds to `level` from the third tick.
    let builds = AtomicU32::new(0);
    let message = panic_text(|| {
        assert_deterministic(
            || {
                FLIP.store(
                    builds.fetch_add(1, Ordering::SeqCst) % 2 == 1,
                    Ordering::SeqCst,
                );
                loaded_engine(game(drift_when_flipped), SCENE)
            },
            |engine| step(engine, 6),
        );
    });
    assert!(message.contains("frame 3"), "{message}");
    assert!(message.contains("`drift`"), "{message}");
    assert!(message.contains("Gauge"), "{message}");
    assert!(message.contains("fields: level"), "{message}");
}

#[test]
fn a_deterministic_body_with_edits_between_steps_passes() {
    assert_deterministic(
        || loaded_engine(game(no_drift), SCENE),
        |engine| {
            step(engine, 3);
            for (g,) in engine.world_mut().query::<(&mut Gauge,)>() {
                g.level = 9.0;
            }
            step(engine, 3);
        },
    );
}

#[test]
fn a_run_that_stops_early_is_its_own_failure() {
    let calls = AtomicU32::new(0);
    let message = panic_text(|| {
        assert_deterministic(
            || loaded_engine(game(no_drift), SCENE),
            |engine| {
                let ticks = if calls.fetch_add(1, Ordering::SeqCst) == 0 {
                    10
                } else {
                    8
                };
                step(engine, ticks);
            },
        );
    });
    assert!(message.contains("run B stopped at frame 8"), "{message}");
    assert!(!message.contains("diverge"), "{message}");
}

#[test]
fn an_edit_that_differs_per_run_before_a_step_is_reported_as_pre_tick() {
    let calls = AtomicU32::new(0);
    let message = panic_text(|| {
        assert_deterministic(
            || loaded_engine(game(no_drift), SCENE),
            |engine| {
                step(engine, 2);
                let bump = (calls.fetch_add(1, Ordering::SeqCst) % 2) as f32;
                for (g,) in engine.world_mut().query::<(&mut Gauge,)>() {
                    g.level += bump;
                }
                step(engine, 2);
            },
        );
    });
    assert!(
        message.contains("already differed as the tick began"),
        "{message}"
    );
    assert!(!message.contains("first differing system"), "{message}");
    assert!(message.contains("fields: level"), "{message}");
}

fn extra_bump(world: &mut World, _: &SystemCtx) {
    for (g,) in world.query::<(&mut Gauge,)>() {
        g.level += 1.0;
    }
}

#[test]
fn a_system_that_runs_in_only_one_run_is_named_as_the_extra() {
    // Only the second `build()` registers `extra_bump`, so run B's schedule is one system longer.
    let builds = AtomicU32::new(0);
    let message = panic_text(|| {
        assert_deterministic(
            || {
                let builder = game(no_drift);
                let builder = if builds.fetch_add(1, Ordering::SeqCst) % 2 == 1 {
                    builder.system("extra_bump", extra_bump)
                } else {
                    builder
                };
                loaded_engine(builder, SCENE)
            },
            |engine| step(engine, 3),
        );
    });
    assert!(message.contains("`extra_bump`"), "{message}");
    assert!(message.contains("fields: level"), "{message}");
}
