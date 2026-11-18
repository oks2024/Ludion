//! Runtime proof for hearing (`Plan/perception.md` Wave 3): noises reach listeners by zone, surface,
//! sensitivity and occluder, deliver on the right tick, feed memory, survive a snapshot and are
//! thread-count invariant.

use ludion_core::codes::code;
use ludion_core::ids::EntityId;
use ludion_core::par;
use ludion_core::time::{FrameIndex, TickRate};
use ludion_ecs::{SystemCtx, World};
use ludion_engine::diagnostics::Diagnostics;
use ludion_engine::{
    Engine, EngineBuilder, HeardNoise, Name, Noise, NoiseId, NoiseRecord, PerceptionConfig,
    PerceptionResults, Sense, StimulusRecord, SurfaceNoise, emit_noise, perception_update, stimuli,
};
use ludion_physics::{LayerSet, SurfaceId};
use ludion_protocol::command::EngineRestoreParams;
use ludion_protocol::{Command, ResponseBody};
use serde_json::Value;

fn entity(name: &str, at: [f32; 3], components: &str) -> String {
    format!(
        "  - name: {name}\n    components:\n      Transform:\n        translation:\n          - {:?}\n          - {:?}\n          - {:?}\n{components}",
        at[0], at[1], at[2]
    )
}

/// A listener at `x` on the X axis; `extra` is YAML lines at 10 spaces under `hearing:`, and
/// `memory` adds a default memory.
fn listener_with(name: &str, x: f32, extra: &str, memory: bool) -> String {
    let memory = if memory { "        memory: {}\n" } else { "" };
    entity(
        name,
        [x, 0.0, 0.0],
        &format!("      Perceiver:\n        hearing:\n{extra}{memory}"),
    )
}

const HEARS_FOOTSTEP: &str = "          hears:\n            - footstep\n";

fn listener(name: &str, x: f32) -> String {
    listener_with(name, x, HEARS_FOOTSTEP, false)
}

fn zone(name: &str, extra: &str) -> String {
    entity(
        name,
        [0.0; 3],
        &format!(
            "      NoiseZone:\n        shape:\n          box:\n            half_extents:\n              - 5.0\n              - 5.0\n              - 5.0\n{extra}"
        ),
    )
}

/// A thin wall across the X axis at `x`, on `sound_wall`.
fn wall(x: f32) -> String {
    entity(
        &format!("wall{x}"),
        [x, 0.0, 0.0],
        "      Collider:\n        shape:\n          box:\n            half_extents:\n              - 0.1\n              - 3.0\n              - 3.0\n        body: static\n        layer: sound_wall\n",
    )
}

fn scene(entities: &[String]) -> String {
    let mut out = String::from(
        "format: 1\nseed: 1\nphysics:\n  space: free\n  layers:\n    - name: sound_wall\n  surfaces:\n    - concrete\n    - metal_grate\nentities:\n",
    );
    for e in entities {
        out += e;
    }
    out
}

fn config() -> PerceptionConfig {
    PerceptionConfig {
        noise_kinds: vec!["footstep".into(), "bang".into()],
        surface_noise: vec![SurfaceNoise {
            surface: "metal_grate".into(),
            multiplier: 1.5,
        }],
        sound_occluder_layers: LayerSet::from(vec!["sound_wall".to_string()]),
        ..Default::default()
    }
}

fn builder_with(config: PerceptionConfig) -> EngineBuilder {
    Engine::builder().perception(config)
}

fn engine_with(builder: EngineBuilder, text: &str) -> Engine {
    let mut engine = builder
        .system("perception_update", perception_update)
        .build()
        .expect("boots");
    engine.load_inline_scene(text).expect("the scene loads");
    engine
}

fn engine(text: &str) -> Engine {
    engine_with(builder_with(config()), text)
}

