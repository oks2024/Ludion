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
    /// The per-voice filter and reverb send; the default is bypass.
    pub spatial: VoiceSpatial,
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
            spatial: VoiceSpatial::default(),
        }
    }
}

/// At or above this cutoff the low-pass is off.
pub const LOW_PASS_OPEN_HZ: f32 = 20_000.0;
/// At or below this cutoff the high-pass is off.
pub const HIGH_PASS_OPEN_HZ: f32 = 20.0;

/// A voice's low-pass and high-pass cutoffs. A side at its open frequency is bypassed: the sink
/// does not run it, so an all-open filter leaves the voice bit-identical to an unfiltered one.
///
/// # Example
///
/// ```
/// use ludion_core::audio::{AudioEvent, VoiceFilter, VoiceSpatial};
///
/// // "A radio: only 300 Hz to 3.4 kHz gets through."
/// let filter = VoiceFilter::band_pass(300.0, 3400.0);
/// assert!(filter.low_pass_on() && filter.high_pass_on());
/// assert!(!VoiceFilter::default().low_pass_on());
/// let event = AudioEvent {
///     spatial: VoiceSpatial { filter, send: 0.0 },
///     ..AudioEvent::new("voice/radio.wav", 1.0)
/// };
/// assert_eq!(event.spatial.filter, filter);
/// ```
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct VoiceFilter {
    /// The low-pass cutoff in Hz; [`LOW_PASS_OPEN_HZ`] or more is off.
    pub low_pass_hz: f32,
    /// The high-pass cutoff in Hz; [`HIGH_PASS_OPEN_HZ`] or less is off.
    pub high_pass_hz: f32,
}

impl VoiceFilter {
    /// Both sides open: no filtering.
    pub const BYPASS: VoiceFilter = VoiceFilter {
        low_pass_hz: LOW_PASS_OPEN_HZ,
        high_pass_hz: 0.0,
    };

    /// A band: frequencies below `high_pass_hz` and above `low_pass_hz` are cut.
    pub fn band_pass(high_pass_hz: f32, low_pass_hz: f32) -> VoiceFilter {
        VoiceFilter {
            low_pass_hz,
            high_pass_hz,
        }
    }

    /// Whether the low-pass runs (a NaN cutoff reads as off).
    pub fn low_pass_on(&self) -> bool {
        self.low_pass_hz < LOW_PASS_OPEN_HZ
    }

    /// Whether the high-pass runs (a NaN cutoff reads as off).
    pub fn high_pass_on(&self) -> bool {
        self.high_pass_hz > HIGH_PASS_OPEN_HZ
    }
}

impl Default for VoiceFilter {
    /// [`BYPASS`](Self::BYPASS).
    fn default() -> VoiceFilter {
        VoiceFilter::BYPASS
    }
}

/// Per-voice spatial state the sink applies: a [`VoiceFilter`] and a reverb send.
///
/// # Example
///
/// ```
/// use ludion_core::audio::VoiceSpatial;
///
/// assert_eq!(VoiceSpatial::default().send, 0.0);
/// assert!(!VoiceSpatial::default().filter.low_pass_on());
/// ```
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct VoiceSpatial {
    /// The filter; the default is bypass.
    pub filter: VoiceFilter,
    /// The reverb send added to the bus's default ([`Bus::default_send`]); the voice's post-fader
    /// send is `clamp(default + send, 0, 1)`.
    pub send: f32,
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
    /// Spoken dialogue and barks.
    Voice,
    /// Interface sounds: clicks, confirmations, menu music stings.
    Ui,
}

/// A mixdown bus: every [`AudioCategory`], the music bus and the master over all of them.
///
/// Runtime gain and pause ([`AudioMessage::BusGain`], [`AudioMessage::BusPause`]) address buses.
/// A tag the sink reads, never simulation state.
///
/// # Example
///
/// ```
/// use ludion_core::audio::{AudioCategory, Bus};
///
/// assert_eq!(Bus::from(AudioCategory::Voice), Bus::Voice);
/// assert_eq!(Bus::from(AudioCategory::Sfx), Bus::Sfx);
/// assert_eq!(Bus::ALL.len(), 6);
/// ```
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Bus {
    /// Multiplies every other bus.
    Master,
    /// Sound effects.
    Sfx,
    /// Ambient beds.
    Ambient,
    /// Dialogue.
    Voice,
    /// Interface sounds.
    Ui,
    /// The music player.
    Music,
}

impl Bus {
    /// Every bus, in index order (`bus as usize`).
    pub const ALL: [Bus; 6] = [
        Bus::Master,
        Bus::Sfx,
        Bus::Ambient,
        Bus::Voice,
        Bus::Ui,
        Bus::Music,
    ];

