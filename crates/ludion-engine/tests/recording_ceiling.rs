//! A replay frame's size ceiling scales with its compressed size, so a keyframe past the old fixed
//! 8 MiB ceiling records and replays (external-game request 0022); a recording that dies early is loud —
//! `stop_recording` returns `L3042` once.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use ludion_core::time::FrameIndex;
use ludion_ecs::{SystemCtx, World};
use ludion_engine::{Engine, EngineBuilder, replay_hashes_with};
use ludion_protocol::Command;
use ludion_protocol::command::SimStepParams;
use ludion_protocol::replay::{ReplayReader, ReplayRecord};
use ludion_sim::WorldHash;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize, JsonSchema, Clone, Debug, Default)]
struct Blob {
    #[serde(default)]
    data: String,
}

const SCENE: &str = "\
format: 1
seed: 1
entities:
  - name: blob
    components:
      Blob:
        data: ''
";

/// On frame 1, fills the blob with ~9 MiB of low-ratio hex from a fixed-seed LCG: frame 0's
/// keyframe is small, the frame-300 one is past 8 MiB.
fn grow(world: &mut World, ctx: &SystemCtx) {
    if ctx.frame.as_u64() != 1 {
        return;
    }
    let mut state = 0x9e37_79b9_7f4a_7c15_u64;
    let hex: String = (0..9 * 1024 * 1024)
        .map(|_| {
            state = state
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1_442_695_040_888_963_407);
            char::from_digit((state >> 60) as u32, 16).unwrap()
        })
        .collect();
    world
        .query::<(&mut Blob,)>()
        .next()
        .expect("the blob")
        .0
        .data = hex;
}

fn game() -> EngineBuilder {
    EngineBuilder::new()
        .components(|registry| registry.register::<Blob>("Blob"))
        .system("grow", grow)
}

fn engine() -> Engine {
    let mut engine = game().build().expect("the test engine builds");
    engine.load_inline_scene(SCENE).expect("the scene loads");
    engine
}

fn step(ticks: u64) -> Command {
    Command::SimStep(SimStepParams { ticks, hash: false })
}

/// A recording sink shared with the test so the recorded bytes can be read back, and that fails
/// every write once `broken` is set.
#[derive(Clone, Default)]
struct SharedSink {
    bytes: Arc<Mutex<Vec<u8>>>,
    broken: Arc<AtomicBool>,
}

impl SharedSink {
    fn bytes(&self) -> Vec<u8> {
        self.bytes.lock().unwrap().clone()
    }
}

impl std::io::Write for SharedSink {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        if self.broken.load(Ordering::SeqCst) {
            return Err(std::io::Error::other("disk full"));
        }
        self.bytes.lock().unwrap().extend_from_slice(buf);
        Ok(buf.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

#[test]
fn a_keyframe_past_the_old_ceiling_records_and_replays() {
    let sink = SharedSink::default();
    let mut engine = engine();
    engine.set_recording(sink.clone()).unwrap();
    engine.execute(step(601)).unwrap();
    assert!(
        engine.is_recording(),
        "the recording survived the big keyframe"
    );
    engine.stop_recording().expect("a clean stop");

    let bytes = sink.bytes();
    let largest_keyframe = ReplayReader::new(&bytes)
        .unwrap()
        .records()
        .expect("the recording decodes")
        .iter()
        .filter_map(|record| match record {
            ReplayRecord::Keyframe { snapshot, .. } => {
                Some(serde_json::to_vec(snapshot).unwrap().len())
            }
            _ => None,
        })
        .max()
        .expect("the recording holds keyframes");
    assert!(
        largest_keyframe > 8 * 1024 * 1024,
        "a keyframe of {largest_keyframe} bytes is past the old ceiling"
    );

    let replayed = replay_hashes_with(game(), &bytes, std::env::temp_dir())
        .expect("the recording replays without error");
    let live: (FrameIndex, WorldHash) = (engine.frame(), engine.world_hash().unwrap());
    assert_eq!(
        replayed.last(),
        Some(&live),
        "replay ends where the live run did"
    );
}

#[test]
fn stop_recording_reports_a_recording_that_died_early() {
    let sink = SharedSink::default();
    let mut engine = engine();
    engine.set_recording(sink.clone()).unwrap();
    engine.execute(step(10)).unwrap();
    sink.broken.store(true, Ordering::SeqCst);
    engine.execute(step(300)).unwrap();
    assert!(
        !engine.is_recording(),
        "the failed keyframe write ended the recording"
    );

    let failure = engine
        .stop_recording()
        .expect_err("the early death is reported");
    assert_eq!(failure.code.as_str(), "L3042");
    assert!(failure.message.contains("frame"), "{}", failure.message);
    assert!(failure.message.contains("L1038"), "{}", failure.message);
    engine
        .stop_recording()
        .expect("the failure is returned once");

    // A second death, left unreported, is cleared by arming a fresh recording. Arming needs frame 0,
    // so the session starts over with a scene load.
    engine.load_inline_scene(SCENE).expect("the scene reloads");
    let sink = SharedSink::default();
    engine.set_recording(sink.clone()).unwrap();
    sink.broken.store(true, Ordering::SeqCst);
    engine.execute(step(300)).unwrap();
    assert!(!engine.is_recording());
    engine.load_inline_scene(SCENE).expect("the scene reloads");
    engine.set_recording(SharedSink::default()).unwrap();
    engine
        .stop_recording()
        .expect("arming clears a stale failure");
}
