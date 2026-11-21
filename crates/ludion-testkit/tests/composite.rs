//! The composite pass's defect gate (`Plan/post-process-pipeline.md` wave 4), measured on a real
//! gallery scene with `ludion image-stats` — the machine half of the arc's instrument.
//!
//! It lives here rather than in `ludion-renderer`'s own test module for the same reason
//! `supersampling.rs` does: `image_stats` is `ludion-testkit`'s, and `ludion-testkit` sits *above*
//! the renderer in the crate graph. The renderer keeps the composite tests that need only pixel
//! arithmetic (byte-identity, the operators' reference values, the exposure ordering).
//!
//! Needs a real GPU adapter and skips gracefully (printing why) when there is none, matching
//! `gallery.rs`.

use std::path::PathBuf;

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

/// Renders `scene` at `factor` render scale with `overrides` applied in memory, or `None` (after
/// printing why) when the machine has no GPU adapter.
///
/// Overrides rather than an edited scene, in both directions: an effect a scene leaves at its
/// default is switched *on* through the `--set` seam wave 1 built, and an effect `post-full.yaml`
/// now authors is switched *off* through the same seam — which is what keeps each gate below a real
/// on/off pair rather than a comparison of two frames that both carry the effect.
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
    let size = FrameSize::scaled(
        width,
        height,
        RenderScale::new(factor).expect("an in-range scale"),
    );
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
        ludion_engine::AntiAliasing::Off,
        None,
    ) {
        Ok(RenderOutcome { image, .. }) => Some(DecodedImage {
            width: image.width,
            height: image.height,
            rgba: image.rgba,
        }),
        Err(errors) if ludion_core::require_gpu::is_no_adapter(&errors) => {
            eprintln!("[{test}] SKIPPED: no GPU adapter available");
            None
        }
        Err(errors) => panic!("[{test}] rendering {scene} at {factor}x failed: {errors:?}"),
    }
}

/// **The defect gate.** `banding_score` — the fraction of the image sitting in flat 8-bit
/// steps a smooth gradient should have crossed — must fall when output dither is switched on.
///
/// Measured on `night-sky.yaml`, which carries the highest banding in the repo (0.8979 in the wave-1
/// baseline) because an authored night-sky gradient at low luminance is exactly the case an 8-bit
/// target quantizes worst — and exactly the defect this feature exists for. `post-full.yaml` is a
/// far *less* sensitive probe — it authors `grain`, which breaks the same runs — and would
/// understate the effect.
///
/// Measured on the pinned RTX 3090 at 480x270: **0.8417 -> 0.3365**, so the 0.10 the gate demands is
/// wide headroom against a drop of nearly 0.51. At the golden's own 960x540 — where the wave-1
/// baseline was taken — it is **0.8979 -> 0.2877**, the same effect at a different sampling density;
/// the gate compares two renders at one size, so it never depends on which.
///
/// The dither now ships **on**, so it is the *undithered* render that has to be asked for. The
/// direction of the comparison is what the gate is, and it is unchanged.
#[test]
fn needs_gpu_dither_lowers_banding_on_the_night_sky() {
    let test = "banding";
    let (w, h) = (480, 270);
    let Some(plain) = render_or_skip(
        test,
        "night-sky.yaml",
        w,
        h,
        1.0,
        &["/entities/0/components/PostProcess/dither=false"],
    ) else {
        return;
    };
    let dithered =
        render_or_skip(test, "night-sky.yaml", w, h, 1.0, &[]).expect("adapter still there");

    let before: ImageStats = image_stats(&plain);
    let after: ImageStats = image_stats(&dithered);
    eprintln!(
        "[{test}] banding_score {:.4} -> {:.4}",
        before.banding_score, after.banding_score,
    );
    assert!(
        after.banding_score < before.banding_score - 0.10,
        "dither must measurably break up 8-bit banding: {:.4} -> {:.4}",
        before.banding_score,
        after.banding_score,
    );
}

