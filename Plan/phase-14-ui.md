# Plan: Phase 14 — Screen-space UI primitives (E3)

> Status: **landed 2026-07-16 — all 5 waves, one commit each (Lore revs 169–173); E3 READY.**
> Windows `ui-hud` goldens blessed and reviewed at both resolutions; the `goldens/macos/` twins are
> queued for the next mac session. No amendments beyond the per-OS-goldens fact update (rev 168).
>
> Originally: authored 2026-07-16, ready to start. This is the wave-level plan for wave **E3** of the
> first-game arc; its brief is `Plan/engine-first-game.md` §"E3 — Screen-space UI primitives" plus the
> cross-cutting rules there, scheduled by a private roadmap (rule 5). The consumer is
> game phase **G3 — The Run** (a private plan, §G3 / roadmap M3): the real HUD (HP bar, run
> timer, weapon slot, quest tracker), the run-summary screen, and the pause menu — all unbuildable
> today because the only text/quad surface is world-space (perspective-distorted and depth-clipped
> under the tilted camera). The thesis: anchored rects/sprites/text as **builtin components** with
> resolution-independent coordinates and explicit draw order, rendered by a new **overlay pass** after
> post (LDR, unaffected by tonemap/bloom), composed by hand in game systems. **No widget toolkit, no
> layout engine, no focus management** — deferred with the recorded trigger "real friction composing
> by hand" (expected candidate: G6's shop). The E3 brief's "Settled scope" items are decided; its
> "Proposed design" items are confirmed or amended below with current-code evidence (amendments are
> called out inline).

## Current-code facts that shape the work

- **The frame graph's external-pass seam makes the overlay pass cheap.** The HD-2D graph is a fixed
  5-pass pipeline built by `build_hd2d_graph(width, height)`
  (`crates/ludion-renderer/src/graph.rs:51–89`): `scene` (HDR Rgba16Float) → bloom threshold/blur H/
  blur V (half-res HDR) → `tonemap` reads `[scene, bloom_v]` and writes `final`
  (**Rgba8UnormSrgb**, usages `RenderAttachment + CopySrc`). Passes are declared with
  `add_external_pass(name, label, reads, target)` (`crates/ludion-render-graph/src/graph.rs:150–199`);
  dependency edges are **derived** from reads, execution order is a deterministic Kahn sort
  (`schedule.rs:142–176`). Adding a pass needs no new enum variant: a `pass::OVERLAY` label constant,
  one `add_external_pass` call, one match arm in `FrameRecorder::record`
  (`crates/ludion-renderer/src/render.rs:722–741`). The `RecordedPass` a new arm receives exposes
  target view/format/extent, resolved input views, device, encoder, and `begin()`/`begin_clear()`
  (`execute.rs:446–515`).
- **The overlay cannot "read `final` and draw on `final`".** The present target's usages exclude
  `TextureBinding` (`resource.rs:178–184`), and a self-read is an `L2001` cycle. The clean wiring
  (mirrors the bloom→tonemap pattern): add an LDR transient (`Rgba8UnormSrgb`,
  `RenderAttachment + TextureBinding` — `TextureDesc::color`, `resource.rs:158–164`), retarget
  `tonemap` to write it, and have `overlay` read `[ldr]`, blit it fullscreen (the covering-triangle
  pattern of `tonemap.wgsl:11–21`), and draw UI on top into `final`. **This is only done when the
  scene has UI content** (decision 5) so every existing golden keeps the byte-identical 5-pass graph.
- **UI draws into an 8-bit sRGB target, after tonemapping.** `tonemap` resolves HDR→sRGB
  (ACES, `tonemap.wgsl:37–41`); overlay pipelines must be built with `OUTPUT_FORMAT`
  (`render.rs:33`), output linear color, and skip lighting entirely. This is the correct place for
  UI: LDR, no bloom, no tonemap curve applied to HUD colors.
