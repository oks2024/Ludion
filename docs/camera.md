---
description: CameraRig (sim path-rail camera, blends, shake) and CameraOffset (presentation-side offset)
---

# Camera rigs

**Scope:** the `CameraRig` component on a scene's camera entity: the `track` mode (the camera rides a `kind: path` rail after a target), the `first_person` mode (a rigid eye, game-written look), the `chase` and `follow` boom modes, timed blends between configurations, `snap_rig`, trauma shake, `camera_relative` for a stick, the `camera` debug draw, `L1304`–`L1309`, `CameraZone` (rig switching by volume, `L1553`–`L1555`), `camera_occluders`, and the presentation `CameraOffset` (`L1332`–`L1334`). The path document is `ludion docs path`; the gameplay clock is `ludion docs game-clock`; the character the camera follows is `ludion docs motor`.

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

Every field defaults, and every half-life, slack, distance, time and decay is a finite number not below 0 (`L1304`). Modes are externally tagged (`mode: {track: {..}}`, `mode: {first_person: {..}}`, `mode: {chase: {..}}`, `mode: {follow: {..}}`).

## First person

```yaml
CameraRig:
  target: { index: 1, generation: 0 }
  rig:
    mode:
      first_person:
        eye_offset: [0.0, 1.6, 0.0]   # yaw frame: x right, y up, z back (not pitched)
        pitch_limit_deg: 89.0         # in (0, 90)
        enter_pitch_deg: null         # pitch a snap or switch into first_person starts at; null carries it
  look_deg: [0.0, 0.0]                # [yaw, pitch] degrees, game-written, consumed each tick
```

The rig owns yaw and pitch as hashed state; the game writes look deltas into `look_deg` (`add_look(rig, yaw, pitch)` accumulates, `set_look(rig, yaw, pitch)` sets the facing for a spawn or respawn). Yaw 0 looks along −Z and **positive yaw turns left**; **positive pitch looks down**. The rig never reads `InputState`.

- Look applies on every tick that has a target, **held gameplay clock included**: hit-stop must not eat mouse motion. Trauma decay and the shake clock stay on gameplay time. With no target, `look_deg` stays pending.
- Look is unsmoothed. Yaw wraps to (−180, 180]; pitch clamps to ±`pitch_limit_deg`.
- The eye sits rigidly at the target plus `eye_offset` in the yaw frame, every tick. Springs, `look_ahead`, `vertical_anchor` and `focus_offset` are ignored; `fov_y_deg`, `blend_in`, shake and trauma apply. No path is needed.
- A snap or a switch keeps yaw and pitch: switching from `track` carries its look direction and the eye blends over `blend_in`. `enter_pitch_deg: p` starts the pitch at `p` instead (clamped on the next look); the yaw is always carried, so `chase` to `first_person` keeps where the player faces.
- `CameraRig::aim()` returns the posed eye and orientation **before shake**, in every mode (`Aim::forward()`, `Aim::rotation_deg()`): the one source of truth for aim rays. Like `camera_relative` it is one tick behind a game system. A shot that wants the look the player has already input this tick adds the pending `look_deg` to `aim()`'s yaw and pitch (pitch clamped), or accepts the one-tick lag.

```rust
fn look(world: &mut World, _ctx: &SystemCtx) {
    let [dx, dy] = world.resource::<InputState>().unwrap().mouse().delta();
    let cam = camera_entity(world);
    add_look_counts(world.get_mut::<CameraRig>(cam).unwrap().unwrap(), dx, dy);
}
```

Mouse sensitivity has one source: `look_deg_per_count` (default `0.1`, omitted from files at the default, finite per `L1304`). `add_look_counts(rig, dx, dy)` converts raw counts through it (mouse right turns right, mouse down looks down); `look_from_counts(rig, dx, dy)` is the same conversion as a value.

## Chase and follow

