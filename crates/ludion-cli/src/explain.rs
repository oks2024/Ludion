//! Implementation of `ludion explain <CODE>`.

use ludion_core::codes::{self, code::UNKNOWN_DIAGNOSTIC_CODE};
use ludion_core::diagnostics::Diagnostic;
use ludion_core::suggest::nearest;
use std::process::ExitCode;

/// Runs `ludion explain <code>`.
///
/// Looks `code` up (case-insensitively) in the `ludion-core` diagnostic-code table and prints its
/// severity, title, and long-form explanation to stdout. A retired code prints why it is gone. Any
/// other code is reported as an [`L0001`](UNKNOWN_DIAGNOSTIC_CODE) diagnostic on stderr — via
/// [`Diagnostic`]'s rustc-style `Display` impl, naming the nearest registered code when one is
/// close — and returns a non-zero exit code.
pub fn run(code: &str) -> ExitCode {
    let code = code.trim().to_ascii_uppercase();
    if let Some(entry) = codes::explain(&code) {
        println!("{} [{}]: {}", entry.code, entry.severity, entry.title);
        println!();
        println!("{}", entry.explanation);
        return ExitCode::SUCCESS;
    }
    if let Some(note) = codes::retired(&code) {
        println!("{code} [retired]: no longer raised");
        println!();
        println!("This code was retired — {note}. Retired codes are never reused.");
        return ExitCode::SUCCESS;
    }
    let help = match nearest(&code, codes::CODES.iter().map(|entry| entry.code.as_str())) {
        Some(near) => format!(
            "did you mean `{near}`? run `ludion explain` with no argument to list every registered code"
        ),
        None => "run `ludion explain` with no argument to list every registered code".to_string(),
    };
    let diagnostic = Diagnostic::new(
        UNKNOWN_DIAGNOSTIC_CODE,
        format!("unknown diagnostic code: {code}"),
    )
    .with_help(help);
    eprintln!("{diagnostic}");
    ExitCode::FAILURE
}

/// Runs `ludion explain` with no code: lists every registered diagnostic code, its severity, and its
/// title.
pub fn list() -> ExitCode {
    for entry in codes::CODES {
        println!("{} [{}]: {}", entry.code, entry.severity, entry.title);
    }
    ExitCode::SUCCESS
}
