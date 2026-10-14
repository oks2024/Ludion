# Plan: Sprite engine — generic 2D sprite layer + LLM authoring tools

> Design settled 2026-08-20 (grilling session, every question ruled by the user). This doc is the
> decision record and the schedule of record for the arc; the shipping log is the Status section.

## Status

**Wave 1 (foundation) MECHANIZED-COMPLETE 2026-09-06.** Supersedes `Plan/phase-8-assets-and-sound.md`
decision 6 / Wave 5 (the `kind: atlas` deferral — its trigger fired: `Tilemap` shipped and a second
game wants per-region sprites). Shipped: the `ludion-sprite` crate (`bake` with all seven effects,
seeded and bit-reproducible; the shelf `pack`; `import_png`); `kind: sprite` / `atlas-source` /
`atlas` in `ludion-scene` with `L1151`–`L1154`; `ludion generate-atlas` + `--check` and the
`atlas-drift` gate (check.mjs, ci.yml, TeamCity); `Sprite.atlas`/`frame`, `size` now `Option`
(derived from the region at `pixels_per_unit`, an authored value the override), `lit` (default
`false`), per-instance UV + `FLAG_LIT` so lit and unlit quads of one atlas share one instanced draw;
the premultiplied loader at both engine seams (an atlas is baked premultiplied and never converted
again); the `Tilemap` legend on an atlas (`TileAppearance` = `texture` | `atlas` + `frame`, `color`,
`lit`); `check-scene` over the three documents and over every scene reference; the W1 tools —
`render --sprite` (contact sheet, `--scale`, `--set`/`--sweep`, the effects echo), `render --atlas`,
`import <png>` (`--quantize`, `--name`, `L4014`/`L4015`); `examples/gallery/sprites/` (ship, bullet,
explosion, tiles packed into `gallery.atlas.yaml`) with the `sprite-atlas` and `sprite-tilemap-atlas`
scenes; docs `sprite-authoring`, `sprite-sheets`; dev-docs `ludion-sprite`, `sprite-pipeline`. Every
pre-existing sprite/tilemap scene (gallery, breakout, hello-paddle, sokoban, shmup-stress, the `new`
template) authors `lit: true`, look preserved. `Sprite`'s hashed bytes moved, so breakout's `run.lrp`
was re-recorded; vigil places no `Sprite` and its replays are untouched.

W1 rulings, beyond the design list:
- `filter:` is carried, not consumed — the sprite pass binds one nearest sampler for every texture.
  A per-atlas sampler rides W3 beside `pixel_snap`, where sampling gets its golden.
