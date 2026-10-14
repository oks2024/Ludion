//! The query-drift verifier (`check_query_drift_with`): an instrumented run's observed query shapes
//! are compared against each system's declared ones — observed-but-undeclared is an `L1126` error,
//! declared-but-never-observed is an `L1127` warning.

use ludion_engine::{Engine, check_query_drift_with};
use ludion_protocol::Command;
use ludion_protocol::command::SimStepParams;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize, JsonSchema, Clone, Copy)]
struct Health {
    hp: f32,
}
#[derive(Serialize, Deserialize, JsonSchema, Clone, Copy)]
struct Mana {
    mp: f32,
}

fn regen(world: &mut ludion_ecs::World, _ctx: &ludion_ecs::SystemCtx) {
    for (h,) in world.query::<(&mut Health,)>() {
        h.hp += 1.0;
    }
}

/// A system whose body runs a query it does NOT declare (it declares `&mut Health` but runs
/// `&mut Mana` too).
fn under_declared(world: &mut ludion_ecs::World, _ctx: &ludion_ecs::SystemCtx) {
    for (h,) in world.query::<(&mut Health,)>() {
        h.hp += 1.0;
    }
    for (m,) in world.query::<(&mut Mana,)>() {
        m.mp += 1.0;
    }
}

/// A system in the snapshot-then-accessor idiom the `&mut self` borrow model forces: it declares
/// (and runs) `&mut Health` as a typed query, then reaches for `Mana` per entity — a write the
/// declaration does not name.
fn per_entity_under_declared(world: &mut ludion_ecs::World, _ctx: &ludion_ecs::SystemCtx) {
    let ids: Vec<_> = world
        .query_ids::<(&mut Health,)>()
        .map(|(id, _)| id)
        .collect();
    for id in ids {
        if let Ok(Some(mana)) = world.get_mut::<Mana>(id) {
            mana.mp += 1.0;
        }
    }
}

const SCENE: &str = "\
format: 1
entities:
  - components:
      Health:
        hp: 1.0
      Mana:
        mp: 1.0
";

fn build(system: &'static str, run: fn(&mut ludion_ecs::World, &ludion_ecs::SystemCtx)) -> Engine {
    let mut engine = Engine::builder()
        .components(|r| {
            r.register::<Health>("Health")?;
            r.register::<Mana>("Mana")
        })
        .system(system, run)
        .query::<(&mut Health,)>()
        .build()
        .unwrap();
    engine.load_inline_scene(SCENE).unwrap();
    engine
}

fn step(engine: &mut Engine) -> Result<(), ludion_core::diagnostics::Diagnostic> {
    engine
        .execute(Command::SimStep(SimStepParams {
            ticks: 2,
            hash: false,
        }))
        .map(|_| ())
}

#[test]
fn declared_query_that_runs_is_clean() {
    let mut engine = build("regen", regen);
    let report = check_query_drift_with(&mut engine, step).unwrap();
    assert!(report.is_clean(), "{report}");
    assert!(report.errors().is_empty());
    assert!(report.warnings().is_empty());
}

#[test]
fn observed_but_undeclared_is_an_error() {
    let mut engine = build("under_declared", under_declared);
    let report = check_query_drift_with(&mut engine, step).unwrap();
    assert!(
        !report.is_clean(),
        "the undeclared &mut Mana query must be an error"
    );
    assert_eq!(report.errors().len(), 1);
    let err = &report.errors()[0];
    assert_eq!(err.code.as_str(), "L1126");
    assert!(err.message.contains("under_declared"), "{}", err.message);
    assert!(err.message.contains("Mana"), "{}", err.message);
}

/// A component the engine's registry never learns about.
#[derive(Serialize, Deserialize, JsonSchema, Clone, Copy)]
struct Scratch {
    n: u32,
}

fn undeclared_unregistered(world: &mut ludion_ecs::World, _ctx: &ludion_ecs::SystemCtx) {
    for (h,) in world.query::<(&mut Health,)>() {
        h.hp += 1.0;
    }
    for (s,) in world.query::<(&mut Scratch,)>() {
        s.n += 1;
    }
}

#[test]
fn an_unregistered_type_in_an_undeclared_shape_prints_its_rust_name() {
    let mut engine = build("undeclared_unregistered", undeclared_unregistered);
    let report = check_query_drift_with(&mut engine, step).unwrap();
    assert_eq!(report.errors().len(), 1, "{report}");
    let err = &report.errors()[0];
    assert_eq!(err.code.as_str(), "L1126");
    assert!(err.message.contains("Scratch"), "{}", err.message);
    assert!(!err.message.contains("TypeId("), "{}", err.message);
}

#[test]
fn per_entity_accessor_is_observed_like_a_query() {
    let mut engine = build("per_entity_under_declared", per_entity_under_declared);
    let report = check_query_drift_with(&mut engine, step).unwrap();
    assert_eq!(report.errors().len(), 1, "{report}");
    let err = &report.errors()[0];
    assert_eq!(err.code.as_str(), "L1126");
    assert!(err.message.contains("(&mut Mana)"), "{}", err.message);
}

#[test]
fn host_side_access_outside_a_system_is_attributed_to_nobody() {
    // Loading a scene and hashing the world read and write components through the same accessors a
    // system uses; neither is a system's access, so neither may land in a system's observed set.
    let mut engine = build("regen", regen);
    let report = check_query_drift_with(&mut engine, |e| {
        e.load_inline_scene(SCENE).unwrap();
        step(e)
    })
    .unwrap();
    assert!(report.is_clean(), "{report}");
    assert!(report.warnings().is_empty(), "{report}");
}

#[test]
fn declared_but_never_observed_is_a_warning() {
    // The system declares `&mut Health` but its body never queries — so the declared shape is
    // unobserved (a warning), and nothing is undeclared (clean of errors).
    fn inert(_world: &mut ludion_ecs::World, _ctx: &ludion_ecs::SystemCtx) {}
    let mut engine = build("inert", inert);
    let report = check_query_drift_with(&mut engine, step).unwrap();
    assert!(
        report.is_clean(),
        "no undeclared queries → no errors: {report}"
    );
    assert_eq!(report.warnings().len(), 1);
    assert_eq!(report.warnings()[0].code.as_str(), "L1127");
    assert!(report.warnings()[0].message.contains("Health"));
}
