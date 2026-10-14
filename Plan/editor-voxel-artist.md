# Plan: Editor Voxel Artist — an asset-iteration view (Editor slice #3)

> Status: **waves 0–4 SHIPPED 2026-07-24 (Lore 329–334); the view is code-complete.** Remaining:
> the wave-5 `voxels/` folder reorg (held — see below) and the live sitting. The wire surface stayed
> at **26 methods** and wave 0 moved no rendered pixel, as designed. Per-wave outcomes, the decisions
> taken during implementation, and the live-smoke list are in the editor's private notebook and `Plan/editor-live-smoke.md`.
>
> **The reorg is bigger than its one-line description below.** `WorldHash::compute` hashes every
> serialized component value and a model path lives in `Mesh.shape.voxel`, so renaming the files
> changes all three districts' WorldHash and forces a `run.lrp` re-record plus a golden re-bless —
> which churns the artifact the pending G7 human gate sits on. The run itself does not change (same
> inputs, same trajectory; only the recorded hash trace moves). Held for a user decision.
>
> Two drift surfaces this arc opened, both deliberate, neither gated: the night stage **copies**
> `env-night.yaml`'s Environment+moon block (a scene document has no include mechanism — `fragments:`
> is a `kind: map` key), and the editor host hardcodes `PROTOCOL_VERSION = 1` in its own small wire
> client rather than depending on `ludion-client`.
>
> Input of record is the editor's private feature notebook
> (decisions settled 2026-07-24 via a grill-me interview with the user — this plan condenses and
> phases them and does not re-litigate any of them). This arc is **client-side plus one contained
> engine format addition**: the wire surface stays at **26 methods** (the view rides `component.set`,
> `asset.reload`, `scene.load`, `render.screenshot`, `query.entities`), and the one engine change
> (wave 0) never touches the render path — no goldens re-blessed, no `run.lrp` re-recorded, no
> exposure for the pending G7 human-gate sitting. It runs beside the game arc.

## Goal

A view dedicated to **asset iteration**: browse every voxel model in the project, look at one live in
a 3D viewport under real game lighting, and improve it with a voxel-specialist Claude persona sitting
in the same window — with light manual editing (palette values, 2D slice painting) for the moments
the human wants the mouse rather than a sentence.

Scored against the North Star: this view is the human-judgment half of a loop the agent already runs
headlessly well. G7's voxel-authoring report is the evidence — six models authored under an
improvement loop that converged in five tooling rounds, ending at *"a competent model lands in 1–2
renders; **color taste is the only loop left**."* Geometry and views are solved for the agent; what
is not solved is a human looking at a value judgment and saying "darker". This view makes that
judgment cost one drag instead of one round trip.

**The friction it retires, precisely.** Every extra render across the whole six-model G7 series was
palette-value calibration, never shape — because `render --model`'s sheet rig is a bright neutral
key+fill while vigil consumes these models at night (torch pools 2.0–2.4, moon fill 0.3, IBL +
bloom). The shipped fix was a *guide paragraph* (engine-guide's palette-calibration paragraph:
author albedo ~2–3× darker than the target read). That paragraph is a workaround for not being able
to see the real thing. This view lets you drag a color while watching the model under the game's own
`env-night.yaml`.

## Decisions summary (from the editor's private feature notebook — the authoritative record)

- **Own stage session, host-spawned.** The view owns a second `ludion serve` managed by the Tauri
  host (lazy on first entry, alive across view switches, graceful `engine.shutdown` then kill on
  exit). Selecting a model = `component.set` of `Mesh.shape.voxel` on the stage's `subject` entity.
  **Rejected:** `scene.load`-swapping the shared game session (violates the recorded edit contract —
  `scene.load` is the one command deliberately left on `ask` because it wipes unsaved human *and*
  agent edits); and a static contact-sheet panel (strictly worse for a human than an orbitable
  viewport, and it is already what the agent does from the CLI).
- **The standing rule is honoured, not spent.** the private voxel-engine notebook: *"File is the editor for
  the authoring phase — no `voxel.*` wire commands until the editor grows a voxel UI."* The editor is
  now growing that UI, and the design still needs **zero** `voxel.*` commands.
- **The stage is committed content, per project** — `games/vigil/scenes/model-stage.yaml`, hosted by
  plain `ludion serve` (built-in components only; no game crate, fast boot, no sim). Content root is
  `scenes/`, so the swap path (`voxels/statue.voxel.yaml`) is byte-identical to a real scene
  reference. **Rejected:** an editor-owned stage (`../../games/...` paths that break when the
  user-configurable project dir moves) and a host-synthesized temp scene (which would make stage
  lighting an editor code change instead of a YAML tweak — and the whole point is that this lighting
  gets tuned by hand, repeatedly).
