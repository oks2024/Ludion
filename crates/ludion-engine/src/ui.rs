//! Pure screen-space UI geometry: anchor resolution and text measurement.
//!
//! This module has no ECS/asset dependencies and does no IO — the geometry is plain arithmetic
//! (`+ - * /` only, no trig, no transcendentals) and [`UiList`] navigation is a pure function of the
//! component and one frame's input edges, so it is safe to call from both
//! [`extract`](crate::extract) (engine-side, building [`ludion_renderer::UiInstance`]s) and sim-side
//! game code doing mouse hit-testing (re-exported as `ludion::math`). See `Plan/phase-14-ui.md`
//! decisions 2–3 for the coordinate model this implements and `Plan/ui-widgets.md` decisions 2 and 4
//! for the list.
//!
//! # Faces
//!
//! Every measurement helper takes the resolved [`FontFace`] first, matching `ludion_font::layout`.
//! A game gets it from the `FontLibrary` resource —
//! `world.resource::<FontLibrary>().cloned().unwrap_or_default()`, then `fonts.face(list.font.as_deref())`
//! — so the block it measures is the block the component's named face lays out.
//!
//! # Coordinate model
//!
//! UI is authored against a virtual **reference canvas**, always `1080` units tall and
//! `1080 * aspect` units wide (`aspect = target_width / target_height`), origin **top-left**, **+Y
//! down**. `scale = target_height / 1080` maps canvas units to pixels uniformly, so one authored
//! `offset`/`size` fits every output resolution.
//!
//! [`Anchor`] names both the canvas point an element is placed at *and*
//! the element's own pivot — the same fraction of the canvas and of the element's own extent. The
//! resolved top-left, in canvas units, is:
//!
//! ```text
//! canvas_point = fraction * canvas_size
//! top_left     = canvas_point + offset - fraction * element_size
//! ```
//!
//! then `top_left * scale` and `element_size * scale` give the pixel-space rect.

use std::borrow::Cow;
use std::ops::Range;

use crate::components::{Anchor, UiList, UiListItem, UiRect, UiText};
pub use ludion_font::FontFace;
use ludion_font::{GlyphQuad, TextLayout, YAxis, fold_line_breaks, layout};
use ludion_sim::{GamepadButton, InputState, Key};

/// The reference canvas is always this many units tall; width is `CANVAS_HEIGHT * aspect`.
const CANVAS_HEIGHT: f32 = 1080.0;

/// The reference canvas width, in canvas units, for a `viewport` (width, height) in pixels —
/// `1080 * aspect`, guarding a zero height the same way [`ludion_renderer::aspect_ratio`] does.
fn canvas_width(viewport: (u32, u32)) -> f32 {
    let (w, h) = viewport;
    let aspect = if h == 0 { 1.0 } else { w as f32 / h as f32 };
    CANVAS_HEIGHT * aspect
}

/// Canvas units → pixels: `target_height / 1080`, guarding a zero height (falls back to `1.0`, the
/// same degenerate-viewport convention as [`canvas_width`]).
fn pixel_scale(viewport: (u32, u32)) -> f32 {
    let h = viewport.1;
    if h == 0 {
        1.0
    } else {
        h as f32 / CANVAS_HEIGHT
    }
}

/// The `(x, y)` fraction of the canvas — and of an element's own extent — that `anchor` names. E.g.
/// [`Anchor::BottomRight`] is `(1, 1)`: the canvas's bottom-right corner, and the element's own
/// bottom-right corner (its pivot).
fn anchor_fraction(anchor: Anchor) -> [f32; 2] {
    match anchor {
        Anchor::TopLeft => [0.0, 0.0],
        Anchor::TopCenter => [0.5, 0.0],
        Anchor::TopRight => [1.0, 0.0],
        Anchor::CenterLeft => [0.0, 0.5],
        Anchor::Center => [0.5, 0.5],
        Anchor::CenterRight => [1.0, 0.5],
        Anchor::BottomLeft => [0.0, 1.0],
        Anchor::BottomCenter => [0.5, 1.0],
        Anchor::BottomRight => [1.0, 1.0],
    }
}

/// Resolves an anchored UI element to a pixel-space rect `[x, y, w, h]` (top-left origin, +Y down).
///
/// `anchor` names both the canvas point (`anchor_fraction(anchor) * canvas_size`) and the element's
/// own pivot (the same fraction of `size`); `offset` is canvas units from that canvas point to that
/// element point. Pure arithmetic — no trig, no transcendentals — so it is bit-exact across
/// platforms.
///
/// # Example
///
/// ```
/// use ludion_engine::components::Anchor;
/// use ludion_engine::ui::resolve_ui_rect;
/// use ludion_font::FontFace;
///
/// // A BottomRight rect with zero offset has its bottom-right corner exactly on the viewport
/// // corner.
/// let rect = resolve_ui_rect(Anchor::BottomRight, [0.0, 0.0], [64.0, 32.0], (1920, 1080));
/// assert_eq!(rect[0] + rect[2], 1920.0);
/// assert_eq!(rect[1] + rect[3], 1080.0);
/// ```
pub fn resolve_ui_rect(
    anchor: Anchor,
    offset: [f32; 2],
    size: [f32; 2],
    viewport: (u32, u32),
) -> [f32; 4] {
    let canvas_size = [canvas_width(viewport), CANVAS_HEIGHT];
    let fraction = anchor_fraction(anchor);
    let canvas_point = [fraction[0] * canvas_size[0], fraction[1] * canvas_size[1]];
    let top_left = [
        canvas_point[0] + offset[0] - fraction[0] * size[0],
        canvas_point[1] + offset[1] - fraction[1] * size[1],
    ];
    let scale = pixel_scale(viewport);
    [
        top_left[0] * scale,
        top_left[1] * scale,
        size[0] * scale,
        size[1] * scale,
    ]
}

/// Resolves a [`UiRect`] to its pixel-space rect `[x, y, w, h]`: the whole render target
/// (`[0, 0, width, height]`) when `fill` is set, ignoring `anchor`, `offset` and `size`; otherwise
/// [`resolve_ui_rect`]. The one resolve every reader of a rect's bounds uses — the overlay pass and a
/// game's mouse hit test see the same rect.
///
/// # Example
///
/// ```
/// use ludion_engine::components::UiRect;
/// use ludion_engine::ui::resolve_ui_rect_of;
///
/// let backdrop = UiRect { fill: true, ..UiRect::default() };
/// assert_eq!(resolve_ui_rect_of(&backdrop, (2560, 1080)), [0.0, 0.0, 2560.0, 1080.0]);
/// ```
pub fn resolve_ui_rect_of(rect: &UiRect, viewport: (u32, u32)) -> [f32; 4] {
    if rect.fill {
        return [0.0, 0.0, viewport.0 as f32, viewport.1 as f32];
    }
    resolve_ui_rect(rect.anchor, rect.offset, rect.size, viewport)
}

/// Measures a [`UiText`] block's extent in **canvas units** from the
/// face's advance widths (`ludion_font::layout`, the one layout every text path shares): `size` is
/// the line height, width is the widest line's advance sum scaled to it, height is the line count ×
/// `size`. Lines split on `'\n'`, `"\r\n"` and `'\r'`; a tab is a space.
///
/// An **empty string measures as `[0, 0]`** — the deliberate special case (a non-empty string with
/// an empty line, e.g. `"A\n\nB"`, still counts that line as one line of zero width, per the normal
/// rule; only the fully-empty string collapses to a zero-line block).
///
/// # Example
///
/// ```
/// use ludion_engine::ui::measure_ui_text;
/// use ludion_font::FontFace;
///
/// assert_eq!(measure_ui_text(FontFace::embedded(), "", 10.0), [0.0, 0.0]);
/// assert_eq!(measure_ui_text(FontFace::embedded(), "HI", 10.0), [20.0, 10.0]); // 2 glyphs wide, 1 line tall
/// assert_eq!(measure_ui_text(FontFace::embedded(), "HI\nTHERE", 10.0), [50.0, 20.0]); // "THERE" (5) is the longest line
/// ```
pub fn measure_ui_text(face: &FontFace, text: &str, size: f32) -> [f32; 2] {
    layout(face, text, size, 0.0).extent()
}

