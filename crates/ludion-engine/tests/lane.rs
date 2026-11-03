//! `LaneConstraint` at runtime: a character on the engine's `Transform` is held to a `kind: path`
//! lane by `move_character`. Each case drives a hero from a game system over a lane written to a
//! temp content root and reads `MoveReport.lane` back; the numbers are the spec's acceptance cases
//! (1–5 the lane's geometry and easing, 9 the platform carry, 10 the re-acquire, 11 the ends, 12
//! determinism, 13 the diagnostics).

use std::path::PathBuf;
use std::time::Duration;

use crossbeam_channel::{bounded, unbounded};
use ludion_core::ids::EntityId;
use ludion_core::par;
use ludion_ecs::{SystemCtx, World};
use ludion_engine::live::{StampedCommand, run_live, shutdown_channel};
use ludion_engine::{
    DebugCategories, Egress, EgressChannels, Engine, LaneConstraint, LaneRecord, Name,
    TraceChannel, TraceRecord, Transform, lane_frame, lane_relative, move_character,
    teleport_character, trace, write_jsonl,
};
use ludion_physics::{LaneEnd, LaneReport, MoveReport, PhysicsWorld};
use ludion_protocol::ResponseBody;
use ludion_protocol::command::{
    EngineRestoreParams, SimStepParams, StreamSubscribeParams, SubscribeMode,
};
use ludion_protocol::event::{Event, LogEvent};
use ludion_protocol::frame::FrameFormat;
use ludion_protocol::{Channel, Command};
use ludion_scene::{PathMetric, PathSampler, check_path};
use ludion_sim::InputScript;

const DT: f32 = 1.0 / 60.0;
/// Metres a tick at 4 m/s.
const STEP: f32 = 4.0 * DT;
/// Ticks the default 0.2 s engage time takes.
const EASE: usize = 12;

/// What the hero does this tick: the displacement to request, after any edit the case makes.
type Act = fn(&mut World, u64, EntityId) -> [f32; 3];

struct Plan(Act);

#[derive(Default)]
struct Log(Vec<Sample>);

struct Sample {
    tick: u64,
    feet: [f32; 3],
    blend: f32,
    report: MoveReport,
}

/// A sample whose call went through the lane.
struct Lane<'a> {
    tick: u64,
    feet: [f32; 3],
    blend: f32,
    report: &'a MoveReport,
    lane: LaneReport,
}

fn hero(world: &World) -> EntityId {
    world
        .entities()
        .find(|&id| {
            world
                .get::<Name>(id)
                .ok()
                .flatten()
                .is_some_and(|name| name.0 == "hero")
        })
        .expect("the scene has a hero")
}

fn step(world: &mut World, ctx: &SystemCtx) {
    let Some(&Plan(act)) = world.resource::<Plan>() else {
        return;
    };
    let tick = ctx.frame.as_u64();
    let hero = hero(world);
    let displacement = act(world, tick, hero);
    let Ok(report) = move_character(world, hero, displacement) else {
        return;
    };
    let feet = world.get::<Transform>(hero).unwrap().unwrap().translation;
    let blend = world
        .get::<LaneConstraint>(hero)
        .ok()
        .flatten()
        .map_or(0.0, |lane| lane.blend);
    if world.resource::<Log>().is_none() {
        world.insert_resource(Log::default());
    }
    world.resource_mut::<Log>().unwrap().0.push(Sample {
        tick,
        feet,
        blend,
        report,
    });
}

fn fixture_root_dir(tag: &str) -> PathBuf {
    std::env::temp_dir().join(format!("ludion-lane-{tag}-{}", std::process::id()))
}

fn fixture_root(tag: &str, files: &[(&str, String)]) -> PathBuf {
    let dir = fixture_root_dir(tag);
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("a temp content root");
    for (name, text) in files {
        std::fs::write(dir.join(name), text).unwrap();
    }
    dir
}

fn path_yaml(points: &[[f32; 3]], interpolation: &str, closed: bool) -> String {
    let mut text = format!(
        "format: 1\nkind: path\nname: lane\ninterpolation: {interpolation}\nclosed: {closed}\npoints:\n"
    );
    for p in points {
        text.push_str(&format!(
            "  - - {:?}\n    - {:?}\n    - {:?}\n",
            p[0], p[1], p[2]
        ));
    }
    text
}

fn straight(length: f32) -> String {
    path_yaml(&[[0.0; 3], [length, 0.0, 0.0]], "linear", false)
}

fn vec3(v: [f32; 3], indent: &str) -> String {
    v.iter().map(|c| format!("{indent}- {c:?}\n")).collect()
}

fn boxed(name: &str, centre: [f32; 3], half: [f32; 3], rotation: [f32; 3]) -> String {
    format!(
        "  - name: {name}\n    components:\n      Transform:\n        translation:\n{}        rotation:\n{}      Collider:\n        shape:\n          box:\n            half_extents:\n{}        body: static\n",
        vec3(centre, "          "),
        vec3(rotation, "          "),
        vec3(half, "              "),
    )
}

fn flat(name: &str, centre: [f32; 3], half: [f32; 3]) -> String {
    boxed(name, centre, half, [0.0; 3])
}

fn floor() -> String {
    flat("floor", [0.0, -0.5, 0.0], [60.0, 0.5, 60.0])
}

fn lane_block(path: &str, extra: &str) -> String {
    format!("      LaneConstraint:\n        path: {path}\n{extra}")
}

