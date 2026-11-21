---
description: What ludion new scaffolds: each file's job, the content root, and where the wiring lives
---

# Project anatomy

**Scope:** The files `ludion new` writes and what each is for, how to get a `ludion` CLI that matches the engine, the content root every asset path resolves against, and how `src/` grows as a game gets bigger. The scene file syntax is scene-format; the two things a game brings are components and systems.

`ludion new <name>` generated this project. The files, and what each is for:

| File | Purpose |
|---|---|
| `src/lib.rs` | The **wiring root**: `register` (the component list) + `game() -> EngineBuilder` (the ordered `.system()` list). **Start here.** |
| `src/components.rs` | The game's components (and, at starter scale, their tunables). |
| `src/systems.rs` | The game's systems (`fn(&mut World, &SystemCtx)`). |
| `src/main.rs` | The binary — three lines: `ludion::run(<game>::game())`. A packaged host with no argv calls `ludion::launch(<game>::game(), LaunchRoots { content, save })` instead, where `content` is the directory the `default_scene` path resolves against. |
| `scenes/main.yaml` | The scene (strict-subset YAML): built-in visuals + your components. |
| `scenes/model-stage.yaml`, `scenes/model-stage-night.yaml`, `scenes/effects-stage.yaml` | The Ludion editor's stage scenes (its Voxel Artist / Models and Effects views) — display surfaces a second `ludion serve` hosts, never your own scene. Built-in components only; the `subject`/`adventurer`/`target` entity names are load-bearing. Safe to delete if you don't use the editor — it re-adds them with **Add stage scenes**. |
| `CLAUDE.md` | Yours to edit; the engine never compares or rewrites it. |
| `LUDION-GUIDE.md` | A copy of `ludion docs engine-guide` made at scaffold time. `ludion docs` warns `L0036` when it differs from the CLI's; `ludion new --refresh-guide` rewrites it. |
| `scripts/ludion.mjs` | A Node wrapper that builds and runs this engine's CLI, and the `link-engine` verb that (re)creates the engine link (written when the engine is a checkout; see below). |
| `tests/gameplay.rs` | Headless sim assertions + a WorldHash-determinism test (no GPU). |
| `tests/golden.rs` | A golden-image test (bless-or-skip until you commit `goldens/<os>/main.png`). |
| `tests/drift.rs` | Asserts every component in the registry survives snapshot migration (`ludion docs components`). |
| `clippy.toml` | The sim-side determinism denylist your game code inherits. |
| `ludion.toml` | The project manifest: `name`, `default_scene` (equal to `game()`'s, `L0038`), the optional `[[import]]`, `[cook]` and `[audio]` tables, and the `[ios] bundle_id` `ludion export` reads (`ludion docs export`). |
| `.gitignore` | `target/`, `.ludion/` (also the engine link, below), `generation/runs/` and `scenes/save/` (the player's profile). Add every `[[import]] to` root: `ludion sync` derives those trees and nothing else ignores them. |
| `Cargo.toml` | A **standalone** crate (`[workspace]` with `exclude = [".ludion"]`); `ludion` is a path dep on the engine, relative through `.ludion/engine` (the engine link, below). The `wire` (the listener) and `window` (the `--window` in-process window) features are declared here and default on — the dev shape; `ludion export` builds with `--no-default-features` and starts through the lib's `ludion_game_start`. |

## Getting the ludion CLI

`node scripts/ludion.mjs <verb>` (for example `node scripts/ludion.mjs docs`) builds the CLI of the engine this project's path dependency names, into the project's own `target/ludion-cli` so it never shares or links against the engine checkout's `target/`. It always matches the engine the game builds against.

Or install one: `cargo install --path <engine>/crates/ludion-cli`, and reinstall whenever the engine moves — the `L0036` warning from `ludion docs` is the tell.

`ludion check-scene` and `ludion render` link no game code, so they know only the engine's components. A scene that uses your own needs the game's binary: `cargo run -- --scene <path> --check-scene`.

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

## The engine link

A project scaffolded from an engine checkout commits no absolute path. Its `Cargo.toml` path deps and `scripts/ludion.mjs` name `.ludion/engine`, a gitignored directory link (a junction on Windows, a symlink elsewhere) to this machine's engine checkout. `ludion new` creates it, so the scaffolding machine needs no extra step. On a fresh clone, or after the engine moves, run once per machine:

```
node scripts/ludion.mjs link-engine <engine checkout>
```

It refuses a path that is not an engine checkout (`L4391`) and a real directory at `.ludion/engine` (`L4392`, never deleted). A missing or dangling link makes the wrapper fail with `L4390`; cargo itself reports an error naming `.ludion/engine/…/Cargo.toml`. `ludion new` reports `L4393` if it could not run the link step (Node missing); the files stay.

**Projects scaffolded before the link** carry absolute engine paths. To migrate: replace the three engine deps in `Cargo.toml` with `{ path = ".ludion/engine/crates/<crate>" }` (keep each dep's other keys), add `exclude = [".ludion"]` under `[workspace]`, replace `scripts/ludion.mjs` with a fresh `ludion new` copy, run `node scripts/ludion.mjs link-engine <engine checkout>`, and make sure `.ludion/` is in every ignore file the project's version control reads.

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
