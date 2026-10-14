#![forbid(unsafe_code)]
#![warn(missing_docs)]

//! `ludion-protocol` — the engine's command contract, in one place.
//!
//! This crate is the *contract layer* of the wire protocol (engine-architecture.md §6): the
//! [`Command`], [`ResponseBody`], and [`Event`] enums, the request/response/event [`envelope`]s, the
//! [`handshake`], the binary [`frame`] header, the [`replay`] session format, and a deterministic
//! [`schema`] export with a stable [`schema_hash`](schema::schema_hash). It is where the parity gate
//! is *defined*: a capability that has no [`Command`] variant here does not exist. The dispatcher
//! (`ludion-engine`) and every client (CLI, editor, MCP) are downstream of this one source of truth.
//!
//! # Boundary
//!
//! `ludion-protocol` depends on `ludion-core` **only** — never on `ludion-ecs`, `ludion-scene`,
//! `ludion-sim`, `ludion-engine`, or the renderer. The rule that keeps it that way: **the protocol
//! never names a concrete component type.** Component values, spawn payloads, query results, input
//! events, snapshots, and graph/timing dumps all travel as [`serde_json::Value`] plus a registry
//! *name* (a `String`); they are validated at dispatch against the component registry, not here
//! (engine-architecture.md §2). The moment `Transform` appeared in a struct in this crate, a
//! protocol→ecs dependency cycle would begin.
//!
//! # The three artifacts, one source
//!
//! Every type here derives `serde` **and** [`schemars::JsonSchema`]. That single derive powers three
//! artifacts that can never drift: the Rust dispatch match, the exported JSON Schema
//! ([`schema::protocol_schema`]), and (downstream, in CI) the generated TypeScript client types. The
//! [`schema_hash`](schema::schema_hash) is announced in the [`handshake`](handshake::ServerHello) so
//! a client built from a stale schema detects the mismatch *at connect time*, not just in CI.
//!
//! # Example
//!
//! ```
//! use ludion_protocol::{Command, Request, PROTOCOL_VERSION};
//! use ludion_protocol::command::SimStepParams;
//!
//! // A request is the versioned envelope around one command.
//! let request = Request::new(42, Command::SimStep(SimStepParams { ticks: 10, hash: true }));
//! let json = serde_json::to_value(&request).unwrap();
//! assert_eq!(json, serde_json::json!({
//!     "v": 1,
//!     "id": 42,
//!     "method": "sim.step",
//!     "params": { "ticks": 10, "hash": true },
//! }));
//! assert_eq!(request.command.method(), "sim.step");
//! assert_eq!(PROTOCOL_VERSION, 1);
//! ```

pub mod command;
pub mod envelope;
pub mod event;
pub mod frame;
pub mod handshake;
pub mod hash;
pub mod perf;
pub mod profile;
pub mod replay;
pub mod response;
pub mod restore;
pub mod schema;

pub use command::{Channel, Command, SubscribeMode};
pub use envelope::{EventMessage, Request, RequestId, Response, Seq, SubId, WireResult};
pub use event::Event;
pub use hash::WireHash;
pub use response::ResponseBody;

/// The one wire-protocol version this engine build speaks.
///
/// v1 supports exactly one protocol version; a bump is a breaking change by design
/// (engine-architecture.md §6). A [`Request`] whose envelope `v` is not this value is rejected with
/// `L3003` by [`Request::from_value`].
pub const PROTOCOL_VERSION: u32 = 1;

/// The largest wire message, in bytes, either end sends or accepts (256 MiB); past it a message is
/// refused with `L3045`.
///
/// Sized from the engine's own limits: an 8K frame in its worst encoding (QOI, ~166 MB) and an 8K
/// screenshot as base64 PNG (~177 MB) both fit. Snapshots are not bounded by the engine.
pub const MAX_MESSAGE_BYTES: usize = 256 * 1024 * 1024;

/// Private JSON-Schema proxies for `ludion-core` types that do not derive [`schemars::JsonSchema`].
///
/// `ludion-core` deliberately does not depend on `schemars` (it is a dependency-light foundation), so
/// [`ludion_core::ids::EntityId`] and [`ludion_core::diagnostics::Diagnostic`] cannot derive the
/// trait. Fields typed as those reference a proxy here via `#[schemars(with = "...")]`; the proxy's
/// schema mirrors the real type's serde shape exactly. The proxies are never constructed — they exist
/// only to describe a schema — so they carry `#[allow(dead_code)]`.
pub(crate) mod schema_proxy {
    use schemars::JsonSchema;

