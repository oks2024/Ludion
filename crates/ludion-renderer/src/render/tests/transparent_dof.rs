//! Depth of field at each transparent's own distance: GPU tests.

use super::*;

// --- Depth of field at each transparent's own distance -----------------------------------

/// The frame edge of the depth-of-field fixtures: large enough that a sprite spans tens of pixels
/// and a checker cell is a few.
const COC_PX: u32 = 128;
/// The aperture the fixtures use: the far field's disc radius is `COC_DOF · height` pixels.
const COC_DOF: f32 = 0.03;

/// The exact-output post with depth of field on and no fog.
fn coc_post(dof: f32, focus: f32) -> PostData {
    PostData {
        dof,
        focus_distance: focus,
        ..exact_post()
    }
}

/// The same scene with the aperture closed: every distance in focus, no pass built.
fn without_dof(scene: &RenderScene) -> RenderScene {
    let mut off = scene.clone();
    if let Some(post) = off.post.as_mut() {
        post.dof = 0.0;
    }
    off
}

/// The pixels a disc of the largest circle of confusion can reach, plus one for rounding.
fn disc_px(height: u32) -> u32 {
    (COC_DOF * height as f32).ceil() as u32 + 1
}

/// A black and white checker, `cell` texels a square, sampled nearest.
fn checker_texture(side: u32, cell: u32) -> Arc<TextureImage> {
    let mut rgba = Vec::new();
    for y in 0..side {
        for x in 0..side {
            let v = if (x / cell + y / cell).is_multiple_of(2) {
                255
            } else {
                0
            };
            rgba.extend_from_slice(&[v, v, v, 255]);
        }
    }
    Arc::new(TextureImage {
        width: side,
        height: side,
        rgba,
        hash: 0xC0C0,
        filter_linear: false,
        premultiplied_on_load: true,
        mean: [1.0; 4],
    })
}

/// A square sprite wearing `scene.textures[0]`, the checker.
fn checker_sprite(center: Vec3, size: f32, alpha: f32) -> crate::scene::SpriteInstance {
    let mut sprite = sized_sprite(center, size, [1.0, 1.0, 1.0, alpha], false);
    sprite.texture = Some(0);
    sprite
}

/// A checker sprite `distance` in front of the camera, 0.35 of that wide (about 39 px), focus at
/// `focus`, over a wall whose face is `wall` away when given and over the open sky otherwise.
fn checker_scene(distance: f32, focus: f32, wall: Option<f32>, alpha: f32) -> RenderScene {
    let mut scene = fog_scene(COC_PX, COC_PX);
    scene.meshes.clear();
    scene.post = Some(coc_post(COC_DOF, focus));
    scene.textures = vec![checker_texture(16, 2)];
    scene.sprites = vec![checker_sprite(
        Vec3::new(0.0, 0.0, -distance),
        distance * 0.35,
        alpha,
    )];
    if let Some(wall) = wall {
        scene.meshes = vec![slab(
            Vec3::new(0.0, 0.0, -(wall + 30.0)),
            60.0,
            [0.25, 0.5, 0.25],
        )];
    }
    scene
}

/// The checker on the focal plane under an orthographic camera, 12 units (about 38 px) wide: a
/// plane perpendicular to the view, so its circle of confusion is exactly zero everywhere and a
/// byte comparison against the unblurred frame is meaningful. `wall` is the wall's face distance.
fn ortho_checker_scene(wall: Option<f32>) -> RenderScene {
    let mut scene = checker_scene(10.0, 10.0, wall, 1.0);
    scene.camera = ortho_fog_camera();
    scene.sprites[0].right = Vec3::new(12.0, 0.0, 0.0);
    scene.sprites[0].up = Vec3::new(0.0, 12.0, 0.0);
    scene
}

/// The sprite's footprint in pixels as `(x0, y0, x1, y1)`, shrunk by `inset` on every side.
fn sprite_rect(scene: &RenderScene, inset: u32) -> (u32, u32, u32, u32) {
    let s = &scene.sprites[0];
    let corners = [(-0.5, -0.5), (0.5, -0.5), (-0.5, 0.5), (0.5, 0.5)].map(|(a, b)| {
        project(
            &scene.camera,
            s.center + s.right * a + s.up * b,
            COC_PX,
            COC_PX,
        )
    });
    let min = |f: fn(&(u32, u32)) -> u32| corners.iter().map(f).min().unwrap();
    let max = |f: fn(&(u32, u32)) -> u32| corners.iter().map(f).max().unwrap();
    (
        min(|c| c.0) + inset,
        min(|c| c.1) + inset,
        max(|c| c.0) - inset,
        max(|c| c.1) - inset,
    )
}

/// The mean absolute luma step to the right and downward neighbour over `rect` (end exclusive):
/// what a blur removes.
fn edge_energy(img: &RenderedImage, rect: (u32, u32, u32, u32)) -> f64 {
    let (x0, y0, x1, y1) = rect;
    let mut sum = 0.0;
    for y in y0..y1 {
        for x in x0..x1 {
            let p = f64::from(luma(img, x, y));
            sum += (f64::from(luma(img, x + 1, y)) - p).abs()
                + (f64::from(luma(img, x, y + 1)) - p).abs();
        }
    }
    sum / f64::from((x1 - x0) * (y1 - y0))
}

/// The largest channel gap between two images over `rect` (end exclusive).
fn rect_gap(a: &RenderedImage, b: &RenderedImage, rect: (u32, u32, u32, u32)) -> u8 {
    let (x0, y0, x1, y1) = rect;
    (y0..y1)
        .flat_map(|y| (x0..x1).map(move |x| (x, y)))
        .map(|(x, y)| channel_gap(rgb_at(a, x, y), rgb_at(b, x, y)))
        .max()
        .unwrap()
}

/// **The fog arm's neutrality proof.** An in-focus sprite under an orthographic camera has a
/// circle of confusion of exactly zero (its distance, 8, is exact in half precision), so the
/// frame that schedules the layer pass with fog off must equal, byte for byte inside the sprite,
/// the frame that does not: the composite's fog arm and the blend are unreachable. Skips with
/// no GPU.
#[test]
fn needs_gpu_an_in_focus_sprite_with_fog_off_is_byte_identical_to_dof_off() {
    let Some(renderer) =
        skip_or_renderer("an_in_focus_sprite_with_fog_off_is_byte_identical_to_dof_off")
    else {
        return;
    };
    let (w, h) = (64u32, 64u32);
    let mut on = fog_scene(w, h);
    on.camera = ortho_fog_camera();
    on.meshes = vec![slab(Vec3::new(0.0, 0.0, -60.0), 20.0, SLAB_COLOR)];
    on.post = Some(coc_post(COC_DOF, 8.0));
    on.sprites = vec![sized_sprite(
        Vec3::new(0.0, 0.0, -8.0),
        16.0,
        OPAQUE_SLAB,
        false,
    )];
    let off = without_dof(&on);
    assert!(
        PostChain::for_scene(&on).transparent_depth_scheduled(),
        "depth of field with a sprite must schedule the layer pass"
    );
    assert!(!PostChain::for_scene(&off).transparent_depth_scheduled());

    let size = FrameSize::native(w, h);
    let a = renderer.render(&on, size).expect("dof on");
    let b = renderer.render(&off, size).expect("dof off");
    let inset = disc_px(h) + 1;
    let rect = (
        32 - 12 + inset,
        32 - 12 + inset,
        32 + 12 - inset,
        32 + 12 - inset,
    );
    assert_eq!(rect_gap(&a, &b, rect), 0, "the sprite's interior moved");
    assert_ne!(
        a.rgba, b.rgba,
        "the wall is defocused, or nothing was proved"
    );
}

