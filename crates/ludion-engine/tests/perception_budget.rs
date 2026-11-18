//! Runtime proof for the sight stagger and ray budget (`Plan/perception.md` Wave 4a): `period_ticks`
//! phases evaluation, `max_sight_rays_per_tick` rotates the perceivers it serves, the rest keep
//! their last samples, and a world with neither hashes exactly as before.

use ludion_core::codes::code;
use ludion_core::ids::EntityId;
use ludion_core::par;
use ludion_ecs::{SystemCtx, World};
use ludion_engine::diagnostics::Diagnostics;
use ludion_engine::{
    Engine, EngineBuilder, Name, PerceiverStatus, PerceptionConfig, PerceptionResults,
    PerceptionState, SightSample, SightSampleRecord, StimulusRecord, Transform, perception_update,
};
use ludion_protocol::command::EngineRestoreParams;
use ludion_protocol::{Command, ResponseBody};
use serde_json::Value;

const WIDE: &str =
    "          - name: wide\n            range: 40.0\n            half_angle_deg: 80.0\n";

/// An entity named `name` at `at` with the extra component YAML (indented to 6).
fn entity(name: &str, at: [f32; 3], components: &str) -> String {
    format!(
        "  - name: {name}\n    components:\n      Transform:\n        translation:\n          - {:?}\n          - {:?}\n          - {:?}\n{components}",
        at[0], at[1], at[2]
    )
}

/// A guard at `x` that looks along -Z, with `period_ticks` and the extra `Perceiver` YAML (at 8).
fn guard(name: &str, x: f32, period: u32, extra: &str) -> String {
    entity(
        name,
        [x, 0.0, 0.0],
        &format!("      Perceiver:\n        sight:\n{WIDE}        period_ticks: {period}\n{extra}"),
    )
}

/// A player-class target whose one point is blocked by `cover_high`, so every guard that sees it
/// casts one ray.
fn mark(name: &str, at: [f32; 3]) -> String {
    entity(
        name,
        at,
        "      SightTarget:\n        class: player\n        points:\n          - offset:\n              - 0.0\n              - 1.0\n              - 0.0\n            blocked_by:\n              - cover_high\n",
    )
}

fn scene(entities: &[String]) -> String {
    let mut out = String::from(
        "format: 1\nseed: 1\nphysics:\n  space: free\n  layers:\n    - name: cover_high\nentities:\n",
    );
    for e in entities {
        out += e;
    }
    // A wall off every sight line, so a physics world exists and the rays are really cast.
    out += &entity(
        "wall",
        [100.0, 0.0, 100.0],
        "      Collider:\n        shape:\n          box:\n            half_extents:\n              - 1.0\n              - 1.0\n              - 0.1\n        body: static\n        layer: cover_high\n",
    );
    out
}

fn builder(budget: Option<u32>) -> EngineBuilder {
    Engine::builder().perception(PerceptionConfig {
        classes: vec!["player".into()],
        max_sight_rays_per_tick: budget,
        ..Default::default()
    })
}

fn engine_with(builder: EngineBuilder, text: &str) -> Engine {
    let mut engine = builder
        .system("perception_update", perception_update)
        .build()
        .expect("boots");
    engine.load_inline_scene(text).expect("the scene loads");
    engine
}

fn engine(budget: Option<u32>, text: &str) -> Engine {
    engine_with(builder(budget), text)
}

fn run(engine: &mut Engine, ticks: usize) {
    for _ in 0..ticks {
        engine.advance_live();
    }
}

fn find(engine: &mut Engine, wanted: &str) -> EntityId {
    engine
        .world_mut()
        .query_ids::<(&Name,)>()
        .find(|(_, (name,))| name.0 == wanted)
        .map(|(id, _)| id)
        .unwrap_or_else(|| panic!("no entity named {wanted}"))
}

fn results(engine: &Engine) -> &PerceptionResults {
    engine.world().resource::<PerceptionResults>().unwrap()
}

fn now(engine: &Engine) -> u64 {
    results(engine).tick()
}

/// The tick `who`'s sight was last evaluated on, from its samples; `None` when it has none.
fn evaluated(engine: &Engine, who: EntityId) -> Option<u64> {
    results(engine)
        .sight(who)
        .first()
        .map(|sample| sample.evaluated_tick)
}

fn count<T: ludion_ecs::Component>(engine: &mut Engine) -> usize {
    engine.world_mut().query_ids::<(&T,)>().count()
}

fn reported(engine: &Engine, code: ludion_core::diagnostics::DiagnosticCode) -> usize {
    engine
        .world()
        .resource::<Diagnostics>()
        .unwrap()
        .reported(code)
        .count()
}

