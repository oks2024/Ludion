//! The two data-doc kinds dialogue owns — `kind: strings` and `kind: dialogue` — and the
//! validation pass over them (`L1711`–`L1718`).

use std::collections::{BTreeMap, BTreeSet};

use ludion_core::codes::code;
use ludion_core::diagnostics::Diagnostic;
use ludion_ecs::World;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::strings::placeholders;
use crate::docs::DocStore;

/// The `kind:` of a string table.
pub(crate) const STRINGS_KIND: &str = "strings";
/// The `kind:` of a speaker-and-line bank.
pub(crate) const DIALOGUE_KIND: &str = "dialogue";

/// One locale's flat key → text map (`kind: strings`). Several files may share a locale; a key
/// appears once per locale (`L1711`).
///
/// ```yaml
/// format: 1
/// kind: strings
/// locale: en
/// strings:
///   radio.intro: "Operator to Neon, do you copy?"
/// ```
#[derive(Serialize, Deserialize, JsonSchema, Clone, Debug, PartialEq, Default)]
#[serde(deny_unknown_fields)]
pub struct StringsDoc {
    /// The locale tag these strings are written in (`en`, `fr`, ...).
    pub locale: String,
    /// Key → text. `{name}` is a placeholder; `{{` and `}}` are literal braces.
    #[serde(default)]
    pub strings: BTreeMap<String, String>,
}

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
    /// Named cues, each the tick after the start it fires on (default none).
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub cues: BTreeMap<String, u32>,
}

/// The slot a speaker's lines queue on: one global `radio`, one global `ui`, and a `world` slot per
/// speaker entity.
#[derive(
    Serialize, Deserialize, JsonSchema, Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash,
)]
#[serde(rename_all = "snake_case")]
pub enum Channel {
    /// Comms: one line at a time across the game.
    Radio,
    /// Spoken from a world entity: one line at a time per speaker.
    World,
    /// Interface voice: one line at a time across the game.
    Ui,
}

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

/// `L1711`–`L1718` over every loaded `kind: strings` and `kind: dialogue` doc, in store order (a
/// duplicate is reported at the later path). `default_locale` is the config's, which a line's text
/// and a speaker's name must exist in.
pub(crate) fn validate_dialogue_docs(world: &World, default_locale: &str) -> Vec<Diagnostic> {
    let mut out = Vec::new();
    let strings = world.resource::<DocStore<StringsDoc>>();
    let dialogue = world.resource::<DocStore<DialogueDoc>>();
    // locale -> key -> (doc path, text); the first writer is kept, a later one is the duplicate.
    let mut tables: BTreeMap<&str, BTreeMap<&str, (&str, &str)>> = BTreeMap::new();
    if let Some(store) = strings {
        for (path, doc) in store.iter() {
            if doc.locale.trim().is_empty() {
                out.push(diagnostic(
                    code::STRINGS_LOCALE_INVALID,
                    format!("`{path}` has an empty `locale`"),
                    format!("strings/{path}/locale"),
                ));
                continue;
            }
            let table = tables.entry(doc.locale.as_str()).or_default();
            for (key, text) in &doc.strings {
                if table.contains_key(key.as_str()) {
                    out.push(diagnostic(
                        code::STRINGS_DUPLICATE_KEY,
                        format!(
                            "string key `{key}` is already defined for locale `{}` by an earlier `kind: strings` doc",
                            doc.locale
                        ),
                        format!("strings/{path}/strings/{key}"),
                    ));
                } else {
                    table.insert(key, (path, text));
                }
            }
        }
    }
    let default = tables.get(default_locale);
    let has_default = |key: &str| default.is_some_and(|table| table.contains_key(key));
    if let Some(store) = dialogue {
        let mut speakers: BTreeSet<&str> = BTreeSet::new();
        for (_, doc) in store.iter() {
            speakers.extend(doc.speakers.keys().map(String::as_str));
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
    // A translation whose placeholders differ from the default locale's drops or invents a value.
    if let Some(default) = default {
        for (locale, table) in &tables {
            if *locale == default_locale {
                continue;
            }
            for (key, (path, text)) in table {
                let Some((_, base)) = default.get(key) else {
                    continue;
                };
                if placeholders(text) != placeholders(base) {
                    out.push(diagnostic(
                        code::STRINGS_PLACEHOLDER_MISMATCH,
                        format!(
                            "string `{key}` in locale `{locale}` uses different `{{placeholders}}` than `{default_locale}`"
                        ),
                        format!("strings/{path}/strings/{key}"),
                    ));
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
