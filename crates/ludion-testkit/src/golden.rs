//! [`GoldenTest`] — one document-vs-golden-PNG comparison, plus the bless flow.
//!
//! A golden test is `{source file, golden PNG, resolution, SSIM threshold}` — plus, for an assembly,
//! an optional clip/time/strip pose, and for a scene, an optional [`GoldenTest::frames`] tick count
//! so a mid-clip flipbook or emitter can land on a committed golden at all. The source is dispatched
//! on its `kind:` (via [`ludion_scene::peek_kind`]): a kind the table marks `model_sheet`
//! (`ludion_scene::KINDS`: `voxel`/`mesh`/`assembly`/`sculpt`) **model document** renders through
//! the engine's model-view harness ([`ludion_engine::render_model_sheet`]) as a deterministic
//! turntable contact sheet (an assembly at its rest pose, or — with [`GoldenTest::clip`] set — posed
//! at an instant or as a phase strip, which is how a *clip* gets a committed golden at all); anything
//! else is a **scene** and drives the exact same lockstep path `ludion render` uses
//! ([`ludion_engine::render_scene`]) — check → instantiate → tick(0) → extract → render
//! — or, with [`GoldenTest::with_builder`], the game's own engine
//! ([`ludion_engine::render_scene_with_builder`]), whose ticks run the game's systems so a golden can
//! be an early wave instead of the scene at rest. An engine the test built and drove itself goes
//! through [`check_engine_golden`], and a recorded session through [`GoldenTest::from_replay`].
//! Either way a golden test is never a second render pipeline to keep in sync with the real one; the
//! sheet is just the rendered image, and everything downstream is identical. Comparison and reporting
//! are [`crate::ssim`] and [`crate::report`]; this module is the glue: render, load the golden,
//! decide pass/fail/bless, and write artifacts on failure.

use std::ops::ControlFlow;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use ludion_core::codes::code;
use ludion_core::diagnostics::Diagnostic;
use ludion_ecs::Registry;
use ludion_engine::{Engine, EngineBuilder, QualitySetting};

use crate::image::{self, DecodedImage};
use crate::report::DiffReport;
use crate::ssim;

/// The default render width for a golden test (see [`crate::golden` module docs] for the rationale
/// — 960×540 keeps committed goldens light while still being large enough to localize damage
/// meaningfully across an 8×8 tile grid).
pub const DEFAULT_WIDTH: u32 = 960;
/// The default render height for a golden test.
pub const DEFAULT_HEIGHT: u32 = 540;
/// The default minimum passing SSIM: conservative for same-GPU, same-driver comparison. Hosted CI
/// on a software rasterizer (llvmpipe) will need its own, looser calibration once a CI remote
/// exists (`engine-architecture.md` §9 risk 6) — that calibration is out of scope here; this
/// constant is the local/pinned-GPU tier only.
pub const DEFAULT_SSIM_THRESHOLD: f64 = 0.99;
/// The default minimum passing SSIM of any single tile of the 8×8 grid. The global mean lets a
/// local defect (a missing HUD element or character) through, since one bad tile moves the mean by
/// under a hundredth; a tile below this floor fails the comparison regardless. The effective floor
/// is clamped to the comparison's threshold, so a looser `ssim_threshold` loosens it too.
pub const DEFAULT_TILE_FLOOR: f64 = 0.95;
/// The default ceiling on any single tile's colour error: the mean absolute R, G, B or A channel
/// difference over the tile, 0–255. SSIM scores luma only, so a hue change that keeps brightness
/// structure (a grade, a palette or channel-order change) passes it; this gate catches that. A fixed
/// constant: a looser `ssim_threshold` does not loosen colour.
pub const DEFAULT_TILE_COLOR_CEILING: f64 = 4.0;

/// One scene-vs-golden comparison.
///
/// A `fn`-pointer `registry` field (defaulting to [`ludion_engine::builtin_registry`]) is the seam a
/// *game* golden test uses: a scene naming custom components validates only against a registry that
/// knows them, so [`with_registry`](GoldenTest::with_registry) swaps in the game's. A shared
/// builder closure goes one further: [`with_builder`](GoldenTest::with_builder) renders through
/// the game's whole engine, so [`frames`](GoldenTest::frames) ticks run the game's **systems** and a
/// golden can be an early wave rather than the scene at rest. Built-in gallery tests never touch
/// either. (The closure lives in an `Arc`, so the struct stays `Clone`; `Debug` is written by hand.)
#[derive(Clone)]
pub struct GoldenTest {
    /// Path to the scene file to render.
    pub scene: PathBuf,
    /// Path to the committed golden PNG (read on a normal run, (re)written on `--bless`).
    pub golden: PathBuf,
    /// Render width in pixels.
    pub width: u32,
    /// Render height in pixels.
    pub height: u32,
    /// The minimum passing global SSIM.
    pub ssim_threshold: f64,
    /// For a `kind: assembly` source: pose it with the clip its `clips:` map binds under this name
    /// instead of rendering its rest pose. `None` (the default) is the rest pose — which is every
    /// golden that existed before clips did.
    pub clip: Option<String>,
    /// The clip instant to pose at, in seconds; `None` means `0.0`. Needs [`clip`](Self::clip).
    pub time: Option<f64>,
    /// Render this many evenly spaced clip phases side by side instead of one instant — a *strip*
    /// golden. Needs [`clip`](Self::clip).
    pub strip: Option<u32>,
    /// For an assembly, character or prefab source: render the blend of two bound clips as a strip
    /// of phases, exactly `ludion render --crossfade FROM:TO:SECONDS` (five phases unless
    /// [`strip`](Self::strip) says otherwise). Conflicts with [`clip`](Self::clip).
    pub crossfade: Option<String>,
    /// Hold each pose for this many sim ticks (stepped cadence), exactly `ludion render --hold N`.
    /// `None` is every tick.
    pub hold: Option<u32>,
    /// For a scene (not a model document): tick this many times before extracting, exactly what
    /// `ludion render --frames N` does. `0` (the default) extracts at tick 0 — every golden that
    /// existed before this field did. This is how a `Sprite`'s flipbook or a `ParticleEmitter`'s
    /// burst lands on a committed golden mid-clip instead of at rest. The schedule ticked is the
    /// built-in one, or the game's own when [`with_builder`](GoldenTest::with_builder) is set.
    pub frames: u64,
    /// The debug-draw categories drawn into the render, exactly `ludion render --debug-draw`. `None`
    /// (the default) draws nothing — every golden that existed before this field did. A category a
    /// system draws needs `frames` of at least 1; host-drawn ones (`colliders`, `hierarchy`) need the
    /// world they read to exist.
    pub debug_draw: ludion_engine::DebugCategories,
    /// A debug view the render is drawn under, exactly `ludion render --debug-view`. `None` (the
    /// default) is the normal render. Scene goldens on the built-in path only: a game builder or a
    /// model document renders without it.
    pub debug_view: Option<ludion_engine::DebugView>,
    /// The anti-aliasing mode the render runs under, exactly `ludion render --aa`. `Off` (the
    /// default) is every golden that existed before this field. Scene goldens on the built-in path
    /// only: a game builder or a model document renders without it.
    pub anti_aliasing: ludion_engine::AntiAliasing,
    /// The registry the scene is validated against — built-in by default, a game's own via
    /// [`with_registry`](GoldenTest::with_registry).
    registry: fn() -> Registry,
    /// The game builder the scene is rendered through when set, via
    /// [`with_builder`](GoldenTest::with_builder): its schedule runs for [`frames`](Self::frames)
    /// ticks, so the golden can be a wave, a midboss entry, a boss phase. `None` (the default) is
    /// the built-in schedule, which moves nothing but the sim clock.
    builder: Option<Arc<dyn Fn() -> EngineBuilder + Send + Sync>>,
    /// The recorded session this test renders a frame of, via [`GoldenTest::from_replay`]; `None`
    /// (the default) renders the `scene` file.
    replay: Option<ReplaySource>,
}

/// A replay a golden renders one frame of.
#[derive(Clone)]
struct ReplaySource {
    bytes: Arc<[u8]>,
    frame: u64,
    project_root: PathBuf,
}

