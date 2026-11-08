//! Local-light in-scatter: GPU tests over the `medium_scatter` pass and its composite add.

use super::*;
use std::sync::Arc;

use crate::scene::{GiData, PointLightData, SpotLightData, WorldVoxelGrid};

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

fn shadow_data() -> Option<crate::scene::LocalShadowData> {
    Some(crate::scene::LocalShadowData {
        bias: 0.05,
        near: 0.1,
        distance: None,
        fade: 0.0,
    })
}

/// A black box of the given edge lengths that casts.
fn pillar(center: Vec3, size: Vec3) -> MeshInstance {
    let mut mesh = slab(center, 1.0, [0.0; 3]);
    mesh.model = Mat4::from_translation(center) * Mat4::from_scale(size);
    mesh
}

fn sum_at(frame: &RenderedImage, x: u32, y: u32) -> i32 {
    let i = ((y * frame.width + x) * 4) as usize;
    sum([frame.rgba[i], frame.rgba[i + 1], frame.rgba[i + 2]])
}

/// The same scene with every light's shadow request dropped.
fn unshadowed(scene: &RenderScene) -> RenderScene {
    let mut off = scene.clone();
    off.lights.iter_mut().for_each(|l| l.shadow = None);
    off.spot_lights.iter_mut().for_each(|l| l.shadow = None);
    off
}

/// A light at `(3, 0, -8)` and a thin wall at `x = 1.5` (z from -12 to -6) between it and the view's
/// centre line: the wall's shadow covers the centre column from 4 to 16 units out, and the lit side
/// (column 110, `x` above 2.4 at 6 units) never meets it.
const GAP: (u32, u32) = (64, 64);
const CLEAR: (u32, u32) = (110, 64);

fn pillar_scene() -> RenderScene {
    let mut scene = scatter_scene();
    scene.meshes = vec![pillar(Vec3::new(1.5, 0.0, -9.0), Vec3::new(0.3, 6.0, 6.0))];
    scene
}

fn pillar_point() -> PointLightData {
    PointLightData {
        shadow: shadow_data(),
        range: 12.0,
        ..point(Vec3::new(3.0, 0.0, -8.0), Vec3::splat(1.5), 12.0, 1.0)
    }
}

fn pillar_spot() -> SpotLightData {
    SpotLightData {
        position: Vec3::new(3.0, 0.0, -8.0),
        direction: Vec3::NEG_X,
        color: Vec3::splat(3.0),
        intensity: 4.0,
        range: 12.0,
        cos_inner: 30.0_f32.to_radians().cos(),
        cos_outer: 40.0_f32.to_radians().cos(),
        shadow: shadow_data(),
        scatter: 1.0,
    }
}

/// The dark gap behind `scene`'s pillar against the `shadow: None` render, and a clear pixel that
/// must not move.
fn assert_gap(renderer: &Renderer, scene: &RenderScene) {
    let lit = renderer.render(&unshadowed(scene), size()).expect("lit");
    let shaded = renderer.render(scene, size()).expect("shaded");
    let (gap_lit, gap_shaded) = (sum_at(&lit, GAP.0, GAP.1), sum_at(&shaded, GAP.0, GAP.1));
    let none = renderer
        .render(&without_scatter(scene), size())
        .expect("none");
    eprintln!(
        "gap lit {gap_lit}, shaded {gap_shaded}, none {}",
        sum_at(&none, GAP.0, GAP.1)
    );
    assert!(
        gap_shaded + 20 < gap_lit,
        "the pillar's shadow darkens the gap: {gap_shaded} against {gap_lit}"
    );
    assert_eq!(
        sum_at(&lit, CLEAR.0, CLEAR.1),
        sum_at(&shaded, CLEAR.0, CLEAR.1),
        "a ray clear of the pillar's shadow is unmoved"
    );
}

/// A shadowed point light's glow has a dark gap behind a pillar. Skips with no GPU.
#[test]
fn needs_gpu_a_pillar_casts_a_dark_gap_in_the_glow() {
    let Some(renderer) = skip_or_renderer("a_pillar_casts_a_dark_gap_in_the_glow") else {
        return;
    };
    let mut scene = pillar_scene();
    scene.lights = vec![pillar_point()];
    assert_gap(&renderer, &scene);
}

