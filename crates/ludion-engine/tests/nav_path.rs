//! Point-to-point navigation: the path queries, and routed agents (`NavGoal` -> `NavRoute`).

use ludion_core::codes::code;
use ludion_core::ids::EntityId;
use ludion_core::par;
use ludion_engine::{
    Engine, NavGoal, NavRoute, NavRouteStatus, PathRequest, PathStatus, Transform, nav_distance,
    nav_path, nav_project, nav_reachable,
};
use ludion_protocol::Command;
use ludion_protocol::command::EngineRestoreParams;

use crate::nav::{
    Block, add_wall, agent, block, config, grid, ground, mover, run, scene, seq, snapshot_of,
    steering_builder, target_yaml, trace, wall,
};

const WALL: [f32; 3] = [10.5, 1.5, 12.0];

/// A goal-bearing agent: `NavAgent` fields, then a `NavGoal` at `goal`.
fn routed(n: usize, at: [f32; 3], goal: [f32; 3], extra: &str) -> String {
    let fields = "        neighbor_layers: 0\n        speed: 6.0\n";
    agent(n, at, fields) + &format!("      NavGoal:\n{extra}        at:\n{}", seq(goal, 10))
}

fn engine_with(blocks: &[Block], agents: &[(usize, [f32; 3], [f32; 3], &str)]) -> Engine {
    let mut engine = steering_builder()
        .nav(config())
        .system("mover", mover)
        .build()
        .expect("boots");
    let mut yaml = scene(blocks);
    for (n, at, goal, extra) in agents {
        yaml += &routed(*n, *at, *goal, extra);
    }
    engine.load_inline_scene(&yaml).unwrap();
    engine
}

fn routes(engine: &mut Engine) -> Vec<(EntityId, NavRoute)> {
    let mut all: Vec<_> = engine
        .world_mut()
        .query_ids::<(&NavRoute,)>()
        .map(|(id, (r,))| (id, r.clone()))
        .collect();
    all.sort_by_key(|(id, _)| *id);
    all
}

fn first_route(engine: &mut Engine) -> NavRoute {
    routes(engine).remove(0).1
}

fn position(engine: &mut Engine) -> [f32; 3] {
    engine
        .world_mut()
        .query_ids::<(&Transform, &NavGoal)>()
        .next()
        .map(|(_, (t, _))| t.translation)
        .unwrap()
}

#[test]
fn queries_report_l4140_before_a_grid_exists() {
    let req = PathRequest::new([2.0, 0.0, 12.0], [20.0, 0.0, 12.0]);
    let mut no_nav = Engine::builder().build().unwrap();
    no_nav.load_inline_scene(&scene(&[ground()])).unwrap();
    assert_eq!(
        nav_path(no_nav.world(), &req).unwrap_err().code,
        code::NAV_NOT_READY
    );

    let mut engine = steering_builder().nav(config()).build().unwrap();
    engine.load_inline_scene(&scene(&[ground()])).unwrap();
    for error in [
        nav_path(engine.world(), &req).unwrap_err(),
        nav_distance(engine.world(), req.from, req.to, "").unwrap_err(),
        nav_reachable(engine.world(), req.from, req.to, "").unwrap_err(),
        nav_project(engine.world(), req.from, "", 1.0).unwrap_err(),
    ] {
        assert_eq!(error.code, code::NAV_NOT_READY);
    }
    run(&mut engine, 2);
    assert!(nav_path(engine.world(), &req).is_ok());
}

#[test]
fn a_path_goes_around_a_wall() {
    let mut engine = steering_builder().nav(config()).build().unwrap();
    engine
        .load_inline_scene(&scene(&[ground(), wall(WALL)]))
        .unwrap();
    run(&mut engine, 2);
    let (from, to) = ([2.0, 0.0, 12.0], [20.0, 0.0, 12.0]);
    let path = nav_path(engine.world(), &PathRequest::new(from, to)).unwrap();
    assert_eq!(path.status, PathStatus::Complete);
    assert!(
        path.cost > 18.0 + 1.0,
        "longer than the straight line: {}",
        path.cost
    );
    assert!(path.waypoints.len() >= 3, "{:?}", path.waypoints);
    assert_eq!(path.waypoints.first(), Some(&from));
    assert_eq!(path.waypoints.last(), Some(&to));
    let grid = grid(&engine);
    for p in &path.waypoints {
        assert!(grid.node_at(*p).is_some(), "{p:?} is walkable");
    }
    let d = nav_distance(engine.world(), from, to, "").unwrap().unwrap();
    assert!((d - path.cost).abs() < 1e-4);
    assert!(nav_reachable(engine.world(), from, to, "").unwrap());
}