fn hero_entity(feet: [f32; 3], lane: &str) -> String {
    format!(
        "  - name: hero\n    components:\n      Transform:\n        translation:\n{}      CharacterMotor:\n        radius: 0.28\n        height: 0.9\n        slope_limit_deg: 45.0\n        step_height: 0.25\n{lane}",
        vec3(feet, "          ")
    )
}

fn scene(entities: &str) -> String {
    format!("format: 1\nseed: 3\nphysics:\n  space: free\nentities:\n{entities}")
}

fn engine_for(tag: &str, files: &[(&str, String)], scene: &str, act: Act) -> Engine {
    let mut engine = Engine::builder()
        .project_root(fixture_root(tag, files))
        .system("step", step)
        .build()
        .expect("the game builds");
    engine.load_inline_scene(scene).expect("the scene loads");
    engine.world_mut().insert_resource(Plan(act));
    engine
}

fn run(tag: &str, files: &[(&str, String)], scene: &str, act: Act, ticks: usize) -> Engine {
    let mut engine = engine_for(tag, files, scene, act);
    for _ in 0..ticks {
        engine.advance_live();
    }
    engine
}

fn log(engine: &Engine) -> &[Sample] {
    &engine.world().resource::<Log>().expect("the hero moved").0
}

fn lanes(engine: &Engine) -> Vec<Lane<'_>> {
    log(engine)
        .iter()
        .filter_map(|sample| {
            Some(Lane {
                tick: sample.tick,
                feet: sample.feet,
                blend: sample.blend,
                report: &sample.report,
                lane: sample.report.lane?,
            })
        })
        .collect()
}

fn set_lane(world: &mut World, hero: EntityId, edit: impl FnOnce(&mut LaneConstraint)) {
    edit(world.get_mut::<LaneConstraint>(hero).unwrap().unwrap());
}

fn smoothstep(b: f32) -> f32 {
    b * b * (3.0 - 2.0 * b)
}

fn level() -> Act {
    |_, _, _| [0.0, -0.1, 0.0]
}

/// Walks the hero along the lane's forward at 4 m/s, as a game turns its stick with the lane frame.
fn follow(world: &mut World, _tick: u64, hero: EntityId) -> [f32; 3] {
    let Some(frame) = lane_frame(world, hero) else {
        return [0.0, -0.1, 0.0];
    };
    let input = lane_relative([1.0, 0.0], &frame, frame.forward);
    [input.direction[0] * STEP, -0.1, input.direction[2] * STEP]
}

// ---- 1: the pull ----

fn engage_at_two(world: &mut World, tick: u64, hero: EntityId) -> [f32; 3] {
    if tick == 2 {
        set_lane(world, hero, |lane| lane.engaged = true);
    }
    [0.0, -0.1, 0.0]
}

#[test]
fn a_rider_off_the_lane_is_pulled_in_capped_and_eased() {
    let scene = scene(&format!(
        "{}{}",
        floor(),
        hero_entity([5.0, 0.0, 1.5], &lane_block("lane.path.yaml", ""))
    ));
    let engine = run(
        "pull",
        &[("lane.path.yaml", straight(40.0))],
        &scene,
        engage_at_two,
        120,
    );
    let lanes = lanes(&engine);
    assert_eq!(lanes[0].tick, 2, "the lane is inert until engaged");
    for (k, lane) in lanes.iter().enumerate() {
        let eased = smoothstep(((k + 1) as f32 / EASE as f32).min(1.0));
        assert!(
            (lane.lane.weight - eased).abs() < 1e-6,
            "tick {}: weight {} vs {eased}",
            lane.tick,
            lane.lane.weight
        );
    }
    let errors: Vec<f32> = lanes.iter().map(|l| l.lane.lateral_error).collect();
    for pair in errors.windows(2) {
        assert!(pair[1].abs() <= pair[0].abs() + 1e-6, "{pair:?}");
        assert!(
            (pair[0] - pair[1]).abs() <= 4.0 * DT + 1e-5,
            "pull exceeds pull_speed: {pair:?}"
        );
    }
    assert!(errors[0] > 1.4, "the pull starts small: {}", errors[0]);
    let settled = EASE + (1.5 / 4.0 * 60.0) as usize + 2;
    assert!(errors[settled].abs() < 1e-4, "{}", errors[settled]);
    assert!(lanes.last().unwrap().feet[2].abs() < 1e-3);
}

// ---- 2: bends ----

const U_BEND: [[f32; 3]; 5] = [
    [0.0, 0.0, 0.0],
    [8.0, 0.0, 0.0],
    [11.0, 0.0, 3.0],
    [8.0, 0.0, 6.0],
    [0.0, 0.0, 6.0],
];
const S_BEND: [[f32; 3]; 5] = [
    [0.0, 0.0, 0.0],
    [6.0, 0.0, 0.0],
    [9.0, 0.0, 3.0],
    [12.0, 0.0, 6.0],
    [18.0, 0.0, 6.0],
];

fn bend_scene() -> String {
    scene(&format!(
        "{}{}",
        floor(),
        hero_entity(
            [0.0, 0.0, 0.0],
            &lane_block("lane.path.yaml", "        engaged: true\n")
        )
    ))
}

fn bend_run(tag: &str, points: &[[f32; 3]]) -> Engine {
    run(
        tag,
        &[("lane.path.yaml", path_yaml(points, "catmull_rom", false))],
        &bend_scene(),
        follow,
        300,
    )
}