- **The 8×8 bitmap font atlas is reusable as-is for screen-space text.** `font.rs` bakes a one-row
  RGBA8 atlas (space, A–Z, 0–9, ~15 punctuation; lowercase folds to uppercase; coverage lives in the
  alpha channel, `font.rs:80–98`), uploaded once at `Renderer::new` and sampled nearest. The
  world-space text path (`build_text_draw`, `render.rs:573–628`) emits 6 vertices per glyph
  (`TextVertexRaw { position, uv, color }`), pen walks +X per glyph / −Y per newline; the fragment
  half of `text.wgsl` (coverage × color) is exactly what UI text wants. What does **not** port: the
  vertex stage projects by `camera.view_proj`, and the pipeline targets `HDR_FORMAT` — the UI text
  pipeline needs a screen-space (pixel→NDC) vertex stage and an `OUTPUT_FORMAT` target.
- **Sprite instancing is the template for UI rects.** `build_sprite_draws` (`render.rs:522–570`)
  packs consecutive same-texture runs into instance buffers, 6 vertices from
  `@builtin(vertex_index)` with no vertex buffer, texture bind groups against the **hash-keyed GPU
  texture cache** (`texture_cache`, upload-on-miss/clone-on-hit, `render.rs:434–448`); `None`
  texture → the built-in 1×1 white (`render.rs:558–561`). Components reference textures by asset
  path; `extract()` resolves paths → `RenderScene.textures` indices (`extract.rs:542–575`). The UI
  rect path reuses the cache, `resolve_textures`, `texture_bind_group`, and the white-texture
  fallback verbatim; only the instance layout/shader (screen-space, unlit) is new.
- **`extract()` already receives the target dimensions — anchor math can live engine-side.** The
  bridge is `extract(world, (w,h), root)` (`crates/ludion-engine/src/extract.rs`), which currently
  uses the dims for aspect only (`extract.rs:520`). It walks entities in ascending slot order, reads
  builtins via typed `world.get::<T>()`, and emits plain-data `RenderScene` (every field
  `PartialEq`, backing the extract-determinism test). Consequence: extract can resolve
  anchor+offset+size to **target-pixel geometry** and the renderer stays free of anchor concepts —
  `RenderScene` carries resolved rects, preserving the boundary "ludion-renderer consumes only plain
  data" (`ludion-renderer` names neither ECS nor scene types, `lib.rs:8–12`).
- **Nothing in `RenderScene` has an explicit draw-order key today** — sprites sort by view depth,
  text runs by entity slot. Screen space has no view depth; the pinned UI order (`order` then
  `EntityId`, per the brief) is a new sort the extract emits pre-sorted.
- **Builtin registration is append-only with two pinned tripwires.** `builtin_registry()`
  (`crates/ludion-engine/src/components.rs:577–606`) currently ends `…Tilemap, Collider` (10 entries);
  the pinned-order test (`components.rs:719–743`) and the pinned world-hash test
  (`components.rs:750–774`, hash `ec8755d1de2f3f84`) must be re-blessed when appending. Appending
  **last** keeps every existing world hash byte-identical (WorldHash emits only components an entity
  has, in registry order — `hash.rs:99–115`); floats already hash via ryu-shortest (`hash.rs:174–209`),
  so UI f32 fields need no new machinery.
- **Drift safety is a hard gate: every field needs a serde default.**
  `assert_components_drift_safe` (`crates/ludion-testkit/src/drift.rs:72–93`) requires each builtin
  to deserialize from `{}`; a field without a default breaks `ludion dev` snapshot restore (L3016).
- **The inspector dropdown for a unit enum is earned by doc comments, not granted.**
  `deriveVariantOptions` (`editor/app/src/viewer/inspect-fields.ts:203–246`) fires only on a
  `oneOf`-of-`const` schema — which schemars emits **only when every variant carries a doc comment**
  (the `Collider.body` pattern, `collider.rs:58–62`); a bare enum emits `{enum:[…]}` and falls to the
  raw-JSON textarea. The 9-point `anchor` must doc-comment every variant. Vectors (`offset`, `size`)
  and the color swatch (`/colou?r/`, length 3–4… note `[f32;4]` also swatches) derive automatically.
- **Semantic validation rides the registry generically.** `register_validated::<T>` stores
  `T::validate_semantics`; scene validation runs it after a schema-clean check and rebases pointers
  (`crates/ludion-scene/src/validate.rs:57–65`). `Collider`'s L1116 (`collider.rs:83–112`) is the
  template. L-code registration is three-part: `const` in `mod code`, a `CodeEntry` in `CODES`
  (for `ludion explain`), and a `DECLARED_CODES` entry (`codes.rs:1962–2011`) — a consistency test
  fails otherwise. **Free codes: L1105–L1109, L1118–L1120.** E3 takes **L1118**.
