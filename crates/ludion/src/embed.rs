//! The embedding core: one lifecycle for every host that is not `main` parsing argv.
//!
//! A native shell (an iOS app, a test, [`launch`](crate::launch)) starts the engine with
//! [`Embedded::start`], feeds it input, tells it about size and suspension, and stops it. `start`
//! does only cheap work on the caller's thread and returns at once; the scene load, the renderer and
//! the sim loop run on a spawned thread, so a shell never blocks its UI on a boot. Commands issued
//! before the boot finishes queue in order and run when it does.
//!
//! The core hosts headless when it has no [`Surface`], and presents each rendered frame into the
//! surface when it has one ([`Engine::attach_surface`](ludion_engine::Engine::attach_surface)). With
//! the `wire` feature and [`StartOptions::listen`] it also binds the wire listener, so an agent
//! attaches exactly as it does to `run --window`. [`launch`](crate::launch) is this core with no
//! surface; [`run`](crate::run) and [`host_scene`](crate::host_scene) (so `ludion serve`) host
//! through it too, windowed or not, and the desktop window is its winit client
//! (`present.rs`). The C ABI a native shell links against is [`ffi`](crate::ffi).

use std::fs::File;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::thread::JoinHandle;
use std::time::Duration;

use crossbeam_channel::{Receiver, Sender};
use ludion_core::codes::code;
use ludion_core::diagnostics::Diagnostic;
use ludion_core::letterbox::HostRect;
#[cfg(feature = "wire")]
use ludion_engine::Engine;
#[cfg(feature = "wire")]
use ludion_engine::live::Activity;
use ludion_engine::live::{
    SIM_STOP_GRACE, ShutdownReceiver, ShutdownSignal, StampedCommand, join_sim, run_live,
    sim_thread_stuck,
};
use ludion_engine::{EngineBuilder, PresentSize, QualitySetting};
use ludion_protocol::command::{InputInjectParams, RenderSizeParams};
use ludion_protocol::{Command, ResponseBody};
use ludion_sim::InputEvent;

use crate::LaunchRoots;
use crate::host;
use crate::touch::{TouchControl, TouchLayout, TouchMapper, TouchPhase};

/// A GPU surface the engine presents into: the caller creates it (a native view must be touched on
/// its own thread), the core attaches it on the boot thread before any renderer exists.
pub struct Surface {
    /// The instance the surface was created from; the renderer is built on the adapter it picks.
    pub instance: wgpu::Instance,
    /// The surface itself.
    pub surface: wgpu::Surface<'static>,
    /// The surface's initial size in physical pixels.
    pub width: u32,
    /// The surface's initial size in physical pixels.
    pub height: u32,
}

/// The host switches [`Embedded::start`] takes beside the builder and the roots. Everything else a
/// packaged host would configure arrives from the environment, as it does for [`run`](crate::run).
#[derive(Default)]
pub struct StartOptions {
    /// Start the engine paused (`sim.pause`) before it attaches a surface or binds a listener.
    pub paused: bool,
    /// Arm a replay recording to this path before the first tick.
    pub record: Option<PathBuf>,
    /// Boot from this snapshot file instead of loading the scene; the scene loads only when the
    /// restore fails. A recording cannot arm on a restored world (`L3047`), so this and `record`
    /// together are refused. Honored for a game source only.
    pub restore: Option<PathBuf>,
    /// The scene-session sidecar `ludion dev` names: `engine.snapshot` writes it, and a `restore`
    /// boot adopts it when its frame matches. Honored for a game source only.
    pub dev_session: Option<PathBuf>,
    /// The host's `--fullscreen` flag: a window host opens borderless fullscreen. Ignored with no
    /// window.
    pub fullscreen: bool,
    /// Bind the wire listener on this `host:port`. `None` hosts with no listener, even in a wire
    /// build.
    #[cfg(feature = "wire")]
    pub listen: Option<String>,
    /// The scene to host, relative to `roots.content`, in place of the builder's default scene.
    pub scene: Option<PathBuf>,
    /// The sim worker count, in place of `LUDION_THREADS`.
    pub threads: Option<usize>,
    /// The render quality (`PRESET|FILE`), in place of `LUDION_QUALITY`.
    pub quality: Option<String>,
    /// Where imported mesh maps are read from; a host that names none gets
    /// [`Cooked`](ludion_engine::TextureSource::Cooked), which falls back to the PNG per map.
    pub textures: Option<ludion_engine::TextureSource>,
    /// Where reflection-probe cubes are read from; a host that names none gets
    /// [`Store`](ludion_engine::ProbeSource::Store), which captures on a miss.
    pub probes: Option<ludion_engine::ProbeSource>,
    /// The quality a packaged build ships with, used when neither `quality` nor `LUDION_QUALITY`
    /// chooses one — so whoever launches the build can still override it.
    pub packaged_quality: Option<String>,
    /// The host's anti-aliasing mode, in place of the builder default. A game's runtime request still
    /// beats it.
    pub anti_aliasing: Option<ludion_engine::AntiAliasing>,
    /// The render extent (`WxH`), in place of `LUDION_RENDER_SIZE`.
    pub render_size: Option<String>,
    /// Honor an `asset.reload` on a renderer `.wgsl` by recreating its pipeline live.
    pub dev_shaders: bool,
    /// Print the `schema hash: …` header line beside the listener's address.
    #[cfg(feature = "wire")]
    pub print_schema_hash: bool,
    /// The host's command line has a `--window` flag: the headless banner suggests it.
    #[cfg(feature = "wire")]
    pub window_flag: bool,
    /// The host's safe area inside the surface, in physical pixels: the present fits the frame into
    /// it, and unless `render_size` or `LUDION_RENDER_SIZE` names an extent the render extent follows
    /// its aspect ([`safe_render_extent`]). `None` is the whole surface and no derived extent.
    pub safe_rect: Option<HostRect>,
    /// The render extent follows the surface's physical size ([`window_render_extent`]) unless an
    /// extent is named; a desktop window host sets it. Ignored when `safe_rect` is given, and ignored
    /// when `record` is set (a recording keeps the default extent, letterboxed).
    pub follow_surface: bool,
    /// The surface's initial size in physical pixels when the host has no [`Surface`] to carry it;
    /// ignored when a surface is given.
    pub size: Option<(u32, u32)>,
    /// The process argv. When it names one of [`run`](crate::run)'s modes that are not hosting
    /// (`--replay-verify`, `--check-scene`, …), the boot thread runs that mode against the roots in
    /// place of hosting and prints its verdict; anything else is ignored and the engine hosts. Argv
    /// selects modes only — the host's shape stays the fields above. Honored for a game source only.
    #[cfg(feature = "wire")]
    pub argv: Option<Vec<String>>,
}

impl StartOptions {
    /// The mesh-map source a hosted engine runs with: the named one, else `Cooked`.
    fn live_textures(&self) -> ludion_engine::TextureSource {
        self.textures
            .unwrap_or(ludion_engine::TextureSource::Cooked)
    }

    /// The probe source a hosted engine runs with: the named one, else `Store`.
    fn live_probes(&self) -> ludion_engine::ProbeSource {
        self.probes.unwrap_or(ludion_engine::ProbeSource::Store)
    }
}

/// The render extent a host's safe area asks for: 720 high at the safe area's aspect, never narrower
/// than 16:9 and never wider than the widest extent a host may ask for. A zero dimension gives 16:9.
/// The extent is host state, never hashed.
pub fn safe_render_extent(safe_w: u32, safe_h: u32) -> (u32, u32) {
    const HEIGHT: u32 = 720;
    if safe_w == 0 || safe_h == 0 {
        return (1280, HEIGHT);
    }
    follow_extent(safe_w, safe_h, HEIGHT)
}

/// The render extent a desktop window asks for: the window's own height (at most
/// [`MAX_RENDER_HEIGHT`](ludion_engine::MAX_RENDER_HEIGHT)) at the window's aspect, never narrower
/// than 16:9 and never wider than the widest extent a host may ask for. A window of another aspect
/// than the extent is letterboxed by the blit: a taller-than-16:9 window gets bars above and below,
/// and so does one clamped by either cap. `None` for a zero dimension (a minimized window), which
/// leaves the extent alone. The extent is host state, never hashed.
pub fn window_render_extent(w: u32, h: u32) -> Option<(u32, u32)> {
    if w == 0 || h == 0 {
        return None;
    }
    Some(follow_extent(w, h, h.min(ludion_engine::MAX_RENDER_HEIGHT)))
}

/// `height` high at the aspect `w`:`h`, its width clamped to `[16:9, MAX_RENDER_WIDTH]`. The caller
/// guarantees non-zero arguments.
fn follow_extent(w: u32, h: u32, height: u32) -> (u32, u32) {
    // Rounded to nearest, in integers: the product fits u64 for any u32 pair.
    let scaled = |num: u32, den: u32| {
        let width = (u64::from(height) * u64::from(num) + u64::from(den) / 2) / u64::from(den);
        u32::try_from(width).unwrap_or(u32::MAX)
    };
    let floor = scaled(16, 9);
    (
        scaled(w, h).clamp(floor, ludion_engine::MAX_RENDER_WIDTH.max(floor)),
        height,
    )
}

/// Where the engine comes from: a game the boot thread builds from its builder (a packaged host, and
/// `run`), or an engine the caller already built and prepared — `ludion serve`, which hosts an
/// arbitrary built-in scene through no game builder.
// Each is built once and moved onto the boot thread whole; boxing would only add indirection.
#[allow(clippy::large_enum_variant)]
pub(crate) enum Source {
    Game {
        builder: EngineBuilder,
        roots: LaunchRoots,
    },
    /// A fully prepared engine, its recording armed by the caller; `record_path` feeds only the
    /// header, so [`StartOptions::record`] must be `None`.
    #[cfg(feature = "wire")]
    Built {
        engine: Engine,
        scene: PathBuf,
        record_path: Option<PathBuf>,
    },
}

/// Sends an input event into the core's intake from any thread — [`Embedded::inject`]'s handle, for a
/// shell whose device-event forwarder outlives borrowing the core.
#[derive(Clone)]
pub(crate) struct Injector(Sender<StampedCommand>);

impl Injector {
    /// Fire-and-forget `input.inject` of an already-serialized event.
    #[cfg_attr(not(feature = "window"), allow(dead_code))]
    pub(crate) fn inject_value(&self, event: serde_json::Value) {
        let (stamped, _reply) = StampedCommand::new(Command::InputInject(InputInjectParams {
            frame: None,
            event,
        }));
        let _ = self.0.send(stamped);
    }
}

