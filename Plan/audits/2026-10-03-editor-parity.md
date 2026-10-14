# Audit: the editor (Tauri host + React frontend) and the parity gate — 2026-10-03

**Scope.** This is a report-only audit at rev `08cd8b98`. It covers the Tauri host in
`editor/app/src-tauri/` (`lib.rs`, `pty.rs`, `files.rs`, `project.rs`, `stage.rs`, `wire.rs`,
`generation.rs`, `tauri.conf.json`, `capabilities/default.json`), the dev launch path
(`scripts/editor.mjs`, `editor/app/vite.config.ts`, `src/main.tsx`), and the React/TS frontend. In
the frontend it covers the protocol client (`src/protocol/client.ts`), undo (`src/editor/undo.ts`,
`mutations.ts`), the project switch and the game slot (`src/shell/*`, `App.tsx`), the stage
lifecycles in the Voxel Artist, Models and Effects views, the pattern tab and its YAML layer
(`src/editor/effects/*`), the Generation view, the model readout, the context bridge and the
drawer's spawn config. The gate coverage in `scripts/check.mjs` and `.github/workflows/ci.yml` was
checked. Already reported elsewhere, so not repeated here: scene-text S1 (shortest-`f32` in the TS
writer against the engine's widening), hierarchy H1 (keep-world reparent), wire-server W5/W7/W9
(handshake timeout, `related` required, no `schema_hash` check in TS).

## Overall assessment

The parity gate holds. Every world mutation the editor makes is a wire command. Every pipeline
capability is a `ludion <verb>` subprocess, with an allowlisted verb set for `gen`. Every document
edit is a splice through the host followed by an engine reload. Transform composition uses the
engine's `world: true`, and undo is built from public inverse commands. The gates are complete in
both `check.mjs` and `ci.yml`: `tauri-fmt`/`clippy`/`test`, `oxlint`/`tsc`/`vitest` and
`schema-drift`.

The weak points are the host's process lifecycle and its trust boundary, not parity:
- `stage_start` is check-then-act across a window that spawns `cargo`. The dev build's StrictMode
  double-mount drives two concurrent starts.
- App exit only *flags* a starting child for a kill.
- The webview origin is an unpinned `localhost:5173` with no CSP, in front of a host that spawns
  any program and reads or writes any path.

On the client side, a peer's `engine.restore` replaces the world without the `scene.changed`
signal the editor relies on to drop its undo stack and selection.

## High

None.

## Medium

### X1 — Whatever answers on `localhost:5173` gets the host's unrestricted spawn and filesystem IPC: the dev port is not pinned and CSP is off · Medium · [verified]

**Evidence.**
- `src-tauri/tauri.conf.json:8` points the window at `devUrl: http://localhost:5173`, and `:26`
  sets `"csp": null`.
- `vite.config.ts` has no `server` block. Vite's default `strictPort: false` therefore moves the
  dev server to 5174 when 5173 is taken, while Tauri still loads 5173.
- `scripts/editor.mjs:80` always runs `tauri dev`, so this is the only way the editor is launched.
- The command surface that origin receives:
  - `pty_spawn` (`pty.rs:516`) runs any `program`/`args`/`env`/`cwd`.
  - `read_editor_file`/`write_editor_file` (`pty.rs:567`, `:602`) take any absolute path.
  - `list_editor_dir`, `read_editor_image` and `editor_file_size` likewise.
- `files.rs:5-7` states the no-sandbox posture deliberately. That is reasonable for a local tool
  only while the webview is guaranteed to load the editor's own bundle.

**Why it matters.** The trust boundary is "the page at devUrl", and nothing pins which server that
is.

**Failure scenarios.**
- Another Vite app (5173 is Vite's default), or a second editor started from a different checkout
  or worktree, already holds 5173. The new editor window loads *that* app's frontend against this
  host. Best case, version skew: missing commands and a frontend from another tree. Worst case, a
  foreign page gets `pty_spawn`.

**Fix.**
- Pin the port in `vite.config.ts` (`server: { port: <n>, strictPort: true }`), the Tauri-recommended
  setup, so a busy port fails loudly instead of silently re-routing.
- Set a restrictive CSP (`default-src 'self'; connect-src ws://127.0.0.1:* ws://localhost:*` plus
  the dev HMR socket).

### X2 — `stage_start` is check-then-act with no in-flight guard, and the dev build's StrictMode double-mount fires it twice · Medium · [suspected]

**Evidence.**
- `stage.rs:647` reads the phase under the lock and releases it.
- The `Starting` phase, the new `generation` and `inner.kill` are only written at `:764`. That is
  after `reclaim_port` (`:691`), the game slot's `sync_project` (`:738`, a `cargo run ludion sync`
  that can take seconds or minutes) and `Command::spawn` (`:747`).
- `stage_start` is `#[tauri::command(async)]`, so two calls run concurrently, and both can see
  `Stopped` and both spawn.
- `src/main.tsx:7` wraps the app in `<StrictMode>`. In development, React mounts, unmounts and
  remounts every effect.
- Each stage effect (`ModelsView.tsx:304`, `VoxelArtistView.tsx:369`, `EffectsView.tsx:120`) awaits
  `stageStatus` and then calls `stageStart` without checking its `cancelled` flag. Every first mount
  therefore sends two `stage_start`s within a millisecond.

**Why it matters.**
- The second spawn overwrites `inner.kill` with its own flag. The first child's flag becomes
  unreachable, and its status updates are dropped as a stale generation.
- `stop()`/`stop_all()` only ever signal the current generation.
- The two `ludion serve --addr 127.0.0.1:9412` children race for the port. If the untracked one wins,
  the view shows `Failed` ("address in use") while an invisible engine serves the port. That engine
  cannot be stopped through its slot until a Restart's `reclaim_port` shuts it down over the wire.
- The same race opens on a project switch. `switchProject` sees `Stopped` while the old game's start
  is still inside `sync_project`, so the old project's game spawns after the switch.

**Not confirmed at runtime.** No GUI was launched. The host race is certain from the code; the
visible outcome depends on which child binds first.

**Fix.**
- Make start atomic per slot: set a `Starting` status (with a fresh generation) under the lock
  before any slow step, and make a concurrent `start` return it.
- In the views, re-check `cancelled` before calling `stageStart`.

### X3 — `engine.restore` replaces the world without `scene.changed`, so the editor keeps an undo stack and selection that now point into a different world · Medium · [verified]

**Evidence.**
- The editor drops its undo history, selection and pointer only on `scene.changed` (`App.tsx:652-670`)
  or on a client change (`App.tsx:518`).
- `scene.load` and `scene.save` push `scene.changed` (`crates/ludion-engine/src/commands/scene.rs:58,81`).
- `engine.restore` (`commands/engine_cmds.rs:29-35` → `snapshot::restore_into`) pushes no event at
  all. `snapshot.rs` has no `push_event`.
- The drawer's agent is auto-allowed every `mcp__ludion` tool. Only `scene_load` and
  `engine_shutdown` are on `ask` (`terminal/spawnConfig.ts:74`), and the stated reason ("can wipe
  unsaved human and agent edits") applies equally to `engine_restore`.

**Why it matters.** Undo entries carry `{index, generation}` ids and before-values from the
pre-restore world. Restore re-installs the snapshot's allocator, so those ids may now name different
entities.

**Failure scenario.**
1. A human edits entity A in the editor.
2. The agent in the drawer calls `engine.restore` with an earlier snapshot.
3. The human presses Ctrl+Z. The editor sends `component.set` to A's old id, which in the restored
   world is either dead (the group is dropped with an error) or reused by another entity, which then
   silently receives A's old value.

The selection and the context-bridge file also keep naming the stale id.

**Fix.**
- Have `engine.restore` push `scene.changed` (path `None`). The editor's existing foreign-change path
  then resets its state.
- Add `mcp__ludion__engine_restore` to the drawer's `ask` list.

### X4 — App exit never kills a slot that is still `Starting`, and on macOS killing the `cargo` wrapper does not reach the engine · Medium · [suspected]

**Evidence.**
- On exit, `lib.rs:66` calls `AppStages::stop_all()`, which calls `stop()` per slot
  (`stage.rs:1188-1192`), not `stop_and_wait()`.
- For a `Starting` slot, the url is still `None`, so `stop()` skips the graceful path, sets the kill
  flag (`:914`) and returns immediately.
- The flag is honoured by a waiter that polls every 50 ms. The pty shutdown that follows returns at
  once when no terminal is open. The process therefore exits before the waiter ever calls
  `child.kill()`.
- `stage.rs`'s own comments say killing the `cargo run` wrapper "can leave the engine behind holding
  the port".

**Platform behaviour (from reasoning, not observed).** On Windows, cargo's job object (and
`tauri dev`'s) takes the tree down. On macOS nothing does.

**Failure scenario (macOS).**
1. A user opens a project; the game slot starts a cold `cargo run`, which takes minutes.
2. The user closes the editor. The build finishes and the game boots, orphaned, on 9411.
3. The next editor launch shows the game slot as `Busy` ("already serving a Ludion engine").

The same applies to a project switch during the game's boot.

**Fix.**
- On exit, use `stop_and_wait()` (bounded), or have `stop()` kill synchronously when there is no url.
- Run the built binary directly (`cargo build`, then spawn the exe), or put the child in its own
  process group and signal the group, so a kill reaches the engine.

## Low

### X5 — `ProtocolClient.connect` never settles if the socket closes cleanly before the `ServerHello` · Low · [verified]

**Evidence.**
- The `close` handler (`client.ts:202-212`) rejects pending calls but not the connect promise.
- Only the `error` handler settles a pre-handshake failure.
- A WebSocket that opened and is then closed by the server fires `close` without `error`. That is
  the case when a server is shutting down mid-handshake: its `read_text` returns `None` on the
  shutdown watch.
- `attemptReconnect` (`App.tsx:973-990`) awaits `connect` with no timeout.

**Failure scenario.** A reconnect attempt lands in the window while a `ludion dev` child is shutting
down for a rebuild. The reconnect machine stays in "attempting" forever, and only a manual
Disconnect/Connect recovers.

**Fix.** In `close`, reject the connect promise if `!handshakeDone`, and add a connect timeout.

### X6 — A rejected handshake surfaces as "server speaks protocol_version undefined", dropping the engine's `L3003` message and help · Low · [verified]

**Evidence.** The server answers a bad `Hello` with an error envelope
(`crates/ludion-server/src/connection.rs:767-781`). `client.ts:168-186` parses that envelope as a
`ServerHello` and compares `protocol_version`, which is `undefined`.

**Why it matters.** This is the one moment the engine says how to fix the problem ("regenerate the
client…"), and the editor discards it.

**Fix.** Check for an `error` key first and raise a `ProtocolError` from it, as `wire.rs:76` already
does on the host side.

### X7 — The Generation view applies a slow verb's result to whichever recipe is selected when it returns, and a recipe switch mid-run re-enables Generate · Low · [verified]

**Evidence.**
- `views/GenerationView.tsx:162-186` (`generate`) and `:141-157` (`runCheck`) apply `setRun`,
  `setRunId` and `setCheck` with no check that `selected` or `projectDir` are unchanged.
- The recipe list stays clickable while `busy` (`AssetBrowser` at `:324-333` is not disabled).
- Selecting a recipe triggers `runCheck` (`:233-243`) through `runVerb`, which calls `setBusy('check')`
  and later `setBusy(null)` while the earlier `gen run` is still going.

**Failure scenario.**
1. Start a multi-minute `gen run` on recipe A, then click recipe B.
2. B's check finishes and the Generate button re-enables, so a second concurrent run can start.
3. When A returns, its candidates render under B. `runDir` and `accept` combine B's name with A's
   run id (`:486`, `:191`), so thumbnails and Accept fail with "no run".

**Fix.** Capture `selected` and `projectDir` per invocation and drop a result that no longer matches.
Count busy verbs instead of overwriting a single slot.

### X8 — Starting a model or effects stage shuts down any Ludion engine on 9412/9413, including another editor's live stage · Low · [verified]

**Evidence.** Model and Effects call `reclaim_port` unconditionally (`stage.rs:690-691`), and it
sends `engine.shutdown` to whatever holds the port (`:340-356`). The design assumes the holder can
only be this host's own orphan. With two editors open on one box (two projects, or two instances on
the shared checkout), they hold the same fixed ports.

**Failure scenario.** Opening the Voxel Artist in editor 2 kills editor 1's model stage. Editor 1's
Restart then kills editor 2's, back and forth.

**Fix.** Only reclaim a listener whose identity this host recorded (for example the engine version
plus a per-host token passed on the command line). Alternatively, bind the stages to port 0 and
read the announced url, which `parse_listening_url` already supports.

### X9 — Save writes over a `generated_from:` scene opened from the scene picker, with no warning · Low · [verified]

**Evidence.**
- `editor/CLAUDE.md` says "A scene bearing `generated_from:` is never written".
- The picker lists generated scenes (marked, `shell/sceneList.ts:20`) and loads them.
- The load's `scene.changed` sets `savePath` to that file (`App.tsx:673`).
- `SessionStrip.tsx:79-94` saves to `savePath` with no generated check (Ctrl+S included).
- The engine carries the marker forward, so `map-drift` catches it later, at `check.mjs` time and
  not at the click.

**Fix.** Disable Save, or have it ask first, while the loaded scene is generated, and name the
source to edit instead.

### X10 — `patternSchema.ts` is a hand-pasted copy of `schema_for!(PatternDoc)` with no drift gate · Low · [verified]

**Evidence.** `src/editor/effects/patternSchema.ts:1-14` says to refresh it by pasting. Only
`docFields.test.ts` reads it, and no test compares it with the Rust schema. It is in sync today:
field lists were compared against `crates/ludion-engine/src/patterns.rs:119-331`.

**Why it matters.** A field added in Rust silently never reaches the form on the effects stage, the
only place the fallback is used.

**Fix.** Emit the schema into a generated file through the existing `schema-drift` path, or add a
Rust test that asserts the TS copy's JSON equals `schema_for!(PatternDoc)`.

### X11 — Undo's entity-ref remap knows only `Parent` and `PrefabInstance`, so a respawned cascade leaves other entity refs dangling · Low · [verified]

**Evidence.**
- `undo.ts:88-90` hardcodes the ref-bearing components.
- `PatternPlayer.target` and `Volley.source` are registered components carrying `Option<EntityId>`
  (`crates/ludion-engine/src/components.rs:3566,3572`; `patterns.rs:420,569`), and a game can
  register more.

**Failure scenario.** Destroy a parent whose child has a `PatternPlayer` aimed at the parent, then
undo. The parent respawns with a new id, the child respawns with `target` still naming the dead id,
and the turret stops aiming.

**Fix.** Find entity refs from the JSON Schema (fields whose schema is the `EntityId` shape) instead
of a name list.

### X12 — The model readout parses `check-scene`'s prose, not a JSON form · Low · [verified]

**Evidence.**
- `stage.rs:1108` runs `check-scene <path>` without `--json`.
- `src/editor/voxel/readout.ts:9-27` parses English summary lines ("283 filled voxels",
  "`: OK (`").
- On the Rust side the format is pinned only by `contains("3 filled voxels")`-style asserts
  (`crates/ludion-cli/tests/check_scene_voxel.rs:54`).
- `--json` exists for other kinds (`check_scene.rs:41-55`) but not for voxel or assembly summaries.

**Why it matters.** A wording change in the CLI silently blanks the readout, and no gate spans both
sides.

**Fix.** Give `check-scene --json` a voxel/assembly summary object and parse that, as the pipeline
verbs already do.

### X13 — One-entity reads go through whole-world `query.entities` · Low · [verified]

**Evidence.**
- The context bridge queries every entity with every registered component to find the selection
  (`App.tsx:549-560`). It reruns on selection, pointer, view and camera-settle changes.
- The gizmo pose queries every entity's `Transform`, `Parent` and world transform
  (`viewer/Viewport.tsx:511-514`).
- The Game Designer view does the same at `App.tsx:618-621`.
- The wire has no entity-id filter (`QueryEntitiesParams`, `crates/ludion-protocol/src/command.rs:693`).

**Why it matters.** The cost scales with world size; vigil's 3× city is the case that matters. This
is not a parity problem: it is a missing wire capability.

**Fix.** Add an `entities: [EntityId]` filter to `query.entities`. A new wire field is the user's
call.

### X14 — Stale docs and comments · Low · [verified]

- `stage.rs:956` says the host watcher is "the **only** reloader in the system: nothing else fires
  `asset.reload`". `PatternEditor.tsx:248` sends `asset.reload` itself, by design (its header,
  `:7-9`).
- `PatternEditor.tsx:9` says "keeps the parity gate at 29 wire methods". The wire has 38.
- `stage.rs:19` and the `notify` comment in `Cargo.toml` describe the watcher as `voxels/**` only.
  It also watches `sculpts/` (`:958-961`).
- `protocol/client.ts:10-11`: "No auto-reconnect … Reconnection policy belongs to the caller
  (viewport UI, later task)". This is a schedule comment, and the reconnect machine now exists in
  `App.tsx`.
- `src-tauri/Cargo.toml:11` describes the crate as a "read-only live viewer". It is the full editor
  host (`productName`/identifier `ludion-viewer` likewise).

## Checked and healthy

- **Parity gate.**
  - Every world mutation in the views is a wire command: `component.set`/`remove`,
    `entity.spawn`/`destroy`/`reparent`, `scene.load`/`save`, `sim.*`, `render.view`, `asset.reload`.
  - No Tauri command touches engine state.
  - The host's two wire calls (`asset.reload`, `engine.shutdown`) are public methods.
  - `gen` runs as an allowlisted `ludion gen <verb> --json` subprocess with `--json`/`--url` owned by
    the host (`generation.rs:62-86`). `sync` runs with `--json`, and `check-scene` (X12 aside) is
    the engine's checker.
  - `add_stage_scenes` copies the checkout's templates and never overwrites (user decision 6).
- **No second scene writer.** The editor writes no scene text. Document edits go through
  `write_editor_file` (atomic temp + rename) and a reload. `yamlDoc.ts` stays block-only and
  comment-preserving (S1 is its known float issue).
- **No duplicated transform math in the gizmo path.** It reads `world: true` from the engine.
- **Undo is response-powered and wire-only.** It covers descendant cascades, remaps respawned ids
  across both stacks and inside `Parent`/`PrefabInstance` values, repairs `PrefabInstance.root` after
  a group, drops a failed group instead of wedging, and clears on a client change and on a foreign
  `scene.changed`.
- **Project switch.** It drops the session first, then stops every running slot, then commits. A
  status/stop failure does not block it. The game slot's `reuses_running` compares the project.
- **Stage teardown.**
  - The graceful `engine.shutdown` comes first, with the kill as a fallback.
  - A generation counter keeps a superseded child's exit from overwriting a fresh status (X2 is the
    path around it).
  - The game slot never reclaims a foreign listener.
