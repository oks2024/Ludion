# Ludion — Engine Architecture

> How the pillars become code. `engine-pillars.md` is the *why*; this document is the *what and how*.
> Every boundary here carries its rationale, because an engine built by an LLM for LLMs will be
> scrutinized harder than human-made software — "it seemed natural" is not a defense.

---

## 1. Decision Log

Decisions settled in design review, with rationale. These are not open questions.

| Decision | Choice | Why |
|---|---|---|
| ECS | **Custom, from scratch** | Full control over error messages, iteration-order determinism, and API regularity; zero dependency churn. Scoped by a freeze list (§4) so it stays milestone-minimal. |
| Wire protocol | **WebSocket + JSON envelope**, JSON-RPC-shaped but our own | Agents can eyeball and hand-craft messages; MCP/tool-calling is JSON-native; JSON Schema falls out of the serde types; browser clients work forever. Own envelope because we need `frame` stamps and structured diagnostics JSON-RPC doesn't model. |
| Content files | **Strict-subset YAML 1.2**, canonical writer | LLM token efficiency (~30–40% fewer tokens than JSON for the same scene); line-oriented → a gizmo drag is a 1-line git diff; nests naturally (TOML degrades badly on nested scene graphs). Ambiguity ("Norway problem") killed by the canonical writer + schema validation (§7). |
| Wire encoding | **JSON stays on the wire** | Protocol messages are tiny — verbosity is irrelevant; interop and tooling win. One data model, two syntaxes chosen per consumer. |
| Project manifest | TOML (`ludion.toml`) | Flat config is TOML's home turf; matches Cargo idiom. |
| Viewer/editor | **Tauri + React**, out-of-process protocol client | LLMs build React UIs better than any other GUI stack (matters for an LLM-built engine); the WS+JSON protocol is webview-native; Tauri keeps the shell in Rust/cargo. TS protocol types are *generated* from JSON Schema in CI, so drift is impossible (§6). |
| Hot reload | **Recompile + restart-with-snapshot**, no dylib reload | Rust dylib reload is an ABI/TypeId/TLS tarpit; `abi_stable` would infect the public API (Pillar 6 violation). Snapshot/restore reuses scene serialization and doubles as replay keyframes (§8). |
| Windowing in M1 | **None — no winit** | The milestone's human requirement is satisfied by the Tauri viewer over frame streaming. Renderer always targets an offscreen texture; "headless is the default" becomes literal. Native windowed mode arrives later as an append-only present pass. |
| Platforms | **Windows + macOS, first-class from day one** | Drives the graphics-API choice (below); adds a second pinned GPU target for the bit-exact golden tier, an OS matrix in CI, and Tauri builds for both. Linux is untargeted for now (nothing forbids it; nothing is tested on it). |
| Graphics API | **wgpu — settled, held loosely** | Validation-as-`Result` keeps GPU mistakes readable by agents (raw Vulkan turns them into device-lost or silent corruption); native Metal backend on macOS, whereas ash would mean MoltenVK translation — a second bug layer and opaque debugging. Known costs accepted: wgpu owns barrier placement and sync; no mesh shaders; bindless immature. **Exit plan, not abstraction:** the frame graph's declarative interface (passes declare resources + dependencies; execution happens below) is the seam — if wgpu is outgrown, the execution layer is *replaced* with ash, and the graph, renderer, and golden tests survive. No preemptive backend traits (see §2). |

---

## 2. Crate Graph

Workspace of small, orthogonal crates. An agent working on any one subsystem should fit that
crate's full public surface in context.

