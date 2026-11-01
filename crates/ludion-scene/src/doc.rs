//! Game data-doc documents: a game registers a `kind` string plus its `schemars` JSON Schema, and
//! the engine gives that kind exactly what mesh/voxel content enjoys — strict-YAML validation with
//! L-coded pointer diagnostics and nearest-match suggestions, plus canonical byte-idempotent emit —
//! for a type this crate never names (`Plan/phase-15-data-docs.md`).
//!
//! Unlike the hand-rolled mesh/voxel parsers, a data-doc is validated the *generic, schema-driven*
//! way the scene-component path is: parse strict-YAML to a [`Value`], run [`crate::validate`]'s
//! `Checker` against the kind's schema, and emit through [`crate::emit`]'s value core. A doc kind
//! reduces to a schema `Value` + a `Value` — this crate's native vocabulary — so no concrete type
//! is ever named here.
//!
//! The envelope (decision 3): `format`/`kind` at the top, then the type's own fields flattened at
//! the top level.
//!
//! ```yaml
//! format: 1
//! kind: weapon
//! name: crossbow   # ← the registered type's own fields, validated against its schema
//! damage: 12.0
//! ```
//!
//! [`check_doc`] validates the `format`/`kind` envelope (`L1119`), strips it, and validates the
//! remaining field map against the kind's schema (`L1120`, JSON Pointer + nearest-match); strict-
//! YAML syntax errors surface the shared `L100x` codes. [`emit_doc`] writes the envelope then the
//! field map in schema-declaration order.

use ludion_core::codes::code;
use ludion_core::diagnostics::{Diagnostic, value_phrase};
use ludion_core::suggest;
use schemars::JsonSchema;
use serde::de::DeserializeOwned;
use serde_json::{Map, Value};

use crate::FORMAT_VERSION;
use crate::parse::node_to_value;
use crate::schema::SchemaCursor;
use crate::validate::{Checker, Subject};
use crate::yaml::{self, pointer_token};

/// The two reserved top-level keys of a doc or profile envelope — everything else at the top level
/// is a field of the registered type.
pub(crate) const ENVELOPE_KEYS: [&str; 2] = ["format", "kind"];

/// Refuses a registered doc or profile type whose schema declares a top-level envelope key.
///
/// `kind`/`format` fields would collide with the envelope (`emit_doc` would write the key twice,
/// `check_doc` strips it), so the registration is rejected up front (`L1257`). A tagged enum's tag
/// sits in each `oneOf`/`anyOf` branch, so those are searched too.
///
/// # Errors
/// `L1257`, naming the first offending field.
pub(crate) fn check_doc_type(kind: &str, schema: &Value) -> Result<(), Diagnostic> {
    let cursor = SchemaCursor::new(schema);
    let mut names = cursor.property_order().unwrap_or_default();
    let resolved = cursor.resolved();
    for key in ["oneOf", "anyOf"] {
        let Some(branches) = resolved.get(key).and_then(Value::as_array) else {
            continue;
        };
        for branch in branches {
            names.extend(cursor.with(branch).property_order().unwrap_or_default());
        }
    }
    let Some(field) = ENVELOPE_KEYS.iter().find(|key| names.contains(key)) else {
        return Ok(());
    };
    Err(Diagnostic::new(
        code::DOC_TYPE_RESERVED_FIELD,
        format!(
            "the `{kind}` type declares a top-level field `{field}`, which collides with the doc \
             envelope's `{field}:` key"
        ),
    )
    .with_help(format!(
        "rename the field (e.g. `#[serde(rename = \"{kind}_{field}\")]` or a new name); `format` \
         and `kind` are reserved at the top level of every data-doc and profile"
    )))
}

