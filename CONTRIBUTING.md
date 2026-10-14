# Contributing to Ludion

Thanks for your interest. This file explains how contributions flow, what to read first, and how to
verify a change.

## How contributions flow

GitHub is the publishing side of the project; the primary version control system is
[Lore](https://github.com/EpicGames/lore). Pull requests are reviewed and merged on GitHub. The
maintainer then runs `node scripts/commit.mjs --import`, which brings the merged commits into Lore
as one revision carrying a `Co-Authored-By:` trailer and the PR title, and pushes a marker commit
back to GitHub. Git keeps your attribution.

The import itself runs only the privacy check; once the revision is in Lore, the private CI runs the
full gate set, which includes a private reference game. A PR that is green on GitHub can therefore
still need a follow-up from the maintainer.

## Before you start

- Read [`CLAUDE.md`](CLAUDE.md). The conventions that matter most: text is the single source of
  truth (scenes and assets are structured text with canonical serialization, so any edit produces a
  minimal diff); sim-side code follows the determinism rules (no std `HashMap`/`HashSet`, no wall
  clock, seeded RNG only); every diagnostic carries an `L####` code; `dispatch.rs` routes and holds
  no logic.
- Read the design documents, [`engine-pillars.md`](engine-pillars.md) and
  [`engine-architecture.md`](engine-architecture.md), before a design change.
- Before changing a subsystem, read its agent doc: `cargo run -p ludion-cli -- devdocs <topic>`
  (bare, it lists every topic). `docs/` topics are mirrored into `crates/ludion-cli/embedded-docs/`;
  after editing one, `node scripts/check.mjs --fix` re-mirrors it.

## Verify loop

Prerequisites: the pinned Rust toolchain (`rust-toolchain.toml`), Node 21 or newer, and
`cargo install cargo-nextest --locked` once per machine.

```
node scripts/check.mjs            # every local gate, compact summary; logs in .build/
node scripts/check.mjs --no-gpu   # the hosted no-gpu profile, for a machine without a pinned GPU
node scripts/check.mjs --fix      # re-mirror docs, regenerate generated sources, cargo fmt
```

Run it after a change and before opening a PR; iterate against its inline errors. The CI workflows
in `.github/workflows/` run these gates: `push.yml` a fast subset on every push, `ci.yml` the full
set on demand.

## Render and golden changes

Golden images and performance budgets are pinned to two reference machines (Windows on an RTX 3090,
macOS on an Apple M4). On any other machine, byte-exact golden comparisons skip with a printed
`L2210` (not the reference GPU) and every golden suite skips with `L2205` on an unpinned OS, instead
of failing. SSIM-tolerant suites (game goldens, import gates) still run there and can fail on other
GPUs. Blessing elsewhere is refused (`L2206`) unless `LUDION_BLESS_ANY_ADAPTER=1`, so a contributor
cannot bless goldens. Anything that moves pixels is verified on
the reference machines before merge, and the maintainer re-blesses. Say in the PR when a change is
expected to move pixels.

## Commit messages

The history is public, so write each message for a stranger: a short summary line, then what changed
and why. No addresses, hostnames or personal paths.

## Terms

Contributions are licensed under the same terms as the project: MIT OR Apache-2.0. Unless you
explicitly state otherwise, any contribution intentionally submitted for inclusion in the work by
you, as defined in the Apache-2.0 license, shall be dual licensed as above, without any additional
terms or conditions.

Please sign off your commits with `git commit -s`, which certifies the
[Developer Certificate of Origin](https://developercertificate.org/). There is no CLA, and no bot
enforces the sign-off.
