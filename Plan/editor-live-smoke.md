# Editor live-smoke checklists — the queued Tauri sitting

> Status: **OPEN — nothing here has been exercised.** The frontend gates (`check.mjs --frontend-only`)
> cannot render React, so every view shipped since the editor arc began has verified only mechanically
> (pure-module tests, parity/schema gates) and accumulated a real-GUI checklist here instead. This file
> is that queue: one section per view/arc, each block a falsifiable checklist a human runs once, live,
> in the Tauri app. **A block is deleted from this file once its sitting passes** — the editor feature notebook's
> header rules apply here too (outcomes only; a passed checklist is removed, not annotated DONE).
> Moved here verbatim from the editor feature notebook's TODO section (which had grown past its 8 KB budget)
> on 2026-09-18; no content was altered in the move.

## Open-project live smoke

(`Plan/editor-open-project.md` W1–W4; unexercised): `node scripts/editor.mjs <project>` opens the editor
with the project's name in the top bar. The title-bar project name opens a popover with recents (current
marked), **Open folder…** and a path field; pasting the checkout itself refuses inline ("…has no
ludion.toml…") without closing the popover; **Open folder…** to <project> switches the name and stops/
restarts any running Voxel Artist/Effects stage on the project's scenes; a terminal open before the switch
survives, its tab still shows the old project's name, and a new terminal's tab shows the project's; a
renamed/moved project directory greys "not found" in recents; the drawer's "project:" line opens the
switcher and no `ProjectConfig` field exists anywhere. Opening a valid project cold shows the connect
panel's game card as "Starting `<name>`…" while cargo builds it, then auto-attaches once it listens
with no manual Connect; breaking the project's code and clicking Run game turns the card to an error
tone with the rustc tail; starting `ludion serve` on 9411 by hand first offers Attach/Replace naming
the engine version (Attach connects as-is, Replace shuts it down and starts the project's own game);
a plain TCP listener on the port offers Attach only, no Replace; switching project while attached
drops the session with no schema-mismatch banner and re-attaches to the new project's game once it
lists; `ludion dev`'s hot-reload blink still reconnects the same session unchanged. The Scenes button
next to Save names the current scene once a load/save has happened (blank on a fresh attach); opening
it lists every scene under `<project>/scenes` recursively with voxel/map/part/anim documents absent
and stage/generated scenes tagged; picking a clean scene loads it without restarting the game process;
with unsaved edits it asks "Discard unsaved changes to `<path>`?" (No keeps the dirty scene, Yes loads
and clears undo/selection); attaching by URL to a `ludion serve` of a different project still resolves
picker loads by content-root-relative path, with a missing file surfacing as a banner diagnostic, not
a crash.

**W4:** on a project without stage scenes, the Voxel Artist and Effects views show the missing-scene
card with **Add stage scenes**; clicking it writes only the missing files and starts the stage; a
hand-edited `model-stage.yaml` survives a second click (never overwritten).

**Exit gate (`Plan/editor-open-project.md`):**
1. From a cold start, `node scripts/editor.mjs "<project>"` opens the editor on the project with its game
   running in the viewport.
2. The project's scenes switch from the picker.
3. vigil ↔ the project switches from the top bar without touching a terminal.
4. The Voxel Artist and Effects stages start on the project after **Add stage scenes**.

## Input-lag live smoke

(joins the next Tauri sitting): the offscreen path stays off unless someone watches a gizmo still land on an object after an fps-cap change with it on; a held key should no longer show a growing gap between press and motion; `GizmoOverlay` should no longer re-render at 4 Hz with an entity selected (devtools profiler).

## Lighter live smoke

(joins the next Tauri sitting; the whole tab is unverified in the GUI, same as every view before it). Serve `examples/gallery/gi-interior.yaml` → Lighter: chip reads `sky + bounce`; drag `sky_visibility` moves the frame during the drag, one Ctrl+Z returns it; GI A/B hold: the interior darkening vanishes, release restores byte-exact, session stays undirtied; Bounce A/B: exactly two visible hitches (press and release), never more; a `cell_size` stop press: one ~300 ms stall, one undo entry; sun `intensity` drag relights live with the bounce term following; click a torch in the lights list → selection lands, PointLight card appears, its slider relights live. Then a vigil district (authors no GiVolume): chip `off`, dimmed shipped defaults, no A/B buttons; "Enable GI" materializes `{}` onto the env-night Environment carrier and nothing else moves; a single field edit on a fresh scene materializes just that field. Persona boots in the drawer, runs both dumps on gi-interior, and on a district reports the L2104 trap instead of hitting it. **Judge the three derived travels in the hand** (`normal_bias` `[0,2]`, sun `intensity` `[0,4]`, `shadow_bias` `[0,0.01]`) — each is a one-line edit if wrong, as `dof` proved. Also: the step ladders' ends (`cell_size` 0.125 on a district may threaten L2103 — the diagnostic should surface in the error banner, worth seeing once).

## Effects view live smoke

(joins the next Tauri sitting; the whole view is GUI-unverified, as every view before it): open the view on a vigil project dir → the effects stage (9413) builds and dials while the model stage stays up; 19 rows appear as sliders / swatches / dropdowns with no raw-JSON row; drag `rate` while running and the plume follows the drag; set `burst: 200, rate: 0`, Restart fires it once; Pause → a still, Step advances one frame, speed ×0.25 slows visibly; loop re-fires the burst on its period; the persona boots in the drawer and `render_screenshot` returns the stage; then place the `Particles` archetype in a level from the Level Designer palette. **Judge the six new travels in the hand** (`extent` 4, `burst` 512, `duration` 10, `life` 5, `speed` 20, `floor` ±4) — each a one-line edit if wrong, as `dof` proved.

## Pattern-tab live smoke

(the same sitting): the `Pattern` tab is enabled and switching to it keeps the time strip in place with `loop` greyed (a document carries its own `loop:`); the picker lists only `kind: pattern` files under `<project>/scenes` (a district scene must NOT appear); `New` writes `patterns/<slug>.yaml` and opens it; the form shows `bullets` as a keyed map and `timeline`/`motion` as ordered lists with **no raw-JSON row anywhere**, `blend`/`shape`/`center` as dropdowns, `ramp`/`turn`/`speed_range` as checkboxes that reveal their fields; **one committed field writes the file and the note reads the engine's own `asset.reload` answer** — edit `speed`, watch the file's mtime move and the volleys re-shape on the next frame; Restart re-stamps `PatternPlayer.started` (the ring fires from frame 0 again); the aim `x`/`z` fields move the emissive marker and an `arc { center: aim }` volley follows it. **The falsifiable claim: open a file with a header comment, edit one number, and every comment plus the key order comes back byte-identical apart from that number** — if a comment vanishes, the document API is not being used as intended. Also: the plain stage registers the `pattern` kind and prints one `L1155` per vigil doc it skips (the stage log should show them, and volleys must play); against a host without the kind the bundled-schema notice must appear and saving must still work; and leaving the tab must clear `PatternPlayer.pattern` (the stage stops firing).

## Voxel-artist scale smoke

(joins the next Tauri sitting): select house-c and idle — a devtools profile shows NO periodic `SlicePainterPanel` render at the 4 Hz status cadence; orbit drag and turntable leave the span grid unrebuilt; a paint drag on house-c holds frame rate; one Ctrl+Z returns a whole stroke byte-exactly; collapsing the painter via the maximize toggle should NOT change idle cost (`VerticalSplit` keeps it mounted — a confirming observation, per the diagnosis's own falsifiable check).

## Game-preview live smoke

(joins the next Tauri sitting): the top-left chip reads `1280×720` while the box is ~320×180 CSS px — if it ever reads the CSS size the honesty mechanism is wrong; **the falsifiable claim: capture 300 frames with the preview off, then 300 with it on, and every GPU pass median plus `frame_total_median_ms` should agree within noise, the only structural difference being `transport` appearing** (honest caveat: encode runs on `spawn_blocking`, so on a core-saturated box the sim can see a second-order scheduling effect — GPU per-pass is structurally unaffected, `frame_total` is not guaranteed to be); Transport group/lane populate within ~1 window of toggling on and go absent again after toggling off; the toggle survives a view switch away and back; Play mode drives vigil with WASD + mouse from the box and the timeline reacts to a horde spawn; leaving the box mid-hold and flipping Play→Edit mid-hold both release the button; Edit mode forwards nothing; a 600-frame Capture still runs on its own socket while the preview keeps streaming on App's client.

## Voxel-artist assemblies live smoke

(joins the next Tauri sitting; can carry the owed voxel-animation player verdicts —
`Plan/voxel-animation.md` Status — player at `scale: 0.125`, `LANTERN_HEIGHT`, attack-latch tuning, in
the same session): the browser lists `.assembly.yaml` files beside voxel models with a kind badge, and
`parts/`/`anims/` never appear as their own folders; opening vigil's `player.assembly.yaml` stands it
at a sensible size under both rigs, with the model panel naming `0.0625` as the scale it used; no
palette editor or paint affordance is reachable anywhere in the panel or the slice grid; selecting
`lower-arm-l` in the parts panel shows its grid read-only under the assembly's palette, the pivot
crosshair sits exactly on a cell boundary where the pivot is `.5`, a `mirror x` part shows its badge,
and the camera frames the part alone; the "whole model" control re-frames back to the subject. Play
`walk`, pause it, scrub to `t 0.45 (tick 27)` and the knee sits at its flex peak; the arrow keys step
one tick at a time; speed ×0.25 visibly slows the motion while the tick label stays honest about which
tick is posed; `attack` (a one-shot) holds its last pose when it runs out, and the loop toggle
restarts it instead. Switching rig mid-clip keeps the clip and the tick; selecting a plain voxel model
afterwards leaves it un-animated AND the sim running at ×1 (never frozen by a leftover scrub pause).
Editing a keyframe's `t` in a text editor re-poses the stage with no restart, and an off-tick `t` shows
flagged on the bar. Last: the context file's `voxel.assembly` block carries the assembly path, the
open part, the clip and the tick/seconds only while paused (`playing: true` and no tick while `walk`
runs).

## View camera live smoke

(`Plan/editor-stage-camera.md` W2–W4; the user's first flight on 2026-09-20 found and confirmed the
fix for one defect — mouse-look rolled the horizon — and everything below is still unexercised.)
**On a stage** (Models, the generated idol): plain `LMB` drag orbits; hold `RMB` and look in a full
circle with the pitch near both limits — the horizon stays level at every heading; `W A S D` / `Q` /
`E` move while held, the wheel changes fly speed, `Shift` is fast, `Z`/`C` narrow and widen the FOV
and it snaps back on release; release `RMB`, `Alt+LMB` orbits **from where the fly ended with no
jump**; `F` frames the subject, `Home` returns to the framed view, the five preset buttons still land
on the sheet's cells; `G` flips the chip to `Game` and the committed home pose, `G` again returns to
the flown pose. The Effects stage is interactive too. **In the Level Designer** on a vigil district:
fly to a rooftop, select a prop and drag it with the gizmo — **the falsifiable claim: the gizmo and
the pick land under the cursor from the flown pose, and `P` places the 3D pointer where the cursor
is**; `G` shows vigil's follow camera untouched; `scene.save` writes no camera change and looking
never dirties the session; entering Play forces `Game` and returning to Edit restores the flown pose;
leaving the view and coming back reopens on the last pose left there. **The agent half:** while the
human is flown, a persona's plain `render_screenshot` shows the *game* framing, `camera: "view"` shows
the human's, and `.ludion/editor-context.json` carries `camera: view|game` and the pose.

## Performance view live smoke

(joins the next Tauri sitting; nothing here renders under the gates): windows arrive ~1 Hz and the timeline scrolls; the ring fills toward 3 min and old columns fall off; leaving the tab unsubscribes and the engine stops paying (check `perf_armed` via a capture or logs); reconnect (`ludion dev` blink) re-subscribes; the capture button stays responsive while the shared session keeps polling (it runs on its own socket — a 600-frame capture is the real test); Save Perfetto → opens at ui.perfetto.dev with four labelled lanes and no overlap warnings; Save JSON; paused sim shows the "no new windows" note; hover readout/legend toggles/lane switch; whether the hero number reads at 30px; the `transport` group appears once a viewer is streaming frames; the `performance` persona boots in the drawer.

## Character clip preview live smoke

(`Plan/animation-clips.md` W3; nothing here renders under the gates.) **Setup:** a scratch project
(`ludion new <scratch>` outside the repo) with `examples/gallery/{characters,clips}` and
`meshes/skin-mannequin.mesh.yaml` copied under its `scenes/`. Open it, go to Models. **Characters lists
`mannequin`**; selecting it stands the mannequin at rest and the strip lists idle/jump/run/spin. **`run`
plays at 1.2×** — the bar advances faster than the tick label's sim frames. **Pause freezes**; dragging the
bar scrubs both ways, including the deep end right after a stage restart. **Run → jump crossfade at
0.3 s** blends visibly and the band shows on the bar; **Restart replays the blend**; scrubbing inside
the band poses intermediate blends. Picking a mesh then removes the animation with no error banner.
**The Voxel Artist's assembly strip still plays vigil's player walk.**

## Transform hierarchy & prefabs (W4) live smoke

(`Plan/transform-hierarchy-and-prefabs.md` W4; nothing here renders under the gates.) **In the Level
Designer** on a vigil district with a scratch prefab (`crate.prefab.yaml`, two parts) under the content
root. **Reparent:** drag an outliner row onto another — **the falsifiable claim: the entity does not jump**
(keep-world), Ctrl+Z puts it back at the exact old local pose and Ctrl+Shift+Z re-parents it, bit-exact
both ways; the gizmo on a child sits at the child's *world* pose, and dragging it edits the child's local
`Transform` (the inspector value moves, the parent's does not). **W1 carried:** destroy a parented entity,
Ctrl+Z — the whole subtree returns with its `Parent` links. **Instances:** an instance's parts group under
one collapsible header; collapsing hides them and expanding restores them. **Apply** asks for confirmation,
writes the prefab file, and leaves nothing on the undo stack. **Revert**, **revert part** and **unpack** each
undo with one Ctrl+Z. **Override markers:** a part whose value was changed shows a marker, which clears after
revert. **The per-field list:** selecting an overridden part lists its overridden pointers, each with a ↺
that reverts only that field (one Ctrl+Z undoes it); a component the prefab lacks, added to a part, lists
as an override and its ↺ removes it; a top-level part's `/Transform…` pointer is listed with no ↺, and its
tooltip says a root's placement is not reverted. **Header flags:** moving an instance's root shows ◆ on its
header, removing a part shows −N, and parenting a plain entity under a non-first part shows ! (L1234 in the
tooltip); each clears once undone. **Gizmo under a sheared parent:** on a child of a rotated, non-uniformly
scaled parent, the gizmo sits on the child's world pose and dragging it moves the child along the axis
dragged, with no jump on release. **Place by drag:** the Prefabs panel lists only `.prefab.yaml` files, searchable; dragging one onto
the ground, and onto a prop's top, lands the instance under the cursor, from a flown view camera too; a
drop off the canvas, or on the sky, places nothing and Escape cancels the drag. The instance is named
`crate`, a second drop `crate-2`. **One Ctrl+Z removes every part**, Ctrl+Shift+Z restores them
(an apply after the redo is the known-limit check: the redone instance has no placement record). **Place →
Ctrl+Z → Ctrl+Shift+Z → save writes exactly one instance entry**, never two (the redone root keeps a live
instance link). `scene.save` writes an instance entry, not loose parts. **Models view:** staging the generated idol shows it under
`metallic: 1` with no "No scene or prefab places this mesh" note.

## Generated-scene Save gate (audit Group 3 X9)

In the Level Designer, with a vigil project. **Load a `generated_from:` scene from the picker** (e.g. the town square): the Save button is disabled and its tooltip names the generator. **Ctrl+S writes nothing** — the file mtime is unchanged. **Load a hand-authored scene**: Save is enabled, and Save and Ctrl+S write it. **Race:** pick a generated scene and press Ctrl+S immediately, before the tooltip settles — the file mtime is unchanged and Save shows "checking the scene…" until the read resolves.


## Diagnostic banner (audit Group 4 L6)

In the editor, a multi-error `scene.load` (a scene with two bad fields) shows the first diagnostic with a collapsed `N related` that expands to the rest, and the message line repeats neither the code nor the help. A sim-side warning (a hot-reloaded data doc with a bad field) shows a code chip, the raw message and the help.

## Schema-hash drift banner and settled connects (audit Group 5 W9, X5, X6)

In the editor, attach to a game built from an older or newer engine (or hand-edit a local `schemaHash.ts` without committing it): an **L3003 banner names both hashes**, has **no dismiss button**, and the session stays usable. A `ludion dev` blink with a matching build shows **no banner**. Kill a game mid-handshake: the reconnect resumes its backoff and **never sticks on "Reconnecting…"**.

## Restore resets the editor and reload status (audit Group 6 X3, D2)

In the editor, with a vigil project under `ludion dev`. **Restore resets undo:** move an entity, then have the drawer's agent call `engine_restore` with an earlier snapshot. The agent **asks before restoring** (Claude and opencode), and after the restore the selection is cleared and **Ctrl+Z does nothing**: no `component.set` reaches the restored world. **A game scene switch** (enter a district gate) also clears undo and selection. **Crash chip:** make a system loop forever behind a flag and save: the code panel's chip shows **crashed** with a tooltip saying the session state was lost and why, then returns to ok once the fresh game serves.

## Gameplay-frame stamps (audit Group 12 K2)

A regression check: the Effects and Models stages are bare `ludion serve` sessions that never hold, so `game_frame` equals `frame` there (the held case is pinned by the server test). **Effects view:** Restart still fires a burst at once, and the loop re-fires on its period. **Pattern tab:** Restart fires the pattern from frame 0. **Models clip preview:** Restart, scrub and crossfade behave as before. Falsifiable claim: **no stamp lands in the future** (no effect waits for a frame that has not come, none starts already aged).

## Editor host: dev port, CSP, stage slots, guards (audit Group 15 X1, X2, X4, X7, X8, X11, X12)

**Dev port and CSP (X1).** `node scripts/editor.mjs <project>`: the window loads with **no CSP violation** in the devtools console; the viewport streams, the terminal drawer opens and renders, a generated thumbnail renders, CodeMirror is styled. Attach the session strip to a LAN engine (`ludion serve --addr 0.0.0.0:<port>` on another box): it connects. With port 1420 already taken, a second `tauri dev` **fails loudly** (no hop to 1421). A packaged build (`tauri build`) loads and is styled: the style-nonce opt-out is exercised only there.

**Stage slots (X2, X4).** The first open of the Voxel Artist, Models and Effects views in the dev build (StrictMode double mount) **spawns one stage engine** each (one `ludion` process per port). Switch project during the game's first cold build: the **old project's game never appears**. Close the editor during a cold build, relaunch: **no Busy slot**, no engine left behind. A model stage started for project A is not handed to project B after a switch. A cold start shows Starting until Listening; a game compile error shows Failed with rustc's error.

**Stage port identity (X8).** A `tauri dev` editor and a packaged one, the Voxel Artist in both: the **second shows Busy** and the first keeps its stage. Kill an editor's process while its stage runs, open the stage in a fresh editor: the **orphan is reclaimed**. Start `ludion serve --addr 127.0.0.1:9412` by hand, open the Voxel Artist: **Busy**, and the manual server survives.

**Undo refs (X11).** A child with a `PatternPlayer` aimed at its parent: destroy the parent, undo: the child's `target` is the **parent's new id** and the turret aims.

**Generation guards (X7).** Generate on recipe A, click recipe B: Generate **stays disabled** with the running timer until A finishes; A's candidates **never show under B**; Check on B works; back on A, the new run is listed newest.

**Model readout (X12).** Voxel Artist: the readout shows size, filled count and components (it was blank before r1260); an `L1129` model shows the diagnostic's first line.

## Generation view failure paths (audit Group 16 C2, C5)

Needs a live ComfyUI; never beside another instance's golden tests.

**Cached resubmit (C2).** `ludion gen run <recipe>` twice with no edit: the second, fully cached run **finishes in seconds** with every candidate downloaded, not `L1170` after `--timeout`. The editor's Generate twice behaves the same.

**ComfyUI down.** Generate shows `L1167` as a diagnostic row (no stderr wall), and no run is selected.

**Timed-out run.** Generate with timeout 1 on a real recipe: the run **stays selected** as `submitted` with `L1170` beside the empty grid; Status reports the pending ids; Fetch before every job finishes says so; after they finish, Fetch fills the grid, the run turns `ok`, and Fetch/Status disable. The timeout field marks non-numbers and values over a week as invalid and disables Generate.

**Partial run.** A run with one failing job (e.g. an out-of-memory seed): the grid shows the finished candidates, a `job N` row with node, class and message, and `L1169`; the run reads `partial`, and Accept on a finished candidate works.

**Seeds.** A recipe whose seed comes from a linked or primitive node: cells read `#i seed ?`, matching `gen sheet`; a pinned seed shows the pinned value.

**Accept refusal.** Accept onto an existing path shows `L1172` as a diagnostic row.

**Cancel.** Generate on a real recipe, then press Cancel during the build: the Cancel button disappears, the run bar returns to idle, stderr ends with "the editor cancelled `ludion gen run`; jobs it already queued on the ComfyUI server are not cancelled there", and Task Manager shows no `cargo` or `ludion` left. Cancel a Fetch the same way.

## Stream subscriptions, reconnect, file sync, gen children (audit remediation 2026-10-10)

Under StrictMode `tauri dev`, the engine's `stream.subscribe` count shows **one `frames` sub per connection**, not two; changing max FPS or arming a probe does not grow it. **Press Disconnect during a reconnect blink**: the editor stays disconnected and never revives the old project's client. Switch projects mid-handshake: the new project's card connects, the old one never reappears. If a game is already listening when the editor first mounts, the auto-attach still reaches "connected" (a StrictMode remount must not leave the card on "connecting").

**Pattern tab:** with a pattern open, edit the file from the terminal, then nudge a field: the conflict banner appears (Reload / Keep mine) and the terminal edit survives until a choice. **Drag a slider and switch documents mid-drag** in the Pattern tab, the Voxel Artist (palette drag) and the Models view (sculpt slider): the old file holds the last dragged value, the new file is untouched.

**Generation children:** start a generate run, then close the editor: no `cargo` or `ludion gen` process survives (Task Manager / `ps`). On macOS, also note what a force-killed (SIGKILL) editor leaves: its process-group children are orphaned by design of the tree kill; record whether that is acceptable.

## Reflection probe archetype
Place **Reflection probe** from the Level Designer palette inside a room with a glossy floor: after the capture, the floor's reflection shows the room's walls instead of the sky. Edit `extent` in the inspector: the reflection's parallax follows, with no rebake prompt.

## Held-input release (Clockout 0025)
Play mode, focus the viewer, hold D (the hero runs), Alt-Tab (Windows) / Cmd-Tab (M4) away and back: the hero stands. Same in the perf preview. Same with a held left mouse button. Hold D, then play → edit → play: the hero stands.

## Host path confinement (audit 2026-10-31 33/L3)
With vigil (in the repo) and a project outside the repo, no view shows an "outside the engine checkout" refusal:
- Switch between the two projects: every view still loads.
- Game Designer: edit and save a value.
- Voxel Artist and Models: browse the libraries.
- Generation: the candidates grid shows thumbnails.
- Terminal drawer: the status line still updates.
- Performance: save.

**Pattern tab, New pattern with an existing name (code audit H-02):** type a name whose slug is already listed (try a different case too) and press New: the note reads "already exists — pick it from the list to open it", the file's bytes are unchanged, and nothing is rewritten. A non-pattern `.yaml` at that path reads the host's "already exists" error instead.

## Inspector u64 round trip (audit 2026-10-31 16/M1)
With vigil playing a random-shape volley, select the live `Volley` in the inspector (edit mode) and nudge `angle_deg`: in-flight bullets keep their angles and speeds, no killed bullet reappears, and the `seed` field shows the same digits before and after.
