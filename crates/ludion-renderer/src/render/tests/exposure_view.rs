//! The metered-exposure instruments: the `exposure_meter` debug view and the verdict readback.

use super::*;
use crate::DebugView;
use crate::scene::{MeterData, MeterWeighting};

const SIZE: (u32, u32) = (256, 144);
const PATCH: Vec3 = Vec3::new(0.0, 0.0, -2.0);

fn post(meter: Option<MeterData>) -> PostData {
    PostData {
        exposure: 1.0,
        meter,
        tonemap: TonemapOperator::None,
        bloom_strength: 0.0,
        dither: false,
        ..PostData::default()
    }
}

fn meter(range: [f32; 2]) -> Option<MeterData> {
    Some(MeterData {
        weighting: MeterWeighting::Center,
        clip: [0.02, 0.02],
        range,
    })
}

fn patch(luminance: f32, post: PostData, view: Option<DebugView>) -> RenderScene {
    let mut scene = RenderScene::empty(SIZE);
    scene.camera = origin_camera(SIZE.0, SIZE.1);
    scene.ambient = [0.0; 3];
    scene.meshes = vec![film_box(PATCH, Vec3::new(8.0, 8.0, 0.5), [luminance; 3])];
    scene.post = Some(post);
    scene.debug_view = view;
    scene
}

fn render(renderer: &Renderer, scene: &RenderScene) -> RenderedImage {
    renderer
        .render(scene, FrameSize::native(SIZE.0, SIZE.1))
        .expect("render")
}

/// **The view draws the histogram bar at the patch's bin, with the mean line over it**: a 0.18 patch
/// bins at `(log2 0.18 + 16) / 24` of the width.
#[test]
fn needs_gpu_the_view_draws_the_histogram_and_the_mean_line() {
    let Some(renderer) = skip_or_renderer("the_view_draws_the_histogram_and_the_mean_line") else {
        return;
    };
    let scene = patch(
        0.18,
        post(meter([-8.0, 8.0])),
        Some(DebugView::ExposureMeter),
    );
    let image = render(&renderer, &scene);
    let expected = (0.18_f32.log2() + 16.0) / 24.0 * SIZE.0 as f32;
    let row = SIZE.1 - 2;
    let mut white = Vec::new();
    let mut green = Vec::new();
    for x in 0..SIZE.0 {
        let [r, g, b] = rgb_at(&image, x, row);
        if r > 200 && g > 200 && b > 200 {
            white.push(x as f32);
        } else if g > 200 && r < 60 && b < 60 {
            green.push(x as f32);
        }
    }
    assert!(!white.is_empty(), "a white bar at the patch's bin");
    assert!(!green.is_empty(), "a green line at the mean");
    for x in white.iter().chain(&green) {
        assert!(
            (x - expected).abs() <= 6.0,
            "x {x} is not near the bin at {expected}"
        );
    }
}

/// **A view on a manual scene draws the normal frame**: it never turns the effect on.
#[test]
fn needs_gpu_the_view_on_a_manual_scene_is_the_normal_frame() {
    let Some(renderer) = skip_or_renderer("the_view_on_a_manual_scene_is_the_normal_frame") else {
        return;
    };
    let plain = render(&renderer, &patch(0.18, post(None), None));
    let viewed = render(
        &renderer,
        &patch(0.18, post(None), Some(DebugView::ExposureMeter)),
    );
    assert_eq!(plain.rgba, viewed.rgba);
}

/// **The readback reads the correction and flags a clamp**: a 0.002 patch meters to the `+2 EV`
/// ceiling, a 0.18 patch to none.
#[test]
fn needs_gpu_the_readback_reads_the_correction_and_the_clamp() {
    let Some(renderer) = skip_or_renderer("the_readback_reads_the_correction_and_the_clamp") else {
        return;
    };
    renderer.set_meter_readback(true);
    render(&renderer, &patch(0.002, post(meter([-2.0, 2.0])), None));
    let dark = renderer.collect_meter_readings(true);
    assert_eq!(dark.len(), 1, "one metered frame, one reading");
    assert_eq!(dark[0].correction_ev, 2.0);
    assert!(dark[0].clamped);

    render(&renderer, &patch(0.18, post(meter([-2.0, 2.0])), None));
    let grey = renderer.collect_meter_readings(true);
    assert_eq!(grey.len(), 1);
    assert!(grey[0].correction_ev.abs() < 0.01, "{:?}", grey[0]);
    assert!(!grey[0].clamped);

    // A manual frame files nothing.
    render(&renderer, &patch(0.18, post(None), None));
    assert!(renderer.collect_meter_readings(true).is_empty());
}

/// **Arming the readback never changes the pixels.**
#[test]
fn needs_gpu_an_armed_readback_renders_the_same_bytes() {
    let Some(renderer) = skip_or_renderer("an_armed_readback_renders_the_same_bytes") else {
        return;
    };
    let scene = patch(0.03, post(meter([-3.0, 3.0])), None);
    let unarmed = render(&renderer, &scene);
    renderer.set_meter_readback(true);
    let armed = render(&renderer, &scene);
    assert_eq!(unarmed.rgba, armed.rgba);
    let viewed = patch(
        0.03,
        post(meter([-3.0, 3.0])),
        Some(DebugView::ExposureMeter),
    );
    let armed_view = render(&renderer, &viewed);
    renderer.set_meter_readback(false);
    assert_eq!(armed_view.rgba, render(&renderer, &viewed).rgba);
}
