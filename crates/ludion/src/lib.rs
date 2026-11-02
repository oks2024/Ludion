#![deny(unsafe_code)]
#![warn(missing_docs)]

//! `ludion` — the facade a game depends on. One crate name, one rustdoc root, one `main`.
//!
//! A Ludion game is a **binary** that depends on this crate (`use ludion::prelude::*`). It brings
//! two things of its own — components and systems — and gets everything else from the engine core:
//! scene load and validation through the component registry, the deterministic fixed-timestep sim,
//! `WorldHash`, recording and replay, the wire protocol (so a viewer or agent can attach), and the
//! golden-test harness (as a dev-dependency, `ludion-testkit`).
//!
//! # Building a game
//!
//! 1. **Define components** — plain structs deriving `serde::{Serialize, Deserialize}` and
//!    `schemars::JsonSchema`. Those derives are *your* dependencies: add `serde` and `schemars` to
//!    your `Cargo.toml` (the facade does not re-export the derive macros).
//! 2. **Write systems** — plain `fn(&mut World, &SystemCtx)`. Time reaches a system only through
//!    `SystemCtx` (`ctx.dt`, `ctx.frame`); never read a wall clock (it would break determinism).
//! 3. **Describe the engine** — [`Engine::builder`](crate::prelude::Engine::builder), registering components on top of the built-in
//!    visuals and adding systems in order.
//! 4. **Host it** — `fn main() -> ExitCode { ludion::run(builder) }`. A packaged host, which has no
//!    argv and no working directory to trust, calls
//!    `ludion::launch(builder, LaunchRoots { content, save })` instead; a native shell that owns
//!    the screen drives the [`embed`] core directly, through its C ABI [`ffi`].
//!
//! ```no_run
//! use ludion::prelude::*;
//! use std::process::ExitCode;
//! use serde::{Deserialize, Serialize};
//! use schemars::JsonSchema;
//!
//! /// A game component. It rides on the same entities as the built-in `Transform`/`Sprite`.
//! #[derive(Serialize, Deserialize, JsonSchema, Clone, Copy)]
//! struct Paddle {
//!     x: f32,
//! }
//!
//! /// A system: drive the paddle right while the `D` key is held, and keep its sprite in step.
//! fn move_paddle(world: &mut World, ctx: &SystemCtx) {
//!     if !world.resource::<InputState>().unwrap().is_held(Key::D) {
//!         return;
//!     }
//!     for (paddle, transform) in world.query::<(&mut Paddle, &mut Transform)>() {
//!         paddle.x += 6.0 * ctx.dt;
//!         transform.translation[0] = paddle.x;
//!     }
//! }
//!
//! fn game() -> EngineBuilder {
//!     Engine::builder()
//!         .components(|reg| reg.register::<Paddle>("Paddle"))
//!         .system("move_paddle", move_paddle)
//!         .default_scene("scenes/paddle.yaml")
//! }
//!
//! fn main() -> ExitCode {
//!     ludion::run(game())
//! }
//! ```
//!
//! Run it: `cargo run -- --scene scenes/paddle.yaml`. It prints one machine-parseable
//! `listening on ws://…` line, then hosts the scene until Ctrl-C. See [`run`] for the argument set.
//!
//! # Testing a game (dev-dependency `ludion-testkit`)
//!
//! - **Sim assertions** — drive the engine (or a raw `ludion_sim::SimDriver`) and assert on component
//!   state; `ludion_testkit::{assert_reaches, assert_hash_sequences_match}` wrap the two common
//!   loops. `WorldHash` determinism is registry-driven, so a custom component is covered for free.
//! - **Golden images** — `ludion_testkit::GoldenTest::new(scene, golden).with_registry(my_registry)`
//!   validates a scene against the game's registry (built-ins plus its components), so a scene that
//!   names a game component renders instead of failing `L1013`.
//! - **Replay** — record a session (`--record`) and reproduce its `WorldHash` trace with
//!   [`ludion_engine::replay_hashes_with`] (or [`ludion_engine::replay_with`] for the driven
//!   engine), handing it the *same* builder configuration. (`ludion replay` on the CLI is
//!   built-in-only; it cannot re-instantiate a scene that names game components — replay a game
//!   recording through your own harness.)
//!
//! # Cargo features
//!
//! Both are on by default; a build for a device with no desktop turns them off
//! (`default-features = false`) and hosts through [`launch`] alone.
//!
//! - **`wire`** — the WebSocket listener a viewer or agent attaches to: [`run`], [`host_scene`],
//!   [`HostOptions`], the argv modes (`--profile`, `--trace`, `--check-docs`, `--replay-frames`,
//!   `--replay-verify`), and `ludion-server` (tokio) in the tree. Off, the embedding core hosts the
//!   same engine with no listener.
//! - **`window`** — the game's own window, with the engine presenting its rendered frame into it
//!   in process: winit, wgpu and `ludion-window`'s device-event builders in the tree. `run --window`
//!   opens it beside the listener, and [`launch`] opens it as the player window. Without `wire` it
//!   is the desktop player shape: no listener, no argv shell. Off, `--window` is refused with `L0018`.
//!
//! The asset watcher (`notify`) rides neither: it is `ludion-assets`' opt-in `watch` feature, enabled
//! only by `ludion-server` and `ludion-cli`, so the feature-less tree carries none of the four.

pub mod embed;
// The one module in the workspace that holds `unsafe`: the C ABI a native host links against.
#[allow(unsafe_code)]
pub mod ffi;
mod host;
pub mod math;
pub mod prelude;
#[cfg(feature = "window")]
mod present;
pub mod touch;
#[cfg(feature = "wire")]
mod usage;

pub use host::ctrl_c_fan_out;
#[cfg(feature = "wire")]
pub use host::{HostOptions, host_scene};

#[cfg(feature = "wire")]
use std::collections::BTreeMap;
#[cfg(feature = "wire")]
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

#[cfg(feature = "wire")]
use clap::Parser;
use ludion_core::codes::code;
use ludion_core::diagnostics::Diagnostic;
use ludion_engine::{EngineBuilder, ProbeSource, TextureSource};

/// The `--aa` value parser: the names of [`ludion_engine::AntiAliasing::ALL`], clap refusing the rest.
#[cfg(feature = "wire")]
fn aa_value_parser() -> impl clap::builder::TypedValueParser<Value = ludion_engine::AntiAliasing> {
    use clap::builder::TypedValueParser;
    clap::builder::PossibleValuesParser::new(
        ludion_engine::AntiAliasing::ALL.map(ludion_engine::AntiAliasing::name),
    )
    .map(|name| {
        ludion_engine::AntiAliasing::from_name(&name).expect("clap admits only a listed name")
    })
}

/// `ludion::run`'s fixed argument set, mirroring `ludion serve` so a game's own binary and the CLI
/// host a scene the same way.
#[cfg(feature = "wire")]
#[derive(Parser, Clone, Debug)]
#[command(
    about = "Host this game's scene live over the Ludion wire protocol",
    long_about = None,
    group(clap::ArgGroup::new("replay_window_mode").args(["replay_frames", "profile_replay"])),
    group(clap::ArgGroup::new("debug_draw_mode").args(["render_out", "replay_frames"])),
    group(clap::ArgGroup::new("profile_mode").args(["profile", "profile_replay"]))
)]
pub(crate) struct RunArgs {
    /// Path to the scene file (strict-subset YAML). Falls back to the builder's `default_scene` when
    /// omitted; an error if neither is set.
    #[arg(long)]
    scene: Option<PathBuf>,
    /// The `host:port` to bind the WebSocket host on.
    #[arg(long, default_value = "127.0.0.1:9411")]
    addr: String,
    /// Record the session to this replay file, armed before the first client can connect.
    #[arg(long)]
    record: Option<PathBuf>,
    /// Start the engine paused instead of running, for a deterministic attach point.
    #[arg(long)]
    paused: bool,
    /// Boot from this snapshot file (an `engine.snapshot` document) instead of loading the scene: the
    /// engine is rooted where `--scene` roots it, `engine.restore` runs with the file before the
    /// listener binds, and one line — `restored frame=<N> report=<JSON>`, or `restore failed: <JSON>`
    /// — precedes `listening on`. A failed restore falls back to loading the scene. `--paused` pauses
    /// after the restore.
    #[arg(
        long,
        value_name = "FILE",
        conflicts_with_all = ["record", "check_scene", "render_out", "profile", "trace", "check_docs", "replay_frames", "replay_verify", "profile_replay"]
    )]
    restore: Option<PathBuf>,
    /// Open the game's own window, presenting each rendered frame into it in process while the
    /// server runs; closing it (or Ctrl-C) stops the host the same as headless. Needs the crate's
    /// default `window` feature; a build without it refuses the flag (`L0018`).
    #[arg(long)]
    window: bool,
    /// Open the window borderless fullscreen (Alt+Enter or F11 toggles it; Ctrl+Cmd+F on macOS).
    /// `LUDION_FULLSCREEN=1` or `0` sets the default; this flag beats it.
    #[arg(long, requires = "window")]
    fullscreen: bool,
    /// Instead of hosting, profile this game's scene: drive a warmup then N measured frames with the
    /// game's own systems running, print the `ProfileReport` JSON to stdout, and exit. Run in
    /// `--release` for numbers comparable to a budget.
    #[arg(long, value_name = "FRAMES")]
    profile: Option<u64>,
    /// Instead of hosting, trace this game's scene: drive N ticks with the game's own systems
    /// running (so a `CharacterMotor` moves) and print a per-tick JSON Lines trace to stdout, then
    /// exit. Channel `motor` is one line per motor entity per tick with its feet and `MoveReport`;
    /// `sensors` is one line per sensor enter/exit. Two runs print identical bytes; compare two
    /// runs, never a committed trace. `ludion docs verify-loop` has the record shapes.
    #[arg(long, value_name = "FRAMES", conflicts_with_all = ["window", "profile"])]
    trace: Option<u64>,
    /// With `--trace`, a JSON input script (the `ludion sim-hash --script` format) to drive the run.
    #[arg(long, value_name = "FILE", requires = "trace")]
    script: Option<PathBuf>,
    /// With `--trace`, the comma-separated channels to print: `motor`, `sensors`. Default: both.
    #[arg(long, value_delimiter = ',', requires = "trace")]
    channels: Vec<ludion_engine::TraceChannel>,
    /// With `--profile` or `--profile-replay`, also check the report against a TOML budget file (an
    /// optional `frame_total_ms` and `frame_total_max_ms`, a `[systems]` and a `[systems_max]` table
    /// of per-system median and worst-frame ceilings, a `[passes]` table, and `load_total_ms`, all in
    /// milliseconds). Any over-budget line is an `L4005` and the process exits non-zero; an invalid
    /// budget file is `L4004`. The `max` keys are skipped when the report carries no tail;
    /// `load_total_ms` is `L4025` when it carries no load section (`--profile-replay`).
    #[arg(long, value_name = "BUDGET", requires = "profile_mode")]
    profile_check: Option<PathBuf>,
    /// Instead of hosting, validate this game's registered data-doc kinds (`EngineBuilder::doc_kind`)
    /// against the resolved scene's content root and exit — no server, no GPU. CI-able as a plain
    /// `cargo run -p <game> -- --check-docs` (Phase 15 / E4, `Plan/phase-15-data-docs.md` decision 7).
    #[arg(long, conflicts_with_all = ["profile", "trace"])]
    check_docs: bool,
    /// Instead of hosting, validate `--scene` against **this game's** component registry — the
    /// built-ins plus everything the builder registers — and exit: every diagnostic rustc-style on
    /// stderr, then a one-line summary. This is the `check-scene` a real game scene can use; the
    /// CLI's is built-in-only and rejects every game component with `L1013`. No server, no window,
    /// no GPU.
    #[arg(
        long,
        requires = "scene",
        conflicts_with_all = ["addr", "window", "profile", "trace", "check_docs", "replay_frames", "replay_verify", "render_out"]
    )]
    check_scene: bool,
    /// Instead of hosting, render `--scene` through this game's own engine and write the frame to
    /// this PNG, then exit — the image a golden test would bless, without recording or blessing one.
    /// The extent is `--width`/`--height`, else the builder's render size; `--quality` applies; and
    /// `--frames <N>` ticks the game's schedule before extracting. No server, no window.
    #[arg(
        long,
        value_name = "PNG",
        requires = "scene",
        conflicts_with_all = ["addr", "window", "profile", "trace", "check_docs", "replay_frames", "replay_verify"]
    )]
    render_out: Option<PathBuf>,
    /// With `--render-out`, tick this game's own schedule this many frames before extracting, so the
    /// frame shows what the game does — an early wave, a boss phase — instead of the scene at rest.
    /// The CLI's `ludion render --frames` is the built-in-only twin: it links no game code, so its
    /// ticks run only the engine-installed systems (path followers, the scene's `physics:`), never
    /// the game's. Default 0, a static tick-0 frame.
    #[arg(long, value_name = "N", requires = "render_out")]
    frames: Option<u64>,
    /// With `--render-out` or `--replay-frames`, draw these debug-draw categories (`all` or a comma
    /// list, e.g. `path,motor`) over the frame: the lines the game's systems drew on the last tick,
    /// 1 px wide and dimmed behind geometry. `ludion docs debug-draw` has the categories the engine
    /// emits.
    #[arg(long, value_name = "CATEGORIES", requires = "debug_draw_mode")]
    debug_draw: Option<String>,
    /// Enable dev-only shader hot reload: an `asset.reload` naming a renderer shader (`.wgsl`)
    /// recreates its pipeline live, keeping the last good one on a bad edit. `ludion dev` sets this on
    /// the children it launches so a WGSL edit round-trips over the wire instead of forcing a rebuild.
    /// Off by default — the shipping path compiles shaders in.
    #[arg(long)]
    dev_shaders: bool,
    /// Exit gracefully when stdin closes, so this host never outlives the process that spawned it:
    /// `ludion dev` sets this on the children it launches and holds their stdin open for their
    /// life. Headless hosting only.
    #[arg(long, conflicts_with = "window")]
    supervised: bool,
    /// Instead of hosting, drive this game's committed replay (`.lrp`) frame-by-frame and render each
    /// stepped sim frame to a numbered PNG (`frame-00000.png`, …) under `--frames-out`, for ffmpeg to
    /// assemble into a clip. Headless and offline: no window, no audio, no server, and — like a plain
    /// replay — it cannot persist a profile (the replay installs its own profile-at-start). The scene
    /// comes from the replay's header, so `--scene` is not needed. Requires `--frames-out`.
    /// `--debug-draw` draws its categories over every frame.
    #[arg(
        long,
        value_name = "FILE",
        conflicts_with_all = ["profile", "trace", "check_docs", "window", "record"]
    )]
    replay_frames: Option<PathBuf>,
    /// With `--replay-frames`, the directory the numbered PNGs are written into (created if absent).
    #[arg(long, value_name = "DIR", requires = "replay_frames")]
    frames_out: Option<PathBuf>,
    /// With `--replay-frames` or `--profile-replay`, the first sim frame to render or measure
    /// (inclusive). Frame 0 is the initial post-load world. Default 0.
    #[arg(long, value_name = "N", requires = "replay_window_mode")]
    from: Option<u64>,
    /// With `--replay-frames` or `--profile-replay`, the last sim frame to render or measure
    /// (inclusive). Default: the replay's final frame (the whole session).
    #[arg(long, value_name = "N", requires = "replay_window_mode")]
    to: Option<u64>,
    /// The render width in pixels, for the two modes that write an image: `--replay-frames`
    /// (default 1920) and `--render-out` (default: the builder's render size).
    #[arg(long, value_name = "PX")]
    width: Option<u32>,
    /// The render height in pixels, for the two modes that write an image: `--replay-frames`
    /// (default 1080) and `--render-out` (default: the builder's render size).
    #[arg(long, value_name = "PX")]
    height: Option<u32>,
    /// Instead of hosting, re-drive this game's committed replay (`.lrp`) with the current build and
    /// check the live state against every keyframe the recording embedded, then exit. Green means the
    /// replay still describes this code; a divergence names the frame, the entities, and the
    /// components that drifted (`L3024`) and exits non-zero. Headless and offline: no window, no GPU,
    /// no server. The scene comes from the replay's header, so `--scene` is not needed — it only
    /// names the content root, which otherwise defaults to the builder's default scene's directory
    /// (the one hosting uses), never the replay file's.
    #[arg(
        long,
        value_name = "FILE",
        conflicts_with_all = ["profile", "trace", "check_docs", "window", "record", "replay_frames", "profile_replay"]
    )]
    replay_verify: Option<PathBuf>,
    /// Instead of hosting, time this game's committed replay (`.lrp`) tick by tick through the game's
    /// own builder, print the `ProfileReport` JSON to stdout, and exit. Every system, sequential cost
    /// and the frame total carries its worst frame (`max_ms`, `max_frame`) and `p99_ms` beside the
    /// median, and `samples` is the per-frame series. `--from`/`--to` select the measured window; ticks
    /// before it run unmeasured. Sim only: the report has no GPU or cluster section. Headless and
    /// offline, the scene comes from the replay's header like `--replay-verify`. Run in `--release`
    /// for numbers comparable to a budget.
    #[arg(
        long,
        value_name = "FILE",
        conflicts_with_all = ["profile", "trace", "check_docs", "window", "record", "replay_frames", "replay_verify", "check_scene", "render_out"]
    )]
    profile_replay: Option<PathBuf>,
    /// Sim worker threads for deterministic parallelism (results are thread-count invariant).
    /// Overrides `LUDION_THREADS`, which itself defaults to this machine's capped available
    /// parallelism. Applies to every mode — hosting, `--profile`, `--replay-verify`,
    /// `--replay-frames` — since it is set before any of them runs.
    #[arg(
        long,
        value_name = "N",
        value_parser = clap::builder::RangedU64ValueParser::<usize>::new().range(1..)
    )]
    threads: Option<usize>,
    /// Render quality for this machine: a preset name (`low`, `medium`, `high`, `full`) or the path
    /// to a TOML override file. Overrides `LUDION_QUALITY`; without either, everything the scene
    /// authors is rendered. Render-only — the simulation, its `WorldHash`, and every recording are
    /// identical at any quality — so this changes pixels and frame cost, never behaviour. A value
    /// that is neither a preset nor a usable file is `L0016` and refuses to start.
    #[arg(long, value_name = "PRESET|FILE")]
    quality: Option<String>,
    /// Anti-aliasing mode: `off` (the default) or `smaa`, SMAA 1x after composite. The host's choice:
    /// it beats the builder's default and loses to the game's own runtime choice (an options menu).
    /// Render-only, so no `WorldHash` or recording moves. Ignored by `--replay-frames`,
    /// `--replay-verify`, `--profile-replay` and `--bake-probes`, whose artifacts are compared across
    /// machines.
    #[arg(long, value_name = "off|smaa", value_parser = aa_value_parser())]
    aa: Option<ludion_engine::AntiAliasing>,
    /// Mesh-map source: `cooked` loads BC7/ASTC maps `ludion sync` wrote, falling back per map to the
    /// PNG with an L1271 warning; `source` decodes the PNGs. Live hosting defaults to `cooked`,
    /// `--profile` to `source`. Verification modes always render `source`.
    #[arg(
        long,
        value_name = "source|cooked",
        value_parser = ["source", "cooked"],
        conflicts_with_all = ["check_scene", "render_out", "check_docs", "replay_frames", "replay_verify", "profile_replay", "bake_probes"]
    )]
    textures: Option<String>,
    /// Reflection-probe source: `store` (the default) reads the content-addressed probe store
    /// `ludion bake-probes` fills and captures on a miss with an L1279 warning; `rebake` captures
    /// every probe at load and leaves the store alone. Live hosting only: verification modes and
    /// `--profile` always capture.
    #[arg(
        long,
        value_name = "store|rebake",
        value_parser = ["store", "rebake"],
        conflicts_with_all = ["check_scene", "render_out", "check_docs", "replay_frames", "replay_verify", "profile_replay", "profile", "trace", "bake_probes"]
    )]
    probes: Option<String>,
    /// Instead of hosting, capture **this game's** scene's reflection probes into the probe store and
    /// exit, printing the `ludion bake-probes --json` object on stdout: the `bake-probes` a game scene
    /// can use, since the CLI's is built-in-only and rejects every game component with `L1013`. The
    /// scene is `--scene`, else the default scene; `--quality` applies (the store key hashes it), so
    /// bake at the quality the game will run. Needs a GPU; a scene with no probe prints an empty list.
    #[arg(
        long,
        conflicts_with_all = ["addr", "window", "profile", "trace", "profile_replay", "check_docs", "check_scene", "replay_frames", "replay_verify", "render_out", "textures", "probes"]
    )]
    bake_probes: bool,
    /// With `--bake-probes`, recapture only the probe with this `Name`, replacing its stored entry.
    #[arg(long, value_name = "NAME", requires = "bake_probes")]
    bake_probe: Option<String>,
    /// Render extent for this machine, as `WIDTHxHEIGHT` in whole pixels (`960x540`) — the size the
    /// hosted engine streams and screenshots at. Overrides `LUDION_RENDER_SIZE`; without either, the
    /// engine's default extent stands. Every stage of the frame path costs in proportion to the pixel
    /// count, so this is the knob that shrinks readback, encode, socket write, decode and blit at
    /// once, where `--quality` lowers GPU work only. Deliberately **not** `--width`/`--height`, which
    /// belong to `--replay-frames`. Applies to hosting; a value that is not a `WIDTHxHEIGHT` extent
    /// is `L0017` and refuses to start.
    #[arg(long, value_name = "WxH")]
    render_size: Option<String>,
}

