//! The `run_replays` example: a recorded run plays back in place to the file's own trace and verifies;
//! an ignored release test measures the size of a 30-minute recording.

#[path = "../examples/run_replays.rs"]
#[allow(dead_code)]
mod example;

use example::{RUN_NAME, bot_input, game, scene};
use ludion::prelude::*;
use ludion_engine::{replay_hashes_with, verify_replay_keyframes};
use ludion_protocol::replay::ReplayReader;
use std::path::{Path, PathBuf};
use std::time::Instant;

type Trace = Vec<(FrameIndex, WorldHash)>;

fn fresh_root(name: &str) -> PathBuf {
    let root =
        std::env::temp_dir().join(format!("ludion-run-replays-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    root
}

/// Plays the arena with the bot until the director has finished the recording; returns the engine
/// (still live) and the ticks played.
fn record(root: &Path, enemies: usize, length: u64, keyframe_every: u64) -> (Engine, u64) {
    let arena = scene(enemies, length);
    std::fs::write(root.join("arena.yaml"), &arena).unwrap();
    let mut engine = game(root, keyframe_every)
        .persist_profile()
        .build()
        .unwrap();
    engine.load_inline_scene(&arena).unwrap();
    let mut tick = 0;
    loop {
        bot_input(&mut engine, tick);
        engine.advance_live();
        tick += 1;
        let finished = engine
            .world()
            .resource::<RunRecording>()
            .and_then(|recording| recording.last_finished().map(str::to_owned));
        if finished.as_deref() == Some(RUN_NAME) {
            return (engine, tick);
        }
        assert!(tick < length + 1000, "the run never finished");
    }
}

fn recording_bytes(root: &Path) -> Vec<u8> {
    std::fs::read(root.join("save/replays").join(format!("{RUN_NAME}.lrp"))).unwrap()
}

#[test]
fn a_recorded_run_plays_back_and_verifies() {
    let root = fresh_root("playback");
    let (mut engine, _) = record(&root, 50, 240, 60);
    let bytes = recording_bytes(&root);
    let expected: Trace = replay_hashes_with(game(&root, 60), &bytes, &root).unwrap();

    engine
        .world_mut()
        .resource_mut::<ReplayPlayback>()
        .unwrap()
        .refresh();
    engine.advance_live();
    let entries = engine
        .world()
        .resource::<ReplayPlayback>()
        .unwrap()
        .entries()
        .to_vec();
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].name, RUN_NAME);
    assert!(entries[0].refusal.is_none(), "{:?}", entries[0].refusal);

    engine
        .world_mut()
        .resource_mut::<ReplayPlayback>()
        .unwrap()
        .play(RUN_NAME, "arena.yaml");
    engine.advance_live();
    let mut played: Trace = vec![(engine.frame(), engine.world_hash().unwrap())];
    while engine.advance_live() {
        played.push((engine.frame(), engine.world_hash().unwrap()));
    }
    assert_eq!(played, expected);

    let outcome = engine
        .world()
        .resource::<ReplayPlayback>()
        .unwrap()
        .last_outcome()
        .cloned();
    assert!(
        matches!(outcome, Some(PlaybackOutcome::Finished { .. })),
        "{outcome:?}"
    );
    assert_eq!(engine.frame(), FrameIndex::ZERO, "back at a fresh arena");

    let report = verify_replay_keyframes(game(&root, 60), &bytes, &root).unwrap();
    assert!(report.divergence.is_none(), "{:?}", report.divergence);
    assert!(report.keyframes > 0 && report.verified == report.keyframes);
    let _ = std::fs::remove_dir_all(&root);
}

/// The size of a 30-minute run: 108 000 ticks at 60 Hz, 500 rigid-body enemies, the projectile pool
/// and the scripted bot, at the default `keyframe_every`. Prints the figures the `replays` topic cites.
///
/// ```text
/// cargo test --release -p ludion --test run_replays -- --ignored --nocapture thirty_minute_500_enemy_recording_size
/// ```
#[test]
#[allow(clippy::disallowed_methods)] // the figure includes the wall time of the record pass
#[ignore = "a release-profile measurement, minutes long"]
fn thirty_minute_500_enemy_recording_size() {
    const TICKS: u64 = 108_000;
    let root = fresh_root("size");
    let default_every = RunRecordingConfig::default().keyframe_every;
    let started = Instant::now();
    let (_, played) = record(&root, 500, TICKS, default_every);
    let wall = started.elapsed();

    let bytes = recording_bytes(&root);
    let records = ReplayReader::new(&bytes).unwrap().records().unwrap();
    let keyframes = records.iter().filter(|r| r.is_keyframe()).count();
    let total = bytes.len();

    // The same run with an interval past its end keeps only the opening keyframe, which isolates
    // what the inputs and commands cost; the rest of the default file is the other keyframes.
    let bare_root = fresh_root("size-bare");
    record(&bare_root, 500, TICKS, TICKS + 1000);
    let bare = recording_bytes(&bare_root);
    let bare_keyframes = ReplayReader::new(&bare)
        .unwrap()
        .records()
        .unwrap()
        .iter()
        .filter(|r| r.is_keyframe())
        .count();
    let extra = total.saturating_sub(bare.len());
    let extra_keyframes = keyframes.saturating_sub(bare_keyframes).max(1);

    println!("ticks played (incl. title): {played}");
    println!(
        "file bytes: {total} ({:.2} MiB)",
        total as f64 / (1024.0 * 1024.0)
    );
    println!("keyframes: {keyframes} (every {default_every} ticks)");
    println!(
        "file bytes with {bare_keyframes} keyframe(s): {}",
        bare.len()
    );
    println!(
        "mean added keyframe size: {} bytes",
        extra / extra_keyframes
    );
    println!("record pass wall time: {:.1} s", wall.as_secs_f64());
    let _ = std::fs::remove_dir_all(&bare_root);

    let report = verify_replay_keyframes(game(&root, default_every), &bytes, &root).unwrap();
    assert!(report.divergence.is_none(), "{:?}", report.divergence);
    let _ = std::fs::remove_dir_all(&root);
}
