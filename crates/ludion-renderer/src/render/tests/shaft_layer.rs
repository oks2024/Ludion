//! Light shafts at each transparent's own distance: GPU tests over the shaft march's layer read.

use super::*;
use ludion_core::math::glam::Mat4;

/// The frame edge of the shaft fixtures: a sprite spans tens of pixels and the blur is small
/// beside it.
const SHAFT_PX: u32 = 128;
/// The medium's extinction: its `3 / σ_t` reach is sixty units, so a near layer ends the march
/// well short of what a wall would.
const SHAFT_SIGMA: f32 = 0.05;
/// The authored `shafts` strength, high enough that the far term is tens of LSB.
const SHAFT_GAIN: f32 = 0.17;

/// The exact-output post with the medium and its shafts on, and a black fog colour so the
/// shaft term is the only light a black surface carries.
fn shaft_layer_post(gain: f32) -> PostData {
    PostData {
        fog_density: SHAFT_SIGMA,
        fog_color: [0.0; 3],
        shafts: gain,
        ..exact_post()
    }
}

/// The direction the fixture's sun travels: toward the camera and down, so the view ray sits
/// in the phase function's forward lobe.
fn shaft_sun_direction() -> Vec3 {
    Vec3::new(0.0, -0.3, 1.0).normalize()
}

/// An empty frame with the medium, a sun and no occluder: every sample the march takes is lit
/// (a point outside the shadow frustum counts as lit), so the term depends on the march's length
/// and its direction alone.
fn shaft_layer_scene(w: u32, h: u32) -> RenderScene {
    let mut scene = fog_scene(w, h);
    scene.meshes.clear();
    scene.sun = Some(test_sun(shaft_sun_direction(), Vec3::ONE, 0.002));
    scene.post = Some(shaft_layer_post(SHAFT_GAIN));
    scene
}

/// A black wall facing the camera, its front at `distance`, big enough to fill the frame.
/// It casts no shadow, so nothing in the frame is shadowed.
fn shaft_wall(distance: f32) -> MeshInstance {
    let size = 3.0 * distance;
    let mut wall = slab(Vec3::new(0.0, 0.0, -distance - size / 2.0), size, [0.0; 3]);
    wall.cast_shadows = false;
    wall
}

/// The Henyey–Greenstein phase the fixture's medium gives a ray (`fog_anisotropy` 0.7).
fn shaft_phase(ray: Vec3) -> f32 {
    let g = 0.7_f32;
    let cos = ray.dot(-shaft_sun_direction());
    (1.0 - g * g) / (4.0 * std::f32::consts::PI * (1.0 + g * g - 2.0 * g * cos).powf(1.5))
}

/// The linear light a black surface at `reach` carries in the uniform lit medium: the march's
/// integral in closed form, `α (1 - e^{-σ R}) · phase · shafts`.
fn shaft_expected(reach: f32, ray: Vec3) -> f32 {
    (1.0 - (-SHAFT_SIGMA * reach).exp()) * shaft_phase(ray) * SHAFT_GAIN
}

pub(super) fn srgb8_to_linear(byte: u8) -> f32 {
    let c = f32::from(byte) / 255.0;
    if c <= 0.04045 {
        c / 12.92
    } else {
        ((c + 0.055) / 1.055).powf(2.4)
    }
}

/// What `shafts` added to every pixel: the frame with them on minus the same frame with
/// `shafts: 0` (same fog, same everything), as the largest channel step in LSB and as linear
/// light in the first channel.
struct ShaftTerm {
    width: u32,
    lsb: Vec<i32>,
    linear: Vec<f32>,
}

impl ShaftTerm {
    fn at(&self, x: u32, y: u32) -> i32 {
        self.lsb[(y * self.width + x) as usize]
    }

    fn linear_at(&self, x: u32, y: u32) -> f32 {
        self.linear[(y * self.width + x) as usize]
    }

    fn mean_linear(&self, pixels: &[(u32, u32)]) -> f32 {
        pixels
            .iter()
            .map(|&(x, y)| self.linear_at(x, y))
            .sum::<f32>()
            / pixels.len() as f32
    }

