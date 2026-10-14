# Documentation rework

Agent-docs exist so an instance can build a focused context **before** it reads source. This arc splits
them by audience, carves the single-file guide into task-scoped topics, and condenses what survives.

## The trigger, measured

29 files, 634 KB (~160k tokens), audited 2026-08-03:

- `docs/engine-guide.md` — 171 KB / 3033 lines, **no table of contents**. Its voxel chapter alone is
  47.6 KB (28%); voxel plus the four rendering chapters are 48%. `editor/personas/voxel-artist.md`
  instructs "read it before your first edit of a session" — a ~43k-token fetch to learn a glyph grid.
  It is also written verbatim into every scaffolded project as `LUDION-GUIDE.md`
  (`crates/ludion-cli/src/new.rs`).
- Audiences mixed inside single files. `ludion-cli.md` was 41 KB of which 33 KB restated `--help`;
  its one non-obvious passage (the `dev` supervisor's snapshot → shutdown → same-port relaunch →
  restore) sat at 88% depth. `ludion.md` was ~70% game-author material. `ludion-scene.md` disclaimed
  the user-facing format three times and documented it anyway.
- ~35–40 KB restated well-named code: a 1.5 KB single line re-listing `lib.rs` re-exports in
  `ludion-engine.md`, a 4 KB single line enumerating struct fields in `ludion-renderer.md`.
- 143 schedule-tense markers ("Phase 15 / E4", "trivially green today", and the pasted commit-message
  assertion "the wire surface did not grow, `schema_hash` unmoved" in four places).
- Duplication: `determinism.md`'s region-model essay duplicated `ludion-ecs.md` near-paragraph-for-
  paragraph; the voxel format and authoring ladder were documented three times.
- A gap: map authoring (`kind: map` / `kind: map-kit`) had no user-facing topic at all.

The corpus already contained the principle, in `diagnostics.md`, refusing to transcribe the code
table: *"Every phase adds rows, so a transcription here rots by construction."* No other doc applied it.

## Settled design

**Two roots, two verbs.** `docs/` (`ludion docs`) is written for building a game with Ludion and is
embedded in the binary. `dev-docs/` (`ludion devdocs`) is written for editing `crates/` and is
**checkout-only, never embedded** — outside an engine checkout every form is `L0008`. The audience
boundary is physical rather than advisory, and the crates.io tarball stops carrying engine internals.

Rejected alternatives: a shared flat namespace with an `audience:` frontmatter tag (nothing stops a
mis-tag, and the listing still mixes both); nested paths as topic names (`guide/voxel-authoring` —
costs resolver, nearest-match, and mirror changes for a purely cosmetic hierarchy).

**Hub + spokes.** A hub is what you fetch when you don't yet know what you need — scope, file map,
rules, pointers, never content; spokes carry the content. `engine-guide.md` keeps its filename as the
guide hub so `new.rs`'s `include_str!` needs no change and a scaffolded project gets an index rather
than 171 KB.

**No mechanical size gate** (user decision). Hub ≤ 4 KB / spoke ≤ 12 KB is a reviewed budget stated in
the skill and CLAUDE.md; crossing it is the signal to split.

The eight writing rules and the two section skeletons live in `.claude/skills/ludion-docs/SKILL.md`,
which is the normative copy — not duplicated here (rule 2).

## Anti-loss protocol

Three nets, the first two mechanical.

1. **Split verbatim, then condense — two commits per area.** The split commit moves text byte-for-byte
   (only heading-level shifts and frontmatter may change) and states line accounting in its message:
   `N moved + M dropped = original`. The condense commit is then reviewable *as a diff against the
   moved text*, so every judgment deletion appears as a deletion instead of hiding inside a rewrite.
