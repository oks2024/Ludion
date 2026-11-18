# Plan: Physics — deterministic rigid bodies on the sim path

> Status: **decided 2026-09-21 — adopt `rapier3d` behind a `ludion-physics` wrapper crate; W1–W4 mechanized.**
> What exists today is `ludion-spatial`'s 2D plane collision and nothing else. This plan records why a
> physics engine is compatible with the bit-exact sim tier (and which half of that argument is now
> rapier's job), the decision and what it gave up, the author surface, the waves re-cut for a wrapper,
> the gates, and the risks of a dependency. Every claim of current-code fact was read from the tree on
> 2026-09-14 and re-checked on 2026-09-21. The first consumer is a Pain-like launcher (its section
> below); the open-world slice (a private notebook) takes the vehicle body on its
> own schedule.

## Trigger criterion

Start when a scheduled vertical slice needs **dynamics**: bodies that move under forces and
contacts rather than under game code (vehicles, thrown or falling props, ragdolls). Neither vigil nor
the external shmup asked for it; every collision need so far was met by queries plus
`move_and_slide`. The trigger is content demand, the same rule textured PBR and OGG audio follow.
The trigger fired: the launcher is the scheduled slice.

## Why it is compatible with determinism

What makes a physics engine non-deterministic is never the arithmetic. It is unordered iteration
(hash-map contact caches), work-stealing threads, platform libm, fast-math and FMA contraction. The
sim path already bans every one of those (`dev-docs/determinism.md`), and the cross-arch replay
gates prove glam-heavy sim code hashes identically on the RTX 3090 box and the Apple M4. With a
dependency doing the solving, the argument splits in two: what rapier has to guarantee and we verify,
and what stays ours.

**Rapier's job, verified rather than owned** (the evidence is in Status):

- **Ordered iteration inside the solver.** `enhanced-determinism` selects `IndexMap`/`IndexSet` with a
  fixed hasher; the dependency audit found no reachable `HashMap`/`HashSet`.
- **Single-threaded solving.** (0020: superseded — `parallel` builds beside `enhanced-determinism` and the step runs on a pool `ludion-physics` owns, sized `par::threads()`; see Decided (Serious Isaac 0020).)
- **Pure-Rust libm inside the solver**, and no `mul_add` in rapier's or parry's own sources.
- **Cross-architecture bit-identity for the operations a real scene exercises.** The cross-arch trace
  is the empirical answer to the one thing reading could not settle (`wide`'s internal fused paths).
  It is evidence for **the scene that was traced**, not a proof about the library; §Risks says what
  follows.
- **Warm-start impulses, island state and the contact graph survive a serialize/deserialize round
  trip well enough to resume bit-identically.** Ours to prove, because upstream does not test it.

**Ours, and unchanged from the original contract:**

- **Fixed timestep, fixed substep count.** `IntegrationParameters::dt` is
  `TickRate::seconds_per_tick()`, set once; no render-rate stepping; the renderer may interpolate
  presentation only.
- **The mirror order is ours.** The handle table is a `Vec<(EntityId, RigidBodyHandle)>` in
  **creation order**, fed in spawn-rank order, and it is the only order that reaches an output. Nothing whose order reaches an
  output iterates a rapier set unsorted — rebuilding the table by walking a set would be exactly
  the determinism bug this contract forbids — so the table is captured in the snapshot, never
  recomputed. The two order-insensitive walks are `record_detection_poses` and
  `collect_contacts`. This is the wrapper's invariant, not an implementation detail.
- **The ECS owns authored state; rapier owns derived state.** One direction per tick: game code
  writes `Velocity` (and pose for kinematic bodies), the write system pushes it in table order, the
  step runs, the mirror pulls poses and velocities back. Two owners of position is the
  `Arc<Mutex<World>>` smell in another shape; the system order is what makes it one owner per phase.
- **Sleep and wake are authored in ticks.** Rapier's sleep is time-based: `RigidBodyActivation`
  accumulates `time_since_can_sleep += dt` each step and sleeps at `>= time_until_sleep` (default
  0.5 s). `dt` is ours, so no wall clock reaches the accumulator and the sum is a fixed sequence of
  identical float additions — effectively a tick count. The tunable is `sleep_after_ticks` (an
  integer), converted once at world-build time to `time_until_sleep = (ticks as f32 - 0.5) * dt`.
  The half-tick offset is deliberate: repeated addition of `dt` is not bit-equal to `n * dt`, so a
  threshold at exactly `ticks * dt` could land on either side of the `>=` by an ulp; offset by half a
  tick, the crossing tick is unambiguous. The linear sleep threshold is scaled by
  `IntegrationParameters::length_unit` (kept at 1.0, metres); the angular threshold only applies to
  collider-less bodies.
- **Solver state is snapshotted.** The `DeterministicRng` precedent, now as an opaque field (see W1).
  `WorldHash` still excludes resources; a diverged cache shows up in the body components one tick
  later, which the per-frame hash trace catches.
- **Tunables are data, not code** (solver iteration counts, thresholds): a change is a data diff and
  a re-bless, never a silent drift.

## Decision: adopt rapier (2026-09-21)

**Ruled by the user, 2026-09-21: adopt `rapier3d`, pinned with `=` (today `=0.35.3`), features
`dim3 f32 std enhanced-determinism serde-serialize` (0020: plus `parallel`, on a pool we own; see Decided (Serious Isaac 0020)), never a SIMD feature, behind a
`ludion-physics` crate that is the only place in the workspace naming a rapier or nalgebra type.**
The author surface stays ours (below) and rapier stays replaceable — that is what the wrapper buys.

**Evidence** (Status has every number): the traces are identical cross-arch on both pinned machines;
the denylist audit found zero reachable violations under the feature set; the pose mirror costs about
one percent of the step; snapshot resume is byte-identical from serialized state; and the ECS→rapier
write path is cheap and reproduces cross-arch.

**What was given up:**

1. *Single-threaded solving.* (0020: the "mutually exclusive" claim is false for 0.35.3 — the two
   compile together — and the ceiling is now a threaded step on `par::threads()` workers; see
   Decided (Serious Isaac 0020).) As first measured, single-threaded, the ceiling: about 2 000 bodies costs 4.6 ms mean and
   9.6 ms p99 of a 16.7 ms tick on the reference box, which is the practical body budget for a 60 fps
   scene with a game on top of it. For content: hundreds of props are comfortable, a few thousand is
   the wall — and the wall is a *measurement on one box and one scene*, not a constant.
2. *Bit-exactness is pinned, not owned.* We hold the version with `=`, the feature set, an audit and
   two gates; we do not hold the arithmetic. Every pin move re-opens the question, which is what the
   pin-upgrade ritual (W4) exists for.
3. *A dependency the agent cannot read.* Rapier and its ~25 crates do not fit in an agent's context,
   so the wrapper's public surface has to be self-sufficient — §Risks carries the rule.

**Why not the alternatives.** Jolt / PhysX over FFI is ruled out: it breaks "Rust all the way down",
puts a C++ toolchain in the iOS and Mac builds, and its determinism is per platform and compiler, not
cross-arch. Growing `ludion-spatial` into our own solver was the standing recommendation until W0
measured; the audit and the mirror cost removed the weight of its premises, and building a solver
while a measured, deterministic one exists is the work the wrapper avoids.

**The named exit.** Replacing rapier behind the crate would mean reimplementing the wrapper's public
surface — Box2D v3 as the reference design (sequential impulses / soft-step, islands, sleeping,
warm-starting), plus the shapes, joints and queries the surface exposes — with every existing gate
already serving as its acceptance test, and every physics scene, golden and `.lrp` re-blessed because
hashes move. What starts it, any one of: (a) a scheduled slice needs more than the measured
single-threaded ceiling *and* profiling shows the solver, not our code, is the cost; (b) a pin upgrade
breaks cross-arch identity or snapshot resume and upstream will not fix it; (c) a platform we must
ship on cannot build it. **No trigger is armed today.**

## Crate graph

`ludion-physics → ludion-core, ludion-ecs, ludion-spatial`, plus `rapier3d` (and `bincode` for the
snapshot bytes). `ludion-engine → ludion-physics` is an unconditional edge, never the reverse and not
a cargo feature: two seams need it — `builtin_registry()` and the snapshot field — and a component
registered only under a feature would be a scene and wire type some builds lack, which the pillar
forbids. The price is
compile time on about 25 CPU-only crates; none is on the mobile check's desktop-only list, so the
`--no-default-features` mobile shape is unaffected in kind. W1 records the `cargo check -p
ludion-engine` delta as a number; if it breaches the fast-iteration rule the answer is to revisit the
edge with a measurement, not to pre-emptively gate it.

**Boundary rule:** `ludion-physics` is the only crate that names a rapier or nalgebra type. It gets its
own strict `clippy.toml` copied from `crates/ludion-sim/clippy.toml` — clippy config does not inherit
from the workspace root, so a new crate silently has no denylist until one is copied in. That denylist
bans type paths in *our* code and cannot see inside a dependency, so a mechanical resolved-feature gate
stands in for it (W1).

## The author surface stays one component

`Collider` is the one surface (`crates/ludion-spatial/src/collider.rs`); nothing gets a second
component for "physics colliders". Before adding a way, remove the first:

- `shape` gains 3D variants: `sphere`, `box` (3-vector `half_extents`), `capsule`, `cylinder`, `convex`
  (hand-authored `points`; the assembly ragdoll switch writes its own; no hull is generated), `heightfield`,
  `compound`. `circle` and `aabb` stay
  as the in-plane forms and validate under the same `L1116`.
- `body` becomes `static | kinematic | rigid`. Today's `dynamic` is renamed `kinematic` (moves
  under game code; the character controller case) with a migration in the restore report. `rigid`
  carries `mass`, `friction`, `restitution`, `linear_damping`, `angular_damping`, and `lock`
  (per-axis translation/rotation locks).
- **2D games keep the plane.** `Engine::builder().plane(...)` locks the off-plane translation and
  the two in-plane rotations on every rigid body, so a 2D game is the 3D solver with axis locks,
  not a second engine.
- Velocities are a `Velocity` component (linear + angular), hashed like any component, so a game
  can read and write them without touching the solver. Forces are applied through it, not through a
  force accumulator API.
- **Where the types live.** `Collider`, `Velocity` and the joint components stay in `ludion-spatial`
  as pure data; registration stays a single site in `builtin_registry()` and the content-speed crate
  stays rapier-free. `ludion-physics` reads them and owns the mapping to parry shapes. The crate name
  now slightly under-describes `ludion-spatial`; a rename is not worth it.
- **The shape mapping is the surface's contract:**

  | `Collider` shape | rapier shape |
  |---|---|
  | `sphere` | `Ball` |
  | `box` | `Cuboid` (3-vector half-extents) |
  | `capsule` | `Capsule` |
  | `cylinder` | `Cylinder` (axis along local `y`, like `capsule`) |
  | `convex` | `ConvexPolyhedron` (over the authored points) |
  | `heightfield` | `HeightField` |
  | `compound` | `Compound` (one collider on one body, parts at Euler-degree offsets) |
  | `circle`, `aabb` | the same `Ball` / `Cuboid`, off-plane extent taken from the plane's locks |

  Every size keeps the `L1116` positive-and-finite rule; new invariants (a `convex` hull that
  degenerates, a heightfield whose grid is ragged) take the next free content code.
- **Sleeping is authored in ticks** — `sleep_after_ticks`, converted as above.

## The 2D queries and `move_and_slide`

`ludion-spatial`'s index and its four queries (`overlaps`, `raycast`, `k_nearest`, `move_and_slide`)
**stay exactly as they are** and do not grow a 3D case. They are a per-frame uniform
sparse grid over `StableMap`, pinned candidate order, and an axis-separated, x-biased test-and-cancel
against statics that is a pure read — a game applies the returned delta itself. Vigil's horde runs them
thousands of times a frame over a structure rebuilt from `Transform`s. The vertical slices running on
them are not re-blessed by this plan.

`ludion-physics` owns every 3D query: ray cast, shape cast and overlap against the rapier world, in a
pinned order the wrapper defines — never rapier's internal iteration order.

**Two tiers, not two ways.** `spatial::*` answers "what is near this point on the plane this frame"
with no bodies, no solver and no physics world; `physics::*` answers queries against a simulated
world and exists only when one does. A scene uses one or the other, and an entity never has both a
`spatial` index entry and a rigid body. Mechanically, `spatial::rebuild` skips any collider whose shape
is a 3D variant or whose `body` is `rigid`, so a physics scene that also registers `rebuild` indexes
nothing rather than indexing a lie.

**The character controller.** Kinematic bodies keep using `move_and_slide`; rapier's
`KinematicCharacterController` is **not** adopted — swapping it in would move every existing game's
hashes and committed replays for a 2D plane game the current resolution already serves exactly, and the
current one is specified to the ulp in its own doc comment.

**Accepted gap:** a **3D** kinematic character controller has no answer in this plan — rapier's
`KinematicCharacterController` against a 3D `move_and_slide` of our own is decided when the open-world
slice asks for one, not before. **Answered by `Plan/character-controller.md` (2026-09-29): our own motor
over `shape_cast`.**

## First consumer — a Pain-like launcher (ruled 2026-09-14)

The first physics game is a Pain-like (aim a slingshot, launch a voxel figure, score on what it
hits), not the vehicle game. Why: one mechanic that is all physics (no AI, navigation, streaming or
quests, so the effort lands in the solver); voxel assemblies are already ragdoll skeletons — rigid
parts on clips become rigid bodies joined by limited joints with no change to the character format;
ragdolls and stacked props are where cheap solvers fail (jitter, joint explosion, piles that never
sleep), so it is the quality oracle a vehicle would hide; and a launch is one input vector with a
deterministic score, so the agent tunes a level by sweeping launch angles and reading score
distributions and hit maps without looking at the screen — replays and hashes verify every launch.
The vehicle body waits for the open-world slice. What the choice pins on the engine:

- **Contact events carry impulse magnitude** and the pair, in a pinned order, from the dynamics wave
  (W2) — scoring is the event stream's first consumer, not an afterthought.
- **Joints are components, created and destroyed at runtime through the existing
  `entity.spawn`/`entity.destroy`/`component.*` commands, not scene-only data.** The mid-flight grab
  creates a joint between two bodies during play.
- **Sleeping and body counts are a budget.** A level holds hundreds of props; the `physics-stress`
  scene and both boxes' budgets land in W2. The jointed-ragdoll cost is in the ragdoll bench (Status,
  Serious Isaac 0017); its budget file is owed to the pinned run.
