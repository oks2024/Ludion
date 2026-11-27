//! The per-entity clock domain (`ludion docs sequences`, "Bound actors"): while a `holds_gameplay`
//! beat plays, the entities bound to its roles and their `Parent` descendants run on their own
//! gameplay instant, and everything else stays frozen. The proof is a comparison: a bound actor's
//! per-tick pose, mover and camera equal, tick for tick, a run of the same beat that does not hold.
//!
//! Runs over a scratch copy of the gallery's `voxels/` directory in the system temp directory.

use std::path::{Path, PathBuf};

use ludion_core::ids::EntityId;
use ludion_core::math::glam::{Mat4, Vec3};
use ludion_core::time::{SimTime, TickRate};
use ludion_engine::sequences::{self, SequenceBound};
use ludion_engine::{
    Animation, Engine, EngineBuilder, Transform, animation_events, blend_clocks, posed_joint_world,
};
use ludion_protocol::command::{EngineRestoreParams, SimStepParams};
use ludion_protocol::{Command, ResponseBody};
use serde_json::Value;

const PART: &str = "lower-arm-r";
const BEAT: &str = "sequences/beat.yaml";
/// The tick the beat is played on, after unheld warm-up ticks: later than the held span, so a stamp
/// shifted back by the lead stays positive (stamps are unsigned frame counts).
const PLAY_AT: usize = 50;
/// The frame the humanoid's `walk` clip was stamped at, before the bind.
const WALK_STARTED: u64 = 45;
const LENGTH: usize = 40;

const SCENE: &str = "\
format: 1
seed: 1
entities:
  - name: humanoid
    components:
      Transform:
        translation:
          - 2.0
          - 0.0
          - 1.0
      Mesh:
        shape:
          assembly: voxels/humanoid.assembly.yaml
      Animation:
        clip: walk
        started: 45
  - name: child
    components:
      Mesh:
        shape: cube
      Transform:
        translation:
          - 0.0
          - 1.0
          - 0.0
      Parent:
        index: 0
        generation: 0
  - name: camera
    components:
      Transform: {}
      Camera: {}
      CameraRig:
        target:
          index: 0
          generation: 0
  - name: mover
    components:
      Transform: {}
      PathFollower:
        path: lift.path.yaml
        speed: 6.0
  - name: twin
    components:
      Transform: {}
      PathFollower:
        path: lift.path.yaml
        speed: 6.0
";

const LIFT: &str = "\
format: 1
kind: path
name: lift
interpolation: linear
closed: false
points:
  - - 0.0
    - 0.0
    - 0.0
  - - 10.0
    - 0.0
    - 0.0
";

fn beat(holds: bool) -> String {
    format!(
        "format: 1
kind: sequence
roles:
  - actor
  - camera
  - mover
length_ticks: {LENGTH}
clock: sim
holds_gameplay: {holds}
tracks:
  animation:
    - at: 0
      role: actor
      clip: run
      fade: 0.25
"
    )
}

fn gallery() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples/gallery")
}

fn copy_dir(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).unwrap();
    for entry in std::fs::read_dir(from).unwrap() {
        let entry = entry.unwrap();
        let target = to.join(entry.file_name());
        if entry.file_type().unwrap().is_dir() {
            copy_dir(&entry.path(), &target);
        } else {
            std::fs::copy(entry.path(), target).unwrap();
        }
    }
}

fn fixture_project(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "ludion-sequence-clock-{}-{name}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    copy_dir(&gallery().join("voxels"), &dir.join("voxels"));
    std::fs::write(dir.join("lift.path.yaml"), LIFT).unwrap();
    dir
}

fn builder(root: &Path, opted_in: bool, holds: bool) -> EngineBuilder {
    let builder = Engine::builder().project_root(root);
    if !opted_in {
        return builder;
    }
    builder
        .sequences()
        .doc_text(BEAT, &beat(holds))
        .system("blend_clocks", blend_clocks)
        .query::<(&ludion_engine::Animation, &ludion_engine::Mesh)>()
        .query::<(&ludion_engine::Animation, &ludion_engine::PrefabInstance)>()
        .query::<(&mut ludion_engine::BlendClock,)>()
        .system("animation_events", animation_events)
        .query::<(&ludion_engine::Animation, &ludion_engine::Mesh)>()
        .query::<(&ludion_engine::BlendClock,)>()
}

