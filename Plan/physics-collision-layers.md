# Plan: Physics collision layers — which bodies ignore which

> Status: **W1–W2 mechanized; W3 closed as not needed (the external game's ruling, 2026-09-24).** Design settled 2026-09-24 (the user's rulings are under §Rulings). Current-code facts were read from the tree on 2026-09-24 (file:line pointers are of
> that date; re-grep before editing). Parent plan: `Plan/physics.md` (W1–W4 mechanized); this is the
> follow-up its W3 finding deferred to the launcher: "Sibling parts that overlap will collide, because
> only parent–child pairs skip contacts."

## Trigger

The first consumer is an external physics game, a Pain-like launcher (an external
client; `docs/plan.md` §10 there forecasts this request at its F1). Its dummy is ten mesh parts held by
scene-authored `Joint`s. Every jointed pair can already skip contact (`Joint.collide: false`, the
default), but **nothing else can**. That leaves three cases:

- **Parts of one figure that are not jointed to each other** — the two upper legs, both jointed to the
  pelvis and overlapping at the hips; an upper arm swinging into the head. A pair like this collides
  every tick it overlaps: the resting-ragdoll jitter and locked hips `Plan/physics.md`'s own risk list
  calls the cheap-solver failure mode.
- **A figure against its launcher** — the dummy passing through the sling frame it sits in.
- **A query that should not see the caster** — the mid-flight grab probes for a grab point near the
  hand. `exclude: Option<EntityId>` drops one entity, and the dummy is ten.

The plan is built either when the external game files the request, or earlier on the user's word. The first two
cases are certain for any multi-part figure. The third is the same mechanism read from the query side.

## What exists (2026-09-24)

- `Collider { shape, body }` (`crates/ludion-spatial/src/collider.rs:282`) is the one physics author
  surface (`Plan/physics.md` §The author surface stays one component). It is hashed through the
  canonical JSON encoding (`crates/ludion-sim/src/hash.rs`), so an absent optional field hashes as
  before.
- `PhysicsWorld::create` (`crates/ludion-physics/src/world.rs:729`) builds every body from a `Collider`
  and sets no collision groups, so every collider is on rapier's default `InteractionGroups::all()`.
- The only pair filter is a joint's `contacts_enabled = joint.collide`
  (`crates/ludion-physics/src/joints.rs:87`).
- Queries (`crates/ludion-physics/src/queries.rs`: `ray_cast`, `shape_cast`, `overlap`) take
  `exclude: Option<EntityId>`. They are called only from that file's own doc tests and
  `crates/ludion-physics/tests/queries.rs`, so no game or engine caller exists yet.
- The scene `physics:` block (`space`, `gravity`, `solver_iterations`, `stabilization_iterations`,
  `sleep_after_ticks`; `docs/collision.md`) reaches the wrapper as `PhysicsSettings`
  (`crates/ludion-physics/src/systems.rs:61`). `apply_settings` re-applies it every tick. A malformed
  block is `L1033`. A restored session carries the block as the snapshot's `scene_physics` field.
- `ludion-spatial`'s 2D tier (`spatial::*`, `move_and_slide`) has no filtering either. It is out of
  scope here (§Non-goals).

## Proposed design

### 1. The matrix is scene policy; membership is per collider

A layer is a name declared once in the scene's `physics:` block, with the layers it ignores. A collider
names the one layer it belongs to:

```yaml
physics:
  space: free
  layers:
    - name: dummy
      ignores: [dummy, sling]
    - name: sling
    - name: props

entities:
  - name: dummy-upper-leg-l
    components:
      Collider:
        shape: { capsule: { half_height: 0.2, radius: 0.08 } }
        body: { rigid: { mass: 6.0 } }
        layer: dummy
```

- **A pair collides unless either side ignores the other's layer.** One declaration covers both
  directions, so the matrix cannot be half-written.
- **A collider without `layer:` is on the implicit layer `default`.** Other layers can name `default` in
  `ignores`. A scene with no `layers:` list simulates exactly as today, and no hash, trace, golden or blob
  moves; a scene that declares `layers:` but no `layer:` fields simulates identically, though its default
  colliders now carry explicit groups, so the physics blob's bytes differ.