```
ludion-core          (glam re-export, ids, Diagnostic + L-code table, SimTime/FrameIndex,
                      DeterministicRng, stable collections)
ludion-ecs           → core            custom ECS + component registry (name + JSON Schema + serde fns)
ludion-scene         → core, ecs       strict-YAML parse/emit, validator, canonical writer
ludion-assets        → core            CPU-only asset data, content hashes, notify watcher, reload events
ludion-sim           → core, ecs       tick loop driver, input events, run-state, WorldHash
ludion-projectiles   → core, physics, spatial
                                       pooled 3D projectiles: SoA pool, swept step, hit events
ludion-render-graph  → core  [wgpu]    virtual resources, pass scheduling, transient aliasing,
                                       dump-graph / dump-timings JSON
ludion-renderer      → core, render-graph, assets [wgpu]
                                       HD-2D pipeline, offscreen target, 3-deep readback ring
ludion-protocol      → core            Command/Response/Event enums, envelope, replay format, schemars
ludion-engine        → ecs, scene, assets, sim, renderer, protocol
                                       Engine struct, dispatch.rs, run drivers, session recording
ludion-server        → engine, protocol [tokio, tungstenite]
                                       WS host, QOI/LZ4 frame encoding, subscriptions
ludion-client        → protocol        typed WS client SDK
ludion-testkit       → engine, client  golden harness: SSIM compare, diff reports, bless flow,
                                       sim assertions
ludion               facade: `use ludion::prelude::*` — one crate name, one rustdoc root for games
ludion-cli   (bin)   → engine, server, client, testkit
                                       new / check-scene / render / test / profile / dump-graph /
                                       explain / replay / sim-hash / dev
ludion-mcp   (bin)   → client          MCP adapter for Claude Code
editor/              Tauri + React; npm isolated here; generated TS protocol types
games/, examples/    → ludion (+ ludion-testkit as dev-dependency)
```

### The command core lives in three places

This is the mechanical form of the parity gate.

1. **Contract** — `enum Command`, `enum Response`, `enum Event` in `ludion-protocol`,
   `schemars`-derived. This crate *is* the gate: a capability that has no variant here does not exist.
2. **Dispatcher** — one exhaustive `match` in `ludion-engine/src/dispatch.rs`:
   `fn dispatch(&mut Engine, Command) -> Result<Response, Diagnostic>`.
   One file, one match. Adding a `Command` variant without a handler is a **compile error** —
   the parity gate enforced by `cargo check`, not by policy review.
3. **Logic** — each match arm is ≤ ~10 lines delegating to a plain function in the owning domain
   crate (`ludion_scene::apply_patch`, `ludion_sim::set_run_state`, `ludion_renderer::screenshot`).
   **Convention: `dispatch.rs` contains no logic, only routing.** If a handler grows, its logic
   moves down into the domain crate. This keeps `ludion-engine` glue-sized forever.

### Boundary rules (cycle prevention)

Each rule exists to prevent a specific failure, not as tidiness:

- **`ludion-protocol` never names a concrete component type.** Component payloads on the wire are
  `serde_json::Value` + registry name, validated at dispatch via the registry. The moment `Transform`
  appears in a protocol struct, protocol→ecs pressure begins and ends in a cycle. Protocol depends
  only on `ludion-core` (for `EntityId`, `FrameIndex`, `Diagnostic`).
- **`ludion-ecs` never knows YAML exists.** The registry stores erased serde functions
  (`fn(&Value, &mut World, Entity) -> Result<(), Diagnostic>`); scene→ecs is one-directional.
- **`ludion-assets` is GPU-free.** CPU bytes + content hashes + reload events only. The renderer owns
  GPU upload and rebuilds on `AssetReloaded { hash }`. A `wgpu` type in `ludion-assets` would drag
  wgpu into the sim-side dependency tree and destroy compile times for content work.
- **`ludion-engine` never depends on `ludion-server`.** Engine exposes channels (command intake,
  event/frame egress); the server is a peer that plugs into them. Frame encoding (QOI/LZ4) happens
  in the server; the engine hands over raw RGBA + frame index.
- **Fixed-timestep *types* (`FrameIndex`, `SimTime`, `TickRate`) live in core; the loop *driver* lives
  in sim.** No loop in core, no time types in sim.
- **Per-component schemas live with the registry in `ludion-ecs`;** `ludion-scene` consumes them to
  validate documents; the full `engine.describe` export is assembled in `ludion-engine`.