/// Greedily word-wraps a [`UiText`] run to `wrap_width` **canvas units**,
/// in the advance units [`measure_ui_text`] measures with: a line holds as many glyphs as fit in
/// `wrap_width`, at least one.
///
/// Returns the input **borrowed and untouched** when wrapping is off or meaningless — `wrap_width` at
/// or below zero (the component default), or either argument non-finite — so an authored scene that
/// never asks for wrapping allocates nothing and keeps exactly the geometry it had.
///
/// Otherwise: an authored line break (`'\n'`, `"\r\n"` or `'\r'`) is a paragraph break and each
/// paragraph wraps on its own (line count and order preserved, an empty paragraph staying one empty
/// line); a tab is a space; within a paragraph, words are the
/// maximal runs of non-space characters (so a run of spaces collapses to one, and leading/trailing
/// spaces are dropped); a word joins the current line while it still fits with its separating space,
/// and starts a new one otherwise. A word may also break after a `-` or `—` (never at U+00A0). A
/// word too long for a whole line is hard-broken into line-width chunks, the last of which
/// continues as the current line.
///
/// Both this and [`measure_ui_text`] see the *same* face and string: [`extract`](crate::extract)
/// draws through `ui_text_draw`, which lays out with [`ludion_font::layout`] — the very layout
/// these two measure — so the measured block and the drawn one cannot disagree.
///
/// # Example
///
/// ```
/// use ludion_engine::ui::wrap_ui_text;
/// use ludion_font::FontFace;
///
/// // 100 canvas units at size 10 is 10 glyphs per line.
/// assert_eq!(wrap_ui_text(FontFace::embedded(), "THE OLD KEEPER WAITS", 10.0, 100.0), "THE OLD\nKEEPER\nWAITS");
/// // The default is no wrap at all: the run comes back exactly as authored.
/// assert_eq!(wrap_ui_text(FontFace::embedded(), "THE OLD KEEPER WAITS", 10.0, 0.0), "THE OLD KEEPER WAITS");
/// ```
pub fn wrap_ui_text<'a>(
    face: &FontFace,
    text: &'a str,
    size: f32,
    wrap_width: f32,
) -> Cow<'a, str> {
    if !wrap_width.is_finite() || wrap_width <= 0.0 || !size.is_finite() || size <= 0.0 {
        return Cow::Borrowed(text);
    }
    Cow::Owned(joined_lines(&layout(face, text, size, wrap_width)))
}

fn joined_lines(laid: &TextLayout<'_>) -> String {
    (0..laid.line_count())
        .map(|i| laid.line_text(i))
        .collect::<Vec<_>>()
        .join("\n")
}

/// The one layout a `UiText` draws: wrapped to `wrap_width`, its block anchored and resolved to pixels.
fn ui_text_block<'f>(
    face: &'f FontFace,
    text: &UiText,
    viewport: (u32, u32),
) -> (TextLayout<'f>, [f32; 4]) {
    let laid = layout(face, &text.text, text.size, text.wrap_width);
    let rect = resolve_ui_rect(text.anchor, text.offset, laid.extent(), viewport);
    (laid, rect)
}

/// The pixel-space glyph quads a [`UiText`] draws: wraps to `wrap_width`, anchors the measured block
/// with [`resolve_ui_rect`], and places one quad per mapped glyph at `size * scale` pixels per cell.
/// The block [`resolve_ui_text`] resolves and the quads drawn here come from one `layout()`, so they
/// cannot disagree.
pub(crate) fn ui_text_draw(face: &FontFace, text: &UiText, viewport: (u32, u32)) -> Vec<GlyphQuad> {
    let (laid, rect) = ui_text_block(face, text, viewport);
    let cell_px = text.size * pixel_scale(viewport);
    laid.quads([rect[0], rect[1]], cell_px, YAxis::Down)
}

/// Resolves a [`UiText`] to the pixel-space rect `[x, y, w, h]` of the block it draws — wrapped to
/// `wrap_width` and anchored by the same rule as [`resolve_ui_rect`], from the one layout
/// [`extract`](crate::extract) draws, so a click on a wrapped label's second line lands inside it.
/// An unwrapped text (`wrap_width` at or below zero) resolves over its [`measure_ui_text`] extent.
///
/// # Example
///
/// ```
/// use ludion_engine::components::{Anchor, UiText};
/// use ludion_engine::ui::resolve_ui_text;
/// use ludion_font::FontFace;
///
/// // A Center-anchored "HI" (2 glyphs wide, 1 line tall, size 10) centers its block.
/// let text = UiText { anchor: Anchor::Center, text: "HI".to_string(), size: 10.0, ..UiText::default() };
/// // Block is 20×10 canvas units; centered at (960, 540) means top-left at (950, 535).
/// assert_eq!(resolve_ui_text(FontFace::embedded(), &text, (1920, 1080)), [950.0, 535.0, 20.0, 10.0]);
/// ```
pub fn resolve_ui_text(face: &FontFace, text: &UiText, viewport: (u32, u32)) -> [f32; 4] {
    ui_text_block(face, text, viewport).1
}

/// The pixel-space geometry a [`UiList`] draws: where its panel goes, where its first line starts,
/// and how far apart the lines are. The one place measurement and drawing agree.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct UiListLayout {
    /// The background panel's pixel rect `[x, y, w, h]`: the measured block grown by `padding` on
    /// every side. Drawn only when the list's `background` alpha is above zero.
    pub background_px: [f32; 4],
    /// The first rendered line's top-left, in pixels.
    pub origin_px: [f32; 2],
    /// The line height in pixels (`size * scale`); glyphs are proportional, so it is not a cell width.
    pub cell_px: f32,
    /// Pixels from one line's top to the next line's top (`(size + line_spacing) * scale`).
    pub line_advance_px: f32,
}

/// The half-open range of `items` a [`UiList`] draws — `0..len` unless `max_visible` is smaller than
/// the list, in which case a window of exactly `max_visible` items follows the cursor: centred on
/// `selected` (itself clamped into range, as everywhere the cursor is read) and clamped so it never
/// runs off either end.
///
/// Pure arithmetic over the component, recomputed every frame — there is no scroll offset to store,
/// so nothing can drift out of step with `selected`.
///
/// # Example
///
/// ```
/// use ludion_engine::components::{UiList, UiListItem};
/// use ludion_engine::ui::ui_list_window;
/// use ludion_font::FontFace;
///
/// let item = |text: &str| UiListItem { text: text.to_string(), enabled: true };
/// let list = UiList {
///     items: (0..10).map(|i| item(&i.to_string())).collect(),
///     selected: 6,
///     max_visible: 3,
///     ..UiList::default()
/// };
/// assert_eq!(ui_list_window(&list), 5..8); // the cursor sits in the middle of its window
/// assert_eq!(ui_list_window(&UiList { max_visible: 0, ..list }), 0..10); // 0 is unlimited
/// ```
pub fn ui_list_window(list: &UiList) -> Range<usize> {
    let len = list.items.len();
    let max_visible = list.max_visible as usize;
    if max_visible == 0 || len <= max_visible {
        return 0..len;
    }
    let selected = (list.selected as usize).min(len - 1);
    let start = selected
        .saturating_sub(max_visible / 2)
        .min(len - max_visible);
    start..start + max_visible
}

/// The lines a [`UiList`] draws, in draw order: the `title` when it is non-empty, then one line per
/// item **inside the drawn window** ([`ui_list_window`]). Measurement and [`extract`](crate::extract)
/// both walk this, so a line can never be measured and not drawn.
///
/// Each line comes with the index of the item it draws — `None` for the title — so a caller colouring
/// a line by item state never re-derives the title offset or the window start.
///
/// An empty `items` vec yields nothing at all — the title alone is not a list (see
/// [`measure_ui_list`]).
///
/// # Example
///
/// ```
/// use ludion_engine::components::{UiList, UiListItem};
/// use ludion_engine::ui::ui_list_lines;
/// use ludion_font::FontFace;
///
/// let list = UiList {
///     title: "WARES".to_string(),
///     items: vec![UiListItem { text: "SALVE".to_string(), enabled: true }],
///     ..UiList::default()
/// };
/// assert_eq!(
///     ui_list_lines(&list).collect::<Vec<_>>(),
///     [(None, "WARES"), (Some(0), "SALVE")],
/// );
/// ```
pub fn ui_list_lines(list: &UiList) -> impl Iterator<Item = (Option<usize>, &str)> {
    let window = ui_list_window(list);
    let start = window.start;
    let title =
        (!list.items.is_empty() && !list.title.is_empty()).then_some((None, list.title.as_str()));
    title.into_iter().chain(
        list.items[window]
            .iter()
            .enumerate()
            .map(move |(offset, item)| (Some(start + offset), item.text.as_str())),
    )
}

/// One `UiList` line (title or item) laid out unwrapped with its line breaks folded to spaces, so an
/// item is always one row. Measurement, `list_hit` and extract all lay a list line out through here.
pub(crate) fn list_line_layout<'f>(face: &'f FontFace, text: &str, size: f32) -> TextLayout<'f> {
    layout(face, &fold_line_breaks(text), size, 0.0)
}

