//! [`EngineBuilder`] — the one way to construct an [`Engine`] that hosts a game's own components and
//! systems on top of the built-in core.
//!
//! A game needs everything the engine provides — scene load/validation through the registry,
//! deterministic sim, `WorldHash`, recording/replay, the wire protocol — *and* its own components and
//! systems. The builder is the single seam that adds the latter without forking the former:
//!
//! - **Components register on top of the built-ins.** [`components`](EngineBuilder::components) runs
//!   a closure against a registry that already holds `Transform`, `Sprite`, … so a scene freely mixes
//!   built-in visuals with game components. A collision (a name already taken, a type registered
//!   twice) surfaces as the registry's own structured `L0005` [`Diagnostic`] from
//!   [`build`](EngineBuilder::build) — construction fails loudly, it does not silently overwrite.
//! - **Systems are plain `fn`s by convention** added in order with [`system`](EngineBuilder::system)
//!   — state lives in the `World`, so a system needs no captured state (the method's generic bound
//!   accepts a closure only so it can capture registration metadata, not as an invitation to carry
//!   state in one). The engine stores the list and rebuilds a fresh
//!   [`Schedule`](ludion_ecs::Schedule) from it on every world reset, so the schedule *survives*
//!   `scene.load` and `engine.restore` (see `Engine::build_schedule`).
//!
//! Recording, replay, snapshot/restore, and `WorldHash` need no extra wiring — they are all
//! registry-driven, so a custom component travels through them for free.
//!
//! # Example
//! ```
//! use ludion_engine::Engine;
//! use serde::{Deserialize, Serialize};
//! use schemars::JsonSchema;
//!
//! #[derive(Serialize, Deserialize, JsonSchema, Clone, Copy)]
//! struct Paddle { x: f32 }
//!
//! fn move_paddle(world: &mut ludion_ecs::World, ctx: &ludion_ecs::SystemCtx) {
//!     for (paddle,) in world.query::<(&mut Paddle,)>() {
//!         paddle.x += 4.0 * ctx.dt;
//!     }
//! }
//!
//! let mut engine = Engine::builder()
//!     .components(|reg| reg.register::<Paddle>("Paddle"))
//!     .system("move_paddle", move_paddle)
//!     .build()
//!     .unwrap();
//! engine.load_inline_scene("format: 1\nentities: []\n").unwrap();
//! ```

use std::path::{Path, PathBuf};

use ludion_core::audio::CategoryGains;
use ludion_core::codes::code;
use ludion_core::diagnostics::Diagnostic;
use ludion_ecs::Registry;
use ludion_protocol::Command;
use ludion_protocol::command::SceneLoadParams;
use ludion_renderer::AntiAliasing;
use ludion_spatial::Plane;
use schemars::JsonSchema;
use serde::Serialize;
use serde::de::DeserializeOwned;

use crate::audio::{AudioDistance, AudioPan};
use crate::builtin_registry;
use crate::docs::{DocKind, DocValidator};
use crate::engine::{DEFAULT_RENDER_SIZE, Engine, SystemSpec};
use crate::player_profile::ProfileKind;

/// The `kind:` a game's projectile-pattern documents declare — the one engine kind registered as a
/// doc store.
pub(crate) const PATTERN_KIND: &str = "pattern";

/// One game-component registration: a closure run against the registry that already holds the
/// built-ins. `FnOnce` because it runs exactly once, at [`build`](EngineBuilder::build) time; `Send`
/// so a host can move the whole builder onto the thread that boots the engine.
type Registration = Box<dyn FnOnce(&mut Registry) -> Result<(), Diagnostic> + Send>;

/// One registered resource mover, from the departing world to the arriving one
/// ([`EngineBuilder::carry_resource`]). Type-erased so the engine holds a list of them without naming
/// any game type; run only by the scene-switch path, once per switch, before the driver is swapped.
pub type ResourceCarrier =
    Box<dyn Fn(&mut ludion_ecs::World, &mut ludion_ecs::World) + Send + Sync>;

/// How a window host treats the mouse cursor: hidden when idle, always shown, or locked. Host state,
/// like the rest of the builder's window settings.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum CursorMode {
    /// Hide the cursor after 2 s without motion; any motion shows it again.
    #[default]
    AutoHide,
    /// Always show the cursor.
    Visible,
    /// Start with the pointer locked: grabbed, hidden and centred; raw `delta()` keeps flowing. A
    /// system releases or re-locks it through [`CursorLock`](crate::CursorLock). Only the in-process
    /// window applies it.
    Locked,
}

/// Builds an [`Engine`] that hosts a game's components and systems on top of the built-in core.
///
/// See the [module docs](crate::builder) for the design. Construct one with [`Engine::builder`],
/// describe the game, and call [`build`](EngineBuilder::build) (or hand the builder to
/// `ludion::run` / [`replay_hashes_with`](crate::replay_hashes_with), which build it for you).
#[derive(Default)]
pub struct EngineBuilder {
    /// The first `.query()` called before any `.system()`; `build_at` refuses it (`L1393`).
    orphan_query: Option<&'static std::panic::Location<'static>>,
    project_root: PathBuf,
    save_root: Option<PathBuf>,
    profile_persistence: bool,
    profile_off_disk: bool,
    run_save: bool,
    checkpoints: Option<crate::CheckpointConfig>,
    run_recordings: Option<crate::RunRecordingConfig>,
    replay_playback: Option<crate::ReplayPlaybackConfig>,
    playback_controls: Option<crate::replay_playback::PlaybackHook>,
    probe_bake_worlds: Option<crate::probe_bake::ProbeBakeWorlds>,
    game_version: Option<String>,
    observe_load: bool,
    category_gains: CategoryGains,
    audio_distance: AudioDistance,
    audio_pan: AudioPan,
    plane: Plane,
    render_size: Option<(u32, u32)>,
    anti_aliasing: AntiAliasing,
    title: Option<String>,
    fullscreen: bool,
    present_interpolation: bool,
    low_latency: bool,
    late_latch: bool,
    cursor: CursorMode,
    registrations: Vec<Registration>,
    systems: Vec<SystemSpec>,
    doc_kinds: Vec<DocKind>,
    /// Whether any entry of `doc_kinds` came from the public [`doc_kind`](EngineBuilder::doc_kind) —
    /// a *game* teaching the engine one of its own types. False while only engine-owned kinds are
    /// registered (what [`patterns`](EngineBuilder::patterns) adds), which is what lets the built-in
    /// engine skip a foreign `kind:` instead of refusing the boot.
    game_doc_kinds: bool,
    doc_validators: Vec<DocValidator>,
    doc_texts: std::collections::BTreeMap<String, String>,
    resource_carriers: Vec<ResourceCarrier>,
    projectile_config: Option<crate::ProjectileConfig>,
    nav_config: Option<crate::NavConfig>,
    perception_config: Option<crate::PerceptionConfig>,
    input_actions: bool,
    projectile_visuals: Option<crate::ProjectileVisuals>,
    profile_kind: Option<ProfileKind>,
    profile_path: Option<PathBuf>,
    default_scene: Option<PathBuf>,
}

impl EngineBuilder {
    /// Starts a builder with the defaults [`Engine::new`] uses: the current directory as project
    /// root and a 1280×720 screenshot size, no game components or systems yet.
    pub fn new() -> Self {
        EngineBuilder {
            orphan_query: None,
            project_root: PathBuf::from("."),
            save_root: None,
            profile_persistence: false,
            profile_off_disk: false,
            run_save: false,
            checkpoints: None,
            run_recordings: None,
            replay_playback: None,
            playback_controls: None,
            probe_bake_worlds: None,
            game_version: None,
            observe_load: false,
            category_gains: CategoryGains::default(),
            audio_distance: AudioDistance::default(),
            audio_pan: AudioPan::default(),
            plane: Plane::default(),
            render_size: None,
            anti_aliasing: AntiAliasing::default(),
            title: None,
            fullscreen: false,
            present_interpolation: false,
            low_latency: false,
            late_latch: false,
            cursor: CursorMode::default(),
            registrations: Vec::new(),
            systems: Vec::new(),
            doc_kinds: Vec::new(),
            game_doc_kinds: false,
            doc_validators: Vec::new(),
            doc_texts: std::collections::BTreeMap::new(),
            resource_carriers: Vec::new(),
            projectile_config: None,
            nav_config: None,
            perception_config: None,
            input_actions: false,
            projectile_visuals: None,
            profile_kind: None,
            profile_path: None,
            default_scene: None,
        }
    }

    /// Sets the project root that `scene.load`'s relative path and every asset reference resolve
    /// against. (Hosting entry points such as `ludion::run` override this to the scene file's own
    /// directory; see [`load_scene_file`](EngineBuilder::load_scene_file).)
    #[must_use]
    pub fn project_root(mut self, project_root: impl Into<PathBuf>) -> Self {
        self.project_root = project_root.into();
        self
    }

    /// Sets the directory the player profile loads from and saves to. Default: the project root, so a
    /// save sits beside its game's content. A shipped build points it at a writable directory outside a
    /// read-only content root (`ludion::launch` sets it from `LaunchRoots::save`). `profile_path` stays
    /// relative to it.
    #[must_use]
    pub fn save_root(mut self, save_root: impl Into<PathBuf>) -> Self {
        self.save_root = Some(save_root.into());
        self
    }

    /// Enables writing the save root at the frame boundary (default off): the profile and, with
    /// [`run_save`](EngineBuilder::run_save), the suspended run; it also backs up an unloadable
    /// profile at build. The `set_audio_sender` host-side mold: only a live host
    /// (`ludion::run`) sets it; a replay, `--check-docs`, `--profile`, or a test leaves it off, so the
    /// [`profile_save_log`](Engine::profile_save_log) still fills but no disk write clobbers the
    /// machine's real save and no boot renames it (decision 5). The write target is the resolved
    /// `<save_root>/<profile_rel_path>` — the same path the boot profile load reads.
    #[must_use]
    pub fn persist_profile(mut self) -> Self {
        self.profile_persistence = true;
        self
    }

    /// Installs a [`RunSave`](crate::RunSave) resource: a game system calls `suspend()` and the
    /// engine writes the running game to `<save_root>/save/suspend.json` at the frame boundary, the
    /// `engine.snapshot` document inside an envelope carrying a [`BuildStamp`](crate::BuildStamp).
    /// The file is read at boot (`RunSave::pending`) and written or deleted only when
    /// [`persist_profile`](EngineBuilder::persist_profile) is on, so a test or replay never touches
    /// disk; the [log](Engine::run_save_log) fills either way. `resume()` restores the file in place
    /// (strictly: a changed build or schema is refused with `L1401`).
    #[must_use]
    pub fn run_save(mut self) -> Self {
        self.run_save = true;
        self
    }

