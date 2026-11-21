//! Secondary views: GPU tests over `views::render_views` and the UI pass's view rects.

use super::screen_effect::neutral;
use super::*;
use crate::graph::{PostChain, build_frame_graph};
use crate::probe::{ProbeData, ProbeVolume};
use crate::scene::{RenderCamera, RenderViewData};
use crate::screen_effect::ScreenEffectData;
use crate::view_scene;
use ludion_core::math::glam::Quat;

const PX: u32 = 256;
const VIEW_PX: u32 = 64;

/// The main camera looks away (+Z) from a red slab at -Z; the view camera looks at it.
fn scene() -> RenderScene {
    let mut scene = RenderScene::empty((PX, PX));
    scene.camera = RenderCamera {
        view: Mat4::look_at_rh(Vec3::ZERO, Vec3::Z, Vec3::Y),
        ..origin_camera(PX, PX)
    };
    scene.ambient = [0.0; 3];
    scene.post = Some(exact_post());
    scene.meshes = vec![slab(Vec3::new(0.0, 0.0, -5.0), 2.0, [1.0, 0.0, 0.0])];
    scene.views = vec![RenderViewData {
        name: "feed".into(),
        size: (VIEW_PX, VIEW_PX),
        camera: origin_camera(VIEW_PX, VIEW_PX),
        layers: u32::MAX,
        lights: u32::MAX,
        sun: true,
        screen_effects: Vec::new(),
    }];
    scene
}

fn with_rect(mut scene: RenderScene, view: Option<u32>) -> RenderScene {
    scene.ui = vec![UiInstance::Rect {
        rect_px: [64.0, 64.0, 128.0, 128.0],
        texture: None,
        color: [1.0; 4],
        uv: [0.0, 0.0, 1.0, 1.0],
        clip: UiClip::NONE,
        view,
        rotation: crate::UiRotation::NONE,
    }];
    scene
}

fn px(frame: &RenderedImage, x: u32, y: u32) -> [u8; 3] {
    let i = ((y * frame.width + x) * 4) as usize;
    [frame.rgba[i], frame.rgba[i + 1], frame.rgba[i + 2]]
}

const C: u32 = PX / 2;

/// A rect naming a view shows what the view's camera sees; the same pixel of a view-free frame is the
/// clear colour. Skips with no GPU.
#[test]
fn needs_gpu_a_view_rect_shows_the_view_camera() {
    let Some(renderer) = skip_or_renderer("a_view_rect_shows_the_view_camera") else {
        return;
    };
    let shown = renderer
        .render(&with_rect(scene(), Some(0)), FrameSize::native(PX, PX))
        .expect("shown");
    let [r, g, b] = px(&shown, C, C);
    assert!(
        r > 200 && g < 20 && b < 20,
        "the rect centre is red: {r},{g},{b}"
    );
    let mut bare = scene();
    bare.views.clear();
    let bare = renderer
        .render(&bare, FrameSize::native(PX, PX))
        .expect("bare");
    assert_eq!(
        px(&bare, C, C),
        px(&bare, 8, 8),
        "no view: the clear colour fills the centre"
    );
    assert_ne!(px(&bare, C, C), px(&shown, C, C));
    assert_eq!(
        px(&shown, 8, 8),
        px(&bare, 8, 8),
        "outside the rect nothing moves"
    );
}

/// `render_view` returns the view's own size, deterministically. Skips with no GPU.
#[test]
fn needs_gpu_render_view_is_sized_and_deterministic() {
    let Some(renderer) = skip_or_renderer("render_view_is_sized_and_deterministic") else {
        return;
    };
    let scene = scene();
    let a = renderer.render_view(&scene, 0).expect("a");
    let b = renderer.render_view(&scene, 0).expect("b");
    assert_eq!((a.width, a.height), (VIEW_PX, VIEW_PX));
    assert_eq!(a.rgba.len(), (VIEW_PX * VIEW_PX * 4) as usize);
    assert_eq!(a.rgba, b.rgba);
    let [r, g, b] = px(&a, VIEW_PX / 2, VIEW_PX / 2);
    assert!(r > 200 && g < 20 && b < 20);
}

