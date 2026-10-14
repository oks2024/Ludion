# Plan: Phase 6 — Hands (editing)

> **CLOSED — shipped; Wave 6's scripted designer session ships as
> `crates/ludion-server/tests/phase6.rs`, and CLAUDE.md §Current state records phases 0–11 closed.**
> Status: **active phase.** Question: can a level designer change the world, with the agent
> guarantee intact? Exit gate (implementation-guide): a designer performs a real edit session; the
> saved scene diff is minimal; the session script replays; the parity audit still finds zero
> editor-private endpoints. Native windowed mode lands here too (append-only, architecture §3).

## Current-code facts that shape the work

- The mutating primitives already exist on the wire: `entity.spawn/destroy`, `component.set/get`,
  `query.entities`, `scene.save`. The editor's tools mostly ride the frozen surface; the protocol
  rev below is small and additive.
- **Load→save is lossy today.** `world_to_scene` (`crates/ludion-engine/src/engine.rs:462`) emits
  `name: None` for every entity and for the scene itself — names survive instantiation nowhere.
  Pillar 4's minimal-diff promise fails before any gizmo exists. This is the phase's first fix.
- Mutations return bare `Ack`. A client cannot build an undo inverse without read-before-write,
  which races a running sim. Inverse data must ride the mutation response.
- `component.remove` does not exist on the wire (ECS command buffers support removal; the protocol
  never exposed it). Undo of an *inserting* `component.set` is impossible without it.
- THE-test friction TODO (deferred "for the next protocol rev"): a world-space rendered-bounds
  query — "what did I just draw, where." This phase **is** that protocol rev, and the same query is
  the editor's pick primitive. One command serves both audiences.
- `schema_hash` gates clients at connect; every client lives in-repo and regenerates from the
  committed snapshot. Additive surface growth is therefore safe.
- The render thread is currently the main thread (no winit). Architecture §3 already specifies
  windowed mode: present pass blits the offscreen texture to a winit surface, the event loop takes
  main, render moves to its own thread. Nothing else changes.

## Design decisions (settled by this plan)

1. **Entity names become a built-in `Name` component.** `instantiate` moves the scene's `name:`
   field into it; `world_to_scene` moves it back out to the `name:` field (never emitted as a
   component block). The scene-level `name`/format fields are retained on the Engine at load and
   written back on save. Rejected alternative: a side-map outside the ECS — invisible to
   snapshot/replay, would need its own persistence path.
   *Consequence:* `Name` is world content, so it enters `WorldHash` — every committed hash trace
   over a scene with named entities changes. Re-record/re-bless via the existing `LUDION_BLESS=1`
   mechanism (breakout `run.lrp`, any pinned hashes) in the same commit.
2. **Undo lives in the client; inversion is response-powered.** Mutating responses grow inverse
   data: `component.set` → `{previous: Value|null}`, `component.remove` → `{previous}`,
   `entity.destroy` → `{components: {name → value, …}}` (full serialized state at destruction).
   Undo/redo are then *ordinary protocol commands* — no `undo.*` method, no engine-side stack, and
   any client (agent included) gets the same affordance for free. Parity is preserved by
   construction. The editor's stack: undo of destroy respawns → fresh `EntityId` → the stack
   rewrites references to the old id in entries above it. Stack clears on `scene.load`.
3. **Protocol rev is additive; `protocol_version` stays 1.** New commands `component.remove` and
   `render.bounds`; enriched response bodies for the three mutations. `schema_hash` changes,
   snapshot + TS types regenerate, the METHODS↔enum sync test grows. `Ack`→structured is
   wire-breaking only for stale out-of-repo clients, which the connect-time hash check rejects by
   design.
4. **Picking = `render.bounds` + client-side hit test.** The command returns, from the last
   extract, per-renderable: entity id, world-space AABB, and the screen-space rect under the
   current camera. The editor picks by point-in-rect + nearest-depth; the agent reads the same
   answer as text ("did my sprite land where I meant"). No separate `query.pick` — one command,
   two audiences.
5. **Editor chrome never enters the engine.** Selection highlight and gizmos are a client-side
   overlay (canvas over the streamed frame), projected with camera math from queried
   `Camera`+`Transform`. Humans and agents keep seeing through the same eyes — the streamed pixels
   contain no editor state.
6. **Edit mode pauses the sim** (editor convention, not engine enforcement): entering edit mode
   sends `sim.pause`; play-testing resumes. Editing a running world stays possible over the wire —
   it's just not what the editor's edit mode does, because live systems overwrite what you drag.
7. **Gizmo drags:** live preview via throttled `component.set` (the inspector already round-trips
   at ~3 Hz; drags send at ~10–15 Hz), **one undo entry per drag** (value-at-mousedown →
   value-at-release). The recording keeps every intermediate set — a replay shows what happened,
   not the undo stack's abstraction. Quantization (snap) happens at the gizmo, per the settled
   pillar; the canonical writer stays lossless.
