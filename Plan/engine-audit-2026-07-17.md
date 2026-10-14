# Engine-wide audit — 2026-07-17

**What this is.** A read-only, eight-scope audit of the whole workspace, run as parallel subagent
audits with strict evidence rules (file:line citations, traced code paths), then deduped and
verified by the orchestrator. The first run was interrupted after three scopes reported; their
reports were salvaged from the session transcripts and the remaining five scopes were re-run with
the identical briefs. Every finding marked **[verified]** was independently re-traced against the
code by the orchestrator before inclusion; unverified findings are subagent claims that were
consistent and evidence-backed but not independently re-checked.

**Scopes:** core determinism (ludion-core/ecs/sim/spatial), text/data pipeline
(ludion-scene/assets/import), engine runtime (ludion-engine), wire protocol
(ludion-protocol/server/client/mcp), renderer (ludion-render-graph/renderer/extract), host/tooling
(ludion/ludion-cli/testkit/window/audio), games/examples/editor, cross-cutting hygiene
(scripts/CI/ignores/docs/budgets/goldens).

**Headline.** No memory-safety, determinism-of-record, or golden-threatening defects anywhere.
The codebase is in unusually good shape — the determinism discipline is mechanically enforced and
real, the egress latest-wins fix is sound, boundary rules hold, and the test suites define
contracts. The real findings cluster into: (1) a small set of genuine bugs, all at
record/replay/restore seams or in secondary paths; (2) latent seams that the recorded roadmap
(replay seek, shadow mapping, third-party clients) will force; (3) an accretion tax — state and
logic managed by convention across N sites instead of by one owner; (4) duplication debt at
predictable boundaries (CLI↔facade, shader↔shader, game↔game).

---

## 1. Verified bugs (worst-first)

### 1.1 [bug][verified] Stale `EntityId`s silently alias live entities after a restore
`crates/ludion-ecs/src/entity.rs:41-63` (`EntityAllocator::from_live`), `world.rs:155-159`.
`from_live` rebuilds every non-live slot at `generation: 0` (the doc comment states it), so a slot
recycled before the snapshot comes back re-issuable at gen 0; the next spawn mints an `EntityId`
bit-identical to a pre-restore stale handle. The headline invariant "a stale `EntityId` is a
reported error, never a silent hit" does not survive snapshot→restore — and restore is the
`ludion dev` hot-reload path where the editor retains ids (selection, undo/redo).
*Direction:* snapshot per-slot generations (and the free list) alongside the live set, or bump all
restored gap generations past any value the run could have produced.

### 1.2 [bug][verified] Live `--record` produces an unreplayable `.lrp`
Chain: `crates/ludion/src/lib.rs:232` arms recording on the free-running host →
`crates/ludion-engine/src/live.rs:217` ticks via `advance_live` (`engine.rs:374-385`), which drains
audio/profile/switch but never touches the recorder — only command-driven `Engine::execute`
records → replay's `drive_actions` (`replay.rs:95-103`) advances frames only on `SimStep` records →
`SimDriver::inject_input` clamps to `max(frame, current)` (`ludion-sim/src/driver.rs:220`), so a
live recording's inputs are scheduled at frames the replay never reaches. A Running host recorded
with `--record` yields a ~1-frame trace. Every committed `.lrp` works only because it was made via
lockstep `drive_hashes` with explicit `SimStep` per tick; nothing tests the live path.
*Direction:* emit a periodic tick/keyframe record from `advance_live` when recording, or make
linear replay advance frames to each record's stamp.

### 1.3 [bug][verified] Event subscriptions have no lossless path and drop under bursts
`crates/ludion-server/src/lib.rs:158` (the egress channel is `bounded(1)`, shared by frames and
events), `crates/ludion-engine/src/live.rs:338` (`let _ = egress.try_send(Egress::Event(…))` —
dropped whenever the slot is occupied), `commands/stream.rs:27-36` (`SubscribeMode::Acked`, the one
mode documented "for agents and recording", is rejected with L3012). Seqs are assigned before the
send, so drops are *visible* gaps — but an agent subscribing to `sim.events`/`log` to verify
gameplay has no lossless option at all, against the North Star. From the pump onward delivery is
genuinely ordered and lossless; the drop is at the engine→pump seam.
*Direction:* implement `acked`, or give events their own unbounded ordered egress channel separate
from the latest-wins frame slot.

