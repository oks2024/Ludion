# Code audit — 2026-07-25

Full-repo audit of Ludion at Lore rev ~338: fragile architecture, dead code, duplication, bugs,
performance, long-term maintainability. **Report only — no fixes applied.** Every finding carries
`file:line` evidence so it can be verified before it is acted on.

Scope covered: all 20 `crates/*` (≈85k Rust LOC), `games/*` (15k), `editor/` (Tauri host 2.3k Rust +
15.7k TS/TSX), `fuzz/`, `scripts/`, `.github/workflows/ci.yml`, `ci/teamcity/`, `Cargo.toml`,
`clippy.toml` set, the shipped `docs/`.

---

## Overall assessment

This is an unusually disciplined codebase. The things that normally dominate an audit of an 100k-LOC
engine are simply absent: **zero** `TODO`/`FIXME`/`HACK`/`todo!()`/`unimplemented!()` anywhere in
`crates/` or `games/`; 14 `#[allow(...)]` in all of `src/`, each justified inline; 1,237 tests with
no stray `#[ignore]`; the stated invariants (`dispatch.rs` has no logic, `METHODS` matches the enum,
registration order is append-only, binary decoders bounds-check) are each pinned by an actual test
rather than by convention.

So the findings below are not "the usual mess". They cluster into four real themes:

1. **One missing primitive with wide blast radius** — `Diagnostic` has no severity field, and that
   single gap is visibly deforming the CLI, the wire surface, and three duplicated printers (B1).
2. **Per-frame waste that the current scene sizes hide** — `extract()` deep-clones and
   `format!()`s on every success path; the nav field reallocates its whole grid every tick; the
   renderer allocates every GPU buffer fresh each frame (C1–C7). None of it bites at 79 entities.
   All of it is proportional to the thing the roadmap is about to grow.
3. **Two gate holes** — 2,312 lines of Tauri-host Rust and 15,651 lines of editor TS are built by
   no CI job at all (E1, E2).
4. **Comment rot in exactly the places an agent reads first** — ~15 comments assert a feature is
   "not yet wired" when it shipped weeks ago (F1). In a codebase whose North Star is autonomous
   agent iterations, a false comment on a struct field is worse than no comment.

Ranked fix order is at the end.

---

## A. Correctness / robustness

### A1 — `accept_loop` hot-spins forever on a persistent accept error · Medium
`crates/ludion-server/src/connection.rs:93-96`

```rust
// A transient accept error (e.g. a client that vanished mid-handshake) must not
// kill the whole host; log-free continue is fine for M1.
Err(_) => continue,
```

The comment reasons about *transient* errors. A **persistent** one — `EMFILE`/`ENFILE` (fd
exhaustion), or a listener that has gone bad — makes `listener.accept()` return `Err` immediately
and forever. The loop then spins at 100% of a core, silently, with no log line and no way to
diagnose it from outside. `biased` select means shutdown is still observed, so it is not a hang —
just an invisible burning core.

**Direction:** distinguish the two. Log once per error class; back off (e.g. 100 ms) after a
failure; treat N consecutive failures as fatal and trip the shutdown watch.

### A2 — a request awaits the sim reply with no timeout, and ignores shutdown while waiting · Medium
`crates/ludion-server/src/connection.rs:281`

```rust
let response = match tokio::task::spawn_blocking(move || reply_rx.recv()).await {
```

`recv()` is unbounded. If the sim thread wedges — an infinite loop in a game system, or a
`sim.step` with a pathological tick count — then per in-flight request one tokio blocking-pool
thread is parked permanently, the connection's request loop never advances, and the shutdown watch
is only polled inside `read_text` (line 154), which is not where we are. The host cannot be shut
down gracefully and cannot report why.

Note this is the *likely* failure mode for this engine specifically: the agent writes the game
systems, and they run on that thread.

**Direction:** `recv_timeout` with a generous bound → `L3009`-style structured error naming the
stall; and `tokio::select!` the reply against the shutdown watch.

### A3 — a panicking game system kills the whole host, with no diagnostic · Medium
`crates/ludion-ecs/src/schedule.rs:155` (`run`), no `catch_unwind` anywhere on the sim path

Game systems are run directly. There is no unwind boundary between a system body and the sim
thread. Game code panics readily by design — the reference game alone has several
`world.get_mut::<T>(e).unwrap().unwrap()` and `world.resource::<T>().unwrap()` sites. Any of these on an unexpected world shape takes the sim thread down;
every subsequent request answers `L3009` and the process is a zombie.

Contrast with how the engine treats *every other* class of author error: a malformed scene is a
pointer-carrying diagnostic, a bad doc edit is `L1119` with the store untouched. A panicking system
is the one authoring mistake that produces no message at all — in the layer the agent writes most.

**Direction:** `catch_unwind` per system boundary → an `L4xxx` diagnostic naming the system, then
auto-pause rather than die. The agent then gets a message it can act on, which is the whole North
Star.

### A4 — `ludion docs` misfires inside a downstream project · Medium
`crates/ludion-cli/src/docs.rs:52-60`

```rust
if dir.join("CLAUDE.md").is_file() && dir.join("docs").is_dir() {
    return Some(dir);   // "checkout mode"
}
```

