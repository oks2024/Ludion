//! Blended meshes (`Mesh.blend`): GPU tests over `transparents`, `transparent_depth` and the sort.

use super::*;
use crate::scene::{BlendMode, BlendedMesh};

const PX: u32 = 128;

fn size() -> FrameSize {
    FrameSize::native(PX, PX)
}

/// Meshes on the dark clear colour, no lights, the exact-output post; the camera looks down -Z.
fn scene(meshes: Vec<MeshInstance>, blended: Vec<BlendedMesh>) -> RenderScene {
    let mut scene = RenderScene::empty((PX, PX));
    scene.camera = origin_camera(PX, PX);
    scene.ambient = [0.0; 3];
    scene.post = Some(exact_post());
    scene.meshes = meshes;
    scene.blended = blended;
    scene
}

/// A thin flat-emissive pane, so its lit colour is exactly `color`'s rgb and its opacity `color[3]`.
fn pane(center: Vec3, side: f32, color: [f32; 4], mode: BlendMode, group: u32) -> BlendedMesh {
    let mut instance = slab(center, 1.0, [color[0], color[1], color[2]]);
    instance.model = Mat4::from_translation(center) * Mat4::from_scale(Vec3::new(side, side, 0.05));
    instance.color = color;
    instance.cast_shadows = false;
    BlendedMesh {
        instance,
        mode,
        group,
    }
}

const CLEAR: [f32; 3] = [0.02, 0.02, 0.035];

fn px(frame: &RenderedImage, x: u32, y: u32) -> [u8; 3] {
    let i = ((y * frame.width + x) * 4) as usize;
    [frame.rgba[i], frame.rgba[i + 1], frame.rgba[i + 2]]
}

const C: u32 = PX / 2;

/// The sRGB byte of each channel of a linear colour.
fn bytes(linear: [f32; 3]) -> [f32; 3] {
    linear.map(srgb8_from_linear)
}

fn assert_near(got: [u8; 3], want: [f32; 3], tolerance: f32, what: &str) {
    for c in 0..3 {
        assert!(
            (f32::from(got[c]) - want[c]).abs() <= tolerance,
            "{what}: channel {c} is {} and should be {} ± {tolerance}",
            got[c],
            want[c]
        );
    }
}

/// Alpha-composites `layers` (colour, opacity) far to near over `background`, in linear light.
fn over(background: [f32; 3], layers: &[([f32; 3], f32)]) -> [f32; 3] {
    layers.iter().fold(background, |behind, (c, a)| {
        std::array::from_fn(|i| c[i] * a + behind[i] * (1.0 - a))
    })
}

fn red_wall() -> Vec<MeshInstance> {
    vec![slab(Vec3::new(0.0, 0.0, -10.0), 6.0, [0.8, 0.1, 0.1])]
}

/// A scene with no blended mesh derives the chain it always did.
#[test]
fn a_scene_without_blended_meshes_keeps_its_chain() {
    let bare = scene(red_wall(), vec![]);
    let chain = crate::graph::PostChain::for_scene(&bare);
    assert!(!chain.blended && !chain.transparent_cover);
    let mut with = bare.clone();
    with.blended = vec![pane(
        Vec3::new(0.0, 0.0, -5.0),
        2.0,
        [0.1, 0.4, 0.9, 0.5],
        BlendMode::Alpha,
        0,
    )];
    let chain = crate::graph::PostChain::for_scene(&with);
    assert!(chain.blended && chain.transparent_cover);
}

/// A pane of opacity zero over the frame leaves every byte as it was. Skips with no GPU.
#[test]
fn needs_gpu_a_clear_pane_changes_nothing() {
    let Some(renderer) = skip_or_renderer("a_clear_pane_changes_nothing") else {
        return;
    };
    let none = renderer
        .render(&scene(red_wall(), vec![]), size())
        .expect("none");
    let clear = pane(
        Vec3::new(0.0, 0.0, -5.0),
        2.0,
        [0.1, 0.4, 0.9, 0.0],
        BlendMode::Alpha,
        0,
    );
    let with = renderer
        .render(&scene(red_wall(), vec![clear]), size())
        .expect("with");
    assert_eq!(none.rgba, with.rgba);
}

