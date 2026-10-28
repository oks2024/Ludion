---
description: CameraRig — a sim-side camera that rides a kind: path rail after a target, blends, snaps, shakes
---

# Camera rigs

**Scope:** the `CameraRig` component on a scene's camera entity: the `track` mode (the camera rides a `kind: path` rail after a target), timed blends between configurations, `snap_rig`, trauma shake, `camera_relative` for a stick, the `camera` debug draw and `L1304`–`L1310`. The path document is `ludion docs path`; the gameplay clock is `ludion docs game-clock`; the character the camera follows is `ludion docs motor`.

A rig is **sim**: it writes the camera's `Transform` (and the perspective `fov_y_deg`) from a system, so the pose is hashed, replayed and safe to unproject through. That is why a `CameraRig` camera is not presentation (`ludion docs presentation`).

## The loop

1. Author the rail as a `kind: path` and put a `CameraRig` on the `Camera` entity, `target` naming the entity to follow (`ludion docs path`).
2. `ludion check-scene S` reports the load-time codes below.
3. Run it and watch the rig: `cargo run -p ludion-testkit --example camera_rig -- --scene examples/gallery/camera-rig.yaml --window --debug-draw camera` — the worked example (hero on a route, a blend to a side rig at tick 60, a snap at 180, a trauma pulse at 220; the driver is `crates/ludion-testkit/tests/common/mod.rs`).
4. Assert in `cargo test`: `WorldHash` and the camera `Transform` per tick; see `ludion docs verify-loop`.

## Format

```yaml
CameraRig:
  target:                             # entity followed; null holds the pose, not an error (default null)
    index: 3
    generation: 0
  rig:                                # the authored part; a write that differs from state.last_applied blends
    mode:
      track:
        path: paths/camera-track.path.yaml   # content-relative kind: path; "" holds the pose (default "")
        offset:                       # eye in the path frame, metres: +x right, +y up, +z back against the tangent
          - 0.0
          - 2.2
          - 4.5
        search_window: 3.0            # metres of arc length either side of the last projection (default 3.0, > 0)
    focus_offset:                     # look point relative to the target origin (default 0, 0, 0)
      - 0.0
      - 0.0
      - 0.0
    springs:                          # gameplay seconds, 0 is rigid
      position_half_life: 0.15
      rotation_half_life: 0.1
    look_ahead:                       # focus shift toward travel; full_speed > 0
      distance: 1.0
      full_speed: 6.0
      half_life: 0.3
    vertical_anchor:
      up_slack: 1.5
      down_slack: 1.0
      half_life: 0.25
    fov_y_deg: null                   # (0, 180); null leaves the Camera projection alone
    blend_in:
      seconds: 1.0                    # 0 cuts
      curve: ease_in_out              # linear | ease_in_out
  shake:                              # outside `rig`: a switch never blends or resets it
    max_offset:                       # camera-local metres at full strength
      - 0.15
      - 0.15
      - 0.0
    max_angle_deg:                    # pitch, yaw, roll
      - 1.5
      - 1.5
      - 2.0
    frequency: 18.0                   # Hz, > 0
    decay_per_second: 1.2             # trauma lost per gameplay second
    exponent: 2.0                     # strength is trauma^exponent, > 0
    seed: 1
  trauma: 0.0                         # 0..1, add with add_trauma
  grounded: true                      # game-written hint for the vertical anchor
  # state: ...                        # runtime, engine-written; hashed and snapshotted
```

Every field defaults, and every half-life, slack, distance, time and decay is a finite number not below 0 (`L1304`). Only `track` exists as a mode; the externally tagged form (`mode: {track: {..}}`) leaves room for more.

## The offset frame

