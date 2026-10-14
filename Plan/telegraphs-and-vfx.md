# Plan: Combat Telegraphs and the VFX Question

> Status: **Wave 1 COMPLETE (2026-08-19).** `GroundMarker` shipped end-to-end: component (registered
> 20th, append-only, semantic validator `L2106` — every shape dimension strictly positive and
> finite, per-field JSON Pointers), extract (`extract_ground_markers` — dimensions × uniform
> `Transform.scale.x` into final world units, rigid model matrix with `GROUND_MARKER_LIFT = 0.01`
> baked into Y, ascending-`EntityId` order; markers deliberately skip `place()` so `render.bounds`
> stays scene-content-only), renderer (`GroundMarkerInstance`, `ground_marker.wgsl` — one branch-free
> rounded-box SDF covering disc and capsule, `fwidth` AA, `QUAD_PAD 1.05`, unlit, alpha-blended,
> `depth_state(false, Less)`, two-sided), gallery example + Windows golden, `docs/components.md`
> entry, `dev-docs/renderer-passes.md` updated.
> **One accepted deviation from this plan's Wave-1 exit wording: there is no `ground_marker` graph
> pass.** Markers draw as one more pipeline *inside* the `scene` pass, between the opaque meshes/sky
> and the sprites — decision 1's own design sentence. A trailing graph pass cannot honor the decided
> draw order: sprites write no depth (`depth_state(false, LessEqual)`), so a later pass would paint
> markers over any sprite standing on them; and the graph derives edges from reads alone (no
> write-after-write ordering). Golden neutrality is therefore *stronger* than the overlay-pass
> precedent: the graph never changes at all (`graph.rs::ground_markers_leave_the_graph_untouched`,
> `dump_graph_snapshot.rs::a_ground_marker_scene_dumps_the_unchanged_minimal_graph`), and an empty
> `ground_markers` binds no pipeline and issues no draw (the sky-triangle inversion). Consequence:
> marker cost is attributed to `scene` in `--dump-timings`/`profile.capture`, like the sky. The GPU
> proof `render.rs::a_ground_marker_draws_bounded_and_occluded` pins SDF-bounded coverage,
> depth-tested-never-written occlusion (byte-identical under a covering slab), and bit-identical
> re-render. Serialization: `shape` is internally tagged (`type: disc` / `type: segment`) — the
> `Projection` precedent for struct-variant enums, not `MeshSource`'s external tagging.
> Decision 6's `[USER CALL]` values shipped as recommended (`GROUND_MARKER_LIFT 0.01`, `fwidth` AA,
> `QUAD_PAD 1.05`). `ground-markers.png` provenance re-blessed on the RTX 3090 2026-09-07 (the laptop
> bytes read SSIM 0.9998 there); macOS bless owed.
> **Wave 2's charge line LANDED in the reference game (vigil, 2026-08-20)**: a
> marker-tagged `GroundMarker` `Segment` per winding charger, spawned and reaped by
> a game system with no engine change — decisions 3 and 4 used exactly as designed
> (`Transform.scale` is the wind-up countdown made visible; the color is an authored HDR RGBA
> tunable). The markers are **reconciled**, not owned: they carry no back-reference to their enemy,
> so one cannot outlive the charger that owes it (a component holding an `EntityId` across a snapshot
> was the alternative). Decision 6's values were **not** confirmed by eye there either — the wave was
> verified headlessly, and a golden catches a telegraph only if a charger happens to be
> winding on the golden frame; the confirmation still rides a sitting. The **landing disc** for a
> ranged attack is unbuilt and stays owed to the game's private plan.
> Originally proposed 2026-08-16 from the reference game's design gap analysis (a private plan); that
> game's human-gate sitting is still queued. Engine-side sibling
> of `Plan/sector-state-persistence.md`, `Plan/ui-widgets.md`, `Plan/audio-ambience.md`. Consumed by
> the reference game's private plans (enemy telegraphs, spawner tiers).

## Why

The reference game's design states a transverse rule binding its whole enemy roster: every lethal or
time-stealing attack must be telegraphed, stated once for all archetypes rather than per-enemy. Two of
the prototype archetypes need it directly: a ranged enemy with an arcing/mortar attack and persistent
hazard zones (ground-anchored area markers, not projectile VFX), and a charger, usually with a charge
telegraph (a directional line indicator). The design separately needs its spawners to read at
gameplay distance (a looping idle effect), raising the sibling question of whether it needs a particle
system.
Today the reference game ships zero ground telegraphs — not even for its aura weapon, whose damage radius
has no visual indicator — and the engine
has no primitive for one. Without this plan, every ranged or charging variant reuses an ad-hoc per-enemy
hack or the game's readability rule goes unmet (an unreadable enemy is an unfair enemy).

