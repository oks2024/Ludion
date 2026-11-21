//! The sim-side audio seam: the [`AudioQueue`] resource game systems push into, and the
//! frame-stamped log the engine drains it into.
//!
//! Audio in Ludion is *events as plain data*, never sound synthesis on the sim thread. A game system
//! pushes an [`AudioEvent`] (a WAV path + gain), or a [loop](AudioQueue::loop_start) request, into
//! the [`AudioQueue`] world resource; the engine drains that queue at each frame boundary (see
//! [`Engine::advance_live`](crate::Engine)) into two sinks:
//!
//! - an in-engine [frame-stamped log](AudioLogEntry) — queryable in-process, the text-first
//!   verification surface (which sound played, at which frame), with no audio goldens; and
//! - an optional [`crossbeam_channel`] sender a host wires to the playback sink — the audio analog of
//!   the sim→render frame handoff, strictly one-directional (playback never feeds back into the sim).
//!
//! Because the events are a pure function of the simulation and nothing about playback is observable
//! from a tick, a replayed session regenerates the exact same stream, and the `WorldHash` is
//! unaffected (the queue is a resource, excluded from the hash).
//!
//! Music is a few more messages ([`AudioQueue::music_play`], [`AudioQueue::music_set`],
//! [`AudioQueue::music_crossfade`], [`AudioQueue::music_stem_gain`], [`AudioQueue::music_stop`]); the
//! sink streams the Ogg Vorbis track or stems and times its fades.
//!
//! A loop's *playback* state lives only in the sink — the engine holds no table of what is sounding,
//! and a world rebuild sends [`AudioMessage::StopAllLoops`] rather than diffing against one.

use ludion_core::audio::LOW_PASS_OPEN_HZ;
pub use ludion_core::audio::{
    AudioCategory, AudioEvent, AudioMessage, Bus, CategoryGains, DUCK_FLOOR_DB, LoopId, MixLayer,
    VoiceFilter, VoiceId, VoiceSpatial,
};
use ludion_core::codes::code;
use ludion_core::diagnostics::Diagnostic;
use ludion_core::ids::EntityId;
use ludion_core::time::FrameIndex;
use ludion_ecs::World;

/// An ECS resource: the per-tick queue of [`AudioMessage`]s a game system pushes into.
///
/// The engine installs one into every world it builds, so a game system can always
/// `world.resource_mut::<AudioQueue>()` it. The engine drains it once per tick at the frame boundary;
/// between ticks it holds the events emitted by the frame that just ran. It is a plain resource,
/// excluded from the `WorldHash` and from snapshots.
#[derive(Default, Debug)]
pub struct AudioQueue {
    pending: Vec<Queued>,
    /// Sounds queued with a NaN or infinite gain since the last drain, played at gain 0 (`L5005`).
    non_finite: Vec<String>,
    /// The next [`VoiceId`] [`play_on`](AudioQueue::play_on) hands out.
    next_voice: u64,
}

/// One pending queue entry: a message ready to send, a positional one-shot the engine resolves
/// against the listener at the frame boundary, or an attached voice it follows.
#[derive(Debug)]
pub(crate) enum Queued {
    Message(AudioMessage),
    At {
        event: AudioEvent,
        position: [f32; 3],
        range: f32,
    },
    On {
        id: VoiceId,
        /// `None` is a global voice ([`AudioQueue::push_voice`]).
        entity: Option<EntityId>,
        event: AudioEvent,
        range: f32,
        offset_ticks: u32,
    },
    StopVoice {
        id: VoiceId,
        fade_frames: u32,
    },
    DuckStart {
        key: String,
        buses: Vec<Bus>,
        depth_db: f32,
        attack: u32,
    },
    DuckEnd {
        key: String,
        release: u32,
    },
}

impl AudioQueue {
    /// Queues an [`AudioEvent`] to play once. Drained at the end of the current tick.
    pub fn push(&mut self, mut event: AudioEvent) {
        event.gain = self.finite_gain(&event.sound, event.gain);
        self.pending
            .push(Queued::Message(AudioMessage::OneShot(event)));
    }

    /// Convenience: queues a sound by path and gain (`1.0` is unattenuated), played once, on the
    /// [`AudioCategory::Sfx`] bus. An ambient one-shot goes through [`push`](Self::push).
    ///
    /// ```
    /// # use ludion_engine::AudioQueue;
    /// let mut queue = AudioQueue::default();
    /// queue.play("sounds/bounce.wav", 1.0);
    /// ```
    pub fn play(&mut self, sound: impl Into<String>, gain: f32) {
        self.push(AudioEvent::new(sound, gain));
    }

    /// Queues an [`AudioEvent`] to play once from `position` in the world. At the frame boundary its
    /// gain is resolved against the listener (the camera) with the [`AudioEmitter`](crate::AudioEmitter)
    /// falloff — distance per the game's [`AudioDistance`], `range` a hard cutoff. Out of range, below
    /// audibility, or with no camera, nothing is sent or logged (and it is not retried); otherwise it
    /// is an ordinary one-shot at the attenuated gain, and under [`AudioPan::Stereo`] carries the
    /// pan of its side of the listener (the event's own `pan` is overwritten).
    /// [`play_at`](Self::play_at) is the `Sfx` convenience; this is the way to place an ambient
    /// one-shot.
    pub fn push_at(&mut self, mut event: AudioEvent, position: [f32; 3], range: f32) {
        event.gain = self.finite_gain(&event.sound, event.gain);
        self.pending.push(Queued::At {
            event,
            position,
            range,
        });
    }

    /// Convenience: queues a sound by path and gain (`1.0` is unattenuated) to play once from
    /// `position`, on the [`AudioCategory::Sfx`] bus. The gain is resolved at the frame boundary
    /// against the listener (the camera) with the [`AudioEmitter`](crate::AudioEmitter) falloff —
    /// distance per the game's [`AudioDistance`], `range` a hard cutoff. Out of range, below
    /// audibility, or with no camera, nothing is sent or logged; once resolved it is an ordinary
    /// one-shot at the attenuated gain, panned under [`AudioPan::Stereo`]. An ambient one-shot goes
    /// through [`push_at`](Self::push_at).
    ///
    /// ```
    /// # use ludion_engine::AudioQueue;
    /// let mut queue = AudioQueue::default();
    /// queue.play_at("sounds/impact.wav", 1.0, [4.0, 0.0, 2.0], 12.0);
    /// ```
    pub fn play_at(&mut self, sound: impl Into<String>, gain: f32, position: [f32; 3], range: f32) {
        self.push_at(AudioEvent::new(sound, gain), position, range);
    }

    /// Plays `sound` once from `entity` and **follows it**: each tick the engine re-resolves the
    /// voice's gain and pan against the listener (the [`play_at`](Self::play_at) falloff, `range` a
    /// hard cutoff), so a speaker walking away fades out and one walking in is heard. The voice
    /// starts even when `entity` is out of earshot, and ends when the clip does (the engine reads the
    /// WAV's length) or when `entity` despawns (a ~10 ms fade). Returns the id [`voice_stop`] takes;
    /// ids count up from `0` and restart when the world rebuilds.
    ///
    /// ```
    /// # use ludion_ecs::World;
    /// # use ludion_engine::audio::{AudioCategory, AudioQueue, VoiceId};
    /// # let guard = World::new().spawn();
    /// let mut queue = AudioQueue::default();
    /// let first = queue.play_on(guard, "voice/chatter.wav", 0.8, 18.0, AudioCategory::Voice);
    /// let second = queue.play_on(guard, "voice/alert.wav", 1.0, 18.0, AudioCategory::Voice);
    /// assert_eq!((first, second), (VoiceId(0), VoiceId(1)));
    /// queue.voice_stop(first, 6);
    /// ```
    ///
    /// [`voice_stop`]: Self::voice_stop
    pub fn play_on(
        &mut self,
        entity: EntityId,
        sound: impl Into<String>,
        gain: f32,
        range: f32,
        category: AudioCategory,
    ) -> VoiceId {
        let event = AudioEvent {
            category,
            ..AudioEvent::new(sound, gain)
        };
        self.push_on(entity, event, range)
    }

