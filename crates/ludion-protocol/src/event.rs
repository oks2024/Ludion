//! [`Event`] — the payloads of the non-frame subscription channels.
//!
//! An event travels inside an [`EventMessage`](crate::EventMessage) envelope: the `event` name plus a
//! `data` object (engine-architecture.md §6). The `frames` channel is delivered as binary
//! [`frame`](crate::frame) messages, not as JSON events, so it has no variant here; the remaining
//! channels — `log`, `sim.events`, `scene.changed`, `asset.reloaded`, `profile` — do.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// A subscription event payload, adjacently tagged by its channel `event` name.
///
/// # Example
/// ```
/// use ludion_protocol::Event;
/// use ludion_protocol::event::AssetReloadedData;
///
/// let event = Event::AssetReloaded(AssetReloadedData {
///     path: "brick_diff.png".into(),
///     hash: "00000000deadbeef".into(),
/// });
/// assert_eq!(
///     serde_json::to_value(&event).unwrap(),
///     serde_json::json!({
///         "event": "asset.reloaded",
///         "data": { "path": "brick_diff.png", "hash": "00000000deadbeef" },
///     }),
/// );
/// ```
// `#[non_exhaustive]`: a future additive protocol rev may add a subscription channel and its event,
// so a downstream client that matches on this must carry a wildcard arm. Kept as a plain comment, not
// a `///` doc: the enum's doc feeds schemars' `description`, and editing it would move the schema hash.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "event", content = "data")]
#[non_exhaustive]
pub enum Event {
    /// A structured log record (`log` channel).
    #[serde(rename = "log")]
    Log(LogEvent),
    /// Gameplay/sim events emitted by systems this frame (`sim.events` channel).
    #[serde(rename = "sim.events")]
    SimEvents(SimEventsData),
    /// The world was replaced or the scene file saved: a `scene.load`, `scene.save`, `engine.restore`
    /// or a game-driven scene switch (`scene.changed` channel).
    #[serde(rename = "scene.changed")]
    SceneChanged(SceneChangedData),
    /// An asset was reloaded (`asset.reloaded` channel).
    #[serde(rename = "asset.reloaded")]
    AssetReloaded(AssetReloadedData),
    /// One closed window of live performance data (`profile` channel).
    // Boxed: a window is an order of magnitude larger than any other event payload (it carries a
    // whole window's per-frame columns), and every event is `clone`d once per subscriber in the
    // fan-out. `Box` is invisible on the wire and in the schema.
    #[serde(rename = "profile")]
    ProfileWindow(Box<crate::perf::ProfileWindowData>),
}

/// Severity of a [`LogEvent`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum LogLevel {
    /// Fine-grained tracing.
    Trace,
    /// Debugging detail.
    Debug,
    /// Ordinary progress.
    Info,
    /// A recoverable concern.
    Warn,
    /// A failure.
    Error,
}

/// The payload of a `log` event.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct LogEvent {
    /// The record's severity.
    pub level: LogLevel,
    /// The log message.
    pub message: String,
    /// The whole diagnostic, present for an `L####` line; `message` is then its rustc-style
    /// rendering.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(with = "Option<crate::schema_proxy::WireErrorSchema>")]
    pub diagnostic: Option<ludion_core::diagnostics::Diagnostic>,
}

impl LogEvent {
    /// The log record for a diagnostic: a warning logs at `Warn`, anything else at `Error`, and
    /// `message` is the diagnostic's `Display` rendering.
    ///
    /// # Example
    /// ```
    /// use ludion_core::codes::code;
    /// use ludion_core::diagnostics::Diagnostic;
    /// use ludion_protocol::event::{LogEvent, LogLevel};
    ///
    /// let event = LogEvent::from_diagnostic(
    ///     Diagnostic::new(code::PROTOCOL_MALFORMED_ENVELOPE, "bad envelope").with_path("/id"),
    /// );
    /// assert_eq!(event.level, LogLevel::Error);
    /// assert!(event.message.contains("bad envelope"));
    /// assert!(event.diagnostic.is_some());
    /// ```
    pub fn from_diagnostic(diagnostic: ludion_core::diagnostics::Diagnostic) -> LogEvent {
        let level = match diagnostic.severity {
            ludion_core::diagnostics::Severity::Warning => LogLevel::Warn,
            _ => LogLevel::Error,
        };
        LogEvent {
            level,
            message: diagnostic.to_string(),
            diagnostic: Some(diagnostic),
        }
    }
}

