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
//! Unless an extent is named, the render extent follows the window's size (debounced drag-resizes)
//! as an ordinary `render.size`; the blit letterboxes whatever the window's aspect leaves over.
//!
//! With the `wire` feature the listener binds exactly as it does headless, so an agent, `ludion
//! profile --attach` and the editor attach to a windowed game unchanged; without it the window is the
//! desktop player and nothing listens. [`WindowRole`] picks the one difference between the two hosts,
//! the title. The out-of-process viewer stays `ludion dev
//! --window`'s: that window must outlive a restarting child process, which is a different role, not
//! a second way to do this one.
//!
//! # Threading
//!
//! [`host_windowed`] must run on the process's **main thread** — winit requires the event loop to
//! own it, which is why [`crate::run`] and [`crate::launch`] must be called from `main`. The main thread makes the
//! window and surface and only forwards: device events become `input.inject` commands through the
//! core's intake. The boot thread owns the engine, builds it, attaches the surface, binds (with `wire`)
//! and presents.
//! Three workers sit beside them: `ludion-window`'s `gilrs` poll (shared with the viewer, so both
//! windows produce identical pad events), a forwarder draining the one device-event channel into the
//! core, and a waiter that turns Ctrl-C and a fatal render fault into a stop request and the host
//! ending (a wire `engine.shutdown`, a failed boot, the requested stop) into a loop exit.
//!
//! A stop never leaves the loop by itself: the boot thread drops the engine, and with it the window's
//! surface, and a Metal layer is released on the main thread only — a main thread blocked joining the
//! boot deadlocks it. So a close or Ctrl-C only asks the core to stop, and the loop keeps running
//! until the boot has ended.

use std::path::Path;
use std::process::ExitCode;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use crossbeam_channel::{Receiver, Sender};
use ludion_core::codes::code;
use ludion_core::diagnostics::Diagnostic;
use ludion_engine::CursorMode;
use ludion_engine::live::SIM_STOP_GRACE;
use ludion_window::chrome::{
    self, CursorAutoHide, ExtentDebounce, FullscreenChord, HostPlatform, KeyRoute, dev_title,
    game_title,
};
use ludion_window::keymap::KeyMapper;
use ludion_window::mouse;
use winit::application::ApplicationHandler;
use winit::dpi::LogicalSize;
use winit::event::WindowEvent;
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop, EventLoopProxy};
use winit::keyboard::{ModifiersState, PhysicalKey};
use winit::window::{Fullscreen, Window, WindowId};

use crate::embed::{Embedded, Injector, Source, StartOptions, Surface};
use crate::host;