/// The same for a spot's beam. Skips with no GPU.
#[test]
fn needs_gpu_a_shadowed_spot_beam_is_cut_by_an_occluder() {
    let Some(renderer) = skip_or_renderer("a_shadowed_spot_beam_is_cut_by_an_occluder") else {
        return;
    };
    let mut scene = pillar_scene();
    scene.spot_lights = vec![pillar_spot()];
    assert_gap(&renderer, &scene);
}

/// With the quality's shadows off the light has no grant, so the scatter is the unshadowed one.
/// Skips with no GPU.
#[test]
fn needs_gpu_shadows_masked_by_quality_scatter_unshadowed() {
    let Some(mut renderer) = skip_or_renderer("shadows_masked_by_quality_scatter_unshadowed")
    else {
        return;
    };
    renderer.set_quality(
        Quality {
            shadows: false,
            ..Quality::FULL
        },
        QualityParams::FULL,
    );
    let mut scene = pillar_scene();
    scene.lights = vec![pillar_point()];
    let masked = renderer.render(&scene, size()).expect("masked");
    let lit = renderer.render(&unshadowed(&scene), size()).expect("lit");
    assert_eq!(sum_at(&masked, GAP.0, GAP.1), sum_at(&lit, GAP.0, GAP.1));
}

/// Seventeen shadowed spots overflow the sixteen-tile budget: the skipped one scatters unshadowed
/// whatever its request says. Skips with no GPU.
#[test]
fn needs_gpu_a_budget_skipped_light_scatters_unshadowed() {
    let Some(renderer) = skip_or_renderer("a_budget_skipped_light_scatters_unshadowed") else {
        return;
    };
    let mut scene = pillar_scene();
    scene.spot_lights = (0..17)
        .map(|i| SpotLightData {
            position: Vec3::new(3.0, 0.0, -8.0 + 0.01 * i as f32),
            ..pillar_spot()
        })
        .collect();
    let plan = crate::local_shadow::plan_local_shadows(
        &scene,
        QualityParams::FULL.local_shadow_atlas_size,
    );
    assert_eq!(plan.skipped.len(), 1, "one spot misses the budget");
    let skipped = plan.skipped[0] as usize - scene.lights.len();
    let shaded = renderer.render(&scene, size()).expect("shaded");
    let mut dropped = scene.clone();
    dropped.spot_lights[skipped].shadow = None;
    let dropped = renderer.render(&dropped, size()).expect("dropped");
    assert_eq!(
        shaded.rgba, dropped.rgba,
        "the skipped light's request changes nothing"
    );
    let mut absent = scene.clone();
    absent.spot_lights[skipped].scatter = 0.0;
    let absent = renderer.render(&absent, size()).expect("absent");
    assert!(
        sum_at(&shaded, GAP.0, GAP.1) > sum_at(&absent, GAP.0, GAP.1),
        "the skipped light still glows in the gap"
    );
}

/// A cube light between four pillars on ±X and ±Z, seen from above: the pixel beyond each pillar is
/// darker than unshadowed, so no cube face is mis-picked. Skips with no GPU.
#[test]
fn needs_gpu_cube_faces_cover_every_direction() {
    let Some(renderer) = skip_or_renderer("cube_faces_cover_every_direction") else {
        return;
    };
    let mut scene = scatter_scene();
    let eye = Vec3::new(0.0, 20.0, 0.0);
    scene.camera.view = Mat4::look_at_rh(eye, Vec3::ZERO, Vec3::NEG_Z);
    scene.camera.position = eye;
    scene.meshes = [
        Vec3::new(3.0, 0.0, 0.0),
        Vec3::new(-3.0, 0.0, 0.0),
        Vec3::new(0.0, 0.0, 3.0),
        Vec3::new(0.0, 0.0, -3.0),
    ]
    .map(|c| pillar(c, Vec3::new(0.6, 6.0, 0.6)))
    .to_vec();
    scene.lights = vec![PointLightData {
        shadow: shadow_data(),
        ..point(Vec3::ZERO, Vec3::splat(4.0), 10.0, 1.0)
    }];
    let lit = renderer.render(&unshadowed(&scene), size()).expect("lit");
    let shaded = renderer.render(&scene, size()).expect("shaded");
    for (name, x, y) in [
        ("+x", 97, 64),
        ("-x", 31, 64),
        ("+z", 64, 97),
        ("-z", 64, 31),
    ] {
        let (a, b) = (sum_at(&lit, x, y), sum_at(&shaded, x, y));
        eprintln!("{name}: lit {a}, shaded {b}");
        assert!(b + 20 < a, "{name}: shaded {b} against lit {a}");
    }
}

