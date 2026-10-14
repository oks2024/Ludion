//! Implementation of `ludion replay <FILE> [--hashes]`.
//!
//! Re-drives a fresh engine through a recorded session — one written by `ludion sim-hash --record`,
//! a live/protocol run, or anything else that used [`Engine::set_recording`](ludion_engine::Engine::set_recording) — through the same
//! dispatch path the original run used ([`ludion_engine::replay_hashes`]), and prints the per-frame
//! `WorldHash` trace it reproduces. Two runs of the same recorded session always print the same
//! trace; comparing a live run's final hash to its own replay is the determinism check (a recorded
//! session replays to an identical `WorldHash` sequence).
//!
//! Recordings this engine makes embed the scene inline, so a replay is self-contained: it reproduces
//! the identical `WorldHash` sequence from any directory or machine, no matter where the scene lived
//! when it was recorded. For the rarer hand-authored replay that names its scene by *path*, that path
//! is resolved against the replay file's **own directory** (not the process cwd), so moving the
//! replay and its scene together as one tree keeps the reference valid.

use std::path::Path;
use std::process::ExitCode;

use ludion_core::codes::code;
use ludion_core::diagnostics::Diagnostic;

/// Runs `ludion replay <path> [--hashes]`.
///
/// On success prints one `<frame> <hash>` line per tick recorded in the trace when `hashes` is set,
/// or just the final line otherwise. On failure prints a rustc-style diagnostic to stderr
/// (`L1015` for an unreadable session file, `L3010`/`L3011` for a malformed one, or whatever the
/// replayed commands themselves raised) and returns a non-zero exit code.
pub fn run(path: &Path, hashes: bool) -> ExitCode {
    let bytes = match std::fs::read(path) {
        Ok(bytes) => bytes,
        Err(error) => {
            let diagnostic = Diagnostic::new(
                code::SCENE_IO,
                format!(
                    "could not read replay session `{}`: {error}",
                    path.display()
                ),
            )
            .with_help("check the path (relative to the current directory) and permissions");
            eprintln!("{diagnostic}");
            return ExitCode::FAILURE;
        }
    };

    // Root the replay's project at the replay file's own directory (resolved absolute, cwd-independent
    // — the same convention `serve`/`sim-hash` use for a scene). Inline-scene replays ignore this;
    // path-scene replays resolve their reference here, so a replay+scene tree stays valid when moved.
    let project_root = std::path::absolute(path)
        .ok()
        .and_then(|abs| abs.parent().map(Path::to_path_buf))
        .unwrap_or_else(|| Path::new(".").to_path_buf());

    let trace = match ludion_engine::replay_hashes(&bytes, &project_root) {
        Ok(trace) => trace,
        Err(diagnostic) => {
            eprintln!("{diagnostic}");
            return ExitCode::FAILURE;
        }
    };

    if hashes {
        for (frame, hash) in &trace {
            println!("{} {hash}", frame.as_u64());
        }
    } else if let Some((frame, hash)) = trace.last() {
        println!("{} {hash}", frame.as_u64());
    }

    ExitCode::SUCCESS
}
