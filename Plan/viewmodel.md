# Plan: Viewmodel — a first-person held-object layer in the main scene pass

> Status: **in progress** — components and codes shipped (subtask 1a); extraction and rendering follow. An external game's request 0003 (priority *blocking*). Code facts read from the tree on 2026-11-07 (file:line pointers are of that date; re-grep before editing).

## Context

A first-person game draws a held gun that must read clearly at all times: it never clips into a wall when fired point-blank, it keeps a fixed apparent size under FOV kick, and it is lit by the scene with emissive parts that bloom. The engine has one scene camera (a second is `L2101`), one depth buffer in standard [0,1] depth, no per-camera render layers, and no depth-range control. Every workaround fails:

- **The gun parented to the camera at world depth** pokes through walls and scales with the gameplay FOV.
- **A scaled-down gun near the eye** breaks lighting, shadows and perspective.
- **The overlay pass** is UI and debug only and cannot light scene meshes.
- **A gun at a faked depth** leaves SSAO and SSR halos on the world around it, and the wall it pokes into shadows it.

So the engine needs a viewmodel layer: a per-entity flag, an own projection and a reserved near depth slice in the same pass, shadow and post rules that keep the gun readable, and a camera source that cannot swim against the world at high refresh rates.

## Decisions

1. **A flag plus an optional lens.** `Viewmodel` is a field-less flag, authored `Viewmodel: {}`, inherited down `Parent`/`ParentJoint` by **OR** (one flagged ancestor makes every descendant mesh a viewmodel; `Visibility` folds by AND). An optional `ViewmodelLens { fov_y_deg, near, depth_slice }` on the camera entity sets the projection; absent means the defaults, 55° / 0.01 / 0.01. The lens projection is independent of the gameplay FOV, `CameraOffset.fov_delta` and the late latch.
2. **Hash posture.** The gun is authored on `presentation: true` entities parented to the sim camera (`docs/presentation.md`), so the flag stays out of `WorldHash`. On a sim entity the flag is hashed like `Visibility`. `ViewmodelLens` on the sim camera is hashed; it is constant config. The flag is render-only: the sim never reads it.
3. **One depth buffer and one scene pass.** The world's depth is remapped into the rest of [0,1] only on frames that draw a viewmodel; the viewmodel gets the reserved slice. Every depth consumer — SSAO, SSR, DoF, fog, clustered lookup, transparent sort and `render.view` picks — decodes through one shared helper, so a remapped frame and an ordinary frame cannot disagree.
4. **Camera source.** The gun's camera-local transform is composed onto the frame's drawn camera at one site, a renderer `compose_viewmodel(scene, camera_model)`, called by extract, `blend` and `latch_camera` with the exact model whose inverse is the world view, so the gun cannot swim against the world. That local is `cam_entity⁻¹ · instance model`, computed in f64, so every mesh path inherits it, skins and morphs included. Its limit is about 1 ulp of the camera's world coordinates; an exact chain fold is a follow-up.
5. **Under a `render.view` override** a viewmodel draws as an ordinary world mesh.
6. **Misuse is a diagnostic, never a crash.**
   - `L1336` (warning): a `Viewmodel` mesh exists but the scene has no single camera that is both perspective and a first-person `CameraRig`. Reported once; the mesh draws as an ordinary world mesh, shadows included.
   - `L1337` (warning): a `ViewmodelLens` is on an entity without `Camera`; ignored and reported once.
   - `L1338` (error): a lens field is out of range (`fov_y_deg` finite in [1, 179], `near` finite and above 0, `depth_slice` finite in (0, 0.5]). A scene with one is refused; at runtime the defaults are used and it is reported once.
7. **Shadows.** Flagged items cast no world shadow (`cast_shadows: false` at extract). They receive a single unfiltered eye-position shadow sample, so the wall they poke into cannot darken them.
8. **Post exclusion.** SSAO, SSR and DoF skip viewmodel pixels and never sample across them; bloom, exposure, tonemap, grade and SMAA apply as for the world. Stencil versus a mask target is picked by the Planner of subtask 3, whichever the post chain samples cheapest.
9. **The flag lives on the draw item**, so skins and morphs inherit it. The skinned proof is owed to `skeletal-animation`.

## Subtasks

- **1a** — components (`Viewmodel`, `ViewmodelLens`), codes `L1336`–`L1338`, the schema re-bless, the `docs/components.md` rows and this plan.
- **1b** — extraction and packet: `MeshInstance.viewmodel`, `RenderScene.viewmodel`, `compose_viewmodel`, the `L1336`/`L1337` runtime reports. No GPU change.
- **2a** — depth slice, remap and the shared decode.
- **2b** — the clustered lookup on true view-z, the eye shadow sample, and keeping viewmodel meshes out of the GI voxelizer, dynamic injection and probe captures.
- **3** — post exclusion for SSAO, SSR and DoF.
- **4** — viewmodel-layer particles and the `camera` docs topic section.

## Acceptance

- Engine goldens: the viewmodel pressed against a close wall shows no clipping; it overlapping a close enemy draws in front of it; SSAO on shows no halo around it; a muzzle-flash particle in the viewmodel layer.
- Engine tests: the flag does not change world shadows or `WorldHash`; the clustered lookup uses the true view-z, e.g. a point light behind the eye still lights the gun.
- A docs topic.

## Status