/// The payload of a `sim.events` event: opaque, game-defined event values for this frame.
#[derive(Clone, Debug, PartialEq, Default, Serialize, Deserialize, JsonSchema)]
pub struct SimEventsData {
    /// The gameplay events, as JSON values the protocol layer does not interpret.
    #[serde(default)]
    pub events: Vec<Value>,
}

/// The payload of a `scene.changed` event.
#[derive(Clone, Debug, PartialEq, Eq, Default, Serialize, Deserialize, JsonSchema)]
pub struct SceneChangedData {
    /// The content-root-relative scene path of a `scene.load` or `scene.save`; absent for an
    /// `engine.restore` or a game-driven scene switch.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
}

/// The payload of an `asset.reloaded` event.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct AssetReloadedData {
    /// The reloaded asset's path.
    pub path: String,
    /// The reloaded asset's new content hash (16 hex digits, matching the asset content hash form).
    pub hash: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn log_event_round_trips() {
        let event = Event::Log(LogEvent {
            level: LogLevel::Warn,
            message: "slow frame".into(),
            diagnostic: None,
        });
        let value = serde_json::to_value(&event).unwrap();
        assert_eq!(
            value,
            serde_json::json!({ "event": "log", "data": { "level": "warn", "message": "slow frame" } }),
        );
        assert_eq!(serde_json::from_value::<Event>(value).unwrap(), event);
    }

    #[test]
    fn a_diagnostic_log_event_round_trips_with_its_structured_field() {
        let diagnostic = ludion_core::diagnostics::Diagnostic::new(
            ludion_core::codes::code::PROTOCOL_MALFORMED_ENVELOPE,
            "bad id",
        )
        .with_path("/id")
        .with_help("send an integer");
        let event = Event::Log(LogEvent::from_diagnostic(diagnostic.clone()));
        let value = serde_json::to_value(&event).unwrap();
        assert_eq!(value["data"]["level"], "error");
        assert_eq!(value["data"]["message"], diagnostic.to_string());
        assert_eq!(
            value["data"]["diagnostic"],
            serde_json::to_value(&diagnostic).unwrap()
        );
        assert_eq!(serde_json::from_value::<Event>(value).unwrap(), event);
    }

    #[test]
    fn profile_window_round_trips_under_its_channel_name() {
        let data = crate::perf::ProfileWindowData {
            first_frame: 0,
            last_frame: 59,
            frames: 60,
            capture: false,
            threads: 8,
            systems: vec![crate::perf::SystemTiming {
                name: "enemy_ai".into(),
                median_ms: 0.4,
                tail: None,
            }],
            sequential: Vec::new(),
            frame_total_median_ms: 1.2,
            gpu: crate::perf::LiveGpuProfile {
                timestamps_supported: false,
                sampled_frames: 0,
                passes: Vec::new(),
            },
            transport: None,
            samples: crate::perf::ProfileSamples {
                frames: vec![0],
                system_names: vec!["enemy_ai".into()],
                system_ms: vec![vec![0.4]],
                frame_total_ms: vec![1.2],
                extract_ms: vec![None],
                submit_ms: vec![None],
                pass_names: Vec::new(),
                pass_ms: Vec::new(),
            },
        };
        let event = Event::ProfileWindow(Box::new(data));
        let value = serde_json::to_value(&event).unwrap();
        assert_eq!(value["event"], serde_json::json!("profile"));
        assert_eq!(value["data"]["frames"], serde_json::json!(60));
        assert_eq!(serde_json::from_value::<Event>(value).unwrap(), event);
    }

    #[test]
    fn scene_changed_omits_absent_path() {
        let event = Event::SceneChanged(SceneChangedData { path: None });
        assert_eq!(
            serde_json::to_value(&event).unwrap(),
            serde_json::json!({ "event": "scene.changed", "data": {} }),
        );
    }
}
