//! `ludion render --paired` (`Plan/paired-animations.md` W6, decisions 33-36): one cell per phase of
//! the committed choke document, the phase lines, and the contact lines `check-scene` prints. Needs a
//! GPU adapter and skips gracefully without one (L2003); the arg-wiring and refusal tests that never
//! reach the renderer need none.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

fn choke() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples/gallery/voxels/choke.paired.yaml")
}

fn run_ludion(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_ludion"))
        .args(args)
        .output()
        .expect("failed to run the ludion binary")
}

/// A fresh output path under `CARGO_TARGET_TMPDIR`.
fn out_path(name: &str) -> PathBuf {
    let path = Path::new(env!("CARGO_TARGET_TMPDIR")).join(name);
    let _ = std::fs::remove_file(&path);
    path
}

fn text(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

fn is_no_adapter(output: &Output) -> bool {
    ludion_core::require_gpu::skip_no_adapter(text(&output.stderr).contains("error[L2003]"))
}

/// Renders the choke document with `extra` flags to `name`; `None` when the box has no adapter.
fn render(name: &str, extra: &[&str]) -> Option<(Output, PathBuf)> {
    let out = out_path(name);
    let doc = choke();
    let mut args = vec!["render", "--paired", doc.to_str().unwrap(), "--out"];
    args.push(out.to_str().unwrap());
    args.extend_from_slice(extra);
    let output = run_ludion(&args);
    if is_no_adapter(&output) {
        eprintln!("SKIPPED {name}: no GPU adapter");
        return None;
    }
    Some((output, out))
}

fn phase_lines(stdout: &str) -> Vec<&str> {
    stdout
        .lines()
        .filter(|line| line.starts_with("phase "))
        .collect()
}

#[test]
fn the_default_draws_one_phase_per_event() {
    let Some((output, out)) = render("paired_default.png", &[]) else {
        return;
    };
    let stdout = text(&output.stdout);
    assert!(output.status.success(), "stderr: {}", text(&output.stderr));
    assert_eq!(phase_lines(&stdout).len(), 3, "{stdout}");
    assert!(
        stdout.contains("[grab]") && stdout.contains("[release]"),
        "{stdout}"
    );
    assert!(out.exists(), "the sheet is written");
}

#[test]
fn a_strip_draws_n_phases_ending_on_the_clip_end() {
    let Some((output, _)) = render("paired_strip.png", &["--strip", "4"]) else {
        return;
    };
    let stdout = text(&output.stdout);
    assert!(output.status.success(), "stderr: {}", text(&output.stderr));
    let phases = phase_lines(&stdout);
    assert_eq!(phases.len(), 4, "{stdout}");
    assert!(phases[3].starts_with("phase 3: t 1.2 s"), "{stdout}");
}

#[test]
fn a_time_draws_one_phase_with_its_event() {
    let Some((output, _)) = render("paired_time.png", &["--time", "0.5"]) else {
        return;
    };
    let stdout = text(&output.stdout);
    assert!(output.status.success(), "stderr: {}", text(&output.stderr));
    let phases = phase_lines(&stdout);
    assert_eq!(phases.len(), 1, "{stdout}");
    assert!(phases[0].contains("[hold]"), "{stdout}");
}

/// The contact line is the one `check-scene` prints, byte for byte.
#[test]
fn the_contact_line_is_check_scenes() {
    let Some((output, _)) = render("paired_contact.png", &[]) else {
        return;
    };
    let rendered = text(&output.stdout);
    let checked = run_ludion(&["check-scene", choke().to_str().unwrap()]);
    let checked = text(&checked.stdout);
    let line = |s: &str| {
        s.lines()
            .find(|l| l.starts_with("contact "))
            .map(str::to_owned)
    };
    assert!(line(&checked).is_some(), "{checked}");
    assert_eq!(line(&rendered), line(&checked));
}

#[test]
fn unusable_phase_flags_print_l1756() {
    for extra in [["--strip", "1"], ["--time", "9"]] {
        let Some((output, _)) = render("paired_refused.png", &extra) else {
            return;
        };
        assert!(!output.status.success(), "{extra:?}");
        assert!(
            text(&output.stderr).contains("L1756"),
            "{extra:?}: {}",
            text(&output.stderr)
        );
    }
}

#[test]
fn paired_with_a_model_flag_is_a_clap_conflict() {
    let output = run_ludion(&[
        "render",
        "--paired",
        choke().to_str().unwrap(),
        "--clip",
        "idle",
    ]);
    assert!(!output.status.success());
    assert!(
        text(&output.stderr).contains("cannot be used with"),
        "{}",
        text(&output.stderr)
    );
}