`ludion new` **generates a `CLAUDE.md`** into every project (`new.rs:54-55`). A user who then adds
an ordinary `docs/` directory silently flips their project into "checkout mode": `ludion docs`
lists *their* docs, `ludion docs determinism` becomes `L0009 unknown topic` with a nearest-match
drawn from their filenames, and the embedded engine docs — which are right there in the binary —
are never consulted. The two markers chosen are the two most generic names a project can have.

**Direction:** require an engine-unique marker (`crates/ludion-core/Cargo.toml`, or an explicit
`.ludion-repo` file), or fall back to embedded on a topic miss instead of erroring.

### A5 — the replay decompression ceiling's stated headroom no longer holds · Low–Medium
`crates/ludion-protocol/src/replay.rs:595-598`

```rust
/// 4 MiB is ~320x the largest keyframe a real recording produces (a committed breakout session tops …
const MAX_DECOMPRESSED_LEN: u64 = 4 * 1024 * 1024;
```

Measured today against the shipped replays: the largest uncompressed record in the reference game's `run.lrp`
is **146,513 bytes** — ~28× headroom, not ~320× — and that is for a 79-entity scene. The E6
budget scene carries **1,011** entities. A keyframe of a scene ~4× that size lands at the ceiling,
and a legitimate replay is then rejected as `L3017`. The ceiling
was calibrated against breakout (a 38 KB `.lrp`); the reference game's replay is 1.4 MB and larger
worlds are planned.

The bound itself is correct and important (it fixed a real fuzz finding). The problem is that its
justification is stale and nothing re-checks it.

**Direction:** derive the ceiling from the largest committed keyframe times a stated multiplier, and
add a test that measures the shipped `run.lrp`s against it so drift fails loudly.

### A6 — a reference-game gameplay finding, tracked privately · Low

### A7 — `ludion dev`'s fingerprint purge silently no-ops outside `target/debug` · Low
`crates/ludion-cli/src/dev.rs:1106-1118`

`fingerprint_root` hardcodes `.join("debug")` and honours only `CARGO_TARGET_DIR`. It ignores
`CARGO_BUILD_TARGET` (a cross-compile puts artifacts under `target/<triple>/debug`) and any
non-`debug` profile. In those configurations `purge_fingerprints` finds nothing, hot reload quietly
degrades to cargo's own freshness check, and no diagnostic says so. The deeper coupling — to cargo's
private `.fingerprint/<unit>-<hash>/` layout — is acknowledged in the comment and is a known future
break.

---

## B. Fragile architecture

### B1 — `Diagnostic` has no severity, and the whole error surface is paying for it · **High**
`crates/ludion-core/src/diagnostics.rs:138-153`

The struct is `{ code, message, path, help, related }`. Error-vs-warning is not represented
anywhere — it is decided by *which container* a diagnostic lands in, per call site. Observable
consequences, all present today:

- **Three copy-pasted printers** in the CLI: `check_scene.rs:293`, `import.rs:107`,
  `map_check.rs:919`. Two of the three *silently drop `diagnostic.path`* that the third prints —
  so a lint with a JSON Pointer loses it depending on which verb raised it. The pillar says
  diagnostics carry pointers.
- The code comments **admit the gap three times**, verbatim: *"the `Diagnostic` model has no
  warning severity of its own"* (`import.rs:104`, `map_check.rs:915`).
- **Wire clients cannot tell warnings from errors.** A `Diagnostic` serialized over the protocol
  carries no severity, so the editor must hardcode a code list to render an `L1028` placement lint
  differently from an `L1005` parse error.
- `ludion explain L####` cannot state whether a code is fatal.

This is the single highest-leverage structural fix in the report: one field plus one shared renderer
collapses D3, D4, and half of the CLI's diagnostic surface.

**Direction:** `severity: Severity { Error, Warning }` on `Diagnostic`, defaulting to `Error` (so no
existing construction changes meaning); record it in the `codes.rs` table; one `render(diagnostic)`
in the CLI; serialize it on the wire.

### B2 — `Engine` is a 29-field god object · Medium–High
`crates/ludion-engine/src/engine.rs:110-217` (2,136 lines)

```
driver, registry, systems, doc_kinds, profile_kind, profile_rel_path, profile_migration, seed,
scene, scene_name, project_root, asset_cache, render_size, renderer, shader_reload, stream_ring,
subscriptions, event_outbox, recorder, pending_doc_reload, shutdown_requested, shutdown_notify,
audio_sender, profile_persistence, replay_scene_text, session, boot_warnings, query_recording,
query_observations
```

It owns sim driving, scene IO, asset caching, the renderer, the stream ring, subscriptions,
recording, replay-mode state, profile persistence, shutdown wiring, query-shape recording, and boot
warnings. Every new capability adds a field **plus a reset-vs-carry decision at three world-reset
sites** (`scene.load`, `engine.restore`, scene `switch`).

The codebase already knows this: `SessionState` (`engine.rs:219-300`) was extracted for exactly this
reason, and its own doc comment cites two prior audit findings — *"the doc-reload log's
clear-vs-carry choice was exactly such a divergence (finding 2.1): `restore` cleared it while
`switch` re-applied it"*. That is the correct pattern; it has been applied to one group out of
several.

