---
description: What ludion new scaffolds: each file's job, the content root, and where the wiring lives
---

# Project anatomy

**Scope:** The files `ludion new` writes and what each is for, the content root every asset path resolves against, and how `src/` grows as a game gets bigger. The scene file syntax is scene-format; the two things a game brings are components and systems.

`ludion new <name>` generated this project. The files, and what each is for:

| File | Purpose |
|---|---|
| `src/lib.rs` | The **wiring root**: `register` (the component list) + `game() -> EngineBuilder` (the ordered `.system()` list). **Start here.** |
| `src/components.rs` | The game's components (and, at starter scale, their tunables). |
| `src/systems.rs` | The game's systems (`fn(&mut World, &SystemCtx)`). |
| `src/main.rs` | The binary — three lines: `ludion::run(<game>::game())`. A packaged host with no argv calls `ludion::launch(<game>::game(), LaunchRoots { content, save })` instead, where `content` is the directory the `default_scene` path resolves against. |
| `scenes/main.yaml` | The scene (strict-subset YAML): built-in visuals + your components. |
| `scenes/model-stage.yaml`, `scenes/model-stage-night.yaml`, `scenes/effects-stage.yaml` | The Ludion editor's stage scenes (its Voxel Artist / Models and Effects views) — display surfaces a second `ludion serve` hosts, never your own scene. Built-in components only; the `subject`/`adventurer`/`target` entity names are load-bearing. Safe to delete if you don't use the editor — it re-adds them with **Add stage scenes**. |
| `tests/gameplay.rs` | Headless sim assertions + a WorldHash-determinism test (no GPU). |
| `tests/golden.rs` | A golden-image test (bless-or-skip until you commit `goldens/<os>/main.png`). |
| `tests/drift.rs` | Asserts every component in the registry survives snapshot migration (`ludion docs components`). |
| `clippy.toml` | The sim-side determinism denylist your game code inherits. |
| `ludion.toml` | The project manifest: `name`, `default_scene`, the optional `[audio]` voice caps, and the `[ios] bundle_id` `ludion export` reads (`ludion docs export`). |
| `Cargo.toml` | A **standalone** crate (empty `[workspace]`); `ludion` is a path dep on the engine. The `wire` (the listener) and `window` (the `--window` in-process window) features are declared here and default on — the dev shape; `ludion export` builds with `--no-default-features` and starts through the lib's `ludion_game_start`. |

Asset files (textures, `meshes/*.mesh.yaml`, `voxels/*.voxel.yaml`) are not scaffolded; when you add
them, they resolve relative to the **content root** — the scene's directory, so `scenes/assets/…`,
`scenes/meshes/…`, `scenes/voxels/…` — not the project root. A `meshes/*.mesh-edit.yaml` beside a
mesh document is the one exception that writes more than it reads: `ludion mesh apply` derives an
edited copy into its own directory rather than touching anything scaffolded here
(`ludion docs mesh-editing`).

`generation/` is the one folder outside the content root a project grows: `generation/recipes/` holds
committed `*.gen.yaml` recipes, `generation/workflows/` the committed API-format graphs they name,
and `generation/runs/` the scratch a run writes — ignored by both version-control systems and never
an engine input (`ludion docs generation`).

A game is a binary depending on the `ludion` facade crate. It brings two things — components and
systems — and gets everything else (scene load/validation, the deterministic sim, `WorldHash`,
recording/replay, the wire protocol, the golden harness) from the engine. Full facade tour:
`ludion devdocs ludion`.

**Code layout: definitions in modules, wiring at the root.** The starter splits `src/` into
per-concern modules — `components.rs`, `systems.rs` — glob re-exported from `lib.rs` so every item
keeps one public path (`<game>::Player`). What stays in the root is the *wiring*: `register` lists
the components and `game()` lists the systems, and that ordered `.system()` list stays in `game()`
because it is the schedule — registration order is execution order, so the root is where you read and
sequence a tick. Every system name must be unique (`L4018` at build). As the game grows you add modules (`enemies.rs`, `weapons.rs`, a dedicated
`tunables.rs`) and re-export them the same way; the root stays a thin wiring layer. The engine nudges
you when a single module concentrates too many systems — the `L1124` boot warning
(`ludion explain L1124`), the signal a module is regrowing a catch-all module. `games/shmup-stress` is a worked
small example: a `lib.rs` wiring root over `components.rs`, `systems.rs` and `tunables.rs`.

## Next

- `ludion docs components` — declaring components, and the built-in visual set.
- `ludion docs systems` — the system contract and the determinism rules.
- `ludion docs getting-started` — the starter game end to end.
- `ludion docs scene-format` — the strict-YAML scene document.
