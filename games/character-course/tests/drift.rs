//! Drift-safety test: every component in this game's registry must be able to build from an empty
//! JSON object `{}` — the property snapshot-restore migration depends on across a `ludion dev`
//! reload. See "Keep components drift-safe" in `docs/components.md`.

use ludion_testkit::assert_components_drift_safe;

#[test]
fn registry_is_drift_safe() {
    assert_components_drift_safe(&character_course::registry());
}

#[test]
fn committed_scenes_respell_nothing_on_save() {
    ludion_testkit::assert_saves_respell_nothing(
        &std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("scenes"),
        &["."],
        character_course::game(),
    );
}

#[test]
fn a_camera_zone_with_a_negative_half_life_is_l1304() {
    let registry = character_course::registry();
    let zone = registry.get("CameraGate").expect("registered");
    let found = zone.validate_semantics(&serde_json::json!({
        "rig": {"springs": {"position_half_life": -1.0}}
    }));
    assert_eq!(found.len(), 1, "{found:?}");
    assert_eq!(found[0].code.as_str(), "L1304");
    assert_eq!(
        found[0].path.as_deref(),
        Some("/rig/springs/position_half_life")
    );
    assert!(zone.validate_semantics(&serde_json::json!({})).is_empty());
}
