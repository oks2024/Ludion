# Plan: Phase 16 — Text profile API (E5)

> Status: **landed 2026-07-17 — all 5 waves, one commit each (Lore revs 183–187); E5 READY.**
> Exit gates green: wire surface 26 / `schema_hash` unmoved (schema-drift gate), zero golden churn,
> every pre-E5 `.lrp` decodes at format 1 with `header.profile == None` (additive header field, no
> version bump), `SNAPSHOT_FORMAT` stays 1 (profile-free snapshots byte-identical), `check.mjs --full`
> green including the determinism canary. Amendments during build: L1122's registration moved from
> Wave 1 to Wave 3 (the reachability test needs its save-path reference site); `profile` added to the
> engine-owned skip kinds in Wave 2 (plan-review catch — E4's doc walk would otherwise L1119-fail a
> game's own save file once any doc kind is registered); Wave 3's drain lands in `advance_live`/
> `advance_observed` only (`step_ticks` delegates to `advance_live` — the plan's "three primitives"
> was imprecise); Wave 4's `--full` run surfaced two cross-crate breaks fixed inline (a ludion-sim
> `ReplayHeader` test initializer, a rustdoc private-intra-doc link); live `L1122` printing deferred —
> `run()` has no per-frame host print seam after the engine moves to the sim thread, so the error is
> queryable via `Engine::profile_save_error()` (noted in `docs/ludion.md`). Superseded after landing
> (audit 2026-10-03, Lore r1146): decision 4's "engine-local until a second site" — the helper is
> `ludion_core::atomic::write_atomic`, shared by every derived-file writer, and its temp is the
> hidden `.<name>.<pid>.<n>.tmp` sibling, not `<path>.tmp`.
>
> Originally: authored 2026-07-16, ready to start. This is the wave-level plan for wave **E5** of
> the first-game arc; its brief is `Plan/engine-first-game.md` §"E5 — Text profile API" plus the
> "Cross-cutting rules" there, scheduled by a private roadmap (rules 5 and 8). The
> consumer is the reference game's persistent profile: unlocks, knowledge flags and lifetime stats
> that outlive a run and a process restart; run-end writes the profile; and the
> reproducibility gate — **a run is exactly reproducible from (seed, profile-at-start, input replay)**.
> The E5 brief's "Proposed design" items are confirmed or amended below with current-code evidence
> (amendments called out inline). The thesis: a game registers **one** serde profile type and gets
> persistent, canonical, versioned, migration-safe save state that reproduces bit-exactly under
> replay — the same guarantees game data-docs (E4) already enjoy, specialized to a **singleton** that
> the sim mutates rather than a content-root collection the engine only reads.
>
> **E5 is a near-clone of the landed E4 `doc_kind`/`DocStore` machinery** (Phase 15, Lore revs
> 176–180): the registration-held-on-`Engine`-and-reinstalled-at-every-reset shape, the
> monomorphized-`fn`-pointer type erasure, the canonical `format:`/`kind:` envelope, the
> inline-in-LRP1 reproducibility story, and the migration-report precedent all exist in tree. This
> plan leans on them and notes exactly where a profile diverges from a doc (singleton, sim-mutated,
> lenient-migrating, host-persisted).

## Current-code facts that shape the work

- **The registration mold is landed and singleton-shaped for free.** `EngineBuilder::doc_kind::<T>`
  (`crates/ludion-engine/src/builder.rs:165-171`, bound `Serialize + DeserializeOwned + JsonSchema +
  'static + Send + Sync`) pushes a `DocKind` (`crates/ludion-engine/src/docs.rs:133-155`) — a `kind`
  string, a captured `schema_for!(T)`, and monomorphized `fn` pointers — onto a persistent
  `doc_kinds: Vec<DocKind>` field carried on `Engine` (`builder.rs:76`, `engine.rs:196,204`). The
  `install_docs` step (`engine.rs:379-386`) re-walks and re-inserts the stores at construction
  (`build_at`, `builder.rs:218`), `scene.load` (`engine.rs:606`), and `restore` (`engine.rs:1157`) —
  the `install_audio_queue` mold (`engine.rs:363-367`) — because a world reset wipes every resource
  (`World::restore_entities` clears resources, `crates/ludion-ecs/src/world.rs:155-158`). **A profile
  is exactly one such registration, but with `Vec` → `Option` and a `DocStore<T>` → `Profile<P>`
  resource.** The profile bound adds `+ Default` (the brief), because a profile load-or-**defaults**.
- **Resources are the right home and trip no drift gate.** `World` stores resources as
  `BTreeMap<TypeId, Box<dyn Any + Send>>` with `insert_resource<T: 'static + Send>` / `resource<T>` /
  `resource_mut<T>` (`world.rs:277-306`); resources never enter `WorldHash` and never serialize into
  a snapshot's entity data. So `Profile<P>` needs no drift-gate work, no builtin-registry append, no
  world-hash re-bless — **its numbers reach the hash only through the components a game's systems
  derive from it** (unlocks → different spawns), which is precisely why the profile-at-start must
  travel in the replay for reproducibility (decision 6/7), the `DocStore` precedent
  (`docs.rs:30-37`).
