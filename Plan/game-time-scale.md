# Gameplay time scale — a fractional `GameClock`

> **Status:** Waves 1–4 shipped in the working tree (see Status). Owner rulings (a)–(f) below remain owed.
>
> Neon Shroud request 0011 (`engine-requests/0011-gameplay-time-scale.md`, audited at engine r2369).
> Siblings: `docs/game-clock.md` (the hold this extends), `Plan/checkpoints.md` decision 3 (`rebased`),
> `Plan/physics.md` §Time scale (`scale_time`, W3 below). Reserved codes: **L4260–69 / L1600–09**.

## Why

A game that slows time — bullet time, an aim-assist stall, a hit-stop ramp — has only `hold()` today:
full speed or frozen. `GameClock` gains a per-frame fractional scale in `(0, 1]` beside `hold`, so
every `game_dt` integrator slows with no edit and a game that never scales is byte-identical.

## What exists today

- `GameClock { held, holding }` in `ludion-core`; the driver folds it once per tick and `SystemCtx`
  carries `game_frame`/`game_time`/`game_dt`.
- Closed-form evaluators read the integer `game_time`/`game_frame`; `ludion-physics` reads `held()` to
  detect a held frame and steps at full rate regardless.

## Decisions

**1. Q16 "excluded" accumulators, not a game counter.** Gameplay position is
`game_q16(now) = now·65536 − held·65536 − slowed_q16 − pending_deficit`, saturating at zero. `held`
keeps meaning whole held frames (physics and `rebased` read it); a scaled frame adds `65536 − step`
to a separate `slowed_q16` and is **never** normalized into `held`. A marked frame already reads
frozen/slowed (`pending_deficit`), so `extract_at` after tick *f* equals the next tick's folded context.

**2. `(0, 1]`, composed by `min`.** Several `scale` calls in a frame keep the smallest. A hold wins:
`holding` gives step 0 whatever was requested. A held tick leaves `step_q16` at full, so a held-only
clock serializes exactly as before; `advance()` returning `false` carries "held".

**3. `MIN_SCALE` = 1/1024, a clamp.** A positive value below it clamps up and is documented. The request
is `round(s as f64 × 65536)`, an exact IEEE op, clamped to `[64, 65536]`. `game_dt = dt × (step/65536)`
and the quotient is exact in f32, so a full step gives exactly `dt`.

**4. Invalid input is `L4260`, latched then drained.** `ludion-core` cannot report, so `scale()` latches
the first bad value (`rejected`, `#[serde(skip)]`) and an engine boundary drain reports it once per
session, as `lane::report_without_motor` does. The fold and the drain clear it, so a snapshot never
holds it.

**5. Floor `started` stamps (request OQ2).** `game_frame`/`game_time` are the floor of the exact
instant. A stamp is a whole frame, so an effect started mid-slow begins up to one gameplay frame early.
Fractional stamps are out of scope.

**6. `game_instant: GameTime` beside the floor `game_time: SimTime`** (OQ1), rather than retyping it.
No evaluator-signature churn while other tracks edit those files; the retype stays deferrable.
`ctx.game_scale()` and `ctx.game_frames_crossed()` (0 or 1, always 1 on an unscaled advanced tick)
are the guards.

**10. The evaluators retype additively** (supersedes the deferral in 6). Each public closed form takes
`impl Into<GameTime>` (`bullet_age`, `bullet_position`, `bullet_is_alive`, `volley_hits`, `live_window`,
`particles::evaluate`, `switch_to_ragdoll`), with `From<SimTime> for GameTime` for whole ticks, so a caller
passing `ctx.game_time` still compiles and gets floor-time results; `ctx.game_instant` gets the fractional
ones. For an integer instant every expression gives the same bits as before (`GameTime::ticks_since`
replaces the integer delta, comparisons run in Q16), so no trace, golden or replay moves.

**11. `held` passes a fractional instant through when `hold <= 1`.** Snapping to the floor would kill
fractional time for every default animation; only a stop-motion `hold > 1` snaps, to a whole tick.

**12. `set_clip_speed` keeps `now: SimTime`.** `ClipAnchor.frame` is a whole `FrameIndex` in the locked
component schema, so a speed change applies on the floor gameplay frame, with a sub-frame pose jump of
fraction × speed difference. A fractional anchor is out of scope.

**13. The timeline and `Timer::just_expired` guard on `game_frames_crossed() > 0`.** Identical unscaled;
under a scale it fires once per crossed gameplay frame instead of on every tick of a repeated frame.
`ai_tick` keeps `game_advanced()`: machine callbacks integrate `ctx.game_dt`.

**14. Animation events resume on the same gameplay tick.** `AnimationWalk.tick` stays the floor
gameplay tick; a walk at the same tick resumes from the record, so clip time advances fractionally and
each event fires once, in order, on the sim tick that crosses it.

**7. No scale above 1** (OQ3). **Hold stays off physics by default** (OQ4). **No audio by default**,
pitch handed to request 0023 (OQ5). **One clock, no per-entity dilation** (OQ6).

**8. Compatibility.** `GameClock::advance() -> bool` and `SystemCtx::with_clock(&clock, advanced)` keep
their signatures. New `GameClock` fields are omitted from serde at identity, so snapshots and replays of
a game that never scales do not move; `GameClock` stays out of `WorldHash`.

