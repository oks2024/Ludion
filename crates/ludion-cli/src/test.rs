//! Implementation of `ludion test [--dir <path>] [--bless] [--filter <substr>]`.
//!
//! Discovers every `*.yaml`/`*.yml` scene directly inside `--dir` (default: the current directory),
//! pairs each with `<dir>/goldens/<os>/<scene-stem>.png` — goldens are blessed per pinned OS
//! (`ludion-testkit`'s suite convention, see `ludion_testkit::suite`) — and either renders +
//! compares (normal run) or renders + (re)writes the golden (`--bless`). Diff artifacts for a
//! failing test land in `<dir>/target/testkit/` — a build-output-style directory, never inside
//! `goldens/` itself.
//!
//! A directory with no scene file is refused like a filter matching none: a run that checked
//! nothing must not read as a pass. A game project's own golden runs through `cargo test`.
//!
//! Unlike `cargo test`, this command never skips gracefully when no GPU adapter is available: it is
//! the user-facing gate, so a missing adapter is a loud, non-zero-exit `L2003` failure like any
//! other render error (only in-repo `cargo test` suites are allowed to skip).

use std::path::Path;
use std::process::ExitCode;

use ludion_core::codes::code;
use ludion_core::diagnostics::Diagnostic;

use ludion_testkit::{GoldenOutcome, SuiteDefaults};

/// Runs `ludion test`.
///
/// Prints one line per discovered scene (pass/fail/blessed, with its SSIM when known), plus a
/// one-line suite summary. Returns a non-zero exit code if any test failed to render, failed its
/// golden comparison, or (on a normal run) is missing a golden, and if the run selected no scene
/// at all (`L2209`).
pub fn run(dir: &Path, bless: bool, filter: Option<&str>) -> ExitCode {
    let tests = match ludion_testkit::discover(dir, SuiteDefaults::default(), filter) {
        Ok(tests) => tests,
        Err(diagnostic) => {
            eprintln!("{diagnostic}");
            return ExitCode::FAILURE;
        }
    };

    if tests.is_empty() {
        eprintln!("{}", checked_nothing(dir, filter));
        return ExitCode::FAILURE;
    }

    let out_dir = dir.join("target").join("testkit");
    let results = ludion_testkit::run_suite(&tests, &out_dir, bless);

    let mut failed = 0usize;
    for result in &results {
        match &result.outcome {
            Ok(GoldenOutcome::Blessed { golden }) => {
                println!("{}: BLESSED -> {}", result.name, golden.display());
            }
            Ok(GoldenOutcome::Passed { report }) => {
                println!(
                    "{}: PASS  ssim={:.4} (threshold {:.4}) worst tile {:.4} worst colour {:.2}",
                    result.name,
                    report.ssim.unwrap_or(1.0),
                    report.threshold,
                    report.worst_tile.as_ref().map_or(1.0, |tile| tile.ssim),
                    report
                        .worst_color_tile
                        .as_ref()
                        .map_or(0.0, |tile| tile.color_error)
                );
            }
            Ok(GoldenOutcome::Failed {
                report,
                actual_path,
                report_path,
            }) => {
                failed += 1;
                println!("{}: FAIL  {}", result.name, report.summary);
                println!("  actual: {}", actual_path.display());
                println!("  report: {}", report_path.display());
                eprintln!("{}", report.diagnostic());
            }
            Err(diagnostics) => {
                failed += 1;
                println!("{}: ERROR", result.name);
                for diagnostic in diagnostics {
                    eprintln!("{diagnostic}");
                    eprintln!();
                }
            }
        }
    }

    let total = results.len();
    println!(
        "{}: {}/{total} passed{}",
        dir.display(),
        total - failed,
        if bless { " (bless run)" } else { "" }
    );

    if failed > 0 {
        ExitCode::FAILURE
    } else {
        ExitCode::SUCCESS
    }
}

/// The `L2209` diagnostic for a run that selected no scene: a filter that matched none, or a
/// directory holding none.
fn checked_nothing(dir: &Path, filter: Option<&str>) -> Diagnostic {
    let stems: Vec<String> = ludion_testkit::discover(dir, SuiteDefaults::default(), None)
        .map(|all| all.into_iter().map(|test| test.name).collect())
        .unwrap_or_default();
    let message = match filter {
        Some(filter) => format!(
            "`--filter {filter}` matched no scene in `{}`",
            dir.display()
        ),
        None => format!("no scene files in `{}`", dir.display()),
    };
    let help = if !stems.is_empty() {
        format!("the suite holds: {}", stems.join(", "))
    } else if dir.join("Cargo.toml").is_file() && dir.join("ludion.toml").is_file() {
        "this is a game project: its golden runs through `cargo test` (`tests/golden.rs`); \
         `ludion test --dir <dir>` takes a directory of built-in-only scene files"
            .to_owned()
    } else {
        "point `--dir` at the directory holding the suite's `*.yaml` scenes".to_owned()
    };
    Diagnostic::new(code::TEST_CHECKED_NOTHING, message).with_help(help)
}
