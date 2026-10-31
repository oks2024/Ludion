//! Lenient player-profile envelope machinery (`open_profile`, Phase 16 / E5 Wave 1) — the save-state
//! sibling of `doc.rs`:
//!
//! - a well-formed envelope returns the stripped field map + `format`, with **no** field checking;
//! - a missing/wrong `format`, a missing/wrong/non-`profile` `kind`, and a non-mapping root each
//!   surface `L1121` (with a JSON Pointer where applicable);
//! - a strict-YAML syntax error surfaces the shared `L100x` codes, not `L1121`;
//! - **byte-idempotence**: `emit_doc("profile", …) → open_profile → emit_doc` is byte-equal (the
//!   canonical write is `emit_doc` with `kind: profile`; the read is `open_profile`);
//! - the new code explains (`ludion explain L1121`).

use ludion_scene::{emit_doc, open_profile};
use proptest::prelude::*;
#[path = "support/deterministic.rs"]
mod deterministic;
use deterministic::deterministic_config;
use serde_json::{Map, Value, json};

/// A profile type's schema — the shape `schema_for!(SaveState)` would produce for a struct with a
/// bool, an unsigned counter, a float, and an optional string. Shared by the unit tests and the
/// round-trip proptest so both exercise the same fields.
fn profile_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "unlocked_crossbow": { "type": "boolean" },
            "runs":             { "type": "integer" },
            "best_time":        { "type": "number" },
            "last_area":        { "type": "string" }
        },
        "required": ["unlocked_crossbow", "runs"]
    })
}

// --- Happy path -------------------------------------------------------------------------------

#[test]
fn valid_envelope_strips_format_and_kind() {
    let source = "\
format: 1
kind: profile
unlocked_crossbow: true
runs: 7
best_time: 42.5
";
    let profile = open_profile(source).expect("a well-formed profile envelope");
    assert_eq!(profile.format, 1);
    // The envelope keys are gone; the type's own fields remain, unvalidated.
    assert!(profile.fields.get("format").is_none());
    assert!(profile.fields.get("kind").is_none());
    assert_eq!(profile.fields["unlocked_crossbow"], json!(true));
    assert_eq!(profile.fields["runs"], json!(7));
    assert_eq!(profile.fields["best_time"], json!(42.5));
}

#[test]
fn unknown_and_missing_fields_are_not_rejected() {
    // Lenience: a field the type does not declare, and none of the "required" fields present, are
    // both fine at this layer — the engine's migration fills/drops them. Only the envelope matters.
    let source = "format: 1\nkind: profile\ntotally_new_field: 3\n";
    let profile = open_profile(source).expect("open_profile does not run the field checker");
    assert_eq!(profile.fields["totally_new_field"], json!(3));
}

#[test]
fn newer_format_is_returned_not_rejected() {
    // A newer-than-engine `format` is the engine's concern (back-up-and-default), not a parse error.
    let source = "format: 2\nkind: profile\nruns: 1\n";
    let profile = open_profile(source).expect("open_profile returns any integer format verbatim");
    assert_eq!(profile.format, 2);
}

// --- Envelope (L1121) -------------------------------------------------------------------------

#[test]
fn missing_format_is_l1121() {
    let source = "kind: profile\nruns: 1\n";
    let diagnostics = open_profile(source).unwrap_err();
    let d = diagnostics
        .iter()
        .find(|d| d.message.contains("format"))
        .expect("a diagnostic about the missing `format`");
    assert_eq!(d.code.as_str(), "L1121");
    assert_eq!(d.path.as_deref(), Some(""));
}

#[test]
fn format_wrong_type_is_l1121_with_pointer() {
    let source = "format: \"1\"\nkind: profile\nruns: 1\n";
    let diagnostics = open_profile(source).unwrap_err();
    let d = diagnostics
        .iter()
        .find(|d| d.path.as_deref() == Some("/format"))
        .expect("a /format diagnostic");
    assert_eq!(d.code.as_str(), "L1121");
}

#[test]
fn missing_kind_is_l1121() {
    let source = "format: 1\nruns: 1\n";
    let diagnostics = open_profile(source).unwrap_err();
    let d = diagnostics
        .iter()
        .find(|d| d.message.contains("kind"))
        .expect("a diagnostic about the missing `kind`");
    assert_eq!(d.code.as_str(), "L1121");
    assert_eq!(d.path.as_deref(), Some(""));
}