fn find(engine: &mut Engine, wanted: &str) -> EntityId {
    engine
        .world_mut()
        .query_ids::<(&Name,)>()
        .find(|(_, (name,))| name.0 == wanted)
        .map(|(id, _)| id)
        .unwrap_or_else(|| panic!("no entity named {wanted}"))
}

fn ctx() -> SystemCtx {
    SystemCtx::new(FrameIndex::new(0), TickRate::DEFAULT)
}

fn noise(at: [f32; 3], radius: f32, kind: &str) -> Noise {
    Noise {
        at,
        radius,
        loudness: 1.0,
        kind: kind.into(),
        source: None,
        surface: None,
        occlusion: false,
    }
}

fn emit(engine: &mut Engine, noise: Noise) -> Option<NoiseId> {
    emit_noise(engine.world_mut(), &ctx(), noise)
}

fn results(engine: &Engine) -> &PerceptionResults {
    engine.world().resource::<PerceptionResults>().unwrap()
}

/// Emits `noise` after the last update, runs one tick and returns who heard it.
fn emit_and_tick(engine: &mut Engine, noise: Noise, listeners: &[&str]) -> Vec<bool> {
    let ids: Vec<EntityId> = listeners.iter().map(|l| find(engine, l)).collect();
    assert!(emit(engine, noise).is_some());
    engine.advance_live();
    ids.iter()
        .map(|id| !results(engine).heard(*id).is_empty())
        .collect()
}

fn warmed(text: &str) -> Engine {
    let mut engine = engine(text);
    engine.advance_live();
    engine
}

#[test]
fn a_zone_multiplier_shrinks_the_reach_and_noises_report_the_source_side_radius() {
    let text = scene(&[
        zone("rain", "        multiplier: 0.5\n"),
        listener("near", 4.0),
        listener("far", 6.0),
    ]);
    let mut engine = warmed(&text);
    // Radius 10 inside a ×0.5 zone reaches 5: 0.4 r hears, 0.6 r does not.
    let heard = emit_and_tick(
        &mut engine,
        noise([0.0; 3], 10.0, "footstep"),
        &["near", "far"],
    );
    assert_eq!(heard, [true, false]);
    let record: &NoiseRecord = &results(&engine).noises()[0];
    assert_eq!(record.effective_radius, 5.0);
    let near = find(&mut engine, "near");
    let got: &HeardNoise = &results(&engine).heard(near)[0];
    assert_eq!((got.distance, got.effective_radius), (4.0, 5.0));
    assert_eq!((got.kind.as_str(), got.loudness), ("footstep", 1.0));
}

#[test]
fn zones_match_by_kind_and_overlaps_go_by_priority_then_lowest_entity() {
    let kinds = "        multiplier: 0.5\n        kinds:\n          - bang\n";
    let mut engine = warmed(&scene(&[zone("rain", kinds), listener("far", 6.0)]));
    let heard = emit_and_tick(&mut engine, noise([0.0; 3], 10.0, "footstep"), &["far"]);
    assert_eq!(heard, [true], "a zone for another kind is ignored");

    let tie = scene(&[
        zone("first", "        multiplier: 0.5\n"),
        zone("second", "        multiplier: 0.1\n"),
        listener("near", 4.0),
    ]);
    let mut engine = warmed(&tie);
    emit_and_tick(&mut engine, noise([0.0; 3], 10.0, "footstep"), &["near"]);
    assert_eq!(results(&engine).noises()[0].effective_radius, 5.0);

    let ranked = scene(&[
        zone("first", "        multiplier: 0.5\n"),
        zone("second", "        multiplier: 0.1\n        priority: 1\n"),
        listener("near", 4.0),
    ]);
    let mut engine = warmed(&ranked);
    let heard = emit_and_tick(&mut engine, noise([0.0; 3], 10.0, "footstep"), &["near"]);
    assert_eq!(heard, [false]);
    assert_eq!(results(&engine).noises()[0].effective_radius, 1.0);
}

