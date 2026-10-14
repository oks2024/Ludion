//! Integration tests for `ludion explain`, driven against the built `ludion` binary.

use std::process::Command;

/// `ludion explain L0001` succeeds and prints the code plus a recognizable phrase from its
/// registered explanation.
#[test]
fn explain_known_code_succeeds() {
    let output = Command::new(env!("CARGO_BIN_EXE_ludion"))
        .args(["explain", "L0001"])
        .output()
        .expect("failed to run the ludion binary");

    assert!(
        output.status.success(),
        "expected `ludion explain L0001` to succeed, stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("L0001"), "stdout was: {stdout}");
    assert!(
        stdout.contains("unknown diagnostic code"),
        "stdout was: {stdout}"
    );
}

/// `ludion explain L9999` (an unregistered code) exits non-zero and reports itself as an L0001
/// diagnostic on stderr.
#[test]
fn explain_unknown_code_fails_with_l0001() {
    let output = Command::new(env!("CARGO_BIN_EXE_ludion"))
        .args(["explain", "L9999"])
        .output()
        .expect("failed to run the ludion binary");

    assert!(
        !output.status.success(),
        "expected `ludion explain L9999` to fail"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("L0001"), "stderr was: {stderr}");
    assert!(stderr.contains("L9999"), "stderr was: {stderr}");
}

fn explain_output(args: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_ludion"))
        .arg("explain")
        .args(args)
        .output()
        .expect("failed to run the ludion binary")
}

/// A lower-case code is the same code.
#[test]
fn explain_accepts_a_lower_case_code() {
    let output = explain_output(&["l0001"]);
    assert!(output.status.success());
    assert!(String::from_utf8_lossy(&output.stdout).starts_with("L0001 ["));
}

/// A retired code is explained from the retired table, not called a typo.
#[test]
fn explain_a_retired_code_says_why() {
    let output = explain_output(&["L1193"]);
    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("L1193 [retired]"), "stdout was: {stdout}");
    assert!(stdout.contains("L1017"), "stdout was: {stdout}");
    assert!(!stdout.contains("mistyped"), "stdout was: {stdout}");
}

/// A near-miss names the closest registered code, and the diagnostic carries no invented pointer.
#[test]
fn explain_a_near_miss_suggests_the_nearest_code() {
    let output = explain_output(&["L001"]);
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("did you mean `L0001`"),
        "stderr was: {stderr}"
    );
    assert!(!stderr.contains("/error/code"), "stderr was: {stderr}");
}

/// A bad per-machine knob never blocks the verbs that read none of them.
#[test]
fn knob_free_verbs_ignore_a_bad_environment() {
    for args in [vec!["explain", "L0014"], vec!["docs"], vec!["devdocs"]] {
        let output = Command::new(env!("CARGO_BIN_EXE_ludion"))
            .args(&args)
            .env("LUDION_THREADS", "bogus")
            .env("LUDION_RENDER_SIZE", "0x0")
            .env("LUDION_QUALITY", "bogus")
            .output()
            .expect("failed to run the ludion binary");
        assert!(
            output.status.success(),
            "`ludion {}` failed under a bad environment, stderr: {}",
            args.join(" "),
            String::from_utf8_lossy(&output.stderr)
        );
    }
}

/// `ludion explain` with no argument lists every registered code — this is the behavior the
/// L0001 help text promises, so it must exist.
#[test]
fn explain_without_code_lists_all_codes() {
    let output = Command::new(env!("CARGO_BIN_EXE_ludion"))
        .arg("explain")
        .output()
        .expect("failed to run the ludion binary");

    assert!(
        output.status.success(),
        "expected `ludion explain` to succeed, stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    for entry in ludion_core::codes::CODES {
        assert!(
            stdout.contains(&entry.code.to_string()),
            "code {} missing from listing, stdout was: {stdout}",
            entry.code
        );
    }
}

/// `ludion explain L####` states whether the code is fatal — the question a reader (or an agent
/// deciding whether to keep going) has before reading a word of the explanation.
#[test]
fn explain_states_the_codes_severity() {
    let explain = |code: &str| {
        let output = Command::new(env!("CARGO_BIN_EXE_ludion"))
            .args(["explain", code])
            .output()
            .expect("failed to run the ludion binary");
        assert!(output.status.success(), "expected `explain {code}` to pass");
        String::from_utf8_lossy(&output.stdout).into_owned()
    };
    assert!(
        explain("L1028").starts_with("L1028 [warning]:"),
        "L1028 is a placement lint"
    );
    assert!(
        explain("L1027").starts_with("L1027 [error]:"),
        "L1027 is the provable gate-link failure"
    );
}

/// The collision-layer collider codes are registered and explain their pointer and fix.
#[test]
fn explain_prints_the_collider_layer_codes() {
    use ludion_core::codes::code;
    for (registered, phrase) in [
        (code::COLLIDER_LAYER_UNKNOWN, "physics.layers"),
        (code::COLLIDER_LAYER_UNCLAIMED, "2D tier has no filtering"),
    ] {
        let output = Command::new(env!("CARGO_BIN_EXE_ludion"))
            .args(["explain", registered.as_str()])
            .output()
            .expect("failed to run the ludion binary");
        assert!(
            output.status.success(),
            "explain {} failed",
            registered.as_str()
        );
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(stdout.contains(phrase), "stdout was: {stdout}");
    }
}
