//! Integration tests for `ScreenEffect`, driven against the built binary: the animated stages are
//! integer hashes of the pixel, the game tick and the seed, so two processes agree.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

fn run_ludion(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_ludion"))
        .args(args)
        .output()
        .expect("failed to run the ludion binary")
}

fn out_path(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(name)
}

/// A lit slab under a UI rect and text below a region effect (order 10), a clean text above it
/// (order 20), and a fullscreen `scan` under all of them (order 0).
const SCENE: &str = "format: 1
name: screen effect
entities:
  - name: camera
    components:
      Transform:
        translation:
          - 0.0
          - 0.0
          - 10.0
      Camera:
        projection:
          type: perspective
          fov_y_deg: 50.0
  - name: slab
    components:
      Transform:
        scale:
          - 40.0
          - 40.0
          - 1.0
      Mesh:
        shape: cube
        color:
          - 1.0
          - 1.0
          - 1.0
          - 1.0
        emissive: 0.4
  - name: panel
    components:
      UiRect:
        anchor: top_right
        offset:
          - -8.0
          - 8.0
        size:
          - 64.0
          - 40.0
        color:
          - 0.2
          - 0.4
          - 0.9
          - 1.0
        order: 5
  - name: panel-text
    components:
      UiText:
        anchor: top_right
        offset:
          - -12.0
          - 12.0
        text: UNDER
        size: 14.0
        order: 5
  - name: clean-text
    components:
      UiText:
        anchor: top_right
        offset:
          - -12.0
          - 36.0
        text: OVER
        size: 14.0
        order: 20
  - name: radar-fx
    components:
      ScreenEffect:
        target:
          region:
            anchor: top_right
            offset:
              - -8.0
              - 8.0
            size:
              - 80.0
              - 60.0
        order: 10
        preset: crt
  - name: screen-fx
    components:
      ScreenEffect:
        order: 0
        preset: scan
";

fn render(scene: &Path, frames: &str, out: &Path) -> Output {
    run_ludion(&[
        "render",
        "--scene",
        scene.to_str().unwrap(),
        "--frames",
        frames,
        "--out",
        out.to_str().unwrap(),
        "--width",
        "160",
        "--height",
        "90",
    ])
}

fn no_adapter(output: &Output) -> bool {
    ludion_core::require_gpu::skip_no_adapter(
        String::from_utf8_lossy(&output.stderr).contains("error[L2003]"),
    )
}

/// Renders `frames` of `scene` to `name` and returns the PNG's bytes, or `None` on a box with no
/// GPU adapter.
fn bytes_of(scene: &Path, frames: &str, name: &str) -> Option<Vec<u8>> {
    let out = out_path(name);
    let run = render(scene, frames, &out);
    if !run.status.success() {
        if no_adapter(&run) {
            eprintln!("SKIPPED screen_effect::{name}: no GPU adapter");
            return None;
        }
        panic!("{}", String::from_utf8_lossy(&run.stderr));
    }
    Some(std::fs::read(&out).unwrap())
}

/// **The effects animate per tick and reproduce across processes.** Two separate OS processes write
/// byte-identical `--frames 0,1,2` strips, and tick 0 differs from tick 1.
#[test]
fn needs_gpu_effects_animate_per_tick_and_reproduce_across_processes() {
    let scene = out_path("screen_effect_scene.yaml");
    std::fs::write(&scene, SCENE).unwrap();
    let Some(first) = bytes_of(&scene, "0,1,2", "screen_effect_strip_1.png") else {
        return;
    };
    let second = bytes_of(&scene, "0,1,2", "screen_effect_strip_2.png").unwrap();
    assert_eq!(&first[1..4], b"PNG", "output is a PNG");
    assert_eq!(
        first, second,
        "two separate OS processes must write byte-identical screen-effect strips",
    );
    let tick0 = bytes_of(&scene, "0", "screen_effect_0.png").unwrap();
    let tick1 = bytes_of(&scene, "1", "screen_effect_1.png").unwrap();
    assert_ne!(
        tick0, tick1,
        "the animated stages must differ between ticks 0 and 1"
    );
}
