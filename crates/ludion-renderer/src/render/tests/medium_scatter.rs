//! Local-light in-scatter: GPU tests over the `medium_scatter` pass and its composite add.

use super::*;
use crate::scene::{PointLightData, SpotLightData};

/// The frame edge of the fixtures.
const PX: u32 = 128;
/// The medium's extinction: `3 / σ_t` reaches sixty units, past every light here.
const SIGMA: f32 = 0.05;

fn size() -> FrameSize {
    FrameSize::native(PX, PX)
}

/// An empty frame in a black medium under the exact-output post: no meshes, so the glow is the only
/// light the air carries beyond the (identical) ambient the baseline shares.
fn scatter_scene() -> RenderScene {
    let mut scene = fog_scene(PX, PX);
    scene.meshes.clear();
    scene.post = Some(PostData {
        fog_density: SIGMA,
        fog_color: [0.0; 3],
        ..exact_post()
    });
    scene
}

fn point(position: Vec3, color: Vec3, range: f32, scatter: f32) -> PointLightData {
    PointLightData {
        position,
        color,
        intensity: 4.0,
        range,
        dynamic: false,
        shadow: None,
        scatter,
    }
}

/// The same scene with every light's `scatter` zeroed: it lights surfaces identically and builds
/// no scatter pass.
fn without_scatter(scene: &RenderScene) -> RenderScene {
    let mut off = scene.clone();
    off.lights.iter_mut().for_each(|l| l.scatter = 0.0);
    off.spot_lights.iter_mut().for_each(|l| l.scatter = 0.0);
    off
}

fn pixel(renderer: &Renderer, scene: &RenderScene, x: u32, y: u32) -> [u8; 3] {
    let frame = renderer.render(scene, size()).expect("render");
    let i = ((y * frame.width + x) * 4) as usize;
    [frame.rgba[i], frame.rgba[i + 1], frame.rgba[i + 2]]
}

fn sum(px: [u8; 3]) -> i32 {
    px.iter().map(|&c| i32::from(c)).sum()
}

const CENTRE: (u32, u32) = (PX / 2, PX / 2);

/// A scattering point light in front of the camera lights the air around it. Skips with no GPU.
#[test]
fn needs_gpu_a_scattering_point_light_glows_in_the_air() {
    let Some(renderer) = skip_or_renderer("a_scattering_point_light_glows_in_the_air") else {
        return;
    };
    let mut scene = scatter_scene();
    scene.lights = vec![point(Vec3::new(0.0, 0.0, -6.0), Vec3::ONE, 10.0, 1.0)];
    let glow = sum(pixel(&renderer, &scene, CENTRE.0, CENTRE.1));
    let none = sum(pixel(
        &renderer,
        &without_scatter(&scene),
        CENTRE.0,
        CENTRE.1,
    ));
    eprintln!("glow {glow}, none {none}");
    assert!(glow > none + 30, "glow {glow} against baseline {none}");
}

/// A light outside the frustum is not selected, so the frame is the one without a scattering light.
/// Skips with no GPU.
#[test]
fn needs_gpu_an_off_frustum_scattering_light_renders_byte_identically_to_none() {
    let Some(renderer) =
        skip_or_renderer("an_off_frustum_scattering_light_renders_byte_identically_to_none")
    else {
        return;
    };
    let mut scene = scatter_scene();
    scene.lights = vec![point(Vec3::new(0.0, 0.0, 30.0), Vec3::ONE, 10.0, 1.0)];
    let on = renderer.render(&scene, size()).expect("on");
    let off = renderer
        .render(&without_scatter(&scene), size())
        .expect("off");
    assert_eq!(on.rgba, off.rgba);
}

/// The glow carries the light's colour. Skips with no GPU.
#[test]
fn needs_gpu_the_glow_takes_the_light_colour() {
    let Some(renderer) = skip_or_renderer("the_glow_takes_the_light_colour") else {
        return;
    };
    let mut scene = scatter_scene();
    scene.lights = vec![point(Vec3::new(0.0, 0.0, -6.0), Vec3::X, 10.0, 1.0)];
    let glow = pixel(&renderer, &scene, CENTRE.0, CENTRE.1);
    let none = pixel(&renderer, &without_scatter(&scene), CENTRE.0, CENTRE.1);
    let gain = |c: usize| i32::from(glow[c]) - i32::from(none[c]);
    eprintln!("gain r {} g {} b {}", gain(0), gain(1), gain(2));
    assert!(gain(0) > 30, "red gain {}", gain(0));
    assert!(
        gain(1) <= 1 && gain(2) <= 1,
        "green {} blue {}",
        gain(1),
        gain(2)
    );
}

/// A wall in front of the light's whole sphere hides the glow behind it. Skips with no GPU.
#[test]
fn needs_gpu_a_wall_in_front_hides_the_glow_behind_it() {
    let Some(renderer) = skip_or_renderer("a_wall_in_front_hides_the_glow_behind_it") else {
        return;
    };
    let mut scene = scatter_scene();
    let mut wall = slab(Vec3::new(0.0, 0.0, -3.0 - 15.0), 30.0, [0.0; 3]);
    wall.cast_shadows = false;
    scene.meshes.push(wall);
    scene.lights = vec![point(Vec3::new(0.0, 0.0, -12.0), Vec3::ONE, 4.0, 1.0)];
    let on = renderer.render(&scene, size()).expect("on");
    let off = renderer
        .render(&without_scatter(&scene), size())
        .expect("off");
    assert_eq!(
        on.rgba, off.rgba,
        "the wall's frame is the no-scatter frame"
    );
}

