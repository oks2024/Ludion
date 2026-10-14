# Milestone 1 — The Proof

The record of Phase 5's exit-criteria run (2026-07-10) and the written answers to Success Checks
1–5 from `engine-pillars.md`. The MVP forcing function: *Claude Code, given a one-line prompt
("make a Breakout clone with dynamic lighting"), goes from `ludion new` to a passing visual test
suite with zero human intervention — and a human can attach the live viewer to the running game
and watch, over the same protocol.*

## THE test — what actually happened

A fresh agent was spawned with the one-line prompt plus **environment facts only**: the path of the
`ludion` binary, an empty working directory outside the engine checkout, the fact that the machine
has the pinned GPU, and a shared `CARGO_TARGET_DIR` for build speed. Every game-facing fact —
components, systems, scenes, queries, determinism rules, the verification loop, recording — had to
come from what `ludion new` generates (the project `CLAUDE.md` and `LUDION-GUIDE.md`, embedded from
`docs/engine-guide.md`).

**Result: passed, zero intervention, ~12.4 minutes, 45 tool calls.** The agent scaffolded
`breakout`, wrote 4 components (`Paddle`, `Ball`, `Brick`, `BallGlow`) and 5 systems (paddle input
with wall clamp; ball physics with paddle "english"; least-penetration brick reflection driving a
glow flash; glow-follows-ball; exponential glow decay), a 15-brick scene, 9 gameplay tests, a
two-run WorldHash-sequence determinism test, a blessed golden it visually verified, and a recorded
session it replay-verified through `replay_hashes_with`. **The entire gameplay suite compiled and
passed on the first run.** It read engine source for exactly two things: a real rendering bug (below)
and how recording finalization works.

The artifact is committed as `games/breakout` (a578f0c): the game, its golden, and `run.lrp` — the
committed session replay, kept alive by `tests/replay.rs`, which asserts on every run that the
committed recording still replays to the identical live per-frame `WorldHash` trace.

**Integrity caveats, for honesty:** (1) the sub-agent harness auto-loads the engine repo's
`CLAUDE.md` into any agent spawned from this checkout — it describes engine internals and pillars,
not game-building APIs, and the agent's gameplay work is traceable to the generated guide, but the
run was not hermetically "docs only". (2) The bless-instruction bug in the scaffold (below) was
found and fixed by the supervising session *before* the run — a fresh-eyes casualty the test would
otherwise have hit.

## What the run found (friction → action)

1. **Engine bug — sprites drew at half their declared `Sprite.size`** (extract passed half-extent
   vectors into a shader whose unit quad already spans `[-0.5, 0.5]`), and **`Transform.scale` was
   silently ignored for sprites**. The agent noticed only because it *looked at* its blessed golden
   and cross-read the shader; no text diagnostic existed for it. Fixed at the source (e434b35):
   extract passes full `size × scale.xy` vectors, regression-tested; scene sizes re-authored so
   gallery/breakout renders stayed byte-identical (proven by unchanged goldens passing at SSIM 1.0).
2. **Scaffold bless instruction couldn't work** (`ludion test --bless` is built-in-registry-only —
   found pre-run by the supervising session). Fixed (dee00b5): `LUDION_BLESS=1 cargo test` blesses
   through the game's own test; the generated-project integration test now exercises the full
   out-of-box bless→compare loop.
3. **Stopping a hosted `--record` session from a script is awkward on Windows.** The agent used the
   public `Engine::set_recording` API in-process instead — the better agent workflow. Now documented
   in the engine guide as the recommended path.
4. **Wish: a world-space "what did I just render" bounds query** — would have caught the size bug in
   one text-only iteration instead of an image inspection. Deferred (frozen v1 command surface);
   candidate for the next protocol rev.

## Success Checks 1–5, answered

**1. Can the agent detect its own rendering mistakes without a human looking at the screen?**
Yes — two tiers, both exercised for real. Regressions against a golden are pure text: tiled SSIM
produces `L2203` with damaged-tile coordinates and compass regions (the broken-lights and
moved-sprite tests assert the localization is *correct*, not just present). First-render mistakes
have no golden to diff against, and there the agent reads the image itself — THE-test agent judged
its board from the blessed PNG and caught a real engine bug doing so. The honest gap: a
geometry-level introspection query (friction #4) would make that second tier text-only too.

**2. Is every failure mode expressible as text the agent can act on?**
Every *engineered* failure mode, yes: scene validation, protocol errors, golden regressions, replay
failures — all L-coded, all with what/where/why/fix (`ludion explain`), JSON Pointers into
documents, structured diff reports. The sprite-size bug is the counterexample that keeps this
honest: a silent wrongness produced no text at all until a golden existed. The system's answer is
the golden discipline itself — bless early, then every subsequent mistake is text.

**3. Could Claude Code build the current milestone game from a one-line prompt?**
Yes — done, not argued: one prompt, zero intervention, ~12.4 minutes to a fully green suite
(gameplay, determinism, golden, session replay), committed as `games/breakout`. With the caveats
recorded above.

**4. Is `cargo check` still fast enough for a 20-iteration fix loop?**
Yes, with headroom. Measured on the dev box, warm: `cargo check -p breakout` after editing the
game's `lib.rs` ≈ **2.1 s**; no-op workspace check ≈ **1 s**; a game's full headless test suite
(gameplay + determinism + replay, no GPU) ≈ **1 s** of test time. Twenty full change–check–test
iterations fit comfortably inside a couple of minutes. (Cold builds are minutes — the shared-target
convention in the docs exists for exactly that.)

**5. Would a fresh agent, given only the docs + examples in context, use the API correctly on the
first try?**
For gameplay code: demonstrably yes — THE-test agent's whole gameplay suite (queries, input edges,
`ctx.dt` time, spawn/despawn, custom+builtin component mixing, `drive_hashes`) compiled and passed
on its first run, and it reported the guide's recipes as "accurate and sufficient". The two places
it needed engine source were a bug (fixed) and a doc gap (recording finalization — now in the
guide). The bless-instruction bug found pre-run is the standing lesson: generated instructions rot
unless a test executes them, so the scaffold's out-of-box loop is now integration-tested end to end.

## Milestone 1 close

- **CLOSED 2026-07-23 by user direction.** Every mechanized criterion on both OSes and every
  hands-on mac gate validated: per-OS goldens (`goldens/windows/` RTX 3090 | `goldens/macos/` M4),
  60 fps at 1080p release on both, cross-arch `WorldHash`, per-OS profile budgets,
  windowed-serve / material-park PBR, `dev --window` + audio + hot reload, and the viewer attached
  mid-run by a human (the gate the previous revision of this section left open). The TeamCity
  mac-agent bring-up was the last standing item and was **decoupled from the close** — its
  scriptable half is shipped; the UI-side agent bring-up is a whenever-convenient task
  (`ci/teamcity/README.md` §"Bringing up a macOS agent").
