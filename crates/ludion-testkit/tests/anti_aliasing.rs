//! The SMAA quality gates (`Clockout request 0009`), measured on real gallery scenes with the
//! instruments the supersampling arc built: SSIM and `image-stats`.
//!
//! They live here for the same reason `supersampling.rs` does: both instruments are
//! `ludion-testkit`'s, which sits above the renderer in the crate graph. Each needs a real GPU adapter
//! and skips gracefully (printing why) when there is none.

use std::path::PathBuf;

use ludion_core::diagnostics::Diagnostic;
use ludion_engine::{
    AntiAliasing, FrameSize, QualitySetting, RenderOutcome, RenderScale, builtin_registry,
    render_scene_with_registry,
};
use ludion_scene::SceneOverride;
use ludion_testkit::{DecodedImage, compare, image_stats, masked_ssim};

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

/// Renders `scene` at `width` × `height`, the given scale and anti-aliasing mode, with `overrides`
/// applied in memory, or `None` (after printing why) when the machine has no GPU adapter.
fn render_or_skip(
    test: &str,
    scene: &str,
    width: u32,
    height: u32,
    factor: f32,
    overrides: &[&str],
    aa: AntiAliasing,
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
        aa,
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
        Err(errors) => {
            panic!("[{test}] rendering {scene} at {factor}x ({aa:?}) failed: {errors:?}")
        }
    }
}

/// What the gates on `post-full.yaml` switch off so a pair differs by anti-aliasing and nothing
/// else.
///
/// The scene authors the whole `PostProcess` roster, and three of those effects soften edges on
/// their own: the defocus blurs everything off the focal plane, the chromatic aberration displaces
/// red and blue by a fraction of a pixel, and the grain lays a per-pixel offset over every edge.
/// Left on, the "before" of a comparison is already softer than a resolved frame and the gate would
/// be measuring them rather than the resolve.
const NO_SOFTENING: [&str; 3] = [
    "/entities/0/components/PostProcess/dof=0.0",
    "/entities/0/components/PostProcess/chromatic_aberration=0.0",
    "/entities/0/components/PostProcess/grain=0.0",
];

/// Rec. 709 luma of the RGB at pixel `index`, in 8-bit levels.
fn luma(image: &DecodedImage, index: usize) -> f64 {
    let px = &image.rgba[index * 4..][..3];
    0.2126 * f64::from(px[0]) + 0.7152 * f64::from(px[1]) + 0.0722 * f64::from(px[2])
}

/// Rec. 709 luma standard deviation over `(x, y, width, height)` of `image`, in 8-bit levels — the
/// spread of a patch, in the same encoded domain `image_stats` scores in.
fn patch_std(image: &DecodedImage, (x0, y0, w, h): (u32, u32, u32, u32)) -> f64 {
    let luma: Vec<f64> = (y0..y0 + h)
        .flat_map(|y| (x0..x0 + w).map(move |x| (x, y)))
        .map(|(x, y)| luma(image, (y * image.width + x) as usize))
        .collect();
    let mean = luma.iter().sum::<f64>() / luma.len() as f64;
    (luma.iter().map(|v| (v - mean).powi(2)).sum::<f64>() / luma.len() as f64).sqrt()
}

/// The pixels whose neighbourhood holds no edge: a pixel is `true` when the luma step to each of its
/// 4-neighbours is under 26 levels (SMAA's own 0.1 threshold in the encoded domain) and the same
/// holds for every pixel within radius 2.
fn edge_free_mask(image: &DecodedImage) -> Vec<bool> {
    let (w, h) = (image.width as usize, image.height as usize);
    let flat: Vec<bool> = (0..w * h)
        .map(|i| {
            let (x, y) = (i % w, i / w);
            let l = luma(image, i);
            let step = |nx: Option<usize>, ny: Option<usize>| match (nx, ny) {
                (Some(nx), Some(ny)) if nx < w && ny < h => {
                    (luma(image, ny * w + nx) - l).abs() < 26.0
                }
                _ => true,
            };
            step(x.checked_sub(1), Some(y))
                && step(Some(x + 1), Some(y))
                && step(Some(x), y.checked_sub(1))
                && step(Some(x), Some(y + 1))
        })
        .collect();
    (0..w * h)
        .map(|i| {
            let (x, y) = ((i % w) as i64, (i / w) as i64);
            (-2i64..=2).all(|dy| {
                (-2i64..=2).all(|dx| {
                    let (nx, ny) = (x + dx, y + dy);
                    nx < 0 || ny < 0 || nx >= w as i64 || ny >= h as i64 || {
                        flat[ny as usize * w + nx as usize]
                    }
                })
            })
        })
        .collect()
}

