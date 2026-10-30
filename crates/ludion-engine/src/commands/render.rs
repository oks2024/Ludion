//! The `render.*` handlers.
//!
//! Two shapes live here. The *render* verbs (`screenshot`, `dump_graph`, `dump_timings`, `bounds`)
//! reuse the Phase 2 lockstep render path: the graph dump and bounds are CPU-only, while the
//! screenshot and timings need an adapter — on an adapterless machine they return `L2003`, and
//! callers graceful-skip on it. The *session levers* (`gi_rebake`, `size`, `view`, `debug`) touch no world and
//! render nothing; each is one verb that both sets and queries, so empty params are a pure read.

use ludion_core::codes::code;
use ludion_core::diagnostics::Diagnostic;
use ludion_protocol::ResponseBody;
use ludion_protocol::command::{
    CameraChoice, GiRebakeMode as WireGiRebakeMode, RenderBoundsParams, RenderDebugParams,
    RenderDebugView, RenderGiRebakeParams, RenderScreenshotParams, RenderSizeParams,
    RenderViewParams,
};
use ludion_renderer::DebugView;
use serde_json::Value;

use crate::DebugCategories;
use crate::commands::collapse;
use crate::engine::{Engine, GiRebakeMode};
use crate::render_size::{MAX_HEIGHT, MAX_WIDTH};
use crate::render_view;

/// `render.screenshot` — render the current world at the requested (or default) resolution and return
/// it as a base64 PNG. Deterministic on a given adapter (two calls at the same frame are byte-equal).
///
/// `camera` chooses the viewpoint **per call** and defaults to the scene's own, so an agent's
/// screenshot shows the game's framing whether or not a human is flying the stream through
/// `render.view`. A screenshot already takes the blocking render path, so choosing its camera here
/// costs no second per-frame render.
pub(crate) fn screenshot(
    engine: &mut Engine,
    params: RenderScreenshotParams,
) -> Result<ResponseBody, Diagnostic> {
    let (default_w, default_h) = engine.render_size();
    let width = params.width.unwrap_or(default_w);
    let height = params.height.unwrap_or(default_h);
    // The same bounds as `render.size`, checked before anything renders: an extent no render can
    // honor is a protocol error, not a GPU one.
    let help = "give `width` and `height` as whole pixel counts inside the range, or omit them to \
                use the session's `render.size`";
    axis("render.screenshot", width, "width", MAX_WIDTH, help)?;
    axis("render.screenshot", height, "height", MAX_HEIGHT, help)?;

    let png = engine.render_png_through(width, height, params.camera == CameraChoice::View)?;
    Ok(ResponseBody::Screenshot {
        width,
        height,
        format: "png".to_string(),
        data: crate::base64::encode(&png),
        // Read after the render, so it describes the image in this very reply.
        stale: engine.gi_stale(),
    })
}

/// `render.gi_rebake` — move this host's GI rebake lever, ask for one rebake, or read both.
///
/// Every field of the request is optional and the reply is the same either way, which is what makes
/// one verb enough: an omitted `mode` leaves the lever alone, so empty params are a pure read. Pure
/// session state — nothing here touches the world, so no frame index moves and nothing is recorded.
pub(crate) fn gi_rebake(
    engine: &mut Engine,
    params: RenderGiRebakeParams,
) -> Result<ResponseBody, Diagnostic> {
    if let Some(mode) = params.mode {
        engine.set_gi_rebake(match mode {
            WireGiRebakeMode::Auto => GiRebakeMode::Auto,
            WireGiRebakeMode::Hold => GiRebakeMode::Hold,
        });
    }
    if params.rebake {
        engine.gi_rebake_now();
    }
    Ok(ResponseBody::GiRebake {
        mode: match engine.gi_rebake_mode() {
            GiRebakeMode::Auto => WireGiRebakeMode::Auto,
            GiRebakeMode::Hold => WireGiRebakeMode::Hold,
        },
        // The rebake this call may have requested runs on the *next* frame, so what is reported is
        // still the last produced frame's staleness — the honest answer, and the one a badge wants.
        stale: engine.gi_stale(),
    })
}

