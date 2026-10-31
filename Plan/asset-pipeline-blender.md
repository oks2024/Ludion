# Plan: Asset pipeline with Blender — the agent drives Blender, `ludion import` takes the result

> Status: **DESIGN SETTLED (2026-09-26, grilling session; every ruling is the user's). Nothing built.**
> **Phase 1 verdict (2026-10-02): Blender wins for the environment.** The 2026-09-30 town experiment (15+ kits, headless
> artists, a shared `STYLE.md`, baked PBR) was judged by the user far above the voxel city; characters stay voxel for now
> (decision 2026-10-01). The loop and the schedule are private plans;
> sources are `games/vigil/art/town/`. Kits export glTF separate (gltf + bin + PNG), which `ludion import` takes.
> **Phase 1 in progress.** Brief 1, the idol, was authored 2026-09-27: 231 k tokens, 18.4 min, a clean
> import, 4 844 triangles, a one-iteration follow-up edit, and a byte-identical headless replay
> (`Plan/artifacts/asset-pipeline-blender-log.md`). **The user rejected it on look (2026-09-27): "does not
> look good" (it was built from primitives, with no Blender modelling tools used). The attempt is
> ignored and the idol restarts from scratch in a fresh session.** Its cost row stays in the log, marked
> as not counting. `blender-craft`'s conventions (export settings, the face-order trap) still stand. The engine half it needs for the goblin is
> `Plan/skeletal-animation.md`, which runs in parallel. `Plan/mesh-modeling.md` is **paused** for this
> experiment (decision 1). Every claim of current-code fact below was read from the tree on 2026-09-26.
>
> **How to read this plan.** Decisions 1–16 are the rulings. *The asset contract* is the
> specification a brief's author follows. Each phase is a step list with its exit. A brief closes by
> appending its log row and the user's read to Status. A phase closes by recording its verdict as a
> decision and refreshing the root `CLAUDE.md` Current-state line.

## Why

Two routes to meshes exist. The in-engine sculpt format (`Plan/mesh-modeling.md`) reached its
creature checkpoint, and the user's first read of that creature was negative. The ComfyUI route
(`Plan/comfy-generation.md`) produces a textured image-to-mesh result that is frozen: it has no parts,
no bones and no editable structure. Blender driven by an agent over the official Blender MCP
extension gave strong results on the user's other projects. It also covers what both routes lack:
modifiers and bevels for hard surfaces, retopology and decimation, UV unwrapping and baking, and
rigging.

The question this plan answers: **can the agent author game-ready meshes in Blender cheaply and
autonomously enough to be Ludion's mesh source?** It measures on real content for the next game,
an external physics game (the one that already drives `Plan/physics.md` and
`Plan/physics-collision-layers.md`), plus one head-to-head brief shared with the two existing routes.

The longer-term question stays open: see what Blender does really well, then judge whether a
dedicated in-engine tool could do better. Sculpt is parked, not retired, so that comparison stays
possible.

## What exists (verified 2026-09-26)

- **Blender 5.2.2 LTS** is installed on the reference box, with the official MCP extension. Its tools
  run arbitrary `bpy` code, take viewport and window screenshots, render the viewport or a thumbnail
  to a path, summarize objects and datablocks, and search the bundled API and manual. The stock glTF
  exporter (`io_scene_gltf2`) is enabled.
- **`ludion import`** (`crates/ludion-import`, `dev-docs/ludion-import.md`) turns a `.gltf`/`.glb`
  into canonical text: `scene.yaml` with one entity per node, `meshes/*.mesh.yaml`, and
  `textures/<hash>.png`. Re-importing gives byte-identical output, and `check-scene` plus
  `check_mesh` run before anything is written.
  - **Supported:** triangles with positions, normals and `TEXCOORD_0`; tangents, given or generated;
    the five metallic-roughness maps as PNG; factors; emissive factors (a tint bakes into an emissive map);
    multiple nodes and meshes; perspective cameras and point lights.
  - **Refused with `L1202`:** skins, animations, morph targets, `BLEND` alpha, a second UV set, JPEG images, and unknown required extensions. The skin refusal is
    lifted by `Plan/skeletal-animation.md` W1. *Amended 2026-10-30 (owner approval 2026-10-29, Clockout 0010): `doubleSided` and `MASK` are mapped (`double_sided`, `alpha_mode: mask`), no longer refused.*
- **Trap: a material with no factors imports as fully metallic** (the glTF defaults are 1.0).
- **ComfyUI** (`games/vigil/generation/workflows/`): all four workflows start from images. TRELLIS
  textures only the mesh it generates. No installed node loads an existing mesh.
- **Baselines for the head-to-head (the idol):**
  - Sculpt: 10 sheet renders, ~191 k tokens (`Plan/artifacts/mesh-modeling-log.md`).
  - ComfyUI: 5 sheets, 142 k tokens (`Plan/artifacts/comfy-generation-log.md`).
  - Blender over MCP on earlier projects: ~15 attempts and ~2 M tokens per mesh
    (`Plan/mesh-modeling.md` decision 17). This is the prior this plan re-measures.
- **Verification tools for a mesh:**
  - `ludion render --model <mesh.yaml>`: the six-view sheet.
  - `check-scene <mesh>.mesh.yaml --bounds`: text output.
  - `L1110`/`L1111`: structure and consistency errors.
  - `L1112`: a warning over 10 000 **vertices**. UV seams roughly double the vertex count.
  - There is no triangle budget for imported meshes. The per-brief budget below is a threshold of
    this plan, not an engine check.

## Decisions

1. **Sculpt is parked as is.** Its code, docs, gallery content and goldens stay shipped and built.
   `Plan/mesh-modeling.md` records the pause. Checkpoint 2, W4 and W6 carry a resume trigger (the
   user's word after this plan's phase-1 verdict). Its `owed.md` items are frozen, not dropped.
   **Why:** the later question "could a dedicated tool beat Blender" needs the sculpt path alive, and
   no negative verdict was recorded.
2. **Blender is an agent tool, and becomes a pipeline verb only if it wins.** In phases 1–2, Ludion
   contains no Blender code: the agent drives Blender over MCP, exports a `.glb`, and `ludion import`
   produces the committed mesh. If the verdict is positive, phase 3 adds a `ludion blender` verb that
   runs the committed scripts headless. The editor would then invoke it as a subprocess with `--json`,
   so it satisfies the parity gate the way `gen` does (`Plan/comfy-generation.md` decision 15).
   **Why:** a verb built before the workflow is validated is gold-plating. The glb → import path works
   today.
3. **The source of an asset is a replayable bpy script.** The committed source is a self-contained
   script. The exported `.glb` and the imported mesh tree are derived and never hand-edited (the
   root `CLAUDE.md` rule for generated content). Interactive MCP exploration is allowed while
   authoring, but a brief is done only when its script rebuilds the asset from a factory-startup
   Blender. **No `.blend` file is ever committed.** **Why:** Pillar 4. A script is text, diffs, can be
   iterated on by an agent, and is exactly what a phase-3 verb would run.
4. **Pinned inputs.** When a stage consumes an input it can't reproduce (a ComfyUI glb, a
   hand-supplied file), that input is copied into the asset's `inputs/` folder and committed, and the
   scripts replay from there. ComfyUI runs stay ignored by git and lore. **Why:** this keeps
   decision 3's promise without pretending ComfyUI is deterministic.