2. **The ledger** — the table below. One row per source section: destination topic, or `DROP` with a
   category. Categories: **D1 duplicated** (name the owning topic), **D2 derivable** (one grep,
   `--help`, or rustdoc), **D3 schedule** (rule 5), **D4 void** (contradicted by source and the whole
   claim is dead — if merely stale, fix it), **D5 connective** (no load-bearing token, rule 4).
3. **The token census** — `scripts/doc-census.mjs`. Extracts diagnostic codes, source paths, shell
   commands, measured numbers, and headings; `--baseline` reports every token present before and
   absent now, exiting non-zero. Baseline: `Plan/artifacts/doc-census-baseline.json`, taken at W0
   close over both roots — **121 codes, 310 paths, 90 commands, 58 measures, 149 headings**. Its
   heading scan skips fenced code blocks, mirroring `extract_section` in `crates/ludion-cli/src/docs.rs`
   (engine-guide.md scans 89 headings naively vs 86 fence-aware — three YAML `#` comments).

Verified before being relied on: deleting one `L0005` reference and one measured number from a copy of
`docs/` made `--baseline` report both and exit 1.

Lore history is the fourth net, but it only helps someone who knows the text existed — the ledger is
the index into it.

## Status

**W0 shipped.** Tooling, rules, and the audience relocation.

- `ludion devdocs` added. One private `DocSet { verb, dir, embedded }` threads through
  `list`/`lookup`/`embedded_body`; `print_body`, `extract_section`, `parse_frontmatter`, `topics`,
  `find_docs_root` are shared verbatim. **An empty embedded table *is* the checkout-only rule** — no
  branch anywhere tests "is this devdocs". `find_docs_root`'s marker stays `crates/ludion-core/Cargo.toml`
  plus `docs/`; `dev-docs/` is deliberately not part of the checkout test.
- 26 engine-internals topics moved `docs/` → `dev-docs/`, verified sha1-identical to their committed
  blobs. `docs/` holds `engine-guide`, `scene-format`, `designer-flow`. `embedded-docs/` shrank 29 → 3;
  `syncEmbeddedDocs()` needed no change (it derives staleness from what is in `docs/`).
- Tests 17 → 29. `embedded_topics_match_the_docs_directory` now also gates that no dev-doc leaks into
  the binary. The pin is `devdocs_outside_a_checkout_is_l0008_for_every_form`, which loops all four
  forms over a topic that *genuinely exists* in `dev-docs/` — so a silent embedded fallback and a
  topic-shaped excuse both fail. `L0009` help names the caller's own root, so the two roots cannot
  cross-suggest.
- `L0008`'s explanation had described discovery as `CLAUDE.md` plus `docs/` — stale since the
  downstream-misfire fix, and user-facing via `ludion explain L0008`. Corrected, and extended for the
  devdocs case.
- `engine-guide.md`'s 12 bare `docs/<name>.md` cross-references became verb commands (rule 8), four of
  them gaining `--section`. Eleven now name dev-docs topics; the preamble states plainly that those
  resolve only in an engine checkout and that nothing in the guide requires them.
- `check.mjs` and `ci.yml` gained a `devdocs` smoke step. `node scripts/check.mjs` ALL GREEN, 19 gates.

**Known interim gap, closes in W2/W3.** Several relocated topics are dual-audience, so their
game-facing halves are currently unreachable from a scaffolded project: `ludion` (the facade tour),
`determinism` (the rules binding game code), `ludion-testkit` (golden tests), `ludion-ecs`
(`CommandBuffer`, since deleted), `ludion-mcp`. W2/W3 move those halves into `docs/`, at which point the guide's
`devdocs` citations become `docs` citations. Tracked here so the gap is closed deliberately rather
than rediscovered.

## Ledger

Populated per wave. W0 moved files without editing them, so it has no section-level rows; the only
content edit is the 12 cross-references and the preamble above, and no token was dropped.

### W1 split — verbatim, line accounting balanced

