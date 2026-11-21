//! Integration tests for animated grain (`PostProcess.grain_motion`), driven against the built
//! binary: the pattern is an integer hash of the cell and the game tick, so two processes agree.

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

/// One lit slab filling the frame under a grain of the given motion.
fn grain_scene(motion: &str) -> String {
    format!(
        "format: 1\nname: grain motion\nentities:\n  - name: environment\n    components:\n      \
         Environment:\n        ibl_intensity: 0.0\n      PostProcess:\n        grain: 0.05\n        \
         grain_motion: {motion}\n  - name: camera\n    components:\n      Transform:\n        \
         translation:\n          - 0.0\n          - 0.0\n          - 10.0\n      Camera:\n        \
         projection:\n          type: perspective\n          fov_y_deg: 50.0\n  - name: slab\n    \
         components:\n      Transform:\n        scale:\n          - 40.0\n          - 40.0\n          \
         - 1.0\n      Mesh:\n        shape: cube\n        color:\n          - 1.0\n          - 1.0\n          \
         - 1.0\n          - 1.0\n        emissive: 0.4\n"
    )
}

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
            eprintln!("SKIPPED grain_motion::{name}: no GPU adapter");
            return None;
        }
        panic!("{}", String::from_utf8_lossy(&run.stderr));
    }
    Some(std::fs::read(&out).unwrap())
}

/// **Animated grain re-rolls per tick and reproduces across processes.** Two separate OS processes
/// write byte-identical `--frames 0,1` strips, and tick 0 differs from tick 1.
#[test]
fn needs_gpu_animated_grain_changes_per_tick_and_reproduces_across_processes() {
    let scene = out_path("grain_motion_animated.yaml");
    std::fs::write(&scene, grain_scene("animated")).unwrap();
    let Some(first) = bytes_of(&scene, "0,1", "grain_motion_animated_strip_1.png") else {
        return;
    };
    let second = bytes_of(&scene, "0,1", "grain_motion_animated_strip_2.png").unwrap();
    assert_eq!(&first[1..4], b"PNG", "output is a PNG");
    assert_eq!(
        first, second,
        "two separate OS processes must write byte-identical animated-grain strips",
    );
    let tick0 = bytes_of(&scene, "0", "grain_motion_animated_0.png").unwrap();
    let tick1 = bytes_of(&scene, "1", "grain_motion_animated_1.png").unwrap();
    assert_ne!(
        tick0, tick1,
        "animated grain must differ between ticks 0 and 1"
    );
}

/// Static grain is one field: ticks 0 and 1 render the same frame.
#[test]
fn needs_gpu_static_grain_holds_across_ticks() {
    let scene = out_path("grain_motion_static.yaml");
    std::fs::write(&scene, grain_scene("static")).unwrap();
    let Some(tick0) = bytes_of(&scene, "0", "grain_motion_static_0.png") else {
        return;
    };
    let tick1 = bytes_of(&scene, "1", "grain_motion_static_1.png").unwrap();
    assert_eq!(tick0, tick1, "static grain must not move with the tick");
}
