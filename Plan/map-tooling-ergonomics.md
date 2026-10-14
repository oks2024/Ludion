# Plan: Map Tooling Ergonomics — the mid-loop verifier does the coordinate math

> Status: **COMPLETE (2026-07-23, Lore rev 303, one orchestrated wave, executed as planned).** All 10
> mechanized exit criteria green; the smoke proof on the three real 3× districts caught one heuristic
> false positive in the first `L1029` implementation (it probed the cell center — a torch-post at a
> clear cell corner read as buried by the fountain's center-clipping collider) and the fix landed with
> the wave: `L1029` probes the marker's actual spawn point `(x, z)`, the only point that matters for
> "would spawn blocked." Generator emission stayed byte-identical (map-drift green); the two
> historical `*.map-check.txt` evidence files were left untouched per decision 10.
>
> Original draft status (2026-07-22), kept for the record:
>
> Origin: the city-scale arc's measured agent-cost evaluation (2026-07-22), recorded as **M1-E in
> `Plan/map-authoring.md`**. Finding: the `edit-map → generate-map → map-check` loop converges in
> 1–2 layout iterations per district, but only because the agent hand-computes every
> `origin`/`row`/`col` ↔ world-unit conversion; that hand-math dominated the map-side token cost and
> produced the wave's one map bug — a perimeter gap authored on the wrong side of the boundary wall —
> which `map-check` could not flag and a full-test roundtrip caught instead. Per invocation,
> `map-check`'s per-cell walkability-delta enumeration was the single largest unused output.
>
> This plan turns the six ranked M1-E improvements into one orchestrated wave. It touches only
> `crates/ludion-scene`, `crates/ludion-cli`, and `crates/ludion-core` (the code table) — never
> `games/vigil`, so it composes with a games-side agent. It ships **no** new protocol command, **no**
> GUI, and **no** new map-format capability: it is purely the mid-loop verifier and the compile verb
> getting better at the job they already have.

## Thesis (scored against the North Star)

The one metric is *how many autonomous iterations an agent runs before a human must look at the
screen*. The map loop is already headless and deterministic; the leak is that the tool reports the
world in coordinates the agent must translate by hand, and stays silent on the one class of layout
bug (a mis-placed gate/anchor) that only a slow full-render or full-test roundtrip exposes. Every
item here moves the metric by **removing a hand-computation or catching a bug in-loop**:

- **The tool speaks both coordinate systems.** A marker/anchor/gate table and an `--at x,z` query
  print world `(x,z)` *and* cell `(col,row)` together, so the agent never counts characters across a
  36-row grid or divides by `cell_size` in its head.
- **The tool flags placement bugs at authoring time.** A gate with no gap in the wall, or a marker
  sitting inside a prop / in a sealed pocket, becomes a warning in the report the agent already reads
  every iteration — not a mystery discovered a full-test later.
- **The report is signal, not noise.** The walkability-delta enumeration — the largest output and, it
  turns out, mostly false positives (see the finding below) — collapses to a one-line count.
- **A shared-fragment edit is one command, not N.** Batch `generate-map`/`map-check`, and a
  write-free `generate-map --check`, keep the "did my edit stay canonical / still match the committed
  scene" question inside the loop.

None of this adds a protocol surface or a GUI; those are M1/M2. This is the loop the agent runs
today, made cheaper and louder about the failures it currently misses.

## Current-code facts that shape the work

- **`crates/ludion-cli/src/map_check.rs`** is the whole report. It resolves+parses the map exactly as
  `generate-map` does (shared `generate_map::load`), runs the pure `ludion_scene::generate` in memory,
  then: strips the generated scene to `Transform`+`Collider` builtins, `instantiate`s, calls
  `ludion_spatial::spatial::rebuild` once, and probes each cell centre with a static-collider overlap
  query (occupancy); 4-connected flood-fill from an anchor (reachability); the per-cell
  walkability-delta; and the cross-map gate-link check (`L1027`). All CPU — **no GPU**, so every part
  is unit-testable in the CLI crate today. The gate check is the *only* step that raises diagnostics
  and the *only* thing that flips the exit code (`ExitCode::FAILURE` when any `L1027` fires); the
  report itself always prints.
- **`map_check.rs` carries a report-only duplicate `Grid`** (lines ~84–136: `origin`/`cell_size`/
  `width`/`height`, `center`, `cell_of`) that mirrors the internal `Grid` in
  `crates/ludion-scene/src/map.rs` (lines ~963–1006: `rect_center`, `cell_of`). The marker-table and
  `--at` work is grid arithmetic; it must reuse **one** grid, not add a third copy.
- **`crates/ludion-scene/src/map.rs`** owns the pure, IO-free `parse_map` / `parse_map_kit` /
  `generate`, plus `MapDoc` (`origin`, `cell_size`, `rows`, `markers`, `base`, `seed`) and `MapMarker`
  (`name`, `kind`, `x`, `z`, `components`). It has no `ludion-spatial`/`ludion-engine` dependency and
  must not gain one (boundary rule) — so anything needing occupancy stays CLI-side.
- **`crates/ludion-cli/src/generate_map.rs`**: `run(map_path, out)` — a single positional `map` plus
  `--out`, defaulting to the `.map` suffix dropped. `load()` (shared with `map-check`) resolves `kit:`
  and each `fragments:` entry relative to the map file. A `self_check_idempotent` already re-parses
  and re-emits the output before writing (parse→emit byte-idempotence). Writing is the only effect;
  a failed generate writes nothing.
- **`crates/ludion-cli/src/main.rs`**: clap `Subcommand`. `GenerateMap { map: PathBuf, out }` and
  `MapCheck { map: PathBuf, anchor, radius }` are per-file positional verbs. The only existing
  multi-input convention is **`Test { dir: PathBuf (default "."), bless, filter }`** — a *directory*
  that scans `*.yaml` inside it, not a list of files. `Render` uses an `ArgGroup` for mutually
  exclusive inputs. No verb today takes a `Vec<PathBuf>` of positionals.
- **`crates/ludion-core/src/codes.rs`**: `L1019`/`L1020` are the reused `Tilemap` codes; `L1021`–
  `L1027` are the map range (`L1027` = `MAP_DANGLING_GATE_ANCHOR`). The next code is **`L1101`**
  (`ASSET_FILE_IO`) — so **`L1028` and `L1029` are free**. Every code appears **twice**: a `CodeEntry`
  (title + explanation, ~line 1240) and the `ALL_CODES` list (~line 2661). `ludion explain` reads
  both; a new code must be added to both or the explain gate fails.
- **`crates/ludion-core/src/diagnostics.rs`**: `Diagnostic` has **no severity field** — its `Display`
  renders `error[L####]:`. "Warning" is a *CLI rendering convention*: `import.rs::print_warning`
  prints `warning[L####]: <message>` + `help:` by hand (the doc-comment says outright "the
  `Diagnostic` model has no warning severity of its own"), and `check-scene` renders budget warnings
  the same way. So making a lint a warning is a rendering choice in `map_check.rs`, not a data-model
  change.
- **`scripts/map-drift.mjs`** is the CI drift gate: it scans `games/` + `examples/` for any `*.yaml`
  bearing a top-level `generated_from:`, regenerates each via `generate-map … --out <temp>`, and
  asserts byte-identity. This is exactly the per-file comparison `generate-map --check` exposes to the
  agent mid-loop. This work does **not** touch the generator's emission, so the gate stays green by
  construction.
- **`crates/ludion-cli/tests/map.rs` + `tests/fixtures/report.txt`** are the report's mechanized
  golden: `map_check_report_matches_golden` asserts the `map-check` stdout byte-for-byte against
  `report.txt` (map path normalized). The M1-E "map-check is snapshot-tested" oracle is **already
  built** — a format change updates this fixture, and that update *is* the format's mechanized capture.
- **Committed evidence files** — private evidence files in `Plan-Private/artifacts/` — are review artifacts from the city-scale arc. They are *not* `*.yaml`, *not* under
  `games/`/`examples/`, and carry no `generated_from:`, so **`map-drift.mjs` never touches them** and
  no gate compares them — they are ungated historical text.

## The finding that reshaped item 3

Running `map-check` read-only on the real districts (2026-07-22):

- `district-plaza.map.yaml` (33×36): **84** "intended-walkable cells blocked by a prop."
- `district-market.map.yaml` (9×7): **10**.

These are not layout bugs. The districts author buildings as **markers** with AABB colliders; a
building's footprint legitimately overhangs the floor-glyph cells beside its centre cell, so the
delta counts *every overhang of every building* — by design. The check as written cannot distinguish
an intended footprint from an accidental overhang, so at district scale the enumeration is not merely
verbose, it is **mostly false-positive noise** that buries the report's real signal (reachability
pockets, gate links). This is the decisive argument for **killing the enumeration outright** rather
than hiding it behind a flag: a per-cell list nobody can act on is worse than a single count, and a
count still works as a cheap sanity number (a sudden jump between iterations is worth a glance).

## Design decisions

### 1. The terse report is the default; the per-cell enumeration is deleted (not hidden)

The walkability-delta becomes exactly one line — `walkability delta: N cell(s) blocked by an
overhanging prop` — with **no** flag to bring the enumeration back. This obeys the repo's one-way
rule (*before adding a second way to do something, remove the first*; no `--verbose` shadow format to
maintain) and follows the finding above: the enumeration is noise, not a hidden power feature. An
agent that needs to know *which* cell is a targeted question answered by the marker table and `--at`
(decisions 2 and 4), not a wall of coordinates. Net effect on the plaza report: **−84 lines** of
enumeration for **+1** count line.

### 2. The marker/anchor/gate table is default-on

Below the grid, `map-check` always prints one line per marker/anchor/gate:

```
markers (37):
  fountain          kind=fountain   world ( 0, -30)   cell (c16,r19)
  south-gate        Gate → market   world ( 2,  42)   cell (c16,r35)
  guide             npc             world (-8, -12)   cell (c14,r15)
  …
```

Default-on, not opt-in, on two grounds. **Token economy:** it *replaces* the deleted enumeration and
is strictly cheaper — the plaza's 37 markers are 37 lines against the 84 it removes, and it is the
single output that kills the most hand-math (world↔cell conversion and character-counting). **Loop
fit:** the whole point is that the agent reads it every iteration; an opt-in table is a table nobody
remembers to ask for. Rows are emitted in the generator's marker source order (stable, matches the
`.map.yaml`), gates annotated with their `→ <district>` target so the table doubles as the gate
inventory.