| source (file + lines) | destination | notes |
| --- | --- | --- |
| `docs/engine-guide.md` L293–1133 (841 lines, the `## Voxel models` chapter) | `voxel-format` (269), `voxel-authoring` (163), `voxel-craft` (119), `voxel-ops` (281) | **832 moved + 9 dropped = 841.** Verbatim except 19 lines carrying an anchor rewrite. |
| ↳ `### Live editing` (5 lines) | `voxel-authoring` `## Gotchas` | the watcher loop is part of the authoring loop |
| ↳ `### Pitfalls (voxel-specific)` (24 lines) | split 16 → `voxel-format`, 8 → `voxel-ops` | distributed by owner (rule 2), not kept as a dumping ground: `L1114`/`L1113`/`L1006` to format, `L1115` + the `L1131`/`L1132` ops-file bullet to ops |
| `dev-docs/ludion-renderer.md` (351 lines) | hub (120) + `renderer-shading` (48), `renderer-ibl` (63), `renderer-medium` (52), `renderer-shaders` (98) | **351 moved + 0 dropped.** Byte-compared per destination against the source range. |

**DROP rows (W1): 9 lines, all D5 structural** — the chapter's own `## Voxel models` H2 (its text is retained as the guide's pointer-section heading, so the `#voxel-models` anchor and the census heading token both survive) and 8 blank seam/bracket lines.

### W2 split — the guide becomes an index hub

**2130 moved + 67 dropped + 38 remaining = 2235.** Independently checked against the committed file: 2107 lines byte-identical, 31 heading-rank shifts, 19 chapter `##`→`#` promotions, 13 anchor rewrites, 0 unexplained.

19 chapters → 18 topics: `project-anatomy`, `components`, `tilemaps`, `data-docs`, `player-profiles`, `scene-transitions`, `materials`, `lighting` (Sun and shadows + Environment & sky), `post-processing`, `systems`, `collision`, `screen-ui`, `audio`, `verify-loop`, `running`, `dev-loop`, `getting-started`, `recipes`, `wire-protocol`. **`docs/engine-guide.md` is now 3984 B** — inside the index-hub budget, and that is what `ludion new` writes into a scaffolded project instead of 171 KB.

**DROP rows (67 lines):** the 24-line `](#anchor)` table of contents (D2 — every entry pointed at a chapter no longer in the file; superseded by the routing list under rule 8); the 35-line `## Scenes` chapter (D1 — `scene-format` owns the document skeleton, `L1002`–`L1010`, canonical floats and registry-key case), **except** its `How paths resolve` paragraph (the content-root rule and the doubled-`scenes/scenes/…` trap), which `scene-format` lacked and which moved there verbatim; the `## Pitfalls` heading (rule 2); a frontmatter description and 6 structural blanks (D5); and 3 lines of "everything you need … every code block below is real" (D4 — false of an index).

`## Pitfalls` distributed by owner rather than kept as a dumping ground: four determinism/schedule bullets → `systems`, the stale-`EntityId`/`L0002` bullet → `recipes` (the spawn/despawn recipe owns handles), the pinned-GPU/SSIM/`L2205` bullet → `verify-loop`, `check-scene` is built-in-only → `scene-format`, `replay` is built-in-only → `running`.

**New justified misses (2), both headings:** `Pitfalls (the honest gotchas)` (rule 2, distributed — exact precedent already ledgered for `Pitfalls (voxel-specific)`) and `Scenes` (D1, a stub deferring to the topic that owns the fact). No code, path, command or measured number was lost. Cross-file anchors after the carve: **zero** — the 6 remaining `](#` are all same-file.

Three `ludion docs engine-guide --section …` calls in the voxel topics were repointed at `verify-loop` and `materials`; those tokens post-date the baseline, so they raise no miss.

**Two topics over the 12 KB spoke budget, deliberately not split in a verbatim pass** — `post-processing` 16.3 KB (seam: `## Fog and light as one medium`, ~5 KB, the participating-medium model) and `lighting` 14.1 KB (carries two chapters by design; seams are `## Sun and shadows` or the independent `Fade`/occlusion section). W3's call.

