# API Freeze Pass — Foundation Crates (Phase 9, Wave 4)

> Executed checklist for the four foundation crates ahead of crates.io 0.1.0 publication:
> **ludion-core, ludion-ecs, ludion-scene, ludion-sim**. Per `Plan/phase-9-hardening.md`
> decision 4 (quality audit + selective `#[non_exhaustive]`). This is an audit-and-fix pass,
> not a redesign.

## Verification status

- `cargo doc --no-deps -p {ludion-core,ludion-ecs,ludion-scene,ludion-sim}` — **zero warnings** (all four).
- `cargo test -p ludion-core -p ludion-ecs -p ludion-scene -p ludion-sim` — **green**, doctests included
  (the examples are the coverage proof: core, ecs 11, scene 8, sim 10 doctests all pass).
- `cargo check --workspace --all-targets` — **exit 0**: the `#[non_exhaustive]` additions broke no
  downstream crate (all 18 crates + 3 games compile).
- `node scripts/check.mjs --rust-only`: `fmt`/`clippy` green; `test` could not be observed green **only**
  because of filesystem contention from the two other freeze-pass agents running concurrently
  (each attempt failed on a *different* OS-level race — `ludion-cli`'s `dev.rs` supervisor test shares a
  fixed `target/tmp/dev_session` scratch path across processes, and one attempt hit
  `failed to remove target/debug/ludion.exe: Access denied` from a concurrently-running binary). None of
  these are compile or logic failures, and none touch the four audited crates. Re-run when the other
  agents are idle.

## The headline finding

All four crates were already documented to an unusually high standard: every entry-point type and the
crate's primary workflows already carried complete, runnable doctests. The freeze pass was therefore
small and surgical — five broken/private-linking doc references, two genuinely-missing examples on
primary types, and the `#[non_exhaustive]` application. No renames were needed (naming was already
regular across all four), and no accidental external `pub` was found.

---

## ludion-core

**Checked:** naming regularity, per-item examples, accidental `pub`, `#[non_exhaustive]` on
`Diagnostic`/`CodeEntry`, rustdoc warnings.

**Naming:** regular. `new`/`parse`/`as_str`/`domain_digit`, the `with_*` builder family on `Diagnostic`,
`next_u32`/`next_u64`/`next_f32`, `gen_range_u32`/`gen_range_usize`. No `get_` inconsistency. No renames.

**Examples added (1):**
- `audio::AudioEvent` — was the one primary (entry-point) type with no runnable example. Added a small
  doctest constructing the event and reading its fields.

  (All other primary types already had examples: `EntityId`, `DeterministicRng`, `TickRate`/`FrameIndex`/
  `SimTime`, `StableMap`/`StableSet`, `suggest::nearest`, `Diagnostic`/`DiagnosticCode`, `codes::explain`
  via the crate-level example, and the `math` wrappers. `codes::explain_code` and the trivial getters
  are one-liners — left example-free by the "prioritize entry points over trivial getters" judgment.)

**`#[non_exhaustive]` applied (2) — per decision 4:**
- `Diagnostic` (`diagnostics.rs`). **Constructor-only verified:** the only construction path across the
  whole workspace is `Diagnostic::new(...)` + the `with_path`/`with_help`/`with_related` builders. The
  single struct-literal/FRU site was `ludion-ecs`'s `Registry::tag` (fixed — see ecs section). Fields stay
  `pub` for reading and `..`-rest pattern matching; a future field is now additive. Doc updated to say so.
- `CodeEntry` (`codes.rs`). **Verified:** constructed only in-crate (the `CODES` table); downstream code
  (`ludion-cli` explain) only *reads* `.code`/`.title`/`.explanation`. A future column is now additive.
  Doc updated to say so.

  Nothing else received `#[non_exhaustive]` (decision 4 + task constraint: `Key`, `RunState`, `InputEvent`
  etc. were deliberately left as clients construct them literally).

**Accidental `pub`:** none. Every public module (`audio`, `codes`, `collections`, `diagnostics`, `ids`,
`math`, `rng`, `suggest`, `time`) is real API. The `code::*` constants and `CODES`/`explain` are the public
surface `ludion explain` reads.

**Doc warnings fixed (1):**
- `math.rs` crate-doc link `` [`f64`] `` was ambiguous between the `f64` submodule and the primitive →
  disambiguated to `` [`f64`](mod@self::f64) ``.

**Constraint honored:** did NOT touch the diagnostic message/explanation strings in `codes.rs` (the
error-audit wave owns those). Only the `CodeEntry` attribute + a doc note were changed there.

---

## ludion-ecs

**Checked:** naming, examples, accidental `pub`, doc warnings, and the `Diagnostic` construction site that
the core `#[non_exhaustive]` forced.

**Naming:** regular. `spawn`/`despawn`, `insert`/`remove`/`get`/`get_mut`/`has`, `insert_resource`/
`resource`/`resource_mut`/`remove_resource`, `register`/`register_validated`/`register_reserved`. The
`get`-family (components) vs bare `resource` (singletons) split is the idiomatic Rust convention
(collection-style `get` vs typed accessor), not an inconsistency — **deliberately left**, no rename.