    /// The reverb send a voice on this bus starts from: `1.0` for `Sfx`, `Ambient` and `Voice`,
    /// `0.0` for the rest, so music and interface sounds stay dry.
    ///
    /// ```
    /// use ludion_core::audio::Bus;
    ///
    /// assert_eq!(Bus::Ambient.default_send(), 1.0);
    /// assert_eq!(Bus::Music.default_send(), 0.0);
    /// ```
    pub fn default_send(self) -> f32 {
        match self {
            Bus::Sfx | Bus::Ambient | Bus::Voice => 1.0,
            Bus::Master | Bus::Ui | Bus::Music => 0.0,
        }
    }
}

impl From<AudioCategory> for Bus {
    fn from(category: AudioCategory) -> Bus {
        match category {
            AudioCategory::Sfx => Bus::Sfx,
            AudioCategory::Ambient => Bus::Ambient,
            AudioCategory::Voice => Bus::Voice,
            AudioCategory::Ui => Bus::Ui,
        }
    }
}

/// Which runtime gain layer a [`AudioMessage::BusGain`] writes. The layers multiply.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MixLayer {
    /// The player's setting (a volume slider); survives a world rebuild.
    User,
    /// The game's mix (ducking, scripted levels); reset by a world rebuild.
    Game,
}

/// The master gain of each mixdown bus: how loud sound effects are against ambience.
///
/// The game authors it once (`EngineBuilder::category_gains`) and the host hands it to the playback
/// sink, which multiplies every voice by its own bus's value while mixing. It is a mixdown decision,
/// the same category of thing as which device to open — never simulation state: it reaches no
/// `WorldHash`, no snapshot and no audio log, so changing a value here cannot move a replay. Every
/// bus, the master included, has an authored field; runtime volume, ducking and pause go through [`AudioMessage::BusGain`] and
/// [`AudioMessage::BusPause`].
///
/// It lives here, beside [`AudioCategory`], because the engine (which stores the authored value) and
/// the sink (which applies it) both need the type and neither may depend on the other. It is
/// `#[non_exhaustive]`: build it from the default with [`with`](Self::with).
///
/// # Example
///
/// ```
/// use ludion_core::audio::{AudioCategory, Bus, CategoryGains};
///
/// // "Ambience sits under the effects."
/// let gains = CategoryGains::default().with(Bus::Ambient, 0.6);
/// assert_eq!(gains.gain_for(AudioCategory::Ambient), 0.6);
/// assert_eq!(gains.gain(Bus::Ambient), 0.6);
/// // The default is the identity: the authored gains, untouched.
/// assert_eq!(CategoryGains::default().gain_for(AudioCategory::Ambient), 1.0);
/// ```
#[derive(Clone, Copy, Debug, PartialEq)]
#[non_exhaustive]
pub struct CategoryGains {
    /// The master gain over every bus.
    pub master: f32,
    /// The master gain of the sound-effect bus.
    pub sfx: f32,
    /// The master gain of the ambient bus.
    pub ambient: f32,
    /// The master gain of the dialogue bus.
    pub voice: f32,
    /// The master gain of the interface bus.
    pub ui: f32,
    /// The master gain of the music bus. Music only travels in [`AudioMessage::MusicPlay`],
    /// [`AudioMessage::MusicSet`] and their companions, so its bus is implied: there is no
    /// [`AudioCategory`] for it and [`gain_for`](Self::gain_for) never returns it.
    pub music: f32,
}

impl Default for CategoryGains {
    /// Identity: every bus at `1.0`, so the mixdown is exactly the sum of the authored gains.
    fn default() -> CategoryGains {
        CategoryGains {
            master: 1.0,
            sfx: 1.0,
            ambient: 1.0,
            voice: 1.0,
            ui: 1.0,
            music: 1.0,
        }
    }
}

impl CategoryGains {
    /// The master gain a voice on `category` is multiplied by while mixing (the music bus is read
    /// from [`music`](Self::music) directly).
    pub fn gain_for(&self, category: AudioCategory) -> f32 {
        self.gain(Bus::from(category))
    }

    /// The authored gain of `bus`.
    pub fn gain(&self, bus: Bus) -> f32 {
        match bus {
            Bus::Master => self.master,
            Bus::Sfx => self.sfx,
            Bus::Ambient => self.ambient,
            Bus::Voice => self.voice,
            Bus::Ui => self.ui,
            Bus::Music => self.music,
        }
    }

