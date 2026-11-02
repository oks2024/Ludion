//! `ParticleEmitter.lit` end to end: a lit emitter's motes take the light a lit sprite would at their
//! centres, and `lit: false` / an absent field are the same render.
//!
//! Every GPU test skips (printing why) when there is no adapter. Scenes are written under
//! `CARGO_TARGET_TMPDIR` at a name carrying the test and the pid, so parallel tests never share one.

use std::path::PathBuf;

use ludion_engine::{FrameSize, RenderOutcome, render_scene};

/// A scene of a dark room (zero ambient, black sky), a camera at z=8, a point light 1.5 units in
/// front of the emitter's near face, and one box emitter of still motes spanning x in [-6, 6].
/// `lit_line` is spliced into the emitter. The light is offset in depth because the term is
/// `|n·l|` with `n` the camera-facing normal: a light in the motes' own plane gives zero.
fn scene_text(blend: &str, lit_line: &str, with_light: bool) -> String {
    let light = if with_light {
        "  - name: lamp\n    components:\n      Transform:\n        translation:\n          - -3.0\n          - 0.0\n          - 2.5\n      PointLight:\n        intensity: 6.0\n        range: 4.5\n"
    } else {
        ""
    };
    format!(
        "format: 1\n\
         seed: 5\n\
         entities:\n\
         \x20 - name: camera\n    components:\n      Transform:\n        translation:\n          - 0.0\n          - 0.0\n          - 8.0\n      Camera: {{}}\n\
         \x20 - name: dark\n    components:\n      Environment:\n        sky_zenith:\n          - 0.0\n          - 0.0\n          - 0.0\n        sky_horizon:\n          - 0.0\n          - 0.0\n          - 0.0\n        sky_ground:\n          - 0.0\n          - 0.0\n          - 0.0\n        ambient:\n          - 0.0\n          - 0.0\n          - 0.0\n\
         {light}\
         \x20 - name: motes\n    components:\n      ParticleEmitter:\n        shape: box\n        extent:\n          - 6.0\n          - 1.0\n          - 1.0\n        rate: 600.0\n        life:\n          - 2.0\n          - 3.0\n        speed:\n          - 0.0\n          - 0.0\n        start_size: 0.3\n        end_size: 0.3\n        start_color:\n          - 1.0\n          - 1.0\n          - 1.0\n          - 1.0\n        end_color:\n          - 1.0\n          - 1.0\n          - 1.0\n          - 1.0\n        blend: {blend}\n        softness: 0.0\n        seed: 3\n{lit_line}"
    )
}

fn write_scene(test: &str, name: &str, text: &str) -> PathBuf {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
        .join(format!("lit-particles-{test}-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join(format!("{name}.yaml"));
    std::fs::write(&path, text).unwrap();
    path
}

/// Renders `text`, or `None` (after printing why) when there is no GPU.
fn render_or_skip(test: &str, name: &str, text: &str) -> Option<RenderOutcome> {
    let path = write_scene(test, name, text);
    match render_scene(&path, FrameSize::native(160, 90), false) {
        Ok(outcome) => Some(outcome),
        Err(errors) if ludion_core::require_gpu::is_no_adapter(&errors) => {
            eprintln!("[{test}] SKIPPED: no GPU adapter available");
            None
        }
        Err(errors) => panic!("[{test}] render failed: {errors:?}"),
    }
}

/// Mean RGB sum over the pixel columns `[x0, x1)`, all rows.
fn mean_luma(outcome: &RenderOutcome, x0: u32, x1: u32) -> f64 {
    let img = &outcome.image;
    let mut sum = 0u64;
    let mut count = 0u64;
    for y in 0..img.height {
        for x in x0..x1 {
            let i = ((y * img.width + x) * 4) as usize;
            sum += img.rgba[i] as u64 + img.rgba[i + 1] as u64 + img.rgba[i + 2] as u64;
            count += 1;
        }
    }
    sum as f64 / count as f64
}

/// A lit box emitter straddling a light is far brighter on the lit side; the unlit twin is flat
/// across the box. The thirds are the box's x in [-6, -2] and [2, 6] (columns 22..60 and 100..138
/// at 160 px for a 60° camera 8 units away); the output dither and bloom keep the dark side from
/// being exactly zero, so the comparison is a ratio.
#[test]
fn needs_gpu_a_lit_emitter_straddling_a_light_is_brighter_inside() {
    for blend in ["alpha", "additive"] {
        let name = format!("straddle-{blend}");
        let Some(lit) = render_or_skip(
            &name,
            "lit",
            &scene_text(blend, "        lit: true\n", true),
        ) else {
            return;
        };
        let unlit = render_or_skip(&name, "unlit", &scene_text(blend, "", true)).unwrap();
        let (lit_in, lit_out) = (mean_luma(&lit, 22, 60), mean_luma(&lit, 100, 138));
        let (unlit_in, unlit_out) = (mean_luma(&unlit, 22, 60), mean_luma(&unlit, 100, 138));
        assert!(
            lit_in > 3.0 * lit_out.max(1.0),
            "{blend}: lit inside {lit_in} must far exceed lit outside {lit_out}",
        );
        assert!(
            unlit_in > 0.0 && (unlit_in / unlit_out - 1.0).abs() < 0.35,
            "{blend}: the unlit twin must be flat across the box ({unlit_in} vs {unlit_out})",
        );
    }
}

/// `lit: false` and an absent field are the same scene: byte-equal images.
#[test]
fn needs_gpu_lit_false_and_an_absent_field_render_identically() {
    let test = "lit-false-vs-absent";
    let Some(absent) = render_or_skip(test, "absent", &scene_text("additive", "", true)) else {
        return;
    };
    let explicit = render_or_skip(
        test,
        "explicit",
        &scene_text("additive", "        lit: false\n", true),
    )
    .unwrap();
    assert_eq!(absent.image, explicit.image);
}