/// Measures a [`UiList`]'s block extent in **canvas units**, from the same advance widths
/// [`measure_ui_text`] uses: width is the widest line's advance sum, height is
/// `lines * size + (lines - 1) * line_spacing` — `line_spacing` is the gap *between* lines, so at
/// `0.0` the block measures exactly like one `UiText` holding the same lines. Padding is not part of
/// this; it grows the background only ([`resolve_ui_list`]).
///
/// An item is one row: its line breaks draw as spaces.
///
/// Height counts only the lines actually drawn (the title plus [`ui_list_window`]'s items), but
/// **width spans the title and every item**, windowed out or not — a panel that changed width as the
/// cursor scrolled would be a moving target to read.
///
/// An **empty `items` vec measures `[0, 0]`**, title included — the list draws nothing at all, and
/// measurement says the same thing (the [`measure_ui_text`] empty-string special case, one step
/// further).
///
/// # Example
///
/// ```
/// use ludion_engine::components::{UiList, UiListItem};
/// use ludion_engine::ui::measure_ui_list;
/// use ludion_font::FontFace;
///
/// let item = |text: &str| UiListItem { text: text.to_string(), enabled: true };
/// let list = UiList {
///     title: "HI".to_string(),
///     items: vec![item("THERE"), item("YOU")],
///     size: 10.0,
///     ..UiList::default()
/// };
/// // "THERE" (5 glyphs) is the longest line; 3 lines tall, no spacing.
/// assert_eq!(measure_ui_list(FontFace::embedded(), &list), [50.0, 30.0]);
/// assert_eq!(measure_ui_list(FontFace::embedded(), &UiList::default()), [0.0, 0.0]);
/// ```
pub fn measure_ui_list(face: &FontFace, list: &UiList) -> [f32; 2] {
    measure_list(face, list).extent
}

/// A `UiList` measured once: its block extent and the layout of each drawn line, in `ui_list_lines`
/// order.
struct MeasuredList<'f> {
    extent: [f32; 2],
    drawn: Vec<(Option<usize>, TextLayout<'f>)>,
}

fn measure_list<'f>(face: &'f FontFace, list: &UiList) -> MeasuredList<'f> {
    if list.items.is_empty() {
        return MeasuredList {
            extent: [0.0, 0.0],
            drawn: Vec::new(),
        };
    }
    let window = ui_list_window(list);
    let title = list_line_layout(face, &list.title, list.size);
    let mut width = title.extent()[0];
    let mut title = Some(title);
    let mut items: Vec<Option<TextLayout<'f>>> = Vec::with_capacity(list.items.len());
    for (index, item) in list.items.iter().enumerate() {
        let laid = list_line_layout(face, &item.text, list.size);
        width = width.max(laid.extent()[0]);
        items.push(window.contains(&index).then_some(laid));
    }
    let drawn: Vec<_> = ui_list_lines(list)
        .filter_map(|(item, _)| {
            let laid = match item {
                None => title.take(),
                Some(index) => items.get_mut(index).and_then(Option::take),
            };
            laid.map(|laid| (item, laid))
        })
        .collect();
    let lines = drawn.len();
    let height = lines as f32 * list.size + (lines - 1) as f32 * list.line_spacing;
    MeasuredList {
        extent: [width, height],
        drawn,
    }
}

/// Resolves a [`UiList`] to the pixel geometry [`extract`](crate::extract) draws it with — measures
/// the block ([`measure_ui_list`]), applies the same anchor/pivot rule as [`resolve_ui_rect`] over
/// that extent, and grows the panel outward by `padding`.
///
/// The panel grows *around* the block rather than being anchored in the block's place, so changing
/// `padding` never moves a line of text.
///
/// # Example
///
/// ```
/// use ludion_engine::components::{UiList, UiListItem};
/// use ludion_engine::ui::resolve_ui_list;
/// use ludion_font::FontFace;
///
/// let list = UiList {
///     items: vec![UiListItem { text: "HI".to_string(), enabled: true }],
///     size: 10.0,
///     padding: 4.0,
///     ..UiList::default()
/// };
/// // TopLeft, zero offset, scale 1.0: the block starts at the corner and the panel hangs outside it.
/// let layout = resolve_ui_list(FontFace::embedded(), &list, (1920, 1080));
/// assert_eq!(layout.origin_px, [0.0, 0.0]);
/// assert_eq!(layout.background_px, [-4.0, -4.0, 28.0, 18.0]);
/// assert_eq!(layout.line_advance_px, 10.0);
/// ```
pub fn resolve_ui_list(face: &FontFace, list: &UiList, viewport: (u32, u32)) -> UiListLayout {
    resolve_list(face, list, viewport).0
}

fn resolve_list<'f>(
    face: &'f FontFace,
    list: &UiList,
    viewport: (u32, u32),
) -> (UiListLayout, MeasuredList<'f>) {
    let measured = measure_list(face, list);
    let block = resolve_ui_rect(list.anchor, list.offset, measured.extent, viewport);
    let scale = pixel_scale(viewport);
    let pad = list.padding * scale;
    let layout = UiListLayout {
        background_px: [
            block[0] - pad,
            block[1] - pad,
            block[2] + pad * 2.0,
            block[3] + pad * 2.0,
        ],
        origin_px: [block[0], block[1]],
        cell_px: list.size * scale,
        line_advance_px: (list.size + list.line_spacing) * scale,
    };
    (layout, measured)
}

/// One drawn `UiList` line: its item (`None` = the title) and pixel quads.
pub(crate) type DrawnLine = (Option<usize>, Vec<GlyphQuad>);

/// Extract's list: the geometry, then each drawn line's item (`None` = title) and pixel quads.
pub(crate) fn ui_list_draw(
    face: &FontFace,
    list: &UiList,
    viewport: (u32, u32),
) -> (UiListLayout, Vec<DrawnLine>) {
    let (layout, measured) = resolve_list(face, list, viewport);
    let lines = measured
        .drawn
        .iter()
        .enumerate()
        .map(|(line, (item, laid))| {
            let quads = laid.quads(
                [
                    layout.origin_px[0],
                    layout.origin_px[1] + line as f32 * layout.line_advance_px,
                ],
                layout.cell_px,
                YAxis::Down,
            );
            (*item, quads)
        })
        .collect();
    (layout, lines)
}

/// The item under `cursor`, or `None` when nothing is hit — the pointer counterpart to
/// [`UiList::navigate`]'s keyboard/gamepad cursor.
///
/// `cursor` is in render-target pixels, origin top-left, `+Y` down — the space
/// [`MouseState::position`](ludion_sim::MouseState::position) reports and [`resolve_ui_list`]
/// resolves into; `viewport` is the same render extent `resolve_ui_list` takes. This walks the exact
/// geometry [`extract`](crate::extract) draws: line `k` in [`ui_list_lines`] order occupies the
/// half-open pixel box `x ∈ [origin.x, origin.x + width)`, `y ∈ [origin.y + k·advance, origin.y +
/// k·advance + cell)`, where `width` is the whole measured block ([`measure_ui_list`]) rather than
/// the item's own glyphs — a short item is hittable across the panel, not only over its own text.
/// An item is one row; its line breaks draw as spaces.
///
/// Returns `None` over the title line, the `line_spacing` gap between lines, the `padding` band
/// around the block, anywhere outside it, an empty list, a list whose `visible` is `false` (nothing
/// is drawn, so nothing is hit), or a non-finite `cursor` coordinate. The
/// returned index already accounts for [`ui_list_window`] (`ui_list_lines` carries `window.start +
/// offset`), and it comes back **whether or not the item is enabled** — a hit test answers "what is
/// under the cursor", and refusing a disabled item is the caller's job, exactly as `navigate` only
/// dims and skips one (a hover over an unaffordable ware is still a legitimate hit).
///
/// # Example
///
/// ```
/// use ludion_engine::components::{UiList, UiListItem};
/// use ludion_engine::ui::list_hit;
/// use ludion_font::FontFace;
///
/// let item = |text: &str| UiListItem { text: text.to_string(), enabled: true };
/// let list = UiList {
///     items: vec![item("SALVE"), item("RELIC")],
///     size: 10.0,
///     ..UiList::default()
/// };
/// // TopLeft, no title: item 0 draws at y in [0, 10), item 1 at y in [10, 20).
/// let viewport = (1920, 1080); // scale 1.0
/// assert_eq!(list_hit(FontFace::embedded(), &list, viewport, [5.0, 5.0]), Some(0));
/// assert_eq!(list_hit(FontFace::embedded(), &list, viewport, [5.0, 15.0]), Some(1));
/// assert_eq!(list_hit(FontFace::embedded(), &list, viewport, [5.0, 25.0]), None); // below the block
/// ```
pub fn list_hit(
    face: &FontFace,
    list: &UiList,
    viewport: (u32, u32),
    cursor: [f32; 2],
) -> Option<u32> {
    if !list.visible || !cursor[0].is_finite() || !cursor[1].is_finite() {
        return None;
    }
    let (layout, measured) = resolve_list(face, list, viewport);
    // The block's own width, not the panel's padded one — padding only grows `background_px`.
    let width_px = measured.extent[0] * pixel_scale(viewport);
    if cursor[0] < layout.origin_px[0] || cursor[0] >= layout.origin_px[0] + width_px {
        return None;
    }
    for (line, (item, _)) in measured.drawn.iter().enumerate() {
        let top = layout.origin_px[1] + line as f32 * layout.line_advance_px;
        let bottom = top + layout.cell_px;
        if cursor[1] >= top && cursor[1] < bottom {
            return item.map(|index| index as u32);
        }
    }
    None
}

