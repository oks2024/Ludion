---
description: UiRect and UiText: the reference canvas, anchors, draw order, and driving a HUD from state
---

# Screen-space UI

**Scope:** HUDs and overlays — the virtual canvas, the nine anchors, draw order, and driving an element from game state. Text wrapping, outline, shadow and rotation are `ludion docs ui-text-styles`. World-space `Text` is in components; the mouse itself is systems; the selectable-menu widget is `ludion docs ui-list`.

Screen-space UI is for HUDs, menus, and overlays: elements that never move with the camera, never
get depth-clipped, and read the same size at every output resolution. Two builtin components carry
it: `UiRect` (a colored or textured quad — `texture: None` is the `Sprite` convention for a plain
tinted rect, not a separate rect/sprite pair) and `UiText` (a run of text in the same faces `Text`
draws in world space — the `default` face of `fonts.yaml`, else the embedded 8×8 face, unless `font` names one — laid out in screen space instead). Both are anchored to one of nine canvas
points via `Anchor`. World-space `Text` still exists and still matters — a floating damage number
over a character, a nameplate, anything that should perspective-scale and occlude with the scene —
UI is for everything that should not.

**Coordinate model.** UI is authored against a virtual **1080-unit-tall reference canvas**; its width
is `1080 * aspect` for whatever resolution you actually render at, so one authored HUD fits every
output resolution — a 4:3 window and a 21:9 monitor both show the same bar in the same corner, just a
different amount of canvas to either side. The origin is **top-left**, **+Y down**, matching
`InputState.mouse.position` — UI geometry and the mouse share one orientation. `anchor` names *both*
the canvas point an element pins to *and* the element's own pivot: `BottomRight` with zero offset
pins the element's own bottom-right corner exactly on the canvas's bottom-right corner (a corner HUD
needs no size arithmetic to hug it), `Center` centers the element on the canvas center. `offset`
moves the element from that shared point, in canvas units, `+Y down`. There is no separate `pivot`
field — one anchor names both ends. To cover the whole screen, set `fill: true` on a `UiRect`: it
resolves to the full render target at any extent or aspect (`anchor`, `offset` and `size` are ignored;
a non-default `offset` or `size` beside it is warning `L1316`). A texture or atlas frame stretches
over it; `UiText` and `UiList` have no `fill`.

**Authoring.** A bottom-left HP bar — a darker frame `UiRect` with a brighter fill `UiRect` on top of
it (`order` is what keeps the fill drawn over the frame, not the other way around) — plus a
center-anchored label:

```yaml
  - name: hp-frame
    components:
      UiRect:
        anchor: bottom_left
        offset:
          - 24.0
          - -24.0
        size:
          - 200.0
          - 32.0
        color:
          - 0.05
          - 0.05
          - 0.06
          - 1.0
        order: 0
  - name: hp-fill
    components:
      UiRect:
        anchor: bottom_left
        offset:
          - 30.0
          - -30.0
        size:                   # driven from game state — see the system below
          - 150.0
          - 20.0
        color:
          - 0.2
          - 0.85
          - 0.3
          - 1.0
        order: 1
  - name: title
    components:
      UiText:
        anchor: center
        text: "GALLERY HUD"
        size: 48.0
        wrap_width: 0.0         # 0 = no wrap (default)
        color:
          - 0.95
          - 0.9
          - 0.7
          - 1.0
        order: 1
```

`UiRect.texture` (an asset path, `None` for a flat color) rides the same hash-keyed texture cache as
`Sprite`. A HUD icon already packed into the game's committed atlas need not be exported as a loose
PNG: set `atlas` (a `kind: atlas` document path, relative to the content root) and `frame` (a region
name) **instead of** `texture`, and the rect samples that region's sub-rectangle — the pair a world
`Sprite` takes (`ludion docs sprite-sheets`), under the same rules. `texture` beside `atlas`, or
either half of the pair alone, is `L1151`; a region the atlas does not carry is `L1152`, both from
`check-scene` as well as extract. `size` stays the authored canvas units — a region supplies pixels
and UVs, never the quad's extent — and rects sharing one atlas batch into one draw. `visible: false`
(default `true`) skips a rect entirely — no draw, no texture resolution — the authored way to hide it
rather than a zero-size or alpha-zero trick. `view: <name>` shows a secondary camera's texture instead of a
`texture` or `atlas` (`ludion docs render-views`): it is the rect's whole image, and `texture`, `atlas`, `frame`,
`slice` or `uv_rect` beside it is `L2373`.

