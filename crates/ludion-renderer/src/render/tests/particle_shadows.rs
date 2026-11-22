//! Shadowed and velocity-stretched particles: GPU tests over the `particle` shader in the transparents
//! pass and the transparent layer.

use super::*;

const PX: u32 = 270;

fn size() -> FrameSize {
    FrameSize::native(PX, PX)
}

/// A lit white particle at `center`, hard-edged, one unit across.
fn lit_particle(center: Vec3, shadowed: bool) -> ParticleInstance {
    let mut p = particle(center, 1.0, [1.0; 4], 0.0);
    p.lit = true;
    p.shadowed = shadowed;
    p
}

/// A thin casting wall in the plane x = 0.5 from z = -12 to -4: it hides neither particle.
fn wall() -> MeshInstance {
    let mut wall = slab(Vec3::ZERO, 1.0, [0.4; 3]);
    wall.model = Mat4::from_translation(Vec3::new(0.5, 4.0, -8.0))
        * Mat4::from_scale(Vec3::new(0.1, 20.0, 8.0));
    wall
}

/// A particle either side of the wall and a shadow-granted point light on the right, shining across it.
fn walled(left_shadowed: bool, lit: bool) -> RenderScene {
    let mut scene = RenderScene::empty((PX, PX));
    scene.camera = origin_camera(PX, PX);
    scene.ambient = [0.2; 3];
    scene.post = Some(exact_post());
    scene.meshes.push(wall());
    let mut light = point_light(Vec3::new(2.0, 1.0, -4.0), 60.0, 30.0);
    light.shadow = Some(crate::scene::LocalShadowData {
        bias: 0.05,
        near: 0.1,
        distance: None,
        fade: 0.0,
    });
    scene.lights = vec![light];
    let mut left = lit_particle(Vec3::new(-2.0, 0.0, -6.0), left_shadowed);
    let mut right = lit_particle(Vec3::new(2.0, 0.0, -6.0), left_shadowed);
    left.lit = lit;
    right.lit = lit;
    scene.particles = vec![left, right];
    scene
}

fn px(frame: &RenderedImage, x: u32, y: u32) -> [u8; 4] {
    let at = ((y * frame.width + x) * 4) as usize;
    [
        frame.rgba[at],
        frame.rgba[at + 1],
        frame.rgba[at + 2],
        frame.rgba[at + 3],
    ]
}

/// Summed rgb of columns `x` over every row.
fn column_sum(frame: &RenderedImage, x: std::ops::Range<u32>) -> u64 {
    (0..frame.height)
        .flat_map(|y| x.clone().map(move |x| (x, y)))
        .map(|(x, y)| {
            px(frame, x, y)[..3]
                .iter()
                .map(|&c| u64::from(c))
                .sum::<u64>()
        })
        .sum()
}

fn columns_equal(a: &RenderedImage, b: &RenderedImage, x: std::ops::Range<u32>) -> bool {
    (0..a.height).all(|y| x.clone().all(|x| px(a, x, y) == px(b, x, y)))
}

/// Lit `shadowed` particles darken behind a casting wall and take the same bytes on the lit side. Skips
/// with no GPU.
#[test]
fn needs_gpu_a_wall_shadows_lit_particles_for_a_local_light() {
    let Some(renderer) = skip_or_renderer("a_wall_shadows_lit_particles_for_a_local_light") else {
        return;
    };
    let open = renderer.render(&walled(false, true), size()).expect("open");
    let mut scene = walled(false, true);
    scene.particles[0].shadowed = true;
    scene.particles[1].shadowed = true;
    let taken = renderer.render(&scene, size()).expect("taken");
    assert!(
        column_sum(&taken, 0..120) < column_sum(&open, 0..120),
        "the particle past the wall darkens"
    );
    assert!(
        columns_equal(&open, &taken, 165..PX),
        "the lit side is untouched"
    );
}

