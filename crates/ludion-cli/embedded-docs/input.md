---
description: Keyboard, mouse and gamepad input: edges, move vector, named actions, gestures, cursor aim
---

# Input

**Scope:** Every device read a system makes — keys, mouse, gamepad; edges and levels; "press anything"; the standard move vector; `Action` bindings and the data-driven action map (gestures, axes); aiming from the cursor or a stick. The system contract and the determinism rules are systems; copyable patterns are recipes.

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

`is_held` is level; `was_pressed`/`was_released` are one-frame edges (one sim tick; under a hold, see `ludion docs game-clock`). `Key` is physical and positional, the whole keyboard:
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
  analog value stays readable; this button has no hysteresis (0.5 presses, 0.49 releases). For hysteresis
  bind `trigger: left|right` with `press`/`release` in an action map (below).

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

`Action` is a const-constructible set of keys, gamepad buttons and mouse buttons; games own their
bindings as constants. `Action::new(keys, buttons, mouse)` is the `const` constructor:

```rust
const JUMP: Action = Action::new(&[Key::Space], &[GamepadButton::South], &[]);
// Fire: left mouse or the right trigger.
const FIRE: Action = Action::new(&[], &[GamepadButton::RightTrigger], &[MouseButton::Left]);

if input.action_pressed(&JUMP) { /* jump */ }
```

`action_pressed` and `action_held` fire for any input of the action; `action_released` waits for the
last of them (some input went up this tick and none is still down). `Action` constants are
compile-time; for taps, holds, axes and a data-driven map, use the action map below. Saved bindings
and per-device prompt labels are not in the engine.

## Action maps — named actions, data-driven

A `kind: input-actions` document names what the player means and which physical inputs mean it.
`Engine::builder().input_actions()` opts in: exactly one `*.yaml` under the content root with
`kind: input-actions` (`L1315` otherwise), validated at build, `scene.load` and every rebuild under the
compile codes below, re-pathed `input-actions/<file>/<pointer>`. The engine compiles it into the
`Actions` resource and the `SimDriver` resolves it once per tick. Without the opt-in,
`ActionMap::compile(&doc, rate)` and inserting `Actions` yourself work the same.
`cargo run -p ludion --example input_actions` is the copyable game.

```yaml
gestures:                      # seconds; all optional
  tap_max: 0.22
  hold: 0.35
  double_tap_window: 0.25
actions:
  crouch:                      # a-z 0-9 _
    kind: button
    bindings:
      - key: C
        gesture: tap
      - key: C
        gesture: hold
  fire:
    kind: button
    bindings:
      - mouse: left
      - trigger: right
        press: 0.5
        release: 0.35
  move:
    kind: axis2
    bindings:
      - keys:
          up: W
          down: S
          left: A
          right: D
      - stick: left
        deadzone: 0.2
  look:
    kind: axis2
    bindings:
      - mouse_delta: true
        scale: 0.05
        invert_y: true
      - stick: right
        scale: 3.0
```

```rust
const CROUCH: ActionName = ActionName::new("crouch");
let actions = world.resource::<Actions>().unwrap();
if actions.pressed(CROUCH) { /* a tap or a hold fired this tick */ }
let [x, y] = actions.axis2(ActionName::new("move"));
```

