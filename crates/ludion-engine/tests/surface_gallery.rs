//! The `surface-tags` gallery scene: a motor walks across a concrete, metal_grate, concrete, puddle,
//! concrete floor and reports each surface where it crosses, the same at one and eight threads; a
//! dropped ball's first contact names the grating and the ball's own tag; the `motor` trace carries
//! the surface name.

use std::path::{Path, PathBuf};

use ludion_core::par;
use ludion_ecs::{SystemCtx, World};
use ludion_engine::{
    Engine, EngineBuilder, Name, TraceChannel, move_character, trace_with, write_jsonl,
};
use ludion_physics::{ContactPhase, PhysicsWorld};
use ludion_sim::InputScript;
use serde_json::Value;

const SCENE: &str = include_str!("../../../examples/gallery/surface-tags.yaml");
const FLOOR: &str = include_str!("../../../examples/gallery/meshes/surface-floor.mesh.yaml");

/// Ticks the hero walks: from x = -3.475 at 0.05 m a tick it leaves the puddle at tick 142 and
/// stops short of the floor's end at x = 4.
const TICKS: usize = 145;
const START_X: f64 = -3.475;
const STEP_X: f64 = 0.05;

/// The surface name the hero's ground reported on each tick that moved it.
#[derive(Default)]
struct Walked(Vec<Option<String>>);

fn walk(world: &mut World, _ctx: &SystemCtx) {
    let hero = world
        .entities()
        .find(|entity| matches!(world.get::<Name>(*entity), Ok(Some(name)) if name.0 == "hero"));
    let Some(hero) = hero else { return };
    let Ok(report) = move_character(world, hero, [0.05, -0.1, 0.0]) else {
        return;
    };
    let name = report.ground.and_then(|hit| hit.surface).and_then(|id| {
        let physics = world.resource::<PhysicsWorld>()?;
        physics.surface_name(id).map(str::to_owned)
    });
    if let Some(walked) = world.resource_mut::<Walked>() {
        walked.0.push(name);
    }
}

fn project(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "ludion-surface-gallery-{}-{name}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("meshes")).unwrap();
    std::fs::write(dir.join("meshes/surface-floor.mesh.yaml"), FLOOR).unwrap();
    std::fs::write(dir.join("scene.yaml"), SCENE).unwrap();
    dir
}

fn game(root: &Path) -> EngineBuilder {
    Engine::builder().project_root(root).system("walk", walk)
}

fn engine(name: &str) -> Engine {
    let mut engine = game(&project(name)).build().expect("boots");
    engine.load_inline_scene(SCENE).expect("the scene loads");
    engine.world_mut().insert_resource(Walked::default());
    engine
}

/// `(surface, first tick index)` per run of equal surfaces, `None`s dropped.
fn runs(walked: &[Option<String>]) -> Vec<(String, usize)> {
    let mut out: Vec<(String, usize)> = Vec::new();
    for (tick, name) in walked.iter().enumerate() {
        let Some(name) = name else { continue };
        if out.last().is_none_or(|(last, _)| last != name) {
            out.push((name.clone(), tick));
        }
    }
    out
}

fn walk_sequence(name: &str) -> Vec<(String, usize)> {
    let mut engine = engine(name);
    for _ in 0..TICKS {
        engine.advance_live();
    }
    let walked = engine.world().resource::<Walked>().unwrap();
    runs(&walked.0)
}

#[test]
fn the_motor_reports_each_surface_where_it_crosses() {
    let sequence = walk_sequence("walk");
    let names: Vec<&str> = sequence.iter().map(|(name, _)| name.as_str()).collect();
    assert_eq!(
        names,
        ["concrete", "metal_grate", "concrete", "puddle", "concrete"],
        "{sequence:?}"
    );
    // `Walked` holds only ticks that moved the hero, the first of which is tick 1 of the walk.
    for ((_, at), edge) in sequence[1..].iter().zip([-2.0, 2.0, 2.4, 3.6]) {
        let expected = ((edge - START_X) / STEP_X).ceil() as i64 - 1;
        assert!(
            (*at as i64 - expected).abs() <= 1,
            "crossing x = {edge}: tick {at}, expected {expected}"
        );
    }
}

#[test]
fn the_sequence_is_the_same_at_one_and_eight_threads() {
    #[allow(clippy::disallowed_methods)]
    let [one, eight] = std::thread::scope(|scope| {
        [(1, "t1"), (8, "t8")]
            .map(|(n, name)| scope.spawn(move || par::with_threads(n, || walk_sequence(name))))
            .map(|handle| {
                handle
                    .join()
                    .unwrap_or_else(|payload| std::panic::resume_unwind(payload))
            })
    });
    assert_eq!(one, eight);
}

#[test]
fn the_balls_first_contact_names_the_grating_and_the_ball() {
    let mut engine = engine("ball");
    let named = |engine: &Engine, name: &str| {
        engine
            .world()
            .entities()
            .find(|e| matches!(engine.world().get::<Name>(*e), Ok(Some(n)) if n.0 == name))
            .unwrap_or_else(|| panic!("no entity named {name}"))
    };
    let (floor, ball) = (named(&engine, "floor"), named(&engine, "ball"));
    for _ in 0..120 {
        engine.advance_live();
        let physics = engine.world().resource::<PhysicsWorld>().unwrap();
        let Some(event) = physics.contact_events().iter().find(|e| {
            e.phase == ContactPhase::Started
                && [e.a, e.b].contains(&floor)
                && [e.a, e.b].contains(&ball)
        }) else {
            continue;
        };
        let [grating, wood] = if event.a == floor {
            physics.contact_surfaces(event)
        } else {
            let [b, a] = physics.contact_surfaces(event);
            [a, b]
        };
        assert_eq!(
            grating.and_then(|id| physics.surface_name(id)),
            Some("metal_grate")
        );
        assert_eq!(wood.and_then(|id| physics.surface_name(id)), Some("wood"));
        return;
    }
    panic!("the ball never touched the floor");
}

#[test]
fn the_motor_trace_names_the_ground_surface() {
    let root = project("trace");
    let records = trace_with(
        game(&root),
        &root.join("scene.yaml"),
        &InputScript::new(),
        60,
        &TraceChannel::ALL,
    )
    .expect("the trace runs");
    let mut bytes = Vec::new();
    write_jsonl(&records, &mut bytes).unwrap();
    let grounds: Vec<Value> = std::str::from_utf8(&bytes)
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str::<Value>(line).unwrap())
        .filter(|line| line["channel"] == "motor")
        .filter_map(|line| line.get("ground").cloned())
        .collect();
    assert!(!grounds.is_empty());
    assert!(
        grounds.iter().any(|ground| ground["surface"] == "concrete"),
        "{grounds:?}"
    );
}