impl std::fmt::Debug for GoldenTest {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("GoldenTest")
            .field("scene", &self.scene)
            .field("golden", &self.golden)
            .field("width", &self.width)
            .field("height", &self.height)
            .field("ssim_threshold", &self.ssim_threshold)
            .field("clip", &self.clip)
            .field("time", &self.time)
            .field("strip", &self.strip)
            .field("crossfade", &self.crossfade)
            .field("hold", &self.hold)
            .field("frames", &self.frames)
            .field("debug_draw", &self.debug_draw)
            .field("debug_view", &self.debug_view)
            .field("anti_aliasing", &self.anti_aliasing)
            .field("registry", &"<fn>")
            .field("builder", &self.builder.as_ref().map(|_| "<fn>"))
            .field(
                "replay",
                &self
                    .replay
                    .as_ref()
                    .map(|r| format!("frame {} of {} bytes", r.frame, r.bytes.len())),
            )
            .finish()
    }
}

/// What the golden flow reads from the environment, so the flow itself can be driven by value.
struct Env {
    platform: Result<&'static str, Diagnostic>,
    bless: bool,
    any_adapter: bool,
    require_reference: bool,
}

impl Env {
    fn current() -> Env {
        Env {
            platform: platform_dir(),
            bless: bless_requested(),
            any_adapter: any_adapter_bless(),
            require_reference: reference_adapter_required(),
        }
    }
}

/// One render of the thing under test: its image and the adapter it rendered on.
type Render<'a> = dyn FnMut() -> Result<(DecodedImage, String), Vec<Diagnostic>> + 'a;

/// How to bless a missing golden, shared by every recipe form.
const BLESS_HINT: &str = "bless it on your pinned GPU with `LUDION_BLESS=1 LUDION_BLESS_ANY_ADAPTER=1 cargo \
     test` (PowerShell: set both with `$env:`), review the written PNG, then commit it";

impl GoldenTest {
    /// Builds a golden test at the default resolution and threshold ([`DEFAULT_WIDTH`] ×
    /// [`DEFAULT_HEIGHT`], [`DEFAULT_SSIM_THRESHOLD`]), validated against the built-in component
    /// registry.
    pub fn new(scene: impl Into<PathBuf>, golden: impl Into<PathBuf>) -> GoldenTest {
        GoldenTest {
            scene: scene.into(),
            golden: golden.into(),
            width: DEFAULT_WIDTH,
            height: DEFAULT_HEIGHT,
            ssim_threshold: DEFAULT_SSIM_THRESHOLD,
            clip: None,
            time: None,
            strip: None,
            crossfade: None,
            hold: None,
            frames: 0,
            debug_draw: ludion_engine::DebugCategories::None,
            debug_view: None,
            anti_aliasing: ludion_engine::AntiAliasing::Off,
            registry: ludion_engine::builtin_registry,
            builder: None,
            replay: None,
        }
    }

    /// A golden at `goldens_root/<os>/<stem>.png`, the per-OS set path every game recipe uses
    /// (`<os>` is [`platform_dir`], or the host OS name off the pinned tier, where
    /// [`check`](GoldenTest::check) skips before reading it). Chain the builders, then
    /// [`check`](GoldenTest::check).
    pub fn in_set(
        scene: impl Into<PathBuf>,
        goldens_root: impl AsRef<Path>,
        stem: &str,
    ) -> GoldenTest {
        GoldenTest::new(scene, set_golden(goldens_root.as_ref(), stem))
    }

    /// A golden of frame `frame` of a recorded `.lrp` session (`bytes`), driven against the engine
    /// of [`with_builder`](GoldenTest::with_builder) (the built-in one when unset) rooted at
    /// `project_root`, at `goldens_root/<os>/<stem>.png`. Frame 0 is the initial post-load world;
    /// the numbering is [`ludion_engine::ReplayFramesRequest`]'s. A frame past the replay's end is
    /// `L3021`. [`frames`](GoldenTest::frames), [`clip`](GoldenTest::clip),
    /// [`debug_view`](GoldenTest::debug_view), [`debug_draw`](GoldenTest::debug_draw),
    /// [`anti_aliasing`](GoldenTest::anti_aliasing) and
    /// [`with_registry`](GoldenTest::with_registry) do not apply to a replay. Compare with
    /// [`check`](GoldenTest::check).
    pub fn from_replay(
        bytes: &[u8],
        frame: u64,
        project_root: impl Into<PathBuf>,
        goldens_root: impl AsRef<Path>,
        stem: &str,
    ) -> GoldenTest {
        let mut test = GoldenTest::in_set(PathBuf::new(), goldens_root, stem);
        test.replay = Some(ReplaySource {
            bytes: Arc::from(bytes),
            frame,
            project_root: project_root.into(),
        });
        test
    }

    /// Draws `categories` into the render. See [`debug_draw`](GoldenTest::debug_draw).
    #[must_use]
    pub fn with_debug_draw(mut self, categories: ludion_engine::DebugCategories) -> Self {
        self.debug_draw = categories;
        self
    }

    /// Renders the scene under `view`. See [`debug_view`](GoldenTest::debug_view).
    #[must_use]
    pub fn with_debug_view(mut self, view: Option<ludion_engine::DebugView>) -> Self {
        self.debug_view = view;
        self
    }

    /// Renders the scene under `mode`. See [`anti_aliasing`](GoldenTest::anti_aliasing).
    #[must_use]
    pub fn with_anti_aliasing(mut self, mode: ludion_engine::AntiAliasing) -> Self {
        self.anti_aliasing = mode;
        self
    }

    /// Validates the scene against `registry` (built-ins plus a game's own components) instead of the
    /// built-in registry alone — the seam a game golden test needs, since its scene names components
    /// the built-in registry would reject with `L1013`. `registry` is a `fn` so it stays `Clone`; pass
    /// a game's `fn() -> Registry` that returns the built-ins with its components registered on top.
    #[must_use]
    pub fn with_registry(mut self, registry: fn() -> Registry) -> Self {
        self.registry = registry;
        self
    }

    /// Renders the scene through `builder`'s own engine, running the **game's** schedule for
    /// [`frames`](Self::frames) ticks — the seam a golden of an early wave, a midboss entry or a boss
    /// phase needs, since the built-in schedule's frame N is frame 0 with nothing but the sim clock
    /// moved. `builder` is any `Fn() -> EngineBuilder` (a closure capturing config included); pass
    /// the game's own (the same one its `main` hands to `ludion::run`).
    ///
    /// This supersedes [`with_registry`](GoldenTest::with_registry): when both are set the builder
    /// wins, because its engine already validates against the game's registry. Scripted input is
    /// [`from_replay`](GoldenTest::from_replay)'s job.
    ///
    /// ```no_run
    /// # use ludion_testkit::GoldenTest;
    /// # fn game() -> ludion_engine::EngineBuilder { ludion_engine::Engine::builder() }
    /// GoldenTest::in_set("scenes/stage-1.yaml", "goldens", "wave-1")
    ///     .with_builder(game)
    ///     .with_frames(600);
    /// ```
    #[must_use]
    pub fn with_builder(
        mut self,
        builder: impl Fn() -> EngineBuilder + Send + Sync + 'static,
    ) -> Self {
        self.builder = Some(Arc::new(builder));
        self
    }

    /// Ticks the schedule `frames` times before extracting, instead of comparing the scene at rest
    /// (tick 0) — the built-in schedule, or the game's own when
    /// [`with_builder`](GoldenTest::with_builder) is set. See [`GoldenTest::frames`].
    #[must_use]
    pub fn with_frames(mut self, frames: u64) -> Self {
        self.frames = frames;
        self
    }

    /// Renders the scene and compares it against the committed golden, writing diagnostic
    /// artifacts (`<stem>.actual.png`, `<stem>.diff.json`) into `out_dir` **on failure only** — a
    /// passing test leaves no trace beyond its return value. `out_dir` must not be the goldens
    /// directory itself (callers pick a scratch/build directory; see `ludion-cli`'s `test` command
    /// for the convention it uses).
    ///
    /// # Errors
    /// A `Vec<Diagnostic>` if the scene fails to render (the same diagnostics `ludion render` would
    /// print) or if the golden cannot be read (`L2201`/`L2204` — see [`GoldenTest::bless`] to create
    /// it), or — for a byte-exact threshold on the host's per-OS golden set — `L2205` (unpinned
    /// platform) / `L2210` (not the reference adapter), both a skip per [`unverifiable`].
    pub fn run(&self, out_dir: &Path) -> Result<GoldenOutcome, Vec<Diagnostic>> {
        run_with(
            &self.golden,
            self.ssim_threshold,
            out_dir,
            &Env::current(),
            &mut || self.render(),
        )
    }