### 3. The grid ruler is default-on

Col guides every 5 columns as a header line, row guides every 5 rows as a left prefix:

```
       0    5    10   15   20   25   30
   0  #################################
   5  #...............................#
  ...
```

Default-on and untoggleable, for the same reason as the table: reading `(col,row)` straight off the
grid is the loop's core act, and an opt-in ruler defeats it. The cost is one header line plus a
short numeric prefix per row — negligible against a grid already printed in full. At 3× (33 cols) it
fits comfortably; wider grids are a terminal-wrapping concern, not the tool's.

### 4. `--at x,z` is a flag (a targeted query), printed alongside the report

`map-check <map> --at 12,-30` appends a focused block:

```
at (12, -30): cell (c19,r18)  blocked  nearest marker `fountain` 8.2u away
```

A flag, not default, because it answers a specific question the agent poses; the full report still
prints (its existing always-print contract holds), so `--at` never costs the surrounding context.
The block reports the cell, walkable/blocked from the same occupancy probe, and the nearest marker by
Euclidean distance (name + distance) — the three facts an agent otherwise derives from three separate
manual steps. `x,z` parses as a single comma-joined value (one token, cross-platform; no reliance on
shell arg-splitting).

### 5. Placement lints are warnings, under two new codes `L1028`/`L1029`