/// **The dither survives a resample.** A scaled frame is dithered where it is written — the last
/// output pass owns the dither — so a gradient rendered at 2x or 0.5x still gets the break-up the
/// band-free look needs. Each scale compares dithered against `dither=false` at that scale, and the
/// scaled dithered score has to stay near the native dithered one.
#[test]
fn needs_gpu_dither_survives_a_resample_on_the_night_sky() {
    let test = "banding_resample";
    let (w, h) = (480, 270);
    let off = &["/entities/0/components/PostProcess/dither=false"];
    let Some(native) = render_or_skip(test, "night-sky.yaml", w, h, 1.0, &[]) else {
        return;
    };
    let native_score = image_stats(&native).banding_score;
    for factor in [2.0, 0.5] {
        let plain =
            render_or_skip(test, "night-sky.yaml", w, h, factor, off).expect("adapter still there");
        let dithered =
            render_or_skip(test, "night-sky.yaml", w, h, factor, &[]).expect("adapter still there");
        let (before, after) = (
            image_stats(&plain).banding_score,
            image_stats(&dithered).banding_score,
        );
        eprintln!(
            "[{test}] {factor}x banding_score {before:.4} -> {after:.4} (native dithered \
             {native_score:.4})",
        );
        assert!(
            after < before - 0.10,
            "dither must survive the resample at {factor}x: {before:.4} -> {after:.4}",
        );
        assert!(
            (after - native_score).abs() < RESAMPLE_BANDING_MARGIN,
            "the resampled dithered frame must band like the native one: {after:.4} at {factor}x \
             vs {native_score:.4}",
        );
    }
}

/// How far the scaled dithered `banding_score` may sit from the native one.
const RESAMPLE_BANDING_MARGIN: f64 = 0.05;

/// What every `banding_score` gate on `post-full.yaml` has to switch off.
///
/// The scene authors `grain`, and grain removes banding by construction — it is a per-pixel offset
/// whose whole job is to break the constant-luma runs this metric counts. Measured, it floors
/// `banding_score` on this scene to about 0.03, at which point a march or a gather that genuinely
/// quantized could not move the number and the gate would pass on anything. Switching it off is what
/// keeps the gates able to fail; the effect under test is still measured through the rest of the
/// shipped chain, dither included.
const GRAIN_OFF: &str = "/entities/0/components/PostProcess/grain=0.0";

/// **Wave 8's defect gate.** A dithered ray-march is a banding risk by construction — thirty-two
/// steps per ray is a coarse integral, and a coarse integral over a smooth medium is exactly what
/// reads as concentric steps — so the wave has to show that its `BAYER4` offsets plus the 4×4 blur
/// actually resolve it rather than merely spreading it.
///
/// The claim is deliberately two-sided. `SHAFTS_BANDING_CEILING` is the absolute number, and the
/// comparison against the same scene with shafts off is the stronger half: an effect that added
/// stepped light would raise the score, and it must not.
///
/// Measured on the pinned RTX 3090 at 480x270 on `post-full.yaml` (whose moon is the only sun in the
/// gallery, so it is the only scene that renders shafts at all), through the engine's shipped
/// imaging chain and the scene's authored roster minus [`GRAIN_OFF`]: **0.2583 off -> 0.2578 on**,
/// the scene's own `shafts` overridden to `0` at one end and to `1.0` at the other.
///
/// **The second clause allows a bounded rise rather than demanding a fall, because the sign of the
/// movement is not the claim.** Under the Narkowicz ACES fit the same measurement read
/// 0.4761 -> 0.4258, a clear fall: in-scattered light lifted the deep-shadow gradient out of the
/// flat 8-bit steps it sat in. AgX has a gentler toe and the exposure sits at 1.3, so those shadows
/// are already lifted before the march contributes, and what it then adds is a broad, shallow ramp
/// — content this metric reads as flat. The movement is now hundredths either way on a metric whose
/// range is 1.0; a march that genuinely quantized would move it by an order of magnitude more, which
/// is what [`SHAFTS_BANDING_RISE`] is sized to catch.
#[test]
fn needs_gpu_light_shafts_do_not_band_the_night_scene() {
    let test = "shafts_banding";
    let (w, h) = (480, 270);
    let Some(plain) = render_or_skip(
        test,
        "post-full.yaml",
        w,
        h,
        1.0,
        &["/entities/0/components/PostProcess/shafts=0.0", GRAIN_OFF],
    ) else {
        return;
    };
    let shafted = render_or_skip(
        test,
        "post-full.yaml",
        w,
        h,
        1.0,
        &["/entities/0/components/PostProcess/shafts=1.0", GRAIN_OFF],
    )
    .expect("adapter still there");

    let before: ImageStats = image_stats(&plain);
    let after: ImageStats = image_stats(&shafted);
    eprintln!(
        "[{test}] banding_score {:.4} -> {:.4}",
        before.banding_score, after.banding_score,
    );
    assert!(
        after.banding_score < SHAFTS_BANDING_CEILING,
        "a dithered march must resolve: banding_score {:.4} exceeds {SHAFTS_BANDING_CEILING}",
        after.banding_score,
    );
    assert!(
        after.banding_score <= before.banding_score + SHAFTS_BANDING_RISE,
        "shafts must not add more than {SHAFTS_BANDING_RISE} of banding: {:.4} -> {:.4}",
        before.banding_score,
        after.banding_score,
    );
}

