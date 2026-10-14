//! Diagnostics: the engine's error UX primitive.
//!
//! Per the pillars, "errors are the UX budget": every failure aims at rustc's bar — *what* is
//! wrong, *where*, *why*, and the *likely fix*. A [`Diagnostic`] carries a stable [`DiagnosticCode`]
//! (`L####`), a [`Severity`], a human message, an optional JSON Pointer
//! [`path`](Diagnostic::path) into the offending document, an optional [`help`](Diagnostic::help)
//! suggestion, and any number of `related` sub-diagnostics. It renders rustc-style through
//! [`Display`](std::fmt::Display) and serializes to the wire error shape.
//!
//! Every code is registered in the central table in [`crate::codes`]; `ludion explain L####` is a
//! lookup there, and the table is where a code's [`Severity`] is declared — [`Diagnostic::new`]
//! reads it from there rather than leaving each call site to decide.
//!
//! # Example
//!
//! ```
//! use ludion_core::codes::code;
//! use ludion_core::diagnostics::{Diagnostic, Severity};
//!
//! let d = Diagnostic::new(code::UNKNOWN_DIAGNOSTIC_CODE, "unknown diagnostic code: L9999")
//!     .with_path("/error/code")
//!     .with_help("run `ludion explain` to list valid codes");
//! assert_eq!(d.code.as_str(), "L0001");
//! assert_eq!(d.severity, Severity::Error);
//! assert!(d.to_string().starts_with("error[L0001]"));
//!
//! // A code the table registers as a lint constructs as a warning, and renders as one.
//! let lint = Diagnostic::new(code::MAP_GATE_SEALED, "gate is sealed behind the boundary wall");
//! assert_eq!(lint.severity, Severity::Warning);
//! assert!(lint.to_string().starts_with("warning[L1028]"));
//! ```

use serde::{Deserialize, Serialize};

/// A validated diagnostic code of the form `L####` (an `L` followed by exactly four digits).
///
/// Stored inline as five ASCII bytes — `Copy`, allocation-free, and free of lifetime baggage, so it
/// travels through snapshots and the wire without ceremony. Construct code constants with
/// [`DiagnosticCode::new`]; malformed literals fail to compile.
///
/// # Example
///
/// ```
/// use ludion_core::diagnostics::DiagnosticCode;
/// const MY_CODE: DiagnosticCode = DiagnosticCode::new("L0042");
/// assert_eq!(MY_CODE.as_str(), "L0042");
/// assert_eq!(MY_CODE.domain_digit(), b'0');
/// ```
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct DiagnosticCode([u8; 5]);

impl DiagnosticCode {
    /// Constructs a code from a `&str`, validating the `L####` shape at compile time for `const`
    /// callers.
    ///
    /// # Panics
    /// Panics (a compile error in `const` context) if `code` is not `L` followed by four ASCII
    /// digits.
    pub const fn new(code: &str) -> Self {
        let b = code.as_bytes();
        assert!(
            b.len() == 5,
            "diagnostic code must be 'L' followed by four digits"
        );
        assert!(b[0] == b'L', "diagnostic code must start with 'L'");
        let mut i = 1;
        while i < 5 {
            assert!(
                b[i] >= b'0' && b[i] <= b'9',
                "diagnostic code must be 'L' followed by four digits"
            );
            i += 1;
        }
        DiagnosticCode([b[0], b[1], b[2], b[3], b[4]])
    }

    /// Parses a runtime string into a code, returning `None` if it is not well-formed.
    pub fn parse(code: &str) -> Option<Self> {
        let b = code.as_bytes();
        if b.len() != 5 || b[0] != b'L' || !b[1..].iter().all(u8::is_ascii_digit) {
            return None;
        }
        Some(DiagnosticCode([b[0], b[1], b[2], b[3], b[4]]))
    }

    /// The code as a `&str`, e.g. `"L0001"`.
    pub fn as_str(&self) -> &str {
        // Safe: every byte was validated ASCII at construction.
        std::str::from_utf8(&self.0).unwrap_or("L????")
    }

    /// The first digit after `L`, which selects the domain range (`0` = core/ecs, `1` = scene/
    /// assets, `2` = render, `3` = protocol/server, `4` = sim, `5` = audio).
    pub const fn domain_digit(&self) -> u8 {
        self.0[1]
    }
}

