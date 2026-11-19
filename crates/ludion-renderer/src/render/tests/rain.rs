//! Rain: GPU tests over the `rain` shader in the transparents pass and the transparent layer.

use super::*;
use crate::rain::RainData;

const PX: u32 = 270;

fn size() -> FrameSize {
    FrameSize::native(PX, PX)
}

fn rain(time_s: f32) -> RainData {
    RainData {
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
        seed: 0,
        time_s,
        per_tile: 96,
    }
}

/// A lit floor under the origin camera, with ambient light and the exact-output post.
fn floor(rain: Option<RainData>) -> RenderScene {
    let mut scene = RenderScene::empty((PX, PX));
    scene.camera = origin_camera(PX, PX);
    scene.ambient = [0.2; 3];
    scene.post = Some(exact_post());
    scene
        .meshes
        .push(slab(Vec3::new(0.0, -16.0, -8.0), 30.0, [0.3; 3]));
    scene.rain = rain;
    scene
}

/// Rain changes the frame, the same time gives the same bytes, and another time gives other bytes.
/// Skips with no GPU.
#[test]
fn needs_gpu_rain_changes_the_frame_and_follows_its_time() {
    let Some(renderer) = skip_or_renderer("rain_changes_the_frame_and_follows_its_time") else {
        return;
    };
    let bare = renderer.render(&floor(None), size()).expect("bare");
    let wet = renderer
        .render(&floor(Some(rain(1.0))), size())
        .expect("wet");
    let again = renderer
        .render(&floor(Some(rain(1.0))), size())
        .expect("again");
    let later = renderer
        .render(&floor(Some(rain(1.25))), size())
        .expect("later");
    assert_ne!(bare.rgba, wet.rgba, "streaks are drawn");
    assert_eq!(wet.rgba, again.rgba, "the same time is byte-identical");
    assert_ne!(wet.rgba, later.rgba, "the drops moved");
}

/// A rain whose streak budget is empty draws nothing: the frame matches one without rain.
#[test]
fn needs_gpu_an_empty_rain_draws_nothing() {
    let Some(renderer) = skip_or_renderer("an_empty_rain_draws_nothing") else {
        return;
    };
    let bare = renderer.render(&floor(None), size()).expect("bare");
    let empty = RainData {
        per_tile: 0,
        ..rain(1.0)
    };
    let drawn = renderer.render(&floor(Some(empty)), size()).expect("empty");
    assert_eq!(bare.rgba, drawn.rgba);
}

/// Under fog the rain also draws into the transparent layer, plain and carrying the medium: both
/// layer pipelines validate and the streaks still change the frame. Skips with no GPU.
#[test]
fn needs_gpu_rain_draws_into_the_transparent_layer_under_fog() {
    let Some(renderer) = skip_or_renderer("rain_draws_into_the_transparent_layer_under_fog") else {
        return;
    };
    let fogged = |rain: Option<RainData>| {
        let mut scene = floor(rain);
        scene.post = Some(PostData {
            fog_density: 0.05,
            ..exact_post()
        });
        scene
    };
    let bare = renderer.render(&fogged(None), size()).expect("bare");
    for medium in [1.0, 0.5] {
        let wet = fogged(Some(RainData {
            medium,
            ..rain(1.0)
        }));
        assert_eq!(
            PostChain::for_scene(&wet).transparent_medium_scheduled(),
            medium < 1.0
        );
        let drawn = renderer.render(&wet, size()).expect("wet");
        assert_ne!(bare.rgba, drawn.rgba, "medium {medium}");
    }
}

/// Rain schedules the transparent cover, and a medium below 1 carries the medium in the layer.
#[test]
fn rain_gates_the_transparent_layer_chain() {
    let bare = PostChain::for_scene(&floor(None));
    assert!(!bare.transparent_cover);
    let wet = PostChain::for_scene(&floor(Some(rain(0.0))));
    assert!(wet.transparent_cover);
    let mut fogged = floor(Some(RainData {
        medium: 0.5,
        ..rain(0.0)
    }));
    fogged.post = Some(PostData {
        fog_density: 0.05,
        ..exact_post()
    });
    let chain = PostChain::for_scene(&fogged);
    assert!(chain.transparent_medium && chain.transparent_medium_scheduled());
}

