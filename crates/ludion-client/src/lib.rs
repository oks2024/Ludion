#![forbid(unsafe_code)]
#![warn(missing_docs)]

//! `ludion-client` — a typed, synchronous WebSocket client for the Ludion protocol.
//!
//! This is the thin peer client the CLI, MCP adapter, and tests speak through. It depends on
//! `ludion-protocol` (the contract) and nothing else engine-side: **the client never depends on
//! `ludion-engine` or `ludion-server`** (engine-architecture.md §2). It knows the wire shapes and
//! the handshake; it does not know how the world is computed.
//!
//! # Connect-time drift check
//!
//! [`Client::connect`] performs the [`Hello`] handshake and
//! verifies two things against the server's reply before returning: the protocol version matches,
//! and the server's `schema_hash` equals this build's [`schema::schema_hash`]. A schema mismatch is
//! a hard `L3003` error at connect time — a client generated from a stale schema finds out
//! immediately, not through a confusing failure three commands later. This is strict by design;
//! there is no lenient mode.
//!
//! # Blocking
//!
//! Every call blocks the calling thread until the matching response arrives. [`Client::call`] is the
//! primitive; a few typed conveniences ([`sim_hash`](Client::sim_hash),
//! [`sim_step`](Client::sim_step), [`screenshot`](Client::screenshot),
//! [`subscribe`](Client::subscribe), [`unsubscribe`](Client::unsubscribe)) wrap it. Request ids are
//! allocated internally and never collide within a client.
//!
//! The connect handshake is bounded ([`HANDSHAKE_TIMEOUT`], or [`Client::connect_within`]); after it,
//! by default a call blocks forever — the sim is single-threaded and dispatch is synchronous, so an
//! infinite-looping game system would otherwise hang every connected client (editor, MCP adapter,
//! window) with no diagnostic, unable even to process a wire `engine.shutdown`. An **opt-in** per-client
//! call timeout ([`with_call_timeout`](Client::with_call_timeout)) bounds the wait and surfaces `L3054`
//! on expiry; omit it and behavior is exactly as before (block forever). It is a transport-side guard,
//! not a sim-side watchdog — the engine keeps running; only this client stops waiting.
//!
//! # Streams
//!
//! On a subscribed connection the server can push two more message shapes at any time: JSON
//! [`EventMessage`]s on a subscribed channel, and binary [`ludion_protocol::frame`] messages on a
//! `frames` subscription. [`Client::call`] never drops these while it waits for its own response —
//! it queues whichever kind it isn't looking for internally, so nothing pushed during a call is
//! lost. [`Client::next_event`] and [`Client::next_frame`] drain those queues (and, once they're
//! empty, block on the socket up to a timeout) to consume that traffic. The queues are bounded:
//! frames are latest-wins per subscription (a newer frame replaces the queued one), and events keep
//! at most [`MAX_BUFFERED_EVENTS`], dropping the oldest and counting it in
//! [`Client::dropped_events`]. Traffic for a subscription this client has unsubscribed is never
//! handed back, even if it was in flight when the unsubscribe was acknowledged.
//!
//! # Example
//! ```no_run
//! use ludion_client::Client;
//! use ludion_protocol::Command;
//!
//! let mut client = Client::connect("ws://127.0.0.1:4321")?;
//! println!("connected to engine, schema {}", client.schema_hash());
//!
//! // The primitive: send any command, get its typed body and applied frame back.
//! let ok = client.call(Command::SimHash)?;
//! println!("world hash response: {:?}", ok.body);
//!
//! // Or a typed convenience.
//! let hash = client.sim_hash()?;
//! println!("world hash: {hash}");
//! # Ok::<(), ludion_core::diagnostics::Diagnostic>(())
//! ```

use std::collections::{BTreeSet, VecDeque};
use std::io::ErrorKind;
use std::net::{SocketAddr, TcpStream, ToSocketAddrs};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use ludion_core::codes::code;
use ludion_core::diagnostics::Diagnostic;
use ludion_core::time::FrameIndex;
use ludion_protocol::command::{
    CameraChoice, RenderScreenshotParams, SimStepParams, StreamSubscribeParams,
    StreamUnsubscribeParams,
};
use ludion_protocol::frame::{FRAME_HEADER_LEN, FrameFormat, FrameHeader};
use ludion_protocol::handshake::{Hello, ServerHello};
use ludion_protocol::{
    Channel, Command, EventMessage, MAX_MESSAGE_BYTES, PROTOCOL_VERSION, Request, Response,
    ResponseBody, SubId, SubscribeMode, WireHash, WireResult, schema,
};
use serde_json::Value;
use tungstenite::error::CapacityError;
use tungstenite::protocol::WebSocketConfig;
use tungstenite::stream::MaybeTlsStream;
use tungstenite::{Message, WebSocket};

/// How long [`Client::connect`] waits on each handshake stage (TCP connect, WebSocket upgrade,
/// `ServerHello`) before failing `L3009`; matches the server's own handshake deadline.
pub const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(10);

/// Every address a `ws://host:port[/path]` url resolves to, in resolver order, or `None` if it is
/// not one or does not resolve.
fn socket_addrs(url: &str) -> Option<Vec<SocketAddr>> {
    let hostport = url.strip_prefix("ws://")?;
    let hostport = hostport.split('/').next().unwrap_or(hostport);
    hostport.to_socket_addrs().ok().map(Iterator::collect)
}

/// Connects to the first of `addrs` that accepts within `timeout`; the last error otherwise.
fn connect_any(addrs: &[SocketAddr], timeout: Duration) -> std::io::Result<TcpStream> {
    let mut last = None;
    for addr in addrs {
        match TcpStream::connect_timeout(addr, timeout) {
            Ok(stream) => return Ok(stream),
            Err(error) => last = Some(error),
        }
    }
    Err(last.unwrap_or_else(|| ErrorKind::AddrNotAvailable.into()))
}

/// A successful call's result: the typed response body plus the sim frame it applied at.
///
/// `frame` is present for essentially every command (the dispatcher stamps the engine's frame on
/// every success); it is `None` only if a future command deliberately omits it.
#[derive(Clone, Debug, PartialEq)]
pub struct CallOk {
    /// The command's success payload.
    pub body: ResponseBody,
    /// The sim frame the command applied at, when the response carried one.
    pub frame: Option<FrameIndex>,
    /// The gameplay frame at the same boundary: what a presentation `started` is stamped with.
    pub game_frame: Option<FrameIndex>,
}

