# Plan: Editor Views — role-based workspaces + an embedded agent terminal

> **CLOSED — shipped; the views arc landed and later editor slices build on it:
> `Plan/editor-voxel-artist.md` ("waves 0–4 SHIPPED 2026-07-24 … Editor slice #3") and
> `Plan/editor-art-director.md` (slice #4), then the Effects view (`Plan/particles-engine-and-editor.md`)
> and the Models view (`Plan/mesh-modeling.md` Wave 2, the model stage over a second document kind;
> named Sculpt until `Plan/editor-stage-camera.md` decision 8 renamed it),
> and the Generation view (`Plan/comfy-generation.md` Wave 4, the first view with no engine session
> at all: the host runs `ludion gen … --json` as a subprocess, per that plan's decision 15);
> the view shells live in `editor/app/src/views/`.**
> Status: **arc starting (planning).** Input of record is the editor's private feature notebook
> (all decisions settled 2026-07-18 via a grill-me interview with the user — this plan condenses and
> phases them, and does not re-litigate any of them). Visual design of record is the mockup
> `editor/mockups/07-meridian-v2.html` (the "Meridian · vertical terminal" layout: full-height Claude
> terminal on the **right**, hierarchy-over-inspector on the **left**, viewport + horizontal kit shelf
> in the center, a view-switcher row, a session strip, a status bar, and a Ctrl+K command palette
> closed by default). This is a **purely client-side arc** — no new wire surface (the surface stays 26
> methods), parity holds via the existing commands + `ludion-mcp` — so it runs beside the game arc and
> is the *next* step, queued behind the work currently in progress.

## Goal

Grow the Tauri editor from a single-surface viewer/editor into a role-based **views** app. Each view
is a curated arrangement of the same shared panel library, plus an embedded terminal drawer that hosts
per-view domain-expert Claude Code instances driving the *same live engine session* the human sees, as
peer protocol clients. The first two views are **Level Designer** (reframes the existing
hierarchy/palette/inspector/gizmo surface + a kit browser) and **Game Designer** (schema-driven
doc-editor forms over E4 data-docs with hot reload). **Lighter** and **Art Director** are declared in
the switcher but not built.

Scored against the North Star (*how many autonomous iterations before a human must look at the
screen?*): the arc puts a domain-expert agent inside the tool the human already has open, sharing the
human's live session — the human points and describes, the agent drives the protocol and verifies
headlessly, and both watch the same viewport. It is the human-in-the-loop counterpart to the headless
agent loop the engine already serves.

## Decisions summary (from the editor's private feature notebook — the authoritative record)

- **Tauri is the product.** The Tauri build is the canonical editor; the Vite browser page is dev-only
  serving of the same frontend, never a supported client. Native host capabilities (PTY, menus,
  dialogs, layout persistence) live in the Tauri Rust side. React + TS codegen stay — no native-UI
  rewrite.
- **A view is a workspace preset, not a hard mode.** One shared panel library; a view = a named layout
  config (which panels, filters, defaults) + which Claude persona its terminals boot. Views are never
  capability walls — the parity gate already makes every tool a protocol command.
- **Layouts are fixed and curated** (hand-designed grids per view; the terminal drawer in one
  consistent slot). No docking framework; rearrangeable panels deferred until real user need.
- **View roster / order:** Level Designer first, Game Designer second. Lighter and Art Director are
  declared but NOT built until the engine surface justifies them. The map painter joins the Level
  Designer view only behind `Plan/map-authoring.md`'s recorded M2 trigger.
- **Game Designer view = docs + Claude for behavior.** Tunables via schema-validated doc forms (hot
  reload); anything Turing-complete goes through the view's Claude writing Rust. The no-embedded-
  scripting pillar holds.
- **The editor owns its PTY host** (Tauri Rust side, portable-pty-style + xterm.js drawer) and spawns
  `claude` directly. **No runtime dependency on exarch** — exarch's embedding code is the reference
  implementation to study, not a backend. (Optional later: register editor-spawned instances with a
  detected exarch broker.)
- **Personas are versioned repo files** (`editor/personas/*.md`) whose text the drawer reads at spawn
  and passes via `claude --append-system-prompt`, with `--mcp-config` injecting `ludion-mcp --url <the
  editor's live server URL>` (it dials the running `ludion serve` lazily — a true peer client). Passing
  the file by path is not an option: the CLI rejects `--append-system-prompt` and
  `--append-system-prompt-file` together ("Cannot use both …", tested), and the editor-session section
  has to ride in the same string. Personas don't inline knowledge; they instruct pulling `ludion docs
  <topic>` on demand. cwd = the game project.
- **Every spawn picks a model, on one screen.** The spawn screen *is* the drawer's empty state (and
  what `+` opens): the four model cards with the last-used id pre-selected, then `Spawn terminal`.
  The choice becomes `--model <id>`. Restart reuses the tab's own model without re-asking.
- **Spawn arg vector (pinned):** `--model <id>`, `--append-system-prompt <persona text + editor-session
  section>`, `--mcp-config <inline json>`, `--settings <inline json>`. **No positional first prompt** —
  a first prompt would burn a real user turn, so spawn-time context is system-prompt material.
- **The ludion surface is pre-approved at spawn.** The `--settings` json carries `permissions.allow`
  = `mcp__ludion` (the server-level rule, covering every protocol command) plus `Bash(ludion:*)` /
  `Bash(ludion *)` / `Bash(cargo run -p ludion-cli:*)`, so the tools the drawer exists to run never
  ask a designer to adjudicate a call they can't judge. `scene.load` stays on `ask` — it can wipe
  unsaved human *and* agent edits. `defaultMode` is unset: everything outside ludion still prompts.
- **Context bridge: spawn-time section + live context file.** View / project dir / scene path / engine
  url are appended to the system prompt at spawn; while running, the editor continuously writes a small
  context file (selection id + components, camera pose, active view) to a known path, and personas read
  it before acting on "here/this"-type instructions. Promoting to a protocol session-context concept is
  deferred until the file proves load-bearing.
- **Edit contract: live peer edits.** While attached to a shared session the persona authors via
  protocol commands like any client; NEVER `scene.load`/dev-reload while the session is dirty;
  `scene.save` is the shared checkpoint capturing both human and agent edits. Undo stays per-client
  own-ops-only (conflicting undo = accepted v1 wrinkle). Text-file editing remains the mode for docs
  (hot-reload-safe) and autonomous work outside a shared session.
- **Instance lifecycle: per-view, persistent, tabbed.** Each view lazily spawns its persona instance
  on first terminal open; PTYs survive view switches (switching swaps which *tab set* the drawer
  shows). A view may host multiple terminals as tabs. Status chips show running/idle/waiting-for-input;
  explicit restart re-spawns fresh. Each terminal tab shows **context usage + prompt-cache expiry**.

## Implementation contracts (chosen by the orchestrator — hold these fixed)

- **Tauri PTY commands:** `pty_spawn`, `pty_write`, `pty_resize`, `pty_kill`, `pty_list` (live-session
  ids for frontend reconciliation), plus `write_editor_context` (atomic write of the context file).
- **Tauri PTY events:** `pty://output` (payload is **base64** — PTY bytes are not guaranteed valid
  UTF-8; decode in the frontend before feeding xterm.js), `pty://exit` (carries the child's exit
  status / signal so the tab can flip its status chip and offer restart).
- **Context file path:** `<game project cwd>/.ludion/editor-context.json`. This is host-written
  runtime state, never committed — it **must be ignored in both places** (CLAUDE.md "Ignore rules live
  twice"): add a rule to the root `.gitignore` **and** the root `.loreignore`. Recommended rule:
  `.ludion/` (a per-project editor scratch dir), mirrored byte-for-byte into both files in the same
  change, following the `games/*/save/` precedent already in both.
- **Personas:** versioned repo files under `editor/personas/` (`level-designer.md` is the first,
  written with this arc; `game-designer.md` lands with Slice #2).
- **Command palette:** visually present in Slice #1 (the mockup's Ctrl+K entry + scrim), but
  **functionally deferred** — recorded as an open item below.

## Phasing

Two built slices (Level Designer, then Game Designer) plus two declared-not-built views.

### Slice #1 — View shell + terminal drawer (the Level Designer view)

The Meridian shell, with every existing viewer/editing panel reframed as the Level Designer view, and
a working terminal drawer hosting a real `claude` instance that drives the live session.

**Work breakdown**

1. **Meridian app shell.** Recompose `editor/app/src/App.tsx` into the mockup's layout: title strip,
   view-switcher row, session strip (Edit/Play, Save, Undo/Redo, connection chip), the three-column
   main row (terminal right / viewport + kit shelf center / hierarchy-over-inspector left), and the
   status bar. Reuse the existing panels wholesale — `Viewport`, `Inspector`, `HierarchyPanel`,
   `PalettePanel` (as the kit shelf), `SessionPanel`, `DiagnosticBanner`, the gizmo overlay. No
   protocol-facing logic changes; this is a layout/composition change. The view-switcher renders all
   four views; Level Designer is active, Game Designer is present-but-empty, Lighter/Art Director are
   disabled with an "awaiting engine surface" tooltip.
2. **View model.** Introduce a small `View` descriptor (id, label, hotkey, the panel set + defaults,
   the persona filename) and a `views/` registry. Slice #1 defines only `level-designer`; the shape
   must accommodate `game-designer` in Slice #2 without a rewrite. No docking framework — layouts are
   fixed per view.
3. **PTY host (Tauri Rust side).** Add a `portable-pty`-based PTY manager to `editor/app/src-tauri`
   exposing the four commands and two events above. It spawns `claude` with:
   the pinned arg vector above (`--model`, `--append-system-prompt` carrying the persona text,
   `--mcp-config` injecting `ludion-mcp --url <the editor's live server URL>`, `--settings`), and
   cwd = the game project. Output is streamed as base64
   `pty://output` events; input arrives via `pty_write`; resize via `pty_resize`; teardown via
   `pty_kill` and on window close. PTYs are keyed so they survive view switches.
4. **xterm.js terminal drawer.** A tabbed terminal component: each tab is one PTY-backed `claude`
   instance; `+` spawns another tab in the current view; switching views swaps the visible tab set (the
   PTYs keep running). Wire xterm.js to the events/commands. Status chips (running / idle /
   waiting-for-input) and an explicit restart-fresh action per tab.
5. **Context bridge.** On selection / camera / view change, write `<cwd>/.ludion/editor-context.json`
   (selection entity id + its components, camera pose, active view; small, debounced, atomic-replace).
   Append the view / project dir / scene path / engine url to the instance's system prompt at spawn,
   pointing it at the context file for everything live.
6. **Status chips: context usage + cache expiry.** Render the per-tab context-usage bar and prompt-
   cache countdown from the mockup. The *source* of these numbers is an open item (see below); Slice #1
   ships the UI and a best-available feed, and hardens the mechanism during the slice.
7. **Command palette (visual only).** Keep the Ctrl+K entry and scrim from the mockup wired to
   open/close; leave command execution unimplemented (open item).

**Exit criteria**

- The Meridian shell renders with **all existing viewer/editing functionality intact** — connect,
  frame stream, hierarchy, inspector, gizmo edit, palette spawn, save, undo/redo, play/edit — and the
  **frontend gates are green** (`node scripts/check.mjs --frontend-only`: oxlint + tsc + vitest), with
  the full `check.mjs` suite green.
- The PTY host **spawns a real `claude` instance** that connects to the same live `ludion serve`
  session via `ludion-mcp` as a **peer client** and drives it (a spawned instance can, e.g.,
  `entity.spawn` / `component.set` and the change appears in the human's viewport).
- The **context file is written continuously** by the editor and **read by the persona** — a deictic
  instruction ("move this house") resolves against the current selection without the human restating
  the id.
- **Status chips are live** (running/idle/waiting; context-usage bar and cache countdown populated by
  the chosen mechanism).
- Multiple terminal tabs coexist; PTYs survive view switches; explicit restart re-spawns fresh.
- The **parity audit still finds zero editor-private endpoints** — the agent terminal adds native host
  capability (PTY) but no engine capability; every world edit still flows through a protocol command.

### Slice #2 — Game Designer view

A second curated view whose panels are **schema-driven doc-editor forms** over the E4 data-docs
(`doc_kind::<T>` / `DocStore<T>`), editing tunables as text with hot reload; its terminal boots a
`game-designer` persona (whose job for anything Turing-complete is to write hot-reloadable Rust — the
no-embedded-scripting pillar holds). Forms are generated from the same JSON Schema the wire/file
validation already uses. Detailed work breakdown deferred to when this slice starts; it reuses the
Slice #1 shell, view model, PTY host, and context bridge unchanged.

**Shipped decisions (2026-07-19).** No LSP in v1: the code editor is CodeMirror 6 + `lang-rust`
syntax only, with an open-in-IDE escape hatch; Claude is the smart layer over the code, not an
in-editor language server. The edit loop rides `ludion dev`, which writes `.ludion/dev-status.json`
(machine-readable build status + rustc diagnostics) that the editor tails for the build-status chip.

### Lighter (Slice #5) — shipped decisions

Its design was drafted outside the repo, so these rulings are recorded here; the shipped code is the
rest of the record. The Lighter inherits every Art Director ruling (materialize-on-carrier,
hold-to-compare semantics, never writes a file, `L3006` = absent).

- **The cost model IS the "bake" UI** — no bake verb exists. Cheap fields ride the shared preview
  slider; `cell_size` and `probe_spacing` are stepped stop rows with one commit-cadence write per
  press, and they deliberately carry **no** `x-ui-range`, because every distinct value re-runs a real
  build. (Recorded in `dev-docs/ludion-ecs.md` beside the colour-triple ruling.)
- **Field A/Bs hold neutral *values*** (`sky_visibility: 0`, `bounce: 0`), **never `component.remove`**
  — a remove/restore would re-voxelize on every flip. Polled card values are entity-tagged, so a moved
  selection cannot render one entity's numbers against another.
- "Enable GI" is `component.set {}`.
- **Deferred by decision:** the probe/cell-count readout (a client-side estimate is an ungated drift
  surface), sheets-in-tab, and azimuth/elevation sun control.

### Declared, not built — Lighter and Art Director

Present in the view switcher as disabled tabs. **Trigger to build either:** the engine surface must
first expose enough editable data to justify a dedicated view. Today it does not — the only light
primitive is `PointLight` plus a single sun-only directional shadow, and the post stack is **not**
exposed as editable data (it is renderer-internal, not a doc/component a form could bind). Build the
Lighter view when lights + shadows become a richer editable surface; build the Art Director view when
the post stack (and/or textured-PBR material data, itself behind its own trigger) becomes editable
data. Until then they stay declared.

## Open items

- **Context / cache-indicator mechanism — RESOLVED (2026-07-19, from a written investigation):
  Claude Code's statusline hook.** The statusline command receives a `context_window` JSON payload
  (total tokens, window size, cache read/creation tokens) on every assistant message plus a
  configurable refresh interval, and it is injectable **per instance** via the `--settings` flag at
  spawn — no polling of the Anthropic API, no changes to the user's global config. The editor points
  it at a small cross-platform Node relay script that writes the JSON to a per-session file the host
  tails. Prompt-cache expiry has **no direct signal anywhere** (statusline, OTEL, or transcript): the
  countdown is derived — timestamp of the last turn with `cache_creation_input_tokens > 0` plus the
  known TTL window, reset whenever a new cache write appears. Rejected: OTEL telemetry (needs a
  collector daemon; batch-export latency) and transcript-JSONL parsing (format documented as internal
  and version-fragile). Validate during the terminal-drawer wave; if the statusline payload shifts,
  the relay script is the single point of repair.
- **Command palette execution.** Slice #1 ships the palette visually (Ctrl+K open/close) but does not
  execute commands. Wiring it to real protocol commands / editor actions is deferred.
- **Game Designer persona.** `editor/personas/game-designer.md` is written with Slice #2, not now.
- **Optional exarch-broker registration.** Later, the editor may register its PTY-spawned instances
  with a *detected* exarch broker (discovery only — never a runtime dependency; the editor's PTY host
  is authoritative). Study exarch's embedding code as the reference implementation.
- **Rearrangeable panels.** Deferred until a real user need — no docking framework in this arc; layouts
  stay fixed and curated.
- **Promoting the context bridge to protocol.** The context file is a client-side convention for now;
  promote it to a protocol session-context concept only once it proves load-bearing.

## Current-code facts that shape the work

- The editor frontend is `editor/app` (React + TS + Vite); the shell is `editor/app/src/App.tsx`, with
  panels under `src/viewer/` (`Viewport`, `Inspector`, `SessionPanel`, `DiagnosticBanner`,
  `GizmoOverlay`, picking/keymap/reconnect logic) and `src/editor/` (`HierarchyPanel`, `PalettePanel`,
  entity-ops, mutations, undo, gizmo). These are reframed into the Level Designer view, not rewritten.
- The Tauri host is `editor/app/src-tauri` — its **own** cargo workspace (deliberately not a member of
  the root `crates/*` workspace), so adding `portable-pty` + `base64` deps there is isolated and does
  not touch the engine workspace. `src-tauri/src/lib.rs` holds the `tauri::Builder` (currently just the
  log plugin) — the PTY commands/events register here.
- **`ludion-mcp`** (`crates/ludion-mcp`) is the peer-client bridge the personas drive: `ludion-mcp
  --url ws://HOST:PORT` (default `ws://127.0.0.1:9411`), connecting **lazily on the first `tools/call`**
  — so it can be launched before/at the same time as the instance, and it hosts **no MCP-private
  capability** (one tool per protocol method, derived from the schema). This is why the arc adds no
  wire surface.
- **Parity gate:** nothing ships in the editor unless it already exists as a protocol command. The PTY
  host is a *native host capability*, not an engine capability — it adds no `Command` variant and no
  editor-private endpoint. Every world edit the agent or human makes is an existing protocol command.
- The mutating primitives the personas use already exist on the wire (`entity.spawn/destroy`,
  `component.set/get/remove`, `query.entities`, `render.bounds`, `render.screenshot`, `scene.save`) —
  Phase 6 delivered them; the surface is 26 methods and this arc keeps it there.
- `.gitignore` and `.loreignore` already mirror each other for runtime state (`games/*/save/` in both);
  the `.ludion/` context-file rule follows the same "ignore rules live twice" discipline.
- `editor/mockups/07-meridian-v2.html` is the layout of record (columns, order, panel heads, kit
  shelf, terminal tab chrome with the context/cache chips, session strip, status bar, closed-by-default
  Ctrl+K palette). Earlier mockups in `editor/mockups/` are prior explorations, superseded by 07.