#[test]
fn a_hero_runs_a_u_turn_and_an_s_bend_on_the_lane() {
    for (tag, points) in [("u-bend", &U_BEND), ("s-bend", &S_BEND)] {
        let engine = bend_run(tag, points);
        let lanes = lanes(&engine);
        assert!(lanes.len() > 250, "{tag}: {}", lanes.len());
        for lane in &lanes[EASE..] {
            assert!(
                lane.lane.lateral_error.abs() < 0.05,
                "{tag} tick {}: {}",
                lane.tick,
                lane.lane.lateral_error
            );
        }
        for pair in lanes.windows(2) {
            assert!(
                pair[1].lane.s > pair[0].lane.s,
                "{tag}: s did not advance at tick {}",
                pair[1].tick
            );
        }
        let first = lanes[0].lane;
        let last = lanes.last().unwrap().lane;
        let total: f32 = lanes.iter().map(|l| l.lane.ds).sum();
        let travelled = last.s - (first.s - first.ds);
        assert!(
            (total - travelled).abs() <= 0.01 * travelled,
            "{tag}: {total} vs {travelled}"
        );
        let asked = (lanes.len() - 1) as f32 * STEP;
        assert!(
            (total - asked).abs() <= 0.02 * asked,
            "{tag}: {total} vs {asked}"
        );
    }
}

// ---- 3: ramps and jumps ----

fn jump_at_forty(_: &mut World, tick: u64, _: EntityId) -> [f32; 3] {
    [STEP, if tick == 40 { 0.15 } else { -0.1 }, 0.0]
}

#[test]
fn the_lane_holds_over_a_ramp_a_jump_and_a_drop() {
    let path = path_yaml(
        &[
            [0.0, 0.0, 0.0],
            [12.0, 0.0, 0.0],
            [20.0, 2.0, 0.0],
            [34.0, 2.0, 0.0],
            [44.0, 2.0, 0.0],
        ],
        "linear",
        false,
    );
    let scene = scene(&format!(
        "{}{}{}{}",
        floor(),
        boxed(
            "ramp",
            [16.024_25, 0.902_99, 0.0],
            [4.1231, 0.1, 5.0],
            [0.0, 0.0, 14.0362]
        ),
        flat("plateau", [27.0, 1.0, 0.0], [7.0, 1.0, 5.0]),
        hero_entity(
            [2.0, 0.0, 0.0],
            &lane_block("lane.path.yaml", "        engaged: true\n")
        )
    ));
    let engine = run(
        "ramp",
        &[("lane.path.yaml", path)],
        &scene,
        jump_at_forty,
        620,
    );
    let lanes = lanes(&engine);
    for lane in &lanes {
        assert!(
            lane.lane.lateral_error.abs() < 0.05,
            "tick {}: {}",
            lane.tick,
            lane.lane.lateral_error
        );
    }
    assert!(
        lanes.iter().any(|l| l.feet[1] > 1.9),
        "the ramp was climbed"
    );
    let at = |tick: u64| lanes.iter().find(|l| l.tick == tick).unwrap();
    let (before, jump) = (at(39), at(40));
    assert!(
        (jump.lane.lateral_error - before.lane.lateral_error).abs() < 1e-4,
        "a jump moves the lane error"
    );
    assert!(!jump.report.grounded, "the jump tick leaves the ground");
    assert!(
        (jump.lane.ds - STEP).abs() < 1e-3,
        "the jump leaves s to the horizontal travel: {}",
        jump.lane.ds
    );
    let drop = lanes.iter().position(|l| l.feet[0] > 34.5).unwrap();
    assert!(
        lanes[drop..].iter().any(|l| !l.report.grounded),
        "the hero falls off the plateau"
    );
    assert!(
        lanes[drop..].iter().any(|l| l.report.landed),
        "and lands on the floor"
    );
}

// ---- 4: a lane passing above itself ----

#[test]
fn a_lane_crossing_over_itself_keeps_its_level() {
    let path = path_yaml(
        &[
            [-8.0, 0.0, 0.0],
            [8.0, 0.0, 0.0],
            [8.0, 0.0, -8.0],
            [0.0, 2.0, -8.0],
            [0.0, 2.0, 8.0],
        ],
        "linear",
        false,
    );
    let scene = scene(&format!(
        "{}{}{}",
        floor(),
        flat("deck", [0.0, 1.9, 0.0], [1.0, 0.1, 8.0]),
        hero_entity(
            [0.0, 2.0, -6.0],
            &lane_block("lane.path.yaml", "        engaged: true\n")
        )
    ));
    let engine = run(
        "overpass",
        &[("lane.path.yaml", path)],
        &scene,
        |_, _, _| [0.0, -0.1, STEP],
        200,
    );
    let lanes = lanes(&engine);
    for pair in lanes.windows(2) {
        assert!(
            (pair[1].lane.s - pair[0].lane.s).abs() <= STEP + 0.1,
            "s jumped at tick {}: {} -> {}",
            pair[1].tick,
            pair[0].lane.s,
            pair[1].lane.s
        );
    }
    let crossed = lanes.iter().filter(|l| l.feet[2].abs() < 0.05).count();
    assert!(crossed > 0, "the deck was walked across the overlap");
    let last = lanes.last().unwrap();
    assert!(last.feet[2] > 5.0 && (last.feet[1] - 2.0).abs() < 0.05);
    let asked = (lanes.len() - 1) as f32 * STEP;
    assert!((last.lane.s - lanes[0].lane.s - asked).abs() < 0.3);
}

