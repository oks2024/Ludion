# Plan: Voxel Authoring Tools — write-side ergonomics for large models

> Status: **MECHANIZED-COMPLETE (2026-08-01, waves 1–4).** Wave 1: the ops engine
> (`crates/ludion-scene/src/voxel_edit.rs`) + `ludion voxel new|apply|diff`, the comment-carrying
> emitter, and both new format keys (`generated_from`, `budget_exempt`; `VOXEL_KEYS` 7 → 9).
> Wave 2: `ludion voxel upscale`. Wave 3: the generation kit — `scripts/voxel/grid.mjs`,
> `ludion voxel from-json` over the `kind: voxel-gen` JSON intermediate (`L1133`),
> `examples/voxel-gen/brazier.*` as the runnable example, and the `voxel-drift` gate in check.mjs +
> ci.yml. Wave 4: the written recipe — the engine guide's authoring ladder, the `ludion-scene` and
> `ludion-cli` docs topics, and the voxel-artist persona paragraph. Zero wire methods, zero
> render-path changes, zero golden or `run.lrp` churn, exactly as scoped.
>
> **Two things ride the next user sitting and are deliberately NOT done:**
> (1) the **L1115 ceiling** — the concrete proposal is drafted verbatim in "Proposal for the sitting
> — the L1115 ceiling" below, and no Rust changed; the budget and `budget_exempt: true` stand as
> shipped until ruled on. (2) Wave 4's **informal THE-test** — "a fresh instance pointed at 'make a
> 60³ creature' finds the ladder from `ludion docs` alone" — which needs a fresh instance to be
> honest and so cannot be self-certified here.
>
> Original approval note, kept for the record:
>
> **APPROVED 2026-08-01, implementation started.** Written because the trigger recorded in
> `Plan/roadmap-voxel-mesh-pbr.md` (decision 11) fired. User verdicts on the open questions:
> `ludion voxel <sub>` subcommand family (question 1); the L1115 budget stays and oversize models
> carry an **explicit opt-out key** — `budget_exempt: true` — to be revisited later (question 2);
> and the tools' consumer of record is the shipped Voxel Artist editor view (its persona drives
> them from the editor terminal), so the persona/docs wave is load-bearing, not garnish.

## The problem, with evidence

Agent instances authoring high-resolution voxel models (the `-hd` variants: `lantern-hd` at
32×40×32, `figure-hd`; ambitions run to ~60³) hit the same wall repeatedly:

1. **Hand-editing fails above ~32³.** Editing means counting columns inside 60-char fixed-width
   quoted rows across 60 layers. The persona's surgical-edit discipline (one character at a known
   line/column) is correct but does not scale to shape-sized edits — a misplaced ellipse is
   hundreds of cells.
2. **Every instance re-invents the same generator.** `games/vigil/.ludion/scratch/gen-lantern-hd.mjs`
   is the surviving specimen: ~340 lines, of which maybe 60 are the lantern. The rest is substrate
   every instance rebuilds from scratch — a grid, `setCell`, `fillEllipse`, symmetric fill, and a
   hand-concatenated YAML emitter duplicating the canonical format and comment conventions by
   string template. The scripts live in gitignored scratch, so the next instance cannot find or
   extend them and starts over.
3. **The HD models are conceptually upscales.** `lantern-hd`'s own header says *"~3x grid
   resolution of the low-res variant… a drop-in higher-detail replacement"* — the instance was
   re-deriving from scratch a model whose correct blocky base was one nearest-neighbor upscale
   away from an existing, proven 12³ file.

The **read side is already solved** and stays untouched: `check-scene --mirror-x/--slice/--column`,
L1129 connectivity, `render --model` contact sheets, the editor's Voxel Artist view for human
judgment. The gap is entirely the **write side above ~32³**.

## Goal

An agent can produce a correct, connected, palette-disciplined 48–64³ model in a handful of
commands and verify it without a render — by *editing* (upscale a proven low-res model, sculpt
with coordinate-addressed ops) or by *generating* (a committed script over a shared library, with
canonical serialization done by the engine, never by the script). Hand-rolled YAML emitters and
line/column arithmetic both become obsolete.

Scored against the North Star: today one HD model costs an instance a bespoke generator script plus
several failed hand-edits; the target is that the substrate is amortized once, in the engine, and
each model costs only the model.

