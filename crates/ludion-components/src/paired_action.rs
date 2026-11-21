//! The [`PairedAction`] component: one participant's half of a playing two-role paired action
//! (`Plan/paired-animations.md` decisions 27–31, Neon Shroud request 0008).

use ludion_core::ids::EntityId;
use ludion_core::time::FrameIndex;
use ludion_ecs::SemanticValidate;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// One participant's state in a playing paired action, written by `start_paired` and removed when the
/// action ends.
///
/// The partners of an action are the entities carrying the same `(document, reference, started)`; each
/// holds one ref, so a witness reads the whole action from either side. `fired` counts the document
/// events this participant has already fired, so a snapshot restore neither loses nor repeats one.
///
/// ```yaml
/// components:
///   PairedAction:
///     document: voxels/choke.paired.yaml
///     action: choke
///     role: target
///     reference: { index: 4, generation: 0 }
///     started: 120
///     fired: 1
/// ```
#[derive(Serialize, Deserialize, JsonSchema, Clone, Debug, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct PairedAction {
    /// The document's normalized content-root path.
    pub document: String,
    /// The document's `name`: events fire as `paired:<action>`.
    pub action: String,
    /// The role this entity plays.
    pub role: String,
    /// The entity cast in the document's `align.reference` role.
    #[schemars(with = "ludion_ecs::EntityRefSchema")]
    pub reference: EntityId,
    /// The frame the action started at: both clips' `Animation.started`.
    #[schemars(with = "u64")]
    pub started: FrameIndex,
    /// How many document events this participant has fired (default `0`).
    #[serde(default, skip_serializing_if = "is_zero")]
    pub fired: u32,
}

fn is_zero(n: &u32) -> bool {
    *n == 0
}

impl SemanticValidate for PairedAction {
    /// No rules: the engine writes it, and a hand-authored one only needs to deserialize.
    fn validate_semantics(_value: &Value) -> Vec<ludion_core::diagnostics::Diagnostic> {
        Vec::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn it_round_trips_and_omits_a_zero_fired() {
        let value = json!({
            "document": "d", "action": "a", "role": "r",
            "reference": { "index": 3, "generation": 1 }, "started": 7,
        });
        let action: PairedAction = serde_json::from_value(value.clone()).unwrap();
        assert_eq!(action.fired, 0);
        assert_eq!(serde_json::to_value(&action).unwrap(), value);
    }

    #[test]
    fn an_unknown_field_is_refused() {
        let value = json!({
            "document": "d", "action": "a", "role": "r",
            "reference": { "index": 0, "generation": 0 }, "started": 0, "wat": 1,
        });
        assert!(serde_json::from_value::<PairedAction>(value).is_err());
    }
}
