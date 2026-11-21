//! Dialogue voice: the per-channel audio table, the WAV-length cache and locale-aware path
//! resolution, and the voice ids of the lines on air.

use std::collections::BTreeMap;
use std::path::PathBuf;

use ludion_assets::AssetRoot;
use ludion_core::audio::{
    AudioCategory, Bus, HIGH_PASS_OPEN_HZ, LOW_PASS_OPEN_HZ, VoiceFilter, VoiceId,
};
use ludion_core::codes::code;
use ludion_core::diagnostics::Diagnostic;
use ludion_core::ids::EntityId;
use ludion_ecs::World;

use super::Channel;
use crate::diagnostics::report_full;

/// How one dialogue channel sounds: the bus, the filter and the cut fade.
///
/// ```
/// use ludion_engine::dialogue::ChannelAudio;
///
/// let radio = ChannelAudio::default_for(ludion_engine::dialogue::Channel::Radio);
/// assert!(radio.filter.low_pass_on());
/// ```
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ChannelAudio {
    /// The bus the voice plays on.
    pub category: AudioCategory,
    /// The voice filter (default: a radio band-pass, otherwise bypass).
    pub filter: VoiceFilter,
    /// The linear gain of the clip.
    pub gain: f32,
    /// The hearing range of a `world` voice, in metres; unused by `radio` and `ui`.
    pub range: f32,
    /// Sim ticks a cut line's voice fades out over.
    pub cut_fade_ticks: u32,
}

impl ChannelAudio {
    /// The default of `channel`: radio `Voice` band-passed 300 Hz to 3.4 kHz, world `Voice`
    /// unfiltered with range 20 m, ui `Ui` unfiltered.
    pub fn default_for(channel: Channel) -> Self {
        let (category, filter, range) = match channel {
            Channel::Radio => (
                AudioCategory::Voice,
                VoiceFilter::band_pass(300.0, 3400.0),
                0.0,
            ),
            Channel::World => (AudioCategory::Voice, VoiceFilter::BYPASS, 20.0),
            Channel::Ui => (AudioCategory::Ui, VoiceFilter::BYPASS, 0.0),
        };
        Self {
            category,
            filter,
            gain: 1.0,
            range,
            cut_fade_ticks: 6,
        }
    }
}

/// The audio of every dialogue channel plus the suspend behaviour
/// ([`EngineBuilder::dialogue_channels`](crate::EngineBuilder::dialogue_channels)).
///
/// ```
/// use ludion_engine::dialogue::DialogueChannels;
///
/// let channels = DialogueChannels::default();
/// assert_eq!(channels.suspend_fade_ticks, 6);
/// assert!(channels.validate().is_ok());
/// ```
#[derive(Clone, Debug, PartialEq)]
pub struct DialogueChannels {
    /// The `radio` channel.
    pub radio: ChannelAudio,
    /// The `world` channel.
    pub world: ChannelAudio,
    /// The `ui` channel.
    pub ui: ChannelAudio,
    /// The buses [`suspend`](super::suspend) pauses (default `[Voice]`: a pause menu's own `Ui`
    /// sounds stay live). Never `Master`.
    pub suspend_buses: Vec<Bus>,
    /// Sim ticks the pause and the resume fade over.
    pub suspend_fade_ticks: u32,
}

impl Default for DialogueChannels {
    fn default() -> Self {
        Self {
            radio: ChannelAudio::default_for(Channel::Radio),
            world: ChannelAudio::default_for(Channel::World),
            ui: ChannelAudio::default_for(Channel::Ui),
            suspend_buses: vec![Bus::Voice],
            suspend_fade_ticks: 6,
        }
    }
}

impl DialogueChannels {
    /// The audio of `channel`.
    pub fn of(&self, channel: Channel) -> &ChannelAudio {
        match channel {
            Channel::Radio => &self.radio,
            Channel::World => &self.world,
            Channel::Ui => &self.ui,
        }
    }

    /// Checks the table.
    ///
    /// # Errors
    /// `L1719` for a gain or range that is negative or not finite, a filter outside
    /// `20 Hz..=20 kHz` or with the high-pass at or above the low-pass, or `Master` in
    /// `suspend_buses`.
    pub fn validate(&self) -> Result<(), Diagnostic> {
        let channels = [
            ("radio", &self.radio),
            ("world", &self.world),
            ("ui", &self.ui),
        ];
        let mut problem = None;
        for (name, audio) in channels {
            for (field, value) in [("gain", audio.gain), ("range", audio.range)] {
                if !(value.is_finite() && value >= 0.0) {
                    problem.get_or_insert(format!(
                        "`{name}.{field}` is {value}; it must be finite and not negative"
                    ));
                }
            }
            let (high, low) = (audio.filter.high_pass_hz, audio.filter.low_pass_hz);
            let in_band = |hz: f32| hz.is_finite() && (0.0..=LOW_PASS_OPEN_HZ).contains(&hz);
            if !in_band(high) || !in_band(low) || (audio.filter.high_pass_on() && high >= low) {
                problem.get_or_insert(format!(
                    "`{name}.filter` (high-pass {high} Hz, low-pass {low} Hz) must lie within \
                     {HIGH_PASS_OPEN_HZ}..={LOW_PASS_OPEN_HZ} Hz with the high-pass below the low-pass"
                ));
            }
        }
        if self.suspend_buses.contains(&Bus::Master) {
            problem
                .get_or_insert("`suspend_buses` names `Master`, which pauses all sound".to_owned());
        }
        match problem {
            Some(problem) => Err(Diagnostic::new(
                code::DIALOGUE_CHANNELS_INVALID,
                format!("the dialogue channel audio is invalid: {problem}"),
            )
            .with_help("see `ludion docs dialogue`")),
            None => Ok(()),
        }
    }
}