/// What a derived render extent follows.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Follow {
    /// The host's safe area ([`safe_render_extent`]), re-derived by `resize`.
    SafeArea,
    /// The surface's physical size ([`window_render_extent`]), re-derived by `follow_surface`.
    Surface,
}

/// The surface's size and safe rect, kept as host state so a resize that arrives before the boot has
/// attached the surface is not lost. Never part of the world.
struct ResizeSlot {
    size: (u32, u32),
    safe: Option<HostRect>,
    /// The render extent a host rule chose, and which rule, when the host lets it (no explicit
    /// extent); `None` leaves the extent alone on every resize.
    derived: Option<(Follow, (u32, u32))>,
    present: Option<PresentSize>,
}

type SharedResize = Arc<Mutex<ResizeSlot>>;

fn lock(slot: &SharedResize) -> std::sync::MutexGuard<'_, ResizeSlot> {
    slot.lock().unwrap_or_else(PoisonError::into_inner)
}

/// How [`Embedded::stop`] reaches the boot thread: the sim loop's own signal when it runs on the boot
/// thread, or the listener loop's stop channel.
enum Stopper {
    Sim(ShutdownSignal),
    #[cfg(feature = "wire")]
    Listener(Sender<()>),
}

/// The receiving half of a [`Stopper`], owned by the boot thread.
enum StopWait {
    Sim(ShutdownReceiver),
    #[cfg(feature = "wire")]
    Listener(Receiver<()>),
}

/// What a touch did, for the host that forwarded it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TouchOutcome {
    /// The touch went through the layout (it may have hit no control).
    Mapped,
    /// The core was suspended: this touch resumed it and is swallowed. The host hides its
    /// tap-to-resume view.
    Resumed,
    /// The game has no touch layout and the core is running: nothing to map.
    NoLayout,
}

/// The touch mapper and whether a suspended core waits for a touch to resume it.
struct TouchHost {
    mapper: Option<TouchMapper>,
    awaiting_resume: bool,
}

/// How long [`Embedded::suspend`] and [`Embedded::resume`] wait for the sim thread. Above a
/// backgrounded `nextDrawable` timeout (~1 s), well under iOS's ~5 s background budget.
const SUSPEND_GRACE: Duration = Duration::from_secs(2);

/// A running embedded engine. Dropping it without [`stop`](Self::stop) still ends the host — the
/// intake closes — but does not wait for it or report how it ended.
pub struct Embedded {
    intake: Sender<StampedCommand>,
    stopper: Stopper,
    resize: SharedResize,
    /// Locked before `resize` when both are held.
    touch: Mutex<TouchHost>,
    ended: Receiver<()>,
    render_fault: Receiver<Diagnostic>,
    #[cfg_attr(not(feature = "window"), allow(dead_code))]
    cursor_lock: Receiver<bool>,
    /// Set by the boot thread once it serves its intake; until then a transition is queued, not awaited.
    running: Arc<AtomicBool>,
    scene: PathBuf,
    boot: JoinHandle<bool>,
}

// The builder moves to the boot thread; a non-`Send` registration or system would break here.
const _: fn() = || {
    fn is_send<T: Send>() {}
    is_send::<EngineBuilder>();
};

/// Everything the boot thread needs, moved onto it whole.
struct Boot {
    prep: Prep,
    scene: PathBuf,
    /// The recording path the header prints: the option's, or a built engine's own.
    record_path: Option<PathBuf>,
    surface: Option<Surface>,
    options: StartOptions,
    intake: Receiver<StampedCommand>,
    stop: StopWait,
    resize: SharedResize,
    running: Arc<AtomicBool>,
    /// The engine's channel for a fatal render fault, armed in `run_boot`.
    render_fault: Sender<Diagnostic>,
    /// The engine's channel for pointer-lock changes, armed in `run_boot`.
    cursor_lock: Sender<bool>,
}

/// How the boot thread gets its engine.
#[allow(clippy::large_enum_variant)]
enum Prep {
    Game {
        builder: EngineBuilder,
        quality: QualitySetting,
        render_size: Option<(u32, u32)>,
        /// For an argv mode: the roots its paths resolve against.
        #[cfg(feature = "wire")]
        roots: LaunchRoots,
    },
    #[cfg(feature = "wire")]
    Ready(Engine),
}

impl Embedded {
    /// Starts the engine and returns at once; the scene load and the renderer run on a spawned
    /// thread. `roots` is where content is read from and the profile is written — the same two
    /// directories [`launch`](crate::launch) takes; the scene is the builder's `default_scene`
    /// resolved against `roots.content`.
    ///
    /// # Errors
    /// `L0014`/`L0016`/`L0017` for a malformed `LUDION_THREADS` / `LUDION_QUALITY` /
    /// `LUDION_RENDER_SIZE`, `L0019` for a builder with no default scene, `L0038` for a default
    /// scene that differs from the content root's `ludion.toml` (`L0023` when that does not load),
    /// and `L3047` for `restore` together with `record` — all before anything is built. A failure
    /// *after* this returns (a bad scene, no GPU adapter, a taken port) prints its diagnostic to
    /// stderr and ends the host: [`ended`](Self::ended) disconnects and [`stop`](Self::stop)
    /// reports failure.
    pub fn start(
        builder: EngineBuilder,
        roots: LaunchRoots,
        surface: Option<Surface>,
        options: StartOptions,
    ) -> Result<Embedded, Diagnostic> {
        Self::start_from(Source::Game { builder, roots }, surface, options)
    }

    /// [`start`](Self::start) over either source of an engine.
    ///
    /// # Errors
    /// As [`start`](Self::start); a [`Source::Built`] engine has already been through its own checks.
    pub(crate) fn start_from(
        source: Source,
        surface: Option<Surface>,
        options: StartOptions,
    ) -> Result<Embedded, Diagnostic> {
        let start_size = surface
            .as_ref()
            .map(|s| (s.width, s.height))
            .or(options.size)
            .unwrap_or((0, 0));
        let (prep, scene, record_path, derived, layout) = match source {
            Source::Game { builder, roots } => {
                // A replay starts from the scene's initial state, which a restored world is not.
                if options.restore.is_some() && options.record.is_some() {
                    return Err(Diagnostic::new(
                        code::RECORDING_ARM_REFUSED,
                        "a recording cannot arm on a restored world: `restore` and `record` were both set",
                    )
                    .with_help("record a session from its scene, or restore without recording"));
                }
                // Resolved before anything is built, so a malformed setting refuses to start rather
                // than failing partway into a boot. A packaged host passes none of the overrides, so
                // the environment alone decides.
                ludion_core::par::set_startup_threads(options.threads)
                    .map_err(|invalid| invalid.diagnostic())?;
                let quality = ludion_engine::resolve_startup_quality(
                    options.quality.as_deref(),
                    options.packaged_quality.as_deref(),
                )?;
                let explicit_size =
                    ludion_engine::resolve_startup_render_size(options.render_size.as_deref())?;
                // A named extent (option, environment or the builder's own call) wins; otherwise a
                // host's safe area, or a following window, chooses one.
                let named = explicit_size.or(builder.render_size_hint());
                let derived = match (named, options.safe_rect) {
                    (None, Some(safe)) => Some((
                        Follow::SafeArea,
                        safe_render_extent(safe.width, safe.height),
                    )),
                    // A recording never silently departs from the default extent: `record` pins it.
                    (None, None) if options.follow_surface && options.record.is_none() => {
                        window_render_extent(start_size.0, start_size.1)
                            .map(|extent| (Follow::Surface, extent))
                    }
                    _ => None,
                };
                // The builder's own extent applies at build.
                let render_size = explicit_size.or(derived.map(|(_, extent)| extent));

                crate::host::check_default_scene(builder.default_scene_hint(), &roots.content)?;
                // The named scene, else the builder's own default, resolved against the content root:
                // a builder with neither cannot be hosted.
                let scene = match options
                    .scene
                    .as_deref()
                    .or_else(|| builder.default_scene_hint())
                {
                    Some(hint) => roots.content.join(hint),
                    None => return Err(no_default_scene()),
                };
                // Read before anything is built: a malformed layout refuses the start.
                let layout = TouchLayout::load_beside(&scene)?;

                // The writable half. Content may sit in a read-only bundle, so the profile persists
                // under its own root instead of beside the scene.
                #[cfg(feature = "wire")]
                let argv_roots = roots.clone();
                let builder = builder.save_root(&roots.save);
                let prep = Prep::Game {
                    builder,
                    quality,
                    render_size,
                    #[cfg(feature = "wire")]
                    roots: argv_roots,
                };
                (prep, scene, options.record.clone(), derived, layout)
            }
            #[cfg(feature = "wire")]
            Source::Built {
                engine,
                scene,
                record_path,
            } => {
                debug_assert!(
                    options.record.is_none(),
                    "a built engine arms its own recording"
                );
                (Prep::Ready(engine), scene, record_path, None, None)
            }
        };

        let (intake_tx, intake_rx) = crossbeam_channel::unbounded();
        let (stopper, stop) = stop_pair(&options);
        let resize = Arc::new(Mutex::new(ResizeSlot {
            size: start_size,
            safe: options.safe_rect,
            derived,
            present: None,
        }));
        // Held by the boot thread for its whole life: it disconnects when the host ends on its own.
        let (ended_tx, ended) = crossbeam_channel::bounded::<()>(0);

        let (fault_tx, render_fault) = crossbeam_channel::bounded::<Diagnostic>(1);
        let (lock_tx, cursor_lock) = crossbeam_channel::unbounded::<bool>();

        let running = Arc::new(AtomicBool::new(false));
        let boot = Boot {
            prep,
            scene: scene.clone(),
            record_path,
            surface,
            options,
            intake: intake_rx,
            stop,
            resize: resize.clone(),
            running: running.clone(),
            render_fault: fault_tx,
            cursor_lock: lock_tx,
        };
        // The scene load used to run on `main`'s stack; keep its headroom on the boot thread.
        let handle = std::thread::Builder::new()
            .name("ludion-sim".into())
            .stack_size(8 << 20)
            .spawn(move || {
                let _ended = ended_tx;
                run_boot(boot)
            })
            .map_err(|error| {
                Diagnostic::new(
                    code::PROTOCOL_ENGINE_UNAVAILABLE,
                    format!("could not spawn the sim thread: {error}"),
                )
            })?;

        Ok(Embedded {
            intake: intake_tx,
            stopper,
            resize,
            touch: Mutex::new(TouchHost {
                mapper: layout.map(TouchMapper::new),
                awaiting_resume: false,
            }),
            ended,
            render_fault,
            cursor_lock,
            running,
            scene,
            boot: handle,
        })
    }

