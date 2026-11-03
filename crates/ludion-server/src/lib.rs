#![forbid(unsafe_code)]
#![warn(missing_docs)]

//! `ludion-server` — the WebSocket host that puts the engine on the wire.
//!
//! A [`Server`] owns a pre-configured [`Engine`] on a dedicated sim thread and exposes it to any
//! number of WebSocket clients, all sharing the **one** command queue that is the sole path to the
//! world (engine-architecture.md §3). It is a peer of the engine, never privileged over it: every
//! request is a [`ludion_protocol::Request`] that becomes a `StampedCommand` and flows through
//! [`ludion_engine::dispatch`], exactly as an in-process caller's would. There is no server-private
//! way to touch the world.
//!
//! # Threading
//!
//! [`Server::bind`] spawns two things (architecture §3):
//!
//! - the **sim thread**, running [`ludion_engine::live::run_live`] — it owns the [`Engine`] and is
//!   the single consumer of the intake channel; and
//! - a **tokio runtime with two worker threads**, confined to this crate, running the TCP accept
//!   loop and one task per connection.
//!
//! A connection's reader decodes requests continuously — each becomes a `StampedCommand` pushed into
//! the crossbeam intake channel — while a per-connection reply pump awaits the one-shot replies on a
//! blocking-pool task (so a worker is never parked on the sim thread) and answers them in request
//! order; in-flight requests are bounded, so a client may pipeline without the backlog growing
//! without end (see `connection.rs`'s module doc). The sim thread's reply send never blocks, and the
//! whole bridge is race-free by construction — one command, one reply, one receiver.
//!
//! # Handshake
//!
//! A connection's first message must be a [`Hello`](ludion_protocol::handshake::Hello); the server
//! answers with a [`ServerHello`] carrying the engine
//! version, protocol version, and schema hash. A wrong first message or an unsupported protocol
//! version is answered with a structured `L3xxx` error and the connection is closed.
//!
//! # Subscriptions and delivery
//!
//! `stream.subscribe` / `stream.unsubscribe` are dispatched like any command — they mint and free ids
//! in the engine's subscription table — and this host *delivers* to them. The engine emits rendered
//! frames and events on its egress seam ([`ludion_engine::egress`]) over two lanes: a bounded(1)
//! latest-wins frame slot and an unbounded, ordered event lane (so an event is never dropped behind a
//! pending frame). A std pump thread drains both into tokio — events onto an ordered bridge, frames into
//! a latest-wins slot — where an event router delivers each JSON event, a frame router encodes the
//! freshest pending frame **once per distinct format** its subscribers asked for and fans it out to the
//! subscribing connections, and a
//! per-subscription task forwards frames latest-wins with an optional `max_fps` throttle. Each
//! connection carries what it owes the socket on **two** outbound lanes — text (replies, events) and
//! binary (frames) — which its one writer task drains text-first, so a reply overtakes queued frames
//! instead of waiting out the megabytes ahead of it; order within a lane is untouched. A connection
//! registers its subs the
//! instant it sees the subscribe reply flow back through it; on disconnect it unregisters them and
//! submits a `stream.unsubscribe` through the one command queue for each, so the engine stops rendering
//! for a viewer that is gone. The delivery path is the private `delivery` module; frames ride the binary
//! [`ludion_protocol::frame`] message, events the JSON [`ludion_protocol::EventMessage`]. A subscription
//! picks its codec — QOI (the wire default) or `LZ4_RGBA` — and only the formats somebody subscribed to
//! are ever encoded.
//!
//! # Transport profiling
//!
//! The delivery path is also the only place the engine's third profiling layer can be measured: the
//! frame encode and the sink write happen here, downstream of the frame the sim produced. The
//! private `perf` module times both and *injects* them into the `profile` window events and
//! `profile.capture` replies passing through, before serialization — so a client reads one completed
//! message instead of stitching layers together, and there is no client-private perf path. Timing is
//! armed by a `profile` subscription or a capture in flight and costs one relaxed atomic load when
//! nobody is watching.
//!
//! # Example
//! ```no_run
//! use ludion_engine::Engine;
//! use ludion_server::Server;
//!
//! let mut engine = Engine::new();
//! engine.load_inline_scene("format: 1\nentities: []\n").unwrap();
//!
//! // Port 0 asks the OS for a free port; read it back for clients to connect to.
//! let server = Server::bind(engine, "127.0.0.1:0").unwrap();
//! println!("listening on {}", server.url());
//!
//! // ... clients connect and drive the engine over the protocol ...
//!
//! server.shutdown(); // closes connections, stops the sim thread, joins it (Drop does this too)
//! ```

