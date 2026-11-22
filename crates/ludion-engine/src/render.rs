//! The lockstep render path: scene file → image, fully sequential.
//!
//! [`render_scene`] is `run_lockstep` in embryonic form (`engine-architecture.md` §3): read → check
//! → instantiate → tick → extract → render → blocking readback, all on one thread with no
//! concurrency, so determinism is trivial. It ticks the world a caller-chosen number of times (0 by
//! default — a static scene, byte-identical to every prior render) before extracting: the sim
//! advances the world, `extract_at()` snapshots it at that sim time, and the renderer turns the
//! snapshot into pixels. The ticked schedule is always `Schedule::new()`'s empty one — no game
//! systems ever run here — so ticking moves nothing by itself; it exists for the things `extract_at`
//! reads straight off sim time, such as an assembly clip or a burst particle emitter, which is why
//! `ludion render --frames` needs no game to show motion.
//!
//! [`render_scene_with_builder`] is the other half of that story, for the one caller that holds the
//! game: it builds the *game's* engine and ticks the *game's* schedule, so frame N is the world a
//! player would be looking at N ticks in rather than frame 0 with the clock moved. Its CPU half,
//! [`engine_at_frame`], stops before the GPU so a headless test can read the ticked world.
//!
//! Scene validation, instantiation, and extraction are all CPU-only and happen **before** the GPU is
//! requested, so a broken scene fails with its diagnostics without ever touching an adapter.

use std::path::Path;

use ludion_core::codes::code;
use ludion_core::diagnostics::Diagnostic;
use ludion_ecs::{Registry, World};
use ludion_protocol::Command;
use ludion_protocol::command::SimStepParams;
use ludion_protocol::perf::LoadProfile;
use ludion_renderer::{
    AntiAliasing, DebugView, FrameSize, GpuUploads, PostChain, QualityParams, RenderScene,
    RenderedImage, Renderer,
};
use ludion_scene::{AppliedOverride, SceneOverride};
use ludion_sim::SimDriver;

use crate::builder::EngineBuilder;
use crate::builtin_registry;
use crate::debug_draw::{DebugCategories, DebugDraw};
use crate::engine::Engine;
use crate::extract::extract_at;
use crate::quality::QualitySetting;
use crate::sequences::{SequenceEnd, SequencePreview};

/// The result of a lockstep render: the image plus the adapter it ran on (for logging).
pub struct RenderOutcome {
    /// The rendered frame.
    pub image: RenderedImage,
    /// A human-readable adapter description, e.g. `"NVIDIA … (Vulkan)"`.
    pub adapter: String,
    /// Per-pass GPU timings JSON, when requested (`collect_timings`); one entry per scheduled pass.
    pub timings_json: Option<String>,
    /// What each render-time override changed, in the order the overrides were given. Empty when
    /// none were passed. Callers echo these — an override silently disagreeing with the file on
    /// disk is a footgun.
    pub applied_overrides: Vec<AppliedOverride>,
    /// The debug-draw categories that drew anything on the last tick — with the categories asked
    /// for, what tells a typo'd name from a category with nothing to show. Empty when none were asked.
    pub debug_drawn: Vec<&'static str>,
    /// The debug view actually drawn: the requested one when the frame traces reflections, `None`
    /// when none was asked for or SSR is off for this frame (not authored, or masked by the quality).
    pub debug_view: Option<DebugView>,
    /// The exposure the meter chose for this frame; `None` for a scene that does not meter.
    pub metered: Option<ludion_renderer::MeterReading>,
}

/// Renders the scene at `path` at `size` through the full pipeline, at [`QualitySetting::full`] —
/// every effect the scene authors, at every axis's ceiling.
///
/// With `collect_timings`, a second, instrumented render collects per-pass GPU timings into
/// [`RenderOutcome::timings_json`] (the `ludion render --dump-timings` payload). The timing run is
/// separate from the image render so the image bytes are identical with and without it.
///
/// # Errors
/// Every failure is a `Vec<Diagnostic>`:
/// - `L1015` if the file cannot be read; `L1001`–`L1014` for parse/validation problems.
/// - `L0004`/`L0006` for instantiation problems.
/// - `L1101`/`L1102`/`L2101` from [`extract_at`].
/// - `L2003`/`L2004`/`L2005` from the GPU (adapter, device, or readback).
pub fn render_scene(
    path: &Path,
    size: FrameSize,
    collect_timings: bool,
) -> Result<RenderOutcome, Vec<Diagnostic>> {
    render_scene_with_registry(
        &builtin_registry(),
        path,
        size,
        &QualitySetting::full(),
        collect_timings,
        &[],
        0,
        &DebugCategories::None,
        None,
        AntiAliasing::Off,
        None,
    )
}

