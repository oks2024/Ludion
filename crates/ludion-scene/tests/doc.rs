//! Generic game data-doc machinery (`check_doc`/`emit_doc`, Phase 15 / E4 Wave 1) — the doc analog
//! of `roundtrip.rs`/`mesh_roundtrip.rs`:
//!
//! - **envelope** (`format`/`kind`) violations are `L1119` with a JSON Pointer;
//! - **schema** mismatches (wrong type, out of range, typo'd field) are `L1120` with a `/field`
//!   pointer and a nearest-match suggestion for a typo;
//! - a **strict-YAML** syntax/strictness error surfaces the shared `L100x` codes, not `L1119`;
//! - **byte-idempotence**: `emit_doc → check_doc → emit_doc` is byte-equal (float fields exercise
//!   ryu-shortest emission);
//! - the two new codes explain (`ludion explain L1119`/`L1120`).

use ludion_scene::{check_doc, deserialize_doc, emit_doc, parse_doc};
use proptest::prelude::*;
use schemars::JsonSchema;
use serde::Deserialize;
#[path = "support/deterministic.rs"]
mod deterministic;
use deterministic::deterministic_config;
use serde_json::{Map, Value, json};

/// A `weapon` kind's schema — the shape `schema_for!(WeaponDoc)` would produce for a struct with
/// two required and several optional fields, one of them range-bounded and one an array. Shared by
/// the unit tests and the round-trip proptest so both exercise the same fields.
fn weapon_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "name":     { "type": "string" },
            "damage":   { "type": "number" },
            "cooldown": { "type": "number" },
            "heat":     { "type": "number", "minimum": 0.0, "maximum": 100.0 },
            "ammo":     { "type": "integer" },
            "piercing": { "type": "boolean" },
            "tags":     { "type": "array", "items": { "type": "string" } }
        },
        "required": ["name", "damage"]
    })
}

// --- Happy path -------------------------------------------------------------------------------

#[test]
fn valid_doc_strips_the_envelope() {
    let source = "\
format: 1
kind: weapon
name: crossbow
damage: 12.0
cooldown: 0.8
";
    let fields = check_doc(source, "weapon", &weapon_schema()).expect("valid weapon doc");
    // The envelope keys are gone; the type's own fields remain.
    assert!(fields.get("format").is_none());
    assert!(fields.get("kind").is_none());
    assert_eq!(fields["name"], json!("crossbow"));
    assert_eq!(fields["damage"], json!(12.0));
    assert_eq!(fields["cooldown"], json!(0.8));
}

// --- Envelope (L1119) -------------------------------------------------------------------------

#[test]
fn missing_format_is_l1119() {
    let source = "kind: weapon\nname: crossbow\ndamage: 12.0\n";
    let diagnostics = check_doc(source, "weapon", &weapon_schema()).unwrap_err();
    let d = &diagnostics[0];
    assert_eq!(d.code.as_str(), "L1119");
    assert_eq!(d.path.as_deref(), Some(""));
    assert!(d.message.contains("weapon"), "message: {}", d.message);
}

#[test]
fn wrong_format_version_is_l1119_with_pointer() {
    let source = "format: 2\nkind: weapon\nname: crossbow\ndamage: 12.0\n";
    let diagnostics = check_doc(source, "weapon", &weapon_schema()).unwrap_err();
    let d = diagnostics
        .iter()
        .find(|d| d.path.as_deref() == Some("/format"))
        .expect("a /format diagnostic");
    assert_eq!(d.code.as_str(), "L1119");
}

#[test]
fn format_wrong_type_is_l1119() {
    let source = "format: \"1\"\nkind: weapon\nname: crossbow\ndamage: 12.0\n";
    let diagnostics = check_doc(source, "weapon", &weapon_schema()).unwrap_err();
    let d = diagnostics
        .iter()
        .find(|d| d.path.as_deref() == Some("/format"))
        .expect("a /format diagnostic");
    assert_eq!(d.code.as_str(), "L1119");
}

#[test]
fn missing_kind_is_l1119() {
    let source = "format: 1\nname: crossbow\ndamage: 12.0\n";
    let diagnostics = check_doc(source, "weapon", &weapon_schema()).unwrap_err();
    let d = diagnostics
        .iter()
        .find(|d| d.code.as_str() == "L1119")
        .expect("an L1119 diagnostic");
    assert_eq!(d.path.as_deref(), Some(""));
    assert!(d.message.contains("kind"), "message: {}", d.message);
}