/// Parses and validates a game data-doc against `kind`'s JSON `schema`, returning the doc's field
/// map with the `format`/`kind` envelope stripped (ready for [`deserialize_doc`] into the
/// registered type, or use [`parse_doc`]).
///
/// # Errors
/// A strict-YAML syntax or strictness violation surfaces the shared `L1001`–`L1009` codes (fail
/// fast). A malformed envelope — missing/wrong `format`, missing/wrong/mismatched `kind`, or a
/// top-level key that is a near-miss of an envelope key — is `L1119`. Fields that do not match the
/// schema are `L1120` (JSON Pointer + nearest-match), collected in one pass.
pub(crate) fn check_doc(
    source: &str,
    kind: &str,
    schema: &Value,
) -> Result<Value, Vec<Diagnostic>> {
    let root = yaml::parse_document(source).map_err(|diagnostic| vec![diagnostic])?;
    let Some(root) = root else {
        return Err(vec![
            Diagnostic::new(
                code::DOC_ENVELOPE_INVALID,
                format!(
                    "the `{kind}` data-doc is empty; it needs at least `format: 1` and \
                     `kind: {kind}`"
                ),
            )
            .with_path("")
            .with_help(format!(
                "a minimal `{kind}` doc is:\n  format: 1\n  kind: {kind}"
            )),
        ]);
    };
    let Value::Object(map) = node_to_value(&root) else {
        return Err(vec![
            Diagnostic::new(
                code::DOC_ENVELOPE_INVALID,
                format!(
                    "the `{kind}` data-doc must be a mapping (line {})",
                    root.line
                ),
            )
            .with_path("")
            .with_help(format!(
                "start the file with the top-level keys `format:` and `kind: {kind}`"
            )),
        ]);
    };

    let mut diagnostics = Vec::new();
    check_format(kind, &map, &mut diagnostics);
    check_kind(kind, &map, &mut diagnostics);

    // Partition the top-level keys: envelope keys are stripped, a key that is a near-miss of an
    // envelope key is an envelope typo (L1119, excluded from the field map so the schema checker
    // does not double-report it), and every other key forms the field map handed to the checker
    // (a typo'd field surfaces there as L1120).
    let schema_fields = schema_field_names(schema);
    let mut fields = Map::new();
    for (key, value) in &map {
        if ENVELOPE_KEYS.contains(&key.as_str()) {
            continue;
        }
        if schema_fields.contains(&key.as_str()) {
            fields.insert(key.clone(), value.clone());
            continue;
        }
        match envelope_typo(key, &schema_fields) {
            Some(candidate) => diagnostics.push(unknown_envelope_key(kind, key, candidate)),
            None => {
                fields.insert(key.clone(), value.clone());
            }
        }
    }

    let field_value = Value::Object(fields);
    let checker = Checker {
        subject: Subject::Doc(kind),
    };
    checker.check(
        &field_value,
        SchemaCursor::new(schema),
        "",
        &mut diagnostics,
    );

    if diagnostics.is_empty() {
        Ok(field_value)
    } else {
        Err(diagnostics)
    }
}

/// Writes a game data-doc in canonical form: the `format`/`kind` envelope, then `value`'s fields in
/// schema-declaration order (ryu shortest-round-trip floats, LF endings). Byte-idempotent for a
/// value the schema accepts: `emit_doc → check_doc → emit_doc` reproduces the same text.
pub(crate) fn emit_doc(kind: &str, value: &Value, schema: &Value) -> String {
    crate::emit::emit_doc(kind, value, schema)
}

/// The step after [`check_doc`]: deserializes a schema-validated field map into `T`. A failure is a
/// schema/serde mismatch, not an authoring error, so it is one `L1120` with no key prefix.
pub(crate) fn deserialize_doc<T: DeserializeOwned>(fields: Value) -> Result<T, Diagnostic> {
    serde_json::from_value::<T>(fields).map_err(|error| {
        Diagnostic::new(
            code::DOC_SCHEMA_MISMATCH,
            format!("passed schema validation but could not be deserialized: {error}"),
        )
        .with_help("this is an engine/schema mismatch — report it with the doc that triggered it")
    })
}

/// Reads a game data-doc into `T` with the boot loader's steps: the reserved-field check, the
/// envelope and schema check, then the deserialize.
pub(crate) fn parse_doc<T: DeserializeOwned + JsonSchema>(
    source: &str,
    kind: &str,
) -> Result<T, Vec<Diagnostic>> {
    let schema = serde_json::to_value(schemars::schema_for!(T))
        .expect("a schemars JSON Schema always serializes to a JSON value");
    check_doc_type(kind, &schema).map_err(|diagnostic| vec![diagnostic])?;
    let fields = check_doc(source, kind, &schema)?;
    deserialize_doc(fields).map_err(|diagnostic| vec![diagnostic])
}