    fn mean_lsb(&self, pixels: &[(u32, u32)]) -> f32 {
        pixels
            .iter()
            .map(|&(x, y)| self.at(x, y) as f32)
            .sum::<f32>()
            / pixels.len() as f32
    }
}

fn shaft_term(renderer: &Renderer, scene: &RenderScene, size: FrameSize) -> ShaftTerm {
    let mut off = scene.clone();
    if let Some(post) = off.post.as_mut() {
        post.shafts = 0.0;
    }
    let on = renderer.render(scene, size).expect("shafts on");
    let off = renderer.render(&off, size).expect("shafts off");
    let pixels = (on.width * on.height) as usize;
    let mut term = ShaftTerm {
        width: on.width,
        lsb: Vec::with_capacity(pixels),
        linear: Vec::with_capacity(pixels),
    };
    for i in 0..pixels {
        let (a, b) = (&on.rgba[i * 4..i * 4 + 3], &off.rgba[i * 4..i * 4 + 3]);
        term.lsb.push(
            (0..3)
                .map(|c| i32::from(a[c]) - i32::from(b[c]))
                .max()
                .unwrap(),
        );
        term.linear
            .push(srgb8_to_linear(a[0]) - srgb8_to_linear(b[0]));
    }
    term
}

/// The pixels the layer covers fully with every pixel within `inset` of them covered too, so
/// the half-resolution rim and the blur's reach stay outside.
fn interior_pixels(texels: &[[f32; 2]], width: u32, inset: u32) -> Vec<(u32, u32)> {
    let height = texels.len() as u32 / width;
    let full = |x: u32, y: u32| texels[(y * width + x) as usize][1] > 0.999;
    covered_pixels(texels, width)
        .into_iter()
        .filter(|&(x, y)| {
            (x.saturating_sub(inset)..=(x + inset).min(width - 1)).all(|nx| {
                (y.saturating_sub(inset)..=(y + inset).min(height - 1)).all(|ny| full(nx, ny))
            })
        })
        .collect()
}

/// The layer target of `scene`. The layer graph records no sun pass, so the sun comes off
/// before the recorder is built; the layer does not depend on it.
fn shaft_layer_texels(renderer: &Renderer, scene: &RenderScene, size: FrameSize) -> Vec<[f32; 2]> {
    let mut sunless = scene.clone();
    sunless.sun = None;
    layer_texels(renderer, &sunless, size)
}

/// The unit direction of the eye ray through pixel `(x, y)` of a `SHAFT_PX` frame from
/// `origin_camera` (60° vertical field of view, looking down −Z), and the distance along it to
/// the plane `z = -depth`. Under `ortho` every ray is −Z and the distance is `depth`.
fn shaft_pixel_ray(x: u32, y: u32, depth: f32, ortho: bool) -> (Vec3, f32) {
    if ortho {
        return (Vec3::NEG_Z, depth);
    }
    let half = 30.0_f32.to_radians().tan();
    let ndc = |p: u32| (p as f32 + 0.5) / SHAFT_PX as f32 * 2.0 - 1.0;
    let toward = Vec3::new(ndc(x) * half, -ndc(y) * half, -1.0);
    (toward.normalize(), depth * toward.length())
}

