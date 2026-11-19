# Plan: Character controller and camera rigs — a 3D motor, paths and configurable cameras

> Status: **DESIGN SETTLED (2026-09-29, grilling session; every ruling is the user's). W1–W2 mechanized; W3's track subset mechanized; the rest of W3+ not started.**
> W2 holds the motor (`CharacterMotor`, `move_character`, `MoveReport`), `SurfaceVelocity`, resize and
> teleport, platform carry, the numeric trace and the course (`ludion docs motor`, `games/character-course`). The fourth external request added
> the lane constraint (`LaneConstraint`, `ludion docs lane`): a lateral hold to a `kind: path` corridor, and the fifth the
> track camera rig (`CameraRig`, `ludion-camera`, `ludion docs camera`), proved on the course's behind and side rigs. A first-person game's
> request added the `first_person` mode (look deltas, pitch clamp, `aim()`) and post-aim kick, roll and FOV channels. `chase` and `follow` are
> mechanized (Neon Shroud request 0010, W3a: decisions 29–32), as are the collision stage, the framing channel and the chase recenter (W3b: decisions 33–36), and the engine `CameraZone` with the occluder query (W3c: decisions 37–39); the gallery chase scene (W3d), decision 16 and the vigil migration remain open; `manual` is dropped (decision 12, amended).
> Neon Shroud request 0006 (2026-11-18): N1 `suspend_character` / `resume_character` / `character_fit` is mechanized (decision 26); N2 postures and N3 carry are accepted by the owner 2026-11-19 (decisions 27–28).
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
  - There are no action maps: `InputState` offers a standard move vector, key-or-button actions and trigger edges, and games own their bindings as constants (`ludion docs input`).
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
   - `MoveReport.lane: Option<LaneReport>` describes a call held to a lane; the signature is unchanged
     and the lane eases on the gameplay `dt` from `PhysicsWorld::gameplay_dt`.
   - Gravity and velocity are the moveset's, not the motor's.
   - **Why:** the moveset needs the report in the same tick (landed means switch state). A function
     keeps ordering in game Rust, the `move_and_slide` and `crossfade` idiom (decision 25 adds a batch).
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
   - a ceiling bump reported to the moveset;
   - a lane constraint (lateral projection + eased pull): a constraint over both modes, not a mode.

   Amendments from the external platformer's request (shipped in W2): `resize_character` and
   `fits_character` (growth is headroom-tested; `fits_character` returns `Result`); `SurfaceVelocity`
   for conveyors; `teleport_character`; the report gains `stepped`, `platform` (incl. `yaw` and
   `surface`), `moved`, `stuck` and `height`; a platform's yaw also turns the rider. Settled: the carry
   is applied after the slide and ground probe (casts see the support at its last-step pose); no carry
   from `rigid` supports; the engine defaults are human scale (the 1 m preset is authored). From the
   external platformer's request 0022: `probe_character_ground`, the motor's ground probe at an
   arbitrary feet position, read-only, at walk reach (step-up above, snap below), returning the hit
   and the rest height. A sweep variant (ground all along a segment) is a possible follow-up; trigger:
   a caller probes more than one point per tick. From Neon Shroud request 0006: `suspend_character`,
   `resume_character` and `character_fit` (decision 26).
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
   - It has four consumers: the track rig, `PathFollower`, game patrols and motor lanes.
   - Patrols: `kind: patrol`, `Plan/nav.md` decision 13 (nav legs, not splines).
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
    - **Amended:** `RigConfig` is embeddable, so a game's zone component carries one and copies it into
      `CameraRig::rig` (the course's `CameraZone`).
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
    - **Amended (2026-11-06, the owner's ruling):** a dedicated `first_person` mode replaces `manual`. The engine owns
      yaw and pitch so a presenter can late-latch look, and recoil, FOV kick and roll apply after aim, as shake does.
    - **Amended:** the game-written `grounded` hint steers the vertical anchor; `camera_relative` reads the
      yaw posed on the previous tick (the rig runs after game systems).
    - **Why:** a game shouldn't recode the common case, and shouldn't recode everything because the rig
      is too stiff. `manual` gives arbitrary Rust control and still gets the shared stages, without a
      mode registry.
13. **Game code switches rigs; the engine blends.** Writing a new rig configuration starts a timed
    blend from the current pose (the `crossfade` shape). *When* to switch is game Rust, for example an
    typically on a sensor's `Entered`/`Exited` event (`sensor_events`). **Why:** trigger conditions differ per game, and a zone
    component can be promoted later on repetition.
    - **Amended:** a write that differs from `state.last_applied` blends from a frozen pose over the incoming
      `blend_in`; `snap_rig` cuts, and so does the empty `last_applied` of a load or switch. The course's trigger is a
      sensor `Entered` event.
14. **The track rig projects the target onto one path.**
    - It uses the sampler's windowed `nearest_s` around last tick's parameter, so loops and switchbacks
      don't jump. `nearest_s`, `nearest_s_global` and `frame_at` are pure queries in W1's sampler in
      `ludion-scene`, shared with game patrols and lane locks.
    - The camera sits at the path point plus an offset in the path's frame, looking at the target.
    - Behind, in-front (boulder chase) and side views are all offsets.
    - **Amended:** shipped as written; the rail's height is authored in the path (the rig adds the offset only).
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
    - **Proposed (Neon Shroud 0005):** more pen primitives and a host-side `nav` category — `Plan/debug-draw.md`.
    - Amended 2026-11-19 (owner): the pen gains `circle`, `arc`, `wedge`, `ring_sector`, `cone`, `frustum_wedge`, and the host-side `nav` (`nav:grid`, `nav:routes`, `nav:flow`) and `perception` categories, see `Plan/debug-draw.md` d4.
18. **The agent verification surface:**
    - scripted-input scenario tests (a stick and button sequence asserted against a `MoveReport`
      sequence);
    - a per-tick numeric motor, rig and path trace as JSON from the CLI;
    - debug draw (the `lane` category beside `motor`; the motor trace line carries a `lane` object
      on a lane call);
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
24. **The lane constraint.** The motor honours an optional `LaneConstraint` that holds the capsule
    laterally to a `kind: path` lane by projecting every applied horizontal displacement and adding a
    capped pull; it eases on and off over gameplay time with a reversible smoothstep. Game code writes
    `engaged`, the motor eases (the pattern of decision 13). Vertical motion is free.
    - It lives in `ludion-engine`: `ludion-physics` stays free of `ludion-scene`, so the engine's
      `move_character` resolves the path and hands the motor a `MoveConstraint`.
    - One constraint per call: `move_character_with` bypasses the lane. A path change needs `s: null`
      (`set_path` does both), so no unhashed state hides. `teleport_character` nulls `s` (the lane
      re-acquires globally) and keeps `blend`, as `set_path` does.
    - **Why:** the lateral removal must live inside collide-and-slide, which only the motor owns, and
      2.5D, rails and lane runners all share it.
25. **Batch moves and the carry rule.** `move_characters(world, &[(entity, displacement)], &mut out)`
    (physics generic over `P`/`R`, engine bound to `Transform`) moves a crowd with its casts run in
    parallel over fixed chunks of 16 (`ludion_core::par`); results apply in slice order. `move_character`
    stays the call for ordered, report-first game logic: its call order is still game order.
    - **Tick-start support pose.** Every support pose a batch call reads (the carry due and the
      `platform` it records) is the world as it was when the batch began. With no support moving during
      the batch the result is bit-identical to serial `move_character` calls in slice order, at 1/2/8
      threads, and independent of slice order. A motor standing on a motor is carried by the support's
      previous-tick motion: a one-tick lag that loses no motion.
    - A duplicate entity moves on its first entry; later ones get `MotorError::DuplicateInBatch`. An
      entity with an active lane gets `MotorError::LaneInBatch` (the engine wrapper checks the
      `resolve` gate); there is no `MoveConstraint` in a batch.
    - **Why:** the 500-motor horde spends about twice the physics step in serial casts
      (`Plan/physics.md` "Horde bench"); casts only read `&PhysicsWorld`, so they parallelise without
      touching the sim's single-writer rule.
    - Shipped under Serious Isaac 0020's conservative clause, not an owner amendment.

26. **Suspend and resume** (Neon Shroud request 0006, wave N1; shipped). `suspend_character(world, entity)`
    parks a motor: `CharacterMotorState.suspended` (omitted at `false`, so every existing scene, replay and
    trace is byte-identical) clears the grounded record and gives the motor **no body** at the next write
    pass; every move (`move_character`, `move_character_with`, `move_characters`) and `teleport_character`
    is `MotorError::Suspended`. `resume_character::<P>(world, entity, feet, yaw, posture)` brings it back
    only where it fits, using `character_fit` (the `headroom` overlap test `resize_character` already
    runs, at the height in force, so a crouch is kept). A blocker is `Ok(ResumeReport { resumed: false,
    blocked_by })` with nothing changed. A fit writes the pose, clears the state like a teleport and
    places the capsule without pushing props. Error order: `NoMotor`, `NoPose`, `NonFiniteTeleport`,
    `UnknownPosture`, `Parented`, `NotSuspended`, then the fit (`InvalidMotor`, `NoPhysicsWorld`).
    - `posture` is in the signature so postures (decision 27) do not break it; no motor declares any in
      N1, so `Some(_)` is `UnknownPosture`. `yaw` is written to the pose and has no effect on the fit of
      an upright capsule.
    - **L1297 exception.** A suspended motor under a `Parent` raises no `L1297`, at runtime or in
      `check-scene` (an authored `CharacterMotorState: { suspended: true }`): it has no body and is
      parked while something else owns it. Resume refuses `Parented`: reparent to the root first.
    - **The ragdoll switches suspend.** `switch_to_ragdoll` and `switch_skin_to_ragdoll` suspend the motor
      on the switched entity or its nearest ancestor that has one (walking `Parent`), on the success
      path only. **Why:** it is one call with nothing to forget (the game's preference), and no existing
      content switches a motor entity or a motor's child, so no hash moves.
    - `resize_character`, `fits_character` and `probe_character_ground` keep working on a suspended motor.
    - Removing `CharacterMotor` is not a suspend: the state and the motor settings are lost.
27. **Postures** (Neon Shroud request 0006, wave N2; **accepted by the owner 2026-11-19**). A `postures:` map on
    `CharacterMotor`, each `upright { height, radius? }` or `lying { length, radius, offset }`, with
    per-posture `step_height`, `slope_limit_deg` and `snap_distance`. A lying capsule yaws with facing
    and never pitches. API: `set_character_posture`, `character_fit` with a posture (the `posture`
    parameter of decision 26 starts to mean something), `turn_character` in 15° destination-tested
    sub-steps; `MoveReport.posture`; `CharacterMotorState.posture`.
    - **Amendments:** decision 5's capsule "never tilts or yaws" and the non-goal "Motor modes beyond
      ground and air", whose trigger (a GDD asking) fired: **shipped additive, owner review owed** — only
      a `lying` posture yaws; upright motors are bit-identical (no trace, replay or golden moved). This
      adds shapes, not modes.
    - N2 is split: **N2a** (shipped, mechanized) the data model plus upright postures; **N2b**
      (mechanized) the `lying` shape through every sweep in `mover.rs`, the write-pass rotation,
      `turn_character` and the amendment above.
    - **N2b rules.** `lying: { length, radius, offset? }`, `offset` default 0, `|offset| <= length/2 - radius`
      so the feet stay under the axis (every ray-down probe stays valid). Forward is local −Z; the yaw is
      the pose rotation's twist about +Y (`carry::yaw_delta`), never pitch or roll. The capsule's
      rotation is `q_yaw * q_x(90°)`, its centre `feet + offset*forward + (0, radius, 0)`, its
      half-segment `length/2 - radius`; `motor_in_force` reports `radius` and `height = 2*radius`.
      Upright code paths keep their exact arithmetic (branch, never a generalized offset). Fit test:
      any switch to, from or between lying postures is tested at the current feet and yaw; an upright to
      upright switch keeps N2a's rule. `turn_character` turns an upright motor at once; a lying one in
      sub-steps of at most 15° (destination-tested, shortest way round), stopping at the first blocked
      step and naming the blocker; rigid props block a lying turn; pitch never. `resize_character` on a
      lying posture is `InvalidHeight`; `fits_character` while lying asks the stand-up question with the
      base radius. Open questions answered: the game's offset preference kept (default 0, authored
      per posture), no pitch to the ground normal, 15° steps, and no drop-spot helper.
    - **N2a rules.** Format `postures: { crouch: { shape: { upright: { height, radius? } }, step_height?,
      slope_limit_deg?, snap_distance? } }`; `shape:` mirrors `Collider.shape`, so `lying` joins without a
      breaking change. Empty `postures` and a `None` posture are omitted from saved text, so no hash,
      replay or trace moves; the component schema snapshot is the only re-blessed artifact.
    - **One motor in force:** `motor_in_force(motor, state)` applies the posture's shape and tunables, then
      the `state.height` resize override on top; every solver, write-pass and fit path uses it. An
      undeclared posture falls back to the base motor.
    - `set_character_posture` clears the height override (the posture defines its shape) and fit-tests
      growth (taller or wider) at the feet like `resize_character`; a blocker changes nothing.
      `character_fit`/`resume_character` take a declared posture; `teleport_character` restores the base
      shape (no posture, authored height) when it fits at the new spot.
    - Diagnostics: L1294 at posture pointers (names with `/` or `~` refused); `L1540` check-scene for an
      authored undeclared `CharacterMotorState.posture`; `L4170` once per entity at runtime (fallback).
28. **Carry** (Neon Shroud request 0006, wave N3; **accepted 2026-11-19, scheduled (wave N3)**). A joint whose `a` is a
    kinematic anchor, documented and tested. The carrier/carried filter is served by request 0002's
    in-place layer membership if that ships in place; otherwise by `IgnoreCollisions { with }`. Recipes
    for the drag carry and the shoulder carry go in `docs/motor.md` §Carrying.

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
   - a trace, a `.lrp` and a golden blessed on the 3090 (the M4 re-bless is owed).
5. Docs: the motor topic, and the `move_and_slide` ↔ motor pointers (decision 7).29. **The pivot is sprung, the angles are not** (Neon Shroud request 0010, W3a; mechanized). In `chase` and
    `follow`, `springs.position_half_life` moves a *pivot* toward the rig focus (look-ahead and the
    vertical anchor are shared with `track`); the eye sits rigidly on a boom from it
    (`Orbit { pivot, yaw, pitch, distance }`). `rotation_half_life` is ignored, as in `first_person`.
    **Why:** stick and mouse look stay 1:1 with no spring lag, the late latch is exact (rotate about the
    pivot), and a held tick still applies look.
30. **Chase look reuses `look_deg`** (0010 W3a; mechanized). The D15 amendment: the game writes deltas
    through `add_look`, the rig applies them on every tick it poses (held clock included), wraps the yaw
    and clamps the pitch to `[pitch_min_deg, pitch_max_deg]` (`min == max` fixes it); `look_deg[1]` moves
    the pitch only with `look_pitch`. The presenter's late latch covers `chase` like `first_person`.
31. **Follow consumes look** (0010 W3a; mechanized). `follow` holds a fixed world yaw, pitch and
    distance and zeroes `look_deg` each tick it poses, so stick input never accumulates during a follow
    zone and pops on the return to `chase`. `track` is unchanged (look is not consumed).
32. **Snap and switch rules** (0010 W3a; mechanized). Into `chase`: yaw is `chase.yaw_deg` if set, else
    the current yaw carries (from `track`, from `first_person`, or 0 on a fresh load); pitch is
    `chase.pitch_deg` clamped; the pivot rests on the focus. Into `follow`: the authored yaw and pitch.
    `first_person.enter_pitch_deg` (`None` carries the pitch) sets the pitch on a snap or switch into
    first person. A spawn facing is `set_look` (its yaw carries; the pitch is reset by the mode's own
    entry rule). The request's "`null` yaw = behind the target's facing" is not built: `RigInput`
    carries no target rotation.
33. **Collision follows the blend and pulls in at once** (Neon Shroud request 0010, W3b; mechanized).
    `RigConfig.collision` (off by default; it lives on the config so a zone can swap it) casts a sphere
    from the pivot (the focus, on `track`) to the blended pose's eye. **Why after the blend:** a blend
    out of first person never cuts through the wall the player peered past. A hit pulls the boom in
    on the same tick, even on a held clock, and starts `recover_delay`; once the hold is spent the boom
    eases back with `recover_half_life`, and the state returns to default when it is whole again. A
    cast that starts overlapping retries once from the target; if that overlaps too, the boom holds
    at `min_distance` and the tick reports `stuck` (`L1552`). A snap clears the boom; a switch keeps it.
    First person never collides.
34. **The `RigCollision` seam** (0010 W3b; mechanized; D20 accepted by the owner 2026-11-19). `ludion-camera` stays
    physics-free: `RigInput.collision` carries a `&dyn RigCollision`, one `cast` method, and
    `ludion-engine` implements it over `PhysicsWorld::shape_cast`, excluding the target (its child
    colliders are not excluded: put them on an ignored layer). Layer names are checked at runtime
    (`L1550`), not by `check-scene`.
35. **Framing moves the eye before collision** (0010 W3b; mechanized). `CameraRig.framing` is
    game-written, like `roll_deg`: a weight in `[0, 1]` eased by `half_life` applies a pivot offset in
    the yaw frame, a yaw and pitch offset and an optional distance, for `chase` and `follow`. The framed
    yaw and pitch are the pose's (`aim()` sees them); `state.yaw_deg` stays the unframed orbit yaw. At
    weight 0 the arithmetic is the plain boom's, bit for bit. The presenter's latch rebuilds the same
    framed boom at the tick's collided length without casting.
36. **Chase recenter** (0010 W3b; mechanized; the decision 12 amendment). `ChaseRig.recenter`, off by
    default: after `delay` gameplay seconds with no look, and while the target's horizontal speed is at
    least `min_speed`, the yaw damps (`half_life`) along the shortest arc toward the heading behind the
    motion. A nonzero look restarts the wait.
37. **Engine camera zones** (Neon Shroud request 0010, W3c; mechanized; the "Engine camera zones" non-goal
    promoted, accepted by the owner 2026-11-19, item 1). `CameraZone { priority, rig }` sits beside a
    sensor `Collider`. Membership is stateless: each tick, before `step_rig`, a 1 cm sphere overlap at the
    target's origin finds the zones (so a respawn, teleport, restore or load is correct with nothing to
    hash). The highest priority wins, then the lowest `EntityId`; a top-priority tie is `L1555`.
    `RigState` gains `zone` and `base` (hashed, skipped at default). Entering from no zone stores the
    config in force as the base, a zone change copies the zone's `rig` so the existing blend runs over its
    `blend_in`, leaving restores the base, and a game write inside a zone becomes the base (item 4).
    No `CameraZone` in the world means no query and no state touched. `L1553` no physics, `L1554`
    check-scene without a sensor. `CameraZone` registers at the registry's end (the append-only order).
38. **The occluder query** (0010 W3c; mechanized; item 6). `camera_occluders(world, camera, &layers,
    radius)` sphere-casts from the rig focus to the posed eye, excluding the target, and returns the
    sorted, deduplicated entities. It is a function, not a `RigState` field: an entity list in a
    component is `L1392`, and a non-hashed resource would differ after a restore. One tick behind, like
    `camera_relative`. The `Fade` policy stays in the game.
39. **The course's gate is renamed `CameraGate`** (0010 W3c). The engine took the name `CameraZone`; the
    course's switch-and-stay component keeps its semantics under the new name. Its hashes moved (component
    names are hashed) and `run.lrp` was re-recorded; no golden moved.

### W3 — Camera rigs and vigil

Items 1, 2 and 5 are mechanized for the track subset (`ludion-camera`, `ludion docs camera`); item 3 as the course's
behind/side rigs, zones, snap and respawn trauma (`games/character-course`, `tests/camera.rs`); a chase section is open.
W3's remaining work splits (Neon Shroud request 0010): **W3a** `chase`, `follow` and `enter_pitch_deg`
(mechanized, decisions 29–32); **W3b** the collision stage, the framing channel and the chase recenter (mechanized, decisions 33–36); **W3c** the engine
`CameraZone` and the occluder query (mechanized, decisions 37–39); **W3d** `examples/gallery/camera-chase.yaml` (its trace and golden, pinned blesses owed) and the course's chase section (re-recording the course corpus a second time).

**Owner rulings (2026-11-19)** (Neon Shroud request 0010):
1. Accepted. Promote the non-goal "Engine camera zones" to a decision; the trigger fired.
2. Accepted. D20: `ludion-camera` stays physics-free behind a `RigCollision` trait that `ludion-engine` implements.
3. Accepted. D12 gains a framing stage before collision.
4. Accepted. A game write while zones are active becomes the new zone `base`.
5. Accepted. `camera_relative` ignores the framing yaw offset during a peek. Not built: it reads the framed
   posed yaw, and a game that wants the unframed one calls
   `camera_relative_yaw(stick, rig.state.yaw_deg)`.
6. Accepted. The occluder fade stays game policy, with the engine handing over an `occluders` list.
7. Accepted. `enter_pitch_deg` defaults to `None`, carrying the pitch.

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
4. The live smoke is owed.

### W5 — Animation hookup (gated on animation-clips W2)

1. The reference controller drives the mannequin: idle, run and jump through `crossfade`, chosen from
   `MoveReport` and speed.
2. The course's `.lrp` and golden re-record if the pose changes them.

### Neon Shroud 0006 — suspend, postures, carry

- **N1 — suspend and resume** (mechanized; decision 26). `suspend_character`, `resume_character`,
  `character_fit`, `CharacterMotorState.suspended`, the `L1297` exception, the ragdoll switches suspend.
  Docs: `docs/motor.md` §Suspend and resume.
- **N2a — upright postures** (mechanized; decision 27). `Posture`, `PostureShape`, `set_character_posture`,
  `motor_in_force`, `MoveReport.posture`, `L1540`/`L4170`. Docs: `docs/motor.md` §Postures.
- **N2b — the `lying` shape** (mechanized; decision 27). `PostureShape::Lying`, `turn_character`,
  `TurnReport`, `motor_capsule_segment`. Docs: `docs/motor.md` §Postures.
- **N3 — carry** (**accepted 2026-11-19, scheduled (wave N3)**, decision 28).

### Exit (human gate, owed)

One sitting (about 30 minutes):

- motor feel on the course (slopes, steps, platforms, snap) and each camera mode;
- vigil's migrated camera, at least as good as before;
- the editor live smoke;
- the mannequin's locomotion look (crossfades and foot sliding at run speed).

The verdict is recorded as a decision. Close: the root `CLAUDE.md` line, and this plan's Status
collapsed.

## Non-goals

- **A full 3D lock** (vertical constrained too: the hero glued to the curve, as in rail grinding with no
  ground under it). **Trigger:** a game asks. Express it as a `constrain: lateral | full` field added
  later; the lane ships `lateral` only, and a rail with collision is already served by it.
- **Per-triangle surface tags** — lifted as tags only (ice, footsteps): see `Plan/surface-tags.md`.
  Kill zones are sensor volumes.
  Amended 2026-11-19 (owner): the non-goal is lifted as tags only, see `Plan/surface-tags.md`.
- **Motor modes beyond ground and air** (swimming, ladders, ledge grab, wall jump); the `lying` posture
  is a shape, shipped additive, owner review owed. **Trigger:** a GDD
  asks. New modes extend `MoveReport`.
- **Dynamic bodies pushing the character.** **Trigger:** a moveset needs knockback from physics.
- **Tilting platforms** (pitch and roll riding). **Trigger:** a level needs one.
- **More rig variants:** a framing dead zone, a static look-at rig, a guide-plus-camera track.
  **Trigger:** a level or game the shipped modes plus `manual` can't serve without repetition.
- **Engine camera zones** — now decision 37 (accepted by the owner 2026-11-19).
- **`exit_blend`** (a zone's own blend when leaving it; the incoming config's `blend_in` is used).
  **Trigger:** a game needs a different leave blend than enter blend.
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
