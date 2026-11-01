---
description: The ludion bin: subcommand pattern, the recipe for adding one, and the dev supervisor
---

# ludion-cli

**Scope:** The `ludion` binary (crate `ludion-cli`): the clap-derive subcommand pattern, the recipe for
adding one, the `--threads` resolution chain, and the `dev` hot-reload supervisor. **What a verb does
is not here** — `--help` is generated from the `Command` doc-comments, and the behaviour belongs to the
task topics: `ludion docs running`, `ludion docs verify-loop`, `ludion docs dev-loop`,
`ludion docs scene-format`, `ludion docs project-anatomy`, `ludion docs map-authoring`,
`ludion docs voxel-ops`, `ludion docs post-processing`, `ludion devdocs ludion-import`,
`ludion devdocs profiling`, `ludion devdocs budget-files`, `ludion devdocs ludion-renderer`,
`ludion devdocs ludion-testkit`. `ludion devdocs crate-graph` covers why a bin may depend on both
sides of a boundary.

## Layout

- `crates/ludion-cli/src/main.rs` — `Cli` (clap `Parser`), `enum Command` (clap `Subcommand`), the exhaustive match. Routing only.
- `crates/ludion-cli/src/explain.rs` — `ludion explain [CODE]`; the module to mirror for a new subcommand.
- Per-module map (every verb, `check_scene.rs` included): `ludion devdocs ludion-cli-verbs`.
- `crates/ludion-cli/tests/` — one integration test file per subcommand against the real built binary, plus `fixtures/`; the files are modules of `tests/it.rs` (declare a new one there), except `dev`, `new`, `phase7` and `profile_attach`.

Every other verb module, what it wraps and its contracts (`--json` shapes, atomic writers, `export` refusal order, `docs` discovery): `ludion devdocs ludion-cli-verbs`. The `dev` supervisor: `ludion devdocs ludion-cli-dev`.

## Recipe: adding a subcommand

1. Create `crates/ludion-cli/src/<name>.rs` with a module doc-comment
   (`//! Implementation of \`ludion <name> ...\`.`) and a
   `pub fn run(...) -> std::process::ExitCode`. Take plain args (`&Path`, `Option<&Path>`, `u64`);
   print results to stdout, `Diagnostic`s to stderr. Mirror `explain.rs` for a minimal command,
   `sim_hash.rs` for one that loads a scene (reuse `check_scene::load_engine` and
   `check_scene::builtin_registry`).
2. Add `mod <name>;` in `main.rs` (modules are in alphabetical order) and mention the command in
   the crate-level `//!` doc.
3. Add a variant to `enum Command` with doc-comments on the variant and every field — clap derive
   turns these into `--help`. Use `#[arg(long)]` flags and `#[arg(long, default_value_t = ...)]`
   for defaults, matching the existing variants.
4. Add the match arm in `main()`: `Command::Name { args... } => name::run(&args...)`. Nothing
   else — the arm is routing only.
5. Add `crates/ludion-cli/tests/<name>.rs` (and `mod <name>;` in `tests/it.rs`) driving the built binary:
   `Command::new(env!("CARGO_BIN_EXE_ludion")).args([...]).output()`, asserting on status,
   stdout, and stderr (see `tests/explain.rs`). Put scene fixtures in `tests/fixtures/`. Note
   `tests/test_cmd.rs`: when the subcommand name collides with a Rust keyword or existing test
   file, suffix the test file, not the module.
6. If the command touches domain logic, put that logic in the owning `ludion-*` crate and keep
   the CLI module thin.



**The `--threads` chain.** `main()` resolves one thing before it routes: the sim worker count, through
`ludion_core::par::set_startup_threads`. `--threads N` is clap `global = true`, so it applies to every
verb, `serve` included; without it the count comes from `LUDION_THREADS`, and without that from the
machine's available parallelism capped at `par::AUTO_THREAD_CAP`. A malformed `LUDION_THREADS` is
`L0014` and exits before any verb that reads the knobs runs; `explain`, `docs`, `devdocs`,
`image-stats` and `new` read none of `--threads`/`--quality`/`--render-size`, so `knob_free` answers
them first and a bad per-machine variable never blocks the help system. `dev` alone does not apply it directly: it spawns the
game as a child, which inherits the environment and resolves the same chain through `ludion::run`.
Results are worker-count invariant, so this moves throughput and timings only — never a hash, a
golden, or a replay (`ludion devdocs determinism`).

## Rules

- **One module per subcommand; `main()` contains only the exhaustive match.** Each arm is a one-line
  delegation to `module::run(...) -> std::process::ExitCode` — the `dispatch.rs` convention: routing,
  never logic. A **family** (`voxel <verb>`) is one nested `Subcommand` and one nested match, each arm
  still a one-line delegation. Nest the routing, never the logic.
- **Success output on stdout; diagnostics on stderr** in rustc style, with a non-zero `ExitCode` on
  failure. Never panic on bad input — every failure is an L-coded `Diagnostic` with help text. A
  verb's stdout carries its data only — status and verdict lines go to stderr, so `> out.json` is
  always parseable. Under `--json` a runtime failure still prints its one object (`ok: false` +
  `diagnostics`); a usage error (a bad flag combination) prints none.
