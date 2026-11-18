# Plan: Input actions — a data-driven action map, gestures, rebinding, glyphs, rumble

> **Waves 1a (sim side) and 1b (engine opt-in) mechanized; Waves 2–4 not started.**
> Driven by Neon Shroud request 0021: a game that is played on keyboard and pad needs to name what
> the player means (`crouch`, `move`), bind it to several physical inputs, tell a tap from a hold from
> a double-tap, let the player rebind, show the right glyph and shake the pad. Game-facing doc:
> `ludion docs input`. Amends `Plan/phase-13-input.md`, which is closed: its out-of-scope list named
> remapping and rumble; the promotion of both is **Proposed — owner ruling owed** (see below).

## Design decisions

1. **The action layer is sim-side data.** The map is a `kind: input-actions` document: gesture times
   and named actions, each with a `kind` (`button`, `axis1`, `axis2`) and a list of bindings. The
   document types and the compiler live in `ludion-sim` (serde only, no YAML there); the engine loads
   the file (Wave 1b). Existing `Action` constants and every `InputState` read stay as they are, so
   a game that never loads a map sees no change.
2. **The `Actions` resource resolves once per tick**, from the `InputState` the driver has just
   updated, and games read it by `ActionName`. The driver resolves it when the world holds one
   (normative order, step 1); a world without one is untouched, so no `WorldHash`, trace, replay or
   golden moves.
3. **Gesture times are ticks, never `game_dt`.** The document states seconds; the compiler rounds half
   up at the tick rate, minimum one tick. A `GameClock` hold or slow motion never stretches a
   threshold. The defaults are `tap_max` 0.22 s, `hold` 0.35 s and `double_tap_window` 0.25 s.
4. **Gestures are per physical input**, so two actions can read one key by different rules. With `n`
   the ticks the input has been down (1 on the press tick): *press* fires on the down edge and is a
   level while down; *hold* fires on the tick `n == hold` and is a level until release; *tap* fires on
   release when no hold fired and `n <= tap_max`, immediately — or, when the same input also has a
   `double_tap` binding, after `double_tap_window` ticks unless a second press arrives (that press fires
   the `double_tap` and cancels the tap; its own release yields no new tap); *double_tap* fires on the
   second press and is a level while that press is held. A tap is an edge only. The expiry is checked
   before a down edge in the same tick: a press at `up + window` is too late.
5. **Trigger hysteresis is per binding.** A `trigger` binding carries `press` (default 0.5) and
   `release` (default 0.7 × `press`) and keeps its own state. The legacy `GamepadButton::*Trigger`
   stays at the fixed 0.5 with no hysteresis, so no recorded trace changes.
6. **One flat binding struct.** A binding is one struct of optional fields (`key`, `mouse`, `pad`,
   `trigger`, `wheel`, `stick`, `keys`, `key_pair`, `pad_pair`, `mouse_delta`) plus modifiers
   (`gesture`, `press`, `release`, `deadzone`, `scale`, `invert_x`, `invert_y`), not a tagged enum, so
   a compile error names the field. The request drew `mouse_delta` as a nested table; it is
   `mouse_delta: true` here so the modifiers sit on every binding the same way.
7. **Compile collects every error** (`L1630`–`L1635`), each with a JSON Pointer
   (`/actions/<name>/bindings/<i>/<field>`) and a fix. A duplicate is the same physical input and
   gesture in one `context`.
8. **Axes combine by first non-zero binding in binding order.** A stick goes through the radial
   deadzone (default 0.2), then `scale` and the inverts; a key cluster is the `move_vector` digital
   rule (1/√2 diagonals); `mouse_delta` is the tick's raw motion times `scale`.
9. **The full key set waits for W3.** A new `Key` or `MouseButton` variant changes the `InputEvent`
   schema, which `replay_content_hash` covers, so every committed `.lrp` would fail `L3010` until
   re-recorded. Wave 1a therefore binds only today's keys and ships a guard test (a serialized script of
   every existing key and button against a checked-in literal, plus pinned names and order). W3 appends
   the whole keyboard (and `MouseButton::Back`/`Forward`) beside the connection events, with one corpus
   re-record (Proposed below).
10. **The gesture state is part of the resource, not the hash.** `Actions::state()` /
    `restore_state` carry it for a snapshot (Wave 1b), so a hold continues across a restore on the same
    tick.

11. **The map is an engine-owned singleton doc kind** (W1b). `Engine::builder().input_actions()` pushes
    `DocKind::new::<InputActionsDoc>("input-actions", singleton)` without setting `game_doc_kinds`
    (the `patterns()` shape), adds `validate_input_actions` and `carry_resource::<Actions>()`. A
    missing or doubled file is the existing `L1315`; compile errors are re-pathed to
    `input-actions/<doc key><pointer>`.
