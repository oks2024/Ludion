# Ludion — Master Implementation Guide

> The road from empty repo to a shipping LLM-first engine, as high-level phases.
> `engine-pillars.md` = why · `engine-architecture.md` = what/how · this document = in what order and when to stop.
>
> Each phase has one **question it answers** and hard **exit criteria**. A phase is done when its
> criteria pass — not when its code feels finished. Phases 0–5 are Milestone 1 and map onto the
> 10-step build order in `engine-architecture.md` §10; phases 6+ are sequenced but deliberately
> less detailed — they get their own design pass when they become current.

---

## How to use this guide (agent instructions)

- Work in the **current phase only**. If a task seems to need code from a later phase, the task is
  mis-scoped — shrink it or flag it.
- Never start phase N+1 while phase N exit criteria fail. Partial credit does not exist.
- Every phase ends with the repo in a demoable state: a command that runs, a test that passes.
- When a phase completes: update `CLAUDE.md`'s "Current state," commit the exit-criteria evidence
  (test names, CLI transcripts) in the phase's closing commit message.

---

## Phase 0 — Bedrock
**Question: does the project have a spine that enforces its own rules?**
*(build-order step 1)*

Workspace scaffolding, `ludion-core` (IDs, `Diagnostic` + L-code table, `SimTime`/`FrameIndex`,
`DeterministicRng`, stable collections), `rust-toolchain.toml` pin, CI skeleton (fmt, clippy with
per-crate denylists, test), `ludion` CLI stub with `explain`.

**Exit criteria**
- `ludion explain L0001` prints a real explanation.
- RNG golden-sequence test passes; a deliberately added `HashMap` in a sim crate fails CI.
- CI green on Windows **and** macOS runners from the very first commit — the platform matrix is
  bedrock, not a later add.

## Phase 1 — Deterministic World
**Question: can a world exist, be described in text, and prove it is deterministic?**
*(build-order steps 2–4: `ludion-ecs`, `ludion-scene`, `ludion-sim`)*

The custom ECS (freeze list from architecture §4), component registry with per-component JSON
Schema, strict-YAML scene parse/emit/validate with the canonical writer, tick loop, input scripts,
`WorldHash`.

**Exit criteria**
- ECS conformance suite passes (generational safety, deterministic query order, command buffers).
- Scene round-trip property tests + emit idempotence (byte-equal); `ludion check-scene broken.yaml`
  prints L-code + JSON pointer + suggestion.
- `ludion sim-hash` identical across two processes; in-process double-run test green in CI;
  one scripted gameplay assertion passes headlessly.

**This phase is the credibility core.** Everything the engine later claims (replay, goldens,
snapshots, editor parity) rests on determinism proven here, before a single pixel exists.

## Phase 2 — Pixels, Verified
**Question: can the engine render, and can an agent *know* the render is right without eyes?**
*(build-order steps 5–7: `ludion-render-graph`, `ludion-renderer`, `ludion-testkit`)*

Frame graph on wgpu (declarative interface kept free of wgpu types where costless — the ash seam),
headless render-to-PNG, HD-2D pipeline (sprite/billboard batcher, low-poly meshes, dynamic point
lights, bloom + tonemap, bitmap text), golden harness (SSIM + tolerance, structured diff reports,
bless flow).

**Exit criteria**
- `ludion render --scene x.yaml --out f.png` deterministic; `--dump-graph` / `--dump-timings`
  JSON snapshot tests pass.
- Example-gallery scenes render with committed goldens on **both** pinned GPU targets (Windows +
  macOS); SSIM thresholds calibrated against llvmpipe for hosted CI.
- An intentionally broken scene produces a diff report that names the damaged region — readable by
  an agent with no image viewer.

## Phase 3 — The Wire
**Question: is every capability reachable over the protocol, provably in sync across languages?**
*(build-order step 8: `ludion-protocol`, `ludion-server`, `ludion-client`)*

Envelope, handshake with `schema_hash`, exhaustive dispatch through the single sim queue,
pause/step/set-speed, screenshot command, replay record/replay, TS type generation in CI.

**Exit criteria**
- Integration test: connect → spawn → `sim.step 10` → screenshot **byte-identical** to the
  in-process render of the same frame.
- Schema-drift CI check green (regenerate + git-diff clean); at least one testkit suite runs over
  the wire, not in-process.