/// Several guards side by side, each seeing the one mark.
fn crowd(guards: usize, period: u32, extra: &str) -> String {
    let mut entities: Vec<String> = (0..guards)
        .map(|i| guard(&format!("g{i}"), i as f32 * 0.5, period, extra))
        .collect();
    entities.push(mark("mark", [0.0, 0.0, -10.0]));
    scene(&entities)
}

#[test]
fn a_staggered_perceiver_is_evaluated_on_its_phase_and_keeps_its_samples_between() {
    let mut engine = engine(None, &crowd(3, 3, ""));
    let ids: Vec<EntityId> = (0..3)
        .map(|i| find(&mut engine, &format!("g{i}")))
        .collect();
    for _ in 0..12 {
        engine.advance_live();
        let tick = now(&engine);
        for id in &ids {
            let phase = u64::from(id.index() % 3);
            let want = (tick >= phase).then(|| tick - (tick - phase) % 3);
            assert_eq!(evaluated(&engine, *id), want, "tick {tick}, guard {id:?}");
            if want.is_some() {
                assert_eq!(results(&engine).sight(*id).len(), 1);
            }
        }
    }
}

#[test]
fn a_budget_below_demand_serves_in_rotation_and_lists_the_rest() {
    let mut engine = engine(Some(2), &crowd(4, 1, ""));
    let ids: Vec<EntityId> = (0..4)
        .map(|i| find(&mut engine, &format!("g{i}")))
        .collect();
    let cursor = |engine: &mut Engine| {
        engine
            .world_mut()
            .query_ids::<(&PerceptionState,)>()
            .next()
            .and_then(|(_, (s,))| s.sight_cursor)
    };
    engine.advance_live();
    let first = now(&engine);
    assert_eq!(results(&engine).deferred(), [ids[2], ids[3]]);
    assert_eq!(evaluated(&engine, ids[0]), Some(first));
    assert_eq!(evaluated(&engine, ids[1]), Some(first));
    assert_eq!(evaluated(&engine, ids[2]), None);
    assert_eq!(cursor(&mut engine), Some(ids[1]));

    engine.advance_live();
    assert_eq!(results(&engine).deferred(), [ids[0], ids[1]]);
    assert_eq!(evaluated(&engine, ids[2]), Some(first + 1));
    assert_eq!(evaluated(&engine, ids[3]), Some(first + 1));
    assert_eq!(
        evaluated(&engine, ids[0]),
        Some(first),
        "a deferred perceiver keeps its old samples"
    );
    assert_eq!(cursor(&mut engine), Some(ids[3]));

    engine.advance_live();
    assert_eq!(evaluated(&engine, ids[0]), Some(first + 2));
    assert_eq!(cursor(&mut engine), Some(ids[1]));
    assert_eq!(
        results(&engine).rays_cast(),
        2,
        "the budget caps the rays once physics exists"
    );
    for id in &ids {
        let waited = now(&engine) - evaluated(&engine, *id).unwrap();
        assert!(waited <= 1, "every perceiver is served within two ticks");
    }
}

#[test]
fn a_perceiver_larger_than_the_budget_is_still_served() {
    let text = scene(&[
        guard("big", 0.0, 1, ""),
        guard("small", 1.0, 1, ""),
        mark("a", [0.0, 0.0, -10.0]),
        mark("b", [1.0, 0.0, -10.0]),
        mark("c", [2.0, 0.0, -10.0]),
    ]);
    let mut engine = engine(Some(1), &text);
    let big = find(&mut engine, "big");
    let small = find(&mut engine, "small");
    engine.advance_live();
    assert_eq!(results(&engine).sight(big).len(), 3);
    assert_eq!(results(&engine).deferred(), [small]);
    engine.advance_live();
    assert_eq!(
        results(&engine).sight(small).len(),
        3,
        "the next turn is the small one's, and it is served over budget too"
    );
}

#[test]
fn a_starved_perceiver_reports_l4221_once() {
    let mut engine = engine(Some(1), &crowd(8, 1, ""));
    run(&mut engine, 4);
    assert_eq!(reported(&engine, code::PERCEPTION_DEFERRED), 0);
    run(&mut engine, 12);
    assert_eq!(reported(&engine, code::PERCEPTION_DEFERRED), 1);

    let mut roomy = self::engine(Some(64), &crowd(8, 1, ""));
    run(&mut roomy, 16);
    assert_eq!(reported(&roomy, code::PERCEPTION_DEFERRED), 0);
}

