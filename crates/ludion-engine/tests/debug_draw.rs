//! Debug draw is host render state (`Plan/character-controller.md` decisions 17, 18 and 20): the
//! drawing a system makes is never hashed, snapshotted or recorded, so a session with every category
//! enabled simulates, snapshots and records identically to one with none. The extraction half lives
//! beside the code (`debug_draw.rs`'s tests), where `extract_render_scene` is visible.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use ludion_ecs::{SystemCtx, World};
use ludion_engine::{DebugCategories, Engine, EngineBuilder, debug_draw, replay_hashes_with};
use ludion_protocol::command::{RenderDebugParams, RenderDebugView, SimStepParams};
use ludion_protocol::{Command, ResponseBody};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

const TICKS: u64 = 60;

const LOOP_PATH: &str = "\
format: 1
kind: path
name: ring
interpolation: catmull_rom
closed: true
points:
  - - 0.0
    - 0.0
    - 0.0
  - - 4.0
    - 0.0
    - 0.0
  - - 4.0
    - 0.0
    - 4.0
  - - 0.0
    - 0.0
    - 4.0
";

const SCENE: &str = "\
format: 1
seed: 7
entities:
  - name: probe
    components:
      Probe:
        until: 1000
  - name: platform
    components:
      Transform: {}
      PathFollower:
        path: ring.path.yaml
        speed: 2.0
        mode: loop
";

/// How long the probe keeps drawing, in frames.
#[derive(Serialize, Deserialize, JsonSchema, Clone, Copy, Debug)]
struct Probe {
    until: u64,
}

fn probe(world: &mut World, ctx: &SystemCtx) {
    let until = world
        .query::<(&Probe,)>()
        .next()
        .map_or(0, |(probe,)| probe.until);
    if ctx.frame.as_u64() < until {
        debug_draw(world, "probe").line([0.0; 3], [0.0, 1.0, 0.0], [0.0, 1.0, 0.0, 1.0]);
    }
}

