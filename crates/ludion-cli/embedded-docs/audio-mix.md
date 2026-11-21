---
description: Audio buses, bus gain (settings sliders), ducking rules and bus pause (pause menu)
---

# Audio mix

**Scope:** change loudness or hold sound at runtime — a settings slider, a pause menu — through the
`AudioQueue`. The authored per-bus master (`category_gains`), loops and music are in `ludion docs audio`.

## The loop

```rust,ignore
fn settings(world: &mut World, _: &SystemCtx) {
    let queue = world.resource_mut::<AudioQueue>().expect("AudioQueue");
    // A music slider at 40%, settled over ~10 ms.
    queue.bus_gain(Bus::Music, MixLayer::User, 0.4, 0);
    // A pause menu: hold the world's sound, 6 ticks of fade, the interface stays live.
    queue.bus_pause(&[Bus::Sfx, Bus::Ambient, Bus::Music], 6);
    // ...and later:
    queue.bus_resume(&[Bus::Sfx, Bus::Ambient, Bus::Music], 6);
}
```

Observe: `engine.audio_log()` holds `AudioMessage::BusGain` / `BusPause`, stamped with the emitting frame.

## Format

| Item | Values |
| --- | --- |
| `Bus` | `Master`, `Sfx`, `Ambient`, `Voice`, `Ui`, `Music` |
| `AudioCategory` → bus | `sfx`, `ambient`, `voice`, `ui` (an `AudioEmitter`'s `category:`); music is its own bus |
| `MixLayer` | `User` (the player's setting), `Game` (scripted mixes) |
| duck layer | engine-owned, never a `MixLayer`: `DuckRule`, `duck_start`/`duck_end` (§Ducking) |

Effective gain of a bus is `authored × user × game × duck × pause`, and every bus but `Master` is also
multiplied by `Master`. Every bus has an authored field in `CategoryGains`, set with
`CategoryGains::default().with(Bus::Ambient, 0.6)` (`master sfx ambient voice ui music`, each `1.0` by default).

## Authored gains

`CategoryGains` is `#[non_exhaustive]`: start from `default()` and `.with(bus, gain)` each bus; `gain(bus)` reads one.
The sink multiplies in `master` for every bus, `voice` and `ui` included.

## Rules

- **Fades are in sim ticks**, like the music verbs. `0` still slews about 10 ms, so a change never clicks.
- **A non-finite gain queues `0.0`** (`L5005`); a negative one clamps to `0` in the sink.
- **A paused bus holds each voice's cursor**, loops and music included: resume continues from the same
  sample. A fading pause is audible until it reaches silence, then voices are skipped. An empty bus
  slice queues nothing.
- **A world rebuild** (a scene load) resets the `Game` layer and every pause to `1.0`
  and drops voices held on a paused bus; the `User` layer survives, so a player's volume holds across levels.
- **A reopened output device** (unplug, switch) keeps the current `User`/`Game` gains and pauses.
- **Defaults are bit-identical.** With no bus call the stream and the mix are unchanged.
- **Nothing here reaches `WorldHash`, snapshots or replays**; the log records what was asked for.

## Ducking

Dip the music and ambience while someone speaks, with no per-line code:

```rust,ignore
Engine::builder().audio_duck(DuckRule {
    when: Bus::Voice, duck: &[Bus::Music, Bus::Ambient],
    depth_db: -9.0, attack_ticks: 6, release_ticks: 30,   // DuckRule::default()
})
// and, for a sequence of your own:
queue.duck_start("briefing", &[Bus::Music], -12.0, 10);   // key, buses, depth_db, attack ticks
queue.duck_end("briefing", 45);                           // key, release ticks
```

Observe: `AudioMessage::Duck { bus, gain, fade_frames }` in `engine.audio_log()`, sent on each edge.

- **A `when` bus is live** while a plain one-shot on it plays (timed by its WAV's length, read only when a
  rule names that bus) or an attached voice (`play_on`) sounds. An attached voice counts only while audible:
  a guard muttering out of earshot ducks nothing.
- **Sources multiply**, rules and manual keys alike, floored at `DUCK_FLOOR_DB` (-24 dB). Two voices at once
  duck once; a bus no live source names is exactly `1.0`.
- **Edge fades**: the duck fades down over the `attack` of the source that turned on and back over the
  `release` of the one that turned off (the longest if several flip together).
- **A rebuild resets every duck** (a scene load, restore); rules persist. Starting a live key again
  replaces it; `duck_end` on an unknown key does nothing; an empty bus slice queues nothing.
- **`depth_db` never boosts**: above `0` clamps to `0`; NaN reads `0` (`L5005`).
- **A manual duck may name `Master`**, to duck a whole cutscene; a rule may not.
- **`L5100` refuses a bad rule at `build`**: `depth_db` not finite or above `0`; `when` of `Master` or `Music`
  (no tracked end); `duck` empty, or naming `Master` or `when` itself.
- No rule and no manual duck: the stream is unchanged. Nothing here reaches `WorldHash`, snapshots or replays.

## Gotchas

- Pause only the buses you mean: `Ui` stays live while `Sfx`/`Ambient`/`Music` hold, which is what a
  pause menu wants; pausing `Master` holds everything.
- The `User` layer's one writer is the player-settings drain (`Settings::request_bus_volume`, `ludion docs settings`); write `Game` yourself.
- The duck layer is engine-owned: duck through a rule or `duck_start`, never `bus_gain`. The `Game` layer
  stays the game's, and the `User` layer the player's: a slider is never lost to a duck.

## Next

- `ludion docs audio` — loops, emitters, music, authored category gains.
- `ludion docs audio-space` — the listener: `AudioListener` and its orientation.
