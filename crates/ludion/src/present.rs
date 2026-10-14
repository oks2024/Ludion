//! The windowed shell: the embedding core's winit client — the game's own OS window, with the engine
//! presenting into it in process.
//!
//! `--window` used to bind the server and then open `ludion-window`'s viewer — a peer protocol
//! client that decoded the very frames the host had just encoded for it. This shell skips that round
//! trip. The main thread creates the window's GPU surface and hands it to
//! [`Embedded::start_from`], whose boot thread builds the engine, gives it the surface
//! ([`Engine::attach_surface`](ludion_engine::Engine::attach_surface), which builds the renderer on the
//! device that surface chose), binds the listener, and from then on blits each rendered frame straight
//! into the window after the frame's own submit. Readback and encode still happen — but only when a
//! wire subscriber asks for frames, and out of the same rendered texture, so what a human sees and what
//! an agent's `render.screenshot` returns are the same pixels by construction.
//!
//! The listener binds exactly as it does headless, so an agent, `ludion profile --attach` and the
//! editor attach to a windowed game unchanged. The out-of-process viewer stays `ludion dev
//! --window`'s: that window must outlive a restarting child process, which is a different role, not
//! a second way to do this one.
//!
//! # Threading
//!
//! [`host_windowed`] must run on the process's **main thread** — winit requires the event loop to
//! own it, which is why [`crate::run`] itself must be called from `main`. The main thread makes the
//! window and surface and only forwards: device events become `input.inject` commands through the
//! core's intake. The boot thread owns the engine, builds it, attaches the surface, binds and presents.
//! Three workers sit beside them: `ludion-window`'s `gilrs` poll (shared with the viewer, so both
//! windows produce identical pad events), a forwarder draining the one device-event channel into the
//! core, and a waiter that turns Ctrl-C and a fatal render fault into a stop request and the host
//! ending (a wire `engine.shutdown`, a failed boot, the requested stop) into a loop exit.
//!
//! A stop never leaves the loop by itself: the boot thread drops the engine, and with it the window's
//! surface, and a Metal layer is released on the main thread only — a main thread blocked joining the
//! boot deadlocks it. So a close or Ctrl-C only asks the core to stop, and the loop keeps running
//! until the boot has ended.

use std::process::ExitCode;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use crossbeam_channel::{Receiver, Sender};
use ludion_core::codes::code;
use ludion_core::diagnostics::Diagnostic;
use ludion_engine::live::SIM_STOP_GRACE;
use ludion_window::keymap::wire_key_event;
use ludion_window::mouse;
use winit::application::ApplicationHandler;
use winit::dpi::PhysicalSize;
use winit::event::WindowEvent;
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop, EventLoopProxy};
use winit::keyboard::PhysicalKey;
use winit::window::{Window, WindowId};

use crate::embed::{Embedded, Injector, Source, StartOptions, Surface};
use crate::host;

/// The window's initial inner size in physical pixels, matching the viewer's — the default stream
/// extent, so the first frame lands 1:1 with no letterbox margin.
const WINDOW_WIDTH: u32 = 1280;
const WINDOW_HEIGHT: u32 = 720;

/// How long a requested stop may take before the window gives up on the boot thread. Longer than the
/// server's own bounded teardown ([`SIM_STOP_GRACE`]), so with a listener the server reports `L4022`
/// with the real activity and [`HostEvent::Ended`] wins; this is the backstop for the no-listener
/// shape, where the boot thread is the sim loop itself and a wedged sim never ends it.
const WINDOW_STOP_GRACE: Duration = Duration::from_secs(SIM_STOP_GRACE.as_secs() + 2);

/// When a requested stop runs out of time. The first request arms it and a second close or Ctrl-C
/// never extends it. Pure, so the deadline rule is testable without a window.
#[derive(Default)]
struct StopTimer(Option<Instant>);

impl StopTimer {
    /// Arms the timer at `now` unless it is already armed; returns the deadline either way.
    fn arm(&mut self, now: Instant) -> Instant {
        *self.0.get_or_insert(now + WINDOW_STOP_GRACE)
    }

