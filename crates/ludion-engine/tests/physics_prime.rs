//! Priming is a pure cache (`Plan/physics.md`, fling 0032). A scene load or switch builds the physics
//! world and runs its first collision detection at commit; tick 1 adopts that work when nothing
//! physics reads has changed, and otherwise throws it away and detects as usual. Every test here
//! compares a primed run against a cold one (`PrimedPhysics` removed right after the rebuild) and
//! demands the same per-tick `WorldHash` and the same contact events, so a prime that leaks into the
//! simulation fails here and not in a game.
//!
//! The scene is built in code: a brick wall glued by fixed joints, two-part compound roof tiles
//! resting on it, a mixed box/sphere/capsule pile on the static floor, and one motor-driven hinge.
//! Every body is placed exactly touching, so tick 1 has live contacts.
//!
//! **Which world-rebuild paths prime.** A scene load (`load_inline_scene`, `load_scene_file`, a second
//! `scene.load`), a switch to another scene and a restart switch all prime: their commit builds the
//! world and tick 1 adopts it. A restore never primes, because the snapshot carries its own
//! `PhysicsWorld` (a frame-0 snapshot has none, and `PrimedPhysics` is not part of one), so the first
//! tick builds cold. A replay start loads the recorded scene, so it primes like a load.

use std::path::PathBuf;

use ludion_core::par;
use ludion_ecs::{SystemCtx, World};
use ludion_engine::{Engine, EngineBuilder, SceneSwitch};
use ludion_physics::{ContactEvent, PhysicsWorld, PrimedPhysics};
use ludion_protocol::command::{EngineRestoreParams, SceneLoadParams, SimStepParams};
use ludion_protocol::{Command, ResponseBody};
use ludion_spatial::{Body, Collider, Velocity};
use serde_json::{Value, json};

const TICKS: usize = 120;

fn fresh_dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "ludion-physics-prime-{}-{name}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// An `f32` as a JSON number carrying its shortest decimal form, so the emitted text reads `0.3`,
/// not `0.30000001192092896`.
fn f(x: f32) -> Value {
    json!(format!("{x:?}").parse::<f64>().unwrap())
}

fn v3(x: [f32; 3]) -> Value {
    json!([f(x[0]), f(x[1]), f(x[2])])
}

/// Block-style YAML for a JSON value: the scene reader allows only empty flow collections.
fn emit(value: &Value, indent: usize, out: &mut String) {
    let pad = " ".repeat(indent);
    match value {
        Value::Object(map) if !map.is_empty() => {
            for (key, child) in map {
                match child {
                    Value::Object(inner) if !inner.is_empty() => {
                        out.push_str(&format!("{pad}{key}:\n"));
                        emit(child, indent + 2, out);
                    }
                    Value::Array(items) if !items.is_empty() => {
                        out.push_str(&format!("{pad}{key}:\n"));
                        emit(child, indent + 2, out);
                    }
                    scalar => out.push_str(&format!("{pad}{key}: {}\n", flow_scalar(scalar))),
                }
            }
        }
        Value::Array(items) if !items.is_empty() => {
            for item in items {
                match item {
                    Value::Object(inner) if !inner.is_empty() => {
                        let mut block = String::new();
                        emit(item, indent + 2, &mut block);
                        out.push_str(&format!("{pad}- {}", &block[indent + 2..]));
                    }
                    Value::Array(inner) if !inner.is_empty() => unreachable!("no nested lists"),
                    scalar => out.push_str(&format!("{pad}- {}\n", flow_scalar(scalar))),
                }
            }
        }
        other => out.push_str(&format!("{pad}{}\n", flow_scalar(other))),
    }
}

fn flow_scalar(value: &Value) -> String {
    match value {
        Value::Object(_) => "{}".into(),
        Value::Array(_) => "[]".into(),
        other => other.to_string(),
    }
}

/// The scene's entities; an entity's index is its document position, which is how a `Joint` names
/// its bodies.
struct Scene(Vec<Value>);

impl Scene {
    fn add(&mut self, name: &str, components: Value) -> usize {
        self.0
            .push(json!({ "name": name, "components": components }));
        self.0.len() - 1
    }

