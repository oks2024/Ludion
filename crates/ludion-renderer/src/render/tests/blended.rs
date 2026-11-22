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
        lit: true,
        nearest: false,
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

// --- Indirect specular and `lit: false` (W2b) ---------------------------------------------------

/// A glossy, non-emissive pane: `color` albedo at opacity `color[3]`, `material` `[roughness, metallic, emissive]`.
fn glossy(center: Vec3, side: f32, color: [f32; 4], material: [f32; 3]) -> MeshInstance {
    let mut instance = slab(center, 1.0, [color[0], color[1], color[2]]);
    instance.model = Mat4::from_translation(center) * Mat4::from_scale(Vec3::new(side, side, 0.05));
    instance.color = color;
    instance.material = material;
    instance.cast_shadows = false;
    instance
}

fn blended_of(instance: MeshInstance) -> BlendedMesh {
    BlendedMesh {
        instance,
        mode: BlendMode::Alpha,
        lit: true,
        nearest: false,
        group: 0,
    }
}

fn assert_within(a: [u8; 3], b: [u8; 3], tolerance: u8, what: &str) {
    for c in 0..3 {
        assert!(
            a[c].abs_diff(b[c]) <= tolerance,
            "{what}: channel {c} is {} against {} (± {tolerance})",
            a[c],
            b[c]
        );
    }
}

/// A pane at opacity 1 sees the sky's specular as its opaque twin does, from the same `Environment`.
/// Skips with no GPU.
#[test]
fn needs_gpu_an_opaque_alpha_pane_reflects_the_sky_like_its_opaque_twin() {
    let Some(renderer) =
        skip_or_renderer("an_opaque_alpha_pane_reflects_the_sky_like_its_opaque_twin")
    else {
        return;
    };
    let pane_at = Vec3::new(0.0, 0.0, -5.0);
    let instance = glossy(pane_at, 3.0, [0.5, 0.5, 0.5, 1.0], [0.1, 0.0, 0.0]);
    let mut opaque = scene(vec![instance], vec![]);
    opaque.environment = Some(vivid_env(1.0));
    let mut blended = scene(vec![], vec![blended_of(instance)]);
    blended.environment = Some(vivid_env(1.0));
    let want = renderer.render(&opaque, size()).expect("opaque");
    let got = renderer.render(&blended, size()).expect("blended");
    assert_within(
        px(&got, C, C),
        px(&want, C, C),
        3,
        "the pane's centre against its opaque twin",
    );
    // The control: the twin reflects something, or the parity proves nothing.
    let mut dry = opaque.clone();
    dry.environment = None;
    let dry = renderer.render(&dry, size()).expect("no environment");
    assert!(
        px(&want, C, C)[1] > px(&dry, C, C)[1] + 10,
        "the opaque twin shows the sky"
    );
}

/// A glossy black pane over black shows only the sky's reflection: brighter with an `Environment` than
/// without. Skips with no GPU.
#[test]
fn needs_gpu_a_glossy_blended_pane_shows_the_sky() {
    let Some(renderer) = skip_or_renderer("a_glossy_blended_pane_shows_the_sky") else {
        return;
    };
    let instance = glossy(
        Vec3::new(0.0, 0.0, -5.0),
        3.0,
        [0.0, 0.0, 0.0, 1.0],
        [0.05, 0.0, 0.0],
    );
    let mut lit = scene(vec![], vec![blended_of(instance)]);
    lit.environment = Some(vivid_env(1.0));
    let mut off = lit.clone();
    off.environment = None;
    let with = renderer.render(&lit, size()).expect("with");
    let without = renderer.render(&off, size()).expect("without");
    let (with, without) = (px(&with, C, C), px(&without, C, C));
    assert!(
        with[1] > without[1] + 15,
        "the sky shows in the pane: {with:?} against {without:?}"
    );
}

