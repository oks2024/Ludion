//! A lit or flashing additive texel claims the layer weight of the colour it adds: GPU tests over
//! the transparent layer and the fog it drives.

use super::shaft_layer::srgb8_to_linear;
use super::*;

const SIDE: u32 = 64;
const DENSITY: f32 = 0.03;
/// Dim enough that a white lit sprite adds well under one.
const LIGHT_INTENSITY: f32 = 0.4;

fn size() -> FrameSize {
    FrameSize::native(SIDE, SIDE)
}

/// A red wall at twenty units under fog, one dim point light in front of where the sprites sit.
fn lit_scene() -> RenderScene {
    let mut scene = own_fog_scene(SIDE, SIDE, DENSITY);
    let mut wall = slab(Vec3::new(0.0, 0.0, -20.0), 60.0, SLAB_COLOR);
    wall.cast_shadows = false;
    scene.meshes = vec![wall];
    scene.ambient = [0.0; 3];
    scene.lights = vec![point_light(
        Vec3::new(0.0, 0.0, -3.0),
        LIGHT_INTENSITY,
        30.0,
    )];
    scene
}

fn lit(mut sprite: crate::scene::SpriteInstance) -> crate::scene::SpriteInstance {
    sprite.lit = true;
    sprite
}

fn lit_particle(additive: bool, color: [f32; 4]) -> ParticleInstance {
    let mut p = particle(Vec3::new(0.0, 0.0, -5.0), 3.0, color, 0.0);
    p.additive = additive;
    p.lit = true;
    p
}

fn centre_g(renderer: &Renderer, scene: &RenderScene) -> f32 {
    let size = size();
    centre_layer(&shaft_layer_texels_sunless(renderer, scene, size), size)[1]
}

fn shaft_layer_texels_sunless(
    renderer: &Renderer,
    scene: &RenderScene,
    size: FrameSize,
) -> Vec<[f32; 2]> {
    let mut sunless = scene.clone();
    sunless.sun = None;
    layer_texels(renderer, &sunless, size)
}

/// The colour the colour pass adds, in linear light: `scene`'s draws alone on black, no fog.
fn emitted(renderer: &Renderer, scene: &RenderScene) -> [f32; 3] {
    let mut bare = scene.clone();
    bare.meshes.clear();
    bare.post = Some(exact_post());
    bare.environment = Some(crate::scene::EnvironmentData {
        sky_zenith: [0.0; 3],
        sky_horizon: [0.0; 3],
        sky_ground: [0.0; 3],
        sun_disc_size: 0.0,
        sun_disc_intensity: 0.0,
        ambient: [0.0; 3],
        ibl_intensity: 0.0,
    });
    let image = renderer.render(&bare, size()).expect("emitted colour");
    let px = rgb_at(&image, SIDE / 2, SIDE / 2);
    std::array::from_fn(|c| srgb8_to_linear(px[c]))
}

/// The worst channel gap between two frames over the centre block, with both centre pixels.
fn interior_gap(a: &RenderedImage, b: &RenderedImage) -> (u8, [u8; 3], [u8; 3]) {
    let mut worst = 0;
    for y in SIDE / 2 - 2..SIDE / 2 + 3 {
        for x in SIDE / 2 - 2..SIDE / 2 + 3 {
            worst = worst.max(channel_gap(rgb_at(a, x, y), rgb_at(b, x, y)));
        }
    }
    (
        worst,
        rgb_at(a, SIDE / 2, SIDE / 2),
        rgb_at(b, SIDE / 2, SIDE / 2),
    )
}