#[test]
fn a_ledge_with_no_ramp_is_not_reachable_and_a_partial_path_gets_close() {
    let mut engine = steering_builder().nav(config()).build().unwrap();
    engine
        .load_inline_scene(&scene(&[
            ground(),
            block([12.0, 0.5, 12.0], [2.0, 0.5, 12.0]),
        ]))
        .unwrap();
    run(&mut engine, 2);
    let (from, top) = ([4.0, 0.0, 12.0], [12.0, 1.0, 12.0]);
    assert!(!nav_reachable(engine.world(), from, top, "").unwrap());
    assert_eq!(nav_distance(engine.world(), from, top, "").unwrap(), None);
    let partial = nav_path(engine.world(), &PathRequest::new(from, top)).unwrap();
    assert_eq!(partial.status, PathStatus::Partial);
    let strict = PathRequest {
        partial: false,
        ..PathRequest::new(from, top)
    };
    let strict = nav_path(engine.world(), &strict).unwrap();
    assert_eq!(strict.status, PathStatus::Unreachable);
    assert!(strict.waypoints.is_empty() && strict.cost.is_infinite());
}

#[test]
fn project_snaps_a_point_inside_a_wall() {
    let mut engine = steering_builder().nav(config()).build().unwrap();
    engine
        .load_inline_scene(&scene(&[ground(), wall(WALL)]))
        .unwrap();
    run(&mut engine, 2);
    let inside = [10.5, 0.0, 12.0];
    let p = nav_project(engine.world(), inside, "", 2.0)
        .unwrap()
        .unwrap();
    assert_ne!(p, inside);
    assert!(grid(&engine).node_at(p).is_some());
    assert_eq!(nav_project(engine.world(), inside, "", 0.1).unwrap(), None);
    let open = [3.2, 0.0, 5.7];
    assert_eq!(
        nav_project(engine.world(), open, "", 1.0).unwrap(),
        Some(open)
    );
}

#[test]
fn a_bad_request_is_l4141_with_a_pointer() {
    let mut engine = steering_builder().nav(config()).build().unwrap();
    engine.load_inline_scene(&scene(&[ground()])).unwrap();
    run(&mut engine, 2);
    let base = PathRequest::new([2.0, 0.0, 12.0], [20.0, 0.0, 12.0]);
    let cases = [
        (
            "/from/1",
            PathRequest {
                from: [0.0, f32::NAN, 0.0],
                ..base.clone()
            },
        ),
        (
            "/to/0",
            PathRequest {
                to: [f32::INFINITY, 0.0, 0.0],
                ..base.clone()
            },
        ),
        (
            "/max_cost",
            PathRequest {
                max_cost: -1.0,
                ..base.clone()
            },
        ),
        (
            "/max_cost",
            PathRequest {
                max_cost: f32::NAN,
                ..base.clone()
            },
        ),
        (
            "/snap_radius",
            PathRequest {
                snap_radius: -0.5,
                ..base.clone()
            },
        ),
        (
            "/max_expanded",
            PathRequest {
                max_expanded: 0,
                ..base.clone()
            },
        ),
    ];
    for (pointer, req) in cases {
        let error = nav_path(engine.world(), &req).unwrap_err();
        assert_eq!(error.code, code::NAV_PATH_REQUEST_INVALID, "{pointer}");
        assert_eq!(error.path.as_deref(), Some(pointer));
    }
    assert!(nav_project(engine.world(), base.from, "", -1.0).is_err());
}

#[test]
fn a_goal_agent_walks_around_a_wall_and_arrives() {
    let goal = [20.0, 0.0, 12.0];
    let mut engine = engine_with(&[ground(), wall(WALL)], &[(0, [2.0, 0.0, 12.0], goal, "")]);
    let mut status = None;
    let mut plans = Vec::new();
    for tick in 0..400 {
        run(&mut engine, 1);
        if let Some((_, route)) = routes(&mut engine).first() {
            status = Some(route.status);
            let plan = (route.waypoints.first().copied(), route.cost.to_bits());
            if route.status != NavRouteStatus::Planning && plans.last() != Some(&plan) {
                plans.push(plan);
            }
            if route.status == NavRouteStatus::Arrived {
                assert!(tick > 30, "took a detour, not a dash");
                break;
            }
        }
    }
    assert_eq!(status, Some(NavRouteStatus::Arrived));
    let at = position(&mut engine);
    assert!(
        ludion_core::math::hypot(at[0] - goal[0], at[2] - goal[2]) <= 0.6,
        "{at:?}"
    );
    assert_eq!(plans.len(), 1, "no grid change, so one plan: {plans:?}");
    let route = first_route(&mut engine);
    assert!(
        !route.partial && route.waypoints.len() >= 3 && route.cost > 18.0,
        "{route:?}"
    );
}

