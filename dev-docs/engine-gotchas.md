---
description: ludion-engine traps: keyframe alignment, project_root, asset.reload, extract and camera quirks
---

# ludion-engine gotchas

**Scope:** the traps in `crates/ludion-engine` that are not owned by a spoke. The hub is `ludion devdocs ludion-engine`; replay and snapshots are `ludion devdocs engine-replay`, the image paths `ludion devdocs engine-image-paths`.

## Gotchas

- Snapshot restore captures dead-slot generations and free-list order (the `allocator` field) and
  the held input, the pending future-frame injected inputs (`pending_input`) and the spatial index's
  filed entries (`spatial_index`), so a restore continues exactly: an `input.inject` for a later
  frame still fires, and a system scheduled before `spatial_rebuild` reads the index the previous
  tick filed.
- **A keyframe aligns by record order, never by frame number.** A recording writes its keyframe
  immediately after the delta record that crossed the interval, and a free-run recording batches
  ticks, so the keyframe's frame is wherever the batch ended. `prepare_replay` therefore keeps
  keyframes in the action list (`ReplayAction::Keyframe`, inert for a plain drive) and
  `verify_replay_keyframes` compares where the record sits.
- **Never compare a keyframe's numbers as raw JSON.** `serde_json` carries `float_roundtrip`
  workspace-wide, so a parsed `f64` is the one that was written; `bisect.rs` still normalizes every
  keyframe value through its registered component type before diffing, so a representation
  difference is not reported as state drift. Before `float_roundtrip`, a parse could land 1 ULP away
  and made breakout's committed `run.lrp` "diverge" from a keyframe it had just written.
- `Engine::new()` defaults `project_root` to `"."` — fine for tests that load an inline scene with
  no asset references, but wrong for anything loading a scene from disk or referencing textures.
  Getting the root wrong doesn't fail `scene.load`; it silently starves rendering downstream (a
  missing-texture `L1101`, surfaced once via the render-failure log dedup, not a crash) — see
  `ludion-cli`'s `check_scene::load_engine` for the construction path that gets this right.
- `asset.reload` drops the named file's CPU `AssetCache` entry and re-reads it from disk; a path the
  engine does not hold is a no-op (`asset_cache.rs`, `commands/asset.rs`). Cache keys are one lexical
  spelling (`ludion_core::paths::normalize`: separators, `.` and `..` collapsed), so any spelling of a
  file reloads every entry read from it. Beyond the cache it reloads a renderer
  shader (`L2008` without the dev flag), a registered data doc, a loaded prefab and a font table or face.
- `scene.save` round-trips entity names: a `name:` loads into the `Name` component and
  `flatten_ids` projects it back out to the entity's `name` field.
- `extract()` reads components via `world.get::<T>()` typed access, so a component inserted only
  as JSON under an unregistered name never renders; registration is what makes data visible.
- The camera looks down its local −Z; `Projection::Orthographic { height }` spans world-space
  height, width follows the viewport aspect. Zero cameras → default framing; two or more → `L2101`.
- Texture deduplication in `extract()` is by *path string* first (then content hash inside the
  texture list) — two different relative spellings of the same file decode twice.
- `dump_frame_graph_json` is re-exported here precisely so `ludion-cli` need not depend on
  `ludion-renderer` for `dump-graph`; the CLI lists `ludion-renderer` nowhere in its manifest —
  keep it that way. `dump_scene_graph_json` is the verb the CLI actually calls: `render_scene`'s
  CPU prologue (check → instantiate → extract, shared with it) plus a dump of the graph the
  extracted scene's `PostChain` selects, so `dump-graph` reports what that scene really renders
  and still needs no adapter. The wire `render.dump_graph` masks the chain the same way, by the
  engine's quality.