Two checks, both computed from the occupancy + reachability the report already builds:

- **`L1028` — gate with no perimeter gap.** A marker carrying a `Gate` component whose cell is not a
  gap punched through the boundary wall: none of its orthogonally-adjacent cells is both walkable and
  on the grid edge, i.e. the gate is sealed behind solid wall. This is exactly the city-scale bug (a
  gap authored on the wrong side).
- **`L1029` — marker on a blocked or unreachable cell.** A marker resolving to a cell the occupancy
  probe found blocked (it would spawn inside a prop), or a cell that is walkable but never reached by
  the reachability flood-fill (a sealed interior pocket). Subsumes the "anchor on a bad cell" case
  without inferring anchor role — a marker inside a prop or a pocket is almost always wrong whatever
  its role. **The probe must exclude the marker's own collider:** a building authored as a marker
  (the market-hall/stall passthrough pattern) blocks its own cell *by design* and would otherwise
  fire on every such marker — the walkability-delta false-positive lesson applied to the lint.
  Blocked by someone else's collider is the bug; blocked by your own is a footprint.

**Severity: warnings**, rendered `warning[L1028]: …` via a `print_warning`-style helper (the
`import.rs` precedent), printed at the top of the output where they cannot be missed, and **not**
flipping the exit code. This is a *deliberate divergence* from `L1027`, and the divergence is the
point: `L1027` is **provable** (a name either resolves in the sibling district or it does not — a
false negative guarantees a runtime `L3019` crash), so it stays an **error** that fails the command.
`L1028`/`L1029` are **heuristic** — they depend on a probe `--radius` and on geometry, and each has a
legitimate exception (a cosmetic or not-yet-cut gate; a marker intentionally placed on a threshold
prop or a decorative island). A heuristic that fails the command breaks the loop on false positives;
a warning the agent reads every iteration catches the real bug without that cost. Consistency is in
*spirit* (surface placement problems at authoring time, the `L1027` intent generalized) not in
*severity*. No `--deny-warnings` promotion knob ships here — out of scope; add it only if a real need
to gate on lints appears.

