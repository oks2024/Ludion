# Example gallery

Every scene under `gallery/` demonstrates exactly **one** rendering feature and is named for it
(the file stem is the feature). Each is a golden test: `ludion-testkit::discover` pairs
`gallery/<stem>.yaml` with `gallery/goldens/<os>/<stem>.png` automatically (goldens are blessed per
pinned OS: `windows`/`macos`) — no manifest needed to add a
scene, just drop the `.yaml` next to the others and bless once.

`examples/scenes/` holds format fixtures rather than feature demos (see below).

## Gallery index

| Scene | Feature | Components exercised | See it |
|---|---|---|---|
| `gallery/sprites.yaml` | `Sprite` — textured and tinted quads | `Transform`, `Camera`, `Sprite`, `PointLight` (lighting only, not the feature) | `ludion render --scene examples/gallery/sprites.yaml --out sprites.png` |
| `gallery/meshes.yaml` | `Mesh` — the three low-poly primitives (`shape: cube`, `plane`, `sphere`) | `Transform`, `Camera`, `Mesh`, `PointLight` (lighting only) | `ludion render --scene examples/gallery/meshes.yaml --out meshes.png` |
| `gallery/mesh-asset.yaml` | `Mesh` — an authored mesh asset (`shape: { asset: … }`); a hand-written `meshes/crystal.mesh.yaml` (faceted normals computed at load) | `Transform`, `Camera`, `Mesh` (asset crystal + primitive ground), `PointLight` (lighting only) | `ludion render --scene examples/gallery/mesh-asset.yaml --out mesh-asset.png` |
| `gallery/voxel-model.yaml` | `Mesh` — an authored voxel model (`shape: { voxel: … }`); hand-written `voxels/lantern.voxel.yaml` + `voxels/tree.voxel.yaml`, greedily meshed with per-quad palette colors (the two trees are the same model, deduplicated to one upload) | `Transform`, `Camera`, `Mesh` (voxel lantern + two voxel trees + primitive ground), `PointLight` (lighting only) | `ludion render --scene examples/gallery/voxel-model.yaml --out voxel-model.png` |
| `gallery/material-park.yaml` | `Mesh` **materials** — a 6×6 `roughness`×`metallic` sweep of spheres (component fields only) + an emissive-glass/metallic-trim voxel `beacon` (palette material fields), lit to show highlights and bloom | `Transform`, `Camera`, `Mesh` (36 sweep spheres + primitive ground + voxel beacon), `PointLight` (warm key + cool fill) | `ludion render --scene examples/gallery/material-park.yaml --out material-park.png` |
| `gallery/night-sky.yaml` | `Environment` — a procedural **moonlit-gradient sky** (authored `sky_zenith`/`horizon`/`ground`, no image asset) that is GPU-prefiltered into **image-based lighting**: the same 6×6 `roughness`×`metallic` sweep as `material-park`, but the metals reflect the sky (irradiance + prefiltered specular via the split-sum BRDF LUT) instead of going black under flat ambient. A `DirectionalLight` moon adds a cool key + its shadow; warm `PointLight`s lift the foreground; the emissive voxel `beacon` blooms (a `PostProcess` on the same entity raises `bloom_strength`). A scene with no `Environment` is byte-identical to before. | `Environment` (the feature), `PostProcess` (bloom knobs), `DirectionalLight` (moon), `Transform`, `Camera`, `Mesh` (36 sweep spheres + ground + voxel beacon), `PointLight` (warm key + cool fill) | `ludion render --scene examples/gallery/night-sky.yaml --out night-sky.png` |
| `gallery/occlusion-fade.yaml` | `Fade` — the render-only occlusion-fade component. Three tall walls each carry an authored `Fade` driving a deterministic screen-door (Bayer) dither *discard* on the opaque mesh pass (no blend, no depth-write change): a light stipple (`fade: 0.75`), a top-fade wall (`fade: 0.5` + `height_min` at mid-height — the ground floor stays solid, only the upper stories dither), and a heavy near-cutaway stipple (`fade: 0.15`). **WHO fades is game policy** (a camera→player raycast, G7's job); this golden pins the pattern the `occlusion_fade` example animates. A wall with no `Fade` (or `fade: 1.0`) is byte-identical to the pre-fade mesh path. | `Transform`, `Camera`, `Fade` (the feature), `Mesh` (walls + ground), `PointLight` (lighting only) | `ludion render --scene examples/gallery/occlusion-fade.yaml --out occlusion-fade.png` |
| `gallery/colliders.yaml` | `Collider` — circle and aabb collision shapes on the ground plane. **The golden shows authored layout, not behavior**: colliders are render-inert (each lit mesh's visible x/z size mirrors its entity's collider `radius`/`half_extents`), and the same scene stripped of its `Collider` blocks renders byte-identically (asserted). Collision *behavior* is proven by `ludion-spatial`'s conformance suite, not this image. | `Transform`, `Camera`, `Collider` (the feature), `Mesh` (visualization only), `PointLight` (lighting only) | `ludion render --scene examples/gallery/colliders.yaml --out colliders.png` |
| `gallery/ground-markers.yaml` | `GroundMarker` — analytic, SDF-rendered ground telegraphs: a translucent HDR warning disc (radius, soft edge) and a charge-line segment (capsule, non-axis-aligned yaw). An opaque pillar stands in the disc to show draw order — geometry occludes the marker where it stands, since the marker pipeline is depth-tested but does not write depth. | `Transform`, `Camera`, `GroundMarker` (the feature), `Mesh` (occluding pillar + ground), `PointLight` (lighting only) | `ludion render --scene examples/gallery/ground-markers.yaml --out ground-markers.png` |
| `gallery/lights.yaml` | `PointLight` — color, intensity, range | `Transform`, `Camera`, `Mesh` (neutral grey, so only the lights vary the image), `PointLight` | `ludion render --scene examples/gallery/lights.yaml --out lights.png`, or `ludion serve --scene examples/gallery/lights.yaml` + the viewer (also the scene the `ludion-client` `animate` example drives) |
| `gallery/text.yaml` | `Text` — bitmap glyph quads | `Transform`, `Camera`, `Text` (unlit — no `PointLight` needed) | `ludion render --scene examples/gallery/text.yaml --out text.png` |
| `gallery/billboards.yaml` | `Sprite` with `billboard: true` — quads that always face the camera | `Transform`, `Camera`, `Sprite`, `Mesh` (ground plane, for 3D context), `PointLight` (lighting only) | `ludion render --scene examples/gallery/billboards.yaml --out billboards.png` |
| `gallery/sprite-atlas.yaml` | `Sprite.atlas`/`frame` — a quad sources its pixels from one named region of a packed `kind: atlas` document instead of a loose PNG | `Transform`, `Camera`, `Sprite` (atlas frames, some with `lit: true`), `PointLight` (lighting only) | `ludion render --scene examples/gallery/sprite-atlas.yaml --out sprite-atlas.png` |
| `gallery/sprite-flipbook.yaml` | `Animation` on a `Sprite` — naming an atlas `flipbooks:` entry under `clip` picks the region shown at `(clip, sim time)` | `Transform`, `Camera`, `Sprite` (unlit, `lit: false`), `Animation` (the feature) | `ludion render --scene examples/gallery/sprite-flipbook.yaml --frames 23 --out sprite-flipbook.png` |
| `gallery/sprite-tilemap-atlas.yaml` | `Tilemap` legend on an atlas — a legend entry names `atlas` + `frame` exactly as a `Sprite` does, so a whole stage is one packed image and one draw | `Transform`, `Camera`, `Sprite` (one foreground sprite from the same atlas), `Tilemap` (the feature, unlit) | `ludion render --scene examples/gallery/sprite-tilemap-atlas.yaml --out sprite-tilemap-atlas.png` |
| `gallery/tilemap.yaml` | `Tilemap` — a grid of single-glyph cells expanded into the sprite batcher, legend entries reusing `Sprite`'s color/texture fields | `Transform`, `Camera`, `Tilemap` (the feature), `PointLight` (lighting only) | `ludion render --scene examples/gallery/tilemap.yaml --out tilemap.png` |
| `gallery/particles.yaml` | `ParticleEmitter` — an alpha-blended smoke column, additive ember fountain, and floor-clipped spore drift, each an endless emitter shown in steady state | `Transform`, `Camera`, `Mesh` (ground), `ParticleEmitter` (the feature), `PointLight` (lighting only) | `ludion render --scene examples/gallery/particles.yaml --out particles.png` |
| `gallery/particles-stress.yaml` | `ParticleEmitter` — the fill-rate stress scene: ~24,000 live particles across twenty endless emitters (smoke, embers, spores), mixed alpha and additive | `Transform`, `Camera`, `Mesh` (ground), `ParticleEmitter` (the feature), `PointLight` (lighting only) | `ludion render --scene examples/gallery/particles-stress.yaml --out particles-stress.png` |
| `gallery/lights-stress.yaml` | `PointLight` — the clustered-forward stress workload: 500 point lights (200 static torches + 300 movers) over lit geometry, sized off vigil's own light counts | `Transform`, `Camera`, `Mesh` (buildings + plaza), `PointLight` (the feature, 500 instances) | `ludion render --scene examples/gallery/lights-stress.yaml --out lights-stress.png` |
| `gallery/gi-interior.yaml` | `GiVolume` — baked sky visibility in a sealed room with one doorway: no `DirectionalLight`, so the wedge of floor light at the door is the sky bake alone; an emissive torch adds a bounce pool | `Transform`, `Camera`, `Environment` (sky only), `GiVolume` (the feature), `Mesh`, `PointLight` (one `GiDynamic` mover light), `PostProcess` | `ludion render --scene examples/gallery/gi-interior.yaml --out gi-interior.png` |
| `gallery/gi-inject-stress.yaml` | `GiVolume` dynamic light injection — 300 `GiDynamic`-marked point lights at the `MAX_PROJECTILES` cap, two populations (spread bolts + a tighter cluster) | `Transform`, `Camera`, `GiVolume` (the feature), `Mesh`, `PointLight` (300 `GiDynamic` instances) | `ludion render --scene examples/gallery/gi-inject-stress.yaml --out gi-inject-stress.png` |
| `gallery/spot-lights.yaml` | `SpotLight` — a cone of light: a narrow beam, a wide soft cone, a hard edge (`inner_angle == outer_angle`) and a coloured spot angled at a wall, over a dim stage; the lit pool is the look (no shadows, no shaft) | `Transform`, `Camera`, `Mesh`, `SpotLight` (the feature) | `ludion render --scene examples/gallery/spot-lights.yaml --out spot-lights.png` |
| `gallery/sun-shadows.yaml` | `DirectionalLight` — a low, raking "sun" casting one orthographic PCF shadow map onto the mesh path, alongside one unshadowed `PointLight` to prove both light types contribute | `Transform`, `Camera`, `DirectionalLight` (the feature), `Mesh`, `PointLight` | `ludion render --scene examples/gallery/sun-shadows.yaml --out sun-shadows.png` |
| `gallery/night-street.yaml` | `Environment`/`DirectionalLight` — a built-in-registry stand-in for a vigil night district (street canyon, colonnade, moon shafts), used to tune the volumetric medium against vigil-shaped geometry | `Transform`, `Camera`, `DirectionalLight` (moon), `Environment`, `Mesh` (street geometry), `PointLight`, `PostProcess` | `ludion render --scene examples/gallery/night-street.yaml --out night-street.png` |
| `gallery/post-full.yaml` | `PostProcess` — the post-process arc's all-on scene: pyramid bloom over an emissive night colonnade, moon disc, voxel beacon and a real depth range, one scene the whole pass roster is enabled and budgeted on | `Transform`, `Camera`, `DirectionalLight` (moon), `Environment`, `Mesh`, `PointLight`, `PostProcess` (the feature, every pass enabled) | `ludion render --scene examples/gallery/post-full.yaml --out post-full.png` |
| `gallery/ui-hud.yaml` | Screen-space UI — `UiRect`/`UiText`, a HUD mock (HP bar, center panel, timer, caption) drawn over a minimal, visibly lit 3D world so the overlay's "draws after tonemap" claim is on screen | `Transform`, `Camera`, `Mesh` (minimal lit world), `PointLight` (lighting only), `UiRect`, `UiText` (the feature) | `ludion render --scene examples/gallery/ui-hud.yaml --out ui-hud.png` |
| `gallery/ui-wrap-window.yaml` | `UiText.wrap_width` + `UiList.max_visible` — a hard-wrapped paragraph (greedy fill, hard break on an overlong word) and a windowed, centered scrolling list, over the same lit-world convention as `ui-hud.yaml` | `Transform`, `Camera`, `Mesh` (minimal lit world), `PointLight` (lighting only), `UiList`, `UiText` (the feature) | `ludion render --scene examples/gallery/ui-wrap-window.yaml --out ui-wrap-window.png` |
| `gallery/ui-atlas-icons.yaml` | `UiRect.atlas` + `frame` — HUD icons drawn from regions of the sprites gallery's committed atlas (three life icons and a jewel, all one batch) beside a plain-color rect, over the same lit-world convention as `ui-hud.yaml` | `Transform`, `Camera`, `Mesh` (minimal lit world), `PointLight` (lighting only), `UiRect` (the feature), `UiText` | `ludion render --scene examples/gallery/ui-atlas-icons.yaml --out ui-atlas-icons.png` |
| `gallery/breakout-mock.yaml` | *Not single-feature* — the milestone's composed preview scene (Breakout-style bricks/paddle/ball) through an **orthographic** `Camera` projection | all of the above | `ludion sim-hash --scene examples/gallery/breakout-mock.yaml --frames 300` (determinism), `ludion render ...` (it is still golden-tested) |

Most gallery scenes carry a `PointLight` purely because `Sprite` and `Mesh` are lit surfaces (see
`crates/ludion-renderer/src/shaders/{sprite,mesh}.wgsl`) — at the renderer's default ambient
(`0.03`) an unlit lit-surface scene reads as near-black. That light is infrastructure, not the
feature under test, except in `lights.yaml` itself, where it *is* the feature. A scene needs none
when its surfaces are explicitly unlit: `text.yaml` (bitmap text), `sprite-flipbook.yaml` and
`sprite-tilemap-atlas.yaml` (both author `lit: false`).

`Camera` and `Transform` appear in every scene (every scene needs a viewpoint and a place to put
things) and are not separately called out per scene above. `Camera` projection: every scene but
`breakout-mock.yaml` uses `perspective`; `breakout-mock.yaml` is the one `orthographic` example.

Not covered by a dedicated scene: `Parent` (hierarchy) and the renderer's bloom/tonemap post stack
— bloom is a global post effect (not a component), already visible wherever a light overdrives a
surface past `1.0` (e.g. `lights.yaml`, `meshes.yaml`), so it does not need its own scene.

## `examples/scenes/`

| Scene | Purpose |
|---|---|
| `scenes/empty.yaml` | The minimal valid scene document (`format: 1`, `entities: []`) — a format fixture for `check-scene`/parser tests, not a feature demo. See `docs/scene-format.md`. |

## Other example directories

- **`examples/import/`** — a glTF import fixture (`scene.gltf`, its `expected/scene.yaml`, and
  per-OS `goldens/`), driving `crates/ludion-cli/tests/import.rs` and documented in
  `dev-docs/ludion-import.md`.
- **`examples/import-textured/`** — a hand-made two-cube glTF fixture exercising the importer's
  full five-map metallic-roughness path (`Plan/textured-pbr.md`), one cube with an authored
  `TANGENT` and one relying on the importer's computed-tangent path (`scene.gltf`, its
  `expected/scene.yaml`, and per-OS `goldens/`), driving `crates/ludion-cli/tests/import.rs`.
