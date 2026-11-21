//! The film lens: the authorable halation colour and the anamorphic streak chain.

use super::*;

const SOURCE: Vec3 = Vec3::new(0.0, 0.0, -10.0);
const RING: Vec3 = Vec3::new(0.74, 0.0, -10.0);

/// One small super-threshold emissive cube dead centre in a dark frame: the streak's row is the
/// frame's middle row, and everything off that row is the radial bloom field alone.
fn streak_scene(w: u32, h: u32) -> RenderScene {
    let mut scene = RenderScene::empty((w, h));
    scene.camera = origin_camera(w, h);
    scene.ambient = [0.0, 0.0, 0.0];
    scene.meshes = vec![film_box(SOURCE, Vec3::splat(1.0), [6.0, 6.0, 6.0])];
    scene
}

/// The imaging chain out of the way: no bloom mix, no tonemap, so a probe's value is the streak's.
fn base() -> PostData {
    PostData {
        exposure: 1.0,
        tonemap: TonemapOperator::None,
        bloom_strength: 0.0,
        dither: false,
        ..PostData::default()
    }
}

fn render(
    renderer: &Renderer,
    scene: &mut RenderScene,
    post: PostData,
    size: FrameSize,
) -> RenderedImage {
    scene.post = Some(post);
    renderer.render(scene, size).expect("render")
}

fn steps(a: [u8; 3], b: [u8; 3]) -> u32 {
    (0..3)
        .map(|c| u32::from(a[c]).abs_diff(u32::from(b[c])))
        .max()
        .unwrap()
}

/// **The streak is horizontal only.** A probe far left and right on the source's row brightens by
/// many steps; a probe the same distance above and below does not move.
#[test]
fn needs_gpu_anamorphic_streaks_horizontally_and_not_vertically() {
    let Some(renderer) = skip_or_renderer("anamorphic_streaks_horizontally") else {
        return;
    };
    let (w, h) = (256u32, 144u32);
    let size = FrameSize::native(w, h);
    let mut scene = streak_scene(w, h);
    let off = render(&renderer, &mut scene, base(), size);
    let on = render(
        &renderer,
        &mut scene,
        PostData {
            anamorphic: 0.5,
            ..base()
        },
        size,
    );
    let (cx, cy) = (w / 2, h / 2);
    let d = 40;
    for x in [cx - d, cx + d] {
        let (was, now) = (rgb_at(&off, x, cy), rgb_at(&on, x, cy));
        assert!(
            steps(was, now) >= 8,
            "the streak must light the source's row at {x}: {was:?} -> {now:?}"
        );
    }
    for y in [cy - d, cy + d] {
        let (was, now) = (rgb_at(&off, cx, y), rgb_at(&on, cx, y));
        assert!(
            steps(was, now) <= 1,
            "the streak must not leak vertically at {y}: {was:?} -> {now:?}"
        );
    }
}

/// A longer streak lights a farther probe; the threshold above the source kills it.
#[test]
fn needs_gpu_anamorphic_length_reaches_further_and_a_high_threshold_kills_it() {
    let Some(renderer) = skip_or_renderer("anamorphic_length_and_threshold") else {
        return;
    };
    let (w, h) = (256u32, 144u32);
    let size = FrameSize::native(w, h);
    let mut scene = streak_scene(w, h);
    let off = render(&renderer, &mut scene, base(), size);
    let streak = |length: f32, threshold: Option<f32>| PostData {
        anamorphic: 0.5,
        anamorphic_length: length,
        anamorphic_threshold: threshold,
        ..base()
    };
    let short = render(&renderer, &mut scene, streak(0.0, None), size);
    let long = render(&renderer, &mut scene, streak(1.0, None), size);
    let killed = render(&renderer, &mut scene, streak(1.0, Some(1000.0)), size);
    let (x, y) = (w / 2 + 100, h / 2);
    let base_px = rgb_at(&off, x, y);
    assert!(
        steps(rgb_at(&long, x, y), base_px) > steps(rgb_at(&short, x, y), base_px) + 2,
        "length 1 must light a farther probe than length 0: {:?} vs {:?} over {base_px:?}",
        rgb_at(&long, x, y),
        rgb_at(&short, x, y),
    );
    assert_eq!(
        killed.rgba, off.rgba,
        "a threshold far above the source must leave no streak at all"
    );
}

