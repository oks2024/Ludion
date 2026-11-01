---
description: The kind: path document, PathSampler, and PathFollower moving platforms on gameplay time
---

# Paths and path followers

**Scope:** a spline authored as text (`kind: path`), sampling it by arc length from game code, and
the engine's `PathFollower` that rides an entity along it (a character motor's lane is `ludion docs lane`, a camera rail `ludion docs camera`). Component defaults live in
`ludion docs components-fx`; gameplay time is `ludion docs game-clock`.

## The loop

1. Write `paths/lift.path.yaml` under the content root (format below).
2. `ludion check-scene paths/lift.path.yaml` — validates it, `L1213`/`L1214` with JSON Pointers.
3. Put `PathFollower: { path: paths/lift.path.yaml, speed: 2.0 }` on an entity in the scene file.
4. `ludion render --scene scenes/main.yaml --frames 120` renders the scene after 120 ticks; the bare
   render host installs the follower system too, so the platform is where it has ridden to.

## Format

```yaml
format: 1
kind: path
name: lift                   # optional
generated_from: generation/blender/level/build.py   # optional
interpolation: catmull_rom   # or linear (default catmull_rom)
closed: false                # default false
points:
  - - 0.0
    - 0.0
    - 0.0
  - - 4.0
    - 0.5
    - -6.0
```

Points are block-of-block (`- - x`, then the other two components aligned under it); a flow sequence
`[0, 0, 0]` is refused. `catmull_rom` is centripetal, so an uneven point spacing never loops or
cusps, and it passes through every point; an open path extends its ends by reflection. `linear` is
straight segments. Canonical text always spells `interpolation` and `closed`.