#[test]
fn kind_mismatch_is_l1119_with_pointer() {
    // A `kind: enemy` file loaded as a `weapon`.
    let source = "format: 1\nkind: enemy\nname: crossbow\ndamage: 12.0\n";
    let diagnostics = check_doc(source, "weapon", &weapon_schema()).unwrap_err();
    let d = &diagnostics[0];
    assert_eq!(d.code.as_str(), "L1119");
    assert_eq!(d.path.as_deref(), Some("/kind"));
    assert!(
        d.message.contains("enemy") && d.message.contains("weapon"),
        "message: {}",
        d.message
    );
}

#[test]
fn kind_wrong_type_is_l1119() {
    let source = "format: 1\nkind: 5\nname: crossbow\ndamage: 12.0\n";
    let diagnostics = check_doc(source, "weapon", &weapon_schema()).unwrap_err();
    let d = diagnostics
        .iter()
        .find(|d| d.path.as_deref() == Some("/kind"))
        .expect("a /kind diagnostic");
    assert_eq!(d.code.as_str(), "L1119");
}

#[test]
fn unknown_envelope_key_is_l1119_with_nearest_match() {
    // A near-miss of `format` (envelope), alongside a valid envelope, is an envelope typo — not a
    // schema-field typo — so it reports L1119, not L1120.
    let source = "\
format: 1
kind: weapon
fromat: 3
name: crossbow
damage: 12.0
";
    let diagnostics = check_doc(source, "weapon", &weapon_schema()).unwrap_err();
    assert_eq!(diagnostics.len(), 1, "diagnostics: {diagnostics:?}");
    let d = &diagnostics[0];
    assert_eq!(d.code.as_str(), "L1119");
    assert_eq!(d.path.as_deref(), Some("/fromat"));
    assert_eq!(d.help.as_deref(), Some("nearest match is `format`"));
}

// --- Schema (L1120) ---------------------------------------------------------------------------

#[test]
fn wrong_type_field_is_l1120_with_pointer() {
    let source = "format: 1\nkind: weapon\nname: crossbow\ndamage: fast\n";
    let diagnostics = check_doc(source, "weapon", &weapon_schema()).unwrap_err();
    let d = diagnostics
        .iter()
        .find(|d| d.path.as_deref() == Some("/damage"))
        .expect("a /damage diagnostic");
    assert_eq!(d.code.as_str(), "L1120");
    assert!(d.message.contains("weapon"), "message: {}", d.message);
}

#[test]
fn out_of_range_field_is_l1120() {
    let source = "format: 1\nkind: weapon\nname: crossbow\ndamage: 12.0\nheat: 999.0\n";
    let diagnostics = check_doc(source, "weapon", &weapon_schema()).unwrap_err();
    let d = diagnostics
        .iter()
        .find(|d| d.path.as_deref() == Some("/heat"))
        .expect("a /heat diagnostic");
    assert_eq!(d.code.as_str(), "L1120");
}

#[test]
fn typo_field_is_l1120_with_nearest_match() {
    // A typo of the *optional* `cooldown` — its nearest match is a schema field, so it is a
    // field-level L1120, and `name`/`damage` still being present keeps it the only diagnostic.
    let source = "format: 1\nkind: weapon\nname: crossbow\ndamage: 12.0\ncooldwn: 0.5\n";
    let diagnostics = check_doc(source, "weapon", &weapon_schema()).unwrap_err();
    assert_eq!(diagnostics.len(), 1, "diagnostics: {diagnostics:?}");
    let d = &diagnostics[0];
    assert_eq!(d.code.as_str(), "L1120");
    assert_eq!(d.path.as_deref(), Some("/cooldwn"));
    assert_eq!(d.help.as_deref(), Some("nearest match is `cooldown`"));
}

#[test]
fn missing_required_field_is_l1120() {
    let source = "format: 1\nkind: weapon\nname: crossbow\n";
    let diagnostics = check_doc(source, "weapon", &weapon_schema()).unwrap_err();
    let d = diagnostics
        .iter()
        .find(|d| d.message.contains("damage"))
        .expect("a diagnostic about the missing `damage`");
    assert_eq!(d.code.as_str(), "L1120");
}

