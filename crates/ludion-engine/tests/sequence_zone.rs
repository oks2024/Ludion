//! A sequence camera key holds the rig against camera zones until its `restore`, which hands the
//! camera back to the zone the target stands in (`ludion docs sequences`, `Plan/sequencer.md` d35).

use std::path::PathBuf;

use ludion_core::ids::EntityId;
use ludion_engine::sequences;
use ludion_engine::{CameraRig, Engine, Name, RigMode, Transform, sequence_update};

const PHYSICS: &str = "physics:\n  space: free\n  layers:\n    - name: wall\n";

/// A chase rig 5 m behind a hero at x = 0.
fn scene() -> String {
    let mut text = format!(
        "format: 1\nseed: 3\n{PHYSICS}entities:\n  - name: cam\n    components:\n      Transform: {{}}\n      Camera: {{}}\n      CameraRig:\n        target:\n          index: 1\n          generation: 0\n        rig:\n          mode:\n            chase:\n              distance: 5.0\n  - name: hero\n    components:\n      Transform:\n        translation:\n          - 0.0\n          - 0.0\n          - 0.0\n"
    );
    text += &zone("first", 5.0, 15.0, 8.0);
    text += &zone("second", 20.0, 30.0, 3.0);
    text
}

/// A sensor box covering x in `[from, to]` whose rig is a chase at `distance`.
fn zone(name: &str, from: f32, to: f32, distance: f32) -> String {
    format!(
        "  - name: {name}\n    components:\n      Transform:\n        translation:\n          - {:?}\n          - 0.0\n          - 0.0\n      Collider:\n        shape:\n          box:\n            half_extents:\n              - {:?}\n              - 5.0\n              - 5.0\n        body: static\n        sensor: true\n      CameraZone:\n        priority: 1\n        rig:\n          mode:\n            chase:\n              distance: {distance:?}\n",
        (from + to) / 2.0,
        (to - from) / 2.0,
    )
}

fn beat(restore_at: u32) -> String {
    format!(
        "format: 1\nkind: sequence\nroles:\n  - cam\nlength_ticks: 400\ntracks:\n  camera:\n    - at: 2\n      role: cam\n      rig:\n        mode:\n          chase:\n            distance: 20.0\n    - at: {restore_at}\n      role: cam\n      restore: true\n"
    )
}

const KEY: &str = "sequences/zone.yaml";

fn boot(tag: &str, restore_at: u32) -> Engine {
    let dir: PathBuf =
        std::env::temp_dir().join(format!("ludion-seq-zone-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let mut engine = Engine::builder()
        .project_root(dir)
        .sequences()
        .system("sequence_update", sequence_update)
        .doc_text(KEY, &beat(restore_at))
        .build()
        .expect("the game builds");
    engine.load_inline_scene(&scene()).expect("the scene loads");
    engine
}

fn named(engine: &Engine, wanted: &str) -> EntityId {
    let world = engine.world();
    world
        .entities()
        .find(|&id| {
            world
                .get::<Name>(id)
                .ok()
                .flatten()
                .is_some_and(|name| name.0 == wanted)
        })
        .unwrap_or_else(|| panic!("the scene has a `{wanted}`"))
}

fn rig_of(engine: &Engine) -> CameraRig {
    let cam = named(engine, "cam");
    engine
        .world()
        .get::<CameraRig>(cam)
        .unwrap()
        .unwrap()
        .clone()
}

fn chase_distance(rig: &CameraRig) -> f32 {
    match &rig.rig.mode {
        RigMode::Chase(chase) => chase.distance,
        other => panic!("expected a chase rig, got {other:?}"),
    }
}

fn hero_to(engine: &mut Engine, x: f32) {
    let hero = named(engine, "hero");
    engine
        .world_mut()
        .get_mut::<Transform>(hero)
        .unwrap()
        .unwrap()
        .translation = [x, 0.0, 0.0];
}

fn advance(engine: &mut Engine, ticks: usize) {
    for _ in 0..ticks {
        engine.advance_live();
    }
}

fn start(engine: &mut Engine) -> sequences::SequenceHandle {
    let cam = named(engine, "cam");
    sequences::play(engine.world_mut(), KEY, &[("cam", cam)]).unwrap()
}

#[test]
fn a_sequence_rig_key_holds_the_rig_against_every_zone() {
    let mut engine = boot("hold", 300);
    hero_to(&mut engine, 10.0);
    advance(&mut engine, 5);
    let zone = rig_of(&engine).state.zone;
    assert_eq!(zone, Some(named(&engine, "first")));
    start(&mut engine);
    advance(&mut engine, 10);
    assert_eq!(chase_distance(&rig_of(&engine)), 20.0);
    hero_to(&mut engine, 25.0);
    advance(&mut engine, 10);
    let rig = rig_of(&engine);
    assert_eq!(
        chase_distance(&rig),
        20.0,
        "the second zone does not take over"
    );
    assert_eq!(
        rig.state.zone, zone,
        "the zone state stays as the override found it"
    );
}

#[test]
fn restore_inside_a_zone_hands_back_to_the_zone_then_the_base() {
    let mut engine = boot("inside", 20);
    hero_to(&mut engine, 10.0);
    advance(&mut engine, 5);
    start(&mut engine);
    advance(&mut engine, 25);
    assert_eq!(
        chase_distance(&rig_of(&engine)),
        8.0,
        "the zone's rig is back"
    );
    hero_to(&mut engine, 0.0);
    advance(&mut engine, 200);
    let rig = rig_of(&engine);
    assert_eq!(
        chase_distance(&rig),
        5.0,
        "leaving the zone restores the base, not the zone's rig"
    );
    assert!(rig.state.zone.is_none());
}

#[test]
fn restore_after_walking_into_a_zone_enters_it_then_leaves_to_the_pre_sequence_rig() {
    let mut engine = boot("walked", 40);
    start(&mut engine);
    advance(&mut engine, 10);
    assert_eq!(chase_distance(&rig_of(&engine)), 20.0);
    hero_to(&mut engine, 10.0);
    advance(&mut engine, 10);
    assert_eq!(
        chase_distance(&rig_of(&engine)),
        20.0,
        "held while the key lasts"
    );
    advance(&mut engine, 25);
    assert_eq!(chase_distance(&rig_of(&engine)), 8.0);
    hero_to(&mut engine, 0.0);
    advance(&mut engine, 200);
    assert_eq!(chase_distance(&rig_of(&engine)), 5.0);
}

#[test]
fn a_skip_with_a_pending_restore_inside_a_zone_ends_on_the_zone_rig_with_a_snap() {
    let mut engine = boot("skip", 300);
    hero_to(&mut engine, 10.0);
    advance(&mut engine, 5);
    let handle = start(&mut engine);
    advance(&mut engine, 10);
    assert!(sequences::skip(engine.world_mut(), handle));
    advance(&mut engine, 2);
    let rig = rig_of(&engine);
    assert_eq!(chase_distance(&rig), 8.0);
    assert_eq!(
        rig.state.last_applied.as_ref(),
        Some(&rig.rig),
        "snapped onto the zone's rig"
    );
}
