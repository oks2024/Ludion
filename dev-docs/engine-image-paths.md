---
description: extract() and the standalone image paths: render_scene, the model, sweep and sprite sheets
---

# Engine: extract and the image paths

**Scope:** The world→`RenderScene` bridge and the standalone file→image paths the CLI drives — a
scene, a lone model document, a swept contact sheet, and the two CPU-composed sprite instruments. What happens to a `RenderScene` after this
is `ludion devdocs ludion-renderer`; the tick-to-pixels path for a *live* subscriber is
`ludion devdocs frame-streaming`; the golden harness that kind-dispatches these is
`ludion devdocs ludion-testkit`.

## Layout

- `crates/ludion-engine/src/extract.rs` — `extract(world, viewport, project_root) -> RenderScene`.
- `crates/ludion-engine/src/render.rs` — `render_scene(path, w, h, collect_timings)`, the standalone lockstep path, and the CPU prologue the other two reuse; `render_scene_with_builder` + `engine_at_frame`, its game-schedule twin.
- `crates/ludion-engine/src/ui.rs` — pure screen-space UI geometry, no ECS or asset deps: `resolve_ui_rect(anchor, offset, size, viewport)` and `measure_ui_text`/`resolve_ui_text`. `ludion::math` re-exports all three for game-side mouse hit-testing, so a hit test is the same arithmetic the overlay pass draws with.
- `crates/ludion-engine/src/model_sheet.rs` — `render_model_sheet(model, views, sheet_width, sheet_height, rig)`. Read the module doc first: it carries the exact framing/lighting formulas and the rig lift's unit-normalization caveat. Its posed forms (`clip`/`time`/`strip` and the gap rasterization) are `ludion devdocs assembly-animation`.
- `crates/ludion-engine/src/sweep.rs` — `render_sweep_sheet(path, base, pointer, values, width, height)` → `SweepOutcome`. Read the module doc first: it carries the grid rule and why a sheet is not a golden.
- `crates/ludion-engine/src/sprite_sheet.rs` — `render_sprite_sheet(&SpriteSheetRequest)` → `SpriteSheetOutcome` and `render_atlas_sheet(path)` → `Image`.

## Rules

- **A broken input never reaches the GPU.** Every path runs its whole CPU prologue — read, parse,
  overrides, validate, instantiate, tick 0, extract — *before* a `Renderer` exists, so an
  unresolvable pointer (`L4008`), a schema-rejected value (`L1014`) or a bad document fails with its
  ordinary diagnostic instead of an adapter error.
- **One adapter per sheet, not one per cell.** Both sheet harnesses create a single `Renderer` and
  reuse it for every view or value, stitching cells with a plain CPU row-copy.
- **A sheet is never a golden.** Its burned-in labels are `UiText` entities, which schedule the
  overlay pass — so a cell will not match the same `render --set` byte for byte. Golden anything that
  matters through `render`; use a sheet only to *choose* a value.
- **The light list's order is a contract, not an implementation detail.** `extract_lights` hands the
  renderer *every* placed light — there is no cap and no selection — in the exact ascending-`EntityId`
  order the walk collected them, unsorted. The renderer uploads that array verbatim and a fragment
  sums its cluster cell's lights in ascending index order, so this order **is** the per-fragment
  floating-point summation order of every lit pixel: sorting or filtering here churns every lit
  golden and voids the clustered loop's proof that it is arithmetically identical to a flat loop.
  `extract_lights`' doc comment is the contract; the consuming half is
  `ludion devdocs renderer-clustering`.
- **The canonical writer stays the only writer.** The model-sheet rig lift re-emits borrowed entities
  through `ludion_scene::emit` rather than formatting floats itself, so the harness never grows a
  second float formatter.

## Dataflow

**`extract()`** walks `World::entities()` in ascending slot order, converts Euler-degree rotations to
quaternions, resolves loose scene textures (sprites, UI) via `ludion_assets::load_png` — failures
*collected* as `L1101`/`L1102` with JSON Pointers and a nearest-filename suggestion — and emits a
`RenderScene` holding no ECS references and no paths. An asset mesh's own texture maps are a separate
seam: they resolve through `mesh_asset::load` (`ludion devdocs ludion-import`'s Gotchas has the
`check-scene` caveat), collecting the same `L1101`/`L1102` codes at each map's own JSON Pointer
(`/base_color_texture`, …) rather than this seam's. Every component derives `Serialize + Deserialize + JsonSchema` with per-field
defaults, so partial YAML instantiates cleanly and `check-scene` gets a schema per component.

It also resolves `UiRect`/`UiText` entities: `ui.rs` turns each element's `anchor`/`offset`/
`size-or-measured-text` into a pixel-space rect against the render target's actual `(width, height)`
— extract already receives the dims for aspect, so anchor math costs nothing new — and the results
land in `RenderScene.ui` **pre-sorted by `(order, EntityId)`**. Screen space has no view depth, so
this is the one place UI draw order is decided. `RenderScene.ui` is empty for a UI-free world, which
is what lets the renderer's overlay pass stay unbuilt for every existing scene.