/// A view that no rect names leaves the main frame as it was without views.
#[test]
fn needs_gpu_an_unshown_view_changes_nothing() {
    let Some(renderer) = skip_or_renderer("an_unshown_view_changes_nothing") else {
        return;
    };
    let size = FrameSize::native(PX, PX);
    let viewed = renderer.render(&scene(), size).expect("viewed");
    let mut bare = scene();
    bare.views.clear();
    let bare = renderer.render(&bare, size).expect("bare");
    assert_eq!(viewed.rgba, bare.rgba);
}

/// A frame with a probe and a view resolves the probes once, not once per view, and its main image
/// is the view-free one. Skips with no GPU.
#[test]
fn needs_gpu_a_view_does_not_resolve_the_probes_again() {
    let probe = ProbeData {
        entity: ludion_core::ids::EntityId::new(1, 0),
        position: Vec3::new(0.0, 1.5, 0.0),
        rotation: Quat::IDENTITY,
        volume: ProbeVolume::Sphere { radius: 5.0 },
        blend: 0.5,
        priority: 0,
    };
    let size = FrameSize::native(PX, PX);
    let mut viewed = scene();
    viewed.probes = vec![probe];
    let mut bare = viewed.clone();
    bare.views.clear();

    let Some(first) = skip_or_renderer("a_view_does_not_resolve_the_probes_again") else {
        return;
    };
    let with_view = first.render(&viewed, size).expect("viewed");
    assert_eq!(
        first.probe_resolve().resolve,
        1,
        "one resolve for a frame with a view"
    );
    let second = Renderer::new().expect("renderer");
    let without = second.render(&bare, size).expect("bare");
    assert_eq!(second.probe_resolve().resolve, 1);
    assert_eq!(with_view.rgba, without.rgba, "the main frame minus its UI");
}

/// A layered frame with a probe, a radar-only mesh and a view on `radar`: the view draws a different mesh
/// list than the main frame, yet the probes resolve once, and again for no later frame. Skips with no GPU.
#[test]
fn needs_gpu_a_layered_view_does_not_resolve_the_probes_again() {
    let probe = ProbeData {
        entity: ludion_core::ids::EntityId::new(1, 0),
        position: Vec3::new(0.0, 1.5, 0.0),
        rotation: Quat::IDENTITY,
        volume: ProbeVolume::Sphere { radius: 5.0 },
        blend: 0.5,
        priority: 0,
    };
    const RADAR: u32 = 2;
    let mut full = scene();
    full.probes = vec![probe];
    full.meshes
        .push(slab(Vec3::new(3.0, 0.0, -5.0), 1.0, [0.0, 0.0, 1.0]));
    full.views[0].layers = RADAR;
    let masks = crate::LayerMasks {
        main: crate::DEFAULT_LAYER_BIT,
        meshes: vec![crate::DEFAULT_LAYER_BIT, RADAR],
        ..crate::LayerMasks::default()
    };
    let (layered, _) = crate::split_layers(full, masks);
    assert_eq!(layered.meshes.len(), 1);
    assert_eq!(view_scene(&layered, 0).meshes.len(), 1);

    let Some(renderer) = skip_or_renderer("a_layered_view_does_not_resolve_the_probes_again")
    else {
        return;
    };
    let size = FrameSize::native(PX, PX);
    renderer.render(&layered, size).expect("first frame");
    renderer.render(&layered, size).expect("second frame");
    assert_eq!(
        renderer.probe_resolve().resolve,
        1,
        "the main frame and its view share one capture"
    );
}

