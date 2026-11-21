//! Agent classes: one grid, cache and link table per `NavConfig.classes` entry, picked by name.

use ludion_core::codes::code;
use ludion_core::par;
use ludion_core::rng::DeterministicRng;
use ludion_engine::diagnostics::Diagnostics;
use ludion_engine::{
    Engine, NavClass, NavConfig, NavRoute, NavSteering, PathRequest, PathStatus, nav_distance,
    nav_path, nav_project, nav_random_point, nav_raycast, nav_reachable,
};
use ludion_nav::{NEIGHBORS, NavGrid, NodeId};

use crate::nav::{
    Block, agent, block, config, grid, ground, mover, run, scene, seq, steering_builder, trace,
};

fn class(name: &str, radius: f32, max_step: f32, max_slope_deg: f32) -> NavClass {
    NavClass {
        name: name.into(),
        radius,
        max_step,
        max_slope_deg,
        ..NavClass::default()
    }
}

fn classed() -> NavConfig {
    NavConfig {
        classes: vec![
            class("walker", 0.3, 0.4, 40.0),
            class("forklift", 1.0, 0.2, 10.0),
            class("cautious", 0.3, 0.4, 10.0),
        ],
        ..config()
    }
}

/// A wall across x = 10.5 with a 0.7 m gap centred on z = 12.5, ending at z = 20 so a wide agent can
/// walk around its far end.
fn gap_wall() -> Vec<Block> {
    vec![
        ground(),
        block([10.5, 1.5, 6.075], [0.5, 1.5, 6.075]),
        block([10.5, 1.5, 16.425], [0.5, 1.5, 3.575]),
    ]
}

fn engine_with(config: NavConfig, yaml: &str) -> Engine {
    let mut engine = steering_builder()
        .nav(config)
        .system("mover", mover)
        .build()
        .expect("boots");
    engine.load_inline_scene(yaml).unwrap();
    engine
}

const FROM: [f32; 3] = [2.0, 0.0, 12.5];
const TO: [f32; 3] = [20.0, 0.0, 12.5];

fn dist(engine: &Engine, class: &str) -> Option<f32> {
    nav_distance(engine.world(), FROM, TO, class).unwrap()
}

fn grid_of<'a>(engine: &'a Engine, class: &str) -> &'a NavGrid {
    engine
        .world()
        .resource::<ludion_engine::Nav>()
        .unwrap()
        .grid_for(class)
        .expect("a built grid")
}

#[test]
fn a_gap_passes_the_walker_and_blocks_the_forklift() {
    let mut engine = engine_with(classed(), &scene(&gap_wall()));
    run(&mut engine, 2);
    let walker = dist(&engine, "walker").expect("the walker fits the gap");
    assert!((walker - 18.0).abs() < 0.5, "{walker}");
    let forklift = dist(&engine, "forklift").expect("the forklift goes around the far end");
    assert!(forklift > walker + 8.0, "{forklift} vs {walker}");
    assert_eq!(dist(&engine, ""), Some(walker), "empty is the first class");
    assert!(grid_of(&engine, "walker").node_count() > grid_of(&engine, "forklift").node_count());
}

#[test]
fn a_ramp_is_walkable_by_the_class_whose_slope_allows_it() {
    let plate = Block {
        at: [12.0, 0.7, 12.0],
        half: [2.0, 0.1, 2.0],
        rot: [0.0, 0.0, 20.0],
    };
    let mut engine = engine_with(classed(), &scene(&[ground(), plate]));
    run(&mut engine, 2);
    let on_ramp = |class: &str| {
        let grid = grid_of(&engine, class);
        grid.node_at([12.5, 1.0, 12.5])
            .map(|n| grid.node_pos(n)[1])
            .is_some_and(|y| y > 0.5)
    };
    assert!(on_ramp("walker"));
    assert!(!on_ramp("cautious"), "10 degrees is too steep for it");
    assert!(!on_ramp("forklift"));
}

fn routed(n: usize, class: &str) -> String {
    let fields =
        format!("        class: {class}\n        neighbor_layers: 0\n        speed: 6.0\n");
    agent(n, FROM, &fields) + &format!("      NavGoal:\n        at:\n{}", seq(TO, 10))
}

fn route_of(engine: &mut Engine) -> NavRoute {
    engine
        .world_mut()
        .query_ids::<(&NavRoute,)>()
        .next()
        .map(|(_, (r,))| r.clone())
        .expect("a route")
}

