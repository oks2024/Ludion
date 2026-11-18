# Plan: Animation layers — bone masks, layers, blend spaces, IK, root motion and a state-graph helper

> Status: **W1, W2a, W2b (sim), A1 and A2 mechanized (2026-11-18); A3, W2c–W6 not started.** Driver: Neon Shroud request 0007
> (`engine-requests/0007-animation-layers-blendspaces-ik.md`, filed 2026-11-17, audited at engine r2369).
> It lifts the deferrals `Plan/animation-clips.md` §Non-goals recorded, each with a fired trigger. The
> design below was written from the tree on 2026-11-18, so re-grep before editing.

## Status

### W1 — bone masks, `AnimationLayers` (override and additive), L1670–L1672

| file | role |
|---|---|
| `crates/ludion-scene/src/character.rs` | `masks:` parse (`L1199`), validate and joint check (`L1200`), `emit`, `resolve_character_mask` |
| `crates/ludion-components/src/animation_layers.rs` | `AnimationLayers`, `AnimationLayer`, `LayerMode`, `layer_weight`, `fade_layer`, `play_layer`, `L1670`/`L1671` |
| `crates/ludion-engine/src/character.rs` | `CharacterData.masks` (one weight per skin joint) |
| `crates/ludion-engine/src/extract.rs` | `layer_poses`, `layered_joint_pose`, `sampled_palette_layered`; `joints_sampled` counts layers |
| `crates/ludion-engine/src/skin_bone.rs` | `L1672` |
| `crates/ludion-engine/tests/animation_layers.rs` | neutrality, mask, ramp, restore, threads, socket, bad-mask proofs |

Codes: `L1670` `ANIMATION_LAYERS_TOO_MANY`, `L1671` `ANIMATION_LAYER_INVALID`, `L1672`
`ANIMATION_LAYERS_ON_SKIN_BONE_SKIN`. A scene without `AnimationLayers` poses bit-identically: no
`WorldHash`, replay, trace, wire-schema or golden moved. Only `component_schemas.json` re-blessed.

### W2a — blend spaces, `Animation.params`, `BlendClock`, `blend_clocks`

| file | role |
|---|---|
| `crates/ludion-scene/src/blend_space.rs` | `triangulate`, `blend_weights_1d`/`_2d`, `BlendWeights` |
| `crates/ludion-scene/src/character.rs` | `blends:` parse (`L1199`), validate and point checks (`L1200`), `emit` |
| `crates/ludion-components/src/blend_clock.rs`, `lib.rs` | `BlendClock` (appended last, sim-only), `Animation.params` (omitted at `[0, 0]`) |
| `crates/ludion-engine/src/character.rs` | `CharacterData.blends` (`CharacterBlendData`, `BlendPointData`: weights, rate, point time) |
| `crates/ludion-engine/src/blend_clock.rs` | the `blend_clocks` system, `blend_crossings` |
| `crates/ludion-engine/src/animation_events.rs`, `extract.rs` | the library's blend map and the blend event slot; `PoseSide`/`PoseFade`, the N-way joint blend |
| `crates/ludion-engine/tests/blend_spaces.rs` | cadence, point pose, neutrality, threads, restore, crossfade, load errors |

Blend errors reuse `L1199`/`L1200`. A scene without a blend poses and hashes as before; `Animation` text
and `component_schemas.json` are the only wire-visible moves. The d2 amendment ships in its opt-in form
(only a blend-using entity hashes a `BlendClock`), still "Proposed — owner ruling owed".

### W2b (sim) — layer blends, layer `params`, `reference:`, layer events

| file | role |
|---|---|
| `crates/ludion-components/src/animation_layers.rs` | `AnimationLayer.events`/`params`/`reference`, `set_layer_params`, `L1671` on a non-finite `params` entry |
| `crates/ludion-components/src/blend_clock.rs` | `BlendClock.layers`, `LayerPhase` |
| `crates/ludion-engine/src/blend_clock.rs` | `blend_clocks` keeps the clock while a layer names a blend; `layer_fallback_phase`; `blend_crossings` takes `(prev, phase)` |
| `crates/ludion-engine/src/extract.rs` | `LayerPose.pose`/`reference` as `PoseSide`, `layer_poses` resolves a layer blend, `PoseSide::at_zero` |
| `crates/ludion-engine/src/animation_events.rs` | `ClipEvent.layer`, `walk_layers`, the "Layers" crossing rule |
| `crates/ludion-engine/tests/layer_events.rs` | events, weight 0, blend sweep and restart, scaled clock, restore, point pose, reference, neutrality |