    /// These gains with `bus` set to `gain`.
    pub fn with(mut self, bus: Bus, gain: f32) -> CategoryGains {
        match bus {
            Bus::Master => self.master = gain,
            Bus::Sfx => self.sfx = gain,
            Bus::Ambient => self.ambient = gain,
            Bus::Voice => self.voice = gain,
            Bus::Ui => self.ui = gain,
            Bus::Music => self.music = gain,
        }
        self
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
    /// Bed `i` of an `AudioZone` entity.
    ZoneBed(crate::ids::EntityId, u8),
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
/// use ludion_core::audio::{AudioCategory, AudioMessage, LoopId, VoiceSpatial};
///
/// // "Keep `sfx/wind.wav` looping at 0.4." — the same message that starts it and updates it.
/// let message = AudioMessage::LoopSync {
///     id: LoopId::Sound("sfx/wind.wav".to_string()),
///     sound: "sfx/wind.wav".to_string(),
///     gain: 0.4,
///     pan: 0.0,
///     category: AudioCategory::Ambient,
///     spatial: VoiceSpatial::default(),
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
        /// The loop's filter and reverb send; a resend retargets the running voice.
        spatial: VoiceSpatial,
    },
    /// Silence the loop running under `id`, if any; the sink fades it out over about 10 ms.
    LoopStop {
        /// Which loop to silence.
        id: LoopId,
    },
    /// Silence every running loop and the music; each fades out over about 10 ms. A loop's playback
    /// state lives only in the sink, so a rebuilt world has nothing to diff against — the blanket
    /// stop is what keeps a loop or a track from outliving the scene that started it. It also resets
    /// the [`MixLayer::Game`] gains to `1.0` and every bus pause, drops the voices held on a paused
    /// bus, and keeps the [`MixLayer::User`] gains. It also resets the reverb to dry and the zone
    /// layer to `1.0`.
    StopAllLoops,
    /// Stream `track` as the music, or update the gain of the track already playing. Music is one
    /// voice: the same track again (not stopping) only takes the new `gain`, so resending it every
    /// tick is harmless; any other track hard-cuts the current one and fades the new one in
    /// ([`MusicCrossfade`](AudioMessage::MusicCrossfade) blends instead).
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
    /// hard-cuts the current music and fades the new one in — [`MusicCrossfade`](AudioMessage::MusicCrossfade)
    /// blends instead.
    MusicSet {
        /// The set's identity: what makes a resend a no-op.
        id: String,
        /// Each stem's content-root-relative Ogg Vorbis path and its starting gain.
        stems: Vec<(String, f32)>,
        /// The fade-in length in sim ticks (`0` starts at full level).
        fade_frames: u32,
    },
    /// Blend from the playing music to a **set** of up to [`MUSIC_STEMS_MAX`] Ogg stems: the new set
    /// fades in over `fade_frames` sim ticks while the live one fades out over the same ticks, as an
    /// equal-power pair. The same `id` again while it is the live set (not stopping) is a no-op, so
    /// resending it every tick is harmless. With nothing playing it is a plain fade-in; a third set
    /// mid-crossfade hard-cuts the outgoing one. A set that cannot open keeps the old music playing.
    ///
    /// ```
    /// use ludion_core::audio::AudioMessage;
    ///
    /// let message = AudioMessage::MusicCrossfade {
    ///     id: "combat".into(),
    ///     stems: vec![("audio/combat.ogg".into(), 1.0)],
    ///     fade_frames: 120,
    /// };
    /// assert!(matches!(message, AudioMessage::MusicCrossfade { fade_frames: 120, .. }));
    /// ```
    MusicCrossfade {
        /// The set's identity: what makes a resend a no-op.
        id: String,
        /// Each stem's content-root-relative Ogg Vorbis path and its starting gain.
        stems: Vec<(String, f32)>,
        /// The crossfade length in sim ticks (`0` hard-cuts).
        fade_frames: u32,
    },
    /// Slew one stem of the live (incoming) set to `gain` over `fade_frames` sim ticks (`0` still
    /// slews about 10 ms, click-free). A stem not in the set is ignored; resending the same gain is
    /// free.
    MusicStemGain {
        /// The stem's path, as named in the set.
        stem: String,
        /// The target linear gain.
        gain: f32,
        /// The slew length in sim ticks.
        fade_frames: u32,
    },
    /// Slew one layer of a bus's runtime gain to `gain` over `fade_frames` sim ticks (`0` still
    /// slews about 10 ms, click-free). The effective bus gain is `authored x user x game x pause`.
    ///
    /// ```
    /// use ludion_core::audio::{AudioMessage, Bus, MixLayer};
    ///
    /// // "The player set music to 40%."
    /// let message = AudioMessage::BusGain {
    ///     bus: Bus::Music,
    ///     layer: MixLayer::User,
    ///     gain: 0.4,
    ///     fade_frames: 0,
    /// };
    /// assert_ne!(message, AudioMessage::StopAllLoops);
    /// ```
    BusGain {
        /// The bus.
        bus: Bus,
        /// The layer written.
        layer: MixLayer,
        /// The target linear gain, at least `0.0`.
        gain: f32,
        /// The slew length in sim ticks.
        fade_frames: u32,
    },
    /// Hold (`paused`) or release the listed buses over `fade_frames` sim ticks. A held voice keeps
    /// its cursor — loops and music included — and silent voices are not advanced.
    BusPause {
        /// The buses held or released.
        buses: Vec<Bus>,
        /// `true` holds, `false` resumes.
        paused: bool,
        /// The fade length in sim ticks.
        fade_frames: u32,
    },
    /// Start a one-shot the engine keeps addressable under `id`: it plays once like
    /// [`OneShot`](AudioMessage::OneShot), but [`VoiceSync`](AudioMessage::VoiceSync) moves its level
    /// and [`VoiceStop`](AudioMessage::VoiceStop) fades it out while it plays.
    ///
    /// ```
    /// use ludion_core::audio::{AudioCategory, AudioMessage, VoiceId, VoiceSpatial};
    ///
    /// // "A guard's chatter, started at 80% a little left of centre."
    /// let message = AudioMessage::VoiceStart {
    ///     id: VoiceId(0),
    ///     sound: "voice/chatter.wav".to_string(),
    ///     gain: 0.8,
    ///     pan: -0.2,
    ///     category: AudioCategory::Voice,
    ///     spatial: VoiceSpatial::default(),
    ///     offset_ticks: 0,
    /// };
    /// assert_ne!(message, AudioMessage::StopAllLoops);
    /// ```
    VoiceStart {
        /// Which voice this starts.
        id: VoiceId,
        /// The sound, as a content-root-relative WAV path.
        sound: String,
        /// A linear gain multiplier applied to the voice's samples.
        gain: f32,
        /// Stereo position: `-1.0` hard left, `0.0` centre, `1.0` hard right.
        pan: f32,
        /// Which mixdown bus the voice belongs to.
        category: AudioCategory,
        /// The voice's filter and reverb send.
        spatial: VoiceSpatial,
        /// Sim ticks into the clip the voice starts at; 0 plays it from the start.
        offset_ticks: u32,
    },
    /// Slew the voice `id` to a new `gain`, `pan` and filter, keeping its cursor. A voice that has
    /// ended is ignored.
    VoiceSync {
        /// The voice addressed.
        id: VoiceId,
        /// The target linear gain.
        gain: f32,
        /// The target stereo position.
        pan: f32,
        /// The target filter and reverb send.
        spatial: VoiceSpatial,
    },
    /// Fade the voice `id` out over `fade_frames` sim ticks (at least about 10 ms), then retire it.
    VoiceStop {
        /// The voice addressed.
        id: VoiceId,
        /// The fade length in sim ticks.
        fade_frames: u32,
    },
    /// Slew the engine-owned duck layer of a bus to `gain` over `fade_frames` sim ticks. The duck
    /// layer multiplies with `authored x user x game x pause`; `gain` is linear in `[0, 1]`, the
    /// product of every active duck on that bus (`1.0` releases it).
    ///
    /// ```
    /// use ludion_core::audio::{AudioMessage, Bus};
    ///
    /// // "Music dips to about 35% over 6 ticks while a voice line plays."
    /// let message = AudioMessage::Duck {
    ///     bus: Bus::Music,
    ///     gain: 0.35,
    ///     fade_frames: 6,
    /// };
    /// assert_ne!(message, AudioMessage::StopAllLoops);
    /// ```
    Duck {
        /// The bus ducked.
        bus: Bus,
        /// The target linear gain in `[0, 1]`.
        gain: f32,
        /// The slew length in sim ticks.
        fade_frames: u32,
    },
    /// Move the reverb the listener hears to `params` over `fade_frames` sim ticks (`0` still
    /// slews click-free). The engine resolves it from the `AudioZone`s around the listener;
    /// [`ReverbParams::DRY`] releases it.
    ///
    /// ```
    /// use ludion_core::audio::{AudioMessage, ReverbParams};
    ///
    /// // "The listener walked into a hangar."
    /// let message = AudioMessage::ReverbSet {
    ///     params: ReverbParams { decay_s: 3.5, pre_delay_ms: 40.0, damping: 0.3, size: 0.9, wet: 0.3 },
    ///     fade_frames: 0,
    /// };
    /// assert_ne!(message, AudioMessage::StopAllLoops);
    /// ```
    ReverbSet {
        /// The target reverb.
        params: ReverbParams,
        /// The slew length in sim ticks.
        fade_frames: u32,
    },
    /// Slew the engine-owned zone layer of a bus to `gain` over `fade_frames` sim ticks. It is
    /// never a [`MixLayer`]: `BusGain` must not write state the engine owns, so the effective bus
    /// gain is `authored x user x game x duck x zone x pause`. `gain` is linear, at least `0.0`;
    /// `1.0` releases it.
    ///
    /// ```
    /// use ludion_core::audio::{AudioMessage, Bus};
    ///
    /// // "The dock zone sits the ambient bus at 90%."
    /// let message = AudioMessage::ZoneGain {
    ///     bus: Bus::Ambient,
    ///     gain: 0.9,
    ///     fade_frames: 0,
    /// };
    /// assert_ne!(message, AudioMessage::StopAllLoops);
    /// ```
    ZoneGain {
        /// The bus moved.
        bus: Bus,
        /// The target linear gain.
        gain: f32,
        /// The slew length in sim ticks.
        fade_frames: u32,
    },
}

