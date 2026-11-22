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
        wetness: 0.0,
        ripples: 0.0,
        splash_rate: 0.0,
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
    mesh.rain_occluder = casts;
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

/// The rain flag list is empty when every instance occludes, and one entry per mesh otherwise.
#[test]
fn rain_casts_are_empty_when_every_instance_occludes() {
    let mut meshes = vec![slab(Vec3::ZERO, 1.0, [0.3; 3]); 3];
    assert!(caster_flags(&meshes, |m| m.rain_occluder).is_empty());
    meshes[1].rain_occluder = false;
    assert_eq!(
        caster_flags(&meshes, |m| m.rain_occluder),
        [true, false, true]
    );
    assert!(caster_flags(&meshes, |m| m.cast_shadows).is_empty());
}

/// `RainOccluder: {}` on a roof that casts no shadow makes it occlude: with 60 m of roof overhead
/// the rained frame equals the rain-free one. Skips with no GPU.
#[test]
fn needs_gpu_a_non_caster_occluder_roof_stops_the_rain() {
    let Some(renderer) = skip_or_renderer("a_non_caster_occluder_roof_stops_the_rain") else {
        return;
    };
    let tagged = || {
        let mut mesh = roof(Vec3::new(0.0, 3.0, 0.0), Vec3::new(60.0, 0.2, 60.0), false);
        mesh.rain_occluder = true;
        Some(mesh)
    };
    let bare = renderer
        .render(&under_roof(None, tagged()), size())
        .expect("bare");
    let covered = renderer
        .render(&under_roof(Some(rain(1.0)), tagged()), size())
        .expect("covered");
    assert_eq!(
        covered.rgba, bare.rgba,
        "the tagged non-caster stops every streak"
    );
}

/// `RainOccluder: { occludes: false }` exempts a caster: the rain draws under it, equal to the frame
/// with occlusion off. Skips with no GPU.
#[test]
fn needs_gpu_an_exempt_caster_roof_lets_the_rain_through() {
    let Some(renderer) = skip_or_renderer("an_exempt_caster_roof_lets_the_rain_through") else {
        return;
    };
    let net = || {
        let mut mesh = roof(Vec3::new(0.0, 3.0, 0.0), Vec3::new(60.0, 0.2, 60.0), true);
        mesh.rain_occluder = false;
        Some(mesh)
    };
    let bare = renderer
        .render(&under_roof(None, net()), size())
        .expect("bare");
    let through = renderer
        .render(&under_roof(Some(rain(1.0)), net()), size())
        .expect("through");
    let off = renderer
        .render(
            &under_roof(
                Some(RainData {
                    occlusion: false,
                    ..rain(1.0)
                }),
                net(),
            ),
            size(),
        )
        .expect("off");
    assert_ne!(
        through.rgba, bare.rgba,
        "streaks show under the exempt caster"
    );
    assert_eq!(through.rgba, off.rgba, "nothing occludes it");
}

/// `rain_density` thins the streaks drawn: half the density renders what a scene authoring half the
/// per-tile count renders at full, and differs from the full rain. Skips with no GPU.
#[test]
fn needs_gpu_rain_density_draws_a_prefix_of_the_same_drops() {
    let Some(mut renderer) = skip_or_renderer("rain_density_draws_a_prefix_of_the_same_drops")
    else {
        return;
    };
    let full = renderer
        .render(&floor(Some(rain(1.0))), size())
        .expect("full");
    let half_count = crate::rain::rain_draw_per_tile(96, 0.5);
    let authored = renderer
        .render(
            &floor(Some(RainData {
                per_tile: half_count,
                ..rain(1.0)
            })),
            size(),
        )
        .expect("authored");
    renderer.set_quality(
        Quality::FULL,
        QualityParams {
            rain_density: 0.5,
            ..QualityParams::FULL
        },
    );
    let thinned = renderer
        .render(&floor(Some(rain(1.0))), size())
        .expect("thinned");
    assert_ne!(
        thinned.rgba, full.rgba,
        "half the streaks differ from all of them"
    );
    assert_eq!(
        thinned.rgba, authored.rgba,
        "the thinned rain is the prefix of the same drops"
    );
}

