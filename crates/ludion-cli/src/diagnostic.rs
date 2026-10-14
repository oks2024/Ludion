//! The one place the CLI turns a [`Diagnostic`] into terminal output.
//!
//! Every verb that surfaces diagnostics — `check-scene`, `import`, `map-check` — renders through
//! here, so a lint's JSON Pointer reaches the reader no matter which verb raised it. Severity comes
//! from the diagnostic itself (the code table decides, `ludion-core`'s `codes`), so there is nothing
//! per-verb left to get wrong: `Display` already leads with `error[...]` or `warning[...]`, and only
//! an error fails the verb.

use ludion_core::diagnostics::{Diagnostic, render_report};
use std::path::Path;
use std::process::ExitCode;

/// Prints one diagnostic to stderr, rustc-style, followed by a blank separator line.
pub(crate) fn print(diagnostic: &Diagnostic) {
    eprintln!("{diagnostic}");
    eprintln!();
}

/// The `--json` object every verb that prints one shares: `ok`, then `fields` (an object; anything
/// else adds nothing), then `diagnostics` — every diagnostic the verb rendered, in order.
pub(crate) fn json_envelope(
    ok: bool,
    fields: serde_json::Value,
    diagnostics: &[Diagnostic],
) -> serde_json::Map<String, serde_json::Value> {
    let mut object = serde_json::Map::new();
    object.insert("ok".into(), ok.into());
    if let serde_json::Value::Object(fields) = fields {
        object.extend(fields);
    }
    object.insert(
        "diagnostics".into(),
        serde_json::to_value(diagnostics).expect("diagnostics serialize"),
    );
    object
}

/// Prints a usage error — a bad flag or argument combination only; anything about files, the
/// environment or the engine is a coded [`Diagnostic`] — and returns the failure exit code.
pub(crate) fn usage_error(message: impl std::fmt::Display) -> ExitCode {
    eprintln!("error: {message}");
    ExitCode::FAILURE
}

/// Prints every diagnostic plus, when any is an error, the `<source>: N errors found` tail. Returns
/// the failure exit code iff an error was among them — warnings alone print and succeed.
pub(crate) fn report(source: &Path, diagnostics: &[Diagnostic]) -> ExitCode {
    fail_on_errors(source, diagnostics).unwrap_or(ExitCode::SUCCESS)
}

/// [`report`] for a verb with passes left to run: prints every diagnostic and returns
/// `Some(FAILURE)` iff one is an error, `None` when only warnings (or nothing) were raised so the
/// caller carries on.
pub(crate) fn fail_on_errors(source: &Path, diagnostics: &[Diagnostic]) -> Option<ExitCode> {
    let rendered = render_report(source, diagnostics);
    eprint!("{}", rendered.text);
    (rendered.errors > 0).then_some(ExitCode::FAILURE)
}

#[cfg(test)]
mod tests {
    use super::*;
    use ludion_core::codes::code;
    use ludion_core::diagnostics::Severity;

    /// The finding this module exists for: a lint renders as a warning *and* keeps its pointer.
    /// `Display` is what both `print` and `report` write, so asserting on it covers every verb.
    #[test]
    fn a_lint_renders_as_a_warning_with_its_pointer() {
        let lint = Diagnostic::new(code::MAP_GATE_SEALED, "gate is sealed")
            .with_path("/markers/4")
            .with_help("open a gap in the perimeter wall");
        assert_eq!(lint.severity, Severity::Warning);
        assert_eq!(
            lint.to_string(),
            "warning[L1028]: gate is sealed\n  --> /markers/4\n  help: open a gap in the perimeter \
             wall"
        );
    }
}
