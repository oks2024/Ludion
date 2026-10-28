---
description: The 3D character motor — CharacterMotor capsule, move_character, MoveReport, platform carry, resize
---

# Character motor

**Scope:** a walking, jumping or crouching 3D character against a physics world: the `CharacterMotor` capsule, `move_character`, the `MoveReport` it returns, platform carry and conveyors, resize and teleport, the `motor` debug draw. The 2D tier's `move_and_slide` is `ludion docs collision`; the bodies the motor walks on and pushes are `ludion docs physics`; trigger volumes are `ludion docs physics-sensors`; paths a platform follows are `ludion docs path`.

The motor moves the character; the **moveset owns gravity, velocity and jump** — pass `move_character` the displacement you want this tick and read the report.

## The loop

1. Author the capsule in the scene (`CharacterMotor` on a root entity, a `physics:` block in the scene) and `ludion check-scene <scene>` — L1294–L1299 below.
2. Call `move_character::<Transform>(world, hero, displacement)` (`ludion_engine::move_character` is the same call plus the `motor` draw) from a game system registered **before** the engine's physics systems; the report is stamped with the frame the physics write pass runs in next. The physics world does not exist until that pass runs, so a world's first tick returns `MotorError::NoPhysicsWorld`.
3. Assert in a game test on a `MoveReport` sequence (`grounded`, `landed`, `stepped`, `platform`), or print the numbers: `ludion docs verify-loop --section "Numeric trace"` (`<game> --trace`, `trace_with` in `cargo test`; run twice and diff).
4. Look: the game binary with `--debug-draw motor` (`ludion docs debug-draw`).

The worked example is `games/character-course`: its `src/lib.rs` controller is the pattern to copy.

## Format

`CharacterMotor`, all lengths in metres. The origin is the **feet**: the capsule body's centre sits `height / 2` above the entity's translation, and the capsule never tilts or yaws with the model. The motor owns its kinematic capsule body; a `Collider` on the same entity is refused (L1295).

| field | default | Clockout 1 m preset | valid |
| --- | --- | --- | --- |
| `radius` | 0.35 | 0.28 | finite, > 0 |
| `height` (feet to crown) | 1.6 | 0.9 | ≥ `2 * radius` + 0.001 |
| `slope_limit_deg` | 50 | 45 | in (0, 90) |
| `step_height` | 0.3 | 0.25 | > 0 and < `height` |
| `snap_distance` (ground snap below the feet) | 0.25 | 0.2 | ≥ 0 |
| `skin_width` (gap kept to what it touches) | 0.01 | 0.01 | > 0 |
| `layer` (from `physics.layers`; omitted = `default`) | none | none | a declared name (L1186) |
| `max_slides` | 4 | 4 | 1..=16 |

The defaults are human scale; a 1 m platformer character overrides the first five as in the preset column. An out-of-range field is L1294 at its JSON Pointer (`/step_height`); the cross-field rules (`step_height < height`, `height ≥ 2 * radius`) are not reachable by the JSON Schema.