/// Hosts a game over the wire protocol: parses the fixed argument set (`--scene`, `--addr`,
/// `--record`, `--paused`, `--window`), loads the scene through the same path `ludion serve` uses
/// (rooted at the scene file's own directory), and serves it until Ctrl-C, then shuts down
/// cleanly — flushing any armed recording.
///
/// Three arguments divert from hosting entirely. `--profile <N>` (optionally with
/// `--profile-check <BUDGET.toml>`) drives the scene headless for a warmup then `N` measured frames
/// **with the game's own systems running**, prints one [`ludion_engine::ProfileReport`] JSON, and
/// exits — the in-process twin of `ludion profile`, sharing its
/// [`profile_engine`](ludion_engine::profile_engine) driver and budget checker. `--check-docs`
/// (mutually exclusive with `--profile`) validates every doc kind the game registered with
/// [`EngineBuilder::doc_kind`] against the resolved scene's content root and exits — CI-able as a
/// plain `cargo run -p <game> -- --check-docs`, with zero new infrastructure: building the engine
/// already performs the walk and validation (`Plan/phase-15-data-docs.md` decision 7).
/// `--replay-frames <FILE>` (with `--frames-out <DIR>`, and an optional `--from`/`--to` window +
/// `--width`/`--height`) drives a committed `.lrp` replay frame-by-frame through the game's own
/// builder and renders each stepped sim frame to a numbered PNG for `ffmpeg` to assemble into a clip
/// — offline and headless, taking its scene from the replay header, over
/// [`ludion_engine::replay_render_frames`]. It resolves no `--scene` and never persists a profile.
/// `--profile-replay <FILE>` (optionally with `--from`/`--to` and `--profile-check`) times a committed
/// `.lrp` tick by tick through the same builder and prints one [`ludion_engine::ProfileReport`]
/// whose entries carry the worst frame and p99 beside the median, over
/// [`ludion_engine::profile_replay`] — sim only, no GPU section.
/// `--trace <FRAMES>` (optionally with `--script <FILE>` and `--channels motor,sensors`) drives the
/// scene with the game's systems running and prints the per-tick JSON Lines of
/// [`ludion_engine::trace`], the numeric trace `ludion docs verify-loop` describes.
/// The replay modes still take their content root from the scene *argument* — `--scene`'s parent,
/// else the builder's default scene's parent — so a replay is verified, rendered and profiled
/// against the same data-docs hosting would load, wherever the `.lrp` file itself is stored.
///
/// Two more divert from hosting, and both answer about the scene the game was pointed at rather than
/// about a session. `--check-scene` parses and validates `--scene` against **this game's** registry
/// — the built-ins plus everything the builder registers — printing every diagnostic rustc-style and
/// a one-line summary, then exiting 0 when clean and non-zero otherwise; nothing is instantiated and
/// no adapter is requested. `--render-out <PNG>` renders the same scene through this game's own
/// engine over [`ludion_engine::render_scene_with_builder`] and writes the frame, at
/// `--width`/`--height` or else the builder's render size, after `--frames <N>` ticks of the
/// **game's** schedule — so the frame can show an early wave or a boss phase, which the CLI's
/// built-in-only `ludion render --frames` can never reach. Both exist because the CLI's
/// `check-scene` and `render` link no game code, so every scene naming a game component fails them
/// with `L1013`: these are the same two verbs against the engine that can see them. Each requires
/// `--scene`, and both conflict with hosting and with every other mode above.
///
/// `--threads <N>` (`N` ≥ 1) resolves the sim worker count through
/// [`ludion_core::par::set_startup_threads`] before any of the modes above run, so it composes with
/// all of them — hosting, `--profile`, `--replay-verify`, `--replay-frames` alike. Without the flag
/// the count comes from `LUDION_THREADS` (the per-machine knob), and without that from the machine's
/// available parallelism capped at [`ludion_core::par::AUTO_THREAD_CAP`] — so a game runs its sim in
/// parallel by default. Results are thread-count invariant by construction (`ludion_core::par`'s
/// contract), so this is a throughput knob, never a hash-moving one. A malformed `LUDION_THREADS` is
/// `L0014` and refuses to start.
///
/// `--quality <PRESET|FILE>` resolves this machine's render quality through
/// [`ludion_engine::resolve_startup_quality`] before any mode runs, so a malformed setting refuses to
/// start whatever the process was about to do (`L0016`). Without the flag the setting comes from
/// `LUDION_QUALITY`, and without that from everything the scene authors. It reaches the hosted engine
/// and the `--profile` engine; `--replay-frames` deliberately renders at full quality regardless,
/// because a clip is an artifact judged against other machines' copies of it, not a frame this box
/// had to afford. Quality is render-only, so no mode's `WorldHash`, recording, or replay moves with it.
///
/// `--render-size <WxH>` resolves this machine's render extent through
/// [`ludion_engine::resolve_startup_render_size`] the same way (`L0017` on a malformed value, from
/// the flag or from `LUDION_RENDER_SIZE`), and applies it to the **hosted** engine: every streamed
/// frame and every dimensionless `render.screenshot` is produced at that extent, so it is the one
/// knob that shrinks readback, encode, socket write, decode and blit together. It is spelled
/// `--render-size` rather than `--width`/`--height` because those two are `--replay-frames`'s. The
/// extent is sim input (`RenderExtent`): a recording carries it in its header and every applied
/// `render.size`, and a game that never reads it hashes identically at every extent — but **a game's mouse
/// input arrives as render-target pixels**, so a session played at one extent is a different input
/// trace from the same session played at another. Each replays bit-identically; they are not
/// comparable with each other. Committed golden-session recordings are made at the default extent:
/// never record one from a resized host.
///
/// This is the whole body of a game's `main`. `builder` carries the game's components and systems
/// (and, optionally, a [`default_scene`](EngineBuilder::default_scene) used when `--scene` is
/// omitted). Because `--window` may open the game's own window, whose winit event loop must run on
/// the process's main thread, `run` itself must be called from `main` and never from a spawned
/// thread.
///
/// `--restore <FILE>` boots from an `engine.snapshot` document instead of loading the scene: the
/// engine is rooted where `--scene` would root it, `engine.restore` runs with the file before the
/// listener binds, and the run state the snapshot carries is installed (`--paused` then pauses). A
/// restore that fails — an unreadable or non-JSON file, a refused snapshot — is reported on stderr and
/// the scene loads into the same engine, so the host still comes up with a fresh world. It conflicts
/// with `--record` and with every mode that does not host. `ludion dev` relaunches its child this way.
///
/// On success it prints one machine-parseable line first — `listening on ws://<addr>`, flushed
/// immediately, so a test or agent can read the bound address when `--addr` asked for an OS-chosen
/// port (`:0`) — then a short human-readable block. With `--restore`, the restore's outcome line
/// (`restored frame=<N> report=<JSON>` or `restore failed: <JSON>`) precedes it.
///
/// Without `--window`, it then blocks until Ctrl-C or a wire `engine.shutdown` command and returns
/// [`ExitCode::SUCCESS`]. With `--window`, it instead opens the game's own window — the engine takes
/// that window's GPU surface and blits every rendered frame into it directly, with no encode, no
/// decode and no client in between — and blocks there; the listener still binds, so agents and the
/// editor attach exactly as they do headless. Closing the window, Ctrl-C, or a wire
/// `engine.shutdown` all end it the same way. Either way, shutdown flushes any armed recording
/// before the process exits (the one graceful teardown). If `--window` was asked for but the window
/// or its GPU surface cannot be created (headless session, no adapter), the server is still shut
/// down cleanly first, then the diagnostic is printed to stderr and a non-zero code returned. A
/// build without the crate's default `window` feature has no window at all and refuses `--window`
/// before binding (`L0018`).
///
/// On any other failure (no scene, a bad scene, an unresolvable `--addr`, a failed bind, a failed
/// `--record` sink) it prints a diagnostic to stderr and returns a non-zero code; nothing is printed
/// on stdout.
#[cfg(feature = "wire")]
pub fn run(builder: EngineBuilder) -> ExitCode {
    let args = RunArgs::parse();
    // The scene stays cwd-relative exactly as given: an empty content root joins to it unchanged.
    let cwd_roots = LaunchRoots {
        content: PathBuf::new(),
        save: PathBuf::new(),
    };
    let (builder, scene) = match run_short_circuit(builder, &args, &cwd_roots) {
        Ok(code) => return code,
        Err(host) => host,
    };

    // A windowless build cannot honour `--window`; refused before anything starts, never hosted
    // headless in its place.
    #[cfg(not(feature = "window"))]
    if args.window {
        eprintln!("{}", host::window_feature_off());
        return ExitCode::FAILURE;
    }

    // The scene stays cwd-relative exactly as given (an empty content root joins to it unchanged),
    // and the profile persists beside it — the project root `load_scene_file` would have used.
    let save = std::path::absolute(&scene)
        .ok()
        .and_then(|absolute| absolute.parent().map(Path::to_path_buf))
        .or_else(|| scene.parent().map(Path::to_path_buf))
        .unwrap_or_else(|| PathBuf::from("."));
    let roots = LaunchRoots {
        content: PathBuf::new(),
        save,
    };
    // The rest of the host lifecycle (engine build, recording, audio, `--paused`, bind, header, watch,
    // `--window`, graceful teardown) is the embedding core's, shared with `ludion serve`.
    // `ludion::run` omits the `schema hash:` header line `serve` prints.
    let options = embed::StartOptions {
        paused: args.paused,
        record: args.record,
        restore: args.restore,
        fullscreen: args.fullscreen,
        listen: Some(args.addr),
        scene: Some(scene),
        threads: args.threads,
        quality: args.quality,
        anti_aliasing: args.aa,
        textures: args.textures.as_deref().and_then(TextureSource::from_name),
        probes: Some(ProbeSource::from_flag(args.probes.as_deref())),
        render_size: args.render_size,
        dev_shaders: args.dev_shaders,
        print_schema_hash: false,
        window_flag: true,
        ..embed::StartOptions::default()
    };
    let source = embed::Source::Game { builder, roots };

    #[cfg(feature = "window")]
    if args.window {
        return present::host_windowed(source, options, present::WindowRole::Dev);
    }
    match embed::Embedded::start_from(source, None, options) {
        Ok(embedded) => {
            let supervisor = if args.supervised {
                host::stdin_closed()
            } else {
                crossbeam_channel::never()
            };
            host::host_until_stopped(embedded, supervisor)
        }
        Err(diagnostic) => {
            eprintln!("{diagnostic}");
            ExitCode::FAILURE
        }
    }
}

