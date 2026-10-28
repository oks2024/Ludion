---
description: Workspace layout, dependency edges, and the boundary rules that prevent cycles
---

# Crate graph

**Scope:** The workspace's crate inventory (built vs planned), the actual dependency edges from
each `Cargo.toml`, and the boundary rules that keep the graph acyclic and compile times fast.
Per-crate behavior lives in the topic docs (`ludion-sim`, `ludion-engine`, `ludion-cli`); the edge listing is `crate-graph-edges`;
the strict/permissive clippy split that rides on this graph is in `determinism`.

## Layout

- `Cargo.toml` — workspace root: `members = ["crates/*", "games/*"]`, resolver 3, edition 2024; shared `[workspace.dependencies]` pin every third-party version (wgpu 30, glam 0.30, schemars 1, serde_json with `preserve_order` and `float_roundtrip`, clap 4, saphyr-parser, xxhash-rust, png, tokio 1, tungstenite/tokio-tungstenite 0.24, crossbeam-channel, zstd, ...).
- `crates/ludion-core/` — ids, `Diagnostic` + `L####` code table, `SimTime`/`FrameIndex`/`TickRate`, `DeterministicRng`, `StableMap`/`StableSet`, libm math. Depends on nothing in-workspace.
- `crates/ludion-ecs/` — custom ECS + component registry (name + JSON Schema + erased serde fns).
- `crates/ludion-scene/` — strict-subset YAML parse/validate/canonical-emit + `instantiate`.
- `crates/ludion-assets/` — CPU-only asset loading (PNG, WAV) + content hashes + the debounced filesystem watcher + the cooked-texture key and KTX2 reader (`cooked`). GPU-free.
- `crates/ludion-cook/` — the texture cook's encoders (BC7, ASTC 4×4, CPU mips, KTX2 writer). Only `ludion-cli` links it; the engine never carries an encoder. Depends on core + assets.
- `crates/ludion-texgen/` — procedural textures: parses a `kind: texture` recipe and evaluates it on the CPU into the PBR map set as PNG bytes, bit-exact across architectures. Only `ludion-cli` links it. Depends on core + scene.
- `crates/ludion-sprite/` — offline sprite baking: `kind: sprite` grids + the seven CPU effects → pixels, the deterministic atlas packer, PNG → sprite-document import. Depends on core + scene + assets; GPU-free, does no IO.
- `crates/ludion-font/` — font faces: `FontFace` (glyph, advance, kerning lookups over a `kind: font` document) and the embedded 8×8 face. Depends on core + scene; pure, no IO, ships PNG bytes without decoding.
- `crates/ludion-camera/` — camera rigs: the `CameraRig` component and the pure per-tick pipeline that poses a camera (path track, springs, look-ahead, vertical anchor, timed blends, seeded trauma shake). Depends on core + ecs + scene; sim-side, GPU-free.
- `crates/ludion-sculpt/` — the sculpt mesher: exact signed distances for the six primitives and the polynomial smooth min/max that fold them. Depends on core + scene; GPU-free, does no IO.
- `crates/ludion-mesh-edit/` — deliberate, replayable edits to a mesh's texture maps: region algebra, UV rasterisation, linear-light paint, a CPU ray cast, the derived-tree reader/writer/checker. Depends on core + scene + assets; GPU-free. `ludion-cli` supplies the one thing it deliberately does not own: the model-sheet camera a pick reuses.
- `crates/ludion-gen/` — the ComfyUI generation client: recipe resolution, API-graph validation, the submit/follow/download pipeline, contact sheets, accept. Depends on core + scene + assets + font (the contact sheet's label stamp) + import (a mesh accept hands its `.glb` to the one glTF reader in-process) + ecs (the `Registry` that accept's scene emit takes — the built-in set is passed down from the CLI, never reached for here), plus `ureq` (no TLS) and `tungstenite`; **host-side, so no determinism denylist** (`ludion devdocs ludion-gen`).
- `crates/ludion-audio/` — the WAV + Ogg Vorbis music playback sink (cpal, lewton). Depends on core + assets, never on the engine.
- `crates/ludion-import/` — glTF → canonical text (scene + mesh documents + re-encoded textures). CPU-only.
- `crates/ludion-sim/` — tick loop, input scripts, run-state, `WorldHash`.
- `crates/ludion-spatial/` — deterministic 2D ground-plane collision: the `Collider` builtin component, a per-frame spatial hash (`SpatialIndex`/`spatial::rebuild`), and pinned-order overlap/raycast/k-nearest queries. Sim-side, GPU-free.
- `crates/ludion-protocol/` — wire types: envelope, `Command` surface, replay format, schema hash.
- `crates/ludion-render-graph/` — headless wgpu frame graph, dump-graph/timings JSON.
- `crates/ludion-renderer/` — the render pipeline over the frame graph, offscreen target.
- `crates/ludion-engine/` — builtin components, `extract()`, lockstep `render_scene`, `Engine` + `dispatch.rs`, recording/replay, snapshot/restore, the live driver.
- `crates/ludion-server/` — the WebSocket host: tokio transport feeding the one command queue.
- `crates/ludion-client/` — sync (blocking) Rust SDK over the wire protocol.
- `crates/ludion-mcp/` — stdio MCP adapter; a peer client (`→ client, protocol, core`) whose tool catalog is derived from `protocol_schema()`.
- `crates/ludion-window/` — the windowed host (winit + wgpu present + gamepad). A peer client too: `→ client, protocol, core, render-graph` (the shared window blit and the adapter policy), never the engine.
- `crates/ludion-shader-bindgen/` — build-time WGSL reflection (naga) generating the renderer's CPU-side uniform/BGL/vertex-input mirrors.
- `crates/ludion-testkit/` — golden harness: SSIM, diff reports, bless flow, sim assertions.
- `crates/ludion/` — the game-facing facade: `prelude` + `ludion::run`/`launch`; depends on engine **and** server **and** window (the server behind its default `wire` feature, the in-process `--window` host behind its default `window` feature).
- `crates/ludion-cli/` — the `ludion` bin (leaf).
- `games/hello-paddle/` — the example game (bin + lib): one custom component + system, mixed scene, four test kinds. `games/breakout`, `games/sokoban`, and `games/shmup-stress` are the other workspace games (`games/vigil`, the private reference game, is a detached workspace present only in the Lore tree); every game depends on the `ludion` facade only.
- `editor/protocol/` — TS protocol types generated from the JSON Schema (not a crate; checked by the schema-drift CI job).
- `examples/gallery/` — scene files + committed goldens driven by `ludion test` / testkit.