// ---- 5: disengaging mid-ease ----

fn engage_then_release(world: &mut World, tick: u64, hero: EntityId) -> [f32; 3] {
    match tick {
        2 => set_lane(world, hero, |lane| lane.engaged = true),
        8 => set_lane(world, hero, |lane| lane.engaged = false),
        _ => {}
    }
    if tick >= 40 {
        [0.0, -0.1, 0.05]
    } else {
        [0.0, -0.1, 0.0]
    }
}

#[test]
fn disengaging_eases_out_from_where_the_blend_was() {
    let scene = scene(&format!(
        "{}{}",
        floor(),
        hero_entity([5.0, 0.0, 1.0], &lane_block("lane.path.yaml", ""))
    ));
    let engine = run(
        "release",
        &[("lane.path.yaml", straight(40.0))],
        &scene,
        engage_then_release,
        60,
    );
    let samples = log(&engine);
    let blend = |tick: u64| samples.iter().find(|s| s.tick == tick).unwrap().blend;
    assert!(blend(2) > 0.0 && (3..8).all(|t| blend(t) > blend(t - 1)));
    assert!(
        (blend(8) - (blend(7) - DT / 0.3)).abs() < 1e-6,
        "the fall starts where the rise stopped: {} vs {}",
        blend(8),
        blend(7)
    );
    let lanes = lanes(&engine);
    for pair in lanes.windows(2) {
        assert!(
            (pair[1].lane.weight - pair[0].lane.weight).abs() <= 1.5 / EASE as f32 + 1e-5,
            "weight jumped at tick {}",
            pair[1].tick
        );
    }
    let off = lanes.iter().find(|l| l.blend == 0.0).unwrap();
    assert_eq!(
        off.lane.weight, 0.0,
        "the call that reaches 0 still reports"
    );
    let next = samples.iter().find(|s| s.tick == off.tick + 1).unwrap();
    assert!(
        next.report.lane.is_none(),
        "inert after the blend reaches 0"
    );
    let free: Vec<&Sample> = samples.iter().filter(|s| s.tick >= 41).collect();
    for pair in free.windows(2) {
        assert!(
            (pair[1].feet[2] - pair[0].feet[2] - 0.05).abs() < 1e-4,
            "moves freely across the old lane"
        );
    }
}

// ---- 9: platform carry ----

fn slab(path: &str) -> String {
    format!(
        "  - name: platform\n    components:\n      Transform: {{}}\n      PathFollower:\n        path: {path}\n        speed: 1.0\n        mode: loop\n      Collider:\n        shape:\n          box:\n            half_extents:\n              - 1.5\n              - 0.25\n              - 1.5\n        body: kinematic\n"
    )
}

fn carried(tag: &str, slab_path: String) -> Engine {
    let scene = scene(&format!(
        "{}{}",
        slab("slab.path.yaml"),
        hero_entity(
            [0.0, 0.3, 0.0],
            &lane_block("lane.path.yaml", "        engaged: true\n")
        )
    ));
    run(
        tag,
        &[
            (
                "lane.path.yaml",
                path_yaml(&[[-10.0, 0.0, 0.0], [60.0, 0.0, 0.0]], "linear", false),
            ),
            ("slab.path.yaml", slab_path),
        ],
        &scene,
        level(),
        80,
    )
}

#[test]
fn a_platform_along_the_lane_carries_the_rider_and_one_across_it_does_not() {
    let along = carried(
        "carry-along",
        path_yaml(&[[0.0, -0.25, 0.0], [30.0, -0.25, 0.0]], "linear", false),
    );
    let rides: Vec<_> = lanes(&along).into_iter().filter(|l| l.tick >= 30).collect();
    assert!(rides.len() > 40);
    for ride in &rides {
        let carry = ride.report.platform;
        assert!(carry.entity.is_some(), "tick {}", ride.tick);
        let expected = carry.translation[0] + carry.surface[0];
        assert!(expected > 0.01, "the slab moves");
        assert!(
            (ride.lane.ds - expected).abs() < 1e-3,
            "tick {}: ds {} vs carry {expected}",
            ride.tick,
            ride.lane.ds
        );
        assert!(ride.lane.lateral_error.abs() < 1e-3);
    }

    let across = carried(
        "carry-across",
        path_yaml(&[[0.0, -0.25, 0.0], [0.0, -0.25, 30.0]], "linear", false),
    );
    let rides: Vec<_> = lanes(&across)
        .into_iter()
        .filter(|l| l.tick >= 30)
        .collect();
    for ride in &rides {
        assert!(
            ride.report.platform.translation[2] > 0.01,
            "the slab moves across"
        );
        assert!(ride.lane.lateral_error.abs() < 1e-3, "tick {}", ride.tick);
        assert!(ride.lane.ds.abs() < 1e-3);
        assert!(
            ride.feet[2].abs() < 1e-3,
            "the carry across the lane is projected away"
        );
    }
}

// ---- 10: re-acquiring after a teleport ----

fn teleport_at_sixty(world: &mut World, tick: u64, hero: EntityId) -> [f32; 3] {
    if tick == 60 {
        let before = world.get::<LaneConstraint>(hero).unwrap().unwrap().blend;
        teleport_character(world, hero, [12.0, 0.0, 1.0], 0.0).unwrap();
        let lane = world.get::<LaneConstraint>(hero).unwrap().unwrap();
        assert!(lane.s.is_none(), "teleport_character nulls s");
        assert_eq!(lane.blend, before, "and keeps blend");
        assert!(before > 0.99, "the lane was fully engaged");
    }
    [STEP, -0.1, 0.0]
}