/// A camera inside the light's sphere is lit by the air around it: the gun-light case. Skips with no GPU.
#[test]
fn needs_gpu_a_light_around_the_camera_scatters() {
    let Some(renderer) = skip_or_renderer("a_light_around_the_camera_scatters") else {
        return;
    };
    let mut scene = scatter_scene();
    // Just ahead of the lens, where the eye ray still forward-scatters; bright, since the stretch inside
    // the sphere before the light is short.
    scene.lights = vec![point(
        Vec3::new(0.0, 0.0, -1.0),
        Vec3::splat(30.0),
        6.0,
        1.0,
    )];
    let glow = sum(pixel(&renderer, &scene, CENTRE.0, CENTRE.1));
    let none = sum(pixel(
        &renderer,
        &without_scatter(&scene),
        CENTRE.0,
        CENTRE.1,
    ));
    eprintln!("glow {glow}, none {none}");
    assert!(glow > none + 30, "glow {glow} against baseline {none}");
}

/// A spot scatters inside its cone and nowhere else in its bounding sphere. Row 22 crosses the plane
/// `z = -4` at `y ≈ 1.5`, inside the sphere: column 64 there runs along the axis, column 119 (`x ≈ 2`)
/// is 53° off it, outside the 40° cone at every point of the ray. Skips with no GPU.
#[test]
fn needs_gpu_a_spot_scatters_only_inside_its_cone() {
    let Some(renderer) = skip_or_renderer("a_spot_scatters_only_inside_its_cone") else {
        return;
    };
    let mut scene = scatter_scene();
    scene.spot_lights = vec![SpotLightData {
        position: Vec3::new(0.0, 0.0, -4.0),
        direction: Vec3::Y,
        color: Vec3::splat(20.0),
        intensity: 4.0,
        range: 6.0,
        cos_inner: 30.0_f32.to_radians().cos(),
        cos_outer: 40.0_f32.to_radians().cos(),
        shadow: None,
        scatter: 1.0,
    }];
    let off = without_scatter(&scene);
    let outside = renderer.render(&scene, size()).expect("on");
    let baseline = renderer.render(&off, size()).expect("off");
    let at = |frame: &RenderedImage, x: u32, y: u32| {
        let i = ((y * frame.width + x) * 4) as usize;
        sum([frame.rgba[i], frame.rgba[i + 1], frame.rgba[i + 2]])
    };
    assert_eq!(
        at(&outside, 119, 22),
        at(&baseline, 119, 22),
        "a ray inside the sphere but outside the cone gathers nothing"
    );
    let inside = at(&outside, CENTRE.0, 22);
    let none = at(&baseline, CENTRE.0, 22);
    eprintln!("inside {inside}, none {none}");
    assert!(
        inside > none + 20,
        "inside the cone {inside} against {none}"
    );
}

/// An opaque sprite in front of the light's whole sphere ends the scatter. Skips with no GPU.
#[test]
fn needs_gpu_an_opaque_sprite_in_front_ends_the_scatter() {
    let Some(renderer) = skip_or_renderer("an_opaque_sprite_in_front_ends_the_scatter") else {
        return;
    };
    let mut scene = scatter_scene();
    scene.lights = vec![point(Vec3::new(0.0, 0.0, -10.0), Vec3::ONE, 5.0, 1.0)];
    let bare = sum(pixel(&renderer, &scene, CENTRE.0, CENTRE.1));
    let none = sum(pixel(
        &renderer,
        &without_scatter(&scene),
        CENTRE.0,
        CENTRE.1,
    ));
    assert!(
        bare > none + 30,
        "the light glows without the sprite: {bare} against {none}"
    );
    scene.sprites = vec![sized_sprite(
        Vec3::new(0.0, 0.0, -3.0),
        3.0,
        OPAQUE_SLAB,
        false,
    )];
    let covered = renderer.render(&scene, size()).expect("covered");
    let off = renderer
        .render(&without_scatter(&scene), size())
        .expect("off");
    let i = ((CENTRE.1 * covered.width + CENTRE.0) * 4) as usize;
    assert_eq!(
        covered.rgba[i..i + 3],
        off.rgba[i..i + 3],
        "a sprite pixel carries nothing from behind the sprite"
    );
}

/// The term is a pure function of the scene. Skips with no GPU.
#[test]
fn needs_gpu_the_term_is_deterministic() {
    let Some(renderer) = skip_or_renderer("the_term_is_deterministic") else {
        return;
    };
    let mut scene = scatter_scene();
    scene.lights = vec![point(Vec3::new(0.0, 0.0, -6.0), Vec3::ONE, 10.0, 1.0)];
    let a = renderer.render(&scene, size()).expect("a");
    let b = renderer.render(&scene, size()).expect("b");
    assert_eq!(a.rgba, b.rgba);
}

/// Both scatter shaders bind exactly the one layout their effect fills.
#[test]
fn the_scatter_shaders_bind_their_own_layouts() {
    assert_eq!(
        fullscreen_layout(ShaderId::MediumScatter),
        LayoutId::MediumScatter
    );
    assert_eq!(
        fullscreen_layout(ShaderId::MediumScatterBlur),
        LayoutId::MediumScatterBlur
    );
}
