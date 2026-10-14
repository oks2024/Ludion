# Plan: Editor Art Director — directing the look by eye (Editor slice #4)

> **WAVE 1 SHIPPED 2026-07-26 (Lore 391–393) — the "under way" below is stale; §Status is the
> record: "wave 1 code-complete (Lore 391–393), unverified in the GUI", with Wave 2 (the cost
> column) and the merged sitting still open.**
> Status: **ACCEPTED 2026-07-26** (direction settled the same day in a grill-me session; this plan
> condenses those decisions and does not re-litigate any of them). **Wave 1 is under way.** Runs
> under the **editor** feature stream (`Plan/editor-views.md`),
> **concurrently** with `Plan/post-process-pipeline.md` wave 12 — the editor is a detached cargo
> workspace on `check.mjs`'s `--frontend-only` lane while wave 12 touches `docs/` and
> `games/vigil/`, so the two do not collide. Two waves plus a **merged human sitting that closes
> both arcs**.
>
> The wire surface stays at **26 methods**: every call this view makes is an existing method
> (`component.get` / `component.set` / `component.remove` / `query.entities` / `engine.describe` /
> `render.dump_graph` / `render.dump_timings` / `render.screenshot`), so the hard parity gate is
> satisfied with no new command. Wave 1 touches **no engine file at all**; wave 2's single engine
> change is deliberately sequenced *after* wave 12's doc sweep, because it moves
> `frame_graph_64x64.json` and `docs/ludion-render-graph.md`.

## Status

**2026-07-26 — plan accepted; wave 1 code-complete (Lore 391–393), unverified in the GUI.**

- **Wave 1 (the tab)** — shipped in three parts: the pure modules + plan + persona + context block
  (391), the view and chain panel (392), the stylesheet (393). The tab is enabled in the switcher.
  Nothing in it has been exercised in the real Tauri app — the frontend gates cannot render React,
  so `Plan/editor-live-smoke.md` is the whole of its verification.
- **Wave 2 (the cost column)** — not started; blocked on `Plan/post-process-pipeline.md` wave 12's
  doc sweep.
- **The merged sitting** — not scheduled; it needs wave 1 in the human's hands.

One thing wave 1 did beyond its brief: `Inspector.tsx`'s field inputs were extracted into
`editor/app/src/viewer/FieldWidgets.tsx` and shared with the chain rows, rather than reimplemented
there. Five near-identical draft-state components collapsed into one, and Inspector shed 400 lines.

Per-wave outcomes, the decisions taken during implementation, and the live-smoke list belong in
the editor's private notebook and `Plan/editor-live-smoke.md` once each wave lands; this section records only what is true of the arc.

## Goal

A human directs the engine's look **by eye, live** — and an agent persists what they chose into the
right source file. This is the view that closes the post-process arc's exit gate with the tool that
arc's own machinery made possible.

`Plan/editor-views.md` (§ *Declared, not built — Lighter and Art Director*) declared this view and
wrote its build trigger:

> build the Art Director view when the post stack (and/or textured-PBR material data) becomes
> editable data … today the post stack is **not** exposed as editable data (it is renderer-internal,
> not a doc/component a form could bind).

`Plan/post-process-pipeline.md` waves 0–11 fired that trigger. `PostProcess` is now a registered,
schemars-validated, opt-in singleton component carrying **30 fields** across the effect roster
(bloom, exposure/tonemap, dither, the seven-field grade, fog, vignette, SSAO, shafts, DoF, film
emulation), and the existing wire methods already reach it.

Today the tab is one row in `editor/app/src/shell/views.ts:19`
(`{ id: 'art-director', label: 'Art Director', enabled: false, note: 'awaiting engine surface' }`)
and nothing else — no persona, no CSS block, no `App.tsx` branch.

Scored against the North Star: this is the human-judgment half of a loop the agent already runs
headlessly well. `ludion render --sweep` and `ludion image-stats` let an agent iterate imaging
alone up to the point where the answer is *"which one looks right"* — and that answer needs a human
looking at the frame. This view makes that judgment cost one drag instead of one round trip.

## The problem the tab has to solve

