# Audit: the diagnostics system and error UX, engine-wide (2026-10-03)

**Scope.** Report only. This audit covers the shared diagnostics machinery, not the per-feature
error UX that the other audits in this series already cover. Files read: `Diagnostic`, `Severity`
and `DiagnosticCode` (`crates/ludion-core/src/diagnostics.rs`); the code table and its tests
(`crates/ludion-core/src/codes.rs`); `suggest::nearest`; the sim-side channel
(`crates/ludion-engine/src/diagnostics.rs`) and its drain and log sites in `engine.rs` and
`live.rs`; the CLI renderer (`crates/ludion-cli/src/diagnostic.rs`), `explain.rs` and the top
level of `main.rs`; the game host's copies (`crates/ludion/src/lib.rs`, `host.rs`); the MCP
adapter's error path; the editor's error surfaces (`editor/app/src/protocol/client.ts`,
`editor/app/src/viewer/diagnostic.ts`, `DiagnosticBanner.tsx`); every JSON Pointer escaper; and
`dev-docs/diagnostics.md`. Rev `34e3fa3f`. Method: static reading, a whole-tree cross-check of
every `L####` literal and `code::` constant against the table, and read-only runs of the existing
debug `ludion` binary in a scratch directory (since deleted).

## Overall assessment

The core is in good shape. The table is consistent: 297 constants and 297 rows, no duplicates,
every constant raised from production code, and every `L####` literal in source and docs
registered. Severity (July B1) has shipped, and the July D3/D4 printer split is fixed. The
weaknesses sit at the edges, where a `Diagnostic` leaves its producer:

- The sim-side channel throws away `help`, `path` and severity.
- The CLI's `report` treats every lint as an error.
- `L1015` has become the engine's catch-all I/O code, and its `explain` text contradicts most of
  its call sites.
- A bad per-machine environment variable stops even `ludion explain` from running.

None of these corrupts state. Each one costs an agent a fix-loop iteration on exactly the path
that Pillar 5 says should be cheapest.

## High

None found.

## Medium

### L1 — The sim-side diagnostic channel keeps only code and message, so every runtime diagnostic loses its help, its pointer and its severity · Medium · [verified]

**Evidence.**
- `Diagnostics` stores `pending: Vec<(DiagnosticCode, String)>`, and `report(world, code, key,
  message)` takes nothing more (`crates/ludion-engine/src/diagnostics.rs:52-57`, `:101-112`).
- The drain prints `"{code}: {message}"` and always logs it at `LogLevel::Warn`
  (`crates/ludion-engine/src/engine.rs:1095-1101`).
- Callers that already hold a full `Diagnostic` strip it down on the way in:
  - `report_doc_diagnostics` passes on only `diagnostic.message` and uses `path` as the latch key
    (`engine.rs:1946-1956`). This is the path for data-doc validators at install time and on hot
    reload, error-severity results included (`engine.rs:2102-2108`).
  - `report_font_faults` does the same (`crates/ludion-engine/src/fonts.rs:247-249`).
- `LogEvent` is just `{level, message}` (`crates/ludion-protocol/src/event.rs:76-81`), so a wire
  client cannot recover what was dropped.
- Log lines that carry diagnostics come in two shapes:
  - The rustc-style `Display` form, used by render failures (`crates/ludion-engine/src/live.rs:671-676`).
  - A bare `L####: message` form, used by the drain, transitions (`engine.rs:1555-1567`), profile
    saves (`engine.rs:1409-1414`) and system panics (`engine.rs:1120-1123`).
- The editor recognises only the rustc form (`DIAGNOSTIC_HEAD`,
  `editor/app/src/viewer/diagnostic.ts:38`). Every sim-side line therefore reaches the banner with
  no code chip and the code left inside the message. The test fixture uses `warning[L1121]: …`,
  but the engine never writes that shape to the log.

