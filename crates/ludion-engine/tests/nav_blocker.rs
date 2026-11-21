//! Dynamic blockers: `NavBlocker` overlays on the class grids (block, cost, interact), routed walkers stopping
//! `at_blocker`, replans on a toggle, classes, a moving blocker, identity without blockers, thread-count
//! invariance and restore.

use ludion_core::codes::code;
use ludion_core::par;
use ludion_engine::{
    Engine, Nav, NavBlocker, NavBlockerMode, NavClass, NavConfig, NavRoute, NavRouteStatus,
    NavSteering, PathRequest, PathStatus, Transform, nav_path, nav_reachable,
};
use ludion_nav::{NEIGHBORS, NodeId};
use ludion_protocol::Command;
use ludion_protocol::command::EngineRestoreParams;

use crate::nav::{
    Block, agent, block, config, ground, mover, run, scene, seq, snapshot_of, steering_builder,
    trace,
};

const START: [f32; 3] = [3.0, 0.0, 12.5];
const GOAL: [f32; 3] = [20.5, 0.0, 12.5];

/// A wall across x = 12 with a 3 m door gap at z = 11..14, and a second gap at z = 2..5 when asked.
fn rooms(second_gap: bool) -> Vec<Block> {
    let mut blocks = vec![ground(), block([12.0, 1.5, 19.0], [0.5, 1.5, 5.0])];
    if second_gap {
        blocks.push(block([12.0, 1.5, 1.0], [0.5, 1.5, 1.0]));
        blocks.push(block([12.0, 1.5, 8.0], [0.5, 1.5, 3.0]));
    } else {
        blocks.push(block([12.0, 1.5, 5.5], [0.5, 1.5, 5.5]));
    }
    blocks
}

/// A blocker sealing the door gap (the radius inflation covers the gap's cells).
fn door(mode: &str, extra: &str) -> String {
    format!(
        "  - name: door\n    components:\n      Transform:\n        translation:\n{}      NavBlocker:\n        shape:\n          box:\n            half_extents:\n{}        mode: {mode}\n{extra}",
        seq([12.0, 1.0, 12.5], 10),
        seq([0.6, 1.0, 1.5], 14)
    )
}

