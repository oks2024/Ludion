---
description: validating game data-docs: --check-docs, cross-doc rules, one-of-a-kind kinds
---

# Checking game data-docs

**Scope:** Validating a game's `kind:` data-docs beyond their schema: the `--check-docs` CI run, cross-doc reference rules, and kinds that must hold exactly one doc. Authoring and registering a kind is data-docs.

## Checking docs in CI

`cargo run -p <game> -- --check-docs` validates every registered kind against the content root and
exits — no server, no GPU, no window. A valid root exits 0 with a one-line summary; a malformed doc
prints its `L1119`/`L1120` diagnostics to stderr and exits non-zero; a game that registers no kinds
exits 0 when its content root holds no data-docs, and a data-doc whose kind nothing registered fails
(`L1155`) — a doc nothing validated is not validated. An engine-owned registration such as
`.patterns()` validates its docs, so a patterns-only game passes; build output (`target/`,
`node_modules/`, dot-directories) is never walked. It reuses the **exact in-process validation
the live engine runs**, so CI and runtime agree by construction — there is no separate schema file to drift. (`ludion check-scene`
stays engine-kind-only: it validates a single scene or engine-kind file and hard-rejects an unknown
`kind:`, because it cannot know a game's types without loading the game binary — which is why the doc
check lives in the game's own facade.)

### Validating references

Schema validation stops at "well-typed string": whether a quest's `district:` is real is a rule
only your game knows. Register it as a builder pass, which sees the world once every store is
installed and so reads across kinds:

```rust,ignore
Engine::builder().doc_kind::<QuestDoc>("quest").validate_docs(|world| {
    let Some(quests) = world.resource::<DocStore<QuestDoc>>() else { return Vec::new() };
    quests.iter().filter(|(_, q)| !DISTRICTS.contains(&q.district.as_str()))
        .map(|(name, q)| Diagnostic::new(code::DOC_REFERENCE_UNRESOLVED,
                format!("quest `{name}` names no district `{}`", q.district))
            .with_path(format!("quest/{name}/district")))
        .collect()
});
```

Report a broken reference as `L1144`, `path` set to `<kind>/<doc name>/<field>` — the failing field's
document path (not an RFC 6901 pointer: no leading slash), and the print latch's once-per-offender key. Call it repeatedly to split rules by kind
(passes run in order and concatenate). They run at host start and under
`--check-docs` (an error refuses the start, a warning prints and it continues), and after every live
doc reload — where a reload that makes a rule report a new error is refused and the old doc stays
live, while warnings and errors that were already there print once and the reload applies.

## One-of-a-kind docs

A kind that is one file, not a set (`rules`, `tuning`), registers with `doc_singleton`:

```rust,ignore
Engine::builder().doc_singleton::<RulesDoc>("rules")
```

Zero docs or two-or-more is `L1315` (path `<kind>`; a second doc names every declaring file), raised
where the validators run — build, `scene.load`/`restore`, district switch, `--check-docs` — and a live
reload that would add a second doc is refused, the store and its generation untouched. Read the doc
with `store.single()`, which is `Some` only for exactly one doc on any store. Use `doc_kind` instead
when a set, or an absent doc, is legitimate.

## Next

- `ludion docs data-docs` — registering a kind, the store, the tuning loop.