    /// The scene the host loads, resolved against the content root.
    pub fn scene(&self) -> &Path {
        &self.scene
    }

    /// Queues one input event for the next tick, exactly as a wire client's `input.inject` would —
    /// a session driven through here records what any other client's does. This is the seam a native
    /// shell's touch mapper feeds with the pad events it emits. Fire-and-forget: an event for a host
    /// that has ended is dropped.
    pub fn inject(&self, event: InputEvent) {
        let event = serde_json::to_value(event).expect("an input event serializes");
        self.injector().inject_value(event);
    }

    /// A cloneable handle onto [`inject`](Self::inject)'s intake for a thread that outlives a borrow.
    #[cfg_attr(not(feature = "window"), allow(dead_code))]
    pub(crate) fn injector(&self) -> Injector {
        Injector(self.intake.clone())
    }

    /// Where the engine reports a fatal render fault (a lost device `L2011`, a dead surface
    /// `L2007`): holds at most one diagnostic at a time; the sender drops with the engine.
    pub(crate) fn render_fault(&self) -> Receiver<Diagnostic> {
        self.render_fault.clone()
    }

    /// Where the engine reports each change of the game's pointer-lock request (`true` locks,
    /// `false` releases); the sender drops with the engine.
    #[cfg_attr(not(feature = "window"), allow(dead_code))]
    pub(crate) fn cursor_lock_requests(&self) -> Receiver<bool> {
        self.cursor_lock.clone()
    }

    /// The surface's present handle, once the boot has attached it: the frame size the last present
    /// drew, for mapping a cursor through the blit's own geometry.
    #[cfg_attr(not(feature = "window"), allow(dead_code))]
    pub(crate) fn present_size(&self) -> Option<PresentSize> {
        lock(&self.resize).present.clone()
    }

    /// Tells the host the surface's new size and safe rect (`None`: the whole surface), in physical
    /// pixels. Before the surface is attached (or with none) it only records them; the boot applies
    /// the latest on attaching. When the safe rect chose the render extent, a rect of another aspect
    /// moves it with a `render.size` through the intake, as any client's would.
    pub fn resize(&self, width: u32, height: u32, safe: Option<HostRect>) {
        let mut slot = lock(&self.resize);
        slot.size = (width, height);
        slot.safe = safe;
        if let Some(present) = &slot.present {
            present.set(width, height);
            present.set_rect(safe);
        }
        if !matches!(slot.derived, Some((Follow::SafeArea, _))) {
            return;
        }
        let Some(safe) = safe else { return };
        self.move_derived_extent(
            &mut slot,
            Follow::SafeArea,
            safe_render_extent(safe.width, safe.height),
        );
    }

    /// Moves a render extent that follows the surface to the surface's physical size `w`×`h`
    /// ([`window_render_extent`]) with a `render.size` through the intake, as any client's would. A
    /// zero size (a minimized window) and an extent that was named or pinned leave it alone. The
    /// host debounces a drag before calling this.
    #[cfg_attr(not(feature = "window"), allow(dead_code))]
    pub(crate) fn follow_surface(&self, w: u32, h: u32) {
        let Some(wanted) = window_render_extent(w, h) else {
            return;
        };
        let mut slot = lock(&self.resize);
        self.move_derived_extent(&mut slot, Follow::Surface, wanted);
    }

    /// Sends `render.size` for `wanted` when `rule` derived the extent and it differs.
    fn move_derived_extent(&self, slot: &mut ResizeSlot, rule: Follow, wanted: (u32, u32)) {
        let Some((current_rule, current)) = slot.derived else {
            return;
        };
        if current_rule != rule || current == wanted {
            return;
        }
        slot.derived = Some((rule, wanted));
        let (stamped, _reply) = StampedCommand::new(Command::RenderSize(RenderSizeParams {
            width: Some(wanted.0),
            height: Some(wanted.1),
        }));
        let _ = self.intake.send(stamped);
    }

    /// Pauses the sim, blocking until the sim thread confirms. A paused engine advances no frame, so
    /// nothing is presented or submitted — the state a backgrounded app must be in. While the boot is
    /// still loading (a scene, an argv mode) nothing answers, so the pause is queued and this returns
    /// at once: the intake is ordered, so the engine comes up paused.
    ///
    /// # Errors
    /// `L3009` if the host has ended (or failed to boot) instead of answering. `L3023` if the sim
    /// thread did not confirm within 2 s; the pause stays queued and applies at the sim's next pass
    /// boundary.
    pub fn suspend(&self) -> Result<(), Diagnostic> {
        // Held controls return to rest first, so a finger down at suspension leaves nothing stuck.
        {
            let mut touch = self.touch_host();
            if let Some(mapper) = &mut touch.mapper {
                for event in mapper.release_all() {
                    self.inject(event);
                }
            }
            touch.awaiting_resume = true;
        }
        self.request_settled(Command::SimPause)
    }

    /// Resumes the sim, blocking until the sim thread confirms — or, while the boot is still
    /// loading, queueing the resume and returning at once, as [`suspend`](Self::suspend) does.
    ///
    /// # Errors
    /// `L3009` if the host has ended. `L3023` if the sim thread did not confirm within 2 s; the
    /// resume stays queued.
    pub fn resume(&self) -> Result<(), Diagnostic> {
        self.touch_host().awaiting_resume = false;
        self.request_settled(Command::SimResume)
    }

    /// Feeds one touch (`x`/`y` in surface pixels, the space of [`resize`](Self::resize)'s safe rect)
    /// through the game's touch layout, injecting the pad events it maps to.
    ///
    /// After [`suspend`](Self::suspend) the first touch to begin resumes the core instead and is
    /// swallowed, so the tap that wakes the game never presses a control — [`TouchOutcome::Resumed`],
    /// even for a game with no layout.
    pub fn touch(&self, id: u64, phase: TouchPhase, x: f32, y: f32) -> TouchOutcome {
        let mut touch = self.touch_host();
        if touch.awaiting_resume && phase == TouchPhase::Began {
            touch.awaiting_resume = false;
            if let Some(mapper) = &mut touch.mapper {
                for event in mapper.swallow(id) {
                    self.inject(event);
                }
            }
            let _ = self.request_settled(Command::SimResume);
            return TouchOutcome::Resumed;
        }
        let Some(mapper) = &mut touch.mapper else {
            return TouchOutcome::NoLayout;
        };
        let safe = self.safe_rect();
        for event in mapper.touch(id, phase, x, y, safe) {
            self.inject(event);
        }
        TouchOutcome::Mapped
    }

    /// The touch controls to draw, resolved against the current safe rect; empty for a game with no
    /// layout.
    pub fn touch_controls(&self) -> Vec<TouchControl> {
        let touch = self.touch_host();
        let safe = self.safe_rect();
        touch
            .mapper
            .as_ref()
            .map_or_else(Vec::new, |mapper| mapper.controls(safe))
    }

    fn touch_host(&self) -> std::sync::MutexGuard<'_, TouchHost> {
        self.touch.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// The safe rect, or the whole surface when the host gave none.
    fn safe_rect(&self) -> HostRect {
        let slot = lock(&self.resize);
        slot.safe.unwrap_or(HostRect {
            x: 0,
            y: 0,
            width: slot.size.0,
            height: slot.size.1,
        })
    }

    /// A receiver that disconnects when the host ends on its own — a failed boot, an in-process or
    /// wire `engine.shutdown`, a panic. Nothing is ever sent on it.
    pub fn ended(&self) -> Receiver<()> {
        self.ended.clone()
    }

    /// Stops the host and waits for it: the sim loop's last act flushes an armed recording, so the
    /// `.lrp` is whole when this returns. [`ExitCode::FAILURE`] if the boot failed, the recording could
    /// not be ended (a full disk, say), or the sim thread panicked or did not stop within [`SIM_STOP_GRACE`] (`L4022`, the thread left detached).
    pub fn stop(self) -> ExitCode {
        self.request_stop();
        // Dropping the intake ends a loop that missed the signal.
        drop(self.intake);
        let joined = match self.stopper {
            // The boot thread is the sim loop: a wedged sim must not hang the caller.
            Stopper::Sim(signal) => match join_sim(self.boot, SIM_STOP_GRACE, &signal) {
                Ok(joined) => joined,
                Err(stuck) => {
                    eprintln!("{stuck}");
                    return ExitCode::FAILURE;
                }
            },
            // The boot thread ends in `host_listener`, whose server teardown is bounded itself.
            #[cfg(feature = "wire")]
            Stopper::Listener(_) => self.boot.join(),
        };
        match joined {
            Ok(true) => ExitCode::SUCCESS,
            Ok(false) => ExitCode::FAILURE,
            Err(payload) => {
                eprintln!(
                    "{}",
                    ludion_engine::live::sim_thread_panicked(payload.as_ref())
                );
                ExitCode::FAILURE
            }
        }
    }

    /// Gives up on a boot thread that did not end `waited` after a stop was asked for: reports `L4022`
    /// with what the sim says it is doing, drops the handle without joining (the thread stays
    /// detached) and returns [`ExitCode::FAILURE`]. The audio sink lives on the boot thread, so
    /// nothing on the calling thread needs releasing first.
    #[cfg_attr(not(feature = "window"), allow(dead_code))]
    pub(crate) fn abandon(self, waited: Duration) -> ExitCode {
        let activity = match &self.stopper {
            Stopper::Sim(signal) => signal.activity(),
            #[cfg(feature = "wire")]
            Stopper::Listener(_) => Activity::Stopping,
        };
        eprintln!("{}", sim_thread_stuck(activity, waited));
        ExitCode::FAILURE
    }

    /// Asks the boot thread to wind down without waiting for it. [`stop`](Self::stop) joins it; a host
    /// whose main thread the boot needs while it unwinds waits on [`ended`](Self::ended) first.
    #[cfg_attr(not(feature = "window"), allow(dead_code))]
    pub(crate) fn request_stop(&self) {
        match &self.stopper {
            Stopper::Sim(signal) => signal.signal(),
            #[cfg(feature = "wire")]
            Stopper::Listener(stop) => {
                let _ = stop.try_send(());
            }
        }
    }

    /// [`request_within`](Self::request_within) once the boot serves its intake; before that, queues
    /// the command and returns without waiting, so a UI thread never parks behind a scene load. A boot
    /// that has already ended still fails.
    fn request_settled(&self, command: Command) -> Result<(), Diagnostic> {
        if self.running.load(Ordering::Acquire) {
            return self.request_within(command, SUSPEND_GRACE).map(drop);
        }
        let (stamped, _reply) = StampedCommand::new(command);
        self.intake.send(stamped).map_err(|_| host_gone())
    }

