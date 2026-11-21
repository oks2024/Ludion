---
description: AudioZone — ambient beds, reverb and bus gains by where the listener stands
---

# Audio zones

**Scope:** make a place sound like itself — a yard's rain, a dock's machine hum and hall reverb — and
crossfade between places as the listener walks. The listener is in `ludion docs audio-space`; loudness
layers are in `ludion docs audio-mix`.

## The loop

1. Add an `AudioZone` to an entity with a `Transform` (the volume sits in its world pose; scale is ignored).

```yaml
- name: dock
  components:
    Transform: { translation: [30.0, 0.0, 0.0] }
    AudioZone:
      shape:
        box:
          half_extents: [20.0, 5.0, 10.0]   # or: sphere: { radius: 8.0 }
      priority: 10        # default 0; higher wins, ties to the lowest EntityId
      blend_m: 3.0        # default 0 (hard edge): weight rises 0 -> 1 over this distance inside
      reverb:             # optional; absent = dry
        preset: hall_medium   # default room_small
        wet: 0.25             # decay_s, pre_delay_ms, damping, size, wet override the preset
      beds:               # optional, at most 8; ambient bus, non-positional
        - sound: amb/machine_hum.wav
          gain: 0.5       # default 1.0
      bus_gains:          # optional: sfx, ambient, voice, ui, music
        ambient: 0.9
```

2. Run. At every frame boundary the engine resolves the zones from the listener point and sends the log
   `LoopSync`/`LoopStop` under `LoopId::ZoneBed(zone, i)`, `ZoneGain { bus, gain }` and `ReverbSet { params }`.
3. Observe: `engine.audio_log()`.
4. Check: `ludion check-scene` reports a bad zone (`L1651`, pointer into the field) and a missing or non-WAV bed
   sound (`L1101`/`L1104` at `/entities/<i>/components/AudioZone/beds/<j>/sound`).

Runnable: the `AudioZone` doctest and `crates/ludion-engine/tests/audio_zones.rs`.

## Resolution

- The listener point is `AudioListener`'s world position (or the camera's). With no listener the step does nothing.
- A zone's **depth** is how far inside the volume the point is (a sphere's `radius - distance`, a box's distance to
  its nearest face). Its **weight** is `0` outside, `1` inside with `blend_m: 0`, else `clamp(depth / blend_m, 0, 1)`.
- Zones holding the listener rank by `priority` (descending), then `EntityId` (ascending). The first, `a`, keeps its
  weight; the second, `b`, takes what `a` leaves, `(1 - w_a) x w_b`; **every other zone is silent**. A weight-1 winner
  masks everything beneath.
- **Beds** play at `bed gain x weight` and stop when the weight reaches `0` or the zone is despawned.
- **Bus gains** move each bus from `1.0` toward the zone's gain by its weight, and land exactly on `1.0` outside every zone.
- **Reverb** blends the zones that carry one: `wet` is the weighted sum, the rest the weighted mean. Outside every
  zone it is exactly dry (`ReverbParams::DRY`).
- No zone in the world sends nothing: the stream is byte-identical to a world without the component.

## Presets

Starting values, tuned by ear later; explicit fields override them.

| preset | `decay_s` | `pre_delay_ms` | `damping` | `size` | `wet` |
| --- | --- | --- | --- | --- | --- |
| `outdoor_open` | 0.4 | 0 | 0.7 | 0.3 | 0.05 |
| `room_small` (default) | 0.6 | 5 | 0.5 | 0.3 | 0.2 |
| `hall_medium` | 1.8 | 20 | 0.4 | 0.6 | 0.25 |
| `hangar_large` | 3.5 | 40 | 0.3 | 0.9 | 0.3 |

Ranges: `decay_s` 0.1 to 20, `pre_delay_ms` 0 to 200, `damping`, `size`, `wet` 0 to 1.

## Rules

- **Beds are not positional.** For a sound that comes from a place, author an `AudioEmitter` (a door, a machine).
- **`L1651`** rejects, with a pointer: a non-finite or non-positive extent or radius, a negative or non-finite
  `blend_m`, more than 8 beds (`/beds/8`), a negative or non-finite bed or bus gain, a reverb field out of range.
- **A zone spawned by code is sanitized, not reported**: a bad shape never holds the listener, beds past 8 are
  ignored, reverb fields are clamped, a bad bus gain is unity.
- **Nothing here reaches `WorldHash`, snapshots or replays** beyond the component itself; resolution runs at the
  frame boundary on the sim thread alone, so it is identical at any thread count.

## Gotchas

- **The sink hears all of it.** Beds play; `ZoneGain` multiplies the bus (a sink-private layer: `authored x user x game x
  duck x zone x pause`); `ReverbSet` drives one algorithmic stereo reverb on a send bus. A voice's send is
  `clamp(bus default + spatial.send, 0, 1)`, taken **after** its fader, filter and pan, so a ducked or paused bus sends
  less. Defaults: `sfx`, `ambient` and `voice` send `1.0`; `ui` and `music` send `0.0` (music stays dry). A scene load
  returns the reverb to dry and the zone layer to `1.0`. A world with no wet reverb mixes bit-identically to before.
- Priority decides who is heard, weight decides how much: two overlapping zones of equal priority never blend — the
  lowest `EntityId` wins whole.

## Asserting on the mix

`ludion_testkit::audio::audio_mix_at(&engine, frame)` folds the audio log up to `frame` into a `MixState`: `bus(Bus)`
(`user`, `game`, `duck`, `zone`, `paused`), `reverb`, `loops`, `voices` and `bed(zone, i)`. It records **targets**, not
mid-fade levels. Limits: a voice's natural end is not in the stream, so it stays until `VoiceStop` or `StopAllLoops`;
the log keeps its last 4096 entries, so a long drive folds a `SoundTap`'s messages with `mix_state(&messages, frame)`.

```rust
let mix = ludion_testkit::audio::audio_mix_at(&engine, frame);
assert!(mix.bed(dock, 0).is_some_and(|bed| bed.gain >= 0.495));
assert!(mix.reverb.near(&hall_medium));
assert!((mix.bus(Bus::Ambient).zone - 0.9).abs() < 0.01);
```

Runnable: `crates/ludion-testkit/tests/audio_mix.rs`, and the gallery scene `examples/gallery/audio-zones.yaml`.

## Next

- `ludion docs audio-space` — the listener and per-voice filters.
- `ludion docs audio-mix` — buses, runtime gain, ducking.
- `ludion docs audio` — emitters, loops, music.
