#![forbid(unsafe_code)]
#![warn(missing_docs)]

//! `ludion-window` — the native attach viewer for a running Ludion server (`ludion dev --window`).
//!
//! This crate opens an OS window that shows the game, and it does so **without any privileged access
//! to the engine**. It is an in-process *peer* protocol client, exactly like the Tauri viewer or an
//! agent: it connects over [`ludion_client`] to an already-running host, subscribes to the `frames`
//! channel, decodes the streamed frames, and paints them into a `wgpu` surface; keyboard events go
//! back to the engine as `input.inject` commands. There is no back door to the renderer's GPU state.
//! A game's own `--window` is a different role — the facade presents the engine's offscreen frame
//! in-process, no client involved — but both windows show the very texture a headless agent's
//! screenshot query reads, which is what keeps the parity pillar intact; this viewer is the window
//! that must outlive its host (a `ludion dev` child restarting under it), so it stays a client.
//!
//! # Threading
//!
//! [`run`] must be called on the process's **main thread** — winit requires the event loop to live
//! there. Inside, four threads cooperate:
//!
//! - **The main thread** owns the winit event loop and the `wgpu` surface. On `resumed` it creates
//!   the window and the presenter; on `RedrawRequested` it draws the latest decoded frame,
//!   letterboxed against a black clear; it reconfigures the surface on resize and on a lost swapchain.
//! - **The stream thread** owns its own [`Client`], subscribes to `frames`
//!   with lossy latest-wins delivery, decodes each frame's QOI payload, and drops it into a shared
//!   latest-wins slot — then wakes the event loop through an [`EventLoopProxy`] so it repaints. It
//!   **survives a lost connection**: on a drop it holds the last frame, flags the window title, and
//!   re-subscribes with backoff (the `reconnect` module), for as long as the window is open. This is
//!   what lets the window ride `ludion dev`'s reload blink, or a child that fails to boot, on the
//!   same port.
//! - **The input thread** owns a second `Client` and forwards mapped input events — keyboard edges
//!   from the event loop, mouse edges from the event loop, gamepad samples from the gamepad thread,
//!   all pre-built `input.inject` event `Value`s sharing one channel — as `input.inject` commands. It
//!   also survives a lost connection — injections during the blink are simply dropped (the sim is
//!   down) and it reconnects lazily on the next event — and it never ends the loop; the stream thread
//!   owns the fatal path.
//! - **The gamepad thread** owns a [`gilrs::Gilrs`] instance and polls it (`next_event_blocking` with
//!   a short timeout, so it wakes promptly to check for shutdown) for the first connected pad only
//!   (when it unplugs, it releases what that pad held and promotes the next one),
//!   translating axis/button samples into `gamepad_*` event `Value`s onto the same channel the input
//!   thread drains (`gamepad.rs` holds the pure mapping; capture is out-of-sim by construction — the
//!   sim only ever sees what this thread injects).
//! - **A small waiter thread** blocks on the caller's `stop` receiver and, when it fires, asks the
//!   event loop to exit — this is how a host wires Ctrl-C.
//!
//! Closing the window or firing `stop` ends the loop cleanly (no `process::exit`); a lost connection
//! does not end it, but a fatal schema mismatch does, with the diagnostic. Every thread is then
//! joined with bounded waits before [`run`] returns.
//!
//! The [`host_input`] router is public because the `ludion` facade's in-process present host calls
//! the same device-event → `input.inject`-event path directly, without going through a `Client` — so
//! a session captured through either window records the same `input.inject` events regardless of
//! which host produced them. The `keymap`, `mouse` and `gamepad` modules and
//! [`spawn_gamepad_thread`] stay public as its building blocks. The transport
//! (this crate's stream/input threads over [`ludion_client`]) stays private: it is this crate's own
//! peer-client concern, not something a different host would want.
//!
//! # Example
//!
//! ```no_run
//! use ludion_window::{run, WindowOptions};
//!
//! // A host wires `stop` to Ctrl-C; here we just never fire it.
//! let (_stop_tx, stop_rx) = crossbeam_channel::unbounded();
//! let options = WindowOptions {
//!     url: "ws://127.0.0.1:4321".to_string(),
//!     title: "Ludion".to_string(),
//!     width: 1280,
//!     height: 720,
//! };
//! run(options, stop_rx)?; // blocks until the window closes or the connection is lost
//! # Ok::<(), ludion_core::diagnostics::Diagnostic>(())
//! ```