/// The room of `probe_shade`'s `a_box_probe_reflects_the_room`: a mirror floor under a blue sky and an emissive
/// red wall. `floor_blended` draws the floor as an alpha-1 blended mesh instead of an opaque one.
fn probe_room(floor_blended: bool) -> RenderScene {
    let mut scene = RenderScene::empty((64, 64));
    scene.post = Some(exact_post());
    scene.environment = Some(crate::scene::EnvironmentData {
        sky_zenith: [0.05, 0.1, 0.9],
        sky_horizon: [0.05, 0.1, 0.9],
        sky_ground: [0.05, 0.1, 0.9],
        ..test_env()
    });
    let eye = Vec3::new(0.0, 2.0, 4.0);
    scene.camera = crate::scene::RenderCamera {
        view: Mat4::look_at_rh(eye, Vec3::ZERO, Vec3::Y),
        projection: Mat4::perspective_rh(60.0_f32.to_radians(), 1.0, 0.1, 100.0),
        position: eye,
    };
    let mut floor = slab(Vec3::ZERO, 1.0, [0.9, 0.9, 0.9]);
    floor.shape = MeshShape::Plane;
    floor.model = Mat4::from_scale(Vec3::new(30.0, 1.0, 30.0));
    floor.material = [0.05, 1.0, 0.0];
    floor.cast_shadows = false;
    let mut wall = slab(Vec3::new(0.0, 4.0, -6.1), 1.0, [1.0, 0.0, 0.0]);
    wall.model = Mat4::from_translation(Vec3::new(0.0, 4.0, -6.1))
        * Mat4::from_scale(Vec3::new(30.0, 8.0, 0.2));
    wall.material = [1.0, 0.0, 4.0];
    scene.meshes = vec![wall];
    if floor_blended {
        scene.blended = vec![blended_of(floor)];
    } else {
        scene.meshes.push(floor);
    }
    scene.probes = vec![crate::ProbeData {
        entity: ludion_core::ids::EntityId::new(1, 0),
        position: Vec3::new(0.0, 4.0, 0.0),
        rotation: ludion_core::math::glam::Quat::IDENTITY,
        volume: crate::ProbeVolume::Box {
            extent: Vec3::new(15.0, 5.0, 6.0),
        },
        blend: 0.0,
        priority: 0,
    }];
    scene
}

/// With a reflection probe the blended mirror floor shows the wall as its opaque twin does. Skips with no GPU.
#[test]
fn needs_gpu_a_blended_floor_reflects_a_probe_like_its_opaque_twin() {
    let Some(renderer) = skip_or_renderer("a_blended_floor_reflects_a_probe_like_its_opaque_twin")
    else {
        return;
    };
    let size = FrameSize::native(64, 64);
    let want = renderer.render(&probe_room(false), size).expect("opaque");
    let got = renderer.render(&probe_room(true), size).expect("blended");
    let [wr, _, wb] = px(&want, 32, 32);
    assert!(
        wr > wb + 40,
        "control: the opaque floor shows the wall ({wr} {wb})"
    );
    assert_within(
        px(&got, 32, 32),
        px(&want, 32, 32),
        3,
        "the floor's reflection",
    );
}

/// An unlit pane is `albedo + emissive` at its opacity whatever lights the scene. Skips with no GPU.
#[test]
fn needs_gpu_an_unlit_pane_ignores_the_lights() {
    let Some(renderer) = skip_or_renderer("an_unlit_pane_ignores_the_lights") else {
        return;
    };
    let albedo = [0.2, 0.5, 0.8];
    let build = |light: Option<Vec3>| {
        let instance = glossy(
            Vec3::new(0.0, 0.0, -5.0),
            3.0,
            [albedo[0], albedo[1], albedo[2], 0.6],
            [0.5, 0.0, 0.5],
        );
        let mut s = scene(
            vec![],
            vec![BlendedMesh {
                lit: false,
                ..blended_of(instance)
            }],
        );
        s.lights = light
            .map(|color| crate::scene::PointLightData {
                position: Vec3::new(0.0, 0.0, -2.0),
                color,
                intensity: 12.0,
                range: 20.0,
                dynamic: false,
                shadow: None,
                scatter: 0.0,
            })
            .into_iter()
            .collect();
        s
    };
    let dark = renderer.render(&build(None), size()).expect("dark");
    let red = renderer
        .render(&build(Some(Vec3::new(1.0, 0.1, 0.1))), size())
        .expect("red light");
    let green = renderer
        .render(&build(Some(Vec3::new(0.1, 1.0, 0.1))), size())
        .expect("green light");
    let colour = [albedo[0] * 1.5, albedo[1] * 1.5, albedo[2] * 1.5];
    let want = bytes(over(CLEAR, &[(colour, 0.6)]));
    for (frame, what) in [
        (&dark, "no light"),
        (&red, "a red light"),
        (&green, "a green light"),
    ] {
        assert_near(px(frame, C, C), want, 3.0, what);
    }
}