    /// [`play_on`](Self::play_on) for a whole [`AudioEvent`], the way to start an attached voice with
    /// a filter: its `spatial` rides every message the voice sends (the event's `pan` is overwritten
    /// by the follow, as in [`push_at`](Self::push_at)). Returns the id [`voice_stop`] takes.
    ///
    /// ```
    /// # use ludion_ecs::World;
    /// # use ludion_engine::audio::{AudioEvent, AudioQueue, VoiceFilter, VoiceSpatial};
    /// # let guard = World::new().spawn();
    /// let mut queue = AudioQueue::default();
    /// // A radio: only 300 Hz to 3.4 kHz gets through.
    /// let radio = AudioEvent {
    ///     spatial: VoiceSpatial {
    ///         filter: VoiceFilter::band_pass(300.0, 3400.0),
    ///         send: 0.0,
    ///     },
    ///     ..AudioEvent::new("voice/radio.wav", 1.0)
    /// };
    /// let id = queue.push_on(guard, radio, 18.0);
    /// queue.voice_stop(id, 6);
    /// ```
    ///
    /// [`voice_stop`]: Self::voice_stop
    pub fn push_on(&mut self, entity: EntityId, event: AudioEvent, range: f32) -> VoiceId {
        self.push_on_from(entity, event, range, 0)
    }

    /// [`push_on`](Self::push_on) starting `offset_ticks` sim ticks into the clip: the way to resume
    /// a line that was already on air. An offset at or past the clip's length emits nothing (the id
    /// is still returned, and a stop to it is ignored); the voice's tracked end is the clip's
    /// remainder.
    ///
    /// ```
    /// # use ludion_ecs::World;
    /// # use ludion_engine::audio::{AudioEvent, AudioQueue};
    /// # let guard = World::new().spawn();
    /// let mut queue = AudioQueue::default();
    /// // The guard's line, resumed 90 ticks in.
    /// let id = queue.push_on_from(guard, AudioEvent::new("voice/chatter.wav", 1.0), 18.0, 90);
    /// queue.voice_stop(id, 6);
    /// ```
    pub fn push_on_from(
        &mut self,
        entity: EntityId,
        mut event: AudioEvent,
        range: f32,
        offset_ticks: u32,
    ) -> VoiceId {
        let id = VoiceId(self.next_voice);
        self.next_voice += 1;
        event.gain = self.finite_gain(&event.sound, event.gain);
        self.pending.push(Queued::On {
            id,
            entity: Some(entity),
            event,
            range,
            offset_ticks,
        });
        id
    }

    /// Plays `event` once as a **global** voice — no position, no listener falloff — that
    /// [`voice_stop`](Self::voice_stop) can fade out: the stoppable form of [`push`](Self::push).
    /// It starts at the event's own gain and pan, is never resynced, ends when the clip does (the
    /// engine reads the WAV's length), and counts for a [`DuckRule`] on its bus while it sounds.
    /// Returns the id [`voice_stop`] takes; ids share [`push_on`](Self::push_on)'s counter.
    ///
    /// ```
    /// # use ludion_engine::audio::{AudioEvent, AudioQueue, VoiceFilter, VoiceSpatial};
    /// let mut queue = AudioQueue::default();
    /// let radio = AudioEvent {
    ///     spatial: VoiceSpatial {
    ///         filter: VoiceFilter::band_pass(300.0, 3400.0),
    ///         send: 0.0,
    ///     },
    ///     ..AudioEvent::new("voice/radio.wav", 1.0)
    /// };
    /// let id = queue.push_voice(radio);
    /// queue.voice_stop(id, 6);
    /// ```
    ///
    /// [`voice_stop`]: Self::voice_stop
    pub fn push_voice(&mut self, event: AudioEvent) -> VoiceId {
        self.push_voice_from(event, 0)
    }

    /// [`push_voice`](Self::push_voice) starting `offset_ticks` sim ticks into the clip: the way to
    /// resume a line that was already on air. An offset at or past the clip's length emits nothing
    /// (the id is still returned, and a stop to it is ignored); the voice's tracked end is the
    /// clip's remainder.
    ///
    /// ```
    /// # use ludion_engine::audio::{AudioEvent, AudioQueue};
    /// let mut queue = AudioQueue::default();
    /// // A radio line, resumed 90 ticks in.
    /// let id = queue.push_voice_from(AudioEvent::new("voice/radio.wav", 1.0), 90);
    /// queue.voice_stop(id, 6);
    /// ```
    pub fn push_voice_from(&mut self, mut event: AudioEvent, offset_ticks: u32) -> VoiceId {
        let id = VoiceId(self.next_voice);
        self.next_voice += 1;
        event.gain = self.finite_gain(&event.sound, event.gain);
        self.pending.push(Queued::On {
            id,
            entity: None,
            event,
            range: 0.0,
            offset_ticks,
        });
        id
    }

    /// Fades the attached voice `id` out over `fade_frames` sim ticks (at least about 10 ms). A voice
    /// that has already ended, was stopped or never existed is ignored.
    pub fn voice_stop(&mut self, id: VoiceId, fade_frames: u32) {
        self.pending.push(Queued::StopVoice { id, fade_frames });
    }

    /// Starts `sound` looping at `gain` until stopped — a global bed identified by its own path, so
    /// two systems looping one path share one voice.
    ///
    /// Idempotent: calling it again with the same sound updates the running loop's gain in place
    /// rather than restarting it (no click), which makes "recompute the gain and resend it every
    /// tick" the natural calling convention. A loop is [`AudioCategory::Ambient`] — the manual
    /// primitive exists for beds.
    ///
    /// There is no liveness diff behind this: a loop plays until [`loop_stop`](Self::loop_stop) or
    /// until a world rebuild (a `scene.load`, a district switch, a restore) sends the blanket stop,
    /// so a forgotten `loop_stop` sounds forever within one scene.
    ///
    /// ```
    /// # use ludion_engine::AudioQueue;
    /// let mut queue = AudioQueue::default();
    /// queue.loop_start("sounds/wind.wav", 0.4);
    /// ```
    pub fn loop_start(&mut self, sound: impl Into<String>, gain: f32) {
        let sound = sound.into();
        let gain = self.finite_gain(&sound, gain);
        self.pending.push(Queued::Message(AudioMessage::LoopSync {
            id: LoopId::Sound(sound.clone()),
            sound,
            gain,
            pan: 0.0,
            category: AudioCategory::Ambient,
            spatial: VoiceSpatial::default(),
        }));
    }

    /// Updates a running loop's gain: the same idempotent message [`loop_start`](Self::loop_start)
    /// sends, spelled the way an update site reads. Starts the loop if it is not already playing.
    pub fn loop_set_gain(&mut self, sound: impl Into<String>, gain: f32) {
        self.loop_start(sound, gain);
    }