A boom from a pivot: the pivot is sprung toward the focus (`target` + `focus_offset`, with `look_ahead` and `vertical_anchor` as in `track`) on `springs.position_half_life`, and the eye hangs rigidly at `distance` from it along yaw and pitch. The angles are **not** sprung (`rotation_half_life` is ignored), so look stays 1:1 and the late latch is exact. Yaw 0 puts the eye on the pivot's +Z side; positive pitch raises it. No path is needed; `ludion_camera::orbit_eye(pivot, yaw, pitch, distance)` is the eye.

```yaml
CameraRig:
  target: { index: 1, generation: 0 }
  rig:
    mode:
      chase:                      # orbit: look turns the boom
        distance: 6.0             # metres, > 0
        pitch_deg: 20.0           # start pitch (positive looks down)
        pitch_min_deg: -30.0      # look's pitch range, each in (-90, 90); min == max fixes the pitch
        pitch_max_deg: 70.0
        yaw_deg: null             # start yaw; null carries the current yaw
        look_pitch: true          # false: look_deg[1] is consumed and dropped
        recenter:                 # off by default: turn back behind the target's motion
          enabled: false
          delay: 1.5              # gameplay seconds without look before it starts
          half_life: 0.6          # seconds for the yaw error to halve
          min_speed: 1.0          # horizontal m/s below which it never turns
      # follow:                   # fixed world yaw, pitch and distance
      #   yaw_deg: 0.0
      #   pitch_deg: 45.0         # in (-90, 90)
      #   distance: 10.0
```

- `chase` takes `look_deg` as `first_person` does (`add_look`, `add_look_counts`): applied on every tick with a target, held clock included, yaw wrapped, pitch clamped to the range. The pivot freezes on a held tick; look still turns the boom.
- `follow` holds its authored angles and **consumes and discards** `look_deg` every tick, so stick input does not pile up in a follow zone and pop on the return to `chase`.
- A snap or switch into `chase` starts the yaw at `yaw_deg`, or carries the current one (from `track`, `first_person`, or 0 on a fresh load), starts the pitch at `pitch_deg`, and rests the pivot on the focus. A spawn facing is `set_look` (its yaw carries). Into `follow`: the authored angles.
- **Recenter:** with `recenter.enabled`, once `delay` gameplay seconds pass with no nonzero look and the target moves at `min_speed` or faster, the yaw damps along the shortest arc toward the heading behind the motion. Any nonzero look restarts the wait; a stopped target never turns the camera. Disabled, it costs nothing and adds nothing to the state.
- Scale a stick by the real `dt` before `add_look` (a stick is a rate, a mouse count is a delta). A late-latching presenter covers `chase` (`ludion docs presentation`).

Runnable: `crates/ludion/examples/chase_camera.rs`.

## Collision

`rig.collision` keeps the eye out of geometry in `chase`, `follow` and `track` (never `first_person`). It casts a sphere from the pivot (the focus, on `track`) to the eye of the blended pose, so a blend out of first person never cuts through the wall the player peered past.

```yaml
rig:
  collision:
    enabled: true
    radius: 0.3              # cast sphere, metres, > 0
    include_layers: []       # physics.layers names the cast hits; empty = every layer
    ignore_layers: []        # names the cast passes through
    min_distance: 0.6        # the shortest boom a hit can pull in to
    skin: 0.05               # metres kept between the hit and the eye sphere
    recover_delay: 0.15      # gameplay seconds the pulled-in boom holds
    recover_half_life: 0.35  # then eases back out
```

- A hit pulls the boom in **on the same tick**, even on a held clock. The hold counts down on gameplay time; then the boom eases back to its full length and the state returns to default. A deeper hit during recovery pulls in at once and restarts the hold.
- The target entity is excluded and sensors are skipped. The target's **child** colliders are not excluded: put them on a layer in `ignore_layers`.
- A cast that starts overlapping retries once from the target. If that overlaps too, the boom holds at `min_distance` and `L1552` is reported.
- The pose written to the `Transform` and `aim()` is the collided one; `rig.state.eye` stays the uncollided eye, and `rig.state.boom` is the collided length (`None` when free).
- Layer names are checked at runtime: an undeclared one is `L1550` and the stage is skipped. A world with no physics is `L1551`. `ludion-camera` itself is physics-free (`RigInput.collision` takes a `RigCollision`); the engine supplies one over `PhysicsWorld::shape_cast`.

