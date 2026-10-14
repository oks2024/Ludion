# Plans

One markdown file per plan. A plan records the decisions, the waves it ships in, and a **Status** section that is its shipping log; read the status line below first, the file's own Status section for detail.
[`owed.md`](owed.md) is the ledger of everything waiting on a human, the second pinned machine, or a trigger.
"Mechanized" means every wave shipped and the automated gates are green; a plan is not closed until its human gate passes.
Terms are defined in the [glossary](#glossary).

The reference game that drove many engine arcs is private; its design and plans are not in this repository. Engine plans mention it as history.

## Engine — active

- [Animation clips](animation-clips.md) — W1–W3 mechanized (clip import, characters, playback, crossfade, events); editor sitting owed.
- [Asset pipeline with Blender](asset-pipeline-blender.md) — phase 1 verdict: Blender for the environment, characters stay voxel; kits and loop shipped.
- [Comfy generation](comfy-generation.md) — W1–W4 mechanized (`kind: generation`, `ludion gen`, the Generation view); checkpoint 2 and live smoke owed.
- [Editor art director](editor-art-director.md) — wave 1 shipped; the cost column (wave 2) and the sitting are open.
- [Editor open project](editor-open-project.md) — W1–W4 mechanized (launch on a game, run it, switch scenes and projects); live smoke owed.
- [Editor view camera](editor-stage-camera.md) — W1–W4 mechanized (`render.view`, orbit and fly in every view); live smoke owed.
- [Editor voxel assemblies](editor-voxel-assemblies.md) — W1–W4 mechanized (assemblies staged read-only in the Voxel Artist); live smoke owed.
- [Font assets](font-assets.md) — W1–W3 mechanized (TTF/OTF bake, `font` field, SDF); W4 breakout and cross-arch bless owed.
- [iOS target](ios-target.md) — D1–D5, M1, M2 done (`ludion export`, simulator run); device milestones M3–M5 wait on the user's word.
- [Mesh modeling](mesh-modeling.md) — W1–W3 mechanized (`kind: sculpt`, implicit-surface mesher, Models view); paused, checkpoint 2 frozen.
- [Mesh texture editing](mesh-texture-editing.md) — W1–W2 mechanized (`kind: mesh-edit`, `ludion mesh pick`/`apply`); Wave 3 waits on the user's word.
- [Particles](particles-engine-and-editor.md) — W1–W4 shipped (stateless engine emitter, Effects view); pinned-box measurements and sittings owed.
- [Physics](physics.md) — W1–W4 mechanized (`ludion-physics` over pinned rapier3d); cross-arch verified, nothing owed.
- [Physics collision layers](physics-collision-layers.md) — W1–W2 mechanized; W3 closed as not needed; nothing owed.
- [Projectile patterns](projectile-patterns.md) — W1–W4 mechanized (`kind: pattern`, `Volley`, `PatternPlayer`); exit rides a first real consumer.
- [Rotation and camera representations](rotation-representation.md) — W1 shipped (rotation math in `ludion_core::math`); macOS half owed.
- [Scalability](scalability.md) — W1–W3 landed (nine quality axes, render scale); W4 planned, macOS confirmation owed.
- [Scene load speed](scene-load-speed.md) — planned (load-phase profiling, parallel asset prefetch, cache retained across loads); not started.
- [Sector state persistence](sector-state-persistence.md) — W3–W4 complete (scene residue across sector transitions); W4 human gate owed.
- [Skeletal animation](skeletal-animation.md) — W1–W3 mechanized (glTF skins, joint tree, GPU bone palette); W4 not started.
- [Sprite engine](sprite-engine.md) — W1–W4 mechanized (sprite/atlas kinds, bake, `generate-atlas`); close rides the external consumer and human verdicts.
- [Telegraphs and VFX](telegraphs-and-vfx.md) — wave 1 complete (`GroundMarker` end to end); later waves not started.
- [Textured PBR](textured-pbr.md) — W1–W4 mechanized (glTF texture-map import fidelity); exit gate's human look owed.
- [Transform hierarchy and prefabs](transform-hierarchy-and-prefabs.md) — W0–W4 mechanized (`Parent`-local transforms, `kind: prefab`); live smoke and exit sitting owed.
- [UI widgets](ui-widgets.md) — W1–W3 mechanized (the selectable list, pixel hit-testing); controller-only sitting owed.
- [Voxel animation](voxel-animation.md) — all five waves mechanized (rigid-part assemblies, declarative clips); sitting owed.

## Engine — shipped

**Rendering**

- [Audio ambience](audio-ambience.md) — closed (loops, positional emitters, category gains).
- [Clustered forward](clustered-forward.md) — mechanized-complete (the many-lights cap removed).
- [Environment, sky and IBL](environment-ibl.md) — mechanized-complete; the look sitting is folded into the merged human gate.
- [GI wave 0: voxelizer](gi-wave0-voxelizer.md) — delivered.
- [GI wave 1: sky visibility](gi-wave1-sky-visibility.md) — delivered.
- [GI wave 2: emissive bounce](gi-wave2-emissive-bounce.md) — delivered.
- [GI wave 3: dynamic injection](gi-wave3-dynamic-injection.md) — delivered.
- [Global illumination](global-illumination.md) — closed, every wave delivered; the look sitting is folded into the merged human gate.
- [Phase 11: PBR mesh shading](phase-11-pbr.md) — closed.
- [Post-process pipeline](post-process-pipeline.md) — closed; only the shipped `ssao_direct` value rides a human sitting.
- [Shader bindgen](shader-bindgen.md) — landed (reflection-generated CPU bindings).
- [Shadow mapping](shadow-mapping.md) — landed (sun-only first slice).
- [Volumetric medium](volumetric-medium.md) — closed; the exit sitting is owed.

**Simulation and data**

- [Deterministic job system](deterministic-job-system.md) — closed; parallelism through `ludion_core::par`, system-level parallelism ruled out.
- [Engine evolutions for the first game](engine-first-game.md) — waves E1–E7 shipped as phases 12–17.
- [Phase 12: spatial module](phase-12-spatial.md) — closed (E1).
- [Phase 13: input](phase-13-input.md) — closed (E2; mouse and gamepad).
- [Phase 14: screen-space UI](phase-14-ui.md) — landed (E3).
- [Phase 15: game data-docs](phase-15-data-docs.md) — landed (E4).
- [Phase 16: text profile API](phase-16-profile.md) — landed (E5).
- [Phase 17: scene transitions](phase-17-transitions.md) — landed (E7).
- [Presentation state](presentation-state.md) — built (entities the sim never sees and the hash never covers).

**Editor**

- [Editor field sliders](editor-field-sliders.md) — shipped.
- [Editor viewport latency](editor-viewport-latency.md) — mechanized-complete (phases 1a, 1b, 2, 2b).
- [Editor views](editor-views.md) — closed (role-based workspaces, embedded terminal).
- [Editor voxel artist](editor-voxel-artist.md) — waves 0–4 shipped; the view is code-complete.

**Tooling and pipeline**

- [Assembly `--set`/`--sweep` staging](assembly-set-sweep.md) — shipped.
- [Engine profiler](engine-profiler.md) — shipped (CPU, GPU and transport, agent-queryable).
- [Frame codec: LZ4](frame-codec-lz4.md) — closed.
- [Map authoring](map-authoring.md) — M0 complete (kit-of-parts levels from a text map source).
- [Map tooling ergonomics](map-tooling-ergonomics.md) — complete.
- [Phase 6: editing](phase-6-editing.md) — closed.
- [Phase 7: designer flow](phase-7-designer-flow.md) — closed.
- [Phase 8: assets and sound](phase-8-assets-and-sound.md) — closed; its atlas deferral is superseded by the sprite engine.
- [Phase 9: hardening](phase-9-hardening.md) — waves 1–6 done.
- [Phase 10: voxel models](phase-10-voxel-models.md) — implemented.
- [Voxel artist at scale](voxel-artist-scale.md) — executed.
- [Voxel authoring tools](voxel-authoring-tools.md) — mechanized-complete.
- [Voxel tooling ergonomics, round 2](voxel-tooling-ergonomics-2.md) — shipped.
- [Voxel tooling, round 3](voxel-tooling-round-3.md) — shipped.

## Deferred and not started

- [Build graph, file size and `par`](build-graph-and-par.md) — planned, not started.
- [Character controller and camera rigs](character-controller.md) — design settled, nothing built.
- [Console strategy](console-strategy.md) — evaluated, not scheduled.
- [Document-kind abstraction](document-kinds.md) — planned, not started.
- [Editor systems panel](editor-systems-panel.md) — evaluated, not scheduled.
- [Carve `Engine` by lifetime](engine-carving.md) — planned, not started.
- [Move the instruments out of `ludion-engine`](engine-crate-split.md) — planned, not started.
- [GPU-driven culling](gpu-driven-culling.md) — design open, not settled.
- [Protocol shape hash](protocol-shape-hash.md) — planned, not started.
- [World streaming](world-streaming.md) — evaluated, not scheduled.

## Records

Audits, remediations, evidence of blessed goldens, surveys and operational guidance. None of these is a build plan.

- [Owed](owed.md) — the ledger of everything waiting on a human, a second machine, or a trigger.
- [Post-publication](post-publication.md) — hygiene deferred past publication, one trigger each.
- [API freeze: client crates](api-freeze-clients.md) — executed checklist for the publication-facing crates.
- [API freeze: engine and renderer crates](api-freeze-engine.md) — executed checklist.
- [API freeze: foundation crates](api-freeze-foundation.md) — executed checklist.
- [Audit remediation](audit-remediation.md) — waves fixing the 2026-07-17 engine audit.
- [Audit remediation, October 2026](audit-remediation-2026-10.md) — groups fixing the 2026-10-03 audits; follow-ups owed.
- [Audit remediation, 2026-10-10](audit-remediation-2026-10-10.md) — fixes for the 2026-10-10 code audit.
- [Bullet-hell stress scene](bullet-hell-stress.md) — a feasibility measurement above the proven entity ceilings.
- [Code audit, 2026-07-25](code-audit-2026-07-25.md) — full-repo audit report; report only, no fixes.
- [Documentation rework](documentation-rework.md) — the split of agent-docs by audience into task-scoped topics.
- [Editor live-smoke checklists](editor-live-smoke.md) — the queued human editor sitting; nothing exercised yet.
- [Engine audit, 2026-07-17](engine-audit-2026-07-17.md) — eight-scope audit report.
- [Engine gap map](engine-gap-map.md) — reference survey against a complete modern 3D engine; not a build plan.
- [macOS port: per-OS goldens](mac-port-bless.md) — bless event evidence.
- [Phase 11 wave 4: bless event](phase-11-wave4-bless.md) — bless event evidence for the BRDF swap.
- [Roadmap: voxel models, mesh path, PBR](roadmap-voxel-mesh-pbr.md) — settled multi-phase direction.
- [Voxel-artist playbook](voxel-artist-playbook.md) — operational guidance for a small local model.
- [Voxel tooling field feedback](voxel-tooling-feedback.md) — findings from authoring a large model; answered by rounds 2 and 3.
- [`artifacts/`](artifacts/) — committed evidence artifacts, with their own README.
- [`audits/`](audits/) — dated audit reports and the master audit.

## Glossary

- **bless / re-bless** — accept new golden images or budgets on a pinned machine as the new reference.
- **checkpoint** — a user verdict inside a plan that gates the waves after it.
- **closed** — every wave shipped and every human gate passed.
- **drift gate** — a check that a committed derived file is byte-identical to its regeneration from source.
- **E1–E7** — the engine waves of the first-game arc ([engine-first-game](engine-first-game.md)): spatial, input, UI, data-docs, profile API, perf gate, transitions.
- **exit gate / exit criteria** — the conditions that close a plan, often a human sitting.
- **G0–G7** — milestones of the reference game, which is private.
- **golden** — a committed reference output (image, trace, hash) a test compares against.
- **grilling** — a design Q&A session with the user that settles a plan's decisions.
- **L-code** — an `L####` diagnostic code; `ludion explain L####` prints it.
- **live smoke** — a human run of the real editor GUI against a checklist.
- **Lore rev** — a revision in the project's primary version control; the git mirror is derived from it.
- **M0 / M1** — milestones of the reference game's private roadmap; both closed.
- **mechanized / mechanized-complete** — every wave shipped and the automated gates green; not closed until its human gate passes.
- **owed** — waiting on a human, the second pinned machine, or a trigger; see [owed.md](owed.md).
- **phase** — a numbered step of the engine build order (phases 0–11 closed; 12–17 are the first-game waves).
- **pinned platform / pinned GPU** — the two machines whose goldens are bit-exact: Windows on an RTX 3090 and macOS on an M4.
- **sitting** — one human session that clears queued live checks and verdicts.
- **trigger** — the condition that un-defers a plan or fires its next step.
- **twin** — the other platform's counterpart of a golden or trace (the macOS twin of a Windows golden).
- **wave (W1…)** — a shippable slice of a plan, committed and verified on its own.