No `WorldHash`, replay, trace, wire-schema or golden moved. A scene without a layer blend never carries a
`BlendClock`; only `component_schemas.json` re-blessed.

### A1 — part masks and `AnimationLayers` on voxel assemblies

| file | role |
|---|---|
| `crates/ludion-scene/src/rig_mask.rs` | `RigMask` (was `CharacterMask`), the shared parser, name validation, emitter and ancestor-walking `resolve_mask`; `resolve_rig_mask` (was `resolve_character_mask`) |
| `crates/ludion-scene/src/character.rs` | re-wired to the shared mask code; messages and `L1199`/`L1200` byte-identical |
| `crates/ludion-scene/src/assembly.rs` | `masks:` after `joints:`: `AssemblyDocument.masks`, parse (`L1137`), unknown part and empty name (`L1138`), emit |
| `crates/ludion-engine/src/assembly_mesh.rs` | `AssemblyData.masks` (`AssemblyMaskData`), `AssemblyData::mask`; masks stay out of the mesh content hash |
| `crates/ludion-engine/src/extract.rs` | `assembly_layer_poses`, `layered_part_pose`, `part_placements_layered`; `part_placements_with` takes the layers; `blended_part_pose` wraps an `f64` core |
| `crates/ludion-engine/src/animation_events.rs` | `walk_layers` and the baseless walk open to assemblies |
| `crates/ludion-engine/tests/assembly_layers.rs` | neutrality, mask, unkeyed channel, descendants over a non-topological order, additive and `reference`, crossfading base, restore, hash neutrality, 1/2/8 threads, events |

No component, `WorldHash`, replay, trace, wire-schema or golden moved; the component schema did not move.

### A2 — blend spaces on voxel assemblies (base and layer)

| file | role |
|---|---|
| `crates/ludion-scene/src/rig_blend.rs` | `BlendSpace` (was `CharacterBlend`), `BlendPoint`, `BlendAt`; the shared parser, validation, point check (bound, looping, sync over the caller's effective events) and emitter, taking the family's structure and consistency codes |
| `crates/ludion-scene/src/character.rs` | re-wired to the shared blend code; messages and `L1199`/`L1200` byte-identical |
| `crates/ludion-scene/src/assembly.rs` | `blends:` after `masks:`: `AssemblyDocument.blends`, parse (`L1137`), validate (`L1138`), emit, `check_assembly_blends` |
| `crates/ludion-engine/src/blend_space.rs` | `BlendSpaceData` (was `CharacterBlendData`) and `resolve_blends`, rig-neutral over each bound clip's name, duration, speed and events |
| `crates/ludion-engine/src/assembly_mesh.rs` | `AssemblyData.blends` and `AssemblyData::blend`; the blend check runs where the clips are read |
| `crates/ludion-engine/src/animation_events.rs`, `blend_clock.rs` | `AnimationLibrary.blends` keyed by model; the base and layer blend branches and `blend_clocks` accept assemblies |
| `crates/ludion-engine/src/extract.rs` | `AssemblySide`, `part_placements_sides` (the wrappers keep their signatures), `animation_pose` and `assembly_layer_poses` resolve blends, `layered_part_pose` samples `layer.side` |
| `crates/ludion-engine/tests/assembly_blends.rs` | cadence, point-equals-clip, 2D, hash neutrality, 1/2/8 threads, restore, crossfade, layer blend on a mask, no clock on a plain assembly, ragdoll switch, `L1138` refusals |

No component, `WorldHash`, replay, trace, wire-schema or golden moved; the component schema did not move.
`check-scene` does not list assembly blends yet.

## Trigger

The game's characters do locomotion plus something else on one skin: carry a body, aim, peek around a
corner, look at a stimulus, walk by stick deflection, take down a guard. `Plan/animation-clips.md`
§Non-goals deferred layers, IK, root motion and a state graph, each with a trigger. Request 0007 fires
all four: three characters (player, militia guard, android worker), built as voxel assemblies (decision 25),
share one driver shape.

## What exists (2026-11-18)

- **One clip plus one crossfade per skin.** `Animation { clip, started, speed, anchor, from*, fade, hold }`;
  `extract::sampled_palette` blends one outgoing and one incoming sample per joint. The pose is
  presentation-only (decision 2 of `Plan/animation-clips.md`); the playback inputs are hashed.
- **One pose source per skin.** `L1197` refuses `Animation` on a `SkinBone`-driven skin.
- **Events** come from the incoming clip of a crossfade only (`animation_events`).
- **Sockets.** `ParentJoint` and `posed_joint_world` go through `skin_palette`, so they read whatever pose
  extract composes.
- **Crowd posing** is parallel and thread-invariant (`par_map_chunks`, fixed chunks); every addition keeps that.

## Decisions

1. **Masks are `joints:` + `descendants:` + optional `weights:`.** One spelling. The request's
   `from: X` is `joints: [X], descendants: true`. `weights` (one per listed joint, `[0, 1]`) defaults to 1.
   With `descendants`, an unlisted joint inherits its nearest listed ancestor's weight; every other joint
   is 0. An unknown joint is `L1200` (help names the nearest); a bad shape or weight is `L1199`.
   **Why:** two ways to say "this joint and below" is the second way the pillars forbid.
2. **Layers are presentation-only, hashed inputs.** `AnimationLayers` is ordinary sim state (it snapshots,
   rides a scene switch); no system reads it. Layers evaluate in list order over the base pose; at most 4
   (`L1670`). A scene without the component is byte-identical.
3. **Override.** A joint the layer clip has no track for is untouched. The effective weight is
   `layer weight × mask weight`; exactly 1 takes the layer's channels verbatim (a missing channel reads as
   rest); below 1 slerps rotation and lerps translation and scale from the pose beneath.
