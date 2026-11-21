//! The bright-to-dark pan check for metered exposure (`PostProcess.exposure_mode: metered`), driven
//! against the built binary: across a camera pan from a bright wall to a dim one, the metered
//! frame's mean brightness stays far tighter than manual's, and the printed EV rises toward the dark
//! side. Per machine, not cross-platform: the meter is a GPU float reduction.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

const PAN_X: [f32; 5] = [-14.0, -7.0, 0.0, 7.0, 14.0];

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

/// A wall of emissive slabs, bright (about 8.0) on the left of the origin and dim (about 0.02) on the
/// right, under the given exposure mode, seen by a camera at entity 1.
fn wall_scene(mode: &str) -> String {
    let mut text = format!(
        "format: 1\nname: pan\nentities:\n  - name: environment\n    components:\n      \
         Environment:\n        ibl_intensity: 0.0\n      PostProcess:\n        exposure_mode: {mode}\n        \
         exposure_range:{}\n  - name: camera\n    components:\n      Transform:\n        \
         translation:{}\n      Camera:\n        projection:\n          type: perspective\n          \
         fov_y_deg: 50.0\n",
        block("          ", &[-6.0, 6.0]),
        block("          ", &[0.0, 0.0, 10.0]),
    );
    for index in 0..14 {
        let x = -26.0 + 4.0 * index as f32 + 2.0;
        let emissive = if x < 0.0 { 8.0 } else { 0.02 };
        text.push_str(&format!(
            "  - name: slab-{index}\n    components:\n      Transform:\n        translation:{}\n        \
             scale:{}\n      Mesh:\n        shape: cube\n        color:{}\n        emissive: {emissive:?}\n",
            block("          ", &[x, 0.0, 0.0]),
            block("          ", &[4.0, 12.0, 1.0]),
            block("          ", &[1.0, 1.0, 1.0, 1.0]),
        ));
    }
    text
}

/// Renders the scene with its camera at `x`; the run's stdout is the summary.
fn render_at(scene: &Path, x: f32, out: &Path) -> Output {
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
        "--set",
        &format!("/entities/1/components/Transform/translation=[{x:?},0.0,10.0]"),
    ])
}

fn no_adapter(output: &Output) -> bool {
    ludion_core::require_gpu::skip_no_adapter(
        String::from_utf8_lossy(&output.stderr).contains("error[L2003]"),
    )
}

/// The mean of the sRGB luma of every pixel.
fn mean_luma(path: &Path) -> f64 {
    let file = std::fs::File::open(path).expect("PNG exists");
    let mut reader = png::Decoder::new(std::io::BufReader::new(file))
        .read_info()
        .expect("valid PNG");
    let mut buf = vec![0u8; reader.output_buffer_size().unwrap()];
    let info = reader.next_frame(&mut buf).expect("frame decodes");
    let pixels = buf[..info.buffer_size()].chunks_exact(4);
    let count = pixels.len() as f64;
    pixels
        .map(|p| 0.2126 * f64::from(p[0]) + 0.7152 * f64::from(p[1]) + 0.0722 * f64::from(p[2]))
        .sum::<f64>()
        / count
}

/// The `+1.42` of the summary's `(metered: +1.42 EV, …)`.
fn summary_ev(output: &Output) -> f32 {
    let stdout = String::from_utf8_lossy(&output.stdout);
    let after = stdout
        .split("(metered: ")
        .nth(1)
        .unwrap_or_else(|| panic!("no metered clause in {stdout}"));
    after
        .split(" EV")
        .next()
        .unwrap()
        .parse()
        .unwrap_or_else(|_| panic!("unparseable EV in {stdout}"))
}

/// **Metering holds the picture's brightness across a pan** from the bright wall to the dim one: the
/// spread of the mean luma over five camera positions is under half of manual's, and the printed
/// metered EV rises toward the dark side.
#[test]
fn needs_gpu_metered_exposure_holds_brightness_across_a_pan() {
    let manual = out_path("exposure_pan_manual.yaml");
    let metered = out_path("exposure_pan_metered.yaml");
    std::fs::write(&manual, wall_scene("manual")).unwrap();
    std::fs::write(&metered, wall_scene("metered")).unwrap();

    let mut spreads = Vec::new();
    let mut evs = Vec::new();
    for (name, scene) in [("manual", &manual), ("metered", &metered)] {
        let mut lumas = Vec::new();
        for (index, x) in PAN_X.into_iter().enumerate() {
            let png = out_path(&format!("exposure_pan_{name}_{index}.png"));
            let run = render_at(scene, x, &png);
            if !run.status.success() {
                if no_adapter(&run) {
                    eprintln!(
                        "SKIPPED metered_exposure_holds_brightness_across_a_pan: no GPU adapter"
                    );
                    return;
                }
                panic!("{}", String::from_utf8_lossy(&run.stderr));
            }
            lumas.push(mean_luma(&png));
            if name == "metered" {
                evs.push(summary_ev(&run));
            } else {
                assert!(
                    !String::from_utf8_lossy(&run.stdout).contains("(metered:"),
                    "a manual frame prints no metered EV"
                );
            }
        }
        let (lo, hi) = lumas
            .iter()
            .fold((f64::MAX, f64::MIN), |(lo, hi), &l| (lo.min(l), hi.max(l)));
        spreads.push(hi - lo);
    }

    let (manual_spread, metered_spread) = (spreads[0], spreads[1]);
    assert!(
        metered_spread < manual_spread / 2.0,
        "metered spread {metered_spread:.1} should be under half of manual's {manual_spread:.1}"
    );
    assert!(
        evs.windows(2).all(|pair| pair[0] <= pair[1]) && evs[0] < evs[4],
        "the metered EV should rise toward the dark side: {evs:?}"
    );
}