/// A connected, handshaken protocol client over one WebSocket.
///
/// Build with [`Client::connect`]; every method blocks until its response arrives. Dropping the
/// client closes the socket.
pub struct Client {
    socket: WebSocket<MaybeTlsStream<TcpStream>>,
    server_hello: ServerHello,
    next_id: u64,
    /// Events and frames buffered while a [`call`](Client::call) or a `next_*` was reading the socket
    /// for something else. Drained first by [`next_event`](Client::next_event) /
    /// [`next_frame`](Client::next_frame).
    inbox: Inbox,
    /// The optional per-call timeout. `None` (the default) blocks forever, exactly as before; `Some`
    /// bounds every [`call`](Client::call)'s wait for its reply and returns `L3054` on expiry. Set via
    /// [`with_call_timeout`](Client::with_call_timeout).
    call_timeout: Option<Duration>,
    /// Set when the transport reported an over-limit inbound message: the stream is mid-frame and the
    /// socket shut down, so every later call fails `L3009` before writing anything.
    lost: AtomicBool,
    /// Ids of `stream.subscribe` calls that timed out. If the host answers one late, the sub it
    /// minted is unsubscribed (see [`stray_reply`](Client::stray_reply)) instead of leaking.
    abandoned_subscribes: Vec<u64>,
}

impl Client {
    /// Connects to a `ws://host:port` URL, performs the handshake, and verifies protocol + schema.
    ///
    /// # Errors
    /// - `L3009` if the server cannot be reached, the connection fails, or the handshake does not
    ///   complete within [`HANDSHAKE_TIMEOUT`].
    /// - `L3051` if the server's handshake reply is not a well-formed [`ServerHello`].
    /// - `L3003` if the server speaks a different protocol version, announces a different
    ///   `schema_hash` (schema drift), or rejects the handshake with its own version error.
    pub fn connect(url: &str) -> Result<Client, Diagnostic> {
        Self::connect_within(url, HANDSHAKE_TIMEOUT)
    }

    /// [`connect`](Client::connect) with the TCP connect, the WebSocket upgrade and the `ServerHello`
    /// read each bounded by `timeout`. Once the handshake succeeds the socket's timeouts are cleared:
    /// calls block as [`with_call_timeout`](Client::with_call_timeout) says.
    ///
    /// # Errors
    /// As [`connect`](Client::connect); `L3009` also when the server accepts the connection but does
    /// not complete the handshake within `timeout`.
    pub fn connect_within(url: &str, timeout: Duration) -> Result<Client, Diagnostic> {
        let unreachable = |detail: String| {
            Diagnostic::new(
                code::PROTOCOL_ENGINE_UNAVAILABLE,
                format!("could not connect to {url}: {detail}"),
            )
            .with_help("check the server is running and the ws:// address and port are correct")
        };
        let stalled = || {
            Diagnostic::new(
                code::PROTOCOL_ENGINE_UNAVAILABLE,
                format!(
                    "the server at {url} accepted the connection but did not complete the \
                     handshake within {timeout:?}"
                ),
            )
            .with_help(
                "the engine may be suspended or wedged; restart it, or retry once it is responsive",
            )
        };

        let addrs =
            socket_addrs(url).ok_or_else(|| unreachable("not a ws://host:port url".into()))?;
        let stream = connect_any(&addrs, timeout).map_err(|e| unreachable(e.to_string()))?;
        stream
            .set_read_timeout(Some(timeout))
            .and_then(|()| stream.set_write_timeout(Some(timeout)))
            .map_err(io_lost)?;
        let (mut socket, _response) = tungstenite::client::client_with_config(
            url,
            MaybeTlsStream::Plain(stream),
            Some(websocket_config()),
        )
        .map_err(|e| match e {
            tungstenite::HandshakeError::Interrupted(_) => stalled(),
            tungstenite::HandshakeError::Failure(tungstenite::Error::Io(io)) if is_timeout(&io) => {
                stalled()
            }
            tungstenite::HandshakeError::Failure(error) => unreachable(error.to_string()),
        })?;

        let hello = Hello {
            protocol_version: PROTOCOL_VERSION,
            client: concat!("ludion-client ", env!("CARGO_PKG_VERSION")).to_string(),
            features: Vec::new(),
        };
        let lost = AtomicBool::new(false);
        send_text(
            &mut socket,
            &lost,
            serde_json::to_string(&hello).map_err(serialize_err)?,
        )?;

        let reply = loop {
            match socket.read() {
                Ok(Message::Text(text)) => break text,
                Ok(Message::Close(_)) => return Err(closed()),
                Ok(_) => continue,
                Err(tungstenite::Error::Io(io)) if is_timeout(&io) => return Err(stalled()),
                Err(error) => return Err(transport_lost(&socket, &lost, error)),
            }
        };
        let value: Value = serde_json::from_str(&reply).map_err(|e| {
            Diagnostic::new(
                code::PROTOCOL_MALFORMED_REPLY,
                format!("the server's handshake reply is not valid JSON: {e}"),
            )
        })?;

        // The server rejects a bad handshake with a `{"id": 0, "error": {...}}` response; surface
        // its diagnostic verbatim.
        if let Some(error) = value.get("error") {
            return Err(
                serde_json::from_value::<Diagnostic>(error.clone()).unwrap_or_else(|_| {
                    Diagnostic::new(
                        code::PROTOCOL_UNSUPPORTED_VERSION,
                        "the server rejected the handshake",
                    )
                }),
            );
        }

        let server_hello: ServerHello = serde_json::from_value(value).map_err(|e| {
            Diagnostic::new(
                code::PROTOCOL_MALFORMED_REPLY,
                format!("the server's handshake reply is not a valid ServerHello: {e}"),
            )
        })?;

        if server_hello.protocol_version != PROTOCOL_VERSION {
            return Err(Diagnostic::new(
                code::PROTOCOL_UNSUPPORTED_VERSION,
                format!(
                    "this client speaks protocol version {PROTOCOL_VERSION}, but the server speaks \
                     version {}",
                    server_hello.protocol_version
                ),
            )
            .with_help("regenerate this client from the server's schema"));
        }

        let local_schema = schema::schema_hash();
        if server_hello.schema_hash != local_schema {
            return Err(Diagnostic::new(
                code::PROTOCOL_UNSUPPORTED_VERSION,
                format!(
                    "schema drift: the server's schema hash is {}, but this client was built \
                     against {local_schema}",
                    server_hello.schema_hash
                ),
            )
            .with_help(
                "regenerate this client from the server's exported schema; the protocol shapes \
                 have diverged",
            ));
        }

        if let MaybeTlsStream::Plain(tcp) = socket.get_ref() {
            tcp.set_read_timeout(None)
                .and_then(|()| tcp.set_write_timeout(None))
                .map_err(io_lost)?;
        }

        Ok(Client {
            socket,
            server_hello,
            next_id: 1,
            inbox: Inbox::default(),
            call_timeout: None,
            lost,
            abandoned_subscribes: Vec::new(),
        })
    }

