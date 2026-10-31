//! Implementation of `ludion render --scene <FILE> --out <PNG> [--width N] [--height N]
//! [--dump-timings <PATH>]` and `ludion render --model <FILE> [--views LIST] [--out <PNG>]
//! [--width N] [--height N]`.
//!
//! **Scene mode** renders the actual scene content through the full render pipeline
//! (`ludion-renderer`, via `ludion_engine::render_scene` — the lockstep driver): check →
//! instantiate → extract → render → blocking readback → PNG. Scene validation, instantiation, and
//! extraction happen before the GPU is requested, so a broken scene fails without an adapter. The
//! render is deterministic: two invocations with the same inputs write byte-identical PNGs.
//! `--dump-timings` additionally writes per-pass GPU timings JSON (one entry per pass) from a
//! second, instrumented render. `--dump-voxels <PNG>` writes the scene's `GiVolume` grid (if
//! extraction built one) as a stitched slice-sheet PNG (`Plan/gi-wave0-voxelizer.md` §11,
//! `ludion_engine::render_voxel_sheet`) — pure CPU, no GPU adapter touched; when `--out` is omitted
//! it is the only thing the command does, and it never acquires an adapter at all. `--dump-gi <PNG>`
//! writes the scene's baked GI probe volume (if extraction built one) as a stitched slice-sheet PNG —
//! VISIBILITY, BENT NORMAL, BOUNCE, and DYNAMIC blocks (`Plan/gi-wave1-sky-visibility.md` §10,
//! `Plan/gi-wave2-emissive-bounce.md` §8, `Plan/gi-wave3-dynamic-injection.md` §10,
//! `ludion_engine::render_gi_sheet`) — unlike `--dump-voxels`
//! this **does** request a GPU adapter, because the probe volume it dumps exists only as a baked GPU
//! texture. The BOUNCE and DYNAMIC blocks each carry a title stating why they draw no data, not a
//! black block, on a scene that authored `bounce: 0.0` or marks no light `GiDynamic`; the summary
//! line echoes the baked/injected states, the split light census (`N baked + M injected`), and
//! whether this dump's own bake was a cache hit.
//! `--set <json-pointer>=<value>` overrides scene values in memory
//! before validation (`--look-at` expands into such overrides — camera pose, DOF focus); each
//! applied override is echoed above the summary line, because an override
//! silently disagreeing with the file on disk is a footgun. `--sweep <json-pointer>=<v,v,…>` renders
//! the same scene once per value and stitches the results into one labelled contact sheet
//! (`ludion_engine::render_sweep_sheet`) — the instrument for the taste calls `image-stats` cannot
//! make. `--frames N` ticks the built-in (game-system-free) schedule N times before extracting, so a
//! burst particle emitter or an assembly clip renders in motion instead of frozen at tick 0 — the
//! default, absent the flag, is still tick 0, byte-identical to every render before this flag
//! existed. `--frames a,b,c` (two or more, ascending) instead renders once per value and stitches a
//! labelled `frame N` strip through the same machinery `--sweep` uses
//! (`ludion_engine::render_frame_strip`), one row wide; it requires `--out` and conflicts with
//! `--sweep`/`--model`, exactly as `--sweep` conflicts with a model sheet. `--scale` moves the render
//! extent: the chain renders at N× the requested size and is
//! resampled back after tonemapping, before any UI overlay — resolved down above 1.0, magnified up
//! below it. Absent, a plain `--scene` render draws at whatever `--quality` asks for (native at
//! every preset but `low`); given, it beats that. The PNG is always the requested size; the summary
//! line names the render extent whenever it differs, so a quality setting can never silently
//! disagree with the resolution the user asked for.
//!
//! **Model mode** (`--model`) renders a `.voxel.yaml`/`.mesh.yaml`/`.assembly.yaml`/`.sculpt.yaml`/`.prefab.yaml`
//! model document (an assembly or prefab at its rest pose) as a turntable contact sheet via
//! `ludion_engine::render_model_sheet` — the model-view harness that collapses the voxel/mesh
//! authoring loop to `check-scene` → `render --model` → look at one image. `--out`
//! defaults to `<name>.sheet.png` in the current directory when omitted. `--rig <scene>` swaps the
//! synthesized bright neutral key + fill for an existing scene's lighting and grade, so a palette
//! bound for a night game can be judged under that game's night — the lift contract and its
//! unit-normalization caveat live in the harness's module docs. `--set`/`--sweep` address the
//! model's **palette** here rather than a scene's components (and, on a sculpt, any numeric field —
//! `/resolution`, `/parts/<i>/blend`), so value calibration — the slowest
//! loop in voxel authoring — is one command; `--true-size` sizes the sheet cell from the model's
//! declared `scale:` instead of the unit-sphere fit that renders every model the same size.
//! `--clip <name>` poses an assembly with one of the clips it binds instead of rendering its rest
//! pose (`--time <seconds>` picks the instant, quantized to 60 Hz sim ticks; `--strip <n>` lays out
//! n evenly spaced phases in one image), and every phase prints a **gap analysis** — the posed
//! model's connected-component count plus, when it is more than one, the parts owning each piece
//! that came away, in the same `connectivity owners:` shape `check-scene` prints over the rest pose.
//! A `.character.yaml` or `.prefab.yaml` poses the same way (alone, no gap analysis: its phases print the clip time,
//! including its binding's overlay speed), and `--crossfade <from>:<to>:<seconds>` lays out the blend
//! of two bound clips as a strip whose phases print their fade `weight`, defaulting `--out` to
//! `<name>-<from>-<to>-crossfade.png`.
//! `--part <name>` frames a sculpt's six views on that part's meshed bounds with the rest of the
//! model kept around it, and `--isolate` hides the rest — one image answers "does the eye sit in the
//! socket"; an unknown or surfaceless part is `L4008`.
//!
//! **Sprite mode** (`--sprite`) composes a `kind: sprite` document's contact sheet through
//! `ludion_engine::render_sprite_sheet`, and **atlas mode** (`--atlas`) a packed `kind: atlas`
//! document's overview through `ludion_engine::render_atlas_sheet`. Both are CPU-only — no adapter
//! is ever requested, so they answer on a machine that has none. `--sprite` defaults `--out` the way
//! model mode does (`<stem>.sheet.png`); `--atlas` takes `<stem>.atlas-sheet.png` instead, because a
//! sprite document and the atlas packed from it share a stem and neither sheet may quietly replace
//! the other. `--set`/`--sweep` address the sprite document here (palette, seed, pivot, effect
//! parameter); `--scale` is the authored game scale the sheet's second block draws at, in whole
//! sheet pixels per texel, and a fractional one is refused before anything is baked. The applied
//! effects and overrides are echoed under the summary line as well as burned into the sheet, so a
//! terminal-only session reads what the image says.
//!
//! **`--atlas --clip <flipbook>`** switches atlas mode from the packing overview to one of the
//! clip instruments `ludion_engine::render_atlas_clip` composes, and the flag precedence between
//! the three is fixed: `--onion` overlays every entry up to the one showing (`--time` picks it,
//! else the clip's last), otherwise `--strip N` lays out N evenly spaced phases and supersedes
//! `--time`, otherwise `--time` (default `0.0`) draws the single entry showing then. Each defaults
//! `--out` to its own name — `<stem>.atlas-onion.png`, `.atlas-strip.png`, `.atlas-frame.png` — so
//! running all three over one atlas writes three files. `--scale` is the same whole-number texels
//! per pixel it is under `--sprite`. Without `--clip` the mode is the unchanged overview, and a
//! `--time`/`--strip`/`--onion` with no clip to modify is `L4008` rather than a silently ignored
//! flag (`ludion_engine::check_atlas_clip_flags`, which also refuses a strip alongside an onion
//! skin and a phase count outside 2..=16).

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use crate::diagnostic::report;
use ludion_core::codes::code;
use ludion_core::diagnostics::Diagnostic;
use ludion_engine::{
    DebugCategories, FrameSize, ModelChannel, ModelSheetRequest, ModelSweep, ModelView,
    QualitySetting, RenderScale, SpriteSheetRequest,
};
use ludion_scene::SceneOverride;