## Framing

`CameraRig.framing` is game-written, like `roll_deg`: corner peek, wall-hug, a shoulder offset. It moves the `chase` or `follow` eye **before** collision (track and first person ignore it).

```yaml
framing:
  weight: 1.0                # goal in [0, 1]; the applied weight eases by half_life
  pivot_offset: [0.8, 0.3, 0.0]  # metres in the yaw frame: x right, y up, z back
  yaw_offset_deg: 10.0
  pitch_offset_deg: 0.0
  distance: 4.0              # boom length at full weight; omit to keep the mode's
  half_life: 0.12
```

- At weight 0 the boom is the plain orbit, bit for bit; a returned peek rests at exactly 0 and leaves the state.
- The framed yaw and pitch are the pose's: `aim()` and `yaw_deg()` see the framed view. `rig.state.yaw_deg` is the unframed orbit yaw.
- `camera_relative` ignores the framing yaw while a `chase` or `follow` rig has a nonzero applied weight: it maps through the unframed `rig.state.yaw_deg`, so "forward" stays toward the wall during a peek. At weight 0 it is `camera_relative_yaw(stick, rig.yaw_deg())` bit for bit.

## Kick and roll

Four post-aim channels on `CameraRig`, applied in every mode in the order kick, roll, shake. Defaults are omitted from files and the hash.

```yaml
CameraRig:
  kick: { half_life: 0.08, fov_half_life: 0.15 }  # gameplay seconds to halve; 0 returns at once
  kick_deg: [0.0, -2.0, 0.0]   # [yaw, pitch, roll] impulse, add with add_kick, decays to exactly 0
  fov_kick_deg: 4.0            # add with add_fov_kick, decays like kick_deg
  roll_deg: 8.0                # persistent, game-written (wall run, bank); never decays
  fov_offset_deg: 8.0          # persistent, game-written (sprint widening); never decays
```

- Signs match `look_deg`: positive yaw turns left, positive pitch looks down, so **recoil up is a negative pitch kick**. Positive roll turns the camera's right vector upward.
- Kicks decay on gameplay seconds: a held clock freezes them, yet a kick added during the hold shows at once. `roll_deg` and `fov_offset_deg` apply as written every tick. Kicks are not clamped; a first-person kicked pitch clamps to `pitch_limit_deg`.
- Field of view is `fov_y_deg + fov_offset_deg + fov_kick_deg` clamped to [1, 179]; both channels are inert when the rig sets no `fov_y_deg`.
- A snap keeps the kicks (zero them for a clean respawn). A switch neither blends nor resets them, and a blend never bakes a kick in.
- Recoil is visual: `aim()`, `yaw_deg()` and `camera_relative` never see these channels, so the shot goes where the crosshair was. Use `CameraOffset` for presentation-only effects; use these when the feel must replay.

Runnable: `crates/ludion/examples/first_person_kick.rs`.

## The frame's resolved camera

`ludion_camera::resolved_camera(rig, target, extra_look_deg) -> Option<ResolvedCamera { aim, view }>` is the one seam that turns a rig's last pose into the frame's camera: extra look, then `aim`, then the post-aim stage (kick, roll, FOV channels, shake). `step_rig` returns `resolved_camera(.., [0, 0]).view`; a presenter latching fresher mouse motion calls it with the look accumulated since the tick. The viewmodel and muzzle convergence read it too; the gun itself is `ludion docs viewmodel`.

- `[0, 0]` is the tick's camera bit for bit; `None` before the first pose.
- Extra look `[yaw, pitch]` (degrees, signs as `look_deg`) applies to a `first_person` or `chase` rig only, and not while a blend runs; otherwise it is ignored. Pitch clamps to `pitch_limit_deg` (or the chase range, moving only with `look_pitch`), the eye follows the new angles (about the pivot, for `chase`), and kick, roll, FOV and shake apply on top.
- Extra look equals what `add_look` plus one held tick would give, so a later tick that consumes the same look never pops the view.
- The renderer sees it through `ludion_renderer::CameraLatch`, built by the engine into each present packet for a first-person or chase rig camera when `late_latch` is on (`ludion docs presentation` § Late latch).

