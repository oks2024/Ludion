---
description: Keyboard, mouse and gamepad input: edges, press-anything, move vector, actions, cursor aim
---

# Input

**Scope:** Every device read a system makes — keys, mouse, gamepad; edges and levels; "press anything"; the standard move vector; `Action` bindings; aiming from the cursor or a stick. The system contract and the determinism rules are systems; copyable patterns are recipes.

## Keys — only through `InputState`

Read the resource; do not touch OS events.

```rust
fn steer(world: &mut World, _ctx: &SystemCtx) {
    let input = world.resource::<InputState>().unwrap();
    let left = input.is_held(Key::Left);       // level: currently down
    let fired = input.was_pressed(Key::Space); // edge: went down this frame
    // ... then mutate components in a separate query (the borrow of `input` ends first).
    let _ = (left, fired);
}
```

`is_held` is level; `was_pressed`/`was_released` are one-frame edges (one sim tick; under a hold, see `ludion docs game-clock`). `Key` variants games use:
`A`–`Z`, `Digit0`–`Digit9`, `Up`/`Down`/`Left`/`Right`, `Space`, `Enter`, `Escape`, `Shift`, `Ctrl`, `Alt` (folded —
left and right are one key, held while either is down; Cmd/Super is not mapped).

## Mouse — `input.mouse()`

`MouseState` carries the same level/edge vocabulary:

- `input.mouse().position()` — `[f32; 2]` in **render-target pixels, origin top-left, +Y down**
  (level: it persists until the next move). This is the pixel space the aim recipe below unprojects.
- `input.mouse().is_held(MouseButton::Left)`, `was_pressed`, `was_released` — buttons, exactly like
  keys (`MouseButton` is `Left` / `Right` / `Middle`).
- `input.mouse().wheel()` — `[f32; 2]`, this frame's accumulated scroll (an edge-like accumulator:
  it sums every wheel motion during the tick and resets to zero at the next tick).
- `input.mouse().delta()` — `[f32; 2]`, this tick's raw mouse motion in device counts (unscaled,
  unbounded — not clamped to the window like `position`); accumulates like `wheel` and resets each
  tick. Scale for sensitivity yourself. Wire form: `{"kind": "mouse_motion", "delta_bits": [<u32>, <u32>]}`.

## Mouse look

- `delta()` is raw device counts; the game scales them (sensitivity, invert, smoothing).
- `.cursor(CursorMode::Locked)` starts the window locked; a system switches at runtime with the
  `CursorLock` resource's `lock()` / `release()` (pause menu, death screen).
- `position()` freezes while locked; `delta()` keeps flowing.
- Focus loss always frees the pointer; focus regain re-locks it with no click. Motion only arrives
  while the window is focused.
- macOS and Windows both use the `Locked` grab: the host centres the pointer, then locks it there
  (relative mode on macOS, a 1×1 clip on Windows); raw motion arrives from device events. Where
  `Locked` is unsupported the pointer is only hidden.
- Headless: a no-op. Only the in-process window (`run --window`, `launch`) applies the lock; the
  attach viewer and the editor do not.

## Gamepad — `input.gamepad()`

Player one's pad (the capture layer forwards the first connected pad only; when it unplugs, the
next connected pad becomes player one). Sticks and triggers are **level analog**; buttons are edges
like keys.

- `input.gamepad().left_stick()` / `.right_stick()` — `[f32; 2]`, each axis in `[-1, 1]`. **+Y is
  up** (the capture layer normalizes to that convention), so pushing a stick up gives `+y` — most
  ground-plane games map that to "away from the camera" (world `−Z`), i.e. use `-y`.
- `input.gamepad().left_trigger()` / `.right_trigger()` — `f32` in `[0, 1]`.
- `input.gamepad().is_held(GamepadButton::South)`, `was_pressed`, `was_released` — buttons.
  `GamepadButton` names faces by **position** (`South`/`East`/`West`/`North` — Xbox A/B/X/Y,
  PlayStation ✕/○/▢/△), plus `LeftShoulder`/`RightShoulder`, `LeftStick`/`RightStick` (clicks),
  `Start`/`Select`, and `DpadUp`/`DpadDown`/`DpadLeft`/`DpadRight`, and `LeftTrigger`/`RightTrigger`.
- Each trigger is also a button, held while its value is `>= TRIGGER_THRESHOLD` (0.5): so
  `was_pressed(GamepadButton::RightTrigger)` is a trigger pull and it can go in an `Action`. The
  analog value stays readable; there is no hysteresis (0.5 presses, 0.49 releases).

