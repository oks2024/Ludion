# Plan: Character controller and camera rigs — a 3D motor, paths and configurable cameras

> Status: **DESIGN SETTLED (2026-09-29, grilling session; every ruling is the user's). W1–W2 mechanized; W3+ not started.**
> W2 holds the motor (`CharacterMotor`, `move_character`, `MoveReport`), `SurfaceVelocity`, resize and
> teleport, platform carry, the numeric trace and the course (`ludion docs motor`, `games/character-course`).
> W1 holds mesh colliders, `kind: path` with its sampler queries, `PathFollower` (motion readout, wrap
> teleports, `orient`) and world-space debug draw including the `colliders` category and `axes`. The
> external platformer's first engine request drove its last additions, and its second shipped sensor
> colliders with enter/exit events (`docs physics-sensors`). The driver is the next test game, a 3D platformer in the style of Crash Bandicoot. Its GDD and name
> come first, and its gameplay gets its own plan. This plan is the engine half of "player feel": the
> character motor, level collision from meshes, paths, and the camera rigs. Everything here must stay
> **genre-agnostic**. The platformer drives it but must not be the only thing it fits. It runs beside
> `Plan/animation-clips.md` and depends on it only in W5. Every claim of current-code fact below was
> read from the tree on 2026-09-29, so re-grep before editing.

## Trigger

The platformer needs a character that runs, jumps and lands on uneven, moving 3D ground, and a camera
that frames it. `Plan/physics.md` left this open ("a **3D** kinematic character controller has no
answer in this plan … decided when a slice asks for one"). The platformer asks.

## What exists (2026-09-29)

- **Planar movement only:**
  - `ludion-spatial::move_and_slide` (`crates/ludion-spatial/src/spatial.rs`) is 2D on the `Plane`
    resource's axes, resolved against static colliders.
  - It tries the full delta, then per axis, then zero.
  - It has no sweep, no depenetration and no vertical axis.
  - No game or example has gravity, jumping, grounding, slopes or steps.
- **`ludion-physics`** (rapier3d `=0.35.3`, enhanced determinism):
  - **Bodies:** kinematic bodies are position-based. The game writes `Transform`, and the write pass
    calls `set_next_kinematic_position`.
  - **Queries:** `ray_cast`, `shape_cast` (every hit, sorted, with normals) and `overlap` take a
    `QueryFilter` (`.exclude(entity)`, `.ignore_layer(name)`) and see the state of the last completed
    step.
  - **Other:** sensor colliders shipped with enter/exit events (`docs/physics-sensors.md`). Rapier's `KinematicCharacterController` is not wrapped.
- **Collider shapes** (`crates/ludion-spatial/src/collider.rs`):
  - 2D: `circle`, `aabb`.
  - 3D: `sphere`, `box`, `capsule`, `cylinder`, hand-pointed `convex`, `heightfield`, `compound`.
  - **No mesh collider**, and no convex-hull generator. Render meshes are never colliders.
- **Camera:**
  - The engine `Camera` is projection, near and far only.
  - Games pose it themselves: vigil's `follow_camera` (`games/vigil/src/hud.rs`) has a fixed pitch,
    look-ahead and deterministic shake, and the external game has a `FollowCamera` with a per-tick lerp.
- **Input:**
  - `InputState` has keys, the mouse and one gamepad, with analog sticks recorded bit-exact in
    replays.
  - There are no action maps; games map raw input.
  - The iOS touch mapper turns a `touch-layout` into gamepad events.
- **Hierarchy:** `Parent` is a link only, with no transform propagation. Nothing supports riding a
  platform.
- **Debug drawing:** there is no world-space debug-draw primitive. The renderer's only overlay is
  screen-space UI, and editor gizmos are client-side.
- **Conventions:** Y-up, 1 u = 1 m, 60 Hz default tick.

## Decisions

1. **Scope: controller plus camera.** The motor, the camera rigs, and what they stand on (mesh
   colliders and paths) are in scope. The platformer's gameplay (crates, enemies, checkpoints) and the
   game crate itself are not. **Why:** stick input is camera-relative, so a feel sitting judges the
   two together. Gameplay has a different exit.
2. **The engine ships a motor; game Rust owns the moveset.**
   - The engine provides collide-and-slide for a capsule with grounding and platform riding.
   - The game provides jump arcs, spin, slide, coyote time and buffering, with tunables in declarative
     text.
   - The platformer's v1 moveset is run, variable-height jump, fall and land, spin, and slide with
     slide-jump. It is written in the game crate once the GDD exists, and its feel details are
     settled there.
   - **Why:** the same line as animation-clips decision 4. Enemies and other games reuse the motor. A
     data-configured moveset drifts toward scripting-by-config.
3. **Our own motor over `shape_cast`, not rapier's `KinematicCharacterController`.** **Why:** a
   platformer's feel lives in the motor's edge cases (ledge snapping, steps, slope sliding), which we
   must tune and test ourselves. Our queries are already cross-arch verified. It keeps rapier
   replaceable and avoids a second rapier surface to audit. This answers `Plan/physics.md`'s accepted
   gap.
4. **Motor API: a function plus a config component.**
   - A `CharacterMotor` component holds the capsule and tunables (slope limit, step height, snap
     distance, skin width, the collision layer).
   - Game Rust calls `move_character(world, entity, displacement) -> MoveReport`.
   - The report carries grounded, ground normal and entity, ceiling hit, wall hits, and the platform
     delta applied.
   - Gravity and velocity are the moveset's, not the motor's.
   - **Why:** the moveset needs the report in the same tick (landed means switch state). A function
     keeps ordering in game Rust, the `move_and_slide` and `crossfade` idiom.
5. **The character is a kinematic capsule body in rapier.** The motor excludes itself from its own
   casts. Other queries (line of sight, projectiles) hit it, and it pushes dynamic props (rapier's
   kinematic behavior). Dynamic bodies never push it. **Why:** one world answers every query, and
   one-way pushing keeps the motor the sole author of its own position.
6. **Motor v1 behaviors:**
   - wall slide and depenetration (the baseline);
   - ground probe and snap-down;
   - slope limit with slide-off;
   - step-up;
   - moving-platform riding (the ground entity's per-tick translation and yaw, with no `Parent`
     propagation);
   - a ceiling bump reported to the moveset.

   Amendments from the external platformer's request (shipped in W2): `resize_character` and
   `fits_character` (growth is headroom-tested; `fits_character` returns `Result`); `SurfaceVelocity`
   for conveyors; `teleport_character`; the report gains `stepped`, `platform` (incl. `yaw` and
   `surface`), `moved`, `stuck` and `height`; a platform's yaw also turns the rider. Settled: the carry
   is applied after the slide and ground probe (casts see the support at its last-step pose); no carry
   from `rigid` supports; the engine defaults are human scale (the 1 m preset is authored).
7. **`move_and_slide` stays, split by domain.** `move_and_slide` serves planar games without a physics
   world (vigil moves hundreds of enemies through it), and the motor serves 3D. Each crate's docs point
   to the other. **Why:** these are different domains, not two ways to do one task, so each domain
   still has one obvious way.
8. **A mesh collider:** `Collider.shape: { mesh: <path> }` over any `kind: mesh` document (imported,
   sculpted or derived).
   - It is allowed on static and kinematic bodies only. On a `rigid` body it is a diagnostic pointing
     at `convex` and `compound`.
   - Rapier's internal-edge fix is enabled.
   - **By convention** the level's bpy script emits a separate simplified collision mesh. Using the
     render mesh directly is allowed (greybox).
   - **Why:** sculpted corridors can't be expressed with primitives. A generic mesh reference limits no
     genre. The split is an authoring convention, not an importer naming rule.
9. **`kind: path`: a generic spline document.**
   - It stores centripetal Catmull-Rom (plus `linear`) through control points, open or closed, with
     arc-length parameterization and a pure sampler.
   - It is hand-authored, or emitted by a bpy script from a Blender curve. An emitted path carries
     `generated_from`, a provenance key the editor reads; the engine has no drift gate for it (it cannot
     run Blender).
   - It has three consumers: the track rig, `PathFollower` and game patrols.
   - **Why:** points on the curve are readable and editable as text (North Star), and centripetal
     avoids cusps. Blender's off-curve handles buy nothing for a camera or platform.
10. **`PathFollower`: an engine component and system.**
    - Its fields are the path, speed, mode (`loop`, `ping_pong`, `once`), phase and `orient` (`none`,
      `yaw` or `frame`, written in the parent's frame).
    - It reads out each tick's motion (`s`, direction, delta, velocity, teleported), unhashed.
    - An open-loop wrap, or a posed move beyond the follower's travel plus a slack, is a teleport: the
      kinematic body snaps with zero velocity and launches no rider.
    - It moves kinematic bodies and runs **before** game systems, so a rider's motor sees this tick's
      platform delta.
    - A game may drive the parameter itself.
    - **Why:** platform-before-rider ordering is a subtle contract the engine owns once.
11. **Camera rigs are engine data, sim-side and hashed.**
    - A `CameraRig` component, posed into the scene `Camera` by one engine system.
    - Its tunables live in scene text, and games switch or nudge it by writing the component.
    - **Why:** the user asked for cameras that are changeable and configurable across genres. The rig is
      sim state because camera yaw feeds camera-relative input. Render may interpolate presentation
      only.
12. **The rig is a staged pipeline with a `manual` mode.**
    - A *mode* computes a desired pose. v1 modes are `chase` (orbit, recentering behind motion, pitch
      limits), `track`, `follow` (fixed offset and angle) and `manual` (the game writes the desired
      pose).
    - Shared stages apply to every mode:
      - critically-damped springs (position per axis and rotation, with half-life tunables replacing
        lerps; the "slows down progressively" ask);
      - look-ahead along velocity;
      - vertical anchoring (the camera's Y follows the ground on landing, not every jump);
      - sphere-cast occlusion pull-in (chase);
      - blend;
      - trauma shake (deterministic, seeded).
    - Every mode exposes its target (entity or point) and offsets as fields.
    - An engine helper `camera_relative(stick, &rig)` maps input to world directions.
    - **Why:** a game shouldn't recode the common case, and shouldn't recode everything because the rig
      is too stiff. `manual` gives arbitrary Rust control and still gets the shared stages, without a
      mode registry.
13. **Game code switches rigs; the engine blends.** Writing a new rig configuration starts a timed
    blend from the current pose (the `crossfade` shape). *When* to switch is game Rust, for example an
    typically on a sensor's `Entered`/`Exited` event (`sensor_events`). **Why:** trigger conditions differ per game, and a zone
    component can be promoted later on repetition.
14. **The track rig projects the target onto one path.**
    - It uses the sampler's windowed `nearest_s` around last tick's parameter, so loops and switchbacks
      don't jump. `nearest_s`, `nearest_s_global` and `frame_at` are pure queries in W1's sampler in
      `ludion-scene`, shared with game patrols and lane locks.
    - The camera sits at the path point plus an offset in the path's frame, looking at the target.
    - Behind, in-front (boulder chase) and side views are all offsets.
15. **The game writes the orbit delta.** `CameraRig` has yaw and pitch input fields that the game fills
    from the stick, mouse, touch or AI. The rig never reads `InputState`. **Why:** there are no action
    maps, so input mapping is the game's (phase-13). Hard-wiring the stick is the stiffness to avoid.
16. **vigil migrates onto the `follow` rig in this plan.** Its look-ahead and shake move into the
    shared stages, and its hand-kept camera is deleted. Its replays re-record last, after the schema
    snapshot. **Why:** a second, genre-different consumer is the proof that the rig is generic.
17. **World-space debug draw.**
    - A render-only resource, cleared each tick and **never hashed, saved or recorded**.
    - Game Rust calls `debug_draw(world).line(..)`, `.capsule(..)`, `.path(..)`, `.axes(..)` and
      similar under a named category, and the motor, rig and paths emit their own categories.
    - The `colliders` category is drawn host-side at extraction from the live physics world, so it costs
      nothing when off.
    - The categories are toggled by a host-state wire method, `render.debug` (the 31st, shaped like
      `render.view`), and on the CLI by `render --debug-draw <categories>`.
    - **Why:** a single screenshot then explains itself, and an agent debugging a moveset draws its own
      vectors instead of printing.
18. **The agent verification surface:**
    - scripted-input scenario tests (a stick and button sequence asserted against a `MoveReport`
      sequence);
    - a per-tick numeric motor, rig and path trace as JSON from the CLI;
    - debug draw;
    - the course's hash trace, `.lrp` and golden.
19. **Editor scope:**
    - a path overlay with control-point dragging (canonical `kind: path` edits);
    - the debug-draw toggle;
    - live rig tunables through the component inspector.
    All of it rides existing edit commands plus `render.debug`.
20. **Crate placement:**
    - The motor is a module of `ludion-physics`, which stays the only crate naming rapier.
    - The rigs go in a new sim-side `ludion-camera` crate (depending on `ludion-physics` for the
      occlusion cast).
    - `kind: path` and its sampler go in `ludion-scene`.
    - Debug draw is an `ludion-engine` resource drawn by a renderer pass.
    - **Why:** small orthogonal crates. Games with no character use the camera.
21. **Proof vehicle:** `examples/character-course`.
    - A greybox course: a Blender-scripted trimesh with slopes past the limit, steps, gaps, a platform
      on a path, a track section and a chase section.
    - A run/jump **reference controller** drives it; it is the runnable example for motor and rigs.
    - The game crate starts after its GDD; it copies the pattern, not the code.
22. **Sequencing and input:**
    - This plan runs parallel to animation-clips; only W5 waits for that plan's W2.
    - The reference controller reads gamepad and keyboard. Touch comes free through the mapper and is
      not scheduled.
23. **Queries skip sensors unless the filter opts in** (`QueryFilter::include_sensors`) — the motor
    never stands on, steps onto or snaps to a sensor, and `MoveReport` never lists one.

## The formats (sketch; exact key names settle in the wave that builds them)

A path:

```yaml
format: 1
kind: path
interpolation: catmull_rom   # or linear
closed: false
points:
  - [0.0, 0.0, 0.0]          # block style if the strict subset refuses flow sequences here
  - [4.0, 0.5, -6.0]
  - [9.0, 1.0, -8.0]
```

A camera rig and a motor on scene entities:

```yaml
CameraRig:
  mode: { track: { path: course/section-1.path.yaml, offset: [0.0, 3.0, 6.0] } }
  target: hero
  springs: { position_half_life: 0.15, rotation_half_life: 0.1 }
  look_ahead: 1.5
  vertical_anchor: true
CharacterMotor:
  radius: 0.35
  height: 1.6
  slope_limit_deg: 50.0
  step_height: 0.3
  snap_distance: 0.25
```

## Waves

### W1 — Foundations: mesh colliders, paths, platforms, debug draw

1. `Collider.shape: { mesh }` in `ludion-spatial`'s shape set and `ludion-physics`'s builder.
   - Static and kinematic bodies only, with the `rigid` diagnostic.
   - The internal-edge fix.
   - A load, restore or switch rebuilds an edited mesh by content hash; `asset.reload` does not, for
     replay safety.
2. `kind: path` in `ludion-scene`: the reader, the canonical writer, validation (at least two points,
   finite values, no coincident neighbors), the centripetal sampler and the arc-length table, plus
   the continuous `tangent_at`, `frame_at` (a no-roll frame) and `nearest_s` / `nearest_s_global`
   projections.
3. `PathFollower`: the component and a system registered before game systems. Its test asserts a
   rider sees the same tick's delta. It also carries the motion readout, wrap teleports and `orient`
   (decision 10).
4. Debug draw: the resource, the game API (with `axes`), the categories (with host-side `colliders`),
   the renderer pass, `render.debug`,
   `render --debug-draw`, and the schema snapshot. A hash-neutrality test runs with debug draw on and
   off.
5. Docs: the collider topic (mesh shape), a path topic, and the debug-draw topic.

### W2 — The motor and the course (mechanized)

The course lives in `games/character-course`, not `examples/` (decision 21 amended: a motor needs a
game crate, and the tooling knows `games/*`).

1. `CharacterMotor`, `move_character` and `MoveReport` in `ludion-physics` (decisions 4 to 6).
2. **Scenario tests** (headless, scripted input), each asserting a `MoveReport` sequence:
   - land on flat ground;
   - walk down a slope staying grounded;
   - slide off one past the limit;
   - climb a step, and refuse one too tall;
   - bump a ceiling;
   - ride a translating and a yawing platform;
   - a dynamic body never pushes the character;
   - push a prop;
   - thread-count invariance.
3. The numeric trace: per-tick position, grounded, ground normal and entity, and platform delta, as
   JSON.
4. `examples/character-course`:
   - the bpy script that emits the course's render and collision meshes and its paths;
   - the run/jump reference controller;
   - a trace, a `.lrp` and a golden blessed on the 3090 (the M4 re-bless goes to `owed.md`).
5. Docs: the motor topic, and the `move_and_slide` ↔ motor pointers (decision 7).

### W3 — Camera rigs and vigil

1. `ludion-camera`: `CameraRig`, the staged pipeline, the `chase`, `track`, `follow` and `manual`
   modes, the shared stages (decision 12), blend (decision 13), and `camera_relative`.
2. Tests:
   - spring convergence;
   - track projection continuity on a switchback and a closed loop;
   - occlusion pull-in against a wall;
   - blend endpoints;
   - shake determinism.
   The rig joins the numeric trace.
3. The course gains a track section, a chase section and a switch between them.
4. **vigil migrates** to `follow` (decision 16). Its replays re-record last.
5. Docs: the camera topic.

### W4 — Editor

1. Path overlay and control-point dragging (writes canonical `kind: path` text).
2. The debug-draw category toggle over `render.debug`.
3. Rig tunables live through the inspector.
4. The live smoke goes to `owed.md`.

### W5 — Animation hookup (gated on animation-clips W2)

1. The reference controller drives the mannequin: idle, run and jump through `crossfade`, chosen from
   `MoveReport` and speed.
2. The course's `.lrp` and golden re-record if the pose changes them.

### Exit (human gate, in `owed.md`)

One sitting (about 30 minutes):

- motor feel on the course (slopes, steps, platforms, snap) and each camera mode;
- vigil's migrated camera, at least as good as before;
- the editor live smoke;
- the mannequin's locomotion look (crossfades and foot sliding at run speed).

The verdict is recorded as a decision. Close: the root `CLAUDE.md` line, and this plan's Status
collapsed.

## Non-goals

- **Per-triangle surface tags** (ice, footsteps, kill surfaces). **Trigger:** a moveset or audio need.
  Kill zones are sensor volumes.
- **Motor modes beyond ground and air** (swimming, ladders, ledge grab, wall jump). **Trigger:** a GDD
  asks. New modes extend `MoveReport`.
- **Dynamic bodies pushing the character.** **Trigger:** a moveset needs knockback from physics.
- **A ledge or edge query.** **Trigger:** an edge animation or enemy AI asks.
- **Tilting platforms** (pitch and roll riding). **Trigger:** a level needs one.
- **More rig variants:** a framing dead zone, a static look-at rig, a guide-plus-camera track.
  **Trigger:** a level or game the shipped modes plus `manual` can't serve without repetition.
- **Engine camera zones.** **Trigger:** a second game repeats the zone pattern.
- **Convex-hull generation** for rigid-body meshes (unchanged from `Plan/physics.md`).
- **Touch input** for the reference controller. **Trigger:** iOS M3 resumes.
- **Wrapping rapier's `KinematicCharacterController`** (decision 3).
- **Splitting the 2D and 3D collision domains.** One `Collider` carries both 2D and 3D shapes, and a
  claims rule routes them to `ludion-spatial` or `ludion-physics`. Rapier's `space: planar` is a
  second planar option. A later plan decides between separate components per dimension and retiring
  the 2D collision in favor of rapier's planar space (only if vigil's crowd stays cheap). **Trigger:**
  an agent or a game trips over collider routing, or the next API freeze.

## Risks

- **Query staleness.** Queries see the last completed step, while `PathFollower` and game code move
  kinematic bodies this tick. The motor must apply a platform's delta explicitly and not rely on a
  cast to see the platform's new pose. The riding tests pin this down.
- **Trimesh ghost contacts.** A capsule sliding across triangle seams can catch internal edges. The
  internal-edge fix and a seam-crossing scenario test are the guard.
- **Camera tuning moves hashes.** Rigs are sim state, so retuning a rig moves every consuming scene's
  `WorldHash` and its replays re-record. This is accepted, the same shape as animation-clips
  decision 15.
- **vigil's re-record.** The migration changes vigil's camera state shape and its whole corpus
  re-records. It lands last in W3, after the schema snapshot.
- **Windowed projection** can lock onto the wrong branch where a path passes close to itself. The
  window size is a tunable, and the closed-loop and switchback tests are the guard.
- **Debug draw leaking into sim state.** It is written from sim systems, so the resource must stay out
  of `WorldHash`, snapshots and replays. W1's on/off hash-neutrality test is the guard.
