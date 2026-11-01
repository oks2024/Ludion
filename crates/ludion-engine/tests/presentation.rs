//! Presentation state (`Plan/presentation-state.md`) through the engine: an external-game-shaped game whose
//! popups live in the presentation range. They are drawn, bounded, snapshotted and restored, and
//! nothing about them moves a hash, a keyframe verdict or a sim id.

use std::io::Write;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use ludion_core::ids::EntityId;
use ludion_ecs::{PRESENTATION_BASE, World};
use ludion_engine::{
    AudioQueue, Engine, EngineBuilder, KEYFRAME_INTERVAL, Name, Sprite, Transform, dispatch,
    extract, replay_hashes_with, report_diagnostic, verify_replay_keyframes,
};
use ludion_protocol::command::{EngineRestoreParams, RenderBoundsParams, SimStepParams};
use ludion_protocol::replay::{ReplayReader, ReplayRecord};
use ludion_protocol::{Command, ResponseBody};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Serialize, Deserialize, JsonSchema, Clone, Copy, PartialEq, Debug)]
struct Score {
    n: u64,
}

#[derive(Serialize, Deserialize, JsonSchema, Clone, Copy, PartialEq, Debug)]
struct Popup {
    born: u64,
    score: u64,
}

const SCENE: &str = "\
format: 1
seed: 3
entities:
  - name: camera
    components:
      Transform:
        translation:
          - 0.0
          - 0.0
          - 10.0
      Camera:
        projection:
          type: orthographic
          height: 4.0
  - name: ball
    components:
      Transform: {}
      Score:
        n: 0
";

