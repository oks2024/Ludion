# Plan: One document-kind abstraction

> Status: **planned, not started.** Source: the 2026-09-25 code audit, finding J3. Current-code facts
> were read from the tree on 2026-09-26; file:line pointers are of that date, so re-grep before editing.

## Trigger

Ludion has 19 `kind:` documents. Each has its own parse, validate and emit, plus its own copies of the
field readers. Two consequences:

- A new kind costs about eight edit sites.
- An input-hardening fix (ceilings, numeric domain) has to be made in five to nine places.

The fix stage removed the worst duplication (below). Build the rest when **any one** of these happens:

- **Any kind needs `format: 2`.** The migration hook must be decided before the first bump, not
  improvised inside one kind. This is the hard trigger.
- A twentieth kind is added.
- A hardening fix has to be copied into three or more kind modules.
- A document-relative path bug recurs (a `../` escape or a key collision), because W3's `DocRef` is
  the structural fix.

## What exists (2026-09-26)

**The kind table shipped.** `ludion_scene::KINDS` (`crates/ludion-scene/src/kinds.rs:36`) lists all 19
kinds:

- **Kinds:** mesh, mesh-edit, voxel, voxel-ops, profile, map, map-kit, assembly, part, animation,
  sprite, palette, atlas, atlas-source, sculpt, generation, origin, touch-layout, pattern.
  Amended 2026-11-19 (owner): `kind: paired` is one more `KINDS` row, see Plan/paired-animations.md d15.
- **Each row** holds `name`, `noun`, `check: KindCheck` (`Here(fn)` or `Elsewhere`), `doc_store` and
  `model_sheet`.
- **Readers:** the engine's content walk (`crates/ludion-engine/src/docs.rs`), `check-scene`'s fallback,
  and the golden dispatch (`crates/ludion-testkit/src/golden.rs:335`).

Kind names are still matched as literals in several places:

- `check-scene` hand-dispatches eight kinds by string before falling back
  (`crates/ludion-cli/src/check_scene.rs:122-151`).
- `model_sheet.rs:1558` matches kind names.
- The voxel verb (`crates/ludion-cli/src/voxel.rs:769`).
- `map.rs:928,939`, `sculpt.rs:426,887`, and `ludion-sculpt`'s `grid.rs:395`.

**A shared reader layer shipped.** `crates/ludion-scene/src/fields.rs` (`parse_number`, `number`,
`engine_number`, `fits_f32`, `parse_size3`) is used by 14 kind modules. `envelope.rs` (`check_envelope`,
`expect_string`, `missing_key`, `unknown_key`) is used by 16. Private readers remain:

- nine `missing_key` wrappers (animation, assembly, atlas, generation, mesh, palette, part, sprite,
  voxel);
- `EMPTY_GLYPH` four times, plus `SPRITE_EMPTY_GLYPH`;
- `parse_size` in `sprite.rs` and `voxel_gen.rs`, and `parse_slices` in `part.rs` and `voxel.rs`;
- `parse_u32`, `parse_vec3` and `parse_keyword`, duplicated between `mesh_edit.rs` (`:866-953`) and
  `sculpt.rs` (`:1176-1318`);
- `map.rs`'s own `expect_*` set (`:1959-2122`).

**13 near-identical `check_*` wrappers** (parse, then validate, then `if diagnostics.is_empty()`) sit in
`crates/ludion-scene/src/lib.rs` (`:454-1620`).

**No `KindDocument` trait, no `DocRef`.** `ludion_core::paths` offers only a lexical
`relative_refusal`.

**Two dense-grid index layouts:**

- y-major `(y*sz+z)*sx+x`: `ludion-scene` `voxel.rs:1765`, `voxel_edit.rs:1081`;
- z-major `x+ex*(y+ey*z)`: `ludion-renderer` `scene.rs:738`, `gi_bin.rs:169`, `ludion-sculpt` `edt.rs:302`.

**Versioning.** Every kind has its own `*_FORMAT_VERSION = 1`, compared through `check_envelope`. No
migration hook exists; only `profile` checks leniently.

## Proposed design

### 1. `trait KindDocument`

```rust
pub trait KindDocument: Sized {
    const KIND: &'static str;
    const FORMAT: u64;
    fn parse(text: &str) -> Result<Self, Vec<Diagnostic>>;
    fn validate(&self) -> Vec<Diagnostic>;
    fn emit(&self) -> String;
}
```