    /// Installs a [`RunRecording`](crate::RunRecording) resource: a game system calls `start(name)`
    /// and `stop()`, and the engine applies them at a free-running frame boundary. A start restarts
    /// the run from the current scene's inline text (only the profile and run-save state carry over)
    /// and records it to `<save_root>/<dir>/<name>.lrp`; a stop publishes it crash-durably. Like
    /// [`run_save`](EngineBuilder::run_save) it touches disk only with
    /// [`persist_profile`](EngineBuilder::persist_profile); elsewhere a request is only logged.
    ///
    /// `build` fails with `L1403` for a zero `keyframe_every` or a `dir` that is not a relative path.
    #[must_use]
    pub fn run_recordings(mut self, config: crate::RunRecordingConfig) -> Self {
        self.run_recordings = Some(config);
        self
    }

    /// Installs a [`Checkpoints`](crate::Checkpoints) resource: a game system captures the world into
    /// a named in-memory slot and restores it at the frame boundary, keeping the frame, held input,
    /// the live profile, `carry_resource` resources and an open recording. See `ludion docs
    /// checkpoints`.
    ///
    /// `build` fails with `L1560` for a `max_slots` of 0 or above 64.
    #[must_use]
    pub fn checkpoints(mut self, config: crate::CheckpointConfig) -> Self {
        self.checkpoints = Some(config);
        self
    }

    /// Installs a [`ReplayPlayback`](crate::ReplayPlayback) resource: a game system calls
    /// `play(name, return_to)` and the engine turns itself, in place, into the replay of
    /// `<save_root>/<dir>/<name>.lrp` at a free-running frame boundary, then reloads `return_to`
    /// when the file ends. It also lists and deletes recordings. Like
    /// [`run_save`](EngineBuilder::run_save) it acts only with
    /// [`persist_profile`](EngineBuilder::persist_profile); elsewhere a request is only logged.
    ///
    /// `build` fails with `L1405` for a `dir` that is not a relative path inside the save root.
    #[must_use]
    pub fn replay_playback(mut self, config: crate::ReplayPlaybackConfig) -> Self {
        self.replay_playback = Some(config);
        self
    }

    /// Installs the playback-controls hook: called at every live boundary while a replay plays,
    /// paused included, with the live input and a [`PlaybackControls`](crate::PlaybackControls) to
    /// pause, resume, change the pace or stop. It sees live input only and has no world access; the
    /// game maps the keys, the engine has no key policy. A hook without
    /// [`replay_playback`](EngineBuilder::replay_playback) is never called.
    #[must_use]
    pub fn playback_controls(
        mut self,
        f: impl FnMut(&ludion_sim::InputState, &mut crate::PlaybackControls) + Send + 'static,
    ) -> Self {
        self.playback_controls = Some(Box::new(f));
        self
    }

    /// Registers the worlds [`Engine::bake_probes`] bakes beyond the loaded scene's: the probes a game
    /// spawns at runtime (a prefab room swap, see [`ProbeRefreeze`](crate::ProbeRefreeze)).
    ///
    /// After baking the loaded world, `bake_probes` calls `f(engine, i)` for `i = 0, 1, …` until it
    /// returns `Ok(None)`, on one engine: world `i` replaces world `i - 1`, so the game despawns the
    /// last one. The hook puts world `i` in place (spawn the room prefab, place the player, step ticks
    /// with `Command::SimStep` if its systems need them) and returns the world's label; leave the world
    /// as the game's swap tick leaves it. The engine then re-freezes the probe world as a tick start
    /// does, so the bake keys the world a `Store` host captures, and bakes the probes present. Each
    /// such probe's report entry carries the label as `world`. An `Err` fails the bake. The hook runs
    /// after every `--bake-probes` load, so one that does not drive the loaded scene returns `Ok(None)`
    /// at once.
    ///
    /// ```
    /// use ludion_engine::Engine;
    ///
    /// let worlds = ["room-a", "room-b"];
    /// let engine = Engine::builder()
    ///     .probe_bake_worlds(move |_engine, i| Ok(worlds.get(i).map(|label| label.to_string())))
    ///     .build()
    ///     .unwrap();
    /// # drop(engine);
    /// ```
    #[must_use]
    pub fn probe_bake_worlds(
        mut self,
        f: impl FnMut(
            &mut crate::Engine,
            usize,
        ) -> Result<Option<String>, ludion_core::diagnostics::Diagnostic>
        + Send
        + 'static,
    ) -> Self {
        self.probe_bake_worlds = Some(Box::new(f));
        self
    }

    /// Names this build of the game in the identity a run recording carries
    /// ([`Engine::game_identity`]). A replay recorded under a different version is refused with
    /// `L1404`, so bump it for changes the component schemas cannot see, such as a balance pass.
    ///
    /// ```
    /// use ludion_engine::Engine;
    ///
    /// let engine = Engine::builder().game_version("1.2").build().unwrap();
    /// assert_eq!(engine.game_identity().version.as_deref(), Some("1.2"));
    /// ```
    #[must_use]
    pub fn game_version(mut self, version: impl Into<String>) -> Self {
        self.game_version = Some(version.into());
        self
    }

    /// Makes every profile install read the recording or the default, never `save/profile.yaml`.
    /// Replay-only (`prepare_replay`), so a replay never depends on the developer's save.
    #[must_use]
    pub(crate) fn replay_profile(mut self) -> Self {
        self.profile_off_disk = true;
        self
    }

    /// Attaches a load observer to the built engine ([`Engine::observe_load`]), so the scene load a
    /// following [`load_scene_file`](EngineBuilder::load_scene_file) runs is measured too. Only the
    /// offline profile driver sets it.
    ///
    /// # Example
    /// ```no_run
    /// use ludion_engine::Engine;
    ///
    /// let engine = Engine::builder()
    ///     .observe_load()
    ///     .load_scene_file(std::path::Path::new("scenes/main.yaml"))?;
    /// # let _ = engine;
    /// # Ok::<(), ludion_core::diagnostics::Diagnostic>(())
    /// ```
    #[must_use]
    pub fn observe_load(mut self) -> Self {
        self.observe_load = true;
        self
    }

    /// Sets this game's audio mixdown: the master gain each [`AudioCategory`](crate::AudioCategory)
    /// bus is multiplied by while the sink mixes. Default: the identity, so every voice plays at the
    /// gain its system authored.
    ///
    /// The bus balance ("ambience sits under the effects") is a property of the game, authored once
    /// here rather than per run — there is no flag and no runtime setter, and no host overrides it:
    /// `ludion::run`, `ludion::launch` and `ludion serve` all read this value off the built engine.
    /// It is mixdown, never simulation state: it reaches no `WorldHash`, no snapshot and no audio
    /// log, so changing it cannot move a replay.
    #[must_use]
    pub fn category_gains(mut self, gains: CategoryGains) -> Self {
        self.category_gains = gains;
        self
    }

    /// Sets whether positional sound is placed across the stereo field. Default
    /// [`AudioPan::Centred`], which keeps the audio stream of a game that never calls this.
    ///
    /// With [`AudioPan::Stereo`] every positional one-shot ([`play_at`](crate::AudioQueue::play_at))
    /// and [`AudioEmitter`](crate::AudioEmitter) carries a `pan` from `-1.0` (left) to `1.0` (right),
    /// resolved against the camera: its right vector, on the horizontal plane, so height never pans
    /// a sound. The sink applies it on a two-channel device only.
    ///
    /// Authored once on the game, like [`category_gains`](Self::category_gains): not simulation
    /// state, so it reaches no `WorldHash` and no replay, which regenerates the stream — the game's
    /// `game()` sets it.
    ///
    /// # Example
    /// ```
    /// use ludion_engine::{AudioPan, Engine};
    ///
    /// let engine = Engine::builder().audio_pan(AudioPan::Stereo).build()?;
    /// assert_eq!(engine.audio_pan(), AudioPan::Stereo);
    /// # Ok::<(), ludion_core::diagnostics::Diagnostic>(())
    /// ```
    #[must_use]
    pub fn audio_pan(mut self, pan: AudioPan) -> Self {
        self.audio_pan = pan;
        self
    }

    /// Sets how positional sound measures distance to the listener. Default
    /// [`AudioDistance::Planar`] (ground plane, height ignored), which keeps the audio stream of a
    /// game that never calls this; [`AudioDistance::Spatial`] counts height, for a 3D game.
    ///
    /// Authored once on the game, not simulation state: no `WorldHash`, no replay, and the game's
    /// `game()` sets it.
    ///
    /// # Example
    /// ```
    /// use ludion_engine::{AudioDistance, Engine};
    ///
    /// let engine = Engine::builder().audio_distance(AudioDistance::Spatial).build()?;
    /// assert_eq!(engine.audio_distance(), AudioDistance::Spatial);
    /// # Ok::<(), ludion_core::diagnostics::Diagnostic>(())
    /// ```
    #[must_use]
    pub fn audio_distance(mut self, distance: AudioDistance) -> Self {
        self.audio_distance = distance;
        self
    }

    /// Sets the plane this game's 2D queries live in — collision, and any engine feature that turns
    /// a 2D angle into a world direction. Default: [`Plane::Xz`], the ground plane.
    ///
    /// Which two world axes the game plays on is a property of the game, authored once here rather
    /// than per run — there is no flag and no runtime setter. A game whose sprites and tilemaps live
    /// in world XY (the plane both draw on) chooses [`Plane::Xy`] so its colliders sit where its
    /// content does instead of stacking at one depth. The value reaches the world as the [`Plane`]
    /// resource, reinstalled at every world (re)build, and is excluded from the world hash and from
    /// snapshots — but it *does* move a replay, because the queries built on it resolve differently.
    #[must_use]
    pub fn plane(mut self, plane: Plane) -> Self {
        self.plane = plane;
        self
    }

    /// Sets the default screenshot resolution (used when a `render.screenshot` omits dimensions).
    /// Naming one pins the render extent: a host that would follow its window or safe area keeps
    /// this one, letterboxed.
    #[must_use]
    pub fn render_size(mut self, width: u32, height: u32) -> Self {
        self.render_size = Some((width, height));
        self
    }