### W2 condense + W3 — the dev-docs mass, and the last gap filled

**Game-facing (21 topics, all ≤ 12 KB; hub 4182 B).** Both mandated splits proved necessary — condensing alone left `post-processing` at ~14 KB and `lighting` marginal at ~12.5 KB, so the breach was a structural signal, not padding: `atmosphere` (the participating medium — lighting physics that happens to be configured through `PostProcess` fields) and `environment` (sky + IBL) are now their own topics, leaving `lighting` a tight 5.1 KB sun-and-shadows topic. `Fade` went to `environment` as a stated judgment call: it is orthogonal to both parents, and both are per-scene opt-in components shaping how a scene is *seen*. **Cross-file prose pointers: 0** (the 7 known plus 2 the grep found).

**Engine-internals: 182.8 KB → 137.1 KB, including an 11.3 KB brand-new topic.** Deepest single fetch 41.6 KB → 19.5 KB. `ludion-cli` 41.2 → 14.6 (26 KB of `--help` catalog gone), `ludion` 18.9 → 10.1 (13 KB of engine-user material), `ludion-engine` 41.6 → 16.8 + 3 spokes, `determinism` 26.0 → 12.3, `ludion-scene` 34.1 → 10.1 + 1 spoke. New spokes: `engine-replay`, `engine-content-kinds`, `engine-image-paths`, `sim-parallelism`, `scene-kind-documents`.

**The map-authoring gap is closed** — `docs/map-authoring.md` (11.3 KB), written against `generate-map`/`map-check` and the committed `kind: map` sources, not moved.

**Deletion under rule 2 was verified fact by fact, not assumed.** 21 sections were deleted from `ludion-cli`/`ludion` only after naming the topic that owns each (`explain` → `verify-loop`, `serve` → `running`, the voxel verbs → `voxel-ops`, `import` → `ludion-import`, and so on). Cross-file duplication resolved: `determinism`'s region-model essay now belongs to `ludion-ecs` (stub + pointer), its four feature blocks to `engine-content-kinds`, and the twice-pasted "the wire surface did not grow, `schema_hash` unmoved" is gone corpus-wide.

**One uncovered fact was found and NOT dropped** — `--replay-verify` as a user-facing flag (when to run it, that it needs no GPU/window/server, its conflict set). No `docs/` topic owned it; it was left in place and reported, and now lives in `docs/verify-loop.md`. This is the protocol working: the instruction was *report, do not delete*, and the census was the backstop had it been dropped anyway.

**Corrected against source, not preserved:** `determinism`'s "No sim system calls `ludion_core::par` … so this is trivially green today" was false — `enemy_ai` runs one `par_for_each` region at `ENEMY_AI_CHUNK_ROWS = 32` and `move_projectiles` holds the call shape at `usize::MAX`, deliberately sequential. Literal `sim-hash` reference hashes were replaced by the commands that reproduce them.

**New justified misses (3), all D1:** `examples/player_profile.rs` and `examples/scene_transition.rs` (bare second addresses; the canonical `crates/ludion/examples/…` paths are cited in both the dev-doc and the game-facing topic — **verified**), and `node gen.mjs | ludion voxel from-json --validate` (`voxel-ops` carries the identical one-pipe form with a real script path). `games/hello-paddle` briefly vanished on a trailing-slash change and was restored. **Running total: 20 misses, all triaged, zero unexplained.**

**Rule 3 refined by doing it.** "Recoverable in one grep" is measured *from the reader*, so the rule bites harder in `dev-docs/` than in `docs/`: a dev-doc's reader has `crates/` open, while a guide topic's reader may be in a scaffolded project with no engine source on disk, where a field table is not a transcription of something reachable — it is the only copy. `components.md`'s table and `post-processing.md`'s roster were kept on that basis (every row carries a default, a range, or an `L####`). The skill now states this.