/// The absolute ceiling `light_shafts_do_not_band_the_night_scene` holds the march to, with wide
/// headroom over the measured 0.2578 for the M4's own arithmetic — so it can only be met by a march
/// that resolves.
const SHAFTS_BANDING_CEILING: f64 = 0.55;

/// How much banding the shaft term may add on the same scene. The measured movement is now
/// hundredths (a shallow ramp over deep shadows rather than anything stepped) and has been negative
/// as often as positive, so this bound is sized off the ACES-era 0.031 rather than off the current
/// noise. A march whose integral actually quantized would move the metric far past this.
const SHAFTS_BANDING_RISE: f64 = 0.06;

/// **Wave 9's defect gate.** A depth of field makes the smoothest images this pipeline can produce —
/// a defocused background is a low-frequency gradient by construction — and a smooth low-luminance
/// gradient on an 8-bit target is exactly what bands. So the effect has to be shown not to introduce
/// the defect that output dither exists to remove.
///
/// The metric is deliberately `banding_score` and deliberately not `edge_aliasing`: a blur lowers
/// edge aliasing for free, so gating on it would be the metric-gaming the plan's Risks section warns
/// about — a number that moves without the image being better.
///
/// Measured on the pinned RTX 3090 at 480x270 on `post-full.yaml` with a 2%-of-frame-height aperture
/// against the scene's own `focus_distance` of 32 (the beacon on its plinth), which puts both the
/// foreground and the whole skyline out of focus, and with [`GRAIN_OFF`]: **0.2552 off -> 0.2595
/// on**, the scene's authored `dof` overridden to `0` at one end and to `0.02` at the other. It
/// rises slightly, because that is what replacing detail with a gradient does, and the gate is that
/// it stays well inside the ceiling rather than that it cannot move at all.
#[test]
fn needs_gpu_depth_of_field_does_not_band_the_night_scene() {
    let test = "dof_banding";
    let (w, h) = (480, 270);
    let Some(plain) = render_or_skip(
        test,
        "post-full.yaml",
        w,
        h,
        1.0,
        &["/entities/0/components/PostProcess/dof=0.0", GRAIN_OFF],
    ) else {
        return;
    };
    let defocused = render_or_skip(
        test,
        "post-full.yaml",
        w,
        h,
        1.0,
        &["/entities/0/components/PostProcess/dof=0.02", GRAIN_OFF],
    )
    .expect("adapter still there");

    let before: ImageStats = image_stats(&plain);
    let after: ImageStats = image_stats(&defocused);
    eprintln!(
        "[{test}] banding_score {:.4} -> {:.4}",
        before.banding_score, after.banding_score,
    );
    assert!(
        after.banding_score < DOF_BANDING_CEILING,
        "a defocused frame must not band: banding_score {:.4} exceeds {DOF_BANDING_CEILING}",
        after.banding_score,
    );
}

/// The ceiling `depth_of_field_does_not_band_the_night_scene` holds the gather to. Set well above
/// the measured 0.2595 with headroom for the M4's own arithmetic, and far below the 0.8979 the same
/// gradient scores undithered at the golden's resolution — so a gather that traded detail for visible
/// steps could not meet it.
const DOF_BANDING_CEILING: f64 = 0.60;