fn check_format(kind: &str, map: &Map<String, Value>, diagnostics: &mut Vec<Diagnostic>) {
    match map.get("format") {
        None => diagnostics.push(
            Diagnostic::new(
                code::DOC_ENVELOPE_INVALID,
                format!("the `{kind}` data-doc is missing the required key `format`"),
            )
            .with_path("")
            .with_help("add `format: 1` as the first line"),
        ),
        Some(Value::Number(n)) if n.as_u64() == Some(FORMAT_VERSION) => {}
        Some(Value::Number(n)) => diagnostics.push(
            Diagnostic::new(
                code::DOC_ENVELOPE_INVALID,
                format!(
                    "the `{kind}` data-doc declares format version {n}; this engine reads version \
                     {FORMAT_VERSION}"
                ),
            )
            .with_path("/format")
            .with_help("re-emit the doc with a matching engine version, or set `format: 1`"),
        ),
        Some(other) => diagnostics.push(
            Diagnostic::new(
                code::DOC_ENVELOPE_INVALID,
                format!(
                    "the `{kind}` data-doc's `format` must be the integer {FORMAT_VERSION}, found \
                     {}",
                    value_phrase(other)
                ),
            )
            .with_path("/format")
            .with_help("set `format: 1`"),
        ),
    }
}

fn check_kind(kind: &str, map: &Map<String, Value>, diagnostics: &mut Vec<Diagnostic>) {
    match map.get("kind") {
        None => diagnostics.push(
            Diagnostic::new(
                code::DOC_ENVELOPE_INVALID,
                format!("the `{kind}` data-doc is missing the required key `kind`"),
            )
            .with_path("")
            .with_help(format!("add `kind: {kind}`")),
        ),
        Some(Value::String(found)) if found == kind => {}
        Some(Value::String(found)) => diagnostics.push(
            Diagnostic::new(
                code::DOC_ENVELOPE_INVALID,
                format!(
                    "this document declares `kind: {found}`, but it was loaded as a `{kind}` \
                     data-doc"
                ),
            )
            .with_path("/kind")
            .with_help(format!(
                "set `kind: {kind}`, or load this file as a `{found}` doc"
            )),
        ),
        Some(other) => diagnostics.push(
            Diagnostic::new(
                code::DOC_ENVELOPE_INVALID,
                format!(
                    "the `{kind}` data-doc's `kind` must be the string `{kind}`, found {}",
                    value_phrase(other)
                ),
            )
            .with_path("/kind")
            .with_help(format!("set `kind: {kind}`")),
        ),
    }
}

fn unknown_envelope_key(kind: &str, key: &str, candidate: &str) -> Diagnostic {
    Diagnostic::new(
        code::DOC_ENVELOPE_INVALID,
        format!("the `{kind}` data-doc has an unknown top-level key `{key}`"),
    )
    .with_path(format!("/{}", pointer_token(key)))
    .with_help(format!("nearest match is `{candidate}`"))
}

/// The declared top-level field names of `schema`, in declaration order (empty if the schema does
/// not describe an object — the field map is then whatever the checker makes of it).
fn schema_field_names(schema: &Value) -> Vec<&str> {
    SchemaCursor::new(schema)
        .property_order()
        .unwrap_or_default()
}

/// If an unknown top-level `key` is a near-miss of an envelope key (`format`/`kind`) rather than of
/// a schema field, returns that envelope key. Nearest is computed over the envelope keys *and* the
/// schema fields together, envelope keys first (so an equidistant tie resolves to the envelope) —
/// only when the winner is an envelope key is this an envelope typo; otherwise it is a field the
/// schema checker will report (`None`).
fn envelope_typo(key: &str, schema_fields: &[&str]) -> Option<&'static str> {
    let candidates = ENVELOPE_KEYS
        .iter()
        .copied()
        .chain(schema_fields.iter().copied());
    match suggest::nearest(key, candidates) {
        Some(winner) => ENVELOPE_KEYS.into_iter().find(|&e| e == winner),
        None => None,
    }
}