#[test]
fn memory_is_refreshed_only_on_evaluation_ticks() {
    let mut engine = engine(None, &crowd(1, 3, "        memory: {}\n"));
    let guard = find(&mut engine, "g0");
    let phase = u64::from(guard.index() % 3);
    for _ in 0..12 {
        engine.advance_live();
        let tick = now(&engine);
        let records: Vec<StimulusRecord> = engine
            .world_mut()
            .query_ids::<(&StimulusRecord,)>()
            .map(|(_, (r,))| r.clone())
            .collect();
        if tick < phase {
            assert!(records.is_empty());
            continue;
        }
        let last_phase = tick - (tick - phase) % 3;
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].last_sensed_tick, last_phase);
        assert_eq!(records[0].sensed_now, tick == last_phase, "tick {tick}");
    }
}

fn walker_path(world: &mut World, ctx: &SystemCtx) {
    let x = -5.0 + 0.2 * ctx.frame.as_u64() as f32;
    let ids: Vec<EntityId> = world
        .query_ids::<(&Name,)>()
        .filter(|(_, (n,))| n.0 == "mark")
        .map(|(id, _)| id)
        .collect();
    for id in ids {
        world.get_mut::<Transform>(id).unwrap().unwrap().translation = [x, 0.0, -10.0];
    }
}

fn moving() -> EngineBuilder {
    builder(Some(3)).system("walker_path", walker_path)
}

/// Six guards on mixed periods and a budget, one with memory, watching a moving mark.
fn busy() -> String {
    let mut entities: Vec<String> = (0..6)
        .map(|i| {
            let memory = if i == 0 { "        memory: {}\n" } else { "" };
            guard(&format!("g{i}"), i as f32, 1 + (i % 3), memory)
        })
        .collect();
    entities.push(mark("mark", [-5.0, 0.0, -10.0]));
    scene(&entities)
}

fn trace(engine: &mut Engine, ticks: usize) -> Vec<(u64, Vec<SightSample>)> {
    (0..ticks)
        .map(|_| {
            engine.advance_live();
            (
                engine.world_hash().unwrap().as_u64(),
                results(engine).samples().to_vec(),
            )
        })
        .collect()
}

#[test]
fn a_restore_rejoins_the_uninterrupted_run_with_stagger_and_budget_on() {
    let text = busy();
    let mut whole = engine_with(moving(), &text);
    let reference = trace(&mut whole, 60);

    let mut first = engine_with(moving(), &text);
    let head = trace(&mut first, 25);
    assert_eq!(head, reference[..25]);
    let snapshot = match first.execute(Command::EngineSnapshot).unwrap().body {
        ResponseBody::Snapshot { snapshot } => snapshot,
        other => panic!("expected a snapshot, got {other:?}"),
    };
    let text_form = snapshot.to_string();
    assert!(text_form.contains("SightSampleRecord") && text_form.contains("PerceiverStatus"));
    let mut resumed = engine_with(moving(), &text);
    match resumed
        .execute(Command::EngineRestore(EngineRestoreParams {
            snapshot: snapshot as Value,
        }))
        .unwrap()
        .body
    {
        ResponseBody::Restored { .. } => {}
        other => panic!("expected a restore, got {other:?}"),
    }
    assert_eq!(trace(&mut resumed, 35), reference[25..]);
}

#[test]
fn stagger_and_budget_are_identical_at_any_thread_count() {
    let text = busy();
    let at = |threads: usize| {
        par::with_threads(threads, || {
            let mut engine = engine_with(moving(), &text);
            trace(&mut engine, 40)
        })
    };
    assert_eq!(at(1), at(8));
}

#[test]
fn a_world_with_no_stagger_and_no_budget_hashes_like_one_that_never_ran_the_update() {
    let text = crowd(3, 1, "");
    let mut with = engine(None, &text);
    let mut without = builder(None).build().expect("boots");
    without.load_inline_scene(&text).unwrap();
    let hashes = |engine: &mut Engine| -> Vec<u64> {
        (0..30)
            .map(|_| {
                engine.advance_live();
                engine.world_hash().unwrap().as_u64()
            })
            .collect()
    };
    let plain = hashes(&mut with);
    assert_eq!(plain, hashes(&mut without));
    assert_eq!(count::<SightSampleRecord>(&mut with), 0);
    assert_eq!(count::<PerceiverStatus>(&mut with), 0);
    assert_eq!(count::<PerceptionState>(&mut with), 0);
    assert!(
        !results(&with).samples().is_empty(),
        "the guards do see the mark"
    );

    let mut staggered = engine(None, &crowd(3, 2, ""));
    assert_ne!(
        hashes(&mut staggered),
        plain,
        "a stagger persists samples, so the hash moves"
    );
}

