//! The wire envelopes: [`Request`], [`Response`], and [`EventMessage`].
//!
//! Every message on the wire is one of these three (engine-architecture.md §6):
//!
//! ```text
//! → {"v":1, "id":42, "method":"entity.spawn", "params":{…}}          Request
//! ← {"id":42, "ok":{…}, "frame":1203, "game_frame":1150}              Response (success)
//! ← {"id":42, "error":{"code":"L…","message":"…","path":"…","help":"…"}}   Response (failure)
//! ← {"event":"scene.changed", "sub":7, "seq":981, "frame":1204, "data":{…}} EventMessage
//! ```
//!
//! The `id` correlates a response to its request; `frame` is the sim frame at which a command applied
//! (carried by every success, so causality and replay stitching are always available) and
//! `game_frame` the gameplay frame at the same boundary; `sub` + `seq`
//! tie an event to its subscription with a monotonic sequence number whose gaps are visible,
//! intentional drops.

use ludion_core::codes::code;
use ludion_core::diagnostics::Diagnostic;
use ludion_core::time::FrameIndex;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::PROTOCOL_VERSION;
use crate::command::Command;
use crate::event::Event;
use crate::response::ResponseBody;

/// A request correlation id, chosen by the client and echoed in the matching [`Response`].
#[derive(
    Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug, Serialize, Deserialize, JsonSchema,
)]
#[serde(transparent)]
#[schemars(transparent)]
pub struct RequestId(pub u64);

impl RequestId {
    /// Wraps a raw id.
    pub const fn new(id: u64) -> Self {
        RequestId(id)
    }
    /// The raw id.
    pub const fn as_u64(self) -> u64 {
        self.0
    }
}

/// A subscription id, minted by the server in the `stream.subscribe` response.
#[derive(
    Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug, Serialize, Deserialize, JsonSchema,
)]
#[serde(transparent)]
#[schemars(transparent)]
pub struct SubId(pub u64);

impl SubId {
    /// Wraps a raw id.
    pub const fn new(id: u64) -> Self {
        SubId(id)
    }
    /// The raw id.
    pub const fn as_u64(self) -> u64 {
        self.0
    }
}

/// A per-subscription monotonic event sequence number. Gaps mean intentional drops (visible to the
/// client), never lost messages by accident.
#[derive(
    Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug, Serialize, Deserialize, JsonSchema,
)]
#[serde(transparent)]
#[schemars(transparent)]
pub struct Seq(pub u64);

impl Seq {
    /// Wraps a raw sequence number.
    pub const fn new(seq: u64) -> Self {
        Seq(seq)
    }
    /// The raw sequence number.
    pub const fn as_u64(self) -> u64 {
        self.0
    }
    /// The next sequence number.
    pub const fn next(self) -> Self {
        Seq(self.0 + 1)
    }
}

/// The request envelope: a protocol version, a correlation id, and one flattened [`Command`].
///
/// # Example
/// ```
/// use ludion_protocol::{Command, Request};
/// use ludion_protocol::command::SceneLoadParams;
///
/// let request = Request::new(1, Command::SceneLoad(SceneLoadParams { path: "level.yaml".into() }));
/// let text = serde_json::to_string(&request).unwrap();
/// let back = Request::from_json(&text).unwrap();
/// assert_eq!(back.id.as_u64(), 1);
/// assert_eq!(back.command.method(), "scene.load");
/// ```
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct Request {
    /// The protocol version; must equal [`PROTOCOL_VERSION`].
    pub v: u32,
    /// The client-chosen correlation id, echoed in the response.
    pub id: RequestId,
    /// The command to run (flattened as `method` + `params`).
    #[serde(flatten)]
    pub command: Command,
}

impl Request {
    /// Builds a request at the current [`PROTOCOL_VERSION`] with the given id and command.
    pub fn new(id: u64, command: Command) -> Self {
        Request {
            v: PROTOCOL_VERSION,
            id: RequestId::new(id),
            command,
        }
    }

    /// Parses a request from JSON text, mapping failures to precise `L3xxx` diagnostics.
    ///
    /// # Errors
    /// - `L3001` — the text is not a JSON object, is missing `v`/`method`, or its params do not
    ///   match the method.
    /// - `L3002` — the `method` is a string but not a method this protocol version defines.
    /// - `L3003` — `v` is present but not the supported protocol version.
    pub fn from_json(text: &str) -> Result<Request, Diagnostic> {
        let value: Value = serde_json::from_str(text)
            .map_err(|e| malformed(format!("request is not valid JSON: {e}")))?;
        Request::from_value(value)
    }