mod connection;
mod delivery;
mod perf;

use std::net::SocketAddr;
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use crossbeam_channel::{Receiver, Sender};
use ludion_assets::WatchHandle;
use ludion_core::codes::code;
use ludion_core::diagnostics::Diagnostic;
use ludion_engine::egress::{Egress, EgressChannels, EgressFrame};
use ludion_engine::live::{
    SIM_STOP_GRACE, ShutdownSignal, StampedCommand, join_sim, run_live, shutdown_channel,
    sim_thread_panicked,
};
use ludion_engine::{ENGINE_VERSION, Engine};
use ludion_protocol::command::AssetReloadParams;
use ludion_protocol::handshake::ServerHello;
use ludion_protocol::{Command, PROTOCOL_VERSION, schema};
use tokio::net::TcpListener;
use tokio::runtime::Runtime;
use tokio::sync::watch;

/// How long [`Server::shutdown`] / [`Drop`] gives the tokio runtime to wind down its tasks before
/// forcing them, so teardown can never hang a test.
const SHUTDOWN_GRACE: Duration = Duration::from_secs(5);

/// The longest [`Server::shutdown`] can take: the connection drain and runtime wind-down share one
/// [`SHUTDOWN_GRACE`] deadline, the runtime keeps a 100 ms floor past it, and the sim join waits at
/// most [`SIM_STOP_GRACE`]. A host that stops a listener-bearing core waits at least this long.
pub const TEARDOWN_BOUND: Duration = Duration::from_millis(
    (SHUTDOWN_GRACE.as_millis() + RUNTIME_FLOOR.as_millis() + SIM_STOP_GRACE.as_millis()) as u64,
);

/// The least the runtime wind-down is given once the drain has used the whole grace.
const RUNTIME_FLOOR: Duration = Duration::from_millis(100);

/// A running WebSocket host for one [`Engine`].
///
/// Construct with [`Server::bind`]; it owns the sim thread and the tokio runtime for its whole life.
/// Dropping it (or calling [`shutdown`](Server::shutdown)) stops the accept loop, closes open
/// connections, signals the sim thread, and joins it — within a bounded grace period, past which a
/// wedged sim thread is reported (`L4022`) and left detached.
pub struct Server {
    local_addr: SocketAddr,
    /// One retained intake sender so the sim thread's receiver stays connected even with zero
    /// clients (dropped on teardown). Connection tasks clone their own from the accept loop's copy.
    _intake_keepalive: Sender<StampedCommand>,
    /// Tells the accept loop and connection tasks to stop.
    connections_shutdown: watch::Sender<bool>,
    /// Tells the sim thread's `run_live` loop to stop.
    sim_shutdown: ShutdownSignal,
    /// Fires when a wire `engine.shutdown` command trips the engine's shutdown flag, so a host can
    /// wake from its Ctrl-C wait and run this server's graceful teardown (see
    /// [`wire_shutdown`](Server::wire_shutdown)). Held to keep the channel open for clients of it.
    wire_shutdown: Receiver<()>,
    runtime: Option<Runtime>,
    sim_thread: Option<JoinHandle<Result<(), Diagnostic>>>,
    /// The std thread bridging the engine's crossbeam egress slot into tokio (frame/event delivery).
    /// It ends on its own once the sim thread drops the egress sender; teardown joins it.
    egress_pump: Option<JoinHandle<()>>,
    /// Count of connection tasks that are alive and not yet fully drained. Teardown waits (bounded)
    /// for this to reach zero before dropping the runtime, so an in-flight reply — notably
    /// `engine.shutdown`'s `Ack` — is written to the socket instead of cancelled mid-write.
    active_connections: Arc<AtomicUsize>,
}

