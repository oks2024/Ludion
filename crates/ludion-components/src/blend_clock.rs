//! The [`BlendClock`] component: the shared phase of a blend-space character, kept by the engine's
//! `blend_clocks` system (`Plan/animation-layers.md` decisions 13–19).

use ludion_core::time::FrameIndex;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// The phase of a character whose `Animation.clip` names a blend space. **Written by
/// `blend_clocks`**: a game reads it and never inserts one. It is hashed sim state, so a restored
/// world resumes the stride it was in.
///
/// `phase` counts unwrapped cycles of the blend's shared normalized phase; `prev` is the phase the
/// previous gameplay tick ended on, so `animation_events` fires the crossings in `(prev, phase]`;
/// it is `-1` on the tick the phase restarted, so that window opens at phase `0` inclusive.
/// `started` is the `Animation.started` this phase belongs to and `reset` the gameplay frame it
/// restarted at. While a crossfade leaves another blend,
/// `from_blend` and `from_phase` carry its phase.
///
/// ```yaml
/// components:
///   BlendClock:
///     blend: locomotion
///     started: 12
///     reset: 12
///     phase: 3.25
///     prev: 3.2
/// ```
///
/// ```
/// use ludion_components::BlendClock;
///
/// let clock = BlendClock::default();
/// assert_eq!(clock.phase, 0.0);
/// assert!(serde_json::to_value(&clock).unwrap().get("from_blend").is_none());
/// ```
#[derive(Serialize, Deserialize, JsonSchema, Clone, Debug, PartialEq)]
pub struct BlendClock {
    /// The blend this phase belongs to (default `""`).
    #[serde(default)]
    pub blend: String,
    /// The `Animation.started` the phase was stamped for (default frame `0`).
    #[serde(default = "crate::frame_zero")]
    #[schemars(with = "u64")]
    pub started: FrameIndex,
    /// The gameplay frame the phase was last zeroed at (default frame `0`).
    #[serde(default = "crate::frame_zero")]
    #[schemars(with = "u64")]
    pub reset: FrameIndex,
    /// Unwrapped phase cycles (default `0.0`).
    #[serde(default)]
    pub phase: f64,
    /// The phase at the end of the previous gameplay tick; `-1` on a restart tick (default `0.0`).
    #[serde(default)]
    pub prev: f64,
    /// The outgoing blend of a crossfade, when it is one (default `""` — none).
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub from_blend: String,
    /// The outgoing blend's phase (default `0.0`).
    #[serde(default, skip_serializing_if = "is_zero")]
    pub from_phase: f64,
    /// The phases of the layers whose `clip` names a blend, in layer order (default none).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub layers: Vec<LayerPhase>,
}

/// One `AnimationLayers` layer's blend phase inside a [`BlendClock`], kept like the base phase.
#[derive(Serialize, Deserialize, JsonSchema, Clone, Debug, PartialEq)]
pub struct LayerPhase {
    /// The layer's name.
    pub name: String,
    /// The blend the layer's `clip` named when the phase was stamped.
    pub blend: String,
    /// The layer's `started` the phase was stamped for (default frame `0`).
    #[serde(default = "crate::frame_zero")]
    #[schemars(with = "u64")]
    pub started: FrameIndex,
    /// Unwrapped phase cycles (default `0.0`).
    #[serde(default)]
    pub phase: f64,
    /// The phase at the end of the previous gameplay tick; `-1` on a restart tick (default `0.0`).
    #[serde(default)]
    pub prev: f64,
}

impl Default for BlendClock {
    fn default() -> Self {
        Self {
            blend: String::new(),
            started: FrameIndex::ZERO,
            reset: FrameIndex::ZERO,
            phase: 0.0,
            prev: 0.0,
            from_blend: String::new(),
            from_phase: 0.0,
            layers: Vec::new(),
        }
    }
}

#[allow(clippy::float_cmp)]
fn is_zero(value: &f64) -> bool {
    *value == 0.0
}
