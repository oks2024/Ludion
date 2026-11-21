//! Off-mesh links: `NavLink` edges in `nav_path`, routed agents stopping `at_link`, `nav_link_done`,
//! replanning on a disabled link, thread-count invariance and restore.

use ludion_core::codes::code;
use ludion_core::ids::EntityId;
use ludion_core::par;
use ludion_ecs::{SystemCtx, World};
use ludion_engine::diagnostics::Diagnostics;
use ludion_engine::{
    Engine, NavLink, NavRoute, NavRouteStatus, NavSteering, PathRequest, PathStatus, Transform,
    nav_link_done, nav_path, nav_reachable,
};
use ludion_protocol::Command;
use ludion_protocol::command::EngineRestoreParams;

use crate::nav::{
    agent, block, config, ground, mover, run, scene, seq, snapshot_of, steering_builder, trace,
};

/// The agent starts on the ground and wants the deck.
const START: [f32; 3] = [3.0, 0.0, 12.5];
const GOAL: [f32; 3] = [20.5, 3.0, 12.5];
/// The ladder's ends: the ground beside the deck wall, and the deck's edge.
const FOOT: [f32; 3] = [12.5, 0.0, 12.5];
const TOP: [f32; 3] = [15.5, 3.0, 12.5];
/// The frame from which the test game "climbs": long enough after the walker stops at the link.
const CLIMB_FRAME: u64 = 150;

/// A ground slab and a deck 3 m up with no ramp.
fn floors() -> String {
    scene(&[ground(), block([19.0, 1.5, 12.0], [5.0, 1.5, 12.0])])
}

fn ladder(extra: &str, from: [f32; 3], to: [f32; 3]) -> String {
    format!(
        "  - name: ladder\n    components:\n      NavLink:\n        from:\n{}        to:\n{}{extra}",
        seq(from, 10),
        seq(to, 10)
    )
}

fn walker(n: usize) -> String {
    let fields = "        neighbor_layers: 0\n        speed: 6.0\n";
    agent(n, START, fields) + &format!("      NavGoal:\n        at:\n{}", seq(GOAL, 10))
}

/// The test game's traversal: once `CLIMB_FRAME` passes, move a stopped walker to the link end.
fn climber(world: &mut World, ctx: &SystemCtx) {
    if ctx.frame.0 < CLIMB_FRAME {
        return;
    }
    let stopped: Vec<(EntityId, [f32; 3])> = world
        .query_ids::<(&NavRoute,)>()
        .filter(|(_, (r,))| r.status == NavRouteStatus::AtLink)
        .map(|(id, (r,))| (id, r.waypoints[r.index as usize + 1]))
        .collect();
    for (id, to) in stopped {
        world.get_mut::<Transform>(id).unwrap().unwrap().translation = to;
        nav_link_done(world, id).unwrap();
    }
}

fn build(yaml: &str) -> Engine {
    let mut engine = steering_builder()
        .nav(config())
        .system("mover", mover)
        .system("climber", climber)
        .build()
        .expect("boots");
    engine.load_inline_scene(yaml).unwrap();
    engine
}

fn with_ladder(extra: &str) -> String {
    floors() + &ladder(extra, FOOT, TOP) + &walker(0)
}

fn route(engine: &mut Engine) -> NavRoute {
    engine
        .world_mut()
        .query_ids::<(&NavRoute,)>()
        .next()
        .map(|(_, (r,))| r.clone())
        .expect("a route")
}

fn ladder_entity(engine: &mut Engine) -> EntityId {
    engine
        .world_mut()
        .query_ids::<(&NavLink,)>()
        .next()
        .map(|(id, _)| id)
        .unwrap()
}

