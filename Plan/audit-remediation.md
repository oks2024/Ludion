# Audit remediation — plan of record

Remediation of `Plan/engine-audit-2026-07-17.md`, orchestrated as waves of independently
verified, independently committed work packages. This file tracks wave status and holds the
decisions deferred to the user. Finding numbers (1.1, §2, …) refer to the audit.

## Waves

### Wave 1 — independent bug fixes (parallel) — **DONE 2026-07-17, Lore revs 203–208, full check green**
- [x] **1.1 Entity generation restore** (rev 205) — snapshots carry full allocator state
  (dead-slot generations + free list) via an additive optional `SnapshotDoc` field written only
  when the live set can't reproduce the allocator; clean snapshots byte-identical, old snapshots
  fall back to the live-set path. Direction (a) chosen for replay-seek id determinism.
- [x] **1.6 Budget key validation** (rev 206) — hard `L4006` for unproducible keys: `[systems]`
  vs the report's own measured list (schedule runs all registered systems), `[passes]` vs the
  static HD-2D pass names; nearest-match help; `auto_fire`→`fire` near-miss is a regression test.
- [x] **1.4 Dev supervisor crash recovery** (rev 204) — `try_wait` probe on snapshot failure;
  alive child keeps serving, exited child relaunches unpaused/unrestored from the just-built exe.
- [x] **1.7 Importer emissive epsilon** (rev 203) — 8-ULP relative tolerance, largest-magnitude
  pivot channel, zero base channels still demand exactly-zero emissive.
- [x] **§5 hygiene batch** (rev 207) — all nine items. `Request::check_version` was *deleted*
  (not routed through): routing it post-deserialize would have let params-mismatch noise mask the
  L3003 a wrong-version client must hear; the early raw-JSON check stays, with the precedence
  pinned in a comment.
- [x] **§5 docs topics** (rev 208) — `ludion-audio`/`ludion-import`/`ludion-window` authored +
  embedded mirrors. Follow-up found: `docs/crate-graph.md` names none of the three crates —
  fold into the next hygiene touch.

### Wave 2 — engine-core seams — **DONE 2026-07-17, revs 210 + 212**
- [x] **3.1 `SessionState` extraction** + **2.1 snapshot reload-log** (rev 210, one package) —
  the five convention-managed session fields moved into `SessionState` with explicit
  `reset()`/`carry()`/`adopt_reloads()`; `SnapshotDoc` carries `doc_reloads` additively and
  restore re-applies it off-disk via the shared `reapply_doc_reloads`. The audit's predicted
  divergence was real and was even *pinned by a test* asserting the buggy disk-authoritative
  restore — rewritten as the fix's regression guard. Clean snapshots byte-identical.
- [x] **1.2 Live-record replayability** (rev 212) — free-running ticks batch into one
  `SimStep(n)` record flushed before any delta and at keyframes: no new record kind, no schema
  movement, live recordings structurally identical to lockstep ones, committed replays
  untouched. `Drop for Engine` flushes the trailing batch (no host calls `stop_recording`).
  Rider: applied-prefix input compaction bounds a live session's script (§2 growth item).
  Fallout fixed in rev 213: the server exit-criteria test recorded a free-running session and
  relied on those ticks being dropped — it now pauses first (lockstep), stable across runs.