/// The origin camera lifted to head height, looking down -Z.
fn head_camera(w: u32, h: u32) -> crate::scene::RenderCamera {
    let eye = Vec3::new(0.0, 1.0, 0.0);
    crate::scene::RenderCamera {
        view: Mat4::look_at_rh(eye, eye + Vec3::new(0.0, 0.0, -1.0), Vec3::Y),
        position: eye,
        ..origin_camera(w, h)
    }
}

/// A flat slab `size` across centred on `center`, thin on y.
fn roof(center: Vec3, size: Vec3, casts: bool) -> MeshInstance {
    let mut mesh = slab(Vec3::ZERO, 1.0, [0.4; 3]);
    mesh.model = Mat4::from_translation(center) * Mat4::from_scale(size);
    mesh.cast_shadows = casts;
    mesh
}

/// A floor under a head-height camera, with an optional roof overhead.
fn under_roof(rain: Option<RainData>, roof: Option<MeshInstance>) -> RenderScene {
    let mut scene = floor(rain);
    scene.camera = head_camera(PX, PX);
    scene.meshes.extend(roof);
    scene
}

fn column_differs(a: &RenderedImage, b: &RenderedImage, x: std::ops::Range<u32>) -> bool {
    (0..a.height).any(|y| x.clone().any(|x| px(a, x, y) != px(b, x, y)))
}

fn px(frame: &RenderedImage, x: u32, y: u32) -> [u8; 4] {
    let i = ((y * frame.width + x) * 4) as usize;
    [
        frame.rgba[i],
        frame.rgba[i + 1],
        frame.rgba[i + 2],
        frame.rgba[i + 3],
    ]
}

/// A 60 m roof overhead stops every streak under it, pixel for pixel; `occlusion: false` draws
/// through it. Skips with no GPU.
#[test]
fn needs_gpu_a_roof_stops_the_rain_under_it() {
    let Some(renderer) = skip_or_renderer("a_roof_stops_the_rain_under_it") else {
        return;
    };
    let big = || {
        Some(roof(
            Vec3::new(0.0, 3.0, 0.0),
            Vec3::new(60.0, 0.2, 60.0),
            true,
        ))
    };
    let bare = renderer
        .render(&under_roof(None, big()), size())
        .expect("bare");
    let open = renderer
        .render(&under_roof(Some(rain(1.0)), None), size())
        .expect("open");
    let covered = renderer
        .render(&under_roof(Some(rain(1.0)), big()), size())
        .expect("covered");
    let through = renderer
        .render(
            &under_roof(
                Some(RainData {
                    occlusion: false,
                    ..rain(1.0)
                }),
                big(),
            ),
            size(),
        )
        .expect("through");
    assert_ne!(
        open.rgba,
        renderer
            .render(&under_roof(None, None), size())
            .unwrap()
            .rgba,
        "the open-sky frame has streaks"
    );
    assert_eq!(covered.rgba, bare.rgba, "no streak shows under the roof");
    assert_ne!(through.rgba, bare.rgba, "occlusion false draws through");
}

/// A roof over the left half only: the left half of the frame stays dry and the right half keeps its
/// rain. Skips with no GPU.
#[test]
fn needs_gpu_a_half_roof_leaves_the_open_side_wet() {
    let Some(renderer) = skip_or_renderer("a_half_roof_leaves_the_open_side_wet") else {
        return;
    };
    let half = || {
        Some(roof(
            Vec3::new(-30.0, 3.0, 0.0),
            Vec3::new(60.0, 0.2, 60.0),
            true,
        ))
    };
    let bare = renderer
        .render(&under_roof(None, half()), size())
        .expect("bare");
    let wet = renderer
        .render(&under_roof(Some(rain(1.0)), half()), size())
        .expect("wet");
    let mid = PX / 2;
    assert!(
        !column_differs(&bare, &wet, 0..mid - 4),
        "the roofed side is dry"
    );
    assert!(
        column_differs(&bare, &wet, mid + 4..PX),
        "the open side keeps its rain"
    );
}

