//! The engine hosts the nav grid: sampling from static colliders, incremental rebuilds, derived-state
//! rules (hash, snapshot, threads) and config validation.

use ludion_core::codes::code;
use ludion_core::par;
use ludion_ecs::World;
use ludion_engine::diagnostics::Diagnostics;
use ludion_engine::{
    Engine, Nav, NavAgent, NavConfig, NavSteering, Transform, TransformHierarchy, nav_steer,
    nav_update,
};
use ludion_nav::{NEIGHBORS, NavGrid, NodeId};
use ludion_protocol::command::EngineRestoreParams;
use ludion_protocol::{Command, ResponseBody};
use ludion_spatial::{Body, Collider, Shape, spatial3d};
use serde_json::Value;

/// One static box: centre, half extents, Euler rotation in degrees.
struct Block {
    at: [f32; 3],
    half: [f32; 3],
    rot: [f32; 3],
}

fn block(at: [f32; 3], half: [f32; 3]) -> Block {
    Block {
        at,
        half,
        rot: [0.0; 3],
    }
}

/// A 24 x 24 m ground slab whose top is at y = 0.
fn ground() -> Block {
    block([12.0, -0.5, 12.0], [12.0, 0.5, 12.0])
}

/// A 3-vector as a block-style YAML sequence at `indent` spaces.
fn seq(v: [f32; 3], indent: usize) -> String {
    v.iter()
        .map(|c| format!("{:indent$}- {c:?}\n", ""))
        .collect()
}

fn scene(blocks: &[Block]) -> String {
    let mut out = String::from("format: 1\nseed: 1\nphysics:\n  space: free\nentities:\n");
    for (n, b) in blocks.iter().enumerate() {
        out +=
            &format!("  - name: b{n}\n    components:\n      Transform:\n        translation:\n");
        out += &seq(b.at, 10);
        out += "        rotation:\n";
        out += &seq(b.rot, 10);
        out += "      Collider:\n        shape:\n          box:\n            half_extents:\n";
        out += &seq(b.half, 14);
        out += "        body: static\n";
    }
    out
}

fn config() -> NavConfig {
    NavConfig {
        nx: 24,
        nz: 24,
        y_min: -2.0,
        y_max: 8.0,
        ..NavConfig::default()
    }
}

fn engine(config: NavConfig, blocks: &[Block]) -> Engine {
    let mut engine = Engine::builder()
        .nav(config)
        .system("nav_update", nav_update)
        .build()
        .expect("boots");
    engine.load_inline_scene(&scene(blocks)).unwrap();
    engine
}

fn run(engine: &mut Engine, ticks: usize) {
    for _ in 0..ticks {
        engine.advance_live();
    }
}

fn nav(engine: &Engine) -> &Nav {
    engine.world().resource::<Nav>().unwrap()
}

fn grid(engine: &Engine) -> &NavGrid {
    nav(engine).grid().expect("the grid is built")
}

/// The grid after the two ticks it takes to see the physics world.
fn built(config: NavConfig, blocks: &[Block]) -> NavGrid {
    let mut engine = engine(config, blocks);
    run(&mut engine, 2);
    grid(&engine).clone()
}

fn assert_same(a: &NavGrid, b: &NavGrid) {
    assert_eq!(a.params(), b.params());
    assert_eq!(a.node_count(), b.node_count());
    for n in (0..a.node_count() as u32).map(NodeId) {
        assert_eq!(a.node_pos(n), b.node_pos(n), "node {n:?}");
        for dir in 0..NEIGHBORS.len() {
            assert_eq!(a.link(n, dir), b.link(n, dir), "node {n:?} dir {dir}");
            assert_eq!(
                a.link_cost(n, dir),
                b.link_cost(n, dir),
                "node {n:?} dir {dir}"
            );
        }
    }
}

fn y_at(grid: &NavGrid, p: [f32; 3]) -> Option<f32> {
    grid.node_at(p).map(|n| grid.node_pos(n)[1])
}

fn near(y: Option<f32>, want: f32) -> bool {
    y.is_some_and(|y| (y - want).abs() < 1e-3)
}

