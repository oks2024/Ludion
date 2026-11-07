//! The viewmodel's depth slice: GPU tests over the remapped depth buffer.

use super::*;
use crate::viewmodel::{ViewmodelData, ViewmodelLocal, compose_viewmodel};
use ludion_core::math::glam::Mat4;

const PX: u32 = 64;
/// A bright green no world slab or fog colour shares.
const GUN_COLOR: [f32; 3] = [0.0625, 0.5, 0.0625];
/// The reserved slice, the lens default.
const SLICE: f32 = 0.01;

/// An empty frame under the exact post, the camera at the origin looking down −Z.
fn gun_scene() -> RenderScene {
    let mut scene = fog_scene(PX, PX);
    scene.meshes.clear();
    scene.post = Some(exact_post());
    scene
}

/// Pushes a 0.1 cube flagged as a viewmodel at camera-local z −0.45 and the lens data that goes
/// with it, composed onto the scene's camera.
fn add_gun(scene: &mut RenderScene) {
    add_gun_at(
        scene,
        Mat4::from_translation(Vec3::new(0.0, 0.0, -0.45)) * Mat4::from_scale(Vec3::splat(0.1)),
    );
}

/// [`add_gun`] at a chosen camera-local pose.
fn add_gun_at(scene: &mut RenderScene, local: Mat4) {
    let mut gun = slab(Vec3::ZERO, 1.0, GUN_COLOR);
    gun.viewmodel = true;
    gun.cast_shadows = false;
    scene.meshes.push(gun);
    scene.viewmodel = Some(ViewmodelData {
        fov_y_deg: 55.0,
        near: 0.01,
        depth_slice: SLICE,
        projection: Mat4::perspective_rh(
            55.0_f32.to_radians(),
            crate::scene::aspect_ratio((PX, PX)),
            0.01,
            1000.0,
        ),
        locals: vec![ViewmodelLocal {
            mesh: scene.meshes.len() as u32 - 1,
            model: local,
            normal_matrix: Mat3::IDENTITY,
        }],
    });
    compose_viewmodel(scene, scene.camera.view.inverse());
}

/// The scene with the gun removed entirely.
fn without_gun(scene: &RenderScene) -> RenderScene {
    let mut off = scene.clone();
    off.meshes.pop();
    off.viewmodel = None;
    off
}

fn is_gun(rgb: [u8; 3]) -> bool {
    rgb[1] > 150 && rgb[0] < 100
}

/// A wall 0.2 in front of the eye, with the gun behind it in world space: the gun draws whole.
/// With the flag off the same mesh is an ordinary world mesh and the wall hides it. Skips with no
/// GPU.
#[test]
fn needs_gpu_a_gun_inside_a_wall_draws_whole() {
    let Some(renderer) = skip_or_renderer("a_gun_inside_a_wall_draws_whole") else {
        return;
    };
    let size = FrameSize::native(PX, PX);
    let mut scene = gun_scene();
    scene
        .meshes
        .push(slab(Vec3::new(0.0, 0.0, -1.7), 3.0, SLAB_COLOR));
    add_gun(&mut scene);
    let gun = renderer.render(&scene, size).expect("gun");
    let centre = rgb_at(&gun, PX / 2, PX / 2);
    assert!(is_gun(centre), "the gun is clipped by the wall: {centre:?}");

    let mut world = scene.clone();
    world.meshes.last_mut().unwrap().viewmodel = false;
    world.viewmodel = None;
    let world = renderer.render(&world, size).expect("world");
    let centre = rgb_at(&world, PX / 2, PX / 2);
    assert!(
        !is_gun(centre),
        "a world mesh behind the wall shows: {centre:?}"
    );
}

/// A cube 0.3 from the eye covers the gun's whole screen rect in world depth, and the gun still
/// draws over its middle while the cube shows around it. Skips with no GPU.
#[test]
fn needs_gpu_the_gun_draws_in_front_of_a_closer_mesh() {
    let Some(renderer) = skip_or_renderer("the_gun_draws_in_front_of_a_closer_mesh") else {
        return;
    };
    let size = FrameSize::native(PX, PX);
    let mut scene = gun_scene();
    scene
        .meshes
        .push(slab(Vec3::new(0.0, 0.0, -0.4), 0.2, SLAB_COLOR));
    add_gun(&mut scene);
    let image = renderer.render(&scene, size).expect("gun");
    let centre = rgb_at(&image, PX / 2, PX / 2);
    assert!(
        is_gun(centre),
        "the gun is hidden by the closer cube: {centre:?}"
    );
    let beside = rgb_at(&image, PX / 2 + 8, PX / 2);
    assert!(
        !is_gun(beside) && beside[0] > beside[1],
        "the cube shows beside the gun: {beside:?}"
    );
}

