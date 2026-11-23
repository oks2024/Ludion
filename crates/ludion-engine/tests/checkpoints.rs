//! In-memory checkpoints (`Plan/checkpoints.md`, Wave 1): a system captures the world into a named
//! slot and restores it at the frame boundary. The sim frame, held input, the live profile,
//! `carry_resource` resources and an open recording continue; the world, RNG, scene and residue
//! rewind; the `GameClock` is rebased.
//!
//! The scene is a floor with three falling crates, a projectile pool firing every fifth tick and a
//! `Tally` component that reads the RNG and the input, so a wrong rewind of any of them diverges the
//! `WorldHash`. A closure system (`director`) plays a frame-addressed script of requests, the way a
//! game system issues them from hashed state: the same script on a replaying engine regenerates the
//! same requests.

use ludion_core::codes::code;
use ludion_core::diagnostics::Diagnostic;
use ludion_core::par;
use ludion_core::rng::DeterministicRng;
use ludion_core::time::{FrameIndex, GameClock};
use ludion_ecs::{Registry, SystemCtx, World};
use ludion_engine::diagnostics::Diagnostics;
use ludion_engine::{
    CheckpointConfig, Checkpoints, Engine, EngineBuilder, Profile, ProjectileConfig, ResidueStore,
    RestoreOutcome, RunSave, SceneSwitch, Transform, TransformHierarchy, projectiles_step,
    replay_hashes_with, spawn_projectile,
};
use ludion_projectiles::ProjectileDesc;
use ludion_protocol::command::{EngineRestoreParams, InputInjectParams};
use ludion_protocol::replay::{ReplayReader, ReplayRecord};
use ludion_protocol::{Command, ResponseBody};
use ludion_sim::{InputState, Key, WorldHash};
use ludion_spatial::spatial3d;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

type Trace = Vec<(FrameIndex, WorldHash)>;

/// Depends on the RNG and on input, so a wrong rewind of either shows in the hash.
#[derive(Serialize, Deserialize, JsonSchema, Clone, Debug, PartialEq)]
struct Tally {
    sum: u64,
    edges: u32,
    held: u32,
}

/// Holds a value JSON cannot store once a script poisons it.
#[derive(Serialize, Deserialize, JsonSchema, Clone, Debug, PartialEq)]
struct Wobble(f32);

#[derive(Serialize, Deserialize, JsonSchema, Default, Clone, Debug, PartialEq)]
struct Save {
    deaths: u32,
}

/// A resource a game marks `carry_resource`.
struct Carried(u32);

/// A resource nobody carries.
struct Fresh;

const HUB: &str = "\
format: 1
name: hub
seed: 5
physics:
  space: free
entities:
  - name: t
    components:
      Tally:
        sum: 0
        edges: 0
        held: 0
      Residue: {}
  - name: floor
    components:
      Transform:
        translation:
          - 0.0
          - 0.0
          - 0.0
      Collider:
        shape:
          box:
            half_extents:
              - 6.0
              - 0.5
              - 6.0
        body: static
  - name: crate0
    components:
      Transform:
        translation:
          - 0.0
          - 3.0
          - 0.0
      Collider:
        shape:
          box:
            half_extents:
              - 0.5
              - 0.5
              - 0.5
        body:
          rigid:
            mass: 1.0
            friction: 0.5
            restitution: 0.25
  - name: crate1
    components:
      Transform:
        translation:
          - 0.25
          - 5.0
          - 0.125
      Collider:
        shape:
          box:
            half_extents:
              - 0.5
              - 0.5
              - 0.5
        body:
          rigid:
            mass: 1.0
            friction: 0.5
            restitution: 0.25
  - name: crate2
    components:
      Transform:
        translation:
          - -0.25
          - 7.0
          - -0.125
      Collider:
        shape:
          box:
            half_extents:
              - 0.5
              - 0.5
              - 0.5
        body:
          rigid:
            mass: 1.0
            friction: 0.5
            restitution: 0.25
";

const NEXT: &str = "format: 1\nname: arrival\nentities:\n  - name: gate\n    components:\n      Transform:\n        translation:\n          - 4.0\n          - 0.0\n          - 0.0\n";