## What exists today (verified)

**No decals, no particle system**, confirmed reading `crates/ludion-renderer/src/scene.rs` and
grepping the renderer:
- `MeshShape` (`scene.rs:41-51`) is `Cube | Plane | Sphere | Asset(usize)` — no circle/ring/sector
  primitive. `MeshInstance` (`scene.rs:54-81`) draws **opaque, depth-written** geometry
  (`prepass.wgsl:10`: "Opaque meshes only. Sprites, billboards and text are alpha-blended";
  `render.rs:1980`: "meshes (opaque, depth-written), then sprites (alpha, depth-tested), then text").
  `Mesh.color`'s alpha channel is not blended by the mesh pipeline.
- `SpriteInstance` (`scene.rs:83-102`) is the one alpha-blended primitive — textured or plain-white
  tinted quad, billboard or non-billboard (`Sprite.billboard`, `docs/components.md:35`), drawn after
  opaque meshes. This is the brief's "flat plane/non-billboard sprite" option: fine for a solid-color
  rectangle, but a soft-edged circle/ring/sector needs a raster texture per shape+size, or an atlas —
  and the atlas/sprite-sheet pipeline is an explicitly deferred feature
  (`Plan/phase-8-assets-and-sound.md`).
- **Z-fighting is real and unmitigated**: a flat quad coplanar with ground geometry has no
  engine-provided offset today — zero hits grepping for a shared lift/bias convention.
- **Particles (superseded 2026-09-04/05)**: this paragraph originally recorded "no particle
  system" and entity-based `spawn_fragments` gore with a recorded, unfired trigger
  (`Plan/phase-7-designer-flow.md:96`, plus a private plan). The trigger fired:
  `Plan/particles-engine-and-editor.md` shipped `ParticleEmitter` (closed-form, hash-neutral) in W1
  and W3 replaced the fragment gore with `DeathBurst`; decision 7 below records the settlement.
- **Voxel-material caveat** (`docs/materials.md:20-43`): material comes from exactly one of three
  surfaces — `Mesh` fields, a voxel palette entry's own keys, or `.mesh.yaml` per-vertex lists.
  **`Mesh.emissive` is inert on any mesh that authors its own material** (every voxel model, every
  `.mesh.yaml` asset) — it works only on bare `Cube`/`Plane`/`Sphere` primitives with no
  palette/mesh-doc backing. Matters for a voxel spawner model: its glow is set via the
  **palette entry**, not `Mesh.emissive` — the same trap the reference game
  already hit once (hit flash forced onto `Mesh.color`, emissive inert on voxel enemies).
- **Precedent for a conditional pass with zero golden churn**: `Plan/phase-14-ui.md` decision 5 made
  the UI `overlay` pass append only when `RenderScene.ui` is non-empty, so "every existing golden
  keeps the byte-identical 5-pass graph." `graph.rs:896` (`add_external_pass`) and the
  `ui_graph_appends_ldr_and_overlay` test (`graph.rs:1133`) are the implementation; `depth_state(write,
  compare)` (`render.rs:3340`) is the reusable helper every pipeline calls for its depth-stencil
  state — exactly what a new alpha-blended, depth-tested-not-written pipeline needs.
- Diagnostic codes: render domain is `L2xxx` (`codes.rs:7-16`); highest registered is `L2205`, so a
  new semantic validator here takes `L2106` (next free slot after `L2105`).

## Decisions

**1. Ground telegraphs get a small, conditional renderer feature — not a flat-mesh/sprite docs
recipe.** A solid-color `Sprite` quad is expressible today, but a game needs soft circular edges and
*variable* radius/angle per instance (a telegraph fills in as its warmup counts down) — a textured
circle/ring needs N baked PNGs or an atlas (deferred), and pushing the z-fight fix to every call site
replicates a single-right-answer problem per game, the "second way" `CLAUDE.md` warns against. The
winner: a `GroundMarker` component + `RenderScene.ground_markers: Vec<GroundMarkerInstance>` + a
dedicated `ground_marker.wgsl` pipeline computing an analytic SDF in the quad's local UV space (disc:
distance-from-center vs. radius, `fwidth`-based soft edge; segment: a capsule SDF for the charge
line) — no texture, no atlas, radius/length are per-instance floats so "filling in" is a value write,
not a texture swap. Reuses `depth_state(false, Less)` (read-only depth test, occluded by geometry,
occludes nothing) — no new frame-graph machinery, one more pipeline in the existing scene pass.
- **Golden cost**: the pass appends only when `RenderScene.ground_markers` is non-empty — verbatim
  the `overlay`-pass trick (decision 5, phase-14-ui). Every existing golden stays byte-identical.
