# Plan: Open a project — launch the editor on a game, run it, switch scenes and projects

> Status: **MECHANIZED-COMPLETE 2026-09-28 — W1–W4 shipped; the exit gate (human) is owed.** Every
> decision was ruled by the user in one session on 2026-09-27: decisions 5 and 7 by name, the rest by
> "go with your recommendations". Runs under the **editor** feature stream
> (`Plan/editor-views.md`). Current-code facts were read from the tree on 2026-09-27; re-grep before
> editing.

## Why

On 2026-09-27 the user asked how to open the editor on a `ludion new` project outside the
checkout. The answer took three acts that nothing ties together:
1. run `cargo run` in the project by hand;
2. start the editor from the checkout;
3. find the project setting behind the terminal drawer's header.

They asked for *"a more straightforward way to change project, or to be able to open the editor
directly set on a project"*. They then added that they want to *change scenes*, which means the
editor must be able to run the game.

## How it works when shipped

The editor still runs from the engine checkout; it has no installed app. It learns its project in
one of three ways:
- **An argument:** `node <checkout>/scripts/editor.mjs <dir>`, run from anywhere. `.` inside the
  project folder works.
- **Nothing:** the wrapper with no argument reopens the last project.
- **The top-bar switcher:** recent projects, **Open folder…**, or a pasted path.

Opening a project always does the same four things:
1. validate its `ludion.toml`;
2. build and start **that project's game binary**, showing compile progress and errors in the
   viewport;
3. attach the main session once the game is listening;
4. list its scenes in a picker that switches them live with `scene.load`.

Switching project shuts the game down and starts the next project's game. Attaching by URL to a
server someone else started still works.

## What exists today (verified)

- **The project is a localStorage string behind the terminal drawer.** `shell/projectDir.ts` holds it
  under `ludion.editor.projectDir`: checkout-relative inside the checkout, absolute outside. It
  defaulted to `games/vigil`; with nothing stored and no `--project` argument the editor now has no
  project and opens on the project switcher. The only editor for it is `ProjectConfig`
  (`terminal/TerminalDrawer.tsx`),
  a free-text field. Fifteen views and panels read the resolved path:
  - persona cwd and `.ludion/editor-context.json`;
  - the stage scenes and the voxel/sculpt/mesh libraries;
  - generation recipes;
  - the Systems code panel and the Performance view.
- **Nothing validates it.** `App.tsx`'s boot probe only checks that the directory exists and falls
  back to `games/vigil` if it does not. No `ludion.toml` check, and the project's name is never shown.
- **The main session is attach-only.** `App.tsx` holds `url` (`DEFAULT_URL = ws://127.0.0.1:9411`) and
  a reconnect machine (`viewer/reconnect.ts`) built for `ludion dev`'s restart blink. The reconnect
  refuses a schema-hash change as terminal. Persona terminals dial the connected URL
  (`terminal/spawnConfig.ts` `mcpConfigJson`), so the game's address is part of every running
  persona's config.
- **Host-spawned engines exist, but only as stages.** `src-tauri/src/stage.rs` has `StageSlot
  { Model, Effects }`, each with a fixed default port. It runs `cargo run -q --manifest-path
  <checkout>/Cargo.toml -p ludion-cli -- serve --scene … --addr …` (`cli_argv`, `cargo`). Readiness is
  the `listening on ws://…` line (`host.rs` prints the same line for `ludion::run`). A status
  machine surfaces cargo compile errors, and `stop` sends a graceful `engine.shutdown`.
  `reclaim_port` shuts down **whatever Ludion engine** holds a stage port. That is right for a
  stage's own orphan and wrong for a game the user started.
- **`scene.load` is on the wire** (the engine resolves the path against its project root), and the
  shell reacts to a foreign load through `scene.changed`: it clears undo (`editor/undo.ts`) and the
  selection, and prefills `savePath`. `dirty` tracks unsaved edits. Nothing in the main session
  *issues* a `scene.load`.