// --- Strict-YAML (L100x) ----------------------------------------------------------------------

#[test]
fn yaml_anchor_surfaces_l100x_not_l1119() {
    let source = "format: 1\nkind: weapon\nname: &a crossbow\ndamage: 12.0\n";
    let diagnostics = check_doc(source, "weapon", &weapon_schema()).unwrap_err();
    assert_eq!(diagnostics.len(), 1);
    let code = diagnostics[0].code.as_str();
    assert!(
        code.starts_with("L100"),
        "expected an L100x code, got {code}"
    );
    assert_ne!(code, "L1119");
}

#[test]
fn a_tab_surfaces_a_yaml_syntax_error() {
    // A tab in indentation is a YAML syntax error, surfaced as L1001, not a panic or L1119.
    let source = "format: 1\nkind: weapon\nname:\n\t- x\ndamage: 12.0\n";
    let diagnostics = check_doc(source, "weapon", &weapon_schema()).unwrap_err();
    let code = diagnostics[0].code.as_str();
    assert!(
        code.starts_with("L100"),
        "expected an L100x code, got {code}"
    );
}

// --- `ludion explain` -------------------------------------------------------------------------

#[test]
fn the_new_codes_explain() {
    let l1119 = ludion_core::codes::explain("L1119").expect("L1119 is registered");
    assert!(l1119.title.contains("envelope"), "title: {}", l1119.title);
    assert!(!l1119.explanation.is_empty());

    let l1120 = ludion_core::codes::explain("L1120").expect("L1120 is registered");
    assert!(l1120.title.contains("schema"), "title: {}", l1120.title);
    assert!(!l1120.explanation.is_empty());
}

// --- Byte-idempotence proptest ----------------------------------------------------------------

fn finite_f64() -> impl Strategy<Value = f64> {
    any::<f64>().prop_filter("finite", |f| f.is_finite())
}

/// A value that conforms to [`weapon_schema`]: the two required fields always present, the optional
/// ones sometimes, every value in its declared type and range. Float fields exercise ryu emission.
fn weapon_value() -> impl Strategy<Value = Value> {
    (
        any::<String>(),
        finite_f64(),
        proptest::option::of(finite_f64()),
        proptest::option::of(0.0f64..=100.0f64),
        proptest::option::of(any::<i64>()),
        proptest::option::of(any::<bool>()),
        proptest::option::of(proptest::collection::vec(any::<String>(), 0..4)),
    )
        .prop_map(|(name, damage, cooldown, heat, ammo, piercing, tags)| {
            let mut map = Map::new();
            map.insert("name".into(), Value::String(name));
            map.insert("damage".into(), Value::from(damage));
            if let Some(cooldown) = cooldown {
                map.insert("cooldown".into(), Value::from(cooldown));
            }
            if let Some(heat) = heat {
                map.insert("heat".into(), Value::from(heat));
            }
            if let Some(ammo) = ammo {
                map.insert("ammo".into(), Value::from(ammo));
            }
            if let Some(piercing) = piercing {
                map.insert("piercing".into(), Value::Bool(piercing));
            }
            if let Some(tags) = tags {
                map.insert(
                    "tags".into(),
                    Value::Array(tags.into_iter().map(Value::String).collect()),
                );
            }
            Value::Object(map)
        })
}

proptest! {
    #![proptest_config(deterministic_config())]
    #[test]
    fn emit_check_emit_is_byte_idempotent(value in weapon_value()) {
        let schema = weapon_schema();
        let first = emit_doc("weapon", &value, &schema);
        let checked = check_doc(&first, "weapon", &schema)
            .expect("canonical output must check clean");
        let second = emit_doc("weapon", &checked, &schema);
        prop_assert_eq!(&second, &first);
        // And once stable, it stays stable.
        let third = emit_doc("weapon", &check_doc(&second, "weapon", &schema).unwrap(), &schema);
        prop_assert_eq!(&third, &first);
    }

    #[test]
    fn emitted_text_is_lf_only(value in weapon_value()) {
        let text = emit_doc("weapon", &value, &weapon_schema());
        prop_assert!(!text.contains('\r'), "canonical output must use LF only");
        prop_assert!(text.ends_with('\n'), "canonical output ends with a newline");
        prop_assert!(text.starts_with("format: 1\nkind: weapon\n"), "envelope first: {text:?}");
    }
}

