# Audit remediation, 2026-10-10 — plan of record

Fixes the findings of the code audit of 2026-10-10: ten read-only reviewers, one section of the
codebase each, 0 High, 25 Medium, 70 Low (reports kept privately).
It follows the rules of `Plan/audit-remediation-2026-10.md`: verify before fixing, a test that fails
before every Medium fix, fix the class rather than the site, docs move with the code, replays
re-recorded last.

## Decisions

1. **Scope: every finding.** The one exception is the renderer's per-frame GPU buffer allocation
   (section 5 #2), retired until a profile shows its cost; that profile is the trigger.
2. **A reference-game gameplay decision** (restart semantics), tracked privately.
3. **Singleton lookups go through a facade extension trait** (`ludion::prelude::SingleExt`,
   `single`/`single_get`); `ludion-ecs` stays freeze-listed. An O(1) lookup would need a public
   index-to-id on `World`, which is a freeze-list decision.
4. **`input.inject` with a past frame is refused** with L3005 instead of being clamped to now; no
   schema change.
5. **A schedule-comment gate** (`ci/scripts/schedule-comments.mjs`, in `check.mjs` and `ci.yml`).
   It bans schedule talk (the current or a forward wave, "a later phase", milestone tags, "a future
   consumer"), not provenance citations of a plan and its wave: CLAUDE.md forbids schedule, not
   citation. Banning bare `Wave N` citations too (~600 lines) is open for the user's word.
6. **The ECS line budget is measured**: `scripts/ecs-budget.mjs` counts non-blank, non-comment code
   lines before the test module (2517 at close, limit 3000); raising the limit is the user's call.
7. **The SpatialIndex is snapshotted**, not rebuilt on restore: a rebuild files end-of-tick poses,
   not the ones a reader scheduled before `spatial_rebuild` saw, so only carrying it is exact.

## Status — closed 2026-10-11 (Lore r1315–r1344)

Every subtask was planned, implemented and verified separately, then committed through
`scripts/commit.mjs`; the commit messages carry the detail.

| Area | Revisions | What changed |
|---|---|---|
| Reference-game gameplay | r1323 | Findings M1 and M25 and related fixes, tracked privately; its replays re-recorded. |
| Stream lifecycle | r1315, r1316 | Editor: one `openSubscription` helper unsubscribes a late reply, a dial gate drops a superseded handshake, listeners are isolated (M2, M24). Server: an unowned `stream.unsubscribe` answers L3007, a subscribe that timed out is reaped (M3, M4). |
| Editor | r1317, r1318, r1321 | One synced-file session for the pattern, voxel, sculpt and recipe editors, with stat-before-write (M5); a host child registry with tree kill on deadline and exit (M23); typed command builders; gen Cancel. |
| Shared-checkout tooling | r1319, r1320, r1327 | `commit.mjs` refuses foreign staging and unstages only its own (M6); `check-lock` cannot be stolen mid-write (M7); scripts and generators write atomically under the atomic-writes gate. |
| Paths | r1322, r1324–r1326, r1330 | `ludion_core::paths` refuses Windows device names and gains `normalize`, `sibling`, `sibling_refusal`, `unreserved_stem`; mesh-edit `out` (M8), recipe names (M9), asset-cache keys (M11) and every kind's sibling paths go through it. |
| Engine content | r1331, r1338 | Carried entity refs remapped across a transition (M10); editor camera near/far guard (M12); `render.bounds` clips at the near plane (M13); f32 range checks through `ludion_core::number`. |
| Snapshot and replay | r1337, r1341 | Pending injected input and the SpatialIndex survive a restore (M16, M17); seek replays by stream position (M15); past-frame inject refused. |
| CLI | r1329, r1336 | `ludion new` name rule L0035 (M18); stdout keeps to the JSON (M19, M20); knob shadows refuse instead of panicking; one Ctrl-C handler; the atlas pair staged before any rename; PNG re-import refused. |
| Content limits | r1328, r1333 | Ogg below 8000 Hz refused (M22); atlases over the PNG side limit refused at `/inputs`, one shelf packer (M21). |
| Clients, testkit, scene, renderer, spatial | r1332, r1334, r1335, r1340 | Client latest-wins frames and capped events; strict golden manifests; scene Lows; one glyph shader snippet; f32 underflow refused in colliders and joints. |
| Singletons, gates | r1339, r1342 | `SingleExt` across ~200 sites; the schedule gate, the ECS budget and five explain texts. |

**Not reproduced**, closed with evidence: M14 (prefab apply on doubly nested instances: the removal
is written as a part path the prefab still loads; a guard test keeps it), section 2 #7 and #14
(guarded anyway), section 1 #7 (bisect does answer frame 0). M10's failing-first test was argued from
the code, not run against it; the whole-feature review judged it sound.

**Whole-feature review** (after r1342): no High or Medium. Its Lows were fixed in r1343 and r1344
(engine f32 underflow in grid and volume fields, `ludion new` names of the template's own
dependencies, seek on keyframes older than r1337, non-finite spatial entries, an honest
`write_all_atomic` doc — its renames are sequential, so "written together" means staged together —
an ignore rule for atomic temp files, Lore `V` move lines in `commit-lib.mjs`).

**Full lane at r1344:** 5999 of 6002 tests pass; the three reds are the never-blessed Windows goldens
already owed (`ui-fonts.png`, `ui-fonts-1080.png`, breakout `menu.png`).

## Follow-ups

Each waits on its trigger; none is scheduled.

- Residue records carry entity refs verbatim; remapping them needs an identity-by-`Name` decision.
- `gen accept` of a mesh can leave a partial tree on a mid-tree write failure (needs a
  failure-injection seam to test).
- The shader branch of `asset.reload` reads an absolute `.wgsl` path ("From 09" in the previous
  round's ledger).
- The editor's `camera.ts` perspective lacks the engine's depth guard; the `Camera` validator still
  judges near/far raw.
- Rust-built components bypass the validators: `Tilemap::world_to_cell` with a zero `cell_size`, and
  the ragdoll and crossfade inputs, have no runtime guard.
- `GoldenTest` thresholds are validated only for manifest callers; `gen`'s contact sheet has no size
  cap; rows-grid maps have no extent cap.
- A trailing dot or space in a stem is stripped by Windows and not by macOS; case variants of a path
  are separate cache keys.
- The editor host's own atomic writers (`project.rs`, `pty.rs`) use a `name.tmp` spelling the ignore
  rule does not match; they write into game projects, not this repository.
- `stage_stop` does not kill an in-flight project sync; a Cancel before the gen child spawns is a
  no-op.
- A new reference-game replay segment would retire four baseline lines of the corpus-coverage test.
- At the next schema bump: `response.rs` "png in M1" (allowlisted in the schedule gate) and the
  `EntityBounds` "projected corners" wording.

Owed to a human: the live smoke in `Plan/editor-live-smoke.md` ("Stream subscriptions, reconnect,
file sync, gen children"), listed as owed.
