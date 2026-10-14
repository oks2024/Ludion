# Audit: asset import and sync pipeline (2026-10-03)

**Scope.** The glTF and PNG import path and the derived-tree sync, checked against the code at rev
`66de4af9`. The review of fe19a265 (sync records, staged swap, per-root lock, `L0023` nested roots,
`L1245` corrupt record) got the closest reading. Files: `crates/ludion-cli/src/{sync,import,manifest,export,generate_atlas}.rs`
and `crates/ludion-cli/tests/sync.rs`; `crates/ludion-import/src/{lib,sources,animation}.rs` (the rest
skimmed); `crates/ludion-assets/src/{lib,watch}.rs`, `crates/ludion-engine/src/mesh_asset.rs`;
the sync call sites in `crates/ludion-cli/src/dev.rs` and `editor/app/src-tauri/src/stage.rs`;
`scripts/{import-sync,map-drift,ludion-bin,check}.mjs` and the matching `.github/workflows/ci.yml` steps;
the `L0023`/`L1245` code-table entries; and vigil's `ludion.toml`, `art/town/lib/{kits,kits.test}.mjs`,
`art/town/prefabs.mjs` and `art/town/square/kits.mjs`. Most of the work was static reading. Findings
marked [verified] were reproduced with the existing debug `ludion` binary on scratch projects outside
the repo, built from a copy of vigil's town kits and from small hand-written glTFs; the scratch
projects were deleted afterwards. The glTF URI-confinement and hardening fixes
(two earlier private audit reports) still hold and are not reported again.

## Overall assessment