**Over-budget residuals, each with arithmetic and a named seam** — `ludion-ecs` 19.5 KB (irreducible 8.1 KB before Dataflow; seam `ecs-queries` ~7 KB), `ludion-engine` 16.8 KB (Gotchas 4.8 kept whole by instruction), `ludion-cli` 14.6 KB (13.8 KB of it named must-stay), `scene-kind-documents` 12.7 KB. None was met by cutting protected content.

### W1 condense — rules 3, 4, 5 applied; every deletion visible as a diff against the split

| file | after split | after condense | what went |
| --- | --- | --- | --- |
| `dev-docs/ludion-renderer.md` | 30.2 KB | **12.1 KB** | ~7.5 KB of `Layout` (a single ~4 KB line transcribing `render.rs`'s registries, `lib.rs`'s re-export list, the 19-shader per-file catalog), ~3.4 KB of pass state recoverable from `record_*`, ~1 KB of rule-5 tense; the pass walk moved to a sixth topic |
| `dev-docs/renderer-passes.md` | — | 4.2 KB | the per-frame pass walk + the composite chain, moved verbatim (byte-compared), the pinned order's scene-referred/display-referred rationale intact |
| `renderer-ibl` / `-shading` / `-medium` / `-shaders` | 20.6 KB | 19.6 KB | transcribed `const` values replaced by a pointer to `ibl.rs`; per-snippet consumer lists (derivable from `COMPOSITIONS`). `renderer-medium` lost ~100 B — every bullet is an invariant with a consequence, and it is the one file where two shaders must agree |
| `docs/voxel-format.md` | 13.9 KB | **11.5 KB** | an orientation worked example folded into the rule it restates; `--rig` prose that `voxel-authoring` owns (rule 2), reduced to a pointer keeping the 2–3× number |
| `docs/voxel-ops.md` | 15.4 KB | **11.9 KB** | the JSON-intermediate block that `ludion explain L1133` prints; the 20-line generator example duplicating the runnable `examples/voxel-gen/brazier.voxel.gen.mjs`; per-verb flag restatement |
| `docs/voxel-authoring.md` | 11.6 KB | 11.1 KB | structural only — `L1117`/mutual-exclusion/`MSYS_NO_PATHCONV` promoted into a real `## Gotchas` |
| `docs/voxel-craft.md` | 9.3 KB | 9.4 KB | **nothing.** Every measured pixel count, the contact-sheet arithmetic, and all three incidents kept; openers reworded only |

Result: the renderer's map costs 12.4 KB instead of a 48.8 KB fetch, deepest subject 7.1 KB; a voxel artist fetches 9.4 KB of craft instead of 171 KB of guide.

**Budget correction (settled in W1).** `hub ≤ 4 KB` was written for the guide-side case and wrongly generalised. A **crate hub** also carries that crate's `Layout`/`Rules`/`Gotchas`/`Verify` — content with no other owner, ~6 KB irreducible — so it is budgeted as a spoke at ≤ 12 KB; an **index hub** (pure routing) stays ≤ 4 KB. The breach still forced a split rather than a waiver: the pass walk became `renderer-passes`, landing the hub at 12.1 KB (143 B over, accepted rather than cutting one of eight invariants).

### Census: the baseline was wrong, and the fix found what it had missed

The W0 baseline was generated **after** W0's edits and compared against itself, so it reported zero misses while ~10 tokens had in fact changed. A net that only validates changes made after it was strung up is not a net. The baseline is now regenerated from the **pre-arc commit `11fefcf`**, and `PATH_RE` widened to track `Plan/` citations — a doc citing a `Plan/` file is citing the decision record for a rule that otherwise reads as an arbitrary omission, exactly what a condensing pass drops without noticing. That added 12 previously-invisible protected tokens.

**Justified misses through W1 — 15, every one triaged, zero unexplained:**