- A region's `pivot` is carried through the bake and marked on both sheets; extract reads it in W3
  (decision 5's in-plane transform).
- `flipbooks:` are parsed, cross-checked against the regions and written onto the atlas; W2 plays
  them.
- The gallery goldens follow the particles precedent: the scene ships, the first bless is the
  pinned GPUs'.

M4 blesses done (`sprite-atlas`, `sprite-tilemap-atlas`, the `billboards`/`ui-hud`/`ui-hud-1080` re-bless); the gallery lane is green on both pinned boxes.

**Wave 2 (flipbook) MECHANIZED-COMPLETE 2026-09-06.** Shipped: `ludion_scene::flipbook_frame` (+
`Flipbook::total_holds`/`duration`, `AtlasDocument::flipbook`) — the pure evaluator beside
`clip_phase`: clip frame `floor(elapsed × fps)`, looping wraps, a one-shot clamps and holds its last
frame, `hold` walked as cumulative clip-frame counts, degenerate input the first frame;
`extract.rs::flipbook_frame_name` beside `animation_pose` — `place()` takes `now`, and a `Sprite`
with `atlas` + `frame` whose `Animation.clip` names one of that atlas's flipbooks shows the
flipbook's region for both the derived size and the UV rect (one name, resolved once); every way of
not animating (no `Animation`, empty or unbound clip, a `texture:` sprite, an atlas that will not
load) is the authored `frame`, silently — the assembly contract. Proof
`crates/ludion-engine/tests/sprite_flipbook.rs` (hash- and replay-neutral, time moves the UV and the
quad, one-shot holds, holds stretch, unbound degrades). Tools: `render --atlas --clip <flipbook>`
draws the entry at `--time` (`<stem>.atlas-frame.png`), `--strip N` (2..=16, the model strip's
phase spacing shared through `model_sheet::phase_times`; `.atlas-strip.png`) and `--onion` (every
entry up to the one showing, pivot-aligned, earlier ones ghosted at 35 %; `.atlas-onion.png`),
`--scale` as under `--sprite`; refusals are `L4008` (orphan flags, strip + onion, N out of range,
an unknown clip listing the carried names). Goldens: `GoldenTest.frames` / the manifest's `frames`
key tick a scene before extracting (`ludion render --frames` for suites), so
`examples/gallery/sprite-flipbook.yaml` is compared at tick 23 (a looping ship mid-bank, four
one-shot explosions staggered through the clip and its hold). Docs `sprite-animation` (+ pointers
from `sprite-sheets`, `sprite-authoring`, `voxel-animation`, `components`, `visuals`), dev-docs
`sprite-pipeline`, `ludion-testkit`, `ludion-cli`. `scripts/map-drift.mjs` now skips
`*.atlas.yaml` (it tried to regenerate the gallery atlas as a map — the W1 check.mjs lane never ran).
Zero wire/schema/hash/replay movement: `Animation` and `Sprite` are unchanged as data.

W2 rulings:
- **No `kind: sprite-animation` document.** The Data-and-bake entry below is superseded: a flipbook is
  the `flipbooks:` entry the atlas carries (W1's ruling), and `Animation.clip` names it directly. One
  document owns both the regions and the clips over them, so a clip can never name a frame its atlas
  lacks.
- The strip's phase spacing is the model strip's, by sharing the function, so the two instruments
  cannot drift; the atlas strip allows 16 phases where the model strip allows 8 because a cell is a
  region, not a turntable.

`sprite-flipbook.png` blessed on both pinned boxes at frame 23.

**Wave 3 (order, blend, transform, camera) MECHANIZED-COMPLETE 2026-09-12.** Shipped (decisions 3–7
as written): `Sprite.layer` (i16, a pure sort key — the sprite sort is `(layer, view depth
back-to-front, order)` and batches split on `(layer, texture)`); `Sprite.blend: alpha | additive`
(a per-instance flag under the one premultiplied `One, OneMinusSrcAlpha` pipeline, so blend never
splits a batch); `flip_x`/`flip_y` applied to the UV rect in extract, the `Tilemap` legend gaining
the same and `Tilemap` a `layer`; non-billboard quads rotating about the atlas region's pivot; `flash`
+ `flash_color` on `Sprite` and `Mesh` (a new instance lane mirroring the occlusion-fade lane,
default-identical; vigil's combat migrated off its `Mesh.color` hack — the proof the flash is generic);
`Orthographic.pixel_snap` (extract-only integer texel snapping of the viewport and sprite centres,
reading the atlas `pixels_per_unit`; the sim never sees it, the world hash is unchanged); the
per-atlas `filter` sampler finally selected per batch. Proof `crates/ludion-engine/tests/sprite_w3.rs`
(drift-safe, hash-neutral). Goldens `sprite-layer-order`, `sprite-additive` (under bloom),
`sprite-pixel-snap` (snap vs shimmer), first-blessed on the RTX 3090. Wire schema hash unchanged.

The external shmup's request queue (one file per request in the game's repo, moved to `Done/` when
shipped) drove the wave's tail: `ENGINE_KINDS` gained every engine document kind (a game registering
its own data-doc kind failed `L1119` on its own sprite, atlas, assembly, part and animation files);
the atlas-clip strip samples the centre of each phase span, so `--strip N` at the clip's frame count
no longer drops the last frame; `glow` is the eighth effect op (a bright core with a fractional-radius
halo on a parabolic falloff over squared integer distance, never opaque, `L1154` out of range);
**`kind: palette`** and the sprite's `palette: { from, overrides }` include (resolved at bake through
one engine-side resolver shared by `check-scene`, `generate-atlas` and `render --sprite`, so an
including sprite bakes byte-identically to the inlined form — `examples/gallery/sprites/emblem.*`;
`L1147` a malformed or unreadable palette, reported at `/palette/from`); **`UiRect.atlas` + `frame`**
(the HUD analog of `Sprite.atlas`, `L1151`/`L1152` shared, one texture entry with the world sprites,
the region UV on a new UI instance lane — golden `ui-atlas-icons`).