**9. Scale state stays out of the snapshot schema.** The scale state is serialized but outside the typed
snapshot schema, so `replay_content_hash` and the corpus are unchanged. Cost: a build without scale
support would restore a scaled snapshot as an unscaled clock, silently. Fold the fields into the schema at
the next deliberate corpus re-record.

## Waves

- **W1 (shipped)** the clock, `GameTime`, `SystemCtx` fields, driver fold, `L4260`, docs, the
  `slow_motion` example. `game_dt` integrators follow; nothing else does.
- **W2 (shipped)** the closed forms in `ludion-engine` read `game_instant`: `extract.rs` particles, clip phase and
  volleys; `animation_events`; `parent_joint`; `ragdoll`; `patterns` (the timeline walks
  `game_frames_crossed`, volley age is fractional); the volley bullet evaluators (`patterns.rs`
  `bullet_age`/`bullet_position`/`bullet_is_alive`/`volley_hits`).
- **W3 (shipped)** physics: the `step` system calls `scale_time(s)` for `0 < s < 1` (no opt-in; a hold
  still does not reach physics); `PhysicsWorld::gameplay_dt` scaled; the kinematic and projectile
  paths pinned by tests.
- **W4 (shipped)** the acceptance run in `tests/game_time_scale.rs` (volley, emitter, animated clip with
  events, platform with rider, dynamic body at s=0.25), a `WorldHash` trace, a mid-slow-down
  restore, and 1/2/8-thread identity.

## Decisions (W3, W4)

**15. The step hook is automatic.** `ludion-physics`'s `step` system reads `ctx.game_scale()` and, for
`0 < s < 1`, calls `scale_time(s.max(MIN_TIME_SCALE))` before the step, composing by `min` with the
game's own call. It is issued and consumed inside the step, so the `to_bytes` guard never sees it, and
nothing enters the snapshot or `WorldHash`.

**16. No `hold_reaches_physics` opt-in.** `PhysicsWorld::freeze()` beside `hold()` already is that
switch; a second would be a second way to do one thing. A hold calls nothing on physics.

**17. `MIN_TIME_SCALE` clamp.** `GameClock::MIN_SCALE` (1/1024) is a hair below physics' `MIN_TIME_SCALE`
(1e-3); physics clamps up to its own floor, a 2.4% mismatch at the extreme only. Neither constant moves.

**18. The motor reads the applied scale.** `gameplay_dt` is `0` after a held frame, else
`seconds_per_tick × applied_scale()` (`× 1.0` exactly when unscaled).

**19. No committed `.trace.txt` for the scale.** The gallery trace gates tick the built-in schedule with
no game systems and never scale. The in-test `WorldHash` trace with a restore and 1/2/8-thread identity
(`tests/game_time_scale.rs`) is the trace of record.

**20. No chase camera in the acceptance scene.** A rig needs a `kind: path` track plus a target, which
is over budget. Camera springs integrate `game_dt` (they follow by construction), but no scaled-camera
test pins them. The acceptance run lives in `tests/game_time_scale.rs`, not `game_clock.rs`.

**21. Physics matches at a measured bound, not f32 epsilon.** A smaller step changes semi-implicit
Euler's error. The measured gaps are ~0.1 m on a fall (bound 0.15 m in the acceptance run), 0.03 m
for the platform and 0.1 m for its rider. Closed forms match exactly.

## Proposed — owner ruling owed

- (a) A `GameClock` `WorldHash` term when non-identity (moves no existing hash only if gated on
  non-identity; a `WorldHash` design change).
- (b) The `Plan/physics.md` §Time scale amendment, "a gameplay scale drives `scale_time`" (W3).
- (c) The `docs/game-clock.md` rule extension to fractional time (shipped as docs in W1; binding only as
  evaluators adopt it).
- (d) `MIN_SCALE` 1/1024.
- (e) Fold `slowed_q16`, `pending_q16` and `step_q16` into the snapshot schema at the next deliberate
  corpus re-record (decision 9); folding now would move `replay_content_hash` and force a corpus re-record.
- (f) No GameClock::hold_reaches_physics opt-in (the request's §3 / open question 4): freeze() beside
  hold() covers it (decision 16). If the owner wants the request's API: a GameClock bool that makes the
  step system call freeze() on a held tick; it touches GameClock's snapshot field.

## Out of scope

Closed-form evaluators (W2); physics `scale_time` and the motor's `gameplay_dt` (W3); the acceptance run,
hash trace and thread identity (W4); the `WorldHash` term (owner ruling); audio pitch (request 0023);
scales above 1; per-entity time dilation; retyping `game_time`; fractional `started` stamps. Each
reopens when a game needs it.

## Status

- **W1:** shipped, Lore rev 2373.
- **W2:** shipped, Lore rev 2374 (integrated green at 2376). The volley bullet evaluators read the
  fractional instant. The `ludion-projectiles` pool has no closed form: it integrates `ctx.game_dt`, so it
  follows the scale; W3's tests pin it.
- **W3:** shipped, Lore rev 2378.
- **W4:** shipped, Lore rev 2378.
- **Owner rulings owed:** (a)–(f) in "Proposed" above. (b) is applied conservatively: `Plan/physics.md`
  §Time scale carries the amendment line.