#[test]
fn a_surface_multiplier_scales_the_radius_and_unlisted_surfaces_are_times_one() {
    // A collider makes the physics world (and its surface table) exist.
    let text = scene(&[wall(-50.0), listener("in", 14.0), listener("out", 16.0)]);
    let mut engine = warmed(&text);
    let on_grate = Noise {
        surface: Some(SurfaceId(1)),
        ..noise([0.0; 3], 10.0, "footstep")
    };
    assert_eq!(
        emit_and_tick(&mut engine, on_grate, &["in", "out"]),
        [true, false]
    );
    assert_eq!(results(&engine).noises()[0].effective_radius, 15.0);

    for surface in [Some(SurfaceId(0)), None] {
        let plain = Noise {
            surface,
            ..noise([0.0; 3], 10.0, "footstep")
        };
        emit_and_tick(&mut engine, plain, &[]);
        assert_eq!(results(&engine).noises()[0].effective_radius, 10.0);
    }
}

#[test]
fn sensitivity_scales_the_listeners_reach() {
    let keen = listener_with(
        "keen",
        19.0,
        "          sensitivity: 2.0\n          hears:\n            - footstep\n",
        false,
    );
    let mut engine = warmed(&scene(&[keen, listener("plain", 19.0)]));
    let heard = emit_and_tick(
        &mut engine,
        noise([0.0; 3], 10.0, "footstep"),
        &["keen", "plain"],
    );
    assert_eq!(heard, [true, false]);
    let keen = find(&mut engine, "keen");
    assert_eq!(results(&engine).heard(keen)[0].effective_radius, 20.0);
}

fn occluded(walls: &[f32], config: PerceptionConfig, occlusion: bool) -> Vec<bool> {
    let mut entities: Vec<String> = walls.iter().map(|x| wall(*x)).collect();
    entities.extend([
        listener("l9", 9.0),
        listener("l11", 11.0),
        listener("l21", 21.0),
    ]);
    let mut engine = engine_with(builder_with(config), &scene(&entities));
    engine.advance_live();
    let hit = Noise {
        occlusion,
        ..noise([0.0; 3], 40.0, "footstep")
    };
    emit_and_tick(&mut engine, hit, &["l9", "l11", "l21"])
}

fn occluder_count(walls: &[f32], config: PerceptionConfig, occlusion: bool) -> usize {
    let mut entities: Vec<String> = walls.iter().map(|x| wall(*x)).collect();
    entities.push(listener("ear", 5.0));
    let mut engine = engine_with(builder_with(config), &scene(&entities));
    engine.advance_live();
    let hit = Noise {
        occlusion,
        ..noise([0.0; 3], 40.0, "footstep")
    };
    let ear = find(&mut engine, "ear");
    assert!(emit(&mut engine, hit).is_some());
    engine.advance_live();
    let heard = results(&engine).heard(ear);
    assert_eq!(heard.len(), 1);
    heard[0].occluders as usize
}

#[test]
fn heard_noise_reports_the_occluders_counted() {
    assert_eq!(occluder_count(&[2.0], config(), true), 1);
    assert_eq!(occluder_count(&[2.0, 3.0], config(), true), 2);
    let capped = PerceptionConfig {
        max_occluders: 1,
        ..config()
    };
    assert_eq!(occluder_count(&[2.0, 3.0], capped, true), 1);
    assert_eq!(occluder_count(&[2.0, 3.0], config(), false), 0);
}

#[test]
fn occluders_halve_the_radius_each_up_to_the_cap() {
    // Radius 40: no wall reaches 40, one wall 20, two walls 10.
    assert_eq!(occluded(&[], config(), true), [true, true, true]);
    assert_eq!(occluded(&[2.0], config(), true), [true, true, false]);
    assert_eq!(occluded(&[2.0, 3.0], config(), true), [true, false, false]);
    let capped = PerceptionConfig {
        max_occluders: 1,
        ..config()
    };
    assert_eq!(occluded(&[2.0, 3.0], capped, true), [true, true, false]);
    let gentle = PerceptionConfig {
        occlusion_factor: 0.75,
        ..config()
    };
    assert_eq!(occluded(&[2.0], gentle, true), [true, true, true]);
}