5. **Stages hand off `.glb` files, and only the last stage is imported.** A chain (Blender → ComfyUI
   → Blender, in any order) passes glb files between stages. `ludion import` runs once, on the final
   glb. **Why:** glb is what both tools speak natively. An intermediate import would need a Ludion →
   glb exporter, which `Plan/mesh-modeling.md` decision 15 rejects as a second source of truth.
6. **Blender alone first.** Phase 1 uses no ComfyUI. Chains, and a ComfyUI node pack that textures an
   existing mesh, are phase 2. **Why (the user's words):** nail Blender alone before combining tools.
7. **Sources live under `generation/blender/<asset>/`** in the owning game, beside the
   ComfyUI recipes. The imported tree goes to `scenes/meshes/<asset>/` as it does today. **Why:**
   `generation/` is already the tree of things that produce committed assets, and a chain that mixes
   tools reads as one place.
8. **Blender 5.2.2 LTS is pinned.** Every script asserts the major.minor version as its first
   statement and fails with a readable message otherwise. The provenance file (*The asset contract*)
   records the full version. An upgrade is an explicit ritual: re-run every script, re-import, and
   review the diffs. It follows the shape of the physics pin ritual (`dev-docs/physics-pin.md`).
9. **Two plans.** This plan is the experiment. Skinning in the engine is `Plan/skeletal-animation.md`.
   The goblin brief joins them. **Why:** an external physics game needs skinning whichever tool wins, so a negative
   Blender verdict must not orphan the engine work.
10. **Run in parallel.** Phase 1 starts now, and the skeletal arc starts alongside it. The goblin is
    modelled and rigged early and waits for the engine to play it. **Why:** the user overrode a
    "static first" gate for production reasons.
11. **External-game assets are committed in that game's repo; the idol in the reference game.** The
    external game's assets (a skinned humanoid, a hinged prop, a small organic prop, a modular kit) land
    in its own `generation/blender/` and `scenes/meshes/`. The idol lands in `games/vigil/`. This plan and its log hold the measurements, verdicts and pointers.
    **Why:** nothing becomes throwaway, and the in-engine read happens under the real game's scale and
    light.
12. **Phase 1 order: idol → goblin → hinged prop → organic prop → modular kit.** The idol is the calibration
    run. It is a known brief with numbers from both other routes, it is cheap, and it pays for the
    mistakes in the conventions (exporter flags, axis, scale, the metallic default) before the
    goblin's measurement.
13. **Verdict thresholds.** A brief passes if all five hold:
    1. The user's in-engine read (the `render --model` sheet, and the asset in a game scene)
       is "good enough to ship as a placeholder, or better".
    2. The agent reaches its own "done" with at most one human look, not counting the blockout
       approval.
    3. Cost is at most 500 k tokens and 1 h wall-clock per static brief, and at most 1.5 M tokens for
       the goblin. The prior for comparison is ~2 M tokens and ~15 attempts.
    4. The import is clean: no `--allow-lossy`, no diagnostics, correct scale and up-axis, and at most
       5 k triangles (10 k for the hero goblin).
    5. A follow-up edit ("longer ears", "heavier counterweight") lands in one script edit plus a
       re-run: one iteration.
14. **Measurement: one fresh subagent per brief.** Each brief runs in its own subagent, so its tokens
    and wall-clock come from the task usage alone. It is briefed with the sealed brief,
    `ludion docs blender-craft` and this plan's *Asset contract*, and nothing else. Results go to
    `Plan/artifacts/asset-pipeline-blender-log.md`. The ranked tool-friction list goes in the same log
    (the authoring-run convention).
15. **Headless replay is measured in phase 1, and gated in phase 3.** Each brief records whether a
    headless re-run (`blender -b --factory-startup --python <script>`) followed by an import gives
    byte-identical output, **always from two fresh Blender processes**. The idol showed that
    `bmesh.ops.create_uvsphere` and `solidify` permute face order between processes but not inside
    one session, so exports within one MCP session prove nothing. The drift gate is designed with the phase-3 verb, as a byte check or a
    perceptual check depending on what phase 1 measured. **Why:** a gate built before the exporter's
    stability is known would be flaky (a P0) or would have to be loosened.
16. **Craft knowledge lives in `ludion docs blender-craft`.** The doc is created with the idol brief
    and grown by every brief: exporter settings, axis and scale, the import traps, the rig-to-ragdoll
    rules, a script skeleton. **Why:** instances on an external game author with Blender straight after phase 1,
    and they read `ludion docs`, not Ludion's `Plan/artifacts/`.

## The asset contract

```
<game>/generation/blender/<asset>/
  build.py          # or 01-blockout.py, 02-finish.py … for a chain; each one stage
  inputs/           # pinned binaries a stage consumes (decision 4); absent when none
  provenance.yaml   # written by the last stage: see below
<game>/scenes/meshes/<asset>/   # `ludion import` output: derived, never hand-edited
```

- **The script.**
  - The first statement is the version assert (decision 8).
  - It starts from `bpy.ops.wm.read_factory_settings(use_empty=True)`, or equivalently runs under
    `--factory-startup`, so the MCP session's state never leaks in.
  - Inputs are paths relative to the asset folder. It writes one `.glb` to a path given after `--`.
  - It has no randomness without an explicit seed.
  - It runs unchanged over MCP (`exec` of the file) and headless.
- **Export settings** are named once in `blender-craft` and copied into every script:
  - glb, +Y up.
  - PNG images only.
  - Only the objects in the export collection.
  - Every material has backface culling on for closed meshes and off for sheets (off exports
    `doubleSided`, which imports as `double_sided: true`), an opaque blend mode (or a `MASK` threshold
    for holes), and explicit metallic and roughness values.
  - Modifiers are applied.
  - The first brief settles the scale convention (metres to units) and records it.
- **`provenance.yaml`** (text, committed): the tool (`blender`), the full Blender version, the date,
  each script with its content hash, each pinned input with its hash, the exporter settings, and the
  import command and output folder. **Why a sidecar and not the mesh document's `origin:` block:**
  `origin:` is ComfyUI-shaped and only `gen accept` writes it. Writing it from an import would need
  Ludion code, which decision 2 leaves out of phases 1–2. The phase-3 verb moves provenance into
  `origin:`.
- **Verification loop for a brief:** Blender screenshots are the author's fast loop. The accepted
  check is Ludion's own: `ludion import`, then `check-scene --bounds`, then `ludion render --model`,
  then the asset placed in a scene of its game. A hand-authored stage scene (`scene-lit.yaml`) may sit
  beside the derived tree, as `idol-generated/` already does. The import never writes that name.

## The briefs (phase 1, sealed before authoring)

Each brief's full text is written into the log before its subagent starts and is never edited after.

1. **Idol (vigil, head-to-head).** The sculpt arc's brief unchanged (`Plan/mesh-modeling.md`
   decision 2 and Wave 1 step 6): an idol on a plinth with one emissive element. It is judged on
   the same stage and views as the sculpt and ComfyUI idols. Flat factor materials, plus the emissive.
