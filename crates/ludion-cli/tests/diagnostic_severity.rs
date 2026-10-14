//! The cross-verb diagnostic-rendering contract, driven against the built `ludion` binary.
//!
//! Severity lives on the diagnostic (it comes from the code table), and every verb prints through
//! one renderer (`ludion-cli`'s `diagnostic` module). These tests are what keeps that true from the
//! outside: for **each** verb that can raise a lint, the output leads with `warning[L####]` and
//! carries the diagnostic's JSON Pointer. Before the shared renderer, two of the three verbs
//! silently dropped the pointer, so a `L1028` reported which gate was sealed but not where in the
//! document to fix it.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

fn run_ludion(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_ludion"))
        .args(args)
        .output()
        .expect("failed to run the ludion binary")
}

fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(name)
}

/// Asserts stderr contains a `warning[<code>]` head whose `-->` pointer line follows it.
fn assert_warning_with_pointer(stderr: &str, code: &str, pointer: &str) {
    let head = format!("warning[{code}]: ");
    let start = stderr
        .find(&head)
        .unwrap_or_else(|| panic!("no `{head}` in stderr:\n{stderr}"));
    let block: String = stderr[start..]
        .lines()
        .take_while(|line| !line.trim().is_empty())
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        block.contains(&format!("\n  --> {pointer}")),
        "the {code} lint dropped its JSON Pointer `{pointer}`; block was:\n{block}"
    );
}

/// `check-scene` on a voxel model whose cells split into two pieces: `L1129` is a lint, so the
/// command still succeeds, and the warning names `/slices`.
#[test]
fn check_scene_renders_a_lint_with_its_pointer() {
    // Two 1x1x1 blobs with a gap between them — one document, two 6-connected components.
    let source = "format: 1\nkind: voxel\nsize:\n  - 3\n  - 1\n  - 1\npalette:\n  a:\n    color:\n      - 1\n      - 1\n      - 1\n      - 1\nslices:\n  -\n    - \"a.a\"\n";
    let path = std::env::temp_dir().join(format!(
        "ludion-severity-disconnected-{}.voxel.yaml",
        std::process::id()
    ));
    std::fs::write(&path, source).expect("failed to write the temp fixture");

    let output = run_ludion(&["check-scene", path.to_str().unwrap()]);
    let _ = std::fs::remove_file(&path);

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        output.status.success(),
        "a lint must not fail check-scene; stderr:\n{stderr}"
    );
    assert_warning_with_pointer(&stderr, "L1129", "/slices");
}

/// `map-check` on a gate the anchor cannot reach: `L1028` is a lint (the command still
/// exits zero) and names the offending marker's document position.
#[test]
fn map_check_renders_a_lint_with_its_pointer() {
    let map = fixture("lint-sealed-gate.map.yaml");
    let output = run_ludion(&["map-check", map.to_str().unwrap()]);

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        output.status.success(),
        "a placement lint must not fail map-check; stderr:\n{stderr}"
    );
    assert_warning_with_pointer(&stderr, "L1028", "/markers/");
}

/// `map-check` on a marker buried in another prop's collider: the same contract for `L1029`, the
/// other placement lint.
#[test]
fn map_check_renders_the_blocked_marker_lint_with_its_pointer() {
    let map = fixture("lint-blocked-marker.map.yaml");
    let output = run_ludion(&["map-check", map.to_str().unwrap()]);

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        output.status.success(),
        "L1029 is a lint; stderr:\n{stderr}"
    );
    assert_warning_with_pointer(&stderr, "L1029", "/markers/");
}

/// `import --allow-lossy` downgrades `L1202` to a lint for that one invocation, and the same
/// renderer prints it — head, then help. `L1202` carries no `path`: the offender is a glTF node,
/// not a position in a Ludion document, so it is named in the message instead.
#[test]
fn import_renders_the_downgraded_l1202_as_a_warning() {
    let input = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../examples/import/scene.gltf");
    let out = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("severity_import");
    let _ = std::fs::remove_dir_all(&out);
    let output = run_ludion(&[
        "import",
        input.to_str().unwrap(),
        "--out",
        out.to_str().unwrap(),
        "--allow-lossy",
    ]);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("warning[L1202]: "), "stderr:\n{stderr}");
    assert!(stderr.contains("\n  help: "), "stderr:\n{stderr}");
}

/// `check-scene` on an atlas sprite with a crossfading `Animation`: `L1209` is a lint, so it prints
/// as a warning, the verb carries on to its `OK (` line, and no `N errors found` tail appears.
#[test]
fn check_scene_succeeds_on_an_atlas_crossfade_lint() {
    let scene = fixture("sprite/scene-crossfade.yaml");
    let output = run_ludion(&["check-scene", scene.to_str().unwrap()]);

    let stderr = String::from_utf8_lossy(&output.stderr);
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        output.status.success(),
        "a lint must not fail check-scene; stderr:\n{stderr}"
    );
    assert_warning_with_pointer(&stderr, "L1209", "/entities/0/components/Animation/fade");
    assert!(stdout.contains("OK ("), "stdout:\n{stdout}");
    assert!(
        !stderr.contains("error found") && !stderr.contains("errors found"),
        "stderr:\n{stderr}"
    );
}