    /// Sets a per-call timeout for every subsequent [`call`](Client::call), returning `self` so it can
    /// be chained after [`connect`](Client::connect).
    ///
    /// With a timeout set, a call that does not receive its reply within `timeout` fails with `L3054`
    /// instead of blocking forever — the guard against a wedged single-threaded sim hanging the
    /// client. **The command may still run**; never resend one blindly. Without a timeout (the
    /// default) a call blocks indefinitely, so no existing caller changes behavior. The timeout
    /// bounds the wait for *this client's* reply; it does not stop the engine.
    ///
    /// # Example
    /// ```no_run
    /// use std::time::Duration;
    /// use ludion_client::Client;
    ///
    /// let mut client = Client::connect("ws://127.0.0.1:4321")?.with_call_timeout(Duration::from_secs(5));
    /// // A call now fails with L3054 rather than hanging if the engine stops answering.
    /// let _ = client.sim_hash();
    /// # Ok::<(), ludion_core::diagnostics::Diagnostic>(())
    /// ```
    pub fn with_call_timeout(mut self, timeout: Duration) -> Self {
        self.call_timeout = Some(timeout);
        self
    }

    /// Sets or clears the per-call timeout in place (see [`with_call_timeout`](Client::with_call_timeout)
    /// for the semantics). `None` restores the default block-forever behavior.
    pub fn set_call_timeout(&mut self, timeout: Option<Duration>) {
        self.call_timeout = timeout;
    }

    /// The current per-call timeout, if any (`None` = block forever).
    pub fn call_timeout(&self) -> Option<Duration> {
        self.call_timeout
    }

    /// The server's announced schema hash (equal to this client's, or [`connect`](Client::connect)
    /// would have failed).
    pub fn schema_hash(&self) -> WireHash {
        self.server_hello.schema_hash
    }

    /// The full [`ServerHello`] the server replied with (engine version, protocol version, schema).
    pub fn server_hello(&self) -> &ServerHello {
        &self.server_hello
    }

    /// Sends `command` and blocks for its response — the primitive every other method builds on.
    ///
    /// Other message shapes can interleave with the response on a subscribed connection: a pushed
    /// [`EventMessage`] or a binary [`ludion_protocol::frame`] message may arrive before it. Neither is
    /// lost — an event is queued for [`next_event`](Client::next_event) and a frame for
    /// [`next_frame`](Client::next_frame), within the bounds in the crate's `# Streams` section.
    ///
    /// # Errors
    /// - The command's own diagnostic if the engine reported one.
    /// - `L3009` if the connection is lost while sending or waiting.
    /// - `L3054` if a per-call timeout is set (see [`with_call_timeout`](Client::with_call_timeout))
    ///   and the reply does not arrive in time; the command may still run.
    /// - `L3051` if the server's response is not well-formed; `L3004` if a binary message that
    ///   arrived while waiting is not a well-formed frame.
    pub fn call(&mut self, command: Command) -> Result<CallOk, Diagnostic> {
        self.refuse_if_lost()?;
        let id = self.next_id;
        self.next_id += 1;

        let is_subscribe = matches!(command, Command::StreamSubscribe(_));
        let unsubscribed = match &command {
            Command::StreamUnsubscribe(params) => Some(params.sub),
            _ => None,
        };

        send_text(
            &mut self.socket,
            &self.lost,
            serde_json::to_string(&Request::new(id, command)).map_err(serialize_err)?,
        )?;

        let result = match self.call_timeout {
            None => self.await_response_blocking(id),
            Some(timeout) => self.await_response_within(id, timeout),
        };
        match (&result, unsubscribed) {
            (Err(error), _) if is_subscribe && error.code == code::PROTOCOL_CALL_TIMED_OUT => {
                self.abandoned_subscribes.push(id);
            }
            (Ok(_), Some(sub)) => self.inbox.close(sub),
            _ => {}
        }
        result
    }

    /// Handles a response that is not the one being waited for. The late answer to a timed-out
    /// subscribe minted a sub nobody holds: close it locally and ask the host to drop it, without
    /// waiting for the reply (that `Ack` is itself a stray and falls through here). A send failure
    /// is ignored — a lost connection takes its subs with it.
    fn stray_reply(&mut self, response: Response) {
        let Some(at) = self
            .abandoned_subscribes
            .iter()
            .position(|id| *id == response.id.as_u64())
        else {
            return;
        };
        self.abandoned_subscribes.swap_remove(at);
        let WireResult::Ok {
            ok: ResponseBody::Subscribed { sub },
            ..
        } = response.result
        else {
            return;
        };
        self.inbox.close(sub);
        let id = self.next_id;
        self.next_id += 1;
        let command = Command::StreamUnsubscribe(StreamUnsubscribeParams { sub });
        if let Ok(text) = serde_json::to_string(&Request::new(id, command)) {
            let _ = send_text(&mut self.socket, &self.lost, text);
        }
    }

    /// Waits (forever) for the response with `id`, buffering any interleaved event/frame traffic instead
    /// of dropping it: a pushed event goes to `events`, a binary frame to `frames`. The id filter (for a
    /// response that isn't ours) stays defensive even though nothing in this SDK issues two calls
    /// concurrently on one connection.
    fn await_response_blocking(&mut self, id: u64) -> Result<CallOk, Diagnostic> {
        loop {
            match read_message(&mut self.socket, &self.lost)? {
                Message::Text(text) => match classify_text(&text)? {
                    TextMessage::Event(event) => self.inbox.push_event(event),
                    TextMessage::Response(response) => {
                        if response.id.as_u64() != id {
                            self.stray_reply(response);
                            continue;
                        }
                        return response_to_result(response);
                    }
                },
                Message::Binary(bytes) => self.inbox.push_frame(decode_frame(bytes)?),
                _ => unreachable!("read_message only ever returns Text or Binary"),
            }
        }
    }

    /// The bounded counterpart of [`await_response_blocking`](Client::await_response_blocking): waits at
    /// most `timeout` for the response with `id`, then fails with `L3054`. Interleaved event/frame
    /// traffic is buffered exactly the same way. The socket's previous read timeout is saved and restored
    /// (even on error), so a later untimed `call`/`next_*` still blocks as configured.
    fn await_response_within(&mut self, id: u64, timeout: Duration) -> Result<CallOk, Diagnostic> {
        let previous = self.set_read_timeout(Some(timeout))?;
        let deadline = Instant::now() + timeout;
        let outcome = loop {
            let remaining = deadline.saturating_duration_since(Instant::now());
            let message = match self.read_message_within(remaining) {
                Ok(Some(message)) => message,
                Ok(None) => break Err(call_timed_out(timeout)),
                Err(error) => break Err(error),
            };
            match message {
                Message::Text(text) => match classify_text(&text) {
                    Ok(TextMessage::Event(event)) => self.inbox.push_event(event),
                    Ok(TextMessage::Response(response)) => {
                        if response.id.as_u64() != id {
                            self.stray_reply(response); // not ours; keep waiting
                            continue;
                        }
                        break response_to_result(response);
                    }
                    Err(error) => break Err(error),
                },
                Message::Binary(bytes) => match decode_frame(bytes) {
                    Ok(frame) => self.inbox.push_frame(frame),
                    Err(error) => break Err(error),
                },
                _ => unreachable!("read_message_within only ever returns Text or Binary"),
            }
        };
        self.restore_read_timeout(previous, outcome)
    }