- **The host reads no argv.** `lib.rs` `run()` manages `pty::registry` and `stage::stages(…,
  repo_root)`. `pty::editor_paths` returns `{ repoRoot, personasDir, relayScript }`.
- **No dialog plugin.** `src-tauri/Cargo.toml` has only `tauri-plugin-log`; `capabilities/default.json`
  grants core window permissions only.
- **A `ludion new` project cannot use the stage views.** `crates/ludion-cli/src/new.rs` embeds
  templates with `include_str!`, and `templates/scenes/` holds only `main.yaml`. The Voxel Artist
  (`voxel/stage.ts` `RIGS`: `model-stage.yaml`, `model-stage-night.yaml`), Models (the same stage)
  and Effects (`effects/stage.ts` `STAGE_SCENE = effects-stage.yaml`) views need scenes only vigil
  has. vigil's reference `voxels/statue.voxel.yaml` and `voxels/adventurer.voxel.yaml`.

## Decisions

1. **`[user]` A project is a directory holding a `ludion.toml`.** The host reads `name` and
   `default_scene` and returns a `ProjectInfo`. A directory without the manifest is refused with
   "`<dir>` has no ludion.toml — a Ludion project is what `ludion new` creates; the engine checkout
   itself is not one." The top bar shows the name. *Rejected:* keeping any existing directory valid,
   which is how a typo'd path fails today, three views later.
2. **`[user]` Launch argument `--project <dir>`, with a thin Node wrapper `scripts/editor.mjs`.** The
   host parses `--project` from its own argv and returns it from `editor_paths` as `launchProject`.
   When present, it beats the stored value for this session **and becomes the stored value**. The
   wrapper resolves a relative `<dir>` against the caller's cwd, checks for `ludion.toml` up front (fast
   failure, the same message as decision 1), and runs `npm run tauri dev -- -- --project <abs>` in
   `editor/app`. Node, because dev scripts here are cross-platform Node. *Rejected:* a `ludion edit`
   CLI verb. The editor is not a workspace crate, and a Rust verb that shells out to npm is a second
   launcher to keep alive.
3. **`[user]` The project moves to a top-bar switcher, and the drawer's editor is deleted.** The
   switcher (in `shell/Titlebar.tsx`) shows the project name. Its menu holds:
   - recent projects: up to 8, most recent first, missing directories greyed with "not found";
   - **Open folder…**: a native picker through `tauri-plugin-dialog`;
   - a paste-a-path field.

   `ProjectConfig` is deleted; the drawer header keeps a read-only project line that opens the
   switcher. Recents live in localStorage beside the project: a per-viewer convenience. One way to
   change the project (the "Before adding a second way, remove the first" rule).
4. **`[user]` Switching project resets project-bound host state and leaves terminals alone.** The
   game (decision 5) and any running stage shut down gracefully; views re-read libraries from the new
   directory. Open persona terminals keep running in their old cwd, and their tabs are labelled with
   that project's name; new terminals open in the new project. *Rejected:* killing terminals, which
   would silently destroy an agent's in-flight work.
