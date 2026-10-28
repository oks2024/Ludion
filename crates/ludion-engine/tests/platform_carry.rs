//! Platform carry on the engine's `Transform`: a rider on a `PathFollower` platform, on a platform
//! parented to a spinning or a travelling entity, and the determinism of a ride across thread
//! counts and an `engine.snapshot` / `engine.restore` mid-ride. A game system calls
//! `ludion_engine::move_character` after the engine has posed the platforms.

use std::path::PathBuf;

use ludion_core::math::atan2;
use ludion_core::par;
use ludion_ecs::{SystemCtx, World};
use ludion_engine::{Engine, EngineBuilder, Name, Transform, move_character};
use ludion_physics::PhysicsPose;
use ludion_protocol::command::EngineRestoreParams;
use ludion_protocol::{Command, ResponseBody};

/// A closed 4 m square in the XZ plane at the platform's centre height, 16 m round.
const RING: &str = "\
format: 1
kind: path
name: ring
interpolation: linear
closed: true
points:
  - - 0.0
    - -0.25
    - 0.0
  - - 4.0
    - -0.25
    - 0.0
  - - 4.0
    - -0.25
    - 4.0
  - - 0.0
    - -0.25
    - 4.0
";

const TICKS: u64 = 600;
const SNAPSHOT_AT: usize = 300;

/// What the rider's system saw each tick.
#[derive(Default)]
struct Ride(Vec<Row>);

struct Row {
    feet: [f32; 3],
    platform: [f32; 3],
    platform_yaw: f32,
    rider_yaw: f32,
    report: String,
}

fn fixture_root(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "ludion-platform-carry-{tag}-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("a temp content root");
    std::fs::write(dir.join("ring.path.yaml"), RING).unwrap();
    dir
}

fn vec3(value: [f32; 3]) -> String {
    let mut text = String::new();
    for component in value {
        text.push_str(&format!("          - {component:?}\n"));
    }
    text
}

fn rider_entity(feet: [f32; 3]) -> String {
    format!(
        "  - name: rider\n    components:\n      Transform:\n        translation:\n{}      CharacterMotor:\n        radius: 0.28\n        height: 0.9\n        slope_limit_deg: 45.0\n        step_height: 0.25\n",
        vec3(feet)
    )
}

const SLAB: &str = "      Collider:\n        shape:\n          box:\n            half_extents:\n              - 1.5\n              - 0.25\n              - 1.5\n        body: kinematic\n";

fn named(world: &World, wanted: &str) -> ludion_core::ids::EntityId {
    world
        .entities()
        .find(|&id| {
            world
                .get::<Name>(id)
                .ok()
                .flatten()
                .is_some_and(|name| name.0 == wanted)
        })
        .unwrap_or_else(|| panic!("the scene has `{wanted}`"))
}

fn yaw_of(q: [f32; 4]) -> f32 {
    2.0 * atan2(q[1], q[3])
}

/// The platform's world pose: its own `Transform`, or the one derived through a `Parent`.
fn world_pose(world: &World, id: ludion_core::ids::EntityId) -> ([f32; 3], [f32; 4]) {
    Transform::derived_world_pose(world, id).unwrap_or_else(|| {
        let pose = world.get::<Transform>(id).unwrap().unwrap();
        (pose.translation(), pose.rotation_quat())
    })
}

/// Spins the entity named `spinner` by 0.1° a tick about +Y and slides `slider` 0.01 m a tick
/// along +X, both when the scene has them.
fn drive(world: &mut World, ctx: &SystemCtx) {
    let frame = ctx.frame.as_u64() as f32;
    for (name, spin) in [("spinner", true), ("slider", false)] {
        let Some(id) = world.entities().find(|&id| {
            world
                .get::<Name>(id)
                .ok()
                .flatten()
                .is_some_and(|n| n.0 == name)
        }) else {
            continue;
        };
        let pose = world.get_mut::<Transform>(id).unwrap().unwrap();
        if spin {
            pose.rotation = [0.0, 0.1 * frame, 0.0];
        } else {
            pose.translation[0] = 0.01 * frame;
        }
    }
}