    fn expired(&self, now: Instant) -> bool {
        self.0.is_some_and(|deadline| now >= deadline)
    }
}

/// Hosts `source` in the game's own window: start the core with the window's surface, and run the
/// winit event loop until the window closes, Ctrl-C arrives, or the host ends (a wire
/// `engine.shutdown`, a failed boot).
///
/// **Must be called on the process's main thread.** The core is stopped cleanly (flushing any armed
/// recording) before this returns, whatever ended the loop. Diagnostics are printed to stderr; the
/// exit code is [`ExitCode::FAILURE`] for a window, surface or boot that failed, an event loop that
/// exited with an error, or a core that did not stop cleanly.
///
/// A window or surface that could not be created is `L2007`; no adapter that can present to the
/// surface is `L2003`/`L2004`; whatever `Server::bind` returns if the address is taken or
/// malformed. A device lost (`L2011`) or a surface that dies (`L2007`) once the game is running
/// ends it the same graceful way: the diagnostic is printed and the exit code is
/// [`ExitCode::FAILURE`].
pub(crate) fn host_windowed(source: Source, options: StartOptions) -> ExitCode {
    let event_loop = match EventLoop::<HostEvent>::with_user_event().build() {
        Ok(event_loop) => event_loop,
        Err(e) => {
            eprintln!(
                "{}",
                unavailable(format!("could not create the OS event loop: {e}"))
            );
            return ExitCode::FAILURE;
        }
    };
    event_loop.set_control_flow(ControlFlow::Wait);
    let proxy = event_loop.create_proxy();

    let running = Arc::new(AtomicBool::new(true));
    let (input_tx, input_rx) = crossbeam_channel::unbounded::<serde_json::Value>();
    // Wakes the waiter thread on loop exit when no stop ever fired (the window was closed). The
    // Ctrl-C handler holds a stop sender for the life of the process, so the waiter can never end by
    // disconnection alone.
    let (internal_stop_tx, internal_stop_rx) = crossbeam_channel::bounded::<()>(1);

    let mut app = App {
        source: Some(source),
        options: Some(options),
        embedded: None,
        proxy,
        running: running.clone(),
        input_tx,
        input_rx: Some(input_rx),
        internal_stop_rx: Some(internal_stop_rx),
        window: None,
        surface_size: (WINDOW_WIDTH, WINDOW_HEIGHT),
        gamepad: None,
        forwarder: None,
        waiter: None,
        fatal: None,
        stop_timer: StopTimer::default(),
        stuck: false,
    };
    let loop_result = event_loop.run_app(&mut app);

    // Tear down: stop the gamepad poll, release the device-event channel so its forwarder ends, wake
    // the waiter, then join all three before the core's own graceful stop.
    running.store(false, Ordering::Relaxed);
    let fatal = app.fatal.take();
    let stuck = app.stuck;
    let embedded = app.embedded.take();
    let workers = [app.gamepad.take(), app.forwarder.take(), app.waiter.take()];
    drop(app); // drops `input_tx`, ending the forwarder's `recv`
    let _ = internal_stop_tx.send(());
    for worker in workers.into_iter().flatten() {
        let _ = worker.join();
    }

    // The loop ends once the boot thread has, so `stop` joins at once and flushes an armed recording
    // before the outcome is reported. If instead a stop outlived `WINDOW_STOP_GRACE`, the boot is
    // abandoned, not joined again: its thread stays detached (`Embedded` has no `Drop`), and its audio
    // sink with it, so nothing on this thread needs forgetting.
    let stopped = match embedded {
        Some(embedded) if stuck => embedded.abandon(WINDOW_STOP_GRACE),
        Some(embedded) => embedded.stop(),
        None => ExitCode::SUCCESS,
    };
    if let Err(e) = loop_result {
        eprintln!(
            "{}",
            unavailable(format!("the window event loop exited with an error: {e}"))
        );
        return ExitCode::FAILURE;
    }
    if let Some(diagnostic) = fatal {
        eprintln!("{diagnostic}");
        return ExitCode::FAILURE;
    }
    stopped
}

