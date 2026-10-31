//! Implementation of `ludion dump-graph --scene <FILE> [--width N] [--height N] [--scale F]`.
//!
//! Validates the scene, then prints the *scheduled* frame graph as canonical JSON — passes
//! (name, kind, reads, writes), virtual resources with their descriptors, the derived execution
//! order, and the transient-aliasing assignment. Validation, extraction, graph construction,
//! scheduling, and aliasing are all pure CPU, so this command needs no GPU adapter. The graph is the
//! one the named scene actually renders at the given size and scale (its UI and sun decide the
//! conditional passes; the render scale decides the `resolve`/`upscale` pass; `--quality` can mask
//! an authored effect back out and supply the scale itself), and the output is deterministic and
//! suitable for byte-level snapshotting.

use std::path::Path;
use std::process::ExitCode;

use ludion_engine::{AntiAliasing, FrameSize, QualitySetting, RenderScale};

use crate::diagnostic;

/// Runs `ludion dump-graph`.
///
/// On success prints the graph JSON to stdout and returns success. On failure (an out-of-range
/// `--scale`, an unreadable or invalid scene, or an unschedulable graph) prints rustc-style
/// diagnostics to stderr and returns a non-zero exit code.
///
/// `quality` is this machine's resolved `--quality`, and it is visible in the dump: an effect this
/// machine masks off leaves the graph exactly as it does when the scene never authored it, a
/// lowered shadow map is declared at this machine's own extent, and a machine that draws below
/// native dumps the smaller scene targets and the `upscale` pass that carries them back — so this
/// verb answers "what does *this box* render" and not only "what does this scene ask for". An
/// explicit `scale` beats the one the quality supplies. `anti_aliasing` adds the `smaa_*` passes
/// after `composite` at every preset.
pub fn run(
    scene: &Path,
    width: u32,
    height: u32,
    scale: Option<f32>,
    anti_aliasing: AntiAliasing,
    quality: &QualitySetting,
) -> ExitCode {
    let explicit = match scale.map(RenderScale::new).transpose() {
        Ok(scale) => scale,
        Err(diagnostic) => return diagnostic::report(scene, &[diagnostic]),
    };
    let scale = ludion_engine::resolve_render_scale(explicit, quality);
    match ludion_engine::dump_scene_graph_json(
        scene,
        FrameSize::scaled(width, height, scale),
        quality,
        anti_aliasing,
    ) {
        Ok(json) => {
            // `dump_graph_json` already ends with a newline; use `print!` to avoid a blank line.
            print!("{json}");
            ExitCode::SUCCESS
        }
        Err(diagnostics) => diagnostic::report(scene, &diagnostics),
    }
}
