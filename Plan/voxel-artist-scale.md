# Voxel Artist at scale — fixing the `house-c` collapse

**Status: EXECUTED (2026-08-06, Lore revs 485–487). See Results at the end of this file.**

## Context

A planning note outside the repo diagnosed why selecting
`house-c.voxel.yaml` (128×192×112 = 2.75M cells, 2,950,367 bytes, `budget_exempt: true`) makes the
Voxel Artist view collapse. This plan is the answer to that diagnosis. Every claim in it was
re-verified against the tree on 2026-08-05 before this plan was written:

- `Grid::from_doc` (`voxel_mesh.rs:82-97`) calls `VoxelDocument::palette_index` per cell, whose
  `row.chars().nth(x)` (`voxel.rs:216`) is O(x) — ~175M char steps per grid build, quadratic in
  width. Confirmed, including the arithmetic (84× the cells of a 32³ model, 344× the char steps).
  The module's own doc comment (`voxel_mesh.rs:69-73`) states the grid exists to avoid exactly this.
- The mesher itself is linear and produces a small mesh (~27k quads here). Confirmed not the problem.
- `parseVoxelDoc` re-runs on **every edit** (`useVoxelEdit.ts:201`), `readScalar` builds each quoted
  row `text += ch` one char at a time (`doc.ts:421-452`), and the undo stack retains up to 64 full
  2.95 MB sources (`paint.ts:184-201`). Confirmed.
- The slice painter is 14,336 unkeyed-work `<span>`s rebuilt on every parent render
  (`SlicePainterPanel.tsx:283-299`), and **no `React.memo`/`useDeferredValue`/`startTransition`
  exists anywhere in `editor/app/src`** (grep: zero hits). Confirmed.
- The view re-renders at idle: 4 Hz fps status (`useFrameStream.ts:42,145` → App state), unthrottled
  `setPose` (`VoxelArtistView.tsx:395-397` — only the wire write is throttled at
  `CAMERA_SEND_MS = 60`), 20 Hz turntable (`TURNTABLE_TICK_MS = 50`), and `slot` rebuilt with fresh
  identity every render (`VoxelArtistView.tsx:693-699`, no `useMemo`) churning `StageViewport`'s
  `ResizeObserver`. All confirmed.
- `check-scene` walks the grid ~5× per invocation (`check_scene.rs:135-179`: connectivity warning
  builds `Occupancy` + flood fill + `component_gaps`, then `component_count()` rebuilds it all, then
  `filled_count()` **twice** at :174/:175). Confirmed.
- The stage engine and `check-scene` run via `cargo run` with no `--release` (`stage.rs:129-141`);
  workspace crates sit at `[profile.dev] opt-level = 1` with debug assertions on (`Cargo.toml:138-145`).
  Confirmed — a constant multiplier on the quadratic, not the cause.
- `extract.rs:87` `.cloned()`s the cached voxel `MeshData` (~6.3 MB) every extract. Confirmed.

The ordering below is the diagnosis's own ranking: the super-linear term first (it taxes every
select, save, palette-drag tick and persona edit), then the idle burn, then the per-edit cost, then
the amplifiers. Constraints that bind every wave: **wire surface stays 27** (no `voxel.*` commands —
the standing private voxel-engine rule), no golden or `run.lrp` moves (the mesher's emitted
stream must stay byte-identical), sim-crate determinism denylists hold (no std `HashMap`), and
`check-scene`'s stdout stays byte-stable (the editor's `parseCheckScene` reads it).

## Wave 1 — engine: linear grid decode (the scaling bug)

The fix is one shape: **decode each row string once, left to right, through a prebuilt glyph
lookup** — never `chars().nth(x)` per cell, never a palette scan per glyph.

- Glyphs are single printable-ASCII chars by format rule (`0x21..=0x7E`), so the lookup is a
  `[Option<u8>; 128]` array built once from the palette — no `HashMap`, no denylist question,
  branchless per cell. Non-ASCII (only reachable in malformed docs) falls back to `None`.
- Add the row-decode to `ludion-scene` (e.g. `VoxelDocument::decode_rows` / a `GlyphLookup` type)
  and rewrite `Grid::from_doc` over it: one pass per row, O(cells) total. Update the `Grid` doc
  comment — after this, the grid is a layout convenience, not the quadratic-avoidance device.
