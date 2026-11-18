---
description: Freezing gameplay time while the sim keeps ticking — GameClock, SystemCtx's game_* fields
---

# The gameplay clock

**Scope:** Freezing gameplay time on demand while the sim keeps ticking — `GameClock`, the two
clocks on `SystemCtx`, and which evaluators read which. Registering systems and the sim's own
`frame`/`time`/`dt` is `ludion docs systems`; the components that read gameplay time are
`ludion docs patterns`, `ludion docs particles`, `ludion docs sprite-animation`, `ludion docs camera`; carrying a hold
across a district switch is `ludion docs scene-transitions`.

## The loop

A game freezes by calling `hold()` on the `GameClock` resource every frame it wants frozen — a
pause overlay's own system, guarded however the game decides a freeze is in effect:

```rust
use ludion::prelude::*;

// `struct Paused(bool)` is the game's own resource.
fn freeze_while_paused(world: &mut World, _ctx: &SystemCtx) {
    if world.resource::<Paused>().unwrap().0 {
        world.resource_mut::<GameClock>().unwrap().hold();
    }
}
```

`hold()` is idempotent within a frame, so several systems may call it in the same tick at no cost.
There is no CLI instrument for a hold — `ludion render --frames` ticks the built-in schedule with
no game systems, so it never calls `hold`. Observe one the way
`crates/ludion-engine/tests/game_clock.rs` does: build an `Engine` with a system that calls `hold()`
across a frame window, tick it, and compare `extract_at`/`bullet_position` results against an
unheld run of the same scene over the same span of frames.

## Format

`SystemCtx` carries two clocks. On an unheld run (no game ever calls `hold`) they agree exactly.

| Field | Meaning | On a held frame |
|---|---|---|
| `frame` / `time` / `dt` | the sim's own — advance every tick regardless | unaffected: the host and a pause overlay still need these |
| `game_frame` / `game_time` / `game_dt` | the same three, read through `GameClock` | stand still: `game_dt` is `0.0`; `game_frame`/`game_time` repeat the frame gameplay time stopped on |

`ctx.game_advanced()` is `ctx.game_dt > 0.0` — the guard a system that must fire once per gameplay
frame (not once per sim tick) returns on.

## Rules

- **Every closed-form evaluator reads gameplay time, and every `started` stamp is a gameplay
  frame.** `bullet_age`/`bullet_is_alive`/`bullet_position`/`volley_hits` take a `game_now`
  parameter — pass `ctx.game_time`, never `ctx.time`. `PatternPlayer.started`,
  `ParticleEmitter.started` and `Animation.started` all want `ctx.game_frame`, never `ctx.frame`:
  on an unheld run the two are equal, so an existing stamp is unaffected; on a held run a
  `ctx.frame` stamp lands in the future and the effect starts late.
- **The exception: `UiTween` stamps default to `clock: real`**, the sim `ctx.frame`, so a pause menu animates over a held game; set `clock: game` for a tween that should freeze with it (`ludion docs ui-tweens`).
- **Input edges are per sim tick, not per gameplay frame.** `was_pressed` is true on one tick,
  including a held one, so a system that returns on `!ctx.game_advanced()` loses a press made
  during a hold. Latch it in an unguarded system ahead of the guarded one, and consume the latch on
  the next advanced frame.
- `play_patterns` walks the timeline on `ctx.game_frame` and returns immediately on
  `!ctx.game_advanced()`, so a hold neither re-fires a timeline entry nor skips one whose frame
  passed while frozen. `advance_volleys` still runs every tick (reaping is idempotent) but ages on
  `ctx.game_time`, so nothing expires during a hold — a volley fired on the first held frame has
  negative age until the thaw and is neither drawn nor hit-testable until then.
- **Cancel is a despawn over a `Volley` query**, and what appears in a cancelled bullet's place is
  the game's own spawn at `bullet_position(…, ctx.game_time, …)` — the loop is under Rules in
  `ludion docs patterns`.

- Animation playback — flipbooks and assembly clips alike — follows the gameplay clock too: one
  clock, no per-component opt-out.
- `GameClock` is a resource, so it is never a `WorldHash` term; a diverging hold surfaces
  indirectly, as diverging entity data (volleys reaped or not, timelines fired or not). It **is**
  carried by `engine.snapshot`/`engine.restore` — the `game_clock` field, omitted from the document
  when the clock holds nothing, so a snapshot recorded before this field existed restores
  unchanged. A replay needs nothing extra: the game re-derives its holds from the replayed input
  that drove the freeze in the first place.
- `Command::SimPause` (`sim.pause`) is the other freeze mechanism and stays distinct: it stops the
  sim ticking at all, overlay input included. `GameClock` keeps the sim ticking and stops only
  gameplay time.
- **A hold does not reach physics.** `GameClock` freezes gameplay time only; rigid bodies keep stepping
  through a hold. To stop them on the same frames, call `PhysicsWorld::freeze()` beside `hold()` (or
  `scale_time` for slow motion) — `ludion docs physics-time-scale`.
- A district switch resets `GameClock` to unheld like every other resource, unless the game
  declares `EngineBuilder::carry_resource::<GameClock>()` (`ludion docs scene-transitions`).

## Gotchas

- Passing `ctx.time` instead of `ctx.game_time` to a pattern evaluator ages and hit-tests bullets
  through a hold — they keep moving on a frozen screen and a hit test scores against where they
  "would" be, not where they're drawn.
- Over the wire, a client stamps `started` from the response envelope's `game_frame`, never its
  `frame`: it is the wire twin of `ctx.game_frame` (`ludion_status` shows both).
- Stamping a `started` field from `ctx.frame` instead of `ctx.game_frame` makes an effect fired
  mid-freeze start late by **every frame the clock has held so far this session** (`GameClock.held`
  accumulates and never resets), not just the freeze in progress.
- **Gameplay time resumes two sim frames after the last `hold()`, not one.** The fold that turns a
  marked frame into a held frame runs at the *start* of the next tick, so the tick right after the
  last held one still repeats the last gameplay frame; the one after that is the first to advance
  again.
- A held `GameClock` does not survive a scene switch by default — a game holding through a district
  change needs `carry_resource::<GameClock>()`; without it the new district starts unheld even if
  the departing one was mid-freeze.

## Next

- `ludion docs systems` — the `SystemCtx` contract, registration order, and the determinism rules.
- `ludion docs ai` — `Timer`/`Cooldown` and agent state machines read gameplay time, so they freeze while the game holds.
- `ludion docs patterns` — `Volley`/`PatternPlayer`, and the closed forms that read gameplay time.
- `ludion docs particles` — `ParticleEmitter`'s live window, the other closed-form evaluator.
- `ludion docs sprite-animation` — flipbook/clip playback through `Animation`.
- `ludion docs scene-transitions` — `carry_resource`, and what does and doesn't survive a switch.
