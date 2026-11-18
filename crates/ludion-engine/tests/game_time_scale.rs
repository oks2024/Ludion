//! The gameplay time scale acceptance run: one scene holding a pattern volley, a particle plume, an
//! animated assembly with events, a kinematic platform carrying a crate and a free-falling body,
//! slowed by a game system calling `GameClock::scale(0.25)`.
//!
//! - **Restore:** a snapshot taken mid slow-down resumes the uninterrupted `WorldHash` trace, which
//!   differs from an unscaled run's and holds many distinct hashes.
//! - **Threads:** the 1-, 2- and 8-thread traces are identical.
//! - **Acceptance:** a run scaled for its whole span and an unscaled run meet at the same gameplay
//!   time with exactly equal closed forms (bullets, particles, the clip pose), the same events and
//!   the same volley count, and with the physics bodies within the step-size bound.
//!
//! No chase-camera rig is in the scene: a rig needs a `kind: path` track plus a target and costs
//! more than its coverage, since `CameraRig` springs integrate `ctx.game_dt` like `PathFollower`.
//! There is no committed `.trace.txt`: the gallery trace gates tick the built-in schedule with no
//! game systems, so a gallery scene cannot scale; this in-test trace is the trace of record.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use ludion_core::ids::EntityId;
use ludion_core::par;
use ludion_core::time::{GameClock, Q16_PER_TICK};
use ludion_ecs::{SystemCtx, World};
use ludion_engine::{
    Animation, Engine, Mesh, Name, PrefabInstance, Sprite, Transform, advance_volleys,
    animation_events, play_patterns,
};
use ludion_protocol::command::EngineRestoreParams;
use ludion_protocol::{Command, ResponseBody};
use serde_json::Value;

use super::game_clock::{
    ASSEMBLY, EventLog, SCALED_SCENE, STORM, SWING, advance_to_instant, drawn, events_of, field,
    record_events,
};

const TOTAL: usize = 240;
const SNAPSHOT_AT: usize = 120;
const SLOW_FROM: u64 = 40;
const SLOW_TO: u64 = 200;
const MIN_DISTINCT: usize = 100;
const THREAD_COUNTS: [usize; 3] = [1, 2, 8];
/// Bounds from the step-size difference between the scaled and unscaled integrations (semi-implicit
/// Euler): measured on the 60-tick acceptance run below, with headroom.
const FALL_BOUND: f32 = 0.15;
const CARGO_BOUND: f32 = 0.1;
/// The integrators (physics, `PathFollower`) finish a scaled tick up to `(1 - s)` of a tick ahead of
/// the closed-form instant the other assertions read: `0.75 * speed * dt` = 0.025 m at 2 m/s.
const PLATFORM_BOUND: f32 = 0.03;

/// A forty-metre straight path along +x, at the slab's height.
const LIFT: &str = "format: 1\nkind: path\nname: lift\ninterpolation: linear\nclosed: false\npoints:\n  - - 0.0\n    - -0.25\n    - 12.0\n  - - 40.0\n    - -0.25\n    - 12.0\n";

/// The physics half of the scene: a faller, a kinematic slab on a path and a crate resting on it.
const PHYSICS_ENTITIES: &str = "  - name: faller
    components:
      Transform:
        translation:
          - 8.0
          - 30.0
          - 6.0
      Collider:
        shape:
          box:
            half_extents:
              - 0.4
              - 0.4
              - 0.4
        body:
          rigid:
            mass: 1.0
      Velocity: {}
  - name: slab
    components:
      Transform:
        translation:
          - 0.0
          - -0.25
          - 12.0
      PathFollower:
        path: lift.path.yaml
        speed: 2.0
        mode: loop
      Collider:
        shape:
          box:
            half_extents:
              - 3.0
              - 0.25
              - 3.0
        body: kinematic
  - name: cargo
    components:
      Transform:
        translation:
          - 0.0
          - 0.41
          - 12.0
      Collider:
        shape:
          box:
            half_extents:
              - 0.4
              - 0.4
              - 0.4
        body:
          rigid:
            mass: 1.0
      Velocity: {}
