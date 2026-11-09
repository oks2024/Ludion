//! The plain-data audio types shared across the sim→playback boundary.
//!
//! An [`AudioEvent`] is a *request to play a sound* — nothing more: a content-root-relative WAV path,
//! a linear gain and a stereo pan. An [`AudioMessage`] is the full vocabulary of that boundary: a
//! one-shot, or the declarative sync/stop of a [loop](LoopId). Neither carries a wall clock, a device handle, or
//! decoded samples, so both are safe to push from a deterministic game system (the engine's
//! `AudioQueue` resource holds these) and to hand across a channel to the out-of-sim playback sink
//! (`ludion-audio`), which is the one crate that ever resolves the path and touches an audio device.
//!
//! They live here, in the dependency-light core, precisely because both sides need the types: the
//! engine *emits* them and the sink *receives* them, and neither may depend on the other (the engine
//! never gains a playback dependency; the sink stays a lean leaf). Plain data in `ludion-core` is the
//! shared vocabulary both agree on.

/// A request to play a sound: a content-root-relative WAV path and a linear gain multiplier.
///
/// Game systems construct these (via the engine's `AudioQueue`); the engine drains them at each
/// frame boundary into a frame-stamped log and, when a host wired one up, a channel to the audio
/// sink. Because it is plain data with no side effects, the exact stream of events a run produces is
/// a pure function of the simulation — so a replayed session regenerates it identically.
///
/// # Example
///
/// ```
/// use ludion_core::audio::{AudioCategory, AudioEvent};
///
/// // "Play `sfx/bounce.wav` at half volume." — a request, not playback.
/// let event = AudioEvent::new("sfx/bounce.wav", 0.5);
/// assert_eq!(event.sound, "sfx/bounce.wav");
/// assert_eq!(event.gain, 0.5);
/// assert_eq!(event.category, AudioCategory::Sfx);
/// ```
#[derive(Clone, Debug, PartialEq)]
pub struct AudioEvent {
    /// The sound to play, as a path relative to the content root (the directory of the scene the
    /// engine was started with). The sink resolves and decodes it as a WAV file.
    pub sound: String,
    /// A linear gain multiplier applied to the sound's samples (`1.0` leaves it unchanged).
    pub gain: f32,
    /// Stereo position: `-1.0` hard left, `0.0` centre, `1.0` hard right.
    pub pan: f32,
    /// Which mixdown bus the sound belongs to.
    pub category: AudioCategory,
}

impl AudioEvent {
    /// A centred one-shot on the [`Sfx`](AudioCategory::Sfx) bus — what a game sound effect is. An
    /// ambient one-shot is a struct literal, spelling its category and pan out.
    pub fn new(sound: impl Into<String>, gain: f32) -> AudioEvent {
        AudioEvent {
            sound: sound.into(),
            gain,
            pan: 0.0,
            category: AudioCategory::Sfx,
        }
    }
}

/// Which mixdown bus a sound belongs to.
///
/// A tag the playback sink reads, never simulation state: a per-category master gain is a mixdown
/// concern (which bus is how loud), the same category of thing as which device to open, so it never
/// reaches a `WorldHash`.
///
/// It serializes because a scene authors it — the engine's `AudioEmitter` component carries one, as
/// the lowercase variant name. Its JSON Schema is hand-written on the component's field: this crate
/// has no `schemars` dependency (the `Parent`/`EntityId` treatment), so a schema for it must be kept
/// matching this serde shape by hand.
#[derive(
    Clone, Copy, Debug, Default, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize,
)]
#[serde(rename_all = "snake_case")]
pub enum AudioCategory {
    /// A game sound effect — the default, and what a one-shot carries.
    #[default]
    Sfx,
    /// A sustained ambient bed: a drone, wind, a tolling bell.
    Ambient,
}