W3 rulings:
- `pixel_snap` is extract-side only; nothing about it reaches the sim or the hash.
- A shipped-wave issue the external game files is fixed inside the arc, never deferred to a later
  wave: the queue is the arc's acceptance test.

`sprite-layer-order`, `sprite-additive`, `sprite-pixel-snap`, `ui-atlas-icons` blessed on the M4 2026-09-15
(cross-GPU SSIM 0.9970–0.9994 against the 3090 set, above the 0.99 tier).

**Wave 4 (measured scale) MEASURED 2026-09-12 on the RTX 3090.** The instrument:
`games/shmup-stress/scenes/steady-drawn.yaml` — `steady`'s census (2,975 live) drawn from 40 regions
of one atlas (`scenes/sprites/shmup.atlas.yaml`, 44 regions, two `.gen.mjs`-authored sprite docs, the
pair byte-exact under `atlas-drift`), 357 of them additive by a closed form, over a 480-cell
`Tilemap` on the same atlas that a frame-indexed system scrolls, at 1080p, unlit, no post, no light.
`BulletArt` and `Backdrop` singletons select the draw from the scene; `Arena.plane: upright` puts the
field on XY because a tilemap's cells only lie there (the ground path's codegen is unchanged). The
three older scenes' bullets were edge-on to their −90° camera (identity rotation, no billboard) and
drew nothing; every bullet is `billboard: true` and unlit now, so their `scene` line is finally a
drawn field (their `WorldHash` moved; the crate holds no replay or hash fixture).

Verdict, medians of four 200-frame release runs, 8 sim threads, 1920×1080: sim 0.13 ms, `extract`
1.69 ms (0.85 for the flat field), GPU `scene` 0.057 ms, the whole frame ≈1.9 ms against the
16.6 ms line — 8.5× headroom, 93 % of it `extract`, 0.49 µs per drawn sprite (0.29 flat; the delta
is the region lookup by name plus the UV rect). Budget `budgets/pinned-rtx3090-sprite-stress.toml`
(`frame_total_ms` 16.6 as a fixed deadline, `extract_ms` 3.4, `[passes] scene` 0.12), checked
in-process the way vigil's E6 gate is and wired into the private pinned-GPU CI; its M4 twin measured
2026-09-15 (never derived). `SpriteBlend` joined `ludion::prelude`.

W4 rulings:
- **Neither "only then" item is a performance need.** A once-per-frame instance repack attacks an
  extract that already fits nine times over, and `uv_scroll` would trade ~480 tilemap instances
  (~0.2 ms) for one quad. The repack is not built. `uv_scroll`/`repeat` stays the external game's
  request 11 — a feature on its own merits (an infinite parallax strip without two leapfrogging
  copies), built on the game's word, not this wave's.
- If `extract` is ever attacked, the lever is the per-sprite region lookup by string name, not the
  buffer.

`budgets/pinned-apple-m4-sprite-stress.toml` measured 2026-09-15 (extract 0.787 median, 1.6 ceiling). Next: close (the git-dep
switch in the external game, the human verdicts, the proof note).

