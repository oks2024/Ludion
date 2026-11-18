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
- `ludion docs hierarchy` — `Parent` and the composed world pose.