#[test]
fn moving_the_goal_to_another_cell_plans_again() {
    let mut engine = engine_with(&[ground()], &[(0, [2.0, 0.0, 12.0], [20.0, 0.0, 12.0], "")]);
    run(&mut engine, 4);
    let before = first_route(&mut engine);
    assert_eq!(before.status, NavRouteStatus::Following);
    let id = routes(&mut engine)[0].0;

    // Inside the same cell: the goal follows, the route is not planned again.
    engine
        .world_mut()
        .get_mut::<NavGoal>(id)
        .unwrap()
        .unwrap()
        .at = [20.4, 0.0, 12.3];
    run(&mut engine, 1);
    let nudged = first_route(&mut engine);
    assert_eq!(nudged.goal, [20.4, 0.0, 12.3]);
    assert_eq!(nudged.cost, before.cost);
    assert_eq!(
        nudged.waypoints.last().map(|p| [p[0], p[2]]),
        Some([20.4, 12.3])
    );

    engine
        .world_mut()
        .get_mut::<NavGoal>(id)
        .unwrap()
        .unwrap()
        .at = [5.0, 0.0, 20.0];
    run(&mut engine, 1);
    let moved = first_route(&mut engine);
    assert_eq!(moved.goal, [5.0, 0.0, 20.0]);
    assert_ne!(moved.cost, before.cost);
    assert_eq!(moved.waypoints.last(), Some(&[5.0, 0.0, 20.0]));
}

#[test]
fn a_wall_across_the_leg_replans_unless_the_goal_says_never() {
    let waypoints_after_wall = |replan: &str| {
        let mut engine = engine_with(
            &[ground()],
            &[(0, [2.0, 0.0, 12.0], [20.0, 0.0, 12.0], replan)],
        );
        run(&mut engine, 4);
        assert_eq!(
            first_route(&mut engine).waypoints.len(),
            2,
            "a straight shot"
        );
        add_wall(&mut engine, WALL);
        run(&mut engine, 6);
        first_route(&mut engine)
    };
    let replanned = waypoints_after_wall("");
    assert!(replanned.waypoints.len() >= 3, "{:?}", replanned.waypoints);
    assert_eq!(replanned.status, NavRouteStatus::Following);
    let stale = waypoints_after_wall("        replan: never\n");
    assert_eq!(stale.waypoints.len(), 2, "never keeps the old route");
}

#[test]
fn the_per_tick_cap_defers_the_rest_to_the_next_tick() {
    let agents: Vec<_> = (0..10)
        .map(|n| (n, [2.0 + n as f32, 0.0, 6.0], [20.0, 0.0, 18.0], ""))
        .collect();
    let mut engine = engine_with(&[ground()], &agents);
    let mut first = Vec::new();
    for _ in 0..6 {
        run(&mut engine, 1);
        first = routes(&mut engine);
        if !first.is_empty() {
            break;
        }
    }
    let planning = first
        .iter()
        .filter(|(_, r)| r.status == NavRouteStatus::Planning)
        .count();
    assert_eq!((first.len(), planning), (10, 2), "{first:?}");
    assert!(
        first[..8]
            .iter()
            .all(|(_, r)| r.status == NavRouteStatus::Following)
    );
    run(&mut engine, 1);
    assert!(
        routes(&mut engine)
            .iter()
            .all(|(_, r)| r.status == NavRouteStatus::Following)
    );
}

fn crowd_engine() -> Engine {
    let agents: Vec<_> = (0..12)
        .map(|n| {
            (
                n,
                [2.0 + 0.8 * n as f32, 0.0, 6.0 + (n % 3) as f32],
                [20.0, 0.0, 18.0],
                "",
            )
        })
        .collect();
    engine_with(&[ground(), wall([10.5, 1.5, 14.0])], &agents)
}

#[test]
fn routed_agents_are_the_same_at_any_thread_count() {
    let one = par::with_threads(1, || trace(&mut crowd_engine(), 150));
    let many = par::with_threads(8, || trace(&mut crowd_engine(), 150));
    assert_eq!(one, many);
    assert_ne!(one[40], one[149], "the crowd moved");
}

#[test]
fn a_restore_mid_route_rejoins_the_uninterrupted_run() {
    let whole = trace(&mut crowd_engine(), 160);
    let mut first = crowd_engine();
    trace(&mut first, 70);
    let snapshot = snapshot_of(&mut first);
    let mut resumed = crowd_engine();
    resumed
        .execute(Command::EngineRestore(EngineRestoreParams { snapshot }))
        .unwrap();
    assert_eq!(trace(&mut resumed, 90), whole[70..]);
}

#[test]
fn an_agent_with_only_a_target_gets_no_route() {
    let mut engine = steering_builder()
        .nav(config())
        .system("mover", mover)
        .build()
        .unwrap();
    let fields = format!(
        "        neighbor_layers: 0\n{}",
        target_yaml([20.0, 0.0, 12.0])
    );
    engine
        .load_inline_scene(&(scene(&[ground()]) + &agent(0, [2.0, 0.0, 12.0], &fields)))
        .unwrap();
    run(&mut engine, 20);
    assert!(routes(&mut engine).is_empty());
}