    /// Parses a request from an already-decoded JSON value (same diagnostics as [`from_json`]).
    ///
    /// [`from_json`]: Request::from_json
    pub fn from_value(value: Value) -> Result<Request, Diagnostic> {
        let obj = value
            .as_object()
            .ok_or_else(|| malformed("request envelope must be a JSON object"))?;

        // Version is checked first, on the raw value: a wrong-version client must hear "unsupported
        // version" (L3003), not whichever unknown-method/params mismatch its stale schema also trips.
        match obj.get("v") {
            Some(Value::Number(n)) if n.as_u64() == Some(u64::from(PROTOCOL_VERSION)) => {}
            Some(other) => return Err(unsupported_version(other)),
            None => {
                return Err(malformed(
                    "request envelope is missing the protocol version field `v`",
                ));
            }
        }

        let method = obj
            .get("method")
            .and_then(Value::as_str)
            .ok_or_else(|| malformed("request envelope is missing a string `method`"))?
            .to_string();
        if !Command::METHODS.contains(&method.as_str()) {
            return Err(unknown_method(&method));
        }

        match obj.get("id") {
            Some(id) if id.is_u64() => {}
            Some(other) => {
                return Err(malformed(format!(
                    "request envelope `id` must be a non-negative integer, got {other}"
                ))
                .with_path("/id"));
            }
            None => {
                return Err(
                    malformed("request envelope is missing the integer correlation `id`")
                        .with_path("/id"),
                );
            }
        }

        serde_json::from_value::<Request>(value).map_err(|e| {
            malformed(format!("`params` do not match method `{method}`: {e}")).with_path("/params")
        })
    }
}

/// The outcome half of a [`Response`]: either an `ok` payload (with the applied `frame`) or an
/// `error`. The two are distinguished on the wire by which key is present.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(untagged)]
pub enum WireResult {
    /// A success: the `ok` payload plus the sim `frame` at which the command applied.
    Ok {
        /// The command's success payload.
        ok: ResponseBody,
        /// The sim frame the command applied at (every mutating success carries it).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        #[schemars(with = "Option<u64>")]
        frame: Option<FrameIndex>,
        /// The gameplay frame at the same boundary: `frame` less every frame the game has held
        /// through its `GameClock`. It is the frame a presentation `started` is stamped with, and
        /// it equals `frame` in a session that never holds.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        #[schemars(with = "Option<u64>")]
        game_frame: Option<FrameIndex>,
    },
    /// A failure: a structured diagnostic.
    Error {
        /// The diagnostic explaining the failure (its serde shape is the wire error object).
        #[schemars(with = "crate::schema_proxy::WireErrorSchema")]
        error: Diagnostic,
    },
}

/// The response envelope: the request's `id`, plus the [`WireResult`].
///
/// # Example
/// ```
/// use ludion_protocol::{Response, ResponseBody};
/// use ludion_core::time::FrameIndex;
///
/// let ok = Response::ok_at(42, ResponseBody::Ack, FrameIndex::new(1203), FrameIndex::new(1150));
/// assert_eq!(
///     serde_json::to_value(&ok).unwrap(),
///     serde_json::json!({ "id": 42, "ok": { "kind": "ack" }, "frame": 1203, "game_frame": 1150 }),
/// );
/// ```
// `#[non_exhaustive]`: this is a response envelope clients *consume* (field access + the `Response::ok`
// / `ok_at` / `error` constructors), so reserving room for a future additive field costs nothing.
// Plain comment, not a `///` doc: the struct's doc feeds schemars' `description` and editing it would
// move the schema hash.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[non_exhaustive]
pub struct Response {
    /// The correlation id of the request this answers.
    pub id: RequestId,
    /// The success-or-error outcome (flattened).
    #[serde(flatten)]
    pub result: WireResult,
}

impl Response {
    /// A success with no applied frame (e.g. a pure read that does not stamp a frame).
    pub fn ok(id: u64, body: ResponseBody) -> Self {
        Response {
            id: RequestId::new(id),
            result: WireResult::Ok {
                ok: body,
                frame: None,
                game_frame: None,
            },
        }
    }

    /// A success carrying the sim frame at which the command applied and the gameplay frame at
    /// the same boundary.
    pub fn ok_at(id: u64, body: ResponseBody, frame: FrameIndex, game_frame: FrameIndex) -> Self {
        Response {
            id: RequestId::new(id),
            result: WireResult::Ok {
                ok: body,
                frame: Some(frame),
                game_frame: Some(game_frame),
            },
        }
    }