/// Prints the visible half-length at the default length, as a fraction of the frame width. Measured
/// where the streak still lifts the source's row by two output steps over the no-streak frame.
#[test]
fn needs_gpu_anamorphic_default_half_length_is_measured() {
    let Some(renderer) = skip_or_renderer("anamorphic_default_half_length") else {
        return;
    };
    let (w, h) = (512u32, 288u32);
    let size = FrameSize::native(w, h);
    let mut scene = streak_scene(w, h);
    let off = render(&renderer, &mut scene, base(), size);
    let on = render(
        &renderer,
        &mut scene,
        PostData {
            anamorphic: 0.5,
            ..base()
        },
        size,
    );
    let cy = h / 2;
    let reach = (w / 2..w)
        .rev()
        .find(|&x| steps(rgb_at(&off, x, cy), rgb_at(&on, x, cy)) >= 2)
        .map_or(0, |x| x - w / 2);
    eprintln!(
        "anamorphic default half-length: {reach} px = {:.3} of width",
        reach as f32 / w as f32
    );
    // The docs state about 0.3 of the width; a change that moves it far must update them.
    assert!(
        (0.2..0.4).contains(&(reach as f32 / w as f32)),
        "the default streak's half-length moved: {reach} px"
    );
}

/// **A gate at zero is a gate.** With `anamorphic: 0` every other new field at a non-default value
/// still renders the absent frame byte for byte, and an explicit `halation_color: None` is the
/// absent colour.
#[test]
fn needs_gpu_a_zero_anamorphic_with_other_fields_set_is_byte_identical_to_absent() {
    let Some(renderer) = skip_or_renderer("zero_anamorphic_byte_identical") else {
        return;
    };
    let (w, h) = (192u32, 108u32);
    let size = FrameSize::native(w, h);
    let mut scene = streak_scene(w, h);
    let absent = render(&renderer, &mut scene, base(), size);
    let set = render(
        &renderer,
        &mut scene,
        PostData {
            anamorphic: 0.0,
            anamorphic_length: 0.9,
            anamorphic_tint: [1.0, 0.0, 0.0],
            anamorphic_threshold: Some(0.5),
            halation_color: Some([0.0, 0.0, 1.0]),
            ..base()
        },
        size,
    );
    assert_eq!(set.rgba, absent.rgba);

    let with_halation = PostData {
        halation: 0.15,
        ..base()
    };
    let a = render(&renderer, &mut scene, with_halation, size);
    let b = render(
        &renderer,
        &mut scene,
        PostData {
            halation_color: None,
            ..with_halation
        },
        size,
    );
    assert_eq!(a.rgba, b.rgba);
}

/// **A blue halation colour makes the ring bluer than red**, the reverse of the stock's warm bleed.
#[test]
fn needs_gpu_a_blue_halation_color_bleeds_blue() {
    let Some(renderer) = skip_or_renderer("blue_halation_color") else {
        return;
    };
    let (w, h) = (192u32, 192u32);
    let size = FrameSize::native(w, h);
    let mut scene = streak_scene(w, h);
    let ring = project(&scene.camera, RING, w, h);
    let dark = rgb_at(&render(&renderer, &mut scene, base(), size), ring.0, ring.1);
    let halated = render(
        &renderer,
        &mut scene,
        PostData {
            halation: 0.15,
            halation_color: Some([0.0, 0.0, 1.0]),
            ..base()
        },
        size,
    );
    let px = rgb_at(&halated, ring.0, ring.1);
    let (red_gain, blue_gain) = (
        u32::from(px[0]).saturating_sub(u32::from(dark[0])),
        u32::from(px[2]).saturating_sub(u32::from(dark[2])),
    );
    assert!(
        blue_gain > red_gain * 2 && blue_gain > 8,
        "a blue halation colour must bleed blue: red +{red_gain} against blue +{blue_gain}"
    );
}