- **Ragdolls are the assembly plus a joint list**: each part a `rigid` body, joints with limits and
  a break impulse authored beside the `clips:` map. A posed clip and a ragdoll are the same parts
  under two owners; the switch is game code.
- **Outside physics:** a follow camera tracking a flying body (game-side, the first non-plane camera
  any game asks for), slow motion through `PhysicsWorld::scale_time`/`freeze` (post-W4; `Speed` only
  paces a live scheduler), and no mesh destruction —
  knocked-over props and breakable joints carry the effect; destruction stays deferred.

## Engine/game boundary

Engine: shapes, the world, the step, joints, sleeping, the vehicle body, the queries, schema,
diagnostics. Game: what happens on contact (a contact event stream on the world, consumed in
registration order), character controllers (kinematic + `move_and_slide`, unchanged), vehicle tuning,
gameplay forces. AI and navigation stay out of this plan. The engine half is now the *wrapper's* half:
shapes, the world, the step, joints, sleeping, the queries, schema and diagnostics are
`ludion-physics`; rapier is an implementation detail no game, no scene and no protocol payload can
observe.

## Joint-motor pose blends (Serious Isaac 0026)

Two hashed, sim-only components beside `JointMotor` (in `ludion-spatial`): `JointPoses` on each jointed part (named poses in `JointMotor.target` units and arity, plus `stiffness`, `damping`, optional `max_force`) and `PoseBlend { pose, weight }` on the group root. The engine system `pose_blend` runs immediately before the physics triple (physics scenes only, after every game system) and writes each part's `JointMotor = { pose·w, stiffness·w, damping·w, max_force }` only when it differs from what the part carries, removing it at `w == 0` so a limp group can settle and sleep (`ludion docs physics-joints` "Pose blends").

- **Group rule.** A part's group root is `JointPoses.group`, else `SkinBone.skin`; neither means the part is inert. Why: `switch_to_ragdoll` leaves no part-to-root link, and adding one would move the WorldHash of every existing voxel ragdoll, so a voxel game sets `group:` on the returned parts.
- **Rest-relative targets.** Poses are measured from the joint's captured zero; `switch_skin_to_ragdoll` switches at the rest pose, so a pose's zero is the rig rest. Any future posed switch must keep rest-relative zeros. `JointZero` and `LimitsFrom::Rest` keep the zeros rest-relative for the posed voxel switch (`Plan/paired-animations.md`).
- **`L4035`** warns once per (group, pose) when a `PoseBlend` above weight 0 names a pose none of its parts carries; weight 0 never warns.
- **Later addition:** a pose from a clip (`clip at t` through the sim-side sampler).
- **Out of scope:** a contact-material blend; engine-driven weight ramps.
- **Voxel settle.** Free voxel ragdoll hulls can jitter awake indefinitely depending on placement: with no `PoseBlend` the `ragdoll-pose-blend` gallery figure at x=-0.5 never sleeps in 2400 ticks (the skin sleeps from tick 551, the wall-pinned figure from 72), while the held-then-zero blend settles all three groups by tick 349. A limp blend therefore does not guarantee sleep in general; 0017's age fallback still retires a corpse that stays awake.

## Decisions

**Proposed (Serious Isaac 0017, owner approval owed).** An engine ragdoll pool, ready to build once approved. The game-side reference is `crates/ludion/examples/ragdoll_budget.rs` (`ludion docs physics-joints` "Ragdoll budget"):

- (i) A `RagdollPool` resource and a `RagdollGroup` handle, registered by `switch_to_ragdoll`/`switch_skin_to_ragdoll` in admission order (tick, then call order). Callers may ignore the handle.
- (ii) A hashed `physics.ragdolls:` block: `max_simulated`, `max_retired`, `max_sim_ticks`, `evict: oldest_unpinned`. The age fallback exists because the bench found 2–5% of parts never sleep. Unset is byte-identical to today; the schema moves.
- (iii) Retire drops the bodies and joints and keeps `Transform` + `SkinBone`; the pool state is snapshot-restored.
- (iv) Over the cap: retire the oldest settled, then evict the oldest unpinned simulated group; a pinned group (a part jointed to a static body) goes last.
- (v) `RagdollRetired { group, reason: Settled | Expired | Evicted }` and `RagdollDespawned { group }` in a per-tick resource, like `AnimationEvents`.
- (vi) One `L####` warning once per session when evictions outpace settles (number chosen at build).
- (vii) Re-admission is rest-zeroed: joint zeros are captured at the rig rest, not the retired pose (the 0017-01 caveat).
- (viii) Mapping from the example: `RagdollBudget` → `ragdolls:`, `RagdollGroup.state` → the events' reasons, its three systems → the engine pass.

**Decided 2026-11-11 (Serious Isaac 0020, owner-approved). A threaded step.** The figures are in Status "Rapier parallel spike". The measured outcome is identical: every committed trace, replay and snapshot restore matched at every pool size.

- (i) The workspace `rapier3d` entry gains `parallel` for every build (one build shape), and the `Cargo.toml` comment is rewritten.
- (ii) `ludion-physics` gives rapier a dedicated pool sized `par::threads()` in `PhysicsWorld::new` and after every decode/restore, and never leaves it on rayon's global pool, so `LUDION_THREADS` and `--threads` govern both. A pool of 1 is built when `par::threads() == 1`, because a `parallel` build with no pool falls back to the global one.
- (iii) `scripts/physics-features.mjs` gets a named carve-out: the expected rapier3d/parry3d feature sets gain `parallel`, and `rayon`/`rayon-core` may be reached only through `ludion-physics → rapier3d (→ parry3d)`. Any other path fails, with the path printed.
- (iv) `Plan/deterministic-job-system.md` (the "no rayon … no external-scheduler carve-outs" line) gains its first named carve-out: rapier's internal scheduler, whose pool `ludion-physics` owns, gated by thread identity. Rayon types stay banned in our code.
- (v) `Cargo.lock` and `fuzz/Cargo.lock` gain the rayon packages.
- (vi) Gates on both pinned boxes: the features gate, `physics-pin-ritual.mjs` (expect PASS, not MOVED), the thread-identity tests, the cross-arch traces, snapshot resume, and the `.lrp` corpus.
- (vii) Re-bless: no hash moves, so nothing re-records. Only the physics budgets (`pinned-*-physics-stress.toml`, the horde budgets) are re-measured, in the pin ritual's order.
- (viii) The plan text it amends: the adopt-rapier decision's feature list, give-up 1, the W1 note on `parallel`, the W4 "no island parallelism" line, the non-goal, `dev-docs/physics-pin.md` §features and §ceiling. It also names the stale "`enhanced-determinism` and `parallel` are mutually exclusive" claim (`Plan/physics.md`, `dev-docs/physics-pin.md`, the root `Cargo.toml` comment) as the docs fix that adoption carries.

**Decided 2026-11-10 (Serious Isaac 0035, owner-approved). An opt-in angular sleep gate, recommended value 2.4 rad/s.** Measured in `crates/ludion-physics/tests/ragdoll_sleep.rs`: a limp ball-jointed ragdoll's small shin capsules keep rocking at about 1.6 rad/s (farthest-point speed only 0.05–0.07 m/s). For a body with a collider, rapier's `RigidBodyActivation::update_energy` hard-codes `|ω| < π/2`, so no linear threshold (`RigidBody.sleep_threshold`, which saturates at 0.1) can sleep them. With 0.1 and `sleep_after_ticks` 15 the calm/shot/curl repro sleeps at 128/160/150 ticks against the 120 the game asked for.