fn walk(world: &mut World, _ctx: &SystemCtx) {
    let rider = named(world, "rider");
    let Ok(report) = move_character(world, rider, [0.0, -0.1, 0.0]) else {
        return;
    };
    let platform = named(world, "platform");
    let (platform_translation, platform_rotation) = world_pose(world, platform);
    let rider_pose = world.get::<Transform>(rider).unwrap().unwrap();
    let row = Row {
        feet: rider_pose.translation,
        platform: platform_translation,
        platform_yaw: yaw_of(platform_rotation),
        rider_yaw: yaw_of(rider_pose.rotation_quat()),
        report: format!("{report:?}"),
    };
    if world.resource::<Ride>().is_none() {
        world.insert_resource(Ride::default());
    }
    world.resource_mut::<Ride>().unwrap().0.push(row);
}

fn game(tag: &str) -> EngineBuilder {
    Engine::builder()
        .project_root(fixture_root(tag))
        .system("drive", drive)
        .system("walk", walk)
}

fn ride(tag: &str, scene: &str) -> Engine {
    let mut engine = game(tag).build().expect("the game builds");
    engine.load_inline_scene(scene).expect("the scene loads");
    for _ in 0..TICKS {
        engine.advance_live();
    }
    engine
}

fn rows(engine: &Engine) -> &[Row] {
    &engine.world().resource::<Ride>().expect("the rider ran").0
}

fn wrap(angle: f32) -> f32 {
    let tau = 2.0 * core::f32::consts::PI;
    (angle + core::f32::consts::PI).rem_euclid(tau) - core::f32::consts::PI
}

/// The rider's point in the platform's frame (yaw only), at row `i`.
fn local(rows: &[Row], i: usize) -> [f32; 3] {
    let row = &rows[i];
    let d = [
        row.feet[0] - row.platform[0],
        row.feet[1] - row.platform[1],
        row.feet[2] - row.platform[2],
    ];
    let (s, c) = ludion_core::math::sin_cos(row.platform_yaw);
    [d[0] * c - d[2] * s, d[1], d[0] * s + d[2] * c]
}

fn assert_fixed(rows: &[Row]) {
    let (start, end) = (local(rows, 30), local(rows, rows.len() - 1));
    for axis in 0..3 {
        assert!(
            (start[axis] - end[axis]).abs() < 1e-3,
            "{axis}: {start:?} vs {end:?}"
        );
    }
}

fn path_scene() -> String {
    format!(
        "format: 1\nseed: 3\nphysics:\n  space: free\nentities:\n  - name: platform\n    components:\n      Transform: {{}}\n      PathFollower:\n        path: ring.path.yaml\n        speed: 1.2\n        mode: loop\n{SLAB}{}",
        rider_entity([0.5, 0.3, 0.5])
    )
}

#[test]
fn a_path_follower_platform_carries_its_rider_for_600_ticks() {
    let engine = ride("path", &path_scene());
    let rows = rows(&engine);
    assert!(rows.len() > 500);
    assert_fixed(rows);
    // The platform really went round: the rider is nowhere near where it started.
    let moved = (rows[rows.len() - 1].feet[0] - rows[30].feet[0]).abs()
        + (rows[rows.len() - 1].feet[2] - rows[30].feet[2]).abs();
    assert!(moved > 3.0, "the platform barely moved: {moved}");
}

