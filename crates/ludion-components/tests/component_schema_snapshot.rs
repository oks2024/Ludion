//! Pins every built-in component's JSON Schema (what `engine.describe` serves) to a committed
//! snapshot.
//!
//! The editor reads component shapes by hand (`editor/app/src/editor/component-shapes.test.ts`
//! checks them against this file), so a schema change that is not re-blessed fails here instead of
//! drifting silently. To re-bless:
//!
//! ```text
//! BLESS_SCHEMA=1 cargo nextest run -p ludion-components --test component_schema_snapshot
//! ```

use std::collections::BTreeMap;

/// The committed pretty-printed schemas, keyed by component name.
const SNAPSHOT: &str = include_str!("snapshots/component_schemas.json");

#[test]
fn component_schemas_match_committed_snapshot() {
    let registry = ludion_components::builtin_registry();
    let schemas: BTreeMap<&str, &serde_json::Value> = registry
        .iter()
        .map(|info| (info.name(), info.schema()))
        .collect();
    let mut document = serde_json::to_string_pretty(&schemas).expect("serialize component schemas");
    document.push('\n');

    if ludion_core::bless::switch_set(ludion_core::bless::BLESS_SCHEMA_ENV) {
        let path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/snapshots/component_schemas.json"
        );
        std::fs::write(path, &document).expect("write component schema snapshot");
    } else {
        assert_eq!(
            document, SNAPSHOT,
            "a component schema changed; re-bless with `BLESS_SCHEMA=1 cargo nextest run -p \
             ludion-components --test component_schema_snapshot` and commit the snapshot",
        );
    }
}