/// A layer on the plane `z = -depth` takes the beams of that short march and none from the air
/// behind it: its interior carries less than half of what the same pixels carry without it, and
/// at every pixel no more than the closed-form term for that pixel's own distance and ray (with
/// a tenth of headroom for the blur's mixing). Reports both.
fn assert_layer_stops_beams(
    what: &str,
    renderer: &Renderer,
    bare: &RenderScene,
    with: &RenderScene,
    inset: u32,
    depth: f32,
    ortho: bool,
) {
    let size = shaft_px_size();
    let interior = interior_pixels(
        &shaft_layer_texels(renderer, with, size),
        size.render.0,
        inset,
    );
    assert!(
        interior.len() > 20,
        "{what} covers pixels outright: {}",
        interior.len()
    );
    let behind = shaft_term(renderer, bare, size);
    let layered = shaft_term(renderer, with, size);
    let over = |&(x, y): &(u32, u32)| {
        let (ray, reach) = shaft_pixel_ray(x, y, depth, ortho);
        layered.linear_at(x, y) / shaft_expected(reach, ray)
    };
    let worst = interior.iter().map(over).fold(0.0_f32, f32::max);
    eprintln!(
        "{what}: layer {:.1} LSB, without {:.1} LSB, worst ratio to the closed form {worst:.3}, over {} pixels",
        layered.mean_lsb(&interior),
        behind.mean_lsb(&interior),
        interior.len(),
    );
    assert!(
        behind.mean_lsb(&interior) > 30.0,
        "{what}: the air behind must carry a term to be stopped: {:.1} LSB",
        behind.mean_lsb(&interior),
    );
    assert!(
        layered.mean_lsb(&interior) < behind.mean_lsb(&interior) / 2.0,
        "{what}: the layer still takes the beams from behind it: {:.1} LSB against {:.1}",
        layered.mean_lsb(&interior),
        behind.mean_lsb(&interior),
    );
    assert!(
        worst <= 1.1,
        "{what}: a pixel carries {worst:.3} times the march over its own distance",
    );
}

fn shaft_px_size() -> FrameSize {
    FrameSize::native(SHAFT_PX, SHAFT_PX)
}

/// An opaque sprite at 3 over a wall at 60 takes only the beams of its own three units.
/// Skips with no GPU.
#[test]
fn needs_gpu_a_near_sprite_takes_no_beams_from_behind_it() {
    let Some(renderer) = skip_or_renderer("a_near_sprite_takes_no_beams_from_behind_it") else {
        return;
    };
    let mut bare = shaft_layer_scene(SHAFT_PX, SHAFT_PX);
    bare.meshes.push(shaft_wall(60.0));
    let mut with = bare.clone();
    with.sprites = vec![sized_sprite(
        Vec3::new(0.0, 0.0, -3.0),
        2.4,
        [0.0, 0.0, 0.0, 1.0],
        false,
    )];
    assert_layer_stops_beams("sprite", &renderer, &bare, &with, 5, 3.0, false);
}

/// The same opaque sprite at 5 reads the same term over a wall twenty units away and over the
/// open sky. Skips with no GPU.
#[test]
fn needs_gpu_a_sprite_takes_the_same_beams_over_a_wall_and_over_the_sky() {
    let Some(renderer) =
        skip_or_renderer("a_sprite_takes_the_same_beams_over_a_wall_and_over_the_sky")
    else {
        return;
    };
    let size = shaft_px_size();
    let mut sky = shaft_layer_scene(SHAFT_PX, SHAFT_PX);
    sky.sprites = vec![sized_sprite(
        Vec3::new(0.0, 0.0, -5.0),
        3.0,
        OPAQUE_SLAB,
        false,
    )];
    let mut wall = sky.clone();
    wall.meshes.push(shaft_wall(20.0));
    let interior = interior_pixels(&shaft_layer_texels(&renderer, &sky, size), SHAFT_PX, 5);
    assert!(interior.len() > 100, "interior: {}", interior.len());
    let over_sky = shaft_term(&renderer, &sky, size);
    let over_wall = shaft_term(&renderer, &wall, size);
    let worst = interior
        .iter()
        .map(|&(x, y)| (over_sky.at(x, y) - over_wall.at(x, y)).abs())
        .max()
        .unwrap();
    eprintln!(
        "sky {:.1} LSB, wall {:.1} LSB, worst gap {worst}",
        over_sky.mean_lsb(&interior),
        over_wall.mean_lsb(&interior),
    );
    assert!(
        over_sky.mean_lsb(&interior) > 10.0,
        "the sprite carries a term"
    );
    assert!(
        worst <= 2,
        "the beams depend on what is behind: {worst} LSB"
    );
}