";

fn scene() -> String {
    let physics = SCALED_SCENE.replacen("entities:\n", "physics:\n  space: free\nentities:\n", 1);
    format!("{physics}{PHYSICS_ENTITIES}")
}

/// Slows the game for frames `SLOW_FROM..SLOW_TO`.
fn slow_window(world: &mut World, ctx: &SystemCtx) {
    if (SLOW_FROM..SLOW_TO).contains(&ctx.frame.as_u64()) {
        world.resource_mut::<GameClock>().unwrap().scale(0.25);
    }
}

/// Slows the game on every frame.
fn slow_always(world: &mut World, _ctx: &SystemCtx) {
    world.resource_mut::<GameClock>().unwrap().scale(0.25);
}

fn idle(_world: &mut World, _ctx: &SystemCtx) {}

fn root(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "ludion-game-time-scale-{tag}-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("a temp content root");
    for (rel, text) in [
        ("patterns/storm.yaml", STORM),
        ("lift.path.yaml", LIFT),
        ("models/fixture.assembly.yaml", ASSEMBLY),
        ("models/anims/swing.anim.yaml", SWING),
        (
            "models/parts/body.part.yaml",
            "format: 1\nkind: part\nsize:\n  - 1\n  - 1\n  - 1\nslices:\n  -\n    - \"A\"\n",
        ),
    ] {
        let path = dir.join(rel);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    }
    dir
}

fn game(dir: &Path, system: fn(&mut World, &SystemCtx)) -> Engine {
    let mut engine = Engine::builder()
        .patterns()
        .system("slow", system)
        .system("play_patterns", play_patterns)
        .system("advance_volleys", advance_volleys)
        .system("animation_events", animation_events)
        .query::<(&Animation, &Mesh)>()
        .query::<(&Animation, &Sprite)>()
        .query::<(&Animation, &PrefabInstance)>()
        .system("record_events", record_events)
        .project_root(dir)
        .build()
        .expect("the documents validate");
    engine.load_inline_scene(&scene()).expect("the scene loads");
    engine
}

fn traced(engine: &mut Engine, ticks: usize) -> Vec<u64> {
    (0..ticks)
        .map(|_| {
            engine.advance_live();
            engine.world_hash().unwrap().as_u64()
        })
        .collect()
}