    /// Stops the loop playing `sound`, if any; the sink fades it out over about 10 ms.
    pub fn loop_stop(&mut self, sound: impl Into<String>) {
        self.pending.push(Queued::Message(AudioMessage::LoopStop {
            id: LoopId::Sound(sound.into()),
        }));
    }

    /// Streams the Ogg Vorbis `track` (a content-root-relative path) as the music, looping
    /// gaplessly, fading in over `fade_frames` sim ticks (`0` starts at full `gain`).
    ///
    /// Music is one voice: this plays as a one-stem set. Playing the track already sounding (and
    /// not stopping) only updates its gain, so resending it every tick is harmless; any other track
    /// or [set](Self::music_set) hard-cuts the current one (`music_crossfade` blends) and fades the new one
    /// in. It plays until [`music_stop`](Self::music_stop) or a
    /// world rebuild's blanket stop. The file streams from disk in the sink, never decoded up front;
    /// a missing or non-Vorbis file warns `L5003` and plays nothing.
    ///
    /// ```
    /// # use ludion_engine::AudioQueue;
    /// let mut queue = AudioQueue::default();
    /// queue.music_play("music/theme.ogg", 0.8, 60);
    /// ```
    pub fn music_play(&mut self, track: impl Into<String>, gain: f32, fade_frames: u32) {
        self.push_music(track.into(), gain, true, fade_frames);
    }

    /// Like [`music_play`](Self::music_play), but the track plays once and the music goes idle at
    /// its end instead of looping.
    pub fn music_play_once(&mut self, track: impl Into<String>, gain: f32, fade_frames: u32) {
        self.push_music(track.into(), gain, false, fade_frames);
    }

    /// Fades the music out over `fade_frames` sim ticks (`0` stops it now), then idles. A no-op
    /// when nothing is playing.
    pub fn music_stop(&mut self, fade_frames: u32) {
        self.pending
            .push(Queued::Message(AudioMessage::MusicStop { fade_frames }));
    }

    /// Streams a **set** of Ogg Vorbis stems (content-root-relative paths with their starting
    /// gains) on one shared playhead, looping together gaplessly, fading in over `fade_frames` sim
    /// ticks. Stems must share a sample rate and length; one that does not is left out with `L5006`,
    /// as is any stem past 8.
    ///
    /// The same `id` while it plays (and not stopping) is a no-op, so resending it every tick is
    /// harmless — the initial stem gains apply only when the set starts. Move a stem afterwards with
    /// [`music_stem_gain`](Self::music_stem_gain). Any other set or [track](Self::music_play)
    /// hard-cuts the current music — [`music_crossfade`](Self::music_crossfade) blends instead.
    ///
    /// ```
    /// # use ludion_engine::AudioQueue;
    /// let mut queue = AudioQueue::default();
    /// queue.music_set(
    ///     "dungeon",
    ///     &[("music/base.ogg", 1.0), ("music/perc.ogg", 0.0)],
    ///     60,
    /// );
    /// ```
    pub fn music_set(&mut self, id: impl Into<String>, stems: &[(&str, f32)], fade_frames: u32) {
        let stems = stems
            .iter()
            .map(|&(stem, gain)| (stem.to_string(), self.finite_gain(stem, gain)))
            .collect();
        self.pending.push(Queued::Message(AudioMessage::MusicSet {
            id: id.into(),
            stems,
            fade_frames,
        }));
    }

    /// Blends from the playing music to the set `id` over `fade_frames` sim ticks: the new set
    /// fades in as the live one fades out (equal power). Resending the live `id` is free; with
    /// nothing playing it is a plain fade-in, and a third set mid-crossfade hard-cuts the outgoing
    /// one. A set that cannot open keeps the old music. Stem gains are sanitized like
    /// [`music_set`](Self::music_set).
    ///
    /// ```
    /// # use ludion_engine::AudioQueue;
    /// let mut queue = AudioQueue::default();
    /// queue.music_crossfade("combat", &[("music/combat.ogg", 1.0)], 120);
    /// ```
    pub fn music_crossfade(
        &mut self,
        id: impl Into<String>,
        stems: &[(&str, f32)],
        fade_frames: u32,
    ) {
        let stems = stems
            .iter()
            .map(|&(stem, gain)| (stem.to_string(), self.finite_gain(stem, gain)))
            .collect();
        self.pending
            .push(Queued::Message(AudioMessage::MusicCrossfade {
                id: id.into(),
                stems,
                fade_frames,
            }));
    }

    /// Slews one stem of the live set to `gain` over `fade_frames` sim ticks (`0` still slews
    /// about 10 ms, so it never clicks). During a crossfade it addresses the incoming set. A stem
    /// not in the set is ignored, and resending the same gain every tick is free.
    ///
    /// ```
    /// # use ludion_engine::AudioQueue;
    /// let mut queue = AudioQueue::default();
    /// queue.music_stem_gain("music/perc.ogg", 1.0, 120);
    /// ```
    pub fn music_stem_gain(&mut self, stem: impl Into<String>, gain: f32, fade_frames: u32) {
        let stem = stem.into();
        let gain = self.finite_gain(&stem, gain);
        self.pending
            .push(Queued::Message(AudioMessage::MusicStemGain {
                stem,
                gain,
                fade_frames,
            }));
    }

    /// Slews one layer of `bus`'s runtime gain to `gain` over `fade_frames` sim ticks (`0` still
    /// slews about 10 ms, so it never clicks). The effective gain is `authored x user x game x
    /// pause`, and the master bus multiplies every other bus. A world rebuild resets the `Game`
    /// layer and keeps the `User` layer.
    ///
    /// ```
    /// # use ludion_engine::audio::{AudioQueue, Bus, MixLayer};
    /// let mut queue = AudioQueue::default();
    /// // A settings slider: music at 40%.
    /// queue.bus_gain(Bus::Music, MixLayer::User, 0.4, 0);
    /// ```
    pub fn bus_gain(&mut self, bus: Bus, layer: MixLayer, gain: f32, fade_frames: u32) {
        let gain = self.finite_gain(&format!("bus {bus:?}"), gain);
        self.pending.push(Queued::Message(AudioMessage::BusGain {
            bus,
            layer,
            gain,
            fade_frames,
        }));
    }

    /// Holds `buses` over `fade_frames` sim ticks: each voice on them keeps its cursor (loops and
    /// music included) until [`bus_resume`](Self::bus_resume). An empty slice queues nothing.
    ///
    /// ```
    /// # use ludion_engine::audio::{AudioQueue, Bus};
    /// let mut queue = AudioQueue::default();
    /// // A pause menu: hold the world's sound, keep the interface alive.
    /// queue.bus_pause(&[Bus::Sfx, Bus::Ambient, Bus::Music], 6);
    /// ```
    pub fn bus_pause(&mut self, buses: &[Bus], fade_frames: u32) {
        self.push_pause(buses, true, fade_frames);
    }

    /// Releases `buses` over `fade_frames` sim ticks; held voices continue from their cursors. An
    /// empty slice queues nothing.
    ///
    /// ```
    /// # use ludion_engine::audio::{AudioQueue, Bus};
    /// let mut queue = AudioQueue::default();
    /// queue.bus_resume(&[Bus::Sfx, Bus::Ambient, Bus::Music], 6);
    /// ```
    pub fn bus_resume(&mut self, buses: &[Bus], fade_frames: u32) {
        self.push_pause(buses, false, fade_frames);
    }

