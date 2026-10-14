---
description: The Diagnostic type, L#### code table, domain ranges, and how to add a code
---

# Diagnostics

**Scope:** The engine-wide error UX: the `Diagnostic` type, its `Severity`, the `DiagnosticCode` (`L####`) system, the centralized code table in `codes.rs`, domain ranges, and the exact recipe for adding a code. The types live in ludion-core; heavy producers are ludion-ecs (L0xxx), ludion-scene (L1xxx, see also scene-format for the file-side view of scene diagnostics), and ludion-protocol/-server/-engine dispatch (L3xxx).

## Layout

- `crates/ludion-core/src/diagnostics.rs` — `DiagnosticCode` and `Diagnostic` types.
- `crates/ludion-core/src/codes.rs` — `mod code` (all `DiagnosticCode` constants, engine-wide), `CodeEntry`, the `CODES` table, `explain()`/`explain_code()`, and the sync tests (including `DECLARED_CODES`).
- `crates/ludion-cli/src/explain.rs` — `ludion explain <CODE>` (lookup) and `ludion explain` (list all).
- `crates/ludion-cli/src/diagnostic.rs` — the CLI's **one** renderer: `print` (a diagnostic + a blank line), `report` (all of them + the `N errors found` tail when any is an error; `FAILURE` iff one is) and `fail_on_errors` (the same print, but `None` when only warnings were raised, for a verb with passes left to run). The text itself comes from `ludion_core::diagnostics::render_report`, which the game facade's `--check-scene` shares. A warning prints and never fails a command; `N errors found` counts errors only. `usage_error` prints a bad-flag line (`error: <message>`) and returns `FAILURE`; the facade's twin is `crates/ludion/src/usage.rs` (`usage_error`, `usage_warning`).
- `crates/ludion-cli/tests/explain.rs` — CLI-level tests.

## Rules