    /// A failure carrying a structured diagnostic.
    pub fn error(id: u64, diagnostic: Diagnostic) -> Self {
        Response {
            id: RequestId::new(id),
            result: WireResult::Error { error: diagnostic },
        }
    }
}

/// The event envelope: one [`Event`] (its `event` name + `data`), plus `sub`, `seq`, and an optional
/// `frame`.
///
/// # Example
/// ```
/// use ludion_protocol::{Event, EventMessage};
/// use ludion_protocol::event::{LogEvent, LogLevel};
/// use ludion_protocol::envelope::{Seq, SubId};
///
/// let msg = EventMessage {
///     event: Event::Log(LogEvent { level: LogLevel::Info, message: "loaded".into(), diagnostic: None }),
///     sub: SubId::new(7),
///     seq: Seq::new(981),
///     frame: None,
/// };
/// let value = serde_json::to_value(&msg).unwrap();
/// assert_eq!(value["event"], serde_json::json!("log"));
/// assert_eq!(value["sub"], serde_json::json!(7));
/// assert_eq!(value["data"]["message"], serde_json::json!("loaded"));
/// ```
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct EventMessage {
    /// The event itself (flattened as `event` + `data`).
    #[serde(flatten)]
    pub event: Event,
    /// The subscription this event belongs to.
    pub sub: SubId,
    /// The per-subscription monotonic sequence number.
    pub seq: Seq,
    /// The sim frame the event describes, when applicable.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(with = "Option<u64>")]
    pub frame: Option<FrameIndex>,
}

/// Builds an `L3001` malformed-envelope diagnostic.
fn malformed(message: impl Into<String>) -> Diagnostic {
    Diagnostic::new(code::PROTOCOL_MALFORMED_ENVELOPE, message).with_help(
        "a request envelope is a JSON object `{\"v\": 1, \"id\": <number>, \"method\": \"<dotted \
         name>\", \"params\": {…}}`; `params` is omitted for parameterless methods",
    )
}

/// Builds an `L3002` unknown-method diagnostic, suggesting the nearest known method.
fn unknown_method(method: &str) -> Diagnostic {
    let mut diag = Diagnostic::new(
        code::PROTOCOL_UNKNOWN_METHOD,
        format!("`{method}` is not a method this protocol version defines"),
    )
    .with_path("/method");
    if let Some(near) = nearest_method(method) {
        diag = diag.with_help(format!("nearest known method is `{near}`"));
    } else {
        diag = diag
            .with_help("run `engine.describe` (or read the exported schema) for the method list");
    }
    diag
}

/// Builds an `L3003` unsupported-version diagnostic.
fn unsupported_version(got: &Value) -> Diagnostic {
    Diagnostic::new(
        code::PROTOCOL_UNSUPPORTED_VERSION,
        format!(
            "this engine speaks protocol version {PROTOCOL_VERSION}, but the envelope declared \
             version {got}"
        ),
    )
    .with_path("/v")
    .with_help(
        "regenerate the client from this engine's schema; protocol versions are breaking by design",
    )
}

