//! Drift-safety test: every component in this game's registry must be able to build from an empty
//! JSON object `{}` — the property snapshot-restore migration depends on across a `ludion dev`
//! reload. See "Keep components drift-safe" in `docs/components.md`.

use ludion_testkit::assert_components_drift_safe;

#[test]
fn registry_is_drift_safe() {
    assert_components_drift_safe(&breakout::registry());
}

#[test]
fn committed_scenes_respell_nothing_on_save() {
    ludion_testkit::assert_saves_respell_nothing(
        &std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("scenes"),
        &["."],
        breakout::game(),
    );
}