/// A lower floor, a 2 m deck from x = 14, and a 30-degree ramp 4 cells wide joining them.
fn ramp_room() -> Vec<Block> {
    vec![
        ground(),
        block([19.0, 1.5, 12.0], [5.0, 0.5, 12.0]),
        Block {
            at: [12.0, 0.913, 12.0],
            half: [2.0, 0.1, 2.0],
            rot: [0.0, 0.0, 30.0],
        },
    ]
}

#[test]
fn a_ramp_joins_two_floors_and_the_step_face_does_not() {
    let mut engine = engine(config(), &ramp_room());
    assert!(nav(&engine).grid().is_none());
    run(&mut engine, 2);
    let (grid, cache) = engine.world_mut().resource_mut::<Nav>().unwrap().parts();
    let grid = grid.expect("two ticks build the grid");
    let target = grid.node_at([20.5, 2.0, 12.5]).expect("a deck node");
    assert_eq!(grid.node_pos(target)[1], 2.0);
    let field = cache.field(grid, target);

    let low = grid.node_at([3.5, 0.0, 12.5]).unwrap();
    assert!(
        field.distance(low).is_finite(),
        "the lower floor reaches the deck"
    );
    assert!(field.flow(low)[0] > 0.0, "and flows toward the ramp");

    let beside = grid
        .node_at([13.5, 0.0, 3.5])
        .expect("ground by the step face");
    assert_eq!(grid.link(beside, 0), None, "a 2 m step is not a link");
    assert!(
        field.distance(beside) > 8.0,
        "it goes around by the ramp: {}",
        field.distance(beside)
    );
}

#[test]
fn a_ledge_with_no_ramp_is_unreachable() {
    let blocks = [ground(), block([12.0, 0.5, 12.0], [2.0, 0.5, 12.0])];
    let mut engine = engine(config(), &blocks);
    run(&mut engine, 2);
    let (grid, cache) = engine.world_mut().resource_mut::<Nav>().unwrap().parts();
    let grid = grid.unwrap();
    let top = grid.node_at([12.5, 1.0, 12.5]).expect("the block top");
    assert_eq!(grid.node_pos(top)[1], 1.0);
    let field = cache.field(grid, top);
    let floor = grid.node_at([3.5, 0.0, 12.5]).unwrap();
    assert!(field.distance(floor).is_infinite());
}

#[test]
fn a_bridge_keeps_the_ground_under_it_only_with_headroom() {
    let high = built(
        config(),
        &[ground(), block([12.0, 3.0, 12.0], [4.0, 0.2, 12.0])],
    );
    assert_eq!(
        y_at(&high, [12.5, 0.0, 12.5]),
        Some(0.0),
        "ground under a high bridge"
    );
    assert!(near(y_at(&high, [12.5, 3.2, 12.5]), 3.2), "the bridge top");
    assert_eq!(
        y_at(&high, [12.5, 2.7, 12.5]),
        Some(0.0),
        "the slab underside is never a node"
    );

    let low = built(
        config(),
        &[ground(), block([12.0, 1.2, 12.0], [4.0, 0.2, 12.0])],
    );
    assert_eq!(
        y_at(&low, [12.5, 0.0, 12.5]),
        None,
        "1 m of headroom is too little"
    );
    assert!(near(y_at(&low, [12.5, 1.4, 12.5]), 1.4));
}

#[test]
fn walls_inflate_by_the_agent_radius() {
    let config = NavConfig {
        agent_radius: 0.6,
        ..config()
    };
    let grid = built(
        config,
        &[ground(), block([12.5, 1.5, 12.0], [0.5, 1.5, 12.0])],
    );
    assert_eq!(y_at(&grid, [11.5, 0.0, 5.5]), None, "0.5 m from the wall");
    assert_eq!(y_at(&grid, [13.5, 0.0, 5.5]), None, "0.5 m from the wall");
    assert_eq!(
        y_at(&grid, [10.5, 0.0, 5.5]),
        Some(0.0),
        "1.5 m from the wall"
    );
}