/// Fog and depth of field decode the remapped buffer correctly: outside the gun's footprint the
/// fogged, blurred frame is the one without the gun. The tolerance is the `[s, 1]` remap's depth
/// quantization of the world's depth, two LSB. Skips with no GPU.
#[test]
fn needs_gpu_fog_and_dof_on_the_world_are_unchanged_by_the_gun() {
    let Some(renderer) = skip_or_renderer("fog_and_dof_on_the_world_are_unchanged_by_the_gun")
    else {
        return;
    };
    let size = FrameSize::native(PX, PX);
    let mut scene = gun_scene();
    scene.meshes = fog_scene(PX, PX).meshes;
    add_gun(&mut scene);
    let bare = without_gun(&scene);

    let on = renderer.render(&scene, size).expect("gun, exact");
    let off = renderer.render(&bare, size).expect("bare, exact");
    let mut mask = vec![false; (PX * PX) as usize];
    for y in 0..PX {
        for x in 0..PX {
            mask[(y * PX + x) as usize] = rgb_at(&on, x, y) != rgb_at(&off, x, y);
        }
    }
    let footprint = mask.iter().filter(|&&m| m).count();
    assert!(footprint > 20, "the gun covers {footprint} pixels");
    // The blur's reach is under a pixel; three more keep its rim out of the comparison.
    let near_gun = |x: u32, y: u32| {
        (y.saturating_sub(3)..=(y + 3).min(PX - 1)).any(|ny| {
            (x.saturating_sub(3)..=(x + 3).min(PX - 1)).any(|nx| mask[(ny * PX + nx) as usize])
        })
    };

    let post = PostData {
        fog_density: 0.03,
        fog_color: FOG_COLOR,
        dof: 0.01,
        focus_distance: 8.0,
        ..exact_post()
    };
    scene.post = Some(post);
    let mut bare = bare;
    bare.post = Some(post);
    let on = renderer.render(&scene, size).expect("gun, fog + dof");
    let off = renderer.render(&bare, size).expect("bare, fog + dof");
    let mut worst = 0_i32;
    let mut checked = 0;
    for y in 0..PX {
        for x in 0..PX {
            if near_gun(x, y) {
                continue;
            }
            checked += 1;
            let (a, b) = (rgb_at(&on, x, y), rgb_at(&off, x, y));
            worst = worst.max(
                (0..3)
                    .map(|c| (i32::from(a[c]) - i32::from(b[c])).abs())
                    .max()
                    .unwrap(),
            );
        }
    }
    eprintln!("fog + dof outside the gun: {checked} pixels, worst {worst} LSB");
    assert!(checked > 3000, "checked {checked}");
    assert!(
        worst <= 2,
        "the gun moved the world's fog or blur: {worst} LSB"
    );
}

/// The gun made a lit surface: no emissive, so only light reaches it.
fn lit_gun_at(scene: &mut RenderScene, local: Mat4) {
    add_gun_at(scene, local);
    scene.ambient = [0.0; 3];
    scene.meshes.last_mut().unwrap().material = [1.0, 0.0, 0.0];
}

fn lit_gun(scene: &mut RenderScene) {
    lit_gun_at(
        scene,
        Mat4::from_translation(Vec3::new(0.0, 0.0, -0.45)) * Mat4::from_scale(Vec3::splat(0.1)),
    );
}

fn world_point(scene: &RenderScene, local: Vec3) -> Vec3 {
    scene.camera.view.inverse().transform_point3(local)
}

/// The pixels where `a` and `b` differ.
fn differing(a: &RenderedImage, b: &RenderedImage) -> Vec<(u32, u32)> {
    let mut out = Vec::new();
    for y in 0..PX {
        for x in 0..PX {
            if rgb_at(a, x, y) != rgb_at(b, x, y) {
                out.push((x, y));
            }
        }
    }
    out
}

fn brightest(image: &RenderedImage, pixels: &[(u32, u32)]) -> u8 {
    pixels
        .iter()
        .flat_map(|&(x, y)| rgb_at(image, x, y))
        .max()
        .unwrap_or(0)
}

fn point(position: Vec3, intensity: f32, range: f32) -> crate::scene::PointLightData {
    crate::scene::PointLightData {
        position,
        color: Vec3::ONE,
        intensity,
        range,
        dynamic: false,
        shadow: None,
    }
}

