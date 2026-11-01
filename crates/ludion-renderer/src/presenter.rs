//! The presenter: a thread that owns a window's renderer and swapchain and paces itself to the
//! display (`Plan/display-paced-present.md`).
//!
//! The sim thread extracts a [`RenderScene`] once per tick and [`publish`](Presenter::publish)es it as
//! a [`PresentPacket`] into a one-slot mailbox — latest wins, the sim never waits beyond a pointer
//! swap. The presenter thread renders a packet it has not seen, then presents the last output; under
//! `Fifo` the present blocks in swapchain acquire, so the loop runs at the display's refresh rate and
//! a 120 Hz panel re-presents each tick's image without re-rendering it. Faults and the renderer's
//! probe resolve come back as [`PresenterNote`]s the sim thread drains.
//!
//! The thread talks to its renderer and surface through [`PresentBackend`]; [`WindowBackend`] is the
//! real one, tests substitute a fake. Nothing here touches `World`, `Engine` or a command, and nothing
//! it reads from the clock reaches the sim: this file is the renderer crate's one wall-clock site.

use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, PoisonError};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use ludion_core::codes::code;
use ludion_core::diagnostics::Diagnostic;
use ludion_core::time::FrameIndex;
use ludion_render_graph::{FrameSinks, Gpu, PresentTarget, StreamReadback};

use crate::graph::{AntiAliasing, FrameSize};
use crate::probe_capture::ProbeResolve;
use crate::quality::{Quality, QualityParams};
use crate::render::Renderer;
use crate::scene::RenderScene;

/// The longest the loop parks on the mailbox when there is nothing to present (a minimized window,
/// or no packet yet) — short enough to notice a restore, long enough never to spin.
const IDLE_WAIT: Duration = Duration::from_millis(100);

/// Presents between two `LUDION_PRESENT_TRACE` summaries.
const TRACE_INTERVAL: usize = 600;

/// The env var that arms the present trace. Any non-empty value other than `"0"` arms it.
const TRACE_ENV: &str = "LUDION_PRESENT_TRACE";

/// The render-side state the engine pushes into its renderer each frame; the presenter applies it
/// when it differs from the last applied.
#[derive(Clone, Debug, PartialEq)]
pub struct PresentSettings {
    /// The quality preset.
    pub quality: Quality,
    /// The preset's tunable parameters.
    pub params: QualityParams,
    /// The anti-aliasing mode.
    pub anti_aliasing: AntiAliasing,
    /// Whether the baked GI baseline is held.
    pub gi_hold: bool,
    /// The reflection-probe store directory, if any.
    pub probe_store: Option<PathBuf>,
}

/// When a tick ran on the sim thread's wall clock, for the presenter's trace.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TickStamp {
    /// The instant the tick ran.
    pub at: Instant,
    /// The wall-clock length one tick should occupy.
    pub period: Duration,
    /// Whether the live loop re-based its deadline for this tick (a lag, a speed change, a resume).
    pub rebased: bool,
}

/// One tick's worth of render input, handed from the sim thread to the presenter.
#[derive(Clone)]
pub struct PresentPacket {
    /// The tick's frame index.
    pub frame: FrameIndex,
    /// The extracted scene; immutable, shared with any stream render of the same pass.
    pub scene: Arc<RenderScene>,
    /// The render and output extents.
    pub size: FrameSize,
    /// Render-side settings in force for this frame.
    pub settings: PresentSettings,
    /// The tick's wall-clock stamp, when the live loop supplies one.
    pub tick: Option<TickStamp>,
}

/// What the presenter sends back to the sim thread.
#[derive(Debug)]
pub enum PresenterNote {
    /// A permanent fault (`L2007` dead surface, `L2011` lost device); the presenter has stopped presenting.
    Fault(Diagnostic),
    /// The renderer's probe resolve changed.
    Probes(ProbeResolve),
    /// A non-fatal failure to log, deduplicated.
    Log(Diagnostic),
}