## The offset frame (`track`)

The target is projected onto the rail (the nearest point within `search_window` of last tick's); the path's no-roll frame there gives the axes. The desired eye is `point + offset.x * right + offset.y * up + offset.z * back`, **zero roll**: the camera never rolls with the rail (a game-written `roll_deg` still applies post-aim). The eye and the yaw/pitch toward the focus then chase it on critically damped springs. Look-ahead shifts the **focus only**, never the projection. While `grounded` the vertical anchor follows the target's height; airborne it holds until the target leaves `up_slack`/`down_slack`.

## Lights per camera
`Camera.lights: [default, portrait]` chooses which `PointLight`/`SpotLight` layers light the main camera's image (`RenderView.lights` does the same for a view). Absent, it is the camera's own `layers`; an unauthored camera takes every light. See `ludion docs render-views`.

## Zones

A `CameraZone` on a sensor volume swaps the rig's config while the rig's target stands in it. Put it beside a `Collider` with `sensor: true` (`L1554` in `check-scene` otherwise).

```yaml
      Collider: { shape: { box: { half_extents: [4.0, 3.0, 4.0] } }, body: static, sensor: true }
      CameraZone:
        priority: 1            # higher wins where zones overlap
        rig: { mode: { chase: { distance: 8.0 } }, blend_in: { seconds: 0.6 } }
```

- **Membership is the target's origin inside the sensor**, recomputed from physics every tick (a 1 cm sphere probe), not event-driven. A respawn, teleport, restore or load is right by construction. Zone bounds draw in the `colliders` debug category; `camera` draws a cross at the active zone and a line to the target.
- **Priority:** the highest wins, the lowest entity id breaking a tie (`L1555`, once per pair).
- **Base:** entering from no zone remembers the config in force; leaving every zone restores it. Zone to zone blends over the incoming `blend_in`; the base is not touched.
- **A game write to `CameraRig::rig` inside a zone becomes the base** and is never fought; it applies on exit.
- **Snap and respawn:** `snap_rig` or a load with the target inside a zone cuts straight to the zone's `rig`, no blend.
- A live edit to the active zone's `rig` applies at the next zone change. `RigState.zone`/`base` are engine-written and hashed. A world with no `CameraZone` does no query and no work. Without physics, zones are skipped (`L1553`).
- For switch-and-stay (never revert), keep a game component and copy its `RigConfig` into `rig` on `Entered`, as `games/character-course` does (`CameraGate`).

## Occluders

`camera_occluders(world, camera, &layers, radius) -> Result<Vec<EntityId>, PhysicsError>` sphere-casts from the rig's focus to the posed eye (skipping the target) and returns the entities on `layers`, sorted and deduplicated. It is a pure read, one tick behind like `camera_relative`; `Ok(vec![])` with no rig, pose, target or physics; a bad layer name is `Err`. Hiding them stays the game's job: feed the list to a `Fade` with hysteresis (fade in on the tick an entity is listed, hold a few ticks after it leaves), see `crates/ludion/examples/occlusion_fade.rs` and `crates/ludion/examples/camera_zones.rs`.

## Blend and snap

- A write to `rig` that differs from `state.last_applied` starts a blend over the **incoming** `blend_in`, from the frozen current pose. A write mid-blend starts from the blended pose, not the old source.
- `snap_rig(&mut rig)` cuts on the next tick: a global path search, springs, look-ahead and anchor at rest, any blend dropped. Use it for respawns and teleports.
- **Implicit snap = an empty `state.last_applied`.** A scene load and a `SceneSwitch` start without it and so snap; `engine.restore` carries the state and resumes bit-exact (a restore mid-blend included).
- A game switches rigs by copying a `RigConfig` (a zone's, embedded in its own component) into `CameraRig::rig`; `trauma` and `shake` are untouched by the switch. A component embedding one should validate it: `impl SemanticValidate` delegating to `RigConfig::validate_semantics` (pointers relative to the config, `L1304`) and register it with `register_validated`; `ludion_camera::invalid_config` is the same check for Rust-built values.

```rust
let rig = world.component_mut::<CameraRig>(camera).unwrap();
rig.rig = zone.rig.clone();      // blends over zone.rig.blend_in.seconds
add_trauma(rig, 0.4);            // clamped to 1; a non-finite amount is ignored
```

## The clock

The pipeline runs on the **gameplay clock**: a `GameClock::hold` on frame f freezes springs, blend and shake from f+1, and the rig re-poses from the frozen state, so a pause neither drifts nor jumps. Trauma and the kicks decay, and shake advances, on gameplay seconds only. Shake is applied last and never moves `CameraRig::yaw_deg()`.

## Ordering and `camera_relative`

Order per tick: `path_follow` → game systems → physics → `camera_rig` → presentation. The target pose the camera reads is this tick's settled one. A game system runs **before** the rig, so `camera_relative(stick, &rig)` (or `camera_relative_yaw` with a yaw you latched) reads **last tick's** yaw: one tick of latency, and the yaw is pre-shake so a shake never twists the controls (and, under an applied framing peek, pre-framing: see Framing). `y` pushes away from the camera.

The system is installed on every schedule and returns at once in a world without a rig, so a rig spawned at runtime poses its camera like an authored one.

## Debug draw

Category `camera` (`--debug-draw camera`), hash-neutral: the rail polyline, the search window around the projection, the projected point, the desired and actual eye (and the line between), the focus and eye-to-focus line, the anchored height, the look-ahead ray and, while a blend runs, its source eye. `first_person` draws the target, its line to the eye, the eye, a 2 m aim ray and a running blend's source eye; `chase` and `follow` draw the focus, the pivot, the boom to the eye, the eye and a running blend's source eye. While a collision pull-in is active, a red cross at the collided eye and a line to the uncollided eye. A held rig draws nothing.

## Diagnostics

On `L1304`–`L1307` and `L1309` the rig **holds its pose**; on `L1308` the lowest-id rig still runs and the others are ignored. `target: null` and `path: ""` are inert and not reported. `ludion explain L####` has the fix.

| code | raised when |
| --- | --- |
| L1304 | a field is out of range (pointer under `/rig/…` including `/rig/collision/…`, `/framing/…`, `/shake/…`, `/trauma`, `/look_deg`, `/look_deg_per_count`, `/kick`, `/kick_deg`, `/roll_deg`; `pitch_limit_deg` outside (0, 90), a non-finite `eye_offset`, `look_deg` or kick channel) |
| L1305 | `target` names a despawned entity (warning, once) |
| L1306 | the track `path` fails to load (once) |
| L1307 | the entity has no `Camera` |
| L1550 | the collision cast was rejected, e.g. an undeclared layer name (pointer `/rig/collision`; the stage is skipped) |
| L1551 | `collision.enabled` in a world with no physics (warning, once) |
| L1552 | pivot and target both inside geometry; the boom holds at `min_distance` (warning, once) |
| L1553 | `CameraZone` entities in a world with no physics (warning, once) |
| L1554 | a `CameraZone` without a `Collider` with `sensor: true` (`check-scene`) |
| L1555 | the target is in two top-priority zones; the lower id wins (warning, once per pair) |
| L1308 | more than one `CameraRig`; only the lowest entity id runs |
| L1309 | the camera entity has a `Parent` (the rig writes a world-space pose) |
| L1332 | a `CameraOffset` field is non-finite |
| L1333 | a `CameraOffset` on a sim entity (never applied) |
| L1334 | more than one presentation `CameraOffset`; the lowest id applies |
| L1335 | `fov_delta` takes a perspective fov outside 1–179° (warning, once) |

## Presentation offset

`CameraOffset` moves the **rendered** camera from a presentation entity (`presentation: true`, or `spawn_presentation`), so a boot intro can pull in without touching `WorldHash`. It is never hashed or applied to the rig; shake stays `add_trauma`.

```yaml
- name: intro
  presentation: true
  components:
    CameraOffset:
      translation: [0.0, 0.0, 4.0]   # camera-local: +x right, +y up, +z back
      rotation_deg: [0.0, 0.0, 0.0]  # Euler XYZ degrees, composed after the rig's rotation
      fov_delta: -10.0               # degrees added to a perspective fov_y_deg
```

- **Frame:** `t' = t + R * translation`, `R' = R * euler_xyz(rotation_deg)`; a pull-in starts at `+z` and eases to 0. Every field defaults to 0 and is omitted at 0; `CameraOffset: {}` is the identity.
- **`fov_delta`:** the result is clamped to 1–179° (`L1335` warning, once). An orthographic camera ignores it; translation and rotation still apply.
- **Which one applies:** the lowest-id offset on a presentation entity; the rest are `L1334`. One on a sim entity is never applied (`L1333`; `check-scene` and `scene.check` refuse it, and a scene that loads anyway ignores it). A non-finite field is refused in a scene; at runtime the whole offset is ignored and the rig's pose is drawn (`L1332`). With no `Camera` it does nothing; its own `Transform`, `Parent` and `Visibility` are ignored.
- **What sees it:** the frame (screenshot, stream, window, `ludion render`), billboards, culling, shadows, sky, fog, SSR, sort order, present interpolation and `render.bounds` of the scene camera. **What does not:** the rig and the camera `Transform`, `WorldHash`, `camera_relative`, the audio listener, the `camera` debug draw, `camera_ray` and `world_to_pixel` (pure functions of the pose you pass), the `render.view` override and `render.bounds` with `camera: "view"`. Depth of field's `focus_distance` is authored from the rendered eye and does not track the offset.
- **Screen UI anchored with `world_to_pixel`** follows the rig unless the game opts in: pass `offset.applied_to(&transform, &camera)` for the pose the frame is drawn from.
- **Interpolation:** the offset is composed before the present blend, so a changing offset blends smoothly and a snapped tick keeps it. An offset that appears or vanishes between ticks slides over that tick. To pop, put `NoInterpolate` on the offset entity (zero the offset rather than despawn it). A `RenderView` camera interpolates beside the main one and ignores the offset; `NoInterpolate` on its entity cuts that view (`ludion docs render-views`).

Runnable: `crates/ludion/examples/camera_offset.rs`.

## Gotchas

- One rig per world, on a root camera.
- Exactly one `Camera` without a `RenderView` is the main camera (`L2101` counts those). A `Camera` beside a `RenderView` is a secondary view: `ludion docs render-views`.
- `Camera.layers: [name, …]` limits what the main camera draws to the listed render layers (absent draws every layer; `ludion docs render-views`). `Camera` is `Clone`, not `Copy`: clone it where a copy was dereferenced.
- A game defining its own `CameraRig` type must name it explicitly (`use crate::…::CameraRig`): the prelude glob exports the engine's one, and vigil does exactly this.
- Rig state saved in scene text resumes on load; a blend stamped later than the game clock (a mid-blend save reloaded at frame 0) is stale, so the rig snaps rather than freezing.
- The late latch rebuilds the framed boom at the tick's collided length without casting again: the presenter has no physics.
- A snap tick does not advance trauma decay or the shake clock (one tick).
- Gate a rig's pose by `WorldHash` and the camera `Transform`, never by a shaken image on an unpinned GPU.

## Next

- `ludion docs path` — authoring the rail; `ludion docs game-clock` — holds
- `ludion docs lane`, `ludion docs motor` — the character the camera follows and the stick it reads
- `ludion docs debug-draw` — seeing the rig
- `ludion docs sequences` — camera keys that write a rig and restore it
- `examples/gallery/camera-chase.yaml` — a chase boom pulled in by a pillar and a ceiling, two overlapping zones, a `camera_pass` prop that fades and a corner peek, driven by `crates/ludion-testkit/tests/common/mod.rs`: `cargo run -p ludion-testkit --example camera_chase -- --scene examples/gallery/camera-chase.yaml --window --debug-draw camera`
- `games/character-course` — the course as the in-game proof: behind and side rigs, zone sensors, a snap and a shake on respawn, `camera_relative`