/// A `GiVolume` scene whose view lights by its own set: the view draws without GI, so two frames bake
/// the main frame's volume once and the view never rebakes it. Skips with no GPU.
#[test]
fn needs_gpu_a_view_on_its_own_light_set_never_rebakes_the_gi() {
    use crate::scene::{GiData, PointLightData, WorldVoxelGrid};
    const PORTRAIT: u32 = 2;
    let total = 8 * 8 * 8;
    let grid = std::sync::Arc::new(
        WorldVoxelGrid {
            origin: Vec3::ZERO,
            cell_size: 1.0,
            extent: [8, 8, 8],
            occupancy: (0..total).map(|i| i % 9 == 0).collect(),
            albedo: vec![[1.0, 0.1, 0.1, 1.0]; total],
            emissive: vec![0.0; total],
            hash: 0,
        }
        .sealed(),
    );
    let light = |x: f32| PointLightData {
        position: Vec3::new(x, 4.0, 4.0),
        color: Vec3::ONE,
        intensity: 1.0,
        range: 6.0,
        dynamic: false,
        shadow: None,
        scatter: 0.0,
    };
    let mut full = scene();
    full.lights = vec![light(2.0), light(6.0)];
    full.voxel = Some(grid);
    full.gi = Some(GiData {
        probe_spacing: 1.0,
        normal_bias: 0.25,
        sky_visibility: 1.0,
        bounce: 1.0,
        count: [9; 3],
        origin: Vec3::ZERO,
        span: Vec3::splat(8.0),
    });
    full.views[0].layers = crate::DEFAULT_LAYER_BIT;
    full.views[0].lights = PORTRAIT;
    let masks = crate::LayerMasks {
        main: crate::DEFAULT_LAYER_BIT,
        main_lights: crate::DEFAULT_LAYER_BIT,
        meshes: vec![crate::DEFAULT_LAYER_BIT],
        lights: vec![crate::DEFAULT_LAYER_BIT, PORTRAIT],
        ..crate::LayerMasks::default()
    };
    let (layered, _) = crate::split_layers(full, masks);
    assert_eq!(layered.lights.len(), 1);
    let derived = view_scene(&layered, 0);
    assert!(derived.gi.is_none() && derived.voxel.is_none());

    let Some(renderer) = skip_or_renderer("a_view_on_its_own_light_set_never_rebakes_the_gi")
    else {
        return;
    };
    let size = FrameSize::native(PX, PX);
    renderer.render(&layered, size).expect("first frame");
    renderer.render(&layered, size).expect("second frame");
    assert_eq!(
        renderer.gi_bake.bake_count(),
        1,
        "the main frame bakes once; the view draws without GI"
    );
}

/// The view's GPU time at two sizes on a lit room: a measurement, not a gate. Run with
/// `cargo test -p ludion-renderer --release -- --ignored --nocapture needs_gpu_bench_view_gpu_ms`.
#[test]
#[ignore = "a timing measurement, not a gate — run with --release --ignored --nocapture"]
fn needs_gpu_bench_view_gpu_ms() {
    let Some(renderer) = skip_or_renderer("bench_view_gpu_ms") else {
        return;
    };
    let mut room = scene();
    room.ambient = crate::DEFAULT_AMBIENT;
    room.post = None;
    room.sun = Some(crate::scene::SunData {
        direction: Vec3::new(-0.4, -1.0, -0.3).normalize(),
        color: Vec3::splat(2.0),
        light_space: Mat4::orthographic_rh(-20.0, 20.0, -20.0, 20.0, 1.0, 40.0)
            * Mat4::look_to_rh(
                Vec3::new(0.4, 1.0, 0.3).normalize() * 20.0,
                Vec3::new(-0.4, -1.0, -0.3).normalize(),
                Vec3::Y,
            ),
        bias: 0.002,
        angular_radius: 0.0,
        cascades: None,
    });
    room.meshes
        .push(slab(Vec3::new(0.0, -1.0, 0.0), 30.0, [0.4; 3]));
    for edge in [512u32, 256] {
        room.views[0].size = (edge, edge);
        room.views[0].camera = origin_camera(edge, edge);
        let derived = view_scene(&room, 0);
        let size = FrameSize::native(edge, edge);
        let mut best = f64::MAX;
        for _ in 0..8 {
            let json = renderer.dump_timings_json(&derived, size).expect("timings");
            if let Some(ms) = frame_gpu_time_ms(&json) {
                best = best.min(ms);
            }
        }
        eprintln!("view {edge}x{edge}: {best:.3} ms GPU (best of 8, summed passes)");
    }
}

/// `frame_gpu_time_ms` out of a timings document, `None` when the adapter reports none.
fn frame_gpu_time_ms(json: &str) -> Option<f64> {
    let rest = &json[json.find("\"frame_gpu_time_ms\"")? + "\"frame_gpu_time_ms\"".len()..];
    let rest = rest.trim_start_matches([':', ' ', '\n']);
    let end = rest.find(|c: char| !(c.is_ascii_digit() || c == '.' || c == 'e' || c == '-'))?;
    rest[..end].parse().ok()
}