4. **Additive reference is the layer clip's first frame (`t = 0`).** With layer sample `c` and reference `r`:
   rotation `base ⊗ slerp(I, r⁻¹ ⊗ c, k)`, translation `base + k (c − r)`, scale `base × (1 + k (c / r − 1))`.
   Quaternion product and conjugate are plain arithmetic. A named `reference: <clip>` is decision 21.
5. **The weight ramps from `from_weight` at `changed` over `fade` seconds**, derived from tick counts like
   `crossfade`, on the base `Animation.hold` grid. `fade_layer` writes only on change, so a per-tick caller
   does not churn the hash; `play_layer` restamps only on a new clip. Games pass `ctx.game_time`.
6. **W1 layers ignore `SkinBone`-driven skins (`L1672`) and assemblies and prefab parts.** One pose source
   per skin stays true. Masks name skin joints, so assembly parts would need part-name masks (see Proposed).
7. **Layer events are not emitted in W1.** Replaced by decision 22 (W2b): an opt-in `events: true`.
8. **Blend spaces** (W2): `blends:` in the character document, 1D by a scalar and 2D by a vector, driven by
   `Animation.params`, with phase sync through a hashed `BlendClock`. A blend emits the events of its
   highest-weight clip (ties go to the lower index) so footstep noise stays on the feet.
9. **A Rust `AnimGraph` helper** (W3), not an asset: a table of `(clip, from, leave, next, fade)` plus an
   `AnimGraphState` component, replacing `CYCLE` in `crates/ludion-testkit/tests/common/mod.rs` with an
   identical switch log and `.lrp`. There is no animation asset language.
10. **IK and look-at** (W4): `IkRig` and `IkTargets` components, a look-at chain and a two-bone solver, run
    after layers; the final pose goes through `posed_joint_world` so sockets follow.
11. **Foot grounding** (W5): read-only physics casts in extract, the 32-character budget of ≤0.5 ms, and
    `ProfileReport` counters (a schema move, re-blessed there).
12. **Root motion** (W6): a `ludion_root_motion` extras flag at import and `root_motion_delta`, a deterministic
    sim read of clip data through the libm sampler.
13. **Blend points** are `clip` plus `at`: a number is 1D (`params[0]`), a block list `[x, z]` is 2D
    (`params[0..2]`). The dimension is inferred and every point must agree; there are no param names.
14. **Weights.** 1D is linear between the sorted neighbours and clamps at the ends. 2D is barycentric over
    a deterministic triangulation: every index triple `a<b<c`, non-degenerate, with no point strictly inside
    its circumcircle, accepted in lexicographic order and skipped if it overlaps an accepted one. A query
    takes the triangle whose closest point to it is nearest, ties to the first, and clamps outside the hull.
    At most 16 points.