### 1.4 [bug][verified] `ludion dev` never recovers a child that crashed on its own
`crates/ludion-cli/src/dev.rs:461-469`. On reload, a failed `snapshot_and_shutdown` (connection
refused when the child already died) prints the diagnostic and returns `Continued`, keeping the
dead `GameChild` as current. Nothing on that path probes `try_wait`; every subsequent edit re-fails
the same way, and in `--window` mode the viewer burns its 30 s reconnect budget and goes fatal. The
module's "a failed snapshot keeps it serving" reasoning assumes the child is alive.
*Direction:* on snapshot failure, probe child liveness and relaunch from the freshly built exe
(state lost but a live server restored) instead of retaining the corpse.

### 1.5 [bug][verified] Mesh normals are wrong under non-uniform scale
`crates/ludion-renderer/src/shaders/mesh.wgsl:70` transforms normals by the model's upper-3×3
(comment admits "Uniform/near-uniform scale assumed") while `crates/ludion-engine/src/extract.rs:400`
bakes the full 3-component scale into the model. A `Mesh` with `scale: [3,1,1]` silently mis-lights;
no validation warns. Latent trap in the shipped PBR feature.
*Direction:* upload a normal matrix (inverse-transpose), or validate/clamp to uniform scale with an
`L####`.

### 1.6 [bug][verified] `check_budget` silently disarms on a misnamed budget key
`crates/ludion-engine/src/profile.rs:415-427`; the doc comment (403-405) records the choice
deliberately ("a budget can only be *exceeded* by something that was measured"). A budget key whose
system/pass was renamed, deleted, or mistyped is indistinguishable from "legitimately not run" —
no warning, exit 0, tripwire permanently disarmed. The G2 `auto_fire`→`fire` rename would have
quietly disarmed the fire tripwire and was caught only by manual review (the private reference-game notebook).
*Direction:* cross-check budget keys against the names the engine's registered systems/passes can
produce, or warn on a budget-only name never seen in any report.

