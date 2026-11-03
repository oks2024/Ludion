# Ludion

Ludion is an LLM-first 3D game engine written in Rust. The agent is the primary developer and the
primary user; humans (level designers, game designers) use the same core through a visual editor
that is a peer client, never a privileged one.

The name: a *ludion* is a Cartesian diver, a sealed figure that responds instantly and
deterministically to external pressure. The engine's thesis is the same: apply a command over the
protocol, observe a deterministic, verifiable response.

The North Star metric: how many autonomous iterations can an agent perform (change, verify,
correct) before a human needs to look at the screen? When agent ergonomics and human ergonomics
conflict, agent ergonomics win.

## Built by agents

The engine is written by AI coding agents under human direction, and agent co-author trailers stay
in the history. [`CLAUDE.md`](CLAUDE.md) is the agents' operating manual; it is also the best map of
the codebase: conventions, crate boundaries, determinism rules and the verify loop.

The design lives in two documents: [`engine-pillars.md`](engine-pillars.md) (the why) and
[`engine-architecture.md`](engine-architecture.md) (crate graph, boundary rules, threading model,
protocol).

## Status

- Pre-1.0. The API and the wire protocol can still change.
- Windows and macOS are supported. Linux builds and passes the no-gpu test suite (proven in a container) but is not a supported target; no hosted run has confirmed it yet.
- The reference game is private, so the public samples are `games/breakout` and `games/sokoban`
  (plus `games/hello-paddle` and `games/shmup-stress`).
- Nothing is published to crates.io yet.

## Quick start

Prerequisites:

- Rust 1.94.0; `rust-toolchain.toml` pins it with rustfmt and clippy.
- Node 21 or newer for the scripts (CI uses 22).
- `cargo install cargo-nextest --locked`, once per machine, for the verify loop.

Run a sample in a window, run its tests:

```
cargo run -p breakout -- --scene games/breakout/scenes/main.yaml --window
cargo run -p sokoban  -- --scene games/sokoban/scenes/main.yaml  --window
cargo test -p breakout     # sim assertions, determinism, golden, session replay
```

Headless is the default: without `--window` the host serves the scene over the wire protocol and
prints `listening on ws://…`. The server is unauthenticated and binds `127.0.0.1:9411`; see
[`SECURITY.md`](SECURITY.md).

Agent-facing documentation is served by the CLI:

```
cargo run -p ludion-cli -- docs                  # list topics
cargo run -p ludion-cli -- docs getting-started  # print one
```

`ludion devdocs` serves the docs for editing the engine itself. More runnable material is in
[`examples/README.md`](examples/README.md) and `examples/gallery/`. The editor (Tauri and React, an
out-of-process protocol client) is described in [`editor/app/README.md`](editor/app/README.md),
which lists its prerequisites.

## Start your own game

Install the CLI from this checkout, then scaffold a project outside it (a project may not live
under `crates/` or `games/`):

```
cargo install --path crates/ludion-cli --locked
cd ..
ludion new my-game
cd my-game
cp <this checkout>/rust-toolchain.toml <this checkout>/Cargo.lock .
cargo test
cargo run -- --window
```

- The project's engine dependencies are path dependencies on this checkout, so keep it where it
  is. Nothing is on crates.io yet, and a CLI installed without a checkout behind it writes
  dependencies that cannot resolve.
- A checkout's `rust-toolchain.toml` does not reach a project outside it: copy it.
- Components derive through the game's own `serde` (`derive`) and `schemars` 1, which the scaffold
  lists because the facade does not re-export them.
- Copy `Cargo.lock` before the first build, so the project starts from the dependency versions the
  engine proved, and commit it: `ludion docs verify-loop`.

## Verifying changes

`node scripts/check.mjs` runs every local gate in parallel and prints a compact pass/fail summary;
per-step logs land in `.build/`. On a machine without the pinned GPU, use
`node scripts/check.mjs --no-gpu`. See [`CONTRIBUTING.md`](CONTRIBUTING.md) for the full loop.

Golden images are pinned to two reference machines: Windows on an RTX 3090 and macOS on an Apple M4.
Anywhere else, a byte-exact golden comparison that cannot be verified is a printed skip, not a
failure: every golden suite skips with `L2205` on an unpinned OS, and byte-exact comparisons skip with
`L2210` on a non-reference GPU (`ludion explain L2205`). SSIM-tolerant suites (game goldens, import
gates) still run and can fail on other GPUs; blessing elsewhere is refused (`L2206`) unless
`LUDION_BLESS_ANY_ADAPTER=1`.

## Continuous integration

- `.github/workflows/ci.yml` is the full gate set, run on demand (nothing runs on push) on Linux, Windows and macOS, with
  the soak behind an input.
- Pinned-GPU goldens and performance budgets run on private hardware only.

## Repository layout

| Path | Contents |
| --- | --- |
| `crates/` | The `ludion-*` workspace crates and the `ludion` facade games depend on |
| `games/` | Sample games, each a `ludion::run` wrapper binary with its scenes and tests |
| `editor/` | The visual editor, a peer protocol client |
| `docs/` | Agent docs for building a game (`ludion docs`) |
| `dev-docs/` | Agent docs for editing the engine (`ludion devdocs`) |
| `examples/` | Runnable examples and the render gallery |
| `Plan/` | Implementation plans; each plan's Status section is its shipping log |
| `scripts/` | The verify loop and the other repository scripts |

## History and private directories

The public history starts at a single initial import; earlier history is kept privately in the
project's primary version control system, [Lore](https://github.com/EpicGames/lore). Git here is a
derived mirror.

Some working directories are private and never published: `.exarch/` (agent coordination
notebooks), `ci/teamcity/` (private CI configuration), `infra/`, `Plan-Private/` (the project's
private plans) and `CLAUDE.local.md`. Public files may point into them; such a link is expected to
dangle.

## Third-party and credits

Built with DINOv3. Some example meshes were generated by models conditioned on Meta's DINOv3; see
[`THIRD-PARTY-NOTICES.md`](THIRD-PARTY-NOTICES.md).

## License

Licensed under either of [MIT](LICENSE-MIT) or [Apache-2.0](LICENSE-APACHE), at your option.

Unless you explicitly state otherwise, any contribution intentionally submitted for inclusion in the
work by you, as defined in the Apache-2.0 license, shall be dual licensed as above, without any
additional terms or conditions.