/// `render.size` — set this session's render extent, or read it.
///
/// The extent is the resolution every live stream frame is produced at, and the default a
/// dimensionless `render.screenshot` uses. Both fields are optional and the reply is the same either
/// way, which is what makes one verb enough: omitting both is a pure read. They are optional
/// *together* — an extent is a pair, so a half-named one is refused rather than half-applied.
///
/// No frame index moves. The extent is sim input (a game reads it as `RenderExtent`), so an applied
/// resize is recorded, and the read form is not. The live stream follows from the next frame ([`Engine::set_render_size`]).
///
/// # Errors
/// `L3025` when exactly one axis is present, or when either is zero or past [`MAX_WIDTH`] ×
/// [`MAX_HEIGHT`]; `L2012` when the extent at the host's render scale is past the renderer's limit.
/// A refused request leaves the extent exactly as it was.
pub(crate) fn size(
    engine: &mut Engine,
    params: RenderSizeParams,
) -> Result<ResponseBody, Diagnostic> {
    if let Some((width, height)) = requested_extent(&params)? {
        // The scale is host state too: an extent that fits at scale 1 may not at scale 4.
        engine.frame_size(width, height)?;
        engine.set_render_size(width, height);
    }
    let (width, height) = engine.render_size();
    Ok(ResponseBody::RenderSize { width, height })
}

/// Apply a `render.size` a replay recorded: the `L3025` validity check and nothing else.
///
/// The live verb also refuses an extent past the renderer's limit at this host's render scale
/// (`L2012`), but that scale is machine-local — a recorded extent must not be refused because the
/// replaying machine renders at a different scale. A render at that extent still validates when it
/// renders.
///
/// # Errors
/// `L3025` as [`size`] gives it.
pub(crate) fn apply_recorded(
    engine: &mut Engine,
    params: &RenderSizeParams,
) -> Result<(), Diagnostic> {
    if let Some((width, height)) = requested_extent(params)? {
        engine.set_render_size(width, height);
    }
    Ok(())
}

/// The `L3025`-checked extent a `render.size` names: `None` for the read form. Both axes are
/// validated before either is applied, so a refused request leaves the extent exactly as it was
/// rather than half-moved.
fn requested_extent(params: &RenderSizeParams) -> Result<Option<(u32, u32)>, Diagnostic> {
    match (params.width, params.height) {
        (Some(width), Some(height)) => {
            let help = size_help();
            let width = axis("render.size", width, "width", MAX_WIDTH, &help)?;
            let height = axis("render.size", height, "height", MAX_HEIGHT, &help)?;
            Ok(Some((width, height)))
        }
        (None, None) => Ok(None),
        // The two plausible guesses for a missing axis — keep the old one, or preserve the aspect
        // ratio — give different pictures, and neither is what the client asked for.
        (present, _) => {
            let missing = if present.is_some() { "height" } else { "width" };
            Err(invalid_extent(
                format!("render.size named one axis without the other: `{missing}` is missing"),
                missing,
                &size_help(),
            ))
        }
    }
}

/// One axis of a requested extent for `verb`: a pixel count inside `1..=max`.
fn axis(verb: &str, pixels: u32, name: &str, max: u32, help: &str) -> Result<u32, Diagnostic> {
    if (1..=max).contains(&pixels) {
        Ok(pixels)
    } else {
        Err(invalid_extent(
            format!("{verb} asked for a {name} of {pixels} pixels; the range is 1..={max}"),
            name,
            help,
        ))
    }
}

/// An `L3025` pointing at the offending axis, carrying the help the verb needs.
fn invalid_extent(message: String, axis: &str, help: &str) -> Diagnostic {
    Diagnostic::new(code::PROTOCOL_RENDER_SIZE_INVALID, message)
        .with_path(format!("/params/{axis}"))
        .with_help(help)
}

/// The help every `render.size` refusal carries: the pair rule, the range, and the read form.
fn size_help() -> String {
    format!(
        "send both axes as whole pixel counts inside 1..={MAX_WIDTH} x 1..={MAX_HEIGHT} \
         (`{{\"width\": 960, \"height\": 540}}`), or send `{{}}` to read the extent without \
         changing it; the reply carries the extent in force either way"
    )
}

