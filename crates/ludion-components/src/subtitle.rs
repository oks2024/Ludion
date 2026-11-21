//! The [`SubtitleView`] component: the built-in subtitle block for the lines on air, and the
//! [`Channel`] its `channels` filter names. `ludion docs dialogue` is the guide.

use ludion_core::diagnostics::Diagnostic;
use ludion_ecs::SemanticValidate;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::{Anchor, as_stored_f32, require_positive_finite_ui_size};

/// The slot a speaker's lines queue on: one global `radio`, one global `ui`, and a `world` slot per
/// speaker entity.
#[derive(
    Serialize, Deserialize, JsonSchema, Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash,
)]
#[serde(rename_all = "snake_case")]
pub enum Channel {
    /// Comms: one line at a time across the game.
    Radio,
    /// Spoken from a world entity: one line at a time per speaker.
    World,
    /// Interface voice: one line at a time across the game.
    Ui,
}

/// A speaker-labelled, wrapped block of the dialogue lines on air, on a background panel.
///
/// It reads the engine's `Subtitles` list, so it draws only in a game that opts into dialogue; a
/// scene without a `SubtitleView` shows no subtitles. A game with its own look reads `Subtitles`
/// and omits the entity. An entity may carry a `Transform` alongside it; it is ignored.
///
/// ```
/// use ludion_components::SubtitleView;
///
/// let view: SubtitleView = serde_json::from_value(serde_json::json!({
///     "anchor": "bottom_center",
///     "max_lines": 2,
///     "channels": ["radio", "ui"],
/// }))
/// .unwrap();
/// assert_eq!(view.max_lines, 2);
/// assert_eq!(view.size, 30.0);
/// ```
#[derive(Serialize, Deserialize, JsonSchema, Clone, Debug, PartialEq)]
pub struct SubtitleView {
    /// Which canvas point (and block pivot) the block is placed at (default `bottom_center`).
    #[serde(default = "default_anchor")]
    pub anchor: Anchor,
    /// Offset from the anchor point, in canvas units, **+Y down** (default `[0, -64]`).
    #[serde(default = "default_offset")]
    pub offset: [f32; 2],
    /// Logical face name, resolved through the content root's `fonts.yaml`; absent → the table's
    /// `default` entry, else the built-in 8×8 face.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub font: Option<String>,
    /// Glyph cell height in canvas units (default `30.0`).
    #[serde(default = "default_size")]
    pub size: f32,
    /// The widest a row may be before it wraps, in canvas units; `0` is no wrap (default `1100.0`).
    #[serde(default = "default_wrap_width")]
    #[schemars(range(min = 0.0))]
    pub wrap_width: f32,
    /// The most rows drawn, newest kept; `0` is all of them (default `3`).
    #[serde(default = "default_max_lines")]
    pub max_lines: u32,
    /// Whether each line is prefixed with its speaker's name, in the speaker's colour (default
    /// `true`).
    #[serde(default = "default_true")]
    pub speaker_label: bool,
    /// Linear RGBA of the line text (default opaque white).
    #[serde(default = "default_color")]
    pub color: [f32; 4],
    /// Linear RGBA of the panel behind the block; alpha `0` draws no panel (default
    /// `[0, 0, 0, 0.55]`).
    #[serde(default = "default_background")]
    pub background: [f32; 4],
    /// How far the panel extends beyond the text on every side, in canvas units (default `12.0`).
    #[serde(default = "default_padding")]
    #[schemars(range(min = 0.0))]
    pub padding: f32,
    /// The channels shown (default all three).
    #[serde(default = "default_channels")]
    pub channels: Vec<Channel>,
    /// Draw order: higher draws later, on top (default `0`).
    #[serde(default)]
    pub order: i32,
    /// Whether the block draws at all (default `true`).
    #[serde(default = "default_true")]
    pub visible: bool,
}

fn default_anchor() -> Anchor {
    Anchor::BottomCenter
}
fn default_offset() -> [f32; 2] {
    [0.0, -64.0]
}
fn default_size() -> f32 {
    30.0
}
fn default_wrap_width() -> f32 {
    1100.0
}
fn default_max_lines() -> u32 {
    3
}
fn default_true() -> bool {
    true
}
fn default_color() -> [f32; 4] {
    [1.0; 4]
}
fn default_background() -> [f32; 4] {
    [0.0, 0.0, 0.0, 0.55]
}
fn default_padding() -> f32 {
    12.0
}
fn default_channels() -> Vec<Channel> {
    vec![Channel::Radio, Channel::World, Channel::Ui]
}

impl Default for SubtitleView {
    fn default() -> Self {
        SubtitleView {
            anchor: default_anchor(),
            offset: default_offset(),
            font: None,
            size: default_size(),
            wrap_width: default_wrap_width(),
            max_lines: default_max_lines(),
            speaker_label: true,
            color: default_color(),
            background: default_background(),
            padding: default_padding(),
            channels: default_channels(),
            order: 0,
            visible: true,
        }
    }
}

impl SemanticValidate for SubtitleView {
    /// Rejects a `size` that is not strictly positive and finite (`L1118`), pointer `/size`.
    fn validate_semantics(value: &Value) -> Vec<Diagnostic> {
        let mut out = Vec::new();
        if let Some(size) = value.get("size").and_then(as_stored_f32) {
            require_positive_finite_ui_size(size, "/size", &mut out);
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_view_is_the_default() {
        let view: SubtitleView = serde_json::from_value(serde_json::json!({})).unwrap();
        assert_eq!(view, SubtitleView::default());
    }

    #[test]
    fn non_positive_size_is_l1118() {
        let out = SubtitleView::validate_semantics(&serde_json::json!({ "size": 0.0 }));
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].path.as_deref(), Some("/size"));
        assert!(SubtitleView::validate_semantics(&serde_json::json!({})).is_empty());
    }
}
