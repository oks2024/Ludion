//! Integration tests for `ludion trace`, driven against the built `ludion` binary: two processes
//! print the same bytes, and a bad channel is refused before anything is printed.

use std::path::PathBuf;
use std::process::{Command, Output};

/// A static sensor with a rigid ball falling into it. Built-in components only: the CLI runs no
/// game systems.
const SCENE: &str = "format: 1
physics:
  space: free
entities:
  - name: zone
    components:
      Transform:
        translation:
          - 0.0
          - 0.0
          - 0.0
      Collider:
        shape:
          box:
            half_extents:
              - 2.0
              - 0.5
              - 2.0
        body: static
        sensor: true
  - name: ball
    components:
      Transform:
        translation:
          - 0.0
          - 1.0
          - 0.0
      Collider:
        shape:
          sphere:
            radius: 0.25
        body:
          rigid:
            mass: 1.0
";

fn scene_file(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("ludion-cli-trace-{}-{name}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("scene.yaml");
    std::fs::write(&path, SCENE).unwrap();
    path
}

fn run_trace(scene: &PathBuf, extra: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_ludion"))
        .arg("trace")
        .arg("--scene")
        .arg(scene)
        .args(["--frames", "60"])
        .args(extra)
        .output()
        .expect("failed to run the ludion binary")
}

#[test]
fn two_processes_print_the_same_trace() {
    let scene = scene_file("same");
    let first = run_trace(&scene, &[]);
    assert!(
        first.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&first.stderr)
    );
    let second = run_trace(&scene, &[]);
    assert_eq!(first.stdout, second.stdout, "the trace is not byte-stable");

    let text = String::from_utf8(first.stdout).unwrap();
    assert!(
        text.lines()
            .any(|line| line.starts_with("{\"channel\":\"sensors\"")),
        "no sensor line in:\n{text}"
    );
}

#[test]
fn a_channel_selects_its_lines_only() {
    let scene = scene_file("select");
    let output = run_trace(&scene, &["--channels", "motor"]);
    assert!(output.status.success());
    assert!(output.stdout.is_empty(), "this scene has no motor");
}

#[test]
fn an_unknown_channel_is_refused_with_nothing_on_stdout() {
    let scene = scene_file("unknown");
    let output = run_trace(&scene, &["--channels", "motor,bogus"]);
    assert!(!output.status.success());
    assert!(output.stdout.is_empty());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("unknown trace channel `bogus`"), "{stderr}");
}