/// Rain, a shadowed local light and a blended pane in one frame record cleanly, and the pane takes the
/// light's own shadow atlas, not the rain map (which trails the pass's reads). Skips with no GPU.
#[test]
fn needs_gpu_rain_does_not_move_a_shadowed_blended_pane() {
    let Some(renderer) = skip_or_renderer("rain_does_not_move_a_shadowed_blended_pane") else {
        return;
    };
    let build = |rain: bool, occluder: bool| {
        let pane = BlendedMesh {
            instance: glossy(
                Vec3::new(0.0, 0.0, -5.0),
                3.0,
                [0.8, 0.8, 0.8, 1.0],
                [1.0, 0.0, 0.0],
            ),
            ..blended_of(slab(Vec3::ZERO, 1.0, [0.0; 3]))
        };
        let mut s = scene(vec![], vec![pane]);
        if occluder {
            let mut block = slab(Vec3::new(1.5, 0.0, -4.0), 0.8, [0.2; 3]);
            block.material = [1.0, 0.0, 0.0];
            s.meshes.push(block);
        }
        s.lights = vec![crate::scene::PointLightData {
            position: Vec3::new(3.0, 0.0, -3.0),
            color: Vec3::ONE,
            intensity: 20.0,
            range: 20.0,
            dynamic: false,
            shadow: Some(crate::scene::LocalShadowData {
                bias: 0.05,
                near: 0.1,
                distance: None,
                fade: 0.0,
            }),
            scatter: 0.0,
        }];
        if rain {
            s.rain = Some(crate::rain::RainData {
                radius: 16.0,
                height: [-6.0, 14.0],
                cell: 4.0,
                fall_speed: [7.0, 9.0],
                wind: [1.2, 0.0, 0.4],
                gust: 0.15,
                drop_width: 0.05,
                streak: 2.0,
                color: [1.0, 1.0, 1.0, 1.0],
                lit: true,
                medium: 1.0,
                occlusion: true,
                wetness: 0.5,
                seed: 0,
                time_s: 1.0,
                per_tile: 0,
            });
        }
        s
    };
    let shadowed = renderer
        .render(&build(false, true), size())
        .expect("shadowed");
    let open = renderer.render(&build(false, false), size()).expect("open");
    let rained = renderer.render(&build(true, true), size()).expect("rained");
    assert!(
        px(&shadowed, C, C)[0] + 10 < px(&open, C, C)[0],
        "control: the block shadows the pane ({:?} against {:?})",
        px(&shadowed, C, C),
        px(&open, C, C)
    );
    assert_eq!(
        px(&rained, C, C),
        px(&shadowed, C, C),
        "rain does not move the pane's shading"
    );
}

// --- `blend_depth: nearest` (W3) ------------------------------------------------------------------

/// A cube of `side` as one part of a figure: opacity `color[3]`, flat emissive, the given sort group.
fn figure_part(center: Vec3, side: f32, color: [f32; 4], group: u32, nearest: bool) -> BlendedMesh {
    let mut instance = slab(center, side, [color[0], color[1], color[2]]);
    instance.color = color;
    instance.cast_shadows = false;
    BlendedMesh {
        instance,
        mode: BlendMode::Alpha,
        lit: true,
        nearest,
        group,
    }
}

const NEAR_RED: [f32; 3] = [0.9, 0.1, 0.1];
const FAR_BLUE: [f32; 3] = [0.1, 0.1, 0.9];