impl std::fmt::Debug for DiagnosticCode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "DiagnosticCode({})", self.as_str())
    }
}

impl std::fmt::Display for DiagnosticCode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

impl Serialize for DiagnosticCode {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(self.as_str())
    }
}

impl<'de> Deserialize<'de> for DiagnosticCode {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let s = String::deserialize(deserializer)?;
        DiagnosticCode::parse(&s).ok_or_else(|| {
            serde::de::Error::custom(format!("invalid diagnostic code '{s}', expected L####"))
        })
    }
}

/// How much a diagnostic costs its reader: a hard failure, or a lint that reports and moves on.
///
/// A code's severity is declared once, in the [`crate::codes`] table, and
/// [`Diagnostic::new`] reads it from there — the container a diagnostic lands in no longer decides
/// how it renders. [`Severity::Error`] is the default, so a code with no opinion is fatal.
///
/// Serializes as the lowercase strings `"error"` / `"warning"`, and is *skipped* on the wire when it
/// is `Error`: an ordinary error's payload is byte-identical to what it was before severity existed,
/// and only a warning pays a field for it.
///
/// # Example
///
/// ```
/// use ludion_core::diagnostics::Severity;
///
/// assert_eq!(Severity::default(), Severity::Error);
/// assert_eq!(Severity::Warning.as_str(), "warning");
/// assert_eq!(serde_json::to_string(&Severity::Warning).unwrap(), "\"warning\"");
/// ```
#[derive(
    Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug, Default, Serialize, Deserialize,
)]
#[serde(rename_all = "lowercase")]
pub enum Severity {
    /// A hard failure: the operation did not happen, and the caller's exit code says so.
    #[default]
    Error,
    /// A lint: reported to the reader, but the operation still succeeded.
    Warning,
}

impl Severity {
    /// The rustc-style word this severity renders as: `"error"` or `"warning"`.
    pub const fn as_str(&self) -> &'static str {
        match self {
            Severity::Error => "error",
            Severity::Warning => "warning",
        }
    }

    /// Whether this is [`Severity::Error`] — the serde `skip_serializing_if` predicate that keeps an
    /// ordinary error's wire payload free of a severity field.
    pub const fn is_error(&self) -> bool {
        matches!(self, Severity::Error)
    }
}

impl std::fmt::Display for Severity {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// A structured, code-carrying diagnostic rendered in rustc's style.
///
/// Construct one with [`Diagnostic::new`] and the `with_*` builders — the struct is
/// `#[non_exhaustive]`, so downstream crates cannot build it with a struct literal and a future
/// field is an additive, non-breaking change. Its fields stay `pub` for reading and pattern
/// matching (with a `..` rest).
///
/// # Example
///
/// ```
/// use ludion_core::codes::code;
/// use ludion_core::diagnostics::Diagnostic;
///
/// let d = Diagnostic::new(code::UNKNOWN_DIAGNOSTIC_CODE, "boom");
/// assert_eq!(d.message, "boom");
/// assert!(d.path.is_none());
/// ```
#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
#[non_exhaustive]
pub struct Diagnostic {
    /// The stable `L####` code identifying this diagnostic class.
    pub code: DiagnosticCode,
    /// Whether this is a hard failure or a lint. Defaults from the code's row in
    /// [`crate::codes::CODES`].
    #[serde(default, skip_serializing_if = "Severity::is_error")]
    pub severity: Severity,
    /// The primary human-readable message.
    pub message: String,
    /// Optional location of the problem. Two grammars: an RFC 6901 JSON Pointer into the file the
    /// diagnostic names (e.g. `/entities/3/components/Sprite/texture`; build tokens with
    /// [`pointer_token`]), or, for data-doc diagnostics, `<kind>/<doc name>/<field>` — no leading
    /// slash, not a pointer (`docs/data-docs.md`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    /// Optional actionable suggestion (the "likely fix").
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub help: Option<String>,
    /// Subordinate diagnostics that add context to this one.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub related: Vec<Diagnostic>,
}

impl Diagnostic {
    /// Creates a diagnostic with a code and message; path, help, and related default to empty, and
    /// the severity comes from the code's row in [`crate::codes::CODES`] (an unregistered code is an
    /// error).
    pub fn new(code: DiagnosticCode, message: impl Into<String>) -> Self {
        Diagnostic {
            code,
            severity: crate::codes::severity(code),
            message: message.into(),
            path: None,
            help: None,
            related: Vec::new(),
        }
    }