- (i) Vendor patch: the collider branch's gate becomes `|ω| < max(π/2, angular_threshold)`. The default `angular_threshold` (0.5) gives `π/2`, so every existing body, trace and `WorldHash` is unchanged. `LUDION-PATCH.md`/`.diff` record the change.
- (ii) Additive `RigidBody.sleep_angular_threshold: Option<f32>` (rad/s, `>= 0`), written to `activation.angular_threshold` at creation and in place, like `sleep_threshold`.
- (iii) Measured with `sleep_threshold` 0.1 and `sleep_after_ticks` 15: calm/shot/curl sleep at 123/132/129 ticks at 2.4 (the least value at which all three have plateaued; at 2.3 calm is still 126, shot and curl plateau from 1.9 and 1.8). Past it the linear/drift gate binds, so the 120-tick budget is not reachable by the angular gate alone; the fastest combination swept, 0.15 / 8 / 2.4, gives 108/124/120.
- (iv) Risk: a body slept mid-rock freezes visibly at up to that angular speed. That is acceptable for corpses, and only opted-in bodies are affected.

**Decided (Serious Isaac 0043). Physics order follows spawn order, never slot values.** The write system builds, rebuilds, pushes and wakes bodies, and inserts joints, in ascending ECS **spawn rank**, a monotonic per-occupancy stamp that `World::spawn` takes. Blasts and partner wakes follow spawn rank too: the write passes ranks to the physics world as a side table refreshed every write, never snapshotted, and a restore has the engine refill it from the restored World (`sync_spawn_ranks`) before applying settings; the motor reads `World::spawn_rank` directly, because it runs before the write. So rapier's whole call history, and with it its arena handles, islands and contact order, is invariant under unrelated entity churn.

The creation-order handle table remains the only iteration order; only the order in which it is fed changes. Ranks are not hashed. They ride the snapshot as an optional `spawn_order`, omitted when it equals slot order. Event and query lists keep their documented `EntityId` order. One-time cost: worlds that respawn claimed bodies into reused slots shift once, and the `.lrp` corpus re-records. The ECS freeze list gains "spawn rank" (`engine-architecture.md` §4), as a named one-off.

## Waves

W0 and its extension are the measurement the decision was made from; Status carries the record.

### W1 — the crate, the world, and one hashed scene

The thinnest thing a game can run.

- `crates/ludion-physics/` created with its strict `clippy.toml` and its devdoc topic. Graph as in
  §Crate graph; `rapier3d` and `bincode` pinned in `[workspace.dependencies]`, rapier with `=`,
  `default-features = false`, features `dim3 f32 std enhanced-determinism serde-serialize`.
  `bincode = "1"` is new; a bincode major bump changes the bytes and is therefore a snapshot-format
  event.
- The world resource: the rapier sets, `IntegrationParameters` built from `TickRate`, and the
  creation-order handle table. Resources are not hashed; the mirror is what reaches the hash.
- The system triple `write → step → mirror` is order-critical and a wrong order fails silently, so the
  game registers **one verb**, `physics::install::<Transform>(&mut schedule)`, which appends the three
  in the fixed order at the position the game chooses — the `spatial::rebuild` precedent without its
  footgun.
- `Collider` gains the 3D shapes and `body: rigid`; `Velocity` lands hashed; the builder's plane
  becomes axis locks on every rigid body.
- **The snapshot field.** `SnapshotDoc` (`crates/ludion-engine/src/snapshot.rs`) gains one additive
  `Option<String>` beside `profile`, carrying **base64 of bincode**, absent when the session has no
  physics world. The precedent chain is `rng` → `profile` → `allocator` → `doc_reloads` → `residue`:
  each is optional, `skip_serializing_if`, invisible to `WorldHash`, and a document without it
  restores as absence, so `SNAPSHOT_FORMAT` stays `1`. It is opaque text because structured JSON is
  closed by the type (see Status). Base64, not a JSON byte array, because `serde_json` writes a
  `Vec<u8>` as an array of numbers — worse than the JSON text. `base64` is already a workspace
  dependency. **The extension's resume proof was obtained on the JSON path and does not transfer across
  codecs; the resume gate re-establishes it on the bincode path — that is its first job, not a
  formality.** A snapshot is also a replay keyframe (`ReplayRecord::Keyframe`, every
  `KEYFRAME_INTERVAL = 300` frames, zstd'd per record) and `engine.snapshot` returns the document as a
  wire response; both carry the blob (§Risks).
- **The mechanical feature gate:** `scripts/physics-features.mjs`, run by `check.mjs` and mirrored into
  `.github/workflows/ci.yml` and `ci/scripts/` per the gate-set rule, asserting over `cargo metadata`
  that `rapier3d`'s resolved feature set is exactly the expected one and that `rayon` and `hashbrown`
  appear nowhere in `ludion-physics`'s normal dependency tree (0020: `rayon` is now reachable only
  through the named `rapier3d`/`parry3d` carve-out). It exists in place of a denylist because
  clippy bans type paths in our code and has never seen inside a dependency
  (`dev-docs/determinism.md` §Gotchas), and cargo unifies features across the workspace: one crate
  anywhere enabling `parallel` would turn rayon on for every build, silently (0020: `parallel` is
  now expected and the gate asserts the pool carve-out instead).
- Verifiable by: a gallery physics scene with a per-tick hash trace, identical on both pinned machines;
  the snapshot-resume trace on the bincode path; the thread-identity gate; the recorded
  `cargo check -p ludion-engine` delta.

### W2 — the dynamics surface complete, and the budgets

- The remaining shapes and their `L####` validation (the next free content code); forces and impulses
  through `Velocity`; the contact event stream — pair plus impulse magnitude, in a pinned order — as a
  resource on the `AudioQueue` precedent in mirror image (a resource the step system fills, read by
  game systems in registration order, where `AudioQueue` is filled by systems and drained by the
  engine).
- Queries against the physics world (ray cast, shape cast, overlap) in the wrapper's pinned order.
- Sleeping tunables in ticks; solver iteration counts as scene data.
- `docs/collision.md` grows its dynamics half (the 2D section stays first); the devdoc topic covers the
  wrapper's internals and the rapier mapping.