    /// Ducks `buses` by `depth_db` (clamped to at most `0.0`) over `attack_ticks` until
    /// [`duck_end`](Self::duck_end) names the same `key`: a sequence's own duck, which multiplies with
    /// any [`DuckRule`] and any other key, floored at 24 dB down. Starting a live key again replaces
    /// it. `Bus::Master` is allowed here, to duck everything. An empty slice queues nothing. A rebuild
    /// drops every manual duck.
    ///
    /// ```
    /// # use ludion_engine::audio::{AudioQueue, Bus};
    /// let mut queue = AudioQueue::default();
    /// queue.duck_start("briefing", &[Bus::Music], -12.0, 10);
    /// queue.duck_end("briefing", 45);
    /// ```
    pub fn duck_start(
        &mut self,
        key: impl Into<String>,
        buses: &[Bus],
        depth_db: f32,
        attack_ticks: u32,
    ) {
        if buses.is_empty() {
            return;
        }
        let key = key.into();
        let depth_db = self.finite_gain(&format!("duck {key}"), depth_db).min(0.0);
        self.pending.push(Queued::DuckStart {
            key,
            buses: buses.to_vec(),
            depth_db,
            attack: attack_ticks,
        });
    }

    /// Releases the duck `key` over `release_ticks`. An unknown key is ignored.
    pub fn duck_end(&mut self, key: impl Into<String>, release_ticks: u32) {
        self.pending.push(Queued::DuckEnd {
            key: key.into(),
            release: release_ticks,
        });
    }

    fn push_pause(&mut self, buses: &[Bus], paused: bool, fade_frames: u32) {
        if buses.is_empty() {
            return;
        }
        self.pending.push(Queued::Message(AudioMessage::BusPause {
            buses: buses.to_vec(),
            paused,
            fade_frames,
        }));
    }

    fn push_music(&mut self, track: String, gain: f32, looping: bool, fade_frames: u32) {
        let gain = self.finite_gain(&track, gain);
        self.pending.push(Queued::Message(AudioMessage::MusicPlay {
            track,
            gain,
            looping,
            fade_frames,
        }));
    }

    /// `gain`, or `0.0` after recording `sound` when it is NaN or infinite. A zero keeps the message
    /// meaningful: a dropped sync would leave a running loop at its last gain and a new loop
    /// undeclared, while at `0.0` the loop stays declared and the next valid gain slews back. A
    /// negative finite gain is left alone: it does not poison a mix.
    fn finite_gain(&mut self, sound: &str, gain: f32) -> f32 {
        if gain.is_finite() {
            return gain;
        }
        self.non_finite.push(sound.to_string());
        0.0
    }

    /// Whether no messages are currently queued.
    pub fn is_empty(&self) -> bool {
        self.pending.is_empty()
    }

    /// Removes and returns every queued entry, leaving the queue empty (the engine's drain path).
    pub(crate) fn take(&mut self) -> Vec<Queued> {
        std::mem::take(&mut self.pending)
    }

    /// Removes and returns the sounds queued with a non-finite gain since the last call (the
    /// engine's drain path reports each once).
    pub(crate) fn take_non_finite(&mut self) -> Vec<String> {
        std::mem::take(&mut self.non_finite)
    }
}

/// Queues a plain one-shot `sound` at `gain` on `world`'s [`AudioQueue`]: the one call for an
/// effect that needs no position and no loop. A positional or looping sound stays on
/// [`AudioQueue`] directly.
///
/// A no-op on a world with no queue. An engine installs the queue on every world it builds, so only
/// a bare `World` in a test can lack one, and a missing sound is not worth a sim panic.
///
/// ```
/// use ludion_ecs::World;
/// use ludion_engine::{AudioQueue, play_sound};
///
/// let mut world = World::new();
/// world.insert_resource(AudioQueue::default());
/// play_sound(&mut world, "sounds/blip.wav", 0.5);
/// // A bare world with no queue is a quiet no-op.
/// play_sound(&mut World::new(), "sounds/blip.wav", 0.5);
/// ```
pub fn play_sound(world: &mut World, sound: impl Into<String>, gain: f32) {
    if let Some(queue) = world.resource_mut::<AudioQueue>() {
        queue.play(sound, gain);
    }
}

/// The gain below which a positional emitter is treated as silent: it syncs nothing, and stops if it
/// was sounding. Small enough to be inaudible, large enough that an emitter exactly at its `range`
/// (falloff `0.0`) never sends a stream of zero-gain syncs.
pub(crate) const AUDIBILITY_EPSILON: f32 = 1e-3;

/// How far an audible emitter's gain may drift from the last one sent before it is sent again: 1% of
/// the sent gain, about 0.09 dB, far under what a listener hears. The sink slews each gain change
/// over a few milliseconds, so the step is smooth.
pub(crate) const RESYNC_GAIN_RATIO: f32 = 0.01;

/// How far an audible emitter's pan may drift from the last one sent before it is sent again: 1% of
/// the stereo field, far under what a listener hears; the sink slews it over a few milliseconds.
pub(crate) const RESYNC_PAN_STEP: f32 = 0.02;

/// How far an occluded sound's low-pass cutoff may drift from the last one sent before it is sent
/// again: 5% of the sent cutoff, under a semitone, a sweep the sink smooths in log2 frequency. A
/// cutoff entering or leaving bypass is always sent.
pub(crate) const RESYNC_CUTOFF_RATIO: f32 = 0.05;

/// Within this distance of the listener a positional sound's pan scales down toward centre, so a
/// sound passing through the listener never jumps from one side to the other.
pub(crate) const PAN_INNER_RADIUS: f32 = 0.5;

/// How a positional sound's distance to the listener is measured: the game authors it once on the
/// builder (`EngineBuilder::audio_distance`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum AudioDistance {
    /// Ground-plane distance (`x`, `z`): height above the listener does not attenuate. The default.
    #[default]
    Planar,
    /// Full three-dimensional distance: a sound overhead is as far as one beside the listener.
    Spatial,
}

/// Whether positional sounds are placed across the stereo field: the game authors it once on the
/// builder (`EngineBuilder::audio_pan`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum AudioPan {
    /// Every positional sound is centred (`pan == 0.0`): the stream of a game that never opts in.
    /// The default.
    #[default]
    Centred,
    /// A positional sound carries a pan from the listener's right and horizontal plane.
    Stereo,
}

/// The last [`AudioMessage::LoopSync`] sent for an emitter in earshot: the filter that keeps a
/// static emitter from resending itself every tick.
pub(crate) struct SentLoop {
    pub(crate) sound: String,
    pub(crate) gain: f32,
    pub(crate) pan: f32,
    pub(crate) category: AudioCategory,
    /// The low-pass cutoff sent: open unless occlusion muffles the loop.
    pub(crate) low_pass_hz: f32,
}

impl SentLoop {
    /// Whether a sync of `sound` at `gain`, `pan` and `low_pass_hz` on `category` would say nothing
    /// new: the same sound and bus, a gain within [`RESYNC_GAIN_RATIO`], a pan within
    /// [`RESYNC_PAN_STEP`] and a cutoff within [`RESYNC_CUTOFF_RATIO`] of the ones sent.
    pub(crate) fn is_current(
        &self,
        sound: &str,
        gain: f32,
        pan: f32,
        low_pass_hz: f32,
        category: AudioCategory,
    ) -> bool {
        self.sound == sound
            && self.category == category
            && resync_is_current(
                (self.gain, self.pan, self.low_pass_hz),
                (gain, pan, low_pass_hz),
            )
    }
}