    /// Overrides the severity the code table declared, consuming and returning `self` (builder
    /// style).
    ///
    /// The table is the source of truth, so this exists only for a code that is genuinely both — one
    /// whose fatality is a property of the *invocation*, not of the class. `L1202`
    /// (`import --allow-lossy`) is the case it was added for: the same unsupported-glTF-feature
    /// finding fails the import by default and is a printed lint under the flag.
    #[must_use]
    pub fn with_severity(mut self, severity: Severity) -> Self {
        self.severity = severity;
        self
    }

    /// Sets the JSON Pointer path, consuming and returning `self` (builder style).
    #[must_use]
    pub fn with_path(mut self, path: impl Into<String>) -> Self {
        self.path = Some(path.into());
        self
    }

    /// Sets the help suggestion, consuming and returning `self` (builder style).
    #[must_use]
    pub fn with_help(mut self, help: impl Into<String>) -> Self {
        self.help = Some(help.into());
        self
    }

    /// Adds a sentence to the help, consuming and returning `self` (builder style): sets it when
    /// there is none, otherwise joins it after the existing help with `"; "`. Use it, not
    /// [`with_help`](Self::with_help), where the code's own help may already be set.
    ///
    /// ```
    /// use ludion_core::codes::code;
    /// use ludion_core::diagnostics::Diagnostic;
    ///
    /// let diagnostic = Diagnostic::new(code::OUTPUT_IO, "could not write `out.json`")
    ///     .with_help("check the directory is writable")
    ///     .append_help("the run continues");
    /// assert_eq!(
    ///     diagnostic.help.as_deref(),
    ///     Some("check the directory is writable; the run continues")
    /// );
    /// ```
    #[must_use]
    pub fn append_help(mut self, extra: impl Into<String>) -> Self {
        let extra = extra.into();
        self.help = Some(match self.help.take() {
            Some(existing) => format!("{existing}; {extra}"),
            None => extra,
        });
        self
    }

    /// Attaches a related sub-diagnostic, consuming and returning `self` (builder style).
    #[must_use]
    pub fn with_related(mut self, related: Diagnostic) -> Self {
        self.related.push(related);
        self
    }
}

impl std::fmt::Display for Diagnostic {
    /// Renders rustc-style:
    ///
    /// ```text
    /// error[L0001]: unknown diagnostic code: L9999
    ///   --> /error/code
    ///   help: run `ludion explain` to list valid codes
    /// ```
    ///
    /// A [`Severity::Warning`] leads with `warning[...]` instead, exactly as rustc does.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}[{}]: {}", self.severity, self.code, self.message)?;
        if let Some(path) = &self.path {
            write!(f, "\n  --> {path}")?;
        }
        if let Some(help) = &self.help {
            write!(f, "\n  help: {help}")?;
        }
        for related in &self.related {
            // Indent nested diagnostics one level so relationships read clearly.
            for line in related.to_string().lines() {
                write!(f, "\n  {line}")?;
            }
        }
        Ok(())
    }
}

/// A rendered diagnostic list: the terminal text and how many of its diagnostics are errors.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RenderedReport {
    /// Every diagnostic, warnings included, then the `N errors found` tail if any is an error.
    pub text: String,
    /// How many of the diagnostics are [`Severity::Error`]; a warning never counts.
    pub errors: usize,
}