/// Renders the scene at `path` through the pipeline, validating it against `registry`, with
/// `overrides` applied to the loaded scene in memory, after ticking the world `frames` times.
///
/// Identical to [`render_scene`] but uses the caller's registry instead of the built-in one — the
/// seam a *game* golden test needs, since its scene names custom components that the built-in
/// registry would reject with `L1013`. Only the built-in visual components are actually drawn
/// (`extract_at` reads them by type); a game component is validated and instantiated but invisible,
/// and no game schedule is ever installed here — `frames` ticks `Schedule::new()`'s empty one, so it
/// moves nothing but the sim clock [`extract_at`] is given and, when the scene authors a `physics:`
/// block, the bare physics driver (`crate::scene_physics::bare_driver`) (`ludion render --frames`,
/// the instrument for judging a burst emitter, an assembly clip, or a physics-driven `SkinBone`
/// ragdoll without writing a test). `frames = 0` is a static tick-0 frame, byte-identical to every
/// render before this parameter existed.
///
/// `view` replaces the finished picture with a full-screen debug view ([`DebugView`]); it is host
/// render state, never part of the scene, and a frame without its subject (no SSR trace, no probes)
/// ignores it ([`RenderOutcome::debug_view`] says what was drawn).
///
/// `overrides` (the `ludion render --set` payload) are applied between parse and validation, so an
/// ill-typed override is rejected by the schema exactly as the same edit to the file would be. The
/// file on disk is never written. Pass `&[]` for the file as authored.
///
/// `quality` is the caller's own — this path builds a one-shot renderer, so nothing else can supply
/// it. [`QualitySetting::full`] renders every effect the scene authors, at every axis's ceiling.
/// `anti_aliasing` is likewise the caller's: this path has no builder or host to resolve it from.
///
/// # Errors
/// As [`render_scene`], plus `L1013` if the scene names a component `registry` does not know, and
/// `L4008` if an override's pointer does not resolve.
#[allow(clippy::too_many_arguments)]
pub fn render_scene_with_registry(
    registry: &Registry,
    path: &Path,
    size: FrameSize,
    quality: &QualitySetting,
    collect_timings: bool,
    overrides: &[SceneOverride],
    frames: u64,
    debug: &DebugCategories,
    view: Option<DebugView>,
    anti_aliasing: AntiAliasing,
    locale: Option<&str>,
) -> Result<RenderOutcome, Vec<Diagnostic>> {
    let (mut render_scene, applied_overrides, debug_drawn) = extract_scene_file(
        registry,
        path,
        size,
        overrides,
        frames,
        &quality.params,
        debug,
        locale,
    )?;
    render_scene.debug_view = view;

    // GPU work happens only now, after all CPU-side validation has passed.
    let mut renderer = Renderer::new().map_err(|d| vec![d])?;
    renderer.set_quality(quality.quality, quality.params);
    renderer.set_anti_aliasing(anti_aliasing);
    let adapter = format!(
        "{} ({})",
        renderer.adapter_name(),
        renderer.adapter_backend()
    );
    renderer.set_meter_readback(true);
    let image = renderer.render(&render_scene, size).map_err(|d| vec![d])?;
    let metered = renderer.collect_meter_readings(true).pop();
    renderer.set_meter_readback(false);

    let timings_json = if collect_timings {
        Some(
            renderer
                .dump_timings_json(&render_scene, size)
                .map_err(|d| vec![d])?,
        )
    } else {
        None
    };

    // The view actually drawn: the same chain the frame was built from drops a view whose SSR is
    // not authored or is masked by the quality.
    let debug_view = PostChain::for_scene(&render_scene)
        .masked(&quality.quality, &quality.params)
        .view();

    Ok(RenderOutcome {
        image,
        adapter,
        timings_json,
        applied_overrides,
        debug_drawn,
        debug_view,
        metered,
    })
}

/// The frame and every secondary view it renders, from one extract: what [`render_scene_views`] returns.
pub struct ViewsOutcome {
    /// The main frame, byte-identical to [`render_scene_with_registry`]'s image of the same scene.
    pub frame: RenderedImage,
    /// Each rendered view by name, in `views` order, at its own `size`.
    pub views: Vec<(String, RenderedImage)>,
    /// A human-readable adapter description, e.g. `"NVIDIA … (Vulkan)"`.
    pub adapter: String,
}

impl ViewsOutcome {
    /// The image of the view named `name`.
    ///
    /// # Errors
    /// `L2379` if no rendered view has that name.
    pub fn view(&self, name: &str) -> Result<&RenderedImage, Diagnostic> {
        match self.views.iter().find(|(view, _)| view == name) {
            Some((_, image)) => Ok(image),
            None => Err(view_not_rendered(
                name,
                self.views.iter().map(|(view, _)| view.as_str()),
            )),
        }
    }
}

/// Renders the scene at `path` as [`render_scene_with_registry`] does, then every secondary view the
/// frame renders alone, each at its own `size` ([`Renderer::render_view`]). The views are the ones the
/// frame's `UiRect`s sample: an unknown, disabled or over-budget `RenderView` renders nothing here either.
///
/// # Errors
/// As [`render_scene_with_registry`].
#[allow(clippy::too_many_arguments)]
pub fn render_scene_views(
    registry: &Registry,
    path: &Path,
    size: FrameSize,
    quality: &QualitySetting,
    overrides: &[SceneOverride],
    frames: u64,
    locale: Option<&str>,
    anti_aliasing: AntiAliasing,
) -> Result<ViewsOutcome, Vec<Diagnostic>> {
    let (render_scene, _, _) = extract_scene_file(
        registry,
        path,
        size,
        overrides,
        frames,
        &quality.params,
        &DebugCategories::None,
        locale,
    )?;
    let mut renderer = Renderer::new().map_err(|d| vec![d])?;
    renderer.set_quality(quality.quality, quality.params);
    renderer.set_anti_aliasing(anti_aliasing);
    let adapter = format!(
        "{} ({})",
        renderer.adapter_name(),
        renderer.adapter_backend()
    );
    let frame = renderer.render(&render_scene, size).map_err(|d| vec![d])?;
    let mut views = Vec::with_capacity(render_scene.views.len());
    for (index, view) in render_scene.views.iter().enumerate() {
        let image = renderer
            .render_view(&render_scene, index)
            .map_err(|d| vec![d])?;
        views.push((view.name.clone(), image));
    }
    Ok(ViewsOutcome {
        frame,
        views,
        adapter,
    })
}

/// The index in `scene.views` of the view named `name`.
///
/// # Errors
/// `L2379` if no rendered view has that name (an unknown name, a disabled view or an over-budget one);
/// the help lists the rendered names, the nearest first.
pub fn find_view(
    views: &[ludion_renderer::RenderViewData],
    name: &str,
) -> Result<usize, Diagnostic> {
    views
        .iter()
        .position(|view| view.name == name)
        .ok_or_else(|| view_not_rendered(name, views.iter().map(|view| view.name.as_str())))
}

