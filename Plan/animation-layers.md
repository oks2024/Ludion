# Plan: Animation layers — bone masks, layers, blend spaces, IK, root motion and a state-graph helper

> Status: **W1 mechanized (2026-11-18); W2–W6 not started.** Driver: Neon Shroud request 0007
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

## Trigger

The game's characters do locomotion plus something else on one skin: carry a body, aim, peek around a
corner, look at a stimulus, walk by stick deflection, take down a guard. `Plan/animation-clips.md`
§Non-goals deferred layers, IK, root motion and a state graph, each with a trigger. Request 0007 fires
all four: three skinned kinds (player, militia guard, android worker) share one driver shape.

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
   Quaternion product and conjugate are plain arithmetic. A named `reference: <clip>` is W2.
5. **The weight ramps from `from_weight` at `changed` over `fade` seconds**, derived from tick counts like
   `crossfade`, on the base `Animation.hold` grid. `fade_layer` writes only on change, so a per-tick caller
   does not churn the hash; `play_layer` restamps only on a new clip. Games pass `ctx.game_time`.
6. **W1 layers ignore `SkinBone`-driven skins (`L1672`) and assemblies and prefab parts.** One pose source
   per skin stays true. Masks name skin joints, so assembly parts would need part-name masks (see Proposed).
7. **Layer events are not emitted in W1.** W2 adds an opt-in `events: true` per layer (default off, so a carry
   loop does not double footsteps); events report the layer name.
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

## Waves

- **W1 (shipped).** Masks, `AnimationLayers`, composition in extract's `Skin` phase, `L1670`–`L1672`, docs.
- **W2.** 1D/2D blend spaces, `Animation.params`, `BlendClock`, blend events, a layer's `clip` naming a blend,
  layer `events`, `reference:`, `render --blend/--layer` strips, gallery `character-layers.yaml` (pinned
  goldens owed).
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
- The rig form: skinned `kind: character` or rigid-part `kind: assembly`. Masks would resolve part names,
  layers would compose in the assembly and prefab-part paths, and `animation-clips` decision 10's merge
  trigger fires.

## Non-goals

- Layers on assemblies and prefab parts. **Trigger:** the rig-form ruling above.
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