**`uv_scroll` shipped on the game's word (2026-09-12).** `Sprite` carries `uv_repeat` (default
`[1, 1]`) and `uv_scroll` (default `[0, 0]`, UV units per second): either off its default sets a
per-instance `FLAG_WRAP` and the sprite fragment stage wraps the frame with `fract` instead of
clamping, so an endless parallax strip is one quad. The offset is `uv_scroll × SimTime`, reduced
`rem_euclid(1.0)` at extract, so the world hash and the sim are untouched (adding the two fields did
move the pinned `WorldHash` of a sprite-carrying world, the only committed value that had to
follow). The clamping path is byte-identical — every existing sprite golden re-passed at exactly
SSIM 1.0 — and `examples/gallery/sprite-uv-scroll.yaml` (frame 30) is the new one. Known limit,
stated in the field doc: a wrapped region edge blends toward its atlas neighbour under a linear
filter.

## Driver and scope

- **Driver: an external cave-style shmup**, written *outside this repo* by the user as a real user
  would, started on day one alongside W1 (a moving ship with a baked sprite on a scrolling tilemap
  is already a W1 consumer and catches format errors while they are cheap). It depends on the
  checkout via a Cargo `path` dep during the arc and **switches to a git dep before the arc
  closes**, so the publish/release friction is felt too.
- **In-repo consumers** (the engine never ships a feature nothing exercises): one
  `examples/gallery/sprite-*.yaml` golden per feature, `games/shmup-stress` as the budget gate, one
  runnable example per public API. **No new in-repo game.**
- **Generic, every art direction.** Pixel-art is authored and goldened first; nothing in the
  renderer assumes it (sampling is a per-atlas property, the integer snap is a camera option).
- **Exit criterion.** Mechanized: all waves' gallery goldens + `shmup-stress` budget green + docs.
  Human: the external game's first stage playable on a **git** dependency, and the user's verdict on
  screen (crispness under `pixel_snap`, additive glow under bloom, hit-flash). The game side writes a
  proof note (what was missing, where tokens went) — the data the next tooling round needs, the
  way `phase-10-voxel-proof.md` was for voxels.

## Rendering model (decisions)

1. **World sprites live in the existing 3D world** under the `Orthographic` camera; the HUD is the
   existing screen-space UI. No second 2D pipeline — one renderer, one extract, one golden harness.
2. **`Sprite.lit: bool`, default `false`.** Today's clustered-forward two-sided path
   (`sprite.wgsl`) becomes the lit branch; unlit is color × tint, no cluster lookup (what 3k
   bullets need). The atlas doc reserves `maps: { color: <png> }` so `emissive:`/`normal:` are
   later additive fields; **no texture maps this arc** — the textured-PBR trigger
   (`Plan/phase-11-pbr.md`) is the external game's to fire, not this plan's.