fn add_wall(engine: &mut Engine, at: [f32; 3]) -> ludion_core::ids::EntityId {
    let world: &mut World = engine.world_mut();
    let id = world.spawn();
    world
        .insert(
            id,
            Transform {
                translation: at,
                ..Transform::default()
            },
        )
        .unwrap();
    let collider = Collider {
        shape: Shape::Box {
            half_extents: [0.5, 1.5, 4.0],
        },
        body: Body::new_static(),
        layer: None,
        sensor: false,
    };
    world.insert(id, collider).unwrap();
    id
}

fn wall(at: [f32; 3]) -> Block {
    block(at, [0.5, 1.5, 4.0])
}

#[test]
fn static_changes_rebuild_only_what_they_touch_and_match_a_fresh_build() {
    let mut engine = engine(config(), &[ground()]);
    run(&mut engine, 2);
    assert_eq!(nav(&engine).rebuilds(), 1);
    run(&mut engine, 3);
    assert_eq!(nav(&engine).rebuilds(), 1, "an idle tick does not rebuild");

    let id = add_wall(&mut engine, [10.5, 1.5, 12.0]);
    run(&mut engine, 2);
    assert_eq!(nav(&engine).rebuilds(), 2);
    assert_same(
        grid(&engine),
        &built(config(), &[ground(), wall([10.5, 1.5, 12.0])]),
    );

    engine
        .world_mut()
        .get_mut::<Transform>(id)
        .unwrap()
        .unwrap()
        .translation = [16.5, 1.5, 6.0];
    run(&mut engine, 2);
    assert_eq!(nav(&engine).rebuilds(), 3);
    assert_same(
        grid(&engine),
        &built(config(), &[ground(), wall([16.5, 1.5, 6.0])]),
    );

    engine.world_mut().despawn(id).unwrap();
    run(&mut engine, 2);
    assert_eq!(nav(&engine).rebuilds(), 4);
    assert_same(grid(&engine), &built(config(), &[ground()]));

    run(&mut engine, 2);
    assert_eq!(nav(&engine).rebuilds(), 4);
    engine
        .world_mut()
        .resource_mut::<Nav>()
        .unwrap()
        .request_full_rebuild();
    run(&mut engine, 1);
    assert_eq!(nav(&engine).rebuilds(), 5);
}

#[test]
fn the_grid_is_the_same_at_any_thread_count() {
    let one = par::with_threads(1, || built(config(), &ramp_room()));
    let many = par::with_threads(8, || built(config(), &ramp_room()));
    assert_same(&one, &many);
}

#[test]
fn nav_is_invisible_to_the_world_hash() {
    let blocks = [ground(), wall([10.5, 1.5, 12.0])];
    let trace = |engine: &mut Engine| -> Vec<u64> {
        (0..20)
            .map(|_| {
                engine.advance_live();
                engine.world_hash().unwrap().as_u64()
            })
            .collect()
    };
    let mut plain = Engine::builder().build().unwrap();
    plain.load_inline_scene(&scene(&blocks)).unwrap();
    let mut with_nav = engine(config(), &blocks);
    assert_eq!(trace(&mut with_nav), trace(&mut plain));
    assert!(nav(&with_nav).grid().is_some());
}

fn snapshot_of(engine: &mut Engine) -> Value {
    match engine.execute(Command::EngineSnapshot).unwrap().body {
        ResponseBody::Snapshot { snapshot } => snapshot,
        other => panic!("expected a snapshot, got {other:?}"),
    }
}

#[test]
fn a_restored_world_rebuilds_the_same_grid() {
    let blocks = ramp_room();
    let mut uninterrupted = engine(config(), &blocks);
    run(&mut uninterrupted, 11);

    let mut first = engine(config(), &blocks);
    run(&mut first, 10);
    let snapshot = snapshot_of(&mut first);
    assert!(snapshot.get("nav").is_none(), "nav is derived state");

    let mut resumed = engine(config(), &blocks);
    resumed
        .execute(Command::EngineRestore(EngineRestoreParams { snapshot }))
        .unwrap();
    assert!(
        nav(&resumed).grid().is_none(),
        "a restore starts with a fresh nav"
    );
    run(&mut resumed, 1);
    assert_same(grid(&resumed), grid(&uninterrupted));
}