/// `render.view` — fly this session's *stream* to a free camera pose, clear that override, or read it.
///
/// Three request forms, three meanings, which is what `pose` being a double option buys: absent is a
/// pure read, an explicit `null` clears, and a pose sets. The reply is the same either way — the
/// override in force once the command applied — so a client never has to assume its request was taken
/// verbatim.
///
/// Pure host render state: nothing here touches the world, so no frame index moves, no `WorldHash`
/// changes, and nothing is recorded. The live stream follows from the next frame
/// ([`Engine::set_view_override`]).
///
/// # Errors
/// `L3032` when the pose carries a non-finite number or a field of view outside `0..180`. A refused
/// request leaves the override exactly as it was.
pub(crate) fn view(
    engine: &mut Engine,
    params: RenderViewParams,
) -> Result<ResponseBody, Diagnostic> {
    if let Some(requested) = params.pose {
        // Validated before anything is written, so a refused pose leaves the human looking at the
        // last good one rather than at nothing.
        let accepted = requested.map(render_view::accept).transpose()?;
        engine.set_view_override(accepted);
    }
    Ok(ResponseBody::RenderView {
        pose: engine.view_override().copied(),
    })
}

/// `render.debug` — read or set which debug-draw categories this host renders.
///
/// Absent `categories` is a pure read; a list replaces the enabled set (`[]` is none, `"all"` is
/// every category). Likewise `view`: absent keeps the debug view, `null` clears it, a name sets
/// it. The reply is the same either way: `enabled` once the command applied, and `drawn`,
/// the categories the last completed tick emitted lines for. Any string is a valid category, so there
/// is nothing to refuse.
///
/// Pure host render state, like [`view`]: no frame index moves, no `WorldHash` changes, nothing is
/// recorded ([`Engine::set_debug_draw`], [`Engine::set_debug_view`]).
pub(crate) fn debug(
    engine: &mut Engine,
    params: RenderDebugParams,
) -> Result<ResponseBody, Diagnostic> {
    if let Some(names) = params.categories {
        engine.set_debug_draw(DebugCategories::from_names(names));
    }
    if let Some(view) = params.view {
        engine.set_debug_view(view.map(renderer_view));
    }
    Ok(ResponseBody::RenderDebug {
        enabled: engine.debug_draw().names(),
        drawn: engine
            .debug_draw_drawn()
            .into_iter()
            .map(String::from)
            .collect(),
        view: engine.debug_view().map(wire_view),
    })
}

fn renderer_view(view: RenderDebugView) -> DebugView {
    match view {
        RenderDebugView::SsrConfidence => DebugView::SsrConfidence,
        RenderDebugView::SsrFootprint => DebugView::SsrFootprint,
        RenderDebugView::ProbeIndex => DebugView::ProbeIndex,
        RenderDebugView::LocalShadowAtlas => DebugView::LocalShadowAtlas,
        RenderDebugView::SunCascades => DebugView::SunCascades,
    }
}

fn wire_view(view: DebugView) -> RenderDebugView {
    match view {
        DebugView::SsrConfidence => RenderDebugView::SsrConfidence,
        DebugView::SsrFootprint => RenderDebugView::SsrFootprint,
        DebugView::ProbeIndex => RenderDebugView::ProbeIndex,
        DebugView::LocalShadowAtlas => RenderDebugView::LocalShadowAtlas,
        DebugView::SunCascades => RenderDebugView::SunCascades,
    }
}

/// `render.dump_graph` — the render graph structure as JSON (no GPU needed).
///
/// The extract is CPU-only, so the chain the live world's content requires is derivable without an
/// adapter — an adapterless machine still answers, and the reported shape is the one a screenshot of
/// this world would actually schedule — masked by this host's quality, like
/// `dump_scene_graph_json`.
pub(crate) fn dump_graph(engine: &mut Engine) -> Result<ResponseBody, Diagnostic> {
    let (width, height) = engine.render_size();
    let scene = engine
        .extract_render_scene(width, height)
        .map_err(collapse)?;
    let quality = engine.quality();
    let chain =
        ludion_renderer::PostChain::for_scene(&scene).masked(&quality.quality, &quality.params);
    let json = ludion_renderer::dump_frame_graph_json(engine.frame_size(width, height)?, chain)?;
    let graph = parse_json(&json, "render graph dump")?;
    Ok(ResponseBody::GraphDump { graph })
}