#[test]
fn a_teleport_with_a_nulled_s_finds_the_global_nearest_then_pulls_in_capped() {
    let scene = scene(&format!(
        "{}{}",
        floor(),
        hero_entity(
            [2.0, 0.0, 0.0],
            &lane_block("lane.path.yaml", "        engaged: true\n")
        )
    ));
    let text = straight(40.0);
    let sampler = PathSampler::new(&check_path(&text).unwrap());
    let engine = run(
        "teleport",
        &[("lane.path.yaml", text)],
        &scene,
        teleport_at_sixty,
        70,
    );
    let lanes = lanes(&engine);
    let at = |tick: u64| lanes.iter().find(|l| l.tick == tick).unwrap();
    let landing = at(60);
    let oracle = sampler.nearest_s_global([12.0 + f64::from(STEP), 0.0, 1.0], PathMetric::Full);
    assert!(
        (f64::from(landing.lane.s) - oracle).abs() < 1e-3,
        "s {} vs the global nearest {oracle}",
        landing.lane.s
    );
    assert!((at(59).lane.lateral_error).abs() < 1e-4);
    let capped = 1.0 - 4.0 * DT;
    assert!(
        (landing.lane.lateral_error - capped).abs() < 1e-3,
        "{}",
        landing.lane.lateral_error
    );
    assert!((at(61).lane.lateral_error - (capped - 4.0 * DT)).abs() < 1e-3);
}

// ---- 11: ends ----

fn there_and_back(_: &mut World, tick: u64, _: EntityId) -> [f32; 3] {
    [if tick < 200 { STEP } else { -STEP }, -0.1, 0.0]
}

#[test]
fn an_open_lane_stops_or_extends_past_its_ends() {
    let scene_for = |ends: &str| {
        scene(&format!(
            "{}{}",
            floor(),
            hero_entity(
                [5.0, 0.0, 0.0],
                &lane_block(
                    "lane.path.yaml",
                    &format!("        engaged: true\n        ends: {ends}\n")
                )
            )
        ))
    };
    let stop = run(
        "ends-stop",
        &[("lane.path.yaml", straight(10.0))],
        &scene_for("stop"),
        there_and_back,
        460,
    );
    let lanes_stop = lanes(&stop);
    let end = lanes_stop.iter().find(|l| l.tick == 199).unwrap();
    assert!((end.feet[0] - 10.0).abs() < 0.02, "{:?}", end.feet);
    assert!((end.lane.s - 10.0).abs() < 0.02);
    assert_eq!(end.lane.at_end, Some(LaneEnd::End));
    let start = lanes_stop.last().unwrap();
    assert!(start.feet[0].abs() < 0.02, "{:?}", start.feet);
    assert_eq!(start.lane.at_end, Some(LaneEnd::Start));

    let extend = run(
        "ends-extend",
        &[("lane.path.yaml", straight(10.0))],
        &scene_for("extend"),
        |_, _, _| [STEP, -0.1, 0.0],
        200,
    );
    let last = *lanes(&extend).last().unwrap().report.lane.as_ref().unwrap();
    let feet = log(&extend).last().unwrap().feet;
    assert!(feet[0] > 17.0, "{feet:?}");
    assert!(
        (last.s - feet[0]).abs() < 0.01,
        "s {} vs x {}",
        last.s,
        feet[0]
    );
    assert!(last.lateral_error.abs() < 1e-3);
    assert_eq!(last.at_end, Some(LaneEnd::End));
}

fn one_lap(world: &mut World, _tick: u64, hero: EntityId) -> [f32; 3] {
    let travelled: f32 = world.resource::<Log>().map_or(0.0, |log| {
        log.0
            .iter()
            .filter_map(|s| s.report.lane)
            .map(|l| l.ds)
            .sum()
    });
    if travelled >= 32.0 {
        return [0.0, -0.1, 0.0];
    }
    match lane_frame(world, hero) {
        Some(frame) => [frame.forward[0] * STEP, -0.1, frame.forward[2] * STEP],
        None => [STEP, -0.1, 0.0],
    }
}

#[test]
fn a_closed_lane_wraps_and_a_lap_sums_to_its_length() {
    let square = path_yaml(
        &[
            [0.0, 0.0, 0.0],
            [8.0, 0.0, 0.0],
            [8.0, 0.0, 8.0],
            [0.0, 0.0, 8.0],
        ],
        "linear",
        true,
    );
    let scene = scene(&format!(
        "{}{}",
        floor(),
        hero_entity(
            [4.0, 0.0, 0.0],
            &lane_block("lane.path.yaml", "        engaged: true\n")
        )
    ));
    let engine = run("lap", &[("lane.path.yaml", square)], &scene, one_lap, 560);
    let lanes = lanes(&engine);
    for lane in &lanes {
        assert!(
            lane.lane.ds.abs() < 0.2,
            "tick {}: {}",
            lane.tick,
            lane.lane.ds
        );
        assert_eq!(lane.lane.at_end, None);
    }
    let total: f32 = lanes.iter().map(|l| l.lane.ds).sum();
    assert!((total - 32.0).abs() < 0.1, "{total}");
    let wraps = lanes
        .windows(2)
        .filter(|p| p[1].lane.s < p[0].lane.s)
        .count();
    assert_eq!(wraps, 1, "s wraps once on one lap");
}

// ---- 12: determinism ----

