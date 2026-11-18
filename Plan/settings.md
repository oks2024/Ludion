# Plan: Player settings — a per-machine settings file, requests and a view

> **Wave 1 mechanized; Waves 2–3 not started.**
> Driven by Neon Shroud request 0022: a game's options menu needs volumes, window mode, quality and
> anti-aliasing that persist per machine, load leniently and never refuse a boot. Game-facing doc:
> `ludion docs settings`. Amends `Plan/audio-ambience.md` decision 6 (a runtime setter now exists).

## Design decisions

1. **One file:** `<save_root>/save/settings.yaml`, `kind: settings`, canonical `format: 1`. It loads
   leniently: a file that cannot be honored (bad YAML, bad envelope `L1700`, newer `format`) is backed
   up (`settings.yaml.bak.<hash>`) and replaced by defaults with one `L4340`; each unknown key is dropped
   (`L4341`); an unknown bus name is kept and re-emitted (`L4341`); a volume outside `0..=1` is clamped
   and a NaN or non-number becomes `1.0`; an unknown preset or AA name is ignored (`L4342`). The field
   pass runs on the JSON value before the typed deserialize, so one bad field never resets the file.
2. **Shared save-file shape** (the 0021 W2 peer): `<save_root>/save/<kind>.yaml`, lenient
   envelope open (`open_envelope`, shared with `open_profile`), `backup_file`, `sweep_temps`,
   `write_durable`, explicit save drained at the frame boundary and written only under a `persist_*`
   opt-in. 0021 W2's `input-bindings.yaml` reuses `open_envelope` and these helpers.
3. **Requests and view.** `Settings` is the write-only request resource, `SettingsView` the read-only
   resource. Both are installed at every world build; the drain at the frame boundary stores values on
   the Engine, so they outlive rebuilds and a restore does not roll them back. Host state: never hashed,
   snapshotted, recorded or on the wire.
4. **Precedence.** Quality: flag → env → file → packaged → full (a pinned quality persists a request but
   does not apply it). AA: game `RenderSettings` → `--aa` → file → builder. Window: flag → env → file →
   builder. Volumes: the file *is* the `User` mix layer.
5. **Requests persist only on `save()`**, and only under `EngineBuilder::persist_settings()`; without
   it the file is never read or written.
6. **`SettingsView` = settings-at-start + every request since**; it shows file values, never host flags,
   so it is deterministic.
7. **The boot audio seed is host delivery, not a log entry:** `set_audio_sender` sends one `User`
   `BusGain` per bus whose value is not `1.0`. Runtime requests are logged `BusGain{User}` messages,
   only when the value changes.
8. **YAML over TOML** (request question 2: the game's answer). **Explicit `save()`** (Q3). **The view
   exposes every field** (Q4). **Unknown bus names are kept and reported once** (Q6).
9. **Emit** writes every present value in schema order and omits unset optional fields.
10. **`RenderSettings` stays as it is** (a non-persisting runtime request); the file's AA slot sits
    below it and `--aa`.

## Waves

- **W1 (shipped):** `open_settings`/`open_envelope`, the `settings` kind row, `L1700` and
  `L4340`–`L4343`, `Settings`/`SettingsView`/`WindowMode`/`QualityPreset`, `persist_settings()`,
  `pin_quality`, the window sender and getter, the audio seed, tests, the `settings_menu` example,
  `docs settings`.
- **W2:** facade boot reads the file before window, renderer and sink; `pin_quality` callers; the window
  host consumer; settings-at-start in the generic `ReplayHeader.save_docs` slot (key `settings`, from 0021 W2b) and replay install; `describe` shows file against
  effective; `ludion new` ignores `save/`; desktop-window and quality docs.
- **W3:** text settings with 0024, rumble with 0021 W4 (binds `set_rumble_intensity`), brightness with 0019.

## Status

**W1:** the file, requests and view. Tests cover the lenient matrix (missing, malformed with backup,
unknown key, unknown bus kept, clamp, NaN, bad names), emit/load round trip, AA precedence over every
combination, apply and persist, off without opt-in, pinned quality, the audio seed, the window sender,
a `WorldHash` trace equal with and without requests and a rebuild keeping the view. No `WorldHash`,
trace, replay, golden or schema snapshot moved.

## Proposed — owner ruling owed

- The window chord writes `display.window` (reverses the Clockout 0023 ruling; W2 ships it
  non-persisting).
- Fold `RenderSettings::request_anti_aliasing` into `Settings` (breaking).
- A snapshot field so keyframes carry the view (moves `replay_content_hash`; the corpus is re-recorded).
- `mute_unfocused`.
- Inline quality override objects in the file.
- Amendments to `Plan/audio-ambience.md` decision 6, `Plan/scalability.md` / `docs/quality.md`
  precedence and `docs/desktop-window.md` "nothing persists".

## Explicitly out of scope (triggers recorded)

- Facade boot and the window host — W2. Text, rumble, brightness — W3.