- **`schema_hash` will not move.** Components cross the wire as opaque `serde_json::Value`
  (`component.set`/`get`); the protocol schema names no component types, so the schema-drift gate
  stays green with no re-bless and the wire surface stays **26 methods**. There is likewise no
  committed per-component schema snapshot — the order/hash tripwires are the only pins.
- **Gallery discovery is one-scene→one-golden, per pinned OS; a second resolution is
  programmatic.** Every `*.yaml` directly in `examples/gallery/` pairs with
  `goldens/<os>/<stem>.png` at 960×540 — `<os>` from `ludion_testkit::platform_dir()`
  (`windows` RTX 3090 | `macos` M4, anything else `L2205`; the per-OS split landed with the macOS
  port, Lore revs 162–166, mid-authoring of this plan) — and goldens are **blessed once per pinned
  platform** (`crates/ludion-testkit/src/suite.rs:5–12,109`). The gallery test asserts **SSIM
  exactly 1.0** on the pinned GPU and skips gracefully with no adapter (hosted CI has no GPU —
  goldens are validated only on the pinned machines). `manifest.json` overrides are
  one-entry-per-stem and live at the `goldens/` root (content, not platform), so the brief's
  "golden at two resolutions" needs the **programmatic `GoldenTest` precedent**
  (`lantern_sheet_test` in `tests/gallery.rs`: explicit `width`/`height`, custom golden path,
  blessed via `LUDION_BLESS=1`). This Windows session can bless only `goldens/windows/`; the
  macOS twins are a queued item for the next mac session (the established per-OS bless flow,
  `Plan/mac-port-bless.md`).
- **The guide precedent is a full `##` chapter** (Phase 12's "Collision and spatial queries",
  ~128 lines: prose lead → YAML authoring block → schedule snippet → API bullets → large copyable
  `rust,ignore` example → caveats). The guide flows to `LUDION-GUIDE.md` automatically
  (`ludion-cli/build.rs` mirror + `new.rs` embed); a docs-topic edit needs a rebuild to satisfy the
  embedded-docs drift test. No new crate → no new docs topic; `ludion-renderer`, `ludion-engine`,
  and `ludion` topics get the updates.
- **The editor palette is hand-maintained, not registry-driven** (`editor/app/src/editor/palette.ts:36`
  — "Exactly this list — no more"). New components are editable via the inspector on entities that
  carry them but are not spawnable archetypes automatically. E3 adds one palette entry (decision 8)
  rather than rebuilding the palette model.

## Design decisions (settled by this plan)

1. **Two components, not three: `UiRect` (with optional texture) and `UiText` — amendment to the
   brief's `UiRect`/`UiSprite`/`UiText`.** The world-space precedent already proves the shape: there
   is no separate world "Rect" — `Sprite.texture: Option<String>` with `None` → the 1×1 white
   texture *is* the colored quad. A `UiSprite` distinct from `UiRect` would be the same component
   twice minus one field ("before adding a second way, remove the first").
   ```rust
   /// Which of the nine canvas points this element pins to — and, equally, which point of the
   /// element sits on it (its own pivot). Every variant doc-commented (inspector dropdown).
   pub enum Anchor { TopLeft, TopCenter, TopRight, CenterLeft, Center, CenterRight,
                     BottomLeft, BottomCenter, BottomRight }   // serde snake_case, default TopLeft

   pub struct UiRect {
       anchor: Anchor,            // default TopLeft
       offset: [f32; 2],          // canvas units from the anchor point, +Y down; default [0,0]
       size:   [f32; 2],          // canvas units; default [100, 100]
       texture: Option<String>,   // asset path; None → solid color (the Sprite convention)
       color:  [f32; 4],          // linear RGBA tint; default white
       order:  i32,               // draw order, higher = later (on top); default 0
   }
   pub struct UiText {
       anchor: Anchor, offset: [f32; 2],
       text:  String,             // default ""
       size:  f32,                // glyph cell height in canvas units; default 24.0
       color: [f32; 4], order: i32,
   }
   ```
   Every field carries a serde default (the drift-safety gate); every `Anchor` variant carries a doc
   comment (the dropdown gate). An entity may carry Ui components alongside a `Transform` — the
   transform is simply ignored by the UI path (screen space has no world placement).
   *Rejected:* three components per the brief (duplication, evidenced by `Sprite`); a single
   `UiElement` enum-of-kinds component (a text and a rect share only anchor/order — an enum payload
   would put the whole surface behind the inspector's raw-JSON fallback, per the variant-widget
   facts, and read worse in scene YAML).