    /// Runs one command through the sim thread, giving up after `grace` with `L3023`. The command stays
    /// queued and applies when the sim next reaches a pass boundary.
    fn request_within(
        &self,
        command: Command,
        grace: Duration,
    ) -> Result<ResponseBody, Diagnostic> {
        let method = command.method();
        let (stamped, reply) = StampedCommand::new(command);
        self.intake.send(stamped).map_err(|_| host_gone())?;
        match reply.recv_timeout(grace) {
            Ok(result) => result.map(|dispatched| dispatched.body),
            Err(crossbeam_channel::RecvTimeoutError::Disconnected) => Err(host_gone()),
            Err(crossbeam_channel::RecvTimeoutError::Timeout) => Err(Diagnostic::new(
                code::PROTOCOL_SIM_STALLED,
                format!(
                    "the sim thread did not confirm `{method}` within {} s",
                    grace.as_secs()
                ),
            )
            .with_help(
                "the transition stays queued and applies at the sim's next pass boundary; a sim \
                 that never answers is wedged (see L3023)",
            )),
        }
    }

    /// Runs one command through the sim thread and returns its answer, waiting without bound: a
    /// test's long `sim.step` is the reason. Production waits go through
    /// [`request_within`](Self::request_within).
    #[cfg(test)]
    pub(crate) fn request(&self, command: Command) -> Result<ResponseBody, Diagnostic> {
        let (stamped, reply) = StampedCommand::new(command);
        self.intake.send(stamped).map_err(|_| host_gone())?;
        match reply.recv() {
            Ok(result) => result.map(|dispatched| dispatched.body),
            // The reply sender dropped unanswered: the host ended with the command still queued.
            Err(_) => Err(host_gone()),
        }
    }
}

/// The stop channel matching the host shape: the listener loop selects over a plain channel, the sim
/// loop takes its own signal.
fn stop_pair(options: &StartOptions) -> (Stopper, StopWait) {
    #[cfg(feature = "wire")]
    if options.listen.is_some() {
        let (tx, rx) = crossbeam_channel::bounded(1);
        return (Stopper::Listener(tx), StopWait::Listener(rx));
    }
    let _ = options;
    let (signal, receiver) = ludion_engine::live::shutdown_channel();
    (Stopper::Sim(signal), StopWait::Sim(receiver))
}

/// Applies the host-level render settings `options` names to the freshly built `engine`. An unset
/// field leaves the builder's default alone.
fn apply_host_settings(engine: &mut ludion_engine::Engine, options: &StartOptions) {
    if let Some(mode) = options.anti_aliasing {
        engine.set_anti_aliasing(mode);
    }
}

/// The boot thread's whole life: build the engine, take the surface, then run the loop until stop.
/// Prints its own diagnostics; `false` is a failed boot.
fn run_boot(boot: Boot) -> bool {
    let Boot {
        prep,
        scene,
        record_path,
        surface,
        options,
        intake,
        stop,
        resize,
        running,
        render_fault,
        cursor_lock,
    } = boot;

    let mut engine = match prep {
        Prep::Game {
            builder,
            quality,
            render_size,
            #[cfg(feature = "wire")]
            roots,
        } => {
            // A named mode (`--replay-verify`, …) runs here in place of hosting, and its verdict is
            // its own to print.
            #[cfg(feature = "wire")]
            let builder = match options.argv.as_deref().and_then(parse_run_argv) {
                Some(args) => match crate::run_short_circuit(builder, &args, &roots) {
                    Ok(code) => return code == ExitCode::SUCCESS,
                    Err((builder, _scene)) => builder,
                },
                None => builder,
            };
            let Ok(mut engine) = crate::prepare_game_engine(
                builder,
                &scene,
                options.restore.as_deref(),
                options.dev_session.as_deref(),
                quality,
                options.live_textures(),
                options.live_probes(),
                render_size,
            ) else {
                return false;
            };
            apply_host_settings(&mut engine, &options);
            // Dev-only shader hot reload: `ludion dev` forwards `--dev-shaders` so its WGSL fast path
            // can round-trip a shader edit over the wire.
            if options.dev_shaders {
                engine.enable_shader_reload();
            }
            // Arm recording before the engine moves into its loop — a host-side switch, not a
            // protocol command — so the session header captures this run's scene reference and seed.
            if let Some(record_path) = &options.record {
                let sink = match File::create(record_path) {
                    Ok(sink) => sink,
                    Err(error) => {
                        return fail(
                            Diagnostic::new(
                                code::OUTPUT_IO,
                                format!(
                                    "could not create replay session `{}`: {error}",
                                    record_path.display()
                                ),
                            )
                            .with_help(
                                "check that the destination directory exists and is writable",
                            ),
                        );
                    }
                };
                if let Err(diagnostic) = engine.set_recording(sink) {
                    return fail(diagnostic);
                }
            }
            engine
        }
        #[cfg(feature = "wire")]
        Prep::Ready(engine) => engine,
    };

    if options.paused
        && let Err(diagnostic) = engine.execute(Command::SimPause)
    {
        return fail(diagnostic);
    }

    engine.arm_render_fault_notify(render_fault);
    engine.set_cursor_lock_sender(cursor_lock);

    // Before any renderer exists, and before a listener moves the engine onto its own sim thread.
    let attached = surface.is_some();
    // The surface borrows from its instance for as long as the host runs: kept here, it drops after
    // the engine that owns the surface.
    let mut _instance = None;
    if let Some(Surface {
        instance,
        surface,
        width,
        height,
    }) = surface
    {
        let present = match engine.attach_surface(&instance, surface, width, height) {
            Ok(present) => present,
            Err(diagnostic) => return fail(diagnostic),
        };
        let mut slot = lock(&resize);
        if slot.size != (width, height) {
            present.set(slot.size.0, slot.size.1);
        }
        present.set_rect(slot.safe);
        slot.present = Some(present);
        _instance = Some(instance);
    }

    let project_root = engine.project_root().to_path_buf();
    let caps = match host::voice_caps(&project_root) {
        Ok(caps) => caps,
        Err(diagnostic) => return fail(diagnostic),
    };
    let audio = host::wire_audio(&mut engine, project_root.clone(), caps);

    // From here the boot only serves its intake: transitions are answered, not queued.
    running.store(true, Ordering::Release);
    // A listener host is the one that took a `Listener` stop channel; it forwards the core's intake
    // into the server it binds. Every other host runs the sim loop right here.
    #[cfg(feature = "wire")]
    let shutdown = match stop {
        StopWait::Listener(stop) => {
            let addr = options
                .listen
                .as_deref()
                .expect("a listener host names its address");
            let ended_well = host_listener(
                engine,
                addr,
                host::HostOptions {
                    scene: &scene,
                    record_path: record_path.as_deref(),
                    paused: options.paused,
                    window: attached,
                    print_schema_hash: options.print_schema_hash,
                    window_flag: options.window_flag,
                },
                &intake,
                &stop,
                &project_root,
            );
            return after_audio(audio, ended_well);
        }
        StopWait::Sim(shutdown) => shutdown,
    };
    #[cfg(not(feature = "wire"))]
    let StopWait::Sim(shutdown) = stop;
    // Only the listener's header reads these.
    #[cfg(not(feature = "wire"))]
    let _ = (record_path, attached);
    // No listener, so no transport: the loop still presents into an attached surface.
    let ended_well = match run_live(engine, intake, shutdown, None) {
        Ok(()) => true,
        Err(diagnostic) => fail(diagnostic),
    };
    after_audio(audio, ended_well)
}

/// Waits (bounded) for the audio session to end, then passes `ended_well` through. The process may
/// exit right after the stop, and exiting while the audio thread is opening the device can deadlock
/// the exit on Windows.
fn after_audio(audio: ludion_audio::AudioSink, ended_well: bool) -> bool {
    audio.wait_timeout(host::AUDIO_STOP_GRACE);
    ended_well
}

/// The process argv as [`run`](crate::run)'s arguments. Foreign arguments (Xcode's
/// `-NSDocumentRevisionsDebugMode YES`, a launcher's own) are not `run`'s, so they warn and yield
/// `None`: the engine hosts.
#[cfg(feature = "wire")]
fn parse_run_argv(argv: &[String]) -> Option<crate::RunArgs> {
    use clap::Parser;
    match crate::RunArgs::try_parse_from(argv) {
        Ok(args) => Some(args),
        Err(error) => {
            let reason = error.to_string();
            let first = reason.lines().next().unwrap_or_default();
            crate::usage::usage_warning(format_args!(
                "ignoring the process arguments (they are not `run`'s): {first}"
            ));
            None
        }
    }
}

/// Binds the wire listener and forwards the core's intake into it until stop, `engine.shutdown`, or
/// the core's intake closes; then shuts the server down cleanly, flushing any armed recording.
/// Returns whether it ended well.
#[cfg(feature = "wire")]
fn host_listener(
    engine: Engine,
    addr: &str,
    opts: host::HostOptions<'_>,
    intake: &Receiver<StampedCommand>,
    stop: &Receiver<()>,
    project_root: &Path,
) -> bool {
    let server = match ludion_server::Server::bind(engine, addr) {
        Ok(server) => server,
        Err(diagnostic) => return fail(diagnostic),
    };
    let asset_watch = host::watch_project(&server, project_root);
    host::announce(&server, &opts);

    let sink = server.command_sink();
    let wire_shutdown = server.wire_shutdown();
    loop {
        crossbeam_channel::select! {
            recv(intake) -> stamped => match stamped {
                Ok(stamped) => sink.forward(stamped),
                Err(_) => break,
            },
            recv(stop) -> _ => break,
            recv(wire_shutdown) -> _ => break,
        }
    }
    drop(asset_watch);
    match server.shutdown() {
        Some(diagnostic) => fail(diagnostic),
        None => true,
    }
}

fn fail(message: impl std::fmt::Display) -> bool {
    eprintln!("{message}");
    false
}

fn no_default_scene() -> Diagnostic {
    Diagnostic::new(
        code::NO_DEFAULT_SCENE,
        "no default scene to host: the engine builder names none",
    )
    .with_help(
        "set one on the engine builder (`EngineBuilder::default_scene`); a host with no scene \
         argument has nowhere else to get it",
    )
}

fn host_gone() -> Diagnostic {
    Diagnostic::new(
        code::PROTOCOL_ENGINE_UNAVAILABLE,
        "the embedded engine is not running: it ended or failed to boot",
    )
    .with_help("the host's stderr carries the diagnostic that ended it")
}