### Wave 3 — renderer + protocol — **DONE 2026-07-17, revs 211 + 214 (+215 doc gate)**
- [x] **1.5 Normal matrix** + **§2 `MAX_LIGHTS`** (rev 211) — per-instance inverse-transpose
  normal matrix (attrs 10–13, `MeshInstanceRaw` 96→160 B), uniform scale = the old upper-3×3
  verbatim; all 12 golden suites byte-identical, zero re-bless. `LIGHT_COUNT` injected into
  mesh/sprite WGSL from the one Rust const; lights BGL pins `min_binding_size` so a mismatch
  fails pipeline validation loudly. (Graphics-engine feature notified: touches its
  shader-bindgen plan's cited facts.)
- [x] **1.3 Lossless events** + **§2 client timeout** (rev 214) — `EgressChannels` splits the
  latest-wins bounded(1) frame slot from a new unbounded ordered event lane, so events are
  gapless end to end (burst-tested through a real server on the GPU); `Acked` stays rejected
  (a real ack protocol = new wire messages = schema movement) with a message pointing at
  `latest_wins` as the lossless path; orphaned `Subscription.mode` removed.
  `Client::with_call_timeout` opt-in, default unchanged, L3009 on expiry. Schema hash unmoved.

### Wave 4 — duplication debt (§4, opportunistic) — **DONE 2026-07-17, rev 218, full check green**
- [-] Shared WGSL snippets (fullscreen-triangle VS; `Camera`/`Light` structs) — **handed to the
  graphics-engine feature** (its `Plan/shader-bindgen.md` owns the shader-source seam and is
  ready to start; duplicating the work here would collide — that instance was notified
  2026-07-17 and confirmed the handoff: dedup + a bindgen-side divergence guard are in their
  scope; our `LIGHT_COUNT` injection is interim, retired by their Wave 3 reflection-generated
  consts — nothing to unwind here).
- [x] `host_scene()` extraction (rev 218) — `ludion::host_scene` + `HostOptions` own the
  serve ↔ `ludion::run` common tail; the real asymmetries stay caller-side (`serve` prints the
  schema hash, `run` persists the player profile). Profile wrappers (`profile_report_json`,
  `check_budget_report`) + `ENGINE_KINDS` made pub (the facade's drift-tolerant copy deleted);
  `ludion-audio` dep dropped from the CLI.
- [x] Testkit helpers (rev 218) — `GoldenTest::single` (the whole per-OS golden recipe) +
  `driver` module (`loaded_engine`, `GameplayDriver` with `hold`/`release`/`tap`), applied to
  breakout/hello-paddle/sokoban. **vigil's adoption is left to the first-game feature**
  (it owns those tests and has a human gate in flight on them).
- [x] Small folds (rev 218) — `diff_fields` → engine `field_diff`, `scalar_string` →
  scene `scalar.rs`, `found_phrase` → core `value_phrase` (pub, serde_json promoted to a real
  core dep), and the scene `envelope.rs` module single-sourcing the scene/mesh/voxel
  format/kind/expect-string/unknown-key/missing-key checks. No new L-code: per-format wording
  and codes are preserved byte-identically (pinned by existing tests), so the fold is pure
  plumbing — the format-2 *migration story* stays an open design question, recorded, not
  built. `doc.rs`/`profile.rs` deliberately stay out (flattened-`Value` walkers; profile is
  lenient by design — rationale in the module docs). Facade-example behavioral tests (§3)
  were not taken — the facade's `check_docs_tests` already cover the seam that mattered.

## Deferred decisions (user)

Recorded here so they're decided from a written analysis, not under pressure. None block the
waves above.

1. **`schema_hash` connect-gate policy (§2).** The strict hash gate hard-fails older clients on
   *any* schema change (L3003), defeating the `#[non_exhaustive]` additive-evolution design.
   Options: (a) declare every schema change breaking — keep the strict gate, document it, accept
   that third-party clients rebuild on every rev; (b) compare a coarser compatibility key
   (protocol version + capability/method set), letting additive revs through. Matters the moment
   a client exists that isn't rebuilt in-repo. Decide before advertising the protocol to
   third-party clients.
2. **Frame backpressure lanes (§2).** Replies, events, and up-to-32 queued binary frames share
   one FIFO per connection; a slow TCP client delays replies behind frames and preferentially
   drops events. Not a correctness break today. Options: priority lanes per kind, or a separate
   socket/stream for frames. Grows with frame size — revisit when frames get bigger (textures)
   or a remote (non-LAN) client appears.
3. **Query construction O(all storages) (§2).** Query build sweeps every storage; scales with
   the registry, not the query. Negligible now; revisit when the component registry grows past
   a few dozen types (e.g. a name→storage index or per-archetype bookkeeping).

## Settled during remediation

- **Depth as a frame-graph resource (§2)** — deferred to the shadow-mapping phase, recorded as
  that phase's first wave in `Plan/phase-11-pbr.md` (out-of-scope section, 2026-07-17).
- **1.8 macOS ui-hud bless** — already queued in the mac bring-up checklist, human-gated on the
  pinned M4; not tracked here.
