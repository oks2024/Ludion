//! A replay reproduces the render extent and the scene text a session was recorded with.
//!
//! `RenderExtent` is sim input (a game unprojects the cursor through it), and a mid-session
//! `scene.load` names a file that may change or vanish before the replay runs. The header carries the
//! extent at arm time, an applied `render.size` is a `Command` record, and an applied `scene.load`
//! is a `SceneLoad` record holding the text it resolved.

use ludion_core::codes::code;
use ludion_ecs::{SystemCtx, World};
use ludion_engine::{Engine, EngineBuilder, RenderExtent, drive_hashes, replay_hashes_with};
use ludion_protocol::command::{RenderSizeParams, SceneLoadParams, SimStepParams};
use ludion_protocol::replay::{
    REPLAY_FORMAT_VERSION, ReplayExtent, ReplayHeader, ReplayReader, ReplayRecord, ReplayWriter,
    SceneRef,
};
use ludion_protocol::{Command, PROTOCOL_VERSION, schema};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// Mirrors the render extent's width into the world each tick, so the extent is visibly sim input.
#[derive(Serialize, Deserialize, JsonSchema, Clone, Debug, PartialEq)]
struct ExtentReadout {
    value: f32,
}

const HUB: &str = "\
format: 1
seed: 5
name: hub
entities:
  - name: player
    components:
      ExtentReadout:
        value: 0.0
";

fn arena(marker_x: f32) -> String {
    format!(
        "\
format: 1
seed: 9
name: arena
entities:
  - name: player
    components:
      ExtentReadout:
        value: 0.0
  - name: marker
    components:
      Transform:
        translation:
          - {marker_x:?}
          - 0.0
          - 0.0
"
    )
}

fn apply_extent(world: &mut World, _ctx: &SystemCtx) {
    let Some(width) = world.resource::<RenderExtent>().map(|extent| extent.width) else {
        return;
    };
    for (readout,) in world.query::<(&mut ExtentReadout,)>() {
        readout.value = width as f32;
    }
}

fn game(root: &Path) -> EngineBuilder {
    Engine::builder()
        .components(|reg| reg.register::<ExtentReadout>("ExtentReadout"))
        .system("apply_extent", apply_extent)
        .project_root(root)
}