/// The `L2379` for `name` against the `rendered` view names.
fn view_not_rendered<'a>(
    name: &str,
    rendered: impl Iterator<Item = &'a str> + Clone,
) -> Diagnostic {
    let names: Vec<&str> = rendered.collect();
    let help = if names.is_empty() {
        "the scene renders no view; add an enabled `RenderView` (`ludion explain L2379`)"
            .to_string()
    } else {
        let nearest = ludion_core::suggest::nearest(name, names.iter().copied())
            .map(|near| format!("did you mean `{near}`? "))
            .unwrap_or_default();
        format!(
            "{nearest}rendered views: {}; `ludion explain L2379`",
            names.join(", ")
        )
    };
    Diagnostic::new(
        code::RENDER_VIEW_NOT_RENDERED,
        format!("no enabled, in-budget `RenderView` named `{name}` renders this frame"),
    )
    .with_help(help)
}

/// The result of a render driven through a game's own [`EngineBuilder`]: the encoded frame plus the
/// adapter it ran on and the output extent it landed at.
///
/// PNG bytes rather than a [`RenderedImage`] because that is what [`Engine::render_png`] — the one
/// render core the live host, a `render.screenshot`, and the replay-to-frames batch already share —
/// produces, and this path adds no second renderer of its own.
pub struct BuilderRenderOutcome {
    /// The rendered frame, PNG-encoded.
    pub png: Vec<u8>,
    /// A human-readable adapter description, e.g. `"NVIDIA … (Vulkan)"`.
    pub adapter: String,
    /// The output extent actually rendered: the caller's `width`/`height` where given, else the
    /// engine's own [`render_size`](Engine::render_size).
    pub size: (u32, u32),
    /// The debug-draw categories the game's systems drew on the last tick (see
    /// [`RenderOutcome::debug_drawn`]).
    pub debug_drawn: Vec<&'static str>,
    /// What loading the scene and rendering its first frame cost: `Some` when the builder observes
    /// load ([`EngineBuilder::observe_load`]), `None` otherwise.
    pub load: Option<LoadProfile>,
}

/// Builds `builder`'s engine rooted at `path`'s directory, loads that scene, and runs `frames` ticks
/// of the **game's own schedule** — the CPU half of [`render_scene_with_builder`], and the one place
/// a game frame is advanced for an offline render.
///
/// [`render_scene_with_registry`] installs `Schedule::new()`'s empty schedule instead, so its frame
/// N is frame 0 with nothing but the sim clock moved. Here every system the builder registered runs,
/// so frame N is the world a player would be looking at N ticks in. The ticks go through the ordinary
/// `sim.step` command, applying at frame boundaries exactly as a hosted session's do.
///
/// Returned rather than rendered so a headless test can assert on the world the ticks produced
/// without asking for a GPU.
///
/// # Errors
/// The collapsed build/`scene.load` diagnostic (a scene with several errors chains each through its
/// `related` list), or `L4007` if a system panicked during the ticks.
pub fn engine_at_frame(
    builder: EngineBuilder,
    path: &Path,
    frames: u64,
) -> Result<Engine, Vec<Diagnostic>> {
    let mut engine = builder.load_scene_file(path).map_err(|d| vec![d])?;
    engine
        .execute(Command::SimStep(SimStepParams {
            ticks: frames,
            hash: false,
        }))
        .map_err(|d| vec![d])?;
    Ok(engine)
}

/// The cumulative GPU uploads of `engine`'s renderer, creating it if it has none (`L2003`/`L2004`
/// as [`Engine::render_png`]). A scene change re-uploads only what the new world's first frame names
/// and the last frame did not, so the difference between two readings around a load shows it.
///
/// ```no_run
/// # use ludion_core::diagnostics::Diagnostic;
/// # use ludion_engine::{Engine, gpu_uploads};
/// # fn demo(engine: &mut Engine) -> Result<(), Diagnostic> {
/// engine.render_png(64, 64)?;
/// let before = gpu_uploads(engine)?;
/// // ... load another scene ...
/// engine.render_png(64, 64)?;
/// let new_meshes = gpu_uploads(engine)?.mesh_assets - before.mesh_assets;
/// # let _ = new_meshes; Ok(()) }
/// ```
///
/// # Errors
/// `L2003`/`L2004` when no GPU adapter or device is available.
pub fn gpu_uploads(engine: &mut Engine) -> Result<GpuUploads, Diagnostic> {
    Ok(engine.renderer()?.uploads())
}

/// Renders the scene at `path` through `builder`'s own engine — the game's registry, its data docs
/// and its **systems** — after `frames` ticks of the game's schedule.
///
/// [`render_scene_with_registry`]'s game-side twin, and the seam behind a game binary's
/// `--render-out --frames N` and `ludion_testkit`'s `GoldenTest::with_builder`: where that path can
/// only show what tick 0 draws (plus whatever `extract_at` reads off the sim clock), this one shows
/// the game — an early wave, a midboss entry — because the systems that spawn it actually ran.
///
/// `width`/`height` default to the engine's own [`render_size`](Engine::render_size) when `None`.
/// `quality` is installed on the engine before the render, so the frame extent is the host's own
/// (that output extent at this engine's render scale), exactly as a live screenshot's is. `debug`
/// chooses the debug-draw categories the frame shows ([`DebugCategories::None`] for the frame as
/// authored); what the game's systems drew on the last tick is what gets drawn.
///
/// # Errors
/// Everything [`engine_at_frame`] raises, plus any render diagnostic — `L2003` when no GPU adapter
/// is available, which callers graceful-skip on.
pub fn render_scene_with_builder(
    builder: EngineBuilder,
    path: &Path,
    width: Option<u32>,
    height: Option<u32>,
    quality: &QualitySetting,
    frames: u64,
    debug: &DebugCategories,
) -> Result<BuilderRenderOutcome, Vec<Diagnostic>> {
    let mut engine = engine_at_frame(builder, path, frames)?;
    engine.set_quality(quality.clone());
    engine.set_debug_draw(debug.clone());
    let (default_width, default_height) = engine.render_size();
    let size = (
        width.unwrap_or(default_width),
        height.unwrap_or(default_height),
    );
    let png = engine.render_png(size.0, size.1).map_err(|d| vec![d])?;
    let load = crate::profile::finish_load(&mut engine);
    let adapter = engine.adapter_label().map_err(|d| vec![d])?;
    Ok(BuilderRenderOutcome {
        png,
        adapter,
        size,
        debug_drawn: engine.debug_draw_drawn(),
        load,
    })
}

