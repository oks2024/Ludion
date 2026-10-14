# Plan: UI widgets — the selectable list, and nothing else

> Status: **Waves 1–3 SHIPPED (2026-08-19, 2026-09-02, 2026-09-03) — mechanized half COMPLETE; the
> Wave 3 human gate (controller-only menu sitting) rides the next gate batch.** Authored from a private gap
> analysis of the reference game's menu needs; companion plans are private.
>
> Wave 3 log (SHIPPED 2026-09-03): `ui::list_hit(list, viewport, cursor) -> Option<u32>` per
> decision 11 — the pixel counterpart of `navigate`, walking `resolve_ui_list` + `ui_list_lines` so
> a hit and the drawn line cannot disagree: the row spans the block width (not the item's own glyphs),
> the title line, the `line_spacing` gap and the `padding` band miss, a windowed row comes back as
> `window.start + row` for free, and **a disabled item is returned** — the caller refuses, the same
> split as everywhere else. Re-exported through `ludion::math`; one `docs/ui-list.md` rule. Editor:
> the `ui-list` palette archetype (`Menu list`, two items with one disabled) and an `inspect-fields`
> vitest over the real `schemars` output — `items` degrades to the raw-JSON widget as the Risks
> section predicted, and `background` is **not** colour-swatched because the swatch heuristic keys on
> a `colou?r` label (the three `*_color` fields are); an editor heuristic, never a field rename (a
> rename moves every hash). Second game: breakout's pause menu — one `UiList` entity authored with
> no items, so the boot frame and the committed golden stand byte-identical; **paused is derived
> from `items` being non-empty**, never a flag; `menu_input` first in the schedule, `Escape`/`Start`
> toggles, `UiNav` walks, a left click activates through `list_hit` against a `VIEWPORT` constant
> `(1280, 720)` (a system has no live render extent — the reference game's convention); `RESUME` clears,
> `RESTART` clears and requests a `SceneSwitch` to `main.yaml` with an empty carry (the first
> non-reference-game transition consumer). `games/breakout/tests/menu.rs` drives keyboard, gamepad and mouse,
> proves the freeze total (two paused ticks hash identical) and the restart honest (bricks broken
> first, `last_transition_error` None). Cost: the entity's bytes are hashed, so breakout's `run.lrp`
> re-recorded (38,379 → 39,107 B) through the replay-scoped bless; the golden untouched and green;
> `ludion-protocol`'s ceiling green. No engine component moved, so the reference game's replays and goldens are
> untouched by construction. Owed a human eye: the controller-only menu sitting (the plan's
> gate), and whether `background` deserves a swatch.
>
> Wave 2 log (SHIPPED 2026-09-02): `UiText.wrap_width` per decision 8 — `wrap_ui_text` in `ui.rs`
> beside `measure_ui_text` (re-exported through `ludion::math`), `extract` wrapping once and handing
> the same string to measurement and the instance; schemars `min 0`, so a negative is the ordinary
> `L1014`, while a non-finite width or size is a no-op, never a diagnostic. `UiList.max_visible` —
> `ui_list_window` derives the drawn window from `selected` every frame (centred, clamped at both
> ends; no scroll offset exists to desync); `ui_list_lines` now yields `(Option<usize>, &str)` so
> extract colours off the item index; the block's height counts the window and its width every item,
> so the panel never resizes under a scrolling cursor. Calls made where the plan was silent, owed a
> human eye: with wrapping on, runs of spaces collapse to one and leading/trailing spaces drop; a
> hard-broken word's tail keeps filling; **no scroll cue** — nothing marks hidden items (a list
> wanting one draws a `UiText` off `ui_list_window`; a sitting judging that wrong is the trigger).
> Gallery: `ui-wrap-window.yaml` — a 14-column paragraph with a hard break and a paragraph break
> beside a 12-item list at `max_visible: 5`, `selected: 7`, one disabled item inside the window and
> the longest item outside it; the Windows golden was laptop Intel/Dx12 bytes until its provenance
> re-bless on the RTX 3090 (2026-09-07; macOS first-bless queued in the private macOS queue). The cost the plan
> did not name: `WorldHash` covers component bytes, so a serde-defaulted field on `UiText` moved
> every scene's hash in the reference game — two of its segment replays and its run replay
> re-recorded after the session test proved the route live (no L1126); its stress replay, one
> other segment and breakout's replay byte-identical (no HUD text in those scenes). Every
> golden byte-identical by construction (the `wrap_width: 0.0` extract test; all seven of the reference
> game's goldens at 1.0000), both tripwires unmoved (a field is not a registry entry), generated scenes unmoved (the
> HUD fragment travels node-for-node; map-drift 17/17). Docs: `docs/ui-list.md` split out of
> `docs/screen-ui.md`, which had reached the 12 KB spoke budget; `docs/components.md` now sits 75 B
> over its own — split owed. Found by the workspace lane: ludion-protocol's
> `committed_replays_stay_under_the_measured_maximum` was already red at HEAD (the committed keyframe
> 229,043 B against `LARGEST_COMMITTED_RECORD` 200 KiB — the reference game's scenes grew past the
> clustered-forward measurement and the laptop never ran that crate); this wave's record is 229,296 B,
> the constant is 240 KiB with its doc comment re-measured, and the reader's ceiling (7.5 MiB) still
> refuses the 8 MiB fuzz reproducer.
>
> Wave 1 log: `UiList`/`UiListItem` registered **21st** (decision 7's "after `GiDynamic`" was stale
> by landing time — `GroundMarker` had taken the 20th slot; append-last honoured, both tripwires
> re-blessed, the pinned world hash provably unmoved). L1118's message/`explain` text widened to name
> `UiList`. `UiNav`/`navigate` in `ui.rs`, the extract lowering per decision 6 (goldens
> byte-identical by construction), `examples/menu_list.rs`, `docs/screen-ui.md` + `components.md`.
> Reference-game consumer proof: its two choice overlays are one `UiList` entity each, and overlays hide
> by holding no items; the HUD driver paints (change-guarded), the phase system owns the cursor.
> Decision 9 shipped as recommended (wrap + skip disabled); decision 10 shipped as recommended —
> **the numeric hotkeys are deleted** and the `was_pressed`/`is_held` split died with them. Calls made where the plan was silent, owed a human
> eye: `selected_color` `[1.0, 0.9, 0.4, 1.0]`, `disabled_color` `[0.4, 0.4, 0.4, 1.0]`,
> `background` default alpha 0; `disabled_color` outranks `selected_color`; the panel inflates
> outward by `padding` (text never moves); an empty list measures `[0,0]`. The run replay re-recorded
> (1,278,551 → 1,449,555 B; the session's walk is now chunked with a `Space` retrigger replacing the
> held digit key); the stress replay and a district golden byte-identical — no bless. Reference-box
> run 2026-09-07: the check.mjs test lane and E6 both green on the RTX 3090 (E6 over three runs, the
> one moved ceiling — `fire` — unrelated to this wave).
>
> This plan fires — or refuses — the trigger recorded by `Plan/phase-14-ui.md` ("Widget toolkit /
> layout engine / focus management — trigger: recorded friction composing by hand (expected: the reference
> game's shop)"). Verdict: **the friction is real and measured; the answer is one component, not a
> toolkit.** Consumers: the reference game's purchase, hub and recap screens (each owned by a private
> plan) and, through `Plan/sector-state-persistence.md`, its at-gate preview.

## Why

The reference game is menu-heavy: purchase lists, a hub that spends banked currency, a pre-run
loadout pick, state shown **before** entering a level, and prose screens. It already carries two
proto-versions of the shape — a pick-1-of-3 and a shop, both hand-composed from `UiRect` + `UiText`,
the case an earlier plan said would decide the question. It did: the hand-composed form has hit a functional wall
(no menu in the game is operable with a gamepad) and an authoring wall (each
screen costs two entities, two enum variants, a hand-tuned panel size and a `format!` blob).

## What exists today

**The engine's UI surface is two components and three pure functions.**

- `UiRect` (anchor/offset/size/texture/color/order) and `UiText` (anchor/offset/text/size/color/
  order) in `crates/ludion-engine/src/components.rs`, registered *validated*; the registry's last
  entry today is `GiDynamic`. One shared semantic code, **L1118** — "UI element size is not positive
  and finite".
- `crates/ludion-engine/src/ui.rs`: `resolve_ui_rect`, `measure_ui_text`, `resolve_ui_text` — pure
  arithmetic on a 1080-unit canvas, origin top-left, +Y down. `resolve_ui_rect` is `ludion::math`.
- `extract()` (`extract.rs:266–315`) resolves UI components to pixel geometry, pushes
  `(order, EntityId, UiInstance)` and sorts with **`sort_by_key`** — a *stable* sort, so an entity
  emitting several instances keeps its emission order at one key (an entity carrying both `UiRect`
  and `UiText` already does). `UiInstance` (`ludion-renderer/src/scene.rs:374`) has exactly two
  variants, `Rect` and `Text`; the overlay pass and its two shaders consume only those.
- `InputState` (`crates/ludion-sim/src/input.rs`) already carries what a focus model needs:
  `was_pressed`/`is_held`/`was_released` over `Key` (arrows, `Enter`, `Space`, `Escape`,
  `Digit0..9`) and `GamepadButton` (`South`/`East`/`West`/`North`, four d-pad directions,
  `Start`/`Select`).
- No engine system is ever installed into a game's schedule: `EngineBuilder::system()`
  (`builder.rs:145`) appends game systems only, in registration order.

**The measured friction in the reference game's HUD** (a 680-line HUD module and a 317-line scene
fragment for 18 entities):

1. **Every screen costs a fixed tax.** Each of the four overlays is
   a `UiRect` panel + a `UiText` block = 2 entities, 2 role-enum variants (the enum is at 15), an
   alpha toggle (panels hide by writing `color[3] = 0.0`, since no component has a visibility field) and
   a `format!` builder in the HUD driver. Further screens are planned; the idiom projects to ~28 roles
   and a driver that is a screen registry.
2. **A menu is one string.** The shop panel concatenates one line per ware into a single `UiText`.
   So: an unaffordable item looks identical to an affordable one (affordability is checked invisibly in
   the input system); there is no selection to highlight; panel sizes are authored constants unrelated
   to how many wares the doc lists; overflow is silent.
3. **No menu is operable with a gamepad.** The shop and upgrade systems read `Key::Digit1/2/3`, and the
   phase input reads `Escape`/`R`/`Enter`. The gamepad drives movement, aim and fire only. In a
   twin-stick game, a controller player can play the run and cannot buy anything.
4. **The same choice is spelled two ways.** The shop uses `was_pressed`, the upgrade pick uses
   `is_held`, for the identical "pick 1 of 3" — the exact defect the "before adding a second way,
   remove the first" rule exists to prevent, and it appeared because there is no shared selection
   surface to be regular about.
5. **Numeric keys cap a menu at nine items with no scrolling.** The planned rosters and loadouts
   both grow run over run.

Non-consumers, stated so they are never used as justification: `games/breakout` authors no UI at
all, and **the Tauri editor is not a consumer** — an out-of-process React client drawing its own DOM
(`editor/app/src/…`), meeting these components only through the generic inspector fed by
`engine.describe`.

## Decisions

**1. Fire the trigger; the answer is one component — `UiList` — not a widget toolkit.** Every demand
above is a vertical stack of text choices with one cursor. A retained-mode widget tree or a
flex/grid layout engine would be a second, richer way to say what `UiRect` + `UiText` + anchors
already say for static content, and would hide a subsystem behind the editor's raw-JSON fallback.
*Rejected:* (a) a widget tree — nothing needs nesting deeper than "panel with lines"; (b) per-item
entities carrying `UiFocusable {group, index}` plus a `UiFocus` cursor — more general (per-item
icons), but it multiplies entities per menu, adds a group-id namespace the engine must police, and
still leaves each item's anchor arithmetic hand-written, fixing neither friction 1 nor 2; (c) do
nothing — refused, friction 3 is a functional defect.

```rust
/// One choice in a `UiList`.
pub struct UiListItem {
    text: String,        // default ""
    enabled: bool,       // default true — a disabled item is dimmed and skipped by the cursor
}

/// A vertical stack of selectable text items with one cursor: the engine's one menu surface.
pub struct UiList {
    anchor: Anchor,            // default TopLeft — the block's canvas pin and its own pivot
    offset: [f32; 2],
    title: String,             // default "" — drawn above the items, never selectable
    items: Vec<UiListItem>,    // default empty — an empty list draws nothing at all
    selected: u32,             // the cursor; clamped to the item count when drawn
    size: f32,                 // glyph cell height in canvas units; default 24.0 (L1118 applies)
    line_spacing: f32,         // extra canvas units between lines; default 0.0 (schemars min 0)
    padding: f32,              // background inset in canvas units; default 16.0 (schemars min 0)
    color: [f32; 4],           // normal item text
    selected_color: [f32; 4],  // the cursor's item
    disabled_color: [f32; 4],  // an `enabled: false` item
    background: [f32; 4],      // RGBA; alpha 0 = no panel drawn
    order: i32,                // the existing draw-order key
}
```

**2. Layout stays anchor + offset; the list's own stack is the only layout the engine does.**
`UiList` measures its block (title + items × (`size` + `line_spacing`); width = longest line ×
`size`, reusing `measure_ui_text`'s glyph model) and pins it by the existing nine-point `Anchor`.
*Rejected:* a `UiStack`/`UiPanel` container — every planned screen is one stack, and a container holding
exactly one child kind is the second way to say `UiList`.

**3. The list owns its background; there is no visibility flag.** `background` alpha + `padding`
give an auto-sized panel, deleting the panel/text entity pair, the hand-tuned size constants and the
`set_hud_panel_alpha` idiom at once. Visibility follows the existing regular rule: **an empty
`items` vec draws nothing**, exactly as `UiText`'s empty string does. *Rejected:* a `visible: bool`
across the three UI components — a second mechanism beside the alpha convention.

**4. Navigation is a pure function the game calls; activation is a return value, not state.**
`UiNav` is a `Copy` struct of three edge flags (`prev`, `next`, `activate`) built by
`UiNav::from_input(&InputState)` with fixed default bindings — `Up`/`DpadUp`, `Down`/`DpadDown`,
`Enter`/`Space`/`GamepadButton::South`; a game wanting other keys constructs it itself (fields
public). `UiList::navigate(&mut self, nav: UiNav) -> Option<u32>` moves `selected` and returns
`Some(index)` on the activate edge over an enabled item. The two-step shape is forced by the borrow
discipline the reference game already uses: read `InputState`, drop the borrow, query `&mut UiList`. The **cursor
is a component field** (it must survive frames, reach `WorldHash`, ride a snapshot and a scene
switch, and show in a golden); the **activation is a return value** (one call long, so a field would
need a clearing protocol someone will forget). *Rejected:* an `activated: Option<u32>` field
(stale-state bug farm that also hashes); an engine-registered navigation system (the first
engine-owned system in a game's schedule, whose ordering against the game's phase systems is exactly
the implicit rule this repo refuses); callbacks (no scripting layer exists). `from_input` reads
**discrete edges only** — an analog stick would need a repeat latch, i.e. hidden state.

**5. The game's phase state is the focus model.** Exactly one list is navigated per frame: the one
the game calls `navigate` on. No engine focus manager, no group ids, no tab order — the reference game's
run-phase state already decides which overlay is up, and every planned screen is modal.

**6. `UiList` lowers in `extract`; renderer, shaders and overlay pass do not change.** One `UiList`
emits a `UiInstance::Rect` (background, when alpha > 0) then one `UiInstance::Text` per rendered
line at its `(order, EntityId)` key — the stable `sort_by_key` preserves that emission order. No new
shader, no new pass, no new `UiInstance` variant, and **every existing golden stays byte-identical
by construction** (no existing scene carries a `UiList`).

**7. Additive-only, and zero new L-codes.** `UiList` appends **last** in `builtin_registry()` (after
`GiDynamic`), keeping every existing world hash byte-identical — the append-last rule Phase 14
proved; both pinned tripwires (registry order, world hash) re-bless in the same commit. `size` rides
the existing **L1118** ("one semantic, one code" — the rule that made `UiRect`/`UiText` share it);
`line_spacing`/`padding` carry schemars `minimum: 0`, so a bad value is the ordinary `L1014` at the
field (the `GiVolume` precedent). `selected` past the end is **clamped when drawn, not a
diagnostic** — it is a runtime field, and a game shrinking a list mid-frame must not be able to
author an error. L1105–L1109 stay free. Components cross the wire as opaque `serde_json::Value`, so
the surface stays **29 methods** and `schema_hash` does not move.

**8. Word wrap is a field on `UiText`, not a new component — and it is Wave 2.**
`UiText.wrap_width: f32`, default `0.0` = no wrap (every authored scene keeps identical geometry),
greedy break on spaces with a hard break for an over-long word, implemented in `ui.rs` beside
`measure_ui_text` so measure and draw cannot disagree. Consumers are the reference game's recaps and flavor
text — prose, not a list — and its design defers that layer to production.

**9. [USER CALL] Navigation feel: wrap at the ends, skip disabled items.** Recommendation: `next` on
the last item goes to the first, and the cursor never lands on an `enabled: false` item (an
all-disabled list keeps its cursor and refuses activation). Behaviour, not knobs. The alternative
(clamp at the ends; the cursor lands on disabled items but activation refuses) is equally
defensible — a taste call about how an unaffordable purchase should feel.

**10. [USER CALL] Do the reference game's numeric hotkeys survive the conversion?** "Before adding a
second way, remove the first" says delete `Digit1/2/3` from its shop and upgrade systems once the cursor
exists — one idiom, one code path, and the `was_pressed`/`is_held` inconsistency dies with it.
Recommendation: **delete them.** Counter-argument worth hearing: in a shop opened under pressure, a
number key beats three d-pad taps.

**11. [USER CALL] Pointer support — Wave 3 or deferred?** A mouse can already hit-test a list by
hand via `resolve_ui_rect`, and keyboard/gamepad is the half an agent verifies headlessly, so it
goes last either way. Recommendation: ship it in Wave 3 as one pure helper
(`ui::list_hit(list, viewport, cursor) -> Option<u32>`, ~30 lines) rather than defer — the human
sittings are mouse-and-keyboard.

## Waves

### Wave 1 — `UiList`, navigation, and the reference game's two choice screens

The reference game's prototype needs one purchase list and one confirm list — exactly this wave.

- Engine: `UiListItem`/`UiList` in `components.rs` (serde default on every field), appended last via
  `register_validated`, L1118 on `size`; block measurement + `navigate` + `UiNav::from_input` in
  `ui.rs`; the `extract` lowering per decision 6; `ludion::prelude` re-exports.
- Engine: `crates/ludion/examples/menu_list.rs` — a three-item menu driven by scripted input, one
  item disabled, printing the activated index. The "every public API ships a runnable example" rule
  *and* the first consumer outside the reference game.
- Docs: `docs/screen-ui.md` gains the list section (authoring YAML, the cursor/activation contract,
  the one-list-per-frame focus rule) via the `ludion-docs` skill; `--fix` re-mirrors
  `embedded-docs/`.
- Consumer proof (the reference game): rebuild its shop and level-up overlays on `UiList`, deleting
  the shop text builder, the level-up `format!` block, four role variants, their four scene
  entities and both hand-tuned panel sizes. Each overlay becomes one authored entity.
- **Exit (mechanized):** serde round-trip + `{}`-deserialization (drift-safety); L1118 semantic tests
  with pointer paths; measurement tests (title, empty list, longest line, `line_spacing`); extract
  tests (background-then-items order preserved at one key, three colors applied, empty list emits
  nothing); a headless driver test scripting d-pad Down×2 + `South` asserting the returned index,
  wrap at the ends, the skipped disabled item, and a byte-identical `WorldHash` across two runs;
  **every existing golden byte-identical**; both tripwires re-blessed; wire surface still 29 and
  `schema_hash` unmoved; `node scripts/check.mjs` green. The reference game re-generates its district scenes
  (its HUD is a shared map fragment — the map-drift gate applies) and re-records its run replay (the
  entity-count change moves the hash trajectory); the E6 `budget-horde` gate stays green with no
  ceiling moved.

### Wave 2 — prose and long lists

- `UiText.wrap_width` per decision 8; `UiList.max_visible: u32` (default 0 = unlimited), the visible
  window derived from `selected` — pure arithmetic, no scroll state, so it cannot desync.
- **Exit (mechanized):** wrap unit tests (space break, over-long word hard break, `0.0` ⇒ geometry
  identical to today, proven by every existing golden staying byte-identical); window tests (cursor
  at both ends, list shorter than the window, window exactly the list length); one gallery scene +
  per-OS golden with a wrapped paragraph and a scrolled list.

### Wave 3 — pointer, editor, and the second game

- `ui::list_hit` (decision 11), re-exported through `ludion::math` beside `resolve_ui_rect`.
- Editor: one palette entry for `UiList` (`editor/app/src/editor/palette.ts`, hand-maintained) plus
  a vitest pinning what the inspector derives from the real schema.
- `games/breakout` gains a title/pause menu on `UiList` — the non-roguelike proof that
  the component is genre-generic.
- **Exit (mechanized):** hit-test unit tests across anchors and viewports; vitest + oxlint + tsc
  green; breakout's menu driven by a headless scripted-input test; `node scripts/check.mjs` green.
- **Human gate (flagged, one sitting, rides the next gate batch):** with the controller only — open
  a purchase menu, tell which items are unaffordable *without being told*, buy one, leave; then the
  same on the hub screens. The question is whether "unaffordable" and "where is my cursor" read at a glance.

## Engine/game boundary

**Engine (`crates/`):** the `UiList`/`UiListItem` components, measurement and navigation in
`ludion-engine/src/ui.rs`, the `extract` lowering, facade re-exports, the docs topic,
`UiText.wrap_width`, `list_hit`. All content-free: the engine knows a stack of strings with a cursor
and nothing of currencies, wares or loadouts.

**Game (the reference game):** every screen's content and semantics — which doc supplies the items, what
an item costs, what an activation does, which phase owns the cursor. A ware's affordability is game
logic that sets `enabled`; the engine only dims and skips. Private plans own those screens.

**Second consumer, concretely:** a selectable list is the least genre-specific UI object there is —
title screen, settings, save-slot picker, dialogue choice, inventory, shop. This plan names two
consumers outside the reference game that ship as gates: `crates/ludion/examples/menu_list.rs` (Wave 1) and
`games/breakout`'s title/pause menu (Wave 3). **The Tauri editor is explicitly not a consumer** — it
renders its own DOM and meets `UiList` only as one more component in the generic inspector. Nothing
here may be justified by an editor need, and no editor-private surface appears in it.

## Deferred / non-goals (triggers recorded)

- **Widget tree / layout engine (flex, grid, nesting)** — trigger: a screen provably not one vertical
  stack *and* not expressible as several anchored lists (an inventory grid; a loadout pick if it
  must read as three side-by-side cards).
- **Per-item rich content (icons, portraits, sub-rects)** — trigger: content whose meaning is lost as
  text. Until then: a separate anchored `UiRect` driven from `selected`.
- **Engine-side focus across several live lists** (tab order, focus stack) — trigger: one screen with
  two simultaneously navigable lists.
- **Action mapping / rebindable menu keys** — unchanged non-goal (`UiNav`'s fields are public);
  trigger unchanged: a second game wanting remapping.
- **Analog-stick navigation** (needs a repeat latch, i.e. hidden state) — trigger: a sitting judging
  d-pad-only navigation wrong.
- **Nine-slice / theming / UI animation** — `UiRect.texture` already stretches; trigger: authored
  frame art that visibly breaks when stretched, or a screen whose reveal must animate.
- **Clipping / scissor rects** — Wave 2's window is index-based; trigger: content that must be cut
  mid-glyph.
- **Proportional or larger fonts** — the 8×8 uppercase atlas stays the one text surface engine-wide;
  trigger unchanged.
- **Input routing / a UI event queue** — the game reads a return value; trigger: three or more
  distinct widget kinds needing dispatch, which this plan is designed never to reach.

## Risks / open questions

- **The editor inspector will show `items` as a raw-JSON textarea.** Verified: `inspect-fields.ts`
  derives widgets only for scalars, bools, strings and *fixed-length numeric* arrays; a
  variable-length array of objects falls back. `Tilemap.rows` has the same shape and has been
  acceptable. A human authoring a menu in the editor edits JSON; the honest fix is an editor
  feature, out of scope.
- **A growing component in the hash.** `items` is variable-length data inside a hashed component, so
  a menu rebuilt every frame with fresh `String`s would churn the world hash. Mitigation is the
  idiom the reference game proved with its HUD text setter: write only on change. Wave 1's two-run `WorldHash` test
  enforces it; the docs topic must state it as the rule.
- **The reference game's committed replay and goldens move.** Deleting four HUD entities shifts
  `EntityId`s and the hash trajectory: the run replay re-records and a district golden re-blesses, both reviewed before
  commit. Sequence Wave 1 clear of another arc re-recording the same corpus.
- **Scope creep is the real risk, not scope.** Every deferral above is something a "UI system" is
  normally expected to have. The defence is that each has a written trigger and Wave 1 ships one
  component; a wave adding a second widget kind has made a flagged decision, not a drive-by.
- **Open: does the at-gate sector preview need anything at all?** As described it is a
  read-only info panel — a `UiText` block, buildable today. It becomes a `UiList` only if the gate
  offers a choice between destinations, which depends on a private plan. This plan
  assumes it does not and blocks neither answer.
- **Open: how much prose does the game carry?** Decision 8's wrap is sized for recaps and flavor
  lines. If the narration grows into multi-screen dialogue (pagination, speaker names, timing),
  that is its own plan, not more fields on `UiText`.