#[test]
fn a_forklift_agent_routes_around_and_a_walker_goes_through() {
    let route = |class: &str| {
        let mut engine = engine_with(classed(), &(scene(&gap_wall()) + &routed(0, class)));
        run(&mut engine, 4);
        route_of(&mut engine)
    };
    let (walker, forklift) = (route("walker"), route("forklift"));
    assert!(
        walker
            .waypoints
            .iter()
            .all(|p| (11.0..=14.0).contains(&p[2])),
        "{:?}",
        walker.waypoints
    );
    assert!(
        forklift.waypoints.iter().any(|p| p[2] > 19.5),
        "{:?}",
        forklift.waypoints
    );
    assert!(forklift.cost > walker.cost + 8.0);
}

fn vault(classes: &str) -> String {
    format!(
        "  - name: vault\n    components:\n      NavLink:\n        from:\n{}        to:\n{}{classes}",
        seq([9.5, 0.0, 12.5], 10),
        seq([11.5, 0.0, 12.5], 10),
    )
}

#[test]
fn a_link_limited_to_a_class_is_absent_for_the_others() {
    let wall = [ground(), block([10.5, 1.5, 12.0], [0.5, 1.5, 12.0])];
    let mut engine = engine_with(
        classed(),
        &(scene(&wall) + &vault("        classes:\n          - walker\n")),
    );
    run(&mut engine, 3);
    assert!(nav_reachable(engine.world(), FROM, TO, "walker").unwrap());
    assert!(!nav_reachable(engine.world(), FROM, TO, "cautious").unwrap());
    assert!(!nav_reachable(engine.world(), FROM, TO, "forklift").unwrap());
    let d = engine.world().resource::<Diagnostics>().unwrap();
    assert_eq!(d.reported(code::NAV_LINK_UNSNAPPED).count(), 0);
    assert_eq!(d.reported(code::NAV_CLASS_UNKNOWN).count(), 0);

    let mut open = engine_with(classed(), &(scene(&wall) + &vault("")));
    run(&mut open, 3);
    assert!(nav_reachable(open.world(), FROM, TO, "cautious").unwrap());
}

#[test]
fn a_link_naming_no_class_reports_once() {
    let wall = [ground(), block([10.5, 1.5, 12.0], [0.5, 1.5, 12.0])];
    let mut engine = engine_with(
        classed(),
        &(scene(&wall) + &vault("        classes:\n          - tank\n")),
    );
    run(&mut engine, 20);
    let d = engine.world().resource::<Diagnostics>().unwrap();
    assert_eq!(d.reported(code::NAV_CLASS_UNKNOWN).count(), 1);
    assert!(!nav_reachable(engine.world(), FROM, TO, "walker").unwrap());
}

#[test]
fn an_unknown_class_is_l4147_from_queries_and_still_for_a_walker() {
    let yaml = scene(&gap_wall()) + &routed(0, "tank");
    let mut engine = engine_with(classed(), &yaml);
    run(&mut engine, 12);
    let w = engine.world();
    let mut rng = DeterministicRng::new(1);
    for error in [
        nav_path(
            w,
            &PathRequest {
                class: "tank".into(),
                ..PathRequest::new(FROM, TO)
            },
        )
        .unwrap_err(),
        nav_distance(w, FROM, TO, "tank").unwrap_err(),
        nav_reachable(w, FROM, TO, "tank").unwrap_err(),
        nav_project(w, FROM, "tank", 1.0).unwrap_err(),
        nav_raycast(w, FROM, TO, "tank").unwrap_err(),
        nav_random_point(w, FROM, 5.0, "tank", &mut rng).unwrap_err(),
    ] {
        assert_eq!(error.code, code::NAV_CLASS_UNKNOWN);
        assert!(
            error
                .help
                .as_deref()
                .is_some_and(|h| h.contains("forklift"))
        );
    }
    let velocity = engine
        .world_mut()
        .query_ids::<(&NavSteering,)>()
        .map(|(_, (s,))| s.velocity)
        .next();
    assert_eq!(velocity, Some([0.0; 3]));
    let d = engine.world().resource::<Diagnostics>().unwrap();
    assert_eq!(d.reported(code::NAV_CLASS_UNKNOWN).count(), 1);
}

