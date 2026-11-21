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
| zone layer | engine-owned, never a `MixLayer`: `AudioZone` `bus_gains` (`ludion docs audio-zones`) |

Effective gain of a bus is `authored × user × game × duck × zone × pause` (`zone` is the engine-owned layer of
`ludion docs audio-zones`), and every bus but `Master` is also
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
- **A world rebuild** (a scene load) resets the `Game` layer, the duck and zone layers and every pause to `1.0`
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

## Crossfade

`music_crossfade(id, &[(stem, gain)], ticks)` blends from the playing music to the set `id`: the new set
fades in over `ticks` while the live one fades out over the same ticks, as an equal-power pair (the two
gains satisfy `in² + out² = 1`, so the blend keeps its loudness). Plain `music_play`/`music_set` fade-ins and
`music_stop` fade-outs stay linear.

```rust,ignore
queue.music_crossfade("combat", &[("music/combat.ogg", 1.0)], 120);
```

- **Two slots, at most two live sets.** A third set mid-crossfade hard-cuts the outgoing one; the
  previous incoming set fades down from where it is, and the new one fades up.
- **Resend is free.** The live `id` again (not stopping) is a no-op. Crossing back to the outgoing
  set restarts it: it is cut, not resumed.
- **Nothing playing, or `ticks == 0`:** a plain `music_set` start (a fade-in, or a hard cut).
- **A set that cannot open** (`L5003`/`L5006`) keeps the old music playing and cuts nothing.
- **`music_stem_gain` addresses the incoming set.** `music_stop` and a world rebuild stop both
  slots; `music_set` and `music_play` hard-cut both and play one.
- A `music_play` track is a one-stem set, so it can be crossfaded from.
- Verify with `audio_messages`: the log carries a frame-stamped `MusicCrossfade`. The blend itself is
  a device-side effect, checked by ear.

## Gotchas

- Pause only the buses you mean: `Ui` stays live while `Sfx`/`Ambient`/`Music` hold, which is what a
  pause menu wants; pausing `Master` holds everything.
- The `User` layer's one writer is the player-settings drain (`Settings::request_bus_volume`, `ludion docs settings`); write `Game` yourself.
- The duck layer is engine-owned: duck through a rule or `duck_start`, never `bus_gain`. The `Game` layer
  stays the game's, and the `User` layer the player's: a slider is never lost to a duck.

## Next

- `ludion docs audio` — loops, emitters, music, authored category gains.
- `ludion docs audio-space` — the listener: `AudioListener` and its orientation.
- `ludion docs audio-zones` — `AudioZone`: beds, reverb and per-bus zone gains by listener position.
