//! GPU dispatch tests: screenshot and timings over the real render path, graceful-skipping on an
//! adapterless machine (an `L2003` in the diagnostic), so CI without a GPU stays green.

use ludion_engine::{Engine, GiRebakeMode, Transform, dispatch};
use ludion_protocol::command::{RenderDebugParams, RenderDebugView, RenderScreenshotParams};
use ludion_protocol::{Command, ResponseBody};

/// A small scene with visible content, so a screenshot is non-trivial.
const SCENE: &str = "\
format: 1
entities:
  - components:
      Transform:
        translation:
          - 0.0
          - 0.0
          - 5.0
      Camera:
        near: 0.1
        far: 100.0
  - components:
      Mesh: {}
";

fn engine() -> Engine {
    let mut engine = Engine::new();
    engine.load_inline_scene(SCENE).unwrap();
    engine
}

/// Whether a diagnostic is the "no GPU adapter" skip signal.
fn is_no_adapter(diag: &ludion_core::diagnostics::Diagnostic) -> bool {
    ludion_core::require_gpu::is_no_adapter(std::slice::from_ref(diag))
}

/// **Screenshot determinism.** Two `render.screenshot` calls at the same frame are byte-identical.
#[test]
fn needs_gpu_two_screenshots_at_the_same_frame_are_identical() {
    let mut engine = engine();
    let params = || {
        Command::RenderScreenshot(RenderScreenshotParams {
            width: Some(160),
            height: Some(90),
            ..RenderScreenshotParams::default()
        })
    };

    let first = match dispatch(&mut engine, params()) {
        Ok(dispatched) => dispatched.body,
        Err(diag) if is_no_adapter(&diag) => {
            eprintln!("[screenshot] SKIPPED: no GPU adapter available");
            return;
        }
        Err(diag) => panic!("screenshot failed: {diag}"),
    };
    let second = dispatch(&mut engine, params()).unwrap().body;

    match (first, second) {
        (
            ResponseBody::Screenshot {
                width: w1,
                height: h1,
                format: f1,
                data: d1,
                stale: s1,
            },
            ResponseBody::Screenshot {
                data: d2,
                format: f2,
                ..
            },
        ) => {
            assert_eq!((w1, h1), (160, 90));
            assert_eq!(f1, "png");
            assert_eq!(f2, "png");
            assert!(!s1, "a default (Auto) engine never holds its GI");
            assert_eq!(
                d1, d2,
                "screenshots at the same frame must be byte-identical"
            );
            assert!(!d1.is_empty());
        }
        other => panic!("expected two Screenshot bodies, got {other:?}"),
    }
}

/// `render.dump_timings` returns well-shaped JSON (one entry per pass) over the real path.
#[test]
fn needs_gpu_dump_timings_over_dispatch() {
    let mut engine = engine();
    let body = match dispatch(&mut engine, Command::RenderDumpTimings) {
        Ok(dispatched) => dispatched.body,
        Err(diag) if is_no_adapter(&diag) => {
            eprintln!("[timings] SKIPPED: no GPU adapter available");
            return;
        }
        Err(diag) => panic!("dump_timings failed: {diag}"),
    };
    match body {
        ResponseBody::TimingsDump { timings } => {
            let passes = timings["passes"].as_array().expect("passes array");
            assert_eq!(passes.len(), 16, "one entry per scheduled pass");
            assert!(
                passes.iter().any(|p| p["name"] == "cull"),
                "a world-mesh scene schedules the cull pass",
            );
        }
        other => panic!("{other:?}"),
    }
}

/// A lit scene that actually voxelizes and bakes: a cube on a plane under a `GiVolume`, with the
/// camera looking at it. The lever's whole subject is the two GI caches, and neither exists for a
/// scene that authors no `GiVolume`.
const GI_SCENE: &str = "\
format: 1
entities:
  - components:
      Transform:
        translation:
          - 0.0
          - 3.0
          - 9.0
      Camera:
        near: 0.1
        far: 100.0
  - components:
      Transform:
        translation:
          - 0.0
          - -1.0
          - 0.0
        scale:
          - 12.0
          - 1.0
          - 12.0
      Mesh:
        shape: plane
  - components:
      Transform: {}
      Mesh:
        shape: cube
  - components:
      GiVolume:
        cell_size: 0.5
        probe_spacing: 2.0