    /// A body at `at` with `shape`, static or a 1 kg rigid body carrying a `Velocity`.
    fn body(&mut self, name: &str, at: [f32; 3], shape: Value, rigid: bool) -> usize {
        let body = if rigid {
            json!({ "rigid": { "mass": 1.0 } })
        } else {
            json!("static")
        };
        let mut components = json!({
            "Transform": { "translation": v3(at) },
            "Collider": { "shape": shape, "body": body },
        });
        if rigid {
            components["Velocity"] = json!({});
        }
        self.add(name, components)
    }

    /// A carrier entity holding one `Joint` between two bodies, plus any `extra` component.
    fn joint(&mut self, name: &str, a: usize, b: usize, kind: Value, extra: Option<(&str, Value)>) {
        let mut components = json!({
            "Joint": {
                "a": { "index": a, "generation": 0 },
                "b": { "index": b, "generation": 0 },
                "kind": kind,
                "anchor": v3([0.0; 3]),
                "collide": false,
            },
        });
        if let Some((key, value)) = extra {
            components[key] = value;
        }
        self.add(name, components);
    }

    fn text(&self) -> String {
        let mut text = String::from(
            "format: 1\nseed: 3\nphysics:\n  space: free\n  solver_iterations: 8\nentities:\n",
        );
        for entity in &self.0 {
            let mut block = String::new();
            emit(entity, 4, &mut block);
            text.push_str(&format!("  - {}", &block[4..]));
        }
        text
    }
}

fn cuboid(half: [f32; 3]) -> Value {
    json!({ "box": { "half_extents": v3(half) } })
}

/// `cols` brick columns by four rows (about 80 bodies at the default width).
fn scene(cols: usize) -> String {
    let mut scene = Scene(Vec::new());
    scene.body("floor", [0.0, -0.5, 0.0], cuboid([30.0, 0.5, 30.0]), false);
    // Bricks 1.0 x 0.5 x 0.5, each face touching its neighbours, glued by fixed joints.
    let mut bricks = vec![vec![0usize; cols]; 4];
    for (row, line) in bricks.iter_mut().enumerate() {
        for (col, slot) in line.iter_mut().enumerate() {
            let at = [col as f32, 0.25 + row as f32 * 0.5, 0.0];
            *slot = scene.body(
                &format!("brick-{row}-{col}"),
                at,
                cuboid([0.5, 0.25, 0.25]),
                true,
            );
        }
    }
    for row in 0..4 {
        for col in 0..cols {
            if col > 0 {
                let (a, b) = (bricks[row][col - 1], bricks[row][col]);
                scene.joint(
                    &format!("glue-h-{row}-{col}"),
                    a,
                    b,
                    json!({ "fixed": {} }),
                    None,
                );
            }
            if row > 0 {
                let (a, b) = (bricks[row - 1][col], bricks[row][col]);
                scene.joint(
                    &format!("glue-v-{row}-{col}"),
                    a,
                    b,
                    json!({ "fixed": {} }),
                    None,
                );
            }
        }
    }
    // Two-part roof tiles, flat on the wall's top face (y = 2.0).
    let tile = json!({ "compound": { "parts": [
        { "shape": cuboid([0.25, 0.05, 0.5]), "translation": v3([-0.25, 0.0, 0.0]) },
        { "shape": cuboid([0.25, 0.05, 0.5]), "translation": v3([0.25, 0.0, 0.0]) },
    ] } });
    for col in 0..cols {
        scene.body(
            &format!("tile-{col}"),
            [col as f32, 2.05, 0.0],
            tile.clone(),
            true,
        );
    }
    // A pile: three stacked boxes per column, a sphere and a capsule each resting on the floor.
    for column in 0..6 {
        let x = 10.0 + column as f32 * 1.5;
        for level in 0..3 {
            let at = [x, 0.25 + level as f32 * 0.5, 5.0];
            scene.body(
                &format!("crate-{column}-{level}"),
                at,
                cuboid([0.25; 3]),
                true,
            );
        }
        let ball = json!({ "sphere": { "radius": f(0.3) } });
        scene.body(&format!("ball-{column}"), [x, 0.3, 8.0], ball, true);
        let pill = json!({ "capsule": { "half_height": f(0.25), "radius": f(0.25) } });
        scene.body(&format!("pill-{column}"), [x, 0.5, 11.0], pill, true);
    }
    // A door on a motor-driven hinge, clear of everything.
    let post = scene.body(
        "post",
        [-10.0, 1.0, 0.0],
        json!({ "sphere": { "radius": f(0.05) } }),
        false,
    );
    let door = scene.body("door", [-9.5, 1.0, 0.0], cuboid([0.5, 0.5, 0.05]), true);
    let hinge = json!({ "hinge": { "axis": v3([0.0, 1.0, 0.0]) } });
    let motor = json!({ "target": [f(30.0)], "stiffness": f(39.5), "damping": f(6.3) });
    scene.joint("hinge", post, door, hinge, Some(("JointMotor", motor)));
    scene.text()
}