pub mod chrome;
mod frame;
pub mod gamepad;
mod gpu;
pub mod host_input;
pub mod keymap;
pub mod mouse;
mod reconnect;

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::Duration;

use crossbeam_channel::{Receiver, Sender};
use gilrs::{GamepadId, Gilrs};
use ludion_client::Client;
use ludion_core::codes::code;
use ludion_core::diagnostics::Diagnostic;
use ludion_protocol::Command;
use ludion_protocol::command::InputInjectParams;
use winit::application::ApplicationHandler;
use winit::dpi::LogicalSize;
use winit::event::WindowEvent;
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop, EventLoopProxy};
use winit::window::{Window, WindowId};

pub use crate::frame::DecodedFrame;
pub use crate::reconnect::{StreamSink, run_frame_stream};

use crate::gpu::{Presenter, RenderError};
use crate::host_input::HostInput;

/// How the streamed frames are decoded to RGBA and handed between threads: a latest-wins slot that
/// the stream thread replaces and the main thread reads. `Arc` so a redraw clones only a pointer,
/// never the megabyte of pixels.
type FrameSlot = Arc<Mutex<Option<Arc<DecodedFrame>>>>;

/// The window title while the stream is retrying a lost connection: the base title with a visible
/// reconnect flag. This flag is the plan's "visible state" — a reconnect must never silently mask a
/// real failure. Pure so the state transition is unit-testable without a window.
fn title_for(base: &str, reconnecting: bool) -> String {
    if reconnecting {
        format!("{base} — reconnecting…")
    } else {
        base.to_string()
    }
}

/// Options for opening the viewer window.
pub struct WindowOptions {
    /// The `ws://host:port` address of the running `ludion serve` host to attach to.
    pub url: String,
    /// The window title.
    pub title: String,
    /// Initial inner width in logical pixels (points; a typical value is 1280), shrunk to fit the monitor.
    pub width: u32,
    /// Initial inner height in logical pixels (points; a typical value is 720), shrunk to fit the monitor.
    pub height: u32,
}

