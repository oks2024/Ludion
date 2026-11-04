---
name: ludion-docs
description: Consult or write Ludion agent-docs — `ludion docs` (building a game) and `ludion devdocs` (editing the engine); use before changing a subsystem, or when a topic needs adding or updating.
---

# Ludion agent docs

Two roots, two verbs, one rule set. Each root **is** its own index: one kebab-case `.md` per topic,
the stem is the topic name (`docs/scene-format.md` → `ludion docs scene-format`). No registry.

| root | verb | audience | availability |
| --- | --- | --- | --- |
| `docs/` | `ludion docs <topic>` | building a game **with** Ludion | embedded in the binary at build time — works with no checkout |
| `dev-docs/` | `ludion devdocs <topic>` | editing `crates/` | **checkout-only**; outside an engine checkout every form is `L0008`, by design |

## Consulting a topic

Listing (either verb, no argument) prints `<topic> — <description>` for every topic. `<topic>` prints
the full text; `--section "<heading>"` prints that heading through the next of equal-or-higher rank
(needs a topic, refuses to combine with `--path`); `--path` prints the file, checkout only. Without
`ludion` on PATH, `cargo run -q -p ludion-cli -- docs` is the same binary. **Discovery** walks up
from the cwd for the first ancestor holding both `crates/ludion-core/Cargo.toml` and `docs/` — a
deliberately engine-specific marker, because the old test (`CLAUDE.md` plus `docs/`) misfired once
`ludion new` started writing a `CLAUDE.md` into every scaffolded project. **Failures**, each with
`ludion explain L####`: `L0008` (no checkout, or `--path` outside one), `L0009` unknown topic +
nearest match, `L0010` malformed frontmatter, `L0015` unmatched section + nearest heading + the
topic's top-level headings. Consult before changing the area a topic covers, but **don't preload:**
listing is cheap, a fetch is a whole file — fetch only the area you're about to change, and a trivial
edit needs none.

## Writing or updating a topic

Update the topic in the same commit as the change that makes it stale. `--path` finds the file.
Kebab-case stem = topic name: crate docs after the crate (`ludion-server.md`), concept docs after the
concern (`determinism.md`), spokes after their hub (`renderer-ibl.md`).

**Editing a `docs/` topic breaks the build until you re-mirror.** `crates/ludion-cli/embedded-docs/`
is a committed byte-for-byte mirror of `docs/`; `build.rs` *checks* it and never writes, so `cargo
build -p ludion-cli` panics — naming each drifted file — until `node scripts/check.mjs --fix`
re-mirrors it. Commit the mirror alongside the topic. **`dev-docs/` is not mirrored**, so editing one
has no build consequence: the practical difference between the two roots.

**Frontmatter is strict** — exactly these three lines, `description` the only key:

```markdown
---
description: <one line, ≤100 chars, completes "this topic covers…">
---
```

Anything else fails the listing with `L0010` and a non-zero exit, and both listings run in CI —
`crates/ludion-cli/tests/docs.rs` over `docs/` under `cargo test`, the `devdocs` smoke step in
`scripts/check.mjs` + `.github/workflows/ci.yml` over `dev-docs/`. Malformed frontmatter breaks CI by
design.

## The eight rules

1. **One audience per file.** A file that needs both voices is two files.
2. **One owner per fact.** A fact lives in one topic; every other mention is a pointer. A dev-doc
   that must name a user-facing format links and stops.
3. **Non-obvious only.** Delete anything the reader recovers in one grep of a well-named symbol.
   Banned by name: re-export lists, struct-field transcriptions, function-signature catalogs,
   `--help` restatement, values that live in a `const`. The model is `dev-docs/diagnostics.md`
   refusing to transcribe the code table: *"Every phase adds rows, so a transcription here rots by
   construction."*
   **"One grep" is measured from the reader, so this rule bites harder in `dev-docs/` than in
   `docs/`.** A dev-doc's reader has `crates/` open; a guide topic's reader may be in a scaffolded
   project with no engine source on disk, where a field table is not a transcription of something
   reachable — it is the only copy. Keep such a table when every row carries a default, a range, or
   an `L####` (rule 4), and make sure exactly one topic holds it (rule 2).
4. **The load-bearing test.** A sentence earns its place by carrying at least one of: a measured
   number, an `L####` code, a file path, an exact command, an invariant *plus what breaks when it is
   violated*, or an incident that actually happened. None of those → compressible prose.
5. **State, never schedule.** No phase/wave/milestone tense, no "not yet", no "today", no pasted
   commit-message assertions. CLAUDE.md's comment rule, extended to docs.
6. **Checkable against source today.** Accuracy over completeness.
7. **Size budget, reviewed not gated.** Spoke ≤ 12 KB (~3k tokens). **Index hub ≤ 4 KB** — pure
   routing, scope and pointers, no content of its own (`docs/engine-guide.md`). **Crate hub ≤ 12 KB**
   — a crate's `Layout`/`Rules`/`Gotchas`/`Verify` are content with no other owner, so a crate hub is
   budgeted as a spoke. Crossing a budget is the signal to split, not a failure, and never a licence
   to delete a load-bearing sentence (rule 4) to hit a number. A hub is what you fetch when you don't
   yet know what you need; spokes carry the content. Shape: a `dev-docs/ludion-renderer.md` hub over
   `renderer-shading` / `renderer-ibl` / `renderer-medium` / `renderer-shaders` / `renderer-passes`.
8. **Cross-references are commands** — `ludion docs voxel-format`, `ludion devdocs renderer-ibl` —
   never a bare `docs/x.md` path: a scaffolded project has no `docs/` on disk. The `lists-in-step`
   gate resolves each citation against the right root (`docs` vs `devdocs`).

## The two skeletons

`dev-docs/` — engine internals:
```markdown
# <Title>
**Scope:** who reads this, what it defers to (name the topics).

## Layout      path — role. One line each. No API listings (rule 3).
## Dataflow    the flows: what crosses a thread, crate, or GPU boundary, and where.
## Rules       invariants; each states what breaks if violated.
## Gotchas     traps that have bitten, with the incident.
## Verify      the exact commands that prove a change here is good.
```

`docs/` — using the engine:
```markdown
# <Title>
**Scope:** the task this covers; what it defers to (name the topics).

## The loop    the canonical author → check → observe commands, in order.
## Format      the schema/fields, as tables. Terse.
## Rules       what makes output valid; each names its L#### where one exists.
## Gotchas
## Next        adjacent topics, written as `ludion docs <topic>` commands.
```

`## How it works` is **retired**: it is exactly where bloat and cross-audience content accumulated
(33 KB of it in `ludion-cli.md`, 20 KB in `ludion-engine.md`), and every doc that grew a genuine
topic essay bolted an ad-hoc H2 on beside it. `## Dataflow` is the narrower question that section was
always trying to answer; an essay that outgrows it becomes a spoke.
