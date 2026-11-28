---
description: The Joint and JointMotor components — hinge, ball, slider, fixed constraints and drives
---

# Physics joints

**Scope:** Authoring a `Joint` component to constrain two bodies together (a rigid body, or a rigid body and a kinematic anchor), and a `JointMotor`
to drive its free axes. Defers to `ludion docs physics` for `Collider`, `RigidBody` and the rest of
`ludion-physics`'s rigid-body dynamics, and `ludion devdocs physics-joints` for the rapier mapping.

## The loop

Author a `Joint` on any entity — including `a` or `b` itself — pointing at two claimed bodies,
then watch it: `is_joint_active` and `joint_events()` on the engine's
`PhysicsWorld` tell you whether it is attached and whether it has broken. It takes effect the tick both bodies exist.

## Format

```yaml
  - name: bob
    components:
      Transform:
        translation:
          - 1.0
          - 3.0
          - 0.0
      Collider:
        shape:
          sphere:
            radius: 0.2
        body:
          rigid:
            mass: 1.0
      Velocity: {}
      Joint:
        a:                   # entity 0: a static anchor
          index: 0
          generation: 0
        b:                   # this entity
          index: 1
          generation: 0
        kind:
          hinge:
            axis:
              - 0.0
              - 0.0
              - 1.0
            limits:  # degrees; omit for free rotation
              - -90.0
              - 90.0
        anchor:  # in a's local frame
          - 0.0
          - 0.0
          - 0.0
        break_impulse: 500.0             # omit: the joint never breaks
        collide: false
```

`a`/`b` name their bodies by document position (generation `0`) in a scene, live `EntityId`s on the
wire. `kind` is one of `hinge` (`axis`, `limits` in degrees about it), `ball` (`limits`, degrees, one
`[min, max]` pair per local `x`/`y`/`z`), `slider` (`axis`, `limits` in world units along it) or
`fixed` (no fields). `anchor` and every `axis` are authored in `a`'s local frame. `break_impulse`
omitted means the joint never breaks; `collide` (default `false`) is whether `a` and `b` still
collide with each other. Multi-axis ball limits and motors are solved with the exact rate of the angle
they constrain, so a limited ragdoll at rest does not gain energy.

## At runtime

A joint forms and tears down with the commands `entity.spawn`/`entity.destroy` already have — no
dedicated joint commands exist (`Plan/physics.md`, ruled 2026-09-23): a joint is a component on its
carrier, so `entity.spawn` creates one. It runs each component's schema check and semantic
validator before touching the world, like `component.set` — a bad `Joint` is `L1183`, nothing spawns.

```json
{
  "method": "entity.spawn",
  "params": {
    "components": {
      "Joint": {
        "a": { "index": 3, "generation": 1 },
        "b": { "index": 7, "generation": 0 },
        "kind": { "ball": {} },
        "anchor": [0.0, 0.0, 0.0],
        "collide": false
      }
    }
  }
}
```

`a`/`b` are live `EntityId`s on the wire, not document positions — take them from `query.entities`,
or, in a freshly loaded scene with no despawns yet, the document position at generation `0`. Destroy
the joint with `entity.destroy` on the carrier (or `component.remove` its `Joint`); the body it held
falls free the next tick.

## Motors

A `JointMotor` on the joint's carrier, beside its `Joint`, drives the joint's free axes toward a
`target`:

```yaml
      Joint: { a: { index: 0, generation: 0 }, b: { index: 1, generation: 0 }, kind: { hinge: { axis: [0.0, 0.0, 1.0] } }, anchor: [0.0, 0.0, 0.0] }
      JointMotor:
        target:  # degrees from the joint's captured zero
          - 30.0
        stiffness: 39.5       # N·m per radian of error
        damping: 6.3          # N·m·s per radian
        max_force: 20.0       # N·m cap; omit: uncapped
```

Those numbers are the tuning rule below for 1 kg at 0.5 m, 2 Hz.