/// The one event a worker thread pushes into the loop.
enum HostEvent {
    /// Ctrl-C fired: ask the host to stop.
    Stop,
    /// The engine reported a fatal render fault: stop, and print it on exit.
    RenderFault(Diagnostic),
    /// The boot thread has ended, however it did: exit the loop.
    Ended,
}

/// The winit application: the window and everything the host owns behind it. `embedded` and every
/// field after it are filled in on `resumed`, the one place a window may be created.
struct App {
    /// Taken by the first `resumed`; `None` afterwards, so a second one cannot start a second host.
    source: Option<Source>,
    options: Option<StartOptions>,
    embedded: Option<Embedded>,
    proxy: EventLoopProxy<HostEvent>,
    running: Arc<AtomicBool>,
    input_tx: Sender<serde_json::Value>,
    /// Handed to the forwarder thread on `resumed`.
    input_rx: Option<Receiver<serde_json::Value>>,
    /// Handed to the waiter thread on `resumed`.
    internal_stop_rx: Option<Receiver<()>>,
    window: Option<Arc<Window>>,
    /// The window's inner size in physical pixels: half of the cursor mapping, whose other half is the
    /// size of the frame the render thread last presented ([`Embedded::present_size`]).
    surface_size: (u32, u32),
    gamepad: Option<JoinHandle<()>>,
    forwarder: Option<JoinHandle<()>>,
    waiter: Option<JoinHandle<()>>,
    /// Set when a fatal error ends the loop; printed by [`host_windowed`].
    fatal: Option<Diagnostic>,
    /// Armed by the first stop request; see [`WINDOW_STOP_GRACE`].
    stop_timer: StopTimer,
    /// Set when the loop exited because the stop timer ran out, not because the boot ended.
    stuck: bool,
}

impl App {
    /// Creates the window and its surface, starts the core with them, and starts the workers — in
    /// that order, because the core's boot thread must take the surface before it builds a renderer.
    fn start(&mut self, event_loop: &ActiveEventLoop) -> Result<(), Diagnostic> {
        let (Some(source), Some(options)) = (self.source.take(), self.options.take()) else {
            return Ok(()); // a previous `resumed` already started (or failed); never start twice
        };
        let attributes = Window::default_attributes()
            .with_title("Ludion")
            .with_inner_size(PhysicalSize::new(WINDOW_WIDTH, WINDOW_HEIGHT));
        let window = Arc::new(
            event_loop
                .create_window(attributes)
                .map_err(|e| unavailable(format!("could not create the window: {e}")))?,
        );
        let inner = window.inner_size();

        let instance = wgpu::Instance::default();
        let surface = instance.create_surface(window.clone()).map_err(|e| {
            unavailable(format!(
                "could not create a GPU surface for the window: {e}"
            ))
        })?;
        let embedded = Embedded::start_from(
            source,
            Some(Surface {
                instance,
                surface,
                width: inner.width,
                height: inner.height,
            }),
            options,
        )?;
        if let Some(name) = embedded.scene().file_name() {
            window.set_title(&format!("Ludion — {}", name.to_string_lossy()));
        }

        let input_rx = self
            .input_rx
            .take()
            .expect("the device-event channel is handed over exactly once");
        self.forwarder = Some(spawn_input_forwarder(embedded.injector(), input_rx));
        self.gamepad = Some(ludion_window::spawn_gamepad_thread(
            self.input_tx.clone(),
            self.running.clone(),
        ));
        let internal_stop = self
            .internal_stop_rx
            .take()
            .expect("the internal stop channel is handed over exactly once");
        self.waiter = Some(spawn_waiter_thread(
            host::ctrl_c_stop(),
            embedded.ended(),
            embedded.render_fault(),
            internal_stop,
            self.proxy.clone(),
        ));

        self.surface_size = (inner.width, inner.height);
        self.embedded = Some(embedded);
        self.window = Some(window);
        Ok(())
    }