#[test]
fn records_and_status_despawn_with_a_disabled_or_dead_perceiver() {
    let mut engine = engine(None, &crowd(3, 2, ""));
    run(&mut engine, 6);
    assert_eq!(count::<SightSampleRecord>(&mut engine), 3);
    assert_eq!(count::<PerceiverStatus>(&mut engine), 3);

    let g0 = find(&mut engine, "g0");
    let g1 = find(&mut engine, "g1");
    engine
        .world_mut()
        .get_mut::<ludion_engine::Perceiver>(g0)
        .unwrap()
        .unwrap()
        .enabled = false;
    engine.advance_live();
    assert_eq!(count::<SightSampleRecord>(&mut engine), 2);
    assert_eq!(count::<PerceiverStatus>(&mut engine), 2);

    engine.world_mut().despawn(g1).unwrap();
    engine.advance_live();
    assert_eq!(count::<SightSampleRecord>(&mut engine), 1);
    assert_eq!(count::<PerceiverStatus>(&mut engine), 1);
}

#[test]
fn dropping_the_stagger_clears_what_it_stored() {
    let mut engine = engine(None, &crowd(2, 2, ""));
    run(&mut engine, 4);
    assert_eq!(count::<SightSampleRecord>(&mut engine), 2);
    let ids: Vec<EntityId> = (0..2)
        .map(|i| find(&mut engine, &format!("g{i}")))
        .collect();
    for id in ids {
        engine
            .world_mut()
            .get_mut::<ludion_engine::Perceiver>(id)
            .unwrap()
            .unwrap()
            .period_ticks = 1;
    }
    engine.advance_live();
    assert_eq!(count::<SightSampleRecord>(&mut engine), 0);
    assert_eq!(count::<PerceiverStatus>(&mut engine), 0);
}

#[test]
fn describe_carries_the_ray_count_and_the_deferred_perceivers() {
    let mut engine = engine(Some(2), &crowd(4, 1, ""));
    run(&mut engine, 2);
    let description = match engine.execute(Command::EngineDescribe).unwrap().body {
        ResponseBody::EngineDescription { description } => description,
        other => panic!("expected a description, got {other:?}"),
    };
    assert_eq!(description["perception"]["rays_cast"], 2);
    assert_eq!(
        description["perception"]["deferred"]
            .as_array()
            .unwrap()
            .len(),
        2
    );

    let mut bare = Engine::builder().build().unwrap();
    let description = match bare.execute(Command::EngineDescribe).unwrap().body {
        ResponseBody::EngineDescription { description } => description,
        other => panic!("expected a description, got {other:?}"),
    };
    assert!(description.get("perception").is_none());
}

#[test]
fn the_registry_lists_the_entity_refs_of_a_carried_sample() {
    let registry = ludion_engine::builtin_registry();
    let info = registry
        .iter()
        .find(|i| i.name() == "SightSampleRecord")
        .unwrap();
    let mut refs: Vec<String> = info
        .entity_refs()
        .iter()
        .map(|r| r.suffix.clone())
        .collect();
    refs.sort();
    assert_eq!(refs, ["/blocker", "/perceiver", "/target"]);
}

#[test]
fn a_zero_budget_is_refused_at_build() {
    let error = builder(Some(0))
        .build()
        .err()
        .expect("a budget of 0 is refused");
    assert_eq!(error.code, code::PERCEPTION_CONFIG_INVALID);
    assert_eq!(error.path.as_deref(), Some("/max_sight_rays_per_tick"));
}

#[test]
fn a_never_evaluated_staggered_perceiver_is_not_starved_by_the_budget() {
    let mut engine = engine(Some(1), &crowd(16, 3, ""));
    let ids: Vec<EntityId> = (0..16)
        .map(|i| find(&mut engine, &format!("g{i}")))
        .collect();
    let mut served = vec![false; ids.len()];
    for _ in 0..50 {
        engine.advance_live();
        let tick = now(&engine);
        for (i, id) in ids.iter().enumerate() {
            served[i] |= evaluated(&engine, *id) == Some(tick);
        }
    }
    assert!(
        served.iter().all(|s| *s),
        "every perceiver was served: {served:?}"
    );
    assert_eq!(
        reported(&engine, code::PERCEPTION_DEFERRED),
        1,
        "L4221 measures a perceiver that was deferred before its first evaluation"
    );
}
