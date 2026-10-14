# Bullet-hell stress scene (shmup feasibility, item 1)

**Question:** can the engine, as shipped, carry a Cave-class bullet hell — ~3,000 live bullets,
hundreds spawned and despawned per frame, one tiny player hitbox — and where does the cost land
(ECS churn, `spatial::rebuild`, sprite extract)? Vigil's proven ceilings are 300 projectiles /
500 enemies; nothing has been measured above them.

**Shape:** a new in-repo game crate `games/shmup-stress` (vigil's membership pattern: `games/*`
glob, relative path deps, no `[workspace]` table), built the way the eventual shmup would be
built — top-down orthographic camera over the ground plane (x, z), bullets as textureless
`Sprite` quads, pattern emitters as plain systems over an authored singleton. It ships three
things:

1. **Scenes** (`scenes/`): `steady.yaml` (emitters hold ~3,000 live bullets with ~25 spawn +
   25 despawn per frame, life 120 frames), `burst.yaml` (bullet-cancel worst case: every 60
   frames every bullet is despawned and 3,000 respawned the same frame), `steady-indexed.yaml`
   (`steady` with a dynamic circle `Collider` on every bullet, so `spatial_rebuild` at 3,000
   entries is measured against the no-index variant — the hypothesis being that bullets must
   NOT live in the spatial index and player-vs-bullet is a plain circle test over a query).
2. **A CI-run determinism + census gate** (`tests/stress.rs`, mirrors vigil
   `budget_horde_is_deterministic_and_does_not_decay`): two runs, identical `WorldHash`
   traces; live-bullet census at the sampled frames equals the closed-form expectation; the
   burst scene's count returns to 3,000 the frame after a cancel. Workspace nextest picks it up
   — no CI list to edit.
3. **Measurement, not a gate:** `budgets/` holds NO calibrated budget yet (this checkout is a
   low-power laptop; numbers from it are untrustworthy). A `#[ignore]`d `tests/profile.rs`
   prints the `ProfileReport` JSON for a scene so any box can dump it; the TeamCity pipeline
   gains a non-gating step that dumps the three reports as build artifacts on the reference
   box. Calibrating `budgets/pinned-rtx3090.toml` from those artifacts is the owed follow-up,
   and the arc's verdict (items 2–5 of the shmup assessment) is read off them.

## Systems (registration order)

`spatial_rebuild` → `emit_bullets` (reads `Arena` singleton + per-`Emitter` pattern: ring of
`count` bullets at angle step, `rate` frames between volleys, deterministic — no RNG needed) →
`move_bullets` (integrate `velocity`, decrement `life`, despawn when expired or outside
`Arena.half_extent`; the churn term) → `cancel_bullets` (only when `Arena.cancel_every > 0`:
despawn all on the cancel frame; emitters re-fill on the next) → `player_hit_test` (iterate
`(&Bullet, &Transform)`, circle test against the `Player` hitbox, count hits into a `HitCount`
resource-like singleton — never the index) → `follow_nothing` (camera static).

Bullet bundle: `Transform`, `Sprite { texture: None, size: [0.3, 0.3], color }`, `Bullet
{ velocity: [f32; 2], life: u32 }`, plus `Collider { circle 0.15, dynamic }` iff
`Arena.bullets_collide`.

## Status

- 2026-08-20: opened; waves dispatched from this file.
- 2026-08-20: **Wave 1 SHIPPED (mechanized).** `games/shmup-stress` (bin + lib, vigil's in-repo
  shape, no RNG — the field is a closed-form function of the frame index): five components
  (`Arena`, `Emitter`, `Bullet`, `Player`, `HitCount`), five systems in the order above, three
  scenes with their census closed form in the header (`steady`/`steady-indexed`:
  `live(N) = 25·min(N+1, 119)` → 2,975 from frame 118; `burst`: 50 spawns/frame, 3,000 wiped on
  every 60th frame). `tests/stress.rs` (determinism + census + hit-test-is-live, 3/3, ~5 s, runs
  in the workspace nextest lane) and the ignored `tests/profile.rs` (ProfileReport dump per scene;
  burst also prints the cancel frames against the median — the mass-despawn cost is in the max, not
  the median). CI: `check.mjs` GAMES + `ci.yml`/private-CI package excludes, and a new non-gating
  shmup-stress profile configuration on the private CI (plain Windows agent, artifacts
  `shmup-stress-profile/<scene>.json`) — **owed: re-provision the private CI and register that
  configuration with its server, then read the first artifacts.** Laptop numbers (Intel, 4 threads,
  untrusted): steady 0.45 ms sim median vs steady-indexed 1.08 ms — `spatial_rebuild` 0.01 → 0.46 ms
  is the whole difference, so the bullets-out-of-the-index hypothesis already reads; burst's
  `emit_bullets` 0.69 ms (3.9× steady: spawn dominates refill). Two costs outside the game's systems
  dwarf all of them at this entity count and are the arc's first real findings to confirm on the
  reference box: `world_hash` 12–21 ms/frame and `extract` 2–4 ms/frame.