fn fresh_dir(name: &str) -> PathBuf {
    let dir =
        std::env::temp_dir().join(format!("ludion-extent-scene-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("arena.yaml"), arena(4.0)).unwrap();
    dir
}

#[derive(Clone, Default)]
struct SharedSink(std::sync::Arc<std::sync::Mutex<Vec<u8>>>);

impl SharedSink {
    fn bytes(&self) -> Vec<u8> {
        self.0.lock().unwrap().clone()
    }
}

impl std::io::Write for SharedSink {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(buf);
        Ok(buf.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

fn step() -> Command {
    Command::SimStep(SimStepParams {
        ticks: 1,
        hash: false,
    })
}

fn resize(width: u32, height: u32) -> Command {
    Command::RenderSize(RenderSizeParams {
        width: Some(width),
        height: Some(height),
    })
}

fn load(path: &str) -> Command {
    Command::SceneLoad(SceneLoadParams { path: path.into() })
}

#[test]
fn a_replay_runs_at_the_extent_it_was_recorded_at() {
    let dir = fresh_dir("extent");
    let mut engine = game(&dir).build().unwrap();
    engine.load_inline_scene(HUB).unwrap();
    engine.execute(resize(960, 540)).unwrap();
    let sink = SharedSink::default();
    engine.set_recording(sink.clone()).unwrap();

    let live = drive_hashes(&mut engine, vec![step(), resize(640, 360), step()]).unwrap();
    engine.stop_recording().unwrap();
    let readout = |engine: &Engine| {
        engine.world().entities().find_map(|id| {
            engine
                .world()
                .get::<ExtentReadout>(id)
                .ok()
                .flatten()
                .map(|readout| readout.value)
        })
    };
    assert_eq!(
        readout(&engine),
        Some(640.0),
        "the system read the resized extent"
    );

    let bytes = sink.bytes();
    let reader = ReplayReader::new(&bytes).unwrap();
    assert_eq!(
        reader.header().render_extent,
        Some(ReplayExtent {
            width: 960,
            height: 540
        })
    );
    let resizes = reader
        .records()
        .unwrap()
        .into_iter()
        .filter(|record| {
            matches!(
                record,
                ReplayRecord::Command {
                    command: Command::RenderSize(_),
                    ..
                }
            )
        })
        .count();
    assert_eq!(resizes, 1, "the one applied resize is recorded");

    // The replay engine boots at the default extent, so only the header can put it at 960.
    let replayed = replay_hashes_with(game(&dir), &bytes, &dir).unwrap();
    assert_eq!(replayed, live);
}

#[test]
fn a_seek_to_a_keyframe_after_a_resize_lands_on_the_resized_extent() {
    let dir = fresh_dir("seek-extent");
    let mut engine = game(&dir).build().unwrap();
    engine.load_inline_scene(HUB).unwrap();
    let sink = SharedSink::default();
    engine.set_recording(sink.clone()).unwrap();
    drive_hashes(&mut engine, vec![step(), resize(640, 360), step()]).unwrap();
    // Stopping writes the closing keyframe, taken after the resize.
    engine.stop_recording().unwrap();

    let bytes = sink.bytes();
    let reader = ReplayReader::new(&bytes).unwrap();
    let seek = reader.seek(engine.frame()).unwrap();
    let (_, snapshot) = seek.keyframe.expect("the closing keyframe");

    // A fresh engine at the default extent: only the keyframe can put it at 640x360.
    let mut fresh = game(&dir).build().unwrap();
    assert_ne!(fresh.render_size(), (640, 360));
    fresh
        .execute(Command::EngineRestore(
            ludion_protocol::command::EngineRestoreParams { snapshot },
        ))
        .unwrap();
    assert_eq!(fresh.render_size(), (640, 360));
    assert_eq!(
        fresh.world().resource::<RenderExtent>().unwrap().size(),
        (640, 360)
    );
}

#[test]
fn a_replayed_scene_load_never_reads_the_file() {
    let dir = fresh_dir("scene-load");
    let mut engine = game(&dir).build().unwrap();
    engine.load_inline_scene(HUB).unwrap();
    let sink = SharedSink::default();
    engine.set_recording(sink.clone()).unwrap();

    let live = drive_hashes(&mut engine, vec![step(), load("arena.yaml"), step()]).unwrap();
    engine.stop_recording().unwrap();

    // The file the load read changes on disk before the replay runs.
    std::fs::write(dir.join("arena.yaml"), arena(9.0)).unwrap();

    let bytes = sink.bytes();
    let records = ReplayReader::new(&bytes).unwrap().records().unwrap();
    let loads: Vec<_> = records
        .iter()
        .filter_map(|record| match record {
            ReplayRecord::SceneLoad { path, document, .. } => Some((path.as_str(), document)),
            _ => None,
        })
        .collect();
    assert_eq!(loads.len(), 1, "one applied load, one SceneLoad record");
    assert_eq!(loads[0].0, "arena.yaml");
    assert!(
        loads[0].1.contains("- 4.0"),
        "the record holds the text the load resolved, not the edited file: {}",
        loads[0].1
    );
    assert!(
        !records.iter().any(|record| matches!(
            record,
            ReplayRecord::Command {
                command: Command::SceneLoad(_),
                ..
            }
        )),
        "the load is not also a plain command record"
    );

    let replayed = replay_hashes_with(game(&dir), &bytes, &dir).unwrap();
    assert_eq!(replayed, live);
}

#[test]
fn a_legacy_command_scene_load_still_replays_from_disk() {
    let dir = fresh_dir("legacy");
    let commands = vec![step(), load("arena.yaml"), step()];
    let mut engine = game(&dir).build().unwrap();
    engine.load_inline_scene(HUB).unwrap();
    let live = drive_hashes(&mut engine, commands.clone()).unwrap();

    let header = ReplayHeader {
        format_version: REPLAY_FORMAT_VERSION,
        engine_version: "0.0.0".into(),
        protocol_version: PROTOCOL_VERSION,
        schema_hash: schema::schema_hash(),
        scene: SceneRef::Inline {
            document: HUB.into(),
        },
        seed: 5,
        profile: None,
        render_extent: None,
        replay_content_hash: None,
    };
    let mut bytes = Vec::new();
    let mut writer = ReplayWriter::new(&mut bytes, &header).unwrap();
    for (frame, command) in [(0, &commands[0]), (1, &commands[1]), (1, &commands[2])] {
        writer
            .write_command(ludion_core::time::FrameIndex::new(frame), command)
            .unwrap();
    }
    writer.into_inner().unwrap();

    let replayed = replay_hashes_with(game(&dir), &bytes, &dir).unwrap();
    assert_eq!(replayed, live);
}

#[test]
fn a_hostile_header_extent_is_refused_by_render_size() {
    let dir = fresh_dir("hostile");
    let header = ReplayHeader {
        format_version: REPLAY_FORMAT_VERSION,
        engine_version: "0.0.0".into(),
        protocol_version: PROTOCOL_VERSION,
        schema_hash: schema::schema_hash(),
        scene: SceneRef::Inline {
            document: HUB.into(),
        },
        seed: 5,
        profile: None,
        render_extent: Some(ReplayExtent {
            width: 0,
            height: 540,
        }),
        replay_content_hash: None,
    };
    let mut bytes = Vec::new();
    ReplayWriter::new(&mut bytes, &header)
        .unwrap()
        .into_inner()
        .unwrap();

    let error = replay_hashes_with(game(&dir), &bytes, &dir).unwrap_err();
    assert_eq!(error.code, code::PROTOCOL_RENDER_SIZE_INVALID);
    assert!(
        error.message.contains("replay header render_extent"),
        "the message names the header: {}",
        error.message
    );
    assert_eq!(error.path.as_deref(), Some("/render_extent/width"));
}

#[test]
fn a_failing_scene_load_record_names_its_frame_and_path() {
    let dir = fresh_dir("bad-load");
    let header = ReplayHeader {
        format_version: REPLAY_FORMAT_VERSION,
        engine_version: "0.0.0".into(),
        protocol_version: PROTOCOL_VERSION,
        schema_hash: schema::schema_hash(),
        scene: SceneRef::Inline {
            document: HUB.into(),
        },
        seed: 5,
        profile: None,
        render_extent: None,
        replay_content_hash: None,
    };
    let mut bytes = Vec::new();
    let mut writer = ReplayWriter::new(&mut bytes, &header).unwrap();
    writer
        .write_scene_load(
            ludion_core::time::FrameIndex::new(1),
            "arena.yaml",
            "format: 1
entities: [this is: not: a scene
",
        )
        .unwrap();
    writer.into_inner().unwrap();

    let error = replay_hashes_with(game(&dir), &bytes, &dir).unwrap_err();
    assert!(
        error.message.contains("frame 1") && error.message.contains("arena.yaml"),
        "the message names the record: {}",
        error.message
    );
}

/// Every `.lrp` committed under `games/` and `examples/`, as `(display path, bytes)`. The fuzz and
/// test reproducers elsewhere are hostile on purpose and are not corpus.
fn committed_replays() -> Vec<(String, Vec<u8>)> {
    fn walk(dir: &Path, display: &str, found: &mut Vec<(String, Vec<u8>)>) {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let shown = format!("{display}/{}", entry.file_name().to_string_lossy());
            if path.is_dir() {
                walk(&path, &shown, found);
            } else if path.extension().is_some_and(|ext| ext == "lrp")
                && let Ok(bytes) = std::fs::read(&path)
            {
                found.push((shown, bytes));
            }
        }
    }
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let mut found = Vec::new();
    for top in ["games", "examples"] {
        walk(&root.join(top), top, &mut found);
    }
    found.sort_by(|a, b| a.0.cmp(&b.0));
    found
}

#[test]
fn committed_replays_are_recorded_at_the_default_extent() {
    let replays = committed_replays();
    assert!(!replays.is_empty(), "the walk found no committed replay");
    let default = RenderExtent::default();
    for (path, bytes) in replays {
        let reader = ReplayReader::new(&bytes).unwrap_or_else(|error| panic!("{path}: {error}"));
        if let Some(extent) = &reader.header().render_extent {
            assert_eq!(
                (extent.width, extent.height),
                (default.width, default.height),
                "{path} was recorded from a resized session; re-record it at the default extent"
            );
        }
    }
}