/// Opens the viewer window and runs it until the window is closed, `stop` fires, or a schema
/// mismatch is seen. A lost connection is retried for as long as the window is open.
///
/// **Must be called on the process's main thread** (a winit requirement). Blocks for the lifetime of
/// the window. The `stop` receiver is the host's shutdown hook: sending `()` (or dropping every
/// sender) asks the window to close cleanly, the same way clicking the close button does.
///
/// # Errors
/// - The handshake's diagnostic (`L3003`, schema mismatch) if a different engine build is on
///   the port; an unreachable or dropped server is retried, never an error.
/// - `L2007` if a window / GPU surface could not be created, `L2003` if no compatible GPU adapter is
///   available, `L2004` if the GPU device could not be created.
///
/// Closing the window or firing `stop` returns `Ok(())`.
pub fn run(options: WindowOptions, stop: Receiver<()>) -> Result<(), Diagnostic> {
    let event_loop = EventLoop::<UserEvent>::with_user_event()
        .build()
        .map_err(|e| {
            Diagnostic::new(
                code::WINDOW_PRESENTATION_UNAVAILABLE,
                format!("could not create the OS event loop: {e}"),
            )
            .with_help(
                "the windowed viewer needs an interactive desktop session; on a headless machine, \
                 attach the streaming viewer over the wire protocol instead",
            )
        })?;
    event_loop.set_control_flow(ControlFlow::Wait);
    let proxy = event_loop.create_proxy();

    let slot: FrameSlot = Arc::new(Mutex::new(None));
    let running = Arc::new(AtomicBool::new(true));
    let (input_tx, input_rx) = crossbeam_channel::unbounded::<serde_json::Value>();
    // Wakes the waiter thread on loop exit when `stop` never fired (e.g. the window was closed).
    let (internal_stop_tx, internal_stop_rx) = crossbeam_channel::bounded::<()>(1);

    let stream = spawn_stream_thread(
        options.url.clone(),
        slot.clone(),
        proxy.clone(),
        running.clone(),
    );
    let input = spawn_input_thread(options.url.clone(), input_rx);
    let gamepad = spawn_gamepad_thread(input_tx.clone(), running.clone());
    let waiter = spawn_waiter_thread(stop, internal_stop_rx, proxy.clone());

    let mut app = App {
        options,
        slot,
        input_tx,
        input: HostInput::default(),
        presenter: None,
        window: None,
        window_size: (0, 0),
        redraw_after_reconfigure: false,
        fatal: None,
    };
    let loop_result = event_loop.run_app(&mut app);

    // Tear down: stop the stream thread and the gamepad poll, release the input channel so its
    // thread ends, wake the waiter, then join everything with the bounded waits each thread respects.
    running.store(false, Ordering::Relaxed);
    let fatal = app.fatal.take();
    drop(app); // drops `input_tx`, ending the input thread's `recv`
    let _ = internal_stop_tx.send(());
    let _ = stream.join();
    let _ = input.join();
    let _ = gamepad.join();
    let _ = waiter.join();

    if let Err(e) = loop_result {
        return Err(Diagnostic::new(
            code::WINDOW_PRESENTATION_UNAVAILABLE,
            format!("the window event loop exited with an error: {e}"),
        ));
    }
    match fatal {
        Some(diagnostic) => Err(diagnostic),
        None => Ok(()),
    }
}

/// Events pushed to the event loop from the worker threads.
enum UserEvent {
    /// A newer decoded frame is in the slot; repaint.
    FrameReady,
    /// The stream connection dropped and is being retried; flag the title, hold the last frame.
    Reconnecting,
    /// The stream reconnected and resubscribed; restore the title.
    Reconnected,
    /// The stream hit a fatal schema mismatch. Carries the diagnostic to surface.
    ConnectionLost(Diagnostic),
    /// The caller's `stop` fired; exit the loop cleanly.
    Shutdown,
}

/// The winit application: window + presenter, plus the shared slot and the input channel.
struct App {
    options: WindowOptions,
    slot: FrameSlot,
    input_tx: Sender<serde_json::Value>,
    /// Maps keyboard, mouse and focus events to `input.inject` events.
    input: HostInput,
    presenter: Option<Presenter>,
    window: Option<Arc<Window>>,
    /// The window's inner size in physical pixels; the surface may be smaller (the texture cap).
    window_size: (u32, u32),
    /// Set when a reconfigure requested a redraw, cleared by the next successful draw, so a surface
    /// that stays lost reconfigures without spinning the loop.
    redraw_after_reconfigure: bool,
    /// Set when a fatal error (lost connection, out-of-memory) ends the loop; becomes `run`'s `Err`.
    fatal: Option<Diagnostic>,
}

impl App {
    /// Records a fatal diagnostic (first one wins) and asks the loop to exit.
    fn fail(&mut self, event_loop: &ActiveEventLoop, diagnostic: Diagnostic) {
        if self.fatal.is_none() {
            self.fatal = Some(diagnostic);
        }
        event_loop.exit();
    }

    /// Flags the window title while the stream is reconnecting (the visible reconnect state).
    fn set_reconnecting_title(&self) {
        if let Some(window) = &self.window {
            window.set_title(&title_for(&self.options.title, true));
        }
    }

