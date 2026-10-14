# Plan: Voxel Tooling Ergonomics — Round 2

> Status: **SHIPPED (2026-08-02, four waves).** Waves 1+3 landed at Lore rev 461, wave 2 at rev 462,
> wave 4 (this doc, plus `docs --section`, the guide sweep, and this Plan doc) closes the arc. Every
> item below is code, not a proposal. Acceptance: the statue model's contact sheet under the shipped
> night rig went mean luma 138.4 → 65.0 with `clipped_low` 0.0000 — a night read, not a crush — and
> `node scripts/check.mjs` is green across every wave (18 gates at wave 2, including the new
> `voxel-kit` parity lane). Two items are explicitly **not** built, with their trigger recorded rather
> than actioned (see "Deferred" below); one content follow-up rides another task (see "Follow-ups").

## Trigger

`Plan/voxel-authoring-tools.md` shipped the write side (`voxel new/apply/upscale/diff/from-json`,
`scripts/voxel/grid.mjs`, the authoring ladder) in the same arc that closed 2026-08-01. Its own status
note flagged an open THE-test: whether a fresh instance pointed at "make a 60³ creature" actually
finds and uses that ladder without hand-holding. Round 2's trigger is that test run for real — four
models (Qwen 3.6, Sonnet 5, Opus, Fable) independently authored a `statue-*.voxel.yaml` variant from
the same brief, each keeping a friction report, consolidated at
`Plan/artifacts/voxel-pipeline-debug-report.md`. The tooling worked — every model shipped a
correct, connected model using only the documented verbs — but two costs repeated across reports and
are the reason this arc exists rather than being filed as noise:

- **Qwen 3.6 burned six regeneration cycles and ~45 minutes** chasing a disconnected-limb warning
  that named a sample cell in the stray hand (`(2,0,1)`) while the actual break was a shoulder gap
  **19 layers away**. The diagnostic named the symptom, never the cause — nothing in `L1129` pointed
  at *where* the two pieces failed to meet.
- **Three of the four models could not reach the shipped night rig agent-side.** `render --model`'s
  contact sheet renders under a synthesized bright-neutral key + fill; the engine guide's own
  calibration paragraph asked authors to eyeball their palette ~2–3× darker than the target read as a
  workaround. Opus spent roughly half its total session hand-building a scratch rig by copying
  `Environment`/`PostProcess`/moon entities out of `env-night.yaml` by hand — and mis-set them twice
  (wrong moon-fill position, wrong ground albedo) before the render told it anything about the model.
  Fable hit the same wall and named it directly: *"the CLI can't render it — so value verification
  still requires you at the stage."* Only Sonnet reached a real night preview, and only via a
  `render --scene model-stage-night.yaml --set` reconstruction that required knowing the file existed
  and manually indexing its entity array — plus a Git-Bash path-mangling gotcha
  (`MSYS_NO_PATHCONV=1`) that cost it a failed call along the way.

Every other finding in the four reports — `set` with no `mode: paint`, the silent mirror-discard
footgun, no exposure/visibility query, the doc's engine guide too long to read whole, the diagonal-line
connectivity trap undocumented at the op itself — is real but cheaper; this arc's four waves work
through the full ranked list, not just the top two.

## What shipped, by wave

### Waves 1 + 3 (Lore rev 461) — the diagnostic, and the rig

**`L1129` now reports the gap, not just the orphan.** The flood fill keeps every component's cells
(bounded by the 64³ budget, transient), then one multi-source BFS seeded from the whole main body
sweeps the grid — empty cells included — so every stray piece's nearest-pair answer falls out of a
single O(cells) pass rather than a pairwise scan; on the grid, BFS distance *is* Manhattan distance,
and each stray's minimum is taken on the explicit `(distance, cell)` key so ties never depend on
traversal order. Each warning line now carries the stray's size, a sample cell, the nearest pair of
cells between it and the main body, and the bridge-cell count that would close the gap. Qwen 3.6's
motivating case — a shoulder gap 19 layers from the reported hand cell — collapses to one line of the
warning instead of a hand-written flood-fill script. The help text for `line` names the diagonal-step
trap outright (an edge-touching cell looks continuous in a slice and is not connected) — Sonnet's
finding, fixed at the source instead of left for the next model to rediscover by reasoning backward
from a generic `L1129`.