- **Z-fighting, resolved once**: a fixed, non-authorable world-space Y lift (`GROUND_MARKER_LIFT`,
  e.g. `0.01` units) baked in at extract time — same "one obvious value" shape as `Fade.height_min`'s
  documented default. No scene author picks an offset.
- **Draw order**: meshes (opaque) → **ground_marker** (alpha, depth-tested, not written) → sprites →
  text. Telegraphs sit on the ground and under characters, never occluding a sprite standing on them.
- **Second-consumer story**: any top-down/third-person action game with ranged or AoE combat needs
  ground indicators — melee ARPGs, tower-defense/RTS range rings, any twin-stick beyond the reference game.

**2. First-wave shape set: Disc and Segment only — Ring and Sector/cone deferred.** Of the reference
game's prototype archetypes, only the charger (charge → **Segment**,
a capsule/rectangle along the charge path) and the ranged enemy (landing point + persistent zone → **Disc**)
attack at range. Ring (an annular boundary, e.g. a future time-bubble edge) and Sector/cone (a true
angular wedge, vs. the design's landing-*point* circle) have no prototype consumer.
`GroundMarkerShape` is an enum, so a later variant touches only the shader's SDF branch, not the pass
wiring or extract integration. **Trigger:** a concrete game enemy variant that needs
one.

**3. `GroundMarker` carries static shape parameters only; animation is game-side via
`Transform.scale`.** Mirrors `Sprite`'s convention (`docs/components.md:35`: drawn size × entity
scale) — authored `radius`/`length` is the *maximum* extent, `Transform.scale.x` (uniform) scales it
down, so a warmup fill is a game system decrementing a countdown and writing scale each frame — the
"countdown f32 decremented by `ctx.dt`" idiom already the reference game's only timer convention (no
engine timer/scheduler exists). The component stays a plain, deterministic data bag (contributes to
`WorldHash` like any other), zero animation logic in `ludion-renderer`/`ludion-engine`.

**4. Color/urgency is an authored RGBA the game writes per frame, not an engine ramp.** `color: [f32;
4]`, HDR-capable so a bright telegraph rides the existing bloom pipeline exactly like
`Mesh.emissive` (`docs/materials.md:83-99`). A game ramping alpha/hue as impact nears is a per-frame
write to an existing field — same shape as the shipped hit-flash pattern (`Mesh.color` written each
frame by a game system).

**5. `Mesh.emissive`/palette caveat is a docs note here, not a design change.** `GroundMarker`
instances are always the new analytic primitive, never voxel/mesh-doc-backed, so decision 1's
inertness trap doesn't apply to markers themselves — only when a spawner/enemy *model* wants a glow
(decision 7). Recorded so game consumers don't
rediscover it.