### 1.7 [bug][verified] glTF importer spuriously rejects proportional emissive
`crates/ludion-import/src/lib.rs:464-480`. `proportional_emissive` recovers `k = e0/c0` then
requires `c * k == e` bit-exactly per channel; `x*(y/x) == y` does not generally hold in f32, so a
legitimately proportional `emissiveFactor = 0.3 × baseColorFactor` can fail (even the recovered
channel itself: `c0*(e0/c0)` needn't round back to `e0`). The unit test passes only because
`k = 0.5` is an exponent shift. Never a silent drop (errors L1202; `--allow-lossy` recovers), so
severity is low — but it wrongly rejects expressible input.
*Direction:* accept `k` within a small relative epsilon / ulp bound.

### 1.8 [bug][verified] macOS gallery goldens missing for `ui-hud` — already tracked
`examples/gallery/goldens/macos/` lacks `ui-hud.png`/`ui-hud-1080.png` (present on Windows; scene
auto-discovered, missing golden is a hard `Err` not a skip). Will fail two gallery tests the moment
the mac TeamCity agent comes up. **Known:** the 2026-07-16 roadmap commit tracks "macOS ui-hud
bless queued" — listed here so the mac bring-up checklist doesn't lose it.
*Direction:* bless on the pinned M4 and commit (human-gated: pinned hardware).

## 2. Latent seams the roadmap will force (fragile, verified where marked)

- **[verified] Snapshots omit `session_doc_reloads`** — `snapshot.rs:35-68` carries no reload log;
  `install_restored` clears it and rebuilds `DocStore`s from disk (`engine.rs:1540-1543`) while
  `install_switched` deliberately re-applies it off-disk (`engine.rs:1591-1598`). The clear-on-restore
  is a *recorded* decision (comment at `engine.rs:1538`: derived components already hold post-reload
  values) — but a replay-seek whose keyframe lands after a live reload, or a switch performed after a
  restore, rebuilds from possibly-changed disk: the exact cross-machine divergence the switch path
  was built to prevent. Latent until a seek consumer ships (the stated goal).
  *Direction:* add the reload log (and generation) to `SnapshotDoc`; re-apply on restore as
  `install_switched` does.
- **`Client::call` has no timeout** — `ludion-client/src/lib.rs:233-251`. The sim is single-threaded
  and dispatch synchronous, so one infinite-looping game system hangs every connected client
  (editor, MCP adapter) forever with no diagnostic; even wire `engine.shutdown` can't be processed.
  *Direction:* optional call timeout surfacing L3009, and/or a sim-side per-tick watchdog.
- **Strict `schema_hash` connect gate defeats the `#[non_exhaustive]` additive-evolution design** —
  `ludion-client/src/lib.rs:171-185` vs the "room to grow" rationale on `ResponseBody`/`Event`. Any
  additive schema change changes the hash and hard-fails older clients at connect (L3003); additive
  revs have only worked because all clients are in-repo and rebuilt together.
  *Direction:* either document all schema changes as breaking, or compare a coarser compatibility
  key (protocol version + capability set).
- **Depth target lives outside the frame graph** — `ludion-renderer/src/render.rs:948` allocates a
  private per-frame depth texture; the graph's `TextureFormat` has only two color variants, so the
  declarative graph cannot express a depth resource. Shadow mapping (recorded roadmap) needs depth
  as a first-class graph resource for dependency ordering.
- **Five independent format-version checks, no migration seam** — `ludion-scene`'s
  `parse.rs:121`, `mesh.rs:232`, `voxel.rs:258`, `doc.rs:149`, `profile.rs:122`: each hard-rejects
  `version != 1` against its own constant; mesh/voxel don't even have a distinct diagnostic for
  version drift (collapsed into L1110/L1113). The first real format bump edits five copies with no
  shared upgrade hook. *Direction:* one `check_envelope(format, kind)` helper + a distinct L-code;
  decide the migration story before format 2.
- **Live-injected inputs accumulate without bound** — `ludion-sim/src/driver.rs:219-223` pushes every
  `input.inject` into the script and `do_tick` only advances the cursor, never compacting. A
  long-lived live session (one `MouseMove`/frame) grows memory linearly forever.
  *Direction:* drain applied events below the cursor at a frame boundary.
- **`MAX_LIGHTS` triple-defined, unvalidated** — `render.rs:28` (Rust, 16) vs hardcoded
  `array<Light, 16>` in `mesh.wgsl`/`sprite.wgsl`, `min_binding_size: None` on the BGL. Bumping the
  Rust constant silently mis-lights instead of failing to compile.
- **Frame backpressure shares one FIFO with replies and events** — `ludion-server/delivery.rs:64,260`,
  `connection.rs:160`: under a slow TCP client, up to 32 queued binary frames sit ahead of command
  replies, and events on that connection are preferentially dropped. Not a correctness break;
  a priority/lane problem that grows with frame size.
- **Query construction is O(all storages)** — `ludion-ecs/src/world.rs:373-381` sweeps every storage
  per query build. Negligible today; scales with the registry, not the query, and "many more
  component types" is an explicit growth axis.

## 3. Accretion & architecture

- **The reinstall/reset checklist is tribal across three lifecycle sites** [verified] —
  `load_scene_document` (`engine.rs:911-949`), `install_restored` (`engine.rs:1518-1554`),
  `install_switched` (`engine.rs:1570-1605`) each hand-repeat ~5 installs + ~5 clear/preserve log
  decisions. They agree today; nothing enforces the set, and finding 2.1 is precisely one such
  divergence. *Direction:* a `SessionState` struct with `reset()`/`carry()` so the sites differ only
  by which they call.
- **Host lifecycle forked between `ludion serve` and `ludion::run`** — `ludion-cli/src/serve.rs:50-202`
  vs `ludion/src/lib.rs:168-330`: audio wiring, recording-arm, `--paused`, `Server::bind`, watch,
  the whole `--window` branch, and three byte-identical teardown helpers (`wait_for_stop`,
  `install_ctrl_c_handler`, `forward_wire_shutdown_to`). A shutdown-ordering fix in one host
  silently misses the other. *Direction:* extract a shared `host_scene(engine, addr, opts)`.
- **Error surfacing is N ad-hoc field+accessor pairs on `Engine`** — six independent
  field/getter/manual-reset triples (`profile_migration`, `profile_save_error`,
  `last_transition_error`, `audio_log`, `profile_save_log`, `shutdown_requested`), each new feature
  minting another on the previous one's "mold". *Direction:* one `SessionReport` drained per frame.
- **Two validation philosophies coexist in the text pipeline** — scene/mesh/voxel hand-roll bespoke
  envelope parsers (`parse_format`/`expect_string`/`unknown_key` byte-identical but for L-codes)
  while data-docs use the generic schema-driven `doc.rs` path. *Direction:* fold the envelope
  helpers into one module; long-term migrate mesh/voxel onto the schema path.
- **Facade examples are compile-gated, not behaviorally exercised** — each `crates/ludion/tests/*`
  "mirrors" its example by re-implementing it rather than importing; example runtime logic can
  drift from its asserted mirror with nothing failing, qualifying the "examples are the primary
  documentation, runnable" guarantee. *Direction:* tests `use` the example's `game()`/systems, or
  spawn the example binary.
- **`oneOf` validated as `anyOf`** — `ludion-scene/src/validate.rs:279-314`, first-match-wins.
  Harmless for schemars-generated disjoint variants; silently under-checks if an overlapping
  hand-written schema is ever registered. Leave, but note.

## 4. Duplication debt (will drift)

| What | Copies | Direction |
|---|---|---|
| Fullscreen-triangle VS (incl. UV V-flip) | 5+ shaders (`tonemap`/`blur`/`threshold`/`blit`/`gradient`/`vignette`) | shared WGSL snippet at pipeline build |
| `Camera`/`Light` structs + attenuation | `mesh.wgsl` + `sprite.wgsl` (must stay bit-identical to `LightsRaw`) | shared snippet |
| Profile report/budget-check CLI wrapper | `ludion-cli/src/profile.rs:19-84` ≈ `ludion/src/lib.rs:337-412` (core correctly single-sourced in engine) | move wrapper into `ludion-engine` |
| `diff_fields` | `snapshot.rs:275-292` ≈ `player_profile.rs:491-508` (self-labeled copy) | hoist one shared fn |
| `found_phrase` value→noun mapping | `validate.rs:512`, `doc.rs:248`, `profile.rs:189`, `yaml.rs:55` | one fn in `ludion-core`/yaml |
| `scalar_string` (Norway-problem primitive) | `emit.rs:260`, `mesh.rs:755`, `voxel.rs:1045` | make `scalar::scalar_string` pub(crate), delete copies |
| Golden-test boilerplate | ~90 near-identical lines × 4 games | `GoldenTest::single(scene, golden, registry)` in testkit |
| Gameplay/drift driver scaffolding | 4 games' `tests/gameplay.rs`/`drift.rs` | shared testkit driver helper |
| Engine-owned kind list | `ludion/src/lib.rs:463` mirrors engine-private `ENGINE_KINDS` (reporting-only blast radius, self-documented) | expose the engine's list as pub |

## 5. Dead code & stale docs

- **`CLAUDE.md` "Current state" is six phases stale** — still says "Currently: Phase 11" while
  Phase 17 closed 2026-07-17 and work is in vigil's G2. The file that overrides all agent behavior
  misstates the project by a week of shipped work. Fold "update Current state" into the phase-close
  routine.
- **`check.mjs` runs two smoke gates CI doesn't** — `docsStep`/`checkDocsStep` (added Phase 15 W4)
  exist only in `scripts/check.mjs:118-128`, not in `.github/workflows/ci.yml` or the TeamCity
  README, despite the trio's exact-parity claim. Mirror them or document local-only.
- **Three shipped crates have no `ludion docs` topic** — `ludion-audio`, `ludion-import`,
  `ludion-window` (verified against the live listing). Undercuts "crate docs bind that crate" for
  exactly the crates with real footguns. The `ludion-docs` skill's job.
- **`.loreignore` misses `editor/app/.gitignore`'s six log patterns** (`logs`, `*.log`,
  `npm-debug.log*`, …) — Lore would track what git ignores. Add them.
