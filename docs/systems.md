---
description: Writing systems: queries, time, keyboard/mouse/gamepad input, the seeded RNG, determinism rules
---

# Systems

**Scope:** The `fn(&mut World, &SystemCtx)` contract — queries, the fixed timestep, every input surface, randomness, and the determinism rules your game code must hold. Spatial queries are collision; copyable patterns are recipes.

A system is a plain function `fn(&mut World, &SystemCtx)`. You register systems in order on the
builder; **registration order is execution order**, once per fixed tick.

```rust
use ludion::prelude::*;

fn move_ball(world: &mut World, ctx: &SystemCtx) {
    for (ball, transform) in world.query::<(&Ball, &mut Transform)>() {
        transform.translation[0] += ball.velocity[0] * ctx.dt;
        transform.translation[1] += ball.velocity[1] * ctx.dt;
    }
}
```

**Queries.** `world.query::<(&A, &mut B)>()` yields one tuple per entity that has *all* the named
components, in ascending entity order. Arity 1–4; a single-element query is `(&mut A,)` (note the
trailing comma). A query needs `&mut World` even when read-only, and must not name the same component
type twice. A query yields component references only — **not** the `EntityId`. When you need ids (to
despawn, say), use `world.query_ids::<(&A,)>()`, which yields `(EntityId, (&A,))`, or iterate
`world.entities()` and read with `world.get`/`world.has` — the copyable form is in `ludion docs recipes`.
A singleton (the only carrier of a component: the run state, the player, the camera) is
`world.single::<T>()` / `world.single_get::<T>()` — the first carrier in entity order, `None` when absent.

**Time — only through `ctx`.** `ctx.dt: f32` is the fixed timestep (`1/60` s by default and never
varies); `ctx.frame: FrameIndex` is the tick count; `ctx.time: SimTime` gives exact
`ctx.time.seconds()`. Never read a wall clock — it would break determinism (and `Instant::now` is a
clippy build error here). `ctx.game_frame`/`ctx.game_time`/`ctx.game_dt` are the same three read
through a `GameClock` a game may hold while frozen; they equal the sim ones exactly until a game
calls `hold()` (`ludion docs game-clock`).

**Input — only through `InputState`.** Read the resource; do not touch OS events.

```rust
fn steer(world: &mut World, _ctx: &SystemCtx) {
    let input = world.resource::<InputState>().unwrap();
    let left = input.is_held(Key::Left);       // level: currently down
    let fired = input.was_pressed(Key::Space); // edge: went down this frame
    // ... then mutate components in a separate query (the borrow of `input` ends first).
    let _ = (left, fired);
}
```

`is_held` is level; `was_pressed`/`was_released` are one-frame edges. `Key` variants games use:
`A`–`Z`, `Digit0`–`Digit9`, `Up`/`Down`/`Left`/`Right`, `Space`, `Enter`, `Escape`.

**Mouse — `input.mouse()`.** `MouseState` carries the same level/edge vocabulary:

- `input.mouse().position()` — `[f32; 2]` in **render-target pixels, origin top-left, +Y down**
  (level: it persists until the next move). This is the pixel space the aim recipe below unprojects.
- `input.mouse().is_held(MouseButton::Left)`, `was_pressed`, `was_released` — buttons, exactly like
  keys (`MouseButton` is `Left` / `Right` / `Middle`).
- `input.mouse().wheel()` — `[f32; 2]`, this frame's accumulated scroll (an edge-like accumulator:
  it sums every wheel motion during the tick and resets to zero at the next tick).

**Gamepad — `input.gamepad()`.** Player one's pad (the capture layer forwards the first connected
pad only). Sticks and triggers are **level analog**; buttons are edges like keys.

- `input.gamepad().left_stick()` / `.right_stick()` — `[f32; 2]`, each axis in `[-1, 1]`. **+Y is
  up** (the capture layer normalizes to that convention), so pushing a stick up gives `+y` — most
  ground-plane games map that to "away from the camera" (world `−Z`), i.e. use `-y`.