#[cfg(feature = "wire")]
impl RunArgs {
    /// The arguments with every path resolved against a host's roots: what is read joins
    /// `roots.content`, what is written joins `roots.save`. An absolute path is unchanged, and an
    /// empty root is the identity, so [`run`] passes its arguments through as they are.
    fn rebased(&self, roots: &LaunchRoots) -> RunArgs {
        let content = |path: &PathBuf| roots.content.join(path);
        let save = |path: &PathBuf| roots.save.join(path);
        RunArgs {
            scene: self.scene.as_ref().map(content),
            replay_verify: self.replay_verify.as_ref().map(content),
            replay_frames: self.replay_frames.as_ref().map(content),
            profile_replay: self.profile_replay.as_ref().map(content),
            profile_check: self.profile_check.as_ref().map(content),
            script: self.script.as_ref().map(content),
            frames_out: self.frames_out.as_ref().map(save),
            render_out: self.render_out.as_ref().map(save),
            record: self.record.as_ref().map(save),
            restore: self.restore.as_ref().map(content),
            ..self.clone()
        }
    }
}

/// Resolves this machine's settings and runs whichever of `run`'s modes that are not hosting the
/// arguments name (`--replay-frames`, `--replay-verify`, `--profile-replay`, `--check-scene`,
/// `--render-out`, `--profile`, `--trace`, `--check-docs`), against `roots`. `Ok` is the process's answer — a mode ran, or a
/// malformed setting refused to start; `Err` hands the builder and the resolved scene back to host.
/// Shared by [`run`], which passes empty roots, and the embedded boot thread, which passes its own.
// The `Err` is the builder handed back to host, once per process; boxing it would only add a move.
#[allow(clippy::result_large_err)]
#[cfg(feature = "wire")]
pub(crate) fn run_short_circuit(
    builder: EngineBuilder,
    args: &RunArgs,
    roots: &LaunchRoots,
) -> Result<ExitCode, (EngineBuilder, PathBuf)> {
    let args = &args.rebased(roots);

    // The worker count is resolved before anything else runs, so it covers every mode below
    // uniformly: `--threads`, else `LUDION_THREADS`, else the machine's capped available parallelism.
    if let Err(invalid) = ludion_core::par::set_startup_threads(args.threads) {
        eprintln!("{}", invalid.diagnostic());
        return Ok(ExitCode::FAILURE);
    }

    // Resolved here too, before any mode runs, so a malformed setting refuses to start whatever the
    // process was going to do — including the modes that render nothing.
    let quality = match ludion_engine::resolve_startup_quality(args.quality.as_deref(), None) {
        Ok(quality) => quality,
        Err(invalid) => {
            eprintln!("{invalid}");
            return Ok(ExitCode::FAILURE);
        }
    };

    // And the render extent, on the same terms: resolved before any mode runs so a malformed value
    // refuses to start whatever the process was going to do. Only hosting applies it — a replay's
    // frames and a `--profile` run are artifacts compared against other machines' copies, so neither
    // takes its size from this box.
    if let Err(invalid) = ludion_engine::resolve_startup_render_size(args.render_size.as_deref()) {
        eprintln!("{invalid}");
        return Ok(ExitCode::FAILURE);
    }

    // The builder's own default scene, against the content root, when no `--scene` names one.
    let default_scene = builder
        .default_scene_hint()
        .map(|hint| roots.content.join(hint));
    let scene_or_default = args.scene.clone().or(default_scene);

    // `--replay-frames` short-circuits hosting entirely: it renders a committed `.lrp` to a numbered
    // PNG sequence offline and exits. It takes its scene from the replay header, so it runs *before*
    // scene resolution (no `--scene` needed) — see `run_replay_frames`.
    if let Some(replay_path) = &args.replay_frames {
        return Ok(run_replay_frames(
            builder,
            replay_path,
            args,
            scene_or_default.as_deref(),
        ));
    }

    // `--replay-verify` short-circuits the same way, and for the same reason: it takes its scene from
    // the replay header, so it runs before scene resolution.
    if let Some(replay_path) = &args.replay_verify {
        return Ok(run_replay_verify(
            builder,
            replay_path,
            scene_or_default.as_deref(),
        ));
    }

    // `--profile-replay` is the third replay mode and short-circuits for the same reason.
    if let Some(replay_path) = &args.profile_replay {
        return Ok(run_profile_replay(
            builder,
            replay_path,
            args,
            scene_or_default.as_deref(),
        ));
    }

    // Resolve the scene: the explicit `--scene`, else the builder's default-scene hint (read before
    // the builder is consumed), else a clear error.
    let Some(scene) = scene_or_default else {
        eprintln!(
            "{}",
            Diagnostic::new(code::NO_DEFAULT_SCENE, "no scene to host")
                .with_help("pass `--scene <FILE>` or set `EngineBuilder::default_scene`")
        );
        return Ok(ExitCode::FAILURE);
    };

    // `--check-scene` short-circuits hosting: parse and validate the scene against this game's own
    // registry and exit. Nothing is instantiated, nothing is hosted, no adapter is requested.
    if args.check_scene {
        return Ok(run_check_scene(builder, &scene));
    }

    // `--render-out` short-circuits it too: one frame of the scene, validated against the same
    // registry, written as a PNG. The one mode here that needs a GPU without hosting anything.
    if let Some(out) = &args.render_out {
        return Ok(run_render_out(
            builder,
            &scene,
            out,
            args.width,
            args.height,
            args.frames.unwrap_or(0),
            &quality,
            &ludion_engine::DebugCategories::parse(args.debug_draw.as_deref().unwrap_or_default()),
            args.aa,
        ));
    }

    // Profiling mode short-circuits hosting: drive the game's own schedule for a warmup then N
    // measured frames and print the report, instead of binding a server. It reuses the exact driver
    // and budget machinery `ludion profile` runs — the only difference is *this* engine carries the
    // game's registered systems, so they show up in the report where the offline CLI's list is empty.
    if let Some(frames) = args.profile {
        return Ok(run_profile(
            builder,
            &scene,
            frames,
            args.profile_check.as_deref(),
            quality,
            args.aa,
            args.textures
                .as_deref()
                .and_then(TextureSource::from_name)
                .unwrap_or_default(),
        ));
    }

    // `--trace` is `--profile`'s sibling: the game's own systems run, and the per-tick records print.
    if let Some(frames) = args.trace {
        return Ok(run_trace(
            builder,
            &scene,
            frames,
            args.script.as_deref(),
            &args.channels,
        ));
    }

    // `--bake-probes` fills the probe store from this game's engine and exits.
    if args.bake_probes {
        return Ok(run_bake_probes(
            builder,
            &scene,
            args.bake_probe.as_deref(),
            quality,
        ));
    }

    // `--check-docs` short-circuits the same way: validate the game's registered data-doc kinds
    // against the resolved scene's content root and exit — no server, no GPU.
    if args.check_docs {
        return Ok(run_check_docs(builder, &scene));
    }

    Err((builder, scene))
}

/// The two directories a packaged host hands the engine instead of argv and a working directory.
///
/// A shipped build has neither: its content sits read-only inside a bundle, and its saves belong
/// somewhere the bundle is not. Both roots are data here, so the same game binary a developer runs
/// from a checkout through [`run`] runs from a bundle through [`launch`] with nothing to parse.
#[derive(Clone, Debug)]
pub struct LaunchRoots {
    /// The directory the builder's `default_scene` path is relative to — what [`run`] implicitly
    /// takes as its working directory. The project root for assets is still the scene file's own
    /// directory, exactly as with `run`.
    pub content: PathBuf,
    /// The directory the player profile persists under (`<save>/<profile_path>`, default
    /// `<save>/save/profile.yaml`). A packaged build's content is read-only; this is its writable
    /// half.
    pub save: PathBuf,
}

/// The address [`launch`] binds on: the same default [`run`]'s `--addr` carries. `launch` has no
/// address knob — a build that must listen beyond loopback is the trigger for one.
#[cfg(feature = "wire")]
pub(crate) const LAUNCH_ADDR: &str = "127.0.0.1:9411";

/// Hosts a game with no argv: the entry point a packaged build's own `main` calls when there is no
/// command line to parse and no working directory to trust. `roots` names the two directories
/// [`run`] takes from the process instead — where content is read from, and where the player profile
/// is written — and nothing else is configurable: no `--scene`, no `--record`, no `--paused`, no
/// `--window`, no address.
///
/// The scene is the builder's [`default_scene`](EngineBuilder::default_scene) resolved against
/// `roots.content`; a builder without one is an error, since no flag can supply it. The three
/// per-machine settings still arrive from the environment on exactly `run`'s terms — `LUDION_THREADS`,
/// `LUDION_QUALITY`, `LUDION_RENDER_SIZE`, `LUDION_FULLSCREEN` — so a malformed value refuses to
/// start (`L0014`, `L0016`, `L0017`, `L0037`) before anything is built.
///
/// With the `window` feature it is the desktop player: the game's own window, titled with the game's
/// title alone, opening in the builder's fullscreen and cursor mode, its render extent following the
/// window. It must be called from the process's main thread, and closing the window, Ctrl-C or a
/// `Shutdown` request stops it gracefully. On a box with no display it fails `L2007`. Without `window`
/// it is the [embedding core](embed) with no surface: `start`, then wait for Ctrl-C.
///
/// With the default `wire` feature the core also binds the wire protocol at the loopback address `run`
/// defaults to — the listener an agent or a viewer attaches to. Built without `wire` it hosts the same
/// engine with no listener at all: the sim thread runs alone until it is stopped, then shuts down
/// cleanly, flushing any armed recording.
///
/// On any failure (no default scene, a bad scene, a failed bind) it prints a diagnostic to stderr and
/// returns a non-zero code.
pub fn launch(builder: EngineBuilder, roots: LaunchRoots) -> ExitCode {
    let options = embed::StartOptions {
        #[cfg(feature = "wire")]
        listen: Some(LAUNCH_ADDR.to_string()),
        ..embed::StartOptions::default()
    };
    #[cfg(feature = "window")]
    return present::host_windowed(
        embed::Source::Game { builder, roots },
        options,
        present::WindowRole::Player,
    );

    #[cfg(not(feature = "window"))]
    launch_headless(builder, roots, options)
}

#[cfg(not(feature = "window"))]
fn launch_headless(
    builder: EngineBuilder,
    roots: LaunchRoots,
    options: embed::StartOptions,
) -> ExitCode {
    let embedded = match embed::Embedded::start(builder, roots, None, options) {
        Ok(embedded) => embedded,
        Err(diagnostic) => {
            eprintln!("{diagnostic}");
            return ExitCode::FAILURE;
        }
    };

    // With the listener, the boot thread prints its own header once the port is bound.
    #[cfg(not(feature = "wire"))]
    {
        use std::io::Write;
        println!("engine version: {}", ludion_engine::ENGINE_VERSION);
        println!("scene: {}", embedded.scene().display());
        println!("listener: off");
        println!("press Ctrl-C to stop");
        let _ = std::io::stdout().flush();
    }

    host::host_until_stopped(embedded, crossbeam_channel::never())
}

/// Builds the engine a game host runs: loads `scene` through `builder` with player-profile
/// persistence on, reports what the boot load found, and applies this machine's render settings. Both
/// [`run`] and [`launch`] go through it, so the two can never differ on what a hosted engine is. On
/// failure it prints the diagnostic to stderr itself and returns the code the caller returns.
pub(crate) fn prepare_game_engine(
    builder: EngineBuilder,
    scene: &Path,
    restore: Option<&Path>,
    quality: ludion_engine::QualitySetting,
    textures: TextureSource,
    probes: ProbeSource,
    render_size: Option<(u32, u32)>,
) -> Result<ludion_engine::Engine, ExitCode> {
    // Player-profile persistence is on for this live host run — the one place a `profile.save()`
    // reaches disk and an unloadable save is backed up (a replay, `--check-docs`, or `--profile` run
    // leaves it off, so none clobbers or renames the developer's real save). Only a game host does
    // this: `ludion serve` hosts arbitrary scenes with no game systems, so it never persists a
    // profile. `--record` inherits persistence — it takes this same path.
    let engine = match restore {
        None => builder.persist_profile().load_scene_file(scene),
        Some(snapshot) => boot_from_snapshot(builder.persist_profile(), scene, snapshot),
    };
    let mut engine = match engine {
        Ok(engine) => engine,
        Err(diagnostic) => {
            eprintln!("{diagnostic}");
            return Err(ExitCode::FAILURE);
        }
    };

    // The boot load carries the profile the build read; report any migration it made (a drifted or
    // reset save) to stderr before the engine moves onto the sim thread, so a real game persists
    // *and* surfaces save drift.
    report_profile_migration(&engine);
    report_boot_warnings(&engine);

    // A host switch, applied before the engine moves onto the sim thread: an attached client sees
    // this machine's quality through `engine.describe` and can never negotiate it.
    engine.set_quality(quality);
    // Likewise a host switch, pixels only: where imported mesh maps are read from.
    engine.set_textures(textures);
    engine.set_probes(probes);
    // The render extent rides beside it, and unlike quality it moves the bytes a stream delivers
    // rather than the GPU work behind them. Set only when a host named one, so an absent flag leaves
    // the engine's own default.
    if let Some((width, height)) = render_size {
        engine.set_render_size(width, height);
    }
    // Quality's render scale and the extent meet here; a pair no render could honor refuses the
    // start rather than failing on the first frame.
    if let Err(diagnostic) = engine.check_render_extent() {
        eprintln!("{diagnostic}");
        return Err(ExitCode::FAILURE);
    }

    Ok(engine)
}

/// The `--restore` boot: roots the engine where `--scene` would, executes `engine.restore` with the
/// snapshot file instead of loading the scene, and prints the outcome line before anything binds. A
/// restore that fails (unreadable file, not JSON, refused snapshot) is reported and the scene loads
/// into the same engine, so the host still comes up with a fresh world.
fn boot_from_snapshot(
    builder: EngineBuilder,
    scene: &Path,
    snapshot_path: &Path,
) -> Result<ludion_engine::Engine, Diagnostic> {
    let resolved = std::path::absolute(scene).unwrap_or_else(|_| scene.to_path_buf());
    let root = resolved
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_else(|| PathBuf::from("."));
    let mut engine = builder.project_root(root).build()?;

    let outcome = read_snapshot(snapshot_path).and_then(|snapshot| {
        let dispatched = engine.execute(ludion_protocol::Command::EngineRestore(
            ludion_protocol::command::EngineRestoreParams { snapshot },
        ))?;
        match dispatched.body {
            ludion_protocol::ResponseBody::Restored { report } => {
                Ok((engine.frame().as_u64(), report))
            }
            _ => unreachable!("engine.restore answers with Restored"),
        }
    });
    println!("{}", boot_restore_line(&outcome));
    let _ = std::io::Write::flush(&mut std::io::stdout());
    if let Err(diagnostic) = outcome {
        eprintln!("{diagnostic}");
        engine.execute(ludion_protocol::Command::SceneLoad(
            ludion_protocol::command::SceneLoadParams {
                path: resolved
                    .file_name()
                    .map(|name| name.to_string_lossy().into_owned())
                    .unwrap_or_default(),
            },
        ))?;
    }
    Ok(engine)
}

/// Reads a `--restore` snapshot file as the JSON `engine.restore` takes.
fn read_snapshot(path: &Path) -> Result<serde_json::Value, Diagnostic> {
    let text = std::fs::read_to_string(path).map_err(|error| {
        Diagnostic::new(
            code::SCENE_IO,
            format!("could not read snapshot `{}`: {error}", path.display()),
        )
        .with_help("check the path (relative to the current directory) and permissions")
    })?;
    serde_json::from_str(&text).map_err(|error| {
        Diagnostic::new(
            code::PROTOCOL_SNAPSHOT_INVALID,
            format!("snapshot `{}` is not JSON: {error}", path.display()),
        )
    })
}

/// The stdout line a `--restore` boot prints before `listening on`: `restored frame=<N> report=<JSON>`
/// on success, `restore failed: <JSON of the diagnostic>` otherwise. `ludion dev` parses both.
fn boot_restore_line(
    outcome: &Result<(u64, ludion_protocol::restore::RestoreReport), Diagnostic>,
) -> String {
    match outcome {
        Ok((frame, report)) => format!(
            "restored frame={frame} report={}",
            serde_json::to_string(report).unwrap_or_default()
        ),
        Err(diagnostic) => format!(
            "restore failed: {}",
            serde_json::to_string(diagnostic).unwrap_or_default()
        ),
    }
}