| tokens | rule | justification |
| --- | --- | --- |
| 11 bare `docs/<name>.md` paths (`ludion`, `determinism`, `ludion-scene`, `ludion-renderer`, `ludion-sim`, `ludion-testkit`, `ludion-mcp`, `ludion-ecs`, `protocol-evolution`, `scene-format`, `designer-flow`) | 8 | rewritten as verb commands. **Verified**: each has ≥1 live command citation and an existing target file — the fact survived, only its address changed |
| `ludion devdocs ludion-renderer --section "Image-based lighting (IBL)"` | 8 | that section became a whole spoke; the guide cites `renderer-ibl` |
| `0.125×` | 3 | `bloom_down`'s shader weights, one grep of `bloom_down.wgsl`. The load-bearing half (the Karis average on `bloom_down_1` **and only it**) is kept in full |
| `The format`, `The author → check → render loop` | — | skeleton conformance renames (`## Format`, `## The loop`); bodies unchanged |
| `Pitfalls (voxel-specific)` | 2 | a subheading redundant with its own `## Gotchas` parent; all five bullets kept |

The baseline is **immutable from here**. `--baseline` therefore reports the accumulated justified misses on every run: the check is not "exit 0" but **"the reported set equals the table above."** Any token reported that is not listed is a regression.

### Known rule-1 violation, deferred with its seam identified

`dev-docs/renderer-shaders.md`'s hot-reload section is still ~40% engine-*user* material. It was kept rather than cut (deleting it would take `L2008`, `L2009`, `crates/ludion-server/tests/shader_reload.rs` and three commands with it). W2/W3 relocates to `docs/`: the three ways to set the dev flag, the `L2008` message, the keep-last-good promise as a user guarantee, and the recursive-watch paragraph ("sub-second edit→pixels against the ~3.1 s release rebuild path"). Staying engine-internal: naga re-validation, the module/pipeline rebuild rule, snippet transactionality, and "render-side only, no `WorldHash` interaction".

Sizes after the split: `engine-guide.md` 171.5 KB → 125.6 KB (3035 → 2234 lines) and it gained the 24-entry `## Contents` table it never had; `ludion-renderer.md` 48.8 KB → 30.2 KB. All four renderer spokes are inside the 12 KB budget; `voxel-format` (13.9 KB) and `voxel-ops` (15.4 KB) are over it, and the renderer hub is 30.2 KB — all three are the condense pass's job, not failures of the split.

## The W1 proof — two fresh instances, neither told the arc existed

Each got a real task and only the CLI. Measured: topics fetched, whether it fell back to source, what it paid for and didn't use.

**Voxel authoring** (author a lantern prop that reads at game scale) — fetched `voxel-format`, `voxel-craft`, `voxel-authoring`, `voxel-ops`: **~44 KB against the 171 KB the guide used to cost**, and the persona's old route was a `--section` call it had to be hand-taught. Every schema rule, orientation rule, check command, craft number and `L####` came from the docs; **no source read was needed for any rule.** It read two `.voxel.yaml` files for something the docs deliberately don't carry — a concrete starting value and a worked exemplar — which is rules-not-exemplars working as designed, not a gap.

**Renderer internals** (add a chromatic-aberration pass) — fetched `renderer-passes`, `renderer-shaders`, `ludion-render-graph`, `ludion-renderer`, `ludion-testkit`, `diagnostics`. **The docs stopped it building a duplicate of a shipped feature**: `renderer-passes` told it chromatic aberration already exists, and it then found the literal request contradicts `Plan/post-process-pipeline.md` decision 2, which names CA as an example of the folds-into-composite category and rejects every-effect-its-own-pass. It called the `docs`/`devdocs` split a clean help — it could rule out the entire game-facing set from the one-line descriptions without spending a fetch.

**Verdict: the shape works. Ship it across W2/W3.**

### What the proof found that the arc has not fixed