    /// `stream.subscribe` — opens a subscription on `channel`, returning the [`SubId`] events or
    /// frames on it will carry.
    ///
    /// `format` selects the payload codec of a [`Channel::Frames`] subscription and is ignored on
    /// every other channel.
    ///
    /// # Example
    /// ```no_run
    /// use ludion_client::Client;
    /// use ludion_protocol::frame::FrameFormat;
    /// use ludion_protocol::{Channel, SubscribeMode};
    ///
    /// let mut client = Client::connect("ws://127.0.0.1:4321")?;
    /// let sub = client.subscribe(
    ///     Channel::SceneChanged,
    ///     SubscribeMode::default(),
    ///     FrameFormat::Qoi,
    /// )?;
    /// println!("subscribed as {sub:?}");
    /// # Ok::<(), ludion_core::diagnostics::Diagnostic>(())
    /// ```
    ///
    /// A subscribe that times out (`L3054`) unsubscribes itself when its late reply is read.
    ///
    /// # Errors
    /// The engine's diagnostic (e.g. `L3046` for an out-of-range `max_fps`), or a
    /// transport error (see [`call`](Client::call)).
    pub fn subscribe(
        &mut self,
        channel: Channel,
        mode: SubscribeMode,
        format: FrameFormat,
    ) -> Result<SubId, Diagnostic> {
        match self
            .call(Command::StreamSubscribe(StreamSubscribeParams {
                channel,
                mode,
                format,
            }))?
            .body
        {
            ResponseBody::Subscribed { sub } => Ok(sub),
            other => Err(unexpected_body("stream.subscribe", "Subscribed", &other)),
        }
    }

    /// `stream.unsubscribe` — closes a subscription opened by [`subscribe`](Client::subscribe).
    ///
    /// # Errors
    /// The engine's diagnostic, or a transport error (see [`call`](Client::call)).
    pub fn unsubscribe(&mut self, sub: SubId) -> Result<(), Diagnostic> {
        match self
            .call(Command::StreamUnsubscribe(StreamUnsubscribeParams { sub }))?
            .body
        {
            ResponseBody::Ack => Ok(()),
            other => Err(unexpected_body("stream.unsubscribe", "Ack", &other)),
        }
    }

    /// Returns the next subscription event, waiting up to `timeout` if none is already buffered.
    ///
    /// Drains the internal queue [`call`](Client::call) fills first (see its docs) and only reads
    /// the socket once that queue is empty — so an event that arrived while a call was in flight is
    /// returned exactly the same way as one that arrives during this wait. `Ok(None)` means `timeout`
    /// elapsed with nothing to report; that is not an error, so a caller can poll a subscription in a
    /// loop. A binary frame seen while waiting is queued for [`next_frame`](Client::next_frame)
    /// instead of being discarded.
    ///
    /// # Example
    /// ```no_run
    /// use ludion_client::Client;
    /// use ludion_protocol::frame::FrameFormat;
    /// use ludion_protocol::{Channel, SubscribeMode};
    /// use std::time::Duration;
    ///
    /// let mut client = Client::connect("ws://127.0.0.1:4321")?;
    /// client.subscribe(
    ///     Channel::SceneChanged,
    ///     SubscribeMode::default(),
    ///     FrameFormat::Qoi,
    /// )?;
    /// match client.next_event(Duration::from_secs(5))? {
    ///     Some(event) => println!("{event:?}"),
    ///     None => println!("no event within the timeout"),
    /// }
    /// # Ok::<(), ludion_core::diagnostics::Diagnostic>(())
    /// ```
    ///
    /// # Errors
    /// `L3009` if the connection is lost while waiting; `L3051`/`L3004` if a message that arrived
    /// could not be parsed.
    pub fn next_event(&mut self, timeout: Duration) -> Result<Option<EventMessage>, Diagnostic> {
        if let Some(event) = self.inbox.pop_event() {
            return Ok(Some(event));
        }
        self.wait_for_event(timeout)
    }

    /// Returns the next binary rendered frame, waiting up to `timeout` if none is already buffered.
    ///
    /// Drains the internal queue [`call`](Client::call) fills first, exactly like
    /// [`next_event`](Client::next_event); the queue holds only the latest frame per subscription, so
    /// a slow reader sees the newest image, not a backlog. `Ok(None)` means `timeout` elapsed with
    /// nothing to report. The returned [`ReceivedFrame`] carries its payload exactly as encoded on the wire —
    /// `header.format` names which of the three codecs; this crate does not decode pixels — a caller
    /// that wants them decodes the payload itself, matching the [`screenshot`](Client::screenshot)
    /// precedent. A subscription event seen while waiting is queued for
    /// [`next_event`](Client::next_event) instead of being discarded.
    ///
    /// # Errors
    /// `L3009` if the connection is lost while waiting; `L3051` if a text message that arrived could
    /// not be parsed, `L3004` if a binary message is not a well-formed frame.
    pub fn next_frame(&mut self, timeout: Duration) -> Result<Option<ReceivedFrame>, Diagnostic> {
        if let Some(frame) = self.inbox.pop_frame() {
            return Ok(Some(frame));
        }
        self.wait_for_frame(timeout)
    }

    /// How many buffered events were discarded since connect because the buffer held
    /// [`MAX_BUFFERED_EVENTS`]. A caller that sees it grow is not draining
    /// [`next_event`](Client::next_event).
    pub fn dropped_events(&self) -> u64 {
        self.inbox.dropped_events
    }

    /// Whether the connection is still open, without sending anything: a non-blocking read that
    /// buffers whatever the server already pushed (events and frames are kept for
    /// [`next_event`](Client::next_event) / [`next_frame`](Client::next_frame); a stray late reply is
    /// dropped) and stops at the first would-block.
    ///
    /// This lets a caller decide a reconnect is safe *before* it sends: a command written into a
    /// connection that had already closed may or may not have been executed, but nothing has been
    /// written yet here. Blocking mode is restored on every path.
    ///
    /// # Errors
    /// `L3009` if the server closed the connection or the transport failed.
    pub fn check_open(&mut self) -> Result<(), Diagnostic> {
        self.refuse_if_lost()?;
        self.set_nonblocking(true)?;
        let outcome = self.drain_ready();
        let restored = self.set_nonblocking(false);
        match outcome {
            Err(error) => Err(error),
            Ok(()) => restored,
        }
    }