    /// Renders the scene and (re)writes the golden PNG unconditionally — creating a missing golden
    /// counts as blessing it. Review the written image before committing it: blessing records
    /// "this is what the renderer currently produces," not "this is correct."
    ///
    /// # Errors
    /// A `Vec<Diagnostic>` if the scene fails to render, an IO diagnostic if the golden cannot be
    /// written, or `L2206` when the golden sits in the host platform's `goldens/<os>/` directory and
    /// the rendering GPU is not that platform's pinned reference adapter (opt out with
    /// `LUDION_BLESS_ANY_ADAPTER=1`).
    pub fn bless(&self) -> Result<GoldenOutcome, Vec<Diagnostic>> {
        bless_with(&self.golden, &Env::current(), &mut || self.render())
    }

    /// The whole game-project golden-test recipe in one call: resolve the per-OS golden path
    /// (`goldens_root/<os>/<stem>.png`), skip gracefully off the pinned tier (`L2205`) or while
    /// `goldens_root` holds no golden at all (a missing golden in a set that has some is `L2201`,
    /// see [`missing_golden`]), bless when [`bless_requested`] says to, otherwise render and
    /// compare — panicking with the diff summary on a mismatch, and skipping gracefully on the no-adapter
    /// signal (`L2003`). This is the entire body every game's `tests/golden.rs` used to hand-roll
    /// per scene; it is exactly
    /// `GoldenTest::in_set(..).with_registry(registry).check(out_dir)`, and see the
    /// [crate docs](crate) for a runnable game-project example.
    ///
    /// # Panics
    /// As [`check`](GoldenTest::check).
    ///
    /// # Example
    /// ```no_run
    /// use ludion_ecs::Registry;
    /// use ludion_engine::builtin_registry;
    /// use ludion_testkit::GoldenTest;
    ///
    /// #[test]
    /// fn main_scene_matches_its_golden() {
    ///     GoldenTest::single(
    ///         "scenes/main.yaml",
    ///         "goldens",
    ///         "main",
    ///         builtin_registry as fn() -> Registry, // a game passes its own `registry` fn here
    ///         std::path::Path::new("target/testkit"),
    ///     );
    /// }
    /// # fn main() {}
    /// ```
    pub fn single(
        scene: impl Into<PathBuf>,
        goldens_root: impl AsRef<Path>,
        stem: &str,
        registry: fn() -> Registry,
        out_dir: &Path,
    ) {
        GoldenTest::in_set(scene, goldens_root, stem)
            .with_registry(registry)
            .check(out_dir);
    }

    /// The golden recipe for this test: skip off the pinned tier (`L2205`), bless when
    /// [`bless_requested`] says to, skip while the golden set is empty, otherwise render and compare —
    /// panicking with the diff summary on a mismatch and skipping gracefully on the no-adapter
    /// signal (`L2003`). Every game entry point ([`single`](GoldenTest::single),
    /// [`from_replay`](GoldenTest::from_replay), [`check_engine_golden`]) runs this one flow, so its
    /// adapter guards (`L2206` on bless, `L2210` on a byte-exact compare) cannot be bypassed.
    /// Valid on any `GoldenTest`, whatever built it.
    ///
    /// # Panics
    /// If an asked-for bless fails (including no GPU adapter — a bless must never skip), or if the
    /// comparison fails (either a genuine mismatch, or a render diagnostic other than a skip).
    pub fn check(&self, out_dir: &Path) {
        check_with(
            &self.golden,
            self.ssim_threshold,
            out_dir,
            &Env::current(),
            &mut || self.render(),
        );
    }

    /// Renders the source at this test's configured resolution, dispatching on its `kind:`: a
    /// kind the table marks `model_sheet` (`ludion_scene::KINDS`) renders as a contact sheet
    /// ([`ludion_engine::render_model_sheet`], all six views at `width`×`height`, or the posed
    /// sheet/strip [`clip`](Self::clip) asks for), everything else as
    /// a scene — through the game's own engine when [`with_builder`](Self::with_builder) set one
    /// ([`ludion_engine::render_scene_with_builder`], whose `frames` ticks run the game's systems),
    /// otherwise through the built-in lockstep path. The underlying engine's own diagnostics (a
    /// broken document, a missing texture, no GPU adapter) propagate unchanged — a golden test never
    /// masks a render failure as a comparison failure. A source that cannot be read, or one with no
    /// `kind:`, takes the scene path, which then surfaces any read/parse error with its own
    /// diagnostic.
    ///
    /// Returns the decoded image alongside the human-readable adapter it rendered on (e.g.
    /// `"NVIDIA GeForce RTX 3090 (Vulkan)"`), so a caller can tell a provenance-shaped failure from
    /// a regression-shaped one ([`provenance_label`]).
    fn render(&self) -> Result<(DecodedImage, String), Vec<Diagnostic>> {
        if let Some(replay) = &self.replay {
            return self.render_replay(replay);
        }
        let kind = std::fs::read_to_string(&self.scene)
            .ok()
            .and_then(|source| ludion_scene::peek_kind(&source));
        let (image, adapter) = match (kind.as_deref(), self.builder.as_deref()) {
            (Some(kind), _)
                if ludion_scene::kind_entry(kind).is_some_and(|entry| entry.model_sheet) =>
            {
                let crossfade = self
                    .crossfade
                    .as_deref()
                    .map(ludion_engine::parse_crossfade)
                    .transpose()
                    .map_err(|d| vec![d])?;
                let outcome =
                    ludion_engine::render_model_sheet(&ludion_engine::ModelSheetRequest {
                        clip: self.clip.as_deref(),
                        time: self.time,
                        strip: self.strip,
                        crossfade,
                        hold: self.hold,
                        ..ludion_engine::ModelSheetRequest::new(
                            &[self.scene.as_path()],
                            self.width,
                            self.height,
                        )
                    })?;
                (outcome.image, outcome.adapter)
            }
            // A game builder renders through the game's whole engine, so the `frames` ticks run its
            // systems where the built-in path below can only move the sim clock. Its frame arrives
            // PNG-encoded (the one render core every live screenshot shares), decoded here with the
            // same decoder the golden itself is read through.
            (_, Some(builder)) => {
                let outcome = ludion_engine::render_scene_with_builder(
                    builder(),
                    &self.scene,
                    Some(self.width),
                    Some(self.height),
                    // A golden is blessed at full quality — see the note on the built-in arm below.
                    &ludion_engine::QualitySetting::full(),
                    self.frames,
                    &self.debug_draw,
                )?;
                let decoded = image::decode_png(&outcome.png).map_err(|d| vec![d])?;
                return Ok((decoded, outcome.adapter));
            }
            (_, None) => {
                let registry = (self.registry)();
                let outcome = ludion_engine::render_scene_with_registry(
                    &registry,
                    &self.scene,
                    ludion_engine::FrameSize::native(self.width, self.height),
                    // A golden is blessed at full quality: this FULL *is* the pin, so a machine's
                    // own quality setting can never move a committed golden — neither its effect
                    // toggles nor the sample counts they run at.
                    &ludion_engine::QualitySetting::full(),
                    false,
                    &[],
                    self.frames,
                    &self.debug_draw,
                    self.debug_view,
                    self.anti_aliasing,
                )?;
                (outcome.image, outcome.adapter)
            }
        };
        Ok((
            DecodedImage {
                width: image.width,
                height: image.height,
                rgba: image.rgba,
            },
            adapter,
        ))
    }

    /// Drives `replay` to its frame and renders the engine there, [`check_engine_golden`]'s render.
    fn render_replay(
        &self,
        replay: &ReplaySource,
    ) -> Result<(DecodedImage, String), Vec<Diagnostic>> {
        let builder = self
            .builder
            .as_deref()
            .map_or_else(Engine::builder, |b| b());
        let mut rendered = None;
        let mut last = 0;
        ludion_engine::replay_with(
            builder,
            &replay.bytes,
            &replay.project_root,
            |engine, frame| {
                last = frame.as_u64();
                if last != replay.frame {
                    return Ok(ControlFlow::Continue(()));
                }
                rendered = Some(render_engine(engine, self.width, self.height));
                Ok(ControlFlow::Break(()))
            },
        )
        .map_err(|d| vec![d])?;
        rendered.unwrap_or_else(|| {
            Err(vec![
                Diagnostic::new(
                    code::REPLAY_FRAME_WINDOW,
                    format!(
                        "the golden asked for frame {}, but this replay records frames 0..={last}",
                        replay.frame
                    ),
                )
                .with_help(format!("golden a frame in 0..={last}")),
            ])
        })
    }
}

