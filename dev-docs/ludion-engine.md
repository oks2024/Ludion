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

- `crates/ludion-engine/src/lib.rs` — crate doc and the re-export surface; the list is the file.
- `crates/ludion-engine/src/engine.rs` — the `Engine`: `SimDriver` (which owns `World` by value), `Registry`, seed, scene ref, project root, render size, lazy GPU `Renderer`, optional window `PresentTarget` (`attach_surface`, `present_attached`), subscription table, optional `Recorder`; `execute()`, `load_inline_scene`, `set_recording`/`stop_recording`, `world_hash`, `advance_live`, `render_frame(frame, readback, present)`.
- `crates/ludion-engine/src/dispatch.rs` — one exhaustive `match` over every `Command` variant → `Dispatched { body, frame, game_frame }`. 79 lines, all routing.
- `crates/ludion-engine/src/commands/` — the handler logic, one module per method family (`scene`, `entity`, `sim`, `render`, `stream`, `asset`, `engine_cmds`); `mod.rs` has `collapse()`, `Vec<Diagnostic>` → one wire diagnostic with `related`.
- `crates/ludion-engine/src/hierarchy.rs` — `world_transform`/`world_affine` (the derived world pose, never stored), `reparent` and `Keep`, the `L1218`–`L1225` faults and the `L1260` reparent refusal, the `hierarchy` debug-draw lines.
- `crates/ludion-engine/src/particles.rs` — the particle motion kernel (`Motion`, closed-form `position_at`) and the analytic `ParticleEmitter` evaluator (`evaluate`); pure functions of `(component, Transform, SimTime)`, no world/resource/clock reads, read fresh by `extract.rs` every frame.
- `crates/ludion-engine/src/manifest.rs` — the `ludion.toml` reader (`load`, `find`, `Manifest::voice_caps`, `L0023`/`L5004`); read by `export`, `sync` and every game host.
- `crates/ludion-engine/src/base64.rs` — a tiny RFC 4648 encoder for the `render.screenshot` payload, so `ludion-protocol` needs no base64 dep.
- `crates/ludion-engine/src/live.rs`, `crates/ludion-engine/src/egress.rs`, `crates/ludion-engine/src/diagnostics.rs` — `ludion devdocs engine-live-loop`.
- `crates/ludion-components/src/lib.rs` — `ludion devdocs engine-components`.
- `crates/ludion-engine/src/render_extent.rs`, `crates/ludion-engine/src/render_size.rs`, `crates/ludion-engine/src/render_view.rs` — `ludion devdocs engine-present`.
- `crates/ludion-engine/src/perf_live.rs`, `crates/ludion-engine/src/profile.rs` — `ludion devdocs profiling`.
- `crates/ludion-engine/src/recording.rs`, `crates/ludion-engine/src/replay.rs`, `crates/ludion-engine/src/snapshot.rs`, `crates/ludion-engine/src/bisect.rs` — `ludion devdocs engine-replay`.
- `crates/ludion-engine/src/docs.rs`, `crates/ludion-engine/src/player_profile.rs`, `crates/ludion-engine/src/transition.rs`, `crates/ludion-engine/src/query_drift.rs` — `ludion devdocs engine-content-kinds`.
- `crates/ludion-engine/src/residue_store.rs` — `ludion devdocs engine-residue`.
- `crates/ludion-engine/src/voxelizer.rs`, `crates/ludion-engine/src/voxel_sheet.rs` — `ludion devdocs engine-voxelizer`.
- `crates/ludion-engine/src/sculpt_mesh.rs` — the `kind: sculpt` loader: bytes → `check_sculpt` → `ludion_sculpt::mesh` → `MeshData` keyed by content hash, the `voxel_mesh.rs` shape; `sculpt_budget_warning` is this crate's `L1112` re-check on the meshed vertex count. The mesher itself is `ludion devdocs ludion-sculpt`.
- `crates/ludion-engine/src/assembly_mesh.rs`, `crates/ludion-engine/src/asset_cache.rs` (the `dependents` map of source-file edges), and `extract.rs`'s `part_placements` — `ludion devdocs assembly-animation`.
- `crates/ludion-engine/src/cooked_maps.rs` — `TextureSource` and the cooked-map lookup behind `Engine::set_textures` (`source` decodes PNGs, `cooked` reads the `ludion sync` store of the first GPU-supported target that has one, and falls back per map with `L1271`; a decal's maps always read their PNG, since sync cooks only what mesh documents name); a host switch like quality, never hashed. `cook_resolved == false` is the one retry lever: a rebuild, `set_textures` or an `asset.reload` of a `.ludion/cooked/` file re-resolves the lookup and re-tests the held fallbacks. Hosts: `ludion docs quality`.
- `crates/ludion-engine/src/extract.rs`, `crates/ludion-engine/src/render.rs`, `crates/ludion-engine/src/ui.rs`, `crates/ludion-engine/src/model_sheet.rs`, `crates/ludion-engine/src/sweep.rs` — `ludion devdocs engine-image-paths`.
- `crates/ludion-engine/tests/` — `dispatch.rs` (per-family happy + diagnostic paths, no GPU), `gpu_dispatch.rs`/`gpu_render.rs` (graceful-skip on `L2003`), `extract.rs`, `dump_graph_snapshot.rs` + `snapshots/`, `docs.rs`, `docs_reload.rs`, `presentation.rs` (presentation entities through snapshot, replay, extract), `transition*.rs`; all compiled as modules of `tests/it.rs` except `thread_identity.rs`.

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