    /// Restores the base window title once the stream is connected again.
    fn set_connected_title(&self) {
        if let Some(window) = &self.window {
            window.set_title(&title_for(&self.options.title, false));
        }
    }

    /// Draws the current frame, handling the swapchain error cases: reconfigure on a lost/outdated
    /// surface, fail on out-of-memory, skip a timed-out acquire.
    fn redraw(&mut self, event_loop: &ActiveEventLoop) {
        let Some(presenter) = self.presenter.as_mut() else {
            return;
        };
        let frame = self.slot.lock().expect("frame slot poisoned").clone();
        match presenter.render(frame) {
            Ok(()) => self.redraw_after_reconfigure = false,
            Err(RenderError::Reconfigure) => {
                presenter.reconfigure();
                if !self.redraw_after_reconfigure {
                    self.redraw_after_reconfigure = true;
                    if let Some(window) = &self.window {
                        window.request_redraw();
                    }
                }
            }
            Err(RenderError::Fatal(diagnostic)) => self.fail(event_loop, diagnostic),
        }
    }
}

/// Maps a cursor position in window physical pixels to render-target pixels: first into the
/// presenter's surface space (the surface is capped below an oversized window), then through the
/// letterbox against the latest decoded frame's size — the same dimensions [`App::redraw`] draws
/// with. `None` before a window/presenter exists or before any frame has been decoded yet.
fn map_cursor(
    presenter: Option<&Presenter>,
    slot: &FrameSlot,
    window_size: (u32, u32),
    x: f32,
    y: f32,
) -> Option<[f32; 2]> {
    let (surface_w, surface_h) = presenter?.size();
    let frame = slot.lock().expect("frame slot poisoned").clone()?;
    let cursor = mouse::window_to_surface((x, y), window_size, (surface_w, surface_h));
    mouse::map_cursor_to_frame(
        cursor,
        frame.width,
        frame.height,
        surface_w,
        surface_h,
        None,
    )
}

impl ApplicationHandler<UserEvent> for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.window.is_some() {
            return; // already created (resumed can fire more than once on some platforms)
        }
        let attributes = Window::default_attributes()
            .with_title(self.options.title.clone())
            .with_inner_size(chrome::first_window_size(
                LogicalSize::new(
                    f64::from(self.options.width),
                    f64::from(self.options.height),
                ),
                chrome::monitor_logical_size(event_loop),
            ));
        let window = match event_loop.create_window(attributes) {
            Ok(window) => Arc::new(window),
            Err(e) => {
                self.fail(
                    event_loop,
                    Diagnostic::new(
                        code::WINDOW_PRESENTATION_UNAVAILABLE,
                        format!("could not create the window: {e}"),
                    ),
                );
                return;
            }
        };
        match Presenter::new(window.clone()) {
            Ok(presenter) => {
                self.presenter = Some(presenter);
                let size = window.inner_size();
                self.window_size = (size.width, size.height);
                self.window = Some(window);
            }
            Err(diagnostic) => self.fail(event_loop, diagnostic),
        }
    }

    fn user_event(&mut self, event_loop: &ActiveEventLoop, event: UserEvent) {
        match event {
            UserEvent::FrameReady => {
                if let Some(window) = &self.window {
                    window.request_redraw();
                }
            }
            UserEvent::Reconnecting => self.set_reconnecting_title(),
            UserEvent::Reconnected => self.set_connected_title(),
            UserEvent::ConnectionLost(diagnostic) => self.fail(event_loop, diagnostic),
            UserEvent::Shutdown => event_loop.exit(),
        }
    }

    fn window_event(
        &mut self,
        event_loop: &ActiveEventLoop,
        _window_id: WindowId,
        event: WindowEvent,
    ) {
        match event {
            WindowEvent::CloseRequested => event_loop.exit(),
            WindowEvent::Resized(size) => {
                self.window_size = (size.width, size.height);
                if let Some(presenter) = self.presenter.as_mut() {
                    presenter.resize(size.width, size.height);
                }
                if let Some(window) = &self.window {
                    window.request_redraw();
                }
            }
            WindowEvent::RedrawRequested => self.redraw(event_loop),
            event => self.input.route(
                &event,
                |x, y| map_cursor(self.presenter.as_ref(), &self.slot, self.window_size, x, y),
                // Best-effort: if the input thread has gone, the game just stops receiving
                // input — never a reason to tear the window down.
                |value| {
                    let _ = self.input_tx.send(value);
                },
            ),
        }
    }
}

