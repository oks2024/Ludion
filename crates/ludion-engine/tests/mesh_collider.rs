//! `Collider.shape: { mesh: <path> }` at the engine level: a rigid sphere comes to rest on a static
//! mesh floor read from the content root, `asset.reload` of the document leaves the sim untouched,
//! and a restore beside an edited document rebuilds the floor.

use crossbeam_channel::{bounded, unbounded};
use ludion_core::ids::EntityId;
use ludion_engine::components::Transform;
use ludion_engine::live::{StampedCommand, run_live, shutdown_channel};
use ludion_engine::{Egress, EgressChannels, Engine};
use ludion_physics::{PhysicsWorld, QueryFilter, ShapeKind};
use ludion_protocol::command::{
    AssetReloadParams, EngineRestoreParams, SimStepParams, StreamSubscribeParams, SubscribeMode,
};
use ludion_protocol::event::{Event, LogEvent};
use ludion_protocol::frame::FrameFormat;
use ludion_protocol::{Channel, Command, ResponseBody};
use ludion_scene::MeshDocument;
use serde_json::Value;
use std::path::{Path, PathBuf};
use std::time::Duration;

const MESH: &str = "meshes/floor.mesh.yaml";

fn fresh_dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "ludion-mesh-collider-{}-{name}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("meshes")).unwrap();
    dir
}

/// A two-triangle floor over `x, z ∈ [-5, 5]` at height `y`, wound with the normal up.
fn write_floor(dir: &Path, y: f64) {
    let doc = MeshDocument {
        positions: vec![
            -5.0, y, -5.0, //
            5.0, y, -5.0, //
            -5.0, y, 5.0, //
            5.0, y, 5.0,
        ],
        indices: vec![0, 2, 1, 1, 2, 3],
        ..MeshDocument::default()
    };
    std::fs::write(dir.join(MESH), ludion_scene::emit_mesh(&doc)).unwrap();
}

fn scene() -> String {
    format!(
        "format: 1\nseed: 3\nphysics:\n  space: free\nentities:\n  \
         - name: floor\n    components:\n      Transform: {{}}\n      Collider:\n        shape:\n          mesh:\n            path: {MESH}\n        body: static\n  \
         - name: ball\n    components:\n      Transform:\n        translation:\n          - 0.0\n          - 2.0\n          - 0.0\n      Collider:\n        shape:\n          sphere:\n            radius: 0.5\n        body:\n          rigid:\n            mass: 1.0\n      Velocity: {{}}\n"
    )
}

fn floor_height(engine: &Engine) -> f32 {
    let physics = engine
        .world()
        .resource::<PhysicsWorld>()
        .expect("the scene has physics");
    let hits = physics
        .ray_cast(
            [2.0, 10.0, 2.0],
            [0.0, -1.0, 0.0],
            20.0,
            &QueryFilter::new(),
        )
        .expect("a finite ray");
    assert_eq!(hits.len(), 1, "{hits:?}");
    hits[0].point[1]
}

fn ball_y(engine: &Engine) -> f32 {
    // Document position 1, after the floor.
    engine
        .world()
        .get::<Transform>(EntityId::new(1, 0))
        .expect("a live entity")
        .expect("the ball has a Transform")
        .translation[1]
}

fn booted(name: &str, floor_y: f64) -> (Engine, PathBuf) {
    let dir = fresh_dir(name);
    write_floor(&dir, floor_y);
    let mut engine = Engine::builder()
        .project_root(dir.clone())
        .build()
        .expect("boots");
    engine.load_inline_scene(&scene()).unwrap();
    (engine, dir)
}

#[test]
fn a_rigid_sphere_comes_to_rest_on_a_mesh_floor() {
    let (mut engine, _) = booted("rest", 0.0);
    for _ in 0..240 {
        engine.advance_live();
    }
    let kind = engine
        .world()
        .resource::<PhysicsWorld>()
        .unwrap()
        .entities()
        .next()
        .and_then(|e| {
            engine
                .world()
                .resource::<PhysicsWorld>()
                .unwrap()
                .shape_kind(e)
        });
    assert_eq!(kind, Some(ShapeKind::TriMesh));
    assert!((ball_y(&engine) - 0.5).abs() < 0.05, "{}", ball_y(&engine));
}

#[test]
fn asset_reload_of_the_mesh_changes_nothing_in_the_sim() {
    let (mut engine, dir) = booted("reload", 0.0);
    for _ in 0..120 {
        engine.advance_live();
    }
    write_floor(&dir, 1.0);
    engine
        .execute(Command::AssetReload(AssetReloadParams {
            path: Some(MESH.into()),
        }))
        .expect("the reload is accepted");
    for _ in 0..60 {
        engine.advance_live();
    }
    assert!(floor_height(&engine).abs() < 1e-5);
}

