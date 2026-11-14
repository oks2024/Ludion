# Master audit index

A rolling series of focused code audits, one feature per report. Each audit is **report only** — no
fixes applied — and every finding carries `file:line` evidence so it can be verified before it is
acted on. Earlier whole-repo audits: `Plan/engine-audit-2026-07-17.md`,
`Plan/code-audit-2026-07-25.md` (with its fix stage in `Plan/audit-remediation.md`).

Severity scale: **High** (wrong result, data loss, broken determinism, or a gate that passes when
it should fail) · **Medium** (a real defect with a narrow trigger, or a fragility the roadmap will
hit) · **Low** (cleanup, drift, minor waste).

## Remediation status

Fixes run as groups in `Plan/audit-remediation-2026-10.md`; each group's Status line there has the
detail (revisions, residuals, follow-ups).

- **Group 1 — gates** (Lore r1129–r1138): fixed gates Q1–Q14, ecs-par E1, E3, render-graph G1, G2.
- **Group 2 — drift gates and generated content** (Lore r1141, r1143–r1148): fixed mesh-edit-sculpt
  M1–M9, maps T2, T3, T10, fonts-ui F7, editor-parity X10; every derived writer now goes through
  `ludion_core::atomic` (theme "Derived writes without temp-and-rename"; the sync record write is
  atomic now, import-sync I4 itself stays with Group 10).
- **Group 3 — scene text and minimal diffs** (Lore r1150–r1156): fixed scene-text S1–S10, voxels Y2,
  Y3, Y4, Y6, editor-parity X9; f32 fields are written as their shortest f32 decimal, and committed
  hand-authored scenes are gated on the writer's spelling (`ludion_testkit::assert_saves_respell_nothing`).