/// Whole sim ticks a clip of `frames` sample frames at `sample_rate` lasts at `hz`, rounding up in
/// integer math (the rule the engine's own voice tracking uses).
pub(crate) fn clip_ticks_of(frames: u64, sample_rate: u32, hz: u32) -> u64 {
    (frames * u64::from(hz)).div_ceil(u64::from(sample_rate.max(1)))
}

/// WAV lengths in ticks per path, read from the content root once. A resource: derived, never
/// hashed or snapshotted, reset with every world build.
#[derive(Debug)]
pub(crate) struct VoiceLibrary {
    root: PathBuf,
    ticks: BTreeMap<String, Option<u64>>,
}

impl VoiceLibrary {
    pub(crate) fn new(root: PathBuf) -> Self {
        Self {
            root,
            ticks: BTreeMap::new(),
        }
    }

    /// The length of `path` in ticks, or `None` when it will not read or decode.
    pub(crate) fn clip_ticks(&mut self, hz: u32, path: &str) -> Option<u64> {
        if let Some(ticks) = self.ticks.get(path) {
            return *ticks;
        }
        let ticks = ludion_assets::load_wav(AssetRoot::content(&self.root), path)
            .ok()
            .map(|pcm| {
                let frames = (pcm.samples.len() / usize::from(pcm.channels.max(1))) as u64;
                clip_ticks_of(frames, pcm.sample_rate, hz)
            });
        self.ticks.insert(path.to_owned(), ticks);
        ticks
    }
}

/// `template` with `{locale}` replaced; a path without the placeholder is returned as is.
pub(crate) fn locale_path(template: &str, locale: &str) -> String {
    template.replace("{locale}", locale)
}

/// The voice file a line plays and its length in ticks: the active locale's, else the default
/// locale's (`L5137`, once per path), else `None` (`L5138`, once per path).
pub(crate) fn resolve(
    world: &mut World,
    hz: u32,
    template: &str,
    active: &str,
    default: &str,
) -> Option<(String, u64)> {
    let wanted = locale_path(template, active);
    let fallback = locale_path(template, default);
    let mut read = |path: &str| world.resource_mut::<VoiceLibrary>()?.clip_ticks(hz, path);
    if let Some(ticks) = read(&wanted) {
        return Some((wanted, ticks));
    }
    if fallback != wanted
        && let Some(ticks) = read(&fallback)
    {
        report_full(
            world,
            format!("voice_fell_back:{wanted}"),
            Diagnostic::new(
                code::DIALOGUE_VOICE_FELL_BACK,
                format!(
                    "voice `{wanted}` does not read; playing the `{default}` one, `{fallback}`"
                ),
            )
            .with_help("record the line for that locale, or accept the fallback"),
        );
        return Some((fallback, ticks));
    }
    report_full(
        world,
        format!("voice_unreadable:{wanted}"),
        Diagnostic::new(
            code::DIALOGUE_VOICE_UNREADABLE,
            format!(
                "voice `{wanted}` does not read; the line is silent and lasts its reading time"
            ),
        )
        .with_help("add the WAV under the content root, or correct the line's `voice:`"),
    );
    None
}

/// The voice ids of the lines on air, and whether the suspend pause is in force. Never hashed or
/// snapshotted; reset with every world build, so a restored mid-line plays on silently.
#[derive(Debug, Default)]
pub(crate) struct DialogueVoices {
    pub(crate) by_line: BTreeMap<EntityId, VoiceId>,
    pub(crate) paused: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_validate() {
        assert!(DialogueChannels::default().validate().is_ok());
    }

    #[test]
    fn validate_names_the_bad_field() {
        let mut channels = DialogueChannels::default();
        channels.world.gain = f32::NAN;
        let error = channels.validate().unwrap_err();
        assert_eq!(error.code, code::DIALOGUE_CHANNELS_INVALID);
        assert!(error.message.contains("world.gain"), "{}", error.message);

        let mut channels = DialogueChannels::default();
        channels.ui.range = -1.0;
        assert!(channels.validate().is_err());

        let mut channels = DialogueChannels::default();
        channels.radio.filter = VoiceFilter::band_pass(4000.0, 300.0);
        assert!(channels.validate().is_err());

        let mut channels = DialogueChannels::default();
        channels.radio.filter = VoiceFilter::band_pass(300.0, 40_000.0);
        assert!(channels.validate().is_err());

        let mut channels = DialogueChannels::default();
        channels.suspend_buses.push(Bus::Master);
        assert!(channels.validate().is_err());

        let mut channels = DialogueChannels::default();
        channels.suspend_buses.clear();
        assert!(channels.validate().is_ok());
    }

    #[test]
    fn tick_math_rounds_up_in_integers() {
        assert_eq!(clip_ticks_of(44_100, 44_100, 60), 60);
        assert_eq!(clip_ticks_of(44_101, 44_100, 60), 61);
        assert_eq!(clip_ticks_of(1, 44_100, 60), 1);
        assert_eq!(clip_ticks_of(0, 44_100, 60), 0);
    }

    #[test]
    fn locale_path_substitutes_only_the_placeholder() {
        assert_eq!(locale_path("voice/{locale}/a.wav", "fr"), "voice/fr/a.wav");
        assert_eq!(locale_path("voice/a.wav", "fr"), "voice/a.wav");
    }
}