/// A sprite across a wall's top edge carries a term with no step at the edge. The camera is
/// orthographic: a perspective frame's phase function slopes the term by itself, about one LSB
/// a row, which a 2 LSB step bound cannot see through. The interior column is inset by the blur
/// footprint. Skips with no GPU.
#[test]
fn needs_gpu_a_sprite_straddling_the_horizon_has_no_beam_seam() {
    let Some(renderer) = skip_or_renderer("a_sprite_straddling_the_horizon_has_no_beam_seam")
    else {
        return;
    };
    let size = shaft_px_size();
    let mut scene = shaft_layer_scene(SHAFT_PX, SHAFT_PX);
    scene.camera = ortho_fog_camera();
    scene.camera.projection = Mat4::orthographic_rh(-8.0, 8.0, -8.0, 8.0, 0.1, 1000.0);
    // Its top face at the horizon: the wall, ten units away, fills the lower half of the frame.
    let mut wall = shaft_wall(10.0);
    wall.model =
        Mat4::from_translation(Vec3::new(0.0, -15.0, -25.0)) * Mat4::from_scale(Vec3::splat(30.0));
    scene.meshes.push(wall);
    scene.sprites = vec![sized_sprite(
        Vec3::new(0.0, 0.0, -5.0),
        6.0,
        OPAQUE_SLAB,
        false,
    )];
    let interior = interior_pixels(&shaft_layer_texels(&renderer, &scene, size), SHAFT_PX, 5);
    let term = shaft_term(&renderer, &scene, size);
    let x = SHAFT_PX / 2;
    let column: Vec<u32> = (0..SHAFT_PX)
        .filter(|&y| interior.contains(&(x, y)))
        .collect();
    assert!(column.len() > 30, "column: {}", column.len());
    assert!(
        column.contains(&(SHAFT_PX / 2)) && column.contains(&(SHAFT_PX / 2 - 1)),
        "the column crosses the horizon"
    );
    let step = column
        .windows(2)
        .map(|pair| (term.at(x, pair[1]) - term.at(x, pair[0])).abs())
        .max()
        .unwrap();
    eprintln!(
        "seam: column {} rows, term {}..{} LSB, worst step {step}",
        column.len(),
        column.iter().map(|&y| term.at(x, y)).min().unwrap(),
        column.iter().map(|&y| term.at(x, y)).max().unwrap(),
    );
    assert!(step <= 2, "a seam at the wall's edge: {step} LSB");
}

/// A half-covered sprite at 5 over a wall at 60 takes the coverage-weighted share: between the
/// opaque sprite's term and the bare wall's, within 15% of their midpoint. The march's step
/// size over sixty units is why it is no tighter. Skips with no GPU.
#[test]
fn needs_gpu_a_soft_sprite_takes_a_coverage_weighted_share_of_the_beams() {
    let Some(renderer) =
        skip_or_renderer("a_soft_sprite_takes_a_coverage_weighted_share_of_the_beams")
    else {
        return;
    };
    let size = shaft_px_size();
    let mut bare = shaft_layer_scene(SHAFT_PX, SHAFT_PX);
    bare.meshes.push(shaft_wall(60.0));
    let sprite = |alpha: f32| {
        let mut scene = bare.clone();
        scene.sprites = vec![sized_sprite(
            Vec3::new(0.0, 0.0, -5.0),
            3.0,
            [0.0, 0.0, 0.0, alpha],
            false,
        )];
        scene
    };
    let opaque = sprite(1.0);
    let soft = sprite(0.5);
    let interior = interior_pixels(&shaft_layer_texels(&renderer, &opaque, size), SHAFT_PX, 5);
    assert!(interior.len() > 100, "interior: {}", interior.len());
    let near = shaft_term(&renderer, &opaque, size).mean_linear(&interior);
    let far = shaft_term(&renderer, &bare, size).mean_linear(&interior);
    let half = shaft_term(&renderer, &soft, size).mean_linear(&interior);
    let midpoint = 0.5 * (near + far);
    let off = (half - midpoint).abs() / midpoint;
    eprintln!(
        "soft: opaque {near:.5}, bare {far:.5}, soft {half:.5}, midpoint {midpoint:.5}, off by {:.1}%",
        off * 100.0
    );
    assert!(
        near < half && half < far,
        "soft {half} not between {near} and {far}"
    );
    assert!(
        off <= 0.15,
        "soft term {half:.5} is {:.1}% from the midpoint {midpoint:.5}",
        off * 100.0
    );
}

