---
description: ludion-engine's per-file layout: what each src file is for and which spoke owns it
---

# ludion-engine layout

**Scope:** the file map of `crates/ludion-engine/src/`. The hub is `ludion devdocs ludion-engine`;
each line points at the spoke that owns the file's depth.

## Layout

- `crates/ludion-engine/src/lib.rs` — crate doc and the re-export surface; the list is the file.
- `crates/ludion-engine/src/engine.rs` — the `Engine`: `SimDriver` (which owns `World` by value), `Registry`, seed, scene ref, project root, render size, lazy GPU `Renderer`, optional window `PresentTarget` (`attach_surface`, `present_attached`), subscription table, optional `Recorder`; `execute()`, `load_inline_scene`, `set_recording`/`stop_recording`, `world_hash`, `advance_live`, `render_frame(frame, readback, present)`.
- `crates/ludion-engine/src/dispatch.rs` — one exhaustive `match` over every `Command` variant → `Dispatched { body, frame, game_frame }`. All routing.
- `crates/ludion-engine/src/commands/` — the handler logic, one module per method family (`scene`, `entity`, `sim`, `render`, `stream`, `asset`, `perf`, `prefab`, `engine_cmds`); `mod.rs` has `collapse()`, `Vec<Diagnostic>` → one wire diagnostic with `related`. Not `collapse.rs`'s `collapse()`, which turns live prefab instances into `scene.save` instance entries.
- `crates/ludion-engine/src/hierarchy.rs` — `world_transform`/`world_affine` (the derived world pose, never stored), `reparent` and `Keep`, the `L1218`–`L1225` faults and the `L1260` reparent refusal, the `hierarchy` debug-draw lines.
- `crates/ludion-engine/src/particles.rs` — the particle motion kernel (`Motion`, closed-form `position_at`) and the analytic `ParticleEmitter` evaluator (`evaluate`); pure functions of `(component, Transform, SimTime)`, no world/resource/clock reads, read fresh by `extract.rs` every frame.
- `crates/ludion-engine/src/manifest.rs` — the `ludion.toml` reader (`load`, `find`, `Manifest::voice_caps`, `L0023`/`L5004`); read by `export`, `sync` and every game host.
- `crates/ludion-engine/src/live.rs`, `crates/ludion-engine/src/egress.rs`, `crates/ludion-engine/src/diagnostics.rs` — `ludion devdocs engine-live-loop`.
- `crates/ludion-components/src/lib.rs` — `ludion devdocs engine-components`.
- `crates/ludion-engine/src/render_extent.rs`, `crates/ludion-engine/src/render_size.rs`, `crates/ludion-engine/src/render_view.rs` — `ludion devdocs engine-present`.
- `crates/ludion-engine/src/perf_live.rs`, `crates/ludion-engine/src/profile.rs` — `ludion devdocs profiling`.
- `crates/ludion-engine/src/recording.rs`, `crates/ludion-engine/src/replay.rs`, `crates/ludion-engine/src/snapshot.rs`, `crates/ludion-engine/src/bisect.rs` — `ludion devdocs engine-replay`.
- `crates/ludion-engine/src/docs.rs`, `crates/ludion-engine/src/player_profile.rs`, `crates/ludion-engine/src/transition.rs`, `crates/ludion-engine/src/query_drift.rs` — `ludion devdocs engine-content-kinds`.
- `crates/ludion-engine/src/residue_store.rs` — `ludion devdocs engine-residue`.
- `crates/ludion-engine/src/voxelizer.rs`, `crates/ludion-engine/src/voxel_sheet.rs` — `ludion devdocs engine-voxelizer`.
- `crates/ludion-engine/src/sculpt_mesh.rs` — the `kind: sculpt` loader: bytes → `check_sculpt` → `ludion_sculpt::mesh` → `MeshData` keyed by content hash, the `voxel_mesh.rs` shape; `sculpt_budget_warning` is this crate's `L1112` re-check on the meshed vertex count. The mesher itself is `ludion devdocs ludion-sculpt`.
- `crates/ludion-engine/src/assembly_mesh.rs`, `crates/ludion-engine/src/asset_cache.rs` (the `dependents` map of source-file edges), and `extract.rs`'s `part_placements` — `ludion devdocs assembly-animation`.
- `crates/ludion-engine/src/cooked_maps.rs` — `TextureSource` and the cooked-map lookup behind `Engine::set_textures` (`source` decodes PNGs, `cooked` reads the `ludion sync` store of the first GPU-supported target that has one, and falls back per map with `L1271`; a decal's maps always read their PNG, since sync cooks only what mesh documents name); a host switch like quality, never hashed. `cook_resolved == false` is the one retry lever: a rebuild, `set_textures` or an `asset.reload` of a `.ludion/cooked/` file re-resolves the lookup and re-tests the held fallbacks. Hosts: `ludion docs quality`.
- `crates/ludion-engine/src/extract.rs`, `crates/ludion-engine/src/render.rs`, `crates/ludion-engine/src/ui.rs`, `crates/ludion-engine/src/model_sheet.rs`, `crates/ludion-engine/src/sweep.rs` — `ludion devdocs engine-image-paths`.
- `crates/ludion-engine/src/collapse.rs` — save collapse: live prefab instances written back as `scene.save` instance entries (distinct from `commands/mod.rs`'s `collapse()`); `prefab.rs`, `prefab_ops.rs`, `prefab_clips.rs` — runtime `spawn_prefab` and the `PrefabLibrary`; create/revert/unpack over live instances; the clips a prefab binds.
- `crates/ludion-engine/src/builder.rs` — `EngineBuilder`, the one way to construct an `Engine` hosting a game's components and systems; `components.rs` re-exports `ludion-components` at the old paths; `instantiate.rs` — the engine-typed hooks `ludion_scene::instantiate_into` calls (naming a part, baking a placement).
- `crates/ludion-engine/src/animation_events.rs` — the per-tick `(entity, clip, event)` crossings game logic reacts to; `audio.rs` — the `AudioQueue` resource and the frame-stamped log the engine drains it into.
- `crates/ludion-engine/src/bounds.rs` — `render.bounds`: CPU world AABB + screen rect per renderable, no GPU; `debug_draw.rs` — named categories of world-space debug lines; `interpolation.rs` — render-side cuts for display-rate pose blending.
- `crates/ludion-engine/src/camera_rig.rs`, `crates/ludion-engine/src/camera_offset.rs` — the `camera_rig` system (`ludion devdocs ludion-camera`) and which `CameraOffset` extract applies, with its latched diagnostics.
- `crates/ludion-engine/src/character.rs`, `crates/ludion-engine/src/skin_bone.rs`, `crates/ludion-engine/src/parent_joint.rs`, `crates/ludion-engine/src/mesh_asset.rs`, `crates/ludion-engine/src/collider_mesh.rs` — loaders and spawn-pose passes for `kind: character` (skinned mesh + clips), `SkinBone` parts, `ParentJoint` validation (pure passes shared by `scene.load`, district switch and `check-scene`), `Mesh.shape: asset` documents, and the `ColliderMeshes` loader behind `Collider.shape: { mesh }`.
- `crates/ludion-engine/src/motor.rs`, `crates/ludion-engine/src/lane.rs`, `crates/ludion-engine/src/path_follow.rs`, `crates/ludion-engine/src/ragdoll.rs`, `crates/ludion-engine/src/scene_physics.rs` — `ludion_physics::move_character` bound to `Transform`; lanes (`kind: path` a motor is held to); path followers; `switch_to_ragdoll` for assemblies; the `physics:` block mapped to `PhysicsSettings` with the write/step/mirror triple.
- `crates/ludion-engine/src/patterns.rs` — `kind: pattern` documents, the two hashed components, the closed-form bullet evaluator and its two systems; `fonts.rs`, `font_sheet.rs` — the content root's `fonts.yaml` library and `ludion render --font`'s specimen sheet.
- `crates/ludion-engine/src/probe_bake.rs`, `crates/ludion-engine/src/probe_check.rs`, `crates/ludion-engine/src/probe_dump.rs` — the probe store's host axis and bake report; `ReflectionProbe` placement checks; `render --dump-probes`.
- `crates/ludion-engine/src/quality.rs`, `crates/ludion-engine/src/render_settings.rs`, `crates/ludion-engine/src/shutdown.rs` — the host-side quality chain (`ludion docs quality`); the `RenderSettings` and `Shutdown` request resources a game system sets for the host.
- `crates/ludion-engine/src/scene_check.rs`, `crates/ludion-engine/src/scene_session.rs` — the scene checks that need the filesystem; the sidecar `scene.save`'s conflict check reads after a dev restart.
- `crates/ludion-engine/src/field_diff.rs`, `crates/ludion-engine/src/trace.rs`, `crates/ludion-engine/src/tick_observer.rs` — shared object-key diffing for the snapshot and profile migration reports; the numeric JSON Lines trace (`load_input_script` for `sim-hash --script`); the read-only per-tick watch behind `assert_deterministic`.
- `crates/ludion-engine/src/load_observer.rs` — the clock-free seam scene load reports through (`ludion devdocs profiling`).
- `crates/ludion-engine/src/mesh_check.rs` — the mesh-asset pass of `scene.check`: every mesh, collider-mesh, mesh-texture, decal-map and grid-legend file a scene names must load (`ludion devdocs engine-image-paths`).
- `crates/ludion-engine/src/replay_compat.rs` — the content hash a replay header records and the gate that compares it (`ludion devdocs engine-replay`).
- `crates/ludion-engine/src/sprite_sheet.rs` — `ludion render --sprite` and `--atlas`: a sprite document's contact sheet and a packed atlas's overview (`ludion devdocs engine-image-paths`).
- `*_tests.rs` files are test modules of their stem.
- `crates/ludion-engine/tests/` — `dispatch.rs` (per-family happy + diagnostic paths, no GPU), `gpu_dispatch.rs`/`gpu_render.rs` (graceful-skip on `L2003`), `extract.rs`, `dump_graph_snapshot.rs` + `snapshots/`, `docs.rs`, `docs_reload.rs`, `presentation.rs` (presentation entities through snapshot, replay, extract), `transition*.rs`; all compiled as modules of `tests/it.rs` except `thread_identity.rs`.