#[test]
fn a_noise_without_occlusion_or_a_world_without_occluder_layers_ignores_walls() {
    assert_eq!(occluded(&[2.0, 3.0], config(), false), [true, true, true]);
    let none = PerceptionConfig {
        sound_occluder_layers: LayerSet::default(),
        ..config()
    };
    assert_eq!(occluded(&[2.0, 3.0], none, true), [true, true, true]);
}

#[test]
fn a_listener_hears_only_the_kinds_it_lists() {
    let both = "          hears:\n            - footstep\n            - bang\n";
    let text = scene(&[
        listener("steps", 2.0),
        listener_with("all", 2.0, both, false),
    ]);
    let mut engine = warmed(&text);
    let heard = emit_and_tick(
        &mut engine,
        noise([0.0; 3], 10.0, "bang"),
        &["steps", "all"],
    );
    assert_eq!(heard, [false, true]);
}

#[derive(Clone)]
struct EmitPlan(u64, Noise);

fn emitter(world: &mut World, ctx: &SystemCtx) {
    let Some(plan) = world.resource::<EmitPlan>().cloned() else {
        return;
    };
    if ctx.frame.as_u64() == plan.0 {
        emit_noise(world, ctx, plan.1);
    }
}

#[test]
fn a_noise_emitted_before_the_update_lands_that_tick_and_one_after_it_the_next() {
    let text = scene(&[listener("ear", 2.0)]);
    let mut engine = engine_with(builder_with(config()).system("emitter", emitter), &text);
    let ear = find(&mut engine, "ear");
    engine
        .world_mut()
        .insert_resource(EmitPlan(3, noise([0.0; 3], 10.0, "footstep")));
    let mut ticks_heard = Vec::new();
    for tick in 0..6 {
        engine.advance_live();
        if !results(&engine).heard(ear).is_empty() {
            ticks_heard.push(tick);
        }
    }
    assert_eq!(
        ticks_heard,
        [3],
        "emitted by an earlier system, delivered at once"
    );

    emit(&mut engine, noise([0.0; 3], 10.0, "footstep")).unwrap();
    assert!(results(&engine).heard(ear).is_empty(), "nothing yet");
    engine.advance_live();
    assert_eq!(
        results(&engine).heard(ear).len(),
        1,
        "delivered the next tick"
    );
    engine.advance_live();
    assert!(results(&engine).heard(ear).is_empty(), "delivered once");
    assert!(results(&engine).noises().is_empty());
}

fn records(engine: &mut Engine, who: &str) -> Vec<StimulusRecord> {
    let id = find(engine, who);
    stimuli(engine.world_mut(), id)
        .into_iter()
        .map(|(_, r)| r)
        .collect()
}