/// Whether `now` (gain, pan, low-pass cutoff) is within [`RESYNC_GAIN_RATIO`], [`RESYNC_PAN_STEP`]
/// and [`RESYNC_CUTOFF_RATIO`] of the `sent` the sink already has — and both cutoffs bypass or both
/// not: the filter an emitter and an attached voice share. Without occlusion every cutoff is the
/// authored one, so the cutoff test never fires.
pub(crate) fn resync_is_current(sent: (f32, f32, f32), now: (f32, f32, f32)) -> bool {
    let ((sent_gain, sent_pan, sent_cutoff), (gain, pan, cutoff)) = (sent, now);
    (gain - sent_gain).abs() <= sent_gain * RESYNC_GAIN_RATIO
        && (pan - sent_pan).abs() <= RESYNC_PAN_STEP
        && (cutoff >= LOW_PASS_OPEN_HZ) == (sent_cutoff >= LOW_PASS_OPEN_HZ)
        && (cutoff - sent_cutoff).abs() <= sent_cutoff * RESYNC_CUTOFF_RATIO
}

/// An attached voice (`AudioQueue::play_on`) the engine keeps following: the entity it rides, what
/// it was authored at, the last gain and pan sent, and the frame it ends.
pub(crate) struct AttachedVoice {
    /// `None` for a global voice ([`AudioQueue::push_voice`]).
    pub(crate) entity: Option<EntityId>,
    pub(crate) bus: Bus,
    pub(crate) gain: f32,
    pub(crate) range: f32,
    pub(crate) sent_gain: f32,
    pub(crate) sent_pan: f32,
    /// The low-pass cutoff of the last message sent.
    pub(crate) sent_low_pass_hz: f32,
    pub(crate) ends: FrameIndex,
    /// The filter and send every message of this voice carries.
    pub(crate) spatial: VoiceSpatial,
}

/// A rule that ducks buses while a sound on another bus is live
/// ([`EngineBuilder::audio_duck`](crate::EngineBuilder::audio_duck)).
///
/// Liveness is tracked sim-side: a plain one-shot on `when` counts for its WAV's length, an attached
/// voice ([`AudioQueue::play_on`]) while it is tracked and audible. While any source is live the
/// engine sends [`AudioMessage::Duck`] for each bus in `duck`, fading in over `attack_ticks` and back
/// out over `release_ticks`. Rules and manual ducks multiply, floored at
/// [`DUCK_FLOOR_DB`](ludion_core::audio::DUCK_FLOOR_DB).
///
/// `when` is a bus with a tracked end — not [`Bus::Master`] or [`Bus::Music`]; `duck` is non-empty
/// and names neither `Master` nor `when`; `depth_db` is finite and at most `0.0`. Anything else
/// fails the build with `L5100`.
///
/// ```
/// use ludion_engine::audio::{Bus, DuckRule};
///
/// // Music and ambience dip 9 dB under dialogue: the default.
/// assert_eq!(DuckRule::default().when, Bus::Voice);
/// let rule = DuckRule { depth_db: -6.0, ..DuckRule::default() };
/// assert_eq!(rule.duck, &[Bus::Music, Bus::Ambient]);
/// ```
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct DuckRule {
    /// The bus whose sounds trigger the duck.
    pub when: Bus,
    /// The buses ducked while `when` is live.
    pub duck: &'static [Bus],
    /// How far they dip, in dB (`0.0` or below).
    pub depth_db: f32,
    /// Sim ticks to fade down when a source turns on.
    pub attack_ticks: u32,
    /// Sim ticks to fade back when the last source turns off.
    pub release_ticks: u32,
}

impl Default for DuckRule {
    fn default() -> DuckRule {
        DuckRule {
            when: Bus::Voice,
            duck: &[Bus::Music, Bus::Ambient],
            depth_db: -9.0,
            attack_ticks: 6,
            release_ticks: 30,
        }
    }
}

/// Refuses the first rule that cannot run (`L5100`), naming its index and the bad field.
pub(crate) fn validate_duck_rules(rules: &[DuckRule]) -> Result<(), Diagnostic> {
    const SHAPE: &str = "write it as DuckRule { when: Bus::Voice, duck: &[Bus::Music, Bus::Ambient], \
                         depth_db: -9.0, attack_ticks: 6, release_ticks: 30 }: `depth_db` finite and at \
                         most 0, `when` a bus other than Master or Music, `duck` non-empty without Master \
                         or `when`";
    for (index, rule) in rules.iter().enumerate() {
        let problem = if !rule.depth_db.is_finite() || rule.depth_db > 0.0 {
            format!(
                "`depth_db` is {} but must be finite and at most 0",
                rule.depth_db
            )
        } else if matches!(rule.when, Bus::Master | Bus::Music) {
            format!(
                "`when` is {:?}, which has no tracked sound length to time a duck by",
                rule.when
            )
        } else if rule.duck.is_empty() {
            "`duck` is empty, so the rule ducks nothing".to_string()
        } else if rule.duck.contains(&Bus::Master) {
            "`duck` names Master, which would duck every bus".to_string()
        } else if rule.duck.contains(&rule.when) {
            format!("`duck` names `when` ({:?}) itself", rule.when)
        } else {
            continue;
        };
        return Err(Diagnostic::new(
            code::AUDIO_DUCK_RULE_INVALID,
            format!("`audio_duck` rule {index}: {problem}"),
        )
        .with_help(SHAPE));
    }
    Ok(())
}

/// The linear gain of `db` decibels, through the pure-Rust `powf` the sim-side rules require.
pub(crate) fn db_to_gain(db: f32) -> f32 {
    ludion_core::math::powf(10.0, db / 20.0)
}

/// One live [`AudioQueue::duck_start`]: the buses it dips and the linear gain it holds them at.
pub(crate) struct ManualDuck {
    pub(crate) buses: Vec<Bus>,
    pub(crate) gain: f32,
}

/// The gain an [`AudioEmitter`](crate::AudioEmitter) sounds at from `listener`: its authored `gain`
/// attenuated by `clamp(1 − distance/range, 0, 1)` — the [`PointLight`](crate::PointLight) falloff.
/// Under [`AudioDistance::Planar`] (the default) distance is measured on the ground plane (`x`, `z`)
/// only, so height above the ground does not attenuate; under [`AudioDistance::Spatial`] it is the
/// full three-dimensional distance.
///
/// `range` is a hard cutoff: an emitter exactly at `range` is exactly `0.0`, and one beyond it stays
/// `0.0`. A non-positive `range` is rejected at authoring time (`L1136`); should one reach here from
/// code — every emitter placed by `games/vigil` is inserted from Rust, past the deserialize-time
/// check — a non-positive or non-finite `range` is silent rather than the falloff formula's `1.0 −
/// distance/range` saturating to full gain at every distance (a negative `range` makes that quotient
/// negative, and `clamp` cannot rescue a fraction that never crosses zero).
pub(crate) fn emitter_gain(
    gain: f32,
    range: f32,
    listener: [f32; 3],
    emitter: [f32; 3],
    distance: AudioDistance,
) -> f32 {
    if !range.is_finite() || range <= 0.0 {
        return 0.0;
    }
    let dx = emitter[0] - listener[0];
    let dz = emitter[2] - listener[2];
    let squared = match distance {
        AudioDistance::Planar => dx * dx + dz * dz,
        AudioDistance::Spatial => {
            let dy = emitter[1] - listener[1];
            dx * dx + dy * dy + dz * dz
        }
    };
    gain * (1.0 - ludion_core::math::sqrt(squared) / range).clamp(0.0, 1.0)
}