**`check-scene --exposure`** is the fourth voxel view: filled/surface/enclosed totals, per-glyph
enclosed-cell counts with samples, and each emissive glyph's exposed faces in the fixed
`+X -X +Y -Y +Z -Z` order. It answers, as text, the two defects that previously cost a GPU render to
notice — interior detail cells nobody will ever see, and the lightbar bug where a glow's faces all
point one direction (Opus's mould crest, which read as a green lightbar from the game camera because
12 of its emissive cells all faced +Y). `voxel apply` gained an enclosed-cells footer line for the
same reason — Fable's "no visibility query" finding, answered without a new verb.

**`render --model --rig <scene>`** lets the contact sheet borrow a real scene's lighting and grade
instead of its synthesized key + fill — closing the gap three of four reports named. The lift is a
keep-list, not a drop-list: every rig entity carrying `Environment` / `PostProcess` /
`DirectionalLight` / `PointLight` is copied whole and re-emitted through the canonical writer (no
second YAML emitter); a lifted light replaces the key + fill outright; an entity named `ground`
carrying a `Mesh` donates its color to the sheet's floor. Zero liftable entities is `L1117`; an
invalid rig reports its own scene diagnostics verbatim. With `--rig` absent the synthesized sheet is
byte-identical to before, pinned by test. The scale tension between model-authoring scale and
game-world scale is documented rather than hidden: `Environment`/`DirectionalLight` are
position-independent so `env-night.yaml` reads true at any scale, but `PointLight` ranges and
`PostProcess`'s distance-coupled terms were tuned at game scale — the recorded fallback (drop
`PostProcess` from the keep-list) rides the first sheet that needs it. The engine guide's palette
section now leads with `--rig`, demoting the 2–3× arithmetic to an explicit fallback, and documents
the absolute-fidelity recipe that already existed but nobody could find (`render --scene
model-stage-night.yaml --set` with the entity index verified by running it, and the
`MSYS_NO_PATHCONV` Git-Bash gotcha named). The voxel-artist persona was updated: the agent self-serves
night reads now, the human stage toggle stays for final judgement.

### Wave 2 (Lore rev 462) — the generator loop, and the ops engine