## Rules

The four boundary rules (each prevents a specific cycle — never relax them):

1. **`ludion-protocol` never names a concrete component type.** Wire payloads are
   `serde_json::Value` + registry name, validated at dispatch. The crate depends only on
   `ludion-core` — no ecs, no scene, no engine.
2. **`ludion-ecs` never knows YAML exists.** The registry stores erased serde fns; scene→ecs is
   one-directional. `ludion-ecs`'s manifest has no YAML dependency; `saphyr-parser` appears only
   in `ludion-scene`.
3. **`ludion-assets` never sees a wgpu type.** CPU bytes + content hashes only; the renderer owns
   GPU upload. A wgpu type here would drag wgpu into the sim-side tree and destroy compile times
   for content work.
4. **`ludion-engine` never depends on `ludion-server`.** The engine *does* depend on
   `ludion-protocol` (its `dispatch.rs` exhaustively matches the `Command` enum — that match is
   the parity gate) and exposes plain crossbeam channels; the server is the tokio-owning peer
   that plugs its WebSocket transport into them, never the other way around.

Supporting rules:

- Fixed-timestep *types* live in `ludion-core::time`; the loop *driver* lives in `ludion-sim`.
  No loop in core, no time types in sim.
- Per-component schemas live with the registry in `ludion-ecs`; `ludion-scene` consumes them.
- `ludion-render-graph` is a wgpu frame graph, full stop — no backend-abstraction traits. The ash
  migration seam is its declarative interface (pass declarations, resource descriptions,
  dependency edges kept free of wgpu types where it costs nothing).
- **Bins may depend on both sides of any boundary** — leaves are allowed to be fat
  (`ludion-cli` does).
- Third-party versions are pinned once in `[workspace.dependencies]`; crates opt in with
  `dep.workspace = true`. Don't declare a bare version in a member crate.

## Edges

The per-crate dependency edges from each `Cargo.toml`, the `wire`/`window`/`watch` feature shapes, where wgpu, tokio, winit, gilrs and notify are confined, and where shared logic goes: `ludion devdocs crate-graph-edges`.

## Verify

```
cargo check --workspace
cargo test --workspace
cargo tree -p ludion-assets -e normal    # must show no wgpu
cargo tree -p ludion-ecs -e normal       # must show no saphyr/YAML
cargo tree -p ludion-engine -e normal --depth 1
```

CI of record is a private TeamCity server building every revision pushed to the Lore server
(`node scripts/commit.mjs` is the send-to-CI step); the generic, VCS-agnostic gate scripts it
runs live in `ci/scripts/`. `.github/workflows/ci.yml` is the public definition of the same jobs — fmt, clippy
`-D warnings`, and the full test suite on Windows + macOS with rustc pinned by
`rust-toolchain.toml`. A job added to one must be added to the other.

## Gotchas

- Adding a dependency edge is an architecture decision, not a convenience: check this file's
  Rules first, and remember that a new crate needs a `clippy.toml`: classify it in `CRATES` in
  `scripts/clippy-denylist.mjs`, add a header comment, then run
  `node scripts/clippy-denylist.mjs --write` (see `determinism` — clippy config does not inherit
  from the workspace root, and `lists-in-step.mjs` fails on an unclassified crate).
- `ludion-cli` deliberately does **not** depend on `ludion-renderer` or `ludion-render-graph`;
  it reaches rendering through `ludion-engine` re-exports (`render_scene`,
  `dump_scene_graph_json`). Keep new render-touching CLI features on that path.
- `ludion-testkit` depends on `ludion-engine` (to render scenes), so testkit is *not* usable
  from the crates below engine; sim-level assertions (`sim_assert`) live in testkit but only
  need `ludion-sim` types — don't move testkit down the graph to "fix" that.
- `examples/gallery/` is not a workspace member (only `crates/*` are); its scenes are exercised
  through `ludion-testkit`'s gallery test and `ludion test --dir examples/gallery`.
- The **`ludion-engine → ludion-spatial`** edge runs one way only. `ludion-engine`'s
  `builtin_registry()` names `Collider` to register it, so it depends on `ludion-spatial`; the
  reverse is forbidden (it would drag the renderer/wgpu into a content-speed crate). Because
  `Transform` lives in `ludion-engine`, `ludion-spatial` cannot name it: `spatial::rebuild` is
  generic over a `GroundPosition` position component, and `ludion-engine` implements
  `GroundPosition for Transform` (collider position is `(translation.x, translation.z)`). Like every
  sim-side crate, `ludion-spatial` carries its own generated `clippy.toml` (it does not inherit the
  workspace root — see `determinism`).