/// The content root the replay modes (`--replay-verify`, `--replay-frames`, `--profile-replay`) resolve content under:
/// the one *hosting* would use, not the replay file's own directory.
///
/// Hosting loads a scene file and takes that file's directory as the project root, so a game's
/// data-docs and sibling scenes resolve there. A replay mode never resolves a scene — the header
/// carries it — but it builds the same engine, so it must root the same way or the replayed world is
/// missing every doc the recording had: `--scene`'s parent when one was given, else the builder's
/// default scene's parent, each relative to the current directory (a scene with no directory
/// component roots at the current directory). Only a game with neither falls back to the replay
/// file's own parent directory.
#[cfg(feature = "wire")]
fn replay_project_root(
    scene_arg: Option<&Path>,
    default_scene: Option<&Path>,
    replay_path: &Path,
) -> PathBuf {
    let root = scene_arg
        .or(default_scene)
        .and_then(Path::parent)
        .or_else(|| replay_path.parent())
        .unwrap_or_else(|| Path::new("."));
    // A bare file name's parent is the empty path, which no engine can walk — it names the current
    // directory. Every other root is made absolute lexically, exactly as
    // `EngineBuilder::load_scene_file` does with a hosted scene.
    if root.as_os_str().is_empty() {
        return PathBuf::from(".");
    }
    std::path::absolute(root).unwrap_or_else(|_| root.to_path_buf())
}

/// Reads a recorded session, as an `L1015` diagnostic when the file cannot be read — the same
/// refusal `ludion replay` prints. Warns (`L3052`) when the file ends in a torn record.
#[cfg(feature = "wire")]
fn read_replay(path: &Path) -> Result<Vec<u8>, Diagnostic> {
    let bytes = std::fs::read(path).map_err(|error| {
        Diagnostic::new(
            code::SCENE_IO,
            format!(
                "could not read replay session `{}`: {error}",
                path.display()
            ),
        )
        .with_help("check the path (relative to the current directory) and permissions")
    })?;
    if let Ok(reader) = ludion_protocol::replay::ReplayReader::new(&bytes)
        && let Some(warning) = reader.torn_tail()
    {
        eprintln!("{warning}");
    }
    Ok(bytes)
}

/// Runs `--replay-frames`: drives this game's committed `.lrp` frame-by-frame through the game's own
/// [`EngineBuilder`] and writes each stepped sim frame in the `[--from, --to]` window to a numbered
/// PNG (`frame-<NNNNN>.png`, zero-padded and ffmpeg-glob-friendly) under `--frames-out`, so ffmpeg can
/// assemble a clip. It is the offline twin of a live host: no server, no window, no audio, and — like
/// a plain replay — it never persists a profile (the render runs through
/// [`ludion_engine::replay_render_frames`], which builds the engine without profile persistence and
/// installs the replay's embedded profile-at-start).
///
/// The scene comes from the replay header, so no `--scene` is resolved here; content still roots
/// where hosting roots it — see [`replay_project_root`]. Progress is reported to
/// stderr every 300 rendered frames (≈5 s of 60 Hz output) so a long render is never silent, and a
/// final `rendered N frame(s) → <dir>` summary goes to stdout. A bad `.lrp`, a missing `--frames-out`,
/// an out-of-range window (`L3021`), an unwritable directory, or no GPU adapter (`L2003`) each print a
/// readable diagnostic to stderr and return a non-zero code.
#[cfg(feature = "wire")]
fn run_replay_frames(
    builder: EngineBuilder,
    replay_path: &Path,
    args: &RunArgs,
    scene_arg: Option<&Path>,
) -> ExitCode {
    let Some(out_dir) = args.frames_out.clone() else {
        return usage::usage_error(
            "`--replay-frames` requires `--frames-out <DIR>` — the directory to write the \
             numbered PNG sequence into",
        );
    };

    let bytes = match read_replay(replay_path) {
        Ok(bytes) => bytes,
        Err(diagnostic) => {
            eprintln!("{diagnostic}");
            return ExitCode::FAILURE;
        }
    };

    if let Err(error) = std::fs::create_dir_all(&out_dir) {
        let diagnostic = Diagnostic::new(
            code::OUTPUT_IO,
            format!(
                "could not create frames output directory `{}`: {error}",
                out_dir.display()
            ),
        )
        .with_help("check the frames output location is writable");
        eprintln!("{diagnostic}");
        return ExitCode::FAILURE;
    }

    let project_root = replay_project_root(scene_arg, builder.default_scene_hint(), replay_path);

    let debug =
        ludion_engine::DebugCategories::parse(args.debug_draw.as_deref().unwrap_or_default());
    let request = ludion_engine::ReplayFramesRequest {
        from: args.from.unwrap_or(0),
        to: args.to,
        width: args.width.unwrap_or(1920),
        height: args.height.unwrap_or(1080),
        debug: debug.clone(),
    };

    let mut count = 0u64;
    let write_frame =
        |frame: u64, png: &[u8]| -> Result<(), ludion_core::diagnostics::Diagnostic> {
            let path = out_dir.join(format!("frame-{frame:05}.png"));
            ludion_core::atomic::write_atomic(&path, png).map_err(|error| {
                ludion_core::diagnostics::Diagnostic::new(
                    ludion_core::codes::code::RENDER_PNG_IO,
                    format!("could not write frame `{}`: {error}", path.display()),
                )
                .with_help("check the frames output location is writable")
            })?;
            count += 1;
            if count.is_multiple_of(300) {
                eprintln!("  … rendered {count} frames (through frame {frame})");
            }
            Ok(())
        };

    match ludion_engine::replay_render_frames(builder, &bytes, &project_root, request, write_frame)
    {
        Ok(outcome) => {
            println!(
                "rendered {} frame(s) → {}{}",
                outcome.rendered,
                out_dir.display(),
                debug
                    .summary(&outcome.debug_drawn)
                    .map_or_else(String::new, |clause| format!(" ({clause})"))
            );
            let _ = std::io::stdout().flush();
            ExitCode::SUCCESS
        }
        Err(diagnostic) => {
            eprintln!("{diagnostic}");
            ExitCode::FAILURE
        }
    }
}

/// Runs `--profile-replay`: times this game's committed `.lrp` tick by tick through the game's own
/// [`EngineBuilder`] ([`ludion_engine::profile_replay`]), prints the `ProfileReport` JSON, and — with
/// `--profile-check` — compares it against a TOML budget, exactly as `--profile` does. Sim only: no
/// GPU. Content roots where hosting roots it — see [`replay_project_root`]. A bad `.lrp` or an
/// out-of-range window (`L3021`) prints a readable diagnostic to stderr and returns a non-zero code.
#[cfg(feature = "wire")]
fn run_profile_replay(
    builder: EngineBuilder,
    replay_path: &Path,
    args: &RunArgs,
    scene_arg: Option<&Path>,
) -> ExitCode {
    let bytes = match read_replay(replay_path) {
        Ok(bytes) => bytes,
        Err(diagnostic) => {
            eprintln!("{diagnostic}");
            return ExitCode::FAILURE;
        }
    };

    let project_root = replay_project_root(scene_arg, builder.default_scene_hint(), replay_path);
    let report = match ludion_engine::profile_replay(
        builder,
        &bytes,
        &project_root,
        args.from.unwrap_or(0),
        args.to,
        replay_path.display().to_string(),
    ) {
        Ok(report) => report,
        Err(diagnostic) => {
            eprintln!("{diagnostic}");
            return ExitCode::FAILURE;
        }
    };

    print!("{}", profile_report_json(&report));
    let _ = std::io::stdout().flush();

    match args.profile_check.as_deref() {
        Some(budget_path) => check_budget_report(&report, budget_path),
        None => ExitCode::SUCCESS,
    }
}

/// Runs `--replay-verify`: re-drives this game's committed `.lrp` through the game's own
/// [`EngineBuilder`] and compares the live state against every keyframe the recording embedded,
/// printing the first divergence.
///
/// This is the replay-churn check. A `.lrp` embeds a full world snapshot every
/// [`KEYFRAME_INTERVAL`](ludion_engine::KEYFRAME_INTERVAL) frames, and after a gameplay change those
/// keyframes describe code that no longer exists — so a re-record is a rubber stamp unless something
/// says *what* drifted. Clean means the committed replay still describes this build; a divergence
/// prints the frame, the entities, and the components that differ as `L3024` and exits non-zero.
///
/// No `--scene` is resolved here either, but content roots where hosting roots it — see
/// [`replay_project_root`] — so the replayed world carries the same data-docs the recording did
/// wherever the `.lrp` happens to be stored. A bad `.lrp`, an unresolvable scene, or a dispatch
/// failure each print a readable diagnostic to stderr and return a non-zero code.
#[cfg(feature = "wire")]
fn run_replay_verify(
    builder: EngineBuilder,
    replay_path: &Path,
    scene_arg: Option<&Path>,
) -> ExitCode {
    let bytes = match read_replay(replay_path) {
        Ok(bytes) => bytes,
        Err(diagnostic) => {
            eprintln!("{diagnostic}");
            return ExitCode::FAILURE;
        }
    };

    let project_root = replay_project_root(scene_arg, builder.default_scene_hint(), replay_path);

    let report = match ludion_engine::verify_replay_keyframes(builder, &bytes, &project_root) {
        Ok(report) => report,
        Err(diagnostic) => {
            eprintln!("{diagnostic}");
            return ExitCode::FAILURE;
        }
    };

    if report.is_clean() {
        println!("{} — {report}", replay_path.display());
        let _ = std::io::stdout().flush();
        return ExitCode::SUCCESS;
    }
    match report.into_result() {
        Ok(()) => ExitCode::SUCCESS,
        Err(diagnostic) => {
            eprintln!("{diagnostic}");
            ExitCode::FAILURE
        }
    }
}

/// Builds this game's engine rooted at `scene`'s own directory, **without loading the scene** — the
/// registry `--check-scene` validates against.
///
/// The rooting is [`EngineBuilder::load_scene_file`]'s (the scene path made absolute lexically, its
/// parent directory as the project root), so the check sees exactly the content root hosting sees.
/// The scene load itself is deliberately left out: the verb does not want the document instantiated
/// into a world before it has been checked, and it re-reads the file itself.
#[cfg(feature = "wire")]
fn build_for_scene(
    builder: EngineBuilder,
    scene: &Path,
) -> Result<ludion_engine::Engine, Vec<Diagnostic>> {
    let resolved = std::path::absolute(scene).unwrap_or_else(|_| scene.to_path_buf());
    let root = resolved
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_else(|| PathBuf::from("."));
    builder
        .build_at(root)
        .map_err(|diagnostic| vec![diagnostic])
}

/// Prints `diagnostics` to stderr through [`ludion_core::diagnostics::render_report`] — the one
/// renderer `ludion check-scene` shares — and returns the failure code iff any is an error.
#[cfg(feature = "wire")]
fn report_diagnostics(source: &Path, diagnostics: &[Diagnostic]) -> ExitCode {
    match fail_on_errors(source, diagnostics) {
        Some(exit) => exit,
        None => ExitCode::SUCCESS,
    }
}

/// [`report_diagnostics`] for a check with passes left to run: `Some(FAILURE)` iff an error was
/// printed, `None` when only warnings were raised.
#[cfg(feature = "wire")]
fn fail_on_errors(source: &Path, diagnostics: &[Diagnostic]) -> Option<ExitCode> {
    let rendered = ludion_core::diagnostics::render_report(source, diagnostics);
    eprint!("{}", rendered.text);
    (rendered.errors > 0).then_some(ExitCode::FAILURE)
}

/// Runs `--check-scene`: reads the scene and validates it against this game's registry
/// ([`ludion_scene::check`], the same one-step check the wire's `scene.check` and the CLI's
/// `check-scene` run), printing every diagnostic and a one-line summary.
///
/// The CLI's `check-scene` builds the *built-in* registry, so every scene naming a game component
/// fails it with `L1013`; this one builds the game's, which is the whole reason the flag exists. It
/// is CPU-only and hosts nothing: the cheapest "is my scene valid" an agent can ask from a command
/// line.
///
/// Everything the CLI's check runs runs here, and everything the wire's `scene.check` runs:
/// [`ludion_engine::scene_engine_diagnostics`] is the one list of engine passes (atlas, font,
/// collider layer, parented body, physics, skin bone, mesh asset, reflection probe and audio), run over the expanded scene after
/// the font table's own faults, so the three clients cannot answer differently about the same scene.
#[cfg(feature = "wire")]
fn run_check_scene(builder: EngineBuilder, scene: &Path) -> ExitCode {
    let engine = match build_for_scene(builder, scene) {
        Ok(engine) => engine,
        Err(diagnostics) => return report_diagnostics(scene, &diagnostics),
    };
    let source = match std::fs::read_to_string(scene) {
        Ok(source) => source,
        Err(error) => {
            let diagnostic = Diagnostic::new(
                ludion_core::codes::code::SCENE_IO,
                format!("could not read `{}`: {error}", scene.display()),
            )
            .with_help("check the path (relative to the current directory) and permissions");
            return report_diagnostics(scene, &[diagnostic]);
        }
    };
    let document = match ludion_scene::check(&source, engine.registry()) {
        Ok(document) => document,
        Err(diagnostics) => return report_diagnostics(scene, &diagnostics),
    };
    // The passes read other files (an `atlas` + `frame` pair names a region of a document on disk),
    // so they are rooted where the engine is. Prefab instances expand first, so the passes see what
    // loads; each diagnostic is mapped back to the document.
    let expanded =
        match ludion_engine::expand_scene(engine.project_root(), engine.registry(), &document) {
            Ok(expanded) => expanded,
            Err(errors) => return report_diagnostics(scene, &errors),
        };
    let located = |errors: Vec<Diagnostic>| -> Vec<Diagnostic> {
        errors.into_iter().map(|d| expanded.locate(d)).collect()
    };
    let document = &expanded.scene;
    let font_faults = ludion_engine::font_table_faults(engine.project_root());
    if !font_faults.is_empty() {
        for (file, fault) in font_faults {
            report_diagnostics(&engine.project_root().join(file), &[fault]);
        }
        return ExitCode::FAILURE;
    }
    let found = ludion_engine::scene_engine_diagnostics(engine.project_root(), document);
    if let Some(exit) = fail_on_errors(scene, &located(found)) {
        return exit;
    }
    let entities = document.entities.len();
    let components: usize = document
        .entities
        .iter()
        .map(|entity| entity.components.len())
        .sum();
    println!(
        "{}: OK ({entities} entit{}, {components} component{})",
        scene.display(),
        if entities == 1 { "y" } else { "ies" },
        if components == 1 { "" } else { "s" }
    );
    let _ = std::io::stdout().flush();
    ExitCode::SUCCESS
}

/// Runs `--render-out`: the exit-code wrapper around [`render_scene_out`], reporting whatever it
/// failed on in `--check-scene`'s shape.
#[cfg(feature = "wire")]
#[allow(clippy::too_many_arguments)]
fn run_render_out(
    builder: EngineBuilder,
    scene: &Path,
    out: &Path,
    width: Option<u32>,
    height: Option<u32>,
    frames: u64,
    quality: &ludion_engine::QualitySetting,
    debug: &ludion_engine::DebugCategories,
    anti_aliasing: Option<ludion_engine::AntiAliasing>,
) -> ExitCode {
    // No host mode exists on this path, so the builder level resolves identically to one.
    let builder = match anti_aliasing {
        Some(mode) => builder.anti_aliasing(mode),
        None => builder,
    };
    match render_scene_out(
        builder.observe_load(),
        scene,
        out,
        width,
        height,
        frames,
        quality,
        debug,
    ) {
        Ok(()) => ExitCode::SUCCESS,
        Err(diagnostics) => report_diagnostics(scene, &diagnostics),
    }
}

