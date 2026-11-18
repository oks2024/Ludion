# API Freeze Pass — Renderer/Engine Group (Wave 4, Phase 9)

Quality audit of the public API of seven crates ahead of the 0.1.0 crates.io publication,
per `Plan/phase-9-hardening.md` decision 4. Audit-and-fix, not redesign.

Crates: **ludion-assets, ludion-render-graph, ludion-renderer, ludion-engine, ludion-window,
ludion-audio, ludion-import.**

Per decision 4, **no `#[non_exhaustive]`** was applied in these crates — it goes only on the
protocol response-side enums and core's `Diagnostic`/`CodeEntry`, owned by other agents.

## Done criteria (all green)

- `cargo test` green for each of the seven crates (doctests prove the examples).
- `cargo doc --no-deps -p <crate>` ZERO warnings for all seven.
- `node scripts/check.mjs --rust-only` → ALL GREEN (fmt, clippy `-D warnings`, test, explain, docs).

The lone red seen during the run was a pre-existing environmental flake in `ludion-cli`'s `dev`
integration test — a stale `target/tmp/dev_session/dev_game` directory a prior interrupted run
left behind (`remove_dir_all` swallows the failure, then `ludion new` refuses the existing dir).
Not in scope, not caused by these edits (doc/visibility-only, no `ludion-cli` code touched);
passes cleanly once the stale dir is removed.

---

## ludion-assets

- **Checked.** GPU-free by boundary rule (verified: no `wgpu` anywhere). Naming regular
  (`load_bytes`/`load_png`/`load_wav`, `decode_png`/`decode_wav`, `encode_png`, `content_hash`).
  Public types `AssetBytes`, `Image`, `AudioPcm`, `WatchHandle` and fn `watch` all documented;
  module-, `content_hash`-, `encode_png`-, `load_png`- and `watch`-level runnable examples present
  and correct. No accidental `pub` (private helpers already private; `watch`/`WatchHandle`
  re-exported deliberately).
- **Changed.** Nothing.
- **Left.** As-is — this crate is already at the target bar.

## ludion-render-graph

- **Checked.** The ash-migration seam holds: the declarative half (`graph`, `schedule`, `resource`,
  `dump`) names no `wgpu` type; wgpu is confined to `execute`/`readback`/`timings`. Two runnable
  examples at crate level (no-GPU inspect + `no_run` render) plus `build_graph`. Naming regular.
- **Changed (doc warnings, 6 fixed).**
  - `schedule` is both a `pub mod` and a re-exported `pub fn`, so every bare `[`schedule`]` link
    was ambiguous or unresolved. Disambiguated each by intent: module references →
    `[`schedule`](mod@…)` (lib.rs, execute.rs); the "…is scheduled into…" / "call schedule to…"
    function references → `[`schedule`](fn@crate::schedule)` (graph.rs ×2).
  - `readback.rs` module doc linked private `[`read_texture_to_image`]` (a `pub(crate)` fn) →
    demoted the link to a plain code span.
  - `resource.rs` linked `[`crate::alias`]` (no such path; the aliaser is a private `fn alias` in
    `schedule`) → repointed to the public `[`Aliasing`](crate::Aliasing)`.
- **Left.** The seven `pub mod`s are the intended public API surface (the backend seam); no
  accidental `pub` found within them.

## ludion-renderer

- **Checked.** Consumes only `RenderScene` (no ECS, no scene, no asset paths) — boundary rule
  intact. Crate-level no-GPU dump example + `no_run` render example; `Renderer` (entry point) and
  `RenderScene`/`RenderScene::empty` documented with examples. Naming regular.
- **Changed (accidental `pub` — 6 demotions).** Two private, non-re-exported modules exposed `pub`
  items used only within the crate (via `render.rs`); narrowed to `pub(crate)`:
  - `font.rs`: `CELL`, `glyph_count`, `glyph_cell`, `atlas_rgba`.
  - `mesh.rs`: `MeshVertex`, `vertices`.
  No callers outside the crate (grep-verified); no doc-link fallout (the referencing items are
  themselves crate-internal).
- **Left.** `scene.rs` / `render.rs` re-exports (`MeshData`, `MeshInstance`, `MeshShape`,
  `RenderScene`, `MAX_LIGHTS`, `Renderer`, …) are the deliberate public surface.

## ludion-engine

