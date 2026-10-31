//! `GameplayDriver` over any engine: scene files, engines in hand, typed input, `run_until`, and
//! `snapshot`/`restore`. Scratch lives under the cargo target tmp dir.

use ludion_engine::{DocStore, Engine, EngineBuilder, Transform};
use ludion_sim::{GamepadAxis, InputEvent, InputState, Key};
use ludion_testkit::{GameplayDriver, inject, loaded_engine, step};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

const EMPTY: &str = "format: 1\nentities: []\n";

#[derive(Serialize, Deserialize, JsonSchema, Clone, Debug, PartialEq)]
struct Weapon {
    damage: f32,
    cooldown: f32,
}

fn scratch(name: &str) -> PathBuf {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(format!("driver-{name}"));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn world_hash(engine: &mut Engine) -> u64 {
    engine.world_hash().unwrap().as_u64()
}

#[test]
fn from_scene_file_resolves_data_docs_relative_to_the_scene() {
    let dir = scratch("scene-file");
    let level = dir.join("level");
    std::fs::create_dir_all(level.join("weapons")).unwrap();
    std::fs::write(level.join("scene.yaml"), EMPTY).unwrap();
    std::fs::write(
        level.join("weapons/axe.yaml"),
        "format: 1\nkind: weapon\ndamage: 30.0\ncooldown: 1.5\n",
    )
    .unwrap();

    let driver = GameplayDriver::from_scene_file(
        Engine::builder().doc_kind::<Weapon>("weapon"),
        &level.join("scene.yaml"),
    );
    let store = driver
        .engine()
        .world()
        .resource::<DocStore<Weapon>>()
        .expect("the weapon store is installed");
    assert!(
        store.iter().any(|(key, _)| key == "weapons/axe.yaml"),
        "the axe doc resolves against the scene's directory"
    );
}

#[test]
fn from_engine_picks_up_a_stepped_frame_and_input_lands() {
    let mut engine = loaded_engine(EngineBuilder::new(), EMPTY);
    step(&mut engine, 7);
    let mut driver = GameplayDriver::from_engine(engine);
    assert_eq!(driver.frame(), 7);

    driver.inject_event(InputEvent::KeyDown { key: Key::Right });
    driver.step(1);
    let held = driver
        .engine_mut()
        .world_mut()
        .resource::<InputState>()
        .unwrap()
        .is_held(Key::Right);
    assert!(held, "the key injected at the cursor is held next tick");
    assert_eq!(driver.frame(), 8);
}

#[test]
fn free_inject_targets_the_next_tick() {
    let mut engine = loaded_engine(EngineBuilder::new(), EMPTY);
    step(&mut engine, 2);
    inject(&mut engine, InputEvent::KeyDown { key: Key::Right });
    step(&mut engine, 1);
    let held = engine
        .world_mut()
        .resource::<InputState>()
        .unwrap()
        .is_held(Key::Right);
    assert!(held);
}

const AWKWARD: u32 = 0x3E99_999A;

fn axis_run() -> (u32, u64) {
    let mut driver = GameplayDriver::new(EngineBuilder::new(), EMPTY);
    driver.set_axis(GamepadAxis::LeftStickX, f32::from_bits(AWKWARD));
    driver.set_axis(GamepadAxis::LeftStickY, -f32::from_bits(0x3F11_1111));
    driver.step(1);
    let x = driver
        .engine_mut()
        .world_mut()
        .resource::<InputState>()
        .unwrap()
        .gamepad()
        .left_stick()[0];
    let hash = world_hash(driver.engine_mut());
    (x.to_bits(), hash)
}

#[test]
fn set_axis_round_trips_bit_exact_and_replays_identically() {
    let (bits, hash) = axis_run();
    assert_eq!(
        bits, AWKWARD,
        "the analog value survives the wire bit-exact"
    );
    assert_eq!(axis_run(), (bits, hash), "two identical runs hash equal");
}

/// A shared in-memory recording sink.
#[derive(Clone, Default)]
struct Sink(std::sync::Arc<std::sync::Mutex<Vec<u8>>>);

impl std::io::Write for Sink {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(buf);
        Ok(buf.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

#[test]
fn a_recorded_axis_run_replays_to_the_live_hash_trace() {
    let sink = Sink::default();
    let mut driver = GameplayDriver::new(EngineBuilder::new(), EMPTY);
    driver.engine_mut().set_recording(sink.clone()).unwrap();
    driver.set_axis(GamepadAxis::LeftStickX, f32::from_bits(AWKWARD));
    driver.set_axis(GamepadAxis::LeftStickY, -f32::from_bits(0x3F11_1111));
    let mut live = Vec::new();
    for _ in 0..3 {
        driver.step(1);
        live.push((driver.frame(), world_hash(driver.engine_mut())));
    }
    driver.engine_mut().stop_recording().unwrap();
    let bytes = sink.0.lock().unwrap().clone();

    let replayed: Vec<(u64, u64)> = ludion_engine::replay_hashes(&bytes, ".")
        .expect("the recording replays")
        .into_iter()
        .map(|(frame, hash)| (frame.as_u64(), hash.as_u64()))
        .collect();
    for entry in &live {
        assert!(
            replayed.contains(entry),
            "live (frame, hash) {entry:?} is absent from the replay trace {replayed:?}"
        );
    }
}

#[test]
fn the_frame_follows_the_engine_when_it_is_stepped_outside_the_driver() {
    let mut driver = GameplayDriver::new(EngineBuilder::new(), EMPTY);
    ludion_testkit::step(driver.engine_mut(), 3);
    assert_eq!(driver.frame(), 3);
    assert_eq!(driver.frame(), driver.engine().frame().as_u64());
}

const ONE_ENTITY: &str = "format: 1
entities:
  - components:
      Transform:
        translation:
          - 1.0
          - 2.0
          - 3.0
";

fn nudge(engine: &mut Engine) {
    let world = engine.world_mut();
    let e = world.entities().next().expect("the scene has an entity");
    world.get_mut::<Transform>(e).unwrap().unwrap().translation[0] += 5.0;
}

#[test]
fn snapshot_and_restore_round_trip_state_and_cursor() {
    let mut driver = GameplayDriver::new(EngineBuilder::new(), ONE_ENTITY);
    driver.step(5);
    let snapshot = driver.snapshot();
    let frame = driver.frame();
    let hash = world_hash(driver.engine_mut());

    driver.step(9);
    nudge(driver.engine_mut());
    assert_ne!(driver.frame(), frame);
    assert_ne!(world_hash(driver.engine_mut()), hash, "the world moved");
    driver.restore(snapshot);
    assert_eq!(driver.frame(), frame);
    assert_eq!(world_hash(driver.engine_mut()), hash);
}

#[test]
fn run_until_returns_the_first_matching_frame_or_none() {
    let mut driver = GameplayDriver::new(EngineBuilder::new(), EMPTY);
    assert_eq!(driver.run_until(20, |e| e.frame().as_u64() >= 6), Some(6));
    assert_eq!(driver.frame(), 6);
    assert_eq!(driver.run_until(4, |_| false), None);
    assert_eq!(driver.frame(), 10);
}