- `input.gamepad().left_trigger()` / `.right_trigger()` — `f32` in `[0, 1]`.
- `input.gamepad().is_held(GamepadButton::South)`, `was_pressed`, `was_released` — buttons.
  `GamepadButton` names faces by **position** (`South`/`East`/`West`/`North` — Xbox A/B/X/Y,
  PlayStation ✕/○/▢/△), plus `LeftShoulder`/`RightShoulder`, `LeftStick`/`RightStick` (clicks),
  `Start`/`Select`, and `DpadUp`/`DpadDown`/`DpadLeft`/`DpadRight`.

**Headless is quiescent, not broken.** Capture lives out-of-sim in the window (`--window`) and the
editor; a plain `ludion serve` / a headless test has nothing feeding it, so `InputState` stays at its
default — mouse at `[0, 0]`, no buttons, zero sticks. A game reading input headless simply sees no
action. That is the correct degrade, and it is what every golden/CI run relies on. Drive input in a
test by scripting it (an `InputScript`, or `input.inject` over the wire) — the same data the window
records, so a test and a live session run the identical path.

**Aim recipe — cursor/stick to the ground plane.** To point something at where the cursor (or a
stick) aims on the `y = 0` ground, turn the pixel into a world ray and intersect it. The two helpers
live in `ludion::math` (re-exported in the prelude as a module, so its `Projection` does not clash with the
`Camera` component's — write `math::Projection`). `ludion::math` also carries
`euler_xyz_degrees_to_quat` / `quat_to_euler_xyz_degrees`, the engine's `Transform.rotation` ↔ quaternion pair:

```rust
use ludion::prelude::*;

fn aim(world: &mut World, _ctx: &SystemCtx) {
    let cursor = world.resource::<InputState>().unwrap().mouse().position();
    // The scene's camera pose + projection (copy it out so the borrow ends).
    let Some((cam, xform)) = world
        .query::<(&Camera, &Transform)>()
        .next()
        .map(|(c, t)| (*c, *t))
    else {
        return;
    };
    let projection = match cam.projection {
        Projection::Perspective { fov_y_deg } => math::Projection::Perspective { fov_y_deg },
        Projection::Orthographic { height, .. } => math::Projection::Orthographic { height },
    };
    // `(1280, 720)` is *your* render resolution — the space `mouse().position()` reports in.
    if let Some(ray) = math::camera_ray(
        xform.translation, xform.rotation, projection, cam.near, cam.far, cursor, (1280, 720),
    ) && let Some([ax, az]) = math::ray_ground_plane(&ray) {
        // Face a turret at the origin toward (ax, az): local forward is −Z, so yaw = atan2(−dx, −dz).
        let yaw = math::atan2(-ax, -az) * (180.0 / core::f32::consts::PI);
        for (_turret, t) in world.query::<(&Turret, &mut Transform)>() {
            t.rotation = [0.0, yaw, 0.0];
        }
    }
}
```

`camera_ray` returns `None` on a degenerate camera (zero viewport, `fov` of 0, …); `ray_ground_plane`
returns `None` when the ray is parallel to or points away from the ground (e.g. the cursor is above
the horizon) — handle both as "no aim this frame." For a **twin-stick** game, take the right stick
directly when it deflects past a small deadzone and fall back to the cursor otherwise; both feed the
same `yaw`. All the trig is `math::atan2` (not `f32::atan2`) because the result becomes hashed sim
state — the aim must be bit-identical across OSes. The complete, runnable version is the
`cursor_aimed_turret` example (`cargo run -p ludion --example cursor_aimed_turret`).

Analog input rarely needs hand-authoring: the window records mouse/stick values for you, and on the
wire/replay each analog field travels as its exact IEEE-754 bit pattern under a `*_bits` name (e.g.
`{"kind": "gamepad_axis", "axis": "left_stick_x", "value_bits": 1056964608}`) so a replayed aim is
bit-exact. Keys and buttons stay fully symbolic and hand-writable
(`{"kind": "key_down", "key": "Space"}`, `{"kind": "mouse_down", "button": "left"}`).

**Randomness — the one seeded RNG.** The sim inserts a single `DeterministicRng` resource, seeded
from the scene's `seed`. Read it with `world.resource_mut::<DeterministicRng>()` (`next_u32`,
`next_f32`, `gen_range_u32`, …). Never `thread_rng` — it is nondeterministic. The RNG type is not in
the prelude: add `ludion-core` to `[dependencies]` and `use ludion_core::rng::DeterministicRng;`.