/// The stream thread: run the reconnecting frame stream, pushing frames into the shared slot and the
/// reconnect lifecycle into the event loop. Ends when `running` goes false (clean) or the connection
/// is permanently lost (surfaces the diagnostic as `ConnectionLost`).
fn spawn_stream_thread(
    url: String,
    slot: FrameSlot,
    proxy: EventLoopProxy<UserEvent>,
    running: Arc<AtomicBool>,
) -> JoinHandle<()> {
    std::thread::spawn(move || {
        let sink = WindowStreamSink {
            slot,
            proxy: proxy.clone(),
            running,
        };
        if let Some(diagnostic) = run_frame_stream(&url, &sink) {
            let _ = proxy.send_event(UserEvent::ConnectionLost(diagnostic));
        }
    })
}

/// The window's [`StreamSink`]: frames land in the latest-wins slot and wake a repaint; the reconnect
/// lifecycle drives the title; `keep_running` mirrors the shared shutdown flag.
struct WindowStreamSink {
    slot: FrameSlot,
    proxy: EventLoopProxy<UserEvent>,
    running: Arc<AtomicBool>,
}

impl StreamSink for WindowStreamSink {
    fn frame(&self, frame: DecodedFrame) {
        *self.slot.lock().expect("frame slot poisoned") = Some(Arc::new(frame));
        let _ = self.proxy.send_event(UserEvent::FrameReady);
    }

    fn reconnecting(&self) {
        let _ = self.proxy.send_event(UserEvent::Reconnecting);
    }

    fn reconnected(&self) {
        let _ = self.proxy.send_event(UserEvent::Reconnected);
    }

    fn keep_running(&self) -> bool {
        self.running.load(Ordering::Relaxed)
    }
}

/// The input thread: forward every mapped device event (keyboard, mouse, gamepad — all pre-built
/// `input.inject` event `Value`s from the one channel) as `input.inject` commands, until the channel
/// closes. This thread does not know which device an event came from; it only wraps the
/// already-built `Value`.
///
/// It survives a lost connection: a dropped socket just clears the client, and each subsequent event
/// lazily reconnects (a still-down server or a schema mismatch simply drops that event — the stream
/// thread owns the fatal path and the title flag, so input is always best-effort and never ends the
/// loop). Injections that arrive during the blink are dropped, which is correct: the sim is down.
///
/// Each call is bounded by [`INPUT_CALL_TIMEOUT`]: a sim that stops answering surfaces `L3054`,
/// which drops the client like a lost socket (`L3009`), so this thread never parks behind a wedged
/// sim and can be joined.
fn spawn_input_thread(url: String, input_rx: Receiver<serde_json::Value>) -> JoinHandle<()> {
    std::thread::spawn(move || {
        // Best-effort initial connect; if it fails, the first events drop until a later one reconnects.
        let mut client = connect_input(&url);
        // Ends when every `input_tx` is dropped (the event loop torn down).
        while let Ok(event) = input_rx.recv() {
            if client.is_none() {
                client = connect_input(&url);
            }
            let Some(active) = client.as_mut() else {
                continue; // still down: drop this event
            };
            let command = Command::InputInject(InputInjectParams { frame: None, event });
            if let Err(diagnostic) = active.call(command)
                && matches!(
                    diagnostic.code,
                    code::PROTOCOL_ENGINE_UNAVAILABLE | code::PROTOCOL_CALL_TIMED_OUT
                )
            {
                // Connection lost or timed out: drop the client so the next event reconnects.
                // Never fatal here.
                client = None;
            }
        }
    })
}