- **Group 4 — diagnostics and error UX** (Lore r1158–r1167): fixed diagnostics L1–L13, wire-server W7,
  W8, ecs-par E6; the sim-side channel and `LogEvent` carry the whole diagnostic (theme "Diagnostics
  lose their payload in transit", L1 part), one severity-aware renderer serves the CLI and the game
  host, and `ci/scripts/diagnostic-prints.mjs` gates uncoded `error:`/`warning:` prints.
- **Group 5 — wire server and protocol clients** (Lore r1169–r1177): fixed wire-server W1–W6, W9–W13,
  editor-parity X5, X6, X13; chosen WebSocket limits (`MAX_MESSAGE_BYTES`, `L3045`) and a bounded
  shutdown (`L4022`) close two "Limits by library default" items, and the editor checks the schema hash.
- **Group 6 — snapshot, restore and `ludion dev`** (Lore r1179–r1185): fixed snapshot-dev D1–D9,
  physics P1, editor-parity X3, replay-pipeline R10; closes the "State outside the snapshot" theme's
  missing test (snapshot → restore → N ticks against the uninterrupted `WorldHash` trace, for a physics
  scene and for vigil).
- **Group 7 — replay pipeline and `WorldHash`** (Lore r1187–r1193): fixed replay-pipeline R1–R9, R11,
  ecs-par E2, E4, E5; replays fail on an unrecorded panic or a world reset inside a frame window,
  recordings close on a keyframe, and `ludion.worldhash.v2` distinguishes NaN, ±∞ and `None`.
- **Group 8 — transform hierarchy and physics** (Lore r1195–r1199): fixed hierarchy-prefabs H1, M1,
  L1–L6, physics P2–P11, spatial-nav N3, N7; keep-world reparent and prefab collapse use an exact
  world→local translation, a rebuild keeps momentum, static bodies follow their transform, and a collider
  nothing will run reports `L1261`/`L1262`.
- **Group 9 — maps and navigation** (Lore r1203–r1205): fixed maps T1, T4–T9, T11–T13, spatial-nav N1,
  N2, N4–N6, N9, N10; `map-check` probes expanded prefabs on a sub-cell lattice and judges `L1028` by
  reachability, marker-only maps declare `bounds:`, and the nav flow survives wall contact at one
  clearance derived from the bodies that read it.
- **Group 10 — asset import, sync and voxels** (Lore r1208–r1211): fixed import-sync I1–I9, I11, voxels
  Y1, Y5, Y7; a root's first sync waits for and recovers from another, case-only name collisions get
  distinct files or are refused, export copies under the sync lock, and `render.bounds` sizes voxel
  content by the GI voxelizer's grid rule.
- **Group 11 — lighting and rendering** (Lore r1216–r1225): fixed lighting O1–O8, render-graph G3–G9,
  patterns-sprites B5, particles K3, K4; generated tangents follow glTF's handedness, the shadow bias
  is in world units, ambient lights meshes and lit sprites take the sun, colours and light ranges are
  validated, the HDR chain stays finite, a lost device ends a windowed game, and the sorts are total.
- **Group 12 — animation and particles** (Lore r1227–r1236): fixed animation A1–A9, particles K1, K2;
  K5 parked for the user. Loop and start events never drop, `set_speed` keeps the clip phase, the wire
  carries `game_frame` for presentation stamps, bursts honour the emitter cap, skinned normals take the
  cofactor, and helper nodes and cubic clips import faithfully; closes the "Time derived from
  floating-point products" theme for clips (flipbooks are Group 13 B4).
- **Group 13 — sprites, patterns, fonts and screen UI** (Lore r1239–r1245): fixed patterns-sprites
  B1–B4, B6–B8, B10, B11, fonts-ui F1–F6; B9 parked for the user. Authoring errors past `loop` are
  `L1149`, a pattern reload the validator rejects is refused and a broken bullet atlas degrades per kind
  (theme "Diagnostics lose their payload in transit", B2 part), flip holds the pivot, flipbooks count
  integer clip ticks (closes the "Time derived from floating-point products" theme for flipbooks), one
  row per `UiList` item, CR/tab are whitespace, and one 8×8 face stamps every CPU label (theme "A fix
  applied at one site", F3 part).
- **Group 14 — audio** (Lore r1247–r1253): fixed audio U1–U13; U4's policy values parked for the user
  (the mechanism shipped). The Vorbis loop seam is granule-exact (theme "Tests that compare the system
  with itself", U1 part), the sink's routing and mixing run under test without a device (theme "Skip
  reads as pass", U2 part), a broken sound reference warns headless, voices fade instead of clicking
  and are bounded, effects share music's linear resampler (theme "A fix applied at one site", U5 part),
  emitters sync on change, and a lost device reopens with backoff.
- **Group 15 — editor host** (Lore r1256–r1263): fixed editor-parity X1, X2, X4, X7, X8, X11, X12, X14.
  The dev server is pinned with `strictPort` under a real CSP, a stage slot starts one engine and a stop
  reaps it (closes the "Crash-safety and concurrency claims untested at their edges" items for the
  editor), the host spawns the engine rather than `cargo run`, a stage reclaims only a dead editor's
  orphan, undo remaps every marked entity ref, and `check-scene --json` gives every kind a structured
  summary the readout parses (theme "A fix applied at one site", X12 part).
- **Group 16 — ComfyUI generation** (Lore r1265–r1275): fixed comfy-gen C1–C14. An accepted asset's
  origin comes from the run's own `run.json` (the two committed generated meshes re-accepted, origin
  only), the socket opens before submit and the mock no longer replays frames (theme "Tests that compare
  the system with itself", C2 part), one failed or lost job leaves the run `partial` instead of stranding
  it (theme "Crash-safety and concurrency claims untested at their edges", C3 part), uploads are
  content-addressed, run folders are created exclusively, and every `gen` verb prints its `--json`
  envelope on failure too, which the editor reads to keep and fetch a failed run.
- **Group 17 — vigil gameplay** (Lore r1277–r1284): fixed vigil-gameplay V2–V7, hierarchy-prefabs L7,
  spatial-nav N8, import-sync I10; V1 parked for the user. Gameplay selection
  moves off id rank to a marker (closes the theme "Identity by `EntityId`
  order"), and cadence timers run at their doc value through one helper pair (theme "A fix applied at one site",
  V4 part); vigil composes parent transforms in its occluders, nav census and kit reader; two tests that
  could not fail are gone (theme "Tests that compare the system with itself", V7 part). Also: a real
  2.0 m void leak in a reference map plugged, and vigil's seal test floods a sound bound.
- **Group 18 — close-out** (Lore r1286–r1312): every follow-up the groups left open is fixed or retired
  with a trigger (`Plan/audit-remediation-2026-10.md`, Group 18's Close-out ledger), and the themes were
  swept for sites no group reached: success on a wrong result (check-docs, a failed mirror commit, a live
  host's recording failure, an empty `ludion test --filter`, empty `node --test` globs), docs that promise
  what the code does not (patterns on CLI hosts, entity names and the WorldHash, flow YAML the reader
  refuses), tests that compare the system with itself (ten given independent oracles, each proven against
  its mutant), diagnostics in transit, validation outside the document path (wire `scene.save` of a
  generated scene, live reloads of every doc kind), derived writes (temp-and-rename for every
  user-visible writer, guarded by the `atomic-writes` gate), float time at tick boundaries, and skips that
  read as passes. map-check's seal check is sound and trusts no gate trigger as a wall; vigil's maps are
  sealed by caps behind their triggers. The git mirror's gap is measured and its repair is owed to the
  user.

## Audits

| # | Date | Feature | Report | High | Medium | Low |
|---|------|---------|--------|------|--------|-----|
| 1 | 2026-10-03 | Replay and record pipeline (`.lrp`, `replay_with`, `--profile-replay`, bisect) | [replay-pipeline](2026-10-03-replay-pipeline.md) | 0 | 3 | 8 |
| 2 | 2026-10-03 | Transform hierarchy and prefabs | [hierarchy-prefabs](2026-10-03-hierarchy-prefabs.md) | 1 | 1 | 7 |
| 3 | 2026-10-03 | Physics integration (`ludion-physics`, scene physics, snapshot/restore) | [physics](2026-10-03-physics.md) | 0 | 4 | 7 |
| 4 | 2026-10-03 | Wire server, protocol, dispatch, client, MCP | [wire-server](2026-10-03-wire-server.md) | 0 | 4 | 9 |
| 5 | 2026-10-03 | Scene text pipeline (YAML subset, canonical writer, schema validation) | [scene-text](2026-10-03-scene-text.md) | 2 | 1 | 7 |
| 6 | 2026-10-03 | Snapshot/restore and the `ludion dev` hot-reload loop | [snapshot-dev](2026-10-03-snapshot-dev.md) | 1 | 3 | 5 |
| 7 | 2026-10-03 | Asset import and `ludion sync` | [import-sync](2026-10-03-import-sync.md) | 1 | 2 | 8 |
| 8 | 2026-10-03 | Projectile patterns and the sprite engine | [patterns-sprites](2026-10-03-patterns-sprites.md) | 0 | 4 | 7 |
| 9 | 2026-10-03 | ECS core, `par` job system, `WorldHash`, determinism denylists | [ecs-par](2026-10-03-ecs-par.md) | 0 | 1 | 5 |
| 10 | 2026-10-03 | Render graph, extraction, frame streaming, golden gate | [render-graph](2026-10-03-render-graph.md) | 1 | 4 | 4 |
| 11 | 2026-10-03 | Editor (Tauri host, React frontend) and the parity gate | [editor-parity](2026-10-03-editor-parity.md) | 0 | 4 | 10 |
| 12 | 2026-10-03 | Fonts, text layout and screen UI | [fonts-ui](2026-10-03-fonts-ui.md) | 0 | 1 | 6 |
| 13 | 2026-10-03 | Particles and VFX | [particles](2026-10-03-particles.md) | 0 | 2 | 3 |
| 14 | 2026-10-03 | Skeletal animation and animation clips | [animation](2026-10-03-animation.md) | 0 | 3 | 6 |
| 15 | 2026-10-03 | Audio (SFX, music, sink, headless path) | [audio](2026-10-03-audio.md) | 0 | 4 | 9 |
| 16 | 2026-10-03 | ComfyUI generation pipeline (`ludion-gen`, `ludion gen`) | [comfy-gen](2026-10-03-comfy-gen.md) | 0 | 5 | 9 |
| 17 | 2026-10-03 | Mesh editing and the sculpt mesher | [mesh-edit-sculpt](2026-10-03-mesh-edit-sculpt.md) | 0 | 2 | 7 |
| 18 | 2026-10-03 | Verification gates (`check.mjs`, `ci.yml`, nextest profiles, budgets, bless paths) | [gates](2026-10-03-gates.md) | 1 | 5 | 8 |
| 19 | 2026-10-03 | vigil core gameplay (private audit) | vigil-gameplay (private audit) | 0 | 3 | 4 |
| 20 | 2026-10-03 | Spatial queries, nav fields, path following | [spatial-nav](2026-10-03-spatial-nav.md) | 0 | 3 | 7 |
| 21 | 2026-10-03 | Diagnostics system and error UX, engine-wide | [diagnostics](2026-10-03-diagnostics.md) | 0 | 4 | 9 |
| 22 | 2026-10-03 | Voxel models, assemblies, the `voxel` verb | [voxels](2026-10-03-voxels.md) | 0 | 1 | 6 |
| 23 | 2026-10-03 | Lighting and shading (PBR, shadows, IBL, GI, post) | [lighting](2026-10-03-lighting.md) | 1 | 4 | 3 |
| 24 | 2026-10-03 | Map generation, tilemaps, map drift gate | [maps](2026-10-03-maps.md) | 0 | 4 | 9 |
| | | **Total (24 audits)** | | **8** | **72** | **163** |

## Main findings

Most severe first, across all audits. Each line links back to its report.

- **High** — on the pinned-GPU lanes, a missing adapter (`L2003`) turns every golden and render test into a printed "SKIPPED" and a pass; nothing asserts the adapter is present, and those lanes are the only place the image gates run. ([gates Q1](2026-10-03-gates.md)) — fixed (Group 1, r1129–r1138)
- **High** — generated tangents carry the bitangent sign backwards for glTF's top-left UV origin (`ludion-import/src/tangents.rs:80`), so normal maps on meshes imported without `TANGENT` render green-inverted; no vigil asset exports `TANGENT`, so every triangle is affected, and the textured fixture's normal map tilts only in X, so it cannot catch it. ([lighting O1](2026-10-03-lighting.md)) — fixed (Group 11, r1216–r1225)
- **High** — the golden gate is colour-blind: SSIM runs on luma only and the per-pixel MAE is computed but never gates, so a fully grayscale render passes all 54 Windows goldens and an R↔B channel swap passes 39 of 54 (measured with an exact copy of the comparison over the committed goldens). ([render-graph G1](2026-10-03-render-graph.md)) — fixed (Group 1, r1129–r1138)
- **High** — `scene.save` writes every `f32` field as its widened 17-digit `f64` (`0.3` → `0.30000001192092896`, `1` → `1.0`), so an editor save of a hand-authored scene rewrites every short decimal; the editor's own TS writer emits shortest-`f32`, and the save tests only compare one save with the next. ([scene-text S1](2026-10-03-scene-text.md)) — fixed (Group 3, r1150–r1156)
- **High** — wire `component.set` / `entity.spawn` skip the JSON Schema check: a typo'd field (`translaton`) returns ok and silently resets `translation` and `rotation` to defaults. ([scene-text S2](2026-10-03-scene-text.md)) — fixed (Group 3, r1150–r1156)
- **High** — when the rebuilt child fails to boot, `ludion dev` discards the snapshot it already took and exits, status left at `building`; the likely trigger (a component rename or doc-kind change that breaks the boot scene) is exactly the edit hot reload exists for. ([snapshot-dev D1](2026-10-03-snapshot-dev.md)) — fixed (Group 6, r1179–r1185)
- **High** — a root's first `ludion sync` survives neither a second sync nor a kill: ownership is checked before the lock and the record is written only after every stem swaps in, so a concurrent sync gets `L1245` instead of waiting and a killed first sync leaves a tree every later run refuses until deleted by hand (both reproduced on a scratch copy of vigil's kits). ([import-sync I1](2026-10-03-import-sync.md)) — fixed (Group 10, r1208–r1211)
- **High** — `reparent` with keep-world moves the child's world position under a rotated, non-uniformly scaled parent (`Trs::inverse().mul_trs` applies inverse scale before inverse rotation; a child at x=1 lands at x=0.5). The editor outliner's drag sends exactly this; skinned-part placement shares the expression; the editor gizmo's TS inversion is correct, so engine and client disagree. ([hierarchy H1](2026-10-03-hierarchy-prefabs.md)) — fixed (Group 8, r1195–r1199)
- **Medium** — The drive loop accepts a system panic (`L4007`) on the last tick of every `sim.step`, live `drive_hashes` included, so a new panic can pass the committed-session round-trip; a one-tick-step recording never reports one. ([replay R1](2026-10-03-replay-pipeline.md)) — fixed (Group 7, r1187–r1193)
- **Medium** — `replay_render_frames`, `--profile-replay` and the bisector assume frames only go up; a recorded `scene.load`/`engine.restore` resets the frame and they overwrite PNGs, double-count profile frames or probe the wrong frame, reporting success. ([replay R2](2026-10-03-replay-pipeline.md)) — fixed (Group 7, r1187–r1193)
- **Medium** — `--profile-replay` names per-system timings by schedule position captured once; a rebuild that adds systems shifts samples onto wrong names, so `[systems_max]` gates the wrong system. ([replay R3](2026-10-03-replay-pipeline.md)) — fixed (Group 7, r1187–r1193)
- **Medium** — every `despawn`/`despawn_many` rebuilds the full parent→child map once any `Parent` exists, even for an empty id list; a vigil scene now has hundreds of links and ~15 per-tick call sites. ([hierarchy M1](2026-10-03-hierarchy-prefabs.md)) — fixed (Group 8, r1195–r1199)
- **Medium** — a snapshot taken on a frozen tick drops `fresh_colliders`; after restore the next blast hits a rebuilt body the uninterrupted run skips, so the resumed run diverges (reproduced). ([physics P1](2026-10-03-physics.md)) — fixed (Group 6, r1179–r1185)
- **Medium** — rebuilding a rigid body with no `Velocity` (any `mass`/`layer`/`lock`/shape edit) resets it to rest, contrary to `docs/physics.md` (reproduced). ([physics P2](2026-10-03-physics.md)) — fixed (Group 8, r1195–r1199)
- **Medium** — a rigid body or 3D collider in a scene with no `physics:` block silently does nothing; no diagnostic. ([physics P3](2026-10-03-physics.md)) — fixed (Group 8, r1195–r1199)
- **Medium** — moving a static body (gizmo, `component.set`, game code) never reaches the solver; the live session disagrees with its own reload. ([physics P4](2026-10-03-physics.md)) — fixed (Group 8, r1195–r1199)
- **Medium** — no WebSocket size limits are set anywhere, so tungstenite's 16 MiB frame / 64 MiB message defaults apply; a raw 4K frame is 33 MB, so `ludion dev --window` at 4K loops reconnecting and the MCP adapter retries the oversized request. ([wire W1](2026-10-03-wire-server.md)) — fixed (Group 5, r1169–r1177)
- **Medium** — shutdown waits forever on a stuck sim thread and Ctrl-C is honoured once; one `sim.step` with a huge tick count makes the host unkillable from its terminal. ([wire W2](2026-10-03-wire-server.md)) — fixed (Group 5, r1169–r1177)
- **Medium** — `profile.capture` on a paused or slowed sim never completes, then reports `L3023` blaming a looping system and wedges later captures with `L4013`. ([wire W3](2026-10-03-wire-server.md)) — fixed (Group 5, r1169–r1177)
- **Medium** — `L3012`'s help promises lossless event delivery; the server drops events when a client's outbound queue fills, and before a subscription registers. ([wire W4](2026-10-03-wire-server.md)) — fixed (Group 5, r1169–r1177)
- **Medium** — a data-doc or profile type with a top-level `kind`/`format` field makes `emit_doc` write a duplicate key (`L1007`); such a profile resets to default on every boot. ([scene-text S3](2026-10-03-scene-text.md)) — fixed (Group 3, r1150–r1156)
- **Medium** — if the snapshot call fails while the old child lives (a looping system → `L3023` after 120 s, or a CLI/game schema mismatch), `dev` reports `ok` and keeps serving the old binary. ([snapshot-dev D2](2026-10-03-snapshot-dev.md)) — fixed (Group 6, r1179–r1185)
- **Medium** — restore re-applies the session's doc-reload log over the disk walk: a doc edited alongside a code change either fails restore with `L1120` (and the kept snapshot fails the same way) or silently reverts to stale text. ([snapshot-dev D3](2026-10-03-snapshot-dev.md)) — fixed (Group 6, r1179–r1185)
- **Medium** — vigil's `NavField` is read before `update_flow_field` rebuilds it and is dropped by restore, so the first post-restore tick branches differently; no vigil snapshot→restore→N-ticks hash test exists. ([snapshot-dev D4](2026-10-03-snapshot-dev.md)) — fixed (Group 6, r1179–r1185)
- **Medium** — a `to` inside a source glTF's directory is never fresh (the hash covers the derived tree and the record), so every sync re-imports; manifest validation accepts the layout. ([import-sync I2](2026-10-03-import-sync.md)) — fixed (Group 10, r1208–r1211)
- **Medium** — mesh and clip name dedup is case-sensitive: on Windows/macOS `Wall` and `wall` collapse into one file holding the second mesh, with no warning; Linux yields two files from the same glTF. ([import-sync I3](2026-10-03-import-sync.md)) — fixed (Group 10, r1208–r1211)
- **Medium** — pattern timeline entries and repeats at or past `loop` never fire and a played pattern's own `loop` is ignored; the validator never reads `loop_frames`, so authored content is silently lost. ([patterns-sprites B1](2026-10-03-patterns-sprites.md)) — fixed (Group 13, r1239–r1245)
- **Medium** — a pattern reload with a broken atlas/frame pair is accepted (`L1151`/`L1152` only logged), then every frame fails while a volley of that kind is live; the pattern tab's normal two-step edit triggers it. ([patterns-sprites B2](2026-10-03-patterns-sprites.md)) — fixed (Group 13, r1239–r1245)
- **Medium** — `flip_x`/`flip_y` mirror the UV rect but not the pivot, so a flipped sprite with an off-centre pivot shifts sideways, contrary to `docs/sprites.md`. ([patterns-sprites B3](2026-10-03-patterns-sprites.md)) — fixed (Group 13, r1239–r1245)
- **Medium** — flipbook frame selection floors `ticks × (1/60) × fps` in floating point; at 15/30/60 fps some ticks show the previous frame (~4 % skipped at 60 fps). ([patterns-sprites B4](2026-10-03-patterns-sprites.md)) — fixed (Group 13, r1239–r1245)
- **Medium** — CI's `denylist-canary` job is a weaker inline copy of `ci/scripts/denylist-canary.sh`: it probes only `HashMap` and accepts any clippy failure as proof, so CI never shows the libm ban firing. No live determinism defect was found in `par`, the ECS or `WorldHash`. ([ecs-par E1](2026-10-03-ecs-par.md)) — fixed (Group 1, r1129–r1138)
- **Medium** — each of the 64 tiles is scored as one SSIM window, so small missing elements slip through: erasing each golden's most detailed 16×16 patch still passes 31 of 54. ([render-graph G2](2026-10-03-render-graph.md)) — fixed (Group 1, r1129–r1138)
- **Medium** — the sprite depth sort mishandles NaN (the particle sort was already fixed); a NaN sprite position can panic the sim thread, and stream renders are not panic-guarded. ([render-graph G3](2026-10-03-render-graph.md), also patterns-sprites B5) — fixed (Group 11, r1216–r1225)
- **Medium** — device-lost or a dead window surface leaves a windowed game frozen while it keeps simulating; the error reaches only the wire `log` channel, contrary to the render-graph doc. ([render-graph G4](2026-10-03-render-graph.md)) — fixed (Group 11, r1216–r1225)
- **Medium** — after one GPU hitch the 3-slot readback ring stays full whenever every pass submits, adding ~2 frames of stream latency for good (traced, not measured). ([render-graph G5](2026-10-03-render-graph.md)) — fixed (Group 11, r1216–r1225)
- **Medium** — the editor window loads from `localhost:5173` with no CSP and no `strictPort`; a foreign server already holding 5173 receives the host's commands (spawn any program, read/write any path). The parity gate itself holds. ([editor-parity X1](2026-10-03-editor-parity.md)) — fixed (Group 15, r1256–r1263)
- **Medium** — `engine.restore` replaces the world without emitting `scene.changed`, so the editor keeps its undo stack and selection and a later undo writes into whichever entity now holds that id; the drawer's agent is auto-allowed to call restore. ([editor-parity X3](2026-10-03-editor-parity.md)) — fixed (Group 6, r1179–r1185)
- **Medium** — `stage_start` marks the slot starting only after slow steps; StrictMode's double mount can spawn two engines, one untracked (suspected). On exit a still-starting slot is orphaned, holding port 9411 (suspected). ([editor-parity X2, X4](2026-10-03-editor-parity.md)) — fixed (Group 15, r1256–r1263)
- **Medium** — a `'
'` in a `UiList` title or item draws an extra line over the next row while measurement and `list_hit` count one row, so a click can return the wrong item; vigil works around it in game code. ([fonts-ui F1](2026-10-03-fonts-ui.md)) — fixed (Group 13, r1239–r1245)
- **Medium** — the 4096-per-emitter cap bounds continuous emission but not `burst`; a Rust-built emitter (which skips validation) can stall extract on the world-owning thread, contrary to the code comment. ([particles K1](2026-10-03-particles.md)) — fixed (Group 12, r1227–r1236)
- **Medium** — docs say stamp `started` with `ctx.game_frame`, but the example and breakout use `ctx.frame`, and the wire exposes only the sim frame, so an agent cannot fire a burst correctly in a game that pauses its clock; the effect can be reaped before it shows. ([particles K2](2026-10-03-particles.md)) — fixed (Group 12, r1227–r1236)
- **Medium** — a `t: 0` event on a looping clip is silently dropped on some wraps (a 1.1 s loop at 60 Hz skips cycle 15; 0.4/0.8/1.3 s also lose wraps), though the validator's help tells authors to put loop events at `t: 0`. ([animation A1](2026-10-03-animation.md)) — fixed (Group 12, r1227–r1236)
- **Medium** — `t: 0` events never fire for a clip started by a system registered after `animation_events` — the documented "switch on `end`" pattern used by the character-cycle example and vigil's `animate_player`. ([animation A2](2026-10-03-animation.md)) — fixed (Group 12, r1227–r1236)
- **Medium** — changing `Animation.speed` mid-clip rescales all elapsed time: the pose jumps and events fire twice or are skipped; the platformer's velocity-driven run cycle will hit it. ([animation A3](2026-10-03-animation.md)) — fixed (Group 12, r1227–r1236)
- **Medium** — the music loop seam is not gapless: the Ogg decoder returns the encoder's padding (22080 decoded vs 22050 declared on the test file) and it plays at every wrap; the gapless test passes because its expected length includes the padding. ([audio U1](2026-10-03-audio.md)) — fixed (Group 14, r1247–r1253)
- **Medium** — the sink's message routing (one-shots, loops, stop-all, music, refill, dead streams) runs in no test: CI has no device and takes the silent path, the device-box tests send no messages. ([audio U2](2026-10-03-audio.md)) — fixed (Group 14, r1247–r1253)
- **Medium** — headless runs never decode sounds, so a broken path or bad WAV warns nowhere; vigil's hand-kept path list misses data-named sounds. ([audio U3](2026-10-03-audio.md)) — fixed (Group 14, r1247–r1253)
- **Medium** — no voice cap and no stealing: a sound re-fired every few ticks stacks copies and saturates the limiter — a likely shmup problem. ([audio U4](2026-10-03-audio.md)) — fixed (Group 14, r1247–r1253); policy values parked: user decision
- **Medium** — an accepted asset's `prompt` and `inputs` are read from the recipe at accept time, not submit time; accepting from an older run records a prompt that never made the asset, with no recipe hash to catch it. ([comfy-gen C1](2026-10-03-comfy-gen.md)) — fixed (Group 16, r1265–r1275)
- **Medium** — `gen run` submits before opening `/ws`; a fully cached resubmission finishes unseen and waits out the 600 s timeout to `L1170` (suspected; the mock replays every frame on connect, so tests cannot see it). ([comfy-gen C2](2026-10-03-comfy-gen.md)) — fixed (Group 16, r1265–r1275)
- **Medium** — one failed job sinks the run: `status`, `fetch` and `accept` all stop on its `L1169`, stranding the good candidates. ([comfy-gen C3](2026-10-03-comfy-gen.md)) — fixed (Group 16, r1265–r1275)
- **Medium** — uploads use the local file name with `overwrite=true`; chained `--image 0003.png` runs overwrite each other server-side and provenance records the wrong hash (suspected). ([comfy-gen C4](2026-10-03-comfy-gen.md)) — fixed (Group 16, r1265–r1275)
- **Medium** — the `--json` contract holds only on success: six of seven `gen` verbs print nothing on stdout when they fail, so the editor cannot find a timed-out run's folder. ([comfy-gen C5](2026-10-03-comfy-gen.md)) — fixed (Group 16, r1265–r1275)
- **Medium** — two mesh-edit documents naming the same `out` silently overwrite each other's tree, and the overwritten edit drops out of the drift gate. ([mesh-edit-sculpt M1](2026-10-03-mesh-edit-sculpt.md)) — fixed (Group 2, r1141, r1143–r1148)
- **Medium** — `mesh-edit-drift.mjs` discovers edits through their derived outputs, so a tree orphaned by an `out` change is never compared and a committed edit with no tree passes vacuously. ([mesh-edit-sculpt M2](2026-10-03-mesh-edit-sculpt.md)) — fixed (Group 2, r1141, r1143–r1148)
- **Medium** — with no adapter or no timestamp support, a budget check skips every `[passes]` and `[post_groups]` line yet prints "all timings within budget" and exits 0. ([gates Q2](2026-10-03-gates.md)) — fixed (Group 1, r1129–r1138)
- **Medium** — hosted CI's `no-gpu` profile excludes 40 whole test binaries (~360 tests), not just GPU tests; CPU-only drift tests in `import.rs` that `map-drift` relies on never run in hosted CI. ([gates Q3](2026-10-03-gates.md)) — fixed (Group 1, r1129–r1138)
- **Medium** — CI's `thread-identity`, `probe` and `soak` jobs skip the derived-import sync step though vigil's scenes reference the derived tree (effect inferred). ([gates Q4](2026-10-03-gates.md)) — fixed (Group 1, r1129–r1138)
- **Medium** — `check.mjs --full` edits `crates/ludion-core/src/lib.rs` in the shared checkout for minutes and a killed run leaves the canary in place; five trace tests re-bless when `LUDION_BLESS` is `0` or empty, and `BLESS_SCHEMA` is not refused. ([gates Q5, Q6](2026-10-03-gates.md)) — fixed (Group 1, r1129–r1138)
- **Medium** — a reference-game gameplay finding (vigil-gameplay V1, private audit) — parked: user decision
- **Medium** — a reference-game gameplay finding (vigil-gameplay V2, private audit) — fixed (Group 17, r1277–r1284)
- **Medium** — a reference-game gameplay finding (vigil-gameplay V3, private audit) — fixed (Group 17, r1277–r1284)
- **Medium** — `map-check` silently drops prefab instances and `Parent` from its occupancy probe: on `district-breach.map.yaml` it counted 29 colliders, called the town square almost all walkable and raised three bogus `L1028`s. ([spatial-nav N1](2026-10-03-spatial-nav.md)) — fixed (Group 9, r1203–r1205)
- **Medium** — a body pressed against a wall usually stands on a nav cell marked blocked (75 % for swarm, 50 % for rushers), gets no flow direction and walks straight into the wall — the wall-hugging G6 was built to remove (mechanism verified, impact suspected). ([spatial-nav N2](2026-10-03-spatial-nav.md)) — fixed (Group 9, r1203–r1205)
- **Medium** — a game that never schedules `spatial::rebuild` gets no 2D collision and no diagnostic — the 2D twin of physics P3. ([spatial-nav N3](2026-10-03-spatial-nav.md)) — fixed (Group 8, r1195–r1199)
- **Medium** — the sim-side diagnostics channel keeps only code and message: every runtime and data-doc diagnostic loses help, pointer and severity and logs as a bare warn line the editor cannot parse into a code chip. ([diagnostics L1](2026-10-03-diagnostics.md)) — fixed (Group 4, r1158–r1167)
- **Medium** — the CLI's `report` (and the game host's copy) counts warnings as errors: a scene with only an `L1209` warning prints "1 error found" and `check-scene` exits 1. ([diagnostics L2](2026-10-03-diagnostics.md)) — fixed (Group 4, r1158–r1167)
- **Medium** — `L1015` "scene file could not be read" also covers writes, deletes, listings and a JSON parse, and its `explain` text contradicts most call sites on path base. ([diagnostics L3](2026-10-03-diagnostics.md)) — fixed (Group 4, r1158–r1167)
- **Medium** — a bad `LUDION_THREADS`/`LUDION_QUALITY`/`LUDION_RENDER_SIZE` blocks every verb, including the `ludion explain` that would explain it. ([diagnostics L4](2026-10-03-diagnostics.md)) — fixed (Group 4, r1158–r1167)
- **Medium** — `render.bounds` reports every voxel model, assembly and part as a scaled unit box, 4–32× too small (a 4-unit model reads 0.125 tall), so the editor's click-picker misses the model; the GI voxelizer uses the real grid size while its comment claims the same rule. ([voxels Y1](2026-10-03-voxels.md)) — fixed (Group 10, r1208–r1211)
- **Medium** — `shadow_bias` is in shadow-depth units, so its world size scales with `shadow_far - shadow_near`: ~0.4 units at vigil's 1/200, so props under ~0.18 units cast no shadow and wall bases show a lit gap; no slope-scaled term. ([lighting O2](2026-10-03-lighting.md)) — fixed (Group 11, r1216–r1225)
- **Medium** — `Environment.ambient` does nothing to meshes: any `Environment` takes the IBL path, which never reads it, while the docs say it lights every surface. ([lighting O3](2026-10-03-lighting.md)) — fixed (Group 11, r1216–r1225)
- **Medium** — lights accept negative intensity and colour; `shadow_extent: 0` or `near == far` puts NaN in the sun's light-space matrix. ([lighting O4](2026-10-03-lighting.md)) — fixed (Group 11, r1216–r1225)
- **Medium** — at the 0.045 roughness clamp specular can exceed fp16 max; an inf in the scene texture becomes NaN under Karis bloom weighting (suspected). ([lighting O5](2026-10-03-lighting.md)) — fixed (Group 11, r1216–r1225)
- **Medium** — `rows: []` silently drops every map marker: `generate-map` exits 0 and `map-check` prints only "grid: empty"; vigil pads a map with a 24×20 all-space grid to dodge it. ([maps T1](2026-10-03-maps.md)) — fixed (Group 9, r1203–r1205)
- **Medium** — `map-drift` finds work only through outputs, so a `.map.yaml` with no committed scene passes, and `L1027` resolves the in-memory sibling — a new district can gate green and fail at runtime. ([maps T2](2026-10-03-maps.md)) — fixed (Group 2, r1141, r1143–r1148)
- **Medium** — `map-drift.mjs` never deletes its temp dir: 313 leaked `ludion-map-drift-*` dirs (1.4 GB) on the reference box. ([maps T3](2026-10-03-maps.md)) — fixed (Group 2, r1141, r1143–r1148)
- **Medium** — `L1028` treats only a grid-edge gap as a gate opening, so fragment-composed districts carry three permanent "gate sealed" warnings that the spatial-nav N1 fix will not clear. ([maps T4](2026-10-03-maps.md)) — fixed (Group 9, r1203–r1205)
- **Low** — `stop_recording` writes no closing keyframe: keyframe checks are vacuous under 300 frames (vigil's `stress.lrp` only checks an empty world). ([replay R7](2026-10-03-replay-pipeline.md)) — fixed (Group 7, r1187–r1193)
- **Low** — `reparent` checks only the moved entity's depth, so descendants can silently exceed the 64-level limit and degrade to roots. ([hierarchy L3](2026-10-03-hierarchy-prefabs.md)) — fixed (Group 8, r1195–r1199)

## Cross-cutting themes

Patterns that recur across more than one audit.

- **Success reported on a wrong result.** Tools that finish green while doing the wrong thing (replay R1, R2, R3, R7; hierarchy H1 vs its own doc claim; physics P3, P4; spatial-nav N1, N3; wire W3; scene-text S2; snapshot-dev D2; patterns-sprites B1, B2). The engine's error discipline is strong at input validation and weaker at checking its own invariants on the way out. Each is fixed (Groups 3–9, 13).
- **Docs promise what the code does not.** `docs/hierarchy.md` on keep-world reparent (hierarchy H1), `docs/physics.md` on velocity across rebuilds (physics P2), `dev-docs/physics-pin.md` on the vendored diff (physics P5), `determinism.md` calling glam's `slerp` libm-pure and `L4007`'s help on resume (ecs-par E2, E6), `L3012` promising lossless events (wire W4), `Environment.ambient` said to light meshes (lighting O3), `docs/sprites.md` on flip with a pivot (patterns-sprites B3), three UI docs still describing the fixed 8×8 face (fonts-ui F5), the particle example and `docs/voxel-animation.md` contradicting the other docs on `game_frame` (particles K2, animation A6), the error schema marking `related` required (wire W7). Each was fixed in its group, and Group 18's sweep corrected the sites no audit named (patterns played by CLI hosts, names outside the WorldHash, a missing `visible` flag, copyable flow YAML the strict reader refuses, five dev-docs).
- **State outside the snapshot.** Derived per-tick state the snapshot forgets breaks restore-then-resume determinism (physics P1, snapshot-dev D4; replay R10 arms on a non-reproducible session). A snapshot → restore → N ticks test against the uninterrupted `WorldHash` trace covers a physics scene and vigil (Group 6).
- **Limits by library default.** Sizes and waits nobody chose: WebSocket frame caps (wire W1), unbounded shutdown wait (wire W2), hostile tick counts overflowing `total_frames` (replay R9). The WebSocket limits and the shutdown wait are chosen now (Group 5), tick counts are bounded (Group 7).
- **Tests that compare the system with itself.** Round-trips checked save-vs-save rather than source-vs-save (scene-text S1), replay accepting the panic the recording also hit (replay R1), keyframe checks with no keyframe to check (replay R7). CI's denylist canary accepts any clippy failure as proof (ecs-par E1); the gapless-loop test bakes the defect into its expectation (audio U1); the ComfyUI mock replays events a real server would not (comfy-gen C2). Test fixtures too symmetric to expose the bug (lighting O1's X-only normal map). The golden gate cannot see colour (render-graph G1, G2), and skips to green with no adapter (gates Q1, Q2). Gates need an independent oracle; the fixed ones got one (Groups 1, 3, 7, 11, 14, 16).
- **Crash-safety and concurrency claims untested at their edges.** fe19a265's "concurrent syncs wait" fails on the first sync (import-sync I1, I4, I5); one failed ComfyUI job strands a whole run (comfy-gen C3); the editor's stage slot races and orphans on exit (editor-parity X2, X4); `ludion dev` loses its snapshot on a failed boot (snapshot-dev D1); shutdown blocks on a stuck sim (wire W2). The kill, the race and the failed step are tested now (Groups 5, 6, 10, 15, 16).
- **A fix applied at one site, not its siblings.** The CLI's `report` was copied into the game host with its warning-as-error bug (diagnostics L2); the particle sort's NaN fix never reached the sprite sort (patterns-sprites B5, render-graph G3); the editor gizmo inverts transforms correctly while the engine does not (hierarchy H1); two window presenters have drifted (render-graph); bounds and the GI voxelizer size voxel content differently (voxels Y1); SFX resample nearest-neighbour beside music's linear resampler (audio U5); three hand-kept 8×8 glyph tables duplicate the embedded font (fonts-ui F3). Each site is fixed (Groups 4, 8, 10, 11, 13–15, 17). When fixing a class of bug, grep for the pattern.
- **Validation lives in the document path only.** Rust-built or wire-set values skip the checks the file path enforces, and some values are never range-checked at all (scene-text S2, particles K1, K3, lighting O4); the guard should sit where the value is consumed, or in the type (fixed in Groups 3, 11, 12).
- **Time derived from floating-point products at tick boundaries.** Flipbook frames (patterns-sprites B4), clip-loop events (animation A1) and speed changes (animation A3) all compute a discrete index from `ticks × dt × rate` in float; exact-boundary ticks land on the wrong side. Integer tick arithmetic, or a carried phase, removes the class (clips Group 12, flipbooks Group 13).
- **Drift gates discover work from outputs, not sources.** The font atlas gate (fonts-ui F7), the mesh-edit gate (mesh-edit-sculpt M2) and the map gate (maps T2) enumerate generated files, so a source with no output — or an output orphaned from its source — is never checked. Enumerate sources; assert each has exactly one up-to-date output and no output lacks a source (fixed for fonts, mesh-edit and maps, Group 2).
- **Minimal diffs broken by write-side tools.** `scene.save` widens floats (scene-text S1) and `voxel` grow/crop unquotes untouched rows (voxels Y3); `voxel apply` edits a `generated_from:` model without warning (voxels Y4); all three are fixed (Group 3).
- **Derived writes without temp-and-rename.** Sync records (import-sync I4), mesh-edit trees (mesh-edit-sculpt M3), ComfyUI downloads (comfy-gen) wrote in place; every derived writer now goes through `ludion_core::atomic` (Group 2).
- **Skip reads as pass.** A missing GPU adapter, timestamp support or audio device turns a gate into a printed skip and exit 0 (gates Q1, Q2; audio U2). Lanes that exist to run a gate assert their precondition and fail when it is absent (Groups 1, 14).
- **Identity by `EntityId` order.** "First match by id" is deterministic but not *meaningful*: id order follows free-list history, so a different entry route picks a different entity (vigil-gameplay V2). Select by a marker or name, never by id rank; the escort does (Group 17).
- **Diagnostics lose their payload in transit.** Help, pointer and severity are dropped on the sim channel (diagnostics L1); render device loss reaches only the `log` channel (render-graph G4); pattern reload errors are only logged (patterns-sprites B2). The structured diagnostic is built correctly and then flattened before anyone reads it; the channel carries the whole diagnostic now (Groups 4, 11, 13).