**Reporting a diagnostic from a system.** When a system notices something wrong — a data-doc naming
a model that is not there, a marker outside the map — report it; never `eprintln!`. The engine drains
the report at the frame boundary onto the `log` channel *and* stderr, **once per `(code, key)` per
session**, so a check that runs every tick prints one line. `key` is the offender's identity (a doc
name, a scene name), so the same code still prints once for each distinct offender. Pick the `L####`
that names the fault — `ludion explain L####` prints the table. A fault with a location or a fix to
suggest goes through `report_full_diagnostic(world, key, diagnostic)` with a whole `Diagnostic`
(`.with_path(..)`, `.with_help(..)`); either way the log line's level follows the code's severity.

```rust
use ludion::prelude::*;

const ASSET_FILE_IO: DiagnosticCode = DiagnosticCode::new("L1101");

fn check_spawners(world: &mut World, _ctx: &SystemCtx) {
    report_diagnostic(world, ASSET_FILE_IO, "mob-den", "the spawner doc names no model file");
}
```

## The determinism rules that bind game code

These make the sim bit-exact and reproducible. The engine's own sim crates enforce them with clippy
denylists; your `clippy.toml` bans the containers, the clock, ad-hoc threads and the `f32`/`f64` transcendental
methods (build failures under `cargo clippy -- -D warnings`). The rest is on you:

- **No `std::HashMap`/`HashSet`** — nondeterministic iteration order. Use
  `ludion_core::collections::{StableMap, StableSet}`.
- **No wall clock** — time is `ctx.dt`/`ctx.frame`/`ctx.time` only.
- **No `thread_rng`** — the seeded `DeterministicRng` resource is the only randomness.
- **Transcendentals through `ludion_core::math`** — plain `f32` arithmetic (and `glam`) is
  deterministic, and so is `sqrt` (IEEE, correctly rounded), but `sin`/`cos`/`exp`/`powf`/… differ
  between Windows and macOS C libraries. Call `ludion_core::math::sin(x)`, not `x.sin()`, on the sim
  path; the game's `clippy.toml` rejects the inherent methods and names the replacement.

Full rationale: `ludion devdocs determinism`.

## Gotchas

- **The schedule is rebuilt from the builder on every scene load / restore.** Systems are engine
  configuration, not world state — so don't cache world state in `static`s or module globals; keep
  all per-run state in components, which hash and travel through snapshot/replay. Game resources ride
  neither the `WorldHash` nor a snapshot (`engine.restore` installs none): a resource is safe only
  when the tick rebuilds it before anything reads it. **A resource read before its writer in the tick
  is cross-frame state and does not survive a restore** — keep such state in a component.
- **`f32` math is deterministic here** for plain arithmetic (glam included) because the engine never
  auto-contracts FMA — but transcendentals are not portable; route them through `ludion_core::math`
  (pure-Rust libm). See the determinism rules above.
- **Entity iteration order is deterministic** (ascending slot index), independent of spawn order — so
  a system that iterates `entities()` or a `query()` sees a fixed order every run.
- **`query` doesn't yield `EntityId`.** Use `world.query_ids` (or `world.entities()` +
  `world.get`/`world.has`) when you need the id (despawn, cross-entity lookups).

## Next

- `ludion docs collision` — colliders and spatial queries.
- `ludion docs recipes` — copyable patterns.
- `ludion docs components` — declaring components, and the built-in visual set.
- `ludion docs verify-loop` — the `cargo test` verification loop.
- `ludion docs game-clock` — freezing gameplay time with `GameClock` while the sim keeps ticking.
- `ludion docs presentation` — entities and systems the sim never sees and the hash never covers.
- `ludion docs debug-draw` — drawing world-space debug lines from a system.