impl Server {
    /// Binds a WebSocket host serving `engine` on `addr`, spawning the sim thread and a 2-worker
    /// tokio runtime, and returns once the listener has a concrete bound address.
    ///
    /// `addr` accepts anything [`std::net::ToSocketAddrs`] does (`"127.0.0.1:0"`, a [`SocketAddr`],
    /// …). Passing port `0` asks the OS for a free port; read the chosen one back with
    /// [`local_addr`](Server::local_addr) or [`url`](Server::url).
    ///
    /// # Errors
    /// - `L3009` if the address cannot be resolved, the tokio runtime cannot be built, or the TCP
    ///   listener cannot bind (the message names the underlying cause).
    pub fn bind(
        mut engine: Engine,
        addr: impl std::net::ToSocketAddrs,
    ) -> Result<Server, Diagnostic> {
        let addr = addr
            .to_socket_addrs()
            .map_err(|e| startup_error(format!("could not resolve the bind address: {e}")))?
            .next()
            .ok_or_else(|| startup_error("the bind address resolved to no socket addresses"))?;

        // Everything that can fail on a bad address or a busy port happens before the sim thread
        // spawns, so those errors return with the engine still on this thread, never run.
        // The tokio runtime: two workers, confined to this crate (architecture §3).
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()
            .map_err(|e| startup_error(format!("could not build the tokio runtime: {e}")))?;
        // Bind synchronously so the caller gets the concrete port before `bind` returns.
        let listener = runtime
            .block_on(TcpListener::bind(addr))
            .map_err(|e| startup_error(format!("could not bind {addr}: {e}")))?;
        let local_addr = listener
            .local_addr()
            .map_err(|e| startup_error(format!("could not read the bound address: {e}")))?;

        // The intake queue (many producers → one sim-thread consumer) and the two shutdown seams.
        let (intake_tx, intake_rx) = crossbeam_channel::unbounded::<StampedCommand>();
        let (sim_shutdown, sim_shutdown_rx) = shutdown_channel();
        let (connections_shutdown, connections_shutdown_rx) = watch::channel(false);

        // The wire-shutdown wake: an `engine.shutdown` command dispatched on the sim thread trips this
        // so a host can wake from its Ctrl-C wait and run `teardown` (the same graceful path). Armed on
        // the engine before it moves onto the sim thread; a bare in-process engine has no such channel.
        let (wire_shutdown_tx, wire_shutdown_rx) = crossbeam_channel::bounded::<()>(1);
        engine.arm_shutdown_notify(wire_shutdown_tx);

        // The egress seam: two lanes (see `ludion_engine::egress`). Frames ride a bounded(1),
        // latest-wins-at-source slot — the engine's render-skip depends on it being bounded(1), skipping
        // a frame while the previous is undrained. Events ride their own unbounded, ordered lane so a
        // burst of events is never displaced by a pending frame (audit finding 1.3): that is the
        // lossless path for subscription events.
        let (frame_egress_tx, frame_egress_rx) = crossbeam_channel::bounded::<Egress>(1);
        let (event_egress_tx, event_egress_rx) = crossbeam_channel::unbounded::<Egress>();

        // The sim thread: sole owner of the engine/world for its whole life. It emits frames and events
        // on the two lanes above; the pump + router + forwarders (below) carry them to the connections.
        let sim_thread = std::thread::Builder::new()
            .name("ludion-sim".into())
            .spawn(move || {
                run_live(
                    engine,
                    intake_rx,
                    sim_shutdown_rx,
                    Some(EgressChannels {
                        frames: frame_egress_tx,
                        events: event_egress_tx,
                    }),
                )
            })
            .map_err(|e| startup_error(format!("could not spawn the sim thread: {e}")))?;

        // Bridge the two crossbeam egress lanes into tokio: a std pump thread drains both eagerly —
        // events into an unbounded mpsc (ordered, never coalesced), frames into a single watch slot
        // (latest-wins: a frame the router hasn't picked up is superseded, never queued). The pump does
        // no encoding — it must recv again immediately, since the engine drops frames while the frame
        // slot is full.
        let (event_tx, event_rx) =
            tokio::sync::mpsc::unbounded_channel::<ludion_engine::egress::EgressEvent>();
        let (frame_tx, frame_rx) = watch::channel::<Option<Arc<EgressFrame>>>(None);
        let egress_pump = std::thread::Builder::new()
            .name("ludion-egress-pump".into())
            .spawn(move || {
                delivery::run_egress_pump(frame_egress_rx, event_egress_rx, event_tx, frame_tx)
            })
            .map_err(|e| startup_error(format!("could not spawn the egress pump: {e}")));
        let egress_pump = match egress_pump {
            Ok(pump) => pump,
            Err(error) => {
                // The sim thread is live and owns the engine: stop and join it rather than detach it.
                sim_shutdown.signal();
                let _ = join_sim(sim_thread, SIM_STOP_GRACE, &sim_shutdown);
                return Err(error);
            }
        };

        // The shared subscription registry (sub id → per-subscriber delivery state), read by the
        // routers and written by each connection as its subscribe/unsubscribe replies pass through.
        let registry = delivery::new_registry();
        // The transport-timing hub: the third profiling layer, which only this side can measure.
        // Disarmed until a client subscribes to `profile` or issues a `profile.capture`.
        let hub = perf::ProfileHub::new();
        // The routers: one delivers events in order (completing a `profile` window's transport
        // section on the way past); the other encodes the freshest pending frame once, timing the
        // encode, and fans it out. Both end when the pump drops its senders.
        runtime.spawn(delivery::event_router(
            event_rx,
            registry.clone(),
            hub.clone(),
        ));
        runtime.spawn(delivery::frame_router(
            frame_rx,
            registry.clone(),
            hub.clone(),
        ));

        let hello = ServerHello {
            engine_version: ENGINE_VERSION.to_string(),
            protocol_version: PROTOCOL_VERSION,
            schema_hash: schema::schema_hash(),
        };
        let active_connections = Arc::new(AtomicUsize::new(0));
        runtime.spawn(connection::accept_loop(
            listener,
            intake_tx.clone(),
            hello,
            registry,
            hub,
            active_connections.clone(),
            connections_shutdown_rx,
        ));

        Ok(Server {
            local_addr,
            _intake_keepalive: intake_tx,
            connections_shutdown,
            sim_shutdown,
            wire_shutdown: wire_shutdown_rx,
            runtime: Some(runtime),
            sim_thread: Some(sim_thread),
            egress_pump: Some(egress_pump),
            active_connections,
        })
    }

