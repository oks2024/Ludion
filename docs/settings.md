---
description: Player settings — volumes, quality, AA, window, rumble, brightness, subtitles; the save file
---

# Player settings

**Scope:** an options menu that remembers the player's choices per machine. Volumes by bus are
`ludion docs audio-mix`; presets are `ludion docs quality`; AA is `ludion docs anti-aliasing`.

## The loop

1. Build with `Engine::builder().persist_settings()`; the file `<save_root>/save/settings.yaml` is read
   at build. Without it the file is never read or written and requests only apply to the session.
2. A menu system writes `world.resource_mut::<Settings>()`: `request_bus_volume(Bus, f32)`,
   `request_master_volume`, `request_quality(QualityPreset)`, `request_anti_aliasing` (beats `--aa` this session),
   `request_window_mode(WindowMode)`, `request_rumble`, `request_brightness`, `request_subtitles`,
   `request_subtitle_scale`, `request_language(&str)`, then `save()` (several calls in a frame are one write).
3. It reads `world.resource::<SettingsView>()` (`volume`, `quality`, `anti_aliasing`, `window_mode`, `live_window_mode` for the window as it is now,
   `rumble`, `brightness`, `subtitles`, `subtitle_scale`, `language`).
4. `cargo run -p ludion --example settings_menu` shows two boots on one scratch directory.

`ludion::run` and `ludion::launch` turn persistence on; a game built straight from `Engine::builder()`
calls `persist_settings()` itself.

## How a host reads it

- **Boot order.** The file is read at build, before the first renderer: quality and AA are in force,
  and the sink starts at the file's volumes (`set_audio_sender`).
- **Quality pin.** A `--quality` flag or `LUDION_QUALITY` applies and pins; the file's preset is
  otherwise kept over a packaged default, and a menu request applies only unpinned.
- **Window.** The window opens before the engine, so the file is peeked first: `--fullscreen`, then
  `LUDION_FULLSCREEN`, then `display.window`, then the builder. A `request_window_mode` always applies,
  even after a start flag. The Alt+Enter chord does not write the file: `window_mode()` stays the saved choice, and
  `live_window_mode()` follows the window (host state for display only, `None` headless; never branch
  hashed state on it). A request for the file's own mode still reaches a window the chord moved.
- **Replay.** A recording carries the settings in force when it was armed in
  `ReplayHeader.save_docs["settings"]`, only if they are not the defaults. A replay installs them into
  `SettingsView` and nothing else: no render state, no window, and it never reads or writes the file.
  A window request made by a replayed game updates the replay's view and never reaches the live window.
- **Describe.** `engine.describe` has `settings: {persisted, quality_pinned, values}`.

## Format

```yaml
format: 1
kind: settings
audio:
  master: 1.0          # 0..=1
  buses: {music: 0.6}  # sfx, ambient, voice, ui, music; a game's own names are kept
display:
  window: borderless_fullscreen   # windowed | borderless_fullscreen
  brightness: 0.25                # -1..=1, in EV
graphics:
  quality: medium                 # low | medium | high | full
  anti_aliasing: smaa             # off | smaa
input:
  rumble: 0.8                     # 0..=1
text:
  subtitles: true
  subtitle_scale: 1.25            # 0.75..=2
  language: fr                    # a tag
```

Unset optional fields are omitted. The writer is canonical (stable order, shortest floats).

## Where each applies

- **Rumble** is host state, `Engine::rumble_intensity` (default 1): it scales each later pad command,
  and 0 drops them. A command played in the same tick as the request keeps the old value. A host may
  also call `set_rumble_intensity`.
- **Brightness** is host render state, `Engine::brightness` (default 0): an EV offset added to the
  exposure compensation before the tonemap, in manual and metered exposure alike, headless and windowed.
  At 0 the frame is the one rendered without it.
- **Subtitles** apply at extract, from the view: `subtitles: false` draws no `SubtitleView`, and
  `subtitle_scale` multiplies its glyph `size` (not `padding` or `wrap_width`). The authored component
  is never written, and dialogue itself runs on.
- **Language** is stored and returned by `SettingsView::language`; it does not change the locale.
- A replay installs the header's values into the view only: `rumble_intensity` and `brightness` stay
  this machine's (the subtitle options follow the view). A replayed game's own requests apply, and an
  in-place playback restores both on return.

## Rules

- **Lenient load, never a refused boot.** A missing file is defaults, silently. A file that cannot be
  honored (bad YAML, bad envelope `L1700`, a newer `format`) is renamed `settings.yaml.bak.<hash>` and
  defaults apply with one `L4340`. An unknown key is dropped (`L4341`); an unknown bus name is kept and
  written back (`L4341`). A volume outside `0..=1` is clamped, a non-number becomes `1.0`, and an
  unknown quality or AA name is ignored (`L4342`). Rumble, brightness and subtitle scale clamp the same
  way, a non-number becoming `1.0`, `0.0` and `1.0`; a `subtitles` that is not a boolean, or a `language`
  that is not a non-empty trimmed string, is ignored (`L4342`). One bad field never resets the rest.
  Warnings are in `Engine::boot_warnings`.
- **Requests apply at the frame boundary.** Volumes go to the `User` mix layer (one `BusGain` per
  changed bus, logged); AA and quality apply at the next render; a window mode goes to the host channel
  `set_window_mode_sender`, once per change. NaN numbers are ignored; others are clamped.
  `request_language` trims the tag and ignores an empty one.
- **`SettingsView` is deterministic** — all but `live_window_mode()`: the file at start plus the sim's own requests, never a host flag.
  It lives on the engine, so a rebuild or a run-save resume does not roll it back. A snapshot carries the
  view when it is not the defaults, so `engine.restore` and a replay seek install the snapshot's view (never
  quality, anti-aliasing, the window or the file). Settings are never hashed or on the wire; a `WorldHash` trace is the same with or without requests.
- **Precedence.** Quality: host flag (`pin_quality`) → file or request → full. AA: `Settings` request →
  `--aa` → file → builder. Volumes: the file is the `User` layer.
- **Sink seed.** `set_audio_sender` sends one `User` `BusGain` per saved bus that is not `1.0`, unlogged.
- A failed write is `L4343`; the previous whole file stays.

## Gotchas

- `save()` before `persist_settings()` is a no-op, not an error.
- A `scene.load` keeps the settings; a `ludion dev` restart re-reads the file, so unsaved requests go.

## Next

Next for `language`: applying it to the locale is not wired, so a game that wants it reads the view and
sets the locale itself (`ludion docs localization`).

`ludion docs audio-mix`, `ludion docs quality`, `ludion docs anti-aliasing`, `ludion docs player-profiles`.
