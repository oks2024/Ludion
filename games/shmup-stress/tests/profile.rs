//! Ignored perf-measurement dumps for this crate's four scenes (`Plan/bullet-hell-stress.md`,
//! `Plan/sprite-engine.md` Wave 4 for `steady-drawn`, and `Plan/projectile-patterns.md` Wave 2 for
//! the A-versus-B) — not part of the ordinary lane (each is `#[ignore]`d). Run by hand, in release,
//! from the workspace root:
//!
//! ```text
//! cargo test -p shmup-stress --release --test profile -- --ignored --nocapture
//! ```
//!
//! Each test builds the arena from its scene and drives the exact warmup-then-measured loop
//! `ludion::run`'s `--profile N` path uses (`run_profile` in `crates/ludion/src/lib.rs`):
//! `ludion_engine::warmup_for(FRAMES)` for the warmup count, then
//! `ludion_engine::profile_engine(&mut engine, name, FRAMES, warmup)`, printed with
//! `ludion::profile_report_json` — the same function the CLI and `ludion::run --profile` print
//! through, so this is byte-identical to what `cargo run --release -p shmup-stress -- --scene
//! <scene> --profile 200` would emit.
//!
//! **The four dumps are two pairs.** `steady` (volleys) against `steady-drawn` (one entity per
//! bullet) at the same census is the migration's A-versus-B: `play_patterns` + `advance_volleys`
//! against `emit_bullets` + `move_bullets`, the volley half of `player_hit_test` against its entity
//! scan, and — the claim the whole plan rests on — `extract` evaluating ~3,000 closed forms against
//! `extract` walking ~3,000 entities. `steady-indexed` against `steady-drawn` is the older pair: the
//! price of putting the same field in the spatial index.
//!
//! `burst` additionally reads the report's own raw per-frame samples (`ProfileReport::samples`,
//! filled by the offline driver — no manual `std::time::Instant` needed, and this crate's
//! `clippy.toml` disallows it outright) for its three cancel frames, since the mass-despawn cost
//! shows up in the max, not the median the top-level report already prints.
//!
//! Every scene is loaded **from its file**, never inlined: a scene's own directory is the content
//! root, and both the atlas `steady-drawn` samples and the `kind: pattern` documents `steady` and
//! `burst` play resolve through it. An inlined copy has no root, so its sprites would silently fall
//! back to untextured unit quads and its pattern players would fire nothing.

use std::path::Path;

use ludion::prelude::*;

/// Measured frames per dump.
const FRAMES: u64 = 200;

/// The cancel frames `burst.yaml` wipes the field on, within [`FRAMES`]'s measured window.
const BURST_CANCEL_FRAMES: [u64; 3] = [60, 120, 180];

/// Builds the arena over `scenes/<name>.yaml`, resolved against this crate's own directory (cargo
/// runs an integration test with the package root as its working directory).
fn build(name: &str) -> Engine {
    shmup_stress::game()
        .load_scene_file(Path::new(&format!("scenes/{name}.yaml")))
        .expect("the scene loads against the game registry")
}

/// Profiles `scenes/<name>.yaml` for [`FRAMES`] measured frames (after `ludion_engine::warmup_for`'s
/// warmup) and prints the report as pretty JSON, labelled `name`.
fn dump(name: &str) -> ludion_engine::ProfileReport {
    let mut engine = build(name);
    let warmup = ludion_engine::warmup_for(FRAMES);
    let report = ludion_engine::profile_engine(&mut engine, name.to_string(), FRAMES, warmup)
        .expect("the profile drive succeeds");
    print!("{}", ludion::profile_report_json(&report));
    report
}

#[test]
#[ignore = "measurement, not a gate — run with --release --nocapture"]
fn profile_steady() {
    dump("steady");
}

#[test]
#[ignore = "measurement, not a gate — run with --release --nocapture"]
fn profile_burst() {
    let report = dump("burst");

    let samples = report
        .samples
        .expect("the offline driver fills raw per-frame samples");
    let median = report.sim.frame_total_median_ms;
    println!("\nburst cancel frames vs the {median:.4} ms median frame total:");
    for &cancel_frame in &BURST_CANCEL_FRAMES {
        match samples.frames.iter().position(|&f| f == cancel_frame) {
            Some(i) => println!(
                "  frame {cancel_frame}: {:.4} ms ({:+.4} ms vs median)",
                samples.frame_total_ms[i],
                samples.frame_total_ms[i] - median
            ),
            None => println!("  frame {cancel_frame}: not in the measured window"),
        }
    }
}

#[test]
#[ignore = "measurement, not a gate — run with --release --nocapture"]
fn profile_steady_indexed() {
    dump("steady-indexed");
}

#[test]
#[ignore = "measurement, not a gate — run with --release --nocapture"]
fn profile_steady_drawn() {
    dump("steady-drawn");
}