fn assert_same(a: &NavGrid, b: &NavGrid) {
    assert_eq!(a.params(), b.params());
    assert_eq!(a.node_count(), b.node_count());
    for n in (0..a.node_count() as u32).map(NodeId) {
        assert_eq!(a.node_pos(n), b.node_pos(n));
        for dir in 0..NEIGHBORS.len() {
            assert_eq!(a.link(n, dir), b.link(n, dir));
            assert_eq!(a.link_cost(n, dir), b.link_cost(n, dir));
        }
    }
}

#[test]
fn one_class_equal_to_the_legacy_fields_changes_nothing() {
    let legacy = NavConfig {
        agent_radius: 0.5,
        agent_height: 1.7,
        max_step: 0.3,
        max_slope_deg: 35.0,
        ..config()
    };
    let single = NavConfig {
        classes: vec![NavClass {
            name: "only".into(),
            radius: 0.5,
            height: 1.7,
            max_step: 0.3,
            max_slope_deg: 35.0,
        }],
        ..config()
    };
    let yaml = scene(&gap_wall());
    let (mut a, mut b) = (engine_with(legacy, &yaml), engine_with(single, &yaml));
    run(&mut a, 2);
    run(&mut b, 2);
    assert_same(grid(&a), grid(&b));
    let req = PathRequest::new(FROM, TO);
    let named = PathRequest {
        class: "only".into(),
        ..req.clone()
    };
    let (pa, pb) = (
        nav_path(a.world(), &req).unwrap(),
        nav_path(b.world(), &named).unwrap(),
    );
    assert_eq!(pa.status, PathStatus::Complete);
    assert_eq!(pa.waypoints, pb.waypoints);
    assert_eq!(pa.cost, pb.cost);
    assert_eq!(nav_path(b.world(), &req).unwrap().waypoints, pa.waypoints);
}

#[test]
fn the_class_grids_and_a_two_class_trace_match_at_any_thread_count() {
    let build = || {
        let mut yaml = scene(&gap_wall());
        for n in 0..6 {
            yaml += &routed(n, if n % 2 == 0 { "walker" } else { "forklift" });
        }
        engine_with(classed(), &yaml)
    };
    let one = par::with_threads(1, || trace(&mut build(), 120));
    let many = par::with_threads(8, || trace(&mut build(), 120));
    assert_eq!(one, many);
    assert_ne!(one[10], one[119], "the agents moved");
    let grids = |threads| {
        par::with_threads(threads, || {
            let mut e = engine_with(classed(), &scene(&gap_wall()));
            run(&mut e, 2);
            ["walker", "forklift", "cautious"].map(|c| grid_of(&e, c).clone())
        })
    };
    for (a, b) in grids(1).iter().zip(&grids(8)) {
        assert_same(a, b);
    }
}

#[test]
fn nav_random_point_repeats_and_stays_reachable() {
    let mut engine = engine_with(classed(), &scene(&gap_wall()));
    run(&mut engine, 2);
    let draw = |seed| {
        let mut rng = DeterministicRng::new(seed);
        (0..20)
            .map(|_| {
                nav_random_point(engine.world(), [4.5, 0.0, 12.5], 5.0, "walker", &mut rng)
                    .unwrap()
                    .unwrap()
            })
            .collect::<Vec<_>>()
    };
    let a = draw(9);
    assert_eq!(a, draw(9));
    assert_ne!(a, draw(10));
    for p in a {
        let d = nav_distance(engine.world(), [4.5, 0.0, 12.5], p, "walker").unwrap();
        assert!(d.is_some_and(|d| d <= 5.0 + 1e-3), "{p:?} is {d:?} away");
    }
    let mut rng = DeterministicRng::new(1);
    assert_eq!(
        nav_random_point(engine.world(), [500.0, 0.0, 500.0], 5.0, "walker", &mut rng).unwrap(),
        None
    );
}

#[test]
fn nav_raycast_stops_at_a_wall() {
    let mut engine = engine_with(classed(), &scene(&gap_wall()));
    run(&mut engine, 2);
    let hit = nav_raycast(engine.world(), [4.5, 0.0, 6.5], [18.5, 0.0, 6.5], "walker")
        .unwrap()
        .expect("the wall blocks the line");
    assert!((8.0..=10.0).contains(&hit[0]), "{hit:?}");
    assert!((hit[2] - 6.5).abs() < 1e-3);
    let clear = nav_raycast(
        engine.world(),
        [2.5, 0.0, 12.5],
        [18.5, 0.0, 12.5],
        "walker",
    )
    .unwrap();
    assert_eq!(clear, None, "the line runs through the gap");
}
