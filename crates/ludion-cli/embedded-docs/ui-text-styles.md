---
description: UiText outline, shadow and wrapping, and rotation of UiRect, UiText and UiGroup
---

# UI text styles

**Scope:** styling a `UiText` or `UiList` (wrapping, outline, shadow) and turning an element (`rotation`). Placement, anchors and draw order are `ludion docs screen-ui`; faces are `ludion docs fonts`.

## Wrapping

`UiText.wrap_width` (canvas units, default `0.0`) is off by default — every authored scene keeps
identical geometry until it's set. Above `0.0`, the text wraps to `wrap_width` measured in the face's advance
widths (a line holds at least one glyph); each authored `\n`, `\r\n` or `\r` is still a paragraph break, wrapped independently, and a tab counts as a space; a word is
a run of non-space characters, which may also break after a `-` or `—` (never at a non-breaking space) (runs of spaces collapse to one, leading/trailing spaces drop); a
greedy fill moves a word to the next line when it would not fit, and a word longer than the column
line is hard-broken into line-sized chunks. The block a wrapped `UiText` occupies is
`math::resolve_ui_text(face, &ui_text, viewport)`, wrapped and anchored from the same layout extract
draws, in render-target pixels as `[x, y, w, h]`; the pure helper doing the wrapping is
`math::wrap_ui_text(face, text, size, wrap_width) -> Cow<str>`, still the wrapped string, re-exported
beside `math::measure_ui_text`. It's meant for prose — recaps, flavour text — not for `UiList` items.

## Outline

`outline: { width, color }` on `UiText` and `UiList` draws ink under the glyphs in the same draw. `width` is canvas units
(`0`, or a `color` alpha of `0`, draws nothing); `color` is linear straight RGBA. It is ink, not layout: measuring, wrapping and
hit tests ignore it, and a group's opacity fades it with the text. A negative or non-finite value is `L1618`.

- **SDF faces** take it from the distance field, up to the baked `spread_px` less one screen pixel.
- **Bitmap faces** (the embedded 8x8 among them) dilate by `round(width)` whole source pixels, `1..=4`, as a square.
- Wider than that clamps and warns `L2304` once per element, naming the face and the widest width.

`examples/gallery/ui-text-outline.yaml` shows each case over bright bands; `cargo run -p ludion --example font_text` builds one.

## Shadow

`shadow: { offset, color, softness }` on `UiText` and `UiList` draws a copy of the ink (the glyphs, plus the outline when there is one) in `color`, moved by `offset` canvas units (+Y down), under the text. `softness` (`0`..`1`, default `0`) blurs it. A `color` alpha of `0` draws nothing. It is ink, not layout: measuring, wrapping, the rotation pivot and `ui_hit` ignore it, and a group's opacity fades it and its scale scales `offset`. A non-finite `offset` or `color`, or a `softness` outside `[0, 1]`, is `L1622`.

- **The shadow is the text's silhouette**, so it turns and scales with the text (a rotated text's shadow turns about the same pivot), and an outlined text casts one flat shape in the shadow colour.
- **SDF faces** blur it by `softness`, a fraction of the face's field band (`1` is the widest the field allows).
- **Bitmap faces** (the embedded 8x8 among them) draw it hard; a `softness` above `0` there is ignored and warns `L2307` once per element, naming the face. The offset is not rounded to source pixels.
- **Order.** A run draws every shadow glyph before its fills, so a later glyph's shadow never covers an earlier glyph. Across runs (a `UiList` line, or a fallback-face run) a later run's shadow can reach an earlier run's fill when the offset spans the line gap.
- **Not on** `UiRadial`, `SubtitleView` or world `Text`.

`examples/gallery/ui-text-shadow.yaml` shows each case over bright bands; `cargo run -p ludion --example font_text` builds one.

## Rotation

`rotation: <degrees>` on `UiRect`, `UiText` and `UiGroup` turns the element **clockwise on screen** (`0`, the default, draws as before; a non-finite value is `L1620`). The pivot is the centre of what the element draws: a rect's box (a nine-slice's nine pieces share one), a text run's glyph bounds, a group's box. A `UiTween` `rotation` track adds to the authored value. `UiList` and `UiRadial` have no field; rotate them through a group. A `UiShape` has its own `rotation`, about its origin (`ludion docs ui-shapes`).

Hit-test a rotated rect with `ludion::math::ui_point_in(&rect, viewport, cursor)`; `ui_rect_corners(&rect, viewport)` gives its drawn corners (top-left, top-right, bottom-right, bottom-left). Both are ungrouped and untweened; `ui_hit` answers for a grouped or animated element. `examples/gallery/ui-rotation.yaml` is the golden.

## Next

- `ludion docs screen-ui` — the canvas, anchors and draw order.
- `ludion docs fonts` — SDF and bitmap faces, `spread_px`.
- `ludion docs ui-groups` — rotate a `UiList` or `UiRadial` through a group.
- `ludion docs ui-tweens` — animate `rotation`.