/// `shadowed` without `lit` is inert: the same bytes as `shadowed: false`. Skips with no GPU.
#[test]
fn needs_gpu_shadowed_without_lit_is_inert() {
    let Some(renderer) = skip_or_renderer("shadowed_without_lit_is_inert") else {
        return;
    };
    let plain = renderer
        .render(&walled(false, false), size())
        .expect("plain");
    let mut scene = walled(false, false);
    scene.particles[0].shadowed = true;
    scene.particles[1].shadowed = true;
    let flagged = renderer.render(&scene, size()).expect("flagged");
    assert_eq!(plain.rgba, flagged.rgba);
}

/// The columns and rows where `frame` differs from `empty`, the same scene without the particle.
fn changed_bounds(
    frame: &RenderedImage,
    empty: &RenderedImage,
) -> (std::ops::RangeInclusive<u32>, std::ops::RangeInclusive<u32>) {
    let on = |x, y| px(frame, x, y) != px(empty, x, y);
    let cols: Vec<u32> = (0..frame.width)
        .filter(|&x| (0..frame.height).any(|y| on(x, y)))
        .collect();
    let rows: Vec<u32> = (0..frame.height)
        .filter(|&y| (0..frame.width).any(|x| on(x, y)))
        .collect();
    (
        *cols.first().unwrap()..=*cols.last().unwrap(),
        *rows.first().unwrap()..=*rows.last().unwrap(),
    )
}

fn streaked(streak: Vec3) -> RenderScene {
    let mut scene = RenderScene::empty((PX, PX));
    scene.camera = origin_camera(PX, PX);
    scene.post = Some(exact_post());
    let mut p = particle(Vec3::new(0.0, 0.0, -6.0), 0.5, [1.0; 4], 0.0);
    p.streak = streak;
    scene.particles = vec![p];
    scene
}

/// A streak along +x widens the particle along x and keeps its rows. Skips with no GPU.
#[test]
fn needs_gpu_a_streak_covers_more_columns_than_the_disc() {
    let Some(renderer) = skip_or_renderer("a_streak_covers_more_columns_than_the_disc") else {
        return;
    };
    let disc = renderer
        .render(&streaked(Vec3::ZERO), size())
        .expect("disc");
    let capsule = renderer
        .render(&streaked(Vec3::new(3.0, 0.0, 0.0)), size())
        .expect("capsule");
    let mut bare = streaked(Vec3::ZERO);
    bare.particles.clear();
    let bare = renderer.render(&bare, size()).expect("bare");
    let ((disc_cols, disc_rows), (capsule_cols, capsule_rows)) = (
        changed_bounds(&disc, &bare),
        changed_bounds(&capsule, &bare),
    );
    assert!(
        capsule_cols.end() - capsule_cols.start() > 2 * (disc_cols.end() - disc_cols.start()),
        "the capsule is much wider: {capsule_cols:?} vs {disc_cols:?}"
    );
    assert_eq!(capsule_rows, disc_rows, "the width is the disc's");
}

/// A zero streak is a disc with the transparent layer on (fog) and off: the lane's other bits (a
/// `shadowed` flag on an unlit particle) move nothing. Skips with no GPU.
#[test]
fn needs_gpu_a_zero_streak_is_the_disc_with_and_without_the_layer() {
    let Some(renderer) = skip_or_renderer("a_zero_streak_is_the_disc_with_and_without_the_layer")
    else {
        return;
    };
    let fogged = || {
        let mut scene = own_fog_scene(PX, PX, 0.03);
        scene.particles = vec![particle(Vec3::new(0.0, 0.0, -6.0), 0.5, [1.0; 4], 0.0)];
        scene
    };
    for scene in [streaked(Vec3::ZERO), fogged()] {
        let disc = renderer.render(&scene, size()).expect("disc");
        let mut flagged = scene.clone();
        flagged.particles[0].shadowed = true;
        let again = renderer.render(&flagged, size()).expect("flagged");
        assert_eq!(disc.rgba, again.rgba);
        let mut moving = scene.clone();
        moving.particles[0].streak = Vec3::new(3.0, 0.0, 0.0);
        let moved = renderer.render(&moving, size()).expect("moved");
        assert_ne!(disc.rgba, moved.rgba, "a streak changes the frame");
    }
}
