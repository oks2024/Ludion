//! `move_characters` on the engine's `Transform`: a batch equals serial `move_character` calls in
//! slice order, is identical at 1, 2 and 8 threads, and carries a motor standing on a motor by the
//! support's previous-tick motion whatever the slice order.

use std::path::PathBuf;

use ludion_core::ids::EntityId;
use ludion_core::par;
use ludion_ecs::{SystemCtx, World};
use ludion_engine::{Engine, Transform, move_character, move_characters};
use ludion_physics::{CharacterMotor, MotorError, PhysicsWorld};

const TICKS: usize = 180;

/// A closed 4 m square at platform height.
const RING: &str = "\
format: 1
kind: path
name: ring
interpolation: linear
closed: true
points:
  - - 0.0
    - 0.75
    - 0.0
  - - 4.0
    - 0.75
    - 0.0
  - - 4.0
    - 0.75
    - 4.0
  - - 0.0
    - 0.75
    - 4.0
";

fn fixture_root(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("ludion-motor-batch-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("a temp content root");
    std::fs::write(dir.join("ring.path.yaml"), RING).unwrap();
    dir
}

fn vec3(value: [f32; 3], indent: &str) -> String {
    value
        .iter()
        .map(|c| format!("{indent}- {c:?}\n"))
        .collect::<String>()
}

fn solid(name: &str, centre: [f32; 3], half: [f32; 3]) -> String {
    format!(
        "  - name: {name}\n    components:\n      Transform:\n        translation:\n{}      Collider:\n        shape:\n          box:\n            half_extents:\n{}        body: static\n",
        vec3(centre, "          "),
        vec3(half, "              ")
    )
}

fn motor(name: &str, feet: [f32; 3]) -> String {
    format!(
        "  - name: {name}\n    components:\n      Transform:\n        translation:\n{}      CharacterMotor:\n        radius: 0.28\n        height: 0.9\n        slope_limit_deg: 45.0\n        step_height: 0.25\n",
        vec3(feet, "          ")
    )
}

const SLAB: &str = "      Collider:\n        shape:\n          box:\n            half_extents:\n              - 1.5\n              - 0.25\n              - 1.5\n        body: kinematic\n";

/// A floor, a step, a wall, a moving platform and 24 motors: riders on the platform, walkers that
/// meet the step and the wall, and some that start in the air.
fn crowd_scene() -> String {
    let mut text = String::from("format: 1\nseed: 3\nphysics:\n  space: free\nentities:\n");
    text += &solid("floor", [0.0, -0.5, 0.0], [14.0, 0.5, 14.0]);
    text += &solid("step", [3.5, 0.1, -7.0], [0.5, 0.1, 7.0]);
    text += &solid("wall", [8.0, 1.5, -7.0], [0.5, 1.5, 7.0]);
    text += &format!(
        "  - name: platform\n    components:\n      Transform: {{}}\n      PathFollower:\n        path: ring.path.yaml\n        speed: 1.2\n        mode: loop\n{SLAB}"
    );
    for i in 0..24usize {
        let feet = match i % 4 {
            0 => {
                let j = i / 4;
                [-0.8 + 0.8 * (j % 3) as f32, 1.3, 0.2 + (j / 3) as f32]
            }
            1 | 2 => [-2.0, 0.05, lane(i)],
            _ => [-2.0, 2.0, lane(i)],
        };
        text += &motor(&format!("m{i:02}"), feet);
    }
    text
}

/// Each walker and faller has a lane of its own, so no motor ever meets another: a motor standing on
/// a motor is the rider test's business.
fn lane(i: usize) -> f32 {
    -2.0 - 0.6 * (i - i / 4 - 1) as f32
}

fn motors(world: &mut World) -> Vec<EntityId> {
    let mut ids: Vec<_> = world
        .query_ids::<(&CharacterMotor,)>()
        .map(|(id, _)| id)
        .collect();
    ids.sort();
    ids
}

/// Per-motor displacement from its index and the tick: walkers go +X and every third turns around
/// every 60 ticks, riders stand still, and gravity always pulls.
fn displacement(i: usize, frame: u64) -> [f32; 3] {
    if i.is_multiple_of(4) {
        return [0.0, -0.1, 0.0];
    }
    let back = (i as u64 + frame / 60).is_multiple_of(3);
    [if back { -0.03 } else { 0.05 }, -0.1, 0.0]
}

fn crowd_moves(world: &mut World, frame: u64) -> Vec<(EntityId, [f32; 3])> {
    motors(world)
        .into_iter()
        .enumerate()
        .map(|(i, id)| (id, displacement(i, frame)))
        .collect()
}

fn serial_walk(world: &mut World, ctx: &SystemCtx) {
    for (id, step) in crowd_moves(world, ctx.frame.as_u64()) {
        let _ = move_character(world, id, step);
    }
}

fn batch_walk(world: &mut World, ctx: &SystemCtx) {
    let moves = crowd_moves(world, ctx.frame.as_u64());
    let mut out = vec![Err(MotorError::NoPhysicsWorld); moves.len()];
    move_characters(world, &moves, &mut out);
}

fn engine(tag: &str, scene: &str, system: fn(&mut World, &SystemCtx)) -> Engine {
    let mut engine = Engine::builder()
        .project_root(fixture_root(tag))
        .system("walk", system)
        .build()
        .expect("the game builds");
    engine.load_inline_scene(scene).expect("the scene loads");
    engine
}

fn tick(engine: &mut Engine) -> u64 {
    engine.advance_live();
    engine.world_hash().unwrap().as_u64()
}

/// What each motor's last report and pose say, for an end-of-run comparison.
fn digest(engine: &mut Engine) -> Vec<String> {
    let ids = motors(engine.world_mut());
    let world = engine.world();
    let physics = world.resource::<PhysicsWorld>().expect("a physics world");
    ids.into_iter()
        .map(|id| {
            let pose = world.get::<Transform>(id).unwrap().unwrap();
            format!(
                "{:?} {:?} {:?}",
                pose.translation,
                pose.rotation,
                physics.last_move_report(id)
            )
        })
        .collect()
}

#[test]
fn a_batch_equals_serial_calls_every_tick() {
    let scene = crowd_scene();
    let mut serial = engine("serial", &scene, serial_walk);
    let mut batch = engine("batch", &scene, batch_walk);
    let (mut grounded_on_platform, mut stepped, mut walled, mut airborne) = (0, 0, 0, 0);
    for t in 0..TICKS {
        assert_eq!(
            tick(&mut serial),
            tick(&mut batch),
            "hashes diverge at tick {t}"
        );
        let ids = motors(batch.world_mut());
        let physics = batch.world().resource::<PhysicsWorld>().unwrap();
        for id in ids {
            if let Some((_, report)) = physics.last_move_report(id) {
                grounded_on_platform += report.platform.entity.is_some() as u32;
                stepped += report.stepped.is_some() as u32;
                walled += !report.walls.is_empty() as u32;
                airborne += !report.grounded as u32;
            }
        }
    }
    assert_eq!(digest(&mut serial), digest(&mut batch));
    assert!(
        grounded_on_platform > 100,
        "riders were carried: {grounded_on_platform}"
    );
    assert!(stepped > 0, "a walker climbed the step");
    assert!(walled > 0, "a walker met the wall");
    assert!(airborne > 0, "a motor was in the air");
}

fn batch_trace(n: usize, tag: &str) -> Vec<u64> {
    let mut batch = engine(tag, &crowd_scene(), batch_walk);
    par::with_threads(n, || (0..TICKS).map(|_| tick(&mut batch)).collect())
}

#[test]
fn a_batch_is_the_same_at_one_two_and_eight_threads() {
    // The override is thread-local, so it is installed in the thread that ticks.
    #[allow(clippy::disallowed_methods)]
    let [one, two, eight] = std::thread::scope(|scope| {
        [1usize, 2, 8]
            .map(|n| scope.spawn(move || batch_trace(n, &format!("threads-{n}"))))
            .map(|handle| {
                handle
                    .join()
                    .unwrap_or_else(|payload| std::panic::resume_unwind(payload))
            })
    });
    assert_eq!(one, two, "1 and 2 threads diverge");
    assert_eq!(one, eight, "1 and 8 threads diverge");
}

/// Motor `a` walks +X on the floor; motor `b` stands on its head. Systems pick the slice order.
fn stack_scene() -> String {
    let mut text = String::from("format: 1\nseed: 3\nphysics:\n  space: free\nentities:\n");
    text += &solid("floor", [0.0, -0.5, 0.0], [14.0, 0.5, 14.0]);
    text += &motor("a", [0.0, 0.0, 0.0]);
    text += &motor("b", [0.0, 0.9, 0.0]);
    text
}

fn stack_walk(world: &mut World, reversed: bool) {
    let ids = motors(world);
    let mut moves = vec![(ids[0], [0.02, -0.1, 0.0]), (ids[1], [0.0, -0.1, 0.0])];
    if reversed {
        moves.reverse();
    }
    let mut out = vec![Err(MotorError::NoPhysicsWorld); 2];
    move_characters(world, &moves, &mut out);
}

fn forward(world: &mut World, _: &SystemCtx) {
    stack_walk(world, false);
}

fn backward(world: &mut World, _: &SystemCtx) {
    stack_walk(world, true);
}

/// Per tick: a's feet x, b's reported carry x, b's feet x.
fn stack_run(tag: &str, system: fn(&mut World, &SystemCtx)) -> Vec<(f32, f32, f32)> {
    let mut engine = engine(tag, &stack_scene(), system);
    (0..60)
        .map(|_| {
            engine.advance_live();
            let ids = motors(engine.world_mut());
            let world = engine.world();
            let x = |id| world.get::<Transform>(id).unwrap().unwrap().translation[0];
            let carry = world
                .resource::<PhysicsWorld>()
                .and_then(|physics| physics.last_move_report(ids[1]))
                .map_or(0.0, |(_, report)| report.platform.translation[0]);
            (x(ids[0]), carry, x(ids[1]))
        })
        .collect()
}

#[test]
fn a_rider_is_carried_by_its_supports_previous_tick_and_slice_order_is_free() {
    let ordered = stack_run("stack-forward", forward);
    let reversed = stack_run("stack-backward", backward);
    assert_eq!(ordered, reversed, "the slice order changed the result");
    let mut carried = 0;
    for t in 5..ordered.len() {
        let support_delta = ordered[t - 1].0 - ordered[t - 2].0;
        let carry = ordered[t].1;
        assert!(
            (carry - support_delta).abs() < 1e-5,
            "tick {t}: carry {carry} vs the support's previous-tick motion {support_delta}"
        );
        carried += (carry > 0.01) as u32;
    }
    assert!(carried > 40, "the rider was carried: {carried}");
}