/// Renders `scene` through this game's own engine and writes the PNG, printing `ludion render`'s
/// summary line.
///
/// [`ludion_engine::render_scene_with_builder`] builds the game's engine on this scene and ticks the
/// **game's** schedule `frames` times before rendering — so the frame carries whatever the game's
/// systems put there, which is the whole reason this verb exists beside `ludion render` (the CLI
/// links no game code, so its `--frames` can only tick the built-in empty schedule).
///
/// The result is a `Vec<Diagnostic>` rather than an exit code so a test can tell an adapterless
/// machine's `L2003` from a real failure.
#[cfg(feature = "wire")]
#[allow(clippy::too_many_arguments)]
fn render_scene_out(
    builder: EngineBuilder,
    scene: &Path,
    out: &Path,
    width: Option<u32>,
    height: Option<u32>,
    frames: u64,
    quality: &ludion_engine::QualitySetting,
    debug: &ludion_engine::DebugCategories,
) -> Result<(), Vec<Diagnostic>> {
    let outcome = ludion_engine::render_scene_with_builder(
        builder, scene, width, height, quality, frames, debug,
    )?;
    ludion_core::atomic::write_atomic(out, &outcome.png).map_err(|error| {
        vec![
            Diagnostic::new(
                ludion_core::codes::code::RENDER_PNG_IO,
                format!("could not write `{}`: {error}", out.display()),
            )
            .with_help("check the output location is writable"),
        ]
    })?;
    // `ludion render`'s summary line, minus the resample clause no flag here can produce: there is
    // no `--scale`, so the render extent is always the output extent.
    println!(
        "{}: rendered {}x{}{}{} on {}",
        out.display(),
        outcome.size.0,
        outcome.size.1,
        if frames == 0 {
            String::new()
        } else {
            format!(" (frames {frames})")
        },
        debug
            .summary(&outcome.debug_drawn)
            .map_or_else(String::new, |clause| format!(" ({clause})")),
        outcome.adapter
    );
    let _ = std::io::stdout().flush();
    if let Some(load) = &outcome.load {
        eprintln!("{}", load_summary(load));
    }
    Ok(())
}

/// One line on what loading cost: every phase that ran in whole milliseconds, the share of the total
/// no phase accounts for, and what was decoded. Under `--frames N` the sim ticks between the scene
/// load and the first frame are in no phase, so they count as unattributed.
#[cfg(feature = "wire")]
fn load_summary(load: &ludion_protocol::perf::LoadProfile) -> String {
    let phases = [
        ("scene", Some(load.scene_ms)),
        ("docs", Some(load.docs_ms)),
        ("mesh", Some(load.mesh_ms)),
        ("texture", Some(load.texture_ms)),
        ("other", Some(load.other_ms)),
        ("voxelize", load.voxelize_ms),
        ("renderer init", load.renderer_init_ms),
        ("gpu upload", load.gpu_upload_ms),
        ("gi bake", load.gi_bake_ms),
        ("probe bake", load.probe_bake_ms),
        ("first frame", load.first_frame_ms),
    ];
    let attributed: f64 = phases.iter().filter_map(|(_, ms)| *ms).sum();
    let unattributed = (load.total_ms - attributed).max(0.0);
    let share = if load.total_ms > 0.0 {
        unattributed / load.total_ms * 100.0
    } else {
        0.0
    };
    let mut line = format!("load {:.0} ms: ", load.total_ms);
    for (name, ms) in phases {
        if let Some(ms) = ms {
            line.push_str(&format!("{name} {ms:.0}, "));
        }
    }
    line.push_str(&format!("unattributed {unattributed:.0} ({share:.0}%)"));
    let decoded: Vec<String> = load
        .assets
        .iter()
        .filter(|counters| counters.decoded > 0 || counters.cooked > 0)
        .map(|counters| match counters.cooked {
            0 => format!("{} {}", counters.decoded, counters.kind),
            cooked => format!("{} {} ({cooked} cooked)", counters.decoded, counters.kind),
        })
        .collect();
    if !decoded.is_empty() {
        line.push_str(&format!("; decoded {}", decoded.join(", ")));
    }
    line
}

/// Runs the in-process profile: builds the game's engine (its components, systems, and default
/// scene), drives the shared warmup + measured-frames loop headless, prints the `ProfileReport` JSON,
/// and — with `--profile-check` — compares it against a TOML budget. The engine, driver, and budget
/// machinery are exactly `ludion profile`'s; only the built-in-vs-game registry differs, which is the
/// whole point (a game's systems now appear in the report).
#[cfg(feature = "wire")]
fn run_profile(
    builder: EngineBuilder,
    scene: &Path,
    frames: u64,
    check: Option<&Path>,
    quality: ludion_engine::QualitySetting,
    anti_aliasing: Option<ludion_engine::AntiAliasing>,
    textures: TextureSource,
) -> ExitCode {
    let mut engine = match builder.observe_load().load_scene_file(scene) {
        Ok(engine) => engine,
        Err(diagnostic) => {
            eprintln!("{diagnostic}");
            return ExitCode::FAILURE;
        }
    };
    engine.set_quality(quality);
    if let Some(mode) = anti_aliasing {
        engine.set_anti_aliasing(mode);
    }
    engine.set_textures(textures);

    let warmup = ludion_engine::warmup_for(frames);
    let report = match ludion_engine::profile_engine(
        &mut engine,
        scene.display().to_string(),
        frames,
        warmup,
    ) {
        Ok(report) => report,
        Err(diagnostics) => return report_diagnostics(scene, &diagnostics),
    };

    print!("{}", profile_report_json(&report));
    let _ = std::io::stdout().flush();

    match check {
        Some(budget_path) => check_budget_report(&report, budget_path),
        None => ExitCode::SUCCESS,
    }
}

/// Runs `--trace`: the game's engine (its systems running) driven by the optional input script,
/// the [`ludion_engine::trace`] records printed as JSON Lines. No channel named means both.
#[cfg(feature = "wire")]
fn run_trace(
    builder: EngineBuilder,
    scene: &Path,
    frames: u64,
    script: Option<&Path>,
    channels: &[ludion_engine::TraceChannel],
) -> ExitCode {
    let script = match script.map(ludion_engine::load_input_script).transpose() {
        Ok(script) => script.unwrap_or_default(),
        Err(diagnostic) => {
            eprintln!("{diagnostic}");
            return ExitCode::FAILURE;
        }
    };
    let channels = if channels.is_empty() {
        &ludion_engine::TraceChannel::ALL[..]
    } else {
        channels
    };
    let records = match ludion_engine::trace_with(builder, scene, &script, frames, channels) {
        Ok(records) => records,
        Err(diagnostic) => {
            eprintln!("{diagnostic}");
            return ExitCode::FAILURE;
        }
    };
    let mut stdout = std::io::stdout().lock();
    match ludion_engine::write_jsonl(&records, &mut stdout).and_then(|()| stdout.flush()) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("could not write the trace to stdout: {error}");
            ExitCode::FAILURE
        }
    }
}

/// Serializes a [`ProfileReport`](ludion_engine::ProfileReport) to pretty JSON with a trailing
/// newline — the exact stdout form both `ludion profile` and `ludion::run --profile` print. Shared so
/// the CLI and the in-process game host emit byte-identical report text.
#[cfg(feature = "wire")]
pub fn profile_report_json(report: &ludion_engine::ProfileReport) -> String {
    let mut json = serde_json::to_string_pretty(report)
        .expect("the profile report is plain data and always serializes");
    json.push('\n');
    json
}

/// Loads the TOML budget at `budget_path`, checks `report` against it, reports every over-budget line
/// (and the within-budget summary) to stderr — stdout stays the report — and returns the process
/// exit code — the shared budget-check wrapper both `ludion profile --check` and
/// `ludion::run --profile-check` call, over the single-sourced [`ludion_engine::load_budget`] /
/// [`ludion_engine::check_budget`] core. An unreadable or
/// invalid budget file is an `L4004`; any over-budget line an `L4005`, and either exits non-zero.
///
/// **A report measured below full quality is refused rather than compared.** A budget is calibrated
/// against everything the scene authors, so its ceilings describe frames a reduced-quality run never
/// rendered: the comparison would pass for a reason nobody chose, which is worse than not running it.
/// The refusal reads off the report's own [`quality`](ludion_engine::ProfileReport::quality)
/// descriptor, so it covers an `--attach` capture of someone else's host exactly as it covers a local
/// run. A report carrying no descriptor at all predates the field and is compared as before. An
/// anti-aliased capture is refused for the same reason: it pays passes the budget never measured.
#[cfg(feature = "wire")]
pub fn check_budget_report(report: &ludion_engine::ProfileReport, budget_path: &Path) -> ExitCode {
    let full = ludion_engine::QualitySetting::FULL_LABEL;
    if let Some(quality) = &report.quality
        && quality.preset != full
    {
        eprintln!("{}", quality_refusal(&quality.preset, budget_path));
        return ExitCode::FAILURE;
    }
    if let Some(quality) = &report.quality
        && quality.anti_aliasing != ludion_engine::AntiAliasing::Off.name()
    {
        eprintln!(
            "{}",
            anti_aliasing_refusal(&quality.anti_aliasing, budget_path)
        );
        return ExitCode::FAILURE;
    }
    let budget = match ludion_engine::load_budget(budget_path) {
        Ok(budget) => budget,
        Err(diagnostic) => {
            eprintln!("{diagnostic}");
            return ExitCode::FAILURE;
        }
    };
    let over = ludion_engine::check_budget(report, &budget);
    if over.is_empty() {
        eprintln!("{}: all timings within budget", budget_path.display());
        return ExitCode::SUCCESS;
    }
    for diagnostic in &over {
        eprintln!("{diagnostic}");
    }
    eprintln!(
        "{}: {} timing{} over budget",
        budget_path.display(),
        over.len(),
        if over.len() == 1 { "" } else { "s" }
    );
    ExitCode::FAILURE
}

/// The `L4020` refusal to compare a report measured at `preset` against `budget`.
#[cfg(feature = "wire")]
fn quality_refusal(preset: &str, budget: &Path) -> Diagnostic {
    let full = ludion_engine::QualitySetting::FULL_LABEL;
    Diagnostic::new(
        code::BUDGET_QUALITY_MISMATCH,
        format!(
            "this measurement was taken at quality `{preset}`, so it cannot be checked against \
             `{}`",
            budget.display(),
        ),
    )
    .with_help(format!(
        "a budget is calibrated against everything the scene authors — re-measure at \
         `--quality {full}` (or with `LUDION_QUALITY` unset) to compare"
    ))
}

/// The `L4020` refusal to compare a report measured with anti-aliasing `mode` against `budget`.
#[cfg(feature = "wire")]
fn anti_aliasing_refusal(mode: &str, budget: &Path) -> Diagnostic {
    Diagnostic::new(
        code::BUDGET_QUALITY_MISMATCH,
        format!(
            "this measurement was taken with anti-aliasing `{mode}`, so it cannot be checked \
             against `{}`",
            budget.display(),
        ),
    )
    .with_help(
        "a budget is calibrated with anti-aliasing off — re-measure with `--aa off` to \
         compare",
    )
}

/// Runs `--bake-probes`: loads `scene` through the game's builder, bakes its reflection probes at
/// `quality` and prints the report as JSON, the object `ludion bake-probes --json` prints. A failure
/// prints its diagnostic on stderr, exits 1, and is also the JSON's `diagnostics`.
#[cfg(feature = "wire")]
fn run_bake_probes(
    builder: EngineBuilder,
    scene: &Path,
    probe: Option<&str>,
    quality: ludion_engine::QualitySetting,
) -> ExitCode {
    let baked = builder.load_scene_file(scene).and_then(|mut engine| {
        engine.set_quality(quality);
        engine.bake_probes(probe)
    });
    match baked {
        Ok(report) => {
            println!("{}", report.to_json(scene, true, &[]));
            ExitCode::SUCCESS
        }
        Err(diagnostic) => {
            eprintln!("{diagnostic}");
            let empty = ludion_engine::ProbeBakeReport::default();
            println!("{}", empty.to_json(scene, false, &[diagnostic]));
            ExitCode::FAILURE
        }
    }
}

/// Runs `--check-docs`: builds the game's engine (its components, systems, default scene, and
/// registered [`EngineBuilder::doc_kind`] kinds) against the resolved scene's content root and exits
/// — no `Server::bind`, no GPU, no "listening" line. Building already performs the full content-root
/// walk and validation as a side effect of construction (`EngineBuilder::build_at`'s `install_docs`
/// step, `Plan/phase-15-data-docs.md` decision 4): a malformed doc's `L1119`/`L1120` — and every
/// other broken doc's, carried as `related` sub-diagnostics that [`Diagnostic`](ludion_core::diagnostics::Diagnostic)'s
/// `Display` prints recursively — reaches stderr through the same collapsed-error path
/// [`load_scene_file`](EngineBuilder::load_scene_file) already uses for a bad scene, and the process
/// exits non-zero.
///
/// The build also runs every [`EngineBuilder::validate_docs`] pass the game registered — the
/// cross-reference rules (a quest's district, a ware's id, a spawn's enemy) schema validation cannot
/// see, after the engine's own `doc_singleton` count (`L1315`). An `Error` one of those passes reports (typically `L1144`) takes the same collapsed-error
/// path as a malformed doc, so a broken reference fails `--check-docs` the way a broken file does;
/// its `Warning`s survive on the built engine and are printed here, before the summary, without
/// failing the run.
///
/// On success it prints one summary line to stdout: how many game data-docs were found under the
/// content root, across how many distinct `kind:`s, and how many validation warnings stand. The run
/// fails (`L1155`) only when a counted kind has no registered doc kind behind it — a game's own or
/// an engine-owned one such as `.patterns()` — because such a doc was never validated and must not
/// read as validated. A patterns-only game passes; a game that registers nothing passes only when
/// its root holds no game data-docs.
#[cfg(feature = "wire")]
fn run_check_docs(builder: EngineBuilder, scene: &Path) -> ExitCode {
    let engine = match builder.load_scene_file(scene) {
        Ok(engine) => engine,
        Err(diagnostic) => {
            eprintln!("{diagnostic}");
            return ExitCode::FAILURE;
        }
    };

    let warnings = engine.doc_validation_warnings();
    for warning in warnings {
        eprintln!("{warning}");
    }

    let root = engine.project_root();
    let counted = count_data_docs(root);
    let doc_count: usize = counted.values().sum();
    let kinds: Vec<&String> = counted.keys().collect();
    let unregistered: Vec<(&String, usize)> = counted
        .iter()
        .filter(|(kind, _)| !engine.registers_doc_kind(kind))
        .map(|(kind, count)| (kind, *count))
        .collect();
    if !unregistered.is_empty() {
        match engine.doc_skip_summary() {
            Some(summary) => eprintln!("{summary}"),
            None => {
                let unvalidated: usize = unregistered.iter().map(|(_, count)| count).sum();
                eprintln!(
                    "{}",
                    Diagnostic::new(
                        code::DOC_KIND_NOT_REGISTERED,
                        format!(
                            "{unvalidated} data-doc{} under `{}` declare a `kind:` this game never \
                             registered: {} — nothing validated {}",
                            if unvalidated == 1 { "" } else { "s" },
                            root.display(),
                            unregistered
                                .iter()
                                .map(|(kind, count)| format!("{kind} ({count})"))
                                .collect::<Vec<_>>()
                                .join(", "),
                            if unvalidated == 1 { "it" } else { "them" },
                        ),
                    )
                    .with_help(
                        "register each kind on the game's builder with doc_kind::<T>(\"<kind>\")"
                    )
                );
            }
        }
        return ExitCode::FAILURE;
    }
    if doc_count == 0 {
        println!(
            "check-docs: {}: no game data-docs found — nothing to validate",
            root.display()
        );
    } else {
        println!(
            "check-docs: {}: {doc_count} data-doc{} across {} kind{} validated{}",
            root.display(),
            if doc_count == 1 { "" } else { "s" },
            kinds.len(),
            if kinds.len() == 1 { "" } else { "s" },
            match warnings.len() {
                0 => String::new(),
                1 => " — 1 warning".to_string(),
                n => format!(" — {n} warnings"),
            },
        );
    }
    let _ = std::io::stdout().flush();
    ExitCode::SUCCESS
}

