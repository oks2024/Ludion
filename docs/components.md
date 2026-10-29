---
description: Declaring and registering components, drift safety, and the core scene/render component table
---

# Components

**Scope:** Declaring your own components, registering them, and keeping them safe across a `ludion dev` reload — plus the built-in components that place, light, and frame a scene. The UI and effect/content components are split into `ludion docs components-ui` and `ludion docs components-fx`; systems that read any of these are systems, the file syntax is scene-format.

A component is a plain struct deriving `Serialize`, `Deserialize`, and `schemars::JsonSchema` (those
three are *your* dependencies — the facade does not re-export the derive macros). No `impl Component`:
any such struct qualifies via a blanket impl.

```rust
use serde::{Deserialize, Serialize};
use schemars::JsonSchema;

#[derive(Serialize, Deserialize, JsonSchema, Clone, Copy, Debug, PartialEq)]
pub struct Ball {
    #[serde(default)]
    pub velocity: [f32; 2],
}
```

Register each on top of the built-ins (see `register` in `src/lib.rs`); a name/type collision is an
`L0005` diagnostic at build, never a silent overwrite.

## Built-in components: scene and render

Your game composes what is drawn from these; game logic lives in your own components. Fields and
defaults (from `crates/ludion-engine/src/components.rs`):

| Component | Fields (default) |
|---|---|
| `Transform` | `translation: [f32;3]` (`[0,0,0]`), `rotation: [f32;3]` Euler XYZ **degrees** (`[0,0,0]`), `scale: [f32;3]` (`[1,1,1]`) — local to its `Parent` when it has one (`ludion docs hierarchy`) |
| `Mesh` | `shape` (`cube`) — a primitive `cube\|plane\|sphere`, a mesh asset `{ asset: "meshes/rock.mesh.yaml" }`, a voxel model `{ voxel: "voxels/lantern.voxel.yaml" }`, a multi-part voxel assembly `{ assembly: "models/player.assembly.yaml" }`, an implicit-surface sculpt model `{ sculpt: "sculpts/idol.sculpt.yaml" }`, or a skinned character `{ character: "characters/hero.character.yaml" }` (paths relative to the content root, or to the prefab file inside a `kind: prefab`; an assembly's part files resolve relative to the assembly document, and one entity draws every part); `color: [f32;4]` (white; RGB `>=0`, alpha `0..=1`: `L1263`); `roughness: f32` (`1.0`), `metallic: f32` (`0.0`), `emissive: f32` (`0.0`) — inert on a mesh that authors its own material (a voxel, assembly or sculpt palette); `flash: f32` (`0.0`) / `flash_color: [f32;3]` (white) are the same hit-flash pair `Sprite` carries. See `ludion docs voxel-format`, `ludion docs sculpt-format`, `ludion docs materials` |
| `SkinBone` | `skin: EntityId` — the skinned mesh entity (a `Mesh` `shape: asset` naming a document with `joints:`), `bone: String` — one of its joint names. A part carrying `SkinBone` and no authored `Transform` spawns at `skin`'s world transform times `bone`'s rest model transform (a non-uniform `skin` scale under a rotated bone shears, which a TRS `Transform` cannot hold — author a uniform skin scale); an authored `Transform` wins outright. Four diagnostics refuse a bad link: the named entity not a skinned mesh (`L1194`), `bone` naming no joint of it (`L1195`), two parts naming the same `(skin, bone)` (`L1196`), and `skin` also carrying an `Animation` — one pose source per skin (`L1197`). The part is the bone's pose source, every frame: `extract` sets the named joint's bone-palette entry from the part's live world transform each frame it draws; a joint no part names keeps its rest pose relative to its parent. Render-only — a runtime-bad link degrades silently to rest rather than erroring |
| `PointLight` | `color: [f32;3]` (white, `>=0`: `L1263`), `intensity: f32` (`1.0`, `>=0`), `range: f32` (`10.0`, `>0`) — position from the `Transform`; a scene may place any number, and what that costs is `ludion docs lighting --section "Point lights: place as many as the look needs"` |
| `SpotLight` | `color: [f32;3]` (white, `>=0`: `L1263`), `intensity: f32` (`1.0`, `>=0`), `range: f32` (`10.0`, `>0`), `direction: [f32;3]` (`[0,-1,0]`, world-space, normalized at extract), `inner_angle`/`outer_angle: f32` (`20`/`30` half-angles in degrees, `[0,90)`, inner `<=` outer: `L1272`) — a point light times a smooth cone; position from the `Transform`, no shadows, no GI; see `ludion docs lighting --section "Spot lights: a cone, and the pool it throws"` |
| `DirectionalLight` | `direction: [f32;3]` (`[-0.3,-1.0,-0.2]`, normalized at extract, non-zero+finite: `L1123`), `color: [f32;3]` (white, `>=0`: `L1263`), `intensity: f32` (`1.0`, `>=0`), `shadow_extent: f32` (`40.0`, `>0`), `shadow_near: f32` (`1.0`) below `shadow_far: f32` (`200.0`) (`L1123`), `shadow_bias: f32` (`0.05`, `>=0`, world units at a 2048² map), `angular_radius: f32` (`0.0`°, `[0,45]`: `L1282`, specular disc) — the "sun"; see `ludion docs lighting --section "Sun and shadows"` |
| `Environment` | `sky_zenith`/`sky_horizon`/`sky_ground: [f32;3]` (a dim night gradient), `sun_disc_size: f32` (`0.0`° = none), `sun_disc_intensity: f32` (`1.0`), `ambient: [f32;3]` (`[0,0,0]`, a flat floor under the IBL), `ibl_intensity: f32` (`1.0`) — opt-in sky + image-based lighting + the ambient term (*lighting*); absent, the pipeline is byte-identical without it. See `ludion docs environment` |
| `PostProcess` | 33 fields in five groups — bloom/exposure/tonemap/dither, the colour grade, the participating medium (fog *and* light shafts), ambient occlusion, and the lens (depth of field, vignette, film emulation). The opt-in *imaging* singleton (what the post stack does to the rendered image, as against `Environment`'s lighting); absent — or empty as `PostProcess: {}` — the pipeline is byte-identical without it. See `ludion docs post-processing` |
| `Camera` | `projection` — `perspective` with `fov_y_deg`, or `orthographic` with `height` plus `pixel_snap: bool` (`false`; `true` rounds the view height to a whole texel-to-pixel scale and each sprite centre onto the atlas texel grid, so crisp art stops shimmering under motion — extract-only, the world hash never moves); default 60° perspective. `near: f32` (`0.1`, `> 0`: `L1014`) below `far: f32` (`1000.0`) (`L1264`) |
| `Parent` | `Parent(EntityId)` — the entity's `Transform` is local to this parent; the composed world pose is derived, never stored. Despawning a parent despawns its subtree. A rigid or static solver collider under a `Parent` is refused (`L1221`/`L1222`). `ludion docs hierarchy` |
| `PrefabInstance` | `{ source, part, root }` — which prefab file a part was spawned from, its name in the prefab, and the instance's first part (`root`, remapped on load and restore). Written by `spawn_prefab`, `prefab.spawn` and scene instance entries, never authored. `ludion docs prefabs` |
| `Fade` | `fade: f32` (`1.0`, `[0,1]`), `height_min: f32` (`-f32::MAX`) — per-instance occlusion dither; render-only, game-policy-driven; a mesh without one takes its nearest ancestor's. See `ludion docs environment --section "Occlusion fade: dither, top-fade, cutaway"` |
| `GiVolume` | `cell_size: f32` (`0.5`, `>0`), `probe_spacing: f32` (`1.0`, `>0`), `normal_bias: f32` (`0.25`, `>=0`), `sky_visibility: f32` (`1.0`, `[0,1]`), `bounce: f32` (`0.0`, `>=0`, UI travel `[0,2]`) — opt-in baked sky-visibility volume attenuating indirect light only; absent, the pipeline is byte-identical without it. See `ludion docs global-illumination` |
| `GiDynamic` | *(no fields, authored `GiDynamic: {}`)* — opt-in per-entity marker for what a system moves: its `PointLight` is injected fresh each frame instead of baked, its `Mesh` neither occludes the probe rays nor deposits into the voxelization; absent, the entity feeds the static bake exactly as before the marker existed. See `ludion docs global-illumination` |
| `ReflectionProbe` | `shape`, `blend`, `priority` — a captured reflection volume. See `ludion docs reflection-probes` |
| `Residue` | *(no fields, authored `Residue: {}`)* — opt-in per-entity marker for state that stays with its **scene** rather than travelling with the player: the components are captured when a switch leaves the scene and re-applied when the run returns to it this session (identity is the entity's `name:`; an unnamed one respawns). Absent, the scene is rebuilt from its text on every visit, exactly as before the marker existed. The runnable pattern is `crates/ludion/examples/scene_residue.rs` |
| `GroundMarker` | `shape` (`{ type: disc, radius: 1.0 }`) — `type: disc { radius }` or `type: segment { length, width }` (local +X is the segment's length axis); `color: [f32;4]` (`[1,1,1,1]`) — linear RGBA, HDR-capable (RGB `>=0`, alpha `0..=1`: `L1263`). An analytic, SDF-rendered ground telegraph, alpha-blended after opaque meshes and before sprites. Every shape dimension must be strictly positive and finite (`L2106`). See `ludion docs ground-telegraphs` |
| `DropShadow` | `shape` (`{ type: disc, radius: 0.5 }`), `opacity` (`0.6`), `softness` (`0.5`), `max_distance` (`20.0`), `fade_distance` (`4.0`) — a projected contact shadow on the first surface straight below the entity, drawn after opaque meshes. Shape dimensions `> 0` (`L2106`), opacity and softness in `0..=1` (`L1014`), distances positive with `fade_distance <= max_distance` (`L2110`). See `ludion docs drop-shadow` |

The screen-space UI primitives (`UiRect`, `UiText`, `UiList`) and world `Text` are
`ludion docs components-ui`. The content and effect components (`Sprite`, `Tilemap`,
`ParticleEmitter`, `Volley`, `PatternPlayer`, `Animation`, `AudioEmitter`) are
`ludion docs components-fx`.

A camera's pose comes from its entity's `Transform` (it looks down local −Z). Exactly one `Camera` is
expected — two is `L2101`; zero uses a default framing.

**Framing a shot from the CLI** — `ludion render --scene examples/gallery/post-full.yaml --look-at 0,1,-4
--azimuth -15 --pitch 30 --distance 12 --out target/framed.png` orbits the scene's root camera around the
target and sets `PostProcess.focus_distance` to the distance. Azimuth is degrees about +Y from +Z (0 puts the
camera on the target's +Z side), pitch degrees looking down, `--fov` the vertical field of view; an omitted
angle or distance is read off the scene camera, so `--look-at` alone reproduces its view. It expands into
echoed `--set` overrides (an explicit `--set` wins) and needs no JSON pointers; in Git Bash a remaining
`--set /…` needs `MSYS_NO_PATHCONV=1`. A follow camera builds its `Transform` from `ludion::math::Orbit` and
writes `PostProcess.focus_distance = orbit.distance`. A `CameraRig` on the camera writes its `Transform` instead (`ludion docs camera`).

**Calibrating a `PointLight`** — the falloff math, the gallery scenes' reference `intensity`/`range`
values — is `ludion docs lighting --section "Point lights: place as many as the look needs"`.

**Ground telegraphs (`GroundMarker`)** — shape/color format, draw order, and the bloom interaction —
is `ludion docs ground-telegraphs`.

**Projected contact shadow (`DropShadow`)** — the first-surface rule, the footprint and the fade — is `ludion docs drop-shadow`.

## Keep components drift-safe

`ludion dev`'s reload loop (`ludion docs dev-loop`) recompiles your game, snapshots the running world,
restarts the process on the new code, and restores the snapshot — so every restore is a *migration*:
the document was serialized against the previous build's component shape and is deserialized against
this one's. A field you just added is simply absent from that document. If the field has
`#[serde(default)]`, it fills in and the rest of the component's data survives untouched. If it
doesn't, the whole component fails to deserialize and gets dropped (loudly, with a migration report)
— losing data that had nothing to do with the new field.

The rule: **give every field on every component `#[serde(default)]`** (as `Ball` above does, or back
the component with a `Default` impl and a matching `Deserialize`), so the component can always be
built from an empty object `{}` — the worst case, where every field it has is one the saved document
never carried.

`ludion new` scaffolds this as a real, enforced test: `tests/drift.rs` calls
`ludion_testkit::assert_components_drift_safe(&<game>::registry())`, which panics naming the offending
component and field the moment you forget the annotation, instead of letting it surface later as an
opaque failure mid-reload. (Exempt: identity/link components like the built-in `Parent` — a required
entity reference has no safe empty value, so weakening it to a default would silently manufacture a
link to a real entity rather than fail loudly; that is worse than the problem this discipline
prevents.)

## Next

- `ludion docs components-ui` — `UiRect`/`UiText`/`UiList`/`Text`: the screen-space and world-text components.
- `ludion docs components-fx` — `Sprite`/`Tilemap`/`ParticleEmitter`/`Volley`/`PatternPlayer`/`PathFollower`/`Animation`/`AudioEmitter`: content and effect components.
- `ludion docs motor` — `CharacterMotor`/`SurfaceVelocity`; `ludion docs lane` — `LaneConstraint`; `ludion docs camera` — `CameraRig`.
- `ludion docs systems` — the system contract, input, and the determinism rules.
- `ludion docs scene-format` — the strict-YAML scene document.
- `ludion docs materials` — the three material scalars.
- `ludion docs lighting` — the sun and its shadow map, `PointLight` calibration, and the `SpotLight` cone.
- `ludion docs global-illumination` — the baked `GiVolume` sky-visibility volume.
- `ludion docs ground-telegraphs` — the `GroundMarker` disc/segment ground telegraph.
- `ludion docs drop-shadow` — the `DropShadow` projected contact shadow.
- `ludion docs dev-loop` — the live reload loop.