- **Reference-box artifacts read 2026-09-07 (RTX 3090, 8 threads, `--profile 200`, release):** steady
  frame total 0.120 ms (move_bullets 0.058, emit_bullets 0.053) / world_hash 4.24 / extract 0.80;
  burst 0.223 (emit_bullets 0.205) / 2.34 / 0.43; steady-indexed 0.264 (spatial_rebuild 0.099 — the
  whole steady→indexed difference, as the laptop read) / 5.01 / 0.83; `scene` pass 0.004 on all three.
  The two costs outside the game's systems still dwarf them on the reference box — `world_hash`
  4–5 ms and `extract` 0.8 ms a frame at 3,000 bullets — so the laptop's two findings are confirmed.
- Next: calibrate `budgets/pinned-rtx3090.toml` from those numbers → verdict on items 2–5
  of the shmup assessment (hash cost, extract cost, churn, index policy).
- **2026-09-12: a fourth scene, and this crate's first calibrated budget** (`Plan/sprite-engine.md`
  Wave 4). A defect first: every bullet was an unrotated, non-billboard XY quad under a `−90°`
  top-down camera, so the three original scenes' sprite pass drew edge-on nothing (0.004 ms) and
  `tunables.rs`'s "seen square-on" comment was false. The archetype is now `billboard: true` and
  unlit (decision 2's no-light-lookup branch, which is what 3,000 quads want); `steady`'s `scene` pass
  moved 0.004 → 0.009 ms, which is the field finally being drawn. Every census assertion held — they
  are closed forms over the frame index and no `.lrp` or hash fixture exists in this crate — but the
  three scenes' `WorldHash` moved, since `Sprite`'s hashed bytes did.
  `scenes/steady-drawn.yaml` then holds `steady`'s exact census while varying only the draw: a
  `BulletArt` singleton puts every bullet on one of 40 atlas regions, 3 of each 25-bullet volley
  additive (357 live), over a 480-cell `Tilemap` a system scrolls, on the upright plane (a tilemap's
  cells are XY quads, so a backdrop needs one), framed at 27 px a bullet at 1080p. Measured here over
  four 200-frame release runs at 8 threads, against the same four runs of `steady`: extract **1.694 vs
  0.852 ms**, `scene` **0.0573 vs 0.0092**, sim frame total 0.129 vs 0.157. Drawing 3,456 quads for
  real costs +0.84 ms of extract and +0.05 ms of GPU; the live per-frame cost is ~1.9 ms against the
  16.6 ms line, so the extract-cost item reads **~9× headroom** even with the field fully drawn.
  `budgets/pinned-rtx3090-sprite-stress.toml` gates it (`frame_total_ms`, `extract_ms`,
  `[passes] scene`), checked in process by the game binary and wired into the private CI's pinned-GPU
  budgets configuration. The arena's own three scenes stay ungated, as this plan intended.
- 2026-08-20: **Wave 1 SHIPPED (mechanized).**  (bin + lib, vigil's in-repo
  shape, no RNG — the field is a closed-form function of the frame index): five components
  (, , , , ), five systems in the order above,
  three scenes with their census closed form in the header (/:
   → 2,975 from frame 118; : 50 spawns/frame, 3,000 wiped on
  every 60th frame).  (determinism + census + hit-test-is-live, 3/3, ~5 s, runs
  in the workspace nextest lane) and the ignored  (ProfileReport dump per
  scene; burst also prints the cancel frames against the median — the mass-despawn cost is in the
  max, not the median). CI:  GAMES + /TeamCity package excludes, and a new
  non-gating TeamCity config  (plain Windows agent, artifacts
  ) — **owed: re-run  and append the id to the
  server's  env, then read the first artifacts.** Laptop numbers (Intel, 4 threads,
  untrusted): steady 0.45 ms sim median vs steady-indexed 1.08 ms —  0.01 → 0.46 ms
  is the whole difference, so the bullets-out-of-the-index hypothesis already reads; burst's
   0.69 ms (3.9× steady: spawn dominates refill). Two costs outside the game's
  systems dwarf all of them at this entity count and are the arc's first real findings to confirm on
  the reference box:  12–21 ms/frame and  2–4 ms/frame.
- Next: reference-box artifacts → calibrate  → verdict on items 2–5
  of the shmup assessment (hash cost, extract cost, churn, index policy).
- **2026-09-12: the instrument migrated to closed-form volleys** — `Plan/projectile-patterns.md`
  Wave 2 owns the migration and its Status carries the detail. `steady` and `burst` are now
  `PatternPlayer` scenes over `scenes/patterns/*.yaml`, holding the same census as before in one
  `Volley` entity per fired ring; `steady-drawn` is the entity baseline the two are measured
  against, and `steady-indexed` moved onto its plane and framing so the `spatial_rebuild`
  subtraction still reads against a twin. This plan's own question is unchanged; the numbers that
  answer it now come in pairs.