/// Renders `diagnostics` for a terminal: each one's [`Display`](std::fmt::Display) form followed by
/// a blank line, in input order, then `<source>: N errors found` — only when at least one is an
/// error, so a list of warnings prints and fails nothing. The caller prints [`RenderedReport::text`]
/// and decides its exit code from [`RenderedReport::errors`].
///
/// # Example
///
/// ```
/// use ludion_core::codes::code;
/// use ludion_core::diagnostics::{Diagnostic, render_report};
///
/// let lint = Diagnostic::new(code::MAP_GATE_SEALED, "gate is sealed");
/// let report = render_report(std::path::Path::new("level.map.yaml"), &[lint]);
/// assert_eq!(report.errors, 0);
/// assert!(!report.text.contains("found"));
/// ```
pub fn render_report(source: &std::path::Path, diagnostics: &[Diagnostic]) -> RenderedReport {
    let mut text = String::new();
    for diagnostic in diagnostics {
        text.push_str(&diagnostic.to_string());
        text.push_str("\n\n");
    }
    let errors = diagnostics.iter().filter(|d| d.severity.is_error()).count();
    if errors > 0 {
        text.push_str(&format!(
            "{}: {errors} error{} found\n",
            crate::paths::slash(source),
            if errors == 1 { "" } else { "s" }
        ));
    }
    RenderedReport { text, errors }
}

/// A short "a string" / "an integer" / "a float" / … phrase naming a concrete JSON value's kind, for
/// type-mismatch diagnostic messages ("expected a mapping, found a string"). Shared by every text
/// format's checker (scene/mesh/voxel/data-doc validation, the strict-YAML parse tree) so the
/// vocabulary — and its wording — has exactly one owner.
///
/// # Example
///
/// ```
/// use ludion_core::diagnostics::value_phrase;
/// use serde_json::json;
///
/// assert_eq!(value_phrase(&json!(null)), "null");
/// assert_eq!(value_phrase(&json!(1.5)), "a float");
/// assert_eq!(value_phrase(&json!(1)), "an integer");
/// assert_eq!(value_phrase(&json!("s")), "a string");
/// assert_eq!(value_phrase(&json!([])), "a sequence");
/// assert_eq!(value_phrase(&json!({})), "a mapping");
/// ```
pub fn value_phrase(value: &serde_json::Value) -> &'static str {
    match value {
        serde_json::Value::Null => "null",
        serde_json::Value::Bool(_) => "a boolean",
        serde_json::Value::Number(n) if n.is_f64() => "a float",
        serde_json::Value::Number(_) => "an integer",
        serde_json::Value::String(_) => "a string",
        serde_json::Value::Array(_) => "a sequence",
        serde_json::Value::Object(_) => "a mapping",
    }
}