/// On frame 2, switches to `scene-b.yaml`'s `floor` anchor, carrying nothing.
fn switch_on_frame_two(world: &mut World, ctx: &SystemCtx) {
    if ctx.frame.as_u64() == 2
        && let Some(switch) = world.resource_mut::<SceneSwitch>()
    {
        switch.request("scene-b.yaml", "floor", vec![]);
    }
}

/// On frame 2, restarts the session's own scene.
fn restart_on_frame_two(world: &mut World, ctx: &SystemCtx) {
    if ctx.frame.as_u64() == 2
        && let Some(switch) = world.resource_mut::<SceneSwitch>()
    {
        switch.restart(vec![]);
    }
}

/// On tick 1 only, writes one body's `Velocity`: an input the prime never saw.
fn kick_on_tick_one(world: &mut World, ctx: &SystemCtx) {
    if ctx.frame.as_u64() != 0 {
        return;
    }
    let mut bodies: Vec<_> = world
        .query_ids::<(&Velocity,)>()
        .map(|(id, _)| id)
        .collect();
    bodies.sort();
    if let Ok(Some(velocity)) = world.get_mut::<Velocity>(bodies[bodies.len() / 2]) {
        velocity.linear = [0.0, 0.75, 0.0];
    }
}

/// On tick 1 only, edits one rigid body's friction on its `Collider`: the prime built that body from
/// the old value, so adopting it would simulate the wrong material.
fn slick_on_tick_one(world: &mut World, ctx: &SystemCtx) {
    if ctx.frame.as_u64() != 0 {
        return;
    }
    let mut bodies: Vec<_> = world
        .query_ids::<(&Collider,)>()
        .filter(|(_, (collider,))| matches!(collider.body, Body::Rigid(_)))
        .map(|(id, _)| id)
        .collect();
    bodies.sort();
    if let Ok(Some(collider)) = world.get_mut::<Collider>(bodies[bodies.len() / 2])
        && let Body::Rigid(rigid) = &mut collider.body
    {
        rigid.friction = 0.05;
    }
}

/// An engine rooted at a fresh directory holding `scene-a.yaml` and `scene-b.yaml`.
fn builder(name: &str) -> (EngineBuilder, PathBuf) {
    let dir = fresh_dir(name);
    std::fs::write(dir.join("scene-a.yaml"), scene(8)).unwrap();
    std::fs::write(dir.join("scene-b.yaml"), scene(6)).unwrap();
    (Engine::builder().project_root(&dir), dir)
}

fn loaded(builder: EngineBuilder) -> Engine {
    let mut engine = builder.build().expect("boots");
    engine
        .load_inline_scene(&scene(8))
        .expect("the scene loads");
    engine
}

/// What a run shows: one hash per tick, the contacts each tick reported, and whether each tick's
/// step adopted the prime.
#[derive(Debug, PartialEq)]
struct Trace {
    hashes: Vec<u64>,
    contacts: Vec<Vec<ContactEvent>>,
}

/// Runs `ticks` more ticks, returning the trace and the per-tick `last_step_primed`.
fn run(engine: &mut Engine, ticks: usize) -> (Trace, Vec<bool>) {
    let mut trace = Trace {
        hashes: Vec::new(),
        contacts: Vec::new(),
    };
    let mut primed = Vec::new();
    for _ in 0..ticks {
        engine.advance_live();
        trace.hashes.push(engine.world_hash().unwrap().as_u64());
        let physics = engine
            .world()
            .resource::<PhysicsWorld>()
            .expect("physics runs");
        trace.contacts.push(physics.contact_events().to_vec());
        primed.push(physics.last_step_primed());
    }
    (trace, primed)
}

