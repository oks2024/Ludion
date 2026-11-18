---
description: Audio buses, runtime bus gain (settings sliders, ducking) and bus pause (pause menu)
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
| `MixLayer` | `User` (the player's setting), `Game` (ducking, scripted mixes) |

Effective gain of a bus is `authored × user × game × pause`, and every bus but `Master` is also
multiplied by `Master`. `voice`, `ui` and `Master` have no authored field: they read `1.0`.

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

## Gotchas

- Pause only the buses you mean: `Ui` stays live while `Sfx`/`Ambient`/`Music` hold, which is what a
  pause menu wants; pausing `Master` holds everything.
- The `User` layer's one writer is the player-settings drain (`Settings::request_bus_volume`, `ludion docs settings`); write `Game` yourself.
- Ducking is a `Game`-layer gain; never write the `User` layer for it or the player's slider is lost
  on the next rebuild.

## Next

- `ludion docs audio` — loops, emitters, music, authored category gains.
- `ludion docs audio-space` — the listener: `AudioListener` and its orientation.