#[test]
fn a_child_of_a_rotating_parent_turns_the_rider_with_it() {
    let scene = format!(
        "format: 1\nseed: 3\nphysics:\n  space: free\nentities:\n  - name: spinner\n    components:\n      Transform: {{}}\n  - name: platform\n    components:\n      Parent:\n        index: 0\n        generation: 0\n      Transform:\n        translation:\n{}{SLAB}{}",
        vec3([2.0, -0.25, 0.0]),
        rider_entity([2.0, 0.3, 0.0])
    );
    let engine = ride("spin", &scene);
    let rows = rows(&engine);
    assert_fixed(rows);
    let offset = |i: usize| wrap(rows[i].rider_yaw - rows[i].platform_yaw);
    let last = rows.len() - 1;
    assert!(
        wrap(offset(last) - offset(30)).abs() < 1e-3,
        "rider yaw drifted from the platform's: {} vs {}",
        offset(last),
        offset(30)
    );
    assert!(rows[last].platform_yaw > 0.5, "the parent really turned");
}

#[test]
fn a_platform_under_a_travelling_parent_is_carried_by_its_derived_pose() {
    let scene = format!(
        "format: 1\nseed: 3\nphysics:\n  space: free\nentities:\n  - name: slider\n    components:\n      Transform: {{}}\n  - name: platform\n    components:\n      Parent:\n        index: 0\n        generation: 0\n      Transform:\n        translation:\n{}{SLAB}{}",
        vec3([0.0, -0.25, 0.0]),
        rider_entity([0.5, 0.3, 0.0])
    );
    let engine = ride("slide", &scene);
    let rows = rows(&engine);
    assert_fixed(rows);
    assert!(
        rows[rows.len() - 1].platform[0] > 5.0,
        "the parent really travelled"
    );
}

fn trace(engine: &mut Engine, ticks: usize) -> Vec<u64> {
    (0..ticks)
        .map(|_| {
            engine.advance_live();
            engine.world_hash().unwrap().as_u64()
        })
        .collect()
}

fn reports(engine: &Engine) -> Vec<String> {
    rows(engine).iter().map(|row| row.report.clone()).collect()
}

fn full_run(tag: &str) -> (Vec<u64>, Vec<String>) {
    let mut engine = game(tag).build().expect("boots");
    engine.load_inline_scene(&path_scene()).unwrap();
    let hashes = trace(&mut engine, TICKS as usize);
    (hashes, reports(&engine))
}

#[test]
fn the_ride_is_the_same_at_one_and_eight_threads() {
    #[allow(clippy::disallowed_methods)]
    let [one, eight] = std::thread::scope(|scope| {
        [(1, "t1"), (8, "t8")]
            .map(|(n, tag)| scope.spawn(move || par::with_threads(n, || full_run(tag))))
            .map(|handle| {
                handle
                    .join()
                    .unwrap_or_else(|payload| std::panic::resume_unwind(payload))
            })
    });
    assert_eq!(one, eight);
}

#[test]
fn the_ride_survives_a_snapshot_restore_mid_ride() {
    let (reference, reference_reports) = full_run("reference");
    let distinct: std::collections::BTreeSet<u64> = reference.iter().copied().collect();
    assert!(distinct.len() > 300, "a flat trace proves nothing");

    let mut first = game("first").build().expect("boots");
    first.load_inline_scene(&path_scene()).unwrap();
    assert_eq!(trace(&mut first, SNAPSHOT_AT), reference[..SNAPSHOT_AT]);
    let document = match first.execute(Command::EngineSnapshot).unwrap().body {
        ResponseBody::Snapshot { snapshot } => snapshot,
        other => panic!("expected a snapshot, got {other:?}"),
    };
    let taken = reports(&first).len();

    let mut resumed = game("resumed").build().expect("boots");
    resumed.load_inline_scene(&path_scene()).unwrap();
    match resumed
        .execute(Command::EngineRestore(EngineRestoreParams {
            snapshot: document,
        }))
        .unwrap()
        .body
    {
        ResponseBody::Restored { .. } => {}
        other => panic!("expected a restore, got {other:?}"),
    }
    assert_eq!(
        resumed.world_hash().unwrap().as_u64(),
        reference[SNAPSHOT_AT - 1]
    );
    assert_eq!(
        trace(&mut resumed, TICKS as usize - SNAPSHOT_AT),
        reference[SNAPSHOT_AT..]
    );
    assert_eq!(reports(&resumed), reference_reports[taken..]);
}