";

/// Moves the second-listed cube (the third entity) to `x`, the way a game system would.
fn move_gi_cube(engine: &mut Engine, x: f32) {
    let cube = engine
        .world()
        .entities()
        .nth(2)
        .expect("the fixture's cube");
    engine
        .world_mut()
        .insert(
            cube,
            Transform {
                translation: [x, 0.0, 0.0],
                ..Transform::default()
            },
        )
        .expect("the cube is alive");
}

/// **The rebake lever end to end, through a real render.** The engine's two halves — the CPU
/// voxelization it extracts through and the GPU bake its renderer holds — are wired to one lever and
/// report through one `gi_stale()`. Auto is the untaken branch, held here beside the taken one so the
/// difference is a decision rather than an accident.
#[test]
fn needs_gpu_holding_the_gi_baseline_reports_stale_and_a_rebake_clears_it() {
    let mut engine = Engine::new();
    engine.load_inline_scene(GI_SCENE).unwrap();

    let baseline = match engine.render_png(160, 90) {
        Ok(png) => png,
        Err(diag) if is_no_adapter(&diag) => {
            eprintln!("[gi rebake] SKIPPED: no GPU adapter available");
            return;
        }
        Err(diag) => panic!("render failed: {diag}"),
    };
    assert!(!engine.gi_stale(), "a fresh bake shows the world itself");

    // The lever is inert over a world that has not moved: holding produces the very same bytes.
    engine.set_gi_rebake(GiRebakeMode::Hold);
    assert_eq!(
        engine.render_png(160, 90).unwrap(),
        baseline,
        "a hold over an unchanged world must change no pixel",
    );
    assert!(!engine.gi_stale());

    // Move an unmarked mesh: both caches miss, both serve what they hold, and the engine says so.
    move_gi_cube(&mut engine, 3.0);
    let held = engine.render_png(160, 90).unwrap();
    assert!(
        engine.gi_stale(),
        "a held frame over a moved world is stale, and must report it",
    );
    assert_ne!(held, baseline, "the moved cube itself still renders");

    // One rebake: the caches refresh to this world, and it becomes the new baseline.
    engine.gi_rebake_now();
    let rebaked = engine.render_png(160, 90).unwrap();
    assert!(
        !engine.gi_stale(),
        "the rebaked frame shows the world itself"
    );
    assert_eq!(
        engine.gi_rebake_mode(),
        GiRebakeMode::Hold,
        "a rebake is an event, not a mode change",
    );
    assert_eq!(
        engine.render_png(160, 90).unwrap(),
        rebaked,
        "and holding over that same world is a plain cache hit",
    );
    assert!(!engine.gi_stale());

    // Auto is the untaken branch: the same motion rebuilds and is never stale.
    engine.set_gi_rebake(GiRebakeMode::Auto);
    move_gi_cube(&mut engine, 0.0);
    assert_eq!(
        engine.render_png(160, 90).unwrap(),
        baseline,
        "Auto rebuilds, so the world back at its start renders its start's bytes",
    );
    assert!(!engine.gi_stale());
}