2. **Goblin (external physics game, skinned hero).** The user's art brief (2026-09-26):
   - **Look:** stylized, frail, Warhammer-like. It looks kind of mean but also stupid, which makes it
     almost cute. Enthusiastic, happy to be here.
   - **Body:** long pointy ears, a potbelly, short bowed legs, a stubbled underbite, green-olive skin.
     About 1.1 m tall.
   - **Clothing:** a plain tunic, a belt, and a number tag on the chest as a baked decal. Gear
     (helmet, pauldrons, shield, satchel) is separate props, not part of the body mesh.
   - **Budget:** 6–8 k triangles, baked PBR (base colour, metallic-roughness, normal).
   - **Readability:** the silhouette must read at 20 m in the dusk light.
   - **Rig:** eleven bones matching the external game's ragdoll parts: head, torso, pelvis, and left and right upper
     arm, lower arm, upper leg and lower leg. Bone names come from that game's part names. Rest pose is a
     relaxed A-pose.
   - **Second output:** a **per-bone split**, eleven rigid part meshes cut by dominant weight. It is
     the external game's placeholder until skinning lands.
3. **Hinged prop (external physics game, hard-surface).** A physics object: a frame, a rigid arm on a hinge, a rigid
   counterweight, and a sling of jointed pieces. It is exported as **one mesh per rigid body**, with
   pivots at the joint points. It uses modifiers, bevels and arrays, and flat factor materials.
