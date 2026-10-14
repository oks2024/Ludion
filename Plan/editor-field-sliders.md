# Plan: Sliders for bounded numeric fields — one shared widget-layer capability

> Status: **SHIPPED 2026-07-26** (Lore 396–399, all four committed, full `node scripts/check.mjs`
> green — all 16 gates). Runs under the **editor** feature stream
> (`Plan/editor-views.md`) and directly serves `Plan/editor-art-director.md`'s exit question —
> *"could a human reach a look they liked without typing a number into a file"* — which the Art
> Director tab could not answer while every value it exposed was a text box.
>
> The wire surface stays at **26 methods**. Every write is `component.set`, at two cadences.

## Status

**2026-07-26 — the arc is code-complete and mechanically verified; nothing in it has been exercised
in the GUI.**

- **396 (the range becomes data)** — `x-ui-range` as a schemars field extension, 30 annotations, the
  coexistence test, the `docs/ludion-ecs.md` section, and the TypeScript half that carries the range
  from schema to widget.
- **397 (the widget)** — `RangeInput`: a track plus a readout that *is* the input, and its
  stylesheet block.
- **398 (one commit path)** — the hand-duplicated pending-assembly base extracted into
  `useComponentCommit.ts` over the pure `pendingBase.ts`. Net **−225 lines**.
- **399 (the live drag)** — `SliderDrag`, the preview cadence on `useComponentCommit.commit`, and
  the burst that keeps one gesture to one undo entry spanning the whole drag.