/// One frame's menu intent: three edge flags a game feeds to [`UiList::navigate`].
///
/// Built from an [`InputState`] by [`UiNav::from_input`] under the engine's fixed bindings, or by
/// hand — the fields are public precisely so a game with its own bindings never needs an engine
/// change. There is no action-mapping layer and no engine-side focus manager: exactly one list is
/// navigated per frame, the one the game chooses to call.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct UiNav {
    /// Move the cursor to the previous selectable item.
    pub prev: bool,
    /// Move the cursor to the next selectable item.
    pub next: bool,
    /// Activate the item under the cursor.
    pub activate: bool,
}

impl UiNav {
    /// Reads this frame's menu intent under the engine's default bindings: `Up`/`DpadUp` is `prev`,
    /// `Down`/`DpadDown` is `next`, and `Enter`/`Space`/`GamepadButton::South` is `activate`.
    ///
    /// **Discrete edges only** (`was_pressed`, never `is_held`): one press is one step, a held
    /// direction does not repeat, and no hidden repeat-latch state exists to desynchronize. An analog
    /// stick is deliberately not read here for the same reason.
    ///
    /// # Example
    ///
    /// ```
    /// use ludion_engine::ui::UiNav;
    /// use ludion_sim::{InputEvent, InputState, Key};
    ///
    /// let mut input = InputState::default();
    /// input.begin_tick();
    /// input.apply(InputEvent::KeyDown { key: Key::Down });
    /// assert!(UiNav::from_input(&input).next);
    ///
    /// input.begin_tick(); // the key is still held, but the edge is gone
    /// assert!(!UiNav::from_input(&input).next);
    /// ```
    pub fn from_input(input: &InputState) -> Self {
        let pad = input.gamepad();
        UiNav {
            prev: input.was_pressed(Key::Up) || pad.was_pressed(GamepadButton::DpadUp),
            next: input.was_pressed(Key::Down) || pad.was_pressed(GamepadButton::DpadDown),
            activate: input.was_pressed(Key::Enter)
                || input.was_pressed(Key::Space)
                || pad.was_pressed(GamepadButton::South),
        }
    }
}

impl UiList {
    /// Applies one frame's [`UiNav`] to the cursor and reports an activation.
    ///
    /// Moves `selected` (also clamping a stale value into range) and returns `Some(index)` when
    /// `activate` fired over an **enabled** item — the activation is this call's return value, never a
    /// field, so there is no stale flag and no clearing protocol to forget.
    ///
    /// The cursor **wraps** at both ends and **never lands on a disabled item**; a list whose items
    /// are all disabled keeps its cursor where it is and refuses activation. An empty list moves
    /// nothing and returns `None`.
    ///
    /// # Example
    ///
    /// ```
    /// use ludion_engine::components::{UiList, UiListItem};
    /// use ludion_engine::ui::UiNav;
    ///
    /// let item = |text: &str, enabled: bool| UiListItem { text: text.to_string(), enabled };
    /// let mut list = UiList {
    ///     items: vec![item("BUY", true), item("SOLD OUT", false), item("LEAVE", true)],
    ///     ..UiList::default()
    /// };
    ///
    /// let next = UiNav { next: true, ..UiNav::default() };
    /// assert_eq!(list.navigate(next), None); // the sold-out item is skipped
    /// assert_eq!(list.selected, 2);
    ///
    /// let activate = UiNav { activate: true, ..UiNav::default() };
    /// assert_eq!(list.navigate(activate), Some(2));
    /// ```
    pub fn navigate(&mut self, nav: UiNav) -> Option<u32> {
        let count = self.items.len();
        if count == 0 {
            return None;
        }
        let mut cursor = (self.selected as usize).min(count - 1);
        if nav.prev {
            cursor = step_cursor(&self.items, cursor, false);
        }
        if nav.next {
            cursor = step_cursor(&self.items, cursor, true);
        }
        self.selected = cursor as u32;
        (nav.activate && self.items[cursor].enabled).then_some(cursor as u32)
    }
}