- **The canonical `format:`/`kind:` envelope + byte-idempotent emit already exist and are
  P-agnostic.** `ludion_scene::emit_doc(kind, value, schema)` (`crates/ludion-scene/src/doc.rs:135`,
  `emit.rs`) writes `format: 1` + `kind: <kind>` + fields in schema-declaration order, ryu floats,
  LF, proven byte-idempotent by proptest (`crates/ludion-scene/tests/doc.rs`). A profile reuses this
  **verbatim** with `kind: profile` — zero new emit machinery. `peek_kind` (`lib.rs:297`) recognizes
  any kind.
- **But `check_doc` REJECTS drift — a profile must MIGRATE it.** `check_doc`
  (`doc.rs:51-130`) validates the envelope (`L1119`), then runs the strict `Checker`
  (`validate.rs:212-…`, `Subject::Doc` → `L1120`, `doc.rs:114-123`) which reports every unknown field
  and type mismatch as a hard error with a nearest-match suggestion — correct for authored content an
  agent must fix, **fatal for a player's accumulated save**. E5's load is a new *lenient* path
  (decision 3): unknown fields reported-and-dropped, missing fields defaulted, an unloadable file
  backed-up-and-defaulted, never a crash and never a hard reject. The `Subject::Profile` seam in
  `validate.rs:177-208` is therefore **not** used — the profile does not run the strict `Checker` at
  all.
- **The migration-report precedent is `RestoreReport` / `engine.restore`.**
  `crates/ludion-protocol/src/restore.rs:41-118` is a boundary-clean report (`entries: Vec<…>`,
  `is_clean()`, per-entry `code()`); `snapshot.rs`'s `migrate_component` (`:215-258`) applies the
  drop-unregistered (`L3015`) / drop-undeserializable (`L3016`) / kept-with-field-diff policy, and
  `diff_fields` (`snapshot.rs:264-281`) computes the filled/removed key lists by re-serializing and
  diffing keys. **The profile migration reuses `diff_fields`'s exact technique** to report
  filled-from-default vs dropped-unknown fields, and mirrors `RestoreReport`'s shape in a new
  `ProfileMigration` type (named to avoid the perf `ProfileReport` collision, decision 1).
- **The AudioQueue drain is the save-request mold, and audio is *regenerated*, not recorded.** A game
  system pushes into the `AudioQueue` resource (`crates/ludion-engine/src/audio.rs:27-30`,
  `take()` :58); the engine drains it at every frame boundary in `drain_audio`
  (`engine.rs:342-358`), called from the one audio-draining tick primitive `advance_live`
  (`engine.rs:300-309`) — also from `advance_observed` (`:318`) and `step_ticks` (`:333`). Audio is a
  pure function of the sim, so a **replay regenerates the identical `(frame, sound, gain)` stream and
  never records it** (`audio.rs:14-16`). **A profile save request is the same shape** (a game system
  decides to save): regenerated by replay, logged frame-stamped, never recorded in LRP1 — the
  disk write is the out-of-sim side effect, gated like the audio *sink* (`set_audio_sender`,
  `engine.rs:463`; the host wires it, a headless/replay engine leaves it unset). This is decision 5.