/// One scripted request, played at the frame it is keyed to.
#[derive(Clone, Debug)]
enum Act {
    Capture(&'static str),
    Restore(&'static str),
    Discard(&'static str),
    Persist(&'static str),
    LoadDisk,
    DiscardDisk,
    Switch,
    Hold,
    Suspend,
    Resume,
    Earn,
    Plant,
    /// Spawns an entity, despawns the last one and rewrites a component: the live world moves on.
    Churn,
    /// Puts a NaN on the first entity.
    Poison,
}

type Script = Arc<Vec<(u64, Act)>>;

fn script(acts: &[(u64, Act)]) -> Script {
    Arc::new(acts.to_vec())
}

fn tally(world: &mut World, _ctx: &SystemCtx) {
    let roll = world
        .resource_mut::<DeterministicRng>()
        .map(DeterministicRng::next_u32)
        .unwrap_or(0);
    let (held, edge) = {
        let input = world.resource::<InputState>().unwrap();
        (input.is_held(Key::W), input.was_pressed(Key::W))
    };
    for (t,) in world.query::<(&mut Tally,)>() {
        t.sum = t.sum.wrapping_add(u64::from(roll));
        t.edges += u32::from(edge);
        t.held += u32::from(held);
    }
}

fn fire(world: &mut World, ctx: &SystemCtx) {
    if ctx.frame.as_u64().is_multiple_of(5) {
        let mut shot = ProjectileDesc::new([0.0, 9.0, 0.0], [0.0, -30.0, 0.0]);
        shot.lifetime = 0.5;
        spawn_projectile(world, shot);
    }
}

fn director(script: Script) -> impl Fn(&mut World, &SystemCtx) + Send + Sync + 'static {
    move |world, ctx| {
        for (_, act) in script.iter().filter(|(at, _)| *at == ctx.frame.as_u64()) {
            match act {
                Act::Capture(name) => world.resource_mut::<Checkpoints>().unwrap().capture(*name),
                Act::Restore(name) => world.resource_mut::<Checkpoints>().unwrap().restore(*name),
                Act::Discard(name) => world.resource_mut::<Checkpoints>().unwrap().discard(*name),
                Act::Persist(name) => world.resource_mut::<Checkpoints>().unwrap().persist(*name),
                Act::LoadDisk => world.resource_mut::<Checkpoints>().unwrap().load_disk(),
                Act::DiscardDisk => world.resource_mut::<Checkpoints>().unwrap().discard_disk(),
                Act::Switch => world.resource_mut::<SceneSwitch>().unwrap().request(
                    "next.yaml".to_string(),
                    "gate".to_string(),
                    Vec::new(),
                ),
                Act::Hold => world.resource_mut::<GameClock>().unwrap().hold(),
                Act::Suspend => world.resource_mut::<RunSave>().unwrap().suspend(),
                Act::Resume => world.resource_mut::<RunSave>().unwrap().resume(),
                Act::Earn => {
                    world
                        .resource_mut::<Profile<Save>>()
                        .unwrap()
                        .get_mut()
                        .deaths += 1;
                }
                Act::Plant => {
                    world.insert_resource(Carried(7));
                    world.insert_resource(Fresh);
                }
                Act::Churn => {
                    let fresh = world.spawn();
                    world
                        .insert(
                            fresh,
                            Tally {
                                sum: 1,
                                edges: 2,
                                held: 3,
                            },
                        )
                        .unwrap();
                    let live: Vec<_> = world.entities().collect();
                    world.despawn(live[live.len() - 2]).unwrap();
                    let first = world.entities().next().unwrap();
                    world
                        .insert(
                            first,
                            Tally {
                                sum: 9,
                                edges: 9,
                                held: 9,
                            },
                        )
                        .unwrap();
                }
                Act::Poison => {
                    let first = world.entities().next().unwrap();
                    world.insert(first, Wobble(f32::NAN)).unwrap();
                }
            }
        }
    }
}

fn game(dir: &Path, max_slots: u32, acts: Script) -> EngineBuilder {
    Engine::builder()
        .project_root(dir)
        .checkpoints(CheckpointConfig { max_slots })
        .profile::<Save>()
        .carry_resource::<Carried>()
        .projectiles(ProjectileConfig {
            capacity: 16,
            ..ProjectileConfig::default()
        })
        .components(|registry| {
            registry.register::<Tally>("Tally")?;
            registry.register::<Wobble>("Wobble")?;
            Ok(())
        })
        .system("tally", tally)
        .system(
            "spatial3d_rebuild",
            spatial3d::rebuild::<Transform, TransformHierarchy>,
        )
        .system("fire", fire)
        .system("projectiles_step", projectiles_step)
        .system("director", director(acts))
}

fn boot(builder: EngineBuilder) -> Engine {
    let mut engine = builder.build().expect("boots");
    engine.load_inline_scene(HUB).unwrap();
    engine
}

fn fresh_dir(name: &str) -> PathBuf {
    let dir =
        std::env::temp_dir().join(format!("ludion-checkpoints-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("next.yaml"), NEXT).unwrap();
    dir
}

/// The engine's `WorldHash`, which also folds in the `GameClock` and the checkpoint store.
fn full_hash(engine: &mut Engine) -> WorldHash {
    engine.world_hash().unwrap()
}

/// [`full_hash`] after each of `n` more ticks.
fn full_trace(engine: &mut Engine, n: usize) -> Vec<WorldHash> {
    (0..n)
        .map(|_| {
            engine.advance_live();
            full_hash(engine)
        })
        .collect()
}

/// The hash of the world's content and the projectile pool, less the clock and store terms. A
/// restore moves those on purpose (the clock rebases past the frames it gave up, the store holds a
/// slot), so "the restore is the capture" is a claim about this hash.
pub(crate) fn content_hash(
    engine: &Engine,
    extra: fn(&mut Registry) -> Result<(), Diagnostic>,
) -> WorldHash {
    let mut registry = ludion_components::builtin_registry();
    registry.register::<Tally>("Tally").unwrap();
    registry.register::<Wobble>("Wobble").unwrap();
    extra(&mut registry).unwrap();
    registry
        .register_resource_digest::<ludion_engine::Projectiles>("projectiles", |p| {
            Some(p.pool().digest())
        })
        .unwrap();
    WorldHash::compute(engine.world(), &registry).unwrap()
}

fn hash(engine: &Engine) -> WorldHash {
    content_hash(engine, |_| Ok(()))
}

/// The content hash after each of `n` more ticks.
fn trace(engine: &mut Engine, n: usize) -> Vec<WorldHash> {
    (0..n)
        .map(|_| {
            engine.advance_live();
            hash(engine)
        })
        .collect()
}

fn checkpoints(engine: &Engine) -> &Checkpoints {
    engine.world().resource::<Checkpoints>().unwrap()
}

fn reported(engine: &Engine, code: ludion_core::diagnostics::DiagnosticCode) -> usize {
    engine
        .world()
        .resource::<Diagnostics>()
        .unwrap()
        .reported(code)
        .count()
}

fn snapshot_of(engine: &mut Engine) -> Value {
    match engine.execute(Command::EngineSnapshot).unwrap().body {
        ResponseBody::Snapshot { snapshot } => snapshot,
        other => panic!("expected a snapshot, got {other:?}"),
    }
}

fn press(engine: &mut Engine, kind: &str) {
    engine
        .execute(Command::InputInject(InputInjectParams {
            frame: None,
            event: json!({ "kind": kind, "key": "W" }),
        }))
        .unwrap();
}

#[test]
fn a_restore_returns_the_world_and_replays_the_same_future() {
    let dir = fresh_dir("rewind");
    let mut engine = boot(game(
        &dir,
        4,
        script(&[(10, Act::Capture("a")), (310, Act::Restore("a"))]),
    ));
    let first = trace(&mut engine, 311);
    let captured = first[10];
    assert_eq!(engine.frame().as_u64(), 311);
    assert_eq!(
        first[310], captured,
        "the world after the restore tick is the captured world"
    );
    assert_eq!(
        checkpoints(&engine).last_restore(),
        Some(&RestoreOutcome::Restored {
            name: "a".to_string(),
            frame: FrameIndex::new(11),
        })
    );
    assert!(checkpoints(&engine).slot("a").is_some());

    let second = trace(&mut engine, 300);
    assert_eq!(
        second[..299],
        first[11..310],
        "the next 300 ticks reproduce the ticks after the capture"
    );
    assert_eq!(engine.frame().as_u64(), 611, "the frame kept counting");
}

#[test]
fn a_slot_survives_ten_restores() {
    let dir = fresh_dir("ten");
    let mut acts = vec![(10, Act::Capture("a"))];
    acts.extend((0..10).map(|i| (30 + 20 * i, Act::Restore("a"))));
    let mut engine = boot(game(&dir, 4, script(&acts)));
    let all = trace(&mut engine, 31 + 20 * 9 + 19);
    let captured = all[10];
    for i in 0..10usize {
        assert_eq!(all[30 + 20 * i], captured, "restore {i}");
        let after = &all[31 + 20 * i..31 + 20 * i + 19];
        assert_eq!(after, &all[11..30], "the 19 ticks after restore {i}");
    }
    assert!(checkpoints(&engine).slot("a").is_some(), "the slot is kept");
}

#[test]
fn the_gameplay_clock_rebases_so_game_frame_matches_the_slot() {
    for acts in [
        vec![(10, Act::Capture("a")), (40, Act::Restore("a"))],
        // Captured inside a hold, restored after more holds.
        vec![
            (3, Act::Hold),
            (4, Act::Hold),
            (6, Act::Hold),
            (6, Act::Capture("a")),
            (20, Act::Hold),
            (40, Act::Restore("a")),
        ],
    ] {
        let dir = fresh_dir("clock");
        let mut engine = boot(game(&dir, 4, script(&acts)));
        trace(&mut engine, 41);
        let slot = checkpoints(&engine).slot("a").unwrap().clone();
        let clock = *engine.world().resource::<GameClock>().unwrap();
        assert_eq!(
            clock.game_frame(engine.frame()),
            slot.game_frame,
            "{acts:?}: gameplay time resumes where the slot stood"
        );
        assert_eq!(engine.frame().as_u64(), 41);
        trace(&mut engine, 5);
        let clock = *engine.world().resource::<GameClock>().unwrap();
        assert_eq!(
            clock.game_frame(engine.frame()).as_u64(),
            slot.game_frame.as_u64() + 5,
            "{acts:?}: and advances one per tick"
        );
    }
}

#[test]
fn the_live_profile_and_carried_resources_survive_a_restore() {
    let dir = fresh_dir("meta");
    let mut engine = boot(game(
        &dir,
        4,
        script(&[
            (10, Act::Capture("a")),
            (20, Act::Plant),
            (30, Act::Earn),
            (60, Act::Restore("a")),
        ]),
    ));
    trace(&mut engine, 61);
    let profile = engine.world().resource::<Profile<Save>>().unwrap();
    assert_eq!(profile.get().deaths, 1, "the death survived the rollback");
    assert_eq!(
        engine.world().resource::<Carried>().map(|c| c.0),
        Some(7),
        "a carry_resource resource crosses"
    );
    assert!(
        engine.world().resource::<Fresh>().is_none(),
        "other resources reset"
    );
}

#[test]
fn held_input_is_kept_and_no_edge_refires() {
    let dir = fresh_dir("input");
    let mut engine = boot(game(
        &dir,
        4,
        script(&[(10, Act::Capture("a")), (40, Act::Restore("a"))]),
    ));
    trace(&mut engine, 20);
    press(&mut engine, "key_down");
    trace(&mut engine, 21);
    assert_eq!(engine.frame().as_u64(), 41, "restored on the last tick");
    let input = engine.world().resource::<InputState>().unwrap();
    assert!(
        input.is_held(Key::W),
        "the capture had W up; the hand is still down"
    );
    trace(&mut engine, 1);
    let input = engine.world().resource::<InputState>().unwrap();
    assert!(input.is_held(Key::W));
    assert!(
        !input.was_pressed(Key::W),
        "the press edge did not fire again"
    );
}

#[test]
fn a_restore_across_a_switch_rewinds_the_scene_and_residue() {
    let dir = fresh_dir("switch");
    let mut engine = boot(game(
        &dir,
        4,
        script(&[
            (10, Act::Capture("a")),
            (20, Act::Switch),
            (40, Act::Restore("a")),
        ]),
    ));
    trace(&mut engine, 25);
    let store = engine.world().resource::<ResidueStore>().unwrap();
    assert_eq!(
        store.scenes().collect::<Vec<_>>(),
        ["hub"],
        "hub left residue"
    );
    trace(&mut engine, 16);
    let store = engine.world().resource::<ResidueStore>().unwrap();
    assert_eq!(
        store.scenes().count(),
        0,
        "the residue rewound to the slot's"
    );
    let snapshot = snapshot_of(&mut engine);
    assert_eq!(
        snapshot["scene_name"], "hub",
        "back in the captured district"
    );
    assert!(checkpoints(&engine).slot("a").is_some());
}

#[test]
fn a_restore_wins_over_a_switch_requested_the_same_frame() {
    let dir = fresh_dir("wins");
    let mut engine = boot(game(
        &dir,
        4,
        script(&[
            (10, Act::Capture("a")),
            (30, Act::Restore("a")),
            (30, Act::Switch),
        ]),
    ));
    trace(&mut engine, 31);
    let snapshot = snapshot_of(&mut engine);
    assert_eq!(snapshot["scene_name"], "hub", "the switch was dropped");
    assert_eq!(reported(&engine, code::CHECKPOINT_SWITCH_DROPPED), 1);
    assert_eq!(engine.frame().as_u64(), 31);
    trace(&mut engine, 3);
    assert!(
        !engine
            .world()
            .resource::<SceneSwitch>()
            .unwrap()
            .is_pending(),
        "and nothing is queued behind it"
    );
}

#[test]
fn a_capture_and_a_restore_in_one_frame_restore_what_was_just_captured() {
    let dir = fresh_dir("same-frame");
    let mut engine = boot(game(
        &dir,
        4,
        script(&[(10, Act::Restore("a")), (10, Act::Capture("a"))]),
    ));
    let mut control = boot(game(&dir, 4, script(&[(10, Act::Capture("a"))])));
    assert_eq!(trace(&mut engine, 12), trace(&mut control, 12));
    assert_eq!(
        checkpoints(&engine).last_restore(),
        Some(&RestoreOutcome::Restored {
            name: "a".to_string(),
            frame: FrameIndex::new(11),
        })
    );
}

#[test]
fn refusals_leave_the_world_untouched() {
    let dir = fresh_dir("refusals");
    let long = "x".repeat(65);
    let leaked: &'static str = Box::leak(long.into_boxed_str());
    let mut control = boot(game(&dir, 2, script(&[])));
    let mut engine = boot(game(
        &dir,
        2,
        script(&[
            (5, Act::Restore("nothing")),
            (6, Act::Capture("bad name")),
            (7, Act::Capture(leaked)),
            (8, Act::Capture("one")),
            (9, Act::Capture("two")),
            (10, Act::Capture("three")),
            (11, Act::Restore("three")),
            (12, Act::Restore("")),
        ]),
    ));
    let expected = trace(&mut control, 20);
    let got = trace(&mut engine, 20);
    assert_eq!(got, expected, "no refusal moved the world");
    assert_eq!(
        reported(&engine, code::CHECKPOINT_NO_SLOT),
        2,
        "nothing, three"
    );
    assert_eq!(
        reported(&engine, code::CHECKPOINT_NAME_INVALID),
        3,
        "space, long, empty"
    );
    assert_eq!(reported(&engine, code::CHECKPOINT_SLOTS_FULL), 1);
    let names: Vec<_> = checkpoints(&engine)
        .slots()
        .iter()
        .map(|s| s.name.as_str())
        .collect();
    assert_eq!(
        names,
        ["one", "two"],
        "sorted, and the full store evicted nothing"
    );
    assert!(matches!(
        checkpoints(&engine).last_restore(),
        Some(RestoreOutcome::Refused { name, .. }) if name == "three"
    ));
}

#[test]
fn recapturing_a_held_name_and_discarding_work_at_the_limit() {
    let dir = fresh_dir("limit");
    let mut engine = boot(game(
        &dir,
        1,
        script(&[
            (5, Act::Capture("a")),
            (6, Act::Capture("a")),
            (7, Act::Discard("a")),
            (7, Act::Capture("b")),
        ]),
    ));
    trace(&mut engine, 10);
    assert_eq!(reported(&engine, code::CHECKPOINT_SLOTS_FULL), 0);
    let names: Vec<_> = checkpoints(&engine)
        .slots()
        .iter()
        .map(|s| s.name.clone())
        .collect();
    assert_eq!(names, ["b"], "discard runs before capture");
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

fn sample(engine: &mut Engine) -> (FrameIndex, WorldHash) {
    (engine.frame(), full_hash(engine))
}

fn recorded_acts() -> Script {
    script(&[
        (10, Act::Capture("a")),
        (50, Act::Restore("a")),
        (80, Act::Capture("b")),
        (320, Act::Restore("b")),
        (400, Act::Restore("a")),
    ])
}

#[test]
fn a_recording_with_deaths_replays_as_one_linear_run() {
    let dir = fresh_dir("recording");
    let sink = SharedSink::default();
    let mut engine = boot(game(&dir, 4, recorded_acts()));
    engine.set_recording(sink.clone()).unwrap();
    let mut live: Trace = vec![sample(&mut engine)];
    for _ in 0..700 {
        engine.advance_live();
        live.push(sample(&mut engine));
    }
    engine.stop_recording().unwrap();

    let bytes = sink.0.lock().unwrap().clone();
    let replayed = replay_hashes_with(game(&dir, 4, recorded_acts()), &bytes, &dir).unwrap();
    assert_eq!(replayed, live);
    let frames: Vec<u64> = live.iter().map(|(f, _)| f.as_u64()).collect();
    assert!(
        frames.windows(2).all(|w| w[1] == w[0] + 1),
        "one linear frame axis"
    );

    // Slots no longer suppress keyframes: the one at 300 sits inside the held window.
    let reader = ReplayReader::new(&bytes).unwrap();
    let inside = reader.seek(FrameIndex::new(350)).unwrap();
    let (keyframe_frame, _) = inside.keyframe.expect("a keyframe inside the held window");
    assert_eq!(keyframe_frame.as_u64(), 300);
}

/// A seek lands on a keyframe between a capture and its restore; the slots ride that keyframe, so
/// the restores the game requests after it still reproduce the live run.
#[test]
fn a_seek_inside_a_held_window_continues_to_the_live_trace() {
    let dir = fresh_dir("seek");
    let sink = SharedSink::default();
    let mut engine = boot(game(&dir, 4, recorded_acts()));
    engine.set_recording(sink.clone()).unwrap();
    let mut live: BTreeMap<u64, u64> = BTreeMap::new();
    live.insert(engine.frame().as_u64(), full_hash(&mut engine).as_u64());
    for _ in 0..700 {
        engine.advance_live();
        live.insert(engine.frame().as_u64(), full_hash(&mut engine).as_u64());
    }
    engine.stop_recording().unwrap();
    let bytes = sink.0.lock().unwrap().clone();

    let reader = ReplayReader::new(&bytes).unwrap();
    // Frame 300 holds slots `a` (captured at 10) and `b` (at 80); `b` restores at 320, `a` at 400.
    let seek = reader.seek(FrameIndex::new(350)).unwrap();
    let (keyframe_frame, snapshot) = seek.keyframe.expect("a keyframe at or before 350");
    assert_eq!(keyframe_frame.as_u64(), 300);
    let mut recon = boot(game(&dir, 4, recorded_acts()));
    recon
        .execute(Command::EngineRestore(EngineRestoreParams { snapshot }))
        .unwrap();
    assert_eq!(
        checkpoints(&recon).slots().len(),
        2,
        "the slots rode the keyframe"
    );
    assert_eq!(full_hash(&mut recon).as_u64(), live[&300]);
    let mut compared = 0;
    for record in seek.deltas {
        if let ReplayRecord::Command { command, .. } = record {
            recon.execute(command).unwrap();
            let frame = recon.frame().as_u64();
            assert_eq!(
                full_hash(&mut recon).as_u64(),
                live[&frame],
                "frame {frame}"
            );
            compared += 1;
        }
    }
    assert!(
        compared > 0 && recon.frame().as_u64() >= 600,
        "drove past both restores (to the next keyframe)"
    );
}

#[test]
fn an_engine_restore_installs_the_snapshot_store() {
    let dir = fresh_dir("install");
    let acts = script(&[
        (2, Act::Capture("a")),
        (3, Act::Restore("a")),
        (20, Act::Restore("a")),
    ]);
    let mut engine = boot(game(&dir, 4, acts.clone()));
    trace(&mut engine, 8);
    assert_eq!(checkpoints(&engine).slots().len(), 1);
    let slots = checkpoints(&engine).slots().to_vec();
    let last = checkpoints(&engine).last_restore().cloned();
    assert!(last.is_some(), "the restore at frame 3 ran");
    let snapshot = snapshot_of(&mut engine);
    let live = full_trace(&mut engine, 30);

    let mut other = boot(game(&dir, 4, acts));
    assert!(checkpoints(&other).slots().is_empty());
    other
        .execute(Command::EngineRestore(EngineRestoreParams { snapshot }))
        .unwrap();
    assert_eq!(checkpoints(&other).slots(), slots.as_slice());
    assert_eq!(checkpoints(&other).last_restore(), last.as_ref());
    assert_eq!(
        full_trace(&mut other, 30),
        live,
        "the restored store serves the restore at frame 20"
    );
}

/// A run resume replaces the store with the suspended one, so a restore requested the same frame
/// asks for a world that is gone: it is refused (`L4204`), and the resumed world is what runs on.
#[test]
fn a_restore_requested_with_a_run_resume_is_refused() {
    let dir = fresh_dir("resume-restore");
    let acts = script(&[
        (2, Act::Capture("a")),
        (4, Act::Suspend),
        (10, Act::Capture("b")),
        (12, Act::Resume),
        (12, Act::Restore("b")),
    ]);
    let mut engine = boot(game(&dir, 4, acts).run_save().persist_profile());
    trace(&mut engine, 14);
    let names: Vec<_> = checkpoints(&engine)
        .slots()
        .iter()
        .map(|slot| slot.name.as_str())
        .collect();
    assert_eq!(names, ["a"], "the suspended store replaced the live one");
    assert!(
        matches!(
            checkpoints(&engine).last_restore(),
            Some(RestoreOutcome::Refused { name, reason })
                if name == "b" && reason.contains("replaced the world")
        ),
        "{:?}",
        checkpoints(&engine).last_restore()
    );
    assert_eq!(reported(&engine, code::CHECKPOINT_RESTORE_FAILED), 1);
}

#[test]
fn a_snapshot_without_slots_omits_the_field() {
    let dir = fresh_dir("nofield");
    let mut engine = boot(game(&dir, 4, script(&[])));
    trace(&mut engine, 3);
    assert!(snapshot_of(&mut engine).get("checkpoints").is_none());
}

/// The store adds a `WorldHash` term only while a slot is held.
#[test]
fn the_world_hash_covers_the_slot_metadata_only_while_a_slot_is_held() {
    let dir = fresh_dir("hashterm");
    let with = |acts: &[(u64, Act)]| {
        let mut engine = boot(game(&dir, 4, script(acts)));
        full_trace(&mut engine, 12)
    };
    let free = with(&[]);
    // A capture at 5 that does not move the world: the hash differs from the slot-free run from
    // the next frame, and a discard puts it back.
    let held = with(&[(5, Act::Capture("a"))]);
    assert_eq!(held[..5], free[..5]);
    assert_ne!(held[6], free[6], "a held slot is in the hash");
    let dropped = with(&[(5, Act::Capture("a")), (8, Act::Discard("a"))]);
    assert_eq!(dropped[..5], free[..5]);
    assert_ne!(dropped[6], free[6]);
    assert_eq!(dropped[9..], free[9..], "an empty store adds no term");
}

/// A structured slot and the document a keyframe restore turns it into hash alike.
#[test]
fn a_structured_slot_and_its_document_twin_hash_alike() {
    let dir = fresh_dir("twin");
    let acts = script(&[(10, Act::Capture("a"))]);
    let mut live = boot(game(&dir, 4, acts.clone()));
    trace(&mut live, 20);
    let snapshot = snapshot_of(&mut live);
    let mut twin = boot(game(&dir, 4, acts));
    twin.execute(Command::EngineRestore(EngineRestoreParams { snapshot }))
        .unwrap();
    assert_eq!(full_hash(&mut twin), full_hash(&mut live));
    assert_eq!(full_trace(&mut twin, 5), full_trace(&mut live, 5));
}

#[test]
fn a_scene_load_clears_the_store() {
    let dir = fresh_dir("load");
    let mut engine = boot(game(&dir, 4, script(&[(2, Act::Capture("a"))])));
    trace(&mut engine, 5);
    engine.load_inline_scene(HUB).unwrap();
    assert!(checkpoints(&engine).slots().is_empty());
}

#[test]
fn a_bad_config_is_refused_at_build() {
    for max_slots in [0, 65] {
        let error = Engine::builder()
            .checkpoints(CheckpointConfig { max_slots })
            .build()
            .err()
            .expect("refused");
        assert_eq!(error.code, code::CHECKPOINT_CONFIG_INVALID);
    }
}

#[test]
fn checkpoint_traces_are_thread_count_invariant() {
    let run = |n: usize| {
        let dir = fresh_dir(&format!("threads-{n}"));
        let mut engine = boot(game(&dir, 4, recorded_acts()));
        trace(&mut engine, 450)
    };
    #[allow(clippy::disallowed_methods)]
    let [one, two, eight] = std::thread::scope(|scope| {
        [1usize, 2, 8]
            .map(|n| {
                let run = &run;
                scope.spawn(move || par::with_threads(n, || run(n)))
            })
            .map(|handle| {
                handle
                    .join()
                    .unwrap_or_else(|payload| std::panic::resume_unwind(payload))
            })
    });
    assert_eq!(one, two, "1 vs 2 threads");
    assert_eq!(one, eight, "1 vs 8 threads");
}

// --- Wave 2: the disk slot ----------------------------------------------------------------------

fn persisting(dir: &Path, acts: Script) -> EngineBuilder {
    game(dir, 4, acts).persist_profile()
}

fn checkpoint_file(dir: &Path) -> PathBuf {
    dir.join("save/checkpoint.json")
}

/// Boots a persisting engine that captures `a` at frame 10 and persists it at 12, then quits.
/// Returns the world hash the capture was taken at.
fn persist_a(dir: &Path) -> WorldHash {
    let acts = script(&[(10, Act::Capture("a")), (12, Act::Persist("a"))]);
    let mut engine = boot(persisting(dir, acts));
    trace(&mut engine, 14)[10]
}

fn backups(dir: &Path) -> Vec<String> {
    std::fs::read_dir(dir.join("save"))
        .unwrap()
        .filter_map(|entry| entry.ok()?.file_name().into_string().ok())
        .filter(|name| name.starts_with("checkpoint.json.bak."))
        .collect()
}

fn refused_reason(engine: &Engine) -> String {
    match checkpoints(engine).last_restore() {
        Some(RestoreOutcome::Refused { reason, .. }) => reason.clone(),
        other => panic!("expected a refusal, got {other:?}"),
    }
}

#[test]
fn a_persisted_slot_continues_after_a_relaunch_and_the_file_stays() {
    let dir = fresh_dir("disk-continue");
    let captured = persist_a(&dir);
    assert!(checkpoint_file(&dir).exists());

    let acts = script(&[(3, Act::LoadDisk), (40, Act::Restore("a"))]);
    let mut engine = boot(persisting(&dir, acts));
    let info = checkpoints(&engine).disk().cloned().expect("read at boot");
    assert_eq!((info.name.as_str(), info.frame.as_u64()), ("a", 11));
    assert!(
        checkpoints(&engine).slots().is_empty(),
        "nothing is loaded before the request"
    );

    let loaded = trace(&mut engine, 4);
    assert_eq!(loaded[3], captured, "the world is the slot's");
    assert_eq!(
        engine.frame().as_u64(),
        11,
        "the frame jumped forward, never back"
    );
    let clock = *engine.world().resource::<GameClock>().unwrap();
    assert_eq!(clock.game_frame(engine.frame()), info.game_frame);
    assert_eq!(
        checkpoints(&engine).last_restore(),
        Some(&RestoreOutcome::Restored {
            name: "a".to_string(),
            frame: FrameIndex::new(11),
        })
    );
    assert!(checkpoint_file(&dir).exists(), "a Continue keeps the file");
    assert_eq!(checkpoints(&engine).disk(), Some(&info));
    assert!(
        checkpoints(&engine).slot("a").is_some(),
        "the slot is in the store"
    );

    // Dying after a Continue restores it.
    let after = trace(&mut engine, 30);
    assert_eq!(
        after[29], captured,
        "the in-memory restore of the loaded slot works"
    );
}

#[test]
fn loading_the_disk_slot_twice_gives_identical_worlds() {
    let dir = fresh_dir("disk-twice");
    persist_a(&dir);
    let acts = script(&[(3, Act::LoadDisk), (20, Act::LoadDisk)]);
    let mut engine = boot(persisting(&dir, acts));
    let first = trace(&mut engine, 4)[3];
    let second = trace(&mut engine, 10)[9];
    assert_eq!(first, second);
}

#[test]
fn discard_disk_removes_the_file() {
    let dir = fresh_dir("disk-discard");
    persist_a(&dir);
    let mut engine = boot(persisting(&dir, script(&[(2, Act::DiscardDisk)])));
    assert!(checkpoints(&engine).disk().is_some());
    trace(&mut engine, 4);
    assert!(!checkpoint_file(&dir).exists());
    assert!(checkpoints(&engine).disk().is_none());
}

fn refused_load_leaves_the_world_alone(label: &str, spoil: impl Fn(&Path)) {
    let dir = fresh_dir(label);
    persist_a(&dir);
    spoil(&checkpoint_file(&dir));

    let mut engine = boot(persisting(&dir, script(&[(3, Act::LoadDisk)])));
    let control_dir = fresh_dir(&format!("{label}-control"));
    let mut control = boot(game(&control_dir, 4, script(&[])));
    assert_eq!(
        trace(&mut engine, 6),
        trace(&mut control, 6),
        "{label}: the world is untouched"
    );

    let reason = refused_reason(&engine);
    assert!(!reason.is_empty());
    assert_eq!(
        checkpoints(&engine).disk_error().map(|d| d.code),
        Some(code::CHECKPOINT_DISK_LOAD_REFUSED),
        "{label}"
    );
    assert!(checkpoints(&engine).disk().is_none(), "{label}");
    assert!(checkpoints(&engine).slots().is_empty(), "{label}");
    assert!(!checkpoint_file(&dir).exists(), "{label}: renamed away");
    assert_eq!(backups(&dir).len(), 1, "{label}");
}

#[test]
fn a_foreign_build_is_refused_and_backed_up() {
    refused_load_leaves_the_world_alone("disk-build", |path| {
        let mut file: Value =
            serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
        file["build"]["engine_version"] = json!("0.0.0-old");
        std::fs::write(path, serde_json::to_string(&file).unwrap()).unwrap();
    });
}

#[test]
fn a_garbled_file_is_refused_and_backed_up() {
    refused_load_leaves_the_world_alone("disk-garbled", |path| {
        std::fs::write(path, "{ not json").unwrap();
    });
}

#[test]
fn a_load_while_recording_is_refused_and_the_file_is_kept() {
    let dir = fresh_dir("disk-recording");
    persist_a(&dir);
    let mut engine = boot(persisting(&dir, script(&[(3, Act::LoadDisk)])));
    engine.set_recording(SharedSink::default()).unwrap();
    trace(&mut engine, 6);
    assert!(refused_reason(&engine).contains("recording"));
    assert_eq!(
        checkpoints(&engine).disk_error().map(|d| d.code),
        Some(code::CHECKPOINT_DISK_LOAD_REFUSED)
    );
    assert!(checkpoint_file(&dir).exists(), "the file is kept");
    assert!(backups(&dir).is_empty(), "and not backed up");
    assert!(checkpoints(&engine).disk().is_some());
    assert!(checkpoints(&engine).slots().is_empty());
}

#[test]
fn a_host_that_keeps_no_saves_updates_disk_and_writes_nothing() {
    let dir = fresh_dir("disk-ephemeral");
    let acts = script(&[
        (2, Act::Capture("a")),
        (3, Act::Persist("a")),
        (6, Act::LoadDisk),
    ]);
    let mut engine = boot(game(&dir, 4, acts));
    trace(&mut engine, 5);
    assert_eq!(
        checkpoints(&engine).disk().map(|d| d.name.as_str()),
        Some("a")
    );
    assert!(!checkpoint_file(&dir).exists());
    trace(&mut engine, 3);
    assert!(refused_reason(&engine).contains("keeps no saves"));
}

#[test]
fn persisting_an_unknown_name_reports_l4200_and_writes_nothing() {
    let dir = fresh_dir("disk-unknown");
    let mut engine = boot(persisting(&dir, script(&[(2, Act::Persist("nope"))])));
    trace(&mut engine, 4);
    assert_eq!(reported(&engine, code::CHECKPOINT_NO_SLOT), 1);
    assert!(!checkpoint_file(&dir).exists());
    assert!(checkpoints(&engine).disk().is_none());
}

#[test]
fn a_disk_load_keeps_the_live_profile() {
    let dir = fresh_dir("disk-profile");
    persist_a(&dir);
    let acts = script(&[(2, Act::Earn), (3, Act::LoadDisk)]);
    let mut engine = boot(persisting(&dir, acts));
    trace(&mut engine, 5);
    let profile = engine.world().resource::<Profile<Save>>().unwrap();
    assert_eq!(
        profile.get().deaths,
        1,
        "the live profile survives a Continue"
    );
}

/// The in-memory (structured) restore against the disk (document) restore of the same capture:
/// the same world after the restore and the same future.
#[test]
fn a_structured_restore_matches_the_document_restore() {
    let (mem_dir, disk_dir) = (fresh_dir("equiv-mem"), fresh_dir("equiv-disk"));
    let mut mem = boot(persisting(
        &mem_dir,
        script(&[(10, Act::Capture("a")), (130, Act::Restore("a"))]),
    ));
    let mut disk = boot(persisting(
        &disk_dir,
        script(&[
            (10, Act::Capture("a")),
            (12, Act::Persist("a")),
            (130, Act::LoadDisk),
        ]),
    ));
    let from_memory = trace(&mut mem, 131 + 120);
    let from_disk = trace(&mut disk, 131 + 120);
    assert!(
        checkpoint_file(&disk_dir).exists(),
        "the disk twin persisted"
    );
    assert_eq!(
        from_memory[130], from_memory[10],
        "the restore is the capture"
    );
    assert_eq!(
        from_memory[130], from_disk[130],
        "same world after the restore"
    );
    assert_eq!(from_memory, from_disk, "same trace, before and after");
}

/// A restored slot is the world as captured, however the live world moved since, and restoring
/// does not consume or alias it.
#[test]
fn a_slot_is_not_aliased_by_the_live_world() {
    let dir = fresh_dir("alias");
    let mut engine = boot(game(
        &dir,
        4,
        script(&[
            (10, Act::Capture("a")),
            (30, Act::Churn),
            (60, Act::Restore("a")),
            (70, Act::Churn),
            (90, Act::Restore("a")),
        ]),
    ));
    let all = trace(&mut engine, 120);
    assert_ne!(all[59], all[10], "the churn moved the live world");
    assert_eq!(all[60], all[10], "first restore");
    assert_ne!(all[89], all[10], "the second churn moved it again");
    assert_eq!(all[90], all[10], "second restore: identical to the first");
    assert_eq!(all[61..70], all[11..20], "and the same future");
}

/// A NaN no longer fails a capture; it fails the disk write, which reports `L4208`, writes nothing
/// and leaves the slot restorable in memory.
#[test]
fn persisting_a_slot_with_a_non_finite_value_reports_l4208_and_the_slot_still_restores() {
    let dir = fresh_dir("nan");
    let mut engine = boot(persisting(
        &dir,
        script(&[
            (2, Act::Poison),
            (4, Act::Capture("a")),
            (6, Act::Persist("a")),
            (10, Act::Restore("a")),
        ]),
    ));
    for _ in 0..12 {
        engine.advance_live();
    }
    assert_eq!(reported(&engine, code::CHECKPOINT_CAPTURE_FAILED), 0);
    assert!(checkpoints(&engine).slot("a").is_some(), "the capture held");
    let error = checkpoints(&engine).disk_error().expect("the write failed");
    assert_eq!(error.code, code::CHECKPOINT_DISK_WRITE_FAILED);
    assert!(error.message.contains("non-finite"), "{}", error.message);
    assert!(!checkpoint_file(&dir).exists(), "nothing was written");
    assert!(checkpoints(&engine).disk().is_none());
    assert_eq!(
        checkpoints(&engine).last_restore(),
        Some(&RestoreOutcome::Restored {
            name: "a".to_string(),
            frame: FrameIndex::new(5),
        }),
        "the slot restores in memory"
    );
}