/// **Wave 10's grain gate, and the reason grain is in the roster at all.** Decision 13 admits film
/// emulation partly because "grain genuinely helps 8-bit night gradients" — a claim about a
/// measurable defect, so it is measured here rather than asserted. The mechanism is the dither's: a
/// per-pixel offset applied in the *encoded* domain collapses the constant-luma runs an 8-bit
/// gradient quantizes into, without touching the image's low-frequency content.
///
/// Same scene and same shape as `dither_lowers_banding_on_the_night_sky`, deliberately, so the two
/// numbers are directly comparable: `night-sky.yaml` carries the highest banding in the repo, which
/// is exactly the defect the claim is about.
///
/// The amplitude is `0.02` because that is what the component's own documentation calls a fine
/// film-ish stipple — about ±5 of the 255 output steps — not because it is the value that wins. The
/// plan's Risks section warns about metric gaming, and the check against it is that the effect is
/// not bought with amplitude: at `0.005` (±1.3 steps, finer than a shipped look would ever be) the
/// same measurement already reads **0.8417 -> 0.1370**.
///
/// Measured on the pinned RTX 3090 at 480x270: **0.8417 -> 0.0115**, so the 0.10 the gate demands —
/// the dither gate's own margin — is met with a drop of 0.83. At the golden's 960x540 it is
/// **0.8979 -> 0.0143**. Grain lands well below the dither's 0.2877 at that size for a plain reason:
/// the ordered tile repeats every four pixels, and the hash does not repeat at all.
///
/// `grain_size` is left at its shipped default, which at a 270-line frame floors to one render pixel
/// (`MIN_GRAIN_CELL`) — the finest grain there is, and so the one that breaks the most runs.
#[test]
fn needs_gpu_grain_lowers_banding_on_the_night_sky() {
    let test = "grain_banding";
    let (w, h) = (480, 270);
    let Some(plain) = render_or_skip(test, "night-sky.yaml", w, h, 1.0, &[]) else {
        return;
    };
    let grainy = render_or_skip(
        test,
        "night-sky.yaml",
        w,
        h,
        1.0,
        &["/entities/0/components/PostProcess/grain=0.02"],
    )
    .expect("adapter still there");

    let before: ImageStats = image_stats(&plain);
    let after: ImageStats = image_stats(&grainy);
    eprintln!(
        "[{test}] banding_score {:.4} -> {:.4}",
        before.banding_score, after.banding_score,
    );
    assert!(
        after.banding_score < before.banding_score - 0.10,
        "grain must measurably break up 8-bit banding: {:.4} -> {:.4}",
        before.banding_score,
        after.banding_score,
    );
}