/// The most beds one `AudioZone` plays.
pub const AUDIO_ZONE_BEDS_MAX: usize = 8;

/// The reverb a listener hears: a decay tail on a send bus. Ranges: `decay_s` `[0.1, 20]`,
/// `pre_delay_ms` `[0, 200]`, `damping`, `size` and `wet` `[0, 1]`.
///
/// # Example
///
/// ```
/// use ludion_core::audio::ReverbParams;
///
/// let hall = ReverbParams { decay_s: 1.8, pre_delay_ms: 20.0, damping: 0.4, size: 0.6, wet: 0.25 };
/// assert!(hall.near(&hall));
/// assert!(!hall.near(&ReverbParams::DRY));
/// ```
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ReverbParams {
    /// Seconds for the tail to fall 60 dB.
    pub decay_s: f32,
    /// Milliseconds before the tail starts.
    pub pre_delay_ms: f32,
    /// High-frequency absorption.
    pub damping: f32,
    /// Room size, small to large.
    pub size: f32,
    /// The mix of reverb against dry sound.
    pub wet: f32,
}

impl ReverbParams {
    /// No reverb.
    pub const DRY: ReverbParams = ReverbParams {
        decay_s: 1.0,
        pre_delay_ms: 0.0,
        damping: 0.5,
        size: 0.5,
        wet: 0.0,
    };