/// An alpha pane over a red wall is the analytic mix, and a sprite behind the pane still shows: the
/// pane wrote no depth. Skips with no GPU.
#[test]
fn needs_gpu_an_alpha_pane_mixes_and_writes_no_depth() {
    let Some(renderer) = skip_or_renderer("an_alpha_pane_mixes_and_writes_no_depth") else {
        return;
    };
    let tint = [0.1, 0.4, 0.9];
    let blended = || {
        vec![pane(
            Vec3::new(0.0, 0.0, -5.0),
            2.0,
            [tint[0], tint[1], tint[2], 0.5],
            BlendMode::Alpha,
            0,
        )]
    };
    let frame = renderer
        .render(&scene(red_wall(), blended()), size())
        .expect("frame");
    let wall = [0.8, 0.1, 0.1];
    assert_near(
        px(&frame, C, C),
        bytes(over(wall, &[(tint, 0.5)])),
        3.0,
        "the pane's centre",
    );
    let bare = renderer
        .render(&scene(red_wall(), vec![]), size())
        .expect("bare");
    assert_near(px(&bare, C, C), bytes(wall), 2.0, "the wall alone");

    // A sprite behind the pane, nearer than the wall: drawn after the pane, depth-tested against the
    // opaque wall only, it covers the pane's centre.
    let mut behind = scene(red_wall(), blended());
    behind.sprites = vec![sized_sprite(
        Vec3::new(0.0, 0.0, -7.0),
        1.0,
        [0.0, 1.0, 0.0, 1.0],
        false,
    )];
    let with_sprite = renderer.render(&behind, size()).expect("sprite");
    assert_near(
        px(&with_sprite, C, C),
        bytes([0.0, 1.0, 0.0]),
        3.0,
        "a sprite behind the pane",
    );
}

/// Additive brightens and never darkens. Skips with no GPU.
#[test]
fn needs_gpu_an_additive_pane_never_darkens() {
    let Some(renderer) = skip_or_renderer("an_additive_pane_never_darkens") else {
        return;
    };
    let glow = pane(
        Vec3::new(0.0, 0.0, -5.0),
        2.0,
        [0.0, 0.8, 0.1, 0.5],
        BlendMode::Additive,
        0,
    );
    let bare = renderer
        .render(&scene(red_wall(), vec![]), size())
        .expect("bare");
    let lit = renderer
        .render(&scene(red_wall(), vec![glow]), size())
        .expect("lit");
    let (before, after) = (px(&bare, C, C), px(&lit, C, C));
    for c in 0..3 {
        assert!(
            after[c] >= before[c],
            "channel {c}: {before:?} -> {after:?}"
        );
    }
    assert!(after[1] > before[1], "the green channel brightens");
    assert_eq!(after[0], before[0], "red gets nothing added");
    // Outside the pane nothing changed.
    assert_eq!(px(&lit, 4, 4), px(&bare, 4, 4));
}