/// **Wave 10's scale-invariance gate: the load-bearing justification for `grain_size` being a
/// fraction of the frame's height rather than a pixel count.** Grain authored per *render* pixel is
/// averaged straight back out by the `resolve` pass at `--scale 2`, so raising a quality setting
/// would change the look — which the arc's standing rule (a scale change is a quality change, never
/// a look change) treats as a bug rather than a trade.
///
/// The claim is proven as an **on/off ratio at each scale** rather than as an absolute spread, which
/// is wave 8's shape: the two scales draw deliberately different images, so only a quantity
/// normalised by the same frame's grain-free self compares across them. The spread is the luma
/// standard deviation over one sky patch, which is the cleanest reading of an amplitude these
/// instruments give — `banding_score` saturates at the cell sizes this test needs, and `ssim`
/// measures agreement rather than spread.
///
/// Measured on the M4 (patch luma sigma), output 480x270, `grain: 0.02`:
///
/// | `grain_size`    | scale | sigma, grain off | sigma, grain on | ratio  |
/// |-----------------|-------|------------------|-----------------|--------|
/// | 0.02            | 1x    | 1.8256           | 3.3674          | 1.8445 |
/// | 0.02            | 2x    | 1.8246           | 3.3673          | 1.8455 |
/// | 0.002 (shipped) | 1x    | 1.8256           | 3.4603          | 1.8954 |
/// | 0.002 (shipped) | 2x    | 1.8246           | 3.4549          | 1.8934 |
///
/// The grain is applied after the resample, at the output extent, so the cell is the same fraction of
/// the frame at every scale — including the shipped `grain_size`, whose cell floors to one *output*
/// pixel at this height.
#[test]
fn needs_gpu_grain_is_render_scale_invariant() {
    let test = "grain_scale";
    let (w, h) = (480, 270);
    let coarse = [
        "/entities/0/components/PostProcess/grain=0.02",
        "/entities/0/components/PostProcess/grain_size=0.02",
    ];
    // The shipped `grain_size`: the finest grain there is at this height.
    let finest = ["/entities/0/components/PostProcess/grain=0.02"];
    let Some(native_off) = render_or_skip(test, "night-sky.yaml", w, h, 1.0, &[]) else {
        return;
    };
    let ssaa_off =
        render_or_skip(test, "night-sky.yaml", w, h, 2.0, &[]).expect("adapter still there");
    let spread = |image: &DecodedImage| patch_std(image, GRAIN_PATCH);

    for (label, grain) in [
        ("grain_size 0.02", &coarse[..]),
        ("shipped grain_size", &finest[..]),
    ] {
        let native_on =
            render_or_skip(test, "night-sky.yaml", w, h, 1.0, grain).expect("adapter still there");
        let ssaa_on =
            render_or_skip(test, "night-sky.yaml", w, h, 2.0, grain).expect("adapter still there");
        let (native_ratio, ssaa_ratio) = (
            spread(&native_on) / spread(&native_off),
            spread(&ssaa_on) / spread(&ssaa_off),
        );
        eprintln!(
            "[{test}] {label}: patch sigma 1x {:.4} -> {:.4} (ratio {native_ratio:.4}), 2x {:.4} \
             -> {:.4} (ratio {ssaa_ratio:.4})",
            spread(&native_off),
            spread(&native_on),
            spread(&ssaa_off),
            spread(&ssaa_on),
        );

        // Non-vacuity, both halves. The grain has to have reached the patch at all — two ratios of
        // 1.0 would agree perfectly and prove nothing — and the two grain-free frames have to agree
        // about what the patch *is*, or the ratios would be normalised by two different denominators.
        assert!(
            native_ratio > 1.3,
            "{label}: grain must visibly spread the patch before its scale invariance means \
             anything: {native_ratio:.4}",
        );
        assert!(
            (spread(&native_off) - spread(&ssaa_off)).abs() < 0.05 * spread(&native_off),
            "{label}: the two grain-free frames must agree about the patch: {:.4} at 1x vs {:.4} \
             at 2x",
            spread(&native_off),
            spread(&ssaa_off),
        );

        assert!(
            (native_ratio - ssaa_ratio).abs() < GRAIN_SCALE_TOLERANCE * native_ratio,
            "{label}: the resample must not average the grain away: ratio {native_ratio:.4} at 1x \
             vs {ssaa_ratio:.4} at 2x, beyond {GRAIN_SCALE_TOLERANCE}",
        );
    }
}

/// How far apart the 1x and 2x grain-to-no-grain ratios may sit. The measured gap is under 0.2%
/// (table above); the margin is for another machine's arithmetic, and it stays far under the gap
/// grain averaged away by a resample would open.
const GRAIN_SCALE_TOLERANCE: f64 = 0.10;

/// The sky patch `grain_is_render_scale_invariant` measures, as `(x, y, width, height)` in a 480x270
/// frame: upper right, clear of the sphere grid, the sun disc and the lantern, so the only thing in
/// it is the smooth gradient the grain is applied to.
const GRAIN_PATCH: (u32, u32, u32, u32) = (400, 8, 64, 48);

/// Rec. 709 luma standard deviation over `(x, y, width, height)` of `image`, in 8-bit levels — the
/// spread of a patch, in the same encoded domain `image_stats` scores in.
fn patch_std(image: &DecodedImage, (x0, y0, w, h): (u32, u32, u32, u32)) -> f64 {
    let luma: Vec<f64> = (y0..y0 + h)
        .flat_map(|y| (x0..x0 + w).map(move |x| (x, y)))
        .map(|(x, y)| {
            let px = &image.rgba[((y * image.width + x) * 4) as usize..][..3];
            0.2126 * f64::from(px[0]) + 0.7152 * f64::from(px[1]) + 0.0722 * f64::from(px[2])
        })
        .collect();
    let mean = luma.iter().sum::<f64>() / luma.len() as f64;
    (luma.iter().map(|v| (v - mean).powi(2)).sum::<f64>() / luma.len() as f64).sqrt()
}
