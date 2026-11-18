# Plan: Input actions — a data-driven action map, gestures, rebinding, glyphs, rumble

> **Wave 1a mechanized (sim side); Waves 1b–4 not started.**
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

## Waves

- **W1a — sim side (shipped):** `InputActionsDoc`, `ActionMap::compile`, `Actions`;
  gestures, trigger hysteresis, axes; the driver hook; `L1630`–`L1635`, `L4270`, `L4271`.
- **W1b — engine opt-in:** `Engine::builder().input_actions()`, an engine-owned doc-singleton kind
  registered like `patterns()`, install beside `install_nav`, `carry_resource::<Actions>()`, a snapshot
  field for `ActionState`, unknown names drained into `Diagnostics` as `L4270`, facade prelude, a
  runnable example.
- **W2 — rebinding:** capture, `kind: input-bindings` overrides, the effective table in the `.lrp` header
  and keyframes, `UiNav::from_actions`, a rebind-menu example. Enforces `locked: true`.
- **W3 — devices, glyphs and the full key set:** the whole keyboard appended after `Alt` plus `Back`/`Forward` (one corpus re-record), the window keymap and mouse mapping for them, recorded `gamepad_connected{family}` / `gamepad_disconnected`,
  `last_device` / `device_changed`, `Actions::source`, a glyph table (`glyph_name`), a focus event
  (Proposed).
- **W4 — rumble:** an outbox and host backends (gilrs on Windows; macOS GameController is decided there).

## Status

**W1a** (this wave): the sim-side action layer, bindings limited to today's keys. The existing key
and button names and order are pinned, and a serialized script of every one is checked against a literal
written before any change, so `replay_content_hash` stays put. `Actions` is tested at every gesture
boundary, trigger hysteresis, axes, every compile error with its pointer, a two-run `SimDriver`
comparison and a mid-hold state round trip. No pinned artifact moved.

## Proposed — owner ruling owed

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
- **Live `asset.reload` of the map** — trigger: tuning an action map in the editor.
- **Keys beyond today's set in bindings** — trigger: W3 (needs the re-record above).
- **Keyboard-layout labels** (what the key at a physical position says on this layout) — trigger: glyphs
  for non-QWERTY players; arrives with W3's glyph table.
- **Gyro** and **multiple pads** — triggers as in `Plan/phase-13-input.md`.
- **The editor's `keymap.ts` forwarding of the new keys** — a hand mirror; trigger: W3's keys landing.
