---
description: Sound as frame-stamped events: the AudioQueue, WAV rules, and verifying the stream in text
---

# Audio

**Scope:** Emitting sound from a system and asserting on it without listening to anything. The frame-boundary drain it shares with saves and switches is player-profiles and scene-transitions.

Sound is **events as data**, never synthesis in your system. A system pushes an `AudioEvent` (a WAV
path + a linear gain) into the `AudioQueue` resource; the engine drains the queue at each frame
boundary, plays it through the host's sink, and records it in a frame-stamped log. Both `AudioQueue`
and `AudioEvent` are in the prelude.

```rust
use ludion::prelude::*;

/// Play a blip when the ball crosses the arena's right edge — a pure observation of the motion,
/// emitted once. `resource_mut::<AudioQueue>()` needs its own borrow, so collect the crossing in the
/// query and push after it ends.
fn ball_edge_sound(world: &mut World, _ctx: &SystemCtx) {
    let mut crossed = false;
    for (transform,) in world.query::<(&Transform,)>() {
        if transform.translation[0] >= 5.0 {
            crossed = true;
        }
    }
    if crossed {
        world
            .resource_mut::<AudioQueue>()
            .expect("the engine installs AudioQueue on every world")
            .play("sounds/blip.wav", 1.0); // path is relative to the scene's directory
    }
}
```

- **Format.** WAV for effects (16-bit PCM, mono or stereo, plain or `WAVE_FORMAT_EXTENSIBLE` with a PCM
  subformat; a bad file is `L1104`), Ogg Vorbis for music. A stereo WAV stays stereo on a stereo device
  (the mean of both channels on a mono one); effects and music resample linearly to the device rate. A
  sound rewritten under a running host is picked up by its first message half a second later (a static
  emitter: at its next sync). Keep clips small; convert other formats first.