#[cfg(test)]
pub(crate) mod test_game {
    //! A one-entity game whose hash a gamepad moves: the left stick's X, while South is held, drives
    //! a component the `WorldHash` covers — so an input that reached the sim is visible as a hash.

    use crate::prelude::*;
    use serde::{Deserialize, Serialize};
    use std::path::PathBuf;

    #[derive(Serialize, Deserialize, schemars::JsonSchema, Clone, Copy, Debug, Default)]
    #[serde(default)]
    struct Mover {
        x: f32,
    }

    fn drift(world: &mut World, ctx: &SystemCtx) {
        let input = world.resource::<InputState>().expect("input state");
        let stick = input.gamepad().left_stick()[0];
        let active = input.gamepad().is_held(GamepadButton::South);
        if !active {
            return;
        }
        for (mover,) in world.query::<(&mut Mover,)>() {
            mover.x += stick * ctx.dt;
        }
    }

    const SCENE: &str =
        "format: 1\nentities:\n  - name: mover\n    components:\n      Mover:\n        x: 0.0\n";

    pub(crate) fn builder_without_default_scene() -> EngineBuilder {
        Engine::builder()
            .components(|reg| reg.register::<Mover>("Mover"))
            .system("drift", drift)
    }

    pub(crate) fn builder() -> EngineBuilder {
        builder_without_default_scene().default_scene("scenes/main.yaml")
    }

    /// A fresh content root holding `scenes/main.yaml`, unique to this process and `name`.
    pub(crate) fn content_root(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("ludion-embed-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("scenes")).unwrap();
        std::fs::write(dir.join("scenes/main.yaml"), SCENE).unwrap();
        dir
    }

    /// A paused host with a queued render fault, started from a built engine.
    #[cfg(feature = "wire")]
    pub(crate) fn start_faulted(name: &str) -> crate::embed::Embedded {
        use crate::embed::{Embedded, Source, StartOptions};
        use ludion_core::codes::code;
        use ludion_core::diagnostics::Diagnostic;

        let content = content_root(name);
        let scene = content.join("scenes/main.yaml");
        let mut engine = builder()
            .load_scene_file(&scene)
            .unwrap_or_else(|d| panic!("{d}"));
        engine.queue_render_fault(Diagnostic::new(code::RENDER_DEVICE_LOST, "test fault"));
        Embedded::start_from(
            Source::Built {
                engine,
                scene,
                record_path: None,
            },
            None,
            StartOptions {
                paused: true,
                ..StartOptions::default()
            },
        )
        .unwrap_or_else(|d| panic!("{d}"))
    }
}

#[cfg(test)]
mod tests {
    #[cfg(feature = "wire")]
    use super::test_game::start_faulted;
    use super::test_game::{builder, builder_without_default_scene, content_root};
    use super::*;
    use ludion_engine::Engine;
    use ludion_protocol::WireHash;
    use ludion_protocol::command::{ComponentSetParams, QueryEntitiesParams, SimStepParams};
    use ludion_sim::{GamepadAxis, GamepadButton};
    use std::time::Duration;

    fn roots(content: &Path) -> LaunchRoots {
        LaunchRoots {
            content: content.to_path_buf(),
            save: content.join("save-root"),
        }
    }

    fn paused() -> StartOptions {
        StartOptions {
            paused: true,
            ..StartOptions::default()
        }
    }

    /// A host that names no texture source boots cooked; one that names `source` keeps it.
    #[test]
    fn a_hosted_engine_defaults_to_cooked_textures() {
        use ludion_engine::TextureSource;

        assert_eq!(
            StartOptions::default().live_textures(),
            TextureSource::Cooked
        );
        let source = StartOptions {
            textures: Some(TextureSource::Source),
            ..StartOptions::default()
        };
        assert_eq!(source.live_textures(), TextureSource::Source);

        let content = content_root("textures-default");
        let engine = crate::prepare_game_engine(
            builder(),
            &content.join("scenes/main.yaml"),
            None,
            None,
            QualitySetting::full(),
            StartOptions::default().live_textures(),
            StartOptions::default().live_probes(),
            None,
        )
        .unwrap_or_else(|_| panic!("the engine boots"));
        assert_eq!(engine.textures(), TextureSource::Cooked);
    }

    /// A host that names no probe source boots on the store; one that names `Fresh` keeps it.
    #[test]
    fn a_hosted_engine_defaults_to_the_probe_store() {
        use ludion_engine::ProbeSource;

        assert_eq!(StartOptions::default().live_probes(), ProbeSource::Store);
        let fresh = StartOptions {
            probes: Some(ProbeSource::Fresh),
            ..StartOptions::default()
        };
        assert_eq!(fresh.live_probes(), ProbeSource::Fresh);

        let content = content_root("probes-default");
        let engine = crate::prepare_game_engine(
            builder(),
            &content.join("scenes/main.yaml"),
            None,
            None,
            QualitySetting::full(),
            StartOptions::default().live_textures(),
            StartOptions::default().live_probes(),
            None,
        )
        .unwrap_or_else(|_| panic!("the engine boots"));
        assert_eq!(engine.probes(), ProbeSource::Store);
    }

    /// The host's `--aa` beats the builder default, and an unset one leaves it standing.
    #[test]
    fn a_host_aa_mode_beats_the_builder_default() {
        use ludion_engine::AntiAliasing;

        let content = content_root("aa-host");
        let boot = |options: StartOptions| {
            let mut engine = crate::prepare_game_engine(
                builder().anti_aliasing(AntiAliasing::Smaa),
                &content.join("scenes/main.yaml"),
                None,
                None,
                QualitySetting::full(),
                options.live_textures(),
                options.live_probes(),
                None,
            )
            .unwrap_or_else(|_| panic!("the engine boots"));
            apply_host_settings(&mut engine, &options);
            engine.anti_aliasing()
        };
        assert_eq!(boot(StartOptions::default()), AntiAliasing::Smaa);
        let off = StartOptions {
            anti_aliasing: Some(AntiAliasing::Off),
            ..StartOptions::default()
        };
        assert_eq!(boot(off), AntiAliasing::Off);
    }

    fn step_hash(embedded: &Embedded, ticks: u32) -> WireHash {
        let ResponseBody::Stepped { hash: Some(hash) } = embedded
            .request(Command::SimStep(SimStepParams {
                ticks: ticks.into(),
                hash: true,
            }))
            .expect("the sim thread answers the step")
        else {
            panic!("a hashed sim.step replies Stepped with a hash");
        };
        hash
    }

    /// The host runs what its intake carries and flushes the armed recording on stop — proven with no
    /// wall-clock window to race: a hashed `sim.step` is answered by the sim thread (the reply is the
    /// proof it ran), then the stop lands, and the recording written on the join replays to exactly
    /// the hash the live sim reported.
    #[test]
    #[cfg_attr(not(feature = "wire"), allow(clippy::needless_update))]
    fn stop_flushes_an_armed_recording_that_replays_to_the_live_hash() {
        let content = content_root("flush");
        let replay = content.join("session.lrp");
        let embedded = Embedded::start(
            builder(),
            roots(&content),
            None,
            StartOptions {
                paused: true,
                record: Some(replay.clone()),
                ..StartOptions::default()
            },
        )
        .unwrap_or_else(|d| panic!("{d}"));

        let live_hash = step_hash(&embedded, 3);
        assert_eq!(embedded.stop(), ExitCode::SUCCESS);

        let bytes = std::fs::read(&replay).expect("the recording reached disk on the join");
        let hashes = ludion_engine::replay_hashes_with(builder(), &bytes, content.join("scenes"))
            .expect("the flushed recording is a whole session");
        let (last_frame, last_hash) = *hashes.last().expect("the step's ticks were recorded");
        assert!(
            last_frame.as_u64() >= 3,
            "at least the three stepped ticks replay, got frame {}",
            last_frame.as_u64()
        );
        assert_eq!(
            WireHash::from_u64(last_hash.as_u64()),
            live_hash,
            "the recording replays to the hash the live sim reported"
        );
    }

    /// A scene that cannot load ends the host instead of hanging its callers: `ended` disconnects, a
    /// later transition errors, and `stop` reports failure.
    #[test]
    fn a_failed_boot_errors_suspend_disconnects_ended_and_fails_stop() {
        let content = content_root("boot-failure");
        std::fs::remove_file(content.join("scenes/main.yaml")).unwrap();
        let embedded = Embedded::start(builder(), roots(&content), None, paused())
            .unwrap_or_else(|d| panic!("{d}"));

        assert_eq!(
            embedded.ended().recv_timeout(Duration::from_secs(30)),
            Err(crossbeam_channel::RecvTimeoutError::Disconnected)
        );
        assert!(
            embedded.suspend().is_err(),
            "no sim thread is left to answer"
        );
        assert_eq!(embedded.stop(), ExitCode::FAILURE);
    }

    /// A suspend or resume during a boot that has not reached its loop returns promptly instead of
    /// parking the caller, and the engine comes up in the state the queued transitions left it.
    #[test]
    fn suspend_and_resume_during_a_blocked_boot_return_at_once() {
        let content = content_root("blocked-boot");
        let (release, gate) = crossbeam_channel::bounded::<()>(0);
        let blocked = builder().components(move |_| {
            let _ = gate.recv();
            Ok(())
        });
        let embedded = Embedded::start(blocked, roots(&content), None, StartOptions::default())
            .unwrap_or_else(|d| panic!("{d}"));
        embedded.inject(InputEvent::GamepadAxis {
            axis: GamepadAxis::LeftStickX,
            value: 1.0,
        });
        embedded.inject(InputEvent::GamepadButtonDown {
            button: GamepadButton::South,
        });

        let (done_tx, done_rx) = crossbeam_channel::bounded(1);
        std::thread::scope(|scope| {
            scope.spawn(|| {
                let outcome = embedded.suspend().and_then(|()| embedded.resume());
                let outcome = outcome.and_then(|()| embedded.suspend());
                let _ = done_tx.send(outcome);
            });
            let outcome = done_rx.recv_timeout(Duration::from_secs(10));
            drop(release);
            assert!(
                matches!(outcome, Ok(Ok(()))),
                "the boot is still gated, yet the transitions returned: {outcome:?}"
            );
        });

        let held = step_hash(&embedded, 1);
        assert_eq!(embedded.stop(), ExitCode::SUCCESS);

        let reference = Embedded::start(builder(), roots(&content), None, paused())
            .unwrap_or_else(|d| panic!("{d}"));
        reference.inject(InputEvent::GamepadAxis {
            axis: GamepadAxis::LeftStickX,
            value: 1.0,
        });
        reference.inject(InputEvent::GamepadButtonDown {
            button: GamepadButton::South,
        });
        assert_eq!(
            held,
            step_hash(&reference, 1),
            "the engine woke paused: only the stepped tick ran"
        );
        assert_eq!(reference.stop(), ExitCode::SUCCESS);
    }