#[test]
fn an_unknown_ignore_layer_is_reported_once_and_keeps_the_grid_empty() {
    let config = NavConfig {
        ignore_layers: vec!["nope".to_string()],
        ..config()
    };
    let mut engine = engine(config, &[ground()]);
    run(&mut engine, 4);
    assert!(nav(&engine).grid().is_none());
    let diagnostics = engine.world().resource::<Diagnostics>().unwrap();
    assert_eq!(
        diagnostics
            .reported(code::NAV_CONFIG_INVALID)
            .collect::<Vec<_>>(),
        ["ignore_layers"]
    );
}

#[test]
fn a_bad_config_names_its_field() {
    let cases: [(&str, NavConfig); 12] = [
        (
            "/origin/1",
            NavConfig {
                origin: [0.0, f32::NAN],
                ..config()
            },
        ),
        (
            "/cell_size",
            NavConfig {
                cell_size: 0.0,
                ..config()
            },
        ),
        ("/nx", NavConfig { nx: 0, ..config() }),
        (
            "/nz",
            NavConfig {
                nx: 4096,
                nz: 4097,
                ..config()
            },
        ),
        (
            "/y_min",
            NavConfig {
                y_min: 8.0,
                ..config()
            },
        ),
        (
            "/agent_radius",
            NavConfig {
                agent_radius: -1.0,
                ..config()
            },
        ),
        (
            "/agent_height",
            NavConfig {
                agent_height: f32::INFINITY,
                ..config()
            },
        ),
        (
            "/max_step",
            NavConfig {
                max_step: -0.1,
                ..config()
            },
        ),
        (
            "/max_slope_deg",
            NavConfig {
                max_slope_deg: 90.0,
                ..config()
            },
        ),
        (
            "/flow_radius",
            NavConfig {
                flow_radius: 0.0,
                ..config()
            },
        ),
        (
            "/ignore_layers/1",
            NavConfig {
                ignore_layers: vec!["a".into(), String::new()],
                ..config()
            },
        ),
        (
            "/y_min",
            NavConfig {
                y_max: f32::NAN,
                ..config()
            },
        ),
    ];
    for (path, bad) in cases {
        let error = bad.validate().expect_err(path);
        assert_eq!(error.code, code::NAV_CONFIG_INVALID, "{path}");
        assert_eq!(error.path.as_deref(), Some(path), "{error:?}");
        assert!(Engine::builder().nav(bad).build().is_err(), "{path}");
    }
}

// --- Agents: `NavAgent` -> `nav_steer` -> `NavSteering` -------------------------------------------

const DECK: [f32; 3] = [20.5, 2.0, 12.5];

/// One agent entity: `fields` is the `NavAgent` body, already indented by 8 spaces.
fn agent(n: usize, at: [f32; 3], fields: &str) -> String {
    let mut out =
        format!("  - name: a{n}\n    components:\n      Transform:\n        translation:\n");
    out += &seq(at, 10);
    out += "      SpatialVolume:\n        layers: 2\n      NavAgent:\n";
    out += fields;
    out
}

fn target_yaml(target: [f32; 3]) -> String {
    format!("        target:\n{}", seq(target, 10))
}

/// Thirty walkers on the low floor, all heading for the deck.
fn crowd(blocks: &[Block]) -> String {
    let mut out = scene(blocks);
    let fields = format!(
        "        speed: 6.0\n        neighbor_layers: 2\n{}",
        target_yaml(DECK)
    );
    for n in 0..30 {
        let at = [2.0 + 1.2 * (n % 6) as f32, 0.0, 8.0 + 1.2 * (n / 6) as f32];
        out += &agent(n, at, &fields);
    }
    out
}