fn velocity(engine: &mut Engine) -> [f32; 3] {
    engine
        .world_mut()
        .query_ids::<(&NavSteering,)>()
        .next()
        .map(|(_, (s,))| s.velocity)
        .unwrap()
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

#[test]
fn a_link_joins_the_floors_for_nav_path() {
    let mut linked = build(&(floors() + &ladder("", FOOT, TOP)));
    run(&mut linked, 3);
    let req = PathRequest::new(START, GOAL);
    let path = nav_path(linked.world(), &req).unwrap();
    assert_eq!(path.status, PathStatus::Complete);
    assert_eq!(path.links.len(), 1);
    let i = path.links[0] as usize;
    assert!(path.waypoints[i][1] < 0.5 && path.waypoints[i + 1][1] > 2.5);
    assert!(nav_reachable(linked.world(), START, GOAL, "").unwrap());

    let mut bare = build(&floors());
    run(&mut bare, 3);
    assert!(!nav_reachable(bare.world(), START, GOAL, "").unwrap());
    let partial = nav_path(bare.world(), &req).unwrap();
    assert!(partial.links.is_empty());
}

#[test]
fn a_walker_stops_at_the_link_and_goes_on_after_nav_link_done() {
    let mut engine = build(&with_ladder(""));
    let ladder = ladder_entity(&mut engine);
    assert!(
        until(&mut engine, 140, |r| r.status == NavRouteStatus::AtLink),
        "stops at the link"
    );
    let stopped = route(&mut engine);
    assert_eq!(stopped.link, Some(ladder));
    assert_eq!(stopped.links.len(), 1);
    engine.advance_live();
    assert_eq!(velocity(&mut engine), [0.0; 3]);
    assert_eq!(
        route(&mut engine).status,
        NavRouteStatus::AtLink,
        "it waits"
    );

    assert!(
        until(&mut engine, 400, |r| r.status == NavRouteStatus::Arrived),
        "the climb ends in arrival"
    );
    let end = route(&mut engine);
    assert_eq!(end.link, None);
    assert!(!end.partial);
}

#[test]
fn a_disabled_link_replans_the_walker() {
    let mut engine = build(&with_ladder(""));
    run(&mut engine, 5);
    let before = route(&mut engine);
    assert_eq!(before.status, NavRouteStatus::Following);
    assert_eq!(before.links.len(), 1);
    assert!(!before.partial);

    let ladder = ladder_entity(&mut engine);
    engine
        .world_mut()
        .get_mut::<NavLink>(ladder)
        .unwrap()
        .unwrap()
        .enabled = false;
    run(&mut engine, 3);
    let after = route(&mut engine);
    assert!(after.links.is_empty(), "no way over the wall any more");
    assert!(after.partial);
}

#[test]
fn nav_link_done_needs_a_walker_at_a_link() {
    let mut engine = build(&with_ladder(""));
    run(&mut engine, 5);
    let walker = engine
        .world_mut()
        .query_ids::<(&NavRoute,)>()
        .next()
        .map(|(id, _)| id)
        .unwrap();
    let error = nav_link_done(engine.world_mut(), walker).unwrap_err();
    assert_eq!(error.code, code::NAV_NOT_AT_LINK);
    let ladder = ladder_entity(&mut engine);
    let error = nav_link_done(engine.world_mut(), ladder).unwrap_err();
    assert_eq!(error.code, code::NAV_NOT_AT_LINK);
}

#[test]
fn an_unsnapped_end_warns_once() {
    let yaml = floors() + &ladder("", FOOT, [100.0, 3.0, 100.0]);
    let mut engine = build(&yaml);
    run(&mut engine, 40);
    let keys: Vec<String> = engine
        .world()
        .resource::<Diagnostics>()
        .unwrap()
        .reported(code::NAV_LINK_UNSNAPPED)
        .map(str::to_string)
        .collect();
    assert_eq!(keys.len(), 1, "{keys:?}");
    assert!(
        engine
            .world()
            .resource::<ludion_engine::Nav>()
            .unwrap()
            .links()
            .is_empty()
    );
}

fn linked_crowd() -> Engine {
    let mut yaml = with_ladder("");
    yaml += &walker(1);
    build(&yaml)
}

#[test]
fn linked_routes_are_the_same_at_any_thread_count() {
    let one = par::with_threads(1, || trace(&mut linked_crowd(), 220));
    let many = par::with_threads(8, || trace(&mut linked_crowd(), 220));
    assert_eq!(one, many);
    assert_ne!(one[40], one[219], "the walkers moved");
}

#[test]
fn a_restore_mid_at_link_rejoins_the_uninterrupted_run() {
    let whole = trace(&mut linked_crowd(), 230);
    let mut first = linked_crowd();
    trace(&mut first, 110);
    assert_eq!(
        route(&mut first).status,
        NavRouteStatus::AtLink,
        "snapshot mid-at_link"
    );
    let snapshot = snapshot_of(&mut first);
    let mut resumed = linked_crowd();
    resumed
        .execute(Command::EngineRestore(EngineRestoreParams { snapshot }))
        .unwrap();
    assert_eq!(trace(&mut resumed, 120), whole[110..]);
}
