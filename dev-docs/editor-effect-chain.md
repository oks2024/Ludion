---
description: chain.ts's effect model, what stage/gate mean, and why an effect needs a FIELDS row
---

# Editor Effect Chain

**Scope:** `editor/app/src/editor/art/chain.ts` — the Art Director panel's model of the
post-process chain: what `EffectId`/`EffectStage` mean, what `FIELDS` drives, and the rule that
keeps hold-to-compare honest. Defers `PostProcess` field semantics themselves to the renderer's own
docs (`ludion devdocs renderer-passes`, `ludion devdocs renderer-shading`) and the wire-level
component schema to `ludion devdocs ludion-protocol`; this topic covers only what the editor
computes on top of them.

## Layout

- `chain.ts` — pure: no React, no wire, no DOM. `EffectId`, `EffectStage`, `EFFECT_ORDER` (pipeline
  order plus per-row preconditions), `FIELDS` (per-field effect/neutral/gate/shipped), the derived
  functions `deriveLiveEffects`, `neutralPatch`, `isComparable`, `bucketSchemaFields`,
  `reconcileWithGraph`, `gateStateKey`. (Rule 3: do not transcribe `EFFECT_ORDER`'s or `FIELDS`'
  row contents here — both are one read of the file away, and every phase's post-process wave adds
  or reshapes rows.)
- `compare.ts` — `beginCompare`/`endCompare`: the hold-to-compare session, built entirely on
  `chain.ts`'s `isComparable`/`neutralPatch`. Owns none of the "what is neutral" logic itself.
- `ChainPanel.tsx` — the accordion UI: one row per `EFFECT_ORDER` entry plus an `other` row for
  `bucketSchemaFields`'s drift bucket. Field widgets are delegated to `viewer/inspect-fields.ts` +
  `viewer/FieldWidgets.tsx` — the same layer the ordinary entity inspector edits through, so the two
  surfaces can never disagree about what an edit does.
- `chain.test.ts` / `compare.test.ts` — the two suites that gate this file; see Verify.

## Dataflow

