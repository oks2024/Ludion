//! A minimal blocking wire-protocol client, host-side.
//!
//! The stage session (see [`crate::stage`]) needs to say exactly two things to the `ludion serve`
//! it owns — `asset.reload {path}` when a watched model changes, and `engine.shutdown` on
//! teardown — from the Tauri host, where the frontend's TS client is not reachable and may not
//! even be mounted. That is a handshake plus a request/response, so this hand-rolls the envelope
//! rather than path-depending on `ludion-client`: pulling `ludion-protocol` into this separate
//! cargo workspace would drag schemars + zstd + the whole schema-hash machinery into the editor
//! build for two commands' worth of JSON.
//!
//! The one thing given up is `ludion-client`'s connect-time schema-drift check. It costs nothing
//! here: these two methods have no params drift to detect (`asset.reload`'s single optional
//! `path`, `engine.shutdown`'s none), and a stale editor talking to a fresh engine would fail
//! loudly on the response anyway.

use std::io::{Read, Write};
use std::net::{TcpStream, ToSocketAddrs};
use std::time::Duration;

use serde_json::{Value, json};
use tungstenite::protocol::WebSocketConfig;
use tungstenite::{Message, WebSocket};

/// The protocol version this host speaks. One version exists (`ludion_protocol::PROTOCOL_VERSION`);
/// a mismatch is a hard server-side error by design, and the message says so.
const PROTOCOL_VERSION: u32 = 1;

/// The largest wire message this host accepts; mirrors `ludion_protocol::MAX_MESSAGE_BYTES` (this
/// crate has no ludion dependencies), in place of tungstenite's 16 MiB default.
const MAX_MESSAGE_BYTES: usize = 256 * 1024 * 1024;

fn websocket_config() -> WebSocketConfig {
    WebSocketConfig {
        max_message_size: Some(MAX_MESSAGE_BYTES),
        max_frame_size: Some(MAX_MESSAGE_BYTES),
        ..WebSocketConfig::default()
    }
}

/// A connected, handshaken socket to one `ludion serve`. Blocking; dropping it closes the socket.
pub struct WireClient {
    socket: WebSocket<TcpStream>,
    next_id: u64,
    /// The `ServerHello` the handshake answered with, kept raw (see the module docs on why this
    /// crate does not depend on `ludion-protocol`'s typed `ServerHello`).
    hello: Value,
}

/// Split `ws://host:port` into the address to dial. Anything else is a caller bug worth naming.
fn socket_addr(url: &str) -> Result<std::net::SocketAddr, String> {
    let hostport = url.strip_prefix("ws://").ok_or_else(|| {
        format!("`{url}` is not a ws:// url (the stage session announces one on startup)")
    })?;
    hostport
        .to_socket_addrs()
        .map_err(|e| format!("cannot resolve `{hostport}`: {e}"))?
        .next()
        .ok_or_else(|| format!("`{hostport}` resolved to no address"))
}

impl WireClient {
    /// Dial `url`, run the `Hello` handshake, and return once the server has answered.
    ///
    /// `timeout` bounds the TCP connect *and* every subsequent read/write, so a wedged engine can
    /// never park the caller's thread indefinitely — the watcher and the teardown path both run on
    /// borrowed threads.
    pub fn connect(url: &str, timeout: Duration) -> Result<WireClient, String> {
        let addr = socket_addr(url)?;
        let stream = TcpStream::connect_timeout(&addr, timeout)
            .map_err(|e| format!("cannot reach the stage engine at {url}: {e}"))?;
        stream
            .set_read_timeout(Some(timeout))
            .map_err(|e| e.to_string())?;
        stream
            .set_write_timeout(Some(timeout))
            .map_err(|e| e.to_string())?;
        let (mut socket, _response) =
            tungstenite::client::client_with_config(url, stream, Some(websocket_config()))
                .map_err(|e| format!("websocket handshake with {url} failed: {e}"))?;

        let hello = json!({
            "protocol_version": PROTOCOL_VERSION,
            "client": concat!("ludion-editor-host ", env!("CARGO_PKG_VERSION")),
        });
        socket
            .send(Message::text(hello.to_string()))
            .map_err(|e| format!("could not send the handshake to {url}: {e}"))?;
        let reply = read_text(&mut socket, url)?;
        // The server answers a bad handshake with an error envelope and closes; surface its words.
        if let Some(message) = error_message(&reply) {
            return Err(format!("{url} rejected the handshake: {message}"));
        }
        Ok(WireClient {
            socket,
            next_id: 1,
            hello: reply,
        })
    }

    /// The engine build's version string the handshake announced (`ServerHello::engine_version`),
    /// or `None` if the reply's shape was ever not what the handshake guarantees.
    pub fn engine_version(&self) -> Option<&str> {
        self.hello.get("engine_version").and_then(Value::as_str)
    }

