//! The read-only per-tick observer: it sees every `sim.step` tick, probes per system on request, and
//! never changes the world the ticks produce.

use std::sync::{Arc, Mutex};

use ludion_core::time::FrameIndex;
use ludion_ecs::{Registry, World};
use ludion_engine::{Engine, EngineBuilder, TickObserver};
use ludion_protocol::Command;
use ludion_protocol::command::SimStepParams;
use ludion_sim::WorldHash;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize, JsonSchema, Clone, Copy)]
struct Counter {
    n: f32,
}

fn first(world: &mut World, _ctx: &ludion_ecs::SystemCtx) {
    for (c,) in world.query::<(&mut Counter,)>() {
        c.n += 0.1;
    }
}

fn second(world: &mut World, _ctx: &ludion_ecs::SystemCtx) {
    for (c,) in world.query::<(&mut Counter,)>() {
        c.n *= 1.5;
    }
}

const SCENE: &str = "\
format: 1
seed: 5
entities:
  - name: subject
    components:
      Counter:
        n: 1.0
";

fn game() -> EngineBuilder {
    Engine::builder()
        .components(|r| r.register::<Counter>("Counter"))
        .system("first", first)
        .system("second", second)
}

fn engine() -> Engine {
    let mut engine = game().build().unwrap();
    engine.load_inline_scene(SCENE).unwrap();
    engine
}

fn step(engine: &mut Engine, ticks: u64) {
    engine
        .execute(Command::SimStep(SimStepParams { ticks, hash: false }))
        .unwrap();
}

#[derive(Default)]
struct Log {
    before: Vec<u64>,
    after: Vec<u64>,
    systems: Vec<&'static str>,
}

struct Recording {
    log: Arc<Mutex<Log>>,
    probe: bool,
}

impl TickObserver for Recording {
    fn before_tick(&mut self, frame: FrameIndex, _: &World, _: &Registry) -> bool {
        self.log.lock().unwrap().before.push(frame.as_u64());
        self.probe
    }
    fn after_system(&mut self, system: &'static str, _: &World, _: &Registry) {
        self.log.lock().unwrap().systems.push(system);
    }
    fn after_tick(&mut self, frame: FrameIndex, _: &World, _: &Registry) {
        self.log.lock().unwrap().after.push(frame.as_u64());
    }
}

fn observed(probe: bool) -> (Engine, Arc<Mutex<Log>>) {
    let log = Arc::new(Mutex::new(Log::default()));
    let mut engine = engine();
    engine.set_tick_observer(Recording {
        log: log.clone(),
        probe,
    });
    (engine, log)
}

#[test]
fn n_steps_give_n_after_tick_calls_and_no_before_tick_on_the_idle_call() {
    let (mut engine, log) = observed(false);
    step(&mut engine, 4);
    let log = log.lock().unwrap();
    assert_eq!(log.after, vec![1, 2, 3, 4]);
    assert_eq!(
        log.before,
        vec![0, 1, 2, 3],
        "the final non-ticking advance_live call must not announce a tick"
    );
    assert!(
        log.systems.is_empty(),
        "an unprobed tick reports no systems"
    );
}

#[test]
fn the_world_hash_is_the_same_with_no_observer_a_watching_one_and_a_probing_one() {
    let hash = |engine: &mut Engine| -> WorldHash {
        step(engine, 6);
        engine.world_hash().unwrap()
    };
    let plain = hash(&mut engine());
    let watching = hash(&mut observed(false).0);
    let probing = hash(&mut observed(true).0);
    assert_eq!(plain, watching);
    assert_eq!(plain, probing);
}

#[test]
fn a_probed_tick_reports_every_system_in_registration_order() {
    let (mut engine, log) = observed(true);
    step(&mut engine, 2);
    assert_eq!(
        log.lock().unwrap().systems,
        vec![
            "path_follow",
            "first",
            "second",
            "camera_rig",
            "path_follow",
            "first",
            "second",
            "camera_rig"
        ]
    );
}

#[test]
fn a_cleared_observer_sees_nothing() {
    let (mut engine, log) = observed(false);
    engine.clear_tick_observer();
    step(&mut engine, 2);
    assert!(log.lock().unwrap().after.is_empty());
}
