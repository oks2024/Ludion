//! Animated grain: the game tick folded into the cell hash, in every pass that owns the grain.

use super::transparent_dof::{grain_post, grain_scene};
use super::*;
use crate::graph::RenderScale;

/// Where the grain is applied: the composite, the SMAA blend, or a resample's last pass.
fn owners() -> [(&'static str, AntiAliasing, FrameSize); 4] {
    let (w, h) = (96u32, 64u32);
    let scale = |factor: f32| FrameSize::scaled(w, h, RenderScale::new(factor).unwrap());
    [
        ("composite", AntiAliasing::Off, FrameSize::native(w, h)),
        ("smaa_blend", AntiAliasing::Smaa, FrameSize::native(w, h)),
        ("resolve", AntiAliasing::Off, scale(2.0)),
        ("upscale", AntiAliasing::Off, scale(0.5)),
    ]
}

fn frame(
    renderer: &mut Renderer,
    aa: AntiAliasing,
    size: FrameSize,
    grain: f32,
    tick: Option<u32>,
) -> Vec<u8> {
    renderer.set_anti_aliasing(aa);
    let mut scene = grain_scene(size.output.0, size.output.1);
    let mut post = grain_post(grain, 1.0, size.output.1);
    post.grain_tick = tick;
    scene.post = Some(post);
    renderer.render(&scene, size).expect("render").rgba
}

/// **The tick moves the grain in every owning pass, and only the tick does.** Static twice is one
/// frame; two ticks are two frames; one tick twice is one frame; and with the grain off the tick is
/// inert, so the frame equals the static one.
#[test]
fn needs_gpu_animated_grain_moves_per_tick_in_every_owning_pass() {
    let Some(mut renderer) = skip_or_renderer("animated_grain_moves_per_tick_in_every_owning_pass")
    else {
        return;
    };
    for (owner, aa, size) in owners() {
        let static_a = frame(&mut renderer, aa, size, 0.05, None);
        let static_b = frame(&mut renderer, aa, size, 0.05, None);
        assert_eq!(
            static_a, static_b,
            "{owner}: static grain must be one frame"
        );

        let tick0 = frame(&mut renderer, aa, size, 0.05, Some(0));
        let tick1 = frame(&mut renderer, aa, size, 0.05, Some(1));
        assert_ne!(
            tick0, tick1,
            "{owner}: ticks 0 and 1 must roll different grain"
        );
        assert_ne!(
            tick0, static_a,
            "{owner}: animated grain is not the static field"
        );

        let tick7_a = frame(&mut renderer, aa, size, 0.05, Some(7));
        let tick7_b = frame(&mut renderer, aa, size, 0.05, Some(7));
        assert_eq!(tick7_a, tick7_b, "{owner}: one tick must be one frame");

        let off_static = frame(&mut renderer, aa, size, 0.0, None);
        let off_animated = frame(&mut renderer, aa, size, 0.0, Some(3));
        assert_eq!(
            off_static, off_animated,
            "{owner}: the tick is inert while grain is zero",
        );
    }
}