    /// The concrete socket address the host is listening on (with the real port when `0` was asked).
    pub fn local_addr(&self) -> SocketAddr {
        self.local_addr
    }

    /// The `ws://host:port` URL clients connect to.
    pub fn url(&self) -> String {
        format!("ws://{}", self.local_addr)
    }

    /// A [`CommandSink`] feeding this server's one intake queue — the same queue a wire request feeds.
    /// Host plumbing for injecting ordinary [`Command`]s (e.g. watcher-forwarded `asset.reload`s), not
    /// a protocol bypass: every submitted command runs through the same parity gate.
    pub fn command_sink(&self) -> CommandSink {
        CommandSink {
            intake: self._intake_keepalive.clone(),
        }
    }

    /// Spawns a filesystem watcher over `root` that forwards every changed path into the intake queue
    /// as an ordinary `asset.reload` command — so a texture edited on disk reloads live, even while the
    /// engine is paused. The engine ignores a forwarded path it does not cache, so the watch can be
    /// broad; the reply is discarded (the reload's effect is the fired `asset.reloaded` event and the
    /// re-rendered frame). Returns the [`AssetWatch`] whose drop stops the watch **and** joins its
    /// forwarding thread; keep it alive for as long as the watch should run, and drop it before
    /// [`shutdown`](Server::shutdown).
    ///
    /// # Errors
    /// `L1103` if the OS watcher cannot start (or the forwarder thread cannot be spawned) — a host
    /// degrades from this by logging it and serving on with explicit `asset.reload` only, never a dead
    /// host.
    pub fn watch_assets(&self, root: &Path, debounce: Duration) -> Result<AssetWatch, Diagnostic> {
        let (changes, handle) = ludion_assets::watch(root, debounce)?;
        let sink = self.command_sink();
        let forwarder = std::thread::Builder::new()
            .name("ludion-asset-forward".into())
            .spawn(move || {
                // Ends when the watcher's handle is dropped: `changes` disconnects and the loop stops.
                for batch in changes {
                    for path in batch {
                        sink.submit(Command::AssetReload(AssetReloadParams { path: Some(path) }));
                    }
                }
            })
            .map_err(|e| {
                Diagnostic::new(
                    code::WATCHER_START_FAILED,
                    format!("could not spawn the asset-watch forwarder thread: {e}"),
                )
            })?;
        Ok(AssetWatch {
            watch: Some(handle),
            forwarder: Some(forwarder),
        })
    }

    /// A receiver that fires once a wire `engine.shutdown` command has been dispatched, so a host can
    /// wait on it alongside Ctrl-C and then run [`shutdown`](Server::shutdown) — the one graceful
    /// teardown, whether the stop came from the keyboard or the wire. The returned receiver is a clone
    /// sharing the one channel; a delivered wake is consumed by whichever receiver reads it first, so a
    /// host uses exactly one (a direct wait, or a forwarder into a window's stop channel).
    pub fn wire_shutdown(&self) -> Receiver<()> {
        self.wire_shutdown.clone()
    }

