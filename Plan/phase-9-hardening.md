# Plan: Phase 9 — Hardening & Public Surface

> **Superseded (2026-09-10):** the license decision recorded below (proprietary, all rights
> reserved, 2026-07-17) was reversed — Ludion is MIT OR Apache-2.0 (CLAUDE.md §Current state). The
> text is kept as the record of the earlier stance.

> Status: **Waves 1–6 done (2026-07-12)** — see the notebook's "Done (Phase 9)" for commits and
> findings. **License decided 2026-07-17: proprietary / all-rights-reserved** (user) — the earlier
> `MIT OR Apache-2.0` was an unreviewed Phase-0 scaffolding default, never a user choice; the
> open-source `LICENSE-MIT`/`LICENSE-APACHE` files are replaced by a single all-rights-reserved
> `LICENSE` (copyright Alexandre Pestana 2026), `Cargo.toml` license = `LicenseRef-AllRightsReserved`.
> Open-source is explicitly a *later* decision, not settled. **Wave 7 (publish) is therefore
> deferred, not merely blocked**: with an all-rights-reserved license there is no crates.io
> publication to do now — the remaining publish-time questions (public mirror, crates.io account)
> only reopen if/when open-sourcing is chosen. Wave 8 (exit gates) can still run *hermetically from a
> local build* (the fresh-agent / fresh-designer checks don't require public artifacts). Question: would a
> hostile stranger — human or agent — succeed on day one? Exit gate (implementation-guide): a
> fresh agent given only shipped docs + examples builds a small game correctly on the first try;
> a fresh human designer installs, attaches, and edits within an hour. Phase 9 re-answers Success
> Checks 1–8 in writing.

## Current-code facts that shape the work

- **No `profile` command exists.** `crates/ludion-cli/src/` holds check_scene, docs, dump_graph,
  explain, new, render, replay, serve, sim_hash, test — yet `profile` is in the settled CLI list.
  `render --dump-timings` already emits real per-pass GPU timings JSON
  (`crates/ludion-render-graph/src/timings.rs:52`); **sim-side per-system timing does not exist** —
  `Schedule::run` (`crates/ludion-ecs/src/schedule.rs:77`) is a bare loop, and the only sanctioned
  wall clock in the engine is `live.rs`'s scoped denylist exception
  (`crates/ludion-engine/src/live.rs:21`).
- **Publication has three bugs-in-waiting.** (1) `ludion new` derives the engine root from
  `env!("CARGO_MANIFEST_DIR")` (`crates/ludion-cli/src/new.rs:186`) and the scaffold's Cargo.toml
  emits four *path* deps into the checkout (`crates/ludion-cli/templates/Cargo.toml`) — an
  installed-from-crates.io binary bakes a build-machine path that exists nowhere. (2) `ludion docs`
  walks ancestors for `CLAUDE.md` + `docs/` (`crates/ludion-cli/src/docs.rs:43`) — an installed
  binary always fails with L0008; only the engine guide is embedded (`new.rs:75`). (3) No LICENSE
  files exist, no crate has a `description`/`repository`/`rust-version` field, and
  `workspace.package.version = "0.0.0"` (`Cargo.toml:8`) — with `license = "MIT OR Apache-2.0"` a
  Phase-0 scaffolding default in `Cargo.toml:10` (**superseded 2026-07-17: license is now
  proprietary / all-rights-reserved by user decision** — see the Status header).
- **Rustdoc coverage is already effectively enforced:** every crate carries
  `#![warn(missing_docs)]` and CI runs `clippy -- -D warnings`, which promotes it to an error. The
  freeze pass is therefore about *quality* (examples, naming, accidental `pub`), not presence.
- **Zero `#[non_exhaustive]` in workspace code** (the sole grep hit is a comment in
  `ludion-client/src/lib.rs:434`). Applying it naively would break the parity gate: `dispatch.rs`'s
  exhaustive match over `Command` lives in *another crate* (`ludion-engine`), and `non_exhaustive`
  forces downstream wildcard arms.
- **No fuzzing exists** (no `fuzz/` dir). Attack surfaces: strict-YAML scene parse (saphyr-events),
  protocol envelope/Command JSON decode, the 36-byte LFR1 frame header, the LRP1 replay reader
  (zstd-framed, seekable, with length fields in the clear — a corrupt `len` is an allocation bomb),
  canonical-writer idempotence (proptested already; fuzzing extends the input space).
- Protocol: 25 methods (`crates/ludion-protocol/src/command.rs:192`; 26 after Phase 7 adds
  `engine.shutdown`), one supported version,
  `schema_hash` handshake rejects stale clients (L3003). The additive-rev re-bless order is
  recorded only as notebook lore (a private notebook, "Decisions (Phase 6)") — not
  a shipped policy document.
- L-codes in use: L0001–L0011, L1001–L1018, L1101–L1102, L2001–L2007, L2101–L2102, L2201–L2204,
  L3001–L3014, L4001–L4003. The Phase 7–8 plans additionally claim L0012/L0013, L1019/L1020,
  L1103/L1104, L1110–L1112, L1201–L1203, L3015/L3016, L5001 — this plan's L4004/L4005 collide with
  none of them. The table already has registration/uniqueness/shape/non-empty tests
  (`codes.rs:1389–1445`), but `DECLARED_CODES` is hand-maintained — a new const can silently skip it.
- Toolchain pinned at 1.94.0 (`rust-toolchain.toml`); goldens/hashes are versioned against the pin;
  a bump is a deliberate re-bless event. Publication must state this honesty tier (bit-exact per
  pin, not per "any stable rustc").
- **CI of record (TeamCity/Perforce, macOS agent) is still not stood up** and is user/hardware-
  gated. Phase 9 cannot own it, but budgets-in-CI and scheduled fuzzing land as configs in
  `ci/teamcity/` regardless; the macOS half of every criterion rides that parallel track.
- The crate set grew: `ludion-window` (Phase 6 Wave 5) is a dependency of both `ludion-cli` and the
  `ludion` facade — any publishable-set list omitting it cannot publish the CLI.

## Design decisions (settled by this plan)

1. **`ludion profile` ships: one JSON combining sim per-system, GPU per-pass, and frame totals.**
   `Schedule` gains a hooked run variant taking a clock-free `before(name)`/`after(name)` observer
   trait — `ludion-ecs` never sees a clock, so its denylist is untouched. The `Instant`-holding
   hook lives in a `ludion-engine` profile driver beside `live.rs`, under a second scoped denylist
   exception with the same justification: wall-clock measurement is *observation*, output-only —
   it never enters sim state, so WorldHash is provably unaffected (asserted by test). GPU numbers
   reuse the existing timestamp path. Methodology: release profile, warmup frames + N measured
   frames, medians (noise resistance). Rejected: timings as an ECS resource (observation entering
   the world poisons determinism); a `profile` wire command (a schema rev for a host-side concern —
   CLI-only like `test`/`new`; recorded as a future additive-rev candidate).
2. **Budgets are TOML files checked by `ludion profile --check`.** `budgets/pinned-rtx3090.toml`:
   per-system ms, per-pass ms, frame total, each with generous headroom (~2× observed) — budgets
   are regression tripwires, not perf goals. `--check` exits nonzero with a per-line over-budget
   report. New codes: **L4004** (budget file unreadable/invalid), **L4005** (budget exceeded).
   CI asserts budgets release-profile on the pinned-GPU TeamCity agent only (precedent: the
   phase4 fps test's honest profile-gating). Rejected: budgets inline in CI scripts (not diffable
   engine content, invisible to `ludion explain`-style tooling).
3. **Fuzzing via cargo-fuzz, quarantined from the pin.** `fuzz/` lives *outside* the workspace with
   its own nightly `rust-toolchain.toml` — justified against the 1.94.0 pin because fuzz targets
   ship nothing, produce no goldens/hashes, and run only in a scheduled bounded TeamCity job.
   Targets: scene parse, envelope/Command JSON decode, LFR1 header, LRP1 reader (headers + seek on
   corrupt input, with an allocation cap on the in-the-clear `len`), emit→parse→emit idempotence.
   Every finding is fixed as an L-coded diagnostic and its input committed as a plain `#[test]` in
   the owning crate, so the pinned stable CI runs regressions without nightly. **"Parsers never
   panic on arbitrary input" becomes a stated guarantee** in the shipped docs. Rejected:
   proptest-only (already have it; coverage-guided fuzzing finds what generators don't encode);
   AFL (cargo-fuzz/libFuzzer is the maintained Rust default).
4. **API freeze pass = quality audit + selective `#[non_exhaustive]`.** Per-crate checklist executed
   and committed: naming regularity, every public item documented *with a runnable example* (the
   convention), no accidental `pub`. `non_exhaustive` goes on what clients *consume*: `Response`,
   `ResponseBody`, `Event`, and `Diagnostic`/`CodeEntry` (after the audit verifies constructor-only
   construction — `Diagnostic::new` + `with_help` is already the pattern). It does **not** go on
   `Command` (the exhaustive dispatch match IS the parity gate — documented as "deliberately
   exhaustive") nor on params structs (clients construct them literally; additive fields are
   serde-defaulted and 0.x-breaking anyway). **Semver stance: everything publishes at 0.1.0; under
   0.x, a minor bump is breaking and a patch is compatible (cargo's native 0.x caret semantics);
   1.0 is a separate, deliberate future event.** Rejected: 1.0 now — it would promise the stability
   this phase exists to test.
5. **The protocol evolution policy becomes a shipped doc, `docs/protocol-evolution.md`** (a
   `ludion docs` topic). Contents promoted from notebook lore: the additive-rev checklist as
   practiced (bless schema snapshot → hand-update `EXPECTED_SCHEMA_HASH` → TS regen → re-record
   `run.lrp` *last*, since its header pins the hash; doc comments feed schemars, so comment edits
   move the hash); what forces v2 (renames, removals, semantic changes); the deprecation path
   (advance notice in docs — with exactly one supported version, removal *is* v2); the
   exactly-one-version stance and its consequence: bumps are breaking by design, L3003 is the
   enforcement. Rejected: multi-version negotiation (double surface, Pillar 6 violation).
6. **crates.io publication: every workspace lib + the two bins, in dependency order.** As of
   Phase 6 that is 16 crates: core → ecs → {scene, sim, assets, protocol} → render-graph →
   renderer → engine → {server, client} → testkit → window → ludion → cli → mcp; Phases 7–8 add
   `ludion-import` and `ludion-audio` (and possibly more), so the set and the name-availability
   check are recomputed when this phase starts. NOT published: `games/*` (`publish = false`),
   `examples/`, `editor/`. Workspace-
   inherited metadata: `description` per crate, `rust-version = "1.94"`, keywords/categories,
   a single all-rights-reserved `LICENSE` file (license *choice* decided 2026-07-17 — proprietary,
   copyright Alexandre Pestana; the earlier MIT/Apache dual-license files were removed). `repository`
   is an **open question the user owns**, but moot while proprietary: git is deliberately local-only,
   and there is no publication to do, so no public mirror or repo link is needed now — this reopens
   only if open-sourcing is later chosen. docs.rs needs no GPU (it builds docs, runs nothing;
   verified locally via `cargo doc --no-deps` per crate). `cargo publish --dry-run` for every crate
   joins CI. Version 0.1.0 per decision 4.
7. **`ludion new` and `ludion docs` grow an installed mode.** `new`: at runtime, if the baked
   engine root exists and contains `crates/ludion`, emit path deps (dev flow unchanged); otherwise
   emit version deps at the binary's own `CARGO_PKG_VERSION` for all four engine crates in the
   template. `docs`: embed all `docs/*.md` topics in the binary (completeness-tested against the
   docs dir); repo-root discovery wins when present (prints paths, files editable), embedded mode
   prints the doc body. Rejected: shipping docs as installed files beside the binary (`cargo
   install` gives one binary, no data dir).
8. **The hermetic fresh-agent test (Success Check 5, honest this time).** Kills milestone-1-proof
   caveat #1 (the harness auto-loaded the engine repo's CLAUDE.md). Harness: `ludion` installed via
   `cargo install ludion-cli` *from the published crates.io artifact* into PATH; agent spawned with
   cwd = a clean directory outside the checkout whose ancestors contain no CLAUDE.md (verified by
   the harness before spawn); fresh session, no MCP config into the checkout, no memory; prompt =
   the one line + environment facts only (binary on PATH, pinned GPU present, optionally a
   prewarmed shared `CARGO_TARGET_DIR` built from a sacrificial scaffold of the same registry
   deps — honest, since it caches only published code). Pass = green suite (gameplay, WorldHash
   determinism, blessed golden, committed replay), zero intervention; transcript audited for any
   engine-internal reference. Companion human gate: a fresh designer — ideally not the project's
   user — installs, attaches the editor, and completes an edit session within an hour from shipped
   docs alone. That requires a distributable editor: a Windows Tauri release bundle ships this
   phase; the macOS bundle rides the hardware gate.
9. **Error audit closes the loop on Pillar 5.** Every table entry reviewed against the rustc bar
   (what/where/why/fix); mechanized additions to the existing tests: a bijection test that every
   `pub const` in `code::` appears in `DECLARED_CODES` (parse `codes.rs` — the hand-maintained list
   is the current gap) and a reachability check that each const is referenced outside `codes.rs`.
   Success Checks 1–8 re-answered in writing in `hardening-proof.md` (precedent:
   `milestone-1-proof.md`), including the honest gaps that document carried.

## Waves

### Wave 1 — Publication blockers, known early (cheap, informative)
- crates.io name-availability check for all publishable names (16 as of Phase 6, plus Phase 7–8
  additions; `ludion` above all) — result recorded in the
  notebook; if squatted, the fallback naming is a user decision raised immediately.
- `ludion new` installed mode (decision 7): runtime root detection, version-dep template branch.
  **Test:** forced-installed-mode scaffold (env override) yields a Cargo.toml with version deps and
  no absolute paths; existing checkout-mode integration test unchanged.
- `ludion docs` embedded topics. **Test:** embedded set == `docs/*.md` (completeness); lookup works
  from a cwd outside any repo.
- Workspace metadata: 0.1.0, descriptions, `rust-version`, a single all-rights-reserved `LICENSE`
  (proprietary, copyright Alexandre Pestana — decided 2026-07-17), `publish = false` on games.
  **Test:** `cargo doc --no-deps` clean. (The `cargo publish --dry-run` gate is moot while
  proprietary — nothing is published; it reinstates only if open-sourcing is later chosen.)

### Wave 2 — `ludion profile` + budgets
- `Schedule` observer hook (ecs, clock-free) + profile driver in `ludion-engine` (scoped denylist
  exception, documented) + `ludion profile --scene … --frames N [--json out] [--check budget.toml]`.
- `budgets/pinned-rtx3090.toml` calibrated from measured medians ×2 headroom; L4004/L4005.
- **Tests:** profile JSON snapshot shape; WorldHash identical with/without the hook; `--check`
  against a deliberately tiny budget exits nonzero with a per-line L4005 report; TeamCity
  release-profile budget step defined in `ci/teamcity/`.

### Wave 3 — Fuzzing
- `fuzz/` (own nightly toolchain, outside the workspace): five targets per decision 3, minimal
  seed corpora committed.
- Initial bounded runs (hours, local); every finding fixed as an L-coded diagnostic + committed
  regression `#[test]` in the owning crate. LRP1 `len` allocation cap lands here.
- **Tests:** regression inputs pass on the pinned stable toolchain; scheduled TeamCity fuzz job
  defined; "parsers never panic" stated in `docs/scene-format.md` + protocol doc.

### Wave 4 — API freeze pass
- Per-crate checklist executed (one commit per crate or small group): examples on every public
  item, naming regularity, `pub` audit; `#[non_exhaustive]` applied per decision 4 (protocol
  response-side enums, Diagnostic/CodeEntry after constructor-only verification).
- **Tests:** existing suites stay green (non_exhaustive misapplication = downstream compile
  errors caught immediately); doctests are the example-coverage proof; checklist committed to
  `Plan/` or `docs/` as the reviewable artifact.

### Wave 5 — Protocol evolution policy
- `docs/protocol-evolution.md` written per decision 5, served by `ludion docs`, cross-linked from
  `docs/ludion-protocol.md` and the engine guide. **Test:** docs frontmatter gate (`ludion docs`
  listing in CI) covers it; the re-bless checklist is verified against the Phase 6 history.

### Wave 6 — Error audit
- Review pass over all ~50 table entries against the rustc bar; rewrite the weak ones.
- Bijection + reachability tests per decision 9. **Test:** the new tests; a deliberately
  unregistered const fails the bijection test.

### Wave 7 — Publish — **DEFERRED (license is proprietary as of 2026-07-17)**
- No crates.io publication while all-rights-reserved. This wave reactivates only if the user later
  chooses to open-source; at that point revisit the license model, the public-mirror/`repository`
  question, and the crates.io account, then: publish all 16 crates in dependency order; verify
  docs.rs builds; tag/record the exact pin + schema_hash + golden set the release corresponds to;
  Windows Tauri bundle built and attached to the release record.

### Wave 8 — Exit gates (expensive, last)
- **Hermetic fresh-agent run** per decision 8, from the published artifacts; result + friction
  written into `hardening-proof.md` alongside Success Checks 1–8 re-answered.
- **Human gate:** fresh designer, install → attach → edit within an hour, shipped docs only.
- Phase closes when both pass and the proof document is committed.

## Risks

- **`non_exhaustive` vs the parity gate** is the subtlest freeze decision — applied to `Command` it
  silently deletes the compile-error guarantee. Settled here (decision 4) so the audit executes a
  written rule, not judgment under way.
- **Budget flakiness is a P0 trap:** wall-clock gates on real hardware flap. Contained: medians,
  ×2 headroom, release-only, pinned-GPU-agent-only; a budget failure must mean a real regression.
- **Publication is near-irreversible** (yank ≠ delete; names are forever). Mitigated by wave
  order: dry-run from Wave 1, actual publish only after freeze/fuzz/audit waves.
- **The repository/mirror question** (local-only git vs public source) and the crates.io ownership
  account are user decisions — **moot while the license is proprietary** (2026-07-17); they reopen
  only if open-sourcing is later chosen, together with the license-model reconsideration.
- **The macOS halves** (goldens, fps, bundle, CI agent) remain hardware-gated — a stated
  precondition tracked on the parallel CI-of-record track, not silently waived by this phase.
- **Hermeticity is easy to fake accidentally:** a stray ancestor CLAUDE.md, a shared MCP config, or
  agent memory re-leaks context. The harness *verifies* the environment before spawn; the
  transcript audit is part of the pass criteria, not optional.
- **Fresh-designer hour budget** includes a Rust toolchain install for `cargo install`. If that
  proves fatal to the hour, prebuilt CLI binaries become a follow-up deliverable — noted, not
  designed here.