**Why it matters.** This is the one channel that game code and data-doc validation report through.
Its users get the "what" but never the "likely fix" (Pillar 5's fourth leg). They also cannot tell
a hard rule violation from a lint.

**Failure scenario.** A designer hot-reloads a `weapon` doc with a dangling `refers_to`. The game's
validator builds `L1144` with `.with_path("weapon/x/damage")` and a help line naming the nearest
doc. The editor shows `L1144: …` at warn level, with no help line and no pointer. The agent has to
re-run validation offline to get the suggestion that was already computed.

**Fix.** Make `Diagnostics` queue a `Diagnostic` (code, message, path, help, severity), with the
key passed separately for the latch. Then:
- Drain it through `Display`, with the level taken from `severity`, so the editor's regex and the
  CLI see a single shape.
- Keep `report(world, code, key, message)` as the thin form for callers that have no help line.
- Moving `LogEvent` to an optional structured `diagnostic` field is the larger, schema-moving step.

### L2 — `diagnostic::report` counts lints as errors and fails the command; a warning-severity `L1209` fails `check-scene` · Medium · [verified]

**Evidence.**
- `report` prints every diagnostic, says `N errors found`, and returns `FAILURE`. It does not look
  at `severity` (`crates/ludion-cli/src/diagnostic.rs:20-31`). The game host has its own copy,
  `report_diagnostics` (`crates/ludion/src/lib.rs:982-994`), with the same behaviour.
- `scene_atlas_diagnostics` mixes the lint `L1209` (`ANIMATION_SPRITE_CROSSFADE`,
  `Severity::Warning`, `codes.rs:5743-5745`) into its result (`scene_check.rs:144`, `:163-170`).
- Both `check_scene.rs:243-245` and `crates/ludion/src/lib.rs:1048-1050` pass that result straight
  to `report`.
- Reproduced: a scratch scene with an atlas `Sprite` and `Animation.fade: 0.5` prints
  `warning[L1209] …` and then `scene.yaml: 1 error found`, with exit code 1. The same scene without
  `fade` passes.
- [suspected] Prefab loading also folds `scene_atlas_diagnostics` into its `Err`
  (`crates/ludion-engine/src/prefab.rs:424-435`). A prefab whose body carries this lint would
  therefore be refused at spawn.

**Why it matters.** The table promises that a lint "never fails its command"
(`the_registered_lints_are_exactly_these`). In practice each call site still decides by which
vector it pushes into, which is the July B1 failure mode coming back one level up. Only the font
pass partitions by severity (`check_scene.rs:254-263`).

**Fix.** Make `report` (and the facade's copy, which should be deleted in favour of one shared
renderer) partition by `severity`:
- Print warnings.
- Count only errors in the tail.
- Return `SUCCESS` when there are none.
- Filter warnings out of the prefab load's refusal set.

### L3 — `L1015` "scene file could not be read" is the engine's catch-all I/O code, and its `explain` text contradicts most call sites · Medium · [verified]

**Evidence.** The constant's doc says it is raised by `check-scene` (`codes.rs:227-229`). It is
also raised for all of the following:
- Writing a scene over the wire (`crates/ludion-engine/src/commands/scene.rs:73-78`).
- Creating, writing and removing a mesh-edit derived tree, and listing a directory
  (`crates/ludion-mesh-edit/src/derive.rs:138-157`, `:1006`, via `io()` at `:841-843`).
- Listing a golden-test directory and *parsing* its JSON manifest
  (`crates/ludion-testkit/src/suite.rs:96-101`, `:157-163`).
- Reading voxel models, sculpts, atlas sources, maps, profiles and sweeps (about 25 sites in total).

The explanation says the path "is resolved relative to the content root … not the process's
current directory" (`codes.rs:2927-2934`). Almost every CLI site's `help` says "relative to the
current directory" (for example `generate_atlas.rs:157-161`, `voxel.rs:737-741`, `render.rs:350-354`).

A smaller instance of the same pattern: `commands::collapse` labels "the operation failed but
reported no diagnostics" as `L3001` malformed envelope (`crates/ludion-engine/src/commands/mod.rs:26-31`).

**Why it matters.** `ludion explain L1015` is the documented next step. For a failed `scene.save`
or a full disk during `mesh apply`, it tells the agent to check a *read* path against the wrong
root. Agents and tests that match on the code cannot tell a missing input from an unwritable
output.

**Fix.**
- Register an output-I/O code, or reuse the existing write-side codes per domain.
- Give the manifest parse its own testkit code.
- Rewrite `L1015`'s explanation to say that CLI paths are cwd-relative and wire paths are
  root-relative, matching the two `help` strings that already exist.

### L4 — A malformed `LUDION_THREADS` / `LUDION_QUALITY` / `LUDION_RENDER_SIZE` blocks every verb, including `explain` and `docs` · Medium · [verified]

**Evidence.** `main.rs:1490-1522` resolves all three before `match cli.command`, and returns
`FAILURE` on error.
- `LUDION_THREADS=bogus ludion explain L0014` prints the `L0014` error and exits 1, so the agent
  cannot read the explanation of the code it just hit.
- `LUDION_RENDER_SIZE=0x0 ludion docs` fails the same way.

The comment justifies this for verbs that render ("a silently-ignored setting means rendering at a
quality nobody chose"). `explain`, `docs`, `devdocs`, `image-stats` and `new` read none of the three.

**Why it matters.** These are per-machine environment variables, often inherited from a shell or
an editor launch. The help system is the recovery path, and it is gated behind the error it is
meant to explain.

**Fix.** Resolve the three knobs lazily, inside the verbs that consume them. At minimum, dispatch
`explain`, `docs` and `devdocs` before the startup resolution.

## Low

### L5 — `ludion render --scene S --frames 0,30 --dump-voxels V` without `--out` panics · Low · [verified]

**Evidence.** `--out` is `required_unless_present_any` with `dump_voxels`/`dump_gi` (`main.rs:385-387`).
`--frames` does not conflict with those flags (`main.rs:475`), and no manual check covers the
combination. `render.rs:228-230` then runs `.expect("main.rs requires --out with --frames a,b,c")`.
Reproduced: `thread 'main' panicked at crates\ludion-cli\src\render.rs:230:26`, exit 101.

**Fix.** Add `frames ≥ 2 && out.is_none()` to the manual usage checks beside `--dump-timings`
(`main.rs:1643-1655`), or make the strip arm return a usage diagnostic.

### L6 — The editor's error banner shows the code and the help twice, and drops `related` · Low · [verified by reading]

**Evidence.**
- `ProtocolError` sets `Error.message` to `"${code}: ${message} (${help})"`
  (`editor/app/src/protocol/client.ts:51`).
- `toDiagnostic` then returns `{code, message: err.message, help}`
  (`editor/app/src/viewer/diagnostic.ts:20-21`).
- `DiagnosticBanner` renders a code chip, then the composite message (code and help again), then
  the help span.
- `related` is never shown. For a `scene.load` failure, `commands::collapse` moves every
  diagnostic after the first into `related` (`commands/mod.rs:33-35`), so the human sees only the
  first error.

**Fix.** Use the raw `message` field in `toDiagnostic` (keep it on `ProtocolError` as
`diagnosticMessage`), and render `related` as a collapsed list.

### L7 — Hand-written `warning:` prefixes double up on rendered diagnostics · Low · [verified by reading]

**Evidence.**
- The audio sink prints `eprintln!("warning: {diagnostic}")` (`crates/ludion-audio/src/lib.rs:242`,
  `:272`, `:283`, `:292`, `:536`; `music.rs:220`). `Display` already starts with
  `warning[L5001]`, so the output reads `warning: warning[L5001]: …`.
- `lib.rs:439` wraps a `load_wav` failure, which is an error-severity `L11xx`, giving
  `warning: audio playback skipped `x`: error[L1101]: …`.
- `crates/ludion/src/host.rs:200` puts a full multi-line rustc block inside parentheses.
- The facade also builds `warning[{code}]` by hand (`crates/ludion/src/lib.rs:1430-1434`, `:1448`).
  It hard-codes an `"L1121"` fallback that cannot be reached (`:1428`).

**Fix.** Print the `Diagnostic` through `Display` alone. Where the context sentence matters, attach
it as `help` or `related`.

### L8 — Runtime failures in several hosts print `error:` with no `L####` code · Low · [verified]

**Evidence.**
- The game host's replay paths: `"error: could not read replay …"` at
  `crates/ludion/src/lib.rs:794`, `:866`, `:922`, and the frames-dir create at `:803`.
- The `ludion profile` output writes (`crates/ludion-cli/src/profile.rs:172`, `:193`, `:201`).
- `ludion new`'s I/O failures (`crates/ludion-cli/src/new.rs:117-193`).
- `check-scene`'s "does not validate `kind: X` documents" (`check_scene.rs:180`).

The usage-error cases (flag conflicts) are fine as clap-style messages. These are not usage errors:
each is a file or environment fault with an existing code family, such as `L30xx` replay or the
render-output codes.

**Fix.** Route each case through a registered code. Add a CLI lint (a grep in `check.mjs`) for
`eprintln!("error:` outside argument validation.

### L9 — A second severity override exists, and the doc claims a test prevents one · Low · [verified]

**Evidence.**
- `gen_cmd.rs:352` downgrades `L1167` (`COMFY_UNREACHABLE`) with `.with_severity(Warning)`.
- `scene_check.rs:452` calls `.with_severity(Warning)` on `L1254` (`FONT_GLYPH_MISSING`), which
  is already a warning in the table, so the call is redundant.
- `dev-docs/diagnostics.md:20` says `L1202` is "the one exception" and that "a test pins the whole
  lint set so a second exception cannot appear quietly". That test pins table rows only, and
  nothing checks `with_severity` call sites.

**Fix.** Either list the deliberate overrides in the doc, or add a source-scan test (the reachability
test's corpus walk is already there) that allows `with_severity` only at named sites.

### L10 — `ludion explain` handles near-misses poorly · Low · [verified]

**Evidence.**
- `explain l0001` is rejected because the lookup is case-sensitive.
- An unknown code gets no nearest-match suggestion, even though `suggest::nearest` exists.
- A retired code (`L0003`, `L1193`) gets `L0001`'s text: "mistyped or belongs to a newer engine
  version". That is wrong for a code an older engine really emitted.
- The `L0001` diagnostic carries the invented pointer `/error/code` (`explain.rs:21-27`), but there
  is no document to point into.

**Fix.**
- Upper-case the input.
- Keep a `RETIRED: &[(code, note)]` list, which already exists as comments in `mod code`, and
  explain retired codes from it.
- Suggest the nearest registered code.
- Drop the path.

### L11 — `Diagnostic::path` has two grammars and nothing marks which one a value uses · Low · [verified by reading]

**Evidence.**
- Field docs and the dev-doc call `path` a JSON Pointer (`diagnostics.rs:202-204`,
  `dev-docs/diagnostics.md:87`).
- Data-doc diagnostics use `<kind>/<doc name>/<field>` with no leading slash, as documented in
  `docs/data-docs.md:174` (for example `crates/ludion/src/lib.rs:1592`, `:1675`,
  `games/vigil/src/validate.rs:56`, `crates/ludion-engine/src/patterns.rs:1141`).
- A client that resolves `path` as an RFC 6901 pointer against the document fails on these
  without saying so.

**Fix.** State both forms in the `path` field docs and in `dev-docs/diagnostics.md`. Better, render
data-doc paths as `<file>#/<pointer>` so that every `path` resolves.

### L12 — The JSON Pointer escaper exists in five copies · Low · [verified]

**Evidence.**
- `check_scene.rs:999-1001` (`pointer_token`)
- `crates/ludion-engine/src/components.rs:2350-2352`
- `crates/ludion-engine/src/collapse.rs:499`, `:508` (inline)
- `crates/ludion-gen/src/workflow.rs:814-816`
- `crates/ludion-scene/src/yaml.rs:83-85` (`pointer_segment`)

All five are correct today (`~` is replaced before `/`). The risk is drift in a sixth copy.

**Fix.** Add one `ludion_core::diagnostics::pointer_token` and delete the copies.

### L13 — Stale dev-doc and a weak test · Low · [verified]

**Evidence.**
- `dev-docs/diagnostics.md:86` warns that forgetting `DECLARED_CODES` "passes all tests silently".
  `code_consts_and_declared_list_are_a_bijection` (`codes.rs:8650-8676`) now catches that.
- `every_code_const_is_referenced_outside_codes_rs` counts references in `tests/` and in
  `#[cfg(test)]` modules (`codes.rs:8686-8719`). A code whose only use is an assertion would pass
  as "live". A production-only cross-check found no such code today.
- The `crates/ludion-engine/src/diagnostics.rs` tests create
  `%TEMP%/ludion-diagnostics-<pid>-<name>` and never remove it (`:217-225`). 2,204 of these
  directories had built up on the reference box.

**Fix.**
- Delete the stale gotcha.
- Skip `/tests/` and `mod tests` in the reachability corpus.
- Give the tests a tempdir guard that removes the directory on drop.

## Checked and healthy

- **Table integrity.**
  - 297 constants map to 297 rows with no duplicates.
  - The constant set and `DECLARED_CODES` are an enforced bijection.
  - Every row has a title and an explanation, and every code is within `L0xxx`–`L5xxx`.
  - Every `L####` literal in `crates/`, `games/`, `editor/app/src`, `scripts/`, `docs/` and
    `dev-docs/` is registered, apart from the retired-code comments and the illustrative
    `L0042`/`L1042`/`L9999`.
  - No code is built at runtime with `format!`.
  - No constant is referenced only from tests.
- **Severity (July B1).** `Diagnostic` carries `severity` from the table. The wire omits it for
  errors, and `explain` and the listing print it.
- **Printers (July D3/D4).** `import`, `map-check` and `check-scene` print through
  `diagnostic::print`/`report`. `Display` renders `path`, `help` and nested `related`, so a direct
  `eprintln!("{d}")` no longer loses the pointer.
- **MCP adapter.** It returns engine diagnostics verbatim as tool errors through `Display`
  (`crates/ludion-mcp/src/server.rs:209-216`), `related` included.
- **`suggest::nearest`.** It is deterministic (earliest candidate wins a tie), case-insensitive,
  counts a transposition as cost 1, and has a sensible budget. It has 100+ call sites.
- **JSON Pointer escaping.** Every name-keyed pointer checked (regions, clips, frames, palette,
  tilemap legend, collapse) escapes `~` and `/` in the right order. Index-keyed pointers need no
  escaping.
- **CLI panics from user input.** Apart from L5, the `expect`s on CLI paths are guarded by clap
  groups or by earlier validation.
- **Sim-side printing.** No `eprintln!` reaches the determinism-gated sim crates (`ludion-ecs`,
  `-spatial`, `-physics`, `-scene`). The engine's stderr prints sit on the frame-boundary drain,
  not inside systems.

## Suggested order of work

1. **L4.** Dispatch `explain` and `docs` before startup resolution. This is a few lines and
   restores the recovery path.
2. **L2.** Make `report` severity-aware and fold the facade's copy into it. This fixes the lint
   failing `check-scene` and the game's `--check-scene`.
3. **L1.** Queue full `Diagnostic`s in `Diagnostics` and drain them through `Display` with a
   severity-derived level. This also fixes the editor's code chip for sim-side lines.
4. **L3.** Split write-side and parse failures off `L1015`, and correct its explanation.
5. **L5, L6, L7.** Small, contained fixes.
6. **L8–L13.** Code hygiene and docs, in any order.