/// A two-part figure in one group: a near red cube, then a far blue one behind it, listed near first so a
/// plain blend lets the far part overdraw the near one.
fn ghost(alpha: f32, nearest: bool) -> Vec<BlendedMesh> {
    vec![
        figure_part(
            Vec3::new(0.0, 0.0, -6.0),
            2.0,
            [NEAR_RED[0], NEAR_RED[1], NEAR_RED[2], alpha],
            0,
            nearest,
        ),
        figure_part(
            Vec3::new(0.8, 0.0, -10.0),
            3.0,
            [FAR_BLUE[0], FAR_BLUE[1], FAR_BLUE[2], alpha],
            0,
            nearest,
        ),
    ]
}

fn max_gap(a: &RenderedImage, b: &RenderedImage) -> u8 {
    a.rgba
        .iter()
        .zip(&b.rgba)
        .map(|(x, y)| x.abs_diff(*y))
        .max()
        .unwrap_or(0)
}

/// (a) At opacity 1 a `nearest` figure is its opaque twin, front surfaces only; without `nearest` the far
/// part overdraws the near one. Skips with no GPU.
#[test]
fn needs_gpu_a_nearest_figure_at_full_opacity_is_its_opaque_twin() {
    let Some(renderer) = skip_or_renderer("a_nearest_figure_at_full_opacity_is_its_opaque_twin")
    else {
        return;
    };
    let twin: Vec<MeshInstance> = ghost(1.0, false).iter().map(|p| p.instance).collect();
    let opaque = renderer
        .render(&scene(twin, vec![]), size())
        .expect("opaque");
    let nearest = renderer
        .render(&scene(vec![], ghost(1.0, true)), size())
        .expect("nearest");
    assert!(
        max_gap(&opaque, &nearest) <= 2,
        "the nearest figure differs from its opaque twin by {} LSB",
        max_gap(&opaque, &nearest)
    );
    assert_near(
        px(&nearest, C, C),
        bytes(NEAR_RED),
        3.0,
        "the near part covers the far one",
    );
    let plain = renderer
        .render(&scene(vec![], ghost(1.0, false)), size())
        .expect("plain");
    assert_near(
        px(&plain, C, C),
        bytes(FAR_BLUE),
        3.0,
        "control: without nearest the far part overdraws",
    );
}

/// (b) At opacity 0.5 a `nearest` figure's covered pixel is one layer over the background, not two.
/// Skips with no GPU.
#[test]
fn needs_gpu_a_nearest_figure_blends_one_layer() {
    let Some(renderer) = skip_or_renderer("a_nearest_figure_blends_one_layer") else {
        return;
    };
    let frame = renderer
        .render(&scene(vec![], ghost(0.5, true)), size())
        .expect("nearest");
    assert_near(
        px(&frame, C, C),
        bytes(over(CLEAR, &[(NEAR_RED, 0.5)])),
        3.0,
        "front surface over the background",
    );
    let plain = renderer
        .render(&scene(vec![], ghost(0.5, false)), size())
        .expect("plain");
    assert_near(
        px(&plain, C, C),
        bytes(over(CLEAR, &[(NEAR_RED, 0.5), (FAR_BLUE, 0.5)])),
        3.0,
        "control: two layers over the background",
    );
    // The copy starts fresh each frame: a repeat frame is byte-identical.
    let again = renderer
        .render(&scene(vec![], ghost(0.5, true)), size())
        .expect("again");
    assert_eq!(frame.rgba, again.rgba);
}

/// (c) A `nearest` group behind an opaque wall is hidden where the wall is: the copy carries the opaque
/// depth. Skips with no GPU.
#[test]
fn needs_gpu_a_nearest_group_behind_a_wall_is_hidden() {
    let Some(renderer) = skip_or_renderer("a_nearest_group_behind_a_wall_is_hidden") else {
        return;
    };
    let wall = vec![slab(Vec3::new(0.0, 0.0, -4.0), 1.0, [0.8, 0.1, 0.1])];
    let frame = renderer
        .render(&scene(wall, ghost(1.0, true)), size())
        .expect("frame");
    assert_near(
        px(&frame, C, C),
        bytes([0.8, 0.1, 0.1]),
        3.0,
        "the wall hides the figure",
    );
    // Beside the wall the figure shows.
    let camera = scene(vec![], vec![]).camera;
    let beside = project(&camera, Vec3::new(2.1, 0.0, -8.5), PX, PX);
    assert_near(
        px(&frame, beside.0, beside.1),
        bytes(FAR_BLUE),
        3.0,
        "the figure shows past the wall edge",
    );
}