/// A view-wide effect: every stage off but the one a test turns on.
fn view_fx(stage: impl FnOnce(&mut ScreenEffectData)) -> ScreenEffectData {
    let mut fx = neutral([0.0, 0.0, VIEW_PX as f32, VIEW_PX as f32], 0);
    stage(&mut fx);
    fx
}

fn with_view_effects(mut scene: RenderScene, effects: Vec<ScreenEffectData>) -> RenderScene {
    scene.views[0].screen_effects = effects;
    scene
}

/// A view's own effect list builds `overlay` and `screen_fx_0` in the view's graph; an empty list
/// builds neither.
#[test]
fn a_view_effect_builds_the_screen_fx_chain_in_the_views_graph() {
    let names = |scene: &RenderScene| {
        let chain = PostChain::for_scene(&view_scene(scene, 0));
        let (graph, _) = build_frame_graph(FrameSize::native(VIEW_PX, VIEW_PX), chain);
        graph
            .passes()
            .iter()
            .map(|p| p.name.to_string())
            .collect::<Vec<_>>()
    };
    let plain = names(&scene());
    assert!(
        !plain
            .iter()
            .any(|n| n == "overlay" || n.starts_with("screen_fx"))
    );
    let fx = with_view_effects(scene(), vec![view_fx(|f| f.scanlines = 0.5)]);
    assert_eq!(PostChain::for_scene(&view_scene(&fx, 0)).screen_effects, 1);
    let names = names(&fx);
    assert_eq!(names[names.len() - 2..], ["overlay", "screen_fx_0"]);
}

/// A view's own effect is applied in its texture; with none the view is what it was. Skips with no GPU.
#[test]
fn needs_gpu_a_view_effect_changes_the_view_texture() {
    let Some(renderer) = skip_or_renderer("a_view_effect_changes_the_view_texture") else {
        return;
    };
    let plain = renderer.render_view(&scene(), 0).expect("plain");
    let mut empty = scene();
    empty.views[0].screen_effects = Vec::new();
    assert_eq!(
        plain.rgba,
        renderer.render_view(&empty, 0).expect("empty").rgba
    );
    let fx = with_view_effects(scene(), vec![view_fx(|f| f.scanlines = 0.5)]);
    let lined = renderer.render_view(&fx, 0).expect("lined");
    assert_eq!((lined.width, lined.height), (VIEW_PX, VIEW_PX));
    assert_ne!(plain.rgba, lined.rgba);
}

/// An animated stage is a function of the tick: frame 0 and 1 differ in the view, one frame twice is
/// identical. Skips with no GPU.
#[test]
fn needs_gpu_a_view_effect_animates_from_the_tick() {
    let Some(renderer) = skip_or_renderer("a_view_effect_animates_from_the_tick") else {
        return;
    };
    let at = |frame: u32| {
        let fx = with_view_effects(
            scene(),
            vec![view_fx(|f| {
                f.noise = 0.5;
                f.frame = frame;
            })],
        );
        renderer.render_view(&fx, 0).expect("view").rgba
    };
    assert_eq!(at(0), at(0));
    assert_ne!(at(0), at(1));
}

/// A view effect is baked into the texture the rect shows: the main frame differs from the
/// effect-free one only inside that rect. Skips with no GPU.
#[test]
fn needs_gpu_a_view_effect_moves_only_the_rect_that_shows_it() {
    let Some(renderer) = skip_or_renderer("a_view_effect_moves_only_the_rect_that_shows_it") else {
        return;
    };
    let size = FrameSize::native(PX, PX);
    let plain = renderer
        .render(&with_rect(scene(), Some(0)), size)
        .expect("plain");
    let fx = with_view_effects(scene(), vec![view_fx(|f| f.scanlines = 0.9)]);
    let lined = renderer
        .render(&with_rect(fx, Some(0)), size)
        .expect("lined");
    let (mut inside, mut outside) = (0, 0);
    for y in 0..PX {
        for x in 0..PX {
            if px(&plain, x, y) != px(&lined, x, y) {
                if (64..192).contains(&x) && (64..192).contains(&y) {
                    inside += 1;
                } else {
                    outside += 1;
                }
            }
        }
    }
    assert!(inside > 0, "the rect shows the filtered view");
    assert_eq!(outside, 0, "nothing outside the rect moves");
}