fn run(tag: &str, system: fn(&mut World, &SystemCtx)) -> Vec<u64> {
    let dir = root(tag);
    let trace = traced(&mut game(&dir, system), TOTAL);
    let _ = std::fs::remove_dir_all(&dir);
    trace
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

fn named(engine: &Engine, name: &str) -> EntityId {
    let world = engine.world();
    world
        .entities()
        .find(|&id| matches!(world.get::<Name>(id), Ok(Some(n)) if n.0 == name))
        .unwrap_or_else(|| panic!("the scene has `{name}`"))
}

fn translation(engine: &Engine, name: &str) -> [f32; 3] {
    engine
        .world()
        .get::<Transform>(named(engine, name))
        .unwrap()
        .unwrap()
        .translation
}

#[test]
fn a_scene_restored_mid_slow_down_reproduces_the_uninterrupted_trace() {
    let reference = run("restore-reference", slow_window);
    let distinct: BTreeSet<u64> = reference.iter().copied().collect();
    assert!(
        distinct.len() > MIN_DISTINCT,
        "the reference trace holds {} distinct hashes of {TOTAL}; a trace this flat proves nothing",
        distinct.len()
    );
    assert_ne!(
        reference,
        run("restore-unscaled", idle),
        "the slow-down must change the trace, or it proves nothing"
    );

    let dir = root("restore-first");
    let mut first = game(&dir, slow_window);
    assert_eq!(traced(&mut first, SNAPSHOT_AT), reference[..SNAPSHOT_AT]);
    assert!(
        first.world().resource::<GameClock>().unwrap().is_scaling(),
        "the snapshot lands while the slow-down runs"
    );
    let document = snapshot_of(&mut first);

    let mut resumed = game(&dir, slow_window);
    restore(&mut resumed, document);
    assert_eq!(
        resumed.world_hash().unwrap().as_u64(),
        reference[SNAPSHOT_AT - 1],
        "the restore boundary itself matches"
    );
    let tail = traced(&mut resumed, TOTAL - SNAPSHOT_AT);
    for (offset, (got, expected)) in tail.iter().zip(&reference[SNAPSHOT_AT..]).enumerate() {
        assert_eq!(got, expected, "tick {} diverged", SNAPSHOT_AT + offset + 1);
    }
    let _ = std::fs::remove_dir_all(&dir);
}

/// Each override is installed inside its own spawned thread (it is thread-local).
#[test]
fn a_slowed_scene_hashes_identically_at_every_thread_count() {
    #[allow(clippy::disallowed_methods)]
    let [one, two, eight] = std::thread::scope(|scope| {
        THREAD_COUNTS
            .map(|n| {
                scope.spawn(move || {
                    par::with_threads(n, || run(&format!("threads-{n}"), slow_window))
                })
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

#[test]
fn a_run_scaled_for_its_whole_span_matches_an_unscaled_run_at_equal_gameplay_time() {
    let dir = root("acceptance");
    let target = 60 * Q16_PER_TICK;

    let mut slow = game(&dir, slow_always);
    advance_to_instant(&mut slow, target);
    assert!(
        slow.frame().as_u64() >= 4 * 59,
        "a quarter-speed run ticks about 4x as often"
    );
    let mut full = game(&dir, idle);
    advance_to_instant(&mut full, target);
    assert_eq!(full.frame().as_u64(), 60);

    // Closed forms: exact.
    let picture = drawn(&full);
    assert!(
        !picture.particles.is_empty() && !picture.meshes.is_empty(),
        "the fixture draws the plume and the assembly"
    );
    assert_eq!(drawn(&slow), picture, "same particles and clip pose");
    let volleys = field(&full);
    assert!(
        volleys.len() >= 8,
        "the storm fired {} volleys",
        volleys.len()
    );
    assert_eq!(field(&slow), volleys, "same volleys, same bullet positions");
    let names = events_of(&full);
    assert_eq!(
        names,
        ["swing", "hit", "end"],
        "each event fires once, in order"
    );
    assert_eq!(events_of(&slow), names);
    assert!(slow.world().resource::<EventLog>().is_some());

    // Integrated physics: within the step-size bound.
    let (a, b) = (translation(&full, "faller"), translation(&slow, "faller"));
    assert!(30.0 - a[1] > 2.0, "the faller fell: {a:?}");
    assert!(
        (a[1] - b[1]).abs() < FALL_BOUND && (a[0] - b[0]).abs() < 1e-4,
        "faller: unscaled {a:?} vs scaled {b:?}; bound {FALL_BOUND} m"
    );
    let (platform_a, platform_b) = (translation(&full, "slab"), translation(&slow, "slab"));
    assert!(
        (platform_a[0] - platform_b[0]).abs() < PLATFORM_BOUND && platform_a[0] > 1.0,
        "the platform follows the path at its true speed: {platform_a:?} vs {platform_b:?} at frame {}",
        slow.frame().as_u64()
    );
    let (c, d) = (translation(&full, "cargo"), translation(&slow, "cargo"));
    assert!(
        (c[0] - d[0]).abs() < CARGO_BOUND,
        "cargo: unscaled {c:?} vs scaled {d:?}; bound {CARGO_BOUND} m"
    );
    let _ = std::fs::remove_dir_all(&dir);
}
