[package]
name = "{{name}}"
version = "0.1.0"
edition = "2024"

# An empty `[workspace]` table makes this project standalone: it does NOT join the engine
# checkout's Cargo workspace (which would fail — a package cannot belong to two workspaces).
[workspace]

[[bin]]
name = "{{name}}"
path = "src/main.rs"
# `ludion::run` is the facade's argv shell, which is the `wire` feature.
required-features = ["wire"]

[lib]
# A library alongside the binary so tests can drive the game's components, systems, and registry
# directly, and a native host links it (`ludion_game_start`). The binary (src/main.rs) is a
# three-line `ludion::run` wrapper over `game()`.
name = "{{name_ident}}"
path = "src/lib.rs"

[features]
# The desktop dev shape; the shipped (`ludion export`) build is `--no-default-features`.
default = ["wire", "window"]
# The facade's argv shell and wire listener: what `ludion::run` (the bin) needs.
wire = ["ludion/wire"]
# The game's own window (`--window`); implies the listener it serves beside.
window = ["wire", "ludion/window"]

[dependencies]
# The one dependency a game needs: `use ludion::prelude::*`, and `ludion::run` in the bin. Features
# are the game's own (above): the shipped build takes none, so the lib carries no listener and no
# window.
#
# `ludion new` fills this in one of two ways. Run from an engine checkout, it points at the engine
# by absolute path (dev flow — move the project anywhere, it still builds against the engine here).
# Run from an installed `ludion` binary, it pins the matching published version instead.
ludion = {{ludion_dep}}
# The component derives are the game's own dependencies — the facade does not re-export them.
serde = { version = "1", features = ["derive"] }
schemars = "1"

[dev-dependencies]
# The golden-image harness (with the game's registry seam) — a game's test-only dependency.
ludion-testkit = {{ludion_testkit_dep}}
# Engine types for tests that name them.
ludion-engine = {{ludion_engine_dep}}
serde_json = "1"

# Dev builds keep your game unoptimized (fast compiles, full debuggability) but build the engine
# and its dependencies optimized, so the render/streaming stack runs at full speed under a debug
# game. The `ludion` line is load-bearing: rustc's share-generics compiles the engine's generic
# hot loops (e.g. the per-frame QOI frame encode) into the facade your binary links, so without it
# a debug run streams at a fraction of the frame rate. See the engine's docs/frame-streaming.md.
[profile.dev.package."*"]
opt-level = 3
[profile.dev.package.ludion]
opt-level = 3