    /// A sim that never returns to its command loop cannot hang a suspend: it reports `L3023` after
    /// the grace, and the host still stops cleanly once the sim is released.
    #[test]
    fn suspend_against_a_held_sim_reports_l3023() {
        use std::sync::atomic::AtomicBool;

        let content = content_root("held-sim");
        let hold = Arc::new(AtomicBool::new(true));
        let entered = Arc::new(AtomicBool::new(false));
        let held = builder().system("hold", {
            let (hold, entered) = (hold.clone(), entered.clone());
            move |_, _| {
                entered.store(true, Ordering::Release);
                while hold.load(Ordering::Acquire) {
                    std::thread::sleep(Duration::from_millis(1));
                }
            }
        });
        let embedded = Embedded::start(held, roots(&content), None, StartOptions::default())
            .unwrap_or_else(|d| panic!("{d}"));
        // The sim is inside a tick, so it cannot reach the command loop to confirm a pause.
        for _ in 0..6000 {
            if entered.load(Ordering::Acquire) {
                break;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        assert!(entered.load(Ordering::Acquire), "the sim never ticked");

        let (done_tx, done_rx) = crossbeam_channel::bounded(1);
        std::thread::scope(|scope| {
            scope.spawn(|| {
                let _ = done_tx.send(embedded.suspend());
            });
            let outcome = done_rx.recv_timeout(Duration::from_secs(10));
            hold.store(false, Ordering::Release);
            let outcome = outcome.expect("suspend returned instead of hanging");
            let diagnostic = outcome.expect_err("a held sim cannot confirm");
            assert_eq!(diagnostic.code, code::PROTOCOL_SIM_STALLED);
        });
        assert_eq!(embedded.stop(), ExitCode::SUCCESS);
    }

    /// Input queued before the boot finishes is held in order and reaches the sim: the hash moves
    /// only when the events were injected.
    #[test]
    fn input_injected_before_the_boot_finishes_reaches_the_sim() {
        let content = content_root("inject");
        let driven = Embedded::start(builder(), roots(&content), None, paused())
            .unwrap_or_else(|d| panic!("{d}"));
        driven.inject(InputEvent::GamepadAxis {
            axis: GamepadAxis::LeftStickX,
            value: 1.0,
        });
        driven.inject(InputEvent::GamepadButtonDown {
            button: GamepadButton::South,
        });
        let driven_hash = step_hash(&driven, 10);
        assert_eq!(driven.stop(), ExitCode::SUCCESS);

        let idle = Embedded::start(builder(), roots(&content), None, paused())
            .unwrap_or_else(|d| panic!("{d}"));
        let idle_hash = step_hash(&idle, 10);
        assert_eq!(idle.stop(), ExitCode::SUCCESS);

        assert_ne!(
            driven_hash, idle_hash,
            "the injected pad input moved the world"
        );
    }

    #[test]
    fn a_builder_with_no_default_scene_is_l0019() {
        let content = content_root("no-scene");
        let diagnostic = Embedded::start(
            Engine::builder(),
            roots(&content),
            None,
            StartOptions::default(),
        )
        .err()
        .expect("nothing to host");
        assert_eq!(diagnostic.code, code::NO_DEFAULT_SCENE);
    }

    /// The device's boot path: a default scene the content root's manifest does not name is refused
    /// before anything is built.
    #[test]
    fn a_default_scene_the_manifest_does_not_name_is_refused() {
        let content = content_root("mismatch");
        std::fs::write(
            content.join("ludion.toml"),
            "name = \"g\"\ndefault_scene = \"scenes/other.yaml\"\n",
        )
        .unwrap();
        let diagnostic = Embedded::start(builder(), roots(&content), None, StartOptions::default())
            .err()
            .expect("the manifest names another scene");
        assert_eq!(diagnostic.code, code::DEFAULT_SCENE_MISMATCH);
    }

    /// The core injects through the same event shape the desktop window's builders produce: the
    /// wire form's field names and bit-exact analog value.
    #[test]
    fn an_input_event_serializes_to_the_window_builders_shape() {
        let axis = serde_json::to_value(InputEvent::GamepadAxis {
            axis: GamepadAxis::LeftStickX,
            value: 0.5,
        })
        .unwrap();
        assert_eq!(
            axis,
            serde_json::json!({"kind": "gamepad_axis", "axis": "left_stick_x", "value_bits": 0.5f32.to_bits()})
        );
        let button = serde_json::to_value(InputEvent::GamepadButtonDown {
            button: GamepadButton::South,
        })
        .unwrap();
        assert_eq!(
            button,
            serde_json::json!({"kind": "gamepad_button_down", "button": "south"})
        );
    }

    /// A prepared engine hosts through the same core `ludion serve` uses: the caller's armed recording
    /// is flushed by the stop, and replays to the hash the live sim reported.
    #[cfg(feature = "wire")]
    #[test]
    fn a_built_engine_hosts_and_flushes_its_own_recording() {
        let content = content_root("built");
        let replay = content.join("session.lrp");
        let scene = content.join("scenes/main.yaml");
        let mut engine = builder()
            .load_scene_file(&scene)
            .unwrap_or_else(|d| panic!("{d}"));
        engine
            .set_recording(std::fs::File::create(&replay).unwrap())
            .unwrap_or_else(|d| panic!("{d}"));

        let embedded = Embedded::start_from(
            Source::Built {
                engine,
                scene,
                record_path: Some(replay.clone()),
            },
            None,
            StartOptions {
                paused: true,
                listen: Some("127.0.0.1:0".into()),
                ..StartOptions::default()
            },
        )
        .unwrap_or_else(|d| panic!("{d}"));

        let live_hash = step_hash(&embedded, 3);
        assert_eq!(embedded.stop(), ExitCode::SUCCESS);

        let bytes = std::fs::read(&replay).expect("the recording reached disk on the join");
        let hashes = ludion_engine::replay_hashes_with(builder(), &bytes, content.join("scenes"))
            .expect("the flushed recording is a whole session");
        let (_, last_hash) = *hashes.last().expect("the step's ticks were recorded");
        assert_eq!(WireHash::from_u64(last_hash.as_u64()), live_hash);
    }

    /// A booted host arms the engine's render-fault channel: a fault the live loop absorbs reaches
    /// [`Embedded::render_fault`].
    #[cfg(feature = "wire")]
    #[test]
    fn a_booted_host_reports_a_render_fault_on_its_channel() {
        let embedded = start_faulted("faulted-channel");
        let fault = embedded
            .render_fault()
            .recv_timeout(std::time::Duration::from_secs(10))
            .expect("the fault reaches the host's channel");
        assert_eq!(fault.code, code::RENDER_DEVICE_LOST);
        embedded.stop();
    }

    /// A booted host arms the engine's pointer-lock channel: a game system's `lock()` reaches
    /// [`Embedded::cursor_lock_requests`].
    #[cfg(feature = "wire")]
    #[test]
    fn a_booted_host_delivers_a_pointer_lock_request_on_its_channel() {
        fn lock_it(world: &mut ludion_ecs::World, _ctx: &ludion_ecs::SystemCtx) {
            if let Some(lock) = world.resource_mut::<ludion_engine::CursorLock>() {
                lock.lock();
            }
        }
        let content = content_root("cursor-lock-channel");
        let scene = content.join("scenes/main.yaml");
        let engine = builder()
            .system("lock_it", lock_it)
            .load_scene_file(&scene)
            .unwrap_or_else(|d| panic!("{d}"));
        let embedded = Embedded::start_from(
            Source::Built {
                engine,
                scene,
                record_path: None,
            },
            None,
            StartOptions::default(),
        )
        .unwrap_or_else(|d| panic!("{d}"));
        let request = embedded
            .cursor_lock_requests()
            .recv_timeout(Duration::from_secs(10))
            .expect("the request reaches the host's channel");
        assert!(request);
        embedded.stop();
    }

    /// A recording that fails on a full disk makes the host's stop a failure, not a clean exit.
    #[cfg(feature = "wire")]
    #[test]
    fn stop_reports_a_recording_that_failed() {
        use std::sync::Arc;
        use std::sync::atomic::{AtomicBool, Ordering};

        struct BreakableSink(Arc<AtomicBool>);
        impl std::io::Write for BreakableSink {
            fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
                if self.0.load(Ordering::SeqCst) {
                    return Err(std::io::Error::other("disk full"));
                }
                Ok(buf.len())
            }
            fn flush(&mut self) -> std::io::Result<()> {
                if self.0.load(Ordering::SeqCst) {
                    return Err(std::io::Error::other("disk full"));
                }
                Ok(())
            }
        }

        let content = content_root("recording-fails");
        let scene = content.join("scenes/main.yaml");
        let mut engine = builder()
            .load_scene_file(&scene)
            .unwrap_or_else(|d| panic!("{d}"));
        let broken = Arc::new(AtomicBool::new(false));
        engine
            .set_recording(BreakableSink(broken.clone()))
            .expect("the recording arms");
        let embedded = Embedded::start_from(
            Source::Built {
                engine,
                scene,
                record_path: None,
            },
            None,
            paused(),
        )
        .unwrap_or_else(|d| panic!("{d}"));
        broken.store(true, Ordering::SeqCst);
        assert_eq!(embedded.stop(), ExitCode::FAILURE);
    }

    /// The headless host prints a fatal render fault and ends with a failure exit code.
    #[cfg(feature = "wire")]
    #[test]
    fn a_headless_host_ends_on_a_render_fault() {
        let embedded = start_faulted("faulted-host");
        let (tx, rx) = crossbeam_channel::bounded(1);
        std::thread::spawn(move || {
            let code = crate::host::host_until(
                embedded,
                crossbeam_channel::never(),
                crossbeam_channel::never(),
            );
            let _ = tx.send(code);
        });
        let code = rx
            .recv_timeout(std::time::Duration::from_secs(10))
            .expect("the host ends on the fault");
        assert_eq!(code, ExitCode::FAILURE);
    }

    /// `StartOptions::scene` stands in for the builder's default scene, resolved against the content
    /// root, so a game with none still hosts what `--scene` names.
    #[test]
    fn a_named_scene_hosts_a_builder_with_no_default() {
        let content = content_root("scene-override");
        let embedded = Embedded::start(
            builder_without_default_scene(),
            roots(&content),
            None,
            StartOptions {
                paused: true,
                scene: Some("scenes/main.yaml".into()),
                ..StartOptions::default()
            },
        )
        .unwrap_or_else(|d| panic!("{d}"));

        assert_eq!(embedded.scene(), content.join("scenes/main.yaml"));
        step_hash(&embedded, 1);
        assert_eq!(embedded.stop(), ExitCode::SUCCESS);
    }

    fn host_with(content: &Path, restore: Option<PathBuf>) -> Embedded {
        Embedded::start(
            builder(),
            roots(content),
            None,
            StartOptions {
                restore,
                ..paused()
            },
        )
        .unwrap_or_else(|d| panic!("{d}"))
    }

    /// Sets `Mover.x` on the scene's one mover.
    fn set_mover_x(embedded: &Embedded, x: f32) {
        let ResponseBody::QueryResult { entities } = embedded
            .request(Command::QueryEntities(QueryEntitiesParams {
                filter: vec!["Mover".into()],
                include: vec![],
                world: false,
                entities: None,
            }))
            .expect("the query is answered")
        else {
            panic!("query.entities replies QueryResult");
        };
        embedded
            .request(Command::ComponentSet(ComponentSetParams {
                entity: entities[0].entity,
                component: "Mover".into(),
                value: serde_json::json!({ "x": x }),
            }))
            .expect("the mover takes the value");
    }

    /// `--restore` boots from the file and never reads the scene: the restored host continues to the
    /// hash the uninterrupted run reached, with `scenes/main.yaml` replaced by text that cannot parse.
    #[test]
    fn a_restore_boot_continues_the_run_without_reading_the_scene() {
        let content = content_root("restore-boot");

        let reference = host_with(&content, None);
        set_mover_x(&reference, 3.5);
        step_hash(&reference, 3);
        let ResponseBody::Snapshot { snapshot } = reference
            .request(Command::EngineSnapshot)
            .expect("the snapshot is taken")
        else {
            panic!("engine.snapshot replies Snapshot");
        };
        let continued = step_hash(&reference, 4);
        assert_eq!(reference.stop(), ExitCode::SUCCESS);

        let fresh = host_with(&content, None);
        let fresh_hash = step_hash(&fresh, 7);
        assert_eq!(fresh.stop(), ExitCode::SUCCESS);
        assert_ne!(
            continued, fresh_hash,
            "the snapshot carries state the scene does not, so the comparison discriminates"
        );

        let file = content.join("snapshot.json");
        std::fs::write(&file, serde_json::to_string(&snapshot).unwrap()).unwrap();
        std::fs::write(content.join("scenes/main.yaml"), "format: [unclosed").unwrap();

        let restored = host_with(&content, Some(file));
        assert_eq!(step_hash(&restored, 4), continued);
        assert_eq!(restored.stop(), ExitCode::SUCCESS);
    }

    /// A `--restore` boot with the dev-session sidecar adopts `scene.save`'s conflict bases; without
    /// it (or with a file from another frame) the first save over an existing file asks.
    #[test]
    fn a_restore_boot_adopts_the_dev_session_when_its_frame_matches() {
        use ludion_protocol::command::SceneSaveParams;

        let content = content_root("restore-session");
        let scene = content.join("scenes/main.yaml");
        // Bytes the canonical writer would not reproduce, so a save is only safe against a base.
        let hand = format!("# hand note\n{}", std::fs::read_to_string(&scene).unwrap());
        std::fs::write(&scene, &hand).unwrap();
        let session = content.join("session.json");
        let boot = |restore: Option<&Path>, session: Option<&Path>| {
            crate::prepare_game_engine(
                builder(),
                &scene,
                restore,
                session,
                QualitySetting::full(),
                StartOptions::default().live_textures(),
                StartOptions::default().live_probes(),
                None,
            )
            .unwrap_or_else(|_| panic!("the engine boots"))
        };
        let save = |engine: &mut ludion_engine::Engine| {
            engine.execute(Command::SceneSave(SceneSaveParams {
                path: "main.yaml".into(),
                force: false,
            }))
        };

        let mut first = boot(None, Some(&session));
        first
            .execute(Command::SceneLoad(
                ludion_protocol::command::SceneLoadParams {
                    path: "main.yaml".into(),
                },
            ))
            .expect("the scene loads");
        let ResponseBody::Snapshot { snapshot } = first
            .execute(Command::EngineSnapshot)
            .expect("the snapshot is taken")
            .body
        else {
            panic!("engine.snapshot replies Snapshot");
        };
        let file = content.join("snapshot.json");
        std::fs::write(&file, serde_json::to_string(&snapshot).unwrap()).unwrap();

        let mut adopted = boot(Some(&file), Some(&session));
        save(&mut adopted).expect("the adopted base is the text on disk");
        std::fs::write(&scene, &hand).unwrap();

        let mut plain = boot(Some(&file), None);
        let refused = save(&mut plain).expect_err("no sidecar, no base");
        assert_eq!(refused.code, code::SCENE_SAVE_CONFLICT);
    }

    /// `restore` with `record` is refused before any file is touched: the recording the caller
    /// already has stays byte-identical.
    #[test]
    fn restore_with_record_is_refused_before_any_file_is_created() {
        let content = content_root("restore-record");
        let record = content.join("s.lrp");
        std::fs::write(&record, b"keep me").unwrap();

        let refused = Embedded::start(
            builder(),
            roots(&content),
            None,
            StartOptions {
                restore: Some(content.join("snap.json")),
                record: Some(record.clone()),
                ..paused()
            },
        );
        let diagnostic = refused.err().expect("restore with record is refused");
        assert_eq!(diagnostic.code, code::RECORDING_ARM_REFUSED);
        assert_eq!(std::fs::read(&record).unwrap(), b"keep me");
    }

    /// A restore that fails — a refused snapshot, a missing file — still brings the host up, on the
    /// scene's fresh world.
    #[test]
    fn a_failed_restore_boot_falls_back_to_the_scene() {
        let content = content_root("restore-fallback");
        let fresh = host_with(&content, None);
        let fresh_hash = step_hash(&fresh, 1);
        assert_eq!(fresh.stop(), ExitCode::SUCCESS);

        let refused = content.join("refused.json");
        std::fs::write(&refused, r#"{"format": 999}"#).unwrap();
        for file in [refused, content.join("missing.json")] {
            let host = host_with(&content, Some(file));
            assert_eq!(step_hash(&host, 1), fresh_hash);
            assert_eq!(host.stop(), ExitCode::SUCCESS);
        }
    }

    /// A host with a listener forwards the core's intake into its server: a command through the
    /// core is answered, and stop shuts the server down cleanly.
    #[cfg(feature = "wire")]
    #[test]
    fn a_listener_host_answers_commands_and_stops_cleanly() {
        let content = content_root("listener");
        let embedded = Embedded::start(
            builder(),
            roots(&content),
            None,
            StartOptions {
                paused: true,
                listen: Some("127.0.0.1:0".into()),
                ..StartOptions::default()
            },
        )
        .unwrap_or_else(|d| panic!("{d}"));

        embedded.suspend().expect("the server's sim thread answers");
        embedded.resume().expect("and again");
        assert_eq!(embedded.stop(), ExitCode::SUCCESS);
    }

    #[test]
    fn the_safe_area_chooses_720_high_never_narrower_than_16_9() {
        assert_eq!(safe_render_extent(2250, 1170), (1385, 720), "iPhone 13");
        assert_eq!(safe_render_extent(2388, 1668), (1280, 720), "iPad Pro");
        assert_eq!(safe_render_extent(1920, 1080), (1280, 720), "exactly 16:9");
        assert_eq!(safe_render_extent(0, 0), (1280, 720));
        assert_eq!(safe_render_extent(100, 0), (1280, 720));
        assert_eq!(safe_render_extent(u32::MAX, 1).1, 720, "no overflow");
        assert_eq!(
            safe_render_extent(u32::MAX, 1),
            (ludion_engine::MAX_RENDER_WIDTH, 720),
            "a thin safe rect is clamped to the widest extent a host may ask for",
        );
    }

    fn iphone_safe_rect() -> HostRect {
        HostRect {
            x: 141,
            y: 0,
            width: 2250,
            height: 1170,
        }
    }

    fn render_extent(embedded: &Embedded) -> (u32, u32) {
        let ResponseBody::RenderSize { width, height } = embedded
            .request(Command::RenderSize(RenderSizeParams::default()))
            .expect("the sim thread answers the query")
        else {
            panic!("render.size replies RenderSize");
        };
        (width, height)
    }

    /// A host's safe area chooses the render extent when nothing names one; an explicit extent wins.
    #[test]
    fn a_safe_rect_derives_the_render_extent_unless_one_is_named() {
        let content = content_root("safe-extent");
        let derived = Embedded::start(
            builder(),
            roots(&content),
            None,
            StartOptions {
                safe_rect: Some(iphone_safe_rect()),
                ..paused()
            },
        )
        .unwrap_or_else(|d| panic!("{d}"));
        assert_eq!(render_extent(&derived), (1385, 720));
        assert_eq!(derived.stop(), ExitCode::SUCCESS);

        let named = Embedded::start(
            builder(),
            roots(&content),
            None,
            StartOptions {
                safe_rect: Some(iphone_safe_rect()),
                render_size: Some("960x540".into()),
                ..paused()
            },
        )
        .unwrap_or_else(|d| panic!("{d}"));
        assert_eq!(render_extent(&named), (960, 540));
        assert_eq!(named.stop(), ExitCode::SUCCESS);
    }

    /// A rotation that changes the safe area's aspect moves a derived extent through `render.size`;
    /// a named extent never moves.
    #[test]
    fn a_resize_follows_a_derived_extent_and_leaves_a_named_one() {
        let content = content_root("safe-resize");
        let derived = Embedded::start(
            builder(),
            roots(&content),
            None,
            StartOptions {
                safe_rect: Some(iphone_safe_rect()),
                ..paused()
            },
        )
        .unwrap_or_else(|d| panic!("{d}"));
        let square = HostRect {
            x: 0,
            y: 0,
            width: 2000,
            height: 1000,
        };
        derived.resize(2000, 1000, Some(square));
        assert_eq!(render_extent(&derived), (1440, 720));
        assert_eq!(derived.stop(), ExitCode::SUCCESS);

        let named = Embedded::start(
            builder(),
            roots(&content),
            None,
            StartOptions {
                safe_rect: Some(iphone_safe_rect()),
                render_size: Some("960x540".into()),
                ..paused()
            },
        )
        .unwrap_or_else(|d| panic!("{d}"));
        named.resize(2000, 1000, Some(square));
        assert_eq!(render_extent(&named), (960, 540));
        assert_eq!(named.stop(), ExitCode::SUCCESS);

        // The builder's own call names an extent too, and beats the safe rect.
        let pinned = Embedded::start(
            builder().render_size(960, 540),
            roots(&content),
            None,
            StartOptions {
                safe_rect: Some(iphone_safe_rect()),
                ..paused()
            },
        )
        .unwrap_or_else(|d| panic!("{d}"));
        pinned.resize(2000, 1000, Some(square));
        assert_eq!(render_extent(&pinned), (960, 540));
        assert_eq!(pinned.stop(), ExitCode::SUCCESS);
    }

    #[test]
    fn the_window_rule_keeps_the_window_height_and_the_16_9_floor() {
        assert_eq!(window_render_extent(2560, 1440), Some((2560, 1440)));
        assert_eq!(
            window_render_extent(3024, 1964),
            Some((3492, 1964)),
            "a taller-than-16:9 window floors at 16:9 and letterboxes"
        );
        assert_eq!(
            window_render_extent(5120, 1440),
            Some((5120, 1440)),
            "ultrawide"
        );
        assert_eq!(window_render_extent(1920, 1200), Some((2133, 1200)));
        assert_eq!(
            window_render_extent(10000, 5000),
            Some((
                ludion_engine::MAX_RENDER_WIDTH,
                ludion_engine::MAX_RENDER_HEIGHT
            )),
            "both caps"
        );
        assert_eq!(window_render_extent(0, 700), None);
        assert_eq!(window_render_extent(700, 0), None);
        assert_eq!(window_render_extent(1, 1), Some((2, 1)));
    }

    fn windowed(content: &Path, builder: EngineBuilder, render_size: Option<&str>) -> Embedded {
        Embedded::start(
            builder,
            roots(content),
            None,
            StartOptions {
                follow_surface: true,
                size: Some((2560, 1440)),
                render_size: render_size.map(Into::into),
                ..paused()
            },
        )
        .unwrap_or_else(|d| panic!("{d}"))
    }

    /// A following window chooses the render extent when nothing names one; the option, the
    /// environment and the builder's own call all count as naming it.
    #[test]
    fn a_window_derives_the_render_extent_unless_one_is_named() {
        let content = content_root("window-extent");
        let derived = windowed(&content, builder(), None);
        assert_eq!(render_extent(&derived), (2560, 1440));
        assert_eq!(derived.stop(), ExitCode::SUCCESS);

        let option = windowed(&content, builder(), Some("960x540"));
        assert_eq!(render_extent(&option), (960, 540));
        assert_eq!(option.stop(), ExitCode::SUCCESS);

        let call = windowed(&content, builder().render_size(960, 540), None);
        assert_eq!(render_extent(&call), (960, 540));
        assert_eq!(call.stop(), ExitCode::SUCCESS);

        let today = Embedded::start(
            builder(),
            roots(&content),
            None,
            StartOptions {
                size: Some((2560, 1440)),
                ..paused()
            },
        )
        .unwrap_or_else(|d| panic!("{d}"));
        assert_eq!(render_extent(&today), (1280, 720), "no follow, no change");
        assert_eq!(today.stop(), ExitCode::SUCCESS);
    }

    /// `follow_surface` moves a derived extent, ignores a minimize, and never moves a named one; a
    /// plain `resize` alone never does.
    #[test]
    fn follow_surface_moves_a_derived_extent_and_leaves_a_named_one() {
        let content = content_root("window-follow");
        let derived = windowed(&content, builder(), None);
        derived.resize(1920, 1200, None);
        assert_eq!(
            render_extent(&derived),
            (2560, 1440),
            "resize alone never moves it"
        );
        derived.follow_surface(1920, 1200);
        assert_eq!(render_extent(&derived), (2133, 1200));
        derived.follow_surface(0, 0);
        assert_eq!(
            render_extent(&derived),
            (2133, 1200),
            "a minimize leaves it"
        );
        assert_eq!(derived.stop(), ExitCode::SUCCESS);

        for named in [
            windowed(&content, builder(), Some("960x540")),
            windowed(&content, builder().render_size(960, 540), None),
        ] {
            named.follow_surface(1920, 1200);
            assert_eq!(render_extent(&named), (960, 540));
            assert_eq!(named.stop(), ExitCode::SUCCESS);
        }
    }

    /// The live hash after `ticks` cumulative stepped ticks equals the replay's hash at frame
    /// `ticks - 1` (the last tick's own frame),
    /// and the live hashes differ from one another, so a match by frame is not a match by accident.
    fn assert_replays_to(
        hashes: &[(crate::prelude::FrameIndex, crate::prelude::WorldHash)],
        live: &[(u64, WireHash)],
    ) {
        for (i, (ticks, hash)) in live.iter().enumerate() {
            let replayed = hashes
                .iter()
                .find(|(frame, _)| frame.as_u64() + 1 == *ticks)
                .unwrap_or_else(|| panic!("the replay has a hash at frame {ticks}"));
            assert_eq!(
                WireHash::from_u64(replayed.1.as_u64()),
                *hash,
                "the replay hash at frame {ticks} is the live one"
            );
            assert!(
                live[..i].iter().all(|(_, earlier)| earlier != hash),
                "the live hashes differ frame to frame"
            );
        }
    }

    /// A recording keeps the default extent while the window resizes, and replays to the live hashes.
    #[test]
    fn a_recording_pins_the_default_extent_while_the_window_resizes() {
        let content = content_root("window-record");
        let replay = content.join("session.lrp");
        let embedded = Embedded::start(
            builder(),
            roots(&content),
            None,
            StartOptions {
                follow_surface: true,
                size: Some((2560, 1440)),
                record: Some(replay.clone()),
                ..paused()
            },
        )
        .unwrap_or_else(|d| panic!("{d}"));
        assert_eq!(render_extent(&embedded), (1280, 720));
        let mut live = vec![(2, step_hash(&embedded, 2))];
        embedded.follow_surface(3024, 1964);
        embedded.resize(3024, 1964, None);
        set_mover_x(&embedded, 4.0);
        live.push((4, step_hash(&embedded, 2)));
        embedded.follow_surface(5120, 1440);
        set_mover_x(&embedded, 6.0);
        live.push((6, step_hash(&embedded, 2)));
        assert_eq!(render_extent(&embedded), (1280, 720));
        assert_eq!(embedded.stop(), ExitCode::SUCCESS);

        let bytes = std::fs::read(&replay).expect("the recording reached disk");
        let hashes = ludion_engine::replay_hashes_with(builder(), &bytes, content.join("scenes"))
            .expect("the flushed recording is a whole session");
        assert_replays_to(&hashes, &live);
    }

    /// A session whose extent moves replays to the same hashes, with the extent coming from the
    /// recorded `render.size` alone.
    #[test]
    fn a_recording_made_while_the_extent_moves_replays_to_the_same_hashes() {
        let content = content_root("safe-record");
        let replay = content.join("session.lrp");
        let embedded = Embedded::start(
            builder(),
            roots(&content),
            None,
            StartOptions {
                safe_rect: Some(iphone_safe_rect()),
                record: Some(replay.clone()),
                ..paused()
            },
        )
        .unwrap_or_else(|d| panic!("{d}"));
        let square = HostRect {
            x: 0,
            y: 0,
            width: 2000,
            height: 1000,
        };
        let mut live = vec![(2, step_hash(&embedded, 2))];
        embedded.resize(2000, 1000, Some(square));
        set_mover_x(&embedded, 4.0);
        live.push((4, step_hash(&embedded, 2)));
        assert_eq!(render_extent(&embedded), (1440, 720));
        embedded.resize(2391, 1170, Some(iphone_safe_rect()));
        set_mover_x(&embedded, 6.0);
        live.push((6, step_hash(&embedded, 2)));
        assert_eq!(embedded.stop(), ExitCode::SUCCESS);

        let bytes = std::fs::read(&replay).expect("the recording reached disk");
        let hashes = ludion_engine::replay_hashes_with(builder(), &bytes, content.join("scenes"))
            .expect("the flushed recording is a whole session");
        assert_replays_to(&hashes, &live);
        let mut extents = Vec::new();
        let engine =
            ludion_engine::replay_with(builder(), &bytes, content.join("scenes"), |engine, _| {
                extents.push(engine.render_size());
                Ok(std::ops::ControlFlow::Continue(()))
            })
            .expect("the recording drives to its end");
        assert!(
            extents.contains(&(1440, 720)),
            "the replay passes through the recorded render.size"
        );
        assert_eq!(
            extents.first(),
            Some(&(1385, 720)),
            "the header's start extent"
        );
        assert_eq!(engine.render_size(), (1385, 720));
    }

    #[cfg(feature = "wire")]
    fn with_argv(content: &Path, argv: &[&str]) -> Embedded {
        Embedded::start(
            builder(),
            roots(content),
            None,
            StartOptions {
                argv: Some(argv.iter().map(|arg| (*arg).to_string()).collect()),
                ..paused()
            },
        )
        .unwrap_or_else(|d| panic!("{d}"))
    }

    /// The process argv names `--replay-verify`: the boot thread runs it against the roots in place
    /// of hosting — no host is left running — and its verdict is the stop code.
    #[cfg(feature = "wire")]
    #[test]
    fn an_argv_replay_verify_runs_on_the_boot_thread_against_the_roots() {
        let content = content_root("argv-verify");
        let replay = content.join("session.lrp");
        let recorded = Embedded::start(
            builder(),
            roots(&content),
            None,
            StartOptions {
                record: Some(replay.clone()),
                ..paused()
            },
        )
        .unwrap_or_else(|d| panic!("{d}"));
        step_hash(&recorded, 3);
        assert_eq!(recorded.stop(), ExitCode::SUCCESS);

        let verified = with_argv(&content, &["game", "--replay-verify", "session.lrp"]);
        assert_eq!(
            verified.ended().recv_timeout(Duration::from_secs(30)),
            Err(crossbeam_channel::RecvTimeoutError::Disconnected),
            "the mode ran and no host was started"
        );
        assert_eq!(verified.stop(), ExitCode::SUCCESS);

        let missing = with_argv(&content, &["game", "--replay-verify", "missing.lrp"]);
        assert_eq!(missing.stop(), ExitCode::FAILURE, "a failing mode fails");
    }

    /// Arguments that are not `run`'s (Xcode's launch arguments) are ignored: the engine hosts.
    #[cfg(feature = "wire")]
    #[test]
    fn foreign_argv_hosts() {
        let content = content_root("argv-foreign");
        let embedded = with_argv(&content, &["game", "-NSDocumentRevisionsDebugMode", "YES"]);
        step_hash(&embedded, 1);
        assert_eq!(embedded.stop(), ExitCode::SUCCESS);
    }
}