12. **Install and carry semantics.** `Actions` is installed beside `install_nav` after every world
    build, unless one is present. A scene load and an `engine.restore` get a fresh `Actions`; a scene
    switch and a checkpoint carry the departing one (carriers run before the install).
13. **The latch rule.** A fresh `Actions` latches, on its first `resolve`, every input at level with no
    down edge that tick (a trigger counts at `press`; wheel steps never latch). A latched input fires no
    edge, has no level or tap and no `held_ticks`; its release is silent and clears the latch. So a key
    held across a load or restore needs a fresh press and never fires a spurious gesture.
14. **Unknown names report `L4270` once each**, drained at the frame boundary into `Diagnostics`
    (key `input-actions/<name>`), with the nearest action names, or all of them when fewer than eight.
15. **Rebuild model (W2a).** `ActionMap` keeps its source `InputActionsDoc` and `TickRate`; `Actions`
    keeps the defaults and an `InputBindingsDoc` of overrides. Every mutation builds the effective doc
    (defaults, each overridden action's `bindings` replaced), recompiles it, and on success swaps the map
    in, keeping gesture state, this tick's outputs and reads for what is still tracked. On error nothing
    changes and the compile errors return. `overrides()` holds only actions that differ from the defaults.
16. **The latch generalises** (replaces decision 13's "first resolve"): an input with no gesture-state
    entry that is at level with no down edge this tick is latched; a trigger with no entry at or above
    `press` too. A layer's first resolve has no entries, so W1b is unchanged; an input newly tracked
    after a rebind is covered, so the key pressed to capture does not fire on the next tick.
    `restore_state` suppresses the latch for its next resolve.
17. **Capture** (`capture`, `capture_result`, `cancel_capture`, `resolve_conflict`): button actions only;
    resolved inside `resolve` after the gesture step. A cancel input wins; otherwise the first press edge
    of the slot's device class by `Ord` (keys, mouse buttons, pad buttons; a trigger button becomes
    `trigger: left|right`). `Index(i)` replaces binding `i` (`i == len` appends); `Keyboard`/`Pad`
    replace the first binding of that class or append; `keep_gesture` copies the replaced gesture.
18. **Conflict policy.** The same source and gesture on another button action in the same `context` is a
    `Conflict`, the candidate stays pending. `Swap` gives the other action the binding we replaced (or
    removes its binding if we appended); `Replace` removes it; `Keep` cancels. Both edits apply atomically,
    nothing may leave an action with no bindings (`L4275`), and a locked other action allows only `Keep`
    (`L4272`). An existing own binding is `Unchanged`.
19. **`locked` is enforced:** every direct edit, capture or override of a locked action is `L4272`.
20. **Lenient overrides.** `apply_overrides` replaces the current overrides and judges each action alone,
    returning warnings at `/actions/<name>`: `L4273` unknown, `L4272` locked, `L4274` does not compile
    (including against another override); that action keeps its defaults.
21. **`save()`** raises a one-shot flag read by `take_save_request()`; the sim layer writes nothing.
22. **`UiNav::from_actions`** reads the reserved `ui_prev`, `ui_next`, `ui_activate` actions; a field whose
    action is undefined keeps `from_input`'s default. `ui_back` is read through `actions.pressed`.
23. **The overrides file (W2b).** `<save_root>/save/input-bindings.yaml`, `format: 1`, `kind: input-bindings`,
    body an `InputBindingsDoc`, canonical via `emit_doc`. It is read leniently: a missing file is no overrides;
    a bad envelope (`L1636`), bad YAML, a value of the wrong shape or a newer `format` is renamed to
    `input-bindings.yaml.bak.<hash>` and the defaults start with one `L4277`; a single bad action never
    resets the file, `apply_overrides` drops it (`L4273`/`L4274`) and applies the rest. A game always boots.
    It is written only on `Actions::save()`, at the frame boundary, crash-durably; a failure is a non-fatal
    `L4278` and the previous file stays.
24. **The host switch.** `persist_input_bindings()` (the `persist_profile`/`persist_settings` mold) gates both
    the read and the write, and needs `input_actions()`. Tests, replays and `--check-docs` leave it off; the
    facade's live boot chains it. Without it `save()` is consumed silently.
25. **At-start overrides.** With the switch on, the file is read at build into the engine's at-start overrides,
    applied to every **fresh** `Actions` (a build, a load, a restore); a carried one (a switch, a checkpoint)
    keeps its own. So a restore after a mid-session rebind reverts to the at-start overrides; exact
    continuation is the Proposed snapshot field. The apply warnings reach `boot_warnings` once, at build.
26. **Recording and replay.** A recording's `ReplayHeader` carries the at-start overrides as canonical text in
    the generic `save_docs` map (key `input-bindings`), omitted when empty. `save_docs` is outside
    `replay_content_hash` and the protocol schema hash, so no pinned artifact moves; it is the one header slot
    for per-machine save documents, which the settings wave reuses for settings-at-start. A replay installs the
    header's entry (or the defaults when absent) and never reads, writes or backs up the file, so a
    committed recording reproduces on any machine.
27. **Rumble is an outbox (W4).** `Rumble` is a plain resource the engine drains at the frame boundary to
    the window host's pad thread, the audio contract: never hashed, snapshotted, recorded or on the wire; a
    replay re-issues the requests by re-running the systems; headless and in the out-of-process viewer it is
    a silent no-op. The shared `RumbleCommand` lives in `ludion-core`, as `AudioMessage` does.
28. **Validation and mixing.** Motors clamp to 0..=1 and `duration` to 0..=2 s; a non-finite value is 0 and
    reports `L4279`; a zero duration or two zero motors is dropped. On the pad, overlapping effects take the
    maximum per motor and nothing plays longer than 2 s. `stop_all()` discards the tick's earlier plays.
29. **Intensity.** `Engine::set_rumble_intensity(0..=1)` scales every request and 0 drops every `Play`; a
    non-finite value is 1. Settings W3 binds the player setting to it.
30. **Backend.** gilrs force feedback on Windows (WGI), strong motor for `low`, weak for `high`. gilrs's
    macOS force feedback is a no-op stub, so rumble is silent on macOS.
31. **Stops.** Rumble stops when the window loses focus (the host sends `StopAll` itself, without the sim)
    and when the primary pad disconnects; with no primary pad commands are discarded.

## Waves

- **W1a — sim side (shipped):** `InputActionsDoc`, `ActionMap::compile`, `Actions`;
  gestures, trigger hysteresis, axes; the driver hook; `L1630`–`L1635`, `L4270`, `L4271`.
- **W1b — engine opt-in (shipped):** `Engine::builder().input_actions()`, an engine-owned
  doc-singleton kind registered like `patterns()`, install beside `install_nav`,
  `carry_resource::<Actions>()`, the latch rule, unknown names drained into `Diagnostics` as `L4270`,
  facade prelude, a runnable example. The snapshot field for `ActionState` moved to Proposed (below).
- **W2a — rebinding, sim side (shipped):** rebind/unbind/reset, capture with conflict resolution,
  `locked` enforcement, `InputBindingsDoc` with lenient `apply_overrides`, the `save()` flag,
  `UiNav::from_actions`; `L4272`–`L4276`. No file, no engine change.
- **W2b — persistence (shipped):** `persist_input_bindings()`, `<save_root>/save/input-bindings.yaml`
  (`kind: input-bindings`, loaded leniently through `open_envelope` + `backup_file` and `apply_overrides`),
  the save drain, `ReplayHeader.save_docs["input-bindings"]` (outside `replay_content_hash`), replay
  installing it and never reading or writing the file, the doc walk skipping `<save_root>/save`, the `rebind_menu` example; `L1636`, `L4277`, `L4278`.
- **W3 — devices, glyphs and the full key set:** the whole keyboard appended after `Alt` plus `Back`/`Forward` (one corpus re-record), the window keymap and mouse mapping for them, recorded `gamepad_connected{family}` / `gamepad_disconnected`,
  `last_device` / `device_changed`, `Actions::source`, a glyph table (`glyph_name`), a focus event
  (Proposed).
- **W4 — rumble (shipped):** the outbox, the validator, the pad-thread mixer and the gilrs backend on Windows (decisions 27–31); macOS is silent.

## Status

**W1a**: the sim-side action layer, bindings limited to today's keys. The existing key
and button names and order are pinned, and a serialized script of every one is checked against a literal
written before any change, so `replay_content_hash` stays put. `Actions` is tested at every gesture
boundary, trigger hysteresis, axes, every compile error with its pointer, a two-run `SimDriver`
comparison and a mid-hold state round trip. No pinned artifact moved.

**W1b**: the engine loads the map. `input_actions()` registers the singleton kind, its validator and the
carrier; `Actions` is installed after every build, carried across a switch and a checkpoint, fresh on a
load and a restore, with the latch rule making a held input need a fresh press there. Tests cover a pad
tap and hold, the keyboard route, a bad document's `L1632` at its re-pathed pointer, `L1315`, one
`L4270`, the restore latch, a hold carried across a switch, and a 1/2/8-thread hash trace. No snapshot,
schema, replay or golden moved.

**W2a**: rebinding is sim-side and in-memory. Tests (`ludion-sim/tests/rebind.rs`) cover round trips
with minimal overrides, an invalid rebind leaving the map unchanged, every locked refusal, capture (pad,
keyboard with a held key and the latch, Escape, a trigger, two presses by `Ord`), conflicts (Swap, Replace,
the last-binding refusal, Keep, a locked other action), lenient overrides, the save flag and a scripted
capture-and-swap that replays identically; `UiNav::from_actions` has a doctest and a unit test. No
`WorldHash`, trace, replay, golden or schema moved.

**W2b**: the overrides persist. `persist_input_bindings()` reads the lenient file at build and writes it
on `Actions::save()`; recordings carry the at-start overrides in `ReplayHeader.save_docs` and a replay
installs them from there. Tests cover save and reload, a lenient boot (`L4273`, `L4274`), a malformed file
backed up (`L4277`), the switch off (file neither read nor written), header-wins replay on a machine whose
file differs or is absent, no `save_docs` key without overrides, a restore reverting to the at-start
overrides, and the `replay_content_hash` and protocol schema hash staying put. The `rebind_menu` example
plays a rebind across two boots. No `WorldHash`, trace, replay, golden or schema moved.

**W4**: rumble. `Rumble`/`RumbleEffect` (`ludion-engine`), `RumbleCommand` (`ludion-core`), the drain to the
facade's pad thread, a pure per-motor mixer in `ludion-window` and the gilrs backend, `L4279`, the
`rumble` example and `docs input` §Rumble. Tests cover delivery, clamping and `L4279`, `stop_all`,
intensity, no sender, restore and rebuild leaving an empty outbox, and an identical `WorldHash` trace with
and without the playing system; the mixer is unit-tested. The pad thread polls every 8 ms while a primary
pad is connected and every 100 ms otherwise. No `WorldHash`, trace, replay, golden or schema moved.
**Owed:** rumble felt on a real pad on the Windows box, and the silence confirmed on the M4.

## Proposed — owner ruling owed

- **A macOS rumble backend** (GameController / CHHaptics). It is platform-shaped code in `crates/`, which
  needs the owner's ruling (a cargo feature). Trigger: the game needs rumble on Mac.
- **The 8 ms pad poll.** A rumble command waits at most one poll, so the thread wakes every 8 ms while a
  primary pad is connected and every 100 ms without one. A pad event channel would remove the polling.

- **`UiNav.back` (breaking).** `UiNav` is built by struct literal in games and docs, so a new field breaks
  every one. W2a ships `from_actions` with no `back` field; games read `ui_back` through `actions.pressed`.
- **Overrides in snapshot keyframes.** Mid-session overrides need a `SnapshotDoc` field, which moves
  `snapshot_schema()` and rides W3's one corpus re-record, like `ActionState`.

- **A snapshot field for `ActionState`** (exact mid-gesture continuation across `engine.restore`, a
  replay seek and a bisect keyframe). A new `SnapshotDoc` field changes `snapshot_schema()`, which
  `replay_content_hash` covers, so it rides W3's one corpus re-record. Until then a restore installs a
  fresh `Actions` and the latch rule applies; a plain linear replay never restores, so it is exact.
  `Actions::state()` / `restore_state()` stay for it.
- **One corpus re-record in W3** for the full key set: new `Key`/`MouseButton` variants move `replay_content_hash`, so every committed replay is re-recorded once, last, after the schema snapshot.
- **Promote remapping and rumble out of `Plan/phase-13-input.md`'s out-of-scope list** (request
  amendments 1–2). Wave 1a builds none of rebinding or rumble; it is the sim side they stand on.
