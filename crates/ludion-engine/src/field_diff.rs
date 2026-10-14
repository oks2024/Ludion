//! Shared object-key diffing, used by both migration reports the engine produces: a restored
//! entity's component drift ([`snapshot`](crate::snapshot)) and a loaded player profile's field drift
//! ([`player_profile`](crate::player_profile)). Both compare a snapshot/file value against its
//! reserialized-from-the-live-type form to report which fields serde filled from defaults (the type
//! grew) and which it dropped (the type shrank) — the same "compare before/after keys" technique, one
//! owner. Nested objects are walked and reported as dotted paths (`inner.b`).

use serde_json::Value;

/// Diffs the object keys of `before` against `after`, recursing into keys both sides hold as objects,
/// and returns `(filled, dropped)` as dotted paths — present only in `after` (filled from serde
/// defaults) and present only in `before` (dropped from the type). Both sorted. Arrays and scalars are
/// leaves: an array of objects is not descended, and a non-object root (e.g. a newtype that serializes
/// to a scalar) has no fields, so both lists come back empty.
pub(crate) fn diff_fields(before: &Value, after: &Value) -> (Vec<String>, Vec<String>) {
    let mut filled = Vec::new();
    let mut dropped = Vec::new();
    collect(before, after, "", &mut filled, &mut dropped);
    filled.sort();
    dropped.sort();
    (filled, dropped)
}

fn collect(
    before: &Value,
    after: &Value,
    prefix: &str,
    filled: &mut Vec<String>,
    dropped: &mut Vec<String>,
) {
    let (Some(before), Some(after)) = (before.as_object(), after.as_object()) else {
        return;
    };
    for (key, after_value) in after {
        match before.get(key) {
            None => filled.push(format!("{prefix}{key}")),
            Some(before_value) => {
                collect(
                    before_value,
                    after_value,
                    &format!("{prefix}{key}."),
                    filled,
                    dropped,
                );
            }
        }
    }
    for key in before.keys().filter(|key| !after.contains_key(*key)) {
        dropped.push(format!("{prefix}{key}"));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn diff(before: Value, after: Value) -> (Vec<String>, Vec<String>) {
        diff_fields(&before, &after)
    }

    #[test]
    fn top_level_fill_and_drop() {
        let (filled, dropped) = diff(json!({"a": 1, "old": 2}), json!({"a": 1, "new": 3}));
        assert_eq!(filled, ["new"]);
        assert_eq!(dropped, ["old"]);
    }

    #[test]
    fn nested_fill_and_drop_report_dotted_paths() {
        let (filled, dropped) = diff(
            json!({"inner": {"a": 1, "legacy": 2}}),
            json!({"inner": {"a": 1, "b": 3}}),
        );
        assert_eq!(filled, ["inner.b"]);
        assert_eq!(dropped, ["inner.legacy"]);
    }

    #[test]
    fn two_levels_deep() {
        let (filled, dropped) = diff(
            json!({"a": {"b": {"c": 1}}}),
            json!({"a": {"b": {"c": 1, "d": 2}}}),
        );
        assert_eq!(filled, ["a.b.d"]);
        assert!(dropped.is_empty());
    }

    #[test]
    fn arrays_of_objects_are_leaves() {
        let (filled, dropped) = diff(json!({"v": [{"x": 1}]}), json!({"v": [{"x": 1, "y": 2}]}));
        assert!(filled.is_empty() && dropped.is_empty());
    }

    #[test]
    fn identical_nested_is_empty() {
        let value = json!({"a": {"b": 1}, "c": 2});
        let (filled, dropped) = diff(value.clone(), value);
        assert!(filled.is_empty() && dropped.is_empty());
    }

    #[test]
    fn scalar_root_is_empty() {
        let (filled, dropped) = diff(json!(1), json!(2));
        assert!(filled.is_empty() && dropped.is_empty());
    }
}