- `examples/gallery/physics-stress.yaml` (the launcher's prop-pile scale) plus
  `budgets/pinned-rtx3090-physics-stress.toml` and the M4 twin — measured on each box, never derived,
  per `dev-docs/budget-files.md`. They land here, not last, because the launcher's prop piles are the
  first body-count test.
- Verifiable by: a gallery golden and a `sim-hash` trace per new scene on both boxes; the budget files
  green; the event stream's order asserted by a test, not by inspection. Each new scene also carries
  gates 2 and 3 (resume at a live tick, 1/2/8 identity); the M4 halves are owed.

### W3 — joints and the ragdoll

- Hinge, ball, slider, fixed — limits and a break impulse each — as components, created and destroyed
  at runtime through the existing `entity.spawn`/`entity.destroy`/`component.*` commands, so a
  mid-flight grab can form one at runtime.
- Ruled 2026-09-23: no joint commands — a joint is a component on a carrier entity, so `entity.spawn`
  is the one way; the schema hash does not move.
- The ragdoll: an assembly's parts as rigid bodies under its joint list, and the clip-to-ragdoll switch
  as game code. A `ragdoll` gallery scene with a replay.
- The raycast-wheel vehicle body (chassis rigid body, per-wheel suspension ray, tyre model, a `kind:`
  document for its tunables) stays on the open-world slice's schedule.
- Verifiable by: the ragdoll replay reproducing on both boxes; a joint created by command surviving a
  snapshot/restore round trip.

### W4 — the ceiling, the ritual and the exit

This replaces the old "parallel islands" wave: no island parallelism exists on this route and none is
planned, because `enhanced-determinism` excludes it. (0020: that exclusion is false; the step itself now runs threaded, see Decided (Serious Isaac 0020).)

- The body-count ceiling written down and *gated*: the stress budget on both boxes is what fails when a
  content change walks past it, and the `physics` row in the profile report (`dev-docs/profiling.md`'s
  sim per-system layer) says where the time went.
- The `.lrp` size line: a physics replay's keyframes carry the blob, so the stress scene's recorded
  replay gets a size ceiling too.
- **The pin-upgrade ritual**, the thing that protects an adopted dependency:
  1. Bump the `=` pin in `[workspace.dependencies]` on its own.
  2. Run the resolved-feature gate.
  3. Re-run the cross-arch trace on **both** pinned machines, for every physics scene.
  4. Re-run the snapshot-resume trace on the bincode path.
  5. If and only if 2–4 pass, treat the hash movement as an ordinary hash-moving change and re-bless
     goldens, budgets and the `.lrp` corpus last, after the schema snapshot.

  A failure at step 3 or 4 means the pin does not move and the finding is recorded; it is also exit
  trigger (b).
- Closed when the ceiling is gated on both boxes, the ritual has been exercised once end-to-end, and no
  exit trigger from §Decision is armed.

### Post-W4 — the external game's physics requests

Requests from the external game's `ludion-requests/`, one at a time, each a minimal engine answer.

- `0004` joint motors — a `JointMotor` on the joint's carrier, pushed onto the live rapier joint
  without a rebuild.
- `0005` impulses — `PhysicsWorld::{apply_impulse, apply_impulse_at_point, radial_impulse}` queue calls the
  write system applies before the step; a `Blast` with a `Falloff` and `blast_hits`; `L3038`.
- `0007` contact point and normal — a `Started` `ContactEvent` carries `point` and `normal`, named and
  oriented like `QueryHit`, read from one solver contact of the pair.
- `0006` time scale — `PhysicsWorld::{scale_time, freeze, last_step_dt}` slow or skip the physics step alone for
  a frame, in the `GameClock::hold` idiom.
- `0008` per-body gravity scale — `RigidBody.gravity_scale` multiplies the scene's gravity for one body
  (`1.0` default, omitted by the writer; `0.0` floats, negative rises), applied in place by a live edit of
  that field alone.
- `0003` cylinder and compound colliders — `Shape::Cylinder` and `Shape::Compound`, one body and one mass over
  several parts; `L1189`.
- `0019` scaled-step energy — a `scale_time` step retunes each joint's softness so its drift correction
  keeps the full-step gain, instead of amplifying an impact's joint stretch by `1/scale`.
- `0018` ragdoll settle — rapier vendored under `vendor/rapier3d/` with an exact multi-axis ball-limit and
  motor jacobian, so a resting ragdoll no longer gains energy.
- `0017` heightfield ray on a grid line — parry3d vendored under `vendor/parry3d/`; heightfield ray casts
  re-seat their start cell on the grid's own boundaries, so a ray on a grid line hits.

## Gates every wave carries

1. **Cross-arch trace, both pinned machines.** Every new physics scene gets one, not just the first:
   the cross-arch evidence is empirical and per-scene (it is what stands in for reading `wide`'s
   internal fused paths), so a new scene is new arithmetic. An empty diff only passes if the trace is
   varied, because the hash canonicalizes every non-finite float to one value — the check is the
   distinct-line count.
2. **Snapshot-resume trace.** Capture at a live tick, drop everything, rebuild from bytes, continue —
   the continuation byte-identical to the uninterrupted run. Upstream tests capture portability and not
   resume, so this gate is the only thing standing behind a resumed physics session, and it re-runs on
   every pin move.
3. **1/2/8-thread identity.** Trivially true for a single-threaded solver, and it still guards that the
   *rest* of the tick (game systems, `par` regions, extract) stays thread-count invariant with physics
   in the frame, that no physics-adjacent code reached for a thread, and that no path — a feature, a
   transitive enable, an environment variable — has quietly put rapier on a second core. The feature
   gate and this gate answer the same question from two directions.
4. **The pin-upgrade ritual** (W4): gates 1 and 2 re-run under a named procedure.

## Deferred / non-goals

- Cloth, soft bodies, fluids, destruction: never in this plan; each needs a written case.
- Continuous collision detection: speculative contacts cover bullets-through-walls at the sim rate;
  full CCD only if a slice shows tunnelling the substep count cannot fix.
  Fast 3D shots sweep with `first_hit` in the projectile pool instead (`Plan/bulk-projectiles.md`).
- A physics debug view in the editor: rides the parity gate — the wireframe is a `render.*` flag
  first, then a peer-client toggle.
- Island parallelism: excluded by `enhanced-determinism`; only reachable by leaving rapier. (0020: not a non-goal any more — rapier's `parallel` step is adopted; see Decided (Serious Isaac 0020).)
- Rapier's character controller: see §The 2D queries.
- A protocol command for the physics queries: `ray_cast`, `shape_cast`, `overlap` and `closest_points` are
  Rust-only; wiring all four is one later arc (schema snapshot + TS regen).
- Springs, velocity-target motors, a per-motor model choice and multibody joints are not built. Trigger:
  a game request that a force-based position drive cannot express.
- A convex hull generator from a voxel, sculpt or mesh source is not built. Trigger: a game whose props'
  hand-authored `points` cannot keep up files a request (external-game request 0010 ruled it out of scope, 2026-09-25).

## Risks / open questions

- **Upstream drift.** The pin is `=`, so nothing moves by accident; the risk is the *cost* of moving
  it — every upgrade is a hash-moving re-bless of the whole physics corpus, gated behind gates 1 and 2.
  The failure mode is staying on an old pin for years because the ritual is expensive; the mitigation
  is that the ritual is written down and exercised once in W4, so its cost is a known quantity rather
  than a deterrent.
- **Snapshot size at scale.** About 5.5 MB of base64 per snapshot at 2 000 bodies, about 0.7 MB at 200.
  A snapshot is also a replay keyframe, written every 300 frames and zstd'd per record, so a recorded
  minute of a 200-body scene carries about 12 keyframes; `engine.snapshot` also returns the document
  over the wire. The committed `.lrp` physics corpus stays small on purpose, W4 puts a size ceiling on
  the stress replay, and if a real slice makes the blob the dominant cost the lever is the keyframe
  interval — a host setting — not the codec.
- **The wide/FMA residual is scene-dependent.** The cross-arch identity is evidence for *the scene that
  was traced*, not a proof about the library. Reading settled that rapier and parry never call a fused
  multiply-add themselves, but `wide`'s internal paths were only traced for the sin/cos polynomial
  `libm_force` bypasses. The mitigation is structural: gate 1 applies to every new physics scene, and a
  cross-arch divergence is exit trigger (b), not a bug to chase in our code.
- **Dependency weight against the crate-size rule.** About 25 crates and a solver that does not fit in
  an agent's context. The rule this forces, as a requirement on W1's API: **the wrapper's public
  surface is the whole physics API** — everything a game, the engine, a scene or a test needs must be
  expressible without opening rapier's documentation, and the devdoc topic carries the mapping (shape →
  parry type, tunable → `IntegrationParameters` field, event → rapier concept) so a diagnosis never
  starts in a dependency. The compile-time half: W1 records the `cargo check -p ludion-engine` delta,
  and a breach of the fast-iteration rule is re-opened with a measurement.
- **Snapshot resume is proven on one box only.** The write path was re-run on the M4 and matched
  (Status); the `--snapshot-at` resume was not. Resume is reference-box evidence, and nothing is owed
  for it as a separate errand: W1's gate 2 runs on both machines and has to re-run there regardless,
  because the codec changes from JSON text to bincode.

## Status

**Decision, 2026-09-21.** The user ruled to adopt `rapier3d` behind `ludion-physics` on the evidence
below: cross-arch identity on both pinned machines, a clean denylist audit, a one-percent mirror cost,
and the extension's snapshot-resume and write-path results. The spike was deleted when the ruling
landed.

**Scaled-step energy (external-game request 0019), 2026-09-30 (Lore rev 982).** Rapier's joint lock rows correct drift
with an unclamped bias of about error over substep, so a small `scale_time` threw a landing ragdoll tens
of metres up. `PhysicsWorld::step` retunes each joint's softness frequency for a scaled step so the gain
matches the full step, then restores it by handle; the scale-1.0 path skips it, so nothing moved.
Contacts were already bounded and are pinned by tests. The related scale-1.0 ragdoll energy gain (the external game
0018) is fixed separately, below.

**Vendored rapier, ragdoll settle (external-game request 0018), 2026-09-30 (Lore rev 987).** Rapier's dim3
`limit_angular` measures a ball joint's limited angle as `2·atan2(q_i, w)` but drives it along the bare
frame axis; on a joint swung on several axes the velocity target disagrees with the angle corrected and
the bias is fed back as energy, hundreds of ticks after impact (`motor_angular` has the same mismatch;
0.36.0 is unchanged). The user ruled to vendor 0.35.3 in `vendor/rapier3d/` behind `[patch.crates-io]`,
byte-identical but for `joint_constraint_helper.rs` (later also `collider.rs`/`collider_components.rs`, see the `ColliderChanges::MATERIAL` entry), with `LUDION-PATCH.md` / `.diff` and the Apache
license; the `=0.35.3` pin stands, `physics-features.mjs` asserts the path source, the pin ritual refuses
`--to` until the patch is ported, and the crate is excluded from the workspace gates. An engine-only
default softness was measured and rejected (it slowed the gain, never settled). `tests/ragdoll_settle.rs`
bounds the solver-injected energy on an isolated capsule ragdoll (0.67 J unpatched, 0.044 J patched).
The two gallery ragdoll traces and replays and the Windows `ragdoll-skinned` golden were re-recorded;
the M4 twin passed unblessed 2026-09-30 (rev 996), with `ragdoll-skinned`'s first macOS bless. The residual settle tail is a head rocking on its neck, physical and governed by
`angular_damping`. **A crates.io publish of `ludion-physics` would lose the patch** (`[patch]` does not
travel with a published crate): publishing needs the fix upstream or a published fork first. Upstream
report on the user's word.

**Vendored parry, heightfield rays on grid lines (external-game request 0017), 2026-10-01 (Lore rev 995).** `HeightField` ray casts
missed on a grid line (about half the vertices of the external game's 151x41, 2 m grid): the start cell came from
`cell_at_point`, which divides by the cell size, while `triangles_at` and the walk put cell boundaries
at `x_at`/`z_at`; the two disagree by an ulp, and a ray with no x or z motion never walks on. **The
user ruled (2026-10-01) to vendor parry3d the rapier way**: `vendor/parry3d/` behind `[patch.crates-io]`,
byte-identical but for `ray_heightfield.rs` (the start cell is re-seated on `x_at`/`z_at`), with
`LUDION-PATCH.md` / `.diff`; `physics-features.mjs` asserts the path source and the pin ritual refuses
`--to` until both diffs are ported. `tests/queries.rs` before the patch: vertical rays through
vertices, edges and diagonals missed 10241 of 36571 at the origin pose, oblique rays along a grid line
3542 of 11622; both now 0. Residue: an oblique ray lying in a grid plane can still miss on a shared edge
or vertex, from f32 rounding in the triangle test, which has no tolerance (282 of 46488 for in-plane
rays travelling along x). Hash impact: none — the no-flag pin ritual ends PASS, so no committed trace or replay moved. **A crates.io publish of `ludion-physics` loses this patch too**,
as it does rapier's. Upstream report on the user's word.

**Mirror baseline, 2026-09-26 (audit 09-25 C1).** The edit-detection baseline is the pose each
component reads back after the mirror's write, not the solver's pose, so a `Transform` body (Euler
degrees, a lossy round trip) is no longer re-posed and woken every tick and now falls asleep. The
three gallery traces and the two `.lrp` replays were re-recorded on the RTX 3090 and
`physics-stress.toml` re-measured (`physics_step` median 1.2455 → 1.1316 ms); `pile.txt`,
`layers.txt` and the goldens did not move. The M4 halves stay owed.

**M4 halves, 2026-09-26 (Lore rev 916).** Every committed physics trace and both `.lrp` replays
reproduce unblessed on the M4, the 1/2/8 thread-identity gates pass there, and the pin ritual at the
committed pin ends PASS — cross-arch identity holds for W1–W4. `macos/physics-pile.png`,
`physics-stress.png` and `ragdoll.png` are blessed and `budgets/pinned-apple-m4-physics-stress.toml`
is measured (`physics_step` median 0.5953 ms) and wired into `budgets-macos`. One finding, fixed:
`switch_to_ragdoll` generated different Euler angles on arm64, through glam's backend-specific
`Quat::normalize`; it now normalizes by hand, and both arches regenerate the committed
`ragdoll.yaml` byte-for-byte (x86-64 checked under Rosetta).

**Cylinder and compound colliders (external-game request 0003), 2026-09-25 (Lore rev 882, docs in the next rev).** What exists:
`cylinder { half_height, radius }` (axis along local `y`, like `capsule`) and `compound { parts: [{ shape,
translation, rotation }] }`, built as one rapier compound collider on one body. Rulings, each with its why:
- R1, `translation` and `rotation` are `[f32; 3]` with `rotation` in Euler XYZ degrees, both defaulting to
  identity and never skipped by the writer: it is exactly `Transform`, so a scene author learns one
  convention, and an identity skip would make a part's presence depend on its data.
- R2, a part is a `sphere`, `box`, `capsule`, `cylinder` or `convex`: parry panics on a nested composite and a
  heightfield or 2D part has no meaning inside a body, so those, an empty `parts` and a non-finite offset are
  the new `L1189` at `/shape/compound/parts/<i>`; part sizes stay `L1116`, a degenerate hull part `L1181`.
  `compound`/`part_shape` repeat the check at runtime, because game code can insert a `Collider` past the validator.
- R3, one body, one collider, one `mass` spread over the parts by volume, with a geometric centre of mass
  that may sit off the entity origin: rapier's own mass-properties rule, so contacts, joints, layers, blasts
  and `gravity_scale` need no compound case.
- R4, a compound is not a query shape (`shape_cast` and `overlap` return `QueryInput`; a `cylinder` works):
  a caller casts each part's shape instead.
The planar spatial tier ignores both shapes as 3D. No trace, golden, replay or schema hash moved. Docs:
`ludion docs collision`, `physics`, `physics-joints`; `ludion devdocs ludion-physics`. Tests: `compound.rs` in
`ludion-physics` (cylinder rest and roll, mass and centre, contact points, layers, queries, a blast, gravity
scale, a snapshot resume) and the L1189 cases in `collider.rs`.

**Per-body gravity scale (external-game request 0008), 2026-09-25 (Lore rev 878).** What exists: `RigidBody.gravity_scale`
(`f32`, default `1.0`, omitted by the canonical writer at that value, any finite number; a value beyond `f32`
range reuses `L1116` at `/body/rigid/gravity_scale`), mapped through rapier's `RigidBodyBuilder::gravity_scale`,
and `PhysicsWorld::gravity_scale(entity)` reading it back. Rulings, each with its why:
- R1, a live `Collider` edit whose only difference is `gravity_scale` is applied in place
  (`retune_gravity_scale`, `set_gravity_scale`, full wake), not rebuilt: a rebuild replaces the body handle,
  which would re-capture every `Joint` zero and drop contacts, and a levitation effect edits this every tick.
- R2, the in-place path also calls `wake_up(true)`: rapier's `set_gravity_scale(_, true)` clears the sleeping
  flag but not the rest timer, so a body already past its threshold fell straight back asleep.
- Known limit: any other tunable edit on a jointed body still rebuilds and re-captures its joint zero.
No trace, golden, replay or schema hash moved. Tests: `gravity_scale` in `ludion-physics` (floating and rising
bodies, a hinged bob keeping its zero, the wake from sleep, a mixed edit rebuilding, a snapshot resume
mid-edit); the doctest on `PhysicsWorld::gravity_scale` is the levitation system `ludion docs physics` describes.

**Time scale (external-game request 0006), 2026-09-25 (Lore rev 874).** What exists: `PhysicsWorld::scale_time(scale)`
(finite, `[1e-3, 1]`, else `QueryInput`; calls in a frame compose by the minimum), `freeze()` (skips the step,
wins over any scale) and `last_step_dt()`, in `time_scale.rs`; the step runs with `base_dt × scale`, restores
`base` and resets the scale, per frame like `GameClock::hold`. `GameClock` is untouched and stays decoupled:
a hold does not stop bodies, so a game freezes on its held frames. Rulings, each with its why:
- R1, a frozen frame keeps `blast_hits`: the write system still applies queued impulses, motors and blasts
  (velocities change, poses do not), and the hits describe impulses that really applied; only the step's own
  products, `contact_events` and joint events, are empty.
- R2, `last_step_dt` is not in the snapshot: a restore reads the base timestep until the first step, where the
  uninterrupted run reads the last scaled one. It is informational; gameplay gates on the game's own hashed
  state, as the scale does, rather than a blob change that would re-record every keyframe.
- R3, `to_bytes` refuses (`Encode`) while a scale below `1.0` or a freeze is pending, like the impulse queue,
  so a requested slow motion is never silently dropped by a snapshot.
- Nothing enters the snapshot or `WorldHash`: determinism holds because the game re-derives the scale every
  frame from its own hashed state. Sleep counts simulated seconds (about `1 / scale` times the ticks) and a
  resting contact's impulse shrinks with `dt` while an impact's does not; `ludion docs physics` says both.
- `Speed` (`ludion-sim`) only paces a live scheduler, so the First-consumer promise of slow motion through it
  is corrected to point here.
- Review fixes: `scale_time` takes `[MIN_TIME_SCALE = 1e-3, 1]` (a tinier scale overflows the solver's inverse
  timestep or steps at `dt = 0`; `freeze()` is the way to stop). Kinematic bodies stay unscaled — a full
  per-tick target in a scaled step pushes at `1 / scale`, pinned by a test and documented: the game scales its
  own kinematic motion, so the ECS pose and the solver pose never diverge. Joint `break_impulse` shrinks with
  `dt` like a resting impulse.

No trace, golden, replay or schema hash moved. Tests: `time_scale` in `ludion-physics` (scaled drift,
min-compose, freeze, the frozen frame, sleep in simulated time, the snapshot guard) and `physics_time_scale`
in `ludion-engine` (snapshot resume mid slow motion, 1/2/8 threads); the doctest on `scale_time` is the
hit-stop and slow-motion blast `ludion docs physics` shows.

Amended (Neon Shroud request 0011): a `GameClock::scale` drives `scale_time` automatically; a hold still
does not reach physics. See `Plan/game-time-scale.md` decisions 15–18.

**Contact point and normal (external-game request 0007), 2026-09-25 (Lore rev 870).** What exists: a `Started`
`ContactEvent` carries `point` and `normal` (`[f32; 3]`, `[0.0; 3]` on a `Stopped`), in `world.rs`'s
`contact_point`. Rulings, each with its why:
- One solver contact of the pair, not a manifold average: an average of a face contact can sit off the
  surfaces, and a single contact is a real touching point.
- The pick key is normal impulse descending, then depth (deepest first), then point, then normal,
  lexicographic by `total_cmp` — never the solver's iteration order, so a zero-impulse resting start or a
  speculative-only pair still gets a deterministic point.
- `point` is the midpoint of the contact's two surface points, and `normal` the manifold normal, both
  read through the colliders' poses at the step's collision detection (recorded just before the step
  into a reused buffer; not the post-step poses, which the step's own motion has moved off the
  manifold's frames): rapier's cached world-space contact data is recycled between steps and can be
  stale.
- `normal` is unit, flipped to point from `a` to `b` under the canonical `a < b`, matching `QueryHit`.
- Per step, deterministic for a given `solver_iterations` (a single-contact start is iteration-stable to
  tolerance; a multi-point start may pick a different corner at a different count, the impulses being
  redistributed), not hashed, rides the snapshot with the event.

Review fix: the first cut read post-step poses, which skewed a moving first collider's point and normal;
the ramp test (a rotated static box, the ball as `a` and as `b`) and the heightfield spawn-order test
pin the detection-pose read and the a-to-b flip.

No replay moved: the `physics-stress` and `ragdoll` gallery replays re-record byte-identical (their
keyframes hold no contacts). No trace, golden or schema hash moved. Tests: `contacts` in `ludion-physics`
(orientation, a zero-impulse corner, heightfield cluster determinism, 1/4/8 solver iterations) and the
doctest on `ContactEvent`, which is the example `ludion docs physics` shows.

**Impulses (external-game request 0005), 2026-09-25 (Lore rev 866).** What exists: `PhysicsWorld::apply_impulse` (at the
centre of mass), `apply_impulse_at_point` and `radial_impulse(Blast)` (`center`, `radius`, `impulse`,
`Constant`/`Linear`/`InverseSquare` falloff, a `QueryFilter`), in `impulse.rs`; the free fn `apply_impulse`
on `Velocity` is removed. Rulings, each with its why:
- A queue drained at the end of the write system, after every body is created and pushed, so a `Velocity`
  the game pushed the same tick never overwrites an impulse and a game system needs no `Velocity`.
- Falloff: `Linear` = `1 - d/radius`; `InverseSquare` = `(min_distance / max(d, min_distance))²`, which
  has no hard edge but the radius cut. `d` is the distance to the collider's closest point, so a large
  body is not shorted for its centre being far away.
- Each hit is applied at the closest point (off-centre hits spin), direction centre → point; centre
  inside the collider falls back to centre → centre of mass; both zero pushes nothing and reports nothing.
- `blast_hits` are one tick's hits: not hashed (the `contacts` precedent) but in the snapshot, its last field.
- A snapshot refuses to encode (`Encode`) while calls are queued rather than silently drop them.
- A missing or non-rigid target is a no-op reported once per entity as `L3038`, not an error at the call:
  the check needs the solver, which the call site does not have.
- Occlusion is not offered.

The blob format changed, so the keyframe blobs in the `physics-stress` and `ragdoll` gallery replays were
re-recorded; every per-frame hash in both is identical. No trace, golden or schema hash moved. Tests:
`impulse` in `ludion-physics` and `ludion-engine`, including snapshot resume and 1/2/8 threads.

**Joint motors (external-game request 0004), 2026-09-25 (Lore rev 862 and this commit).** What exists: a `JointMotor`
component (`target`, `stiffness`, `damping`, optional `max_force`) on a joint's carrier drives the
joint's free axes toward a target measured from the captured zero, in the units and axes of `limits`;
`PhysicsWorld::drive_joints` diffs it against the live `GenericJoint` each tick and pushes it without a
rebuild. Rulings, each with its why:
- A separate component, so a retarget never re-captures the joint's zero.
- The motor state rides the rapier blob, so the snapshot format is unchanged.
- Force-based, ruled 2026-09-25: `stiffness` is N·m/rad (N/m on a slider) and `max_force` a real torque
  (force), set as `MotorModel::ForceBased` on every driven axis. The acceleration-based default scales
  by the part's own inertia, which made a light part on a long lever untunable.
  The tuning rule (`stiffness = I·ω²`, `damping = 2·I·ω`) holds while the part's own inertia is within
  about 10× of its swing inertia about the anchor (a rod hinged at its end is about 4×); a point-like
  part on a long lever overshoots and rings, because rapier's angular motor row sees only the part's
  own inertia. Fix: more damping, or give the part real extent.
- Motor force stays outside `break_impulse`: rapier writes it to the motor row, not to the locked-axis
  impulses the break reads.
- `L1188` is static (the component alone); `L3037` is runtime (against the carrier's `Joint`).
- A motor with no `Joint` is silently inert.
- Keep-awake: a driven joint whose error exceeds 1e-3 wakes its sleeping dynamic bodies, so a steady
  drive never stalls asleep short of its target. Cost: a motor that settles off target (a limit, a
  `max_force` cap, gravity sag) never sleeps.

Not moved: the schema hash `be7ebe1877875ed1`, every trace, golden and replay. Tests: `joint_motor` in
`ludion-physics` and `ludion-engine`, including snapshot resume and 1/2/8 threads.

**W4 mechanized, 2026-09-23 (Lore revs 825–830).** What exists: `ContactEvent`'s impulse is per-step
(rapier's `total_impulse()` already sums the substeps; pinned in `crates/ludion-physics/tests/contacts.rs`);
the 512-body ceiling is gated — by the RTX 3090 stress budget, a test pinning the stress pile's body
count, and a committed stress replay whose size has a ceiling — and written down in
`dev-docs/physics-pin.md`; a profile test pins the write/step/mirror triple in an offline report; the
ragdoll golden reframed so the six figures fill the frame, its scene, trace, replay and Windows golden
regenerated; and the pin-upgrade ritual as a script, rehearsed once end-to-end (next paragraph). The M4
halves are in the owed ledger. The wire is unchanged at 30 methods, schema hash `be7ebe1877875ed1`.

**W4 review, 2026-09-23 (Lore revs 829–830).** A whole-wave review found defects in the ritual and one
in the scene, all fixed. The failure parser choked on the counter nextest prints before a test name, so
MOVED was unreachable and a hash-moving bump read FAIL; the MOVED protocol claimed replay tests fail
after the traces are re-blessed when they pass while the `.lrp` keyframes keep the old pin's physics
blob (no gate compares them — owed); `--bless-traces` now re-records only after an
unblessed MOVED and names the replays to re-record; a failure after a pin move prints the exact revert;
and the leftover-version scan walks the working tree, not the git mirror. In the scene, the ragdoll
floor and obstacles were drawn at half their collider size, so figures landed on invisible geometry;
the meshes now match the colliders, which moved the scene's hash, so its trace, replay and golden were
regenerated.

**W4 pin-upgrade ritual, 2026-09-23.** `scripts/physics-pin-ritual.mjs` runs §W4's five steps on demand
(`--to <x.y.z>` moves the pin and prints what moved in `Cargo.lock` and every file still naming the old
version; the feature gate; the physics and thread-identity gates under nextest; the re-bless order
printed, never run) and ends PASS, FAIL or MOVED with a per-step wall time. On one machine the committed
traces and replays are the previous pin's, so a failure confined to a named list of those comparisons
is MOVED, not FAIL, and `--bless-traces` re-records the four traces for the second machine to check;
`dev-docs/physics-pin.md` has the protocol. Rehearsed on the RTX 3090 box with the pin moved
0.35.3 → 0.35.2 → 0.35.3 (0.35.2 never committed): the baseline ran in 15 s warm, the move to 0.35.2
rebuilt rapier and all dependents in 112 s (104 s of it the gates' build) and moved only `rapier3d` in
`Cargo.lock` — parry3d stayed 0.30.2 — and every gate passed with no hash moving, then 21 s back to
0.35.3 with `Cargo.toml` and `Cargo.lock` byte-identical to the committed pin. The M4 half and the first
real run are in the owed ledger.

**W3 mechanized, 2026-09-23 (Lore revs 817–821).** What exists: `Joint` — hinge, ball, slider or
fixed, with limits in degrees or units, an optional `break_impulse` and a `collide` flag — as a
pure-data component in `ludion-spatial`, mapped by the wrapper onto rapier impulse joints, with
`JointBroken` events and the joint table riding the physics snapshot blob; codes `L1183`/`L3035`, and
`L1017`/`L1018` generalized from `Parent` to any entity reference; joints formed and destroyed at
runtime through the existing `entity.spawn`/`component.*`/`entity.destroy` commands, with
`entity.spawn` now semantically validating every component the way `component.set` already did; a
`kind: assembly` model's optional `joints:` map (`L1184`) and a `Mesh.shape: assembly_part` that draws
one part from its own entity (`L1185`); the public `switch_to_ragdoll` (`L3036`) that turns a posed
assembly entity into one rigid body per part joined by `Joint`s, with its runnable example
`ragdoll_switch`; and `examples/gallery/ragdoll.yaml` — six ragdolls falling onto static obstacles,
generated by `crates/ludion-engine/tests/ragdoll_scene.rs` and byte-compared against it as the drift
gate (`scripts/map-drift.mjs` skips a scene whose `generated_from` ends in `.rs`) — beside its
600/600-distinct trace, the resume at tick 300, 1/2/8-thread identity, CLI `sim-hash`, `ragdoll.lrp`
(its keyframe carries the physics blob), and the Windows golden. The wire is unchanged at 30 methods,
schema hash `be7ebe1877875ed1`.

Rulings made while building, each reversible by the user:

- **A joint names its bodies by `EntityId`**, the `Parent` precedent; any entity may carry one,
  including `a` or `b` itself, so a body that needs more than one joint takes a dedicated carrier.
- **The joint's frame is captured from the two bodies' current poses when it is built**, so the
  current relative pose is its zero and limits read as "from the rest pose"; a rebuild re-captures.
- **Break removes the component.** Exceeding `break_impulse` drops the rapier joint, records a
  `JointBroken` event, and the mirror removes `Joint` from the carrier — a joint exists iff its
  component does.
- **A missing body leaves the joint inert, never deleted** — `L3035` once per carrier, re-forming
  when both bodies exist again.
- **Planar axis locks compose with joints in the solver with no special code**; only a hinge about
  the plane normal, or an in-plane slider, avoids over-constraining a planar scene.
- **Angles are degrees**, the `Transform` convention; the wrapper converts by multiplication.
- **No joint commands.** A joint is a component on a carrier entity, so dedicated commands would be a
  second way to do what `entity.spawn`/`component.*`/`entity.destroy` already do; the schema hash does
  not move.
- **A ragdoll's parts become entities only at the switch**, and the root is kept; a part renders
  through `Mesh.shape: assembly_part`.
- **The collider is the 8-corner convex of the part's bounds**, in the part-mesh frame, with mass =
  volume × scale³.
- **The child carries the joint to its parent**, anchored at the child's pivot; a part with no
  `joints:` entry is `fixed`, and limits count from the pose at the switch.

Findings for W4:
- `dev-docs/ludion-physics.md` is about 177 B over its topic budget.
- Joint limits count from the switch pose, not the rest pose.
- Sibling parts that overlap will collide, because only parent–child pairs skip contacts. → `Plan/physics-collision-layers.md` (collision layers)
- `ContactEvent`'s impulse was suspected of the joints' per-substep bias; W4 measured it: rapier's
  contact `total_impulse()` already sums over substeps, so the reported magnitude is per-step and
  independent of `solver_iterations` (pinned by a test in `crates/ludion-physics/tests/contacts.rs`).

**W3 review, 2026-09-23 (Lore rev 823).** A whole-wave review found four defects and five smaller
issues, all fixed. Four stand out: a `Joint` re-added to a carrier the tick after it broke never
formed; a break impulse depended on `solver_iterations`, because rapier reports only the last
substep's impulse, so it is now scaled to a per-step value and a slider's end stop counts toward it;
the ragdoll switch accepted a zero or negative scale; and an axis that rounds to zero in f32 reached
the solver as NaN. The rest: the runtime codes name their entity; the drift probe covers `Joint`;
the map drift gate checks a Rust generator exists; the docs say a saved and reloaded joint
re-captures its zero at the saved pose.

**W2 mechanized, 2026-09-23 (Lore revs 807–812 and the budget commit).** What exists: the scene
`physics:` block (`space` required, `solver_iterations`/`stabilization_iterations`/
`sleep_after_ticks` optional, the triple listed in `engine.describe` after the game systems) and its
additive `scene_physics` snapshot field; every 3D `Collider` shape validates (a convex hull that
cannot enclose a volume is `L1181`, a ragged or too-small heightfield grid is `L1182`, a rigid body's
mass joins the positive-and-finite `L1116` rule); `apply_impulse` turns an impulse into a `Velocity`
change and wakes a sleeping body; gravity is an optional field of the `physics:` block (the wrapper's
own gravity setters removed as a second way to set it); the post-step contact event stream (pair plus
solved impulse magnitude, sorted by the pair's stable ids, read one tick behind since the triple runs
after every game system); the three read-only queries (`ray_cast`/`shape_cast`/`overlap`) returning
`Result`, hits sorted by distance then entity id, overlap sorted by entity id, one entity excludable,
a non-finite/negative input or a query shape the solver cannot build reported as an error naming the
argument, never an empty result; `docs/collision.md`'s dynamics half and the devdoc split
(`dev-docs/ludion-physics.md`, `dev-docs/physics-events-queries.md`, `dev-docs/physics-pin.md`);
`examples/gallery/physics-stress.yaml` — 512 box/sphere/capsule rigid props in a static bin, 600/600
distinct trace, resume at tick 300 reproducing to the trace's end, 1/2/8-thread identity, CLI
`sim-hash` against the committed trace; the RTX 3090 budget
(`budgets/pinned-rtx3090-physics-stress.toml`, `physics_step` median 1.2455 ms at `--frames 300`).

Rulings made while building, each reversible by the user:

- **The triple runs after every game system.** The builder verb is gone, so no game picks a
  position; a system that writes `Velocity` or teleports a body this tick gets integrated this tick.
- **`space` is required inside the `physics:` block**, never inferred — the default plane freezes
  gravity's axis.
- **`sleep_after_ticks` is resolved in the wrapper at body creation, never written back into the
  component** — the per-body value if authored, else the block's default, else 30 — so a save never
  writes a value the author didn't.
- **Iteration defaults are rapier's own** (`num_solver_iterations` 4, `num_internal_stabilization_iterations`
  1), pinned equal by a unit test so a pin upgrade that changes a default fails loudly, and capped
  1..=64 (`L1033` — a runaway count would stall every host's tick).
- **The `scene_physics` snapshot field is additive**, on the `profile`/`physics` precedent: absent
  restores as absence, `SNAPSHOT_FORMAT` stays 1.
- **Contacts are diffed from the post-step contact graph**, not rapier's own event handler (its
  Started fires pre-solve, with no impulse); always on; sensors exist and never produce contacts (sensor events are a separate stream); the list is not hashed and rides
  the snapshot blob, so a restored session sees exactly what the uninterrupted run saw.
- **Queries return every hit, sorted, and a bad input is a `Result` error**, never an empty result —
  the solver's own iteration order is never observed.
- **512 props for the stress scene**: W0 measured 200 bodies too calm to stay live for a 600-tick
  trace and 2 000 at the measured single-threaded step-time ceiling; 512 is a heavy launcher-level
  scale that stays live through the trace while keeping the snapshot a manageable size.

Findings for W3: a bare `render`/`sweep` host never drains shape rejections; the rejection latch is
per (code, entity), so a re-broken entity is not re-reported; `apply_overrides` does not reach
`/physics` in a scene (overrides still target entities only); pre-rev-809 snapshot blobs no longer
decode (the contact-event stream change), and none is committed; the gallery pile trace was
re-recorded at rev 807 (the `sleep_after_ticks` `Option` moved its bytes).

**W2 review, 2026-09-23 (Lore revs 814–815).** A whole-wave review found nine defects, all fixed.
The one that mattered most: a `planar` scene simulated differently under `render`/`sweep` than under
engine hosts, because those hosts never installed the plane. Now a missing plane reads as the
documented default and both hosts share one helper. The rest: the prelude gains `apply_impulse` and
`PhysicsError`; `L1181`'s fix text is actionable; restoring a physics world without a scene block
warns (`L3034`); a restored block is re-validated; queries reject a non-unit rotation; the rejection
latch is keyed by entity. A changed `Collider` on a live body now rebuilds that body at its current
pose and `Velocity`, where before it was silently ignored. The built-from `Collider` rides the
snapshot as JSON bytes, because a serde-skipped optional field breaks bincode's positional decoding.
Physics blobs from before rev 815 no longer decode. No trace, golden or budget moved.

**W1 mechanized, 2026-09-22 (Lore rev 804 and the follow-up).** What exists: `crates/ludion-physics/`
(the only crate naming a rapier or nalgebra type; `rapier3d = "=0.35.3"`, `bincode = "1"` in the
workspace table), `PhysicsWorld`, the one verb `install::<P>(&mut Schedule, PhysicsSpace)`, the
`PhysicsPose` seam `Transform` implements, `Collider`'s 3D shapes and `body: rigid`, the hashed
`Velocity`, the `dynamic` → `kinematic` rename (old spelling accepted on read by serde and by the
generated schema, every committed scene and generator migrated), the snapshot field (base64 of
bincode, `SNAPSHOT_FORMAT` still 1, `L3033` for a corrupt blob), `scripts/physics-features.mjs` in
`check.mjs` and CI, `dev-docs/ludion-physics.md`, and the gallery scene `examples/gallery/physics-pile.yaml`
with its committed 600-tick trace `physics-pile.trace.txt`. Gates on the reference box: the crate's
own pile trace (300 of 300 distinct), the gallery trace (600 of 600 distinct), the engine-level
resume proof on the bincode path (`crates/ludion-engine/tests/physics_snapshot.rs`, restore at tick
80, identical through 200, 200 of 200 distinct), 1/2/8-thread identity on the gallery scene
(`crates/ludion-engine/tests/thread_identity.rs`), the resolved-feature gate green. Owed: the M4
halves of gates 1 and 2 and the `macos/physics-pile.png` golden. The `cargo check
-p ludion-engine` delta is not measurable warm (1.2 s before and after); the one-time cold build of
the rapier subtree is about 16 s, and a clean `cargo check -p ludion-physics` about 27 s.

Three rulings made while building, each reversible by the user:

- **The claim rule is the complement of `spatial::rebuild`'s skip.** Physics claims an entity iff its
  shape is 3D or its body is `rigid` (`ludion_physics::claims`); a static or kinematic `circle`/`aabb`
  stays in the spatial tier, so a physics scene's level geometry is authored with 3D shapes, and the
  resource is created only on the first tick that claims something — a 2D game with the verb in its
  schedule pays one typed join per tick. The first cut claimed every `Collider`, which would have
  turned vigil's walls into solver bodies.
- **The space is explicit at the opt-in site.** The engine always installs a `Plane` (default `Xz`),
  and turning it into locks freezes the axis gravity points down, so a 3D pile in a default engine
  did not fall at all (49 distinct hashes of 200 in the first draft). `PhysicsSpace::Free` is full
  3D with no plane locks; `Planar` is the plane's off-plane translation and in-plane rotations locked
  on every rigid body. `EngineBuilder::physics(space)` records the schedule position and the space;
  `build_schedule` installs the triple there on every world rebuild.
- **hashbrown is not gated.** The plan's feature-gate bullet asked for `rayon` and `hashbrown` absent;
  parry3d pulls `hashbrown` under `std` → `alloc` with an explicit fixed foldhash hasher, and the
  workspace already carries it through `serde_json`'s `preserve_order` → `indexmap`. A hasher choice
  is a source fact `cargo metadata` cannot see, so the gate pins the exact rapier3d and parry3d
  feature sets (with `enhanced-determinism` checked by name) and asserts `rayon`/`rayon-core`
  unreachable; the per-scene cross-arch trace is what stands behind the container question.

Findings for W2 (the space/describe/host questions here are superseded above by the "W2 mechanized"
entry's scene `physics:` block and its `engine.describe` entries): the rename moved the hash bytes
of every entity whose body was `dynamic` (the variant name is hashed through its serialization), and
nothing committed pinned such a hash — the replay tests compare a fresh recording against a fresh
replay. A host with no game crate (`ludion serve`, `sim-hash`, `render`) builds a bare engine and
never opts into physics, so a gallery physics scene steps under a test or a game only; whether a
scene document should declare its space is an open question for the user. `physics_write`/`step`/
`mirror` are not `SystemSpec`s and do not appear in `engine.describe`, and `EngineBuilder::physics`
hardcodes `Transform` as the pose.

**W0 on both pinned machines (RTX 3090, Apple M4).** The spike was `examples/rapier-spike/` (rapier3d
0.35.3, features `dim3 f32 std enhanced-determinism`, no `parallel` or SIMD), deleted when the ruling
landed; the commands below are reproducible only from a revision that still carried it (`lore history`
finds it).

- **Same-box determinism.** Two runs are byte-identical at 200 and at 2 000 bodies. The pile is live
  for 468 ticks at 200 bodies and then sleeps; at 2 000 it is live for the full 600. The reference
  traces were committed as `examples/rapier-spike/traces/windows-200.txt` and `windows-2000.txt`.
  **Cross-arch passes:** the M4 traces are byte-identical to the reference box's at 200 and 2 000
  bodies (x86_64 / arm64), which is also the empirical answer to the audit's unverified `wide` ops
  for this scene. The comparison was made with, from `examples/rapier-spike/`:

  ```bash
  cargo run --locked --release -- --bodies 200  --ticks 600 > m4-200.txt
  cargo run --locked --release -- --bodies 2000 --ticks 600 > m4-2000.txt
  diff m4-200.txt  traces/windows-200.txt
  diff m4-2000.txt traces/windows-2000.txt
  ```

  The trace is stdout only (the banner and cargo's output are stderr — do not merge the streams).
  Empty diffs pass only if the trace is varied, since the hash canonicalizes every non-finite float to
  one value; the reference is 468 distinct lines of 600 at 200 bodies and 600 of 600 at 2 000
  (`sort -u | wc -l`), so the empty diffs prove the M4 trace varied too and the non-finite caveat is
  closed. The trace is thread-count invariant.
- **Build inputs.** The reference traces were recorded under rustc 1.94.0 (the root toolchain pin)
  with the committed `Cargo.lock`, which pins only `rapier3d` exactly; the arithmetic-bearing crates
  beneath it (nalgebra, simba, wide, glam, indexmap) are held by the lockfile alone. The comparison
  ran `--locked` under the pinned toolchain; a `--locked` failure or a moved toolchain pin would have
  meant stop and report, and a mismatch obtained by dropping `--locked`, running `cargo update` or
  regenerating a trace is not evidence. If the lock has gone stale because a path crate gained a
  dependency, both traces are re-recorded at one revision on both machines.
- **Cost.** Release build, single-threaded rapier, 32 logical cores, `LUDION_THREADS` unset, over the
  whole 600-tick run (the timing includes the probe's per-system clock read):

  | bodies | step mean / p99 (ms) | mirror mean (ms) | mirror share |
  |---|---|---|---|
  | 200 | 0.22 / 0.60 | 0.0025 | 1.1% |
  | 2 000 | 4.6 / 9.6 | 0.021 | 0.45% |

  Over the first 300 ticks alone the 200-body step reads 0.33 ms mean, 0.61 ms p99. The mirror
  measured is one-directional (rapier to pose); it omits the quaternion-to-Euler conversion a real
  `Transform` mirror pays; the ECS-to-rapier write path is measured in the extension below. At 2 000
  bodies the step's mean is over a quarter of the 16.7 ms tick and its p99 well over half. The M4 step
  cost was not measured.
- **Denylist audit** (a private audit report): zero
  reachable violations under the feature set. It turns on two things: `enhanced-determinism` selects
  `IndexMap`/`IndexSet` with a fixed hasher and pure-Rust libm; `wide` is an unconditional dependency.
  No `mul_add`/`simd_mul_add` call exists in rapier's or parry's own sources, and `wide`'s internal
  fused path was traced only for its sin/cos polynomial, which `libm_force` bypasses; other `wide`
  ops are unverified by reading, which the cross-arch diff checked empirically. Not reachable: rayon
  behind `parallel`, `hashbrown` behind `not(enhanced-determinism)`, the profiler clock.

All three questions were answered and none settled build: (1) passes cross-arch, (3) is clean, (2) is
about one percent. The adopt-or-build ruling was made from these numbers (Decision above), and the
spike and its traces are deleted.

**Ragdoll bench (Serious Isaac 0017), 2026-11-09 (M5, indicative).** `tests/ragdoll_bench.rs`
(ignored, release): N mannequin ragdolls (the 0016 parts prefab, 11 parts and 10 joints each) switched
in by `switch_skin_to_ragdoll`, beside 500 walking `CharacterMotor`s, 8 threads, 120 measured ticks per
phase, medians of three runs, unpinned M5 (not authoritative). `switch_skin_to_ragdoll` loads the
prefab once and reuses the cached copy, so no per-switch file read is in the figures.

| N | phase | step med / mean / p99 (ms) | write med | asleep-part share at window end |
|---|---|---|---|---|
| 60 | all awake | 0.98 / 0.99 / 1.30 | 0.32 | 0% |
| 60 | settled | 0.17 / 0.17 / 0.19 | 0.31 | 98% |
| 60 | blast (40 groups) | 0.74 / 0.78 / 1.03 | 0.32 | 32% |
| 60 | churn 25/s | 0.59 / 0.60 / 0.69 | 0.35 | 0% |
| 120 | all awake | 1.63 / 1.72 / 2.67 | 0.57 | 14% |
| 120 | settled | 0.32 / 0.32 / 0.44 | 0.58 | 97% |
| 120 | blast (40 groups) | 0.93 / 0.94 / 1.31 | 0.59 | 63% |
| 120 | churn 25/s | 0.79 / 0.79 / 0.95 | 0.63 | 41% |
| 240 | all awake | 3.32 / 3.65 / 6.42 | 1.14 | 7% |
| 240 | settled | 0.58 / 0.58 / 0.72 | 1.10 | 95% |
| 240 | blast (40 groups) | 1.28 / 1.34 / 2.05 | 1.12 | 78% |
| 240 | churn 25/s | 1.02 / 1.03 / 1.24 | 1.15 | 68% |

Mirror is 0.05 / 0.12 / 0.23 ms at N = 60 / 120 / 240; `motor_walk` for the 500 motors is about 3.8 ms
in every phase. The "settled" phase never fully slept within the 1 800-tick cap at any N: 2–5% of parts
stay awake (the cap, not a pass, ended each wait). Run-to-run spread was a few percent, except one
noisy run (N = 120 settled step 0.51 ms, N = 240 churn p99 3.1 ms), which the medians drop. Not comparable to the W0 2 000-body row (another machine); the pinned run gives the
like-for-like figure. Does churn matter: at N = 120 the churn-phase step is
+0.47 ms and the write +0.05 ms over settled (the freshly spawned groups land awake), and the
`ragdoll_churn` system itself costs 0.11 ms mean and 0.31 ms p99 per tick (a median of 0, since it
works on 25 ticks in 60). The pooling decision is owed, not made here.

**Horde bench (Serious Isaac 0020), 2026-11-09 (M5, indicative).** `tests/horde_bench.rs` (ignored,
release, `--features physics-profiler`; run: `cargo test -p ludion-engine --release --features
physics-profiler --test it -- horde_bench --ignored --nocapture`) drives the committed
`physics-horde` scene (500 motors, 120 ragdolls with 24 pinned to the walls, a quarter of the
unpinned re-thrown every 30 ticks, 5,000 pool shots) for 240 ticks after 120 of warmup, medians of three runs, unpinned M5 (not
authoritative). The `WorldHash` is equal at 1 and 8 threads.

| system | 1t mean / p99 (ms) | 8t mean / p99 (ms) | speedup |
|---|---|---|---|
| `motor_walk` (500 `move_character`) | 3.76 / 3.99 | 3.76 / 3.99 | 1.0x |
| `physics_step` | 1.78 / 2.08 | 1.79 / 2.08 | 1.0x |
| `physics_write` | 0.62 / 0.65 | 0.59 / 0.65 | 1.05x |
| `projectiles_step` | 0.68 / 0.90 | 0.26 / 0.30 | 2.6x |
| `physics_mirror` | 0.14 / 0.16 | 0.14 / 0.16 | 1.0x |

**Batch (decision 25 of `Plan/character-controller.md`):** the `motor_walk` row above is the serial `move_character` figure; the driver now calls `move_characters`, and the same run gives 3.72 / 3.92 ms at 1t and 1.22 / 1.26 ms at 8t (3.05x, 7.5 and 2.4 us per motor), the 1t and 8t `WorldHash` equal and the committed trace unchanged.

`motor_walk` is 7.5 us per motor. Step stages at 8t (ms, mean): solver 0.88 (velocity resolution
0.81), collision detection 0.63 (narrow phase 0.46, broad phase 0.14), update 0.06, island and
constraint upkeep under 0.03 each; 1,865 bodies, 1,820 awake, 406 active contact pairs, 1,224 impulse
joints. Pair census (mean per tick, tracked / touching): rigid-rigid 3,422 / 177, rigid-static 386 /
229; kinematic-static, kinematic-kinematic and rigid-kinematic 0 / 0. Rapier's broad phase drops a
pair that `active_collision_types` or a collision group rejects before it enters the contact graph
(`broad_phase_bvh/update.rs`), so motor-motor and motor-static pairs cost no narrow phase. All 1,320
ragdoll parts stay awake in every measured tick. The motor casts dominate: `motor_walk` (3.8 ms) costs about twice the whole physics step (1.8 ms),
of which the solver is half and the narrow phase a quarter, and `move_character` is serial.

**Rapier parallel spike (Serious Isaac 0020), 2026-11-09 (M5, indicative).** Method: a scratch copy of the tree outside the repo (nothing committed, no Cargo, lock or source change in the checkout), `"parallel"` added to the workspace `rapier3d` entry, built beside `enhanced-determinism`, the horde bench and the physics gates run in it. The claim that `enhanced-determinism` and `parallel` are mutually exclusive (give-up 1, `dev-docs/physics-pin.md`, the root `Cargo.toml` comment) is false for rapier 0.35.3: the combination compiles, and the one `compile_error!` is `simd8` + `enhanced-determinism`. Upstream's `tests/parallel_path_parity.rs` passes with `parallel` on and off. Lock: `Cargo.lock` gains `rayon 1.12.0`, `rayon-core 1.13.0`, `crossbeam-deque 0.8.8`, `crossbeam-epoch 0.9.21` and the `rayon` edges of rapier3d/parry3d; no existing package moves (`fuzz/Cargo.lock` takes the same four). `scripts/physics-features.mjs` fails as designed: "rapier3d@0.35.3 resolved features do not match the pinned set. extra: parallel".

Identity (gates: `ludion-physics`, `ludion-engine` `physics_*`/`ragdoll*`, `ludion-cli` `physics::`, `thread_identity` ignored, the CLI `replay::` corpus; a pass is bit-equal to the committed single-threaded traces and replays, including `horde_hashes_are_thread_count_invariant` at 1/2/8 `par` threads and the snapshot restores):

| rapier pool | physics gates (434) | thread_identity (6) | replay (6) |
|---|---|---|---|
| global, `RAYON_NUM_THREADS=1` | 433 | 6 | 6 |
| global, 2 | 434 | 6 | 6 |
| global, 8 | 434 | 6 | 6 |
| dedicated, sized `par::threads()` | 434 | 6 | 6 |

The one miss is `sweep_alloc::sweeps_do_not_allocate` (allocation count 3, not 0). It also fails once in the off build in the same copy and passes on the other `parallel` runs, so it is an allocation-counter flake on this box, not a hash divergence. No hash diverged at any pool size, `RAYON_NUM_THREADS=1` included, so no `cfg(parallel)` path (the BVH refit among them) moves a result. Nothing to localize.

Timing (horde bench, medians of three, `physics_step` at the 8t `par` column; stages are the 8t `step_breakdown`; off = the committed build):

| build / rapier pool | step mean / p99 (ms) | solver | narrow phase | broad phase |
|---|---|---|---|---|
| off | 1.83 / 2.10 | 0.88 | 0.47 | 0.15 |
| on, global 1 | 1.97 / 2.24 | 0.89 | 0.49 | 0.19 |
| on, global 2 | 1.34 / 1.57 | 0.57 | 0.36 | 0.14 |
| on, global 8 | 1.12 / 1.36 | 0.47 | 0.18 | 0.16 |
| on, global 18 (the default: cores) | 1.61 / 1.91 | 0.52 | 0.20 | 0.32 |
| on, dedicated 8 (`par::threads()`) | 1.06 / 1.32 | 0.46 | 0.17 | 0.13 |

The step falls from 1.83 to 1.06 ms (1.7x) at 8 workers. Velocity resolution goes 0.82 → 0.39 ms and the narrow phase 0.47 → 0.17 ms. Beyond 8 workers it regresses: the default global pool sizes to the 18 cores and the broad-phase refit alone doubles (0.15 → 0.32 ms), so the pool must be sized, not left to rayon. A pool of 1 is 7% slower than off (the `parallel` paths run inline with their overhead); that row was measured as the global pool of 1, not as a dedicated pool of 1.

Oversubscription: the same-run `motor_walk` batch at 8t is 1.25–1.31 ms in every `parallel` configuration against 1.27 off, and `projectiles_step` 0.223–0.226 ms against 0.224. Rapier's idle workers do not compete with `par` regions in the measured loop (the step is its own system, so the two never overlap in time). Thread-count facts: the global pool is a second un-governed knob (`RAYON_NUM_THREADS`, default cores), outside `LUDION_THREADS`/`--threads`, and a `parallel` build with no dedicated pool uses it; a dedicated pool is sized from `par::threads()` but its field is `serde(skip)`, so a decoded world has no pool until `ludion-physics` rebuilds it. The pinned-box runs are owed on the 3090 and M4: identity under `parallel`, the cross-arch traces, the `.lrp` corpus, then these timings.

**W0 extension — snapshot resume and the write path** (detail in the private
a private plan; pointer only). Reference box, on the spike:

- **Resume from serialized state is byte-identical.** Serializing the nine persistent rapier structs
  and the creation-order handle table beside the ECS half, dropping the driver, world and every rapier
  set, and rebuilding from the text alone with a fresh pipeline gives a 600-line trace byte-identical
  to the uninterrupted one, at snapshot ticks 2, 150 and 500, at 200 and 2 000 bodies, with and
  without the write path on.
- **The restore-boundary self-check** (hash checked at the restore boundary) was clean at every point,
  so an ECS-restore fault and a rapier-resume fault cannot be confused.
- **The ECS→rapier write path is cheap.** A `Velocity` component written by game code at fixed ticks
  with integer-derived impulses, pushed into rapier by a system ahead of the step and pulled back by
  the mirror, costs under one percent of the step; the tick-480 kick wakes a pile that is fully asleep;
  the traces reproduce run to run. Enabling the serde feature moved neither committed trace, and the
  lockfile gained two packages and changed no version.
- **The M4 run, 2026-09-21:** the kick traces are byte-identical to the reference box's at 200 and
  2 000 bodies, so the write path is proven cross-arch, as the pose mirror was in W0. **Snapshot resume
  remains reference-box-only evidence** — the M4 run covered the kick traces, not `--snapshot-at`;
  W1's gate 2 on both machines is what closes it.
- **Structured JSON is impossible.** `serde_json::to_value` — what `capture()` uses — rejects the
  narrow phase (`body_solver_color_masks: Vec<u128>`, "number out of range"), so the state cannot be a
  structured sub-value of the snapshot. That is closed by the type, not by taste: it rides as an
  opaque string or byte field.
- **Codec, at tick 150** (like for like, the state as JSON text against bincode):

  | bodies | JSON | bincode | ser | de |
  |---|---|---|---|---|
  | 200 | 2 418 812 B, 4.7 ms ser | 520 276 B | 0.43 ms | 0.68 ms |
  | 2 000 | 19 665 480 B, 40 ms ser | 4 142 692 B | 3.3 ms | 4.8 ms |

  About 4.7× smaller and 11× faster. In a JSON document the bytes ride as base64 (about 5.5 MB at
  2 000 bodies), still about 3.6× smaller than the JSON text.
- **What rapier warrants upstream.** Its `tests/snapshot_portability.rs` tests *capture*
  byte-portability under exactly our feature set (native and wasm32); it publishes no test that
  deserializes and keeps stepping. Resume fidelity is ours alone, and this extension is its only
  evidence.

- **external-game request 0023 (2026-10-02).** The vendored rapier3d/parry3d are path dependencies (workspace `rapier3d` entry; rapier's own `parry3d`), not a root `[patch.crates-io]`, so a path consumer builds the patched crates; the features gate probes a scratch consumer. Temporary — revisit before any crates.io publish.

- **Audit 2026-10-03 (physics P2–P10).** The vendored rapier3d also carries a `ColliderChanges::MATERIAL`
  patch (`collider.rs`, `collider_components.rs`): contact recycling skipped the pair update that combines
  friction and restitution, so a material edit on a resting pair was never read; `LUDION-PATCH.md` has the
  detail. A rebuilt rigid body keeps its solver momentum when it carries no `Velocity`; a moved `static`
  body re-poses the solver body; a `heightfield` on `rigid` is `L1211` like a mesh; the resolved-feature
  gate pins `glam`/`glamx` and denies `fast-math`/`core-simd`; `collect_contacts` computes a contact point
  only for a pair that just started.

**Serious Isaac 0026 (pose blends), mechanized.** `JointPoses`, `PoseBlend`, the `pose_blend` system and `L4035` shipped; the schema hash moved, no WorldHash or replay hash did.

**Serious Isaac 0035 (angular sleep gate), mechanized.** The vendor gate patch, `RigidBody.sleep_angular_threshold` (`L1116` validation, creation and in-place apply) and the pinned corpse tune (0.1 / 15 / 2.4) shipped; the component schema moved, no WorldHash did; the replay content hash moved and the committed replays are re-recorded separately.

**Serious Isaac 0020 (threaded step), mechanized.** `parallel` is on, rapier on a pool `ludion-physics` owns, hashes unmoved. RTX 3090, 8 threads, release: horde bench step 4.654 / 6.583 → 2.262 / 3.014 ms (mean / p99); at 1 thread 4.421 → 4.969 ms; stress step 1.216 → 1.019 ms. The physics-stress budget, re-measured over four warm 300-frame runs at 8 threads (median of runs, ms), still passes everywhere and is **not re-pinned** (a global swing moves no ceiling): `physics_step` 1.1316 → 0.9918 (ceiling 2.30), `physics_write` 0.0851 → 0.1104 (0.17), `physics_mirror` 0.0670 → 0.0739 (0.14), frame total 1.2861 → 1.3229 (2.60). No horde budget exists; `ragdoll_bench`'s ceilings are in-code and unpinned. M4 twins are owed in the owed ledger.

**Serious Isaac 0020 follow-up (step deadlock), mechanized.** The step hung at 0% CPU in `join_deferred_bvh_optimize`: the broad phase `rayon::spawn`s its deferred BVH optimize, which from a worker lands on that worker's own deque with no guaranteed wake, and the join then blocked that same worker in a plain `recv`. The vendored join now runs pool jobs (`rayon::yield_now`) until the task arrives (`vendor/rapier3d/LUDION-PATCH.md`). `tests/solver_pool_progress.rs` holds a deterministic repro (one of two workers pinned by a blocker, red before the fix) and a 1/2/8-thread soak with reloads, restores and pool rebuilds, both under a watchdog. No hash moved. Fix rev r2324.

**Serious Isaac 0043 (physics order follows spawn rank), 2026-11-15, mechanized.** The write system feeds rapier in ECS spawn-rank order; blasts, partner wakes and the motor tie-break use spawn rank (43-04: a rank side table for the write, `World::spawn_rank` for the motor); `SnapshotDoc.spawn_order` carries the order; `tests/churn.rs` pins bit-identity under churn. `physics-stress`, 300 frames, 8 threads, release, M5 (unpinned, indicative), median of 3: `physics_write` 0.060 → 0.063 ms, `physics_step` 0.512 → 0.511 ms. 43-04 (same scene, 8 threads, median of 3): `physics_write` 0.065 → 0.069 ms (the per-write rank table; 5 runs after), `physics_step` 0.516 → 0.506 ms. The gated CPU traces are checked at the corpus re-record.