#[test]
fn a_restore_beside_an_edited_mesh_rebuilds_the_floor() {
    let (mut engine, dir) = booted("restore", 0.0);
    for _ in 0..30 {
        engine.advance_live();
    }
    let snapshot: Value = match engine.execute(Command::EngineSnapshot).unwrap().body {
        ResponseBody::Snapshot { snapshot } => snapshot,
        other => panic!("expected a snapshot, got {other:?}"),
    };
    assert!(floor_height(&engine).abs() < 1e-5);

    write_floor(&dir, 1.0);
    match engine
        .execute(Command::EngineRestore(EngineRestoreParams { snapshot }))
        .unwrap()
        .body
    {
        ResponseBody::Restored { .. } => {}
        other => panic!("expected a restore, got {other:?}"),
    }
    for _ in 0..2 {
        engine.advance_live();
    }
    assert!((floor_height(&engine) - 1.0).abs() < 1e-5);
}

/// The `log`-channel lines a live session emits over `ticks` ticks.
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

/// The `L1212` records of a session whose `MESH` document is absent (`text` `None`) or `text`.
fn l1212_records(name: &str, text: Option<&str>) -> Vec<LogEvent> {
    let dir = fresh_dir(name);
    if let Some(text) = text {
        std::fs::write(dir.join(MESH), text).unwrap();
    }
    let mut engine = Engine::builder().project_root(dir).build().expect("boots");
    engine.load_inline_scene(&scene()).unwrap();
    log_lines(engine, 5)
        .into_iter()
        .filter(|line| {
            line.diagnostic
                .as_ref()
                .is_some_and(|d| d.code.as_str() == "L1212")
        })
        .collect()
}

#[test]
fn a_missing_mesh_document_refuses_the_load_and_keeps_the_prior_world() {
    let dir = fresh_dir("refused");
    let mut engine = Engine::builder()
        .project_root(dir.clone())
        .build()
        .expect("boots");
    engine
        .load_inline_scene("format: 1\nseed: 1\nentities:\n  - name: before\n")
        .unwrap();
    let before = engine.world().entity_count();
    let errors = engine
        .load_inline_scene(&scene())
        .expect_err("the collider mesh is missing");
    assert_eq!(errors.len(), 1, "{errors:?}");
    assert_eq!(errors[0].code.as_str(), "L1212");
    assert_eq!(
        errors[0].path.as_deref(),
        Some("/entities/0/components/Collider/shape/mesh/path")
    );
    let related: Vec<&str> = errors[0].related.iter().map(|d| d.code.as_str()).collect();
    assert_eq!(related, ["L1101"], "{errors:?}");
    assert_eq!(engine.world().entity_count(), before);
    engine.advance_live();
}

#[test]
fn a_scene_without_a_physics_block_loads_with_a_missing_collider_mesh() {
    let dir = fresh_dir("no-physics");
    let mut engine = Engine::builder().project_root(dir).build().expect("boots");
    let text = scene().replace("physics:\n  space: free\n", "");
    engine.load_inline_scene(&text).expect("loads as before");
}

#[test]
fn a_mesh_deleted_before_a_restore_is_one_l1212_with_the_read_failure_as_related() {
    let (mut engine, dir) = booted("missing-doc", 0.0);
    let snapshot: Value = match engine.execute(Command::EngineSnapshot).unwrap().body {
        ResponseBody::Snapshot { snapshot } => snapshot,
        other => panic!("expected a snapshot, got {other:?}"),
    };
    std::fs::remove_file(dir.join(MESH)).unwrap();
    match engine
        .execute(Command::EngineRestore(EngineRestoreParams { snapshot }))
        .unwrap()
        .body
    {
        ResponseBody::Restored { .. } => {}
        other => panic!("expected a restore, got {other:?}"),
    }
    let records: Vec<LogEvent> = log_lines(engine, 5)
        .into_iter()
        .filter(|line| {
            line.diagnostic
                .as_ref()
                .is_some_and(|d| d.code.as_str() == "L1212")
        })
        .collect();
    assert_eq!(records.len(), 1, "{records:?}");
    let diagnostic = records[0].diagnostic.as_ref().unwrap();
    assert!(
        !diagnostic.message.contains(": L1"),
        "{}",
        diagnostic.message
    );
    let related: Vec<&str> = diagnostic.related.iter().map(|d| d.code.as_str()).collect();
    assert_eq!(related, ["L1101"], "{diagnostic:?}");
}