### 6. Batch is multiple positional paths; `--out` stays single-input only

`generate-map <a.map.yaml> <b.map.yaml> …` and `map-check <a> <b> …` take a `Vec<PathBuf>` of
positionals, processing each in turn and aggregating the exit code (non-zero if any input fails).
`--out` is valid only with exactly one input (a clap conflict / arity check) — with N inputs each
writes to its own default path.

Chosen over a `--dir` scan or a glob. A **glob** is rejected because Windows is the primary platform
and PowerShell does not expand `*.map.yaml` into argv — a glob would silently pass a literal `*` on
the primary target. **`--dir`** (the `ludion test` convention) is rejected as a *mismatch*: `test` is
inherently a suite-runner with no per-file identity, whereas `generate-map`/`map-check` are per-file
verbs whose natural plural is "these files," and a directory holding map sources also holds their
generated scenes, hand-authored scenes, kits, and fragments, forcing a `*.map.yaml`-only sub-filter
that is really just a glob in disguise. Explicit positionals are deterministic, cross-platform, and
match the motivating case (the three districts are known paths; regenerating all after a shared
kit/fragment edit is three named arguments). For the "verify everything" case, `node
scripts/map-drift.mjs` already checks every generated scene repo-wide.

### 7. `generate-map --check` regenerates in memory and compares, writing nothing

`generate-map <map> --check` runs the full generate + idempotence self-check, then compares the
result byte-for-byte against the committed scene already at the output path (the default path, or
`--out`), prints `up to date` or `DRIFT` (naming the path), writes nothing, and exits non-zero on
drift. It is the drift gate's per-file comparison (`scripts/map-drift.mjs`) exposed to the mid-loop
agent, replacing a hand-rolled generate-to-temp + `diff`. `--check` composes with `--out` (which
names the comparison target) and with batch (each input compared against its own output). Because it
never writes, an agent verifies "does my source still match the committed scene" without dirtying the
tree or needing a `git checkout` afterward — the North Star metric, applied to the drift question.

### 8. Where each piece lives (crate boundaries)

- **`ludion-scene` (pure, spatial-free):** promote the grid geometry — `cell_of` (world→cell),
  `cell_center`/`rect_center` (cell→world), width/height — to a small **public** `MapGrid` (or free
  functions on `MapDoc`), and delete the report-only duplicate `Grid` in `map_check.rs`, so the
  generator and the report share one coordinate implementation (a one-way-rule payoff, not a third
  copy). The marker-table *data* (each `MapMarker` → its `(col,row)` and world `(x,z)`) is pure and
  belongs here too, unit-testable with no registry or GPU.