    /// Stops the host: closes the accept loop and every connection, signals the sim thread, and
    /// joins it. Equivalent to dropping the server, but explicit and able to run before scope end.
    ///
    /// The join waits at most [`SIM_STOP_GRACE`]. Returns `Some` when the sim did not stop cleanly —
    /// `L4017` if it ended in a panic, `L4022` if it was still busy past the grace (the thread is then
    /// left detached), or the armed recording's failure (`L3042`, or its final flush's error): the
    /// host must exit non-zero rather than report a clean stop.
    pub fn shutdown(mut self) -> Option<Diagnostic> {
        self.teardown()
    }

    /// [`shutdown`](Server::shutdown) for tests: panics with the diagnostic if the sim thread did not
    /// stop cleanly, so a sim-thread panic fails the test that caused it.
    #[doc(hidden)]
    pub fn shutdown_clean(self) {
        if let Some(diagnostic) = self.shutdown() {
            panic!("server did not shut down cleanly: {diagnostic:?}");
        }
    }

    /// The shared teardown used by both [`shutdown`](Server::shutdown) and [`Drop`] (which ignores
    /// the result: an explicit `shutdown` has already taken the sim handle by then).
    fn teardown(&mut self) -> Option<Diagnostic> {
        self.teardown_within(SIM_STOP_GRACE)
    }

    /// [`teardown`](Server::teardown) with the sim thread's join bounded by `sim_grace`.
    fn teardown_within(&mut self, sim_grace: Duration) -> Option<Diagnostic> {
        // 1. Tell the accept loop and connection tasks to stop: readers finish any in-flight request,
        //    queue its reply, then observe the flag and close.
        let _ = self.connections_shutdown.send(true);
        // 2. Tell the sim thread to stop.
        self.sim_shutdown.signal();
        // 3. Let connection tasks drain and close *before* dropping the runtime, so a reply already
        //    handed back by the sim thread — notably `engine.shutdown`'s `Ack` — is actually written
        //    to the socket rather than cancelled mid-write when the runtime is dropped. The runtime's
        //    own worker threads keep making progress while this thread polls; bounded so it can never
        //    hang (a wedged client is force-closed by step 4's timeout).
        let drain_deadline = Instant::now() + SHUTDOWN_GRACE;
        while self.active_connections.load(Ordering::Acquire) > 0 && Instant::now() < drain_deadline
        {
            std::thread::sleep(Duration::from_millis(5));
        }
        // 4. Wind the runtime down with the remaining grace as a hard backstop. This drops the
        //    connection tasks' intake senders too.
        if let Some(runtime) = self.runtime.take() {
            let remaining = drain_deadline
                .saturating_duration_since(Instant::now())
                .max(RUNTIME_FLOOR);
            runtime.shutdown_timeout(remaining);
        }
        // 5. Join the sim thread (it has already observed the shutdown signal or the now-disconnected
        //    intake channel), waiting at most `sim_grace`.
        let mut sim_fault = None;
        if let Some(handle) = self.sim_thread.take() {
            match join_sim(handle, sim_grace, &self.sim_shutdown) {
                Ok(Ok(Ok(()))) => {}
                Ok(Ok(Err(recording))) => sim_fault = Some(recording),
                Ok(Err(payload)) => sim_fault = Some(sim_thread_panicked(payload.as_ref())),
                Err(stuck) => {
                    // The pump only ends when the sim thread drops its egress sender, and a stuck
                    // sim never does: joining it would hang here too, so detach it with the sim.
                    drop(self.egress_pump.take());
                    return Some(stuck);
                }
            }
        }
        // 6. Join the egress pump. The sim thread has dropped the egress sender by now, so the pump's
        //    blocking `recv` has returned and the thread is exiting — the join cannot hang.
        if let Some(handle) = self.egress_pump.take() {
            let _ = handle.join();
        }
        sim_fault
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        let _ = self.teardown();
    }
}

/// A running asset watch (see [`Server::watch_assets`]). Dropping it stops the watcher, then joins
/// the forwarding thread, so nothing is submitted into the intake after the drop. Drop it before
/// [`Server::shutdown`], so no change is submitted into a stopping queue.
pub struct AssetWatch {
    watch: Option<WatchHandle>,
    forwarder: Option<JoinHandle<()>>,
}