**`render_scene()`** is the file→image lockstep path: read → check → instantiate → tick(0) → extract →
render → readback, one thread. The `render.screenshot` handler reuses the same extract-and-render core
against the engine's *current* world and lazy renderer, so the two cannot diverge.

**`render_scene_with_builder()`** is its game-side twin, for the one caller that holds the game's
`EngineBuilder` (a game binary's `--render-out`, `GoldenTest::with_builder`). `render_scene`'s ticks
run `Schedule::new()`'s empty schedule, so its frame N is frame 0 with nothing but the sim clock
moved; this one builds the game's engine, steps it `frames` times through the ordinary `sim.step`
command, and renders through `Engine::render_png` — the same core `render.screenshot` uses — so a
golden can be an early wave rather than the scene at rest. Its CPU half, `engine_at_frame()`, returns
the ticked engine before any adapter is requested, which is what lets a headless test assert on the
ticks without a GPU.

**`render_model_sheet()`** takes a model document instead of a scene. It peeks `kind:`
(`ludion_scene::peek_kind`) to load it as voxel or mesh, computes the model-space AABB, and derives
one normalization — unit bounding sphere, bottom-center at the origin — that one hand-tuned
camera/light rig frames from six fixed directions. Each requested view's scene is *synthesized as
strict-YAML text*, then checked and extracted like any other.

Its optional `rig` argument borrows lighting instead of synthesizing it: the rig scene is validated
with the ordinary `ludion_scene::check`, every entity carrying an `Environment`, `PostProcess`,
`DirectionalLight` or `PointLight` is copied whole and appended, a rig entity named `ground` donates
its `Mesh` color to the ground plane, and a rig carrying any light suppresses the key/fill pair
outright. **With no rig the synthesized text is byte-identical to what it always was**, pinned by a
unit test — which is why every existing model golden is untouched. The subject is normalized to a unit
sphere, so sky and directional light read true while a game rig's absolute point-light *ranges* do
not.

**`render_sweep_sheet()`** is the one built for a human sitting rather than a regression: N renders of
the *same* authored scene, one per candidate value at one JSON Pointer. It reads the file once, then
runs `render_scene`'s CPU prologue per cell with the fixed `base` overrides plus the swept value. The
grid is near-square (`cols = ceil(sqrt(N))`, `rows = ceil(N / cols)`) because a taste call needs cell
*area*, capped at `MAX_SWEEP_CELLS` (16) with an `L4008` rather than a silent truncation. Each cell's
label is a `UiText` entity **appended** to that cell's scene — deliberately a direct scene mutation,
because `apply_overrides` cannot create an entity and that restriction is worth keeping.
`SweepOutcome` carries everything the caller must echo: `pointer`, `original`, `applied_base`, the
grid shape, and a `SweepCell` per value.

**`render_sprite_sheet()`** and **`render_atlas_sheet()`** are the pair that never reaches a
`Renderer` at all — `voxel_sheet`'s "zero pixels" shape, and its label stamp (`ludion_font::stamp_label` over the
embedded face) and blit helpers, raised to `pub(crate)` rather than copied a third time. The sprite sheet reads a `kind: sprite` document,
applies its own `--set` domain (`/seed`, `/palette/<glyph>/…`, `/frames/<name>/pivot/<0|1>`,
`/effects/<i>/<param>[/<j>]` — anything else is `L4008`, and an override that breaks the document is
re-caught by `validate_sprite`), bakes it through `ludion-sprite`, and composes: every frame upscaled
to legibility with a texel ruler on the first tile, every frame again at the authored game scale, and
a footer that **echoes the effect list it applied** (`Plan/sprite-engine.md` — an effect painted into
glyphs is a defect, so the sheet says out loud which ops ran). With a sweep it becomes one cell per
candidate value instead. The atlas overview composites the committed premultiplied PNG over the sheet
background and outlines and names every region. Both are pure integer composition over baked pixels,
so two runs write byte-identical bytes on either platform — but they are still sheets, not goldens.

The `--set` primitive both sheets compose with exists because `WorldHash` hashes every registered
component's data, so tuning a look *on disk* would invalidate every committed replay. The whole
tuning loop therefore costs zero replay churn.

## Gotchas

- `--out` for a model sheet defaults to `<stem>.sheet.png` from the model's own file stem
  (`lantern.voxel.yaml` → `lantern.sheet.png`), in the current directory — **not** beside the model.
- The extract-side traps — typed access hiding an unregistered component, the camera's local −Z and
  `L2101`, path-string texture dedup — are in `ludion devdocs ludion-engine`'s `## Gotchas`, which
  owns them for the whole crate.

## Verify

```
cargo nextest run -p ludion-engine --test it -E 'test(/^extract::/)'
cargo nextest run -p ludion-engine --test it -E 'test(/^sprite_sheet::/)'   # CPU-only: no adapter, exact-pixel probes
cargo run -p ludion-cli -- render --scene examples/gallery/lights.yaml --out target/lights.png
cargo run -p ludion-cli -- render --model examples/gallery/voxels/lantern.voxel.yaml --out target/lantern.sheet.png
cargo nextest run -p ludion-engine --test it -E 'test(/^gpu_render::/)'     # graceful-skips without an adapter (L2003)
```