/// The master gain of each mixdown bus: how loud sound effects are against ambience.
///
/// The game authors it once (`EngineBuilder::category_gains`) and the host hands it to the playback
/// sink, which multiplies every voice by its own bus's value while mixing. It is a mixdown decision,
/// the same category of thing as which device to open — never simulation state: it reaches no
/// `WorldHash`, no snapshot and no audio log, so changing a value here cannot move a replay. There
/// is no runtime setter: a player-facing volume slider would be a separate surface.
///
/// It lives here, beside [`AudioCategory`], because the engine (which stores the authored value) and
/// the sink (which applies it) both need the type and neither may depend on the other.
///
/// # Example
///
/// ```
/// use ludion_core::audio::{AudioCategory, CategoryGains};
///
/// // "Ambience sits under the effects."
/// let gains = CategoryGains { sfx: 1.0, ambient: 0.6, music: 0.8 };
/// assert_eq!(gains.gain_for(AudioCategory::Ambient), 0.6);
/// // The default is the identity: the authored gains, untouched.
/// assert_eq!(CategoryGains::default().gain_for(AudioCategory::Ambient), 1.0);
/// ```
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CategoryGains {
    /// The master gain of the sound-effect bus.
    pub sfx: f32,
    /// The master gain of the ambient bus.
    pub ambient: f32,
    /// The master gain of the music bus. Music only travels in [`AudioMessage::MusicPlay`],
    /// [`AudioMessage::MusicSet`] and their companions, so its bus is implied: there is no
    /// [`AudioCategory`] for it and [`gain_for`](Self::gain_for) never returns it.
    pub music: f32,
}

impl Default for CategoryGains {
    /// Identity: every bus at `1.0`, so the mixdown is exactly the sum of the authored gains.
    fn default() -> CategoryGains {
        CategoryGains {
            sfx: 1.0,
            ambient: 1.0,
            music: 1.0,
        }
    }
}

impl CategoryGains {
    /// The master gain a voice on `category` is multiplied by while mixing (the music bus is read
    /// from [`music`](Self::music) directly).
    pub fn gain_for(&self, category: AudioCategory) -> f32 {
        match category {
            AudioCategory::Sfx => self.sfx,
            AudioCategory::Ambient => self.ambient,
        }
    }
}

/// How many voices may sound at once: the playback sink's polyphony policy.
///
/// At most [`per_sound`](Self::per_sound) live one-shots of one sound and at most
/// [`total`](Self::total) live voices overall; when either is full the oldest one-shot is stolen
/// with a short fade. A game overrides the defaults in the `[audio]` table of `ludion.toml`. Like
/// [`CategoryGains`] it is a mixdown decision, never simulation state, and lives here because the
/// engine (which validates the manifest value) and the sink (which applies it) both need the type
/// and neither may depend on the other.
///
/// # Example
///
/// ```
/// use ludion_core::audio::VoiceCaps;
///
/// // The default: four of one sound, sixty-four in all.
/// assert_eq!(VoiceCaps::default(), VoiceCaps { per_sound: 4, total: 64 });
/// // The upper bound a manifest may ask for.
/// assert_eq!(VoiceCaps::MAX_TOTAL, 256);
/// ```
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct VoiceCaps {
    /// The most live one-shots of a single sound.
    pub per_sound: usize,
    /// The most live voices of every kind.
    pub total: usize,
}

impl VoiceCaps {
    /// The largest `total` a manifest may ask for: the mixer reserves its voices up front.
    pub const MAX_TOTAL: usize = 256;
}

impl Default for VoiceCaps {
    /// Four live one-shots per sound, sixty-four voices in total.
    fn default() -> VoiceCaps {
        VoiceCaps {
            per_sound: 4,
            total: 64,
        }
    }
}

/// The identity of a looping voice: what an [`AudioMessage::LoopSync`] updates in place and an
/// [`AudioMessage::LoopStop`] silences.
///
/// Two constructors because the two uses need different uniqueness. A global bed is a singleton, so
/// its sound path already names it. A positional emitter is not: several towers can loop one WAV
/// independently, and only the owning entity tells them apart.
///
/// # Example
///
/// ```
/// use ludion_core::audio::LoopId;
///
/// // Two beds on the same path are the same loop; the path is the identity.
/// assert_eq!(LoopId::Sound("sfx/wind.wav".to_string()), LoopId::Sound("sfx/wind.wav".to_string()));
/// ```
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum LoopId {
    /// A singleton bed, identified by its sound path.
    Sound(String),
    /// A per-entity emitter, identified by its owner.
    Entity(crate::ids::EntityId),
}

