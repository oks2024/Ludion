//! [`RestoreReport`] — what an `engine.restore` migration had to change to load a snapshot.
//!
//! Restore is a **migration**, not a strict round-trip (engine-architecture.md §8): the dev loop
//! restores a snapshot taken from one build into a freshly recompiled binary, so component-schema
//! drift between the snapshot and the running build is normal, not fatal. Instead of hard-failing on
//! drift, `engine.restore` reconstructs the world as faithfully as it can and returns this report of
//! every divergence — an **empty** report means a clean, lossless restore.
//!
//! Like everything in this crate, the report never names a concrete component type (the boundary
//! rule): it speaks only in registry-name strings and entity ids. The report is carried by
//! [`ResponseBody::Restored`](crate::ResponseBody::Restored).

use ludion_core::codes::code;
use ludion_core::ids::EntityId;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// The outcome of an `engine.restore`: the per-component changes the migration made to load the
/// snapshot. An **empty** [`entries`](RestoreReport::entries) list is a clean restore — every entity
/// and every component came back exactly as snapshotted.
///
/// Entries are in restore order: the document-level entries first (an engine-version difference,
/// then each unknown top-level field, sorted), then the per-entity entries — entities in ascending
/// snapshot slot-index order, and within an entity, its components in the order the snapshot stored
/// them (registration order at snapshot time).
///
/// # Example
/// ```
/// use ludion_protocol::restore::{RestoreReport, RestoreEntry};
/// use ludion_core::ids::EntityId;
///
/// // A clean restore carries no entries.
/// let clean = RestoreReport::default();
/// assert!(clean.is_clean());
///
/// // A drop of an unregistered component maps to L3015.
/// let entry = RestoreEntry::ComponentUnregistered {
///     entity: EntityId::new(2, 0),
///     component: "OldGizmo".to_string(),
/// };
/// assert_eq!(entry.code(), Some("L3015"));
/// ```
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct RestoreReport {
    /// One entry per divergence the migration found (empty = clean restore).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub entries: Vec<RestoreEntry>,
}

impl RestoreReport {
    /// Whether the restore was clean and lossless (no entries).
    pub fn is_clean(&self) -> bool {
        self.entries.is_empty()
    }
}

/// One change an `engine.restore` migration made to a single component of a single entity.
///
/// The two `Component…` drop variants map to diagnostic codes ([`code`](RestoreEntry::code)); the
/// informational [`ComponentMigrated`](RestoreEntry::ComponentMigrated) field-diff has none. Run
/// `ludion explain L3015` / `L3016` for the full explanation of each drop.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum RestoreEntry {
    /// The snapshot named a component this build no longer registers, so it was **dropped** — the
    /// entity restored without it. Diagnostic code `L3015`.
    ComponentUnregistered {
        /// The entity the dropped component belonged to.
        #[schemars(with = "crate::schema_proxy::EntityIdSchema")]
        entity: EntityId,
        /// The dropped component's registry name (as stored in the snapshot).
        component: String,
    },
    /// The component is still registered, but its snapshotted value no longer deserializes into the
    /// current type, so it was **dropped** — the entity restored without it. Diagnostic code `L3016`.
    ComponentUndeserializable {
        /// The entity the dropped component belonged to.
        #[schemars(with = "crate::schema_proxy::EntityIdSchema")]
        entity: EntityId,
        /// The dropped component's registry name.
        component: String,
        /// The serde failure text explaining why the value would not deserialize.
        error: String,
    },
    /// The component was **kept**, but its serialized shape changed: `filled` field names came from
    /// serde defaults (added to the type since the snapshot), and `removed` field names were present
    /// in the snapshot but no longer exist on the type. Every other field restored verbatim. This is
    /// informational — no diagnostic code — and is only reported when at least one field changed.
    ComponentMigrated {
        /// The entity whose component was migrated.
        #[schemars(with = "crate::schema_proxy::EntityIdSchema")]
        entity: EntityId,
        /// The migrated component's registry name.
        component: String,
        /// Field paths newly filled from serde defaults (added since the snapshot), sorted; a nested
        /// field is dotted (`inner.b`).
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        filled: Vec<String>,
        /// Field paths present in the snapshot but no longer on the type (dropped), sorted; a nested
        /// field is dotted.
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        removed: Vec<String>,
    },
    /// The snapshot was captured by a different engine version than this build. The restore went
    /// ahead; this is a warning, not a refusal. Diagnostic code `L3048`.
    EngineVersionDiffers {
        /// The engine version stored in the snapshot.
        snapshot_version: String,
        /// The running engine's version.
        engine_version: String,
    },
    /// The snapshot document carried a top-level field this build does not define; the restore
    /// ignored it. Diagnostic code `L3049`.
    SnapshotFieldUnknown {
        /// The unknown top-level field's name.
        field: String,
    },
}

impl RestoreEntry {
    /// The diagnostic code for this entry, or `None` for the informational field migration.
    ///
    /// The dropped-component variants carry a code an agent can pass to `ludion explain`; the
    /// field-level migration is not a failure and has none.
    pub fn code(&self) -> Option<&'static str> {
        match self {
            RestoreEntry::ComponentUnregistered { .. } => {
                Some(code::RESTORE_COMPONENT_UNREGISTERED.as_str())
            }
            RestoreEntry::ComponentUndeserializable { .. } => {
                Some(code::RESTORE_COMPONENT_UNDESERIALIZABLE.as_str())
            }
            RestoreEntry::ComponentMigrated { .. } => None,
            RestoreEntry::EngineVersionDiffers { .. } => {
                Some(code::RESTORE_ENGINE_VERSION_DIFFERS.as_str())
            }
            RestoreEntry::SnapshotFieldUnknown { .. } => {
                Some(code::RESTORE_SNAPSHOT_FIELD_UNKNOWN.as_str())
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clean_report_serializes_without_entries() {
        assert_eq!(
            serde_json::to_value(RestoreReport::default()).unwrap(),
            serde_json::json!({}),
        );
    }

    #[test]
    fn entries_round_trip_and_carry_codes() {
        let report = RestoreReport {
            entries: vec![
                RestoreEntry::ComponentUnregistered {
                    entity: EntityId::new(0, 0),
                    component: "OldGizmo".into(),
                },
                RestoreEntry::ComponentUndeserializable {
                    entity: EntityId::new(1, 0),
                    component: "Health".into(),
                    error: "invalid type: string, expected f32".into(),
                },
                RestoreEntry::ComponentMigrated {
                    entity: EntityId::new(2, 0),
                    component: "Velocity".into(),
                    filled: vec!["damping".into()],
                    removed: vec!["legacy".into()],
                },
                RestoreEntry::EngineVersionDiffers {
                    snapshot_version: "0.9.0".into(),
                    engine_version: "1.0.0".into(),
                },
                RestoreEntry::SnapshotFieldUnknown {
                    field: "from_the_future".into(),
                },
            ],
        };
        let value = serde_json::to_value(&report).unwrap();
        assert_eq!(
            serde_json::from_value::<RestoreReport>(value).unwrap(),
            report,
        );
        assert_eq!(report.entries[0].code(), Some("L3015"));
        assert_eq!(report.entries[1].code(), Some("L3016"));
        assert_eq!(report.entries[2].code(), None);
        assert_eq!(report.entries[3].code(), Some("L3048"));
        assert_eq!(report.entries[4].code(), Some("L3049"));
    }
}
