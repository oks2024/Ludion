//! The `nav` and `nav_states` trace channels: why a route planned, what came out, and each agent's state per
//! tick; identity with the channels off and byte-identical output across runs.

use ludion_core::ids::EntityId;
use ludion_ecs::{SystemCtx, World};
use ludion_engine::{
    Engine, NavBlocker, NavBlockerMode, NavConfig, NavGoal, NavRoute, TraceChannel, Transform,
    nav_link_done, write_jsonl,
};
use ludion_engine::{NavRouteStatus, TraceRecord};
use ludion_sim::InputScript;
use serde_json::Value;

use crate::nav::{
    agent, block, config, ground, mover, scene, seq, steering_builder, target_yaml,
    trace as hash_trace,
};

const START: [f32; 3] = [3.0, 0.0, 12.5];
const GOAL: [f32; 3] = [20.5, 0.0, 12.5];
const CHANNELS: [TraceChannel; 2] = [TraceChannel::Nav, TraceChannel::NavStates];

/// A wall across x = 12 with a 3 m door gap at z = 11..14.
fn rooms() -> Vec<crate::nav::Block> {
    vec![
        ground(),
        block([12.0, 1.5, 19.0], [0.5, 1.5, 5.0]),
        block([12.0, 1.5, 5.5], [0.5, 1.5, 5.5]),
    ]
}

fn door(mode: &str) -> String {
    format!(
        "  - name: door\n    components:\n      Transform:\n        translation:\n{}      NavBlocker:\n        shape:\n          box:\n            half_extents:\n{}        mode: {mode}\n",
        seq([12.0, 1.0, 12.5], 10),
        seq([0.6, 1.0, 1.5], 14)
    )
}

fn walker(n: usize, at: [f32; 3], goal: [f32; 3]) -> String {
    let fields = "        neighbor_layers: 0\n        speed: 6.0\n";
    agent(n, at, fields) + &format!("      NavGoal:\n        at:\n{}", seq(goal, 10))
}

fn build_with(config: NavConfig, yaml: &str) -> Engine {
    let mut engine = steering_builder()
        .nav(config)
        .system("mover", mover)
        .system("climber", climber)
        .build()
        .expect("boots");
    engine.load_inline_scene(yaml).unwrap();
    engine
}

fn build(yaml: &str) -> Engine {
    build_with(config(), yaml)
}

/// The link scene's game: from frame 150, move a walker stopped at a link to its far end.
fn climber(world: &mut World, ctx: &SystemCtx) {
    if ctx.frame.0 < 150 {
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

fn traced(engine: &mut Engine, frames: u64, channels: &[TraceChannel]) -> Vec<Value> {
    let records = ludion_engine::trace(engine, &InputScript::new(), frames, channels).unwrap();
    records
        .iter()
        .map(|r| serde_json::to_value(r).unwrap())
        .collect()
}

fn channel<'a>(lines: &'a [Value], name: &str) -> Vec<&'a Value> {
    lines.iter().filter(|l| l["channel"] == name).collect()
}

fn of<'a>(lines: &[&'a Value], entity: &str) -> Vec<&'a Value> {
    lines
        .iter()
        .copied()
        .filter(|l| l["entity"] == entity)
        .collect()
}

#[test]
fn a_walker_around_a_wall_logs_one_plan_and_a_state_per_tick() {
    let mut engine = build(&(scene(&rooms()) + &walker(0, START, GOAL)));
    let lines = traced(&mut engine, 400, &CHANNELS);

    let plans = channel(&lines, "nav");
    assert_eq!(plans.len(), 1, "one plan: {plans:?}");
    let plan = plans[0];
    assert_eq!(plan["entity"], "a0");
    assert_eq!(plan["reason"], "new");
    assert_eq!(plan["status"], "complete");
    assert!(plan["expanded"].as_u64().unwrap() > 0);
    assert_eq!(plan["to"], serde_json::json!(GOAL));
    assert!(plan.get("class").is_none());

    let route = engine
        .world_mut()
        .query_ids::<(&NavRoute,)>()
        .next()
        .map(|(_, (r,))| r.clone())
        .unwrap();
    assert_eq!(plan["cost"].as_f64().unwrap() as f32, route.cost);
    assert_eq!(
        plan["waypoints"].as_u64().unwrap() as usize,
        route.waypoints.len()
    );

    let states = channel(&lines, "nav_states");
    assert_eq!(states.len(), 400, "one line per agent per tick");
    assert!(states.iter().all(|s| s["mode"] == "route"));
    assert!(states.iter().all(|s| s["goal"] == serde_json::json!(GOAL)));
    assert_eq!(states[0]["tick"], 0);
    let following = states.iter().position(|s| s["status"] == "following");
    let arrived = states.iter().position(|s| s["status"] == "arrived");
    assert!(
        following.is_some() && arrived.is_some() && following < arrived,
        "following then arrived"
    );
    let indices: Vec<u64> = states.iter().filter_map(|s| s["index"].as_u64()).collect();
    assert!(indices.windows(2).all(|w| w[0] <= w[1]), "{indices:?}");
}

