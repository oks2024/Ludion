//! `EngineBuilder` tests: a game's components register on top of the built-ins, and — the bug this
//! fixes — a configured schedule *survives* `scene.load` and `engine.restore` instead of being
//! silently replaced with an empty one.

use ludion_core::codes::code;
use ludion_ecs::{Registry, SystemCtx, World};
use ludion_engine::{Engine, EngineBuilder};
use ludion_protocol::command::{EngineRestoreParams, SimStepParams};
use ludion_protocol::{Command, ResponseBody};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// A tiny game component whose only job is to be advanced by a system, so a test can observe whether
/// the schedule actually ran.
#[derive(Serialize, Deserialize, JsonSchema, Clone, Copy, Debug, PartialEq)]
struct Ticks {
    n: u32,
}

/// The game system: bump every `Ticks` component once per tick.
fn count(world: &mut World, _ctx: &SystemCtx) {
    for (t,) in world.query::<(&mut Ticks,)>() {
        t.n += 1;
    }
}

/// A builder for a game that registers `Ticks` and runs `count` — fresh each call.
fn game() -> EngineBuilder {
    Engine::builder()
        .components(|reg| reg.register::<Ticks>("Ticks"))
        .system("count", count)
}

/// A single-`Ticks`-entity scene naming a game component (a bare engine would reject it, `L1013`).
const SCENE: &str = "format: 1\nentities:\n  - components:\n      Ticks:\n        n: 0\n";

fn ticks_n(engine: &mut Engine) -> u32 {
    let mut n = 0;
    for (t,) in engine.world_mut().query::<(&Ticks,)>() {
        n = t.n;
    }
    n
}

fn step(engine: &mut Engine, ticks: u64) {
    engine
        .execute(Command::SimStep(SimStepParams { ticks, hash: false }))
        .expect("sim steps");
}

#[test]
fn game_components_register_on_top_of_the_builtins() {
    let engine = game().build().expect("the game builds");
    assert!(
        engine.registry().get("Ticks").is_some(),
        "the game component is registered"
    );
    assert!(
        engine.registry().get("Transform").is_some(),
        "the built-in visual components are still present"
    );
}

#[test]
fn a_name_colliding_with_a_builtin_is_a_structured_error() {
    #[derive(Serialize, Deserialize, JsonSchema)]
    struct NotATransform;
    let err = match Engine::builder()
        .components(|reg| reg.register::<NotATransform>("Transform"))
        .build()
    {
        Ok(_) => panic!("registering over a built-in name must fail"),
        Err(err) => err,
    };
    assert_eq!(err.code, code::DUPLICATE_COMPONENT_REGISTRATION);
}

#[test]
fn the_schedule_survives_scene_load() {
    let mut engine = game().build().expect("the game builds");
    // load_inline_scene rebuilds the world (and, before the fix, dropped the schedule).
    engine
        .load_inline_scene(SCENE)
        .expect("the scene loads against the game registry");
    step(&mut engine, 5);
    assert_eq!(
        ticks_n(&mut engine),
        5,
        "the configured system must still run after scene.load"
    );
}

#[test]
fn the_schedule_survives_snapshot_restore() {
    let mut e1 = game().build().expect("the game builds");
    e1.load_inline_scene(SCENE).expect("scene loads");
    step(&mut e1, 5);
    assert_eq!(ticks_n(&mut e1), 5);

    let snapshot = match e1.execute(Command::EngineSnapshot).unwrap().body {
        ResponseBody::Snapshot { snapshot } => snapshot,
        other => panic!("expected a snapshot body, got {other:?}"),
    };

    // Restore into a second engine built from the same game config (a bare engine could neither
    // deserialize `Ticks` nor run `count`).
    let mut e2 = game().build().expect("the game builds");
    match e2
        .execute(Command::EngineRestore(EngineRestoreParams { snapshot }))
        .unwrap()
        .body
    {
        // Same build both sides: a clean migration with an empty report.
        ResponseBody::Restored { report } => assert!(report.is_clean(), "{report:?}"),
        other => panic!("expected a restored body, got {other:?}"),
    }
    assert_eq!(ticks_n(&mut e2), 5, "restore reproduces the world");

    // The load-bearing assertion: the system still runs after restore. Before the fix, restore
    // installed an empty schedule and this stayed at 5.
    step(&mut e2, 5);
    assert_eq!(
        ticks_n(&mut e2),
        10,
        "the configured system must still run after engine.restore"
    );
}

