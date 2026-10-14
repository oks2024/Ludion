# Plan: Phase 7 — Designer Flow (scripting & content)

> **CLOSED — shipped 2026-07-11; Wave 5's exit-criteria run is recorded in
> `phase-7-sokoban-proof.md` ("The record of Phase 7 Wave 5's exit-criteria run (2026-07-11)") and
> the loop gate ships as `crates/ludion-cli/tests/phase7.rs`.**
> Status: **next phase (Phase 6 gates only on humans).** Question: can a game designer iterate on
> behavior without touching the engine? Exit gate (implementation-guide): behavior-change loop
> ≤ ~5 s end to end; a non-programmer following the guide ships a gameplay tweak via agent + hot
> reload. Mechanism is settled: recompile + restart-with-snapshot (architecture §8), never dylib
> reload.

## Current-code facts that shape the work

- **No `dev` command exists** (`ludion-cli/src/main.rs:17` defers it; §8 fully specifies the loop).
  The wire already carries what the supervisor needs — `engine.snapshot`/`engine.restore` are 2 of
  the 25 frozen methods — and the snapshot needs only world+RNG+frame+run-state, because systems
  are plain `fn` pointers the Engine re-registers into a fresh `Schedule` at every world reset
  (Phase 5 decision): a restarted binary brings its own code.
- **Restore hard-fails on any component-schema drift**, despite §8 requiring migration:
  `ludion-engine/src/snapshot.rs:117-134` rejects incompatible documents with L3008, and
  `deserialize_insert` propagates L0004 (component no longer registered,
  `ludion-ecs/src/registry.rs:251`) and L0006 (value no longer deserializes) as hard errors.
  `engine.restore` returns bare `Ack` (`commands/engine_cmds.rs:32`) — no room for a report.
- **Serde-default discipline is real for built-ins, absent for games.** Every built-in field carries
  `#[serde(default)]` (`ludion-engine/src/components.rs:38-207`) *except* `Text.text`; breakout's
  four components have one defaulted field between them. Nothing enforces or measures it.
- **There is no texture cache anywhere.** `extract()` re-reads every PNG from disk on every extract
  (`ludion-engine/src/extract.rs:336`) and the renderer re-uploads every texture per render
  (`ludion-renderer/src/render.rs:344-348`); `TextureImage.hash` is documented as "the renderer's
  texture-cache key" (`scene.rs:109`) but nothing consumes it. So a texture edit already shows
  *while running* — accidentally, at per-frame disk-IO cost — but never while paused (the
  `frame_dirty` gate, `live.rs:352`). `asset.reload` only validates loadability
  (`commands/asset.rs:23`: "there is no cache to drop"); `ludion-assets/src/lib.rs:17`: no watcher
  yet. The `asset.reloaded` event variant already exists on the wire.
- **No client survives a restart:** the editor deliberately has no auto-reconnect
  (`editor/app/src/protocol/client.ts:10-11` — "Reconnection policy belongs to the caller");
  `ludion-window` treats a lost connection as fatal and returns from `run` (`lib.rs:31,107-108`).
