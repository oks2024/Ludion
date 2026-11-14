# Ludion editor

A Tauri + React app under `editor/app`: an out-of-process client of the engine's wire protocol, a
peer of the CLI and the MCP adapter — never a privileged one. Run, prerequisites and the view list:
`editor/app/README.md`. View architecture: `Plan/editor-views.md`; each view's own plan is
`Plan/editor-*.md`.

Launch: `node scripts/editor.mjs <project-dir>` (`--help`).

## Hard rules

- **The parity gate.** Nothing ships in a view unless it already exists as a protocol command. No
  editor-private endpoint, no state only the editor can reach. A view that needs a capability the
  wire lacks gets the protocol command first, in `crates/` — and a new wire method is the user's
  decision, not a wave's.
- **Pipeline verbs are a host subprocess.** `import`, `generate-map`, `generate-atlas`, `gen` run as
  `ludion <verb> --json` from the Tauri host, so any client with a shell has the same capability
  (`Plan/comfy-generation.md` decision 15). Never reimplement one in TypeScript.
- **Text is the source of truth.** A view edits a committed file by splicing the value in place
  through the host, so the git diff is the one changed scalar; the host watcher reloads the engine.
  A scene bearing `generated_from:` is never written — persist to its source.
- **`editor/protocol/src/protocol.d.ts`, `editor/app/src/editor/effects/patternSchema.ts` and
  `editor/app/src/protocol/schemaHash.ts` are generated** from the engine's JSON Schema snapshots (the
  second from `crates/ludion-engine/tests/snapshots/pattern_schema.json`, the third the XXH3 of the
  protocol snapshot the editor compares at connect) — never hand-edit them;
  `npm run generate` in `editor/protocol`. The `schema-drift` gate rejects a stale one.
- **No frontend gate renders React.** `check.mjs --frontend-only` (oxlint, `tsc -b`, vitest) proves
  pure modules only, so put logic in pure modules with tests, and treat a view as *unverified in the
  GUI* until a human sitting passes. A wave that ships a view adds its falsifiable click-path
  checklist to `Plan/editor-live-smoke.md` and one line to the owed ledger — "mechanized-complete",
  never "done".

## Traps

- **A view id is also a filename.** `TerminalDrawer` reads the persona as
  `<personasDir>/<viewId>.md`, so renaming a view renames `editor/personas/<id>.md` with it or the
  drawer breaks for that view (the Sculpt → Models rename, `Plan/editor-stage-camera.md`).
- **The view name and the document kind are different names.** The Models view edits `kind: sculpt`
  documents and stages `kind: mesh` ones; `scenes/sculpts/`, `src/editor/sculpt/` and the
  `ludion sculpt` CLI kept their names.
- **Library roots differ per kind.** A shape asset resolves against the content root (the loaded
  scene's parent directory): the mesh library's root *is* the content root, while the sculpt and
  voxel libraries prepend a subdirectory. Cross-negative tests pin it; do not copy one library's
  path logic into another.
- **What stands on a stage is a function of the current selection and the committed scene, never of
  the click history** — the subject's components are captured once at dial and every selection
  builds over that baseline, or one asset's material follows the next asset onto the stage.
- `src-tauri/` is its own cargo workspace, isolated from the engine's; its gates are `tauri-fmt`,
  `tauri-clippy`, `tauri-test` in the default `check.mjs`. `src-tauri/src/pty.rs` carries
  host-specific PTY code for Windows and macOS; Linux is untested.

## Verify

```
node scripts/check.mjs --frontend-only   # oxlint + tsc -b + vitest, ~10 s
node scripts/check.mjs                   # adds the Tauri host's Rust gates and schema-drift
```
