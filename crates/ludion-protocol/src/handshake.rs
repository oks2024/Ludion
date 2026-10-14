//! The connection handshake (engine-architecture.md §6).
//!
//! On connect the client sends [`Hello`]; the server replies with [`ServerHello`]. The reply carries
//! the [`schema_hash`](ServerHello::schema_hash) — the XXH3 digest of the exported protocol schema —
//! so a client generated from a stale schema detects the drift **at connect time**, not just in CI.
//! v1 supports exactly one protocol version; a mismatch is a hard, breaking failure by design.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::WireHash;

/// The client's opening message.
///
/// # Example
/// ```
/// use ludion_protocol::handshake::Hello;
/// use ludion_protocol::PROTOCOL_VERSION;
///
/// let hello = Hello {
///     protocol_version: PROTOCOL_VERSION,
///     client: "ludion-cli 0.0.0".into(),
///     features: vec![],
/// };
/// assert_eq!(serde_json::to_value(&hello).unwrap()["protocol_version"], serde_json::json!(1));
/// ```
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Hello {
    /// The protocol version the client was built against.
    pub protocol_version: u32,
    /// A human-facing client identifier (name + version), for logs.
    pub client: String,
    /// Optional feature flags the client advertises; unknown ones are ignored by the server.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub features: Vec<String>,
}

/// The server's reply to a [`Hello`].
///
/// # Example
/// ```
/// use ludion_protocol::handshake::ServerHello;
/// use ludion_protocol::{schema, PROTOCOL_VERSION};
///
/// let reply = ServerHello {
///     engine_version: "0.0.0".into(),
///     protocol_version: PROTOCOL_VERSION,
///     schema_hash: schema::schema_hash(),
/// };
/// assert_eq!(reply.schema_hash.to_string().len(), 16);
/// ```
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ServerHello {
    /// The engine build's version string.
    pub engine_version: String,
    /// The protocol version the server speaks (must match the client's for the session to proceed).
    pub protocol_version: u32,
    /// The hash of the server's exported protocol schema; a client compares it to its own.
    pub schema_hash: WireHash,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn handshake_round_trips() {
        let hello = Hello {
            protocol_version: crate::PROTOCOL_VERSION,
            client: "test 1".into(),
            features: vec!["frames".into()],
        };
        let back: Hello = serde_json::from_str(&serde_json::to_string(&hello).unwrap()).unwrap();
        assert_eq!(hello, back);

        let reply = ServerHello {
            engine_version: "0.0.0".into(),
            protocol_version: crate::PROTOCOL_VERSION,
            schema_hash: WireHash::from_u64(1),
        };
        let back: ServerHello =
            serde_json::from_str(&serde_json::to_string(&reply).unwrap()).unwrap();
        assert_eq!(reply, back);
    }
}
