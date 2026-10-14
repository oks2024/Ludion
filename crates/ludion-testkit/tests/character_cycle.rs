//! The gallery `character-cycle` scene's CPU proofs: the mannequin's clip cycle under
//! `common::cycle_builder` (idle → run → jump → run → spin → run → idle, crossfaded and leaving the
//! one-shots on their `end` events) traces deterministically, matches the committed cross-arch trace,
//! switches clips in the expected order, and replays from a recording. CPU-only on purpose: the
//! rendered proof is `gallery.rs`'s `character-cycle-driven` golden.

mod common;

use std::path::PathBuf;

use ludion_engine::{Animation, AnimationEvents, Engine};
use ludion_protocol::Command;
use ludion_protocol::command::SimStepParams;

/// One full cycle (about 372 ticks) plus the return to idle and a margin.
const TICKS: usize = 420;

fn gallery_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../examples/gallery")
}

fn scene_path() -> PathBuf {
    gallery_root().join("character-cycle.yaml")
}

fn trace_path() -> PathBuf {
    gallery_root().join("character-cycle.trace.txt")
}

fn replay_path() -> PathBuf {
    gallery_root().join("character-cycle.lrp")
}

/// The scene's engine: the shared game half, rooted at the gallery so the character resolves.
fn build() -> Engine {
    let mut engine = common::cycle_builder()
        .project_root(gallery_root())
        .build()
        .expect("the cycle engine builds");
    engine
        .load_inline_scene(&std::fs::read_to_string(scene_path()).expect("the committed scene"))
        .expect("the committed scene loads");
    engine
}

/// One `<tick:04> <hash:016x>` line per tick.
fn run(n: usize) -> Vec<String> {
    let mut engine = build();
    (0..n)
        .map(|tick| {
            engine.advance_live();
            let hash = engine.world_hash().unwrap().as_u64();
            format!("{tick:04} {hash:016x}")
        })
        .collect()
}

fn committed_trace() -> Vec<String> {
    std::fs::read_to_string(trace_path())
        .expect("the committed reference trace")
        .lines()
        .filter(|line| !line.starts_with('#') && !line.is_empty())
        .map(str::to_owned)
        .collect()
}

#[test]
fn two_in_process_runs_of_the_character_cycle_are_identical() {
    assert_eq!(run(TICKS), run(TICKS));
}

#[test]
fn the_character_cycle_matches_the_committed_cross_arch_trace() {
    let trace = run(TICKS);
    if ludion_core::bless::bless_requested() {
        let header = "\
# Per-tick WorldHash of examples/gallery/character-cycle.yaml under common::cycle_builder\n\
# (crates/ludion-testkit/tests/character_cycle.rs).\n\
# Recorded with: LUDION_BLESS=1 cargo nextest run -p ludion-testkit --test character_cycle\n\
# The M4 must reproduce this file byte-identically - never fabricated by copying the Windows trace.\n";
        std::fs::write(trace_path(), format!("{header}{}\n", trace.join("\n")))
            .expect("the gallery directory is committed");
        return;
    }
    let committed = committed_trace();
    assert_eq!(
        committed.len(),
        TICKS,
        "the committed trace covers every tick"
    );
    for (tick, (actual, expected)) in trace.iter().zip(&committed).enumerate() {
        assert_eq!(actual, expected, "the trace diverges at tick {tick}");
    }
}

/// One clip switch as read off the `Animation` component: the tick it first shows, the new clip, the
/// clip it crossfades from (empty for a hard cut), the fade, and whether the outgoing clip's `end`
/// event fired that tick.
#[derive(Debug, PartialEq)]
struct Switch {
    tick: usize,
    clip: String,
    from: String,
    fade: f32,
    on_end: bool,
}

