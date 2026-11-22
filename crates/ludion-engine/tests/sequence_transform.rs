//! The sequence transform track (`ludion docs sequences`): `to` spans relative to an anchor, `path`
//! spans, replace-on-same-role, skip equivalence, snapshot/restore and degradation.

use ludion_core::codes::code;
use ludion_core::ids::EntityId;
use ludion_core::math::{euler_xyz_degrees_to_quat, quat_mul, quat_rotate};
use ludion_ecs::Parent;
use ludion_engine::hierarchy::world_transform;
use ludion_engine::sequences::{self, SequenceEvent, SequenceHandle, SequenceTrack};
use ludion_engine::{Diagnostics, Engine, SequenceEvents, Transform, sequence_update};
use ludion_protocol::command::{EngineRestoreParams, SimStepParams};
use ludion_protocol::{Command, ResponseBody};
use serde_json::Value;
use std::path::PathBuf;

const SEQ: &str = "sequences/t.yaml";
const PATH: &str = "paths/walk.path.yaml";
const WALK: &str = "format: 1
kind: path
interpolation: linear
closed: false
points:
  - - 0.0
    - 0.0
    - 0.0
  - - 10.0
    - 0.0
    - 0.0
  - - 10.0
    - 0.0
    - -10.0
";

fn fresh_dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "ludion-sequence-transform-{}-{name}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("paths")).unwrap();
    std::fs::write(dir.join(PATH), WALK).unwrap();
    dir
}

fn doc(tracks: &str, length: u32) -> String {
    format!(
        "format: 1
kind: sequence
roles:
  - hero
  - door
  - walker
length_ticks: {length}
tracks:
  transform:
{tracks}"
    )
}

struct Stage {
    engine: Engine,
    hero: EntityId,
    door: EntityId,
    walker: EntityId,
}

fn at(translation: [f32; 3], yaw: f32) -> Transform {
    Transform {
        translation,
        rotation: [0.0, yaw, 0.0],
        scale: [1.0; 3],
    }
}

fn stage(name: &str, sequence: &str) -> Stage {
    let mut engine = Engine::builder()
        .project_root(fresh_dir(name))
        .sequences()
        .system("sequence_update", sequence_update)
        .doc_text(SEQ, sequence)
        .build()
        .expect("the docs are valid");
    engine
        .load_inline_scene("format: 1\nseed: 1\nentities: []\n")
        .unwrap();
    let world = engine.world_mut();
    let (hero, door, walker) = (world.spawn(), world.spawn(), world.spawn());
    world.insert(hero, at([0.0; 3], 0.0)).unwrap();
    world.insert(door, at([10.0, 0.0, 0.0], 90.0)).unwrap();
    world.insert(walker, at([0.0; 3], 0.0)).unwrap();
    Stage {
        engine,
        hero,
        door,
        walker,
    }
}

fn play(stage: &mut Stage) -> SequenceHandle {
    let (hero, door, walker) = (stage.hero, stage.door, stage.walker);
    sequences::play(
        stage.engine.world_mut(),
        SEQ,
        &[("hero", hero), ("door", door), ("walker", walker)],
    )
    .unwrap()
}

fn tick(engine: &mut Engine) -> Vec<SequenceEvent> {
    engine
        .execute(Command::SimStep(SimStepParams {
            ticks: 1,
            hash: false,
        }))
        .unwrap();
    engine
        .world()
        .resource::<SequenceEvents>()
        .unwrap()
        .iter()
        .cloned()
        .collect()
}

fn pose(engine: &Engine, id: EntityId) -> Transform {
    *engine.world().get::<Transform>(id).unwrap().unwrap()
}

fn world_pose(engine: &Engine, id: EntityId) -> ludion_core::math::Trs {
    world_transform(engine.world(), id).unwrap().trs
}

/// Two unit quaternions that name the same rotation, whatever their signs.
fn same_rotation(a: &[f32; 4], b: &[f32; 4], what: &str) {
    let dot: f32 = a.iter().zip(b).map(|(x, y)| x * y).sum();
    assert!((dot.abs() - 1.0).abs() < 1e-5, "{what}: {a:?} vs {b:?}");
}

fn near(a: &[f32], b: &[f32], what: &str) {
    for (x, y) in a.iter().zip(b) {
        assert!((x - y).abs() < 1e-5, "{what}: {a:?} vs {b:?}");
    }
}

