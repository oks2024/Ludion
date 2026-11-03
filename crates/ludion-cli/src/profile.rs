//! Implementation of
//! `ludion profile --scene <FILE> --frames N [--json <out>] [--check <budget.toml>] [--scale F]
//! [--perfetto <out>]`, and its live counterpart
//! `ludion profile --attach <ws-url> [--frames N] [--json <out>] [--check <budget.toml>]
//! [--perfetto <out>]`.
//!
//! **Offline** ([`run`]) drives a scene for a warmup then N measured frames and prints one JSON
//! report combining sim per-system timings, GPU per-pass timings, and frame totals -- every number a
//! median over the measured frames (see [`ludion_engine::profile`]). **Attach** ([`run_attach`])
//! instead connects to an already-running host and issues one `profile.capture` over the next N
//! *live* frames -- the gap the offline driver structurally cannot close: it links no game code, so
//! its `sim.systems` list holds only engine-registered systems (the physics triple, for a scene with
//! a `physics:` block), while an attached report carries the running game's own systems. Both paths
//! converge on the same [`finish`] facade: `--json`, `--perfetto`, and `--check` behave identically
//! however the report was produced.
//!
//! A capture also records the render quality it was taken at, so `--check` can refuse to compare a
//! reduced-quality measurement against a budget calibrated on the full one.
//!
//! With `--check`, the report is compared against a TOML budget file: optional `frame_total_ms`,
//! `post_total_ms`, `cluster_build_ms`, `extract_ms`, `load_total_ms`, `gpu_frame_ms` and
//! `gpu_frame_max_ms` scalars, and `[systems]`,
//! `[passes]` and `[post_groups]` tables of millisecond ceilings. `load_total_ms` gates the report's
//! single-run load total and is `L4025` on a report with no load section (an `--attach` capture).
//! Only the offline path measures the cluster build, so a `cluster_build_ms` line is skipped rather
//! than failed on an `--attach` capture; `extract_ms`
//! has no such gap — both the offline driver and an attach capture carry an `extract` sequential
//! entry, so it is checkable either way.
//! Any over-budget line is an `L4005` and the command exits non-zero; an unreadable or
//! invalid budget file is an `L4004`. With `--perfetto <out>`, the report is also serialized to a
//! Chrome trace-event JSON file (openable at ui.perfetto.dev) via [`crate::perfetto::chrome_trace`].
//!
//! Profiling is a wall-clock measurement: run the offline path in `--release` (a debug build's
//! numbers are not comparable to a budget calibrated in release), on the pinned hardware the budget
//! was calibrated on, and at the render scale it was calibrated at -- `--scale` moves the pixel work
//! either way, and so does a `--quality` that supplies its own scale, so those numbers are a
//! different measurement rather than a noisier one. An attach capture
//! measures whatever the attached host is actually running, release or debug -- the same caveat
//! applies to comparing its numbers against a budget calibrated a different way.

use std::path::Path;
use std::process::ExitCode;
use std::time::Duration;

use ludion::{check_budget_report, profile_report_json};
use ludion_client::Client;
use ludion_core::codes::code;
use ludion_core::diagnostics::Diagnostic;
use ludion_engine::{ProfileReport, RenderScale, warmup_for};
use ludion_protocol::perf::{MAX_CAPTURE_WALL_SECONDS, ProfileCaptureParams};
use ludion_protocol::{Command, ResponseBody};

/// Runs `ludion profile --scene <FILE> ...` -- the offline driver.
///
/// `quality` is this machine's resolved `--quality`: it decides which optional effects the measured
/// frames render, and it rides back out in the report so a reader is never left guessing what a GPU
/// number was paid for. With `--check` it is also a gate -- see [`finish`].
#[allow(clippy::too_many_arguments)]
pub fn run(
    scene: &Path,
    frames: u64,
    json_out: Option<&Path>,
    check: Option<&Path>,
    scale: Option<f32>,
    anti_aliasing: Option<ludion_engine::AntiAliasing>,
    perfetto_out: Option<&Path>,
    quality: ludion_engine::QualitySetting,
) -> ExitCode {
    let scale = match scale.map(RenderScale::new).transpose() {
        Ok(scale) => scale,
        Err(diagnostic) => {
            eprintln!("{diagnostic}");
            return ExitCode::FAILURE;
        }
    };
    let report = match ludion_engine::profile_scene(
        scene,
        frames,
        warmup_for(frames),
        scale,
        anti_aliasing,
        quality,
    ) {
        Ok(report) => report,
        Err(diagnostics) => {
            return crate::diagnostic::report(scene, &diagnostics);
        }
    };

    finish(&report, json_out, check, perfetto_out)
}

