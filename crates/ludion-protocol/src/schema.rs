//! The machine-readable protocol schema and its stable [`schema_hash`].
//!
//! Every wire type in this crate derives [`schemars::JsonSchema`]; [`protocol_schema`] assembles them
//! into one deterministic JSON document, and [`schema_hash`] is the XXH3-64 of its canonical bytes.
//! The hash is announced in the [handshake](crate::handshake::ServerHello) so a client generated from
//! a stale schema detects the drift at connect time, and CI regenerates the document to fail on any
//! uncommitted change (engine-architecture.md §6, risk register #4).
//!
//! Determinism: `serde_json`'s `preserve_order` feature keeps object keys in insertion order, but the
//! document is additionally **canonicalized** — every object's keys are sorted recursively (array
//! order, which is semantic in JSON Schema, is preserved) — so the exact bytes do not depend on the
//! order `schemars` happens to emit definitions in. [`schema_document`] is that canonical string, and
//! it is what [`schema_hash`] hashes and what the committed snapshot test compares against.

use schemars::JsonSchema;
use serde_json::{Map, Value};

use crate::WireHash;
use crate::command::Command;
use crate::envelope::{EventMessage, Request, Response};
use crate::handshake::{Hello, ServerHello};
use crate::response::ResponseBody;

/// Builds the full protocol schema as one canonical JSON value.
///
/// The document names the protocol version and the JSON Schema of every wire type: the request /
/// response / event envelopes, the [`Command`] / [`ResponseBody`] / [`Event`](crate::Event) bodies,
/// the handshake messages, and the binary frame header.
///
/// # Example
/// ```
/// let schema = ludion_protocol::schema::protocol_schema();
/// assert_eq!(schema["protocol_version"], serde_json::json!(1));
/// assert!(schema["command"].is_object());
/// ```
pub fn protocol_schema() -> Value {
    let mut root = Map::new();
    root.insert(
        "protocol_version".into(),
        Value::from(crate::PROTOCOL_VERSION),
    );
    root.insert("request".into(), schema_of::<Request>());
    root.insert("response".into(), schema_of::<Response>());
    root.insert("event".into(), schema_of::<EventMessage>());
    root.insert("command".into(), schema_of::<Command>());
    root.insert("response_body".into(), schema_of::<ResponseBody>());
    root.insert("hello".into(), schema_of::<Hello>());
    root.insert("server_hello".into(), schema_of::<ServerHello>());
    root.insert(
        "frame_header".into(),
        schema_of::<crate::frame::FrameHeader>(),
    );
    canonicalize(Value::Object(root))
}

/// The canonical protocol schema as a pretty-printed, newline-terminated string.
///
/// This exact byte string is what [`schema_hash`] hashes and what the committed snapshot test
/// asserts against, so the two can never disagree.
pub fn schema_document() -> String {
    let mut s = serde_json::to_string_pretty(&protocol_schema())
        .expect("the protocol schema always serializes");
    s.push('\n');
    s
}

/// The XXH3-64 of the canonical [`schema_document`] bytes, as a 16-hex [`WireHash`].
///
/// # Example
/// ```
/// let a = ludion_protocol::schema::schema_hash();
/// let b = ludion_protocol::schema::schema_hash();
/// assert_eq!(a, b); // stable across calls
/// assert_eq!(a.to_string().len(), 16);
/// ```
pub fn schema_hash() -> WireHash {
    WireHash::of_bytes(schema_document().as_bytes())
}

/// Removes the prose keywords `description` and `title` from a JSON Schema value, recursively.
///
/// Only schema keywords go: under `properties`, `$defs`, `definitions`, `patternProperties` and
/// `dependentSchemas` the keys are *names* (a parameter called `description` survives), and the
/// instance data under `default`, `examples`, `const` and `enum` is never touched.
pub fn strip_prose(value: Value) -> Value {
    const NAME_MAPS: [&str; 5] = [
        "properties",
        "$defs",
        "definitions",
        "patternProperties",
        "dependentSchemas",
    ];
    const DATA: [&str; 4] = ["default", "examples", "const", "enum"];
    match value {
        Value::Object(map) => Value::Object(
            map.into_iter()
                .filter(|(k, _)| k != "description" && k != "title")
                .map(|(k, v)| {
                    let v = if DATA.contains(&k.as_str()) {
                        v
                    } else if NAME_MAPS.contains(&k.as_str()) {
                        match v {
                            Value::Object(names) => Value::Object(
                                names
                                    .into_iter()
                                    .map(|(n, s)| (n, strip_prose(s)))
                                    .collect(),
                            ),
                            other => other,
                        }
                    } else {
                        strip_prose(v)
                    };
                    (k, v)
                })
                .collect(),
        ),
        Value::Array(items) => Value::Array(items.into_iter().map(strip_prose).collect()),
        other => other,
    }
}

/// The XXH3-64 of a value's canonical (key-sorted, compact) JSON bytes.
pub fn canonical_hash(value: &Value) -> WireHash {
    let bytes =
        serde_json::to_vec(&canonicalize(value.clone())).expect("a JSON value always serializes");
    WireHash::of_bytes(&bytes)
}