#[test]
fn the_request_cap_defers_a_plan_and_the_next_tick_retries_it() {
    let config = NavConfig {
        max_path_requests_per_tick: 1,
        ..config()
    };
    let yaml = scene(&rooms()) + &walker(0, START, GOAL) + &walker(1, [3.0, 0.0, 14.0], GOAL);
    let mut engine = build_with(config, &yaml);
    let lines = traced(&mut engine, 12, &[TraceChannel::Nav]);
    let plans = channel(&lines, "nav");
    assert_eq!(plans.len(), 3, "{plans:?}");
    let first = plans[0]["tick"].clone();
    assert_eq!(
        (plans[0]["entity"].clone(), plans[0]["reason"].clone()),
        ("a0".into(), "new".into())
    );
    assert_eq!(plans[0]["status"], "complete");
    assert_eq!(plans[1]["entity"], "a1");
    assert_eq!(plans[1]["tick"], first);
    assert_eq!(plans[1]["reason"], "new");
    assert_eq!(plans[1]["status"], "deferred");
    assert!(plans[1].get("cost").is_none());
    assert_eq!(plans[1]["expanded"], 0);
    assert_eq!(plans[2]["entity"], "a1");
    assert_eq!(plans[2]["tick"].as_u64(), first.as_u64().map(|t| t + 1));
    assert_eq!(plans[2]["reason"], "retry");
    assert_eq!(plans[2]["status"], "complete");
}

#[test]
fn a_moved_goal_logs_goal_moved() {
    let mut engine = build(&(scene(&rooms()) + &walker(0, START, GOAL)));
    traced(&mut engine, 10, &[TraceChannel::Nav]);
    let id = engine
        .world_mut()
        .query_ids::<(&NavGoal,)>()
        .next()
        .map(|(id, _)| id)
        .unwrap();
    engine
        .world_mut()
        .get_mut::<NavGoal>(id)
        .unwrap()
        .unwrap()
        .at = [20.5, 0.0, 20.5];
    let lines = traced(&mut engine, 3, &[TraceChannel::Nav]);
    let plans = channel(&lines, "nav");
    assert_eq!(plans.len(), 1, "{plans:?}");
    assert_eq!(plans[0]["reason"], "goal_moved");
    assert_eq!(plans[0]["to"], serde_json::json!([20.5, 0.0, 20.5]));
}

#[test]
fn an_interact_door_shows_at_blocker_then_blocker_closed() {
    let mut engine = build(&(scene(&rooms()) + &door("interact") + &walker(0, START, GOAL)));
    let lines = traced(&mut engine, 200, &CHANNELS);
    let states = channel(&lines, "nav_states");
    let waiting = states
        .iter()
        .find(|s| s["status"] == "at_blocker")
        .expect("the walker stops at the door");
    assert_eq!(waiting["blocker"], "door");
    assert_eq!(channel(&lines, "nav").len(), 1);

    let door = engine
        .world_mut()
        .query_ids::<(&NavBlocker,)>()
        .next()
        .map(|(id, _)| id)
        .unwrap();
    engine
        .world_mut()
        .get_mut::<NavBlocker>(door)
        .unwrap()
        .unwrap()
        .mode = NavBlockerMode::Block;
    let lines = traced(&mut engine, 3, &[TraceChannel::Nav]);
    let plans = channel(&lines, "nav");
    assert_eq!(plans.len(), 1, "{plans:?}");
    assert_eq!(plans[0]["reason"], "blocker_closed");
    assert_eq!(plans[0]["status"], "partial");
}

