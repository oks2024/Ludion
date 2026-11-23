//! Shared strict-YAML-tree envelope checks for the crate's three hand-parsed top-level document
//! shapes (scene/mesh/voxel — `parse.rs`/`mesh.rs`/`voxel.rs`): a `format:` version check
//! ([`check_envelope`]), a `kind:` check ([`check_kind`], mesh/voxel only — a scene document has no
//! `kind` key), an "expect this key to be a string" check ([`expect_string`]), and the "unrecognized
//! top-level key" / "missing required top-level key" diagnostics ([`unknown_key`], [`missing_key`]).
//!
//! Each caller supplies its own [`DiagnosticCode`] and vocabulary (the document noun, its valid key
//! set) — the wording stays bespoke per format on purpose (an agent's fix suggestion should name the
//! document it is looking at: "re-emit the scene…" vs "…the model…"), but the comparison/traversal
//! logic that produces it is single-sourced here, so a `format` version-2 policy change (the audit's
//! "5-way format-version fork", `Plan/engine-audit-2026-07-17.md` §2/§4) is one edit, not three.
//!
//! `doc.rs`/`profile.rs`'s envelope checks are deliberately **not** folded in here: they walk an
//! already-flattened `serde_json::Value` map (`crate::parse::node_to_value` has already discarded
//! line numbers by that point) rather than this module's [`Node`]/[`MapEntry`] tree, and
//! `profile.rs`'s version check is intentionally lenient — a player's save must migrate, never
//! hard-reject on drift (see its module docs) — so folding it in here would change its behavior, not
//! just its plumbing.

use ludion_core::diagnostics::{Diagnostic, DiagnosticCode};
use ludion_core::suggest;
use serde_json::Value;

use crate::AssemblyClip;
use crate::yaml::{MapEntry, Node, NodeValue, pointer_token};

/// Checks a `format:` entry: a non-negative integer equal to `expected`. Pushes a diagnostic for a
/// non-integer format (tagged `type_code`, the "found ... (line ...)" shape every structure
/// diagnostic in this crate uses) or a version other than `expected` (tagged `version_code` — the
/// "5-way format-version fork" this module single-sources: the *comparison* changes in one place
/// when a format 2 is designed; the wording stays per-caller). The two diagnostics keep independent
/// codes because scene already distinguishes them (`SCENE_TYPE_MISMATCH` vs
/// `SCENE_UNSUPPORTED_FORMAT`) while mesh/voxel collapse both into their one structure code — passing
/// the same code for both reproduces the collapsed behavior exactly.
///
/// `declares` names the document as it reads in "`{declares}` declares format version …" (e.g.
/// `"the scene"`, `"the mesh document"`); `reemit` is the noun substituted into "re-emit the
/// `{reemit}` with a matching engine version, or upgrade this one"; `current` is the noun in "the
/// current `{current}` format version is `format: 1`".
#[allow(clippy::too_many_arguments)] // the per-format wording knobs (see module docs) are the point
pub(crate) fn check_envelope(
    entry: &MapEntry,
    type_code: DiagnosticCode,
    version_code: DiagnosticCode,
    expected: u64,
    declares: &str,
    reemit: &str,
    current: &str,
    diagnostics: &mut Vec<Diagnostic>,
) {
    match &entry.node.value {
        NodeValue::Scalar(Value::Number(n)) if n.as_u64().is_some() => {
            let version = n.as_u64().expect("checked by the guard");
            if version != expected {
                diagnostics.push(
                    Diagnostic::new(
                        version_code,
                        format!(
                            "{declares} declares format version {version}; this engine reads \
                             version {expected}"
                        ),
                    )
                    .with_path("/format")
                    .with_help(format!(
                        "re-emit the {reemit} with a matching engine version, or upgrade this one"
                    )),
                );
            }
        }
        other => diagnostics.push(
            Diagnostic::new(
                type_code,
                format!(
                    "`format` must be a non-negative integer, found {} (line {})",
                    other.type_phrase(),
                    entry.node.line
                ),
            )
            .with_path("/format")
            .with_help(format!(
                "the current {current} format version is `format: 1`"
            )),
        ),
    }
}