/// Applies each agent's steering: `xz += velocity * dt`, `y` from the grid node under it.
fn mover(world: &mut World, ctx: &ludion_ecs::SystemCtx) {
    let moves: Vec<_> = world
        .query_ids::<(&Transform, &NavSteering)>()
        .map(|(id, (t, s))| (id, t.translation, s.velocity))
        .collect();
    for (id, at, v) in moves {
        let mut next = [at[0] + v[0] * ctx.dt, at[1], at[2] + v[2] * ctx.dt];
        if let Some(grid) = world.resource::<Nav>().and_then(Nav::grid) {
            // Probe a little above the agent so a ramp cell a slope-widened step up is found.
            if let Some(node) = grid.node_at([next[0], at[1] + 0.6, next[2]]) {
                next[1] = grid.node_pos(node)[1];
            }
        }
        world.get_mut::<Transform>(id).unwrap().unwrap().translation = next;
    }
}

fn steering_builder() -> ludion_engine::EngineBuilder {
    Engine::builder()
        .system(
            "spatial3d_rebuild",
            spatial3d::rebuild::<Transform, TransformHierarchy>,
        )
        .system("nav_update", nav_update)
        .system("nav_steer", nav_steer)
}

fn chase_engine() -> Engine {
    let mut engine = steering_builder()
        .nav(config())
        .system("mover", mover)
        .build()
        .expect("boots");
    engine.load_inline_scene(&crowd(&ramp_room())).unwrap();
    engine
}

fn trace(engine: &mut Engine, ticks: usize) -> Vec<u64> {
    (0..ticks)
        .map(|_| {
            engine.advance_live();
            engine.world_hash().unwrap().as_u64()
        })
        .collect()
}

fn velocities(engine: &mut Engine) -> Vec<[f32; 3]> {
    let mut all: Vec<_> = engine
        .world_mut()
        .query_ids::<(&NavSteering,)>()
        .map(|(id, (s,))| (id, s.velocity))
        .collect();
    all.sort_by_key(|(id, _)| *id);
    all.into_iter().map(|(_, v)| v).collect()
}

fn computes(engine: &mut Engine) -> u64 {
    engine
        .world_mut()
        .resource_mut::<Nav>()
        .unwrap()
        .cache_mut()
        .computes()
}

#[test]
fn a_crowd_reaches_the_deck_and_repeats_bit_for_bit() {
    let mut first = chase_engine();
    let a = trace(&mut first, 300);
    let b = trace(&mut chase_engine(), 300);
    assert_eq!(a, b, "two fresh runs give the same per-tick hashes");
    assert_ne!(a[0], a[299], "the crowd moved");
    let arrived = first
        .world_mut()
        .query_ids::<(&Transform, &NavAgent)>()
        .filter(|(_, (t, _))| {
            let d = [t.translation[0] - DECK[0], t.translation[2] - DECK[2]];
            (d[0] * d[0] + d[1] * d[1]).sqrt() < 2.0 && (t.translation[1] - 2.0).abs() < 1e-3
        })
        .count();
    assert!(
        arrived >= 25,
        "{arrived} of 30 reached the deck by the ramp"
    );
}

#[test]
fn a_restore_mid_chase_continues_the_same_trace() {
    let whole = trace(&mut chase_engine(), 200);

    let mut first = chase_engine();
    trace(&mut first, 80);
    let snapshot = snapshot_of(&mut first);
    let mut resumed = chase_engine();
    resumed
        .execute(Command::EngineRestore(EngineRestoreParams { snapshot }))
        .unwrap();
    let tail = trace(&mut resumed, 120);
    assert_eq!(tail, whole[80..], "the restored chase matches from tick 81");
}

#[test]
fn the_chase_is_the_same_at_any_thread_count() {
    let one = par::with_threads(1, || trace(&mut chase_engine(), 120));
    let many = par::with_threads(8, || trace(&mut chase_engine(), 120));
    assert_eq!(one, many);
}

