//! Lenient player-profile envelope loading (`open_profile`) — the save-state
//! analog of [`crate::doc`], but deliberately *lenient*.
//!
//! A profile file shares the exact `format`/`kind` envelope + flattened-fields shape a data-doc
//! uses (and is written by the same [`crate::emit_doc`], with `kind: profile`), so a canonical
//! profile looks like:
//!
//! ```yaml
//! format: 1
//! kind: profile
//! unlocked_crossbow: true   # ← the game's registered profile type's own fields
//! runs: 7
//! ```
//!
//! But where [`crate::check_doc`] runs the strict `Checker` and hard-rejects any drift (correct for
//! authored content an agent must fix), a profile is a *player's* accumulated save: it must migrate,
//! not reject. So `open_profile` validates only the envelope (`format` present + integer, `kind ==
//! profile`), strips it, and hands the raw remaining field map back to the engine's lenient
//! migration — it never runs the field `Checker`, never rejects an unknown or retyped field. Only a
//! malformed *envelope* (or a non-mapping / unparseable document) is an error here, surfaced as
//! [`code::PROFILE_LOAD_FAILED`] (`L1121`); the engine turns that into the back-up-and-default path.
//! Strict-YAML syntax errors surface the shared `L100x` codes, exactly as elsewhere.

use ludion_core::codes::code;
use ludion_core::diagnostics::{Diagnostic, DiagnosticCode, value_phrase};
use serde_json::{Map, Value};

use crate::parse::node_to_value;
use crate::yaml;

/// The `kind` string every profile document declares.
pub(crate) const PROFILE_KIND: &str = "profile";

/// A profile document with its `format`/`kind` envelope validated and stripped: the remaining field
/// map plus the declared `format` version.
///
/// The engine's lenient migration owns what happens next — `serde_json::from_value::<P>(fields)`
/// with missing fields defaulted, and the `format` compared against the version the running build
/// reads. `open_profile` deliberately does no field validation of its own.
#[derive(Clone, Debug, PartialEq)]
pub struct ProfileEnvelope {
    /// The document's own fields (the registered profile type's data), envelope keys removed —
    /// ready for `serde_json::from_value` into the game's profile type.
    pub fields: Value,
    /// The declared envelope `format` version, returned verbatim (a newer-than-engine version is the
    /// engine's concern, not a parse error).
    pub format: u64,
}

/// Parses a profile document's `format`/`kind` envelope leniently, returning the stripped field map
/// and the declared `format`. See the module docs for why no field `Checker` runs.
///
/// # Errors
/// A malformed envelope — a missing/non-integer `format`, a missing/non-string/non-`profile`
/// `kind`, or a non-mapping document — is a single [`code::PROFILE_LOAD_FAILED`] (`L1121`)
/// diagnostic with a JSON Pointer at the offending key where one exists. A strict-YAML syntax or
/// strictness violation surfaces the shared `L1001`–`L1009` codes.
pub(crate) fn open_profile(source: &str) -> Result<ProfileEnvelope, Vec<Diagnostic>> {
    open_envelope(source, PROFILE_KIND, code::PROFILE_LOAD_FAILED)
}

/// The lenient `format`/`kind` envelope open every per-machine save file shares (`profile`,
/// `settings`): validates only the envelope for `kind`, reports a bad one as `code`.
pub(crate) fn open_envelope(
    source: &str,
    kind: &str,
    code: DiagnosticCode,
) -> Result<ProfileEnvelope, Vec<Diagnostic>> {
    let root = yaml::parse_document(source).map_err(|diagnostic| vec![diagnostic])?;
    let Some(root) = root else {
        return Err(vec![
            Diagnostic::new(
                code,
                format!("the {kind} is empty; it needs at least `format: 1` and `kind: {kind}`"),
            )
            .with_path("")
            .with_help(format!("a minimal {kind} is:\n  format: 1\n  kind: {kind}")),
        ]);
    };
    let Value::Object(map) = node_to_value(&root) else {
        return Err(vec![
            Diagnostic::new(
                code,
                format!("the {kind} must be a mapping (line {})", root.line),
            )
            .with_path("")
            .with_help(format!(
                "start the file with the top-level keys `format:` and `kind: {kind}`"
            )),
        ]);
    };

    let mut diagnostics = Vec::new();
    let format = check_format(&map, kind, code, &mut diagnostics);
    check_kind(&map, kind, code, &mut diagnostics);
    if !diagnostics.is_empty() {
        return Err(diagnostics);
    }
    let format = format.expect("a format value is present when there are no envelope diagnostics");

    let mut fields = Map::new();
    for (key, value) in &map {
        if crate::doc::ENVELOPE_KEYS.contains(&key.as_str()) {
            continue;
        }
        fields.insert(key.clone(), value.clone());
    }
    Ok(ProfileEnvelope {
        fields: Value::Object(fields),
        format,
    })
}

/// The declared `format` version, or `None` (with a diagnostic pushed) if it is missing or not a
/// non-negative integer. A version other than the engine's is *not* rejected here — it is returned
/// for the engine's lenient migration to back-up-and-default on.
fn check_format(
    map: &Map<String, Value>,
    kind: &str,
    code: DiagnosticCode,
    diagnostics: &mut Vec<Diagnostic>,
) -> Option<u64> {
    match map.get("format") {
        None => {
            diagnostics.push(
                Diagnostic::new(
                    code,
                    format!("the {kind} is missing the required key `format`"),
                )
                .with_path("")
                .with_help("add `format: 1` as the first line"),
            );
            None
        }
        Some(Value::Number(n)) => match n.as_u64() {
            Some(version) => Some(version),
            None => {
                diagnostics.push(
                    Diagnostic::new(
                        code,
                        format!("the {kind}'s `format` must be a non-negative integer, found {n}"),
                    )
                    .with_path("/format")
                    .with_help("set `format: 1`"),
                );
                None
            }
        },
        Some(other) => {
            diagnostics.push(
                Diagnostic::new(
                    code,
                    format!(
                        "the {kind}'s `format` must be an integer, found {}",
                        value_phrase(other)
                    ),
                )
                .with_path("/format")
                .with_help("set `format: 1`"),
            );
            None
        }
    }
}

fn check_kind(
    map: &Map<String, Value>,
    kind: &str,
    code: DiagnosticCode,
    diagnostics: &mut Vec<Diagnostic>,
) {
    match map.get("kind") {
        None => diagnostics.push(
            Diagnostic::new(
                code,
                format!("the {kind} is missing the required key `kind`"),
            )
            .with_path("")
            .with_help(format!("add `kind: {kind}`")),
        ),
        Some(Value::String(found)) if found == kind => {}
        Some(Value::String(found)) => diagnostics.push(
            Diagnostic::new(
                code,
                format!(
                    "this document declares `kind: {found}`, but a {kind} must be `kind: {kind}`"
                ),
            )
            .with_path("/kind")
            .with_help(format!("set `kind: {kind}`")),
        ),
        Some(other) => diagnostics.push(
            Diagnostic::new(
                code,
                format!(
                    "the {kind}'s `kind` must be the string `{kind}`, found {}",
                    value_phrase(other)
                ),
            )
            .with_path("/kind")
            .with_help(format!("set `kind: {kind}`")),
        ),
    }
}