**6. `[USER CALL]` — SDF anti-alias width and the lift constant's exact value are a taste call.**
Recommendation: `fwidth()`-based analytic AA (matches existing shader style) and
`GROUND_MARKER_LIFT = 0.01` world units as a starting point, confirmed at the first wave that renders
a telegraph in anger (likely the reference game's ranged/charger wave), not a dedicated sitting.

**7. Spawner visuals — SETTLED (2026-09-04): the particle trigger fired and shipped.** The
register-by-register design below stands as authored; what fired was clause (b) of the sharpened
trigger — a continuously-emitting, high-count effect that would blow the entity and `WorldHash`
budget if built as individual ECS entities — and that constraint is what chose the shipped design.
`Plan/particles-engine-and-editor.md` W1 (2026-09-04) added `ParticleEmitter` as the registry's 24th
entry with its particles a pure closed-form function of `(emitter, sim time)` evaluated at extract:
never entities, never a pool, so a plume costs one component and no hash surface. W3 (2026-09-05)
landed the consumers — the reference game's entity-based death gore is deleted in favour of a `DeathBurst` ember
burst, and a game system stands one plume beside every spawner.
- **Standard spawner — looping idle effect readable at gameplay distance**: a pulse is a
  `Transform.scale` sinusoid on a voxel model driven by `SimTime`/`FrameIndex` (never wall clock) —
  same idiom as decision 3. Translucent accents map onto `Sprite` (already alpha-blended), not `Mesh`
  (opaque, confirmed above) — a small alpha-tinted quad, scaled up over its early window.
  No emitter needed for a handful of discrete accents per spawner.
- **Architectural top-tier spawner**: a *place* — kit-level
  dressing under the existing voxel-authoring loop, not a rendering feature.
- **Portal/tear tier**: this plan's `ground_marker` pass is
  directly reusable (a large pulsing disc/ring at the base, HDR feeding bloom) plus an
  emissive prop for the vertical tear. Explicitly launch-scale,
  well past the prototype.

## Waves

### Wave 1 — `GroundMarker` component, conditional pass, Disc + Segment
- **Engine**: `GroundMarker` component (`shape: GroundMarkerShape { Disc { radius }, Segment {
  length, width } }`, `color: [f32; 4]`, registered in `components.rs` alongside `Sprite`/`Fade`).
  Semantic validation: dimensions must be `> 0` (`L2106`, new). `extract.rs` resolves each marker +
  `Transform` into a `GroundMarkerInstance` (shape, model matrix with `GROUND_MARKER_LIFT` baked in,
  color) appended to `RenderScene.ground_markers`.
- **Renderer**: `GroundMarkerInstance` in `scene.rs`; `ground_marker.wgsl` (instanced quad, SDF
  disc/capsule, `fwidth` soft edge, alpha blend, `depth_state(false, Less)`); pass appended iff
  `RenderScene.ground_markers` is non-empty. One gallery example
  (`examples/gallery/ground-markers.yaml`: a disc and a segment on a flat floor), blessed golden on
  both pinned GPUs.
- **Exit criteria (mechanized)**: extract-determinism test extended to `ground_markers`;
  `--dump-graph` on a marker-free scene shows the unchanged pass list, on the new gallery scene shows
  `ground_marker` between `mesh`/`shadow` and `sprite`; new golden blessed at SSIM 1.0; every
  pre-existing golden byte-identical; `node scripts/check.mjs` green.
- **Human gate**: none to close the wave (docs-example golden gets eyes at bless time); the real look
  gets eyes once the reference game wires a telegraph in anger.

### Wave 2 — game integration (landing disc, charge line)
Lands in the consuming game. A `GroundMarker` entity spawned on
attack windup, `Transform.scale` driven by the existing countdown idiom, despawned on
impact/expiry — no engine change. Exit: behavioral test per attack (telegraph appears N frames before
impact, despawns after) + `WorldHash` determinism preserved. Scoped by
the game's private plan, not scheduled here. **Charge line delivered 2026-08-20**
(see the Status header); the landing disc is still owed.

### Deferred wave — Ring / Sector shapes
Triggered per decision 2. Adds one `GroundMarkerShape` variant and one SDF branch; no
pass/graph/extract-plumbing change.

## Engine/game boundary

- **`crates/ludion-engine`**: `GroundMarker` component, its semantic validator (`L2106`), extract
  integration.
- **`crates/ludion-renderer`**: `GroundMarkerInstance`, `ground_marker.wgsl`, the conditional pass,
  `GROUND_MARKER_LIFT`.
- **The game**: every telegraph's timing, color ramp, despawn logic; the spawner visuals'
  `Transform.scale` pulsing and voxel-model authoring.
- **Generic second consumer**: any genre with ranged/AoE combat or radius-based abilities (decision
  1) — why the shape lives in `ludion-renderer`, not in a game.

## Deferred / non-goals

- **Ring and Sector/cone shapes** — trigger: a concrete enemy variant that needs one (decision 2).
- **A particle/GPU-emitter system** — trigger unchanged from the two recorded conditions
  (`Plan/phase-7-designer-flow.md:96`, plus a private plan), sharpened in decision 7: a human gate's verdict, or a measured entity/`WorldHash` budget breach from a high-count effect. Neither
  fired.
- **Sprite atlas / sprite-sheet pipeline** — unrelated (the SDF approach sidesteps it); own trigger
  (`Plan/phase-8-assets-and-sound.md`) untouched.
- **A general decal system** (arbitrary projected textures on arbitrary geometry) — the reference game's needs
  are all ground-anchored; no named consumer for the general case. Trigger: a concrete need for a
  telegraph/effect on non-flat or vertical geometry (e.g. a wall-mounted sigil).

## Risks / open questions

- **`[USER CALL]`** (decision 6): SDF anti-alias width and `GROUND_MARKER_LIFT`'s value —
  recommendation given, confirmed at first real use.
- The Segment/capsule SDF's exact geometry (rounded capsule vs. plain rectangle) is a Wave-1 shader
  detail; either reads fine for a charge line and doesn't affect the component's public shape.
- If the ranged enemy turns out to need an actual traveling wedge/cone (contra this plan's
  reading of the design as a landing-point circle), Ring/Sector's trigger fires immediately at that
  wave — flagged so the game's author checks the design against the final attack
  before assuming Disc suffices.
- Audio telegraphing (a sound cue alongside the visual) is out of scope here —
  `Plan/audio-ambience.md`'s territory if the game's readability rule extends to attack
  telegraphs specifically.