/// The renderer and surface behind a [`Presenter`]; the thread owns it for its lifetime.
pub trait PresentBackend: Send {
    /// Renders `packet` into the backend's own output, applying its settings first.
    ///
    /// # Errors
    /// Whatever the render raises (`L2007`/`L2011` are fatal to the presenter; anything else is logged).
    fn render(&mut self, packet: &PresentPacket) -> Result<(), Diagnostic>;
    /// Presents the last rendered output; blocks at the display's rate. A no-op before the first
    /// render.
    ///
    /// # Errors
    /// `L2007` if the surface is permanently unavailable.
    fn present(&mut self) -> Result<(), Diagnostic>;
    /// The renderer's probe resolve so far, if it has one.
    fn probe_resolve(&self) -> Option<ProbeResolve>;
    /// Hot-reloads one shader on the backend's renderer; returns the new shader generation.
    ///
    /// # Errors
    /// `SHADER_RELOAD_INVALID` for an unknown path, or the compile error.
    fn reload_shader(&mut self, path: &str, source: String) -> Result<u64, Diagnostic>;
    /// Whether there is nothing to present into (a 0×0 surface).
    fn idle(&self) -> bool;
}

struct Slot {
    packet: Option<PresentPacket>,
    reloads: Vec<(String, String)>,
    closed: bool,
}

struct Shared {
    slot: Mutex<Slot>,
    wake: Condvar,
}

impl Shared {
    fn lock(&self) -> MutexGuard<'_, Slot> {
        self.slot.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

/// The handle to a running presenter thread. Dropping it closes the mailbox and joins the thread.
pub struct Presenter {
    shared: Arc<Shared>,
    notes: Receiver<PresenterNote>,
    thread: Option<JoinHandle<()>>,
}

impl Presenter {
    /// Starts the thread over `backend`. Reads `LUDION_PRESENT_TRACE` once.
    pub fn spawn(backend: Box<dyn PresentBackend>) -> Presenter {
        let shared = Arc::new(Shared {
            slot: Mutex::new(Slot {
                packet: None,
                reloads: Vec::new(),
                closed: false,
            }),
            wake: Condvar::new(),
        });
        let (notes_tx, notes) = mpsc::channel();
        let trace = PresentTrace::from_env();
        let thread = {
            let shared = Arc::clone(&shared);
            std::thread::Builder::new()
                .name("ludion-presenter".to_owned())
                .spawn(move || run_loop(&shared, backend, &notes_tx, trace))
                .expect("spawning the presenter thread")
        };
        Presenter {
            shared,
            notes,
            thread: Some(thread),
        }
    }

    /// Replaces the mailbox's packet with `packet` and wakes the presenter. Never waits on it.
    pub fn publish(&self, packet: PresentPacket) {
        self.shared.lock().packet = Some(packet);
        self.shared.wake.notify_one();
    }

    /// Queues a shader hot reload for the presenter's renderer; a failure comes back as a
    /// [`PresenterNote::Log`].
    pub fn reload_shader(&self, path: &str, source: String) {
        self.shared.lock().reloads.push((path.to_owned(), source));
        self.shared.wake.notify_one();
    }