/// `render.screenshot` checks its extent against the `render.size` bounds before rendering anything
/// (`L3025`), and a screenshot whose render extent the scale pushes past the pinned texture limit is
/// `L2012` — neither reaches the GPU.
#[test]
fn screenshot_refuses_an_extent_no_render_can_honor() {
    let mut engine = engine();
    let shot = |width, height| {
        Command::RenderScreenshot(RenderScreenshotParams {
            width: Some(width),
            height: Some(height),
            ..RenderScreenshotParams::default()
        })
    };
    for (width, height, pointer) in [(0, 90, "/params/width"), (160, 0, "/params/height")] {
        let diag = dispatch(&mut engine, shot(width, height)).expect_err("zero axis is refused");
        assert_eq!(diag.code.as_str(), "L3025");
        assert_eq!(diag.path.as_deref(), Some(pointer));
    }
    let diag = dispatch(&mut engine, shot(7681, 90)).expect_err("past the ceiling is refused");
    assert_eq!(diag.code.as_str(), "L3025");

    engine.set_render_scale(ludion_engine::RenderScale::new(2.0).unwrap());
    let diag =
        dispatch(&mut engine, shot(7680, 4320)).expect_err("8K at scale 2 is past the limit");
    assert_eq!(diag.code.as_str(), "L2012");
}

/// `render.size` refuses an extent that fits alone but not at the host's render scale (`L2012`),
/// and a refusal leaves the extent exactly as it was.
#[test]
fn render_size_refuses_an_extent_the_scale_pushes_past_the_limit() {
    use ludion_protocol::command::RenderSizeParams;
    let mut engine = engine();
    let size = |width, height| {
        Command::RenderSize(RenderSizeParams {
            width: Some(width),
            height: Some(height),
        })
    };
    let before = engine.render_size();

    engine.set_render_scale(ludion_engine::RenderScale::new(2.0).unwrap());
    let diag = dispatch(&mut engine, size(7680, 4320)).expect_err("8K at scale 2 is refused");
    assert_eq!(diag.code.as_str(), "L2012");
    assert_eq!(
        engine.render_size(),
        before,
        "a refusal leaves the extent alone"
    );

    dispatch(&mut engine, size(3840, 2160)).expect("4K at scale 2 fits");
    assert_eq!(engine.render_size(), (3840, 2160));
}

/// A host's boot check names the same `L2012` for a pair no render could honor.
#[test]
fn check_render_extent_names_the_scaled_pair() {
    let mut engine = engine();
    engine.set_render_size(7680, 4320);
    engine.check_render_extent().expect("8K at scale 1 fits");
    engine.set_render_scale(ludion_engine::RenderScale::new(2.0).unwrap());
    let diag = engine.check_render_extent().unwrap_err();
    assert_eq!(diag.code.as_str(), "L2012");
    assert!(diag.message.contains("15360x8640"), "{}", diag.message);
}

/// **A live host draws the SSR view `render.debug` sets**: the frame differs from the normal one,
/// and is the very picture the offline `--debug-view` renders at the same size.
#[test]
fn needs_gpu_render_debug_view_draws_the_ssr_view_the_cli_draws() {
    let scene = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../examples/gallery/mirror-floor.yaml");
    let (w, h) = (160, 90);
    let mut engine = Engine::new();
    engine
        .load_inline_scene(&std::fs::read_to_string(&scene).unwrap())
        .unwrap();
    let normal = match engine.render_png(w, h) {
        Ok(png) => png,
        Err(diag) if is_no_adapter(&diag) => {
            eprintln!("[render.debug view] SKIPPED: no GPU adapter available");
            return;
        }
        Err(diag) => panic!("render failed: {diag}"),
    };
    engine
        .execute(Command::RenderDebug(RenderDebugParams {
            categories: None,
            view: Some(Some(RenderDebugView::SsrConfidence)),
        }))
        .unwrap();
    let live = engine.render_png(w, h).unwrap();
    assert_ne!(live, normal, "the view replaces the composite");

    let offline = ludion_engine::render_scene_with_registry(
        &ludion_engine::builtin_registry(),
        &scene,
        ludion_engine::FrameSize::native(w, h),
        &ludion_engine::QualitySetting::full(),
        false,
        &[],
        0,
        &ludion_engine::DebugCategories::None,
        Some(ludion_engine::DebugView::SsrConfidence),
        ludion_engine::AntiAliasing::Off,
    )
    .unwrap();
    assert_eq!(
        live,
        offline.image.encode_png().unwrap(),
        "the same bytes as --debug-view"
    );
}