// --- the GI term ---------------------------------------------------------------------------------

/// The fixtures' haze: a mid-grey ambient, so the glow of the medium (not a surface) is what a pixel
/// reads. The slab behind it is black and unlit, so the haze is the pixel.
const GI_SIGMA: f32 = 0.4;

/// An `8³` unit-cell grid at the origin: a solid one-cell skin when `shell` (a sealed box no sky ray
/// leaves, whose skin glows `radiance` red), else all free.
fn gi_grid(shell: bool, radiance: f32) -> Arc<WorldVoxelGrid> {
    let extent = [8u32, 8, 8];
    let total = 8 * 8 * 8;
    let skin = |i: usize| {
        let (x, y, z) = (i % 8, i / 8 % 8, i / 64);
        [x, y, z].iter().any(|&c| c == 0 || c == 7)
    };
    Arc::new(
        WorldVoxelGrid {
            origin: Vec3::ZERO,
            cell_size: 1.0,
            extent,
            occupancy: (0..total).map(|i| shell && skin(i)).collect(),
            albedo: vec![[1.0, 0.1, 0.1, 1.0]; total],
            emissive: (0..total)
                .map(|i| if shell && skin(i) { radiance } else { 0.0 })
                .collect(),
            hash: 0,
        }
        .sealed(),
    )
}

/// The haze scene inside the grid's box: a camera at its centre looking down -Z at a black slab,
/// a grey medium, no light. `gi` is `(sky_visibility, bounce)` of a volume over the grid, or none.
fn gi_scene(grid: &Arc<WorldVoxelGrid>, gi: Option<(f32, f32)>) -> RenderScene {
    let mut scene = fog_scene(PX, PX);
    let eye = Vec3::new(4.0, 4.0, 4.0);
    scene.camera.view = Mat4::look_at_rh(eye, eye - Vec3::Z, Vec3::Y);
    scene.camera.position = eye;
    scene.meshes = vec![slab(Vec3::new(4.0, 4.0, 0.0), 3.0, [0.0; 3])];
    scene.post = Some(PostData {
        fog_density: GI_SIGMA,
        fog_color: [0.5; 3],
        ..exact_post()
    });
    if let Some((sky_visibility, bounce)) = gi {
        let count = [9u32; 3];
        scene.voxel = Some(grid.clone());
        scene.gi = Some(GiData {
            probe_spacing: 1.0,
            normal_bias: 0.25,
            sky_visibility,
            bounce,
            count,
            origin: Vec3::ZERO,
            span: Vec3::splat(8.0),
        });
    }
    scene
}

fn linear(byte: u8) -> f32 {
    let c = f32::from(byte) / 255.0;
    if c <= 0.04045 {
        c / 12.92
    } else {
        ((c + 0.055) / 1.055).powf(2.4)
    }
}

/// A sealed room lets no sky in, so its haze goes dark; an open volume leaves it as it was.
#[test]
fn needs_gpu_an_enclosed_room_darkens_its_haze() {
    let Some(renderer) = skip_or_renderer("an_enclosed_room_darkens_its_haze") else {
        return;
    };
    let room = gi_grid(true, 0.0);
    let open = gi_grid(false, 0.0);
    let bare = sum(pixel(&renderer, &gi_scene(&room, None), CENTRE.0, CENTRE.1));
    let sealed = sum(pixel(
        &renderer,
        &gi_scene(&room, Some((1.0, 0.0))),
        CENTRE.0,
        CENTRE.1,
    ));
    let visible = sum(pixel(
        &renderer,
        &gi_scene(&open, Some((1.0, 0.0))),
        CENTRE.0,
        CENTRE.1,
    ));
    eprintln!("bare {bare}, sealed {sealed}, open {visible}");
    assert!(bare > 150, "the medium glows without GI: {bare}");
    assert!(
        sealed < bare / 3,
        "the sealed room darkens the haze: {sealed} against {bare}"
    );
    assert!(
        (visible - bare).abs() <= 3,
        "open sky leaves it: {visible} against {bare}"
    );
}

