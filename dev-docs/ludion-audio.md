---
description: The cpal playback sink — engine AudioMessages become sound, out-of-sim and outside determinism
---
# ludion-audio

**Scope:** The out-of-sim audio playback sink: a thread over `cpal` that turns the engine's
[`AudioMessage`]s into sound on the default output device. This is the *one* crate that ever touches
an audio device — the sim side (the `AudioQueue` resource and the frame-stamped `audio_log`) lives
in `ludion-engine`/`ludion-core`; this doc covers only the sink itself: device handling,
decoding/caching, the mixer, and the silent-without-a-device contract. It does not cover WAV
decoding (`ludion-assets` owns `load_wav`) or how a host wires the channel up (the facade's
`wire_audio` in `crates/ludion/src/host.rs` calls `Engine::set_audio_sender` + `AudioSink::spawn`).

## Layout
- `crates/ludion-audio/src/lib.rs` — `AudioSink` (spawn/wait) and its idle pool of playback threads,
  the session body (`run`), `Output`/`CpalOutput` (the device seam), `Playback` (stream health,
  reopen) and `serve` (its loop), `Router` (sink thread: `route`/`step`), `Mixer`
  (callback: `render`), the `cpal` stream builder (`build_stream`), the WAV→device-format converter
  (`to_device_buffer`), `check_message` (shared with the testkit's `SoundTap`), tests.
- `crates/ludion-audio/src/music.rs` — streamed Ogg Vorbis music (`lewton`): `MusicFeed` (sink thread:
  decoder, linear resampler, loop reopen, the failure table) filling a bounded ring of `MusicChunk`s,
  and `MusicPlayer` (callback: generation check, gain ramp, sum). `Track::next_packet` clamps the
  final packet to the last page's granule (gapless loops); `is_feeding` includes staged samples so a
  once track's tail drains; `Converter::retarget` rebuilds on a reopened track's changed format.
- `crates/ludion-core/src/audio.rs` — the shared types this crate receives: `AudioEvent` (sound path
  + gain + category, `AudioEvent::new` defaulting to `Sfx`), `AudioMessage` (one-shot / loop sync /
  loop stop / blanket stop / music play / music stop), `LoopId`, `AudioCategory`, and `CategoryGains` (the mixdown the game
  authors on its builder).
- `crates/ludion-engine/src/audio.rs` — the sim-side seam: `AudioQueue` (the resource game systems
  push into, with `play`, `play_at`/`push_at` (positional one-shots) and `loop_start`/`loop_set_gain`/`loop_stop`, `music_play`/`music_play_once`/`music_stop`), `AudioLogEntry` (the
  frame-stamped log the engine drains into, independent of whether a sink is attached), and
  `emitter_gain`/`audible_gain`/`AUDIBILITY_EPSILON`, the falloff and audibility rule the emitters and `play_at` share.
- `crates/ludion-components/src/lib.rs` + `engine.rs` — the declarative path: the `AudioEmitter`
  component (`L1136`: `range` must be positive and finite) and `Engine::sync_audio_emitters`, the
  frame-boundary step beside `drain_audio`.
- `crates/ludion-assets/src/lib.rs` — `AudioPcm` + `load_wav`/`decode_wav`, the WAV decode this cache
  calls into.
- `games/hello-paddle/tests/audio.rs` — an end-to-end proof: a game emitting `AudioEvent`s through a
  live engine.

## Rules
- **Music streams through a bounded chunk ring.** Generations discard stale chunks (a newer one is
  held until its command arrives, never dropped); the callback never decodes or allocates; fades and
  the loop seam are computed in the sink (`spawn` takes the tick length to time them).
- **Playback never feeds back into the sim.** The channel from `Engine` to `AudioSink` is strictly
  one-directional — the sink has no way to influence a
  tick, so wall-clock jitter from the device is unobservable from the simulation. This is what keeps
  audio outside every determinism gate.
- **No device is not a failure.** On a headless/CI/device-less machine, `open_output` reports
  [`L5001`](../crates/ludion-core/src/codes.rs) (`AUDIO_DEVICE_UNAVAILABLE`) once as a `warning[L5001]` and
  the sink goes *silent*: it drains every event, so nothing blocks and the run stays green. A stream
  lost mid-run is reported once at `L5002` and reopened with backoff. A broken sound
  reference warns once and is skipped.
- **`AudioEvent`/`AudioMessage` live in `ludion-core`, not in this crate or `ludion-engine`.** Both
  the emitter (the engine, never gaining a playback dependency) and the sink (this crate, a lean leaf)
  need the types, and neither may depend on the other — so they sit in the shared core both depend on.
- **Loop playback state lives here and nowhere else.** The engine declares (`LoopSync` is idempotent:
  same id + same sound = a gain change in place, no restart) and holds no mirror of what is sounding;
  this crate's `loops: HashMap<LoopKey, Voice>` (`Copy` keys; `Router::key` interns names) is the whole
  of that state, reserved by `Mixer::new`; the Router's `declared` mirror replays it into a reopened
  stream. So a world rebuild sends `StopAllLoops`, not a diff.
  `Engine::audible_emitters` is no counter-example: a `StableMap<EntityId, SentLoop>` of the last
  sync per emitter in earshot (the resend filter), no playback state, cleared by the same rebuild.
- **Per-category master gain is the sink's, not the world's.** `AudioSink::spawn` takes a
  `CategoryGains { sfx, ambient, music }` (identity by default); `mix_into` multiplies each voice by
  `gains.gain_for(voice.category)` once per voice per callback, and `MusicPlayer` applies `music`.
  It is a mixdown/output concern, so no gain value can reach a `WorldHash`, a replay or the
  `audio_log`. The **game** authors it (`EngineBuilder::category_gains`), rides on the built
  `Engine`, and the facade's `wire_audio` reads it there — one value, no per-host knob, no runtime
  setter.
- **The decode cache is keyed on the file, not the path, and holds failures too.** Every attempt is
  stamped with the file's `(len, mtime)` (`stamp_of`, re-read at most every `STAMP_RECHECK`, 500 ms), so
  a WAV regenerated under a running host is re-decoded on its next message after that; a failed decode
  is cached exactly like a success, so a bad `sound` costs one open per file version, not one per
  `LoopSync`. Metadata is fair game here: the sink thread is outside every determinism gate.
- **The mixer soft-limits before it clamps.** `soft_limit` passes anything at or under
  `LIMITER_KNEE` (0.8) untouched and bends everything above it onto full scale along a continuous,
  monotone curve. `collapse_simultaneous` bounds *identical* stacking; this bounds the cross-voice sum
  it cannot see. The `[-1, 1]` clamp stays as the last line, never as the shaping.
- **Decode and resample happen off the real-time audio callback.** `Mixer::render` only sums cached,
  device-format `f32` buffers; decoding happens on the sink thread (`Router`). Never add
  decoding/allocation inside the `cpal` callback. Both halves hold no stream, so tests drive message
  → `VoiceCommand` → samples without a device (`Rig`); `build_stream` only wraps `Mixer::render`.
- **One envelope per voice; `VoiceLimits` bounds the rest.** `voice_limits(VoiceCaps)` builds them (oldest-steal); the caps (default 4 per sound / 64 total) reach `AudioSink::spawn` from the game's `ludion.toml` `[audio]` table, read by the facade host (`host::voice_caps`, nearest manifest at or above the scene dir). `Envelope` is an integer countdown stepped
  once per device frame (`RAMP_MS` = 10) for fades and slews. Only live voices (loops, or not-retiring
  one-shots) count against the caps; `voices` holds ≤ 2×total, cutting the lowest fading voice. Loops
  are never stolen; the Router holds those past the total. `Quietest` is level × bus master. `Mixer::new` reserves every container and
  `render` mixes in blocks; tests build a `Mixer` with other limits.
- `#![forbid(unsafe_code)]`, `#![warn(missing_docs)]`, like every sibling.

## Dataflow
The engine drains each tick's `AudioMessage`s onto an unbounded `crossbeam_channel` and into its bounded `audio_log` (4096); `Engine::sync_audio_emitters` adds the positional `LoopSync`/`LoopStop`s; the sink's `Router` turns a message into one `VoiceCommand` for the `Mixer` callback. Message path, emitter falloff, stream-loss recovery and the mixer render: `ludion devdocs ludion-audio-flow`.

## Verify
```
cargo test -p ludion-audio             # router + mixer without a device, mixer math, L5001
cargo test -p ludion-engine audio      # AudioQueue/AudioLogEntry, the drain-at-frame-boundary path
cargo test -p hello-paddle --test audio  # end-to-end: a game's audio through a live engine
cargo run -p ludion-cli -- explain L5001
```
No golden covers audio — the frame-stamped `audio_log` is the replay-checkable surface.

## Gotchas
- **`AudioQueue`/`audio_log` are excluded from `WorldHash` and from snapshots.** They are plain
  ECS resources holding request data, not simulation state — do not route gameplay logic through
  "did a sound play," and do not expect `engine.snapshot`/`engine.restore` to preserve pending or
  historical audio events.
- **A broken sound path warns once, not every frame.** Expect exactly one load diagnostic
  (`L1101`) per
  distinct path *per version of that file*, not a repeat every time the event fires. Rewriting the file is what earns a second line (and a second decode).
- **Playback threads never exit.** cpal 0.15's WASAPI backend caches its `IMMDeviceEnumerator` in a
  process-global `OnceLock`, inside the COM apartment of the first thread that looked up a device. A
  thread exit runs `CoUninitialize` and leaves that cache dangling, so the next host's device lookup
  is an access violation (`0xc0000005`) on a second host. Never go back to a thread per sink; a finished session parks its thread in `IDLE`.
- **The sink is its own thread.** `AudioSink::spawn` never fails (an OS spawn failure is the only panic
  path); device failures degrade to the silent path, never an `Err` for the host.
- **This crate depends on `ludion-assets`, not the other way around**, and neither depends on
  `ludion-engine` — only a host wires the engine's sender to this crate's receiver. There is no
  engine-side playback API by design.