/// The gain a sound placed at `at` reaches `listener` with, or `None` when it is inaudible there —
/// the one audibility rule the AudioEmitter sync and play_at share.
pub(crate) fn audible_gain(
    gain: f32,
    range: f32,
    listener: [f32; 3],
    at: [f32; 3],
    distance: AudioDistance,
) -> Option<f32> {
    let gain = emitter_gain(gain, range, listener, at, distance);
    (gain.is_finite() && gain > AUDIBILITY_EPSILON).then_some(gain)
}

/// The listener's pose for stereo placement: where the ear is, and which way is right and up.
#[derive(Clone, Copy, Debug)]
pub(crate) struct ListenerPose {
    pub(crate) position: [f32; 3],
    pub(crate) right: [f32; 3],
    pub(crate) up: [f32; 3],
    /// The entity the ear is on (the `AudioListener` carrier or the camera), whose hierarchy never
    /// occludes.
    pub(crate) entity: Option<EntityId>,
}

fn dot(a: [f32; 3], b: [f32; 3]) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

/// Where a sound at `at` sits across the listener's stereo field, `-1.0` (left) to `1.0` (right).
///
/// The offset is projected onto the listener's horizontal plane (perpendicular to `up`), so height
/// never moves a sound sideways: dead ahead, behind and straight overhead are exactly `0.0`. Inside
/// [`PAN_INNER_RADIUS`] the side is scaled down linearly by `len / PAN_INNER_RADIUS`, so a sound on
/// top of the listener is centred and the pan never jumps as it passes through them.
pub(crate) fn stereo_pan(listener: &ListenerPose, at: [f32; 3]) -> f32 {
    let d = [
        at[0] - listener.position[0],
        at[1] - listener.position[1],
        at[2] - listener.position[2],
    ];
    let up = dot(d, listener.up);
    let p = [
        d[0] - listener.up[0] * up,
        d[1] - listener.up[1] * up,
        d[2] - listener.up[2] * up,
    ];
    let len = ludion_core::math::sqrt(dot(p, p));
    if len == 0.0 {
        return 0.0;
    }
    (dot(p, listener.right) / len * (len / PAN_INNER_RADIUS).min(1.0)).clamp(-1.0, 1.0)
}

/// The gain and pan a sound placed at `at` reaches `listener` with, or `None` when it is inaudible
/// there: [`audible_gain`] plus, under [`AudioPan::Stereo`], the [`stereo_pan`]. A
/// [`Centred`](AudioPan::Centred) game gets a literal `0.0` with no pan computed.
pub(crate) fn audible_voice(
    gain: f32,
    range: f32,
    listener: &ListenerPose,
    at: [f32; 3],
    distance: AudioDistance,
    pan: AudioPan,
) -> Option<(f32, f32)> {
    let gain = audible_gain(gain, range, listener.position, at, distance)?;
    let pan = match pan {
        AudioPan::Centred => 0.0,
        AudioPan::Stereo => stereo_pan(listener, at),
    };
    Some((gain, pan))
}

/// One drained [`AudioMessage`], stamped with the frame during which it was emitted.
///
/// The engine's audio log is a `Vec` of these, in emission order — the exact, replayable stream a
/// test asserts on. The frame stamp is the emitting frame for a game system's message, and the
/// current frame for the blanket stop a world rebuild sends.
#[derive(Clone, Debug, PartialEq)]
pub struct AudioLogEntry {
    /// The frame during which this message was emitted.
    pub frame: FrameIndex,
    /// What was asked for: a one-shot, a loop sync or stop, or the blanket stop of a world rebuild.
    pub message: AudioMessage,
}

impl AudioLogEntry {
    /// The sound this entry names, if it names one — a one-shot, a loop sync or a music play (its
    /// track). A stop names a [`LoopId`], not a path, and the blanket and music stops name nothing.
    pub fn sound(&self) -> Option<&str> {
        match &self.message {
            AudioMessage::OneShot(event) => Some(&event.sound),
            AudioMessage::LoopSync { sound, .. } => Some(sound),
            AudioMessage::MusicPlay { track, .. } => Some(track),
            AudioMessage::MusicSet { id, .. } | AudioMessage::MusicCrossfade { id, .. } => Some(id),
            AudioMessage::MusicStemGain { stem, .. } => Some(stem),
            AudioMessage::VoiceStart { sound, .. } => Some(sound),
            AudioMessage::LoopStop { .. }
            | AudioMessage::StopAllLoops
            | AudioMessage::MusicStop { .. }
            | AudioMessage::BusGain { .. }
            | AudioMessage::Duck { .. }
            | AudioMessage::ZoneGain { .. }
            | AudioMessage::ReverbSet { .. }
            | AudioMessage::BusPause { .. }
            | AudioMessage::VoiceSync { .. }
            | AudioMessage::VoiceStop { .. } => None,
        }
    }

