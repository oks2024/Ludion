//! The 1/2/8-thread hash-identity gate (`Plan/deterministic-job-system.md` J0): the named replay
//! corpus — `run.lrp` here, `games/vigil/run.lrp` and `games/vigil/stress.lrp` the other two members
//! — must replay to bit-identical `WorldHash` sequences no matter how many worker threads
//! `ludion_core::par` is told to use. Breakout's own systems are sequential, so this gate
//! guards them and the shared engine `par` paths.
//!
//! `#[ignore]`d: driving `run.lrp` three times (once per thread count) is cheap for this small game,
//! but the gate lives in the same `--full`/CI lane as vigil's heavier corpus members rather than the
//! default lane, so it stays consistent with them. Run via
//! `cargo nextest run -E 'binary(thread_identity)' --run-ignored ignored-only` (what
//! `check.mjs --full` and CI do), or `cargo test -p breakout --test thread_identity -- --ignored`.

use std::fs;
use std::path::{Path, PathBuf};

use ludion::prelude::*;
use ludion_core::par;
use ludion_engine::{RunConfig, bisect_divergence, first_divergent_frame, verify_replay_keyframes};

/// Thread counts the gate requires bit-identical `WorldHash` traces at, in the order the three
/// concurrent drives below are named and destructured.
const THREAD_COUNTS: [usize; 3] = [1, 2, 8];

fn project_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

/// Drives `bytes` at 1, 2, and 8 threads and asserts the three `WorldHash` traces are element-wise
/// identical.
///
/// Each `par::with_threads` override is installed **inside its own spawned thread's closure** —
/// the override is thread-local, so setting it on the calling thread before spawning would be a
/// no-op for the thread that actually runs the replay (`ludion_core::par`'s own doc). On a mismatch,
/// this localizes the divergence with `first_divergent_frame` + `bisect_divergence` so the panic
/// names the frame, the first differing system, and the differing components — not just "traces
/// differ".
fn assert_hash_identity_across_thread_counts(
    corpus_name: &str,
    bytes: &[u8],
    root: &Path,
    build: impl Fn() -> EngineBuilder + Sync,
) {
    let build = &build;
    // Test orchestration, not sim work: three whole engines driven side by side, each drive fully
    // sequential except for the thread-count override under test.
    #[allow(clippy::disallowed_methods)]
    let [one, two, eight] = std::thread::scope(|scope| {
        THREAD_COUNTS
            .map(|n| {
                scope.spawn(move || {
                    par::with_threads(n, || {
                        ludion_engine::replay_hashes_with(build(), bytes, root)
                    })
                })
            })
            .map(|handle| {
                handle
                    .join()
                    .unwrap_or_else(|payload| std::panic::resume_unwind(payload))
            })
    });

    let one = one.unwrap_or_else(|d| panic!("{corpus_name}: 1-thread replay failed: {d}"));
    let two = two.unwrap_or_else(|d| panic!("{corpus_name}: 2-thread replay failed: {d}"));
    let eight = eight.unwrap_or_else(|d| panic!("{corpus_name}: 8-thread replay failed: {d}"));

    // A pairwise check against the 1-thread trace covers the whole set: if 2 and 8 both match 1 they
    // match each other, and thread count must never move the result regardless of which pair is
    // compared.
    if one != two {
        let frame = first_divergent_frame(&one, &two);
        report_divergence(corpus_name, bytes, root, build, 1, 2, frame);
    }
    if one != eight {
        let frame = first_divergent_frame(&one, &eight);
        report_divergence(corpus_name, bytes, root, build, 1, 8, frame);
    }
}

/// Bisects an already-detected divergence between `left_n` and `right_n` threads and panics with the
/// localized report: the frame, the first differing system, and the component diff there.
fn report_divergence(
    corpus_name: &str,
    bytes: &[u8],
    root: &Path,
    build: &(impl Fn() -> EngineBuilder + Sync),
    left_n: usize,
    right_n: usize,
    frame: Option<FrameIndex>,
) {
    let report = bisect_divergence(
        RunConfig::new(format!("{left_n} thread(s)"), build)
            .scoped(move |body| par::with_threads(left_n, body)),
        RunConfig::new(format!("{right_n} thread(s)"), build)
            .scoped(move |body| par::with_threads(right_n, body)),
        bytes,
        root,
    )
    .unwrap_or_else(|d| {
        panic!("{corpus_name}: bisecting the {left_n}-vs-{right_n}-thread divergence failed: {d}")
    });

    match report {
        Some(report) => panic!(
            "{corpus_name}: {left_n}-thread and {right_n}-thread replays diverge at frame {} \
             (hash traces first differ there):\n{report}",
            frame.map_or_else(|| "?".to_string(), |f| f.as_u64().to_string()),
        ),
        None => panic!(
            "{corpus_name}: {left_n}-thread and {right_n}-thread hash traces differ, but \
             bisect_divergence found no divergent frame in the common prefix — the traces likely \
             differ in length"
        ),
    }
}

#[test]
#[ignore = "thread-identity gate; run via check.mjs --full or CI"]
fn run_lrp_hashes_are_thread_count_invariant() {
    let bytes = fs::read(project_root().join("run.lrp")).expect("run.lrp reads back");
    assert_hash_identity_across_thread_counts(
        "games/breakout/run.lrp",
        &bytes,
        &project_root(),
        breakout::game,
    );
}

/// The keyframe-churn half of the gate (`Plan/deterministic-job-system.md` J0): today's single-
/// threaded code must still reproduce every keyframe the committed replay embeds. Lives alongside the
/// thread-identity test because both ride the same `--full`/CI lane and watch the same corpus.
#[test]
#[ignore = "thread-identity gate; run via check.mjs --full or CI"]
fn run_lrp_reproduces_its_recorded_keyframes() {
    let bytes = fs::read(project_root().join("run.lrp")).expect("run.lrp reads back");
    let report = verify_replay_keyframes(breakout::game(), &bytes, project_root())
        .expect("run.lrp verifies");
    assert!(report.is_clean(), "run.lrp: {report}");
    assert!(
        report.keyframes > 0,
        "run.lrp embeds no keyframes, so the check passed vacuously: {report}"
    );
}
