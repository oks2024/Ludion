//! The facade's usage-error printers: the only prefixed (`error:` / `warning:`) lines the host
//! prints. For a bad flag or argument only; anything about files, the environment or the engine is
//! a coded `Diagnostic` printed through `Display`.

use std::process::ExitCode;

/// Prints a usage error and returns the failure exit code.
pub(crate) fn usage_error(message: impl std::fmt::Display) -> ExitCode {
    eprintln!("error: {message}");
    ExitCode::FAILURE
}

/// Prints a usage warning: the process carries on.
pub(crate) fn usage_warning(message: impl std::fmt::Display) {
    eprintln!("warning: {message}");
}