`Inspector.tsx` is already a **fully generic, schema-driven component editor**: it fetches the
registry's schemas via `engine.describe`, derives a widget per field with `deriveFields(schema,
value)` (`editor/app/src/viewer/inspect-fields.ts`), and writes back with `component.set`. So a
property grid over `PostProcess` ships today in the Level Designer tab. This view must justify
itself by what the Inspector structurally **cannot** do:

| Gap | Why the Inspector can't close it |
|---|---|
| Which effects are **actually running** | `PostChain::for_scene` (`crates/ludion-renderer/src/graph.rs`) is derived, not authored. A zero gate means the pass is absent from the graph; `shafts: 0.02` renders nothing without a `DirectionalLight` (the two-gate rule) |
| **Finding or creating** the singleton | Most scenes — including all of vigil — author no `PostProcess` at all, so there is nothing to select |
| **A/B** | Every look verdict in the post-process arc was reached by comparison |
| **Pipeline order** | The Inspector shows schema order; the arc pinned DoF → bloom mix → fog → shafts → exposure → tonemap → grade → vignette → grain → dither |
| **Cost** | `render.dump_timings` is on the wire but the effect→passes mapping is not |

## Decisions summary

1. **Scheduling.** Own plan under the editor stream, run **concurrently** with post-process wave 12.
   Wave 2's engine change is sequenced *after* wave 12's doc sweep lands.

2. **Edit contract: live set, persist to source.** `component.set` on the live `PostProcess` for the
   feedback loop; persist by a **surgical in-place edit of the source YAML**
   (`games/vigil/scenes/env-night.yaml` for vigil). **Never `scene.save`** — vigil's districts carry
   `generated_from:` and are guarded by the `map-drift` CI gate, and saving would de-share the
   fragment into three copies. **Never parse → re-emit** — it erases authoring comments (the Voxel
   Artist's rule). This is the Level Designer's live loop plus the Voxel Artist's file rule, both
   already existing as persona prose to adapt.

3. **Centerpiece: the chain, not the form.** The panel *is* the pipeline — one row per effect in
   pinned order, each showing whether it is *actually in the graph*, a hold-to-compare toggle, and
   its fields nested underneath (rendered with the existing `deriveFields` widgets, not new ones).

4. **Chain truth from fields, cost later.** Wave 1 derives each row's live/dead state from the
   `PostProcess` values plus real preconditions, cross-checked against `render.dump_graph`'s pass
   list. The cost column waits for wave 2.

5. **Scope: the global-look singletons.** `PostProcess` + `Environment`. The rule — **singletons
   that describe the whole frame belong to the Art Director; lights you place belong to the
   Lighter.** This resolves the couplings the arc documented (`fog_color` must sit near
   `sky_horizon` or the horizon reads as a slab) while leaving the still-unbuilt Lighter a real job:
   `PointLight`s, and the sun's direction and shadow tunables.

6. **Session: shared**, like Level and Game Designer. Post-process is global to the scene and is
   judged against real content, so there is no rig to isolate and the Voxel Artist's private-stage
   precedent does not apply.

7. **A/B is hold-to-compare with restore on release.** Press-and-hold writes the neutral value;
   release writes back the exact prior value from the same captured object. Net-zero by
   construction, so it is reported as **neither a mutation nor an undo entry** and never dirties the
   shared session — which matters because the Level Designer shares it and a `scene.save` from
   another tab would otherwise persist whatever state a comparison was left in. A
   `render.screenshot` reference capture pins beside the live stream for side-by-side.

8. **Absent component: show effective, materialize on first edit** (`component.get` on an absent
   component raises `L3006`, read by the tab as `{}` — never a banner). A scene with no `PostProcess`
   is not unstyled — it renders the shipped chain, and the engine guarantees that an empty
   `PostProcess: {}` is byte-identical to no component at all (`PostData::default` in
   `crates/ludion-renderer/src/scene.rs` is the one place the two agree, field for field). So the
   chain view always shows the **effective** look, with unauthored values dimmed and labelled as the
   shipped default; the first field the human touches materializes the component carrying only that
   field. **Target entity rule:** the entity carrying `Environment` if one exists (the two
   global-look singletons; vigil pairs them in `env-night.yaml`), else the lowest `EntityId` — named
   in the UI. **Never spawn an entity**: the arc's own `night-sky.yaml` took its component on an
   *existing* entity precisely so no `EntityId` moved and no golden or replay shifted.

9. **Persistence is the persona's job; the tab never writes a file.** "Which file?" has no general
   answer — scene or fragment, which of three fragments, `generated_from:` or hand-authored,
   deliberately shared across districts or not. A button would encode `generate-map`'s rules in
   TypeScript as a second, drifting copy. An agent reads `generated_from:`, follows `fragments:`,
   edits in place, regenerates, and verifies with `map-check` / `check-scene`.

10. **Context bridge: `artDirector: { entity, authored, effect } | null`.** The target entity is
    carried **because the placement rule is the UI's** — putting it in the context file gives that
    rule exactly one implementation and lets the agent inherit it, so a "just add some fog" request
    lands where the UI would have put it. Field *values* stay out: one `component.get` returns them,
    and duplication across layers is what the private terminal notebook calls a defect.

11. **No build-time gate for the field→effect table.** `editor/protocol/src/protocol.d.ts` carries
    the command union only, not component schemas, so nothing can compare a TS table against the
    Rust component at build time. Instead the tab fetches `PostProcess`'s schema at runtime via
    `engine.describe` and buckets every field it does not recognise into a visible **"Other"** row —
    a field added by a future wave shows up unassigned rather than silently disappearing.

## Implementation contracts (hold these fixed)

1. **The chain rows are derived, never authored twice.** Whether a row is live is
   `PostChain::for_scene`'s answer, reproduced in TS and cross-checked against
   `render.dump_graph`'s pass list — not a second opinion. Three effects add passes (`ssao` → a
   depth+normal prepass, the estimate and its blur; `shafts` → a march and a blur, **and only with a
   sun**; `dof` → a CoC prefilter and a gather). Everything else folds into the one `composite` pass
   behind a uniform branch, so its "live" state is a value comparison, not a pass lookup.
2. **Hold-to-compare is net-zero by construction.** Release restores the **captured object**, never
   a recomputed one. This is what lets the feature be reported as neither a mutation nor an undo
   entry in a session the Level Designer shares.
3. **The tab never writes a file and never calls `scene.save`.** Persistence is decision 9.
4. **The shipped defaults are not all neutral.** `bloom_strength`, `exposure` and `dither` are the
   engine's shipped imaging chain, derived rather than picked; every *effect gate* is off. The
   dimmed "shipped default" labelling must show the real number, not imply zero.
5. **All testable logic lives in pure `.ts` siblings.** `editor/app/vite.config.ts:9` sets vitest's
   `include` to `src/**/*.test.ts` — **`.ts` only** — so nothing testable may live in a `.tsx`.

## Layout

```
┌──────────────────────────────────────────────────────────────────────────┐
│ Titlebar · ViewSwitcher: Level │ Game │ Voxel Artist │ Art Director       │
├─────────────────────┬──────────────────────────────┬─────────────────────┤
│ THE CHAIN           │                              │                     │
│ ● dof        0.0    │                              │      TERMINAL       │
│ ● bloom      0.1    │        VIEWPORT              │   (art-director     │
│ ● fog        —      │   (optionally split over a   │    persona, shares   │
│ ○ shafts  no sun    │    reference capture)        │    the game session) │
│ ● exposure   1.3    │                              │                     │
│ ● tonemap   agx     │                              │                     │
│ ● grade      —      │                              │                     │
│ ● vignette   —      │                              │                     │
│ ● grain      —      │                              │                     │
│ ● dither    on      │                              │                     │
│ ● ssao       —      │                              │                     │
│ ▸ Other             │                              │                     │
├─────────────────────┴──────────────────────────────┴─────────────────────┤
│ StatusBar · PostProcess on `environment` (#3) · not authored — shipped    │
└──────────────────────────────────────────────────────────────────────────┘
```

A row's fields nest underneath it when opened; the open row is what the context bridge's `effect`
field carries.

## Wave 1 — the tab (pure frontend + one markdown file)

No engine file is touched. Every protocol call is an existing method, satisfying
`editor/app/src/protocol/parity.test.ts` (which asserts every `method: '…'` literal is a member of
the generated `Command` union, and that `new WebSocket(` / `.send(` appear only in
`editor/app/src/protocol/client.ts`).

**New files**

- `editor/app/src/views/ArtDirectorView.tsx` — the view. Follows `GameDesignerViewProps`'
  established shape (client, mode, selection, mutation/status/context callbacks, drawer plumbing).
  Layout: `col col-side` chain panel | `col-center` viewport (optionally over a reference capture
  via the existing `editor/app/src/views/VerticalSplit.tsx`) | `col col-term` `TerminalDrawer`.
- `editor/app/src/editor/art/chain.ts` + `chain.test.ts` — **pure**: the pinned effect order, the
  field→effect table, the neutral value per gating field, and `deriveLiveEffects(values, hasSun)`
  returning which rows are actually in the graph. Also the "Other" bucketing of unrecognised schema
  fields.
- `editor/app/src/editor/art/target.ts` + `target.test.ts` — **pure**: the materialization-target
  rule (`Environment` carrier, else lowest `EntityId`) over a `query.entities` result.
- `editor/app/src/editor/art/compare.ts` + `compare.test.ts` — **pure**: the hold-to-compare reducer
  (capture → neutralize → restore-exact), including the invariant that a release always restores the
  captured object rather than a recomputed one.
- `editor/app/src/editor/art/ChainPanel.tsx` — the rows; per-field widgets come from the existing
  `deriveFields`, **not** from new widgets.
- `editor/personas/art-director.md` — resolved purely by filename from `viewId`
  (`editor/app/src/terminal/TerminalDrawer.tsx:164`), so this is a file, not code.

**Edited files**

- `editor/app/src/shell/views.ts` — `enabled: true`, drop `note`.
- `editor/app/src/App.tsx` — a branch in the view ternary (the chain beginning at `:790`), plus the
  `artDirector` field in the debounced context-bridge writer.
- `editor/app/src/terminal/context.ts` — the `artDirector` block on `EditorContext` +
  `buildEditorContext` (the `voxel` block is the shape to follow).
- `editor/app/src/index.css` — a labelled block with an `ad-` class prefix, over the existing
  `:root` Meridian tokens and the shared `.main-row/.col*/.panel-head/.icon-btn/.segmented`
  primitives. (The file carries both conventions — the Game Designer's numbered `=== 7. … ===` and
  the Voxel Artist's `--- … ---` sub-block; either is fine, be consistent with the neighbours.)

**The persona** follows the 7-section skeleton every existing persona shares. Its spine, the axis
that differentiates it from the other three:

- **Imaging, not lighting.** It owns `PostProcess` and `Environment`; it *reads* `DirectionalLight`
  (shafts need a sun and take their tint from it) but does not place or shape lights.
- **The pinned pipeline order**, which it must know to answer "why did that do nothing".
- **Live set, persist to source** (decision 2) — with the generated-scene trap stated explicitly.
- **The two instruments**: `ludion render --sweep` for taste, `ludion image-stats` for defects — and
  the metric-gaming warning that governs them: *a number that moves without the image moving is a
  failure*, which the arc demonstrated twice (chromatic aberration lowering `edge_aliasing` while
  anti-aliasing nothing; a blur doing the same for free).
- **Every gate must state what it switches off** — wave 11's durable lesson, since the all-on scene
  hosts several gates and four had gone vacuous.

No change is needed in `editor/app/src/terminal/spawnConfig.ts`,
`editor/app/src-tauri/src/pty.rs`, or the session registry: the pre-approved permission set
(`mcp__ludion`, `Bash(ludion:*)`, …) already covers everything this persona does, and `scene.load`
stays on `ask`.

## Wave 2 — the cost column (engine, after wave 12's doc sweep)

`POST_GROUPS` is a private const at `crates/ludion-engine/src/profile.rs:464` and cannot be derived
by name prefix (bloom's eleven passes are `threshold`, `down_0..4`, `up_0..4`; the prepass belongs to
`ssao`). Move it down into `crates/ludion-renderer/src/graph.rs` beside the passes it names, emit it
in `dump_frame_graph_json`, and the chain view gains a per-effect millisecond figure from
`render.dump_timings` grouped by it. **No new wire method** — the `render.dump_graph` payload grows.
`ludion dump-graph` gets the same benefit for agents.

Touches `frame_graph_64x64.json` (re-snapshot) and `docs/ludion-render-graph.md`, which is why it is
sequenced after wave 12.

## The merged sitting — closes both arcs

Post-process wave 12 **drops its vigil-adopts deliverable**; the look is authored at the sitting.

- The human directs vigil's look live through the tab, on the plaza, against the real night city.
- The agent persists the result into `games/vigil/scenes/env-night.yaml`, regenerates all four
  consumers (`ludion generate-map` + `ludion map-check`, `map-drift` green), and the human confirms
  the shared fragment reads right on the market and cathedral too.
- Then, **once, after the sitting**: `run.lrp` re-record (authoring `PostProcess` adds a registered
  component to the initial world, so `WorldHash` moves), `district-plaza` golden re-bless, and the
  macOS entry in the private macOS queue updated in place.
- Also on the agenda, inherited from wave 12 and G7: the occlusion verdict (`O` cycles
  off/dither/top-fade, losers deleted after), the moon disc, the G0 camera caveat.

**Two verdicts, one sitting:** the tab is judged on whether a human could reach a look they liked
without typing a number into a file; the arc is judged on the look itself.

**Fallback, deliberately not pre-staged:** `examples/gallery/post-full.yaml`'s header is already the
worked night recipe with every value justified, including the finding that `contrast` cannot be used
on a night frame (it pivots about 0.5 and `clipped_low` breaches its ceiling — the darkening goes to
`gamma`, whose reciprocal power pins both ends). If the tab underdelivers, the agent persists that
recipe by hand during the same sitting and the arc still closes that day. Nothing is authored in
advance, so there is no stale commented block to keep coherent.

## Verification

- `node scripts/check.mjs --frontend-only` (oxlint + `tsc -b` + vitest) during the loop, then the
  full suite before committing.
- The pure-module tests carry the real invariants: hold-to-compare restores the captured object
  exactly (net-zero); the target rule prefers the `Environment` carrier; `deriveLiveEffects` reports
  shafts dead without a sun even when authored non-zero; an unrecognised schema field lands in
  "Other" rather than vanishing.
- `editor/app/src/protocol/parity.test.ts` gates the wire surface for free.
- **Live smoke** (the frontend gates cannot render React): host the plaza with
  `cargo run -p vigil -- --scene games/vigil/scenes/district-plaza.yaml` (a district scene names
  vigil's own components, so plain `ludion serve` cannot host it), attach the editor, and walk the
  loop end to end — open the tab on a scene with no `PostProcess`, confirm the effective/dimmed
  state, touch one field and watch it materialize on the named entity, hold-to-compare and confirm
  the value returns byte-exactly, spawn the persona and have it persist to `env-night.yaml` +
  regenerate + `map-check`.
- Wave 2 additionally: `frame_graph_64x64.json` re-snapshotted, `cargo test --workspace` green, and
  the chain view's group figures reconciled against `ludion profile --check` on the same scene.

## Consequences for `Plan/post-process-pipeline.md`

Wave 12's entry needs amending: the vigil-adopts bullet moves to the sitting, the `run.lrp`
re-record and `district-plaza` re-bless move to after it, the doc sweep is unchanged, and the exit
gate gains the tab as its instrument — which also resolves the standing ambiguity that the gate as
written judges "committed sweep sheets" that wave 5 explicitly declined to commit.

## Current-code facts that shape the work

- `crates/ludion-engine/src/components.rs` — `PostProcess`'s doc comment is the contract: the opt-in
  singleton rule (lowest-`EntityId` wins), *absence is meaningful*, the composite's pinned order,
  and per-effect notes (fog's skip on background pixels, the grade's pinned order of operations,
  SSAO sitting **outside** the composite because it modulates lighting, the shafts two-gate rule,
  DoF blending first). `Environment` sits just above it with the same posture, and the split between
  the two is by subject: `Environment` authors *lighting*, `PostProcess` authors *imaging*.
- `crates/ludion-renderer/src/graph.rs` — `PostChain` / `PostChain::for_scene`: the one place the
  "which passes does this scene need" question is answered, so the tab must reproduce it rather than
  invent a second answer. `shafts` is the only effect gated on the scene as well as on a field.
- `crates/ludion-renderer/src/scene.rs` — `PostData::default` is the engine's shipped imaging in one
  place, and the doc comment states the guarantee the tab's "effective look" display depends on.
- `crates/ludion-engine/src/profile.rs:464` — `POST_GROUPS`, the four post budget groups, private
  and deliberately without `composite` or `prepass` entries. Wave 2 moves it.
- `editor/app/src/viewer/inspect-fields.ts` — `deriveFields(schema, value)`, the existing
  schema→widget derivation to reuse unchanged.
- `editor/app/src/views/GameDesignerView.tsx` / `VerticalSplit.tsx` — the props shape and the
  vertical split to reuse.
- `editor/app/src/terminal/context.ts` — `EditorContext` and `buildEditorContext`; the `voxel` block
  is the precedent for `artDirector` (optional on input, defaulting to `null`).
- `games/vigil/scenes/env-night.yaml` — the shared fragment: an `environment` entity carrying
  `Environment` and a `moon` entity carrying `Transform` + `DirectionalLight`. It is merged by the
  generator into **four** maps (`district-plaza`, `district-market`, `district-cathedral`,
  `budget-horde`) via each `.map.yaml`'s `fragments:` list, and each of those four scenes carries
  `generated_from:` — which is exactly why the tab must not save.
- `examples/gallery/post-full.yaml` — the all-on scene and the worked night recipe; also the host
  for several `image-stats` gates, each of which must state what it switches off.