1. **No devdoc covers the budget-file format** (`budgets/*.toml`: `[passes]` vs `[post_groups]`, the calibration method). The instance read `budgets/pinned-apple-m4.toml` directly. A missing topic, not a condensing failure — W3.
2. **The editor's effect chain (`editor/app/src/editor/art/chain.ts`) is undocumented in either set**, including what its `stage` values mean; a renderer effect must add a row there. W3.
3. **A load-bearing fact lived only in a `Plan/` file** — the decision log that makes the composite-folding rule intelligible. This is exactly why `PATH_RE` now tracks `Plan/` citations, and it argues those citations are load-bearing under rule 4, not decoration.
4. **`renderer-passes` and `ludion-renderer` both describe the frame graph** (per-pass semantics vs which-pass-exists-when), so one mental model costs two fetches. The cross-references work; watch whether it recurs before treating it as a defect.
5. Pass counts in `ludion-renderer`'s Verify block are content-derived and were presented as if fixed — now annotated as moving when a conditional pass ships, with the command named as the source of truth.

### Content defects the proof surfaced (not documentation)

- **`examples/gallery/voxels/lantern.voxel.yaml` violates the emissive rule** the craft topic states: its `G` glyph is warm orange (1.0, 0.72, 0.3) with no `emissive:` field — an unlit colour pretending to glow. **Recorded, deliberately not fixed** (user decision): fixing it makes bloom appear, moves a committed gallery golden, and drags a re-bless into a docs arc.
- **The density convention is contradicted by shipped content.** `voxel-format` states 16 voxels/world unit (`scale: 0.0625`) for standard props and cites "a 1-unit crate spans 16 cells" — but `crate.voxel.yaml` is 4³ at `scale: 0.5`, and no shipped model uses 0.0625 (real scales run 0.05–1.0). An authoring agent trusting the doc produces models ~8× denser than the game's. **Open — the value is not settled and the docs are deliberately left unchanged until it is.** Whoever settles it owns the `voxel-format` edit and should also drop the crate as its example.

## Remaining waves

- **W1 — pilot, both skeletons.** Guide: the 47.6 KB voxel chapter → `voxel-format` (L300–547),
  `voxel-authoring` (L548–706), `voxel-ops` (L827–1100), `voxel-craft` (L707–826); `engine-guide.md`
  becomes the hub. Dev: `ludion-renderer.md` → hub + `renderer-shading` / `renderer-ibl` /
  `renderer-medium` / `renderer-shaders`. Then the fresh-agent proof, before the bulk migration.
  `editor/personas/voxel-artist.md` currently calls
  `ludion docs engine-guide --section "Craft — what makes a model read"` — repoint it to `voxel-craft`
  in the same wave, or the call breaks.
- **W2 — the rest of the guide** → ~20 task topics; fill the map-authoring gap; distribute
  `## Pitfalls` into each topic's `## Gotchas` (rule 2) rather than keeping a dumping ground.
- **W3 — condense `dev-docs/`**, hub + spokes, applying rules 3 and 5 to the (b)/(d) mass named above.
- **W4 — sweep**: remaining cross-references, `Plan/*.md` doc-topic mentions, final census, the proof
  re-run.

## Acceptance

Mechanical: `check.mjs` green per wave and `--full` at close; census balanced or ledger-rowed; line
accounting balanced in every split commit; `checkout_and_embedded_lookup_agree_byte_for_byte` green.

The proof: a fresh instance given only the CLI, on three tasks — author a voxel prop that reads at game
scale; add a render pass; add a CLI subcommand. Recorded per task: **bytes of docs pulled**, whether it
fell back to reading source anyway, and whether it succeeded. Run on the W1 pilot and again at close.

Headline to beat: a voxel-authoring session costs 171 KB today; target is `voxel-format` plus
`voxel-craft` ≈ 15 KB, and a scaffolded `LUDION-GUIDE.md` of ~4 KB instead of 171 KB.