/// (d) Each group copies the depth afresh: a nearer-centred group partly behind an earlier group's front
/// surface still draws over it. Skips with no GPU.
#[test]
fn needs_gpu_a_later_nearest_group_is_not_clipped_by_an_earlier_one() {
    let Some(renderer) = skip_or_renderer("a_later_nearest_group_is_not_clipped_by_an_earlier_one")
    else {
        return;
    };
    let groups = vec![
        // A deep cube whose front face (z = -5) lies in front of the small cube below.
        figure_part(
            Vec3::new(0.0, 0.0, -7.0),
            4.0,
            [0.1, 0.8, 0.1, 1.0],
            0,
            true,
        ),
        // Its centre is nearer than the deep cube's, so it draws second, though behind that front face.
        figure_part(
            Vec3::new(0.0, 0.0, -6.0),
            1.0,
            [0.9, 0.1, 0.1, 1.0],
            1,
            true,
        ),
    ];
    let frame = renderer
        .render(&scene(vec![], groups), size())
        .expect("frame");
    assert_near(
        px(&frame, C, C),
        bytes([0.9, 0.1, 0.1]),
        3.0,
        "the later group draws over the earlier one's front surface",
    );
}

/// (e) A sprite behind a `nearest` ghost still draws over it: the ghost wrote nothing into `scene_depth`.
/// Skips with no GPU.
#[test]
fn needs_gpu_a_sprite_behind_a_nearest_ghost_still_draws() {
    let Some(renderer) = skip_or_renderer("a_sprite_behind_a_nearest_ghost_still_draws") else {
        return;
    };
    let mut s = scene(vec![], ghost(1.0, true));
    s.sprites = vec![sized_sprite(
        Vec3::new(0.0, 0.0, -9.0),
        1.0,
        [0.0, 1.0, 0.0, 1.0],
        false,
    )];
    let frame = renderer.render(&s, size()).expect("frame");
    assert_near(
        px(&frame, C, C),
        bytes([0.0, 1.0, 0.0]),
        3.0,
        "the sprite covers the ghost",
    );
}

/// (f) The transparent layer at a `nearest` figure's pixel holds one layer at its front surface's
/// distance, where a plain blend holds two. Skips with no GPU.
#[test]
fn needs_gpu_the_layer_holds_the_front_surface_of_a_nearest_figure() {
    let Some(renderer) = skip_or_renderer("the_layer_holds_the_front_surface_of_a_nearest_figure")
    else {
        return;
    };
    let layer_at = |nearest: bool| {
        let mut s = scene(vec![], ghost(0.5, nearest));
        s.post = Some(PostData {
            fog_density: 0.02,
            ..exact_post()
        });
        let texels = layer_texels(&renderer, &s, size());
        let [weighted, coverage] = texels[(C * PX + C) as usize];
        (weighted / coverage, coverage)
    };
    let (mean, coverage) = layer_at(true);
    assert!((coverage - 0.5).abs() < 0.01, "coverage {coverage}");
    // The near part's front face: its centre at 6, half a side of 1.
    assert!((mean - 5.0).abs() < 0.05, "mean distance {mean}");
    let (_, plain) = layer_at(false);
    assert!(plain > 0.7, "control: two layers cover {plain}");
}

/// (g) A frame without a `nearest` part derives the chain it always did.
#[test]
fn only_a_nearest_part_asks_the_chain_for_a_depth_copy() {
    let plain = crate::graph::PostChain::for_scene(&scene(vec![], ghost(0.5, false)));
    assert!(plain.blended && !plain.blend_nearest);
    let nearest = crate::graph::PostChain::for_scene(&scene(vec![], ghost(0.5, true)));
    assert!(nearest.blended && nearest.blend_nearest);
}