15. **Phase** φ is unwrapped cycles. A point clip `i` plays at `frac(φ + sᵢ) · durᵢ`, `sᵢ = t_syncᵢ / durᵢ`
    (0 without `sync`). The rate is `Animation.speed · Σ wᵢ · binding_speedᵢ / durᵢ` per game second,
    integrated by `blend_clocks` with `game_dt`. A restart (new `(clip, started)`) sets φ to the stateless
    value `elapsed · rate` (so a fresh clock agrees with decision 18) and parks `prev` at `-1`.
16. **Blend events.** A blend emits the dominant point's effective events (max weight, ties to the lower
    index), each at phase position `frac(e/durᵢ − sᵢ)`, fired on crossings in `(prev, φ]`; `prev = -1` on a
    restart tick opens the window at phase 0 inclusive. Events report the **blend** name as `ClipEvent.clip`.
17. **`hold` does not step a blend's phase** (W2a limit; trigger: a held-cadence character on a blend).
18. **With no `BlendClock`** (system not registered) extract falls back to `φ = elapsed · rate(params now)`:
    stateless, exact while the params hold still, used by tools; no events fire.
19. **A blend name may not equal a clip name** (`Animation.clip` resolves both).
20. **A layer `clip` resolves as a clip, else as a blend of the character** (decision 19 keeps the names
    disjoint). A layer blend is driven by `AnimationLayer.params`, never `Animation.params`. Its phase is a
    `BlendClock.layers` entry `{ name, blend, started, phase, prev }`, integrated by `blend_clocks` exactly as
    decision 15 does, at `layer.speed`, and restarted on a new `(clip, started)`. This happens only while the
    entity's base `Animation` sits on a character `Mesh`. Without a clock, extract falls back to
    `clock_elapsed(started, speed) · rate(params)` (decision 18).
21. **`reference: <clip>`** (additive only) takes the reference pose from that clip's first frame. Without it,
    the reference is the layer's own first frame: for a blend, each point at `t = 0` under the current
    weights. An unknown reference skips the layer silently, as an unknown mask does.
22. **Layer events are opt-in with `events: true`.** They fire only while the layer's weight at `now` is above
    0. A plain clip layer is walked **statelessly** over `(T(i − step), T(i)]` on the exact gameplay instant
    `i`, where `T` is the layer's clock times the binding speed and `step` is this tick's gameplay step. The
    window is `[0, T(i)]` when `started` lies after `floor(i − step)`. Windows tile exactly, so nothing
    repeats. A layer started by a system registered *after* `animation_events` loses only an event at exactly
    `t = 0`; register `play_layer` callers before it, as for `Animation`. A raw `speed` write mid-clip may
    skip or repeat one crossing that tick. A blend layer fires its dominant point's events over its
    `BlendClock.layers` window (decision 16). Events carry `ClipEvent.clip` = the layer's clip or blend name
    and `ClipEvent.layer` = the layer name. Base events have `layer: ""`. Order: an entity's base events,
    then its layers' in list order.
23. **`set_layer_params(&mut layers, name, params) -> bool`** writes only on change. The request's `param: N`
    indices into `Animation.params` are not built (decision 13): a layer owns its two params.
24. **`hold` does not step layer blend phases** (decision 17, same trigger).

25. **Assemblies are the game's rig** (owner ruling 2026-11-17, request 0007 §Summary). The shipped skinned
    path stays for skinned games; each later wave lands on assemblies first.
26. **Assembly `masks:` use decision 1's spelling, with `joints:` naming parts**, under `L1137`/`L1138`. The
    request's `except:` is spelled by listing the excluded subtree root with weight 0 under
    `descendants: true`. Resolution walks ancestors, because authored part order is not topological.
27. **Assembly override** blends each channel the layer clip keys toward the layer by `k = weight × mask`,
    verbatim at `k ≥ 1`. A channel it does not key keeps the base, and a part with no track is untouched.
    This is the request's preference; it differs from skins, where a tracked joint's missing channel reads rest.
28. **Assembly additive** measures from the part's **rest**, because a rigid part's sample already is its delta
    from rest (request §2). A `reference: <clip>` replaces rest with that clip's first frame:
    `R = R_base · slerp(I, r⁻¹ c, k)`, `T = T_base + k (c − r)`, `S = S_base · (1 + k (c/r − 1))`, with
    `r = (I, 0, 1)` by default.