/// `goldens_root/<os>/<stem>.png`: the golden every recipe form resolves. Off the pinned tier the
/// directory is the host OS name, which the flow skips before reading.
fn set_golden(goldens_root: &Path, stem: &str) -> PathBuf {
    goldens_root
        .join(platform_dir().unwrap_or(std::env::consts::OS))
        .join(format!("{stem}.png"))
}

/// Renders `engine` as it stands, at full quality, and names the adapter — the render every
/// engine-driven golden shares. The engine's quality is left at full.
fn render_engine(
    engine: &mut Engine,
    width: u32,
    height: u32,
) -> Result<(DecodedImage, String), Vec<Diagnostic>> {
    engine.set_quality(QualitySetting::full());
    let png = engine.render_png(width, height).map_err(|d| vec![d])?;
    let image = image::decode_png(&png).map_err(|d| vec![d])?;
    let adapter = engine.adapter_label().map_err(|d| vec![d])?;
    Ok((image, adapter))
}

/// The golden recipe for an engine the test built and drove itself — a menu opened by injected
/// input, a state reached through the game's own API: renders it as it stands, at full quality and
/// the default resolution ([`DEFAULT_WIDTH`] × [`DEFAULT_HEIGHT`]), against
/// `goldens_root/<os>/<stem>.png`, with every guard [`GoldenTest::check`] has. The engine is not
/// stepped, and its quality is left at full.
///
/// # Panics
/// As [`GoldenTest::check`].
///
/// # Example
/// ```no_run
/// # use ludion_engine::Engine;
/// # fn open_menu(_: &mut Engine) {}
/// let mut engine = Engine::new();
/// open_menu(&mut engine);
/// ludion_testkit::check_engine_golden(
///     &mut engine,
///     "goldens",
///     "menu-open",
///     std::path::Path::new("target/testkit"),
/// );
/// ```
pub fn check_engine_golden(
    engine: &mut Engine,
    goldens_root: impl AsRef<Path>,
    stem: &str,
    out_dir: &Path,
) {
    check_with(
        &set_golden(goldens_root.as_ref(), stem),
        DEFAULT_SSIM_THRESHOLD,
        out_dir,
        &Env::current(),
        &mut || render_engine(engine, DEFAULT_WIDTH, DEFAULT_HEIGHT),
    );
}

/// The compare flow: skip when `golden` has no pinned tier to compare on, `render` and compare
/// against it, write `<stem>.actual.png` / `<stem>.diff.json` to `out_dir` on a failure only.
fn run_with(
    golden: &Path,
    threshold: f64,
    out_dir: &Path,
    env: &Env,
    render: &mut Render,
) -> Result<GoldenOutcome, Vec<Diagnostic>> {
    let skip = |adapter: Option<&str>| {
        compare_skip(
            golden,
            threshold,
            env.platform.clone(),
            adapter,
            env.any_adapter,
        )
    };
    if let Some(diag) = skip(None) {
        return Err(vec![diag]);
    }
    let (actual, adapter) = render()?;
    if let Some(diag) = skip(Some(&adapter)) {
        return Err(vec![refuse_adapter_skip(diag, env.require_reference)]);
    }
    let expected = image::read_golden(golden).map_err(|d| vec![d])?;

    let stem = test_stem(golden);
    let actual_path = out_dir.join(format!("{stem}.actual.png"));
    let report_path = out_dir.join(format!("{stem}.diff.json"));

    let report = report_for(
        &expected,
        &actual,
        &golden.display().to_string(),
        &actual_path.display().to_string(),
        threshold,
        Some(&adapter),
    );

    if report.pass {
        return Ok(GoldenOutcome::Passed { report });
    }

    std::fs::create_dir_all(out_dir).map_err(|error| vec![io_diagnostic(out_dir, error)])?;
    write_png(&actual_path, &actual)?;
    ludion_core::atomic::write_atomic(&report_path, report.to_json())
        .map_err(|error| vec![io_diagnostic(&report_path, error)])?;

    Ok(GoldenOutcome::Failed {
        report,
        actual_path,
        report_path,
    })
}

/// The bless flow: `render`, refuse (`L2206`) a pinned-set golden from a non-reference adapter, and
/// write `golden`.
fn bless_with(
    golden: &Path,
    env: &Env,
    render: &mut Render,
) -> Result<GoldenOutcome, Vec<Diagnostic>> {
    let (actual, adapter) = render()?;
    if let Ok(platform) = &env.platform
        && let Some(refusal) = bless_refusal(golden, platform, &adapter, env.any_adapter)
    {
        return Err(vec![refusal]);
    }
    if let Some(parent) = golden.parent() {
        std::fs::create_dir_all(parent).map_err(|error| vec![io_diagnostic(parent, error)])?;
    }
    write_png(golden, &actual)?;
    Ok(GoldenOutcome::Blessed {
        golden: golden.to_path_buf(),
    })
}

/// The whole recipe behind [`GoldenTest::check`], [`GoldenTest::single`] and
/// [`check_engine_golden`]; see [`GoldenTest::check`].
fn check_with(golden: &Path, threshold: f64, out_dir: &Path, env: &Env, render: &mut Render) {
    if let Err(diag) = &env.platform {
        eprintln!("SKIPPED golden test:\n{diag}");
        return;
    }

    if env.bless {
        match bless_with(golden, env, render) {
            Ok(_) => {
                eprintln!("blessed {} — review it, then commit it", golden.display());
                return;
            }
            Err(diags) => panic!("blessing the golden failed: {diags:?}"),
        }
    }

    if !golden.exists() {
        missing_golden(golden, BLESS_HINT);
        return;
    }

    match run_with(golden, threshold, out_dir, env, render) {
        // The score is printed on a PASS too: a golden can sit inside the tolerance a long way
        // from the scene, and the worst tile is what says how much floor margin remains.
        Ok(GoldenOutcome::Passed { report }) => eprintln!("golden passed: {}", report.summary),
        Ok(GoldenOutcome::Blessed { .. }) => {}
        Ok(GoldenOutcome::Failed { report, .. }) => match &report.provenance {
            Some(provenance) => {
                panic!("golden comparison failed: {}\n{provenance}", report.summary)
            }
            None => panic!("golden comparison failed: {}", report.summary),
        },
        Err(diags) if ludion_core::require_gpu::is_no_adapter(&diags) => {
            eprintln!("SKIPPED golden test: no GPU adapter (L2003)");
        }
        Err(diags) => match unverifiable(&diags) {
            Some(skip) => eprintln!("SKIPPED golden test:\n{skip}"),
            None => panic!("rendering the golden scene failed: {diags:?}"),
        },
    }
}

/// The per-OS goldens subdirectory for the host platform: `"windows"` or `"macos"`, the two pinned
/// GPU targets. Bit-exact goldens only hold per (OS, GPU), so each pinned platform keeps its own
/// blessed set under `goldens/<os>/`; callers join this onto their `goldens/` root (the suite
/// discovery in [`crate::suite`] does it automatically).
///
/// # Errors
/// `L2205` on any other OS — there is no pinned golden set to compare against there. Game golden
/// tests and in-repo suites treat that as a documented skip (print the diagnostic, pass).
pub fn platform_dir() -> Result<&'static str, Diagnostic> {
    match std::env::consts::OS {
        "windows" => Ok("windows"),
        "macos" => Ok("macos"),
        other => Err(Diagnostic::new(
            code::RENDER_GOLDEN_PLATFORM_UNPINNED,
            format!(
                "`{other}` has no pinned golden tier — bit-exact goldens are blessed per-OS, and \
                 only the two pinned platforms (`windows`, `macos`) have committed sets"
            ),
        )
        .with_help(
            "goldens live in per-OS subdirectories: `goldens/windows/<stem>.png` and \
             `goldens/macos/<stem>.png`. Run golden tests on a pinned platform, or bless a set for \
             this one on its pinned GPU (`ludion test --bless`; game projects: `LUDION_BLESS=1 \
             LUDION_BLESS_ANY_ADAPTER=1 cargo test`) and commit it",
        )),
    }
}

/// The adapter substring naming the pinned Windows reference GPU (`goldens/windows/`): every
/// bit-exact Windows golden was blessed on this card.
pub const WINDOWS_REFERENCE_ADAPTER: &str = "RTX 3090";
/// The adapter substring naming the pinned macOS reference GPU (`goldens/macos/`): every bit-exact
/// macOS golden was blessed on this chip.
pub const MACOS_REFERENCE_ADAPTER: &str = "Apple M4";

