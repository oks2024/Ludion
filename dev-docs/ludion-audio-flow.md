---
description: ludion-audio dataflow: engine drain and emitter sync, sink session, stream recovery, mixer render
---

# ludion-audio dataflow

**Scope:** how an `AudioMessage` travels from the engine's frame-boundary drain to device samples, and how the sink recovers a lost stream. The hub (layout, rules, gotchas) is `ludion devdocs ludion-audio`.

## Dataflow
A host builds an unbounded `crossbeam_channel` (on purpose: the change-only protocol never resends a
stop or sync, and the sink drains it in every state), gives the engine the sender
(`Engine::set_audio_sender`), and gives this crate the receiver plus a content root and the engine's
own mixdown gains (`AudioSink::spawn(rx, content_root, engine.category_gains(),
engine.seconds_per_tick())`). The engine's per-tick drain (`Engine::advance_live`) pushes every
`AudioMessage` a game system queued that frame onto the channel *and* into its own `audio_log` — the
sink is optional; the log is not. `Engine::install_audio_queue` emits one more on every world
rebuild: the blanket `StopAllLoops`. **`audio_log` is a `VecDeque` bounded to `AUDIO_LOG_CAPACITY`
(4096)**, oldest dropped first (a switch carries the log, so unbounded would leak in a shipped host).

Two more arrive from the engine with no game system behind them. `Engine::sync_audio_emitters` runs
at the same frame boundary, straight after the queue drain: it finds the listener (lowest-`EntityId`
via `query_ids::<(&Camera,)>()`; none means a no-op tick), joins `AudioEmitter`/`Transform` with
`query_ids::<(&AudioEmitter, &Transform)>()` in ascending `EntityId` — a typed storage join, not a
per-entity probe, so zero emitters costs nothing — and sends a `LoopSync` under `LoopId::Entity(id)`
(when audible, then on a 1% gain move or a new sound or bus) at
`gain × clamp(1 − ground_plane_distance/range, 0, 1)` (`emitter_gain` returns `0.0` for a
non-positive or non-finite `range`, rather than the clamp saturating to full gain; `L1136` catches it
at deserialize time, but every vigil emitter is code-inserted). Anything that fell below
`AUDIBILITY_EPSILON` since last tick — despawned, muted, out of range, component removed — gets a `LoopStop` first, off the
`audible_emitters` keys. Stops precede syncs so a recycled `EntityId` cannot have its
fresh sync undone by the departing entity's stop.

`AudioSink::spawn` hands a session to an idle `ludion-audio` playback thread (or spawns one) running
`run`: `Playback::start` opens the default output and builds a stream on its native config
(`F32`/`I16`/`U16`; else `L5001`); `serve` loops `recv_timeout` into `Playback::wake`. The callback queue
is bounded (`COMMAND_CAPACITY`, `try_send`); a full queue, a cpal error, a rendered stream silent for
`STALL_AFTER` or none rendering within `START_WITHIN` drops the stream (`L5002`), and `wake` reopens it
with backoff (reset after `HEALTHY_AFTER` of rendering), replaying `declared` and resuming the music;
before the first render one-shots past `PRESTART_ONE_SHOTS` are dropped; while lost, messages are
routed and dropped. `Router::route` turns one
message into at most one `VoiceCommand`: a sound resolves its buffer from a
`HashMap<String, Decoded>` cache (`resolve`: stamped, failures cached) — first use decodes via `load_wav` and
`to_device_buffer` (music's linear `Converter`) — and
becomes a `Voice { samples, pos: 0, envelope, category, looping }`; stops carry no sound; an unloadable
sound or track is `None`. `step` sends the command, then refills the music ring on **every** wake,
so a failed route never starves it.

`Mixer::render` (the callback's whole body) applies arriving commands, collapses same-sound one-shots
of one callback (`collapse_simultaneous`: loudest gain, first-arrival order; ~10–21 ms scope, not one
sim frame), admits them under `VoiceLimits`, then mixes in `MIX_BLOCK_FRAMES` blocks: voices through
their envelope and bus master, the music, `soft_limit`, clamp, convert. Voice identity is the buffer's
`Arc` pointer; `Router::superseded` keeps evicted buffers off the callback's drop. `advance_into` moves the one cursor: a one-shot past its end drops; a loop wraps.

When the engine's sender is dropped, `serve` returns and `run` drops the stream. The thread returns to
the idle pool and drops the session's `done` sender; `AudioSink::drop` waits on that, so tearing down
blocks until playback has stopped.

The **silent path** (`drain_silently`) drains every event to disconnect without touching a device;
its `SilentCheck` runs `check_message` once per file version and prints the device path's warning.