/// **Sharpness.** A sprite on the focal plane keeps its 2-texel checker over a defocused wall.
/// Skips with no GPU.
#[test]
fn needs_gpu_an_in_focus_sprite_stays_sharp() {
    let Some(renderer) = skip_or_renderer("an_in_focus_sprite_stays_sharp") else {
        return;
    };
    let on = checker_scene(10.0, 10.0, Some(60.0), 1.0);
    let off = without_dof(&on);
    let size = FrameSize::native(COC_PX, COC_PX);
    let rect = sprite_rect(&on, disc_px(COC_PX) + 1);
    let sharp = edge_energy(&renderer.render(&off, size).expect("off"), rect);
    let kept = edge_energy(&renderer.render(&on, size).expect("on"), rect);
    eprintln!(
        "sharpness ratio {:.4} ({kept:.2} / {sharp:.2})",
        kept / sharp
    );
    assert!(
        kept / sharp >= 0.95,
        "the in-focus sprite lost its edges: {kept:.2} against {sharp:.2}"
    );
}

/// A sprite away from focus takes one circle of confusion whatever sits behind it: the
/// checker's blur is the same over a wall and over the open sky. Skips with no GPU.
#[test]
fn needs_gpu_a_sprite_takes_the_same_coc_over_a_wall_and_over_the_sky() {
    let Some(renderer) =
        skip_or_renderer("a_sprite_takes_the_same_coc_over_a_wall_and_over_the_sky")
    else {
        return;
    };
    let size = FrameSize::native(COC_PX, COC_PX);
    let wall = checker_scene(20.0, 10.0, Some(60.0), 1.0);
    let sky = checker_scene(20.0, 10.0, None, 1.0);
    let rect = sprite_rect(&wall, disc_px(COC_PX) + 1);
    let over_wall = renderer.render(&wall, size).expect("wall");
    let over_sky = renderer.render(&sky, size).expect("sky");
    let sharp = edge_energy(
        &renderer.render(&without_dof(&wall), size).expect("off"),
        rect,
    );
    let blurred = edge_energy(&over_wall, rect);
    let gap = rect_gap(&over_wall, &over_sky, rect);
    eprintln!("blur kept {:.3}, wall/sky gap {gap}", blurred / sharp);
    assert!(
        blurred / sharp < 0.9,
        "the sprite must be defocused to mean anything"
    );
    assert!(
        gap <= 2,
        "the sprite's blur follows what is behind it: gap {gap} LSB"
    );
}

/// An in-focus sprite across a wall's top edge has no step in its blur there: its interior
/// equals the unblurred frame on both sides. Skips with no GPU.
#[test]
fn needs_gpu_a_sprite_straddling_the_horizon_has_no_coc_seam() {
    let Some(renderer) = skip_or_renderer("a_sprite_straddling_the_horizon_has_no_coc_seam") else {
        return;
    };
    let mut on = ortho_checker_scene(None);
    on.meshes = vec![slab(Vec3::new(0.0, -30.0, -60.0), 60.0, [0.25, 0.5, 0.25])];
    let size = FrameSize::native(COC_PX, COC_PX);
    let rect = sprite_rect(&on, disc_px(COC_PX) + 1);
    let a = renderer.render(&on, size).expect("on");
    let b = renderer.render(&without_dof(&on), size).expect("off");
    let mid = (rect.1 + rect.3) / 2;
    let above = rect_gap(&a, &b, (rect.0, rect.1, rect.2, mid));
    let below = rect_gap(&a, &b, (rect.0, mid, rect.2, rect.3));
    eprintln!("seam: above {above} below {below}");
    assert!(
        above <= 1 && below <= 1,
        "a step across the wall's edge: {above} / {below}"
    );
}

/// A sprite well nearer than focus spreads over the background past its silhouette, though the
/// wall behind it is on the focal plane. Skips with no GPU.
#[test]
fn needs_gpu_a_near_sprite_blurs_past_its_silhouette() {
    let Some(renderer) = skip_or_renderer("a_near_sprite_blurs_past_its_silhouette") else {
        return;
    };
    let mut on = fog_scene(COC_PX, COC_PX);
    on.meshes = vec![slab(Vec3::new(0.0, 0.0, -40.0), 60.0, [0.0, 0.0, 0.0])];
    on.post = Some(coc_post(COC_DOF, 10.0));
    on.sprites = vec![sized_sprite(
        Vec3::new(0.0, 0.0, -5.0),
        1.75,
        [1.0, 1.0, 1.0, 1.0],
        false,
    )];
    let size = FrameSize::native(COC_PX, COC_PX);
    let a = renderer.render(&on, size).expect("on");
    let b = renderer.render(&without_dof(&on), size).expect("off");
    let row = COC_PX / 2;
    let edge = (0..COC_PX)
        .rev()
        .find(|&x| luma(&b, x, row) > 600)
        .expect("the sprite");
    let spread = luma(&a, edge + 1, row).saturating_sub(luma(&b, edge + 1, row));
    eprintln!("near spread {spread} one pixel past the edge at x={edge}");
    assert!(spread >= 200, "the near sprite does not spread: {spread}");
}

/// A soft sprite in focus keeps a circle of confusion between its own and the wall's, and its
/// interior has no spike. Two bounds: a blur never raises a pixel step above the sharp frame's
/// (plus rounding), and the checker keeps over 0.2 of its edge energy, about twice what the
/// wall's full blur leaves (0.108 measured with the old code), since the circle of confusion is
/// the half-weight mix of the wall's and the sprite's zero.
/// Skips with no GPU.
#[test]
fn needs_gpu_a_soft_in_focus_sprite_is_continuous() {
    let Some(renderer) = skip_or_renderer("a_soft_in_focus_sprite_is_continuous") else {
        return;
    };
    let on = checker_scene(10.0, 10.0, Some(60.0), 0.5);
    let size = FrameSize::native(COC_PX, COC_PX);
    let rect = sprite_rect(&on, disc_px(COC_PX) + 1);
    let a = renderer.render(&on, size).expect("on");
    let b = renderer.render(&without_dof(&on), size).expect("off");
    let max_step = |img: &RenderedImage| {
        let (x0, y0, x1, y1) = rect;
        (y0..y1)
            .flat_map(|y| (x0..x1).map(move |x| (x, y)))
            .map(|(x, y)| {
                let p = i64::from(luma(img, x, y));
                (i64::from(luma(img, x + 1, y)) - p)
                    .abs()
                    .max((i64::from(luma(img, x, y + 1)) - p).abs())
            })
            .max()
            .unwrap()
    };
    let (on_step, off_step) = (max_step(&a), max_step(&b));
    let kept = edge_energy(&a, rect) / edge_energy(&b, rect);
    eprintln!("soft: steps {on_step} / {off_step}, sharpness kept {kept:.3}");
    assert!(
        on_step <= off_step + 3,
        "a spike: {on_step} against {off_step}"
    );
    assert!(
        kept > 0.2,
        "the soft sprite is blurred like the bare wall: {kept:.3}"
    );
}

/// In-focus particles and world text stay as sharp as they would with the aperture closed, over a
/// far wall: every pixel two texels inside the layer's coverage equals the unblurred frame. Skips with no GPU.
#[test]
fn needs_gpu_in_focus_particles_and_world_text_stay_clear_of_the_background_blur() {
    let Some(renderer) =
        skip_or_renderer("in_focus_particles_and_world_text_stay_clear_of_the_background_blur")
    else {
        return;
    };
    let size = FrameSize::native(COC_PX, COC_PX);
    let mut base = fog_scene(COC_PX, COC_PX);
    base.camera = ortho_fog_camera();
    base.camera.projection = Mat4::orthographic_rh(-8.0, 8.0, -8.0, 8.0, 0.1, 1000.0);
    base.meshes = vec![slab(Vec3::new(0.0, 0.0, -90.0), 60.0, [0.25, 0.5, 0.25])];
    base.post = Some(coc_post(COC_DOF, 10.0));
    let mut particles = base.clone();
    particles.particles = vec![particle(
        Vec3::new(0.0, 0.0, -10.0),
        5.0,
        [1.0, 1.0, 1.0, 1.0],
        0.0,
    )];
    let mut text = base.clone();
    text.texts = vec![layer_text(-10.0)];
    for (what, scene) in [("particle", particles), ("text", text)] {
        // A rim pixel shares a half-extent block with partly covered texels, which take a mixed
        // circle of confusion; the interior is what stays clear.
        let texels = layer_texels(&renderer, &scene, size);
        let full = |x: u32, y: u32| texels[(y * COC_PX + x) as usize][1] > 0.999;
        let covered: Vec<(u32, u32)> = covered_pixels(&texels, COC_PX)
            .into_iter()
            .filter(|&(x, y)| {
                (x.saturating_sub(2)..=x + 2).all(|nx| {
                    (y.saturating_sub(2)..=y + 2)
                        .all(|ny| full(nx.min(COC_PX - 1), ny.min(COC_PX - 1)))
                })
            })
            .collect();
        assert!(
            covered.len() > 30,
            "{what} covers some pixels outright: {}",
            covered.len()
        );
        let a = renderer.render(&scene, size).expect("on");
        let b = renderer.render(&without_dof(&scene), size).expect("off");
        let worst = covered
            .iter()
            .map(|&(x, y)| channel_gap(rgb_at(&a, x, y), rgb_at(&b, x, y)))
            .max()
            .unwrap();
        eprintln!("{what}: worst gap {worst} over {} pixels", covered.len());
        assert!(worst <= 2, "{what} took the background's blur: {worst} LSB");
    }
}