/// An alpha particle and a world text run each stop the beams at their own distance. Skips
/// with no GPU.
#[test]
fn needs_gpu_particles_and_world_text_stop_the_beams() {
    let Some(renderer) = skip_or_renderer("particles_and_world_text_stop_the_beams") else {
        return;
    };
    let mut bare = shaft_layer_scene(SHAFT_PX, SHAFT_PX);
    bare.meshes.push(shaft_wall(60.0));
    let mut particles = bare.clone();
    particles.particles = vec![particle(
        Vec3::new(0.0, 0.0, -3.0),
        2.4,
        [0.0, 0.0, 0.0, 1.0],
        0.0,
    )];
    let mut text = bare.clone();
    let mut run = layer_text(-4.0);
    run.color = [0.0, 0.0, 0.0, 1.0];
    text.texts = vec![run];
    for (what, scene, inset, depth) in [("particle", &particles, 5, 3.0), ("text", &text, 5, 4.0)] {
        assert_layer_stops_beams(what, &renderer, &bare, scene, inset, depth, false);
    }
}

/// A black additive texel claims nothing: the frame equals the one without the sprite, byte for
/// byte. Skips with no GPU.
#[test]
fn needs_gpu_a_black_additive_texel_stops_no_beams() {
    let Some(renderer) = skip_or_renderer("a_black_additive_texel_stops_no_beams") else {
        return;
    };
    let size = shaft_px_size();
    let mut bare = shaft_layer_scene(SHAFT_PX, SHAFT_PX);
    bare.meshes.push(shaft_wall(60.0));
    let mut black = bare.clone();
    black.sprites = vec![sized_sprite(
        Vec3::new(0.0, 0.0, -3.0),
        2.4,
        [0.0, 0.0, 0.0, 1.0],
        true,
    )];
    assert_eq!(
        renderer.render(&bare, size).expect("bare").rgba,
        renderer.render(&black, size).expect("black").rgba,
        "a black additive quad must leave the beams alone",
    );
}

/// A pixel the layer does not reach keeps the plain march: past the blur's footprint and two more
/// pixels, a frame with a small sprite equals one without. Skips with no GPU.
#[test]
fn needs_gpu_an_uncovered_pixel_keeps_the_plain_march() {
    let Some(renderer) = skip_or_renderer("an_uncovered_pixel_keeps_the_plain_march") else {
        return;
    };
    let size = shaft_px_size();
    let mut bare = shaft_layer_scene(SHAFT_PX, SHAFT_PX);
    bare.meshes.push(shaft_wall(60.0));
    let mut with = bare.clone();
    with.sprites = vec![sized_sprite(
        Vec3::new(0.0, 0.0, -5.0),
        1.0,
        OPAQUE_SLAB,
        false,
    )];
    let texels = shaft_layer_texels(&renderer, &with, size);
    let covered: Vec<(u32, u32)> = texels
        .iter()
        .enumerate()
        .filter(|(_, texel)| texel[1] > 0.0)
        .map(|(i, _)| (i as u32 % SHAFT_PX, i as u32 / SHAFT_PX))
        .collect();
    assert!(covered.len() > 50, "the sprite covers {}", covered.len());
    let margin = 7;
    let near = |x: u32, y: u32| {
        covered
            .iter()
            .any(|&(cx, cy)| cx.abs_diff(x).max(cy.abs_diff(y)) <= margin)
    };
    let a = renderer.render(&bare, size).expect("bare");
    let b = renderer.render(&with, size).expect("with");
    let mut checked = 0;
    for y in 0..SHAFT_PX {
        for x in 0..SHAFT_PX {
            if !near(x, y) {
                checked += 1;
                assert_eq!(rgb_at(&a, x, y), rgb_at(&b, x, y), "pixel ({x}, {y}) moved");
            }
        }
    }
    assert!(checked > 10_000, "checked {checked}");
}