    /// `L3009` once an over-limit inbound message has ended the connection (see [`transport_lost`]).
    fn refuse_if_lost(&self) -> Result<(), Diagnostic> {
        if self.lost.load(Ordering::Relaxed) {
            return Err(Diagnostic::new(
                code::PROTOCOL_ENGINE_UNAVAILABLE,
                "the connection was ended after an over-limit message from the server",
            )
            .with_help("reconnect; the stream cannot be resynchronised mid-frame"));
        }
        Ok(())
    }

    /// Reads and buffers every message already waiting on the socket (see
    /// [`check_open`](Client::check_open)).
    fn drain_ready(&mut self) -> Result<(), Diagnostic> {
        loop {
            match self.socket.read() {
                Ok(Message::Text(text)) => match classify_text(&text)? {
                    TextMessage::Event(event) => self.inbox.push_event(event),
                    TextMessage::Response(response) => self.stray_reply(response),
                },
                Ok(Message::Binary(bytes)) => self.inbox.push_frame(decode_frame(bytes)?),
                Ok(Message::Close(_)) => return Err(closed()),
                Ok(_) => continue, // ping / pong
                Err(tungstenite::Error::Io(e)) if e.kind() == ErrorKind::WouldBlock => {
                    return Ok(());
                }
                Err(e) => return Err(transport_lost(&self.socket, &self.lost, e)),
            }
        }
    }

    /// Switches the underlying TCP stream between blocking and non-blocking reads (plain `ws://`
    /// only, like [`set_read_timeout`](Client::set_read_timeout)).
    fn set_nonblocking(&self, nonblocking: bool) -> Result<(), Diagnostic> {
        let MaybeTlsStream::Plain(tcp) = self.socket.get_ref() else {
            return Err(Diagnostic::new(
                code::PROTOCOL_ENGINE_UNAVAILABLE,
                "this client's transport does not support probing (only plain ws:// is supported)",
            ));
        };
        tcp.set_nonblocking(nonblocking).map_err(io_lost)
    }

    /// The `next_event`/`next_frame` implementation once the relevant buffer is empty: sets a socket
    /// read timeout (saving the previous value), reads until an event arrives or `timeout` elapses,
    /// buffering any binary frame seen along the way, then restores the previous read timeout so a
    /// later [`call`](Client::call) blocks as usual.
    fn wait_for_event(&mut self, timeout: Duration) -> Result<Option<EventMessage>, Diagnostic> {
        let previous = self.set_read_timeout(Some(timeout))?;
        let deadline = Instant::now() + timeout;
        let outcome = loop {
            let remaining = deadline.saturating_duration_since(Instant::now());
            let message = match self.read_message_within(remaining) {
                Ok(Some(message)) => message,
                Ok(None) => break Ok(None),
                Err(error) => break Err(error),
            };
            match message {
                Message::Text(text) => match classify_text(&text) {
                    Ok(TextMessage::Event(event)) if self.inbox.is_closed(event.sub) => {}
                    Ok(TextMessage::Event(event)) => break Ok(Some(event)),
                    Ok(TextMessage::Response(response)) => self.stray_reply(response),
                    Err(error) => break Err(error),
                },
                Message::Binary(bytes) => match decode_frame(bytes) {
                    Ok(frame) => self.inbox.push_frame(frame),
                    Err(error) => break Err(error),
                },
                _ => unreachable!("read_message_within only ever returns Text or Binary"),
            }
        };
        self.restore_read_timeout(previous, outcome)
    }

    /// The mirror of [`wait_for_event`](Client::wait_for_event) for binary frames: any subscription
    /// event seen while waiting is buffered for [`next_event`](Client::next_event) instead.
    fn wait_for_frame(&mut self, timeout: Duration) -> Result<Option<ReceivedFrame>, Diagnostic> {
        let previous = self.set_read_timeout(Some(timeout))?;
        let deadline = Instant::now() + timeout;
        let outcome = loop {
            let remaining = deadline.saturating_duration_since(Instant::now());
            let message = match self.read_message_within(remaining) {
                Ok(Some(message)) => message,
                Ok(None) => break Ok(None),
                Err(error) => break Err(error),
            };
            match message {
                Message::Binary(bytes) => match decode_frame(bytes) {
                    Ok(frame) if self.inbox.is_closed(frame.header.sub) => {}
                    Ok(frame) => break Ok(Some(frame)),
                    Err(error) => break Err(error),
                },
                Message::Text(text) => match classify_text(&text) {
                    Ok(TextMessage::Event(event)) => self.inbox.push_event(event),
                    Ok(TextMessage::Response(response)) => self.stray_reply(response),
                    Err(error) => break Err(error),
                },
                _ => unreachable!("read_message_within only ever returns Text or Binary"),
            }
        };
        self.restore_read_timeout(previous, outcome)
    }

    /// Restores the socket's read timeout to `previous` — always, even when `outcome` is an error,
    /// so a recoverable failure (a malformed frame, say) never leaves a stale timeout behind for a
    /// later [`call`](Client::call) — then returns `outcome`. When both fail, `outcome`'s error wins
    /// (it is the one the caller can act on).
    fn restore_read_timeout<T>(
        &self,
        previous: Option<Duration>,
        outcome: Result<T, Diagnostic>,
    ) -> Result<T, Diagnostic> {
        let restored = self.set_read_timeout(previous);
        match outcome {
            Err(error) => Err(error),
            Ok(value) => restored.map(|_| value),
        }
    }

    /// Sets the socket's read timeout, returning the previous value so a caller can restore it.
    ///
    /// This client only ever speaks plain `ws://` (the workspace pins `tungstenite` to default
    /// features, so no TLS backend is compiled in), so the socket is always
    /// [`MaybeTlsStream::Plain`]; the non-`Plain` arm can't be reached in practice but is kept as a
    /// diagnostic rather than a panic since `MaybeTlsStream` is `#[non_exhaustive]`.
    fn set_read_timeout(&self, timeout: Option<Duration>) -> Result<Option<Duration>, Diagnostic> {
        let MaybeTlsStream::Plain(tcp) = self.socket.get_ref() else {
            return Err(Diagnostic::new(
                code::PROTOCOL_ENGINE_UNAVAILABLE,
                "this client's transport does not support timed reads (only plain ws:// is \
                 supported)",
            ));
        };
        let previous = tcp.read_timeout().map_err(io_lost)?;
        tcp.set_read_timeout(timeout).map_err(io_lost)?;
        Ok(previous)
    }