/// The `dump-graph` payload for the scene at `path`: the frame graph that scene actually renders,
/// as canonical JSON. No GPU — this is [`render_scene`]'s CPU prologue plus a graph dump, so an
/// adapterless machine answers it.
///
/// The chain comes from the extracted scene ([`PostChain::for_scene`]) masked by `quality`, so the
/// dump is the graph that machine really renders: a scene carrying UI or a sun dumps its six-pass
/// shape rather than the minimal graph's five, a supersampled `size` adds the `resolve` pass (which
/// makes this the way to verify a render scale without a GPU), a masked effect's passes are absent
/// exactly as if the scene had never authored it, and a machine that lowers `shadow_map_size` dumps
/// its own shadow map's extent. A scene that renders secondary views adds each view's own graph under
/// a trailing `views` key (anti-aliasing off, the same quality mask).
///
/// # Errors
/// As [`render_scene`]'s CPU half (`L1015`, `L1001`–`L1014`, `L0004`/`L0006`,
/// `L1101`/`L1102`/`L2101`), plus `L2001`/`L2002` if the graph fails to schedule — it never does.
pub fn dump_scene_graph_json(
    path: &Path,
    size: FrameSize,
    quality: &QualitySetting,
    anti_aliasing: AntiAliasing,
) -> Result<String, Vec<Diagnostic>> {
    // The graph is a statement about which passes run, and particles are drawn inside the `scene`
    // pass whatever their count — so this path extracts at full density regardless of `quality`,
    // whose effect on the dump is the mask and the declared extents.
    let (scene, _, _) = extract_scene_file(
        &builtin_registry(),
        path,
        size,
        &[],
        0,
        &QualityParams::FULL,
        &DebugCategories::None,
        None,
    )?;
    let masked = |s: &ludion_renderer::RenderScene| {
        PostChain::for_scene(s).masked(&quality.quality, &quality.params)
    };
    ludion_renderer::dump_scene_frame_graph_json(
        &scene,
        size,
        PostChain {
            aa: anti_aliasing,
            ..masked(&scene)
        },
        masked,
    )
    .map_err(|d| vec![d])
}