- **A boolean `--json` prints exactly one compact JSON value on one line.** Documents — the profile
  report, `dump-graph`, timings, files written to disk — are pretty; `| jq .` pretty-prints the line
  for a human.
- **All scene-consuming commands validate through `ludion_engine::builtin_registry()`**; no command
  defines its own component set.
- **Doc-comments on `Command` variants and their fields *are* the `--help` text** — keep them accurate
  and behavior-shaped ("Two runs with the same inputs always print the same hash"). This is why no
  flag catalog belongs in this file. One place needs `#[command(verbatim_doc_comment)]`: the
  `voxel apply` ops skeleton, which clap's reflow would destroy.
- **Every subcommand gets an integration test against the real built binary**
  (`env!("CARGO_BIN_EXE_ludion")`), not against the module functions.
- **Machine-readable output must stay parseable** — the dump-graph and timings JSON, the 16-hex-digit
  hash, `serve`'s first line. Agents consume stdout programmatically.

## Verify

```
cargo test -p ludion-cli
cargo run -p ludion-cli -- explain L0001
cargo run -p ludion-cli -- check-scene crates/ludion-cli/tests/fixtures/good.yaml
cargo run -p ludion-cli -- sim-hash --scene crates/ludion-cli/tests/fixtures/good.yaml --frames 600
cargo run -p ludion-cli -- sim-hash --scene crates/ludion-cli/tests/fixtures/good.yaml --record s.lrp
cargo run -p ludion-cli -- replay s.lrp   # prints the same final hash the recording run printed
cargo run -p ludion-cli -- docs           # lists every docs/ topic; devdocs lists dev-docs/
cargo nextest run -p ludion-cli --test it -E 'test(/^voxel_cmd::/)' # the voxel family's integration tests
cargo run -p ludion-cli -- serve --scene crates/ludion-cli/tests/fixtures/good.yaml --paused
                                           # prints `listening on ws://...`, then blocks for Ctrl-C
```

`render`/`test` additionally need a GPU adapter:
`cargo run -p ludion-cli -- test --dir examples/gallery`.

## Gotchas

- The package is `ludion-cli` but the binary is `ludion` (`[[bin]] name = "ludion"`); in integration
  tests that is `env!("CARGO_BIN_EXE_ludion")`, not `..._ludion-cli`. Clap derives kebab-case names
  from variants: `CheckScene` → `check-scene`, `DumpGraph` → `dump-graph`.
- `ludion render --debug-view ssr_confidence|ssr_footprint|probe_index|local_shadow_atlas|sun_cascades` draws the SSR trace, the winning reflection probe per pixel, or the local-light shadow atlas, instead of the picture (`sun_cascades` instead tints the picture by sun cascade) (scene mode only); it never turns its effect on, and the summary line says when the normal frame was drawn (`ludion devdocs renderer-ssr`, `ludion devdocs renderer-probes`, `ludion devdocs renderer-passes`).
- `ludion render --dump-probes <dir>` writes each reflection probe's captured and prefiltered strips and `probes.json` (scene mode, needs a GPU, captures fresh, never touches the store; `ludion devdocs renderer-probes`).
- `ludion-cli` carries the *strict* determinism clippy denylist even though it is a leaf bin — no
  `std::collections::HashMap` or `Instant::now` in CLI code either.
- **The CLI links no game code, and three limits follow.** `sim-hash` runs no game systems, so world
  content — and the hash — does not change with `--frames`; `ludion replay` cannot replay a game
  recording (`L1013` at load); `ludion test --bless` cannot bless a scene naming game components.
  Each has a game-side counterpart in `ludion docs verify-loop`.
- `serve` is the one subcommand depending on `ludion-server` (allowed: the CLI may depend on both
  sides of the engine/server boundary). Its tests spawn it and `Child::kill` it — std has no
  cross-platform way to deliver a graceful Ctrl-C to a *child*, so a test needing a real
  signal-driven clean-shutdown flush is deferred.
- `serve`/`sim-hash`/`check-scene` root the engine's `project_root` at the **scene file's own
  directory**, resolved absolute first, so a texture-backed scene streams identically from any cwd.
  Getting this wrong does not fail the load — it silently renders nothing for a `frames` subscriber
  (see `ludion devdocs ludion-engine` on `Engine::new()`'s `"."` default).
- `ludion new` refuses to scaffold under the workspace's `crates/*`/`games/*` member globs (`L0011`)
  — that would make the project a member of two workspaces.
- Blessing the import golden is a plain render, not an `import` flag: `ludion render --scene
  examples/import/expected/scene.yaml --width 960 --height 540 --out
  examples/import/goldens/<os>/scene.png` on that platform's pinned GPU. The committed
  `examples/import/` tree is `crates/ludion-cli/tests/import.rs`'s fixture; the mapping is
  `ludion devdocs ludion-import` (`Plan/phase-8-assets-and-sound.md` decision 3).
- `profile --check` numbers mean nothing outside `--release` on the calibrated hardware
  (`Plan/phase-9-hardening.md` decisions 1–2, `ci/teamcity/` config 6); the file format and the
  calibration conventions are `ludion devdocs budget-files`.
- `image-stats`' four metrics gate regressions and are never a target to optimise — a real image
  legitimately carries some banding, some hard edges, some clipped pixels
  (`Plan/post-process-pipeline.md` decision 10; metrics: `ludion devdocs ludion-testkit`).