fn applied_ats(events: &[SequenceEvent]) -> Vec<u32> {
    events
        .iter()
        .filter_map(|event| match event {
            SequenceEvent::KeyApplied {
                track: SequenceTrack::Transform,
                at,
                ..
            } => Some(*at),
            _ => None,
        })
        .collect()
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

const ANCHORED: &str = "    - at: 2
      role: hero
      over: 10
      to:
        anchor: door
        offset:
          - 0.0
          - 0.0
          - 2.0
        yaw_deg: 30.0
";

#[test]
fn a_linear_anchor_span_follows_the_formula_and_ends_on_the_target() {
    let mut stage = stage("anchor", &doc(ANCHORED, 20));
    play(&mut stage);
    let door_rotation = euler_xyz_degrees_to_quat([0.0, 90.0, 0.0]);
    let rotated = quat_rotate(door_rotation, [0.0, 0.0, 2.0]);
    let target = [10.0 + rotated[0], rotated[1], rotated[2]];
    // The playhead is the tick count minus one: 8 ticks put it at 7, half through the span.
    for _ in 0..8 {
        tick(&mut stage.engine);
    }
    let half = pose(&stage.engine, stage.hero).translation;
    assert_eq!(half, target.map(|c| c * 0.5));
    for _ in 0..20 {
        tick(&mut stage.engine);
    }
    let end = pose(&stage.engine, stage.hero);
    assert_eq!(end.translation, target);
    let want = quat_mul(door_rotation, euler_xyz_degrees_to_quat([0.0, 30.0, 0.0]));
    same_rotation(
        &world_pose(&stage.engine, stage.hero).rotation,
        &want,
        "rotation",
    );
    assert_eq!(pose(&stage.engine, stage.door), at([10.0, 0.0, 0.0], 90.0));
}

#[test]
fn a_parented_role_ends_on_the_world_target() {
    let key = "    - at: 0
      role: hero
      over: 6
      to:
        offset:
          - 3.0
          - 0.0
          - 4.0
        yaw_deg: 45.0
";
    let mut stage = stage("parented", &doc(key, 10));
    let parent = stage.engine.world_mut().spawn();
    let world = stage.engine.world_mut();
    world.insert(parent, at([5.0, 1.0, 0.0], 30.0)).unwrap();
    world.insert(stage.hero, Parent(parent)).unwrap();
    world.insert(stage.hero, at([1.0, 0.0, 0.0], 0.0)).unwrap();
    play(&mut stage);
    for _ in 0..14 {
        tick(&mut stage.engine);
    }
    let end = world_pose(&stage.engine, stage.hero);
    near(&end.translation, &[3.0, 0.0, 4.0], "translation");
    same_rotation(
        &end.rotation,
        &euler_xyz_degrees_to_quat([0.0, 45.0, 0.0]),
        "rotation",
    );
}

#[test]
fn a_path_span_samples_the_path_and_orients_to_it() {
    let key = format!(
        "    - at: 0
      role: walker
      over: 10
      path: {PATH}
      orient: yaw
"
    );
    let mut stage = stage("path", &doc(&key, 12));
    let sampler = ludion_scene::PathSampler::new(&ludion_scene::check_path(WALK).unwrap());
    let length = sampler.length();
    play(&mut stage);
    for _ in 0..3 {
        tick(&mut stage.engine);
    }
    // Playhead 2 of 10: s = 0.2 of the length, on the first segment, facing +X.
    let early = pose(&stage.engine, stage.walker);
    let want = sampler.point_at(0.2 * length).map(|c| c as f32);
    assert_eq!(early.translation, want);
    assert!((early.rotation[1] + 90.0).abs() < 1e-3, "{early:?}");
    for _ in 0..12 {
        tick(&mut stage.engine);
    }
    let end = pose(&stage.engine, stage.walker);
    assert_eq!(end.translation, sampler.point_at(length).map(|c| c as f32));
    assert!(end.rotation[1].abs() < 1e-3, "{end:?}");
}

#[test]
fn a_second_key_on_the_role_starts_from_the_current_pose() {
    let keys = "    - at: 0
      role: hero
      over: 10
      to:
        offset:
          - 10.0
          - 0.0
          - 0.0
    - at: 5
      role: hero
      over: 5
      to:
        offset:
          - 0.0
          - 0.0
          - 10.0
";
    let mut stage = stage("replace", &doc(keys, 12));
    play(&mut stage);
    let mut seen = Vec::new();
    for _ in 0..8 {
        tick(&mut stage.engine);
        seen.push(pose(&stage.engine, stage.hero).translation);
    }
    // Playheads 0..=7: the first span runs to 5 at playhead 5, the second takes over from there.
    assert_eq!(seen[4], [4.0, 0.0, 0.0]);
    assert_eq!(seen[5], [5.0, 0.0, 0.0], "no jump at the second key");
    assert_eq!(seen[6], [4.0, 0.0, 2.0]);
    for _ in 0..8 {
        tick(&mut stage.engine);
    }
    assert_eq!(
        pose(&stage.engine, stage.hero).translation,
        [0.0, 0.0, 10.0]
    );
}

const MIXED: &str = "    - at: 1
      role: hero
      over: 6
      to:
        offset:
          - 1.0
          - 2.0
          - 3.0
        yaw_deg: 20.0
    - at: 4
      role: walker
      over: 8
      ease: smooth
      path: paths/walk.path.yaml
      orient: frame
    - at: 7
      role: hero
      over: 6
      ease: out_quad
      to:
        anchor: door
        offset:
          - 0.0
          - 1.0
          - 1.0
        yaw_deg: 10.0
    - at: 15
      role: walker
      to:
        offset:
          - -1.0
          - 0.0
          - -1.0
";

#[test]
fn a_skip_at_any_tick_ends_where_the_natural_run_does() {
    let length = 20;
    let sequence = doc(MIXED, length);
    let mut natural = stage("skip-natural", &sequence);
    play(&mut natural);
    let mut natural_events = Vec::new();
    for _ in 0..=length {
        natural_events.extend(applied_ats(&tick(&mut natural.engine)));
    }
    assert_eq!(natural_events, [1, 4, 7, 15]);
    let want = (
        pose(&natural.engine, natural.hero),
        pose(&natural.engine, natural.walker),
    );
    for t in 0..length {
        let mut skipped = stage(&format!("skip-{t}"), &sequence);
        let handle = play(&mut skipped);
        let mut applied = Vec::new();
        for _ in 0..t {
            applied.extend(applied_ats(&tick(&mut skipped.engine)));
        }
        assert!(sequences::skip(skipped.engine.world_mut(), handle));
        applied.extend(applied_ats(&tick(&mut skipped.engine)));
        let got = (
            pose(&skipped.engine, skipped.hero),
            pose(&skipped.engine, skipped.walker),
        );
        assert_eq!(got, want, "skipped after {t} ticks");
        let mut once = applied.clone();
        once.sort_unstable();
        once.dedup();
        assert_eq!(once, applied, "a key applies once, skipped after {t}");
        assert!(applied.contains(&7) && applied.contains(&15), "{applied:?}");
    }
}

#[test]
fn a_snapshot_mid_span_replays_the_poses_and_the_hash() {
    let mut stage = stage("restore", &doc(MIXED, 20));
    play(&mut stage);
    for _ in 0..9 {
        tick(&mut stage.engine);
    }
    let snapshot = snapshot_of(&mut stage.engine);
    let (hero, walker) = (stage.hero, stage.walker);
    let record = |engine: &mut Engine| {
        (0..14)
            .map(|_| {
                let events = tick(engine);
                (
                    events,
                    pose(engine, hero),
                    pose(engine, walker),
                    engine.world_hash().unwrap().as_u64(),
                )
            })
            .collect::<Vec<_>>()
    };
    let reference = record(&mut stage.engine);
    restore(&mut stage.engine, snapshot);
    assert_eq!(record(&mut stage.engine), reference);
}

#[test]
fn a_role_without_a_transform_reports_once_and_the_beat_completes() {
    let key = "    - at: 0
      role: hero
      over: 4
      to:
        offset:
          - 1.0
          - 0.0
          - 0.0
";
    let mut stage = stage("bare", &doc(key, 8));
    stage
        .engine
        .world_mut()
        .remove::<Transform>(stage.hero)
        .unwrap();
    let handle = play(&mut stage);
    let mut ended = false;
    for _ in 0..12 {
        ended |= tick(&mut stage.engine)
            .iter()
            .any(|e| matches!(e, SequenceEvent::Ended { handle: h, .. } if *h == handle));
    }
    assert!(ended);
    let found = stage
        .engine
        .world()
        .resource::<Diagnostics>()
        .unwrap()
        .reported(code::SEQUENCE_KEY_DEGRADED)
        .count();
    assert_eq!(found, 1);
}

#[test]
fn a_cancel_leaves_the_pose_where_it_was() {
    let mut stage = stage("cancel", &doc(ANCHORED, 20));
    let handle = play(&mut stage);
    for _ in 0..8 {
        tick(&mut stage.engine);
    }
    let held = pose(&stage.engine, stage.hero);
    assert_ne!(held, at([0.0; 3], 0.0));
    assert!(sequences::cancel(stage.engine.world_mut(), handle));
    for _ in 0..5 {
        tick(&mut stage.engine);
    }
    assert_eq!(pose(&stage.engine, stage.hero), held);
}
