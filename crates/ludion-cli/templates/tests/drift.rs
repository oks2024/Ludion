//! Drift-safety test: every component in this game's registry must be able to build from an empty
//! JSON object `{}` — the property snapshot-restore migration depends on across a `ludion dev`
//! reload (a restarted binary restores a snapshot taken against the *old* code). See "Keep components
//! drift-safe" in `LUDION-GUIDE.md`. The second test checks that every entity reference a component
//! holds is marked (`#[schemars(with = "EntityRefSchema")]`), so scene load and save can see it.

use ludion_testkit::{assert_components_drift_safe, assert_entity_refs_marked};

#[test]
fn registry_is_drift_safe() {
    assert_components_drift_safe(&{{name_ident}}::registry());
}

#[test]
fn registry_marks_every_entity_reference() {
    assert_entity_refs_marked(&{{name_ident}}::registry());
}