29. **Layers on assemblies are presentation only**, read on the base `hold` grid (decision 5). The ragdoll
    switch keeps sampling the base pose alone; trigger: a ragdoll that must start from a layered pose. Layer
    events work on assemblies exactly as decision 22.
30. **`excluded: local | model`** (legs staying planted under a torso lean) is deferred, an owner question.
    Trigger: the owner confirms it, or a layered torso visibly swings the legs.
31. **Assembly `blends:` use decisions 13–19** through the shared parser (`L1137`/`L1138`). Point clips are
    bound in `clips:` and loop. `sync` is among the clip's own events (an assembly has no overlay). A
    point's binding speed is 1. A blend name may not equal a clip binding.
32. **The assembly N-way part blend.** Walk the entries in order: rotation is a running slerp `w / accumulated`,
    a missing rotation read as identity; translation and scale are weighted sums, missing read as 0 and 1; a
    channel **no** entry keys stays `None`, so an untouched part keeps its exact rest. One weight-1 entry is that
    clip's `part_pose` verbatim.
33. **The ragdoll switch samples the base blend** at the switch instant, from the `BlendClock` phase advanced by
    the tick fraction, else the stateless fallback. Layers stay excluded (decision 29).
34. **Layer blends on assemblies follow decision 20** (`BlendClock.layers`, `AnimationLayer.params`). An
    additive blend layer measures from rest unless `reference:` is set (decision 28).

## Waves

- **W1 (shipped).** Masks, `AnimationLayers`, composition in extract's `Skin` phase, `L1670`–`L1672`, docs.
- **W2a (shipped).** 1D/2D blend spaces, `Animation.params`, `BlendClock`, `blend_clocks`, blend events
  (decisions 13–19).
- **W2b (sim, shipped).** A layer's `clip` naming a blend, layer `params`, `reference:`, layer `events`
  (decisions 20–24).
- **A1 (shipped).** Assembly `masks:`, override and additive layers in the assembly path, layer events
  (decisions 25–30).
- **A2 (shipped).** Assembly `blends:` and layer blends, sharing the blend parser through `ludion-scene`
  (decisions 31–34).
- **A3.** Prefab-part layers and blends, in `prefab_part_poses`.
- **W2c.** `render --model … --blend/--layer` strips (`model_sheet.rs` + CLI), gallery
  `assembly-layers.yaml` and its goldens, after A2 so they show an assembly (pinned goldens owed).
- **W3.** `AnimGraph` and `AnimGraphState` (decision 9).
- **W4.** Look-at and two-bone IK (decision 10).
- **W5.** Ground feet, the bench, profile counters (decision 11).
- **W6.** Root motion import and `root_motion_delta` (decision 12).

## Proposed — owner ruling owed

- Lifting `animation-clips` decision 5 ("masked layers, root motion and IK are non-goals"): W1 ships the
  conservative opt-in variant — absent component, identical bytes.
- Amending `animation-clips` decision 2 ("the pose is visual-only"): W2's `BlendClock` and W6's root motion
  are sim reads of clip data (the trade decision 15 made for events).
- The `AnimGraph` helper against `animation-clips` decision 4 and `voxel-animation` decision 6 (no state
  machine asset): it is Rust, not data.
- The W5 budget (≤0.5 ms for 32 characters).
- `animation-clips` decision 10's merge trigger fired (a character feature assemblies also want); merging
  `kind: character` and `kind: assembly` is not done here — Proposed, owner ruling owed.

## Non-goals

- Layers on prefab parts. **Trigger:** A3.
- Per-layer anchors (`set_clip_speed` for a layer). **Trigger:** a layer clip that must change rate mid-clip.
- Layer-to-layer blending (a crossfade inside one layer). **Trigger:** a layer whose clip switch visibly pops.
- A masked crossfade of the base. **Trigger:** a base switch that must not touch the upper body.

## Risks

- **Per-frame cost.** A layer is another sampler pass per skinned entity per frame. Cap 4, and `joints_sampled`
  counts `joints × (1 + fading + layers)` so the profile shows it; the W5 bench holds the line.
- **A layer clip names joints the mesh lacks.** The character's clips are checked at load, so this cannot
  happen for a bound clip; an unknown clip or mask name is a silent degrade, as for `Animation`.
- **Weight ramp on the hold grid.** A held base steps the layer weight with it; that is intended (stop-motion
  reads as one cadence).