/// Takes the prime out of a freshly rebuilt world: the cold twin of a primed run.
fn take_prime(engine: &mut Engine) -> PrimedPhysics {
    engine
        .world_mut()
        .remove_resource::<PrimedPhysics>()
        .expect("this rebuild path primes at commit")
}

/// The two twins of one rebuild path: `make` builds an engine just after the rebuild; the cold twin
/// drops its prime, the primed twin keeps it. Both run `TICKS` ticks and must agree exactly.
fn assert_twins(make: impl Fn() -> Engine) {
    let mut primed_engine = make();
    assert!(
        primed_engine.world().resource::<PrimedPhysics>().is_some(),
        "this rebuild path no longer primes: nothing to adopt at tick 1"
    );
    let mut cold_engine = make();
    drop(take_prime(&mut cold_engine));
    let (primed, flags) = run(&mut primed_engine, TICKS);
    let (cold, cold_flags) = run(&mut cold_engine, TICKS);
    assert!(flags[0], "the primed twin adopts the prime on tick 1");
    assert!(!flags[1..].iter().any(|&f| f), "only tick 1 is primed");
    assert!(!cold_flags.iter().any(|&f| f), "the cold twin never primes");
    assert_eq!(primed, cold, "a prime changed what tick 1.. computed");
    let distinct = {
        let mut hashes = primed.hashes.clone();
        hashes.sort_unstable();
        hashes.dedup();
        hashes.len()
    };
    assert!(
        distinct >= 30,
        "the scene is alive, not a repeated line ({distinct} distinct)"
    );
    assert!(
        primed.contacts.iter().any(|tick| !tick.is_empty()),
        "the scene has live contacts for the comparison to bite on"
    );
}

#[test]
fn a_primed_load_matches_a_cold_one_at_one_and_four_threads() {
    for threads in [1, 4] {
        par::with_threads(threads, || {
            assert_twins(|| loaded(builder(&format!("load-{threads}")).0));
        });
    }
}

#[test]
fn tick_one_contacts_are_identical() {
    let mut primed = loaded(builder("contacts-p").0);
    let mut cold = loaded(builder("contacts-c").0);
    drop(take_prime(&mut cold));
    let (primed_trace, _) = run(&mut primed, 1);
    let (cold_trace, _) = run(&mut cold, 1);
    assert_eq!(primed_trace.contacts, cold_trace.contacts);
    assert_eq!(primed_trace.hashes, cold_trace.hashes);
}

/// A game system that writes a body's `Velocity` on tick 1 changes an input the prime consumed, so
/// the write falls back to the cold build and the result is still the cold one.
#[test]
fn an_input_changed_on_tick_one_forces_the_cold_fallback() {
    let make = || loaded(builder("kick").0.system("kick", kick_on_tick_one));
    let mut primed_engine = make();
    assert!(primed_engine.world().resource::<PrimedPhysics>().is_some());
    let mut cold_engine = make();
    drop(take_prime(&mut cold_engine));
    let (primed, flags) = run(&mut primed_engine, TICKS);
    let (cold, _) = run(&mut cold_engine, TICKS);
    assert!(
        !flags.iter().any(|&f| f),
        "a changed input must discard the prime"
    );
    assert_eq!(primed, cold);

    // And the kick matters: the same run without it differs.
    let (untouched, _) = run(&mut loaded(builder("kick-control").0), TICKS);
    assert_ne!(primed.hashes, untouched.hashes);
}

/// The cold create path reads the entity's `Collider` itself, so an edit to it on tick 1 must send
/// the write down the cold path even though nothing else changed.
#[test]
fn a_collider_edited_on_tick_one_forces_the_cold_fallback() {
    let make = || loaded(builder("slick").0.system("slick", slick_on_tick_one));
    let mut primed_engine = make();
    assert!(primed_engine.world().resource::<PrimedPhysics>().is_some());
    let mut cold_engine = make();
    drop(take_prime(&mut cold_engine));
    let (primed, flags) = run(&mut primed_engine, TICKS);
    let (cold, _) = run(&mut cold_engine, TICKS);
    assert!(
        !flags.iter().any(|&f| f),
        "an edited Collider must discard the prime"
    );
    assert_eq!(primed, cold);

    let (untouched, _) = run(&mut loaded(builder("slick-control").0), TICKS);
    assert_ne!(primed.hashes, untouched.hashes, "the friction edit matters");
}