/// Two overlapping panes swap their draw order when the camera moves to the other side. Skips with
/// no GPU.
#[test]
fn needs_gpu_overlapping_panes_swap_order_with_the_camera() {
    let Some(renderer) = skip_or_renderer("overlapping_panes_swap_order_with_the_camera") else {
        return;
    };
    let red = [0.9, 0.1, 0.1];
    let blue = [0.1, 0.1, 0.9];
    let panes = || {
        vec![
            pane(
                Vec3::new(0.0, 0.0, -5.0),
                3.0,
                [red[0], red[1], red[2], 0.5],
                BlendMode::Alpha,
                0,
            ),
            pane(
                Vec3::new(0.0, 0.0, -9.0),
                3.0,
                [blue[0], blue[1], blue[2], 0.5],
                BlendMode::Alpha,
                1,
            ),
        ]
    };
    let from_front = renderer
        .render(&scene(vec![], panes()), size())
        .expect("front");
    assert_near(
        px(&from_front, C, C),
        bytes(over(CLEAR, &[(blue, 0.5), (red, 0.5)])),
        3.0,
        "red in front",
    );
    let mut behind = scene(vec![], panes());
    let eye = Vec3::new(0.0, 0.0, -14.0);
    behind.camera.view = Mat4::look_at_rh(eye, Vec3::new(0.0, 0.0, -5.0), Vec3::Y);
    behind.camera.position = eye;
    let from_back = renderer.render(&behind, size()).expect("back");
    assert_near(
        px(&from_back, C, C),
        bytes(over(CLEAR, &[(red, 0.5), (blue, 0.5)])),
        3.0,
        "blue in front",
    );
}

/// With fog, the transparent layer's mean distance at a pane pixel is the pane's distance. Skips with
/// no GPU.
#[test]
fn needs_gpu_the_layer_holds_the_panes_own_distance() {
    let Some(renderer) = skip_or_renderer("the_layer_holds_the_panes_own_distance") else {
        return;
    };
    let mut fogged = scene(
        vec![],
        vec![pane(
            Vec3::new(0.0, 0.0, -8.0),
            2.0,
            [0.1, 0.4, 0.9, 0.5],
            BlendMode::Alpha,
            0,
        )],
    );
    fogged.post = Some(PostData {
        fog_density: 0.02,
        ..exact_post()
    });
    let texels = layer_texels(&renderer, &fogged, size());
    let [weighted, coverage] = texels[(C * PX + C) as usize];
    assert!((coverage - 0.5).abs() < 0.01, "coverage {coverage}");
    let mean = weighted / coverage;
    // The front face of a 0.05-thick pane centred at 8.
    assert!((mean - 7.975).abs() < 0.02, "mean distance {mean}");
    assert_eq!(texels[0], [0.0, 0.0], "outside the pane the layer is empty");
}

/// A blended mesh writes no sun-shadow texel: the ground below it is lit exactly as without it, and
/// an opaque pane in the same place does shadow it. Skips with no GPU.
#[test]
fn needs_gpu_a_blended_mesh_casts_no_shadow() {
    let Some(renderer) = skip_or_renderer("a_blended_mesh_casts_no_shadow") else {
        return;
    };
    let ground_scene = |extra_opaque: bool, extra_blended: bool| {
        let mut s = drop_shadow_fixture(PX, PX, false);
        s.post = Some(exact_post());
        s.sun = Some(test_sun(Vec3::new(0.0, -1.0, 0.0), Vec3::ONE, 0.002));
        let high = Vec3::new(0.0, 4.0, 0.0);
        if extra_opaque {
            s.meshes
                .push(cube(high, Vec3::new(3.0, 0.05, 3.0), [0.8, 0.8, 0.8, 1.0]));
        }
        if extra_blended {
            s.blended
                .push(pane(high, 3.0, [0.8, 0.8, 0.8, 0.5], BlendMode::Alpha, 0));
            // Lay it flat: the helper's pane is vertical.
            s.blended[0].instance.model =
                Mat4::from_translation(high) * Mat4::from_scale(Vec3::new(3.0, 0.05, 3.0));
        }
        s
    };
    let at = project(&ground_scene(false, false).camera, Vec3::ZERO, PX, PX);
    let bare = renderer
        .render(&ground_scene(false, false), size())
        .expect("bare");
    let blended = renderer
        .render(&ground_scene(false, true), size())
        .expect("blended");
    let opaque = renderer
        .render(&ground_scene(true, false), size())
        .expect("opaque");
    assert_ne!(
        px(&opaque, at.0, at.1),
        px(&bare, at.0, at.1),
        "control: an opaque slab shadows the ground there"
    );
    assert_eq!(px(&blended, at.0, at.1), px(&bare, at.0, at.1));
}
