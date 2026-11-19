# Plan: Sequencer — `kind: sequence`, keyed tracks, skip

> **Waves 1–2a mechanized — the document, `sequences::play`, six tracks, skip, `holds_gameplay` and `wait: line_end`/`release` ship; the per-entity clock domain (W2b), transform spans and preview are laid out below.**
> Driven by Neon Shroud request 0025 (sequencer / timeline). Game-facing doc: `ludion docs sequences`.

## Design decisions

1. **Opt-in, not builtin.** `Engine::builder().sequences()` registers `kind: sequence` with
   `doc_kind_engine_owned::<SequenceDoc>`, one `validate_docs` pass, the components `SequencePlayer` and
   `SequenceBinding` through the builder's `components` closure, and installs an empty `SequenceEvents` at every world
   build. Nothing is builtin, so `component_schemas.json`, the protocol schema, goldens, replays and every existing
   `WorldHash` are untouched; a game that opts in and never plays hashes as if it had not.
2. **Scheduling.** The game schedules `.system("sequence_update", sequence_update)` before `dialogue_update` (a dialogue
   key starts the same tick) and before the systems that read `SequenceEvents`. A built-in schedule placement for CLI
   preview is W4.
3. **Keyed by path.** The document is keyed by its content-root path (the `PatternPlayer.pattern` rule); the request's
   `name:` is dropped.
4. **API marks, the system does.** `sequences::play(world, key, &[(role, entity)]) -> Result<SequenceHandle, Diagnostic>`
   spawns one `SequencePlayer` (`Pending`) and one `SequenceBinding` per pair; `skip` and `cancel` set a flag. This
   differs from the request's `SequencePlayer::new(..).bind(..)` because a list of entity refs is refused (`L1392`).
   `sequence_update` stamps `started` from the clock on its first run and applies the keys at 0 that tick.
5. **Evaluation.** Playhead = `now − started`, `now` = `ctx.game_frame` (`clock: game`, default) or `ctx.frame`
   (`clock: sim`), the clock copied into the player at start. Every key with `applied_through < at <= playhead` applies
   (`applied_through: Option<u32>`). A held clock re-applies nothing; under a time scale keys apply the tick the floor
   frame crosses them. Order per player: track (camera, animation, ui, audio, dialogue, event), then `at`, then
   document order; players in ascending `EntityId`, so a later player's write to the same component wins.
6. **Completion.** On the tick `playhead >= length_ticks` (a sequence started on F ends `Completed` on F+L; keys at
   `at == L` apply that tick) `Ended { Completed }` is emitted and the player and its bindings are despawned; the handle
   is valid until it ends (the `LineHandle` rule).
7. **Camera track.** A key has exactly one of `rig: <RigConfig map>` (inline; no rig doc kind exists) or
   `restore: true`, plus `snap: bool` (default false). The first applied rig key saves the role's current
   `CameraRig.rig` into its `SequenceBinding.saved_rig`; `restore` writes it back.