- **The LRP1 header is the natural profile-at-start slot; adding a field is free.** `ReplayHeader`
  (`crates/ludion-protocol/src/replay.rs:103-117`) is zstd'd JSON, so a `#[serde(default)] profile:
  Option<String>` is additive with **no framing/version change** and every old `.lrp` decodes (the
  reader parses the header as JSON, `replay.rs:369`). This is the identical direction E4 used to grow
  `ReplayRecord` (the `DocReload` kind at format 1, `replay.rs:30-38,156`) and E2 used to grow
  `InputEvent` — **`REPLAY_FORMAT_VERSION` stays 1** (`replay.rs:56`). `SceneRef::Inline`
  (`replay.rs:78-83`) is the "boot state travels inline" precedent the profile follows exactly.
  `replay_header()` (`engine.rs:926-937`) is where the field is populated from the live profile.
- **Replay reapply-off-disk is a landed pattern.** `replay_hashes_with` (`crates/ludion-engine/src/
  replay.rs:157-197`) loads the header scene (`load_header_scene` :208-234 — `SceneRef::Inline` loads
  verbatim, no disk), reseeds (`:167`), and drives the records through `drive_actions` (`:87-122`);
  `ReplayAction::DocReload` (`:77-82`) calls `engine.apply_doc_reload_inline` (`engine.rs:437-448`) —
  "reapply inline content without touching disk". **The profile-at-start installs the same way**: a
  new step after `load_header_scene`/`reseed` installs the header's embedded profile text, overriding
  whatever the boot disk-load put there.
- **`schema_hash` excludes the replay/snapshot/profile types — the additions cannot move it.**
  `schema_hash` hashes `protocol_schema()` (`crates/ludion-protocol/src/schema.rs:36-54,76-78`), which
  is `Request`/`Response`/`Event`/`Command`/`ResponseBody`/`Hello`/`ServerHello`/`FrameHeader` only.
  `ReplayHeader`, `SnapshotDoc`, and a `Profile<P>`'s schema (held on `Engine` like `DocKind`, never
  in the registry) are all outside it. E4 confirms this empirically: it added `ReplayRecord::DocReload`
  and the board records `schema_hash` unmoved. **No new `Command` → wire surface stays 26** (saving is
  a drained resource flag; loading is boot-time; the migration report surfaces via stderr + an engine
  accessor — none is a wire method).
- **`std::fs::rename` overwrites atomically on Windows — the briefing's warning is outdated (verified).**
  A probe on the pinned Windows machine (`fs::write` OLD → `fs::write` tmp NEW → `fs::rename(tmp, dest)`)
  printed `rename-over-existing OK; dest now = "NEW"`. Modern Rust std maps `fs::rename` to a
  replace-if-exists rename on Windows, so the atomic-write helper is simply **write-temp-in-same-dir →
  `fs::rename`**, with **no** remove-then-rename dance and **no** non-atomic window. There are **no**
  atomic writes anywhere in the repo today — every write is a direct `std::fs::write` (e.g. `scene.save`,
  `crates/ludion-engine/src/commands/scene.rs`) — so the helper is new code (decision 4).
- **Free L-codes:** `L1105–L1109`, `L1121+`, `L1123+` in the L11xx band (`crates/ludion-core/src/
  codes.rs:183-275`; scene/asset/doc latest = `L1120` at :275). E5 takes **L1121** (profile could not
  be loaded as authored → backed up + defaulted) and **L1122** (profile could not be saved — a
  frame-boundary IO error). Registration is three-part — a `const` in `mod code`, a `CodeEntry` in
  `CODES` (`:493`), a `DECLARED_CODES` entry — enforced by the bijection (`:2308`) and reachability
  (`:2339`) tests (a new const must be referenced outside `codes.rs`).
- **The content root is the scene file's directory.** `load_scene_file` sets `project_root =
  scene_path.parent()` (`builder.rs:235-247`); relative paths resolve against `engine.project_root()`.
  So the default `save/profile.yaml` resolves under the scene's directory (for vigil,
  `games/vigil/scenes/save/profile.yaml`) — the same root-relative rule every asset/doc uses
  (decision 8).
- **E4's doc discovery would reject an on-disk profile — `profile` must join the engine-owned skip
  kinds.** `load_doc_stores` walks every `*.yaml` under the content root whenever *any* doc kind is
  registered, skips `kind`-less scenes and `ENGINE_KINDS = ["mesh", "voxel"]`
  (`crates/ludion-engine/src/docs.rs:28,350`), and raises **L1119 with a nearest-match suggestion for
  any other unregistered `kind:`** (`docs.rs:363,432-451`). The default save location is inside the
  content root (decision 8), so the moment a game registers a doc kind (G4 does) *and* has a saved
  profile, boot would fail on its own save file. Fix owned by Wave 2: add `"profile"` to
  `ENGINE_KINDS` (and to the facade's `ENGINE_OWNED_KINDS` mirror, `crates/ludion/src/lib.rs:453`,
  used by `run_check_docs`) so the doc walk and `--check-docs` skip profile files — the profile loader
  owns them, exactly as the asset cache owns `mesh`/`voxel`. The `.tmp`/backup siblings need no rule
  (`profile.yaml.tmp` / `profile.yaml.bak.<hash>` are not `*.yaml` matches).
- **The facade host + short-circuit + example molds.** `RunArgs`/`run` (`crates/ludion/src/lib.rs:
  97-130,168`) with the `--profile`/`--check-docs` short-circuits (`:192-199`); `set_audio_sender` +
  `AudioSink::spawn` wired at `:217-219` (the host-side out-of-sim seam the profile persistence gate
  copies); the recording arm at `:223-238`. Facade example mold: `crates/ludion/examples/data_docs.rs`
  (+ `hud_bar.rs`, `cursor_aimed_turret.rs`) with a behavioral two-run WorldHash test. Guide chapter
  mold: `docs/engine-guide.md` "Game data-docs" (`##` H2); docs topics are `docs/*.md` stems; the
  `embedded_topics_match_the_docs_directory` drift test gates the mirror. vigil (`games/vigil`) has
  zero persistence today — G5 is its first.

## Design decisions (settled by this plan)

1. **Keep `EngineBuilder::profile::<P>()` and the `Profile<P>` resource (the settled "profile"
   vocabulary); name the new engine module `player_profile.rs`; leave the perf profiler untouched.**
   The settled decision is the "text profile API" and the user vocabulary is "profile"; the collision
   with the performance profiler (`crates/ludion-engine/src/profile.rs` — `profile_engine`, `Budget`,
   `ProfileReport`; the `--profile <FRAMES>`/`--profile-check` flags; the `ludion profile` CLI) is
   **conceptual only** — no type or flag name actually clashes: `Profile<P>` vs `ProfileReport`/`Budget`,
   builder `.profile::<P>()` vs the `--profile` CLI flag (a different surface). The one literal clash is
   the file name, resolved by putting the save facility in `player_profile.rs`, never `profile.rs`.
   The protocol migration type is `ProfileMigration` (not `ProfileReport`). Rustdoc and the guide
   always qualify: "player profile (save state)" vs "performance profile (timings)".
   *Rejected:* renaming the API to `save_profile::<P>()`/`SaveProfile<P>` — diverges from the settled
   "profile" vocabulary for no real ambiguity gain, and `save` is already overloaded (`scene.save`).
   *Rejected:* renaming the perf profiler — it is shipped and load-bearing across the roadmap's E6
   perf-gate (`ludion profile` + TOML budgets, measured every milestone from M1); churn for no benefit.