/// A 2×2 block with no layer coverage runs the unchanged code: a wall with a small sprite in
/// front and the same wall without it are byte-identical beyond the disc and two target texels
/// from the sprite. Skips with no GPU.
#[test]
fn needs_gpu_an_uncovered_block_is_byte_identical_with_the_layer_on() {
    let Some(renderer) = skip_or_renderer("an_uncovered_block_is_byte_identical_with_the_layer_on")
    else {
        return;
    };
    let mut bare = fog_scene(COC_PX, COC_PX);
    bare.meshes = vec![
        slab(Vec3::new(-10.0, 0.0, -45.0), 10.0, SLAB_COLOR),
        slab(Vec3::new(12.0, 2.0, -45.0), 12.0, [0.25, 0.5, 0.25]),
    ];
    bare.post = Some(coc_post(COC_DOF, 10.0));
    let mut with = bare.clone();
    with.sprites = vec![sized_sprite(
        Vec3::new(0.0, 0.0, -10.0),
        1.2,
        OPAQUE_SLAB,
        false,
    )];
    assert!(PostChain::for_scene(&with).transparent_depth_scheduled());
    let size = FrameSize::native(COC_PX, COC_PX);
    let a = renderer.render(&bare, size).expect("bare");
    let b = renderer.render(&with, size).expect("with");
    let (x0, y0, x1, y1) = sprite_rect(&with, 0);
    let reach = disc_px(COC_PX) + 4;
    let mut checked = 0;
    for y in 0..COC_PX {
        for x in 0..COC_PX {
            let far = x + reach < x0 || x > x1 + reach || y + reach < y0 || y > y1 + reach;
            if far {
                checked += 1;
                assert_eq!(pixel(&a, x, y), pixel(&b, x, y), "pixel ({x}, {y}) moved");
            }
        }
    }
    assert!(checked > 8000);
    assert_ne!(a.rgba, b.rgba, "the sprite is visible");
}

/// The live path: toggling the layer pass for depth of field alone leaves every frame equal to
/// its steady twin. Skips with no GPU.
#[test]
fn needs_gpu_a_streamed_dof_flip_leaves_the_image_untouched() {
    let mut bare = fog_scene(COC_PX, COC_PX);
    bare.meshes = vec![slab(Vec3::new(0.0, 0.0, -90.0), 60.0, [0.25, 0.5, 0.25])];
    bare.post = Some(coc_post(COC_DOF, 10.0));
    let mut with = bare.clone();
    with.sprites = vec![sized_sprite(
        Vec3::new(0.0, 0.0, -10.0),
        3.0,
        OPAQUE_SLAB,
        false,
    )];
    streamed_flip_is_neutral(
        "a_streamed_dof_flip_leaves_the_image_untouched",
        FrameSize::native(COC_PX, COC_PX),
        &bare,
        &with,
    );
}

/// Under an orthographic camera the circle of confusion measures depth along the view: a row of
/// slabs on the focal plane, wide in the frame, equals the unblurred frame. The old measure
/// fans out from the eye and blurs the sides. Skips with no GPU.
#[test]
fn needs_gpu_ortho_coc_measures_along_the_view() {
    let Some(renderer) = skip_or_renderer("ortho_coc_measures_along_the_view") else {
        return;
    };
    let (w, h) = (64u32, 64u32);
    let mut on = fog_scene(w, h);
    on.camera = ortho_fog_camera();
    on.meshes = (0..10)
        .map(|i| {
            let mut stripe = slab(
                Vec3::new(-18.0 + 4.0 * i as f32, 0.0, -7.0),
                1.0,
                if i % 2 == 0 {
                    SLAB_COLOR
                } else {
                    [0.0625, 0.5, 0.0625]
                },
            );
            stripe.model = Mat4::from_translation(Vec3::new(-18.0 + 4.0 * i as f32, 0.0, -7.0))
                * Mat4::from_scale(Vec3::new(4.0, 44.0, 4.0));
            stripe
        })
        .collect();
    on.post = Some(coc_post(COC_DOF, 5.0));
    let size = FrameSize::native(w, h);
    let a = renderer.render(&on, size).expect("on");
    let b = renderer.render(&without_dof(&on), size).expect("off");
    let gap = rect_gap(&a, &b, (0, 0, w, h));
    eprintln!("ortho plane gap {gap}");
    assert_eq!(a.rgba, b.rgba, "the plane on the focus moved: gap {gap}");
}

/// Fog and depth of field together: a sprite on the focal plane over a far wall equals the
/// unblurred frame within one step. Skips with no GPU.
#[test]
fn needs_gpu_fog_and_dof_agree_on_a_sprite() {
    let Some(renderer) = skip_or_renderer("fog_and_dof_agree_on_a_sprite") else {
        return;
    };
    let mut on = ortho_checker_scene(Some(60.0));
    on.post = Some(PostData {
        dof: COC_DOF,
        focus_distance: 10.0,
        ..own_fog_post(0.03, FOG_COLOR)
    });
    let size = FrameSize::native(COC_PX, COC_PX);
    let rect = sprite_rect(&on, disc_px(COC_PX) + 1);
    let a = renderer.render(&on, size).expect("on");
    let b = renderer.render(&without_dof(&on), size).expect("off");
    let gap = rect_gap(&a, &b, rect);
    eprintln!("fog + dof gap {gap}");
    assert!(
        gap <= 1,
        "fog and depth of field disagree on the sprite: {gap} LSB"
    );
}

/// The gallery's mirror floor at 1080p: a back wall, a sphere and a cube, all rough so their own
/// texels early-out, under the gallery camera. The same scene the confidence view is read from.
fn mirror_floor_gallery() -> (FrameSize, RenderScene) {
    let size = FrameSize::native(1920, 1080);
    let mut scene = RenderScene::empty(size.render);
    scene.post = Some(PostData {
        ssr: 1.0,
        ..exact_post()
    });
    scene.environment = Some(test_env());
    let eye = Vec3::new(0.0, 5.0, 11.0);
    let pitch = 25.0_f32.to_radians();
    scene.camera = crate::scene::RenderCamera {
        view: Mat4::look_at_rh(
            eye,
            eye + Vec3::new(0.0, -pitch.sin(), -pitch.cos()),
            Vec3::Y,
        ),
        projection: Mat4::perspective_rh(45.0_f32.to_radians(), 16.0 / 9.0, 0.1, 1000.0),
        position: eye,
    };
    let mut floor = ground(50.0);
    floor.material = [0.02, 1.0, 0.0];
    let rough = |mut m: MeshInstance, shape: Option<MeshShape>| {
        m.material = [0.9, 0.0, 0.0];
        if let Some(shape) = shape {
            m.shape = shape;
        }
        m
    };
    let wall = rough(
        cube(
            Vec3::new(0.0, 3.0, -7.0),
            Vec3::new(18.0, 6.0, 0.4),
            [0.6; 4],
        ),
        None,
    );
    let ball = rough(
        cube(
            Vec3::new(-3.0, 0.8, -3.0),
            Vec3::splat(1.6),
            [0.8, 0.2, 0.2, 1.0],
        ),
        Some(MeshShape::Sphere),
    );
    let block = rough(
        cube(
            Vec3::new(6.0, 0.8, -2.0),
            Vec3::splat(1.6),
            [0.2, 0.2, 0.8, 1.0],
        ),
        None,
    );
    scene.meshes = vec![floor, wall, ball, block];
    (size, scene)
}