/// `render.bounds` — the world AABB + screen rect of every renderable entity under the current
/// camera. Pure CPU (extract + projection math, no GPU): it answers even on an adapterless machine,
/// and its geometry comes from the same [`place`](crate::extract::place) path a real render sizes
/// with, so a reported bound can never disagree with what would be drawn.
pub(crate) fn bounds(
    engine: &mut Engine,
    params: RenderBoundsParams,
) -> Result<ResponseBody, Diagnostic> {
    let (render_size, entities) = engine.render_bounds(params.camera == CameraChoice::View)?;
    Ok(ResponseBody::RenderBounds {
        render_size,
        entities,
    })
}

/// `render.dump_timings` — the last frame's per-pass GPU timings as JSON (renders once with
/// timestamp queries; needs a GPU adapter).
pub(crate) fn dump_timings(engine: &mut Engine) -> Result<ResponseBody, Diagnostic> {
    let (width, height) = engine.render_size();
    let scene = engine
        .extract_render_scene(width, height)
        .map_err(collapse)?;
    let size = engine.frame_size(width, height)?;
    let renderer = engine.renderer()?;
    let json = renderer.dump_timings_json(&scene, size)?;
    let timings = parse_json(&json, "render timings dump")?;
    Ok(ResponseBody::TimingsDump { timings })
}

/// Parses a JSON string the renderer produced into a value (a should-not-fail guard around the
/// renderer's own JSON output).
fn parse_json(json: &str, what: &str) -> Result<Value, Diagnostic> {
    serde_json::from_str(json).map_err(|error| {
        Diagnostic::new(
            code::RENDER_PNG_IO,
            format!("the {what} was not valid JSON: {error}"),
        )
    })
}

#[cfg(test)]
mod tests {
    use ludion_protocol::command::ViewPose;

    use super::*;

    fn engine() -> Engine {
        let mut engine = Engine::new();
        engine
            .load_inline_scene("format: 1\nseed: 7\nentities: []\n")
            .unwrap();
        engine
    }

    fn extent(params: RenderSizeParams) -> (Engine, ResponseBody) {
        let mut engine = engine();
        let body = size(&mut engine, params).expect("the extent is accepted");
        (engine, body)
    }

    fn refused(engine: &mut Engine, params: RenderSizeParams) -> Diagnostic {
        let before = engine.render_size();
        let diagnostic = size(engine, params).expect_err("the extent must be refused");
        assert_eq!(diagnostic.code, code::PROTOCOL_RENDER_SIZE_INVALID);
        assert_eq!(
            engine.render_size(),
            before,
            "a refused request leaves the extent exactly as it was",
        );
        diagnostic
    }

    /// Empty params are a pure read: nothing moves, and the reply still carries the extent in force.
    /// This is the half that makes one verb enough.
    #[test]
    fn empty_params_report_the_extent_without_changing_it() {
        let (engine, body) = extent(RenderSizeParams::default());
        let (width, height) = engine.render_size();
        assert_eq!(body, ResponseBody::RenderSize { width, height });
    }

    /// Both axes present moves the extent, and the reply reports the *resulting* pair — so a client
    /// never has to assume its request was taken verbatim.
    #[test]
    fn both_axes_set_the_extent_and_the_reply_reports_the_result() {
        let (engine, body) = extent(RenderSizeParams {
            width: Some(960),
            height: Some(540),
        });
        assert_eq!(engine.render_size(), (960, 540));
        assert_eq!(
            body,
            ResponseBody::RenderSize {
                width: 960,
                height: 540,
            }
        );
    }