fn hashes(tag: &str) -> Vec<u64> {
    let mut engine = engine_for(
        tag,
        &[("lane.path.yaml", path_yaml(&U_BEND, "catmull_rom", false))],
        &bend_scene(),
        follow,
    );
    (0..200)
        .map(|_| {
            engine.advance_live();
            engine.world_hash().unwrap().as_u64()
        })
        .collect()
}

#[test]
fn a_lane_run_hashes_the_same_twice_and_at_one_and_eight_threads() {
    let first = hashes("hash-a");
    assert_eq!(first, hashes("hash-b"));
    let distinct: std::collections::BTreeSet<u64> = first.iter().copied().collect();
    assert!(distinct.len() > 100, "a flat trace proves nothing");
    #[allow(clippy::disallowed_methods)]
    let [one, eight] = std::thread::scope(|scope| {
        [(1, "hash-t1"), (8, "hash-t8")]
            .map(|(n, tag)| scope.spawn(move || par::with_threads(n, || hashes(tag))))
            .map(|handle| {
                handle
                    .join()
                    .unwrap_or_else(|payload| std::panic::resume_unwind(payload))
            })
    });
    assert_eq!(one, eight);
    assert_eq!(one, first);
}

/// The follow walk as a system that needs no resource, so it survives an `engine.restore` (which
/// rebuilds the world and drops `Plan` and `Log`).
fn follow_system(world: &mut World, _ctx: &SystemCtx) {
    let hero = hero(world);
    let d = follow(world, 0, hero);
    let _ = move_character(world, hero, d);
}

fn restorable_bend(tag: &str) -> Engine {
    let mut engine = Engine::builder()
        .project_root(fixture_root(
            tag,
            &[("lane.path.yaml", path_yaml(&U_BEND, "catmull_rom", false))],
        ))
        .system("follow", follow_system)
        .build()
        .expect("the game builds");
    engine.load_inline_scene(&bend_scene()).expect("loads");
    engine
}

fn blend_of(engine: &Engine) -> f32 {
    let world = engine.world();
    world
        .get::<LaneConstraint>(hero(world))
        .unwrap()
        .unwrap()
        .blend
}

#[test]
fn a_snapshot_restore_mid_lane_resumes_the_same_hashes() {
    const TICKS: usize = 200;
    let mut engine = restorable_bend("restore-ref");
    let mut reference = Vec::new();
    let mut forwards = Vec::new();
    for _ in 0..TICKS {
        engine.advance_live();
        reference.push(engine.world_hash().unwrap().as_u64());
        let world = engine.world();
        forwards.push(lane_frame(world, hero(world)).map(|frame| frame.forward));
    }
    // The first tick has no physics world, so no move: the lane finds its place from the second.
    let first = forwards
        .iter()
        .flatten()
        .next()
        .copied()
        .expect("the lane finds its place");
    let on_the_bend = (40..TICKS)
        .find(|&t| forwards[t].is_some_and(|f| (0..3).any(|i| (f[i] - first[i]).abs() > 0.1)))
        .expect("the hero reaches the bend");

    for at in [5, on_the_bend] {
        let mut before = restorable_bend(&format!("restore-a-{at}"));
        for _ in 0..at {
            before.advance_live();
        }
        let blend = blend_of(&before);
        if at == 5 {
            assert!(0.0 < blend && blend < 1.0, "tick 5 is mid-ease: {blend}");
        } else {
            assert_eq!(blend, 1.0, "tick {at} is on the bend at full blend");
        }
        let snapshot = match before.execute(Command::EngineSnapshot).unwrap().body {
            ResponseBody::Snapshot { snapshot } => snapshot,
            other => panic!("expected a snapshot, got {other:?}"),
        };

        let mut resumed = restorable_bend(&format!("restore-b-{at}"));
        match resumed
            .execute(Command::EngineRestore(EngineRestoreParams { snapshot }))
            .unwrap()
            .body
        {
            ResponseBody::Restored { .. } => {}
            other => panic!("expected a restore, got {other:?}"),
        }
        let world = resumed.world();
        assert!(
            lane_frame(world, hero(world)).is_some(),
            "tick {at}: the frame is readable before any move"
        );
        assert_eq!(resumed.world_hash().unwrap().as_u64(), reference[at - 1]);
        for (i, expected) in reference[at..].iter().enumerate() {
            resumed.advance_live();
            assert_eq!(
                resumed.world_hash().unwrap().as_u64(),
                *expected,
                "tick {} after a restore at {at}",
                at + i + 1
            );
        }
    }
}

#[test]
fn lane_frame_reads_an_authored_place_before_any_move() {
    let engine = engine_for(
        "authored",
        &[("lane.path.yaml", straight(10.0))],
        &scene(&format!(
            "{}{}",
            floor(),
            hero_entity(
                [1.0, 0.0, 0.0],
                &lane_block(
                    "lane.path.yaml",
                    "        engaged: true\n        s: 1.0\n        blend: 1.0\n"
                )
            )
        )),
        level(),
    );
    let world = engine.world();
    let frame = lane_frame(world, hero(world)).expect("an authored lane has a frame");
    assert_eq!(frame.s, 1.0);
}