/// Boot-time `L1124` schedule-module-concentration warning: the engine groups registered systems by
/// their definition module (from `std::any::type_name`) and emits one warning per module past the
/// 12-system threshold. The two nested modules below give two *distinct* real module paths, so the
/// `type_name` grouping is exercised for real (not mocked). A warning is stored, never fatal — the
/// engines here all build — and asserted through `Engine::boot_warnings`.
mod schedule_concentration {
    use super::*;

    mod alpha {
        use ludion_ecs::{SystemCtx, World};
        /// A no-op system defined in module `alpha`; every registration of it groups under `…::alpha`.
        pub fn noop(_world: &mut World, _ctx: &SystemCtx) {}
    }

    mod beta {
        use ludion_ecs::{SystemCtx, World};
        /// A no-op system in a *second* module, so it groups distinctly from `alpha::noop`.
        pub fn noop(_world: &mut World, _ctx: &SystemCtx) {}
    }

    /// Registers `run` onto `builder` `count` times. Grouping keys on the function's definition module
    /// (its `type_name`), not the call site, so repeatedly registering one module's `fn` accumulates
    /// that module's system count — the concentration the warning detects.
    fn with_systems<F>(builder: EngineBuilder, count: usize, run: F) -> EngineBuilder
    where
        F: Fn(&mut World, &SystemCtx) + Copy + Send + Sync + 'static,
    {
        let mut builder = builder;
        for _ in 0..count {
            // System names are unique per schedule; the leak is a few bytes in a test.
            let name: &'static str =
                Box::leak(format!("noop_{}", builder.system_names().len()).into_boxed_str());
            builder = builder.system(name, run);
        }
        builder
    }

    #[test]
    fn thirteen_systems_in_one_module_warn_once() {
        let engine = with_systems(Engine::builder(), 13, alpha::noop)
            .build()
            .expect("the engine builds — the warning is non-fatal");
        let warnings = engine.boot_warnings();
        assert_eq!(
            warnings.len(),
            1,
            "one module past the threshold yields exactly one warning",
        );
        let warning = &warnings[0];
        assert_eq!(warning.code, code::SCHEDULE_MODULE_CONCENTRATION);
        assert!(
            warning.message.contains("alpha"),
            "the warning names the offending module: {}",
            warning.message,
        );
        assert!(
            warning.message.contains("13"),
            "the warning names the module's system count: {}",
            warning.message,
        );
        assert!(
            warning.path.is_some(),
            "the warning points at the module's first wiring site",
        );
    }

    #[test]
    fn twelve_systems_in_one_module_are_silent() {
        let engine = with_systems(Engine::builder(), 12, alpha::noop)
            .build()
            .expect("the engine builds");
        assert!(
            engine.boot_warnings().is_empty(),
            "12 is at the threshold, not past it — no warning",
        );
    }

    #[test]
    fn seven_plus_six_across_two_modules_are_silent() {
        let builder = with_systems(Engine::builder(), 7, alpha::noop);
        let engine = with_systems(builder, 6, beta::noop)
            .build()
            .expect("the engine builds");
        assert!(
            engine.boot_warnings().is_empty(),
            "13 systems total, but no single module is over threshold — no warning",
        );
    }

    #[test]
    fn no_systems_no_warning() {
        let engine = Engine::builder().build().expect("the engine builds");
        assert!(
            engine.boot_warnings().is_empty(),
            "a schedule with no systems has no module to over-concentrate",
        );
    }
}

/// The registry seam is enough for a whole game registry, not just one component.
#[test]
fn registry_helper_pattern_layers_cleanly() {
    fn registry() -> Registry {
        let mut reg = ludion_engine::builtin_registry();
        reg.register::<Ticks>("Ticks").unwrap();
        reg
    }
    let reg = registry();
    assert_eq!(reg.len(), ludion_engine::builtin_registry().len() + 1);
    assert!(reg.get("Ticks").is_some());
}
