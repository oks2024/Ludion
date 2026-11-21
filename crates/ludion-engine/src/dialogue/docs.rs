//! The data-doc kind dialogue owns — `kind: dialogue` — and the validation pass over it
//! (`L1712`–`L1715`, `L1717`, `L1720`, `L1721`).

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use ludion_assets::AssetRoot;
use ludion_core::codes::code;
use ludion_core::diagnostics::Diagnostic;
use ludion_ecs::World;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::voice::locale_path;
use crate::docs::DocStore;
use crate::localization::{default_locale, key_tables};

/// The `kind:` of a speaker-and-line bank.
pub(crate) const DIALOGUE_KIND: &str = "dialogue";

/// A speaker and line bank (`kind: dialogue`).
///
/// ```yaml
/// format: 1
/// kind: dialogue
/// speakers:
///   operator: { name: speaker.operator, channel: radio }
/// lines:
///   intro: { speaker: operator, text: radio.intro, priority: high }
/// ```
#[derive(Serialize, Deserialize, JsonSchema, Clone, Debug, PartialEq, Default)]
#[serde(deny_unknown_fields)]
pub struct DialogueDoc {
    /// Speakers by id, unique across every dialogue doc (`L1713`).
    #[serde(default)]
    pub speakers: BTreeMap<String, Speaker>,
    /// Lines by id, unique across every dialogue doc (`L1712`).
    #[serde(default)]
    pub lines: BTreeMap<String, LineDef>,
}

/// Who talks and where it is heard.
#[derive(Serialize, Deserialize, JsonSchema, Clone, Debug, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Speaker {
    /// The string-table key of the speaker's display name.
    pub name: String,
    /// The slot this speaker's lines queue on.
    pub channel: Channel,
    /// A portrait the game's own UI looks up; the engine only carries the string (default none).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub portrait: Option<String>,
    /// Subtitle colour, linear RGBA (default white).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub color: Option<[f32; 4]>,
}

/// One authored line.
#[derive(Serialize, Deserialize, JsonSchema, Clone, Debug, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct LineDef {
    /// The speaker id.
    pub speaker: String,
    /// The string-table key of the line's text.
    pub text: String,
    /// Queue priority (default `normal`).
    #[serde(default)]
    pub priority: Priority,
    /// Whether it cuts the line its slot is playing (default `none`).
    #[serde(default)]
    pub interrupt: Interrupt,
    /// Whether the line holds the `GameClock` while it plays (default `false`).
    #[serde(default)]
    pub holds_gameplay: bool,
    /// What a blocked channel does to the line while it waits (default `wait`).
    #[serde(default)]
    pub when_blocked: WhenBlocked,
    /// Ticks the line may wait queued before it is dropped (default unbounded).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_wait_ticks: Option<u32>,
    /// Authored length in ticks; absent uses the reading-time rule (default absent).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub duration_ticks: Option<u32>,
    /// A WAV under the content root, `{locale}` replaced by the active locale; the line lasts the
    /// clip plus `tail_ticks` (default silent).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub voice: Option<String>,
    /// Ticks the line outlasts its voice; absent uses the config's `tail_ticks` (default absent).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tail_ticks: Option<u32>,
    /// A `world` line is subtitled only while the listener is within this many world units of the
    /// speaker; the line still plays (default absent: always subtitled).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subtitle_range: Option<f32>,
    /// Named cues, each the tick after the start it fires on (default none).
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub cues: BTreeMap<String, u32>,
}

pub use ludion_components::subtitle::Channel;

/// Queue priority; a higher one starts first and may cut a lower one.
#[derive(
    Serialize,
    Deserialize,
    JsonSchema,
    Clone,
    Copy,
    Debug,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    Hash,
    Default,
)]
#[serde(rename_all = "snake_case")]
pub enum Priority {
    /// Ambient barks.
    Low,
    /// The default.
    #[default]
    Normal,
    /// Story beats.
    High,
    /// Starts through a gameplay hold.
    Critical,
}

/// Whether a head line cuts the line its slot is playing.
#[derive(Serialize, Deserialize, JsonSchema, Clone, Copy, Debug, PartialEq, Eq, Hash, Default)]
#[serde(rename_all = "snake_case")]
pub enum Interrupt {
    /// Never; the line waits its turn.
    #[default]
    None,
    /// Only a line of strictly lower priority.
    Lower,
    /// Whatever is playing.
    Any,
}

/// What a blocked channel does to a queued line.
#[derive(Serialize, Deserialize, JsonSchema, Clone, Copy, Debug, PartialEq, Eq, Hash, Default)]
#[serde(rename_all = "snake_case")]
pub enum WhenBlocked {
    /// Stay queued until the channel unblocks.
    #[default]
    Wait,
    /// End `Blocked` at once.
    Drop,
}

fn diagnostic(
    code: ludion_core::diagnostics::DiagnosticCode,
    message: String,
    path: String,
) -> Diagnostic {
    Diagnostic::new(code, message)
        .with_path(path)
        .with_help("see `ludion docs dialogue`")
}

