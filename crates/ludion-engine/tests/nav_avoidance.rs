//! Agent avoidance: `NavAgent.avoidance` swaps separation for an ORCA velocity. Head-on passes, priority,
//! classes, blockers, identity for agents without it, thread-count invariance, restore and `L1528`.

use ludion_core::par;
use ludion_engine::{Engine, Nav, NavAgent, NavClass, NavConfig, Transform};
use ludion_nav::NodeMark;
use ludion_protocol::Command;
use ludion_protocol::command::EngineRestoreParams;

use crate::nav::{
    agent, config, ground, mover, run, scene, seq, snapshot_of, steering_builder, target_yaml,
    trace,
};

/// A walking avoider heading for `goal`; `avoidance` is the body of the `avoidance:` map (already indented
/// by 10 spaces, empty for `{}`).
fn avoider(n: usize, at: [f32; 3], goal: [f32; 3], speed: f32, avoidance: &str) -> String {
    let avoidance = if avoidance.is_empty() {
        "        avoidance: {}\n".to_string()
    } else {
        format!("        avoidance:\n{avoidance}")
    };
    let fields = format!(
        "        speed: {speed:?}\n        neighbor_layers: 2\n{}{avoidance}",
        target_yaml(goal)
    );
    agent(n, at, &fields)
}

fn build_with(config: NavConfig, yaml: &str) -> Engine {
    let mut engine = steering_builder()
        .nav(config)
        .system("mover", mover)
        .build()
        .expect("boots");
    engine.load_inline_scene(yaml).unwrap();
    engine
}

fn open_room(agents: &str) -> String {
    scene(&[ground()]) + agents
}

fn positions(engine: &mut Engine) -> Vec<[f32; 3]> {
    let mut all: Vec<_> = engine
        .world_mut()
        .query_ids::<(&Transform, &NavAgent)>()
        .map(|(id, (t, _))| (id, t.translation))
        .collect();
    all.sort_by_key(|(id, _)| *id);
    all.into_iter().map(|(_, p)| p).collect()
}

fn dist_xz(a: [f32; 3], b: [f32; 3]) -> f32 {
    ludion_core::math::hypot(a[0] - b[0], a[2] - b[2])
}

/// Runs `ticks`, returning the smallest centre distance between agents 0 and 1 and each one's greatest
/// z deviation from its start.
fn cross(engine: &mut Engine, ticks: usize) -> (f32, [f32; 2]) {
    let start = positions(engine);
    let mut min = f32::MAX;
    let mut deviation = [0.0f32; 2];
    for _ in 0..ticks {
        engine.advance_live();
        let now = positions(engine);
        min = min.min(dist_xz(now[0], now[1]));
        for i in 0..2 {
            deviation[i] = deviation[i].max((now[i][2] - start[i][2]).abs());
        }
    }
    (min, deviation)
}

const LEFT: [f32; 3] = [3.0, 0.0, 12.5];
const RIGHT: [f32; 3] = [21.0, 0.0, 12.5];

fn head_on(a: &str, b: &str) -> String {
    open_room(&(avoider(0, LEFT, RIGHT, 6.0, a) + &avoider(1, RIGHT, LEFT, 6.0, b)))
}

#[test]
fn two_walkers_pass_head_on_and_arrive() {
    let radius = "          radius: 0.5\n";
    let mut engine = build_with(config(), &head_on(radius, radius));
    let (min, _) = cross(&mut engine, 400);
    assert!(min >= 1.0 - 0.05, "minimum centre distance {min}");
    let end = positions(&mut engine);
    assert!(
        dist_xz(end[0], RIGHT) < 1.5 && dist_xz(end[1], LEFT) < 1.5,
        "{end:?}"
    );
}

#[test]
fn the_higher_priority_agent_deviates_less() {
    let radius = "          radius: 0.5\n";
    let high = format!("{radius}          priority: 1\n");
    let mut engine = build_with(config(), &head_on(&high, radius));
    let (min, deviation) = cross(&mut engine, 400);
    assert!(min >= 1.0 - 0.05, "minimum centre distance {min}");
    assert!(
        deviation[0] < deviation[1],
        "priority 1 deviated {} and priority 0 {}",
        deviation[0],
        deviation[1]
    );
}

