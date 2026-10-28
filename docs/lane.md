---
description: LaneConstraint — hold a CharacterMotor to a kind: path lane, blend in and out, lane draw
---

# Lanes

**Scope:** holding a `CharacterMotor` to a `kind: path` corridor so a 3D character runs a lane: the `LaneConstraint` component, what `move_character` does with it, easing in and out, the lane frame and `lane_relative` for a stick, `MoveReport.lane`, the `lane` debug draw and trace fields. The motor itself is `ludion docs motor`; the path document is `ludion docs path`.

A lane is a **constraint on the motor, not a mode**: with no `LaneConstraint` (or one that is off) `move_character` is the unconstrained move, bit for bit. The lane is horizontal only — vertical motion (jump, fall, ramps) stays the motor's.

## Format

```yaml
LaneConstraint:
  path: paths/corridor.path.yaml   # kind: path, relative to the content root (default "" — inert)
  engaged: true                    # eases the weight toward 1; false eases toward 0 (default false)
  offset: 0.0                      # metres right of the path, along the lane lateral (default 0.0)
  engage_time: 0.2                 # gameplay seconds for blend 0 -> 1; 0 is instant (default 0.2)
  disengage_time: 0.3              # gameplay seconds for blend 1 -> 0 (default 0.3)
  pull_speed: 4.0                  # m/s the pull to the centre line moves the feet at full weight (default 4.0)
  search_window: 2.0               # metres of arc length either side of s searched for the nearest point (default 2.0)
  ends: extend                     # extend | stop, past the end of an open path (default extend)
  # s: null                        # runtime: metres along the path, null until the lane finds its place
  # blend: 0.0                     # runtime: the linear blend 0..1
```

A game writes `engaged`; `move_character` writes `s` and `blend` (both hashed, so a snapshot resumes mid-ease). The entity needs a `CharacterMotor` (`L1302`).

## What the motor does

Per `move_character` call, with weight `w = smoothstep(blend)` (`blend` rises linearly over `engage_time`, falls over `disengage_time`, on the **gameplay clock**; reversible mid-ease; a time of `0` is instant):

1. **Projection.** Every horizontal displacement the motor applies — the requested step, each slide remainder, both platform-carry passes — loses `w` times its component along the lane's lateral. A conveyor or a platform crossing the lane cannot shove the character out of it; one running along the lane still carries it.
2. **Pull.** After the slide a separate correction moves the feet toward the centre line, at most `w * pull_speed * dt` metres per call. It is swept like any displacement, so a wall stops it (`pull_blocked`) instead of the feet creeping through.
3. **Vertical is free.** Gravity, jumps, steps and ground snap are untouched.
4. **Re-find.** The new `s` is the nearest path point to the feet within `search_window` plus the tick's travel, using the full 3D distance, so a lane passing above itself never jumps levels.

Past the end of an open path, `ends: extend` continues the lane straight along the last horizontal direction; `ends: stop` clamps the character at the end. A closed path wraps `s`.

## Switching, teleporting

- A lane that was fully off acquires `s` by the nearest point over the **whole** path when it engages, then pulls in capped and eased. A big offset is not a snap. Engaging from `blend` 0 always runs this global acquire; a game that knows better writes `s` once `blend > 0` (a written `s` at `blend` 0 is replaced).
- Changing the path at runtime must also null `s`: `LaneConstraint::set_path(path)` does both. A write to `path` alone leaves the old `s` in place.
- After `teleport_character` (or any `Transform` write that moves the feet far), set `s` to `None` so the lane finds the global nearest again.
- `move_character_with` (an explicit `MoveConstraint`) **bypasses the lane**: one constraint per call.

## Frame and stick

`lane_frame(world, entity) -> Option<LaneFrame>` reads the frame at the lane's current `s` — `point`, `tangent`, `forward` (horizontal), `lateral = forward x up` (the lane's right), `weight` — for a game system that decides its step before it moves; `None` when the lane is off or has not found its place (it reads the path itself, so it never waits on a move). `lane_relative(stick, &frame, screen_right)` turns a `[x, y]` stick into a direction along the lane: the stick's horizontal axis runs the lane whichever way the camera sees it. Below `cos 80°` between the screen's right and the lane it falls back to `+forward`.

```rust
let free = [stick[0], 0.0, stick[1]];
let direction = match lane_frame(world, hero) {
    Some(frame) => {
        let lane = lane_relative(stick, &frame, camera_right).direction;   // .along / .across are the signed parts
        std::array::from_fn(|i| free[i] + (lane[i] - free[i]) * frame.weight)
    }
    None => free,
};
let step = direction.map(|c| c * speed * dt);
move_character(world, hero, [step[0], vertical, step[2]])?;
```

`None` is normal before the lane has found its place, so a game must still move then, or the lane never engages.

## The report

`MoveReport.lane: Option<LaneReport>` is `Some` on a call that ran on a lane:

| field | meaning |
| --- | --- |
| `s`, `ds` | metres along the path after the move; the change over the move (carry included; a closed lane reports the short way round) |
| `weight`, `engaged`, `easing` | the weight used; the component flag; whether `blend` was still moving |
| `lateral_error` | signed metres from the centre line to the feet, along `lateral`, after the move |
| `pull_blocked` | a wall stopped the pull |
| `frame` | the `LaneFrame` at the final `s` |
| `at_end` | `Start` / `End` of an open path (either `ends` mode) |

## Debug draw

Category `lane` (`--debug-draw lane`), drawn by `ludion_engine::move_character` on a lane call, hash-neutral. Consts are in the prelude:

| const | draws |
| --- | --- |
| `LANE_PATH` | the path polyline at the lane offset, one sample per 0.25 m (≤ 256 segments); its alpha is the weight, so the lane fades in as it engages |
| `LANE_ANCHOR` | a cross at the lane frame's point |
| `LANE_FORWARD` / `LANE_LATERAL` | the frame's forward and lateral rays, 0.5 m |
| `LANE_ON` .. `LANE_OFF` | the line from the feet to the anchor, blended by `lane_error_colour(lateral_error)`; full `LANE_OFF` from `LANE_ERROR_FULL` (0.5 m) |

## Trace

A `motor` trace line for a lane call carries `"lane":{"s","ds","weight","lateral_error","pull_blocked"}` after `height`; lines without a lane have no `lane` key (`ludion docs verify-loop --section "Numeric trace"`).

## Diagnostics

| code | raised when |
| --- | --- |
| L1300 | a `LaneConstraint` field is out of range (a time, `pull_speed` or `search_window` negative or not finite, `offset`/`s` not finite, `blend` outside 0..1): check-scene, and once per entity at runtime for a lane built in Rust |
| L1301 | the `path` fails to load (runtime, once per entity; the lane stays inert) |
| L1302 | a `LaneConstraint` entity has no `CharacterMotor` (check-scene, runtime) |
| L1303 | warning: the path has a near-vertical span (horizontal tangent under 0.05), where the lane frame is carried from the neighbouring stretch (scanned once per path, reported once per entity) |

`ludion explain L1300` prints the fix.

## Gotchas

- One lane per entity; a second lane is a second entity or a switch of `path` (`set_path`).
- Vertical is unconstrained. A full 3D lock (a tube) is not offered.
- Keep a lane path off vertical runs; `L1303` names the span.
- A `path` change while engaged needs `s` nulled too, or the lane keeps searching near the old `s`.

## Next

- `ludion docs motor` — the motor the lane constrains; `ludion docs path` — authoring the path
- `ludion docs debug-draw`, `ludion docs verify-loop` — seeing and tracing a lane