**`from-json` gained `--validate`**, the third of exactly-one-of `--out`/`--check`/`--validate`: parse
the intermediate, emit canonically in memory, run the same validation the write path runs
(`L1115`/the gap-reporting `L1129`/the exposure note), write nothing. A generator's whole verify loop
is now one pipe — `node gen.mjs | ludion voxel from-json --validate` — where the friction report
counted three commands and a ~15-second regeneration cycle per hypothesis. `--check` (the drift
gate's path) now prints those same warnings before its byte-compare instead of early-returning past
them; its exit code stays purely drift-governed.

**`set` gained `mode: paint`** (Fable's workaround-with-single-cell-`box` finding, fixed at the op
that should have had it from the start), and the silent mirror-discard footgun is now **`L1134`
`VOXEL_OPS_SHADOWED_BY_MIRROR`** — a warning naming both ends: which mirror op overwrote how many
cells, and which earlier op wrote them, in which discarded half. Provenance rides a parallel writers
array through one `write_cell` mutator, carried intact through `shift`/`grow`/`crop` remaps, and
tallied per `(mirror, writer)` pair so a shape losing dozens of cells to one mirror is one finding,
not dozens — Opus's finding, fixed at the engine rather than documented as a thing to remember.

**`scripts/voxel/grid.mjs` grew a parity-gated inspection layer**: `components()` (the JS twin of the
Rust connectivity checker, matched on every observable — scan order, face-only adjacency, the
size-then-sample sort), `assertConnected()` (the pre-emit one-liner that turns a disconnected model
into a build break instead of a round trip through `--validate`), and `ellipseCells()` (the integer
footprint a cross-section rasterizes to — the shoulder-gap question that cost Qwen 3.6 45 minutes,
answerable now without regenerating). Parity is a **gate**, not a promise: `grid.test.mjs` and
`crates/ludion-scene/tests/voxel_edit.rs` pin the identical fixture, each pointing at the other, wired
as the `voxel-kit` lane in `check.mjs` and as a CI step.

### Wave 4 (this wave) — the doc-length gap, and the sweep

**`ludion docs <topic> --section <heading>`** — Opus's fifth-ranked finding: the engine guide runs to
thousands of lines and one report resorted to `--path` + `sed` to read three sections by hand. The new
flag prints the text under the first ATX heading whose trimmed text matches (case-insensitively),
through everything up to the next heading of equal or higher level; fenced code blocks are excluded
from heading detection, since the guide's own YAML examples carry `#`-prefixed comments that would
otherwise misread as sections. A miss is a new registered code, **`L0015`
`DOCS_UNKNOWN_SECTION`**, with a nearest-match suggestion and the topic's top-level headings listed —
applied identically whether the doc text came off disk or the embedded copy, matching the invariant
`ludion docs` already held for the whole-doc case.

**The guide swept for the surface the first three waves shipped without a reader-facing sentence**:
the authoring ladder's generator rung now names the one-pipe `--validate` loop and the pre-emit
`assertConnected`/`ellipseCells` helpers; the ops-file bullets gained a `mirror` entry documenting
`L1134` and a one-line cross-reference from `line` to the diagonal-step trap already covered in the
connectivity section; `docs/ludion-cli.md` documents `--section` on the `docs` verb and fills in
`from-json --validate`, which had never been added when the flag shipped; `docs/ludion-scene.md`'s
`VoxelEdit` bullet was missing the `warnings` field (`L1134`'s carrier) entirely. `--exposure` and
`mode: paint` were checked and found already covered from waves 1–2 — nothing to add there.

## Deferred, with recorded triggers (do not build)

- **`--sweep` reaching a voxel model's palette.** Opus's second-ranked finding: `--sweep` is
  scene-JSON-pointer only, so the one calibration loop every report named as the recurring
  project-wide cost — palette value A/B — is the one thing it can't drive; Opus worked around it by
  copying the model to scratch and patching it with Python. **Trigger to build:** a real calibration
  task where `--rig`'s relative A/B (render the same model twice, eyeball the difference) is
  insufficient and an author genuinely needs `--sweep`'s side-by-side grid over palette values. This
  needs asset-doc override machinery `--sweep` does not have today (its overrides resolve against a
  scene document, not a model's `palette:` map) — real design work, not a flag addition, so it waits
  for a task that actually needs it rather than being built speculatively.
- **An angled/capsule limb primitive.** Sonnet's finding: only an axis-aligned `cylinder` exists, so a
  diagonally-braced limb has to be faked with `ellipsoid`+`cylinder` pairs — workable for the blocky
  genre this arc targets, but a real expressiveness gap. **Trigger to build:** a second model
  independently hitting the same fake-a-diagonal-limb wall — one report naming a workaround is a data
  point, not yet a pattern.
- **Colored/highlighted slice output.** Not named directly in the four reports, but implied by the
  same category of friction the ruler and `--exposure` already answered (column-counting by eye).
  **Trigger to build:** another column-counting friction report landing *after* both the ruler and
  `--exposure` are in an author's hands — if the two shipped answers don't retire this class of
  complaint, the gap is real; building it ahead of that evidence would be guessing at which columns
  need highlighting and why.

## Follow-ups riding other owners

- **`statue.voxel.yaml`'s palette recalibration.** Two of four reports (Sonnet, confirmed
  independently by rendering the shipped base model through the same rig) found its flesh/claw/mould
  values read near-white under the real night rig — 4–10× above the guide's near-black band. This is
  now judgeable agent-side via `--rig` (it wasn't, before this arc), but it is a **content** change
  that re-blesses the `district-plaza` golden, not a tooling change, and is scheduled as its own task
  per user decision rather than folded into this arc.
- **The TeamCity CI mirror wants the `voxel-kit` step.** `ci/teamcity/README.md` is held by another
  instance's uncommitted edits (per the shared-checkout convention, this arc does not touch it); the
  step itself is already wired into `.github/workflows/ci.yml` and `scripts/check.mjs`, so the
  TeamCity side is a mechanical port whenever that instance's edits land.
