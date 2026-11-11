//! [`Engine`] — the single owner of the running world and the state every command acts on.
//!
//! The engine holds the [`SimDriver`] (which owns the [`World`] by value — no `Arc<Mutex<_>>`, ever),
//! the component [`Registry`], the current scene reference and seed, a monotonically increasing frame
//! (via the driver), the subscription table, an optional recording sink, and a lazily-initialized GPU
//! [`Renderer`] for screenshots. Commands reach it through [`Engine::execute`] (which records
//! mutating commands into the active session) or the pure [`dispatch`](crate::dispatch) parity gate.
//! The engine itself is single-threaded; the [`live`](crate::live) module wraps it in a sim-thread
//! loop that a transport (the server) feeds over channels, without the engine ever knowing about
//! tokio or WebSockets.
//!
//! # Example
//! ```
//! use ludion_engine::Engine;
//! use ludion_protocol::Command;
//! use ludion_protocol::command::EntitySpawnParams;
//! use std::collections::BTreeMap;
//!
//! let mut engine = Engine::new();
//! engine.load_inline_scene("format: 1\nentities: []\n").unwrap();
//!
//! let mut components = BTreeMap::new();
//! components.insert("Transform".to_string(), serde_json::json!({ "translation": [1.0, 2.0, 3.0] }));
//! let dispatched = engine
//!     .execute(Command::EntitySpawn(EntitySpawnParams { name: None, components, presentation: false }))
//!     .unwrap();
//! assert_eq!(engine.world().entity_count(), 1);
//! assert_eq!(dispatched.frame.as_u64(), 0);
//! ```

use ludion_assets::AssetRoot;
use std::collections::{BTreeMap, VecDeque};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use ludion_core::codes::code;
use ludion_core::collections::{StableMap, StableSet};
use ludion_core::diagnostics::Diagnostic;
use ludion_core::ids::EntityId;
use ludion_core::rng::{DeterministicRng, WorldSeed};
use ludion_core::time::{FrameIndex, GameClock, TickRate};
use ludion_ecs::{ComponentInfo, QueryElemShape, QueryObservations, Registry, Schedule, World};
use ludion_protocol::command::{CullFrom, ViewPose};
use ludion_protocol::envelope::Seq;
use ludion_protocol::event::{Event, LogEvent, SceneChangeKind, SceneChangedData};
use ludion_protocol::profile::ProfileMigration;
use ludion_protocol::replay::{
    REPLAY_FORMAT_VERSION, ReplayExtent, ReplayHeader, ReplayWriter, SceneRef,
};
use ludion_protocol::response::EntityBounds;
use ludion_protocol::{Channel, PROTOCOL_VERSION, RequestId, SubId, schema};
use ludion_renderer::{
    AntiAliasing, BlendData, FrameSinks, FrameSize, Gpu, PresentBackend, PresentPacket,
    PresentSettings, PresentSize, PresentTarget, Presenter, PresenterNote, ProbeResolve,
    ProbeWorld, RenderScale, RenderScene, RenderedImage, Renderer, StreamReadback, TickStamp,
    WindowBackend,
};
use ludion_scene::{ExpandedScene, InstanceTransform, Scene, SceneEntity, ScenePhysics};
use ludion_sim::{InputState, RunState, SimConfig, SimDriver, Speed, WorldHash};
use ludion_spatial::Plane;
use serde_json::Value;

use crate::asset_cache::AssetCache;
use crate::audio::{
    AudioDistance, AudioListener, AudioLogEntry, AudioMessage, AudioPan, AudioQueue, CategoryGains,
    LoopId, Queued, SentLoop, audible_voice,
};
use crate::builtin_registry;
use crate::components::{AudioEmitter, Camera, Name, ReflectionProbe, Transform};
use crate::cooked_maps::TextureSource;
use crate::dispatch::Dispatched;
use crate::docs::{DocKind, DocValidator, PreparedDocs};
use crate::load_observer::{FrameLoadObserver, LoadObserver, LoadPhase};
use crate::perf_live::{LivePerf, ProfileWindowData, Span};
use crate::player_profile::{ProfileKind, ProfileSaveEntry};
use crate::probe_bake::{ProbeBake, ProbeBakeReport, ProbeOutcome, ProbeSource};
use crate::quality::QualitySetting;
use crate::recording::{AppliedDocReload, Recorder};
use crate::replay_playback::{
    ActivePlayback, PlaybackControls, PlaybackHook, PlaybackInput, PlaybackOutcome, PlaybackPlan,
    PlaybackSpeed, ReplayEntry, ReplayPlayback, ReplayPlaybackState, SavedHost,
};
use crate::run_recording::{
    ActiveRun, RUN_RECORDING_LOG_CAPACITY, RunRecording, RunRecordingEntry, RunRecordingRequest,
    RunRecordingState,
};
use crate::run_save::{
    ResumeOutcome, RunSave, RunSaveEntry, RunSaveRequest, RunSaveState, SuspendInfo,
};
use crate::scene_physics;
use crate::tick_observer::TickObserver;

/// This engine build's version string (used in `engine.describe`, snapshots, and replay headers).
pub const ENGINE_VERSION: &str = env!("CARGO_PKG_VERSION");

/// The default screenshot resolution when a `render.screenshot` omits width/height, and the extent
/// [`RenderExtent::default`](crate::RenderExtent::default) reports.
pub(crate) const DEFAULT_RENDER_SIZE: (u32, u32) = (1280, 720);

/// Whether a host lets the global-illumination caches follow the world, or holds the baseline they
/// already have ([`Engine::set_gi_rebake`]).
///
/// The two caches behind GI — the CPU voxelization (keyed on every *unmarked* instance's transform,
/// colour and emissive) and the GPU probe bake (keyed on the grid hash and the unmarked light set) —
/// are built to be load-time work. A live world whose movers carry no `GiDynamic` marker busts both
/// keys every frame, so enabling GI on such a scene from an editor costs a full revoxelize plus a
/// full bake per frame. [`Hold`](GiRebakeMode::Hold) is the session-level answer: keep serving the
/// retained grid and volumes, and report honestly that what is on screen is stale.
///
/// **Host/session state, never world state.** It is not hashed into a `WorldHash`, not captured by
/// `engine.snapshot`, and not recorded into a replay, so a held session simulates and records exactly
/// as an unheld one does. Every one-shot path (`ludion render`, goldens, replays, `sim-hash`,
/// `ludion test`) constructs its engine at the default and has no spelling of the lever at all.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum GiRebakeMode {
    /// Both caches follow the world: a changed input rebuilds. Today's behaviour, and the default —
    /// nothing about a render at `Auto` differs from a render before the lever existed.
    #[default]
    Auto,
    /// Both caches hold what they have. A key miss serves the retained grid / volumes and reports
    /// staleness ([`Engine::gi_stale`]) instead of rebuilding. A *first* build is never held: there
    /// is nothing to serve, and an unlit frame is a worse answer than a stale one.
    Hold,
}

/// How often a recording session writes a keyframe snapshot, in frames (it also writes one at the
/// start and one at the stop).
///
/// 300 frames is five seconds at the default 60 Hz tick rate — a seek granularity that keeps the log
/// small (risk register #9) while bounding how many deltas a seek must replay after the nearest
/// keyframe.
pub const KEYFRAME_INTERVAL: u64 = 300;

/// One registered game system: its name, its runnable body, and the metadata an editor
/// Systems panel introspects — where the function is *defined* (`fn_path`) and where it was *wired*
/// into the schedule (`registered_at`). Run in registration order; the [`Engine`] rebuilds a fresh
/// [`Schedule`] from the list on every world reset (see [`Engine::build_schedule`]), which is what
/// lets a game's schedule *survive* `scene.load` and `engine.restore` instead of being silently
/// dropped — hence the [`Clone`], so cloning a spec into the schedule is a refcount bump.
///
/// The body is an `Arc<dyn Fn…>` rather than a `fn` pointer because [`EngineBuilder::system`] accepts
/// a generic `F` (so it can `#[track_caller]`-capture the two metadata fields), and a generic cannot
/// be coerced to a bare `fn` without `unsafe`, which the workspace forbids. Systems stay plain `fn`s
/// by convention all the same — state lives in the `World`, not a captured closure — so per-tick cost
/// is one indirect call, exactly as before.
#[derive(Clone)]
pub(crate) struct SystemSpec {
    /// The schedule name, surfaced by the profiler and `engine.describe`.
    pub(crate) name: &'static str,
    /// Whether the system is part of the sim or runs after physics against presentation state.
    pub(crate) phase: ludion_ecs::Phase,
    /// The runnable body, cloned (a refcount bump) into each rebuilt [`Schedule`].
    pub(crate) run: ludion_ecs::SystemFn,
    /// The definition path of the registered function (`std::any::type_name` of `F`) — for a bare
    /// `fn` item, its module path, e.g. `vigil::movement::move_player`. The honest "where is this
    /// defined" signal, read at build time by [`schedule_module_warnings`] to group systems by their
    /// definition module (the `L1124` schedule-concentration warning), and exposed by `engine.describe`
    /// as each system's `fn_path` (a Rust module path — no filesystem leak — that a jump-to-source
    /// consumer needs).
    pub(crate) fn_path: &'static str,
    /// The call site that wired this system into the builder — the `.system(…)` line.
    pub(crate) registered_at: &'static std::panic::Location<'static>,
    /// The component queries this system *declares* it runs — each a tuple's [`QueryElemShape`] list,
    /// in declaration order — collected from the `.query::<...>()` calls chained after `.system(...)`.
    /// Empty for a system that declares none (its `engine.describe` `queries` is `[]`). Element types
    /// are held as `TypeId`s here and resolved to registry names at describe time; every declared type
    /// is verified to be registered at build (`L1125`).
    pub(crate) declared_queries: Vec<Vec<QueryElemShape>>,
}

/// The single owner of the running world and every piece of state a command acts on.
///
/// See the [crate-level docs](crate) for the ownership model.
pub struct Engine {
    driver: SimDriver,
    /// Shared with the world's [`PrefabLibrary`](crate::prefab::PrefabLibrary), which validates
    /// prefab components from inside a system.
    registry: Arc<Registry>,
    /// The game systems, in registration order. Empty for a bare engine (the default). Held so the
    /// [`Schedule`] can be rebuilt from scratch after every world reset (`scene.load`,
    /// `engine.restore`) — each [`SystemSpec`] is cheaply [`Clone`] (an `Arc` refcount bump), so this
    /// is the schedule's persistent source of truth, not a value that gets consumed on the first load.
    systems: Vec<SystemSpec>,
    /// The loaded scene's `physics:` block, re-read by every world rebuild. `None` for a bare
    /// engine or a scene that never declared one — the only opt-in there is.
    ///
    /// The physics triple always runs **after** every game system: with no builder verb to pick a
    /// position, a system that writes `Velocity` or teleports a body this tick is integrated this
    /// tick, and one that reads a resolved pose sees last tick's, the usual fixed-step shape. Held
    /// on the engine and re-read by [`build_schedule`](Engine::build_schedule) so it survives every
    /// world reset — construction, `scene.load`, `engine.restore`, `scene.switch` — the way the
    /// game's own registration order does.
    scene_physics: Option<ScenePhysics>,
    /// The registered game data-doc kinds, in registration order. Empty for a bare engine. Held (like
    /// [`systems`](Engine::systems)) so every world reset — construction, `scene.load`,
    /// `engine.restore` — can rebuild each kind's `DocStore` resource from the content root, rather
    /// than losing it to the `SimDriver` resource wipe. Empty means [`install_docs`](Engine::install_docs)
    /// does nothing (no walk, no IO).
    doc_kinds: Vec<DocKind>,
    /// The game's semantic doc-validation passes, in registration order
    /// ([`EngineBuilder::validate_docs`](crate::EngineBuilder::validate_docs)). Held beside
    /// [`doc_kinds`](Engine::doc_kinds) and re-run wherever those stores are rebuilt, so the rules a
    /// game states about its own content hold on every world the engine builds, not only the first.
    pub(crate) doc_validators: Vec<DocValidator>,
    /// Whether a *game* registered any of [`doc_kinds`](Engine::doc_kinds) through
    /// [`EngineBuilder::doc_kind`](crate::EngineBuilder::doc_kind), rather than the list holding only
    /// engine-owned kinds. False for the built-in engine `ludion serve`/`ludion sim-hash` builds,
    /// which hosts anyone's scene and so skips a foreign `kind:` (an `L1155` in
    /// [`doc_skip_warnings`](Engine::doc_skip_warnings)) where a game's engine fails the boot on it.
    pub(crate) game_doc_kinds: bool,
    /// The docs the builder holds as text ([`EngineBuilder::doc_text`](crate::EngineBuilder::doc_text)),
    /// keyed like the content-root walk. Merged over the walk by every
    /// [`prepare_docs`](crate::docs::prepare_docs), so they outlive `scene.load`, restore and district
    /// rebuilds; a disk reload of their key is ignored.
    pub(crate) doc_texts: BTreeMap<String, crate::docs::InlineDoc>,
    /// The game's registered resource movers, in registration order
    /// ([`EngineBuilder::carry_resource`](crate::EngineBuilder::carry_resource)). Run by
    /// [`commit_world`](Engine::commit_world) on a switch alone — a scene switch is the one world rebuild
    /// that continues a session, so it is the one rebuild a resource may cross. Empty for a game that
    /// registered none, which is every game until one asks.
    pub(crate) resource_carriers: Vec<crate::builder::ResourceCarrier>,
    /// The game's projectile pool config ([`EngineBuilder::projectiles`](crate::EngineBuilder::projectiles)),
    /// already validated at build; `None` installs no pool.
    pub(crate) projectile_config: Option<crate::ProjectileConfig>,
    /// The game's nav config ([`EngineBuilder::nav`](crate::EngineBuilder::nav)), already validated at
    /// build; `None` installs no grid.
    pub(crate) nav_config: Option<crate::NavConfig>,
    /// How the pool is drawn ([`EngineBuilder::projectile_visuals`](crate::EngineBuilder::projectile_visuals)).
    pub(crate) projectile_draw: crate::projectile_draw::ProjectileDraw,
    /// What the last install-time validation pass reported at `Warning` severity — the errors
    /// refused the load, so only lints survive to be read. Host/test state like
    /// [`profile_migration`](Engine::profile_migration): never hashed, never snapshotted. See
    /// [`doc_validation_warnings`](Engine::doc_validation_warnings).
    doc_validation_warnings: Vec<Diagnostic>,
    /// What the last content-root walk skipped for declaring a `kind:` this engine does not register
    /// (`L1155`), in walk order. Always empty once a game registered a kind of its own — that engine
    /// fails the boot on a foreign kind instead. Host/test state like
    /// [`doc_validation_warnings`](Engine::doc_validation_warnings): never hashed, never snapshotted.
    doc_skip_warnings: Vec<Diagnostic>,
    /// The one `L1155` that says what [`doc_skip_warnings`](Engine::doc_skip_warnings) says, in one
    /// line — what a host prints. `None` when the walk skipped nothing.
    doc_skip_summary: Option<Diagnostic>,
    /// The one registered player-profile kind, if the game registered a `Profile` type. Held (like
    /// [`doc_kinds`](Engine::doc_kinds)) so every world reset — construction, `scene.load`,
    /// `engine.restore` — reinstalls the `Profile<P>` resource from the persistent registration
    /// rather than losing it to the `SimDriver` resource wipe. `None` means [`install_profile`](Engine::install_profile)
    /// does nothing (no IO). The singleton analog of `doc_kinds`.
    profile_kind: Option<ProfileKind>,
    /// The save-root-relative path the profile loads from and saves to (default `save/profile.yaml`,
    /// overridable via `EngineBuilder::profile_path`). Resolved against [`save_root`](Engine::save_root)
    /// at each `install_profile`.
    profile_rel_path: PathBuf,
    /// The last profile load's lenient-migration report — filled/dropped fields, or a backup-and-reset
    /// (`L1121`). Empty (clean) with no registered profile or a losslessly loaded one. Host/test state,
    /// never in the `WorldHash` or a snapshot. See [`profile_migration`](Engine::profile_migration).
    profile_migration: ProfileMigration,
    seed: u64,
    scene: Option<SceneRef>,
    /// `scene`'s inline document: a load seeds it from its checked parse, a restore or switch
    /// parses on first use.
    authored: std::sync::OnceLock<Option<Scene>>,
    /// The loaded scene's `name:` field, retained so `scene.save` writes it back (authoring
    /// metadata; not world content, so it never enters the world hash). `None` for a
    /// programmatically-built world or a scene that declared no name.
    scene_name: Option<String>,
    project_root: PathBuf,
    /// The text this session last loaded or saved at each scene path (keyed by `base_key`):
    /// `scene.save`'s conflict base. Outside the world, the snapshot and `WorldHash`; a dev restart
    /// carries it in the scene-session sidecar.
    scene_bases: BTreeMap<PathBuf, String>,
    /// The project-root-relative path the current world was loaded or switched from; `None` before
    /// the first load or switch, and after `engine.restore` or `load_inline_scene` (the world no
    /// longer comes from a file). What a restart's `scene.changed` names.
    current_scene_path: Option<String>,
    /// Where `engine.snapshot` writes the scene-session sidecar; `None` writes none.
    scene_session_file: Option<PathBuf>,
    /// The frame, scene identity and scene path the last snapshot captured: a later `scene.save`
    /// rewrites only the sidecar's bases under that stamp, because a restart restores the snapshot,
    /// not the live world.
    scene_session_stamp: Option<SceneSessionStamp>,
    /// The wire request id of the command being executed, set by the live loop around each command.
    current_request: Option<RequestId>,
    /// The directory the profile resolves under; `None` = the project root. Set by
    /// [`EngineBuilder::save_root`](crate::EngineBuilder::save_root) — the seam a shipped build (a
    /// read-only content root) points at a separate writable directory. Unlike `project_root`, this
    /// never moves under `load_scene_file` or a scene switch: a save's location is a host decision, not
    /// a content-authoring one.
    pub(crate) save_root: Option<PathBuf>,
    /// This game's audio mixdown, from [`EngineBuilder::category_gains`](crate::EngineBuilder::category_gains):
    /// the master gain each bus is mixed through. The engine only carries it — a host reads it when
    /// it wires the playback sink up. Never simulation state, so it enters no `WorldHash` and no
    /// snapshot.
    pub(crate) category_gains: CategoryGains,
    /// How positional sounds measure distance, from
    /// [`EngineBuilder::audio_distance`](crate::EngineBuilder::audio_distance). Not simulation
    /// state: it shapes the audio stream only, so it enters no `WorldHash` and no snapshot.
    pub(crate) audio_distance: AudioDistance,
    /// Whether positional sounds are panned, from
    /// [`EngineBuilder::audio_pan`](crate::EngineBuilder::audio_pan). Not simulation state.
    pub(crate) audio_pan: AudioPan,
    /// The plane this game's 2D queries live in, from [`EngineBuilder::plane`](crate::EngineBuilder::plane).
    /// The engine mirrors it onto every world it builds as the [`Plane`] resource, which is how a
    /// query reads it; the field is the persistent copy that survives a resource wipe.
    pub(crate) plane: Plane,
    /// The CPU decoded-image cache `extract()` reads sprite textures through, so the live frame stream
    /// no longer re-reads and re-decodes every PNG from disk each frame. `asset.reload` invalidates
    /// entries here; cleared on a `scene.load`. Asset bytes never enter the `WorldHash`.
    asset_cache: AssetCache,
    render_size: (u32, u32),
    /// The host's view-camera override (`render.view`): the free pose the *stream* draws through
    /// instead of the scene's own `Camera`, or `None` for the scene's. A client/session setting
    /// exactly as [`quality`](Engine::quality) is — not hashed, not snapshotted, not recorded — and
    /// invisible to every path that did not ask for it: see `crate::render_view`.
    view_override: Option<ViewPose>,
    /// Which camera the camera cull view tests against (`render.view`'s `cull_from`). Host render
    /// state like `view_override`: never hashed, snapshotted or recorded.
    cull_from: CullFrom,
    /// The stream camera captured when `cull_from: frozen` applied; `Some` only while that mode is
    /// in force. Host render state, never hashed, snapshotted or recorded.
    pub(crate) frozen_cull: Option<ludion_renderer::RenderCamera>,
    /// The host's debug-draw filter ([`set_debug_draw`](Engine::set_debug_draw)): which categories the
    /// stream's [`RenderScene`] carries. Host render state like `view_override`, never in a world.
    debug_draw: crate::DebugCategories,
    /// The host's SSR debug view ([`set_debug_view`](Engine::set_debug_view)), shown in place of the
    /// composite. Host render state like `debug_draw`, never in a world.
    debug_view: Option<ludion_renderer::DebugView>,
    /// The host's explicit render scale ([`set_render_scale`](Engine::set_render_scale)), if it named
    /// one. `None` — the default — leaves the extent to whatever
    /// [`quality`](Engine::quality) resolved to, which is native unless the machine asked to draw
    /// below it. Held as the *override* rather than as the effective factor so the two can be set in
    /// either order: see [`render_scale`](Engine::render_scale). A client setting, never sim state:
    /// not hashed, not snapshotted, and invisible to `extract()` — it changes only how many pixels
    /// the chain draws before resampling back to [`render_size`](Engine::render_size).
    render_scale_override: Option<RenderScale>,
    /// The anti-aliasing mode the game packaged
    /// ([`EngineBuilder::anti_aliasing`](crate::EngineBuilder::anti_aliasing)), `Off` unless set. The
    /// weakest of the three sources; see [`anti_aliasing`](Engine::anti_aliasing). A client setting,
    /// never sim state: not hashed, not snapshotted, and invisible to `extract()`.
    pub(crate) anti_aliasing_default: AntiAliasing,
    /// Whether a handover collects `BlendData` for the presenter to blend per refresh
    /// ([`EngineBuilder::present_interpolation`](crate::EngineBuilder::present_interpolation)).
    /// Render-only; never hashed or snapshotted.
    pub(crate) present_interpolation: bool,
    /// Whether the window surface asks for frame latency 1
    /// ([`EngineBuilder::low_latency`](crate::EngineBuilder::low_latency)). Render-only.
    pub(crate) low_latency: bool,
    /// Whether the presenter re-resolves the first-person camera with unconsumed mouse motion
    /// ([`EngineBuilder::late_latch`](crate::EngineBuilder::late_latch)). Render-only.
    pub(crate) late_latch: bool,
    /// The window host's raw mouse total ([`set_look_motion`](Engine::set_look_motion)).
    pub(crate) look_motion: Option<Arc<ludion_renderer::LookMotion>>,
    /// The host's explicit mode ([`set_anti_aliasing`](Engine::set_anti_aliasing)), if it named one.
    /// Same status as `anti_aliasing_default`.
    anti_aliasing_override: Option<AntiAliasing>,
    /// Whether the renderer culls meshes against the camera ([`set_frustum_culling`](Engine::set_frustum_culling)).
    /// Host setting: render-only, never hashed, snapshotted or on the wire, and not a quality axis.
    /// Oracle tests and the "before" bench turn it off; a game cannot request it.
    frustum_culling: bool,
    /// The mode the game's own systems last requested through
    /// [`RenderSettings`](crate::RenderSettings), the strongest source. Lives here rather than in the
    /// world so a district switch or restore does not forget it; same status as the others.
    anti_aliasing_game: Option<AntiAliasing>,
    /// The pointer lock the game last requested through [`CursorLock`](crate::CursorLock). Lives here
    /// so a world rebuild does not forget it; host state, never hashed or snapshotted.
    cursor_lock_game: Option<bool>,
    /// The window host's channel for [`cursor_lock_game`](Engine::cursor_lock_request) changes
    /// ([`set_cursor_lock_sender`](Engine::set_cursor_lock_sender)); unset headless.
    cursor_lock_sender: Option<crossbeam_channel::Sender<bool>>,
    /// The host's render quality setting ([`set_quality`](Engine::set_quality)) — which optional
    /// effects to draw, plus the label of the choice that produced them. Default
    /// [`QualitySetting::default`] (everything the scene authors, labelled `"full"`). A client
    /// setting exactly as [`render_scale`](Engine::render_scale) is: not hashed, not snapshotted, and
    /// invisible to `extract()` — two machines at different qualities run the same simulation and
    /// differ only in pixels.
    quality: QualitySetting,
    /// Where imported mesh maps come from ([`set_textures`](Engine::set_textures)). Default
    /// [`TextureSource::Source`]. A client setting exactly as [`quality`](Engine::quality) is: not
    /// hashed, not snapshotted, not recorded and invisible to the simulation.
    textures: TextureSource,
    /// Whether the asset cache is pointed at the cooked store (or away from it) for the current
    /// [`textures`](Engine::textures) value and its held cook fallbacks have been re-tested against
    /// the store. Cleared by `set_textures`, a world rebuild and an `asset.reload` of a store file;
    /// the first extract after re-resolves the lookup (which needs the renderer's cook target) and
    /// retests the fallbacks.
    cook_resolved: bool,
    /// Whether a scene save asked the next cache reader to evict what the world no longer names.
    prune_pending: bool,
    /// Where reflection-probe cubes come from ([`set_probes`](Engine::set_probes)). Default
    /// [`ProbeSource::Fresh`]. A client setting exactly as [`textures`](Engine::textures) is: not
    /// hashed, not snapshotted, not recorded and invisible to the simulation.
    probes: ProbeSource,
    /// The probe store directory under `Store`, resolved when the axis is set.
    probe_store: Option<PathBuf>,
    /// The world the probe captures read, frozen at the first tick or render after a load; `None`
    /// until a scene with probes freezes one.
    probe_world: Option<ProbeWorld>,
    /// A load-class event happened: the next tick or render re-freezes the probe world.
    probe_world_pending: bool,
    /// The renderer's probe resolve count `finish_probe_frame` last examined.
    probe_resolve_seen: u64,
    /// Whether a bake is rendering, which writes the store on purpose and so warns of no miss.
    probe_baking: bool,
    /// This host's GI rebake lever ([`set_gi_rebake`](Engine::set_gi_rebake)). Default
    /// [`GiRebakeMode::Auto`] — today's behaviour, byte-identical. A client/session setting exactly
    /// as [`quality`](Engine::quality) is: not hashed, not snapshotted, not recorded, and invisible
    /// to the simulation.
    gi_rebake: GiRebakeMode,
    /// A pending one-shot rebake ([`gi_rebake_now`](Engine::gi_rebake_now)): the next frame runs with
    /// the hold suppressed, so both caches refresh to the current world and become the new baseline,
    /// and the flag clears itself. Self-clearing rather than a mode of its own, because "rebake" is
    /// an event and `Hold`/`Auto` is a state — a rebake at `Auto` is simply a no-op the next frame
    /// consumes.
    gi_rebake_force: bool,
    /// Whether the world was rebuilt since the last cached extract, so the next one prefetches the
    /// assets it references in parallel before walking them.
    prefetch_pending: bool,
    /// Whether the world was rebuilt since the cache was last brought to it: the next cache reader
    /// keeps what the world names and revalidates the kept entries' files.
    revalidate_pending: bool,
    /// The staleness the last completed frame reported: whether *either* GI cache served something
    /// built under inputs the frame no longer had. Recomputed per frame, never latched.
    gi_stale: bool,
    renderer: Option<Renderer>,
    /// Shader sources reloaded this session, oldest first, for a renderer built later.
    shader_reloads: Vec<(String, String)>,
    /// The shader directory a host has enabled dev-only shader hot reload for ([`enable_shader_reload`](Engine::enable_shader_reload)).
    /// Default `None`: an `asset.reload` naming a renderer `.wgsl` is an `L2008` structured error, and
    /// no shader is ever read from disk — every shipping/test/golden/`ludion render` path uses the
    /// compiled-in shaders. Only an explicitly dev-flagged host (`serve --dev-shaders`, a game run with
    /// `--dev-shaders`, `ludion dev`) turns it on, confining the pipeline-recreation risk to dev hosts.
    /// Render-side switch, never sim state: it is not hashed and not part of a snapshot.
    shader_reload_dir: Option<PathBuf>,
    /// The per-tick watch ([`set_tick_observer`](Engine::set_tick_observer)). Host state, never
    /// hashed or snapshotted; `Send` because the engine moves into the live sim thread.
    tick_observer: Option<Box<dyn TickObserver>>,
    /// The live frame stream's persistent 3-deep readback ring (lazily created, recreated on a render
    /// size change, discarded when no `frames` subscriber remains). `None` outside an active stream.
    stream_ring: Option<StreamReadback<FrameIndex>>,
    /// See [`frame_timeline`](Self::frame_timeline).
    frame_timeline: u64,
    /// The thread that renders and presents into this host's window
    /// ([`attach_surface`](Engine::attach_surface)), or `None` for a headless host — every CLI render,
    /// golden, test and served session. Host state exactly as [`renderer`](Engine::renderer) is: not
    /// hashed, not snapshotted, not serialized, and invisible to the simulation. The sim thread only
    /// hands it each tick's extracted scene; it never renders or waits for the window.
    presenter: Option<Presenter>,
    /// The surface's GPU context, kept so the sim-side renderer shares the presenter's device
    /// ([`Gpu::share`]); `None` on a headless host.
    surface_gpu: Option<Gpu>,
    /// Bumped when the world is replaced (load, switch, restore): a [`BlendData`] stamp, so a
    /// display-rate blend never slides an entity across a rebuild. A counter rather than a flag,
    /// because the presenter's mailbox is latest-wins and a flag on a dropped packet would be lost.
    present_epoch: u64,
    subscriptions: Subscriptions,
    event_outbox: Vec<Event>,
    recorder: Option<Recorder>,
    /// Why the last recording died early, until [`stop_recording`](Engine::stop_recording) returns it.
    recording_failure: Option<Diagnostic>,
    /// The game data-doc reload the last-dispatched `asset.reload` applied, if any — set by the
    /// handler ([`reload_doc_from_disk`](Engine::reload_doc_from_disk)) and taken by
    /// [`execute`](Engine::execute) so recording can write it as a `DocReload` record. A doc reload is
    /// indistinguishable from a texture reload by the `Command`, so this state is the seam recording
    /// reads (Phase 15 / E4, decision 9). Always taken each `execute`, so it never leaks across
    /// commands.
    pending_doc_reload: Option<AppliedDocReload>,
    /// The command, in the form it actually mutated the world, that a handler which *failed after
    /// mutating* leaves for [`execute`](Engine::execute) to record — set through
    /// [`note_applied_before_error`](Engine::note_applied_before_error). Today only a panic-stopped
    /// `sim.step` does (with the ticks that ran). Always taken each `execute`, and cleared by
    /// [`command_panicked`](Engine::command_panicked), so it never leaks across commands.
    applied_before_error: Option<ludion_protocol::Command>,
    /// Set by an `engine.shutdown` command: the live loop observes it and returns after replying, so a
    /// host can run its graceful teardown. Observable on a bare in-process engine too (no host = the
    /// flag is simply read, nothing exits). See [`request_shutdown`](Engine::request_shutdown).
    shutdown_requested: bool,
    /// A host's wake channel, armed by [`arm_shutdown_notify`](Engine::arm_shutdown_notify) before the
    /// engine moves onto the sim thread. Tripped alongside the flag so the host can wake from its
    /// Ctrl-C wait and run the same teardown; `None` for a bare in-process engine.
    shutdown_notify: Option<crossbeam_channel::Sender<()>>,
    /// A host's channel for a fatal render fault, armed by
    /// [`arm_render_fault_notify`](Engine::arm_render_fault_notify) before the engine moves onto
    /// the sim thread; `None` for a bare in-process engine.
    render_fault_notify: Option<crossbeam_channel::Sender<Diagnostic>>,
    /// A fault the live loop absorbs on its next pass; see [`Engine::queue_render_fault`].
    queued_render_fault: Option<Diagnostic>,
    /// An optional one-directional sender to the playback sink, wired by a host
    /// ([`set_audio_sender`](Engine::set_audio_sender)). Each drained message is forwarded here for
    /// playback; `None` for a headless/test engine, which still fills the [`audio_log`](Engine::audio_log).
    audio_sender: Option<crossbeam_channel::Sender<AudioMessage>>,
    /// The last sync sent for each [`AudioEmitter`](crate::AudioEmitter) in earshot at the last frame
    /// boundary — the one piece of state [`sync_audio_emitters`](Engine::sync_audio_emitters) keeps
    /// between ticks. Its keys are how an emitter that despawns, mutes, or loses its component
    /// mid-scene is stopped rather than left sounding; its values are the resend filter, no playback
    /// state. Host-side bookkeeping over an out-of-sim stream: never hashed, never snapshotted, never
    /// recorded. Cleared by [`install_audio_queue`](Engine::install_audio_queue), whose blanket stop
    /// already silences everything a rebuilt world used to sound.
    audible_emitters: StableMap<EntityId, SentLoop>,
    /// Whether a host has enabled writing the profile to disk at the frame boundary
    /// ([`persist_profile`](crate::EngineBuilder::persist_profile)). Default `false`, so a
    /// replay/CI/test run fills the log but never clobbers or renames the machine's real
    /// `save/profile.yaml` (decision 5); only a live host (`ludion::run`) turns it on.
    pub(crate) profile_persistence: bool,
    /// Invariant: a replay's profile comes from the recording or the default, never disk. Set by
    /// `prepare_replay`, so every profile install (the build's, a mid-replay `scene.load` or restore
    /// without profile text) skips the developer's `save/profile.yaml`.
    pub(crate) profile_off_disk: bool,
    /// The suspended-run state, `Some` once [`run_save`](crate::EngineBuilder::run_save) opted in.
    /// The [`RunSave`] resource mirrors it after every world rebuild.
    run_save: Option<RunSaveState>,
    /// The run-recording state, `Some` once [`run_recordings`](crate::EngineBuilder::run_recordings)
    /// opted in. It lives here, not in `SessionState`, because a run's reset clears the session.
    run_recording: Option<RunRecordingState>,
    /// The replay-playback state, `Some` once [`replay_playback`](crate::EngineBuilder::replay_playback)
    /// opted in.
    replay_playback: Option<ReplayPlaybackState>,
    /// The playback in progress. Taken out of the engine while its cursor ticks, so the tick the
    /// cursor performs re-enters [`advance_live`](Engine::advance_live) as a plain tick.
    playback: Option<ActivePlayback>,
    /// The playback-controls hook ([`EngineBuilder::playback_controls`](crate::EngineBuilder::playback_controls)).
    /// Host state: never in the world, a hash or a snapshot.
    pub(crate) playback_hook: Option<PlaybackHook>,
    /// The name of the recording playing, set for the whole playback (the cursor's absence while it
    /// ticks does not clear it).
    playback_name: Option<String>,
    /// The game's own version string (`EngineBuilder::game_version`), part of [`Engine::game_identity`].
    game_version: Option<String>,
    /// True from build until the first world rebuild: the profile the build read is what that first
    /// `scene.load` carries instead of re-reading disk (see `commit_world`).
    pub(crate) boot_profile_pending: bool,
    /// Under replay, the off-disk source for district scene text: a map from a switch's `scene_path`
    /// to the target district's canonical text, pre-scanned from the replay's `SceneSwitch` records
    /// by [`replay_hashes_with`](crate::replay_hashes_with) and installed before the drive. `Some`
    /// puts [`drain_scene_switch`](Engine::drain_scene_switch) in replay mode — it resolves a switch's
    /// target from this map, **never disk** (cross-machine purity, the `apply_doc_reload_inline`
    /// off-disk posture), and a map miss takes the identical rejection path a live disk-read failure
    /// takes (decision 4's determinism note: both are the same world no-op, only the unhashed message
    /// differs). `None` (a live engine only) reads disk and,
    /// when recording, writes the switch as a `SceneSwitch` record. A restart reads neither: its
    /// text is the held `scene`. Replay-drive-scoped: set only on a
    /// replay engine, never on a live one.
    replay_scene_text: Option<BTreeMap<String, String>>,
    /// The replay's recorded prefab texts, which stand in for the disk under the same posture as
    /// [`replay_scene_text`](Engine::replay_scene_text): `Some` on a replay engine (possibly empty),
    /// `None` on a live one, where each disk read is recorded as a `prefab_text` record.
    replay_prefab_text: Option<Arc<crate::prefab::PrefabTextSource>>,
    /// The rebuild epoch a replay's prefab library resolves its read texts in: each `commit_world`
    /// under replay takes [`replay_prefab_next_epoch`](Engine::replay_prefab_next_epoch), as the live
    /// library is emptied by each one. A restart is the exception: it stays in this epoch.
    replay_prefab_epoch: u32,
    /// The epoch the next world rebuild is in — also the epoch a load resolves its prefab reads in,
    /// before it commits.
    replay_prefab_next_epoch: u32,
    /// Per-session state that is not world content: the frame-stamped verification logs, the last
    /// non-fatal warnings, and the off-disk data-doc reload log. The three world-reset sites decide, as
    /// one unit, whether it resets to a fresh session or carries — see [`SessionState`].
    session: SessionState,
    /// Non-fatal warnings produced once at engine build from the registered-system metadata: the
    /// `L1124` schedule-module-concentration warning(s). Computed in [`from_parts`](Engine::from_parts)
    /// and read by a host (`ludion::run`) to print at boot. Empty for a bare engine or a game whose
    /// modules are all under threshold. Pure build-time metadata — never sim state, so it is not hashed,
    /// not snapshotted, and never regenerated on a world reset.
    boot_warnings: Vec<Diagnostic>,
    /// Whether opt-in query-shape recording is armed (default `false`). Host/test only: when on, each
    /// fresh world is armed for recording and observations accumulate into [`query_observations`](Engine::query_observations)
    /// across scene loads and district switches. Never sim state — not hashed, snapshotted, or replayed.
    query_recording: bool,
    /// The per-system query shapes observed while [`query_recording`](Engine::query_recording) has been
    /// on, accumulated across world resets (each reset builds a fresh world; its observations are drained
    /// in here before the swap). Drained by [`take_query_observations`](Engine::take_query_observations).
    query_observations: QueryObservations,
    /// The live profiler, present exactly while it is armed ([`perf_arm`](Engine::perf_arm)).
    /// `None` — the default — is what makes profiling free when it is off: the live tick's only cost
    /// is this one `Option` test, with no clock read, no observer, and no allocation. Boxed so an
    /// unarmed [`Engine`] carries a pointer rather than the whole window state. Pure host-side
    /// observation: never hashed, never snapshotted, never recorded (see [`crate::perf_live`]).
    perf: Option<Box<LivePerf>>,
}

/// The most recent [`AudioLogEntry`] entries a session keeps. Beyond this the oldest is dropped (the
/// [`crate::perf_live`] rolling-window mold): a live host's session — vigil's restart is a scene
/// switch, which carries the log — runs far longer than any test or replay ever drives one engine, so
/// an unbounded log is a slow memory leak in a shipped host. The text-first verification surface stays
/// exact for anything a test drives (every existing test is far below this bound); only a session that
/// has actually run this long loses its earliest entries.
const AUDIO_LOG_CAPACITY: usize = 4096;

/// The most [`ProfileSaveEntry`]s the profile-save log keeps, the oldest dropped first — the
/// [`AUDIO_LOG_CAPACITY`] bound, for the same reason: a long session must not grow it without limit.
const PROFILE_SAVE_LOG_CAPACITY: usize = 4096;

/// The most [`RunSaveEntry`]s the run-save log keeps, the oldest dropped first.
const RUN_SAVE_LOG_CAPACITY: usize = 4096;

/// What `engine.snapshot` captured, which the scene-session sidecar keeps describing.
struct SceneSessionStamp {
    frame: u64,
    scene_hash: String,
    path: Option<String>,
}

/// Per-session engine state that is **not** world content and is **not** captured verbatim in a
/// snapshot: the frame-stamped verification logs (audio, profile-save — a replay regenerates them),
/// the last non-fatal diagnostics (profile-save `L1122`, transition `L3018`–`L3020`, a caught system
/// panic `L4007`), and the off-disk
/// data-doc reload log a switch/restore re-applies over a freshly-walked content root.
///
/// This exists to make the reinstall/reset checklist one owner's decision instead of a convention
/// hand-repeated across the three world-reset sites (`scene.load`, `restore`, `switch`; audit finding
/// 3.1). Each site touches this state as **one unit** — [`reset`](SessionState::reset),
/// [`carry`](SessionState::carry), or [`adopt`](SessionState::adopt) — so a site
/// cannot clear one field and silently forget a sibling. The doc-reload log's clear-vs-carry choice
/// was exactly such a divergence (finding 2.1): `restore` cleared it while `switch` re-applied it, so
/// a snapshot restored the docs from possibly-changed disk. It is now a visible, exhaustive decision.
#[derive(Default)]
struct SessionState {
    /// The frame-stamped audio events emitted so far this session, in emission order — the text-first
    /// verification surface (see [`crate::audio`]). Bounded to [`AUDIO_LOG_CAPACITY`] entries, oldest
    /// dropped first. Never enters the `WorldHash` or a snapshot; it is host/test state, not sim state.
    audio_log: VecDeque<AudioLogEntry>,
    /// The frame-stamped profile-save requests drained so far this session, in request order — the
    /// text-first verification surface for saves (the [`audio_log`](SessionState::audio_log) mold). A
    /// pure function of the sim (a replay regenerates it), never in the `WorldHash` or a snapshot.
    /// Bounded to [`PROFILE_SAVE_LOG_CAPACITY`] entries, oldest dropped first.
    profile_save_log: VecDeque<ProfileSaveEntry>,
    /// The most recent frame-boundary profile-save failure (`L1122`), or `None` if the last save
    /// succeeded or none was attempted. A save write is atomic and non-fatal; this is how a host prints
    /// the warning and a test asserts it. Cleared on a successful write.
    profile_save_error: Option<Diagnostic>,
    /// The frame-stamped run-save requests drained so far this session — the
    /// [`profile_save_log`](SessionState::profile_save_log) mold. Bounded to
    /// [`RUN_SAVE_LOG_CAPACITY`], oldest dropped first.
    run_save_log: VecDeque<RunSaveEntry>,
    /// The most recent frame-boundary run-save failure (`L1400`); cleared on a successful write.
    run_save_error: Option<Diagnostic>,
    /// The most recent scene-switch warning: a rejection (`L3018` target unloadable / `L3019` anchor
    /// missing) or a succeeded-but-degraded switch (`L3020` — a dead carry id was skipped), or `None`
    /// if the last switch was fully clean or none was attempted (the
    /// [`profile_save_error`](SessionState::profile_save_error) mold for transitions). A switch is
    /// transactional and non-fatal: a failure never touches the live world or stops the sim; this is
    /// how a host prints the warning and a test asserts it. Cleared only on a fully-clean switch.
    last_transition_error: Option<Diagnostic>,
    /// The `code: message` line the last transition outcome printed, so an identical one is not
    /// printed again. A game that re-requests a rejected switch every frame it stands in the gate
    /// zone would otherwise flood stderr with one line per frame; any *different* outcome — another
    /// failure, or a clean switch — clears the latch, so nothing is ever swallowed.
    last_transition_report: Option<String>,
    /// The caught system panic (`L4007`) that auto-paused the sim, until something takes it —
    /// `sim.step` answers with it, and a host or test reads it through
    /// [`take_system_panic`](Engine::take_system_panic). `None` when no system has panicked since the
    /// last take or world reset. The wire-discoverable half of the same report is the `Error` log
    /// event pushed alongside it; this field is what gives the request that *caused* the tick a real
    /// answer instead of an Ack.
    last_system_panic: Option<Diagnostic>,
    /// The game data-doc reloads applied this session: one entry per `(kind, key)` holding its latest
    /// canonical text and its reload count, in first-appearance order. Recorded by both the live `asset.reload` doc branch
    /// ([`reload_doc_from_disk`](Engine::reload_doc_from_disk)) and the replay reapply
    /// ([`apply_doc_reload_inline`](Engine::apply_doc_reload_inline)), so it is identical live and under
    /// replay. A scene switch **and a restore** re-apply these off-disk after
    /// [`prepare_docs`](crate::docs::prepare_docs) re-walks the content root, so a doc reloaded *before* the reset
    /// survives the district rebuild with its reloaded value — even on a checkout whose on-disk doc has
    /// since reverted (Phase 17 / E7 item 4; audit finding 2.1). Snapshotted additively so a restore
    /// adopts the snapshot's log rather than rebuilding from disk. Never hashed — the `DocStore` it
    /// feeds is a resource.
    doc_reloads: crate::docs::DocReloadLog,
    /// What each scene visited this session kept behind, keyed by the scene's `name:` — the state a
    /// switch captured off its `Residue`-marked entities on the way out and re-applies on the way back
    /// in (see [`crate::transition`]). Engine state, not world content: never hashed, and a game reads
    /// it only through the world a switch rebuilds. Snapshotted additively (the `doc_reloads` mold) so
    /// a restore adopts the snapshot's map and a return performed after it re-applies what that session
    /// kept. Empty for a game that marks nothing, which is what makes every residue-free path identical
    /// to the one that existed before residue did.
    residue: StableMap<String, crate::transition::SceneResidue>,
    /// The `Name`s the scene document the live world was instantiated from authors — the provenance a
    /// residue capture stamps onto each record, so a return can tell a renamed authored entity (drop
    /// the record) from one a system spawned (respawn it). Rebuilt at every world build from a
    /// document (`scene.load`, a switch) and `None` after a `restore`: the restored world still
    /// carries its inline scene text, so [`Engine::authored_names`] derives the set from it on demand
    /// (an empty set when the text is not inline or does not parse — every record then reads as
    /// code-spawned, which respawns, the answer that never loses state).
    authored_names: Option<StableSet<String>>,
}

impl SessionState {
    /// Drops just the transient logs and warnings (the doc-reload log and the scene residue are
    /// decided separately by the caller). Shared by [`reset`](SessionState::reset) and
    /// [`adopt`](SessionState::adopt).
    fn reset_logs(&mut self) {
        self.audio_log.clear();
        self.profile_save_log.clear();
        self.profile_save_error = None;
        self.run_save_log.clear();
        self.run_save_error = None;
        self.last_transition_error = None;
        self.last_transition_report = None;
        self.last_system_panic = None;
    }

    /// A fresh session — drop every log, the doc-reload set **and** the scene residue. The
    /// `scene.load` posture: a new scene starts a clean verification stream that a replay regenerates
    /// from frame 0, no reload carries across a full reload of the world, and no district remembers a
    /// visit from a run that is over.
    fn reset(&mut self) {
        self.reset_logs();
        self.doc_reloads.clear();
        self.residue.clear();
        // The caller re-stamps this from the scene it just instantiated.
        self.authored_names = Some(StableSet::new());
    }

    /// Continue the session unchanged — every log, the doc-reload set and the scene residue survive.
    /// The `switch` posture: a switch is mid-session, the opposite of `scene.load`/`restore`, and
    /// remembering the district you just left is the whole point of residue. A no-op by construction;
    /// it exists so a lifecycle site states the reset-vs-carry choice explicitly (the whole point of
    /// this struct — the choice is never implicit). The switch itself updates the residue map inside
    /// [`crate::transition`]'s `perform`, which is where the departing world is still readable.
    fn carry(&mut self) {}

    /// A `restore`: the transient logs reset (a snapshot never captures them; a replay regenerates
    /// them), but the doc-reload set is **replaced** by the snapshot's `reloads`. The caller then
    /// re-applies them off-disk (via [`prepare_docs`](crate::docs::prepare_docs)) so a
    /// post-restore switch or a replay-seek reproduces the reloaded docs instead of rebuilding from
    /// possibly-changed disk (audit finding 2.1). An empty `reloads` (a snapshot with no live reload,
    /// or an older snapshot predating the field) restores the pre-fix behavior: a clean disk rebuild.
    ///
    /// The scene residue is **replaced** the same way — a restore reconstructs the session from a
    /// document, so what each scene kept comes from that document, never from the live map (which
    /// would re-apply, on the next return, state captured *after* the snapshot being restored). A
    /// snapshot taken after a return therefore reproduces that return's residue, whether the restore
    /// is a `ludion dev` restart or a replay seek onto the keyframe. An empty `residue` — a game that
    /// marks nothing, or an older snapshot predating the field — adopts nothing.
    fn adopt(
        &mut self,
        reloads: crate::docs::DocReloadLog,
        residue: StableMap<String, crate::transition::SceneResidue>,
    ) {
        self.reset_logs();
        self.doc_reloads = reloads;
        self.residue = residue;
        // The restored world keeps its inline scene text, so the authored set is derived from it when
        // a capture asks (see `Engine::authored_names`) — a restore stays lazy.
        self.authored_names = None;
    }
}

impl Default for Engine {
    fn default() -> Self {
        Engine::new()
    }
}

impl Engine {
    /// Creates an engine with an empty world, the built-in component registry, no game systems, and
    /// the current directory as its project root.
    pub fn new() -> Self {
        Engine::with_project_root(".")
    }

    /// Creates a bare engine (built-in registry, empty schedule) whose scene/asset paths resolve
    /// relative to `project_root`.
    ///
    /// This is the construction path every built-in CLI command uses. A *game* — one that registers
    /// its own components and systems — is built through [`Engine::builder`] instead.
    pub fn with_project_root(project_root: impl Into<PathBuf>) -> Self {
        Engine::from_parts(
            builtin_registry(),
            Vec::new(),
            Vec::new(),
            None,
            crate::player_profile::default_profile_rel_path(),
            project_root.into(),
            DEFAULT_RENDER_SIZE,
        )
    }

    /// Starts an [`EngineBuilder`](crate::EngineBuilder): the one way to build an engine that hosts a game's own components
    /// and systems on top of the built-in core (see [`crate::builder`]).
    ///
    /// # Example
    /// ```
    /// use ludion_engine::Engine;
    /// # use serde::{Deserialize, Serialize};
    /// # use schemars::JsonSchema;
    /// #[derive(Serialize, Deserialize, JsonSchema, Clone, Copy)]
    /// struct Paddle { x: f32 }
    ///
    /// let engine = Engine::builder()
    ///     .components(|reg| reg.register::<Paddle>("Paddle"))
    ///     .system("move_paddle", |_world, _ctx| { /* ... */ })
    ///     .build()
    ///     .unwrap();
    /// assert!(engine.registry().get("Paddle").is_some());
    /// // The built-in visual components are still present — scenes mix them with game components.
    /// assert!(engine.registry().get("Transform").is_some());
    /// ```
    pub fn builder() -> crate::builder::EngineBuilder {
        crate::builder::EngineBuilder::new()
    }

    /// The single owned-parts constructor behind [`new`](Engine::new),
    /// [`with_project_root`](Engine::with_project_root), and [`EngineBuilder::build_at`]. `registry`
    /// already carries the built-ins plus any game components; `systems` is the game schedule's
    /// persistent source of truth (rebuilt into a fresh [`Schedule`] on every world reset).
    ///
    /// Every argument is a piece of engine configuration the caller already holds separately;
    /// bundling them into a struct would only move the same list one indirection away.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn from_parts(
        registry: Registry,
        systems: Vec<SystemSpec>,
        doc_kinds: Vec<DocKind>,
        profile_kind: Option<ProfileKind>,
        profile_rel_path: PathBuf,
        project_root: PathBuf,
        render_size: (u32, u32),
    ) -> Self {
        // Group the registered systems by definition module and warn on any catch-all module (`L1124`),
        // once, from the metadata — before `systems` is moved into the engine.
        let boot_warnings = schedule_module_warnings(&systems);
        let mut engine = Engine {
            driver: SimDriver::new(World::new(), Schedule::new(), SimConfig::default()),
            registry: Arc::new(registry),
            systems,
            scene_physics: None,
            doc_kinds,
            doc_validators: Vec::new(),
            game_doc_kinds: false,
            doc_texts: BTreeMap::new(),
            resource_carriers: Vec::new(),
            projectile_config: None,
            nav_config: None,
            projectile_draw: crate::projectile_draw::ProjectileDraw::default(),
            doc_validation_warnings: Vec::new(),
            doc_skip_warnings: Vec::new(),
            doc_skip_summary: None,
            profile_kind,
            profile_rel_path,
            profile_migration: ProfileMigration::default(),
            seed: 0,
            scene: None,
            authored: std::sync::OnceLock::new(),
            scene_name: None,
            project_root,
            scene_bases: BTreeMap::new(),
            current_scene_path: None,
            scene_session_file: None,
            scene_session_stamp: None,
            current_request: None,
            save_root: None,
            category_gains: CategoryGains::default(),
            audio_distance: AudioDistance::default(),
            audio_pan: AudioPan::default(),
            plane: Plane::default(),
            asset_cache: AssetCache::new(),
            render_size,
            view_override: None,
            cull_from: CullFrom::View,
            frozen_cull: None,
            debug_draw: crate::DebugCategories::None,
            debug_view: None,
            render_scale_override: None,
            anti_aliasing_default: AntiAliasing::Off,
            present_interpolation: false,
            low_latency: false,
            late_latch: false,
            look_motion: None,
            anti_aliasing_override: None,
            frustum_culling: true,
            anti_aliasing_game: None,
            cursor_lock_game: None,
            cursor_lock_sender: None,
            quality: QualitySetting::default(),
            textures: TextureSource::default(),
            cook_resolved: false,
            prune_pending: false,
            probes: ProbeSource::default(),
            probe_store: None,
            probe_world: None,
            probe_world_pending: true,
            probe_resolve_seen: 0,
            probe_baking: false,
            gi_rebake: GiRebakeMode::default(),
            gi_rebake_force: false,
            prefetch_pending: false,
            revalidate_pending: false,
            gi_stale: false,
            renderer: None,
            shader_reloads: Vec::new(),
            shader_reload_dir: None,
            tick_observer: None,
            stream_ring: None,
            frame_timeline: 0,
            presenter: None,
            surface_gpu: None,
            present_epoch: 0,
            subscriptions: Subscriptions::new(),
            event_outbox: Vec::new(),
            recorder: None,
            recording_failure: None,
            pending_doc_reload: None,
            applied_before_error: None,
            shutdown_requested: false,
            shutdown_notify: None,
            render_fault_notify: None,
            queued_render_fault: None,
            audio_sender: None,
            audible_emitters: StableMap::new(),
            profile_persistence: false,
            profile_off_disk: false,
            run_save: None,
            run_recording: None,
            replay_playback: None,
            playback: None,
            playback_hook: None,
            playback_name: None,
            game_version: None,
            boot_profile_pending: false,
            replay_scene_text: None,
            replay_prefab_text: None,
            replay_prefab_epoch: 0,
            replay_prefab_next_epoch: 0,
            session: SessionState::default(),
            boot_warnings,
            query_recording: false,
            query_observations: QueryObservations::default(),
            perf: None,
        };
        // Install the game schedule into the initial (empty) world so an engine that is driven before
        // any `scene.load` still runs its systems.
        let schedule = engine.build_schedule(None);
        engine.driver = SimDriver::new(World::new(), schedule, SimConfig::default());
        engine.install_audio_queue();
        engine.install_presentation_rules();
        engine.install_scene_switch();
        engine.install_shutdown();
        engine.install_render_settings();
        engine.install_cursor_lock();
        engine.install_debug_draw();
        engine.install_game_clock(GameClock::default());
        engine.install_diagnostics(crate::Diagnostics::default());
        engine.install_render_extent();
        engine.install_plane();
        engine.install_animation_library();
        engine.install_skin_rests();
        engine.install_path_library();
        engine.install_font_library();
        engine.install_prefab_library();
        engine.install_collider_meshes();
        engine.install_residue_store();
        engine
    }

    /// Builds a fresh [`Schedule`] from the registered [`systems`](Engine::systems) and `physics`,
    /// in the order [`assemble_schedule`](scene_physics::assemble_schedule) states. Called at every
    /// world reset — construction, `scene.load`,
    /// `engine.restore`, `scene.switch` — so the game schedule is reinstated instead of dropped (the
    /// bug this fixes: the old code hard-coded `Schedule::new()` on load and restore, silently
    /// discarding any configured schedule).
    ///
    /// Takes the config as a parameter rather than reading [`scene_physics`](Engine::scene_physics):
    /// the restore and switch paths build the schedule **before** the point of no return, and the
    /// field is committed only after it.
    pub(crate) fn build_schedule(&self, physics: Option<&ScenePhysics>) -> Schedule {
        scene_physics::assemble_schedule(physics, &self.systems)
    }

    /// Sets the default screenshot resolution (used when a `render.screenshot` omits dimensions).
    pub fn with_render_size(mut self, width: u32, height: u32) -> Self {
        self.render_size = (width, height);
        self.install_render_extent();
        self
    }

    /// Sets this session's render extent: the default screenshot resolution, *and* the extent every
    /// live stream frame is produced at.
    ///
    /// The non-consuming sibling of [`with_render_size`](Engine::with_render_size) — both write the
    /// one field, and [`render_size`](Engine::render_size) reads it. A host applies it before the
    /// engine moves onto the sim thread (`--render-size`, else `LUDION_RENDER_SIZE`, resolved by
    /// [`resolve_startup_render_size`](crate::resolve_startup_render_size)).
    ///
    /// The extent never enters a `WorldHash`, but it is sim *input* (a game reads it as
    /// [`RenderExtent`](crate::RenderExtent)): a snapshot carries it in its own `render_extent` field,
    /// a recording's header carries it at arm time, and an applied `render.size` is recorded, so a
    /// replay runs at the extent it was recorded at. Unlike
    /// [`set_render_scale`](Engine::set_render_scale) — which draws at `render_size × scale` and
    /// resamples *back* to it — this moves the number of bytes a stream actually delivers, which is
    /// the whole reason it exists. It is mirrored onto the live world as the
    /// [`RenderExtent`](crate::RenderExtent) resource, which is how a game system reads it.
    ///
    /// The live stream follows it from the next frame: `render_frame` rebuilds its readback
    /// ring whenever the extent moves, and every downstream stage already handles a mid-stream
    /// resize.
    pub fn set_render_size(&mut self, width: u32, height: u32) {
        self.render_size = (width, height);
        self.install_render_extent();
    }

    /// Points the live frame stream at `pose`, or returns it to the scene's own `Camera` with `None`.
    ///
    /// The `render.view` lever (`crate::render_view`). Host render state like
    /// [`render_size`](Engine::render_size): the view pose never enters a `WorldHash`, a snapshot or
    /// a recording (while a recording and a snapshot both carry the extent), so a session flown to a
    /// rooftop simulates and records identically to one that never moved — and clearing it renders
    /// byte-identically to before it was ever set.
    ///
    /// It moves the *stream*. [`render_png`](Engine::render_png) and `render.bounds` choose their
    /// camera per call and default to the scene's, so an agent's screenshot keeps the game's framing
    /// while a human flies.
    pub fn set_view_override(&mut self, pose: Option<ViewPose>) {
        self.view_override = pose;
    }

    /// The view override in force, or `None` when the stream is on the scene's own `Camera`.
    pub fn view_override(&self) -> Option<&ViewPose> {
        self.view_override.as_ref()
    }

    /// The camera the camera cull view tests against ([`CullFrom::View`] unless `render.view` chose).
    pub fn cull_from(&self) -> CullFrom {
        self.cull_from
    }

    /// Chooses the camera the camera cull view tests against. Host render state, exactly as
    /// [`set_view_override`](Engine::set_view_override) is.
    ///
    /// `Frozen` captures the stream camera now (the view override in force, else the scene's) and
    /// holds it; setting `Frozen` again recaptures; `View` and `Game` drop the capture. The image is
    /// always drawn through the rendered camera; only the frustum the cull keeps instances by moves.
    ///
    /// # Errors
    /// The camera diagnostic (`L2101`, an ambiguous scene) when `Frozen` cannot resolve the stream
    /// camera; nothing changes then.
    pub fn set_cull_from(&mut self, mode: CullFrom) -> Result<(), Diagnostic> {
        self.frozen_cull = match mode {
            CullFrom::Frozen => Some(crate::extract::stream_camera(
                self.driver.world(),
                self.render_size,
                self.view_override.as_ref(),
            )?),
            CullFrom::View | CullFrom::Game => None,
        };
        self.cull_from = mode;
        Ok(())
    }

    /// Chooses which debug-draw categories the render extraction carries (`crate::debug_draw`).
    ///
    /// Host render state, exactly as [`set_view_override`](Engine::set_view_override) is: never
    /// hashed, snapshotted or recorded, and no system can read it. The pen always records; this only
    /// filters what reaches the renderer, and the default ([`DebugCategories::None`](crate::DebugCategories))
    /// extracts byte-identically to an engine without debug draw.
    pub fn set_debug_draw(&mut self, categories: crate::DebugCategories) {
        self.debug_draw = categories;
    }

    /// Chooses the SSR debug view the stream draws in place of the composite, or `None` for the
    /// normal frame.
    ///
    /// Host render state, exactly as [`set_debug_draw`](Engine::set_debug_draw) is: never hashed,
    /// snapshotted or recorded. A view never turns SSR on — a frame without SSR draws normally.
    pub fn set_debug_view(&mut self, view: Option<ludion_renderer::DebugView>) {
        self.debug_view = view;
    }

    /// The SSR debug view the host chose.
    pub fn debug_view(&self) -> Option<ludion_renderer::DebugView> {
        self.debug_view
    }

    /// The debug-draw categories the host enabled.
    pub fn debug_draw(&self) -> &crate::DebugCategories {
        &self.debug_draw
    }

    /// The categories the last completed tick drew under, sorted — whether enabled or not.
    pub fn debug_draw_drawn(&self) -> Vec<&'static str> {
        crate::debug_draw::drawn(self.driver.world())
    }

    /// Sets this host's render scale explicitly (`ludion serve --scale`, `ludion profile --scale`),
    /// overriding whatever [`quality`](Engine::quality) asks for.
    ///
    /// A host switch, not a protocol command — the pixel budget is the host's, so a connected viewer
    /// never negotiates it and no wire method exposes it. Every render this engine performs —
    /// screenshots, the live frame stream, profiling — draws at
    /// [`render_size`](Engine::render_size) × the scale and resamples back, so the images a client
    /// receives keep exactly the dimensions it asked for.
    pub fn set_render_scale(&mut self, scale: RenderScale) {
        self.render_scale_override = Some(scale);
    }

    /// The scale this host actually draws at: the explicit one if
    /// [`set_render_scale`](Engine::set_render_scale) named one, else the one
    /// [`quality`](Engine::quality) carries, else native
    /// ([`resolve_render_scale`](crate::resolve_render_scale)).
    ///
    /// Resolved on read from two independently held values rather than folded together on write,
    /// which is what makes [`set_quality`](Engine::set_quality) and `set_render_scale` commute: a
    /// host may call them in either order and gets the same frame.
    pub fn render_scale(&self) -> RenderScale {
        crate::resolve_render_scale(self.render_scale_override, &self.quality)
    }

    /// Turns frustum culling on or off for this host. On by default; survives scene loads.
    ///
    /// A host switch, not a protocol command and not a game request: it changes what the renderer
    /// skips, never the sim.
    pub fn set_frustum_culling(&mut self, on: bool) {
        self.frustum_culling = on;
    }

    /// Whether frustum culling is on.
    pub fn frustum_culling(&self) -> bool {
        self.frustum_culling
    }

    /// Attaches (or detaches) an [`ExtractObserver`](crate::extract_observer::ExtractObserver) that
    /// sees every extract phase boundary. Pure observation: it never changes the scene or the hash.
    #[doc(hidden)]
    pub fn set_extract_observer(
        &mut self,
        observer: Option<Box<dyn crate::extract_observer::ExtractObserver>>,
    ) {
        self.asset_cache.set_extract_observer(observer);
    }

    /// Detaches and returns the extract observer, so a profiler can borrow the slot and restore it.
    #[doc(hidden)]
    pub fn take_extract_observer(
        &mut self,
    ) -> Option<Box<dyn crate::extract_observer::ExtractObserver>> {
        self.asset_cache.take_extract_observer()
    }

    /// Sets this host's anti-aliasing mode, beating the game's packaged default from
    /// [`EngineBuilder::anti_aliasing`](crate::EngineBuilder::anti_aliasing) but not the game's own
    /// runtime request through [`RenderSettings`](crate::RenderSettings).
    ///
    /// A host switch, not a protocol command: it changes how pixels are drawn, never the sim. Applies
    /// from the next render.
    pub fn set_anti_aliasing(&mut self, mode: AntiAliasing) {
        self.anti_aliasing_override = Some(mode);
    }

    /// The anti-aliasing mode this engine renders with: the game's runtime request, else the host's
    /// [`set_anti_aliasing`](Engine::set_anti_aliasing), else the builder default, else `Off`
    /// ([`resolve_anti_aliasing`](crate::resolve_anti_aliasing)). Resolved on read from three
    /// separately held values, so the order they are set in never matters.
    pub fn anti_aliasing(&self) -> AntiAliasing {
        crate::resolve_anti_aliasing(
            self.anti_aliasing_game,
            self.anti_aliasing_override,
            self.anti_aliasing_default,
        )
    }

    /// Sets this host's render quality — which optional effects every render performs (see
    /// [`resolve_startup_quality`](crate::resolve_startup_quality), which produces the setting from a
    /// `--quality` flag or `LUDION_QUALITY`).
    ///
    /// A host switch, not a protocol command — quality is a client's GPU budget, so a connected
    /// viewer never negotiates it and no wire method exposes it. Render-only: never sim state, never
    /// hashed into a `WorldHash`, never snapshotted, and invisible to `extract()`, so the same scene
    /// simulates identically at every quality and differs only in pixels. It applies to a renderer
    /// already in use as well as to one this engine has yet to create.
    pub fn set_quality(&mut self, setting: QualitySetting) {
        self.quality = setting;
        if let Some(renderer) = self.renderer.as_mut() {
            renderer.set_quality(self.quality.quality, self.quality.params);
        }
    }

    /// Starts measuring load: from the next scene load through the next extract, the engine records
    /// per-phase wall time and per-kind decode counters for [`profile_engine`](crate::profile_engine)
    /// to report as [`ProfileReport::load`](crate::ProfileReport::load).
    ///
    /// Pure observation: an attached observer never changes the world or its `WorldHash`. A host that
    /// does not call it reads no clock for loading.
    ///
    /// # Example
    /// ```no_run
    /// use ludion_engine::Engine;
    ///
    /// let mut engine = Engine::new();
    /// engine.observe_load();
    /// engine.load_inline_scene("format: 1\nentities: []\n").unwrap();
    /// ```
    pub fn observe_load(&mut self) {
        self.asset_cache
            .set_observer(Some(Box::new(crate::profile::LoadClock::default())));
    }

    /// Attaches `observer` as the load observer.
    #[cfg(test)]
    pub(crate) fn set_load_observer(&mut self, observer: Box<dyn LoadObserver>) {
        self.asset_cache.set_observer(Some(observer));
    }

    /// Whether a load observer is attached.
    pub(crate) fn load_observed(&self) -> bool {
        self.asset_cache.observed()
    }

    /// Detaches the load observer, ending the measurement.
    pub(crate) fn take_load_observer(&mut self) -> Option<Box<dyn LoadObserver>> {
        self.asset_cache.take_observer()
    }

    /// This host's render quality setting (see [`set_quality`](Engine::set_quality)).
    pub fn quality(&self) -> &QualitySetting {
        &self.quality
    }

    /// Chooses where imported mesh maps come from: [`TextureSource::Source`] (the default) decodes
    /// every PNG, [`TextureSource::Cooked`] reads each `[[import]]` or `[cook] roots` map's BC7/ASTC
    /// copy from `<project>/.ludion/cooked/` and falls back to the PNG, with one `L1271` warning per
    /// load, where there is none.
    ///
    /// A host switch, not a protocol command, and render-only exactly as
    /// [`set_quality`](Engine::set_quality) is: never sim state, never hashed, never snapshotted,
    /// invisible to `extract()` and to a recorded session. The next extract applies it, dropping the
    /// maps (and the meshes built from them) the other setting loaded.
    ///
    /// # Example
    /// ```
    /// use ludion_engine::{Engine, TextureSource};
    ///
    /// let mut engine = Engine::new();
    /// assert_eq!(engine.textures(), TextureSource::Source);
    /// engine.set_textures(TextureSource::Cooked);
    /// assert_eq!(engine.textures(), TextureSource::Cooked);
    /// ```
    pub fn set_textures(&mut self, source: TextureSource) {
        self.textures = source;
        self.cook_resolved = false;
        self.probe_world_stale();
    }

    /// Where this host's imported mesh maps come from (see [`set_textures`](Engine::set_textures)).
    pub fn textures(&self) -> TextureSource {
        self.textures
    }

    /// Sets where this host's reflection-probe cubes come from: [`ProbeSource::Store`] reads the
    /// content-addressed probe store under the project (`.ludion/cooked/probes/`) and captures,
    /// writing the entry, on a miss, reporting the miss as `L1279`; the default
    /// [`ProbeSource::Fresh`] captures every probe at load and never touches the store. A render
    /// setting like [`set_textures`](Engine::set_textures): never hashed, snapshotted or recorded.
    ///
    /// # Example
    /// ```
    /// use ludion_engine::{Engine, ProbeSource};
    ///
    /// let mut engine = Engine::new();
    /// assert_eq!(engine.probes(), ProbeSource::Fresh);
    /// engine.set_probes(ProbeSource::Store);
    /// assert_eq!(engine.probes(), ProbeSource::Store);
    /// ```
    pub fn set_probes(&mut self, source: ProbeSource) {
        self.probes = source;
        self.probe_store = match source {
            ProbeSource::Fresh => None,
            ProbeSource::Store => Some(crate::probe_bake::store_dir(&self.project_root)),
        };
    }

    #[cfg(test)]
    pub(crate) fn probe_store_dir(&self) -> Option<&Path> {
        self.probe_store.as_deref()
    }

    #[cfg(test)]
    pub(crate) fn has_renderer(&self) -> bool {
        self.renderer.is_some()
    }

    /// Where this host's reflection-probe cubes come from (see [`set_probes`](Engine::set_probes)).
    pub fn probes(&self) -> ProbeSource {
        self.probes
    }

    /// Bakes the scene's reflection probes into the probe store and reports each one. Bakes use
    /// cooked textures and the store, because the store key hashes a map's cook key and the hosts
    /// default to cooked maps: a bake from the PNGs would never hit. A scene with no probe gives an
    /// empty report and builds no renderer.
    ///
    /// With `force` naming a probe's `Name`, that probe's entry is deleted first and recaptured.
    ///
    /// # Errors
    /// `L1280` if `force` names no probe; a render failure (`L2003` with no GPU) as it comes.
    pub fn bake_probes(&mut self, force: Option<&str>) -> Result<ProbeBakeReport, Diagnostic> {
        let store = crate::probe_bake::store_dir(&self.project_root);
        let named = self.named_probes();
        if named.is_empty() {
            return match force {
                Some(name) => Err(crate::probe_bake::unknown_probe(name, &[])),
                None => Ok(ProbeBakeReport {
                    store,
                    probes: Vec::new(),
                }),
            };
        }
        if let Some(name) = force {
            let names: Vec<String> = named.iter().filter_map(|(_, n)| n.clone()).collect();
            if !names.iter().any(|n| n == name) {
                return Err(crate::probe_bake::unknown_probe(name, &names));
            }
        }
        self.set_textures(TextureSource::Cooked);
        self.set_probes(ProbeSource::Store);
        self.probe_baking = true;
        let report = self.bake_renders(force, &named);
        self.probe_baking = false;
        report
    }

    /// Marks the frozen probe world stale: the next tick or render re-freezes it, so a load-class
    /// event (a world rebuild, a reloaded asset, another texture source) recaptures the probes.
    pub(crate) fn probe_world_stale(&mut self) {
        self.probe_world_pending = true;
    }

    /// Freezes the probe world from the frame-0 world before the first tick after a load-class
    /// event, so a bake and a host that ticks first capture the same world. A world without probes
    /// pays nothing but the entity scan, once.
    fn freeze_probe_world_before_tick(&mut self) {
        if !self.probe_world_pending {
            return;
        }
        self.probe_world_pending = false;
        self.probe_world = if self.named_probes().is_empty() {
            None
        } else {
            self.extract_render_scene_inner(self.render_size.0, self.render_size.1, false, None)
                .ok()
                .map(ProbeWorld::new)
        };
    }

    /// Everything that must happen before a tick runs: the probe world freeze, and, only when a tick
    /// will actually run, the clearing of last tick's presentation cuts. A paused loop keeps its
    /// marks until a frame hands them over.
    fn before_tick(&mut self) {
        self.freeze_probe_world_before_tick();
        let will_tick = match self.driver.run_state() {
            RunState::Running => true,
            RunState::Stepping { remaining } => remaining > 0,
            _ => false,
        };
        if will_tick {
            crate::interpolation::clear_cuts(self.driver.world_mut());
        }
    }

    /// The scene's `ReflectionProbe` entities with their `Name`s, in entity order.
    fn named_probes(&self) -> Vec<(EntityId, Option<String>)> {
        let world = self.driver.world();
        world
            .entities()
            .chain(world.presentation_entities())
            .filter(|&id| matches!(world.get::<ReflectionProbe>(id), Ok(Some(_))))
            .map(|id| {
                let name = world.get::<Name>(id).ok().flatten().map(|n| n.0.clone());
                (id, name)
            })
            .collect()
    }

    fn bake_renders(
        &mut self,
        force: Option<&str>,
        named: &[(EntityId, Option<String>)],
    ) -> Result<ProbeBakeReport, Diagnostic> {
        // The face size comes from the quality setting, not the extent.
        let render = |engine: &mut Engine| {
            engine
                .render_image(256, 144, false)
                .map(|_| ())
                .map_err(|mut diagnostics| diagnostics.swap_remove(0))
        };
        render(self)?;
        let dir = self.probe_store.clone().expect("the bake set the store");
        let mut report = self.probe_bake_report(&dir, named);
        if let Some(name) = force {
            let forced = report
                .probes
                .iter()
                .find(|p| p.name.as_deref() == Some(name));
            if let Some(probe) = forced {
                let _ = std::fs::remove_file(
                    dir.join(ludion_assets::cooked::cooked_file_name(probe.key)),
                );
            }
            let renderer = self.renderer_mut()?;
            // Changing the store directory drops the resident captures, so the next render resolves
            // again and finds the entry gone.
            renderer.set_probe_store(None);
            renderer.set_probe_store(Some(dir.clone()));
            render(self)?;
            report = self.probe_bake_report(&dir, named);
        }
        Ok(report)
    }

    /// The last probe resolve as a profile section: store hits against captures. `None` for a scene
    /// without probes or an engine that has not rendered.
    pub fn probe_profile(&self) -> Option<ludion_protocol::perf::ProbeProfile> {
        let resolved = self.renderer.as_ref()?.probe_resolve();
        if resolved.probes.is_empty() {
            return None;
        }
        let hits = resolved.probes.iter().filter(|p| p.loaded).count() as u64;
        Some(ludion_protocol::perf::ProbeProfile {
            hits,
            captures: resolved.probes.len() as u64 - hits,
        })
    }

    fn probe_bake_report(
        &self,
        dir: &Path,
        named: &[(EntityId, Option<String>)],
    ) -> ProbeBakeReport {
        let resolved = self
            .renderer
            .as_ref()
            .map(Renderer::probe_resolve)
            .unwrap_or_default();
        let probes = resolved
            .probes
            .iter()
            .filter_map(|probe| {
                Some(ProbeBake {
                    entity: probe.entity,
                    name: named
                        .iter()
                        .find(|(id, _)| *id == probe.entity)
                        .and_then(|(_, name)| name.clone()),
                    key: probe.key?,
                    outcome: if probe.loaded {
                        ProbeOutcome::Loaded
                    } else {
                        ProbeOutcome::Captured
                    },
                })
            })
            .collect();
        ProbeBakeReport {
            store: dir.to_path_buf(),
            probes,
        }
    }

    /// Reports an `L1279` for each probe resolve that captured probes a `Store` host expected to
    /// load, once per resolve (a resolve with nothing missing reports nothing). Called after every
    /// render.
    fn finish_probe_frame(&mut self) {
        if self.probes != ProbeSource::Store || self.probe_baking {
            return;
        }
        let Some(renderer) = self.renderer.as_ref() else {
            return;
        };
        let resolved = renderer.probe_resolve();
        self.note_probe_resolve(&resolved);
    }

    /// [`finish_probe_frame`](Engine::finish_probe_frame) over a resolve the caller already holds —
    /// the sim-side renderer's, or the presenter's.
    fn note_probe_resolve(&mut self, resolved: &ProbeResolve) {
        if self.probes != ProbeSource::Store || self.probe_baking {
            return;
        }
        if resolved.resolve == self.probe_resolve_seen {
            return;
        }
        self.probe_resolve_seen = resolved.resolve;
        let world = self.driver.world();
        let missing: Vec<String> = resolved
            .probes
            .iter()
            .filter(|probe| !probe.loaded)
            .map(
                |probe| match world.get::<Name>(probe.entity).ok().flatten() {
                    Some(name) => format!("probe `{}`", name.0),
                    None => format!("the unnamed probe {:?}", probe.entity),
                },
            )
            .collect();
        if missing.is_empty() {
            return;
        }
        let diagnostic = crate::probe_bake::not_stored_warning(&missing);
        if let Some(sink) = self.driver.world_mut().resource_mut::<crate::Diagnostics>() {
            sink.report_full("probes: store", diagnostic);
        }
    }

    /// Points the asset cache at the cooked store this host's [`textures`](Engine::textures) and
    /// renderer imply, once per setting. Under `Cooked` this builds the renderer ahead of the walk,
    /// because the cook target is the GPU's.
    fn resolve_cook(&mut self) {
        if self.cook_resolved {
            return;
        }
        let lookup = match self.textures {
            TextureSource::Source => None,
            TextureSource::Cooked => {
                // Without a GPU there is no target; the render that follows reports `L2003`.
                let supported = match self.ensure_renderer() {
                    Ok(()) => self
                        .renderer
                        .as_ref()
                        .map_or_else(Vec::new, Renderer::cook_targets),
                    Err(_) => Vec::new(),
                };
                crate::cooked_maps::lookup_for(&self.project_root, &supported).map(Arc::new)
            }
        };
        let changed = self.asset_cache.set_cook(lookup);
        let retried = self.asset_cache.retry_cook_misses();
        if changed || retried {
            self.prefetch_pending = true;
        }
        self.cook_resolved = true;
    }

    /// A file in the cooked store moved: the next extract re-tests the held cook fallbacks.
    pub(crate) fn cook_store_moved(&mut self) {
        self.cook_resolved = false;
    }

    /// A scene save asks the next cache reader to evict what the world no longer names.
    pub(crate) fn asset_cache_prune_requested(&mut self) {
        self.prune_pending = true;
    }

    /// Sets whether the GI caches follow the world ([`GiRebakeMode::Auto`], the default) or hold the
    /// baseline they already have ([`GiRebakeMode::Hold`]) — see [`GiRebakeMode`] for why a live
    /// session wants the second.
    ///
    /// Session state, not sim state and not a [`QualitySetting`] axis: it never enters a
    /// `WorldHash`, a snapshot or a recording, so two engines at different settings simulate
    /// identically and a session held while recording replays on an `Auto` engine bit-for-bit. It
    /// takes effect from the next frame this engine extracts and renders.
    pub fn set_gi_rebake(&mut self, mode: GiRebakeMode) {
        self.gi_rebake = mode;
    }

    /// This host's GI rebake lever (see [`set_gi_rebake`](Engine::set_gi_rebake)).
    pub fn gi_rebake_mode(&self) -> GiRebakeMode {
        self.gi_rebake
    }

    /// Requests one rebake: the next frame runs with the hold suppressed, so both GI caches rebuild
    /// against the current world and that becomes the new held baseline. A no-op at
    /// [`GiRebakeMode::Auto`] (nothing is being held), and idempotent — two requests before a frame
    /// are one rebake.
    pub fn gi_rebake_now(&mut self) {
        self.gi_rebake_force = true;
    }

    /// Whether the last frame this engine produced showed *held* global illumination — a
    /// voxelization or a probe bake built under inputs the frame no longer had.
    ///
    /// Recomputed every frame from the two caches' own key comparisons, never latched: a world that
    /// moves away from the held baseline and back again reports stale and then clean. Always `false`
    /// at [`GiRebakeMode::Auto`], and on a frame whose scene authors no `GiVolume`.
    pub fn gi_stale(&self) -> bool {
        self.gi_stale
    }

    /// Whether the frame about to be produced holds the GI baseline: the session's lever, with a
    /// pending one-shot rebake suppressing it. The one place the two pieces of lever state combine,
    /// so the CPU cache and the GPU bake can never be handed different answers for one frame.
    fn gi_hold(&self) -> bool {
        matches!(self.gi_rebake, GiRebakeMode::Hold) && !self.gi_rebake_force
    }

    /// Records the finished frame's GI staleness from the two caches that answer to the lever, and
    /// consumes a pending one-shot rebake — whose whole definition is "one frame with the hold
    /// suppressed". Called at the end of every path that extracts *and* renders.
    fn finish_gi_frame(&mut self) {
        let bake_stale = self.renderer.as_ref().is_some_and(Renderer::gi_stale);
        self.gi_stale = self.asset_cache.gi_stale() || bake_stale;
        self.gi_rebake_force = false;
    }

    /// The [`FrameSize`] a render of `width` × `height` uses on this host: the requested output
    /// extent at the host's [`render_scale`](Engine::render_scale). The one place the host settings
    /// meet a render request, so every entry point resamples identically — and validates
    /// identically ([`FrameSize::validate`]).
    ///
    /// # Errors
    /// `L2012` if the output extent or its scale-derived render extent is out of range.
    pub(crate) fn frame_size(&self, width: u32, height: u32) -> Result<FrameSize, Diagnostic> {
        FrameSize::scaled(width, height, self.render_scale()).validate()
    }

    /// Checks that the host's render extent, at its render scale, is one the renderer can allocate —
    /// what a host calls once after applying `--render-size`, `--scale` and quality, so a bad pair
    /// refuses the start instead of failing on the first frame.
    ///
    /// # Errors
    /// `L2012` naming both extents and the limit.
    pub fn check_render_extent(&self) -> Result<(), Diagnostic> {
        let (width, height) = self.render_size;
        self.frame_size(width, height).map(drop)
    }

    // --- Observation ----------------------------------------------------------------------------

    /// The component registry.
    pub fn registry(&self) -> &Registry {
        &self.registry
    }

    /// The current world, for queries and assertions.
    pub fn world(&self) -> &World {
        self.driver.world()
    }

    /// The current world, mutably (for tests and command handlers).
    pub fn world_mut(&mut self) -> &mut World {
        self.driver.world_mut()
    }

    /// The next frame to be simulated — the value stamped on every response.
    pub fn frame(&self) -> FrameIndex {
        self.driver.frame()
    }

    /// Which frame-index run a frame belongs to. A `scene.load` or `engine.restore` starts a new
    /// one, because the index can move backwards; a scene switch keeps its frame and so its run.
    /// Host state: never hashed, snapshotted or recorded.
    pub fn frame_timeline(&self) -> u64 {
        self.frame_timeline
    }

    /// The gameplay frame at the same boundary as [`frame`](Self::frame): the next tick's
    /// `ctx.game_frame`, i.e. `frame` less every frame the world's `GameClock` has held (see
    /// `GameClock::game_frame`). Equals `frame` when the world has no clock.
    pub fn game_frame(&self) -> FrameIndex {
        let frame = self.frame();
        self.driver
            .world()
            .resource::<GameClock>()
            .map_or(frame, |c| c.game_frame(frame))
    }

    /// The current run-state (running / paused / stepping).
    pub fn run_state(&self) -> RunState {
        self.driver.run_state()
    }

    /// The current wall-clock pacing factor (live-mode only; never affects a tick).
    pub fn speed(&self) -> Speed {
        self.driver.speed()
    }

    /// The simulation tick rate the live driver runs at.
    pub(crate) fn tick_rate(&self) -> TickRate {
        self.driver.tick_rate()
    }

    /// The seed the current world was instantiated with (from the scene, default 0).
    pub fn seed(&self) -> u64 {
        self.seed
    }

    /// The fixed simulation timestep in seconds (`1 / tick_rate`) — used by the live scheduler to
    /// pace ticks by the wall clock. This is a constant of the run; it never varies with speed.
    pub fn seconds_per_tick(&self) -> f64 {
        self.driver.tick_rate().seconds_per_tick()
    }

    /// Performs at most one tick, as the current run-state allows, then drains the frame's
    /// [`AudioQueue`] into the audio log (and the wired sender). Returns whether a tick ran.
    ///
    /// This is the engine's single audio-draining tick primitive: both the live scheduler (see
    /// [`run_live`](crate::live::run_live)) and `sim.step` (via `step_ticks`)
    /// funnel through it, so the audio event stream is a pure function of the sim regardless of which
    /// path drove it. It delegates the tick itself to the driver's
    /// [`advance`](ludion_sim::SimDriver::advance), so `Running` free-runs and an in-flight `sim.step`
    /// decrements and pauses exactly as in every other path — the audio drain adds no stepping logic.
    ///
    /// While the live profiler is armed ([`perf_arm`](Engine::perf_arm)) this takes the observed
    /// path instead; the branch is one `Option` test, and the tick it performs is the same one (see
    /// the private `advance_live_profiled`).
    ///
    /// A [`TickObserver`] set with [`set_tick_observer`](Engine::set_tick_observer) watches here, so
    /// it sees every tick `sim.step` and the live loop perform.
    pub fn advance_live(&mut self) -> bool {
        if let Some(playback) = self.playback.take() {
            return self.advance_playback(playback);
        }
        self.before_tick();
        let Some(mut observer) = self.tick_observer.take() else {
            return self.advance_live_unwatched();
        };
        let will_tick = match self.driver.run_state() {
            RunState::Running => true,
            RunState::Stepping { remaining } => remaining > 0,
            _ => false,
        };
        let probe = will_tick
            && observer.before_tick(self.driver.frame(), self.driver.world(), &self.registry);
        let ran = if probe {
            let frame = self.driver.frame();
            let was_running = self.driver.run_state() == RunState::Running;
            // Disjoint field borrows: the registry is read by the observer while the driver ticks.
            let registry = &self.registry;
            let ran = self
                .driver
                .advance_probed(&mut |name, world| observer.after_system(name, world, registry));
            if ran {
                self.finish_live_tick(frame, was_running);
            }
            ran
        } else {
            self.advance_live_unwatched()
        };
        if ran {
            observer.after_tick(self.driver.frame(), self.driver.world(), &self.registry);
        }
        self.tick_observer = Some(observer);
        ran
    }

    /// Installs a read-only per-tick [`TickObserver`], replacing any previous one. The observer is
    /// host state: not in the world, not hashed, not snapshotted — a restore or scene load leaves it
    /// in place.
    pub fn set_tick_observer(&mut self, observer: impl TickObserver + 'static) {
        self.tick_observer = Some(Box::new(observer));
    }

    /// Removes the [`TickObserver`], if any.
    pub fn clear_tick_observer(&mut self) {
        self.tick_observer = None;
    }

    /// The tick [`advance_live`](Engine::advance_live) performs when no observer probes it.
    fn advance_live_unwatched(&mut self) -> bool {
        if self.perf.is_some() {
            return self.advance_live_profiled();
        }
        // The frame captured *before* the tick is the one whose systems run and emit events; the
        // driver advances the counter to `frame + 1` inside `advance`.
        let frame = self.driver.frame();
        // Whether this is a *free-running* tick (as opposed to a `sim.step`, which sets `Stepping`).
        // Only free-running ticks lack a command record of their own and need batching into the
        // recording — a `sim.step`'s ticks are already captured by its `SimStep` command record.
        let was_running = self.driver.run_state() == RunState::Running;
        let ran = self.driver.advance();
        if ran {
            self.finish_live_tick(frame, was_running);
        }
        ran
    }

    /// The armed twin of [`advance_live`](Engine::advance_live): the same tick with the live
    /// profiler's [`ScheduleObserver`](ludion_ecs::ScheduleObserver) attached and a wall-clock span
    /// around it.
    ///
    /// Both are pure observation — the observer cannot alter what runs and the span's result is
    /// written into host-side profiler state, never into the world — so this tick and the
    /// [`WorldHash`](Engine::world_hash) after it are bit-identical to the unarmed path's, and the
    /// recording it feeds is byte-identical (`live_perf_does_not_change_world_hash` and
    /// `profiled_recording_is_byte_identical` in [`crate::perf_live`] assert exactly that).
    fn advance_live_profiled(&mut self) -> bool {
        let frame = self.driver.frame();
        let was_running = self.driver.run_state() == RunState::Running;
        // Disjoint field borrows (the `advance_probed` precedent): the profiler's observer is
        // written by the schedule while the driver ticks.
        let perf = self.perf.as_mut().expect("the armed path holds a profiler");
        let observer = perf.begin_frame();
        let span = Span::start();
        let ran = self.driver.advance_observed(observer);
        let frame_total_ms = span.ms();
        if ran {
            self.perf
                .as_mut()
                .expect("the armed path holds a profiler")
                .end_frame(frame, frame_total_ms);
            self.finish_live_tick(frame, was_running);
        }
        ran
    }

    /// The frame-boundary work a *live* tick performs once the driver has advanced: the shared
    /// drains, then batching the tick into an active recording.
    fn finish_live_tick(&mut self, frame: FrameIndex, was_running: bool) {
        if self.drain_frame_boundary(frame) {
            // The panicking tick counts (the frame advanced), so a replay must run it: batch it, but
            // write no keyframe of the half-written world. The batch is flushed here with the
            // `system_panic` marker, so it ends at the panicking tick — the sim is paused.
            if was_running && let Some(mut recorder) = self.recorder.take() {
                recorder.pending_ticks += 1;
                match crate::recording::write_system_panic(&mut recorder, frame) {
                    Ok(()) => self.recorder = Some(recorder),
                    Err(e) => self.stop_recording_on_failure(&io_to_diagnostic(e)),
                }
            }
            return;
        }
        if was_running && self.recorder.is_some() {
            self.record_free_run_tick();
        }
        // After the tick is batched: a stop then ends the file on this tick, and a start's reset
        // leaves no pending tick of the old run in the new recording.
        if was_running {
            self.drain_run_recording(frame);
            self.drain_replay_playback();
        }
    }

    /// The frame-boundary drains every tick primitive performs after the driver advances: the caught
    /// system panic — which short-circuits the rest, see
    /// [`drain_system_panic`](Engine::drain_system_panic) — then the game's own reported
    /// [`Diagnostics`](crate::Diagnostics), audio, profile saves, run-save requests, a requested residue clear, a
    /// requested quit, and a pending scene switch. Returns whether a panic short-circuited the
    /// boundary.
    ///
    /// A quit requested in the same frame as a switch wins: the world the switch would rebuild is the
    /// one the host is about to tear down, and the rebuild would only wipe the pending quit.
    fn drain_frame_boundary(&mut self, frame: FrameIndex) -> bool {
        crate::prefab::sweep_presentation_parts(self.driver.world_mut());
        self.drain_prefab_texts();
        self.drain_debug_draw();
        if self.drain_system_panic() {
            return true;
        }
        self.drain_diagnostics();
        self.drain_audio(frame);
        self.sync_audio_emitters(frame);
        self.drain_profile(frame);
        self.drain_run_save(frame);
        self.take_host_requests();
        self.drain_residue_clear();
        self.drain_render_settings();
        self.drain_cursor_lock();
        if !self.drain_shutdown() {
            self.drain_scene_switch(frame);
        }
        false
    }

    /// Drains what game systems reported into [`Diagnostics`](crate::Diagnostics) this tick — and
    /// what `ludion-physics` queued as shape rejections and missing-joint carriers, drained into
    /// the same resource first, and the `L0028` phase-boundary refusals the world queued (whatever its
    /// callers did with the `Result`) — onto both sinks a reader can have: a `Warn` [`Event::Log`] for any
    /// `log`-channel subscriber, and the host's stderr for a headless or windowed run with no
    /// client attached.
    ///
    /// The once-per-`(code, key)` latch lives in the resource, so a system reporting every tick
    /// arrives here once; the queue is drained before [`drain_scene_switch`](Engine::drain_scene_switch)
    /// so nothing reported in the departing district is lost with it.
    fn drain_diagnostics(&mut self) {
        crate::diagnostics::report_boundary_crossings(self.driver.world_mut());
        crate::diagnostics::report_unregistered(self.driver.world_mut(), &self.registry);
        crate::hierarchy::report_hierarchy_faults(self.driver.world_mut());
        scene_physics::report_parented_bodies(self.driver.world_mut());
        scene_physics::report_scaled_colliders(self.driver.world_mut());
        crate::lane::report_without_motor(self.driver.world_mut());
        let (physics_installed, game_systems) =
            self.driver
                .system_names()
                .fold((false, false), |(physics, game), name| match name {
                    ludion_physics::STEP => (true, game),
                    ludion_physics::WRITE
                    | ludion_physics::MIRROR
                    | crate::path_follow::SYSTEM_NAME
                    | crate::pose_blend::SYSTEM_NAME
                    | crate::camera_rig::SYSTEM_NAME => (physics, game),
                    _ => (physics, true),
                });
        scene_physics::report_unrun_colliders(
            self.driver.world_mut(),
            physics_installed,
            game_systems,
        );
        crate::patterns::report_unrun_patterns(self.driver.world_mut(), game_systems);
        crate::projectiles::report_unrun_step(self.driver.world_mut(), game_systems);
        crate::fonts::report_font_faults(self.driver.world_mut());
        scene_physics::report_rejections(self.driver.world_mut());
        scene_physics::report_missing_joints(self.driver.world_mut());
        scene_physics::report_motor_mismatches(self.driver.world_mut());
        scene_physics::report_impulse_misses(self.driver.world_mut());
        scene_physics::report_physics_scale(self.driver.world_mut());
        self.emit_pending_diagnostics();
    }

    /// Sends what the [`Diagnostics`](crate::Diagnostics) resource holds to both sinks and empties it.
    fn emit_pending_diagnostics(&mut self) {
        let reported = match self.driver.world_mut().resource_mut::<crate::Diagnostics>() {
            Some(diagnostics) if !diagnostics.is_empty() => diagnostics.take(),
            _ => return,
        };
        for diagnostic in reported {
            eprintln!("{diagnostic}");
            self.push_event(Event::Log(LogEvent::from_diagnostic(diagnostic)));
        }
    }

    /// Takes the `L4007` diagnostic the driver holds for a caught system panic, adds an `Error` log
    /// event, and pauses. Returns whether one was found, so the tick's remaining frame-boundary work is
    /// skipped.
    ///
    /// The skip is the point. Those drains read the world the panic left half-written, and one of
    /// them — [`drain_scene_switch`](Engine::drain_scene_switch) — rebuilds it outright, which would
    /// destroy the very state the diagnostic is asking a reader to inspect; the recorder would also
    /// write a keyframe of it. The pause is what stops this repeating: a system that panicked on this
    /// world shape almost always panics on it again, and an unpaused sim would emit the same
    /// diagnostic every frame forever. Everything that does not tick still answers, so `entity.query`,
    /// `component.get`, and `sim.hash` inspect the failure directly.
    fn drain_system_panic(&mut self) -> bool {
        let Some(diagnostic) = self.driver.take_system_panic() else {
            return false;
        };
        self.push_event(Event::Log(LogEvent::from_diagnostic(diagnostic.clone())));
        self.session.last_system_panic = Some(diagnostic);
        self.driver.pause();
        true
    }

    /// Records one free-running live tick into the active session by batching it (see
    /// [`Recorder::pending_ticks`](crate::recording::Recorder)); at each keyframe boundary the batch
    /// is flushed as a `SimStep` record and a keyframe snapshot is written, so a live recording gains
    /// the same seek keyframes and frame-advancing records a lockstep recording has.
    ///
    /// This is the only recording I/O the free-run loop performs (the per-tick increment is just a
    /// counter). A write failure here has no waiting client, so it follows the shared
    /// `stop_recording_on_failure` policy — like a render failure
    /// in [`emit_egress`](crate::live), it is surfaced on the `log` channel — and recording is stopped
    /// so a broken sink is not retried every tick; the `.lrp` remains valid up to the last successful
    /// flush.
    fn record_free_run_tick(&mut self) {
        let Some(mut recorder) = self.recorder.take() else {
            return;
        };
        recorder.pending_ticks += 1;
        match self.free_run_keyframe(&mut recorder) {
            Ok(skipped) => {
                self.recorder = Some(recorder);
                self.report_keyframe_skip(skipped);
            }
            // recorder dropped (not restored) — recording ends here.
            Err(diagnostic) => self.stop_recording_on_failure(&diagnostic),
        }
    }

    /// Writes a keyframe (preceded by a `SimStep` flush of the batched free-run ticks) when the
    /// current frame has crossed the [`KEYFRAME_INTERVAL`] boundary — the free-run analogue of the
    /// keyframe half of [`write_record`](Engine::write_record). A no-op between boundaries. Returns
    /// the skip diagnostic when the keyframe was skipped (see
    /// [`keyframe_or_skip`](Engine::keyframe_or_skip)).
    fn free_run_keyframe(&self, recorder: &mut Recorder) -> Result<Option<Diagnostic>, Diagnostic> {
        let current = self.frame();
        if current.as_u64() >= recorder.last_keyframe.as_u64() + recorder.keyframe_interval {
            crate::recording::flush_pending_ticks(recorder, current).map_err(io_to_diagnostic)?;
            return self.keyframe_or_skip(recorder, current);
        }
        Ok(None)
    }

    /// Captures the world and writes it as a keyframe at `frame`. A world whose capture fails
    /// with `L0007` (a non-finite float, or another component serialize failure) skips the keyframe instead of ending the
    /// recording: nothing is written, `last_keyframe` moves to `frame` so the next attempt comes one
    /// interval later, and the diagnostic is returned for the `&mut self` caller to report. The
    /// delta and input records continue, and a replay seeks from the nearest earlier keyframe. Any
    /// other capture failure is returned as an error.
    fn keyframe_or_skip(
        &self,
        recorder: &mut Recorder,
        frame: FrameIndex,
    ) -> Result<Option<Diagnostic>, Diagnostic> {
        match crate::snapshot::capture(self) {
            Ok(snapshot) => {
                crate::recording::write_keyframe(recorder, frame, &snapshot)
                    .map_err(io_to_diagnostic)?;
                Ok(None)
            }
            Err(diagnostic) if diagnostic.code == code::COMPONENT_SERIALIZE_FAILED => {
                recorder.last_keyframe = frame;
                Ok(Some(diagnostic))
            }
            Err(diagnostic) => Err(diagnostic),
        }
    }

    /// Reports a keyframe skipped by [`keyframe_or_skip`](Engine::keyframe_or_skip) into `Diagnostics`,
    /// latched by its message so one report per entity, component and count.
    fn report_keyframe_skip(&mut self, skipped: Option<Diagnostic>) {
        if let Some(diagnostic) = skipped {
            let key = diagnostic.message.clone();
            crate::diagnostics::report_full(self.driver.world_mut(), key, diagnostic);
        }
    }

    /// Advances one tick with a [`ScheduleObserver`] attached, draining audio exactly as
    /// [`advance_live`](Engine::advance_live) does — the profile driver's tick primitive.
    ///
    /// The observer only watches per-system boundaries; it never enters sim state, so this tick — and
    /// the resulting [`WorldHash`](Engine::world_hash) — is bit-identical to
    /// [`advance_live`](Engine::advance_live)'s with no observer (the profile driver's determinism
    /// test asserts exactly this).
    pub(crate) fn advance_observed(
        &mut self,
        observer: &mut dyn ludion_ecs::ScheduleObserver,
    ) -> bool {
        self.before_tick();
        let frame = self.driver.frame();
        let ran = self.driver.advance_observed(observer);
        if ran {
            self.drain_frame_boundary(frame);
        }
        ran
    }

    /// Advances one tick with a per-system probe attached, draining exactly as
    /// [`advance_live`](Engine::advance_live) does — the divergence-bisect tick primitive.
    ///
    /// The probe sees each system's name, the world as that system left it, and the registry needed
    /// to read the world (hash it, serialize its components). Like the observer flavor above it never
    /// enters sim state, so the tick — and the resulting [`WorldHash`](Engine::world_hash) — is
    /// bit-identical to [`advance_live`](Engine::advance_live)'s.
    pub(crate) fn advance_probed(&mut self, probe: &mut crate::bisect::SystemProbe<'_>) -> bool {
        self.before_tick();
        let frame = self.driver.frame();
        // Disjoint field borrows: the registry is read by the probe while the driver ticks.
        let registry = &self.registry;
        let ran = self
            .driver
            .advance_probed(&mut |name, world| probe(name, world, registry));
        if ran {
            self.drain_frame_boundary(frame);
        }
        ran
    }

    /// The probed twin of [`step_ticks`](Engine::step_ticks) — the exact same step-then-drain loop,
    /// with [`advance_probed`](Engine::advance_probed) as its tick.
    pub(crate) fn step_ticks_probed(
        &mut self,
        ticks: u64,
        probe: &mut crate::bisect::SystemProbe<'_>,
    ) {
        self.driver.step(ticks);
        while self.advance_probed(probe) {}
    }

    /// The observed twin of [`step_ticks`](Engine::step_ticks) — the exact same step-then-drain loop,
    /// with [`advance_observed`](Engine::advance_observed) as its tick.
    pub(crate) fn step_ticks_observed(
        &mut self,
        ticks: u64,
        observer: &mut dyn ludion_ecs::ScheduleObserver,
    ) {
        self.driver.step(ticks);
        while self.advance_observed(observer) {}
    }

    /// Steps exactly `ticks` ticks then pauses, draining audio each tick — the audio-aware
    /// counterpart of the driver's [`run_ticks`](ludion_sim::SimDriver::run_ticks) that `sim.step`
    /// uses, so a stepped run produces the same frame-stamped audio stream a live run would. Returns
    /// how many ticks ran — fewer than `ticks` when a system panic paused the sim, the panicking tick
    /// included.
    pub(crate) fn step_ticks(&mut self, ticks: u64) -> u64 {
        self.driver.step(ticks);
        let mut ran = 0;
        while self.advance_live() {
            ran += 1;
        }
        ran
    }

    /// Drains the world's [`AudioQueue`] into the frame-stamped log and, if wired, the sink sender.
    ///
    /// `frame` is the frame whose systems emitted the events (captured before the tick advanced the
    /// counter). Order is preserved: the log entries are in push order, so the stream is deterministic.
    /// A positional one-shot ([`AudioQueue::play_at`]) is resolved here against the listener and
    /// logged as a plain one-shot at its attenuated gain, or dropped when inaudible or with no camera.
    fn drain_audio(&mut self, frame: FrameIndex) {
        let (queued, non_finite) = match self.driver.world_mut().resource_mut::<AudioQueue>() {
            Some(queue) if !queue.is_empty() => (queue.take(), queue.take_non_finite()),
            _ => return,
        };
        if !non_finite.is_empty()
            && let Some(sink) = self.driver.world_mut().resource_mut::<crate::Diagnostics>()
        {
            for sound in non_finite {
                let message = format!(
                    "`{sound}` was queued with a gain that is NaN or infinite; it plays at gain 0"
                );
                sink.report(code::AUDIO_GAIN_NON_FINITE, sound, message);
            }
        }
        let listener = queued
            .iter()
            .any(|item| matches!(item, Queued::At { .. }))
            .then(|| audio_listener(self.driver.world_mut()))
            .flatten();
        for item in queued {
            match item {
                Queued::Message(message) => self.emit_audio(frame, message),
                Queued::At {
                    mut event,
                    position,
                    range,
                } => {
                    let Some((gain, pan)) = listener.as_ref().and_then(|listener| {
                        audible_voice(
                            event.gain,
                            range,
                            listener,
                            position,
                            self.audio_distance,
                            self.audio_pan,
                        )
                    }) else {
                        continue;
                    };
                    event.gain = gain;
                    event.pan = pan;
                    self.emit_audio(frame, AudioMessage::OneShot(event));
                }
            }
        }
    }

    /// Forwards one audio message to the wired sink and records it in the frame-stamped log — the
    /// single path every emitted message takes, so the log and the sink never disagree.
    fn emit_audio(&mut self, frame: FrameIndex, message: AudioMessage) {
        if let Some(sender) = &self.audio_sender {
            // A dropped/closed sink receiver just means playback stopped; the log still records it.
            let _ = sender.send(message.clone());
        }
        if self.session.audio_log.len() >= AUDIO_LOG_CAPACITY {
            self.session.audio_log.pop_front();
        }
        self.session
            .audio_log
            .push_back(AudioLogEntry { frame, message });
    }

    /// Keeps every [`AudioEmitter`](crate::AudioEmitter) in the world synced to its distance from the
    /// listener, sending a sync only when it says something new, and stops the ones that stopped
    /// being audible — the declarative half of the audio seam, run at the frame boundary beside
    /// [`drain_audio`](Engine::drain_audio) so a game registers no system for it.
    ///
    /// The listener is the lowest-`EntityId` entity carrying a [`Camera`](crate::Camera); with none,
    /// the step is a no-op for the tick (it does not stop what is already sounding — a world between
    /// cameras is not a world that went quiet). Emitters are visited in ascending `EntityId`, so both
    /// the gains and the message order are a pure function of the world.
    ///
    /// An emitter syncs when it becomes audible, then again only when its gain moves past
    /// [`RESYNC_GAIN_RATIO`](crate::audio::RESYNC_GAIN_RATIO) of the last gain sent, its pan moves
    /// past [`RESYNC_PAN_STEP`](crate::audio::RESYNC_PAN_STEP), or its sound or bus changes; the sink
    /// holds the loop in between. Under [`AudioPan::Centred`] the pan is always `0.0`, so it never
    /// triggers a sync.
    ///
    /// The liveness diff is what makes despawning an emitter enough: an id that was audible last tick
    /// and is not now — despawned, muted below the audibility epsilon, moved out of range, or with its
    /// component removed — gets a `LoopStop`. Stops precede syncs so a recycled `EntityId` cannot have
    /// its fresh sync undone by the departing entity's stop.
    fn sync_audio_emitters(&mut self, frame: FrameIndex) {
        let Some(listener) = audio_listener(self.driver.world_mut()) else {
            return;
        };
        let mut audible = StableSet::new();
        let mut syncs = Vec::new();
        // A typed join over the `AudioEmitter`/`Transform` storages, not a per-entity probe of every
        // live entity — free in a scene with none, ascending `EntityId` like the walk it replaces.
        let ids: Vec<EntityId> = self
            .driver
            .world_mut()
            .query_ids::<(&AudioEmitter, &Transform)>()
            .map(|(id, _)| id)
            .collect();
        let world = self.driver.world();
        for id in ids {
            let Ok(Some(emitter)) = world.get::<AudioEmitter>(id) else {
                continue;
            };
            let Some(at) = crate::hierarchy::world_transform(world, id) else {
                continue;
            };
            let Some((gain, pan)) = audible_voice(
                emitter.gain,
                emitter.range,
                &listener,
                at.trs.translation,
                self.audio_distance,
                self.audio_pan,
            ) else {
                continue;
            };
            audible.insert(id);
            let sent = self.audible_emitters.get(&id);
            if sent.is_some_and(|sent| sent.is_current(&emitter.sound, gain, pan, emitter.category))
            {
                continue;
            }
            self.audible_emitters.insert(
                id,
                SentLoop {
                    sound: emitter.sound.clone(),
                    gain,
                    pan,
                    category: emitter.category,
                },
            );
            syncs.push(AudioMessage::LoopSync {
                id: LoopId::Entity(id),
                sound: emitter.sound.clone(),
                gain,
                pan,
                category: emitter.category,
            });
        }
        let stops: Vec<EntityId> = self
            .audible_emitters
            .keys()
            .filter(|id| !audible.contains(id))
            .copied()
            .collect();
        for id in &stops {
            self.audible_emitters.remove(id);
        }
        for id in stops {
            self.emit_audio(
                frame,
                AudioMessage::LoopStop {
                    id: LoopId::Entity(id),
                },
            );
        }
        for sync in syncs {
            self.emit_audio(frame, sync);
        }
    }

    /// Drains the profile save request at the frame boundary, beside [`drain_audio`](Engine::drain_audio):
    /// if a game system called [`Profile::save`](crate::Profile::save) since the last drain, clear the
    /// flag, push a frame-stamped [`ProfileSaveEntry`] into the
    /// [`profile_save_log`](Engine::profile_save_log), and — only when a host has enabled persistence —
    /// render the profile to canonical text and write it crash-durably to the resolved save path.
    ///
    /// Like the audio drain, the log is a pure function of the sim, so a replay regenerates the
    /// identical stream and it is never recorded. The disk write is out-of-sim and gated: a
    /// replay/CI/test run leaves persistence off (the log still fills), so it never clobbers the
    /// developer's real save. A write failure is a non-fatal `L1122` stored on the engine
    /// ([`profile_save_error`](Engine::profile_save_error)) and printed on the `log` channel and
    /// stderr, so a save that silently stops persisting is visible; it never panics or stops the sim, the
    /// whole save stays on disk, and the next save request retries (a `scene.load` before that retry
    /// reverts to the disk copy and loses the unsaved value).
    fn drain_profile(&mut self, frame: FrameIndex) {
        let Some(kind) = self.profile_kind.as_ref() else {
            return;
        };
        if !kind.take_dirty(self.driver.world_mut()) {
            return;
        }
        if self.session.profile_save_log.len() >= PROFILE_SAVE_LOG_CAPACITY {
            self.session.profile_save_log.pop_front();
        }
        self.session
            .profile_save_log
            .push_back(ProfileSaveEntry { frame });
        if !self.profile_persistence {
            return;
        }
        let Some(text) = kind.emit(self.driver.world()) else {
            return;
        };
        let path = self.save_root().join(&self.profile_rel_path);
        match ludion_core::atomic::write_durable(&path, &text) {
            Ok(()) => self.session.profile_save_error = None,
            Err(error) => {
                let diagnostic = Diagnostic::new(
                    ludion_core::codes::code::PROFILE_SAVE_FAILED,
                    format!(
                        "the player profile could not be written to `{}`: {error}",
                        path.display()
                    ),
                )
                .with_help(
                    "a whole save stays on disk (the new bytes are synced to a hidden `.tmp` sibling \
                     before they replace it), and the next `profile.save()` retries; a `scene.load` \
                     before that retry starts a fresh session from that file, so the unsaved value \
                     is lost; check the save location is writable",
                );
                // Both sinks, once per failed save — a save is an explicit game request, so there is
                // no per-frame flood to latch against (the `drain_diagnostics` pair).
                eprintln!("{diagnostic}");
                self.push_event(Event::Log(LogEvent::from_diagnostic(diagnostic.clone())));
                self.session.profile_save_error = Some(diagnostic);
            }
        }
    }

    /// Drains the run-save requests at the frame boundary, straight after
    /// [`drain_profile`](Engine::drain_profile): a [`RunSave::discard`] runs before a
    /// [`RunSave::suspend`], then a [`RunSave::resume`], each pushing a frame-stamped
    /// [`RunSaveEntry`]. Only a persisting live host touches disk: a suspend writes the snapshot
    /// envelope crash-durably, a discard removes the file, a resume restores it in place (strictly:
    /// any refusal is an `L1401`, a backup and [`ResumeOutcome::Refused`], with the world untouched);
    /// elsewhere only `pending` changes and a resume is only logged. A write failure is a non-fatal
    /// `L1400` on both sinks, the previous whole file stays, and the next request retries. Runs before
    /// the shutdown and switch drains, so a suspend in a quit or switch frame captures the departing
    /// world.
    fn drain_run_save(&mut self, frame: FrameIndex) {
        let Some(state) = self.run_save.as_ref() else {
            return;
        };
        let Some(resource) = self.driver.world_mut().resource_mut::<RunSave>() else {
            return;
        };
        let discard = std::mem::take(&mut resource.discard);
        let suspend = std::mem::take(&mut resource.suspend);
        let resume = std::mem::take(&mut resource.resume);
        if !discard && !suspend && !resume {
            return;
        }
        let stamp = state.stamp.clone();
        let persisting = self.profile_persistence && !self.profile_off_disk;
        let path = self.save_root().join(crate::run_save::RUN_SAVE_PATH);
        let mut pending = state.pending.clone();
        let mut suspended = false;
        for (wanted, request) in [
            (discard, RunSaveRequest::Discard),
            (suspend, RunSaveRequest::Suspend),
            (resume, RunSaveRequest::Resume),
        ] {
            if !wanted {
                continue;
            }
            if self.session.run_save_log.len() >= RUN_SAVE_LOG_CAPACITY {
                self.session.run_save_log.pop_front();
            }
            self.session
                .run_save_log
                .push_back(RunSaveEntry { frame, request });
            let outcome = match request {
                // Runs after the loop: it rebuilds the world.
                RunSaveRequest::Resume => continue,
                RunSaveRequest::Discard => {
                    let removed = if persisting {
                        crate::run_save::remove_file(&path)
                    } else {
                        Ok(())
                    };
                    removed.map(|()| None)
                }
                RunSaveRequest::Suspend => {
                    let info = SuspendInfo {
                        frame,
                        scene: self.scene_name().map(str::to_string),
                        saved_at_build: stamp.clone(),
                    };
                    let written = if persisting {
                        crate::snapshot::capture(self)
                            .map_err(|error| {
                                format!(
                                    "the run could not be captured for `{}`: {error}",
                                    path.display()
                                )
                            })
                            .and_then(|snapshot| crate::run_save::write(&path, &info, snapshot))
                    } else {
                        Ok(())
                    };
                    written.map(|()| Some(info))
                }
            };
            match outcome {
                Ok(new_pending) => {
                    pending = new_pending;
                    if persisting {
                        self.session.run_save_error = None;
                        suspended = suspended || request == RunSaveRequest::Suspend;
                    }
                }
                Err(message) => self.run_save_failed(message),
            }
        }
        // The recorder is still mid-batch here; `drain_run_recording` finalizes it after the tick.
        if suspended
            && let Some(state) = self.run_recording.as_mut()
            && state.active.is_some()
        {
            state.stop = true;
            state.suspended = true;
        }
        if let Some(state) = self.run_save.as_mut() {
            state.pending.clone_from(&pending);
        }
        if let Some(resource) = self.driver.world_mut().resource_mut::<RunSave>() {
            resource.set_pending(pending);
        }
        if resume && persisting {
            let recording = self
                .run_recording
                .as_ref()
                .is_some_and(|state| state.active.is_some());
            if recording {
                self.refuse_resume_kept(
                    "a run recording is open: stop() it before resume()".to_string(),
                );
            } else {
                self.resume_run(&path, &stamp, frame);
            }
        }
    }

    /// Refuses a resume without consuming anything: an `L1401` on both sinks and
    /// [`ResumeOutcome::Refused`], with `pending` and the file untouched and no backup.
    fn refuse_resume_kept(&mut self, reason: String) {
        let diagnostic = Diagnostic::new(
            ludion_core::codes::code::RUN_SAVE_RESUME_REFUSED,
            reason.clone(),
        )
        .with_help("the suspended run is kept; call `RunRecording::stop()`, then `resume()` again");
        eprintln!("{diagnostic}");
        self.push_event(Event::Log(LogEvent::from_diagnostic(diagnostic.clone())));
        self.session.run_save_error = Some(diagnostic);
        let outcome = ResumeOutcome::Refused { reason };
        if let Some(state) = self.run_save.as_mut() {
            state.last_resume = Some(outcome.clone());
        }
        if let Some(resource) = self.driver.world_mut().resource_mut::<RunSave>() {
            resource.set_last_resume(Some(outcome));
        }
    }

    /// Restores the suspended run at `path` in place, or refuses it. Runs last in
    /// [`drain_run_save`](Engine::drain_run_save) because a restore replaces the world. Held and
    /// pending input are cleared; on success the file is deleted and `pending` is `None`.
    fn resume_run(
        &mut self,
        path: &Path,
        stamp: &crate::run_save::BuildStamp,
        request_frame: FrameIndex,
    ) {
        // Consumed first: a failed delete after the restore cannot then give a second resume.
        let parked = match crate::run_save::consume(path) {
            Ok(parked) => parked,
            Err(reason) => {
                self.refuse_resume(path, reason, false);
                return;
            }
        };
        let read_from = parked.as_deref().unwrap_or(path);
        let (frame, snapshot) = match crate::run_save::read_resume(read_from, stamp) {
            Ok(read) => read,
            Err(reason) => {
                self.refuse_consumed(path, parked.as_deref(), reason);
                return;
            }
        };
        // The host's pacing outlives the resume: a paused or stepping host stays so.
        let host_state = (self.driver.run_state(), self.driver.speed());
        // Before the restore: `install_restored` mirrors this state into the fresh world.
        if let Some(state) = self.run_save.as_mut() {
            state.pending = None;
            state.last_resume = Some(ResumeOutcome::Resumed { frame });
        }
        if let Err(diagnostic) =
            crate::snapshot::restore_into(self, &snapshot, crate::snapshot::DriftPolicy::Refuse)
        {
            self.refuse_consumed(path, parked.as_deref(), diagnostic.message);
            return;
        }
        let (run_state, speed) = host_state;
        match run_state {
            RunState::Running => self.driver.resume(),
            RunState::Paused => self.driver.pause(),
            RunState::Stepping { remaining } => self.driver.step(remaining),
        }
        self.driver.set_speed(speed);
        self.set_current_scene_path(None);
        self.push_event(Event::SceneChanged(SceneChangedData {
            kind: SceneChangeKind::Restore,
            path: None,
            request_id: self.current_request(),
        }));
        // The restore adopted the snapshot's session and cleared the log; keep the request visible.
        self.session.run_save_log.push_back(RunSaveEntry {
            frame: request_frame,
            request: RunSaveRequest::Resume,
        });
        // Nothing was parked when the file was missing, and that read already refused above.
        if let Some(parked) = parked {
            match crate::run_save::remove_file(&parked) {
                Ok(()) => self.session.run_save_error = None,
                Err(message) => self.report_run_save_failure(
                    message,
                    "the run resumed; a stale `suspend.json.resuming` remains and the next \
                     persisting boot backs it up",
                ),
            }
        }
    }

    /// Refuses a resume whose file was parked by [`consume`](crate::run_save::consume): the file
    /// goes back under its name and is backed up beside it, like any refusal.
    fn refuse_consumed(&mut self, path: &Path, parked: Option<&Path>, reason: String) {
        let Some(parked) = parked else {
            self.refuse_resume(path, reason, false);
            return;
        };
        match std::fs::rename(parked, path) {
            Ok(()) => self.refuse_resume(path, reason, true),
            Err(error) => {
                let reason = format!(
                    "{reason}; the file could not be restored for a backup ({error}), so it was left as `{}`",
                    parked.display()
                );
                self.refuse_resume(path, reason, false);
            }
        }
    }

    /// Reports an `L1401` on both sinks, optionally backs the file up beside itself, and records the
    /// refusal; the world is untouched.
    fn refuse_resume(&mut self, path: &Path, reason: String, backup: bool) {
        let mut reason = reason;
        if backup && let Err(error) = crate::player_profile::backup_file(path) {
            reason = format!(
                "{reason}; the file could not be backed up ({error}), so it was left in place"
            );
        }
        let diagnostic = Diagnostic::new(
            ludion_core::codes::code::RUN_SAVE_RESUME_REFUSED,
            reason.clone(),
        )
        .with_help("start a new run; the original is kept as `suspend.json.bak.<hash>` beside it");
        eprintln!("{diagnostic}");
        self.push_event(Event::Log(LogEvent::from_diagnostic(diagnostic.clone())));
        self.session.run_save_error = Some(diagnostic);
        let outcome = ResumeOutcome::Refused { reason };
        if let Some(state) = self.run_save.as_mut() {
            state.pending = None;
            state.last_resume = Some(outcome.clone());
        }
        if let Some(resource) = self.driver.world_mut().resource_mut::<RunSave>() {
            resource.set_pending(None);
            resource.set_last_resume(Some(outcome));
        }
    }

    /// Reports an `L1400` on both sinks (stderr and the `log` channel) and keeps it as the last
    /// run-save error.
    fn run_save_failed(&mut self, message: String) {
        self.report_run_save_failure(
            message,
            "the run continues and a previous whole `suspend.json` stays on disk; \
             the next `suspend()` retries; check the save location is writable",
        );
    }

    /// [`run_save_failed`](Engine::run_save_failed) with its own help text.
    fn report_run_save_failure(&mut self, message: String, help: &str) {
        let diagnostic = Diagnostic::new(ludion_core::codes::code::RUN_SAVE_WRITE_FAILED, message)
            .with_help(help);
        eprintln!("{diagnostic}");
        self.push_event(Event::Log(LogEvent::from_diagnostic(diagnostic.clone())));
        self.session.run_save_error = Some(diagnostic);
    }

    /// Computes the build stamp, reads the suspended run (persisting hosts only) and sweeps foreign
    /// temps beside it; called once at build when [`run_save`](crate::EngineBuilder::run_save) is on.
    pub(crate) fn init_run_save(&mut self) {
        let stamp = crate::run_save::BuildStamp {
            engine_version: ENGINE_VERSION.to_string(),
            game_schema: crate::run_save::game_schema_hash(&self.registry, &self.doc_kinds),
            projectiles: self
                .projectile_config
                .as_ref()
                .map(crate::run_save::projectile_config_hash),
        };
        let mut pending = None;
        if self.profile_persistence && !self.profile_off_disk {
            let path = self.save_root().join(crate::run_save::RUN_SAVE_PATH);
            crate::player_profile::sweep_temps(&path);
            crate::run_save::backup_stale_resuming(&path);
            pending = crate::run_save::read_pending(&path);
        }
        self.run_save = Some(RunSaveState {
            stamp,
            pending,
            last_resume: None,
        });
        self.install_run_save();
    }

    /// (Re)installs the [`RunSave`] resource from the engine's state; a no-op without
    /// [`run_save`](crate::EngineBuilder::run_save). Called after every world (re)build, since each
    /// wipes the resources.
    fn install_run_save(&mut self) {
        if let Some(state) = self.run_save.as_ref() {
            self.driver
                .world_mut()
                .insert_resource(RunSave::mirror(state, self.playback_name.is_some()));
        }
    }

    /// Records the game's version string for [`game_identity`](Engine::game_identity); called once at build.
    pub(crate) fn set_game_version(&mut self, version: Option<String>) {
        self.game_version = version;
    }

    /// The identity of this game build: the hash of the registered component and data-doc schemas
    /// (engine built-ins included) and the [`game_version`](crate::EngineBuilder::game_version).
    /// A replay menu compares it to `ReplayReader::header().game` to list a recording without
    /// replaying it; `None` there means a session recording or a file from before the field.
    pub fn game_identity(&self) -> ludion_protocol::replay::GameIdentity {
        ludion_protocol::replay::GameIdentity {
            schema: crate::run_save::game_schema_hash(&self.registry, &self.doc_kinds),
            version: self.game_version.clone(),
        }
    }

    /// Installs the run-recording state; called once at build when
    /// [`run_recordings`](crate::EngineBuilder::run_recordings) is on. No disk I/O.
    pub(crate) fn init_run_recording(&mut self, config: crate::RunRecordingConfig) {
        self.run_recording = Some(RunRecordingState {
            config,
            active: None,
            last_finished: None,
            log: VecDeque::new(),
            error: None,
            stop: false,
            start: None,
            suspended: false,
        });
        self.install_run_recording();
    }

    /// (Re)installs the [`RunRecording`] resource from the engine's state; a no-op without
    /// [`run_recordings`](crate::EngineBuilder::run_recordings). Called after every world (re)build.
    fn install_run_recording(&mut self) {
        if let Some(state) = self.run_recording.as_ref() {
            self.driver
                .world_mut()
                .insert_resource(RunRecording::mirror(state, self.playback_name.is_some()));
        }
    }

    /// The frame-stamped run-recording requests drained so far, in request order, bounded to the most
    /// recent entries. It outlives a run's reset.
    pub fn run_recording_log(&self) -> Option<&VecDeque<RunRecordingEntry>> {
        self.run_recording.as_ref().map(|state| &state.log)
    }

    /// The most recent run-recording failure (`L1402`), or `None` if the last request succeeded.
    pub fn run_recording_error(&self) -> Option<&Diagnostic> {
        self.run_recording.as_ref()?.error.as_ref()
    }

    /// Where the recording called `name` is published: `<save_root>/<dir>/<name>.lrp`.
    pub fn run_recording_path(&self, name: &str) -> PathBuf {
        let dir = self
            .run_recording
            .as_ref()
            .map_or("", |state| state.config.dir.as_str());
        self.save_root().join(dir).join(format!("{name}.lrp"))
    }

    /// The scene text a run recording restarts from: the inline document the session holds. Only
    /// the [`Profile`](crate::Profile) and run-save state carry across the reset; entities,
    /// resources, residue and doc reloads drop.
    fn run_start_scene_text(&self) -> Option<String> {
        self.current_scene_text()
    }

    /// Moves the run-recording and replay-playback requests off the world's resources into engine
    /// state, before a scene switch rebuilds the world and wipes them (as
    /// [`drain_run_save`](Engine::drain_run_save) runs before it for the same reason). They are
    /// applied at the end of the tick, by [`drain_run_recording`](Engine::drain_run_recording) and
    /// [`drain_replay_playback`](Engine::drain_replay_playback).
    fn take_host_requests(&mut self) {
        if let Some(state) = self.run_recording.as_mut()
            && let Some(resource) = self.driver.world_mut().resource_mut::<RunRecording>()
        {
            state.stop |= std::mem::take(&mut resource.stop);
            if let Some(name) = resource.start.take() {
                state.start = Some(name);
            }
        }
        if let Some(state) = self.replay_playback.as_mut()
            && let Some(resource) = self.driver.world_mut().resource_mut::<ReplayPlayback>()
        {
            if let Some(play) = resource.play.take() {
                state.play = Some(play);
            }
            state.refresh |= std::mem::take(&mut resource.refresh);
            state.delete.append(&mut resource.delete);
        }
    }

    /// Applies the run-recording requests at the end of a free-running tick (see
    /// [`RunRecording`]): a stop publishes the file, then a start restarts the run from the scene
    /// text at frame 0 and arms the ordinary recorder. Only a persisting host resets or touches
    /// disk; elsewhere the requests are only logged. Every failure is a non-fatal `L1402`.
    fn drain_run_recording(&mut self, frame: FrameIndex) {
        let Some(state) = self.run_recording.as_mut() else {
            return;
        };
        let stop = std::mem::take(&mut state.stop);
        let start = state.start.take();
        state.suspended = false;
        let persisting = self.profile_persistence && !self.profile_off_disk;
        let died = self
            .run_recording
            .as_ref()
            .is_some_and(|state| state.active.is_some())
            && self.recorder.is_none();
        if died {
            let failure = self.recording_failure.take().unwrap_or_else(|| {
                Diagnostic::new(code::RECORDING_STOPPED, "the recorder is gone")
            });
            self.publish_run(Some(&failure));
        }
        if !stop && start.is_none() {
            if died {
                self.install_run_recording();
            }
            return;
        }
        let mut requests = Vec::new();
        if stop {
            requests.push(RunRecordingRequest::Stop);
        }
        if let Some(name) = &start {
            requests.push(RunRecordingRequest::Start(name.clone()));
        }
        if let Some(state) = self.run_recording.as_mut() {
            for request in requests {
                if state.log.len() >= RUN_RECORDING_LOG_CAPACITY {
                    state.log.pop_front();
                }
                state.log.push_back(RunRecordingEntry { frame, request });
            }
        }
        if persisting {
            if stop
                && self
                    .run_recording
                    .as_ref()
                    .is_some_and(|s| s.active.is_some())
            {
                let closed = self.close_recorder();
                self.publish_run(closed.as_ref().err());
            }
            if let Some(name) = start {
                self.start_run_recording(&name);
            }
        }
        self.install_run_recording();
    }

    /// The start half of [`drain_run_recording`](Engine::drain_run_recording).
    fn start_run_recording(&mut self, name: &str) {
        let active = self
            .run_recording
            .as_ref()
            .is_some_and(|state| state.active.is_some());
        if active {
            let closed = self.close_recorder();
            self.publish_run(closed.as_ref().err());
        }
        if let Err(reason) = crate::run_recording::validate_name(name) {
            self.run_recording_failed(
                format!("run recording not started: {reason}"),
                "use a plain file-safe name; the run was not restarted",
            );
            return;
        }
        if self.recorder.is_some() {
            self.run_recording_failed(
                format!(
                    "run recording `{name}` not started: a session recording is running and wins"
                ),
                "stop the `--record` session first; the run was not restarted",
            );
            return;
        }
        let Some(text) = self.run_start_scene_text() else {
            self.run_recording_failed(
                format!("run recording `{name}` not started: the scene has no inline text to restart from"),
                "load the scene with `scene.load` or `load_inline_scene`; the run was not restarted",
            );
            return;
        };
        let path = self.run_recording_path(name);
        let host_state = (self.current_scene_path.clone(), self.driver.speed());
        if let Err(errors) = self.load_scene_carrying_profile(text) {
            let detail = errors
                .first()
                .map_or_else(String::new, |d| d.message.clone());
            self.run_recording_failed(
                format!("run recording `{name}` not started: the scene did not reload: {detail}"),
                "the run was not restarted",
            );
            return;
        }
        self.current_scene_path.clone_from(&host_state.0);
        self.driver.set_speed(host_state.1);
        self.push_event(Event::SceneChanged(SceneChangedData {
            kind: SceneChangeKind::Load,
            path: host_state.0,
            request_id: None,
        }));
        let temp = ludion_core::atomic::temp_sibling(&path);
        let opened = path
            .parent()
            .map_or(Ok(()), std::fs::create_dir_all)
            .and_then(|()| std::fs::File::create(&temp))
            .map_err(|error| format!("{}: {error}", temp.display()));
        let armed = opened.and_then(|file| {
            let interval = self
                .run_recording
                .as_ref()
                .map_or(1, |state| state.config.keyframe_every);
            let game = Some(self.game_identity());
            self.arm_recorder(Box::new(std::io::BufWriter::new(file)), interval, game)
                .map_err(|diagnostic| diagnostic.message)
        });
        match armed {
            Ok(()) => {
                if let Some(state) = self.run_recording.as_mut() {
                    state.active = Some(ActiveRun {
                        name: name.to_string(),
                        temp,
                        path,
                    });
                    state.error = None;
                }
            }
            Err(message) => {
                let _ = std::fs::remove_file(&temp);
                self.run_recording_failed(
                    format!(
                        "run recording `{name}` could not be written: {message}; the run was restarted but is not recorded"
                    ),
                    "check the replays directory is writable and has free space",
                );
            }
        }
    }

    /// Installs the replay-playback state; called once at build when
    /// [`replay_playback`](crate::EngineBuilder::replay_playback) is on. No disk I/O.
    pub(crate) fn init_replay_playback(&mut self, config: crate::ReplayPlaybackConfig) {
        self.replay_playback = Some(ReplayPlaybackState {
            config,
            last_outcome: None,
            entries: Vec::new(),
            error: None,
            play: None,
            refresh: false,
            delete: Vec::new(),
        });
        self.install_replay_playback();
    }

    /// (Re)installs the [`ReplayPlayback`] resource from the engine's state; a no-op without
    /// [`replay_playback`](crate::EngineBuilder::replay_playback). Called after every world (re)build.
    fn install_replay_playback(&mut self) {
        if let Some(state) = self.replay_playback.as_ref() {
            self.driver
                .world_mut()
                .insert_resource(ReplayPlayback::mirror(
                    state,
                    self.playback_name.as_deref(),
                    self.playback_name.is_some(),
                ));
        }
    }

    /// The name of the recording playing in place, if any.
    pub fn playing(&self) -> Option<&str> {
        self.playback_name.as_deref()
    }

    /// The most recent playback problem (`L1405`), or `None` if the last request succeeded.
    pub fn replay_playback_error(&self) -> Option<&Diagnostic> {
        self.replay_playback.as_ref()?.error.as_ref()
    }

    /// Where the recording called `name` lives: `<save_root>/<dir>/<name>.lrp`.
    pub fn replay_path(&self, name: &str) -> PathBuf {
        let dir = self
            .replay_playback
            .as_ref()
            .map_or("", |state| state.config.dir.as_str());
        self.save_root().join(dir).join(format!("{name}.lrp"))
    }

    /// Whether the live loop should idle: the sim is paused and no replay is playing. A replay's own
    /// `sim.step`s leave the driver paused between ticks, so the loop must keep ticking through one;
    /// a paused playback keeps ticking too, so the controls hook runs each tick period.
    pub fn pacing_paused(&self) -> bool {
        self.playback_name.is_none() && self.driver.run_state() == RunState::Paused
    }

    /// Ends the playback in progress at the current replay frame and returns to its `return_to`
    /// scene with the pre-play profile. A no-op when nothing plays.
    pub fn stop_playback(&mut self) {
        let Some(playback) = self.playback.take() else {
            return;
        };
        let outcome = PlaybackOutcome::Stopped {
            name: playback.name.clone(),
            frame: self.frame(),
        };
        self.return_from_playback(playback, outcome);
    }

    /// Holds the playing replay at its frame. A no-op when nothing plays.
    pub fn pause_playback(&mut self) {
        if let Some(playback) = self.playback.as_mut() {
            playback.paused = true;
        }
    }

    /// Continues a paused playback. A no-op when nothing plays.
    pub fn resume_playback(&mut self) {
        if let Some(playback) = self.playback.as_mut() {
            playback.paused = false;
        }
    }

    /// Sets the pace of the playing replay at once. A no-op when nothing plays.
    pub fn set_playback_speed(&mut self, speed: PlaybackSpeed) {
        if let Some(playback) = self.playback.as_mut() {
            playback.speed = speed;
            self.apply_playback_speed(speed);
        }
    }

    /// Whether the playing replay is held; `false` when nothing plays.
    pub fn playback_paused(&self) -> bool {
        self.playback.as_ref().is_some_and(|p| p.paused)
    }

    /// The pace of the playing replay, or `None` when nothing plays.
    pub fn playback_speed(&self) -> Option<PlaybackSpeed> {
        self.playback.as_ref().map(|p| p.speed)
    }

    fn apply_playback_speed(&mut self, speed: PlaybackSpeed) {
        if let Ok(speed) = Speed::new(speed.factor()) {
            self.driver.set_speed(speed);
        }
    }

    /// Applies the replay-playback requests at the end of a free-running tick (see
    /// [`ReplayPlayback`]): deletes, then a refresh, then a play. Only a persisting host touches disk
    /// or the world; elsewhere the requests are dropped.
    fn drain_replay_playback(&mut self) {
        let Some(state) = self.replay_playback.as_mut() else {
            return;
        };
        let play = state.play.take();
        let refresh = std::mem::take(&mut state.refresh);
        let deletes = std::mem::take(&mut state.delete);
        if play.is_none() && !refresh && deletes.is_empty() {
            return;
        }
        if self.profile_persistence && !self.profile_off_disk {
            let deleted = !deletes.is_empty();
            for name in deletes {
                self.delete_replay(&name);
            }
            if refresh || deleted {
                self.refresh_replay_entries();
            }
            if let Some((name, return_to)) = play {
                self.start_playback(&name, &return_to);
            }
        }
        self.install_replay_playback();
    }

    /// Removes `<name>.lrp`; a missing file is already what was asked for.
    fn delete_replay(&mut self, name: &str) {
        if let Err(reason) = crate::run_recording::validate_name(name) {
            self.playback_failed(
                format!("replay not deleted: {reason}"),
                "use a plain file-safe name",
            );
            return;
        }
        let path = self.replay_path(name);
        match std::fs::remove_file(&path) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => self.playback_failed(
                format!("replay `{name}` not deleted: {}: {error}", path.display()),
                "check that the file is not open elsewhere and the directory is writable",
            ),
        }
    }

    /// Lists the replays directory into the playback state, sorted by name. A missing directory is an
    /// empty list.
    fn refresh_replay_entries(&mut self) {
        let dir = self.replay_path("x").with_file_name("");
        let mut entries = Vec::new();
        if let Ok(read) = std::fs::read_dir(&dir) {
            for item in read.flatten() {
                let path = item.path();
                if path.extension().and_then(|e| e.to_str()) != Some("lrp") {
                    continue;
                }
                let Some(name) = path.file_stem().and_then(|s| s.to_str()) else {
                    continue;
                };
                if crate::run_recording::validate_name(name).is_err() {
                    continue;
                }
                let bytes = item.metadata().map_or(0, |m| m.len());
                let refusal = self.replay_refusal(&path).err();
                entries.push(ReplayEntry {
                    name: name.to_string(),
                    bytes,
                    refusal,
                });
            }
        }
        entries.sort_by(|a, b| a.name.cmp(&b.name));
        if let Some(state) = self.replay_playback.as_mut() {
            state.entries = entries;
        }
    }

    /// `Err` with why this build cannot replay the file at `path`: unreadable, malformed,
    /// incompatible, or recorded by another build of the game.
    fn replay_refusal(&self, path: &Path) -> Result<(), Diagnostic> {
        let bytes = std::fs::read(path).map_err(|error| {
            Diagnostic::new(
                code::REPLAY_PLAYBACK_FAILED,
                format!("could not read `{}`: {error}", path.display()),
            )
        })?;
        let reader = ludion_protocol::replay::ReplayReader::new(&bytes)?;
        crate::replay_compat::check_compatible(reader.header())?;
        crate::replay_compat::check_game(reader.header(), &self.game_identity())
    }

    /// Everything a start checks before it may touch the world.
    fn playback_preflight(&self, name: &str, return_to: &str) -> Result<PlaybackPlan, Diagnostic> {
        let refused = |message: String, help: &str| {
            Diagnostic::new(code::REPLAY_PLAYBACK_FAILED, message).with_help(help.to_string())
        };
        if let Err(reason) = crate::run_recording::validate_name(name) {
            return Err(refused(
                format!("replay not played: {reason}"),
                "use the file stem of a recording in the replays directory",
            ));
        }
        let path = self.replay_path(name);
        let bytes = std::fs::read(&path).map_err(|error| {
            refused(
                format!("replay `{name}` not played: {}: {error}", path.display()),
                "list the recordings with `ReplayPlayback::refresh` and play one of those",
            )
        })?;
        let reader = ludion_protocol::replay::ReplayReader::new(&bytes)?;
        crate::replay_compat::check_compatible(reader.header())?;
        let mut scan = crate::replay::scan_replay(&reader, crate::replay::Keyframes::Skip)?;
        crate::replay_compat::check_game(reader.header(), &self.game_identity())?;
        let doc_texts = crate::docs::resolve_doc_texts(
            &self.doc_kinds,
            std::mem::take(&mut scan.inline_docs).into_iter().collect(),
        )?;
        if self.recorder.is_some() {
            return Err(refused(
                format!("replay `{name}` not played: a recording is running"),
                "stop the `--record` session or the run recording first",
            ));
        }
        if self.playback_name.is_some() {
            return Err(refused(
                format!("replay `{name}` not played: another replay is playing"),
                "stop the current playback first",
            ));
        }
        let resolved = crate::commands::scene::resolve_in_root(self, return_to).map_err(|d| {
            refused(
                format!("replay `{name}` not played: return scene: {}", d.message),
                "name a scene file below the content root as `return_to`",
            )
        })?;
        let return_text = std::fs::read_to_string(&resolved).map_err(|error| {
            refused(
                format!(
                    "replay `{name}` not played: return scene `{}` is unreadable: {error}",
                    resolved.display()
                ),
                "name a scene file that exists as `return_to`",
            )
        })?;
        Ok(PlaybackPlan {
            header: reader.header().clone(),
            scan,
            doc_texts,
            resolved,
            return_text,
        })
    }

    /// The start half of [`drain_replay_playback`](Engine::drain_replay_playback): every check runs
    /// before the first mutation, so a refusal leaves the live world untouched.
    fn start_playback(&mut self, name: &str, return_to: &str) {
        let plan = match self.playback_preflight(name, return_to) {
            Ok(plan) => plan,
            Err(diagnostic) => {
                self.playback_refused(name, diagnostic);
                return;
            }
        };
        let PlaybackPlan {
            header,
            scan,
            doc_texts,
            resolved,
            return_text,
        } = plan;
        let saved = SavedHost {
            doc_texts: std::mem::replace(&mut self.doc_texts, doc_texts),
            profile_off_disk: self.profile_off_disk,
            profile_text: self.emit_profile_text(),
            speed: self.driver.speed(),
            render_size: self.render_size,
            return_to: return_to.to_string(),
            resolved,
            return_text,
            fallback_path: self.current_scene_path.clone(),
            fallback_text: self.current_scene_text(),
        };
        let active = ActivePlayback {
            name: name.to_string(),
            cursor: crate::replay::ActionCursor::new(scan.actions),
            frames: 0,
            saved,
            live_input: InputState::default(),
            pending_live: Vec::new(),
            paused: false,
            speed: PlaybackSpeed::X1,
        };
        self.profile_off_disk = true;
        self.playback_name = Some(name.to_string());
        if let Some(state) = self.replay_playback.as_mut() {
            state.last_outcome = None;
            state.error = None;
        }
        if let Err(diagnostic) =
            crate::replay::install_replay(self, &header, scan.scene_text, scan.prefab_reads)
        {
            let outcome = PlaybackOutcome::Failed {
                name: name.to_string(),
                diagnostic: diagnostic.clone(),
            };
            self.playback_failed(
                format!("replay `{name}` could not start: {}", diagnostic.message),
                "the engine returned to the `return_to` scene",
            );
            self.return_from_playback(active, outcome);
            return;
        }
        self.push_event(Event::SceneChanged(SceneChangedData {
            kind: SceneChangeKind::Load,
            path: None,
            request_id: None,
        }));
        self.driver.set_speed(active.saved.speed);
        self.playback = Some(active);
    }

    /// One live tick of a playing replay: the cursor runs to the next tick of the shared driver.
    fn advance_playback(&mut self, mut playback: ActivePlayback) -> bool {
        playback.live_input.begin_tick();
        for event in std::mem::take(&mut playback.pending_live) {
            playback.live_input.apply(event);
        }
        if let Some(mut hook) = self.playback_hook.take() {
            let mut controls = PlaybackControls::new(playback.paused, playback.speed);
            hook(&playback.live_input, &mut controls);
            self.playback_hook = Some(hook);
            if controls.stop_requested() {
                let outcome = PlaybackOutcome::Stopped {
                    name: playback.name.clone(),
                    frame: self.frame(),
                };
                self.return_from_playback(playback, outcome);
                return false;
            }
            playback.paused = controls.paused();
            if controls.speed() != playback.speed {
                playback.speed = controls.speed();
                self.apply_playback_speed(playback.speed);
            }
        }
        self.driver.world_mut().insert_resource(PlaybackInput {
            input: playback.live_input.clone(),
        });
        if playback.paused {
            self.playback = Some(playback);
            return false;
        }
        let ticked = playback
            .cursor
            .advance(self, &mut crate::replay::PlainDrive);
        if matches!(ticked, Ok(crate::replay::Advance::Ticked)) {
            self.apply_playback_speed(playback.speed);
        }
        match ticked {
            Ok(crate::replay::Advance::Ticked) => {
                playback.frames += 1;
                self.playback = Some(playback);
                true
            }
            Ok(_) => {
                let outcome = PlaybackOutcome::Finished {
                    name: playback.name.clone(),
                    frames: playback.frames,
                };
                self.return_from_playback(playback, outcome);
                false
            }
            Err(diagnostic) => {
                let outcome = PlaybackOutcome::Failed {
                    name: playback.name.clone(),
                    diagnostic: diagnostic.clone(),
                };
                self.playback_failed(
                    format!("replay `{}` failed: {}", playback.name, diagnostic.message),
                    "the engine returned to the `return_to` scene",
                );
                self.return_from_playback(playback, outcome);
                false
            }
        }
    }

    /// Ends a playback: restores the host state, reloads the `return_to` scene carrying the pre-play
    /// profile, disarms the replay's off-disk maps and records `outcome`.
    fn return_from_playback(&mut self, playback: ActivePlayback, outcome: PlaybackOutcome) {
        let SavedHost {
            doc_texts,
            profile_off_disk,
            profile_text,
            speed,
            render_size,
            return_to,
            resolved,
            return_text,
            fallback_path,
            fallback_text,
        } = playback.saved;
        self.doc_texts = doc_texts;
        self.profile_off_disk = profile_off_disk;
        self.playback = None;
        self.playback_name = None;
        self.replay_scene_text = None;
        self.replay_prefab_text = None;
        self.replay_prefab_epoch = 0;
        self.replay_prefab_next_epoch = 0;
        self.set_render_size(render_size.0, render_size.1);
        match self.load_scene_with_profile(return_text.clone(), profile_text.clone()) {
            Ok(_) => {
                self.set_scene_base(resolved, return_text);
                self.set_current_scene_path(Some(return_to));
            }
            Err(errors) => {
                let detail = errors
                    .first()
                    .map_or_else(String::new, |d| d.message.clone());
                self.playback_failed(
                    format!("return scene `{return_to}` did not load: {detail}"),
                    "the scene from before the replay was reloaded instead",
                );
                if let Some(text) = fallback_text
                    && self.load_scene_with_profile(text, profile_text).is_ok()
                {
                    self.set_current_scene_path(fallback_path);
                }
            }
        }
        self.push_event(Event::SceneChanged(SceneChangedData {
            kind: SceneChangeKind::Load,
            path: self.current_scene_path.clone(),
            request_id: None,
        }));
        self.driver.set_speed(speed);
        if let Some(state) = self.replay_playback.as_mut() {
            state.last_outcome = Some(outcome);
        }
        self.install_replay_playback();
    }

    /// Records a refused start as the outcome and reports it.
    fn playback_refused(&mut self, name: &str, diagnostic: Diagnostic) {
        self.playback_note(
            diagnostic.message.clone(),
            "no replay started; the world is unchanged",
        );
        if let Some(state) = self.replay_playback.as_mut() {
            state.error = Some(diagnostic.clone());
            state.last_outcome = Some(PlaybackOutcome::Refused {
                name: name.to_string(),
                diagnostic,
            });
        }
    }

    /// Reports an `L1405` and keeps it as the last playback error.
    fn playback_failed(&mut self, message: String, help: &str) {
        let diagnostic = self.playback_note(message, help);
        if let Some(state) = self.replay_playback.as_mut() {
            state.error = Some(diagnostic);
        }
    }

    /// Reports an `L1405` on both sinks.
    fn playback_note(&mut self, message: String, help: &str) -> Diagnostic {
        let diagnostic =
            Diagnostic::new(code::REPLAY_PLAYBACK_FAILED, message).with_help(help.to_string());
        eprintln!("{diagnostic}");
        self.push_event(Event::Log(LogEvent::from_diagnostic(diagnostic.clone())));
        diagnostic
    }

    /// Ends the active run's file: publishes it durably when `closed_with` is `None`, otherwise
    /// removes the temp and reports why it did not finish.
    fn publish_run(&mut self, closed_with: Option<&Diagnostic>) {
        let Some(run) = self
            .run_recording
            .as_mut()
            .and_then(|state| state.active.take())
        else {
            return;
        };
        let outcome = match closed_with {
            Some(diagnostic) => Err(format!(
                "the recording stopped early: {}: {}",
                diagnostic.code.as_str(),
                diagnostic.message
            )),
            None => ludion_core::atomic::commit_durable(&run.temp, &run.path)
                .map_err(|error| format!("{}: {error}", run.path.display())),
        };
        match outcome {
            Ok(()) => {
                if let Some(state) = self.run_recording.as_mut() {
                    state.last_finished = Some(run.name);
                    state.error = None;
                }
            }
            Err(message) => {
                let _ = std::fs::remove_file(&run.temp);
                self.run_recording_failed(
                    format!("run recording `{}` did not finish: {message}", run.name),
                    "no file was published; a previous recording of that name is untouched",
                );
            }
        }
    }

    /// Reports an `L1402` on both sinks and keeps it as the last run-recording error.
    fn run_recording_failed(&mut self, message: String, help: &str) {
        let diagnostic =
            Diagnostic::new(code::RUN_RECORDING_FAILED, message).with_help(help.to_string());
        eprintln!("{diagnostic}");
        self.push_event(Event::Log(LogEvent::from_diagnostic(diagnostic.clone())));
        if let Some(state) = self.run_recording.as_mut() {
            state.error = Some(diagnostic);
        }
    }

    /// Drains a requested residue clear at the frame boundary: if a game system called
    /// [`ResidueStore::clear`](crate::ResidueStore::clear) since the last drain, the session forgets
    /// every scene it has visited and the world's mirror is reinstalled emptied — so the next return
    /// to any district loads it fresh from its text.
    ///
    /// Runs **before** [`drain_scene_switch`](Engine::drain_scene_switch), which is what makes a clear
    /// and a switch requested in the same frame compose: the map is emptied first, then the switch
    /// captures the departing district into it — a run restarting in place and immediately travelling
    /// keeps the scene it is leaving *now* and forgets the run before it.
    ///
    /// Like the audio and profile drains this is a pure function of the sim (a system's request), so a
    /// replay regenerates it: nothing about it is recorded, and nothing about it enters the
    /// `WorldHash` — the residue map is engine-side session state either way.
    fn drain_residue_clear(&mut self) {
        let requested = self
            .driver
            .world_mut()
            .resource_mut::<crate::ResidueStore>()
            .is_some_and(crate::ResidueStore::take_clear);
        if !requested {
            return;
        }
        self.session.residue.clear();
        self.install_residue_store();
    }

    /// Moves the game's pending [`RenderSettings`](crate::RenderSettings) request onto the engine, which
    /// outlives every world rebuild. Render-only and a pure function of the sim: nothing about it is
    /// recorded, hashed or snapshotted, and the next render pushes it to the renderer.
    fn drain_render_settings(&mut self) {
        let requested = self
            .driver
            .world_mut()
            .resource_mut::<crate::RenderSettings>()
            .and_then(crate::RenderSettings::take);
        if let Some(choice) = requested {
            self.anti_aliasing_game = choice;
        }
    }

    /// Moves the game's pending [`CursorLock`](crate::CursorLock) request onto the engine and sends it
    /// to the armed host channel when it differs from the last one. Host state: nothing about it is
    /// recorded, hashed or snapshotted.
    fn drain_cursor_lock(&mut self) {
        let requested = self
            .driver
            .world_mut()
            .resource_mut::<crate::CursorLock>()
            .and_then(crate::CursorLock::take);
        if let Some(value) = requested
            && self.cursor_lock_game != Some(value)
        {
            self.cursor_lock_game = Some(value);
            if let Some(sender) = &self.cursor_lock_sender {
                let _ = sender.send(value);
            }
        }
    }

    /// Drains a quit a game system asked for through the [`Shutdown`](crate::Shutdown) resource,
    /// returning whether one was pending.
    ///
    /// The request routes into [`request_shutdown`](Engine::request_shutdown) — the very path the wire
    /// `engine.shutdown` handler takes — so a keyboard quit and a wire quit are the same event: the
    /// live loop leaves its loop, an armed host wakes into its graceful teardown, and a bare
    /// in-process engine only observes [`shutdown_requested`](Engine::shutdown_requested).
    ///
    /// Like the residue clear this is a pure function of the sim, so a replay regenerates it: nothing
    /// about it is recorded (`engine.shutdown` is not a recorded command either — a quit mutates no
    /// world state, and the recording simply ends at the frame it stops the session), and nothing
    /// about it enters the `WorldHash`.
    fn drain_shutdown(&mut self) -> bool {
        let requested = self
            .driver
            .world_mut()
            .resource_mut::<crate::Shutdown>()
            .is_some_and(crate::Shutdown::take);
        if requested && self.playback_name.is_some() {
            self.playback_note(
                "a quit requested by the replayed game was dropped".to_string(),
                "the replay is not the host's session; close the window to quit",
            );
            return false;
        }
        if requested {
            self.request_shutdown();
        }
        requested
    }

    /// Drains a pending in-run scene switch at the frame boundary, after
    /// [`drain_residue_clear`](Engine::drain_residue_clear):
    /// if a game system called [`SceneSwitch::request`](crate::SceneSwitch) since the last drain, resolve
    /// the target district's text from disk and rebuild the world into it, continuing the session (see
    /// [`crate::transition::perform`]).
    ///
    /// Runs **last** at the boundary so a `profile.save()` requested in the same frame is written
    /// before the world is rebuilt. Every failure — the target text unresolvable, or a `perform` reject
    /// (`L3018`/`L3019`/`L3029`) — is a non-fatal warning stored on
    /// [`last_transition_error`](Engine::last_transition_error) and printed by
    /// [`report_transition`](Engine::report_transition): the live world is untouched and the sim
    /// runs on in the current district. A switch that *succeeds* but skipped a dead carry id leaves an
    /// `L3020` warning there too; only a fully-clean switch clears it.
    ///
    /// The target text is resolved off-disk under replay and from disk live: when
    /// [`replay_scene_text`](Engine::replay_scene_text) is `Some` (a replay), the target comes from that
    /// pre-scanned map — **never disk**, for cross-machine purity — and a map miss takes the same
    /// rejection path a live disk-read failure takes. When it is `None` (a live run), the target is read
    /// from `<project_root>/<scene_path>` and, if a recorder is active and the switch succeeded (clean
    /// *or* `L3020`-degraded — the switch happened either way), written as a `SceneSwitch` record
    /// carrying that disk text inline. A *rejected* switch records nothing: under replay the map miss
    /// reproduces the identical rejection.
    fn drain_scene_switch(&mut self, frame: FrameIndex) {
        let switch = match self
            .driver
            .world_mut()
            .resource_mut::<crate::transition::SceneSwitch>()
            .and_then(crate::transition::SceneSwitch::take)
        {
            None => return,
            Some(crate::transition::PendingTransition::Restart { carry }) => {
                self.restart_scene(carry);
                return;
            }
            Some(crate::transition::PendingTransition::Switch(switch)) => switch,
        };
        let target_text = match &self.replay_scene_text {
            // Replay: the target text is the embedded record's, never disk. A miss is the same world
            // no-op an unreadable live target is — the switch is rejected and the sim continues.
            Some(map) => match map.get(&switch.scene_path) {
                Some(text) => text.clone(),
                None => {
                    self.report_transition(Some(target_unresolved_off_disk(&switch.scene_path)));
                    return;
                }
            },
            // Live: read the target from disk.
            None => {
                if let Some(refusal) = ludion_core::paths::content_refusal(&switch.scene_path) {
                    self.report_transition(Some(target_outside_root(
                        &switch.scene_path,
                        refusal.describe(),
                    )));
                    return;
                }
                let path = self.project_root.join(&switch.scene_path);
                match std::fs::read_to_string(&path) {
                    Ok(text) => text,
                    Err(error) => {
                        self.report_transition(Some(target_unresolved_on_disk(&path, &error)));
                        return;
                    }
                }
            }
        };
        let recording = self.replay_scene_text.is_none();
        let scene_path = switch.scene_path.clone();
        // A rejection (`Err`) and a skipped-carry warning (`Ok(Some)`) both land on the error channel;
        // only a fully-clean switch (`Ok(None)`) clears it. The rejection diagnostic wins by construction
        // — `perform` only returns `Ok` after the swap succeeded.
        let outcome = crate::transition::perform(self, switch, &target_text);
        // A live, recording run writes the switch as a record when it succeeded (both the clean and the
        // `L3020`-degraded cases — the switch happened). A rejected switch writes no record.
        let record_error = if recording && outcome.is_ok() {
            self.record_scene_switch(frame, &scene_path, &target_text)
        } else {
            None
        };
        if outcome.is_ok() {
            if recording {
                self.set_scene_base(self.project_root.join(&scene_path), target_text);
            }
            self.current_scene_path = Some(scene_path.clone());
        }
        self.finish_transition(outcome, record_error, Some(scene_path));
    }

    /// Performs a requested restart: rebuilds the current scene from the text the session holds,
    /// never from disk, and writes no record (a replay holds the same text and the rebuild stays in
    /// its prefab epoch). A session with no inline scene text rejects it, live and under replay alike.
    fn restart_scene(&mut self, carry: Vec<EntityId>) {
        let Some(text) = self.current_scene_text() else {
            self.report_transition(Some(crate::transition::restart_no_scene()));
            return;
        };
        let outcome = crate::transition::perform_restart(self, carry, &text);
        let path = self.current_scene_path.clone();
        self.finish_transition(outcome, None, path);
    }

    /// The tail both transitions share: the world was replaced, so `scene.changed` subscribers hear
    /// of it, then the outcome lands on the error channel. The event names `path`, which joins onto
    /// `project_root` like a `scene.load`'s; a switch path may hold `..` (`content_refusal`), which
    /// `scene.save` refuses with `L3040`.
    fn finish_transition(
        &mut self,
        outcome: Result<Option<Diagnostic>, Diagnostic>,
        record_error: Option<Diagnostic>,
        path: Option<String>,
    ) {
        if outcome.is_ok() {
            self.push_event(Event::SceneChanged(SceneChangedData {
                kind: SceneChangeKind::Switch,
                path,
                request_id: None,
            }));
        }
        let outcome = match outcome {
            Err(diagnostic) => Some(diagnostic),
            // A clean transition surfaces only a recording IO failure (if any); a degraded one keeps
            // its `L3020` warning (a recording failure on top of it is unlikely and lower-priority).
            Ok(warning) => warning.or(record_error),
        };
        self.report_transition(outcome);
    }

    /// The scene text the session holds: the inline document every load, switch and restore leaves in
    /// `scene`. `None` for a `Path` reference or no scene, which a restart rejects.
    fn current_scene_text(&self) -> Option<String> {
        match &self.scene {
            Some(SceneRef::Inline { document }) => Some(document.clone()),
            Some(SceneRef::Path { .. }) | None => None,
        }
    }

    /// Stores a transition outcome on [`last_transition_error`](Engine::last_transition_error) and,
    /// for anything but a fully-clean switch, prints it on the two sinks a reader can have — an
    /// [`Event::Log`] for a `log`-channel subscriber and the host's stderr — exactly as
    /// [`drain_diagnostics`](Engine::drain_diagnostics) does for a game's own reports.
    ///
    /// The print is latched on the line itself, so a game that re-requests a rejected switch every
    /// frame it stands in the gate zone reports it once instead of once per frame; a different
    /// outcome, including a clean switch, clears the latch. The level follows the code's registered
    /// severity, so a rejection reads as an error and a degraded-but-performed switch as a warning.
    fn report_transition(&mut self, outcome: Option<Diagnostic>) {
        let line = outcome.as_ref().map(ToString::to_string);
        if line != self.session.last_transition_report {
            if let (Some(line), Some(diagnostic)) = (&line, &outcome) {
                eprintln!("{line}");
                self.push_event(Event::Log(LogEvent::from_diagnostic(diagnostic.clone())));
            }
            self.session.last_transition_report = line;
        }
        self.session.last_transition_error = outcome;
    }

    /// Writes a `SceneSwitch` record for a just-performed live switch, if a recording session is
    /// active — the [`record`](Engine::record) mold: take the recorder out so its `&mut` does not alias
    /// the `&self` reads. Returns a non-fatal `Diagnostic` on a recording IO error (surfaced on
    /// [`last_transition_error`](Engine::last_transition_error) rather than panicking mid-tick), or
    /// `None` when there is no recorder or the write succeeded.
    fn record_scene_switch(
        &mut self,
        frame: FrameIndex,
        scene_path: &str,
        document: &str,
    ) -> Option<Diagnostic> {
        let mut recorder = self.recorder.take()?;
        let result =
            crate::recording::write_scene_switch(&mut recorder, frame, scene_path, document);
        self.recorder = Some(recorder);
        result.err().map(io_to_diagnostic)
    }

    /// Installs the replay off-disk scene-text source (see [`replay_scene_text`](Engine::replay_scene_text)),
    /// putting [`drain_scene_switch`](Engine::drain_scene_switch) in replay mode. Called by
    /// [`prepare_replay`](crate::prepare_replay) before every drive, even when the replay carries no
    /// `SceneSwitch` record, so a replay never reads a switch target off disk.
    pub(crate) fn install_replay_scene_text(&mut self, scene_text: BTreeMap<String, String>) {
        self.replay_scene_text = Some(scene_text);
    }

    /// Installs a fresh [`AudioQueue`] resource into the current world so game systems can push audio
    /// events, and stops every loop the departing world started. Called after every world (re)build;
    /// the resource is excluded from the world hash and from snapshots.
    ///
    /// The blanket stop is a stop, not a diff: a loop's playback state lives only in the sink, so the
    /// rebuilt world has nothing to diff against. It goes through the ordinary emit path, so the log
    /// records it too — a `scene.load` and a restore then clear that log as they clear the rest of the
    /// session, while a district switch (which continues the session) keeps it in the stream.
    ///
    /// The emitter map resets with it, for the same reason: the blanket stop has already silenced
    /// every emitter the departing world was sounding, so carrying its ids forward would only emit a
    /// redundant stop each — against entity handles the rebuilt world may have recycled — and its
    /// sent gains would swallow the syncs the rebuilt world needs.
    fn install_audio_queue(&mut self) {
        self.driver
            .world_mut()
            .insert_resource(AudioQueue::default());
        self.audible_emitters.clear();
        let frame = self.frame();
        self.emit_audio(frame, AudioMessage::StopAllLoops);
    }

    /// Declares what a presentation system may touch on the new world: the diagnostics and audio
    /// outboxes and the prefab library (the only resources it may write, the library only through
    /// `spawn_prefab`), and `Name` as the label its refusals quote.
    /// Called after every world (re)build — the [`install_audio_queue`](Engine::install_audio_queue)
    /// mold.
    fn install_presentation_rules(&mut self) {
        let world = self.driver.world_mut();
        world.allow_presentation_writes::<crate::Diagnostics>();
        world.allow_presentation_writes::<AudioQueue>();
        world.allow_presentation_writes::<crate::prefab::PrefabLibrary>();
        world.allow_presentation_writes::<crate::patterns::PatternSystemsRan>();
        world.set_entity_label::<crate::components::Name>();
    }

    /// Installs a fresh (empty) [`SceneSwitch`] resource into the current world so game systems can
    /// request an in-run district change. Called after every world (re)build — the
    /// [`install_audio_queue`](Engine::install_audio_queue) mold, including inside a switch itself, so
    /// the freshly-built district starts with no pending switch. Excluded from the world hash and from
    /// snapshots.
    fn install_scene_switch(&mut self) {
        self.driver
            .world_mut()
            .insert_resource(crate::transition::SceneSwitch::default());
    }

    /// Installs the configured [`Projectiles`](crate::Projectiles) pool into the current world unless
    /// it already holds one (a restore's, or one a `carry_resource` moved across a switch). Called
    /// after every world (re)build; a no-op for a game that configured none.
    pub(crate) fn install_projectiles(&mut self) {
        let Some(config) = &self.projectile_config else {
            return;
        };
        if self
            .driver
            .world()
            .resource::<crate::Projectiles>()
            .is_some()
        {
            return;
        }
        if let Ok(projectiles) = crate::Projectiles::new(config.clone()) {
            self.driver.world_mut().insert_resource(projectiles);
        }
    }

    /// Installs the configured [`Nav`](crate::Nav) into the current world unless it already holds one
    /// (one a `carry_resource` moved across a switch). Called after every world (re)build, so a
    /// restored world gets a fresh grid; a no-op for a game that configured none.
    pub(crate) fn install_nav(&mut self) {
        let Some(config) = &self.nav_config else {
            return;
        };
        if self.driver.world().resource::<crate::Nav>().is_some() {
            return;
        }
        if let Ok(nav) = crate::Nav::new(config.clone()) {
            self.driver.world_mut().insert_resource(nav);
        }
    }

    /// Installs a fresh (empty) [`Shutdown`](crate::Shutdown) resource into the current world so game
    /// systems can ask the host to quit. Called after every world (re)build — the
    /// [`install_scene_switch`](Engine::install_scene_switch) mold, so a freshly-built district starts
    /// with no pending quit. Excluded from the world hash and from snapshots.
    fn install_shutdown(&mut self) {
        self.driver
            .world_mut()
            .insert_resource(crate::Shutdown::default());
    }

    /// Installs a fresh (empty) [`RenderSettings`](crate::RenderSettings) resource into the current
    /// world, the [`install_shutdown`](Engine::install_shutdown) mold. Excluded from the world hash and
    /// from snapshots.
    fn install_render_settings(&mut self) {
        self.driver
            .world_mut()
            .insert_resource(crate::RenderSettings::default());
    }

    /// Installs a fresh (empty) [`CursorLock`](crate::CursorLock) resource into the current world, the
    /// [`install_render_settings`](Engine::install_render_settings) mold.
    fn install_cursor_lock(&mut self) {
        self.driver
            .world_mut()
            .insert_resource(crate::CursorLock::default());
    }

    /// Installs a fresh (empty) [`DebugDraw`](crate::DebugDraw) resource into the current world. Called
    /// after every world (re)build, the [`install_shutdown`](Engine::install_shutdown) mold; excluded
    /// from the world hash and from snapshots.
    fn install_debug_draw(&mut self) {
        self.driver
            .world_mut()
            .insert_resource(crate::DebugDraw::default());
    }

    /// Ends the debug-draw tick: what systems drew becomes what the renderer shows. First in the frame
    /// boundary, so a tick that panicked cannot leave a stale pending drawing behind.
    fn drain_debug_draw(&mut self) {
        if let Some(draw) = self.driver.world_mut().resource_mut::<crate::DebugDraw>() {
            draw.end_tick();
        }
    }

    /// Installs the [`GameClock`] resource — the clock a game holds while it is frozen, and the one
    /// every closed-form evaluator reads. Called after every world (re)build (the
    /// [`install_audio_queue`](Engine::install_audio_queue) mold) with a default (unheld) clock, so
    /// a `scene.load` and a district switch start gameplay time level with sim time; a restore
    /// passes the snapshotted clock instead, because a restore is an exact continuation. A game that
    /// wants a hold to survive a switch declares
    /// [`carry_resource::<GameClock>`](crate::EngineBuilder::carry_resource).
    ///
    /// Installs nothing when the world already carries a clock, which is exactly the
    /// `carry_resource` case: the carriers run before this on a switch, and a game that asked for
    /// its hold to survive must not have it reset underneath. A resource, so excluded from the
    /// `WorldHash` by construction.
    fn install_game_clock(&mut self, clock: GameClock) {
        let world = self.driver.world_mut();
        if world.resource::<GameClock>().is_none() {
            world.insert_resource(clock);
        }
    }

    /// Installs the [`Diagnostics`](crate::Diagnostics) resource on the current world so game
    /// systems can report, moving `carried` — the departing world's once-per-`(code, key)` latch —
    /// onto it. Called after every world (re)build (the
    /// [`install_audio_queue`](Engine::install_audio_queue) mold). Excluded from the world hash and
    /// from snapshots.
    ///
    /// The latch travels because "once per session" is a property of the session, not of a world: a
    /// run that walks back and forth through a gate would otherwise re-print the same fault at every
    /// crossing.
    fn install_diagnostics(&mut self, mut carried: crate::Diagnostics) {
        carried.set_scope(self.scene_scope());
        self.driver.world_mut().insert_resource(carried);
    }

    /// The loaded scene's identity for [`offender_key`](crate::diagnostics::offender_key): its
    /// `name:`, else its path, else the content hash of its inline text, else `""` at boot.
    fn scene_scope(&self) -> String {
        if let Some(name) = &self.scene_name {
            return name.clone();
        }
        match &self.scene {
            Some(SceneRef::Path { path }) => path.clone(),
            Some(SceneRef::Inline { document }) => {
                format!("{:016x}", ludion_assets::content_hash(document.as_bytes()))
            }
            None => String::new(),
        }
    }

    /// Mirrors this session's [`render_size`](Engine::render_size) onto the current world as the
    /// [`RenderExtent`](crate::RenderExtent) resource, so a game system can map a cursor pixel back
    /// into the world without naming an extent of its own. Called after every world (re)build (the
    /// [`install_audio_queue`](Engine::install_audio_queue) mold) and from every writer of the
    /// field. Excluded from the world hash; a snapshot carries the extent in its own
    /// `render_extent` field.
    fn install_render_extent(&mut self) {
        let (width, height) = self.render_size;
        self.driver
            .world_mut()
            .insert_resource(crate::RenderExtent { width, height });
    }

    /// Mirrors this game's [`plane`](Engine::plane) onto the current world as the [`Plane`] resource,
    /// which is what [`spatial`](ludion_spatial::spatial)'s queries read. Called after every world
    /// (re)build (the [`install_audio_queue`](Engine::install_audio_queue) mold) — a `scene.load` or
    /// `restore` wipes the resources, and the plane is game configuration, not world content, so it is
    /// reinstated rather than snapshotted.
    pub(crate) fn install_plane(&mut self) {
        let plane = self.plane;
        self.driver.world_mut().insert_resource(plane);
    }

    /// Installs an empty [`SkinRests`](crate::parent_joint::SkinRests) over the project root after
    /// every world (re)build, the [`install_animation_library`](Engine::install_animation_library)
    /// mold: filled lazily from committed content, never hashed or snapshotted.
    pub(crate) fn install_skin_rests(&mut self) {
        let root = self.project_root.clone();
        crate::parent_joint::install(self.driver.world_mut(), &root);
    }

    /// Installs an empty [`AnimationLibrary`](crate::animation_events::AnimationLibrary) over the
    /// project root after every world (re)build, the [`install_plane`](Engine::install_plane) mold:
    /// [`animation_events`](crate::animation_events) fills it lazily from committed content, so it
    /// is rebuilt from disk rather than snapshotted, and is never hashed.
    pub(crate) fn install_animation_library(&mut self) {
        let library = crate::animation_events::AnimationLibrary::new(self.project_root.clone());
        self.driver.world_mut().insert_resource(library);
    }

    /// Installs a [`PathLibrary`](crate::path_follow::PathLibrary) over the project root after every
    /// world (re)build, the [`install_animation_library`](Engine::install_animation_library) mold,
    /// holding only the paths of lanes the world already places: paths are read afresh from committed
    /// content, never hashed, and `asset.reload` never touches it.
    pub(crate) fn install_path_library(&mut self) {
        let library = crate::path_follow::PathLibrary::new(self.project_root.clone());
        self.driver.world_mut().insert_resource(library);
        crate::lane::cache_placed_paths(self.driver.world_mut());
    }

    /// Installs the [`FontLibrary`](crate::FontLibrary) loaded from the project root after every
    /// world (re)build, the [`install_path_library`](Engine::install_path_library) mold: never
    /// hashed, never snapshotted.
    pub(crate) fn install_font_library(&mut self) {
        let library = crate::FontLibrary::load(AssetRoot::content(&self.project_root));
        self.driver.world_mut().insert_resource(library);
    }

    /// Installs an empty [`PrefabLibrary`](crate::prefab::PrefabLibrary) over the project root and the
    /// scene's `physics:` block after every world (re)build, the
    /// [`install_path_library`](Engine::install_path_library) mold. `asset.reload` refreshes the
    /// prefabs it holds through [`reload_prefab`](Engine::reload_prefab).
    pub(crate) fn install_prefab_library(&mut self) {
        let library = self.new_prefab_library(
            self.scene_physics.clone(),
            self.driver.frame(),
            self.replay_prefab_epoch,
        );
        self.driver.world_mut().insert_resource(library);
    }

    fn new_prefab_library(
        &self,
        physics: Option<ScenePhysics>,
        frame: FrameIndex,
        epoch: u32,
    ) -> crate::prefab::PrefabLibrary {
        crate::prefab::PrefabLibrary::new(
            self.project_root.clone(),
            Arc::clone(&self.registry),
            physics,
            frame,
            self.replay_prefab_text.clone(),
            epoch,
        )
    }

    /// The library for a world about to be committed, with every `source.preload` entry readied
    /// (read through `reads`, so a replay resolves them from the recording) and the load's own
    /// prefab reads seeded. `frame` is the frame the new driver will run next.
    ///
    /// # Errors
    /// Every entry's diagnostics — an unreadable path at `/preload/{i}`, an invalid prefab with its
    /// own located diagnostics.
    pub(crate) fn prepare_prefab_library(
        &self,
        source: &Scene,
        physics: Option<ScenePhysics>,
        frame: FrameIndex,
        mut reads: crate::prefab::LoadReads,
    ) -> Result<crate::prefab::PrefabLibrary, Vec<Diagnostic>> {
        let mut diagnostics = Vec::new();
        let mut keys = Vec::new();
        for (index, path) in source.preload.iter().enumerate() {
            match reads.resolve(path) {
                Ok(_) => match crate::prefab::prefab_key(path) {
                    Ok(key) => keys.push(key),
                    Err(found) => diagnostics.extend(found),
                },
                Err(mut error) => {
                    error.message = format!("the scene preloads `{path}`: {}", error.message);
                    diagnostics.push(error.with_path(format!("/preload/{index}")));
                }
            }
        }
        // Under replay the epoch the commit is about to enter: the next one, or for a restart the
        // current one.
        let mut library = self.new_prefab_library(physics, frame, reads.epoch);
        library.adopt(std::mem::take(&mut reads.held));
        library.seed(reads.reads);
        for key in keys {
            // Routed by the held text's `kind:`, never by the file name.
            if let Err(found) = library.preload(&key) {
                diagnostics.extend(found);
            }
        }
        if diagnostics.is_empty() {
            Ok(library)
        } else {
            Err(diagnostics)
        }
    }

    /// Installs the replay's recorded prefab texts and rebuilds the library over them, so a replay
    /// never reads a prefab off disk. `epoch` is the rebuild epoch the engine's world is in. Called
    /// before the drive, when no tick has run.
    #[cfg(test)]
    pub(crate) fn install_replay_prefab_text(
        &mut self,
        source: crate::prefab::PrefabTextSource,
        epoch: u32,
    ) {
        self.replay_prefab_text = Some(Arc::new(source));
        self.replay_prefab_epoch = epoch;
        self.replay_prefab_next_epoch = epoch + 1;
        self.install_prefab_library();
    }

    /// Installs the replay's recorded prefab texts before its first scene loads: the load commits
    /// epoch 0 and resolves its prefab reads from the recording, never the disk.
    pub(crate) fn arm_replay_prefab_text(&mut self, source: crate::prefab::PrefabTextSource) {
        self.replay_prefab_text = Some(Arc::new(source));
        self.replay_prefab_epoch = 0;
        self.replay_prefab_next_epoch = 0;
        self.install_prefab_library();
    }

    /// A resolver for the prefab texts a scene load (or district switch) reads, in the epoch the
    /// rebuilt world will be in.
    pub(crate) fn load_reads(&self) -> crate::prefab::LoadReads {
        self.reads_in(self.replay_prefab_next_epoch)
    }

    /// The resolver for a restart: the epoch the world is already in, holding the outcomes the
    /// current library decided, so the rebuilt world files no new read.
    pub(crate) fn restart_reads(&self) -> crate::prefab::LoadReads {
        let held = self
            .driver
            .world()
            .resource::<crate::prefab::PrefabLibrary>()
            .map(crate::prefab::PrefabLibrary::held_for_restart)
            .unwrap_or_default();
        self.reads_in(self.replay_prefab_epoch).with_held(held)
    }

    fn reads_in(&self, epoch: u32) -> crate::prefab::LoadReads {
        crate::prefab::LoadReads::new(
            self.project_root.clone(),
            self.replay_prefab_text.clone(),
            epoch,
        )
    }

    /// Hands the prefab texts the library read since the last drain to the recording as `prefab_text`
    /// records (dropped when nothing records), and moves the library's frame to the tick about to run.
    pub(crate) fn drain_prefab_texts(&mut self) {
        let frame = self.driver.frame();
        let Some(library) = self
            .driver
            .world_mut()
            .resource_mut::<crate::prefab::PrefabLibrary>()
        else {
            return;
        };
        let log = library.take_log();
        library.frame = frame;
        let Some(mut recorder) = self.recorder.take() else {
            return;
        };
        let mut failure = None;
        for (stamp, key, source, reload) in &log {
            if let Err(error) =
                crate::recording::write_prefab_text(&mut recorder, *stamp, key, source, *reload)
            {
                failure = Some(io_to_diagnostic(error));
                break;
            }
        }
        match failure {
            None => self.recorder = Some(recorder),
            Some(diagnostic) => self.stop_recording_on_failure(&diagnostic),
        }
    }

    /// Installs the [`ColliderMeshes`](ludion_physics::ColliderMeshes) library over the project root
    /// after every world (re)build, the [`install_animation_library`](Engine::install_animation_library)
    /// mold: a restore or switch reads the mesh documents afresh, so a body whose document changed is
    /// rebuilt on the next tick. `asset.reload` never touches it.
    pub(crate) fn install_collider_meshes(&mut self) {
        let library = crate::collider_mesh::library(self.project_root.clone());
        self.driver.world_mut().insert_resource(library);
    }

    /// Takes the current world's diagnostics latch, for [`install_diagnostics`](Engine::install_diagnostics)
    /// to hand to the world a district switch builds. Must be called **before** the driver is swapped.
    fn carry_diagnostics(&mut self) -> crate::Diagnostics {
        self.driver
            .world_mut()
            .resource_mut::<crate::Diagnostics>()
            .map(crate::Diagnostics::carried)
            .unwrap_or_default()
    }

    /// Installs a fresh [`ResidueStore`](crate::ResidueStore) resource — a copy of the session's
    /// residue map — into the current world, so game systems can read what a district they are *not*
    /// in kept behind. Called after every world (re)build (the
    /// [`install_audio_queue`](Engine::install_audio_queue) mold) and after the clear drain, always
    /// **after** the session's own residue has settled for that site, so the mirror is never a step
    /// stale. Excluded from the world hash and from snapshots — the session map behind it is what a
    /// snapshot carries.
    fn install_residue_store(&mut self) {
        // Disjoint field borrows (the install_docs pattern): the session's residue map and the
        // registry are read while the world is borrowed mutably to receive the mirror.
        let store = crate::ResidueStore::new(&self.session.residue, &self.registry);
        self.driver.world_mut().insert_resource(store);
    }

    /// (Re)builds every registered data-doc kind's [`DocStore`](crate::DocStore) resource by walking
    /// the content root, and inserts it into the current world. Called after every world (re)build —
    /// the [`install_audio_queue`](Engine::install_audio_queue) mold — so a `scene.load` or `restore`
    /// (which wipe all resources) reinstates the stores from the persistent doc-kind registrations,
    /// at generation 0. A no-op with no registered kinds (no walk, no IO).
    ///
    /// Every registered [`validate_docs`](crate::EngineBuilder::validate_docs) pass then runs against
    /// the freshly-installed stores: an `Error` it reports fails this call exactly as a malformed doc
    /// does, and a `Warning` is reported into [`Diagnostics`](crate::Diagnostics) (so it prints once
    /// at the next frame boundary) and kept in
    /// [`doc_validation_warnings`](Engine::doc_validation_warnings) for a host that wants it before
    /// any tick.
    ///
    /// A file the walk skipped for declaring a `kind:` this engine does not register lands in
    /// [`doc_skip_warnings`](Engine::doc_skip_warnings) — only possible when no game kind is
    /// registered, where a foreign kind is a fact about the content root rather than an error.
    ///
    /// # Errors
    /// Every diagnostic the content-root walk collected: `L1101` for an unreadable file,
    /// `L1119`/`L1120` for a malformed doc (fail-fast across all files), `L1119` for an unregistered
    /// `kind`; then every `Error` a registered validation pass reported (typically `L1144`), the
    /// engine's `L1315` singleton count first.
    /// Before any of that, `L1257` for a registered doc or profile type that declares a top-level
    /// `format`/`kind` field.
    pub(crate) fn install_docs(&mut self) -> Result<(), Vec<Diagnostic>> {
        crate::docs::validate_registered_types(&self.doc_kinds, self.profile_kind.as_ref())
            .map_err(|diagnostic| vec![diagnostic])?;
        // Disjoint field borrows (the extract_render_scene pattern): the content root, doc-kind list
        // and validators are read while the world is borrowed mutably to receive the stores. Boot
        // never re-applies a reload log, so it passes none.
        let prepared = crate::docs::prepare_docs(
            &self.project_root,
            &self.doc_kinds,
            self.game_doc_kinds,
            &self.doc_validators,
            &self.doc_texts,
            &crate::docs::DocReloadLog::default(),
            self.driver.world_mut(),
        )?;
        self.commit_docs(prepared);
        Ok(())
    }

    /// [`prepare_docs`](crate::docs::prepare_docs) against a world that is not yet this engine's —
    /// the fallible half of a rebuild, run on the detached new world so a failure leaves the engine
    /// untouched. `reloads` is the log to re-apply on top (the session's, or a snapshot's).
    ///
    /// # Errors
    /// See [`install_docs`](Engine::install_docs), plus `L1119`/`L1120` from a logged reload.
    pub(crate) fn prepare_docs_for(
        &self,
        world: &mut World,
        reloads: &crate::docs::DocReloadLog,
    ) -> Result<PreparedDocs, Vec<Diagnostic>> {
        crate::docs::prepare_docs(
            &self.project_root,
            &self.doc_kinds,
            self.game_doc_kinds,
            &self.doc_validators,
            &self.doc_texts,
            reloads,
            world,
        )
    }

    /// The infallible half of [`prepare_docs_for`](Engine::prepare_docs_for): records what the walk
    /// skipped and what the validators warned about, and latches the warnings into
    /// [`Diagnostics`](crate::Diagnostics) (so they print once at the next frame boundary) and
    /// [`doc_validation_warnings`](Engine::doc_validation_warnings). Runs after
    /// [`install_diagnostics`](Engine::install_diagnostics), on the world the docs were prepared for.
    fn commit_docs(&mut self, docs: PreparedDocs) {
        self.doc_skip_warnings = docs.skips.warnings;
        self.doc_skip_summary = docs.skips.summary;
        self.report_doc_diagnostics(&docs.warnings);
        self.doc_validation_warnings = docs.warnings;
    }

    /// Pushes each diagnostic into the world's [`Diagnostics`](crate::Diagnostics) resource, keyed by
    /// its document path (the `<kind>/<doc name>/<field>` convention) so the once-per-offender latch
    /// distinguishes two docs failing the same rule. A pathless diagnostic keys on its message, which
    /// is the only identity it has.
    fn report_doc_diagnostics(&mut self, diagnostics: &[Diagnostic]) {
        let Some(sink) = self.driver.world_mut().resource_mut::<crate::Diagnostics>() else {
            return;
        };
        for diagnostic in diagnostics {
            let key = diagnostic
                .path
                .clone()
                .unwrap_or_else(|| diagnostic.message.clone());
            sink.report_full(key, diagnostic.clone());
        }
    }

    /// What the last install-time doc validation reported at `Warning` severity, in registration
    /// order — empty for a game with no registered pass, or one whose content is clean.
    ///
    /// Errors are not here: they failed the build or the `scene.load` that ran the pass. This is the
    /// surface a host prints before it starts ticking (`--check-docs` does); a running host sees the
    /// same lines through [`Diagnostics`](crate::Diagnostics) at the first frame boundary.
    pub fn doc_validation_warnings(&self) -> &[Diagnostic] {
        &self.doc_validation_warnings
    }

    /// The `L1155` warnings the last content-root walk produced: one per `*.yaml` skipped for
    /// declaring a `kind:` this engine does not register, in walk order.
    ///
    /// Only ever non-empty on an engine no game registered a kind on — the built-in engine behind
    /// `ludion serve` and `ludion sim-hash`, which hosts an arbitrary scene and so cannot know the
    /// game kinds its content root holds. A game's own engine fails the boot on such a file
    /// (`L1119`), so this stays empty there. A host prints these at boot; build-time metadata, never
    /// world state.
    pub fn doc_skip_warnings(&self) -> &[Diagnostic] {
        &self.doc_skip_warnings
    }

    /// The one `L1155` a host prints in place of [`doc_skip_warnings`](Engine::doc_skip_warnings):
    /// how many documents the last content-root walk skipped and which kinds they declared, in kind
    /// order. `None` when it skipped nothing.
    ///
    /// This is the host's form, not the precise one. A game's scene directory routinely holds tens of
    /// its own docs, and printing a three-line block each buries whatever the host says next — the
    /// bind failure behind a stale stage engine, in the incident that prompted this. Use
    /// [`doc_skip_warnings`](Engine::doc_skip_warnings) when the file names matter.
    pub fn doc_skip_summary(&self) -> Option<&Diagnostic> {
        self.doc_skip_summary.as_ref()
    }

    /// Whether any registered data-doc kind carries this name — a game's own
    /// [`doc_kind`](crate::EngineBuilder::doc_kind) or an engine-owned store such as
    /// [`patterns`](crate::EngineBuilder::patterns). A doc of a registered kind was validated at
    /// boot; one of any other kind was not.
    ///
    /// # Example
    /// ```
    /// use ludion_engine::Engine;
    ///
    /// let engine = Engine::builder().patterns().build().unwrap();
    /// assert!(engine.registers_doc_kind("pattern"));
    /// assert!(!engine.registers_doc_kind("weapon"));
    /// ```
    pub fn registers_doc_kind(&self, kind: &str) -> bool {
        self.doc_kinds.iter().any(|dk| dk.kind == kind)
    }

    /// Removes the temps of saves that earlier processes died inside. Boot only, and only in a host
    /// that persists: a replay or a test never touches the save directory.
    pub(crate) fn sweep_profile_debris(&self) {
        if self.profile_kind.is_some() && self.profile_persistence && !self.profile_off_disk {
            crate::player_profile::sweep_temps(&self.save_root().join(&self.profile_rel_path));
        }
    }

    /// (Re)installs the [`Profile<P>`](crate::Profile) resource by loading `save/profile.yaml` (or the
    /// configured relative path) from the save root, leniently migrating it, and storing the report
    /// on the engine. Called after every world (re)build — the [`install_docs`](Engine::install_docs)
    /// mold — because `scene.load`/`restore` wipe all resources. A no-op with no registered profile
    /// (no IO), so a game that registers none boots byte-identically to before this facility existed.
    ///
    /// Never fails the boot: an unloadable save is backed up and defaulted (an `L1121` reset recorded
    /// in [`profile_migration`](Engine::profile_migration)), so a corrupt or drifted profile never
    /// bricks the game — unlike a malformed data-doc, which is authored content and fails the load.
    pub(crate) fn install_profile(&mut self) {
        // Disjoint field borrows (the install_docs pattern): the profile kind and resolved path are
        // read while the world is borrowed mutably to receive the resource.
        let Some(kind) = self.profile_kind.as_ref() else {
            return;
        };
        let path = self.save_root().join(&self.profile_rel_path);
        let report = if self.profile_off_disk {
            kind.load_default(&path, self.driver.world_mut())
        } else {
            kind.load(&path, self.driver.world_mut(), self.profile_persistence)
        };
        self.profile_migration = report;
    }

    /// The live player profile rendered to its canonical `kind: profile` text, or `None` when no
    /// profile is registered (or the resource is somehow absent). This is the profile-at-start a
    /// recording embeds in its [`ReplayHeader`] and a snapshot embeds in its keyframe, so a replay or a
    /// restore can reinstall the exact profile off-disk (Phase 16 / E5, decisions 6/7).
    pub(crate) fn emit_profile_text(&self) -> Option<String> {
        self.profile_kind.as_ref()?.emit(self.driver.world())
    }

    /// (Re)installs the [`Profile<P>`](crate::Profile) resource directly from `text` — engine-emitted
    /// canonical `kind: profile` text embedded in a replay header or a snapshot keyframe — **without
    /// touching disk**. A restore/replay is an exact continuation, so it must honor the embedded
    /// profile-at-start (preserving unsaved in-memory changes across a `ludion dev` reload, and
    /// reproducing on a machine whose `save/profile.yaml` differs or is absent) rather than re-read the
    /// possibly-different on-disk file. Stores the load's [`ProfileMigration`] like
    /// [`install_profile`](Engine::install_profile); a no-op with no registered profile. Never fails:
    /// unparseable embedded text backs off to `P::default()` with an `L1121` reset.
    pub(crate) fn install_profile_from_text(&mut self, text: &str) {
        let Some(kind) = self.profile_kind.as_ref() else {
            return;
        };
        let report = kind.load_text(text, self.driver.world_mut());
        self.profile_migration = report;
    }

    /// The last profile load's lenient-migration report (the `install_docs` diagnostics analog): the
    /// fields filled from defaults or dropped, or the backup-and-reset (`L1121`) if the save could not
    /// be honored. The build's report survives the boot `scene.load` (which carries the profile the
    /// build read) and is replaced by the next load that reads disk. Clean
    /// ([`is_clean`](ProfileMigration::is_clean)) with no registered profile, a first-boot default, or
    /// a lossless load. A host prints it at boot; tests assert on it.
    pub fn profile_migration(&self) -> &ProfileMigration {
        &self.profile_migration
    }

    /// The non-fatal warnings produced once at engine build (the `L1124` schedule-module-concentration
    /// warning) — empty for a bare engine, and for a game whose every definition module stays under the
    /// per-module system threshold. A host (`ludion::run`) prints these at boot; a test asserts on them.
    /// The warnings are pure build-time metadata: they never enter the `WorldHash`, a snapshot, or a
    /// replay, and an engine built directly in a test simply leaves them unprinted.
    pub fn boot_warnings(&self) -> &[Diagnostic] {
        &self.boot_warnings
    }

    /// Attempts to reload `path` as a game data-doc — the `asset.reload` handler's doc branch.
    ///
    /// If `path` resolves to an existing `*.yaml` under the content root whose top-level `kind:` is a
    /// registered doc kind, it is re-read, validated, and upserted into that kind's [`DocStore`], the
    /// store's generation is bumped, and the swap is armed for recording (via
    /// [`pending_doc_reload`](Engine::pending_doc_reload)) — returning `Some(content_hash)` of the new
    /// canonical doc (for the `asset.reloaded` event). `Ok(None)` if `path` is not a registered doc (a
    /// texture/mesh/voxel asset, a scene, an unregistered kind, or a non-YAML/out-of-root path) so the
    /// caller leaves it the existing silent no-op.
    ///
    /// # Errors
    /// `L1321` for a `*.yaml` path that climbs out of the root with `..`; `L100x` if the edited doc
    /// fails to parse as strict-subset YAML at all, `L1119`/`L1120` if it
    /// parses but is malformed, and any `Error` the registered validators report after the swap that
    /// they did not report before it (`L1144`, `L1149`–`L1152`, `L1315` for a second doc of a
    /// `doc_singleton` kind, a game's own codes) — each a diagnosed
    /// error, never a crash or a silent skip; the store keeps its previous contents and generation (a
    /// bad save mid-tune must not wipe state), and nothing is armed for recording.
    ///
    /// A key the builder holds as [`doc_text`](crate::EngineBuilder::doc_text) is never reloaded
    /// from disk (`Ok(None)`): the override wins for the engine's life.
    pub(crate) fn reload_doc_from_disk(&mut self, path: &str) -> Result<Option<u64>, Diagnostic> {
        if self.doc_kinds.is_empty() {
            return Ok(None);
        }
        let Some((key, source)) = crate::docs::resolve_doc_path(&self.project_root, path)? else {
            return Ok(None);
        };
        if self.doc_texts.contains_key(&key) {
            return Ok(None);
        }
        let peeked = crate::docs::peek_reload_kind(&key, &source)?;
        let kind = match peeked {
            Some(kind) if self.doc_kinds.iter().any(|dk| dk.kind == kind) => kind,
            // No `kind` (a scene), an engine/asset kind, or an unregistered kind: not our path.
            _ => return Ok(None),
        };
        // Disjoint field borrows (the install_docs pattern): the doc-kind list is read while the world
        // is borrowed mutably to receive the upsert.
        let doc_kinds = &self.doc_kinds;
        let fields = crate::docs::check_reload(doc_kinds, &kind, &key, &source)
            .map_err(crate::commands::collapse)?;
        let validators = &self.doc_validators;
        let before = crate::docs::run_validators(
            doc_kinds,
            validators,
            &self.project_root,
            self.driver.world(),
        );
        let world = self.driver.world_mut();
        let (document, undo) = crate::docs::apply_reload(doc_kinds, world, &kind, &key, fields)
            .map_err(crate::commands::collapse)?;
        let reported = crate::docs::run_validators(
            doc_kinds,
            validators,
            &self.project_root,
            self.driver.world(),
        );
        // An error the swap introduced refuses it: the old document goes back before anything is
        // logged, armed or reported. An error that was already there still prints and applies.
        let introduced: Vec<Diagnostic> = reported
            .iter()
            .filter(|d| d.severity == ludion_core::diagnostics::Severity::Error)
            .filter(|d| !before.contains(d))
            .cloned()
            .collect();
        if !introduced.is_empty() {
            undo(self.driver.world_mut());
            return Err(crate::commands::collapse(introduced));
        }
        let hash = ludion_assets::content_hash(document.as_bytes());
        // Log the reload so a later scene switch or a snapshot restore re-applies it off-disk on the
        // rebuilt district (see `SessionState::doc_reloads`) — the same canonical text a replay's
        // `DocReload` record and a snapshot's `doc_reloads` field carry.
        self.session.doc_reloads.record(&kind, &key, &document);
        self.pending_doc_reload = Some(AppliedDocReload {
            kind,
            key,
            document,
        });
        // What the swap left standing — warnings and pre-existing errors; the gate above refused the
        // rest — goes to `Diagnostics`, which prints it once per offender.
        self.report_doc_diagnostics(&reported);
        Ok(Some(hash))
    }

    /// Refreshes a prefab the world's [`PrefabLibrary`](crate::prefab::PrefabLibrary) already holds —
    /// the `asset.reload` handler's prefab branch. `Ok(None)` if `path` is outside the content root or
    /// no spawn has loaded it (the engine only reloads what it holds); otherwise the file is re-read
    /// and re-validated and `Some(content_hash)` returned. Live instances are untouched.
    ///
    /// # Errors
    /// The prefab's diagnostics when the edited file fails validation; the library keeps the entry it
    /// had.
    pub(crate) fn reload_prefab(&mut self, path: &str) -> Result<Option<u64>, Diagnostic> {
        let Some((key, source)) = crate::docs::resolve_doc_path(&self.project_root, path)? else {
            return Ok(None);
        };
        let Some(library) = self
            .driver
            .world_mut()
            .resource_mut::<crate::prefab::PrefabLibrary>()
        else {
            return Ok(None);
        };
        if !library.holds(&key) {
            return Ok(None);
        }
        library
            .replace(&key, &source)
            .map_err(crate::commands::collapse)?;
        self.drain_prefab_texts();
        Ok(Some(ludion_assets::content_hash(source.as_bytes())))
    }

    /// Reloads the font table when `path` is `fonts.yaml` or a face document it lists: the library
    /// is rebuilt from disk and replaces the world's resource, returning the file's content hash.
    /// `None` when `path` is not a font file the library holds. A re-bake rewrites the atlas PNG too,
    /// which reloads through the asset cache.
    ///
    /// # Errors
    /// `L1255` while a recording is armed: fonts are sim-visible through measurement, so a swap
    /// would desynchronize the replay.
    pub(crate) fn reload_fonts(&mut self, path: &str) -> Result<Option<u64>, Diagnostic> {
        let Some((key, source)) = crate::docs::resolve_doc_path(&self.project_root, path)? else {
            return Ok(None);
        };
        let Some(library) = self.driver.world().resource::<crate::FontLibrary>() else {
            return Ok(None);
        };
        if !library.holds(&key) {
            return Ok(None);
        }
        if self.recorder.is_some() {
            return Err(Diagnostic::new(
                ludion_core::codes::code::FONT_LOAD,
                format!(
                    "reloading the font `{key}` while recording would desynchronize the replay"
                ),
            )
            .with_path(key)
            .with_help("stop recording first; a committed replay re-reads fonts from disk"));
        }
        self.install_font_library();
        Ok(Some(ludion_assets::content_hash(source.as_bytes())))
    }

    /// Writes the live `entities` as a new `kind: prefab` at `path` under the content root and
    /// returns the normalized key. The first entity is the prefab's root, and the parts follow in the
    /// order given; each needs a unique `Name` without `/`, and a reference between them must stay
    /// inside the set. An existing prefab is overwritten with no check against what it held
    /// (unlike `scene.save`, there is no base to compare); an existing file that is not a prefab
    /// document is refused. The world's entities are unchanged. When the session's prefab library
    /// already holds the prefab it takes the new text as
    /// a recorded reload, so later spawns use it and a replay agrees; live instances are not
    /// re-expanded. The file is written after the library accepted the text, and the library is
    /// restored if the write fails.
    ///
    /// # Errors
    /// `L1236` for an empty or duplicate set, a dead entity, a missing, duplicate or `/`-bearing
    /// name, a reference out of the set, a path that is not a `.prefab.yaml`, an existing file that is not
    /// a prefab document (or a generated prefab) or a failed write;
    /// `L1230` for an absolute path or a world with no prefab library; the prefab's own diagnostics
    /// (`L1229` for a `Camera` part) when the result would not validate, including a dependent
    /// prefab the new text breaks. Nothing is written on error.
    ///
    /// # Example
    /// ```
    /// use ludion_engine::{Engine, Name};
    ///
    /// let root = std::env::temp_dir().join(format!("ludion-create-prefab-doc-{}", std::process::id()));
    /// std::fs::create_dir_all(&root).unwrap();
    /// let mut engine = Engine::builder().project_root(&root).build().unwrap();
    /// let lid = engine.world_mut().spawn();
    /// engine.world_mut().insert(lid, Name("lid".into())).unwrap();
    ///
    /// let key = engine.create_prefab(&[lid], "crate.prefab.yaml").unwrap();
    /// assert_eq!(key, "crate.prefab.yaml");
    /// assert!(std::fs::read_to_string(root.join(&key)).unwrap().contains("name: lid"));
    /// # std::fs::remove_dir_all(&root).ok();
    /// ```
    pub fn create_prefab(
        &mut self,
        entities: &[EntityId],
        path: &str,
    ) -> Result<String, Vec<Diagnostic>> {
        let result = crate::prefab_ops::create(self.driver.world_mut(), entities, path);
        self.drain_prefab_texts();
        if let Ok(key) = &result {
            self.refresh_prefab_clip_cache(key);
        }
        result
    }

    /// Re-reads the cached clip rests of the prefab `path` after its file was rewritten: the file is
    /// a source of that cache entry, so a part would otherwise pose against the old rest. A failed
    /// re-read leaves the entry dropped for the next extract to report.
    fn refresh_prefab_clip_cache(&mut self, path: &str) {
        let root = self.project_root().to_path_buf();
        if let Some(key) = self
            .asset_cache()
            .resolve_key(AssetRoot::content(&root), path)
        {
            let _ = self
                .asset_cache_mut()
                .reload(AssetRoot::content(&root), &key);
        }
    }

    /// Writes the edits of the prefab instance `instance` is part of back into its prefab file: each
    /// part's component differences from the prefab become overrides merged into the prefab's parts,
    /// and parts the instance lost are deleted from it. `parts` limits the write to those part paths
    /// (`None` writes all). The anchor's `Transform` and a `Parent` onto an entity outside the
    /// instance are the instance's placement and are never written.
    ///
    /// The session's prefab library takes the new text as a recorded reload, so later spawns use it
    /// and a replay agrees; other live instances are not re-expanded. The file is written after the
    /// library accepted it, and restored in the library if the write fails.
    ///
    /// # Errors
    /// `L1235` when `instance` is not part of a prefab instance; `L1237` for an unknown `parts`
    /// path, an override carrying an entity reference, a reference out of the instance, a prefab
    /// part another still refers to being removed, or a failed write; `L1234` and `L1230` from the
    /// collapse, and the edited prefab's own diagnostics if it would not validate. Nothing changes
    /// on error.
    ///
    /// # Example
    /// ```
    /// use ludion_engine::{Engine, Transform, spawn_prefab};
    ///
    /// let root = std::env::temp_dir().join(format!("ludion-apply-prefab-doc-{}", std::process::id()));
    /// std::fs::create_dir_all(&root).unwrap();
    /// std::fs::write(
    ///     root.join("lamp.prefab.yaml"),
    ///     "format: 1\nkind: prefab\nentities:\n  - name: post\n    components:\n      Transform: {}\n  - name: bulb\n    components:\n      Parent:\n        index: 0\n        generation: 0\n      Transform:\n        translation:\n        - 0.0\n        - 2.0\n        - 0.0\n",
    /// )
    /// .unwrap();
    ///
    /// let mut engine = Engine::builder().project_root(&root).build().unwrap();
    /// let ids = spawn_prefab(engine.world_mut(), "lamp.prefab.yaml", None, Some("lamp-1"), false).unwrap();
    /// let moved = Transform { translation: [0.0, 5.0, 0.0], rotation: [0.0; 3], scale: [1.0; 3] };
    /// engine.world_mut().insert(ids[1], moved).unwrap();
    ///
    /// let applied = engine.apply_prefab(ids[0], None).unwrap();
    /// assert_eq!(applied.parts, vec!["bulb".to_string()]);
    /// let text = std::fs::read_to_string(root.join("lamp.prefab.yaml")).unwrap();
    /// assert!(text.contains("- 5.0"));
    /// # std::fs::remove_dir_all(&root).ok();
    /// ```
    pub fn apply_prefab(
        &mut self,
        instance: EntityId,
        parts: Option<&[String]>,
    ) -> Result<crate::PrefabApplied, Vec<Diagnostic>> {
        let result = crate::prefab_ops::apply(self.driver.world_mut(), instance, parts);
        self.drain_prefab_texts();
        if let Ok(applied) = &result {
            self.refresh_prefab_clip_cache(&applied.path);
        }
        result
    }

    /// Reapplies a recorded prefab reload at replay time: re-validates the inline `document` as the
    /// prefab `key` and swaps it in, **without touching the disk**. A world with no library is a no-op.
    ///
    /// # Errors
    /// The prefab's own diagnostic if the recorded text fails validation (a replay against an
    /// incompatible build) — surfaced, not silently dropped.
    pub(crate) fn apply_prefab_reload_inline(
        &mut self,
        key: &str,
        document: &str,
    ) -> Result<(), Diagnostic> {
        let Some(library) = self
            .driver
            .world_mut()
            .resource_mut::<crate::prefab::PrefabLibrary>()
        else {
            return Ok(());
        };
        library.replace(key, document).map_err(|diagnostics| {
            let mut diagnostic = crate::commands::collapse(diagnostics);
            diagnostic.message =
                format!("replayed prefab reload of `{key}`: {}", diagnostic.message);
            diagnostic
        })
    }

    /// Reapplies a recorded `DocReload` at replay time: validates the inline `document` against the
    /// registered `kind`'s schema and upserts it into the [`DocStore`] at `key` (bumping generation),
    /// **without touching the on-disk file** — so a replay reproduces the swap on a checkout whose doc
    /// differs or is gone (the inline-content proof, decision 9). Not recorded (replay never records).
    ///
    /// # Errors
    /// `L1119`/`L1120` if the inline content fails validation against the current schema (a replay
    /// against a schema-incompatible build) — surfaced, not silently dropped.
    pub(crate) fn apply_doc_reload_inline(
        &mut self,
        kind: &str,
        key: &str,
        document: &str,
    ) -> Result<(), Diagnostic> {
        let doc_kinds = &self.doc_kinds;
        let world = self.driver.world_mut();
        crate::docs::reload_doc(doc_kinds, world, kind, key, document)
            .map_err(crate::commands::collapse)?;
        // Log the reload identically to the live path, so a switch under replay re-applies the same
        // reloads on the rebuilt district that the live run did (Phase 17 / E7, item 4).
        self.session.doc_reloads.record(kind, key, document);
        Ok(())
    }

    /// The frame-stamped audio events emitted so far this session, in emission order, bounded to the
    /// most recent `AUDIO_LOG_CAPACITY` entries.
    ///
    /// This is the text-first verification surface for audio (there are no audio goldens): a test
    /// scripts input, drives the engine, and asserts on the frame-stamped message stream here. It is
    /// reset whenever the world is reset (a new `scene.load` starts a fresh stream).
    pub fn audio_log(&self) -> &VecDeque<AudioLogEntry> {
        &self.session.audio_log
    }

    /// Wires the one-directional channel to a playback sink: every drained [`AudioMessage`] is
    /// forwarded here for playback (in addition to the [`audio_log`](Engine::audio_log)). A host
    /// installs this before the engine moves onto the sim thread; a headless/test engine leaves it
    /// unset and still fills the log. Playback is strictly downstream — it never feeds back into the
    /// sim.
    pub fn set_audio_sender(&mut self, sender: crossbeam_channel::Sender<AudioMessage>) {
        self.audio_sender = Some(sender);
    }

    /// Wires the one-directional channel to the window host: each change of the game's
    /// [`CursorLock`](crate::CursorLock) request is sent here (`true` locks, `false` releases), once
    /// per change. A host installs this before the engine moves onto the sim thread; a headless/test
    /// engine leaves it unset and the request stays a silent no-op.
    pub fn set_cursor_lock_sender(&mut self, sender: crossbeam_channel::Sender<bool>) {
        self.cursor_lock_sender = Some(sender);
    }

    /// The pointer lock the game last requested through [`CursorLock`](crate::CursorLock), `None`
    /// before any request.
    pub fn cursor_lock_request(&self) -> Option<bool> {
        self.cursor_lock_game
    }

    /// The frame-stamped profile-save requests drained so far this session, in request order — the
    /// text-first verification surface for saves (the [`audio_log`](Engine::audio_log) mold).
    ///
    /// A system's [`Profile::save`](crate::Profile::save) at frame N appears here as one entry stamped
    /// N (several saves within one frame collapse into one). Like the audio log it is a pure function
    /// of the sim, so a replay regenerates the identical stream, and it is reset at every world reset.
    /// Bounded to the most recent `PROFILE_SAVE_LOG_CAPACITY` entries.
    pub fn profile_save_log(&self) -> &VecDeque<ProfileSaveEntry> {
        &self.session.profile_save_log
    }

    /// The most recent frame-boundary profile-save failure (`L1122`), or `None` if the last save
    /// succeeded or none was attempted (the [`profile_migration`](Engine::profile_migration) mold for
    /// the save side).
    ///
    /// A save write is crash-durable and non-fatal: a failure never stops the sim, a whole save
    /// stays on disk, and the next [`Profile::save`](crate::Profile::save) retries (a `scene.load`
    /// first reverts to the disk copy). A host prints this
    /// warning; a test asserts on it. Cleared on a successful write and at every world reset.
    pub fn profile_save_error(&self) -> Option<&Diagnostic> {
        self.session.profile_save_error.as_ref()
    }

    /// The frame-stamped run-save requests drained so far this session, in request order — the
    /// [`profile_save_log`](Engine::profile_save_log) mold, a pure function of the sim, reset at
    /// every world reset and bounded to the most recent `RUN_SAVE_LOG_CAPACITY` entries.
    pub fn run_save_log(&self) -> &VecDeque<RunSaveEntry> {
        &self.session.run_save_log
    }

    /// The most recent frame-boundary run-save failure (`L1400`) or refused resume (`L1401`), or
    /// `None` if the last write or resume succeeded or none was attempted (the [`profile_save_error`](Engine::profile_save_error) mold).
    pub fn run_save_error(&self) -> Option<&Diagnostic> {
        self.session.run_save_error.as_ref()
    }

    /// The most recent scene-switch warning: a rejection (`L3018` target unloadable / `L3019` anchor
    /// missing) or a succeeded-but-degraded switch (`L3020` — a dead carry id was skipped), or `None`
    /// if the last switch was fully clean or none was attempted (the
    /// [`profile_save_error`](Engine::profile_save_error) mold for transitions).
    ///
    /// A switch is transactional and non-fatal: a rejected switch never touches the live world or
    /// stops the sim — the run continues in the current district; a degraded switch (`L3020`) still
    /// happened, carrying whichever entities were live. A host prints this warning to stderr; a test
    /// asserts on it. Cleared only on a fully-clean switch and at every world reset.
    pub fn last_transition_error(&self) -> Option<&Diagnostic> {
        self.session.last_transition_error.as_ref()
    }

    /// Takes the caught system panic (`L4007`) that auto-paused the sim, if one is outstanding.
    ///
    /// A single-taker surface on purpose: `sim.step` takes it so the request that drove the failing
    /// tick answers with the diagnostic instead of an Ack, and a host or test takes it to assert on
    /// it. Leaving it set would make the *next* step report a panic that already happened. The
    /// subscription-visible copy is the `Error` log event pushed at the same moment, which is how a
    /// wire client watching a free-running sim discovers the auto-pause; the run-state itself reads
    /// `Paused` from then on.
    pub fn take_system_panic(&mut self) -> Option<Diagnostic> {
        self.session.last_system_panic.take()
    }

    /// This session's data-doc reload log (one entry per `(kind, key)`: latest canonical text and reload count) — read by
    /// [`crate::snapshot::capture`] so a snapshot carries the reloads and a restore re-applies them
    /// off-disk instead of rebuilding the `DocStore`s from possibly-changed disk (audit finding 2.1).
    pub(crate) fn session_doc_reloads(&self) -> &crate::docs::DocReloadLog {
        &self.session.doc_reloads
    }

    /// What each scene visited this session kept behind, keyed by scene `name:` — read by
    /// [`crate::transition`]'s `perform` to re-apply a returning district's residue.
    pub(crate) fn session_residue(&self) -> &StableMap<String, crate::transition::SceneResidue> {
        &self.session.residue
    }

    /// The session's scene residue, mutably — written by [`crate::transition`]'s `perform` once a
    /// switch is past every fallible check, so a rejected switch leaves the map untouched.
    pub(crate) fn session_residue_mut(
        &mut self,
    ) -> &mut StableMap<String, crate::transition::SceneResidue> {
        &mut self.session.residue
    }

    /// The project root that scene and asset paths resolve against.
    pub fn project_root(&self) -> &Path {
        &self.project_root
    }

    /// Sets where `engine.snapshot` writes the scene-session sidecar (see
    /// [`adopt_scene_session`](Self::adopt_scene_session)); `None` writes none.
    pub fn set_scene_session_file(&mut self, file: Option<PathBuf>) {
        self.scene_session_file = file;
    }

    /// Writes the scene-session sidecar: the frame, scene path and conflict bases as of now.
    pub(crate) fn write_scene_session(&mut self) {
        self.scene_session_stamp = Some(SceneSessionStamp {
            frame: self.frame().as_u64(),
            scene_hash: self.scene_identity(),
            path: self.current_scene_path.clone(),
        });
        self.write_scene_session_doc();
    }

    /// Rewrites the sidecar after a `scene.save`: the current bases under the snapshot's own frame,
    /// scene and path, so a restart from that snapshot sees the save instead of a stale base. No-op
    /// before the first snapshot.
    pub(crate) fn refresh_scene_session(&self) {
        if self.scene_session_stamp.is_some() {
            self.write_scene_session_doc();
        }
    }

    fn write_scene_session_doc(&self) {
        let (Some(file), Some(stamp)) = (&self.scene_session_file, &self.scene_session_stamp)
        else {
            return;
        };
        let doc = crate::scene_session::SceneSessionDoc {
            frame: stamp.frame,
            scene_hash: stamp.scene_hash.clone(),
            path: stamp.path.clone(),
            bases: self
                .scene_bases
                .iter()
                .filter_map(|(key, text)| Some((key.to_str()?.to_string(), text.clone())))
                .collect(),
        };
        // A failed write leaves the old file, which may still match by frame and scene (then it
        // describes the same world) or not (the next boot falls back to unset and says so); failing
        // `engine.snapshot` would cost the designer's state.
        let _ = doc.write(file);
    }

    /// The `WireHash` string of the scene reference `engine.snapshot` captures (`self.scene`).
    fn scene_identity(&self) -> String {
        let bytes = serde_json::to_vec(&self.scene).unwrap_or_default();
        ludion_protocol::hash::WireHash::of_bytes(&bytes).to_string()
    }

    /// Adopts the scene session a previous process wrote beside its snapshot: the scene path a
    /// restart names and `scene.save`'s conflict bases. Taken only when the file's frame equals this
    /// engine's (restored) frame and scene; otherwise path and bases stay as they were.
    ///
    /// # Errors
    /// `L0033` when the file is missing, malformed, or from another frame or scene.
    pub fn adopt_scene_session(&mut self, file: &Path) -> Result<(), Diagnostic> {
        let degraded =
            |why: String| {
                Diagnostic::new(
                code::DEV_DEGRADED,
                format!("could not adopt the scene session {}: {why}", file.display()),
            )
            .with_help(
                "the first `scene.save` over an existing file will ask for confirmation (`L3058`); \
                 `scene.load` it or pass `force: true`",
            )
            };
        let doc = crate::scene_session::SceneSessionDoc::read(file).map_err(&degraded)?;
        let frame = self.frame().as_u64();
        if doc.frame != frame {
            return Err(degraded(format!(
                "it was written at frame {}, the restored world is at frame {frame}",
                doc.frame
            )));
        }
        let scene_hash = self.scene_identity();
        if doc.scene_hash != scene_hash {
            return Err(degraded(format!(
                "it was written for scene {}, the restored world holds scene {scene_hash}",
                doc.scene_hash
            )));
        }
        self.current_scene_path = doc.path;
        self.scene_bases = doc
            .bases
            .into_iter()
            .map(|(key, text)| (PathBuf::from(key), text))
            .collect();
        Ok(())
    }

    pub(crate) fn scene_base(&self, resolved: &Path) -> Option<&str> {
        self.scene_bases
            .get(&base_key(resolved))
            .map(String::as_str)
    }

    pub(crate) fn set_scene_base(&mut self, resolved: PathBuf, text: String) {
        self.scene_bases.insert(base_key(&resolved), text);
    }

    pub(crate) fn set_current_scene_path(&mut self, path: Option<String>) {
        self.current_scene_path = path;
    }

    pub(crate) fn set_current_request(&mut self, request: Option<RequestId>) {
        self.current_request = request;
    }

    pub(crate) fn current_request(&self) -> Option<RequestId> {
        self.current_request
    }

    /// The directory the player profile loads from and saves to: the builder's `save_root`, else the
    /// project root.
    pub fn save_root(&self) -> &Path {
        self.save_root.as_deref().unwrap_or(&self.project_root)
    }

    /// This game's audio mixdown: the master gain each bus is mixed through, as the builder
    /// authored it. A host reads it here when it wires the playback sink up.
    pub fn category_gains(&self) -> CategoryGains {
        self.category_gains
    }

    /// How positional sounds measure distance, as the builder authored it.
    pub fn audio_distance(&self) -> AudioDistance {
        self.audio_distance
    }

    /// Whether positional sounds are panned across the stereo field, as the builder authored it.
    pub fn audio_pan(&self) -> AudioPan {
        self.audio_pan
    }

    /// The plane this game's 2D queries live in, as the builder authored it.
    pub fn plane(&self) -> Plane {
        self.plane
    }

    /// The default screenshot resolution.
    pub fn render_size(&self) -> (u32, u32) {
        self.render_size
    }

    /// The `WorldHash` of the current world — the determinism verification primitive.
    ///
    /// A non-finite float in a hashed component is reported as `L4024`, once per component type per
    /// session, and emitted at once — `ludion sim-hash` exits right after hashing, so it never
    /// reaches another frame boundary. The hash itself is unaffected.
    ///
    /// # Errors
    /// Propagates a component's serialization failure (`L0007`), which would itself be a bug.
    pub fn world_hash(&mut self) -> Result<WorldHash, Diagnostic> {
        let report = WorldHash::compute_with_report(self.driver.world(), &self.registry)?;
        if let Some(site) = report.non_finite {
            let diagnostic = Diagnostic::new(
                code::HASHED_NON_FINITE_FLOAT,
                format!(
                    "component `{}` on entity {} holds a non-finite float (NaN or ±inf); {} \
                     non-finite value(s) in this hash",
                    site.component, site.entity, site.count
                ),
            )
            .with_help(
                "a NaN or infinity in sim state usually comes from a division by zero or the \
                 normalisation of a zero vector; `ludion explain L4024` says how to find the writer",
            );
            crate::diagnostics::report_full(self.driver.world_mut(), site.component, diagnostic);
            self.emit_pending_diagnostics();
        }
        Ok(report.hash)
    }

    // --- Command entry points -------------------------------------------------------------------

    /// Dispatches `command` through the parity gate, recording it into the active session first
    /// (mutating commands only — see the `recording` module).
    ///
    /// This is the recording-aware entry point a driver or server uses. The pure
    /// [`dispatch`](crate::dispatch) function is the parity gate itself, for callers that do not
    /// record. A command is recorded as it *applied* (see `Applied`): itself on success, a shortened
    /// form when the handler failed part-way (a `sim.step` stopped by a system panic), nothing when the
    /// failure left the world untouched. A record-write failure stops recording
    /// (`stop_recording_on_failure`) and never changes the answer.
    ///
    /// # Errors
    /// Whatever the command's handler reports (see [`dispatch`](crate::dispatch)).
    pub fn execute(&mut self, command: ludion_protocol::Command) -> Result<Dispatched, Diagnostic> {
        if self.playback.is_some()
            && let Some(answer) = self.gate_live_command(&command)
        {
            return answer;
        }
        // The frame at which the command is drained and applied (before it runs); a `sim.step`
        // advances the frame, so this is captured up front for the record stamp.
        let drain_frame = self.frame();
        let result = crate::dispatch::dispatch(self, command.clone());
        // Both stashes are taken always, so neither leaks to the next command; a doc reload is
        // recorded even when the command then failed, because the store swap already happened.
        let partial = self.applied_before_error.take();
        let applied_doc = self.pending_doc_reload.take();
        debug_assert!(
            partial.is_none() || result.is_err(),
            "a handler notes a partial application only when it fails"
        );
        let applied = Applied {
            mutated: match &result {
                Ok(_) => Some(command),
                Err(_) => partial,
            },
            result,
        };
        // The panicking tick is the last one the step ran, so the frame has advanced past it.
        let panicked_at = matches!(&applied.result, Err(d) if d.code == code::SYSTEM_PANICKED)
            .then(|| FrameIndex::new(self.frame().as_u64() - 1));
        self.record(
            applied.mutated.as_ref(),
            drain_frame,
            applied_doc,
            panicked_at,
        );
        applied.result
    }

    /// Answers a live command that arrives while a replay plays, or lets it through (`None`): input
    /// goes to the live `InputState`, `render.size` is deferred to the return, and every other
    /// command the recording would write (and a data-doc reload) is refused with `L1405`.
    fn gate_live_command(
        &mut self,
        command: &ludion_protocol::Command,
    ) -> Option<Result<Dispatched, Diagnostic>> {
        use ludion_protocol::{Command, ResponseBody};
        let body = match command {
            Command::InputInject(params) => {
                let event = match crate::commands::sim::parse_input_event(params) {
                    Ok(event) => event,
                    Err(diagnostic) => return Some(Err(diagnostic)),
                };
                self.playback.as_mut()?.pending_live.push(event);
                ResponseBody::Ack
            }
            Command::RenderSize(params) => {
                match crate::commands::render::requested_extent(params) {
                    Ok(Some((width, height))) => {
                        if let Err(diagnostic) = self.frame_size(width, height) {
                            return Some(Err(diagnostic));
                        }
                        self.playback.as_mut()?.saved.render_size = (width, height);
                        ResponseBody::RenderSize { width, height }
                    }
                    Ok(None) => return None,
                    Err(diagnostic) => return Some(Err(diagnostic)),
                }
            }
            Command::AssetReload(_) => {
                return Some(Err(
                    self.playback_refusal(command, "retry after the playback")
                ));
            }
            Command::SceneSave(_) | Command::PrefabCreate(_) | Command::PrefabApply(_) => {
                return Some(Err(self.playback_refusal(
                    command,
                    "it would write the replayed world to disk; retry after the playback",
                )));
            }
            _ if crate::recording::record_kind(command).is_some() => {
                return Some(Err(self.playback_refusal(
                    command,
                    "use the playback controls (`Engine::pause_playback`, …) or stop the playback first",
                )));
            }
            _ => return None,
        };
        Some(Ok(Dispatched {
            body,
            frame: self.frame(),
            game_frame: self.game_frame(),
        }))
    }

    fn playback_refusal(&self, command: &ludion_protocol::Command, help: &str) -> Diagnostic {
        let name = self.playback_name.as_deref().unwrap_or("");
        Diagnostic::new(
            code::REPLAY_PLAYBACK_FAILED,
            format!(
                "`{}` is refused while replay `{name}` plays",
                command.method()
            ),
        )
        .with_help(help.to_string())
    }

    /// Notes, for the [`execute`](Engine::execute) that is running the handler, the form of the
    /// command that actually mutated the world before the handler failed — what recording writes in
    /// place of the command that was sent.
    pub(crate) fn note_applied_before_error(&mut self, applied: ludion_protocol::Command) {
        self.applied_before_error = Some(applied);
    }

    /// Recovers from a command handler that panicked, and returns the `L4016` diagnostic that
    /// answers its request. Called by the live loop's unwind boundary.
    ///
    /// The stashed doc reload and partial application are dropped because [`execute`](Engine::execute)
    /// never reached its `take`, and they would otherwise be recorded against the *next* command. The
    /// handler may have half-applied its edit, which no record can describe, so an active recording
    /// stops here (`L3042`): the free-run ticks before the command are flushed first, and no keyframe is
    /// written because the world is the panic's wreckage. The driver is paused because nothing should
    /// tick on that world; the world itself is left as the panic left it, as evidence.
    pub(crate) fn command_panicked(&mut self, method: &str, message: &str) -> Diagnostic {
        self.pending_doc_reload = None;
        self.applied_before_error = None;
        self.driver.pause();
        let diagnostic = Diagnostic::new(
            code::COMMAND_PANICKED,
            format!(
                "the command `{method}` panicked on frame {}: {message}",
                self.frame().as_u64()
            ),
        )
        .with_help(
            "the sim is paused, the world is as the panic left it, and the recording stops here; \
             the panic's location is on the host's stderr; `engine.restore` or `scene.load` \
             gets back to a known world (`ludion explain L4016`)",
        );
        if let Some(mut recorder) = self.recorder.take() {
            // The flush is best effort: the recording is stopping anyway.
            let _ = crate::recording::flush_pending_ticks(&mut recorder, self.frame());
            self.recorder = Some(recorder);
            self.stop_recording_on_failure(&diagnostic);
        }
        diagnostic
    }

    // --- Shutdown ------------------------------------------------------------------------------

    /// Flags a graceful-shutdown request (the `engine.shutdown` handler) and wakes an armed host.
    ///
    /// The flag is what [`run_live`](crate::live::run_live) observes to leave its loop after replying;
    /// the wake channel (armed by a host via [`arm_shutdown_notify`](Engine::arm_shutdown_notify)) is
    /// what lets the host itself wake from its Ctrl-C wait and run the same graceful teardown. On a
    /// bare in-process engine there is no channel, so this only sets the observable flag — nothing
    /// exits the process.
    pub(crate) fn request_shutdown(&mut self) {
        self.shutdown_requested = true;
        if let Some(notify) = &self.shutdown_notify {
            // A capacity-1 channel; one wake is all a host needs, so a full/closed channel is fine.
            let _ = notify.try_send(());
        }
    }

    /// Whether an `engine.shutdown` has been requested — the live loop's exit condition, and a bare
    /// in-process caller's way to observe that a shutdown command ran.
    pub fn shutdown_requested(&self) -> bool {
        self.shutdown_requested
    }

    /// Arms the host wake channel a wire `engine.shutdown` trips, so the host can run its graceful
    /// teardown. A host installs this before the engine moves onto the sim thread (see
    /// `ludion_server::Server::wire_shutdown`).
    pub fn arm_shutdown_notify(&mut self, notify: crossbeam_channel::Sender<()>) {
        self.shutdown_notify = Some(notify);
    }

    /// Arms the host channel a fatal render fault is reported on: a lost device (`L2011`) or a dead
    /// window surface (`L2007`). The live loop stops rendering and reports the fault once, so the
    /// host can end the session. A host installs this before the engine moves onto the sim thread.
    pub fn arm_render_fault_notify(&mut self, notify: crossbeam_channel::Sender<Diagnostic>) {
        self.render_fault_notify = Some(notify);
    }

    /// Test seam: the live loop's next pass absorbs `fault` as a failed render, so a host's fault
    /// wiring can be exercised without losing a real device.
    #[doc(hidden)]
    pub fn queue_render_fault(&mut self, fault: Diagnostic) {
        self.queued_render_fault = Some(fault);
    }

    /// The live loop's one drain of render-side trouble: a queued test fault first, then whatever
    /// the presenter thread reported — probe resolves feed the store warning, non-fatal failures
    /// become `log` events, and the first fatal fault is returned (the presenter has stopped).
    pub(crate) fn take_queued_render_fault(&mut self) -> Option<Diagnostic> {
        if let Some(fault) = self.queued_render_fault.take() {
            return Some(fault);
        }
        let notes = self.presenter.as_ref()?.take_notes();
        let mut fault = None;
        for note in notes {
            match note {
                PresenterNote::Probes(resolved) => self.note_probe_resolve(&resolved),
                PresenterNote::Log(diag) => {
                    self.push_event(Event::Log(LogEvent::from_diagnostic(diag)));
                }
                PresenterNote::Fault(diag) => {
                    fault.get_or_insert(diag);
                }
            }
        }
        fault
    }

    /// Reports a fatal render fault to an armed host. A full or closed channel is fine: the host
    /// needs one report.
    pub(crate) fn report_render_fault(&self, fault: &Diagnostic) {
        if let Some(notify) = &self.render_fault_notify {
            let _ = notify.try_send(fault.clone());
        }
    }

    // --- Scene loading --------------------------------------------------------------------------

    /// Replaces the world with the scene in `document` (inline canonical YAML text).
    ///
    /// Convenience for embedded scenes and self-contained replays; `scene.load` on the wire reads a
    /// file and funnels into the same core. See `load_scene_document`
    /// for the reset semantics.
    ///
    /// # Errors
    /// Every parse/validation/instantiation diagnostic the scene produced.
    pub fn load_inline_scene(&mut self, document: &str) -> Result<u32, Vec<Diagnostic>> {
        let count = self.load_scene_document(document.to_string())?;
        self.current_scene_path = None;
        Ok(count)
    }

    /// Core scene load: check → instantiate → replace world, reseed the RNG from the scene seed
    /// (default 0), reset the frame to 0 and the run-state to Running.
    ///
    /// This is the session's world-reset point, matching the replay header which captures the scene
    /// reference and seed as the session's starting conditions.
    ///
    /// The engine stores `document` inline ([`SceneRef::Inline`]); the checked parse seeds
    /// `Engine.authored`.
    pub(crate) fn load_scene_document(&mut self, document: String) -> Result<u32, Vec<Diagnostic>> {
        self.load_begin(LoadPhase::Scene);
        let profile_text = if self.boot_profile_pending {
            self.emit_profile_text()
        } else {
            None
        };
        let loaded = self.instantiate_and_commit(document, profile_text);
        self.load_end(LoadPhase::Scene);
        loaded
    }

    /// [`load_scene_document`](Engine::load_scene_document) that installs the live profile, unsaved
    /// writes included, instead of re-reading the save from disk: a run's reset keeps the profile.
    fn load_scene_carrying_profile(&mut self, document: String) -> Result<u32, Vec<Diagnostic>> {
        let profile_text = self.emit_profile_text();
        self.load_scene_with_profile(document, profile_text)
    }

    /// [`load_scene_carrying_profile`](Engine::load_scene_carrying_profile) with the profile text
    /// given: the live profile of a moment ago, such as the one a playback saved before it began.
    fn load_scene_with_profile(
        &mut self,
        document: String,
        profile_text: Option<String>,
    ) -> Result<u32, Vec<Diagnostic>> {
        self.load_begin(LoadPhase::Scene);
        let loaded = self.instantiate_and_commit(document, profile_text);
        self.load_end(LoadPhase::Scene);
        loaded
    }

    /// Starts `phase` on the load observer, if one is attached.
    fn load_begin(&mut self, phase: LoadPhase) {
        if let Some(observer) = self.asset_cache.observer_mut() {
            observer.begin(phase);
        }
    }

    /// Ends `phase` on the load observer, if one is attached.
    fn load_end(&mut self, phase: LoadPhase) {
        if let Some(observer) = self.asset_cache.observer_mut() {
            observer.end(phase);
        }
    }

    /// The body of [`load_scene_document`](Engine::load_scene_document), bracketed by its caller so
    /// the scene phase closes on every error return too.
    fn instantiate_and_commit(
        &mut self,
        document: String,
        profile_text: Option<String>,
    ) -> Result<u32, Vec<Diagnostic>> {
        let mut reads = self.load_reads();
        let Instantiated {
            source,
            scene,
            mut world,
            placements: prefab_placements,
        } = instantiate_named_world(
            &document,
            &self.registry,
            self.project_root(),
            &mut |path| reads.resolve(path),
        )?;
        // The data-doc stores are engine configuration derived from committed content, not world
        // state, so they survive the reset. A malformed doc fails the load exactly as a bad scene
        // does — before anything on the engine has changed.
        self.load_begin(LoadPhase::Docs);
        let docs = self.prepare_docs_for(&mut world, &crate::docs::DocReloadLog::default());
        self.load_end(LoadPhase::Docs);
        let docs = docs?;
        let seed = scene.seed.unwrap_or(0);
        let count = world.entity_count() as u32;
        // Loading a scene resets the world, but the systems must survive it (they are engine
        // configuration, not world state), so the schedule is rebuilt from the persistent list.
        let schedule = self.build_schedule(scene.physics.as_ref());
        let driver = SimDriver::new(
            world,
            schedule,
            SimConfig {
                seed,
                ..SimConfig::default()
            },
        );
        let prefab_library =
            self.prepare_prefab_library(&source, scene.physics.clone(), driver.frame(), reads)?;
        self.commit_world(PreparedWorld {
            driver,
            seed,
            scene: Some(SceneRef::Inline { document }),
            scene_name: scene.name.clone(),
            scene_physics: scene.physics.clone(),
            docs,
            prefab_library: Some(prefab_library),
            prefab_placements,
            kind: Rebuild::Load {
                authored: authored_names(&scene),
                source: Box::new(source),
                profile_text,
            },
        });
        Ok(count)
    }

    /// Reseeds the world's RNG (and the recorded seed) to `seed`, leaving the world otherwise
    /// unchanged. Used by replay to honor the header seed after loading the scene.
    pub(crate) fn reseed(&mut self, seed: u64) {
        self.seed = seed;
        self.driver
            .world_mut()
            .insert_resource(DeterministicRng::new(seed));
        self.driver.world_mut().insert_resource(WorldSeed(seed));
    }

    // --- Handler-facing accessors (pub(crate)) --------------------------------------------------

    /// The driver, mutably (run control and input injection).
    pub(crate) fn driver_mut(&mut self) -> &mut SimDriver {
        &mut self.driver
    }

    /// Disjoint borrows of the registry and the world — component handlers need both at once.
    pub(crate) fn parts_mut(&mut self) -> (&Registry, &mut World) {
        (&*self.registry, self.driver.world_mut())
    }

    /// The subscription table, mutably.
    pub(crate) fn subscriptions_mut(&mut self) -> &mut Subscriptions {
        &mut self.subscriptions
    }

    /// The subscription table, for read-only queries (open channels, frame subscribers).
    pub(crate) fn subscriptions(&self) -> &Subscriptions {
        &self.subscriptions
    }

    /// Queues an event for the live loop to fan out to matching subscribers.
    ///
    /// Producers (`scene.load`, the asset-reload commands) push here;
    /// [`run_live`](crate::live::run_live) drains the outbox once per pass, addresses each event to
    /// every open subscriber on its channel, and drops any event with no subscriber (there is no
    /// buffering for future subscribers, per architecture §6). With no transport the loop discards
    /// every event each pass (stderr already printed any diagnostic). Outside the live loop nothing
    /// drains the outbox, so it lives as long as that finite drive.
    pub(crate) fn push_event(&mut self, event: Event) {
        self.event_outbox.push(event);
    }

    /// Removes and returns every queued event (see [`push_event`](Engine::push_event)).
    pub(crate) fn take_events(&mut self) -> Vec<Event> {
        std::mem::take(&mut self.event_outbox)
    }

    /// Extracts the current world once, stamped with the given `frame`, and feeds it to whichever
    /// consumers the two flags ask for: `readback` renders it on this thread, files it into the stream
    /// ring and non-blocking-collects every finished readback and keeps the newest (the live
    /// frame-stream production path — see [`emit_egress`](crate::live)); `handover` publishes the
    /// extracted scene to the presenter thread, which renders and presents it on its own renderer
    /// (`tick` is the live loop's wall-clock stamp for the presenter's trace). Unlike the
    /// blocking screenshot path (`render.screenshot`) the readback does not stall the sim thread:
    /// it pipelines up to [`StreamReadback::DEPTH`] frames, blocking only when every ring slot is
    /// in flight (it then waits once and drains the ring; ~2 frames is the worst-case latency).
    ///
    /// Returns the collected `(frame index, image)` — the frame that readback actually finished, an
    /// older frame than the one just submitted — or `None` if the submitted frame's predecessors are
    /// still in flight (and always `None` with `readback` off, which touches no ring slot). Like the
    /// blocking path, this is a pure read of the world: it never mutates sim state or the
    /// `WorldHash`. The ring is created lazily and recreated when the render size changes.
    ///
    /// A handover-only frame still extracts, advances GI and is measured by the profiler, but it
    /// creates no sim-side renderer or ring: the sim thread never draws for the window.
    ///
    /// # Errors
    /// `L2003`/`L2004` if no GPU is available; a collapsed extract diagnostic (`L1101`/`L2101`…) if the
    /// world cannot be extracted; `L2005` on a readback failure. A dead window surface (`L2007`) or
    /// lost device (`L2011`) on the presenter arrives through
    /// [`take_queued_render_fault`](Engine::take_queued_render_fault).
    ///
    /// While the live profiler is armed this also times the two host spans of the frame
    /// (`extract` and the sim-side `submit`), arms the frame's per-pass GPU timing, and drains
    /// whatever timing samples the ring has finished — samples for *earlier* frames, filed by their
    /// own stamp.
    pub(crate) fn render_frame(
        &mut self,
        frame: FrameIndex,
        readback: bool,
        handover: bool,
        tick: Option<TickStamp>,
    ) -> Result<Option<(FrameIndex, RenderedImage)>, Diagnostic> {
        let (width, height) = self.render_size;
        let size = self.frame_size(width, height)?;
        let span = self.perf.is_some().then(Span::start);
        // The stream is the one path the view override moves: a subscriber sees where a human flew.
        let publish = handover && self.presenter.is_some();
        let collect = publish && self.present_interpolation;
        let mut blend = BlendData::default();
        let scene = Arc::new(
            self.extract_render_scene_blend(width, height, true, collect.then_some(&mut blend))
                .map_err(crate::commands::collapse)?,
        );
        let extract_ms = span.map(Span::ms);
        if publish && let Some(presenter) = self.presenter.as_ref() {
            presenter.publish(PresentPacket {
                frame,
                scene: Arc::clone(&scene),
                size,
                settings: self.present_settings(),
                tick,
                blend: collect.then(|| Arc::new(blend)),
                camera_latch: (self.late_latch && self.view_override.is_none())
                    .then(|| crate::camera_latch::collect(self.driver.world(), (width, height)))
                    .flatten(),
            });
        }
        if !readback {
            if let Some(perf) = self.perf.as_mut() {
                perf.record_render(frame, extract_ms, None);
            }
            self.finish_gi_frame();
            return Ok(None);
        }
        self.renderer()?; // lazily create the renderer before borrowing its ring field disjointly
        // The ring is sized from the *output* extent — the render scale never changes how many bytes
        // come back — so a scale change cannot stale it.
        let stale = match &self.stream_ring {
            Some(ring) => ring.dimensions() != (width, height),
            None => true,
        };
        if stale {
            let renderer = self.renderer.as_ref().expect("just initialized");
            self.stream_ring = Some(renderer.new_stream_readback(size)?);
        }
        // Disjoint field borrows: the renderer, the readback ring and the profiler's timing ring.
        let renderer = self.renderer.as_ref().expect("renderer initialized");
        let ring = self.stream_ring.as_mut().expect("ring just ensured");
        let timing = self.perf.as_mut().map(|perf| perf.timing_ring(renderer));
        let sinks = FrameSinks {
            readback: true,
            present: None,
        };
        let span = timing.is_some().then(Span::start);
        let collected = renderer.render_frame(&scene, size, ring, frame, timing, sinks);
        let submit_ms = span.map(Span::ms);
        if let Some(perf) = self.perf.as_mut() {
            perf.record_render(frame, extract_ms, submit_ms);
        }
        self.collect_perf_timings();
        self.finish_gi_frame();
        self.finish_probe_frame();
        collected
    }

    /// Drains the live profiler's finished GPU timing samples, once per rendered frame. A no-op
    /// when disarmed or before any adapter exists.
    fn collect_perf_timings(&mut self) {
        // Disjoint field borrows: the profiler drains its own ring through the renderer's context.
        let (Some(renderer), Some(perf)) = (self.renderer.as_ref(), self.perf.as_mut()) else {
            return;
        };
        perf.collect_gpu(renderer);
    }

    // --- Live profiling -------------------------------------------------------------------------

    /// Arms the live profiler: from the next tick on, the engine measures the frames it is already
    /// producing into [`ProfileWindowData`]s a host takes with
    /// [`perf_take_windows`](Engine::perf_take_windows). Idempotent — arming an armed engine keeps
    /// the windows it has already filled.
    ///
    /// Arming is the whole switch. A disarmed engine pays one `Option` test per tick and reads no
    /// clock at all, which is why nothing here is on by default (see [`crate::perf_live`]).
    pub fn perf_arm(&mut self) {
        if self.perf.is_none() {
            self.perf = Some(Box::new(LivePerf::new()));
        }
    }

    /// Disarms the live profiler, dropping every partially filled window, every completed one not
    /// yet taken, and the GPU timing ring. The next tick is back on the unmeasured path.
    pub fn perf_disarm(&mut self) {
        self.perf = None;
    }

    /// Whether the live profiler is armed.
    pub fn perf_armed(&self) -> bool {
        self.perf.is_some()
    }

    /// Takes every completed rolling [`ProfileWindowData`], oldest first (empty when disarmed, or when none
    /// has closed since the last take).
    pub fn perf_take_windows(&mut self) -> Vec<ProfileWindowData> {
        match self.perf.as_mut() {
            Some(perf) => perf.take_windows(),
            None => Vec::new(),
        }
    }

    /// Arms a capture over the next `frames` live frames, arming the profiler itself if it is not
    /// already. Returns `false` — arming nothing — when a capture is already in flight or its report
    /// has not been taken.
    ///
    /// The report lands in [`perf_take_capture`](Engine::perf_take_capture) once the window has its
    /// frames (and their late GPU samples); the rolling stream keeps filling underneath it.
    pub fn perf_begin_capture(&mut self, frames: u64) -> bool {
        self.perf_arm();
        self.perf
            .as_mut()
            .expect("just armed")
            .begin_capture(frames)
    }

    /// Whether a capture is armed, or has completed and not yet been taken.
    pub fn perf_capture_in_flight(&self) -> bool {
        self.perf
            .as_ref()
            .is_some_and(|perf| perf.capture_in_flight())
    }

    /// Takes the completed capture report, if one is waiting.
    pub fn perf_take_capture(&mut self) -> Option<ProfileWindowData> {
        self.perf.as_mut().and_then(|perf| perf.take_capture())
    }

    /// Frames the open capture still needs, or `None` when no capture is open.
    pub fn perf_capture_remaining(&self) -> Option<u64> {
        self.perf.as_ref().and_then(|perf| perf.capture_remaining())
    }

    /// Drops the open (or finished, untaken) capture so the next one can arm.
    pub fn perf_cancel_capture(&mut self) {
        if let Some(perf) = self.perf.as_mut() {
            perf.cancel_capture();
        }
    }

    /// Drains every finished readback from the live stream ring and returns the newest, without
    /// submitting a new frame, so the tail frames of a burst still arrive after the world stops
    /// changing. `None` if the ring is absent or its oldest readback is still in flight.
    ///
    /// # Errors
    /// `L2005` if a readback's map completed with an error.
    pub(crate) fn collect_stream_ring(
        &mut self,
    ) -> Result<Option<(FrameIndex, RenderedImage)>, Diagnostic> {
        let Some(renderer) = self.renderer.as_ref() else {
            return Ok(None);
        };
        let Some(ring) = self.stream_ring.as_mut() else {
            return Ok(None);
        };
        renderer.collect_streamed(ring)
    }

    /// Discards the live stream ring and any in-flight readbacks (dropping the wgpu buffers unmaps
    /// them cleanly). Called when the last `frames` subscriber goes away, so no readback is ever
    /// collected for zero subscribers, and when a load or restore starts a new frame timeline, so
    /// no pre-commit readback is stamped with a post-commit run.
    pub(crate) fn discard_stream_ring(&mut self) {
        self.stream_ring = None;
    }

    /// The current scene reference (for snapshot / replay headers).
    pub(crate) fn scene_ref(&self) -> Option<&SceneRef> {
        self.scene.as_ref()
    }

    /// The loaded scene's `name:` field, retained for `scene.save` and snapshots.
    pub(crate) fn scene_name(&self) -> Option<&str> {
        self.scene_name.as_deref()
    }

    /// The loaded scene's `physics:` block, retained for snapshots and re-read by every world
    /// rebuild.
    pub(crate) fn scene_physics(&self) -> Option<&ScenePhysics> {
        self.scene_physics.as_ref()
    }

    /// The `Name`s the live world's scene document authors — what a residue capture stamps each record
    /// with (see [`SessionState::authored_names`]). Derived from the inline scene text after a
    /// restore; empty for a world with no parsable inline document behind it.
    pub(crate) fn authored_names(&self) -> std::borrow::Cow<'_, StableSet<String>> {
        match &self.session.authored_names {
            Some(names) => std::borrow::Cow::Borrowed(names),
            None => std::borrow::Cow::Owned(
                self.authored_scene()
                    .map(authored_names)
                    .unwrap_or_default(),
            ),
        }
    }

    /// The injected events not yet applied (for snapshots).
    pub(crate) fn pending_input(&self) -> ludion_sim::InputScript {
        self.driver.pending_input()
    }

    /// The world's single [`DeterministicRng`] resource (for snapshots).
    ///
    /// # Errors
    /// A snapshot-format diagnostic if the resource is somehow absent (it is inserted at every world
    /// construction, so this is a should-not-happen guard).
    pub(crate) fn rng(&self) -> Result<&DeterministicRng, Diagnostic> {
        self.driver
            .world()
            .resource::<DeterministicRng>()
            .ok_or_else(|| {
                Diagnostic::new(
                    ludion_core::codes::code::PROTOCOL_SNAPSHOT_INVALID,
                    "the world has no DeterministicRng resource to snapshot",
                )
            })
    }

    /// Renders the current post-step world at `width` × `height` and encodes it as PNG bytes.
    ///
    /// This is the reusable core shared by the `render.screenshot` handler (which base64-wraps the
    /// bytes for the wire) and the offline replay-to-frames render
    /// ([`replay_render_frames`](crate::replay_render_frames), which writes them straight to disk).
    /// Deterministic on a given adapter: two calls at the same frame produce byte-identical PNGs.
    ///
    /// # Errors
    /// `L2003` if no GPU adapter is available (callers graceful-skip on it), or any extract/render/
    /// encode diagnostic.
    pub fn render_png(&mut self, width: u32, height: u32) -> Result<Vec<u8>, Diagnostic> {
        self.render_png_through(width, height, false)
    }

    /// The human-readable adapter this engine renders on, `"<name> (<backend>)"`, e.g.
    /// `"NVIDIA GeForce RTX 3090 (Vulkan)"` — what a golden's provenance line classifies.
    /// Creates the renderer if it has none.
    ///
    /// # Errors
    /// `L2003`/`L2004` as [`render_png`](Engine::render_png).
    pub fn adapter_label(&mut self) -> Result<String, Diagnostic> {
        let renderer = self.renderer()?;
        Ok(format!(
            "{} ({})",
            renderer.adapter_name(),
            renderer.adapter_backend()
        ))
    }

    /// [`render_png`](Engine::render_png), through the view override when one is in force.
    ///
    /// The `camera: "view"` half of `render.screenshot`. `use_view` false is exactly `render_png`,
    /// which is why the default keeps every existing caller — the CLI, the offline replay render,
    /// every golden — on the scene's own camera whether or not a human is flying.
    pub fn render_png_through(
        &mut self,
        width: u32,
        height: u32,
        use_view: bool,
    ) -> Result<Vec<u8>, Diagnostic> {
        self.render_image(width, height, use_view)
            .map_err(crate::commands::collapse)?
            .encode_png()
    }

    /// Renders the current post-step world and returns the image unencoded — the body of
    /// [`render_png_through`](Engine::render_png_through), and the one place the load observer's
    /// first-frame phases are bracketed (the extract, the renderer's creation, its uploads and the
    /// draw).
    ///
    /// # Errors
    /// Extract's diagnostics as they come; any renderer diagnostic (`L2003` among them) alone.
    pub(crate) fn render_image(
        &mut self,
        width: u32,
        height: u32,
        use_view: bool,
    ) -> Result<RenderedImage, Vec<Diagnostic>> {
        let size = self.frame_size(width, height).map_err(|d| vec![d])?;
        self.load_begin(LoadPhase::FirstFrame);
        let image = self.render_image_inner(width, height, use_view, size);
        self.load_end(LoadPhase::FirstFrame);
        image
    }

    /// [`render_image`](Engine::render_image) past its `FirstFrame` bracket, which its caller closes
    /// on every path.
    fn render_image_inner(
        &mut self,
        width: u32,
        height: u32,
        use_view: bool,
        size: FrameSize,
    ) -> Result<RenderedImage, Vec<Diagnostic>> {
        let scene = self.extract_render_scene_through(width, height, use_view)?;
        self.ensure_renderer().map_err(|d| vec![d])?;
        // Detached for the render and put back before anything can return, so an error path cannot
        // leave the engine observer-less.
        let mut observer = self.asset_cache.take_observer();
        let renderer = self.renderer.as_ref().expect("just initialized");
        let image = match observer.as_deref_mut() {
            Some(observer) => {
                renderer.render_observed(&scene, size, &mut FrameLoadObserver(observer))
            }
            None => renderer.render(&scene, size),
        };
        self.asset_cache.set_observer(observer);
        self.finish_gi_frame();
        self.finish_probe_frame();
        image.map_err(|d| vec![d])
    }

    /// The lazily-initialized GPU renderer, requesting an adapter on first use.
    ///
    /// # Errors
    /// `L2003`/`L2004` if no GPU adapter or device is available (callers graceful-skip on `L2003`).
    pub(crate) fn renderer(&mut self) -> Result<&Renderer, Diagnostic> {
        self.ensure_renderer()?;
        Ok(self.renderer.as_ref().expect("just initialized"))
    }

    /// Creates the renderer if this host has none yet, and hands it this frame's GI hold — the
    /// render-side half of the rebake lever, applied at the one place every render path passes
    /// through, so a frame's bake and its voxelization always answer to the same lever state.
    ///
    /// # Errors
    /// `L2003`/`L2004` if no GPU adapter or device is available.
    fn ensure_renderer(&mut self) -> Result<(), Diagnostic> {
        if self.renderer.is_none() {
            self.load_begin(LoadPhase::RendererInit);
            let renderer = self.new_renderer();
            self.load_end(LoadPhase::RendererInit);
            let mut renderer = renderer?;
            for (path, source) in &self.shader_reloads {
                let _ = renderer.reload_shader(path, source.clone());
            }
            self.renderer = Some(renderer);
        }
        let hold = self.gi_hold();
        let store = self.probe_store.clone();
        let anti_aliasing = self.anti_aliasing();
        let frustum_culling = self.frustum_culling;
        let renderer = self.renderer.as_mut().expect("just initialized");
        renderer.set_frustum_culling(frustum_culling);
        renderer.set_gi_hold(hold);
        renderer.set_anti_aliasing(anti_aliasing);
        renderer.set_probe_store(store);
        Ok(())
    }

    /// The lazily-initialized GPU renderer, mutably — for the dev-only shader reload, the one path
    /// that recreates a pipeline mid-session.
    ///
    /// # Errors
    /// `L2003`/`L2004` if no GPU adapter or device is available.
    pub(crate) fn renderer_mut(&mut self) -> Result<&mut Renderer, Diagnostic> {
        self.ensure_renderer()?;
        Ok(self.renderer.as_mut().expect("just initialized"))
    }

    /// Constructs a headless renderer carrying this host's [`quality`](Engine::quality), so both
    /// lazy-init sites produce a renderer already at the machine's setting rather than at the default.
    ///
    /// # Errors
    /// `L2003`/`L2004` if no GPU adapter or device is available.
    fn new_renderer(&self) -> Result<Renderer, Diagnostic> {
        let renderer = match &self.surface_gpu {
            Some(gpu) => Renderer::from_gpu(gpu.share()?)?,
            None => Renderer::new()?,
        };
        Ok(self.adopt_renderer(renderer))
    }

    /// The render-side state the presenter's renderer applies each frame: the same values
    /// [`ensure_renderer`](Engine::ensure_renderer) and [`adopt_renderer`](Engine::adopt_renderer)
    /// push into the sim-side one.
    fn present_settings(&self) -> PresentSettings {
        PresentSettings {
            quality: self.quality.quality,
            params: self.quality.params,
            anti_aliasing: self.anti_aliasing(),
            gi_hold: self.gi_hold(),
            probe_store: self.probe_store.clone(),
            frustum_culling: self.frustum_culling,
        }
    }

    /// Hands a freshly built renderer this host's [`quality`](Engine::quality) — the single owner of
    /// that step, so neither construction path (the lazy headless one, or an attached surface's) can
    /// leave a renderer at the default setting.
    fn adopt_renderer(&self, mut renderer: Renderer) -> Renderer {
        renderer.set_quality(self.quality.quality, self.quality.params);
        renderer
    }

    /// Attaches a window `surface` this host presents into, and starts the presenter thread that
    /// renders and presents it. Returns the [`PresentSize`] handle the window thread writes each
    /// resize into.
    ///
    /// A surface is a surface: the caller creates it from whatever window system it has (a winit
    /// window on the desktop, a platform view elsewhere) and this engine neither knows nor asks
    /// which. From here the live loop hands every ticked frame's extracted scene to the presenter,
    /// which draws it on its own renderer over the surface's device and presents display-paced
    /// (`Fifo`); the sim thread never renders for, or waits on, the window, and a headless host's
    /// commands are unchanged.
    ///
    /// Must be called **before the first frame**: the renderer owns its device, and a session cannot
    /// swap devices underneath the caches, rings and pipelines already built on the old one.
    ///
    /// # Errors
    /// `L2007` if this engine already has a renderer, or if the surface cannot be configured;
    /// `L2003`/`L2004` if no adapter can present to it or no device can be created.
    pub fn attach_surface(
        &mut self,
        instance: &wgpu::Instance,
        surface: wgpu::Surface<'static>,
        width: u32,
        height: u32,
    ) -> Result<PresentSize, Diagnostic> {
        if self.renderer.is_some() || self.presenter.is_some() {
            return Err(Diagnostic::new(
                code::WINDOW_PRESENTATION_UNAVAILABLE,
                "a renderer already exists; attach the surface before the first frame",
            ));
        }
        let gpu = Gpu::request_for_surface(instance, &surface)?;
        let target = PresentTarget::new(
            &gpu,
            surface,
            width,
            height,
            if self.low_latency { 1 } else { 2 },
        )?;
        let size = target.size_handle();
        let backend = WindowBackend::new(gpu.share()?, target)?;
        self.presenter = Some(Presenter::spawn(Box::new(backend), self.presenter_look()));
        self.surface_gpu = Some(gpu);
        Ok(size)
    }

    /// Hands the engine the window host's raw mouse total, which the presenter latches against
    /// when [`late_latch`](crate::EngineBuilder::late_latch) is on. Call before
    /// [`attach_surface`](Engine::attach_surface).
    pub fn set_look_motion(&mut self, look: Arc<ludion_renderer::LookMotion>) {
        self.look_motion = Some(look);
    }

    /// What the presenter gets: the host's mouse total, only with the late latch on.
    fn presenter_look(&self) -> Option<Arc<ludion_renderer::LookMotion>> {
        self.late_latch.then(|| self.look_motion.clone()).flatten()
    }

    /// Test seam: runs a presenter over `backend` in place of a window, so the sim/presenter
    /// decoupling is exercised without a surface.
    #[doc(hidden)]
    pub fn attach_present_backend(&mut self, backend: Box<dyn PresentBackend>) {
        self.presenter = Some(Presenter::spawn(backend, self.presenter_look()));
    }

    /// Whether a window surface is attached ([`attach_surface`](Engine::attach_surface)) — the live
    /// loop's test for whether a frame is owed to a window even with nothing subscribed.
    pub fn present_attached(&self) -> bool {
        self.presenter.is_some()
    }

    /// Enables dev-only shader hot reload for this host: an `asset.reload` naming a renderer `.wgsl`
    /// recreates its module + dependent pipeline instead of returning the not-enabled `L2008` error.
    /// Only a file inside the renderer's shader source dir ([`ludion_renderer::SHADER_SOURCE_DIR`])
    /// can stand in for an engine shader. A host switch, not a protocol command — set before the
    /// engine moves onto the sim thread. Never sim state (not hashed, not snapshotted); the shipping
    /// path is unaffected until a reload lands.
    pub fn enable_shader_reload(&mut self) {
        self.enable_shader_reload_from(ludion_renderer::SHADER_SOURCE_DIR);
    }

    /// [`enable_shader_reload`](Engine::enable_shader_reload) with `dir` as the shader directory — a
    /// copy of the engine's, so a test can edit shaders without touching the checkout.
    pub fn enable_shader_reload_from(&mut self, dir: impl Into<PathBuf>) {
        self.shader_reload_dir = Some(dir.into());
    }

    /// Whether dev-only shader hot reload is enabled (see [`enable_shader_reload`](Engine::enable_shader_reload)).
    pub fn shader_reload_enabled(&self) -> bool {
        self.shader_reload_dir.is_some()
    }

    /// Reloads a renderer shader from disk into the live renderer (the dev-only `asset.reload` shader
    /// branch). Reads `path` — absolute, or relative to the project root, either way inside the shader
    /// dir — hands the source to the renderer under its path within that dir (`mesh.wgsl`,
    /// `snippets/camera.wgsl`) for naga validation + dependent-pipeline recreation, and returns the
    /// reloaded source's content hash for the `asset.reloaded` event. Render-side only — no world
    /// state, no `WorldHash`, nothing a snapshot carries.
    ///
    /// # Errors
    /// `L1321` if the path lies outside the shader dir, or the dir is not on this machine (nothing is
    /// read);
    /// `L1101` if the file cannot be read;
    /// `L2009` if the file is no reloadable shader, the WGSL fails validation, or the recreated
    /// pipeline is layout-incompatible (the last good pipeline stays live).
    pub(crate) fn reload_shader(&mut self, path: &str) -> Result<u64, Diagnostic> {
        let (full, key) = self.shader_path(path)?;
        ludion_renderer::check_shader_reload(&key)?;
        let source = std::fs::read_to_string(&full).map_err(|error| {
            Diagnostic::new(
                code::ASSET_FILE_IO,
                format!("could not read shader `{}`: {error}", full.display()),
            )
        })?;
        if let Some(presenter) = self.presenter.as_ref() {
            presenter.reload_shader(&key, source.clone());
        }
        // No sim-side renderer yet: nothing to reload, and building one just for this would cost a
        // device. It is kept, and the renderer built later compiles from it.
        let Some(renderer) = self.renderer.as_mut() else {
            let hash = ludion_assets::content_hash(source.as_bytes());
            self.shader_reloads.retain(|(p, _)| *p != key);
            self.shader_reloads.push((key, source));
            return Ok(hash);
        };
        let hash = renderer.reload_shader(&key, source.clone())?;
        self.shader_reloads.retain(|(p, _)| *p != key);
        self.shader_reloads.push((key, source));
        Ok(hash)
    }

    /// Resolves a shader reload path to the file and its key within the shader dir: absolute, or
    /// relative to the project root, and either way a file inside the dir `enable_shader_reload*`
    /// named. The key is the dir-relative path joined with `/`.
    fn shader_path(&self, path: &str) -> Result<(PathBuf, String), Diagnostic> {
        let dir = self
            .shader_reload_dir
            .as_deref()
            .expect("shader reload is enabled before a reload is routed here");
        let refuse = |why: &str| {
            Diagnostic::new(
                code::CONTENT_PATH_OUTSIDE_ROOT,
                format!("shader path `{path}` {why}"),
            )
            .with_help(format!(
                "reload a shader by a path inside the renderer's shader directory `{}`, absolute \
                 or relative to the project root",
                dir.display()
            ))
        };
        let full = if Path::new(path).is_absolute() {
            PathBuf::from(path)
        } else {
            if let Some(refusal) = ludion_core::paths::content_refusal(path) {
                return Err(refuse(refusal.describe()));
            }
            self.project_root.join(path)
        };
        let canonical_dir = std::fs::canonicalize(dir).map_err(|_| {
            refuse(&format!(
                "cannot be reloaded: this binary was built at `{}`, which is not on this machine; \
                 shaders are compiled in",
                dir.display()
            ))
        })?;
        let canonical = std::fs::canonicalize(&full).map_err(|error| {
            Diagnostic::new(
                code::ASSET_FILE_IO,
                format!("could not read shader `{}`: {error}", full.display()),
            )
        })?;
        let relative = canonical.strip_prefix(&canonical_dir).map_err(|_| {
            refuse(&format!(
                "lies outside the renderer's shader directory `{}`",
                canonical_dir.display()
            ))
        })?;
        let key = relative
            .components()
            .map(|c| c.as_os_str().to_string_lossy())
            .collect::<Vec<_>>()
            .join("/");
        Ok((full, key))
    }

    /// Brings the asset cache to the world after a rebuild: keeps what the new world names and drops
    /// whatever changed on disk ([`AssetCache::retain_referenced`]), then — when `prefetch` — decodes
    /// the misses in parallel. Each step runs once per rebuild, at the first cache reader that asks
    /// for it, so a headless query pays no decode bill. Returns whether it prefetched.
    fn refresh_asset_cache(&mut self, prefetch: bool) -> bool {
        let revalidate = std::mem::take(&mut self.revalidate_pending);
        let prefetch = prefetch && std::mem::take(&mut self.prefetch_pending);
        let prune = std::mem::take(&mut self.prune_pending);
        if !revalidate && !prefetch && !prune {
            return false;
        }
        let refs =
            crate::extract::asset_refs(self.driver.world(), AssetRoot::content(&self.project_root));
        if revalidate {
            self.asset_cache
                .retain_referenced(AssetRoot::content(&self.project_root), &refs);
        } else if prune {
            self.asset_cache.evict_unreferenced(&refs);
        }
        if prefetch {
            self.asset_cache
                .prefetch(AssetRoot::content(&self.project_root), &refs);
        }
        prefetch
    }

    /// Extracts the current world into a [`RenderScene`] at `width` × `height`, resolving sprite
    /// textures **through** the CPU [`AssetCache`] (miss = load + decode from disk once, hit = a map
    /// lookup) — so the live stream and screenshots no longer re-read every PNG per render. `&mut`
    /// because a cache miss inserts.
    pub(crate) fn extract_render_scene(
        &mut self,
        width: u32,
        height: u32,
    ) -> Result<RenderScene, Vec<Diagnostic>> {
        self.extract_render_scene_through(width, height, false)
    }

    /// The scene a windowed host hands the presenter, with the blend data that rides beside it: the
    /// extract at the render size through the view override. A test seam; the engine collects the
    /// same pair itself on handover.
    ///
    /// # Errors
    /// The extract's collapsed diagnostic.
    #[doc(hidden)]
    pub fn extract_present_scene(&mut self) -> Result<(RenderScene, BlendData), Diagnostic> {
        let (width, height) = self.render_size;
        let mut blend = BlendData::default();
        let scene = self
            .extract_render_scene_blend(width, height, true, Some(&mut blend))
            .map_err(crate::commands::collapse)?;
        Ok((scene, blend))
    }

    /// The camera latch a windowed host's packet would carry for the current world, or `None`
    /// (not a first-person rig camera, or a view override in force). A test seam.
    #[doc(hidden)]
    pub fn present_camera_latch(&self) -> Option<Arc<dyn ludion_renderer::CameraLatch>> {
        let (width, height) = self.render_size;
        self.view_override
            .is_none()
            .then(|| crate::camera_latch::collect(self.driver.world(), (width, height)))
            .flatten()
    }

    /// [`extract_render_scene`](Engine::extract_render_scene), through the view override when
    /// `use_view` is set and one is in force.
    ///
    /// Keeping the two-argument shape means the callers with no opinion — the graph dump, the timing
    /// dump, the profiler — cannot accidentally express one.
    pub(crate) fn extract_render_scene_through(
        &mut self,
        width: u32,
        height: u32,
        use_view: bool,
    ) -> Result<RenderScene, Vec<Diagnostic>> {
        self.asset_cache.forget_failures();
        self.extract_render_scene_blend(width, height, use_view, None)
    }

    /// [`extract_render_scene_through`](Engine::extract_render_scene_through), also filling `blend`
    /// when given. The scene is identical either way.
    fn extract_render_scene_blend(
        &mut self,
        width: u32,
        height: u32,
        use_view: bool,
        mut blend: Option<&mut BlendData>,
    ) -> Result<RenderScene, Vec<Diagnostic>> {
        let mut scene =
            self.extract_render_scene_inner(width, height, use_view, blend.as_deref_mut())?;
        if let Some(blend) = blend {
            blend.epoch = self.present_epoch;
        }
        if !scene.probes.is_empty() {
            if self.probe_world_pending || self.probe_world.is_none() {
                self.probe_world = Some(ProbeWorld::new(scene.clone()));
                self.probe_world_pending = false;
            }
            scene.probe_world = self.probe_world.clone();
        }
        Ok(scene)
    }

    /// The extract without the probe world attached: what a freeze stores.
    fn extract_render_scene_inner(
        &mut self,
        width: u32,
        height: u32,
        use_view: bool,
        blend: Option<&mut BlendData>,
    ) -> Result<RenderScene, Vec<Diagnostic>> {
        // The CPU half of the rebake lever, applied where every cached extract passes: the same
        // answer `ensure_renderer` gives the bake, since nothing between them can move the lever
        // (commands apply at frame boundaries).
        let hold = self.gi_hold();
        self.asset_cache.set_gi_hold(hold);
        self.resolve_cook();
        let prefetched = self.refresh_asset_cache(true);
        // The sim clock the assembly clip evaluator poses against: read out by value first, so the
        // driver's immutable borrow below stays the only one. This machine's particle density comes
        // out with it, for the same borrow reason and from the same setting the renderer was built
        // with — the axis is spent here, in extract, and never reaches `ludion-renderer` at all.
        let now = self.driver.time();
        let particle_density = self.quality.params.particle_density;
        // Disjoint field borrows: the view override, the world (through the driver), the project root
        // and the cache are four separate fields, so this borrows each without aliasing `self`.
        let view = use_view.then_some(self.view_override.as_ref()).flatten();
        let world = self.driver.world();
        let root = AssetRoot::content(&self.project_root);
        let cache = &mut self.asset_cache;
        let projectile_draw = &mut self.projectile_draw;
        let mut degraded = Vec::new();
        if prefetched {
            let misses = cache.cook_misses();
            if !misses.is_empty() {
                let target = cache
                    .cook()
                    .and_then(crate::cooked_maps::CookLookup::target);
                // Keyed by the count so a retry that leaves fewer fallbacks reports again.
                degraded.push((
                    format!("textures: cooked ({} uncooked)", misses.len()),
                    crate::cooked_maps::not_cooked_warning(target, &misses),
                ));
            }
        }
        let scene = crate::extract::extract_through_blend(
            world,
            (width, height),
            root,
            cache,
            now,
            particle_density,
            view,
            &mut degraded,
            blend,
            Some(projectile_draw),
        )
        .map(|mut scene| {
            scene.debug_lines = crate::debug_draw::segments(world, &self.debug_draw);
            if use_view {
                scene.cull_camera = match self.cull_from {
                    CullFrom::Game if view.is_some() => {
                        crate::extract::stream_camera(world, (width, height), None).ok()
                    }
                    CullFrom::Frozen => self.frozen_cull,
                    _ => None,
                };
            }
            if self.debug_draw.enables(crate::debug_draw::CULL) {
                let levels = self
                    .renderer
                    .as_ref()
                    .is_none_or(Renderer::draws_lod_levels);
                scene.debug_lines.extend(ludion_renderer::cull_marks(
                    &scene,
                    self.quality.params.lod_bias,
                    levels,
                ));
            }
            if self.debug_draw.enables(crate::debug_draw::LOCAL_SHADOWS) {
                let edge = self.quality.local_shadow_edge();
                scene
                    .debug_lines
                    .extend(ludion_renderer::local_shadow_marks(&scene, edge));
            }
            scene.debug_view = self.debug_view;
            scene
        });
        // References that drew a fallback instead of failing the frame, latched once per offender.
        if let Some(sink) = self.driver.world_mut().resource_mut::<crate::Diagnostics>() {
            for (key, diagnostic) in degraded {
                sink.report_full(key, diagnostic);
            }
        }
        scene
    }

    /// The `render.bounds` query, through the view override when `use_view` is set and one is in
    /// force. Sizes document-backed meshes through the CPU [`AssetCache`] (`&mut` because a miss
    /// inserts), with the same disjoint field borrows as
    /// [`extract_render_scene_through`](Engine::extract_render_scene_through). After a rebuild it
    /// keeps and revalidates the cached assets like the first extract, but decodes only what it sizes.
    pub(crate) fn render_bounds(
        &mut self,
        use_view: bool,
    ) -> Result<([u32; 2], Vec<EntityBounds>), Diagnostic> {
        self.asset_cache.forget_failures();
        self.refresh_asset_cache(false);
        let size = self.render_size();
        let view = use_view.then_some(self.view_override.as_ref()).flatten();
        crate::bounds::compute(
            self.driver.world(),
            size,
            AssetRoot::content(&self.project_root),
            &mut self.asset_cache,
            view,
        )
    }

    /// The CPU asset cache (for the `asset.reload` handler's membership check).
    pub(crate) fn asset_cache(&self) -> &AssetCache {
        &self.asset_cache
    }

    /// The CPU asset cache, mutably (for `asset.reload` to drop and re-read an entry).
    pub(crate) fn asset_cache_mut(&mut self) -> &mut AssetCache {
        &mut self.asset_cache
    }

    /// Assembles the `engine.describe` document: engine/protocol versions, the schema hash, every
    /// registered component's JSON Schema (in registration order), every registered doc kind's JSON
    /// Schema (by kind, sorted), and the full protocol schema.
    pub(crate) fn describe_document(&self) -> Value {
        let mut components = serde_json::Map::new();
        for info in self.registry.iter() {
            components.insert(info.name().to_string(), info.schema().clone());
        }
        // The registered doc kinds' schemas — the *same* `schema_for!(T)` documents `check_doc`
        // validates a `kind:` file against, so a client's authoring form and the engine's validation
        // read one truth. Sorted by kind (registration order carries no meaning here, and a stable
        // key order keeps two describes of the same engine byte-identical); `serde_json`'s
        // `preserve_order` means insertion order is what ships, so the sort happens on the way in.
        let mut documents = serde_json::Map::new();
        for (kind, schema) in self
            .doc_kinds
            .iter()
            .map(|dk| (dk.kind.as_str(), &dk.schema))
            .collect::<BTreeMap<_, _>>()
        {
            documents.insert(kind.to_string(), schema.clone());
        }
        // The ordered schedule, the introspection seam the editor's Game Designer / Systems view
        // consumes. `order` is the registration index, which is the execution order (systems run
        // strictly in the order added). `file` is normalized to forward slashes so a path recorded on
        // Windows (where `Location::file` yields backslashes) matches one recorded elsewhere.
        // `fn_path` is the system's Rust module path (`std::any::type_name` of the registered fn) — it
        // is exposed here (a module path, not a filesystem path, so no on-disk layout leaks) because
        // jump-to-source needs it. `queries` is each declared `.query::<...>()` shape, its element
        // types resolved to registry component names (verified registered at build, so resolution
        // always hits) with their read/write access.
        // The live schedule is walked rather than reassembled, so the engine-owned systems
        // (`path_follow`, the physics triple, `camera_rig`) and the game's presentation systems
        // describe at the orders they actually run at.
        let physics = scene_physics::describe_entries();
        let systems: Vec<Value> = self
            .driver
            .system_names()
            .enumerate()
            .map(|(order, name)| {
                let mut entry = if let Some(spec) = self.systems.iter().find(|s| s.name == name) {
                    serde_json::json!({
                        "name": spec.name,
                        "order": 0,
                        "fn_path": spec.fn_path,
                        "registered_at": {
                            "file": spec.registered_at.file().replace('\\', "/"),
                            "line": spec.registered_at.line(),
                        },
                        "queries": self.declared_queries_document(spec),
                    })
                } else if name == crate::path_follow::SYSTEM_NAME {
                    crate::path_follow::describe_entry()
                } else if name == crate::pose_blend::SYSTEM_NAME {
                    crate::pose_blend::describe_entry()
                } else if name == crate::camera_rig::SYSTEM_NAME {
                    crate::camera_rig::describe_entry()
                } else {
                    physics
                        .iter()
                        .find(|e| e["name"] == name)
                        .cloned()
                        .unwrap_or_else(|| serde_json::json!({ "name": name }))
                };
                entry["order"] = order.into();
                entry
            })
            .collect();
        serde_json::json!({
            "engine_version": ENGINE_VERSION,
            "protocol_version": PROTOCOL_VERSION,
            "schema_hash": schema::schema_hash().to_string(),
            "components": Value::Object(components),
            "documents": Value::Object(documents),
            "systems": systems,
            // This host's render quality: the label of the choice plus the toggles it resolved to,
            // read-only — a client sees what this machine renders, and cannot negotiate it.
            "quality": {
                "preset": self.quality.label,
                "ssao": self.quality.quality.ssao,
                "shafts": self.quality.quality.shafts,
                "dof": self.quality.quality.dof,
                "gi_dynamic": self.quality.quality.gi_dynamic,
                "shadows": self.quality.quality.shadows,
            },
            // The anti-aliasing mode this host draws with, read-only like `quality`.
            "anti_aliasing": self.anti_aliasing().name(),
            // The `render.view` override in force, or `null` when the stream is on the scene's own
            // camera — host render state a client can read without a round trip of its own.
            "view": match &self.view_override {
                Some(pose) => serde_json::to_value(pose).unwrap_or(Value::Null),
                None => Value::Null,
            },
            "protocol": schema::protocol_schema(),
        })
    }

    /// The `queries` array for one system in `engine.describe`: each declared query as
    /// `{"components": [{"name": <registry name>, "access": "read"|"write"}, ...]}`, in declaration
    /// order (and tuple order within each query). Every declared type was verified registered at build
    /// (`L1125`), so the registry lookup always resolves; the Rust type name is a defensive fallback.
    fn declared_queries_document(&self, spec: &SystemSpec) -> Value {
        let queries: Vec<Value> = spec
            .declared_queries
            .iter()
            .map(|query| {
                let components: Vec<Value> = query
                    .iter()
                    .map(|elem| {
                        let name = self
                            .registry
                            .get_by_type_id(elem.type_id)
                            .map(|info| info.name().to_string())
                            .unwrap_or_else(|| elem.type_name.to_string());
                        serde_json::json!({
                            "name": name,
                            "access": if elem.write { "write" } else { "read" },
                        })
                    })
                    .collect();
                serde_json::json!({ "components": components })
            })
            .collect();
        Value::Array(queries)
    }

    /// The registered game systems (with their declared queries), for the query-drift verifier.
    pub(crate) fn systems(&self) -> &[SystemSpec] {
        &self.systems
    }

    /// Arms opt-in query-shape recording on the engine (default off; host/test only). Every world the
    /// engine builds from here — the current one, and any rebuilt by a scene load or district switch —
    /// records the component shape of each [`World::query`](ludion_ecs::World::query) the sim runs,
    /// attributed to the running system, and the observations accumulate across those resets. Off by
    /// default with zero sim cost (one `Option` check per query). Drain with
    /// [`take_query_observations`](Engine::take_query_observations). This never affects the `WorldHash`,
    /// a snapshot, or a replay — it only observes; it exists for the query-drift verifier and the
    /// editor's Game Designer view.
    ///
    /// # Example
    /// ```
    /// use ludion_engine::Engine;
    /// use ludion_protocol::Command;
    /// use ludion_protocol::command::SimStepParams;
    ///
    /// let mut engine = Engine::new();
    /// engine.load_inline_scene("format: 1\nentities: []\n").unwrap();
    /// engine.begin_query_recording();
    /// engine.execute(Command::SimStep(SimStepParams { ticks: 1, hash: false })).unwrap();
    /// let observed = engine.take_query_observations();
    /// // A bare engine runs only its own `path_follow` and `camera_rig`.
    /// assert_eq!(observed.systems().count(), 2);
    /// ```
    pub fn begin_query_recording(&mut self) {
        self.query_recording = true;
        self.query_observations = QueryObservations::default();
        self.arm_query_recording();
    }

    /// Stops recording and returns every query shape observed since [`begin_query_recording`](Engine::begin_query_recording),
    /// keyed by system name — draining the current world's observations plus the cross-reset
    /// accumulator. Leaves the engine non-recording.
    pub fn take_query_observations(&mut self) -> QueryObservations {
        self.drain_query_recording();
        self.query_recording = false;
        std::mem::take(&mut self.query_observations)
    }

    /// Arms recording on the current world, if the engine is recording. Called after every world reset.
    fn arm_query_recording(&mut self) {
        if self.query_recording {
            self.driver.world_mut().begin_query_recording();
        }
    }

    /// Drains the current world's observations into the engine accumulator, if recording — called before
    /// every world reset, so observations survive a scene load or district switch that rebuilds the world.
    fn drain_query_recording(&mut self) {
        if self.query_recording
            && let Some(observed) = self.driver.world_mut().take_query_recording()
        {
            self.query_observations.merge(observed);
        }
    }

    /// Serializes the current world into a [`Scene`] for `scene.save` (canonical YAML).
    ///
    /// Entities are visited in ascending slot-index order; each carries its registered components in
    /// registration order. A [`Name`](crate::components::Name) component is projected back out to the
    /// entity's `name:` field (never emitted as a component block — it is reserved), so a scene
    /// document round-trips byte-identically. The scene's `name`/`seed` are the ones retained at load
    /// (a programmatically-built world has neither: `name` and — absent a load — `seed` follow the
    /// engine's defaults). Provenance and seed presence round-trip from the source document, so a
    /// save of a generated scene keeps its drift-gate marker.
    ///
    /// A component's entity references (`Parent`'s implicit link, `Joint`'s `a`/`b`, and the
    /// optional `PatternPlayer.target` and `Volley.source`) link to live `EntityId`s, but a scene
    /// identifies an entity by **document position**, so each is remapped to
    /// `{index: <target's position in this save>, generation: 0}` — the shape
    /// [`instantiate`](ludion_scene::instantiate) reads back
    /// ([`ComponentInfo::entity_refs`]). Without the remap, live slot indices
    /// that no longer equal document positions (after any despawn) would be written verbatim and
    /// silently corrupt the reference on reload.
    ///
    /// # Errors
    /// `L1018` if any live *required* entity reference points at an entity not in the save set
    /// (dangling after a non-cascading `entity.destroy`), or any reference (optional or not) points
    /// at its own carrier where the field forbids that, or the tree links (`Parent`) form a cycle —
    /// the world cannot be expressed as a valid scene. A dangling optional reference is saved as
    /// `null`.
    ///
    /// A live prefab instance whose parts are all named `<instance>/<part>` is written as one
    /// instance entry (`prefab`, `presentation`, `transform`, `overrides`, `removed`) that loads
    /// back to the same world; instances whose parts are not so named stay plain entities carrying
    /// their `PrefabInstance`. The `L1234` refusals are in the `collapse` module.
    pub(crate) fn world_to_scene(&self) -> Result<Scene, Diagnostic> {
        let world = self.driver.world();
        let (mut scene, live, authored) = self.save_inputs()?;
        if live
            .iter()
            .any(|&id| world.has::<crate::prefab::PrefabInstance>(id))
        {
            scene.entities = crate::collapse::collapse(
                self.save_live(authored),
                std::mem::take(&mut scene.entities),
                &live,
            )?;
        }
        Ok(scene)
    }

    /// The flat scene of the live world, its entity ids and the authored source document: what a
    /// save collapses.
    fn save_inputs(&self) -> Result<(Scene, Vec<EntityId>, Option<&Scene>), Diagnostic> {
        let world = self.driver.world();
        let (entities, live) = flatten_world(world, &self.registry)?;
        // The source document owns provenance and seed presence; a world with no parsable document
        // behind it is not map-generated and always states the seed it runs with.
        let authored = self.authored_scene();
        let (generated_from, seed) = authored.map_or((None, Some(self.seed)), |scene| {
            (scene.generated_from.clone(), scene.seed)
        });
        let scene = Scene {
            name: self.scene_name.clone(),
            generated_from,
            seed,
            physics: self.scene_physics.clone(),
            preload: authored.map_or_else(Vec::new, |scene| scene.preload.clone()),
            entities,
        };
        if let Some(cycle) = ludion_scene::hierarchy_cycles(&scene, &self.registry).first() {
            return Err(hierarchy_fault(world, &live, cycle));
        }
        Ok((scene, live, authored))
    }

    fn save_live<'a>(&'a self, authored: Option<&'a Scene>) -> crate::collapse::Live<'a> {
        let world = self.driver.world();
        crate::collapse::Live {
            world,
            registry: &self.registry,
            project_root: &self.project_root,
            authored,
            library: world.resource::<crate::prefab::PrefabLibrary>(),
            exact_fallback: true,
            placement: None,
            stubs: None,
            positions: None,
        }
    }

    /// What a save would write for each prefab instance: per instance root, its prefab, and the
    /// collapse's result (`None` for an unnamed spawn). `instances` names the ones wanted by any of
    /// their parts, all when `None`.
    ///
    /// # Errors
    /// `L1235` for an entity that is not part of an instance; `L1018` for instances whose entities
    /// cannot be flattened.
    pub(crate) fn prefab_overrides(
        &self,
        instances: Option<&[EntityId]>,
    ) -> Result<Vec<crate::collapse::InstanceOverrides>, Diagnostic> {
        let world = self.driver.world();
        let mut roots: Vec<(EntityId, String)> = Vec::new();
        let mut note = |root: EntityId, source: String| {
            if !roots.iter().any(|(held, _)| *held == root) {
                roots.push((root, source));
            }
        };
        match instances {
            Some(requested) => {
                for &id in requested {
                    let (source, root, _) = crate::prefab_ops::instance_parts(world, id)?;
                    note(root, source);
                }
            }
            None => {
                for id in world.entities().chain(world.presentation_entities()) {
                    if let Ok(Some(held)) = world.get::<crate::prefab::PrefabInstance>(id) {
                        note(held.root, held.source.clone());
                    }
                }
            }
        }
        // A runtime room has no prefab text to diff against: refused here, out of the collapse.
        let rooms: Vec<Option<Diagnostic>> = roots
            .iter()
            .map(|(root, source)| {
                crate::prefab_ops::refuse_room(
                    world,
                    source,
                    *root,
                    (
                        ludion_core::codes::code::PREFAB_NOT_INSTANCE,
                        "prefab.overrides",
                        "diff against",
                    ),
                )
                .err()
            })
            .collect();
        let wanted: Vec<EntityId> = roots
            .iter()
            .zip(&rooms)
            .filter(|(_, room)| room.is_none())
            .map(|((root, _), _)| *root)
            .collect();
        let mut results = if wanted.is_empty() {
            Vec::new()
        } else {
            crate::collapse::instance_overrides(self.save_live(self.authored_scene()), &wanted)?
        }
        .into_iter();
        let library = world.resource::<crate::prefab::PrefabLibrary>();
        let mut memo: BTreeMap<String, Vec<String>> = BTreeMap::new();
        Ok(roots
            .into_iter()
            .zip(rooms)
            .map(|((root, prefab), room)| {
                let result = match room {
                    Some(refusal) => Err(refusal),
                    None => results.next().expect("a result per wanted root"),
                };
                let root_parts = match (&result, library) {
                    (Ok(_), Some(library)) => memo
                        .entry(prefab.clone())
                        .or_insert_with(|| library.root_parts_of(&prefab))
                        .clone(),
                    _ => Vec::new(),
                };
                crate::collapse::InstanceOverrides {
                    root,
                    prefab,
                    result,
                    root_parts,
                }
            })
            .collect())
    }

    /// The live source document, parsed, when the world has one.
    fn authored_scene(&self) -> Option<&Scene> {
        self.authored
            .get_or_init(|| {
                let Some(SceneRef::Inline { document }) = &self.scene else {
                    return None;
                };
                ludion_scene::parse(document).ok()
            })
            .as_ref()
    }

    /// The current world as canonical scene text — the text-out twin of
    /// [`load_inline_scene`](Self::load_inline_scene). A trusted in-process caller that wants the
    /// text (a test, a host writing its own file) reads it here; `scene.save` on the wire writes the
    /// same text below the content root.
    ///
    /// # Errors
    /// `L1018` if the world cannot be expressed as a valid scene (a dangling reference or a
    /// `Parent` cycle); see `world_to_scene`.
    ///
    /// # Example
    ///
    /// ```
    /// use ludion_engine::Engine;
    ///
    /// let mut engine = Engine::new();
    /// engine.load_inline_scene("format: 1\nseed: 7\nentities: []\n").unwrap();
    /// let text = engine.scene_text().unwrap();
    /// assert!(text.contains("seed: 7"));
    /// ```
    pub fn scene_text(&self) -> Result<String, Diagnostic> {
        Ok(ludion_scene::emit(&self.world_to_scene()?, self.registry()))
    }

    // --- Snapshot / restore ---------------------------------------------------------------------

    /// Rebuilds the engine's world, RNG, frame, run-state, speed, scene reference, and seed from a
    /// snapshot document produced by `engine.snapshot`, returning a [`RestoreReport`] of any
    /// component-schema drift the migration worked around (empty = a clean restore). The registry,
    /// project root, render settings, subscriptions, and recording sink are left untouched.
    ///
    /// # Errors
    /// `L3008` if the document itself is malformed (wrong shape, unparseable, unsupported
    /// `format`/speed); `L0025` if its entity allocator state is inconsistent; `L3041` if its
    /// `allocator` field's live slots differ from its `entities` list. The engine is untouched on
    /// any of them. Per-component drift is migrated and reported, never an error — see
    /// [`restore_into`](crate::snapshot::restore_into).
    ///
    /// [`RestoreReport`]: ludion_protocol::restore::RestoreReport
    pub(crate) fn restore_from_value(
        &mut self,
        value: &Value,
    ) -> Result<ludion_protocol::restore::RestoreReport, Diagnostic> {
        crate::snapshot::restore_into(self, value, crate::snapshot::DriftPolicy::Migrate)
    }

    // --- Recording ------------------------------------------------------------------------------

    /// Begins recording the session to `sink`, writing the replay header (scene reference + seed +
    /// versions + schema hash) and an initial keyframe at the current frame.
    ///
    /// Arm this right after loading a scene, before the first tick, so the header captures the
    /// session's starting conditions. Every subsequent mutating command (and injected input) is
    /// written frame-stamped; keyframes are written every [`KEYFRAME_INTERVAL`] frames and one at stop. Pure reads
    /// leave no trace (see the `recording` module).
    ///
    /// # Errors
    /// `L3047` with no scene loaded or at a frame other than 0 (a scene load resets the frame); the
    /// engine is untouched. A replay-format diagnostic if the header or initial keyframe cannot be
    /// written to `sink`.
    pub fn set_recording(&mut self, sink: impl Write + Send + 'static) -> Result<(), Diagnostic> {
        let arm_help = "arm the recording right after loading the scene, before the first tick \
                        (a scene load resets the frame to 0)";
        if self.scene.is_none() {
            return Err(Diagnostic::new(
                code::RECORDING_ARM_REFUSED,
                "cannot arm a recording: no scene is loaded",
            )
            .with_help(arm_help));
        }
        if self.frame() != FrameIndex::ZERO {
            return Err(Diagnostic::new(
                code::RECORDING_ARM_REFUSED,
                format!(
                    "cannot arm a recording at frame {}: a replay starts from the scene's initial state",
                    self.frame().as_u64()
                ),
            )
            .with_help(arm_help));
        }
        if self
            .run_recording
            .as_ref()
            .is_some_and(|state| state.active.is_some())
        {
            return Err(Diagnostic::new(
                code::RECORDING_ARM_REFUSED,
                "cannot arm a recording: a run recording is active",
            )
            .with_help("stop the run recording first (`RunRecording::stop`)"));
        }
        self.arm_recorder(Box::new(sink), KEYFRAME_INTERVAL, None)
    }

    /// Writes the header, initial docs and keyframe to `sink` and starts recording at the current
    /// frame, with a keyframe every `keyframe_interval` ticks. The caller has checked the frame.
    fn arm_recorder(
        &mut self,
        sink: Box<dyn Write + Send>,
        keyframe_interval: u64,
        game: Option<ludion_protocol::replay::GameIdentity>,
    ) -> Result<(), Diagnostic> {
        let mut header = self.replay_header();
        header.game = game;
        let writer = ReplayWriter::new(sink, &header).map_err(io_to_diagnostic)?;
        let mut recorder = Recorder::new(writer, keyframe_interval, self.frame());
        crate::recording::write_inline_docs(&mut recorder, self.frame(), &self.doc_texts)
            .map_err(io_to_diagnostic)?;
        let snapshot = crate::snapshot::capture(self)?;
        crate::recording::write_keyframe(&mut recorder, self.frame(), &snapshot)
            .map_err(io_to_diagnostic)?;
        self.recorder = Some(recorder);
        self.recording_failure = None;
        if let Some(library) = self
            .driver
            .world_mut()
            .resource_mut::<crate::prefab::PrefabLibrary>()
        {
            library.relog_all();
        }
        self.drain_prefab_texts();
        Ok(())
    }

    /// Whether a recording session is currently active.
    pub fn is_recording(&self) -> bool {
        self.recorder.is_some()
    }

    /// Stops recording, flushing the sink. Idempotent. Writes a closing keyframe unless nothing was
    /// recorded since the last keyframe, the session ended on a system panic, or the capture fails
    /// with `L0007` (that keyframe is skipped and the failure reported).
    ///
    /// # Errors
    /// `L3042` if the recording died early (the failure is returned once); a replay-format
    /// diagnostic if the final flush fails.
    pub fn stop_recording(&mut self) -> Result<(), Diagnostic> {
        let closed = self.close_recorder();
        if self
            .run_recording
            .as_ref()
            .is_some_and(|state| state.active.is_some())
        {
            self.publish_run(closed.as_ref().err());
        }
        closed
    }

    /// The body of [`stop_recording`](Engine::stop_recording): closes the recorder and its sink.
    fn close_recorder(&mut self) -> Result<(), Diagnostic> {
        if self.recorder.is_none()
            && let Some(failure) = self.recording_failure.take()
        {
            return Err(failure);
        }
        if let Some(mut recorder) = self.recorder.take() {
            // Flush any free-run ticks batched since the last delta/keyframe so the trailing stretch
            // of a live session is present in the file (the current frame is the batch end).
            crate::recording::flush_pending_ticks(&mut recorder, self.frame())
                .map_err(io_to_diagnostic)?;
            if recorder.writer.records_written() != recorder.records_at_mark {
                let skipped = self.keyframe_or_skip(&mut recorder, self.frame())?;
                self.report_keyframe_skip(skipped);
            }
            recorder.writer.into_inner().map_err(io_to_diagnostic)?;
        }
        Ok(())
    }

    /// The replay header describing the current session's starting conditions.
    pub(crate) fn replay_header(&self) -> ReplayHeader {
        ReplayHeader {
            format_version: REPLAY_FORMAT_VERSION,
            engine_version: ENGINE_VERSION.to_string(),
            protocol_version: PROTOCOL_VERSION,
            schema_hash: schema::replay_schema_hash(),
            scene: self.scene.clone().unwrap_or(SceneRef::Inline {
                document: String::new(),
            }),
            seed: self.seed,
            profile: self.emit_profile_text(),
            render_extent: Some(ReplayExtent {
                width: self.render_size.0,
                height: self.render_size.1,
            }),
            replay_content_hash: Some(crate::replay_compat::replay_content_hash()),
            game: None,
        }
    }

    /// Records what a command applied (if it is recordable and a session is active) at its
    /// `drain_frame`, then writes a keyframe if the frame has crossed the [`KEYFRAME_INTERVAL`]
    /// boundary. `applied` is `None` when the command changed nothing.
    ///
    /// Infallible: the command already ran, so a write failure cannot un-apply it. It goes through
    /// `stop_recording_on_failure` instead of the caller's answer.
    fn record(
        &mut self,
        applied: Option<&ludion_protocol::Command>,
        drain_frame: FrameIndex,
        applied_doc: Option<AppliedDocReload>,
        panicked_at: Option<FrameIndex>,
    ) {
        self.write_command_record(applied, drain_frame, applied_doc, panicked_at);
        // A suspend during a lockstep `sim.step` ends the run recording once the step that carried it
        // is in the file; a free-running tick consumes the flag in `drain_run_recording`.
        let held_start = self
            .run_recording
            .as_mut()
            .filter(|state| state.suspended)
            .map(|state| state.start.take());
        if let Some(start) = held_start {
            self.drain_run_recording(self.frame());
            if let Some(state) = self.run_recording.as_mut() {
                state.start = start;
            }
        }
    }

    fn write_command_record(
        &mut self,
        applied: Option<&ludion_protocol::Command>,
        drain_frame: FrameIndex,
        applied_doc: Option<AppliedDocReload>,
        panicked_at: Option<FrameIndex>,
    ) {
        if applied.is_none() && applied_doc.is_none() && panicked_at.is_none() {
            return;
        }
        // Take the recorder out so its `&mut` does not alias the `&self` reads (frame, snapshot).
        let Some(mut recorder) = self.recorder.take() else {
            return;
        };
        match self.write_record(
            &mut recorder,
            applied,
            drain_frame,
            applied_doc.as_ref(),
            panicked_at,
        ) {
            Ok(skipped) => {
                self.recorder = Some(recorder);
                self.report_keyframe_skip(skipped);
            }
            Err(diagnostic) => self.stop_recording_on_failure(&diagnostic),
        }
    }

    /// The one policy for a recording write that fails after the command it records was applied —
    /// there is no caller to fail: recording stops (the recorder is already out of `self`, so it is
    /// dropped and the `.lrp` stays valid up to the last successful flush) and the reason is reported
    /// as `L3042` through `Diagnostics` — [`drain_diagnostics`](Engine::drain_diagnostics) prints it
    /// and emits the `log` event — and kept for [`stop_recording`](Engine::stop_recording) to return.
    fn stop_recording_on_failure(&mut self, diagnostic: &Diagnostic) {
        self.recorder = None;
        let frame = self.frame().as_u64();
        let message = format!(
            "the recording stopped at frame {frame}: {}: {}",
            diagnostic.code.as_str(),
            diagnostic.message
        );
        let mut help = String::from(
            "the .lrp is valid up to the last whole record (a torn final record is dropped with L3052); fix the cause and re-record",
        );
        if let Some(cause) = &diagnostic.help {
            help.push_str(" (");
            help.push_str(cause);
            help.push(')');
        }
        let stopped = Diagnostic::new(code::RECORDING_STOPPED, message).with_help(help);
        crate::diagnostics::report_full(
            self.driver.world_mut(),
            format!("frame {frame}"),
            stopped.clone(),
        );
        self.recording_failure = Some(stopped);
    }

    /// Writes one delta record and, if due, one keyframe (`Some` carries the diagnostic of a keyframe
    /// skipped for an `L0007` capture failure). Split from [`record`](Engine::record) so
    /// `recorder` is a local (disjoint from the `&self` reads it needs).
    fn write_record(
        &self,
        recorder: &mut Recorder,
        command: Option<&ludion_protocol::Command>,
        drain_frame: FrameIndex,
        applied_doc: Option<&AppliedDocReload>,
        panicked_at: Option<FrameIndex>,
    ) -> Result<Option<Diagnostic>, Diagnostic> {
        let scene_text = match &self.scene {
            Some(SceneRef::Inline { document }) => Some(document.as_str()),
            _ => None,
        };
        crate::recording::write_delta(recorder, command, drain_frame, applied_doc, scene_text)
            .map_err(io_to_diagnostic)?;
        if let Some(frame) = panicked_at {
            crate::recording::write_system_panic(recorder, frame).map_err(io_to_diagnostic)?;
            // A panicked tick's half-written world is never keyframed.
            return Ok(None);
        }

        let current = self.frame();
        if current.as_u64() >= recorder.last_keyframe.as_u64() + recorder.keyframe_interval {
            return self.keyframe_or_skip(recorder, current);
        }
        Ok(None)
    }
}

impl Drop for Engine {
    /// Finalizes an active recording when the engine is dropped.
    ///
    /// A live host ends its recording with [`stop_recording`](Engine::stop_recording) and surfaces
    /// its failure ([`run_live`](crate::live::run_live) returns it). Drop stays the backstop for an
    /// engine dropped without one: without it the trailing batched free-run ticks (buffered in
    /// `Recorder::pending_ticks` since the last keyframe/delta, up to a keyframe interval) would
    /// never be written. Idempotent: after a `stop_recording` the `recorder` is `None`, so this
    /// no-ops. A flush I/O error at drop has no caller to report to, so it is dropped.
    fn drop(&mut self) {
        if self.recorder.is_some() {
            let _ = self.stop_recording();
        }
    }
}

/// Remaps every entity reference `info`'s serialized `value` carries
/// ([`ComponentInfo::entity_refs`]) from a live [`EntityId`] to the scene shape
/// (`{index: <document position>, generation: 0}`). A component with no entity-reference fields
/// (the common case) is returned unchanged. An *optional* field that is absent or `null` stays so,
/// and a *nullable* one (`Option<EntityId>`) whose target is not in the save set is written as
/// `null`; an optional field that cannot hold `null` (a required field of an `Option<struct>`)
/// refuses a dangling target like a required one.
///
/// # Errors
/// `L1018` if a non-nullable reference's target is not in the save set, or a reference the field
/// forbids from naming its carrier does (never silently nulled — that would lose the data).
/// `L0007` if the component's serialized value omits a required reference field.
pub(crate) fn remap_entity_refs(
    world: &World,
    id: EntityId,
    info: &ComponentInfo,
    mut value: Value,
    position: &BTreeMap<EntityId, usize>,
) -> Result<Value, Diagnostic> {
    let component = info.name();
    for field in info.entity_refs() {
        let suffix = field.suffix.as_str();
        let raw = if suffix.is_empty() {
            &value
        } else if field.optional {
            match value.pointer(suffix) {
                Some(raw) if !raw.is_null() => raw,
                _ => continue,
            }
        } else {
            let Some(raw) = value.pointer(suffix) else {
                return Err(Diagnostic::new(
                    ludion_core::codes::code::COMPONENT_SERIALIZE_FAILED,
                    format!(
                        concat!(
                            "component `{}` on {} serialized without its required entity ",
                            "reference `{}`"
                        ),
                        component,
                        entity_label(world, id),
                        suffix.trim_start_matches('/'),
                    ),
                )
                .with_help(concat!(
                    "a field the schema requires must always serialize: drop its ",
                    "`skip_serializing_if`, or make it `Option<EntityId>`"
                )));
            };
            raw
        };
        let target = entity_id_from_value(raw);
        if !field.allow_self && target == id {
            return Err(reference_fault(
                world,
                id,
                component,
                suffix,
                RefFault::SelfReference,
            ));
        }
        if !ludion_ecs::is_presentation(id) && ludion_ecs::is_presentation(target) {
            return Err(reference_fault(
                world,
                id,
                component,
                suffix,
                RefFault::PresentationTarget(target),
            ));
        }
        let remapped = match position.get(&target) {
            Some(&pos) => serde_json::json!({ "index": pos, "generation": 0 }),
            None if field.nullable => Value::Null,
            None => {
                return Err(reference_fault(
                    world,
                    id,
                    component,
                    suffix,
                    RefFault::Dangling(target),
                ));
            }
        };
        if suffix.is_empty() {
            value = remapped;
        } else {
            *value.pointer_mut(suffix).expect("checked present above") = remapped;
        }
    }
    Ok(value)
}

/// Every live entity as a [`SceneEntity`] in live order (sim entities by slot, then presentation
/// ones), with each entity reference remapped to a position in that order, and the id at each
/// position. Provenance (`PrefabInstance`) is left out: the `collapse` module reads it from the world.
///
/// # Errors
/// `L1018` for a reference the save set cannot express; see `world_to_scene`.
pub(crate) fn flatten_world(
    world: &World,
    registry: &Registry,
) -> Result<(Vec<SceneEntity>, Vec<EntityId>), Diagnostic> {
    // Presentation entities are written after the sim ones, so document positions follow this
    // order; a sim entity can never reference one (`remap_entity_refs`).
    let live: Vec<EntityId> = world
        .entities()
        .chain(world.presentation_entities())
        .collect();
    let entities = flatten_ids(world, registry, &live, &|_| true)?;
    Ok((entities, live))
}

/// [`flatten_world`] for the entities `live`, in that order, which is also the position every
/// reference is remapped to. An entity for which `refs_of` is false is written without its
/// entity-reference components: it is present only to be referred to.
///
/// # Errors
/// `L1018` for a reference `live` cannot express.
pub(crate) fn flatten_ids(
    world: &World,
    registry: &Registry,
    live: &[EntityId],
    refs_of: &dyn Fn(EntityId) -> bool,
) -> Result<Vec<SceneEntity>, Diagnostic> {
    let position: BTreeMap<EntityId, usize> = live
        .iter()
        .enumerate()
        .map(|(pos, &id)| (id, pos))
        .collect();
    let mut entities = Vec::with_capacity(live.len());
    for &id in live {
        let mut components = Vec::new();
        for info in registry.iter() {
            // Reserved components (Name) are authoring fields, not blocks — skip them here and
            // read them out into their dedicated field below.
            if info.reserved() || info.name() == crate::collapse::PROVENANCE {
                continue;
            }
            if !info.entity_refs().is_empty() && !refs_of(id) {
                continue;
            }
            if let Ok(Some(value)) = info.serialize(world, id) {
                let value = remap_entity_refs(world, id, info, value, &position)?;
                components.push((info.name().to_string(), value));
            }
        }
        let name = world
            .get::<crate::components::Name>(id)
            .ok()
            .flatten()
            .map(|name| name.0.clone());
        entities.push(SceneEntity {
            name,
            presentation: ludion_ecs::is_presentation(id),
            components,
            instance: None,
        });
    }
    Ok(entities)
}

/// An `{index, generation}` JSON value (an entity reference's own serialized shape) as an
/// [`EntityId`]. A missing or wrong-typed field reads as `0` — unreachable for a real registered
/// component, whose `EntityId` field always serializes both.
fn entity_id_from_value(value: &Value) -> EntityId {
    let index = value.get("index").and_then(Value::as_u64).unwrap_or(0) as u32;
    let generation = value.get("generation").and_then(Value::as_u64).unwrap_or(0) as u32;
    EntityId::new(index, generation)
}

/// The entity `field` of a component's serialized `value` names, or `None` when an optional field
/// is absent or `null`.
pub(crate) fn ref_target(value: &Value, field: &ludion_ecs::EntityRefField) -> Option<EntityId> {
    let raw = if field.suffix.is_empty() {
        value
    } else {
        value.pointer(&field.suffix)?
    };
    (!raw.is_null()).then(|| entity_id_from_value(raw))
}

/// Why `scene.save` refused an entity reference.
enum RefFault {
    /// The target is not in the save set.
    Dangling(EntityId),
    /// The reference names its own carrier and the field forbids that.
    SelfReference,
    /// A sim entity's reference names a presentation entity.
    PresentationTarget(EntityId),
}

/// Builds the `L1018` diagnostic for a cycle of tree links, naming each entity on the loop by its
/// live id and `Name`. `live[position]` maps the cycle's document positions back to entities.
pub(crate) fn hierarchy_fault(
    world: &World,
    live: &[EntityId],
    cycle: &ludion_scene::HierarchyCycle,
) -> Diagnostic {
    let label = |position: usize| {
        let id = live[position];
        match world.get::<crate::components::Name>(id).ok().flatten() {
            Some(name) => format!("entity {id} (`{}`)", name.0),
            None => format!("entity {id}"),
        }
    };
    let mut chain: Vec<String> = cycle.members.iter().map(|&m| label(m)).collect();
    chain.push(label(cycle.members[0]));
    let component = &cycle.component;
    Diagnostic::new(
        ludion_core::codes::code::SCENE_SAVE_DANGLING_REF,
        format!(
            "cannot save: the `{component}` links form a cycle: {}",
            chain.join(" -> ")
        ),
    )
    .with_help(format!(
        "a hierarchy is a tree — `component.set` one `{component}` in the loop to another entity, \
         or `component.remove` it to make that entity a root"
    ))
}

/// `entity <id>`, with its `Name` when it has one.
fn entity_label(world: &World, entity: EntityId) -> String {
    match world.get::<crate::components::Name>(entity).ok().flatten() {
        Some(name) => format!("entity {entity} (`{}`)", name.0),
        None => format!("entity {entity}"),
    }
}

/// Builds the `L1018` diagnostic naming the referencing entity (by `Name` if it has one) and the
/// dead/absent target, or the forbidden self-reference. `suffix` is the entity-reference's
/// JSON-pointer suffix from [`ComponentInfo::entity_refs`] (`""` for `Parent`'s implicit link);
/// `Parent`'s wording stays byte-identical to before this generalized, a named field (`Joint`'s
/// `a`/`b`) gets analogous wording naming it.
fn reference_fault(
    world: &World,
    referrer: EntityId,
    component: &str,
    suffix: &str,
    fault: RefFault,
) -> Diagnostic {
    let label = entity_label(world, referrer);
    let field_name = suffix.trim_start_matches('/');
    let (pointing, names) = match fault {
        RefFault::Dangling(target) => (
            format!("at {target}, which is not in the scene"),
            "a live entity",
        ),
        RefFault::SelfReference => (
            "at itself, which the field forbids".to_string(),
            "a live entity other than its carrier",
        ),
        RefFault::PresentationTarget(target) => (
            format!("at presentation entity {target}; the sim never references presentation state"),
            "a sim entity",
        ),
    };
    let (message, help) = if field_name.is_empty() {
        (
            format!("cannot save: {label} has a `{component}` pointing {pointing}"),
            format!(
                "repair the reference first — `component.set` the `{component}` on {referrer} to \
                 {names}, or `component.remove` it"
            ),
        )
    } else {
        (
            format!("cannot save: {label}'s `{component}` has `{field_name}` pointing {pointing}"),
            format!(
                "repair the reference first — `component.set` the `{component}` on {referrer} so \
                 `{field_name}` names {names}, or `component.remove` it"
            ),
        )
    };
    Diagnostic::new(ludion_core::codes::code::SCENE_SAVE_DANGLING_REF, message).with_help(help)
}

/// The key `scene_bases` files a scene path under: the canonical path, so two spellings of one
/// file (case, `..`, symlinks) share a base. A path that does not exist keeps its own spelling —
/// `scene.save` looks a base up only for an existing destination and inserts after the write.
fn base_key(path: &Path) -> PathBuf {
    std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
}

/// Maps a replay-writing I/O error to an output-I/O diagnostic; a record the reader would refuse
/// (`RecordTooLarge`) keeps its own `L3017`.
fn io_to_diagnostic(error: std::io::Error) -> Diagnostic {
    if let Some(too_large) = error
        .get_ref()
        .and_then(|inner| inner.downcast_ref::<ludion_protocol::replay::RecordTooLarge>())
    {
        return too_large.to_diagnostic();
    }
    Diagnostic::new(
        ludion_core::codes::code::OUTPUT_IO,
        format!("could not write the replay session: {error}"),
    )
    .with_help("check the recording sink (disk space, permissions) and re-record the session")
}

/// `L3018` for a live switch whose target file could not be read from disk.
fn target_unresolved_on_disk(path: &Path, error: &std::io::Error) -> Diagnostic {
    Diagnostic::new(
        ludion_core::codes::code::TRANSITION_TARGET_UNRESOLVED,
        format!(
            "the target district `{}` could not be read for a scene switch: {error}",
            path.display()
        ),
    )
    .with_help(
        "check the district's scene file exists under the project root and is readable; the switch \
         was rejected and the sim continues in the current district",
    )
}

/// `L1321` for a live switch whose target path would leave the project root.
fn target_outside_root(scene_path: &str, why: &str) -> Diagnostic {
    Diagnostic::new(
        ludion_core::codes::code::CONTENT_PATH_OUTSIDE_ROOT,
        format!("the scene-switch target `{scene_path}` {why}"),
    )
    .with_help(
        "a gate's `target` names a scene relative to the project root and must stay inside it",
    )
}

/// `L3018` for a replayed switch whose target text is not in the embedded scene-text map — the
/// off-disk analog of an unreadable live target (decision 4: the same world no-op, different message).
fn target_unresolved_off_disk(scene_path: &str) -> Diagnostic {
    Diagnostic::new(
        ludion_core::codes::code::TRANSITION_TARGET_UNRESOLVED,
        format!(
            "the target district `{scene_path}` has no embedded text in this replay's scene-switch \
             records, so the switch could not be resolved off-disk"
        ),
    )
    .with_help(
        "a switch that succeeded at record time embeds its target text in the `.lrp`; a switch that \
         was rejected at record time embeds none and reproduces the same rejection under replay",
    )
}

/// The number of systems a single definition module may register before the boot emits an `L1124`
/// schedule-concentration warning. Past this, one module owns too much of the schedule — the catch-all
/// module smell the module-split convention exists to catch.
///
/// Calibrated against the real games: pre-split vigil defined 27 systems at the crate root and would
/// fire here, so the teeth are real; post-split vigil's largest module holds five or six systems and
/// every other game five or fewer, so all current games clear it with ~2x headroom. Twelve systems
/// plus the helpers they need is already a ~1,500-line file — past the point an agent's edit loop and
/// a human's reading both start to suffer.
const SYSTEMS_PER_MODULE_WARN_THRESHOLD: usize = 12;

/// The definition *module* of a registered system, derived from `std::any::type_name` of its function
/// type (a bare `fn` item's type name is its path, e.g. `vigil::movement::move_player`): strip any
/// generic arguments (truncate at the first `<`, so `spatial::rebuild::<Transform, TransformHierarchy>` and any other
/// instantiation group together), then drop the final `::segment` (the function name), leaving the
/// module path.
///
/// Returns `None` — degrading silently rather than risking a false positive — for a path that has no
/// `::` after stripping (a bare name) or that names a closure (`{{closure}}`). `type_name`'s exact
/// format is best-effort and not guaranteed, which is acceptable for a human-facing warning that never
/// touches sim state.
fn definition_module(fn_path: &str) -> Option<&str> {
    let base = fn_path.split('<').next().unwrap_or(fn_path);
    if base.contains("{{") {
        return None;
    }
    let module = &base[..base.rfind("::")?];
    (!module.is_empty()).then_some(module)
}

/// Groups the registered `systems` by their [`definition_module`] and returns one `L1124` warning per
/// module that concentrates more than [`SYSTEMS_PER_MODULE_WARN_THRESHOLD`] systems, in first-seen
/// module order. Each warning names the module, its system count, and the schedule's total, and points
/// at that module's first `.system(...)` wiring site (`file:line`). Systems whose `type_name` cannot be
/// parsed into a module are skipped, so they never inflate a count or invent a false positive.
///
/// A plain `Vec` (not a hash map) accumulates the groups: this runs once at build, not in the sim, and
/// insertion-order iteration keeps the output deterministic without a `type_name`-keyed hash.
fn schedule_module_warnings(systems: &[SystemSpec]) -> Vec<Diagnostic> {
    struct Group<'a> {
        module: &'a str,
        first_at: &'static std::panic::Location<'static>,
        count: usize,
    }
    let mut groups: Vec<Group<'_>> = Vec::new();
    for spec in systems {
        let Some(module) = definition_module(spec.fn_path) else {
            continue;
        };
        match groups.iter_mut().find(|g| g.module == module) {
            Some(group) => group.count += 1,
            None => groups.push(Group {
                module,
                first_at: spec.registered_at,
                count: 1,
            }),
        }
    }
    let total = systems.len();
    groups
        .iter()
        .filter(|group| group.count > SYSTEMS_PER_MODULE_WARN_THRESHOLD)
        .map(|group| {
            Diagnostic::new(
                code::SCHEDULE_MODULE_CONCENTRATION,
                format!(
                    "module `{}` defines {} of the schedule's {} systems — past the \
                     {}-per-module threshold, a single module concentrating this many systems is \
                     the catch-all schedule module the module-split convention guards against",
                    group.module, group.count, total, SYSTEMS_PER_MODULE_WARN_THRESHOLD
                ),
            )
            .with_path(format!(
                "{}:{}",
                group.first_at.file(),
                group.first_at.line()
            ))
        })
        .collect()
}

/// What one dispatched command left applied: the caller's answer, and what recording writes.
struct Applied {
    result: Result<Dispatched, Diagnostic>,
    /// The command as it mutated the world: itself on success, a handler-noted shortened form when
    /// it failed part-way, `None` when the failure left the world untouched.
    mutated: Option<ludion_protocol::Command>,
}

/// One open subscription: its channel and the next sequence number to stamp on an event delivered to
/// it. Frame subscriptions ignore `next_seq` (frames are keyed by frame index, not a per-subscriber
/// sequence — architecture §6).
///
/// The delivery `mode` is deliberately *not* stored here: event delivery is uniformly lossless
/// regardless of mode (events ride their own ordered lane — audit finding 1.3), and the only
/// mode-dependent behavior, the `frames` `max_fps` throttle, is applied transport-side from the
/// subscribe request's own params (`ludion-server`'s `delivery::register`). Nothing engine-side reads
/// the mode, so it is not kept (audit §5 "stored, never read").
struct Subscription {
    channel: Channel,
    next_seq: Seq,
}

/// The subscription table: allocates and frees subscription ids, remembers each one's channel and
/// delivery mode, and owns the per-subscription event sequence counters the live loop stamps.
pub(crate) struct Subscriptions {
    next: u64,
    open: BTreeMap<u64, Subscription>,
    /// Monotonically counts how many `frames` subscriptions have ever opened. The live loop watches
    /// it to notice a *new* frame subscriber and retry a previously-failed GPU adapter.
    frames_opened: u64,
}

impl Subscriptions {
    fn new() -> Self {
        Subscriptions {
            next: 1,
            open: BTreeMap::new(),
            frames_opened: 0,
        }
    }

    /// Opens a subscription on `channel`, returning its fresh id.
    pub(crate) fn open(&mut self, channel: Channel) -> SubId {
        let id = self.next;
        self.next += 1;
        if channel == Channel::Frames {
            self.frames_opened += 1;
        }
        self.open.insert(
            id,
            Subscription {
                channel,
                next_seq: Seq::new(0),
            },
        );
        SubId::new(id)
    }

    /// Closes subscription `sub`.
    ///
    /// # Errors
    /// `L3007` if `sub` is not currently open.
    pub(crate) fn close(&mut self, sub: SubId) -> Result<(), Diagnostic> {
        if self.open.remove(&sub.as_u64()).is_some() {
            Ok(())
        } else {
            Err(Diagnostic::new(
                ludion_core::codes::code::PROTOCOL_UNKNOWN_SUBSCRIPTION,
                format!("subscription {} is not open", sub.as_u64()),
            )
            .with_help("unsubscribe each id exactly once, using the `sub` from stream.subscribe"))
        }
    }

    /// Whether any subscription on `channel` is currently open.
    pub(crate) fn has_subscriber(&self, channel: Channel) -> bool {
        self.open.values().any(|s| s.channel == channel)
    }

    /// Whether any `frames` subscription is currently open (the guard for producing a frame at all).
    pub(crate) fn has_frame_subscriber(&self) -> bool {
        self.has_subscriber(Channel::Frames)
    }

    /// The ids of every open `frames` subscription — the fan-out list for one rendered frame.
    pub(crate) fn frame_subscribers(&self) -> Vec<SubId> {
        self.open
            .iter()
            .filter(|(_, s)| s.channel == Channel::Frames)
            .map(|(&id, _)| SubId::new(id))
            .collect()
    }

    /// How many `frames` subscriptions have opened over this table's lifetime (see the field docs).
    pub(crate) fn frames_opened(&self) -> u64 {
        self.frames_opened
    }

    /// For every open subscription on `channel`, assigns and advances its event sequence, returning
    /// one `(sub, seq)` pair per subscriber in ascending id order.
    ///
    /// The sequence advances whether or not the resulting message is actually delivered: a downstream
    /// drop shows up as a gap in the client's `seq` stream — a visible, intentional drop, never a
    /// silent loss (architecture §6). Ids are ascending because the backing map is ordered.
    pub(crate) fn assign_event_seqs(&mut self, channel: Channel) -> Vec<(SubId, Seq)> {
        let mut out = Vec::new();
        for (&id, sub) in self.open.iter_mut() {
            if sub.channel == channel {
                let seq = sub.next_seq;
                sub.next_seq = seq.next();
                out.push((SubId::new(id), seq));
            }
        }
        out
    }
}

/// The exact pieces a snapshot restores into an [`Engine`] — used by [`crate::snapshot`].
pub(crate) struct RestoredWorld {
    pub driver: SimDriver,
    pub seed: u64,
    pub scene: Option<SceneRef>,
    pub scene_name: Option<String>,
    /// The snapshot's embedded player profile (canonical `kind: profile` text), or `None` for a
    /// profile-free game or a snapshot taken before this field existed. `Some` installs off-disk;
    /// `None` falls back to the disk load-or-default (Phase 16 / E5, decision 7).
    pub profile: Option<String>,
    /// The snapshot's `scene_physics` field, decoded — `None` for a bare engine, a scene that never
    /// declared a `physics:` block, or a snapshot taken before this field existed.
    pub scene_physics: Option<ScenePhysics>,
    /// The snapshot's data-doc reload log (one entry per `(kind, key)`: latest canonical text and reload count), empty for
    /// a snapshot with no live reloads or one predating the field. Adopted into the session and
    /// re-applied off-disk on restore, so a post-restore switch or a replay-seek reproduces the reloaded
    /// docs instead of rebuilding from possibly-changed disk (audit finding 2.1).
    pub doc_reloads: crate::docs::DocReloadLog,
    /// The snapshot's scene residue (what each visited scene kept behind, keyed by scene `name:`),
    /// empty for a game that marks nothing or a snapshot predating the field. Adopted into the session
    /// on restore, so a return performed after the restore — or a replay-seek keyframe landing after
    /// one — re-applies exactly what the snapshotted session had captured.
    pub residue: StableMap<String, crate::transition::SceneResidue>,
    /// The snapshot's [`GameClock`] — default (unheld) for a game that never freezes or a snapshot
    /// predating the field. Installed verbatim, so a restore resumes the gameplay time the capture
    /// had, not a clock level with the restored frame counter.
    pub game_clock: GameClock,
    /// The snapshot's held [`InputState`] — idle for a snapshot of an untouched session or one
    /// predating the field. Installed verbatim, so the restored session keeps the keys, cursor and
    /// sticks the capture had.
    pub input: InputState,
    /// The snapshot's injected events addressed to frames it has not run — empty for a snapshot
    /// predating the field. Installed on the restored driver, so an `input.inject` for a later frame
    /// still fires.
    pub pending_input: ludion_sim::InputScript,
    /// Whether the snapshot was taken on the boundary a scene restart landed at: installed onto the
    /// restored `SceneSwitch`, so the next tick sees the same fact the live session did.
    pub arrived_by_restart: bool,
    /// The snapshot's validated render extent, or `None` for a snapshot predating the field, which
    /// leaves the host's extent alone. Applied without the render-scale check: it is sim input.
    pub render_extent: Option<(u32, u32)>,
}

/// A top-level prefab instance's root and the placement its scene entry gave it.
pub(crate) type Placed = (EntityId, InstanceTransform);

/// What [`instantiate_named_world`] builds.
pub(crate) struct Instantiated {
    /// The checked document, before instance expansion — what `Engine.authored` holds.
    pub(crate) source: Scene,
    /// The flat scene that was instantiated; positions and names match `world`.
    pub(crate) scene: Scene,
    pub(crate) world: World,
    pub(crate) placements: Vec<Placed>,
}

/// A world rebuilt off to the side, every fallible step already behind it, waiting for
/// [`Engine::commit_world`] to swap it in.
pub(crate) struct PreparedWorld {
    pub(crate) driver: SimDriver,
    pub(crate) seed: u64,
    pub(crate) scene: Option<SceneRef>,
    pub(crate) scene_name: Option<String>,
    pub(crate) scene_physics: Option<ScenePhysics>,
    pub(crate) docs: PreparedDocs,
    /// The new world's library, seeded with the prefab texts the load read and with its `preload:`
    /// entries readied; `None` on a restore, which installs an empty one.
    pub(crate) prefab_library: Option<crate::prefab::PrefabLibrary>,
    /// Each top-level instance's root and the placement its entry gave it; empty on a restore, which
    /// has no document behind it.
    pub(crate) prefab_placements: Vec<Placed>,
    pub(crate) kind: Rebuild,
}

/// Which rebuild a [`PreparedWorld`] is, with the pieces only that rebuild carries.
pub(crate) enum Rebuild {
    /// `scene.load`: a fresh session. `profile_text` is the build's profile, carried by the boot
    /// load only.
    Load {
        authored: StableSet<String>,
        /// The checked source document; seeds `Engine.authored` so the first save or overrides
        /// read skips the parse.
        source: Box<Scene>,
        profile_text: Option<String>,
    },
    /// `engine.restore`: an exact continuation of a snapshotted session.
    Restore {
        doc_reloads: crate::docs::DocReloadLog,
        residue: StableMap<String, crate::transition::SceneResidue>,
        game_clock: GameClock,
        input: Box<InputState>,
        pending_input: ludion_sim::InputScript,
        profile: Option<String>,
        arrived_by_restart: bool,
        render_extent: Option<(u32, u32)>,
    },
    /// A scene switch: the same session, a new district. A `restart` rebuilds the current scene and
    /// stays in its prefab epoch.
    Switch {
        authored: StableSet<String>,
        profile_text: Option<String>,
        restart: bool,
    },
}

impl Engine {
    /// Installs a world/driver rebuilt by [`crate::snapshot::restore_into`], leaving the registry,
    /// project root, render settings, subscriptions, and recording sink untouched.
    ///
    /// # Errors
    /// Every diagnostic the content-root doc walk collected — a restore is a fresh boot of that
    /// world, so a malformed data-doc fails it exactly as it fails a `scene.load`. The engine is
    /// untouched on failure.
    pub(crate) fn install_restored(
        &mut self,
        mut restored: RestoredWorld,
    ) -> Result<(), Vec<Diagnostic>> {
        let docs = self.prepare_docs_for(restored.driver.world_mut(), &restored.doc_reloads)?;
        self.commit_world(PreparedWorld {
            driver: restored.driver,
            seed: restored.seed,
            scene: restored.scene,
            scene_name: restored.scene_name,
            scene_physics: restored.scene_physics,
            docs,
            prefab_library: None,
            prefab_placements: Vec::new(),
            kind: Rebuild::Restore {
                doc_reloads: restored.doc_reloads,
                residue: restored.residue,
                game_clock: restored.game_clock,
                input: Box::new(restored.input),
                pending_input: restored.pending_input,
                profile: restored.profile,
                arrived_by_restart: restored.arrived_by_restart,
                render_extent: restored.render_extent,
            },
        });
        Ok(())
    }

    /// Swaps a prepared world in and reinstalls everything the engine keeps outside it. Infallible:
    /// every check that can fail ran on the detached world, so a rebuild either lands whole or the
    /// engine is exactly as it was.
    ///
    /// This is the one place a rebuild's reset-or-carry decisions live — each `match` below is one
    /// row of the table, over the three rebuilds. A new row goes here, not at a call site.
    pub(crate) fn commit_world(&mut self, prepared: PreparedWorld) {
        let PreparedWorld {
            mut driver,
            seed,
            scene,
            scene_name,
            scene_physics,
            docs,
            prefab_library,
            prefab_placements,
            mut kind,
        } = prepared;
        // Before `install_render_extent` mirrors the size onto the fresh world.
        if let Rebuild::Restore {
            render_extent: Some(size),
            ..
        } = &kind
        {
            self.render_size = *size;
        }
        // Observations from the departing world accumulate across the reset; the fresh one is re-armed
        // at the end.
        self.drain_query_recording();
        // The latch is session state, not world state, so it is taken before the swap. It crosses a
        // switch only: a load or restore is a fresh session, so a still-broken scene reports again.
        let diagnostics = match &kind {
            Rebuild::Switch { .. } => self.carry_diagnostics(),
            Rebuild::Load { .. } | Rebuild::Restore { .. } => crate::Diagnostics::default(),
        };
        // The game's registered resources cross only on a switch: a `scene.load` is a fresh session and
        // a `restore` reinstalls from a snapshot that never held them. Both worlds are alive here — the
        // departing one is dropped by the swap.
        if matches!(kind, Rebuild::Switch { .. }) {
            for carry in &self.resource_carriers {
                carry(self.driver.world_mut(), driver.world_mut());
            }
        }
        // Held input: a load is a new session (the fresh state `SimDriver` came with), a restore
        // continues the snapshot's held input and its pending injected events, a switch carries the
        // departing world's held input and its pending injected events. After the carriers, so this
        // row is authoritative.
        match &mut kind {
            Rebuild::Load { .. } => {}
            Rebuild::Restore {
                input,
                pending_input,
                ..
            } => {
                driver.world_mut().insert_resource(*std::mem::take(input));
                driver.set_pending_input(std::mem::take(pending_input));
            }
            Rebuild::Switch { .. } => driver.carry_input_from(&mut self.driver),
        }
        self.driver = driver;
        if matches!(kind, Rebuild::Load { .. } | Rebuild::Restore { .. }) {
            self.frame_timeline += 1;
            self.discard_stream_ring();
        }
        // A rebuild can move the frame backwards; the next keyframe is one interval after the
        // rebuilt frame, not after the old boundary.
        let frame = self.driver.frame();
        if let Some(recorder) = self.recorder.as_mut()
            && frame < recorder.last_keyframe
        {
            recorder.last_keyframe = frame;
        }
        self.seed = seed;
        self.scene = scene;
        self.authored = match &mut kind {
            Rebuild::Load { source, .. } => {
                std::sync::OnceLock::from(Some(std::mem::take(&mut **source)))
            }
            Rebuild::Restore { .. } | Rebuild::Switch { .. } => std::sync::OnceLock::new(),
        };
        self.scene_name = scene_name;
        self.scene_physics = scene_physics;
        // The engine-owned resources go onto the new world, which carries none.
        self.install_audio_queue();
        self.install_presentation_rules();
        self.install_scene_switch();
        self.install_projectiles();
        self.install_nav();
        if matches!(
            kind,
            Rebuild::Switch { restart: true, .. }
                | Rebuild::Restore {
                    arrived_by_restart: true,
                    ..
                }
        ) && let Some(switch) = self
            .driver
            .world_mut()
            .resource_mut::<crate::transition::SceneSwitch>()
        {
            switch.mark_arrived_by_restart();
        }
        self.install_shutdown();
        self.install_render_settings();
        self.install_cursor_lock();
        self.install_debug_draw();
        // A restore resumes the gameplay time it captured, not a clock level with the frame counter.
        self.install_game_clock(match &kind {
            Rebuild::Restore { game_clock, .. } => *game_clock,
            Rebuild::Load { .. } | Rebuild::Switch { .. } => GameClock::default(),
        });
        self.install_diagnostics(diagnostics);
        self.install_render_extent();
        self.install_plane();
        self.install_animation_library();
        self.install_skin_rests();
        self.install_path_library();
        self.install_font_library();
        if self.replay_prefab_text.is_some()
            && !matches!(kind, Rebuild::Switch { restart: true, .. })
        {
            self.replay_prefab_epoch = self.replay_prefab_next_epoch;
            self.replay_prefab_next_epoch += 1;
        }
        match prefab_library {
            Some(library) => {
                self.driver.world_mut().insert_resource(library);
            }
            None => self.install_prefab_library(),
        }
        if let Some(library) = self
            .driver
            .world_mut()
            .resource_mut::<crate::prefab::PrefabLibrary>()
        {
            for (root, placement) in prefab_placements {
                library.place(root, placement);
            }
        }
        self.install_collider_meshes();
        // Session, after the audio queue: its blanket loop stop is logged there, and a load or restore
        // clears the logs. A load starts a clean stream a replay regenerates from frame 0; a restore
        // adopts the snapshot's reload log and residue so a later switch or replay-seek reproduces
        // the reloaded docs; a switch is mid-session and carries all of it. A restored world has no
        // document of its own, so its authored set is derived from the restored inline text on demand.
        match &mut kind {
            Rebuild::Load { authored, .. } => {
                self.session.reset();
                self.session.authored_names = Some(std::mem::take(authored));
            }
            Rebuild::Restore {
                doc_reloads,
                residue,
                ..
            } => self
                .session
                .adopt(std::mem::take(doc_reloads), std::mem::take(residue)),
            Rebuild::Switch { authored, .. } => {
                self.session.carry();
                self.session.authored_names = Some(std::mem::take(authored));
            }
        }
        // After the session settles: the mirror copies its residue. On a switch `perform` has already
        // committed the departing scene's capture, so the arriving district reads what it just left.
        self.install_residue_store();
        self.install_run_save();
        self.install_run_recording();
        self.install_replay_playback();
        self.commit_docs(docs);
        // A later load reads the persisted save; the boot load carries the profile the build read.
        // A restore is an exact continuation, so it installs the snapshot's embedded profile off-disk
        // (preserving unsaved changes, reproducing on a machine whose save differs) and falls back to
        // disk only without one. A switch carries the in-memory profile — re-reading disk would revert
        // an unlock earned this run; a profile-free game installs nothing.
        self.boot_profile_pending = false;
        match &kind {
            Rebuild::Load {
                profile_text: Some(text),
                ..
            } => {
                // Re-reading disk would find a save the build backed up and replace its L1121 report
                // with a clean one.
                let boot_report = std::mem::take(&mut self.profile_migration);
                self.install_profile_from_text(text);
                self.profile_migration = boot_report;
            }
            Rebuild::Load {
                profile_text: None, ..
            } => self.install_profile(),
            Rebuild::Restore {
                profile: Some(text),
                ..
            } => self.install_profile_from_text(text),
            Rebuild::Restore { profile: None, .. } => self.install_profile(),
            Rebuild::Switch {
                profile_text: Some(text),
                ..
            } => self.install_profile_from_text(text),
            Rebuild::Switch {
                profile_text: None, ..
            } => {}
        }
        // The decoded assets stay: the first extract keeps what the new world names and re-hashes its
        // files. Only the voxelization belongs to the old world.
        if matches!(kind, Rebuild::Load { .. }) {
            self.asset_cache.reset_voxelization();
        }
        // A rebuilt world is never lit from the old world's voxelization or probe bake: the first
        // frame runs unheld, so both GI caches rebuild and become the new baseline.
        self.gi_rebake_force = true;
        self.probe_world_stale();
        self.present_epoch = self.present_epoch.wrapping_add(1);
        self.projectile_draw.reset_convergence();
        // Prefetched at the first extract, not here: headless rebuilds (replay verification, sim
        // hashing) never draw, and must not pay the decode bill.
        self.prefetch_pending = true;
        self.cook_resolved = false;
        self.revalidate_pending = true;
        self.arm_query_recording();
    }
}

/// Checks and instantiates a scene `document` into a fresh world, projecting each entity's `name:`
/// field into a [`Name`](crate::components::Name) component and every `SkinBone` part's spawn pose
/// into a [`Transform`](crate::components::Transform) — the world-reset front half shared by
/// [`load_scene_document`](Engine::load_scene_document) and [`crate::transition::perform`].
///
/// `ludion-scene` stays ignorant of engine component types (crate-boundary rule), so the `Name`
/// projection (an `InstanceHooks` impl) and the `SkinBone` resolution both live here.
/// `instantiate_into` spawns one entity per scene entity in document order on a fresh world;
/// `ludion_scene::instantiated_ids` maps each document position to its entity (a
/// `presentation: true` entity does not take the next sim slot), which the `SkinBone` pass's
/// document positions rely on too.
///
/// `project_root` roots a `SkinBone`'s skin mesh document exactly as every other asset reference
/// resolves, and the prefab files of the scene's instance entries.
///
/// Prefab instances are expanded first ([`ludion_scene::expand`], reading each prefab through
/// `resolve`); the returned scene is the **flat** one that was instantiated, so a caller's positions
/// and names match the world.
/// Each part of a top-level instance carries a [`PrefabInstance`](crate::PrefabInstance).
/// Diagnostics of the post-expansion passes are located back into the document.
///
/// # Errors
/// Every parse/validation/expansion/instantiation/`SkinBone` diagnostic the scene produced, and an
/// `L1212` (with the `L1101` as related) for each physics-claimed `Collider` mesh file that cannot
/// be opened when the scene has a `physics:` block.
///
/// `placements` is each top-level instance's root with the `transform` its entry places it at;
/// `source` is the checked document before expansion.
pub(crate) fn instantiate_named_world(
    document: &str,
    registry: &Registry,
    project_root: &Path,
    resolve: &mut dyn FnMut(&str) -> Result<String, Diagnostic>,
) -> Result<Instantiated, Vec<Diagnostic>> {
    let scene = ludion_scene::check(document, registry)?;
    let expanded = crate::scene_check::expand_scene_with(registry, &scene, resolve)?;
    let unreadable = crate::mesh_check::missing_collider_meshes(project_root, &expanded.scene);
    if !unreadable.is_empty() {
        return Err(unreadable.into_iter().map(|d| expanded.locate(d)).collect());
    }
    let (world, ids) = instantiate_expanded_world(&expanded, registry, project_root)?;
    let placements = expanded
        .instances
        .iter()
        .filter(|instance| instance.depth == 0)
        .filter_map(|instance| {
            let entry = scene.entities[instance.entry].instance.as_ref()?;
            Some((ids[instance.parts.start], entry.transform.clone()))
        })
        .collect();
    Ok(Instantiated {
        source: scene,
        scene: expanded.scene,
        world,
        placements,
    })
}

/// Instantiates an expanded scene into a fresh world — the post-expansion half of
/// [`instantiate_named_world`], and the scratch build `scene.save` compares a live instance against.
/// Returns the world and the entity id of each flat entity, in order.
///
/// # Errors
/// Every instantiation and `SkinBone` diagnostic, located back into the document.
pub(crate) fn instantiate_expanded_world(
    expanded: &ExpandedScene,
    registry: &Registry,
    project_root: &Path,
) -> Result<(World, Vec<EntityId>), Vec<Diagnostic>> {
    let mut world = World::new();
    let ids = ludion_scene::instantiate_expanded(
        &mut world,
        expanded,
        registry,
        &ludion_scene::InstantiateOptions {
            placement: None,
            name_prefix: None,
            hooks: &crate::instantiate::EngineHooks,
        },
    )
    .map_err(|errors| {
        errors
            .into_iter()
            .map(|d| expanded.locate(d))
            .collect::<Vec<_>>()
    })?;
    for instance in expanded.instances.iter().filter(|i| i.depth == 0) {
        let root = ids[instance.parts.start];
        for (k, part) in instance.paths.iter().enumerate() {
            world
                .insert(
                    ids[instance.parts.start + k],
                    crate::prefab::PrefabInstance {
                        source: instance.source.clone(),
                        part: part.clone(),
                        root,
                    },
                )
                .expect("the entity was just instantiated and is alive");
        }
    }
    let mut errors =
        crate::parent_joint::scene_parent_joint_diagnostics(project_root, &expanded.scene);
    if let Err(pose_errors) = crate::skin_bone::apply_spawn_poses(
        &mut world,
        &expanded.scene,
        ludion_assets::AssetRoot::content(project_root),
    ) {
        errors.extend(pose_errors);
    }
    if !errors.is_empty() {
        return Err(errors.into_iter().map(|d| expanded.locate(d)).collect());
    }
    crate::parent_joint::install(&mut world, project_root);
    Ok((world, ids))
}

/// The `Name`s a scene document authors — the provenance a residue capture reads (see
/// [`SessionState::authored_names`]). Entities the document leaves unnamed contribute nothing: an
/// anonymous record is never matched by name in the first place.
pub(crate) fn authored_names(scene: &Scene) -> StableSet<String> {
    scene
        .entities
        .iter()
        .filter_map(|entity| entity.name.clone())
        .collect()
}

/// Where the ear is: the world position of the lowest-`EntityId` entity carrying a [`Camera`], or
/// `None` when the world has none.
///
/// There is no separate `Listener` component — every scene that renders already says where the
/// viewpoint is, and a second way to say it would be a second thing to keep in sync. A camera with no
/// [`Transform`] listens from the origin facing the default orientation (`+X` right, `+Y` up), exactly
/// as it renders from there. The camera's rotation gives the stereo axes.
fn audio_listener(world: &mut World) -> Option<AudioListener> {
    // A typed join over the `Camera` storage, not a per-entity probe of every live entity — free in
    // a scene with none, and `query_ids` yields ascending `EntityId` so the first row is the lowest.
    let (id, _) = world.query_ids::<(&Camera,)>().next()?;
    Some(match crate::hierarchy::world_transform(world, id) {
        Some(pose) => AudioListener {
            position: pose.trs.translation,
            right: ludion_core::math::quat_rotate(pose.trs.rotation, [1.0, 0.0, 0.0]),
            up: ludion_core::math::quat_rotate(pose.trs.rotation, [0.0, 1.0, 0.0]),
        },
        None => AudioListener {
            position: [0.0; 3],
            right: [1.0, 0.0, 0.0],
            up: [0.0, 1.0, 0.0],
        },
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hierarchy::TransformHierarchy;
    use ludion_ecs::SystemCtx;
    use ludion_protocol::command::EngineRestoreParams;
    use ludion_protocol::{Command, ResponseBody};

    fn step_one(engine: &mut Engine) {
        crate::dispatch(
            engine,
            Command::SimStep(ludion_protocol::command::SimStepParams {
                ticks: 1,
                hash: false,
            }),
        )
        .expect("step");
    }

    /// Arming is only legal on a loaded scene at frame 0 (`L3047`), and refusing leaves the engine
    /// untouched; a scene load resets the frame, so arming after a re-load is legal again.
    #[test]
    fn arming_a_recording_needs_a_loaded_scene_at_frame_zero() {
        const DOC: &str = "format: 1
entities: []
";
        let refused = |engine: &mut Engine| {
            let error = engine.set_recording(Vec::new()).expect_err("arm refused");
            assert_eq!(error.code, code::RECORDING_ARM_REFUSED);
            assert!(error.help.is_some(), "the refusal names the fix");
            assert!(!engine.is_recording());
        };

        let mut engine = Engine::new();
        refused(&mut engine);

        engine.load_inline_scene(DOC).unwrap();
        step_one(&mut engine);
        refused(&mut engine);

        engine.load_inline_scene(DOC).unwrap();
        engine.set_recording(Vec::new()).expect("armed at frame 0");
        assert!(engine.is_recording());
    }

    /// A recording refused for size reports the replay ceiling (`L3017`), not a sink failure.
    #[test]
    fn a_too_large_record_maps_to_the_replay_ceiling_code() {
        let mut sink = Vec::new();
        let header = ludion_protocol::replay::ReplayHeader {
            format_version: ludion_protocol::replay::REPLAY_FORMAT_VERSION,
            engine_version: "0.0.0".into(),
            protocol_version: ludion_protocol::PROTOCOL_VERSION,
            schema_hash: ludion_protocol::schema::schema_hash(),
            scene: ludion_protocol::replay::SceneRef::Inline {
                document: "x".repeat(8 * 1024 * 1024),
            },
            seed: 0,
            profile: None,
            render_extent: None,
            replay_content_hash: None,
            game: None,
        };
        let error = ludion_protocol::replay::ReplayWriter::new(&mut sink, &header)
            .err()
            .expect("an over-ceiling header is refused");
        assert_eq!(
            io_to_diagnostic(error).code,
            ludion_core::codes::code::REPLAY_DECOMPRESSED_TOO_LARGE
        );

        let other = std::io::Error::other("disk full");
        assert_eq!(
            io_to_diagnostic(other).code,
            ludion_core::codes::code::OUTPUT_IO
        );
    }

    /// The voxelizer fixture as a live world: one cube under a `GiVolume`, loaded once. Every
    /// subsequent motion is [`move_cube`] — a world edit, not a reload, because a `scene.load` clears
    /// the asset cache and a cleared cache has nothing to hold.
    const SCENE: &str = "format: 1
entities:
  - components:
      Transform: {}
      Mesh:
        shape: cube
  - components:
      GiVolume:
        cell_size: 0.5
";

    /// Moves the cube to `x`, the way a system would.
    fn move_cube(engine: &mut Engine, x: f32) {
        let cube = engine.world().entities().next().expect("the cube");
        engine
            .world_mut()
            .insert(
                cube,
                Transform {
                    translation: [x, 0.0, 0.0],
                    ..Transform::default()
                },
            )
            .expect("the cube is alive");
    }

    /// An engine on the fixture scene, with one frame already behind it so a baseline exists.
    fn engine_on_the_fixture() -> Engine {
        let mut engine = Engine::new();
        engine.load_inline_scene(SCENE).unwrap();
        assert!(
            engine.world().entities().count() == 2,
            "the fixture is the cube plus the GiVolume carrier",
        );
        cpu_frame(&mut engine);
        engine
    }

    /// Extracts one frame's worth of world (no GPU — the CPU half of a frame) and closes it the way
    /// a rendered frame does, so the lever's per-frame lifecycle is exercised end to end. The bake
    /// half of the staleness combination is `gi_bake.rs`'s own fixture; here the renderer is absent
    /// and contributes `false`.
    fn cpu_frame(engine: &mut Engine) {
        engine
            .extract_render_scene(320, 180)
            .expect("the fixture scene extracts");
        engine.finish_gi_frame();
    }

    /// The lever's default is the historical behaviour, and the flags it carries start clean.
    #[test]
    fn the_default_lever_is_auto_and_nothing_is_stale() {
        let engine = Engine::new();
        assert_eq!(engine.gi_rebake_mode(), GiRebakeMode::Auto);
        assert!(!engine.gi_stale());
        assert!(!engine.gi_hold(), "Auto never holds");
    }

    /// At `Auto` a moving unmarked instance rebuilds every frame — the reported failure, asserted so
    /// the untaken branch stays proven rather than assumed.
    #[test]
    fn a_moving_instance_at_auto_is_never_stale() {
        let mut engine = engine_on_the_fixture();
        assert!(!engine.gi_stale());
        move_cube(&mut engine, 4.0);
        cpu_frame(&mut engine);
        assert!(
            !engine.gi_stale(),
            "Auto rebuilds rather than serving a held grid, so nothing is ever stale",
        );
    }

    /// The lever's whole lifecycle at the engine's own altitude: `Hold` freezes the baseline and
    /// reports staleness per frame, `gi_rebake_now` spends exactly one frame with the hold
    /// suppressed and installs a new baseline, and `Auto` clears the report.
    #[test]
    fn hold_reports_stale_and_a_rebake_installs_the_new_baseline() {
        let mut engine = engine_on_the_fixture();
        engine.set_gi_rebake(GiRebakeMode::Hold);

        move_cube(&mut engine, 4.0);
        cpu_frame(&mut engine);
        assert!(engine.gi_stale(), "the world moved off the held baseline");

        // Not latched: back at the baseline, the same comparison says clean.
        move_cube(&mut engine, 0.0);
        cpu_frame(&mut engine);
        assert!(!engine.gi_stale());

        move_cube(&mut engine, 4.0);
        cpu_frame(&mut engine);
        assert!(engine.gi_stale());

        // One rebake: the frame it covers is unheld, and the world it rebuilt is the new baseline.
        engine.gi_rebake_now();
        assert!(!engine.gi_hold(), "a pending rebake suppresses the hold");
        cpu_frame(&mut engine);
        assert!(
            !engine.gi_stale(),
            "the rebaked frame shows the world itself"
        );
        assert!(
            engine.gi_hold(),
            "and the one-shot is spent — the session is held again",
        );
        assert_eq!(
            engine.gi_rebake_mode(),
            GiRebakeMode::Hold,
            "a rebake is an event, not a mode change",
        );

        cpu_frame(&mut engine);
        assert!(!engine.gi_stale(), "the rebake became the new baseline");

        // Releasing the lever rebuilds and clears the report.
        move_cube(&mut engine, 9.0);
        cpu_frame(&mut engine);
        assert!(engine.gi_stale());
        engine.set_gi_rebake(GiRebakeMode::Auto);
        cpu_frame(&mut engine);
        assert!(!engine.gi_stale());
    }

    /// A rebake requested at `Auto` is a no-op the next frame consumes — nothing is held, so there is
    /// nothing to suppress, and the request must not survive into a later `Hold`.
    #[test]
    fn a_rebake_at_auto_is_consumed_by_the_next_frame() {
        let mut engine = engine_on_the_fixture();
        engine.gi_rebake_now();
        cpu_frame(&mut engine);
        engine.set_gi_rebake(GiRebakeMode::Hold);
        assert!(
            engine.gi_hold(),
            "the stale one-shot must not leak into the hold that follows it",
        );
    }

    /// `scene.load` clears the asset cache but not the host's lever, so a new scene under a hold
    /// builds once (a first build is never held) and holds from there.
    #[test]
    fn a_scene_loaded_under_a_hold_builds_once() {
        let mut engine = Engine::new();
        engine.set_gi_rebake(GiRebakeMode::Hold);
        engine.load_inline_scene(SCENE).unwrap();
        cpu_frame(&mut engine);
        assert!(
            !engine.gi_stale(),
            "a scene loaded under a hold voxelizes itself rather than showing nothing",
        );
        assert_eq!(engine.gi_rebake_mode(), GiRebakeMode::Hold);

        move_cube(&mut engine, 4.0);
        cpu_frame(&mut engine);
        assert!(engine.gi_stale(), "and holds from there");
    }

    /// A rebuild of any kind spends one unheld frame, so the new world is never lit from the old
    /// world's voxelization or bake, and the session holds again after it.
    #[test]
    fn a_rebuild_under_a_hold_spends_one_unheld_frame() {
        let mut engine = engine_on_the_fixture();
        engine.set_gi_rebake(GiRebakeMode::Hold);
        assert!(engine.gi_hold());

        engine.load_inline_scene(SCENE).unwrap();
        assert!(!engine.gi_hold(), "a load forces one rebuild");
        cpu_frame(&mut engine);
        assert!(engine.gi_hold(), "and the hold resumes after it");

        let snapshot = match engine.execute(Command::EngineSnapshot).unwrap().body {
            ResponseBody::Snapshot { snapshot } => snapshot,
            other => panic!("expected a snapshot, got {other:?}"),
        };
        engine
            .execute(Command::EngineRestore(EngineRestoreParams {
                snapshot: serde_json::Value::clone(&snapshot),
            }))
            .unwrap();
        assert!(!engine.gi_hold(), "a restore forces one rebuild");
        cpu_frame(&mut engine);
        assert!(engine.gi_hold());
    }

    /// A district switch is a rebuild too: the arriving world is lit unheld once.
    #[test]
    fn a_switch_under_a_hold_spends_one_unheld_frame() {
        fn switch_on_frame_1(world: &mut World, ctx: &SystemCtx) {
            if ctx.frame.as_u64() == 1
                && let Some(switch) = world.resource_mut::<crate::SceneSwitch>()
            {
                switch.request(
                    "next.yaml".to_string(),
                    "gate".to_string(),
                    Vec::<EntityId>::new(),
                );
            }
        }
        let dir = std::env::temp_dir().join(format!("ludion-gi-switch-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let next = format!(
            "{SCENE}  - name: gate
    components:
      Transform: {{}}
"
        );
        std::fs::write(dir.join("next.yaml"), next).unwrap();

        let mut engine = Engine::builder()
            .system("switch_on_frame_1", switch_on_frame_1)
            .project_root(&dir)
            .build()
            .expect("the test engine builds");
        engine.load_inline_scene(SCENE).unwrap();
        engine.set_gi_rebake(GiRebakeMode::Hold);
        cpu_frame(&mut engine);
        assert!(engine.gi_hold());

        engine.advance_live(); // frame 0
        engine.advance_live(); // frame 1 — the switch is performed at the boundary
        assert!(
            engine.last_transition_error().is_none(),
            "the switch is clean: {:?}",
            engine.last_transition_error()
        );
        assert!(!engine.gi_hold(), "a switch forces one rebuild");
        cpu_frame(&mut engine);
        assert!(engine.gi_hold());
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A 2D collider with no `spatial::rebuild` is `L1262` only in a world with game systems; a bare
    /// host (no systems), and a game that does schedule the rebuild, stay silent.
    #[test]
    fn l1262_needs_game_systems_and_no_rebuild() {
        fn noop(_: &mut World, _: &SystemCtx) {}
        let scene = "format: 1\nentities:\n  - name: wall\n    components:\n      Transform: {}\n      \
                     Collider: {}\n";
        let reported = |builder: crate::EngineBuilder| {
            let mut engine = builder.build().expect("the test engine builds");
            engine.load_inline_scene(scene).unwrap();
            engine.advance_live();
            engine.advance_live();
            crate::diagnostics::is_latched(engine.world(), code::COLLIDER_NO_SPATIAL_INDEX, "world")
        };
        assert!(!reported(Engine::builder()), "a bare host is silent");
        assert!(reported(Engine::builder().system("game", noop)));
        assert!(!reported(Engine::builder().system(
            "spatial_rebuild",
            ludion_spatial::spatial::rebuild::<Transform, TransformHierarchy>
        )));
    }

    /// A save keeps what the source document authored: the drift-gate marker and seed presence, on a
    /// fresh load and after a restore.
    #[test]
    fn a_saved_scene_keeps_its_authored_provenance_and_seed() {
        let generated = "format: 1
generated_from: x.map.yaml
seed: 7
entities: []
";
        let mut engine = Engine::new();
        engine.load_inline_scene(generated).unwrap();
        let text = engine.scene_text().unwrap();
        assert!(text.contains("generated_from: x.map.yaml"), "{text}");
        assert!(text.contains("seed: 7"), "{text}");

        let snapshot = match engine.execute(Command::EngineSnapshot).unwrap().body {
            ResponseBody::Snapshot { snapshot } => snapshot,
            other => panic!("expected a snapshot, got {other:?}"),
        };
        engine.load_inline_scene(SCENE).unwrap();
        engine
            .execute(Command::EngineRestore(EngineRestoreParams {
                snapshot: serde_json::Value::clone(&snapshot),
            }))
            .unwrap();
        assert_eq!(engine.scene_text().unwrap(), text, "restore round-trips it");

        engine.load_inline_scene(SCENE).unwrap();
        let text = engine.scene_text().unwrap();
        assert!(
            !text.contains("seed:"),
            "a seedless document saves seedless: {text}"
        );
        assert!(!text.contains("generated_from"), "{text}");
        assert_eq!(ludion_scene::parse(&text).unwrap().seed, None);
    }

    /// A load seeds the authored document from its checked parse; a restore leaves it to the first use.
    #[test]
    fn a_load_seeds_the_authored_document() {
        let document = "format: 1
seed: 3
entities:
  - name: a
    components:
      Transform: {}
";
        let mut engine = Engine::new();
        engine.load_inline_scene(document).unwrap();
        assert_eq!(
            engine.authored.get(),
            Some(&ludion_scene::parse(document).ok())
        );

        let text = engine.scene_text().unwrap();
        let snapshot = match engine.execute(Command::EngineSnapshot).unwrap().body {
            ResponseBody::Snapshot { snapshot } => snapshot,
            other => panic!("expected a snapshot, got {other:?}"),
        };
        engine
            .execute(Command::EngineRestore(EngineRestoreParams {
                snapshot: serde_json::Value::clone(&snapshot),
            }))
            .unwrap();
        assert!(engine.authored.get().is_none(), "a restore stays lazy");
        assert_eq!(engine.scene_text().unwrap(), text);
    }

    /// The lever is host state: it moves no `WorldHash`, whatever the world does under it. The
    /// snapshot/replay half of the same claim is `tests/dispatch.rs`.
    #[test]
    fn the_lever_moves_no_world_hash() {
        let mut auto = engine_on_the_fixture();
        let mut held = engine_on_the_fixture();
        held.set_gi_rebake(GiRebakeMode::Hold);
        held.gi_rebake_now();

        for x in [1.0, 2.0, 3.0] {
            move_cube(&mut auto, x);
            cpu_frame(&mut auto);
            move_cube(&mut held, x);
            cpu_frame(&mut held);
            assert_eq!(
                auto.world_hash().unwrap(),
                held.world_hash().unwrap(),
                "the lever must be invisible to the simulation",
            );
        }
        assert!(
            held.gi_stale() && !auto.gi_stale(),
            "and yet the two engines really did take different branches",
        );
    }

    /// Beyond [`AUDIO_LOG_CAPACITY`] entries the oldest is dropped, oldest first — a live host's
    /// session (vigil's restart is a scene switch, which carries the log) must not grow it without
    /// bound.
    #[test]
    fn the_audio_log_drops_the_oldest_entries_beyond_its_capacity() {
        let mut engine = Engine::new();
        engine
            .load_inline_scene("format: 1\nentities: []\n")
            .unwrap();
        let extra = 5;
        for i in 0..(AUDIO_LOG_CAPACITY + extra) {
            engine
                .world_mut()
                .resource_mut::<AudioQueue>()
                .unwrap()
                .play(format!("s{i}.wav"), 1.0);
            engine.advance_live();
        }
        assert_eq!(engine.audio_log().len(), AUDIO_LOG_CAPACITY);
        let oldest = engine.audio_log().front().expect("bounded, not empty");
        assert_eq!(
            oldest.sound(),
            Some(format!("s{extra}.wav")).as_deref(),
            "the earliest surviving entry is the (extra+1)-th pushed; the rest were evicted oldest-first"
        );
    }

    /// A non-finite gain is queued at 0, so the loop stays declared, and is reported once per sound.
    #[test]
    fn a_non_finite_audio_gain_plays_at_zero_and_warns_once() {
        let mut engine = Engine::new();
        engine
            .load_inline_scene("format: 1\nentities: []\n")
            .unwrap();
        for _ in 0..2 {
            engine
                .world_mut()
                .resource_mut::<AudioQueue>()
                .unwrap()
                .loop_start("x.wav", f32::NAN);
            engine.advance_live();
        }
        let gains: Vec<_> = engine.audio_log().iter().filter_map(|e| e.gain()).collect();
        assert_eq!(gains, [0.0, 0.0]);
        let diagnostics = engine.world().resource::<crate::Diagnostics>().unwrap();
        let reported: Vec<_> = diagnostics.reported(code::AUDIO_GAIN_NON_FINITE).collect();
        assert_eq!(reported, ["x.wav"]);
    }

    /// A sink that accepts writes until its shared switch is flipped, then fails every one.
    struct BreakableSink(std::sync::Arc<std::sync::atomic::AtomicBool>);

    impl Write for BreakableSink {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            if self.0.load(std::sync::atomic::Ordering::SeqCst) {
                return Err(std::io::Error::other("disk full"));
            }
            Ok(buf.len())
        }

        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    /// The command already ran when its record fails to write, so the answer is the command's own:
    /// recording stops, the reason is logged once, and the world keeps the edit.
    #[test]
    fn a_record_write_failure_stops_recording_and_keeps_the_answer() {
        let broken = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let mut engine = Engine::new();
        engine
            .load_inline_scene("format: 1\nentities: []\n")
            .unwrap();
        engine
            .set_recording(BreakableSink(broken.clone()))
            .expect("the header and first keyframe write");
        assert!(engine.is_recording());
        engine.take_events();

        broken.store(true, std::sync::atomic::Ordering::SeqCst);
        let answer = engine
            .execute(ludion_protocol::Command::EntitySpawn(
                ludion_protocol::command::EntitySpawnParams {
                    name: Some("late".into()),
                    components: Default::default(),
                    presentation: false,
                },
            ))
            .expect("a record-write failure never turns an applied command into an error");

        let ludion_protocol::ResponseBody::Spawned { entity } = answer.body else {
            panic!("entity.spawn answers Spawned");
        };
        assert!(engine.world().is_alive(entity), "the spawn stayed applied");
        assert!(!engine.is_recording(), "recording stopped");
        // The report rides `Diagnostics`, which drains at the next frame boundary.
        engine
            .execute(ludion_protocol::Command::SimStep(
                ludion_protocol::command::SimStepParams {
                    ticks: 1,
                    hash: false,
                },
            ))
            .unwrap();
        let logs: Vec<_> = engine
            .take_events()
            .into_iter()
            .filter_map(|event| match event {
                Event::Log(log) => Some(log),
                _ => None,
            })
            .collect();
        assert_eq!(logs.len(), 1, "one report: {logs:?}");
        assert!(logs[0].message.contains("L3042"), "{logs:?}");
        assert!(logs[0].message.contains("recording stopped"));
        let failure = engine.stop_recording().expect_err("the death is returned");
        assert_eq!(failure.code.as_str(), "L3042");
    }
    /// A project root holding a hand-authored copy of the hinge prefab (clips and meshes beside it) at
    /// `hinge/model.prefab.yaml`: the imported `generated_from` line is dropped so the file is editable.
    fn hinge_root(name: &str) -> PathBuf {
        let root = std::env::temp_dir().join(format!("ludion-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let source =
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples/import-node-clips/expected");
        for file in [
            "model.prefab.yaml",
            "clips/open.anim.yaml",
            "clips/snap.anim.yaml",
            "meshes/base.mesh.yaml",
            "meshes/lid.mesh.yaml",
            "meshes/latch.mesh.yaml",
        ] {
            let to = root.join("hinge").join(file);
            std::fs::create_dir_all(to.parent().unwrap()).unwrap();
            std::fs::copy(source.join(file), to).unwrap();
        }
        let file = root.join("hinge/model.prefab.yaml");
        let hand_authored: String = std::fs::read_to_string(&file)
            .unwrap()
            .lines()
            .filter(|line| !line.starts_with("generated_from"))
            .map(|line| format!("{line}\n"))
            .collect();
        std::fs::write(&file, hand_authored).unwrap();
        root
    }

    /// `apply_prefab` on a hand-authored prefab re-reads the clip rests the engine's cache holds:
    /// the lid then poses exactly as in a fresh extract over the rewritten file.
    #[test]
    fn apply_prefab_refreshes_the_cached_clip_rests() {
        let scene = "format: 1
seed: 1
entities:
  - name: opening
    prefab: hinge/model.prefab.yaml
    overrides:
      base:
        Animation:
          clip: open
";
        let root = hinge_root("apply-rests");
        let mut engine = Engine::with_project_root(&root);
        engine.load_inline_scene(scene).expect("the scene loads");
        engine.extract_render_scene(320, 180).expect("extracts");
        let ids: Vec<_> = engine.world().entities().collect();
        let mut moved = *engine.world().get::<Transform>(ids[1]).unwrap().unwrap();
        moved.rotation[0] += 10.0;
        engine.world_mut().insert(ids[1], moved).unwrap();
        engine.apply_prefab(ids[0], None).expect("applies");

        let after = engine
            .extract_render_scene(320, 180)
            .expect("extracts")
            .meshes;
        let mut fresh = Engine::with_project_root(&root);
        fresh.load_inline_scene(scene).expect("the scene loads");
        let fresh = fresh
            .extract_render_scene(320, 180)
            .expect("extracts")
            .meshes;
        assert_eq!(after[1].model, fresh[1].model);
        assert_eq!(after[2].model, fresh[2].model);
        let _ = std::fs::remove_dir_all(&root);
    }

    const HINGE_SCENE: &str = "format: 1
seed: 1
entities:
  - name: opening
    prefab: hinge/model.prefab.yaml
    overrides:
      base:
        Animation:
          clip: open
";

    const EXTRA_PART: &str = "  - name: extra
    components:
      Parent:
        index: 0
        generation: 0
      Transform: {}
";

    fn spawn_hinge(engine: &mut Engine, name: &str) -> Vec<EntityId> {
        crate::prefab::spawn_prefab(
            engine.world_mut(),
            "hinge/model.prefab.yaml",
            None,
            Some(name),
            false,
        )
        .expect("the hinge spawns")
    }

    #[derive(Clone, Default)]
    struct SharedBuf(std::sync::Arc<std::sync::Mutex<Vec<u8>>>);

    impl Write for SharedBuf {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            self.0.lock().unwrap().extend_from_slice(buf);
            Ok(buf.len())
        }

        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    fn reload_hinge(engine: &mut Engine) -> Result<(), Diagnostic> {
        engine
            .execute(Command::AssetReload(
                ludion_protocol::command::AssetReloadParams {
                    path: Some("hinge/model.prefab.yaml".into()),
                },
            ))
            .map(|_| ())
    }

    /// An animated instance puts its prefab in the clip-rest cache, and `asset.reload` of the file
    /// must still reach the library: a later spawn takes the new text, a bad edit leaves the old one,
    /// and a recording carries the reload.
    #[test]
    fn asset_reload_of_an_animated_prefab_reaches_the_library() {
        let root = hinge_root("reload-animated");
        let file = root.join("hinge/model.prefab.yaml");
        let original = std::fs::read_to_string(&file).unwrap();
        let mut engine = Engine::with_project_root(&root);
        engine
            .load_inline_scene(HINGE_SCENE)
            .expect("the scene loads");
        let buffer = SharedBuf::default();
        engine.set_recording(buffer.clone()).expect("arms");
        engine.extract_render_scene(320, 180).expect("extracts");
        let before = spawn_hinge(&mut engine, "first").len();

        std::fs::write(&file, format!("{original}{EXTRA_PART}")).unwrap();
        reload_hinge(&mut engine).expect("the edit reloads");
        assert_eq!(spawn_hinge(&mut engine, "second").len(), before + 1);

        std::fs::write(&file, format!("{original}  - name: extra\n    bogus: 1\n")).unwrap();
        reload_hinge(&mut engine).expect_err("a bad edit is refused");
        assert_eq!(spawn_hinge(&mut engine, "third").len(), before + 1);

        engine.stop_recording().unwrap();
        let bytes = buffer.0.lock().unwrap().clone();
        let reloads = ludion_protocol::replay::ReplayReader::new(bytes.as_slice())
            .unwrap()
            .records()
            .unwrap()
            .into_iter()
            .filter(|record| {
                matches!(
                    record,
                    ludion_protocol::replay::ReplayRecord::PrefabText { reload: true, .. }
                )
            })
            .count();
        assert_eq!(
            reloads, 1,
            "the applied reload is recorded, the refused one is not"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    const TWO_HINGES: &str = "format: 1
seed: 1
entities:
  - name: a
    prefab: hinge/model.prefab.yaml
    overrides:
      base:
        Animation:
          clip: open
  - name: b
    prefab: hinge/model.prefab.yaml
    overrides:
      base:
        Animation:
          clip: open
";

    fn models(engine: &mut Engine) -> Vec<ludion_core::math::glam::Mat4> {
        engine
            .extract_render_scene(320, 180)
            .expect("extracts")
            .meshes
            .iter()
            .map(|mesh| mesh.model)
            .collect()
    }

    /// An instance poses against the text it was expanded from: applying another instance's edit
    /// (which rewrites the file) leaves it where it was.
    #[test]
    fn an_applied_edit_leaves_another_live_instance_posed_as_before() {
        let root = hinge_root("rests-bound");
        let mut engine = Engine::with_project_root(&root);
        engine
            .load_inline_scene(TWO_HINGES)
            .expect("the scene loads");
        let before = models(&mut engine);
        let ids: Vec<_> = engine.world().entities().collect();
        // The latch's translation is a channel `open` animates, so its rest matters to the pose.
        let mut moved = *engine.world().get::<Transform>(ids[2]).unwrap().unwrap();
        moved.translation[2] += 0.25;
        engine.world_mut().insert(ids[2], moved).unwrap();
        engine.apply_prefab(ids[0], None).expect("applies");

        let after = models(&mut engine);
        assert_eq!(&after[3..], &before[3..], "instance b did not move");
        let _ = std::fs::remove_dir_all(&root);
    }

    /// A hand-edit of a part's rest reloaded through `asset.reload` reaches new spawns only; the
    /// instances already live keep the rest of their text.
    #[test]
    fn a_reloaded_rest_reaches_new_instances_and_not_live_ones() {
        let root = hinge_root("rests-reload");
        let file = root.join("hinge/model.prefab.yaml");
        let mut engine = Engine::with_project_root(&root);
        engine
            .load_inline_scene(TWO_HINGES)
            .expect("the scene loads");
        let before = models(&mut engine);

        let text = std::fs::read_to_string(&file).unwrap();
        // The latch's translation is a channel `open` animates, so its rest matters to an instance
        // whose stored pose differs from it; the lid's translation is a channel `open` leaves to
        // the rest, so a new instance draws it from there.
        let edited = text
            .replacen(
                "          - 0.05\n          - 0.8\n",
                "          - 0.05\n          - 0.9\n",
                1,
            )
            .replacen("          - 0.1\n", "          - 0.3\n", 1);
        assert_ne!(edited, text, "the rests moved");
        std::fs::write(&file, &edited).unwrap();
        reload_hinge(&mut engine).expect("reloads");
        assert_eq!(
            models(&mut engine),
            before,
            "live instances keep their rest"
        );

        let spawned = spawn_hinge(&mut engine, "c");
        let animation: crate::components::Animation =
            serde_json::from_value(serde_json::json!({"clip": "open"})).unwrap();
        engine.world_mut().insert(spawned[0], animation).unwrap();
        let all = models(&mut engine);
        let mut fresh = Engine::with_project_root(&root);
        fresh
            .load_inline_scene(HINGE_SCENE)
            .expect("the scene loads");
        let fresh = models(&mut fresh);
        assert_eq!(&all[6..], &fresh[..], "a new spawn poses from the new rest");
        assert_ne!(all[7], before[1], "the new rest moved the lid");
        let _ = std::fs::remove_dir_all(&root);
    }

    /// A prefab root whose `Mesh` is an assembly plays the assembly's clips: the prefab's same-named
    /// clip does not also pose the other parts.
    #[test]
    fn a_root_drawing_an_assembly_does_not_play_the_prefab_clip() {
        let root = hinge_root("model-root");
        let voxels = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples/gallery/voxels");
        for entry in walk(&voxels) {
            let to = root
                .join("voxels")
                .join(entry.strip_prefix(&voxels).unwrap());
            std::fs::create_dir_all(to.parent().unwrap()).unwrap();
            std::fs::copy(&entry, to).unwrap();
        }
        let file = root.join("hinge/model.prefab.yaml");
        let text = std::fs::read_to_string(&file).unwrap();
        let edited = text.replace(
            "asset: meshes/base.mesh.yaml",
            "assembly: ../voxels/figure.assembly.yaml",
        );
        assert_ne!(edited, text);
        std::fs::write(&file, edited).unwrap();
        fn walk(dir: &Path) -> Vec<PathBuf> {
            let mut files = Vec::new();
            for entry in std::fs::read_dir(dir).unwrap() {
                let path = entry.unwrap().path();
                if path.is_dir() {
                    files.extend(walk(&path));
                } else {
                    files.push(path);
                }
            }
            files
        }
        let scene = |animation: &str| {
            format!(
                "format: 1
seed: 1
entities:
  - name: opening
    prefab: hinge/model.prefab.yaml
{animation}"
            )
        };
        let draw = |scene: String| {
            let mut engine = Engine::with_project_root(&root);
            engine.load_inline_scene(&scene).expect("the scene loads");
            // Half a second in, the `open` clip has moved the lid well off its rest.
            engine
                .execute(Command::SimStep(ludion_protocol::command::SimStepParams {
                    ticks: 30,
                    hash: false,
                }))
                .unwrap();
            models(&mut engine)
        };
        let still = draw(scene(""));
        let playing = draw(scene(
            "    overrides:\n      base:\n        Animation:\n          clip: open\n",
        ));
        assert_eq!(still.len(), playing.len());
        let parts = still.len() - 2;
        assert_eq!(
            &playing[parts..],
            &still[parts..],
            "the lid and latch stay at rest"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    /// Runs the wire `scene.check` over `scene.yaml` in `root`, returning `(valid, diagnostics)`.
    fn wire_check(root: &Path) -> (bool, Vec<Diagnostic>) {
        std::fs::write(
            root.join("scene.yaml"),
            "format: 1\nname: s\nentities:\n  - name: plain\n  - name: hinge-1\n    prefab: hinge/model.prefab.yaml\n",
        )
        .unwrap();
        let mut engine = Engine::with_project_root(root);
        let ResponseBody::SceneChecked { valid, diagnostics } = engine
            .execute(Command::SceneCheck(
                ludion_protocol::command::SceneCheckParams {
                    path: "scene.yaml".into(),
                },
            ))
            .expect("checks")
            .body
        else {
            panic!("scene.check answers SceneChecked");
        };
        (valid, diagnostics)
    }

    /// A clip track naming no part of a placed prefab is `L1314` at the placing entry, once per
    /// prefab; the unbroken hinge draws none.
    #[test]
    fn scene_check_reports_a_clip_track_naming_no_part_of_a_placed_prefab() {
        let root = hinge_root("scene-check-clips");
        let (valid, diagnostics) = wire_check(&root);
        assert!(valid, "{diagnostics:?}");
        assert!(diagnostics.is_empty(), "{diagnostics:?}");

        let clip = root.join("hinge/clips/snap.anim.yaml");
        let text = std::fs::read_to_string(&clip).unwrap();
        assert!(text.contains("\n  latch:\n"));
        std::fs::write(&clip, text.replace("\n  latch:\n", "\n  latchh:\n")).unwrap();
        let (valid, diagnostics) = wire_check(&root);
        assert!(!valid);
        assert_eq!(diagnostics.len(), 1, "{diagnostics:?}");
        let found = &diagnostics[0];
        assert_eq!(found.code.as_str(), "L1314");
        assert_eq!(found.path.as_deref(), Some("/entities/1"));
        assert!(
            found.message.starts_with("in `hinge/model.prefab.yaml`: "),
            "{}",
            found.message
        );
        assert_eq!(found.help.as_deref(), Some("nearest part is `latch`"));
        let _ = std::fs::remove_dir_all(&root);
    }

    /// The ear stays on the rig: a presentation `CameraOffset` never moves the audio listener.
    #[test]
    fn a_camera_offset_leaves_the_audio_listener_on_the_rig() {
        let mut world = World::new();
        let camera = world.spawn();
        world
            .insert(
                camera,
                crate::components::Transform {
                    translation: [1.0, 2.0, 3.0],
                    rotation: [0.0, 30.0, 0.0],
                    ..Default::default()
                },
            )
            .unwrap();
        world
            .insert(camera, crate::components::Camera::default())
            .unwrap();
        let rig = audio_listener(&mut world).expect("a camera listens");
        let intro = world.spawn_presentation();
        world
            .insert(
                intro,
                crate::components::CameraOffset {
                    translation: [5.0, 5.0, 5.0],
                    rotation_deg: [10.0, 20.0, 30.0],
                    fov_delta: -10.0,
                },
            )
            .unwrap();
        let with = audio_listener(&mut world).expect("still listens");
        assert_eq!(
            (with.position, with.right, with.up),
            (rig.position, rig.right, rig.up)
        );
    }

    fn memo_root(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("ludion-memo-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn sprite_png() -> Vec<u8> {
        ludion_assets::encode_png(2, 2, &[255; 16]).unwrap()
    }

    const MEMO_SPRITE_SCENE: &str =
        "format: 1\nentities:\n  - components:\n      Sprite:\n        texture: a.png\n";

    fn memo_engine(dir: &Path, scene: &str) -> Engine {
        let mut engine = Engine::builder().project_root(dir).build().unwrap();
        engine.load_inline_scene(scene).unwrap();
        engine
    }

    fn live_blend(engine: &mut Engine) -> Result<RenderScene, Vec<Diagnostic>> {
        engine.extract_render_scene_blend(64, 64, false, None)
    }

    fn reload(engine: &mut Engine, path: &str) {
        engine
            .execute(ludion_protocol::Command::AssetReload(
                ludion_protocol::command::AssetReloadParams {
                    path: Some(path.to_string()),
                },
            ))
            .unwrap();
    }

    #[test]
    fn a_live_frame_remembers_a_failure_until_an_asset_reload() {
        let dir = memo_root("live");
        let mut engine = memo_engine(&dir, MEMO_SPRITE_SCENE);
        assert!(live_blend(&mut engine).is_err());
        std::fs::write(dir.join("a.png"), sprite_png()).unwrap();
        assert!(
            live_blend(&mut engine).is_err(),
            "the failure is remembered"
        );
        reload(&mut engine, "elsewhere/unrelated.png");
        assert!(live_blend(&mut engine).is_ok());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_one_shot_extract_always_retries_a_failure() {
        let dir = memo_root("one-shot");
        let mut engine = memo_engine(&dir, MEMO_SPRITE_SCENE);
        assert!(live_blend(&mut engine).is_err());
        std::fs::write(dir.join("a.png"), sprite_png()).unwrap();
        assert!(engine.extract_render_scene(64, 64).is_ok());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn an_asset_reload_of_any_path_retries_a_failed_assembly() {
        let dir = memo_root("assembly");
        let scene = "format: 1\nentities:\n  - components:\n      Mesh:\n        shape:\n          assembly: models/m.assembly.yaml\n";
        std::fs::create_dir_all(dir.join("models/parts")).unwrap();
        std::fs::write(
            dir.join("models/m.assembly.yaml"),
            "format: 1\nkind: assembly\nname: m\nsize:\n  - 2\n  - 1\n  - 1\npalette:\n  A:\n    color:\n      - 1.0\n      - 0.0\n      - 0.0\n      - 1.0\nparts:\n  - name: body\n    file: parts/m.part.yaml\n    place:\n      - 0\n      - 0\n      - 0\n",
        )
        .unwrap();
        let mut engine = memo_engine(&dir, scene);
        assert!(live_blend(&mut engine).is_err());
        std::fs::write(
            dir.join("models/parts/m.part.yaml"),
            "format: 1\nkind: part\nsize:\n  - 2\n  - 1\n  - 1\nslices:\n  -\n    - \"AA\"\n",
        )
        .unwrap();
        assert!(live_blend(&mut engine).is_err(), "no edge names the part");
        reload(&mut engine, "models/parts/m.part.yaml");
        assert!(live_blend(&mut engine).is_ok());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn scene_save_evicts_what_the_world_no_longer_names() {
        let dir = memo_root("save");
        std::fs::write(dir.join("a.png"), sprite_png()).unwrap();
        std::fs::write(
            dir.join("b.png"),
            ludion_assets::encode_png(2, 2, &[7; 16]).unwrap(),
        )
        .unwrap();
        let mut engine = memo_engine(&dir, MEMO_SPRITE_SCENE);
        engine.extract_render_scene(64, 64).unwrap();
        let sprite = engine.world().entities().next().unwrap();
        engine
            .world_mut()
            .insert(
                sprite,
                crate::components::Sprite {
                    texture: Some("b.png".to_string()),
                    ..Default::default()
                },
            )
            .unwrap();
        engine.extract_render_scene(64, 64).unwrap();
        assert!(engine.asset_cache().contains("a.png"), "kept until a save");

        engine
            .execute(ludion_protocol::Command::SceneSave(
                ludion_protocol::command::SceneSaveParams {
                    path: "saved.yaml".to_string(),
                    force: false,
                },
            ))
            .unwrap();
        engine.extract_render_scene(64, 64).unwrap();
        assert!(!engine.asset_cache().contains("a.png"));
        assert!(engine.asset_cache().contains("b.png"));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