#[test]
fn wrong_kind_is_l1121_with_pointer() {
    // A `kind: weapon` file loaded as a profile.
    let source = "format: 1\nkind: weapon\nruns: 1\n";
    let diagnostics = open_profile(source).unwrap_err();
    assert_eq!(diagnostics.len(), 1, "diagnostics: {diagnostics:?}");
    let d = &diagnostics[0];
    assert_eq!(d.code.as_str(), "L1121");
    assert_eq!(d.path.as_deref(), Some("/kind"));
    assert!(d.message.contains("weapon"), "message: {}", d.message);
}

#[test]
fn kind_wrong_type_is_l1121() {
    let source = "format: 1\nkind: 5\nruns: 1\n";
    let diagnostics = open_profile(source).unwrap_err();
    let d = diagnostics
        .iter()
        .find(|d| d.path.as_deref() == Some("/kind"))
        .expect("a /kind diagnostic");
    assert_eq!(d.code.as_str(), "L1121");
}

#[test]
fn non_mapping_root_is_l1121() {
    let source = "- format\n- kind\n";
    let diagnostics = open_profile(source).unwrap_err();
    assert_eq!(diagnostics.len(), 1, "diagnostics: {diagnostics:?}");
    assert_eq!(diagnostics[0].code.as_str(), "L1121");
    assert_eq!(diagnostics[0].path.as_deref(), Some(""));
}

#[test]
fn empty_document_is_l1121() {
    let diagnostics = open_profile("").unwrap_err();
    assert_eq!(diagnostics.len(), 1);
    assert_eq!(diagnostics[0].code.as_str(), "L1121");
}

// --- Strict-YAML (L100x) ----------------------------------------------------------------------

#[test]
fn yaml_anchor_surfaces_l100x_not_l1121() {
    let source = "format: 1\nkind: profile\nlast_area: &a caves\nruns: 1\n";
    let diagnostics = open_profile(source).unwrap_err();
    assert_eq!(diagnostics.len(), 1);
    let code = diagnostics[0].code.as_str();
    assert!(
        code.starts_with("L100"),
        "expected an L100x code, got {code}"
    );
    assert_ne!(code, "L1121");
}

#[test]
fn a_tab_surfaces_a_yaml_syntax_error() {
    let source = "format: 1\nkind: profile\nruns:\n\t- 1\n";
    let diagnostics = open_profile(source).unwrap_err();
    let code = diagnostics[0].code.as_str();
    assert!(
        code.starts_with("L100"),
        "expected an L100x code, got {code}"
    );
}

// --- `ludion explain` -------------------------------------------------------------------------

#[test]
fn the_new_code_explains() {
    let l1121 = ludion_core::codes::explain("L1121").expect("L1121 is registered");
    assert!(l1121.title.contains("profile"), "title: {}", l1121.title);
    assert!(l1121.explanation.contains("backup"));
    assert!(!l1121.explanation.is_empty());
}

// --- Byte-idempotence proptest ----------------------------------------------------------------

fn finite_f64() -> impl Strategy<Value = f64> {
    any::<f64>().prop_filter("finite", |f| f.is_finite())
}

/// A value that conforms to [`profile_schema`]: the two required fields always present, the optional
/// ones sometimes, every value in its declared type. Float fields exercise ryu emission.
fn profile_value() -> impl Strategy<Value = Value> {
    (
        any::<bool>(),
        any::<u64>(),
        proptest::option::of(finite_f64()),
        proptest::option::of(any::<String>()),
    )
        .prop_map(|(unlocked, runs, best_time, last_area)| {
            let mut map = Map::new();
            map.insert("unlocked_crossbow".into(), Value::Bool(unlocked));
            map.insert("runs".into(), Value::from(runs));
            if let Some(best_time) = best_time {
                map.insert("best_time".into(), Value::from(best_time));
            }
            if let Some(last_area) = last_area {
                map.insert("last_area".into(), Value::String(last_area));
            }
            Value::Object(map)
        })
}

proptest! {
    #![proptest_config(deterministic_config())]
    #[test]
    fn emit_open_emit_is_byte_idempotent(value in profile_value()) {
        let schema = profile_schema();
        let first = emit_doc("profile", &value, &schema);
        let reopened = open_profile(&first).expect("canonical output must open clean");
        let second = emit_doc("profile", &reopened.fields, &schema);
        prop_assert_eq!(&second, &first);
        // And once stable, it stays stable.
        let third = emit_doc("profile", &open_profile(&second).unwrap().fields, &schema);
        prop_assert_eq!(&third, &first);
    }

    #[test]
    fn emitted_text_is_lf_only(value in profile_value()) {
        let text = emit_doc("profile", &value, &profile_schema());
        prop_assert!(!text.contains('\r'), "canonical output must use LF only");
        prop_assert!(text.ends_with('\n'), "canonical output ends with a newline");
        prop_assert!(text.starts_with("format: 1\nkind: profile\n"), "envelope first: {text:?}");
    }
}