## "Press anything" — `input.any_pressed()`

True on the tick any key, mouse button or gamepad button goes down — one tick per press, like an
edge. Every key counts (`Escape` and the modifiers too); the cursor, the wheel and the sticks never
do; a trigger counts once pulled past `TRIGGER_THRESHOLD`. Add the analog test yourself, with your own threshold:

```rust
if input.any_pressed() || input.gamepad().any_axis_beyond(0.5) {
    skip_intro();
}
```

Levels, for latching on one device: `input.any_key_held()`, `input.mouse().any_held()`,
`input.gamepad().any_held()` (buttons only). `InputState::is_idle` is not this test: it is the
snapshot-omission check and counts held keys, analogs and the cursor position.

## Move vector — `input.move_vector()`

Arrows, WASD and the D-pad win over the left stick; with none of them down the result is the left
stick through a radial deadzone of `STICK_DEADZONE` (0.2), rescaled so it ramps from 0 at the edge to
1 at full tilt. Digital input reads as −1/0/+1 per axis (opposites cancel) and two axes at once are
scaled by 1/√2, so the length never exceeds 1. `+x` is right and `+y` is up/forward (`W` and
stick-up). A game wanting full speed on any deflection normalises it itself. It feeds
`camera_relative*` and `lane_relative` (`ludion docs camera`, `ludion docs lane`).

For your own deadzone, `input.gamepad().left_stick_dz(dz)` / `right_stick_dz(dz)`, with `dz` in `[0, 1)`.

## Actions — keys or buttons under one name

`Action` is a const-constructible set of keys and gamepad buttons; games own their bindings as constants:

```rust
const JUMP: Action = Action { keys: &[Key::Space], buttons: &[GamepadButton::South] };

if input.action_pressed(&JUMP) { /* jump */ }
```

`action_pressed` and `action_held` fire for any input of the action; `action_released` waits for the
last of them (some input went up this tick and none is still down). Rebinding, saved bindings and
per-device prompt labels are out of scope.

## Headless is quiescent, not broken

Capture lives out-of-sim in the window (`--window`) and the
editor; a plain `ludion serve` / a headless test has nothing feeding it, so `InputState` stays at its
default — mouse at `[0, 0]`, no buttons, zero sticks. A game reading input headless simply sees no
action. That is the correct degrade, and it is what every golden/CI run relies on. Drive input in a
test by scripting it (an `InputScript`, or `input.inject` over the wire) — the same data the window
records, so a test and a live session run the identical path.

## Focus loss and unplug

When the window loses focus it releases every key and mouse button it had forwarded as down, so
an Alt-Tab mid-hold never leaves the hero walking; a key still held on return resumes on its next
real press. When player one's pad unplugs, the capture layer zeroes every non-zero axis (the trigger
buttons release with their axes) and releases held buttons. These releases are ordinary recorded
`input.inject` events, so a replay reproduces them. A game cannot yet *see* focus or pad connection
(no pause-on-focus-loss signal); a game that needs one would get it as new recorded events.

## Aim recipe — cursor/stick to the ground plane

To point something at where the cursor (or a
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
the horizon) — handle both as "no aim this frame." For a **twin-stick** game, take
`right_stick_dz(STICK_DEADZONE)` directly when it is non-zero and fall back to the cursor otherwise; both feed the
same `yaw`. All the trig is `math::atan2` (not `f32::atan2`) because the result becomes hashed sim
state — the aim must be bit-identical across OSes. The complete, runnable version is the
`cursor_aimed_turret` example (`cargo run -p ludion --example cursor_aimed_turret`).

Analog input rarely needs hand-authoring: the window records mouse/stick values for you, and on the
wire/replay each analog field travels as its exact IEEE-754 bit pattern under a `*_bits` name (e.g.
`{"kind": "gamepad_axis", "axis": "left_stick_x", "value_bits": 1056964608}`) so a replayed aim is
bit-exact. Keys and buttons stay fully symbolic and hand-writable
(`{"kind": "key_down", "key": "Space"}`, `{"kind": "mouse_down", "button": "left"}`).

## Next

- `ludion docs systems` — the system contract, time, randomness, determinism rules.
- `ludion docs recipes` — copyable patterns.
- `ludion docs camera` / `ludion docs lane` — feeding `move_vector` to a camera-relative or lane-relative mover.
