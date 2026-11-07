---
description: Viewmodel and ViewmodelLens — a first-person gun that never clips; light, post, muzzle flash
---

# Viewmodel: a first-person held object

**Scope:** `Viewmodel`, `ViewmodelLens`, `L1336`–`L1338`, and what the renderer does with a flagged
mesh or emitter: its own projection, a reserved depth slice, lighting, shadow, post and a muzzle flash.
The camera it hangs from is `ludion docs camera`; the presentation entity that keeps it out of the
hash is `ludion docs presentation`. The design record is `Plan/viewmodel.md`.

## Authoring the gun

Three parts: a first-person `CameraRig` camera (optionally with a `ViewmodelLens`), a gun entity
`Parent`ed to it, and `Viewmodel: {}` on the gun. Make the gun `presentation: true` so the flag stays
out of `WorldHash` (on a sim entity it is hashed like `Visibility`; `ViewmodelLens` on a sim camera
is hashed, as constant config).

```yaml
- name: camera
  components:
    Transform: { translation: [0.0, 1.6, 0.0] }
    Camera: { projection: { type: perspective, fov_y_deg: 70.0 } }
    CameraRig: { target: { index: 1, generation: 0 }, rig: { mode: { first_person: {} } } }
    ViewmodelLens: {}
- name: gun
  presentation: true
  components:
    Parent: { index: 0, generation: 0 }   # the camera
    Transform: { translation: [0.22, -0.18, -0.45], scale: [0.08, 0.08, 0.45] }
    Mesh: { shape: cube, color: [0.1, 0.9, 0.35, 1.0], roughness: 0.4 }
    Viewmodel: {}
```

Runnable: `examples/gallery/viewmodel-wall.yaml` (a gun pushed through a close wall),
`viewmodel-muzzle.yaml` (adds a flash), `viewmodel-ssao.yaml`, `viewmodel-enemy.yaml`.

`Viewmodel` is inherited down `Parent`/`ParentJoint` by OR: one flagged ancestor makes every descendant
mesh a viewmodel. The gun's `Transform` is camera-local, so recoil and sway are local offsets written
to the gun each frame.

## The lens

`ViewmodelLens` on the camera entity: `fov_y_deg` (`55`, 1–179), `near` (`0.01`, > 0), `depth_slice`
(`0.01`, in (0, 0.5]); `ViewmodelLens: {}` is the defaults and absent means the defaults. It is
independent of the gameplay `fov_y_deg`, `CameraOffset.fov_delta` and the late latch, so a sprint FOV
kick does not stretch the gun.

The gun draws in the same pass as the world but in a reserved depth slice `[0, depth_slice)`; the
world is remapped into the rest of `[0, 1]`, only on frames that draw a viewmodel. So a wall 0.25 in
front of the eye can never clip the gun, and the gun draws over a close enemy. Without a viewmodel the
frame is byte-identical to one without this feature.

The world near plane against the player collider is the game's job: keep the collider radius at or
above the near-plane corner distance, or the camera sees through geometry the gun is hiding.

## Camera source

The gun is composed onto the frame's *drawn* camera: the interpolated, late-latched, `CameraOffset`
camera the world is drawn from, at one site. The gun and the world turn together at any refresh rate
and the gun does not swim. See `ludion docs camera` (the frame's resolved camera).

## Lighting and shadow

Every light whose range reaches the gun's bounds lights it, under any lens (even one wider than the
world's, and a point light behind the eye). Sun and local-light shadows are sampled once at the camera
eye, unfiltered, and applied to the whole gun, so the wall it pokes into cannot darken it. The gun
casts no world shadow, and is kept out of probe captures and the GI voxel grid (probe keys do not move
with the camera).

## Post

SSAO, SSR and DoF skip gun pixels: no AO halo around the gun, no reflection on it, and it stays sharp
whatever is blurred behind it. Bloom, exposure, tonemap, grade and SMAA apply as to the world.
`ludion docs post-processing`.

## Muzzle flash

A `ParticleEmitter` whose entity is `Viewmodel` or has a `Viewmodel` ancestor is a viewmodel emitter:
no extra field. It simulates in the camera's frame, draws last through the lens inside the gun's depth
slice, and stays on the drawn muzzle (see `viewmodel-muzzle.yaml`). It is render-only, so `WorldHash`
is unchanged. **Parent it to an unscaled muzzle node**, never to a non-uniformly scaled mesh: particles
read `scale.x`, so a squashed gun squashes the flash. Lingering smoke moves with the camera by
construction; put it on a world emitter. `ludion docs particles`.

## Misuse

- `L1336` (warning, once): a `Viewmodel` mesh exists but the scene has no single perspective
  first-person `CameraRig` camera. It draws as an ordinary world mesh, shadows included.
- `L1337` (warning, once): `ViewmodelLens` on an entity without `Camera`; ignored.
- `L1338` (error): a lens field out of range. The scene is refused; at runtime the defaults apply.
- Under a `render.view` override the gun draws as an ordinary world mesh.

## Known limits

- GI sky visibility and probe irradiance are read at the gun's world position.
- An SSR ray crossing the gun's footprint falls back to probes or sky.
- Reflections, fog, shafts and decals see a gun pixel at about the world near plane; the measured fog
  error is invisible at authored densities (`σ ≤ 0.1`).
- A flash writes no depth, so DoF and fog over it follow what lies behind it.
- A `lit` viewmodel particle's shading reads the screen tile; flashes are additive and unlit.
- A skinned gun is unproven.

## Next

- `ludion docs camera` — the first-person rig and its kick channels.
- `ludion docs presentation` — presentation entities.
- `ludion docs particles` — `ParticleEmitter`.
- `ludion docs projectiles` — pooled 3D shots swept against physics and the `spatial3d` index.
