//! Implementation of `ludion sim-hash --scene <FILE> [--script <FILE>] [--frames <N>]
//! [--record <FILE>]`.
//!
//! The cross-process half of the determinism harness (`engine-architecture.md` §5): load the scene
//! into a fresh [`Engine`](ludion_engine::Engine) (check → instantiate → replace world, seeded from
//! the scene's `seed` — `0` when absent), inject any `--script` events, run `--frames` fixed ticks,
//! and print the final
//! [`WorldHash`](ludion_sim::WorldHash) (16 lowercase hex digits) on stdout. Two invocations with the
//! same inputs must print the same hash; CI runs exactly that comparison, which catches ASLR-,
//! environment-, and process-state-dependent nondeterminism that an in-process double run cannot.
//!
//! Driving the run through [`Engine::execute`](ludion_engine::Engine::execute) (rather than talking
//! to [`ludion_sim::SimDriver`] directly, as earlier phases did) is what makes `--record <FILE>` a
//! one-line addition: `check_scene::arm_recording` arms recording right after the scene loads, and
//! every subsequent command — each injected input, then the `sim.step` — is written to the session
//! file exactly as a live/protocol run would record it. `ludion replay` re-drives that file through
//! the same dispatch path and must reproduce the identical `WorldHash` sequence. `ludion serve
//! --record` arms recording the same way, through the same `check_scene::load_engine` /
//! `check_scene::arm_recording` helpers this module uses.
//!
//! **No game systems run at this level.** The CLI has only the built-in registry — game systems
//! and game components live in the game binary, which reaches the sim through the engine (`ludion dev`,
//! the protocol's `sim.hash`). The command still proves the determinism claim end to end: scene → world instantiation, N ticks of the fixed-timestep driver, and the
//! canonical hash are all deterministic across processes. The same pipeline, not a demo stand-in,
//! gains systems when the engine arrives.

use std::path::Path;
use std::process::ExitCode;

use ludion_protocol::Command;
use ludion_protocol::command::{InputInjectParams, SimStepParams};
use ludion_sim::InputScript;

use crate::check_scene;

/// Runs `ludion sim-hash`.
///
/// On success prints exactly one line — the final `WorldHash` — on stdout. On failure prints
/// rustc-style diagnostics to stderr and returns a non-zero exit code. When `record_path` is given,
/// arms recording right after the scene loads, so the written session's header captures this run's
/// scene reference and seed, and every input/step that follows becomes a replayable delta.
pub fn run(
    scene_path: &Path,
    script_path: Option<&Path>,
    frames: u64,
    record_path: Option<&Path>,
) -> ExitCode {
    let input = match script_path {
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

    if let Some(record_path) = record_path
        && let Err(exit) = check_scene::arm_recording(&mut engine, record_path)
    {
        return exit;
    }

    for scripted in input.events() {
        let event = serde_json::to_value(scripted.event).expect("InputEvent always serializes");
        let injected = engine.execute(Command::InputInject(InputInjectParams {
            frame: Some(scripted.frame),
            event,
        }));
        if let Err(diagnostic) = injected {
            eprintln!("{diagnostic}");
            return ExitCode::FAILURE;
        }
    }

    let stepped = engine.execute(Command::SimStep(SimStepParams {
        ticks: frames,
        hash: false,
    }));
    if let Err(diagnostic) = stepped {
        eprintln!("{diagnostic}");
        return ExitCode::FAILURE;
    }

    // Idempotent no-op when `--record` was not given.
    if let Err(diagnostic) = engine.stop_recording() {
        eprintln!("{diagnostic}");
        return ExitCode::FAILURE;
    }

    match engine.world_hash() {
        Ok(hash) => {
            println!("{hash}");
            ExitCode::SUCCESS
        }
        Err(diagnostic) => {
            eprintln!("{diagnostic}");
            ExitCode::FAILURE
        }
    }
}