    /// Reads the next `Text`/`Binary` message within `remaining` (setting the socket's read timeout
    /// to it first), or `Ok(None)` if `remaining` elapses first with nothing arriving. Ping/pong are
    /// skipped transparently; a close frame or transport error is a hard `L3009`.
    fn read_message_within(&mut self, remaining: Duration) -> Result<Option<Message>, Diagnostic> {
        self.refuse_if_lost()?;
        if remaining.is_zero() {
            return Ok(None);
        }
        self.set_read_timeout(Some(remaining))?;
        loop {
            match self.socket.read() {
                Ok(msg @ (Message::Text(_) | Message::Binary(_))) => return Ok(Some(msg)),
                Ok(Message::Close(_)) => return Err(closed()),
                Ok(_) => continue, // ping / pong
                Err(tungstenite::Error::Io(e)) if is_timeout(&e) => return Ok(None),
                Err(e) => return Err(transport_lost(&self.socket, &self.lost, e)),
            }
        }
    }

    /// `sim.hash` — the current [`WorldHash`](ludion_protocol::WireHash), the determinism primitive.
    ///
    /// # Errors
    /// The engine's diagnostic, or a transport error (see [`call`](Client::call)).
    pub fn sim_hash(&mut self) -> Result<WireHash, Diagnostic> {
        match self.call(Command::SimHash)?.body {
            ResponseBody::SimHash { hash } => Ok(hash),
            other => Err(unexpected_body("sim.hash", "SimHash", &other)),
        }
    }

    /// `sim.step` — run exactly `ticks` ticks then pause, optionally returning the resulting
    /// `WorldHash` (present iff `hash`).
    ///
    /// # Errors
    /// The engine's diagnostic, or a transport error (see [`call`](Client::call)).
    pub fn sim_step(&mut self, ticks: u64, hash: bool) -> Result<Option<WireHash>, Diagnostic> {
        match self
            .call(Command::SimStep(SimStepParams { ticks, hash }))?
            .body
        {
            ResponseBody::Stepped { hash } => Ok(hash),
            other => Err(unexpected_body("sim.step", "Stepped", &other)),
        }
    }

    /// `render.screenshot` — render one guaranteed-frame image and return it as
    /// `(width, height, format, base64_png)`.
    ///
    /// The image bytes are standard-base64 PNG (this crate does not decode them — a caller that
    /// wants pixels decodes the base64 and PNG itself). Omitted dimensions use the engine's default.
    ///
    /// # Errors
    /// The engine's diagnostic (e.g. `L2003` when the server has no GPU adapter), or a transport
    /// error (see [`call`](Client::call)).
    pub fn screenshot(
        &mut self,
        width: Option<u32>,
        height: Option<u32>,
    ) -> Result<Screenshot, Diagnostic> {
        match self
            .call(Command::RenderScreenshot(RenderScreenshotParams {
                width,
                height,
                // The SDK's screenshot is an agent's, so it answers through the game's own camera
                // whether or not a host is streaming a flown `render.view` somewhere else.
                camera: CameraChoice::Scene,
            }))?
            .body
        {
            ResponseBody::Screenshot {
                width,
                height,
                format,
                data,
                stale,
            } => Ok(Screenshot {
                width,
                height,
                format,
                data,
                stale,
            }),
            other => Err(unexpected_body("render.screenshot", "Screenshot", &other)),
        }
    }
}

/// A rendered screenshot as returned by [`Client::screenshot`]: dimensions plus base64 PNG bytes.
#[derive(Clone, Debug, PartialEq)]
pub struct Screenshot {
    /// Image width in pixels.
    pub width: u32,
    /// Image height in pixels.
    pub height: u32,
    /// The image encoding — `"png"`.
    pub format: String,
    /// The image bytes as standard (RFC 4648) base64 of the PNG.
    pub data: String,
    /// Whether the server was holding its global-illumination baseline when this image was rendered
    /// (`render.gi_rebake` at `hold`), so the light shown is one the current world no longer implies.
    pub stale: bool,
}

/// One binary rendered frame as returned by [`Client::next_frame`]: its decoded
/// [`FrameHeader`] plus the still-encoded payload.
///
/// The payload stays exactly as the wire carried it; `header.format` names which of the wire's
/// three codecs it is in (QOI, LZ4_RGBA, or RAW_RGBA — see
/// [`FrameFormat`]). This crate does not decode pixels, the
/// same choice [`Screenshot`] makes. A caller that wants pixels decodes the payload itself,
/// matching `header.format` to a decoder (e.g. the `qoi` crate for `FrameFormat::Qoi`).
#[derive(Clone, Debug, PartialEq)]
pub struct ReceivedFrame {
    /// The frame's header: which subscription it belongs to, the sim frame it was rendered from,
    /// its encoding, and its dimensions.
    pub header: FrameHeader,
    /// The encoded pixel payload (`header.format`-encoded, `header.width * header.height` pixels).
    pub payload: Vec<u8>,
}

/// Maps a completed [`Response`] to a [`CallOk`] or the engine's diagnostic.
fn response_to_result(response: Response) -> Result<CallOk, Diagnostic> {
    match response.result {
        WireResult::Ok {
            ok,
            frame,
            game_frame,
        } => Ok(CallOk {
            body: ok,
            frame,
            game_frame,
        }),
        WireResult::Error { error } => Err(error),
    }
}

/// Builds the `L3054` diagnostic for a call whose per-call timeout elapsed before the reply arrived.
fn call_timed_out(timeout: Duration) -> Diagnostic {
    Diagnostic::new(
        code::PROTOCOL_CALL_TIMED_OUT,
        format!("the engine did not reply within the {timeout:?} call timeout"),
    )
    .with_help(
        "the command may still run, so do not resend it blindly; check the host is running and \
         read the state back first (the sim thread may be wedged, e.g. an infinite-looping system)",
    )
}

/// Sends one already-serialized JSON `text` as a text WebSocket message, mapping a transport
/// failure to `L3009`. Call sites serialize the concrete wire type with `serde_json::to_string`
/// (whose `Serialize` bound stays inside `serde_json`), so this crate needs no direct `serde` dep.
fn send_text(
    socket: &mut WebSocket<MaybeTlsStream<TcpStream>>,
    lost: &AtomicBool,
    text: String,
) -> Result<(), Diagnostic> {
    check_outgoing_size(text.len(), MAX_MESSAGE_BYTES)?;
    socket
        .send(Message::text(text))
        .map_err(|e| transport_lost(socket, lost, e))
}

/// The one WebSocket config every connection is opened with: the wire's message ceiling in place of
/// tungstenite's 16 MiB default, which a 4K raw frame already exceeds.
fn websocket_config() -> WebSocketConfig {
    WebSocketConfig {
        max_message_size: Some(MAX_MESSAGE_BYTES),
        max_frame_size: Some(MAX_MESSAGE_BYTES),
        ..WebSocketConfig::default()
    }
}

