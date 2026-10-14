# API freeze pass — client-facing crates (Phase 9 Wave 4)

Executed checklist for the seven publication-facing crates: **ludion-protocol, ludion-server,
ludion-client, ludion-testkit, ludion (facade), ludion-cli, ludion-mcp**. Audit-and-fix per
`Plan/phase-9-hardening.md` decision 4. Companion to `api-freeze-*.md` from the concurrent agents
auditing the other crates.

## Cross-cutting: `#[non_exhaustive]` application

Applied to the three response-side wire types clients *consume*, per decision 4:

- `ResponseBody` (`ludion-protocol/src/response.rs`)
- `Event` (`ludion-protocol/src/event.rs`)
- `Response` — the response *envelope* struct (`ludion-protocol/src/envelope.rs`); reserves room for
  a future additive field, consumed via field access + the `ok`/`ok_at`/`error` constructors.

**Deliberately NOT applied to `Command`** — the exhaustive dispatch match in `ludion-engine` IS the
parity gate, so a variant without a handler must stay a compile error. A source note to that effect
sits directly above the enum.

**The rationale notes are plain `//` comments, not `///` docs.** A container's doc comment feeds
schemars' `description` into the exported schema, so a `///` edit on any of these types would MOVE the
protocol `schema_hash` and force the full additive-rev re-bless (snapshot → `EXPECTED_SCHEMA_HASH` →
TS regen → re-record `games/breakout/run.lrp`, which `ludion-engine`'s `check_compatible` hard-rejects
on hash mismatch). Since `#[non_exhaustive]` is an attribute (schema-neutral) and the wire types
already carry excellent doc-with-example content, keeping the notes as `//` comments delivers the
documentation intent with **zero schema movement and zero re-bless**. Verified: the schema snapshot
test and the full `schema-drift` gate both report *no drift* after all edits.

**Match-site fallout: exactly one site.** The whole workspace + `games/*` was compiled with `--tests`
to surface every broken match. Only `ludion-engine/src/egress.rs::event_channel` (a cross-crate total
match over `Event`) needed a wildcard; it now carries an honest `unreachable!` panic (the engine
constructs every event it emits, so an unmapped variant is an engine bug, not runtime input) and its
doc was updated. **No fallout in client/cli/mcp/server**: every existing match on `ResponseBody`
there already had a defensive catch-all `other =>` arm, and the `WireResult` matches stay exhaustive
(`WireResult` is not `#[non_exhaustive]`).

## Per-crate

### ludion-protocol
- **Naming:** regular; no renames.
- **Docs/examples:** already exemplary (every wire type has a runnable doctest). No `///` edits, to
  hold the schema hash (see above). Added `//` rationale notes on `ResponseBody`, `Event`, `Response`,
  `Command`.
- **`pub` audit:** clean — every `pub` item is intentional wire surface; `schema_proxy` is correctly
  `pub(crate)`.
- **`#[non_exhaustive]`:** `ResponseBody`, `Event`, `Response` (applied); `Command` (documented
  exclusion).
- **Doc warnings:** zero.
- **Schema:** unmoved (snapshot + schema-drift both green).