// --- Registered-type envelope collisions (L1257) -----------------------------------------------

#[test]
fn top_level_envelope_field_is_l1257() {
    for field in ["kind", "format"] {
        let schema = json!({ "type": "object", "properties": { field: { "type": "string" } } });
        let d = ludion_scene::check_doc_type("weapon", &schema).unwrap_err();
        assert_eq!(d.code.as_str(), "L1257");
        assert!(d.message.contains(&format!("`{field}`")), "{}", d.message);
    }
}

#[test]
fn nested_envelope_field_is_accepted() {
    let schema = json!({
        "type": "object",
        "properties": { "stats": { "type": "object", "properties": { "kind": { "type": "string" } } } }
    });
    assert!(ludion_scene::check_doc_type("weapon", &schema).is_ok());
    assert!(ludion_scene::check_doc_type("weapon", &weapon_schema()).is_ok());
}

#[test]
fn all_of_part_and_one_of_branch_are_searched() {
    let all_of = json!({
        "type": "object",
        "properties": { "name": { "type": "string" } },
        "allOf": [{ "properties": { "kind": { "type": "string" } } }]
    });
    assert_eq!(
        ludion_scene::check_doc_type("weapon", &all_of)
            .unwrap_err()
            .code
            .as_str(),
        "L1257"
    );
    let one_of = json!({
        "oneOf": [
            { "type": "object", "properties": { "kind": { "const": "a" } } },
            { "type": "object", "properties": { "kind": { "const": "b" } } }
        ]
    });
    assert_eq!(
        ludion_scene::check_doc_type("weapon", &one_of)
            .unwrap_err()
            .code
            .as_str(),
        "L1257"
    );
}

#[cfg(debug_assertions)]
#[test]
#[should_panic(expected = "duplicate the envelope")]
fn emit_doc_refuses_an_envelope_key_in_debug_builds() {
    emit_doc("weapon", &json!({ "kind": "x" }), &weapon_schema());
}

// --- Typed reader (`parse_doc`) -----------------------------------------------------------------

#[derive(Debug, PartialEq, Deserialize, JsonSchema)]
struct Weapon {
    name: String,
    damage: f32,
}

#[test]
fn parse_doc_reads_a_valid_doc_into_its_type() {
    let source = "format: 1\nkind: weapon\nname: crossbow\ndamage: 12.0\n";
    let weapon: Weapon = parse_doc(source, "weapon").expect("valid weapon doc");
    assert_eq!(
        weapon,
        Weapon {
            name: "crossbow".into(),
            damage: 12.0
        }
    );
}

#[test]
fn parse_doc_wrong_kind_is_l1119() {
    let source = "format: 1\nkind: armor\nname: crossbow\ndamage: 12.0\n";
    let errors = parse_doc::<Weapon>(source, "weapon").unwrap_err();
    assert_eq!(errors[0].code.as_str(), "L1119");
}

#[test]
fn parse_doc_wrong_typed_field_is_l1120_with_pointer() {
    let source = "format: 1\nkind: weapon\nname: crossbow\ndamage: fast\n";
    let errors = parse_doc::<Weapon>(source, "weapon").unwrap_err();
    assert_eq!(errors[0].code.as_str(), "L1120");
    assert_eq!(errors[0].path.as_deref(), Some("/damage"));
}

#[test]
fn parse_doc_refuses_a_type_with_a_top_level_kind_field() {
    #[derive(Debug, Deserialize, JsonSchema)]
    struct Tagged {
        #[allow(dead_code)]
        kind: String,
    }
    let errors = parse_doc::<Tagged>("format: 1\nkind: tagged\n", "tagged").unwrap_err();
    assert_eq!(errors[0].code.as_str(), "L1257");
}

#[test]
fn deserialize_doc_failure_is_one_l1120_with_the_shared_help() {
    let error = deserialize_doc::<Weapon>(json!({ "damage": "x" })).unwrap_err();
    assert_eq!(error.code.as_str(), "L1120");
    assert!(
        error
            .message
            .starts_with("passed schema validation but could not be deserialized: "),
        "{}",
        error.message
    );
    assert!(
        error
            .help
            .as_deref()
            .is_some_and(|help| help.contains("engine/schema mismatch")),
        "{:?}",
        error.help
    );
}
