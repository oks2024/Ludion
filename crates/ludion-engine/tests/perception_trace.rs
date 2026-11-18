//! Runtime proof for the `perception` trace channel (`Plan/perception.md` Wave 4b): one `visible`
//! edge per key, one noise line per delivery that tick's gameplay time advanced on, `remember` and
//! `forget` lines for the records, nothing for a scene without perception, and a sim the channel
//! cannot move.

use ludion_core::ids::EntityId;
use ludion_core::time::GameClock;
use ludion_ecs::{SystemCtx, World};
use ludion_engine::{
    Engine, EngineBuilder, Name, Noise, PerceptionConfig, PerceptionResults, TraceChannel,
    TraceRecord, Transform, emit_noise, perception_update, trace,
};
use ludion_sim::InputScript;
use serde_json::Value;

/// An entity named `name` at `at` with the extra component YAML (indented to 6).
fn entity(name: &str, at: [f32; 3], components: &str) -> String {
    format!(
        "  - name: {name}\n    components:\n      Transform:\n        translation:\n          - {:?}\n          - {:?}\n          - {:?}\n{components}",
        at[0], at[1], at[2]
    )
}

/// A guard at the origin looking along -Z: a 30 degree zone called `narrow`, plus `extra` `Perceiver`
/// YAML (at 8).
fn guard(extra: &str) -> String {
    entity(
        "guard",
        [0.0; 3],
        &format!(
            "      Perceiver:\n        sight:\n          - name: narrow\n            range: 40.0\n            half_angle_deg: 30.0\n{extra}"
        ),
    )
}

/// A player-class target with one unblockable point at its origin.
fn mark(name: &str, at: [f32; 3]) -> String {
    entity(
        name,
        at,
        "      SightTarget:\n        class: player\n        points:\n          - offset:\n              - 0.0\n              - 0.0\n              - 0.0\n",
    )
}

fn scene(entities: &[String]) -> String {
    let mut out = String::from("format: 1\nseed: 1\nphysics:\n  space: free\nentities:\n");
    for e in entities {
        out += e;
    }
    out
}

/// The mark crosses the guard's zone along z = -10: in it from about tick 31 to tick 89.
fn walker_path(world: &mut World, ctx: &SystemCtx) {
    let x = -12.0 + 0.2 * ctx.frame.as_u64() as f32;
    let ids: Vec<EntityId> = world
        .query_ids::<(&Name,)>()
        .filter(|(_, (n,))| n.0 == "mark")
        .map(|(id, _)| id)
        .collect();
    for id in ids {
        world.get_mut::<Transform>(id).unwrap().unwrap().translation = [x, 0.0, -10.0];
    }
}

fn hold_window(world: &mut World, ctx: &SystemCtx) {
    if (30..=59).contains(&ctx.frame.as_u64()) {
        world.resource_mut::<GameClock>().unwrap().hold();
    }
}

/// A footstep 3 m from the guard on these frames, emitted before `perception_update`.
fn emitter(world: &mut World, ctx: &SystemCtx) {
    if [10, 28, 45, 70].contains(&ctx.frame.as_u64()) {
        let noise = Noise {
            at: [3.0, 0.0, 0.0],
            radius: 6.0,
            loudness: 0.8,
            kind: "footstep".into(),
            source: None,
            surface: None,
            occlusion: false,
        };
        emit_noise(world, ctx, noise);
    }
}

fn builder() -> EngineBuilder {
    Engine::builder()
        .perception(PerceptionConfig {
            classes: vec!["player".into()],
            noise_kinds: vec!["footstep".into()],
            ..Default::default()
        })
        .system("walker_path", walker_path)
        .system("hold_window", hold_window)
        .system("emitter", emitter)
        .system("perception_update", perception_update)
}

fn engine_with(builder: EngineBuilder, text: &str) -> Engine {
    let mut engine = builder.build().expect("boots");
    engine.load_inline_scene(text).expect("the scene loads");
    engine
}

fn lines(engine: &mut Engine, frames: u64, channels: &[TraceChannel]) -> Vec<Value> {
    trace(engine, &InputScript::new(), frames, channels)
        .expect("the trace runs")
        .iter()
        .map(|r: &TraceRecord| serde_json::to_value(r).unwrap())
        .collect()
}

fn events<'a>(lines: &'a [Value], event: &str) -> Vec<&'a Value> {
    lines
        .iter()
        .filter(|l| l["channel"] == "perception" && l["event"] == event)
        .collect()
}

#[test]
fn a_target_crossing_a_zone_prints_one_visible_edge_each_way() {
    let text = scene(&[guard(""), mark("mark", [-12.0, 0.0, -10.0])]);
    let mut engine = engine_with(builder(), &text);
    let out = lines(&mut engine, 120, &[TraceChannel::Perception]);
    let sight = events(&out, "sight");
    assert_eq!(sight.len(), 2, "{sight:?}");
    assert_eq!(sight[0]["visible"], true);
    assert_eq!(sight[0]["zone"], "narrow");
    assert_eq!(sight[0]["perceiver"], "guard");
    assert_eq!(sight[0]["target"], "mark");
    assert_eq!(sight[0]["in_zone"], true);
    assert_eq!(sight[1]["visible"], false);
    assert_eq!(sight[1]["in_zone"], false);
    assert!(
        sight[1].get("distance").is_none(),
        "a dropped key has no sample fields"
    );
    assert!(sight[0]["tick"].as_u64() < sight[1]["tick"].as_u64());
}

