//! The physics time scale at the engine level: a game system that fires a blast and answers it with
//! a hit-stop freeze then a slow-motion stretch, driven by a hashed component, resumes from an
//! engine snapshot taken mid slow-motion bit-identically, changes the trace against the same scene
//! left unscaled, and hashes identically at 1, 2 and 8 threads.

use ludion_core::ids::EntityId;
use ludion_core::par;
use ludion_ecs::{SystemCtx, World};
use ludion_engine::{Engine, EngineBuilder};
use ludion_physics::{Blast, Falloff, PhysicsWorld, QueryFilter};
use ludion_protocol::command::EngineRestoreParams;
use ludion_protocol::{Command, ResponseBody};
use ludion_spatial::{Body, Collider};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeSet;
use std::path::PathBuf;

const TICKS: usize = 200;
/// Mid slow-motion: the blast at frame 40 freezes frames 40-42 and scales frames 43-67.
const SNAPSHOT_AT: usize = 51;
const MIN_DISTINCT: usize = 100;
/// The frame whose frozen step rebuilds a crate's edited collider into its reused slot.
const REBUILD_AT: u64 = 40;
const THREAD_COUNTS: [usize; 3] = [1, 2, 8];

/// The game's slow-motion state, hashed with the world: frames of freeze and of scale still owed.
#[derive(Serialize, Deserialize, JsonSchema, Clone, Copy, Debug, PartialEq)]
struct SlowMo {
    ticks_left: u32,
    hit_stop: u32,
}

fn fresh_dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("ludion-time-scale-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn game(name: &str, system: fn(&mut World, &SystemCtx)) -> EngineBuilder {
    Engine::builder()
        .project_root(fresh_dir(name))
        .components(|reg| reg.register::<SlowMo>("SlowMo"))
        .system("slow_mo", system)
}

/// A static floor, eight rigid boxes above it and the slow-motion sentinel, block style.
fn scene() -> String {
    let mut text = String::from("format: 1\nseed: 5\nphysics:\n  space: free\nentities:\n");
    text.push_str(
        "  - name: floor\n    components:\n      Transform:\n        translation:\n          - 0.0\n          - -0.5\n          - 0.0\n      Collider:\n        shape:\n          box:\n            half_extents:\n              - 12.0\n              - 0.5\n              - 12.0\n        body: static\n",
    );
    for i in 0..8 {
        let x = (i % 4) as f32 * 1.1 - 1.6;
        let z = (i / 4) as f32 * 1.1 - 0.5;
        text.push_str(&format!(
            "  - name: crate{i}\n    components:\n      Transform:\n        translation:\n          - {x:?}\n          - {:?}\n          - {z:?}\n      Collider:\n        shape:\n          box:\n            half_extents:\n              - 0.4\n              - 0.4\n              - 0.4\n        body:\n          rigid:\n            mass: 1.0\n      Velocity: {{}}\n",
            0.4 + i as f32 * 0.05,
        ));
    }
    text.push_str(
        "  - name: slowmo\n    components:\n      SlowMo:\n        ticks_left: 0\n        hit_stop: 0\n",
    );
    text
}

/// Every 40 ticks a blast, then three frozen frames and 25 scaled ones, `apply` gating the
/// physics calls only so the unscaled control still walks the same hashed state.
fn slow_mo_system(world: &mut World, ctx: &SystemCtx, apply: bool) {
    let tick = ctx.frame.as_u64();
    let mut action = None;
    for (state,) in world.query::<(&mut SlowMo,)>() {
        if tick != 0 && tick.is_multiple_of(40) {
            state.ticks_left = 25;
            state.hit_stop = 3;
        }
        if state.hit_stop > 0 {
            state.hit_stop -= 1;
            action = Some(None);
        } else if state.ticks_left > 0 {
            state.ticks_left -= 1;
            action = Some(Some(0.25));
        }
    }
    let Some(physics) = world.resource_mut::<PhysicsWorld>() else {
        return;
    };
    if tick != 0 && tick.is_multiple_of(40) {
        physics
            .radial_impulse(Blast {
                center: [0.0, 0.3, 0.0],
                radius: 3.0,
                impulse: 10.0,
                falloff: Falloff::Linear,
                filter: QueryFilter::new(),
            })
            .expect("a valid blast queues");
    }
    if !apply {
        return;
    }
    match action {
        Some(None) => physics.freeze(),
        Some(Some(scale)) => physics.scale_time(scale).expect("in range"),
        None => {}
    }
}

fn scaled(world: &mut World, ctx: &SystemCtx) {
    slow_mo_system(world, ctx, true);
}

fn unscaled(world: &mut World, ctx: &SystemCtx) {
    slow_mo_system(world, ctx, false);
}