    /// Sets the anti-aliasing mode this game ships with ([`AntiAliasing::Off`] unless set). A packaged
    /// default: a host's [`Engine::set_anti_aliasing`] beats it, and so does the game's own runtime
    /// request through [`RenderSettings`](crate::RenderSettings).
    #[must_use]
    pub fn anti_aliasing(mut self, mode: AntiAliasing) -> Self {
        self.anti_aliasing = mode;
        self
    }

    /// Sets the window title a window host shows. Default: the project manifest's `name`, else
    /// `Ludion`. Read by every window host; host state, never reaches the world, hash or snapshots.
    ///
    /// # Example
    /// ```
    /// use ludion_engine::Engine;
    ///
    /// let builder = Engine::builder().title("Clockout");
    /// assert_eq!(builder.title_hint(), Some("Clockout"));
    /// ```
    #[must_use]
    pub fn title(mut self, title: impl Into<String>) -> Self {
        self.title = Some(title.into());
        self
    }

    /// Sets whether a window host opens borderless fullscreen (default windowed). The `--fullscreen`
    /// flag and `LUDION_FULLSCREEN` beat it. Read by every window host; host state, never reaches the
    /// world, hash or snapshots.
    ///
    /// # Example
    /// ```
    /// use ludion_engine::Engine;
    ///
    /// assert!(Engine::builder().fullscreen(true).fullscreen_hint());
    /// ```
    #[must_use]
    pub fn fullscreen(mut self, fullscreen: bool) -> Self {
        self.fullscreen = fullscreen;
        self
    }

    /// Sets whether a windowed host draws every display refresh blended between the last two ticks
    /// (default off: one draw per tick). Render-only: the sim, hashes, replays, screenshot, stream
    /// and goldens are unchanged. Snaps and what blends are in `ludion docs presentation`
    /// § Frame pacing.
    ///
    /// # Example
    /// ```
    /// use ludion_engine::Engine;
    ///
    /// assert!(Engine::builder().present_interpolation(true).present_interpolation_hint());
    /// ```
    #[must_use]
    pub fn present_interpolation(mut self, on: bool) -> Self {
        self.present_interpolation = on;
        self
    }

    /// Sets whether a windowed host keeps its `Fifo` swapchain at frame latency 1 instead of 2
    /// (default off): up to one refresh less input-to-photon, at the cost of less slack before a
    /// missed refresh. Window only; render-only: the sim, hashes, replays, screenshot, stream and
    /// goldens are unchanged. See `ludion docs presentation` § Frame pacing.
    ///
    /// # Example
    /// ```
    /// use ludion_engine::Engine;
    ///
    /// assert!(Engine::builder().low_latency(true).low_latency_hint());
    /// ```
    #[must_use]
    pub fn low_latency(mut self, on: bool) -> Self {
        self.low_latency = on;
        self
    }

    /// Sets whether a windowed host re-resolves the first-person camera with the mouse motion the
    /// last tick has not consumed yet, every refresh (default off): look reaches the screen at
    /// display rate instead of after the next tick.
    ///
    /// Applies to a single unparented perspective first-person
    /// [`CameraRig`](ludion_camera::CameraRig) camera, only while the window's pointer is locked, and
    /// assumes the game turns every mouse count into look through `add_look_counts`. Mouse only: the
    /// gamepad stick is not latched. With it on, the camera's rotation is not interpolated. Window
    /// only; render-only: the sim, hashes, replays, screenshot, stream and goldens are unchanged.
    /// See `ludion docs presentation` § Late latch.
    ///
    /// # Example
    /// ```
    /// use ludion_engine::Engine;
    ///
    /// assert!(Engine::builder().late_latch(true).late_latch_hint());
    /// ```
    #[must_use]
    pub fn late_latch(mut self, on: bool) -> Self {
        self.late_latch = on;
        self
    }

    /// Sets whether a window host hides an idle cursor ([`CursorMode::AutoHide`] unless set). Read by
    /// every window host; host state, never reaches the world, hash or snapshots.
    ///
    /// # Example
    /// ```
    /// use ludion_engine::{CursorMode, Engine};
    ///
    /// let builder = Engine::builder().cursor(CursorMode::Visible);
    /// assert_eq!(builder.cursor_hint(), CursorMode::Visible);
    /// ```
    #[must_use]
    pub fn cursor(mut self, cursor: CursorMode) -> Self {
        self.cursor = cursor;
        self
    }

    /// Registers game components. The closure receives a [`Registry`] that already carries the
    /// built-in visual components, so registrations land *on top of* them.
    ///
    /// Call it once with several `register` calls, or several times — registrations apply in order at
    /// [`build`](EngineBuilder::build) time, and any error (an `L0005` name/type collision) fails the
    /// build.
    #[must_use]
    pub fn components(
        mut self,
        register: impl FnOnce(&mut Registry) -> Result<(), Diagnostic> + Send + 'static,
    ) -> Self {
        self.registrations.push(Box::new(register));
        self
    }