#[test]
fn agents_share_one_field_per_target_cell() {
    let mut engine = steering_builder().nav(config()).build().expect("boots");
    let mut yaml = scene(&ramp_room());
    for n in 0..30 {
        yaml += &agent(
            n,
            [2.0 + 0.5 * n as f32 % 8.0, 0.0, 8.0],
            &format!("        neighbor_layers: 0\n{}", target_yaml(DECK)),
        );
    }
    engine.load_inline_scene(&yaml).unwrap();
    run(&mut engine, 3);
    assert_eq!(computes(&mut engine), 1, "30 agents, one target cell");

    let other = [4.5, 0.0, 20.5];
    let ids: Vec<_> = engine
        .world_mut()
        .query_ids::<(&NavAgent,)>()
        .map(|(id, _)| id)
        .collect();
    for (n, id) in ids.iter().enumerate() {
        if n % 2 == 0 {
            engine
                .world_mut()
                .get_mut::<NavAgent>(*id)
                .unwrap()
                .unwrap()
                .target = other;
        }
    }
    run(&mut engine, 1);
    assert_eq!(computes(&mut engine), 2, "a second cell, a second field");

    for id in &ids {
        let agent = engine
            .world_mut()
            .get_mut::<NavAgent>(*id)
            .unwrap()
            .unwrap();
        if agent.target == other {
            agent.target = [4.9, 0.0, 20.1];
        }
    }
    run(&mut engine, 3);
    assert_eq!(
        computes(&mut engine),
        2,
        "moving inside the cell recomputes nothing"
    );
}

fn lone_scene(agents: &[(usize, [f32; 3], String)], blocks: &[Block]) -> String {
    let mut yaml = scene(blocks);
    for (n, at, fields) in agents {
        yaml += &agent(*n, *at, fields);
    }
    yaml
}

#[test]
fn a_flyer_heads_straight_for_its_target_with_or_without_nav() {
    let fields = format!(
        "        flying: true\n        speed: 5.0\n{}",
        target_yaml([10.0, 1.0, 12.0])
    );
    let yaml = lone_scene(
        &[(0, [4.0, 5.0, 12.0], fields)],
        &[ground(), block([7.0, 2.0, 12.0], [0.5, 2.0, 4.0])],
    );
    for with_nav in [false, true] {
        let builder = if with_nav {
            steering_builder().nav(config())
        } else {
            Engine::builder()
                .system(
                    "spatial3d_rebuild",
                    spatial3d::rebuild::<Transform, TransformHierarchy>,
                )
                .system("nav_steer", nav_steer)
        };
        let mut engine = builder.build().expect("boots");
        engine.load_inline_scene(&yaml).unwrap();
        run(&mut engine, 3);
        let v = velocities(&mut engine)[0];
        let want = [6.0 / 52f32.sqrt() * 5.0, -4.0 / 52f32.sqrt() * 5.0, 0.0];
        for axis in 0..3 {
            assert!(
                (v[axis] - want[axis]).abs() < 1e-4,
                "nav {with_nav}: {v:?} vs {want:?}"
            );
        }
    }
}

#[test]
fn neighbours_push_apart_and_layer_zero_turns_it_off() {
    let pair = |layers: u32| {
        let fields = format!(
            "        arrive_radius: 100.0\n        neighbor_layers: {layers}\n{}",
            target_yaml([10.0, 0.0, 12.0])
        );
        let yaml = lone_scene(
            &[
                (0, [10.0, 0.0, 12.0], fields.clone()),
                (1, [10.4, 0.0, 12.0], fields),
            ],
            &[ground()],
        );
        let mut engine = steering_builder().nav(config()).build().expect("boots");
        engine.load_inline_scene(&yaml).unwrap();
        run(&mut engine, 3);
        velocities(&mut engine)
    };
    let on = pair(2);
    assert!(on[0][0] < 0.0 && on[1][0] > 0.0, "opposite pushes: {on:?}");
    assert!((on[0][0] + on[1][0]).abs() < 1e-5);
    assert_eq!((on[0][1], on[0][2]), (0.0, 0.0));
    assert_eq!(pair(0), vec![[0.0; 3]; 2]);
}

#[test]
fn walkers_are_still_before_the_grid_exists() {
    let mut engine = chase_engine();
    run(&mut engine, 1);
    assert!(nav(&engine).grid().is_none());
    let v = velocities(&mut engine);
    assert_eq!(v.len(), 30);
    assert!(v.iter().all(|v| *v == [0.0; 3]));
}
