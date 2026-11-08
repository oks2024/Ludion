//! `SpriteInstance.medium` / `ParticleInstance.medium`: GPU tests over the transparent layer's blue
//! channel and the composite's per-pixel scale on it.

use super::*;
use crate::scene::PointLightData;

const PX: u32 = 128;
/// Optical depth of the quads' ray: `0.05 · 30 = 1.5`, so about 78% of a quad's colour is fogged away.
const SIGMA: f32 = 0.05;
const LEFT: (u32, u32) = (PX / 2 - 15, PX / 2);
const RIGHT: (u32, u32) = (PX / 2 + 15, PX / 2);
const OPAQUE: [f32; 4] = [SLAB_COLOR[0], SLAB_COLOR[1], SLAB_COLOR[2], 1.0];

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

/// Two identical opaque sprites side by side at the same distance, scaling the medium by `left` and
/// `right`, under the medium.
fn sprite_twins(left: f32, right: f32) -> RenderScene {
    let mut scene = fog_scene(PX, PX);
    let mut a = sized_sprite(Vec3::new(-4.0, 0.0, -30.0), 4.0, OPAQUE, false);
    let mut b = sized_sprite(Vec3::new(4.0, 0.0, -30.0), 4.0, OPAQUE, false);
    a.medium = left;
    b.medium = right;
    scene.meshes.clear();
    scene.sprites = vec![a, b];
    scene.post = Some(fog_post());
    scene
}

/// The same for two opaque particle discs.
fn particle_twins(left: f32, right: f32) -> RenderScene {
    let mut scene = fog_scene(PX, PX);
    let mut a = particle(Vec3::new(-4.0, 0.0, -30.0), 4.0, OPAQUE, 0.0);
    let mut b = particle(Vec3::new(4.0, 0.0, -30.0), 4.0, OPAQUE, 0.0);
    a.medium = left;
    b.medium = right;
    scene.meshes.clear();
    scene.particles = vec![a, b];
    scene.post = Some(fog_post());
    scene
}

/// The same scene with the medium off: what an exempt quad must look like.
fn without_medium(scene: &RenderScene) -> RenderScene {
    let mut off = scene.clone();
    off.post = Some(exact_post());
    off
}

fn pixel(frame: &RenderedImage, at: (u32, u32)) -> [u8; 3] {
    let i = ((at.1 * frame.width + at.0) * 4) as usize;
    [frame.rgba[i], frame.rgba[i + 1], frame.rgba[i + 2]]
}

fn gap(a: [u8; 3], b: [u8; 3]) -> i32 {
    (0..3)
        .map(|ch| (i32::from(a[ch]) - i32::from(b[ch])).abs())
        .max()
        .unwrap()
}

fn assert_close(a: [u8; 3], b: [u8; 3], what: &str) {
    assert!(gap(a, b) <= 1, "{what}: {a:?} vs {b:?}");
}

fn render(renderer: &Renderer, scene: &RenderScene) -> RenderedImage {
    renderer.render(scene, size()).expect("render")
}

/// Whichever transparent kind `scene` holds, an exempt quad renders as if there were no medium and
/// its twin is fogged; a half value lands strictly between. Skips with no GPU.
fn exempt_and_half(renderer: &Renderer, twins: fn(f32, f32) -> RenderScene) {
    let scene = twins(0.0, 1.0);
    assert!(PostChain::for_scene(&scene).transparent_medium_scheduled());
    let fogged = render(renderer, &scene);
    let clear = render(renderer, &without_medium(&scene));
    assert_close(pixel(&fogged, LEFT), pixel(&clear, LEFT), "exempt quad");
    let (fog_twin, clear_twin) = (pixel(&fogged, RIGHT), pixel(&clear, RIGHT));
    assert!(
        gap(fog_twin, clear_twin) > 40,
        "the twin is fogged: {fog_twin:?} vs {clear_twin:?}"
    );

    let half = render(renderer, &twins(0.5, 1.0));
    let (h, f, c) = (
        pixel(&half, LEFT),
        pixel(&fogged, RIGHT),
        pixel(&clear, RIGHT),
    );
    // The quad is red and the fog blue, so red falls and blue rises with the fog.
    assert!(f[0] < h[0] && h[0] < c[0], "red {f:?} < {h:?} < {c:?}");
    assert!(c[2] < h[2] && h[2] < f[2], "blue {c:?} < {h:?} < {f:?}");
}

/// A sprite at `medium: 0` shows as if there were no air; at 0.5 it lies between. Skips with no GPU.
#[test]
fn needs_gpu_an_exempt_sprite_renders_as_if_there_were_no_medium() {
    let Some(renderer) = skip_or_renderer("an_exempt_sprite_renders_as_if_there_were_no_medium")
    else {
        return;
    };
    exempt_and_half(&renderer, sprite_twins);
}

/// The same for a particle. Skips with no GPU.
#[test]
fn needs_gpu_an_exempt_particle_renders_as_if_there_were_no_medium() {
    let Some(renderer) = skip_or_renderer("an_exempt_particle_renders_as_if_there_were_no_medium")
    else {
        return;
    };
    exempt_and_half(&renderer, particle_twins);
}