#[test]
fn the_default_radius_is_the_class_radius() {
    let class = |name: &str, radius: f32| NavClass {
        name: name.into(),
        radius,
        ..NavClass::default()
    };
    let config = NavConfig {
        classes: vec![class("walker", 0.4), class("forklift", 1.0)],
        ..config()
    };
    let forklift = avoider(0, LEFT, RIGHT, 6.0, "")
        .replace("        speed:", "        class: forklift\n        speed:");
    let walker = avoider(1, RIGHT, LEFT, 6.0, "")
        .replace("        speed:", "        class: walker\n        speed:");
    let mut engine = build_with(config, &open_room(&(forklift + &walker)));
    let (min, _) = cross(&mut engine, 400);
    assert!(
        min >= 1.4 - 0.05,
        "a forklift and a walker stayed {min} apart"
    );
}

#[test]
fn an_avoider_beside_a_blocker_wall_never_stands_on_a_blocked_node() {
    let wall = "  - name: wall\n    components:\n      Transform:\n        translation:\n"
        .to_string()
        + &seq([12.0, 1.0, 5.0], 10)
        + "      NavBlocker:\n        shape:\n          box:\n            half_extents:\n"
        + &seq([0.5, 1.0, 5.0], 14);
    let radius = "          radius: 0.5\n";
    let yaml = head_on(radius, radius).replace("12.5\n", "11.0\n") + &wall;
    let mut engine = build_with(config(), &yaml);
    run(&mut engine, 2);
    for tick in 0..400 {
        engine.advance_live();
        let positions = positions(&mut engine);
        let grid = engine.world().resource::<Nav>().unwrap().grid().unwrap();
        for p in positions {
            if let Some(node) = grid.node_at([p[0], p[1] + 0.6, p[2]]) {
                assert_ne!(grid.mark(node), NodeMark::Block, "tick {tick}: {p:?}");
            }
        }
    }
}

fn crowd(extra: &str) -> String {
    let mut agents = String::new();
    for n in 0..8 {
        let at = [4.0 + 1.0 * (n % 4) as f32, 0.0, 4.0 + 1.0 * (n / 4) as f32];
        let fields = format!(
            "        speed: 3.0\n        neighbor_layers: 2\n{}",
            target_yaml([6.0, 0.0, 8.0])
        );
        agents += &agent(n, at, &fields);
    }
    open_room(&(agents + extra))
}

#[test]
fn agents_without_avoidance_ignore_a_far_avoider() {
    let far = avoider(99, [22.0, 0.0, 22.0], [22.0, 0.0, 2.0], 3.0, "");
    let mut alone = build_with(config(), &crowd(""));
    let mut with = build_with(config(), &crowd(&far));
    run(&mut alone, 120);
    run(&mut with, 120);
    let a = positions(&mut alone);
    let b = positions(&mut with);
    assert_eq!(a[..], b[..a.len()], "separation-only agents are unchanged");
}

fn crossing() -> Engine {
    let mut agents = String::new();
    for n in 0..6 {
        let z = 8.0 + 1.6 * n as f32;
        agents += &avoider(n, [3.0, 0.0, z], [21.0, 0.0, z + 0.05], 5.0, "");
        agents += &avoider(n + 6, [21.0, 0.0, z + 0.8], [3.0, 0.0, z + 0.75], 5.0, "");
    }
    build_with(config(), &open_room(&agents))
}

#[test]
fn a_crossing_is_the_same_at_any_thread_count() {
    let one = par::with_threads(1, || trace(&mut crossing(), 120));
    let many = par::with_threads(8, || trace(&mut crossing(), 120));
    assert_eq!(one, many);
    assert_ne!(one[0], one[119], "the crowd moved");
}

#[test]
fn a_restore_mid_crossing_rejoins_the_trace() {
    let whole = trace(&mut crossing(), 160);
    let mut first = crossing();
    trace(&mut first, 70);
    let snapshot = snapshot_of(&mut first);
    let mut resumed = crossing();
    resumed
        .execute(Command::EngineRestore(EngineRestoreParams { snapshot }))
        .unwrap();
    assert_eq!(trace(&mut resumed, 90), whole[70..]);
}

#[test]
fn invalid_avoidance_is_refused_with_l1528() {
    for bad in [
        "        avoidance:\n          radius: 0.0\n",
        "        avoidance:\n          horizon_s: -1.0\n",
        "        avoidance:\n          max_neighbors: 17\n",
        "        avoidance:\n          max_neighbors: 0\n",
        "        flying: true\n        avoidance: {}\n",
    ] {
        let yaml = format!(
            "{}  - name: bad\n    components:\n      NavAgent:\n{bad}",
            scene(&[ground()])
        );
        let mut engine = steering_builder().nav(config()).build().unwrap();
        let error = engine.load_inline_scene(&yaml).unwrap_err();
        assert!(format!("{error:?}").contains("L1528"), "{bad}: {error:?}");
    }
}