Reads: `pressed` / `held` / `released` (buttons; `released` waits for the last held binding),
`held_ticks` and `hold_progress` (a `hold` binding's ticks down and its fraction of the threshold),
`axis` (`axis1`), `axis2`. An unknown name reads inactive or zero and is reported once through
`take_unknown()`; with the builder opt-in the engine reports each unknown name once as `L4270`.
`Actions` is fresh after `scene.load` and `engine.restore`, and carried by a scene switch and a
checkpoint. A fresh layer **latches** an input already held on its first tick (a trigger already at or
above `press` too): it fires nothing, reads inactive and contributes no `held_ticks` until released, so
after a load or restore a held input needs a fresh press. A binding names one source; the source must fit the action's kind:

| kind | sources |
|---|---|
| `button` | `key`, `mouse`, `pad`, `trigger: left\|right`, `wheel: up\|down` |
| `axis1` | `wheel: x\|y`, `key_pair: [neg, pos]`, `pad_pair: [neg, pos]` |
| `axis2` | `keys: {up,down,left,right}` (1/√2 diagonals), `stick: left\|right`, `mouse_delta: true` |

Modifiers: `gesture` on a button source (not a wheel step); `press`/`release` on `trigger` only (default
0.5 and 0.7 × `press`; a trigger presses at or above `press` and releases below `release`);
`deadzone` on `stick` only (radial, default 0.2); `scale`, `invert_x`, `invert_y` on analog sources
(`invert_x` negates an `axis1` source's one value). An axis action reads its first non-zero binding in
binding order. `context` scopes the duplicate check; `locked` forbids rebinding (below). Compile errors,
all at once with a JSON Pointer: `L1630` zero or several sources, `L1631` source does not fit the kind,
`L1632` modifier does not fit the source, `L1633` value out of range, `L1634` same input and gesture
twice in one context, `L1635` empty bindings or a bad name.

### Gestures

Per physical input, with `n` the ticks it has been down (1 on the press tick): **press** fires on the
down edge and is level while down; **hold** fires on the tick `n == hold` and is level until release;
**tap** fires on release when no hold fired and `n <= tap_max`; **double_tap** fires on a second press
within `double_tap_window` after a tap and is level while held. When an input has a `double_tap`
binding its tap is deferred: it fires `double_tap_window` ticks after the release, unless the second
press cancels it — that latency is the cost of telling the two apart. Thresholds are sim ticks
(seconds rounded half up at the tick rate, minimum 1), never `game_dt`: a `GameClock` hold or slow
motion never stretches one. `Actions::state()` and `restore_state()` carry a mid-gesture state; the
engine's snapshot does not yet, so a restore starts the layer fresh (the latch above).

### Rebinding

All sim-side and in-memory, so a replay reproduces it; every change recompiles the defaults plus the
overrides, and a map that does not compile changes nothing (the errors come back). By action name:
`bindings(name)`, `rebind(name, i, binding)` (`i == len` appends), `unbind`, `reset`, `reset_all`,
`overrides()` (only actions that differ from the defaults). An action keeps at least one binding, and
a `locked: true` action refuses every edit (`L4272`).

**Capture** binds the next press: `capture(name, slot, CaptureOptions::default())` (`button` actions;
slot `Index(i)`, `Keyboard` or `Pad`), then read `capture_result()` — `Pending`, `Bound`, `Unchanged`,
`Conflict { with }`, `Cancelled`. Inputs already held at the call are ignored; two presses in one tick
pick the smaller by `Ord` (keys, mouse buttons, pad buttons); a trigger pull binds `trigger`. `Escape`
(and `cancel_pad`) cancel; `keep_gesture` (default) gives the new binding the replaced one's gesture.
A press another action in the same `context` already has is a `Conflict`: `resolve_conflict` with
`Swap` (the other action takes the replaced binding), `Replace` (it loses its binding) or `Keep`; a
locked other action allows only `Keep`, and nothing may leave an action empty (`L4275`).
**The latch after a binding:** an input newly tracked has no gesture state, so one that is held with no
fresh press is latched — the key just pressed to capture never fires the action until pressed again.

**Overrides** are an `InputBindingsDoc` (`actions: {name: [bindings]}`). `apply_overrides(&doc)` is
lenient, per action, and returns warnings at `/actions/<name>`: `L4273` unknown action, `L4272` locked,
`L4274` does not compile (that action keeps its defaults). `save()` raises a flag and
`take_save_request()` clears it. Codes: `L4272`–`L4276`.

**Saving.** `Engine::builder().persist_input_bindings()` (with `input_actions()`; `ludion::run` sets it)
stores the overrides in `<save_root>/save/input-bindings.yaml` (`format: 1`, `kind: input-bindings`, the
`InputBindingsDoc` body). `actions.save()` writes it at the frame boundary; nothing else does, and a write
failure is a non-fatal `L4278` (read it with `engine.input_bindings_save_error()`). The boot read is
lenient and the game always boots: a bad action is dropped with `L4273`/`L4274` and the rest applies; a
file that cannot be honored (`L1636` envelope, bad YAML, a newer `format`) is renamed to
`input-bindings.yaml.bak.<hash>`, the defaults apply and `engine.boot_warnings()` holds one `L4277`.

The file is read once, at build, as the **overrides at start**; every fresh `Actions` (a build, a load, a
restore) starts from them, a carried one (a scene switch, a checkpoint) keeps its own. A restore after a
mid-session rebind therefore reverts to the overrides at start. Without the switch a test, a replay or
`--check-docs` never reads or writes the file. A recording carries the overrides at start in its header
(`save_docs`, omitted when there are none) and a replay installs those, so it reproduces on a machine whose
file differs or is absent. The example `cargo run -p ludion --example rebind_menu` is a controls screen:
a `UiList` row per action, `capture` on activation, `save` once bound, and a second boot showing the rebind.

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

## Rumble — `Rumble`

```rust
world.resource_mut::<Rumble>().unwrap().play(RumbleEffect { low: 0.8, high: 0.3, duration: 0.18 });
world.resource_mut::<Rumble>().unwrap().stop_all();
```

`low` drives the strong motor, `high` the weak one, `duration` is seconds. Values clamp (motors to
0..=1, `duration` to 0..=2); a request with a zero duration or both motors at 0 is dropped; a non-finite
value counts as 0 and reports `L4279`. `stop_all()` discards the earlier plays of that tick.

**The audio contract.** The outbox drains at the frame boundary to the window host's pad thread. It is
never hashed, snapshotted, recorded or on the wire, and a replay re-issues the requests by re-running
the systems. Headless and in the out-of-process viewer nothing listens, so a game never branches on a pad.

**On the pad.** Overlapping effects take the maximum per motor. A request plays at most 2 s. Rumble stops
when the window loses focus and when the primary pad disconnects. `Engine::set_rumble_intensity(0..=1)`
scales every request (0 drops them); the settings menu binds the player's choice to it.

**Backends.** Windows plays through gilrs force feedback. macOS rumbles with the `apple-rumble` cargo
feature (`ludion = { ..., features = ["apple-rumble"] }`): GameController haptics, the left grip plays
`low` and the right plays `high`, and a pad with one locality takes the louder of the two. Without the
feature macOS is silent (gilrs's macOS force feedback is a stub), and on Windows the feature is inert.
The example `cargo run -p ludion --example rumble` presses a key headless and prints the command.

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