/// The trace texel `(x, y)` of a gallery trace at `(tw, th)` as a camera-ray hit test against the
/// sphere at `centre` of `radius`, through the texel's source pixel (the trace's own mapping).
fn source_ray_hits_sphere(
    camera: &crate::scene::RenderCamera,
    size: FrameSize,
    (tw, th): (u32, u32),
    (x, y): (u32, u32),
    centre: Vec3,
    radius: f32,
) -> bool {
    let (rw, rh) = size.render;
    let px = (((x as f32 + 0.5) * rw as f32 / tw as f32).floor()).min(rw as f32 - 1.0);
    let py = (((y as f32 + 0.5) * rh as f32 / th as f32).floor()).min(rh as f32 - 1.0);
    let ndc = ludion_core::math::glam::Vec2::new(
        (px + 0.5) / rw as f32 * 2.0 - 1.0,
        1.0 - (py + 0.5) / rh as f32 * 2.0,
    );
    let inv = (camera.projection * camera.view).inverse();
    let unproject = |z: f32| {
        let p = inv * ludion_core::math::glam::Vec4::new(ndc.x, ndc.y, z, 1.0);
        p.truncate() / p.w
    };
    let near = unproject(0.0);
    let dir = (unproject(1.0) - near).normalize();
    let oc = near - centre;
    let b = oc.dot(dir);
    b * b - (oc.dot(oc) - radius * radius) >= 0.0 && -b > 0.0
}

/// Under a mirror-floor sphere, scanning a column from the reflection's centre up to the sphere
/// itself: sphere hits, then a thickness ramp, then a clean miss band. Hit attributes from the
/// wrong render pixel at the silhouette used to black out texels in between, a comb of
/// confidence crossing 0.5 many times in one column. Measured over the 69 columns scanned: 36
/// columns crossed more than once (up to 7 times) before the fix, 3 after (3 times each, where
/// the rim's facing ramp meets the thickness ramp).
#[test]
fn needs_gpu_ssr_trace_has_no_comb_under_a_sphere() {
    let Some(renderer) = skip_or_renderer("ssr_trace_has_no_comb_under_a_sphere") else {
        return;
    };
    let (size, scene) = mirror_floor_gallery();
    let (tw, th, texels) = ssr_trace_texels(&renderer, &scene, size);
    let centre = Vec3::new(-3.0, 0.8, -3.0);
    let clip = scene.camera.projection * scene.camera.view * centre.extend(1.0);
    let radius_px = 0.8 * scene.camera.projection.y_axis.y * 0.5 * th as f32 / clip.w;
    let (cx, _) = project(&scene.camera, centre, tw, th);
    let (_, start_y) = project(&scene.camera, Vec3::new(-3.0, -0.8, -3.0), tw, th);
    let (mut columns, mut combed) = (0, 0);
    for x in (cx - radius_px as u32 + 2)..=(cx + radius_px as u32 - 2) {
        let mut crossings = 0;
        let mut previous = None;
        for y in (0..=start_y).rev() {
            if source_ray_hits_sphere(&scene.camera, size, (tw, th), (x, y), centre, 0.8) {
                break;
            }
            let confident = texels[(y * tw + x) as usize][3] > 0.5;
            match previous {
                None => assert!(confident, "column {x}: the first texel {y} is not a hit"),
                Some(p) if p != confident => crossings += 1,
                _ => {}
            }
            previous = Some(confident);
        }
        assert!(
            crossings <= 3,
            "column {x}: confidence crosses 0.5 {crossings} times"
        );
        combed += usize::from(crossings > 1);
        columns += 1;
    }
    assert!(columns >= 20, "only {columns} columns scanned");
    assert!(combed <= 4, "{combed} of {columns} columns are combed");
}

/// Beside a reflected cube edge the march used to run out of iterations on rays grazing the
/// silhouette, leaving a miss band between two hits. Counts the sandwiched misses in the rows
/// between the cube's bottom-front edge and its mirror image's bottom. Measured: 43 before the
/// fix, 1 after.
#[test]
fn needs_gpu_ssr_trace_has_no_gap_beside_a_reflected_edge() {
    let Some(renderer) = skip_or_renderer("ssr_trace_has_no_gap_beside_a_reflected_edge") else {
        return;
    };
    let (size, scene) = mirror_floor_gallery();
    let (tw, th, texels) = ssr_trace_texels(&renderer, &scene, size);
    let (_, top) = project(&scene.camera, Vec3::new(6.0, 0.0, -1.2), tw, th);
    let (_, bottom) = project(&scene.camera, Vec3::new(6.0, -1.6, -1.2), tw, th);
    let mut xs = (u32::MAX, 0);
    for corner in 0..8 {
        let at = Vec3::new(
            if corner & 1 == 0 { 5.2 } else { 6.8 },
            if corner & 2 == 0 { 0.0 } else { -1.6 },
            if corner & 4 == 0 { -2.8 } else { -1.2 },
        );
        let (x, _) = project(&scene.camera, at, tw, th);
        xs = (xs.0.min(x), xs.1.max(x));
    }
    let (lo, hi) = (xs.0.saturating_sub(12), (xs.1 + 12).min(tw - 1));
    let mut sandwiched = 0;
    for y in top..=bottom {
        let row = |x: u32| texels[(y * tw + x) as usize][3] > 0.5;
        let first = (lo..=hi).find(|&x| row(x));
        let last = (lo..=hi).rev().find(|&x| row(x));
        if let (Some(first), Some(last)) = (first, last) {
            sandwiched += (first..=last).filter(|&x| !row(x)).count();
        }
    }
    assert!(sandwiched <= 4, "{sandwiched} sandwiched misses");
}

/// The SSR trace finds a box in a mirror floor: the floor just in front of the cube's base reflects
/// its red front face (confident, red-dominant), floor well to the side reflects only the sky above
/// the frame (confidence 0), and the sky itself is exactly zero. The trace is read back from a
/// harness graph that makes it an output.
#[test]
fn needs_gpu_ssr_trace_hits_a_box_in_a_mirror_floor() {
    let Some(renderer) = skip_or_renderer("ssr_trace_hits_a_box_in_a_mirror_floor") else {
        return;
    };
    let size = FrameSize::native(96, 64);
    let mut scene = RenderScene::empty(size.render);
    scene.post = Some(exact_post());
    scene.environment = Some(test_env());
    let eye = Vec3::new(0.0, 4.5, 7.5);
    scene.camera = crate::scene::RenderCamera {
        view: Mat4::look_at_rh(eye, Vec3::new(0.0, 0.5, 0.0), Vec3::Y),
        projection: Mat4::perspective_rh(60.0_f32.to_radians(), 1.5, 0.1, 100.0),
        position: eye,
    };
    let mut floor = ground(50.0);
    floor.material = [0.02, 1.0, 0.0];
    let mut red_box = cube(Vec3::new(0.0, 0.5, 0.0), Vec3::ONE, [1.0, 0.0, 0.0, 1.0]);
    red_box.material = [0.5, 0.0, 1.0];
    scene.meshes = vec![floor, red_box];

    let (tw, th, texels) = ssr_trace_texels(&renderer, &scene, size);
    let texel = |x: u32, y: u32| texels[(y * tw + x) as usize];
    for y in 0..th {
        for x in 0..tw {
            assert!(texel(x, y).iter().all(|v| v.is_finite()), "({x}, {y})");
        }
    }
    let at = |world: Vec3| {
        let (x, y) = project(&scene.camera, world, tw, th);
        (x, y, texel(x, y))
    };
    for z in [0.8, 1.0] {
        let (x, y, t) = at(Vec3::new(0.0, 0.0, z));
        assert!(t[3] > 0.5, "floor at z={z} ({x}, {y}): confidence {}", t[3]);
        assert!(
            t[0] > 2.0 * t[1] && t[0] > 2.0 * t[2],
            "floor at z={z} ({x}, {y}) is not red: {t:?}"
        );
    }
    for world in [
        Vec3::new(5.0, 0.0, 0.0),
        Vec3::new(-5.0, 0.0, 0.0),
        Vec3::new(4.0, 0.0, -2.0),
    ] {
        let (x, y, t) = at(world);
        assert_eq!(t[3], 0.0, "floor {world} ({x}, {y}) should miss: {t:?}");
    }
    for (x, y) in [(0, 0), (tw - 1, 0)] {
        assert_eq!(texel(x, y), [0.0; 4], "sky texel ({x}, {y})");
    }
}