2. **Coordinates: a 1080-high reference canvas, origin top-left, +Y down; the anchor names both the
   canvas point and the element's own pivot.** The virtual canvas is always 1080 units tall and
   `1080 × aspect` wide; `scale = target_height / 1080` maps canvas → pixels uniformly, so one
   authored HUD fits every resolution (the brief's rule) and width differences are absorbed by
   anchoring, not stretching. One regular pivot rule: the element's *own* anchor-named point sits at
   the canvas anchor point + offset — `BottomRight` pins the rect's bottom-right corner to the
   canvas's bottom-right corner (the corner-HUD case needs no size arithmetic); `Center` centers a
   pause-menu panel with zero offset. `UiText` measures its block (8×8 glyph cells: width =
   longest-line glyph count × `size`, height = line count × `size`) and pivots the same way, which
   gives centered text under center anchors for free. Origin/+Y-down matches the mouse-position
   convention from E2, so `InputState.mouse.position` and UI geometry share one orientation.
   *Rejected:* separate `pivot` field (a second knob with no G3 consumer; the fused rule covers HUD
   corners, bars, and centered panels — trigger to revisit: real content that needs a detached
   pivot); NDC or pixel authoring units (resolution-dependent, the exact failure the brief excludes).

3. **Anchor resolution is one pure engine-side function; the renderer never sees an anchor.**
   `resolve_ui_rect(anchor, offset, size_or_measured_text, viewport (w,h)) -> pixel rect` lives in
   `ludion-engine` next to the components; `extract()` calls it (it already has the dims — fact
   above) and emits **resolved pixel geometry** into `RenderScene.ui`, pre-sorted by
   `(order, EntityId)` (pinned; screen space has no depth to sort by). The same function is
   re-exported through `ludion::math` for game-side hit-testing (`mouse.position` is already in
   render-target pixels — the brief's "games hit-test via the same anchor math, exposed as a
   helper"). It is pure arithmetic — no trig, no transcendentals — so determinism costs nothing.
   `RenderScene.ui` is one `Vec<UiInstance>` with `UiInstance::Rect { rect_px, texture:
   Option<usize>, color }` and `UiInstance::Text { origin_px, cell_px, color, text }` — one
   interleaved, order-sorted vec so rects and text layer correctly (a HUD bar's fill draws over its
   frame, its label over both).
   *Rejected:* resolving in the renderer (spreads anchor semantics across the boundary; the
   engine-side resolve keeps `ludion-renderer` plain-data — evidenced by the crate rule);
   two per-kind vecs with a merge key (an enum vec states the interleaving directly).

4. **Rendering: one overlay pass, two small pipelines, batched in sorted order.** The overlay match
   arm first blits the tonemapped LDR input fullscreen (covering triangle, the `tonemap.wgsl`
   pattern), then walks `RenderScene.ui` in its pinned order, packing **consecutive same-kind,
   same-texture runs** into batches (the sprite-batching pattern): `ui_rect.wgsl` — instanced
   screen-space quads (pixel rect → NDC in the vertex stage, no camera bind group), texel × tint,
   alpha-blended, no depth; `ui_text.wgsl` — the world text path's vertex-buffer shape with a
   pixel→NDC vertex stage, sampling the existing glyph atlas's alpha as coverage. Both pipelines
   target `OUTPUT_FORMAT` (LDR sRGB). Textures ride the existing hash-keyed cache; `None` → the
   white texture.
   *Rejected:* one uber-shader for rect+text (two tiny shaders beat branchy uniforms for
   regularity); depth-buffer layering (UI order is a list sort, not a depth test).

5. **The overlay pass exists only when the scene has UI content — zero golden churn by
   construction.** `build_hd2d_graph` gains a `ui: bool` (or equivalent) input: `false` → the
   exact current 5-pass graph, byte-identical output for every existing golden and an unchanged
   `--dump-graph` for UI-free scenes; `true` → `tonemap` retargets to the new `ldr` transient and
   `overlay` reads `[ldr]`, writes `final`. The flag derives from `RenderScene.ui.is_empty()` at
   render time (the graph is already rebuilt per render call — fact above).
   *Rejected:* an always-on overlay/blit (risks a 12-golden bless event on an sRGB round-trip
   nuance and burns a fullscreen pass on every UI-free scene, for no consumer benefit; if a future
   wave wants the graph static it can revisit with evidence).

6. **Registration: both components appended after `Collider`, `register_validated`, one new L-code
   `L1118`.** Order becomes `[…, Tilemap, Collider, UiRect, UiText]`; the two pinned tripwire tests
   are re-blessed (order list + world-hash constant — the hash of a Ui-free world stays byte-identical
   by the append-last rule, which the existing test proves). Semantic validation (the `Collider`
   template): **L1118 "UI element size is not positive and finite"** covering `UiRect.size` axes and
   `UiText.size`, with component-relative JSON Pointers (`/size/0`, `/size`); registered three-part
   (const + `CODES` entry + `DECLARED_CODES`). No other validation — a zero-`order`, empty-string,
   off-canvas element is legal and harmless.
   *Rejected:* validating `offset` bounds (off-screen placement is a legitimate authoring state, not
   an error); separate codes per component (one semantic, one code — the L1116 mold).

7. **Goldens: one gallery HUD scene, blessed at two resolutions — discovery at 960×540 plus a
   programmatic 1920×1080 `GoldenTest`.** `examples/gallery/ui-hud.yaml`: a HUD mock exercising the
   surface — corner-anchored bars (rect-over-rect layering proving `order`), center panel with
   translucency, text at three anchors, one textured rect — over a minimal world scene (proving UI
   draws above post, unaffected by bloom). Discovery pairs it with `goldens/<os>/ui-hud.png`
   (960×540); a hand-written test in `tests/gallery.rs` (the `lantern_sheet_test` mold) renders the
   same scene at 1920×1080 against `goldens/<os>/ui-hud-1080.png` — same-aspect ×2 proves uniform
   canvas scaling on real pixels. This wave blesses the `windows/` pair; the `macos/` pair is
   queued for the next mac session (notebook TODO — until then the mac gallery suite will flag the
   missing goldens, the expected per-OS bless flow). Aspect-*change* behavior (anchors hugging corners as canvas width grows) is proven
   exactly by `resolve_ui_rect` unit tests across 16:9/4:3/21:9 viewports — cheap and precise where
   a third golden would be blunt.
   *Rejected:* two scene files (same content twice, drift risk); a `--width/--height` on
   `ludion test` (new CLI surface for one test's need — the programmatic seam already exists).

8. **Editor: the inspector works by construction; the palette gains one "HUD panel" entry.** The
   inspector picks up both components from `engine.describe` automatically; with decision 1's doc
   comments, `anchor` renders as a dropdown, `offset`/`size` as vectors, `color` as a swatch,
   `order` as a number — a vitest against the real schemars output pins this (the degrade-nowhere
   check). The hand-maintained palette (fact above) gets one `UiRect` archetype ("HUD panel") so
   designers can spawn UI at all; a fuller UI-authoring story is deferred with the widget-toolkit
   trigger.
   *Rejected:* making the palette registry-driven (a real editor feature, out of E3's scope — and
   the palette's staleness notes are pre-existing tech debt, not this wave's to fix drive-by).

9. **Additive-only, confirmed:** no wire change (components are opaque `Value`s on the wire —
   surface stays **26 methods**, `schema_hash` unmoved), no snapshot growth (components are already
   drift-safe snapshot citizens), no editor-private endpoint (the inspector/palette ride existing
   commands), one new L-code (L1118). Every pre-E3 golden stays byte-identical by decision 5's
   construction. **If any wave discovers otherwise, that is a flagged decision, not a drive-by.**

## Waves

Ordering rationale: components + registry first (the data model everything else names, Wave 1) →
the pure geometry + extract seam (Wave 2) → the renderer pass on top of both (Wave 3) → the visible
proof + editor checks (Wave 4) → facade surface, guide, docs, exit (Wave 5). Each wave is
independently landable, one commit.

### Wave 1 — components, registry, validation (`UiRect`, `UiText`, `Anchor`, L1118)
- `Anchor`/`UiRect`/`UiText` in `crates/ludion-engine/src/components.rs` per decisions 1–2 (serde
  defaults on every field; doc comments on every `Anchor` variant); `SemanticValidate` impls;
  appended to `builtin_registry` after `Collider` via `register_validated`; L1118 registered
  three-part in `crates/ludion-core/src/codes.rs`.
- Re-bless the pinned registry-order test (now 12 names) and the pinned world-hash test (appending
  last keeps the Ui-free hash — the existing test's own assertion pattern proves it).
- **Exit gate (mechanized):** serde round-trip + `{}`-deserialization (drift) tests for both
  components; schema tests proving `anchor` emits `oneOf`-of-`const` (the dropdown contract);
  L1118 semantic tests (negative/zero/NaN size → pointer-carrying diagnostics; valid docs clean);
  `ludion explain L1118` works; scene validation picks the components up generically
  (a `check-scene` fixture); `node scripts/check.mjs` green.

### Wave 2 — anchor geometry + extraction (`resolve_ui_rect`, `RenderScene.ui`)
- `resolve_ui_rect` (+ the text-measure helper) in `ludion-engine` per decisions 2–3, pure and
  viewport-parameterized; `UiInstance` on `RenderScene` (plain data, `PartialEq`); `extract()`
  walks Ui components, resolves geometry, emits the `(order, EntityId)`-sorted vec.
- **Exit gate (mechanized):** resolve unit tests — all nine anchors at 960×540, 1920×1080 (×2
  uniform scaling of every rect), and 4:3/21:9 viewports (corners hug corners, center stays
  centered); text-measure tests (multi-line, empty); sort-order tests (order ties break on
  EntityId; interleaved kinds); extract tests (component → expected `UiInstance`, texture path →
  index via the existing dedup, `Transform` ignored); the extract-determinism test extended to a
  UI-carrying world; `node scripts/check.mjs` green.

### Wave 3 — the overlay pass (graph wiring, `ui_rect.wgsl`, `ui_text.wgsl`)
- Graph: the conditional `ldr` transient + `overlay` external pass per decisions 4–5
  (`pass::OVERLAY` constant, retargeted tonemap, derived edges); `FrameRecorder` arm: LDR blit +
  order-walk batching (same-kind, same-texture runs) through the two new `OUTPUT_FORMAT` pipelines;
  glyph atlas + texture cache + white-texture reuse.
- **Exit gate (mechanized):** graph tests — UI-free scene builds the exact current 5-pass schedule,
  UI-carrying scene schedules `overlay` after `tonemap` with the derived edge (the Kahn-order
  assertion pattern); every existing gallery golden **byte-identical** (run locally — the
  construction proof made visible); a smoke render of a UI scene produces non-background pixels in
  the anchored regions (bytes asserted, golden comes in Wave 4); `--dump-graph` on a UI scene shows
  the pass; `node scripts/check.mjs` green.

### Wave 4 — gallery scene, two-resolution goldens, editor checks
- `examples/gallery/ui-hud.yaml` per decision 7; bless `goldens/windows/ui-hud.png` (960×540,
  discovery) and `goldens/windows/ui-hud-1080.png` (1920×1080, programmatic test in
  `tests/gallery.rs`, the `lantern_sheet` mold — golden path via `platform_dir()`).
- Editor: the vitest pinning widget derivation from the real `UiRect`/`UiText` schemas (anchor
  dropdown, vectors, swatch, number — decision 8); the palette's "HUD panel" entry.
- **Exit gate (mechanized):** both goldens blessed and green at SSIM exactly 1.0 on the pinned GPU;
  gallery discovery picks `ui-hud.yaml` up with zero config; the two-resolution test green; vitest
  + oxlint + tsc green; every pre-E3 golden still byte-identical; `node scripts/check.mjs` green.

### Wave 5 — facade surface, example, guide, docs, exit
- `ludion::prelude` re-exports `UiRect`/`UiText`/`Anchor`; `ludion::math` re-exports
  `resolve_ui_rect` (the hit-test helper); prelude doc list extended.
- A small facade **example** (`crates/ludion/examples/hud_bar.rs`): a system that drives a
  UiRect HP bar's `size` from game state and centers a `UiText` label — the copyable G3 HUD
  pattern — with a behavioral test asserting the driven `size`/determinism (two-run WorldHash,
  the `cursor_aim.rs` mold).
- Engine-guide chapter **"Screen-space UI"** (placed after "Collision and spatial queries", the
  Phase-12 chapter shape): authoring YAML, the canvas/anchor/pivot model, order semantics, the
  hit-test recipe (`mouse.position` + `resolve_ui_rect`), the world-`Text`-vs-`UiText` distinction
  (both stay — different purposes), the "UI is post-tonemap LDR" note. Docs topics updated via the
  `ludion-docs` skill: `ludion-renderer` (overlay pass), `ludion-engine` (components + resolve),
  `ludion` (prelude/math); embedded-docs drift satisfied by rebuild.
- **Exit gate (roadmap "ready"):** wire surface still 26 + `schema_hash` unmoved (by construction,
  decision 9); all pre-E3 goldens byte-identical; example + behavioral test green; guide chapter +
  docs topics shipped; `node scripts/check.mjs` green. **Handoff to G3:** HUD, run summary, and
  pause menu are composable from `UiRect`/`UiText` + `order`; G3 deletes G0's interim world-space
  countdown. **No human gate is owned by E3 itself** — the M3 human gate (G3's "the triage moment
  happened") judges the HUD in anger; the two blessed goldens get eyes at bless time.

## Explicitly out of scope (triggers recorded)

- **Widget toolkit / layout engine / focus management** — trigger: recorded friction composing by
  hand (expected: G6's shop). The brief's core deferral.
- **Input routing to UI** (click-on-button dispatch) — games hit-test via `InputState.mouse` +
  `resolve_ui_rect`; trigger: the same widget-toolkit friction.
- **9-slice, UI animation, styling/theming systems** — trigger: real content demand post-G3.
- **A detached `pivot` field** (anchor names both ends, decision 2) — trigger: content that needs
  an element pinned by a non-anchor point.
- **UI-specific fonts / font assets** — the 8×8 bitmap atlas is the one text surface engine-wide;
  trigger: content the fixed glyph set can't express (shared with any future world-text demand).
- **Registry-driven editor palette** — one hand-added entry ships (decision 8); trigger: palette
  staleness becoming a recurring defect class.
- **`ludion test` resolution flags** — the programmatic `GoldenTest` seam covers multi-resolution;
  trigger: a third consumer of non-default-resolution suites.

## Risks

- **sRGB/blend nuances in the overlay pass.** Drawing into `Rgba8UnormSrgb` with alpha blending
  blends in linear space post-decode — correct, but tint/alpha arithmetic must match what the
  goldens bless. Contained by Wave 3's byte-identity gate on existing goldens (the conditional
  graph makes it structural) and Wave 4's two blessed goldens getting human eyes at bless time.
- **Text measurement drift between resolve and draw.** The anchor pivot uses measured text size;
  if the renderer's pen advance and the engine's measure disagree, centered text sits off-pivot.
  Contained by deriving both from the same constants (8×8 cell, `size` scaling) and a Wave 2 test
  asserting measure == the vertex generator's extent for known strings (checked again visually in
  the Wave 4 golden).
- **Golden validation is local-only, and per-OS.** Hosted CI has no GPU adapter; the two UI
  goldens are proven on the pinned Windows machine this phase (the standing tier-2 posture). The
  macOS twins don't exist until the next mac session blesses them — queued in the notebook; the
  mac gallery suite is expected to flag them until then.
- **Registry append mistakes are silent until re-bless.** Both pinned tripwires (order list, hash
  constant) must move in Wave 1's commit — the tests make a wrong append loud, but only if run;
  check.mjs runs them (`cargo test --workspace`).
- **Sequencing vs open gates.** E3 shares only the guide/docs files with the open Phase 8/9/M0/M1
  human gates; it adds no wire method and (by decision 5) no golden churn, so it is safe to run
  with those gates open. Per roadmap rule 2, E3 is the wave in flight during M2 — the user directed
  its start on 2026-07-16 with M1's human gates still queued; those gates are unaffected and stay
  queued.
