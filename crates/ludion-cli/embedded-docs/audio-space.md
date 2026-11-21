---
description: Where the ear is — AudioListener, a listener separate from the camera, and its orientation
---

# Audio space

**Scope:** choose where positional sound is measured from when that is not the camera — a first-person
head, a third-person character, a listener parented to a mover. Emitters, `play_at` and pan are in
`ludion docs audio`; loudness is in `ludion docs audio-mix`.

## The loop

1. Put `AudioListener` on an entity, typically a child of the player at head height.

```yaml
- name: player-ear
  components:
    Parent: { index: 1, generation: 0 }   # the player: the second entity in the scene
    Transform: { translation: [0.0, 1.6, 0.0] }   # head height, local to the parent
    AudioListener: {}                     # orientation: camera (default) | self
```

2. Run. Every `AudioEmitter` and `play_at` / `push_at` now measures distance from that entity's world position.
3. Observe: `engine.audio_log()` — the `LoopSync` and `OneShot` gains are the ones measured from the listener.
4. Check: `ludion check-scene` reports a second listener (`L1650`).

Runnable: the `AudioListener` doctest and `crates/ludion-engine/tests/audio.rs`.

## Format

| Field | Values | Meaning |
| --- | --- | --- |
| `orientation` | `camera` (default) | stereo left/right follow the lowest-`EntityId` `Camera`, so a sound pans as the screen shows it |
| | `self` | stereo left/right follow this entity's own world rotation (`+X` right, `+Y` up) |

## Rules

- **No `AudioListener` means the camera listens**, exactly as before: the stream is byte-identical.
- **The lowest `EntityId` wins.** A second carrier is `L1650` (a warning): once at runtime, and in `check-scene` at
  `/entities/<i>/components/AudioListener`.
- **Distance always comes from the listener's world position**, composed through `Parent`; it
  need not be a body, a camera or have a `Transform` (no `Transform` listens from the origin).
- **With no camera in the world** the axes come from the listener itself, whatever `orientation` says — a
  listener alone is enough for emitters to sync.
- **Nothing here reaches `WorldHash`, snapshots or replays**; the component is read by the audio path only.

## Attached voices

`AudioQueue::play_on` plays a one-shot that follows an entity while it plays.

```rust,ignore
let id = queue.play_on(guard, "voice/chatter.wav", 0.8, 18.0, AudioCategory::Voice); // entity, sound, gain, range, bus
queue.voice_stop(id, 6); // optional: fade it out over 6 ticks
```

- **It follows the entity.** Each tick the engine re-resolves the voice's gain and pan against the listener (the
  `play_at` falloff) and sends a `VoiceSync` only when they move past the emitter's 1% / 0.02 steps.
- **It starts even when inaudible**, at gain `0.0`: a line begun out of earshot is heard when its speaker walks in.
  A voice on an already-dead entity sends nothing.
- **It ends with the clip.** The engine reads the WAV's length at the frame boundary and stops following after
  `ceil(frames x tick_hz / sample_rate)` ticks; `audio_messages` carries `VoiceStart`, `VoiceSync`, `VoiceStop`.
- **An audible attached voice counts for ducking** (`DuckRule`, `ludion docs audio-mix`); one out of earshot does not.
- **A despawned entity stops it** with the sink's ~10 ms fade; `voice_stop` on an ended or unknown id is ignored.
- **Ids count up from `0` per queue** and restart when the world rebuilds (`scene.load`, restore).
- A device reopen drops attached voices, as it drops one-shots; only loops come back.
- **`push_voice(event)` is the global form:** a stoppable one-shot with no entity and no falloff. It starts at the
  event's own gain and pan, ends with its clip, counts for ducking, and is never resynced; `voice_stop` fades it.
  A radio or UI line is one (`ludion docs dialogue`).
- **`push_voice_from(event, offset_ticks)` and `push_on_from(entity, event, range, offset_ticks)`** start the voice that many
  ticks into the clip, for resuming a line already on air; the engine tracks only the remainder. An offset at or past the
  clip sends nothing (the id is still returned, a stop is ignored).
- **A held bus holds the voice.** While `bus_pause` holds a voice's bus (or the master) its tracked end moves out one tick
  per held tick, so `voice_stop` after the resume still reaches it, and a held voice does not keep a `DuckRule` live.

## Filters

A voice carries `spatial: VoiceSpatial { filter: VoiceFilter { low_pass_hz, high_pass_hz }, send }`; the default is
bypass. `AudioQueue::push_on` is `play_on` for a whole `AudioEvent`, so a voice can start filtered:

