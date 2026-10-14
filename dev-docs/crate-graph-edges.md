---
description: Actual workspace dependency edges per Cargo.toml, feature gates, and where wgpu/tokio/winit appear
---

# Crate graph — edges

**Scope:** the in-workspace dependency edges as the manifests declare them, the optional-feature
shapes, and which third-party stacks stay confined to which crate. The inventory and the boundary
rules are `ludion devdocs crate-graph`; the clippy split that rides on this graph is `ludion devdocs determinism`.

## Layout

Actual in-workspace edges, from each member's `Cargo.toml` (arrow = depends on):

```
ludion-core          → (none)                          [glam, libm, serde]
ludion-ecs           → core                            [schemars, serde, serde_json]
ludion-scene         → core, ecs                       [ryu, saphyr-parser, serde_json]
ludion-assets        → core                            [png, xxhash-rust, ktx2; watch†: notify, crossbeam-channel]
ludion-cook          → core, assets                    [intel_tex_2, ctt-astcenc, ktx2]  (encoders confined here)
ludion-font          → core, scene                     []
ludion-sim           → core, ecs                       [serde, serde_json, xxhash-rust]
ludion-spatial       → core, ecs                       [schemars, serde, serde_json]
ludion-protocol      → core                            [schemars, serde, serde_json, xxhash-rust, zstd]
ludion-render-graph  → core                            [wgpu, pollster, png, serde_json]
ludion-renderer      → core, render-graph, assets, font [wgpu, bytemuck]
ludion-engine        → core, ecs, scene, sculpt, assets, font, sim, spatial, renderer, protocol
                                                       [crossbeam-channel, schemars, serde, serde_json]
ludion-server        → core, engine, assets†, protocol [tokio, tokio-tungstenite, futures-util,
                                                        crossbeam-channel, serde_json]
ludion-client        → core, protocol                  [tungstenite, serde_json]
ludion-testkit       → core, ecs, engine, sim, assets, audio, protocol, scene
                                                       [crossbeam-channel, png, saphyr-parser, serde,
                                                        serde_json, toml]
ludion               → core, ecs, sim, spatial, protocol, scene, engine, audio, server*, window**
                                                       [clap*, ctrlc, crossbeam-channel, serde_json,
                                                        winit**, wgpu**]   (facade)
                       * behind the default `wire` feature   ** behind the default `window` feature (implies `wire`)
                       † `ludion-assets`' opt-in `watch` feature — enabled by server and cli only
ludion-cli   (bin)   → ludion, core, ecs, engine, assets†, protocol, scene, sim, testkit, window, …
                                                       [clap, serde_json, toml]
hello-paddle (bin+lib) → ludion   (+dev: testkit, engine, client, protocol, core, serde_json)
```

## Rules

`ludion` is the one crate allowed to depend on both `ludion-engine` and `ludion-server` — it unites
them behind a game's `main` (the engine still never depends on the server). Games depend only on the
facade at runtime, and on `ludion-testkit` (+ `ludion-engine` for `replay_hashes_with`) as
dev-dependencies. `games/*` are workspace members; `examples/*` are not.

wgpu appears in `ludion-render-graph`/`ludion-renderer` and, behind the facade's default `window`
feature, directly in `ludion` too (the surface `--window` presents into); tokio only in
`ludion-server`; winit behind that same `window` feature (its event loop runs on the facade's
`present.rs`, not only inside `ludion-window`); gilrs only in `ludion-window`; and `notify` only
behind `ludion-assets`' `watch` feature — so a facade built `--no-default-features` (the mobile
shape, `Plan/ios-target.md`) carries none of the five; `scripts/mobile-check.mjs` asserts it.
Everything above the `ludion-render-graph` line on the left column is CPU-only, which is what keeps
`cargo check` fast for content and sim work. Dev-dependencies add two
test-only edges the normal graph forbids nothing about: `ludion-server → client, testkit`
(the over-the-wire SSIM/parity tests live in the server's integration tests), and
`ludion-testkit → ludion` (the gallery cycle example's host, `examples/character_cycle.rs`).

The `editor/` Tauri + React app and `ludion-mcp` both ship; the MCP adapter derives its whole tool
catalog from `protocol_schema()`, so it cannot hold an editor-private endpoint (the parity gate).
The architecture doc's graph (§2) describes the *end state* — e.g. it lists
`ludion-testkit → engine, client`; the current manifests intentionally lack the testkit → client edge
(the wire tests sit in `ludion-server`'s dev-deps instead).

Where shared logic goes: domain logic lives in the owning `ludion-*` crate; `ludion-engine` is
glue (`dispatch.rs` routing only); `ludion-cli` modules are thin `run()` wrappers.
The builtin component registry is defined once in `ludion-engine` and shared downward.