/// **The defect gate.** `edge_aliasing` — the fraction of edge pixels that are hard steps rather
/// than graded — must fall measurably on a hard-edged scene when SMAA runs. This is the number the
/// pass exists to move.
///
/// Both renders switch off [`NO_SOFTENING`]. Measured on the pinned RTX 3090 at 480x270 on
/// post-full.yaml: **0.7983 off -> 0.4640 smaa**, a drop of 0.3343, against **0.4916** for plain
/// 2x supersampling (the 0.15 reference `supersampling.rs` demands of SSAA). The margin is 0.15, half
/// the measured drop rounded down to 0.05: a pass that reads as a no-op or a regression fails, and
/// ordinary tuning of the detection threshold does not.
#[test]
fn needs_gpu_smaa_lowers_edge_aliasing() {
    let test = "smaa_edge_aliasing";
    let (w, h) = (480, 270);
    let Some(off) = render_or_skip(
        test,
        "post-full.yaml",
        w,
        h,
        1.0,
        &NO_SOFTENING,
        AntiAliasing::Off,
    ) else {
        return;
    };
    let smaa = render_or_skip(
        test,
        "post-full.yaml",
        w,
        h,
        1.0,
        &NO_SOFTENING,
        AntiAliasing::Smaa,
    )
    .expect("adapter still there");
    let ss2 = render_or_skip(
        test,
        "post-full.yaml",
        w,
        h,
        2.0,
        &NO_SOFTENING,
        AntiAliasing::Off,
    )
    .expect("adapter still there");
    let (off, smaa, ss2) = (image_stats(&off), image_stats(&smaa), image_stats(&ss2));
    eprintln!(
        "[{test}] edge_aliasing off {:.4}, smaa {:.4}, scale 2 {:.4}",
        off.edge_aliasing, smaa.edge_aliasing, ss2.edge_aliasing
    );
    assert!(
        smaa.edge_aliasing < off.edge_aliasing - 0.15,
        "smaa must measurably soften hard edges: {:.4} -> {:.4}",
        off.edge_aliasing,
        smaa.edge_aliasing,
    );
}

/// **SMAA resolves edges; it does not blur the frame.** Two halves, both on post-full.yaml at
/// 480x270 under [`NO_SOFTENING`]:
///
/// - The smaa frame is closer to a 4x-supersampled reference than the unresolved frame is. Measured
///   on the pinned RTX 3090: **ssim 0.9895 (smaa) vs 0.9858 (off)**.
/// - Away from edges the smaa frame is the off frame. The mask comes from the off frame: a pixel
///   counts when its luma step to each 4-neighbour is under 26 levels (SMAA's 0.1 threshold in the
///   encoded domain) and the same holds within radius 2; `masked_ssim` then keeps only the 11x11
///   windows wholly inside it. Measured: **ssim 0.99991 over 78424 windows**, so 0.999 leaves a
///   tenth-of-a-percent for rounding. The window floor of 50000 (of about 122000 possible) stops
///   the mask from going vacuous.
#[test]
fn needs_gpu_smaa_is_not_a_blur() {
    let test = "smaa_is_not_a_blur";
    let (w, h) = (480, 270);
    let Some(off) = render_or_skip(
        test,
        "post-full.yaml",
        w,
        h,
        1.0,
        &NO_SOFTENING,
        AntiAliasing::Off,
    ) else {
        return;
    };
    let smaa = render_or_skip(
        test,
        "post-full.yaml",
        w,
        h,
        1.0,
        &NO_SOFTENING,
        AntiAliasing::Smaa,
    )
    .expect("adapter still there");
    let ss4 = render_or_skip(
        test,
        "post-full.yaml",
        w,
        h,
        4.0,
        &NO_SOFTENING,
        AntiAliasing::Off,
    )
    .expect("adapter still there");
    let toward_smaa = compare(&smaa, &ss4).ssim;
    let toward_off = compare(&off, &ss4).ssim;
    let flat = masked_ssim(&off, &smaa, &edge_free_mask(&off)).expect("edge-free windows exist");
    eprintln!(
        "[{test}] ssim to scale 4: smaa {toward_smaa:.4}, off {toward_off:.4}; edge-free ssim {:.5} over {} windows",
        flat.ssim, flat.windows
    );
    assert!(
        toward_smaa > toward_off,
        "smaa must sit nearer the supersampled reference: {toward_smaa:.4} vs {toward_off:.4}",
    );
    assert!(
        flat.windows >= 50_000,
        "the edge-free mask went vacuous: {} windows",
        flat.windows
    );
    assert!(
        flat.ssim >= 0.999,
        "smaa changed edge-free pixels: ssim {:.5}",
        flat.ssim
    );
}