    /// Send one command and block for its response. `Ok(())` means the engine accepted it.
    pub fn call(&mut self, method: &str, params: Option<Value>) -> Result<(), String> {
        let id = self.next_id;
        self.next_id += 1;
        let mut request = json!({ "v": PROTOCOL_VERSION, "id": id, "method": method });
        if let Some(params) = params {
            request["params"] = params;
        }
        self.socket
            .send(Message::text(request.to_string()))
            .map_err(|e| format!("could not send `{method}`: {e}"))?;

        // Events and frames can interleave with the reply (nothing here subscribes, but the server
        // is free to push), so read until the envelope carrying our own id arrives.
        loop {
            let value = read_text(&mut self.socket, method)?;
            if value.get("id").and_then(Value::as_u64) != Some(id) {
                continue;
            }
            return match error_message(&value) {
                Some(message) => Err(format!("`{method}` was rejected: {message}")),
                None => Ok(()),
            };
        }
    }

    /// Close politely. A failure here is not worth reporting — the socket is going away regardless.
    pub fn close(mut self) {
        let _ = self.socket.close(None);
    }
}

/// The `error.message` of a response envelope, or `None` when it is not an error.
fn error_message(value: &Value) -> Option<String> {
    let error = value.get("error")?;
    Some(
        error
            .get("message")
            .and_then(Value::as_str)
            .map(str::to_string)
            .unwrap_or_else(|| error.to_string()),
    )
}

/// Read the next text message, skipping binary frames and pings. `what` names the operation in the
/// error so a timeout says which call stalled.
fn read_text<S: Read + Write>(socket: &mut WebSocket<S>, what: &str) -> Result<Value, String> {
    loop {
        match socket.read() {
            Ok(Message::Text(text)) => {
                return serde_json::from_str(&text)
                    .map_err(|e| format!("`{what}`: the engine sent malformed JSON: {e}"));
            }
            Ok(Message::Close(_)) => {
                return Err(format!("`{what}`: the stage engine closed the connection"));
            }
            Ok(_) => continue, // binary frames / pongs: not ours
            Err(e) => return Err(format!("`{what}`: {e}")),
        }
    }
}

/// Connect, send one command, close. The whole host-side wire usage is one-shot bursts (a debounced
/// batch of reloads, a shutdown), so a persistent socket would only add a reconnect state machine.
pub fn send_one(
    url: &str,
    method: &str,
    params: Option<Value>,
    timeout: Duration,
) -> Result<(), String> {
    let mut client = WireClient::connect(url, timeout)?;
    let result = client.call(method, params);
    client.close();
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn socket_addr_requires_a_ws_url() {
        assert!(socket_addr("ws://127.0.0.1:9411").is_ok());
        let err = socket_addr("http://127.0.0.1:9411").unwrap_err();
        assert!(err.contains("not a ws:// url"), "{err}");
    }

    #[test]
    fn error_envelopes_yield_their_message() {
        let ok = serde_json::json!({ "id": 1, "ok": { "kind": "ack" }, "frame": 12 });
        assert_eq!(error_message(&ok), None);
        let err = serde_json::json!({ "id": 1, "error": { "code": "L3002", "message": "unknown method" } });
        assert_eq!(error_message(&err).as_deref(), Some("unknown method"));
        // A shape we do not recognize still surfaces something actionable rather than nothing.
        let odd = serde_json::json!({ "id": 1, "error": "boom" });
        assert_eq!(error_message(&odd).as_deref(), Some("\"boom\""));
    }

    /// Pins [`PROTOCOL_VERSION`] to the engine's own constant by reading it out of
    /// `ludion-protocol`'s source.
    ///
    /// This crate is a separate cargo workspace on purpose (see the module docs), so there is no
    /// `ludion_protocol::PROTOCOL_VERSION` to `const_assert` against and no build script that could
    /// see one without taking the dependency the separation exists to avoid. Reading the source text
    /// is the cheapest link that still *fails* on a bump — and it mirrors what the frontend's
    /// `parity.test.ts` already does for the method union. A version bump therefore lands here as a
    /// red test naming both numbers, not as a silent handshake rejection at runtime.
    #[test]
    fn protocol_version_matches_the_engine() {
        let source = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../../crates/ludion-protocol/src/lib.rs");
        let text = std::fs::read_to_string(&source)
            .unwrap_or_else(|e| panic!("cannot read {}: {e}", source.display()));
        let engine: u32 = text
            .split_once("pub const PROTOCOL_VERSION: u32 =")
            .and_then(|(_, rest)| rest.split_once(';'))
            .map(|(value, _)| value.trim())
            .unwrap_or_else(|| {
                panic!(
                    "`pub const PROTOCOL_VERSION: u32 = <n>;` no longer appears in {} — this pin \
                     needs updating to wherever the constant moved",
                    source.display()
                )
            })
            .parse()
            .expect("PROTOCOL_VERSION is a plain integer literal");
        assert_eq!(
            PROTOCOL_VERSION, engine,
            "the editor host speaks protocol version {PROTOCOL_VERSION} but the engine speaks \
             {engine}; update `wire.rs`'s constant (and re-check the two methods it sends)"
        );
    }
}