#[test]
fn a_link_traversal_shows_at_link_with_the_links_name() {
    let goal = [20.5, 3.0, 12.5];
    let yaml = scene(&[ground(), block([19.0, 1.5, 12.0], [5.0, 1.5, 12.0])])
        + &format!(
            "  - name: ladder\n    components:\n      NavLink:\n        from:\n{}        to:\n{}",
            seq([12.5, 0.0, 12.5], 10),
            seq([15.5, 3.0, 12.5], 10)
        )
        + &walker(0, START, goal);
    let mut engine = build(&yaml);
    let lines = traced(&mut engine, 160, &CHANNELS);
    let plans = channel(&lines, "nav");
    assert_eq!(plans[0]["links"], 1, "{plans:?}");
    let states = channel(&lines, "nav_states");
    let at_link = states
        .iter()
        .find(|s| s["status"] == "at_link")
        .expect("the walker stops at the link");
    assert_eq!(at_link["link"], "ladder");
}

#[test]
fn a_goal_off_the_grid_is_unreachable_with_no_cost() {
    let mut engine = build(&(scene(&rooms()) + &walker(0, START, [40.0, 0.0, 12.5])));
    let lines = traced(&mut engine, 10, &CHANNELS);
    let plans = channel(&lines, "nav");
    assert_eq!(plans[0]["status"], "unreachable", "{plans:?}");
    assert!(plans[0].get("cost").is_none());
    let states = channel(&lines, "nav_states");
    assert!(states.iter().any(|s| s["status"] == "unreachable"));
}

#[test]
fn field_and_flying_agents_print_their_mode() {
    let fields = |extra: &str| {
        format!(
            "        neighbor_layers: 0\n        speed: 6.0\n{extra}{}",
            target_yaml(GOAL)
        )
    };
    let yaml = scene(&rooms())
        + &agent(0, START, &fields(""))
        + &agent(1, [3.0, 2.0, 14.0], &fields("        flying: true\n"));
    let mut engine = build(&yaml);
    let lines = traced(&mut engine, 5, &CHANNELS);
    assert!(channel(&lines, "nav").is_empty(), "no goal, no plan");
    let states = channel(&lines, "nav_states");
    let field = of(&states, "a0");
    let fly = of(&states, "a1");
    assert_eq!(field.len(), 5);
    assert!(field.iter().all(|s| s["mode"] == "field"));
    assert!(field.iter().all(|s| s.get("status").is_none()));
    assert!(fly.iter().all(|s| s["mode"] == "fly"));
    assert_eq!(field[0]["goal"], serde_json::json!(GOAL));
    assert_eq!(field[0]["position"], serde_json::json!(START));
}

#[test]
fn a_world_without_nav_agents_prints_no_nav_lines() {
    let mut engine = build(&scene(&rooms()));
    let lines = traced(&mut engine, 10, &TraceChannel::ALL);
    assert!(channel(&lines, "nav").is_empty());
    assert!(channel(&lines, "nav_states").is_empty());
}

#[test]
fn tracing_does_not_move_the_world_hash_and_runs_repeat_byte_for_byte() {
    let yaml = scene(&rooms()) + &door("interact") + &walker(0, START, GOAL);
    let plain = hash_trace(&mut build(&yaml), 120);

    let mut engine = build(&yaml);
    let mut hashes = Vec::new();
    let mut records: Vec<TraceRecord> = Vec::new();
    ludion_engine::trace(&mut engine, &InputScript::new(), 0, &CHANNELS).unwrap();
    for _ in 0..120 {
        records
            .extend(ludion_engine::trace(&mut engine, &InputScript::new(), 1, &CHANNELS).unwrap());
        hashes.push(engine.world_hash().unwrap().as_u64());
    }
    assert_eq!(plain, hashes);

    let run = || {
        let mut engine = build(&yaml);
        let records =
            ludion_engine::trace(&mut engine, &InputScript::new(), 120, &CHANNELS).unwrap();
        let mut out = Vec::new();
        write_jsonl(&records, &mut out).unwrap();
        out
    };
    let (a, b) = (run(), run());
    assert!(!a.is_empty());
    assert_eq!(a, b);
}
