//! The engine connection: lazy connect, probe-then-reconnect, and a status probe.
//!
//! Startup never touches the network — a [`Session`] holds no client until the first `tools/call`
//! needs one, so `tools/list` works with no engine running. Before each call a held client is probed
//! for a closed connection; only then does the session reconnect, so a transient `ludion serve`
//! restart is invisible to the model. A command is never re-sent: a call that fails is returned, and
//! the dead client is dropped so the *next* call reconnects.

use ludion_client::{CallOk, Client};
use ludion_core::codes::code;
use ludion_core::diagnostics::Diagnostic;
use ludion_protocol::Command;

/// A lazily-connected wire session to one `ludion serve` endpoint.
pub(crate) struct Session {
    url: String,
    client: Option<Client>,
}

impl Session {
    /// A session that will connect to `url` on first use.
    pub(crate) fn new(url: String) -> Self {
        Session { url, client: None }
    }

    /// The endpoint URL this session targets.
    pub(crate) fn url(&self) -> &str {
        &self.url
    }

    /// Sends `command` once, connecting lazily and reconnecting first if the held connection is
    /// already closed.
    ///
    /// The reconnect decision is made *before* anything is written (`Client::check_open`), so it is
    /// provably safe; a call that fails after a clean probe is returned, never retried — the command
    /// may have run.
    ///
    /// # Errors
    /// The engine's own [`Diagnostic`] (rendered verbatim into the tool result), or `L3009` if the
    /// engine is unreachable or the connection was lost during the call.
    pub(crate) fn call(&mut self, command: Command) -> Result<CallOk, Diagnostic> {
        if let Some(client) = self.client.as_mut()
            && client.check_open().is_err()
        {
            self.client = None;
        }
        self.ensure_connected()?;
        let result = self
            .client
            .as_mut()
            .expect("ensure_connected left a client")
            .call(command);
        if let Err(diagnostic) = &result
            && diagnostic.code == code::PROTOCOL_ENGINE_UNAVAILABLE
        {
            self.client = None;
        }
        result
    }

    /// Connects if not already connected. A no-op when a live client is held.
    ///
    /// # Errors
    /// `L3009` (from [`Client::connect`]) if the engine cannot be reached, or `L3003` on schema /
    /// protocol-version drift.
    fn ensure_connected(&mut self) -> Result<(), Diagnostic> {
        if self.client.is_none() {
            self.client = Some(Client::connect(&self.url)?);
        }
        Ok(())
    }

    /// Reconnecting status probe: the server hello plus a `sim.hash` (its current frame + WorldHash).
    ///
    /// # Errors
    /// `L3009` if the engine is unreachable; any diagnostic `sim.hash` reports.
    pub(crate) fn status(&mut self) -> Result<Status, Diagnostic> {
        let ok = self.call(Command::SimHash)?;
        let client = self.client.as_ref().expect("call left a client connected");
        let hello = client.server_hello();
        let hash = match &ok.body {
            ludion_protocol::ResponseBody::SimHash { hash } => hash.to_string(),
            _ => "?".to_string(),
        };
        Ok(Status {
            url: self.url.clone(),
            engine_version: hello.engine_version.clone(),
            protocol_version: hello.protocol_version,
            schema_hash: hello.schema_hash.to_string(),
            frame: ok.frame.map(|f| f.as_u64()),
            game_frame: ok.game_frame.map(|f| f.as_u64()),
            world_hash: hash,
        })
    }
}

/// A snapshot of connection health, returned by the `ludion_status` tool.
pub(crate) struct Status {
    /// The engine URL this adapter targets.
    pub(crate) url: String,
    /// The engine build version from the handshake.
    pub(crate) engine_version: String,
    /// The protocol version the engine speaks.
    pub(crate) protocol_version: u32,
    /// The engine's exported-schema hash (equal to this build's, or connect would have failed).
    pub(crate) schema_hash: String,
    /// The current sim frame, if the response carried one.
    pub(crate) frame: Option<u64>,
    /// The current gameplay frame (the sim frame less every frame the game has held).
    pub(crate) game_frame: Option<u64>,
    /// The current WorldHash.
    pub(crate) world_hash: String,
}