- **Sensitivity and invert are binding modifiers** (`scale`, `invert_x`, `invert_y`), so they persist
  with a player's rebinds in W2.
- **A tap on an input that also has a `double_tap` binding is deferred** by `double_tap_window`. The
  cost is that latency; the alternative (fire at once, then double-tap) cannot cancel the tap.
- **Legacy `GamepadButton::*Trigger` keeps the fixed 0.5 with no hysteresis**; hysteresis is per
  binding only, so no replay moves.
- **`locked: true`** is parsed now and enforced by W2's rebinder.
- **Axis combination: first non-zero binding in binding order.** Summing or taking the largest
  magnitude are the alternatives.

## Explicitly out of scope (triggers recorded)

- **`stick_dir` button bindings** (a stick direction as a button) — trigger: a game that wants menu or
  dodge input from a stick flick.
- **Per-binding gesture thresholds** — trigger: a game needing a different hold time for one action.
- **Live `asset.reload` of the map** — `Actions` compiles at world build and is not rebuilt by a
  reload; trigger: tuning an action map in the editor.
- **Keys beyond today's set in bindings** — trigger: W3 (needs the re-record above).
- **Keyboard-layout labels** (what the key at a physical position says on this layout) — trigger: glyphs
  for non-QWERTY players; arrives with W3's glyph table.
- **Gyro** and **multiple pads** — triggers as in `Plan/phase-13-input.md`.
- **The editor's `keymap.ts` forwarding of the new keys** — a hand mirror; trigger: W3's keys landing.