/// The opaque UI panel's pixel rectangle at 960x540, inset 2 px, as `(x0, y0, x1, y1)` with the end
/// exclusive. The scene's `label-panel` is bottom-right anchored at offset (-24, -24) with size
/// 300x64 on the 1920x1080 reference canvas, which the overlay halves at 960x540: x 798..948,
/// y 496..528.
const LABEL_PANEL: (u32, u32, u32, u32) = (800, 498, 946, 526);

/// **The overlay draws after the resolve.** The scene's opaque label panel is UI, so its interior
/// must be byte-identical between off and smaa; a glyph edge blended over scene pixels would not be,
/// which is why the gate measures inside the panel. The whole frames must differ, or SMAA never ran
/// and the equality proves nothing.
#[test]
fn needs_gpu_smaa_leaves_ui_pixels_alone() {
    let test = "smaa_ui";
    let Some(off) = render_or_skip(
        test,
        "anti-aliasing.yaml",
        960,
        540,
        1.0,
        &[],
        AntiAliasing::Off,
    ) else {
        return;
    };
    let smaa = render_or_skip(
        test,
        "anti-aliasing.yaml",
        960,
        540,
        1.0,
        &[],
        AntiAliasing::Smaa,
    )
    .expect("adapter still there");
    assert_ne!(off.rgba, smaa.rgba, "smaa must have run");
    let (x0, y0, x1, y1) = LABEL_PANEL;
    for y in y0..y1 {
        let row = ((y * 960 + x0) * 4) as usize..((y * 960 + x1) * 4) as usize;
        assert_eq!(
            off.rgba[row.clone()],
            smaa.rgba[row],
            "row {y} of the panel moved"
        );
    }
}

/// The grain panel's patch in a 480x270 frame, `(x, y, width, height)`: the middle of the flat wall
/// (x 328..474, y 54..183), with no edge, prop or shadow inside it.
const GRAIN_PATCH: (u32, u32, u32, u32) = (368, 95, 64, 48);

/// **AA never smears the film grain.** Detection runs on the grain-free frame, and `smaa_blend`
/// applies the same grain at the same pixel positions as composite does. Measured on the pinned RTX
/// 3090 on the wall patch at 480x270: **std 4.4529 off, 4.4521 smaa, 0.2051 with grain 0**. The
/// smaa/off agreement bound is 10%; the non-vacuity bound (the grain must be at least 1.3x the
/// grain-free spread) holds by a factor of 21. Dropping `smaa_blend`'s grain collapses the smaa std
/// to the 0.2 floor and fails the first bound.
#[test]
fn needs_gpu_smaa_keeps_the_grain() {
    let test = "smaa_grain";
    let (w, h) = (480, 270);
    let Some(off) = render_or_skip(
        test,
        "anti-aliasing.yaml",
        w,
        h,
        1.0,
        &[],
        AntiAliasing::Off,
    ) else {
        return;
    };
    let smaa = render_or_skip(
        test,
        "anti-aliasing.yaml",
        w,
        h,
        1.0,
        &[],
        AntiAliasing::Smaa,
    )
    .expect("adapter still there");
    let clean = render_or_skip(
        test,
        "anti-aliasing.yaml",
        w,
        h,
        1.0,
        &["/entities/1/components/PostProcess/grain=0.0"],
        AntiAliasing::Off,
    )
    .expect("adapter still there");
    let (s_off, s_smaa, s_clean) = (
        patch_std(&off, GRAIN_PATCH),
        patch_std(&smaa, GRAIN_PATCH),
        patch_std(&clean, GRAIN_PATCH),
    );
    eprintln!("[{test}] patch std: off {s_off:.4}, smaa {s_smaa:.4}, grain 0 {s_clean:.4}");
    assert!(
        s_off > 1.3 * s_clean,
        "the grain must show: std {s_off:.4} with grain, {s_clean:.4} without",
    );
    assert!(
        (s_smaa - s_off).abs() <= 0.1 * s_off,
        "smaa changed the grain: std {s_smaa:.4} vs {s_off:.4} off",
    );
}

/// **Same input, same bytes.** The golden pins SMAA, so two renders of the grained gallery scene
/// must agree exactly.
#[test]
fn needs_gpu_smaa_is_deterministic() {
    let test = "smaa_determinism";
    let Some(first) = render_or_skip(
        test,
        "anti-aliasing.yaml",
        960,
        540,
        1.0,
        &[],
        AntiAliasing::Smaa,
    ) else {
        return;
    };
    let second = render_or_skip(
        test,
        "anti-aliasing.yaml",
        960,
        540,
        1.0,
        &[],
        AntiAliasing::Smaa,
    )
    .expect("adapter still there");
    assert_eq!(first.rgba, second.rgba);
}