    /// Asks the host to wind down without leaving the loop: the boot thread drops the engine, and
    /// with it the window's surface, and a Metal layer can only be released on the main thread — so
    /// this thread keeps servicing the loop until the boot reports [`HostEvent::Ended`].
    ///
    /// Arms the stop timer: a sim that never ends the boot must not hang the window forever.
    // The window host owns the wall clock; the stop deadline never touches the sim.
    #[allow(clippy::disallowed_methods)]
    fn request_stop(&mut self, event_loop: &ActiveEventLoop) {
        if let Some(embedded) = &self.embedded {
            embedded.request_stop();
            let deadline = self.stop_timer.arm(Instant::now());
            event_loop.set_control_flow(ControlFlow::WaitUntil(deadline));
        }
    }

    /// Records a fatal diagnostic; the first one wins.
    fn record_fatal(&mut self, diagnostic: Diagnostic) {
        if self.fatal.is_none() {
            self.fatal = Some(diagnostic);
        }
    }

    /// Records a fatal diagnostic and exits the loop. Only safe while the boot does not yet hold
    /// the surface; a runtime fault goes through [`request_stop`](Self::request_stop) instead.
    fn fail(&mut self, event_loop: &ActiveEventLoop, diagnostic: Diagnostic) {
        self.record_fatal(diagnostic);
        event_loop.exit();
    }

    /// Maps a cursor position in window-surface physical pixels to render-target pixels, against the
    /// window's current size and the size of the frame the render thread last presented — the same
    /// two dimensions the blit letterboxes against. `None` before a surface is attached or before
    /// any frame has been presented, which is exactly when a cursor position has nowhere to land.
    fn map_cursor(&self, x: f32, y: f32) -> Option<[f32; 2]> {
        let present_size = self.embedded.as_ref()?.present_size()?;
        let (frame_width, frame_height) = present_size.frame_size()?;
        let (surface_width, surface_height) = self.surface_size;
        mouse::map_cursor_to_frame(
            (x, y),
            frame_width,
            frame_height,
            surface_width,
            surface_height,
            present_size.rect(),
        )
    }
}

impl ApplicationHandler<HostEvent> for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if let Err(diagnostic) = self.start(event_loop) {
            self.fail(event_loop, diagnostic);
        }
    }

    fn user_event(&mut self, event_loop: &ActiveEventLoop, event: HostEvent) {
        match event {
            HostEvent::Stop => self.request_stop(event_loop),
            HostEvent::RenderFault(diagnostic) => {
                self.record_fatal(diagnostic);
                self.request_stop(event_loop);
            }
            HostEvent::Ended => event_loop.exit(),
        }
    }

    // Runs when the `WaitUntil` deadline passes (and after every other wake); only the stop timer acts.
    #[allow(clippy::disallowed_methods)]
    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        if self.stop_timer.expired(Instant::now()) {
            self.stuck = true;
            event_loop.exit();
        }
    }

    fn window_event(
        &mut self,
        event_loop: &ActiveEventLoop,
        _window_id: WindowId,
        event: WindowEvent,
    ) {
        match event {
            WindowEvent::CloseRequested => self.request_stop(event_loop),
            WindowEvent::Resized(size) => {
                self.surface_size = (size.width, size.height);
                // The render thread reconfigures the swapchain on its next present; nothing to
                // redraw here, since the sim thread — not this one — paints the window.
                if let Some(embedded) = &self.embedded {
                    embedded.resize(size.width, size.height, None);
                }
            }
            WindowEvent::KeyboardInput {
                event,
                is_synthetic: false,
                ..
            } => {
                if let PhysicalKey::Code(key_code) = event.physical_key
                    && let Some(wire) = wire_key_event(key_code, event.state, event.repeat)
                {
                    // Best-effort: if the forwarder has gone, the game just stops receiving keys —
                    // never a reason to tear the window down.
                    let _ = self.input_tx.send(wire.to_value());
                }
            }
            WindowEvent::CursorMoved { position, .. } => {
                if let Some(pos) = self.map_cursor(position.x as f32, position.y as f32) {
                    let _ = self.input_tx.send(mouse::mouse_move_event(pos));
                }
            }
            WindowEvent::MouseInput { state, button, .. } => {
                if let Some(value) = mouse::mouse_button_event(button, state) {
                    let _ = self.input_tx.send(value);
                }
            }
            WindowEvent::MouseWheel { delta, .. } => {
                let _ = self.input_tx.send(mouse::mouse_wheel_event(delta));
            }
            _ => {}
        }
    }
}