/// The known method whose name is closest to `method` by edit distance, if any is reasonably close.
fn nearest_method(method: &str) -> Option<&'static str> {
    ludion_core::suggest::nearest(method, Command::METHODS.iter().copied())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::command::{EntitySpawnParams, SimStepParams};

    #[test]
    fn request_round_trips_the_spec_shape() {
        let request = Request::new(
            42,
            Command::EntitySpawn(EntitySpawnParams {
                name: None,
                components: Default::default(),
            }),
        );
        let value = serde_json::to_value(&request).unwrap();
        assert_eq!(
            value,
            serde_json::json!({ "v": 1, "id": 42, "method": "entity.spawn", "params": {} }),
        );
        assert_eq!(Request::from_value(value).unwrap(), request);
    }

    #[test]
    fn success_response_matches_spec() {
        let r = Response::ok_at(
            42,
            ResponseBody::Stepped { hash: None },
            FrameIndex::new(1203),
            FrameIndex::new(1150),
        );
        assert_eq!(
            serde_json::to_value(&r).unwrap(),
            serde_json::json!({
                "id": 42, "ok": { "kind": "stepped" }, "frame": 1203, "game_frame": 1150
            }),
        );
    }

    #[test]
    fn error_response_matches_spec() {
        let diag = Diagnostic::new(code::PROTOCOL_UNKNOWN_METHOD, "no such thing")
            .with_path("/method")
            .with_help("try entity.spawn");
        let r = Response::error(42, diag);
        let value = serde_json::to_value(&r).unwrap();
        assert_eq!(value["id"], serde_json::json!(42));
        assert_eq!(value["error"]["code"], serde_json::json!("L3002"));
        assert_eq!(value["error"]["path"], serde_json::json!("/method"));
        assert_eq!(
            value["error"]["help"],
            serde_json::json!("try entity.spawn")
        );
        assert!(value.get("ok").is_none());
        // Round-trips.
        assert_eq!(serde_json::from_value::<Response>(value).unwrap(), r);
    }

    #[test]
    fn ok_and_error_are_distinguished_on_deserialize() {
        let ok: Response = serde_json::from_value(
            serde_json::json!({ "id": 1, "ok": { "kind": "ack" }, "frame": 5 }),
        )
        .unwrap();
        // A reply without `game_frame` still parses.
        match ok.result {
            WireResult::Ok { game_frame, .. } => assert_eq!(game_frame, None),
            WireResult::Error { .. } => panic!("expected a success"),
        }
        let err: Response = serde_json::from_value(
            serde_json::json!({ "id": 1, "error": { "code": "L3001", "message": "x" } }),
        )
        .unwrap();
        assert!(matches!(err.result, WireResult::Error { .. }));
    }

    #[test]
    fn from_json_flags_unknown_method_as_l3002_with_suggestion() {
        let diag =
            Request::from_json(r#"{"v":1,"id":1,"method":"entity.spwn","params":{}}"#).unwrap_err();
        assert_eq!(diag.code, code::PROTOCOL_UNKNOWN_METHOD);
        assert!(diag.help.unwrap().contains("entity.spawn"));
    }

    #[test]
    fn envelope_rejects_a_bad_id_at_slash_id() {
        for (text, needle) in [
            (r#"{"v":1,"method":"sim.pause"}"#, "missing"),
            (r#"{"v":1,"id":"7","method":"sim.pause"}"#, "\"7\""),
            (r#"{"v":1,"id":-1,"method":"sim.pause"}"#, "-1"),
        ] {
            let diag = Request::from_json(text).unwrap_err();
            assert_eq!(diag.code, code::PROTOCOL_MALFORMED_ENVELOPE, "{text}");
            assert_eq!(diag.path.as_deref(), Some("/id"), "{text}");
            assert!(diag.message.contains(needle), "{text}: {}", diag.message);
        }
    }

    #[test]
    fn unsupported_version_reports_the_raw_value() {
        for (text, needle) in [
            (
                r#"{"v":4294967297,"id":1,"method":"sim.pause"}"#,
                "4294967297",
            ),
            (r#"{"v":"1","id":1,"method":"sim.pause"}"#, "\"1\""),
        ] {
            let diag = Request::from_json(text).unwrap_err();
            assert_eq!(diag.code, code::PROTOCOL_UNSUPPORTED_VERSION, "{text}");
            assert_eq!(diag.path.as_deref(), Some("/v"), "{text}");
            assert!(diag.message.contains(needle), "{text}: {}", diag.message);
        }
    }

    #[test]
    fn from_json_flags_bad_version_as_l3003() {
        let diag = Request::from_json(r#"{"v":2,"id":1,"method":"sim.pause"}"#).unwrap_err();
        assert_eq!(diag.code, code::PROTOCOL_UNSUPPORTED_VERSION);
    }

    #[test]
    fn from_json_flags_bad_params_as_l3001() {
        // `sim.step` needs a `ticks` number.
        let diag =
            Request::from_json(r#"{"v":1,"id":1,"method":"sim.step","params":{"ticks":"lots"}}"#)
                .unwrap_err();
        assert_eq!(diag.code, code::PROTOCOL_MALFORMED_ENVELOPE);
    }

    #[test]
    fn from_json_flags_missing_version_as_l3001() {
        let diag = Request::from_json(r#"{"id":1,"method":"sim.pause"}"#).unwrap_err();
        assert_eq!(diag.code, code::PROTOCOL_MALFORMED_ENVELOPE);
    }

    #[test]
    fn valid_step_request_parses() {
        let req = Request::from_json(r#"{"v":1,"id":9,"method":"sim.step","params":{"ticks":10}}"#)
            .unwrap();
        assert_eq!(
            req.command,
            Command::SimStep(SimStepParams {
                ticks: 10,
                hash: false
            })
        );
    }
}