/// With every transparent at 1 the gate is off and the layer stays two-channel; the unexempt quad of a
/// gated frame matches the plain frame. Skips with no GPU.
#[test]
fn needs_gpu_quads_at_one_derive_no_blue_channel_and_render_as_before() {
    let Some(renderer) =
        skip_or_renderer("quads_at_one_derive_no_blue_channel_and_render_as_before")
    else {
        return;
    };
    let plain = sprite_twins(1.0, 1.0);
    assert!(!PostChain::for_scene(&plain).transparent_medium);
    let plain_frame = render(&renderer, &plain);
    assert_eq!(
        plain_frame.rgba,
        render(&renderer, &plain).rgba,
        "deterministic"
    );
    let gated = render(&renderer, &sprite_twins(0.0, 1.0));
    assert_close(
        pixel(&gated, RIGHT),
        pixel(&plain_frame, RIGHT),
        "the unexempt sprite of a gated frame",
    );
}

/// An exempt sprite in front of a fogged mesh at half coverage lands between the fully fogged and
/// the fog-free renders. Skips with no GPU.
#[test]
fn needs_gpu_an_exempt_half_covering_sprite_over_a_fogged_mesh_lands_between() {
    let Some(renderer) = skip_or_renderer("an_exempt_half_covering_sprite_over_a_fogged_mesh")
    else {
        return;
    };
    let scene_of = |medium: f32| {
        let mut scene = fog_scene(PX, PX);
        scene.meshes = vec![slab(Vec3::new(0.0, 0.0, -40.0), 24.0, SLAB_COLOR)];
        let mut sprite = sized_sprite(
            Vec3::new(0.0, 0.0, -20.0),
            24.0,
            [FOG_COLOR[0], FOG_COLOR[1], FOG_COLOR[2], 0.5],
            false,
        );
        sprite.medium = medium;
        scene.sprites = vec![sprite];
        scene.post = Some(fog_post());
        scene
    };
    let exempt = scene_of(0.0);
    assert!(PostChain::for_scene(&exempt).transparent_medium_scheduled());
    let centre = (PX / 2, PX / 2);
    let exempt_px = pixel(&render(&renderer, &exempt), centre);
    let fogged_px = pixel(&render(&renderer, &scene_of(1.0)), centre);
    let clear_px = pixel(&render(&renderer, &without_medium(&exempt)), centre);
    assert_ne!(
        fogged_px, clear_px,
        "the fixture separates fogged from clear"
    );
    let between = |ch: usize| {
        let (lo, hi) = (
            fogged_px[ch].min(clear_px[ch]),
            fogged_px[ch].max(clear_px[ch]),
        );
        lo <= exempt_px[ch] && exempt_px[ch] <= hi
    };
    assert!(
        (0..3).all(between) && gap(exempt_px, fogged_px) > 2,
        "exempt {exempt_px:?} between fogged {fogged_px:?} and clear {clear_px:?}"
    );
}

/// An exempt sprite on a frame where no mesh is below 1 takes no in-scatter from a scattering light.
/// Skips with no GPU.
#[test]
fn needs_gpu_an_exempt_sprite_takes_no_local_in_scatter() {
    let Some(renderer) = skip_or_renderer("an_exempt_sprite_takes_no_local_in_scatter") else {
        return;
    };
    let mut scene = sprite_twins(0.0, 1.0);
    scene.lights.push(PointLightData {
        position: Vec3::new(0.0, 0.0, -10.0),
        color: Vec3::ONE,
        intensity: 4.0,
        range: 40.0,
        dynamic: false,
        shadow: None,
        scatter: 1.0,
    });
    let chain = PostChain::for_scene(&scene);
    assert!(chain.local_scatter && !chain.mesh_medium_scheduled());
    let scattered = render(&renderer, &scene);
    let clear = render(&renderer, &without_medium(&scene));
    assert_close(
        pixel(&scattered, LEFT),
        pixel(&clear, LEFT),
        "exempt sprite under a scattering light",
    );
    let (twin, clear_twin) = (pixel(&scattered, RIGHT), pixel(&clear, RIGHT));
    assert!(
        gap(twin, clear_twin) > 40,
        "the twin takes the fog and glow: {twin:?} vs {clear_twin:?}"
    );
}

/// An exempt sprite on a frame where no mesh is below 1 takes no sun shafts. Skips with no GPU.
#[test]
fn needs_gpu_an_exempt_sprite_takes_no_shafts() {
    let Some(renderer) = skip_or_renderer("an_exempt_sprite_takes_no_shafts") else {
        return;
    };
    let mut scene = sprite_twins(0.0, 1.0);
    scene.sun = Some(test_sun(
        Vec3::new(0.0, -0.3, 1.0).normalize(),
        Vec3::ONE,
        0.002,
    ));
    scene.post = Some(PostData {
        shafts: 0.17,
        ..fog_post()
    });
    let chain = PostChain::for_scene(&scene);
    assert!(chain.shafts && !chain.mesh_medium_scheduled());
    let shafted = render(&renderer, &scene);
    let clear = render(&renderer, &without_medium(&scene));
    assert_close(
        pixel(&shafted, LEFT),
        pixel(&clear, LEFT),
        "exempt sprite under shafts",
    );
    let (twin, clear_twin) = (pixel(&shafted, RIGHT), pixel(&clear, RIGHT));
    assert!(
        gap(twin, clear_twin) > 40,
        "the twin is fogged: {twin:?} vs {clear_twin:?}"
    );
}