/// How long one `input.inject` call may wait for its reply: one sim pass many times over, and far
/// under the server's 120 s.
const INPUT_CALL_TIMEOUT: Duration = Duration::from_secs(1);

fn connect_input(url: &str) -> Option<Client> {
    Client::connect(url)
        .map(|client| client.with_call_timeout(INPUT_CALL_TIMEOUT))
        .ok()
}

/// The gamepad thread: poll `gilrs` for the first connected pad's events and push mapped
/// `gamepad_*` event `Value`s onto `input_tx`, the same channel this crate's own input thread
/// drains (a caller reusing this function, like the facade's in-process present host, drains
/// `input_tx`'s receiver itself instead).
///
/// `Gilrs::new` failing (no backend available, e.g. an unsupported OS configuration) simply ends the
/// thread immediately — the headless/no-gamepad degrade (`InputState` stays default) covers it, same
/// as a machine with the backend but no pad plugged in, which just never yields an event. Polls with
/// a bounded timeout (rather than an unbounded block) so the thread notices `running` going false and
/// exits promptly on teardown, without busy-waiting between samples. Only the first-seen connected
/// pad's events are forwarded (multi-pad is out of scope, `Plan/phase-13-input.md`). A `Disconnected`
/// from that pad first sends the releases for everything it held (axes to zero, buttons up), then
/// promotes the next still-connected pad, if any.
pub fn spawn_gamepad_thread(
    input_tx: Sender<serde_json::Value>,
    running: Arc<AtomicBool>,
) -> JoinHandle<()> {
    std::thread::spawn(move || {
        let mut gilrs = match Gilrs::new() {
            Ok(gilrs) => gilrs,
            Err(_) => return, // no gamepad backend on this machine: nothing to poll, ever
        };
        let mut primary: Option<GamepadId> = gilrs.gamepads().next().map(|(id, _)| id);
        let mut held = gamepad::PadHeld::default();
        while running.load(Ordering::Relaxed) {
            let Some(event) = gilrs.next_event_blocking(Some(Duration::from_millis(100))) else {
                continue; // timed out with no event: loop back and re-check `running`
            };
            match event.event {
                gilrs::EventType::Connected => {
                    if primary.is_none() {
                        primary = Some(event.id);
                    }
                    continue;
                }
                gilrs::EventType::Disconnected => {
                    let connected = gilrs.gamepads().map(|(id, _)| id);
                    for value in
                        gamepad::on_disconnect(&mut primary, event.id, &mut held, connected)
                    {
                        let _ = input_tx.send(value);
                    }
                    continue;
                }
                _ => {}
            }
            if primary != Some(event.id) {
                continue; // a second pad: out of scope, ignore its events
            }
            if let Some(value) = held.forward(&event.event) {
                let _ = input_tx.send(value);
            }
        }
    })
}

/// The waiter thread: forward the caller's `stop` to the loop as a clean shutdown, or exit quietly
/// when the loop tears down first (signaled over `internal_stop`).
fn spawn_waiter_thread(
    stop: Receiver<()>,
    internal_stop: Receiver<()>,
    proxy: EventLoopProxy<UserEvent>,
) -> JoinHandle<()> {
    std::thread::spawn(move || {
        crossbeam_channel::select! {
            recv(stop) -> _ => { let _ = proxy.send_event(UserEvent::Shutdown); }
            recv(internal_stop) -> _ => {}
        }
    })
}

#[cfg(test)]
mod tests {
    use super::title_for;

    #[test]
    fn title_flags_reconnecting_and_restores_the_base() {
        let base = "Ludion — breakout.yaml";
        assert_eq!(title_for(base, false), base);
        assert_eq!(title_for(base, true), format!("{base} — reconnecting…"));
        // Restoring after a reconnect returns exactly the base title (no residue).
        assert_eq!(title_for(base, false), base);
    }
}