```yaml
  - name: life-icon
    components:
      UiRect:
        anchor: top_left
        offset:
          - 24.0
          - 24.0
        size:
          - 48.0
          - 48.0
        atlas: sprites/hud.atlas.yaml   # instead of `texture:` — one region of the committed atlas
        frame: icon/life
        order: 2
```

`UiText.font` names a baked face from `fonts.yaml` (`ludion docs fonts`); without it the text uses the `default` face of `fonts.yaml`, else the embedded 8×8 face described below. `UiText.size` is the glyph cell height in canvas units; its block measures width = longest
line's advance width (glyph count × `size` on the 8×8 face), height = line count × `size` — so a `Center` anchor centers the whole
measured block for free, and an empty `text: ""` is legal and draws nothing (a common toggle: swap a
label's text between `""` and a message rather than adding or removing the entity). `visible: false`
(default `true`) is the same off switch `UiRect` offers, orthogonal to an empty `text`.

`UiText.wrap_width`, outline, shadow and `rotation` are `ludion docs ui-text-styles`.

**Order.** Every `UiRect`/`UiText` in a scene draws in one pass, sorted by `(order, EntityId)` —
higher `order` draws later, i.e. on top; a tie breaks on ascending `EntityId` (pinned, reproducible).
Rects and text interleave in that one order — there is no "rects then text" pass split, so a fill can
sit over its frame and a label over both, by `order` alone.

**UI is post-tonemap LDR, unaffected by bloom.** The overlay pass runs after the scene's bloom
and tonemap, drawing straight into the 8-bit output — the same pixels a screenshot or an attached
viewer sees, with no HDR bleed and no tonemap curve applied to HUD colors. A UI-free scene skips the
pass entirely (zero cost, zero golden churn); it exists only when a scene actually carries a
`UiRect`/`UiText`.

**Driving a bar from game state** — plain typed world access, the same shape as any other system:

```rust,ignore
use ludion::prelude::*;

/// Marks the fill `UiRect` so the system can find it (the entity carrying `Health` is separate).
#[derive(Serialize, Deserialize, JsonSchema, Clone, Copy, Debug, Default)]
struct HpFill {}

fn drive_hp_bar(world: &mut World, _ctx: &SystemCtx) {
    let Some(fraction) = world
        .query::<(&Health,)>()
        .next()
        .map(|(health, )| (health.current / health.max).clamp(0.0, 1.0))
    else {
        return;
    };
    for (_marker, fill) in world.query::<(&HpFill, &mut UiRect)>() {
        fill.size[0] = MAX_FILL_WIDTH * fraction; // MAX_FILL_WIDTH matches the frame's authored width
    }
}
```

The complete, runnable version — a full drain-and-drive HUD with a "GAME OVER" label — is the
`hud_bar` example (`cargo run -p ludion --example hud_bar`).

**Mouse hit-testing.** There is no input routing to UI — no click-on-button dispatch — so a game
hit-tests by hand with the same anchor math the renderer draws with, re-exported as
`math::resolve_ui_rect` (or `math::resolve_ui_rect_of(&rect, viewport)`, which takes the `UiRect`
itself and honours `fill`):

```rust,ignore
let cursor = world.resource::<InputState>().unwrap().mouse().position(); // render-target pixels
let viewport = world.resource::<RenderExtent>().copied().unwrap_or_default().size();
let rect = math::resolve_ui_rect(Anchor::BottomLeft, [24.0, -24.0], [200.0, 32.0], viewport);
let hit = cursor[0] >= rect[0] && cursor[0] < rect[0] + rect[2]
       && cursor[1] >= rect[1] && cursor[1] < rect[1] + rect[3];
```

`mouse().position()` is already in render-target pixels — the same space `resolve_ui_rect` resolves
to — so the hit test is a plain point-in-rect comparison: no unprojection, no camera (unlike the aim
recipe in `ludion docs systems`). A windowed session's letterbox transform has already mapped the
mouse into that space for you. Hit-test a `UiText` against `math::resolve_ui_text`, so a click on a
wrapped label's second line lands inside it.

**Selectable lists: `UiList`.** `UiList` is the engine's one menu surface — a vertical stack of
selectable text lines with one cursor (a purchase list, a pause menu, a dialogue choice). See
`ludion docs ui-list` for its format, the cursor/activation contract, and the windowing rule.

**Caveats.**
- `size` (`UiRect.size`'s two axes and `UiText.size`) must be strictly positive and finite —
  `L1118`; an off-canvas `offset` is legal and harmless (nothing enforces on-screen placement).
  Author `visible: false` to hide an element; `size` stays positive either way.
- The embedded face's glyph set is `Text`'s 8×8 uppercase bitmap atlas (a named face has its own charset, `ludion docs fonts`): lowercase folds to uppercase, an unmapped
  character renders blank — the same rule for `UiText` as for world-space `Text`.
- A `Transform` on a UI entity is simply ignored — screen space has no world placement.
- Mouse coordinates are render-target pixels, not window pixels; a windowed session's letterbox
  transform already accounts for the difference (`ludion docs systems`).

`examples/gallery/ui-hud.yaml` is the full gallery HUD (corner bars, a translucent center panel, text
at three anchors, a textured rect) over a lit, bloom-triggering world — the golden proof that UI draws
above post-processing, unaffected. `examples/gallery/ui-atlas-icons.yaml` is the atlas variant: a row
of life icons and a jewel drawn from regions of the sprites gallery's committed atlas, proving a
`UiRect` samples an atlas region exactly as a world `Sprite` does.

## Moving elements

`UiTween` animates opacity, offset, scale, rotation, size and colour closed-form (`ludion docs ui-tweens`); `UiWorldAnchor` pins an element to a 3D target and clamps it to the screen edge (`ludion docs ui-world-anchor`). Both resolve in `extract`: nothing is written per tick. A textured rect can sample a sub-rect (`uv_rect`) or draw as a nine-slice frame (`slice`): `ludion docs ui-nine-slice`. A `UiGroup` is a box other elements anchor, scale, fade and hide inside (`group: <ref>`): `ludion docs ui-groups`.

## Filtering the frame

`ScreenEffect` draws a CRT, analog-video or scan filter over the whole frame or a rectangle of it, under every UI element above its `order`: `ludion docs screen-effects`.

## Next

- `ludion docs ui-nine-slice` — `uv_rect` and resizable frames.
- `ludion docs ui-tweens` — closed-form HUD motion.
- `ludion docs ui-world-anchor` — HUD elements that follow 3D targets.
- `ludion docs ui-groups` — a box that scales, fades and hides the UI inside it.
- `ludion docs sprites` — the sprite layer's hub; a `UiRect` spends the same committed atlas.
- `ludion docs ui-text-styles` — text wrapping, outline, shadow, rotation.
- `ludion docs ui-list` — the selectable-menu widget: format, cursor/activation, windowing.
- `ludion docs fonts` — TrueType faces for `UiText`/`UiList`/`Text`.
- `ludion docs systems` — the system contract and the determinism rules.
- `ludion docs components` — declaring components, and the built-in visual set.
- `ludion docs screen-effects` — CRT, analog-video and scan filters over the frame or a region.
- `ludion docs post-processing` — the imaging chain.
- `ludion docs verify-loop` — the `cargo test` verification loop.