**Examples added (1):**
- `SemanticValidate` (trait, `registry.rs`) — a genuine primary workflow (registering a component whose
  cross-field invariants JSON Schema can't express) that had a detailed contract but no runnable example.
  Added a complete doctest: implement `validate_semantics` on a `Range { min, max }`, register it via
  `register_validated`, and assert valid/invalid values through `ComponentInfo::validate_semantics`.

  (All other primary types already had examples: `World`, `World::query`, `Registry`, `Schedule`,
  `ScheduleObserver`, `CommandBuffer` (since deleted, 2026-09-26), `Parent`. The `Component` marker trait is covered by the crate-level
  example. `ComponentInfo`'s methods are covered by the `Registry` round-trip example.)

**Construction-site fix (for core's `#[non_exhaustive] Diagnostic`):**
- `Registry::tag` built its result with functional-record-update (`Diagnostic { message: …, ..diagnostic }`)
  — the sole cross-crate literal/FRU construction of `Diagnostic` in the workspace. Rewrote it to mutate the
  (still-`pub`) `message` field on an owned value, which is allowed under `#[non_exhaustive]` and is more
  minimal than reconstructing through the builders. Behavior identical (path/help/related preserved).

**Accidental `pub`:** none demoted. The query traits/join iterators (`Query`, `Join1..4`, `Column`, …) and
`AnyStorage` are already `#[doc(hidden)] pub use` (they must be nameable in `World::query`'s signature/return
type — documented reason already present). `IntoTarget`/`TargetHandle`/`PendingEntity` are legitimately public
(they appear in `CommandBuffer` method signatures; both went with `CommandBuffer`, deleted 2026-09-26). `pub fn`s inside the private `mod yaml`/`mod schema`/… are
crate-internal by virtue of the private module (not externally reachable) — left as-is.

**Doc warnings fixed (3):**
- Crate docs (`lib.rs`) linked to the private modules `` [`crate::entity`] `` and `` [`crate::storage`] `` →
  repointed to the public items that document the same contract (`World::spawn`/`World::despawn`,
  `World::entities`).
- `IntoTarget` and `TargetHandle` docs linked to the private `Target` enum → reworded to plain prose
  (the concrete target enum is intentionally private; `TargetHandle` is its opaque public wrapper).

---

## ludion-scene

**Checked:** naming, examples, accidental `pub`, doc warnings, schema-hash safety.

**Naming:** regular. Free-function pipeline `parse`/`validate`/`check`/`instantiate`/`emit` with the parallel
`parse_mesh`/`validate_mesh`/`check_mesh`/`emit_mesh` + `peek_kind`/`mesh_budget_warning`. Consistent
verb(+`_mesh`) convention. No renames.

**Examples added (0):** already complete. Every public free function (`parse`, `validate`, `check`,
`instantiate`, `emit`, `peek_kind`, `emit_mesh`) carries a runnable doctest, plus a crate-level end-to-end
example. `Scene`/`SceneEntity`/`MeshDocument` are plain data covered by those; `MeshDocument::vertex_count`/
`triangle_count` are trivial getters exercised by the `emit_mesh` doctest. `validate_mesh`/`parse_mesh`/
`mesh_budget_warning` are documented with `# Errors`; their behavior is shown through `check_mesh`/`emit_mesh`.

**Accidental `pub`:** none. All non-re-exported modules (`emit`, `parse`, `validate`, `instantiate`,
`scalar`, `schema`, `yaml`, `mesh`-internals) are private `mod`s; their inner `pub fn`s are crate-scoped by
the private module and unreachable externally. Public surface is exactly the `lib.rs` re-exports + free fns.

**`#[non_exhaustive]`:** none (correct — decision 4 scopes it to core's Diagnostic/CodeEntry and the
protocol response-side enums, none of which live here).

**Doc warnings fixed (0):** already zero.

**Schema-hash safety:** this crate holds no wire/schemars types (`Scene`/`MeshDocument` are internal data,
not schema-emitting). No doc edits were made that could move the protocol schema hash; nothing needed reverting.

---

## ludion-sim

**Checked:** naming, examples, accidental `pub`, doc warnings.

**Naming:** regular. `SimDriver` verbs `advance`/`advance_observed`/`run_ticks`/`step`/`pause`/`resume`,
accessors `world`/`world_mut`/`frame`/`time`/`tick_rate`/`run_state`/`speed`, `WorldHash::compute`/`as_u64`/
`parse`. No renames.

**Examples added (0):** already complete. `SimDriver`, `WorldHash`, `WorldHash::parse`, `RunState`, `Speed`,
`Key`, `InputEvent`, `InputScript`, `InputState` each carry a runnable doctest, plus a crate-level scripted-run
example. `SimConfig`/`DriverRestore`/`ScriptedInput` are plain data structs covered by the `SimDriver`/
`InputScript` examples.

**Accidental `pub`:** none. `DriverRestore` is public because `ludion-engine`'s snapshot restore constructs
it — a legitimate cross-crate API, not an accident. Public modules `driver`/`hash`/`input`/`run_state` are all
real API.

**`#[non_exhaustive]`:** none (correct per decision 4 / task constraint — `RunState`, `InputEvent`, `Key` are
client-constructed and were deliberately left exhaustive).

**Doc warnings fixed (0):** already zero.

---

## Summary

| Crate | Examples added | Renames | `pub` demotions | `#[non_exhaustive]` | Doc warnings fixed |
|-------|---------------|---------|-----------------|---------------------|--------------------|
| ludion-core | 1 (`AudioEvent`) | 0 | 0 | `Diagnostic`, `CodeEntry` | 1 (`f64` module link) |
| ludion-ecs | 1 (`SemanticValidate`) | 0 | 0 | 0 (+ fixed 1 `Diagnostic` FRU site) | 3 (entity/storage/Target links) |
| ludion-scene | 0 | 0 | 0 | 0 | 0 |
| ludion-sim | 0 | 0 | 0 | 0 | 0 |

**Deliberately left:** the ecs `get`-family vs `resource` accessor split (idiomatic, not an inconsistency);
trivial one-liner getters without examples (per the audit's entry-point-first judgment); all client-constructed
enums (`Key`, `RunState`, `InputEvent`) left exhaustive per decision 4; the diagnostic table strings in
`codes.rs` (owned by the error-audit wave).