fn traced(engine: &mut Engine, ticks: usize) -> Vec<u64> {
    (0..ticks)
        .map(|_| {
            engine.advance_live();
            engine.world_hash().unwrap().as_u64()
        })
        .collect()
}

fn ticks_left(engine: &mut Engine) -> u32 {
    engine
        .world_mut()
        .query::<(&SlowMo,)>()
        .map(|(state,)| state.ticks_left)
        .next()
        .expect("the scene has a slow-motion sentinel")
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

fn run(name: &str, system: fn(&mut World, &SystemCtx)) -> Vec<u64> {
    let mut engine = game(name, system).build().expect("boots");
    engine.load_inline_scene(&scene()).unwrap();
    traced(&mut engine, TICKS)
}

#[test]
fn a_scene_restored_mid_slow_motion_reproduces_the_uninterrupted_trace() {
    let text = scene();

    let reference = run("uninterrupted", scaled);
    let distinct: BTreeSet<u64> = reference.iter().copied().collect();
    assert!(
        distinct.len() > MIN_DISTINCT,
        "the reference trace holds {} distinct hashes of {TICKS}; a trace this flat means the \
         blasts never moved anything",
        distinct.len()
    );
    assert_ne!(
        reference,
        run("unscaled", unscaled),
        "the slow motion must change the trace, or it proves nothing"
    );

    let mut snapshotted = game("snapshotted", scaled).build().expect("boots");
    snapshotted.load_inline_scene(&text).unwrap();
    let first_half = traced(&mut snapshotted, SNAPSHOT_AT);
    assert_eq!(
        first_half,
        reference[..SNAPSHOT_AT],
        "two runs of one scene"
    );
    assert!(
        ticks_left(&mut snapshotted) > 0,
        "the snapshot must land while slow motion is still running"
    );
    let document = snapshot_of(&mut snapshotted);

    let mut resumed = game("resumed", scaled).build().expect("boots");
    resumed.load_inline_scene(&text).unwrap();
    restore(&mut resumed, document);
    assert_eq!(
        resumed.world_hash().unwrap().as_u64(),
        reference[SNAPSHOT_AT - 1],
        "the restore boundary itself matches"
    );
    let second_half = traced(&mut resumed, TICKS - SNAPSHOT_AT);
    for (offset, (resumed_hash, expected)) in second_half
        .iter()
        .zip(&reference[SNAPSHOT_AT..])
        .enumerate()
    {
        assert_eq!(
            resumed_hash,
            expected,
            "tick {} diverged after the resume",
            SNAPSHOT_AT + offset + 1
        );
    }
}

/// The scaled run at 1, 2 and 8 threads — each override installed inside its own spawned thread,
/// the `thread_identity.rs` shape (the override is thread-local).
#[test]
fn a_slowed_scene_hashes_identically_at_every_thread_count() {
    #[allow(clippy::disallowed_methods)]
    let [one, two, eight] = std::thread::scope(|scope| {
        THREAD_COUNTS
            .map(|n| {
                scope.spawn(move || par::with_threads(n, || run(&format!("threads-{n}"), scaled)))
            })
            .map(|handle| {
                handle
                    .join()
                    .unwrap_or_else(|payload| std::panic::resume_unwind(payload))
            })
    });
    assert_eq!(one, two, "the 1-thread and 2-thread traces diverge");
    assert_eq!(one, eight, "the 1-thread and 8-thread traces diverge");
}

/// The first crate (entity 1, after the floor): its mass is edited on a frozen frame.
const REBUILT: EntityId = EntityId::new(1, 0);

/// A three-frame hit-stop from [`REBUILD_AT`], the rebuilt crate's collider edited on its first
/// frozen frame, a blast centred on it on the second (still frozen), then a blast every 30 ticks.
fn rebuild_freeze_system(world: &mut World, ctx: &SystemCtx) {
    let tick = ctx.frame.as_u64();
    let mut frozen = false;
    for (state,) in world.query::<(&mut SlowMo,)>() {
        if tick == REBUILD_AT {
            state.hit_stop = 3;
        }
        if state.hit_stop > 0 {
            state.hit_stop -= 1;
            frozen = true;
        }
    }
    if tick == REBUILD_AT
        && let Ok(Some(collider)) = world.get_mut::<Collider>(REBUILT)
        && let Body::Rigid(body) = &mut collider.body
    {
        body.mass = 2.0;
    }
    let Some(physics) = world.resource_mut::<PhysicsWorld>() else {
        return;
    };
    let blast_at = tick == REBUILD_AT + 1
        || (tick >= REBUILD_AT + 3 && (tick - REBUILD_AT - 3).is_multiple_of(30));
    if blast_at {
        physics
            .radial_impulse(Blast {
                center: [-1.6, 0.4, -0.5],
                radius: 3.0,
                impulse: 10.0,
                falloff: Falloff::Linear,
                filter: QueryFilter::new(),
            })
            .expect("a valid blast queues");
    }
    if frozen {
        physics.freeze();
    }
}

fn blast_hit_entities(engine: &mut Engine) -> Vec<EntityId> {
    engine
        .world_mut()
        .resource::<PhysicsWorld>()
        .map(|physics| physics.blast_hits().iter().map(|hit| hit.entity).collect())
        .unwrap_or_default()
}

#[test]
fn a_scene_restored_mid_freeze_after_a_rebuild_reproduces_the_uninterrupted_trace() {
    // Ticks run: frames 0..=REBUILD_AT, so the snapshot follows the frozen step that rebuilt the
    // collider and precedes the blast frame.
    const SNAPSHOT_AT: usize = REBUILD_AT as usize + 1;
    let text = scene();

    let mut uninterrupted = game("rebuild-reference", rebuild_freeze_system)
        .build()
        .expect("boots");
    uninterrupted.load_inline_scene(&text).unwrap();
    let mut reference = Vec::new();
    let mut first_blast = None;
    for _ in 0..TICKS {
        uninterrupted.advance_live();
        reference.push(uninterrupted.world_hash().unwrap().as_u64());
        let hits = blast_hit_entities(&mut uninterrupted);
        if first_blast.is_none() && !hits.is_empty() {
            first_blast = Some(hits);
        }
    }
    let first_blast = first_blast.expect("the first blast hits something");
    assert!(
        !first_blast.contains(&REBUILT),
        "the rebuilt crate must be blast-immune until the next real step: {first_blast:?}"
    );
    let distinct: BTreeSet<u64> = reference.iter().copied().collect();
    assert!(
        distinct.len() > MIN_DISTINCT,
        "the reference trace holds {} distinct hashes of {TICKS}",
        distinct.len()
    );

    let mut snapshotted = game("rebuild-snapshotted", rebuild_freeze_system)
        .build()
        .expect("boots");
    snapshotted.load_inline_scene(&text).unwrap();
    let first_half = traced(&mut snapshotted, SNAPSHOT_AT);
    assert_eq!(
        first_half,
        reference[..SNAPSHOT_AT],
        "two runs of one scene"
    );
    let document = snapshot_of(&mut snapshotted);

    let mut resumed = game("rebuild-resumed", rebuild_freeze_system)
        .build()
        .expect("boots");
    resumed.load_inline_scene(&text).unwrap();
    restore(&mut resumed, document);
    assert_eq!(
        resumed.world_hash().unwrap().as_u64(),
        reference[SNAPSHOT_AT - 1],
        "the restore boundary itself matches"
    );
    let second_half = traced(&mut resumed, TICKS - SNAPSHOT_AT);
    for (offset, (resumed_hash, expected)) in second_half
        .iter()
        .zip(&reference[SNAPSHOT_AT..])
        .enumerate()
    {
        assert_eq!(
            resumed_hash,
            expected,
            "tick {} diverged after the resume",
            SNAPSHOT_AT + offset + 1
        );
    }
}

// ---- a gameplay scale drives the step ----

/// A forty-metre straight path along +x.
const LIFT: &str = "format: 1\nkind: path\nname: lift\ninterpolation: linear\nclosed: false\npoints:\n  - - 0.0\n    - -0.25\n    - 0.0\n  - - 40.0\n    - -0.25\n    - 0.0\n";
/// Platform speed in metres per gameplay second.
const PLATFORM_SPEED: f32 = 2.0;
/// Unscaled ticks and the scaled ticks that cover the same gameplay time once the first (unscaled)
/// tick has run: the scale requested on frame 0 folds into frame 1.
const SHORT: usize = 61;
const LONG: usize = 1 + 4 * (SHORT - 1);

/// A faller high above nothing, a kinematic slab on a path and a crate resting on it.
fn scale_scene() -> String {
    "format: 1\nseed: 5\nphysics:\n  space: free\nentities:\n  - name: faller\n    components:\n      Transform:\n        translation:\n          - 8.0\n          - 30.0\n          - 0.0\n      Collider:\n        shape:\n          box:\n            half_extents:\n              - 0.4\n              - 0.4\n              - 0.4\n        body:\n          rigid:\n            mass: 1.0\n      Velocity: {}\n  - name: slab\n    components:\n      Transform: {}\n      PathFollower:\n        path: lift.path.yaml\n        speed: 2.0\n        mode: loop\n      Collider:\n        shape:\n          box:\n            half_extents:\n              - 3.0\n              - 0.25\n              - 3.0\n        body: kinematic\n  - name: cargo\n    components:\n      Transform:\n        translation:\n          - 0.0\n          - 0.41\n          - 0.0\n      Collider:\n        shape:\n          box:\n            half_extents:\n              - 0.4\n              - 0.4\n              - 0.4\n        body:\n          rigid:\n            mass: 1.0\n      Velocity: {}\n".to_string()
}

/// Requests a quarter-speed gameplay clock every frame.
fn quarter_clock(world: &mut World, _ctx: &SystemCtx) {
    if let Some(clock) = world.resource_mut::<ludion_core::time::GameClock>() {
        clock.scale(0.25);
    }
}

fn idle(_world: &mut World, _ctx: &SystemCtx) {}

fn lift_engine(name: &str, system: fn(&mut World, &SystemCtx)) -> Engine {
    let root = fresh_dir(name);
    std::fs::write(root.join("lift.path.yaml"), LIFT).unwrap();
    let mut engine = Engine::builder()
        .project_root(root)
        .components(|reg| reg.register::<SlowMo>("SlowMo"))
        .system("slow_mo", system)
        .build()
        .expect("boots");
    engine.load_inline_scene(&scale_scene()).unwrap();
    engine
}

fn named(engine: &Engine, name: &str) -> EntityId {
    let world = engine.world();
    world
        .entities()
        .find(|&id| matches!(world.get::<ludion_engine::Name>(id), Ok(Some(n)) if n.0 == name))
        .unwrap_or_else(|| panic!("the scene has `{name}`"))
}

fn pose(engine: &Engine, name: &str) -> [f32; 3] {
    let id = named(engine, name);
    engine
        .world()
        .get::<ludion_engine::Transform>(id)
        .unwrap()
        .unwrap()
        .translation
}

fn linear_velocity(engine: &Engine, name: &str) -> [f32; 3] {
    let id = named(engine, name);
    engine
        .world()
        .get::<ludion_spatial::Velocity>(id)
        .unwrap()
        .unwrap()
        .linear
}

fn advance_ticks(engine: &mut Engine, ticks: usize) {
    for _ in 0..ticks {
        engine.advance_live();
    }
}

/// The same scene at quarter speed for `LONG` ticks and unscaled for `SHORT`.
fn matched_pair(tag: &str) -> (Engine, Engine) {
    let mut unscaled = lift_engine(&format!("{tag}-unscaled"), idle);
    advance_ticks(&mut unscaled, SHORT);
    let mut scaled = lift_engine(&format!("{tag}-scaled"), quarter_clock);
    advance_ticks(&mut scaled, LONG);
    (unscaled, scaled)
}

#[test]
fn a_scaled_fall_lands_where_the_unscaled_one_does_up_to_the_step_size() {
    let (unscaled, scaled) = matched_pair("fall");
    let (a, b) = (pose(&unscaled, "faller"), pose(&scaled, "faller"));
    assert!(30.0 - a[1] > 2.0, "the faller fell: {a:?}");
    // Semi-implicit Euler: the quarter-size step moves the drop by about g * t * (dt - dt/4) / 2.
    assert!(
        (a[1] - b[1]).abs() < 0.1,
        "unscaled {a:?} vs scaled {b:?}; measured bound 0.1 m after a {} m drop",
        30.0 - a[1]
    );
    assert!((a[0] - b[0]).abs() < 1e-4 && (a[2] - b[2]).abs() < 1e-4);
}

#[test]
fn a_kinematic_platform_pushes_its_cargo_at_its_true_speed_under_a_scale() {
    let (unscaled, scaled) = matched_pair("carry");
    let (rest, slow) = (
        linear_velocity(&unscaled, "cargo")[0],
        linear_velocity(&scaled, "cargo")[0],
    );
    assert!(
        rest > 0.5 * PLATFORM_SPEED,
        "the cargo rides the platform: {rest}"
    );
    assert!(
        slow < 1.5 * PLATFORM_SPEED,
        "the scaled push is not 4x ({slow} vs the platform's {PLATFORM_SPEED})"
    );
    assert!(
        (rest - slow).abs() < 0.3,
        "unscaled cargo speed {rest} vs scaled {slow}; measured bound 0.3 m/s"
    );
}

#[test]
fn a_snapshot_taken_mid_scale_resumes_bit_identically() {
    const TOTAL: usize = 200;
    const AT: usize = 120;
    let mut reference = lift_engine("mid-scale-reference", quarter_clock);
    let expected = traced(&mut reference, TOTAL);
    let distinct: BTreeSet<u64> = expected.iter().copied().collect();
    assert!(
        distinct.len() > MIN_DISTINCT,
        "{} distinct hashes",
        distinct.len()
    );

    let mut first = lift_engine("mid-scale-first", quarter_clock);
    traced(&mut first, AT);
    let document = snapshot_of(&mut first);

    let mut resumed = lift_engine("mid-scale-resumed", quarter_clock);
    restore(&mut resumed, document);
    assert_eq!(
        resumed.world_hash().unwrap().as_u64(),
        expected[AT - 1],
        "the restore boundary itself matches"
    );
    assert_eq!(traced(&mut resumed, TOTAL - AT), expected[AT..]);
}
