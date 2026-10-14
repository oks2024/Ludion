# Plan: Editor view camera — orbit and fly in every editor view, the game camera one key away

> Status: **W1–W4 MECHANIZED-COMPLETE 2026-09-20 (the wire is 30 methods, schema hash
> `be7ebe1877875ed1`) — each wave's exit is a human sitting and is owed, not passed.** Design settled 2026-09-18, every fork ruled by the user
> (two rounds, nine answers). Decisions are marked `[user]` (ruled) or `[proposed]` (this plan's
> recommendation with the alternative it rejects, not contested by the user). Runs under the **editor** feature
> stream (`Plan/editor-views.md`). Current-code facts were read from the tree on
> 2026-09-18; re-grep before editing. The file keeps its first name; the scope outgrew "stage".

## Why

On 2026-09-18 the user tried to judge the ComfyUI-generated textured idol
(`games/vigil/scenes/meshes/idol-generated/`, ≈5k triangles, 1024² baked maps) in the editor's **Level
Designer view** and could not approach it: *"in the editor we don't have a free camera, so I can't look
at the mesh close. From far it looks good."* Two human gates are blocked on that look —
`Plan/comfy-generation.md` checkpoint 2's close look and `Plan/textured-pbr.md`'s exit gate, human
half — and every asset `Plan/mesh-texture-editing.md` produces will need the same look. The user asked
for a **free camera and an orbit camera**, then ruled that it belongs in **every editor view**, with a
return to the **in-game camera at any moment**.

## What exists today (verified)

- **The Level Designer viewport has no camera navigation at all.** `viewer/Viewport.tsx` picks, drags
  gizmos and places the 3D pointer through the scene's own `Camera` entity (`readCameraKit`), and
  nothing moves it. The Art Director, Lighter and Game Designer views share that session and that
  viewport.
- **The model stage already orbits — client-side, over an existing wire command.**
  `editor/app/src/editor/voxel/orbit.ts` is pure math (orbit / pan / multiplicative dolly, the five
  sheet presets lifted from `ModelView::direction_and_up`, a turntable, two framing modes, `lookAtPose`
  reproducing `ViewRig::compute`'s Euler-XYZ decomposition); `StageViewport.tsx` turns pointer events
  into gestures; the view throttles poses onto the stage camera's `Transform` with `component.set` at
  `CAMERA_SEND_MS = 60` (`voxel/stage.ts:207`). `VoxelArtistView.tsx` and `ModelsView.tsx` each carry
  their own copy of that plumbing — W3 deletes the duplication. The Effects stage is
  `interactive={false}`.
- **~~No view puts a `kind: mesh` asset on the stage.~~ W1 changed this.** It was the finding that
  shaped the wave: the Voxel Artist set `Mesh.shape.voxel` and the Sculpt view `Mesh.shape.sculpt`, so a
  generated mesh was reachable only by serving its `scene.yaml` into the Level Designer viewport — where
  the user was standing. The Models view now lists and stages `kind: mesh` documents (see Status).
- **No fly mode anywhere**, no frame-the-subject key, no reset key.
- **`component.set` on the camera is only safe on a stage.** In the main session the scene `Camera` is
  load-bearing: a write dirties a shared session, is captured by `scene.save`, moves `WorldHash`, and
  is overwritten every frame by a game's own camera system (vigil's `follow_camera`).
- **Host render state has a precedent: `render.size`** (the 29th method; `ludion-engine/src/
  render_size.rs`, `commands/render.rs`) — state of the host's renderer, not of the `World`: never
  saved, never hashed, reported back, affecting the stream and `render.screenshot` alike.

## Decisions

1. **The view camera exists in every editor view.** `[user 2026-09-18]` Level Designer, Art Director,
   Lighter, Game Designer, the three stage views — Effects included, which replaces that view's
   fixed-camera decision: the persona's frame is still one key away (decision 2). The Performance
   view's game preview is the game and stays the game camera.