/// What [`extract_scene_file`] returns: the scene, what each override changed, and the debug-draw
/// categories that drew on the last tick.
type ExtractedScene = (RenderScene, Vec<AppliedOverride>, Vec<&'static str>);

/// Expands `scene`'s prefab instance entries against `project_root` and spawns the flat result,
/// `SkinBone` poses applied. Returns the expanded scene (the one later passes read) and its world.
pub(crate) fn expanded_world(
    project_root: &Path,
    registry: &Registry,
    scene: &ludion_scene::Scene,
) -> Result<(ludion_scene::Scene, World), Vec<Diagnostic>> {
    let expanded = crate::scene_check::expand_scene(project_root, registry, scene)?;
    let (world, _) = crate::engine::instantiate_expanded_world(&expanded, registry, project_root)?;
    Ok((expanded.scene, world))
}

/// The world half of [`extract_scene_file`]: read → parse → override → validate → instantiate →
/// tick, stopping before any extraction, so a caller that needs the world after extract (the GI
/// dumps diagnosing an empty grid) keeps it. Errors are exactly [`extract_scene_file`]'s up to its
/// extract step.
pub(crate) fn scene_file_driver(
    registry: &Registry,
    path: &Path,
    overrides: &[SceneOverride],
    frames: u64,
    debug: &DebugCategories,
) -> Result<(SimDriver, Vec<AppliedOverride>), Vec<Diagnostic>> {
    let (scene, mut world, applied_overrides) = scene_file_world(registry, path, overrides)?;
    let project_root = path.parent().unwrap_or(Path::new("."));

    // No game schedule is ever installed here — the seed threads through so the shape matches the
    // full driver, even though nothing here consumes randomness. Physics is the one exception: a
    // scene's `physics:` block is the only opt-in there is, so a bare host steps it too.
    if *debug != DebugCategories::None {
        world.insert_resource(DebugDraw::default());
    }
    let mut driver = crate::scene_physics::bare_driver(world, &scene, project_root, false);
    // `run_ticks` spelled out, so the debug drawing rolls over at each tick boundary as the engine's
    // own does.
    driver.step(frames);
    while driver.advance() {
        if let Some(draw) = driver.world_mut().resource_mut::<DebugDraw>() {
            draw.end_tick();
        }
    }
    if let Some(d) = driver.take_system_panic() {
        return Err(vec![d]);
    }

    Ok((driver, applied_overrides))
}

/// read → parse → override → validate → instantiate, the prologue every bare host shares.
fn scene_file_world(
    registry: &Registry,
    path: &Path,
    overrides: &[SceneOverride],
) -> Result<(ludion_scene::Scene, World, Vec<AppliedOverride>), Vec<Diagnostic>> {
    let source = std::fs::read_to_string(path).map_err(|error| {
        vec![
            Diagnostic::new(
                code::SCENE_IO,
                format!("could not read `{}`: {error}", path.display()),
            )
            .with_help("check the path (relative to the current directory) and permissions"),
        ]
    })?;

    let mut scene = ludion_scene::parse(&source)?;
    let applied_overrides = ludion_scene::apply_overrides(&mut scene, overrides)?;
    let diagnostics = ludion_scene::validate(&scene, registry);
    if !diagnostics.is_empty() {
        return Err(diagnostics);
    }
    let project_root = path.parent().unwrap_or(Path::new("."));
    let (scene, mut world) = expanded_world(project_root, registry, &scene)?;
    crate::scene_nav::install(&mut world, &scene)?;
    Ok((scene, world, applied_overrides))
}

/// The CPU half of the lockstep path: read → parse → override → validate → instantiate → tick →
/// extract, with no GPU touched. Shared by [`render_scene_with_registry`], [`dump_scene_graph_json`],
/// and `crate::voxel_sheet::render_voxel_sheet` so a scene that renders, a scene that dumps its
/// graph, and a scene that dumps its GI grid all go through exactly one validation and extraction
/// path.
///
/// Overrides land between parse and validation deliberately: the schema then rejects an ill-typed
/// override with the `L1014` it would give the same value written into the file, rather than a
/// second, weaker check here.
///
/// `frames` ticks the world that many times, over the empty built-in `Schedule` (no game systems
/// are ever installed here), before [`extract_at`] snapshots it at the sim time that reaches.
/// [`render_scene_with_registry`] threads its own caller's count through; `dump_scene_graph_json` and
/// `crate::voxel_sheet::render_voxel_sheet` pass `0` and keep their static tick-0 frame.
///
/// Extraction uses [`size.output`](FrameSize::output), never the render extent: the camera's aspect
/// is the same either way, but screen-space UI is authored in output pixels and drawn by the overlay
/// pass after the resolve.
///
/// `quality` is the caller's own (its `particle_density` and `lod_bias` are read): only the path that renders
/// pixels for a machine passes that machine's value, so `ludion render --quality low` draws the
/// thinned population while the graph dump and the voxel sheets stay at
/// [`QualityParams::FULL`]'s — an instrument compared across boxes must not move with one.
///
/// `debug` picks the debug-draw categories the extracted scene carries; anything but
/// [`DebugCategories::None`] installs the [`DebugDraw`] resource the bare host's systems draw into,
/// and the third element names the categories that drew anything on the last tick (empty for `None`).
///
/// `locale` is the active locale of a bare render: the content root's string tables are loaded and
/// keyed text resolved when it is `Some` or the scene has a keyed `UiText`/`Text`; every other scene
/// takes no part in localization. `None` means the default locale.
#[allow(clippy::too_many_arguments)]
pub(crate) fn extract_scene_file(
    registry: &Registry,
    path: &Path,
    size: FrameSize,
    overrides: &[SceneOverride],
    frames: u64,
    quality: &QualityParams,
    debug: &DebugCategories,
    locale: Option<&str>,
) -> Result<ExtractedScene, Vec<Diagnostic>> {
    let (mut driver, applied_overrides) =
        scene_file_driver(registry, path, overrides, frames, debug)?;
    let (render_scene, drawn) = extract_driver(&mut driver, path, size, quality, debug, locale)?;
    Ok((render_scene, applied_overrides, drawn))
}

/// The extraction half of [`extract_scene_file`], over a world already ticked.
fn extract_driver(
    driver: &mut SimDriver,
    path: &Path,
    size: FrameSize,
    quality: &QualityParams,
    debug: &DebugCategories,
    locale: Option<&str>,
) -> Result<(RenderScene, Vec<&'static str>), Vec<Diagnostic>> {
    let project_root = path.parent().unwrap_or(Path::new("."));
    if locale.is_some() || has_keyed_text(driver.world_mut()) {
        let world = driver.world_mut();
        crate::localization::install_content_docs(world, project_root);
        if let Some(tag) = locale {
            crate::localization::set_locale(world, tag);
        }
        crate::localization::resolve_pass(world);
    }
    if driver.world().resource::<crate::Nav>().is_some() {
        crate::nav::refresh(driver.world_mut());
    }
    let mut render_scene = extract_at(
        driver.world(),
        size.output,
        ludion_assets::AssetRoot::content(project_root),
        driver.time(),
        quality.particle_density,
    )?;
    render_scene.debug_lines = crate::debug_draw::segments(driver.world(), debug);
    if debug.enables(crate::debug_draw::CULL) {
        let marks = ludion_renderer::cull_marks(&render_scene, quality.lod_bias, true);
        render_scene.debug_lines.extend(marks);
    }
    let drawn = crate::debug_draw::drawn(driver.world());
    Ok((render_scene, drawn))
}

/// What [`sequence_preview_world`] leaves: the ticked world and what the preview found.
pub struct SequencePreviewWorld {
    /// The driver, stopped right after the update whose playhead is the preview's `at`.
    pub driver: SimDriver,
    /// What each `--set` override changed.
    pub applied_overrides: Vec<AppliedOverride>,
    /// The document's content-root key.
    pub key: String,
    /// How the sequence ended by then, `None` while it is still playing.
    pub ended: Option<SequenceEnd>,
    /// The warnings the run reported (`L4352` unbound role, `L4353` degraded key).
    pub warnings: Vec<Diagnostic>,
}

/// The CPU half of [`render_sequence_preview`]: loads the scene at `path` into the bare host with the
/// built-in sequence update seated, plays `preview` over it and steps `preview.at + 1` ticks, with no
/// GPU touched. Only the built-in schedule runs (`path_follow`, `sequence_update`, the scene's physics,
/// `camera_rig`): no game systems and no dialogue.
///
/// # Errors
/// The scene's diagnostics as [`render_scene_with_registry`] gives them, plus `L4355`, `L4356` and
/// `L4351` from [`SequencePreview`]'s install.
pub fn sequence_preview_world(
    registry: &Registry,
    path: &Path,
    overrides: &[SceneOverride],
    preview: &SequencePreview,
) -> Result<SequencePreviewWorld, Vec<Diagnostic>> {
    let (scene, world, applied_overrides) = scene_file_world(registry, path, overrides)?;
    let project_root = path.parent().unwrap_or(Path::new("."));
    let mut driver = crate::scene_physics::bare_driver(world, &scene, project_root, true);
    let (key, handle) =
        crate::sequences::preview::install(driver.world_mut(), project_root, preview)?;
    let ended = crate::sequences::preview::run(&mut driver, handle, preview);
    if let Some(d) = driver.take_system_panic() {
        return Err(vec![d]);
    }
    let warnings = crate::sequences::preview::warnings(driver.world_mut());
    Ok(SequencePreviewWorld {
        driver,
        applied_overrides,
        key,
        ended,
        warnings,
    })
}

/// What [`render_sequence_preview`] returns.
pub struct SequencePreviewOutcome {
    /// The rendered frame.
    pub image: RenderedImage,
    /// A human-readable adapter description.
    pub adapter: String,
    /// What each `--set` override changed.
    pub applied_overrides: Vec<AppliedOverride>,
    /// The document's content-root key.
    pub key: String,
    /// How the sequence ended by the rendered tick, `None` while it is still playing.
    pub ended: Option<SequenceEnd>,
    /// The warnings the run reported.
    pub warnings: Vec<Diagnostic>,
}

/// Renders the scene at `path` as [`render_scene_with_registry`] does, after playing `preview` over it
/// ([`sequence_preview_world`]): no timings, debug drawing or view.
///
/// # Errors
/// As [`sequence_preview_world`] and [`render_scene_with_registry`].
#[allow(clippy::too_many_arguments)]
pub fn render_sequence_preview(
    registry: &Registry,
    path: &Path,
    size: FrameSize,
    quality: &QualitySetting,
    overrides: &[SceneOverride],
    preview: &SequencePreview,
    anti_aliasing: AntiAliasing,
    locale: Option<&str>,
) -> Result<SequencePreviewOutcome, Vec<Diagnostic>> {
    let mut run = sequence_preview_world(registry, path, overrides, preview)?;
    let (render_scene, _) = extract_driver(
        &mut run.driver,
        path,
        size,
        &quality.params,
        &DebugCategories::None,
        locale,
    )?;

    let mut renderer = Renderer::new().map_err(|d| vec![d])?;
    renderer.set_quality(quality.quality, quality.params);
    renderer.set_anti_aliasing(anti_aliasing);
    let adapter = format!(
        "{} ({})",
        renderer.adapter_name(),
        renderer.adapter_backend()
    );
    let image = renderer.render(&render_scene, size).map_err(|d| vec![d])?;
    Ok(SequencePreviewOutcome {
        image,
        adapter,
        applied_overrides: run.applied_overrides,
        key: run.key,
        ended: run.ended,
        warnings: run.warnings,
    })
}

/// Whether any `UiText` or `Text` carries a localization key: the only scenes a bare host resolves
/// strings for without being asked to.
fn has_keyed_text(world: &mut World) -> bool {
    world
        .query_ids::<(&ludion_components::UiText,)>()
        .any(|(_, (t,))| t.key.is_some())
        || world
            .query_ids::<(&ludion_components::Text,)>()
            .any(|(_, (t,))| t.key.is_some())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_scene_with_an_instance_entry_extracts_its_parts() {
        let root =
            std::env::temp_dir().join(format!("ludion-render-instance-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(
            root.join("lamp.prefab.yaml"),
            "format: 1\nkind: prefab\nentities:\n  - name: bulb\n    components:\n      Transform:\n        translation:\n          - 0.0\n          - 1.0\n          - 0.0\n      PointLight:\n        color:\n          - 1.0\n          - 1.0\n          - 1.0\n        intensity: 5.0\n        range: 10.0\n",
        )
        .unwrap();
        let scene = root.join("scene.yaml");
        std::fs::write(
            &scene,
            "format: 1\nseed: 0\nentities:\n  - name: lamp\n    prefab: lamp.prefab.yaml\n",
        )
        .unwrap();
        let (extracted, _, _) = extract_scene_file(
            &builtin_registry(),
            &scene,
            FrameSize::native(64, 64),
            &[],
            0,
            &QualityParams::FULL,
            &DebugCategories::None,
            None,
        )
        .expect("the instance entry expands");
        assert_eq!(extracted.lights.len(), 1);
        let _ = std::fs::remove_dir_all(&root);
    }

    fn localized_root(tag: &str, keyed: bool, strings_doc_valid: bool) -> std::path::PathBuf {
        let root =
            std::env::temp_dir().join(format!("ludion-render-locale-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(
            root.join("locales.yaml"),
            "format: 1\nkind: locales\ndefault: en\nlocales:\n  en:\n    name: English\n  fr:\n    name: Français\n",
        )
        .unwrap();
        let strings = |locale: &str, hello: &str| {
            format!("format: 1\nkind: strings\nlocale: {locale}\nstrings:\n  hello: {hello}\n")
        };
        std::fs::write(root.join("en.yaml"), strings("en", "Hello")).unwrap();
        let fr = if strings_doc_valid {
            strings("fr", "Bonjour")
        } else {
            "format: 1\nkind: strings\nlocale: [broken\n".to_owned()
        };
        std::fs::write(root.join("fr.yaml"), fr).unwrap();
        let text = if keyed {
            "      UiText:\n        text: fallback\n        key: hello\n"
        } else {
            "      UiText:\n        text: plain\n"
        };
        std::fs::write(
            root.join("scene.yaml"),
            format!("format: 1\nseed: 0\nentities:\n  - name: label\n    components:\n{text}"),
        )
        .unwrap();
        root
    }

    fn extracted_ui_text(root: &Path, locale: Option<&str>) -> String {
        let (extracted, _, _) = extract_scene_file(
            &builtin_registry(),
            &root.join("scene.yaml"),
            FrameSize::native(64, 64),
            &[],
            0,
            &QualityParams::FULL,
            &DebugCategories::None,
            locale,
        )
        .expect("the scene extracts");
        extracted
            .ui
            .iter()
            .find_map(|instance| match instance {
                ludion_renderer::UiInstance::Text { text, .. } => Some(text.clone()),
                _ => None,
            })
            .expect("the scene draws one text")
    }

    #[test]
    fn a_keyed_scene_resolves_its_text_in_the_requested_locale() {
        let root = localized_root("keyed", true, true);
        assert_eq!(extracted_ui_text(&root, None), "Hello");
        assert_eq!(extracted_ui_text(&root, Some("fr")), "Bonjour");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_scene_without_a_key_ignores_a_malformed_strings_file() {
        let root = localized_root("plain", false, false);
        assert_eq!(extracted_ui_text(&root, None), "plain");
        let _ = std::fs::remove_dir_all(&root);
    }

    /// A one-triangle mesh document binding `../textures/t.png` as its base-colour map.
    const TEXTURED_MESH: &str = "format: 1
kind: mesh
positions:
  - 0.0
  - 0.0
  - 0.0
  - 1.0
  - 0.0
  - 0.0
  - 0.0
  - 1.0
  - 0.0
uvs:
  - 0.0
  - 0.0
  - 1.0
  - 0.0
  - 0.0
  - 1.0
base_color_texture:
  path: ../textures/t.png
indices:
  - 0
  - 1
  - 2
";

    /// A scratch project: meshes `a`, `s` (shared) and `b`, each binding its own base-colour map, `a`
    /// and `s` also binding `shared.png` as their emissive map, plus one loose sprite PNG per scene.
    fn reupload_project(tag: &str) -> std::path::PathBuf {
        let dir =
            std::env::temp_dir().join(format!("ludion-reupload-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("meshes")).unwrap();
        std::fs::create_dir_all(dir.join("textures")).unwrap();
        let png = |red: u8| ludion_assets::encode_png(1, 1, &[red, 20, 30, 255]).unwrap();
        for (name, red) in [
            ("a", 10),
            ("s", 20),
            ("b", 30),
            ("shared", 40),
            ("sp_a", 50),
            ("sp_b", 60),
        ] {
            std::fs::write(dir.join(format!("textures/{name}.png")), png(red)).unwrap();
        }
        for (name, shared) in [("a", true), ("s", true), ("b", false)] {
            let mut doc = TEXTURED_MESH.replace("t.png", &format!("{name}.png"));
            if shared {
                doc.push_str("emissive_texture:\n  path: ../textures/shared.png\n");
            }
            std::fs::write(dir.join(format!("meshes/{name}.mesh.yaml")), doc).unwrap();
        }
        dir
    }

    /// A scene naming `meshes` (by stem), one loose sprite and a spawn anchor.
    fn reupload_scene(meshes: &[&str], sprite: &str) -> String {
        let mut scene = String::from("format: 1\nentities:\n");
        for mesh in meshes {
            scene.push_str(&format!(
                "  - components:\n      Mesh:\n        shape:\n          asset: meshes/{mesh}.mesh.yaml\n"
            ));
        }
        scene.push_str(&format!(
            "  - components:
      Sprite:
        texture: textures/{sprite}.png
        size:
          - 1.0
          - 1.0
  - name: anchor
    components:
      Transform:
        translation:
          - 0.0
          - 0.0
          - 0.0
"
        ));
        scene
    }

    /// What a render added to the engine's renderer since `before`; `None` with no GPU adapter.
    fn render_delta(engine: &mut Engine, before: GpuUploads) -> Option<GpuUploads> {
        match engine.render_png(64, 64) {
            Ok(_) => {}
            Err(d) if ludion_core::require_gpu::is_no_adapter(std::slice::from_ref(&d)) => {
                eprintln!("SKIPPED reupload: no GPU adapter");
                return None;
            }
            Err(d) => panic!("render failed: {d:?}"),
        }
        let now = gpu_uploads(engine).unwrap();
        Some(GpuUploads {
            textures: now.textures - before.textures,
            mesh_maps: now.mesh_maps - before.mesh_maps,
            mesh_assets: now.mesh_assets - before.mesh_assets,
        })
    }

    /// The scenes' unique assets: one sprite (`sp_*`), one base-colour map and one mesh.
    const OWN_ASSETS: GpuUploads = GpuUploads {
        textures: 1,
        mesh_maps: 1,
        mesh_assets: 1,
    };

    #[test]
    fn needs_gpu_a_second_scene_uploads_only_its_own_assets() {
        let dir = reupload_project("load");
        let mut engine = Engine::with_project_root(&dir);
        let a = reupload_scene(&["a", "s"], "sp_a");
        engine.load_inline_scene(&a).unwrap();
        let Some(first) = render_delta(&mut engine, GpuUploads::default()) else {
            let _ = std::fs::remove_dir_all(&dir);
            return;
        };

        let mut before = first;
        engine
            .load_inline_scene(&reupload_scene(&["b", "s"], "sp_b"))
            .unwrap();
        let to_b = render_delta(&mut engine, before).unwrap();
        before = gpu_uploads(&mut engine).unwrap();
        engine.load_inline_scene(&a).unwrap();
        let back_to_a = render_delta(&mut engine, before).unwrap();
        let _ = std::fs::remove_dir_all(&dir);

        assert_eq!(
            to_b, OWN_ASSETS,
            "sp_b, b.png and b; s and its maps are held"
        );
        assert_eq!(back_to_a, OWN_ASSETS, "sp_a, a.png and a; s is held");
    }

    #[test]
    fn needs_gpu_a_steady_frame_uploads_nothing() {
        let dir = reupload_project("steady");
        let mut engine = Engine::with_project_root(&dir);
        engine
            .load_inline_scene(&reupload_scene(&["a", "s"], "sp_a"))
            .unwrap();
        let Some(first) = render_delta(&mut engine, GpuUploads::default()) else {
            let _ = std::fs::remove_dir_all(&dir);
            return;
        };
        let steady = render_delta(&mut engine, first).unwrap();
        let _ = std::fs::remove_dir_all(&dir);

        assert_eq!(steady, GpuUploads::default(), "a held reference re-uploads");
    }

    #[test]
    fn needs_gpu_a_switch_uploads_only_its_own_assets() {
        let dir = reupload_project("switch");
        let mut engine = Engine::with_project_root(&dir);
        engine
            .load_inline_scene(&reupload_scene(&["a", "s"], "sp_a"))
            .unwrap();
        let Some(first) = render_delta(&mut engine, GpuUploads::default()) else {
            let _ = std::fs::remove_dir_all(&dir);
            return;
        };

        let text = reupload_scene(&["b", "s"], "sp_b");
        std::fs::write(dir.join("district.yaml"), &text).unwrap();
        let switch = crate::transition::SwitchTo {
            scene_path: "district.yaml".into(),
            spawn_anchor: "anchor".into(),
            carry: vec![],
        };
        crate::transition::perform(&mut engine, switch, &text).unwrap();
        let to_b = render_delta(&mut engine, first).unwrap();
        let _ = std::fs::remove_dir_all(&dir);

        assert_eq!(
            to_b, OWN_ASSETS,
            "sp_b, b.png and b; s and its maps are held"
        );
    }

    const ROOM_B: &str = "rooms/b.prefab.yaml";

    /// The reupload project plus `rooms/b.prefab.yaml`, one part drawing mesh `b`, and an engine
    /// running scene `a`.
    fn prewarm_engine(tag: &str) -> (Engine, std::path::PathBuf) {
        let dir = reupload_project(tag);
        std::fs::create_dir_all(dir.join("rooms")).unwrap();
        std::fs::write(
            dir.join(ROOM_B),
            "format: 1\nkind: prefab\nname: room-b\nentities:\n  - name: floor\n    components:\n      Mesh:\n        shape:\n          asset: ../meshes/b.mesh.yaml\n",
        )
        .unwrap();
        let mut engine = Engine::builder().project_root(&dir).build().unwrap();
        engine
            .load_inline_scene(&reupload_scene(&["a", "s"], "sp_a"))
            .unwrap();
        (engine, dir)
    }

    fn prewarm_b(engine: &mut Engine) {
        crate::prewarm_prefab(engine.world_mut(), ROOM_B, None).unwrap();
    }

    fn spawn_b(engine: &mut Engine) -> Vec<ludion_core::ids::EntityId> {
        crate::spawn_prefab(engine.world_mut(), ROOM_B, None, None, false).unwrap()
    }

    fn only_b_assets() -> GpuUploads {
        GpuUploads {
            textures: 0,
            mesh_maps: 1,
            mesh_assets: 1,
        }
    }

    #[test]
    fn needs_gpu_a_prewarmed_prefab_spawns_without_uploading() {
        let (mut engine, dir) = prewarm_engine("prewarm-spawn");
        let Some(first) = render_delta(&mut engine, GpuUploads::default()) else {
            let _ = std::fs::remove_dir_all(&dir);
            return;
        };
        prewarm_b(&mut engine);
        let warm = render_delta(&mut engine, first).unwrap();
        let mut before = gpu_uploads(&mut engine).unwrap();
        let steady = render_delta(&mut engine, before).unwrap();
        spawn_b(&mut engine);
        before = gpu_uploads(&mut engine).unwrap();
        let spawned = render_delta(&mut engine, before).unwrap();
        let _ = std::fs::remove_dir_all(&dir);

        assert_eq!(warm, only_b_assets(), "the prewarm uploads b and its map");
        assert_eq!(steady, GpuUploads::default(), "kept, not evicted");
        assert_eq!(spawned, GpuUploads::default(), "the spawn uploads nothing");
    }

    #[test]
    fn needs_gpu_a_prewarm_changes_no_pixel_hash_or_id() {
        let (mut warm, warm_dir) = prewarm_engine("prewarm-same-warm");
        let (mut cold, cold_dir) = prewarm_engine("prewarm-same-cold");
        let png = |engine: &mut Engine| match engine.render_png(64, 64) {
            Ok(png) => Some(png),
            Err(d) if ludion_core::require_gpu::is_no_adapter(std::slice::from_ref(&d)) => None,
            Err(d) => panic!("render failed: {d:?}"),
        };
        let (Some(_), Some(_)) = (png(&mut warm), png(&mut cold)) else {
            eprintln!("SKIPPED prewarm_changes_no_pixel: no GPU adapter");
            let _ = std::fs::remove_dir_all(&warm_dir);
            let _ = std::fs::remove_dir_all(&cold_dir);
            return;
        };
        prewarm_b(&mut warm);
        let warm_frame = png(&mut warm).unwrap();
        let cold_frame = png(&mut cold).unwrap();
        let warm_ids = spawn_b(&mut warm);
        let cold_ids = spawn_b(&mut cold);
        let warm_after = png(&mut warm).unwrap();
        let cold_after = png(&mut cold).unwrap();
        let warm_hash = warm.world_hash().unwrap();
        let cold_hash = cold.world_hash().unwrap();
        let _ = std::fs::remove_dir_all(&warm_dir);
        let _ = std::fs::remove_dir_all(&cold_dir);

        assert_eq!(warm_frame, cold_frame, "the prewarm frame draws the same");
        assert_eq!(
            warm_after, cold_after,
            "and so does the frame after the spawn"
        );
        assert_eq!(warm_ids, cold_ids, "no id shifted");
        assert_eq!(warm_hash, cold_hash, "the world hash did not move");
    }

    #[test]
    fn needs_gpu_a_superseded_or_cleared_prewarm_is_evicted() {
        let (mut engine, dir) = prewarm_engine("prewarm-evict");
        let Some(first) = render_delta(&mut engine, GpuUploads::default()) else {
            let _ = std::fs::remove_dir_all(&dir);
            return;
        };
        prewarm_b(&mut engine);
        let warm = render_delta(&mut engine, first).unwrap();
        engine
            .world_mut()
            .resource_mut::<crate::PrefabPrewarm>()
            .unwrap()
            .clear();
        let mut before = gpu_uploads(&mut engine).unwrap();
        render_delta(&mut engine, before).unwrap();
        spawn_b(&mut engine);
        before = gpu_uploads(&mut engine).unwrap();
        let spawned = render_delta(&mut engine, before).unwrap();
        let _ = std::fs::remove_dir_all(&dir);

        assert_eq!(warm, only_b_assets());
        assert_eq!(
            spawned.mesh_assets, 1,
            "the cleared prewarm was freed, so the spawn uploads b again"
        );
    }
}