4. **Organic prop (external physics game, small organic).** A scatterable small prop, made in Blender from scratch in
   phase 1. Phase 2 re-does it as a ComfyUI → Blender chain, which gives a same-brief comparison.
5. **Modular kit (external physics game).** Three or four pieces on a shared grid and material. Pieces
   are **pre-broken**: each structural part is a separate mesh, because the external game holds structures
   together with `fixed` joints. One grabbable part is its own piece. One script produces the
   whole kit.

## Phases

### Phase 1 — Blender alone

1. **Conventions (with the idol).**
   - Create `docs/blender-craft.md` (with its embedded-docs mirror via `check.mjs --fix`) and the log
     (below).
   - Author the idol under `games/vigil/generation/blender/idol/`.
   - Settle and record the scale and axis conventions, the export settings and the import traps.
2. **The goblin**, then the **hinged prop**, **organic prop** and **modular kit**, in that order (decision 12).
   - Each one is a fresh subagent (decision 14).
   - The blockout gets one approval from the user, which is not counted as a look.
   - Each ends with the user's one look (in-engine), a log row, the follow-up-edit test (threshold 5)
     and the headless-replay measurement (decision 15).
   - External-game commits happen in its own repo. The log row points at them.
   - The goblin's script exports both glbs from the start, but only the per-bone split is imported
     until `Plan/skeletal-animation.md` W1 accepts skins. The skinned one is re-exported and imported
     then.