/// The hash of the [`protocol_schema`] with its prose stripped: a doc-comment edit does not move it,
/// a new field, command or response shape does. What a replay header records (`schema_hash`).
///
/// # Example
/// ```
/// let a = ludion_protocol::schema::replay_schema_hash();
/// assert_eq!(a, ludion_protocol::schema::replay_schema_hash());
/// ```
pub fn replay_schema_hash() -> WireHash {
    canonical_hash(&strip_prose(protocol_schema()))
}

/// The canonical JSON Schema of a [`ReplayRecord`](crate::replay::ReplayRecord), the wire half of a
/// replay's content.
pub fn replay_record_schema() -> Value {
    canonicalize(schema_of::<crate::replay::ReplayRecord>())
}

/// The JSON Schema of `T` as a value.
pub fn schema_of<T: JsonSchema>() -> Value {
    serde_json::to_value(schemars::schema_for!(T))
        .expect("a schemars JSON Schema always serializes to a JSON value")
}

/// Recursively sorts every object's keys (arrays keep their order). This makes the serialized bytes
/// independent of the order `schemars` emits keys/definitions in, so the hash and snapshot are stable.
fn canonicalize(value: Value) -> Value {
    match value {
        Value::Object(map) => {
            let mut entries: Vec<(String, Value)> =
                map.into_iter().map(|(k, v)| (k, canonicalize(v))).collect();
            entries.sort_by(|a, b| a.0.cmp(&b.0));
            Value::Object(entries.into_iter().collect())
        }
        Value::Array(items) => Value::Array(items.into_iter().map(canonicalize).collect()),
        other => other,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn document_is_canonical_and_stable() {
        let a = schema_document();
        let b = schema_document();
        assert_eq!(a, b);

        // Every object's keys are sorted (canonical form).
        fn keys_sorted(v: &Value) -> bool {
            match v {
                Value::Object(map) => {
                    let keys: Vec<&String> = map.keys().collect();
                    let mut sorted = keys.clone();
                    sorted.sort();
                    keys == sorted && map.values().all(keys_sorted)
                }
                Value::Array(items) => items.iter().all(keys_sorted),
                _ => true,
            }
        }
        assert!(keys_sorted(&protocol_schema()));
    }

    #[test]
    fn hash_matches_the_document_bytes() {
        assert_eq!(
            schema_hash(),
            WireHash::of_bytes(schema_document().as_bytes())
        );
    }

    #[test]
    fn document_names_every_section() {
        let schema = protocol_schema();
        for section in [
            "protocol_version",
            "request",
            "response",
            "event",
            "command",
            "response_body",
            "hello",
            "server_hello",
            "frame_header",
        ] {
            assert!(schema.get(section).is_some(), "missing section {section}");
        }
    }

    #[test]
    fn strip_prose_keeps_names_and_data() {
        let schema = serde_json::json!({
            "title": "T", "description": "doc",
            "properties": {
                "description": {"description": "a param named description", "type": "string"},
                "title": {"type": "string", "default": {"description": "kept"}}
            }
        });
        let stripped = strip_prose(schema);
        assert_eq!(
            stripped,
            serde_json::json!({
                "properties": {
                    "description": {"type": "string"},
                    "title": {"type": "string", "default": {"description": "kept"}}
                }
            })
        );
    }

    #[test]
    fn doc_only_difference_hashes_equal_and_new_property_differs() {
        let a = serde_json::json!({"properties": {"x": {"type": "u32", "description": "one"}}});
        let b = serde_json::json!({"properties": {"x": {"type": "u32", "description": "two"}}});
        let c = serde_json::json!({"properties": {"x": {"type": "u32"}, "y": {"type": "u32"}}});
        let h = |v: &Value| canonical_hash(&strip_prose(v.clone()));
        assert_eq!(h(&a), h(&b));
        assert_ne!(h(&a), h(&c));
    }

    #[test]
    fn full_hash_still_sees_descriptions() {
        assert!(schema_document().contains("\"description\""));
        assert_eq!(replay_schema_hash(), replay_schema_hash());
    }

    #[test]
    fn every_params_type_denies_unknown_fields() {
        let schema = schema_of::<Command>();
        let defs = schema["$defs"]
            .as_object()
            .expect("command schema has $defs");
        let strict = |def: &Value| def["additionalProperties"] == Value::Bool(false);
        let mut checked = 0;
        for (name, def) in defs {
            if name == "SubscribeMode" {
                let members = def["oneOf"].as_array().expect("SubscribeMode is a oneOf");
                assert!(
                    members.iter().all(strict),
                    "SubscribeMode member accepts unknown fields"
                );
            } else if name.ends_with("Params") || name == "Placement" || name == "ViewPose" {
                assert!(
                    strict(def),
                    "{name} must carry #[serde(deny_unknown_fields)]"
                );
            } else {
                continue;
            }
            checked += 1;
        }
        assert!(checked >= 33, "expected every params type, saw {checked}");
    }
}