8. **Windowed mode = `--window` on `ludion serve` and `ludion::run`.** winit event loop takes
   main, render moves to its own thread, a present pass blits the existing offscreen texture.
   Window keyboard/mouse feed the same command intake as `input.inject` (same `InputState` path —
   one input truth). `run_lockstep` and all tests remain windowless; CI never opens a window.

## Waves

### Wave 1 — Save fidelity + protocol rev (engine/protocol, no editor)
- `Name` component (registry, schema, instantiate/world_to_scene round-trip); Engine retains scene
  `name` + seed. **Test: canonical scene → `scene.load` → `scene.save` → byte-equal.** Normalize
  any gallery scene the canonical writer would reformat (content-identical; goldens untouched).
- `component.remove` (params, dispatch arm, handler, L-code for "component not present"?
  — decide: removing an absent component is an error, matching `component.get`).
- Inverse-carrying response bodies for `component.set`/`component.remove`/`entity.destroy`.
- `render.bounds` (world AABB + screen rect per renderable from the last extract; L-coded error
  when no camera). Closes the THE-test friction TODO.
- Schema snapshot + TS regen; re-record `games/breakout/run.lrp` and re-bless affected hashes.

### Wave 2 — Editor edit mode core
- Edit/play mode toggle (pause/resume), selection via `render.bounds` hit test, selection overlay.
- Inspector becomes editable: field edits send `component.set`; schema-driven input widgets from
  `engine.describe` (numbers, vectors, colors, strings, booleans).
- Undo/redo stack (inverse responses → command stack, id-remap on respawn), Ctrl+Z/Ctrl+Y.
- Save button → `scene.save`; dirty indicator off `scene.changed`.

### Wave 3 — Transform gizmos
- Translate first (axis/plane handles), then rotate, then scale; screen-space overlay, camera
  projection math shared with the selection overlay.
- Snap settings (translate step, rotate degrees, scale step) applied at the gizmo; drag lifecycle
  = preview sets + one undo entry.

### Wave 4 — Entity palette
- Spawn from the component registry (`engine.describe` schemas → default component sets; a small
  curated palette of starting archetypes: sprite, mesh, light, camera, text).
- Duplicate selection (`component.get` sweep → `entity.spawn`), destroy with undo.
- Hierarchy view (existing `Parent` component); reparent = `component.set` on `Parent`.
  **Open question to resolve here:** destroy semantics for entities with children (engine behavior
  today is unverified — investigate, decide, test; a dangling `Parent` must at minimum be a
  readable diagnostic, not a panic).

### Wave 5 — Windowed mode (independent of waves 2–4; can land any time after 1)
- winit dependency confined to a new present layer (crate placement per §2 rules — renderer never
  sees winit types if a thin `ludion-window` crate is cheaper than polluting `ludion-renderer`).
- `--window` flag: event loop on main, render thread spawned, present pass blits; window input →
  command intake. Close = graceful shutdown (recordings flush, same path as ctrl-c).

### Wave 6 — Exit criteria
- **Scripted designer session** (`ludion-server/tests/phase6.rs`): over the wire, as the editor
  would — load gallery scene, pause, pick via bounds, drag (throttled sets), spawn from palette,
  destroy, undo twice, save. Assert: (a) the saved file differs from the source by exactly the
  expected lines (fixture diff — the minimal-diff gate, mechanized); (b) the recorded session
  replays to the identical per-frame hash trace; (c) undo restored byte-identical component
  values.
- Parity audit extended to the new methods (viewer uses only generated protocol types; `new
  WebSocket(` count still 1; grep for any editor-private endpoint = zero).
- **Human gate (user):** a real edit session in the editor — drag, tweak, spawn, undo, save —
  then `git diff` the scene and eyeball that it reads as *what they did and nothing else*.

## Risks

- **WorldHash changes from `Name`** ripple into every committed trace over named scenes. Contained:
  one wave-1 commit re-records/re-blesses; the bless mechanism is established and explicit.
- **First-save normalization noise:** hand-authored scenes that aren't byte-canonical will one-time
  reformat on first save. Pre-normalizing the gallery in wave 1 makes the exit-gate diff honest.
- **Ack→structured responses** change the wire shape of three existing methods. All known clients
  are in-repo and regenerate; the handshake rejects stale ones loudly (L3003). Accepted.
- **winit takes the main thread** — restructuring `run_live`'s thread arrangement is the riskiest
  mechanical change. Mitigation: windowless remains the default and the only CI-exercised path;
  windowed is additive scaffolding around the same `Engine::tick`/`extract`/`render` seams.
- **Undo across structural changes** (destroy→undo→respawn id remap, `Parent` references) has
  real edge cases; wave 4's open question is scheduled *before* the exit-gate wave so the answer
  is designed, not improvised.
- **Scope creep via the palette:** archetype curation can balloon. Vertical-slice rule applies —
  the palette ships with exactly what the gallery scenes need.
