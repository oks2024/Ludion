# Audit: mesh editing and the sculpt mesher (2026-10-03)

**Scope.** This audit reads the write-side content verbs at rev `7ac43c5e`. It covers
`crates/ludion-mesh-edit` (`derive`, `raster`, `paint`, `fill`, `region`, `ray`, and
`tests/derive.rs`) and `crates/ludion-sculpt` (`grid`, `field`, `surface_nets`, `decimate`,
`soup`, `sources`, and `lib.rs`). It also covers `crates/ludion-cli/src/mesh.rs` (`mesh pick`,
`mesh apply [--check] [--view]`), `crates/ludion-cli/src/sculpt.rs` (`sculpt new`), and the
mesh-edit document model in `crates/ludion-scene/src/mesh_edit.rs`. Engine-side, it covers the
loaders `crates/ludion-engine/src/sculpt_mesh.rs` and `mesh_asset.rs`. Gate-side, it covers
`scripts/mesh-edit-drift.mjs` and its CI and `check.mjs` wiring, plus the docs
`docs/mesh-editing.md` and `docs/mesh-edit-regions.md`. The editor's sculpt inspector writes text
through the host file writer and was audited in `2026-10-03-editor-parity.md`, so it is not
re-read here.

Every probe ran the prebuilt `ludion` binary against scratch copies outside the repo, which were
deleted afterwards. That included `mesh apply --check` on copies of vigil's three committed idol
edits (all green).

## Overall assessment

The numeric core is solid and deterministic by construction:
- Texel math is `f64` with one rounding to bytes, and every transcendental goes through
  `ludion_core::math`.
- The weld uses a `BTreeMap` over position bits, and seam bridges are sorted and deduplicated.
- Dilation reads a snapshot, and the soup sampler uses thread-count-invariant `par_chunks`.
- Both crates sit in the `sim` clippy tier, and conformance-hash tests pin the derived PNGs across
  platforms.

The weaknesses are in the filesystem layer around the derive, not in the math:
- `Derived::write` trusts that the `out` directory is its own. Two edits naming the same `out`
  silently overwrite each other, and a `source` inside `out` makes `apply` rewrite its own input.
- The drift gate finds edits only through their outputs. An orphaned tree, a clobbered edit, or an
  edit whose tree was never committed is therefore never compared.
- Writes are in place and not atomic.

No High findings. The two Medium findings are both gate-coverage holes I reproduced.

## High

None.

## Medium

### M1 — Two edit documents naming the same `out` silently overwrite each other's tree, and the loser drops out of the drift gate · Medium · [verified]

**Evidence.** `Derived::write` (`crates/ludion-mesh-edit/src/derive.rs:113-163`) refuses only
files whose names it would not produce (`is_derive_owned`, `:967-978`). Another edit's
`mesh.mesh.yaml` and its `textures/<16 hex>.png` match those names, so they are overwritten and
removed. Nothing compares the `generated_from:` of the `mesh.mesh.yaml` already on disk against the
edit now writing.

**Reproduction (scratch).** I made `a.mesh-edit.yaml` and `b.mesh-edit.yaml`, both with
`out: shared` and different colours.
1. `mesh apply a`, then `mesh apply b`. Both printed "wrote shared".
2. `a`'s PNG was deleted and `shared/mesh.mesh.yaml` now carries
   `generated_from: "../b.mesh-edit.yaml"`.
3. `mesh apply a --check` fails, but `mesh apply b --check` is green.

**Why it matters.** `scripts/mesh-edit-drift.mjs` discovers edits only through a derived
document's `generated_from:` (`:68-89`), so edit `a` is now never checked. Any scene pointing at
`shared/mesh.mesh.yaml` "for a" renders b's glow, and every gate is green. The likely way to get
here is copying an edit document that has an explicit `out:` and changing only its ops.

**Fix.** Before writing, read the existing `mesh.mesh.yaml` (if any). If its `generated_from`
does not name this edit document, refuse with `L1176` and name the owning edit.

### M2 — The mesh-edit drift gate cannot see an orphaned derived tree or an edit with no tree · Medium · [verified]

**Evidence.** `scripts/mesh-edit-drift.mjs:63-89` walks `*.mesh.yaml` files and deduplicates by
edit path. It runs `mesh apply <edit> --check` once, which compares only the tree at the edit's
*current* `out`.

**Reproduction (scratch).**
1. `mesh apply r` with `out: one`.
2. Change it to `out: two` and apply again.
3. `one/mesh.mesh.yaml` and `two/mesh.mesh.yaml` both say `generated_from: "../r.mesh-edit.yaml"`.
   `mesh apply r --check` exits 0, and `one/` is never compared again.

In the same way, a committed `*.mesh-edit.yaml` whose `out` tree was deleted or never committed
yields no candidate, so the gate passes vacuously.

**Why it matters.** The tree is "write-only for agents", so the drift gate is its only guard. A
scene still pointing at the orphan keeps stale content forever with every gate green. The gate is
also silent about an edit document that was never applied.