- Route the other whole-grid walkers through the same lookup: `filled_count` (currently a palette
  scan per glyph, ~33M compares here), `component_count`/`Occupancy`, `component_gaps`, and
  `validate`'s per-char `Vec<char>::contains`.
- **Correctness pin:** a test asserting the decoded grid equals the per-cell `palette_index` result
  cell-for-cell on committed models plus one synthetic wide model; the existing pinned mesh fixtures
  already pin the emitted vertex stream byte-identical.

Exit: `ludion check-scene house-c.voxel.yaml` and a stage `asset.reload` timed before/after; the
cost ratio vs a 48³ model should now track the **cell** ratio, not width². This one wave removes the
super-linear term from select, save, palette drag, and every persona edit at once.

## Wave 2 — `check-scene`: one grid, many checks

Compute `filled_count()` once into a local; build `Occupancy` once and share it between
`voxel_connectivity_warning` and `component_count` (restructure the two `ludion-scene` helpers to
accept a prebuilt occupancy, keeping the public convenience wrappers). Stdout must remain
byte-identical — the editor's readout parser and any scripts depend on it. Exit: ~5 whole-grid
passes → ~2 (decode + flood fill), verified by timing.

## Wave 3 — frontend: render isolation (the idle burn)

The painter grid must re-render only when the **document layer** it shows changes, never because
camera/fps state moved. In dependency order:

- `useMemo` the `slot` object in `VoxelArtistView` — fixes the per-render `ResizeObserver`
  teardown/recreate in `StageViewport` for free.
- Wrap `SlicePainterPanel` in `React.memo` and memoize the span grid itself (`useMemo` keyed on
  rows, palette-derived cell views, layer `y`, onion state, active glyph). Precompute the 12
  per-palette-entry swatch values (`swatchCss`/`swatchIsDark`) once per palette, not per cell.
  All callbacks are already `useCallback`; the audit item is prop identity.
- Throttle `setPose` to the same `CAMERA_SEND_MS` cadence as the wire write (state and wire were
  deliberately split; the split stays, only the state-set rate joins the throttle), forced on
  gesture end so the resting pose is exact — the wire path's own posture.
- Stop the 4 Hz App-wide cascade: fps/status leaves App-level `useState`. Smallest honest form: a
  tiny external status store (set from `useFrameStream`, subscribed by `StatusBar` alone), so a
  wobbling fps number re-renders one status segment, not the app. This fixes the same idle burn in
  every view, not just Voxel Artist.

Exit (joins the next Tauri sitting, since nothing here renders under the gates): a devtools
performance profile with `house-c` selected and **no input** shows no periodic `SlicePainterPanel`
render; orbit drag and turntable no longer rebuild the grid. The diagnosis's own falsifiable check
stands: collapsing the painter via the maximize toggle should NOT help (it stays mounted under
`VerticalSplit`), while these changes should.

## Wave 4 — frontend: incremental edits (the paint cost)

- **Fast path:** a paint stroke's splices are equal-length single-char replacements inside slice
  rows. Patch the working `VoxelDocument` in place — replace the char in the affected row string(s)
  and splice the source string — instead of `parseVoxelDoc(fullSource)`. Anything not
  length-preserving (palette edits, `addGlyph`, header changes) keeps the full reparse; it is rare
  and correct. Property test: the patched doc byte-equals `parseVoxelDoc(applied(source, splices))`
  across all committed models.
- **Undo stores splices, not sources:** an entry becomes the inverse splice list (offset, removed,
  inserted) of the gesture it undoes. `UNDO_LIMIT` stays 64; retained memory drops from up to
  ~190 MB to kilobytes. Undo/redo applies the inverse through the same fast path.
- **Cheapen the full parse** for the paths that keep it (selection, external change, undo fallback):
  `readScalar`'s quoted branch scans for the closing quote/escape and appends **slices**, not chars;
  `splitLines` finds the first non-space index instead of allocating a `trimStart()` temp per line.

Exit: painting a cell on `house-c` costs one small splice + one span-grid layer update; a paint drag
holds frame rate in the sitting.

## Wave 5 — amplifiers (measurement-gated, after re-timing on waves 1–4)