/// Checks a `kind:` entry against `expected` (mesh/voxel only — a scene document has no `kind` key).
/// `doc_noun` names the document as it reads in "unrecognized document kind `{s}`; `{doc_noun}` must
/// declare `kind: {expected}`" (e.g. `"a mesh asset document"`, `"a voxel model document"`).
pub(crate) fn check_kind(
    entry: &MapEntry,
    code: DiagnosticCode,
    expected: &str,
    doc_noun: &str,
    diagnostics: &mut Vec<Diagnostic>,
) {
    match &entry.node.value {
        NodeValue::Scalar(Value::String(s)) if s == expected => {}
        NodeValue::Scalar(Value::String(s)) => diagnostics.push(
            Diagnostic::new(
                code,
                format!(
                    "unrecognized document kind `{s}`; {doc_noun} must declare `kind: {expected}`"
                ),
            )
            .with_path("/kind")
            .with_help(format!(
                "set `kind: {expected}`, or remove the `kind` key for a scene document"
            )),
        ),
        other => diagnostics.push(
            Diagnostic::new(
                code,
                format!(
                    "`kind` must be a string, found {} (line {})",
                    other.type_phrase(),
                    entry.node.line
                ),
            )
            .with_path("/kind")
            .with_help(format!("set `kind: {expected}`")),
        ),
    }
}

/// Checks that `node` is a string, pushing a `code`-tagged diagnostic at `pointer` (naming the field
/// as `what`) otherwise. A non-null scalar gets the "quote the value" help; anything else (a
/// sequence, a mapping, `null`) does not.
pub(crate) fn expect_string(
    node: &Node,
    pointer: &str,
    what: &str,
    code: DiagnosticCode,
    diagnostics: &mut Vec<Diagnostic>,
) -> Option<String> {
    match &node.value {
        NodeValue::Scalar(Value::String(s)) => Some(s.clone()),
        other => {
            let mut diagnostic = Diagnostic::new(
                code,
                format!(
                    "{what} must be a string, found {} (line {})",
                    other.type_phrase(),
                    node.line
                ),
            )
            .with_path(pointer);
            if matches!(other, NodeValue::Scalar(v) if !v.is_null()) {
                diagnostic = diagnostic.with_help("quote the value to make it a string");
            }
            diagnostics.push(diagnostic);
            None
        }
    }
}

/// Parses a document's `clips` key: a mapping of short name to clip file path, returned sorted by
/// short name (the canonical order the emitters write). `example` is the file path the help shows.
/// A duplicate short name is already rejected as `L1007` by the shared YAML layer.
pub(crate) fn parse_clips(
    entry: &MapEntry,
    code: DiagnosticCode,
    example: &str,
    diagnostics: &mut Vec<Diagnostic>,
) -> Vec<AssemblyClip> {
    let NodeValue::Mapping(entries) = &entry.node.value else {
        diagnostics.push(
            Diagnostic::new(
                code,
                format!(
                    "`clips` must be a mapping of clip name to clip file, found {} (line {})",
                    entry.node.value.type_phrase(),
                    entry.node.line
                ),
            )
            .with_path("/clips")
            .with_help(format!(
                "write one binding per line:\n  clips:\n    walk: {example}"
            )),
        );
        return Vec::new();
    };
    let mut out = Vec::with_capacity(entries.len());
    for sub in entries {
        let pointer = format!("/clips/{}", pointer_token(&sub.key));
        if let Some(file) = expect_string(&sub.node, &pointer, "a clip binding", code, diagnostics)
        {
            out.push(AssemblyClip {
                name: sub.key.clone(),
                file,
                speed: 1.0,
                events: Vec::new(),
            });
        }
    }
    out.sort_by(|a, b| a.name.cmp(&b.name));
    out
}

/// Builds the "does not have a key named" diagnostic for an unrecognized top-level (or, via `base`,
/// nested) key, with a nearest-match suggestion over `valid`. `context` names the mapping as it reads
/// in "`{context}` does not have a key named …" (e.g. `"a scene document"`, `"an entity"`).
pub(crate) fn unknown_key<const N: usize>(
    key: &str,
    entry: &MapEntry,
    base: &str,
    context: &str,
    valid: [&'static str; N],
    code: DiagnosticCode,
) -> Diagnostic {
    let mut diagnostic = Diagnostic::new(
        code,
        format!(
            "{context} does not have a key named `{key}` (line {})",
            entry.key_line
        ),
    )
    .with_path(format!("{base}/{}", pointer_token(key)));
    diagnostic = match suggest::nearest(key, valid) {
        Some(candidate) => diagnostic.with_help(format!("nearest match is `{candidate}`")),
        None => diagnostic.with_help(format!("valid keys are: {}", valid.join(", "))),
    };
    diagnostic
}

/// Builds the "missing the required top-level key" diagnostic (mesh/voxel — scene inlines its own,
/// one per field, since only `format` is unconditionally required). `doc_noun` names the document as
/// it reads in "the `{doc_noun}` is missing …" (e.g. `"mesh document"`, `"voxel document"`).
pub(crate) fn missing_key(
    key: &str,
    help: &str,
    code: DiagnosticCode,
    doc_noun: &str,
) -> Diagnostic {
    Diagnostic::new(
        code,
        format!("the {doc_noun} is missing the required top-level key `{key}`"),
    )
    .with_path("")
    .with_help(help)
}
