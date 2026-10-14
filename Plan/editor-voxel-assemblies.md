# Plan: Editor Voxel Artist — assemblies, parts and clips on the stage

> Status: **W1–W4 mechanized-complete 2026-09-19 — the exit is the live sitting, owed, not passed.**
> Decisions are marked `[user]` (ruled) or `[proposed]` (this plan's recommendation with the
> alternative it rejects, not contested). Runs under the **voxel-engine** feature stream, touching the
> editor stream's Voxel Artist view (`Plan/editor-voxel-artist.md`). It ran **on the
> `component.set` stage camera** (`Plan/editor-stage-camera.md` W2–W4 were not started when this
> plan's work landed), so that plan's W3 inherits this view's new panels when it lands. See `##
> Status` below for what shipped and what outlives it.

## Why

`Plan/voxel-animation.md` shipped multi-part models and declarative clips with **no editor surface**
(its decision 11: "file is the editor, the voxel precedent"). The Voxel Artist view arrived after
that ruling and was never extended: a human cannot open `player.assembly.yaml`, see which part is
which, or watch `walk` without the CLI. The voxel-animation human sitting (player at
`scale: 0.125`, the attack latch, the hinge read) is owed and is exactly the look this view should
give. The agent's side is whole already — `render --model x.assembly.yaml`, `--clip`, `--time`,
`--strip`, the per-phase gap report — so this plan adds **human perception of what the agent can
already perceive**, not a new capability.

## What exists today (verified)

- **The browser lists one extension.** `VoxelArtistView.tsx` passes `VOXEL_EXT` (`.voxel.yaml`) to
  `AssetBrowser`; the host's `list_editor_dir` takes one suffix. `player.assembly.yaml` sits in the
  listed directory and is filtered out; `parts/` and `anims/` would list as empty folders.
- **The stage can already show an assembly.** `Mesh.shape` is externally tagged —
  `{ "assembly": "voxels/player.assembly.yaml" }` beside `voxel`/`asset`/`sculpt` — and the view
  swaps the subject with `component.set`. No engine work stands between an assembly and the stage.
- **Live reload already covers all three file kinds.** The host watcher forwards every changed path
  under the content root unfiltered (`src-tauri/src/stage.rs` `relative_asset_path`), and the
  engine's `assembly_sources` reverse edges re-read the assembly when it, any part, or any bound
  clip changes (`ludion devdocs assembly-animation`).
- **Playback needs no new wire method.** `Animation { clip, started, speed }` is a registered
  component; pose is a pure function of `(clip, now − started)` at extract. The Effects view's time
  strip (`editor/effects/time.ts`) is the precedent: `started` is stamped from a reply envelope's
  `frame`, run state rides `sim.pause` / `sim.step` / `sim.set_speed`.
- **Scrubbing needs no new wire method either — and unlike particles it goes backwards.** With the
  sim paused `now` is fixed, so writing `started = now − k` poses the model at tick `k`. This is the
  same trick the strip sheet uses (`started = now − tick_i`). `started` is an unsigned frame index,
  so `now ≥ k` must hold; a fresh session is stepped forward once to clear the longest clip.
- **Parts are not entities.** Nothing on the wire can hide, tint or pick one part of a staged
  assembly. `render --model --part/--isolate` exists for `.sculpt.yaml` only.
- **A part file is a voxel grid without a palette** (`kind: part`: `size` + `slices`; glyphs resolve
  against the owning assembly's palette). An assembly's `palette:` block has the voxel format's
  shape. `editor/voxel/doc.ts` reads `size`/`palette`/`slices` with exact source locations;
  `cellview.ts` paints a slice from a grid plus a palette.
- **`check-scene` on an assembly prints a different OK line** — `OK (assembly figure, 8x8x2, 4
  parts, 1 clip)` plus one `clip <name>: <path>` line per binding — which `voxel/readout.ts` does
  not parse today.
- **Content:** vigil's `player.assembly.yaml` (11 parts, `walk` + `attack`) is committed and is the
  only fixture a test may name. The user's own local test assembly is what the view is wanted for;
  it stays uncommitted by the user's ruling, so nothing in the tree may depend on it.

## Decisions

1. **It lives in the Voxel Artist view, in the same browser.** `[proposed]` Assemblies list beside
   `.voxel.yaml` models with a badge; `.part.yaml` and `.anim.yaml` files are **not** listed — they
   are reached through the assembly that binds them, which is the only context in which a part has
   colours or a clip has a body. Two listings merged client-side (no host change); empty folders
   pruned with the opt-in prop the Meshes library added. *Rejected:* the Models view (it stages
   sculpts and meshes; voxel authors live here); listing parts standalone (a part has no palette).
2. **The stage shows what the game shows: `Mesh.shape.assembly` on `subject`, the asset reference
   `voxels/<relPath>`.** `[proposed]` What stands on the stage stays a function of the current
   selection and the committed scene (the stage-camera W1 rule): selecting a non-assembly removes
   the `Animation` component; a rig switch (`scene.load`) re-applies selection, clip and tick.
3. **One TypeScript reader for the assembly document, pure and tested against vigil's player.**
   `[proposed]` `editor/voxel/assembly.ts` reads `size`, `palette`, `parts` (name, file, parent,
   place, pivot, mirror) and `clips` from the strict-subset YAML; `anim.ts` reads a clip's
   `duration`, `loop` and per-track keyframe times. The readout stays the engine's (`check-scene`,
   second OK-line shape) — one checker. *Rejected:* scraping parts and clips from `check-scene`
   stdout (it prints neither the hierarchy nor pivots).
4. **Viewing a part is 2D-first, with the camera doing the 3D half.** `[user 2026-09-19]` A parts panel
   shows the hierarchy (parent tree, `mirror: x` badge, file, place, pivot). Selecting a part (a)
   opens its grid **read-only** in the slice panel, coloured through the assembly's palette, with
   the pivot drawn as a crosshair on the slices it crosses, and (b) re-pivots and frames the orbit
   on that part's rest-pose box (`place` + `size`, times the subject's scale — client arithmetic).
   No part is hidden or tinted in 3D. *Rejected for this plan:* 3D isolate/highlight — it needs an
   engine capability that does not exist, and the parity gate says the agent gets it first (see
   Deferred). A scratch one-part assembly written into the project tree was considered and
   rejected: the stage never holds unsaved state, and it litters content.
5. **Clips play through the `Animation` component on the subject, over existing wire methods.**
   `[proposed]` A clip strip under the viewport: the clip list from `clips:`, play / pause /
   restart, a loop toggle for one-shots (the Effects view's `loopDue` shape), the `sim.set_speed`
   multipliers, and a **tick-quantized scrub bar** — dragging pauses the sim and writes
   `started = frame − k`, latest-wins at the camera throttle's cadence. The bar draws each
   keyframe time from the clip file and flags any that is not a 60 Hz tick multiple (the documented
   authoring trap — a `t` between ticks never renders exactly). The label reads like the CLI's
   phase report: `t 0.4 (tick 24)`. Rest pose is the empty clip name.
6. **Parity of perception: the context block names the tick.** `[proposed]` The view's
   `.ludion/editor-context.json` block gains the assembly path, selected part, clip and tick, so
   the persona can run `render --model … --clip walk --time 0.4` and read the same pose the human
   is paused on. The persona file learns assemblies, the humanoid convention and the strip-first
   habit by pointer to `ludion docs voxel-assemblies` / `voxel-animation`, not by copy.
7. **Read-only first.** `[user 2026-09-19]` Waves 1–4 view; they do not edit parts, palettes of
   assemblies, pivots or keyframes. The edit surfaces are separate, triggered waves (Deferred) —
   the vertical slice is "a human can judge the player rig", which is a look.
8. **Wire stays at 29, no crate touched, no golden moves.** `[proposed]` Everything above is
   editor TypeScript over `component.set` / `component.remove` / `sim.*` and file reads.

## Waves

- **W1 — an assembly on the stage.** Decisions 1–3: merged listing with badge and pruning,
  `assembly.ts` reader, `Mesh.shape.assembly` staging with the selection-is-state rule, the
  assembly readout line, painter and palette panels showing an honest "assemblies are read-only
  here" state. Exit: `player.assembly.yaml` stands at rest on both rigs; pure modules tested over
  the committed player; `check.mjs --frontend-only` green.
- **W2 — the parts panel.** Decision 4: hierarchy tree, read-only part slices through the assembly
  palette, pivot crosshair, mirrored parts shown as authored with the badge, frame-on-part. Exit:
  every part of the player opens; the pivot crosshair agrees with the assembly comment block's
  global joint heights (a test computes `place + pivot` for shoulder, hip, knee).
- **W3 — the clip strip.** Decision 5: `anim.ts`, play/pause/restart/loop/speed, the scrub bar with
  keyframe ticks and the off-tick flag, the fresh-session step-forward guard. **Verify first:**
  that a `component.set` on a paused stage session produces a new frame (the scrub depends on it),
  and that the stage's sim runs unpaused today. Exit: a test drives the scrub arithmetic
  (`started = frame − k`, the guard, one-shot hold, loop wrap) as pure data; an integration test
  against a served fixture asserts the subject's `Animation` after play, scrub and deselect.
- **W4 — persona, context, docs, smoke.** Decision 6; `editor/personas/voxel-artist.md`; the
  editor README's view entry; a **Voxel-artist assemblies live smoke** block appended to
  `Plan/editor-live-smoke.md` (open the player, select `lower-arm-l`, play `walk`, scrub to the
  knee's peak at `t 0.45`, switch rig mid-clip and see the pose survive, edit a keyframe in a text
  editor and watch the stage re-pose with no restart). Exit: the block exists; the sitting is
  owed, not passed — and it can carry the voxel-animation sitting's player verdicts with it.

## Deferred (each with its trigger)

- **3D part isolate / highlight.** Engine-first by the parity gate: extend `render --model --part
  [--isolate]` from sculpt to assemblies (parts are already separate `MeshInstance`s in the sheet
  path, so isolate is an instance filter), then decide how a live stage shows it. *Trigger:* the W4
  sitting says the 2D grid plus frame-on-part does not answer "which piece is this".
- **Painting parts.** The surgical edit module (`voxel/edit.ts`) addresses `slices`, which a part
  file has in the same shape; the missing half is a palette that lives in another file. *Trigger:*
  the first sitting where the human wants to fix a cell while looking at a pose.
- **Assembly palette editing.** Same block shape as a voxel model's; the palette editor's surgical
  writes should carry over. *Trigger:* the statue-style recalibration recurring on an assembly.
- **Pivot and keyframe editing** (a pivot gizmo, a dope sheet). *Trigger:* a second animated
  character authored with a human in the loop. Until then the file is the editor and this view is
  the eye.
- **Onion skin, clip blending preview, mp4 export.** No trigger recorded; the engine has hard cuts
  only and `--strip` is the arc read.

## Settled questions (the user, 2026-09-19)

2D-first part viewing is enough for the first cut (decision 4); the view is read-only (decision 7);
the plan runs now, ahead of the stage-camera migration.

## Status

- **W1** — the browser lists `.assembly.yaml` beside `.voxel.yaml` with a badge, `parts/`/`anims/`
  pruned; `assembly.ts` reads size/palette/parts/clips positionally; the stage takes
  `Mesh.shape.assembly` on `subject` at `ASSEMBLY_STAGE_SCALE`, over the same selection-is-state rule
  as a voxel model.
- **W2** — the parts panel: hierarchy tree with mirror badges, a part's grid read-only under the
  assembly's palette, the pivot crosshair, frame-on-part.
- **W3** — the clip strip over the subject's `Animation` component: play/pause/restart/loop/speed
  through `sim.*`, a tick-quantized scrub bar (`started = frame − k`), the off-tick flag, the
  fresh-session step-forward guard.
- **W4** — decision 6's context block (the `assembly` sub-block on `ContextVoxel`, additive and
  omitted for a plain voxel model, tick/seconds present only while paused); the persona learns
  assemblies by pointer to `ludion docs voxel-assemblies`/`voxel-animation`; the editor README's view
  entry; this file's live-smoke block.

**Rulings that outlive the wave:**

- **`ASSEMBLY_STAGE_SCALE = 0.0625`** is the project's voxel content-density convention (16 voxels per
  world unit), not any game's placement — an assembly declares no `scale:`, so the stage has to pick
  a number and say which one it used rather than silently comparing against one the view chose.
- **The mirrored-pivot finding:** a `mirror: x` part's authored pivot is part-local and unmirrored,
  but the composed grid holds mirrored cells — `pivotGlobal` has to carry the pivot into the mirrored
  frame (`partSize.x − px`) before combining it with `place`. Skipping that step puts the crosshair a
  whole limb width off on every `-r` part of a humanoid.
- **`component.remove` on an entity that does not carry the component is `L3006`.** The clip strip
  only asks for the removal when its own state believes it put an `Animation` there (`clip !== null`)
  — a scene reload can drop the component without telling anyone, and treating that `L3006` as
  anything but ordinary would surface a banner for a normal reload.
- **`sim.set_speed` is wall-clock pacing, not a component field.** `Animation.speed` is written `1.0`
  always; a sim frame stays exactly one clip tick at any multiplier, which is what lets the bar read
  the same ticks at ×1 and ×0.25 and just call it slower.
- **No integration test covers the staged subject end to end.** The existing harness serves a scene
  with no `subject`/assembly entity, so W1–W3 verify through pure-module tests over the committed
  player fixture; the served-fixture assertion W3 sketched was not built.

**Known live-GUI risks, unverified until the sitting:**

- The pivot crosshair's pixel placement depends on the `.vx-grid` CSS grid's 1px cell gap; a stylesheet
  change could shift it off a cell boundary without any test failing.
- The scrub bar's playhead and the frame readout both move on the 10 Hz frame poll (`PLAYING_POLL_MS`),
  not on every sim tick — fast motion may look stepped rather than smooth.
- The asset browser does not re-list a new assembly written to disk until the view is re-entered
  (its reload key is keyed on save/revision, not on a directory watch).