- **`ludion-render-graph` is a wgpu frame graph, full stop.** No backend-abstraction traits, no
  generic resource types. It will have exactly one client for years; genericity would be
  flexibility for its own sake. The acknowledged possibility that wgpu is later swapped for ash
  changes nothing here: the migration seam is the graph's *declarative* interface (pass
  declarations, resource descriptions, dependency edges — keep these free of wgpu types where it
  costs nothing), and the swap would replace the execution layer wholesale. Abstracting it
  preemptively would be paying for the migration every day without ever migrating.
- Bins (`ludion-cli`) may depend on both sides of any boundary — leaves are allowed to be fat.

### Diagnostics

`ludion-core` defines `Diagnostic { code, message, paths/spans, help, related }` and hosts a single
static table (`codes.rs`) mapping every `L####` code to its long-form explanation.
Ranges per domain: `L0xxx` core/ecs, `L1xxx` scene/assets, `L2xxx` render, `L3xxx` protocol/server,
`L4xxx` sim. A centralized const table (not `inventory`/`linkme` collection magic) is grep-able and
agent-legible; `ludion explain L1042` is a table lookup. A unit test asserts every constructed code
exists in the table. Scene diagnostics point into documents with JSON Pointer paths
(`/entities/3/components/Sprite/texture`).

---

## 3. Threading / Process Model

**Processes:** engine (one per running game / CLI invocation), Tauri viewer, MCP adapter — all
separate, all speaking the protocol.

**Threads in the engine process (live mode):**

1. **Sim thread** — synchronous, *sole owner of `World`* and the only thread that drives the tick.
   Loop: `drain_commands() → apply_inputs() → run_systems(SimDt) → extract() → render + submit`.
   A system may fan its own per-entity work out over scoped workers via `ludion_core::par` (§5
   rule 5); those workers borrow disjoint slices and join before the system returns, so ownership
   and the sequential schedule are unchanged.
   Fixed 60 Hz tick. Wall clock is used only by this thread's scheduler to decide *when* to tick —
   it never enters sim state.
2. **Tokio runtime** (2 workers, confined to `ludion-server`) — WS accept/decode; pushes
   `StampedCommand { cmd, reply: oneshot::Sender<Response> }` into a crossbeam channel; the sim
   thread replies via the oneshot (sync-side send is non-blocking; the bridge is race-free by
   construction).
3. **notify watcher thread** — asset-change events into the same command intake as everything else.
4. **Presenter thread** (windowed hosts only) — render-only: owns the window surface and its own
   renderer, fed latest-wins extracted scenes, and paces itself to the display (`Fifo`). It never
   touches `World`, `Engine` or a command (`Plan/display-paced-present.md`).

The stream's rendering runs on the sim thread, inside `emit_egress` after the tick; when windowed,
the main thread runs the winit loop and only forwards input, and the window is drawn by the
presenter thread, never the sim thread.

**Sim→render handoff:** `extract()` produces a `RenderScene` — transforms, sprites, lights, camera
as plain data, no ECS references. The sim thread renders it for the frame stream, and publishes the
same `Arc<RenderScene>` to the presenter as a `PresentPacket` (scene, size, render-side settings,
tick stamp) through a one-slot mailbox: latest wins, the sim never waits on the window. Stream
latest-wins happens downstream, at the egress frame slot. The renderer never touches `World`. This
is the only sim/render coupling.

**One queue, no exceptions.** ALL protocol traffic — reads, writes, `sim.pause`, `sim.step` — flows
through the single command queue and applies at frame boundaries. A paused engine keeps draining
and answering; it just doesn't tick. Consequences: no read/write races ever; queries always observe
a consistent frame boundary; every response carries `applied_frame` for free; pause–inspect–modify–
step debugging needs zero extra machinery. **Never introduce an `Arc<Mutex<World>>` shortcut for a
"quick" query path** — that single mutex would delete the entire correctness argument.