/// The pinned reference-adapter substring for `platform` (`"windows"`/`"macos"`), or `None` for a
/// platform with no pinned tier.
pub fn reference_adapter(platform: &str) -> Option<&'static str> {
    match platform {
        "windows" => Some(WINDOWS_REFERENCE_ADAPTER),
        "macos" => Some(MACOS_REFERENCE_ADAPTER),
        _ => None,
    }
}

/// Whether `LUDION_BLESS_ANY_ADAPTER` opts out of the reference-adapter check in
/// [`GoldenTest::bless`]: set to anything other than empty or `0`. For a game project whose pinned
/// GPU is its own card.
fn any_adapter_bless() -> bool {
    ludion_core::bless::switch_set(ludion_core::bless::BLESS_ANY_ADAPTER_ENV)
}

/// The `L2206` refusal for blessing `golden` from `adapter`, or `None` when the bless may proceed.
///
/// Refuses only a golden whose parent directory is `platform`'s per-OS set (`goldens/<os>/`) while
/// `adapter` does not contain that platform's reference adapter, unless `any_adapter` opts out. A
/// golden outside a per-OS directory (a scratch or single-file golden) is never refused.
fn bless_refusal(
    golden: &Path,
    platform: &str,
    adapter: &str,
    any_adapter: bool,
) -> Option<Diagnostic> {
    let in_pinned_dir = golden
        .parent()
        .and_then(Path::file_name)
        .is_some_and(|name| name == platform);
    let reference = reference_adapter(platform)?;
    if !in_pinned_dir || any_adapter || adapter.contains(reference) {
        return None;
    }
    Some(
        Diagnostic::new(
            code::RENDER_GOLDEN_BLESS_ADAPTER_UNPINNED,
            format!(
                "refusing to bless `{}`: it rendered on {adapter}, not the pinned {reference} for \
                 `goldens/{platform}/`",
                golden.display()
            ),
        )
        .with_help(
            "bless on the pinned reference machine; a game project whose pinned GPU is its own \
             card sets `LUDION_BLESS_ANY_ADAPTER=1` beside `LUDION_BLESS=1`",
        ),
    )
}

/// The environment variable that turns an `L2210` skip into a panic: set on a box that carries the
/// reference GPU, where wgpu picking another adapter means the box is misconfigured, not that the
/// comparison is optional (the adapter-side twin of `LUDION_REQUIRE_GPU`).
const REQUIRE_REFERENCE_ADAPTER_ENV: &str = "LUDION_REQUIRE_REFERENCE_ADAPTER";

fn reference_adapter_required() -> bool {
    ludion_core::bless::switch_set(REQUIRE_REFERENCE_ADAPTER_ENV)
}

/// Passes `skip` through, or panics when it is `L2210` and `required`.
fn refuse_adapter_skip(skip: Diagnostic, required: bool) -> Diagnostic {
    assert!(
        !(required && skip.code == code::RENDER_GOLDEN_COMPARE_ADAPTER_UNPINNED),
        "{REQUIRE_REFERENCE_ADAPTER_ENV} is set but the byte-exact comparison cannot run: {}",
        skip.message
    );
    skip
}

/// The skip diagnostic for an RGBA-identity comparison (`threshold >= 1.0`) of `golden` that cannot be
/// verified here, or `None` when the comparison should run.
///
/// Applies only to a golden whose parent directory is the host's per-OS set (`goldens/<os>/`), unless
/// `any_adapter` opts out. An unpinned `platform` is `L2205`; once the render is known (`adapter` is
/// `Some`), an adapter that does not contain the platform's reference adapter is `L2210`. Call it with
/// `adapter: None` before rendering and again with the adapter after.
fn compare_skip(
    golden: &Path,
    threshold: f64,
    platform: Result<&'static str, Diagnostic>,
    adapter: Option<&str>,
    any_adapter: bool,
) -> Option<Diagnostic> {
    let in_host_set = golden
        .parent()
        .and_then(Path::file_name)
        .is_some_and(|name| name == std::env::consts::OS);
    if threshold < 1.0 || any_adapter || !in_host_set {
        return None;
    }
    let platform = match platform {
        Ok(platform) => platform,
        Err(unpinned) => return Some(unpinned),
    };
    let adapter = adapter?;
    let reference = reference_adapter(platform)?;
    if adapter.contains(reference) {
        return None;
    }
    Some(
        Diagnostic::new(
            code::RENDER_GOLDEN_COMPARE_ADAPTER_UNPINNED,
            format!(
                "skipping the byte-exact comparison against `{}`: it rendered on {adapter}, not the \
                 pinned {reference} for `goldens/{platform}/`",
                golden.display()
            ),
        )
        .with_help(
            "bit-exact goldens hold only on the pinned reference machine; a project whose pinned \
             GPU is its own card sets `LUDION_BLESS_ANY_ADAPTER=1` to compare anyway",
        ),
    )
}

/// The diagnostic that makes a test a documented skip rather than a failure: an unpinned platform
/// (`L2205`), a byte-exact comparison off the reference adapter (`L2210`), or — via
/// [`ludion_core::require_gpu::is_no_adapter`], which panics under `LUDION_REQUIRE_GPU` — a missing
/// GPU adapter (`L2003`). `None` means the failure is real.
pub fn unverifiable(diags: &[Diagnostic]) -> Option<&Diagnostic> {
    let skip = diags.iter().find(|d| {
        d.code == code::RENDER_GOLDEN_PLATFORM_UNPINNED
            || d.code == code::RENDER_GOLDEN_COMPARE_ADAPTER_UNPINNED
    });
    skip.or_else(|| {
        ludion_core::require_gpu::is_no_adapter(diags)
            .then(|| diags.iter().find(|d| d.code == code::RENDER_NO_ADAPTER))
            .flatten()
    })
}

/// Classifies a golden failure by where it rendered, so a red is never read as a regression on
/// nothing but the SSIM score alone — the mechanical backstop for the "known laptop reds" habit.
/// `platform` is [`platform_dir`]'s `"windows"`/`"macos"`; `adapter` is the human-readable string a
/// render outcome carries (e.g. `"NVIDIA GeForce RTX 3090 (Vulkan)"`). A red on the pinned reference
/// adapter for the current platform is a regression; anywhere else it is environment until re-run
/// there.
pub fn provenance_label(adapter: &str, platform: &str) -> String {
    let Some(reference) = reference_adapter(platform) else {
        return format!(
            "provenance: rendered on {adapter}, `{platform}` has no pinned reference adapter \
             — a red here is environment until run on a pinned platform"
        );
    };
    if adapter.contains(reference) {
        format!("provenance: rendered on the pinned {reference} — this is a regression")
    } else {
        format!(
            "provenance: rendered on {adapter}, not the pinned {reference} — a red here is \
             environment until the reference box rules"
        )
    }
}

/// Whether the current test run was asked to (re)bless goldens instead of comparing: true when the
/// `LUDION_BLESS` environment variable is set to anything other than empty or `0`.
///
/// This is the *game-project* bless convention — `LUDION_BLESS=1 cargo test` (PowerShell:
/// `$env:LUDION_BLESS='1'; cargo test`). A game's golden test must bless in its own process, because
/// only the game knows its component registry ([`GoldenTest::with_registry`]); the CLI's
/// `ludion test --bless` directory suite validates against the built-in registry alone, so it cannot
/// bless a scene that names game components. Mirrors the engine's own `BLESS_SCHEMA=1` snapshot flow.
/// Blessing when asked should be unconditional and loud: callers should treat a bless failure
/// (including no GPU adapter, `L2003`) as a hard test failure, never a skip.
pub fn bless_requested() -> bool {
    ludion_core::bless::bless_requested()
}

/// What a golden recipe does when `golden` (`…/goldens/<os>/<stem>.png`) does not exist: panics with
/// `L2201` when the set is live — some per-OS subdirectory of the goldens root already holds a
/// `.png`, so this one was deleted or never blessed — otherwise prints a loud skip and returns, which
/// keeps a fresh `ludion new` project green until its first golden is committed. `bless_hint` says
/// how to bless one.
///
/// # Panics
/// With the `L2201` diagnostic when the golden set is live.
pub fn missing_golden(golden: &Path, bless_hint: &str) {
    let live = golden
        .parent()
        .and_then(Path::parent)
        .is_some_and(golden_set_is_live);
    if live {
        let diag = Diagnostic::new(
            code::RENDER_GOLDEN_MISSING,
            format!(
                "no golden at {}, but the golden set already has committed images — this one was \
                 deleted or never blessed",
                golden.display()
            ),
        )
        .with_help(bless_hint.to_string());
        panic!("{diag}");
    }
    eprintln!(
        "no golden yet at {} — {bless_hint}. Skipping the comparison until the first golden is \
         committed.",
        golden.display()
    );
}