/// Shafts and fog on, a spark that comes and goes: the streamed frames match a fresh renderer's.
/// Skips with no GPU.
#[test]
fn needs_gpu_a_streamed_shafts_flip_leaves_the_image_untouched() {
    let (w, h) = (64u32, 64u32);
    let mut bare = shaft_layer_scene(w, h);
    bare.meshes.push(shaft_wall(60.0));
    let mut with = bare.clone();
    with.sprites = vec![sized_sprite(
        Vec3::new(0.0, 0.0, -5.0),
        3.0,
        OPAQUE_SLAB,
        false,
    )];
    streamed_flip_is_neutral(
        "a_streamed_shafts_flip_leaves_the_image_untouched",
        FrameSize::native(w, h),
        &bare,
        &with,
    );
}

/// Under an orthographic camera a plane facing it carries one term wherever it is seen, and
/// along a row with a height falloff. Skips with no GPU.
#[test]
fn needs_gpu_ortho_shafts_do_not_fan() {
    let Some(renderer) = skip_or_renderer("ortho_shafts_do_not_fan") else {
        return;
    };
    let (w, h) = (64u32, 64u32);
    let size = FrameSize::native(w, h);
    let mut scene = shaft_layer_scene(w, h);
    scene.camera = ortho_fog_camera();
    let mut plane = slab(Vec3::new(0.0, 0.0, -60.0), 100.0, [0.0; 3]);
    plane.cast_shadows = false;
    scene.meshes.push(plane);
    let spread = |term: &ShaftTerm, pixels: &[(u32, u32)]| {
        let first = term.at(pixels[0].0, pixels[0].1);
        pixels
            .iter()
            .map(|&(x, y)| (term.at(x, y) - first).abs())
            .max()
            .unwrap()
    };
    let everywhere = [
        (0, 0),
        (63, 0),
        (0, 63),
        (63, 63),
        (32, 32),
        (8, 40),
        (55, 12),
    ];
    let one_row = [(0, 32), (10, 32), (32, 32), (50, 32), (63, 32)];

    let plain = shaft_term(&renderer, &scene, size);
    eprintln!(
        "plain: {} LSB, spread {}",
        plain.at(32, 32),
        spread(&plain, &everywhere)
    );
    assert!(plain.at(32, 32) > 20, "the plane carries a term");
    assert!(spread(&plain, &everywhere) <= 2, "ortho shafts fan");

    let mut height = scene.clone();
    if let Some(post) = height.post.as_mut() {
        post.fog_height_falloff = 0.05;
    }
    let falloff = shaft_term(&renderer, &height, size);
    eprintln!("height: spread along a row {}", spread(&falloff, &one_row));
    assert!(
        spread(&falloff, &one_row) <= 2,
        "height shafts fan along a row"
    );
}

/// A near sprite under an orthographic camera stops the beams at its view depth. Skips with no
/// GPU.
#[test]
fn needs_gpu_a_near_sprite_stops_the_beams_under_ortho() {
    let Some(renderer) = skip_or_renderer("a_near_sprite_stops_the_beams_under_ortho") else {
        return;
    };
    let mut bare = shaft_layer_scene(SHAFT_PX, SHAFT_PX);
    bare.camera = ortho_fog_camera();
    bare.camera.projection = Mat4::orthographic_rh(-8.0, 8.0, -8.0, 8.0, 0.1, 1000.0);
    bare.meshes.push(shaft_wall(100.0));
    let mut with = bare.clone();
    with.sprites = vec![sized_sprite(
        Vec3::new(0.0, 0.0, -3.0),
        6.0,
        [0.0, 0.0, 0.0, 1.0],
        false,
    )];
    assert_layer_stops_beams("ortho sprite", &renderer, &bare, &with, 5, 3.0, true);
}
