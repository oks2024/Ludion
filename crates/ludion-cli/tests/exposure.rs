//! Integration tests for metered exposure (`PostProcess.exposure_mode: metered`), driven against the
//! built binary: the meter is a GPU float reduction, so the claim is per machine, not cross-platform.

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

/// A scene file's block-style sequence (flow style is not allowed in scene files).
fn block(indent: &str, values: &[f32]) -> String {
    values
        .iter()
        .map(|value| format!("\n{indent}- {value:?}"))
        .collect()
}

/// A bright-to-dark row of four emissive slabs under the given exposure mode.
fn gradient_scene(mode: &str) -> String {
    let mut text = format!(
        "format: 1\nname: metered gradient\nentities:\n  - name: environment\n    components:\n      \
         Environment:\n        ibl_intensity: 0.0\n      PostProcess:\n        exposure_mode: {mode}\n  \
         - name: camera\n    components:\n      Transform:\n        translation:{}\n      \
         Camera:\n        projection:\n          type: perspective\n          fov_y_deg: 50.0\n",
        block("          ", &[0.0, 0.0, 10.0]),
    );
    for (index, (x, emissive)) in [(-6.0, 6.0), (-2.0, 1.0), (2.0, 0.2), (6.0, 0.02)]
        .into_iter()
        .enumerate()
    {
        text.push_str(&format!(
            "  - name: slab-{index}\n    components:\n      Transform:\n        translation:{}\n        \
             scale:{}\n      Mesh:\n        shape: cube\n        color:{}\n        emissive: {emissive:?}\n",
            block("          ", &[x, 0.0, 0.0]),
            block("          ", &[4.0, 8.0, 1.0]),
            block("          ", &[1.0, 1.0, 1.0, 1.0]),
        ));
    }
    text
}

fn render(scene: &Path, out: &Path) -> Output {
    run_ludion(&[
        "render",
        "--scene",
        scene.to_str().unwrap(),
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

/// **Metering is deterministic and live.** Two separate OS processes render the same metered scene
/// to byte-identical PNGs (the histogram is integer atomics and one thread's fixed-order reduce), and
/// the image differs from the same scene under `exposure_mode: manual`. Per machine, not
/// cross-platform.
#[test]
fn needs_gpu_two_processes_render_the_same_metered_scene_identically() {
    let metered = out_path("exposure_metered_scene.yaml");
    let manual = out_path("exposure_manual_scene.yaml");
    std::fs::write(&metered, gradient_scene("metered")).unwrap();
    std::fs::write(&manual, gradient_scene("manual")).unwrap();
    let (first, second, plain) = (
        out_path("exposure_metered_1.png"),
        out_path("exposure_metered_2.png"),
        out_path("exposure_manual.png"),
    );

    let a = render(&metered, &first);
    if !a.status.success() {
        if no_adapter(&a) {
            eprintln!(
                "SKIPPED two_processes_render_the_same_metered_scene_identically: no GPU adapter"
            );
            return;
        }
        panic!("{}", String::from_utf8_lossy(&a.stderr));
    }
    for (scene, out) in [(&metered, &second), (&manual, &plain)] {
        let run = render(scene, out);
        assert!(
            run.status.success(),
            "{}",
            String::from_utf8_lossy(&run.stderr)
        );
    }

    let bytes = std::fs::read(&first).unwrap();
    assert_eq!(&bytes[1..4], b"PNG", "output is a PNG");
    assert_eq!(
        bytes,
        std::fs::read(&second).unwrap(),
        "two separate OS processes metering the same scene must write byte-identical PNGs",
    );
    assert_ne!(
        bytes,
        std::fs::read(&plain).unwrap(),
        "metered exposure must change the image against manual",
    );
}