/// A point light behind the eye lights a gun whose lens position is nowhere near that light's world
/// tile. Skips with no GPU.
#[test]
fn needs_gpu_a_point_light_behind_the_eye_lights_the_gun() {
    let Some(renderer) = skip_or_renderer("a_point_light_behind_the_eye_lights_the_gun") else {
        return;
    };
    let size = FrameSize::native(PX, PX);
    let mut scene = gun_scene();
    lit_gun(&mut scene);
    let dark = renderer.render(&scene, size).expect("dark gun");
    assert!(
        rgb_at(&dark, PX / 2, PX / 2).iter().all(|&c| c < 4),
        "the gun is lit with no light: {:?}",
        rgb_at(&dark, PX / 2, PX / 2)
    );
    scene.lights = vec![point(Vec3::new(0.0, 0.0, 0.5), 4.0, 5.0)];
    let lit = renderer.render(&scene, size).expect("lit gun");
    let centre = rgb_at(&lit, PX / 2, PX / 2);
    assert!(
        centre[1] > 40,
        "the light does not reach the gun: {centre:?}"
    );
}

/// A gun drawn outside the world frustum still reads the small light beside it, and the culled
/// cluster build renders it byte-identically to the all-lights oracle. Skips with no GPU.
#[test]
fn needs_gpu_the_gun_reads_every_light_that_reaches_it() {
    let Some(renderer) = skip_or_renderer("the_gun_reads_every_light_that_reaches_it") else {
        return;
    };
    let size = FrameSize::native(PX, PX);
    let mut scene = gun_scene();
    scene.camera.projection = Mat4::perspective_rh(
        10.0_f32.to_radians(),
        crate::scene::aspect_ratio((PX, PX)),
        0.1,
        1000.0,
    );
    lit_gun_at(
        &mut scene,
        Mat4::from_translation(Vec3::new(0.15, -0.12, -0.45)) * Mat4::from_scale(Vec3::splat(0.06)),
    );
    let bare = without_gun(&scene);
    let mask = differing(
        &renderer.render(&scene, size).expect("gun"),
        &renderer.render(&bare, size).expect("bare"),
    );
    assert!(mask.len() > 20, "the gun covers {} pixels", mask.len());

    scene.lights = vec![point(
        world_point(&scene, Vec3::new(0.15, -0.12, -0.40)),
        4.0,
        0.05,
    )];
    renderer.cluster_build.set(ClusterBuild::Culled);
    let culled = renderer.render(&scene, size).expect("culled");
    renderer.cluster_build.set(ClusterBuild::Degenerate);
    let oracle = renderer.render(&scene, size).expect("oracle");
    renderer.cluster_build.set(ClusterBuild::Culled);
    let lit = brightest(&culled, &mask);
    assert!(
        lit > 40,
        "the light beside the gun does not reach it: {lit}"
    );
    assert_eq!(culled.rgba, oracle.rgba, "the viewmodel cell lost a light");
}

/// A sun shining through a wall 0.25 ahead of the eye: the gun, poking through it, takes the sun at
/// the eye, so every gun pixel equals the wall-free frame's. Skips with no GPU.
#[test]
fn needs_gpu_a_wall_does_not_shadow_the_gun() {
    let Some(renderer) = skip_or_renderer("a_wall_does_not_shadow_the_gun") else {
        return;
    };
    let size = FrameSize::native(PX, PX);
    let mut scene = gun_scene();
    scene.sun = Some(test_sun(Vec3::NEG_Z, Vec3::splat(3.0), 0.002));
    lit_gun(&mut scene);
    let mut walled = scene.clone();
    let mut wall = slab(Vec3::ZERO, 1.0, SLAB_COLOR);
    wall.model = Mat4::from_translation(Vec3::new(0.0, 0.0, -0.25))
        * Mat4::from_scale(Vec3::new(4.0, 4.0, 0.05));
    wall.material = [1.0, 0.0, 0.0];
    walled.meshes.insert(0, wall);
    walled.viewmodel.as_mut().unwrap().locals[0].mesh += 1;

    let with_wall = renderer.render(&walled, size).expect("wall");
    let clear = renderer.render(&scene, size).expect("clear");
    let mut bare = without_gun(&walled);
    bare.viewmodel = None;
    let mask = differing(&with_wall, &renderer.render(&bare, size).expect("bare"));
    assert!(mask.len() > 20, "the gun covers {} pixels", mask.len());
    assert!(
        brightest(&clear, &mask) > 40,
        "the sun does not light the gun"
    );
    for &(x, y) in &mask {
        assert_eq!(rgb_at(&with_wall, x, y), rgb_at(&clear, x, y), "({x}, {y})");
    }
}