    /// An extent is a pair. Naming one axis alone is refused rather than guessed at, and the
    /// diagnostic points at the axis that is missing.
    #[test]
    fn a_half_named_extent_is_refused_pointing_at_the_missing_axis() {
        let mut engine = engine();
        for (params, missing) in [
            (
                RenderSizeParams {
                    width: Some(960),
                    height: None,
                },
                "height",
            ),
            (
                RenderSizeParams {
                    width: None,
                    height: Some(540),
                },
                "width",
            ),
        ] {
            let diagnostic = refused(&mut engine, params);
            assert_eq!(
                diagnostic.path.as_deref(),
                Some(format!("/params/{missing}").as_str()),
            );
        }
    }

    /// Zero and past-the-ceiling are the same refusal, per axis, with the offending axis named — the
    /// engine-side twin of `L0017`'s bounds on the startup flag.
    #[test]
    fn an_unrenderable_axis_is_refused_naming_that_axis() {
        let mut engine = engine();
        for (params, axis, bound) in [
            (
                RenderSizeParams {
                    width: Some(0),
                    height: Some(540),
                },
                "width",
                MAX_WIDTH,
            ),
            (
                RenderSizeParams {
                    width: Some(960),
                    height: Some(0),
                },
                "height",
                MAX_HEIGHT,
            ),
            (
                RenderSizeParams {
                    width: Some(MAX_WIDTH + 1),
                    height: Some(540),
                },
                "width",
                MAX_WIDTH,
            ),
            (
                RenderSizeParams {
                    width: Some(960),
                    height: Some(MAX_HEIGHT + 1),
                },
                "height",
                MAX_HEIGHT,
            ),
        ] {
            let diagnostic = refused(&mut engine, params);
            assert_eq!(
                diagnostic.path.as_deref(),
                Some(format!("/params/{axis}").as_str()),
            );
            assert!(
                diagnostic.message.contains(&format!("1..={bound}")),
                "the message names the axis's own bound: {}",
                diagnostic.message,
            );
        }
        // The taller axis's lower ceiling never bounds the wider one.
        let (engine, _) = extent(RenderSizeParams {
            width: Some(MAX_WIDTH),
            height: Some(MAX_HEIGHT),
        });
        assert_eq!(engine.render_size(), (MAX_WIDTH, MAX_HEIGHT));
        let _ = engine;
    }

    /// Every refusal carries the same help — the pair rule, the range, and the read form — so a
    /// client that sent any of the wrong shapes learns the right one from the diagnostic alone.
    #[test]
    fn every_refusal_carries_the_pair_rule_and_the_read_form_as_help() {
        let mut engine = engine();
        for params in [
            RenderSizeParams {
                width: Some(960),
                height: None,
            },
            RenderSizeParams {
                width: Some(0),
                height: Some(0),
            },
        ] {
            let help = refused(&mut engine, params)
                .help
                .expect("the family carries help");
            assert!(help.contains("both axes"), "{help}");
            assert!(help.contains("{}"), "the read form is named: {help}");
        }
    }

    fn pose() -> ViewPose {
        ViewPose {
            translation: [1.0, 2.0, 3.0],
            rotation: [-15.0, 45.0, 0.0],
            fov_y_deg: Some(50.0),
        }
    }

    fn flown(engine: &mut Engine, pose: Option<Option<ViewPose>>) -> ResponseBody {
        view(engine, RenderViewParams { pose }).expect("the pose is accepted")
    }

    /// Empty params are a pure read: nothing moves, and the reply still carries the override in
    /// force. The half that makes one verb enough, as `render.size` has it.
    #[test]
    fn empty_params_report_the_override_without_changing_it() {
        let mut engine = engine();
        assert_eq!(
            flown(&mut engine, None),
            ResponseBody::RenderView { pose: None },
        );
        assert_eq!(engine.view_override(), None);
    }

    /// A pose sets the override and an explicit `null` clears it — the two states the double option
    /// exists to tell apart — and the reply reports the *resulting* one either way.
    #[test]
    fn a_pose_sets_the_override_and_an_explicit_null_clears_it() {
        let mut engine = engine();
        assert_eq!(
            flown(&mut engine, Some(Some(pose()))),
            ResponseBody::RenderView { pose: Some(pose()) },
        );
        assert_eq!(engine.view_override(), Some(&pose()));

        assert_eq!(
            flown(&mut engine, Some(None)),
            ResponseBody::RenderView { pose: None },
        );
        assert_eq!(
            engine.view_override(),
            None,
            "clearing returns the stream to the scene's own camera",
        );
    }