/// A roof that casts no shadow does not occlude: the frame equals the one with occlusion off.
/// Skips with no GPU.
#[test]
fn needs_gpu_a_non_caster_roof_does_not_occlude() {
    let Some(renderer) = skip_or_renderer("a_non_caster_roof_does_not_occlude") else {
        return;
    };
    let ghost = || {
        Some(roof(
            Vec3::new(0.0, 3.0, 0.0),
            Vec3::new(60.0, 0.2, 60.0),
            false,
        ))
    };
    let bare = renderer
        .render(&under_roof(None, ghost()), size())
        .expect("bare");
    let kept = renderer
        .render(&under_roof(Some(rain(1.0)), ghost()), size())
        .expect("kept");
    let off = renderer
        .render(
            &under_roof(
                Some(RainData {
                    occlusion: false,
                    ..rain(1.0)
                }),
                ghost(),
            ),
            size(),
        )
        .expect("off");
    assert_ne!(kept.rgba, bare.rgba, "the rain shows under a non-caster");
    assert_eq!(kept.rgba, off.rgba, "nothing occludes it");
}

/// The `rain_occlusion` pass time under a roof at 1080p: a measurement, not a gate. Run with
/// `cargo test -p ludion-renderer --release -- --ignored --nocapture needs_gpu_bench_rain_occlusion_gpu_ms`.
#[test]
#[ignore = "a timing measurement, not a gate — run with --release --ignored --nocapture"]
fn needs_gpu_bench_rain_occlusion_gpu_ms() {
    let Some(renderer) = skip_or_renderer("bench_rain_occlusion_gpu_ms") else {
        return;
    };
    let size = FrameSize::native(1920, 1080);
    let mut scene = under_roof(
        Some(RainData {
            drop_width: 0.006,
            color: [0.55, 0.6, 0.65, 0.25],
            ..rain(2.0)
        }),
        Some(roof(
            Vec3::new(0.0, 3.0, 0.0),
            Vec3::new(8.0, 0.2, 8.0),
            true,
        )),
    );
    scene.camera = head_camera(1920, 1080);
    let mut best = f64::MAX;
    for _ in 0..12 {
        let json = renderer.dump_timings_json(&scene, size).expect("timings");
        if let Some(ms) = pass_gpu_time_ms(&json, "rain_occlusion") {
            best = best.min(ms);
        }
    }
    eprintln!("rain_occlusion: {best:.3} ms (best of 12, 1080p)");
}

/// The `transparents` pass time with and without rain at the default density: a measurement, not a
/// gate. Run with
/// `cargo test -p ludion-renderer --release -- --ignored --nocapture needs_gpu_bench_rain_gpu_ms`.
#[test]
#[ignore = "a timing measurement, not a gate — run with --release --ignored --nocapture"]
fn needs_gpu_bench_rain_gpu_ms() {
    let Some(renderer) = skip_or_renderer("bench_rain_gpu_ms") else {
        return;
    };
    let size = FrameSize::native(1920, 1080);
    let mut best = [f64::MAX; 2];
    for (slot, with_rain) in [false, true].into_iter().enumerate() {
        let mut scene = floor(with_rain.then(|| RainData {
            drop_width: 0.006,
            color: [0.55, 0.6, 0.65, 0.25],
            ..rain(2.0)
        }));
        scene.camera = origin_camera(1920, 1080);
        for _ in 0..12 {
            let json = renderer.dump_timings_json(&scene, size).expect("timings");
            if let Some(ms) = pass_gpu_time_ms(&json, "transparents") {
                best[slot] = best[slot].min(ms);
            }
        }
    }
    eprintln!(
        "transparents: {:.3} ms without rain, {:.3} ms with rain (best of 12, 1080p)",
        best[0], best[1]
    );
}

/// One pass's `gpu_time_ms` out of a timings document, `None` when the adapter reports none.
fn pass_gpu_time_ms(json: &str, pass: &str) -> Option<f64> {
    let at = json.find(&format!("\"{pass}\""))?;
    let rest = &json[at..];
    let key = "\"gpu_time_ms\"";
    let rest = &rest[rest.find(key)? + key.len()..];
    let rest = rest.trim_start_matches([':', ' ', '\n']);
    let end = rest.find(|c: char| !(c.is_ascii_digit() || c == '.' || c == 'e' || c == '-'))?;
    rest[..end].parse().ok()
}
