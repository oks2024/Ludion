//! Secondary views: GPU tests over `views::render_views` and the UI pass's view rects.

use super::*;
use crate::probe::{ProbeData, ProbeVolume};
use crate::scene::{RenderCamera, RenderViewData};
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