/// The widest game scale a sprite contact sheet's second block takes. Past it a frame's texels are
/// no longer being judged at their shipped size, and the sheet is only being magnified.
const MAX_SPRITE_SCALE: u32 = 16;

/// One `ludion render` invocation, as the flags spell it. Exactly one of
/// `scene`/`model`/`sprite`/`atlas`/`font` is set — enforced by the clap `ArgGroup` in `main.rs`.
pub struct Request<'a> {
    /// `--scene`: render a scene through the full pipeline.
    pub scene: Option<&'a Path>,
    /// `--model`: render one model document as a turntable contact sheet, or several as a lineup.
    /// Empty exactly when `scene` is `Some`.
    pub models: &'a [&'a Path],
    /// `--views` (model mode).
    pub views: &'a [ModelView],
    /// `--rig` (model mode).
    pub rig: Option<&'a Path>,
    /// `--sprite`: compose one `kind: sprite` document's contact sheet, CPU-only.
    pub sprite: Option<&'a Path>,
    /// `--atlas`: compose one packed `kind: atlas` document's overview — or, with `clip`, one of
    /// its flipbooks. CPU-only either way.
    pub atlas: Option<&'a Path>,
    /// `--font`: compose one `kind: font` document's specimen sheet, CPU-only.
    pub font: Option<&'a Path>,
    /// `--out`; defaulted in model, sprite, atlas and font modes.
    pub out: Option<&'a Path>,
    /// `--width`.
    pub width: u32,
    /// `--height`.
    pub height: u32,
    /// `--dump-timings` (scene mode).
    pub dump_timings: Option<&'a Path>,
    /// `--dump-voxels` (scene mode).
    pub dump_voxels: Option<&'a Path>,
    /// `--dump-gi` (scene mode).
    pub dump_gi: Option<&'a Path>,
    /// `--dump-probes` (scene mode).
    pub dump_probes: Option<&'a Path>,
    /// `--set`, unparsed.
    pub set: &'a [String],
    /// `--sweep`, unparsed.
    pub sweep: Option<&'a str>,
    /// `--frames` (scene mode): empty renders tick 0 (today's behaviour), one value ticks the world
    /// that many times before the ordinary render, two or more (ascending — `main.rs` checks before
    /// this `Request` is built) render a labelled time strip. Conflicts with `--sweep`/`--model` at
    /// the clap level.
    pub frames: &'a [u64],
    /// `--debug-draw` (scene mode), unparsed: `all` or a comma list of categories.
    pub debug_draw: Option<&'a str>,
    /// `--debug-view` (scene mode): a full-screen view of the SSR trace instead of the finished picture.
    pub debug_view: Option<ludion_engine::DebugView>,
    /// `--look-at` and its companions (scene mode): expanded into overrides by `framing::expand`.
    pub framing: crate::framing::FramingArgs<'a>,
    /// `--scale`, if it was given. In scene mode a render-extent multiplier; absent, the extent is
    /// `quality`'s. In sprite mode the whole-number game scale of the sheet's second block, and in
    /// atlas-clip mode the whole-number scale the flipbook instrument draws at.
    pub scale: Option<f32>,
    /// `--aa` (plain scene mode): the anti-aliasing the one-shot renderer applies; `Off` when absent.
    pub anti_aliasing: ludion_engine::AntiAliasing,
    /// `--true-size` (model mode).
    pub true_size: bool,
    /// `--clip` (model mode: the assembly clip to pose with; atlas mode: the flipbook to draw).
    pub clip: Option<&'a str>,
    /// `--time` (model and atlas-clip modes).
    pub time: Option<f64>,
    /// `--strip` (model and atlas-clip modes).
    pub strip: Option<u32>,
    /// `--crossfade` (model mode): the `<from>:<to>:<seconds>` spec, parsed in `run_model`.
    pub crossfade: Option<&'a str>,
    /// `--part` (model mode, a sculpt): frame the sheet on this named part.
    pub part: Option<&'a str>,
    /// `--isolate` (model mode, with `--part`): hide everything but the part.
    pub isolate: bool,
    /// `--channel` (model mode): draw one contribution of the stage instead of the finished picture.
    pub channel: Option<ModelChannel>,
    /// `--onion` (atlas-clip mode).
    pub onion: bool,
    /// The global `--quality`, resolved. Applies to a plain `--scene` render only: a model sheet and
    /// a `--sweep` contact sheet are authoring instruments, compared across machines and against
    /// committed references, so they render everything the content authors whatever this box chose.
    pub quality: &'a QualitySetting,
}

