//! [`ProfileMigration`] — what loading a saved player profile had to change to honor it.
//!
//! A player profile (`save/profile.yaml`, the persistent save state a game registers with
//! `profile::<P>`) is loaded **leniently**, not as a strict round-trip (`Plan/phase-16-profile.md`,
//! decision 3): the profile type drifts across builds as a game grows, and a player's accumulated
//! save must survive that drift rather than be rejected. So the load migrates — filling fields the
//! game added from their defaults, dropping fields the game removed, and, when the file cannot be
//! honored at all, backing it up and starting a fresh default — and returns this report of every
//! change. An **empty** report means the profile loaded exactly as it was written.
//!
//! Like everything in this crate, the report is boundary-clean: it never names the concrete profile
//! type, speaking only in field-name strings and paths. It is the profile analog of
//! [`RestoreReport`](crate::restore::RestoreReport).

use ludion_core::codes::code;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// The outcome of loading a saved player profile: the changes the lenient migration made to honor
/// the file. An **empty** [`entries`](ProfileMigration::entries) list means a clean load — the file
/// deserialized exactly, with no field drift and no reset.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ProfileMigration {
    /// One entry per change the migration made (empty = the profile loaded exactly as authored).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub entries: Vec<ProfileMigrationEntry>,
}

impl ProfileMigration {
    /// Whether the profile loaded cleanly and losslessly (no entries).
    pub fn is_clean(&self) -> bool {
        self.entries.is_empty()
    }
}

/// One change the lenient profile migration made while loading a saved profile.
///
/// The two `Fields…` variants are informational field drift (no diagnostic code); [`Reset`] is the
/// backup-and-default path and carries [`code`](ProfileMigrationEntry::code) `L1121`. Run
/// `ludion explain L1121` for the full explanation.
///
/// [`Reset`]: ProfileMigrationEntry::Reset
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ProfileMigrationEntry {
    /// Fields the game's profile type has that the saved file lacked, so they were **filled** from
    /// serde defaults (added to the type since the save). Informational — no diagnostic code.
    FieldsDefaulted {
        /// The filled field paths, sorted; a nested field is dotted (`inner.b`).
        fields: Vec<String>,
    },
    /// Fields the saved file had that the game's profile type no longer declares, so they were
    /// **dropped** (removed from the type since the save). Informational — no diagnostic code.
    FieldsDropped {
        /// The dropped field paths, sorted; a nested field is dotted (`inner.b`).
        fields: Vec<String>,
    },
    /// The saved file could not be honored at all — a malformed envelope, a value that no longer
    /// deserializes into the current profile type, or a newer-than-engine `format` — so it was
    /// **backed up** and a fresh `P::default()` profile started. The player's bytes are preserved in
    /// the backup, never destroyed, and the game still boots. Diagnostic code `L1121`.
    Reset {
        /// The path (or file name) the original save was renamed to, e.g.
        /// `profile.yaml.bak.<content-hash>`.
        backup: String,
        /// Why the file could not be honored (the envelope/deserialize/version failure text).
        reason: String,
    },
}

impl ProfileMigrationEntry {
    /// The diagnostic code for this entry, or `None` for the informational field-drift variants.
    ///
    /// Only [`Reset`](ProfileMigrationEntry::Reset) — the backup-and-default path — carries a code an
    /// agent can pass to `ludion explain`; filling and dropping fields are lossless migration steps,
    /// not failures, and have none.
    pub fn code(&self) -> Option<&'static str> {
        match self {
            ProfileMigrationEntry::Reset { .. } => Some(code::PROFILE_LOAD_FAILED.as_str()),
            ProfileMigrationEntry::FieldsDefaulted { .. }
            | ProfileMigrationEntry::FieldsDropped { .. } => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clean_report_serializes_without_entries() {
        assert_eq!(
            serde_json::to_value(ProfileMigration::default()).unwrap(),
            serde_json::json!({}),
        );
        assert!(ProfileMigration::default().is_clean());
    }

    #[test]
    fn entries_round_trip_and_carry_codes() {
        let report = ProfileMigration {
            entries: vec![
                ProfileMigrationEntry::FieldsDefaulted {
                    fields: vec!["runs".into()],
                },
                ProfileMigrationEntry::FieldsDropped {
                    fields: vec!["legacy_score".into()],
                },
                ProfileMigrationEntry::Reset {
                    backup: "profile.yaml.bak.a1b2c3".into(),
                    reason: "the profile's `kind` must be the string `profile`".into(),
                },
            ],
        };
        let value = serde_json::to_value(&report).unwrap();
        assert_eq!(
            serde_json::from_value::<ProfileMigration>(value).unwrap(),
            report,
        );
        assert!(!report.is_clean());
        assert_eq!(report.entries[0].code(), None);
        assert_eq!(report.entries[1].code(), None);
        assert_eq!(report.entries[2].code(), Some("L1121"));
    }
}