- **401 (the first sitting's verdicts)** — the user drove the sliders in the real Tauri app and
  returned three: `dof` travels to `0.15` rather than its legal `0.25`; colour triples give up their
  tracks entirely, waiting for the picker that belongs there; and a ranged vector stacks one
  full-width track per channel instead of cramming N tracks into one row. All three are in.

The frontend gates cannot render React, so `Plan/editor-live-smoke.md` carries what
that sitting did **not** reach — the drag's live preview and its one-undo-entry claim, the
readout↔input swap, and the same widgets in the Level Designer tab.

**What the first sitting settled about the metadata itself.** Two rules came out of it that the
original design did not state:

- **Travel narrower than legal is a category, not a redundancy.** `dof` is the first field where
  "what the validator accepts" and "what you will plausibly want" have different answers, and both
  are facts worth keeping. Dropping either loses one.
- **A colour triple declares no travel, and that absence is the whole mechanism.** No client
  special-cases colours; they simply have nothing to draw a track from. The alternative — a
  `field.color` branch in the widget — would have been a second rule spelling the same thing, and
  the two would drift the moment a colour picker arrives.

## Goal

A bounded numeric field is a slider you drag while watching the viewport update live; an unbounded
one stays exactly the text box it was. One implementation, in the one shared widget layer both the
Level Designer's inspector and the Art Director's chain already render through
(`editor/app/src/viewer/FieldWidgets.tsx`), so no surface gets a private answer.

The metadata was already on the wire before this arc started. `Registry::register_with` stores each
component's whole schema and `engine.describe` ships it verbatim; the **only** place the bounds were
dropped was `deriveField` in `editor/app/src/viewer/inspect-fields.ts`, which read
`type`/`minItems`/`maxItems` and never looked at `minimum`/`maximum` — three tests said so out loud.
So the work was: stop dropping the bounds, declare the ones that existed only as doc-comment prose,
and grow one widget.

## Two ranges, not one

| | keyword | meaning | who enforces it |
|---|---|---|---|
| **legal** | `minimum` / `maximum` | outside this the scene is invalid | `crates/ludion-scene/src/validate.rs` → `L1014` |
| **useful** | `x-ui-range: [lo, hi]` | how far the slider handle travels | nobody — it is inert to every validator |

`x-ui-range` is an unknown JSON Schema keyword, so it is ignored by validation **by construction**,
not by an opt-out anyone maintains. It is not a UI hint smuggled into the engine: it makes
machine-readable what each field's doc comment already stated in prose (*"the useful UI top is
~0.5"*, *"0.01 is a haze and 0.1 is a soup"*), so an agent tuning from the CLI inherits it too.

Three rules bind the two together, and each carries a test:

- **Travel** = `x-ui-range` if present, else `[minimum, maximum]` if **both** bounds exist, else
  `null` — a plain number box, today's behaviour unchanged.
- **Widen, never clip** (`widen`). Travel is widened to contain the live value, so opening a slider
  on an out-of-range authored number can never silently rewrite it. This is `materialRange`'s rule
  from the voxel palette, generalised: *a slider must never silently rewrite the number it was
  opened on*. Widening travel does not make a value legal.
- **Clamp to legal** (`clampToLegal`). A **dragged** value is clamped to `minimum`/`maximum`, so a
  gesture can never author an `L1014`. A **typed** value is sent exactly as written, so the engine's
  own diagnostic speaks — the two input paths are deliberately not symmetric.

`dragValue` quantizes **before** it clamps, never after: rounding a value already sitting on a legal
bound can push it back off (`0.9999` → `1` past a `maximum` of `0.9999`), which is the one thing
that path exists to prevent.

## Decisions

1. **Display ranges are declared in Rust, beside the field.** A schemars field extension emitting
   `x-ui-range` on the field's own subschema. Rejected: a TypeScript table, which would be a second,
   ungated home for "how big is this number" — and the editor is a detached cargo workspace, so
   nothing could gate it against the component.

2. **Verified before anything was written on top of it.** `schemars_derive` 1.2.1 emits field-level
   `extend` as `schema.insert(key, json!(value))`, a post-mutator on the field's own subschema, so
   it coexists with `minimum`/`maximum`. `x_ui_range_coexists_with_the_validated_range` in
   `crates/ludion-engine/src/components.rs` pins both halves: that the extension does not disturb
   `minimum`, and that on a fixed numeric array the keyword lands on the **array node, not on
   `items`**. The second is schemars' choice, not ours, and the vector sliders depend on it.

3. **The numbers come from the field's own doc comment**, not from taste applied at annotation time.
   Where the prose stated only a default and a unit, the number was derived and is flagged as such
   (finding 2) rather than presented as transcribed.

4. **Fixed numeric arrays get `x-ui-range` read per element, and never
   `#[schemars(inner(range(…)))]`.** The latter emits real `items.minimum`/`maximum` and would turn
   the existing element-wise semantic diagnostics (`L1130` on `PostProcess`, `L1128` on
   `Environment`) into competing `L1014`s — two diagnostics for one mistake.

5. **Control shape: a track plus a readout that *is* the input.** Click the readout and it becomes
   the same `DraftInput` every other number box uses, so the typed path reuses the draft-and-commit
   contract instead of growing a second one. Not a second box permanently beside the track.

6. **Travel is widened off the *committed* value, not the live one**, so the track cannot shrink
   under the pointer mid-gesture.

7. **Drag is live, gizmo-style, and collapses to one undo entry.** `SliderDrag` reuses
   `PREVIEW_INTERVAL_MS = 66` and `shouldSendUpdate` from `editor/gizmo/drag.ts` rather than
   inventing a second cadence.

8. **One quantization rule in the repo.** `quantize` moved from `editor/voxel/palette.ts` into
   `slider.ts`; `palette.ts` imports it. Three **significant** digits, not three decimals — measured
   on the palette, where decimal rounding breaks the sRGB picker round-trip for 19 of 256 8-bit
   levels. A slider needs the same property: a knob at `0.002` must not quantize to `0`.
   (CLAUDE.md: *before adding a second way to do something, remove the first.*)

9. **The pending-assembly base gets extracted before the preview path is written, not after.** Both
   callers already carried a hand-duplicated copy; writing the preview cadence twice into two copies
   of the same code is how they drift.

10. **A held comparison owns the component outright.** Not ordered against a slider preview —
    refused. See finding 4 and the wave 399 note below.

11. **Genuinely unbounded fields get no annotation, and that is the correct answer, not a gap.**
    `Transform.translation`, `fog_height` (a signed world Y), `PointLight::range` (finding 1).

## Per-wave outcomes

### 396 — the range becomes data

**Engine.** 30 `x-ui-range` annotations: `Mesh::emissive`, `PointLight::intensity`, seven on
`Environment` (`sky_zenith`/`sky_horizon`/`sky_ground`/`ambient` as per-element array ranges,
`sun_disc_size`, `sun_disc_intensity`, `ibl_intensity`), twenty on `PostProcess`, and
`Projection::Perspective::fov_y_deg`. Fields whose `minimum`/`maximum` pair is already an honest
track — `temperature`, `tint`, `vignette`, `vignette_radius`, `ssao`, `dof`,
`chromatic_aberration`, `grain`, `Mesh::roughness`/`metallic`, `Fade::fade` — were left alone.
`grain_size` is the one deliberate **narrowing** of a hard `[0, 1]`: `0.002` ships and `1.0` is
absurd.

Safety properties confirmed rather than assumed: `schema_hash` hashes the **protocol** schema only,
so it did not move; `protocol_schema.snapshot.json` and the `schema-drift` gate cover the eight
protocol sections and `EngineDescription.description` is a `serde_json::Value`, so both were
untouched; `WorldHash` hashes component **values**, so there was no `run.lrp` re-record and no
golden re-bless.

Documented in `docs/ludion-ecs.md` under *"Two ranges on a numeric field"*, with the
`crates/ludion-cli/embedded-docs/` mirror re-generated by `--fix`.

**Frontend.** `FieldRange` on `NumberField`/`VectorField` in
`editor/app/src/viewer/inspect-fields.ts` — travel from `x-ui-range`, else the legal pair when
**both** bounds exist, else `null`. A malformed `x-ui-range` (wrong length, non-finite, `lo >= hi`)
yields `null`, i.e. today's number box: never a crash, never a broken slider. The three tests that
asserted the range keywords were ignored now assert the opposite.

New pure module `editor/app/src/viewer/slider.ts`: `widen`, `niceStep` (a 1/2/5×10ⁿ step over ~100
stops, so the handle lands on numbers that read like authored ones), `quantize`, `clampToLegal`.

### 397 — the widget

`RangeInput` in `editor/app/src/viewer/FieldWidgets.tsx`, one branch off the existing `number` case
and the same per-component for `vector`. `.field-slider` / `.field-range` / `.field-readout` in
`index.css`. The readout is fixed-width (`6em`) and right-aligned deliberately: a number changing
width under the pointer would resize the track it is being dragged on.

A shipped-default slider recedes via `accent-color`, not via `.ad-field-default`'s dashed border —
a native range input has no border for that treatment to act on. Native styling throughout, like
`.vx-range`; there are no custom thumb styles anywhere in the file and none were added.

The Inspector's existing no-op skips survived untouched: a number reparsed to the same value, or
JSON that only reformats, must send nothing, or a redundant `component.set` becomes an undo entry
for an edit that did not happen.

### 398 — one commit path

`editor/app/src/viewer/useComponentCommit.ts` (the hook) over the pure `pendingBase.ts`
(`pollHandback`, `shouldRollBack`, `STALE_POLL_LIMIT = 2`). Both `Inspector.tsx` and
`ArtDirectorView.tsx` adopt it. **Net −225 lines.**

**Finding: the two copies had no behavioural divergence** — identical statement for statement. The
only real differences were the Art Director's three genuine extras, which became the `initialBase`
and `onApplied` parameters rather than a fork. `MutationReport` moved to the hook and its four
importers were repointed, so there is one import path for it.

### 399 — the live drag

`SliderDrag` in `slider.ts` — pure and tested, so the gesture's policy is testable without a DOM.
`useComponentCommit.commit` grew a third argument `cadence?: 'preview' | 'commit'`, defaulting to
`'commit'` so every existing call site is untouched. A `'preview'` write does everything a commit
does **except** call `onReport`, which is the only thing `App.handleMutation` turns into an undo
entry.

**The non-obvious part.** An undo entry's `before` is the response body's `previous`, which for a
drag's final write is the *last preview* — so Ctrl+Z would land mid-drag. The hook therefore keeps a
`PreviewBurst` whose first reply's `previous` is substituted into the one report the gesture makes.
The origin comes from the engine's own answer, never recomputed. Which write opens the burst is
decided at issue time, not on reply: replies can land out of order, the issue order cannot.

A gesture that ends back on its start still commits — the engine really is holding previews, and
that write is the restore — and still marks the scene dirty, exactly as the gizmo does. Undoing that
entry is correctly a no-op; leaving the scene silently clean would not be. A gesture that never
previewed and never moved writes nothing.

**Slider vs hold-to-compare.** `ArtDirectorView.commitField` drops field writes at **either**
cadence while `compareRef.current !== null` — refusing the later writer rather than ordering them.
A preview landing between the compare's capture and its restore would be silently restored away,
and `compare.ts`'s whole invariant is that release restores the **captured** object. `resetBase`
deliberately does not drop an open preview burst: its origin is what the gesture's one undo entry
steps back to, and a net-zero writer leaves that value exactly where it was.

## Grade control (shipped 2026-07-26, Lore 403-404 — not part of the wave plan above)

The grade presents Resolve-style `master`/`temp`/`tint` per band over `PostProcess`'s stored
`[f32; 3]` triples, with **no engine change**. `lift`'s band uses an additive (CDL-offset) algebra
whose basis is the derivative of the multiplicative chroma algebra at neutral, so one `temp` value
reads as the same warmth in all three bands (lift/gamma/gain) despite lift being additive and the
other two multiplicative.

## Findings

1. **`PointLight::range` got no annotation, deliberately.** Its doc states only a default; shipped
   scenes author 8, 11, 12, 14, 25 **and 170** (the district fill lights). No single track serves
   both ends — a travel of 200 makes a 12-unit lamp 6% of the slider, and 50 forces the widen rule
   on every district scene. It needs a taste call or a doc sentence first.

2. **Four `x-ui-range` values were derived rather than transcribed**, because their docs state a
   default and a unit but no ceiling: `exposure` `[0, 4]`, `bloom_threshold` `[0, 4]`, `fog_start`
   `[0, 100]`, `focus_distance` `[0, 100]`. One-line edits if the sitting disagrees; this is the
   shortlist to hold under the hand.

3. **`fov_y_deg` travel is `[10, 150]`, not the `0..180` legal interval** — both ends of that
   interval are a degenerate camera. Currently unreachable by any widget anyway: `Projection` is a
   `oneOf` and falls to `deriveFields`' raw-JSON fallback.

4. **PRE-EXISTING HOLE, found and not fixed: hold-to-compare's restore is unconditional against any
   other writer.** `endCompare` writes the captured object back regardless of what happened during
   the hold, so a `PostProcess` write from the terminal persona over MCP, or from another attached
   client, is silently reverted. The slider is excluded only because it lives in the same view and
   can be refused there; nothing protects the general case. Fixing it would mean re-reading before
   restoring, which is exactly the "two code paths agreeing" that `compare.ts`'s own header rejects
   — a real design tension, not a typo.

5. **Cosmetic:** the compare's release bypasses the pending base, so the panel can show the neutral
   value for up to one poll interval (~300 ms) after release.

6. **Two narrow known races; the consequence is one mis-sized undo entry, never corruption.** A
   gesture that ends within one round-trip of its first preview starts its entry one preview late.
   A gesture whose final commit is refused by the compare guard leaves the burst open, so the next
   gesture's entry starts too far back — reaching it requires holding an arrow key on a track while
   mouse-pressing A/B.

## Verification

**Mechanized (green).**

- Full `node scripts/check.mjs` across all four revisions — all 16 gates.
- `x_ui_range_coexists_with_the_validated_range` is the proof that field-level `extend` emits what
  the editor expects, and it landed before any TypeScript was written on top of it.
- The pure tests carry the real invariants: travel widens to contain an out-of-range authored value
  rather than clipping it; a dragged value is clamped to `minimum`/`maximum`; a typed one is not; a
  malformed `x-ui-range` degrades to a number box; one drag is one undo entry; a drag returning to
  its start still sends, because previews went out; Escape restores the captured base.
- `editor/app/src/protocol/parity.test.ts` gates the wire surface for free — **no new method, the
  surface stays at 26.**
- No golden re-blessed, no `run.lrp` re-recorded, `schema-drift` and `map-drift` untouched.

**Live smoke — the whole of the human-visible verification**, since the frontend gates cannot render
React. The list lives in `Plan/editor-live-smoke.md` and joins the queued Tauri sitting; host the plaza with

```
cargo run -p vigil -- --scene games/vigil/scenes/district-plaza.yaml
```

and walk it. Its two load-bearing items are that a `PostProcess` slider moves the frame *during* the
drag and one Ctrl+Z returns it, and that the Level Designer's inspector still edits, reverts on
Escape, and skips no-op numbers — the latter because wave 398 refactored shipped, working code.

## Current-code facts that shape any follow-up

- `crates/ludion-engine/src/components.rs` — the 30 annotations and the coexistence test. Adding one
  is a one-line attribute beside the field; the number belongs in the doc comment first.
- `editor/app/src/viewer/inspect-fields.ts` — `FieldRange`, and `deriveField`'s travel/legal
  resolution. The module header enumerates the exact schemars shapes it handles.
- `editor/app/src/viewer/slider.ts` — the arithmetic and `SliderDrag`. Pure; every rule that decides
  what number a gesture produces lives here, and the widget is left with rendering.
- `editor/app/src/viewer/useComponentCommit.ts` + `pendingBase.ts` — the one commit path and the two
  cadences.
- `editor/app/src/views/ArtDirectorView.tsx` — `commitField`, where the compare guard refuses both
  cadences.
- `docs/ludion-ecs.md` § *Two ranges on a numeric field* — the convention's home, mirrored into
  `crates/ludion-cli/embedded-docs/`.