`CharacterMotorState` is engine-written (grounded, ground entity, the ground's last pose, a height override, the last carry tick). It rides the world hash and the snapshot; a game never edits it except through the motor functions.

`SurfaceVelocity { velocity: [x, y, z] }` — a conveyor's velocity in m/s in the **collider's local frame** (it turns with the entity). It acts only on a `static` or `kinematic` collider (L1299 otherwise).

## Behaviour

`move_character` runs these in order, then writes the feet (and the rider's yaw) to the pose and the state:

1. **Depenetration** pushes the capsule out of overlap, moving the feet at most `radius` per call. When no clear pose is within that, or four passes do not clear it, the feet stay put and the report has `stuck: true` (crushed; a press never pops the rider through, the game decides): the slide and the platform carry are skipped, `platform` is the default, the grounded/ground/platform record is kept so the next tick carries from where it left off, and the ground probe still runs and can snap the feet.
2. **Collide-and-slide** up to `max_slides` iterations: surfaces within `slope_limit_deg` are walkable; steeper ones are walls for horizontal motion, and creases are handled by sliding along both. Upward motion into a ceiling is removed and reported in `ceiling`.
3. **Step-up**, only when grounded before the call: a wall-class face is climbed (up `step_height`, forward, down) if it lands on walkable ground within `step_height` with headroom for the capsule. An airborne capsule never steps.
4. **Ground probe and snap**: the probe looks `snap_distance + skin_width` below the feet when the motor was grounded before and `displacement.y <= 0`, else `2 * skin_width`. A capsule resting on a ledge rim is grounded by the walkable face under its feet, not the edge's interpolated normal, so rested poses stay bit-stable.
5. **Platform carry**, below.

**`stepped` is the per-tick climb.** A slow walk spreads one step over two or three ticks, each reporting its share; a moveset summing climbs must sum across ticks. It is `Some` only when the feet actually rose that tick.

**Props.** A free (dynamic) prop is pushed by the capsule; a pinned prop blocks like a wall; nothing pushes the character. `move_character_with(…, Some(&mut constraint))` takes a `SlideConstraint` whose `project(feet, step)` reshapes the input displacement before every cast (a lane, a rail, a plane lock). It never shapes the carry.

### Platform carry

A motor that ended its last call grounded on a `static` or `kinematic` support (or another motor) is carried **once per tick, on its first call**: the support's translation and its yaw about its own pivot since the stored pose (read through the derived world pose, so parented platforms work), plus its `SurfaceVelocity` times the gameplay `dt` (zero while a clock hold freezes it). The yaw also turns the rider. The carry is swept like a move: it stops at walls and ceilings and never steps. The surface displacement is swept first against everything, the support's own collider included (a belt's end stop holds the rider); the rigid part follows with the support excluded.

**Why after the slide.** Casts see colliders as of the last step, so the support sits at its previous pose. Sliding and probing in that frame keeps the rider's relation to its support exact; carrying first would put a rider on a descending platform inside the stale collider and depenetration would push it up every tick.

**No carry from `rigid` supports** — their collider is already at this tick's pose, so carrying would double-count; `ground.entity` still names them. Dynamic bodies get no surface velocity. Move the platform **before** the motor's call; a platform moved after it is carried next tick.

## The report

| `MoveReport` field | a moveset reads it for |
| --- | --- |
| `grounded`, `landed` | walk or fall; `landed` = began airborne, ended grounded (the thud, the double-jump reset) |
| `ground` (entity, point, normal, `walkable`, `distance`) | what it stands on; a steep floor is reported with `walkable: false` |
| `ceiling`, `walls` | head bump (zero vertical speed); one wall per entity and plane, sorted by travel distance then entity |
| `stepped` | the climb of this tick (above) |
| `platform` (entity, translation, yaw, surface) | what carried it; default (no entity) when no carry ran or the move was stuck |
| `moved`, `height` | the feet's total movement incl. depenetration and carry; the capsule height used |
| `stuck` | crushed: no clear pose |

`platform` holds the *intended* carry even when a wall blocked it. Takeoff momentum is `(platform.translation + platform.surface) / dt`; the motor never adds momentum itself. Static floors report their entity with zeros. Two calls in one tick carry only on the first; with several motors, **call order is game order**.

## Resize, fits, teleport

- `resize_character(world, hero, height)` → `ResizeReport { height, blocked_by }`. Shrinking always succeeds with the feet planted. Growing is tested first against a skin-lifted capsule at the feet: when something is inside it the height stays and `blocked_by` names the lowest-id blocker. A height the motor would reject (below `2 * radius + 0.001`, non-finite, not above `step_height`) is `MotorError::InvalidHeight`; growing past the authored `height` is allowed.
- `fits_character(world, hero, height)` → `Result<bool, MotorError>` is that test without changing anything. A non-finite overlap query counts as "fits".
- The test reads the last completed step: a ceiling despawned in tick N still blocks in N and is gone in N + 1.
- `teleport_character(world, hero, feet, yaw)` places the feet and yaw **without driving the capsule**: no prop on the way is pushed, and none near the old or new spot is launched. It clears grounded, ground, platform and the carry tick (no carry next tick) and restores the authored height when it fits at the new spot. A sleeping prop beside either spot is woken and shows one gravity step of speed before it settles — a wake, not a launch. A plain `Transform` write is a **move**: it drives the capsule and pushes props (`ludion docs physics` §Teleports for rigid bodies).

## Freshness

Casts and overlaps see the last completed physics step. A press or wall coming at the character is seen one tick late: one tick of overlap, then depenetration, and `stuck` up to a tick late; the platform carry is the explicit exception. Queries skip sensors by default, so the motor never stands on, or reports, a trigger volume (`ludion docs physics-sensors`). The motor's own capsule and the layers its `layer` ignores (`ludion docs collision-layers`) are excluded from its casts.

## Debug draw

Category `motor` (`DebugCategories::parse("motor")`), drawn only by `ludion_engine::move_character`, hash-neutral. Colours are consts in the prelude so a game can match them:

| const | draws |
| --- | --- |
| `MOTOR_GROUNDED` / `MOTOR_AIRBORNE` / `MOTOR_STUCK` | the capsule: green, orange, red |
| `MOTOR_GROUND_NORMAL` | the ground normal at the contact |
| `MOTOR_WALL` / `MOTOR_CEILING` | each wall normal / a cross and ray at the ceiling |
| `MOTOR_CARRY` | the carry vector |
| `MOTOR_PROBE` | the ground probe, `ground_probe_reach` long |

## Diagnostics

| code | raised when |
| --- | --- |
| L1294 | a `CharacterMotor` field is out of range (check-scene, and at runtime for a motor built in Rust) |
| L1295 | a `CharacterMotor` and a `Collider` share an entity (check-scene, runtime) |
| L1296 | a `CharacterMotor` in a scene with no `physics:` block — its capsule is never built (check-scene, once at runtime) |
| L1297 | a `CharacterMotor` entity has a `Parent`; it writes world-space feet (check-scene, once at runtime) |
| L1298 | a `SurfaceVelocity` component is non-finite (the component validator) |
| L1299 | a `SurfaceVelocity` entity has no `Collider`, or a rigid one (check-scene) |
| L1186 | the motor's `layer` is not declared in `physics.layers` |

`MotorError` (`NoMotor`, `NoPose`, `NoPhysicsWorld`, `NonFiniteDisplacement`, `InvalidMotor`, `InvalidHeight`, `NonFiniteTeleport`) are `Err` returns with a message, not codes. `ludion explain L1294` prints the fix.

## Gotchas

- **Registered after physics:** a motor system registered after the engine's physics systems stamps reports with the wrong frame; register it before.
- **A moveset that sums `stepped` per call** under-counts a slow climb; sum across ticks.
- **Not modelled:** carry from `rigid` supports, surface velocity on dynamic bodies, curved belts (straight segments only), airborne step-up, platform pitch and roll (only yaw carries).
- **Reports are unhashed**: `PhysicsWorld::last_move_report` keeps each motor's last one for tools and the trace; the pose and `CharacterMotorState` are the hashed result.

## Next

- `ludion docs physics` — the bodies the motor pushes; `ludion docs physics-sensors` — triggers
- `ludion docs collision` — the 2D `move_and_slide`; `ludion docs path` — moving platforms
- `ludion docs debug-draw`, `ludion docs verify-loop` — seeing and tracing a move