struct Game {
    engine: Engine,
    humanoid: EntityId,
    child: EntityId,
    camera: EntityId,
    mover: EntityId,
    twin: EntityId,
}

fn game(root: &Path, opted_in: bool, holds: bool) -> Game {
    let mut engine = builder(root, opted_in, holds).build().unwrap();
    engine.load_inline_scene(SCENE).expect("the scene loads");
    let ids: Vec<EntityId> = engine.world().entities().collect();
    let [humanoid, child, camera, mover, twin] = ids[..] else {
        panic!("the scene has five entities");
    };
    Game {
        engine,
        humanoid,
        child,
        camera,
        mover,
        twin,
    }
}

fn step(engine: &mut Engine) {
    engine
        .execute(Command::SimStep(SimStepParams {
            ticks: 1,
            hash: false,
        }))
        .unwrap();
}

/// One tick's observable state.
#[derive(Clone, Debug, PartialEq)]
struct Record {
    pose: Mat4,
    mover: [f32; 3],
    twin: [f32; 3],
    camera: [f32; 3],
    bound: [bool; 3],
    hash: u64,
}

fn translation(game: &Game, id: EntityId) -> [f32; 3] {
    game.engine
        .world()
        .get::<Transform>(id)
        .unwrap()
        .unwrap()
        .translation
}

fn world_mat(pose: ludion_engine::WorldPose) -> Mat4 {
    let [x, y, z, t] = pose.trs.to_affine().cols.map(Vec3::from);
    Mat4::from_cols(x.extend(0.0), y.extend(0.0), z.extend(0.0), t.extend(1.0))
}

fn record(game: &mut Game) -> Record {
    let hash = game.engine.world_hash().unwrap().as_u64();
    let world = game.engine.world();
    let now = SimTime::from_ticks(game.engine.frame().as_u64(), TickRate::DEFAULT);
    let pose = posed_joint_world(world, game.humanoid, PART, now)
        .unwrap()
        .expect("the part resolves");
    let bound = |id| matches!(world.get::<SequenceBound>(id), Ok(Some(_)));
    Record {
        pose: world_mat(pose),
        mover: translation(game, game.mover),
        twin: translation(game, game.twin),
        camera: translation(game, game.camera),
        bound: [bound(game.humanoid), bound(game.camera), bound(game.child)],
        hash,
    }
}

fn play(game: &mut Game) {
    let (actor, camera, mover) = (game.humanoid, game.camera, game.mover);
    sequences::play(
        game.engine.world_mut(),
        BEAT,
        &[("actor", actor), ("camera", camera), ("mover", mover)],
    )
    .unwrap();
}

/// `PLAY_AT` unheld ticks, the beat played, then the beat's length and 25 ticks past it.
fn run(root: &Path, holds: bool) -> Vec<Record> {
    let mut game = game(root, true, holds);
    let mut records = Vec::new();
    for tick in 0..PLAY_AT + LENGTH + 25 {
        if tick == PLAY_AT {
            play(&mut game);
        }
        step(&mut game.engine);
        records.push(record(&mut game));
    }
    records
}