- **Stale dependency-rationale comments** — root `Cargo.toml` claims `qoi` is server-only (also
  client dev-dep + window runtime) and `toml` is CLI-only (engine's `load_budget` parses it).
- **Stale CLI docs in two proof games** [verified] — `games/{breakout,hello-paddle}/src/main.rs:4-5`
  omit `--window` from `ludion::run`'s documented surface; sokoban's is current. These are
  documentation-by-example; sync the wording.
- **`Request::check_version` is unused pub API** — `ludion-protocol/src/envelope.rs:129-135`;
  `from_value` re-implements the check inline. Delete or route through it.
- **`Subscription.mode` stored, never read** — `engine.rs:1403`, orphan of the unshipped acked
  delivery mode (see 1.3 — implementing acked resolves both).
- **Orphaned asset** — `examples/gallery/voxels/house.voxel.yaml`: fully authored, referenced by no
  scene or test. Wire in or remove.
- **Stale scene-crate doc contract** — `ludion-scene/src/lib.rs:224-226` claims entity names "never
  affect a `WorldHash`"; false since Phase 6's engine-side `Name` projection. Reword.
- **`DiagnosticCode::domain_digit` docs omit the audio (`5`) range**, and the domain enumeration is
  triplicated across `diagnostics.rs`/`codes.rs`/the L0000 explanation. Derive from one table or
  add a drift test.

## 6. Verified clean (checked, no action)

Frame-graph schedule/alias core (half-open lifetimes, safe bloom aliasing, byte-stable UI
retarget); Cook-Torrance NaN/÷0 guards; sRGB handling; readback row-padding; egress pump
latest-wins fix (watch slot, no lock across await, pinned by regression test); connection
lifecycle/shutdown flush; `.lrp` framing (bomb ceiling, additive record kinds, old-file tests);
dispatch parity gate (26 one-line arms, no wildcard, schema-locked `METHODS`); MCP adapter
(schema-derived tool catalog, no per-command match); determinism math across core crates (libm
routing, no glam reducing ops on hash paths, PCG reference vector); spatial pinned iteration order
(oracle-tested); ECS query join; canonical writer + strict-YAML (ryu throughout, idempotence
proptests); `ludion-assets` GPU-free, watcher drop/debounce sound; importer's no-silent-drops rule
holds end to end; `ludion-window` thread seams/teardown/reconnect; `ludion-audio` device-absence
handling; testkit SSIM tiling + per-OS `platform_dir` + bless flow; vigil game logic incl. the
AimMode tri-genre switch; editor undo/redo, reconnect, picking, schema-derived inspector; camera
math triple (extract.rs ↔ camera.ts) checked line-by-line — **no drift found** (the renderer
audit's drift-risk flag is real as a coupling, but current copies agree); workspace dependency
hygiene; docs/ ↔ embedded-docs mirror.

## 7. Suggested priority order

1. **Entity generation restore bug (1.1)** — silent aliasing on a shipped path; small, contained fix.
2. **Live-record replayability (1.2)** — the `--record` flag currently ships a false promise; decide
   the tick-record design (it also unblocks the seek roadmap cleanly).
3. **`SessionState` extraction (3.1) + snapshot reload-log (2.1)** — do together; the struct makes
   the log's clear/carry choice explicit and prevents the next divergence.
4. **`check_budget` key validation (1.6)** — one near-miss already; cheap warning.
5. **Dev supervisor crash recovery (1.4)** — quality-of-life for the core iterate loop.
6. **Normal matrix (1.5) + `MAX_LIGHTS` single-sourcing** — before more mesh content lands.
7. **Acked delivery or event lane (1.3) + client call timeout** — before agents lean harder on
   event verification.
8. **CLAUDE.md current-state + CI gate mirror + .loreignore + docs topics** — hygiene batch, one
   sitting.
9. **Duplication debt (§4)** — fold opportunistically when touching each area; the shader snippets
   and host extraction first.

Importer epsilon (1.7), macOS ui-hud bless (1.8, queued, human-gated), and the §5 doc rewords ride
along with whichever change next touches their files.