## Settled context this plan builds on (not re-litigated)

- Voxels are a producer of meshes; the `.voxel.yaml` format is fixed (7 keys incl. `scale`); format
  compression stays rejected; **the generator script IS the sanctioned compression** (roadmap
  decisions 2, 11).
- Comments are load-bearing: the header docstring and per-slice anatomy comments are how the next
  agent re-reads intent. Any tool that erases them is broken (editor plan contract 1).
- `emit_voxel` is the one canonical writer (byte-pinned + idempotence proptests in
  `crates/ludion-scene/tests/voxel_roundtrip.rs`).
- "Rust is the only Turing-complete layer" governs the **engine and games**, not build tooling —
  `synth-sfx.mjs`, `check.mjs`, `commit.mjs` are the precedent. Generator scripts are build tooling
  and are Node by the repo's cross-platform-tooling convention.
- The `generated_from:` + drift-gate pattern (map-drift) is the shipped shape for
  committed-generated content.

## Decisions proposed by this plan

1. **One new CLI verb family: `ludion voxel <sub>`** (`new`, `apply`, `upscale`, `diff`,
   `from-json`). A subcommand family rather than five flat verbs — the flat list (`generate-map`,
   `map-check`, …) is already long, and these five share a domain, an ops engine, and a help page.
   (Open question 1 if the user prefers flat.)
2. **Ops files are the one edit interface for batch edits.** `voxel apply <model> --ops <ops.yaml>`
   takes a typed list of operations; no flag-soup equivalent (one obvious way). Single-cell edits
   remain what they are today: a direct surgical text edit, as the persona teaches.
3. **Surgical for edits, canonical for births.** `apply` mutates the existing file in place,
   per-character, preserving every comment, blank line, and key order byte-for-byte outside the
   edited cells. `new`, `upscale --out`, and `from-json` create files and write canonical text via
   `emit_voxel` (extended to carry comments — see Wave 1).