/// Whether any per-OS subdirectory of `root` holds a committed `.png`. An unreadable or missing
/// `root` is not live.
fn golden_set_is_live(root: &Path) -> bool {
    let Ok(entries) = std::fs::read_dir(root) else {
        return false;
    };
    entries.flatten().any(|entry| {
        std::fs::read_dir(entry.path()).is_ok_and(|files| {
            files
                .flatten()
                .any(|file| file.path().extension().is_some_and(|ext| ext == "png"))
        })
    })
}

/// Compares two already-encoded PNG images at `threshold`, producing the same [`DiffReport`] a
/// file-based [`GoldenTest`] would — but for a candidate that did **not** come from an in-process
/// render.
///
/// This is the entry point for callers whose candidate image arrives as raw bytes from somewhere
/// other than [`ludion_engine::render_scene`] — most importantly a `render.screenshot` fetched over
/// the protocol (`ludion-server`/`ludion-client`), where the pixels never touch disk. It decodes
/// both buffers with the same decoder `image::read_golden` uses and runs the identical SSIM
/// comparison and localization [`GoldenTest::run`] runs, so a wire-driven golden check is the *same*
/// comparison as the in-process one, never a second path to keep in sync.
///
/// # Errors
/// `L2204` if either buffer is not a decodable PNG.
pub fn compare_png_bytes(
    golden_png: &[u8],
    actual_png: &[u8],
    threshold: f64,
) -> Result<DiffReport, Diagnostic> {
    let golden = image::decode_png(golden_png)?;
    let actual = image::decode_png(actual_png)?;
    Ok(report_for(
        &golden,
        &actual,
        "<golden png>",
        "<wire screenshot>",
        threshold,
        // The candidate arrived over the wire from whatever process rendered it — never this
        // process's own render — so there is no adapter here to classify against.
        None,
    ))
}

/// The one place a decoded golden/actual pair becomes a [`DiffReport`]: a dimension mismatch when the
/// sizes differ, otherwise a full tiled SSIM comparison; at `threshold >= 1.0` the pair must also be
/// RGBA-identical. Shared by [`GoldenTest::run`] (file-based)
/// and [`compare_png_bytes`] (bytes-based) so there is exactly one comparison-decision path.
/// `adapter`, when known, becomes the report's [`DiffReport::provenance`] line.
fn report_for(
    golden: &DecodedImage,
    actual: &DecodedImage,
    golden_label: &str,
    actual_label: &str,
    threshold: f64,
    adapter: Option<&str>,
) -> DiffReport {
    let tile_floor = DEFAULT_TILE_FLOOR.min(threshold);
    let mut report = if (golden.width, golden.height) != (actual.width, actual.height) {
        DiffReport::dimension_mismatch(
            golden_label,
            actual_label,
            (golden.width, golden.height),
            (actual.width, actual.height),
            threshold,
            tile_floor,
            DEFAULT_TILE_COLOR_CEILING,
        )
    } else {
        let comparison = ssim::compare(golden, actual);
        DiffReport::from_comparison(
            golden_label,
            actual_label,
            actual.width,
            actual.height,
            threshold,
            tile_floor,
            DEFAULT_TILE_COLOR_CEILING,
            &comparison,
        )
    };
    if (golden.width, golden.height) == (actual.width, actual.height)
        && threshold >= 1.0
        && report.pass
        && golden.rgba != actual.rgba
    {
        report.pass = false;
        report.summary = identity_failure_summary(golden, actual, report.mae);
    }
    report.provenance = adapter.map(|a| provenance_label(a, std::env::consts::OS));
    report
}

/// The `FAIL:` line for a pair that clears SSIM and colour at threshold 1.0 but is not RGBA-identical.
/// One pass over the pixel pairs; `golden` and `actual` have equal dimensions.
fn identity_failure_summary(
    golden: &DecodedImage,
    actual: &DecodedImage,
    mae: Option<f64>,
) -> String {
    let width = golden.width.max(1) as usize;
    let mut differing = 0usize;
    let mut alpha_only = 0usize;
    let mut first: Option<(usize, &[u8], &[u8])> = None;
    for (index, (g, a)) in golden
        .rgba
        .chunks_exact(4)
        .zip(actual.rgba.chunks_exact(4))
        .enumerate()
    {
        if g == a {
            continue;
        }
        differing += 1;
        if g[..3] == a[..3] {
            alpha_only += 1;
        }
        first.get_or_insert((index, g, a));
    }
    let (index, g, a) = first.unwrap_or((0, &[0; 4], &[0; 4]));
    format!(
        "FAIL: ssim and colour pass but the image is not byte-identical at threshold 1.0 (RGBA \
         identity): {differing} differing pixel(s), {alpha_only} in alpha only; first at ({}, {}) \
         golden rgba {:?} vs actual rgba {:?}; mae {:.6}",
        index % width,
        index / width,
        g,
        a,
        mae.unwrap_or(0.0)
    )
}

/// The result of running (or blessing) one [`GoldenTest`].
pub enum GoldenOutcome {
    /// The golden was (re)written; nothing was compared.
    Blessed {
        /// Where the golden was written.
        golden: PathBuf,
    },
    /// The render matched its golden within threshold. No artifacts were written.
    Passed {
        /// The full comparison report (for callers that want the SSIM value even on a pass).
        report: DiffReport,
    },
    /// The render did not match its golden. `actual_path`/`report_path` name the artifacts written
    /// next to (but not inside) the goldens directory. `report.provenance` (see [`provenance_label`])
    /// names the adapter the failing render ran on.
    Failed {
        /// The full comparison report.
        report: DiffReport,
        /// Where the freshly rendered "actual" PNG was written.
        actual_path: PathBuf,
        /// Where the JSON diff report was written.
        report_path: PathBuf,
    },
}

impl GoldenOutcome {
    /// Whether this outcome represents a passing (or blessed) test.
    pub fn is_ok(&self) -> bool {
        !matches!(self, GoldenOutcome::Failed { .. })
    }

    /// The global SSIM score, when one was computed (`None` for `Blessed` or a dimension
    /// mismatch).
    pub fn ssim(&self) -> Option<f64> {
        match self {
            GoldenOutcome::Blessed { .. } => None,
            GoldenOutcome::Passed { report } | GoldenOutcome::Failed { report, .. } => report.ssim,
        }
    }
}

/// The golden's file stem (e.g. `sprites` for `sprites.png`), used to name sibling artifacts.
fn test_stem(golden: &Path) -> String {
    golden
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| "golden".to_string())
}

/// Encodes and writes `image` as a PNG at `path` (used for both the golden and the `.actual.png`
/// artifact — the same deterministic encoder either way).
fn write_png(path: &Path, image: &DecodedImage) -> Result<(), Vec<Diagnostic>> {
    let mut bytes = Vec::new();
    {
        let mut encoder = png::Encoder::new(&mut bytes, image.width, image.height);
        encoder.set_color(png::ColorType::Rgba);
        encoder.set_depth(png::BitDepth::Eight);
        let mut writer = encoder.write_header().map_err(|error| {
            vec![Diagnostic::new(
                code::RENDER_PNG_IO,
                format!(
                    "could not write PNG header for `{}`: {error}",
                    path.display()
                ),
            )]
        })?;
        writer.write_image_data(&image.rgba).map_err(|error| {
            vec![Diagnostic::new(
                code::RENDER_PNG_IO,
                format!(
                    "could not write PNG pixel data for `{}`: {error}",
                    path.display()
                ),
            )]
        })?;
    }
    ludion_core::atomic::write_atomic(path, bytes).map_err(|error| vec![io_diagnostic(path, error)])
}

