//! Implementation of
//! `ludion serve --scene <FILE> [--addr <ADDR>] [--record <FILE>] [--paused] [--window]`.
//!
//! Hosts a scene live over the wire protocol (`ludion-server`) so a human's viewer or an agent's
//! protocol client can attach — the running example of the wire protocol
//! (`implementation-guide.md` phases 0-9). Loading and validating the scene reuses the exact
//! construction path `sim-hash` drives (`check_scene::load_engine`), so a broken scene fails
//! identically here and there. `--record` arms recording (`check_scene::arm_recording`, matching
//! `sim-hash --record`'s pattern) *before* the engine moves onto the sim thread — recording stays a
//! host-side switch, there is no protocol command for it. `--paused` starts the engine paused: a
//! deterministic attach point, since a paused engine still answers queries and can be driven with
//! explicit `sim.step` over the wire (`ludion-server`'s "pause-before-serve" pattern). `--window`
//! opens the scene's own native window through the facade's windowed shell: the engine takes the
//! window's surface before it binds and presents its offscreen frame in-process, while the listener
//! still binds for peers (`ludion devdocs ludion`). The `ludion-window` viewer is `dev --window`'s.

use std::path::Path;
use std::process::ExitCode;

use ludion::{HostOptions, host_scene};

use crate::check_scene;

/// Runs `ludion serve`.
///
/// On success, prints exactly one machine-parseable line first — `listening on ws://<addr>`,
/// flushed immediately — so a test or agent can read the bound address (relevant when `--addr`
/// asks for an OS-chosen port) without racing the rest of startup. A short human-readable block
/// (engine version, schema hash, scene path, recording status, and the stop instruction) follows.
///
/// Without `--window`, the process then blocks until Ctrl-C or a wire `engine.shutdown` command, at
/// which point it shuts the server down cleanly — closing connections, stopping the sim thread, and
/// flushing any armed recording — and returns success. Both paths run the one graceful teardown.
///
/// With `--window`, it instead opens the scene's own native window on **this thread** (a winit
/// requirement — `run` must therefore be called from `main`, not a spawned thread), presenting the
/// engine's offscreen frame in-process, and blocks there; closing the window or Ctrl-C both end it
/// the same way. If the window cannot be opened at all (headless session, no GPU), the server is
/// still shut down cleanly before the diagnostic is printed and a non-zero code returned — the user
/// asked for a window, so falling back to silent headless serving would be surprising.
///
/// On failure — a bad scene, an unresolvable `--addr`, a failed bind, or a failed `--record`
/// sink — prints a diagnostic to stderr and returns a non-zero exit code; nothing is printed on
/// stdout.
#[allow(clippy::too_many_arguments)]
pub fn run(
    scene_path: &Path,
    addr: &str,
    record_path: Option<&Path>,
    paused: bool,
    window: bool,
    dev_shaders: bool,
    scale: Option<f32>,
    quality: ludion_engine::QualitySetting,
    render_size: Option<(u32, u32)>,
) -> ExitCode {
    let scale = match scale.map(ludion_engine::RenderScale::new).transpose() {
        Ok(scale) => scale,
        Err(diagnostic) => {
            eprintln!("{diagnostic}");
            return ExitCode::FAILURE;
        }
    };
    let mut engine = match check_scene::load_engine(scene_path) {
        Ok(engine) => engine,
        Err(exit) => return exit,
    };
    // Host quality settings, applied before the engine moves onto the sim thread — like
    // `--dev-shaders`, and for the same reason: they are the host's GPU budget, not the world's. An
    // attached client reads both through `engine.describe` and negotiates neither. `--scale` is set
    // only when it was given, so an absent flag leaves the extent to the quality rather than
    // overriding it with a native nobody asked for.
    if let Some(scale) = scale {
        engine.set_render_scale(scale);
    }
    engine.set_quality(quality);
    // The extent is the third of them, and the one that moves bytes rather than GPU work: it sizes
    // every streamed frame's readback, encode and socket write, which `--scale` deliberately does
    // not (it renders at `render_size × scale` and resamples back). Set only when a host named one,
    // so an absent flag leaves the engine's own default rather than restating it here.
    if let Some((width, height)) = render_size {
        engine.set_render_size(width, height);
    }
    // The scale and the extent meet here; a pair no render could honor refuses the start rather than
    // failing on the first frame.
    if let Err(diagnostic) = engine.check_render_extent() {
        eprintln!("{diagnostic}");
        return ExitCode::FAILURE;
    }

    if let Some(record_path) = record_path
        && let Err(exit) = check_scene::arm_recording(&mut engine, record_path)
    {
        return exit;
    }

    // Dev-only shader hot reload: an `asset.reload` on a renderer `.wgsl` recreates its pipeline live
    // (`Plan/shader-bindgen.md`). Enabled before the engine moves onto the sim thread.
    if dev_shaders {
        engine.enable_shader_reload();
    }

    // The engine is built and recording (if any) is armed; the rest of the host lifecycle (audio,
    // `--paused`, bind, header, watch, `--window`, graceful teardown) is shared with `ludion::run`.
    // `serve` prints the `schema hash:` header line — it is the attach point external clients read.
    host_scene(
        engine,
        addr,
        HostOptions {
            scene: scene_path,
            record_path,
            paused,
            window,
            print_schema_hash: true,
            window_flag: true,
        },
    )
}
