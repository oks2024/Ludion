//! Pins `schema_for!(PatternDoc)` to a committed snapshot.
//!
//! The editor's bundled `patternSchema.ts` is generated from this snapshot, so a `PatternDoc`
//! change that is not re-blessed fails here instead of drifting silently. To re-bless:
//!
//! ```text
//! BLESS_SCHEMA=1 cargo test -p ludion-engine --test pattern_schema_snapshot
//! ```
//!
//! then `npm run generate` in `editor/protocol` and commit both files.

/// The committed pretty-printed schema.
const SNAPSHOT: &str = include_str!("snapshots/pattern_schema.json");

#[test]
fn pattern_schema_matches_committed_snapshot() {
    let mut document =
        serde_json::to_string_pretty(&schemars::schema_for!(ludion_engine::PatternDoc))
            .expect("serialize pattern schema");
    document.push('\n');

    if ludion_core::bless::switch_set(ludion_core::bless::BLESS_SCHEMA_ENV) {
        let path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/snapshots/pattern_schema.json"
        );
        std::fs::write(path, &document).expect("write pattern schema snapshot");
    } else {
        assert_eq!(
            document, SNAPSHOT,
            "PatternDoc schema changed; re-bless with `BLESS_SCHEMA=1 cargo test -p ludion-engine \
             --test pattern_schema_snapshot`, then run `npm run generate` in editor/protocol and \
             commit the snapshot + editor/app/src/editor/effects/patternSchema.ts",
        );
    }
}