#[test]
fn hearing_records_merge_per_source_and_anonymous_ones_per_kind() {
    let both = "          hears:\n            - footstep\n            - bang\n";
    let text = scene(&[
        listener_with("guard", 2.0, both, true),
        entity("a", [1.0, 0.0, 0.0], ""),
        entity("b", [0.0, 0.0, 1.0], ""),
    ]);
    let mut engine = warmed(&text);
    let (a, b) = (find(&mut engine, "a"), find(&mut engine, "b"));
    let from = |source, at, loudness, kind: &str| Noise {
        source,
        loudness,
        ..noise(at, 10.0, kind)
    };
    for n in [
        from(Some(a), [1.0, 0.0, 0.0], 0.5, "footstep"),
        from(Some(b), [0.0, 0.0, 1.0], 0.7, "footstep"),
        from(Some(a), [1.5, 0.0, 0.0], 0.9, "bang"),
        from(None, [3.0, 0.0, 0.0], 0.2, "footstep"),
        from(None, [4.0, 0.0, 0.0], 0.3, "footstep"),
        from(None, [5.0, 0.0, 0.0], 0.4, "bang"),
    ] {
        assert!(emit(&mut engine, n).is_some());
    }
    engine.advance_live();
    let all = records(&mut engine, "guard");
    assert_eq!(
        all.len(),
        4,
        "a, b, anonymous footstep, anonymous bang: {all:?}"
    );
    assert!(
        all.iter()
            .all(|r| r.sense == Sense::Hearing && r.sensed_now)
    );
    let of_a = all.iter().find(|r| r.source == Some(a)).unwrap();
    assert_eq!((of_a.last_position, of_a.strength), ([1.5, 0.0, 0.0], 0.9));
    let anonymous: Vec<_> = all.iter().filter(|r| r.source.is_none()).collect();
    let steps = anonymous.iter().find(|r| r.kind == "footstep").unwrap();
    assert_eq!(
        (steps.last_position, steps.strength),
        ([4.0, 0.0, 0.0], 0.3)
    );
    assert!(anonymous.iter().any(|r| r.kind == "bang"));
}

#[test]
fn a_listener_without_memory_keeps_no_records() {
    let mut engine = warmed(&scene(&[listener("ear", 2.0)]));
    emit_and_tick(&mut engine, noise([0.0; 3], 10.0, "footstep"), &["ear"]);
    assert!(records(&mut engine, "ear").is_empty());
}

fn reported(engine: &Engine, code: ludion_core::diagnostics::DiagnosticCode) -> usize {
    engine
        .world()
        .resource::<Diagnostics>()
        .unwrap()
        .reported(code)
        .count()
}

#[test]
fn an_invalid_noise_is_dropped_and_reported_once() {
    let mut engine = warmed(&scene(&[listener("ear", 2.0)]));
    for bad in [f32::NAN, -1.0, f32::INFINITY] {
        assert!(emit(&mut engine, noise([0.0; 3], bad, "footstep")).is_none());
    }
    let loud = Noise {
        loudness: f32::NAN,
        ..noise([0.0; 3], 1.0, "footstep")
    };
    assert!(emit(&mut engine, loud).is_none());
    assert!(emit(&mut engine, noise([f32::NAN, 0.0, 0.0], 1.0, "footstep")).is_none());
    assert_eq!(reported(&engine, code::NOISE_INVALID), 1);

    assert!(emit(&mut engine, noise([0.0; 3], 1.0, "footstp")).is_none());
    assert!(emit(&mut engine, noise([0.0; 3], 1.0, "footstp")).is_none());
    assert_eq!(reported(&engine, code::PERCEPTION_NAME_UNKNOWN), 1);

    engine.advance_live();
    assert!(results(&engine).noises().is_empty(), "nothing was queued");
    assert_eq!(
        engine
            .world_mut()
            .query_ids::<(&ludion_engine::PendingNoise,)>()
            .count(),
        0
    );
}

#[test]
fn a_game_with_no_perception_config_drops_a_noise_silently() {
    let mut world = World::new();
    assert!(emit_noise(&mut world, &ctx(), noise([0.0; 3], 1.0, "footstep")).is_none());
    assert_eq!(
        world
            .query_ids::<(&ludion_engine::PerceptionState,)>()
            .count(),
        0
    );
}

#[test]
fn noise_ids_rise_by_one_and_the_registry_lists_the_source_ref() {
    let mut engine = warmed(&scene(&[listener("ear", 2.0)]));
    let ids: Vec<NoiseId> = (0..3)
        .map(|_| emit(&mut engine, noise([0.0; 3], 1.0, "footstep")).unwrap())
        .collect();
    assert_eq!(ids, [NoiseId(0), NoiseId(1), NoiseId(2)]);
    let registry = ludion_engine::builtin_registry();
    let info = registry
        .iter()
        .find(|i| i.name() == "PendingNoise")
        .unwrap();
    let refs: Vec<String> = info
        .entity_refs()
        .iter()
        .map(|r| r.suffix.clone())
        .collect();
    assert_eq!(refs, ["/source"]);
}

