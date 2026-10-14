# Engine gap map — Ludion against a complete modern 3D engine

> Status: **REFERENCE** — a survey taken 2026-09-30, not a build plan. It records what a
> Unity-HDRP / Godot 4 / UE5 user would find missing, which gaps are rejected by design, and the
> recommended order of attack. Nothing here is scheduled by being listed: each gap still earns its
> build through a consuming game, a request, or its own plan's trigger. Re-survey when a milestone
> closes; strike a line when its gap ships (with the plan that shipped it).

## Framing

The pillars' anti-goal is explicit: Ludion is "not a general-purpose human-first engine competing
with Bevy/Godot on features". This map is therefore a gap *map*, not a backlog. Its value is
knowing, before a game asks, which holes are foundational (cost grows with every game built on
top), which are content-triggered (build when a slice needs them), and which are closed by design.

Where Ludion already leads every mainstream engine, and must keep leading:

- **Determinism** — bit-exact sim, `WorldHash` identical across x86 and ARM and at 1/2/8 threads,
  `.lrp` replays with keyframe verification, divergence bisection (`dev-docs/determinism.md`).
- **Verification** — SSIM goldens with structured diffs, `--sweep` contact sheets, `image-stats`,
  per-OS profile budgets (`dev-docs/budget-files.md`).
- **Agent surface** — 30 typed wire methods, the MCP adapter, `L####`-coded diagnostics with
  `ludion explain`, embedded docs.

## The gaps, by area

Legend: **absent** — no code, no plan; **deferred** — a plan records a trigger; **designed** — a
settled plan, nothing built; **rejected** — closed by design.

### Runtime foundations

- **Transform hierarchy** — absent. `Parent` is a link only: no propagated world transform, no child
  list, no cascade despawn (`docs/components.md`, `dev-docs/ludion-ecs.md`). Every 3D game walks it
  by hand. Open design question: sim-side (hashed) world transforms or extract-only.
- **Runtime prefabs** — absent. No way to instantiate an authored template into a live world;
  `ludion_scene::instantiate` builds a whole `World`. `instantiate_into` exists only as a design in
  `Plan/world-streaming.md` W2.
- **Culling and LOD** — deferred (`Plan/world-streaming.md`, W0/W1 need no trigger). Extract walks
  every entity; no frustum or distance culling, no visibility flag, no voxel LOD; the asset cache
  never evicts.
- **Async loading and streaming** — deferred (`Plan/world-streaming.md` W2+). Loading is synchronous;
  one district at a time (`docs/scene-transitions.md`).
- **Trigger volumes** — absent by choice: "a trigger volume stays an `overlap` query" (`Plan/physics.md`).
- **Change detection, relations, parallel scheduler** — rejected (ECS freeze list,
  `engine-architecture.md` §4).

### Rendering

- **Lights and shadows** — one active `DirectionalLight`; one hand-placed 2048² orthographic shadow
  map, no cascades, no automatic fit. `SpotLight` deferred; point/spot shadows deferred
  (`Plan/clustered-forward.md`, `Plan/shadow-mapping.md`). Area lights absent.
- **Transparency and material coverage** — no alpha-blended, alpha-cutout or double-sided meshes
  (glTF refuses them with L1202); no order-independent transparency; no clearcoat, sheen,
  transmission or subsurface. Foliage, glass, fences and hair cards are out.
- **Textures** — PNG decoded to RGBA8 only; no BC/ASTC/KTX2 compression. Voxel texturing deferred
  (`Plan/textured-pbr.md`).
- **Anti-aliasing and upscaling** — supersampling (`--scale`) only. FXAA deferred; SMAA absent
  (both compatible with the no-temporal rule); FSR/DLSS absent (the temporal ones are out with TAA).
- **Reflections and GI** — IBL of the procedural sky plus the opt-in voxel/probe diffuse GI. No SSR
  (deferred), no reflection probes, no specular GI.
- **Exposure** — manual only; auto-exposure absent. `.cube` LUT grading deferred.
- **Game-authored shaders** — absent: no custom WGSL surface, no material graph
  (`dev-docs/ludion-shader-bindgen.md`).
- **Cameras** — exactly one (`L2101`): no render-to-texture, split screen, portals; minimap deferred
  (a private plan).
- **Geometry** — no decals (analytic `GroundMarker` telegraphs only), terrain renderer, foliage,
  morph targets (L1202), GPU-driven or indirect draws. MRT deferred.
- **Particles** — CPU, unlit camera-facing discs. No GPU simulation, mesh particles, collision;
  world-space trails rejected (`docs/particles.md`).
- **Output** — sRGB swapchain only; no HDR display output, no VRS.
- **Debug draw** — designed (`Plan/character-controller.md` W3); none in the engine today.
- **Mesh shaders, ray tracing** — deferred (`CLAUDE.md`).

### Animation