**Two drivers over the same functions:**

- `run_live()` — the threaded arrangement above (`ludion dev`, serving clients).
- `run_lockstep()` — one thread, fully sequential: `drain → tick → extract → render → blocking
  readback → compare/emit`. Used by `ludion render`, `ludion test`, golden tests, and replay
  verification. Determinism is trivial because there is no concurrency at all.

`Engine::tick()`, `extract()`, and `Renderer::render(&RenderScene)` are thread-agnostic; only the
~100-line driver differs.

**Readback without stalling:** ring of 3 `MAP_READ` staging buffers. Frame N issues
`copy_texture_to_buffer` + `map_async`; when mapped (typically N+2), the bytes leave the sim thread on
the bounded(1) egress lane. The server's frame router encodes each frame once per subscribed format
(QOI / LZ4_RGBA / RAW_RGBA) on the tokio blocking pool, and each connection's writer task sends it.
~2 frames of stream latency is irrelevant for viewing.
Exception: `render.screenshot` and lockstep mode use `device.poll(Wait)` — a deliberate stall,
because the answer must correspond to an exact frame index.

**Windowed mode:** the sim-side renderer keeps targeting the offscreen texture — the single source
of truth for goldens, `render.screenshot`, and the frame stream; for a game's own `--window` the
presenter thread renders the same extracted scene on its own renderer (same device) and blits it into
the winit surface at the display's rate, so the window costs the sim thread an extract it already
paid for and a pointer swap. The winit event loop takes the main thread. Nothing else changes —
windowed is an append-only feature, not a mode switch.

---

## 4. Custom ECS — Scope and Shape

**Freeze list (the complete v1 feature set):**

- Entities as generational indices (`EntityId { index, generation }`), deterministic free-list order.
- Component storage with defined, documented, conformance-tested iteration order.
- Typed queries (`world.query::<(&Transform, &mut Velocity)>()`), explicit and macro-light — plus
  their chunk-parallel form (`world.par_for_each::<(&mut Transform, &Velocity), _>(…)`,
  `par_join_fold`), which is the same join over `ludion_core::par` chunks.
- Explicit sequential system registration — systems run in registration order, period. This is a
  determinism rule, not a scheduling limitation.
- Component registry: `register::<T>()` records name, JSON Schema (`schemars`), and erased serde
  fns — powering scene IO, the inspector, snapshots, and protocol validation from one source.
- Hierarchy = a plain `Parent(EntityId)` component. No relation graphs; a scene rejects `Parent`
  cycles. Child lists are derived (`ChildMap`), never stored; `World::despawn`/`despawn_many`
  cascade (the named entity, then descendants in ascending slot; a sim entity's presentation
  descendants included) — ruling E1, `Plan/transform-hierarchy-and-prefabs.md`.