- **Two rigs, two stage scenes, switched by `scene.load` on the stage session** — the one session
  where `scene.load` can never destroy work, because nothing is ever edited in it. `model-stage.yaml`
  (neutral — shape reading) and `model-stage-night.yaml` (includes vigil's own `env-night.yaml`
  fragment — palette-value judgment under real torchlight). The neutral rig is a **hand-tuned
  approximation** of the sheet rig; see the honest caveat in *Implementation contracts*.
- **Categories = folders.** The browser walks `<content-root>/voxels/**` recursively and the
  directory tree *is* the taxonomy. A derived usage index (model → use sites → category + scale) was
  proposed and **rejected by the user** in favour of plain file organization.
- **`scale` becomes a 7th key in the `.voxel.yaml` format**, backfilled into every existing model
  from its real use site. **Metadata + placement-time application, NOT engine-applied:** the render
  path is untouched, `Transform.scale` keeps exactly one meaning everywhere, and whoever *places* a
  model (this view's stage, the kit shelf, a future map kit) stamps the declared scale. Scenes
  override by authoring a Transform, as they do today. **The doc is authoritative as *intent*; the
  scene is authoritative as *fact*.** **Rejected:** an engine-applied intrinsic scale — it would
  require migrating all 19 vigil models plus every use site (kit entries, enemy docs, map fragments,
  four generated scenes) atomically or everything double-scales, and it would make `Transform.scale:
  1.0` mean two different things depending on shape type.
- **One reloader: the host.** It watches `<content-root>/voxels/**` and auto-fires `asset.reload
  {path}` (debounced ~150 ms). The engine's own docs bless this shape: *"a host can forward
  everything under the asset root and the engine ignores what it does not hold"* — unknown paths are
  a cheap no-op, and voxel reloads are render-only (never hashed, never recorded, re-render while
  paused). The persona is told never to call `asset.reload` and never to `scene.load`.
- **Persona `editor/personas/voxel-artist.md` dials the STAGE session only** (not the game session —
  it may not even be running, and two engines in one context invites confusing them). Its edit
  contract is **asset text files**, not live protocol edits. It gets two kinds of eyes, both already
  built: `render_screenshot` over MCP returns an **image content block** (ludion-mcp's own docs call
  it *"the agent's eyes"*), so it sees the human's live camera and rig; `render --model` sheets give
  the systematic six-angle sweep. The existing spawn pre-approval (`mcp__ludion`, `Bash(ludion:*)`,
  `Bash(cargo run -p ludion-cli:*)`) already covers everything it needs — **no permission change**.
- **Parity of perception is the through-line.** The neutral rig approximates the sheet rig; the
  viewport's preset buttons carry the sheet's own cell labels (`FRONT +Z`, `LEFT`, `TOP Y`, `ISO`,
  `GAME 58`); and the model readout comes from shelling out to `check-scene`. **One checker, the
  engine's** — the TS parse builds the painter's grid only and never judges the model.
- **Viewport:** orbit / pan / dolly via throttled `component.set` on the stage camera; the five
  sheet-named presets; a declared-scale-vs-fill-frame toggle (declared scale stands the model beside
  `adventurer` at its own scale — the size judgment a contact sheet cannot give); a turntable toggle.
  **No gizmo, no hierarchy, no selection, no Play/Edit** — one subject, no sim.
- **Layout** reuses both existing precedents (see *Layout* below).
- **Manual editing ships in the first cut, in inverted order of the obvious:** palette value editing
  (cheapest *and* highest value — it is literally the G7 friction) and 2D slice painting. 3D
  click-to-paint is deferred but **not blocked**.
- **The app's root connect gate is lifted.** The connect screen becomes a per-view precondition
  instead of gating the whole shell, so the editor opens with no game server running and this view
  just works.

## Implementation contracts (hold these fixed)

1. **Every write to a `.voxel.yaml` is a surgical in-place text edit — never a parse → `emit_voxel`
   round trip.** This is the hardest constraint in the arc. `emit_voxel` exists and is canonical
   (byte-pinned fixture + idempotence proptests in `crates/ludion-scene/tests/voxel_roundtrip.rs`),
   but vigil's hand-authored models are **not** in canonical form: they carry a header docstring
   (silhouette intent, front-face convention, intended scale) and a comment above nearly every slice
   (`# y0 — ground contact: hands planted FORWARD …`). Those comments are how an agent re-reads a
   model's intent on the next visit; a round trip would erase all of them and produce a diff the size
   of the file. The format makes surgical editing easy — a slice row is a quoted fixed-width string
   on its own line, so painting cell `(x, z)` in layer `y` is replacing **one character at a known
   line/column offset**.
2. **One checker, the engine's.** The model readout (size, filled count, component count, `L1129`)
   comes from `check-scene` via subprocess, debounced. Do not reimplement connectivity in TS.
3. **The host is the only reloader.** Exactly one component fires `asset.reload`. The persona is
   told so in writing; the painter and palette editor rely on the watcher like any other writer.
4. **The neutral rig is an approximation, and the file says so.** `render --model`'s key+fill is
   synthesized in Rust (`ludion_engine::model_sheet`), not read from a scene, so `model-stage.yaml`
   can only approximate it. Extracting that rig into shared authored data was **rejected**: it would
   put the pinned lantern-sheet golden at risk for a benefit measured in "close enough". The
   approximation is acceptable because the neutral rig's job is *shape* reading, where exactness does
   not matter — and the rig where exactness *does* matter (night, for palette values) is not an
   approximation at all: it includes the game's own `env-night.yaml` by reference.
5. **Wave 0 must not move a single rendered pixel.** `scale` is parsed, validated, emitted, printed,
   and read by placement code — never consumed by the mesher or the render path. The proof is that
   no golden is re-blessed and `run.lrp` is not re-recorded.
6. **The stage never holds unsaved state.** It is a display surface. This is what makes `scene.load`
   safe there and what lets the host restart it freely after a crash.

## Layout

Curated arrangement of shared panels; both halves inherit an existing precedent (Level Designer's
left column + right terminal, Game Designer's centre vertical split with maximize toggle).

```
┌──────────────────────────────────────────────────────────────────────────┐
│ Titlebar · ViewSwitcher: Level Designer │ Game Designer │ Voxel Artist    │
├─────────────────┬──────────────────────────────────────┬─────────────────┤
│ ASSET BROWSER   │  [FRONT][LEFT][TOP][ISO][GAME 58]    │                 │
│ voxels/         │  rig: neutral │ night   scale: 0.24  │    TERMINAL     │
│  ├ enemies/     │                                      │   (voxel-artist │
│  │  ├ statue     │           STAGE VIEWPORT             │    persona,     │
│  │  └ figure   │          (orbit · turntable)         │    dials the    │
│  ├ buildings/   │                                      │    stage)       │
│  └ props/       │                                      │                 │
├─────────────────┼──────────────────────────────────────┤                 │
│ MODEL           │  SLICE PAINTER      y = 4  [◀][▶]    │                 │
│ 9×9×9 · 312 vox │  ....FFF....   ← onion-skin y=3      │                 │
│ 1 component ✓   │  ...FMMMF...                         │                 │
│ scale 0.24      │  ..FMEEEMF..                         │                 │
│ PALETTE ■D ■E   │                                      │                 │
│         ■F ■M   │                                      │                 │
└─────────────────┴──────────────────────────────────────┴─────────────────┘
│ StatusBar · model path · reload ok / L1114 slice row 4 is 8 chars, exp 9  │
└──────────────────────────────────────────────────────────────────────────┘
```

The palette swatches do triple duty: display, the palette *editor* (picker + emissive/roughness), and
the paint-glyph selector for the slice painter.

## Phasing

Each wave is independently useful and independently green (`node scripts/check.mjs`, scoped).

### Wave 0 — the `scale` field (engine, standalone, ships alone)

Touches `crates/ludion-scene/src/voxel.rs` only, plus content backfill and docs.

- `VOXEL_KEYS` 6 → 7 (`format`, `kind`, `name`, `size`, `scale`, `palette`, `slices`); a parse arm
  for `scale`; **shipped as `VoxelDocument.scale: Option<f64>`** (not `f32` as first drafted — an
  `f32` breaks the canonical writer's shortest-round-trip emit). The key list drives the unknown-key
  nearest-suggestion path, so a typo'd `scal:` starts suggesting the real key for free.
- Validation in `validate` (`L1114` family): present ⇒ finite and strictly positive.
- `emit_voxel` writes it in canonical key order; extend the pinned canonical-text fixture and the
  round-trip/idempotence proptests in `crates/ludion-scene/tests/voxel_roundtrip.rs`.
- `check-scene`'s voxel path prints it in the `OK (…)` summary (`crates/ludion-cli/src/check_scene.rs`
  `run_voxel`).
- Guide: a short subsection under *Origin, sizing, and tint* in `docs/engine-guide.md` stating the
  intent/fact split, plus the embedded copy synced byte-identical
  (`crates/ludion-cli/embedded-docs/engine-guide.md`).
- **Backfill**: all 19 `games/vigil/scenes/voxels/*.voxel.yaml` + the 3 gallery models, each with the
  number lifted from its real use site (`scenes/enemies/rusher.yaml` → 0.24; `scenes/city.kit.yaml`
  Transforms for props/buildings; the header comments already record the author's intent and agree).
- **No schema surface**: voxel documents are deliberately not validated against the component
  registry schema, so the schema-drift gate is untouched.

Exit: `check.mjs --rust-only` green; every golden and `run.lrp` byte-identical (state this explicitly
in the commit — it is the safety property).

### Wave 1 — the stages (content)

- `games/vigil/scenes/model-stage.yaml`: ground plane, a `subject` entity (`Mesh.shape.voxel`
  pointing at a default model), an `adventurer` scale-reference entity, a `Camera`, and a neutral
  key+fill approximating the sheet rig — tuned once by eye against `render --model` of the same
  model, with a comment in the file recording that it is an approximation and why (contract 4).
- `games/vigil/scenes/model-stage-night.yaml`: same subject/camera/reference, but consuming the
  shared `env-night.yaml` fragment plus a torch-equivalent `PointLight` at the game's real intensity.
- Both must be servable by plain `ludion serve` — **built-in components only**.

Exit: `ludion check-scene` clean on both; one manual `ludion serve` + browser look at each.

### Wave 2 — host (Tauri Rust, `editor/app/src-tauri/`)

- `list_editor_dir(path, ext)` — recursive listing for the browser tree (the host has
  `read`/`stat`/`write_editor_file` but no directory listing yet).
- Stage-session lifecycle: spawn `cargo run -q --manifest-path <repo>/Cargo.toml -p ludion-cli --
  serve <stage> --port <n>` (the same launch form `spawnConfig` already uses for ludion-mcp, so it
  works whether or not `ludion` is on PATH), parse the printed `listening on ws://…` to confirm,
  emit status events, restart on demand, `engine.shutdown` then kill on app exit. `pty.rs`'s
  waiter-thread pattern already solves owned-child cleanup — follow it.
- Asset watcher: watch `<content-root>/voxels/**`, debounce ~150 ms, forward `asset.reload {path}`
  to the stage session.
- `run_check_scene(path)` — the debounced subprocess behind the model readout (contract 2).
- Root-gate lift in `App.tsx`: the connect screen stops being the app root and becomes a per-view
  precondition; views needing the game session render a connect state in their viewport slot.

Exit: `src-tauri` tests + `check.mjs --frontend-only` green.

**Shipped detail:** colour quantization in the palette editor is 3 *significant* digits, not 3
decimals (the sRGB slope near black makes decimal rounding lossy) — the one quantization rule later
reused by the field-slider widget (`Plan/editor-field-sliders.md`). `readStage` stamps the model
reference's declared `scale` onto its own `Transform` on load, so the declared-scale-vs-fill-frame
toggle can't be lied to when the number moves underneath it.

### Wave 3 — the view (frontend)

- `views.ts` gains `voxel-artist` (enabled); `views/VoxelArtistView.tsx` implements the layout above,
  reusing Game Designer's split component.
- Asset browser (tree build is pure + tested), model panel (readout parse is pure + tested), palette
  swatch display.
- Viewport: orbit/pan/dolly math (pure + tested) → throttled `component.set` on the stage camera; the
  five sheet-named presets; rig toggle (`scene.load` between the two stages); scale toggle; turntable.
- Terminal wiring with `viewId: 'voxel-artist'` and `editor/personas/voxel-artist.md` — the drawer's
  persona resolution is already generic `<personasDir>/<viewId>.md`, so this is a file, not code.
  The persona carries the edit contract (files, not protocol; never `asset.reload`; never
  `scene.load`), the two-eyes technique, the sheet-output convention (`.ludion/sheets/`), and a
  pointer to `ludion docs` topics rather than inlined knowledge.
- Context bridge gains a `voxel` block (selected model path, active rig, camera pose) so "make *this*
  darker" resolves without naming a file.

Exit: TS tests on every pure module; `check.mjs --frontend-only` green.

### Wave 4 — editing

- **The surgical-edit module first, and tested hardest** (`editor/voxel/edit.ts`, pure): set cell,
  set palette color/material, add glyph. Tests must assert that painting one cell changes **exactly
  one character** in the file and that every comment, blank line, and key order survives byte-identical.
- Palette editor (picker + emissive/roughness) — the highest-value half; pair it with the night rig.
- Slice painter: layer navigation, onion-skin of the layer below, glyph selection from the palette
  swatches, local undo stack.
- Conflicts (human painting vs. persona editing the same file) reuse the existing `fileSync` reducer:
  external change on a clean painter silently reloads; on a dirty painter, the reload/keep-mine banner.

Exit: TS tests green; a hand check that a paint + a palette drag both land in the viewport via the
watcher without touching a reload control.

### Wave 5 — close-out, then the folder reorg

- Plan Status, the editor notebook's Done entry, roadmap/CLAUDE.md refresh if the arc warrants it.
- **Then** the vigil `voxels/` reorg into `voxels/enemies/`, `voxels/buildings/`, `voxels/props/`.
  Deliberately last: it rewrites every reference in `city.kit.yaml` and the enemy docs and forces
  regenerating four generated scenes to stay byte-identical under the `map-drift` gate — and the
  tree is better designed after looking at the flat one in the real browser.

## Open items

- **3D click-to-paint** (deferred, not blocked). No per-voxel picking exists on the wire
  (`render.bounds` is entity AABBs only), but the client holds the grid (it parsed the file), the
  camera (`readCameraKit`), and the transform — so a TS DDA march over a ≤48³ grid yields the hit
  cell, and a hover highlight projects to screen with the same math the gizmo overlay already uses.
  **Zero new wire surface.** Trigger: the slice painter proving too indirect in real use.
- **Failed-reload blanking** (accepted). A failed `asset.reload` leaves the cache entry *dropped*
  (`AssetCache::reload`), so a malformed or half-written file blanks the model until the next good
  save. Surfaced as an `L1113`/`L1114` banner — arguably better feedback than a silently stale model.
  Revisit only if mid-save flicker becomes annoying in practice.
- **`scene.check` cannot validate a model document.** Its handler calls `ludion_scene::check` (the
  scene validator) with no `kind:` dispatch, and `asset.reload` returns only the *first* error, so
  the non-fatal `L1129` stray-component warning never reaches the wire at all. Teaching `scene.check`
  to dispatch on `kind:` would be a principled widening with no new method — but its response body
  (`SceneChecked { valid, diagnostics }`) has nowhere to carry the `9×9×9, 312 filled, 1 component`
  summary, so it would not remove the subprocess. Recorded as a real engine gap; not on this arc's
  path.
- **Multi-project stage scaffolding.** A project without a `model-stage.yaml` should get one offered
  rather than an error. Shape of the scaffold (CLI verb vs. editor-side template) is unsettled.
- **Mesh assets** (`*.mesh.yaml`) are out of scope for this view. `render --model` already handles
  them, so a read-only preview is a small later addition if it is ever wanted.

## Current-code facts that shape the work

- `crates/ludion-scene/src/voxel.rs:43` — `VOXEL_KEYS` is a strict 6-key allowlist; unknown keys
  diagnose with a nearest-key suggestion. `VoxelDocument` is at `:84`. There is **no** `JsonSchema`
  derive — voxel docs are intentionally outside the registry-schema path.
- `ludion_scene::emit_voxel` is a real canonical emitter; `crates/ludion-scene/tests/voxel_roundtrip.rs`
  pins its exact text and proves emit-idempotence. It is **not** usable for editing hand-authored
  files (contract 1).
- `crates/ludion-engine/src/commands/asset.rs` — reload is engine-side-filtered ("a host can forward
  everything under the asset root"), render-only, replay-safe, and re-renders while paused because it
  trips `frame_dirty` like a `component.set`.
- `crates/ludion-engine/src/asset_cache.rs:160` — on reload failure the entry is *left dropped*.
- `crates/ludion-mcp/src/server.rs:236` — `render.screenshot` becomes an MCP **image** content block.
- `crates/ludion-assets/src/lib.rs:80` — `resolve` is a plain `root.join`; the content root is the
  served scene file's directory. This is why the stage lives in the game's `scenes/`.
- `editor/app/src-tauri/src/pty.rs` — host command patterns (`read`/`stat`/`write_editor_file`,
  atomic temp+rename, `editor_paths()`), plus the owned-child waiter-thread teardown to copy for the
  stage session. No directory listing exists yet.
- `editor/app/src/App.tsx:~795` — the connect screen is currently the app's root gate (wave 2 lifts it).
- `editor/app/src/views/GameDesignerView.tsx` — the vertical split + maximize toggle to reuse.
- `editor/app/src/editor/gizmo/camera.ts` (`readCameraKit`, `screenToRay`) — camera math already in TS.
- `games/vigil/scenes/enemies/rusher.yaml` carries `model:` + `scale: 0.24`; `scenes/city.kit.yaml`
  carries `voxel:` + Transforms — these are the backfill sources for wave 0.