#[test]
fn lane_frame_survives_a_deleted_path_file_after_a_load() {
    let tag = "cached-load";
    let engine = engine_for(
        tag,
        &[("lane.path.yaml", straight(10.0))],
        &scene(&format!(
            "{}{}",
            floor(),
            hero_entity(
                [1.0, 0.0, 0.0],
                &lane_block(
                    "lane.path.yaml",
                    "        engaged: true\n        s: 1.0\n        blend: 1.0\n"
                )
            )
        )),
        level(),
    );
    std::fs::remove_file(fixture_root_dir(tag).join("lane.path.yaml")).unwrap();
    let world = engine.world();
    assert!(lane_frame(world, hero(world)).is_some());
}

#[test]
fn lane_frame_survives_a_deleted_path_file_after_a_restore() {
    let mut before = restorable_bend("cached-restore-a");
    for _ in 0..10 {
        before.advance_live();
    }
    let snapshot = match before.execute(Command::EngineSnapshot).unwrap().body {
        ResponseBody::Snapshot { snapshot } => snapshot,
        other => panic!("expected a snapshot, got {other:?}"),
    };
    let mut resumed = restorable_bend("cached-restore-b");
    resumed
        .execute(Command::EngineRestore(EngineRestoreParams { snapshot }))
        .unwrap();
    std::fs::remove_file(fixture_root_dir("cached-restore-b").join("lane.path.yaml")).unwrap();
    let world = resumed.world();
    assert!(lane_frame(world, hero(world)).is_some());
}

// ---- 12: debug draw and trace ----

/// Per-tick hashes, the categories drawn and the snapshot bytes of the U-bend run.
fn bend_draw_run(tag: &str, categories: DebugCategories) -> (Vec<u64>, Vec<&'static str>, Vec<u8>) {
    let mut engine = engine_for(
        tag,
        &[("lane.path.yaml", path_yaml(&U_BEND, "catmull_rom", false))],
        &bend_scene(),
        follow,
    );
    engine.set_debug_draw(categories);
    let mut hashes = Vec::new();
    for _ in 0..120 {
        engine.advance_live();
        hashes.push(engine.world_hash().unwrap().as_u64());
    }
    let snapshot = match engine.execute(Command::EngineSnapshot).unwrap().body {
        ludion_protocol::ResponseBody::Snapshot { snapshot } => {
            serde_json::to_vec(&snapshot).unwrap()
        }
        other => panic!("expected a snapshot, got {other:?}"),
    };
    (hashes, engine.debug_draw_drawn(), snapshot)
}

#[test]
fn the_lane_category_never_perturbs_the_hash_or_snapshot() {
    let (hashes, drawn, snapshot) = bend_draw_run("draw-on", DebugCategories::parse("lane"));
    let (plain_hashes, plain_drawn, plain_snapshot) =
        bend_draw_run("draw-off", DebugCategories::None);
    assert!(drawn.contains(&"lane"), "the lane category drew nothing");
    assert_eq!(
        drawn, plain_drawn,
        "the pen records whatever the host filter"
    );
    assert_eq!(hashes, plain_hashes, "per-tick hashes");
    assert_eq!(snapshot, plain_snapshot, "snapshot bytes");
}

#[test]
fn an_inert_lane_draws_nothing() {
    let scene = scene(&format!(
        "{}{}",
        floor(),
        hero_entity([5.0, 0.0, 0.0], &lane_block("lane.path.yaml", ""))
    ));
    let engine = run(
        "draw-inert",
        &[("lane.path.yaml", straight(40.0))],
        &scene,
        level(),
        6,
    );
    assert!(!engine.debug_draw_drawn().contains(&"lane"));
}

#[test]
fn the_motor_trace_carries_the_lane_fields() {
    let mut engine = engine_for(
        "trace-lane",
        &[("lane.path.yaml", path_yaml(&U_BEND, "catmull_rom", false))],
        &bend_scene(),
        follow,
    );
    let records = trace(&mut engine, &InputScript::new(), 60, &[TraceChannel::Motor]).unwrap();
    let lanes_traced: Vec<&LaneRecord> = records
        .iter()
        .filter_map(|record| match record {
            TraceRecord::Motor(motor) => motor.report.as_ref()?.lane.as_ref(),
            TraceRecord::Sensors(_) => None,
        })
        .collect();
    assert_eq!(
        lanes_traced.len(),
        59,
        "every tick but the first (no physics world yet) ran on the lane"
    );
    let last = *lanes_traced.last().unwrap();
    let sample = log(&engine).last().unwrap().report.lane.unwrap();
    assert_eq!(
        (
            last.s,
            last.ds,
            last.weight,
            last.lateral_error,
            last.pull_blocked
        ),
        (
            sample.s,
            sample.ds,
            sample.weight,
            sample.lateral_error,
            sample.pull_blocked
        )
    );
    let hero = hero(engine.world());
    let physics = engine.world().resource::<PhysicsWorld>().unwrap();
    let stored = physics.last_move_report(hero).unwrap().1.lane.unwrap();
    assert_eq!(
        stored, sample,
        "the physics world's report carries the lane"
    );
    let mut text = Vec::new();
    write_jsonl(&records, &mut text).unwrap();
    assert!(
        String::from_utf8(text)
            .unwrap()
            .contains("\"lane\":{\"s\":")
    );

    let scene = scene(&format!("{}{}", floor(), hero_entity([0.0; 3], "")));
    let mut engine = engine_for("trace-free", &[], &scene, level());
    let records = trace(&mut engine, &InputScript::new(), 5, &[TraceChannel::Motor]).unwrap();
    let mut text = Vec::new();
    write_jsonl(&records, &mut text).unwrap();
    let text = String::from_utf8(text).unwrap();
    assert!(text.contains("\"stuck\""), "the moves were reported");
    assert!(!text.contains("\"lane\""));
}