fn io_diagnostic(path: &Path, error: std::io::Error) -> Diagnostic {
    Diagnostic::new(
        code::RENDER_PNG_IO,
        format!("could not write `{}`: {error}", path.display()),
    )
    .with_help("check the output location is writable")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch_set(name: &str) -> PathBuf {
        let root =
            std::env::temp_dir().join(format!("ludion-golden-set-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        root
    }

    #[test]
    fn an_empty_golden_root_is_not_live() {
        let root = scratch_set("empty");
        assert!(!golden_set_is_live(&root));
        assert!(!golden_set_is_live(&root.join("absent")));
        // The skip path returns instead of panicking.
        missing_golden(&root.join("windows").join("a.png"), "bless it");
    }

    #[test]
    fn a_png_under_any_platform_makes_the_set_live() {
        let root = scratch_set("live");
        std::fs::create_dir_all(root.join("macos")).unwrap();
        std::fs::write(root.join("macos").join("a.png"), b"").unwrap();
        assert!(golden_set_is_live(&root));
        let missing = root.join("windows").join("b.png");
        let panic = std::panic::catch_unwind(|| missing_golden(&missing, "bless it")).unwrap_err();
        let text = panic.downcast_ref::<String>().expect("a formatted panic");
        assert!(text.contains("L2201"), "{text}");
        assert!(text.contains("b.png"), "{text}");
    }

    #[test]
    fn a_non_png_file_does_not_make_the_set_live() {
        let root = scratch_set("notes");
        std::fs::create_dir_all(root.join("windows")).unwrap();
        std::fs::write(root.join("windows").join("notes.txt"), b"").unwrap();
        assert!(!golden_set_is_live(&root));
    }

    #[test]
    fn bless_refuses_the_pinned_dir_from_a_non_reference_adapter() {
        let golden = Path::new("goldens/windows/scene.png");
        let refusal = bless_refusal(golden, "windows", "Intel(R) UHD Graphics (Vulkan)", false)
            .expect("a laptop GPU must not bless goldens/windows/");
        assert_eq!(refusal.code.as_str(), "L2206");
        assert!(
            refusal.message.contains("Intel(R) UHD"),
            "{}",
            refusal.message
        );
        assert!(refusal.message.contains("RTX 3090"), "{}", refusal.message);
        assert!(refusal.message.contains("scene.png"), "{}", refusal.message);
        assert!(
            refusal
                .help
                .as_deref()
                .is_some_and(|help| help.contains("LUDION_BLESS_ANY_ADAPTER=1"))
        );
    }

    #[test]
    fn bless_passes_on_the_reference_adapter() {
        let golden = Path::new("goldens/windows/scene.png");
        assert!(
            bless_refusal(golden, "windows", "NVIDIA GeForce RTX 3090 (Vulkan)", false).is_none()
        );
        let golden = Path::new("goldens/macos/scene.png");
        assert!(bless_refusal(golden, "macos", "Apple M4 (Metal)", false).is_none());
    }

    #[test]
    fn bless_passes_outside_a_per_os_dir() {
        let scratch = Path::new("target/tmp/goldens/cube.voxel.png");
        assert!(bless_refusal(scratch, "windows", "Intel(R) UHD Graphics", false).is_none());
        let other_os = Path::new("goldens/macos/scene.png");
        assert!(bless_refusal(other_os, "windows", "Intel(R) UHD Graphics", false).is_none());
    }

    #[test]
    fn bless_opt_out_passes_on_any_adapter() {
        let golden = Path::new("goldens/windows/scene.png");
        assert!(bless_refusal(golden, "windows", "Intel(R) UHD Graphics", true).is_none());
    }

    fn host_golden() -> PathBuf {
        Path::new("goldens")
            .join(std::env::consts::OS)
            .join("scene.png")
    }

    const LAPTOP: &str = "Intel(R) UHD Graphics (Vulkan)";

    #[test]
    fn compare_skips_a_byte_exact_golden_on_a_non_reference_adapter() {
        let skip = compare_skip(&host_golden(), 1.0, Ok("windows"), Some(LAPTOP), false)
            .expect("a laptop GPU cannot verify a bit-exact golden");
        assert_eq!(skip.code.as_str(), "L2210");
        assert!(skip.message.contains("Intel(R) UHD"), "{}", skip.message);
        assert!(skip.message.contains("RTX 3090"), "{}", skip.message);
        assert!(skip.message.contains("scene.png"), "{}", skip.message);
    }

    #[test]
    #[should_panic(expected = "LUDION_REQUIRE_REFERENCE_ADAPTER")]
    fn a_required_reference_adapter_panics_on_l2210() {
        let skip = compare_skip(&host_golden(), 1.0, Ok("windows"), Some(LAPTOP), false).unwrap();
        refuse_adapter_skip(skip, true);
    }

    #[test]
    fn an_optional_reference_adapter_keeps_the_skip() {
        let skip = compare_skip(&host_golden(), 1.0, Ok("windows"), Some(LAPTOP), false).unwrap();
        assert_eq!(refuse_adapter_skip(skip, false).code.as_str(), "L2210");
    }

    #[test]
    fn compare_runs_on_the_reference_adapter() {
        let adapter = Some("NVIDIA GeForce RTX 3090 (Vulkan)");
        assert!(compare_skip(&host_golden(), 1.0, Ok("windows"), adapter, false).is_none());
        let adapter = Some("Apple M4 (Metal)");
        assert!(compare_skip(&host_golden(), 1.0, Ok("macos"), adapter, false).is_none());
    }

    #[test]
    fn compare_runs_below_a_byte_exact_threshold() {
        assert!(compare_skip(&host_golden(), 0.99, Ok("windows"), Some(LAPTOP), false).is_none());
    }

    #[test]
    fn compare_runs_on_a_scratch_golden() {
        let scratch = Path::new("target/tmp/goldens/cube.voxel.png");
        assert!(compare_skip(scratch, 1.0, Ok("windows"), Some(LAPTOP), false).is_none());
    }

    #[test]
    fn compare_opt_out_runs_on_any_adapter() {
        assert!(compare_skip(&host_golden(), 1.0, Ok("windows"), Some(LAPTOP), true).is_none());
    }

    #[test]
    fn compare_on_an_unpinned_platform_is_l2205_before_any_render() {
        let unpinned = Diagnostic::new(code::RENDER_GOLDEN_PLATFORM_UNPINNED, "no tier");
        let skip = compare_skip(&host_golden(), 1.0, Err(unpinned), None, false)
            .expect("an unpinned platform has nothing to compare against");
        assert_eq!(skip.code.as_str(), "L2205");
    }

    #[test]
    fn compare_waits_for_the_adapter_before_judging_it() {
        assert!(compare_skip(&host_golden(), 1.0, Ok("windows"), None, false).is_none());
    }

    #[test]
    fn unverifiable_names_the_skip_codes_only() {
        let platform = Diagnostic::new(code::RENDER_GOLDEN_PLATFORM_UNPINNED, "a");
        let adapter = Diagnostic::new(code::RENDER_GOLDEN_COMPARE_ADAPTER_UNPINNED, "b");
        let other = Diagnostic::new(code::RENDER_GOLDEN_MISSING, "c");
        assert_eq!(
            unverifiable(&[platform]).map(|d| d.code.as_str()),
            Some("L2205")
        );
        assert_eq!(
            unverifiable(&[adapter]).map(|d| d.code.as_str()),
            Some("L2210")
        );
        assert!(unverifiable(&[other]).is_none());
    }

    #[test]
    fn with_frames_sets_the_field_and_defaults_to_zero() {
        let test = GoldenTest::new("scene.yaml", "golden.png");
        assert_eq!(test.frames, 0, "an un-ticked golden extracts at rest");
        assert_eq!(test.with_frames(23).frames, 23);
    }

    #[test]
    fn with_builder_sets_the_seam_and_defaults_to_none() {
        let test = GoldenTest::new("scene.yaml", "golden.png");
        assert!(
            test.builder.is_none(),
            "a golden ticks the built-in schedule unless a game builder is handed to it"
        );
        assert!(
            test.with_builder(ludion_engine::Engine::builder)
                .builder
                .is_some()
        );
    }

    #[test]
    fn with_builder_takes_a_capturing_closure_and_the_test_stays_clone_and_debug() {
        let n = 3;
        let test = GoldenTest::new("scene.yaml", "golden.png").with_builder(move || {
            let _ = n;
            ludion_engine::Engine::builder()
        });
        let copy = test.clone();
        assert!(copy.builder.is_some());
        let text = format!("{copy:?}");
        assert!(text.contains("<fn>"), "{text}");
    }

    fn solid(rgba: [u8; 4]) -> DecodedImage {
        DecodedImage {
            width: 16,
            height: 16,
            rgba: rgba.repeat(16 * 16),
        }
    }

    const REFERENCE: &str = "NVIDIA GeForce RTX 3090 (Vulkan)";

    fn env(bless: bool, any_adapter: bool) -> Env {
        Env {
            platform: Ok("windows"),
            bless,
            any_adapter,
            require_reference: false,
        }
    }

    /// Runs `check_with` over a render that counts its calls; returns the count and the panic text.
    fn check(
        golden: &Path,
        out_dir: &Path,
        env: &Env,
        image: &DecodedImage,
        adapter: &str,
    ) -> (u32, Option<String>) {
        let mut calls = 0;
        let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            check_with(golden, 0.99, out_dir, env, &mut || {
                calls += 1;
                Ok((image.clone(), adapter.to_owned()))
            });
        }));
        let panic = outcome.err().map(|payload| {
            payload
                .downcast_ref::<String>()
                .cloned()
                .unwrap_or_else(|| "a non-string panic".to_owned())
        });
        (calls, panic)
    }

    #[test]
    fn a_bless_writes_the_golden_on_the_reference_adapter() {
        let root = scratch_set("bless");
        let golden = root.join("windows").join("a.png");
        let (calls, panic) = check(
            &golden,
            &root.join("out"),
            &env(true, false),
            &solid([255, 0, 0, 255]),
            REFERENCE,
        );
        assert_eq!((calls, panic), (1, None));
        assert!(golden.exists());
    }

    #[test]
    fn a_bless_from_another_adapter_is_refused_unless_opted_out() {
        let root = scratch_set("bless-refused");
        let golden = root.join("windows").join("a.png");
        let red = solid([255, 0, 0, 255]);
        let (_, panic) = check(&golden, &root.join("out"), &env(true, false), &red, LAPTOP);
        let text = panic.expect("a laptop GPU must not bless the pinned set");
        assert!(text.contains("L2206"), "{text}");
        assert!(!golden.exists());

        let (_, panic) = check(&golden, &root.join("out"), &env(true, true), &red, LAPTOP);
        assert_eq!(panic, None);
        assert!(golden.exists());
    }

    #[test]
    fn an_unpinned_platform_skips_before_rendering() {
        let root = scratch_set("unpinned");
        let mut unpinned = env(true, false);
        unpinned.platform = Err(Diagnostic::new(
            code::RENDER_GOLDEN_PLATFORM_UNPINNED,
            "no tier",
        ));
        let (calls, panic) = check(
            &root.join("linux").join("a.png"),
            &root.join("out"),
            &unpinned,
            &solid([0, 0, 0, 255]),
            LAPTOP,
        );
        assert_eq!((calls, panic), (0, None));
    }

    #[test]
    fn a_fresh_golden_set_skips_without_rendering() {
        let root = scratch_set("fresh");
        let (calls, panic) = check(
            &root.join("windows").join("a.png"),
            &root.join("out"),
            &env(false, false),
            &solid([0, 0, 0, 255]),
            REFERENCE,
        );
        assert_eq!((calls, panic), (0, None));
    }

    #[test]
    fn a_live_golden_set_missing_its_golden_is_l2201() {
        let root = scratch_set("live-missing");
        std::fs::create_dir_all(root.join("macos")).unwrap();
        std::fs::write(root.join("macos").join("a.png"), b"").unwrap();
        let (calls, panic) = check(
            &root.join("windows").join("b.png"),
            &root.join("out"),
            &env(false, false),
            &solid([0, 0, 0, 255]),
            REFERENCE,
        );
        assert_eq!(calls, 0);
        let text = panic.expect("a live set with a missing golden must fail");
        assert!(text.contains("L2201"), "{text}");
    }

    #[test]
    fn a_failing_compare_names_where_it_rendered() {
        // `GoldenTest::from_replay(..).check(..)` ends in this same `check_with`, so its failure
        // carries the same provenance line.
        let root = scratch_set("fail");
        let golden = root.join("windows").join("a.png");
        write_png(&golden, &solid([255, 0, 0, 255])).unwrap();
        let out = root.join("out");
        let (calls, panic) = check(
            &golden,
            &out,
            &env(false, false),
            &solid([0, 0, 255, 255]),
            LAPTOP,
        );
        assert_eq!(calls, 1);
        let text = panic.expect("a red render against a blue golden must fail");
        assert!(text.contains("provenance:"), "{text}");
        assert!(out.join("a.actual.png").exists());
    }

    #[test]
    fn a_passing_compare_leaves_no_artifacts() {
        let root = scratch_set("pass");
        let golden = root.join("windows").join("a.png");
        write_png(&golden, &solid([255, 0, 0, 255])).unwrap();
        let out = root.join("out");
        let (calls, panic) = check(
            &golden,
            &out,
            &env(false, false),
            &solid([255, 0, 0, 255]),
            REFERENCE,
        );
        assert_eq!((calls, panic), (1, None));
        assert!(!out.exists());
    }

    #[test]
    fn in_set_resolves_the_per_os_golden() {
        let test = GoldenTest::in_set("scene.yaml", "goldens", "main");
        let dir = platform_dir().unwrap_or(std::env::consts::OS);
        assert_eq!(test.golden, Path::new("goldens").join(dir).join("main.png"));
    }

    #[test]
    fn test_stem_strips_extension() {
        assert_eq!(test_stem(Path::new("/a/b/sprites.png")), "sprites");
        assert_eq!(test_stem(Path::new("lights.png")), "lights");
    }

    #[test]
    fn provenance_label_calls_the_pinned_reference_adapter_a_regression() {
        let label = provenance_label("NVIDIA GeForce RTX 3090 (Vulkan)", "windows");
        assert!(label.contains("this is a regression"), "{label}");

        let label = provenance_label("Apple M4 (Metal)", "macos");
        assert!(label.contains("this is a regression"), "{label}");
    }

    #[test]
    fn provenance_label_calls_any_other_adapter_environment() {
        let label = provenance_label("NVIDIA GeForce GPU (Vulkan)", "windows");
        assert!(label.contains("not the pinned RTX 3090"), "{label}");
        assert!(
            label.contains("environment until the reference box rules"),
            "{label}"
        );

        let label = provenance_label("Apple M4 (Metal)", "windows");
        assert!(label.contains("not the pinned RTX 3090"), "{label}");
    }

    #[test]
    fn provenance_label_names_no_reference_off_the_pinned_platforms() {
        let label = provenance_label("Mesa llvmpipe", "linux");
        assert!(label.contains("no pinned reference adapter"), "{label}");
    }

    fn opaque_image(size: u32) -> DecodedImage {
        let mut rgba = Vec::new();
        for y in 0..size {
            for x in 0..size {
                rgba.extend_from_slice(&[(x * 7 % 256) as u8, (y * 5 % 256) as u8, 90, 255]);
            }
        }
        DecodedImage {
            width: size,
            height: size,
            rgba,
        }
    }

    fn report(golden: &DecodedImage, actual: &DecodedImage, threshold: f64) -> DiffReport {
        report_for(golden, actual, "g", "a", threshold, None)
    }

    #[test]
    fn identical_rgba_passes_at_one() {
        let image = opaque_image(32);
        assert!(report(&image, &image.clone(), 1.0).pass);
    }

    #[test]
    fn an_alpha_only_lsb_fails_at_one_with_l2207() {
        let golden = opaque_image(32);
        let mut actual = golden.clone();
        actual.rgba[(5 * 32 + 3) * 4 + 3] = 254;
        let report = report(&golden, &actual, 1.0);
        assert!(!report.pass);
        assert_eq!(report.diagnostic().code.as_str(), "L2207");
        assert!(report.summary.contains("(3, 5)"), "{}", report.summary);
        assert!(
            report.summary.contains("1 in alpha only"),
            "{}",
            report.summary
        );
    }

    #[test]
    fn an_rgb_lsb_fails_at_one() {
        let golden = opaque_image(32);
        let mut actual = golden.clone();
        actual.rgba[2] = 91;
        assert!(!report(&golden, &actual, 1.0).pass);
    }

    #[test]
    fn an_alpha_lsb_passes_below_one() {
        let golden = opaque_image(32);
        let mut actual = golden.clone();
        actual.rgba[3] = 254;
        assert!(report(&golden, &actual, 0.99).pass);
    }

    #[test]
    fn a_transparent_tile_fails_the_colour_ceiling_below_one() {
        let golden = opaque_image(64);
        let mut actual = golden.clone();
        for y in 0..8 {
            for x in 0..8 {
                actual.rgba[(y * 64 + x) * 4 + 3] = 0;
            }
        }
        let report = report(&golden, &actual, 0.99);
        assert!(!report.pass);
        assert_eq!(report.diagnostic().code.as_str(), "L2207");
    }
}
