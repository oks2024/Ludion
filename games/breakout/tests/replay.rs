//! The committed session replay (`run.lrp`) — Milestone 1's "full session replay committed as an
//! example". Two guarantees, re-proven on every run:
//!
//! 1. **Round trip:** a fresh in-process recording of the session replays — through a fresh engine
//!    rebuilt from this game's own `EngineBuilder` — to the live session's per-frame `WorldHash`
//!    trace exactly.
//! 2. **The committed artifact is alive:** `run.lrp` as committed still replays to that same trace,
//!    so the example can never silently rot as the engine or the game moves.
//!
//! Recording uses the same engine API `cargo run -- --record run.lrp` uses (`Engine::set_recording`);
//! in-process recording keeps the test hermetic. After an *intentional* change to the game, the
//! scene, or the engine's hash-visible behavior, re-record the committed session with
//! `LUDION_BLESS=1 cargo test` (the same explicit bless flow as the golden PNG) and commit the new
//! `run.lrp` alongside the new golden.

use std::fs;
use std::path::PathBuf;

use breakout::Brick;
use ludion::prelude::*;
use serde_json::json;

const MAIN_SCENE: &str = include_str!("../scenes/main.yaml");

fn project_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

/// The content root the game runs against, where `fonts.yaml` lives.
fn scenes_dir() -> PathBuf {
    project_root().join("scenes")
}

/// The recorded session: steer the paddle both ways, then let the ball run long enough to bounce
/// off walls and paddle and break bricks (~7.5 seconds of play).
fn session_commands() -> Vec<Command> {
    let inject = |kind: &str, key: &str| {
        Command::InputInject(command::InputInjectParams {
            frame: None,
            event: json!({"kind": kind, "key": key}),
        })
    };
    vec![
        inject("key_down", "Right"),
        Command::SimStep(command::SimStepParams {
            ticks: 60,
            hash: false,
        }),
        inject("key_up", "Right"),
        inject("key_down", "Left"),
        Command::SimStep(command::SimStepParams {
            ticks: 60,
            hash: false,
        }),
        inject("key_up", "Left"),
        Command::SimStep(command::SimStepParams {
            ticks: 330,
            hash: false,
        }),
    ]
}

#[test]
fn recorded_session_replays_to_identical_worldhash_trace() {
    let committed = project_root().join("run.lrp");
    // Crate-prefixed name: CARGO_TARGET_TMPDIR is the workspace-shared `target/tmp`, so a bare
    // `run.lrp` collides with vigil's replay test under a concurrent workspace run (the sokoban
    // precedent).
    let fresh = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("breakout-run.lrp");

    // --- Record: live session with a recording sink armed. ---
    let mut engine =
        ludion_testkit::loaded_engine(breakout::game().project_root(scenes_dir()), MAIN_SCENE);
    engine
        .set_recording(fs::File::create(&fresh).expect("the fresh recording is writable"))
        .expect("recording arms");
    let live = ludion_engine::drive_hashes(&mut engine, session_commands())
        .expect("the live session runs");
    engine.stop_recording().expect("recording flushes");

    // The session was a real game: bricks got broken during it.
    let bricks_left = engine.world_mut().query::<(&Brick,)>().count();
    assert!(
        bricks_left < 15,
        "the recorded session must actually break bricks, {bricks_left} left"
    );

    // --- Round trip: the fresh recording replays to the live trace. ---
    let bytes = fs::read(&fresh).expect("the fresh recording reads back");
    let replayed = ludion_engine::replay_hashes_with(breakout::game(), &bytes, scenes_dir())
        .expect("the fresh recording replays");
    assert_eq!(
        live, replayed,
        "the replayed session must reproduce the live WorldHash trace exactly"
    );

    // --- Bless: `LUDION_BLESS=1` adopts the fresh recording as the committed example. ---
    if ludion_testkit::bless_requested() {
        fs::copy(&fresh, &committed).expect("adopting the fresh recording as run.lrp");
        eprintln!(
            "re-recorded {} — review the diff, then commit it",
            committed.display()
        );
        return;
    }

    // --- The committed artifact is alive: run.lrp still replays to the same trace. ---
    let committed_bytes = fs::read(&committed).expect("the committed run.lrp reads back");
    let committed_replayed =
        ludion_engine::replay_hashes_with(breakout::game(), &committed_bytes, scenes_dir())
            .expect("the committed run.lrp replays");
    assert_eq!(
        live, committed_replayed,
        "the committed run.lrp no longer matches the live session — after an intentional \
         game/scene/engine change, re-record it with `LUDION_BLESS=1 cargo test` and commit it"
    );
}