- **Paths** resolve against the content root (the scene's directory), exactly like `Sprite.texture`.
- **No device is fine.** On a headless/CI machine the sink warns once (`L5001`) and plays silent; the
  events still route, so nothing blocks and tests stay green. It keeps retrying the open, so a
  device that appears later is picked up with the declared loops and music, and it still warns
  (`L1101`/`L1104`/`L5003`) once per broken file, so a typo shows on a headless run. Audio never
  gates determinism. A device lost mid-run (`L5002`: unplugged, switched or stalled) reopens on the
  current default, retrying with backoff up to every 5 s; loops and music fade back in, and one-shots
  fired meanwhile are dropped. A burst of one-shots in one tick beyond half the sink's queue sheds
  the excess one-shots and never touches the stream. One warning when it goes, one note when it
  returns; nothing else moves.
- **A non-finite gain plays at 0.** A NaN or infinite gain (a `hp / max_hp` with `max_hp` 0) is
  queued as `0.0` with one `L5005` warning per sound, so it never poisons the mix.
- **Verify it in text, no goldens.** The stream is a pure function of the sim — a replay regenerates
  it exactly, and the `WorldHash` is unaffected (the queue is a resource). Assert on the frame-stamped
  log in a headless test:

```rust,ignore
use ludion_testkit::audio::audio_stream;

// After scripting input and stepping the engine:
assert_eq!(audio_stream(&engine), vec![(20, "sounds/blip.wav".to_string())]);
```

Catch a broken path in a test with `SoundTap`: it sees every message, where the log keeps only the
latest, and asserts each named sound and track would decode.

```rust,ignore
let tap = ludion_testkit::audio::SoundTap::install(&mut engine); // right after the scene loads
// ... drive the game ...
let sounds = tap.assert_sounds_decode(&content_root); // panics naming each file that would not play
assert!(sounds.contains("sounds/blip.wav"));
```

## Loops

A sound that must *keep* sounding — a drone, wind, a tolling bell — is a loop, not a repeated
`play()` (re-firing a one-shot restarts the sample from the top: stutter, not a bed). Loops are keyed
by their sound path, so one path is one bed however many systems ask for it.

```rust,ignore
let queue = world.resource_mut::<AudioQueue>().expect("installed on every world");
queue.loop_start("sounds/wind.wav", 0.4);   // start it, or update it if it is already playing
queue.loop_set_gain("sounds/wind.wav", 0.7); // the same message, spelled for an update site
queue.loop_stop("sounds/wind.wav");          // fades out over 10 ms
```

- **`loop_start` is idempotent**, so "recompute a gain and resend it every tick" is the intended
  calling convention: the running voice slews to the new gain over ~10 ms in place, without
  restarting.
- **A loop plays until you stop it.** There is no liveness diff — a forgotten `loop_stop` sounds for
  the rest of the scene.
- **A world rebuild stops every loop.** A `scene.load`, a district switch, and a restore each emit a
  blanket stop (each loop fades out over 10 ms), so a bed never crosses into a scene that did not
  ask for it.
- **Verify it the same way**: `audio_stream` lists every frame a sound is asked to sound, loop syncs
  included. When a test has to tell a start from a stop, `ludion_testkit::audio::audio_messages`
  gives the unflattened `(frame, AudioMessage)` stream.

## Positional emitters

An `AudioEmitter` is a loop *placed in the world*: audible within `range` of the listener, louder the
nearer it is. It is the sound analog of `PointLight` — the position is the entity's `Transform`, and
**no system is needed**. Put it in the scene and the engine keeps it in sync.

```yaml
- name: bell-tower
  components:
    Transform:
      translation:
        - 12.0
        - 0.0
        - -30.0
    AudioEmitter:
      sound: "sounds/bell.wav"
      gain: 0.8      # gain standing on top of it, before falloff (default 1.0)
      range: 40.0    # falls silent at this distance (default 10.0; must be > 0 — L1136)
      category: ambient
```

- **Falloff** is `gain × clamp(1 − distance/range, 0, 1)`, with distance measured on the **ground
  plane** (`x`, `z`) only — height above the listener does not attenuate. `range` is a hard cutoff:
  exactly `0` at `range` and beyond.
- **The listener is the camera** — specifically the lowest-`EntityId` entity carrying one. There is no
  separate listener component. A scene with no camera syncs nothing that tick.
- **Going inaudible is a stop.** Despawn the entity, remove the component, mute it, or walk out of
  `range`, and the loop stops on that tick. This liveness diff is what the manual `loop_start` path
  does not have, so an emitter needs no cleanup discipline at all.
- **Syncs are sent on change.** A `LoopSync` goes out when the emitter becomes audible, then again only
  when its gain moves past 1% of the last one sent or its sound or bus changes; the sink holds the loop
  between. One gain scales every channel — there is no pan.
- **Below audibility, nothing is sent**, so distant emitters cost nothing on the wire.
- **`sound` is checked.** `ludion check-scene`, a game's `--check-scene` and the wire's `scene.check`
  decode it as the sink does: a missing or empty path is `L1101`, a file that is not a supported WAV
  `L1104`. A prefab's emitters are checked where a scene places it.
- **It never touches the sim.** The engine only reads the component; the `WorldHash` is untouched either way.

Verify it the same way as everything else — `audio_messages` carries a `LoopSync` per audible
emitter whenever it changes, keyed by `LoopId::Entity(id)`, so a test can assert a gain rising on
approach.

## Positional one-shots

`AudioQueue::play_at` places a one-shot in the world: the same falloff and listener as an
`AudioEmitter`, resolved on the frame it plays.

```rust,ignore
queue.play_at("sounds/impact.wav", 1.0, impact_position, 12.0); // sound, gain, [x, y, z], range
```

- **The logged gain is the attenuated one.** Once resolved it is an ordinary one-shot, so
  `audio_messages` shows a `OneShot` at `gain × falloff`, indistinguishable from a `play()` at that gain.
- **Out of range, below audibility, or with no camera, nothing is sent.** A dropped one-shot is not
  retried on a later frame.
- **`push_at(event, position, range)`** places an ambient one-shot, as `push` does for a global one.

## Music

`AudioQueue::music_play` streams an Ogg Vorbis track under the content root as the one music voice.

```rust,ignore
queue.music_play("music/theme.ogg", 0.8, 60); // track, gain, fade-in in ticks; loops gaplessly
queue.music_play_once("music/sting.ogg", 1.0, 0);
queue.music_stop(90); // fade out over 90 ticks, then idle
```

- **Streamed, not decoded up front.** The sink decodes a little ahead of the device from disk, so a
  long track costs no start-up stall and holds only a short ring (~0.7 s) in memory; a loop's seam is gapless (the encoder's tail padding is trimmed at the stream's last granule), a play-once track's tail always plays, a stereo track on a mono device is the mean of both channels, and a track re-exported in place at another rate or channel count is converted for its new format at the next wrap.
- **One voice.** The track already playing (and not stopping) only takes the new gain, so resending
  `music_play` every tick is harmless; a new gain slews in over 10 ms. Any other track hard-cuts the current one and fades in; there
  is no crossfade.
- **Fades are in sim ticks**, converted with the engine's tick rate; `0` is instant.
- **A world rebuild stops the music** with a 10 ms fade-out (`scene.load`, a district switch, a
  restore), so the game's own `music_play` after the rebuild restarts it.
- **A bad track is `L5003`** (missing, not Ogg Vorbis, Opus-in-Ogg, a sample rate below 8000 Hz, or a
  decode error): a warning, once per file version, and nothing plays. Encode with
  `ffmpeg -i in.wav -c:a libvorbis -q:a 5 out.ogg`.
- **Verify it in text.** `audio_messages` shows `MusicPlay { track, gain, looping, fade_frames }` and
  `MusicStop { fade_frames }` stamped with the frame, and `audio_stream` lists each play's track.
  Its bus is the `music` field of `CategoryGains`.

## Category gains

Every message carries an `AudioCategory`: a `play()` or `play_at()` one-shot is `sfx`, while loops (`loop_start`)
and `AudioEmitter`s are `ambient` by default. The category is a *tag* — the sink reads it, no system
does.

Music has its own bus, the `music` field, with no category tag: only `music_play` reaches it.

The **game** names a per-bus master gain on its builder, and the sink multiplies each voice by its
own bus's. `CategoryGains` is in the prelude:

```rust,ignore
fn game() -> EngineBuilder {
    Engine::builder()
        .system("hit_sounds", hit_sounds)
        // "Ambience sits under the effects." — one authored mixdown, not a per-run flag.
        .category_gains(CategoryGains { sfx: 1.0, ambient: 0.6, music: 0.8 })
        .default_scene("scenes/plaza.yaml")
}
```

- **Every host honours it:** `ludion::run`, `ludion::launch` and `ludion serve` read it off the built engine.
- **The default is the identity** (`1.0` on every bus): the sink plays exactly the gains its systems
  authored.
- **It is a mixdown concern, not gameplay.** The gains live in the sink, never in the world — the same
  category of thing as which output device to open. No value here moves the `WorldHash`, a replay, or
  the audio log: the log records what was *asked for*, at the authored gain.
- **There is no runtime setter.** The mixdown is authored once, at build time; a player-facing volume slider is a separate surface.
- **The mixer soft-limits, it does not clip.** Voices that sum past `0.8` are bent onto full scale by
  a smooth knee rather than flat-topped, so a staged kill firing four distinct sounds at once stays
  clean.
- **Voices are bounded.** At most 4 one-shots of one sound and 64 voices in all sound at once, loops
  included. Past either cap, the oldest one-shot fades out over 10 ms. Loops count but are never
  stolen: a loop past 64 loops waits, and starts, fading in, when a loop stops. Copies of one sound
  started in the same callback are one voice. A game overrides both caps in `ludion.toml`'s
  `[audio]` table (`voices_per_sound`, `voices_total`, integers from 1 to 256, per-sound no larger
  than total; a bad value is `L5004` and the game refuses to boot).

## Next

- `ludion docs systems` — the system contract and the determinism rules.
- `ludion docs verify-loop` — the `cargo test` verification loop.
- `ludion docs running` — hosting a scene and observing it.