/// The next enabled index from `from`, wrapping — or `from` itself when nothing else is selectable.
/// Bounded by one full lap, so an all-disabled list terminates with the cursor where it was.
fn step_cursor(items: &[UiListItem], from: usize, forward: bool) -> usize {
    let count = items.len();
    let mut index = from;
    for _ in 0..count {
        index = if forward {
            (index + 1) % count
        } else {
            (index + count - 1) % count
        };
        if items[index].enabled {
            return index;
        }
    }
    from
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A filled rect is the whole target at 4:3, 16:9, 21:9 and a resized 2560x1080, whatever its
    /// anchor, offset and size say; without `fill` the anchored arithmetic is unchanged.
    #[test]
    fn a_filled_rect_covers_the_viewport_at_every_extent() {
        let filled = UiRect {
            anchor: Anchor::BottomRight,
            offset: [30.0, -12.0],
            size: [7.0, 9.0],
            fill: true,
            ..UiRect::default()
        };
        for viewport in [
            (1440, 1080),
            (1920, 1080),
            (2520, 1080),
            (2560, 1080),
            (1, 1),
        ] {
            let expected = [0.0, 0.0, viewport.0 as f32, viewport.1 as f32];
            assert_eq!(resolve_ui_rect_of(&filled, viewport), expected);
        }
        let anchored = UiRect {
            fill: false,
            ..filled
        };
        assert_eq!(
            resolve_ui_rect_of(&anchored, (1920, 1080)),
            resolve_ui_rect(
                anchored.anchor,
                anchored.offset,
                anchored.size,
                (1920, 1080)
            )
        );
    }

    // --- resolve_ui_rect: all nine anchors, at two resolutions (uniform 2x scaling) ---

    const ANCHORS: [Anchor; 9] = [
        Anchor::TopLeft,
        Anchor::TopCenter,
        Anchor::TopRight,
        Anchor::CenterLeft,
        Anchor::Center,
        Anchor::CenterRight,
        Anchor::BottomLeft,
        Anchor::BottomCenter,
        Anchor::BottomRight,
    ];

    /// At 1920×1080, every anchor's resolved rect is exactly 2× its 960×540 twin — same aspect
    /// (16:9), so the canvas is identical and only `scale` doubles.
    #[test]
    fn all_anchors_scale_uniformly_between_960x540_and_1920x1080() {
        for anchor in ANCHORS {
            let small = resolve_ui_rect(anchor, [4.0, -6.0], [50.0, 20.0], (960, 540));
            let large = resolve_ui_rect(anchor, [4.0, -6.0], [50.0, 20.0], (1920, 1080));
            for i in 0..4 {
                assert!(
                    (large[i] - small[i] * 2.0).abs() < 1e-4,
                    "anchor {anchor:?} component {i}: small={small:?} large={large:?}"
                );
            }
        }
    }

    /// Corner anchors hug the actual viewport corners with zero offset, at a 4:3 viewport — the
    /// pivot rule needs no size arithmetic from the caller.
    #[test]
    fn corner_anchors_hug_corners_at_4_3_viewport() {
        let viewport = (960, 720);
        let size = [64.0, 32.0];

        let top_left = resolve_ui_rect(Anchor::TopLeft, [0.0, 0.0], size, viewport);
        assert_eq!([top_left[0], top_left[1]], [0.0, 0.0]);

        let top_right = resolve_ui_rect(Anchor::TopRight, [0.0, 0.0], size, viewport);
        assert!((top_right[0] + top_right[2] - viewport.0 as f32).abs() < 1e-3);
        assert!((top_right[1] - 0.0).abs() < 1e-3);

        let bottom_left = resolve_ui_rect(Anchor::BottomLeft, [0.0, 0.0], size, viewport);
        assert!((bottom_left[0] - 0.0).abs() < 1e-3);
        assert!((bottom_left[1] + bottom_left[3] - viewport.1 as f32).abs() < 1e-3);

        let bottom_right = resolve_ui_rect(Anchor::BottomRight, [0.0, 0.0], size, viewport);
        assert!((bottom_right[0] + bottom_right[2] - viewport.0 as f32).abs() < 1e-3);
        assert!((bottom_right[1] + bottom_right[3] - viewport.1 as f32).abs() < 1e-3);
    }

    /// The same corner-hugging property holds at an ultrawide (21:9-ish) viewport — the canvas
    /// widens, but `x + w`/`y + h` still land exactly on the target edges.
    #[test]
    fn corner_anchors_hug_corners_at_ultrawide_viewport() {
        let viewport = (2560, 1080);
        let size = [100.0, 50.0];

        let top_right = resolve_ui_rect(Anchor::TopRight, [0.0, 0.0], size, viewport);
        assert!((top_right[0] + top_right[2] - viewport.0 as f32).abs() < 1e-2);

        let bottom_right = resolve_ui_rect(Anchor::BottomRight, [0.0, 0.0], size, viewport);
        assert!((bottom_right[0] + bottom_right[2] - viewport.0 as f32).abs() < 1e-2);
        assert!((bottom_right[1] + bottom_right[3] - viewport.1 as f32).abs() < 1e-2);
    }

    /// `Center` stays centered — zero offset centers the element regardless of viewport aspect.
    #[test]
    fn center_anchor_stays_centered_across_viewports() {
        for viewport in [(960, 540), (1920, 1080), (960, 720), (2560, 1080)] {
            let size = [40.0, 20.0];
            let rect = resolve_ui_rect(Anchor::Center, [0.0, 0.0], size, viewport);
            let cx = rect[0] + rect[2] / 2.0;
            let cy = rect[1] + rect[3] / 2.0;
            assert!(
                (cx - viewport.0 as f32 / 2.0).abs() < 1e-2,
                "viewport {viewport:?}: cx={cx}"
            );
            assert!(
                (cy - viewport.1 as f32 / 2.0).abs() < 1e-2,
                "viewport {viewport:?}: cy={cy}"
            );
        }
    }

    /// Offsets move the resolved rect by `offset * scale`, independent of anchor or size.
    #[test]
    fn offset_moves_the_rect_by_offset_times_scale() {
        let viewport = (1920, 1080); // scale = 1.0
        let base = resolve_ui_rect(Anchor::TopLeft, [0.0, 0.0], [50.0, 50.0], viewport);
        let moved = resolve_ui_rect(Anchor::TopLeft, [12.0, -7.0], [50.0, 50.0], viewport);
        assert!((moved[0] - base[0] - 12.0).abs() < 1e-4);
        assert!((moved[1] - base[1] + 7.0).abs() < 1e-4);

        let viewport_half = (960, 540); // scale = 0.5
        let base_half = resolve_ui_rect(Anchor::TopLeft, [0.0, 0.0], [50.0, 50.0], viewport_half);
        let moved_half =
            resolve_ui_rect(Anchor::TopLeft, [12.0, -7.0], [50.0, 50.0], viewport_half);
        assert!((moved_half[0] - base_half[0] - 6.0).abs() < 1e-4); // 12 * 0.5
        assert!((moved_half[1] - base_half[1] + 3.5).abs() < 1e-4); // -7 * 0.5
    }

    /// `BottomRight` with zero offset lands exactly on the viewport corner — the pivot rule's
    /// headline case (a corner HUD needs no size arithmetic).
    #[test]
    fn bottom_right_pivot_lands_exactly_on_the_corner() {
        let rect = resolve_ui_rect(Anchor::BottomRight, [0.0, 0.0], [64.0, 24.0], (1920, 1080));
        assert_eq!(rect[0] + rect[2], 1920.0);
        assert_eq!(rect[1] + rect[3], 1080.0);
    }

    // --- text measurement ---

    #[test]
    fn measure_single_line() {
        assert_eq!(
            measure_ui_text(FontFace::embedded(), "HELLO", 10.0),
            [50.0, 10.0]
        );
    }

    #[test]
    fn measure_multi_line_uses_the_longest_line() {
        assert_eq!(
            measure_ui_text(FontFace::embedded(), "HI\nTHERE\nYOU", 10.0),
            [50.0, 30.0]
        );
    }

    #[test]
    fn measure_empty_string_is_zero() {
        assert_eq!(measure_ui_text(FontFace::embedded(), "", 24.0), [0.0, 0.0]);
    }

    /// A non-empty string with an empty *line* still counts that line normally (only the
    /// fully-empty string is the zero-extent special case).
    #[test]
    fn measure_embedded_empty_line_counts_as_one_line_of_zero_width() {
        assert_eq!(
            measure_ui_text(FontFace::embedded(), "A\n\nB", 10.0),
            [10.0, 30.0]
        );
    }

    /// A `Center`-anchored text block resolves so its measured block is centered on the canvas.
    #[test]
    fn center_anchored_text_centers_the_block() {
        let viewport = (1920, 1080); // scale 1.0, canvas == pixels
        let text = UiText {
            anchor: Anchor::Center,
            text: "HI".to_string(),
            size: 10.0,
            ..UiText::default()
        };
        // Block is 20 (2 glyphs * 10) x 10 canvas units; centered at (960, 540).
        assert_eq!(
            resolve_ui_text(FontFace::embedded(), &text, viewport),
            [950.0, 535.0, 20.0, 10.0]
        );
    }

    #[test]
    fn empty_text_resolves_to_a_zero_size_block_without_panicking() {
        let text = UiText {
            size: 24.0,
            ..UiText::default()
        };
        assert_eq!(
            resolve_ui_text(FontFace::embedded(), &text, (1920, 1080)),
            [0.0, 0.0, 0.0, 0.0]
        );
    }

    /// A wrapped text resolves to the block it draws: two lines of four cells, not one unwrapped line.
    #[test]
    fn resolve_ui_text_wraps_like_the_drawn_block() {
        let viewport = (1920, 1080);
        let text = UiText {
            anchor: Anchor::Center,
            text: "AAAA BBBB".to_string(),
            size: 10.0,
            wrap_width: 40.0,
            ..UiText::default()
        };
        let rect = resolve_ui_text(FontFace::embedded(), &text, viewport);
        assert_eq!(rect, [940.0, 530.0, 40.0, 20.0]);
        let quads = ui_text_draw(FontFace::embedded(), &text, viewport);
        let edge = |index: usize, min: bool| {
            quads.iter().map(|q| q.pos[index]).fold(
                if min { f32::MAX } else { f32::MIN },
                |a, b| {
                    if min { a.min(b) } else { a.max(b) }
                },
            )
        };
        assert_eq!(edge(0, true), rect[0]);
        assert_eq!(edge(1, true), rect[1]);
        assert_eq!(edge(2, false), rect[0] + rect[2]);
        assert_eq!(edge(3, false), rect[1] + rect[3]);
    }

    // --- text wrapping ---

    /// The headline case: a paragraph breaks on spaces at the column boundary, never mid-word.
    #[test]
    fn wrap_breaks_on_spaces_at_the_column_boundary() {
        // 10 columns: "THE OLD" (7) fits, "THE OLD KEEPER" (14) does not.
        assert_eq!(
            wrap_ui_text(FontFace::embedded(), "THE OLD KEEPER WAITS", 10.0, 100.0),
            "THE OLD\nKEEPER\nWAITS"
        );
        // Exactly filling a line is still one line — the fit test is `<=`, not `<`.
        assert_eq!(
            wrap_ui_text(FontFace::embedded(), "ABCDE FGHI", 10.0, 100.0),
            "ABCDE FGHI"
        );
    }

    /// A word longer than a whole line is hard-broken, and its tail keeps filling — a short word
    /// after it shares the last chunk's line rather than starting a third.
    #[test]
    fn wrap_hard_breaks_an_over_long_word() {
        assert_eq!(
            wrap_ui_text(FontFace::embedded(), "ABCDEFGHIJKLM", 10.0, 50.0),
            "ABCDE\nFGHIJ\nKLM",
            "5 columns, chunked from the front"
        );
        assert_eq!(
            wrap_ui_text(FontFace::embedded(), "ABCDEFG HI", 10.0, 50.0),
            "ABCDE\nFG HI"
        );
        assert_eq!(
            wrap_ui_text(FontFace::embedded(), "HI ABCDEFG", 10.0, 50.0),
            "HI\nABCDE\nFG",
            "the current line is flushed before the long word starts"
        );
    }

    /// The component default is a no-op that allocates nothing — the property every existing
    /// authored scene's geometry rests on.
    #[test]
    fn wrap_width_zero_borrows_the_input_unchanged() {
        let text = "THE OLD KEEPER WAITS";
        let out = wrap_ui_text(FontFace::embedded(), text, 24.0, 0.0);
        assert!(
            matches!(out, Cow::Borrowed(_)),
            "no allocation at the default"
        );
        assert_eq!(out, text);
    }

    /// A wrap width or size that cannot describe a column count is the same no-op, never a panic and
    /// never a divide-by-zero line count.
    #[test]
    fn wrap_refuses_non_finite_or_non_positive_arguments() {
        let text = "THE OLD KEEPER";
        for (size, wrap_width) in [
            (24.0, -1.0),
            (24.0, f32::NAN),
            (24.0, f32::INFINITY),
            (0.0, 100.0),
            (-4.0, 100.0),
            (f32::NAN, 100.0),
        ] {
            let out = wrap_ui_text(FontFace::embedded(), text, size, wrap_width);
            assert!(
                matches!(out, Cow::Borrowed(_)),
                "size {size} / wrap_width {wrap_width} must be a no-op"
            );
            assert_eq!(out, text);
        }
    }

    /// An authored `'\n'` is a paragraph break: each side wraps on its own, and an empty paragraph
    /// survives as one empty line (so a deliberate blank line stays a blank line).
    #[test]
    fn wrap_treats_newlines_as_independent_paragraphs() {
        assert_eq!(
            wrap_ui_text(FontFace::embedded(), "AAA BBB CCC\nDDD EEE", 10.0, 70.0),
            "AAA BBB\nCCC\nDDD EEE",
            "7 columns; the second paragraph does not absorb the first's tail"
        );
        assert_eq!(
            wrap_ui_text(FontFace::embedded(), "A\n\nB", 10.0, 100.0),
            "A\n\nB"
        );
        assert_eq!(wrap_ui_text(FontFace::embedded(), "", 10.0, 100.0), "");
    }

    /// Words are the maximal non-space runs: a run of spaces collapses to one separator, and leading
    /// and trailing spaces are dropped.
    #[test]
    fn wrap_collapses_runs_of_spaces() {
        assert_eq!(
            wrap_ui_text(FontFace::embedded(), "  A   B  ", 10.0, 100.0),
            "A B"
        );
    }

    /// A wrap width narrower than one glyph still yields one column — a degenerate authored value
    /// produces a tall stack, never an empty or infinite one.
    #[test]
    fn wrap_columns_floor_to_at_least_one() {
        assert_eq!(
            wrap_ui_text(FontFace::embedded(), "ABC", 10.0, 4.0),
            "A\nB\nC"
        );
    }

    /// Measurement over the wrapped string agrees with the width that produced it — the invariant
    /// that makes wrapping safe to feed straight into `resolve_ui_text`.
    #[test]
    fn a_wrapped_paragraph_measures_within_its_wrap_width() {
        let size = 12.0;
        let wrap_width = 240.0;
        let wrapped = wrap_ui_text(
            FontFace::embedded(),
            "THE KEEPER WAITS BENEATH THE CATHEDRAL FOR ANYONE WILLING TO LISTEN",
            size,
            wrap_width,
        );
        assert!(wrapped.contains('\n'), "the sample must actually wrap");
        assert!(
            measure_ui_text(FontFace::embedded(), &wrapped, size)[0] <= wrap_width,
            "measured {:?} over {wrap_width}",
            measure_ui_text(FontFace::embedded(), &wrapped, size)
        );
    }

    // --- UiList: measurement, layout, navigation ---

    use ludion_sim::InputEvent;

    fn item(text: &str) -> UiListItem {
        UiListItem {
            text: text.to_string(),
            enabled: true,
        }
    }

    fn disabled(text: &str) -> UiListItem {
        UiListItem {
            text: text.to_string(),
            enabled: false,
        }
    }

    fn list_of(items: Vec<UiListItem>) -> UiList {
        UiList {
            items,
            size: 10.0,
            ..UiList::default()
        }
    }

    /// The title is one more line, measured for width like any other.
    #[test]
    fn measure_list_counts_the_title_as_a_line() {
        let untitled = list_of(vec![item("HI")]);
        assert_eq!(
            measure_ui_list(FontFace::embedded(), &untitled),
            [20.0, 10.0]
        );

        let titled = UiList {
            title: "MENU".to_string(),
            ..untitled
        };
        // "MENU" (4 glyphs) is now the longest line, and the block is two lines tall.
        assert_eq!(measure_ui_list(FontFace::embedded(), &titled), [40.0, 20.0]);
    }

    /// An empty `items` vec measures `[0, 0]` — a title alone is not a list, and it draws nothing.
    #[test]
    fn measure_empty_list_is_zero_even_with_a_title() {
        assert_eq!(
            measure_ui_list(FontFace::embedded(), &UiList::default()),
            [0.0, 0.0]
        );
        let title_only = UiList {
            title: "MENU".to_string(),
            ..UiList::default()
        };
        assert_eq!(
            measure_ui_list(FontFace::embedded(), &title_only),
            [0.0, 0.0]
        );
        assert_eq!(ui_list_lines(&title_only).count(), 0);
    }

    /// Width is the widest line, whichever line that is.
    #[test]
    fn measure_list_width_is_the_longest_line() {
        let list = list_of(vec![item("HI"), item("THERE"), item("YOU")]);
        assert_eq!(measure_ui_list(FontFace::embedded(), &list), [50.0, 30.0]);
    }

    /// `line_spacing` is the gap *between* lines: `n` lines add `n - 1` gaps, never a trailing one.
    #[test]
    fn measure_list_line_spacing_is_a_gap_between_lines() {
        let tight = list_of(vec![item("A"), item("B"), item("C")]);
        assert_eq!(measure_ui_list(FontFace::embedded(), &tight), [10.0, 30.0]);

        let spaced = UiList {
            line_spacing: 4.0,
            ..tight
        };
        assert_eq!(measure_ui_list(FontFace::embedded(), &spaced), [10.0, 38.0]); // 3 * 10 + 2 * 4

        let single = UiList {
            items: vec![item("A")],
            size: 10.0,
            line_spacing: 4.0,
            ..UiList::default()
        };
        assert_eq!(
            measure_ui_list(FontFace::embedded(), &single),
            [10.0, 10.0],
            "one line, no gap"
        );
    }

    /// The panel grows outward from the block, so `padding` never moves the text; the line advance
    /// carries `line_spacing`.
    #[test]
    fn resolve_list_grows_the_panel_without_moving_the_block() {
        let list = UiList {
            items: vec![item("HI"), item("YOU")],
            size: 10.0,
            line_spacing: 2.0,
            padding: 5.0,
            ..UiList::default()
        };
        let viewport = (1920, 1080); // scale 1.0
        let layout = resolve_ui_list(FontFace::embedded(), &list, viewport);

        let block = resolve_ui_rect(
            list.anchor,
            list.offset,
            measure_ui_list(FontFace::embedded(), &list),
            viewport,
        );
        assert_eq!(layout.origin_px, [block[0], block[1]]);
        assert_eq!(
            layout.background_px,
            [
                block[0] - 5.0,
                block[1] - 5.0,
                block[2] + 10.0,
                block[3] + 10.0
            ]
        );
        assert_eq!(layout.cell_px, 10.0);
        assert_eq!(layout.line_advance_px, 12.0);

        let padded = UiList {
            padding: 40.0,
            ..list
        };
        assert_eq!(
            resolve_ui_list(FontFace::embedded(), &padded, viewport).origin_px,
            layout.origin_px,
            "padding must not move the text"
        );
    }

    /// A `Center`-anchored list centers its block, and the symmetric panel stays centered with it.
    #[test]
    fn center_anchored_list_centers_the_block_and_its_panel() {
        let list = UiList {
            anchor: Anchor::Center,
            items: vec![item("HI")],
            size: 10.0,
            padding: 5.0,
            ..UiList::default()
        };
        let layout = resolve_ui_list(FontFace::embedded(), &list, (1920, 1080));
        assert_eq!(layout.origin_px, [950.0, 535.0]); // 20x10 block centered at (960, 540)
        let panel = layout.background_px;
        assert_eq!(panel[0] + panel[2] / 2.0, 960.0);
        assert_eq!(panel[1] + panel[3] / 2.0, 540.0);
    }

    // --- UiList: the visible window ---

    fn windowed(count: usize, max_visible: u32, selected: u32) -> UiList {
        UiList {
            items: (0..count).map(|i| item(&i.to_string())).collect(),
            selected,
            max_visible,
            size: 10.0,
            ..UiList::default()
        }
    }

    /// `max_visible` at `0` is unlimited, and a window at least as long as the list shows all of it —
    /// the two ways a list opts out of windowing.
    #[test]
    fn a_list_that_fits_is_never_windowed() {
        assert_eq!(ui_list_window(&windowed(8, 0, 3)), 0..8, "0 is unlimited");
        assert_eq!(
            ui_list_window(&windowed(3, 5, 1)),
            0..3,
            "shorter than the window"
        );
        assert_eq!(
            ui_list_window(&windowed(5, 5, 4)),
            0..5,
            "exactly the window"
        );
        assert_eq!(ui_list_window(&UiList::default()), 0..0, "an empty list");
    }

    /// The window follows the cursor: centred in the middle of the list, clamped flush against
    /// either end so it always shows exactly `max_visible` items.
    #[test]
    fn the_window_centers_the_cursor_and_clamps_at_both_ends() {
        assert_eq!(
            ui_list_window(&windowed(12, 5, 0)),
            0..5,
            "cursor at the top"
        );
        assert_eq!(ui_list_window(&windowed(12, 5, 1)), 0..5, "still clamped");
        assert_eq!(ui_list_window(&windowed(12, 5, 7)), 5..10, "centred");
        assert_eq!(
            ui_list_window(&windowed(12, 5, 11)),
            7..12,
            "cursor at the end"
        );
        assert_eq!(
            ui_list_window(&windowed(12, 5, 99)),
            7..12,
            "a stale cursor clamps like everywhere else it is read"
        );
    }

    /// The drawn lines are the title plus the windowed items only, each tagged with its own item
    /// index — the mapping `extract` colours by.
    #[test]
    fn list_lines_yield_the_title_and_only_the_windowed_items() {
        let list = UiList {
            title: "WARES".to_string(),
            ..windowed(12, 3, 6)
        };
        assert_eq!(
            ui_list_lines(&list).collect::<Vec<_>>(),
            [
                (None, "WARES"),
                (Some(5), "5"),
                (Some(6), "6"),
                (Some(7), "7")
            ]
        );
    }

    /// Height shrinks to the window, width does not: the panel must not resize under a scrolling
    /// cursor, so it measures every item.
    #[test]
    fn measure_list_height_follows_the_window_but_width_spans_every_item() {
        let mut list = windowed(6, 2, 0);
        list.items[4] = item("LONGEST-ITEM"); // 12 glyphs, outside the window at selected 0
        assert_eq!(ui_list_window(&list), 0..2);
        assert_eq!(
            measure_ui_list(FontFace::embedded(), &list),
            [120.0, 20.0],
            "2 drawn lines tall, as wide as the widest item anywhere"
        );

        list.selected = 4; // the long item scrolls into view
        assert_eq!(
            measure_ui_list(FontFace::embedded(), &list),
            [120.0, 20.0],
            "the block is the same size wherever the cursor is"
        );
    }

    // --- UiList: pointer hit-testing ---

    /// A point inside line `line`'s drawn box: the vertical middle, and `x_fraction` of the way
    /// across the full block width. Built from `resolve_ui_list`/`measure_ui_list`, never a
    /// hand-typed pixel, so it stays correct however the layout constants change.
    fn point_on_line(
        list: &UiList,
        viewport: (u32, u32),
        line: usize,
        x_fraction: f32,
    ) -> [f32; 2] {
        let layout = resolve_ui_list(FontFace::embedded(), list, viewport);
        let width_px = measure_ui_list(FontFace::embedded(), list)[0] * pixel_scale(viewport);
        let top = layout.origin_px[1] + line as f32 * layout.line_advance_px;
        [
            layout.origin_px[0] + width_px * x_fraction,
            top + layout.cell_px / 2.0,
        ]
    }

    /// The anchor only moves the block, never the geometry `list_hit` reads relative to it — the
    /// same item hits at `TopLeft`, `Center` and `BottomRight`.
    #[test]
    fn list_hit_finds_the_item_under_the_cursor_at_three_anchors() {
        for anchor in [Anchor::TopLeft, Anchor::Center, Anchor::BottomRight] {
            let list = UiList {
                anchor,
                items: vec![item("SALVE"), item("RELIC")],
                size: 10.0,
                ..UiList::default()
            };
            let viewport = (1920, 1080);
            assert_eq!(
                list_hit(
                    FontFace::embedded(),
                    &list,
                    viewport,
                    point_on_line(&list, viewport, 0, 0.5)
                ),
                Some(0),
                "anchor {anchor:?}"
            );
            assert_eq!(
                list_hit(
                    FontFace::embedded(),
                    &list,
                    viewport,
                    point_on_line(&list, viewport, 1, 0.5)
                ),
                Some(1),
                "anchor {anchor:?}"
            );
        }
    }

    /// The same logical item hits at two viewports with different `pixel_scale` — the hit test reads
    /// `resolve_ui_list`'s own scale rather than assuming one.
    #[test]
    fn list_hit_agrees_across_viewports_with_different_pixel_scale() {
        let list = UiList {
            items: vec![item("SALVE"), item("RELIC"), item("LEAVE")],
            size: 10.0,
            ..UiList::default()
        };
        for viewport in [(1920, 1080), (1280, 720)] {
            assert_eq!(
                list_hit(
                    FontFace::embedded(),
                    &list,
                    viewport,
                    point_on_line(&list, viewport, 2, 0.5)
                ),
                Some(2),
                "viewport {viewport:?}"
            );
        }
    }

    /// The title line is drawn but never selectable — a hit over it is `None`, same as `navigate`
    /// never landing there.
    #[test]
    fn list_hit_returns_none_over_the_title_line() {
        let list = UiList {
            title: "WARES".to_string(),
            items: vec![item("SALVE")],
            size: 10.0,
            ..UiList::default()
        };
        let viewport = (1920, 1080);
        assert_eq!(
            list_hit(
                FontFace::embedded(),
                &list,
                viewport,
                point_on_line(&list, viewport, 0, 0.5)
            ),
            None,
            "line 0 is the title"
        );
        assert_eq!(
            list_hit(
                FontFace::embedded(),
                &list,
                viewport,
                point_on_line(&list, viewport, 1, 0.5)
            ),
            Some(0),
            "line 1 is the item"
        );
    }

    /// A point in the `line_spacing` gap belongs to neither line — each line's box is exactly
    /// `cell_px` tall, not the full `line_advance_px`.
    #[test]
    fn list_hit_returns_none_in_the_line_spacing_gap() {
        let list = UiList {
            items: vec![item("A"), item("B")],
            size: 10.0,
            line_spacing: 6.0,
            ..UiList::default()
        };
        let viewport = (1920, 1080);
        let layout = resolve_ui_list(FontFace::embedded(), &list, viewport);
        let gap_y = layout.origin_px[1] + (layout.cell_px + layout.line_advance_px) / 2.0;
        assert_eq!(
            list_hit(
                FontFace::embedded(),
                &list,
                viewport,
                [layout.origin_px[0], gap_y]
            ),
            None
        );
    }

    /// The `padding` band grows the panel but is not itself hittable — only the measured
    /// title+items block is.
    #[test]
    fn list_hit_returns_none_in_the_padding_band() {
        let list = UiList {
            items: vec![item("HI")],
            size: 10.0,
            padding: 20.0,
            ..UiList::default()
        };
        let viewport = (1920, 1080);
        let layout = resolve_ui_list(FontFace::embedded(), &list, viewport);
        let cursor = [
            layout.origin_px[0] - 10.0,
            layout.origin_px[1] + layout.cell_px / 2.0,
        ];
        assert!(
            cursor[0] > layout.background_px[0],
            "still inside the padded panel"
        );
        assert_eq!(
            list_hit(FontFace::embedded(), &list, viewport, cursor),
            None
        );
    }

    /// The right edge is exclusive, and the hittable width is the whole block, not one item's own
    /// glyphs — a short item is still hit near the panel's far edge.
    #[test]
    fn list_hit_right_edge_is_exclusive_and_spans_the_whole_block_width() {
        let list = UiList {
            items: vec![item("HI"), item("A LONGER ITEM HERE")],
            size: 10.0,
            ..UiList::default()
        };
        let viewport = (1920, 1080);
        let layout = resolve_ui_list(FontFace::embedded(), &list, viewport);
        let width_px = measure_ui_list(FontFace::embedded(), &list)[0] * pixel_scale(viewport);
        let y0 = layout.origin_px[1] + layout.cell_px / 2.0; // line 0, the short item "HI"

        let just_inside = [layout.origin_px[0] + width_px - 0.5, y0];
        assert_eq!(
            list_hit(FontFace::embedded(), &list, viewport, just_inside),
            Some(0),
            "hittable across the panel, not only over its own glyphs"
        );

        let on_edge = [layout.origin_px[0] + width_px, y0];
        assert_eq!(
            list_hit(FontFace::embedded(), &list, viewport, on_edge),
            None,
            "half-open"
        );
    }

    /// A windowed list's hit maps a drawn row to `window.start + row`, exactly as `ui_list_lines`
    /// tags it — the geometry a scrolled panel draws is what a hit test must answer for.
    #[test]
    fn list_hit_maps_a_windowed_row_to_window_start_plus_row() {
        let list = UiList {
            title: "WARES".to_string(),
            ..windowed(12, 3, 6)
        };
        let viewport = (1920, 1080);
        assert_eq!(ui_list_window(&list), 5..8);
        // Line 0 is the title; lines 1..=3 are the windowed items 5, 6, 7.
        for (line, expected_index) in [(1, 5u32), (2, 6), (3, 7)] {
            let cursor = point_on_line(&list, viewport, line, 0.5);
            assert_eq!(
                list_hit(FontFace::embedded(), &list, viewport, cursor),
                Some(expected_index),
                "line {line}"
            );
        }
    }

    /// A hit test answers "what is under the cursor", not "what can be activated" — a disabled item
    /// still comes back, and the caller is the one that refuses it.
    #[test]
    fn list_hit_returns_a_disabled_item_index() {
        let list = list_of(vec![item("A"), disabled("B")]);
        let viewport = (1920, 1080);
        let cursor = point_on_line(&list, viewport, 1, 0.5);
        assert_eq!(
            list_hit(FontFace::embedded(), &list, viewport, cursor),
            Some(1)
        );
    }

    /// A hidden list draws nothing (extract skips it), so nothing is ever hit.
    #[test]
    fn list_hit_on_a_hidden_list_is_none() {
        let mut list = list_of(vec![item("A"), item("B")]);
        let viewport = (1920, 1080);
        let cursor = point_on_line(&list, viewport, 1, 0.5);
        assert_eq!(
            list_hit(FontFace::embedded(), &list, viewport, cursor),
            Some(1)
        );
        list.visible = false;
        assert_eq!(
            list_hit(FontFace::embedded(), &list, viewport, cursor),
            None
        );
    }

    /// An empty list draws nothing, so nothing is ever hit.
    #[test]
    fn list_hit_on_an_empty_list_is_always_none() {
        let list = UiList::default();
        assert_eq!(
            list_hit(FontFace::embedded(), &list, (1920, 1080), [0.0, 0.0]),
            None
        );
        assert_eq!(
            list_hit(FontFace::embedded(), &list, (1920, 1080), [500.0, 500.0]),
            None
        );
    }

    /// A non-finite cursor coordinate is a no-op, never a panic — the same convention as
    /// `wrap_ui_text`'s non-finite guard.
    #[test]
    fn list_hit_refuses_a_non_finite_cursor() {
        let list = list_of(vec![item("A")]);
        let viewport = (1920, 1080);
        let good = point_on_line(&list, viewport, 0, 0.5);
        assert_eq!(
            list_hit(FontFace::embedded(), &list, viewport, [f32::NAN, good[1]]),
            None
        );
        assert_eq!(
            list_hit(FontFace::embedded(), &list, viewport, [good[0], f32::NAN]),
            None
        );
        assert_eq!(
            list_hit(
                FontFace::embedded(),
                &list,
                viewport,
                [f32::INFINITY, good[1]]
            ),
            None,
            "infinite is also not finite"
        );
    }

    /// `next` past the last item wraps to the first; `prev` from the first wraps to the last.
    #[test]
    fn navigation_wraps_at_both_ends() {
        let next = UiNav {
            next: true,
            ..UiNav::default()
        };
        let prev = UiNav {
            prev: true,
            ..UiNav::default()
        };
        let mut list = list_of(vec![item("A"), item("B"), item("C")]);

        assert_eq!(list.navigate(next), None);
        assert_eq!(list.selected, 1);
        assert_eq!(list.navigate(next), None);
        assert_eq!(list.selected, 2);
        assert_eq!(list.navigate(next), None);
        assert_eq!(list.selected, 0, "next past the end wraps to the first");

        assert_eq!(list.navigate(prev), None);
        assert_eq!(list.selected, 2, "prev from the first wraps to the last");
    }

    /// The cursor never lands on a disabled item, in either direction.
    #[test]
    fn navigation_skips_disabled_items() {
        let next = UiNav {
            next: true,
            ..UiNav::default()
        };
        let prev = UiNav {
            prev: true,
            ..UiNav::default()
        };
        let mut list = list_of(vec![item("A"), disabled("B"), disabled("C"), item("D")]);

        list.navigate(next);
        assert_eq!(list.selected, 3, "two disabled items are stepped over");
        list.navigate(next);
        assert_eq!(list.selected, 0, "and the wrap skips them too");
        list.navigate(prev);
        assert_eq!(list.selected, 3);
    }

    /// An all-disabled list keeps its cursor and refuses activation — nothing is selectable, so
    /// nothing moves and nothing fires.
    #[test]
    fn an_all_disabled_list_keeps_its_cursor_and_refuses_activation() {
        let mut list = list_of(vec![disabled("A"), disabled("B")]);
        list.selected = 1;
        let everything = UiNav {
            prev: true,
            next: true,
            activate: true,
        };
        assert_eq!(list.navigate(everything), None);
        assert_eq!(list.selected, 1);
    }

    /// Activation returns the cursor's index over an enabled item, and `None` over a disabled one —
    /// the case an authored `selected` can produce, since navigation never parks there.
    #[test]
    fn activation_returns_the_index_only_over_an_enabled_item() {
        let activate = UiNav {
            activate: true,
            ..UiNav::default()
        };
        let mut list = list_of(vec![item("A"), disabled("B")]);
        assert_eq!(list.navigate(activate), Some(0));

        list.selected = 1; // authored onto the disabled item
        assert_eq!(list.navigate(activate), None);
        assert_eq!(list.selected, 1, "a refused activation moves nothing");
    }

    /// An out-of-range `selected` is clamped, never an error; an empty list navigates to nothing.
    #[test]
    fn navigation_clamps_a_stale_cursor_and_tolerates_an_empty_list() {
        let mut list = list_of(vec![item("A"), item("B")]);
        list.selected = 99;
        assert_eq!(
            list.navigate(UiNav {
                activate: true,
                ..UiNav::default()
            }),
            Some(1),
            "the clamped cursor is the last item"
        );
        assert_eq!(list.selected, 1);

        let mut empty = UiList {
            selected: 7,
            ..UiList::default()
        };
        assert_eq!(
            empty.navigate(UiNav {
                next: true,
                activate: true,
                ..UiNav::default()
            }),
            None
        );
        assert_eq!(empty.selected, 7, "an empty list has no cursor to clamp");
    }

    /// `UiNav::from_input` reads the fixed bindings, and reads **edges**: a held key is one step.
    #[test]
    fn nav_from_input_reads_the_default_bindings_as_edges() {
        let mut input = InputState::default();
        input.begin_tick();
        input.apply(InputEvent::KeyDown { key: Key::Down });
        input.apply(InputEvent::KeyDown { key: Key::Enter });
        let nav = UiNav::from_input(&input);
        assert!(nav.next && nav.activate && !nav.prev);

        input.begin_tick(); // still held, no new edge
        assert_eq!(UiNav::from_input(&input), UiNav::default());

        input.begin_tick();
        input.apply(InputEvent::KeyDown { key: Key::Up });
        input.apply(InputEvent::KeyDown { key: Key::Space });
        let nav = UiNav::from_input(&input);
        assert!(nav.prev && nav.activate && !nav.next);
    }

    /// The gamepad bindings are the same three intents: d-pad up/down and the south face button.
    #[test]
    fn nav_from_input_reads_the_gamepad_bindings() {
        let mut input = InputState::default();
        input.begin_tick();
        input.apply(InputEvent::GamepadButtonDown {
            button: GamepadButton::DpadDown,
        });
        input.apply(InputEvent::GamepadButtonDown {
            button: GamepadButton::South,
        });
        let nav = UiNav::from_input(&input);
        assert!(nav.next && nav.activate && !nav.prev);

        input.begin_tick();
        input.apply(InputEvent::GamepadButtonDown {
            button: GamepadButton::DpadUp,
        });
        assert!(UiNav::from_input(&input).prev);

        // A button the menu does not bind is not menu intent.
        input.begin_tick();
        input.apply(InputEvent::GamepadButtonDown {
            button: GamepadButton::East,
        });
        assert_eq!(UiNav::from_input(&input), UiNav::default());
    }
}
