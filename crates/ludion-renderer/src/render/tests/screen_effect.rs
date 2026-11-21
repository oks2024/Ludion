//! Screen effects: the overlay split and the effect pass. Every pixel outside an effect's rect keeps the
//! no-effect frame, the UI at or above an effect's `ui_start` draws clean, and the animated stages are
//! functions of the tick and nothing else.

use super::transparent_dof::grain_scene;
use super::*;
use crate::graph::RenderScale;
use crate::scene::UiInstance;
use crate::screen_effect::{ScreenEffectData, ScreenMask};

const W: u32 = 96;
const H: u32 = 64;

/// An effect over `rect_px` that changes nothing: every stage at its off value.
pub(super) fn neutral(rect_px: [f32; 4], ui_start: u32) -> ScreenEffectData {
    ScreenEffectData {
        rect_px,
        ui_start,
        time_s: 0.0,
        frame: 0,
        seed: 0,
        scanlines: 0.0,
        mask_strength: 0.0,
        curvature: 0.0,
        glow: 0.0,
        chroma_bleed: 0.0,
        noise: 0.0,
        tracking: 0.0,
        roll: 0.0,
        jitter: 0.0,
        flicker: 0.0,
        sweep: 0.0,
        sweep_period: 4.0,
        scanline_count: 16,
        posterize: 0,
        mask: ScreenMask::None,
        phosphor: [0.0; 3],
        interlace: false,
    }
}

fn scanlined(rect_px: [f32; 4], ui_start: u32) -> ScreenEffectData {
    ScreenEffectData {
        scanlines: 0.8,
        ..neutral(rect_px, ui_start)
    }
}

fn ui_rect(rect_px: [f32; 4], color: [f32; 4]) -> UiInstance {
    UiInstance::Rect {
        rect_px,
        texture: None,
        color,
        uv: [0.0, 0.0, 1.0, 1.0],
        clip: UiClip::NONE,
        view: None,
        rotation: crate::UiRotation::NONE,
    }
}

/// The 3D box plus two opaque UI rects, a white one and a red one, both inside the region the tests
/// filter (`[32, 16, 48, 40]`).
fn scene(w: u32, h: u32, effects: Vec<ScreenEffectData>) -> RenderScene {
    let mut scene = grain_scene(w, h);
    scene.ui = vec![
        ui_rect([36.0, 20.0, 16.0, 16.0], [1.0, 1.0, 1.0, 1.0]),
        ui_rect([56.0, 20.0, 16.0, 16.0], [1.0, 0.0, 0.0, 1.0]),
    ];
    scene.screen_effects = effects;
    scene
}

fn render(renderer: &mut Renderer, scene: &RenderScene, size: FrameSize) -> Vec<u8> {
    renderer.render(scene, size).expect("render").rgba
}

fn pixel(rgba: &[u8], w: u32, x: u32, y: u32) -> [u8; 4] {
    let i = ((y * w + x) * 4) as usize;
    [rgba[i], rgba[i + 1], rgba[i + 2], rgba[i + 3]]
}

/// Whether pixel `(x, y)` is inside `rect`.
fn inside(rect: [f32; 4], x: u32, y: u32) -> bool {
    let (x, y) = (x as f32 + 0.5, y as f32 + 0.5);
    x >= rect[0] && x < rect[0] + rect[2] && y >= rect[1] && y < rect[1] + rect[3]
}

/// The effect leaves every pixel outside its rect equal to the no-effect frame, and changes some
/// inside it.
fn assert_outside_identity(renderer: &mut Renderer, size: FrameSize, label: &str) {
    let (w, h) = size.output;
    let rect = [32.0, 16.0, 48.0, 40.0];
    let plain = render(renderer, &scene(w, h, vec![]), size);
    let filtered = render(renderer, &scene(w, h, vec![scanlined(rect, 0)]), size);
    let mut changed_inside = 0;
    for y in 0..h {
        for x in 0..w {
            let same = pixel(&plain, w, x, y) == pixel(&filtered, w, x, y);
            if inside(rect, x, y) {
                changed_inside += usize::from(!same);
            } else {
                assert!(same, "{label}: pixel ({x}, {y}) outside the rect changed");
            }
        }
    }
    assert!(
        changed_inside > 0,
        "{label}: the effect changed nothing inside"
    );
}

/// (a) A region effect changes pixels inside its rect and none outside.
#[test]
fn needs_gpu_a_region_effect_leaves_the_outside_untouched() {
    let Some(mut renderer) = skip_or_renderer("a_region_effect_leaves_the_outside_untouched")
    else {
        return;
    };
    assert_outside_identity(&mut renderer, FrameSize::native(W, H), "native");
}

/// (g) The same identity under SMAA and under both resamples.
#[test]
fn needs_gpu_outside_identity_holds_under_smaa_and_a_resample() {
    let Some(mut renderer) = skip_or_renderer("outside_identity_holds_under_smaa_and_a_resample")
    else {
        return;
    };
    renderer.set_anti_aliasing(AntiAliasing::Smaa);
    assert_outside_identity(&mut renderer, FrameSize::native(W, H), "smaa");
    renderer.set_anti_aliasing(AntiAliasing::Off);
    let scaled = |factor: f32| FrameSize::scaled(W, H, RenderScale::new(factor).unwrap());
    assert_outside_identity(&mut renderer, scaled(2.0), "resolve");
    assert_outside_identity(&mut renderer, scaled(0.5), "upscale");
}