/// The `rain_occlusion` view tints a floor under a roof orange and an open floor blue; a rain-free
/// scene draws the normal frame. Skips with no GPU.
#[test]
fn needs_gpu_the_rain_occlusion_view_tints_covered_and_exposed_surfaces() {
    let Some(renderer) =
        skip_or_renderer("the_rain_occlusion_view_tints_covered_and_exposed_surfaces")
    else {
        return;
    };
    let half = || {
        Some(roof(
            Vec3::new(-30.0, 3.0, 0.0),
            Vec3::new(60.0, 0.2, 60.0),
            true,
        ))
    };
    let mut viewed = matte_under_roof(1.0, half());
    viewed.debug_view = Some(crate::DebugView::RainOcclusion);
    let frame = renderer.render(&viewed, size()).expect("viewed");
    let (covered, exposed) = (px(&frame, PX / 4, PX - 6), px(&frame, PX * 3 / 4, PX - 6));
    assert!(
        covered[0] > covered[2],
        "under the roof is orange-dominant: {covered:?}"
    );
    assert!(
        exposed[2] > exposed[0],
        "the open floor is blue-dominant: {exposed:?}"
    );

    let mut dry = floor(None);
    let normal = renderer.render(&dry, size()).expect("normal");
    dry.debug_view = Some(crate::DebugView::RainOcclusion);
    let unviewed = renderer.render(&dry, size()).expect("rain-free view");
    assert_eq!(
        unviewed.rgba, normal.rgba,
        "a view never conjures its subject"
    );
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

// --- Wetness -------------------------------------------------------------------------------------

/// A rain with no streaks: only the wetness can change a frame.
fn wet_rain(wetness: f32) -> RainData {
    RainData {
        per_tile: 0,
        wetness,
        ripples: 0.0,
        ..rain(0.0)
    }
}

/// An sRGB-encoded 8-bit channel back to linear light.
fn linear(v: u8) -> f32 {
    let c = f32::from(v) / 255.0;
    if c <= 0.040_45 {
        c / 12.92
    } else {
        ((c + 0.055) / 1.055).powf(2.4)
    }
}

/// One matte, ambient-lit face filling the frame, `distance` ahead of a camera at `eye` looking along
/// `dir`: its normal faces the camera. The rain has no streaks and no occlusion, so exposure is 1 and
/// the facing term alone decides how wet it looks.
fn facing_scene(eye: Vec3, dir: Vec3, up: Vec3, wetness: f32) -> RenderScene {
    let mut scene = RenderScene::empty((PX, PX));
    scene.camera = crate::scene::RenderCamera {
        view: Mat4::look_to_rh(eye, dir, up),
        position: eye,
        ..origin_camera(PX, PX)
    };
    scene.ambient = [0.5; 3];
    scene.post = Some(exact_post());
    // A 30 m cube whose near face is 4 m from the eye.
    let mut face = slab(eye + dir * 19.0, 30.0, [0.5; 3]);
    face.material = [0.8, 0.0, 0.0];
    scene.meshes.push(face);
    scene.rain = Some(RainData {
        occlusion: false,
        ..wet_rain(wetness)
    });
    scene
}

fn centre(frame: &RenderedImage) -> u8 {
    px(frame, PX / 2, PX / 2)[0]
}

/// At full wetness the albedo of a matte surface darkens by `mix(1, 0.6, 0.5)` facing up, by half that
/// pull on a wall, and not at all facing down. Skips with no GPU.
#[test]
fn needs_gpu_wetness_darkens_by_facing() {
    let Some(renderer) = skip_or_renderer("wetness_darkens_by_facing") else {
        return;
    };
    let ratio = |eye: Vec3, dir: Vec3, up: Vec3| {
        let dry = renderer
            .render(&facing_scene(eye, dir, up, 0.0), size())
            .expect("dry");
        let wet = renderer
            .render(&facing_scene(eye, dir, up, 1.0), size())
            .expect("wet");
        (linear(centre(&wet)) / linear(centre(&dry)), dry, wet)
    };
    let (up, ..) = ratio(Vec3::new(0.0, 4.0, 0.0), -Vec3::Y, -Vec3::Z);
    let (wall, ..) = ratio(Vec3::ZERO, -Vec3::Z, Vec3::Y);
    let (down, dry, wet) = ratio(Vec3::ZERO, Vec3::Y, -Vec3::Z);
    assert!(
        (up - 0.8).abs() < 0.03,
        "facing up darkens to 0.8, got {up}"
    );
    assert!(
        (wall - 0.9).abs() < 0.03,
        "a wall darkens to 0.9, got {wall}"
    );
    assert_eq!(dry.rgba, wet.rgba, "a down-facing surface stays dry");
    assert_eq!(down, 1.0);
}

/// Wetness drops the roughness: a rough plane under a sun that mirrors toward the camera gets a
/// brighter highlight peak when wet. Skips with no GPU.
#[test]
fn needs_gpu_wetness_glosses_a_rough_surface() {
    let Some(renderer) = skip_or_renderer("wetness_glosses_a_rough_surface") else {
        return;
    };
    let (w, h) = (64u32, 64u32);
    let frame = FrameSize::native(w, h);
    let peak = |wetness: f32, metallic: f32| {
        let mut scene = RenderScene::empty((w, h));
        scene.post = Some(exact_post());
        scene.ambient = [0.0; 3];
        let eye = Vec3::new(0.0, 3.0, -5.196);
        scene.camera = crate::scene::RenderCamera {
            view: Mat4::look_at_rh(eye, Vec3::ZERO, Vec3::Y),
            projection: Mat4::perspective_rh(40.0_f32.to_radians(), 1.0, 0.1, 100.0),
            position: eye,
        };
        let mut floor = ground(20.0);
        floor.color = [1.0; 4];
        floor.material = [0.7, metallic, 0.0];
        scene.meshes = vec![floor];
        scene.sun = Some(test_sun(
            Vec3::new(0.0, -0.5, -0.866),
            Vec3::splat(0.4),
            0.002,
        ));
        scene.rain = Some(RainData {
            occlusion: false,
            ..wet_rain(wetness)
        });
        let img = renderer.render(&scene, frame).expect("render");
        (0..h)
            .flat_map(|y| (0..w).map(move |x| (x, y)))
            .map(|(x, y)| luma(&img, x, y))
            .max()
            .unwrap_or(0)
    };
    assert!(
        peak(1.0, 0.0) > peak(0.0, 0.0),
        "the wet highlight peaks higher"
    );
}

/// A matte floor under a head-height camera with an ambient light, rained on (no streaks) at
/// `wetness`, with an optional roof.
fn matte_under_roof(wetness: f32, roof: Option<MeshInstance>) -> RenderScene {
    let mut scene = under_roof(Some(wet_rain(wetness)), roof);
    scene.ambient = [0.6; 3];
    scene.meshes[0].material = [0.8, 0.0, 0.0];
    scene
}

/// The frame's near floor rows: far floor and the horizon lie past the occlusion map's edge fade.
fn near_floor(a: &RenderedImage, b: &RenderedImage, x: std::ops::Range<u32>) -> bool {
    (PX * 3 / 4..PX).any(|y| x.clone().any(|x| px(a, x, y) != px(b, x, y)))
}

/// Floor under a roof is dry, byte for byte, inside the roof's footprint away from the edge fade; the
/// open half of the same frame changes. Skips with no GPU.
#[test]
fn needs_gpu_wetness_leaves_the_roofed_floor_dry() {
    let Some(renderer) = skip_or_renderer("wetness_leaves_the_roofed_floor_dry") else {
        return;
    };
    let half = || {
        Some(roof(
            Vec3::new(-30.0, 3.0, 0.0),
            Vec3::new(60.0, 0.2, 60.0),
            true,
        ))
    };
    let dry = renderer
        .render(&matte_under_roof(0.0, half()), size())
        .expect("dry");
    let wet = renderer
        .render(&matte_under_roof(1.0, half()), size())
        .expect("wet");
    let mid = PX / 2;
    assert!(
        !near_floor(&dry, &wet, 0..mid - 4),
        "the roofed floor stays dry"
    );
    assert!(
        near_floor(&dry, &wet, mid + 4..PX),
        "the open floor gets wet"
    );
}

/// The `scene` pass time of a wet matte floor at wetness 0 and 1: a measurement, not a gate. Run with
/// `cargo test -p ludion-renderer --release -- --ignored --nocapture needs_gpu_bench_wetness_gpu_ms`.
#[test]
#[ignore = "a timing measurement, not a gate — run with --release --ignored --nocapture"]
fn needs_gpu_bench_wetness_gpu_ms() {
    let Some(renderer) = skip_or_renderer("bench_wetness_gpu_ms") else {
        return;
    };
    let size = FrameSize::native(1920, 1080);
    let mut best = [f64::MAX; 2];
    for (slot, wetness) in [0.0, 1.0].into_iter().enumerate() {
        let mut scene = matte_under_roof(wetness, None);
        scene.camera = head_camera(1920, 1080);
        for _ in 0..12 {
            let json = renderer.dump_timings_json(&scene, size).expect("timings");
            if let Some(ms) = pass_gpu_time_ms(&json, "scene") {
                best[slot] = best[slot].min(ms);
            }
        }
    }
    eprintln!(
        "scene: {:.3} ms at wetness 0, {:.3} ms at wetness 1 (best of 12, 1080p)",
        best[0], best[1]
    );
}

// --- Porosity ------------------------------------------------------------------------------------

/// A 30 m wall quad facing +Z, four metres in front of a camera at the origin, whose every vertex
/// carries `porosity` and the default material. The rain has no streaks and no occlusion, so only
/// porosity and the wall's facing (0.5) decide how far it darkens.
fn porous_wall(
    vertex_porosity: f32,
    instance_porosity: f32,
    authored: bool,
    wetness: f32,
) -> RenderScene {
    let mut scene = RenderScene::empty((PX, PX));
    scene.camera = origin_camera(PX, PX);
    scene.ambient = [0.5; 3];
    scene.post = Some(exact_post());
    let mut mesh = triangle(0xA11C ^ u64::from(vertex_porosity.to_bits()));
    mesh.positions = vec![
        -15.0, -15.0, 0.0, 15.0, -15.0, 0.0, 15.0, 15.0, 0.0, -15.0, 15.0, 0.0,
    ];
    mesh.normals = [0.0, 0.0, 1.0].repeat(4);
    mesh.colors = vec![1.0; 16];
    mesh.materials = [0.8, 0.0, 0.0].repeat(4);
    mesh.porosity = vec![vertex_porosity; 4];
    mesh.uvs = vec![0.0; 8];
    mesh.tangents = [1.0, 0.0, 0.0, 1.0].repeat(4);
    mesh.weights = [1.0, 0.0, 0.0, 0.0].repeat(4);
    mesh.bone_indices = vec![0; 16];
    mesh.indices = vec![0, 1, 2, 0, 2, 3];
    mesh.authored_material = authored;
    scene.mesh_assets.push(Arc::new(mesh));
    let mut wall = slab(Vec3::new(0.0, 0.0, -4.0), 1.0, [0.5; 3]);
    wall.shape = MeshShape::Asset(0);
    wall.material = [0.8, 0.0, 0.0];
    wall.use_vertex_material = authored;
    wall.porosity = instance_porosity;
    scene.meshes.push(wall);
    scene.rain = Some(RainData {
        occlusion: false,
        ..wet_rain(wetness)
    });
    scene
}

/// The wet-to-dry albedo ratio at the wall's centre.
fn wall_ratio(renderer: &Renderer, vertex: f32, instance: f32, authored: bool) -> f32 {
    let dry = renderer
        .render(&porous_wall(vertex, instance, authored, 0.0), size())
        .expect("dry");
    let wet = renderer
        .render(&porous_wall(vertex, instance, authored, 1.0), size())
        .expect("wet");
    linear(centre(&wet)) / linear(centre(&dry))
}

/// A vertex that authors porosity 0 keeps its albedo in the rain, 1 darkens more than 0.5, and the
/// selected value is the vertex's, whatever the instance lane holds. Skips with no GPU.
#[test]
fn needs_gpu_vertex_porosity_sets_how_far_rain_darkens() {
    let Some(renderer) = skip_or_renderer("vertex_porosity_sets_how_far_rain_darkens") else {
        return;
    };
    let dense = wall_ratio(&renderer, 0.0, 0.5, true);
    let half = wall_ratio(&renderer, 0.5, 0.5, true);
    let full = wall_ratio(&renderer, 1.0, 0.5, true);
    assert!(
        (dense - 1.0).abs() < 0.01,
        "porosity 0 stays dry-coloured, got {dense}"
    );
    assert!(
        (half - 0.9).abs() < 0.03,
        "porosity 0.5 darkens to 0.9, got {half}"
    );
    assert!(
        full < half - 0.05,
        "porosity 1 is darker than 0.5: {full} vs {half}"
    );

    let wet = |vertex: f32| {
        renderer
            .render(&porous_wall(vertex, 0.5, true, 1.0), size())
            .expect("wet")
    };
    let dry = |vertex: f32| {
        renderer
            .render(&porous_wall(vertex, 0.5, true, 0.0), size())
            .expect("dry")
    };
    assert_ne!(wet(0.0).rgba, wet(1.0).rgba, "differs under rain");
    assert_eq!(dry(0.0).rgba, dry(1.0).rgba, "equal with no wetness");
}

/// The instance lane governs geometry that authors no material, and is inert where the vertices
/// author one. Skips with no GPU.
#[test]
fn needs_gpu_instance_porosity_is_inert_on_authored_geometry() {
    let Some(renderer) = skip_or_renderer("instance_porosity_is_inert_on_authored_geometry") else {
        return;
    };
    let plain = wall_ratio(&renderer, 0.5, 1.0, false);
    let plain_default = wall_ratio(&renderer, 0.5, 0.5, false);
    assert!(
        plain < plain_default - 0.05,
        "the instance lane darkens: {plain} vs {plain_default}"
    );
    let a = wall_ratio(&renderer, 0.5, 1.0, true);
    let b = wall_ratio(&renderer, 0.5, 0.0, true);
    assert_eq!(a, b, "authored geometry ignores the instance lane");
}

/// Porosity 0 keeps the albedo but the roughness still drops: a rough plane's highlight peaks higher
/// wet than dry at porosity 0. Skips with no GPU.
#[test]
fn needs_gpu_porosity_zero_still_glosses() {
    let Some(renderer) = skip_or_renderer("porosity_zero_still_glosses") else {
        return;
    };
    let (w, h) = (64u32, 64u32);
    let frame = FrameSize::native(w, h);
    let peak = |wetness: f32| {
        let mut scene = RenderScene::empty((w, h));
        scene.post = Some(exact_post());
        scene.ambient = [0.0; 3];
        let eye = Vec3::new(0.0, 3.0, -5.196);
        scene.camera = crate::scene::RenderCamera {
            view: Mat4::look_at_rh(eye, Vec3::ZERO, Vec3::Y),
            projection: Mat4::perspective_rh(40.0_f32.to_radians(), 1.0, 0.1, 100.0),
            position: eye,
        };
        let mut floor = ground(20.0);
        floor.color = [1.0; 4];
        floor.material = [0.7, 0.0, 0.0];
        floor.porosity = 0.0;
        scene.meshes = vec![floor];
        scene.sun = Some(test_sun(
            Vec3::new(0.0, -0.5, -0.866),
            Vec3::splat(0.4),
            0.002,
        ));
        scene.rain = Some(RainData {
            occlusion: false,
            ..wet_rain(wetness)
        });
        let img = renderer.render(&scene, frame).expect("render");
        (0..h)
            .flat_map(|y| (0..w).map(move |x| (x, y)))
            .map(|(x, y)| luma(&img, x, y))
            .max()
            .unwrap_or(0)
    };
    assert!(
        peak(1.0) > peak(0.0),
        "the film still glosses at porosity 0"
    );
}

// --- Ripples -------------------------------------------------------------------------------------

/// A wet rain (no streaks, no sky shafts) at `ripples` and game time `time_s`.
fn rippling(ripples: f32, time_s: f32) -> RainData {
    RainData {
        ripples,
        time_s,
        ..wet_rain(1.0)
    }
}

/// The wet matte floor, rippling, with an optional roof.
fn rippled_floor(ripples: f32, time_s: f32, roof: Option<MeshInstance>) -> RenderScene {
    let mut scene = matte_under_roof(1.0, roof);
    scene.rain = Some(rippling(ripples, time_s));
    // A point light low in front of the camera, so the lowered roughness has a highlight to bend.
    scene.lights.push(crate::scene::PointLightData {
        position: Vec3::new(0.0, 0.0, -5.0),
        color: Vec3::ONE,
        intensity: 2.0,
        range: 40.0,
        dynamic: false,
        shadow: None,
        scatter: 0.0,
    });
    scene
}

/// The half roof of the wetness tests: it covers every x left of the middle.
fn half_roof() -> Option<MeshInstance> {
    Some(roof(
        Vec3::new(-30.0, 3.0, 0.0),
        Vec3::new(60.0, 0.2, 60.0),
        true,
    ))
}

/// Ripples change a wet floor with game time, hold at a held time and are off at `ripples: 0`,
/// whatever the time. Skips with no GPU.
#[test]
fn needs_gpu_ripples_animate_the_wet_floor_with_game_time() {
    let Some(renderer) = skip_or_renderer("ripples_animate_the_wet_floor_with_game_time") else {
        return;
    };
    let at = |ripples: f32, time_s: f32| {
        renderer
            .render(&rippled_floor(ripples, time_s, None), size())
            .expect("render")
    };
    let (first, held, later) = (at(1.0, 0.0), at(1.0, 0.2), at(1.0, 0.2));
    assert!(held.rgba == later.rgba, "one tick, one picture");
    assert!(near_floor(&first, &held, 0..PX), "the rings moved");
    assert!(
        at(0.0, 0.0).rgba == at(0.0, 0.2).rgba,
        "ripples: 0 does not read the clock"
    );
    assert!(
        near_floor(&at(0.0, 0.2), &held, 0..PX),
        "ripples change the floor"
    );
}

/// Wet floor under a roof stays still at any `ripples`, byte for byte, while the open half ripples.
/// Skips with no GPU.
#[test]
fn needs_gpu_covered_ground_does_not_ripple() {
    let Some(renderer) = skip_or_renderer("covered_ground_does_not_ripple") else {
        return;
    };
    let off = renderer
        .render(&rippled_floor(0.0, 0.2, half_roof()), size())
        .expect("off");
    let on = renderer
        .render(&rippled_floor(2.0, 0.2, half_roof()), size())
        .expect("on");
    let mid = PX / 2;
    assert!(
        !near_floor(&off, &on, 0..mid - 4),
        "the roofed floor stays still"
    );
    assert!(near_floor(&off, &on, mid + 4..PX), "the open floor ripples");
}

/// A `coat` decal over the wet floor: a lowered-roughness white coat so its lobe shows the ripples.
fn coat_scene(
    ripples: f32,
    time_s: f32,
    decal_ripples: f32,
    blend: crate::scene::DecalBlend,
    x: f32,
    roof: Option<MeshInstance>,
) -> RenderScene {
    let mut scene = rippled_floor(ripples, time_s, roof);
    let box_to_world = Mat4::from_translation(Vec3::new(x, -1.0, -4.0))
        * Mat4::from_scale(Vec3::new(3.0, 1.0, 4.0));
    scene.decals.push(crate::scene::DecalInstance {
        box_to_world,
        world_to_box: box_to_world.inverse(),
        blend,
        color: [1.0, 1.0, 1.0, 1.0],
        opacity: 1.0,
        roughness: Some(0.1),
        metallic: 0.0,
        ripples: decal_ripples,
        uv_rect: [0.0, 0.0, 1.0, 1.0],
        angle_fade: [60.0, 80.0],
        layer: 0,
        maps: crate::scene::DecalMaps::default(),
        receivers: crate::scene::DecalReceivers::Exclude(Vec::new()),
    });
    scene
}

/// An exposed coat decal ripples with game time; `Decal.ripples: 0` is `Rain.ripples: 0` byte for byte;
/// an `alpha` decal never ripples. The floor's own ripples are off (`Rain.ripples` scales both, so the
/// compare uses the decal weight alone through `Decal.ripples`). Skips with no GPU.
#[test]
fn needs_gpu_an_exposed_coat_decal_ripples_and_an_alpha_decal_does_not() {
    let Some(renderer) =
        skip_or_renderer("an_exposed_coat_decal_ripples_and_an_alpha_decal_does_not")
    else {
        return;
    };
    use crate::scene::DecalBlend::{Alpha, Coat};
    let render = |scene: RenderScene| renderer.render(&scene, size()).expect("render");
    let coat = |ripples, time_s, decal| render(coat_scene(ripples, time_s, decal, Coat, 2.0, None));
    let (a, b) = (coat(1.0, 0.0, 1.0), coat(1.0, 0.2, 1.0));
    assert!(near_floor(&a, &b, 0..PX), "the coat's rings moved");
    assert!(b.rgba == coat(1.0, 0.2, 1.0).rgba, "one tick, one picture");
    // The floor still ripples at `Rain.ripples: 1`, so compare a still decal against a still rain
    // only where the floor's own ripples cannot reach: with the decal off, the two frames differ by the
    // floor alone, and the coat's contribution is what the first comparison isolates.
    let still_decal = coat(1.0, 0.2, 0.0);
    let still_rain = coat(0.0, 0.2, 1.0);
    let still_both = coat(0.0, 0.2, 0.0);
    assert!(
        still_rain.rgba == still_both.rgba,
        "Rain.ripples 0 is still water on the decal"
    );
    assert!(
        still_decal.rgba != b.rgba,
        "Decal.ripples scales the coat's rings"
    );
    let alpha = |decal| render(coat_scene(1.0, 0.2, decal, Alpha, 2.0, None));
    assert!(
        alpha(0.0).rgba == alpha(2.0).rgba,
        "an alpha decal reads no ripple weight"
    );
}

/// A coat decal under a roof is byte-equal at `ripples` 0 and 2, and a frame with a coat decal and no
/// rain does not depend on `Decal.ripples`. Skips with no GPU.
#[test]
fn needs_gpu_a_covered_coat_decal_does_not_ripple() {
    let Some(renderer) = skip_or_renderer("a_covered_coat_decal_does_not_ripple") else {
        return;
    };
    use crate::scene::DecalBlend::Coat;
    let render = |scene: RenderScene| renderer.render(&scene, size()).expect("render");
    let covered = |ripples| render(coat_scene(ripples, 0.2, 1.0, Coat, -2.0, half_roof()));
    let (off, on) = (covered(0.0), covered(2.0));
    assert!(
        !near_floor(&off, &on, 0..PX / 2 - 4),
        "the covered coat is still"
    );
    assert!(
        near_floor(&off, &on, PX / 2 + 4..PX),
        "the open floor ripples"
    );
    let dry = |decal| {
        let mut scene = coat_scene(1.0, 0.2, decal, Coat, 2.0, None);
        scene.rain = None;
        render(scene)
    };
    assert!(dry(0.0).rgba == dry(2.0).rgba, "no rain, no ripples");
}

// --- Splashes ------------------------------------------------------------------------------------

/// A rain with no streaks and no wetness: only splashes can change a frame.
fn splashing(rate: f32, time_s: f32) -> RainData {
    RainData {
        per_tile: 0,
        splash_rate: rate,
        time_s,
        ..rain(0.0)
    }
}

/// `meshes` seen from `camera` under a splashing rain. The plain slab floor writes no map, so every
/// surface here is a caster.
fn splash_scene(
    rain: RainData,
    camera: crate::scene::RenderCamera,
    meshes: Vec<MeshInstance>,
) -> RenderScene {
    let mut scene = RenderScene::empty((PX, PX));
    scene.camera = camera;
    scene.ambient = [0.3; 3];
    scene.post = Some(exact_post());
    scene.meshes = meshes;
    scene.rain = Some(rain);
    scene
}

/// A casting floor whose top is y = 0, 30 m across, in front of the head camera.
fn splash_floor() -> MeshInstance {
    roof(
        Vec3::new(0.0, -0.05, -12.0),
        Vec3::new(30.0, 0.1, 30.0),
        true,
    )
}

fn from_above() -> crate::scene::RenderCamera {
    let eye = Vec3::new(0.0, 3.0, 0.0);
    crate::scene::RenderCamera {
        view: Mat4::look_at_rh(eye, Vec3::new(0.0, 0.0, -7.0), Vec3::Y),
        position: eye,
        ..origin_camera(PX, PX)
    }
}

/// Splashes change the frame, hold at a held game time and move with it. Skips with no GPU.
#[test]
fn needs_gpu_splashes_land_on_a_casting_floor_and_follow_game_time() {
    let Some(renderer) = skip_or_renderer("splashes_land_on_a_casting_floor_and_follow_game_time")
    else {
        return;
    };
    let at = |rate: f32, time_s: f32| {
        renderer
            .render(
                &splash_scene(
                    splashing(rate, time_s),
                    head_camera(PX, PX),
                    vec![splash_floor()],
                ),
                size(),
            )
            .expect("render")
    };
    let (off, on, again, later) = (at(0.0, 0.0), at(1.0, 0.0), at(1.0, 0.0), at(1.0, 0.08));
    assert_ne!(off.rgba, on.rgba, "splashes are drawn");
    assert!(on.rgba == again.rgba, "one tick, one picture");
    assert_ne!(on.rgba, later.rgba, "the splashes moved with game time");
}

/// Floor under a roof gets no splash, byte for byte; the open half of the frame does, and so does the
/// roof's top. Skips with no GPU.
#[test]
fn needs_gpu_splashes_stay_off_covered_floor_and_land_on_the_roof() {
    let Some(renderer) = skip_or_renderer("splashes_stay_off_covered_floor_and_land_on_the_roof")
    else {
        return;
    };
    let low_roof = || {
        roof(
            Vec3::new(-10.0, 1.25, -10.0),
            Vec3::new(20.0, 0.1, 20.0),
            true,
        )
    };
    let under = |rate: f32| {
        renderer
            .render(
                &splash_scene(
                    splashing(rate, 0.0),
                    head_camera(PX, PX),
                    vec![splash_floor(), low_roof()],
                ),
                size(),
            )
            .expect("under")
    };
    let (off, on) = (under(0.0), under(2.0));
    let mid = PX / 2;
    assert!(
        !column_differs(&off, &on, 0..mid - 4),
        "the roofed floor gets no splash"
    );
    assert!(
        column_differs(&off, &on, mid + 4..PX),
        "the open floor splashes"
    );
    let top = |rate: f32| {
        renderer
            .render(
                &splash_scene(
                    splashing(rate, 0.0),
                    from_above(),
                    vec![roof(
                        Vec3::new(0.0, 1.25, -7.0),
                        Vec3::new(12.0, 0.1, 10.0),
                        true,
                    )],
                ),
                size(),
            )
            .expect("top")
    };
    assert_ne!(top(0.0).rgba, top(2.0).rgba, "the roof's top splashes");
}

/// A 60 degree caster takes no splash while a flat one in the same place does, and `occlusion: false`
/// leaves no map to land on. Skips with no GPU.
#[test]
fn needs_gpu_steep_tops_and_a_missing_map_take_no_splash() {
    let Some(renderer) = skip_or_renderer("steep_tops_and_a_missing_map_take_no_splash") else {
        return;
    };
    let plate = |tilt: f32| {
        let mut m = roof(Vec3::ZERO, Vec3::new(8.0, 0.2, 8.0), true);
        m.model = Mat4::from_translation(Vec3::new(0.0, 0.5, -6.0))
            * Mat4::from_rotation_x(tilt.to_radians())
            * Mat4::from_scale(Vec3::new(8.0, 0.2, 8.0));
        m
    };
    let render = |rain: RainData, mesh: MeshInstance| {
        renderer
            .render(&splash_scene(rain, head_camera(PX, PX), vec![mesh]), size())
            .expect("render")
    };
    let (off, on) = (
        render(splashing(0.0, 0.0), plate(60.0)),
        render(splashing(2.0, 0.0), plate(60.0)),
    );
    assert!(off.rgba == on.rgba, "a 60 degree ramp takes no splash");
    let (flat_off, flat_on) = (
        render(splashing(0.0, 0.0), plate(0.0)),
        render(splashing(2.0, 0.0), plate(0.0)),
    );
    assert_ne!(flat_off.rgba, flat_on.rgba, "a flat plate there does");
    let blind = |rate| {
        render(
            RainData {
                occlusion: false,
                ..splashing(rate, 0.0)
            },
            plate(0.0),
        )
    };
    assert!(
        blind(0.0).rgba == blind(2.0).rgba,
        "no occlusion map, no splash"
    );
}
