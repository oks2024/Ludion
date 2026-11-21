//! Metered exposure (`exposure_meter`, composite step 5): the correction puts the trimmed mean log2
//! luminance on mid-grey, clamps to `exposure_range`, and `exposure_compensation` offsets both modes.

use super::*;
use crate::scene::{MeterData, MeterWeighting};

const PATCH: Vec3 = Vec3::new(0.0, 0.0, -2.0);
const WIDE: [f32; 2] = [-8.0, 8.0];

/// The imaging chain out of the way (no bloom, no tonemap, no dither), `exposure: 1.0`, so a pixel is
/// the linear colour times the composite's exposure, encoded.
fn post(meter: Option<MeterData>, exposure: f32, compensation: f32) -> PostData {
    PostData {
        exposure,
        exposure_compensation: compensation,
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

/// A scene of flat emissive slabs filling the 64x36 frame, one per `(x centre, colour)`, 4 units wide.
fn slab_scene(slabs: &[(f32, [f32; 3])], post: PostData) -> RenderScene {
    let mut scene = RenderScene::empty((64, 36));
    scene.camera = origin_camera(64, 36);
    scene.ambient = [0.0; 3];
    scene.meshes = slabs
        .iter()
        .map(|&(x, color)| film_box(Vec3::new(x, 0.0, PATCH.z), Vec3::new(4.0, 8.0, 0.5), color))
        .collect();
    scene.post = Some(post);
    scene
}

fn patch(luminance: f32, post: PostData) -> RenderScene {
    let mut scene = RenderScene::empty((64, 36));
    scene.camera = origin_camera(64, 36);
    scene.ambient = [0.0; 3];
    scene.meshes = vec![film_box(PATCH, Vec3::new(8.0, 8.0, 0.5), [luminance; 3])];
    scene.post = Some(post);
    scene
}

fn centre(renderer: &Renderer, scene: &RenderScene) -> [u8; 3] {
    let image = renderer
        .render(scene, FrameSize::native(64, 36))
        .expect("render");
    rgb_at(&image, image.width / 2, image.height / 2)
}

fn within(a: [u8; 3], b: [u8; 3], steps: u8) -> bool {
    (0..3).all(|c| a[c].abs_diff(b[c]) <= steps)
}

/// **A frame already at mid-grey meters to a correction of zero**: the centre pixel matches manual.
#[test]
fn needs_gpu_a_mid_grey_patch_meters_to_no_correction() {
    let Some(renderer) = skip_or_renderer("a_mid_grey_patch_meters_to_no_correction") else {
        return;
    };
    let manual = centre(&renderer, &patch(0.18, post(None, 1.0, 0.0)));
    let metered = centre(&renderer, &patch(0.18, post(meter(WIDE), 1.0, 0.0)));
    assert!(within(manual, metered, 1), "{manual:?} vs {metered:?}");
}

/// **The correction clamps to `exposure_range`**: a dark patch is lifted by exactly +2 EV (x4), a
/// bright one lowered by exactly -2 EV (x0.25).
#[test]
fn needs_gpu_the_correction_clamps_to_the_range() {
    let Some(renderer) = skip_or_renderer("the_correction_clamps_to_the_range") else {
        return;
    };
    let dark = centre(&renderer, &patch(0.002, post(meter([-2.0, 2.0]), 1.0, 0.0)));
    let dark_manual = centre(&renderer, &patch(0.002, post(None, 4.0, 0.0)));
    assert!(within(dark, dark_manual, 1), "{dark:?} vs {dark_manual:?}");
    let unmetered = centre(&renderer, &patch(0.002, post(None, 1.0, 0.0)));
    assert!(dark[0] > unmetered[0], "the dark patch is lifted");

    let bright = centre(&renderer, &patch(20.0, post(meter([-2.0, 2.0]), 1.0, 0.0)));
    let bright_manual = centre(&renderer, &patch(20.0, post(None, 0.25, 0.0)));
    assert!(
        within(bright, bright_manual, 1),
        "{bright:?} vs {bright_manual:?}"
    );
}

/// **A range of `[0, 0]` leaves nothing to meter**: metered is manual, byte for byte.
#[test]
fn needs_gpu_a_zero_range_is_identical_to_manual() {
    let Some(renderer) = skip_or_renderer("a_zero_range_is_identical_to_manual") else {
        return;
    };
    let size = FrameSize::native(64, 36);
    let manual = renderer
        .render(&patch(0.05, post(None, 1.3, 0.0)), size)
        .expect("manual");
    let metered = renderer
        .render(&patch(0.05, post(meter([0.0, 0.0]), 1.3, 0.0)), size)
        .expect("metered");
    assert_eq!(manual.rgba, metered.rgba);
}

/// **Compensation is `exposure * 2^EV` in both modes**: `+1` manual is `exposure: 2.0` byte for byte,
/// and on a metered frame it stacks on the correction.
#[test]
fn needs_gpu_compensation_scales_exposure_and_stacks_on_the_meter() {
    let Some(renderer) = skip_or_renderer("compensation_scales_exposure_and_stacks") else {
        return;
    };
    let size = FrameSize::native(64, 36);
    let plus_one = renderer
        .render(&patch(0.05, post(None, 1.0, 1.0)), size)
        .expect("compensated");
    let doubled = renderer
        .render(&patch(0.05, post(None, 2.0, 0.0)), size)
        .expect("doubled");
    assert_eq!(plus_one.rgba, doubled.rgba);

    // A 0.002 patch meters to the +2 EV ceiling; +1 compensation on top is x8 in all.
    let stacked = centre(&renderer, &patch(0.002, post(meter([-2.0, 2.0]), 1.0, 1.0)));
    let manual = centre(&renderer, &patch(0.002, post(None, 8.0, 0.0)));
    assert!(within(stacked, manual, 1), "{stacked:?} vs {manual:?}");
}

/// **The meter is deterministic**: one process renders a gradient scene to the same bytes twice,
/// and metering changes the image.
#[test]
fn needs_gpu_a_metered_gradient_renders_the_same_twice() {
    let Some(renderer) = skip_or_renderer("a_metered_gradient_renders_the_same_twice") else {
        return;
    };
    let size = FrameSize::native(64, 36);
    let slabs = [
        (-6.0, [0.005; 3]),
        (-2.0, [0.05; 3]),
        (2.0, [0.5; 3]),
        (6.0, [3.0; 3]),
    ];
    let metered = slab_scene(&slabs, post(meter(WIDE), 1.0, 0.0));
    let first = renderer.render(&metered, size).expect("first");
    let second = renderer.render(&metered, size).expect("second");
    assert_eq!(first.rgba, second.rgba);
    let manual = renderer
        .render(&slab_scene(&slabs, post(None, 1.0, 0.0)), size)
        .expect("manual");
    assert_ne!(first.rgba, manual.rgba, "the meter moved the image");
}

/// **Sky weighs a quarter of a surface**: the same two luminances, but with the bright half drawn
/// as sky rather than as a surface, pull the mean up less, so the dark half is lifted more.
#[test]
fn needs_gpu_sky_pulls_the_metered_value_less_than_a_surface() {
    let Some(renderer) = skip_or_renderer("sky_pulls_the_metered_value_less") else {
        return;
    };
    let dark = [0.01; 3];
    let bright = [1.0; 3];
    let mut over_sky = slab_scene(&[(-2.0, dark)], post(meter(WIDE), 1.0, 0.0));
    over_sky.environment = Some(crate::scene::EnvironmentData {
        sky_zenith: bright,
        sky_horizon: bright,
        sky_ground: bright,
        sun_disc_size: 0.0,
        sun_disc_intensity: 0.0,
        ambient: [0.0; 3],
        ibl_intensity: 0.0,
    });
    let over_surface = slab_scene(&[(-2.0, dark), (2.0, bright)], post(meter(WIDE), 1.0, 0.0));
    let size = FrameSize::native(64, 36);
    let left = |scene: &RenderScene| {
        let image = renderer.render(scene, size).expect("render");
        rgb_at(&image, 8, 18)
    };
    let (sky, surface) = (left(&over_sky), left(&over_surface));
    assert!(
        sky[0] > surface[0],
        "the dark half is lifted more beside sky ({sky:?}) than beside a surface ({surface:?})"
    );
}
