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
    let local =
        Mat4::from_translation(Vec3::new(0.0, 0.0, -0.45)) * Mat4::from_scale(Vec3::splat(0.1));
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