/// Counts, per kind, every `*.yaml` the engine's content-root walk reads
/// ([`ludion_engine::content_root_yaml`]: build output and dot-directories are not content) whose
/// top-level `kind:` names a game data-doc (present, and not an engine-owned kind). Purely a
/// reporting walk: by the time `--check-docs` calls this, building the engine has already performed
/// the real validation, so every file of a registered kind counted here is known-valid.
///
/// The engine-owned kinds it excludes are the `ludion_scene::KINDS` rows through
/// [`ludion_engine::is_engine_kind`] — the same test the engine's own content-root walk skips by —
/// so there is no second copy to drift.
#[cfg(feature = "wire")]
fn count_data_docs(root: &Path) -> BTreeMap<String, usize> {
    let mut counts = BTreeMap::new();
    for (_, path) in ludion_engine::content_root_yaml(root) {
        let Ok(source) = std::fs::read_to_string(&path) else {
            continue;
        };
        let skipped = match ludion_scene::sniff_kind(&source) {
            ludion_scene::KindSniff::Absent => true,
            ludion_scene::KindSniff::Kind(kind) => ludion_engine::is_engine_kind(kind),
            ludion_scene::KindSniff::Undecided => false,
        };
        if !skipped
            && let Some(kind) = ludion_scene::peek_kind(&source)
            && !ludion_engine::is_engine_kind(&kind)
        {
            *counts.entry(kind).or_insert(0) += 1;
        }
    }
    counts
}

/// Prints the boot profile migration to stderr when it is not clean — a real game's one visible
/// signal that a saved profile drifted from the current type. The `L1121` reset renders like a scene
/// diagnostic (`warning[L1121]: …`); the informational field-drift entries render as short info lines.
/// A clean load (or a profile-free game) prints nothing.
fn report_profile_migration(engine: &ludion_engine::Engine) {
    use ludion_protocol::profile::ProfileMigrationEntry;

    let migration = engine.profile_migration();
    if migration.is_clean() {
        return;
    }
    for entry in &migration.entries {
        match entry {
            ProfileMigrationEntry::FieldsDefaulted { fields } => {
                eprintln!(
                    "profile: filled {} new field(s) from defaults: {}",
                    fields.len(),
                    fields.join(", ")
                );
            }
            ProfileMigrationEntry::FieldsDropped { fields } => {
                eprintln!(
                    "profile: dropped {} unknown field(s): {}",
                    fields.len(),
                    fields.join(", ")
                );
            }
            ProfileMigrationEntry::Reset { backup, reason } => {
                eprintln!("{}", profile_reset_diagnostic(backup, reason));
            }
        }
    }
}

/// The `L1121` diagnostic for a stored profile that was reset to defaults; `backup` is the path the
/// old file was moved to, empty when it could not be.
fn profile_reset_diagnostic(backup: &str, reason: &str) -> Diagnostic {
    let help = if backup.is_empty() {
        "started a fresh default profile".to_string()
    } else {
        format!("backed up to `{backup}` and started a fresh default profile")
    };
    Diagnostic::new(code::PROFILE_LOAD_FAILED, reason).with_help(help)
}

/// Prints each stored boot warning (`L1124` schedule-module concentration) to stderr in rustc's
/// warning style, before the engine moves onto the sim thread — the `report_profile_migration` mold.
/// It is a game author's one boot-time signal that a definition module is concentrating too many
/// schedule systems. Never fails a run; a game whose modules are all under threshold prints nothing,
/// and an engine built directly in a test (no host) prints nothing at all.
fn report_boot_warnings(engine: &ludion_engine::Engine) {
    for warning in engine.boot_warnings() {
        let mut warning = warning.clone();
        // The wiring site is a `file:line`; normalize Windows backslashes so it reads the same
        // on every platform.
        warning.path = warning.path.map(|path| path.replace('\\', "/"));
        eprintln!("{warning}");
    }
}

#[cfg(all(test, feature = "wire"))]
mod check_docs_tests {
    //! `run_check_docs` exercised in-process against real temp-directory content roots — the same
    //! fixture mold `ludion-engine/tests/docs.rs` uses for `doc_kind`/`DocStore` (Phase 15 / E4).
    //! There is no `[[bin]]` in this crate to spawn as a genuine subprocess (a game binary is the
    //! only thing that calls `ludion::run`), so these call `run_check_docs` directly — the exact
    //! function `run` short-circuits into for `--check-docs`, over a real `EngineBuilder::build`
    //! and a real filesystem content root, not a mock.

    use super::*;
    use schemars::JsonSchema;
    use serde::{Deserialize, Serialize};

    #[derive(Serialize, Deserialize, JsonSchema, Clone, Debug, PartialEq)]
    struct Weapon {
        damage: f32,
    }

    /// A fresh, empty temp directory unique to this process + `name` (removed first, so a re-run
    /// starts clean) — the content root a builder is pointed at.
    fn fresh_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "ludion-facade-check-docs-{}-{name}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// Writes `contents` to `root`/`rel`, creating parent directories.
    fn write(root: &Path, rel: &str, contents: &str) {
        let path = root.join(rel);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, contents).unwrap();
    }

    const SCENE: &str = "format: 1\nentities: []\n";
    const CROSSBOW: &str = "format: 1\nkind: weapon\ndamage: 12.0\n";
    const AXE: &str = "format: 1\nkind: weapon\ndamage: 30.0\n";

    /// A game that registers no kind never walks its content root, so a game data-doc on disk was
    /// validated by nothing: `--check-docs` fails rather than call it validated.
    #[test]
    fn an_unregistered_kind_fails_when_the_game_registers_none() {
        let dir = fresh_dir("zero-kinds");
        write(&dir, "scene.yaml", SCENE);
        write(&dir, "junk.yaml", "format: 1\nkind: wepon\n");

        let code = run_check_docs(EngineBuilder::new(), &dir.join("scene.yaml"));
        assert_eq!(code, ExitCode::FAILURE);
    }

    /// A game with no kinds and no game data-docs has nothing to validate and passes — the shape of
    /// a CI smoke over a scene-only project.
    #[test]
    fn no_kinds_and_no_docs_exits_success() {
        let dir = fresh_dir("no-kinds-no-docs");
        write(&dir, "scene.yaml", SCENE);

        let code = run_check_docs(EngineBuilder::new(), &dir.join("scene.yaml"));
        assert_eq!(code, ExitCode::SUCCESS);
    }

    /// An engine whose only kinds are engine-owned skips a game's kind with `L1155`; that doc was
    /// not validated either, so `--check-docs` fails.
    #[test]
    fn a_patterns_only_engine_fails_on_a_skipped_game_kind() {
        let dir = fresh_dir("patterns-only");
        write(&dir, "scene.yaml", SCENE);
        write(&dir, "weapons/crossbow.yaml", CROSSBOW);

        let code = run_check_docs(EngineBuilder::new().patterns(), &dir.join("scene.yaml"));
        assert_eq!(code, ExitCode::FAILURE);
    }

    /// `.patterns()` registers its kind through the engine-owned store, so a game whose only
    /// data-docs are patterns has every doc validated and passes.
    #[test]
    fn a_patterns_only_game_passes_on_its_pattern_docs() {
        let dir = fresh_dir("patterns-pass");
        write(&dir, "scene.yaml", SCENE);
        write(
            &dir,
            "patterns/probe.yaml",
            "format: 1\nkind: pattern\nname: probe\nbullets:\n  needle:\n    life: 60\n\
             timeline:\n  - at: 0\n    bullet: needle\n    speed: 4.0\n",
        );

        let code = run_check_docs(EngineBuilder::new().patterns(), &dir.join("scene.yaml"));
        assert_eq!(code, ExitCode::SUCCESS);
    }

    /// Build output and dot-directories are never part of the content root, so a stray yaml there
    /// is not a data-doc the game forgot to register.
    #[test]
    fn yaml_under_build_output_and_dot_dirs_is_not_counted() {
        let dir = fresh_dir("build-output");
        write(&dir, "scene.yaml", SCENE);
        for junk in [
            "target/debug/junk.yaml",
            "node_modules/x/junk.yaml",
            ".ludion/junk.yaml",
        ] {
            write(&dir, junk, "format: 1\nkind: wepon\n");
        }

        let code = run_check_docs(EngineBuilder::new(), &dir.join("scene.yaml"));
        assert_eq!(code, ExitCode::SUCCESS);
    }

    /// A registered kind with every doc on disk valid exits 0.
    #[test]
    fn valid_content_root_exits_success() {
        let dir = fresh_dir("valid");
        write(&dir, "scene.yaml", SCENE);
        write(&dir, "weapons/crossbow.yaml", CROSSBOW);
        write(&dir, "weapons/axe.yaml", AXE);

        let builder = EngineBuilder::new().doc_kind::<Weapon>("weapon");
        let code = run_check_docs(builder, &dir.join("scene.yaml"));
        assert_eq!(code, ExitCode::SUCCESS);
    }

    /// A `doc_singleton` kind with two docs on disk fails `--check-docs` with the engine's `L1315`.
    #[test]
    fn a_second_singleton_doc_fails_check_docs() {
        let dir = fresh_dir("singleton-two");
        write(&dir, "scene.yaml", SCENE);
        write(&dir, "weapons/crossbow.yaml", CROSSBOW);
        write(&dir, "weapons/axe.yaml", AXE);

        let builder = EngineBuilder::new().doc_singleton::<Weapon>("weapon");
        let code = run_check_docs(builder, &dir.join("scene.yaml"));
        assert_eq!(code, ExitCode::FAILURE);

        let builder = EngineBuilder::new().doc_singleton::<Weapon>("weapon");
        let err = match builder.load_scene_file(&dir.join("scene.yaml")) {
            Ok(_) => panic!("two singleton docs must fail the build"),
            Err(diagnostic) => diagnostic,
        };
        let rendered = err.to_string();
        assert!(rendered.contains("L1315"), "{rendered}");
        assert!(rendered.contains("weapons/axe.yaml"), "{rendered}");
    }

    /// Two independently-broken docs (one envelope error, one schema error) both fail the build, and
    /// — the point of the collapsed-diagnostic path — *both* L-codes reach what `run_check_docs`
    /// prints to stderr, not just the first. `Diagnostic`'s `Display` renders `related` recursively
    /// (`ludion-core/src/diagnostics.rs`), so asserting on the rendered text is exactly what
    /// `eprintln!("{diagnostic}")` would put on the stream.
    #[test]
    fn two_broken_docs_both_reach_the_rendered_diagnostic_and_exit_is_non_zero() {
        let dir = fresh_dir("broken");
        write(&dir, "scene.yaml", SCENE);
        // An unregistered kind (L1119) and a schema mismatch (L1120).
        write(&dir, "weapons/wrong_kind.yaml", "format: 1\nkind: enemy\n");
        write(
            &dir,
            "weapons/bad_field.yaml",
            "format: 1\nkind: weapon\ndamage: fast\n",
        );

        // `Engine` is not `Debug`, so `expect_err` cannot be used here.
        let builder = EngineBuilder::new().doc_kind::<Weapon>("weapon");
        let err = match builder.load_scene_file(&dir.join("scene.yaml")) {
            Ok(_) => panic!("both docs are malformed"),
            Err(diagnostic) => diagnostic,
        };
        let rendered = err.to_string();
        assert!(rendered.contains("L1119"), "{rendered}");
        assert!(rendered.contains("L1120"), "{rendered}");
        assert!(rendered.contains("wrong_kind.yaml"), "{rendered}");
        assert!(rendered.contains("bad_field.yaml"), "{rendered}");

        let builder = EngineBuilder::new().doc_kind::<Weapon>("weapon");
        let code = run_check_docs(builder, &dir.join("scene.yaml"));
        assert_eq!(code, ExitCode::FAILURE);
    }

    /// A second toy kind whose `refers_to` names a [`Weapon`] doc by its store key — the shape of
    /// cross-reference a JSON Schema cannot check, since any string is well-typed here.
    #[derive(Serialize, Deserialize, JsonSchema, Clone, Debug, PartialEq)]
    struct Loadout {
        refers_to: String,
    }

    const LOADOUT_OK: &str = "format: 1\nkind: loadout\nrefers_to: weapons/crossbow.yaml\n";
    const LOADOUT_BROKEN: &str = "format: 1\nkind: loadout\nrefers_to: weapons/ghost.yaml\n";

    /// The `EngineBuilder::validate_docs` pass these tests register: every `Loadout` must name a
    /// `Weapon` that loaded, reported as `L1144` on the `<kind>/<doc name>/<field>` document path.
    fn loadouts_name_a_weapon(
        world: &ludion_ecs::World,
    ) -> Vec<ludion_core::diagnostics::Diagnostic> {
        let (Some(loadouts), Some(weapons)) = (
            world.resource::<ludion_engine::DocStore<Loadout>>(),
            world.resource::<ludion_engine::DocStore<Weapon>>(),
        ) else {
            return Vec::new();
        };
        loadouts
            .iter()
            .filter(|(_, loadout)| weapons.get(&loadout.refers_to).is_none())
            .map(|(name, loadout)| {
                ludion_core::diagnostics::Diagnostic::new(
                    ludion_core::codes::code::DOC_REFERENCE_UNRESOLVED,
                    format!(
                        "loadout `{name}` names weapon `{}`, which is not in the content root",
                        loadout.refers_to
                    ),
                )
                .with_path(format!("loadout/{name}/refers_to"))
            })
            .collect()
    }

    /// A builder carrying both toy kinds and the cross-reference pass.
    fn validating_builder() -> EngineBuilder {
        EngineBuilder::new()
            .doc_kind::<Weapon>("weapon")
            .doc_kind::<Loadout>("loadout")
            .validate_docs(loadouts_name_a_weapon)
    }

    /// The seam's point: every doc passes its own schema, and the *reference between them* is what
    /// is broken. The registered pass reports `L1144`, which fails the build exactly as a malformed
    /// doc does — so `--check-docs` exits non-zero and the rendered diagnostic names the document
    /// path of the offending field.
    #[test]
    fn an_unresolved_doc_reference_fails_check_docs() {
        let dir = fresh_dir("unresolved-ref");
        write(&dir, "scene.yaml", SCENE);
        write(&dir, "weapons/crossbow.yaml", CROSSBOW);
        write(&dir, "loadouts/scout.yaml", LOADOUT_BROKEN);

        // `Engine` is not `Debug`, so `expect_err` cannot be used here.
        let err = match validating_builder().load_scene_file(&dir.join("scene.yaml")) {
            Ok(_) => panic!("the loadout names a weapon that is not there"),
            Err(diagnostic) => diagnostic,
        };
        let rendered = err.to_string();
        assert!(rendered.contains("L1144"), "{rendered}");
        assert!(
            rendered.contains("loadout/loadouts/scout.yaml/refers_to"),
            "{rendered}"
        );
        assert!(rendered.contains("weapons/ghost.yaml"), "{rendered}");

        assert_eq!(
            run_check_docs(validating_builder(), &dir.join("scene.yaml")),
            ExitCode::FAILURE
        );
    }

    /// The same content with the referenced weapon present is clean — the pass reports nothing and
    /// the run exits 0 with no warnings standing.
    #[test]
    fn a_resolved_doc_reference_passes_check_docs() {
        let dir = fresh_dir("resolved-ref");
        write(&dir, "scene.yaml", SCENE);
        write(&dir, "weapons/crossbow.yaml", CROSSBOW);
        write(&dir, "loadouts/scout.yaml", LOADOUT_OK);

        let engine = validating_builder()
            .load_scene_file(&dir.join("scene.yaml"))
            .unwrap_or_else(|d| panic!("{d}"));
        assert!(engine.doc_validation_warnings().is_empty());

        assert_eq!(
            run_check_docs(validating_builder(), &dir.join("scene.yaml")),
            ExitCode::SUCCESS
        );
    }

    /// A pass may also report a lint. Severity comes from the code's row in the table, so a
    /// `Warning`-severity code (which one is irrelevant to this seam — only its severity is)
    /// survives the build on the engine, is printed by `--check-docs`, and leaves the exit success.
    #[test]
    fn a_warning_from_a_validator_prints_without_failing() {
        let dir = fresh_dir("validator-warning");
        write(&dir, "scene.yaml", SCENE);
        write(&dir, "weapons/crossbow.yaml", CROSSBOW);

        let lint = |world: &ludion_ecs::World| {
            let Some(weapons) = world.resource::<ludion_engine::DocStore<Weapon>>() else {
                return Vec::new();
            };
            weapons
                .iter()
                .map(|(name, _)| {
                    ludion_core::diagnostics::Diagnostic::new(
                        ludion_core::codes::code::MESH_VERTEX_BUDGET_EXCEEDED,
                        format!("weapon `{name}` is over its authoring budget"),
                    )
                    .with_path(format!("weapon/{name}/damage"))
                })
                .collect()
        };
        let builder = || {
            EngineBuilder::new()
                .doc_kind::<Weapon>("weapon")
                .validate_docs(lint)
        };

        let engine = builder()
            .load_scene_file(&dir.join("scene.yaml"))
            .unwrap_or_else(|d| panic!("{d}"));
        let warnings = engine.doc_validation_warnings();
        assert_eq!(warnings.len(), 1, "{warnings:?}");
        assert_eq!(warnings[0].code.as_str(), "L1112");
        assert_eq!(
            warnings[0].path.as_deref(),
            Some("weapon/weapons/crossbow.yaml/damage")
        );

        assert_eq!(
            run_check_docs(builder(), &dir.join("scene.yaml")),
            ExitCode::SUCCESS
        );
    }

    /// A `save/profile.yaml` under the content root must never fail `--check-docs`: `profile` is an
    /// engine-owned kind the doc walk skips (owned by the profile loader), so a game with a registered
    /// doc kind *and* a saved profile validates clean. Without the `profile` skip, the walk would hit
    /// the save file as an unregistered `kind: profile` and fail the build (L1119).
    #[test]
    fn a_saved_profile_is_ignored_by_check_docs() {
        let dir = fresh_dir("profile-skip");
        write(&dir, "scene.yaml", SCENE);
        write(&dir, "weapons/crossbow.yaml", CROSSBOW);
        // A player save beside the content — an engine-owned `kind: profile`, not a data-doc.
        write(
            &dir,
            "save/profile.yaml",
            "format: 1\nkind: profile\nunlocked_crossbow: true\nruns: 7\n",
        );

        let builder = EngineBuilder::new().doc_kind::<Weapon>("weapon");
        let code = run_check_docs(builder, &dir.join("scene.yaml"));
        assert_eq!(
            code,
            ExitCode::SUCCESS,
            "the profile file is engine-owned and must not fail the data-doc walk"
        );
    }

    /// `--check-docs` composes with an explicit scene exactly like `--profile` does: the scene's own
    /// directory becomes the content root (`EngineBuilder::load_scene_file`), not the process's
    /// current directory or the scene's grandparent. A malformed doc that sits *outside* the scene's
    /// directory (a sibling of it, not under it) must never be walked — proven by it not failing the
    /// build; if the root were resolved one level too high, this exact malformed doc would fail it.
    #[test]
    fn content_root_is_the_scenes_own_directory_not_its_parent() {
        let dir = fresh_dir("root-is-scene-dir");
        write(&dir, "scenes/game.yaml", SCENE);
        write(&dir, "scenes/weapons/crossbow.yaml", CROSSBOW);
        // Outside the scene's own directory — a bad envelope that would fail the build if the walk
        // ever reached it.
        write(
            &dir,
            "weapons/bad_field.yaml",
            "format: 1\nkind: weapon\ndamage: fast\n",
        );

        let builder = EngineBuilder::new().doc_kind::<Weapon>("weapon");
        let code = run_check_docs(builder, &dir.join("scenes/game.yaml"));
        assert_eq!(
            code,
            ExitCode::SUCCESS,
            "the malformed doc outside scenes/ must never be walked"
        );
    }
}

