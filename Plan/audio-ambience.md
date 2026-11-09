# Plan: Audio Ambience — loops, positional emitters, category gains

> Status: **Adaptive multi-stem music SHIPPED (2026-11-09, Serious Isaac 0025, conservative)** — `AudioQueue::music_set(id,
> stems, fade)` plays up to 8 Ogg stems (`MUSIC_STEMS_MAX`) on one shared playhead and
> `music_stem_gain(stem, gain, fade)` slews one stem's gain in sim ticks. The sink decodes each stem with its
> own decoder and converter but sends one chunk carrying every stem's samples for the same device frames, so
> the stems cannot drift; a stem whose rate or length differs from the first (or past the 8th) is left out
> with `L5006`. The same id again is a no-op (initial gains apply only at the set's start); a different set
> hard-cuts. `music_play` keeps emitting today's `MusicPlay` (byte-identical stream) and plays as a one-stem
> set. No wire, schema, replay or hash movement; no Cargo change. Crossfade is design only (§Decisions
> Proposed (0025)). By-ear check on a stereo device is owed (`Plan/owed.md`).
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
> copy's pan. No wire, hash or replay movement. By-ear check on a stereo device is owed
> (`Plan/owed.md`).
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

**6. Category is a tag the sink applies gain to, not sim-observed state.** `AudioMessage`/`AudioEvent`
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
- **A `Listener` decoupled from `Camera`.** Decision 3; trigger is a genre that demonstrably needs
  one, not a hypothetical.
- **Runtime-adjustable category gain (a volume slider).** Decision 6; trigger is `Plan/ui-widgets.md`
  landing a settings surface, or a game wanting programmatic ducking badly enough to justify the
  `AudioSink`-side setter early.
- **Loop fade-out on stop.** Shipped: a 10 ms stop fade and gain slew.
- **Stereo pan.** In scope since 2026-11-01: a 3D consumer needs "from the right direction" (vigil
  did not), shipped as the opt-in `AudioPan` (see Status).
- **Reverb, geometry occlusion, HRTF, Doppler.** Out of scope — §6.1 asks for "audible when near".

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
