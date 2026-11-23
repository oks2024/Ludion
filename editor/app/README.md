# Ludion editor

The Ludion editor is a Tauri + React application: an out-of-process client of the engine's wire
protocol. It streams frames, sends commands, and reads state over the same WebSocket+JSON protocol
any CLI or MCP client uses. It is a peer client, never a privileged one — the parity gate holds
here as everywhere: nothing ships in this UI unless it already exists as a protocol command.

## Prerequisites

- Rust, pinned by the root `rust-toolchain.toml` — currently `1.94.0`, with `rustfmt` and `clippy`.
- Node.js, current LTS (no engine constraint is declared in `package.json`).
- The Tauri CLI, invoked through `npm run tauri` (installed as a dev dependency; no global install
  needed).
- Platform build dependencies for Tauri (WebView runtime, linkers, etc.) — follow the official
  prerequisites page: https://tauri.app/start/prerequisites/

## Run

Install once, in `editor/app` (and `editor/protocol`, only if regenerating `protocol.d.ts`):

```
npm ci                       # in editor/app
npm ci                       # in editor/protocol, only if regenerating protocol.d.ts
```

Then launch the editor from the repo root with `scripts/editor.mjs`:

```
node scripts/editor.mjs <project-dir>   # from anywhere; `.` when you're already inside the project
node scripts/editor.mjs                 # no argument: reopens whichever project was last open
node scripts/editor.mjs --help          # usage
```

`<project-dir>` is a Ludion project — a directory holding `ludion.toml`, what `ludion new` creates
(the engine checkout itself is not one). The wrapper resolves it to an absolute path and runs the
Tauri CLI's `tauri dev` with `--project <abs>` for the app; the editor's boot probe validates the
argument and persists it for next time.

One dev editor per machine: `tauri dev` pins Vite to port 1420 with `strictPort`, so a second dev
editor, from this checkout or any other, fails at start. A second editor beside a dev one must be a
packaged build.

The editor attaches to an engine on any host (a LAN engine, the iOS device build), so the CSP's
`connect-src` admits any WebSocket origin (`ws:` and `wss:`). The guard against foreign code is
`script-src 'self'` plus the pinned dev port.

### Attaching to a server you started yourself

The editor is a client of the engine's wire protocol, not the only way to host one. To attach to
an engine you started separately — any scene, hosted over the wire:

```
cargo run -p ludion-cli -- serve --scene <path/to/scene.yaml>
```

This binds a WebSocket host, printing `listening on ws://127.0.0.1:9411` on its first stdout line.
The app defaults to `ws://127.0.0.1:9411` (`DEFAULT_URL` in `src/App.tsx`). To attach elsewhere,
edit the **Server URL** field in the connect bar — it's a runtime UI input, not a build-time
setting, so no rebuild is needed to point at a different host or port (e.g. one started with
`--addr`).

## Views

The view switcher (`src/shell/views.ts`) is a registry of workspace presets, each targeting one of
the editor's personas (`editor/personas/`):

- **Level Designer** — scene authoring: hierarchy, viewport, inspector.
- **Game Designer** — tunables and declarative text data, not code.
- **Voxel Artist** — voxel model editing, with a live asset-reload watcher; also stages `kind:
  assembly` rigs read-only, with a parts panel (hierarchy, pivot crosshair, frame-on-part) and a clip
  strip (play/pause/scrub, tick-quantized) over the existing `Animation` component.
- **Models** — implicit-surface model editing (kind: sculpt), sharing the model stage and its asset-reload watcher, and previews skinned `kind: assembly` (`mesh:`) clips.
- **Effects** — particle emitter authoring.
- **Lighter** — lighting and environment.
- **Art Director** — palette and visual-consistency review.
- **Performance** — profiling and budget surfaces.

Every declared view is built; `enabled: false` would render a disabled tab with a tooltip, but none
are currently disabled at the view level (see Known gaps for narrower, in-view restrictions).

## Verify

```
node scripts/check.mjs --frontend-only   # oxlint, tsc -b, vitest run
npm test                                 # vitest run, same as above, run directly in editor/app
```

The default `node scripts/check.mjs` (no flags) also runs the Tauri host's Rust gates against
`src-tauri`: `tauri-fmt`, `tauri-clippy`, and `tauri-test`.

## Layout

- `src/` — the React application.
- `src-tauri/` — the Tauri host, in Rust; its own cargo workspace, isolated from the engine's.
- `../protocol/` — TypeScript protocol types generated from the engine's JSON Schema snapshot;
  never hand-edit `src/protocol.d.ts`, run `npm run generate` in that package instead.
- `../mockups/` — static HTML mockups exploring visual design directions for the same product
  state, plus `SPEC.md`, the shared spec they're all faithful to.
- `../design/` — design-stage assets (currently a `mockups/` subdirectory).
- `../personas/` — one Markdown file per view, describing the user it's designed for.

## Known gaps

- The Ctrl+K command-palette chip in the view switcher is a placeholder — it does not open a
  palette (`src/shell/ViewSwitcher.tsx`).
- The Effects view's Pattern tab is disabled (`src/editor/effects/EmitterPanel.tsx`): it waits on
  the engine reading a `kind: pattern` file, which nothing in this checkout does yet.
- Windows and macOS are the supported hosts. `src-tauri/src/pty.rs` carries host-specific PTY code
  for both; Linux is untested.
