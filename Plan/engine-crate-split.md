# Plan: Move the instruments out of `ludion-engine`

> Status: **planned, not started.** Source: the 2026-09-25 code audit, finding J2 (the architecture
> doc's risk #10, "`ludion-engine` god-crate drift"). Current-code facts were read from the tree on
> 2026-09-26; file:line pointers are of that date, so re-grep before editing.

## Trigger

`ludion-engine` is meant to be glue: logic lives in the domain crates. It went from 16k to 49k lines
in two months. The largest growth is CPU-only instruments that touch no `World`, renderer or GPU. Every
game, test and tool that links the engine compiles wgpu, rapier3d, sculpt, sprite and spatial: about
198 crates, against 35 for `ludion-sim`.

Build it when **any one** of these happens:

- a pipeline verb or tool wants a sheet builder without linking the renderer;
- a component field rename breaks `synthesize_scene` at runtime (below);
- `ludion-engine` passes 55k lines;
- `cargo check -p ludion-engine` after a one-line edit exceeds the 20-iteration fix-loop budget on the
  reference box.

W1 (the sprite override move) is cheap and can go earlier on its own.

## What exists (2026-09-26)

**`crates/ludion-engine/src` is 49,462 lines.** The largest files:

| File | Lines |
|---|---|
| `components.rs` | 5,515 |
| `model_sheet.rs` | 4,183 |
| `engine.rs` | 4,125 |
| `extract.rs` | 3,633 |
| `sprite_sheet.rs` | 3,047 |
| `patterns.rs` | 2,175 |
| `profile.rs` | 1,874 |
| `voxel_sheet.rs` | 1,058 |
| `sweep.rs` | 663 |

**`sprite_sheet.rs` is the engine's only `ludion-sprite` user** (`:70`, `BakedFrame`). It touches no
`World`, renderer or wgpu, and borrows layout helpers from `voxel_sheet.rs`.

- It holds `apply_sprite_overrides` (`:596`, about 500 lines with its writers).
- Its siblings live in `ludion-scene`: `apply_overrides` (`overrides.rs:209`), `apply_sculpt_overrides`
  (`sculpt.rs:2135`), and `apply_palette_overrides` / `apply_assembly_palette_overrides`
  (`voxel.rs:2251,2304`).

**`resolve_sprite_palette`** (`sprite_sheet.rs:428`, re-exported at `lib.rs:149`) is called by two
CLI verbs: `generate_atlas.rs:255` and `check_scene.rs:687`.

**`model_sheet.rs::synthesize_scene` (`:2925`) is a second YAML emitter.** It is built from `format!`
and a `quote_yaml` that escapes only `\` and `"` (`:2962-2998`), so it bypasses the canonical writer.

**`extract.rs` mixes four concerns:**

- render extraction (`:483-588` and the per-feature extractors);
- atlas IO (`load_texture` `:328`, `load_atlas` `:337`, `AtlasCache` `:262`);
- validation (`:396-444`);
- assembly pose math (`animation_pose` `:2326`, `part_placements[_with]` `:2428,2440`,
  `compose_chain` `:2493`).

The sim-side ragdoll (`ragdoll.rs:35`) imports that pose math and passes its own libm Euler conversion
(`:311`).

**`components.rs`** has about 3,185 lines of code and a large test module. It holds 24 component
structs, their `SemanticValidate` helpers, and about 30 free serde default functions (`:2897-2972`).

## Proposed design

- **W1 — `apply_sprite_overrides` moves to `ludion-scene`**, next to its four siblings, together with
  `resolve_sprite_palette`.
  - `generate-atlas` and `check-scene` then import from `ludion-scene`.
  - This is a pure move: no behaviour change, no golden moves.
- **W2 — a `ludion-sheet` crate.**
  - It takes the four sheet builders (`model_sheet`, `sprite_sheet`, `voxel_sheet`, `sweep`) and their
    shared grid/blit/stamp/font layer.
  - `ModelCamera` gets its own module.
  - The crate depends on `ludion-sprite` and on what the builders actually read. `ludion-engine`
    depends on `ludion-sheet`, not the reverse.
  - Where a builder needs a rendered frame, it takes the render as a callback or a trait object that
    the engine supplies. Settle this seam first: `model_sheet` calls `extract_at`. Measure whether the
    seam is worth it, or whether only `sprite_sheet` and the layout layer move.
- **W3 — the model-sheet scene is built as `ludion_scene::Scene` values** and written through the
  canonical writer. `quote_yaml` and the `format!` YAML are deleted.
- **W4 — split `extract.rs`:**
  - render extraction stays;
  - `atlas.rs` takes the atlas IO and validation;
  - a sim-safe `pose.rs` takes the assembly pose math. It always uses libm, so the ragdoll stops
    passing a conversion in. This coordinates with `Plan/rotation-representation.md`, which owns where
    the conversion lives.
- **W5 — split `components.rs`** into `components/{render,post,ui,tilemap,fx}.rs` plus `registry.rs`,
  with each family's defaults beside it. Registration order is append-only and hashed, so `registry.rs`
  keeps the one ordered list.

## Risks

- **Registration order.**
  - W5 must not reorder `register_*` calls. The schema snapshot and every hash are the judge.
  - Split by moving code, never by re-sequencing it.
- **The sheet builders gate goldens.**
  - Model, sprite and voxel sheets have pinned goldens on both platforms. W2 and W3 must leave every
    sheet golden byte-identical on the reference box.
  - W3 in particular changes the emitter. Diff the synthesized YAML before and after on every committed
    model, and expect it to be identical modulo canonical key order. If it is not, say why.
- **Churn against open arcs.** `extract.rs` and `components.rs` are edited by most rendering work.
  Sequence each wave as a single move commit.
- **Docs.** `dev-docs/ludion-engine.md` and `dev-docs/crate-graph.md` describe the file layout, and W2
  adds a crate: update `engine-architecture.md` §2's crate graph and the per-crate clippy denylist set.

## Exit gate

- `node scripts/check.mjs` green per wave. No golden, trace, schema hash or `.lrp` moves in any wave.
- After W2, `ludion-engine` no longer depends on `ludion-sprite`, and the `generate-atlas` verb's
  imports name no `ludion_engine` symbol.
- Report `cargo tree -p ludion-engine | wc -l` before and after, and the wall time of `cargo check`
  after a one-line edit in `sprite_sheet.rs`'s new home.

## Non-goals

- Splitting `engine.rs`. That is `Plan/engine-carving.md`.
- Splitting `ludion-renderer`'s `render.rs`. That is `Plan/build-graph-and-par.md`.
- Making any instrument a protocol command.

## Status

Planned, not started (2026-09-26). Nothing from this plan shipped in the 2026-09 fix stage. The kind
table that `model_sheet` should read instead of matching kind names is in place (`ludion_scene::KINDS`);
see `Plan/document-kinds.md`.