fn walker(n: usize, at: [f32; 3], goal: [f32; 3], class: &str) -> String {
    let class = if class.is_empty() {
        String::new()
    } else {
        format!("        class: {class}\n")
    };
    let fields = format!("{class}        neighbor_layers: 0\n        speed: 6.0\n");
    agent(n, at, &fields) + &format!("      NavGoal:\n        at:\n{}", seq(goal, 10))
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

fn build(yaml: &str) -> Engine {
    build_with(config(), yaml)
}

fn route(engine: &mut Engine) -> NavRoute {
    routes(engine).remove(0)
}

fn routes(engine: &mut Engine) -> Vec<NavRoute> {
    let mut all: Vec<_> = engine
        .world_mut()
        .query_ids::<(&NavRoute,)>()
        .map(|(id, (r,))| (id, r.clone()))
        .collect();
    all.sort_by_key(|(id, _)| *id);
    all.into_iter().map(|(_, r)| r).collect()
}

fn door_entity(engine: &mut Engine) -> ludion_core::ids::EntityId {
    engine
        .world_mut()
        .query_ids::<(&NavBlocker,)>()
        .next()
        .map(|(id, _)| id)
        .unwrap()
}

fn edit_door(engine: &mut Engine, edit: impl FnOnce(&mut NavBlocker)) {
    let door = door_entity(engine);
    edit(
        engine
            .world_mut()
            .get_mut::<NavBlocker>(door)
            .unwrap()
            .unwrap(),
    );
}

fn until(engine: &mut Engine, limit: usize, done: impl Fn(&NavRoute) -> bool) -> bool {
    for _ in 0..limit {
        engine.advance_live();
        if engine
            .world_mut()
            .query_ids::<(&NavRoute,)>()
            .next()
            .is_some_and(|(_, (r,))| done(r))
        {
            return true;
        }
    }
    false
}

fn velocity(engine: &mut Engine) -> [f32; 3] {
    engine
        .world_mut()
        .query_ids::<(&NavSteering,)>()
        .next()
        .map(|(_, (s,))| s.velocity)
        .unwrap()
}

fn class(name: &str, radius: f32) -> NavClass {
    NavClass {
        name: name.into(),
        radius,
        ..NavClass::default()
    }
}

#[test]
fn a_block_door_seals_the_rooms() {
    let mut open = build(&(scene(&rooms(false)) + &door("block", "        enabled: false\n")));
    run(&mut open, 3);
    assert!(nav_reachable(open.world(), START, GOAL, "").unwrap());

    let mut sealed = build(&(scene(&rooms(false)) + &door("block", "")));
    run(&mut sealed, 3);
    assert!(!nav_reachable(sealed.world(), START, GOAL, "").unwrap());
    let path = nav_path(sealed.world(), &PathRequest::new(START, GOAL)).unwrap();
    assert_eq!(path.status, PathStatus::Partial);

    let yaml = scene(&rooms(false)) + &door("block", "") + &walker(0, START, GOAL, "");
    let mut routed = build(&yaml);
    run(&mut routed, 5);
    assert!(
        route(&mut routed).partial,
        "the route stops short of the goal"
    );
}

#[test]
fn a_cost_door_is_detoured_only_when_a_second_door_is_cheaper() {
    let distance = |extra: &str| {
        let mut e = build(&(scene(&rooms(true)) + &door("cost", extra)));
        run(&mut e, 3);
        nav_path(e.world(), &PathRequest::new(START, GOAL))
            .unwrap()
            .cost
    };
    let plain = {
        let mut e = build(&scene(&rooms(true)));
        run(&mut e, 3);
        nav_path(e.world(), &PathRequest::new(START, GOAL))
            .unwrap()
            .cost
    };
    let cheap = distance("        cost: 2.0\n");
    assert!(
        (cheap - (plain + 2.0)).abs() < 0.1,
        "{cheap} vs {plain}: crossed once"
    );
    let dear = distance("        cost: 100.0\n");
    assert!(
        dear > plain + 5.0 && dear < plain + 100.0,
        "{dear}: the other door"
    );
}

#[test]
fn a_walker_stops_at_an_interact_door_and_goes_on_when_it_is_opened() {
    let yaml = scene(&rooms(false)) + &door("interact", "") + &walker(0, START, GOAL, "");
    let mut engine = build(&yaml);
    let entity = door_entity(&mut engine);
    assert!(
        until(&mut engine, 200, |r| r.status == NavRouteStatus::AtBlocker),
        "stops at the door"
    );
    let stopped = route(&mut engine);
    assert_eq!(stopped.blocker, Some(entity));
    assert_eq!(stopped.blockers.len(), 1);
    assert!(!stopped.partial);
    for _ in 0..30 {
        engine.advance_live();
        assert_eq!(
            route(&mut engine).status,
            NavRouteStatus::AtBlocker,
            "it waits"
        );
        assert_eq!(velocity(&mut engine), [0.0; 3]);
    }

    edit_door(&mut engine, |b| b.enabled = false);
    assert!(
        until(&mut engine, 400, |r| r.status == NavRouteStatus::Arrived),
        "the walker goes through once it is open"
    );
    assert_eq!(route(&mut engine).blocker, None);
}

#[test]
fn an_opened_interact_door_that_becomes_cost_lets_the_walker_through() {
    let yaml = scene(&rooms(false)) + &door("interact", "") + &walker(0, START, GOAL, "");
    let mut engine = build(&yaml);
    assert!(until(&mut engine, 200, |r| r.status == NavRouteStatus::AtBlocker));
    edit_door(&mut engine, |b| b.mode = NavBlockerMode::Cost);
    assert!(until(&mut engine, 400, |r| r.status == NavRouteStatus::Arrived));
}

#[test]
fn an_interact_door_that_becomes_block_replans_the_waiting_walker() {
    let yaml = scene(&rooms(false)) + &door("interact", "") + &walker(0, START, GOAL, "");
    let mut engine = build(&yaml);
    assert!(until(&mut engine, 200, |r| r.status == NavRouteStatus::AtBlocker));
    edit_door(&mut engine, |b| b.mode = NavBlockerMode::Block);
    run(&mut engine, 3);
    let after = route(&mut engine);
    assert_ne!(after.status, NavRouteStatus::AtBlocker);
    assert!(
        after.partial && after.blockers.is_empty(),
        "no way through a block"
    );
}

#[test]
fn toggling_a_block_door_replans_only_the_routes_that_cross_it() {
    let yaml = scene(&rooms(false))
        + &door("block", "        enabled: false\n")
        + &walker(0, START, GOAL, "")
        + &walker(1, [16.5, 0.0, 4.5], [20.5, 0.0, 8.5], "");
    let mut engine = build(&yaml);
    run(&mut engine, 5);
    let before = routes(&mut engine);
    assert!(before.iter().all(|r| !r.partial));
    edit_door(&mut engine, |b| b.enabled = true);
    run(&mut engine, 3);
    let after = routes(&mut engine);
    assert!(after[0].partial, "the crossing route is cut");
    assert_eq!(
        after[1].waypoints, before[1].waypoints,
        "the other room's route is kept"
    );
    assert!(!after[1].partial);
}

#[test]
fn a_blocker_applies_only_to_the_classes_it_names() {
    let config = NavConfig {
        classes: vec![class("walker", 0.4), class("forklift", 0.45)],
        ..config()
    };
    let yaml = scene(&rooms(false)) + &door("block", "        classes:\n          - forklift\n");
    let mut engine = build_with(config, &yaml);
    run(&mut engine, 3);
    assert!(nav_reachable(engine.world(), START, GOAL, "walker").unwrap());
    assert!(!nav_reachable(engine.world(), START, GOAL, "forklift").unwrap());

    edit_door(&mut engine, |b| b.classes = vec!["ghost".into()]);
    run(&mut engine, 3);
    assert!(nav_reachable(engine.world(), START, GOAL, "forklift").unwrap());
    let keys = engine
        .world()
        .resource::<ludion_engine::diagnostics::Diagnostics>()
        .unwrap()
        .reported(code::NAV_CLASS_UNKNOWN)
        .count();
    assert_eq!(keys, 1, "an unknown class name reports once");
}

#[test]
fn a_moving_blocker_is_followed_by_the_overlay() {
    let yaml = scene(&[ground()]) + &door("block", "");
    let mut engine = build(&yaml);
    run(&mut engine, 3);
    let blocked = |engine: &Engine, at: [f32; 3]| {
        let grid = engine.world().resource::<Nav>().unwrap().grid().unwrap();
        grid.is_blocked(grid.node_at(at).unwrap())
    };
    assert!(blocked(&engine, [12.5, 0.0, 12.5]));
    assert!(!blocked(&engine, [18.5, 0.0, 12.5]));
    let generation = engine
        .world()
        .resource::<Nav>()
        .unwrap()
        .grid()
        .unwrap()
        .generation();

    let door = door_entity(&mut engine);
    engine
        .world_mut()
        .get_mut::<Transform>(door)
        .unwrap()
        .unwrap()
        .translation[0] = 18.0;
    run(&mut engine, 2);
    assert!(!blocked(&engine, [12.5, 0.0, 12.5]));
    assert!(blocked(&engine, [18.5, 0.0, 12.5]));
    let after = engine
        .world()
        .resource::<Nav>()
        .unwrap()
        .grid()
        .unwrap()
        .generation();
    assert!(after > generation, "a moved blocker bumps the generation");
}

#[test]
fn a_world_without_blockers_is_unchanged() {
    let bare = scene(&rooms(true));
    let mut plain = build(&bare);
    // A disabled blocker and a blocker of an unknown class both leave the grid alone.
    let mut idle = build(&(bare + &door("block", "        enabled: false\n")));
    run(&mut plain, 3);
    run(&mut idle, 3);
    let (a, b) = (
        plain.world().resource::<Nav>().unwrap().grid().unwrap(),
        idle.world().resource::<Nav>().unwrap().grid().unwrap(),
    );
    assert_eq!(a.node_count(), b.node_count());
    for n in (0..a.node_count() as u32).map(NodeId) {
        for dir in 0..NEIGHBORS.len() {
            assert_eq!(a.link(n, dir), b.link(n, dir));
            assert_eq!(a.link_cost(n, dir), b.link_cost(n, dir));
        }
    }
    assert_eq!(a.generation(), b.generation());
    let req = PathRequest::new(START, GOAL);
    assert_eq!(
        nav_path(plain.world(), &req).unwrap(),
        nav_path(idle.world(), &req).unwrap()
    );
}

fn crowd() -> Engine {
    let yaml = scene(&rooms(false))
        + &door("interact", "")
        + &walker(0, START, GOAL, "")
        + &walker(1, [3.0, 0.0, 14.5], [20.5, 0.0, 10.5], "");
    build(&yaml)
}

/// The test game: opens the door once a walker has waited 40 ticks.
fn opener_trace(engine: &mut Engine, ticks: usize) -> Vec<u64> {
    (0..ticks)
        .map(|t| {
            engine.advance_live();
            if t == 160 {
                edit_door(engine, |b| b.enabled = false);
            }
            engine.world_hash().unwrap().as_u64()
        })
        .collect()
}

#[test]
fn blocker_routes_are_the_same_at_any_thread_count() {
    let one = par::with_threads(1, || opener_trace(&mut crowd(), 120 + 100));
    let many = par::with_threads(8, || opener_trace(&mut crowd(), 120 + 100));
    assert_eq!(one, many);
    assert_ne!(one[30], one[219], "the walkers moved");
}

#[test]
fn a_restore_mid_wait_rejoins_the_uninterrupted_run() {
    let whole = trace(&mut crowd(), 160);
    let mut first = crowd();
    trace(&mut first, 110);
    assert_eq!(
        route(&mut first).status,
        NavRouteStatus::AtBlocker,
        "snapshot mid-wait"
    );
    let snapshot = snapshot_of(&mut first);
    let mut resumed = crowd();
    resumed
        .execute(Command::EngineRestore(EngineRestoreParams { snapshot }))
        .unwrap();
    assert_eq!(trace(&mut resumed, 50), whole[110..]);
}

#[test]
fn an_invalid_blocker_is_refused_with_l1527() {
    for bad in [
        "        shape:\n          box:\n            half_extents:\n              - 0.0\n              - 1.0\n              - 1.0\n",
        "        cost: -1.0\n",
        "        classes:\n          - \"\"\n",
    ] {
        let yaml = format!(
            "{}  - name: bad\n    components:\n      NavBlocker:\n{bad}",
            scene(&[ground()])
        );
        let mut engine = steering_builder().nav(config()).build().unwrap();
        let error = engine.load_inline_scene(&yaml).unwrap_err();
        assert!(format!("{error:?}").contains("L1527"), "{bad}: {error:?}");
    }
}