#[test]
fn a_bound_actor_moves_exactly_as_an_unheld_run() {
    let root = fixture_project("equal");
    let held = run(&root, true);
    let unheld = run(&root, false);
    assert_eq!(held.len(), unheld.len());
    for (tick, (h, u)) in held.iter().zip(&unheld).enumerate() {
        assert_eq!(h.pose, u.pose, "tick {tick}: the pose");
        assert_eq!(h.mover, u.mover, "tick {tick}: the mover");
        assert_eq!(h.camera, u.camera, "tick {tick}: the camera");
    }
    assert_ne!(
        held[PLAY_AT].pose,
        held[PLAY_AT + 20].pose,
        "the actor animates during the hold"
    );
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn unbound_entities_freeze_through_the_hold() {
    let root = fixture_project("freeze");
    let held = run(&root, true);
    let unheld = run(&root, false);
    let end = PLAY_AT + LENGTH;
    // Tick `PLAY_AT` still advanced: the hold it marks takes effect from the next tick.
    for tick in PLAY_AT + 1..=end {
        assert_eq!(
            held[tick].twin,
            held[PLAY_AT + 1].twin,
            "tick {tick}: the unbound follower is frozen"
        );
    }
    assert_ne!(held[end + 1].twin, held[end].twin, "it moves again after");
    assert!(
        unheld[PLAY_AT..end]
            .windows(2)
            .all(|pair| pair[0].twin != pair[1].twin),
        "an unheld run moves it every tick"
    );
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn the_bound_component_lives_from_start_to_the_tick_after_the_end() {
    let root = fixture_project("lives");
    let held = run(&root, true);
    let end = PLAY_AT + LENGTH;
    assert!(held[..PLAY_AT].iter().all(|r| r.bound == [false; 3]));
    for (tick, rec) in held.iter().enumerate().take(end + 1).skip(PLAY_AT) {
        assert_eq!(
            rec.bound, [true; 3],
            "tick {tick}: humanoid, camera and the Parent-child are bound"
        );
    }
    assert!(
        held[end + 1..].iter().all(|r| r.bound == [false; 3]),
        "released the tick after the end"
    );
    let unheld = run(&root, false);
    assert!(
        unheld.iter().all(|r| r.bound == [false; 3]),
        "a non-holding beat never binds"
    );
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn release_shifts_every_listed_stamp_by_the_lead() {
    let root = fixture_project("shift");
    let mut game = game(&root, true, true);
    let animation = |game: &Game| {
        game.engine
            .world()
            .get::<Animation>(game.humanoid)
            .unwrap()
            .unwrap()
            .clone()
    };
    let mut before = None;
    let mut ended = None;
    for tick in 0..PLAY_AT + LENGTH + 5 {
        if tick == PLAY_AT {
            play(&mut game);
        }
        step(&mut game.engine);
        let bound = matches!(
            game.engine.world().get::<SequenceBound>(game.humanoid),
            Ok(Some(_))
        );
        if tick == PLAY_AT + LENGTH {
            before = Some(animation(&game));
        }
        if ended.is_none() && tick > PLAY_AT && !bound {
            ended = Some(animation(&game));
        }
    }
    let (before, after) = (before.unwrap(), ended.expect("the entity is released"));
    assert_eq!(before.from_started.as_u64(), WALK_STARTED, "pre-bind stamp");
    let lead = before.started.as_u64() - after.started.as_u64();
    assert_eq!(
        lead, LENGTH as u64,
        "the lead is the held span: the beat's length"
    );
    assert_eq!(
        before.from_started.as_u64() - after.from_started.as_u64(),
        lead,
        "a stamp from before the bind shifts too"
    );
    assert_eq!(
        before.fade_started.as_u64() - after.fade_started.as_u64(),
        lead
    );
    let _ = std::fs::remove_dir_all(&root);
}

fn snapshot_of(engine: &mut Engine) -> Value {
    match engine.execute(Command::EngineSnapshot).unwrap().body {
        ResponseBody::Snapshot { snapshot } => snapshot,
        other => panic!("expected a snapshot, got {other:?}"),
    }
}

#[test]
fn a_restore_mid_bind_reproduces_the_hashes() {
    let root = fixture_project("restore");
    let mut first = game(&root, true, true);
    for tick in 0..PLAY_AT + 12 {
        if tick == PLAY_AT {
            play(&mut first);
        }
        step(&mut first.engine);
    }
    let snapshot = snapshot_of(&mut first.engine);
    let mut second = game(&root, true, true);
    match second
        .engine
        .execute(Command::EngineRestore(EngineRestoreParams { snapshot }))
        .unwrap()
        .body
    {
        ResponseBody::Restored { .. } => {}
        other => panic!("expected a restore, got {other:?}"),
    }
    for tick in 0..LENGTH + 10 {
        step(&mut first.engine);
        step(&mut second.engine);
        assert_eq!(
            record(&mut first),
            record(&mut second),
            "tick {tick} after the restore"
        );
    }
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn a_world_that_never_binds_hashes_like_one_without_sequences() {
    let root = fixture_project("quiet");
    let trace = |opted_in: bool| {
        let mut game = game(&root, opted_in, true);
        (0..30)
            .map(|_| {
                step(&mut game.engine);
                game.engine.world_hash().unwrap().as_u64()
            })
            .collect::<Vec<_>>()
    };
    assert_eq!(trace(true), trace(false));
    let _ = std::fs::remove_dir_all(&root);
}
