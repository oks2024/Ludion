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

## Gotchas

- To pan by where the player's body faces rather than where the camera looks, use `orientation: self`.
- There is no blend between player and camera: parent the listener where the ear should be.
- The listener moves with its entity every tick; an emitter re-syncs only when its gain or pan moves past
  the usual steps (`ludion docs audio`).

## Next

- `ludion docs audio` — emitters, `play_at`, stereo pan.
- `ludion docs audio-mix` — buses, runtime gain, pause.
- `ludion docs hierarchy` — `Parent` and the composed world pose.