**Fix.** Make the gate input-driven:
- Walk `*.mesh-edit.yaml` and check each one, so a missing tree fails.
- For each derived `mesh.mesh.yaml`, also require that its directory equals the `out_dir` its edit
  resolves to. `mesh apply --check --json` already prints `out_dir`.

## Low

### M3 — `Derived::write` writes in place, document first; a kill or I/O error leaves a document naming maps that do not exist yet · Low · [verified]

**Evidence.** `derive.rs:134-147` calls `std::fs::write` straight onto the final paths, in
`files()` order: the document first (`:85-94`), then each PNG. Stale maps are removed afterwards
(`:151-161`).

**Failure scenario.**
- If the process is killed, or the disk fills, between the document and its PNG, the tree holds a
  document whose emissive path does not exist.
- A hot reload (`ludion dev`, the editor's watcher) that fires in that window gets `L1101`/`L1102`.
- The drift gate flags the broken tree and a re-run repairs it, which is why this is Low. This
  disk-full case has happened on the reference box.

**Fix.** Write each PNG first (content-hash names, so writing a new one never disturbs a reader).
Then write the document to a sibling temp file and rename it into place, and remove stale maps
last. `sculpt new` (`crates/ludion-cli/src/sculpt.rs:101-131`) already uses temp-then-rename.

### M4 — A `source` inside the edit's own `out` is accepted, so `apply` rewrites its own input and never converges · Low · [verified]

**Evidence.** `derive.rs:259-293` resolves `source` and `out` independently. Nothing refuses a
`source` path that lies under `out`.

**Reproduction (scratch).** Derive once into `self/`, then set
`source: self/mesh.mesh.yaml`. Each `mesh apply` repaints its previous output and rewrites the
document it read. `--check` reports DRIFT after every apply.

**Why it matters.** The docs promise "the source mesh is never rewritten". A chained edit that
reuses the first edit's `out` reaches this state.

**Fix.** After the lexical normalisation that already exists, refuse (`L1174` at `/source`) when
`source_rel` starts with `out_rel`.

### M5 — `strength: 0` validates, but always fails as `L1177` with help that says the region misses the mesh · Low · [verified]

**Evidence.** Validation accepts `strength` in `[0, 1]` inclusive
(`crates/ludion-scene/src/mesh_edit.rs`, the strength check in `validate`). In `paint.rs` the
weight becomes `base * 0`, every texel is skipped, and `derive.rs:496-502` then emits
`painted_nothing`.

**Reproduction.** I set op 1 of the plate fixture to `strength: 0.0`. Output: "op 1 painted
nothing: its `capsule` region spans [0, -0.1, 0.7]..[1, 0.1, 0.9] while the mesh spans
[0, 0, 0]..[1, 0, 1]", with help "move or grow the region until the two boxes overlap". The boxes
already overlap.

**Fix.** Either refuse `strength: 0` at validation (a range of `(0, 1]`), or special-case the
`L1177` message when `region_texels > 0`. The second also covers a `fill` whose weights are all
zero.

### M6 — A map size taken from the base-colour map is range-checked with a pointer to a key the author never wrote · Low · [verified by reading]

**Evidence.** In `derive.rs:610-640`, when `size` is omitted the dimensions come from the source's
base-colour PNG. A PNG over 4096 on either side then gets "`size` is {width}, outside the 1..=4096
texel range", pointed at `{pointer}/size`. That key does not exist, and only the width is printed
even when the height is the axis that is out of range.

**Fix.** Branch the message: "the base-colour map is W×H, above 4096; add `size:` to create a
smaller map". Point it at `{pointer}/map`.

### M7 — Every engine load of a sculpt pays for warning-only `hidden_by` lattice passes, and then discards them · Low · [verified]

**Evidence.**
- `crates/ludion-sculpt/src/lib.rs:323` computes `hider()`, a full extra lattice pass, for every
  part with no surface, on every `mesh_with` call.
- `crates/ludion-engine/src/sculpt_mesh.rs:60` meshes through that call on each load and reload.
- `mesh_data` (`:160`) then drops `stats`.

**Measurement (release binary, `check-scene`).** One body sphere at `resolution: 128` with 8 small
spheres:

| small spheres | grid | meshing time |
|---|---|---|
| 8 hidden | 236³ | 7.5 s |
| 8 visible | 236×238×236 | 4.8 s |

That is about 2.8 s of warning-only work on each reload. The editor's sculpt inspector triggers a
reload on every throttled slider write, and a part buried in the body is a normal intermediate
state while authoring.

**Fix.** Compute `hidden_by` lazily. Either add a `mesh_with_stats` variant that `check-scene` and
the editor's stats path call, or move `hider()` into `unresolved_parts`, which is the only reader.

### M8 — Mesh-edit diagnostics print OS-native separators while the rest of the verb prints forward slashes · Low · [verified]

**Evidence.**
- `derive.rs:852` (`drift`) uses `path.display()`, as do the `write` and `walk` messages
  (`:119-122`, `:998-1006`). On Windows the gate prints `DRIFT shared\mesh.mesh.yaml`, which I
  observed.
- `crates/ludion-cli/src/mesh.rs:679-681` (`path_text`) normalises every path the verb itself prints
  to `/`.
- `--check --json` puts these messages in its `drift` array, so the same failure reads differently
  on the two pinned platforms.

**Fix.** Format derive diagnostics with the same forward-slash spelling, or with the
edit-relative path lexically.

### M9 — Stale text: the docs promise a refusal the code does not make, and the drift gate's "cheap" comment is false · Low · [verified]

- `docs/mesh-editing.md:165` says `out` is refused if it holds "a stray leftover from an earlier
  edit targeting a different map set". `Derived::write` removes such a file silently
  (`derive.rs:149-161`), because it is derive-owned by name. M1's reproduction shows this removal.
- `scripts/mesh-edit-drift.mjs:16-18` says each candidate "is read only as far as its `positions:`
  list". `:69` calls `readFileSync` on the whole file and `generatedFrom` splits the whole text
  before looping. Today that is 121 files and about 52 MB under `games/` and `examples/` on every
  gate run. Read a fixed-size prefix instead (the field sits in the first lines), or fix the
  comment.

## Checked and healthy

- **Determinism of the derive.**
  - All texel math is `f64` with one rounding (`WorkingMap::to_rgba8`).
  - sRGB conversion goes through `ludion_core::math::powf`, and `sqrt` through
    `ludion_core::math::f64`.
  - There is no std transcendental in either crate outside test code (grep).
  - The raster tie goes to the lower triangle, and `MapSampler` uses integer resampling.
  - Fill: the weld is a `BTreeMap` on bits with `-0.0` folded, union-find unions under the lower
    root, the bridge list is sorted and deduplicated, BFS uses a fixed neighbour order, and the dilate
    reads a snapshot with a fixed 8-neighbour sum order.
  - "Seeded fill" means a seed *point*. There is no RNG anywhere in either crate.
  - `the_plate_derives_to_the_conformance_hash` and `the_strip_fill_derives_to_the_conformance_hash`
    pin bytes across platforms. The vigil idol trees check green on a scratch copy.
- **Determinism of the sculpt mesher.**
  - Everything is `f32` through `ludion_core::math`.
  - Sampling and surface nets run sequentially in z, y, x order, with fixed quad-split ties.
  - The decimator heap key is `(error bits, edge index, version)`, so ties are total.
  - Soup BVH sorts use `total_cmp`, and the only parallel step (`mesh_source`) writes disjoint
    chunks by index through `ludion_core::par`.
  - Both crates are on the `sim` clippy tier (`scripts/clippy-denylist.mjs`).
- **Robustness.**
  - UVs outside `[0, 1]`, index out of range, and extent 0 or above 4096 are `L1176`.
  - A zero-UV-area triangle is skipped, and a zero-length capsule segment does not divide.
  - `closest_on_triangle` handles a zero-area triangle, and `smin` keeps `a` on NaN or `inf - inf`.
  - An empty soup gets a positive one-cell grid. `Grid::sample` checks extent overflow and caps at
    16 M corners, and repeat is capped at 64.
  - `nearest_surface` of an empty mesh returns infinity, which becomes a clean `L1178`.
  - A pick that misses is `L1175`, with a re-render hint.
- **Validation UX.**
  - `out` is refused when absolute (either separator, or a drive letter) or when it contains `..`.
  - Every op and region error carries a JSON Pointer.
  - `L1177` names both bounding boxes, and `L1178` names the nearest colour and its distance.
  - `self_check` round-trips the derived document before anything is written.
- **Foreign-file refusal** happens before any byte is written. Stale owned maps are removed, and
  `check()` flags untracked files in `out`.
- **Pick and `apply --view`** share the single `model_camera` the sheet uses. The engine has a test
  tying the public camera to the sheet's own, and pick reports the hit re-interpolated over
  model-space corners.
- **Lexical paths.** `relative`/`normalize` keep the derived document byte-identical across OSes.
  The engine resolves texture references document-relative, and `asset_cache` normalises
  dependency keys.
- **Gate wiring.** `mesh-edit-drift` is in `check.mjs`, `.github/workflows/ci.yml`, and the private
  CI step list. `a_written_tree_checks_green_and_drifts_on_one_byte` proves `check()` can fail.

## Suggested order of work

1. **M1 + M4.** Both are ownership checks in `Derived::write` and `derive`: the existing document's
   `generated_from`, and `source` not under `out`. Small, local, and they close the silent
   overwrites.
2. **M2.** Make `mesh-edit-drift.mjs` input-driven and require each derived tree to sit at its
   edit's `out`. Fold M9's whole-file read into the same change.
3. **M3.** PNGs first, then the document via temp-then-rename, then stale maps removed last.
4. **M7.** Compute `hidden_by` only where a warning is printed.
5. **M5, M6, M8, and M9's docs line.** Message and doc polish.