/// Escapes one reference token of an RFC 6901 JSON Pointer (`~` → `~0`, then `/` → `~1`), for
/// building a [`Diagnostic::path`] from a map key.
///
/// ```
/// use ludion_core::diagnostics::pointer_token;
///
/// assert_eq!(format!("/legend/{}", pointer_token("a/b")), "/legend/a~1b");
/// ```
pub fn pointer_token(segment: &str) -> String {
    segment.replace('~', "~0").replace('/', "~1")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::codes::code;

    #[test]
    fn pointer_token_matches_rfc_6901_examples() {
        assert_eq!(pointer_token("a/b"), "a~1b");
        assert_eq!(pointer_token("m~n"), "m~0n");
        assert_eq!(pointer_token("~/"), "~0~1");
        assert_eq!(pointer_token("plain"), "plain");
    }

    #[test]
    fn append_help_sets_help_when_there_is_none() {
        let d = Diagnostic::new(code::OUTPUT_IO, "m").append_help("extra");
        assert_eq!(d.help.as_deref(), Some("extra"));
    }

    #[test]
    fn append_help_joins_after_existing_help() {
        let d = Diagnostic::new(code::OUTPUT_IO, "m")
            .with_help("first")
            .append_help("second");
        assert_eq!(d.help.as_deref(), Some("first; second"));
    }

    #[test]
    fn code_parse_rejects_malformed() {
        assert!(DiagnosticCode::parse("L0001").is_some());
        assert!(DiagnosticCode::parse("L001").is_none());
        assert!(DiagnosticCode::parse("X0001").is_none());
        assert!(DiagnosticCode::parse("L00A1").is_none());
        assert!(DiagnosticCode::parse("L00011").is_none());
    }

    #[test]
    fn display_is_rustc_style() {
        let d = Diagnostic::new(
            code::UNKNOWN_DIAGNOSTIC_CODE,
            "unknown diagnostic code: L9999",
        )
        .with_path("/error/code")
        .with_help("run `ludion explain` to list valid codes");
        let rendered = d.to_string();
        assert_eq!(
            rendered,
            "error[L0001]: unknown diagnostic code: L9999\n  --> /error/code\n  help: run `ludion explain` to list valid codes"
        );
    }

    #[test]
    fn related_diagnostics_nest() {
        let child = Diagnostic::new(code::UNKNOWN_DIAGNOSTIC_CODE, "child");
        let parent = Diagnostic::new(code::UNKNOWN_DIAGNOSTIC_CODE, "parent").with_related(child);
        let rendered = parent.to_string();
        assert!(rendered.contains("error[L0001]: parent"));
        assert!(rendered.contains("  error[L0001]: child"));
    }

    #[test]
    fn serde_round_trips() {
        let d = Diagnostic::new(code::UNKNOWN_DIAGNOSTIC_CODE, "boom").with_path("/a/b");
        let json = serde_json::to_string(&d).unwrap();
        let back: Diagnostic = serde_json::from_str(&json).unwrap();
        assert_eq!(d, back);
    }

    #[test]
    fn serde_skips_empty_optionals() {
        let d = Diagnostic::new(code::UNKNOWN_DIAGNOSTIC_CODE, "x");
        let json = serde_json::to_string(&d).unwrap();
        assert!(!json.contains("path"));
        assert!(!json.contains("help"));
        assert!(!json.contains("related"));
    }

    #[test]
    fn severity_defaults_from_the_code_table() {
        assert_eq!(
            Diagnostic::new(code::UNKNOWN_DIAGNOSTIC_CODE, "x").severity,
            Severity::Error
        );
        assert_eq!(
            Diagnostic::new(code::MAP_GATE_SEALED, "x").severity,
            Severity::Warning
        );
    }

    #[test]
    fn display_leads_with_the_severity() {
        let warning = Diagnostic::new(code::MAP_MARKER_BLOCKED, "marker sits inside a collider")
            .with_path("/markers/2")
            .with_help("move it one cell");
        assert_eq!(
            warning.to_string(),
            "warning[L1029]: marker sits inside a collider\n  --> /markers/2\n  help: move it one \
             cell"
        );
    }

    #[test]
    fn an_error_serializes_without_a_severity_field() {
        // The wire payload of an ordinary error is byte-identical to the pre-severity shape.
        let d = Diagnostic::new(code::UNKNOWN_DIAGNOSTIC_CODE, "x");
        assert_eq!(
            serde_json::to_string(&d).unwrap(),
            r#"{"code":"L0001","message":"x"}"#
        );
    }

    #[test]
    fn a_warning_round_trips_its_severity() {
        let d = Diagnostic::new(code::MAP_GATE_SEALED, "sealed");
        let json = serde_json::to_string(&d).unwrap();
        assert!(json.contains(r#""severity":"warning""#), "json was: {json}");
        let back: Diagnostic = serde_json::from_str(&json).unwrap();
        assert_eq!(back, d);
    }

    #[test]
    fn render_report_counts_only_errors() {
        let error = Diagnostic::new(code::UNKNOWN_DIAGNOSTIC_CODE, "bad code");
        let lint = Diagnostic::new(code::MAP_GATE_SEALED, "gate is sealed");
        let source = std::path::Path::new("a/b.yaml");

        let mixed = render_report(source, &[error.clone(), lint.clone()]);
        assert_eq!(mixed.errors, 1);
        assert_eq!(
            mixed.text,
            format!("{error}\n\n{lint}\n\na/b.yaml: 1 error found\n")
        );

        let warnings = render_report(source, std::slice::from_ref(&lint));
        assert_eq!(warnings.errors, 0);
        assert_eq!(warnings.text, format!("{lint}\n\n"));

        let two = render_report(source, &[error.clone(), error]);
        assert_eq!(two.errors, 2);
        assert!(two.text.ends_with("2 errors found\n"));

        let none = render_report(source, &[]);
        assert_eq!((none.errors, none.text.as_str()), (0, ""));
    }

    #[test]
    fn with_severity_overrides_the_table() {
        let d = Diagnostic::new(code::IMPORT_UNSUPPORTED_FEATURE, "a base-color texture");
        assert_eq!(d.severity, Severity::Error);
        assert_eq!(
            d.with_severity(Severity::Warning).severity,
            Severity::Warning
        );
    }
}