- **`examples/voxel-gen/`** — a worked voxel-generator example (`brazier.voxel.gen.mjs` emits the
  JSON `brazier.voxel.yaml` is checked against), documented in `docs/voxel-generation.md`.

## Three ways to look at a scene

- **`ludion render --scene <file> --out <png> [--width --height]`** — one deterministic frame to a
  PNG, no window, no server. Fastest way to eyeball a change.
- **`ludion serve --scene <file> [--paused]`** then attach the Tauri/browser viewer (or
  `ludion-client`) over the wire protocol — for watching a scene live, or for a peer client to drive
  it (see `crates/ludion-client/examples/animate.rs`, built for `lights.yaml`).
- **`ludion test --dir examples/gallery [--bless] [--filter <substr>]`** — render every gallery
  scene and compare against its committed golden (`--bless` (re)writes the golden instead).
  This is what CI/`cargo test -p ludion-testkit` exercises for the whole directory at once.

## Adding or changing a gallery scene

1. One scene, one feature. If you are tempted to add a second component beyond `Transform`/`Camera`
   and the minimum lighting needed to see a lit surface, that is a sign it belongs in a different
   scene (or the feature already has one — check the table above and the builtin component
   registry, `crates/ludion-engine/src/components.rs`, before adding a new file).
2. Validate: `cargo run -p ludion-cli -- check-scene examples/gallery/<name>.yaml`.
3. If you changed rendered content (not just comments/`name:`), re-bless *that scene only* and
   sanity-check the PNG before committing:
   `cargo run -p ludion-cli -- test --dir examples/gallery --bless --filter <name>`.
4. `cargo run -p ludion-cli -- test --dir examples/gallery` must be all green before committing.
5. Search the repo for the filename before renaming one — `ludion-server`, `ludion-engine`, and
   `ludion-cli` integration tests and `docs/*.md` reference specific gallery scenes by path.