impl Drop for AssetWatch {
    fn drop(&mut self) {
        // The watcher's drop joins its debounce thread, which disconnects `changes` and ends the
        // forwarder; the join is bounded because `CommandSink::submit` never blocks.
        drop(self.watch.take());
        if let Some(forwarder) = self.forwarder.take() {
            let _ = forwarder.join();
        }
    }
}

/// A host-side seam to submit a [`Command`] into a [`Server`]'s one intake queue (see
/// [`Server::command_sink`]).
///
/// Fire-and-forget: the command runs through [`ludion_engine::dispatch`] like any wire request, but the
/// reply is dropped. Cloneable and `Send`, so a background thread (a filesystem-watcher forwarder) can
/// own one for the server's whole life.
#[derive(Clone)]
pub struct CommandSink {
    intake: Sender<StampedCommand>,
}

impl CommandSink {
    /// Submits `command` into the intake queue, discarding its reply. A no-op if the sim thread has
    /// already gone (teardown) — the send simply fails.
    pub fn submit(&self, command: Command) {
        let (stamped, _reply) = StampedCommand::new(command);
        let _ = self.intake.send(stamped);
    }

    /// Forwards an already-stamped command into the intake queue with its reply sender intact, so
    /// the host that stamped it reads the answer. Host plumbing onto the same intake a wire request
    /// feeds; a no-op after teardown, like [`submit`](CommandSink::submit).
    pub fn forward(&self, stamped: StampedCommand) {
        let _ = self.intake.send(stamped);
    }
}

/// Builds an `L3009` engine-unavailable diagnostic for a startup failure (the engine never came up).
fn startup_error(message: impl Into<String>) -> Diagnostic {
    Diagnostic::new(code::PROTOCOL_ENGINE_UNAVAILABLE, message)
        .with_help("check the bind address is free and well-formed, then retry")
}

#[cfg(test)]
mod tests {
    use super::*;
    use ludion_engine::live::Activity;
    use ludion_protocol::command::SimStepParams;

    #[test]
    fn a_busy_port_fails_the_bind_before_the_sim_ever_runs() {
        let occupied = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let ran = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let flag = ran.clone();
        let mut engine = Engine::builder()
            .system("mark_ran", move |_, _| flag.store(true, Ordering::Release))
            .build()
            .unwrap();
        engine
            .load_inline_scene("format: 1\nentities: []\n")
            .unwrap();

        let error = Server::bind(engine, occupied.local_addr().unwrap())
            .err()
            .expect("the port is taken");
        assert_eq!(error.code, code::PROTOCOL_ENGINE_UNAVAILABLE);
        std::thread::sleep(Duration::from_millis(100));
        assert!(
            !ran.load(Ordering::Acquire),
            "the sim thread must not spawn for a bind that fails"
        );
    }

    #[test]
    fn a_wedged_sim_step_does_not_hang_teardown() {
        // A system that never returns in test time wedges the sim inside one step.
        let mut engine = Engine::builder()
            .system("wedge", |_, _| std::thread::sleep(Duration::from_secs(600)))
            .build()
            .unwrap();
        engine
            .load_inline_scene("format: 1\nentities: []\n")
            .unwrap();
        let mut server = Server::bind(engine, "127.0.0.1:0").unwrap();
        server
            .command_sink()
            .submit(Command::SimStep(SimStepParams {
                ticks: 1,
                hash: false,
            }));
        // Bounded by a deadline, not a poll count: a loaded box can starve the sim thread for seconds.
        let deadline = std::time::Instant::now() + Duration::from_secs(60);
        while server.sim_shutdown.activity() != Activity::Tick
            && std::time::Instant::now() < deadline
        {
            std::thread::sleep(Duration::from_millis(5));
        }
        assert_eq!(
            server.sim_shutdown.activity(),
            Activity::Tick,
            "the sim never reached the tick"
        );

        let (done_tx, done_rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let stuck = server.teardown_within(Duration::from_millis(200));
            let _ = done_tx.send(stuck);
            std::mem::forget(server);
        });
        let stuck = done_rx
            .recv_timeout(Duration::from_secs(10))
            .expect("teardown hung on a wedged sim thread")
            .expect("a wedged sim is reported");
        assert_eq!(stuck.code.as_str(), "L4022");
        assert!(stuck.message.contains("sim tick"), "{}", stuck.message);
    }
}