- `deriveLiveEffects(values, ctx)` reads a scene's authored `PostProcess` values (`{}` for
  unauthored — every field then reads its `shipped` default, which is what the engine actually
  renders) plus a `ChainContext` (today only `hasSun`: whether the scene carries a
  `DirectionalLight`). An effect is *engaged* when any of its `gate: true` fields differs from that
  field's `neutral`; it is *live* when engaged AND its scene precondition (`requires`) AND its
  effect precondition (`requiresEffect`) both hold. This mirrors the engine's own gate condition by
  design — `chain.ts`'s header names the exact expression it generalizes (`post.shafts > 0.0 &&
  post.fog_density > 0.0 && scene.sun.is_some()`), which is the thing to re-check if that condition's
  shape ever changes in the renderer.
- `gateStateKey` exists because only 3 of the 13 rows (`ssao`, `dof`, `shafts` —
  `PASS_GATING_EFFECTS`) can add or remove a frame-graph pass; every other effect is a branch inside
  a pass the graph builds unconditionally. A caller refetches `render.dump_graph` only when this key
  moves, so an ordinary fog-density nudge never costs a round trip.
- `reconcileWithGraph` cross-checks the derived model against a real `render.dump_graph` pass list.
  A disagreement in either direction (`expected` vs `present`) is always surfaced, never swallowed —
  the panel is built to look wrong out loud rather than silently drift from the engine.

## Rules

- **A renderer effect is not done until its `chain.ts` `FIELDS` entry exists.** `neutralPatch(effect)`
  — the function `compare.ts`'s hold-to-compare press writes — only neutralizes the fields
  `effectFields(effect)` returns, which is exactly `FIELDS`' reverse index. A field the engine ships
  but this table does not know is invisible to `neutralPatch`: pressing that effect's A/B toggle
  writes every *known* field to neutral and leaves the unknown one live, so the "without this effect"
  frame the human is judging still carries it.
- This is not hypothetical — it happened. `PostProcess.ssao_direct` landed on the wire 2026-07-26
  (the component field only) and validated fine; `env-night.yaml` already authored `ssao_direct: 1.0`
  through the shared night grade. It sat authored, validated, and doing nothing in the Art Director's
  SSAO comparison until the renderer plumbing and the `chain.ts` `FIELDS` row landed together
  2026-07-29 (Lore rev 405 per CLAUDE.md's Current-state). The graphics notebook's own record:
  "without the row the Art Director's hold-to-compare A/B on the `ssao` row left `ssao_direct`
  contributing and the 'off' half was not off." No diagnostic fires for this gap — the field passes
  schema validation cleanly — so the only symptom is a human directing a look against a comparison
  that silently compares nothing.
- **The one safety net today is partial.** `bucketSchemaFields`, run against `engine.describe`'s
  runtime schema, puts a wholly-unrecognized field name into an `other` bucket instead of dropping
  it, and reports the reverse drift (`missing`) when this table names a field the schema no longer
  has. That catches a field `chain.ts` has never heard of — visible, not neutralized. It does not
  make `neutralPatch` correct on its own: a reviewer must still confirm the new field's `FIELDS` row
  exists and carries the right `gate`, because the bucket check only proves the omission is visible,
  not that the comparison is honest.
- `stage` says *where in the frame* an effect executes and is not itself load-bearing for
  `deriveLiveEffects`/`neutralPatch` — it is what makes the panel's fixed order legible rather than
  arbitrary. Three values: `scene` (inside the scene pass — only `ssao`, because ambient occlusion
  modulates the ambient/IBL term rather than being an imaging step), `fetch` (the composite's read of
  the rendered scene, before any colour has formed — only `chromatic_aberration`, because it is how
  the image was sampled, not a filter applied after), `composite` (a step of `composite.wgsl`'s
  pinned execution order — every other row). `EFFECT_ORDER`'s array order is pinned to that same
  execution order and is the tiebreaker whenever prose or schema order would disagree with it
  (`chain.ts`'s own header comment says so explicitly).
- `gate` is per-**field**, not per-effect, and a field can carry a real (non-null) `neutral` while
  still being `gate: false` — its own value has a meaningful "nothing" state without itself switching
  the effect off. Four fields are this shape: `fog_height_falloff`, `fog_albedo`, `fog_sky`, and
  `ssao_direct` (each effect's actual gate is a different field — `fog_density`, `ssao`, etc.).
  Guessing "has a neutral" implies "is the gate" is wrong for all four; `chain.test.ts`'s "the field
  table" block pins each case by name.
- `alwaysLive` exists for exactly one row, `tonemap`: every operator is a curve, so there is no value
  that means "off". `isComparable('tonemap')` is `false` for the same reason, `neutralPatch('tonemap')`
  returns `{}`, and the UI disables that row's hold-to-compare toggle rather than offering one that
  visibly does nothing.

## Gotchas

- There is no `EFFECT_META`. The per-field shipped/neutral/gate table is named `FIELDS` (a
  `Record<string, FieldEntry>`); the per-row pipeline metadata (`stage`, `gatedPasses`, `requires`,
  `requiresEffect`, `alwaysLive`) lives separately on `EFFECT_ORDER`'s `EffectRow` array, keyed by
  `EffectId`. The two are joined through `effectFields(effect)`/`effectRow(effect)`, never merged
  into one table.
- Nothing ties `FIELDS`/`EFFECT_ORDER` to the Rust `PostProcess` struct at compile time —
  `editor/protocol/src/protocol.d.ts` carries only the wire command union, never component schemas
  (`chain.ts`'s header cites this as the post-process plan's decision 11). The only drift detector is
  `bucketSchemaFields`, run at runtime against a live `engine.describe` schema; no build-time or CI
  step fails when a new `PostProcess` field ships with no matching `FIELDS` row. Catching the
  ssao_direct-shaped gap earlier than "someone opens the Art Director tab and notices a field sitting
  in `other`" requires a human doing that check, not a gate that runs itself.
- `effectFields`/`neutralPatch` iterate `FIELD_NAMES` (`Object.keys(FIELDS)`), so a field missing its
  `FIELDS` entry is invisible to every function in this file, not `neutralPatch` alone —
  `bucketSchemaFields`'s `other` bucket is the only place such a field becomes visible at all.
- `requiresEffect` (only on `shafts`, naming `fog`) deliberately reads the other effect's
  *engagement*, not its *live* state — chaining on `live` would make a sunless scene's shafts row
  blame the fog instead of the sun, the wrong answer since the sun is the blocker the panel cannot
  fix from this row. `chain.test.ts`'s "blames the sun first" case pins this.

## Verify

```
cd editor/app
npx vitest run src/editor/art/chain.test.ts
npx vitest run src/editor/art/compare.test.ts
npx tsc -b        # the only check that would catch a FIELDS/EFFECT_ORDER shape mistake at all
```

No Rust-side test exercises this file — it has no wire dependency of its own. The manual cross-check
for "did the new effect's row land": open the Art Director tab against a scene authoring the new
field and confirm it renders as one of the pinned rows, not as an entry under `other`.