### ludion-server
- **Naming:** regular; no renames.
- **Docs:** fixed two rustdoc warnings in `src/lib.rs` — a redundant explicit link target
  (`[`ServerHello`](…)` → `[`ServerHello`]`, resolves via the crate's `use`) and a link to the private
  `delivery` module (delinked to prose "the private `delivery` module").
- **`pub` audit:** clean — `Server`, `CommandSink`, `command_sink`, `watch_assets`, `wire_shutdown`,
  `shutdown` are the intended host API; `connection`/`delivery` are private modules.
- **`#[non_exhaustive]`:** none (no consumed enums own by this crate).
- **Doc warnings:** zero.

### ludion-client
- **Naming:** regular; no renames. Typed conveniences (`sim_hash`, `sim_step`, `screenshot`,
  `subscribe`, `unsubscribe`, `next_event`, `next_frame`) are consistent verb forms over the `call`
  primitive.
- **Docs:** fixed two "redundant explicit link target" warnings in `src/lib.rs`
  (`[`Hello`](…)` and `[`FrameHeader`](…)` → shorthands, both resolve via `use`). `no_run` doctests on
  the network-requiring API (`connect`, `subscribe`, `next_event`) are appropriate and kept.
- **`pub` audit:** clean — `Client`, `CallOk`, `Screenshot`, `ReceivedFrame` and their fields are the
  intended API.
- **`#[non_exhaustive]`:** none owned here; its `ResponseBody` matches already had catch-all arms, so
  no fallout.
- **Doc warnings:** zero.

### ludion-testkit
- **Naming:** regular; no renames.
- **Docs:** fixed three "links to private item" warnings — `image::read_golden` (golden.rs),
  `tile_bounds` and `region_name` (ssim.rs) delinked to plain code spans, since the referents are
  private. The lib's two module-level examples (one `no_run` golden test, one pure SSIM compare) are
  retained.
- **`pub` audit:** clean — the re-export set in `lib.rs` plus the `audio`/`drift`/`golden`/`report`/
  `sim_assert`/`ssim`/`suite` modules are the intended harness API; `image` is private.
- **`#[non_exhaustive]`:** none.
- **Doc warnings:** zero.

### ludion (facade) — the most-important crate
- **Naming:** regular.
- **Docs/examples:** the crate-level doc is the primary game-author example and already walks the full
  `Engine::builder` → `.components()` → `.system()` → `.default_scene()` → `ludion::run` path as a
  complete `no_run` program; the prelude doc enumerates exactly what the glob brings and what it
  deliberately omits (the `serde`/`schemars` derive macros). Fixed the one rustdoc warning: the
  unresolved `[`Engine::builder`]` intra-doc link now points at `crate::prelude::Engine::builder`
  (the type is re-exported through the prelude, not imported into `lib.rs` scope).
- **`pub` audit:** clean — only `run` and the `prelude` are public; everything else in `lib.rs`
  (arg struct, shutdown plumbing) is private.
- **`#[non_exhaustive]`:** none.
- **Doc warnings:** zero.

### ludion-cli
- **Naming:** regular. `crates/ludion-cli/src/{new,docs,profile}.rs` + `build.rs` were reworked this
  week and audited as-is; no renames.
- **Docs:** fixed two unresolved intra-doc links — `[`Engine::set_recording`]` → full path
  `ludion_engine::Engine::set_recording` (replay.rs), and a stale `[`wait_for_ctrl_c`]` →
  `[`wait_for_stop`]` (serve.rs; the function had been renamed).
- **Bin-name collision (the known warning):** the `ludion` CLI bin and the `ludion` facade lib share
  the crate name, so `cargo doc` writes both to `doc/ludion/` and warns. Resolved with `doc = false`
  on the `[[bin]]` target in `Cargo.toml` (with a comment) — the bin has no public API to document
  and would only restate the facade lib's docs; the lib owns that doc path. Verified: no CLI
  doctests were lost (the bin has none).
- **`pub` audit:** N/A for a bin (not depended upon); internal command modules are appropriately
  scoped.
- **Doc warnings:** zero (including the collision, checked by building `-p ludion -p ludion-cli`
  together).

### ludion-mcp
- **Naming:** regular; internal items are `pub(crate)` throughout (`McpServer`, `Session`, `Status`,
  `error_code`, tool types).
- **Docs:** no warnings; the crate-level doc documents the tool surface, connection lifecycle, and
  registration. `success_content`/`session.status` match `ResponseBody` with a partial `if let` /
  a wildcard arm respectively, so `#[non_exhaustive]` produced no fallout here.
- **`pub` audit:** clean — nothing over-exposed (it is a bin; the surface is `pub(crate)`).
- **Doc warnings:** zero.

## Schema hash

**Unmoved.** No re-bless was needed or performed. All doc-quality edits were confined to non-wire
types or to `//` comments / intra-doc-link targets on wire types (link targets are not part of the
schemars `description` text). Confirmed by `cargo test -p ludion-protocol --test schema_snapshot`
(green before and after) and `ci/teamcity/schema-drift.ps1` reporting `schema drift: none`.

## Verification

- `cargo doc --no-deps -p <crate>` for all seven: **zero warnings**, including the bin-name collision.
- `cargo clippy --all-targets -- -D warnings` on the seven: clean (this promotes `missing_docs` to an
  error, so doc *presence* is also proven).
- Doctests green for all seven.
- `cargo test` green for six of the seven crates (protocol, server, client, testkit, ludion, mcp).
- `ludion-cli`: the one integration test `dev_reloads_and_preserves_state_across_a_real_edit`
  (`tests/dev.rs`) is flaky **only under concurrent execution** — it scaffolds into the hardcoded
  path `target/tmp/dev_session/dev_game`, and the two concurrent Wave-4 agents each run
  `check.mjs --rust-only`, whose preamble `rm -rf target/tmp/dev_session` deletes the other's
  in-flight game dir (closing the `ludion dev` child's stdout). Not caused by this wave: the scaffold
  template references none of the `#[non_exhaustive]` types, the whole workspace + `games/*` compiled
  clean with `--tests`, and clippy is green. It passes in isolation.

## Left deliberately

- **No `///` edits to wire-type container docs**, to hold the schema hash and avoid the run.lrp
  re-record. The wire types were already well-documented; the marginal doc gain did not justify a
  full protocol re-bless.
- **No renames** anywhere — naming was already regular across the seven crates.
