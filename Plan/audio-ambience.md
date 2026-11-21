# Plan: Audio Ambience — loops, positional emitters, category gains

> Status: **Audio space and mix (Neon Shroud request 0023): Wave A5a SHIPPED (2026-11-21)** — `AudioZone` (box or sphere in the entity's world pose, `priority`, `blend_m`, optional `reverb` of a `ReverbPreset` plus per-field overrides, up to 8 ambient beds, `bus_gains`), `L1651` (`AUDIO_ZONE_INVALID`, the `SemanticValidate` pointers), `LoopId::ZoneBed(EntityId, u8)`, `ReverbParams` (`DRY`, `clamped`, `near`), `AudioMessage::{ReverbSet, ZoneGain}`, the frame-boundary resolution in `ludion-engine/src/audio_zones.rs` (between `refresh_occlusion` and `sync_audio_emitters`), bed sounds checked by `check-scene` at `/entities/<i>/components/AudioZone/beds/<j>/sound`, `ludion docs audio-zones`. Zone beds play for real; the sink carries `ReverbSet` and `ZoneGain` to nothing (A5b). A world with no zone has a byte-identical stream; no wire, hash, replay or schema movement beyond the new component (the component schema snapshot gained `AudioZone`). **By-ear tuning of the preset values is owed** (they ship as proposed). **Wave A4b SHIPPED (2026-11-21)** — `EngineBuilder::audio_occlusion(AudioOcclusion)` (off by default; an identical stream without it), listener-to-source rays through `PhysicsWorld::ray_cast` with the game's `QueryFilter` for `AudioEmitter` loops, `play_at`/`push_at` and `play_on`/`push_on` (never global sounds), per-ray `per_hit_db`/`max_db`, mean gain and a log2 low-pass cutoff by blocked share, root-based self-exclusion, a `max_rays_per_tick` round-robin budget, smoothing with unsmoothed first results, `RESYNC_CUTOFF_RATIO`, `L5101`/`L5102`, `ludion docs audio-space` §Occlusion. No per-emitter opt-out and no debug draw (Deferred). No wire, hash, replay or schema movement. **Wave A4a SHIPPED (2026-11-21)** — `VoiceSpatial { filter, send }` and `VoiceFilter { low_pass_hz, high_pass_hz }` (`LOW_PASS_OPEN_HZ`, `HIGH_PASS_OPEN_HZ`, `band_pass`) on `AudioEvent` and `AudioMessage::{LoopSync, VoiceStart, VoiceSync}` (the accepted struct-literal break), the sink's per-voice RBJ low/high-pass (`ludion-audio/src/dsp.rs`; a bypassed side runs no stage, a log2 sweep over the 10 ms ramp; `send` carried, not mixed), `AudioQueue::push_on`, `CategoryGains` `#[non_exhaustive]` with `master`/`voice`/`ui` and `with(bus, gain)`/`gain(bus)`, `ludion docs audio-space` §Filters. No wire, hash, replay or schema movement; no external game literal broke. **Wave A3b SHIPPED (2026-11-18)** — `DuckRule`, `EngineBuilder::audio_duck`, `AudioQueue::duck_start`/`duck_end`, `AudioMessage::Duck`, the sink's duck layer, `L5100`, `ludion docs audio-mix` §Ducking. No wire, hash or replay movement. **Wave A3a SHIPPED (2026-11-18)** — `AudioQueue::play_on`/`voice_stop`, `VoiceId`, `AudioMessage::{VoiceStart, VoiceSync, VoiceStop}`, the sink's keyed voices (a finished one leaves the loop table), `ludion docs audio-space` §Attached voices (§Decision 11). No wire, hash or replay movement. **Wave A2 SHIPPED (2026-11-18)** — `AudioListener { orientation }` and `ListenerOrientation::{Camera, Listener}` (`camera` / `self`), `L1650` (runtime once + `check-scene`), `ludion docs audio-space` (§Decision 10). No wire, hash or replay movement; the component schema snapshot gained `AudioListener`. By-ear check owed with A1's. **Wave A1 SHIPPED (2026-11-18)** — `AudioCategory::{Voice, Ui}`, `Bus`, `MixLayer`, `AudioQueue::bus_gain`/`bus_pause`/`bus_resume`, `AudioMessage::{BusGain, BusPause}`, per-frame bus envelopes in the sink (held buses keep cursors, device reopen replays bus state), `ludion docs audio-mix` (§Decisions 7–9). No wire, hash or replay movement; the component schema snapshot gained the `voice` and `ui` consts. By-ear check on a real device is owed. Wave A5b (the sink side) and A6 not started.
> Earlier: **Adaptive multi-stem music SHIPPED (2026-11-09, Serious Isaac 0025, conservative)** — `AudioQueue::music_set(id,
> stems, fade)` plays up to 8 Ogg stems (`MUSIC_STEMS_MAX`) on one shared playhead and
> `music_stem_gain(stem, gain, fade)` slews one stem's gain in sim ticks. The sink decodes each stem with its
> own decoder and converter but sends one chunk carrying every stem's samples for the same device frames, so
> the stems cannot drift; a stem whose rate or length differs from the first (or past the 8th) is left out
> with `L5006`. The same id again is a no-op (initial gains apply only at the set's start); a different set
> hard-cuts. `music_play` keeps emitting today's `MusicPlay` (byte-identical stream) and plays as a one-stem
> set. No wire, schema, replay or hash movement; no Cargo change. Crossfade is design only (§Decisions
> Proposed (0025)). By-ear check on a stereo device is owed.
> Earlier: **Stereo pan + spatial distance SHIPPED (2026-11-01, r2013, Clockout 0035)** — two per-game builder
> knobs, `EngineBuilder::audio_pan(AudioPan::{Centred, Stereo})` and
> `audio_distance(AudioDistance::{Planar, Spatial})`, both defaulting to today's behaviour so every
> existing game's stream is unchanged. Pan is **opt-in** and separate from the distance knob (the
> request implied always-on; a 2D game and vigil keep a centred stream). `AudioEvent` and
> `AudioMessage::LoopSync` gained `pan: f32` — a **public API break** for a game writing an
> `AudioEvent` struct literal (the documented ambient one-shot; `AudioEvent::new` sets `0.0`). The
> engine resolves it from the camera's rotated right/up axes (`stereo_pan`, horizontal plane only, so
> ahead/behind/overhead are exactly `0.0`); the sink applies a constant-power law at unity centre
> (hard pan is +3 dB on the near side) with a 10 ms slew, on two-channel devices only, and a
> `Centred` voice takes the unpanned path bit-identically. Deviation from the request's literal
> acceptance: inside `PAN_INNER_RADIUS` (0.5) the pan scales continuously by `len/R` instead of a hard
> zero, so it never jumps as a sound passes through the listener. Resync step `0.02`; a stereo clip
> narrows to its channel mean as the pan leaves centre; `collapse_simultaneous` takes the louder
> copy's pan. No wire, hash or replay movement. By-ear check on a stereo device is owed.
> **Wave 3 COMPLETE (2026-09-03) — the plan's mechanized half is CLOSED** — `AudioEvent`
> carries `category: AudioCategory` and `AudioEvent::new(sound, gain)` is the `Sfx` default (`play()`
> goes through it; an ambient one-shot is a literal through `push`; every other literal migrated —
> compiler-enforced, three found). The sink's `CategoryGains { sfx, ambient }` (`Default` = identity,
> `gain_for`) is the third argument of the ONE `AudioSink::spawn` signature — no second constructor,
> no `HostOptions` field, no runtime setter (decision 6's `[USER CALL]` and master-plan item 18 stay
> deferred); `Voice` carries its category and `mix_into` resolves the bus master once per voice per
> callback (one multiply per sample); a same-sound `LoopSync` upsert updates gain AND category in
> place, so a loop changes bus without a restart; `collapse_simultaneous` keeps the first arrival's
> category (identity is the sound). `crates/ludion/src/host.rs` passes `CategoryGains::default()`:
> the vigil mixdown values this plan called "a tuning call once Wave 3 lands" have no game-side
> surface by design — the host lives inside `crates/ludion`, so a game wanting `ambient: 0.6` is the
> trigger for a builder/`LaunchRoots` setter, judged then. Tests: three sink tests (a mixed
> Sfx+Ambient scene at exact dyadic sums, the identity default, an Ambient→Sfx re-sync swapping the
> master without moving the cursor) and `play` pinned to `Sfx` in the engine; `docs/audio.md` gained
> `## Category gains` (7,071 B of 12 KB, mirror byte-identical); one dev-docs rule. Zero
> wire/schema/hash movement (`AudioCategory`'s hand-written schema is untouched; the tag rides the
> sink); committed replays untouched (vigil replay + session canary). Verified on the laptop:
> workspace clippy `-D warnings` clean; ludion-testkit's `composite` lane went red in parallel with
> `wgpu error: Out of Memory` on the Intel iGPU and green 5/5 serially — environment, not the change.
> **Wave 2 COMPLETE (2026-08-29)** — `AudioEmitter { sound, gain, range, category }` is the
> engine's 23rd built-in (append-only `register_validated`, `L1136` rejects `range <= 0` — the
> `COLLIDER_NON_POSITIVE_EXTENT` mirror — and `ludion explain L1136` works). The sync step
> (`sync_audio_emitters`) sits in the frame boundary right after `drain_audio`: listener =
> lowest-`EntityId` `Camera` (no camera → no-op tick, decision 3 literal), ascending-id walk, falloff
> `gain × clamp(1 − ground_dist/range, 0, 1)`, nothing sent at or below `AUDIBILITY_EPSILON`;
> liveness is an engine-side `StableSet` diffed per tick with **stops emitted before syncs** (a
> recycled id cannot have its fresh sync undone), reset in `install_audio_queue` beside the blanket
> `StopAllLoops`. `AudioCategory`'s field schema is HAND-WRITTEN (`schemars(schema_with)`) — the
> `Parent`/`EntityId` treatment; ludion-core stays schemars-free, and a test pins that an unknown bus
> name is still `L1014`-rejected. Emitter default category is `ambient` (an emitter is a bed by
> construction; the plan left it open). Schema-drift gate clean (a registry append is invisible to
> the protocol snapshot — verified, 82 protocol tests green); committed replays untouched (proved by
> canary: vigil replay + session 6/6). `docs/components.md` sits 6 bytes under its 12 KB budget —
> the next component addition should split the topic. `dev-docs/ludion-engine.md`'s registry-order
> bullet was ALREADY stale (stops at `PostProcess`) and was left for a docs pass rather than
> half-fixed. hello-paddle's walking-toward/away scenario is the second-consumer proof.
> **Wave 1 COMPLETE (2026-08-29)** — the loop primitive shipped as decision 1's literal
> transport: `AudioMessage { OneShot, LoopSync, LoopStop, StopAllLoops }` + `LoopId::{Sound,
> Entity}` + `AudioCategory { Sfx, Ambient }` in `ludion_core::audio` (`EntityId` lives in
> ludion-core, so `LoopId::Entity` names it directly); `AudioQueue` carries `Vec<AudioMessage>` in
> push order (`play()` wraps into `OneShot`, every call site untouched) and gains
> `loop_start`/`loop_set_gain`/`loop_stop` (`loop_set_gain` delegates to `loop_start` — decision 1
> makes them the same idempotent `LoopSync`; loops are `Ambient`); loop playback state lives ONLY in
> the sink (`HashMap<LoopId, Voice>` beside the one-shot `Vec`, upsert = same id + same buffer Arc →
> gain in place / different sound → replace from 0; `StopAllLoops` clears); `Voice` wraps `pos` in
> looping mode; `install_audio_queue` emits `StopAllLoops` through the ordinary emit path so a
> rebuild's blanket stop reaches sink AND log (one engine transition test moved 1 → 2 entries — the
> intended consequence). `AudioLogEntry` became `{ frame, message }` (a flat kind would have needed
> placeholder fields; `sound()`/`gain()` accessors kept the four consumers compiling and
> `ludion_testkit::audio::audio_messages` joins `audio_stream` for tests that tell a sync from a
> stop). Sink stores but does not yet apply `category` (Wave 3's mixdown). `docs/audio.md` grew the
> loops section (mirror re-copied byte-identical); dev-docs updated. Zero wire/schema movement; the
> committed vigil replay is untouched (audio is outside `WorldHash`). Waves 2–3 remain PROPOSED.
> Originally authored 2026-08-16 from a game-design gap analysis.
> Companion plans: private.
>
> **Relationships:** self-contained — builds only on shipped `ludion-engine`/`ludion-audio` surface.
> Wave 3's category-gain *control surface* (a settings slider) is blocked on `Plan/ui-widgets.md`'s
> menu capability; this plan ships the plumbing, not the widget.

## Why

A game's ambience (tolling bells, chanting, a crackling hearth) and sound-based discoverability both
need sound that *persists* and *varies with distance* — neither is expressible today. Sound is a
first-class identity channel alongside silhouette and color, not a polish pass. Today's audio
(`AudioQueue::play`) is one-shot only: fire, play once, gone. There is no way to leave a bell tolling,
no way to make a chant louder as the player nears it, no way to keep ambient beds from competing
with combat SFX. This closes that gap as a generic capability — sound-based discoverability is not
vigil-specific; any spatial game wants "make this loop findable by ear."

## What exists today (verified)

- **`AudioEvent`** (`crates/ludion-core/src/audio.rs:32`): `{ sound: String, gain: f32 }` — a request
  to play once, nothing more.
- **`AudioQueue`** (`crates/ludion-engine/src/audio.rs:28`): a per-tick resource; `.play()` pushes an
  `AudioEvent`; `Engine::drain_audio` (`engine.rs:897-913`, called from `advance_live_profiled` at
  line 756) drains it every frame boundary into the frame-stamped `audio_log` (queryable via
  `ludion_testkit::audio::audio_stream`) and, if wired, a `crossbeam_channel::Sender<AudioEvent>`.
  Excluded from `WorldHash`/snapshots. `install_audio_queue` (`engine.rs:1068-1072`) reinstalls a
  fresh, empty queue after **every** world rebuild — same mold as `install_scene_switch`
  (`engine.rs:1079-1083`).
- **`AudioSink`** (`crates/ludion-audio/src/lib.rs`): the one device-touching crate. Its
  `Voice { samples: Arc<Vec<f32>>, pos, gain }` (line 75) plays once and retires the instant `pos`
  passes `samples.len()` in `mix_into` (line 269) — **no looping primitive exists anywhere.**
  `collapse_simultaneous` (line 245) merges same-callback duplicate one-shots to the loudest gain — a
  burst anti-clip guarantee, not a mixer bus. The mixer sums, then soft-limits (a smooth knee past `0.8`; `docs/audio.md`).
- **No positional concept exists.** No `Listener`, no per-entity audio component. The closest analog
  is `PointLight` (`components.rs:186-197`: color/intensity/`range`, position from `Transform`) — a
  component the renderer consumes with **no game-registered system required**; its falloff
  (`clamp(1 − dist/range, 0, 1)`, per `Plan/clustered-forward.md`) is the shape this plan borrows.
  `Camera` (`components.rs:1350`) has position via `Transform` too; no `Listener` type exists.
- **Determinism** (`docs/audio.md`, `dev-docs/ludion-audio.md`): the event stream is "a pure function
  of the simulation," a replay regenerates it identically, `WorldHash` is unaffected (the queue is a
  resource). **Any new surface must preserve this** — sim-observable state lives in `World`
  components; the sink stays a dumb, replayable consumer of a deterministic event stream.