/// Refuses a request of `len` bytes over `limit` with `L3045` before anything is written.
fn check_outgoing_size(len: usize, limit: usize) -> Result<(), Diagnostic> {
    if len <= limit {
        return Ok(());
    }
    Err(Diagnostic::new(
        code::PROTOCOL_MESSAGE_TOO_LARGE,
        format!("this request is {len} bytes, over the {limit}-byte wire limit; it was not sent"),
    )
    .with_help("narrow the request, or snapshot a smaller world; `ludion explain L3045` has more"))
}

/// Maps a (should-never-happen) serialization failure of a wire value to a diagnostic.
fn serialize_err(error: serde_json::Error) -> Diagnostic {
    Diagnostic::new(
        code::PROTOCOL_MALFORMED_ENVELOPE,
        format!("could not serialize a request: {error}"),
    )
}

/// Reads the next `Text`/`Binary` message, blocking until one arrives. Ping/pong are skipped
/// transparently; a close frame or transport error is a hard `L3009`.
fn read_message(
    socket: &mut WebSocket<MaybeTlsStream<TcpStream>>,
    lost: &AtomicBool,
) -> Result<Message, Diagnostic> {
    loop {
        match socket.read().map_err(|e| transport_lost(socket, lost, e))? {
            msg @ (Message::Text(_) | Message::Binary(_)) => return Ok(msg),
            Message::Close(_) => return Err(closed()),
            _ => continue, // ping / pong
        }
    }
}

/// A text message classified by shape: a [`Response`] (has an `id`) or a pushed [`EventMessage`]
/// (has no `id`, but always an `event`). The two shapes never overlap on the wire (envelope.rs).
enum TextMessage {
    /// A reply to a request this client sent.
    Response(Response),
    /// A message pushed by a subscription this client opened.
    Event(EventMessage),
}

/// Classifies and parses one text message as a [`Response`] or a pushed [`EventMessage`], by
/// whether its JSON object carries an `event` key.
///
/// # Errors
/// `L3051` if the text is not JSON, or does not match the shape its `event` key implies.
fn classify_text(text: &str) -> Result<TextMessage, Diagnostic> {
    let value: Value = serde_json::from_str(text).map_err(|e| {
        Diagnostic::new(
            code::PROTOCOL_MALFORMED_REPLY,
            format!("the server's message is not valid JSON: {e}"),
        )
    })?;
    if value.get("event").is_some() {
        serde_json::from_value::<EventMessage>(value)
            .map(TextMessage::Event)
            .map_err(|e| {
                Diagnostic::new(
                    code::PROTOCOL_MALFORMED_REPLY,
                    format!("the server's event message is not well-formed: {e}"),
                )
            })
    } else {
        serde_json::from_value::<Response>(value)
            .map(TextMessage::Response)
            .map_err(|e| {
                Diagnostic::new(
                    code::PROTOCOL_MALFORMED_REPLY,
                    format!("the server's response is not a valid response envelope: {e}"),
                )
            })
    }
}

/// Decodes a binary WebSocket message as an LFR1 [`ludion_protocol::frame`] message.
///
/// # Errors
/// `L3004` if the bytes are not a well-formed frame message (see
/// [`FrameHeader::decode`](ludion_protocol::frame::FrameHeader::decode)).
fn decode_frame(mut bytes: Vec<u8>) -> Result<ReceivedFrame, Diagnostic> {
    let (header, _) = FrameHeader::decode(&bytes)?;
    bytes.drain(..FRAME_HEADER_LEN);
    Ok(ReceivedFrame {
        header,
        payload: bytes,
    })
}

/// The most events [`Client`] keeps buffered for [`next_event`](Client::next_event); past it the oldest
/// is dropped and counted in [`Client::dropped_events`].
pub const MAX_BUFFERED_EVENTS: usize = 4096;

/// Traffic read off the socket while looking for something else: bounded events (oldest dropped,
/// counted) and at most one frame per subscription (latest wins — the only subscribe mode there is).
#[derive(Default)]
struct Inbox {
    events: VecDeque<EventMessage>,
    frames: VecDeque<ReceivedFrame>,
    dropped_events: u64,
    /// Subscriptions that were unsubscribed: a frame can outlive its unsubscribe `Ack` (frames ride a
    /// lower-priority lane than replies), so their traffic is refused, not just purged once.
    closed: BTreeSet<u64>,
}

impl Inbox {
    fn is_closed(&self, sub: SubId) -> bool {
        self.closed.contains(&sub.as_u64())
    }

    /// Records `sub` as closed and drops what is already queued for it.
    fn close(&mut self, sub: SubId) {
        self.closed.insert(sub.as_u64());
        self.events.retain(|event| event.sub != sub);
        self.frames.retain(|frame| frame.header.sub != sub);
    }

    fn push_event(&mut self, event: EventMessage) {
        if self.is_closed(event.sub) {
            return;
        }
        if self.events.len() >= MAX_BUFFERED_EVENTS {
            self.events.pop_front();
            self.dropped_events += 1;
        }
        self.events.push_back(event);
    }

    fn push_frame(&mut self, frame: ReceivedFrame) {
        if self.is_closed(frame.header.sub) {
            return;
        }
        self.frames
            .retain(|queued| queued.header.sub != frame.header.sub);
        self.frames.push_back(frame);
    }

    fn pop_event(&mut self) -> Option<EventMessage> {
        self.events.pop_front()
    }

    fn pop_frame(&mut self) -> Option<ReceivedFrame> {
        self.frames.pop_front()
    }
}

/// Whether an I/O error is the "the read timeout elapsed" signal rather than a real transport loss.
/// The exact `ErrorKind` a timed-out read reports is platform-dependent (`WouldBlock` or
/// `TimedOut`), so both are treated as a timeout.
fn is_timeout(error: &std::io::Error) -> bool {
    matches!(error.kind(), ErrorKind::WouldBlock | ErrorKind::TimedOut)
}

/// Maps a failure to configure the socket's read timeout to an `L3009` diagnostic.
fn io_lost(error: std::io::Error) -> Diagnostic {
    Diagnostic::new(
        code::PROTOCOL_ENGINE_UNAVAILABLE,
        format!("could not configure the socket's read timeout: {error}"),
    )
    .with_help("the underlying TCP connection may have been lost")
}

/// Builds the `L3009` diagnostic for a server-initiated close.
fn closed() -> Diagnostic {
    Diagnostic::new(
        code::PROTOCOL_ENGINE_UNAVAILABLE,
        "the server closed the connection",
    )
}