fn assert_claims_what_it_adds(
    what: &str,
    renderer: &Renderer,
    lit_scene: RenderScene,
    unlit: impl Fn([f32; 3]) -> RenderScene,
) {
    let e = emitted(renderer, &lit_scene);
    let twin = unlit(e);
    let (g_lit, g_twin) = (centre_g(renderer, &lit_scene), centre_g(renderer, &twin));
    let frame_lit = renderer.render(&lit_scene, size()).expect("lit frame");
    let frame_twin = renderer.render(&twin, size()).expect("unlit frame");
    let (gap, a, b) = interior_gap(&frame_lit, &frame_twin);
    eprintln!(
        "{what}: emitted {e:?}; G lit {g_lit:.4} unlit twin {g_twin:.4}; centre lit {a:?} twin {b:?}; interior gap {gap} LSB",
    );
    assert!(
        (g_lit - g_twin).abs() <= 0.02 * g_twin.max(1e-3),
        "{what}: layer G {g_lit} against the unlit twin's {g_twin}",
    );
    assert!(gap <= 3, "{what}: composited interiors differ by {gap} LSB");
}

/// A white lit additive sprite in dim light claims what it adds, not its unlit brightness.
#[test]
fn needs_gpu_a_dim_lit_additive_claims_what_it_adds() {
    let Some(renderer) = skip_or_renderer("a_dim_lit_additive_claims_what_it_adds") else {
        return;
    };
    let mut scene = lit_scene();
    scene.sprites = vec![lit(sized_sprite(
        Vec3::new(0.0, 0.0, -5.0),
        3.0,
        [1.0; 4],
        true,
    ))];
    assert_claims_what_it_adds("lit additive sprite", &renderer, scene.clone(), |e| {
        let mut twin = scene.clone();
        twin.sprites = vec![sized_sprite(
            Vec3::new(0.0, 0.0, -5.0),
            3.0,
            [e[0], e[1], e[2], 1.0],
            true,
        )];
        twin
    });
}

#[test]
fn needs_gpu_a_dim_lit_additive_particle_claims_what_it_adds() {
    let Some(renderer) = skip_or_renderer("a_dim_lit_additive_particle_claims_what_it_adds") else {
        return;
    };
    let mut scene = lit_scene();
    scene.particles = vec![lit_particle(true, [1.0; 4])];
    assert_claims_what_it_adds("lit additive particle", &renderer, scene.clone(), |e| {
        let mut twin = scene.clone();
        let mut p = lit_particle(true, [e[0], e[1], e[2], 1.0]);
        p.lit = false;
        twin.particles = vec![p];
        twin
    });
}

/// A black additive sprite painting bright with its flash claims the flash's colour.
#[test]
fn needs_gpu_a_flashing_dark_additive_claims_what_it_adds() {
    let Some(renderer) = skip_or_renderer("a_flashing_dark_additive_claims_what_it_adds") else {
        return;
    };
    let flash_color = [1.0, 0.6, 0.3];
    for (amount, tolerance) in [(1.0_f32, 0.02_f32), (0.5, 0.05)] {
        let mut scene = lit_scene();
        scene.lights.clear();
        let mut flashing = sized_sprite(Vec3::new(0.0, 0.0, -5.0), 3.0, [0.0, 0.0, 0.0, 1.0], true);
        flashing.flash = amount;
        flashing.flash_color = flash_color;
        scene.sprites = vec![flashing];
        let mut twin = scene.clone();
        twin.sprites = vec![sized_sprite(
            Vec3::new(0.0, 0.0, -5.0),
            3.0,
            [
                flash_color[0] * amount,
                flash_color[1] * amount,
                flash_color[2] * amount,
                1.0,
            ],
            true,
        )];
        let (g, g_twin) = (centre_g(&renderer, &scene), centre_g(&renderer, &twin));
        eprintln!("flash {amount}: G {g:.4} unlit twin {g_twin:.4}");
        assert!(
            (g - g_twin).abs() <= tolerance * g_twin,
            "flash {amount}: G {g} against the unlit twin's {g_twin}",
        );
        if amount == 1.0 {
            assert!(g > 0.9, "a full flash claims nearly all: {g}");
        }
    }
}

