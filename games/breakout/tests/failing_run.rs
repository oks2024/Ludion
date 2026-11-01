//! The failing-run recipe (`ludion docs failing-run`), run against the real game: a test that
//! panics leaves a replayable recording; a replay stops at the failing frame and snapshots it for
//! `--restore`; the last frames of a run render with debug lines.

use std::fs;
use std::ops::ControlFlow;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::PathBuf;

use ludion::prelude::*;
use ludion_engine::replay_with;
use ludion_testkit::GameplayDriver;
use serde_json::json;

const MAIN_SCENE: &str = include_str!("../scenes/main.yaml");

/// The frame the failing test dies at.
const FAIL_FRAME: u64 = 240;

fn scenes_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("scenes")
}

/// Pid-suffixed so concurrent runs on the shared target dir never collide.
fn scratch() -> PathBuf {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
        .join(format!("breakout-failing-run-{}", std::process::id()));
    fs::create_dir_all(&dir).expect("the scratch directory is creatable");
    dir
}

fn inject(engine: &mut Engine, kind: &str, key: &str) {
    engine
        .execute(Command::InputInject(command::InputInjectParams {
            frame: None,
            event: json!({"kind": kind, "key": key}),
        }))
        .expect("input injects");
}

fn step(engine: &mut Engine, ticks: u64) {
    engine
        .execute(Command::SimStep(command::SimStepParams {
            ticks,
            hash: false,
        }))
        .expect("the sim steps");
}

#[test]
fn a_failing_test_leaves_a_replay_that_reaches_its_frame() {
    let scratch = scratch();
    let lrp = scratch.join("run.lrp");

    // 1. Arm the recording right after loading; the test dies mid-run without stopping it.
    let failed = catch_unwind(AssertUnwindSafe(|| {
        let mut engine =
            ludion_testkit::loaded_engine(breakout::game().project_root(scenes_dir()), MAIN_SCENE);
        engine
            .set_recording(fs::File::create(&lrp).expect("the recording is writable"))
            .expect("recording arms");
        inject(&mut engine, "key_down", "Right");
        step(&mut engine, 60);
        inject(&mut engine, "key_up", "Right");
        inject(&mut engine, "key_down", "Left");
        step(&mut engine, 60);
        inject(&mut engine, "key_up", "Left");
        step(&mut engine, 120);
        panic!(
            "the ball left the arena at frame {}",
            engine.frame().as_u64()
        );
    }));
    assert!(failed.is_err(), "the scripted test fails");

    // Dropping the engine during the unwind flushed the batched ticks: the file reaches the frame.
    let bytes = fs::read(&lrp).expect("the recording reads back");

    // 3. Replay to the failing frame, then snapshot it.
    let engine = replay_with(breakout::game(), &bytes, scenes_dir(), |_, frame| {
        Ok(if frame.as_u64() == FAIL_FRAME {
            ControlFlow::Break(())
        } else {
            ControlFlow::Continue(())
        })
    })
    .expect("the recording replays");
    assert_eq!(engine.frame().as_u64(), FAIL_FRAME);

    let mut driver = GameplayDriver::from_engine(engine);
    let snapshot = driver.snapshot();
    let snap_path = scratch.join("snap.json");
    fs::write(
        &snap_path,
        serde_json::to_vec(&snapshot).expect("snapshot serializes"),
    )
    .expect("the snapshot is writable");
    eprintln!("snapshot: {}", snap_path.display());

    // The in-process twin of `--restore snap.json`: a fresh engine lands on the same frame and hash.
    let fresh =
        ludion_testkit::loaded_engine(breakout::game().project_root(scenes_dir()), MAIN_SCENE);
    let mut fresh = GameplayDriver::from_engine(fresh);
    fresh.restore(snapshot);
    assert_eq!(fresh.frame(), driver.frame());
    assert_eq!(
        fresh.engine_mut().world_hash().expect("hashes"),
        driver.engine_mut().world_hash().expect("hashes")
    );
}

#[test]
fn needs_gpu_the_last_frames_of_a_replay_render_with_debug_lines() {
    const N: u64 = 300;
    let bytes = fs::read(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("run.lrp"))
        .expect("run.lrp reads back");

    let mut rendered: Vec<u64> = Vec::new();
    let result = replay_with(breakout::game(), &bytes, scenes_dir(), |engine, frame| {
        let frame = frame.as_u64();
        if frame >= N - 120 {
            engine.set_debug_draw(DebugCategories::parse("colliders"));
            let png = engine.render_png(320, 180)?;
            assert!(
                png.starts_with(b"\x89PNG\r\n\x1a\n"),
                "frame {frame} is a PNG"
            );
            rendered.push(frame);
        }
        Ok(if frame >= N {
            ControlFlow::Break(())
        } else {
            ControlFlow::Continue(())
        })
    });

    match result {
        Ok(engine) => {
            assert_eq!(rendered, ((N - 120)..=N).collect::<Vec<_>>());
            assert_eq!(engine.frame().as_u64(), N);
        }
        Err(diagnostic)
            if ludion_core::require_gpu::is_no_adapter(std::slice::from_ref(&diagnostic)) =>
        {
            eprintln!("SKIPPED the debug-line clip test: no GPU adapter (L2003)");
        }
        Err(diagnostic) => panic!("the clip failed: {diagnostic}"),
    }
}