/// Runs `ludion profile --attach <ws-url> ...` -- the live driver.
///
/// Connects to `url` (an already-running `ludion serve` or any other protocol host), issues one
/// `profile.capture` for `frames` live frames, and hands the resulting report to the same
/// [`finish`] facade [`run`] uses. Samples are requested only when `--perfetto` is set -- they are
/// several times the payload of medians alone (`ludion_protocol::perf::ProfileCaptureParams`'s own
/// docs), and nothing else here needs them.
///
/// # Errors
/// Prints the connection or capture diagnostic (e.g. `L3003` schema drift, `L3009` connect failure,
/// `L3054` call timeout, `L4012` an out-of-range `frames`, `L4013` a capture already in flight,
/// `L4023` a paused or too slow sim whose window cannot close) and exits non-zero.
pub fn run_attach(
    url: &str,
    frames: u64,
    json_out: Option<&Path>,
    check: Option<&Path>,
    perfetto_out: Option<&Path>,
) -> ExitCode {
    let mut client = match Client::connect(url) {
        Ok(client) => client,
        Err(diagnostic) => {
            eprintln!("{diagnostic}");
            return ExitCode::FAILURE;
        }
    };
    // The reply is deferred server-side until the requested window closes (`run_live` holds the
    // reply sender), so the call timeout must cover that whole window, not just a round trip.
    client.set_call_timeout(Some(capture_timeout()));

    let capture = client.call(Command::ProfileCapture(ProfileCaptureParams {
        frames,
        include_samples: perfetto_out.is_some(),
    }));
    let report = match capture {
        Ok(ok) => match ok.body {
            ResponseBody::ProfileReport { report } => *report,
            other => {
                eprintln!(
                    "{}",
                    ludion_client::unexpected_body("profile.capture", "ProfileReport", &other)
                        .with_help("the host and this CLI may be different engine versions")
                );
                return ExitCode::FAILURE;
            }
        },
        Err(diagnostic) => {
            eprintln!("{diagnostic}");
            return ExitCode::FAILURE;
        }
    };

    finish(&report, json_out, check, perfetto_out)
}

/// The call timeout for an attach capture: the longest wall-clock window the host accepts
/// ([`MAX_CAPTURE_WALL_SECONDS`]) plus a flat allowance for connect/handshake overhead and a busy
/// host. An accepted capture closes within that bound at the host's own speed and tick rate, so the
/// timeout fires only on a genuinely wedged sim (`Client::call`'s own `L3054`).
fn capture_timeout() -> Duration {
    Duration::from_secs_f64(MAX_CAPTURE_WALL_SECONDS + 30.0)
}

/// The shared `--json` / `--perfetto` / `--check` output facade for both [`run`] and [`run_attach`]:
/// one report shape, two capture sources, one output pipeline.
///
/// `--check` refuses a measurement taken below full quality rather than comparing it (the refusal
/// lives in [`check_budget_report`], so it covers an attached host's own setting too): a budget is
/// calibrated against everything the scene authors, and ceilings compared against frames that skipped
/// an effect would pass for a reason nobody chose.
fn finish(
    report: &ProfileReport,
    json_out: Option<&Path>,
    check: Option<&Path>,
    perfetto_out: Option<&Path>,
) -> ExitCode {
    // The report -> JSON text and the budget-check flow are single-sourced in the `ludion` facade
    // (shared with `ludion::run --profile`); `--json <out>` writing the text to a file instead of
    // stdout is `ludion profile`'s only addition.
    let json = profile_report_json(report);
    match json_out {
        Some(path) => {
            if let Err(error) = ludion_core::atomic::write_atomic(path, &json) {
                eprintln!(
                    "{}",
                    output_write_failed(&format!(
                        "could not write profile JSON to `{}`: {error}",
                        path.display()
                    ))
                );
                return ExitCode::FAILURE;
            }
            eprintln!(
                "{}: wrote profile ({} frames)",
                path.display(),
                report.measured_frames
            );
        }
        None => print!("{json}"),
    }

    if let Some(path) = perfetto_out {
        match crate::perfetto::chrome_trace(report) {
            Ok(trace) => {
                let text =
                    serde_json::to_string(&trace).expect("a chrome_trace Value serializes to JSON");
                if let Err(error) = ludion_core::atomic::write_atomic(path, &text) {
                    eprintln!(
                        "{}",
                        output_write_failed(&format!(
                            "could not write the Perfetto trace to `{}`: {error}",
                            path.display()
                        ))
                    );
                    return ExitCode::FAILURE;
                }
                eprintln!("{}: wrote Perfetto trace", path.display());
            }
            Err(message) => {
                eprintln!(
                    "{}",
                    Diagnostic::new(code::PROFILE_SAMPLES_MISSING, message)
                );
                return ExitCode::FAILURE;
            }
        }
    }

    if let Some(budget_path) = check {
        return check_budget_report(report, budget_path);
    }
    ExitCode::SUCCESS
}

/// The `L1038` diagnostic for a report file (`--json`, `--perfetto`) that could not be written.
fn output_write_failed(message: &str) -> Diagnostic {
    Diagnostic::new(code::OUTPUT_IO, message).with_help("check the output location is writable")
}
