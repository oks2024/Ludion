//! The [`AnimGraphState`] component: which state of a game's Rust `AnimGraph` an entity is in
//! (`Plan/animation-layers.md` decisions 50-53, Neon Shroud request 0007 §6).

use ludion_core::time::FrameIndex;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// The state of an entity's `AnimGraph`, by the stable name the graph's table gives it.
///
/// `AnimGraph::step` reads it and rewrites it only on a transition, so it stays quiet in the hash between
/// transitions. There is no implicit entry: a game spawns it beside an [`Animation`](crate::Animation)
/// already playing that state's clip. A name the graph does not declare is a runtime `step` error
/// (`L4290`), not a load error, because the graph is Rust and invisible at load.
///
/// ```yaml
/// components:
///   AnimGraphState:
///     state: run
///     entered: 60
///     previous: idle
/// ```
#[derive(Serialize, Deserialize, JsonSchema, Clone, Debug, PartialEq)]
pub struct AnimGraphState {
    /// The current state's name (default empty: no state yet).
    #[serde(default)]
    pub state: String,
    /// The gameplay frame the state was entered (default frame `0`).
    #[serde(default = "frame_zero")]
    #[schemars(with = "u64")]
    pub entered: FrameIndex,
    /// The state the entity left to enter this one; omitted when empty (default none).
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub previous: String,
}

impl Default for AnimGraphState {
    fn default() -> Self {
        AnimGraphState {
            state: String::new(),
            entered: FrameIndex::ZERO,
            previous: String::new(),
        }
    }
}

fn frame_zero() -> FrameIndex {
    FrameIndex::ZERO
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn the_empty_document_is_the_default_and_round_trips() {
        let empty: AnimGraphState = serde_json::from_value(json!({})).unwrap();
        assert_eq!(empty, AnimGraphState::default());
        let value = serde_json::to_value(&empty).unwrap();
        assert!(value.get("previous").is_none(), "empty previous is omitted");
        assert_eq!(
            serde_json::from_value::<AnimGraphState>(value).unwrap(),
            empty
        );
    }

    #[test]
    fn a_full_document_round_trips() {
        let doc = json!({ "state": "run", "entered": 60, "previous": "idle" });
        let state: AnimGraphState = serde_json::from_value(doc.clone()).unwrap();
        assert_eq!(state.entered, FrameIndex(60));
        assert_eq!(serde_json::to_value(&state).unwrap(), doc);
    }
}