`generated_from` is provenance: the content-root-relative script that emitted the file (unlike other kinds'
`generated_from`, which resolves against the document's directory). Editors treat the file as generated;
the engine and the map-drift gate run no drift check, so re-run your script and diff.

| Code | Raised for |
|---|---|
| `L1213` | empty/non-mapping root, envelope, wrong type (`generated_from` included), unknown key or `interpolation`, a point that is not three numbers |
| `L1214` | fewer than 2 points (3 when `closed`), a non-finite or out-of-f32 component, coincident neighbours (the last and first too when closed), a blank, absolute or `..` `generated_from` |
| `L1269` | a `PathFollower` `speed` or `phase` that is not a finite `f32` (a follower's, not a path document's) |

## Sampling from game code

`PathSampler` (in the prelude, with `check_path`, `PathDocument`, `PathFrame` and `PathMetric`) is a pure function of a
validated document — `+ − × ÷` and `sqrt` only, so it is bit-identical on every machine:

```rust
let doc = check_path(&std::fs::read_to_string("paths/lift.path.yaml")?)?;
let sampler = PathSampler::new(&doc);
let here = sampler.point_at(2.5);     // metres of arc length; open clamps to [0, length], closed wraps
let facing = sampler.tangent_at(2.5); // unit curve derivative, continuous on catmull_rom
let frame = sampler.frame_at(2.5);    // PathFrame { position, tangent, right, up }
let q = frame.rotation();             // [x, y, z, w], w >= 0
let s = sampler.nearest_s(player, last_s, 4.0, PathMetric::Full);
```

`s` is true arc length on both interpolations: a `catmull_rom` path integrates the curve speed per
table sub-interval (Gauss-Legendre) and inverts it with a fixed-count Newton, bit-identical
everywhere. `point_at` therefore moves at unit speed and `s` may be differentiated. A `catmull_rom`
`length()` is the true arc length (to quadrature precision, far below a millimetre; a closed
follower wraps on it, a ping-pong turns on it). A `linear` tangent is its segment's direction: at an interior corner the outgoing segment's, at an
open end the last segment's.

`frame_at` is a no-roll frame: `right` is horizontal and `up` leans toward +Y (the reference up is
fixed; a loop-de-loop is not supported). Where the tangent is vertical, `right` carries over from the
nearest non-vertical stretch, so it does not flip through a vertical run (a Catmull-Rom S-bend whose
tangent swings past vertical does flip `right`, inherent to a +Y no-roll frame). `rotation()` has
local −Z facing travel (as a camera does), +Y on `up`, +X on `right`.

`nearest_s(p, hint, window_metres, metric)` searches `window_metres` of arc length either side of
`hint` (open clamps, closed wraps; a window of at least half the length is global). Pass the
previous result as `hint` and a window shorter than any hairpin, or a switchback hands back the wrong
leg. `nearest_s_global` ignores the window. `PathMetric::Horizontal` measures in XZ and uses height
only to break ties, for legs that cross at different heights. Ties go to the smaller `s`.

## `PathFollower`

`path` (content-root-relative; `""` is inert), `speed` (metres per gameplay second, negative runs
backwards, `1.0`), `mode` (`loop` | `ping_pong` | `once`, `loop`), `phase` (metres along the path,
the authored start offset, rewritten every tick), `orient` (`none` | `yaw` | `frame`, `none`).

The engine's `path_follow` system runs **before** every game system and writes
`Transform.translation` (scale untouched, rotation too unless `orient` is set) every tick, so a system that rides the
platform reads *this* tick's pose. `phase` advances by `speed · game_dt` only when gameplay time
advanced, so a `GameClock` hold freezes platforms. `loop` wraps (an open path jumps end → start),
`ping_pong` runs `0 → L → 0` with `phase` counting to `2L`, `once` clamps to `[0, L]`.

To drive the position yourself, set `speed: 0` and write `phase`; it poses on the next tick. The pose
is in the parent's frame: points are scene coordinates for a root.

**Orientation.** `orient: yaw` turns the follower about +Y to face the horizontal travel direction;
`frame` uses the path's no-roll frame (`frame_at`). Local **−Z faces travel** and +Y is
up, as for cameras and glTF; a ping-pong return leg or a negative `speed` faces backwards. The rotation is
written as Euler degrees into `Transform.rotation`, in the parent's frame like the translation. On a
vertical stretch `yaw` writes nothing and the previous rotation stays (a path that *starts* vertical keeps
the authored rotation until it first moves horizontally). `orient` is omitted from a serialized follower
while `none`, so only scenes that set it change hash. An oriented follower also draws its axes (X red, Y
green, Z blue) under `path`.

**Motion.** `path_follower_motion(world, entity) -> Option<FollowerMotion>` reads what this tick did:
`s`, `direction` (the sign of travel, `-1` towards the start; flips on a ping-pong return; with `speed` 0,
the sign of the phase change, kept when it held), `delta` (the translation
written minus the one replaced), `velocity` (`delta / game_dt`, zero on a held tick and on a
teleport) and `teleported`. It is recomputed before any game system, never hashed or snapshotted,
and `None` for a non-follower, a follower that posed nothing, or straight after a restore.

**Teleports.** A tick that wraps an open `loop` path, or moves further than `|speed|·dt + 0.5 m`, is a
teleport (the slack keeps a game-driven `phase` at `speed: 0` carrying its riders). A `kinematic`
follower is then snapped to the new pose with zero velocity instead of driven, so riders are left
behind, not launched. For a smaller deliberate jump call `teleport_kinematic(world, entity)` after
writing the pose.

## Rules

- `path_follow` is installed on every schedule (scene load, restore, district switch) and lists first
  in `engine.describe`, at `order: 0`; in a world without a follower it costs one empty query. A
  follower spawned at runtime moves like an authored one.
- A path that cannot be read or fails validation raises `L1215` once per entity; the entity stays put.
- A `speed` or `phase` that is not a finite `f32` (NaN, infinite, or past `f32` range) is `L1269` at `/speed` or `/phase`; built in Rust, the follower is held where it is and `L1269` is raised once per entity.
- The path library is read at load, restore and district switch and never hashed or snapshotted.

## Gotchas

- `asset.reload` does not re-read a path: an edited path takes effect at the next load, restore or
  switch, which keeps a mid-session reload replay-safe.
- A follower on a `rigid` body has its pose overwritten by the physics mirror every tick. Put it on a
  `kinematic` body or none.
- A block-style file only: a flow-style `points: [[0,0,0]]` fails `L1005`.

## Next

- `ludion docs game-clock` — gameplay time, holds, and `ctx.game_advanced()`.
- `ludion docs components-fx` — the `PathFollower` row beside the other content components.
- `ludion docs physics` — kinematic bodies and the mirror.
- `ludion docs camera` — a `CameraRig` rides a path as a track.