The target is projected onto the rail (the nearest point within `search_window` of last tick's); the path's no-roll frame there gives the axes. The desired eye is `point + offset.x * right + offset.y * up + offset.z * back`, **zero roll**: the camera never rolls with the rail. The eye and the yaw/pitch toward the focus then chase it on critically damped springs. Look-ahead shifts the **focus only**, never the projection. While `grounded` the vertical anchor follows the target's height; airborne it holds until the target leaves `up_slack`/`down_slack`.

## Blend and snap

- A write to `rig` that differs from `state.last_applied` starts a blend over the **incoming** `blend_in`, from the frozen current pose. A write mid-blend starts from the blended pose, not the old source.
- `snap_rig(&mut rig)` cuts on the next tick: a global path search, springs, look-ahead and anchor at rest, any blend dropped. Use it for respawns and teleports.
- **Implicit snap = an empty `state.last_applied`.** A scene load and `scene.switch` start without it and so snap; `engine.restore` carries the state and resumes bit-exact (a restore mid-blend included).
- A game switches rigs by copying a `RigConfig` (a zone's, embedded in its own component) into `CameraRig::rig`; `trauma` and `shake` are untouched by the switch. A component embedding one should validate it: `impl SemanticValidate` delegating to `RigConfig::validate_semantics` (pointers relative to the config, `L1304`) and register it with `register_validated`; `ludion_camera::invalid_config` is the same check for Rust-built values.

```rust
let rig = world.get_mut::<CameraRig>(camera)?.unwrap();
rig.rig = zone.rig.clone();      // blends over zone.rig.blend_in.seconds
add_trauma(rig, 0.4);            // clamped to 1; a non-finite amount is ignored
```

## The clock

The pipeline runs on the **gameplay clock**: a `GameClock::hold` on frame f freezes springs, blend and shake from f+1, and the rig re-poses from the frozen state, so a pause neither drifts nor jumps. Trauma decays and shake advances on gameplay seconds only. Shake is applied last and never moves `CameraRig::yaw_deg()`.

## Ordering and `camera_relative`

Order per tick: `path_follow` → game systems → physics → `camera_rig` → presentation. The target pose the camera reads is this tick's settled one. A game system runs **before** the rig, so `camera_relative(stick, &rig)` (or `camera_relative_yaw` with a yaw you latched) reads **last tick's** yaw: one tick of latency, and the yaw is pre-shake so a shake never twists the controls. `y` pushes away from the camera.

The system installs only when the world holds a rig at build, restore or scene switch, so a world without one keeps its schedule and hashes. A rig spawned into a world without the system never poses the camera: `L1310`, once per rig; author it in the scene file.

## Debug draw

Category `camera` (`--debug-draw camera`), hash-neutral: the rail polyline, the search window around the projection, the projected point, the desired and actual eye (and the line between), the focus and eye-to-focus line, the anchored height, the look-ahead ray and, while a blend runs, its source eye. A held rig draws nothing.

## Diagnostics

On each of `L1304`–`L1309` the rig **holds its pose**. `target: null` and `path: ""` are inert and not reported. `ludion explain L####` has the fix.

| code | raised when |
| --- | --- |
| L1304 | a field is out of range (pointer under `/rig/…`, `/shake/…`, `/trauma`) |
| L1305 | `target` names a despawned entity (warning, once) |
| L1306 | the track `path` fails to load (once) |
| L1307 | the entity has no `Camera` |
| L1308 | more than one `CameraRig`; only the lowest entity id runs |
| L1309 | the camera entity has a `Parent` (the rig writes a world-space pose) |
| L1310 | a rig exists in a world whose schedule lacks `camera_rig` |

## Gotchas

- One rig per world, on a root camera.
- A game defining its own `CameraRig` type must name it explicitly (`use crate::…::CameraRig`): the prelude glob exports the engine's one, and vigil does exactly this.
- Rig state saved in scene text resumes on load; a blend stamped later than the game clock (a mid-blend save reloaded at frame 0) is stale, so the rig snaps rather than freezing.
- A snap tick does not advance trauma decay or the shake clock (one tick).
- Gate a rig's pose by `WorldHash` and the camera `Transform`, never by a shaken image on an unpinned GPU.

## Next

- `ludion docs path` — authoring the rail; `ludion docs game-clock` — holds
- `ludion docs lane`, `ludion docs motor` — the character the camera follows and the stick it reads
- `ludion docs debug-draw` — seeing the rig
- `games/character-course` — the course as the in-game proof: behind and side rigs, zone sensors, a snap and a shake on respawn, `camera_relative`