/// The replay-to-frames capability (G7's clip tooling, the seam behind `cargo run -p breakout --
/// --replay-frames run.lrp --frames-out <dir> --from 0 --to 60`): driving the committed `run.lrp`
/// frame-by-frame through breakout's own builder renders one correctly-named, decodable PNG per sim
/// frame in the requested `[from, to]` window, at the requested resolution.
///
/// This is the in-process twin of the CLI invocation — `ludion::run` parses those flags off the
/// process argv (untestable without a real subprocess), then calls exactly this
/// `ludion_engine::replay_render_frames` with the same window; here we call it directly, the mold
/// `recorded_session_replays_to_identical_worldhash_trace` above uses for `replay_hashes_with`.
///
/// GPU-less machines skip gracefully on the no-adapter signal (`L2003`), exactly as the golden test
/// does — the first frame's render is where the adapter is requested.
#[test]
fn needs_gpu_committed_run_renders_to_a_png_sequence() {
    // 320×180 keeps the 61-frame render quick; the resolution is a plain parameter, so the CLI's
    // 1920×1080 default (in `ludion::run`) exercises the same path at a larger size.
    const WIDTH: u32 = 320;
    const HEIGHT: u32 = 180;
    const TO: u64 = 60;

    let committed_bytes = fs::read(project_root().join("run.lrp")).expect("run.lrp reads back");

    // Crate + pid prefixed: CARGO_TARGET_TMPDIR is the workspace-shared `target/tmp`, which collides
    // across concurrent instances (the recorded gotcha) and with the sibling test's `breakout-run.lrp`.
    let out_dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
        .join(format!("breakout-replay-frames-{}", std::process::id()));
    let _ = fs::remove_dir_all(&out_dir);
    fs::create_dir_all(&out_dir).expect("the frames output directory is creatable");

    let request = ludion_engine::ReplayFramesRequest {
        from: 0,
        to: Some(TO),
        width: WIDTH,
        height: HEIGHT,
        debug: ludion_engine::DebugCategories::None,
    };

    let mut written: Vec<u64> = Vec::new();
    let result = ludion_engine::replay_render_frames(
        breakout::game(),
        &committed_bytes,
        scenes_dir(),
        request,
        |frame, png| {
            let path = out_dir.join(format!("frame-{frame:05}.png"));
            fs::write(&path, png).expect("a frame PNG is writable");
            written.push(frame);
            Ok(())
        },
    );

    match result {
        Ok(outcome) => {
            // Window [0, 60] inclusive → 61 frames, named by absolute sim frame index.
            assert_eq!(
                outcome.rendered, 61,
                "an inclusive [0, 60] window is 61 frames"
            );
            assert_eq!(
                written,
                (0..=TO).collect::<Vec<_>>(),
                "frames arrive in ascending order"
            );

            for frame in 0..=TO {
                let path = out_dir.join(format!("frame-{frame:05}.png"));
                let bytes = fs::read(&path).unwrap_or_else(|e| {
                    panic!("frame {frame} was not written to {}: {e}", path.display())
                });
                // `compare_png_bytes` decodes both buffers, so a self-compare both proves the file is a
                // valid PNG (else `L2204`) and yields its dimensions.
                let report = ludion_testkit::compare_png_bytes(&bytes, &bytes, 0.99)
                    .unwrap_or_else(|d| panic!("frame {frame} is not a decodable PNG: {d}"));
                assert_eq!(
                    (report.actual_width, report.actual_height),
                    (WIDTH, HEIGHT),
                    "frame {frame} rendered at the requested resolution"
                );
            }

            // The window is closed on the right: no frame past `TO` was written.
            assert!(
                !out_dir.join(format!("frame-{:05}.png", TO + 1)).exists(),
                "nothing past the requested `--to` should be rendered"
            );

            let _ = fs::remove_dir_all(&out_dir);
        }
        Err(diagnostic)
            if ludion_core::require_gpu::is_no_adapter(std::slice::from_ref(&diagnostic)) =>
        {
            eprintln!("SKIPPED replay-to-frames test: no GPU adapter (L2003)");
            let _ = fs::remove_dir_all(&out_dir);
        }
        Err(diagnostic) => panic!("replay-to-frames render failed: {diagnostic}"),
    }
}
