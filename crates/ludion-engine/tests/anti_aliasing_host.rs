//! GPU tests for the engine's anti-aliasing precedence: the builder default reaches the renderer
//! through the golden harness, and the game's runtime switch changes the next frame without moving the
//! `WorldHash`.
//!
//! Each test skips gracefully (printing why) when no adapter is available.

use std::path::PathBuf;

use ludion_core::diagnostics::Diagnostic;
use ludion_core::time::FrameIndex;
use ludion_ecs::{SystemCtx, World};
use ludion_engine::{
    AntiAliasing, DebugCategories, Engine, EngineBuilder, FrameSize, QualitySetting, Settings,
    builtin_registry, render_scene_with_builder, render_scene_with_registry,
};

/// A bright, yaw-rotated cube on black: every silhouette edge is a staircase SMAA visibly moves.
const SCENE: &str = r#"format: 1
name: "Anti-aliasing host fixture"
entities:
  - name: camera
    components:
      Transform:
        translation:
          - 0.0
          - 0.0
          - 4.0
      Camera:
        projection:
          type: perspective
          fov_y_deg: 50.0
  - name: key-light
    components:
      Transform:
        translation:
          - 1.0
          - 3.0
          - 3.0
      PointLight:
        color:
          - 1.0
          - 1.0
          - 1.0
        intensity: 40.0
        range: 20.0
  - name: cube
    components:
      Transform:
        rotation:
          - 20.0
          - 33.0
          - 0.0
      Mesh:
        shape: cube
        color:
          - 1.0
          - 1.0
          - 1.0
          - 1.0
"#;

const SIZE: (u32, u32) = (160, 90);

/// A scratch project directory under the OS temp dir, removed on drop.
struct TempProject(PathBuf);

impl TempProject {
    fn new(tag: &str) -> Self {
        let dir = std::env::temp_dir().join(format!("ludion-aa-host-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("the temp project dir creates");
        TempProject(dir)
    }

    fn scene(&self) -> PathBuf {
        let path = self.0.join("scene.yaml");
        std::fs::write(&path, SCENE).expect("the scene fixture writes");
        path
    }
}

impl Drop for TempProject {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn no_adapter(errors: &[Diagnostic]) -> bool {
    ludion_core::require_gpu::is_no_adapter(errors)
}

/// The harness render of the fixture through `builder`, or `None` (printing why) with no GPU.
fn harness_png(test: &str, builder: EngineBuilder, scene: &std::path::Path) -> Option<Vec<u8>> {
    match render_scene_with_builder(
        builder,
        scene,
        Some(SIZE.0),
        Some(SIZE.1),
        &QualitySetting::default(),
        0,
        &DebugCategories::None,
    ) {
        Ok(outcome) => Some(outcome.png),
        Err(errors) if no_adapter(&errors) => {
            eprintln!("[{test}] SKIPPED: no GPU adapter available");
            None
        }
        Err(errors) => panic!("[{test}] render failed: {errors:?}"),
    }
}

#[test]
fn needs_gpu_the_harness_honours_the_builder_default() {
    let project = TempProject::new("harness");
    let scene = project.scene();
    let test = "harness";
    let Some(off) = harness_png(test, Engine::builder(), &scene) else {
        return;
    };
    let smaa_builder = harness_png(
        test,
        Engine::builder().anti_aliasing(AntiAliasing::Smaa),
        &scene,
    )
    .unwrap();

    let mut host = Engine::builder().load_scene_file(&scene).unwrap();
    host.set_anti_aliasing(AntiAliasing::Smaa);
    let smaa_host = host.render_png(SIZE.0, SIZE.1).unwrap();

    let bare = render_scene_with_registry(
        &builtin_registry(),
        &scene,
        FrameSize::native(SIZE.0, SIZE.1),
        &QualitySetting::default(),
        false,
        &[],
        0,
        &DebugCategories::None,
        None,
        ludion_engine::AntiAliasing::Off,
    )
    .unwrap();

    assert_eq!(
        smaa_builder, smaa_host,
        "the builder default is the host switch"
    );
    assert_ne!(smaa_builder, off, "SMAA must move edge pixels");
    assert_eq!(
        off,
        bare.image.encode_png().unwrap(),
        "an Off builder is the bare path"
    );
}

fn nothing(_world: &mut World, _ctx: &SystemCtx) {}

fn toggle(world: &mut World, ctx: &SystemCtx) {
    let mode = match ctx.frame {
        FrameIndex(1) => AntiAliasing::Smaa,
        FrameIndex(2) => AntiAliasing::Off,
        _ => return,
    };
    if let Some(settings) = world.resource_mut::<Settings>() {
        settings.request_anti_aliasing(mode);
    }
}

#[test]
fn needs_gpu_the_runtime_toggle_matches_a_twin_fixed_at_each_mode() {
    let project = TempProject::new("toggle");
    let scene = project.scene();
    let mut toggling = Engine::builder()
        .system("toggle", toggle)
        .load_scene_file(&scene)
        .unwrap();
    toggling.set_anti_aliasing(AntiAliasing::Off);
    let mut twin_off = Engine::builder()
        .system("toggle", nothing)
        .load_scene_file(&scene)
        .unwrap();
    let mut twin_smaa = Engine::builder()
        .anti_aliasing(AntiAliasing::Smaa)
        .system("toggle", nothing)
        .load_scene_file(&scene)
        .unwrap();

    let mut frames = Vec::new();
    for (tick, expected) in [AntiAliasing::Off, AntiAliasing::Smaa, AntiAliasing::Off]
        .into_iter()
        .enumerate()
    {
        toggling.advance_live();
        twin_off.advance_live();
        twin_smaa.advance_live();
        let png = match toggling.render_png(SIZE.0, SIZE.1) {
            Ok(png) => png,
            Err(error) if no_adapter(std::slice::from_ref(&error)) => {
                eprintln!("[toggle] SKIPPED: no GPU adapter available");
                return;
            }
            Err(error) => panic!("[toggle] render failed: {error:?}"),
        };
        let twin = match expected {
            AntiAliasing::Off => &mut twin_off,
            AntiAliasing::Smaa => &mut twin_smaa,
        };
        assert_eq!(toggling.anti_aliasing(), expected, "tick {tick}");
        assert_eq!(
            png,
            twin.render_png(SIZE.0, SIZE.1).unwrap(),
            "tick {tick}: the frame is the twin fixed at {expected:?}"
        );
        assert_eq!(
            toggling.world_hash().unwrap(),
            twin.world_hash().unwrap(),
            "tick {tick}: the switch never touches the hash"
        );
        frames.push(png);
    }
    assert_ne!(
        frames[0], frames[1],
        "the smaa frame differs from the off frame"
    );
    assert_eq!(frames[0], frames[2], "switching back is exact");
}