/// A non-additive weight is alpha, whatever the light does.
#[test]
fn needs_gpu_a_lit_alpha_sprite_claims_its_alpha() {
    let Some(renderer) = skip_or_renderer("a_lit_alpha_sprite_claims_its_alpha") else {
        return;
    };
    let alpha = 0.4;
    let mut sprite_scene = lit_scene();
    sprite_scene.sprites = vec![lit(sized_sprite(
        Vec3::new(0.0, 0.0, -5.0),
        3.0,
        [1.0, 1.0, 1.0, alpha],
        false,
    ))];
    let mut particle_scene = lit_scene();
    particle_scene.particles = vec![lit_particle(false, [1.0, 1.0, 1.0, alpha])];
    for (what, scene) in [("sprite", sprite_scene), ("particle", particle_scene)] {
        let g = centre_g(&renderer, &scene);
        eprintln!("lit alpha {what}: G {g:.5}");
        assert!((g - alpha).abs() <= 2e-3, "lit alpha {what}: G {g}");
    }
}

/// The wall's brightest-channel gap to `reference` along row `y`: the first column where `row`
/// has moved more than halfway from `wall` to `inside`.
fn edge_column(image: &RenderedImage, y: u32, wall: [u8; 3], inside: [u8; 3]) -> u32 {
    let total = i32::from(channel_gap(wall, inside));
    (0..image.width)
        .find(|&x| {
            let px = rgb_at(image, x, y);
            let moved = (0..3)
                .map(|c| (i32::from(px[c]) - i32::from(wall[c])).abs())
                .max()
                .unwrap();
            moved * 2 > total
        })
        .expect("the sprite has an edge")
}

/// The layer is sampled at the right texel at any render scale: an opaque sprite over a wall under
/// fog reads the same at 2x and 0.5x as at 1x, interior, wall and edge.
#[test]
fn needs_gpu_fog_reads_the_right_layer_texel_at_any_render_scale() {
    let Some(renderer) = skip_or_renderer("fog_reads_the_right_layer_texel_at_any_render_scale")
    else {
        return;
    };
    let mut scene = own_fog_scene(SIDE, SIDE, DENSITY);
    scene.meshes = vec![slab(Vec3::new(0.0, 0.0, -60.0), 90.0, SLAB_COLOR)];
    scene.sprites = vec![sized_sprite(
        Vec3::new(0.0, 0.0, -5.0),
        3.0,
        OPAQUE_SLAB,
        false,
    )];
    let base = renderer.render(&scene, size()).expect("scale 1");
    let (wall, inside) = (
        rgb_at(&base, 3, SIDE / 2),
        rgb_at(&base, SIDE / 2, SIDE / 2),
    );
    assert!(
        channel_gap(wall, inside) > 20,
        "sprite {inside:?} against wall {wall:?}"
    );
    let base_edge = edge_column(&base, SIDE / 2, wall, inside);
    for factor in [2.0_f32, 0.5] {
        let scale = crate::RenderScale::new(factor).unwrap();
        let image = renderer
            .render(&scene, FrameSize::scaled(SIDE, SIDE, scale))
            .expect("scaled render");
        assert_eq!((image.width, image.height), (SIDE, SIDE));
        let interior = channel_gap(rgb_at(&image, SIDE / 2, SIDE / 2), inside);
        let beyond = channel_gap(rgb_at(&image, 3, SIDE / 2), wall);
        let edge = edge_column(&image, SIDE / 2, wall, inside);
        eprintln!(
            "scale {factor}: interior gap {interior}, wall gap {beyond}, edge {edge} (scale 1: {base_edge})"
        );
        assert!(
            interior <= 2,
            "scale {factor}: sprite interior moved {interior} LSB"
        );
        assert!(beyond <= 2, "scale {factor}: wall moved {beyond} LSB");
        assert!(
            edge.abs_diff(base_edge) <= 1,
            "scale {factor}: edge at {edge}, scale 1 at {base_edge}"
        );
    }
}