/// `L1712`–`L1715`, `L1717`, `L1720` and `L1721` over every loaded `kind: dialogue` doc, in store
/// order (a duplicate is reported at the later path). A line's text and a speaker's name must exist
/// in the default locale; the string tables themselves are validated by localization.
pub(crate) fn validate_dialogue_docs(world: &World, root: &Path) -> Vec<Diagnostic> {
    let default_locale = default_locale(world);
    let default_locale = default_locale.as_str();
    let mut out = Vec::new();
    // Voice path -> whether it reads; one load per distinct path per pass.
    let mut reads: BTreeMap<String, bool> = BTreeMap::new();
    let mut readable = |path: String| {
        *reads.entry(path).or_insert_with_key(|path| {
            ludion_assets::load_wav(AssetRoot::content(root), path).is_ok()
        })
    };
    let dialogue = world.resource::<DocStore<DialogueDoc>>();
    let tables = key_tables(world);
    let default = tables.get(default_locale);
    let has_default = |key: &str| default.is_some_and(|table| table.contains(key));
    if let Some(store) = dialogue {
        let mut speakers: BTreeSet<&str> = BTreeSet::new();
        let mut channels: BTreeMap<&str, Channel> = BTreeMap::new();
        for (_, doc) in store.iter() {
            speakers.extend(doc.speakers.keys().map(String::as_str));
            for (id, speaker) in &doc.speakers {
                channels.entry(id.as_str()).or_insert(speaker.channel);
            }
        }
        let mut seen_speakers: BTreeSet<&str> = BTreeSet::new();
        let mut seen_lines: BTreeSet<&str> = BTreeSet::new();
        for (path, doc) in store.iter() {
            for (id, speaker) in &doc.speakers {
                if !seen_speakers.insert(id) {
                    out.push(diagnostic(
                        code::DIALOGUE_DUPLICATE_SPEAKER,
                        format!(
                            "speaker `{id}` is already defined by an earlier `kind: dialogue` doc"
                        ),
                        format!("dialogue/{path}/speakers/{id}"),
                    ));
                }
                if !has_default(&speaker.name) {
                    out.push(missing_default(
                        &speaker.name,
                        default_locale,
                        format!("dialogue/{path}/speakers/{id}/name"),
                    ));
                }
            }
            for (id, line) in &doc.lines {
                if !seen_lines.insert(id) {
                    out.push(diagnostic(
                        code::DIALOGUE_DUPLICATE_LINE,
                        format!(
                            "line `{id}` is already defined by an earlier `kind: dialogue` doc"
                        ),
                        format!("dialogue/{path}/lines/{id}"),
                    ));
                }
                if !speakers.contains(line.speaker.as_str()) {
                    out.push(diagnostic(
                        code::DIALOGUE_UNKNOWN_SPEAKER,
                        format!(
                            "line `{id}` names speaker `{}`, which no `kind: dialogue` doc defines",
                            line.speaker
                        ),
                        format!("dialogue/{path}/lines/{id}/speaker"),
                    ));
                }
                if !has_default(&line.text) {
                    out.push(missing_default(
                        &line.text,
                        default_locale,
                        format!("dialogue/{path}/lines/{id}/text"),
                    ));
                }
                if line.duration_ticks == Some(0) {
                    out.push(diagnostic(
                        code::DIALOGUE_LINE_INVALID,
                        format!(
                            "line `{id}` has `duration_ticks: 0`; a line lasts at least one tick"
                        ),
                        format!("dialogue/{path}/lines/{id}/duration_ticks"),
                    ));
                }
                if let Some(template) = &line.voice {
                    let voice_path = format!("dialogue/{path}/lines/{id}/voice");
                    if !readable(locale_path(template, default_locale)) {
                        out.push(diagnostic(
                            code::DIALOGUE_VOICE_MISSING,
                            format!(
                                "line `{id}` has voice `{}`, which does not read for the default locale `{default_locale}`",
                                locale_path(template, default_locale)
                            ),
                            voice_path,
                        ));
                    } else if template.contains("{locale}") {
                        for locale in tables.keys().filter(|locale| **locale != default_locale) {
                            let file = locale_path(template, locale);
                            if !readable(file.clone()) {
                                out.push(diagnostic(
                                    code::DIALOGUE_VOICE_LOCALE_MISSING,
                                    format!(
                                        "line `{id}` has no voice `{file}` for locale `{locale}`; the default locale's plays"
                                    ),
                                    voice_path.clone(),
                                ));
                            }
                        }
                    }
                }
                if let Some(range) = line.subtitle_range {
                    let problem = if !range.is_finite() || range <= 0.0 {
                        Some(format!(
                            "line `{id}` has `subtitle_range: {range}`; it must be finite and above 0"
                        ))
                    } else if channels
                        .get(line.speaker.as_str())
                        .is_some_and(|channel| *channel != Channel::World)
                    {
                        Some(format!(
                            "line `{id}` has a `subtitle_range`, which only applies to a world speaker"
                        ))
                    } else {
                        None
                    };
                    if let Some(message) = problem {
                        out.push(diagnostic(
                            code::DIALOGUE_LINE_INVALID,
                            message,
                            format!("dialogue/{path}/lines/{id}/subtitle_range"),
                        ));
                    }
                }
                if let Some(duration) = line.duration_ticks {
                    for (name, at) in &line.cues {
                        if *at >= duration {
                            out.push(diagnostic(
                                code::DIALOGUE_LINE_INVALID,
                                format!("cue `{name}` of line `{id}` is at tick {at}, at or past its `duration_ticks` {duration}"),
                                format!("dialogue/{path}/lines/{id}/cues/{name}"),
                            ));
                        }
                    }
                }
            }
        }
    }
    out
}

fn missing_default(key: &str, default_locale: &str, path: String) -> Diagnostic {
    diagnostic(
        code::STRING_KEY_MISSING_DEFAULT,
        format!("string key `{key}` is missing from the default locale `{default_locale}`"),
        path,
    )
}