/// Maps a lost/failed WebSocket connection to a diagnostic, and ends the connection when the error
/// leaves it unusable.
///
/// An over-limit inbound message (`L3045`) is rejected on its header, mid-frame: the stream cannot be
/// resynchronised, so the socket is shut down and every later read, send or
/// [`check_open`](Client::check_open) fails as `L3009`, which a caller reconnects on. The `lost` flag
/// is set here, at the transport-error site — never by matching a response's code, since the engine
/// can answer a request with an `L3045` on a healthy connection.
fn transport_lost(
    socket: &WebSocket<MaybeTlsStream<TcpStream>>,
    lost: &AtomicBool,
    error: tungstenite::Error,
) -> Diagnostic {
    if matches!(error, tungstenite::Error::Capacity(_)) {
        lost.store(true, Ordering::Relaxed);
        if let MaybeTlsStream::Plain(tcp) = socket.get_ref() {
            let _ = tcp.shutdown(std::net::Shutdown::Both);
        }
    }
    describe_transport_error(error)
}

/// The diagnostic for a WebSocket error: `L3045` for an over-limit message, else `L3009`.
fn describe_transport_error(error: tungstenite::Error) -> Diagnostic {
    if let tungstenite::Error::Capacity(CapacityError::MessageTooLong { size, max_size }) = error {
        return Diagnostic::new(
            code::PROTOCOL_MESSAGE_TOO_LARGE,
            format!(
                "the server sent a message of {size} bytes, over the {max_size}-byte wire limit"
            ),
        )
        .with_help(
            "narrow the query, or snapshot a smaller world; `ludion explain L3045` has more",
        );
    }
    Diagnostic::new(
        code::PROTOCOL_ENGINE_UNAVAILABLE,
        format!("the connection to the server was lost: {error}"),
    )
    .with_help("the server may have shut down; reconnect once it is running again")
}

/// Builds an `L3044` diagnostic for a response whose `kind` did not match the command.
///
/// This should not happen against a matching-schema server (the handshake guarantees agreement), so
/// it is a protocol-shape error, not a malformed request.
pub fn unexpected_body(method: &str, expected: &str, got: &ResponseBody) -> Diagnostic {
    Diagnostic::new(
        code::PROTOCOL_UNEXPECTED_RESPONSE,
        format!("`{method}` returned an unexpected response body (wanted {expected}), got {got:?}"),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `localhost` may resolve to `::1` before `127.0.0.1`: every resolved address is tried.
    #[test]
    fn a_name_resolving_to_several_addresses_connects_on_the_one_that_listens() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let addrs =
            socket_addrs(&format!("ws://localhost:{port}/path")).expect("localhost resolves");
        connect_any(&addrs, Duration::from_secs(2)).expect("the 127.0.0.1 listener is reached");
        assert!(socket_addrs("http://localhost:1").is_none());
    }

    #[test]
    fn an_over_limit_inbound_message_is_l3045_not_l3009() {
        let error = tungstenite::Error::Capacity(CapacityError::MessageTooLong {
            size: 300,
            max_size: 200,
        });
        let diagnostic = describe_transport_error(error);
        assert_eq!(diagnostic.code, code::PROTOCOL_MESSAGE_TOO_LARGE);
        assert!(diagnostic.message.contains("300"), "{}", diagnostic.message);
        assert!(diagnostic.message.contains("200"), "{}", diagnostic.message);
    }

    #[test]
    fn the_outgoing_size_check_names_the_size_and_the_limit() {
        assert!(check_outgoing_size(10, 10).is_ok());
        let diagnostic = check_outgoing_size(11, 10).unwrap_err();
        assert_eq!(diagnostic.code, code::PROTOCOL_MESSAGE_TOO_LARGE);
        assert!(diagnostic.message.contains("11"), "{}", diagnostic.message);
    }

    #[test]
    fn an_unexpected_reply_body_is_l3044() {
        let diagnostic = unexpected_body("sim.hash", "SimHash", &ResponseBody::Ack);
        assert_eq!(diagnostic.code, code::PROTOCOL_UNEXPECTED_RESPONSE);
        assert_eq!(diagnostic.code.as_str(), "L3044");
        assert!(
            diagnostic.message.contains("sim.hash"),
            "{}",
            diagnostic.message
        );
    }

    fn frame(sub: u64, index: u64) -> ReceivedFrame {
        ReceivedFrame {
            header: FrameHeader {
                sub: SubId::new(sub),
                frame: FrameIndex::new(index),
                format: FrameFormat::Qoi,
                width: 1,
                height: 1,
                gi_stale: false,
            },
            payload: vec![index as u8],
        }
    }

    fn event(seq: u64) -> EventMessage {
        serde_json::from_value(serde_json::json!({
            "event": "scene.changed",
            "data": {},
            "sub": 1,
            "seq": seq,
        }))
        .expect("a well-formed scene.changed event")
    }

    #[test]
    fn a_newer_frame_replaces_the_queued_one_of_its_subscription() {
        let mut inbox = Inbox::default();
        inbox.push_frame(frame(1, 10));
        inbox.push_frame(frame(1, 11));
        inbox.push_frame(frame(2, 12));
        inbox.push_frame(frame(1, 13));
        let first = inbox.pop_frame().unwrap();
        assert_eq!(
            (first.header.sub, first.header.frame.as_u64()),
            (SubId::new(2), 12)
        );
        let second = inbox.pop_frame().unwrap();
        assert_eq!(
            (second.header.sub, second.header.frame.as_u64()),
            (SubId::new(1), 13)
        );
        assert!(inbox.pop_frame().is_none());
    }

    #[test]
    fn close_purges_both_queues_and_refuses_later_traffic_for_that_sub_only() {
        let mut inbox = Inbox::default();
        inbox.push_frame(frame(1, 10));
        inbox.push_frame(frame(2, 11));
        inbox.push_event(event(1)); // sub 1
        inbox.close(SubId::new(1));
        assert!(inbox.pop_event().is_none());
        inbox.push_frame(frame(1, 12));
        inbox.push_event(event(2));
        assert!(inbox.pop_event().is_none());
        let kept = inbox.pop_frame().unwrap();
        assert_eq!(kept.header.sub, SubId::new(2));
        assert!(inbox.pop_frame().is_none());
        inbox.push_frame(frame(2, 13));
        assert!(inbox.pop_frame().is_some());
    }

    #[test]
    fn a_full_event_buffer_drops_the_oldest_and_counts_it() {
        let mut inbox = Inbox::default();
        for seq in 0..(MAX_BUFFERED_EVENTS as u64 + 3) {
            inbox.push_event(event(seq));
        }
        assert_eq!(inbox.dropped_events, 3);
        assert_eq!(inbox.events.len(), MAX_BUFFERED_EVENTS);
        assert_eq!(inbox.pop_event().unwrap().seq.as_u64(), 3);
    }

    #[test]
    fn decoding_a_frame_keeps_the_payload_bytes() {
        let sent = frame(7, 3);
        let payload = vec![9, 8, 7, 6, 5];
        let received = decode_frame(sent.header.encode(&payload)).unwrap();
        assert_eq!(received.header, sent.header);
        assert_eq!(received.payload, payload);
    }
}