**Explicitly out (do not build):** parallel scheduler, archetype-graph cleverness, change-detection
ticks (the inspector polls queries at low Hz until a real need appears), relations, exclusive
systems, world splitting. Chunk-level data parallelism *within* one system is the one sanctioned
exception (via `ludion_core::par`, per `Plan/deterministic-job-system.md` J0/J1) — its ECS entry
points are `World::par_for_each`/`par_join_fold`; a system-level parallel scheduler stays out.
**Budget 3000 code lines under `crates/ludion-ecs/src`** (non-blank, non-comment, outside the test
module; gated by `scripts/ecs-budget.mjs`, raising it is the user's decision). The original ≈ 2–3k
was set for the sequential v1 and the crate sat at ≈ 2.9k against it; J1 (user unfreeze, 2026-08-01)
deliberately spends the difference on the parallel region machinery (`par_query.rs`) — a one-off
raise for a named plan, not a standing licence to grow. Conformance tests are written *before*
storage internals: generational safety (stale `EntityId` is an error, not a hit on a recycled
entity), query iteration order.

---

## 5. Determinism Mechanics

The rules all sim-side code follows (core, ecs, scene, sim, game crates) — each mechanically
enforced, not aspirational:

1. **No std `HashMap`/`HashSet`.** Use `ludion_core::collections::{StableMap, StableSet}`
   (BTree-backed). Their contract is *sorted key order*, not merely stable order: ECS storage is a
   pair of dense columns (`keys: Vec<u32>` strictly ascending, aligned with `values: Vec<T>`) and
   the query join is a sorted merge that silently drops rows on an unsorted column, so an
   insertion-ordered backing store (indexmap) is not a legal substitute — and the ascending keys are
   also what let a parallel chunk take a contiguous, disjoint `&mut` sub-slice per column. ECS
   iteration order is defined and conformance-tested.
2. **No wall clock in sim.** `std::time::Instant`/`SystemTime` banned; time reaches systems only as
   `SimTime`/`FrameIndex` parameters.
3. **One seed, one master RNG, derived streams only through the sanctioned constructor.**
   `DeterministicRng` (PCG32) as an ECS resource, seeded from the scene/replay header; the seed
   itself rides alongside as `WorldSeed`. A draw whose order must not couple distant systems builds
   a throwaway stream with `DeterministicRng::derived(world_seed, frame, entity)` (or
   `derived_salted` for a non-entity draw) — never by hand-seeding a generator from ad-hoc numbers.
   `rand::thread_rng` banned.
4. **Transcendentals via pure-Rust `libm`,** wrapped in `ludion_core::math` (`sin/cos/atan2/pow`…).
   Platform libm implementations differ across OSes; the pure-Rust one doesn't. Plain arithmetic and
   glam are fine (Rust does not auto-contract FMA).
5. **Systems run sequentially in explicit registration order.** There is no parallel scheduler; no
   system ever runs concurrently with another. Data parallelism *inside* a system is sanctioned
   only through `ludion_core::par` (fixed chunk boundaries, disjoint `&mut` chunks, ordered
   reductions, deterministic panic propagation) — its results depend on the chunk size, never on
   the thread count. At the ECS level the entry points are `World::par_for_each` (element-wise
   writes; the query's `&mut` columns are lifted out of the world for the region) and
   `World::par_join_fold` (read-only, ordered map-reduce). `World` is `Sync` only so those chunks
   can share `&World`; the sim thread still solely owns it and `Arc<Mutex<World>>` stays banned.
   Ad-hoc `std::thread` spawns/scopes and rayon stay banned in sim crates; see
   `Plan/deterministic-job-system.md` and `dev-docs/determinism.md`. The worker count defaults to the
   machine's available parallelism capped at `par::AUTO_THREAD_CAP` (8, the top of the identity
   gate's range) and is set per-machine by `LUDION_THREADS` or per-run by a host's `--threads`, so
   the parallel path is what ordinarily runs — including under `cargo test`.
6. **Deterministic entity allocation** — free-list pop order defined and tested.

**Enforcement:**

- Per-crate `clippy.toml` `disallowed-types` / `disallowed-methods` (sim-side crates get the strict
  list; renderer/server get a permissive one) + `cargo clippy -- -D warnings` in CI → violations are
  build failures.
- `rust-toolchain.toml` pins the compiler. Goldens and hashes are versioned against the pin;
  toolchain bumps are deliberate events that re-bless hashes.
- `#![forbid(unsafe_code)]` in every crate except the `ludion-shader-bindgen` tool (which has no
  `unsafe`) and the `ludion` facade, which `deny`s it and allows it only for the C-ABI embedding
  surface (`ffi.rs`); a game may `deny` it and allow the one symbol its native host calls.
- **Three-layer CI determinism test:**
  1. In-process double run — same seed + script twice, compare per-frame `WorldHash` sequences
     (catches per-instance `RandomState` leaks).
  2. Cross-process — two invocations of `ludion sim-hash --scene s --script i --frames 600`,
     compare final hash (catches ASLR/env-dependent nondeterminism).
  3. Replay round-trip — record a session, replay it, compare hash sequences.
- **`WorldHash`** = canonical world serialization (registry order, entity-index order) → xxh3.
  Exposed as `ludion sim-hash` (CLI) and `sim.hash` (protocol): a first-class *agent-facing*
  verification primitive, not just CI plumbing.

**Honesty tier:** bit-exact is guaranteed per (target, pinned toolchain). Cross-OS bit-exactness is
*measured* in CI as an informative signal, not a gate, for v1.

---

## 6. Wire Protocol

### Envelope

```json
→ {"v":1, "id":42, "method":"entity.spawn", "params":{...}}
← {"id":42, "ok":{...}, "frame":1203}
← {"id":42, "error":{"code":"L1042", "message":"…", "path":"/entities/3/components/Sprite/texture",
                     "help":"nearest match is 'brick_diff.png'"}}
← {"event":"scene.changed", "sub":7, "seq":981, "frame":1204, "data":{...}}
```

`frame` = the sim frame at which the command applied. Every mutating response carries it; it is what
makes agent reasoning about causality — and replay stitching — possible.

### Rust representation

One `#[serde(tag = "method", content = "params")] enum Command` in `ludion-protocol`, `schemars`
derive. One enum → exhaustive dispatch match (§2) → schema export → generated TS types. One source
of truth, three artifacts.

### Method surface (M1)

`scene.load / scene.save / scene.check` · `entity.spawn / entity.destroy` ·
`component.set / component.get` · `query.entities` ·
`sim.pause / sim.resume / sim.step / sim.set_speed / sim.hash` · `input.inject` ·
`render.screenshot / render.dump_graph / render.dump_timings` ·
`stream.subscribe / stream.unsubscribe` · `asset.reload` ·
`engine.describe / engine.snapshot / engine.restore`

- `sim.step {ticks: n}` runs exactly n ticks then pauses; returns the final frame and (optionally)
  `WorldHash`. This is the agent's single-step debugger. A paused engine still answers queries and
  mutations.
- `sim.set_speed` scales the live scheduler only; `dt` never changes (determinism).
- `engine.describe` returns the full machine-readable surface: component schemas from the registry +
  protocol schemas — Pillar 7's "machine-oriented reference" as an endpoint.

### Handshake and versioning

Client → `hello { protocol_version, client, features }`;
server → `{ engine_version, protocol_version, schema_hash }`.
`schema_hash` = hash of the full generated schema. A client generated from a stale schema detects
drift **at connect time**, not just in CI. v1 supports exactly one protocol version; bumps are
breaking by design.

### Subscriptions and frame streaming

`stream.subscribe {channel, params} → {sub_id}`; events carry `sub` + monotonic `seq`. Events are
ordered and never coalesced; a `seq` gap appears only when the client stops reading and its bounded
text lane overflows, so a drop is visible to the client. Channels: `frames`, `log`, `sim.events`,
`scene.changed`, `asset.reloaded`, `profile`.

Frames are binary WS messages — fixed header
`{sub_id, frame_index, format: QOI | LZ4_RGBA | RAW_RGBA, w, h}` +
payload. One subscriber mode:

- **`latest_wins`** (the viewer default): at most one pending frame per subscriber; unsent frames are
  replaced. Optional `max_fps`. Under load the viewer degrades to lower fps instead of building
  latency.
  There is no lossless mode: an agent that needs every frame drives `sim.step` + `render.screenshot`.

`render.screenshot` is a separate command returning one guaranteed-frame image through the same
render path. The stream and the screenshot share the same endpoint texture — **humans and agents see
through the same eyes**, per the pillars.

### Replay

The replay format lives in `ludion-protocol` (if sim owned a private format there would be two —
a Pillar 6 violation). A session = snapshot header (scene ref + RNG seed + engine/schema versions) +
frame-stamped command/input deltas, zstd-framed, with periodic keyframe snapshots (§8's format) for
seeking. Recording happens in `ludion-engine` at queue-drain time and is explicit (`--record`).
"Any editor session is replayable as a command script" is therefore literal: `ludion replay
session.lrp` re-drives the engine through the same queue.

---

## 7. Scene Format and the Canonical Writer

- **Strict subset of YAML 1.2:** no anchors, no aliases, no tags, no multi-document. Parse via
  `saphyr` behind a strictness layer that rejects the excluded constructs with L-coded diagnostics.
  (`serde_yaml` is deprecated/unmaintained — we own the emitter, which is small.)
- **Canonical emitter:** stable key order (registry/struct order, not alphabetical — read through enum variants and `Option`s; map keys in byte order), 2-space indent,
  ambiguous scalars always quoted (kills the Norway problem), one entity per block → minimal diffs.
- **Floats: ryu shortest-round-trip representation.** ("Fixed decimal precision" — the earlier spec —
  is a data-loss bug: it breaks round-tripping and makes goldens flap.) The canonical form is
  deterministic, minimal-diff, *and* lossless. A field the schema marks `f32` is written as its
  shortest *f32* decimal (`0.3`, not the widened `0.30000001192092896`) — same `f32` on read-back.
  Quantization, where wanted, happens at the source
  (a gizmo emits quantized values); the writer never destroys information.
- **Property tests:** value → emit → parse → equal; emit → parse → emit → byte-equal (idempotence).
- Validation errors cite JSON Pointer paths + L-codes + suggestions
  (*"…nearest match is 'brick_diff.png'"*).
- JSON Schema (from the component registry) validates scene documents — schema is about the data
  model, not the syntax, so it serves YAML files and JSON wire payloads alike.

---

## 8. Hot Reload (Rust Script Crates)

**V1 = recompile + restart-with-state-snapshot.** True dylib reload is rejected, not deferred:
unstable Rust ABI, `TypeId` mismatches across dylib boundaries break the ECS, TLS/statics/allocator
hazards, and `abi_stable` would infect every public type (Pillar 6 violation).

Mechanism — the game crate is a **bin** depending on `ludion`; `ludion dev` supervises:

1. Watch game `src/` (notify, debounced).
2. On change: `cargo build` — game crate only; engine crates are stable dependencies and stay cached
   (this is also the `cargo check`-speed story).
3. On success: `engine.snapshot` → World serialized via the registry (JSON) + RNG state +
   `FrameIndex` + run-state + scene refs, to a snapshot file.
4. Kill old process; start new bin on the same port; `engine.restore {snapshot}`.
5. Clients auto-reconnect and re-subscribe; the handshake reports the restored frame. A 1–2 s viewer
   blink is the accepted v1 cost — no connection-holding proxy for M1.

Components register with `Default` + serde defaults, so snapshot migration across code changes fills
added fields and reports removed ones as per-component L-coded diagnostics (with entity paths), not
crashes. Realistic loop: 1.5–3.5 s. The snapshot machinery is not throwaway — it is the replay
keyframe format and the foundation any future dylib path would need anyway.

---

## 9. Risk Register

| # | Risk | Mitigation |
|---|---|---|
| 1 | 1080p60 frame streaming chokes the Tauri webview (decode + canvas upload) | `latest_wins` drop policy — stated viewer target is 1080p@30; QOI decode in WASM / `ImageBitmap`; reduced-scale stream while screenshots stay native-res; shared-texture escape hatch already named in the pillars |
| 2 | wgpu readback stalls the sim thread (rendering runs on it) | 3-deep staging ring + `map_async`, encode on the tokio blocking pool; blocking waits only in lockstep mode where they're semantically required. **Measure at build step 5, not after** |
| 3 | Custom ECS scope creep (the classic six-month hole) | Freeze list (§4), 3000-code-line budget (§4, `scripts/ecs-budget.mjs`), conformance tests written before storage internals, every addition must be exercised by the current milestone |
| 4 | Protocol/TS schema drift | CI regenerates schema + TS types and fails on git diff; `schema_hash` handshake catches stale clients at runtime; golden test pins `engine.describe` |
| 5 | Determinism breaks across rustc versions/platforms | Toolchain pin; pure-Rust `libm`; determinism gated per-target, cross-OS measured informatively; toolchain bumps re-bless hashes |
| 6 | Headless CI has no real GPU (llvmpipe ≠ dev-box rendering) | Exactly why goldens gate on SSIM + tolerance; calibrate thresholds against llvmpipe early (step 6); bit-exact tier only on the pinned GPU runner |
| 7 | tokio/sim boundary races | Structurally eliminated: sim thread solely owns `World`; every read and write flows through one queue with oneshot replies. The invariant is sacred — no mutex shortcuts |
| 8 | Parity-gate rot (editor capabilities without commands) | Exhaustive dispatch match = compile error; CI requires ≥1 test suite to run over the wire |
| 9 | Replay log growth | Header + deltas only (Breakout ≈ KB/min), zstd framing, periodic keyframes, recording is opt-in |
| 10 | `ludion-engine` god-crate drift | "dispatch.rs contains no logic" convention + logic lives in domain crates + engine never depends on server |

---

## 10. Milestone-1 Build Order

Each step ends with something that runs — a test that passes or a CLI command that produces output.

1. **Workspace + `ludion-core` + CI.** IDs, `Diagnostic` + code table, time types,
   `DeterministicRng`, stable collections; toolchain pin, clippy denylists.
   *Verify:* `ludion explain L0001` prints; RNG golden-sequence test; a deliberate `HashMap` fails CI.
2. **`ludion-ecs`.** Entities, storage, queries, registry + per-component schema.
   *Verify:* conformance suite passes (generational safety, deterministic query order).
3. **`ludion-scene`.** Strict-YAML parse, canonical emitter, validator.
   *Verify:* round-trip property tests; emit idempotence (byte-equal);
   `ludion check-scene broken.yaml` prints an L-code + JSON pointer + suggestion.
4. **`ludion-sim` + determinism harness.** Tick loop, input scripts, `WorldHash`.
   *Verify:* `ludion sim-hash` identical across two processes; in-process double-run in CI;
   a scripted assertion ("entity.x > 3.0 by frame 30") passes.
5. **`ludion-render-graph` + headless wgpu.** Two trivial passes, transient aliasing, PNG readback.
   *Verify:* `ludion render --scene empty.yaml --out f.png` is deterministic;
   `--dump-graph` JSON snapshot test.
6. **`ludion-renderer` HD-2D pipeline.** Sprite/billboard batcher, low-poly meshes, dynamic point
   lights, bloom + tonemap, bitmap text.
   *Verify:* gallery scenes render; first goldens committed; `--dump-timings` emits JSON;
   SSIM thresholds calibrated against llvmpipe.
7. **`ludion-testkit`.** SSIM compare, structured diff reports, bless flow, sim assertions.
   *Verify:* `ludion test` passes in an example project; an intentionally broken scene yields a
   readable JSON diff report pointing at the damaged region.
8. **`ludion-protocol` + `ludion-server` + `ludion-client`.** Envelope, handshake with schema hash,
   dispatch through the sim queue, pause/step, screenshot; TS generation in CI.
   *Verify:* integration test — connect, spawn, step 10, screenshot matches in-process render
   byte-for-byte; schema-drift check green; one testkit suite runs over the wire.
9. **Frame streaming + Tauri viewer (read-only).** Binary frames channel, QOI encode, viewport +
   inspector (query commands) + protocol input injection.
   *Verify:* viewer attaches to a running example at ≥30 fps/1080p on localhost; a recorded viewer
   session replays to an identical `WorldHash`.
10. **`ludion-mcp` + `ludion new` template + engine guide + THE test.**
    *Verify:* the MVP forcing function — Claude Code, one-line prompt, builds Breakout via MCP/CLI
    to a passing golden suite; a human attaches the viewer mid-run; the session replay is committed
    as an example.
