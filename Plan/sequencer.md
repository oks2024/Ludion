# Plan: Sequencer — `kind: sequence`, keyed tracks, skip

> **Wave 1 mechanized — the document, `sequences::play`, six tracks and skip ship; holds, transform spans and preview are laid out below.**
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
11. **Later waves own their fields.** Waits, `holds_gameplay` with the per-entity clock domain, the transform track,
    voices, music and duck keys, preview and wire are unknown fields today (`L1120`), never silently ignored.
12. **UI `start`.** A ui key's track `start` is a frame offset after the key applies, on the track's own clock
    (`real` → `ctx.frame`, `game` → `ctx.game_frame`).

## Waves

- **W1 (shipped):** the document and validator (`L1730`–`L1733`), `play`/`skip`/`cancel`/`is_playing`, `SequenceEvents`,
  camera, animation, ui, audio, dialogue and event tracks, clocks `game` and `sim`, `L4350`–`L4354`, the example and
  `ludion docs sequences`.
- **W2 holds:** `holds_gameplay` and the engine-owned per-entity clock domain (`SequenceBound`), `wait: line_end` and
  `wait: release`.
- **W3:** the transform track (anchor pose and ease, `kind: path` spans), audio voices stopped on skip,
  `music_crossfade`, duck keys, the camera-zone override flag (with request 0010).
- **W4:** built-in schedule placement, `ludion render --sequence --bind --at`, wire `sequence.play/seek/skip` (a
  protocol schema move), a gallery scene and goldens, testkit `sequence_trace`.

## Out of scope

- An editor timeline UI, branches and conditions: indefinitely (Pillar 6; the game's Rust decides what plays).

## Proposed — owner ruling owed

- Request amendments 2 (a per-entity clock domain, W2) and 3 (waits are not a state machine).
- `skippable` defaults to `true`.
- The API differs from the request (decision 4): free functions over `&mut World`, one entity per binding.

## Status

- W1 mechanized: `crates/ludion-engine/src/sequences/`, `tests/sequences.rs`, `crates/ludion/examples/sequence.rs`.