3. **The external game's docs.** Its `CLAUDE.md` asset rule and its design doc name this pipeline, and
   it uses the goblin's per-bone split (done 2026-09-26, alongside this plan).
4. **Exit: the phase-1 verdict.** A decision records it per brief and overall against decision 13's
   thresholds, with the idol compared across all three routes. Blender either wins (phase 3 is
   scheduled), loses (sculpt's resume trigger is offered), or is mixed (per-kind: which asset kinds
   go through Blender).

### Phase 2 — chains (on the user's word after phase 1)

1. Provision a ComfyUI node pack that textures a given mesh from reference images (Hunyuan3D-2.1
   Paint or equivalent; infrastructure is private). Write its `kind: generation` recipe.
2. The organic prop as a ComfyUI → Blender chain (TRELLIS mesh, then Blender cleans it up, re-bakes it and
   fixes scale and orientation).
3. The goblin texture as a Blender → ComfyUI → Blender chain (Blender shape, ComfyUI texture, Blender
   bake and cleanup).
4. Exit: a decision on which chains earn a place, and on whether `Plan/mesh-texture-editing.md` W3 is
   still needed.

### Phase 3 — the verb (only if Blender wins)

`ludion blender` (`run`, `check`, each with `--json`) runs the committed scripts headless and imports
the result. Provenance moves into the mesh document's `origin:`. The drift gate follows decision 15's
measurement, and the editor invokes the verb as a subprocess. This phase gets planned in detail at
the phase-1 verdict, not before.

## Engine/game boundary

Phases 1–2 change no engine code: content, docs, the log, and the external game's docs. The skinned goblin's
engine needs are all in `Plan/skeletal-animation.md`.

## Non-goals

- A `.blend` in either repo.
- A Ludion → glb exporter (`Plan/mesh-modeling.md` decision 15).
- Animation clips: glTF animations stay `L1202` (`Plan/skeletal-animation.md` decision 2).
- An editor view for Blender.
- Blender on CI or the GPU-less agents.
- Retiring sculpt.

## Risks

- **The MCP session and the headless run diverge.** User preferences, add-ons or scene leftovers
  change the output. Factory startup in the script is the mitigation. Decision 15's measurement is
  the detector.
- **Screenshot-heavy loops are expensive.** Images dominate the token count. The log records
  screenshots separately from in-engine sheet renders, so the cost is explainable.
- **Exporter defaults hit `L1202`.** JPEG sources export as JPEG. *Amended 2026-10-30 (owner approval 2026-10-29, Clockout 0010): the `doubleSided` half is gone; it maps now.* The contract's export settings are the mitigation. The idol brief pays for
  whatever the settings miss.
- **The thresholds sit above what the other routes cost on the idol.** Sculpt spent ~191 k tokens
  and ComfyUI 142 k, against Blender's 500 k threshold. A Blender idol that passes decision 13 can
  still lose the head-to-head on cost; the verdict weighs both.
- **A second pinned toolchain.** The Blender pin joins rapier as a version that moves only by ritual.

## Artifacts

`Plan/artifacts/asset-pipeline-blender-log.md` is created with the idol. It holds one row per brief
and is never rewritten. Columns:

`asset | phase | sealed brief | MCP calls | Blender screenshots | sheet renders | tokens | wall-clock | human looks | import clean | triangles | follow-up edit iterations | headless replay byte-identical | verdict`

Below the table: the ranked tool-friction list, and each brief's render-by-render notes.

## Docs owed

Phase 1: `docs/blender-craft.md` (new; it goes into `docs/` so `ludion docs` serves it), a pointer
from `docs/import` or the mesh-import topic, and the external game's two docs. Phase 3: the verb's CLI docs.