/// The forwarder thread: inject every mapped device event (keyboard and mouse from the event loop,
/// pad samples from the gamepad thread — all pre-built `input.inject` event `Value`s sharing one
/// channel) into the core's intake as an ordinary `input.inject` command.
///
/// The [`Injector`] feeds the same queue a wire request does, not a private path: a session played in
/// this window records exactly what a session driven by any other client's `input.inject` records.
/// Ends when every sender is dropped (the event loop torn down).
fn spawn_input_forwarder(
    injector: Injector,
    input_rx: Receiver<serde_json::Value>,
) -> JoinHandle<()> {
    std::thread::spawn(move || {
        while let Ok(event) = input_rx.recv() {
            injector.inject_value(event);
        }
    })
}

/// The waiter thread: forward Ctrl-C to the loop as a stop request, a fatal render fault as a
/// stop with its diagnostic, and the host ending — for any reason: the stop, a wire
/// `engine.shutdown`, a failed boot — as its exit; or end quietly when the loop tore down first
/// (signaled over `internal_stop`).
fn spawn_waiter_thread(
    ctrl_c: Receiver<()>,
    ended: Receiver<()>,
    render_fault: Receiver<Diagnostic>,
    internal_stop: Receiver<()>,
    proxy: EventLoopProxy<HostEvent>,
) -> JoinHandle<()> {
    std::thread::spawn(move || {
        let mut render_fault = render_fault;
        loop {
            crossbeam_channel::select! {
                recv(ctrl_c) -> _ => { let _ = proxy.send_event(HostEvent::Stop); }
                recv(render_fault) -> fault => match fault {
                    Ok(fault) => { let _ = proxy.send_event(HostEvent::RenderFault(fault)); }
                    // The engine dropped its sender; `ended` follows. A disconnected receiver
                    // would fire every pass, so swap it for one that never does.
                    Err(_) => render_fault = crossbeam_channel::never(),
                },
                recv(ended) -> _ => {
                    let _ = proxy.send_event(HostEvent::Ended);
                    return;
                }
                recv(internal_stop) -> _ => return,
            }
        }
    })
}

/// `L2007` — the same code the out-of-process viewer raises when it cannot present, for the same
/// reason from this side of the process boundary.
fn unavailable(detail: String) -> Diagnostic {
    Diagnostic::new(code::WINDOW_PRESENTATION_UNAVAILABLE, detail).with_help(
        "run from an interactive desktop session with working GPU drivers; on a headless host leave \
         `--window` off and attach a viewer over the wire protocol instead",
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[allow(clippy::disallowed_methods)]
    fn the_stop_timer_expires_exactly_at_the_grace_and_the_first_arm_wins() {
        let t0 = Instant::now();
        let mut timer = StopTimer::default();
        assert!(
            !timer.expired(t0 + Duration::from_secs(3600)),
            "unarmed never expires"
        );

        let deadline = timer.arm(t0);
        assert_eq!(deadline, t0 + WINDOW_STOP_GRACE);
        assert!(!timer.expired(deadline - Duration::from_millis(1)));
        assert!(timer.expired(deadline));

        assert_eq!(
            timer.arm(t0 + Duration::from_secs(1)),
            deadline,
            "a second request never extends it"
        );
    }
}