/// An occluder between the sun and the eye alone darkens the gun, uniformly. Skips with no GPU.
#[test]
fn needs_gpu_an_eye_in_shadow_darkens_the_whole_gun() {
    let Some(renderer) = skip_or_renderer("an_eye_in_shadow_darkens_the_whole_gun") else {
        return;
    };
    let size = FrameSize::native(PX, PX);
    let mut scene = gun_scene();
    scene.sun = Some(test_sun(
        Vec3::new(-1.0, 0.0, -1.0),
        Vec3::splat(3.0),
        0.002,
    ));
    lit_gun(&mut scene);
    let sun_side = Vec3::new(1.0, 0.0, 1.0).normalize();
    let mut blocked = scene.clone();
    let mut occluder = slab(sun_side * 0.3, 0.2, SLAB_COLOR);
    occluder.material = [1.0, 0.0, 0.0];
    blocked.meshes.insert(0, occluder);
    blocked.viewmodel.as_mut().unwrap().locals[0].mesh += 1;

    let lit = renderer.render(&scene, size).expect("lit");
    let dark = renderer.render(&blocked, size).expect("blocked");
    let mask = differing(
        &lit,
        &renderer.render(&without_gun(&scene), size).expect("bare"),
    );
    assert!(mask.len() > 20, "the gun covers {} pixels", mask.len());
    assert!(
        brightest(&lit, &mask) > 40,
        "the sun does not light the gun"
    );
    for &(x, y) in &mask {
        let (l, d) = (rgb_at(&lit, x, y), rgb_at(&dark, x, y));
        assert!(
            d.iter().map(|&c| u32::from(c)).sum::<u32>()
                < l.iter().map(|&c| u32::from(c)).sum::<u32>()
                && d.iter().all(|&c| c <= 2),
            "({x}, {y}): {l:?} -> {d:?}"
        );
    }
}

/// Sun cascades and a shadowed spot light on a floor and a caster: outside the gun's own footprint
/// the frame is the one without the gun. Skips with no GPU.
#[test]
fn needs_gpu_world_shadows_are_unchanged_by_the_gun() {
    let Some(renderer) = sun_renderer("world_shadows_are_unchanged_by_the_gun", 512, 3) else {
        return;
    };
    let size = FrameSize::native(PX, PX);
    let mut scene = gun_scene();
    let mut sun = test_sun(Vec3::new(-0.4, -1.0, -0.6), Vec3::splat(2.0), 0.002);
    sun.cascades = Some(crate::scene::SunCascadeConfig {
        count: 3,
        shadow_distance: 20.0,
        splits: None,
        shadow_near: -100.0,
        shadow_far: 100.0,
    });
    scene.sun = Some(sun);
    let mut floor = ground(12.0);
    floor.model = Mat4::from_translation(Vec3::new(0.0, -1.0, -3.0)) * floor.model;
    let caster = cube(
        Vec3::new(0.3, -0.5, -2.5),
        Vec3::splat(0.8),
        [0.8, 0.3, 0.2, 1.0],
    );
    scene.meshes = vec![floor, caster];
    let mut spot = spot_light(
        Vec3::new(-1.5, 1.5, -1.0),
        Vec3::new(0.5, -1.0, -1.0),
        30.0,
        12.0,
        20.0,
        45.0,
    );
    spot.shadow = Some(crate::scene::LocalShadowData {
        bias: 0.02,
        near: 0.1,
        distance: None,
        fade: 0.0,
    });
    scene.spot_lights = vec![spot];

    let mut footprint_scene = gun_scene();
    add_gun(&mut footprint_scene);
    let footprint = differing(
        &renderer.render(&footprint_scene, size).expect("footprint"),
        &renderer
            .render(&without_gun(&footprint_scene), size)
            .expect("empty"),
    );
    assert!(
        footprint.len() > 20,
        "the gun covers {} pixels",
        footprint.len()
    );

    add_gun(&mut scene);
    let on = renderer.render(&scene, size).expect("gun");
    let off = renderer.render(&without_gun(&scene), size).expect("bare");
    let lit_world = differing(
        &off,
        &renderer.render(&without_floor(&scene), size).expect("dark"),
    );
    assert!(
        lit_world.len() > 500,
        "the world is lit over {} pixels",
        lit_world.len()
    );
    let covered = |x: u32, y: u32| {
        footprint
            .iter()
            .any(|&(fx, fy)| fx.abs_diff(x) <= 1 && fy.abs_diff(y) <= 1)
    };
    for y in 0..PX {
        for x in 0..PX {
            if !covered(x, y) {
                assert_eq!(rgb_at(&on, x, y), rgb_at(&off, x, y), "({x}, {y})");
            }
        }
    }
}

fn without_floor(scene: &RenderScene) -> RenderScene {
    let mut out = scene.clone();
    out.sun = None;
    out.spot_lights.clear();
    out
}