fn switches() -> Vec<Switch> {
    let mut engine = build();
    let mut log = Vec::new();
    let mut previous = String::from("idle");
    for tick in 0..TICKS {
        engine.advance_live();
        let (entity, (animation,)) = engine
            .world_mut()
            .query_ids::<(&Animation,)>()
            .next()
            .expect("the mannequin plays a clip");
        let (clip, from, fade) = (
            animation.clip.clone(),
            animation.from.clone(),
            animation.fade,
        );
        if clip != previous {
            let on_end = engine
                .world()
                .resource::<AnimationEvents>()
                .is_some_and(|events| events.fired(entity, &previous, "end"));
            log.push(Switch {
                tick,
                clip: clip.clone(),
                from,
                fade,
                on_end,
            });
            previous = clip;
        }
    }
    log
}

/// The readable proof that driver and events work: the clip order, the hard cut into `spin`, the
/// crossfades everywhere else, and both one-shots leaving on their `end` tick.
#[test]
fn the_cycle_switches_clips_in_order_on_the_expected_events() {
    let log = switches();
    let order: Vec<(&str, &str, bool)> = log
        .iter()
        .map(|s| (s.clip.as_str(), s.from.as_str(), s.on_end))
        .collect();
    assert_eq!(
        order,
        [
            ("run", "idle", false),
            ("jump", "run", false),
            ("run", "jump", true),
            ("spin", "", false),
            ("run", "spin", true),
            ("idle", "run", false),
        ],
        "switch log: {log:#?}"
    );
    assert_eq!(log[0].tick as u64, common::IDLE_TICKS);
    assert_eq!(log[0].fade, common::RUN_IN_FADE);
    assert_eq!(log[3].fade, 0.0, "the spin is a hard cut");
}

fn hashes_of(lines: &[String]) -> Vec<&str> {
    lines
        .iter()
        .map(|line| line.split_whitespace().nth(1).expect("a hash column"))
        .collect()
}

fn replayed(bytes: &[u8]) -> Vec<String> {
    let gallery = gallery_root();
    let replayed = ludion_engine::replay_hashes_with(
        common::cycle_builder().project_root(gallery.clone()),
        bytes,
        &gallery,
    )
    .expect("the recording replays");
    replayed[1..]
        .iter()
        .map(|(_, hash)| format!("{:016x}", hash.as_u64()))
        .collect()
}

/// A fresh recording of the session reproduces the committed trace, and so does the committed
/// `character-cycle.lrp`. Recorded at the default stream extent.
#[test]
fn the_committed_character_cycle_replay_reproduces_the_committed_trace() {
    let mut engine = build();
    let fresh_path =
        PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("ludion-testkit-character-cycle.lrp");
    engine
        .set_recording(std::fs::File::create(&fresh_path).expect("the recording is writable"))
        .expect("recording arms");
    let live = ludion_engine::drive_hashes(
        &mut engine,
        vec![Command::SimStep(SimStepParams {
            ticks: TICKS as u64,
            hash: false,
        })],
    )
    .expect("the session runs");
    engine.stop_recording().expect("recording flushes");

    let live_hashes: Vec<String> = live[1..]
        .iter()
        .map(|(_, hash)| format!("{:016x}", hash.as_u64()))
        .collect();
    let fresh_bytes = std::fs::read(&fresh_path).expect("the recording reads back");
    assert_eq!(
        replayed(&fresh_bytes),
        live_hashes,
        "the fresh recording replays to its own trace"
    );
    if ludion_core::bless::bless_requested() {
        std::fs::write(replay_path(), &fresh_bytes).expect("adopting the fresh recording");
        return;
    }

    let committed = committed_trace();
    let committed_hashes = hashes_of(&committed);
    assert_eq!(
        live_hashes, committed_hashes,
        "the recorded session matches the trace"
    );
    let committed_bytes = std::fs::read(replay_path()).expect("the committed character-cycle.lrp");
    assert_eq!(
        replayed(&committed_bytes),
        committed_hashes,
        "the committed character-cycle.lrp no longer replays to the committed trace - after an \
         intentional change, re-record with `LUDION_BLESS=1 cargo nextest run -p ludion-testkit \
         --test character_cycle` and commit it"
    );
}