- **The PTY host.**
  - The cursor-query answer before `ClosePseudoConsole` and the open-pseudoconsole count are gated
    by tests.
  - A bounded drain runs on exit.
  - `check_cmdline` names the oversized argument.
- **No shell anywhere in the host.** Every subprocess is `Command::new("cargo")` with an argv, so
  there is no shell injection.
- **Request-id matching in the TS client.** It uses a map keyed by id and tolerates unknown ids.
  Pending calls are rejected on close. The frame/event demux is pure and tested.
- **No HTML injection sinks in the frontend.** There is no `dangerouslySetInnerHTML`, `innerHTML` or
  `eval`. Generated images arrive as size-capped `data:` urls.
- **Gate coverage.**
  - `tauri-fmt`/`tauri-clippy`/`tauri-test` run in both `check.mjs:284-286` and `ci.yml:204-213`.
  - `oxlint`/`tsc -b`/`vitest` run in both (`ci.yml:430-458`).
  - `schema-drift` regenerates `protocol.d.ts` and diffs it (`ci.yml:389-424`, `check.mjs:499`).

## Suggested order of work

1. **X2** — make `stage_start` atomic per slot, and re-check `cancelled` in the three views. This is
   cheap and removes the invisible duplicate engine.
2. **X1** — `strictPort` plus a pinned port in `vite.config.ts`, and a real CSP. These are config
   only.
3. **X3** — `scene.changed` on `engine.restore`, and `engine_restore` on the drawer's `ask` list.
4. **X4** — a bounded wait on exit, and a kill that reaches the engine (spawn the built binary
   directly).
5. **X5, X6** — client handshake robustness. Small and testable in `client.ts`'s pure tests.
6. **X7, X9** — UX guards in the Generation view and Save.
7. **X8, X10–X14** — as touched.