The importer is in good shape. Its output is deterministic: it keeps document order, uses `BTreeSet`
dedup and content-hashed texture names, writes forward slashes everywhere, and validates the whole
plan before writing anything. The sync's day-to-day path also works: fresh trees are skipped, a
changed source re-imports alone, partial outputs are detected, debris is swept, and the OS file lock
cannot go stale. The weak spot is the first sync of a root, which fe19a265 was meant to harden.
Ownership is checked before the lock is taken, and the record is written only after every swap. So
while the first sync is still running (8 s on vigil's kits, debug build), a second sync is refused as
`L1245` instead of waiting, and a first sync killed partway leaves a tree the next run refuses. Both
were reproduced. Two other bugs were reproduced. A `to` placed under a source's directory is never
fresh, and the manifest validation lets that layout through. Mesh and clip names that differ only in
case overwrite each other on Windows and macOS. The rest are small error-UX, documentation and
latent-script gaps.

## High

### I1 — The first sync of a root is neither concurrency-safe nor crash-safe: a second sync during it, or any sync after it was killed, is refused as `L1245` · High · [verified]

**Evidence.** `discover` runs the ownership check before any lock is taken
(`crates/ludion-cli/src/sync.rs:388-402`). It refuses a `to` that has no `.ludion-sync.json` and holds
anything besides the lock file and `.sync-*`/`.old-*` debris (`is_sync_artifact`, `:200-203`). The
lock is taken later (`:168-171`). The record is written only at the very end of `sync_table`
(`:631-638`), after every `swap_in` has already renamed finished `<stem>/` directories into place
(`:689`, `:696-709`).

**Why it matters.** The comment at `:164-167` says syncs "wait, never refuse", and fe19a265 says
concurrent syncs wait. That only holds once a record exists. While the first sync is running, `to`
holds real stem directories and no record, which is exactly the state `discover` calls "files the sync
did not write". The `L1245` help then tells the user to delete `to` by hand. Doing that mid-race
deletes the tree the other process is building.

**Failure scenario (reproduced).** A scratch project held vigil's 18 town kits, and its tree had never
been synced. Sync A was started. As soon as the first stem (`ref-figure`) appeared in `assets/town`,
sync B was started. B exited 1 with `error[L1245]: …assets/town exists and holds files the sync did not write`
while A went on to import all 18 kits. In a second run, A was killed (by its own PID) at the same point.
Every later `ludion sync` exited 1 with the same `L1245` until the directory was deleted by hand. Both
cases happen in practice: on a fresh checkout or after a tree deletion, `ludion dev`, the editor's
Game slot and the `check.mjs` `import-sync` step all sync the same project, and a Ctrl-C during the
first `ludion dev` start is ordinary. `concurrent_syncs_both_succeed` (`crates/ludion-cli/tests/sync.rs:252`)
passes only because both processes run `discover` before either one swaps a stem in. Despite its
`sync-first-run-race` name, `a_lock_and_staging_dir_without_a_record_still_syncs` (`:279`) covers only
the lock-plus-staging state.

**Fix.** Claim ownership before the first swap. Under the lock, write a provisional record (this
importer, no entries) atomically. Then move the "owned or empty" decision under the lock as well, so
that a waiter re-checks it after it acquires the lock. A record with no entries already makes every
stem re-import, so a killed first run recovers on its own. Add a test that starts sync B after A's
first swap, for example by seeding a stem directory beside the lock file.

## Medium

### I2 — A `to` inside a source glTF's directory is never fresh, and the manifest accepts it · Medium · [verified]

**Evidence.** An entry's freshness hash is `directory_hash` over everything below the glTF's directory,
recursively (`sync.rs:377-383`, `:476-503`). `validate_imports` (`crates/ludion-cli/src/manifest.rs:124-170`)
checks that each `to` is confined and that no two `to`s nest. It never compares a `to` with the `from`
directories.

**Why it matters.** With `from = "kits/*.gltf"` and `to = "kits/out"`, each source hash includes the
derived tree, the sync record (which stores the previous hash) and the lock file. Each sync rewrites the
record, so the next hash always differs. `ludion dev` then re-imports and swaps the tree before every
reload. The hash is also computed in `discover` before the lock is taken (`:378`). On Windows, reading
another sync's lock file while that sync holds it can fail with a lock violation, and that failure is
reported as an `L1245` "could not read" refusal.

**Failure scenario (reproduced).** A scratch project with that manifest was synced four times in a
row with nothing changed. All four runs printed `{"imported":["a"],"skipped":[], …}`.

**Fix.** In `validate_imports`, refuse (as `L0023`) any `to` that lies inside the literal prefix of a
`from` before its first `*` segment, or say so in a help line. As a backstop, `collect_files` can skip
the record, the lock file and `.sync-*`/`.old-*` entries.

### I3 — Mesh and clip names that differ only in case overwrite each other on case-insensitive file systems · Medium · [verified]

**Evidence.** `dedup` keeps a case-sensitive `BTreeSet<String>` (`crates/ludion-import/src/lib.rs:1191-1205`).
Mesh files are written as `meshes/{name}.mesh.yaml` (`:708-709`) and clips as `clips/{stem}.anim.yaml`
(`crates/ludion-import/src/animation.rs:299-306`). `Import::write` writes each file with
`std::fs::write`, so a later file silently overwrites an earlier one (`lib.rs:161-189`).

**Why it matters.** Blender allows object names that differ only in case (`Wall` and `wall`). On
Windows and on macOS's default APFS, both parts then point at one file that holds the second mesh, and
nothing reports it. On a case-sensitive file system the import writes two files instead, so the same
glTF produces different trees on different machines. That breaks the cross-OS byte-identity the
derived tree relies on (the M4 byte-compare is still owed).

**Failure scenario (reproduced).** A glTF had two triangles, mesh `Wall` at z=0 and mesh `wall` at
z=5. `ludion import` reported "2 meshes" but wrote only `meshes/Wall.mesh.yaml`. That file holds
`name: wall` and the z=5 positions, and the prefab's two parts reference `meshes/Wall.mesh.yaml` and
`meshes/wall.mesh.yaml`.

**Fix.** Make `dedup` compare a lowercased key while keeping the authored spelling, so the second name
becomes `wall-1`. Apply the same check to sync stems in `discover` (`sync.rs:367-376`), where `Well.gltf`
and `well.gltf` in different kit folders would share one `<to>/well` directory and one `.sync-well`
staging directory.

## Low

### I4 — The sync record is written in place, and `L1245`'s help prints a literal `<to>` · Low · [suspected]

`sync_table` writes the record with `std::fs::write` (`sync.rs:631-633`). A process killed mid-write
leaves truncated JSON, which the next run refuses as a corrupt record (`read_record`, `:206-219`) until
someone deletes the tree. The window is small because the record is a few KB. The refusal's help
reads "delete `<to>` and run `ludion sync` again" (`:216`), with the placeholder never filled in,
while the other `L1245` messages name the path. **Fix:** write to a temporary sibling and rename it
over the record (dev's `write_atomic` already does this), and format the help with the real path.

### I5 — The swap has a window with no tree, and a failed rollback lets the next sync delete the only copy · Low · [verified]

The module doc claims "a reader never sees a half-written tree" (`sync.rs:10-11`). In `swap_in`
(`:696-709`), `dest` is renamed away before `staging` is renamed in, so for a moment `<to>/<stem>`
does not exist at all. A running child (dev syncs while the old child keeps serving) that loads a mesh
at that moment gets `L1101`. `rename_retry` retries any error 4 times at 50 ms each (`:712-723`).
That is short against antivirus or indexer handles on files that were just written. If both the
forward rename and the rollback rename fail, the live tree is left as `.old-<stem>`, and the next sync
removes it silently as debris (`:531-547`). The entry is no longer in the record, so it does
re-import, but until then the game has no tree for that stem. **Fix:** reword the doc as "never a
partially written tree, but briefly none". Retry only `PermissionDenied` and sharing violations, with a
longer back-off. If the rollback fails, report it instead of leaving the tree to the debris sweep.

### I6 — `ludion export` copies the derived trees without taking the sync lock · Low · [suspected]

`export` runs `ensure_fresh` and then calls `copy_tree` on each `[[import]] to` without the lock
(`crates/ludion-cli/src/export.rs:422-434`). A concurrent `ludion dev` or editor sync that swaps a stem
during the copy can put a mix of two versions in the bundle, or fail the copy with a vanished
directory. **Fix:** do the copy while holding `lock_root` (make `ensure_fresh` return the held locks,
or add a `with_locked_roots` helper).

### I7 — `ludion sync --json` prints nothing on stdout when it aborts with `L0023` or `L1203` · Low · [verified]

`run` prints the JSON object only when every abort diagnostic is `L1245` (`sync.rs:260-262`). A
nested-`to` manifest (`L0023`) produced empty stdout and exit 1. A `--json` client then has to treat
"no output" as an error. **Fix:** always print the object, with the diagnostics under `refused` or a
new `errors` key.

### I8 — Stale text: the `L0023` explanation omits the nested-`to` refusal, and `ensure_fresh` says it prints nothing · Low · [verified]

The `MANIFEST_INVALID` explanation (`crates/ludion-core/src/codes.rs`, the `L0023` `CodeEntry`) lists
"two tables share a `to`" but not the `to`-inside-another-`to` refusal that fe19a265 added
(`manifest.rs:153-168`), so `ludion explain L0023` does not describe the error a user just got.
`ensure_fresh`'s doc says "Prints nothing" (`sync.rs:231`), yet `sync_table` prints each entry's
import warnings, such as `L1239`, through `diagnostic::print` (`:595-597`). **Fix:** add the clause to
the code text and reword the doc comment.

### I9 — `collect_files` follows symlinks, so a symlink loop below a source directory overflows the stack · Low · [suspected]

`collect_files` recurses whenever `path.is_dir()` is true, and `is_dir` follows links
(`sync.rs:489-503`). A link back to an ancestor recurses until the stack overflows, which crashes the
sync. In `ludion dev` it crashes dev itself, because dev calls the sync in-process. A link that points
outside the project is hashed silently. **Fix:** check `symlink_metadata` and skip links, or refuse
them with a diagnostic.

### I10 — vigil's prefab generator reads kit pieces' local transforms as world transforms · Low · [verified]

`parseKitScene` records `parent: /Parent:/.test(b)` but keeps only the part's local TRS
(`games/vigil/art/town/lib/kits.mjs:7-26`). `kitPiece`/`kitXf` and `prefabText`
(`games/vigil/art/town/prefabs.mjs:83-101`) use `e.t/e.r/e.s` and never compose the parent. Today no
kit has a `Parent:` (all 18 derived prefabs are flat), but the importer does emit `Parent` for child
nodes (`crates/ludion-import/src/lib.rs:479,495`). A kit re-exported with one parented object would
regenerate its prefab and the square with that piece misplaced. The `--check` drift gate would only
report a change, not that the change is wrong. `lib/run.mjs:97` already composes parents.
**Fix:** compose the parent chain in `parseKitScene`, or have `readKit` fail loudly on a `Parent:`
until it is supported, and add a case to `kits.test.mjs`.

### I11 — `relative_to` builds a malformed `generated_from` when the output and the source are on different drives · Low · [suspected]

`relative_to` (`crates/ludion-cli/src/generate_atlas.rs:482-504`) strips the common component prefix.
When the drive prefixes differ, it emits one `..` per component of the output directory, including
its prefix and root, then the source's own `Prefix` and `RootDir`, joined with `/`. The result is
something like `../../D:/\/src/x.gltf`. In-repo content is always on one drive, but
`ludion import --out` to another drive writes a provenance marker that names nothing. **Fix:** fall
back to the absolute source path with forward slashes, or refuse, when the prefixes differ.

## Checked and healthy

- **URI confinement** (`crates/ludion-import/src/sources.rs:110-151`). Raw and percent-decoded URIs
  both pass `relative_refusal`. Before reading, the importer checks `metadata().is_file()` and caps
  the read at 512 MiB. The sync's directory hash covers exactly the files the importer may read: the
  glTF's directory and everything below it.
- **Lock lifetime.** `File::lock` is an OS lock (LockFileEx or flock), so a crashed sync cannot leave
  a stale lock. A leftover lock file counts as a sync artifact. Locks are taken in manifest order, and
  nested roots are refused, so two syncs cannot deadlock.
- **Determinism of the output.** Globs are sorted per segment, stems are sorted, parallel import
  results are re-ordered by index, and the record is a `BTreeMap`. Outputs, `source` and
  `generated_from` all use forward slashes. Texture names are content hashes of re-encoded PNGs. The
  importer has no std `HashMap`. Staging is a sibling at the same depth, so `generated_from` matches a
  hand import byte for byte (pinned by `assert_matches_hand_import`).
- **Incremental correctness.** A changed source, a changed `from` (the recorded `source` is
  compared), a different importer binary, a deleted or partial output, and a vanished source are all
  handled and unit-tested (`sync.rs:756-815`). A failed import never touches the live tree and drops
  its record entry, so the next run retries it.
- **Ownership.** A `to` that holds foreign files is refused and left untouched, and the sync deletes
  directories only under a root that carries a record (once I1 is fixed, the same holds during the
  first run). The manifest refuses `..`, absolute paths, a shared `to` and nested `to`s.
- **Drift gates.** In `check.mjs` the drift gates (`map-drift` included) run only after the
  `import-sync` step has finished (`syncDone`), and `ci.yml` orders the steps the same way.
  `map-drift` runs each `.mjs` generator's `--check` once, which covers `town-square.yaml` and every
  `prefabs/town/*.prefab.yaml`. Imported clips and prefabs are byte-compared by
  `crates/ludion-cli/tests/import.rs`. None of these gates can pass vacuously: each one fails when the
  generator it names is missing.
- **Engine reads.** `ludion_assets::load_bytes` reads into memory and does not memory-map, so a
  running engine does not hold derived files open and block the swap's rename. The `town-lib` node
  test is wired into `check.mjs`, `ci.yml` and TeamCity, and `missingKitMessage` names the fix.

## Suggested order of work

1. **I1.** Write a provisional record under the lock and make the ownership decision under the lock;
   add a test that starts a second sync after the first swap. This is the commit's headline promise.
2. **I3.** Dedup mesh and clip names (and sync stems) case-insensitively. This is a one-function fix
   that removes a silent cross-OS divergence.
3. **I2.** Refuse a `to` under a `from` directory in `validate_imports`.
4. **I4, I5, I7.** Atomic record write, a fuller retry and rollback in the swap, and JSON output on
   every abort. All three are small and sit in `sync.rs`.
5. **I6, I8, I9, I10, I11.** Export locking, doc text, symlink handling, the kit parent chain, and
   cross-drive `relative_to`, as convenient.