- **Checked.** Boundary rules hold (no `ludion-server` dep; the `dispatch` exhaustive match is the
  parity gate — left deliberately exhaustive, no `non_exhaustive`). Crate-level example present;
  `EngineBuilder`, `Engine::builder`, `dispatch`, and the profile driver documented. Naming regular
  (`render_scene`/`render_scene_with_registry`, `replay_hashes`/`replay_hashes_with`,
  `profile_scene`). `profile.rs` and its `ScheduleObserver` seam (Wave 2) audited — the one
  wall-clock site is documented and its determinism-neutrality asserted by test.
- **Changed.**
  - **Example added.** `profile_scene` (a primary entry point) gained a `no_run` doctest.
  - **Doc warnings (24 fixed).** All were narrative doc comments linking to intentionally private
    items or to non-dependency crates (which engine *cannot* link by boundary rule). Repointed or
    demoted to plain code spans:
    - private modules/items → plain code: `replay` (→ linked public `replay_hashes` instead),
      `Engine::build_schedule`, `crate::commands`, `crate::recording` (×2), `Engine::step_ticks`,
      `Engine::load_scene_document`, `emit_egress`, `[module docs](crate::engine)` (→ `crate`).
    - non-dependency crates → plain code: `ludion::run` (×6, the downstream facade),
      `ludion_testkit::assert_components_drift_safe`, `ludion_server::Server::wire_shutdown`.
    - resolved to correct paths: `[`StampedCommand`]` → `crate::live::StampedCommand`;
      `[`EngineBuilder`]` → `crate::EngineBuilder`; `[`Sender::is_full`]` →
      `crossbeam_channel::Sender::is_full`.
    - invalid link syntax `crossbeam_channel::bounded(1)` (a call, not a path) → plain code (×2).
    - `profile.rs` redundant explicit targets `[`ScheduleObserver`](ludion_ecs::ScheduleObserver)`
      → `[`ScheduleObserver`]` (already in scope) (×2).
- **Left.** No accidental `pub`: every crate-root re-export is intended API; private modules stay
  private. `Command`/`dispatch` left exhaustive by design (decision 4).

## ludion-window

- **Checked.** In-process peer protocol client (no privileged renderer access) — the parity pillar
  is respected. `run` + `WindowOptions` documented with a `no_run` example; the `StreamSink` /
  `run_frame_stream` headless seam documented.
- **Changed.**
  - **Accidental `pub` (1 demotion).** `reconnect::STREAM_POLL` was `pub(crate)` but used only
    within `reconnect.rs` → plain private `const`.
  - **Doc warnings (3 fixed).** `[`reconnect`]` (private module) → plain code; `[`STREAM_POLL`]` on
    the public `StreamSink::keep_running` doc → reworded to prose ("one streaming-poll interval");
    redundant explicit target `[`Client`](ludion_client::Client)` → `[`Client`]`.
- **Left.** As-is otherwise.

## ludion-audio

- **Checked.** Correctly downstream of every determinism gate (drains events, silent without a
  device). `AudioSink` (the sole public type) documented with a `no_run` example.
- **Changed.** **Doc warning (1 fixed):** redundant explicit target
  `[`AudioEvent`](ludion_core::audio::AudioEvent)` → `[`AudioEvent`]` (already imported).
- **Left.** As-is otherwise.

## ludion-import

- **Checked.** Deterministic glTF→canonical-text compiler; module doc carries the exhaustive
  mapping table and the determinism guarantee. `import` (entry), `Import`/`Import::write`,
  `MeshArtifact`, `TextureArtifact`, and the `SCENE_FILE`/`MESH_DIR`/`TEXTURE_DIR` consts all
  documented. Naming regular. No `wgpu` dependency (re-encode delegates to `ludion_assets`).
- **Changed.** **Example added.** `import` (the entry point) gained a runnable doctest compiling a
  one-node glTF to a single entity (models the crate's own integration tests, so it is robust).
- **Left.** `Import::write` documented without a doctest — it writes a directory tree to disk
  (needs a temp dir and a fully built plan); the module example plus the `import` doctest cover the
  primary workflow, so a filesystem-touching doctest here would be noise, not documentation.

## Addendum — surface tags (2026-11-18)

`ludion-physics`: `QueryHit`, `PointHit`, `GroundHit` and `SurfaceHit` gain a public `surface: Option<SurfaceId>` field. An additive change, not `#[non_exhaustive]`: no game constructs these literals (`Plan/surface-tags.md` decision 6).
