# Audit remediation, October 2026 — plan of record

Fixes every finding of the 24 focused audits of 2026-10-03 (`Plan/audits/master-audit.md`:
8 High, 72 Medium, 163 Low). The work runs as **groups**, strictly one at a time. Each group goes
to a freshly spawned Tech Lead, who splits it into subtasks, commits each verified subtask through
`scripts/commit.mjs`, and reports DONE before the next group starts. Finding IDs (`Q1`, `S2`, …)
refer to the audit reports in `Plan/audits/`; each report's "Suggested order of work" is the
default order inside a group.

## Rules for every group

- **Verify before fixing.** A finding marked `[suspected]` or `[verified by reading]` is reproduced
  first, ideally as a failing test. A finding that does not reproduce is closed in the report as
  "not reproduced", with the evidence.
- **Every High and Medium fix lands with a test that fails before it.** Lows get a test when it is
  cheap. A test must use an independent oracle: compare against the source, not against the
  system's own previous output (master index, "Tests that compare the system with itself").
- **Fix the class, not the site.** Before closing a finding, grep for the same pattern elsewhere
  (master index, "A fix applied at one site, not its siblings"), and fix the siblings in the same
  group when they are in its area. Record the rest as follow-ups.
- **Docs move with the code.** A behaviour change updates its `docs/` or `dev-docs/` topic in the
  same subtask (`check.mjs --fix` re-mirrors `embedded-docs/`). Stale-doc findings are fixed, not
  deferred.
- **Hash and golden discipline.** A change that moves `WorldHash` or the schema hash re-records the
  `.lrp` corpus last in its group, after the schema snapshot. Golden re-blesses run on the
  reference box (RTX 3090) and only after looking at the before/after image; a re-bless is never
  the way to make a red gate green without that look.
- **Product decisions go to the user.** The Tech Lead asks them in one batch at the start of its
  group. The known ones are listed per group below.
- **Status lives here.** When a group closes, its Tech Lead fills in the group's Status line
  (Lore revisions, findings fixed / not reproduced / deferred with reason), and marks the
  findings fixed in the audit reports' master index.

## Groups

Order rationale: gates first, because every later group is verified by them. Then the text and
diagnostics foundations that later fixes write through. Then the determinism core (snapshot,
replay), then the engine subsystems, then the tools, and vigil last, because it moves the replay
corpus and depends on several engine fixes (N1, D4, H1).