    /// Appends a system to the schedule. Systems run once per tick in the order added — the same
    /// determinism contract as [`Schedule::add_system`](ludion_ecs::Schedule::add_system).
    ///
    /// The generic `F` bound accepts a closure only as an implementation necessity: it is what lets
    /// this method `#[track_caller]`-capture the call site ([`Location::caller`](std::panic::Location::caller))
    /// and the function's definition path ([`type_name`](std::any::type_name)) into the
    /// engine-internal `SystemSpec` that an editor Systems panel introspects. By
    /// convention every game still passes a bare `fn` item — system state lives in the `World`, never
    /// a captured closure — so `fn_path` reads as an honest module path (e.g.
    /// `vigil::movement::move_player`) and per-tick cost is one indirect call, unchanged.
    #[track_caller]
    #[must_use]
    pub fn system<F>(self, name: &'static str, run: F) -> Self
    where
        F: Fn(&mut ludion_ecs::World, &ludion_ecs::SystemCtx) + Send + Sync + 'static,
    {
        self.push_system(ludion_ecs::Phase::Sim, name, run)
    }

    /// Appends a presentation system: it runs every tick **after** the physics triple and every
    /// [`system`](EngineBuilder::system), may spawn and despawn presentation entities
    /// ([`World::spawn_presentation`](ludion_ecs::World::spawn_presentation)), and may write no sim
    /// state. Registration order among presentation systems is run order. See
    /// `Plan/presentation-state.md`.
    #[track_caller]
    #[must_use]
    pub fn presentation_system<F>(self, name: &'static str, run: F) -> Self
    where
        F: Fn(&mut ludion_ecs::World, &ludion_ecs::SystemCtx) + Send + Sync + 'static,
    {
        self.push_system(ludion_ecs::Phase::Presentation, name, run)
    }

    #[track_caller]
    fn push_system<F>(mut self, phase: ludion_ecs::Phase, name: &'static str, run: F) -> Self
    where
        F: Fn(&mut ludion_ecs::World, &ludion_ecs::SystemCtx) + Send + Sync + 'static,
    {
        self.systems.push(SystemSpec {
            name,
            phase,
            run: std::sync::Arc::new(run),
            fn_path: std::any::type_name::<F>(),
            registered_at: std::panic::Location::caller(),
            declared_queries: Vec::new(),
        });
        self
    }

    /// The registered system names, in the order they will run — the schedule as data, before an
    /// [`Engine`] is built and without a project root to build one against.
    ///
    /// It exists so a game can *test* the ordering contracts its systems depend on ("the input latch
    /// runs first", "the clock runs above every consumer of the value it settles") instead of
    /// stating them in a comment: assert on `position()` of the names in this slice. Physics is not
    /// game-registered — a loaded scene's `physics:` block is the only opt-in, and its triple always
    /// runs after every game system — so this list is game systems only. Presentation systems run
    /// after physics, so they come last: sim systems first, then presentation, each in registration
    /// order.
    #[must_use]
    pub fn system_names(&self) -> Vec<&'static str> {
        let phase = |p| self.systems.iter().filter(move |s| s.phase == p);
        phase(ludion_ecs::Phase::Sim)
            .chain(phase(ludion_ecs::Phase::Presentation))
            .map(|spec| spec.name)
            .collect()
    }

    /// Declares one component query the most-recently-registered system runs — the same tuple syntax
    /// the system body uses. Chain it onto a [`system`](EngineBuilder::system) call, once per distinct
    /// query a multi-query system runs:
    ///
    /// ```
    /// use ludion_engine::Engine;
    /// use serde::{Deserialize, Serialize};
    /// use schemars::JsonSchema;
    ///
    /// #[derive(Serialize, Deserialize, JsonSchema, Clone, Copy)]
    /// struct Loadout { rate: f32 }
    /// #[derive(Serialize, Deserialize, JsonSchema, Clone, Copy)]
    /// struct Health { hp: f32 }
    ///
    /// fn fire(world: &mut ludion_ecs::World, _ctx: &ludion_ecs::SystemCtx) {
    ///     for (_loadout,) in world.query::<(&Loadout,)>() {}
    ///     for (_health,) in world.query::<(&mut Health,)>() {}
    /// }
    ///
    /// let builder = Engine::builder()
    ///     .components(|r| { r.register::<Loadout>("Loadout")?; r.register::<Health>("Health") })
    ///     .system("fire", fire)
    ///         .query::<(&Loadout,)>()
    ///         .query::<(&mut Health,)>();
    /// # let _ = builder;
    /// ```
    ///
    /// Read/write access is captured from the tuple (`&T` = read, `&mut T` = write). The declared
    /// types are resolved to registry component names at [`build`](EngineBuilder::build) and published
    /// by `engine.describe`; a declared type that is not a registered component fails the build with
    /// `L1125`. The query-drift verifier (`ludion_engine::check_query_drift_replay`) compares these
    /// declarations against what a run actually iterated.
    ///
    /// Called before any [`system`](EngineBuilder::system) there is no system to attribute the
    /// query to: the call is recorded and [`build`](EngineBuilder::build) fails with `L1393`
    /// naming the call site. Chain it directly onto the `.system(...)` it describes.
    #[must_use]
    #[track_caller]
    pub fn query<'w, Q: ludion_ecs::Query<'w>>(mut self) -> Self {
        let shape = Q::shape();
        match self.systems.last_mut() {
            Some(spec) => spec.declared_queries.push(shape),
            None => {
                self.orphan_query
                    .get_or_insert(std::panic::Location::caller());
            }
        }
        self
    }

    /// Registers a game data-doc `kind`: every `*.yaml` under the content root whose top-level
    /// `kind:` matches loads, schema-validated, into a [`DocStore<T>`](crate::DocStore) resource at
    /// each world build.
    ///
    /// `T` carries the same bound as a [`Component`](ludion_ecs::Component) — `Serialize +
    /// DeserializeOwned + JsonSchema` — because a doc round-trips through canonical text and publishes
    /// a schema for validation. Unlike a component, a doc kind adds **no** registry entry and no
    /// world-hash surface: the store is a resource a game's systems read (`world.resource::<DocStore<T>>()`),
    /// and its numbers reach the hash only through the components those systems derive from it.
    ///
    /// Register several kinds with repeated calls (use [`doc_singleton`](EngineBuilder::doc_singleton)
    /// for a kind that must hold exactly one doc); a malformed doc fails the build (or `scene.load`)
    /// with an `L1119`/`L1120` diagnostic, exactly as a bad scene does.
    ///
    /// # Example
    /// ```
    /// use ludion_engine::{DocStore, Engine};
    /// use schemars::JsonSchema;
    /// use serde::{Deserialize, Serialize};
    ///
    /// #[derive(Serialize, Deserialize, JsonSchema, Clone)]
    /// struct Weapon {
    ///     damage: f32,
    /// }
    ///
    /// let builder = Engine::builder().doc_kind::<Weapon>("weapon");
    /// # let _ = builder;
    /// ```
    #[must_use]
    pub fn doc_kind<T>(mut self, kind: &str) -> Self
    where
        T: Serialize + DeserializeOwned + JsonSchema + 'static + Send + Sync,
    {
        self.game_doc_kinds = true;
        self.doc_kind_engine_owned::<T>(kind)
    }

    /// Registers a game data-doc kind that must hold **exactly one** doc — a `rules` or `tuning`
    /// file, not a set.
    ///
    /// Everything else is [`doc_kind`](EngineBuilder::doc_kind): the same [`DocStore`](crate::DocStore),
    /// schema check, live reload and recording. In addition the engine counts the kind's docs at
    /// every point the validators run (build, `scene.load`/`restore`/district switch, `--check-docs`,
    /// live `asset.reload`): zero or two-or-more is an `L1315` error. A reload that would add a second
    /// doc is refused and the store keeps its contents and generation. Read the doc with
    /// [`DocStore::single`](crate::DocStore::single).
    ///
    /// # Example
    /// ```
    /// use ludion_engine::Engine;
    /// use schemars::JsonSchema;
    /// use serde::{Deserialize, Serialize};
    ///
    /// #[derive(Serialize, Deserialize, JsonSchema, Clone)]
    /// struct Rules {
    ///     lives: u32,
    /// }
    ///
    /// let builder = Engine::builder().doc_singleton::<Rules>("rules");
    /// # let _ = builder;
    /// ```
    #[must_use]
    pub fn doc_singleton<T>(mut self, kind: &str) -> Self
    where
        T: Serialize + DeserializeOwned + JsonSchema + 'static + Send + Sync,
    {
        self.game_doc_kinds = true;
        self.doc_kinds.push(DocKind::new::<T>(kind, true));
        self
    }

    /// Supplies a data-doc as text, replacing the file doc at `key` or adding one — a test's variant
    /// of a content file without writing it to disk.
    ///
    /// `key` is content-root-relative, like the walk's (`weather/rain.yaml`; `\` is read as `/`); a
    /// key that is absolute, has a `..` segment or does not end in `.yaml` is an `L1119`. The text goes
    /// through the same kind dispatch, schema check and [`validate_docs`](EngineBuilder::validate_docs)
    /// passes as a file doc (`L1119`/`L1120`, and `L1315` for a [`doc_singleton`](EngineBuilder::doc_singleton)
    /// kind), and fails [`build`](EngineBuilder::build) the same way. A shadowed file is not read, so
    /// the text also wins over a malformed file. A second call with the same key replaces the first.
    ///
    /// The doc lives as long as the engine: it is reinstalled on every `scene.load`, restore and
    /// district rebuild, and a live `asset.reload` of its key is ignored. Replacing a singleton's file
    /// keeps its count at one; a new key makes two (`L1315`). A recording armed on the engine carries
    /// it ahead of the first keyframe, so a replay reproduces it. Builder-only: there is no live
    /// setter, and the content root is still walked (use an empty scratch root to keep it unread).
    ///
    /// # Example
    /// ```
    /// use ludion_engine::Engine;
    /// use schemars::JsonSchema;
    /// use serde::{Deserialize, Serialize};
    ///
    /// #[derive(Serialize, Deserialize, JsonSchema, Clone)]
    /// struct Weapon {
    ///     damage: f32,
    /// }
    ///
    /// let builder = Engine::builder()
    ///     .doc_kind::<Weapon>("weapon")
    ///     .doc_text("weapons/crossbow.yaml", "format: 1\nkind: weapon\ndamage: 99\n");
    /// # let _ = builder;
    /// ```
    #[must_use]
    pub fn doc_text(mut self, key: &str, text: &str) -> Self {
        self.doc_texts.insert(key.to_owned(), text.to_owned());
        self
    }

    /// Registers a kind the *engine* owns, without claiming the content root the way
    /// [`doc_kind`](EngineBuilder::doc_kind) does.
    ///
    /// The difference is what an unregistered `kind:` means. A game that registers its own kinds is
    /// the authority on its content root, so a `kind:` it does not know is a typo and fails the boot
    /// (`L1119`). An engine carrying only engine-owned kinds — the built-in engine behind
    /// `ludion serve` and `ludion sim-hash` — hosts anyone's scene and cannot know a game's kinds, so
    /// a foreign `kind:` is skipped with an `L1155` warning instead.
    fn doc_kind_engine_owned<T>(mut self, kind: &str) -> Self
    where
        T: Serialize + DeserializeOwned + JsonSchema + 'static + Send + Sync,
    {
        self.doc_kinds.push(DocKind::new::<T>(kind, false));
        self
    }

    /// Registers a semantic validation pass over the loaded data-docs — the cross-reference rules a
    /// JSON Schema cannot state, because a `district:` or an `enemy:` is a well-typed string and only
    /// the game knows what it must name.
    ///
    /// The closure receives the world *after* every registered [`doc_kind`](EngineBuilder::doc_kind)'s
    /// [`DocStore`](crate::DocStore) is installed, so a rule may read across kinds, and returns one
    /// [`Diagnostic`] per broken reference. Register it more than once to split the rules by kind:
    /// the passes run in registration order and their results concatenate into one report.
    ///
    /// The engine's own [`doc_singleton`](EngineBuilder::doc_singleton) count runs first, at the same
    /// moments, and a game with only singleton kinds needs no validator for it.
    ///
    /// The engine runs the passes at three moments: at build and at every `scene.load`/`restore`/
    /// district switch (an `Error` refuses the load exactly as a malformed doc's `L1119` does; a
    /// `Warning` prints and the start continues), after a live `asset.reload` of a doc (an `Error` the
    /// edit newly causes refuses the reload and the old doc stays live; warnings and errors that were
    /// already there print once through [`Diagnostics`](crate::Diagnostics)), and under a host's
    /// `--check-docs`.
    ///
    /// Report a broken cross-reference under `L1144`
    /// ([`DOC_REFERENCE_UNRESOLVED`](ludion_core::codes::code::DOC_REFERENCE_UNRESOLVED)) with the
    /// game's own message; a rule that is not a cross-reference may carry any registered code. Set
    /// [`with_path`](Diagnostic::with_path) to the document path of the field that failed —
    /// `<kind>/<doc name>/<field>`, e.g. `quest/rescue-the-keeper/district` — which is also the key
    /// the once-per-offender print latch uses.
    ///
    /// # Example
    /// ```
    /// use ludion_core::codes::code;
    /// use ludion_core::diagnostics::Diagnostic;
    /// use ludion_engine::{DocStore, Engine};
    /// # use schemars::JsonSchema;
    /// # use serde::{Deserialize, Serialize};
    /// # #[derive(Serialize, Deserialize, JsonSchema, Clone)]
    /// # struct Quest { district: String }
    /// let builder = Engine::builder()
    ///     .doc_kind::<Quest>("quest")
    ///     .validate_docs(|world| {
    ///         let Some(quests) = world.resource::<DocStore<Quest>>() else {
    ///             return Vec::new();
    ///         };
    ///         quests
    ///             .iter()
    ///             .filter(|(_, quest)| quest.district != "plaza")
    ///             .map(|(name, quest)| {
    ///                 Diagnostic::new(
    ///                     code::DOC_REFERENCE_UNRESOLVED,
    ///                     format!("quest `{name}` names district `{}`, which does not exist", quest.district),
    ///                 )
    ///                 .with_path(format!("quest/{name}/district"))
    ///             })
    ///             .collect()
    ///     });
    /// # let _ = builder;
    /// ```
    #[must_use]
    pub fn validate_docs(
        mut self,
        validate: impl Fn(&ludion_ecs::World) -> Vec<Diagnostic> + Send + Sync + 'static,
    ) -> Self {
        self.doc_validators.push(Box::new(validate));
        self
    }

    /// Opts the game into projectile patterns: registers the engine-owned
    /// [`PatternDoc`](crate::PatternDoc) under `kind: pattern` **and** its validator
    /// ([`validate_pattern_docs`](crate::validate_pattern_docs)) in one call.
    ///
    /// One method rather than two, because the two halves are not independently useful: a game that
    /// loads pattern documents without the validator gets an unknown bullet kind as a volley that
    /// silently fires nothing at runtime instead of an `L1148` at check time. The engine also
    /// resolves every bullet kind's `atlas`/`frame` against the content root (`L1151`/`L1152`).
    ///
    /// Registration is the whole of the opt-in — the two systems
    /// ([`play_patterns`](crate::play_patterns), [`advance_volleys`](crate::advance_volleys)) are
    /// `pub fn`s the game places in its own schedule order, and every hit, side, damage and cancel
    /// rule is game code.
    ///
    /// # Example
    /// ```
    /// use ludion_engine::Engine;
    ///
    /// let builder = Engine::builder().patterns();
    /// # let _ = builder;
    /// ```
    #[must_use]
    pub fn patterns(self) -> Self {
        self.doc_kind_engine_owned::<crate::PatternDoc>(PATTERN_KIND)
            .validate_docs(crate::validate_pattern_docs)
    }

    /// Opts the game into the data-driven action map (`ludion docs input`): registers the
    /// engine-owned `kind: input-actions` document as a singleton, its validator
    /// ([`validate_input_actions`](crate::validate_input_actions)) and a
    /// [`carry_resource`](EngineBuilder::carry_resource) for [`Actions`](ludion_sim::Actions) in one
    /// call.
    ///
    /// One method rather than three, for the [`patterns`](EngineBuilder::patterns) reason: the
    /// parts are not independently useful. Without the validator a bad binding surfaces as a
    /// silently dead action; without the carrier a scene switch drops a hold in progress.
    ///
    /// The engine compiles the one document into an `Actions` resource at every world build. A
    /// scene switch or checkpoint carries it; a load or a restore installs a fresh one, and an
    /// input held across that needs a fresh press. Exactly one `kind: input-actions` file must
    /// exist under the content root (`L1315` otherwise).
    ///
    /// # Example
    /// ```
    /// use ludion_engine::Engine;
    ///
    /// let builder = Engine::builder().input_actions();
    /// # let _ = builder;
    /// ```
    #[must_use]
    pub fn input_actions(mut self) -> Self {
        self.input_actions = true;
        self.doc_kinds
            .push(DocKind::new::<ludion_sim::InputActionsDoc>(
                crate::INPUT_ACTIONS_KIND,
                true,
            ));
        self.validate_docs(crate::validate_input_actions)
            .carry_resource::<ludion_sim::Actions>()
    }

    /// Moves the resource of type `T` from the departing world to the arriving one across a **scene
    /// switch** (`ludion docs scene-transitions`), if the departing world holds one.
    ///
    /// A world rebuild wipes every resource, which is the right default: a resource is derived,
    /// unhashed, un-snapshotted state, and anything the run *is* belongs on a carried entity. But a
    /// host-side latch — which input device the player last touched, a cursor mode, a debug toggle —
    /// is deliberately none of those: it describes the session, not the world, and putting it on an
    /// entity would drag presentation state into the `WorldHash`. This is the seam for that case, and
    /// only that case.
    ///
    /// **Only a switch carries.** A `scene.load` is a fresh session and an `engine.restore` continues
    /// one from a snapshot that never held the resource in the first place, so neither runs the movers
    /// — a carried resource is as absent after a load as it was before the game ever ran. So a carried
    /// resource must be session state a restore may reset: a resource read before its writer in the
    /// tick is cross-frame state, and belongs in a component (`ludion docs systems`).
    ///
    /// Registrations accumulate; one type registered twice moves once (the second mover finds
    /// nothing). The value moves verbatim — no serialization, so `T` needs no derive beyond `Send +
    /// Sync`.
    ///
    /// # Example
    /// ```
    /// use ludion_engine::Engine;
    ///
    /// /// Which control scheme the HUD should name — session state, never world state.
    /// #[derive(Clone, Copy, Default)]
    /// struct LastDevice(u8);
    ///
    /// let builder = Engine::builder().carry_resource::<LastDevice>();
    /// # let _ = builder;
    /// ```
    #[must_use]
    pub fn carry_resource<T: Send + Sync + 'static>(mut self) -> Self {
        self.resource_carriers.push(Box::new(|from, to| {
            if let Some(resource) = from.remove_resource::<T>() {
                to.insert_resource(resource);
            }
        }));
        self
    }

    /// Opts the game into the projectile pool: the engine installs a
    /// [`Projectiles`](crate::Projectiles) resource sized by `config` at every world build. Schedule
    /// [`projectiles_step`](crate::projectiles_step) after `spatial3d_rebuild` to move it. The last
    /// call wins; [`build`](EngineBuilder::build) refuses an invalid config with `L4027`.
    ///
    /// A scene switch gives the arriving district a fresh pool; add
    /// `carry_resource::<Projectiles>()` to keep the departing one instead.
    ///
    /// # Example
    /// ```
    /// use ludion_engine::{Engine, ProjectileConfig, projectiles_step};
    ///
    /// let engine = Engine::builder()
    ///     .projectiles(ProjectileConfig { capacity: 64, ..ProjectileConfig::default() })
    ///     .system("projectiles_step", projectiles_step)
    ///     .build()
    ///     .unwrap();
    /// # let _ = engine;
    /// ```
    #[must_use]
    pub fn projectiles(mut self, config: crate::ProjectileConfig) -> Self {
        self.projectile_config = Some(config);
        self
    }

    /// Opts the game into navigation: the engine installs a [`Nav`](crate::Nav) resource built from
    /// `config` at every world build. Schedule [`nav_update`](crate::nav_update) before the systems
    /// that read the grid. The last call wins; [`build`](EngineBuilder::build) refuses an invalid
    /// config with `L4032`.
    ///
    /// A scene switch gives the arriving district a fresh grid; add `carry_resource::<Nav>()` to keep
    /// the departing one instead.
    ///
    /// # Example
    /// ```
    /// use ludion_engine::{Engine, NavConfig, nav_update};
    ///
    /// let engine = Engine::builder()
    ///     .nav(NavConfig { nx: 32, nz: 32, ..NavConfig::default() })
    ///     .system("nav_update", nav_update)
    ///     .build()
    ///     .unwrap();
    /// # let _ = engine;
    /// ```
    #[must_use]
    pub fn nav(mut self, config: crate::NavConfig) -> Self {
        self.nav_config = Some(config);
        self
    }

    /// Opts into sight: installs `config` and an empty [`PerceptionResults`](crate::PerceptionResults)
    /// at every world build. Schedule [`perception_update`](crate::perception_update) after the
    /// systems that move perceivers and targets. The last call wins; [`build`](EngineBuilder::build)
    /// refuses an invalid config with `L4220`.
    ///
    /// # Example
    /// ```
    /// use ludion_engine::{Engine, PerceptionConfig, perception_update};
    ///
    /// let engine = Engine::builder()
    ///     .perception(PerceptionConfig { classes: vec!["player".into()] })
    ///     .system("perception_update", perception_update)
    ///     .build()
    ///     .unwrap();
    /// # let _ = engine;
    ///
    /// let too_many = PerceptionConfig { classes: (0..33).map(|n| n.to_string()).collect() };
    /// assert!(Engine::builder().perception(too_many).build().is_err());
    /// ```
    #[must_use]
    pub fn perception(mut self, config: crate::PerceptionConfig) -> Self {
        self.perception_config = Some(config);
        self
    }

    /// Sets how the projectile pool is drawn: a table from shot `tag` to a look (billboard streak,
    /// cube or sphere mesh, or hidden), with a fallback for unlisted tags. Shots draw through the
    /// scene pass's existing particle and mesh paths, so no pass is added. Without a table every
    /// shot draws as a small additive white billboard. The last call wins;
    /// [`build`](EngineBuilder::build) refuses an invalid table with `L4028`.
    ///
    /// # Example
    /// ```
    /// use ludion_engine::{Engine, ProjectileConfig, ProjectileVisuals};
    ///
    /// let engine = Engine::builder()
    ///     .projectiles(ProjectileConfig::default())
    ///     .projectile_visuals(ProjectileVisuals::default())
    ///     .build()
    ///     .unwrap();
    /// # let _ = engine;
    /// ```
    #[must_use]
    pub fn projectile_visuals(mut self, visuals: crate::ProjectileVisuals) -> Self {
        self.projectile_visuals = Some(visuals);
        self
    }

    /// Registers the game's one player-profile type `P`: its persistent save state
    /// ([`Profile<P>`](crate::Profile)), loaded from `save/profile.yaml` (or [`profile_path`](EngineBuilder::profile_path))
    /// into a resource at every world build and mutated by the game's systems.
    ///
    /// `P` carries the component bound plus **`Default`** — because a profile *load-or-defaults*: a
    /// first boot (no file) and an unloadable save both start from `P::default()`. Unlike a data-doc,
    /// a profile loads **leniently** (`Plan/phase-16-profile.md`, decision 3): a field the type gained
    /// fills from its default, a field it lost is dropped, and a save that cannot be honored at all is
    /// backed up and defaulted — never a rejected boot, never a destroyed save. The changes surface via
    /// [`Engine::profile_migration`](crate::Engine::profile_migration).
    ///
    /// A game registers at most one profile; the **last** call wins (like [`default_scene`](EngineBuilder::default_scene)).
    /// Registering none leaves the engine profile-free — no `Profile` resource, no save IO.
    ///
    /// # Example
    /// ```
    /// use ludion_engine::{Engine, Profile};
    /// use schemars::JsonSchema;
    /// use serde::{Deserialize, Serialize};
    ///
    /// #[derive(Serialize, Deserialize, JsonSchema, Default)]
    /// struct Save {
    ///     unlocked_crossbow: bool,
    ///     runs: u32,
    /// }
    ///
    /// let builder = Engine::builder().profile::<Save>();
    /// # let _ = builder;
    /// ```
    #[must_use]
    pub fn profile<P>(mut self) -> Self
    where
        P: Serialize + DeserializeOwned + JsonSchema + Default + 'static + Send + Sync,
    {
        self.profile_kind = Some(ProfileKind::new::<P>());
        self
    }

    /// Overrides where the player profile loads from and saves to, relative to the save root (default:
    /// the project root, default path `save/profile.yaml`). The path resolves root-relative like every
    /// asset/doc reference — an absolute path is not the intended surface (the root-relative resolution
    /// is what keeps a save beside its game's content). Only meaningful alongside [`profile`](EngineBuilder::profile).
    #[must_use]
    pub fn profile_path(mut self, rel: impl Into<PathBuf>) -> Self {
        self.profile_path = Some(rel.into());
        self
    }

    /// Records the scene a host loads when no scene is named: `ludion::run` without `--scene`, and
    /// `ludion::launch` or the C ABI, which have no command line. Purely a hosting hint — the engine
    /// core never consults it. A host refuses (`L0038`) a hint that differs from the `default_scene`
    /// in the content root's `ludion.toml`, which `ludion export` and the editor read.
    #[must_use]
    pub fn default_scene(mut self, path: impl Into<PathBuf>) -> Self {
        self.default_scene = Some(path.into());
        self
    }

    /// The default scene recorded by [`default_scene`](EngineBuilder::default_scene), if any — read
    /// by `ludion::run` before it consumes the builder.
    pub fn default_scene_hint(&self) -> Option<&Path> {
        self.default_scene.as_deref()
    }

    /// The title recorded by [`title`](EngineBuilder::title), if any — read by a window host before it
    /// consumes the builder.
    pub fn title_hint(&self) -> Option<&str> {
        self.title.as_deref()
    }

    /// The extent named by [`render_size`](EngineBuilder::render_size), if any — read by a window or
    /// safe-area host before it consumes the builder.
    pub fn render_size_hint(&self) -> Option<(u32, u32)> {
        self.render_size
    }

    /// The default recorded by [`fullscreen`](EngineBuilder::fullscreen).
    pub fn fullscreen_hint(&self) -> bool {
        self.fullscreen
    }

    /// The flag recorded by [`present_interpolation`](EngineBuilder::present_interpolation).
    pub fn present_interpolation_hint(&self) -> bool {
        self.present_interpolation
    }

    /// The flag recorded by [`late_latch`](EngineBuilder::late_latch).
    pub fn late_latch_hint(&self) -> bool {
        self.late_latch
    }

    /// The flag recorded by [`low_latency`](EngineBuilder::low_latency).
    pub fn low_latency_hint(&self) -> bool {
        self.low_latency
    }

    /// The mode recorded by [`cursor`](EngineBuilder::cursor).
    pub fn cursor_hint(&self) -> CursorMode {
        self.cursor
    }

    /// Builds the engine at the configured project root.
    ///
    /// # Errors
    /// Any registration [`Diagnostic`] (an `L0005` collision with a built-in or another game
    /// component), or an `L4018` for a duplicate or reserved system name.
    pub fn build(self) -> Result<Engine, Diagnostic> {
        let root = self.project_root.clone();
        self.build_at(root)
    }

    /// Builds the engine, overriding the project root with `project_root`. This is the seam replay
    /// uses: the registry and systems come from the builder (the game), the root from the caller —
    /// `ludion::run`'s replay modes pass the scene's directory, exactly as hosting resolves it.
    ///
    /// # Errors
    /// Any registration [`Diagnostic`], or an `L4018` for a duplicate or reserved system name.
    pub fn build_at(mut self, project_root: PathBuf) -> Result<Engine, Diagnostic> {
        if let Some(at) = self.orphan_query {
            return Err(Diagnostic::new(
                code::QUERY_DECL_NO_SYSTEM,
                format!(
                    "`EngineBuilder::query::<...>()` at {}:{} was called before any system was registered, so it describes no system",
                    at.file(),
                    at.line()
                ),
            )
            .with_help(
                "chain it onto the `.system(name, fn)` it describes — e.g. \
                 .system(\"fire\", fire).query::<(&Loadout, &Transform)>()",
            ));
        }
        if let Some(config) = &self.projectile_config {
            config.validate()?;
        }
        if let Some(config) = &self.nav_config {
            config.validate()?;
        }
        if let Some(config) = &self.perception_config {
            config.validate()?;
        }
        if let Some(visuals) = &self.projectile_visuals {
            visuals.validate()?;
            visuals.check_assets(&project_root)?;
        }
        let mut registry = builtin_registry();
        registry
            .register_resource_digest::<crate::Projectiles>("projectiles", |p| p.pool().digest())?;
        for register in self.registrations {
            register(&mut registry)?;
        }
        // Resolve every declared query against the assembled registry before the engine exists: a
        // declared component type that is not registered is a build error (`L1125`), naming the system,
        // the query index, the Rust type, and the fix.
        validate_declared_queries(&registry, &self.systems)?;
        validate_system_names(&self.systems)?;
        let profile_rel_path = self
            .profile_path
            .unwrap_or_else(crate::player_profile::default_profile_rel_path);
        let doc_texts = crate::docs::resolve_doc_texts(&self.doc_kinds, self.doc_texts)?;
        let mut engine = Engine::from_parts(
            registry,
            self.systems,
            self.doc_kinds,
            self.profile_kind,
            profile_rel_path,
            project_root,
            self.render_size.unwrap_or(DEFAULT_RENDER_SIZE),
        );
        engine.save_root = self.save_root;
        // Before `install_profile`: the boot load decides on it whether an unloadable save is renamed.
        engine.profile_persistence = self.profile_persistence;
        engine.profile_off_disk = self.profile_off_disk;
        engine.category_gains = self.category_gains;
        engine.audio_distance = self.audio_distance;
        engine.audio_pan = self.audio_pan;
        // Set before the mirror: `install_plane` copies the field onto the world, and every later
        // world (re)build re-runs it.
        engine.plane = self.plane;
        engine.anti_aliasing_default = self.anti_aliasing;
        engine.present_interpolation = self.present_interpolation;
        engine.low_latency = self.low_latency;
        engine.late_latch = self.late_latch;
        engine.install_plane();
        // Set before `install_docs`, which runs them: the passes are engine configuration derived
        // from the game, re-run on every world the engine builds.
        engine.doc_validators = self.doc_validators;
        // Set before `install_docs` too: it decides whether a foreign `kind:` in the content root
        // refuses the boot or is skipped with a warning.
        engine.game_doc_kinds = self.game_doc_kinds;
        // Set before `install_docs`, which merges them over the content-root walk.
        engine.doc_texts = doc_texts;
        engine.resource_carriers = self.resource_carriers;
        engine.projectile_config = self.projectile_config;
        engine.nav_config = self.nav_config;
        engine.perception_config = self.perception_config;
        engine.input_actions = self.input_actions;
        if let Some(visuals) = self.projectile_visuals {
            engine.projectile_draw.visuals = visuals;
        }
        engine.install_projectiles();
        engine.install_nav();
        engine.install_perception();
        // Populate the data-doc stores into the initial (pre-scene-load) world so a game driven
        // before any `scene.load` still sees its docs; a malformed doc — or a broken cross-reference
        // a registered validation pass reports as an error — fails the build here (the collapsed
        // diagnostic carries every one as `related`).
        engine.install_docs().map_err(crate::commands::collapse)?;
        // After `install_docs`: it compiles the action map from the doc store that call fills.
        engine.install_input_actions();
        engine.sweep_profile_debris();
        // Load the registered profile into the same initial world (or default it). Never fails the
        // build — an unloadable save is backed up and defaulted, reported via `profile_migration`.
        engine.install_profile();
        if self.run_save {
            engine.init_run_save();
        }
        if let Some(config) = self.checkpoints {
            config.validate()?;
            engine.init_checkpoints(config);
        }
        if let Some(config) = self.run_recordings.clone() {
            if config.keyframe_every == 0 {
                return Err(Diagnostic::new(
                    code::RUN_RECORDING_CONFIG_INVALID,
                    "run_recordings: `keyframe_every` is 0",
                )
                .with_help("use a keyframe interval of at least 1 tick; the default is 3600"));
            }
            if let Some(refusal) = ludion_core::paths::content_refusal(&config.dir) {
                return Err(Diagnostic::new(
                    code::RUN_RECORDING_CONFIG_INVALID,
                    format!(
                        "run_recordings: `dir` {:?} {}",
                        config.dir,
                        refusal.describe()
                    ),
                )
                .with_help(
                    "use a relative directory below the save root, such as `save/replays`",
                ));
            }
            engine.init_run_recording(config);
        }
        if let Some(config) = self.replay_playback.clone() {
            if let Some(refusal) = ludion_core::paths::content_refusal(&config.dir) {
                return Err(Diagnostic::new(
                    code::REPLAY_PLAYBACK_FAILED,
                    format!(
                        "replay_playback: `dir` {:?} {}",
                        config.dir,
                        refusal.describe()
                    ),
                )
                .with_help(
                    "use a relative directory below the save root, such as `save/replays`",
                ));
            }
            engine.init_replay_playback(config);
        }
        engine.playback_hook = self.playback_controls.take();
        engine.probe_bake_worlds = self.probe_bake_worlds.take();
        engine.set_game_version(self.game_version.clone());
        engine.boot_profile_pending = true;
        if self.observe_load {
            engine.observe_load();
        }
        Ok(engine)
    }

    /// Roots the engine at `scene_path`'s own directory, builds it, and loads the scene — the shared
    /// construction path `ludion serve`, `ludion sim-hash`, and `ludion::run` all start from, so a
    /// scene fails identically wherever it is loaded.
    ///
    /// The path is resolved to an absolute one first (purely lexically — it does not touch the
    /// filesystem), and the engine's project root becomes that file's parent directory. A scene's
    /// relative asset references therefore resolve against the scene file, not the process's current
    /// directory, and a `--record` session's scene reference is cwd-independent.
    ///
    /// A scene that cannot be read is reported *before* the engine is built, because the root the
    /// content walk would sweep is inferred from the very path that turned out to be wrong — the
    /// walk would report whatever it found in a directory nobody meant to name.
    ///
    /// # Errors
    /// `L1015` if `scene_path` cannot be read, a registration [`Diagnostic`], or the collapsed
    /// `scene.load` diagnostic (a scene with several errors chains each through the diagnostic's
    /// `related` list).
    pub fn load_scene_file(mut self, scene_path: &Path) -> Result<Engine, Diagnostic> {
        let resolved = std::path::absolute(scene_path).unwrap_or_else(|_| scene_path.to_path_buf());
        if let Err(error) = std::fs::metadata(&resolved) {
            return Err(Diagnostic::new(
                code::SCENE_IO,
                format!("could not read scene `{}`: {error}", resolved.display()),
            )
            .with_help("check the path (relative to the current directory) and permissions"));
        }
        let project_root = resolved
            .parent()
            .map(Path::to_path_buf)
            .unwrap_or_else(|| PathBuf::from("."));
        self.project_root = project_root;
        let mut engine = self.build()?;
        engine.execute(Command::SceneLoad(SceneLoadParams {
            // The project root is this file's directory, so the wire path is just its name.
            path: resolved
                .file_name()
                .map(|name| name.to_string_lossy().into_owned())
                .unwrap_or_default(),
        }))?;
        Ok(engine)
    }
}

/// Verifies every declared query names registered components, resolving each element's `TypeId`
/// against `registry`. Returns the first `L1125` (build errors are one-at-a-time here, like a
/// registration collision) naming the system, the declared query's index, the offending Rust type,
/// and the fix.
fn validate_declared_queries(
    registry: &Registry,
    systems: &[SystemSpec],
) -> Result<(), Diagnostic> {
    for spec in systems {
        for (index, query) in spec.declared_queries.iter().enumerate() {
            for elem in query {
                if registry.get_by_type_id(elem.type_id).is_none() {
                    return Err(Diagnostic::new(
                        code::QUERY_DECL_UNKNOWN_COMPONENT,
                        format!(
                            "system `{}`, declared query #{index}: component type `{}` is not \
                             registered, so the query cannot be resolved to a component name",
                            spec.name, elem.type_name,
                        ),
                    )
                    .with_help(
                        "register the component (EngineBuilder::components(|r| \
                         r.register::<T>(\"Name\"))) or fix the .query::<...>() declaration to name a \
                         registered component",
                    ));
                }
            }
        }
    }
    Ok(())
}

/// Verifies every system carries a name of its own: the recording, the query-drift check, the
/// profiler and `L4007` all key by name, and a scene's `physics:` block installs systems under
/// [`ludion_physics::WRITE`], [`STEP`](ludion_physics::STEP) and [`MIRROR`](ludion_physics::MIRROR),
/// so a game system may not take those. Returns the first `L4018`, naming both registration sites
/// of a duplicate.
fn validate_system_names(systems: &[SystemSpec]) -> Result<(), Diagnostic> {
    const RESERVED: [&str; 3] = [
        ludion_physics::WRITE,
        ludion_physics::STEP,
        ludion_physics::MIRROR,
    ];
    for spec in systems {
        if RESERVED.contains(&spec.name) {
            return Err(Diagnostic::new(
                code::SYSTEM_NAME_DUPLICATE,
                format!(
                    "the system name `{}` is reserved for the physics systems a scene's `physics:` \
                     block installs, but a system registered at {} takes it",
                    spec.name, spec.registered_at,
                ),
            )
            .with_help("give the system a distinct name"));
        }
    }
    for (index, spec) in systems.iter().enumerate() {
        if let Some(first) = systems[..index]
            .iter()
            .find(|other| other.name == spec.name)
        {
            return Err(Diagnostic::new(
                code::SYSTEM_NAME_DUPLICATE,
                format!(
                    "the system name `{}` is registered twice: at {} and at {}",
                    spec.name, first.registered_at, spec.registered_at,
                ),
            )
            .with_help(
                "give every system a distinct name — recording, profiling and panic reports \
                 key systems by it",
            ));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Engine;

    /// A fresh, empty temp directory unique to this process + `name` (removed first, so a re-run
    /// starts clean).
    fn fresh_dir(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("ludion-save-root-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn no_save_root_falls_back_to_the_project_root() {
        let root = fresh_dir("default");
        let engine = Engine::builder().project_root(&root).build().unwrap();
        assert_eq!(engine.save_root(), root);
    }

    #[test]
    fn an_explicit_save_root_overrides_the_project_root() {
        let root = fresh_dir("project");
        let save = fresh_dir("save");
        let engine = Engine::builder()
            .project_root(&root)
            .save_root(&save)
            .build()
            .unwrap();
        assert_eq!(engine.save_root(), save);
        assert_eq!(engine.project_root(), root);
    }

    /// `system_names` is the schedule as data: registration order, every name, and nothing a
    /// `query` declaration adds — so a game can assert its ordering contracts without a project root.
    #[test]
    fn system_names_are_the_registration_order() {
        fn noop(_world: &mut ludion_ecs::World, _ctx: &ludion_ecs::SystemCtx) {}
        let builder = Engine::builder()
            .system("first", noop)
            .system("second", noop)
            .system("third", noop);
        assert_eq!(builder.system_names(), vec!["first", "second", "third"]);
        assert!(Engine::builder().system_names().is_empty());
    }

    #[test]
    fn a_duplicate_system_name_fails_the_build_naming_both_sites() {
        fn noop(_world: &mut ludion_ecs::World, _ctx: &ludion_ecs::SystemCtx) {}
        let builder = Engine::builder();
        let first = line!() + 1;
        let builder = builder.system("tick", noop);
        let builder = builder.system("tick", noop);
        let error = builder
            .build()
            .err()
            .expect("a duplicate name must fail the build");
        assert_eq!(error.code, code::SYSTEM_NAME_DUPLICATE);
        for line in [first, first + 1] {
            assert!(
                error.message.contains(&format!("builder.rs:{line}:")),
                "both registration sites are named: {}",
                error.message
            );
        }
    }

    #[test]
    fn the_window_hints_default_and_return_what_was_set() {
        let defaults = Engine::builder();
        assert_eq!(defaults.title_hint(), None);
        assert!(!defaults.fullscreen_hint());
        assert!(!defaults.present_interpolation_hint());
        assert!(!defaults.low_latency_hint());
        assert!(Engine::builder().low_latency(true).low_latency_hint());
        assert!(!defaults.late_latch_hint());
        assert!(Engine::builder().late_latch(true).late_latch_hint());
        assert_eq!(defaults.cursor_hint(), CursorMode::AutoHide);

        let set = Engine::builder()
            .title("Game")
            .fullscreen(true)
            .cursor(CursorMode::Visible);
        assert_eq!(set.title_hint(), Some("Game"));
        assert!(set.fullscreen_hint());
        assert_eq!(set.cursor_hint(), CursorMode::Visible);
        let locked = Engine::builder().cursor(CursorMode::Locked);
        assert_eq!(locked.cursor_hint(), CursorMode::Locked);
    }

    #[test]
    fn the_render_size_hint_is_none_until_one_is_named() {
        assert_eq!(Engine::builder().render_size_hint(), None);
        assert_eq!(
            Engine::builder().render_size(960, 540).render_size_hint(),
            Some((960, 540))
        );
    }

    #[test]
    fn a_reserved_physics_name_fails_the_build() {
        fn noop(_world: &mut ludion_ecs::World, _ctx: &ludion_ecs::SystemCtx) {}
        let error = Engine::builder()
            .system(ludion_physics::STEP, noop)
            .build()
            .err()
            .expect("a reserved name must fail the build");
        assert_eq!(error.code, code::SYSTEM_NAME_DUPLICATE);
        assert!(error.message.contains("reserved"), "{}", error.message);
    }

    #[test]
    fn a_presentation_system_runs_after_every_sim_system_and_physics() {
        fn noop(_world: &mut ludion_ecs::World, _ctx: &ludion_ecs::SystemCtx) {}
        let root = fresh_dir("presentation-order");
        let engine = Engine::builder()
            .project_root(&root)
            .presentation_system("popups", noop)
            .system("score", noop)
            .build()
            .unwrap();
        let physics = ludion_scene::ScenePhysics {
            space: ludion_scene::ScenePhysicsSpace::Free,
            gravity: None,
            solver_iterations: None,
            stabilization_iterations: None,
            sleep_after_ticks: None,
            layers: Vec::new(),
        };
        let schedule = engine.build_schedule(Some(&physics));
        let order: Vec<&str> = schedule.system_names().collect();
        let at = |name: &str| order.iter().position(|n| *n == name).unwrap();
        assert!(at("score") < at(ludion_physics::STEP), "{order:?}");
        assert!(at(ludion_physics::STEP) < at("popups"), "{order:?}");
        assert!(
            schedule
                .system_phases()
                .any(|(n, p)| n == "popups" && p == ludion_ecs::Phase::Presentation),
            "{order:?}"
        );
    }

    #[test]
    fn distinct_system_names_build() {
        fn noop(_world: &mut ludion_ecs::World, _ctx: &ludion_ecs::SystemCtx) {}
        let root = fresh_dir("distinct-names");
        Engine::builder()
            .project_root(&root)
            .system("first", noop)
            .system("second", noop)
            .build()
            .unwrap();
    }

    /// The audio placement a game authors reaches the built engine; unauthored, it is the centred,
    /// planar stream every existing game has.
    #[test]
    fn the_authored_audio_placement_rides_the_built_engine() {
        let root = fresh_dir("placement");
        let plain = Engine::builder().project_root(&root).build().unwrap();
        assert_eq!(plain.audio_pan(), AudioPan::Centred);
        assert_eq!(plain.audio_distance(), AudioDistance::Planar);

        let engine = Engine::builder()
            .project_root(&root)
            .audio_pan(AudioPan::Stereo)
            .audio_distance(AudioDistance::Spatial)
            .build()
            .unwrap();
        assert_eq!(engine.audio_pan(), AudioPan::Stereo);
        assert_eq!(engine.audio_distance(), AudioDistance::Spatial);
    }

    /// The mixdown a game authors reaches the built engine, where every host reads it; unauthored,
    /// it is the identity, so a game that says nothing hears the gains its systems asked for.
    #[test]
    fn the_authored_mixdown_rides_the_built_engine() {
        let root = fresh_dir("gains");
        assert_eq!(
            Engine::builder()
                .project_root(&root)
                .build()
                .unwrap()
                .category_gains(),
            CategoryGains::default()
        );

        let gains = CategoryGains {
            sfx: 1.0,
            ambient: 0.6,
            music: 0.8,
        };
        let engine = Engine::builder()
            .project_root(&root)
            .category_gains(gains)
            .build()
            .unwrap();
        assert_eq!(engine.category_gains(), gains);
    }

    /// Several registered passes run in registration order and their results concatenate into one
    /// report — the property that lets a game split its rules by doc kind.
    #[test]
    fn registered_validators_run_in_order_and_concatenate() {
        let root = fresh_dir("validators-order");
        let lint = |tag: &'static str| {
            move |_world: &ludion_ecs::World| {
                vec![
                    Diagnostic::new(code::MESH_VERTEX_BUDGET_EXCEEDED, format!("{tag}-1")),
                    Diagnostic::new(code::MESH_VERTEX_BUDGET_EXCEEDED, format!("{tag}-2")),
                ]
            }
        };
        let engine = Engine::builder()
            .project_root(&root)
            .validate_docs(lint("first"))
            .validate_docs(lint("second"))
            .build()
            .unwrap();
        let messages: Vec<&str> = engine
            .doc_validation_warnings()
            .iter()
            .map(|d| d.message.as_str())
            .collect();
        assert_eq!(messages, ["first-1", "first-2", "second-1", "second-2"]);
    }

    /// An `Error` a pass reports refuses the build exactly as a malformed doc's `L1119` does, and
    /// nothing is left standing in the warnings.
    #[test]
    fn a_validator_error_refuses_the_build() {
        let root = fresh_dir("validators-error");
        // `Engine` is not `Debug`, so `expect_err` cannot be used here.
        let err = match Engine::builder()
            .project_root(&root)
            .validate_docs(|_world| {
                vec![
                    Diagnostic::new(
                        code::DOC_REFERENCE_UNRESOLVED,
                        "quest names a dead district",
                    )
                    .with_path("quest/rescue/district"),
                ]
            })
            .build()
        {
            Ok(_) => panic!("the validator reported an error"),
            Err(diagnostic) => diagnostic,
        };
        let rendered = err.to_string();
        assert!(rendered.contains("L1144"), "{rendered}");
        assert!(rendered.contains("quest/rescue/district"), "{rendered}");
    }

    /// A live doc reload re-runs the passes: an `Error` they newly report refuses the reload with the
    /// old doc live and nothing logged, while a `Warning` still applies and prints once. The path is
    /// the report key.
    #[test]
    fn a_doc_reload_a_validator_rejects_is_refused_and_a_warning_still_applies() {
        use serde::Deserialize;

        #[derive(Serialize, Deserialize, JsonSchema, Clone)]
        struct Weapon {
            damage: f32,
        }

        let root = fresh_dir("validators-reload");
        std::fs::create_dir_all(root.join("weapons")).unwrap();
        let file = root.join("weapons/crossbow.yaml");
        std::fs::write(
            &file,
            "format: 1
kind: weapon
damage: 12.0
",
        )
        .unwrap();

        // Clean at boot (damage is in range), broken only once the file is edited below.
        let mut engine = Engine::builder()
            .project_root(&root)
            .doc_kind::<Weapon>("weapon")
            .validate_docs(|world: &ludion_ecs::World| {
                let Some(weapons) = world.resource::<crate::DocStore<Weapon>>() else {
                    return Vec::new();
                };
                weapons
                    .iter()
                    .filter(|(_, weapon)| weapon.damage > 100.0)
                    .map(|(name, _)| {
                        Diagnostic::new(
                            code::DOC_REFERENCE_UNRESOLVED,
                            format!("weapon `{name}` is out of range"),
                        )
                        .with_path(format!("weapon/{name}/damage"))
                        .with_help("lower the damage")
                    })
                    .collect()
            })
            .validate_docs(|world: &ludion_ecs::World| {
                let Some(weapons) = world.resource::<crate::DocStore<Weapon>>() else {
                    return Vec::new();
                };
                weapons
                    .iter()
                    .filter(|(_, weapon)| weapon.damage > 50.0)
                    .map(|(name, _)| {
                        Diagnostic::new(
                            code::MESH_VERTEX_BUDGET_EXCEEDED,
                            format!("weapon `{name}` is strong"),
                        )
                        .with_path(format!("weapon/{name}/damage"))
                        .with_help("lower the damage")
                    })
                    .collect()
            })
            .build()
            .unwrap();
        assert!(engine.doc_validation_warnings().is_empty());
        let stored = |engine: &Engine| {
            let store = engine
                .world()
                .resource::<crate::DocStore<Weapon>>()
                .unwrap();
            (
                store.get("weapons/crossbow.yaml").unwrap().damage,
                store.generation(),
            )
        };
        let logs = |engine: &mut Engine| -> Vec<ludion_protocol::event::LogEvent> {
            engine.advance_live();
            engine
                .take_events()
                .into_iter()
                .filter_map(|event| match event {
                    ludion_protocol::event::Event::Log(log) if log.diagnostic.is_some() => {
                        Some(log)
                    }
                    _ => None,
                })
                .collect()
        };
        let (_, generation) = stored(&engine);

        std::fs::write(
            &file,
            "format: 1
kind: weapon
damage: 999.0
",
        )
        .unwrap();
        let refused = engine
            .reload_doc_from_disk("weapons/crossbow.yaml")
            .expect_err("a reload that makes a validator report an error is refused");
        let rendered = refused.to_string();
        assert!(rendered.contains("L1144"), "{rendered}");
        assert!(
            rendered.contains("weapon/weapons/crossbow.yaml/damage"),
            "{rendered}"
        );
        assert_eq!(stored(&engine), (12.0, generation), "the store moved");
        assert!(logs(&mut engine).is_empty());

        std::fs::write(
            &file,
            "format: 1
kind: weapon
damage: 60.0
",
        )
        .unwrap();
        let applied = engine
            .reload_doc_from_disk("weapons/crossbow.yaml")
            .expect("a warning does not refuse the reload");
        assert!(applied.is_some());
        assert_eq!(stored(&engine), (60.0, generation + 1));
        let logs = logs(&mut engine);
        assert_eq!(logs.len(), 1, "{logs:?}");
        let diagnostic = logs[0].diagnostic.as_ref().unwrap();
        assert_eq!(diagnostic.code, code::MESH_VERTEX_BUDGET_EXCEEDED);
        assert!(diagnostic.message.contains("crossbow"), "{logs:?}");
        assert_eq!(
            diagnostic.path.as_deref(),
            Some("weapon/weapons/crossbow.yaml/damage")
        );
        assert_eq!(diagnostic.help.as_deref(), Some("lower the damage"));
        assert_eq!(logs[0].level, ludion_protocol::event::LogLevel::Warn);
        assert!(
            logs[0].message.contains("help: lower the damage"),
            "{logs:?}"
        );
    }

    /// A live reload that would leave a `doc_singleton` kind with two docs is refused by the engine's
    /// count alone (no `validate_docs`), with the store and generation untouched and nothing logged.
    #[test]
    fn a_reload_that_adds_a_second_singleton_doc_is_refused() {
        use serde::Deserialize;

        #[derive(Serialize, Deserialize, JsonSchema, Clone)]
        struct Weapon {
            damage: f32,
        }

        let root = fresh_dir("singleton-reload");
        std::fs::create_dir_all(root.join("weapons")).unwrap();
        let a = root.join("weapons/a.yaml");
        std::fs::write(&a, "format: 1\nkind: weapon\ndamage: 12.0\n").unwrap();

        let mut engine = Engine::builder()
            .project_root(&root)
            .doc_singleton::<Weapon>("weapon")
            .build()
            .unwrap();
        let stored = |engine: &Engine| {
            let store = engine
                .world()
                .resource::<crate::DocStore<Weapon>>()
                .unwrap();
            (
                store
                    .iter()
                    .map(|(k, w)| (k.clone(), w.damage))
                    .collect::<Vec<_>>(),
                store.generation(),
            )
        };
        let (_, generation) = stored(&engine);

        std::fs::write(
            root.join("weapons/b.yaml"),
            "format: 1\nkind: weapon\ndamage: 5.0\n",
        )
        .unwrap();
        let refused = engine
            .reload_doc_from_disk("weapons/b.yaml")
            .expect_err("a second singleton doc is refused");
        let rendered = refused.to_string();
        assert!(rendered.contains("L1315"), "{rendered}");
        assert_eq!(
            stored(&engine),
            (vec![("weapons/a.yaml".to_string(), 12.0)], generation),
            "the store moved"
        );
        engine.advance_live();
        let logged = engine
            .take_events()
            .into_iter()
            .any(|event| matches!(event, ludion_protocol::event::Event::Log(_)));
        assert!(!logged, "a refused reload logged");

        std::fs::write(&a, "format: 1\nkind: weapon\ndamage: 60.0\n").unwrap();
        engine
            .reload_doc_from_disk("weapons/a.yaml")
            .expect("editing the one doc applies");
        assert_eq!(
            stored(&engine),
            (vec![("weapons/a.yaml".to_string(), 60.0)], generation + 1)
        );
    }

    /// A registered resource crosses a **scene switch** and is gone after a `scene.load` — the two
    /// halves of `carry_resource`'s contract, on one engine so the second cannot pass by accident.
    #[test]
    fn a_registered_resource_crosses_a_switch_and_not_a_load() {
        /// Session state a game would otherwise lose at every crossing.
        #[derive(Clone, Copy, PartialEq, Eq, Debug)]
        struct LastDevice(u8);

        /// Frame 0 latches the resource; frame 2 asks for the switch (nothing is carried — the
        /// resource is the whole subject).
        fn latch_then_switch(world: &mut ludion_ecs::World, ctx: &ludion_ecs::SystemCtx) {
            match ctx.frame.as_u64() {
                0 => {
                    world.insert_resource(LastDevice(7));
                }
                2 => {
                    if let Some(switch) = world.resource_mut::<crate::SceneSwitch>() {
                        switch.request("next.yaml".to_string(), "anchor".to_string(), Vec::new());
                    }
                }
                _ => {}
            }
        }

        const EMPTY: &str = "format: 1\nentities: []\n";

        let root = fresh_dir("carry-resource");
        std::fs::write(
            root.join("next.yaml"),
            "format: 1\nentities:\n  - name: anchor\n    components:\n      Transform:\n        \
             translation:\n          - 1.0\n          - 0.0\n          - 2.0\n",
        )
        .unwrap();
        let mut engine = Engine::builder()
            .project_root(&root)
            .carry_resource::<LastDevice>()
            .system("latch_then_switch", latch_then_switch)
            .build()
            .unwrap();
        engine.load_inline_scene(EMPTY).unwrap();

        engine.advance_live(); // frame 0 — latched
        assert_eq!(
            engine.world().resource::<LastDevice>(),
            Some(&LastDevice(7))
        );
        engine.advance_live(); // frame 1
        engine.advance_live(); // frame 2 — the switch is performed at the boundary
        assert!(
            engine.last_transition_error().is_none(),
            "the switch is clean: {:?}",
            engine.last_transition_error()
        );
        assert_eq!(
            engine.world().resource::<LastDevice>(),
            Some(&LastDevice(7)),
            "a registered resource moves into the district the switch built"
        );

        engine.load_inline_scene(EMPTY).unwrap();
        assert_eq!(
            engine.world().resource::<LastDevice>(),
            None,
            "a scene.load is a fresh session, so it carries nothing"
        );
    }

    #[test]
    fn load_scene_file_overrides_project_root_but_not_save_root() {
        let root = fresh_dir("scene");
        let save = fresh_dir("scene-save");
        let scene_path = root.join("main.yaml");
        std::fs::write(&scene_path, "format: 1\nentities: []\n").unwrap();
        let engine = Engine::builder()
            .save_root(&save)
            .load_scene_file(&scene_path)
            .unwrap();
        assert_eq!(engine.project_root(), root);
        assert_eq!(engine.save_root(), save);
    }
}
