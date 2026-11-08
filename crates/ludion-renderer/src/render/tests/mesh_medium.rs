//! `MeshInstance.medium`: GPU tests over the `scene_medium` mask and the composite's per-pixel scale.

use super::*;

use crate::scene::PointLightData;

const PX: u32 = 128;
/// Optical depth of the cubes' ray: `0.05 · 30 = 1.5`, so about 78% of a cube's colour is fogged away.
const SIGMA: f32 = 0.05;
/// The cubes' centres in the frame: ±4 units at 30 units out is about ±15 px.
const LEFT: (u32, u32) = (PX / 2 - 15, PX / 2);
const RIGHT: (u32, u32) = (PX / 2 + 15, PX / 2);

fn size() -> FrameSize {
    FrameSize::native(PX, PX)
}

fn fog_post() -> PostData {
    PostData {
        fog_density: SIGMA,
        fog_color: FOG_COLOR,
        ..exact_post()
    }
}

/// Two identical emissive cubes side by side at the same distance, scaling the medium by `left` and
/// `right`, under the medium.
fn twin_scene(left: f32, right: f32) -> RenderScene {
    let mut scene = fog_scene(PX, PX);
    let mut a = slab(Vec3::new(-4.0, 0.0, -30.0), 4.0, SLAB_COLOR);
    let mut b = slab(Vec3::new(4.0, 0.0, -30.0), 4.0, SLAB_COLOR);
    a.medium = left;
    b.medium = right;
    scene.meshes = vec![a, b];
    scene.post = Some(fog_post());
    scene
}

/// The same scene with the medium off: what an exempt mesh must look like.
fn without_medium(scene: &RenderScene) -> RenderScene {
    let mut off = scene.clone();
    off.post = Some(exact_post());
    off
}

fn pixel(frame: &RenderedImage, at: (u32, u32)) -> [u8; 3] {
    let i = ((at.1 * frame.width + at.0) * 4) as usize;
    [frame.rgba[i], frame.rgba[i + 1], frame.rgba[i + 2]]
}

/// The largest channel step between two pixels.
fn gap(a: [u8; 3], b: [u8; 3]) -> i32 {
    (0..3)
        .map(|ch| (i32::from(a[ch]) - i32::from(b[ch])).abs())
        .max()
        .unwrap()
}

fn assert_close(a: [u8; 3], b: [u8; 3], what: &str) {
    for ch in 0..3 {
        assert!(
            (i32::from(a[ch]) - i32::from(b[ch])).abs() <= 1,
            "{what}: {a:?} vs {b:?}"
        );
    }
}

fn render(renderer: &Renderer, scene: &RenderScene) -> RenderedImage {
    renderer.render(scene, size()).expect("render")
}

/// A cube at `medium: 0` shows as if there were no air; its twin at 1 is fogged. Skips with no GPU.
#[test]
fn needs_gpu_an_exempt_cube_renders_as_if_there_were_no_medium() {
    let Some(renderer) = skip_or_renderer("an_exempt_cube_renders_as_if_there_were_no_medium")
    else {
        return;
    };
    let scene = twin_scene(0.0, 1.0);
    assert!(PostChain::for_scene(&scene).mesh_medium);
    let fogged = render(&renderer, &scene);
    let clear = render(&renderer, &without_medium(&scene));
    assert_close(pixel(&fogged, LEFT), pixel(&clear, LEFT), "exempt cube");
    let (fog_twin, clear_twin) = (pixel(&fogged, RIGHT), pixel(&clear, RIGHT));
    assert!(
        gap(fog_twin, clear_twin) > 40,
        "the twin is fogged: {fog_twin:?} vs {clear_twin:?}"
    );
}

/// A half value lands strictly between the fogged and the unfogged pixel. Skips with no GPU.
#[test]
fn needs_gpu_a_half_value_lies_between_fogged_and_clear() {
    let Some(renderer) = skip_or_renderer("a_half_value_lies_between_fogged_and_clear") else {
        return;
    };
    let half = render(&renderer, &twin_scene(0.5, 1.0));
    let full = render(&renderer, &twin_scene(1.0, 1.0));
    let clear = render(&renderer, &without_medium(&twin_scene(1.0, 1.0)));
    let (h, f, c) = (pixel(&half, LEFT), pixel(&full, LEFT), pixel(&clear, LEFT));
    // The slab is red and the fog blue, so red falls and blue rises with the fog.
    assert!(f[0] < h[0] && h[0] < c[0], "red {f:?} < {h:?} < {c:?}");
    assert!(c[2] < h[2] && h[2] < f[2], "blue {c:?} < {h:?} < {f:?}");
}

/// With every mesh at 1 the gate is off and the frame is the plain medium's; the unexempt cube of a
/// gated frame matches it. Skips with no GPU.
#[test]
fn needs_gpu_meshes_at_one_derive_no_mask_and_render_as_before() {
    let Some(renderer) = skip_or_renderer("meshes_at_one_derive_no_mask_and_render_as_before")
    else {
        return;
    };
    let plain = twin_scene(1.0, 1.0);
    assert!(!PostChain::for_scene(&plain).mesh_medium);
    let plain_frame = render(&renderer, &plain);
    assert_eq!(
        plain_frame.rgba,
        render(&renderer, &plain).rgba,
        "deterministic"
    );
    let gated = render(&renderer, &twin_scene(0.0, 1.0));
    assert_close(
        pixel(&gated, RIGHT),
        pixel(&plain_frame, RIGHT),
        "the unexempt cube of a gated frame",
    );
}

/// An exempt cube takes no in-scatter from a scattering light in the air. Skips with no GPU.
#[test]
fn needs_gpu_an_exempt_cube_takes_no_local_in_scatter() {
    let Some(renderer) = skip_or_renderer("an_exempt_cube_takes_no_local_in_scatter") else {
        return;
    };
    let mut scene = twin_scene(0.0, 1.0);
    scene.lights.push(PointLightData {
        position: Vec3::new(0.0, 0.0, -10.0),
        color: Vec3::ONE,
        intensity: 4.0,
        range: 40.0,
        dynamic: false,
        shadow: None,
        scatter: 1.0,
    });
    assert!(PostChain::for_scene(&scene).local_scatter);
    let scattered = render(&renderer, &scene);
    let clear = render(&renderer, &without_medium(&scene));
    assert_close(
        pixel(&scattered, LEFT),
        pixel(&clear, LEFT),
        "exempt cube under a scattering light",
    );
    let (twin, clear_twin) = (pixel(&scattered, RIGHT), pixel(&clear, RIGHT));
    assert!(
        gap(twin, clear_twin) > 40,
        "the twin takes the fog and glow: {twin:?} vs {clear_twin:?}"
    );
}

/// An exempt cube takes no sun shafts either. Skips with no GPU.
#[test]
fn needs_gpu_an_exempt_cube_takes_no_shafts() {
    let Some(renderer) = skip_or_renderer("an_exempt_cube_takes_no_shafts") else {
        return;
    };
    let mut scene = twin_scene(0.0, 1.0);
    scene.sun = Some(test_sun(
        Vec3::new(0.0, -0.3, 1.0).normalize(),
        Vec3::ONE,
        0.002,
    ));
    scene.post = Some(PostData {
        shafts: 0.17,
        ..fog_post()
    });
    assert!(PostChain::for_scene(&scene).shafts);
    let shafted = render(&renderer, &scene);
    let clear = render(&renderer, &without_medium(&scene));
    assert_close(
        pixel(&shafted, LEFT),
        pixel(&clear, LEFT),
        "exempt cube under shafts",
    );
}