**Direction:** the same treatment for at least three more groups — render/stream (`renderer`,
`render_size`, `stream_ring`, `shader_reload`), recording/replay (`recorder`, `replay_scene_text`,
`pending_doc_reload`), and host wiring (`shutdown_*`, `audio_sender`, `profile_persistence`) —
each with an explicit reset/carry method so a reset site cannot forget a sibling.

### B3 — queries are arity-capped at 4 and require `&mut World` even to read · Medium
`crates/ludion-ecs/src/query.rs:62` (`fn build(world: &'w mut World)`), `:342-345` (`Join1`–`Join4`)

`make_col` takes `&'w mut dyn AnyStorage` for both the shared and exclusive cases, so even
`query::<(&Transform,)>()` demands an exclusive world borrow. Two consequences, both visible in the
one real game:

- **Read paths pay a clone + allocation to escape the borrow.** The reference game's AI system
  snapshots into a `Vec` because later reads need `&World`, which a live query
  iterator would hold. There are 26 such `collect()`s across its systems.
- **Arity 4 is a hard ceiling** with no `With<T>`/`Without<T>` filter escape, so a 5-component join
  becomes a 4-tuple plus manual `world.get` lookups.

**Direction:** a `RefCol`-only `query_ref(&World)` path (shared columns need only `&dyn AnyStorage`,
so this is expressible today), and either raise the tuple arity or add zero-size filter params.

### B4 — no first-class hashed singleton, so games fake one · Medium
`crates/ludion-sim/src/hash.rs:39-43` ("**Deliberately excluded:** resources")

Resources are excluded from `WorldHash` *and* from snapshots, so any global game state that must be
deterministic and replayable has to be a component on a synthetic entity. The reference game therefore
queries its singleton components dozens of times across its systems, and runs a dedicated
system whose whole job is enforcing at-most-one by convention.

Every game built on this engine will reinvent the same pattern.

**Direction:** a registry-level "at most one instance" marking plus `world.singleton::<T>()` /
`singleton_mut::<T>()`, hashed and snapshotted like any component.

### B5 — `StableMap`'s documented escape hatch would break the ECS · Medium
`crates/ludion-core/src/collections.rs:10-12` vs `crates/ludion-ecs/src/storage.rs:4-6`

collections.rs claims: *"Should profiling ever demand it, the backing store can be swapped for an
insertion-ordered map without changing callers"*.

storage.rs relies on the opposite: *"Because `StableMap` iterates in ascending key order, iterating a
storage visits entities in ascending index order for free — the determinism contract queries rely
on."* And `query.rs`'s sorted-merge join (`next_indexed`, lines 293-327) is **only correct for
sorted columns** — an insertion-ordered backend would make it silently drop rows.

So the documented escape hatch, if ever taken, breaks the ECS's core contract without a compile
error. It is a trap left for a future reader.

**Direction:** delete the claim, or state that `StableMap` is contractually key-sorted and give the
ECS a distinct alias that says so.

### B6 — audio anti-clipping is a per-call-site game convention, not an engine guarantee · Medium
`crates/ludion-audio/src/lib.rs:215-229`

The mixer sums voices and hard-clamps: `T::from_sample(mixed.clamp(-1.0, 1.0))`. There is no
limiter. The reference game compensates with a hand-maintained rule: any event that can fire many
times a tick is coalesced to one play per frame at its hook site, and gains stay ≤ ~0.8.

That is an unenforced invariant repeated at ~7 sites, in game code, guarding an engine-side
deficiency. A new sound hook that forgets it distorts, and no test catches it.

**Direction:** put the guarantee in the sink — a cheap soft limiter, or one-voice-per-sound-per-
callback de-duplication.

### B7 — `editor/app/src-tauri` is a second Rust world with different rules · Low–Medium
`editor/app/src-tauri/Cargo.toml`

