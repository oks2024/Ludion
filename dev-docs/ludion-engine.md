---
description: The command core hub: Engine, the exhaustive dispatch parity gate, and the crate's spokes
---

# ludion-engine

**Scope:** The `Engine` — the one command core and sole owner of the running world — the exhaustive
`dispatch.rs` parity gate and its `commands/` handlers, and the routing table to everything this
crate does in depth. Everything that happens between two commands — the thread, the frame-boundary
drains, the `egress` seam out of the sim thread — is `ludion devdocs engine-live-loop`.
The wire *contract* (envelopes, `Command`, replay container, schema
hash, the `frame` LFR1 format) is `ludion devdocs ludion-protocol`; the WS host is
`ludion devdocs ludion-server`; `ludion devdocs crate-graph` holds the boundary rules and
`ludion devdocs determinism` the reason this crate carries the strict clippy list.

## Spokes

- `ludion devdocs engine-layout` — the per-file map of `src/`, each file routed to its owning spoke.
- `ludion devdocs engine-live-loop` — the tokio-free live seam: `run_live`'s drain/tick/emit/park
  loop, the frame-boundary drains, the diagnostics channel, and egress fan-out.
- `ludion devdocs engine-replay` — recording, replay, snapshots, keyframes, divergence bisection.
- `ludion devdocs engine-content-kinds` — data-docs, player profiles, scene transitions, and the
  typed system-query declarations `engine.describe` publishes.
- `ludion devdocs engine-residue` — scene residue: capture and re-apply inside `perform`, the session
  map, and the `ResidueStore` mirror.
- `ludion devdocs engine-components` — the built-in components and `builtin_registry()`'s
  append-only registration order.
- `ludion devdocs engine-image-paths` — `extract()` and the three standalone file→image paths.
- `ludion devdocs engine-present` — the window present target and the `RenderExtent` resource.
- `ludion devdocs assembly-animation` — multi-part voxel models: the three document families, per-part
  meshing, the multi-source cache entry, and extract-time posing.
- `ludion devdocs engine-voxelizer` — the CPU voxelizer behind `GiVolume`: signature cache, combine
  rule, the point-sampling safety condition.

## Layout

- `crates/ludion-engine/src/engine.rs` — the `Engine`: `SimDriver` (which owns `World` by value), `Registry`, seed, scene ref, project root, render size, lazy GPU `Renderer`, optional window `PresentTarget`, subscription table, optional `Recorder`; `execute()`, `world_hash`, `advance_live`, `render_frame`.
- `crates/ludion-engine/src/dispatch.rs` — one exhaustive `match` over every `Command` variant → `Dispatched`. All routing.
- `crates/ludion-engine/src/commands/` — the handler logic, one module per method family.
- Every other file: `ludion devdocs engine-layout`.

## Rules

- **`ludion-engine` depends on `ludion-protocol` but never on `ludion-server`.** The Cargo.toml
  dependency list — core, ecs, scene, assets, sculpt, sim, renderer, protocol, crossbeam-channel — is
  the gate: no tokio, no tungstenite. The server plugs into the engine, not the reverse.
- **`dispatch.rs` contains no logic, only routing**, and there is **no wildcard arm** — the exhaustive
  match IS the parity gate, so a command without a handler is a compile error. When a handler grows,
  its logic moves into the owning module or domain crate, never into `dispatch.rs`.
- **The sim thread solely owns `World` — never `Arc<Mutex<World>>`.** The `Engine` holds the
  `SimDriver` (and through it the world) by value; `run_live` takes the `Engine` by value onto one
  thread. Every read and write — queries, pause/step, mutations — arrives as a `StampedCommand` on the
  one intake channel and applies at a frame boundary in the drain step.
- **This crate carries the strict clippy denylist** (no std `HashMap`/`HashSet`, no wall clock). The
  one scoped exception is `run_live`, the single legitimate wall-clock site: `Instant::now` decides
  only *when* to tick, never anything a tick observes.
- **`Engine::with_project_root` is the real construction path; `Engine::new()`'s `"."` default is a
  convenience for tests with no on-disk assets.** The project root is where `scene.load`'s relative
  path and every texture reference resolve, and every CLI entry point that loads a scene from disk
  constructs with it explicitly, rooted at the scene file's own directory.
- **Presentation systems (`EngineBuilder::presentation_system`) run after the physics triple** and own
  the presentation entity range (`Plan/presentation-state.md`). Extract and bounds walk both ranges;
  snapshots carry `presentation`/`presentation_allocator`; a presentation system may write only
  `Diagnostics` and `AudioQueue` (a follow-up widens it on demand), anything else is `L0028`.
  `Engine::drain_diagnostics` drains the world's queued `L0028` refusals first (`diagnostics::report_boundary_crossings`, sorted by key), so a discarded `Result` still logs once.

## Dataflow

The `Engine` is the state every command acts on: driver + world, registry, seed, scene reference,
subscription table, optional recorder, and a lazily-initialized GPU renderer (the first
`render.screenshot` requests the adapter, so a broken scene still fails CPU-side first). `dispatch()`
routes and stamps the frame the command applied at — a `sim.step` advances it, so a response
envelope's `frame` is always the post-command value. `Engine::execute()` is `dispatch()` plus
recording (`ludion devdocs engine-replay`). What happens between two commands — ticking, draining,
emitting — is `ludion devdocs engine-live-loop`.

## Verify

```
cargo test -p ludion-engine                      # dispatch, recording, replay, snapshot, extract, query-drift
cargo clippy -p ludion-engine -- -D warnings     # the determinism denylist
cargo run -p ludion-cli -- sim-hash --scene examples/gallery/meshes.yaml --frames 60 --record target/session.lrp
cargo run -p ludion-cli -- replay target/session.lrp --hashes
cargo run -p ludion-cli -- render --scene examples/gallery/lights.yaml --out target/lights.png
cargo run -p ludion-cli -- render --model examples/gallery/voxels/lantern.voxel.yaml --out target/lantern.sheet.png
```

`tests/dispatch.rs` needs no GPU; `gpu_dispatch.rs`/`gpu_render.rs` graceful-skip without an adapter
(`L2003`). The `replay` run must print the same final hash the recording run printed — that is the
round-trip check in miniature.

## Gotchas

- **A sweep sheet is a judging instrument, never a golden.** Its burned-in labels are `UiText`
  entities, so every cell schedules the overlay pass; a cell will not match the same
  `render --set` invocation byte for byte. Golden anything that matters through `render`, and use the
  sheet only to choose a value.
- `dispatch()` and `Engine::execute()` differ in exactly one thing: `execute` records. Drive a
  session you mean to record through `execute` (the live loop and CLI already do); calling
  `dispatch` directly silently bypasses the recorder.
- Arm `set_recording` **right after** loading the scene, before the first tick — the replay header
  snapshots the scene ref and seed at arm time, so it refuses (`L3047`) with no scene loaded or at a
  frame other than 0. A scene load resets the frame, so a session that has run re-loads before
  arming. (And for a wire session, arm before `Server::bind` — see `ludion-server`.)
- More traps (snapshot restore, keyframes, `asset.reload`, `extract()`, camera, `dump-graph`): `ludion devdocs engine-gotchas`.