#[test]
fn noise_lines_equal_the_deliveries_and_a_held_frame_repeats_nothing() {
    let text = scene(&[guard(
        "        hearing:\n          hears:\n            - footstep\n",
    )]);
    let mut traced = engine_with(builder(), &text);
    let out = lines(&mut traced, 90, &[TraceChannel::Perception]);
    let noise = events(&out, "noise");

    let mut reference = engine_with(builder(), &text);
    let guard = reference
        .world_mut()
        .query_ids::<(&Name,)>()
        .find(|(_, (n,))| n.0 == "guard")
        .map(|(id, _)| id)
        .unwrap();
    let mut expected = Vec::new();
    let mut last_tick = None;
    for _ in 0..90 {
        let frame = reference.frame().as_u64();
        reference.advance_live();
        let results = reference.world().resource::<PerceptionResults>().unwrap();
        if last_tick != Some(results.tick()) {
            last_tick = Some(results.tick());
            for heard in results.heard(guard) {
                expected.push((frame, heard.clone()));
            }
        }
    }
    assert!(!expected.is_empty(), "the guard hears the footsteps");
    assert_eq!(noise.len(), expected.len(), "{noise:?}");
    for (line, (frame, heard)) in noise.iter().zip(&expected) {
        assert_eq!(line["tick"], *frame);
        assert_eq!(line["listener"], "guard");
        assert_eq!(line["noise"], heard.noise.0);
        assert_eq!(line["kind"], heard.kind.as_str());
        assert_eq!(line["distance"], f64::from(heard.distance));
        assert_eq!(line["effective_radius"], f64::from(heard.effective_radius));
        assert_eq!(line["occluders"], heard.occluders);
        assert!(line.get("source").is_none());
    }
    let mut ids: Vec<u64> = noise.iter().map(|l| l["noise"].as_u64().unwrap()).collect();
    ids.dedup();
    assert_eq!(
        ids.len(),
        noise.len(),
        "no noise prints twice across the hold"
    );
}

#[test]
fn remember_and_forget_print_on_the_ticks_a_record_spawns_and_is_forgotten() {
    let memory = "        memory:\n          forget_sight_s: 1.0\n";
    let text = scene(&[guard(memory), mark("mark", [-12.0, 0.0, -10.0])]);
    let mut engine = engine_with(builder(), &text);
    let out = lines(&mut engine, 220, &[TraceChannel::Perception]);
    let remember = events(&out, "remember");
    let forget = events(&out, "forget");
    assert_eq!((remember.len(), forget.len()), (1, 1), "{out:?}");
    assert_eq!(remember[0]["perceiver"], "guard");
    assert_eq!(remember[0]["source"], "mark");
    assert_eq!(remember[0]["sense"], "sight");
    assert!(remember[0].get("kind").is_none());
    assert_eq!(forget[0]["sense"], "sight");
    let seen = events(&out, "sight")[0]["tick"].as_u64().unwrap();
    assert_eq!(remember[0]["tick"].as_u64().unwrap(), seen);
    assert!(
        forget[0]["tick"].as_u64().unwrap() > remember[0]["tick"].as_u64().unwrap() + 60,
        "forgotten after the one second of gameplay time"
    );
}

#[test]
fn a_scene_without_perception_prints_no_perception_line() {
    let text = scene(&[entity(
        "cube",
        [0.0; 3],
        "      Mesh:\n        shape: cube\n",
    )]);
    let mut engine = engine_with(Engine::builder(), &text);
    let out = lines(&mut engine, 10, &TraceChannel::ALL);
    assert!(out.iter().all(|l| l["channel"] != "perception"), "{out:?}");
}

#[test]
fn the_channel_parses_and_an_unknown_one_lists_it() {
    assert_eq!(
        "perception".parse::<TraceChannel>(),
        Ok(TraceChannel::Perception)
    );
    assert!(TraceChannel::ALL.contains(&TraceChannel::Perception));
    let error = "bogus".parse::<TraceChannel>().unwrap_err();
    assert!(error.contains("`perception`"), "{error}");
}

#[test]
fn recording_the_channel_does_not_move_the_world_hash() {
    let memory = "        memory:\n          forget_sight_s: 1.0\n        hearing:\n          hears:\n            - footstep\n";
    let text = scene(&[guard(memory), mark("mark", [-12.0, 0.0, -10.0])]);
    let mut plain = engine_with(builder(), &text);
    let mut traced = engine_with(builder(), &text);
    for tick in 0..120 {
        plain.advance_live();
        lines(&mut traced, 1, &[TraceChannel::Perception]);
        assert_eq!(
            plain.world_hash().unwrap(),
            traced.world_hash().unwrap(),
            "tick {tick}"
        );
    }
}