- **Every diagnostic carries a registered code.** No ad-hoc error strings on user-facing paths; a code that is constructible anywhere must have a row in `CODES` (test-enforced).
- **Severity belongs to the code, not to the call site.** A code either names a lint or it does not, so `CodeEntry.severity` is the source of truth and `Diagnostic::new` reads it from there — the container a diagnostic lands in (an `Err` vs a `warnings` vec) no longer decides how it renders. Two deliberate exceptions, whose fatality is a property of the *invocation*: `L1202` (`import --allow-lossy`) and `L1167` (`gen`'s live check is optional) are downgraded explicitly with `.with_severity(...)`. `with_severity_is_called_only_at_named_sites` (in `codes.rs`) scans the source and fails on any other call site, and a test pins the whole lint set so a second exception cannot appear quietly.
- **The table is centralized** in `ludion-core`'s `codes.rs` — one grep-able const array, no `inventory`/`linkme` collection magic. Even codes raised by other crates (ecs, scene, render, sim, cli) declare their constants and rows here.
- **Codes are stable forever.** A code names a diagnostic *class*; tools, tests, and agents match on it. Never renumber, never reuse a retired number: a retired code's const and `CODES` row are deleted (a test forces it), and its number moves to the `RETIRED` table in `codes.rs`.
- **Domain ranges** partition on the first digit (`DiagnosticCode::domain_digit()`, test-enforced to be `0`–`5`):

  | Range   | Domain            |
  |---------|-------------------|
  | `L0xxx` | core / ecs        |
  | `L1xxx` | scene / assets    |
  | `L2xxx` | render (incl. testkit golden codes L22xx) |
  | `L3xxx` | protocol / server (incl. the replay-session codes) |
  | `L4xxx` | sim               |
  | `L5xxx` | audio             |

- **Print a diagnostic through `Display` alone**, never behind a hand-written `error:`/`warning:` prefix: the code, severity and path come from the value. Context goes in `help`, via `Diagnostic::append_help` when help may already be set. A bad flag or argument combination is the one exception: it goes through `diagnostic::usage_error` (CLI) or `usage::usage_error`/`usage_warning` (facade). `ci/scripts/diagnostic-prints.mjs` (in `check.mjs` and CI) fails any other prefixed print under `crates/*/src` and `games/*/src`.
- **Rustc's bar:** every diagnostic states what is wrong, where (JSON Pointer `path` when a document is involved), why, and the likely fix (`help`). Suggestions ("nearest match is \`X\`") must be deterministic.
- Only codes constructible by code that exists *today* are registered — no speculative rows for future crates.

## How it works

`DiagnosticCode` is five ASCII bytes inline (`[u8; 5]`) — `Copy`, allocation-free. `DiagnosticCode::new("L0042")` is `const` and panics at compile time on a malformed literal; `DiagnosticCode::parse` is the runtime fallible form. Serializes as the plain string `"L0042"`.

`Diagnostic { code, severity, message, path: Option<String>, help: Option<String>, related: Vec<Diagnostic> }` builds with `Diagnostic::new(code, msg).with_path(...).with_help(...).with_related(...)`. `severity` is `Severity::Error` or `Severity::Warning`, read from the code's table row by `Diagnostic::new`; `.with_severity(...)` overrides it. `Display` renders rustc-style, leading with the severity word:

```
error[L1014]: component `Transform`: unknown field `yy`
  --> /entities/0/components/Transform/yy
  help: nearest match is `y`

warning[L1028]: gate `plaza-east` is sealed behind the boundary wall
  --> /markers/4
  help: open a walkable perimeter cell next to the gate
```

Serde skips empty optionals, giving the wire error shape (`{"code":"L1042","message":"...","path":"...","help":"..."}`). `severity` is skipped when it is `Error`, so **an ordinary error's wire payload is byte-identical to the pre-severity shape** — only a lint pays a field for it, and a client that ignores the field reads every error exactly as before. `ludion-protocol`'s `schema_proxy::WireErrorSchema` mirrors that shape by hand, and is what generates `editor/protocol/src/protocol.d.ts`.

The table is `CODES: &[CodeEntry]` where `CodeEntry { code: DiagnosticCode, severity: Severity, title: &'static str, explanation: &'static str }`. `explanation` is the long-form text `ludion explain` prints — typically: what happened, the mechanism/rationale, common causes, and the fix. `codes::explain(&str)` and `codes::explain_code(DiagnosticCode)` are linear lookups returning `Option<&'static CodeEntry>`; `codes::severity(DiagnosticCode)` is the severity-only shortcut. `ludion explain` prints `L#### [error|warning]: <title>`.

**Test sync:** `codes.rs` has a `#[cfg(test)] const DECLARED_CODES: &[DiagnosticCode]` mirroring every constant in `mod code`, kept in sync by hand, a bijection test (`code_consts_and_declared_list_are_a_bijection`) failing on drift. Tests assert: every declared code is registered (`declared_codes_are_registered`), table codes are unique, every entry re-parses as `L####` with non-empty title/explanation, every code falls inside L0xxx–L5xxx, and the set of `Severity::Warning` rows is exactly the pinned lint list (`the_registered_lints_are_exactly_these`).

**Which numbers are taken, and which of them are lints, is a question for the table — not for this doc.** Every phase adds rows, so a transcription here rots by construction. `ludion explain` lists every registered code with its severity and title, and `grep 'Severity::Warning' crates/ludion-core/src/codes.rs` lists the lints alone. What *is* stable, and worth knowing before you pick a number, is the sub-block convention inside each domain — see the recipe below.

## How to add a code

All edits are in `crates/ludion-core/src/codes.rs`:

1. Pick the next free number in the owning domain range (sub-blocks are conventions: `L11xx` assets within scene/assets, `L21xx` engine-extract render errors, `L22xx` testkit).
2. Add the constant to `pub mod code` with a doc comment stating the trigger and which crate raises it:
   `pub const MY_NEW_CODE: DiagnosticCode = DiagnosticCode::new("L1016");`
3. Add a `CodeEntry` row to `CODES`, keeping the table in numeric order within its domain section. Use `severity: Severity::Error` unless raising the code genuinely leaves the command successful — then use `Severity::Warning` and add the constant to the `LINTS` list in the tests module. Write the `explanation` to rustc's bar: what happened, why the rule exists, common causes, the fix.
4. Add the constant to `DECLARED_CODES` in the tests module (same order as `mod code`).
5. Construct it at the raise site via `Diagnostic::new(code::MY_NEW_CODE, ...)`, adding `.with_path(json_pointer)` and `.with_help(suggestion)` whenever a document or a plausible fix exists.
6. Run `cargo test -p ludion-core` (sync tests) and `cargo run -p ludion-cli -- explain L1016` (prints the new entry).

## Verify

```
cargo test -p ludion-core                     # table sync, uniqueness, shape, domain-range, lint-set tests
cargo run -p ludion-cli -- explain L0001      # single lookup (exit success)
cargo run -p ludion-cli -- explain L1028      # a lint: prints `L1028 [warning]: ...`
cargo run -p ludion-cli -- explain L9999      # unknown code -> itself an L0001 diagnostic
cargo run -p ludion-cli -- explain            # lists every code with its severity and title
cargo test -p ludion-cli --test explain       # CLI behavior tests
```

## Gotchas

- Numbers within a range encode conventionally meaningful sub-blocks (`L1101`+ = assets, `L2101`+ = extract-time render, `L2201`+ = testkit) — follow the neighborhood, don't just append at the domain's end.
- `Diagnostic.path` is the *only* structured location field — there is no span struct; line/column information, where known (YAML parse errors), is embedded in `message` text. It has two grammars: an RFC 6901 JSON Pointer into the file the diagnostic names (escape map keys with `ludion_core::diagnostics::pointer_token`, never by hand), or, for data-doc diagnostics, `<kind>/<doc name>/<field>` — no leading slash, not a pointer (see `ludion docs data-docs`). A consumer must not parse the second as the first.
- **A new lint moves the pinned set.** `the_registered_lints_are_exactly_these` compares against a hand-written list in table order, so a `Severity::Warning` row added without the matching `LINTS` entry fails the test — deliberately: "this code never fails its command" is a decision, not a detail.
- **Adding a field to `Diagnostic` is a protocol rev.** The wire error object is mirrored by hand in `ludion-protocol`'s `schema_proxy::WireErrorSchema` (ludion-core does not derive `JsonSchema`), so a new field moves the `schema_hash` and runs the whole additive-rev checklist in `protocol-evolution` — snapshot bless, `EXPECTED_SCHEMA_HASH`, `protocol.d.ts`, and a `run.lrp` re-record last.
- `LogEvent.diagnostic` (the `log` channel's optional field) carries the same wire shape as an error response's diagnostic — build log lines with `LogEvent::from_diagnostic`, never a hand-formatted `code: message`.
- `L3xxx` is protocol/server territory — do not park unrelated codes in it.
- Explanations are `&'static str` in a const table: keep them plain text (they print to terminals), escape inner double quotes, and remember `ludion explain` is the primary reader — write for an agent deciding its next action.