    /// The linear gain this entry carries, if it carries one (a one-shot, a loop sync or a music play).
    pub fn gain(&self) -> Option<f32> {
        match &self.message {
            AudioMessage::OneShot(event) => Some(event.gain),
            AudioMessage::LoopSync { gain, .. }
            | AudioMessage::MusicPlay { gain, .. }
            | AudioMessage::MusicStemGain { gain, .. }
            | AudioMessage::BusGain { gain, .. }
            | AudioMessage::Duck { gain, .. }
            | AudioMessage::ZoneGain { gain, .. }
            | AudioMessage::VoiceStart { gain, .. }
            | AudioMessage::VoiceSync { gain, .. } => Some(*gain),
            AudioMessage::LoopStop { .. }
            | AudioMessage::StopAllLoops
            | AudioMessage::VoiceStop { .. }
            | AudioMessage::BusPause { .. }
            | AudioMessage::MusicSet { .. }
            | AudioMessage::MusicCrossfade { .. }
            | AudioMessage::ReverbSet { .. }
            | AudioMessage::MusicStop { .. } => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The gain every queued message carries, in order.
    fn queued_gains(queue: &mut AudioQueue) -> Vec<f32> {
        queue
            .take()
            .into_iter()
            .map(|item| match item {
                Queued::Message(AudioMessage::OneShot(event)) => event.gain,
                Queued::Message(AudioMessage::LoopSync { gain, .. }) => gain,
                Queued::Message(AudioMessage::MusicPlay { gain, .. }) => gain,
                Queued::At { event, .. } => event.gain,
                other => panic!("no gain: {other:?}"),
            })
            .collect()
    }

    /// A NaN or infinite gain is queued as 0 by every verb and recorded; a finite one is untouched.
    #[test]
    fn a_non_finite_gain_is_queued_as_zero_and_recorded() {
        for bad in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY] {
            let mut queue = AudioQueue::default();
            queue.play("a.wav", bad);
            queue.play_at("b.wav", bad, [0.0; 3], 5.0);
            queue.loop_start("c.wav", bad);
            queue.loop_set_gain("d.wav", bad);
            queue.music_play("e.ogg", bad, 0);
            queue.music_play_once("f.ogg", bad, 0);
            assert_eq!(queue.take_non_finite().len(), 6);
            assert_eq!(queued_gains(&mut queue), [0.0; 6]);
        }
        let mut queue = AudioQueue::default();
        queue.play("a.wav", 0.25);
        queue.loop_start("c.wav", -0.5);
        queue.music_play("e.ogg", 7.0, 0);
        assert_eq!(queued_gains(&mut queue), [0.25, -0.5, 7.0]);
        assert!(queue.take_non_finite().is_empty());
    }

    /// A non-finite bus gain is queued as 0 and recorded under the bus's name; a finite one passes.
    #[test]
    fn a_non_finite_bus_gain_is_queued_as_zero_and_recorded() {
        let mut queue = AudioQueue::default();
        queue.bus_gain(Bus::Music, MixLayer::User, f32::NAN, 3);
        queue.bus_gain(Bus::Sfx, MixLayer::Game, 0.5, 0);
        assert_eq!(queue.take_non_finite(), ["bus Music"]);
        let messages = queue.take();
        let [
            Queued::Message(AudioMessage::BusGain { gain: first, .. }),
            Queued::Message(AudioMessage::BusGain { gain: second, .. }),
        ] = messages.as_slice()
        else {
            panic!("two bus gains, got {messages:?}");
        };
        assert_eq!((*first, *second), (0.0, 0.5));
    }

    /// A NaN duck depth is recorded and queues `0.0` dB, a boost is clamped to `0.0`, and an empty
    /// bus list queues nothing.
    #[test]
    fn a_manual_duck_depth_is_finite_and_never_a_boost() {
        let mut queue = AudioQueue::default();
        queue.duck_start("a", &[Bus::Music], f32::NAN, 1);
        queue.duck_start("b", &[Bus::Music], 6.0, 1);
        queue.duck_start("c", &[], -6.0, 1);
        assert_eq!(queue.take_non_finite(), ["duck a"]);
        let queued = queue.take();
        let [
            Queued::DuckStart {
                depth_db: first, ..
            },
            Queued::DuckStart {
                depth_db: second, ..
            },
        ] = queued.as_slice()
        else {
            panic!("two manual ducks, got {queued:?}");
        };
        assert_eq!((*first, *second), (0.0, 0.0));
    }

    /// A rule that cannot run is `L5100`, naming its index.
    #[test]
    fn an_invalid_duck_rule_is_l5100() {
        let valid = DuckRule::default();
        assert!(validate_duck_rules(&[valid, valid]).is_ok());
        let bad = [
            DuckRule {
                depth_db: 3.0,
                ..valid
            },
            DuckRule {
                depth_db: f32::NAN,
                ..valid
            },
            DuckRule {
                when: Bus::Master,
                ..valid
            },
            DuckRule {
                when: Bus::Music,
                duck: &[Bus::Sfx],
                ..valid
            },
            DuckRule { duck: &[], ..valid },
            DuckRule {
                duck: &[Bus::Master],
                ..valid
            },
            DuckRule {
                duck: &[Bus::Voice],
                ..valid
            },
        ];
        for rule in bad {
            let error = validate_duck_rules(&[valid, rule]).unwrap_err();
            assert_eq!(error.code, code::AUDIO_DUCK_RULE_INVALID, "{rule:?}");
            assert!(error.message.contains("rule 1"), "{}", error.message);
        }
    }

    /// Pausing or resuming no buses is not a message.
    #[test]
    fn an_empty_pause_queues_nothing() {
        let mut queue = AudioQueue::default();
        queue.bus_pause(&[], 4);
        queue.bus_resume(&[], 4);
        assert!(queue.is_empty());
        queue.bus_pause(&[Bus::Sfx], 4);
        assert!(!queue.is_empty());
    }

    /// The set verbs guard every stem gain the same way.
    #[test]
    fn a_non_finite_stem_gain_is_queued_as_zero_and_recorded() {
        let mut queue = AudioQueue::default();
        queue.music_set("s", &[("a.ogg", f32::NAN), ("b.ogg", 0.5)], 0);
        queue.music_stem_gain("b.ogg", f32::INFINITY, 0);
        assert_eq!(queue.take_non_finite(), ["a.ogg", "b.ogg"]);
        let messages = queue.take();
        let [
            Queued::Message(AudioMessage::MusicSet { stems, .. }),
            Queued::Message(AudioMessage::MusicStemGain { gain, .. }),
        ] = messages.as_slice()
        else {
            panic!("two music messages, got {messages:?}");
        };
        assert_eq!(
            stems,
            &[("a.ogg".to_string(), 0.0), ("b.ogg".to_string(), 0.5)]
        );
        assert_eq!(*gain, 0.0);

        queue.music_crossfade("c", &[("c.ogg", f32::NAN), ("d.ogg", 0.5)], 30);
        assert_eq!(queue.take_non_finite(), ["c.ogg"]);
        let messages = queue.take();
        let [
            Queued::Message(AudioMessage::MusicCrossfade {
                id,
                stems,
                fade_frames,
            }),
        ] = messages.as_slice()
        else {
            panic!("one crossfade, got {messages:?}");
        };
        assert_eq!((id.as_str(), *fade_frames), ("c", 30));
        assert_eq!(
            stems,
            &[("c.ogg".to_string(), 0.0), ("d.ogg".to_string(), 0.5)]
        );
        let entry = |message| AudioLogEntry {
            frame: FrameIndex(0),
            message,
        };
        let set = entry(AudioMessage::MusicSet {
            id: "s".into(),
            stems: vec![],
            fade_frames: 0,
        });
        assert_eq!((set.sound(), set.gain()), (Some("s"), None));
        let crossfade = entry(AudioMessage::MusicCrossfade {
            id: "c".into(),
            stems: vec![],
            fade_frames: 0,
        });
        assert_eq!((crossfade.sound(), crossfade.gain()), (Some("c"), None));
        let stem = entry(AudioMessage::MusicStemGain {
            stem: "b.ogg".into(),
            gain: 0.25,
            fade_frames: 0,
        });
        assert_eq!((stem.sound(), stem.gain()), (Some("b.ogg"), Some(0.25)));
    }

    /// `play` is the sound-effect verb: what it queues rides the `Sfx` bus, whatever the sink's
    /// per-category master gains are.
    #[test]
    fn play_queues_a_one_shot_on_the_sfx_bus() {
        let mut queue = AudioQueue::default();
        queue.play("shot.wav", 0.5);
        let messages = queue.take();
        let [Queued::Message(AudioMessage::OneShot(event))] = messages.as_slice() else {
            panic!("play queues exactly one one-shot, got {messages:?}");
        };
        assert_eq!(event.category, AudioCategory::Sfx);
    }

    #[test]
    fn a_resync_needs_a_gain_move_past_one_percent_or_a_new_sound_or_bus() {
        let sent = SentLoop {
            sound: "bell.wav".to_string(),
            gain: 0.6,
            pan: 0.0,
            category: AudioCategory::Ambient,
            low_pass_hz: LOW_PASS_OPEN_HZ,
        };
        let open = LOW_PASS_OPEN_HZ;
        assert!(sent.is_current("bell.wav", 0.605, 0.0, open, AudioCategory::Ambient));
        assert!(!sent.is_current("bell.wav", 0.61, 0.0, open, AudioCategory::Ambient));
        assert!(!sent.is_current("gong.wav", 0.6, 0.0, open, AudioCategory::Ambient));
        assert!(!sent.is_current("bell.wav", 0.6, 0.0, open, AudioCategory::Sfx));
    }

    #[test]
    fn a_resync_needs_a_pan_move_past_the_step() {
        let sent = SentLoop {
            sound: "bell.wav".to_string(),
            gain: 0.6,
            pan: 0.3,
            category: AudioCategory::Ambient,
            low_pass_hz: LOW_PASS_OPEN_HZ,
        };
        let open = LOW_PASS_OPEN_HZ;
        assert!(sent.is_current(
            "bell.wav",
            0.6,
            0.3 + RESYNC_PAN_STEP / 2.0,
            open,
            AudioCategory::Ambient
        ));
        assert!(sent.is_current(
            "bell.wav",
            0.6,
            0.3 - RESYNC_PAN_STEP / 2.0,
            open,
            AudioCategory::Ambient
        ));
        assert!(!sent.is_current("bell.wav", 0.6, 0.33, open, AudioCategory::Ambient));
        assert!(!sent.is_current("bell.wav", 0.6, 0.0, open, AudioCategory::Ambient));
    }

    #[test]
    fn a_resync_needs_a_cutoff_move_past_five_percent_or_a_bypass_change() {
        let sent = SentLoop {
            sound: "bell.wav".to_string(),
            gain: 0.6,
            pan: 0.0,
            category: AudioCategory::Ambient,
            low_pass_hz: 1200.0,
        };
        let at = |hz: f32| sent.is_current("bell.wav", 0.6, 0.0, hz, AudioCategory::Ambient);
        assert!(at(1200.0 * 1.04));
        assert!(!at(1200.0 * 1.06));
        assert!(!at(LOW_PASS_OPEN_HZ));
        // Open to open is current whatever the ratio.
        let open = SentLoop {
            low_pass_hz: LOW_PASS_OPEN_HZ,
            ..sent
        };
        assert!(open.is_current(
            "bell.wav",
            0.6,
            0.0,
            LOW_PASS_OPEN_HZ,
            AudioCategory::Ambient
        ));
    }

    /// The falloff at the four distances that pin its shape: full gain on top of the emitter, half at
    /// half `range`, exactly zero at `range`, and still zero beyond it.
    #[test]
    fn emitter_gain_falls_off_linearly_to_zero_at_range() {
        let at = |x: f32| emitter_gain(0.8, 10.0, [0.0; 3], [x, 0.0, 0.0], AudioDistance::Planar);
        assert_eq!(at(0.0), 0.8);
        assert_eq!(at(5.0), 0.4);
        assert_eq!(at(10.0), 0.0);
        assert_eq!(at(25.0), 0.0);
    }

    /// A non-positive or non-finite `range` is silent at every distance, including on top of the
    /// emitter — never the falloff formula's clamp saturating to full gain (the historical bug: a
    /// negative range never stopped sounding). `L1136` catches this at deserialize time; this is the
    /// code-inserted path (`games/vigil` builds every `AudioEmitter` in Rust) reaching the same guard.
    #[test]
    fn emitter_gain_is_silent_for_a_non_positive_or_non_finite_range() {
        let at_listener =
            |range: f32| emitter_gain(1.0, range, [0.0; 3], [0.0; 3], AudioDistance::Planar);
        assert_eq!(at_listener(-1.0), 0.0);
        assert_eq!(at_listener(0.0), 0.0);
        assert_eq!(at_listener(f32::NAN), 0.0);
        assert_eq!(at_listener(f32::INFINITY), 0.0);
        assert_eq!(at_listener(f32::NEG_INFINITY), 0.0);
    }

    /// The shared rule: audible inside `range`, `None` at `range` and for a non-positive `range`.
    #[test]
    fn audible_gain_is_none_when_inaudible() {
        let at = |x: f32, range: f32| {
            audible_gain(1.0, range, [0.0; 3], [x, 0.0, 0.0], AudioDistance::Planar)
        };
        assert_eq!(at(4.0, 10.0), Some(0.6));
        assert_eq!(at(10.0, 10.0), None);
        assert_eq!(at(0.0, 0.0), None);
        assert_eq!(at(0.0, -1.0), None);
    }

    /// Planar distance is measured on the ground plane only: height does not attenuate. Spatial
    /// distance counts it.
    #[test]
    fn planar_distance_ignores_height_and_spatial_counts_it() {
        let gain = |at: [f32; 3], distance| emitter_gain(1.0, 10.0, [0.0; 3], at, distance);
        assert_eq!(
            gain([0.0, 40.0, 5.0], AudioDistance::Planar),
            gain([0.0, 0.0, 5.0], AudioDistance::Planar),
        );
        assert_eq!(gain([0.0, 5.0, 0.0], AudioDistance::Planar), 1.0);
        assert_eq!(gain([0.0, 5.0, 0.0], AudioDistance::Spatial), 0.5);
        assert_eq!(gain([0.0, 40.0, 5.0], AudioDistance::Spatial), 0.0);
    }

    /// A listener at the origin looking down `-Z`: right is `+X`, up is `+Y`.
    fn facing_north() -> ListenerPose {
        ListenerPose {
            position: [0.0; 3],
            right: [1.0, 0.0, 0.0],
            up: [0.0, 1.0, 0.0],
            entity: None,
        }
    }

    #[test]
    fn a_sound_to_the_right_pans_right_and_to_the_left_pans_left() {
        let listener = facing_north();
        assert!(stereo_pan(&listener, [4.0, 0.0, -1.0]) > 0.9);
        assert!(stereo_pan(&listener, [-4.0, 0.0, -1.0]) < -0.9);
        let turned = ListenerPose {
            right: [-1.0, 0.0, 0.0],
            ..listener
        };
        assert!(
            stereo_pan(&turned, [4.0, 0.0, -1.0]) < -0.9,
            "a half turn flips the sign"
        );
    }

    #[test]
    fn dead_ahead_behind_and_overhead_are_exactly_centre() {
        let listener = facing_north();
        for at in [
            [0.0, 0.0, -5.0],
            [0.0, 0.0, 5.0],
            [0.0, 7.0, 0.0],
            [0.0, 7.0, -3.0],
        ] {
            assert_eq!(stereo_pan(&listener, at), 0.0, "{at:?}");
        }
    }

    #[test]
    fn the_inner_radius_scales_the_pan_toward_centre() {
        let listener = facing_north();
        assert_eq!(stereo_pan(&listener, [0.0; 3]), 0.0);
        assert_eq!(
            stereo_pan(&listener, [PAN_INNER_RADIUS * 0.5, 0.0, 0.0]),
            0.5
        );
        assert_eq!(stereo_pan(&listener, [PAN_INNER_RADIUS, 0.0, 0.0]), 1.0);
        assert_eq!(
            stereo_pan(&listener, [-PAN_INNER_RADIUS * 3.0, 0.0, 0.0]),
            -1.0
        );
    }

    #[test]
    fn a_centred_game_resolves_a_literal_zero_pan() {
        let listener = facing_north();
        let at = [4.0, 0.0, 0.0];
        let centred = audible_voice(
            1.0,
            10.0,
            &listener,
            at,
            AudioDistance::Planar,
            AudioPan::Centred,
        );
        let stereo = audible_voice(
            1.0,
            10.0,
            &listener,
            at,
            AudioDistance::Planar,
            AudioPan::Stereo,
        );
        assert_eq!(
            centred.map(|(_, pan)| pan.to_bits()),
            Some(0.0f32.to_bits())
        );
        assert_eq!(stereo.map(|(_, pan)| pan), Some(1.0));
        assert_eq!(centred.map(|(gain, _)| gain), stereo.map(|(gain, _)| gain));
        let far = [40.0, 0.0, 0.0];
        assert_eq!(
            audible_voice(
                1.0,
                10.0,
                &listener,
                far,
                AudioDistance::Planar,
                AudioPan::Stereo
            ),
            None
        );
    }
}
