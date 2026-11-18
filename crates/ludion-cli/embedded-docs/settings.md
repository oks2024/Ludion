---
description: Player settings — volumes, quality, anti-aliasing, window mode; the request/view resources and the save file
---

# Player settings

**Scope:** an options menu that remembers the player's choices per machine. Volumes by bus are
`ludion docs audio-mix`; presets are `ludion docs quality`; AA is `ludion docs anti-aliasing`.

## The loop

1. Build with `Engine::builder().persist_settings()`; the file `<save_root>/save/settings.yaml` is read
   at build. Without it the file is never read or written and requests only apply to the session.
2. A menu system writes `world.resource_mut::<Settings>()`: `request_bus_volume(Bus, f32)`,
   `request_master_volume`, `request_quality(QualityPreset)`, `request_anti_aliasing`,
   `request_window_mode(WindowMode)`, then `save()` (several calls in a frame are one write).
3. It reads `world.resource::<SettingsView>()` (`volume`, `quality`, `anti_aliasing`, `window_mode`).
4. `cargo run -p ludion --example settings_menu` shows two boots on one scratch directory.

Persistence is opt-in and `ludion::run` does not read the file yet: a game that wants it calls
`persist_settings()` itself.

## Format

```yaml
format: 1
kind: settings
audio:
  master: 1.0          # 0..=1
  buses: {music: 0.6}  # sfx, ambient, voice, ui, music; a game's own names are kept
display:
  window: borderless_fullscreen   # windowed | borderless_fullscreen
graphics:
  quality: medium                 # low | medium | high | full
  anti_aliasing: smaa             # off | smaa
```

Unset optional fields are omitted. The writer is canonical (stable order, shortest floats).

## Rules

- **Lenient load, never a refused boot.** A missing file is defaults, silently. A file that cannot be
  honored (bad YAML, bad envelope `L1700`, a newer `format`) is renamed `settings.yaml.bak.<hash>` and
  defaults apply with one `L4340`. An unknown key is dropped (`L4341`); an unknown bus name is kept and
  written back (`L4341`). A volume outside `0..=1` is clamped, a non-number becomes `1.0`, and an
  unknown quality or AA name is ignored (`L4342`). One bad field never resets the rest. Warnings are
  in `Engine::boot_warnings`.
- **Requests apply at the frame boundary.** Volumes go to the `User` mix layer (one `BusGain` per
  changed bus, logged); AA and quality apply at the next render; a window mode goes to the host channel
  `set_window_mode_sender`, once per change. NaN volumes are ignored; others are clamped.
- **`SettingsView` is deterministic**: the file at start plus the sim's own requests, never a host flag.
  It lives on the engine, so a rebuild or `engine.restore` does not roll it back. Settings are never
  hashed, snapshotted or on the wire; a `WorldHash` trace is the same with or without requests.
- **Precedence.** Quality: host flag (`pin_quality`) → file or request → full. AA: `RenderSettings` →
  `--aa` → file or request → builder. Volumes: the file is the `User` layer.
- **Sink seed.** `set_audio_sender` sends one `User` `BusGain` per saved bus that is not `1.0`, unlogged.
- A failed write is `L4343`; the previous whole file stays.

## Gotchas

- `save()` before `persist_settings()` is a no-op, not an error.
- A `scene.load` keeps the settings; a `ludion dev` restart re-reads the file, so unsaved requests go.

## Next

`ludion docs audio-mix`, `ludion docs quality`, `ludion docs anti-aliasing`, `ludion docs player-profiles`.