// ---- 13: diagnostics ----

fn log_lines(mut engine: Engine, ticks: u64) -> Vec<LogEvent> {
    engine
        .execute(Command::SimPause)
        .expect("the engine pauses");
    let (intake, intake_rx) = unbounded();
    let (shutdown, shutdown_rx) = shutdown_channel();
    let (frames, _frames_rx) = bounded::<Egress>(1);
    let (events, events_rx) = unbounded::<Egress>();
    let handle = std::thread::spawn(move || {
        run_live(
            engine,
            intake_rx,
            shutdown_rx,
            Some(EgressChannels { frames, events }),
        )
    });
    let send = |command: Command| {
        let (stamped, reply) = StampedCommand::new(command);
        intake.send(stamped).unwrap();
        reply
            .recv_timeout(Duration::from_secs(5))
            .expect("the sim thread replies")
            .expect("the command succeeds");
    };
    send(Command::StreamSubscribe(StreamSubscribeParams {
        channel: Channel::Log,
        mode: SubscribeMode::LatestWins { max_fps: None },
        format: FrameFormat::Qoi,
    }));
    send(Command::SimStep(SimStepParams { ticks, hash: false }));
    let mut lines = Vec::new();
    let mut wait = Duration::from_secs(5);
    while let Ok(Egress::Event(event)) = events_rx.recv_timeout(wait) {
        if let Event::Log(log) = event.message.event {
            lines.push(log);
        }
        wait = Duration::from_millis(300);
    }
    shutdown.signal();
    let _ = handle.join();
    lines
}

fn reported<'a>(lines: &'a [LogEvent], code: &str) -> Vec<&'a LogEvent> {
    lines
        .iter()
        .filter(|l| {
            l.diagnostic
                .as_ref()
                .is_some_and(|d| d.code.as_str() == code)
        })
        .collect()
}

fn walk(_: &mut World, _: u64, _: EntityId) -> [f32; 3] {
    [0.03, -0.1, 0.0]
}

/// The feet of a plain walk over `scene`, and whether any call reported a lane.
fn feet_trace(tag: &str, files: &[(&str, String)], scene: &str, act: Act) -> (Vec<[f32; 3]>, bool) {
    let engine = run(tag, files, scene, act, 30);
    let lane = log(&engine).iter().any(|s| s.report.lane.is_some());
    (log(&engine).iter().map(|s| s.feet).collect(), lane)
}

#[test]
fn a_lane_that_cannot_run_is_reported_once_and_leaves_the_motor_alone() {
    let hero = |lane: &str| {
        scene(&format!(
            "{}{}",
            floor(),
            hero_entity([0.0, 0.0, 0.0], lane)
        ))
    };
    let plain = feet_trace("diag-plain", &[], &hero(""), walk);
    assert!(!plain.1);

    // L1301: the path is missing.
    let missing = hero(&lane_block("nowhere.path.yaml", "        engaged: true\n"));
    assert_eq!(feet_trace("diag-missing", &[], &missing, walk), plain);
    let engine = engine_for("diag-missing-log", &[], &missing, walk);
    let lines = log_lines(engine, 20);
    let found = reported(&lines, "L1301");
    assert_eq!(found.len(), 1, "{lines:?}");
    assert!(found[0].message.contains("nowhere.path.yaml"));

    // L1300: a lane built in Rust with a negative pull_speed.
    let broken = hero(&lane_block("lane.path.yaml", "        engaged: true\n"));
    let files = [("lane.path.yaml", straight(40.0))];
    let poison: Act = |world, _, hero| {
        set_lane(world, hero, |lane| lane.pull_speed = -1.0);
        [0.03, -0.1, 0.0]
    };
    assert_eq!(feet_trace("diag-negative", &files, &broken, poison), plain);
    let lines = log_lines(engine_for("diag-negative-log", &files, &broken, poison), 20);
    let found = reported(&lines, "L1300");
    assert_eq!(found.len(), 1, "{lines:?}");
    assert!(found[0].message.contains("pull_speed"));

    // L1302: a lane spawned on an entity with no motor.
    let orphan: Act = |world, tick, _| {
        if tick == 3 {
            let entity = world.spawn();
            let lane = LaneConstraint {
                path: "lane.path.yaml".into(),
                engaged: true,
                ..LaneConstraint::default()
            };
            world.insert(entity, lane).unwrap();
        }
        [0.03, -0.1, 0.0]
    };
    let lines = log_lines(engine_for("diag-orphan", &files, &hero(""), orphan), 20);
    assert_eq!(reported(&lines, "L1302").len(), 1, "{lines:?}");

    // L1303: a path with a near-vertical span still runs the lane.
    let climb = path_yaml(
        &[
            [0.0, 0.0, 0.0],
            [4.0, 0.0, 0.0],
            [4.0, 4.0, 0.0],
            [8.0, 4.0, 0.0],
        ],
        "linear",
        false,
    );
    let vertical = [("lane.path.yaml", climb)];
    let lines = log_lines(engine_for("diag-vertical", &vertical, &broken, walk), 20);
    let found = reported(&lines, "L1303");
    assert_eq!(found.len(), 1, "{lines:?}");
    assert!(found[0].message.contains(" m"), "{}", found[0].message);
    let engine = run("diag-vertical-run", &vertical, &broken, walk, 30);
    assert!(lanes(&engine).len() > 20, "the lane still runs");
}
