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
   does not apply it). AA: game `Settings` request → `--aa` → file → builder. Window: flag → env → file →
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
10. **`RenderSettings` is folded into `Settings`** (R13, owner ruling 2026-11-19): `Settings::request_anti_aliasing`
    is the game's one runtime request. It beats `--aa` for the session, and `save()` persists it to the
    file, which sits below `--aa` at the next boot. There is no clear.
11. **The header carries the settings in force at arm, only when non-default** (`save_docs["settings"]`,
    outside `replay_content_hash`, so a default recording's header is byte-identical). A replay installs
    the view only (`install_settings_from_text`): never render state, never the window, never disk.
    Replay and in-place playback build with `profile_off_disk`, which also gates the file read and the
    write, and the window request is not sent on the host channel (the view still updates, so it
    matches the recording). A playback return restores the settings, the AA slot and, unless pinned, the quality.
12. **Quality pin rule:** `Engine::apply_startup_quality(setting, named)`. A quality a flag or
    `LUDION_QUALITY` named (`startup_quality_named`) applies and pins; otherwise the file's preset,
    already applied at build, stays; only without one does the resolved (packaged or full) setting apply.
13. **The window is peeked before the engine** (`settings_window_mode_at`: no backup, no warning; the
    build reports once). Start: flag, then env, then file, then builder. A runtime request always
    applies, through `HostEvent::WindowMode`. The chord stays non-persisting; the view reports the live mode
    through its own host-state accessor, `SettingsView::live_window_mode()`, fed by a host-to-engine channel
    (never a `Command`), and a request is sent when it differs from the live mode, else from the file's.
14. **`engine.describe` gains a `settings` section:** `persisted`, `quality_pinned`, `values`.
    The existing `quality` and `anti_aliasing` keys already report the effective values.

`ludion new` already ignores the save directory (`/scenes/save/` in the gitignore template).

## Waves

- **W1 (shipped):** `open_settings`/`open_envelope`, the `settings` kind row, `L1700` and
  `L4340`–`L4343`, `Settings`/`SettingsView`/`WindowMode`/`QualityPreset`, `persist_settings()`,
  `pin_quality`, the window sender and getter, the audio seed, tests, the `settings_menu` example,
  `docs settings`.
- **W2 (shipped):** facade boot reads the file before window, renderer and sink; `apply_startup_quality`;
  the window host consumer; settings-at-start in the generic `ReplayHeader.save_docs` slot (key
  `settings`, from 0021 W2b) and replay install; `describe` settings section; desktop-window and
  quality docs.
- **W3:** text settings with 0024, rumble with 0021 W4 (binds `set_rumble_intensity`), brightness with 0019.
- **Live window mode (shipped, R13):** the settings view reports the live window mode, so it
  never disagrees with the window after a chord (decision 13); the chord stays non-persisting.

## Status

**W1:** the file, requests and view. Tests cover the lenient matrix (missing, malformed with backup,
unknown key, unknown bus kept, clamp, NaN, bad names), emit/load round trip, AA precedence over every
combination, apply and persist, off without opt-in, pinned quality, the audio seed, the window sender,
a `WorldHash` trace equal with and without requests and a rebuild keeping the view. No `WorldHash`,
trace, replay, golden or schema snapshot moved.

**W2:** `ludion::run` and `ludion::launch` call `persist_settings()` and read the file at build:
quality and AA are in force before the first renderer, the window opens in the file's mode (peeked
before the engine exists) and the sink seeds from `set_audio_sender`. Tests: parse_text and peek
units (no backup), `startup_quality_named`, `apply_startup_quality` over every named/file/resolved
combination, the header-wins `WorldHash` trace on three roots, a replay that never reads or writes the
file (a malformed one stays untouched, no `L4340`), a default header byte-identical, a recording armed
after a request, the describe section, in-place playback returning the pre-play view and render state,
and the facade boot test. Live smoke is owed to a human: boot borderless from the file on Windows and
macOS, and a menu request toggles the window. No `replay_content_hash`, `WorldHash`, replay, golden or
protocol schema moved.

**R13:** `RenderSettings` is folded into `Settings` (decisions 4, 10), and `SettingsView::live_window_mode()` follows the window
(decision 13). The window host reports its mode over a channel on start, after a chord, a request and a resize. No
`replay_content_hash`, `WorldHash`, replay, golden or schema snapshot moved. Live smoke owed to a human, with W2's.

## Owner rulings (2026-11-19)

- Shipped (R13). The chord stays temporary, keeping the Clockout 0023 ruling; the settings view reports the live window mode instead (Waves). The window chord writes `display.window` (reverses the Clockout 0023 ruling; W2 ships it
  non-persisting).
- Shipped (R13). Fold `RenderSettings::request_anti_aliasing` into `Settings` (breaking).
- Accepted, scheduled (R14 re-record round). A snapshot field so keyframes carry the view (moves `replay_content_hash`; the corpus is re-recorded).
- Accepted. `mute_unfocused`.
- Accepted. Inline quality override objects in the file.
- Accepted. Amendments to `Plan/audio-ambience.md` decision 6, `Plan/scalability.md` / `docs/quality.md`
  precedence and `docs/desktop-window.md` "nothing persists".

## Explicitly out of scope (triggers recorded)

- Text, rumble, brightness — W3.