#[test]
fn a_load_from_a_file_primes() {
    assert_twins(|| {
        let (builder, dir) = builder("load-file");
        builder
            .load_scene_file(&dir.join("scene-a.yaml"))
            .expect("the scene file loads")
    });
}

#[test]
fn a_second_scene_load_primes() {
    assert_twins(|| {
        let (builder, _) = builder("reload");
        let mut engine = builder.build().expect("boots");
        let path = String::from("scene-a.yaml");
        engine
            .execute(Command::SceneLoad(SceneLoadParams { path: path.clone() }))
            .expect("the first load");
        for _ in 0..5 {
            engine.advance_live();
        }
        engine
            .execute(Command::SceneLoad(SceneLoadParams { path }))
            .expect("the reload");
        engine
    });
}

#[test]
fn a_switch_primes_the_arriving_scene() {
    assert_twins(|| {
        let (builder, _) = builder("switch");
        let mut engine = loaded(builder.system("switch", switch_on_frame_two));
        for _ in 0..3 {
            engine.advance_live();
        }
        assert!(engine.last_transition_error().is_none());
        engine
    });
}

#[test]
fn a_restart_switch_primes_the_rebuilt_scene() {
    assert_twins(|| {
        let (builder, _) = builder("restart");
        let mut engine = loaded(builder.system("restart", restart_on_frame_two));
        for _ in 0..3 {
            engine.advance_live();
        }
        assert!(engine.last_transition_error().is_none());
        engine
    });
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

/// A restore runs cold by construction: the restored world carries no `PrimedPhysics`, and its
/// continuation must be the uninterrupted run's, from a frame-0 snapshot and from a mid-run one.
#[test]
fn a_restore_runs_cold_and_continues_exactly() {
    let (uninterrupted, _) = run(&mut loaded(builder("restore-ref").0), TICKS);
    for at in [0, 30] {
        let mut source = loaded(builder(&format!("restore-src-{at}")).0);
        run_quiet(&mut source, at);
        let document = snapshot_of(&mut source);

        let mut target = loaded(builder(&format!("restore-dst-{at}")).0);
        restore(&mut target, document);
        assert!(
            target.world().resource::<PrimedPhysics>().is_none(),
            "a restore carries no prime"
        );
        let (resumed, flags) = run(&mut target, TICKS - at);
        assert!(
            !flags.iter().any(|&f| f),
            "a restored world never adopts a prime"
        );
        assert_eq!(
            resumed.hashes,
            uninterrupted.hashes[at..],
            "restore at {at}"
        );
        assert_eq!(
            resumed.contacts,
            uninterrupted.contacts[at..],
            "restore at {at}"
        );
    }
}

fn run_quiet(engine: &mut Engine, ticks: usize) {
    for _ in 0..ticks {
        engine.advance_live();
    }
}

/// A recorded session replays to its own hashes, and those are the cold run's: the replay's engine
/// loads the recorded scene, so it primes like a load.
#[test]
fn a_replay_of_a_primed_session_reproduces_its_hashes() {
    let (builder_for_record, dir) = builder("replay");
    let mut engine = loaded(builder_for_record);
    let path = dir.join("recording.lrp");
    engine
        .set_recording(std::fs::File::create(&path).expect("the recording is writable"))
        .expect("recording arms");
    let live = ludion_engine::drive_hashes(
        &mut engine,
        vec![Command::SimStep(SimStepParams {
            ticks: TICKS as u64,
            hash: false,
        })],
    )
    .expect("the session runs");
    engine.stop_recording().expect("recording flushes");

    let bytes = std::fs::read(&path).unwrap();
    let replayed =
        ludion_engine::replay_hashes_with(Engine::builder().project_root(&dir), &bytes, &dir)
            .expect("the recording replays");
    let replayed: Vec<u64> = replayed.iter().map(|(_, hash)| hash.as_u64()).collect();
    let live: Vec<u64> = live.iter().map(|(_, hash)| hash.as_u64()).collect();
    assert_eq!(replayed, live);

    let mut cold = loaded(Engine::builder().project_root(&dir));
    drop(take_prime(&mut cold));
    let (cold_trace, _) = run(&mut cold, TICKS);
    assert_eq!(live[1..], cold_trace.hashes[..]);
}