#[test]
fn a_malformed_mesh_document_is_one_l1212_carrying_the_validators_diagnostics() {
    let text = "format: 1
kind: [
";
    let oracle = ludion_scene::check_mesh(text).expect_err("malformed");
    let records = l1212_records("bad-doc", Some(text));
    assert_eq!(records.len(), 1, "{records:?}");
    let diagnostic = records[0].diagnostic.as_ref().unwrap();
    assert!(
        !diagnostic.message.contains(": L1"),
        "{}",
        diagnostic.message
    );
    assert_eq!(
        diagnostic.related.first().map(|d| d.code),
        oracle.first().map(|d| d.code)
    );
}

/// A tagged floor: triangle 0 (`x + z < 0`) is `concrete`, triangle 1 is `metal_grate`.
fn tagged_scene_and_floor(name: &str) -> (Engine, PathBuf) {
    let dir = fresh_dir(name);
    let doc = MeshDocument {
        positions: vec![
            -5.0, 0.0, -5.0, //
            5.0, 0.0, -5.0, //
            -5.0, 0.0, 5.0, //
            5.0, 0.0, 5.0,
        ],
        indices: vec![0, 2, 1, 1, 2, 3],
        surfaces: Some(ludion_scene::MeshSurfaces {
            names: vec!["concrete".into(), "metal_grate".into()],
            runs: vec![[1, 0], [1, 1]],
        }),
        ..MeshDocument::default()
    };
    std::fs::write(dir.join(MESH), ludion_scene::emit_mesh(&doc)).unwrap();
    let scene = format!(
        "format: 1\nseed: 3\nphysics:\n  space: free\n  surfaces:\n    - concrete\n    - metal_grate\nentities:\n  \
         - name: floor\n    components:\n      Transform: {{}}\n      Collider:\n        shape:\n          mesh:\n            path: {MESH}\n        body: static\n"
    );
    let mut engine = Engine::builder()
        .project_root(dir.clone())
        .build()
        .expect("boots");
    engine.load_inline_scene(&scene).unwrap();
    (engine, dir)
}

fn surface_names_at(engine: &Engine) -> [Option<String>; 2] {
    let physics = engine
        .world()
        .resource::<PhysicsWorld>()
        .expect("the scene has physics");
    [[-2.0, -2.0], [2.0, 2.0]].map(|[x, z]| {
        let hit = physics
            .ray_cast([x, 10.0, z], [0.0, -1.0, 0.0], 20.0, &QueryFilter::new())
            .expect("a finite ray")
            .remove(0);
        hit.surface
            .and_then(|id| physics.surface_name(id))
            .map(str::to_string)
    })
}

#[test]
fn a_restored_engine_answers_mesh_tags_before_its_first_step() {
    let (mut engine, _) = tagged_scene_and_floor("tags-restore");
    for _ in 0..3 {
        engine.advance_live();
    }
    let want = [
        Some("concrete".to_string()),
        Some("metal_grate".to_string()),
    ];
    assert_eq!(surface_names_at(&engine), want);

    let snapshot: Value = match engine.execute(Command::EngineSnapshot).unwrap().body {
        ResponseBody::Snapshot { snapshot } => snapshot,
        other => panic!("expected a snapshot, got {other:?}"),
    };
    match engine
        .execute(Command::EngineRestore(EngineRestoreParams { snapshot }))
        .unwrap()
        .body
    {
        ResponseBody::Restored { .. } => {}
        other => panic!("expected a restore, got {other:?}"),
    }
    assert_eq!(
        surface_names_at(&engine),
        want,
        "no step has run since the restore"
    );
}

#[test]
fn a_collider_mesh_naming_an_undeclared_surface_is_an_l1697_warning() {
    let (engine, dir) = tagged_scene_and_floor("tags-undeclared");
    drop(engine);
    let source = std::fs::read_to_string(dir.join(MESH))
        .unwrap()
        .replace("metal_grate", "metal_gratte");
    std::fs::write(dir.join(MESH), source).unwrap();
    let scene = ludion_scene::check(
        &format!(
            "format: 1\nseed: 3\nphysics:\n  space: free\n  surfaces:\n    - concrete\n    - metal_grate\nentities:\n  \
             - name: floor\n    components:\n      Collider:\n        shape:\n          mesh:\n            path: {MESH}\n        body: static\n"
        ),
        &ludion_engine::builtin_registry(),
    )
    .unwrap();
    let found = ludion_engine::scene_mesh_asset_diagnostics(&dir, &scene);
    assert_eq!(found.len(), 1, "{found:?}");
    assert_eq!(found[0].code.as_str(), "L1697");
    assert_eq!(
        found[0].path.as_deref(),
        Some("/entities/0/components/Collider/shape/mesh/path")
    );
    assert_eq!(
        found[0].help.as_deref(),
        Some("nearest match is `metal_grate`")
    );
}