- `target` has one number for a `hinge` (degrees) or a `slider` (world units) and three for a
  `ball` (degrees about `a`'s local x/y/z). It is measured from the captured zero, with the sign
  and units of `limits`.
- The drive is a force-based spring-damper toward `target` (rapier's `MotorModel::ForceBased`),
  solved implicitly, so a high `stiffness` stays stable. `stiffness` is N·m/rad on a hinge or ball
  — per **radian** of error, although `target` is in degrees — or N/m on a slider; `damping` is
  N·m·s/rad or N·s/m. The same numbers drive a heavy limb slower than a light one, as a real servo
  would.
- **Tuning rule.** A hinge swings a part of mass `m` whose centre is `r` from the anchor, so
  `I ≈ m·r²` (plus the part's own inertia when `r` is small). Pick a response frequency `f` in Hz,
  `ω = 2π·f`: `stiffness = I·ω²` and `damping = 2·I·ω` (critically damped). A slider uses `m` in
  place of `I`. 1 kg at 0.5 m and 2 Hz gives `I = 0.25`, `ω ≈ 12.57`, stiffness ≈ 39.5, damping
  ≈ 6.3.
- The rule holds while the part's own inertia is within about 10× of its swing inertia about the
  anchor (a rod hinged at its end is about 4×). A point-like part on a long lever overshoots and
  rings, because rapier's angular motor row sees only the part's own inertia. Fix it with more
  damping, or give the part real extent.
- `max_force` caps the real torque (hinge/ball, N·m) or force (slider, N) per axis; omitted means
  uncapped. Set it to at least the torque gravity puts on the part (`m·g·r`), or the part sags.
- A motor is passive with no `JointMotor`, or with `stiffness: 0` and `damping: 0`; either is
  bit-identical to no motor.
- Editing the motor every tick is the intended use: it never rebuilds the joint or re-captures the
  zero, and a change wakes both bodies. An active motor short of its target keeps its bodies awake
  (its error above 1e-3: about 0.11° or 1 mm), so a motor that cannot reach its target — a limit, a
  `max_force` cap, gravity sag — never sleeps.
- The motor row's own impulse never counts toward `break_impulse`, but the reaction a hinge or ball
  motor causes on the joint's locked axes does: a stiff drive against a low `break_impulse` can break its own joint.
- `L1188` rejects the component alone at load or `component.set`: a `target` arity other than 1 or
  3, a non-finite value, a negative value. `L3037` reports at runtime, against the carrier's
  `Joint`, a `fixed` joint, an arity that does not match the kind, or a bad value that bypassed
  `L1188`; the joint stays passive until the `JointMotor` or `Joint` changes, reported once per offender (a prefab part, a named entity, or an unnamed entity's slot). A passive motor (stiffness and damping both 0) is not checked, so a mismatch reports
  when a system first makes it active. A `JointMotor` with no `Joint` (after a break, say) is silently inert.

## Pose blends

`JointPoses` (per jointed part) and `PoseBlend` (on the ragdoll group's root) drive a whole group
between named poses by one weight, so a game writes one number a tick instead of a `JointMotor` per
part:

```yaml
      PoseBlend:               # on the group root
        pose: curl
        weight: 0.5            # 0 limp .. 1 full pose
      JointPoses:              # on each jointed part
        group: { index: 3, generation: 0 }   # the root; omit on a skin part
        stiffness: 40.0        # at weight 1
        damping: 8.0
        max_force: 200.0       # never scaled; omit: uncapped
        poses:
          curl: [60.0]         # JointMotor.target units and arity
          stand: [0.0]
```

- **Group.** A part belongs to `JointPoses.group` if set, else its `SkinBone.skin` (so a skinned
  ragdoll needs no `group:`). Voxel and other parts set `group:` to the root. A part with neither is
  inert: the system never touches its motor.
- **Resolution.** The `pose_blend` system runs just before the physics step, after every game system,
  and gives each grouped part `JointMotor { target: pose·w, stiffness·w, damping·w, max_force }`,
  with `w = weight` clamped to 0..1 (NaN counts as 0). At `w == 0`, with no `PoseBlend` on the root
  (or a dead root), or when the part lacks the named pose, the part carries **no** motor: it goes
  limp and can settle and sleep. A held non-zero blend short of its target keeps the bodies awake
  (see Motors). The blend owns the motor of a part carrying `JointPoses`; do not hand-write one.
  A part that loses `JointPoses` (or its group) while the weight is non-zero keeps its last written
  `JointMotor`: set the weight to 0 (or remove the `JointMotor`) before stripping `JointPoses`, e.g.
  in a pool reset.
- **Write on change.** The system writes only when the result differs from what the part carries, so
  a steady weight never wakes a body.
- **Rest-relative.** Poses are measured from the joint's captured zero, like `target`.
  `switch_skin_to_ragdoll` switches at the rig's rest pose, so a pose's zero is the rest pose; any
  posed switch must keep rest-relative zeros.
- `L1188` rejects a pose that is not 1 or 3 numbers (`/poses/<name>`), a non-finite one, and a
  negative `stiffness`, `damping` or `max_force`. A pose of the wrong arity for its joint reports
  `L3037` as a motor would. `L4035` warns once per (group, pose) when a `PoseBlend` above weight 0
  names a pose none of its parts carries; weight 0 never warns, so a root may hold a `PoseBlend`
  before its parts exist.

See `examples/gallery/ragdoll-pose-blend.yaml` (skinned, voxel and pinned ragdolls; driver in
`crates/ludion-testkit/tests/common/mod.rs`).

## Rules

- An end may be `kinematic`: it is posed by its parent or `Transform`, never pushed, and the other end follows it (a sleeping part wakes).
- `a` and `b` must name two distinct bodies; every axis and the anchor must be finite; an axis must
  be non-zero; each `limits` pair must be ordered `[min, max]` with both finite; `break_impulse`, if
  set, must be strictly positive and finite as an `f32` — any violation is `L1183`, pointing at the
  offending field.
- `a`/`b` must name a document position in range (`L1017`) and `scene.save` fails with `L1018` if
  either now names a despawned entity — the same rule `Parent` follows. A dismembered part (`a`/`b`
  despawned) is a legitimate inert state to *run* with, but cannot be *saved* until the dangling
  `Joint` is repaired or removed.
- `anchor` and the kind's `axis` are authored in `a`'s local frame (a compound's is the entity's,
  not a part's), but the joint's **zero is the
  bodies' current relative pose when it is built** — not the numbers alone. A runtime-spawned joint forms
  exactly where the bodies already are. Across a save/reload a scene is not a snapshot: reloading
  re-captures every joint's zero at the *saved* pose, so every limit shifts to match.
- **`Joint.zero` overrides the capture.** `zero: [x, y, z, w]` on the `Joint` (absent: captured at
  build) is the rotation of `b` in `a`'s frame that counts as zero: limits, `JointMotor.target` and
  `JointPoses` are measured from it, while the anchors still coincide at build. It is part of the
  `Joint`, so it survives a body rebuild, a ragdoll retire and re-admission, a snapshot restore and a
  scene save and reload; adding, editing or removing one rebuilds the joint. A pose outside a limit
  measured from the zero is pulled back by the solver on the first step. A non-finite or near-zero
  `zero` is `L1750` (and is ignored by the solver, as if absent).
- A `break_impulse` a step's linear constraint impulse exceeds removes the joint and reports
  `JointBroken { joint, a, b, impulse }` on `PhysicsWorld::joint_events()`; the `Joint` component
  itself is removed the same tick, so a joint exists iff its component does. A slider's own end-stop
  impulse counts toward the break the same as a hinge's or fixed joint's locked axes; a hinge's or
  ball's *angular* limits never do — a break is a linear-impulse quantity.
- If `a` or `b` is dead or has no body, the joint sits inert — never deleted — and `L3035` reports it
  once per offender for the session. It re-forms once both bodies exist (an unclaimed `Collider` coming good, a delayed spawn) — never
  for a dead `a`/`b`: that carrier stays inert until its `Joint` names a live body.

## Skinned ragdolls

A skinned mesh's bones follow rigid bodies the same way a voxel ragdoll's parts do — `Joint`s between
ordinary rigid-body entities — plus one `SkinBone { skin, bone }` per part, naming the skinned mesh
entity and the joint it drives (`ludion docs components`). Each part's own `Collider` is a `compound`
of one capsule along the bone's local +Y (Blender's head → tail axis), offset `[0, len/2, 0]` from the
part's origin so `half_height = len/2 - radius`; a part with no authored `Transform` spawns at its
bone's rest pose, so it needs no separate placement. Physics tunables (`Collider`, `Joint`, masses,
limits, motors) are authored exactly as for a voxel ragdoll.

Anchors and axes stay in the parent joint's local frame; for a skinned rig that frame comes from `ludion check-scene <mesh> --bounds` (or `--bounds --json` for full precision): each
joint's `head_in_parent` **is** the anchor, multiplied by the skin entity's own uniform scale (a gallery
skin stays at `1`, so the anchor is a straight copy there) — this multiply-by-scale rule holds only
while every bone's own rest `scale` is `1`, which a normal Blender export gives; a rig with a non-unit
rest scale on some bone needs its anchor derived by hand. A hinge's `axis` is not always as simple: it
is authored in the parent bone's own rotated local frame, which only lines up with a world axis by
coincidence (a vertical leg bone's local X stays world-lateral; an A-pose arm bone's does not) — see
`examples/gallery/ragdoll-skinned.yaml` and `Plan/skeletal-animation.md` decision 8 for the worked
case and its cost.

### Switching a live skin

A skin already playing a clip can become its ragdoll mid-run: `switch_skin_to_ragdoll(world, skin, parts)`
(Rust only, no wire method) spawns a parts prefab bound to the live skin. The prefab is an ordinary
`kind: prefab` of `SkinBone` parts and `Joint`s whose **last** entity is named `$skin` and carries only
the skin's `Mesh`, spelled so it resolves to the same mesh document (a path in a prefab is relative to
the prefab's directory). `$skin` spawns nothing; `SkinBone.skin` on the parts points at its position:

```yaml
  - name: lower_leg_l
    components:
      SkinBone: { skin: { index: 11, generation: 0 }, bone: lower_leg_l }
      # Collider, Velocity, Joint ...
  - name: "$skin"
    components:
      Mesh:
        shape:
          assembly: "../characters/mannequin.assembly.yaml"
```

The skin keeps its id and `Mesh` and loses `Animation`, `Collider` and `Velocity`. Each rigid part gets
the skin's linear velocity plus angular × (part origin − skin origin), and the skin's angular
velocity; a part's own authored `Velocity` is replaced. A part's linear velocity is taken at its bone
origin, not the body's centre of mass (exact when the skin does not spin). The switch is
all-or-nothing: a dead or presentation skin, no `Transform`, a non-uniform or non-positive scale, a
mesh that is not skinned or differs from the prefab's, a missing or misplaced `$skin`, a part bound
to another skin or another ref to `$skin`, no `SkinBone` part, or a skin already driven by `SkinBone`
parts (a second switch, or a scene-rigged skin) is `L3036`, and nothing is spawned or changed. The switch also suspends the `CharacterMotor` on the skin or its nearest ancestor (`ludion docs motor`
§Suspend and resume). Call it on one tick (`ctx.game_frame == N`, never
`>=`). See `examples/gallery/ragdoll-skin-switch.yaml` and `prefabs/mannequin-ragdoll.prefab.yaml`.
**The parts land at the bone rest pose, not the pose the clip shows on the switch tick; the posed
switch is owed.**

## Ragdoll budget

A settled ragdoll stops costing physics when a game **retires** it: in one tick, remove `Collider`,
`Joint`, `JointMotor` and `Velocity` from every part, keeping `Transform` and `SkinBone`. The bodies and
joints leave `PhysicsWorld` on the next physics write (`body_count`, `joint_count` show it), the skin
stays drawn in its settled pose, and the retired parts are ordinary hashed state that round-trips
`engine.snapshot`/`engine.restore`. Drop the `Joint` in the same tick: one left on a part whose body is
gone goes inert and is reported.

- **Settle:** every part `PhysicsWorld::is_sleeping(part) == Some(true)`. Sleep is already debounced by
  `sleep_after_ticks` and restored with the snapshot, so the retire system keeps no counter. A blast
  wakes sleepers; retiring, not sleeping, bounds the cost.
  A limp ragdoll of ball-jointed, near-spherical parts sleeps late: an island sleeps only when every part is
  still, and rapier also refuses sleep while any part spins faster than π/2 rad/s, so a slowly spinning
  shin holds all eleven awake (measured: 185 / 231 / never ticks to sleep when calm / shot / curled, over
  2,000–3,300 awake part-ticks in 300). Give its parts `sleep_threshold: 0.1`, `sleep_after_ticks: 15`
  and `sleep_angular_threshold: 2.4` (123 / 132 / 129 ticks; without the angular value 128 / 160 / 150).
  Past 2.4 the linear/drift gate binds, so 120 ticks is not reachable this way; `sleep_threshold: 0.15`
  with `sleep_after_ticks: 8` is the fastest tune measured (108 / 124 / 120). A part may freeze mid-rock at up
  to that angular speed, and only bodies that opt in change. Damping alone is chaotic, not a fix.
- **Never-sleepers:** some parts never fall asleep, so pair the settle retire with a max-age
  force-retire (the example's `max_sim_ticks`), or those groups stay simulated forever.
- **Pinned group:** a part joined to a `Body::Static` collider outside the group. Evict those last.
- **Re-admission:** re-insert the removed `Collider` and `Joint` (plus a `Velocity` for the hit). Bodies
  rebuild at the retired pose, and **joint zeros are re-captured there, so limits count from the
  settled pose, not the rig rest** — unless the `Joint` keeps a `zero` (a `LimitsFrom::Rest` switch
  sets one; `ludion docs voxel-assemblies` §Ragdolls). Keeping the removed values across a restore is the game's job
  (re-read them from the parts prefab in the same order).

The runnable pool is `cargo run -p ludion --example ragdoll_budget`: a simulated-group cap,
settle-retire, oldest-unpinned eviction (pinned last) and a retired cap, its state in registered
components so it hashes and restores. Copy its half above `main`.

Measured ragdoll step cost comes from the ragdoll bench (`crates/ludion-engine/tests/ragdoll_bench.rs`),
with its figures in `Plan/physics.md` (the "Ragdoll bench" Status entry). The
idiom is pinned by `crates/ludion-testkit/tests/ragdoll_retire.rs`, with helpers in `tests/common`.

## Gotchas

- A carried body is a rigid part `Joint`ed to a kinematic anchor under a walking motor; the carrier-side layer swap that frees the pair is `ludion docs motor` §Carrying.
- Editing a live `Joint`, or rebuilding a body it names (a `Collider` edit), re-captures the zero at
  the bodies' new poses — it does not resume the old constraint. The exception is an in-place tunable
  edit (`ludion docs physics` §Live edits): it keeps the zero; any other edit on a jointed body
  still rebuilds it and re-captures the zero.
- A `Transform` write teleports a body without re-capturing the zero: move a jointed figure by moving
  every body the same tick by the same rigid transform; a lone moved body is pulled back.
- A `SkinBone` part with no authored `Transform` spawns at the rig's rest pose (the skin entity's
  `Transform` times the bone's rest transform), so every `Joint`'s zero on it is captured at that rest
  pose — every `limits` pair is measured from the rig's authored rest, not from wherever the part might
  otherwise land, and a re-rig in Blender moves what every limit means.
- A switched skin's joint zeros are captured at rest, as at load (the parts spawn there).
- A skin whose scale is not 1 gives scaled parts, and the solver refuses their colliders with `L1290`
  (`ludion explain L1290`): keep switched skins at scale 1.
- `scene.save` of a switched skin is refused with `L1234` (the prefab re-expansion cannot drop
  `$skin`): persist it with `engine.snapshot` / `engine.restore`.
- Any entity may carry a `Joint`, including `a` or `b` itself: a ragdoll's convention is each child
  part carrying the joint back to its parent part; a runtime grab spawns a dedicated carrier instead.
  One `Joint` per carrier — a body that needs more than one joint takes a dedicated carrier per
  joint.
- In a `planar` scene, only a hinge about the plane normal (or a slider in-plane) is not
  over-constrained by the plane's own axis locks.
- A pair on layers that ignore each other never collides, even with `collide: true`: the layer
  filter wins (`ludion docs collision-layers`).
- Springs, velocity-target motors and multibody joints are not offered.

## Next

- `ludion docs physics` — `Collider`, `RigidBody`, forces, contacts and queries.
- `ludion devdocs physics-joints` — the rapier mapping and break-detection internals.
- `ludion docs voxel-assemblies` — Ragdolls: an assembly's `joints:` list and `switch_to_ragdoll`.
- `ludion docs components` — `SkinBone`: the link between a rigid part and a skinned mesh's bone.