/// Scratch project root: an empty directory per test name, wiped on entry.
fn fresh_dir(name: &str) -> PathBuf {
    let dir =
        std::env::temp_dir().join(format!("ludion-presentation-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn ball(world: &mut World) -> EntityId {
    world
        .query_ids::<(&Score,)>()
        .next()
        .map(|(id, _)| id)
        .expect("the scene's ball")
}

/// Sim: bump the ball's score; every 7th tick spawn a sim entity, so sim ids are exercised.
fn score(world: &mut World, ctx: &ludion_ecs::SystemCtx) {
    for (s,) in world.query::<(&mut Score,)>() {
        s.n += 1;
    }
    if (ctx.frame.as_u64() + 1).is_multiple_of(7) {
        let id = world.spawn();
        world.insert(id, Transform::default()).unwrap();
    }
}

/// Presentation: a popup every 3rd tick reading the ball's score, retired after 3 ticks so
/// presentation slots recycle; one audio event proves the outbox allowlist.
fn popups(world: &mut World, ctx: &ludion_ecs::SystemCtx) {
    let now = ctx.frame.as_u64();
    let ball = ball(world);
    let current = world.get::<Score>(ball).unwrap().copied().unwrap();
    let expired: Vec<EntityId> = world
        .query_ids::<(&Popup,)>()
        .filter(|(_, (p,))| p.born + 3 < now)
        .map(|(id, _)| id)
        .collect();
    for id in expired {
        world.despawn(id).unwrap();
    }
    if now.is_multiple_of(3) {
        let id = world.spawn_presentation();
        world.insert(id, Transform::default()).unwrap();
        world.insert(id, Name(format!("popup/{now}"))).unwrap();
        world
            .insert(
                id,
                Sprite {
                    size: Some([1.0, 1.0]),
                    ..Sprite::default()
                },
            )
            .unwrap();
        world
            .insert(
                id,
                Popup {
                    born: now,
                    score: current.n,
                },
            )
            .unwrap();
        if let Some(queue) = world.resource_mut::<AudioQueue>() {
            queue.play("pop.wav", 1.0);
        }
    }
}

fn game(with_popups: bool) -> EngineBuilder {
    let builder = Engine::builder()
        .project_root(fresh_dir(if with_popups { "with" } else { "without" }))
        .components(|r| {
            r.register::<Score>("Score")?;
            r.register::<Popup>("Popup")
        })
        .system("score", score);
    if with_popups {
        builder.presentation_system("popups", popups)
    } else {
        builder
    }
}

fn loaded(with_popups: bool) -> Engine {
    let mut engine = game(with_popups).build().unwrap();
    engine.load_inline_scene(SCENE).unwrap();
    engine.set_render_size(256, 256);
    engine
}

fn advance_trace(engine: &mut Engine, n: u64) -> Vec<u64> {
    (0..n)
        .map(|_| {
            engine.advance_live();
            engine.world_hash().unwrap().as_u64()
        })
        .collect()
}

fn snapshot_of(engine: &mut Engine) -> Value {
    match engine.execute(Command::EngineSnapshot).unwrap().body {
        ResponseBody::Snapshot { snapshot } => snapshot,
        other => panic!("expected a snapshot, got {other:?}"),
    }
}

fn restore(
    engine: &mut Engine,
    snapshot: Value,
) -> Result<(), ludion_core::diagnostics::Diagnostic> {
    match engine
        .execute(Command::EngineRestore(EngineRestoreParams { snapshot }))?
        .body
    {
        ResponseBody::Restored { .. } => Ok(()),
        other => panic!("expected a restore, got {other:?}"),
    }
}

#[derive(Clone, Default)]
struct SharedSink(Arc<Mutex<Vec<u8>>>);

impl Write for SharedSink {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(buf);
        Ok(buf.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

/// Records `ticks` single-tick steps of the popups game; returns the `.lrp` bytes and the live end.
fn record(ticks: u64) -> (Vec<u8>, Engine) {
    let sink = SharedSink::default();
    let mut engine = game(true).build().unwrap();
    engine.load_inline_scene(SCENE).unwrap();
    engine.set_recording(sink.clone()).unwrap();
    for _ in 0..ticks {
        engine
            .execute(Command::SimStep(SimStepParams {
                ticks: 1,
                hash: false,
            }))
            .unwrap();
    }
    engine.stop_recording().unwrap();
    let bytes = sink.0.lock().unwrap().clone();
    (bytes, engine)
}

fn presentation_ids(engine: &Engine) -> Vec<EntityId> {
    engine.world().presentation_entities().collect()
}

/// (a) Presentation entities never enter the hash or move a sim id.
#[test]
fn presentation_entities_do_not_move_the_hash_trace_or_sim_ids() {
    let ticks = 2 * KEYFRAME_INTERVAL + 1;
    let mut a = loaded(false);
    let mut b = loaded(true);
    let trace_a = advance_trace(&mut a, ticks);
    let trace_b = advance_trace(&mut b, ticks);
    assert!(!presentation_ids(&b).is_empty(), "popups are alive");
    assert_eq!(trace_a, trace_b);
    assert_eq!(
        a.world().entities().collect::<Vec<_>>(),
        b.world().entities().collect::<Vec<_>>()
    );
}

/// (b) Replay reproduces the live trace; keyframes carry presentation state yet verify clean
/// against builds with and without the presentation system.
#[test]
fn replay_and_keyframe_verify_compare_sim_only() {
    let (bytes, engine) = record(2 * KEYFRAME_INTERVAL + 1);
    let mut engine = engine;
    let live = (engine.frame(), engine.world_hash().unwrap());
    let replayed = replay_hashes_with(game(true), &bytes, std::env::temp_dir()).unwrap();
    assert_eq!(replayed.last(), Some(&live));

    for with_popups in [true, false] {
        let report = verify_replay_keyframes(game(with_popups), &bytes, ".").unwrap();
        assert!(report.is_clean(), "with_popups={with_popups}:\n{report}");
        assert!(report.keyframes >= 3, "{}", report.keyframes);
    }

    let carries = ReplayReader::new(&bytes)
        .unwrap()
        .records()
        .unwrap()
        .iter()
        .any(|record| match record {
            ReplayRecord::Keyframe { snapshot, .. } => snapshot
                .get("presentation")
                .and_then(Value::as_array)
                .is_some_and(|p| !p.is_empty()),
            _ => false,
        });
    assert!(carries, "a keyframe carries presentation entities");
}

/// (c) Snapshot round trip, with a recycled presentation slot.
#[test]
fn a_snapshot_restores_presentation_entities_and_their_allocator() {
    let mut source = loaded(true);
    // Step until a recycled slot makes the allocator irreducible while popups are alive.
    let mut document = Value::Null;
    for _ in 0..100 {
        advance_trace(&mut source, 1);
        document = snapshot_of(&mut source);
        if document.get("presentation_allocator").is_some()
            && document.get("presentation").is_some()
        {
            break;
        }
    }
    assert!(
        document.get("presentation_allocator").is_some(),
        "a free slot at a recycled generation makes the allocator irreducible: {:?}",
        document["presentation"]
    );

    let mut resumed = loaded(true);
    restore(&mut resumed, document.clone()).unwrap();
    let ids = presentation_ids(&source);
    assert!(!ids.is_empty());
    assert_eq!(presentation_ids(&resumed), ids);
    for &id in &ids {
        for name in [
            source.world().get::<Popup>(id).unwrap().copied(),
            resumed.world().get::<Popup>(id).unwrap().copied(),
        ] {
            assert!(name.is_some());
        }
        assert_eq!(
            source.world().get::<Popup>(id).unwrap(),
            resumed.world().get::<Popup>(id).unwrap()
        );
        assert_eq!(
            source.world().get::<Name>(id).unwrap(),
            resumed.world().get::<Name>(id).unwrap()
        );
    }

    let after = advance_trace(&mut source, 20);
    assert_eq!(advance_trace(&mut resumed, 20), after);
    assert_eq!(presentation_ids(&resumed), presentation_ids(&source));

    let mut round = loaded(true);
    restore(&mut round, document.clone()).unwrap();
    let again = snapshot_of(&mut round);
    assert_eq!(
        serde_json::to_string(&again).unwrap(),
        serde_json::to_string(&document).unwrap()
    );
}

/// (d) A world with no presentation entities snapshots without the new keys.
#[test]
fn a_world_without_presentation_entities_snapshots_byte_identically() {
    let mut engine = loaded(false);
    advance_trace(&mut engine, 30);
    let document = snapshot_of(&mut engine);
    assert!(document.get("presentation").is_none());
    assert!(document.get("presentation_allocator").is_none());
}

/// (e) A presentation entity below the presentation range is refused before the engine is touched.
#[test]
fn a_presentation_entity_in_the_sim_range_is_refused() {
    let mut source = loaded(true);
    advance_trace(&mut source, 20);
    let mut document = snapshot_of(&mut source);
    document["presentation"][0]["index"] = Value::from(PRESENTATION_BASE - 1);

    let mut target = loaded(true);
    advance_trace(&mut target, 5);
    let (frame, hash) = (target.frame(), target.world_hash().unwrap());
    let error = restore(&mut target, document).unwrap_err();
    assert_eq!(error.code.as_str(), "L0025");
    assert!(
        error
            .path
            .as_deref()
            .is_some_and(|p| p.starts_with("/presentation")),
        "{:?}",
        error.path
    );
    assert_eq!(
        (target.frame(), target.world_hash().unwrap()),
        (frame, hash)
    );
}

/// (f) Presentation sprites are bounded and extracted.
#[test]
fn presentation_sprites_are_bounded_and_extracted() {
    let mut engine = loaded(true);
    advance_trace(&mut engine, 4);
    let popup = presentation_ids(&engine)[0];
    let ResponseBody::RenderBounds { entities, .. } = dispatch(
        &mut engine,
        Command::RenderBounds(RenderBoundsParams::default()),
    )
    .unwrap()
    .body
    else {
        panic!("expected RenderBounds");
    };
    assert!(entities.iter().any(|e| e.entity == popup));

    let scene = extract(
        engine.world(),
        (256, 256),
        ludion_assets::AssetRoot::content(&fresh_dir("extract")),
    )
    .unwrap();
    assert_eq!(scene.sprites.len(), presentation_ids(&engine).len());
}

static CAUGHT: Mutex<Vec<(String, String)>> = Mutex::new(Vec::new());

/// A presentation system that writes sim state is refused with a message naming both sides.
fn trespass(world: &mut World, _ctx: &ludion_ecs::SystemCtx) {
    let ball = ball(world);
    if let Err(diagnostic) = world.get_mut::<Score>(ball) {
        CAUGHT.lock().unwrap().push((
            diagnostic.code.as_str().to_string(),
            diagnostic.message.clone(),
        ));
        report_diagnostic(world, diagnostic.code, "ball", diagnostic.message);
    }
}

/// (g) The L0028 refusal names the entity by its `Name` and the system.
#[test]
fn a_presentation_write_to_sim_state_names_the_entity_and_the_system() {
    let mut engine = Engine::builder()
        .project_root(fresh_dir("trespass"))
        .components(|r| r.register::<Score>("Score"))
        .presentation_system("popups", trespass)
        .build()
        .unwrap();
    engine.load_inline_scene(SCENE).unwrap();
    engine.advance_live();
    let caught = CAUGHT.lock().unwrap();
    let (code, message) = caught.first().expect("the write was refused");
    assert_eq!(code, "L0028");
    assert!(message.contains("`ball`"), "{message}");
    assert!(message.contains("popups"), "{message}");
}

fn named(name: &str, presentation: bool, extra: &str) -> String {
    let flag = if presentation {
        "    presentation: true\n"
    } else {
        ""
    };
    format!("  - name: {name}\n{flag}    components:\n      Transform: {{}}\n{extra}")
}

fn parent_of(position: usize) -> String {
    format!("      Parent:\n        index: {position}\n        generation: 0\n")
}

fn scene_of(entities: &[String]) -> String {
    format!("format: 1\nentities:\n{}", entities.concat())
}

fn engine_with(entities: &[String]) -> Engine {
    let mut engine = Engine::new();
    engine.load_inline_scene(&scene_of(entities)).unwrap();
    engine
}

fn name_of(engine: &Engine, id: EntityId) -> Option<String> {
    engine
        .world()
        .get::<Name>(id)
        .unwrap()
        .map(|name| name.0.clone())
}

/// (h) A presentation entity between two sim entities leaves the sim ids, the `Name`s and the hash
/// as if it were absent, and `query.entities` lists it.
#[test]
fn a_scene_presentation_entity_between_sim_entities_is_invisible_to_the_sim() {
    let mut with = engine_with(&[
        named("a", false, ""),
        named("popup", true, ""),
        named("b", false, ""),
    ]);
    let mut without = engine_with(&[named("a", false, ""), named("b", false, "")]);

    let sim: Vec<EntityId> = with.world().entities().collect();
    assert_eq!(sim, without.world().entities().collect::<Vec<_>>());
    assert_eq!(name_of(&with, sim[0]).as_deref(), Some("a"));
    assert_eq!(name_of(&with, sim[1]).as_deref(), Some("b"));
    let popup = presentation_ids(&with);
    assert_eq!(popup, [EntityId::new(PRESENTATION_BASE, 0)]);
    assert_eq!(name_of(&with, popup[0]).as_deref(), Some("popup"));
    assert_eq!(with.world_hash().unwrap(), without.world_hash().unwrap());

    let mut with = with;
    let body = with
        .execute(Command::QueryEntities(
            ludion_protocol::command::QueryEntitiesParams {
                filter: vec![],
                include: vec!["Name".to_string()],
                world: false,
                entities: None,
            },
        ))
        .unwrap()
        .body;
    let ResponseBody::QueryResult { entities } = body else {
        panic!("expected a query result");
    };
    let listed: Vec<EntityId> = entities.iter().map(|m| m.entity).collect();
    assert_eq!(listed, [sim[0], sim[1], popup[0]]);
}

/// (i) `scene.save` writes sim entities first, then the presentation ones; reloading and
/// re-emitting is byte-identical.
#[test]
fn scene_text_writes_presentation_entities_last_and_round_trips() {
    let engine = engine_with(&[
        named("a", false, ""),
        named("p", true, ""),
        named("b", false, ""),
        named("q", true, ""),
    ]);
    let text = engine.scene_text().unwrap();
    let position = |needle: &str| {
        text.find(needle)
            .unwrap_or_else(|| panic!("{needle}\n{text}"))
    };
    assert!(position("name: a") < position("name: b"));
    assert!(position("name: b") < position("name: p"));
    assert!(position("name: p") < position("name: q"));
    assert_eq!(text.matches("presentation: true").count(), 2);

    let mut reloaded = Engine::new();
    reloaded.load_inline_scene(&text).unwrap();
    assert_eq!(reloaded.scene_text().unwrap(), text);
}

/// (j) A sim `Parent` pointed at a presentation entity cannot be saved.
#[test]
fn saving_a_sim_parent_on_a_presentation_entity_is_l1018() {
    let mut engine = engine_with(&[
        named("a", false, ""),
        named("b", false, ""),
        named("popup", true, ""),
    ]);
    let (b, popup) = (
        engine.world().entities().nth(1).unwrap(),
        presentation_ids(&engine)[0],
    );
    engine
        .execute(Command::ComponentSet(
            ludion_protocol::command::ComponentSetParams {
                entity: b,
                component: "Parent".to_string(),
                value: serde_json::json!({ "index": popup.index(), "generation": popup.generation() }),
            },
        ))
        .unwrap();
    let error = engine.scene_text().unwrap_err();
    assert_eq!(error.code.as_str(), "L1018");
    assert!(
        error
            .message
            .contains(&format!("presentation entity {popup}")),
        "{}",
        error.message
    );
}

/// (k) A presentation entity parented to a sim entity saves and reloads intact.
#[test]
fn a_presentation_entity_parented_to_a_sim_entity_round_trips() {
    let engine = engine_with(&[
        named("a", false, ""),
        named("b", false, ""),
        named("popup", true, &parent_of(1)),
    ]);
    let (b, popup) = (
        engine.world().entities().nth(1).unwrap(),
        presentation_ids(&engine)[0],
    );
    let parent = engine
        .world()
        .get::<ludion_ecs::Parent>(popup)
        .unwrap()
        .expect("popup has a Parent")
        .0;
    assert_eq!(parent, b);

    let text = engine.scene_text().unwrap();
    let mut reloaded = Engine::new();
    reloaded.load_inline_scene(&text).unwrap();
    assert_eq!(reloaded.scene_text().unwrap(), text);
    assert_eq!(presentation_ids(&reloaded), [popup]);
}