/// One IEEE binary16 for a normal value (or zero) that is exactly representable.
fn f16_exact(v: f32) -> u16 {
    if v == 0.0 {
        return 0;
    }
    let bits = v.to_bits();
    let sign = (bits >> 16) & 0x8000;
    let exp = ((bits >> 23) & 0xff) as i32 - 127 + 15;
    let mant = (bits & 0x7f_ffff) >> 13;
    assert!(
        (1..31).contains(&exp) && bits & 0x1fff == 0,
        "{v} is not f16-exact"
    );
    (sign | ((exp as u32) << 10) | mant) as u16
}

fn upload_2d(
    renderer: &Renderer,
    format: wgpu::TextureFormat,
    (w, h): (u32, u32),
    bytes: &[u8],
    bytes_per_texel: u32,
) -> wgpu::TextureView {
    let size = wgpu::Extent3d {
        width: w,
        height: h,
        depth_or_array_layers: 1,
    };
    let texture = renderer
        .gpu
        .device()
        .create_texture(&wgpu::TextureDescriptor {
            label: Some("pyramid oracle source"),
            size,
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
    renderer.gpu.queue().write_texture(
        wgpu::TexelCopyTextureInfo {
            texture: &texture,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        bytes,
        wgpu::TexelCopyBufferLayout {
            offset: 0,
            bytes_per_row: Some(w * bytes_per_texel),
            rows_per_image: Some(h),
        },
        size,
    );
    texture.create_view(&wgpu::TextureViewDescriptor::default())
}

/// Reads a one-channel `R32Float` target back, unpadding the 256-byte row alignment.
fn read_r32(renderer: &Renderer, texture: &wgpu::Texture) -> Vec<f32> {
    f32_texels(&read_texels(renderer, texture, 4, wgpu::TextureAspect::All))
}

/// The real reduce pipelines, run over a 7×5 source into 3×2 (ratios 7/3 and 5/2, so the
/// footprints overlap and an odd edge is kept), match the CPU oracles that pin the same rules:
/// Hi-Z exactly, the colour seed and tent within half-float rounding.
#[test]
fn needs_gpu_ssr_pyramid_reduces_match_the_cpu_oracle() {
    use crate::ssr_pyramid::tests::{color_reduce, color_seed, hiz_reduce};
    let Some(renderer) = skip_or_renderer("ssr_pyramid_reduces_match_the_cpu_oracle") else {
        return;
    };
    let (src, dst) = ((7u32, 5u32), (3u32, 2u32));
    let texels = (src.0 * src.1) as usize;

    // Hi-Z: distinct depths in (0, 1), a deterministic scramble so a misplaced tap shows.
    let depth: Vec<f32> = (0..texels)
        .map(|i| ((i * 17 + 5) % 37) as f32 / 40.0 + 0.01)
        .collect();
    let bytes: Vec<u8> = depth.iter().flat_map(|d| d.to_le_bytes()).collect();
    let view = upload_2d(&renderer, wgpu::TextureFormat::R32Float, src, &bytes, 4);
    let params = shader_gen::HizParams {
        target_extent: [dst.0 as f32, dst.1 as f32],
        viewmodel_slice: 0.0,
        pad: 0.0,
    };
    let target = run_effect_into(
        &renderer,
        ShaderId::HizReduce,
        &[&view],
        bytemuck::bytes_of(&params),
        dst,
        wgpu::TextureFormat::R32Float,
    );
    assert_eq!(read_r32(&renderer, &target), hiz_reduce(&depth, src, dst));

    // Colour: eighths, so every value is f16-exact and distinct per channel.
    let color: Vec<[f32; 4]> = (0..texels)
        .map(|i| {
            let v = |c: usize| (i * 3 + c) as f32 / 8.0;
            [v(0), v(1), v(2), 1.0]
        })
        .collect();
    let bytes: Vec<u8> = color
        .iter()
        .flatten()
        .flat_map(|&c| f16_exact(c).to_le_bytes())
        .collect();
    let view = upload_2d(&renderer, HDR_FORMAT, src, &bytes, 8);
    for (reduce, want) in [
        (0u32, color_seed(&color, src, dst)),
        (1, color_reduce(&color, src, dst)),
    ] {
        let params = shader_gen::SsrColorParams {
            target_extent: [dst.0 as f32, dst.1 as f32],
            reduce,
            pad: 0,
        };
        let got = run_effect(
            &renderer,
            ShaderId::SsrColor,
            &[&view],
            bytemuck::bytes_of(&params),
            dst,
        );
        for (i, (g, w)) in got.iter().zip(&want).enumerate() {
            for c in 0..4 {
                assert!(
                    (g[c] - w[c]).abs() <= 1e-3 * w[c].abs().max(1.0),
                    "reduce {reduce} texel {i} channel {c}: gpu {} vs oracle {}",
                    g[c],
                    w[c]
                );
            }
        }
    }
}

/// **The bloom chain scrubs a non-finite texel at each stage.** Drives the threshold, the Karis
/// downsample and the upsample directly over a uniform source of `+inf`, `-inf`, NaN, the
/// largest finite half-float and a small value, so removing the `hdr_safe` in any one of the
/// three shaders fails here — which the end-to-end test below cannot, for the target it renders
/// never holds an infinity on a GPU that saturates the store.
#[test]
fn needs_gpu_the_bloom_chain_scrubs_non_finite_texels() {
    let Some(renderer) = skip_or_renderer("the_bloom_chain_scrubs_non_finite_texels") else {
        return;
    };
    const CEILING: f32 = 60_000.0;
    let small = 2.0_f32.powi(-10) * (1.0 + 25.0 / 1024.0); // f16 0x1419
    let extent = (4u32, 4u32);
    let texel = [1.0 / extent.0 as f32, 1.0 / extent.1 as f32];
    // (label, f16 bits, threshold, karis downsample, upsample)
    let cases: [(&str, u16, f32, f32, f32); 5] = [
        ("+inf", 0x7C00, CEILING, CEILING, CEILING),
        ("-inf", 0xFC00, 0.0, 0.0, 0.0),
        ("NaN", 0x7E00, 0.0, 0.0, 0.0),
        ("65504", 0x7BFF, CEILING, CEILING, CEILING),
        ("1e-3", 0x1419, 0.0, small, 2.0 * small),
    ];
    for (label, bits, want_threshold, want_down, want_up) in cases {
        let source = renderer
            .gpu
            .device()
            .create_texture(&wgpu::TextureDescriptor {
                label: Some("bloom scrub source"),
                size: wgpu::Extent3d {
                    width: extent.0,
                    height: extent.1,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: HDR_FORMAT,
                usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
                view_formats: &[],
            });
        let one = 0x3C00_u16;
        let texel_bytes: Vec<u8> = [bits, bits, bits, one]
            .iter()
            .flat_map(|half| half.to_le_bytes())
            .collect();
        let data: Vec<u8> = texel_bytes.repeat((extent.0 * extent.1) as usize);
        renderer.gpu.queue().write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &source,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            &data,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(extent.0 * 8),
                rows_per_image: Some(extent.1),
            },
            wgpu::Extent3d {
                width: extent.0,
                height: extent.1,
                depth_or_array_layers: 1,
            },
        );
        let view = source.create_view(&wgpu::TextureViewDescriptor::default());

        let threshold = BloomThreshold {
            level: 1.0,
            pad0: 0.0,
            pad1: 0.0,
            pad2: 0.0,
        };
        let down = DownsampleParams {
            texel: [texel[0], texel[1], 1.0, 0.0],
        };
        let up = UpsampleParams {
            texel: [texel[0], texel[1], 0.0, 0.0],
        };
        let stages = [
            (
                "threshold",
                run_effect(
                    &renderer,
                    ShaderId::Threshold,
                    &[&view],
                    bytemuck::bytes_of(&threshold),
                    extent,
                ),
                want_threshold,
            ),
            (
                "karis downsample",
                run_effect(
                    &renderer,
                    ShaderId::BloomDown,
                    &[&view],
                    bytemuck::bytes_of(&down),
                    extent,
                ),
                want_down,
            ),
            (
                "upsample",
                run_effect(
                    &renderer,
                    ShaderId::BloomUp,
                    &[&view, &view],
                    bytemuck::bytes_of(&up),
                    extent,
                ),
                want_up,
            ),
        ];
        for (stage, texels, want) in stages {
            for texel in texels {
                for &channel in &texel[..3] {
                    assert!(
                        channel.is_finite() && (0.0..=CEILING).contains(&channel),
                        "{label} through {stage}: {texel:?} is not finite and within the ceiling"
                    );
                    assert!(
                        (channel - want).abs() <= want * 1e-3,
                        "{label} through {stage}: {texel:?}, expected {want}"
                    );
                }
            }
        }
    }
}

/// **A non-finite scene texel never darkens the bloom.** One small unlit sprite writes a
/// non-finite value into the scene target over a dim backdrop that blooms nothing. Bloom only adds
/// light, so outside the sprite's footprint no pixel may be darker with bloom on than off. The
/// scrub itself is pinned stage by stage by
/// `needs_gpu_the_bloom_chain_scrubs_non_finite_texels`.
///
/// The "infinity" sprite overflows in the sprite shader, and its `mix` against the flash colour
/// then turns the infinity into a NaN, so the target holds NaN in both cases; an `Rgba16Float`
/// store saturates rather than overflowing here, so no texel is a true infinity on a GPU that
/// behaves like this one.
#[test]
fn needs_gpu_a_non_finite_scene_texel_never_darkens_the_bloom() {
    let Some(renderer) = skip_or_renderer("a_non_finite_scene_texel_never_darkens_the_bloom")
    else {
        return;
    };
    let (w, h) = (128u32, 128u32);
    let size = FrameSize::native(w, h);
    let centre = Vec3::new(0.0, 0.0, -10.0);
    let half = 1.0;
    // The infinity is made in the shader: the sprite shader multiplies the tint by its own
    // alpha, so `f32::MAX`-scale RGB with an alpha of 3 overflows f32.
    for (label, rgb, alpha) in [("infinity", 3.0e38_f32, 3.0_f32), ("NaN", f32::NAN, 1.0)] {
        let mut scene = RenderScene::empty((w, h));
        scene.camera = origin_camera(w, h);
        scene.ambient = [0.0; 3];
        scene.meshes = vec![film_box(
            Vec3::new(0.0, 0.0, -12.0),
            Vec3::new(40.0, 40.0, 0.5),
            [0.2; 3],
        )];
        scene.sprites = vec![crate::scene::SpriteInstance {
            center: centre,
            right: Vec3::new(half, 0.0, 0.0),
            up: Vec3::new(0.0, half, 0.0),
            color: [rgb, rgb, rgb, alpha],
            texture: None,
            uv: crate::scene::SpriteInstance::FULL_UV,
            lit: false,
            layer: 0,
            additive: true,
            flash: 0.0,
            flash_color: [1.0; 3],
            uv_repeat: crate::scene::SpriteInstance::NO_TILING,
            uv_offset: [0.0; 2],
        }];
        let base = PostData {
            dither: false,
            chromatic_aberration: 0.0,
            dof: 0.0,
            halation: 0.0,
            shafts: 0.0,
            ssao: 0.0,
            ..PostData::default()
        };
        scene.post = Some(PostData {
            bloom_strength: 0.0,
            ..base
        });
        let off = renderer.render(&scene, size).expect("a bloom-off frame");
        scene.post = Some(base);
        let on = renderer.render(&scene, size).expect("a bloom-on frame");

        // The sprite's projected footprint, plus a 2 px margin.
        let (cx, cy) = project(&scene.camera, centre, w, h);
        let (ex, ey) = project(&scene.camera, centre + Vec3::new(half, half, 0.0), w, h);
        let reach = (ex.abs_diff(cx)).max(ey.abs_diff(cy)) + 2;
        for y in 0..h {
            for x in 0..w {
                if x.abs_diff(cx) <= reach && y.abs_diff(cy) <= reach {
                    continue;
                }
                let (a, b) = (rgb_at(&on, x, y), rgb_at(&off, x, y));
                for c in 0..3 {
                    assert!(
                        i32::from(a[c]) >= i32::from(b[c]) - 2,
                        "{label}: bloom darkened ({x}, {y}) channel {c}: {b:?} -> {a:?}"
                    );
                }
            }
        }
    }
}

/// The chromatic-aberration fixture: one emissive bar centred on the optical axis and a second one
/// well off to the right, both spanning the frame's full height, on an unlit background.
///
/// Two bars, because the effect's whole claim is *radial*: the off-centre bar is where a
/// displacement exists to be measured, and the centred one is where it must be exactly zero. One
/// bar could not separate "displaces the channels" from "displaces the channels everywhere".
///
/// The bar's own colour is achromatic, so a fringe is entirely the aberration's doing rather than
/// something already in the image.
fn ca_scene(w: u32, h: u32) -> RenderScene {
    let mut scene = RenderScene::empty((w, h));
    scene.camera = origin_camera(w, h);
    scene.ambient = [0.0, 0.0, 0.0];
    scene.meshes = vec![
        film_box(CA_PLANE, Vec3::new(1.0, 24.0, 1.0), [0.9, 0.9, 0.9]),
        film_box(CA_BAR, Vec3::new(0.6, 24.0, 1.0), [0.9, 0.9, 0.9]),
    ];
    scene
}

/// The centred bar, the off-centre bar (x ∈ [3.5, 4.7] at ten units out, i.e. u ∈ [0.80, 0.91] on
/// a square frame), and the two dark probes just outside its edges — the inner one between the bar
/// and the frame's centre, the outer one beyond it.
///
/// The probes sit on dark ground a few pixels clear of the silhouette, which is the only place a
/// fringe can be read: inside the bar every channel is saturated, and far outside it none reaches.
/// The gaps are calibrated against the displacement, which at ten units out is `2 · x ·
/// chromatic_aberration` world units — proportional to the distance from the axis, so the inner
/// probe is the tighter of the two by construction and sets the geometry.
///
/// The bar is deliberately below the bloom threshold, so no glow reaches the probes and the only
/// thing that can light them is a displaced tap.
const CA_PLANE: Vec3 = Vec3::new(0.0, 0.0, -10.0);
const CA_BAR: Vec3 = Vec3::new(4.1, 0.0, -10.0);
const CA_INNER_PROBE: Vec3 = Vec3::new(3.3, 0.0, -10.0);
const CA_OUTER_PROBE: Vec3 = Vec3::new(4.9, 0.0, -10.0);
/// The aperture the probe geometry is calibrated at: it displaces a channel by 0.53 world units at
/// the inner probe, which clears the 0.2-unit gap to the bar with room to spare, and by 0.78 at
/// the outer one, which reaches the bar from beyond it without leaving the frustum.
const CA_STRENGTH: f32 = 0.08;

/// The fixture's post, and it pins `tonemap: none` deliberately. What the assertions below read
/// is a *per-channel* displacement, and only a per-channel operator lets that survive to the
/// output: the shipped AgX works in a Rec.2020 space through non-diagonal inset and outset
/// matrices, so under it a displaced red or blue genuinely does move the output green. That
/// coupling belongs to the operator, not to the aberration, and reading the displacement through
/// it would be measuring the wrong thing. Both sides of every comparison use this, so the only
/// difference between them is `chromatic_aberration`.
fn ca_post(strength: f32) -> PostData {
    PostData {
        chromatic_aberration: strength,
        tonemap: TonemapOperator::None,
        ..PostData::default()
    }
}

/// **The aberration is radial, signed, and zero on the axis.** A dark pixel just outside the
/// off-centre bar on the side *away* from the frame's centre reads red, because the red tap is
/// pulled inward and lands on the bar; a dark pixel on the side *toward* the centre reads blue,
/// because the blue tap is pushed outward onto it. The two fringes are therefore opposite in sign,
/// which is what separates a genuine radial displacement from any per-pixel channel scaling, from
/// a constant (longitudinal) offset, and from a blur.
#[test]
fn needs_gpu_chromatic_aberration_splits_an_off_centre_edge_into_opposed_fringes() {
    let Some(renderer) =
        skip_or_renderer("chromatic_aberration_splits_an_off_centre_edge_into_opposed_fringes")
    else {
        return;
    };
    let (w, h) = (256u32, 256u32);
    let size = FrameSize::native(w, h);
    let mut scene = ca_scene(w, h);
    let at = |world| project(&scene.camera, world, w, h);
    let (inner, outer, axis) = (at(CA_INNER_PROBE), at(CA_OUTER_PROBE), at(CA_PLANE));

    scene.post = Some(ca_post(0.0));
    let off = renderer.render(&scene, size).expect("no aberration");
    // The probes must land on dark ground beside the bar rather than on it, or a "fringe" would
    // just be the bar's own colour read back.
    let bar = rgb_at(&off, axis.0, axis.1);
    for (name, p) in [("inner", inner), ("outer", outer)] {
        let px = rgb_at(&off, p.0, p.1);
        assert!(
            u32::from(px[0]) + 100 < u32::from(bar[0]),
            "the {name} probe must be dark ground beside the bar, not the bar: {px:?} against \
             {bar:?}",
        );
    }

    scene.post = Some(ca_post(CA_STRENGTH));
    let on = renderer.render(&scene, size).expect("aberration on");
    // What each channel gained, which is what a displacement means — the background is not
    // achromatic, so an absolute channel comparison would be reading the clear colour.
    let gained = |p: (u32, u32), channel: usize| {
        i32::from(rgb_at(&on, p.0, p.1)[channel]) - i32::from(rgb_at(&off, p.0, p.1)[channel])
    };

    let (outer_red, outer_blue) = (gained(outer, 0), gained(outer, 2));
    assert!(
        outer_red > 20 && outer_red > outer_blue + 20,
        "beyond the bar the red tap is pulled inward onto it while the blue tap is pushed off \
         it, so the fringe must be red: red +{outer_red}, blue +{outer_blue}",
    );
    let (inner_red, inner_blue) = (gained(inner, 0), gained(inner, 2));
    assert!(
        inner_blue > 20 && inner_blue > inner_red + 20,
        "between the bar and the frame's centre the displacement reverses, so the fringe must \
         be blue: red +{inner_red}, blue +{inner_blue}",
    );

    // Green is not displaced at all, and every step behind it here is per-channel, so its bytes
    // must be *exactly* the ones the unaberrated frame wrote — at both fringes and on the axis.
    for (name, p) in [("inner", inner), ("outer", outer), ("axis", axis)] {
        assert_eq!(
            gained(p, 1),
            0,
            "green is left on the sensor's own grid, so the {name} probe's green byte cannot \
             move",
        );
    }

    // On the optical axis the displacement is exactly zero. The tolerance is one output step and
    // covers the last bit of the interpolated `uv` at a pixel centre, not the effect: a whole
    // fringe is worth two hundred steps a few pixels away.
    let (was, now) = (rgb_at(&off, axis.0, axis.1), rgb_at(&on, axis.0, axis.1));
    for channel in 0..3 {
        assert!(
            u32::from(was[channel]).abs_diff(u32::from(now[channel])) <= 1,
            "the frame's centre has no radial direction to displace along: {was:?} -> {now:?}",
        );
    }
}

/// The halation fixture: one small, **super-threshold** emissive cube off to the right of a dark
/// frame, so there is a real blurred bright field for the effect to borrow and a dark annulus
/// around it to read the bleed in.
///
/// Off-centre rather than centred so the same scene also exercises the aberration in
/// [`neutral_film_emulation_is_byte_identical_and_every_effect_can_move_the_frame`].
fn halation_scene(w: u32, h: u32) -> RenderScene {
    let mut scene = RenderScene::empty((w, h));
    scene.camera = origin_camera(w, h);
    scene.ambient = [0.0, 0.0, 0.0];
    scene.meshes = vec![film_box(HALATION_SOURCE, Vec3::splat(1.0), [6.0, 6.0, 6.0])];
    scene
}

/// The bright source and a probe a few pixels outside its silhouette — inside the bloom chain's
/// reach, which is the field halation borrows, and outside anything the source itself covers.
const HALATION_SOURCE: Vec3 = Vec3::new(2.0, 0.0, -10.0);
const HALATION_RING: Vec3 = Vec3::new(2.74, 0.0, -10.0);

/// **Halation is a warm bleed, and it is not a tinted bloom mix.** The whole test runs at
/// `bloom_strength: 0.0`, so the ordinary mix contributes exactly nothing and everything the ring
/// gains is halation's — which is the claim worth proving, since halation reuses `bloom_tex`.
///
/// Two directions at once: the ring gets *brighter*, and it gets brighter in **red much more than
/// in blue**. A colourless bleed passes the first clause and fails the second; a tint applied to
/// the wrong term fails the first.
#[test]
fn needs_gpu_halation_bleeds_warm_around_a_bright_source_with_no_bloom_mix() {
    let Some(renderer) =
        skip_or_renderer("halation_bleeds_warm_around_a_bright_source_with_no_bloom_mix")
    else {
        return;
    };
    let (w, h) = (192u32, 192u32);
    let size = FrameSize::native(w, h);
    let mut scene = halation_scene(w, h);
    let ring = project(&scene.camera, HALATION_RING, w, h);
    // What the second clause below reads is a *ratio between channels*, so the operator has to
    // be out of the way: AgX works through non-diagonal Rec.2020 inset and outset matrices and
    // has a shoulder, so it compresses red against blue exactly where a warm bleed is strongest
    // — under it the tint's own ratio is unreadable at the probe. `none` leaves the channels
    // independent, which is the domain the halation tint is defined in.
    let base = PostData {
        exposure: 1.0,
        tonemap: TonemapOperator::None,
        ..PostData::default()
    };
    let no_bloom = PostData {
        bloom_strength: 0.0,
        ..base
    };

    // The probe sits on dark ground that the *bloom* field reaches — which is what makes
    // "halation with no bloom mix" a meaningful thing to ask for rather than a probe in the void.
    scene.post = Some(no_bloom);
    let dark = renderer
        .render(&scene, size)
        .expect("no bloom, no halation");
    scene.post = Some(base);
    let bloomed = renderer.render(&scene, size).expect("bloom, no halation");
    let (dark_px, bloom_px) = (
        rgb_at(&dark, ring.0, ring.1),
        rgb_at(&bloomed, ring.0, ring.1),
    );
    // The source is emissive well past 1.0, so it reads 255 in every channel under any
    // operator; the probe reads under a quarter of that, which is what "dark ground" means here.
    assert!(
        dark_px[0] < 64,
        "the ring probe must be dark ground, not the source: {dark_px:?}",
    );
    assert!(
        bloom_px[0] > dark_px[0] + 8,
        "the ring probe must lie inside the blurred bright field halation borrows: {dark_px:?} \
         -> {bloom_px:?}",
    );

    // 0.15, not 0.8: halation reuses `bloom_tex`, so it inherits the chain's *gain* as well as
    // its threshold and radius — and that chain is now the six-level pyramid, which carries
    // about six times the energy the engine's former single blur did at the same strength. At
    // 0.8 both channels rail and the ratio below stops being readable; 0.15 is the same amount
    // of light, on `bloom_strength`'s own scale.
    scene.post = Some(PostData {
        halation: 0.15,
        ..no_bloom
    });
    let halated = renderer.render(&scene, size).expect("halation, no bloom");
    let px = rgb_at(&halated, ring.0, ring.1);
    assert!(
        u32::from(px[0]) > u32::from(dark_px[0]) + 16,
        "halation must light the ring with the bloom mix switched off entirely: {dark_px:?} -> \
         {px:?}",
    );
    let (red_gain, blue_gain) = (
        u32::from(px[0]) - u32::from(dark_px[0]),
        u32::from(px[2]).saturating_sub(u32::from(dark_px[2])),
    );
    assert!(
        red_gain > blue_gain * 2,
        "the bleed is film halation, so it must be warm rather than achromatic: red +{red_gain} \
         against blue +{blue_gain}",
    );
}

/// The grain fixture: one emissive slab far larger than the frustum, at a mid-grey clear of both
/// clipping ends, so every pixel of the frame carries the same colour and the only spread a probe
/// can find is the grain's.
fn grain_scene(w: u32, h: u32) -> RenderScene {
    let mut scene = RenderScene::empty((w, h));
    scene.camera = origin_camera(w, h);
    scene.ambient = [0.0, 0.0, 0.0];
    scene.meshes = vec![film_box(
        Vec3::new(0.0, 0.0, -10.0),
        Vec3::new(24.0, 24.0, 1.0),
        [0.35, 0.35, 0.35],
    )];
    scene
}

/// The grain fixture's post. `dither: false` is what makes the patch below *exactly* flat before
/// grain touches it: the shipped ordered dither is a sub-LSB offset indexed by pixel position, so
/// it would put a step of spread on the baseline the grain's spread is measured against. The two
/// share one encode round trip in the shader, so switching one off changes nothing about the
/// other.
fn grain_post(grain: f32, cell_px: f32, h: u32) -> PostData {
    PostData {
        grain,
        // `grain_size` is a fraction of the frame's height, so a cell of a stated number of
        // *pixels* is that fraction — which is the conversion the test exists to pin.
        grain_size: cell_px / h as f32,
        dither: false,
        ..PostData::default()
    }
}

/// The mean luma over the same window [`patch_contrast`] takes its standard deviation over.
fn patch_mean(img: &RenderedImage, at: (u32, u32), radius: u32) -> f64 {
    let (x0, x1) = (
        at.0.saturating_sub(radius),
        (at.0 + radius).min(img.width - 1),
    );
    let (y0, y1) = (
        at.1.saturating_sub(radius),
        (at.1 + radius).min(img.height - 1),
    );
    let mut sum = 0.0;
    let mut count = 0.0;
    for y in y0..=y1 {
        for x in x0..=x1 {
            sum += f64::from(luma(img, x, y));
            count += 1.0;
        }
    }
    sum / count
}

/// **Grain adds spread without adding brightness, and `grain_size` is a cell.** On a patch that is
/// exactly flat to begin with, the standard deviation goes from zero to tens of output steps while
/// the mean holds — an offset that was not centred on zero would fail the second clause — and at a
/// cell of eight pixels every run of eight horizontally adjacent pixels is **exactly** equal while
/// the runs themselves differ, which no per-pixel noise can pass.
#[test]
fn needs_gpu_grain_spreads_a_flat_patch_without_moving_its_mean_and_quantizes_into_cells() {
    let Some(renderer) = skip_or_renderer(
        "grain_spreads_a_flat_patch_without_moving_its_mean_and_quantizes_into_cells",
    ) else {
        return;
    };
    let (w, h) = (128u32, 128u32);
    let size = FrameSize::native(w, h);
    let mut scene = grain_scene(w, h);
    let at = (w / 2, h / 2);
    let window = 24u32;

    scene.post = Some(grain_post(0.0, 1.0, h));
    let off = renderer.render(&scene, size).expect("grain off");
    let flat = rgb_at(&off, at.0, at.1);
    for y in (at.1 - window)..=(at.1 + window) {
        for x in (at.0 - window)..=(at.0 + window) {
            assert_eq!(
                rgb_at(&off, x, y),
                flat,
                "the patch must be exactly flat at ({x}, {y}) before grain can spread it",
            );
        }
    }
    assert!(
        flat[0] > 48 && flat[0] < 208,
        "the patch must sit clear of both clipping ends, or the offsets would be cut: {flat:?}",
    );

    // One-pixel cells: the finest grain there is, and the case the spread is measured on.
    scene.post = Some(grain_post(0.1, 1.0, h));
    let fine = renderer.render(&scene, size).expect("fine grain");
    let spread = patch_contrast(&fine, at, window);
    assert!(
        spread > 20.0,
        "an amplitude of 0.1 of the encoded range is worth about ±25 output steps per channel, \
         so the patch's spread must rise far off zero: {spread:.2}",
    );
    let drift = patch_mean(&fine, at, window) - patch_mean(&off, at, window);
    assert!(
        drift.abs() < 5.0,
        "the offset is centred on zero, so grain must add spread and not brightness: {drift:.2} \
         against a spread of {spread:.2}",
    );

    // Eight-pixel cells: the exact statement, and the one a per-pixel hash cannot pass.
    const CELL: u32 = 8;
    scene.post = Some(grain_post(0.1, CELL as f32, h));
    let coarse = renderer.render(&scene, size).expect("coarse grain");
    let mut changes = 0;
    let first_cell = at.0 / CELL - 6;
    for cell in first_cell..first_cell + 12 {
        let x0 = cell * CELL;
        let value = rgb_at(&coarse, x0, at.1);
        for x in x0..x0 + CELL {
            assert_eq!(
                rgb_at(&coarse, x, at.1),
                value,
                "every pixel of cell {cell} must carry one hash of one cell coordinate",
            );
        }
        if rgb_at(&coarse, x0 - 1, at.1) != value {
            changes += 1;
        }
    }
    assert!(
        changes >= 9,
        "the cells must actually differ from one another, or 'constant everywhere' would pass \
         the clause above: {changes} of 12 boundaries changed",
    );
}

/// **Byte identity, on bytes, for all three effects at once — and none of the three
/// passes vacuously.**
///
/// The first half is the fog-and-vignette shape: a `PostProcess` whose *other* knobs are doing real work,
/// with the film roster left neutral, is byte-identical to the same scene with those fields
/// unauthored. That is what makes "neutral is skipped, never computed" a claim about the emitted
/// arithmetic rather than about a value — including the refactor that put the grain and the dither
/// through one shared encode/decode round trip, since the working knobs below have the dither on.
///
/// The second half turns each effect on **alone** and requires the frame to move. Without it the
/// equalities above would also hold for three effects that were never wired up.
#[test]
fn needs_gpu_neutral_film_emulation_is_byte_identical_and_every_effect_can_move_the_frame() {
    let Some(renderer) = skip_or_renderer(
        "neutral_film_emulation_is_byte_identical_and_every_effect_can_move_the_frame",
    ) else {
        return;
    };
    let (w, h) = (96u32, 96u32);
    let size = FrameSize::native(w, h);
    let mut scene = halation_scene(w, h);
    let working = PostData {
        exposure: 1.3,
        tonemap: TonemapOperator::Agx,
        contrast: 1.2,
        saturation: 0.8,
        vignette: 0.3,
        dither: true,
        ..PostData::default()
    };

    scene.post = Some(working);
    let unauthored = renderer
        .render(&scene, size)
        .expect("render with the film knobs at their defaults");

    scene.post = Some(PostData {
        chromatic_aberration: 0.0,
        halation: 0.0,
        grain: 0.0,
        // Deliberately not the default: a non-default cell size must be inert while `grain` is
        // zero, so the gate is the amplitude alone.
        grain_size: 0.02,
        ..working
    });
    let neutral = renderer
        .render(&scene, size)
        .expect("render with the film knobs explicitly neutral");
    assert_eq!(
        unauthored.rgba, neutral.rgba,
        "a neutral aberration, halation and grain must be skipped, not computed — and the \
         dither must survive sharing its encode/decode trip with the grain",
    );

    for (name, on) in [
        (
            "chromatic_aberration",
            PostData {
                chromatic_aberration: CA_STRENGTH,
                ..working
            },
        ),
        (
            "halation",
            PostData {
                halation: 0.8,
                ..working
            },
        ),
        (
            "grain",
            PostData {
                grain: 0.1,
                ..working
            },
        ),
    ] {
        scene.post = Some(on);
        assert_ne!(
            renderer.render(&scene, size).expect("one effect on").rgba,
            neutral.rgba,
            "`{name}` on the same scene must render differently — else nothing was proved",
        );
    }
}