```rust,ignore
let radio = AudioEvent { category: AudioCategory::Voice,
    spatial: VoiceSpatial { filter: VoiceFilter::band_pass(300.0, 3400.0), send: 0.0 }, ..AudioEvent::new("voice/radio.wav", 1.0) };
let id = queue.push_on(guard, radio, 18.0); // entity, event, range
```

- **Bypass is not run.** A low-pass at `20000` Hz or more, a high-pass at `20` Hz or less (or a NaN cutoff) is off: the
  sink keeps no filter for that side, so the voice is bit-identical to an unfiltered one.
- **One biquad per side**, per voice, ahead of gain; a cutoff change sweeps in log frequency over the ~10 ms ramp.
  Channels past the eighth pass unfiltered.
- **The filter rides every message:** `OneShot`, `VoiceStart`, `VoiceSync` and `LoopSync` carry `spatial`; a
  resent `LoopSync` or `VoiceSync` retargets the running voice and keeps its cursor.
- **`send` is carried, not mixed:** there is no reverb bus yet.

## Occlusion

Off unless the game opts in: `Engine::builder().audio_occlusion(AudioOcclusion::default())`. Every **positional**
source (an `AudioEmitter` loop, `play_at`/`push_at`, `play_on`/`push_on`) is then muffled and quieted while static
geometry lies between it and the listener. Global sounds (`play`, `loop_start`, music) are never occluded. With no
builder call the message stream is byte-identical to today.

| Field | Default | Meaning |
| --- | --- | --- |
| `filter` | `QueryFilter::new().only_fixed()` | which colliders occlude; `include_layer`/`ignore_layer` work |
| `rays` | `3` | `1`, `3` or `5` rays per source |
| `spread_m` | `0.6` | side-ray offset at the source |
| `per_hit_db` | `-6.0` | loss per distinct blocker on one ray |
| `max_db` | `-18.0` | floor of one ray's loss |
| `occluded_low_pass_hz` | `1200.0` | cutoff when every ray is blocked |
| `interval_ticks` | `2` | ticks between re-casts of one source |
| `max_rays_per_tick` | `96` | ray budget per tick, shared round-robin |
| `smoothing_ticks` | `8` | ticks a changed result takes to apply |

- **The mapping.** Rays start at the listener; the centre ray ends at the source, `3` adds ends `spread_m` to either
  side (horizontal), `5` also above and below. A ray's loss is `blockers x per_hit_db`, floored at `max_db`. The
  source's gain is the mean of its rays' losses; its cutoff slides in log frequency from open (20 kHz) toward
  `occluded_low_pass_hz` by the share of blocked rays, and no blocked ray is exactly bypass. One wall across all three
  rays is `per_hit_db` and the occluded cutoff; one clear side ray gives partial values. An authored `spatial` low-pass
  stays: the lower cutoff wins.
- **Self-exclusion.** A hit on the source's or the listener's own hierarchy (same `Parent` root) is ignored, so a
  guard's capsule or the player's body never occludes.
- **Cost.** At most `max_rays_per_tick` rays a tick; due sources are cast round-robin and an uncast one keeps its old
  result. A one-shot or voice starting this tick is cast at once and its first result is applied unsmoothed (a voice
  starting behind a wall starts muffled); later changes slew over `smoothing_ticks`. Queries read the last completed
  physics step.
- **Errors.** `L5101` refuses a bad config at `build` (`rays` not 1/3/5, a positive or non-finite loss, a cutoff outside
  20-20000 Hz, `interval_ticks` 0, `max_rays_per_tick` below `rays`); `L5102` is a warning, once, when the physics query
  refused the filter (an undeclared layer): sources then play unoccluded.

## Gotchas

- A WAV that is not 16-bit PCM never decodes: its voice still starts (the sink reports it) but is never followed
  or ended by length.
- To pan by where the player's body faces rather than where the camera looks, use `orientation: self`.
- There is no blend between player and camera: parent the listener where the ear should be.
- The listener moves with its entity every tick; an emitter re-syncs only when its gain or pan moves past
  the usual steps (`ludion docs audio`).

## Next

- `ludion docs audio` — emitters, `play_at`, stereo pan.
- `ludion docs audio-mix` — buses, runtime gain, pause.
- `ludion docs audio-zones` — `AudioZone`: ambient beds, reverb and bus gains by where the listener stands.
- `ludion docs hierarchy` — `Parent` and the composed world pose.