2. **The in-game camera is reachable at any moment.** `[user 2026-09-18]` One toggle — a viewport chip
   reading `View` / `Game` and the `G` key (UE's game-view key) — flips between the view camera and the
   scene's own `Camera`, keeping the view pose for the flip back. **Play mode forces `Game`** (mouse
   coordinates are render-target pixels through the game's camera) and restores the view camera on
   return to Edit. **A view opens on the last view pose left there** `[user 2026-09-18]` — kept
   per view and scene path in `localStorage` (a per-machine convenience, never project data); with none
   stored, on `Game`.
3. **One mechanism for all of it: a host-state view override, `render.view` — a 30th wire method. The
   schema hash moves; the snapshot test, the generated TS types and the MCP adapter follow.**
   `[user 2026-09-18: the 30th method is agreed]` `render.view { pose: { translation, rotation (Euler-XYZ degrees), fov_y_deg? } | null }`
   in the shape of `render.size`: renderer state, not `World` state. While set, the **frame stream**
   goes through the override; `null` returns it to the scene's `Camera`. `render.screenshot` and
   `render.bounds` are per-call (decision 6). It is never written to a scene, never in `WorldHash`, never recorded in a replay, not part
   of `engine.snapshot`; `engine.describe` reports it; `may_change_frame()` is `true`. The parity gate
   is met by construction — any client can set it. *Rejected:* `component.set` on the scene camera
   (dirties, saves, hashes, and loses to `follow_camera` — see above); a spawned editor-camera entity
   (a scene entity that `scene.save` captures, against the 3D-pointer-is-virtual precedent); pure
   client state (the pixels are the engine's).
4. **The stage views move onto `render.view` too, and the `component.set` camera path is deleted.**
   `[proposed]` "Before adding a second way, remove the first": one hook, one verb, one meaning of
   `Game` (on a stage, the committed home pose). *Rejected:* leaving the stages on `component.set` —
   two camera mechanisms in one editor, and `G` would need a second implementation.
5. **Gestures mimic Unreal Engine 5.** `[user 2026-09-18]` Transcribed:
   - **Fly:** hold **RMB** to look; while held, `W A S D` move, `Q`/`E` down/up, `Z`/`C` narrow/widen
     FOV (resets on release), **wheel while held** changes fly speed; `Shift` fast.
   - **Orbit / dolly / pan around the pivot:** `Alt+LMB` orbit, `Alt+RMB` dolly, `Alt+MMB` pan. The
     pivot is the selection's bounds centre (the subject on a stage), else the point ahead of the
     camera.
   - **Plain drags:** `MMB` (or `LMB+RMB`) drag pans; wheel dollies along the view ray.
   - **`F`** frames the selection (the subject, or the selected sculpt part, on a stage); `G` is
     decision 2.
   - **Plain `LMB`:** in the Level Designer viewport it stays what it is — pick, marquee, gizmo drag
     (UE's LMB-drag-to-walk is the one binding not copied; it fights the gizmo). On a stage there is
     nothing to select, so plain `LMB` drag **orbits**, which is what UE's own asset-preview viewports
     do. `[user 2026-09-18]`
   - The five sheet presets stay as toolbar buttons on the stages; `Home` returns to the framed view.
6. **What an agent sees while a human flies: the game camera.** `[user 2026-09-18]` The override moves
   the *stream*, not the query: `render.screenshot` and `render.bounds` gain an optional
   `camera: "scene" | "view"`, **default `"scene"`**, so every existing agent call, test and CLI path
   behaves exactly as today whether or not a human is flying, and an agent never has to touch
   `render.view` (which would yank the human's viewport) to get the game's framing. `"view"` is the
   opt-in: the editor's own picking and gizmo layout pass it, and a persona passes it when the human
   says "look here" — the context file (`.ludion/editor-context.json`) carries `camera: view|game` and
   the pose so it knows there is a "here". `"view"` with no override set is the scene camera, not an
   error. A screenshot already takes the blocking render path, so choosing its camera per call adds no
   second per-frame render. Both parameter additions ride the same schema-hash move as the method.
7. **Camera writes ride the display's cadence, one call in flight.** `[proposed]` A
   `requestAnimationFrame` loop sends the latest pose only when the previous `render.view` has answered
   (latest-wins, the `framePump.ts` shape), replacing the fixed 60 ms throttle — 16 Hz reads as stutter
   under a held key. `render.view` is answered by the host without a trip through the sim queue if the
   `render.size` path allows it (verify in the design pass). `lagProbe.ts` is the instrument; nothing in
   `Plan/editor-viewport-latency.md`'s open phases is a prerequisite.
8. **Meshes join the stage library, and the Sculpt view is renamed Models.** `[user 2026-09-18]` A
   **Meshes** section lists `kind: mesh` documents under the project's `scenes/`; selecting one writes
   the asset's own `Mesh` component (read from the sibling `scene.yaml` node that references it, so
   `metallic: 1.0` arrives — the comfy log's wave-2 finding 10) onto `subject`. Multi-node imports show
   their first mesh node and say so. The persona file and view id follow the rename.
9. **Determinism and goldens do not move.** `[proposed]` The override is unset in every CLI, test and
   replay path; `WorldHash` is identical with it set or not (a test says so); a frame rendered after
   `render.view null` is byte-identical to one rendered before it was ever set (a test says so). The
   model sheet's fixed views are untouched — this plan no longer touches `model_sheet.rs` at all.
10. **The CLI pose (`render --model --orbit`) waits.** `[user 2026-09-18]` See Deferred.

## Waves

- **W1 — a mesh on the stage; Sculpt becomes Models.** Decision 8, on the orbit that already ships.
  Editor-only, wire stays 29. **Mechanized-complete 2026-09-18 — see Status below.** Its **exit** is
  the user opening the generated idol in the Models view, getting close, and giving the two owed
  verdicts (comfy checkpoint 2's close look; the textured-PBR human half). Ships alone — the gates do
  not wait for the rest.
- **W2 — `render.view`, engine-side.** Decisions 3 and 9: the command, the ≤10-line dispatch arm, the
  handler beside `render.size`'s, the stream reading the override, the `camera` parameter on
  `render.screenshot` and `render.bounds` (decision 6, default `"scene"`), its `L####`
  for a malformed pose, `engine.describe`, `may_change_frame`, the schema snapshot re-blessed (hash
  named in the commit), TS types regenerated, `docs/` + `dev-docs/` topics, a runnable example, the two
  determinism tests. **Exit:** from a shell, with `render.view` set, a plain `render.screenshot` is
  byte-identical to one taken before it was set, `camera: "view"` shows the new pose, a stream
  subscriber sees the new pose, `null` restores the stream byte-identically, and `sim.hash` never
  moved.
- **W3 — one camera hook, UE gestures, on the stages.** Decisions 4, 5, 7: `useViewCamera` over pure
  `orbit.ts` + a new pure `fly.ts` (pose + held keys + dt → pose), the rAF send loop, pointer lock,
  `F` / `Home` / `G` and the chip, fly→orbit hand-back without a jump; both views' copies and the
  `component.set` camera path deleted; the Effects stage goes interactive. **Exit:** the user flies
  around the idol, releases, Alt-orbits from where the fly ended, presses `G` and is home.
- **W4 — the main session.** The same hook in `viewer/Viewport.tsx`: the `CameraKit` that picking,
  gizmos and the 3D pointer project through comes from the override pose when `View` is active; Play
  forces `Game`; the context file fields (decision 6); the personas' one paragraph. **Exit:** in the
  Level Designer on a vigil district the user flies to a rooftop, selects and drags a prop with the
  gizmo landing where the cursor is, presses `G` and sees the follow camera untouched; `scene.save`
  writes no camera change and the session is not dirtied by looking.

## Engine / game boundary

W1, W3, W4 are TypeScript under `editor/app/src` (plus the persona files). W2 is the one engine wave:
`ludion-protocol` (the command), `ludion-engine` (handler, renderer camera selection, bounds),
`ludion-core` (the code), docs. Nothing in `games/` changes; a game never sees the override — its
camera entity and every system that reads it are untouched. Nothing platform-shaped.

## Deferred

- **`render --model --orbit <az>,<el>[,<dist>…]` and the copyable pose** `[user: wait]` — the agent's
  reproducible free view, also the comfy log's `--yaw` answer. When built it goes **through the public
  `model_camera(view, min, max, cell_w, cell_h)`** that `Plan/mesh-texture-editing.md` Wave 1 publishes
  (a pose beside `ModelView`, so `the_public_camera_is_the_sheet_camera` keeps meaning something), and
  `ludion mesh pick` takes the same value through the same call. *Trigger:* an agent needs a view the
  seven fixed cells cannot give, or mesh pick needs an off-axis camera.
- **Pivot-on-click / zoom-to-cursor** — needs depth under the pointer; the wire has rects, not depth.
  *Trigger:* `ludion mesh pick`'s CPU ray cast is host-callable, or pan-then-dolly proves not enough.
- **UE's LMB-drag walk, camera bookmarks (`Ctrl+1…`), orthographic views, a view cube, camera-speed
  UI, gamepad / 3D-mouse.** *Trigger:* a sitting asks.
- **`render --model --rig`** — the comfy arc's call.

## Settled questions (the user, 2026-09-18)

Round one: the look was in the Level Designer view; the camera belongs in every editor view with the
in-game camera reachable at any moment; gestures mimic Unreal Engine 5; meshes join the stage library
and the Sculpt view becomes Models; the CLI orbit pose waits. Round two: an agent’s screenshot follows
the **game** camera (decision 6); plain left-drag keeps orbiting on the stages (decision 5); a view
opens on the last view pose left there (decision 2); the wire grows to 30 methods (decision 3). No fork
is open.

## Status

### W2–W4 — `render.view`, the camera hook, the main session (mechanized-complete 2026-09-20)

- **W2, the engine wave.** `render.view` is the 30th method; the schema hash moved
  `73b28b8a495cd4b5` → `be7ebe1877875ed1`, and the generated TS types followed. The override is one
  `Option<ViewPose>` on the `Engine` (`crates/ludion-engine/src/render_view.rs`, beside
  `render_size.rs`): `{}` reads it, `{"pose": null}` clears it, a non-finite pose or a `fov_y_deg`
  outside `0..180` is `L3032` and leaves it untouched. The pose carries geometry only — clip planes,
  and an unnamed FOV, come from the scene's `Camera`; the override is always perspective, so the
  orthographic pixel snap is skipped while it is in force. `render.screenshot` and `render.bounds`
  take `camera: "scene" | "view"`, default `"scene"`; the SDK's screenshot pins `Scene`. Decision 9's
  two determinism tests are `crates/ludion-engine/tests/render_view.rs`; the example is
  `crates/ludion-engine/examples/render_view.rs`; `docs/wire-protocol.md` §6 and
  `ludion devdocs engine-present` are the topics. No golden moved. The committed `.lrp` corpus
  (breakout's, vigil's `run`, `stress` and the five segments) was re-recorded because a replay is
  locked to the schema hash (`L3010`) — the scripts and the simulation are unchanged.
- **W3, the hook.** `editor/app/src/editor/camera/`: pure `fly.ts` and `gestures.ts` under
  `useViewCamera.ts` (the rAF latest-wins send loop, pointer lock, `F` / `Home` / `G`). Models, Voxel
  Artist and Effects are wired through it, and the `component.set` camera path is deleted.
- **W4, the main session.** `viewer/Viewport.tsx` builds the `CameraKit` for picking, gizmos and the 3D
  pointer from the flown pose while `View` is active and passes `camera: "view"` on its three
  `render.bounds` calls; Play forces `Game`; the chip; the context file's camera block; the four
  main-session personas' paragraph.
- **A roll-free pose is a level horizon, not `rotation[2] === 0`** — the first flight's finding
  (the user, 2026-09-20: mouse-look rolled the camera). Under `Rx·Ry·Rz` the X angle turns about
  *world* X, so stepping Euler X/Y as pitch/yaw rolls any yawed camera. `lookDrag` now reads yaw and
  pitch off the view ray and emits through `lookAtPose` with world up, the orbit rig's own path;
  `fly.test.ts` pins the horizon (the camera's right axis has no world-Y component) and that a
  vertical drag leaves the heading alone. The user confirmed the fix in the running editor.

**Owed — the sittings.** The user has flown it and the look is right; the waves' falsifiable exits
(W3's fly → Alt-orbit → `G` hand-back; W4's gizmo landing under the cursor from a flown pose, and
`scene.save` writing no camera change) are the checklist in `Plan/editor-live-smoke.md`.

### W1 — a mesh on the stage; Sculpt becomes Models (mechanized-complete 2026-09-18)

Editor-only TypeScript. **The wire stayed at 29 methods, no crate was touched, no golden moved**, and
the stage rides the orbit that already shipped.

- **The Meshes library.** A second `AssetBrowser` beside the sculpts, rooted at the project's
  `scenes/` and listing `.mesh.yaml`. The two browsers share one selection (`StageModel =
  { kind: 'sculpt' | 'mesh', relPath }`) because the stage holds exactly one subject; a selection
  survives a view switch in `localStorage` under the pre-rename key, so no stored model was orphaned.
- **The lift.** `editor/app/src/editor/mesh/` is two pure modules: `stage.ts` (roots and the asset
  reference) and `lift.ts` (candidate scenes, and the node that places a mesh). Selecting a mesh reads
  the nearest ancestor scene that references it — **never the library root**, where a project's whole
  district set lives — and writes that node's *whole* `Mesh` component onto `subject`, so the
  importer's authored `metallic: 1.0` and the lit variant's `emissive: 3.0` arrive instead of the
  component's defaults. It also stamps the node's `Transform.scale`, without which a mesh would arrive
  at the stage subject's committed `0.24` voxel placeholder — a quarter size, and no close look.
- **The asset reference is the listing path verbatim, and that is a finding, not a convention copy.**
  `Mesh.shape.asset` resolves against the **content root**, which is the loaded scene file's parent
  directory (`Engine::load_scene_file` takes `resolved.parent()` in
  `crates/ludion-engine/src/builder.rs`; `ludion_assets`' resolve is a plain `root.join` in
  `crates/ludion-assets/src/lib.rs`). The stage scene is `<projectDir>/scenes/model-stage.yaml`, so the
  content root **is** the Meshes library root — unlike the sculpt and voxel libraries, which list a
  subdirectory and must prepend it. Inside the mesh's own `scene.yaml` the reference is relative to
  *that* scene's directory, so the lift resolves before comparing; two cross-negative tests pin it (the
  pristine scene must not match the lit mesh, nor the lit scene the pristine one).
- **What stands on the stage is a function of the current selection and the committed scene, never of
  the click history.** The subject's `Mesh` and `Transform` are captured once when the session dials —
  the `cameraBaseRef` idiom — and both apply paths build over that baseline. Without it each selection
  merged over whatever the previous click left, so a lit mesh's `emissive: 3.0` followed the next
  sculpt onto the stage: a stale material silently carried into the very verdict this wave exists for.
  A sculpt selection now also restores the baseline scale, which upholds the view's "the scene's
  placement is the only scale there is" invariant rather than breaking it.
- **Empty directories are pruned on the mesh path only.** `list_editor_dir` always emits directories,
  so a `.mesh.yaml` scan of a project's `scenes/` yielded 19 folders for 2 meshes. `AssetBrowser` gained
  an opt-in `prune` prop defaulting to `false`; the voxel and sculpt browsers are behaviourally
  identical to before.
- **The rename.** View id `sculpt` to `models`, label "Sculpt" to "Models", `views/SculptView.tsx` to
  `views/ModelsView.tsx`, and `editor/personas/sculpt.md` to `editor/personas/models.md` — the last is
  load-bearing, not cosmetic: `TerminalDrawer` reads the persona as `<personasDir>/<viewId>.md`, so a
  stale filename would have broken the drawer for this view. The `kind: sculpt` document format, the
  `scenes/sculpts/` tree, `editor/app/src/editor/sculpt/` and the `ludion sculpt` CLI keep their names —
  the document kind was not renamed.
- **`MIN_ORBIT_DISTANCE` was left at 0.1, and that is measured, not assumed.** The idol is
  1.00 x 0.84 x 0.61 world units at the stamped scale; the glowing eye mark is about 0.064 across. At
  0.1 the frame is 0.083 world units tall, so the mark fills roughly 77% of viewport height. Going
  tighter is **blocked, not merely unnecessary**: the framing that would fill the frame with the 0.020
  painted cross-section is 0.024, inside the stage camera's `near: 0.05`, and even 0.078 clears near by
  less than the idol's own surface depth across the eye. Lowering the constant would buy a clipped
  frame. **If the sitting wants closer, it is a stage-camera `near` decision, above this wave.**

**Owed — the human sitting (the wave's exit).** Click path: open the editor on `games/vigil`, then the
**Models** view, the **Meshes** section, `meshes/idol-generated/meshes/mesh-0.mesh.yaml`; orbit, pan
and dolly in to the eye. Then the lit sibling,
`meshes/idol-generated/meshes/idol-lit/mesh.mesh.yaml`. It unblocks two verdicts that have been waiting
on nothing else: **`Plan/comfy-generation.md` checkpoint 2's close look** (the generated idol beside the
sculpt idol) and **`Plan/textured-pbr.md`'s exit gate, human half** (the look at the fire-triggering
ComfyUI asset on the model stage).

**Known and left, each a ruling the sitting may overturn:**
- The Meshes browser passes no `reloadKey`, so a mesh added on disk needs a view switch to appear.
  There is no mesh edit session to bump it from; the host's stage watcher covers `voxels/` and
  `sculpts/`, not `meshes/`.
- A mesh no scene places falls back to the stage's committed material at unit scale, and says so in a
  notice. Unit scale, not the committed `0.24`: a mesh document's model space is world units, which is
  what the importer's own scene writes.
- A multi-node import shows its first mesh node and says so, per decision 8.
- The per-selection `listEditorDir` walk is deliberate — reusing the browser's listing would mean
  threading its state back out through `AssetBrowser`, and a content root is tens of directories on a
  click.