- **No wire protocol involvement** — `ludion-protocol`/`dispatch.rs` name no audio type (zero greps).
  This plan keeps it that way.
- **OGG/streaming music**: trigger recorded (`Plan/phase-8-assets-and-sound.md`, "music-length tracks
  making committed WAVs a repo-size problem"); declined for vigil at G7 (2026-07-23), then
  **shipped 2026-10-03, rev 1106 (external shmup request 10)** — `docs/audio.md` §Music. Stems shipped
  2026-11-09 (0025). Set-to-set crossfade is proposed (§Decisions Proposed (0025)); Opus and live music
  volume stay deferred.
- **Vigil's discipline** (`games/vigil/CLAUDE.md:66-78`): WAVs are generated
  (`node scripts/synth-sfx.mjs`), path+gain live as `pub const`s in `tunables.rs`, **gains stay
  ≤ ~0.8** since the mixer's soft knee starts there. New loop/emitter gains inherit this, unrelaxed.

## Decisions

**1. Add a loop primitive keyed by identity, not a start/stop pair of one-shots.** Re-firing `play()`
every tick re-triggers the sample from position 0 — audible stutter, not a loop — and cannot express
"smoothly track distance" without restarting on every gain change. The transport gains a small enum
next to `AudioEvent` in `ludion_core::audio`:

```rust
pub enum AudioMessage {
    OneShot(AudioEvent),          // today's play(), unchanged
    LoopSync { id: LoopId, sound: String, gain: f32, category: AudioCategory },
    LoopStop { id: LoopId },
    StopAllLoops,                 // decision 4
}
```

`LoopSync` is **idempotent and declarative**: calling it every tick with the same `id`/`sound`
updates the running voice's gain in place (no restart, no click); a new `id`, or a changed `sound`
under an existing `id`, starts fresh. This makes "recompute distance, resend gain, every tick" the
natural calling convention instead of asking every call site to hand-diff its own gain changes.

**2. `LoopId` has two constructors, because the two use cases need different identity.** A global
ambient bed (city drone, wind) is a singleton — its natural identity is the sound path, reusing what
the decode cache and `collapse_simultaneous` already key on. A positional emitter (Decision 3) is
*not* a singleton — a district can place several bell towers looping the same WAV independently;
collapsing them to one shared voice would silence all but one. So: `LoopId::Sound(path)` for the
manual primitive (Wave 1), `LoopId::Entity(EntityId)` for the declarative `AudioEmitter` component
(Wave 2 — `EntityId` is already the stable, recycled owner). Both funnel through one sink-side
`HashMap<LoopId, Voice>` (std, not the sim-side denylisted collections — the sink sits outside that
boundary already). A caller-invented string handle was considered and rejected: it just reinvents
`EntityId`'s uniqueness guarantee, badly.

**3. The listener is the `Camera`-holding entity's `Transform`; no new `Listener` type.** Every
rendering scene already has exactly one meaningful camera position — a second, audio-only "where is
the ear" concept would be a second way to say the same thing. Multiple `Camera` entities tie-break on
ascending `EntityId` (the `clustered-forward` extract-order convention). No `Camera` present: Wave 2's
system is a no-op that tick (`Option`-test-and-skip, the profiler arc's "disarmed" idiom).
**[USER CALL]** — a future genre might want a listener decoupled from the camera; recommendation is
to ship the coupled version now and add a `Listener` marker only when a real second consumer needs it.

**4. A world rebuild sends a blanket stop, not a diff.** `SceneSwitch` already guarantees resources
don't cross a transition; a loop's playback state lives only in the sink (Decision 6), so the engine
has nothing to diff against after a rebuild. `install_audio_queue` (`engine.rs:1068`) is extended to
also send `AudioMessage::StopAllLoops` on every reinstall — directly over `audio_sender`, the same
resource-request mold as `drain_audio`. One-shots are unaffected (they retire on their own).

**5. `AudioEmitter`'s stop is a per-tick liveness diff, not caller discipline.** An emitter entity can
be despawned mid-scene (an escorted NPC, a dying enemy's aura) with no `SceneSwitch` involved —
Decision 4 doesn't fire here. Wave 2's system already re-queries every `AudioEmitter` entity each
tick; it keeps a `StableSet<EntityId>` (`ludion_core::collections`, resource-excluded-from-hash, same
category as `AudioQueue`) of last tick's synced handles, diffs against this tick's, and emits
`LoopStop` for anything that dropped out. **Only the declarative path gets this for free** — Wave 1's
manual `loop_start`/`loop_stop` has no safety net: a forgotten `loop_stop` leaks, like forgetting to
clear any other engine resource-request flag.

*(The runtime setter for the player's volumes is `Plan/settings.md`.)* **6. Category is a tag the sink applies gain to, not sim-observed state.** `AudioMessage`/`AudioEvent`
gain `category: AudioCategory` (`Sfx` | `Ambient`; `Sfx` default, matching `.play()`'s existing call
sites). The per-category *master* gain, though, lives in `AudioSink`, not `World`/`AudioQueue` — it's
a mixdown/output concern (which bus is how loud), same category as "which device to open," not
gameplay state a replay must reproduce. **[USER CALL]** — a future volume slider naturally reads from
a game's own `Profile<P>` (the established durable-setting vehicle) into a small `AudioSink`-side
setter; recommend building that only once `Plan/ui-widgets.md` lands a settings surface, not now.

## Decisions Proposed (0025)

Owner ruling owed. Nothing below is built.

**Set-to-set crossfade.** `music_set` and `music_play` hard-cut; a game that wants the old music to fade
while the new rises has no verb. Design: two **set slots**, each with its own ring and generation (sets need
no mutual alignment, so no chunk carries both). A crossfade starts the new set in the idle slot and fades
the old slot out over the same ticks; a third set mid-crossfade hard-cuts the outgoing slot; at most 2 live
sets (16 stems); a world rebuild stops both. The sink's per-callback cost is one more ring drain and
mix.

**API recommendation: a new verb `music_crossfade(id, stems, ticks)`**, so the shipped `music_set` keeps
its hard cut — flipping `music_set` to crossfade would change the behaviour of games already calling it.
Tests owed: both sets audible mid-fade, the old one silent after, a third set mid-fade cutting the
outgoing slot, `audio_messages` showing the new message.

## Decisions — audio space and mix (Neon Shroud 0023)

Numbered on from Decision 6. Codes: L5100–29 sink and runtime, L1650–69 scene validation.

7. **Buses.** `AudioCategory` gains `Voice` and `Ui`; a `Bus` enum (`Master | Sfx | Ambient | Voice | Ui | Music`) names every mixdown bus, with `From<AudioCategory>`. The master bus multiplies every other bus.
8. **Runtime gain.** A sim-emitted `AudioMessage::BusGain { bus, layer, gain, fade_frames }` with two layers, `User` (a settings slider) and `Game` (ducking, scripted mixes). Effective gain = `authored x user x game x pause`. Amends Decision 6 and Deferred "runtime gain": both triggers fired (0022 settings, ducking).
9. **Pause.** `AudioMessage::BusPause` holds each voice's cursor (loops and music included). A world rebuild (`StopAllLoops`) resets the Game layer and the pauses, keeps the User layer, and drops the voices held on a paused bus.
10. **`AudioListener`.** A component that moves the listener off the camera; absent means the camera, byte-identical. Amends Decision 3. Settled in A2: `orientation: camera` (default; pan axes from the lowest-id `Camera`) or `self` (the listener's own world rotation); with no camera the listener's own axes are used whatever `orientation` says; the lowest `EntityId` carrier wins and a second raises `L1650` (a warning; once at runtime, and in `check-scene`); distance is from the listener's world position through `Parent`, and the listener need not be a body.
11. **Attached one-shots.** `play_on(entity, ...)` returns a `VoiceId`; the sim reads a WAV header for the clip length. Settled in A3a: ids are a per-queue counter, reset on rebuild; a voice starts even when inaudible (gain `0.0`) so a speaker walking into earshot is heard; it is tracked until `start + ceil(len)` ticks (`ceil(frames x tick_hz / sample_rate)`, integer math, read once per path through `load_wav`, so a full decode); a despawn stops it with the sink's ~10 ms ramp fade; an unreadable clip is sent but never tracked; a device reopen drops voices (the router re-declares loops only); a voice carries a filter and send from A4a (Decision 13).
12. **Ducking.** `DuckRule` plus manual `duck_start`/`duck_end`, with a `-24 dB` floor (`DUCK_FLOOR_DB`). Settled (A3b): the message is `Duck { bus, gain, fade_frames }`, applied as a sink-private fourth layer (`authored x user x game x duck x pause`), not a `MixLayer::Duck` (a public layer would let `bus_gain` write state the engine owns). Liveness is sim-side: plain one-shots on a `when` bus are timed by `voice_ticks`, attached voices count while tracked and audible. Sources multiply, floored at `DUCK_FLOOR_DB`; the fade is the attack of a source turning on or the release of one turning off. A rebuild resets ducks. `when` is never `Master` or `Music`; `L5100` refuses a bad rule at build.
13. **`VoiceSpatial { filter, send }`** rides on messages; the sink applies per-voice biquads. Settled in A4a: `AudioEvent` and `AudioMessage::{LoopSync, VoiceStart, VoiceSync}` carry `spatial: VoiceSpatial { filter: VoiceFilter { low_pass_hz, high_pass_hz }, send }`, default bypass (`LOW_PASS_OPEN_HZ` 20 kHz and `HIGH_PASS_OPEN_HZ` 20 Hz mean off) with `send 0.0`. `send` is carried and not mixed (no reverb bus until A5). The sink runs one RBJ low-pass and one high-pass biquad per voice, filtering each raw sample before gain; a bypassed side has no stage, so the voice is bit-identical to an unfiltered one. A cutoff change sweeps in log2 frequency over the 10 ms ramp, a side turning on sweeps in from open and a side turning off sweeps open and drops its stage. `AudioQueue::push_on(entity, event, range)` starts an attached voice with a filter; `play_on` delegates to it.
14. **Occlusion.** Ray occlusion on `ray_cast` with 0002's masks, a round-robin budget per frame, smoothing in the engine. Settled in A4b: `EngineBuilder::audio_occlusion(AudioOcclusion)`, off by default (no call, an identical stream); only positional sources (`AudioEmitter` loops, `play_at`/`push_at`, `play_on`/`push_on`) are cast, never `play`, `loop_start` or music. Rays run listener to source (the centre, then `rays` 3 adds ±right and 5 adds ±up at `spread_m`), each through `PhysicsWorld::ray_cast` with the game's `QueryFilter` (default `only_fixed`). Hits whose `hierarchy_root` is the source's or the listener's root are dropped, so a guard's own capsule or the player's body never occludes. A ray's `db` is `(distinct blockers x per_hit_db)` floored at `max_db`; the source's gain is the mean of its rays' dB and its `blocked` the share of blocked rays, which sets the low-pass cutoff by a log2 lerp from open to `occluded_low_pass_hz` (`blocked == 0` is bypass). The budget is `max_rays_per_tick` rays per tick, spent on due sources (`interval_ticks`) round-robin from a cursor; an uncast source keeps its result; a play-time cast (`At`, `On`) is unsmoothed and counts against the budget, a short budget reads as clear. A source's first result is applied unsmoothed; later targets slew over `smoothing_ticks`. The resync rule gains the cutoff: `RESYNC_CUTOFF_RATIO` 0.05 (a message goes out when the cutoff moves more than 5 % or crosses bypass). The occluded message filter is the authored one with `low_pass_hz = min(authored, cutoff)`. Codes: `L5101` (a bad config, at build), `L5102` (the physics query refused the filter, once per engine; sources then read clear). Engine-side host state, never hashed or snapshotted.
15. **`AudioZone`.** Box or sphere, priority, blend, beds via `LoopId::ZoneBed`, `bus_gains`; resolved at the frame boundary from the listener point, not from sensors. Settled in A5a: per zone, ascending `EntityId`, `depth` is the sphere's `r - |p|` or the box's `min_i(h_i - |local_i|)` in the zone's world translation and rotation (scale ignored); `w` is `0` for `depth <= 0` or a degenerate shape, `1` for a hard edge (`blend_m <= 0`), else `clamp(depth / blend_m, 0, 1)`. Zones with `w > 0` rank by priority descending then `EntityId` ascending; `a` and `b` are the first two, with effective weights `e_a = w_a` and `e_b = (1 - w_a) x w_b` and every other zone `0`, so a full-weight winner masks everything beneath. Beds are `LoopSync`s under `LoopId::ZoneBed(zone, i)` on the ambient bus, non-positional, at `bed gain x e`, stopped when `e` reaches `0` or the zone or bed is gone. Bus gains go out as `ZoneGain` per bus (`lerp(1, g_a, e_a) x lerp(1, g_b, e_b)`), resent past `RESYNC_GAIN_RATIO` and always on landing exactly at `1.0`. The zone layer is an engine-owned layer beside the duck layer, never `BusGain { layer: Game }`: `BusGain` must not write state the engine owns (Decision 12's argument), so the effective gain is `authored x user x game x duck x zone x pause`. One code, `L1651`, for every semantic fault; runtime never reports (a code-spawned bad zone is sanitized). Deferred: positional beds (author a door `AudioEmitter`), runtime diagnostics for code-spawned zones, zone debug draw.
16. **Reverb.** One algorithmic reverb send and named presets. Message and preset half settled in A5a: `AudioMessage::ReverbSet { params: ReverbParams { decay_s, pre_delay_ms, damping, size, wet }, fade_frames }`, sent when the weighted blend moves past `ReverbParams::near` (1%, floor 1e-3) or lands exactly on `DRY`. Zones among `a`/`b` with a `reverb` key contribute: `wet` is the sum of `e x wet`, the other fields the `e`-weighted mean. Presets (`outdoor_open`, `room_small` the default, `hall_medium`, `hangar_large`) ship as proposed — decay_s 0.4/0.6/1.8/3.5, pre_delay_ms 0/5/20/40, damping 0.7/0.5/0.4/0.3, size 0.3/0.3/0.6/0.9, wet 0.05/0.2/0.25/0.3 — **by-ear tuning owed**. The DSP, the send bus and the per-bus default sends are A5b.
17. **`music_crossfade`.** The §Decisions Proposed (0025) design; the second consumer has arrived, **accepted 2026-11-19, scheduled (A6)**.
18. **`ludion_testkit::audio::audio_mix_at`.** A deterministic offline mix probe for tests.

## Waves — audio space and mix

- **A1 — mix** (this plan's first wave): decisions 7–9, `AudioQueue::bus_gain`/`bus_pause`/`bus_resume`, the sink's per-bus envelopes.
- **A2 — listener — SHIPPED 2026-11-18:** a new `crates/ludion-components/src/audio.rs`, `audio_listener` in `engine.rs`, a multi-carrier warning in L1650–69.
- **A3a — attached voices — SHIPPED 2026-11-18** (decision 11: `play_on`, `VoiceId`, keyed voices in the sink, the WAV-length read).
- **A3b — ducking — SHIPPED 2026-11-18** (decision 12: `AudioMessage::Duck`, `DuckRule`, `duck_start`/`duck_end`, `L5100`); reuses A3a's `voice_ticks` WAV-length cache.
- **A4a — `VoiceSpatial`, sink filters, the `CategoryGains` builder** (13): `VoiceSpatial` on `AudioEvent` is a struct-literal break, **accepted 2026-11-19, scheduled (A4, R13)**; in progress.
- **A4b — occlusion — SHIPPED 2026-11-21** (14): needed 0002's masks; an engine-only follow-up that writes into A4a's `VoiceSpatial`, with the 5% cutoff resync rule. The per-source opt-out is deferred (§Deferred).
- **A5a — audio zones, engine side** (15, 16 message and preset half): **SHIPPED 2026-11-21**. `AudioZone`, `L1651`, `LoopId::ZoneBed`, `ReverbSet`, `ZoneGain`, the resolution, `check-scene` bed sounds, `ludion docs audio-zones`.
- **A5b — zones, sink side** (16 DSP half, 18): the reverb DSP on a send bus, per-bus default sends (`sfx`/`ambient`/`voice` `1.0`, `ui`/`music` `0.0`; a voice's send is `clamp(bus default + spatial.send, 0, 1)`), the sink-private zone gain layer, the `ReverbSet` slew, `audio_mix_at`, gallery `examples/gallery/audio-zones.yaml` with its walk test and golden.
- **A6 — `music_crossfade`** (17), **accepted 2026-11-19, scheduled (after A5)**.

### Owner rulings (2026-11-19)

- Accepted, scheduled (R13 API break round; `#[non_exhaustive]` plus a builder); implemented in A4a. **`CategoryGains` fields.** A1 ships without new fields: `gain_for(Voice | Ui)` reads `1.0` and the master's authored value is `1.0`, because new fields break struct literals (one in `builder.rs`). The ruling: fields vs `#[non_exhaustive]` plus a builder.
- Accepted, scheduled (audio A4, R13); implemented in A4a. **`VoiceSpatial`'s struct-literal break** on `AudioEvent` (A4).
- Accepted. **The sim-side WAV read** for clip length (Decision 11).
- Accepted, scheduled (occlusion A4, R13; reverb A5). **Reverb and occlusion in scope**, amending the Deferred bullet; HRTF and Doppler stay out.
- Accepted, scheduled (A6, after A5). **`music_crossfade`** (Decision 17).
- Accepted. **The duck depth default** of `-9 dB` (Decision 12; now `DuckRule::default()`).
- Accepted. **An attached voice ducks only while audible**: one out of earshot does not dip the music (Decision 12).
- Accepted. **The duck layer is sink-private**, not a `MixLayer` (Decision 12).

## Waves

**Wave 1 — the loop primitive.** `ludion_core::audio` gains `AudioMessage`/`LoopId`/`AudioCategory`;
`AudioQueue` gains `loop_start(sound, gain)` / `loop_set_gain(sound, gain)` / `loop_stop(sound)` (the
`LoopId::Sound` path only); `install_audio_queue` sends `StopAllLoops` on rebuild (Decision 4);
`ludion-audio`'s `Voice` gains a looping mode (wrap `pos` instead of retiring) plus a
`HashMap<LoopId, Voice>` loop table beside the one-shot `Vec<Voice>`; `AudioLogEntry` grows enough
shape to distinguish start/sync/stop/stop-all in the text-first log. This alone lets a scene author a
global ambient bed (drone, wind, a single tolling bell) with one setup call and automatic cleanup on
any scene switch — **consumable by the existing sector prototype** (§9's vertical slice) the moment it
lands, even though §9 doesn't name audio explicitly.
- *Exit:* `cargo test -p ludion-audio` (loop wrap, in-place gain update, `StopAllLoops` clears the
  table, manual-loop → switch → manual-loop-again never leaks); `cargo test -p ludion-engine audio`
  (`AudioMessage` variants drain in order; a replay with `loop_start`/`loop_stop` regenerates a
  byte-identical `audio_log`, extending `ludion_testkit::audio::audio_stream`);
  `cargo test -p hello-paddle --test audio` extended with one loop scenario.

**Wave 2 — `AudioEmitter`: positional discoverability.** New component
`AudioEmitter { sound: String, gain: f32, range: f32, category: AudioCategory }` (mirrors
`PointLight`; same falloff shape, ground-plane `(x, z)` distance via `ludion_core::math::sqrt` — planar
stays the default, `AudioDistance::Spatial` a per-game option). No
game-registered system needed: an internal engine step (alongside `drain_audio` in
`advance_live_profiled`) queries every `(EntityId, &Transform, &AudioEmitter)`, computes gain, pushes
`LoopSync{ id: LoopId::Entity(id), .. }` above a small audibility epsilon (inaudible emitters send
nothing), and diffs liveness per Decision 5. The direct consumer: a chanting NPC or a bell tower, discoverable by ear
alone, zero authoring friction beyond placing the component. A semantic validator rejects `range <= 0.0` at the next free `L11xx`
code (`L1136` as of this writing, mirroring `COLLIDER_NON_POSITIVE_EXTENT`).
- *Exit:* falloff unit test at four known distances (`0` ⇒ full, half-range ⇒ half, at/beyond range ⇒
  exactly `0`); a despawn-mid-scene test proving `LoopStop` fires the next tick with no scene switch
  involved; a `games/hello-paddle` example (walking toward/away from a placed emitter) as the generic
  second-consumer proof; `L1136` registered, `ludion explain L1136` works. No human gate is owned
  here — the *feel* of an NPC getting louder on approach belongs to whichever game plan wires
  `AudioEmitter` onto it.

**Wave 3 — category gains.** `AudioMessage`/`AudioEvent` gain `category` (Decision 6); every existing
struct-literal `AudioEvent` construction (found today in `ludion-audio`'s and `ludion-engine`'s own
unit tests) needs `category: AudioCategory::Sfx` or migrates to a new `AudioEvent::new(sound, gain)`
default constructor — mechanical, compiler-enforced. `AudioSink::spawn` gains an optional per-category
master-gain parameter (default `1.0`/`1.0`), applied once per callback when summing — a
content-authored mixdown knob a host hard-codes (e.g. "ambient bus at 0.6"), not a live control (see
Decision 6's `[USER CALL]`).
- *Exit:* a sink test proving the category multiplier applies correctly to a mixed Sfx+Ambient scene;
  both crates green with `category` threaded through; `docs/audio.md` gains a short section (still
  under its 12 KB spoke budget).

## Engine/game boundary

**Lands in `crates/`:** `AudioMessage`/`LoopId`/`AudioCategory` (`ludion-core`), the loop table and
category mixdown (`ludion-audio`), `AudioQueue`'s new methods and the internal `AudioEmitter` sync
step (`ludion-engine`), the `AudioEmitter` component and its validator — all genre-generic, nothing
names a game, a district, or a character. The second-consumer story is concrete: `games/hello-paddle`'s
existing `tests/audio.rs` is the standing "prove it outside vigil" example, and both waves extend it.

**Stays in the game:** which sounds are authored as loops or emitters, at what gain (the
existing ≤0.8 discipline), and placing `AudioEmitter` on NPC/bell entities in scene YAML. That
authoring work is out of scope here. Category-gain *values* are a per-game tuning call, made once
Wave 3 lands.

## Deferred / non-goals

- **OGG/streaming music.** Shipped as a `CategoryGains.music` bus, not a category variant; see
  `docs/audio.md` §Music.
- **A `Listener` decoupled from `Camera`.** Decision 3, amended by Decision 10 (wave A2); trigger is a genre that demonstrably needs
  one, not a hypothetical.
- **Runtime-adjustable category gain (a volume slider).** Decision 6, superseded by Decision 8 (shipped, wave A1); the old trigger is `Plan/ui-widgets.md`
  landing a settings surface, or a game wanting programmatic ducking badly enough to justify the
  `AudioSink`-side setter early.
- **Loop fade-out on stop.** Shipped: a 10 ms stop fade and gain slew.
- **Stereo pan.** In scope since 2026-11-01: a 3D consumer needs "from the right direction" (vigil
  did not), shipped as the opt-in `AudioPan` (see Status).
- **Duck state across a restore.** A restore rebuilds the world today: its `StopAllLoops` resets every duck and
  ends every voice, so nothing stays ducked and nothing needs re-deriving. Once a voice can outlive a restore,
  the first `sync_ducks` after it re-derives each bus's target from voice liveness (the rules' live flags set
  from the restored voices, fade `0`) rather than waiting for an edge. Trigger: request 0012's restore that
  re-derives zone and duck state, or any voice that survives a restore.
- **Per-source occlusion opt-out.** An `AudioEmitter` field breaks vigil's three struct literals and moves the component schema. Trigger: a game needing an unoccluded positional source.
- **Audio debug-draw rays.** Trigger: the first occlusion tuning session.
- **Reverb, geometry occlusion, HRTF, Doppler.** Reverb and occlusion proposed into scope (Decisions 14, 16; **accepted 2026-11-19, scheduled (A4, A5)**); HRTF and Doppler stay out. Originally out of scope — §6.1 asks for "audible when near".

## Risks / open questions

- **Wave 2's per-tick cost** — the "only audible emitters send events" liveness scan (Decision 5) is
  cheap at a typical scale (a handful of emitters per district) but is an unbounded linear cost
  if a future scene authors hundreds close together. No budget key is proposed — nothing near that
  scale exists yet; the fix, if needed, mirrors `NAV_FLOW_RADIUS`'s windowed selection, not a
  primitive redesign. Worth a glance at the E6 gate once Wave 2 lands; not measured yet.
- **`LoopId::Sound` collisions across unrelated systems** (Decision 2's manual primitive): two
  subsystems both looping the same WAV without coordinating silently share one voice (last write wins
  on gain). Mitigated by convention — one drone, one wind bed, authored in one place, as vigil's
  `tunables.rs` already does for sound paths — not new machinery.