- **Stopping a hosted session cleanly from a script is awkward on Windows** (milestone-1-proof
  friction #3). There is no shutdown command on the wire; dev's kill step would be a hard kill.
- **Budget context:** warm `cargo check -p breakout` after a game edit ≈ 2.1 s (milestone-1-proof).
  The 5 s loop must also fit a debug build+link, snapshot, process restart, and restore.
- Both hosts print the machine-parseable `listening on ws://…` first line (`serve.rs:78`) — the
  precedent dev's own status lines follow. Additive protocol revs are safe; the re-bless order is
  recorded in a private notebook (Phase 6 decisions, last bullet).
- Taken L-codes (`ludion-core/src/codes.rs`): L0001–L0011, L1001–L1018, L1101–L1102, L2001–L2007,
  L2101–L2102, L2201–L2204, L3001–L3014, L4001–L4003.

## Design decisions (settled by this plan)

1. **`ludion dev` is a supervisor: a peer protocol client + child-process manager** in `ludion-cli`.
   Loop per §8: notify-watch the game's `src/` (debounced ~300 ms) → `cargo build` → on success
   `engine.snapshot` over the wire → stop child → restart on the same port → `engine.restore` —
   every engine interaction an ordinary wire command, parity by construction. **It runs from the
   game's directory**: `cargo build` / `cargo run --quiet` with cwd = project dir works identically
   for workspace members and standalone `ludion new` scaffolds — no `-p` detection logic.
   `ludion dev [--scene S] [--addr A] [--window]` forwards `--scene`/`--addr` to the child (scene
   optional, falling back to the builder's `default_scene`); L0012 if cwd is not a runnable game
   bin. A failed build never kills the running child — the designer keeps playing the last good
   binary while cargo's errors pass through. Restarted children launch `--paused`; the restore
   installs the snapshot's run-state, so no fresh-scene frames tick between listen and restore.
   Machine-parseable status lines (stdout, one per step): `dev watching <src-dir>`, the child's
   `listening on ws://…` relayed verbatim, `dev reload building`, `dev reload build failed`,
   `dev reload restored frame=<N> in <ms> ms` — the last is what the exit-gate test times.
   Rejected: a `cargo watch` dependency (the snapshot/restart choreography between build and run is
   the actual work); a connection-holding proxy (§8 accepts the 1–2 s blink for v1).
2. **`engine.shutdown` joins the wire.** The supervisor's "stop child" step becomes a protocol
   command: flush recordings, close connections, exit 0 — exactly what Ctrl-C does. This closes
   friction #3 (scripted session stop on Windows) for every client, not just dev, and keeps dev's
   loop 100% wire-visible. Hard-kill remains the fallback on timeout. Rejected: dev sending a
   platform signal (Windows has no clean cross-process Ctrl-C; and a capability only dev has would
   be a parity hole).
3. **Restore stops hard-failing on drift; the migration report rides the response.** Per-entity,
   per-component: an unregistered component → dropped with L3015; a value that no longer
   deserializes → dropped with L3016 (serde error + entity id); added fields fill from serde
   defaults inside deserialization, with the filled/removed *field names* computed honestly by
   re-serializing the inserted value and diffing keys against the snapshot's. `engine.restore`
   returns new `ResponseBody::Restored { report }` instead of `Ack` (empty report = clean restore) —
   dev prints it, tests assert on it; L3008 remains only for genuinely malformed documents.
   Rejected: default-filling a whole undeserializable component (systems then act on invented
   state — absent + a loud report is more honest); a separate migration-report query (state
   outliving the response is a second source of truth). *Discipline:* new testkit helper
   `assert_components_drift_safe(registry)` probes each component with `{}` — one that can't build
   from an empty object can't absorb future field additions. `Text.text` gets a `""` serde default
   so built-ins pass; the scaffold gains the test; the guide documents the rule.
4. **Second game: a Sokoban-like, agent-built via the THE-test pattern** — fresh agent, one-line
   prompt, docs-only, zero intervention — with `ludion dev` named in the prompt so the loop is
   dogfooded, not demoed. Content types are demand-driven from it: **`Tilemap` becomes a built-in
   component, authored inline in the scene** — `cell_size`, a `legend` (glyph → tile appearance,
   reusing Sprite's color/texture fields), `rows` as a list of equal-length strings. Strings are
   scalars, so the strict-YAML subset and canonical writer handle it as-is; a level edit is a
   glyph-grid diff — the minimal-diff pillar at its best. `extract()` expands cells into the
   existing sprite batcher (no new GPU work); game systems read the same component for
   walls/targets; it enters WorldHash via the registry (new scenes only — no re-bless). Validation:
   L1019 ragged rows, L1020 glyph not in legend (JSON Pointers). Boxes/player stay ordinary
   entities. Rejected: a separate tilemap file format (a second text format + loader for a
   milestone needing one grid; trigger: tilemaps shared across scenes); per-cell sprite entities
   spawned by the game (hundreds of entities polluting queries, saves, and diffs). **Palettes and
   particles-lite are deferred** unless the game demands them; triggers recorded: palettes when
   color sets want sharing across scenes, particles when feedback exceeds what sprites fake.
5. **Asset watcher + a real `asset.reload` (the artist half).** `ludion-assets` gains the notify
   wrapper (`watch(root, debounce) -> Receiver<Vec<String>>`, watcher thread per §3); the engine
   gains the cache reload finally has to invalidate: a CPU `AssetCache` (path → decoded `Image`)
   that `extract()` reads through, plus a renderer GPU texture cache keyed on the content hash it
   was always documented to want (upload on miss, evict entries unreferenced by the current frame).
   Hosts (`ludion serve`, `ludion::run`) spawn the watcher and forward changed paths into the one
   command intake as ordinary `asset.reload` commands — filtered to paths present in the cache, so
   editor temp files are ignored. The handler drops the cache entry, reloads, fires the existing
   `asset.reloaded {path, hash}` event, and dirties the frame so a *paused* engine re-renders
   (fixing today's gap). Asset bytes never enter WorldHash, so reload is sim-invisible and
   replay-safe. L1103 for a watcher that fails to start; watcher failure degrades to explicit
   `asset.reload`, never a dead host.
6. **Clients survive the dev blink: auto-reconnect + re-subscribe.** Editor: reconnect loop in App
   (client.ts's documented "caller policy" seam) — bounded backoff, on success re-subscribe frames/
   scene.changed/log; undo history already clears on connection change. `ludion-window`: stream and
   input threads retry with the same bounds instead of ending the loop, holding the last frame and
   flagging the title. This is what lets **`ludion dev --window` host the window in the dev process
   itself** (supervision moves to a worker thread; winit keeps main) — one persistent window across
   restarts, children always headless. Rejected: passing `--window` to the child (the window dies
   and reopens every reload).
7. **The designer-facing workflow doc is an agent-docs topic: `docs/designer-flow.md`** ("LLM as
   the designer's compiler", pillar language from `engine-pillars.md:87`) — what to ask the agent,
   how to run `ludion dev`, what the reload lines mean, written for a non-programmer; served by
   `ludion docs designer-flow`. The engine guide gains the agent-facing half (an "iterating with
   ludion dev" section + Tilemap chapter). Rejected: a chapter only inside the engine guide — that
   document is the *agent's* context budget, and the human gate needs a doc a human actually reads.

## Waves

### Wave 1 — Snapshot migration + the one schema rev (engine/protocol; groundwork)
- Migration semantics per decision 3: L3015/L3016 report entries, filled/removed-field diff,
  `ResponseBody::Restored { report }`; L3008 narrowed to malformed documents.
- `engine.shutdown` (params, dispatch arm, handler; graceful path shared with Ctrl-C). Method
  surface 25 → 26.
- `Text.text` serde default; `assert_components_drift_safe` in testkit; scaffold template gains the
  drift-safety test; guide documents the rule.
- One schema rev, landed here and never again this phase: bless snapshot → `EXPECTED_SCHEMA_HASH` →
  TS regen → re-record `games/breakout/run.lrp` last (the recorded order).
- **Tests:** restore of a doctored snapshot (unknown component / broken value / missing field)
  yields the exact expected report and a running world; clean restore reports empty; shutdown over
  the wire flushes an armed recording (cross-process, the friction-#3 regression test).

### Wave 2 — `ludion dev` (supervisor, headless)
- `dev.rs` per decisions 1–2: watcher, debounce, build, snapshot, shutdown-or-kill, restart
  `--paused` on the same port, restore, status lines; L0012/L0013 (not a game project / child
  failed); works from a scaffold dir and from `games/*`.
- **Tests:** integration test drives a scratch scaffold — touch `src/lib.rs`, assert the status-line
  sequence, assert component state survives the restart (snapshot fidelity through a real process
  boundary), assert a build-breaking edit leaves the old child serving. First loop-time measurement
  logged here (not yet gated) so the 5 s risk is data, not hope.

### Wave 3 — Asset watcher + real reload
- `AssetCache` in engine (extract reads through it), GPU texture cache keyed on content hash,
  watcher in `ludion-assets` (L1103), host wiring, reload → invalidate + `asset.reloaded` + frame
  dirty. Goldens must stay byte-identical (cache is a pure lookup change).
- **Tests:** paused engine + overwritten PNG → `asset.reloaded` fires and the next streamed frame
  differs (the today-impossible case); cache eviction on reload; unreferenced-path events ignored;
  full gallery goldens untouched.

### Wave 4 — Reconnect (editor + window) + `dev --window`
- Editor reconnect loop + re-subscribe; `ludion-window` retry loops; dev hosts the persistent
  window per decision 6.
- **Tests:** editor vitest on the reconnect state machine; window keymap/retry units; integration —
  kill and restart a serve under an attached client, assert re-subscription resumes frames.

### Wave 5 — Tilemap + the second game
- `Tilemap` component (registry, schema, L1019/L1020 validation, extract → sprite batcher, engine
  guide chapter with a runnable example); golden for a tilemap gallery scene.
- **THE test, second run:** fresh agent, one-line Sokoban prompt naming `ludion dev`, docs-only →
  `games/sokoban` with green gameplay/WorldHash/golden/replay suite; friction report written up
  (the honest-generality forcing function). Content demands beyond tilemaps are triaged against
  decision 4's triggers, not built speculatively.

### Wave 6 — Exit criteria
- **Mechanized loop time** (`ludion-cli/tests/phase7.rs`): against a pre-warmed scratch game, touch
  a source file and parse dev's own status lines for the touch→`restored` wall time; assert
  ≤ 5 s in release, print-only in debug (profile-gated honestly, the phase4 fps precedent).
- `docs/designer-flow.md` written and exercised end to end by its author (agent-side dry run).
- **Human gate (user):** a non-programmer follows the doc and ships a gameplay tweak — describe the
  change, watch the agent edit + `ludion dev` reload it, see it live in the attached viewer/window.

## Risks

- **The 5 s budget is the phase's honest bet:** 2.1 s is only the *check*; debug link + restart +
  restore ride on top. Wave 2 measures before Wave 6 gates; if it misses, the written-down levers
  are dev-profile linker settings in the scaffold and snapshot-over-stdout instead of a second wire
  round-trip — not a relaxed gate.
- **Same-port rebind races** (TIME_WAIT, child not yet listening): dev waits on the `listening on`
  line with a timeout and retries the bind window; L0013 with the child's captured output when it
  genuinely dies.
- **Migration can bless silent wrongness:** a defaulted field a system depends on changes gameplay
  without an error. Contained: the report is loud in dev's output, and drift-safety is a designed
  property (testkit probe), not an accident.
- **Watcher storms** (editors write temp files, cargo touches `target/`): debounce + cache-membership
  filter for assets; `src/`-only watch + build coalescing for dev. Flaky-watch behavior is a P0 bug
  under the determinism pillar's flake rule.
- **Reconnect can mask real failures** (a crashed engine looks like a long blink): bounded retries,
  visible state in both clients, and dev's own status lines say whether a restart is in progress.
- **Schema-rev ripple** is contained to Wave 1 by design; the re-bless order is already written down
  and `run.lrp` re-records last.
- **Second-game scope creep:** the vertical-slice rule applies — tilemaps are in because Sokoban
  cannot exist without them; palettes/particles stay out until a game, not a plan, demands them.