#[cfg(all(test, feature = "wire"))]
mod host_diagnostic_tests {
    //! The host's coded failures render through `Diagnostic`'s `Display` alone.

    use super::*;

    #[test]
    fn profile_reset_renders_one_coded_warning() {
        let rendered = profile_reset_diagnostic("p.bak", "bad envelope").to_string();
        assert!(
            rendered.starts_with("warning[L1121]: bad envelope"),
            "{rendered}"
        );
        assert_eq!(rendered.matches("warning").count(), 1, "{rendered}");
        assert!(rendered.contains("p.bak"), "{rendered}");
    }

    #[test]
    fn profile_reset_without_a_backup_says_only_that_it_started_fresh() {
        let rendered = profile_reset_diagnostic("", "bad envelope").to_string();
        assert!(!rendered.contains("backed up"), "{rendered}");
        assert!(rendered.contains("fresh default profile"), "{rendered}");
    }

    #[test]
    fn read_replay_of_a_missing_path_is_a_coded_diagnostic_naming_it() {
        let path = std::env::temp_dir().join(format!(
            "ludion-read-replay-{}-missing.lrp",
            std::process::id()
        ));
        let diagnostic = read_replay(&path).expect_err("the path does not exist");
        assert_eq!(diagnostic.code, code::SCENE_IO);
        assert!(
            diagnostic.message.contains(&path.display().to_string()),
            "{}",
            diagnostic.message
        );
    }

    #[test]
    fn the_boot_restore_line_has_one_shape_per_outcome() {
        use ludion_core::ids::EntityId;
        use ludion_protocol::restore::{RestoreEntry, RestoreReport};

        let report = RestoreReport {
            entries: vec![RestoreEntry::ComponentUnregistered {
                entity: EntityId::new(2, 0),
                component: "Player".into(),
            }],
        };
        let line = boot_restore_line(&Ok((7, report)));
        assert!(
            line.starts_with(
                "restored frame=7 report={\"entries\":[{\"kind\":\"component_unregistered\""
            ),
            "{line}"
        );

        let diagnostic = Diagnostic::new(code::PROTOCOL_SNAPSHOT_INVALID, "bad");
        let line = boot_restore_line(&Err(diagnostic));
        assert!(
            line.starts_with("restore failed: {") && line.contains("\"L3008\""),
            "{line}"
        );
    }

    #[test]
    fn restore_conflicts_with_record_and_the_non_hosting_modes() {
        assert!(RunArgs::try_parse_from(["game", "--restore", "x"]).is_ok());
        for other in [
            ["--record", "y"],
            ["--replay-verify", "y"],
            ["--render-out", "y"],
        ] {
            let args = [
                "game",
                "--scene",
                "s.yaml",
                "--restore",
                "x",
                other[0],
                other[1],
            ];
            assert!(RunArgs::try_parse_from(args).is_err(), "{other:?}");
        }
    }

    #[test]
    fn debug_draw_needs_a_render_or_a_replay_render() {
        let replay = [
            "game",
            "--replay-frames",
            "r.lrp",
            "--frames-out",
            "d",
            "--debug-draw",
            "motor",
        ];
        assert!(RunArgs::try_parse_from(replay).is_ok());
        let render = [
            "game",
            "--scene",
            "s.yaml",
            "--render-out",
            "o.png",
            "--debug-draw",
            "motor",
        ];
        assert!(RunArgs::try_parse_from(render).is_ok());
        let alone = ["game", "--scene", "s.yaml", "--debug-draw", "motor"];
        assert!(RunArgs::try_parse_from(alone).is_err());
    }

    #[test]
    fn aa_parses_a_mode_and_refuses_the_rest() {
        use clap::builder::TypedValueParser;
        use ludion_engine::AntiAliasing;

        assert_eq!(RunArgs::try_parse_from(["game"]).unwrap().aa, None);
        let parsed = RunArgs::try_parse_from(["game", "--aa", "smaa"]).unwrap();
        assert_eq!(parsed.aa, Some(AntiAliasing::Smaa));
        assert!(RunArgs::try_parse_from(["game", "--aa", "bogus"]).is_err());
        let listed: Vec<_> = aa_value_parser()
            .possible_values()
            .expect("a possible-values parser lists its values")
            .map(|value| value.get_name().to_string())
            .collect();
        assert_eq!(
            listed,
            AntiAliasing::ALL.map(|mode| mode.name().to_string())
        );
    }

    #[test]
    fn textures_parses_a_source_and_refuses_the_rest() {
        let parsed = RunArgs::try_parse_from(["game", "--textures", "cooked"]).unwrap();
        assert_eq!(parsed.textures.as_deref(), Some("cooked"));
        assert!(
            RunArgs::try_parse_from(["game", "--profile", "5", "--textures", "cooked"]).is_ok()
        );
        assert!(RunArgs::try_parse_from(["game", "--textures", "bogus"]).is_err());
        for other in [
            ["--render-out", "y"],
            ["--replay-verify", "y"],
            ["--replay-frames", "y"],
            ["--check-scene", ""],
        ] {
            let mut args = vec![
                "game",
                "--scene",
                "s.yaml",
                "--textures",
                "cooked",
                other[0],
            ];
            if !other[1].is_empty() {
                args.push(other[1]);
            }
            assert!(RunArgs::try_parse_from(args).is_err(), "{other:?}");
        }
        assert!(RunArgs::try_parse_from(["game", "--check-docs", "--textures", "cooked"]).is_err());
    }

    #[test]
    fn probes_parses_a_source_and_refuses_verification_modes() {
        for value in ["store", "rebake"] {
            let parsed = RunArgs::try_parse_from(["game", "--probes", value]).unwrap();
            assert_eq!(parsed.probes.as_deref(), Some(value));
        }
        assert!(RunArgs::try_parse_from(["game", "--probes", "bogus"]).is_err());
        for other in [
            ["--render-out", "y"],
            ["--replay-verify", "y"],
            ["--replay-frames", "y"],
            ["--profile", "5"],
            ["--check-scene", ""],
        ] {
            let mut args = vec!["game", "--scene", "s.yaml", "--probes", "rebake", other[0]];
            if !other[1].is_empty() {
                args.push(other[1]);
            }
            assert!(RunArgs::try_parse_from(args).is_err(), "{other:?}");
        }
        assert!(RunArgs::try_parse_from(["game", "--check-docs", "--probes", "rebake"]).is_err());
    }

    #[test]
    fn trace_takes_its_script_and_channels_and_refuses_other_modes() {
        let parsed = RunArgs::try_parse_from([
            "game",
            "--trace",
            "30",
            "--script",
            "in.json",
            "--channels",
            "motor,sensors",
        ])
        .unwrap();
        assert_eq!(parsed.trace, Some(30));
        assert_eq!(parsed.channels.len(), 2);
        assert!(RunArgs::try_parse_from(["game", "--script", "in.json"]).is_err());
        assert!(RunArgs::try_parse_from(["game", "--trace", "5", "--channels", "bogus"]).is_err());
        for other in [["--profile", "5"], ["--check-docs", ""], ["--window", ""]] {
            let mut args = vec!["game", "--trace", "5", other[0]];
            if !other[1].is_empty() {
                args.push(other[1]);
            }
            assert!(RunArgs::try_parse_from(args).is_err(), "{other:?}");
        }
    }

    #[test]
    fn bake_probes_stands_alone_and_bake_probe_needs_it() {
        assert!(RunArgs::try_parse_from(["game", "--bake-probes"]).is_ok());
        assert!(RunArgs::try_parse_from(["game", "--bake-probes", "--bake-probe", "room"]).is_ok());
        assert!(RunArgs::try_parse_from(["game", "--bake-probe", "room"]).is_err());
        for other in [
            ["--profile", "5"],
            ["--check-docs", ""],
            ["--textures", "cooked"],
            ["--probes", "rebake"],
            ["--window", ""],
        ] {
            let mut args = vec!["game", "--bake-probes", other[0]];
            if !other[1].is_empty() {
                args.push(other[1]);
            }
            assert!(RunArgs::try_parse_from(args).is_err(), "{other:?}");
        }
    }
}

#[cfg(all(test, feature = "wire"))]
mod budget_quality_tests {
    //! The one gate that stands between a reduced-quality measurement and a budget calibrated on
    //! the full one. It lives in `check_budget_report` rather than in either caller, so `ludion
    //! profile --check`, `ludion profile --attach --check`, and a game's own `--profile-check` are
    //! covered by the same six lines.

    use super::*;
    use ludion_engine::{AntiAliasing, ProfileReport, QualitySetting, SimProfile};

    /// A report labelled `quality`, with timings comfortably inside [`budget`]'s ceilings — the
    /// descriptor is the subject here, not the numbers.
    fn report_at(quality: Option<&str>, aa: ludion_engine::AntiAliasing) -> ProfileReport {
        ProfileReport {
            scene: "scenes/main.yaml".to_string(),
            warmup_frames: 0,
            measured_frames: 60,
            quality: quality.map(|preset| {
                let mut descriptor = QualitySetting::default().descriptor(aa);
                descriptor.preset = preset.to_string();
                descriptor
            }),
            sim: SimProfile {
                systems: Vec::new(),
                sequential: Vec::new(),
                frame_total_median_ms: 0.5,
                frame_total_tail: None,
                threads: Some(1),
            },
            gpu: ludion_engine::GpuProfile {
                available: false,
                timestamps_supported: false,
                passes: Vec::new(),
                frame_median_ms: None,
                frame_tail: None,
            },
            transport: None,
            cluster: None,
            local_shadows: None,
            sun_cascades: None,
            decals: None,
            load: None,
            probes: None,
            samples: None,
        }
    }

    /// A real budget file whose one ceiling [`report_at`] sits under, so a run that reaches the
    /// comparison passes it and one that is refused cannot be mistaken for an over-budget line.
    fn budget(name: &str) -> PathBuf {
        let path = std::env::temp_dir().join(format!(
            "ludion-budget-quality-{}-{name}.toml",
            std::process::id()
        ));
        std::fs::write(
            &path,
            "frame_total_ms = 16.6
",
        )
        .expect("the scratch budget is writable");
        path
    }

    /// Every setting that is not `full` is refused rather than compared — including the ones whose
    /// toggles currently coincide with full's, because the report describes the *choice* and a
    /// budget describes a frame that rendered everything.
    #[test]
    fn a_non_full_measurement_is_refused_rather_than_compared() {
        let budget = budget("refused");
        for preset in ["low", "medium", "high", QualitySetting::CUSTOM_LABEL] {
            assert_eq!(
                check_budget_report(&report_at(Some(preset), AntiAliasing::Off), &budget),
                ExitCode::FAILURE,
                "`{preset}` must be refused, not compared",
            );
        }
        let _ = std::fs::remove_file(&budget);
    }

    /// The refusal is a coded diagnostic, and its first line leads with the code.
    #[test]
    fn the_quality_refusal_is_coded_and_names_both_sides() {
        let diagnostic = quality_refusal("low", Path::new("budgets/x.toml"));
        assert_eq!(diagnostic.code, code::BUDGET_QUALITY_MISMATCH);
        let rendered = diagnostic.to_string();
        assert!(rendered.starts_with("error[L4020]: "), "{rendered}");
        assert!(rendered.contains("`low`"), "{rendered}");
        assert!(rendered.contains("x.toml"), "{rendered}");
    }

    /// A full-quality report is compared normally, and so is one from a host predating the
    /// descriptor — an absent field is missing information, not evidence of a reduced setting.
    #[test]
    fn a_full_or_descriptorless_measurement_is_compared() {
        let budget = budget("compared");
        for report in [
            report_at(Some("full"), AntiAliasing::Off),
            report_at(None, AntiAliasing::Off),
        ] {
            assert_eq!(check_budget_report(&report, &budget), ExitCode::SUCCESS);
        }
        let _ = std::fs::remove_file(&budget);
    }

    /// An SMAA capture pays passes the budget never measured, so a `full` report with the mode on
    /// is refused with the same code as a reduced preset.
    #[test]
    fn an_anti_aliased_measurement_is_refused_rather_than_compared() {
        let budget = budget("anti-aliased");
        assert_eq!(
            check_budget_report(&report_at(Some("full"), AntiAliasing::Smaa), &budget),
            ExitCode::FAILURE,
        );
        let _ = std::fs::remove_file(&budget);
        let diagnostic = anti_aliasing_refusal("smaa", Path::new("budgets/x.toml"));
        assert_eq!(diagnostic.code, code::BUDGET_QUALITY_MISMATCH);
        let rendered = diagnostic.to_string();
        assert!(rendered.starts_with("error[L4020]: "), "{rendered}");
        assert!(rendered.contains("`smaa`"), "{rendered}");
        assert!(rendered.contains("x.toml"), "{rendered}");
    }
}