fn fixture_root() -> PathBuf {
    let dir = std::env::temp_dir().join(format!("ludion-debug-draw-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("a temp content root");
    std::fs::write(dir.join("ring.path.yaml"), LOOP_PATH).unwrap();
    dir
}

fn game(root: &Path) -> EngineBuilder {
    Engine::builder()
        .project_root(root)
        .components(|registry| registry.register::<Probe>("Probe"))
        .system("probe", probe)
}

fn engine(root: &Path, categories: &DebugCategories) -> Engine {
    let mut engine = game(root).build().expect("the game builds");
    engine.set_debug_draw(categories.clone());
    engine.load_inline_scene(SCENE).expect("the scene loads");
    engine
}

#[test]
fn drawing_never_perturbs_the_world_hash() {
    let root = fixture_root();
    let run = |categories: &DebugCategories| {
        let mut engine = engine(&root, categories);
        (0..TICKS)
            .map(|_| {
                engine.advance_live();
                engine.world_hash().unwrap().as_u64()
            })
            .collect::<Vec<_>>()
    };
    assert_eq!(run(&DebugCategories::All), run(&DebugCategories::None));
}

#[test]
fn drawing_never_perturbs_a_snapshot() {
    let root = fixture_root();
    let snapshot = |categories: &DebugCategories| {
        let mut engine = engine(&root, categories);
        for _ in 0..TICKS {
            engine.advance_live();
        }
        assert_eq!(engine.debug_draw_drawn(), ["path", "probe"]);
        match engine.execute(Command::EngineSnapshot).unwrap().body {
            ResponseBody::Snapshot { snapshot } => serde_json::to_vec(&snapshot).unwrap(),
            other => panic!("expected a snapshot, got {other:?}"),
        }
    };
    assert_eq!(
        snapshot(&DebugCategories::All),
        snapshot(&DebugCategories::None)
    );
}

#[derive(Clone, Default)]
struct SharedSink(Arc<Mutex<Vec<u8>>>);

impl std::io::Write for SharedSink {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(buf);
        Ok(buf.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

#[test]
fn drawing_never_perturbs_a_recording() {
    let root = fixture_root();
    let record = |categories: &DebugCategories| {
        let sink = SharedSink::default();
        let mut engine = engine(&root, categories);
        engine.set_recording(sink.clone()).unwrap();
        engine
            .execute(Command::SimStep(SimStepParams {
                ticks: TICKS,
                hash: false,
            }))
            .unwrap();
        engine.stop_recording().unwrap();
        let bytes = sink.0.lock().unwrap().clone();
        (bytes, engine.frame(), engine.world_hash().unwrap())
    };
    let (on, frame, hash) = record(&DebugCategories::All);
    let (off, _, _) = record(&DebugCategories::None);
    assert_eq!(on, off, "the recorded bytes do not depend on the filter");
    let replayed = replay_hashes_with(game(&root), &on, &root).expect("the recording replays");
    assert_eq!(replayed.last(), Some(&(frame, hash)));
}

fn toggle(engine: &mut Engine, tick: u64) {
    let categories = if tick.is_multiple_of(2) {
        vec!["all".to_string()]
    } else {
        Vec::new()
    };
    engine
        .execute(Command::RenderDebug(RenderDebugParams {
            categories: Some(categories),
            view: None,
        }))
        .expect("render.debug is always accepted");
}

#[test]
fn toggling_render_debug_never_perturbs_the_hash_or_the_recording() {
    let root = fixture_root();
    let run = |toggling: bool| {
        let sink = SharedSink::default();
        let mut engine = engine(&root, &DebugCategories::None);
        engine.set_recording(sink.clone()).unwrap();
        let hashes = (0..TICKS)
            .map(|tick| {
                if toggling {
                    toggle(&mut engine, tick);
                }
                engine
                    .execute(Command::SimStep(SimStepParams {
                        ticks: 1,
                        hash: false,
                    }))
                    .unwrap();
                engine.world_hash().unwrap().as_u64()
            })
            .collect::<Vec<_>>();
        engine.stop_recording().unwrap();
        let bytes = sink.0.lock().unwrap().clone();
        (hashes, bytes, engine.frame(), engine.world_hash().unwrap())
    };
    let (toggled_hashes, toggled_bytes, frame, hash) = run(true);
    let (plain_hashes, plain_bytes, _, _) = run(false);
    assert_eq!(toggled_hashes, plain_hashes);
    assert_eq!(
        toggled_bytes, plain_bytes,
        "no render.debug record reaches the recording"
    );
    let replayed = replay_hashes_with(game(&root), &toggled_bytes, &root).expect("it replays");
    assert_eq!(replayed.last(), Some(&(frame, hash)));
}

#[test]
fn toggling_the_hierarchy_category_never_perturbs_the_hash() {
    let root = fixture_root();
    let scene = format!(
        "{SCENE}  - name: held\n    components:\n      Parent:\n        index: 1\n        generation: 0\n      Transform: {{}}\n"
    );
    let run = |categories: &DebugCategories| {
        let mut engine = game(&root).build().expect("the game builds");
        engine.set_debug_draw(categories.clone());
        engine.load_inline_scene(&scene).expect("the scene loads");
        (0..TICKS)
            .map(|_| {
                engine.advance_live();
                engine.world_hash().unwrap().as_u64()
            })
            .collect::<Vec<_>>()
    };
    assert_eq!(
        run(&DebugCategories::parse("hierarchy")),
        run(&DebugCategories::None)
    );
}

fn debug_view(
    engine: &mut Engine,
    view: Option<Option<RenderDebugView>>,
) -> Option<RenderDebugView> {
    match engine
        .execute(Command::RenderDebug(RenderDebugParams {
            categories: None,
            view,
        }))
        .expect("render.debug is always accepted")
        .body
    {
        ResponseBody::RenderDebug { view, .. } => view,
        other => panic!("expected RenderDebug, got {other:?}"),
    }
}

#[test]
fn render_debug_sets_reads_and_clears_a_view() {
    let root = fixture_root();
    let mut engine = engine(&root, &DebugCategories::None);
    assert_eq!(debug_view(&mut engine, None), None);
    assert_eq!(
        debug_view(&mut engine, Some(Some(RenderDebugView::SsrConfidence))),
        Some(RenderDebugView::SsrConfidence),
    );
    assert_eq!(
        debug_view(&mut engine, None),
        Some(RenderDebugView::SsrConfidence),
        "an absent view reads what is in force",
    );
    assert_eq!(debug_view(&mut engine, Some(None)), None);
    assert_eq!(debug_view(&mut engine, None), None);
}

#[test]
fn setting_a_view_moves_no_frame_and_no_hash() {
    let root = fixture_root();
    let mut engine = engine(&root, &DebugCategories::None);
    engine.advance_live();
    let (frame, hash) = (engine.frame(), engine.world_hash().unwrap());
    debug_view(&mut engine, Some(Some(RenderDebugView::SsrFootprint)));
    assert_eq!(engine.frame(), frame);
    assert_eq!(engine.world_hash().unwrap(), hash);
}

#[test]
fn render_debug_draws_probes_and_takes_the_probe_index_view() {
    let root = fixture_root();
    let scene = format!(
        "{SCENE}  - name: mirror\n    components:\n      Transform: {{}}\n      ReflectionProbe:\n        shape:\n          sphere:\n            radius: 3.0\n"
    );
    let mut engine = game(&root).build().expect("the game builds");
    engine.load_inline_scene(&scene).expect("the scene loads");
    engine.advance_live();
    let (frame, hash) = (engine.frame(), engine.world_hash().unwrap());
    let reply = engine
        .execute(Command::RenderDebug(RenderDebugParams {
            categories: Some(vec!["probes".to_string()]),
            view: Some(Some(RenderDebugView::ProbeIndex)),
        }))
        .expect("render.debug is always accepted");
    let ResponseBody::RenderDebug {
        enabled,
        drawn,
        view,
    } = reply.body
    else {
        panic!("expected RenderDebug");
    };
    assert_eq!(enabled, ["probes"]);
    assert!(drawn.iter().any(|name| name == "probes"), "{drawn:?}");
    assert_eq!(view, Some(RenderDebugView::ProbeIndex));
    assert_eq!(engine.frame(), frame);
    assert_eq!(engine.world_hash().unwrap(), hash);
}

#[test]
fn render_debug_takes_the_local_shadow_atlas_view() {
    let root = fixture_root();
    let mut engine = engine(&root, &DebugCategories::None);
    assert_eq!(
        debug_view(&mut engine, Some(Some(RenderDebugView::LocalShadowAtlas))),
        Some(RenderDebugView::LocalShadowAtlas),
    );
    assert_eq!(
        debug_view(&mut engine, None),
        Some(RenderDebugView::LocalShadowAtlas)
    );
    assert_eq!(debug_view(&mut engine, Some(None)), None);
}

#[test]
fn render_debug_takes_the_sun_cascades_view() {
    let root = fixture_root();
    let mut engine = engine(&root, &DebugCategories::None);
    assert_eq!(
        debug_view(&mut engine, Some(Some(RenderDebugView::SunCascades))),
        Some(RenderDebugView::SunCascades),
    );
    assert_eq!(
        debug_view(&mut engine, None),
        Some(RenderDebugView::SunCascades)
    );
    assert_eq!(debug_view(&mut engine, Some(None)), None);
}

const PHYSICS_SCENE: &str = "\
format: 1
seed: 7
physics:
  space: free
entities:
  - name: floor
    components:
      Transform: {}
      Collider:
        shape:
          box:
            half_extents:
              - 4.0
              - 0.5
              - 4.0
        body: static
  - name: ball
    components:
      Transform:
        translation:
          - 0.0
          - 3.0
          - 0.0
      Collider:
        shape:
          sphere:
            radius: 0.5
        body:
          rigid:
            mass: 1.0
      Velocity: {}
";

#[test]
fn toggling_the_colliders_category_never_perturbs_the_hash_snapshot_or_recording() {
    let root = fixture_root();
    let run = |categories: &DebugCategories| {
        let sink = SharedSink::default();
        let mut engine = game(&root).build().expect("the game builds");
        engine.set_debug_draw(categories.clone());
        engine
            .load_inline_scene(PHYSICS_SCENE)
            .expect("the scene loads");
        engine.set_recording(sink.clone()).unwrap();
        let hashes = (0..TICKS)
            .map(|_| {
                engine
                    .execute(Command::SimStep(SimStepParams {
                        ticks: 1,
                        hash: false,
                    }))
                    .unwrap();
                engine.world_hash().unwrap().as_u64()
            })
            .collect::<Vec<_>>();
        let snapshot = match engine.execute(Command::EngineSnapshot).unwrap().body {
            ResponseBody::Snapshot { snapshot } => serde_json::to_vec(&snapshot).unwrap(),
            other => panic!("expected a snapshot, got {other:?}"),
        };
        engine.stop_recording().unwrap();
        let recording = sink.0.lock().unwrap().clone();
        (hashes, snapshot, recording)
    };
    let drawn = run(&DebugCategories::parse("colliders"));
    let plain = run(&DebugCategories::None);
    assert_eq!(drawn.0, plain.0, "per-tick hashes");
    assert_eq!(drawn.1, plain.1, "snapshot bytes");
    assert_eq!(drawn.2, plain.2, "recording bytes");
}