- `license = "MIT OR Apache-2.0"` — contradicts the repo-wide decision recorded in root
  `Cargo.toml:12` (`LicenseRef-AllRightsReserved`, "proprietary / all-rights-reserved by user
  decision 2026-07-17"). One crate in the tree ships a permissive license header.
- `edition = "2021"`, `rust-version = "1.77.2"` vs the workspace's `2024` / `1.94`.
- A hand-rolled wire client with its own `const PROTOCOL_VERSION: u32 = 1;`
  (`editor/app/src-tauri/src/wire.rs:25`). The rationale (avoid dragging schemars + zstd into the
  editor build) is sound and documented, but nothing pins that constant to
  `ludion_protocol::PROTOCOL_VERSION` — a version bump silently diverges.
- See also **E1**: none of it is built by any gate.

The 2026-09-10 reversal (CLAUDE.md §Current state) makes `MIT OR Apache-2.0` the repo-wide
decision after all, so what this finding flagged as a contradiction is now the settled state
everywhere in the tree.

### B8 — `build.rs` writes into the committed source tree · Low–Medium
`crates/ludion-cli/build.rs:71-75`

```rust
/// Mirrors the repo `docs/*.md` into the vendored dir: copies new/changed files, deletes vendored …
std::fs::create_dir_all(vendored).expect("create embedded-docs/");
```

Cargo build scripts are expected to write only to `OUT_DIR`. This one mutates
`crates/ludion-cli/embedded-docs/` — 28 committed files. Effects: builds are non-hermetic; a plain
`cargo build` can dirty the working tree (this session's own `git status` shows **both**
`docs/engine-guide.md` and `crates/ludion-cli/embedded-docs/engine-guide.md` modified); it fails on a
read-only checkout or a vendored/verify build.

The drift test that guards the result (`crates/ludion-cli/tests/docs.rs:245-301`) is genuinely good.
The auto-write mechanism is what should go.

**Direction:** make the build script *check* and fail on drift rather than write, and move the
mirroring into an explicit verb (`ludion docs --sync`) or `check.mjs --fix`.

---

## C. Performance

None of these are causing a measured failure today (E6 reports 1.81 ms against a 16.6 ms budget).
All of them scale with the exact dimension the roadmap grows.

### C1 — `extract()` deep-clones every component of every renderable, every frame · Medium–High
`crates/ludion-engine/src/extract.rs:667-670`

```rust
fn get<T: ludion_ecs::Component + Clone>(world: &World, id: EntityId) -> Option<T> {
    world.get::<T>(id).ok().flatten().cloned()
}
```

`World::get` already returns `Result<Option<&T>>` (`world.rs:281`) and `place()` takes `&World`, so
the `.cloned()` is gratuitous. It is called **8× per entity per frame**: Transform, Mesh, Sprite,
Tilemap, PointLight, Text in `place()` (lines 548-661) plus UiRect and UiText in `extract_inner`
(192, 216) — each an `ensure_alive` + BTreeMap lookup, each hit a deep clone.

Worst cases: `Tilemap` clones its whole `Vec<String>` **and** its legend map; `Text` and `UiText`
clone their `String` twice (once inside `get`, again at lines 658 / 226).

### C2 — per-frame diagnostic-string formatting on the success path · Medium–High
`crates/ludion-engine/src/extract.rs:572, 591, 612, 625, 199`

```rust
mesh_pointer: format!("/entities/{index}/components/Mesh/shape/{shape_field}"),   // every mesh
texture_pointer: format!("/entities/{index}/components/Sprite/texture"),          // every sprite
let key = glyph.to_string();                                                      // every tile cell
texture_pointer: format!("/entities/{index}/components/Tilemap/legend/{…}/texture"), // every tile cell
```

These JSON Pointers are consumed **only on an asset-load error path**. They are built
unconditionally, every frame, for every mesh, every sprite, and **every tilemap cell**. A 100×100
tilemap allocates ~20,000 `String`s per frame that are then dropped unread.

**Direction:** carry `(index, field)` in `Placed*` and `format!` inside the error constructor.

### C3 — a reference-game gameplay finding, tracked privately · Medium

### C4 — a reference-game gameplay finding, tracked privately · Medium

### C5 — a reference-game gameplay finding, tracked privately · Medium

### C6 — `spatial::k_nearest` allocates and full-sorts per call, once per agent per frame · Medium
`crates/ludion-spatial/src/spatial.rs:314-328`

```rust
let mut candidates: Vec<(f32, EntityId)> = index.candidate_entries(…).collect();
candidates.sort_by(…);
candidates.into_iter().take(k).map(|(_, e)| e).collect()   // second Vec
```

Collect-all-then-full-sort-then-take-`k`. Called once per agent for separation
in the reference game — **500 allocations + sorts per frame** in the budget scene, plus
a second `Vec` per call.

Note the sibling `raycast` (`spatial.rs:257-261`) *does* carry an explicit "deliberately a full scan,
here is the measurement that justifies it" note. `k_nearest` has no such justification and is on a
far hotter path.

**Direction:** a fixed `k`-element max-heap (no allocation past `k`) and a caller-supplied output
buffer.

### C7 — the renderer creates every GPU buffer and most bind groups fresh each frame · Medium
`crates/ludion-renderer/src/render.rs:1707` (`init_buffer`, 19 call sites), `:867`
(`texture_bind_group` per sprite batch)

There is no persistent-uniform-buffer + `queue.write_buffer` path and no instance-buffer ring: the
camera, lights, params, sun, env, and UI uniforms plus every mesh/sprite/text/UI instance batch are
`device.create_buffer(mapped_at_creation: true)` every frame, and each sprite batch also builds a
fresh bind group. wgpu defers buffer destruction, so this is allocator churn proportional to draw
count.

**Direction:** persistent uniform buffers written per frame; a grow-only instance arena; cache the
per-texture bind group beside its texture in `texture_cache` (which already exists and already keys
on content hash).

### C8 — `build_mesh_asset_draws` is O(distinct assets × mesh instances) · Low
`crates/ludion-renderer/src/render.rs:788-823`

Re-filters all of `scene.meshes` once per asset index. Measured today: a reference-game scene has 9 distinct voxel
assets and 76 mesh entities — 684 iterations, irrelevant. It grows multiplicatively; a single
bucketing pass is the same amount of code.

### C9 — asset/texture dedup is a linear scan over a `Vec<(String, usize)>` · Low
`crates/ludion-engine/src/extract.rs:158, 246`

String comparisons per instance × distinct paths. Fine at 9 assets; replace with a map before it
isn't.

### C10 — `WorldHash::compute` round-trips every component through `serde_json::Value` · Low
`crates/ludion-sim/src/hash.rs:99-115`

Per entity × per registered component: an erased `serialize` producing an allocating `Value`, then a
recursive text-encoding walk that re-sorts every object's keys. Not on the live frame path — but
`replay_hashes` calls it **per step** (`crates/ludion-engine/src/replay.rs:92-117`), so verifying a
180 s / 10,800-frame reference-game replay pays that cost 10,800 times.

**Direction (later):** an erased "hash directly from the typed value" fn in the registry, skipping
the `Value` intermediate.

---

## D. Duplication and dead weight

### D1 — two divergent nearest-match / edit-distance implementations · Medium
`crates/ludion-core/src/suggest.rs:20-58` vs `crates/ludion-engine/src/extract.rs:895-935`

`ludion_core::suggest::nearest` is the shared helper, used by 8 call sites across the CLI, engine,
protocol, and scene crates. It implements **optimal string alignment** (Damerau) — *"because it is
the single most common typo — adjacent transposition"* costs 1.

`extract.rs` re-implements the same idea for asset-filename suggestions with **plain Levenshtein**
(transposition costs 2) and its own budget/best loop.

Behavioural consequence: a transposed-letter typo (`lantren.voxel.yaml` vs `lantern.voxel.yaml`) is
suggested by every path in the engine **except** the asset path. This also violates the repo's own
stated convention: *"Before adding a second way to do something, remove the first."*

**Direction:** delete `extract.rs:918-935`, call `suggest::nearest` over the read directory names.

### D2 — `single_char` duplicated across crates · Low
`crates/ludion-engine/src/components.rs:678` and `crates/ludion-scene/src/map.rs:2045` — different
bodies, identical semantics. Both crates depend on `ludion-core`.

### D3 — the `L0004` diagnostic is built three times with three different `help` strings · Low–Medium
`crates/ludion-ecs/src/registry.rs:420`, `crates/ludion-engine/src/commands/entity.rs:203`,
`crates/ludion-scene/src/validate.rs:153`. Only the last offers a nearest-match suggestion. Same
code, three different user experiences depending on which path raised it.

### D4 — three `print_warning` copies in the CLI · Low–Medium
`check_scene.rs:293`, `import.rs:107`, `map_check.rs:919`. `check_scene`'s prints `diagnostic.path`;
the other two drop it. Downstream of **B1**.

### D5 — the append-only registration rationale is restated 6 times · Low
`crates/ludion-engine/src/components.rs:1058-1099` — the same ~4-line "appended at the very end so no
existing component's registration index shifts, keeping every prior scene/replay world hash
byte-identical" paragraph, six times. State the invariant once above `builtin_registry`.

### D6 — 8 finished-wave artifacts committed at the repository root · Low
Several reference-game wave artifacts, plus `phase-11-wave4-lights.diff.json`,
`phase-11-wave4-meshes.diff.json`, `shadow-mapping-wave3-ui-hud-1080.diff.json`,
`phase-11-wave4-bless.md` — all tracked, all one-shot outputs of closed waves, all at the root
alongside `CLAUDE.md` and the four proof docs. They belong under `Plan/` or an `artifacts/` dir, or
should be dropped.

### D7 — a reference-game gameplay finding, tracked privately · Low

**No dead code found.** `#[allow(dead_code)]` appears 5 times, each a documented schema-only or
handle-retention case. `ludion-mcp`, `ludion-window`, and `ludion-import` are each reachable from a
real consumer.

---

## E. Gate and tooling gaps

### E1 — 2,312 lines of Tauri-host Rust are built by **no** gate · **High**
`editor/app/src-tauri/src/` — `stage.rs` 1,058, `pty.rs` 784, `files.rs` 237, `wire.rs` 176

- Root `Cargo.toml:3`: `members = ["crates/*", "games/*"]` — `editor/` is deliberately excluded, and
  `src-tauri/Cargo.toml` carries an empty `[workspace]` to detach itself.
- `.github/workflows/ci.yml`: its four jobs (`check`, `packaging`, `denylist-canary`,
  `schema-drift`) never enter that directory. `cargo clippy --workspace` / `cargo test --workspace`
  cannot see it.
- `ci/teamcity/`: no script for it.
- `scripts/check.mjs`: no step for it.

So its `cargo fmt`, its clippy lints, and its 5 tests run only when a human remembers. This is the
PTY host, the asset watcher, and the wire client that the editor's terminal drawer depends on.

**Direction:** add `cargo fmt --check` / `clippy -D warnings` / `test` with
`--manifest-path editor/app/src-tauri/Cargo.toml` to both `check.mjs` and `ci.yml`.

### E2 — the editor frontend's gates are in `check.mjs` but not in CI · Medium
`scripts/check.mjs:130-132` runs `oxlint`, `tsc -b`, and `vitest` in `editor/app` — covering 15,651
lines of TS/TSX and 50 test files. `.github/workflows/ci.yml` runs **none** of them; its only
frontend job regenerates `editor/protocol/src/protocol.d.ts` for schema drift.

Root `CLAUDE.md` states the rule as *"Canonical gate definitions still live in
`.github/workflows/ci.yml` + `ci/teamcity/`; keep the script in step if a gate is added there."*
Here the drift runs the other way: the local script is stronger than the canonical definition, so
the canonical definition is quietly the weaker one.

### E3 — four crates have no `clippy.toml`, with no recorded decision · Low
`ludion-audio`, `ludion-import`, `ludion-shader-bindgen`, `ludion-window`. All four are plausibly
off the sim path, so "no denylist" is probably right — but `ludion-client` already sets the precedent
of committing an *explicitly empty* file with a comment explaining why. Absent file vs. deliberate
empty file is exactly the distinction that matters when someone adds sim-adjacent code later.

### E4 — the fuzz job exists only in TeamCity scripts · Low
`ci/teamcity/fuzz.sh` / `fuzz.ps1` are referenced by neither `ci.yml` nor `check.mjs`, so on the
GitHub side the five targets never run. The machinery itself is in good shape — the one finding to
date (`fuzz/findings/replay_reader/zstd-decompression-bomb.lrp`) is fixed, documented, and covered by
a committed regression test on the stable toolchain.

---

## F. Documentation drift

### F1 — ~15 comments claim a feature is "not yet wired" when it shipped · Medium–High

These sit on the exact struct fields and registration lines an agent reads immediately before
editing them. Each was verified against the current code:

| Comment | Claim | Reality |
|---|---|---|
| `ludion-renderer/src/render.rs:71-74` | fade lane is *"**Dead as of Wave 2** … the mesh shader declares `@location(14)` but does not read it … lands in Wave 3"* | `shaders/mesh.wgsl:119, 243-256` reads it and performs the Bayer discard |
| `ludion-renderer/src/render.rs:1774` | *"**Dead as of Wave** …"* (same lane) | same |
| `ludion-engine/src/components.rs:218` | `DirectionalLight` — *"no shader reads it until Wave 3"* | shadow mapping shipped 2026-07-17; `shadow.wgsl` + the sun term exist |
| `ludion-engine/src/components.rs:421-422` | `Fade` — *"no shader reads it yet … the dither discard lands in Wave 3"* | shipped |
| `ludion-engine/src/components.rs:835, 900, 1076` | `UiRect`/`UiText` — *"render-inert until the overlay pass lands (a later phase)"* | Phase 14 UI shipped; `ui_rect.wgsl` / `ui_text.wgsl` and both pipelines exist; the reference game's HUD is built on it |
| `ludion-engine/src/components.rs:1087, 1097` | same, at the registration sites | same |
| `ludion-engine/src/extract.rs:325` | `SunData` — *"no shader reads it yet (shadow-mapping Wave 2)"* | shipped |
| `ludion-engine/src/extract.rs:338` | `Environment` — *"a `Some` is currently render-inert"* | Environment/IBL shipped 2026-07-22; `sky.wgsl`, `ibl.wgsl` |
| `ludion-renderer/src/scene.rs:75, 107, 131, 288` | `sun` / `environment` — *"render-inert"*, *"Wave 2 data-only: no shader reads it yet"* | shipped |
| `crates/ludion-cli/embedded-docs/crate-graph.md:103` | *"`ludion-mcp` is coming online"* | ships, with a parity test |
| `crates/ludion-cli/src/check_scene.rs:349` | built-in registry is *"(Transform, Sprite, Mesh, PointLight, Camera, Text, Parent)"* | 15 components (adds Name, Tilemap, Collider, UiRect, UiText, DirectionalLight, Environment, Fade) |

A grep for the pattern family (`lands in Wave`, `until Wave`, `this wave`, `render-inert`,
`no shader reads`, `Dead as of`, `not yet`) returns **66 hits** across `crates/*/src` + `games/*/src`;
the table above is the subset I confirmed false.

**Root cause:** comments are written in the narrative tense of the wave that authored them
("this wave", "lands in Wave 3"), which is guaranteed to become false the moment the next wave lands.
The claim is not merely stale — it is actively misleading, because it tells a reader that changing
the field is render-neutral when it is not.

**Direction:** (a) a close-ritual step that greps this pattern set before a phase closes; (b) a house
rule that a code comment describes **state**, never **schedule** — wave/plan sequencing belongs in
`Plan/`.

### F2 — comment-to-code ratio is 0.36, and much of it is provenance prose · Low
15,638 comment lines to 43,030 code lines across `crates/*/src`. Where it explains *why* — the
normal-matrix derivation, the byte-level hash rules, the borrow-splitting in `query.rs` — it is
genuinely excellent and should not be touched. But a large fraction is provenance: wave numbers,
plan filenames, prior audit finding IDs, "decision 4", "the `import.rs` rendering precedent". That
category is what rots (F1), and it duplicates what `Plan/` and Lore history already record.

---

## Checked and healthy

Recorded so a later pass does not re-derive it:

- **No `TODO`/`FIXME`/`HACK`/`XXX`/`todo!()`/`unimplemented!()`** anywhere in `crates/` or `games/`.
- **14 `#[allow(...)]` in all of `src/`**, each with an inline justification.
- **`dispatch.rs` is exactly the stated contract** — 26 one-line arms, no wildcard, no logic (82
  lines total).
- **Invariants are test-pinned, not convention-pinned:** `Command::METHODS` ↔ enum variants
  (`command.rs:494-508`); MCP tool set ↔ `METHODS` (`tools.rs:157`, `integration.rs:183`); built-in
  registration order (`components.rs:1216`); embedded docs ↔ `docs/` (`tests/docs.rs:245-301`);
  generated map scenes ↔ sources (`scripts/map-drift.mjs`); shader bindgen output
  (`ludion-shader-bindgen -- --check`); schema ↔ TS types.
- **Binary decoders bounds-check before every slice** — `frame.rs:128-153`, `replay.rs:416-445`,
  `replay.rs:503-536` all use `checked_add(..).filter(|end| *end <= bytes.len())` before indexing.
  The `.unwrap()`s on `try_into()` are provably safe behind those checks.
- **Determinism denylists are real** — 16 per-crate `clippy.toml` files, plus a CI canary job that
  appends a violation and asserts clippy rejects it.
- **1,237 tests**, no `#[ignore]` outside the deliberately-ignored probe harness
  (the reference game's probe harness, each with a reason string).
- **`ludion-spatial` ordering and tie-breaks** are explicit and documented at every comparison
  (`total_cmp` then `EntityId`), with the full-scan choices justified by stated measurements.
- **The fuzz finding was handled correctly** — reproducer committed, fix bounded, regression test on
  the stable toolchain, a written correction of the plan's wrong suspicion.
- **435 transitive dependencies** with the usual duplicate majors (`rand`, `thiserror`, `bitflags`,
  `hashbrown`, `windows-*`) — all from the wgpu/tokio/tauri trees, nothing gratuitous in the
  first-party manifests.

---

## Suggested order of work

Cheap-and-high-value first; each line is independent of the others.

**Tier 1 — small diffs, disproportionate payoff**
1. **E1** — wire `editor/app/src-tauri` into `check.mjs` + `ci.yml`. Three lines; unblocks every
   future finding in 2,312 unguarded lines.
2. **F1** — correct the ~15 false "not yet wired" comments. Pure text; removes active
   misinformation from the files an agent opens first.
3. **D1** — delete the duplicate edit-distance in `extract.rs`, call `suggest::nearest`. Fixes a real
   UX inconsistency and removes a "second way".
4. **C2** — make the extract JSON Pointers lazy. Localised; removes ~20k allocations/frame in the
   tilemap case.
5. **C1** — `place()` borrows instead of cloning. Mechanical (`get` → `&T`), touches one file.

**Tier 2 — the structural ones**
6. **B1** — add `Severity` to `Diagnostic`; one shared renderer. Collapses D3 + D4 and unblocks the
   editor's warning UX.
7. **E2** — bring `ci.yml` up to `check.mjs`'s frontend coverage (or record why it deliberately
   differs).
8. **C3 + C5 + C4** — the nav-field trio: mutate in place, census once, bound the gradient sweep.
   One file, and it makes the "no O(map-area) term" claim in `CLAUDE.md` true.
9. **A1 + A2** — server accept backoff and a bounded reply wait. Both small; both remove a silent
   failure mode.
10. **A3** — `catch_unwind` per system → a diagnostic + auto-pause. The highest-value *ergonomic*
    fix for an agent-written game layer.

**Tier 3 — worth deciding, not necessarily now**
11. **B2** — continue the `SessionState` extraction pattern across `Engine`'s remaining field groups.
12. **B3 / B4** — a shared-borrow query path and a first-class hashed singleton. Both change the game
    authoring API, so they want a plan, not a patch.
13. **C6 / C7** — `k_nearest` heap-select; persistent renderer buffers. Do these when a measurement
    demands them, not before — but note them in the E6 gate so the measurement is watched.
14. **A4, A5, A6, A7, B5–B8, C8–C10, D2, D5–D7, E3, E4** — small, independent, batchable.

---

## Status — fix stage complete (2026-07-25, Lore revs 344–354)

Scope decided by the user: **Tiers 1 and 2 in full, plus Tier 3 item 14** (the small batchables).
**24 of the 38 findings are fixed.** Every gate green, `check.mjs --full` included.

| Rev | Findings |
|---|---|
| 344 | D6 |
| 345 | C1, C2, C9, D1 |
| 346 | A1, A2 |
| 347 | E1, E2, E3, E4 |
| 348 | C3, C4, C5, A6 |
| 349 | F1, D5 |
| 350 | **B1**, D3, D4 |
| 351 | D7 |
| 352 | A3 |
| 353 | A4, A5, A7, B5, B8, D2 |
| 354 | B6, B7, C8, C10 (skipped — see below) |

### Deferred, as this report itself recommends
- **B2** — decompose the 29-field `Engine` along the `SessionState` precedent.
- **B3 / B4** — a shared-borrow query path and a first-class hashed singleton. Both change the
  game-authoring API and want a plan of their own, not a patch.
- **C6 / C7** — `k_nearest` heap-select and persistent renderer buffers. Do them when a measurement
  demands them; note them in the E6 gate so the measurement is watched.
- **C10's erased typed-hash fn** — attempted, measured, **deliberately not shipped**. Hashing is 60%
  of reference-game replay verification, but 8 of those 34 s are the fixed registry×entity lookup the change
  does not remove, so its ceiling is one test going 57 s → ~37 s inside a ~16 min suite — against
  ~400 lines of hand-written serde `Serializer` permanently on the determinism spine, whose failure
  mode is a repo-wide breakage. A bit-identical allocation removal inside the existing encoder
  shipped instead (21.2% faster per `compute`, proven against a verbatim pre-change oracle).

### Corrections to the report itself, found while fixing
- **A7 was understated.** The fingerprint purge is not merely broken for cross-compiles — cargo's
  `target/` sits at the *workspace* root, so `ludion dev` in a game crate looked in
  the crate's own `target/`. It has never worked for any engine game.
- **B5's claim had an origin the report did not name**: `engine-architecture.md` §5 rule 1 said
  "swap to indexmap only if profiling demands", and indexmap is exactly the insertion-ordered store
  that would silently break the ECS's sorted-merge join. Fixed at the source too.
- **F1 was larger than the ~15 tabulated.** About 30 comments were verified false and corrected;
  the hits that were still accurate are listed in rev 349's message as evidence of verification.
- **D4 was vacuous for one verb**: `map-check`'s `L1028`/`L1029` carried no JSON Pointer at all, so
  "every verb renders its pointer" could not have been proven for it. They now carry `/markers/<n>`.

### Durable prevention added, not just the fixes
- **F1's root cause** — comments written in the authoring wave's tense — is now a house rule in root
  `CLAUDE.md` ("a comment describes state, never schedule") *and* a step in the milestone close
  ritual (a private roadmap's rule 4) with the literal grep.
- **A5's stale justification** cannot recur silently: the ceiling is derived from a measurement that
  a test re-takes against every committed `run.lrp` on each run.
- **E1/E2** close the gate holes, so future findings in those 18k lines fail CI instead of waiting
  for an audit.

### Observed during the arc, unresolved
- **A GPU test flake — now seen TWICE.** **Sighting 1 (2026-07-25):** one `--full` run died with
  `STATUS_ACCESS_VIOLATION` in `ludion-renderer --lib` after 18/20 tests; the two that never reported
  were `sun_changes_pixels_and_is_deterministic` and
  `environment_ibl_changes_pixels_and_prefilter_is_deterministic`. It passed 20/20 immediately on
  re-run. **Sighting 2 (2026-07-26, during post-process wave 9):** both `ludion-server::phase10`
  tests died with `0xc0000005` at tests 1104/1106 of a 1419-test `check.mjs` lane, with a
  `ludion-cli::phase7` cargo build and two reference-game replays running alongside. Different binaries,
  different tests, one signature: a process holding a wgpu device dies with an access violation under
  a saturated box, with no Rust panic and no wgpu validation message; captured stdout is `running
  1 test` then `(test aborted)` with no libtest result line, so the fault is inside the test body
  rather than at process-exit teardown.

  Two traps this cost us, recorded so the third sighting does not repeat them. **(1) It looks like
  whatever landed most recently.** Sighting 2 was first attributed to wave 9 on a stash/restore A/B —
  4 clean runs on the previous commit against 2 crashes on the new tree, on one command, back to
  back. That is far too small a sample to separate a regression from a schedule-sensitive flake, and
  ~170 later executions on the accused tree — including the `gpu` group raised from 4 to 8 and to 32,
  and under a saturating concurrent `cargo build --workspace --all-targets` — never reproduced it.
  Any wave that adds a shader or a pipeline shifts which tests co-schedule, which moves the flake's
  base rate without being its cause. **(2) The tempting fix disarms the harness.** Lowering
  `gpu = { max-threads = 4 }` in `.config/nextest.toml`, or serializing the binary, would retire the
  symptom and the tripwire together.

  **What the next sighting actually needs is a dump.** Set
  `HKLM\SOFTWARE\Microsoft\Windows\Windows Error Reporting\LocalDumps` with `DumpType=2` (requires
  admin; it is not set today, which is why neither sighting was captured — there are also no WER
  records and no TDR/`nvlddmkm` events for them). One captured occurrence names the faulting module
  and settles whether this is the DX12 driver, wgpu, or engine code, which no amount of re-running
  will.
- **`target/debug/incremental` corrupted once** under concurrent agent builds (rustc ICE, "Failed to
  recover key for `def_kind`"), failing the one test that shells out to build the binary. Deleting
  the directory fixed it.
- **`games/*/run.lrp` were re-recorded** at rev 350, as `docs/protocol-evolution.md` step 5 requires
  for an additive rev. Trajectories are unchanged — severity is not a component and cannot enter a
  `WorldHash`; only the header's `schema_hash` stamp moved. A pending human gate's artifact bytes
  did move, so that sitting should be aware of it.