3. **`Sprite.layer: i16` is a pure sort key**: `(layer, view depth back-to-front, EntityId)`. No Z
   displacement (it would leak into picking, shadows, debug draws), depth test vs meshes kept, no
   depth write (today's behaviour). Batches split on `(layer run, texture)`; with one atlas per
   layer group that is a handful of draws.
4. **Premultiplied alpha, one pipeline.** Atlases bake premultiplied; the loader premultiplies any
   straight-alpha PNG so authors never meet the convention. `Sprite.blend: alpha | additive` is a
   per-instance flag (additive = output alpha forced to 0 under `One, OneMinusSrcAlpha`), so blend
   never splits a batch. Tint multiplies the premultiplied texel.
5. **In-plane transform.** `Transform.rotation` already rotates non-billboard quads
   (`extract.rs` placement); add `flip_x`/`flip_y` (two bools) and the per-region **pivot in
   pixels** from the atlas (default region center) so rotation is about the visual center.
6. **Hit-flash on both `Sprite` and `Mesh`**: `flash: f32` + `flash_color: [f32; 3]` (default
   white), lerp toward the color. Vigil's `combat.rs` `mesh.color` hack migrates onto it — the
   proof it is generic.
7. **Pixel-perfect camera.** `Orthographic { height, pixel_snap }` snaps the viewport to an
   integer texel scale and sprite centers to texels, reading the atlas `pixels_per_unit`. A
   gallery golden shows the shimmer without it.
8. **Units.** `pixels_per_unit` lives on the atlas doc (art is authored at a density, as voxels
   declare `scale:`); `Sprite.size` becomes the derived `region_px / pixels_per_unit` with
   `size:` kept as an explicit override for untextured quads and deliberate stretches.
9. **`Tilemap` joins the atlas**: the legend gains `atlas`/`frame`, validated by the same codes
   — a stage is one atlas, and the scrolling-background answer before `uv_scroll` is measured.
10. **Flipbooks reuse the `Animation` component.** `Animation { clip, started, speed }` resolves
    `clip` against whatever the entity renders (assembly clips for `Mesh`, atlas flipbooks for
    `Sprite`); one game-side idiom (`games/vigil/src/animation.rs` ports unchanged). Visual-only
    by construction, evaluated at `extract_at` from `(clip, now)` — hash-neutral, proven the same
    way as assemblies (`assembly_animation.rs`).

## Data and bake (decisions)

- **`kind: sprite`** — the LLM authoring form, mirroring the voxel doc: single-char glyphs,
  `palette: { glyph: { color: [r,g,b,a], emissive? } }`, `.` transparent, **named frames of one
  size per doc** (a differently sized pose is another doc), optional per-frame `pivot` (px),
  and a doc-level ordered **`effects:`** list. Emissive stays in the palette so an emissive map
  can bake from the same source later.
- **Effects are baked on the CPU**, deterministic, doc-level `seed` for noise. A fixed enumerated
  op set — never expressions, never a node graph (the no-DSL pillar). **All seven ship in W1**
  (ruling: an instance under pressure routes around a missing op rather than asking for it):
  `gradient` (glyph set → linear/radial ramp between two colors, texel coordinates), `outline`
  (1-px color around the silhouette, inside/outside), `shade` (light direction, darken/lighten by
  silhouette-derived height), `shadow` (offset copy, color/alpha), `dither` (ordered two-tone
  between glyph tones), `noise` (seeded color jitter), `blur` (box radius — glows). Each op
  targets `glyphs: [..]` or the whole frame, optional `frames:` filter (default all); list order
  is application order. Guard rails: the sprite-authoring doc's gotchas say a gradient, glow,
  shadow or shading painted in glyphs is a defect (use `effects:`; a missing op is a notebook
  TODO, not a workaround), and the contact sheet echoes the effect list it applied.
- **`kind: atlas-source`** lists inputs (sprite docs + loose PNGs; region name = doc name + frame,
  or PNG stem) and declares the flipbooks it carries. **`ludion generate-atlas`** (new verb —
  `import` is foreign-format-in; this is our text → our binary) writes the committed
  `<name>.png` + `<name>.atlas.yaml`. Offline and committed, never runtime-packed.
- **`kind: atlas`** — derived, canonical: `generated_from:`, `pixels_per_unit`, `maps: { color }`,
  `filter: nearest | linear` (nearest default), `regions:` as a `BTreeMap<name, { rect:
  [x,y,w,h] px, pivot: [px,py] }>` — explicit rects only (grid shorthands live in the bake tool,
  not the committed form). Packer: deterministic shelf, sorted (height desc, name), 1-px
  extrude, no power-of-two requirement. `generate-atlas --check` byte-compares like
  `generate-map --check` and joins check.mjs + ci.yml as the **`atlas-drift`** gate.
- ~~**`kind: sprite-animation`**~~ (superseded by the W2 ruling above — flipbooks live on the atlas): `frames: [names]`, `fps`, `loop: bool`, optional per-frame
  `hold: n` (integer frame counts — tick quantization exact by construction). One-shot holds the
  last frame, like `attack`.
- **`Sprite` authored form**: `atlas: Option<String>` + `frame: String`; `uv` is derived at
  extract and never authored. `texture:` stays for single images.
- **Crate: new `ludion-sprite`** (sprite doc → `Image`, effects, packer), depended on by
  `ludion-assets`' loaders and the CLI; doc parsing for all three kinds in `ludion-scene` like
  every `kind:`. Keeps `ludion-scene`'s `cargo check` fast and gives the effects code one owner.
  `crate-graph` dev-doc gets one line.
- **Codes**: `L1151`–`L1154` (renumbered 2026-09-06 — `L1144`–`L1146` were taken 2026-09-05 by the
  data-doc validation seam, a private plan, and `Plan/projectile-patterns.md` reserves
  `L1148`–`L1150`; `L1147` is free): unknown atlas, unknown frame (with did-you-mean), sprite-doc
  structure, effects; `L4014+` for the CLI verbs. (`L1136` stays reserved by
  `Plan/audio-ambience.md`.)
- **Renderer**: per-instance UV rect + flags in `SpriteInstanceRaw`; batching by atlas texture
  within a layer run (3,000 bullets of 40 kinds = one instanced draw).

## Tools (launch requirement, grown in rounds)

The voxel arc's lesson: authoring tooling ships *with* the format, and converges by authoring
rounds, not up front. `--set`/`--sweep` pointers cover the palette and `/effects/<i>/<param>`.

- **W1**: `render --sprite <doc>` contact sheet (every frame labelled, at 1× and an authored game
  scale, texel ruler, applied-effects echo); `--set`/`--sweep` over palette + effects;
  **PNG → sprite-doc import** (lossless when ≤ the single-glyph budget, else `--quantize N`
  deterministic median-cut with the loss reported — the docs are meant to be edited); atlas
  overview sheet (regions outlined + named).
- **W2**: `--strip` over a flipbook, `--onion` (consecutive frames overlaid).
- **On demand** (only when an authoring session asks): `--mirror-x`, silhouette/contrast report at
  target scale.

## Waves

| wave | content |
| --- | --- |
| **1 — foundation** | `ludion-sprite` crate; `kind: sprite` (+ all seven effects), `kind: atlas-source`, `kind: atlas`; `generate-atlas` + `--check` + `atlas-drift` gate; `Sprite.atlas/frame`, `lit`, per-instance UV, batching by atlas; premultiplied loader; `Tilemap` atlas legend; `L1151`–`L1154`; `check-scene` frame validation; W1 tools; gallery goldens (atlas sheet, tilemap-on-atlas); docs `sprite-authoring`, `sprite-sheets`. |
| **2 — flipbook** | `kind: sprite-animation`; `Animation` resolves against `Sprite`; evaluator at `extract_at`; hash-neutrality proof test; `--strip`/`--onion`; golden + example; docs `sprite-animation`. |
| **3 — order, blend, transform, camera** | `layer`, `blend`, `flip_x/flip_y`, pivot-aware rotation, `flash` on `Sprite` + `Mesh` (vigil migrated), `pixel_snap`; goldens (layer order, additive under bloom, snap vs shimmer); docs `sprites` hub. |
| **4 — measured scale** | Against `shmup-stress`: **3,000 sprites across ≥ 40 frames of one atlas + 200 additive + a scrolling tilemap at 1080p, frame ≤ 16.6 ms on the reference box**, `extract` + sprite pass budgeted in `budgets/*.toml`. Only then: instance buffer packed once per frame, `uv_scroll`/repeat on the background quad. |
| **close** | git-dep switch in the external game, human verdicts, proof note, `Current state` refresh. |

**Particles** are *not* a wave: a separate deferred plan with trigger "entity-spawned fragments
blow the budget at the stress scene". **Voxel → sprite baking** was ruled out (if voxels are
cheaper to author, the sprite tools are what is lacking).

## Docs owed

`docs/sprites.md` (hub), `sprite-authoring.md`, `sprite-sheets.md`, `sprite-animation.md`;
`dev-docs/sprite-pipeline.md`; one-line updates to `crate-graph`, `engine-image-paths`,
`tilemaps`. Written per wave by the `ludion-docs` skill.