/// A volume at zero strength is no volume: the whole frame is the one without it, medium and all.
#[test]
fn needs_gpu_sky_visibility_zero_and_bounce_zero_is_byte_identical_to_no_gi() {
    let Some(renderer) = skip_or_renderer("zero_strength_gi_is_byte_identical") else {
        return;
    };
    let room = gi_grid(true, 4.0);
    let none = renderer
        .render(&gi_scene(&room, None), size())
        .expect("render");
    let zero = renderer
        .render(&gi_scene(&room, Some((0.0, 0.0))), size())
        .expect("render");
    assert_eq!(none.rgba, zero.rgba);
}

/// A red emitter's bounce reddens the haze it fills: the black slab is lit by nothing, so the
/// change is the in-scatter alone.
#[test]
fn needs_gpu_bounce_adds_tinted_in_scatter() {
    let Some(renderer) = skip_or_renderer("bounce_adds_tinted_in_scatter") else {
        return;
    };
    let room = gi_grid(true, 0.2);
    let off = pixel(
        &renderer,
        &gi_scene(&room, Some((0.0, 0.0))),
        CENTRE.0,
        CENTRE.1,
    );
    let on = pixel(
        &renderer,
        &gi_scene(&room, Some((0.0, 1.0))),
        CENTRE.0,
        CENTRE.1,
    );
    eprintln!("off {off:?}, on {on:?}");
    let gain = |c: usize| i32::from(on[c]) - i32::from(off[c]);
    assert!(
        gain(0) > 10,
        "the haze takes the bounce: {on:?} against {off:?}"
    );
    assert!(
        gain(0) > 3 * gain(2).max(1),
        "and its red tint: {on:?} against {off:?}"
    );
}

/// The sky takes no medium, so no GI term: a frame of nothing but background is the one without GI.
#[test]
fn needs_gpu_the_background_takes_no_gi_term() {
    let Some(renderer) = skip_or_renderer("the_background_takes_no_gi_term") else {
        return;
    };
    let room = gi_grid(true, 4.0);
    let mut bare = gi_scene(&room, None);
    bare.meshes.clear();
    let mut lit = gi_scene(&room, Some((1.0, 1.0)));
    lit.meshes.clear();
    let none = renderer.render(&bare, size()).expect("render");
    let with = renderer.render(&lit, size()).expect("render");
    assert_eq!(none.rgba, with.rgba);
}

/// Both terms in one pass add: the light's glow rides on the GI-scaled ambient, so its linear
/// contribution is the same with or without the volume.
#[test]
fn needs_gpu_a_scattering_light_and_gi_compose() {
    let Some(renderer) = skip_or_renderer("a_scattering_light_and_gi_compose") else {
        return;
    };
    let room = gi_grid(true, 0.0);
    let lit = |gi: Option<(f32, f32)>, scatter: f32| {
        let mut scene = gi_scene(&room, gi);
        scene.lights = vec![point(
            Vec3::new(4.0, 4.0, 2.5),
            Vec3::splat(0.05),
            6.0,
            scatter,
        )];
        linear(pixel(&renderer, &scene, CENTRE.0, CENTRE.1)[1])
    };
    let none = lit(None, 0.0);
    let light = lit(None, 1.0);
    let gi = lit(Some((0.5, 0.0)), 0.0);
    let both = lit(Some((0.5, 0.0)), 1.0);
    eprintln!("none {none}, light {light}, gi {gi}, both {both}");
    assert!(
        light > none + 0.02,
        "the light glows: {light} against {none}"
    );
    assert!(
        gi < none - 0.02,
        "the volume dims the ambient: {gi} against {none}"
    );
    let tolerance = 1.0 / 255.0;
    assert!(
        ((both - gi) - (light - none)).abs() <= tolerance,
        "light term {} with GI, {} without",
        both - gi,
        light - none,
    );
}