fn trace(engine: &mut Engine, ticks: usize) -> Vec<u64> {
    (0..ticks)
        .map(|_| {
            engine.advance_live();
            engine.world_hash().unwrap().as_u64()
        })
        .collect()
}

fn snapshot_text() -> String {
    let both = "          hears:\n            - footstep\n";
    scene(&[listener_with("guard", 3.0, both, true), wall(1.0)])
}

fn emit_far(engine: &mut Engine) {
    let n = Noise {
        occlusion: true,
        loudness: 0.8,
        ..noise([0.0; 3], 20.0, "footstep")
    };
    emit(engine, n).unwrap();
}

#[test]
fn a_snapshot_between_emission_and_delivery_replays_the_same_heard_and_hashes() {
    let text = snapshot_text();
    let mut whole = engine(&text);
    trace(&mut whole, 5);
    emit_far(&mut whole);
    let reference = trace(&mut whole, 10);

    let mut first = engine(&text);
    trace(&mut first, 5);
    emit_far(&mut first);
    let snapshot = match first.execute(Command::EngineSnapshot).unwrap().body {
        ResponseBody::Snapshot { snapshot } => snapshot,
        other => panic!("expected a snapshot, got {other:?}"),
    };
    assert!(snapshot.to_string().contains("PendingNoise"));
    let mut resumed = engine(&text);
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
    resumed.advance_live();
    let resumed_guard = find(&mut resumed, "guard");
    let heard = results(&resumed).heard(resumed_guard).to_vec();
    assert_eq!(heard.len(), 1, "the restored noise was delivered");
    assert_eq!(heard[0].loudness, 0.8);
    let mut got = vec![resumed.world_hash().unwrap().as_u64()];
    got.extend(trace(&mut resumed, 9));
    assert_eq!(got, reference);

    let mut again = engine(&text);
    trace(&mut again, 5);
    emit_far(&mut again);
    again.advance_live();
    let again_guard = find(&mut again, "guard");
    assert_eq!(results(&again).heard(again_guard), heard.as_slice());
}

#[test]
fn deliveries_are_identical_at_any_thread_count() {
    let both = "          hears:\n            - footstep\n";
    let mut entities = vec![wall(2.0)];
    entities
        .extend((0..12).map(|i| listener_with(&format!("l{i}"), 1.0 + i as f32, both, i % 2 == 0)));
    let text = scene(&entities);
    let at = |threads: usize| {
        par::with_threads(threads, || {
            let mut engine = engine(&text);
            engine.advance_live();
            let ids: Vec<EntityId> = (0..12)
                .map(|i| find(&mut engine, &format!("l{i}")))
                .collect();
            let mut out = Vec::new();
            for tick in 0..6 {
                if tick % 2 == 0 {
                    emit_far(&mut engine);
                }
                engine.advance_live();
                out.push((
                    engine.world_hash().unwrap().as_u64(),
                    ids.iter()
                        .flat_map(|id| results(&engine).heard(*id).to_vec())
                        .collect::<Vec<HeardNoise>>(),
                ));
            }
            out
        })
    };
    assert_eq!(at(1), at(8));
}

#[test]
fn a_world_that_never_emits_hashes_like_one_without_the_update() {
    let text = scene(&[listener("ear", 2.0), entity("p", [0.0; 3], "")]);
    let mut with = engine(&text);
    let mut without = Engine::builder().build().expect("boots");
    without.load_inline_scene(&text).unwrap();
    assert_eq!(trace(&mut with, 20), trace(&mut without, 20));
}