- **One layer per collider.** Multi-membership (Godot's layer bitmask) is a second way to express the
  same matrix; it is left out until a slice needs it.
- Why a declared list rather than names discovered from colliders: a name is validated at check time
  with a nearest-match suggestion (a typo in `ignores` would otherwise silently collide), and a body
  spawned at runtime with a new name has no bit to take. The matrix also lives in one place, the way
  every collision matrix a designer has used before does (Unity's physics matrix, Godot's masks).

### 2. The rapier mapping (the wrapper's alone)

- Layer `i` in declaration order is bit `i + 1`; `default` is bit 0. That allows 31 declared layers.
- A collider's `InteractionGroups` are: membership = its bit; filter = every bit except the layers it
  ignores **or that ignore it** (the matrix is symmetrised at resolve time, so rapier's mutual-AND test
  gives the "either side" rule).
- These are **collision groups**, not solver groups. A filtered pair is never even detected, so it
  produces no contact events. That is the right reading for scoring: a pair the scene says never meets,
  never meets.
- A filtered pair beats a joint's `collide: true`. This is documented, not diagnosed.
- **Live edits.** The layer table is re-resolved from `PhysicsSettings` every tick, with gravity and the
  iteration counts. A changed table re-applies groups to every collider, and a changed `layer:` rebuilds
  that body through the existing `is_stale` path. An agent can therefore edit the matrix on a live
  session and see it on the next tick.
- **The snapshot blob does not change shape.** Groups live on rapier's colliders, which the blob already
  serializes, and the table is derived from the restored `scene_physics` block rather than stored. So
  the committed `.lrp` keyframes (`ragdoll.lrp`, `physics-stress.lrp`) stay decodable. This matters
  because no gate checks them yet (the owed replay-keyframe debt).

### 3. Queries filter by layer — one parameter, replacing `exclude`

`ray_cast`, `shape_cast` and `overlap` replace `exclude: Option<EntityId>` with `filter: &QueryFilter`:

```rust
let probe = QueryFilter::new().ignore_layer("dummy");   // or .exclude(entity), or both
let near = physics.overlap(&Shape::Sphere { radius: 0.3 }, hand, [0.0, 0.0, 0.0, 1.0], &probe)?;
```

- `QueryFilter` holds `exclude: Option<EntityId>` and a set of ignored layer names.
- An unknown name is `PhysicsError::QueryInput`, never an empty result — the queries' existing rule
  (`Plan/physics.md` W2 decisions).
- `exclude` moves into the filter rather than staying beside it: two parameters that both mean "skip
  this" would be two ways to do one thing, and no caller outside the crate exists to migrate.
- Result order is unchanged (casts by `(distance, entity)`, `overlap` by entity); the filter only drops.

### 4. Diagnostics

- `L1033` (the block is malformed) grows three cases, each with a JSON Pointer:
  - a duplicate or reserved (`default`) name;
  - more than 31 layers;
  - an `ignores` entry naming no declared layer (with a nearest-match suggestion).
- **New code — a collider's `layer:` names no declared layer.**
  - `check-scene` reports it with the nearest match.
  - At runtime, a spawned collider with an unknown layer gets no body. It takes the `ShapeRejection`
    path, is reported once per entity, and is retried when its `Collider` changes — the precedent a
    rejected shape already sets.
- **New code — `layer:` on a collider physics does not claim** (a 2D `circle`/`aabb` that is not
  rigid). The 2D tier has no filtering, so the field would be silently ignored. `check-scene` refuses
  it rather than let a designer believe it works.

## Waves

### W1 — layers in the solver

- `physics:` block `layers:` list: parse, validate (`L1033` cases), emit canonically (declaration order
  kept — it is the author's order and the bit order), carry through `scene_physics`.
- `Collider.layer: Option<String>`, `skip_serializing_if`. The component schema regenerates; the wire
  schema hash is expected not to move (`schema_snapshot.rs` is the judge). The drift test keeps
  `Collider {}` deserializing.
- `PhysicsSettings` gains the resolved table. `create` sets groups, and `apply_settings` re-applies them
  when the table changes. The two new codes are registered in `ludion-core` with `ludion explain` text.
- Tests (in `crates/ludion-physics/tests/layers.rs`):
  - two stacked boxes on a self-ignoring layer fall through each other, with no contact event;
  - a one-sided `ignores` filters both directions;
  - `default` can be ignored;
  - a live matrix edit takes effect next tick;
  - a `layer:` edit rebuilds the body at its pose and velocity;
  - no-layers scenes hash exactly as before (the existing pile, stress and ragdoll traces pass
    unchanged — that is the regression gate);
  - snapshot resume through a mid-run restore.
- A new pinned trace, `crates/ludion-physics/tests/traces/layers.txt`: a small jointed figure falling
  into a pile with self-ignoring parts, chaotic enough to stay varied (the pile scenes' kick rule).
  Its M4 half is owed.

### W2 — queries, docs, and the figure case end to end

- `QueryFilter` replaces `exclude` on all three queries. Unit tests: layer-filtered hits, an unknown
  name is an error, and `exclude` plus a layer compose.
- Docs:
  - `docs/collision.md` gains a Layers section (block, field, the either-side rule, the filtered pair
    emitting no events, the joint interplay).
  - `docs/physics-joints.md` gets the one-line joint interplay under Gotchas.
  - `dev-docs/ludion-physics.md` gains the group mapping. That topic is already about 177 B over its
    budget (`Plan/physics.md` W3 findings), so trim first, then add.
  - `check.mjs --fix` re-mirrors the embedded docs.
- The `ragdoll` gallery scene is **not** changed: its trace, golden and replay stay the cross-arch
  record of W3. The figure case is proven by W1's trace. The external game's own scene is the consumer's proof.

### W3 — closed as not needed: joint zeros survive a body rebuild

Not built. It was conditional on the external game keeping a kinematic seat (parts `kinematic` while seated,
`rigid` at release); the external game ruled otherwise (its request `0001`, ruling 3 below).

**Known limit.** A body rebuild — a `Collider` edit, such as a `kinematic` → `rigid` switch on a jointed
figure — still re-captures each joint's zero at the current pose (`crates/ludion-physics/src/world.rs`,
the `needs_attach` arm; `docs/physics-joints.md` §Gotchas), so a crouched seat skews every limit. The
fix, were a game to need it: `JointEntry` keeps the `GenericJoint` it built, a re-attach after a cascade
removal reuses it, only an edit to the `Joint` itself re-captures, and `ragdoll.lrp` is re-recorded
(hash-neutral, last, after the schema snapshot). The engine-free alternative is to hold a rigid figure
in place with a `fixed` joint and destroy that joint at release, so no body is ever rebuilt.

## Gates

`Plan/physics.md` §Gates every wave carries applies unchanged:

- a cross-arch trace for the new `layers` trace (the M4 half owed);
- snapshot resume;
- 1/2/8-thread identity (the existing gate covers it);
- the resolved-feature gate (no new rapier feature is needed — `InteractionGroups` is core).

Every existing physics trace, golden, budget and `.lrp` passing untouched is the proof that an unlayered
scene is unchanged. `node scripts/check.mjs` is green at each wave's end.

## Non-goals

- **Filtering in `ludion-spatial`'s 2D tier.** No 2D game has asked; the `layer:`-on-unclaimed error
  keeps the gap loud.
- **Multi-layer membership, solver-only groups** (detect but do not resolve), and per-pair
  `ignores` overrides on a collider. Each is a second way until a slice needs it.
- **The mesh-figure document** (the external game's other forecast request, a reusable part list with joints and
  layers). It is its own plan when the external game's second scene needs the dummy.

## Risks

- **A filtered pair the author forgot.** A pair that should collide but was filtered is silent by
  construction — there is no contact event to notice. Mitigation: `check-scene --bounds` (or the
  `engine.describe` physics section) prints the resolved matrix, so an agent reads which pairs never
  meet without running a launch. This belongs in W1 if the output is cheap to add.
- **Bit order is declaration order.** Reordering `layers:` changes rapier's bits but not any result:
  filtering depends only on the symmetric matrix, and group bits are not hashed. A test pins that a
  reordered list produces an identical trace.

## Rulings (user, 2026-09-24)

1. **Scene-level matrix + one `layer:` per collider**, as proposed in §Proposed design 1.
2. **Build now** — W1 and W2 ship without waiting for the external game to file the request; execution is driven by
   a fresh tech-lead instance.
3. **W3 waits** for the external game's seat decision at its F1; not built as a standalone trap fix. Superseded by
   ruling 7.

Rulings of 2026-09-25:

4. The resolved matrix prints from the CLI only (`check-scene --bounds [--json]`), not `engine.describe`.
5. The external game filed request `0001` (2026-09-24); its `## Shipped` section records the ship; the docs topic is `ludion docs collision-layers`.
6. `L1187` is static-only (`check-scene`); the runtime never raises it.
7. **W3 is closed as not needed.** the external game's seat decision: the goblin is a rigid ragdoll from load and
   release removes the sling joint, so no body is rebuilt and the joint-zero re-capture never arises
   for it. No other consumer needs it; the trap stays a documented known limit (§W3).

## Status

**Sensors shipped** (reversing the earlier non-goal): a sensor sits on one layer and follows the same table; see `docs/physics-sensors.md`.

**W1 shipped** in Lore revs 854–856: the data half and validation (`physics.layers`, `Collider.layer`,
`L1033`, `L1186`, `L1187`), the check verbs (`check-scene --bounds [--json]` matrix), and the solver
half (rapier collision groups, runtime `L1186`, the `layers` trace whose M4 half is in the owed ledger).
**W2 shipped**: `QueryFilter` on `ray_cast`, `shape_cast` and `overlap` in place of `exclude`, and the
docs (`ludion docs collision-layers`, `ludion devdocs ludion-physics`). **M4 half verified** (2026-09-26, rev 916): the
`layers` trace reproduces unblessed there. **W3 closed as not needed** (2026-09-25): the external game's request `0001` ruled
out the kinematic seat; nothing built, the joint-zero re-capture stays a known limit.