#[cfg(all(test, feature = "wire"))]
mod replay_root_tests {
    //! Where a replay mode resolves content from. The rule under test is not about replay at all:
    //! it is that `--replay-verify` builds the *same* engine hosting builds, and hosting roots at
    //! its scene file's directory. A replay committed anywhere else — the game root, a `replays/`
    //! folder — must not move the content root, or the replayed world silently carries none of the
    //! game's data-docs and every keyframe "diverges" for a reason that is not a code change.

    use super::*;
    use schemars::JsonSchema;
    use serde::{Deserialize, Serialize};

    #[derive(Serialize, Deserialize, JsonSchema, Clone, Debug, PartialEq)]
    struct Weapon {
        damage: f32,
    }

    const SCENE: &str = "format: 1\nentities: []\n";
    const CROSSBOW: &str = "format: 1\nkind: weapon\ndamage: 12.0\n";

    /// A fresh, empty temp directory unique to this process + `name` (removed first, so a re-run
    /// starts clean).
    fn fresh_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "ludion-facade-replay-root-{}-{name}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// Writes `contents` to `root`/`rel`, creating parent directories.
    fn write(root: &Path, rel: &str, contents: &str) {
        let path = root.join(rel);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, contents).unwrap();
    }

    /// The discriminator: a registered validation pass that refuses a content root with no weapon
    /// doc in it. Doc stores are resources, so a missing doc is invisible to a `WorldHash` and to a
    /// keyframe — this pass is what makes "which root did it walk?" observable as a build failure.
    fn a_weapon_must_exist(world: &ludion_ecs::World) -> Vec<ludion_core::diagnostics::Diagnostic> {
        let Some(weapons) = world.resource::<ludion_engine::DocStore<Weapon>>() else {
            return Vec::new();
        };
        if !weapons.is_empty() {
            return Vec::new();
        }
        vec![ludion_core::diagnostics::Diagnostic::new(
            ludion_core::codes::code::DOC_REFERENCE_UNRESOLVED,
            "no weapon doc was found in the content root".to_string(),
        )]
    }

    /// The toy game: one doc kind, the pass above, and a default scene — the hint `run` reads to
    /// root every mode.
    fn game(default_scene: &Path) -> EngineBuilder {
        EngineBuilder::new()
            .doc_kind::<Weapon>("weapon")
            .validate_docs(a_weapon_must_exist)
            .default_scene(default_scene)
    }

    /// A session recorded by a hosted engine, verified back with the `.lrp` sitting **outside** the
    /// content root. Rooted the old way — at the replay file's own directory — the docs are not
    /// there and the engine refuses to build; rooted the way hosting roots, the recording
    /// reproduces every keyframe it embedded.
    #[test]
    fn replay_verify_roots_at_the_scene_not_at_the_replay_file() {
        let dir = fresh_dir("outside-content");
        let content = dir.join("content");
        let scene = content.join("stage.yaml");
        write(&content, "stage.yaml", SCENE);
        write(&content, "weapons/crossbow.yaml", CROSSBOW);
        // Committed away from the scenes, the way a game keeps its replay beside its manifest.
        let replay = dir.join("replays/session.lrp");
        std::fs::create_dir_all(replay.parent().unwrap()).unwrap();

        // Record through the hosted shape: rooted at the scene's directory, scene inline so the
        // artifact is self-contained. Long enough to cross one `KEYFRAME_INTERVAL` boundary, so the
        // file carries a keyframe there as well as the one arming writes.
        let mut engine = game(&scene)
            .project_root(&content)
            .build()
            .unwrap_or_else(|d| panic!("{d}"));
        engine
            .load_inline_scene(SCENE)
            .unwrap_or_else(|d| panic!("{d:?}"));
        engine
            .set_recording(std::fs::File::create(&replay).expect("the recording sink is writable"))
            .expect("recording arms");
        ludion_engine::drive_hashes(
            &mut engine,
            vec![ludion_protocol::Command::SimStep(
                ludion_protocol::command::SimStepParams {
                    ticks: ludion_engine::KEYFRAME_INTERVAL + 1,
                    hash: false,
                },
            )],
        )
        .unwrap_or_else(|d| panic!("{d}"));
        engine.stop_recording().expect("recording flushes");

        let bytes = std::fs::read(&replay).expect("the recording reads back");

        // The control: the replay file's own directory as the root. No weapon doc lives there, so
        // the pass fails the build before a single frame is driven.
        assert!(
            ludion_engine::verify_replay_keyframes(
                game(&scene),
                &bytes,
                replay.parent().expect("the replay has a directory")
            )
            .is_err(),
            "rooting at the replay file must not find the game's docs — otherwise this test proves \
             nothing"
        );

        // The rule: hosting's root, derived from the builder's default scene.
        let report = ludion_engine::verify_replay_keyframes(
            game(&scene),
            &bytes,
            replay_project_root(None, Some(&scene), &replay),
        )
        .unwrap_or_else(|d| panic!("{d}"));
        assert!(
            report.keyframes >= 2,
            "the session must embed a keyframe past the arming one: {report}"
        );
        assert!(report.is_clean(), "{report}");

        // And the exit code the flag returns, through the function `run` dispatches into.
        assert_eq!(
            run_replay_verify(game(&scene), &replay, None),
            ExitCode::SUCCESS
        );
    }

    /// The root rule itself, in the shapes a game presents it with. The first is vigil's: the
    /// replay committed at the game root, where a bare file name's parent is the empty path — which
    /// walks nothing at all.
    #[test]
    fn the_content_root_comes_from_the_scene_argument() {
        let cwd = std::env::current_dir().expect("a current directory");
        assert_eq!(
            replay_project_root(
                None,
                Some(Path::new("scenes/camp.yaml")),
                Path::new("run.lrp")
            ),
            cwd.join("scenes")
        );
        // An explicit `--scene` outranks the default, exactly as it does when hosting.
        assert_eq!(
            replay_project_root(
                Some(Path::new("other/stage.yaml")),
                Some(Path::new("scenes/camp.yaml")),
                Path::new("run.lrp")
            ),
            cwd.join("other")
        );
        // Only a game with no scene of either kind falls back to the replay's own directory.
        assert_eq!(
            replay_project_root(None, None, Path::new("recordings/run.lrp")),
            cwd.join("recordings")
        );
        // And with nothing to take a directory from, the current one.
        assert_eq!(
            replay_project_root(None, None, Path::new("run.lrp")),
            PathBuf::from(".")
        );
    }
}

#[cfg(all(test, feature = "wire"))]
mod scene_verb_tests {
    //! `--check-scene` and `--render-out` exercised in-process against real temp-directory content
    //! roots, the `check_docs_tests` mold: there is no `[[bin]]` in this crate to spawn (a game
    //! binary is the only thing that calls `ludion::run`), so these call the exact functions `run`
    //! short-circuits into, over a real `EngineBuilder::build_at` and a real filesystem.
    //!
    //! The property both tests turn on is the registry: the scene names a game component, which the
    //! built-in registry rejects with `L1013`, so a pass here can only come from the game's own.

    use super::*;
    use schemars::JsonSchema;
    use serde::{Deserialize, Serialize};

    /// The game component the fixture scene names — invisible to the renderer (only built-in visual
    /// components draw), and unknown to the built-in registry, which is the point.
    #[derive(Serialize, Deserialize, JsonSchema, Clone, Copy)]
    struct Paddle {
        x: f32,
    }

    /// A scene naming [`Paddle`] beside the built-in visuals: a camera looking at a lit quad.
    const SCENE: &str = r#"format: 1
entities:
  - name: camera
    components:
      Transform:
        translation:
          - 0.0
          - 0.0
          - 6.0
      Camera:
        projection:
          type: perspective
          fov_y_deg: 60.0
  - name: paddle
    components:
      Transform:
        translation:
          - 0.0
          - -2.0
          - 0.0
      Sprite:
        color:
          - 0.2
          - 0.8
          - 1.0
          - 1.0
      Paddle:
        x: 0.0
"#;

    /// The game: one component, and nothing else a scene verb needs.
    fn game() -> EngineBuilder {
        EngineBuilder::new().components(|reg| reg.register::<Paddle>("Paddle"))
    }

    /// A fresh, empty temp directory unique to this process + `name`, holding `scene.yaml`.
    fn fixture(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "ludion-facade-scene-verbs-{}-{name}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("scene.yaml"), SCENE).unwrap();
        dir
    }

    /// The seam itself: a scene naming a game component checks clean against the game's registry and
    /// fails against a registry without it. The failing half is what makes the passing half mean
    /// something — without it the test would pass for a game with no components at all.
    #[test]
    fn a_game_component_checks_against_the_games_registry_and_not_the_builtins() {
        let dir = fixture("check");
        let scene = dir.join("scene.yaml");

        assert_eq!(run_check_scene(game(), &scene), ExitCode::SUCCESS);

        let diagnostics = ludion_scene::check(SCENE, &ludion_engine::builtin_registry())
            .expect_err("the built-in registry does not know `Paddle`");
        assert!(
            diagnostics.iter().any(|d| d.code.as_str() == "L1013"),
            "{diagnostics:?}"
        );
        assert_eq!(
            run_check_scene(EngineBuilder::new(), &scene),
            ExitCode::FAILURE
        );
    }

    /// A malformed scene exits non-zero through the same path.
    #[test]
    fn a_broken_scene_fails_check_scene() {
        let dir = fixture("broken");
        let scene = dir.join("scene.yaml");
        std::fs::write(
            &scene,
            "format: 1\nentities:\n  - name: x\n    components:\n      Transform:\n        \
             translation: sideways\n",
        )
        .unwrap();
        assert_eq!(run_check_scene(game(), &scene), ExitCode::FAILURE);
    }

    /// The other half of the check, the one that reads the filesystem: a `Sprite` naming an atlas
    /// that is not on disk is textually perfect and draws nothing, so `--check-scene` runs the same
    /// `L1151` pass `ludion check-scene` does rather than answering differently about one scene.
    #[test]
    fn an_unresolvable_atlas_reference_fails_check_scene() {
        let dir = fixture("atlas");
        let scene = dir.join("scene.yaml");
        std::fs::write(
            &scene,
            "format: 1\nentities:\n  - name: paddle\n    components:\n      Sprite:\n        \
             atlas: sprites/missing.yaml\n        frame: paddle\n",
        )
        .unwrap();

        let document = ludion_scene::check(
            &std::fs::read_to_string(&scene).unwrap(),
            &ludion_engine::builtin_registry(),
        )
        .unwrap_or_else(|d| panic!("the scene is textually valid: {d:?}"));
        let diagnostics = ludion_engine::scene_atlas_diagnostics(&dir, &document);
        assert!(
            diagnostics.iter().any(|d| d.code.as_str() == "L1151"),
            "{diagnostics:?}"
        );

        assert_eq!(run_check_scene(game(), &scene), ExitCode::FAILURE);
    }

    /// An `AudioEmitter` whose WAV is not on disk plays nothing: `--check-scene` runs the sound pass.
    #[test]
    fn a_missing_sound_fails_check_scene() {
        let dir = fixture("sound");
        let scene = dir.join("scene.yaml");
        std::fs::write(
            &scene,
            "format: 1\nentities:\n  - name: lamp\n    components:\n      AudioEmitter:\n        \
             sound: hum.wav\n",
        )
        .unwrap();
        assert_eq!(run_check_scene(game(), &scene), ExitCode::FAILURE);
    }

    /// A `Mesh` whose document is not on disk draws nothing: `--check-scene` runs the mesh-asset pass,
    /// and passes once the file exists.
    #[test]
    fn a_missing_mesh_fails_check_scene_until_the_file_exists() {
        let dir = fixture("mesh");
        let scene = dir.join("scene.yaml");
        std::fs::write(
            &scene,
            "format: 1
entities:
  - name: rock
    components:
      Mesh:
        shape:
                       asset: rock.mesh.yaml
",
        )
        .unwrap();
        assert_eq!(run_check_scene(game(), &scene), ExitCode::FAILURE);
        std::fs::copy(
            concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/../ludion-cli/tests/fixtures/good_mesh.mesh.yaml"
            ),
            dir.join("rock.mesh.yaml"),
        )
        .unwrap();
        assert_eq!(run_check_scene(game(), &scene), ExitCode::SUCCESS);
    }

    /// `L1209` (an atlas sprite crossfading) is a lint: `--check-scene` prints it and still passes.
    #[test]
    fn an_atlas_crossfade_lint_does_not_fail_check_scene() {
        let dir = fixture("crossfade");
        let sprite = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../ludion-cli/tests/fixtures/sprite/"
        );
        for file in ["ship.atlas.yaml", "ship.png"] {
            std::fs::copy(format!("{sprite}{file}"), dir.join(file)).unwrap();
        }
        let scene = dir.join("scene.yaml");
        std::fs::write(
            &scene,
            std::fs::read_to_string(format!("{sprite}scene-crossfade.yaml")).unwrap(),
        )
        .unwrap();
        assert_eq!(run_check_scene(game(), &scene), ExitCode::SUCCESS);
    }

    /// The `--render-out` load line names every phase that ran, omits absent ones, and ends with the
    /// unattributed share and the non-zero decode counts.
    #[test]
    fn load_summary_names_phases_share_and_decodes() {
        use ludion_protocol::perf::{AssetLoadCounters, LoadProfile};

        let counters = |kind: &str, decoded| AssetLoadCounters {
            kind: kind.to_string(),
            decoded,
            cooked: 0,
            reused: 0,
            bytes_read: 0,
        };
        let mut load = LoadProfile {
            scene_ms: 290.4,
            docs_ms: 120.0,
            mesh_ms: 830.0,
            texture_ms: 1050.0,
            other_ms: 12.0,
            gpu_upload_ms: Some(1800.0),
            first_frame_ms: Some(400.0),
            voxelize_ms: Some(300.0),
            renderer_init_ms: Some(250.0),
            gi_bake_ms: Some(600.0),
            probe_bake_ms: None,
            total_ms: 5712.0,
            assets: vec![
                counters("texture", 235),
                counters("mesh", 79),
                counters("voxel", 0),
            ],
        };
        assert_eq!(
            load_summary(&load),
            "load 5712 ms: scene 290, docs 120, mesh 830, texture 1050, other 12, voxelize 300, \
             renderer init 250, gpu upload 1800, gi bake 600, first frame 400, \
             unattributed 60 (1%); decoded 235 texture, 79 mesh"
        );

        load.assets[0].decoded = 12;
        load.assets[0].cooked = 223;
        assert!(
            load_summary(&load).ends_with("; decoded 12 texture (223 cooked), 79 mesh"),
            "{}",
            load_summary(&load)
        );

        load.gpu_upload_ms = None;
        load.gi_bake_ms = None;
        load.renderer_init_ms = None;
        load.assets.clear();
        assert_eq!(
            load_summary(&load),
            "load 5712 ms: scene 290, docs 120, mesh 830, texture 1050, other 12, voxelize 300, \
             first frame 400, unattributed 2710 (47%)"
        );
    }

    /// `--render-out` writes a PNG at the requested extent. Skips gracefully on an adapterless
    /// machine (an `L2003` in the diagnostic list), the convention every GPU test here follows.
    #[test]
    fn needs_gpu_render_out_writes_a_png_of_the_requested_size() {
        let dir = fixture("render");
        let scene = dir.join("scene.yaml");
        let out = dir.join("frame.png");

        let quality = ludion_engine::QualitySetting::full();
        match render_scene_out(
            game(),
            &scene,
            &out,
            Some(96),
            Some(64),
            0,
            &quality,
            &ludion_engine::DebugCategories::None,
        ) {
            Ok(()) => {}
            Err(diagnostics) if ludion_core::require_gpu::is_no_adapter(&diagnostics) => {
                eprintln!("skipping: no GPU adapter available");
                return;
            }
            Err(diagnostics) => panic!("{diagnostics:?}"),
        }

        let image = ludion_assets::load_png(ludion_assets::AssetRoot::content(&dir), "frame.png")
            .unwrap_or_else(|d| panic!("{d}"));
        assert_eq!((image.width, image.height), (96, 64));
    }
}