/// One request on the sim→playback channel: play a sound once, or keep a loop in sync.
///
/// [`LoopSync`](AudioMessage::LoopSync) is declarative and idempotent — resending it with the same
/// `id` and `sound` only updates the running voice's gain, so "recompute the gain, resend it" is the
/// calling convention rather than hand-diffing at each call site. A new `id`, or a different `sound`
/// under an existing one, starts a fresh voice from the top.
///
/// # Example
///
/// ```
/// use ludion_core::audio::{AudioCategory, AudioMessage, LoopId};
///
/// // "Keep `sfx/wind.wav` looping at 0.4." — the same message that starts it and updates it.
/// let message = AudioMessage::LoopSync {
///     id: LoopId::Sound("sfx/wind.wav".to_string()),
///     sound: "sfx/wind.wav".to_string(),
///     gain: 0.4,
///     pan: 0.0,
///     category: AudioCategory::Ambient,
/// };
/// assert_ne!(message, AudioMessage::StopAllLoops);
/// ```
#[derive(Clone, Debug, PartialEq)]
pub enum AudioMessage {
    /// Play a sound once, then retire it.
    OneShot(AudioEvent),
    /// Start `sound` looping under `id`, or update the gain of the loop already running there.
    LoopSync {
        /// Which loop this addresses.
        id: LoopId,
        /// The sound to loop, as a content-root-relative WAV path.
        sound: String,
        /// A linear gain multiplier applied to the loop's samples.
        gain: f32,
        /// Stereo position: `-1.0` hard left, `0.0` centre, `1.0` hard right.
        pan: f32,
        /// Which mixdown bus the loop belongs to.
        category: AudioCategory,
    },
    /// Silence the loop running under `id`, if any; the sink fades it out over about 10 ms.
    LoopStop {
        /// Which loop to silence.
        id: LoopId,
    },
    /// Silence every running loop and the music; each fades out over about 10 ms. A loop's playback
    /// state lives only in the sink, so a rebuilt world has nothing to diff against — the blanket
    /// stop is what keeps a loop or a track from outliving the scene that started it.
    StopAllLoops,
    /// Stream `track` as the music, or update the gain of the track already playing. Music is one
    /// voice: the same track again (not stopping) only takes the new `gain`, so resending it every
    /// tick is harmless; any other track hard-cuts the current one and fades the new one in.
    MusicPlay {
        /// The Ogg Vorbis track, as a content-root-relative path.
        track: String,
        /// A linear gain multiplier, applied before the music bus master.
        gain: f32,
        /// Whether the track restarts gaplessly at its end instead of ending.
        looping: bool,
        /// The fade-in length in sim ticks (`0` starts at full gain); the sink converts it to
        /// device frames with the tick length.
        fade_frames: u32,
    },
    /// Fade the music out over `fade_frames` sim ticks (`0` stops it now), then idle.
    MusicStop {
        /// The fade-out length in sim ticks.
        fade_frames: u32,
    },
    /// Stream a **set** of up to [`MUSIC_STEMS_MAX`] Ogg stems on one shared playhead, looping, each
    /// at its own gain. The same `id` again while it plays (not stopping) is a no-op, so resending
    /// it every tick is harmless — the initial stem gains apply only when the set starts, and later
    /// ones move through [`MusicStemGain`](AudioMessage::MusicStemGain). Any other set or track
    /// hard-cuts the current music and fades the new one in.
    MusicSet {
        /// The set's identity: what makes a resend a no-op.
        id: String,
        /// Each stem's content-root-relative Ogg Vorbis path and its starting gain.
        stems: Vec<(String, f32)>,
        /// The fade-in length in sim ticks (`0` starts at full level).
        fade_frames: u32,
    },
    /// Slew one stem of the playing set to `gain` over `fade_frames` sim ticks (`0` still slews
    /// about 10 ms, click-free). A stem not in the set is ignored; resending the same gain is free.
    MusicStemGain {
        /// The stem's path, as named in the set.
        stem: String,
        /// The target linear gain.
        gain: f32,
        /// The slew length in sim ticks.
        fade_frames: u32,
    },
}

/// The most stems a [`AudioMessage::MusicSet`] plays; stems past it are dropped with `L5006`.
pub const MUSIC_STEMS_MAX: usize = 8;