    /// Drains the notes the presenter has sent since the last call, oldest first.
    pub fn take_notes(&self) -> Vec<PresenterNote> {
        self.notes.try_iter().collect()
    }
}

impl Drop for Presenter {
    fn drop(&mut self) {
        self.shared.lock().closed = true;
        self.shared.wake.notify_one();
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

/// The diagnostics that end presenting: a dead surface, a rejected GPU, a lost device.
fn is_fatal(diag: &Diagnostic) -> bool {
    diag.code == code::WINDOW_PRESENTATION_UNAVAILABLE || diag.code == code::RENDER_DEVICE_LOST
}

/// The presenter loop — the renderer crate's one wall-clock site, reading `Instant::now` only to
/// time its own acquire+present and feed the opt-in trace; nothing it reads reaches the sim.
#[allow(clippy::disallowed_methods)]
fn run_loop(
    shared: &Shared,
    mut backend: Box<dyn PresentBackend>,
    notes: &Sender<PresenterNote>,
    mut trace: Option<PresentTrace>,
) {
    let mut rendered: Option<FrameIndex> = None;
    let mut dead = false;
    let mut last_probes = ProbeResolve::default();
    let mut last_error: Option<String> = None;
    loop {
        let (packet, reloads) = {
            let mut slot = shared.lock();
            if slot.closed {
                return;
            }
            (slot.packet.take(), std::mem::take(&mut slot.reloads))
        };
        for (path, source) in reloads {
            if let Err(diag) = backend.reload_shader(&path, source) {
                let _ = notes.send(PresenterNote::Log(diag));
            }
        }
        if dead {
            wait(shared, IDLE_WAIT);
            continue;
        }
        if let Some(packet) = packet
            && rendered != Some(packet.frame)
        {
            if let (Some(trace), Some(tick)) = (trace.as_mut(), packet.tick) {
                trace.note_tick(tick);
            }
            match backend.render(&packet) {
                Ok(()) => {
                    rendered = Some(packet.frame);
                    if let Some(probes) = backend.probe_resolve()
                        && probes != last_probes
                    {
                        last_probes = probes.clone();
                        let _ = notes.send(PresenterNote::Probes(probes));
                    }
                }
                Err(diag) => {
                    if report(notes, &mut last_error, diag) {
                        dead = true;
                        continue;
                    }
                }
            }
        }
        if backend.idle() || rendered.is_none() {
            wait(shared, IDLE_WAIT);
            continue;
        }
        let started = Instant::now();
        if let Err(diag) = backend.present() {
            if report(notes, &mut last_error, diag) {
                dead = true;
            }
            continue;
        }
        if let (Some(trace), Some(frame)) = (trace.as_mut(), rendered) {
            trace.note_present(frame, started.elapsed());
        }
    }
}

/// Sends `diag` as a fault or a deduplicated log note; true when it was fatal.
fn report(
    notes: &Sender<PresenterNote>,
    last_error: &mut Option<String>,
    diag: Diagnostic,
) -> bool {
    if is_fatal(&diag) {
        let _ = notes.send(PresenterNote::Fault(diag));
        return true;
    }
    let rendered = diag.to_string();
    if last_error.as_deref() != Some(rendered.as_str()) {
        let _ = notes.send(PresenterNote::Log(diag));
        *last_error = Some(rendered);
    }
    false
}

/// Parks until something is published, a reload is queued, the handle closes, or `timeout` passes.
fn wait(shared: &Shared, timeout: Duration) {
    let slot = shared.lock();
    if slot.closed || slot.packet.is_some() || !slot.reloads.is_empty() {
        return;
    }
    let _ = shared
        .wake
        .wait_timeout(slot, timeout)
        .unwrap_or_else(PoisonError::into_inner);
}

/// The armed `LUDION_PRESENT_TRACE` state: counts over one [`TRACE_INTERVAL`] of presents, printed
/// to stderr. Pure observation, shaped like the live loop's `TickTrace`.
struct PresentTrace {
    presents: usize,
    distinct: usize,
    repeats: usize,
    gaps: u64,
    rebased: usize,
    last_frame: Option<FrameIndex>,
    waits_ms: Vec<f64>,
    window_start: Option<Instant>,
}

impl PresentTrace {
    fn from_env() -> Option<PresentTrace> {
        let armed = std::env::var(TRACE_ENV).is_ok_and(|value| !value.is_empty() && value != "0");
        armed.then(|| PresentTrace {
            presents: 0,
            distinct: 0,
            repeats: 0,
            gaps: 0,
            rebased: 0,
            last_frame: None,
            waits_ms: Vec::with_capacity(TRACE_INTERVAL),
            window_start: None,
        })
    }

    fn note_tick(&mut self, tick: TickStamp) {
        if tick.rebased {
            self.rebased += 1;
        }
    }

    #[allow(clippy::disallowed_methods)]
    fn note_present(&mut self, frame: FrameIndex, wait: Duration) {
        let now = Instant::now();
        let start = *self.window_start.get_or_insert(now - wait);
        self.presents += 1;
        match self.last_frame {
            Some(last) if last == frame => self.repeats += 1,
            Some(last) => {
                self.distinct += 1;
                self.gaps += frame.0.saturating_sub(last.0).saturating_sub(1);
            }
            None => self.distinct += 1,
        }
        self.last_frame = Some(frame);
        self.waits_ms.push(wait.as_secs_f64() * 1000.0);
        if self.presents >= TRACE_INTERVAL {
            self.report(now.duration_since(start));
        }
    }

    fn report(&mut self, elapsed: Duration) {
        self.waits_ms.sort_by(f64::total_cmp);
        let at = |fraction: f64| {
            let index = ((self.waits_ms.len() as f64 * fraction).ceil() as usize).max(1) - 1;
            self.waits_ms[index.min(self.waits_ms.len() - 1)]
        };
        eprintln!(
            "present-trace: {:.1} presents/s, {} distinct frames, {} repeats, {} frame-index gaps, \
             {} rebased ticks, acquire+present p50 {:.2} ms p99 {:.2} ms",
            self.presents as f64 / elapsed.as_secs_f64().max(1e-9),
            self.distinct,
            self.repeats,
            self.gaps,
            self.rebased,
            at(0.50),
            at(0.99),
        );
        self.presents = 0;
        self.distinct = 0;
        self.repeats = 0;
        self.gaps = 0;
        self.rebased = 0;
        self.waits_ms.clear();
        self.window_start = None;
    }
}

/// The real backend: a [`Renderer`], the private ring whose textures hold the rendered output, and
/// the window's [`PresentTarget`].
pub struct WindowBackend {
    renderer: Renderer,
    gpu: Gpu,
    ring: Option<StreamReadback<FrameIndex>>,
    ring_size: Option<FrameSize>,
    target: PresentTarget,
    applied: Option<PresentSettings>,
}

impl WindowBackend {
    /// Builds a backend over `gpu` (the surface's context) and the window's `target`. The renderer
    /// runs on a [`Gpu::share`] of `gpu`; `gpu` itself serves the blit.
    ///
    /// # Errors
    /// Whatever [`Gpu::share`] or [`Renderer::from_gpu`] raises.
    pub fn new(gpu: Gpu, target: PresentTarget) -> Result<WindowBackend, Diagnostic> {
        let renderer = Renderer::from_gpu(gpu.share()?)?;
        Ok(WindowBackend {
            renderer,
            gpu,
            ring: None,
            ring_size: None,
            target,
            applied: None,
        })
    }

    fn apply(&mut self, settings: &PresentSettings) {
        if self.applied.as_ref() == Some(settings) {
            return;
        }
        self.renderer.set_quality(settings.quality, settings.params);
        self.renderer.set_anti_aliasing(settings.anti_aliasing);
        self.renderer.set_gi_hold(settings.gi_hold);
        self.renderer.set_probe_store(settings.probe_store.clone());
        self.applied = Some(settings.clone());
    }
}

impl PresentBackend for WindowBackend {
    fn render(&mut self, packet: &PresentPacket) -> Result<(), Diagnostic> {
        self.apply(&packet.settings);
        if self.ring.is_none() || self.ring_size != Some(packet.size) {
            self.ring = Some(self.renderer.new_stream_readback(packet.size)?);
            self.ring_size = Some(packet.size);
        }
        let ring = self.ring.as_mut().expect("ring built above");
        self.renderer.render_frame(
            &packet.scene,
            packet.size,
            ring,
            packet.frame,
            None,
            FrameSinks {
                readback: false,
                present: None,
            },
        )?;
        Ok(())
    }

    fn present(&mut self) -> Result<(), Diagnostic> {
        match self.ring.as_ref().and_then(StreamReadback::last_output) {
            Some(output) => self.target.present(&self.gpu, output),
            None => Ok(()),
        }
    }

    fn probe_resolve(&self) -> Option<ProbeResolve> {
        Some(self.renderer.probe_resolve())
    }

    fn reload_shader(&mut self, path: &str, source: String) -> Result<u64, Diagnostic> {
        self.renderer.reload_shader(path, source)
    }

    fn idle(&self) -> bool {
        let (width, height) = self.target.requested_size();
        width == 0 || height == 0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Default)]
    struct Log {
        renders: Vec<u64>,
        presents: usize,
    }

    /// Renders are logged; `present` blocks on `gate` until a token arrives (or the sender drops),
    /// standing in for a display that paces the loop.
    struct Fake {
        log: Arc<Mutex<Log>>,
        gate: Receiver<()>,
    }

    impl PresentBackend for Fake {
        fn render(&mut self, packet: &PresentPacket) -> Result<(), Diagnostic> {
            self.log.lock().unwrap().renders.push(packet.frame.0);
            Ok(())
        }
        fn present(&mut self) -> Result<(), Diagnostic> {
            let _ = self.gate.recv();
            self.log.lock().unwrap().presents += 1;
            Ok(())
        }
        fn probe_resolve(&self) -> Option<ProbeResolve> {
            None
        }
        fn reload_shader(&mut self, _: &str, _: String) -> Result<u64, Diagnostic> {
            Ok(0)
        }
        fn idle(&self) -> bool {
            false
        }
    }

    fn packet(frame: u64) -> PresentPacket {
        PresentPacket {
            frame: FrameIndex(frame),
            scene: Arc::new(RenderScene::empty((16, 16))),
            size: FrameSize::native(16, 16),
            settings: PresentSettings {
                quality: Quality::FULL,
                params: QualityParams::MEDIUM,
                anti_aliasing: AntiAliasing::Off,
                gi_hold: false,
                probe_store: None,
            },
            tick: None,
        }
    }

    fn until(what: &str, mut done: impl FnMut() -> bool) {
        for _ in 0..500 {
            if done() {
                return;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        panic!("timed out waiting for {what}");
    }

    #[test]
    fn presenter_types_are_send() {
        fn assert_send<T: Send>() {}
        assert_send::<PresentPacket>();
        assert_send::<PresenterNote>();
        assert_send::<Presenter>();
    }

    /// Packets 1–3 published while the loop is blocked in `present` collapse to the newest, and the
    /// later presents repeat it without rendering again.
    #[test]
    fn the_newest_packet_wins_and_a_repeat_present_does_not_re_render() {
        let log = Arc::new(Mutex::new(Log::default()));
        let (gate_tx, gate) = mpsc::channel();
        let presenter = Presenter::spawn(Box::new(Fake {
            log: Arc::clone(&log),
            gate,
        }));
        presenter.publish(packet(0));
        until("frame 0 rendered", || log.lock().unwrap().renders == [0]);
        for frame in 1..=3 {
            presenter.publish(packet(frame));
        }
        gate_tx.send(()).unwrap();
        until("frame 3 rendered", || log.lock().unwrap().renders == [0, 3]);
        for _ in 0..3 {
            gate_tx.send(()).unwrap();
        }
        until("repeat presents", || log.lock().unwrap().presents >= 4);
        assert_eq!(
            log.lock().unwrap().renders,
            [0, 3],
            "1 and 2 were superseded"
        );
        drop(gate_tx);
        drop(presenter);
    }

    #[test]
    fn a_fatal_render_error_is_a_fault_note_and_presenting_stops() {
        struct Dying(Arc<Mutex<Log>>);
        impl PresentBackend for Dying {
            fn render(&mut self, _: &PresentPacket) -> Result<(), Diagnostic> {
                Err(Diagnostic::new(code::RENDER_DEVICE_LOST, "lost"))
            }
            fn present(&mut self) -> Result<(), Diagnostic> {
                self.0.lock().unwrap().presents += 1;
                Ok(())
            }
            fn probe_resolve(&self) -> Option<ProbeResolve> {
                None
            }
            fn reload_shader(&mut self, _: &str, _: String) -> Result<u64, Diagnostic> {
                Ok(0)
            }
            fn idle(&self) -> bool {
                false
            }
        }
        let log = Arc::new(Mutex::new(Log::default()));
        let presenter = Presenter::spawn(Box::new(Dying(Arc::clone(&log))));
        presenter.publish(packet(0));
        let mut notes = Vec::new();
        until("a fault note", || {
            notes.extend(presenter.take_notes());
            !notes.is_empty()
        });
        assert!(matches!(&notes[0], PresenterNote::Fault(d) if d.code == code::RENDER_DEVICE_LOST));
        assert_eq!(log.lock().unwrap().presents, 0);
    }
}