- It is a plain trait, with no macro and no registry magic (Pillar 6).
- `KINDS` rows gain their check through one generic `check::<T: KindDocument>`, which deletes the 13
  wrappers.
- The kinds marked `Elsewhere` (profile, map, map-kit, touch-layout, pattern) implement it where they
  live, or stay `Elsewhere` when their crate cannot depend on `ludion-scene`'s side.

### 2. `fields.rs` owns every reader

- The remaining private readers move into `fields.rs`.
- Readers take the `DiagnosticCode` and noun as parameters, the `envelope::expect_string` pattern, so
  the nine `missing_key` wrappers and the `mesh_edit`/`sculpt` duplicates go away.
- One `EMPTY_GLYPH`.
- The engine numeric domain check stays here.

### 3. `DocRef`

`DocRef` is a typed document-relative reference. It is resolved once, against the document's
directory, confined to the content root, and `..`-collapsed.

- It gives one key for one file, which also closes the asset-cache edge-key duplication recorded in the
  fix stage.
- It is built on `ludion_core::paths`.
- Kind fields that name a file become `DocRef`.

### 4. The migration hook

`KindDocument` gains `fn migrate(from: u64, value: &mut serde_yaml::Value) -> Result<(), Diagnostic>`,
with a default that refuses.

- `check_envelope` calls it when `format < FORMAT`.
- A migrated document is re-emitted canonically by `ludion check-scene --fix` (or a new `migrate`
  verb), never silently on load.
- This is a decision for the first bump; the plan records the default.

### 5. One dense-grid type

`DenseGrid<T>` in `ludion-core` (or `ludion-scene`, if the renderer can read it) fixes one layout.

- Voxel data is hashed and golden-pinned, so the stored layout must not change. Pick y-major (the
  scene side, which is serialized), and give the renderer and sculpt users an accessor.
- The z-major users are GPU-upload paths, so check each one's upload order before converting it.

### 6. Kind literals read the table

`check-scene`'s eight-way string dispatch, `model_sheet`'s match and the voxel verb become
`kind_entry` lookups, or `T::KIND` comparisons.

## Waves

- **W1** — `fields.rs` absorbs the private readers; one `EMPTY_GLYPH`. Pure refactor; no diagnostic
  text changes (the `check-scene` corpus tests are the judge).
- **W2** — the `KindDocument` trait, implemented kind by kind (one commit per 3–4 kinds); the `check_*`
  wrappers are deleted; the kind literals read the table.
- **W3** — `DocRef` for every file-naming field; the asset-cache key uses it.
- **W4** — `DenseGrid`. Only if a third layout user appears or a layout bug is found; otherwise it is
  recorded as not needed.
- **The migration hook** lands with the first `format: 2`, whichever wave is current.

## Risks

- **Diagnostic drift.** Readers that take the code as a parameter can pick the wrong one. Every kind's
  error-case tests must pass unchanged (codes and JSON Pointers). Do not re-bless them.
- **Emit changes are content changes.** `emit` must stay byte-identical on every committed document.
  The drift gates (`generate-map`, `generate-atlas`, `mesh apply`) and a re-emit sweep over the content
  tree are the gate.
- **Crate direction.** `KindDocument` lives in `ludion-scene`. Kinds implemented outside it
  (`pattern`'s doc store in the engine, `touch-layout` in the facade) must not pull `ludion-scene` into
  a crate that is forbidden to see it. Check `engine-architecture.md` §2 before moving each.

## Exit gate

- `node scripts/check.mjs` green per wave. No golden, hash or committed content moves.
- After W2, a new kind is: one module implementing `KindDocument`, one `KINDS` row and a code range.
  Prove it by counting the edit sites of a scratch kind in a throwaway branch.
- `rg 'fn missing_key|EMPTY_GLYPH' crates/ludion-scene/src` finds only `fields.rs`/`envelope.rs`.

## Status

Planned, not started (2026-09-26). The 2026-09 fix stage shipped the prerequisites: `ludion_scene::KINDS`
with its uniqueness test, the shared `fields.rs` numeric reader and domain check, and `envelope.rs`'s
code-parameterised helpers.