4. **Validate-before-write, atomically.** Every writing verb parses + validates the result
   in memory (same checks as `check-scene`) and writes temp + rename only on success. A bad op
   yields an L-coded diagnostic naming the op index and the cell; the file on disk never holds a
   half-applied state. (This also protects the editor's watcher path: a save is never a blank.)
5. **Generation = script → JSON intermediate → engine emits.** Scripts never write YAML. A
   committed library (`scripts/voxel/grid.mjs`) builds the grid; the script prints one JSON object
   (`size`, `scale`, `palette`, `layers`, `layer_comments`, `header`); `ludion voxel from-json`
   validates it and writes the canonical file. Canonical serialization stays in exactly one place.
6. **`generated_from:` marks pure-generated models and gates them.** A model whose source of truth
   is its script carries `generated_from: <name>.voxel.gen.mjs` (script committed next to the
   model, same as `.map.yaml` sources) and joins a new `voxel-drift` gate in check.mjs + ci.yml:
   regenerate byte-identical or the commit is rejected. A script that was one-shot scaffolding for
   later hand-sculpting stays in `.ludion/scratch/` and the file is authoritative — presence of
   `generated_from:` is the marker, exactly the rule the maps already taught.
7. **The documented ladder for >48³:** hand-author ≤32³ → `voxel upscale` → sculpt with `apply`
   ops → verify with `check-scene` / `render --model`. From-scratch generation is the fallback for
   models with no low-res ancestor. The guide and the voxel-artist persona both say so, and both
   explicitly ban hand-rolled YAML emitters.

## The tools

### `voxel new <out> --size W,H,D [--palette-from <model>]`
Scaffold: empty layers, palette copied from an existing model or a commented stub, header comment
template (silhouette intent / front-face convention / intended scale — the fields every good vigil
model already carries).

### `voxel apply <model> --ops <ops.yaml>`
Typed ops, executed in order, each addressed in voxel coordinates:

- `set {at: [x,y,z], glyph: G}` — one cell (also the `.`-carve form via `glyph: "."`).
- `box {from: [x,y,z], to: [x,y,z], glyph: G}` / `ellipsoid {center, radii, glyph}` /
  `cylinder {base, axis, radius, height, glyph}` / `line {from, to, glyph}`.
- `paint {…same shapes…}` — recolor only non-empty cells (sculpting detail onto a surface without
  changing silhouette).
- `mirror {axis: x, from: left|right}` — copy one half over the other (make symmetry true, not
  just measured).
- `shift {by: [dx,dy,dz]}` / `crop {…}` / `grow {…}` — geometry-preserving reshapes of the grid.
- `comment {y: N, text: "…"}` — update a layer's anatomy comment when the anatomy changed.

All shape ops clip to the grid. The ops file is reviewable, diffable, and cheap for an agent to
write — and it is a typed, serializable command list, which is the house style.

### `voxel upscale <model> --factor N [--out <path>]`
Nearest-neighbor: each source cell becomes an N³ block; palette and `scale` carried over (`scale`
divided by N so the world footprint is unchanged — the header's own arithmetic in `lantern-hd`
confirms this is the intent); each source layer's comment carried onto the first of its N output
layers. Output is a correct, connected, drop-in base for sculpting. **This is the single
highest-leverage verb** — it converts "generate 216k cells" into "edit a correct model".

### `voxel diff <a> <b>`
Per-layer cell diff (counts + a `--slice`-style annotated view of changed layers). "Did my edit
land" gets a text answer before any render; also the review tool for regenerated models.

### `scripts/voxel/grid.mjs` + `voxel from-json`
The committed generation library: `Grid` (size, palette), the fill primitives above as functions,
`lathe`/`loft` from a per-y profile (the actual shape of `gen-lantern-hd.mjs` — stacked
interpolated ellipses), mirror helpers, seeded-RNG speckle (`Math.random` banned — output must be
byte-idempotent for the drift gate), and `emit()` which prints the JSON intermediate to stdout.
Convention: `node <name>.voxel.gen.mjs | ludion voxel from-json --out <name>.voxel.yaml`.

## Phasing

Each wave independently green (`node scripts/check.mjs`, scoped) and independently useful.

### Wave 1 — the ops engine (`ludion-scene` + CLI: `new`, `apply`, `diff`)
- Both new format keys land here (not wave 3/4), following the `scale` precedent — parsed,
  validated, emitted, never consumed by the mesher: `generated_from` (string) and
  `budget_exempt` (bool, only `true` is valid; suppresses L1115). `VOXEL_KEYS` 7 → 9.
- `crates/ludion-scene/src/voxel_edit.rs`: the op types (strict-YAML ops document), the
  in-place surgical executor over the source text, the comment extractor (header + per-layer —
  needed by grid-reshape ops and by wave 2's upscale), validation integration. New L-codes
  registered in ludion-core (ops parse errors, op-out-of-range, post-apply validation failure).
- Extend `emit_voxel` with optional header/layer comments (needed by every birthing verb; the
  existing pinned fixture stays byte-identical when no comments are supplied).
- CLI verbs + `--help`; tests: byte-preservation outside edited cells (the editor plan's wave-4
  test discipline, applied here first), op semantics per shape, atomic-write-on-failure.
- Exit: an agent can sculpt `lantern-hd` with an ops file and `voxel diff` shows exactly the
  intended cells changed; comments byte-identical.

### Wave 2 — `upscale`
- Rides the wave-1 engine and comment-carrying emitter. Tests: factor round trips (upscale
  preserves connectivity and palette; `scale`/N arithmetic; comment carry-down), plus one real
  proof: upscale `lantern` 12³→×3 and diff against the hand-built `lantern-hd` base as a sanity
  report (not a gate — they will differ; the point is the base is usable).
- Exit: 15³ → 60³ in one command, `check-scene` clean, sheet renders sane.

### Wave 3 — the generation kit (`grid.mjs`, `from-json`, `voxel-drift` gate)
- JSON intermediate schema (documented in the guide; validated by `from-json` with pointer
  diagnostics); `scripts/voxel/grid.mjs` with a small runnable example generator; the
  `generated_from:` key (8th key — same wave-0-style contract as `scale`: parsed, validated,
  emitted, **never consumed by the mesher**, no golden or replay churn); `voxel-drift` in
  check.mjs + ci.yml patterned on `map-drift`.
- Migrate `gen-lantern-hd.mjs` onto the library as the proving case — whichever of its ~280
  substrate lines the library cannot absorb is a library gap.
- Exit: gate green on a committed generated model; the example generator's output byte-idempotent.

### Wave 4 — the written recipe + ceiling revisit
- Engine-guide section (the ladder, the JSON schema, the ban on hand-rolled emitters), embedded
  mirror re-synced, `ludion docs` topic updated, `editor/personas/voxel-artist.md` paragraph.
- Revisit the L1115 cell-count budget: single-piece creatures legitimately want 48–64³ (a monster
  does not assemble from parts the way a building does). Proposal: raise the warning threshold for
  models, keep the guidance that *architecture* above 48³ assembles — wording to the user at the
  sitting. Roadmap decision 11 gets an addendum pointing here; the private voxel-engine notebook updated.
- Exit: a fresh instance pointed at "make a 60³ creature" finds the ladder from `ludion docs`
  alone (the THE-test discipline, informally).

**Delivered 2026-08-01.** Engine guide: `docs/engine-guide.md` → "Voxel models" gained the
`### Authoring above hand-editing size — the ladder` section (the three rungs in order, one
subsection per verb with copyable examples, the ops-file reference, the generation pipeline + the
ban on hand-rolled emitters, the JSON intermediate schema, `generated_from:`/the drift gate,
`budget_exempt:`); the chapter's key list went 7 → 9 keys, and the `L1115` pitfall and the
`author → check → render` loop's step 1 now point at the ladder. `ludion docs` topics: `ludion-scene`
gained a "Voxel authoring (the write side)" section plus the two new `Layout` entries and the 9-key
set; `ludion-cli` gained the nested `voxel <verb>` family and a Rules clause for nested routing.
Persona: `editor/personas/voxel-artist.md` gained the write-verbs paragraph in its Edit contract.
The ceiling is a proposal only, drafted below — no Rust changed in this wave.

## Proposal for the sitting — the L1115 ceiling

Drafted 2026-08-01 to be ruled on verbatim. **Nothing here is implemented**; `L1115` and
`VOXEL_CELL_BUDGET` stand exactly as shipped until the user rules.

**The finding.** The budget was set when the only voxel content was props. Two things have changed:
the tooling now makes a large *single-piece* model cheap to produce correctly (`upscale` → `apply`),
and the content that wants it is creatures — which do not assemble from parts the way a building
does. A hero creature authored at the density convention lands at roughly 64 wide × 80 tall × 64
deep = 327,680 cells, which warns today; so does the ordinary ×2 upscale of a proven 32×48×32. The
warning is firing on the *sanctioned* path, which is the definition of a mis-set threshold — and the
`budget_exempt: true` opt-out, settled as provisional at this plan's approval, is currently absorbing
that misfire rather than marking genuine outliers.

**The proposal, in three parts:**

> 1. **Raise the `L1115` warning threshold from 262,144 cells (64³) to 393,216 cells (64×96×64).**
>    The new number is chosen to clear a 64-wide creature at natural human-ish proportions and to
>    clear a ×2 upscale of any model that was itself inside the old budget — i.e. the ladder's
>    normal output never warns. It is a warning threshold, not a capability limit: nothing about
>    meshing, loading, or rendering changes.
> 2. **Re-word the guidance, keeping the architecture rule intact.** The engine guide's sweet spot
>    becomes: *single-piece models (creatures, hero props) author at 16³–48³ and legitimately reach
>    48–64³ when the silhouette needs the resolution; architecture and world-scale content above
>    48³ still assembles from several models — the engine has no chunked-voxel streaming.* The
>    `L1115` explanation text changes the same way: "split it into several smaller models" stays the
>    fix for *structures*, and stops being the blanket advice for everything oversize.
> 3. **`budget_exempt: true` stays exactly as it is** — the explicit, author-only opt-out, never
>    written by a tool, `false` still an `L1114`. With the threshold corrected it goes back to
>    meaning what it was designed to mean: "this model is a deliberate outlier", not "the budget is
>    mis-set". No existing file needs to change; a model that no longer warns may drop the key at
>    the next edit, but nothing forces it.

**If ruled yes**, the implementation is small and self-contained: `VOXEL_CELL_BUDGET` in
`crates/ludion-scene/src/voxel.rs`, the `L1115` explanation in `crates/ludion-core/src/codes.rs`,
the two guidance paragraphs in `docs/engine-guide.md` (+ the embedded mirror), and the budget-crossing
CLI test in `crates/ludion-cli/tests/voxel_cmd.rs` (its fixture would need a larger model to keep
gating the warning). No golden, no replay, no wire change — `L1115` is a warning that never fails a
build.

**Alternatives, for the ruling to reject explicitly:**

- **Leave 64³ and lean on `budget_exempt`.** Cheapest, and defensible: every oversize model then
  carries a visible, deliberate marker. Cost: the marker appears on ordinary creatures, so it stops
  discriminating, and the ladder's own output warns.
- **Drop `L1115` for models entirely**, keeping only the connectivity warning. Rejected in this
  draft: the budget's real job — catching content that drifted toward world scale in one file, which
  the engine cannot stream — is still a job worth doing.
- **Split the budget by aspect**: warn on total cells only above a *cubic* extent (e.g. any
  dimension > 96). More precise, more machinery, and no evidence yet that the single scalar
  mis-fires in any direction but the one above.

## Deliberately not building

- **A declarative `kind: voxel-gen` format.** Organic models need interpolation and conditionals;
  the roadmap settled that scripts are the compression. A DSL here is cleverness over regularity.
- **MagicaVoxel `.vox` import.** Serves only humans (agents cannot drive MagicaVoxel);
  trigger-gate it like the atlas pipeline: first human artist who shows up with a `.vox` file.
- **Editor changes / wire surface.** Zero new wire methods, zero render-path changes, no golden or
  `run.lrp` churn anywhere in this plan (the `generated_from:` key follows the `scale` precedent:
  metadata, never consumed by the mesher). The editor's slice painter is the human tool; this arc
  is the agent tool.
- **Rust-side generator scripts.** Considered (determinism culture favors Rust) and rejected:
  generation is offline build tooling with a byte-idempotence gate on its output — the gate, not
  the language, is the determinism guarantee, and Node is the repo's settled dev-script layer.

## Open questions — settled 2026-08-01

1. **Verb shape:** `ludion voxel <sub>` family. SETTLED (user).
2. **The ceiling:** keep the L1115 budget; a model above it declares `budget_exempt: true` (a new
   optional key, valid only as `true` — present-and-false is an L1114 error suggesting removal).
   Explicitly provisional; may be revisited. SETTLED (user).
3. **Editor convergence (later, not this plan):** the editor's TS `edit.ts` (planned wave 4 of
   `Plan/editor-voxel-artist.md`) and this Rust ops engine will be two surgical editors of the same
   format. Acceptable duplication for now (different processes, different granularity), but the
   "one checker, the engine's" principle suggests the painter could eventually shell to
   `voxel apply` the way the readout shells to `check-scene`. Flagged so the two plans know about
   each other; no action proposed.

## Current-code facts that shape the work

- `crates/ludion-scene/src/voxel.rs:43` — `VOXEL_KEYS` is a strict 7-key allowlist (`scale` landed
  as the 7th, editor plan wave 0); `VoxelDocument` at `:86` already exposes `palette_index`,
  `filled_count`, `component_count`, `mirror_x_pairs`, `render_slice`, `render_column` — the ops
  engine reuses these rather than growing a second grid model.
- `crates/ludion-cli/src/check_scene.rs:129` — `run_voxel` + the `--mirror-x`/`--slice`/`--column`
  views: the read-side surface the write side must mirror in style (usage errors, block output).
- `ludion_scene::emit_voxel` — canonical writer, byte-pinned in
  `crates/ludion-scene/tests/voxel_roundtrip.rs`; currently comment-less (Wave 1 extends it).
- `games/vigil/.ludion/scratch/gen-lantern-hd.mjs` — the specimen ad-hoc generator; Wave 3's
  proving case.
- `games/vigil/scenes/voxels/` — 19+ models incl. the `-hd` pair; every model carries the header
  docstring + per-slice comments the tools must preserve.
- check.mjs `map-drift` + `.github/workflows/ci.yml` — the drift-gate pattern `voxel-drift` copies.
- `games/vigil/CLAUDE.md` sounds rule — the `synth-sfx.mjs` precedent this plan's generation
  convention is modeled on (committed script, write-only byte-idempotent output).