- State machines / blend trees, masked and additive layers, IK, root motion, proportion
  retargeting, morph targets, sockets — each deferred with a trigger (`Plan/animation-clips.md`
  non-goals). A data-driven state machine was rejected as a pillar violation
  (`Plan/voxel-animation.md`); the Rust-side shape is open.
- Timeline / sequencer for cutscenes and tweening — absent (`kind: pattern` is bullet-volley only).

### Physics

- 3D character motor — designed (`Plan/character-controller.md`); 2D `move_and_slide` ships.
- CCD — deferred (speculative contacts only; trigger: tunnelling the substep count can't fix).
- Vehicles — deferred (open-world slice). Cloth, soft bodies, fluids, destruction — "each needs a
  written case".
- Scale — single-threaded solver; the gate is 512 bodies, about 2k is the measured wall.

### AI

- Navmesh, pathfinding library, behaviour trees, steering, perception — absent in the engine.
  vigil runs game-side flow fields; `ludion-nav` deferred until a second consumer
  (a private plan).

### Audio

- WAV effects, every source downmixed to mono: no stereo panning. Streamed Ogg music shipped
  2026-10-03, rev 1106 (external shmup request 10); music crossfades, Opus and live music volume stay
  deferred. No runtime volume control, no reverb, occlusion or HRTF (out of scope,
  `Plan/audio-ambience.md`).

### Input and UI

- One gamepad; action maps and rebinding deferred (`Plan/phase-13-input.md`).
- Text is an embedded 8×8 uppercase ASCII bitmap: no TTF/OTF, no Unicode, hence no localization.
  No layout engine, widget tree, theming or clipping (`Plan/ui-widgets.md`, `Plan/phase-14-ui.md`).
  Accessibility absent. Localization and accessibility wait on a post-M7 grilling.

### Shipping

- `ludion export` targets iOS only; no desktop packaging, asset packing or installer. Content is a
  directory root by ruling (`LaunchRoots`).
- Linux untested; Android and web ruled out; consoles deferred (`Plan/console-strategy.md`).
- Networking — none; no plan, no trigger.

### Editor

- No general asset browser (voxel and sculpt kinds only), no prefabs, no log console (a
  single-diagnostic banner), no material editor, no sequencer, no terrain painting, no
  distributable build.
- Play is `sim.resume`; Stop does not revert the world, though `engine.snapshot`/`engine.restore`
  already exist.
- **No editor view has been exercised in the real GUI**: the frontend gates cannot render React,
  so every view queues a human checklist (`Plan/editor-live-smoke.md`: "nothing here has been
  exercised").

### Rejected by design — keep them rejected

Deferred shading, TAA and every form of temporal accumulation, MSAA, lightmaps, HDRI and physical
skies, embedded scripting. Each costs something real (cheap AA, temporal upscalers, SSR quality),
and each buys the bit-exact sim and the golden-image tier. They are not gaps to fill.

## Recommended order

Scored on three questions: is a real user blocked, does the cost grow the longer it waits, and does
it raise the number of autonomous agent iterations (the North Star)?

1. **Unblock the consuming games.** The external games' request folders are the priority queue;
   a `blocking` request outranks any roadmap wave.
2. **Drain and mechanize the human queue.** `Plan/owed.md` carries about thirty entries and grows
   faster than sittings drain it. Run the G7 merged sitting (it unlocks the M2–M7 closes), and build
   a real-GUI gate for the editor — the React app driven by a browser automation harness against
   `ludion serve`, asserting DOM and screenshots — so most live-smoke checklist lines become
   mechanical and only feel verdicts need a human.
3. **Foundations whose cost grows with every game:**
   - Transform propagation plus runtime template instantiation (prefabs). The platformer's moving
     platforms and held props, world-streaming W2, and editor prefabs all sit on it. Grill the
     sim-side-versus-extract question first.
   - Frustum culling and a visibility flag (world-streaming W0/W1, no trigger needed; budgets gate it).
   - Font assets (TTF/OTF into a glyph atlas) — prerequisite for localization, menus and any
     shipped game.
4. **The platformer as the next vertical slice** (`Plan/character-controller.md`). Let it pull the
   3D gaps it actually hits rather than building them in the abstract: debug draw (also an
   agent-verification win — colliders visible in screenshots), cascaded shadows, alpha-cutout and
   double-sided materials (the Blender pipeline hits these on its first props), animation blending
   beyond crossfade, CCD if a fast fall tunnels.
5. **The shippable-game bundle, when a game is declared shipping** (post-M7 grilling): desktop
   export and packaging, texture compression (before iOS device work), music crossfades, stereo
   panning with volume buses and a settings surface, input rebinding, localization, SMAA.

Not recommended now: networking, material graphs or a custom-shader surface (a second, weaker API
surface — Pillar 6), GPU particles, SSR or reflection probes, terrain, vehicles, Android, web,
consoles.
