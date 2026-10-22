//! The supersampling quality gates (`Plan/post-process-pipeline.md` wave 3), measured on real
//! gallery scenes with the two instruments the arc built for exactly this: SSIM and `image-stats`.
//!
//! They live here rather than in `ludion-renderer`'s own test module because both instruments are
//! `ludion-testkit`'s, and `ludion-testkit` sits *above* the renderer in the crate graph — the
//! renderer keeps the tests that need only pixel arithmetic. Each needs a real GPU adapter and skips
//! gracefully (printing why) when there is none, matching `gallery.rs`.

use std::path::PathBuf;

use ludion_core::diagnostics::Diagnostic;
use ludion_engine::{
    FrameSize, QualitySetting, RenderOutcome, RenderScale, builtin_registry,
    render_scene_with_registry,
};
use ludion_scene::SceneOverride;
use ludion_testkit::{DecodedImage, ImageStats, image_stats};

fn gallery(scene: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../examples/gallery")
        .join(scene)
}

fn no_adapter(errors: &[Diagnostic]) -> bool {
    ludion_core::require_gpu::is_no_adapter(errors)
}

fn scale(factor: f32) -> RenderScale {
    RenderScale::new(factor).expect("an in-range test scale")
}

/// Renders `scene` at `width` × `height` and the given scale with `overrides` applied in memory, or
/// `None` (after printing why) when the machine has no GPU adapter.
fn render_or_skip(
    test: &str,
    scene: &str,
    width: u32,
    height: u32,
    factor: f32,
    overrides: &[&str],
) -> Option<DecodedImage> {
    let specs: Vec<SceneOverride> = overrides
        .iter()
        .map(|spec| SceneOverride::parse(spec).expect("a well-formed test override"))
        .collect();
    let size = FrameSize::scaled(width, height, scale(factor));
    match render_scene_with_registry(
        &builtin_registry(),
        &gallery(scene),
        size,
        &QualitySetting::full(),
        false,
        &specs,
        0,
        &ludion_engine::DebugCategories::None,
        None,
    ) {
        Ok(RenderOutcome { image, .. }) => Some(DecodedImage {
            width: image.width,
            height: image.height,
            rgba: image.rgba,
        }),
        Err(errors) if no_adapter(&errors) => {
            eprintln!("[{test}] SKIPPED: no GPU adapter available");
            None
        }
        Err(errors) => panic!("[{test}] rendering {scene} at {factor}x failed: {errors:?}"),
    }
}

/// What the `edge_aliasing` gate has to switch off in `post-full.yaml` for its pair to be a
/// supersampling pair and nothing else.
///
/// The scene authors the whole `PostProcess` roster, and three of those effects soften edges on
/// their own: the defocus blurs everything off the focal plane, the chromatic aberration displaces
/// red and blue by a fraction of a pixel, and the grain lays a per-pixel offset over every edge.
/// Measured at the golden's 960x540, they take `edge_aliasing` from 0.566 to 0.214 between them —
/// so left on, the "before" of the comparison is already softer than a supersampled frame and the
/// gate would be measuring them rather than the resolve. Switching them off is not a weakened
/// assertion: it is what makes the two images differ by supersampling alone.
const NO_SOFTENING: [&str; 3] = [
    "/entities/0/components/PostProcess/dof=0.0",
    "/entities/0/components/PostProcess/chromatic_aberration=0.0",
    "/entities/0/components/PostProcess/grain=0.0",
];

/// **The plan's stated invariant: a scale change is a quality change, never a look change.**
///
/// The same scene at the same output size, rendered at 1x and at 2x, must be the same *image* —
/// same framing, same exposure, same bloom radius. SSIM is the right instrument because the two
/// frames are deliberately not byte-identical: the whole point is that the 2x frame's edges carry
/// partial coverage the 1x frame quantized away.
///
/// The residual difference is therefore concentrated entirely on silhouettes and on high-frequency
/// shading (specular highlights on the sphere), which is a few percent of the pixels — hence the
/// threshold below rather than 1.0. Measured on the pinned RTX 3090: **ssim 0.9949, mae 0.209** on
/// meshes.yaml at 480x270. It is a real gate: sizing any bloom target off the render extent instead
/// of the output extent narrows the glow and drops this well under it.
#[test]
fn needs_gpu_a_render_scale_change_is_not_a_look_change() {
    let test = "scale_invariance";
    let (w, h) = (480, 270);
    let Some(native) = render_or_skip(test, "meshes.yaml", w, h, 1.0, &[]) else {
        return;
    };
    let supersampled =
        render_or_skip(test, "meshes.yaml", w, h, 2.0, &[]).expect("adapter still there");

    assert_eq!(
        (supersampled.width, supersampled.height),
        (w, h),
        "supersampling must not change the image the caller gets back",
    );
    let comparison = ludion_testkit::compare(&native, &supersampled);
    eprintln!(
        "[{test}] ssim = {:.4}, mae = {:.3}",
        comparison.ssim, comparison.mae
    );
    assert!(
        comparison.ssim > 0.98,
        "1x and 2x must be the same look: ssim {:.4}",
        comparison.ssim,
    );
}

/// **The defect gate.** `edge_aliasing` — the fraction of edge pixels that are hard steps
/// rather than graded — must fall measurably on a hard-edged scene when the frame is supersampled.
/// This is the number the wave exists to move, and moving it is what proves the resolve is doing
/// real anti-aliasing rather than just costing pixels.
///
/// Both renders switch off [`NO_SOFTENING`], the three authored effects that blur edges without
/// resolving anything, so the pair differs by the render scale and nothing else. Measured on the
/// pinned RTX 3090 at 480x270 on post-full.yaml: **0.8181 -> 0.4891**, so the 0.15 the gate demands
/// is comfortable headroom against a drop of 0.33. (It read 0.8816 -> 0.5843 before the scene
/// authored its roster — the fog and the grade move the "before" a little, the drop not at all.)
#[test]
fn needs_gpu_supersampling_lowers_edge_aliasing() {
    let test = "edge_aliasing";
    let (w, h) = (480, 270);
    let Some(native) = render_or_skip(test, "post-full.yaml", w, h, 1.0, &NO_SOFTENING) else {
        return;
    };
    let supersampled = render_or_skip(test, "post-full.yaml", w, h, 2.0, &NO_SOFTENING)
        .expect("adapter still there");

    let before: ImageStats = image_stats(&native);
    let after: ImageStats = image_stats(&supersampled);
    eprintln!(
        "[{test}] edge_aliasing {:.4} -> {:.4}",
        before.edge_aliasing, after.edge_aliasing,
    );
    assert!(
        after.edge_aliasing < before.edge_aliasing - 0.15,
        "supersampling must measurably soften hard edges: {:.4} -> {:.4}",
        before.edge_aliasing,
        after.edge_aliasing,
    );
}
