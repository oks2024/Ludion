# Plan: Per-machine engine location

> **Shipped (W1) — verification in the Status section.** Driven by an external game's request: a
> project scaffolded by `ludion new` committed an absolute engine path, so a second machine could not
> build it. Game-facing doc: `ludion docs project-anatomy`.

## Problem

A checkout-flow `ludion new` wrote the engine checkout's absolute path into `Cargo.toml` and
`scripts/ludion.mjs`. Committing that project (to an external game's repository, say) published one
machine's path, and a clone on another machine did not build until someone edited both files by
hand. The engine location is a per-machine fact and must not be committed.

## Goal

The committed project names `.ludion/engine`, a gitignored directory link (a junction on Windows, a
symlink elsewhere) to this machine's engine checkout. `ludion new` creates the link, so the
scaffolding machine needs no extra step; a fresh clone runs
`node scripts/ludion.mjs link-engine <engine checkout>` once. Installed flow (version deps) is
unchanged.

## Design decisions

1. **Git dependency plus `[patch."<url>"]` in a gitignored `.cargo/config.toml` — rejected.**
   Measured in a scratch project (cargo 1.94): a patched build still fetches the git repository into
   `CARGO_HOME/git/db`, so every fresh machine needs network and downloads the full public mirror;
   `Cargo.lock` records `source = "git+…#<rev>"` unpatched and no source patched, so the lock flips
   between a patched and an unpatched machine; the public mirror lags the source of truth, so an
   unpatched build silently compiles an older engine.
2. **`[patch.crates-io]` over version deps — rejected.** An unpatched machine would resolve `ludion`
   on crates.io, which this project has not published (a later, separate owner decision); it would
   also pre-empt that decision.
3. **Chosen: committed relative path deps on `.ludion/engine/crates/<crate>` plus a gitignored
   link.** Measured: it builds, and `Cargo.lock` holds no path or source for path deps, so it is
   byte-identical across machines. **Trap:** a path dep under the package directory becomes an
   implicit member of the game's (empty) workspace, and the engine crates'
   `version.workspace = true` then fails (`workspace.package.edition was not defined`). Fix: the
   template's `[workspace]` gets `exclude = [".ludion"]`.
4. **Link creation lives in Node only** (`scripts/ludion.mjs link-engine`). Rust std has no portable
   directory-link API and `cfg(target_os)` is banned in `crates/`; `fs.symlinkSync(abs, link,
   'junction')` makes a junction on Windows (no admin, no Developer Mode) and a symlink elsewhere,
   in one call with no platform branch. `ludion new` runs the verb after writing files; Node is
   already required in checkout flow (the wrapper exists for it). A failed run is `L4393`.
5. **One way (Pillar 6).** The link is the one per-machine value. No `LUDION_ENGINE` environment
   variable and no `.ludion/engine-path` file: Cargo could not read either, so they would be a
   second, partial way.
6. **Safety, verified.** `std::fs::remove_dir_all`, Node `fs.unlinkSync` and
   `fs.rmSync({recursive})` all remove a junction without touching its target. The wrapper replaces
   only a link (`L4392` for a real directory) and resolves its argument with `realpathSync.native`
   so it never links to another link.
7. **Shared target directory.** Cargo hashed the linked engine crate identically from two different
   scaffold dirs (artifacts reused), so the scaffold tests should not get slower.
8. **Open question — the engine by git URL, no checkout.** Not shipped; it does not fall out of this
   design. Without a checkout the route is the installed flow (published version deps), gated on the
   crates.io decision. A hand-written `ludion = { git = … }` dependency works with Cargo today but
   is unsupported (mirror lag, full-repository fetch).

## Diagnostics

`L4390` link missing/dangling/not an engine (wrapper); `L4391` `link-engine` target is not a
checkout; `L4392` `.ludion/engine` is a real directory; `L4393` `ludion new` could not link.

## Out of scope

The external game's tree (its migration recipe is in `ludion docs project-anatomy`). Git-URL and
crates.io flows. The `ludion dev` shader watch through the link. A guard against a link-shaped engine
root in `L0011`. Editor changes.

## Owner rulings

- None required: the request's options (a) git/`[patch]` and (b) env var / path file were rejected
  under the autonomous-process ruling (decisions 1, 2, 5); the open question is answered by
  decision 8.
- **Proposed — owner ruling owed:** `ludion new` also writes a `.loreignore` beside `.gitignore`
  (both naming `.ludion/`), since Lore reads only `.loreignore` and a Lore-hosted project would
  otherwise see the engine link. Not built.

## Status

W1 shipped: codes `L4390`–`L4393`, the template changes, `ludion new` linking, `link-engine`, tests
in `crates/ludion-cli/tests/new.rs`, the docs section.

W1 verified on macOS (2026-11-22). Voxel generators in a scaffolded project import `grid.mjs`
through the link (`docs/voxel-generation.md`, `docs/project-anatomy.md`); no env var.