/// Runs `ludion render`.
///
/// On success writes the PNG (and, if requested, the timings JSON) and prints a one-line summary.
/// On any failure prints rustc-style diagnostics to stderr and returns a non-zero exit code.
pub fn run(request: &Request) -> ExitCode {
    // The two CPU sheets answer before anything reaches for an adapter or a scene.
    if let Some(doc) = request.sprite {
        return run_sprite(request, doc);
    }
    if let Some(doc) = request.atlas {
        return run_atlas(request, doc);
    }
    if let Some(doc) = request.font {
        return run_font(doc, request.out);
    }
    match (request.scene, request.models.first()) {
        (Some(scene), None) => {
            let explicit = match request.scale.map(RenderScale::new).transpose() {
                Ok(scale) => scale,
                Err(diagnostic) => return report(scene, &[diagnostic]),
            };
            let (width, height) = (request.width, request.height);
            let framed = match crate::framing::expand(scene, &request.framing, request.set) {
                Ok(framed) => framed,
                Err(diagnostics) => return report(scene, &diagnostics),
            };
            if let Some(summary) = &framed.summary {
                println!("{summary}");
            }
            let set = framed.set.as_slice();
            match request.sweep {
                // `--dump-voxels`/`--dump-gi` conflict with `--sweep` at the clap level, so `--out`
                // is still unconditionally required here.
                Some(spec) => {
                    let out = request.out.expect(
                        "clap requires --out with --sweep (dump-voxels/dump-gi conflict with it)",
                    );
                    // A sheet is an authoring instrument, judged against other machines' copies of
                    // it, so it takes the explicit scale and never the machine's — the same reason
                    // it renders at FULL whatever this box asked for.
                    let scale = explicit.unwrap_or(RenderScale::NATIVE);
                    run_sweep(scene, out, width, height, set, spec, scale)
                }
                // `--frames` conflicts with `--sweep` at the clap level, so this arm is reachable
                // whenever a time strip was asked for.
                None if request.frames.len() >= 2 => {
                    let Some(out) = request.out else {
                        return crate::diagnostic::usage_error(
                            "a `--frames` strip needs `--out <OUT>`: it stitches one sheet \
                             and has no default output name",
                        );
                    };
                    let scale = explicit.unwrap_or(RenderScale::NATIVE);
                    run_frame_strip(scene, out, width, height, set, request.frames, scale)
                }
                None => run_scene(
                    scene,
                    request.out,
                    width,
                    height,
                    request.dump_timings,
                    request.dump_voxels,
                    request.dump_gi,
                    request.dump_probes,
                    set,
                    request.frames.first().copied().unwrap_or(0),
                    ludion_engine::resolve_render_scale(explicit, request.quality),
                    request.quality,
                    &DebugCategories::parse(request.debug_draw.unwrap_or_default()),
                    request.debug_view,
                    request.anti_aliasing,
                ),
            }
        }
        // Model mode renders native: a turntable sheet is an authoring aid, not an image anyone
        // judges imaging on, and `--scale` conflicts with `--model` at the clap level.
        (None, Some(first)) => run_model(request, first),
        _ => {
            unreachable!(
                "clap's ArgGroup enforces exactly one of --scene/--model/--sprite/--atlas/--font"
            )
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn run_scene(
    scene: &Path,
    out: Option<&Path>,
    width: u32,
    height: u32,
    dump_timings: Option<&Path>,
    dump_voxels: Option<&Path>,
    dump_gi: Option<&Path>,
    dump_probes: Option<&Path>,
    set: &[String],
    frames: u64,
    scale: RenderScale,
    quality: &QualitySetting,
    debug: &DebugCategories,
    view: Option<ludion_engine::DebugView>,
    anti_aliasing: ludion_engine::AntiAliasing,
) -> ExitCode {
    let overrides = match parse_overrides(set) {
        Ok(overrides) => overrides,
        Err(diagnostics) => return report(scene, &diagnostics),
    };
    let size = FrameSize::scaled(width, height, scale);

    // Exactly one echo pass happens for the whole invocation: whichever dump runs first when
    // `--out` is absent (so a dump-only run still echoes its overrides), or — when `--out` is
    // given — none of them, because the real render below always echoes its own. `echo_overrides`
    // starts true only in the former case and flips off the moment something echoes.
    let mut echo_overrides = out.is_none();

    if let Some(voxel_out) = dump_voxels
        && let Err(failure) =
            run_dump_voxels(scene, voxel_out, size, &overrides, &mut echo_overrides)
    {
        return failure;
    }
    if let Some(gi_out) = dump_gi
        && let Err(failure) = run_dump_gi(scene, gi_out, size, &overrides, &mut echo_overrides)
    {
        return failure;
    }
    if let Some(probes_out) = dump_probes
        && let Err(failure) = run_dump_probes(
            scene,
            probes_out,
            size,
            quality,
            &overrides,
            &mut echo_overrides,
        )
    {
        return failure;
    }
    if out.is_none() {
        // Dump-only: no `--out` was given, so there is no real render to do. `--dump-voxels` never
        // requested a GPU adapter to get here; `--dump-gi`, if given, did.
        return ExitCode::SUCCESS;
    }
    // Reachable only with `--out` given: whatever dumps ran above succeeded, and there is still a
    // real render to do.
    let out = out.expect("checked above: dump-only already returned");

    match ludion_engine::render_scene_with_registry(
        &crate::check_scene::builtin_registry(),
        scene,
        size,
        quality,
        dump_timings.is_some(),
        &overrides,
        frames,
        debug,
        view,
        anti_aliasing,
    ) {
        Ok(outcome) => {
            for applied in &outcome.applied_overrides {
                echo(applied);
            }
            if let Err(diagnostic) = outcome.image.write_png(out) {
                eprintln!("{diagnostic}");
                return ExitCode::FAILURE;
            }
            if let (Some(path), Some(json)) = (dump_timings, &outcome.timings_json)
                && let Err(error) = ludion_core::atomic::write_atomic(path, json)
            {
                let diagnostic = Diagnostic::new(
                    code::OUTPUT_IO,
                    format!("could not write timings to `{}`: {error}", path.display()),
                )
                .with_help("check the output location is writable");
                eprintln!("{diagnostic}");
                return ExitCode::FAILURE;
            }
            println!(
                "{}: rendered {width}x{height}{}{}{}{} on {}",
                out.display(),
                resample_note(size),
                frames_note(frames),
                debug
                    .summary(&outcome.debug_drawn)
                    .map_or_else(String::new, |clause| format!(" ({clause})")),
                debug_view_note(view, outcome.debug_view),
                outcome.adapter
            );
            ExitCode::SUCCESS
        }
        Err(diagnostics) => report(scene, &diagnostics),
    }
}

/// The summary clause for `--debug-view`: the view drawn, or why the normal frame was drawn instead.
fn debug_view_note(
    asked: Option<ludion_engine::DebugView>,
    drawn: Option<ludion_engine::DebugView>,
) -> String {
    match (asked, drawn) {
        (Some(_), Some(view)) => format!(" (debug view: {})", view.name()),
        (Some(view), None) if view.is_ssr() => format!(
            " (debug view: {} not drawn: SSR is off for this frame (PostProcess.ssr is 0, or this quality masks it))",
            view.name()
        ),
        (Some(ludion_engine::DebugView::LocalShadowAtlas), None) => format!(
            " (debug view: {} not drawn: no local light is granted a shadow for this frame (none casts, none is in view, or this quality masks shadows))",
            ludion_engine::DebugView::LocalShadowAtlas.name()
        ),
        (Some(ludion_engine::DebugView::SunCascades), None) => format!(
            " (debug view: {} not drawn: the sun fits no cascades for this frame (no sun, no `cascades`, or this quality masks shadows))",
            ludion_engine::DebugView::SunCascades.name()
        ),
        (Some(view), None) => format!(
            " (debug view: {} not drawn: the scene has no reflection probes)",
            view.name()
        ),
        (None, _) => String::new(),
    }
}

/// `--dump-voxels`: writes the scene's `GiVolume` slice sheet and its summary line. Pure CPU —
/// `ludion_engine::render_voxel_sheet` never requests a GPU adapter, so this runs whether or not the
/// invocation goes on to a real `--out` render.
///
/// `echo_overrides` starts `true` exactly when no `--out` was given (see `run_scene`); this prints
/// the echo and flips it to `false` on success, so a second dump run in the same invocation (or the
/// `--out` render below) never prints the same echoes twice.
fn run_dump_voxels(
    scene: &Path,
    voxel_out: &Path,
    size: FrameSize,
    overrides: &[SceneOverride],
    echo_overrides: &mut bool,
) -> Result<(), ExitCode> {
    match ludion_engine::render_voxel_sheet(
        &crate::check_scene::builtin_registry(),
        scene,
        size,
        overrides,
    ) {
        Ok(outcome) => {
            if *echo_overrides {
                for applied in &outcome.applied_overrides {
                    echo(applied);
                }
                *echo_overrides = false;
            }
            if let Err(diagnostic) = outcome.image.write_png(voxel_out) {
                eprintln!("{diagnostic}");
                return Err(ExitCode::FAILURE);
            }
            println!(
                "{}: voxel sheet {}x{}, grid {}x{}x{}, {} occupied cell{}",
                voxel_out.display(),
                outcome.image.width,
                outcome.image.height,
                outcome.extent[0],
                outcome.extent[1],
                outcome.extent[2],
                outcome.occupied_cells,
                if outcome.occupied_cells == 1 { "" } else { "s" },
            );
            Ok(())
        }
        Err(diagnostics) => Err(report(scene, &diagnostics)),
    }
}

/// `--dump-gi`: writes the scene's baked GI probe-volume slice sheet — VISIBILITY, BENT NORMAL,
/// BOUNCE, and DYNAMIC blocks — and its summary line. Unlike [`run_dump_voxels`],
/// `ludion_engine::render_gi_sheet` **does** request a GPU adapter once the CPU prologue passes — the
/// probe volume it dumps exists only as a baked GPU texture.
///
/// `echo_overrides` follows the same convention as [`run_dump_voxels`]'s.
fn run_dump_gi(
    scene: &Path,
    gi_out: &Path,
    size: FrameSize,
    overrides: &[SceneOverride],
    echo_overrides: &mut bool,
) -> Result<(), ExitCode> {
    match ludion_engine::render_gi_sheet(
        &crate::check_scene::builtin_registry(),
        scene,
        size,
        overrides,
    ) {
        Ok(outcome) => {
            if *echo_overrides {
                for applied in &outcome.applied_overrides {
                    echo(applied);
                }
                *echo_overrides = false;
            }
            if let Err(diagnostic) = outcome.image.write_png(gi_out) {
                eprintln!("{diagnostic}");
                return Err(ExitCode::FAILURE);
            }
            println!(
                "{}: GI sheet {}x{}, probes {}x{}x{}, bounce {}, dynamic {}, lights {} baked + {} \
                 injected, bake {}",
                gi_out.display(),
                outcome.image.width,
                outcome.image.height,
                outcome.count[0],
                outcome.count[1],
                outcome.count[2],
                if outcome.bounce_baked {
                    "baked"
                } else {
                    "not baked"
                },
                match outcome.dynamic_absent {
                    None => "injected".to_string(),
                    Some(reason) => format!("not injected ({reason})"),
                },
                outcome.lights_baked,
                outcome.lights_injected,
                if outcome.bake_cached {
                    "cache hit"
                } else {
                    "fresh"
                },
            );
            // A one-shot render is unconditionally at the `auto` rebake lever — there is no spelling
            // of `hold` here, and no earlier frame for a held baseline to come from. Said out loud so
            // a sheet that disagrees with a live session's is read as the live one being stale, not
            // as the two dumps disagreeing.
            println!("  rebake auto, gi not held: a dumped sheet is always this scene's own bake");
            Ok(())
        }
        Err(diagnostics) => Err(report(scene, &diagnostics)),
    }
}

/// `--dump-probes`: writes each reflection probe's captured and prefiltered strips and the
/// `probes.json` manifest into `dir`, then a summary and one line per probe. Needs a GPU adapter;
/// `echo_overrides` follows [`run_dump_voxels`]'s convention.
fn run_dump_probes(
    scene: &Path,
    dir: &Path,
    size: FrameSize,
    quality: &QualitySetting,
    overrides: &[SceneOverride],
    echo_overrides: &mut bool,
) -> Result<(), ExitCode> {
    let outcome = match ludion_engine::render_probe_dump(
        &crate::check_scene::builtin_registry(),
        scene,
        size,
        quality,
        overrides,
    ) {
        Ok(outcome) => outcome,
        Err(diagnostics) => return Err(report(scene, &diagnostics)),
    };
    if *echo_overrides {
        for applied in &outcome.applied_overrides {
            echo(applied);
        }
        *echo_overrides = false;
    }
    let write = || -> Result<(), Diagnostic> {
        std::fs::create_dir_all(dir).map_err(|error| output_io(dir, &error))?;
        for probe in &outcome.probes {
            if let Some(captured) = &probe.captured {
                captured.write_png(dir.join(probe.captured_file()))?;
            }
            probe
                .prefiltered
                .write_png(dir.join(probe.prefiltered_file()))?;
        }
        let mut manifest = serde_json::to_string_pretty(&outcome.manifest(scene, &quality.label))
            .expect("a manifest serializes");
        manifest.push('\n');
        let path = dir.join("probes.json");
        ludion_core::atomic::write_atomic(&path, manifest.as_bytes())
            .map_err(|error| output_io(&path, &error))
    };
    if let Err(diagnostic) = write() {
        eprintln!("{diagnostic}");
        return Err(ExitCode::FAILURE);
    }
    println!(
        "{}: {} probes, face {}, quality {}",
        dir.display(),
        outcome.probes.len(),
        outcome.face_size,
        quality.label
    );
    for probe in &outcome.probes {
        let [x, y, z] = probe.position;
        println!(
            "  #{} {} at ({x}, {y}, {z}) key {:016x} {}{}",
            probe.rank,
            probe.shape_name(),
            probe.key,
            if probe.stored { "stored" } else { "not stored" },
            probe
                .capture_ms
                .map(|ms| format!(", capture {ms:.1} ms"))
                .unwrap_or_default(),
        );
    }
    Ok(())
}

fn output_io(path: &Path, error: &std::io::Error) -> Diagnostic {
    Diagnostic::new(
        code::OUTPUT_IO,
        format!("could not write `{}`: {error}", path.display()),
    )
    .with_help("check the output location is writable")
}

/// Which way the frame was resampled, as the word a summary line says it with, or `None` when the
/// render extent and the output one are the same. The one owner of the direction, so the two summary
/// lines below cannot disagree about it.
fn resample_verb(size: FrameSize) -> Option<&'static str> {
    size.resample().map(|_| {
        if size.supersampled() {
            "supersampled"
        } else {
            "upscaled"
        }
    })
}

/// The ` (supersampled from WxH)` / ` (upscaled from WxH)` clause a summary line grows when the
/// render extent is not the output one — a scale that silently disagreed with the printed resolution
/// would be the same footgun an un-echoed `--set` is, and below native it is the more important of
/// the two to say out loud. Empty when the extents match, so the ordinary line is unchanged.
fn resample_note(size: FrameSize) -> String {
    resample_verb(size)
        .map(|verb| format!(" ({verb} from {}x{})", size.render.0, size.render.1))
        .unwrap_or_default()
}

/// The ` (frames N)` clause a single-value `--frames N` grows onto the summary line — a render
/// that silently ticked the world before extracting would be the same footgun an un-echoed `--set`
/// is. Empty at `frames == 0`, so the ordinary tick-0 line is unchanged.
fn frames_note(frames: u64) -> String {
    if frames == 0 {
        String::new()
    } else {
        format!(" (frames {frames})")
    }
}

/// `--sweep`: one contact sheet of the same scene at N values of one pointer, for the taste calls
/// `image-stats` cannot make. The `--set` overrides apply to every cell, and so does `--scale`.
#[allow(clippy::too_many_arguments)]
fn run_sweep(
    scene: &Path,
    out: &Path,
    width: u32,
    height: u32,
    set: &[String],
    sweep: &str,
    scale: RenderScale,
) -> ExitCode {
    let overrides = match parse_overrides(set) {
        Ok(overrides) => overrides,
        Err(diagnostics) => return report(scene, &diagnostics),
    };
    let (pointer, values) = match ludion_scene::parse_sweep(sweep) {
        Ok(parsed) => parsed,
        Err(diagnostic) => return report(scene, &[diagnostic]),
    };

    match ludion_engine::render_sweep_sheet(
        scene, &overrides, &pointer, &values, width, height, scale,
    ) {
        Ok(outcome) => {
            if let Err(diagnostic) = outcome.image.write_png(out) {
                eprintln!("{diagnostic}");
                return ExitCode::FAILURE;
            }
            for applied in &outcome.applied_base {
                echo(applied);
            }
            println!(
                "sweep {}: was {}, over {}",
                outcome.pointer,
                previous(outcome.original.as_ref()),
                outcome
                    .cells
                    .iter()
                    .map(|cell| compact(&cell.value))
                    .collect::<Vec<_>>()
                    .join(", ")
            );
            let cell = FrameSize::scaled(
                outcome.image.width / outcome.cols.max(1),
                outcome.image.height / outcome.rows.max(1),
                scale,
            );
            println!(
                "{}: {} cells in a {}x{} grid, {}x{}{} on {}",
                out.display(),
                outcome.cells.len(),
                outcome.cols,
                outcome.rows,
                outcome.image.width,
                outcome.image.height,
                resample_verb(cell)
                    .map(|verb| format!(" (cells {verb} from {}x{})", cell.render.0, cell.render.1))
                    .unwrap_or_default(),
                outcome.adapter
            );
            ExitCode::SUCCESS
        }
        Err(diagnostics) => report(scene, &diagnostics),
    }
}

/// `--frames a,b,c`: one labelled time strip of the same scene ticked to each value, for judging
/// motion (a burst particle emitter, an assembly clip) that a static tick-0 render cannot show. The
/// `--set` overrides apply to every cell, and so does `--scale` — the same contract [`run_sweep`]
/// gives its own cells.
fn run_frame_strip(
    scene: &Path,
    out: &Path,
    width: u32,
    height: u32,
    set: &[String],
    frames: &[u64],
    scale: RenderScale,
) -> ExitCode {
    let overrides = match parse_overrides(set) {
        Ok(overrides) => overrides,
        Err(diagnostics) => return report(scene, &diagnostics),
    };

    match ludion_engine::render_frame_strip(scene, &overrides, frames, width, height, scale) {
        Ok(outcome) => {
            if let Err(diagnostic) = outcome.image.write_png(out) {
                eprintln!("{diagnostic}");
                return ExitCode::FAILURE;
            }
            for applied in &outcome.applied_base {
                echo(applied);
            }
            let cell = FrameSize::scaled(
                outcome.image.width / outcome.cols.max(1),
                outcome.image.height,
                scale,
            );
            println!(
                "frames {}: {} cells, {}x{}{} on {}",
                outcome
                    .cells
                    .iter()
                    .map(|cell| cell.frame.to_string())
                    .collect::<Vec<_>>()
                    .join(","),
                outcome.cells.len(),
                outcome.image.width,
                outcome.image.height,
                resample_verb(cell)
                    .map(|verb| format!(" (cells {verb} from {}x{})", cell.render.0, cell.render.1))
                    .unwrap_or_default(),
                outcome.adapter
            );
            ExitCode::SUCCESS
        }
        Err(diagnostics) => report(scene, &diagnostics),
    }
}

/// The `--set` specs as parsed overrides, collecting every malformed one so a batch of typos comes
/// back in a single run.
fn parse_overrides(set: &[String]) -> Result<Vec<SceneOverride>, Vec<Diagnostic>> {
    let mut overrides = Vec::with_capacity(set.len());
    let mut diagnostics = Vec::new();
    for spec in set {
        match SceneOverride::parse(spec) {
            Ok(over) => overrides.push(over),
            Err(diagnostic) => diagnostics.push(diagnostic),
        }
    }
    if diagnostics.is_empty() {
        Ok(overrides)
    } else {
        Err(diagnostics)
    }
}

/// One-line JSON for an echoed override value — compact so the echo stays greppable.
fn compact(value: &serde_json::Value) -> String {
    serde_json::to_string(value).unwrap_or_else(|_| value.to_string())
}

/// The prior value in an echo: what the file held, or `unset` when the override created the field.
/// Spelled apart from `null` on purpose — "the scene never mentioned this field" is the case where
/// an operator most wants to re-read the spelling.
fn previous(value: Option<&serde_json::Value>) -> String {
    value.map_or_else(|| "unset".to_string(), compact)
}

/// Echoes one applied override. An override silently disagreeing with the file on disk is a
/// footgun, so every one of them is printed above the summary line.
fn echo(applied: &ludion_scene::AppliedOverride) {
    println!(
        "set {}: {} -> {}",
        applied.pointer,
        previous(applied.from.as_ref()),
        compact(&applied.to)
    );
}

/// `model` is the first path — the one a single-model render takes its default `--out` and its
/// diagnostic header from. A lineup always has an explicit `--out` (enforced in `main.rs`).
fn run_model(request: &Request, model: &Path) -> ExitCode {
    let crossfade = match request
        .crossfade
        .map(ludion_engine::parse_crossfade)
        .transpose()
    {
        Ok(crossfade) => crossfade,
        Err(diagnostic) => return report(model, &[diagnostic]),
    };
    let out = request
        .out
        .map(Path::to_path_buf)
        .unwrap_or_else(|| default_sheet_out(model, request));

    let overrides = match parse_overrides(request.set) {
        Ok(overrides) => overrides,
        Err(diagnostics) => return report(model, &diagnostics),
    };
    let swept = match request.sweep.map(ludion_scene::parse_sweep).transpose() {
        Ok(swept) => swept,
        Err(diagnostic) => return report(model, &[diagnostic]),
    };

    let sheet = ludion_engine::render_model_sheet(&ModelSheetRequest {
        models: request.models,
        views: request.views,
        width: request.width,
        height: request.height,
        rig: request.rig,
        set: &overrides,
        sweep: swept
            .as_ref()
            .map(|(pointer, values)| ModelSweep { pointer, values }),
        true_size: request.true_size,
        clip: request.clip,
        time: request.time,
        strip: request.strip,
        crossfade,
        part: request.part,
        isolate: request.isolate,
        channel: request.channel,
    });

    match sheet {
        Ok(outcome) => {
            if let Err(diagnostic) = outcome.image.write_png(&out) {
                eprintln!("{diagnostic}");
                return ExitCode::FAILURE;
            }
            for applied in &outcome.applied {
                echo(applied);
            }
            if let (Some(sweep), Some((_, values))) = (&outcome.sweep, &swept) {
                println!(
                    "sweep {}: was {}, over {}",
                    sweep.pointer,
                    previous(sweep.original.as_ref()),
                    values.iter().map(compact).collect::<Vec<_>>().join(", ")
                );
            }
            if let Some(framing) = outcome.true_size {
                println!(
                    "true-size: {:.2} units tall -> {:.0} px of subject",
                    framing.world_height, framing.subject_px
                );
            }
            if let Some(clip) = &outcome.clip {
                print_clip_report(clip);
            }
            println!(
                "{}: rendered {}x{} on {}",
                out.display(),
                outcome.image.width,
                outcome.image.height,
                outcome.adapter
            );
            ExitCode::SUCCESS
        }
        Err(diagnostics) => report(model, &diagnostics),
    }
}

/// The per-phase gap analysis (`Plan/voxel-animation.md` decision 11), printed above the summary
/// line: one line per phase naming the instant it rendered at, the tick that is, and how many
/// connected components the posed model has — followed, when it has more than one, by the same
/// `connectivity owners:` attribution block `check-scene` prints over the rest pose. One `grep`
/// over a strip's output answers "does this clip pull the model apart, and where".
fn print_clip_report(clip: &ludion_engine::ClipOutcome) {
    if let Some(fade) = &clip.crossfade {
        println!(
            "crossfade {} -> {} over {}s, {} phases quantized to {} Hz ticks",
            fade.from,
            clip.name,
            fade.seconds as f32,
            clip.phases.len(),
            clip.hz,
        );
    }
    println!(
        "clip {}: duration {}, loop {}, {} phase{} quantized to {} Hz ticks",
        clip.name,
        clip.duration,
        clip.looped,
        clip.phases.len(),
        if clip.phases.len() == 1 { "" } else { "s" },
        clip.hz,
    );
    for (index, phase) in clip.phases.iter().enumerate() {
        print!("  phase {index}: t {} (tick {})", phase.time, phase.tick);
        if let Some(weight) = phase.weight {
            print!(", weight {weight:.3}");
        }
        if let Some(components) = phase.components {
            print!(
                ", {components} component{}",
                if components == 1 { "" } else { "s" }
            );
        }
        println!();
        if phase.strays.is_empty() {
            continue;
        }
        println!("    connectivity owners:");
        for stray in &phase.strays {
            let [x, y, z] = stray.sample;
            print!(
                "      stray sample ({x},{y},{z}) owned by {}",
                join_owners(&stray.owners)
            );
            if let Some(main) = &stray.main {
                let [mx, my, mz] = main.cell;
                print!(
                    "; nearest main-body cell ({mx},{my},{mz}) owned by {}",
                    join_owners(&main.owners)
                );
            }
            println!();
        }
    }
}

/// The comma-joined part names owning one cell, or `none` — `check_scene::join_part_names`'s
/// spelling, so the two attribution blocks read identically.
fn join_owners(owners: &[String]) -> String {
    if owners.is_empty() {
        "none".to_string()
    } else {
        owners.join(", ")
    }
}

/// The default `--out` for model and sprite mode: `<name>.sheet.png` in the current directory, where
/// `<name>` is the document's file stem with a trailing `.voxel`/`.mesh`/`.assembly`/`.sculpt`/
/// `.character`/`.prefab`/`.sprite` suffix stripped (`lantern.voxel.yaml` -> `lantern.sheet.png`, `ship.sprite.yaml` ->
/// `ship.sheet.png`). Atlas mode has its own name ([`default_atlas_out`]) because a sprite document
/// and the atlas it is packed into routinely share a stem, and one silently overwriting the other's
/// sheet would be the worst kind of default.
///
/// A posed render names what it shows instead, since a rest sheet and a pose of the same model are
/// different images: `<name>-<clip>-strip.png` for a strip, `<name>-<clip>-t<seconds>.png` for one
/// instant (`figure.assembly.yaml --clip wave --time 0.4` -> `figure-wave-t0.4.png`), and a
/// crossfade `<name>-<from>-<to>-crossfade.png`. The seconds
/// are the *requested* ones — the file name is chosen before the render quantizes them, and a name
/// that silently disagreed with the flag that produced it would be worse than one that rounds. A
/// part sheet likewise names its part: `idol.sculpt.yaml --part head` -> `idol-head.sheet.png`, and
/// `idol-head-isolated.sheet.png` under `--isolate`.
fn default_sheet_out(model: &Path, request: &Request) -> PathBuf {
    let stem = model
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("model")
        .to_string();
    let name = stem
        .strip_suffix(".voxel")
        .or_else(|| stem.strip_suffix(".mesh"))
        .or_else(|| stem.strip_suffix(".assembly"))
        .or_else(|| stem.strip_suffix(".sculpt"))
        .or_else(|| stem.strip_suffix(".character"))
        .or_else(|| stem.strip_suffix(".prefab"))
        .or_else(|| stem.strip_suffix(".sprite"))
        .unwrap_or(&stem);
    // A channel is a different picture of the same subject, so it never lands on the ordinary
    // sheet's name — the two are read side by side.
    if let Some(channel) = request.channel {
        return PathBuf::from(format!("{name}-{}.sheet.png", channel.name()));
    }
    // A malformed spec never reaches here: `run_model` refuses it first.
    if let Some(spec) = request
        .crossfade
        .and_then(|spec| ludion_engine::parse_crossfade(spec).ok())
    {
        return PathBuf::from(format!("{name}-{}-{}-crossfade.png", spec.from, spec.to));
    }
    match (request.clip, request.strip, request.part) {
        (Some(clip), Some(_), _) => PathBuf::from(format!("{name}-{clip}-strip.png")),
        (Some(clip), None, _) => PathBuf::from(format!(
            "{name}-{clip}-t{}.png",
            request.time.unwrap_or(0.0).max(0.0)
        )),
        (None, _, Some(part)) if request.isolate => {
            PathBuf::from(format!("{name}-{part}-isolated.sheet.png"))
        }
        (None, _, Some(part)) => PathBuf::from(format!("{name}-{part}.sheet.png")),
        _ => PathBuf::from(format!("{name}.sheet.png")),
    }
}

/// The default `--out` for atlas mode: `<name>.atlas-<kind>.png` in the current directory, where
/// `<name>` is the atlas document's file stem with a trailing `.atlas` stripped and `<kind>` names
/// the instrument (`ship.atlas.yaml` -> `ship.atlas-sheet.png`, `ship.atlas-strip.png`, …).
///
/// Deliberately not [`default_sheet_out`]'s name: `generate-atlas` mints `ship.atlas.yaml` from
/// `ship.sprite.yaml`, so the two documents sit in one directory under one stem, and a shared
/// default would have the second sheet silently replace the first. The four instruments are kept
/// apart from each other for the same reason — running all four writes four files.
fn default_atlas_out(atlas: &Path, kind: &str) -> PathBuf {
    let stem = atlas
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("atlas")
        .to_string();
    let name = stem.strip_suffix(".atlas").unwrap_or(&stem);
    PathBuf::from(format!("{name}.atlas-{kind}.png"))
}

/// `--sprite`: bakes the document and writes its contact sheet. Pure CPU — no adapter is requested,
/// so this answers on a machine that has none.
///
/// Everything the sheet burns in is also printed: the effects that ran and what each `--set`
/// changed, so a session that never opens the image still reads what produced it.
fn run_sprite(request: &Request, doc: &Path) -> ExitCode {
    let scale = match sprite_scale(request.scale) {
        Ok(scale) => scale,
        Err(message) => {
            return crate::diagnostic::usage_error(message);
        }
    };
    let out = request
        .out
        .map(Path::to_path_buf)
        .unwrap_or_else(|| default_sheet_out(doc, request));

    let overrides = match parse_overrides(request.set) {
        Ok(overrides) => overrides,
        Err(diagnostics) => return report(doc, &diagnostics),
    };
    let swept = match request.sweep.map(ludion_scene::parse_sweep).transpose() {
        Ok(swept) => swept,
        Err(diagnostic) => return report(doc, &[diagnostic]),
    };

    match ludion_engine::render_sprite_sheet(&SpriteSheetRequest {
        doc: doc.to_path_buf(),
        scale,
        set: overrides,
        sweep: swept,
    }) {
        Ok(outcome) => {
            if let Err(diagnostic) = write_sheet(&outcome.image, &out) {
                eprintln!("{diagnostic}");
                return ExitCode::FAILURE;
            }
            println!(
                "wrote {} ({}x{})",
                out.display(),
                outcome.image.width,
                outcome.image.height
            );
            for effect in &outcome.effects {
                println!("  effect {effect}");
            }
            for applied in &outcome.applied {
                println!(
                    "  set {}: {} -> {}",
                    applied.pointer,
                    previous(applied.from.as_ref()),
                    compact(&applied.to)
                );
            }
            ExitCode::SUCCESS
        }
        Err(diagnostics) => report(doc, &diagnostics),
    }
}

/// `--atlas`: the packing overview, or — with `--clip` — one of the document's flipbooks. Pure CPU,
/// like its sprite twin.
///
/// The clip flags are checked at the engine seam first, so an orphaned `--time`/`--strip`/`--onion`
/// is refused before either branch reads a file.
fn run_atlas(request: &Request, doc: &Path) -> ExitCode {
    if let Err(diagnostics) = ludion_engine::check_atlas_clip_flags(
        request.clip,
        request.time,
        request.strip,
        request.onion,
    ) {
        return report(doc, &diagnostics);
    }
    match request.clip {
        Some(clip) => run_atlas_clip(request, doc, clip),
        None => run_atlas_sheet(doc, request.out),
    }
}

/// The default `--out` for font mode: `<name>.font-sheet.png` in the current directory, where
/// `<name>` is the font document's file stem with a trailing `.font` stripped.
fn default_font_out(font: &Path) -> PathBuf {
    let stem = font
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("font")
        .to_string();
    let name = stem.strip_suffix(".font").unwrap_or(&stem);
    PathBuf::from(format!("{name}.font-sheet.png"))
}

/// `--font`: the specimen sheet of a `kind: font` document. Pure CPU, nothing to override.
fn run_font(doc: &Path, out: Option<&Path>) -> ExitCode {
    let out = out
        .map(Path::to_path_buf)
        .unwrap_or_else(|| default_font_out(doc));
    match ludion_engine::render_font_sheet(doc) {
        Ok(image) => {
            if let Err(diagnostic) = write_sheet(&image, &out) {
                eprintln!("{diagnostic}");
                return ExitCode::FAILURE;
            }
            println!("wrote {} ({}x{})", out.display(), image.width, image.height);
            ExitCode::SUCCESS
        }
        Err(diagnostics) => report(doc, &diagnostics),
    }
}

/// The packing overview: the atlas document is already baked, so there is nothing to override and
/// nothing to echo.
fn run_atlas_sheet(doc: &Path, out: Option<&Path>) -> ExitCode {
    let out = out
        .map(Path::to_path_buf)
        .unwrap_or_else(|| default_atlas_out(doc, "sheet"));
    match ludion_engine::render_atlas_sheet(doc) {
        Ok(image) => {
            if let Err(diagnostic) = write_sheet(&image, &out) {
                eprintln!("{diagnostic}");
                return ExitCode::FAILURE;
            }
            println!("wrote {} ({}x{})", out.display(), image.width, image.height);
            ExitCode::SUCCESS
        }
        Err(diagnostics) => report(doc, &diagnostics),
    }
}

/// One flipbook of the atlas, as an instant, a strip of phases or an onion skin.
///
/// Everything the image burns in is printed too — the clip's shape and every phase or overlaid
/// entry — so a session that never opens the PNG still reads what produced it.
fn run_atlas_clip(request: &Request, doc: &Path, clip: &str) -> ExitCode {
    let scale = match sprite_scale(request.scale) {
        Ok(scale) => scale,
        Err(message) => {
            return crate::diagnostic::usage_error(message);
        }
    };
    let out = request
        .out
        .map(Path::to_path_buf)
        .unwrap_or_else(|| default_atlas_out(doc, atlas_clip_kind(request)));

    match ludion_engine::render_atlas_clip(&ludion_engine::AtlasClipRequest {
        atlas: doc.to_path_buf(),
        clip: clip.to_string(),
        time: request.time,
        strip: request.strip,
        onion: request.onion,
        scale,
    }) {
        Ok(outcome) => {
            if let Err(diagnostic) = write_sheet(&outcome.image, &out) {
                eprintln!("{diagnostic}");
                return ExitCode::FAILURE;
            }
            println!(
                "wrote {} ({}x{})",
                out.display(),
                outcome.image.width,
                outcome.image.height
            );
            println!(
                "  clip {}  {} frames  {} fps  {:.3}s  {}",
                outcome.clip,
                outcome.entries,
                outcome.fps,
                outcome.duration,
                if outcome.looped { "loop" } else { "one-shot" }
            );
            for phase in &outcome.phases {
                println!("  {phase}");
            }
            ExitCode::SUCCESS
        }
        Err(diagnostics) => report(doc, &diagnostics),
    }
}

/// Which of the three clip instruments a request asks for, as the defaulted `--out` names it. The
/// precedence is the module doc's: an onion skin, else a strip, else the single instant.
fn atlas_clip_kind(request: &Request) -> &'static str {
    if request.onion {
        "onion"
    } else if request.strip.is_some() {
        "strip"
    } else {
        "frame"
    }
}

/// Resolves `--scale` as a sprite sheet's game scale: whole sheet pixels per texel, `1..=16`,
/// defaulting to 1. A fraction is refused rather than rounded — the flag names how many pixels one
/// texel occupies, and there is no such thing as two and a half of them.
fn sprite_scale(scale: Option<f32>) -> Result<u32, String> {
    let Some(scale) = scale else {
        return Ok(1);
    };
    if scale.fract() != 0.0 || scale < 1.0 || scale > MAX_SPRITE_SCALE as f32 {
        return Err(format!(
            "invalid value for '--scale <SCALE>': with --sprite (and with --atlas --clip) it is \
             the game scale in whole sheet pixels per texel, so it must be a whole number in \
             1..={MAX_SPRITE_SCALE} — got '{scale}'"
        ));
    }
    Ok(scale as u32)
}

/// Writes a CPU-composed sheet to `path`. `ludion_assets::Image` is the sheet harnesses' output and
/// carries no writer of its own (unlike the renderer's `RenderedImage`), so the deterministic
/// encoder is called directly here.
fn write_sheet(image: &ludion_assets::Image, path: &Path) -> Result<(), Diagnostic> {
    let bytes = ludion_assets::encode_png(image.width, image.height, &image.rgba)
        .map_err(|error| png_failure(format!("could not encode `{}`: {error}", path.display())))?;
    ludion_core::atomic::write_atomic(path, bytes)
        .map_err(|error| png_failure(format!("could not write `{}`: {error}", path.display())))
}

fn png_failure(detail: String) -> Diagnostic {
    Diagnostic::new(code::RENDER_PNG_IO, detail)
        .with_help("check the output location is writable and that there is free disk space")
}