    /// Mirrors [`ludion_core::ids::EntityId`]'s serde form: `{ "index": u32, "generation": u32 }`.
    #[derive(JsonSchema)]
    #[allow(dead_code)]
    pub(crate) struct EntityIdSchema {
        pub index: u32,
        pub generation: u32,
    }

    /// Mirrors [`ludion_core::diagnostics::Severity`]'s serde form: the lowercase strings `"error"`
    /// and `"warning"`.
    #[derive(JsonSchema)]
    #[schemars(rename_all = "lowercase")]
    #[allow(dead_code)]
    pub(crate) enum SeveritySchema {
        Error,
        Warning,
    }

    /// Mirrors [`ludion_core::diagnostics::Diagnostic`]'s serde form (the wire error object): a
    /// `code`, a `message`, an optional `severity` / `path` / `help`, and any number of `related`
    /// sub-diagnostics.
    ///
    /// `severity` is absent on an ordinary error (the core type skips it when it is `Error`), so a
    /// client that ignores the field reads every error exactly as it did before severity existed;
    /// only a lint carries it.
    #[derive(JsonSchema)]
    #[allow(dead_code)]
    pub(crate) struct WireErrorSchema {
        pub code: String,
        pub severity: Option<SeveritySchema>,
        pub message: String,
        pub path: Option<String>,
        pub help: Option<String>,
        #[serde(default)]
        pub related: Vec<WireErrorSchema>,
    }
}

#[cfg(test)]
mod tests {
    use ludion_core::codes::code;
    use ludion_core::diagnostics::{Diagnostic, Severity};
    use ludion_core::ids::EntityId;

    /// The property names a proxy schema declares, sorted.
    fn proxy_properties<T: schemars::JsonSchema>() -> Vec<String> {
        let schema = serde_json::to_value(schemars::schema_for!(T)).unwrap();
        let mut names: Vec<String> = schema["properties"]
            .as_object()
            .expect("proxy schemas are plain objects with properties")
            .keys()
            .cloned()
            .collect();
        names.sort();
        names
    }

    /// The keys a value actually serializes with, sorted.
    fn serialized_keys(value: &impl serde::Serialize) -> Vec<String> {
        let mut keys: Vec<String> = serde_json::to_value(value)
            .unwrap()
            .as_object()
            .expect("the mirrored types serialize as objects")
            .keys()
            .cloned()
            .collect();
        keys.sort();
        keys
    }

    // The schema_proxy mirrors exist because ludion-core does not derive JsonSchema; nothing ties
    // them to the real serde shapes at compile time, so these tests do — a field added to or renamed
    // on the core type without the proxy following is a silent schema lie otherwise.

    #[test]
    fn entity_id_proxy_matches_the_real_serde_shape() {
        assert_eq!(
            serialized_keys(&EntityId::new(7, 1)),
            proxy_properties::<crate::schema_proxy::EntityIdSchema>(),
        );
    }

    #[test]
    fn wire_error_proxy_matches_the_real_serde_shape() {
        // Populate every optional field so skip_serializing_if hides nothing — including
        // `severity`, which only a warning emits.
        let mut full = Diagnostic::new(code::PROTOCOL_MALFORMED_ENVELOPE, "message")
            .with_severity(Severity::Warning)
            .with_path("/params")
            .with_help("help");
        full.related.push(Diagnostic::new(
            code::PROTOCOL_MALFORMED_ENVELOPE,
            "related",
        ));
        assert_eq!(
            serialized_keys(&full),
            proxy_properties::<crate::schema_proxy::WireErrorSchema>(),
        );
    }

    #[test]
    fn wire_error_proxy_required_matches_a_minimal_error() {
        let schema =
            serde_json::to_value(schemars::schema_for!(crate::schema_proxy::WireErrorSchema))
                .unwrap();
        let mut required: Vec<String> = schema["required"]
            .as_array()
            .expect("the proxy schema lists its required fields")
            .iter()
            .map(|name| name.as_str().unwrap().to_string())
            .collect();
        required.sort();
        assert_eq!(
            serialized_keys(&Diagnostic::new(code::PROTOCOL_MALFORMED_ENVELOPE, "m")),
            required,
            "a minimal error serializes exactly the fields the schema requires"
        );
    }

    /// The whole point of `skip_serializing_if`: an ordinary error's payload did not change when
    /// severity was added, so a client that never learns the field still reads every error.
    #[test]
    fn an_error_carries_no_severity_on_the_wire() {
        let error = Diagnostic::new(code::PROTOCOL_MALFORMED_ENVELOPE, "message");
        assert!(!serialized_keys(&error).contains(&"severity".to_string()));
    }
}