- A recorded session replays to an identical `WorldHash` sequence.

## Phase 4 — Human Eyes
**Question: can a person watch, inspect, and poke a running game without any privileged access?**
*(build-order step 9: `editor/` Tauri + React viewer, read-only)*

Frame-streaming viewport (QOI, `latest_wins`), entity inspector via query commands, input injection
from the webview, connection/session panel. npm toolchain isolated to `editor/`; TS protocol types
consumed from Phase 3's generation.

**Exit criteria**
- Viewer attaches to a running example at ≥30 fps / 1080p on localhost, on Windows and macOS.
- The inspector shows live component values using only public protocol commands (parity audit:
  zero endpoints exist that the CLI couldn't also call).
- A recorded viewer session replays to an identical `WorldHash`.

## Phase 5 — The Proof
**Question: does the MVP forcing function actually pass?**
*(build-order step 10: `ludion-mcp`, `ludion new`, engine guide, THE test)*

MCP adapter over `ludion-client`, project template, the shipped `CLAUDE.md`-style engine guide,
example gallery polished to "one feature each."

**Exit criteria — Milestone 1 closes here**
- Claude Code, from the one-line prompt, builds Breakout with dynamic lighting to a passing golden
  suite with zero human intervention.
- A human attaches the viewer mid-run and watches.
- The full session replay is committed as an example.
- Success Checks 1–5 in `engine-pillars.md` reviewed and answered in writing.

---

## Phase 6 — Hands (editing)
**Question: can a level designer change the world, with the agent guarantee intact?**

First *mutating* editor tools: selection, transform gizmos, entity palette, undo/redo. All of it
rides the command core — undo is command inversion, every gizmo drag lands as `component.set`,
every session stays a replayable command script. Scene saves go through the canonical writer
(minimal diffs — the Pillar 4 promise gets its first real test). Native windowed mode lands here
too (winit present pass, append-only per architecture §3).

**Exit gate:** a designer performs a real edit session; the saved scene diff is minimal; the session
script replays; the parity audit still finds zero editor-private endpoints.

## Phase 7 — Designer Flow (scripting & content)
**Question: can a game designer iterate on behavior without touching the engine?**

`ludion dev` hot-reload loop polished (recompile + snapshot/restore, architecture §8), snapshot
migration diagnostics, the "LLM as the designer's compiler" workflow documented and exercised,
richer content types as the current game demands them (tilemaps, palettes, particles-lite).
First real second game built to force generality honestly.

**Exit gate:** behavior-change loop ≤ ~5 s end to end; a non-programmer following the guide ships a
gameplay tweak via agent + hot reload.

## Phase 8 — Opening the World (assets & sound)
**Question: can content come from outside the text formats without breaking the text-first truth?**

glTF import (deferred until now by the pillars), image/atlas pipeline, minimal audio. Imports are
*compilers into* the canonical text/asset formats — imported content is diffable, validatable,
and agent-editable like everything else.

**Exit gate:** an imported glTF scene passes `check-scene`, renders with goldens, and re-exports
canonically.

## Phase 9 — Hardening & Public Surface
**Question: would a hostile stranger — human or agent — succeed on day one?**

API freeze pass (rustdoc coverage, `#[non_exhaustive]` audits), performance budgets asserted in CI
(`profile --json` against budget files), fuzzing the protocol and scene parser, versioned protocol
evolution policy, crates.io publication, the "fresh agent, docs-only context" test from Success
Check 5 run against a clean model context.

**Exit gate:** a fresh agent given only shipped docs + examples builds a small game correctly on the
first try; a fresh human designer installs, attaches, and edits within an hour.

---

## Sequencing rules (why this order and not another)

1. **Determinism before pixels** (1 before 2) — goldens are meaningless on a flaky sim; retrofitting
   determinism is the most expensive refactor in engine history.
2. **Wire before viewer** (3 before 4) — the viewer must be born a pure client; if it appears first,
   it *will* grow private access.
3. **Proof before polish** (5 before 6+) — the MVP forcing function is the earliest moment the
   project can honestly fail; reach it with minimum invested capital.
4. **Editing after replay** (6 after 3/5) — undo/redo and session scripts are command-core features;
   building gizmos before the command core is how editors drift.
5. **Imports last among features** (8) — every phase before it strengthens the invariants that
   imports are most likely to violate.