/// The window's requested initial inner size in logical pixels (points), matching the viewer's — the
/// default stream extent on a 1x display. [`chrome::first_window_size`] shrinks it to the monitor;
/// `surface_size` is always read back from the window in physical pixels.
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

    fn deadline(&self) -> Option<Instant> {
        self.0
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
pub(crate) fn host_windowed(source: Source, options: StartOptions, role: WindowRole) -> ExitCode {
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
    let (title_hint, fullscreen_default, cursor) = match &source {
        Source::Game { builder, .. } => (
            builder.title_hint().map(str::to_owned),
            builder.fullscreen_hint(),
            builder.cursor_hint(),
        ),
        #[cfg(feature = "wire")]
        Source::Built { .. } => (None, false, CursorMode::default()),
    };
    let fullscreen =
        match chrome::resolve_startup_fullscreen(options.fullscreen, fullscreen_default) {
            Ok(fullscreen) => fullscreen,
            Err(diagnostic) => {
                eprintln!("{diagnostic}");
                return ExitCode::FAILURE;
            }
        };
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
        keys: KeyMapper::default(),
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
        role,
        title_hint,
        fullscreen,
        cursor_enabled: cursor == CursorMode::AutoHide,
        platform: HostPlatform::current(),
        modifiers: ModifiersState::empty(),
        chord: FullscreenChord::default(),
        cursor: None,
        extent: ExtentDebounce::default(),
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

/// Which host the window serves; only the title differs.
#[derive(Clone, Copy)]
pub(crate) enum WindowRole {
    /// `run --window` and `host_scene`: the title names the scene file beside the game.
    #[cfg_attr(not(feature = "wire"), allow(dead_code))]
    Dev,
    /// `launch`: the title is the game's alone.
    Player,
}

impl WindowRole {
    fn title(self, game: &str, scene_file: Option<&str>) -> String {
        match self {
            Self::Dev => dev_title(game, scene_file),
            Self::Player => game.to_owned(),
        }
    }
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
    /// Folds left/right modifiers into one engine key across events.
    keys: KeyMapper,
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
    role: WindowRole,
    /// The builder's window title, if the game set one.
    title_hint: Option<String>,
    /// Whether the window opens borderless fullscreen (flag, environment or builder default).
    fullscreen: bool,
    /// Whether an idle cursor hides ([`CursorMode::AutoHide`]).
    cursor_enabled: bool,
    platform: HostPlatform,
    /// The latest modifier state, for the fullscreen chord.
    modifiers: ModifiersState,
    chord: FullscreenChord,
    /// Built with the window.
    cursor: Option<CursorAutoHide>,
    /// Settles drag-resizes before the render extent follows the window.
    extent: ExtentDebounce,
}

/// The window title a game shows: the builder's, else the manifest's `name`, else `Ludion`. The
/// manifest is looked up from the scene's directory; any failure falls back silently, since the
/// title is cosmetic and export/sync report a bad manifest.
pub(crate) fn game_title_for(builder_title: Option<&str>, scene: &Path) -> String {
    let manifest_name = std::path::absolute(scene)
        .ok()
        .and_then(|scene| scene.parent().and_then(ludion_engine::manifest::find))
        .and_then(|root| ludion_engine::manifest::load(&root).ok())
        .map(|manifest| manifest.name);
    game_title(builder_title, manifest_name.as_deref())
}

impl App {
    /// Creates the window and its surface, starts the core with them, and starts the workers — in
    /// that order, because the core's boot thread must take the surface before it builds a renderer.
    // The window host owns the wall clock; the cursor timer never touches the sim.
    #[allow(clippy::disallowed_methods)]
    fn start(&mut self, event_loop: &ActiveEventLoop) -> Result<(), Diagnostic> {
        let (Some(source), Some(mut options)) = (self.source.take(), self.options.take()) else {
            return Ok(()); // a previous `resumed` already started (or failed); never start twice
        };
        options.follow_surface = true;
        let mut attributes = Window::default_attributes()
            .with_title(game_title(self.title_hint.as_deref(), None))
            .with_inner_size(chrome::first_window_size(
                LogicalSize::new(f64::from(WINDOW_WIDTH), f64::from(WINDOW_HEIGHT)),
                chrome::monitor_logical_size(event_loop),
            ));
        if self.fullscreen {
            attributes = attributes.with_fullscreen(Some(Fullscreen::Borderless(None)));
        }
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
        let file = embedded.scene().file_name().map(|n| n.to_string_lossy());
        window.set_title(&self.role.title(
            &game_title_for(self.title_hint.as_deref(), embedded.scene()),
            file.as_deref(),
        ));

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
        self.cursor = Some(CursorAutoHide::new(self.cursor_enabled, Instant::now()));
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
    fn request_stop(&mut self) {
        if let Some(embedded) = &self.embedded {
            embedded.request_stop();
            self.stop_timer.arm(Instant::now());
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
            HostEvent::Stop => self.request_stop(),
            HostEvent::RenderFault(diagnostic) => {
                self.record_fatal(diagnostic);
                self.request_stop();
            }
            HostEvent::Ended => event_loop.exit(),
        }
    }

    // Runs when the `WaitUntil` deadline passes (and after every other wake); the one place control
    // flow is set: the stop timer and the idle-cursor timer share it.
    #[allow(clippy::disallowed_methods)]
    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        let now = Instant::now();
        if self.stop_timer.expired(now) {
            self.stuck = true;
            event_loop.exit();
            return;
        }
        if let Some(cursor) = &mut self.cursor
            && let Some(visible) = cursor.poll(now)
            && let Some(window) = &self.window
        {
            window.set_cursor_visible(visible);
        }
        if let Some((w, h)) = self.extent.poll(now)
            && let Some(embedded) = &self.embedded
        {
            embedded.follow_surface(w, h);
        }
        let next = [
            self.stop_timer.deadline(),
            self.cursor.as_ref().and_then(CursorAutoHide::deadline),
            self.extent.deadline(),
        ]
        .into_iter()
        .flatten()
        .min();
        event_loop.set_control_flow(next.map_or(ControlFlow::Wait, ControlFlow::WaitUntil));
    }

    // The window host owns the wall clock; the cursor timer never touches the sim.
    #[allow(clippy::disallowed_methods)]
    fn window_event(
        &mut self,
        _event_loop: &ActiveEventLoop,
        _window_id: WindowId,
        event: WindowEvent,
    ) {
        match event {
            WindowEvent::CloseRequested => self.request_stop(),
            WindowEvent::Resized(size) => {
                self.surface_size = (size.width, size.height);
                // The render thread reconfigures the swapchain on its next present; nothing to
                // redraw here, since the sim thread — not this one — paints the window.
                // The render extent follows once the drag settles (`about_to_wait`).
                if let Some(embedded) = &self.embedded {
                    embedded.resize(size.width, size.height, None);
                    self.extent
                        .on_resize((size.width, size.height), Instant::now());
                }
            }
            WindowEvent::ModifiersChanged(modifiers) => self.modifiers = modifiers.state(),
            WindowEvent::KeyboardInput {
                event,
                is_synthetic: false,
                ..
            } => {
                if let PhysicalKey::Code(key_code) = event.physical_key {
                    match self.chord.on_key(
                        self.platform,
                        key_code,
                        self.modifiers,
                        event.state,
                        event.repeat,
                    ) {
                        KeyRoute::Toggle => {
                            if let Some(window) = &self.window {
                                let next = if window.fullscreen().is_some() {
                                    None
                                } else {
                                    Some(Fullscreen::Borderless(None))
                                };
                                window.set_fullscreen(next);
                            }
                            return;
                        }
                        KeyRoute::Swallow => return,
                        KeyRoute::Forward => {}
                    }
                }
                if let PhysicalKey::Code(key_code) = event.physical_key
                    && let Some(wire) = self.keys.map(key_code, event.state, event.repeat)
                {
                    // Best-effort: if the forwarder has gone, the game just stops receiving keys —
                    // never a reason to tear the window down.
                    let _ = self.input_tx.send(wire.to_value());
                }
            }
            // A modifier let go while another window had focus never gets a key-up.
            WindowEvent::Focused(false) => {
                for wire in self.keys.release_modifiers() {
                    let _ = self.input_tx.send(wire.to_value());
                }
            }
            WindowEvent::CursorMoved { position, .. } => {
                if let Some(cursor) = &mut self.cursor
                    && let Some(visible) = cursor.on_motion(Instant::now())
                    && let Some(window) = &self.window
                {
                    window.set_cursor_visible(visible);
                }
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
    fn the_player_window_is_titled_with_the_game_alone() {
        assert_eq!(
            WindowRole::Dev.title("Clockout", Some("main.yaml")),
            dev_title("Clockout", Some("main.yaml"))
        );
        assert_eq!(
            WindowRole::Player.title("Clockout", Some("main.yaml")),
            "Clockout"
        );
        assert_eq!(WindowRole::Player.title("Clockout", None), "Clockout");
    }

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
        assert_eq!(timer.deadline(), Some(deadline));
    }
}