5. **`[user]` The editor runs the project's game.** It is a new host slot, `StageSlot::Game`, reusing
   the stage machinery: status machine, compile-error surfacing, graceful stop, stop-all on exit.
   Four differences from a stage:
   - **The command is the game binary:** `cargo run -q --manifest-path <project>/Cargo.toml --
     --scene <default_scene> --addr <addr>`, with cwd = the project. It runs the game binary, not
     `ludion serve`, so the project's own components and systems are registered.
   - **The address is `127.0.0.1:9411`**, the port `DEFAULT_URL` and every persona config already
     dial, so a restart keeps every live terminal's mcp connection pointed at the right place.
     *Rejected:* port 0, which would strand the running personas on every restart.
   - **No automatic reclaim.** If 9411 is busy, the host asks the listener for `engine.describe`.
     The viewport then offers **Attach** (the default, and the only choice when the listener is not a
     Ludion engine) or **Replace**, which sends `engine.shutdown` after the click, then starts. A
     server the user or an agent started is never shut down without a click.
   - **The main session auto-attaches** on `Listening`, through the existing connect path. A reconnect
     after a game restart that changes the schema hash (a different engine build) is a fresh connect,
     not the reconnect machine's terminal failure, because the editor caused the restart.

   **Attaching by URL stays** as the fallback for a server started outside the editor
   (`ludion dev`, an agent's `serve`). Parity gate: spawning a process is host work like the stages,
   not a world command, so the wire does not change.
6. **`[user]` The template ships the stage scenes, and an existing project can add them.**
   `crates/ludion-cli/templates/scenes/` gains `model-stage.yaml`, `model-stage-night.yaml` and
   `effects-stage.yaml`. They derive from vigil's, with the subjects switched to primitive meshes
   (`Mesh.shape: cube`) so they reference no asset. The entity names the views address
   (`subject`, `camera`, and any other name `voxel/stage.ts` / `effects/stage.ts` look up — re-grep)
   are kept. A stage view whose scene is missing says so and offers **Add stage scenes**: the host
   copies those three template files from `<checkout>/crates/ludion-cli/templates/scenes/` into
   `<project>/scenes/` and never overwrites an existing file. The template directory is the only
   copy; the editor holds no stage scene text of its own.
7. **`[user]` A scene picker in the main session.** It lives in `shell/SessionStrip.tsx`, next to
   Save, and lists `<project>/scenes/**/*.yaml` documents whose `kind:` is absent (a scene).
   `*-stage*.yaml` and `generated_from:` scenes are listed but marked. Picking one sends
   `scene.load` with the path relative to the content root. If `dirty` is set, it first asks: "Discard
   unsaved changes to `<savePath>`?". The loaded scene is marked from `savePath`, which
   `scene.changed` already prefills. It works against any attached server. **Switching scenes never
   restarts the game**; switching project does (decision 5). The game slot's `--scene` is only the
   first scene; the picker's later choice is not persisted.

## Implementation

### W1 — Validate and launch on a project

- `src-tauri/src/project.rs` (new):
  - `ProjectInfo { dir, name, default_scene }`;
  - `read_project(dir) -> Result<ProjectInfo, String>`, which parses `ludion.toml` with the `toml`
    crate (already in the workspace lockfile; add it to the host's `Cargo.toml` if absent) and
    refuses a missing or unreadable manifest with decision 1's message;
  - `launch_project() -> Option<String>`, which reads `--project` from `std::env::args` (accepts
    `--project <dir>` and `--project=<dir>`) and ignores the rest.

  Unit tests for both, argv injected as a slice.
- `pty::editor_paths` adds `launchProject: Option<String>`; a new `#[tauri::command] project_info(dir)`
  wraps `read_project`. Register it in `lib.rs`.
- `shell/projectDir.ts`: `pickProjectDir(stored, launch)`, where launch wins. `App.tsx`'s boot probe
  uses it and persists the winner. Checking the manifest via `project_info` replaces the bare exists
  check. The `games/vigil` fallback stays only for "nothing stored and no argument".
  `projectDir.test.ts` covers launch-beats-stored and launch-persists.
- `scripts/editor.mjs`: resolve, verify `ludion.toml`, `spawn('npm', ['run', 'tauri', 'dev', '--',
  '--', '--project', abs], { cwd: editor/app, stdio: 'inherit', shell: true })`, and propagate the
  exit code. `--help` prints the usage.
- `editor/app/README.md` "Run" section is rewritten around the wrapper; `editor/CLAUDE.md` gets a
  one-line pointer if its live-smoke recipe names the launch command.

**Exit:** `check.mjs` green; `node scripts/editor.mjs "<project>"` opens with the project's name shown.

### W2 — The switcher

- `tauri-plugin-dialog` added to `src-tauri/Cargo.toml`, `@tauri-apps/plugin-dialog` to `package.json`,
  `"dialog:allow-open"` to `capabilities/default.json`.
- `shell/recents.ts` (pure, tested): push/dedupe/cap-8 over normalized paths, stored under
  `ludion.editor.recentProjects`.
- `shell/ProjectSwitcher.tsx` in `Titlebar.tsx`: name button, menu with recents, **Open folder…**
  (`open({ directory: true })`), the path field. Every choice goes through `project_info` before
  `handleProjectDirChange`; a refusal shows inline.
- `App.tsx` `handleProjectDirChange` becomes the one switch point:
  1. stop the stages (`stage_stop` for each running slot);
  2. push the recent;
  3. persist.

  A test asserts it stops running slots.
- `TerminalDrawer.tsx`: delete `ProjectConfig` and its `showConfig` state; the header line opens the
  switcher. Each terminal records the project dir it was spawned with and shows that project's name
  on its tab.

**Exit:** `check.mjs` green; switch vigil ↔ the project from the top bar with a terminal open; the terminal
survives, labelled with its original project.

### W3 — Run the game, pick scenes

- `stage.rs`:
  - `StageSlot::Game`, default addr `127.0.0.1:9411`.
  - A `StageCommand` enum, `Cli { args }` or `Game { project }`, chosen per slot, replacing the
    hard-wired `cargo(repo_root, ["serve", …])`. `Game` builds `cargo run -q --manifest-path
    <project>/Cargo.toml -- --scene … --addr …` with `current_dir(project)`, under the same no-window
    posture.
  - `reclaim_port` is skipped for `Game`. Instead `probe_port(addr)` returns `Free`,
    `LudionEngine { describe }` or `Foreign`, and a start on a busy port returns that as a
    structured `Busy` status instead of an error.
  - `stage_start` gains `replace: bool` for the **Replace** path.
  - Unit tests: argv shape for `Game`; the busy-port branch with an injected probe (the
    `reclaim_port_within` precedent).
- `shell/gameSession.ts` (pure, tested): the phases `Idle → Starting (compile log) → Listening(url)
  → Exited(reason)`, plus `Busy(kind)`. Start is triggered by the project changing or the viewport's
  **Run game** button.
- `App.tsx`: on `Listening`, call the existing connect path with the slot's URL. Before a restart the
  editor caused, clear `expectedSchemaRef`, so a new engine build is a fresh connect. On `Exited`
  with an error, the viewport shows the compile/runtime tail the stage UI already renders
  (share that component; do not fork it).
- The connect panel shows three choices: **Run game** (primary), **Attach to URL**, and on `Busy`,
  **Attach** or **Replace**.
- `shell/ScenePicker.tsx` in `SessionStrip.tsx`:
  - list through `list_editor_dir` recursively under `<project>/scenes`;
  - read each candidate's first lines for `kind:` / `generated_from:` (the host's
    `read_editor_file`);
  - send `scene.load`, confirming first when `dirty`.

  Tests cover the classification (pure) and that a picker load takes the existing foreign-load path
  (undo cleared, selection dropped).
- `lib.rs`'s `Exit` handler already calls `stop_all`, which covers the game.
- The `#[ignore]`d live test in `stage.rs` gains a game-slot twin. It scaffolds a scratch project
  with the checkout's `ludion new` (the `crates/ludion-cli/tests/new.rs` scratch pattern), starts it
  on port 0, waits for `Listening`, sends `scene.load` of `scenes/main.yaml`, and stops it.

**Exit:** `check.mjs` green, the ignored live test passes locally, and opening a project cold builds and
shows its game.

### W4 — Stage scenes for every project

- Add the three template scenes (decision 6) and their `ENTRIES` in `new.rs`. `tests/new.rs`
  asserts they are scaffolded; the existing `generated_project_passes_cargo_test` must still pass,
  and a new assertion runs `check-scene` on each.
- `src-tauri`: `#[tauri::command] add_stage_scenes(project)` copies missing files from
  `repo_root/crates/ludion-cli/templates/scenes/` and returns the list written. Unit-tested on a temp
  dir, including never-overwrite.
- `voxel/stage.ts` and `effects/stage.ts`: when the start is refused because the scene is missing,
  the status offers **Add stage scenes**, then restarts.
- `docs/` has a `ludion new` project-anatomy topic (`crates/ludion-cli/embedded-docs/project-anatomy.md`
  mirrors it). Add the stage scenes there, then run `check.mjs --fix` to re-mirror.

**Exit:** `check.mjs` green; a fresh `ludion new` project and an external project both start the Voxel Artist and
Effects stages.

## Exit gate (human, owed once W4 ships)

1. From a cold start, `node scripts/editor.mjs "<project>"` opens the editor on the project with its game
   running in the viewport.
2. The project's scenes switch from the picker.
3. vigil ↔ the project switches from the top bar without touching a terminal.
4. The Voxel Artist and Effects stages start on the project after **Add stage scenes**.

## Status

**W1 shipped** (Lore rev 953): a project is a directory holding `ludion.toml`; the host's
`project_info` reads it, `--project <dir>` is the launch argument, and `scripts/editor.mjs` is the
wrapper.

**W2 shipped** (rev 954): the title-bar project switcher (recents, **Open folder…**, a paste-a-path
field) replaces the terminal drawer's free-text field; switching stops every running stage first.

**W3 shipped** (revs 955–957): the host runs a project's own game as `StageSlot::Game` with busy-port
detection and Attach/Replace; the main session auto-attaches once it is listening; a Scenes menu next
to Save lists and loads the project's scene documents via `scene.load`.

**W4 shipped** (this change): `ludion new` scaffolds `scenes/model-stage.yaml`,
`scenes/model-stage-night.yaml` and `scenes/effects-stage.yaml` with primitive-cube subjects (no
asset reference); an existing project without them gets an **Add stage scenes** button on the
missing-scene card in the Voxel Artist, Models and Effects views, which copies only the missing
files from the checkout's template directory and restarts the stage.

**Deviations from the design above:**
- The wrapper runs `tauri.js` through node with no shell.
- Busy-port detection uses the `ServerHello` handshake, not `engine.describe`.
- `scene.load` paths are content-root-relative.
- The pure module backing the scene picker is `sceneList.ts`, not `ScenePicker.ts` (TS1149 forbids a
  module name colliding with the component file).
- `stage_stop` blocks until the slot has actually exited.
- A terminal's Restart stays in the tab's own (possibly stale) project, not the currently switched-to
  one.
- **Known gap:** `savePath` stays empty on a fresh attach, so the scene picker marks no current scene
  until the first load or save.
- **W4's own:** the template subject is a primitive cube and `adventurer` a primitive figure (not a
  voxel model), so the Voxel Artist's declared-scale reference framing is degraded on a fresh
  project until a real model is staged.

**Review fixes** (the private review and its fix list): stop never
contacts a listener the slot did not spawn; a different project restarts the game slot and boot
waits for the launch project; a switch always drops the main session; the listening card offers
Attach; the scene picker lists the booted scene's directory; stale drawer copy and wave-named
comments.

## Not designed here

- **A packaged, double-clickable editor** (a project launcher in the style of Unity Hub). It needs a
  relocatable host: `pty::repo_root` is `CARGO_MANIFEST_DIR`-derived, and every host spawn is
  `cargo run` out of the checkout. Trigger: the editor ships to someone without a checkout.
- **Persisting the last-picked scene per project.** The game always starts on `default_scene`.
  Trigger: the user asks to resume where they left off.
