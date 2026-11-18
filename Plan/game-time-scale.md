# Gameplay time scale — a fractional `GameClock`

> **Status:** Wave 1 shipped in the working tree (see Status). Waves 2–4 planned.
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
- **W2** the closed forms in `ludion-engine` read `game_instant`: `extract.rs` particles, clip phase and
  volleys; `animation_events`; `parent_joint`; `ragdoll`; `patterns` (the timeline walks
  `game_frames_crossed`, volley age is fractional); the `ludion-projectiles` bullet evaluators.
- **W3** physics: the engine calls `scale_time(s)` inside the step frame; opt-in
  `GameClock::hold_reaches_physics`; `PhysicsWorld::gameplay_dt` scaled.
- **W4** the acceptance run in `tests/game_clock.rs` (volley, emitter, skinned clip with events,
  platform with rider, dynamic body, chase camera at s=0.25), a `WorldHash` trace, a mid-slow-down
  restore, and 1/2/8-thread identity.

## Proposed — owner ruling owed

- (a) A `GameClock` `WorldHash` term when non-identity (moves no existing hash only if gated on
  non-identity; a `WorldHash` design change).
- (b) The `Plan/physics.md` §Time scale amendment, "a gameplay scale drives `scale_time`" (W3).
- (c) The `docs/game-clock.md` rule extension to fractional time (shipped as docs in W1; binding only as
  evaluators adopt it).
- (d) `MIN_SCALE` 1/1024.
- (e) Fold `slowed_q16`, `pending_q16` and `step_q16` into the snapshot schema at the next deliberate
  corpus re-record (decision 9); folding now would move `replay_content_hash` and force a corpus re-record.

## Out of scope

Closed-form evaluators (W2); physics `scale_time` and the motor's `gameplay_dt` (W3); the acceptance run,
hash trace and thread identity (W4); the `WorldHash` term (owner ruling); audio pitch (request 0023);
scales above 1; per-entity time dilation; retyping `game_time`; fractional `started` stamps. Each
reopens when a game needs it.

## Status

- **W1:** in the working tree, rev to be filled by the tech lead.