#[cfg(test)]
mod tests {
    use std::net::TcpListener;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::thread;

    use ludion_protocol::handshake::ServerHello;
    use ludion_protocol::{PROTOCOL_VERSION, Response, ResponseBody, schema};
    use tungstenite::Message;

    use super::*;

    /// Handshakes one accepted connection, returning its socket.
    fn handshake(stream: std::net::TcpStream) -> tungstenite::WebSocket<std::net::TcpStream> {
        let mut socket = tungstenite::accept(stream).expect("ws handshake");
        let _hello = socket.read();
        let server_hello = ServerHello {
            engine_version: "0.0.0".into(),
            protocol_version: PROTOCOL_VERSION,
            schema_hash: schema::schema_hash(),
        };
        socket
            .send(Message::text(serde_json::to_string(&server_hello).unwrap()))
            .expect("send ServerHello");
        socket
    }

    /// A stub that accepts every connection, counts each request it reads, and closes without replying.
    fn spawn_dropping_server(requests: Arc<AtomicUsize>) -> String {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind stub listener");
        let addr = listener.local_addr().unwrap();
        thread::spawn(move || {
            for stream in listener.incoming() {
                let mut socket = handshake(stream.expect("accept"));
                if let Ok(Message::Text(_)) = socket.read() {
                    requests.fetch_add(1, Ordering::SeqCst);
                }
                drop(socket);
            }
        });
        format!("ws://{addr}")
    }

    /// A command whose connection dies mid-call is reported, never re-sent.
    #[test]
    fn a_call_lost_mid_flight_is_not_resent() {
        let requests = Arc::new(AtomicUsize::new(0));
        let url = spawn_dropping_server(requests.clone());
        let mut session = Session::new(url);
        let error = session
            .call(Command::SimHash)
            .expect_err("no reply ever comes");
        assert_eq!(error.code, code::PROTOCOL_ENGINE_UNAVAILABLE);
        assert_eq!(
            requests.load(Ordering::SeqCst),
            1,
            "the command was re-sent"
        );
    }

    /// A stub that answers one request per connection, then closes it; counts every request read.
    fn spawn_restarting_server(requests: Arc<AtomicUsize>) -> String {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind stub listener");
        let addr = listener.local_addr().unwrap();
        thread::spawn(move || {
            for stream in listener.incoming() {
                let mut socket = handshake(stream.expect("accept"));
                if let Ok(Message::Text(text)) = socket.read() {
                    requests.fetch_add(1, Ordering::SeqCst);
                    let id = serde_json::from_str::<serde_json::Value>(&text).unwrap()["id"]
                        .as_u64()
                        .unwrap();
                    let zero = ludion_core::time::FrameIndex::ZERO;
                    let reply = Response::ok_at(id, ResponseBody::Ack, zero, zero);
                    socket
                        .send(Message::text(serde_json::to_string(&reply).unwrap()))
                        .expect("send reply");
                }
                let _ = socket.close(None);
                let _ = socket.flush();
            }
        });
        format!("ws://{addr}")
    }

    /// A connection that closed between calls is replaced before the next send, with no re-send.
    #[test]
    fn a_connection_closed_between_calls_is_reconnected_before_sending() {
        let requests = Arc::new(AtomicUsize::new(0));
        let url = spawn_restarting_server(requests.clone());
        let mut session = Session::new(url);
        session.call(Command::SimHash).expect("first call");
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
        while session
            .client
            .as_mut()
            .expect("a client is held")
            .check_open()
            .is_ok()
        {
            assert!(
                std::time::Instant::now() < deadline,
                "the close never arrived"
            );
            thread::sleep(std::time::Duration::from_millis(5));
        }
        session
            .call(Command::SimHash)
            .expect("second call reconnects");
        assert_eq!(requests.load(Ordering::SeqCst), 2);
    }
}