    /// A refused pose leaves the override exactly as it was, which is what makes the verb safe to
    /// drive from a gesture loop: one bad frame of input leaves the human on the last good pose.
    #[test]
    fn a_refused_pose_leaves_the_override_as_it_was() {
        let mut engine = engine();
        flown(&mut engine, Some(Some(pose())));
        let mut bad = pose();
        bad.translation[1] = f32::NAN;
        let diagnostic = view(
            &mut engine,
            RenderViewParams {
                pose: Some(Some(bad)),
            },
        )
        .expect_err("a non-finite pose must be refused");
        assert_eq!(diagnostic.code, code::PROTOCOL_RENDER_VIEW_INVALID);
        assert_eq!(engine.view_override(), Some(&pose()));
    }

    /// The override is a pure render concern: flying it moves no frame index and no `WorldHash`, so
    /// a session a human is flying simulates exactly as one nobody touched.
    #[test]
    fn flying_the_view_moves_neither_the_frame_nor_the_hash() {
        let mut engine = engine();
        let frame = engine.frame();
        let hash = engine.world_hash().expect("the world hashes");
        flown(&mut engine, Some(Some(pose())));
        assert_eq!(engine.frame(), frame);
        assert_eq!(engine.world_hash().expect("the world hashes"), hash);
    }

    fn debug_with(engine: &mut Engine, categories: Option<&[&str]>) -> (Vec<String>, Vec<String>) {
        let categories = categories.map(|names| names.iter().map(|n| n.to_string()).collect());
        match debug(
            engine,
            RenderDebugParams {
                categories,
                view: None,
            },
        )
        .expect("any category is accepted")
        {
            ResponseBody::RenderDebug { enabled, drawn, .. } => (enabled, drawn),
            other => panic!("render.debug answered {other:?}"),
        }
    }

    /// Empty params read; a list sets, `all` enables everything, `[]` clears — and every form
    /// answers with the set in force once the command applied.
    #[test]
    fn debug_reads_sets_and_clears_the_enabled_categories() {
        let mut engine = engine();
        assert_eq!(debug_with(&mut engine, None).0, Vec::<String>::new());

        assert_eq!(
            debug_with(&mut engine, Some(&["path", "motor"])).0,
            ["motor", "path"]
        );
        assert_eq!(
            debug_with(&mut engine, None).0,
            ["motor", "path"],
            "a read changes nothing"
        );

        assert_eq!(debug_with(&mut engine, Some(&["path", "all"])).0, ["all"]);
        assert_eq!(*engine.debug_draw(), DebugCategories::All);

        assert!(debug_with(&mut engine, Some(&[])).0.is_empty());
        assert_eq!(*engine.debug_draw(), DebugCategories::None);
    }

    /// Like the view override, the category set is a pure render concern.
    #[test]
    fn setting_debug_categories_moves_neither_the_frame_nor_the_hash() {
        let mut engine = engine();
        let frame = engine.frame();
        let hash = engine.world_hash().expect("the world hashes");
        debug_with(&mut engine, Some(&["all"]));
        assert_eq!(engine.frame(), frame);
        assert_eq!(engine.world_hash().expect("the world hashes"), hash);
    }

    /// The extent is a pure render concern: setting it moves no frame index and no `WorldHash`, so a
    /// resized session simulates exactly as an unresized one does.
    #[test]
    fn resizing_moves_neither_the_frame_nor_the_hash() {
        let mut engine = engine();
        let frame = engine.frame();
        let hash = engine.world_hash().expect("the world hashes");
        size(
            &mut engine,
            RenderSizeParams {
                width: Some(640),
                height: Some(360),
            },
        )
        .expect("the extent is accepted");
        assert_eq!(engine.frame(), frame);
        assert_eq!(engine.world_hash().expect("the world hashes"), hash);
    }
}