2. **File format reuses the doc envelope + `emit_doc` verbatim (`format: 1` + `kind: profile` +
   flattened fields), but mints profile-specific L-codes — confirming the brief's "new L-codes".** The
   canonical write is `ludion_scene::emit_doc("profile", &value, &schema)` (`doc.rs:135`) — identical
   bytes, byte-idempotence proptest inherited. The *load* is a new lenient path (decision 3) whose
   diagnostics are **`L1121`**, not `L1119`/`L1120`, because a profile "error" is a warning + backup
   (semantically unlike a doc's hard reject) and the `Subject::Profile` seam / strict `Checker` is not
   invoked. This confirms the brief's "new L-codes for parse/validate."
   *Rejected:* reusing `check_doc` + `L1119`/`L1120` — the strict `Checker` rejects any drifted save,
   which for a player profile means *destroying progress on the first schema change*. *Rejected:* a
   wholly separate envelope — needless divergence from the `mesh`/`voxel`/doc `format:`/`kind:`
   convention that `emit_doc` already produces perfectly.

3. **Migration is lenient and engine-owned: fill-missing, drop-and-report-unknown, back-up-and-default
   on anything unloadable — never reject, never crash, never silently destroy.** The lenient path lives
   in `player_profile.rs` (it needs `P`, via the monomorphized `ProfileKind`). `ludion-scene` gains one
   small P-agnostic entry, `open_profile(source) -> Result<ProfileEnvelope, Vec<Diagnostic>>`, that
   parses strict-YAML to a `Value`, validates that `format`/`kind` are present and `kind == "profile"`,
   strips them, and returns `{ fields: Value, format: u64 }` — it does **not** run the strict `Checker`
   (no unknown-field or type rejection). The engine's `ProfileKind::load` then:
   - `serde_json::from_value::<P>(fields)` — missing fields fill via `P: Default` / `#[serde(default)]`;
   - on success, builds a `ProfileMigration` by diffing the file's keys against the reserialized value
     (the `diff_fields` mold, `snapshot.rs:264`): keys only in the reserialized form = **filled** from
     defaults; keys only in the file = **dropped** (unknown/removed since);
   - on `from_value` failure (a **retyped** field / structurally unparseable value), on **unparseable
     YAML or a bad envelope** (`open_profile` Err), or on a **newer-than-engine `format`** version:
     the file is **backed up** (renamed to `profile.yaml.bak.<content-hash>`) and the profile starts
     from `P::default()`, emitting **`L1121`** (a warning, non-fatal — boot never blocks) with a
     `ProfileMigration::Reset { backup, reason }` entry.

   A player's bytes are never destroyed (the backup preserves them) and the game always boots. The
   report surfaces two ways: the host prints it to stderr at boot (a scene-diagnostic-shaped line), and
   `Engine::profile_migration()` exposes it for tests (the `audio_log` / `RestoreReport` mold).
   *Rejected:* refuse to boot on a corrupt/newer profile — a save must never brick the game.
   *Rejected:* silently reset — destroys progress with no trace. *Rejected:* per-field lenient
   deserialize of `P` — serde is all-or-nothing on a struct, so the whole-value backup is the honest
   analog of `RestoreReport`'s whole-component drop (`snapshot.rs:235-242`). *Rejected:* preserving
   unknown fields across a re-save — canonical emit only writes schema fields, so a newer save opened
   by an older build *would* lose them on the next write; the newer-`format` → backup rule (above)
   prevents an older build from silently downgrading a newer save.

4. **Save request = a `dirty` flag on `Profile<P>`, drained at the frame boundary beside audio;
   atomic write = write-temp-then-rename; disk write gated on host-enabled persistence.** A game system
   mutates the profile through `Profile<P>` and calls `profile.save()` (sets the flag). A new
   `drain_profile(frame)` step runs alongside `drain_audio` in `advance_live`/`advance_observed`/
   `step_ticks`: it pushes a frame-stamped `ProfileSaveEntry` into a `profile_save_log` (regenerable,
   like `audio_log`) and, **when persistence is enabled**, emits `emit_doc("profile", …)` and writes it
   atomically. The atomic helper (`ludion_core::atomic::write_atomic(path, bytes)`) writes a hidden
   temp sibling in the same directory then `std::fs::rename(tmp, path)` — verified to replace-atomically on Windows and
   POSIX (the brief's "rename fails on Windows" is outdated). The helper is engine-local (its only
   consumer today is the profile save; promote to `ludion-core` under the "before a second way, remove
   the first" rule when a second consumer appears). Crash-test mechanization: a test can't kill a
   process mid-write, so it drives the helper's two steps by hand — write the temp, assert `dest` still
   holds the OLD bytes (rename not yet done), then rename and assert the swap — proving the intermediate
   state never exposes a partial or empty `dest`.
   *Rejected:* a separate `ProfileSaveQueue` resource — the flag belongs with the value the game
   already holds (one resource, one read). *Rejected:* writing inside the sim tick — breaks the
   no-I/O-in-sim rule; the drain is post-tick engine-level, exactly like audio. *Rejected:* a
   non-atomic direct `fs::write` — a crash mid-write corrupts the save (the failure the brief's
   crash-test exists to preclude).

5. **Replays regenerate save requests (like audio), never record them; exactly one mode writes disk —
   a live host run.** A save request is a pure function of sim state, so `replay_hashes_with` re-drives
   the same ticks and the same system re-decides to save at the same frame — the `profile_save_log`
   rebuilds identically with no LRP1 record (the audio contract, `audio.rs:14-16`; **unlike** the
   `DocReload` record, which *is* recorded because it is an external mutation the sim cannot
   regenerate). The disk write is out-of-sim and gated on host-enabled persistence: `ludion::run`
   enables it (a live host run, including `--record`, writes); `replay_hashes_with`, `--check-docs`,
   `--profile`, and every test leave it disabled (the log still fills — timing stays replay-visible —
   but no disk write clobbers the machine's real profile).
   *Rejected:* recording save requests in LRP1 — redundant (the sim regenerates them) and it would make
   a replay overwrite the developer's real save. *Rejected:* writing in all modes — a replay or a CI
   test would silently overwrite `save/profile.yaml`, a data-loss footgun.

6. **Profile-at-start rides an additive `ReplayHeader.profile: Option<String>` (canonical text), not a
   new record kind.** Profile-at-start is *boot state*, exactly like the scene (`SceneRef::Inline`), so
   it belongs in the header, not in a stream of frame-stamped records. The header is zstd'd JSON, so
   `#[serde(default)] profile: Option<String>` is additive with no framing change and `REPLAY_FORMAT_VERSION`
   stays 1 (`replay.rs:56`); every old `.lrp` decodes (its header simply lacks the key → `None`). It is
   populated in `replay_header()` (`engine.rs:926`) from the live profile's `emit_doc` text at
   record-arm time, and consumed at replay boot by a new `install_profile_from_text` step in
   `replay_hashes_with`, run after `load_header_scene`/`reseed`, **overriding the boot disk-load** — so
   a committed `.lrp` reproduces bit-exactly on a machine whose `save/profile.yaml` differs or is absent
   (the `apply_doc_reload_inline` off-disk precedent, `engine.rs:437`).
   *Rejected:* a new LRP1 record kind — profile-at-start is one boot value, not per-frame events; the
   header is its home (matching the scene). *Rejected:* re-reading disk at replay — defeats
   reproducibility across machines, the exact gap the header embedding closes.

7. **Boot/`scene.load` load-or-default from disk; `restore` (and seek) install the *snapshot's*
   embedded profile — so `SnapshotDoc` gains an additive `profile` field.** `install_profile` runs at
   the same three reset points as `install_docs` (`install_audio_queue` mold): at construction and
   `scene.load` it load-or-defaults from the resolved path (a fresh session reads the persisted
   profile). On `restore` the resource is reinstalled from the **snapshot's** embedded profile, **not**
   re-read from disk — a restore is an exact continuation, and `ludion dev`'s snapshot → process-restart
   → restore must preserve *unsaved* in-memory profile changes. So `SnapshotDoc` (`snapshot.rs:34-59`)
   gains `#[serde(default, skip_serializing_if = "Option::is_none")] profile: Option<String>` (the
   `scene_name` additive precedent, `:49-54`), captured in `capture()` from the live profile's canonical
   text and restored in `restore_into()`. Because the snapshot *is* the keyframe format, a replay
   **seek** (which reconstructs from a keyframe) then installs the correct profile too. `Profile<P>`
   itself stays a resource — excluded from `WorldHash` and from the entity serialization — reaching the
   hash only through derived components, which is exactly why the profile-at-start must travel in the
   header (linear replay) and the keyframe (seek / dev-reload).
   *Rejected:* re-reading disk on restore — loses unsaved dev-reload state and may read a stale/newer
   file; a restore must be exact. *Rejected:* not snapshotting the profile — a `ludion dev` reload would
   silently reset accumulated meta-state and a seek would diverge from the linear replay.
   Amended 2026-11-19 (owner): a checkpoint restore keeps the live profile, see `Plan/checkpoints.md` (d).

8. **Default `<project_root>/save/profile.yaml`, overridable with `EngineBuilder::profile_path(rel)`;
   no CLI flag, no test-only API.** The default resolves root-relative like every asset/doc (project
   root = the scene's directory, so for vigil it is `games/vigil/scenes/save/profile.yaml`, beside the
   scene's content — the E4 content-root rule). A game wanting another location calls `.profile_path(rel)`.
   Tests point at a scratch profile simply by building with a scratch `project_root` (the `doc_kind`
   test mold) — no test-only surface needed.
   *Rejected:* a `--profile-path` CLI flag — surface for a need G5 does not have (the path is a game
   decision, not a per-run one; add if a real need appears). *Rejected:* an absolute-path builder method
   — breaks the root-relative resolution every other content path uses.

9. **No wire method — 26 methods and `schema_hash` unmoved, verified.** Saving is a drained resource
   flag; loading is boot-time; the migration report surfaces via stderr + `Engine::profile_migration()`.
   None is a `Command`/`Response`/`Event`, so the wire surface stays 26. `schema_hash` is over the wire
   envelopes/bodies/handshake/frame-header only (`schema.rs:36-54`), excluding `ReplayHeader`,
   `SnapshotDoc`, and `P`'s schema (held on `Engine`, never in the registry) — so the header/snapshot
   additions and `P`'s schema move it not at all (the E4 precedent proves the direction).
   *Rejected:* a `profile.get`/`profile.save` wire pair — no consumer (G5 is game-side; an editor
   profile viewer is a recorded later trigger).

10. **Non-visual wave: a facade example + behavioral tests + G5 as proof, zero golden churn.**
    `crates/ludion/examples/player_profile.rs` registers a `Profile`-typed save (`unlocked_crossbow:
    bool`, `runs: u32`), a system that (a) spawns a different starting component when the profile is
    unlocked and (b) on a run-end condition sets the unlock and calls `profile.save()`, and a **two-run
    WorldHash behavioral test**: (i) a run from an unlocked profile diverges from a default-profile run
    (profile-at-start drives the hash through the system), and (ii) same seed + same profile + same
    inputs reproduces twice. G5 consumes exactly this pattern. Docs do not render, so there is **no
    gallery scene, no golden, and zero golden churn by construction** (the Phase 9/15 non-visual
    posture). Two new L-codes (`L1121`, `L1122`); two additive serde fields (`ReplayHeader.profile`,
    `SnapshotDoc.profile`), `REPLAY_FORMAT_VERSION`/`SNAPSHOT_FORMAT` both unchanged; no new component,
    no wire change. **If any wave discovers otherwise, that is a flagged decision, not a drive-by.**

## Waves

Ordering rationale: the pure P-agnostic file machinery + report type + codes first (Wave 1) → the typed
registration + `Profile<P>` + boot load-or-default + migration, the read path everything names (Wave 2)
→ the save/drain/atomic-write path on top (Wave 3) → the replay/snapshot embedding and the full
reproducibility story (Wave 4) → the runnable example, host wiring polish, guide, docs, and exit (Wave
5). Each wave is independently landable, one commit, ending in `node scripts/check.mjs` green.

### Wave 1 — profile file machinery (`open_profile`, `ProfileMigration`, `L1121`/`L1122`)

- `ludion-scene`: one lenient P-agnostic entry `open_profile(source) -> Result<ProfileEnvelope,
  Vec<Diagnostic>>` (strict-YAML → `Value` via `yaml::parse_document`; require `format` present and
  `kind == "profile"`, strip them, return `{ fields: Value, format: u64 }`; **no** strict field
  checker). Canonical write is the existing `emit_doc("profile", …)` re-exported for the engine's use.
  Bad/empty envelope or non-mapping root → a fatal `Vec<Diagnostic>` the engine turns into the
  backup-and-default path (decision 3).
- `ludion-protocol`: `ProfileMigration { entries: Vec<ProfileMigrationEntry> }` mirroring `RestoreReport`
  (`restore.rs:41-118`): `is_clean()`; variants `FieldsDefaulted { fields }` (informational),
  `FieldsDropped { fields }` (informational), `Reset { backup: String, reason: String }` (carries
  `L1121`); per-entry `code()`. Boundary-clean (strings only).
- `ludion-core`: **`L1121`** ("a saved profile could not be loaded as authored — it was backed up and a
  fresh default profile started") registered three-part. (**`L1122`** is registered in Wave 3, where its
  first reference site — the save path — lands; registering it here would fail the reachability test,
  `codes.rs:2339`, which requires every code const to be referenced outside `codes.rs`.)
- **Exit gate (mechanized):** `open_profile` unit tests — a good envelope returns the stripped field map
  + format; a missing/wrong `format`/`kind`, a non-mapping root, and unparseable YAML each Err. A
  **byte-idempotence proptest** over a hand-built schema + conforming `Value` (`emit_doc → open_profile
  → emit_doc` byte-equal — the `doc.rs` mold). `ProfileMigration` round-trips and carries `L1121`.
  `ludion explain L1121` works. `node scripts/check.mjs` green.

### Wave 2 — `profile::<P>()` registration + `Profile<P>` resource + boot load-or-default + migration

- `EngineBuilder::profile::<P>()` (`P: Serialize + DeserializeOwned + JsonSchema + Default + 'static +
  Send + Sync`) and `EngineBuilder::profile_path(rel)` (decision 8), capturing one `ProfileKind`
  (`Option<ProfileKind>` on `Engine`, the singleton analog of `doc_kinds: Vec<DocKind>`): the captured
  `schema_for!(P)` + monomorphized `load` (disk text → `Profile<P>` + `ProfileMigration`) / `emit`
  (live `Profile<P>` → canonical text) / `default` fns.
- `Profile<P>` resource in `player_profile.rs`: holds the `P` value + a `dirty` flag; `get`/`get_mut`,
  `save()` (sets `dirty`). Prelude re-exports `Profile`.
- `install_profile` at construction, `scene.load`, and `restore` (the `install_docs`/`install_audio_queue`
  mold): construction/`scene.load` load-or-default from the resolved path via the lenient migration
  (decision 3); an unloadable file → backup + `P::default()` + `L1121`. `Engine::profile_migration()`
  exposes the last load's report. (`restore`'s embedded-profile install is Wave 4; Wave 2 restore
  load-or-defaults from disk, tightened in Wave 4.)
- `"profile"` joins the engine-owned skip kinds: `ENGINE_KINDS` (`docs.rs:28`) and the facade mirror
  `ENGINE_OWNED_KINDS` (`ludion/src/lib.rs:453`), so E4's doc walk and `--check-docs` skip profile
  files instead of raising L1119 (the current-code fact above — without this, a game with any
  registered doc kind fails boot on its own save file).
- **Exit gate (mechanized):** a fixture registers a `Profile` and boots from a scratch root →
  `Profile<P>` present, value = the file's (or `Default` when absent). **Migration-report tests** over a
  `v1 → v2` `P`: a **field added** to `P` (file lacks it) → filled from default, reported; a **field
  removed** from `P` (file has it) → dropped, reported; a **field retyped** in `P` (file's value no
  longer deserializes) → file backed up, `P::default()` loaded, `L1121` + `Reset` entry, and the backup
  file holds the original bytes. A **corrupt YAML** file → same backup-and-default path, never a panic.
  The resource **survives `scene.load`** (reinstalled). Two boots from the same file produce identical
  `Profile<P>` and identical `WorldHash` (a system deriving a component from the profile). A game
  registering **both** a doc kind and a profile boots clean with a `save/profile.yaml` in the content
  root (the skip-kind proof), and `--check-docs` ignores it. `node scripts/check.mjs` green.

### Wave 3 — save path: drain + atomic write + persistence gate + save log

- `drain_profile(frame)` beside `drain_audio` in the three tick primitives (`advance_live`,
  `advance_observed`, `step_ticks`): if `Profile<P>.dirty`, clear it, push a frame-stamped
  `ProfileSaveEntry` into `profile_save_log`, and — when persistence is enabled — `emit_doc` + write
  atomically. `Engine::profile_save_log()` exposes the stream (the `audio_log` mold).
- `ludion_core::atomic::write_atomic(path, bytes)` (write a hidden temp sibling → `fs::rename`, creating parent dirs).
- Persistence gate: `EngineBuilder::persist_profile`, wired by
  the host in Wave 5; unset ⇒ log-only, no disk write (decision 5). A save-time IO error is a **warning**
  `L1122` (logged, non-fatal; the next request retries), never a sim crash.
- `ludion-core`: **`L1122`** ("a profile could not be written to disk this frame") registered
  three-part here, where its reference site lands (deferred from Wave 1 for the reachability test);
  `ludion explain L1122` works.
- **Exit gate (mechanized):** a `profile.save()` in a system produces a frame-stamped `ProfileSaveEntry`
  at the right frame; with persistence enabled the file appears on disk with the canonical bytes and is
  byte-idempotent (`emit_doc → open_profile → emit_doc`). **Atomic-write crash-simulation test**: drive
  the helper's two steps by hand — after writing the temp, `dest` still holds the OLD profile; after the
  rename, `dest` is the NEW one and the temp is gone (old profile intact through the whole window). A
  save-time IO failure surfaces `L1122` without panicking. A **replay-mode** run (persistence disabled)
  regenerates the same `profile_save_log` and writes **no** disk file. `node scripts/check.mjs` green.

### Wave 4 — replay + snapshot embedding, the reproducibility story

- `ReplayHeader.profile: Option<String>` (additive `#[serde(default)]`), populated in `replay_header()`
  from the live profile's `emit_doc` text; consumed by a new `install_profile_from_text` step in
  `replay_hashes_with`, run after `load_header_scene`/`reseed`, overriding the boot disk-load (decision
  6). `REPLAY_FORMAT_VERSION` stays 1.
- `SnapshotDoc.profile: Option<String>` (additive), captured in `snapshot::capture` and restored in
  `restore_into` → `install_restored`, so `restore` (and a replay **seek**) install the exact embedded
  profile instead of re-reading disk (decision 7). `SNAPSHOT_FORMAT` stays 1.
- **Exit gate (mechanized):** the **reproducibility test** — same seed + same embedded profile + same
  input records ⇒ identical `WorldHash` trace, run twice. The **cross-machine** test — a recorded
  session with a non-default profile-at-start reproduces its trace from the committed `.lrp` on a
  checkout whose on-disk `save/profile.yaml` **differs or is absent** (the embedded profile wins). A
  run from an unlocked profile **diverges** from a default-profile run at frame 0 (profile-at-start
  matters). **Every pre-E5 committed `.lrp` still decodes** (old header lacks `profile` → `None`); a
  snapshot round-trip carries the profile; a `dev`-style snapshot→restore preserves the in-memory
  profile bit-exactly. Wire surface still **26**, `schema_hash` unmoved. `node scripts/check.mjs --full`
  green (the determinism canary).

### Wave 5 — facade example, host wiring, guide chapter, docs topics, exit

- `crates/ludion/examples/player_profile.rs` (decision 10): the copyable G5 pattern — register a
  `Profile` save, a system that reads it (unlock → different starting state) and writes it on a run-end
  condition, and the two-run WorldHash behavioral test (unlocked-diverges + same-inputs-reproduces).
- Host wiring in `ludion::run`: after `load_scene_file`, enable profile persistence at the resolved
  `<root>/save/profile.yaml` (the `set_audio_sender` mold, `lib.rs:217-219`) and print any boot
  `ProfileMigration` (`L1121` + filled/dropped) to stderr — so a real game actually persists and reports
  drift. (`--record` inherits it; `--check-docs`/`--profile`/replay stay disabled.)
- Engine-guide chapter **"Player profiles"** (placed after "Game data-docs"): `profile::<P>()`
  registration + `P: Default`, `Profile<P>` access + `profile.save()`, the load-or-default + lenient
  migration + backup behavior, the `save/profile.yaml` location + `profile_path`, the reproducibility
  guarantee (seed + profile-at-start + input replay), and the non-goals (one slot, no cloud sync, no
  encryption). Docs topics via the `ludion-docs` skill: `ludion-scene` (`open_profile`), `ludion-engine`
  (`profile::<P>` + `Profile<P>` + migration + save drain + LRP1/snapshot embedding), `ludion` (facade
  registration + host persistence), `scene-format` (the `kind: profile` envelope), `determinism`
  (profile-at-start embedded, save requests regenerated not recorded). No new crate → no new topic;
  `embedded_topics_match_the_docs_directory` satisfied by rebuild.
- **Exit gate (roadmap "ready"):** round-trip byte-idempotence proptest (W1) + migration-report tests
  (W2) + atomic-write crash test (W3) + reproducibility test (W4) all green; wire surface **26** +
  `schema_hash` unmoved; no golden churn; example + behavioral test green; guide chapter + docs topics
  shipped. **Handoff to G5:** unlocks/knowledge-flags/lifetime-stats are `Profile<P>` fields, run-end
  calls `profile.save()`, and the reproducibility gate is the W4 embedding. **No human gate is owned by
  E5** — M5's G5 gate ("die, restart, feel the difference the previous run made") judges the facility in
  anger.

## Explicitly out of scope (triggers recorded)

- **Multiple profiles / save slots** — one slot at `save/profile.yaml`; trigger: a second person plays
  the same install (the brief's non-goal).
- **Cloud / Steam sync, encryption / anti-cheat** — text-is-truth applies to saves; trigger: none
  foreseen for the prototype (anti-cheat is a whole-engine non-goal).
- **Preserving unknown fields across a re-save** — canonical emit writes only schema fields; the
  newer-`format` → backup rule (decision 3) protects a newer save from an older build; trigger: a real
  need to round-trip forward-compatible fields (would require a shadow field store).
- **Editor profile UI** — the inspector edits components, not the profile; trigger: hand-editing the
  profile becoming recurring friction (watch G5/G6).
- **A generic `.resource::<R>(value)` builder seam** — `profile::<P>()` and `doc_kind::<T>()` are the
  two seams the game consumes; trigger: a third, non-doc non-profile resource a game registers at build.
- **A `--profile-path` CLI flag** — the path is a game decision; trigger: a host that must relocate the
  save at run time.
- **Promoting `write_atomic` to `ludion-core`** — engine-local until a second consumer; trigger: a
  second atomic-write site (e.g. `scene.save` wanting crash-safety).

## Risks

- **The two "profile" meanings must never blur in the code or docs.** `Profile<P>`/`player_profile.rs`
  (save) vs `ProfileReport`/`Budget`/`profile.rs`/`--profile` (perf). Contained by the decision-1 naming
  discipline (distinct file, distinct types, qualified prose) and a rustdoc pass in Wave 5; a reviewer
  checks no doc sentence says "profile" unqualified where both could be meant.
- **Lenient migration must never destroy or brick a save.** The whole point is that a player's bytes
  survive any schema drift. Contained by the Wave 2 backup-file assertion (the original bytes are on
  disk after a retyped/corrupt load) and the never-panics posture (every failure path is `L1121`, a
  warning). A reviewer confirms no migration path calls `panic!`/`unwrap` on file contents.
- **The LRP1 header addition is a replay-format change.** Contained by making it additive
  (`#[serde(default)]`, format stays 1) and gating on both the old-`.lrp`-still-decodes test and the
  cross-machine reproducibility test (Wave 4) — the exact posture E4 used for `DocReload` and E2 for
  `InputEvent`. `schema_hash` composition (`schema.rs:36-54`) confirms the header is outside it, but the
  Wave 4 gate re-asserts `schema_hash` unmoved regardless.
- **`SnapshotDoc.profile` must not perturb a profile-free snapshot's bytes.** The additive field is
  `skip_serializing_if = "Option::is_none"` (the `scene_name` precedent), so a game with no registered
  profile snapshots byte-identically. Contained by a Wave 4 test that a profile-free snapshot round-trips
  unchanged and that `SNAPSHOT_FORMAT` stays 1.
- **Persistence-gate correctness: exactly one mode writes disk.** A replay or CI run must never
  overwrite the developer's real `save/profile.yaml`. Contained by decision 5 (writes gated on a
  host-set path that only `ludion::run` sets) and the Wave 3 replay-mode test (log fills, no file
  written). A reviewer confirms `replay_hashes_with`, `run_profile`, and `run_check_docs` never enable
  persistence.
- **Sequencing vs open gates.** E5 adds no wire method, no component, and no golden; it shares only the
  guide/docs files and two additive serde fields with the open Phase 8/9/M0/M1/M2 human gates, so it is
  safe to run with those queued. Per roadmap rule 2 E5 is the engine wave in flight during M4 (its
  consumer is G5/M5); if the user directs its start ahead of the queued human gates, that is the E3/E4
  rule-2-throttle-override precedent, gates unaffected.