- **`extract.rs` mesh clone:** stop `.cloned()`-ing ~6.3 MB per streamed frame — cache
  `Arc<MeshData>` (or hand extract a borrow) through the `SceneAssets` seam. Mechanical but touches
  a trait; do it once the big terms are gone so its effect is measurable.
- **`--release` for the stage engine + `check-scene`** (`stage.rs::cli_argv`): only if the re-timing
  still shows noticeable load/check latency at opt-level 1. Costs a one-time cold release build on
  first view entry (the readiness-is-an-event design already tolerates compiles); buys the constant
  factor plus overflow-checks off.
- **Policy, small and optional:** a sentence in `docs/voxel-format.md` stating measured load/edit
  expectations per size tier and what `budget_exempt` does **not** exempt you from; and a
  `check-scene` advisory when the emitted mesh would be pathological (the un-capped 3D-checkerboard
  case the diagnosis noted). No hard cap — `budget_exempt` keeps meaning exempt.

## Deliberately not done

- **No canvas rewrite of the slice painter.** 14k spans render fine when they render once; wave 3
  makes them render once. A canvas painter is the fallback if the sitting still finds single-render
  mount cost objectionable on the widest models — decided by measurement, not preemptively.
- **No LOD / mesh cap / chunked voxels.** `house-c` meshes small; the chunked-worlds seam stays a
  seam until content demands it.
- **No wire surface change, no new verbs.** Everything above is client- or engine-internal.
- **No `check-scene` subprocess cancellation.** Wave 2 makes each spawn cheap; process management
  complexity isn't paid for a 250 ms-debounced call.

## Results (2026-08-06)

Executed as three commits — rev 485 (waves 3–4, frontend), rev 486 (waves 1–2, engine), rev 487
(wave 5) — orchestrated across parallel subagents, sequenced against the voxel-tooling instance's
revs 483/484 on the shared `voxel.rs` (they ceded their own planned `from_doc` fix to this plan;
their next wave builds on the APIs below). Measurements, all on house-c:

- **Wave 1:** `Grid::from_doc` **247.1 → 10.3 ms** (dev opt-level 1); cost now tracks cells, not
  width². The decode API became `VoxelDocument::for_each_cell` (a visitor — each consumer wants its
  own flat indexing, and no `lib.rs` export was needed), parity with `palette_index` pinned
  cell-for-cell over every on-disk model (`tests/voxel_decode.rs`).
- **Wave 2:** new `VoxelDocument::connectivity()` = component count + `L1129` from one occupancy +
  one flood fill; `check-scene` went from 4 decodes + 2 floods to 2 + 1, stdout byte-identical,
  0.286 s wall on house-c.
- **Wave 3:** as designed (`frameStatus.ts` store — App's `onFrameStatus` chain stays the single
  writer, StatusBar subscribes to the fps *number* so a steady stream re-renders nothing; painter
  memo keyed on content-stable inputs via `cellview.ts`; `setPose` throttled; `slot` memoized). The
  idle-profile exit check rides the next Tauri sitting.
- **Wave 4:** full parse **84.3 → 15.2 ms**, per painted cell **84.3 → 0.14 ms**; undo retention
  from 2.95 MB/gesture to tens of bytes/cell. `patch.ts` refuses to the full reparse on length
  change, non-slice-row splices, re-tokenizing characters, or mismatched recorded text; undo entries
  are forward splice lists inverted at use.
- **Wave 5:** `AssetCache` (it lives in ludion-engine, not ludion-assets as this plan guessed)
  stores `Arc<MeshData>`; the per-frame ~6.3 MB clone is a refcount bump. Textures deliberately
  still deep-copy (`TextureImage` is built field-by-field; not the measured waste).
  **`--release` for the stage: dropped by measurement** — post-fix there is nothing left for it to
  rescue. **Policy docs: not done** — optional, and the voxel docs are another instance's active
  area; the advisory remains open if wanted.
- **Known external:** the untracked, corrupt `statue-gemma.voxel.yaml` reddens 6 vitest cases in
  this shared checkout (verified pre-existing; the model tests glob the directory, so a clean
  checkout is unaffected). Regenerating or removing that file clears it; owned by the statue-variant
  comparison set, not this plan.