- **`ludion-cli` (presentation + spatial analysis):** everything that needs occupancy/reachability —
  the report rendering, the ruler, the table's *formatting*, the `--at` query resolution, and both
  placement lints — stays in `map_check.rs`, because occupancy hangs off `ludion-spatial` +
  `ludion-engine::builtin_registry`, which `ludion-scene` cannot depend on. The batch loop and
  `--check` comparison are CLI-level control flow in `generate_map.rs` / `main.rs`. This matches the
  existing split (the report's CPU analysis already lives CLI-side) and keeps the pure/impure line
  clean.

### 9. Scope guard — explicitly NOT in this work

- **No protocol command.** `scene.generate` is M1 proper; nothing here touches `dispatch.rs`, the
  `Command` enum, or the wire surface (which stays at 26 methods).
- **No GUI.** The 2D painter is M2, deferred behind its own trigger.
- **No new map-format capability.** No new kit/marker/grid fields, no new expansion mode, no runtime
  or nav change. The generator's *emission is byte-identical* before and after — the drift gate and
  every committed generated scene must stay green untouched.
- **map-check stays read-only; `generate-map --check` writes nothing.** The only write-effecting path
  remains plain `generate-map` (unchanged), and batch just runs it N times.

### 10. Committed evidence files stay as historical artifacts

The two repo-root `*.map-check.txt` files are **not regenerated** by this work and are left as-is.
They are ungated (decision-8 fact: `map-drift.mjs` never sees them), so a report-format change cannot
fail CI through them, and regenerating them would erase their meaning — a timestamped record of what
the city-scale Wave 2 review actually saw. The current-format evidence this work produces is the
updated `tests/fixtures/report.txt` golden plus the smoke-proof run (Wave exit criteria); if a
reviewer wants a fresh full-district report committed, it lands under a new name, not by overwriting
the historical two.

## The wave

**One orchestrated wave, one package, one subagent.** The two natural halves — (A) report + table +
ruler + `--at` + lints, and (B) batch verbs + `--check` — were considered as parallel packages and
**rejected**: both edit `main.rs` (clap args) and `map_check.rs` (A rewrites the report internals; B
wraps `run` in a batch loop), so parallel agents would collide in the same two files. The surface is
~4 files (`map.rs`, `map_check.rs`, `generate_map.rs`, `main.rs`) plus `codes.rs` and one test
fixture — small enough for one agent. Internal ordering: land the shared `MapGrid` + report/table/
ruler/lints first (they define the report), then wrap batch + `--check` around the settled single-map
verbs.

**Sequencing.** Executable immediately after the city-scale arc's **Wave 4** commits. It requires the
district **map sources** to exist (`district-market.map.yaml` / `district-cathedral.map.yaml` land in
city-scale Wave 2) so the smoke proof runs against a real 3× source. It never writes under
`games/vigil` — the smoke proof is a read-only `map-check` and a write-free `generate-map --check` —
so it composes with a games-side agent working in `games/vigil`.

### Exit criteria (mechanized)

1. **Report golden updated and green.** `crates/ludion-cli/tests/fixtures/report.txt` reflects the
   new format (ruler + marker table + one-line delta count) and `map_check_report_matches_golden`
   passes byte-for-byte.
2. **Marker table — pure test.** A `ludion-scene` unit test: a known map → the expected
   `(name, kind, world (x,z), cell (col,row))` rows, asserting the cell arithmetic with no CLI.
3. **`--at x,z` — behavioral test.** On a fixture with a known blocked cell and a known walkable
   cell: the query reports the right cell, the right blocked/walkable verdict, and the right
   nearest-marker name+distance.
4. **Placement lints — behavioral tests.** A map with a `Gate` sealed behind solid wall raises
   `L1028` (rendered `warning[L1028]`, exit unchanged); a marker on another prop's cell and a marker
   in a sealed pocket each raise `L1029`; a building marker whose own collider blocks its cell raises
   neither (the self-exclusion); a clean district raises neither. Deterministic and GPU-free (the
   existing CPU spatial probe).
5. **Batch — behavioral tests.** `generate-map a b` writes both default outputs; `map-check a b`
   prints both reports; a batch with one failing input exits non-zero; `--out` with two inputs is a
   usage error.
6. **`generate-map --check` — behavioral tests.** Against a committed-matching source: `up to date`,
   exit zero, **file unchanged** (assert bytes before == after). Against a deliberately drifted
   source: `DRIFT`, non-zero, still no write.
7. **`ludion explain L1028` and `L1029` work** — both added to the `CodeEntry` table *and* the
   `ALL_CODES` list; the explain/docs gate green.
8. **Drift gate unaffected.** `node scripts/map-drift.mjs` green; every committed generated scene
   byte-identical (the generator's emission is untouched — assert this explicitly, it is the
   invariant the whole map pipeline rests on).
9. **Smoke proof.** Run the loop on a real 3× district source — `generate-map --check` (write-free,
   reports up-to-date) then `map-check` (the new report reads correctly: ruler aligned, table
   coordinates match the grid, lints silent on the shipped-clean district) — as evidence the tooling
   works on production content, not just fixtures.
10. **`node scripts/check.mjs` full green** (fmt, clippy, test, CLI smoke, schema-drift, map-drift).

## Alternatives considered and rejected

- **Hide the walkability enumeration behind `--verbose` instead of deleting it** — rejected by the
  one-way rule and the noise finding: a second report format to maintain, for an output that is mostly
  false positives at district scale. The count carries the signal; `--at`/the table carry the detail.
- **Make placement lints errors (like `L1027`)** — rejected; a heuristic geometry check with a
  `--radius` knob and legitimate exceptions would break the loop on false positives. Warnings surface
  the bug in-loop without that cost. `L1027` stays an error because it is provable.
- **Batch via `--dir` (the `ludion test` convention) or a glob** — rejected: a glob does not expand on
  the primary platform (PowerShell); `--dir` mismatches per-file verbs and needs a `*.map.yaml`
  sub-filter that is a glob in disguise. Explicit positionals are deterministic and cross-platform.
- **A third grid implementation for the table/`--at`** — rejected; the report already duplicates
  `ludion-scene`'s grid once. Promote one `MapGrid` to shared public API and delete the duplicate.
- **Ship this as the M1 protocol command** — rejected; these are CLI-loop ergonomics with no editor
  or wire consumer. Folding in a `scene.generate` command would drag the whole parity-gate/dispatch
  surface into a tooling wave. M1 stays its own step.
