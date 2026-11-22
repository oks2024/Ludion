//! `ludion render --view` and `--views-sheet` (`Plan/secondary-views.md` D26-D28), against the built
//! binary and the radar gallery scene: one 256x256 view beside a 320x180 frame.

use std::path::PathBuf;
use std::process::{Command, Output};

fn scene() -> String {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../examples/gallery/render-view-radar.yaml")
        .to_string_lossy()
        .into_owned()
}

fn out_path(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(name)
}

fn render(extra: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_ludion"))
        .args([
            "render",
            "--scene",
            &scene(),
            "--width",
            "320",
            "--height",
            "180",
        ])
        .args(extra)
        .output()
        .expect("failed to run the ludion binary")
}

fn stderr(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

fn no_adapter(output: &Output) -> bool {
    ludion_core::require_gpu::skip_no_adapter(stderr(output).contains("error[L2003]"))
}

fn png_size(path: &PathBuf) -> (u32, u32) {
    let bytes = std::fs::read(path).expect("the PNG was written");
    assert_eq!(&bytes[1..4], b"PNG");
    let word = |at: usize| u32::from_be_bytes(bytes[at..at + 4].try_into().unwrap());
    (word(16), word(20))
}

#[test]
fn needs_gpu_view_writes_the_view_alone_at_its_size() {
    let out = out_path("render_views_radar.png");
    let run = render(&["--view", "radar", "--out", out.to_str().unwrap()]);
    if no_adapter(&run) {
        return;
    }
    assert!(run.status.success(), "{}", stderr(&run));
    assert_eq!(png_size(&out), (256, 256));
    let line = String::from_utf8_lossy(&run.stdout);
    assert!(line.contains("view radar: 256x256 on "), "{line}");
}

#[test]
fn needs_gpu_views_sheet_is_the_frame_and_the_views_in_one_row() {
    let out = out_path("render_views_sheet.png");
    let run = render(&["--views-sheet", "--out", out.to_str().unwrap()]);
    if no_adapter(&run) {
        return;
    }
    assert!(run.status.success(), "{}", stderr(&run));
    assert_eq!(png_size(&out), (320 + 8 + 256, 256));
    let line = String::from_utf8_lossy(&run.stdout);
    assert!(
        line.contains("views-sheet: frame 320x180, radar 256x256"),
        "{line}"
    );
}

#[test]
fn needs_gpu_an_unknown_view_is_l2379_with_the_rendered_names() {
    let out = out_path("render_views_unknown.png");
    let run = render(&["--view", "radr", "--out", out.to_str().unwrap()]);
    if no_adapter(&run) {
        return;
    }
    assert!(!run.status.success());
    let text = stderr(&run);
    assert!(text.contains("L2379"), "{text}");
    assert!(text.contains("did you mean `radar`?"), "{text}");
}

#[test]
fn each_refusal_names_the_flags_that_clash() {
    for extra in [
        vec!["--view", "radar", "--views-sheet"],
        vec!["--view", "radar", "--debug-view", "irradiance"],
        vec!["--views-sheet", "--sweep", "/seed=1,2"],
        vec!["--view", "radar", "--steps"],
        vec!["--view", "radar", "--dump-timings", "t.json"],
    ] {
        let run = render(&extra);
        assert!(!run.status.success(), "{extra:?}");
        assert!(
            stderr(&run).contains("cannot be used with"),
            "{extra:?}: {}",
            stderr(&run)
        );
    }
}

#[test]
fn a_frames_strip_is_refused_with_a_view() {
    let run = render(&["--view", "radar", "--frames", "0,5"]);
    assert!(!run.status.success());
    assert!(
        stderr(&run).contains("`--frames` strip"),
        "{}",
        stderr(&run)
    );
}

#[test]
fn a_view_beside_a_model_is_refused() {
    let run = Command::new(env!("CARGO_BIN_EXE_ludion"))
        .args(["render", "--model", "x.voxel.yaml", "--view", "radar"])
        .output()
        .unwrap();
    assert!(!run.status.success());
    assert!(
        stderr(&run).contains("cannot be used with"),
        "{}",
        stderr(&run)
    );
}
