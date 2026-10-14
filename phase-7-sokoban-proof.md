# Phase 7 — THE test, second run (Sokoban)

The record of Phase 7 Wave 5's exit-criteria run (2026-07-11). The forcing function, per
`Plan/phase-7-designer-flow.md` decision 4: *a fresh agent, from a one-line prompt naming
`ludion dev` ("Make a Sokoban clone, iterating on it with `ludion dev`"), goes from `ludion new` to
a fully green suite with zero human intervention — dogfooding the designer loop, not demoing it.*

## What actually happened

A fresh agent was spawned with the one-line prompt plus **environment facts only** (binary path, an
empty directory outside the checkout, the pinned-GPU fact, the shared `CARGO_TARGET_DIR`), and an
explicit prohibition on reading the engine checkout. Every game-facing fact — components, systems,
scenes, the `Tilemap` API, recording, the dev loop — came from what `ludion new` generates
(`LUDION-GUIDE.md`, embedded from `docs/engine-guide.md`).

Result: a complete Sokoban clone — the level authored as **one built-in `Tilemap`** (walls, floor,
targets as glyphs; the renderer and the game rules read the same grid, exactly the design bet),
plus `GridPos`/`Player`/`Pushable`/`GameState` components and four systems (discrete push-rule
movement with edge-detected input, win detection with live `Text` state, transform snapping via
`cell_to_world`, a deterministic win-pulse light). **12/12 tests green on the first compile+run**:
9 gameplay (push semantics, wall/crate blocking, solve + R-reset, WorldHash determinism over a
scripted solve), drift-safety, a blessed golden, and an in-process record→replay hash round-trip.
fmt and clippy (`-D warnings`) clean. Imported byte-for-byte as `games/sokoban` (commit 814079f).

`ludion dev` was used for real, not ceremonially: the agent kept a dev host running while tuning,
and its **code swaps landed on the live world** (`dev reload restored frame=9724 in 3175 ms`) — it
verified over the wire that the restored snapshot carried old component state while the new binary's
code drove it, which is precisely the loop's contract. It then attached as a raw protocol peer,
played moves via `input.inject`, diagnosed text placement numerically with `render.bounds`, and
captured the win screen via `render.screenshot`. Its close: *"the core loop — change, test, read a
hash/SSIM/bounds diff, correct — held up exactly as promised; I never needed a human to look at the
screen."*

## Friction → action

| # | Finding | Verdict | Action |
|---|---------|---------|--------|
| 1 | Guide-derived APIs (`glyph_at`, `cell_to_world`, `replay_hashes_with`) all worked first try | praise | none — the examples-as-docs bet keeps paying |
| 2 | `scene.load` help says "resolved relative to the project root" but resolves against the **scene file's directory** (Phase 4 roots the engine at the scene's parent); the agent hit a doubled `scenes\scenes\` path and had to reverse-engineer it | **real defect** (misleading help, not wrong rooting) | Wave 6: reword the help/explain text to name the actual base ("the content root — the directory of the scene the engine was started with") |
| 3 | Raw wire protocol (hello/envelope) undocumented in the guide; three probe rounds to attach without MCP | doc gap | Wave 6: short "raw wire" appendix in the engine guide (hello shape, envelope, one example) |
| 4 | No way to render a *mid-game* state through the golden harness (goldens render initial state; live host + screenshot is the only path) | deferred | recorded as a testkit trigger: "run commands, then render" helper when a second game wants it |
| 5 | `ludion dev` watches `src/` only; scene edits silently don't reach the live world (restore preserves the old world) — the agent ran stale scene data before understanding why | UX gap | Wave 6: dev also watches `scenes/` and prints a hint line (no auto-reload — the world is the designer's state) |
| 6 | `PointLight` intensity/attenuation scale unguessable from the component docs; three bless-and-eyeball rounds | doc gap | Wave 6: one calibration sentence in the guide's PointLight row |
| 7 | `Text` has no anchor/align control; width discoverable only via live `render.bounds` | deferred | trigger recorded: alignment fields when a game needs centered UI text |
| 8 | Scaffold derive-line formatting fights `cargo fmt` after adding a derive; clippy `collapsible_if` on a natural nesting | nit | Wave 6: pre-format the template |

## What this proves for the exit gate

The Phase 7 question — *can a game designer iterate on behavior without touching the engine?* — has
its agent-side half answered: the loop is real (watch → build → snapshot → restart → restore,
~3.2 s debug, measured), survivable by clients, and a from-docs agent both used and validated it.
The human half (a non-programmer following `docs/designer-flow.md` ships a tweak) is Wave 6's gate.