    /// These params clamped into range; a non-finite field becomes its [`DRY`](Self::DRY) value.
    pub fn clamped(self) -> ReverbParams {
        fn fix(v: f32, dry: f32, lo: f32, hi: f32) -> f32 {
            if v.is_finite() { v.clamp(lo, hi) } else { dry }
        }
        let d = ReverbParams::DRY;
        ReverbParams {
            decay_s: fix(self.decay_s, d.decay_s, 0.1, 20.0),
            pre_delay_ms: fix(self.pre_delay_ms, d.pre_delay_ms, 0.0, 200.0),
            damping: fix(self.damping, d.damping, 0.0, 1.0),
            size: fix(self.size, d.size, 0.0, 1.0),
            wet: fix(self.wet, d.wet, 0.0, 1.0),
        }
    }

    /// Whether every field is within `max(|other.field| x 0.01, 1e-3)` of `other`'s.
    pub fn near(&self, other: &ReverbParams) -> bool {
        let close = |a: f32, b: f32| (a - b).abs() <= (b.abs() * 0.01).max(1e-3);
        close(self.decay_s, other.decay_s)
            && close(self.pre_delay_ms, other.pre_delay_ms)
            && close(self.damping, other.damping)
            && close(self.size, other.size)
            && close(self.wet, other.wet)
    }
}

/// The identity of one attached voice (`AudioQueue::play_on`): a per-queue counter, reset when the
/// world rebuilds. A tag the sink keys its voice table by, never simulation state.
///
/// # Example
///
/// ```
/// use ludion_core::audio::VoiceId;
///
/// assert!(VoiceId(0) < VoiceId(1));
/// ```
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct VoiceId(pub u64);

/// The most stems a [`AudioMessage::MusicSet`] plays; stems past it are dropped with `L5006`.
pub const MUSIC_STEMS_MAX: usize = 8;

/// The deepest a bus's combined duck goes, in dB: stacked ducks multiply but never pass it.
pub const DUCK_FLOOR_DB: f32 = -24.0;