8. **Skip.** `sequences::skip -> bool` is `false` when not live or the doc says `skippable: false` (default `true`:
   whether "seen once" is a game rule). At the next update, the **last** camera, animation and ui key of each role, if
   not yet applied, applies in end form (camera: write, then `snap_rig` for every camera role touched; animation: hard
   cut at the game frame; ui: the tween written with every `once` track's `duration` 0 and `start` = now). Unplayed
   audio and dialogue keys are dropped, the lines it said are interrupted (found by `DialogueLine.seq`), unfired
   `on_skip: fire` events (default) fire in order and `drop` ones do not. Then `Ended { Skipped }`. A beat's state
   outcome never depends on whether it was watched.
9. **Cancel.** `sequences::cancel -> bool`: no end state, lines interrupted, `Ended { Cancelled }`. A document unloaded
   mid-play cancels the same way (`L4353`).
10. **Events.** `SequenceEvents`, rewritten every run: `KeyApplied { handle, track, at }` (the W1 trace; a skip emits
    it for each end form), `Fired { handle, name }`, `Ended { handle, end }`, with `fired(handle, name)` and
    `ended(handle)`.
11. **Later waves own their fields.** The transform track, voices, music and duck keys, preview and wire are unknown
    fields today (`L1120`), never silently ignored. Holds and waits shipped in W2a (decisions 13–18).
12. **UI `start`.** A ui key's track `start` is a frame offset after the key applies, on the track's own clock
    (`real` → `ctx.frame`, `game` → `ctx.game_frame`).
13. **`holds_gameplay: bool`** (default false) on `SequenceDoc`. While the player is live (Pending or Playing, waiting
    included) `sequence_update` calls `GameClock::hold()` once at its end, except on the tick the player ends. It
    implies `clock: sim`, since a held game clock would never advance the playhead; an explicit `clock: game` with it
    is `L1734`. `SequenceDoc.clock` is an `Option`; `effective_clock()` resolves it.
14. **Waits.** `DialogueKey.wait: line_end` and `EventKey.wait: release` (one-variant enums, so a wrong wait on a track
    is a serde error). Two wait keys at the same `at`, across both tracks, are `L1735`: one pending wait per player.
15. **Playhead with waits.** `playhead = now − started − waited`. A pending wait clears when its dialogue line no
    longer exists (`DialogueLine.seq`) or when `released` is set (consumed); while it holds, nothing applies and the
    player cannot complete, even at `at == length_ticks`. On clear, `waited = elapsed − wait.at` and later keys apply
    from the next clock frame. The playhead is clamped to the earliest due wait key before any track applies. Keys at
    the wait's own `at` in every track apply that tick. A refused `say` (`L4353`) sets no wait.
16. **`sequences::release(world, handle) -> bool`**: `false` when not live; otherwise sets one latch (not a counter)
    that clears the current `wait: release` or the next one reached, so an early release cannot stall the beat.
17. **Skip and cancel ignore waits.** Skip still applies every end form and fires every `on_skip: fire` event; the hold
    ends with the player.
18. **`sequences::holding(world) -> bool`** mirrors `dialogue::holding`: a live `holds_gameplay` sequence is holding
    the clock.

## Waves

- **W1 (shipped):** the document and validator (`L1730`–`L1733`), `play`/`skip`/`cancel`/`is_playing`, `SequenceEvents`,
  camera, animation, ui, audio, dialogue and event tracks, clocks `game` and `sim`, `L4350`–`L4354`, the example and
  `ludion docs sequences`.
- **W2a holds (shipped):** `holds_gameplay`, `wait: line_end`, `wait: release`, `sequences::release`/`holding`,
  `L1734`/`L1735`.
- **W2b per-entity clock domain:** `SequenceBound { player, bound_sim, bound_game }`, added and removed by the engine on
  the bound roles and their hierarchy children. It gives a per-entity instant `bound_game + (sim_now − bound_sim)`
  while bound. Every gameplay-time reader takes it: `extract.rs` closed forms, `parent_joint.rs`,
  `animation_events.rs`, `blend_clock.rs`, `camera_rig.rs`, `path_follow.rs` and the sequence's own animation stamp.
  At release the stamps each actor took during the hold (`Animation.started`, crossfades and layers, the rig blend
  start) shift back by the lead, so nothing jumps. `docs/game-clock.md`'s "no per-component opt-out" line is amended
  there (request amendment 2, still Proposed).
- **W3:** the transform track (anchor pose and ease, `kind: path` spans), audio voices stopped on skip,
  `music_crossfade`, duck keys, the camera-zone override flag (with request 0010).
- **W4:** built-in schedule placement, `ludion render --sequence --bind --at`, wire `sequence.play/seek/skip` (a
  protocol schema move), a gallery scene and goldens, testkit `sequence_trace`.

## Out of scope

- An editor timeline UI, branches and conditions: indefinitely (Pillar 6; the game's Rust decides what plays).

## Proposed — owner ruling owed

- Request amendments 2 (a per-entity clock domain, W2b) and 3 (waits are not a state machine).
- `SequenceDoc.clock` becomes an `Option` (a public field type; no game uses sequences, grep of both game roots
  2026-11-19), and `sequences::release` is a single latch taking a handle, not the request's `release(entity)`.
- `skippable` defaults to `true`.
- The API differs from the request (decision 4): free functions over `&mut World`, one entity per binding.

## Status

- W1 mechanized: `crates/ludion-engine/src/sequences/`, `tests/sequences.rs`, `crates/ludion/examples/sequence.rs`.
- W2a mechanized: `sequences/{docs,mod,update}.rs`, `tests/sequences.rs`, `L1734`/`L1735` in `codes.rs`,
  `docs/sequences.md`, `docs/game-clock.md`, the example. Limit until W2b: a `holds_gameplay` beat freezes its bound
  actors' animation and camera; only real-clock ui tracks, audio, dialogue and events advance.