### Group 1 — Gates you can trust
Findings: gates Q1–Q14; ecs-par E1, E3; render-graph G1, G2.
Goal: no lane reports green when its precondition is missing (an adapter, timestamps, the
derived tree), the golden comparison sees colour and small vanished elements, the bless
switches have one meaning, and `--full` stops editing the shared checkout.
Watch for: tightening G1/G2 may turn existing goldens red; each one is looked at before it is
re-blessed (it may be a real, so far invisible, regression). Mirror every gate change in
`check.mjs` and `.github/workflows/ci.yml` + `ci/scripts/`.
Status: **closed** — Lore r1129–r1138. All 18 findings fixed, none deferred or "not reproduced":
Q2+Q14 (r1129, `L4019`), Q1 (r1130, `ludion_core::require_gpu`, `LUDION_REQUIRE_GPU=1`), Q6+E3
(r1131, `ludion_core::bless`, denylist gains `elapsed` and `Builder::spawn*`), Q5+E1+Q12+Q11 (r1132,
one Node script per gate in `ci/scripts/`, `check.mjs` lock, fuzz crate linted), Q4+Q9+Q10+Q8+Q13
(r1133, derived-imports guard, per-test and per-job ceilings), Q3+Q7 (r1134, `needs_gpu_*` names,
`LUDION_FORBID_GPU=1`), G1+G2 (r1135, windowed SSIM + per-tile colour ceiling, `L2207`; no golden
moved). A whole-group review led to r1136–r1138: soak/probe ceilings, TeamCity `cmd` env syntax and
mirroring, POSIX process-group kill, software adapters refused under `LUDION_REQUIRE_GPU`, the import
guard checks source hashes (enumeration moved to `ludion-assets::import_sync`), must-differ tests
assert extent, crate- and manifest-level denylist allows rejected.
Owed: the M4 golden re-check under the new comparison (`Plan/owed.md`); the POSIX kill-tree path is
untested on macOS. Follow-ups (unproven review suspicions, left for Group 18's sweep): the bless
direct-read guard is single-line only; the "prefixed test touches no device" heuristic is weak (a
CPU test misnamed `needs_gpu_` leaves hosted CI); `L4019` trusts `available && timestamps_supported`;
the worst-window floor may trip on non-pinned consumers; an equal-luma 16×16 colour change stays
under the 4.0 ceiling (documented); `gate-scripts.test.mjs` skips schema-drift without
`node_modules`; external games' goldens are not in the oracle; the probe and thread-identity
60-minute ceilings are unmeasured. Pre-existing, outside Group 1: a vigil
probe test fails in `--full` (→ Group 17).

### Group 2 — Drift gates and generated content
Findings: mesh-edit-sculpt M1–M9; maps T2, T3, T10; fonts-ui F7; editor-parity X10.
Goal: every drift gate enumerates sources, and asserts each source has exactly one up-to-date
output and no output lacks a source; derived writes go through temp-and-rename; the
`patternSchema.ts` copy gets a drift gate or is generated.
Status: **closed** — Lore r1141, r1143–r1148 (r1142 is unrelated: a pre-existing uncommitted
mesh-edit `color_feather` feature, committed first as the base on the user's word). All 15 findings
fixed, none deferred or "not reproduced": X10 (r1141, `patternSchema.ts` generated from a pinned
ludion-engine snapshot, checked by schema-drift; the hand copy had drifted), M1+M3–M6+M8+M9 docs
(r1143, `L1176` ownership, `L1174` source-in-out, PNGs → document → stale), M7 (r1144, the hidden-part
search runs only in `unresolved_parts`), M2+M9+T2+T3+T10+F7 (r1145, `scripts/drift-lib.mjs`
source-driven two-way pairing, `map-drift` via `generate-map --check`). Class fix r1146: one
`ludion_core::atomic::write_atomic` for every derived writer (atlas, map, import, sync, ludion-gen,
testkit goldens), five private copies removed. A whole-group review led to r1147–r1148: `.mjs`
generators are sources (`// ludion: drift-generator` marker), an empty scan fails, per-thread temp
names, owned-temp-only sweep, case-folded path checks, no silent overwrite on a read error.
Follow-ups (Group 18): `voxel-drift.mjs` is output-driven; non-derived writers (render, profile,
export, new, the facade, replay sinks) are not atomic; no write-order test for `sync.rs` or
ludion-gen's writers; an unmarked generator whose scenes all lose `generated_from` stays invisible.

### Group 3 — Scene text and minimal diffs
Findings: scene-text S1–S10; voxels Y2, Y3, Y4, Y6; editor-parity X9.
Goal: load → save of a hand-authored scene is a fixed point (S1, with a gate over the committed
scenes), the wire path runs the schema checker (S2), and no write-side tool rewrites lines it did
not touch or edits a `generated_from:` document without saying so.
Watch for: S1 moves committed derived trees; sequence it with the import and mesh drift gates.
Status: **closed** — Lore r1150–r1156. All 15 findings fixed, none deferred: S2+S10+S6 (r1150,
`ludion_scene::check_component` on `component.set`/`entity.spawn`; S10 does not arise from derives,
since schemars 1.x merges a flattened struct, so it was fixed and tested on a hand-written `allOf`),
S3+S4+S5+S9 (r1151, new `L1257`, `L1037`), Y2+Y3+Y4+Y6 (r1152, six generated models requoted
through their scripts), S1+S8 (r1153, `format_f32_float` at `format: float` positions; ragdoll and
import fixtures regenerated; WorldHash, schema hash and `.lrp` corpus unmoved), the S1 corpus gate
(r1154, a one-time same-value respelling of the committed scenes, comments kept), X9+S7 (r1155). User
decisions: the corpus gate checks value spelling, not bytes, because `scene.save` drops comments; X9
disables Save and names the generator; Y4 refuses. A whole-group review found 1 Medium (the gate's
f32 tolerance applied to every field type, so it could have blessed a changed u64 or f64; no damage)
and 6 Lows, all fixed in r1156. Follow-ups (Group 18): `scene.save` drops every YAML comment (70 of 82
hand-authored scenes carry some; needs a comment-preserving writer); `scene.save` over the wire still
writes a `generated_from:` scene (the refusal is editor-only; changing it changes the carry-forward
contract); a flattened internally tagged enum (`oneOf`) and `additionalProperties` inside an `allOf`
part are not modelled by the checker; `engine.restore` snapshots are not schema-checked; the
`scene.save` `SCENE_IO` help is stale; BOM + `%YAML 1.2` reads as a second document (saphyr);
`part.rs` and `sprite.rs` still write letter-led rows bare; the fuzz oracle is not mutation-proven
against the f32 rule applied to an f64 field; the vigil respell gate is the slowest new test (~33 s
under load).

### Group 4 — Diagnostics and error UX
Findings: diagnostics L1–L13; wire-server W7, W8; ecs-par E6.
Goal: the sim-side channel carries the whole diagnostic (help, pointer, severity); warnings
never fail a command; `L1015` is split into honest codes; a bad environment variable never
blocks `explain` or `docs`; one JSON Pointer escaper.
Status: **closed** — Lore r1158–r1167. All 16 findings fixed, none deferred or "not reproduced":
L4+L5+L10 (r1158, `RETIRED` code table), L2+L9 (r1159, `ludion_core::diagnostics::render_report`
replaces six copies; L2's suspected prefab refusal reproduced and fixed; `with_severity` pinned to two
named sites), L3+E6 (r1160, new `L1038` output I/O, `L2208` manifest parse, `L0029` silent failure),
L7+L8 (r1161–r1162, new `L0030`–`L0034`, `L1258`, `L1259`, `L4020`, `L4021`, the
`ci/scripts/diagnostic-prints.mjs` gate in check.mjs and CI), L11+L12+L13+W8 (r1163), L1+W7 (r1164,
`report_full_diagnostic`, optional `LogEvent.diagnostic`; schema hash `d5f769fa052ee453` →
`1ebb24651f97ed0f`, the 13 committed `.lrp` re-recorded, `WorldHash`, goldens and budgets unmoved), L6
(r1165, banner live smoke queued in `Plan/owed.md`). User decisions: L1 adds the structured `LogEvent`
field; L11 documents both `path` grammars instead of migrating. A whole-group review found 2 Medium
(library fault strings still flattened diagnostics into log text; a refused `entity.reparent` carried
the warning lint `L1220`) and 8 Low, all fixed in r1166 (`L1260`) and r1167 (`L3044`, `L0030` widened
to every create-target-exists refusal). Follow-ups (Group 18): migrate data-doc paths to
`<file>#/<pointer>`; `check-scene`, `sync`, `generate-*`, `voxel`/`sculpt`/`mesh` still resolve the
`LUDION_*` knobs up front; `dev.rs` `status_from_diagnostics` hard-codes level `error`; `gen` prints a
downgraded `L1167` but stores the original severity; `ludion-gen` write failures use `L1162` /
accept-refused instead of `L1038` (comfy-gen group); `ludion-client` codes unparseable replies `L3001`;
the print gate does not scan `editor/app/src-tauri`; phase4's 30 fps exit test failed twice under
`check.mjs` load during the group.

### Group 5 — Wire server and protocol clients
Findings: wire-server W1–W6, W9–W13; editor-parity X5, X6, X13.
Goal: chosen WebSocket size limits and timeouts instead of library defaults; a bounded,
repeatable shutdown; `profile.capture` that knows a paused sim from a stalled one; event delivery
that matches what `L3012` promises (or a promise that matches the delivery); TS client checks
`schema_hash` and settles every connect.
Status: **closed** — Lore r1169–r1177. All 14 findings fixed, none deferred or "not reproduced": W1+W10
(r1170, one 256 MiB `ludion_protocol::MAX_MESSAGE_BYTES` on server, client and editor host, `L3045`; the
MCP adapter probes with `Client::check_open` and never re-sends), W2+W5 (r1171, the sim join bounded by
`SIM_STOP_GRACE`, `L4022` naming the sim's activity, a second Ctrl-C exits 130, a 10 s handshake
deadline), W9+X5+X6 (r1172, generated `schemaHash.ts` = XXH3 of the snapshot, a persistent `L3003`
banner while staying connected, connects settle on close/timeout/error envelope), W3+W12 (r1173, `L4023`
refuses a capture that cannot close, transport rings hold `MAX_CAPTURE_FRAMES`), W4+W11+W6 (r1174, a
subscribe registers out of FIFO with a bounded in-flight buffer, prompt disconnect unsubscribe, `L3046`),
X13+W13 (r1175, `query.entities` `entities` filter, `SubscribeMode::Acked` dropped and `L3012` retired;
schema hash `1ebb24651f97ed0f` → `21ce1bc54b32e9ad`, the 13 `.lrp` re-recorded, `WorldHash`, goldens and
budgets unmoved). Also r1169: phase4's 30 fps gates (failing only under box load, ~60 fps alone) run as
`check.mjs`'s quiet `fps-gate` tail and report the render-only ceiling on failure. User decisions: the
fps tail; X13 adds the wire field; Acked dropped; W9 warns rather than refuses. Tech Lead decision: a
capture on a paused sim stays refused (sync clients cannot pipeline the closing step), docs say resume,
capture, pause. A whole-group review found 2 Medium (the mid-flight capture cancel assumed nominal tick
rate and cancelled captures on slow hosts; a windowed host still hung on close with a wedged sim) and
7 Low, all fixed in r1176–r1177 except the gizmo's two-query pose. Owed: the schema-drift banner and
wedged-sim window-close live smokes (`Plan/owed.md`). Follow-ups (Group 18): the `L4022` stuck path
cannot flush an armed recording; the embed listener path joins boot bounded only through server
teardown; a host rendering slower than its tick rate still times a capture out as `L3023`; the editor's
Performance view could disable Capture while paused; `prefab-actions.ts` still reads the whole world
(could use `entities`); the gizmo pose reads parent and child in two queries (one-refresh skew).

### Group 6 — Snapshot, restore and `ludion dev`
Findings: snapshot-dev D1–D9; physics P1; editor-parity X3; replay-pipeline R10.
Goal: `dev` never loses a snapshot and never reports a failed reload as ok; restore honours the
disk; no per-tick state lives outside the snapshot. Adds the missing test the master index calls
for: snapshot → restore → N ticks equals the uninterrupted run over the full state, for a
physics scene and for the reference game.
Status: **closed** — Lore r1179–r1185. All 12 findings fixed, none deferred or "not reproduced": P1
(r1179, `fresh_colliders` and `last_step_dt` ride the physics blob; a full-`WorldHash` restore taken
mid-freeze matches the uninterrupted run), D1+D3+D9 (r1180, the snapshot is written to
`.ludion/dev-snapshot.json` before shutdown, a boot failure keeps `dev` alive and retries, a kept snapshot
wins, the doc-reload log is stripped; scripted failure-branch tests), D5+D6+D8+X3+R10 (r1181, dotted nested
drift, `L3048`/`L3049` report entries, `engine.restore` and a game scene switch push `scene.changed`, the
drawer asks before `engine_restore`, `set_recording` refuses late with `L3047`; schema hash
`21ce1bc54b32e9ad` → `b63f8e5f96316693`, the 13 `.lrp` re-recorded), D1 boot mode (r1182, host
`--restore FILE`, `dev` relaunches through it so a renamed component reports `L3015` instead of failing the
launch), D4 (r1183, a reference-game system reorder and a hashed pending-request component; three vigil snapshot → restore → N ticks `WorldHash` tests; `run.lrp` re-recorded),
D2+D7+D8 (r1184, timeouts on every `dev` call, a wedged child restarts fresh with a state-lost status,
`L3003` named as version skew, grouped migration lines, a source fingerprint stops one save reloading twice,
the host watches before it announces). User decisions: the restore boot mode; P1 snapshots the list (the
blast quirk kept and documented); D4 reorders the schedule; R10 refuses. A whole-group review found 1 Medium
(a kept snapshot was lost across a `dev` restart; a fresh `dev` now resumes from it, user decision, and the
file exists only while kept) and 2 Low (restore + record not refused on the embed path; no handshake
timeout in `ludion-client`), all fixed in r1185 with the `L3016` grouping; vigil's restore boot measured
well under a second, so the 20 s listen timeout stays. Owed: the restore and reload-status live smoke
(`Plan/owed.md`). Follow-ups (Group 18): `--window` gives up ~30 s after a failed boot; the `dev`
integration tests use fixed scratch names and the failures test takes 70–165 s under load; the source
fingerprint misses a same-size edit under coarse mtimes; two `dev` sessions share one kept file; a
reference-game carried state needs a hashed-or-not check; `L3048` is latent while every build
reports 0.0.0; the editor's save path after a game scene switch (the event carries no path);
`PrefabLibrary::relog_all` could simplify; two new reference-game replay segments would
retire the new `UNEXERCISED_UNTIL_SEGMENTS` entries.

### Group 7 — Replay pipeline and `WorldHash`
Findings: replay-pipeline R1–R9, R11; ecs-par E2, E4, E5.
Goal: replay tools fail on a panic, a frame reset or a shifted schedule instead of reporting
success; keyframe checks are never vacuous; `WorldHash` distinguishes NaN, ±∞ and `None`.
Watch for: E4 moves every trace, so the corpus is re-recorded at the end of the group.
Status: **closed** — Lore r1187–r1193. All 13 findings fixed: R1 (r1187, a `system_panic` record, kind 7,
marks the recorded panic tick; any other panic fails replay with `L4007`, a marked tick that no longer
panics is `L3050`), E2+E5 (r1188, glam's backend-dependent Quat/Vec4 methods banned in the sim tier,
extended in r1193), R2+R3+R5+R6+R9 (r1189, frame-windowed tools refuse a world reset after the first
tick with `L3021` — user decision, no drive-position keying; profiles key systems by name), R4+R7+R8+R11
(r1190, a closing keyframe on `stop_recording`, the recorded scene-text map always installed), E4 (r1191,
`ludion.worldhash.v2` — NaN, ±inf and `None` hash distinctly, NaN sign and payload deliberately not, plus
a once-per-component `L4024` warning — user decision; `Engine::world_hash` takes `&mut self`; gallery
traces re-blessed), corpus re-record (r1192; `stress.lrp`'s committed shape stays keyframe-less for its
size gate, so its thread-identity check is trace-only). Review fixes (r1193): bisect at a recorded panic
tick, no keyframe after a lockstep panic marker, committed run recordings must carry a keyframe.
Owed: the M4 reproduces the five re-blessed gallery traces (`Plan/owed.md`). Follow-ups for Group 18:
the strict tier (ludion-engine, ludion-core) carries no glam Quat/Vec4 ban; `cargo doc` of vigil has a
private intra-doc link; the fuzz corpus's breakout
`run.lrp` is an older copy, not re-recorded.

### Group 8 — Transform hierarchy and physics
Findings: hierarchy-prefabs H1, M1, L1–L6; physics P2–P11; spatial-nav N3, N7.
Goal: keep-world reparent agrees with the editor gizmo's inversion; despawn stops rebuilding the
child map; a rigid body keeps its momentum across a rebuild; static bodies follow their
transform; physics or 2D collision that is configured but cannot run says so.
Status: **closed** — Lore r1195–r1199. All 19 findings fixed, none deferred or "not reproduced": H1+L2+L3+L4
(hierarchy half)+L5+L6 (r1195, `Trs::localize` with an exact translation, used by keep-world reparent and
skinned parts, which now localize against the parent's chain pose; reparent refuses a move that pushes a
descendant past the depth cap), P2+P4–P10 (r1196, a rebuild keeps a Velocity-less body's momentum; a
static body follows its `Transform` by a bit compare against the solver pose, no snapshot change — user
decision; heightfield on rigid is `L1211`; glam/glamx in the feature gate; P10 docs only — user decision),
M1+L1 (r1197, the cascade builds the child map only when a doomed id is a parent; dead prefab roots pruned
when the maps double), P3+P11+N3+N7+L4 (r1198, `L1261` at check-scene and runtime, `L1262` runtime,
`Collider::is_solver_owned`, strict contact in `move_and_slide` only — user decision). Review fixes (r1199):
the same lossy inverse in prefab collapse's placement recovery and part overrides (`Trs::parent_for`), the
unrun-collider scan only on a Collider row-count change, an allocation-free latch probe, a phase-free reparent
depth guard. No WorldHash trace, replay or golden moved. Follow-ups for Group 18: M1 has no before/after
reference-game profile (after: 0.73 ms median frame); the unrun-collider rescan misses a same-tick swap that keeps
the row count until the next change; P10's `teleport` waits on a game asking.

### Group 9 — Maps and navigation
Findings: maps T1, T4–T9, T11–T13; spatial-nav N1, N2, N4–N6, N9, N10.
Goal: `map-check` sees prefab instances and `Parent`, an empty grid keeps its markers, `L1028`
understands fragment-composed maps, and a body against a wall still gets a flow direction.
Status: **closed** — Lore r1203–r1205. All 17 findings fixed, none deferred or "not reproduced". r1203: N2
(a blocked cell beside reached ones flows to its lowest reached neighbour), N4 (`nav::nav_clearance`, the
largest flow-reading body radius, user decision), N5, N6, N9 (arrival grid
build 35.6 → 11.6 ms on a large scene: `spatial::overlaps_any` and a rasterised `build_grid` checked against
the per-cell scan), N10; the replay gates found blind to code drift now check committed keyframes in the
default lane, the corpus was re-recorded (a new replay segment).
r1204: N1 (map-check expands prefabs through `expand_scene`, keeps `Parent`, samples a sub-cell lattice),
T4 (`L1028` by reachability from the anchor; void beyond the walls is "outside"), T1 (`L1023` for markers
on an empty grid, `bounds:` for marker-only maps — user decision), T5 (merged primitives need a cell-sized
`aabb` — user decision), T6, T7 (map-level `physics:`, fragment `physics:`/`seed:` refused — user
decision), T8, T9 (`L1039` drift), T11–T13; found on the way and fixed: fragment entity references
(prefab overrides included) are rebased on composition, and generate-map/map-check run scene validation
and report faults at their source file. r1205: a CRLF fix and this Status. Follow-ups for Group 18: `stress.lrp` has no keyframes; breakout's and sokoban's
replay tests share the trace-only blind check; a cheaper per-cell nav test if 11.6 ms on arrival ever
matters.

### Group 10 — Asset import, sync and voxels
Findings: import-sync I1–I9, I11; voxels Y1, Y5, Y7.
Goal: the first sync of a root is concurrency- and crash-safe; case-only name collisions are
refused; `render.bounds` sizes voxel content from its real grid, the same rule the GI voxelizer
uses.
Status: **closed** — Lore r1208–r1211. All 13 findings fixed, none deferred or "not reproduced".
r1208: I1 (ownership decided under the sync lock, a provisional record written before the first swap, so
a second sync waits and a killed first sync recovers), I4 (the record write was already atomic; the help
names the path), I5, I7 (`--json` always prints, new `errors` key), I8, I9 (a symlink or junction under a
source directory is refused, `L1245` — user decision); the sync tests use per-process scratch dirs.
r1209: I3 (mesh and clip file stems dedup case-insensitively, keeping the authored spelling; part and
joint names unchanged; case-only sync stems refused), I2 (a `to` inside or holding a `from` directory is
`L0023`; the source hash skips sync debris), I6 (`export` copies under `sync::lock_imports`, not across
the cargo build), I11 (a cross-drive `generated_from` is refused, never absolute — user decision).
r1210: Y1 (`render.bounds` boxes voxel models, assemblies and parts by their grid through the voxelizer's
`grid_box`, and meshes, sculpts and characters by their vertex extent, loading through the `AssetCache` on
a miss; the unit box only when a document fails to load), Y5 (lineup and set/sweep staging re-root `..`
dependency paths under the slot — user decision; an escape is `L1138`/`L1157`), Y7. A whole-group review
led to r1211: mixed-depth `..` staging anchors, case-insensitive `to`-in-`from`, drive-letter compare.
Follow-ups (Group 18): ludion-gen `run.rs` copies `--image` inputs by bare file name and ludion-gen
`recipe::relative_to` falls back to an absolute path (both fixed by Group 16, r1271); the cli `relative_to` compares non-drive components
case-sensitively; no dedicated test for sculpt/character vertex-extent bounds; `placement.rs` could reuse
the new extents for voxel/assembly/sculpt; the I1 kill test and the export-lock test can pass vacuously
under timing (their deterministic companions hold); a reference-game finding (I10) stays with Group 18.

### Group 11 — Lighting and rendering
Findings: lighting O1–O8; render-graph G3–G9; patterns-sprites B5; particles K3, K4.
Goal: correct glTF tangent handedness (with a fixture whose normal map tilts in Y), a
world-space shadow bias, validated light parameters, no inf/NaN in the HDR chain, a total sprite
sort, and a windowed game that reports a lost device.
Watch for: O1 and O2 change how the reference game looks; re-bless on the reference box after a look.
User decisions: O3 (make `Environment.ambient` light meshes, or change the doc) and O6 (make lit
sprites take the sun and IBL, or change the doc).
Status: **closed** — Lore r1216–r1225. All 20 findings fixed, none deferred or "not reproduced".
r1216: O1 (glTF bitangent sign; a committed Blender 5.2 export is the oracle; the import-textured
fixture tilts in Y). r1217: O3 (user: ambient lights meshes, added inside the IBL branch; its default
under an `Environment` is zero, since the IBL is the ambient — Tech Lead call, no golden churn), O6
(user: lit sprites take an unshadowed sun, no IBL), O7, O8. r1218: O2 (`shadow_bias` in world units,
default 0.05, plus a hardware slope bias of 1.5, both picked from sweeps; the audit's "small props
cast no shadow" holds only for thin casters). Both defaults are hashed: vigil's corpus re-recorded
twice. r1219: O4, K3 (light ranges, L1123 broadened, new L1263 for negative or out-of-range colours on
every HDR colour). r1220: O5 (`HDR_CEILING` 60000 at the mesh store, NaN-safe threshold and bloom;
the 3090's fp16 store saturates rather than overflowing); the image-stats tests got their own fixtures.
r1221: G4 (a lost device or dead surface ends a windowed game), G5 (latest-wins readback), G9.
r1222: G8 (one `ludion_render_graph::blit`; ludion-window now depends on ludion-render-graph).
r1223: G3, B5, K4 (`total_cmp` sorts with an index tie-break, siblings in profile, perf, the reference game's HUD),
G6, G7 (textures shared by `Arc`). A whole-group review led to r1224–r1225: the sun light view from
its direction (symmetric ranges no longer NaN), `MeshGrid` colour checks, particle alpha above 1 legal
again, `Camera` near/far (new L1264), a bloom test that fails per missing scrub, the render fault
handled by the headless host and the iOS API, the sRGB fallback format, viewer device faults fatal.
Owed: M4 re-blesses, Metal's fp16 overflow, the blit live smoke (`Plan/owed.md`). Follow-ups (Group
18): lit sprites under an `Environment` have no ambient floor (no IBL term for sprites); composite,
DOF, halation and shafts read raw scene texels; a non-finite sprite placement has no diagnostic;
`engine-architecture.md` risk row 2 says "render thread"; a headless `ludion dev` child that exits on
a render fault is relaunched only at the next reload.

### Group 12 — Animation and particles
Findings: animation A1–A9; particles K1, K2, K5.
Goal: clip time in integer ticks or a carried phase, so `t: 0` events fire on every wrap and on
the frame a clip starts, and a speed change never rescales elapsed time; `burst` respects the
emitter cap; the docs, the example and the wire agree on which frame stamps `started`.
User decision: K5 (move purely cosmetic emitters off hashed sim entities).
Status: **closed** — Lore r1227–r1236. 11 of 12 findings fixed; K5 **parked for the user** (the
product decision above), none "not reproduced". r1227: K1 (burst gets only the room the continuous
window leaves under the cap), K2's doc and example half (`particle_burst`, `ground_marker`, breakout's
burst and reaper on gameplay time; `docs/game-clock.md` says a sim-frame stamp is late by every frame
held this session). r1228: A8 (a helper node's transform is folded into the joint rests below it; a
shearing fold is `L1202`), A9 (reproduced: 0.035 units against a reported 0; cubic intervals take 4–64
steps, `L1206` past the cap). r1229: A1 (the window test decides the loop cycle; 5 ms sweep), A2
(decided: a snapshotted, unhashed `AnimationWalk` record — `t: 0` fires on the start tick for a starter
registered before `animation_events`, on the next tick for one after, never lost or doubled), A3
(`ClipAnchor` + `set_speed`; pose, flipbook and events read one clock), A6. r1230: K2's wire half
(`game_frame` on every success envelope, schema hash `fed07a7f8cbc41b6`; the editor's five stamp sites).
r1231: A5 (`L1142` for a non-step segment turning ≥180° on an axis; the importer's reduce never merges
one), A7 (`ClipSampler`, binary search, scratch buffer; bit-identical). r1232: A4 (sign-corrected
cofactor normals; `character-cycle-driven`, `ragdoll-skinned` re-blessed after a look). r1233: corpus
re-recorded (header hash only). A whole-group review led to r1234–r1236: emitted half-turn segments
split at the source midpoint (an exact 180° flip was refused since r1231), a zero-scale helper
collapses instead of NaN, `serde_json/float_roundtrip` workspace-wide (an `f64` anchor crossing JSON
text moved `WorldHash`), the walk caps a tick at the latest 64 cycles (`MAX_CYCLES_PER_WALK`) and keeps
the furthest time reached, `AssetCache::reload` drops every dependent first, specific `L1202` help.
Owed: M4 twins of the two goldens, the gameplay-frame live smoke (`Plan/owed.md`). Follow-ups (Group
18): multi-axis windings pass A5's per-axis rule ((179,179,179) plays as 1.74°); a one-axis zero scale
collapses to a point, not a plane; a shearing glTF `matrix` node loses its shear silently
(`ludion-import` `lib.rs`, `skin.rs`); channels on folded joints and reframed roots are dropped, not
re-expressed; `skin.rs` `local()` uses glam's `DQuat::normalize`; the assembly part path is not on
`ClipSampler`; culled skinned instances are still sampled (no culling reaches extract); a caught panic
inside the event walk would leave its resources removed; the editor clip scrub assumes every stepped
tick advances the gameplay frame. For Group 13: `play_patterns` loops may share A1's float-cycle class
(B1), flipbooks already read `clip_elapsed` (B4), and `AssetCache::reload`'s two-pass fix is minimal (B2).

### Group 13 — Sprites, patterns, fonts and screen UI
Findings: patterns-sprites B1–B4, B6–B11; fonts-ui F1–F6.
Goal: pattern entries past `loop` fire or are refused; a broken reload is refused, not logged;
flip mirrors the pivot; flipbook frames from integer ticks; one text layout that handles `\n`,
`\r` and `\t` in every path, and no hand-kept 8×8 glyph tables.
User decision: B9 (should bullets orient to their heading, and is that opt-in).
Status: **closed** — Lore r1239–r1245. 15 of 16 findings fixed; B9 **parked for the user** (the
product decision above; `docs/patterns.md` now states that bullet quads keep the plane's axes, so use
round art), none "not reproduced". r1239: F3 (one `ludion_font::stamp_label` over the embedded face for
every CPU sheet label; the face grew a second atlas row of the 15 characters the sheets used, pinned
against the retired table; no golden moved). r1240: B1 (entries, repeat trains and played leaves at or
past `loop` are `L1149`; `elapsed_frames` is u64-exact, so not A1's class), B6 and a played pattern's
own `loop` (new warning `L1265`), B7 (+ the angle-step sibling), B2 (a pattern reload whose bullet
atlas or frame fails is refused before the upsert; extract draws a kind that breaks under a live doc
untextured, reported once per kind), B11. r1241: B8 (O(1) walk, seed ordinals bit-identical), B10
(age versus life in ticks, segment durations at the real rate). r1242: F1 (UiList folds line breaks in
one helper shared by measure, `list_hit` and draw; a game-side duplicate deleted), F2 (CRLF and a lone
CR break lines, a tab breaks and advances like a space; `L1256` follows), F5. r1243: B3 (flip mirrors
the pivot), B4 (one speed-weighted clip-tick clock; `ClipAnchor` stores `ticks`; the flipbook frame is
`floor(ticks × fps / rate)`, exact across `set_speed`). r1244: F6 (`resolve_ui_text` takes `&UiText`
and shares `ui_text_draw`'s layout), F4 (one layout per list line, a linear `wrap_paragraph`, latch-first
`report_font_faults`; fewer per-frame allocations, not none). A whole-group review led to r1245: any
validator error refuses a pattern reload, and the atlas half refuses only a reference the edit changes
and breaks; the walk fires an `every: 0` train once; `L1265` and leaf-loop wording; an independent
oracle back in the UiText rect tests. No `WorldHash`, schema hash, replay or golden moved; no corpus
re-record. Owed: the M4 run of the embedded-face goldens (`Plan/owed.md`). Follow-ups (Group 18):
a reload that pushes *another* pattern doc into an error stays reported; other doc kinds still apply a
reload the validator rejects; `particles.rs` `floor(elapsed × rate)` emission index and `fade_weight`'s
end-of-fade tick share B4's float class (`model_sheet` mirrors the latter); ludion-gen labels overflow
cells under ~150 px (fixed by Group 16, r1270); `ludion-client` `handshake_timeout` and `ludion-cli`
`dev_keeps_the_snapshot_through_failed_boots` are slow or flaky under a full lane.

### Group 14 — Audio
Findings: audio U1–U13.
Goal: a truly gapless loop seam; the sink's message routing under test without a device; broken
sound references reported on headless runs; a voice cap with stealing; linear resampling and
de-clicked stops.
User decision: U4's voice cap policy (per-sound limit, steal oldest or quietest).
Status: **closed** — Lore r1247–r1253. All 13 findings fixed; U4's policy settled 2026-10-10 (4 per
sound, 64 total, oldest one-shot stolen; a game overrides both in `ludion.toml`'s `[audio]`), none
"not reproduced". r1247: U2 (`Router`/`serve` and
`Mixer::render` split out of the run loop and the callback; device-free message → command → samples
tests), U13. r1248: U3 (the silent sink checks each distinct sound and track once per file version,
`L1101`/`L1104`/`L5003`; public `ludion_audio::check_message`; testkit `SoundTap` in the reference game's replay and
other tests). r1249: U7 (one integer-countdown
envelope, `RAMP_MS` 10: stop, steal and blanket-stop fades, gain slews), U4 mechanism (both steal
strategies; loops count but are never stolen; Quietest refuses a quieter newcomer; containers reserved,
block mixing). r1250: U1 (output clamped to the last page's granule; seam test against the granule
read from the file bytes), U8, U9, U10 (the reopen wiring has no end-to-end test: no second encoded
fixture), U12. r1251: U5 (SFX through music's `Converter` at decode, stereo kept, periodic tail), U6
(an emitter resyncs on a gain move past 1 % or a sound/bus change; stamps re-read at most every 500 ms;
loop ids interned to `LoopKey`). r1252: U11 (the default output reopens with 250 ms → 5 s backoff, the
declared loops replay, music resumes; command queue bounded at 1024; the suspected backlog reproduced).
A whole-group review led to r1253: loops past the voice total wait in the Router and start when one
stops; superseded buffers are freed on the sink thread; the music gain slews, sink-restarted loops fade
in, a bus change crossfades; before the first callback one-shots stop at half the queue; no first
callback within 5 s is a loss; the backoff resets after 5 s of playing. No `WorldHash`, schema hash,
replay or golden moved; no corpus re-record. Owed: the policy and an audio listening sitting
(`Plan/owed.md`). Follow-ups (Group 18): check-scene validating `AudioEmitter.sound` (engine
`scene_check`, cli, facade, prefab bodies); `lose()` drops the cpal stream on the sink thread and could
block on a stuck driver; the granule clamp assumes a stream starting at granule 0; a default-device
change while the old device plays is not followed, and a device absent at session start is not
retried; a static emitter loop reloads a rewritten WAV only at its next sync; hello-paddle and
breakout could adopt `SoundTap`;
`Plan/audio-ambience.md` still says "no limiter" twice.

### Group 15 — Editor host
Findings: editor-parity X1, X2, X4, X7, X8, X11, X12, X14.
Goal: a pinned dev port with `strictPort` and a CSP; the stage slot guarded against double start
and orphaned exit; readouts from `--json`, not prose; undo remaps every entity ref.
Watch for: the live smoke rule in `editor/CLAUDE.md`.
Status: **closed** — Lore r1256–r1263. All 8 findings fixed; none "not reproduced", none deferred.
r1256: X1 (Vite on 1420 with `strictPort`, `devUrl` in step; the packaged CSP in `tauri.conf.json`, the
dev CSP in Vite's response headers because Tauri's `devCsp` never reaches a `devUrl` page; the style
nonce disabled for xterm/CodeMirror; a config test). r1257: X7 (per-invocation project/recipe/run scope,
per-verb busy counts, `inflight.ts`), X11 (undo reads each component's `x-entity-ref` fields from
`engine.describe` through `entityRefs.ts`, a port of `entity_ref.rs`'s walk; the post-group repair covers
every ref-bearing component; no wire change), X14's frontend comments. r1258: X2 (a start reserves
Starting and a generation under one lock before the slow steps, a concurrent start returns it, a stop
during the pre-steps cancels it; a slot reuses a session only for the same project or content root;
`stageMount.ts` re-checks cancellation), X4's exit half (`stop()` waits for the reap, `stop_all` runs
concurrently, `stop_and_wait` removed), X14's host prose. r1259: X4's kill reach (the host runs `cargo
build --message-format` and spawns the built executable; no platform code). r1260: X12 (`check-scene
--json` envelope for voxel and assembly; the readout parses committed samples the CLI tests also check;
the readout had been blank since the CLI line gained its `meshed` segment). r1261: X8 (a per-port lock
file in the temp dir and an engine-PID record; only a dead editor's orphan is reclaimed). r1262: X12's
class (the envelope for every kind). A whole-group review led to r1263: the port lock is slot-owned, so a
cancelled start no longer blocks the next; the waiter's build → executable → record path runs under
default-lane tests; connect-src admits any `ws:`/`wss:` host; stale siblings; the README says one dev
editor per machine. Decisions (the coordinator, for the user): port 1420; productName/identifier
`ludion-viewer` kept (a rename resets WebView2 localStorage; offered to the user as an option); stage
ports 9412/9413 stay fixed; X8 wire-free (a wire change only when additive and no wire-free route exists);
any WebSocket host may be attached. No `WorldHash`, schema hash, replay or golden moved; no corpus
re-record. Owed: the editor host live smoke and the M4 kill-reach run (`Plan/owed.md`). Follow-ups
(Group 18): no drift gate ties `entityRefs.ts` to `entity_ref.rs`; editor reads of CLI prose remain
(`stage.rs` `parse_listening_url`, `anim.ts` `tickLabel` against `render --strip`); `sync --json` prints
diagnostics as rendered strings; `--bounds --json` lacks the envelope; X8's documented limit (a stale
record plus an unrelated engine on the port is reclaimed); a running stage engine holds
`target/debug/ludion.exe`, so ludion-cli relinks fail on Windows while it runs (as under `cargo run`).

### Group 16 — ComfyUI generation
Findings: comfy-gen C1–C14.
Goal: accept records the run's own prompt and inputs; `/ws` opens before submit; one failed job
no longer strands the run; uploads are content-addressed; every verb keeps the `--json` contract
on failure.
Watch for: GPU generation never runs beside another instance's golden tests.
Status: **closed** — Lore r1265–r1275. All 14 findings fixed (C2 and C4, both suspected, reproduced on
the mock first); none "not reproduced", none deferred. Also fixed: Group 2's two ludion-gen follow-ups
and Group 13's sheet-label overflow. r1265: C1 (`run.json` snapshots `prompt` and `inputs`; accept
builds the origin from `run.json` alone; optional `recipe_hash`; `date` is the run's date; an old
report re-reads its recipe only when the recipe and workflow hashes match, else `L1172`), C8 (prompt =
encoders reaching a `positive` input, by graph edges), C7 (checkpoints = any `*_name` input with a model
extension). r1266–r1267: the idol and creature-head meshes re-accepted from their kept runs into
scratch, byte-identical geometry, so only their origin blocks changed (+BiRefNet, +`recipe_hash`; the
creature head loses an input its run never used, a live C1 instance); an old report with no `images`
rebuilds them from the run's legacy `inputs/` copies; BiRefNet (MIT, model card checked) credited in
`THIRD-PARTY-NOTICES.md`. r1268: C2 (subscribe to `/ws` before the first submit; a `/history` sweep at
entry and every 2 s; the mock routes frames per client id and never replays). r1269: C3 + C12 (a
per-job `failed` list, reason `error`/`lost`, status `ok`/`partial`/`failed`; lost = in neither queue nor
history; a finalized run is never re-queried; no queue delete). r1270: C6 (`with_seed` reports
written/pinned/unknown; `Origin.seed` optional; new warning `L1266` for identical candidates), C9 (temp
and PreviewImage outputs skipped, `batch_index`; a preview-only graph is `L1171`), sheet labels shrink,
then fall back to `#index`, then drop. r1271: C4 (uploads as `ludion-<hash>.<ext>`, no overwrite;
`inputs/` copies use the same name), C13 (exclusive `create_dir`, `-2`, `-3` suffixes), C11 (`https://`
refused, body-read errors `L1168`, a 600 s `/view` timeout), C10, C14; a workflow outside the project
root is `L1162`. r1272: C5's CLI half (every gen verb prints one `{ok, …, diagnostics}` object, also on
failure, with `dir` and the report whenever `run.json` exists; one envelope helper shared with
check-scene; the mock moved to `tests/common/`; shapes checked against samples in the editor's
`__fixtures__/`). r1273: C5's editor half (the view keeps a failed, timed-out or partial run, offers
Status and Fetch, a timeout field, failed/lost job rows, an unknown seed as `?`). A whole-group review
led to r1274–r1275: a `./`-spelled recipe path no longer gets a false `L1162` (a regression from
r1271); status of a finalized run is offline; a job with no output is reason `empty`; a partial submit
writes `run.json`; `--timeout` is clamped to 7 days; key-order sample checks; numeric run-folder order.
Decisions (the coordinator, for the user): a `partial` run exits non-zero, as `ludion sync` does;
`https://` refused, no TLS dependency; the committed meshes were corrected only by a byte-identical
re-accept, never by hand. Tech Lead calls: the ones named above, plus one envelope for every gen verb
and Fetch kept all-or-nothing (documented). No `WorldHash`, schema hash, replay or golden moved; no
corpus re-record. Owed: the live ComfyUI check of C2 and the Generation view failure-path smoke
(`Plan/owed.md`). Follow-ups (Group 18): check.mjs's doctest step lacks `--no-fail-fast`; the
Generation view shows reason `empty` as a plain error; `voxel/ModelPanel.tsx` keeps its own diagnostic
rows (`generation/DiagnosticList.tsx` exists); wrapped string literals with space runs outside
ludion-gen and `gen_cmd.rs` were not swept; pid-suffixed test scratch dirs accumulate in `%TEMP%`.

### Group 17 — reference-game gameplay
Findings: vigil-gameplay V1–V7 (reference-game gameplay findings, tracked privately); hierarchy-prefabs L7;
spatial-nav N8; import-sync I10.
Depends on: groups 6, 8 and 9.
Watch for: the game's hashed-state and replay traps; the corpus is re-recorded last.
Status: **closed** — Lore r1277–r1284. Fixed V2–V7, L7, N8, I10; none "not reproduced"; **V1 parked for the
user** (`Plan/owed.md`). The gameplay fixes are recorded privately. r1278: a map-seal leak found and fixed;
`docs/map-check.md` states the lattice's limit. r1280: L7, N8, I10 (kit pieces compose their parent chain).
r1284: `run.lrp` and the segments re-recorded; `stress.lrp` and every golden unchanged. Owed: V1, the M4
corpus check. Follow-ups (Group 18): a sound leak lint in map-check; the map builder's auto plugs catch only
narrow gaps (wider ones rely on a hand-kept list, now guarded by the seal test).

### Group 18 — Close-out
A Reviewer's whole-remediation pass; a sweep of the master index's cross-cutting themes for
sites the groups did not reach; the master index updated with each finding's outcome; the
`CLAUDE.md` current-state section refreshed if the wire method count or schema hash moved.
Status: **closed** — Lore r1286–r1312. Every open follow-up of Groups 1–17 was fixed or retired with a
trigger (the Close-out ledger below); the theme sweep's confirmed sites were fixed; a whole-group review
found two High defects in Group 18's own work, both fixed (r1306; r1307 and r1310), and its Lows were
fixed in r1304–r1306 and r1311. r1286: the `--full` doc step is
green again (a public doc linked the private `extract_through`), plus stale doc lines and the fuzz
corpus's breakout replay. r1287: `commit.mjs --mirror-only` and `mirror-gap.mjs` — the git mirror lags
Lore by ~280 public paths (240 revisions never mirrored; a clone does not build) and has tracked Lore-only
private paths since c06a2d18; the catch-up commit was denied to agents and is **owed to the user**
(`Plan/owed.md`, "Waiting on the user's word"), and the fresh-root cutover must not carry that history;
r1305 keeps tracked private files out of every normal mirror commit meanwhile. r1288, r1291: diagnostics
keep code, severity and payload in transit (`sync --json` and `--bounds --json` envelopes, L1038 for
gen write failures, L3051 for an unparseable server message, the print gate over the Tauri host, L2109
for a non-finite placement, L1202 for a shearing glTF matrix); G4's L1167 did not reproduce. r1289:
particles release and retire on exact ticks, multi-axis rotation keys obey a combined winding rule
(L1142), glam's Quat/Vec4 banned in the strict tier; the fade weight did not reproduce. r1290: every
writer that replaces a user-visible file goes through temp-and-rename (the PNG writer included, closing
the owed 1224 item), voxel-drift enumerates its generators; r1305 adds the `atomic-writes` gate. r1292,
r1309: tests that waited on the clock prove the state they reached, loud skips (schema-drift without
`node_modules`, the canary without its token list), a per-checkout test TEMP, and the `ludion dev` tests'
two flake causes reproduced and fixed (a shared scratch dir across concurrent runs; a crash trigger
racing its reply). r1293, r1302: wire `scene.save` refuses a `generated_from` scene with L1267 in the core
(the editor keeps its echo), a BOM before `%YAML` parses, check-scene decodes `AudioEmitter.sound` (at
check time only, never at spawn), one engine check list serves the CLI, the facade and `scene.check`, and
a live reload of any doc kind is refused when it would install a validator Error. r1294, r1304: docs that
promised what the code does not (patterns played by CLI hosts, names outside the WorldHash, a missing
`visible` flag, flow YAML the strict reader refuses, five dev-docs). r1295: the master index records every
finding's outcome. r1296, r1299, r1301, r1303, r1307, r1310: map-check's seal check is sound (L1040, a
warning; map-drift refuses a committed map that raises it) and treats no gate trigger as a wall (a hold
gate or the rearm lock lets a player pass one unfired); collider-only caps behind the reference game's
triggers seal all 18 maps; replay segments adjusted; the corpus re-recorded twice (WorldHash moved; no golden, no
schema hash). r1297, r1300, r1306: success reported on a wrong result (check-docs, a failed mirror commit,
a recording failure on a live host, `ludion test --filter` matching nothing as L2209, a PNG side over 8192,
`node --test` globs matching nothing), and the self-comparing wire hash test replaced. r1298: ten tests
that compared the system with itself check an independent value, each proven against its mutant.
Decisions (the coordinator, for the user): the mirror tooling and catch-up as above; L1267 in the core;
the reference game's caps (Tech Lead extension after the review: caps behind every trigger).
Tech Lead calls: L1040 stays a warning (sound, not exact) with map-drift as the gate; `AudioEmitter.sound`
is a check-time error; wire `scene.check` returns warnings with `valid: true`; `--check-docs` fails only on
kinds nothing registers. Owed: the mirror catch-up (user), the M4 run of the re-recorded corpus. For the
user to see: two reference-game design notes, tracked privately; `%TEMP%` holds ~85k stale `ludion-*` entries from past runs.

#### Close-out ledger — follow-ups retired with a trigger
Every follow-up the groups left open was fixed in Group 18 (Status above) or is retired here. Each one
is reopened when its trigger fires, not before. Owed sittings, M4 checks and live smokes stay in
`Plan/owed.md`; the parked user decisions (K5, B9, U4's policy values, V1, the `ludion-viewer` rename) are
untouched.
- **Scene text (G3).** `scene.save` drops YAML comments; a comment-keeping writer waits until a comment
  someone needed is lost to an editor or wire save. The schema checker's flattened `oneOf` enums and
  `additionalProperties` inside `allOf`: when a component schema first uses either. `engine.restore`
  snapshots are not schema-checked: when a snapshot from another build or a hand edit can reach restore.
  The fuzz oracle's f32-on-f64 rule is not mutation-proven: at the next change to the canonical float
  writer. the respell gate (~33 s): when the test lane breaches its budget with it on the critical
  path.
- **Diagnostics (G4, G15, Group 18).** Data-doc paths as `<file>#/<pointer>`: when a tool needs to jump from
  a diagnostic to a data-doc field. `LUDION_*` knobs resolved up front in check-scene, sync, generate-*,
  voxel, sculpt and mesh: when a bad value first surfaces mid-run. `mesh pick`/`apply` and `image-stats`
  `--json` envelopes: when a consumer first parses that verb's JSON. The editor host's `sync_project` and
  stage URL parsing read CLI prose, and the TS client's parse errors carry no code: when a CLI prose change
  breaks the editor, or the editor's sync UI needs a structured failure. `render.bounds` reports non-finite
  placements as-is: when a consumer receives a non-finite extent. The offline extract (`ludion render`)
  drops degraded diagnostics: when someone has to find a render fallback by eye.
- **Wire and hosts (G5, G6).** L4022's stuck path cannot flush an armed recording: at the first stuck-host
  L4022 during a recording. The embed listener's boot join is bounded only by server teardown: when an
  embed host hangs at boot. A host that renders slower than its tick rate times a capture out as L3023:
  at the first such L3023 on a merely slow host. The Performance view could disable Capture while
  paused, prefab actions read the whole world, the gizmo reads its pose in two queries, the editor's
  save path after a game scene switch, `PrefabLibrary::relog_all`: at the next edit to each. `--window`
  gives up ~30 s after a failed boot: when a dev session loses a fix to it. Two dev sessions share one
  kept snapshot file: when two `ludion dev` sessions first run on one project. L3048 is latent while every
  build reports 0.0.0: at the first versioned build. `ludion dev`'s change fingerprint misses a same-size
  edit under coarse mtimes: when an edit is missed on such a filesystem. The dev failures test's 70–165 s:
  when the test lane breaches its budget with it on the critical path. Two unexercised reference-game replay segments:
  at the next edit to the systems they cover. The editor TS client's hello check has no mismatch test: at the
  next edit to its handshake. The editor's live integration tests need `LUDION_WS_URL`, which no lane
  sets: when a CI lane first starts `ludion serve`. `a_socket_that_never_upgrades_does_not_hold_shutdown`
  can pass on a slow accept loop: at the next edit to that test.
- **Replay and physics (G7, G8, G9).** No before/after reference-game profile for M1: at the next profiling
  pass on the reference box. The unrun-collider rescan misses a same-tick swap that keeps the row count:
  when a game first swaps colliders within one tick. P10's teleport: when a game asks for it. `stress.lrp`
  has no keyframes: at its next re-record. breakout's and sokoban's replay tests share the trace-only blind
  check: at the next edit to either.- **Assets and rendering (G10, G11, Group 18).** `relative_to` compares non-drive components
  case-sensitively: when a path differing only in case is refused or mis-relativized on Windows. No test
  for the sculpt and character vertex-extent bounds, and `placement.rs` could reuse them: at the next edit
  to either. Lit sprites under an `Environment` have no ambient floor: when a lit-sprite scene under an
  environment looks wrong. Composite, DOF, halation and shafts read raw scene texels: when an artifact is
  traced to it. A headless dev child that exits on a render fault relaunches only at the next reload: when
  an agent waits on a dead child. check-scene does not resolve loose Sprite/UiRect/Tilemap texture or
  Mesh `shape` files (they fail at extract): at the next pass added to `scene_engine_diagnostics`, or a
  scene that passes check-scene and fails at extract with L1101.
- **Animation, particles, patterns (G12, G13, Group 18).** One-axis zero scale collapses to a point; channels
  on folded joints and reframed roots are dropped; the importer's `local()` normalizes with glam's
  `DQuat`: when content first hits each (an import that differs across platforms, for the last). The
  assembly part path is not on `ClipSampler`, culled skinned instances are still sampled, the editor's clip
  scrub assumes every tick advances the gameplay frame: at the next edit to each, or a profile showing the
  sampling. A caught panic in the animation event walk leaves its resources removed (as do ludion-physics'
  write and mirror): not a defect under L4007's contract (the world stays as the panic left it and the sim
  pauses; restore, reload and switch reinstall them); reopen when a supported path resumes ticking after
  an L4007 without a rebuild, or a game callback runs inside the walk. breakout's `reap_bursts` compares
  f32 seconds before a hashed despawn: with the K5 decision.
- **Audio (G14).** The reopen wiring has no end-to-end test: when a second encoded fixture exists. `lose()`
  drops the cpal stream on the sink thread: at the first audio hang on device loss. The clamp assumes a
  stream starting at granule 0: at the first Ogg that does not. Default-device changes are not followed
  and an absent device is never retried: at the first report of audio on the wrong device. A static
  emitter loop picks up a rewritten WAV only at its next sync, and hello-paddle and breakout could adopt
  `SoundTap`: at the next edit to either.
- **Editor host and generation (G15, G16).** No drift gate ties `entityRefs.ts` to `entity_ref.rs`: at the
  next edit to `entity_ref.rs`. A stale port record plus an unrelated engine gets reclaimed (documented):
  when it kills an unrelated engine. A running stage engine locks `target/debug/ludion.exe`: when a relink
  failure blocks an agent loop. The Generation view's empty-reason error and ModelPanel's own diagnostic
  rows: at the next edit to each. Wrapped literals with space runs outside ludion-gen: when one shows in a
  diagnostic.
- **Gates and tests (G1, G2, Group 18).** Group 1's unproven suspicions, judged by Group 18's Reviewer: the
  bless guard reads single lines only (latent: when a bless read bypasses `ludion_core::bless`); the
  `needs_gpu_` heuristic, L4019, the worst-window floor and the 60-minute probe ceilings are sound
  (ceilings measured at 17–40× headroom); the equal-luma 16×16 blind spot is by design
  (`dev-docs/ludion-testkit.md`); external games' goldens are outside this repo (when one goes red after an
  engine bump, that is the gate working). Replay recording sinks stay streaming logs (a torn tail fails
  with L3010): at the first L3010 truncated-record on a killed session, write record-aligned. ludion-gen's
  write order is right by construction: when one of its writers first overwrites an existing output. The
  unmarked sprite generators in shmup-stress: when `kind: sprite` gains `generated_from:`. The private CI
  lacks the git-free node gates (diagnostic-prints, lists-in-step, claude-md, physics-features,
  pin-ritual): at the next edit to its provisioning. The `atomic-writes` gate matches writer calls by
  name (a `use std::fs::write` import or `OpenOptions` truncation passes): when a writer reaches a
  user-visible file that way. `commit.mjs --import` does not check contributed paths against the private
  set: when the mirror gets a remote that accepts contributions. Tests that hold by construction (the sculpt
  lattice bounds, the path band, the cluster and GI-bin self-lookups, the profile budget limits, the
  input loop of a scene with no systems): at the next edit to the code each covers.
  `docs/hierarchy.md`'s flow-style `translation:` example: when that file's pending edit is committed.
  A hand-kept stress-scene copy nothing regenerates: when its source docs diverge. `png`
  is not `=`-pinned though atlases are byte-compared: at a lockfile bump that changes encoder bytes. The
  export-lock test cannot read the sync's waiting line in-process (`tests/sync.rs` asserts it through the
  same lock): when export locks through anything else.
- **Wire text (Group 18).** The protocol's doc comments and `protocol.d.ts` still say `scene.check`'s
  diagnostics are empty when valid (warnings now come back with `valid: true`); they feed the schema
  hash: at the next schema-moving change. A reload's "introduced error" compares whole diagnostics, so a
  pre-existing error whose text moves with an edit refuses the reload: at the first such refusal.