/// (b) The effect filters the UI drawn before its `ui_start` and leaves the UI after it clean.
#[test]
fn needs_gpu_ui_above_the_effect_draws_clean() {
    let Some(mut renderer) = skip_or_renderer("ui_above_the_effect_draws_clean") else {
        return;
    };
    let size = FrameSize::native(W, H);
    let plain = render(&mut renderer, &scene(W, H, vec![]), size);
    // `ui_start: 1`: the white rect (index 0) is under the effect, the red one (index 1) above it.
    let filtered = render(
        &mut renderer,
        &scene(W, H, vec![scanlined([32.0, 16.0, 48.0, 40.0], 1)]),
        size,
    );
    let (mut under, mut above) = (0, 0);
    for y in 20..36 {
        for x in 36..52 {
            under += usize::from(pixel(&plain, W, x, y) != pixel(&filtered, W, x, y));
        }
        for x in 56..72 {
            above += usize::from(pixel(&plain, W, x, y) != pixel(&filtered, W, x, y));
        }
    }
    assert!(under > 0, "the rect under the effect is filtered");
    assert_eq!(
        above, 0,
        "the rect at the effect's ui_start draws exactly as without it"
    );
}

/// (c) The noise is a function of the tick: two ticks differ, one tick twice is one frame.
#[test]
fn needs_gpu_noise_follows_the_tick() {
    let Some(mut renderer) = skip_or_renderer("noise_follows_the_tick") else {
        return;
    };
    let size = FrameSize::native(W, H);
    let at = |renderer: &mut Renderer, frame: u32| {
        let effect = ScreenEffectData {
            noise: 0.4,
            frame,
            ..neutral([0.0, 0.0, W as f32, H as f32], 0)
        };
        render(renderer, &scene(W, H, vec![effect]), size)
    };
    let f0 = at(&mut renderer, 0);
    let f0_again = at(&mut renderer, 0);
    let f1 = at(&mut renderer, 1);
    assert_eq!(f0, f0_again, "one tick is one frame");
    assert_ne!(f0, f1, "two ticks roll different noise");
}

/// (d) Curvature leaves the corners of the region black.
#[test]
fn needs_gpu_curvature_blackens_the_corners() {
    let Some(mut renderer) = skip_or_renderer("curvature_blackens_the_corners") else {
        return;
    };
    let size = FrameSize::native(W, H);
    let mut s = scene(
        W,
        H,
        vec![ScreenEffectData {
            curvature: 0.3,
            ..neutral([0.0, 0.0, W as f32, H as f32], 1)
        }],
    );
    // A full-frame white backdrop under the effect, so a black corner can only come from it.
    s.ui[0] = ui_rect([0.0, 0.0, W as f32, H as f32], [1.0; 4]);
    let img = render(&mut renderer, &s, size);
    assert_eq!(pixel(&img, W, 0, 0), [0, 0, 0, 255], "the corner is black");
    assert_eq!(
        pixel(&img, W, W / 2, 10),
        [255, 255, 255, 255],
        "the middle stays"
    );
}

/// (e) Four effects render, each altering its own rect.
#[test]
fn needs_gpu_four_effects_each_alter_their_rect() {
    let Some(mut renderer) = skip_or_renderer("four_effects_each_alter_their_rect") else {
        return;
    };
    let size = FrameSize::native(W, H);
    let rects = [
        [0.0, 0.0, 32.0, 32.0],
        [48.0, 0.0, 32.0, 32.0],
        [0.0, 32.0, 32.0, 32.0],
        [48.0, 32.0, 32.0, 32.0],
    ];
    let plain = render(&mut renderer, &scene(W, H, vec![]), size);
    let effects = rects.iter().map(|r| scanlined(*r, 0)).collect();
    let filtered = render(&mut renderer, &scene(W, H, effects), size);
    for rect in rects {
        let mut changed = 0;
        for y in 0..H {
            for x in 0..W {
                if inside(rect, x, y) {
                    changed += usize::from(pixel(&plain, W, x, y) != pixel(&filtered, W, x, y));
                }
            }
        }
        assert!(changed > 0, "{rect:?} was not filtered");
    }
}

/// (f) A fill effect filters the whole frame; the UI above it is clean.
#[test]
fn needs_gpu_a_fill_effect_filters_the_frame_under_clean_ui() {
    let Some(mut renderer) = skip_or_renderer("a_fill_effect_filters_the_frame_under_clean_ui")
    else {
        return;
    };
    let size = FrameSize::native(W, H);
    let backdrop = ui_rect([0.0, 0.0, W as f32, H as f32], [0.5, 0.5, 0.5, 1.0]);
    let badge = ui_rect([40.0, 24.0, 16.0, 16.0], [0.5, 0.5, 0.5, 1.0]);
    let build = |effects: Vec<ScreenEffectData>| {
        let mut s = scene(W, H, effects);
        s.ui = vec![backdrop.clone(), badge.clone()];
        s
    };
    let plain = render(&mut renderer, &build(vec![]), size);
    let filtered = render(
        &mut renderer,
        &build(vec![ScreenEffectData {
            posterize: 2,
            ..neutral([0.0, 0.0, W as f32, H as f32], 1)
        }]),
        size,
    );
    assert_ne!(
        pixel(&plain, W, 2, 2),
        pixel(&filtered, W, 2, 2),
        "the frame is posterized"
    );
    assert_eq!(
        pixel(&plain, W, 48, 32),
        pixel(&filtered, W, 48, 32),
        "the badge above the effect is clean",
    );
}
