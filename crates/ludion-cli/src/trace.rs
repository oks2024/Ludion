//! Implementation of `ludion trace --scene <FILE> [--script <FILE>] --frames <N> [--channels …]`.
//!
//! Loads the scene into a fresh built-in-only engine, drives it with the optional input script, and
//! prints [`ludion_engine::trace`]'s records as JSON Lines on stdout. No game systems run at this
//! level, so a motor only reports when a game drives it: the game binary's `--trace` is the same
//! trace with the game's systems running.

use std::io::Write;
use std::path::Path;
use std::process::ExitCode;

use ludion_core::codes::code;
use ludion_core::diagnostics::Diagnostic;
use ludion_engine::TraceChannel;
use ludion_sim::InputScript;

use crate::check_scene;

/// Runs `ludion trace`: JSONL on stdout, a diagnostic on stderr and a non-zero exit on failure.
pub fn run(
    scene_path: &Path,
    script_path: Option<&Path>,
    frames: u64,
    channels: &[TraceChannel],
) -> ExitCode {
    let script = match script_path {
        Some(path) => match ludion_engine::load_input_script(path) {
            Ok(script) => script,
            Err(diagnostic) => {
                eprintln!("{diagnostic}");
                return ExitCode::FAILURE;
            }
        },
        None => InputScript::new(),
    };

    let mut engine = match check_scene::load_engine(scene_path) {
        Ok(engine) => engine,
        Err(exit) => return exit,
    };

    let records = match ludion_engine::trace(&mut engine, &script, frames, channels) {
        Ok(records) => records,
        Err(diagnostic) => {
            eprintln!("{diagnostic}");
            return ExitCode::FAILURE;
        }
    };

    let mut stdout = std::io::stdout().lock();
    match ludion_engine::write_jsonl(&records, &mut stdout).and_then(|()| stdout.flush()) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            let diagnostic = Diagnostic::new(
                code::OUTPUT_IO,
                format!("could not write the trace to stdout: {error}"),
            )
            .with_help("check what stdout is piped or redirected to: a closed pipe or a full disk");
            eprintln!("{diagnostic}");
            ExitCode::FAILURE
        }
    }
}
