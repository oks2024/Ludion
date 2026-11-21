//! Dialogue: string tables, a deterministic line queue and subtitles (`ludion docs dialogue`).
//!
//! A game opts in with [`EngineBuilder::dialogue`](crate::EngineBuilder::dialogue), schedules
//! [`dialogue_update`] once per tick, and talks through the free functions here. **The API only
//! marks state** — [`say`] spawns a queued [`DialogueLine`], [`interrupt`] sets a cancel — and the
//! system emits every event, so an interruption and the `Ended` it causes land the same tick the
//! system runs.
//!
//! State is entities, not a resource, so it rides hashing, snapshots and checkpoints: one
//! [`DialogueLine`] per said line (its handle *is* the entity) and one lazily spawned
//! [`DialogueControl`]. A game that never speaks spawns neither and hashes as if it had not opted in.

mod docs;
mod queue;
mod strings;
mod voice;

use ludion_core::codes::code;
use ludion_core::diagnostics::Diagnostic;
use ludion_core::ids::EntityId;
use ludion_ecs::World;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

pub use docs::{
    Channel, DialogueDoc, Interrupt, LineDef, Priority, Speaker, StringsDoc, WhenBlocked,
};
pub use queue::{DialogueEvent, DialogueEvents, Subtitle, Subtitles, dialogue_update};
pub use strings::{format_text, text};
pub use voice::{ChannelAudio, DialogueChannels};

pub(crate) use docs::{DIALOGUE_KIND, STRINGS_KIND, validate_dialogue_docs};
pub(crate) use strings::StringIndex;
pub(crate) use voice::{DialogueVoices, VoiceLibrary};

/// The game's dialogue settings ([`EngineBuilder::dialogue`](crate::EngineBuilder::dialogue)).
///
/// A line with no authored `duration_ticks` lasts `max(min_s, base_s + per_char_s × chars)` seconds,
/// `chars` counting the formatted active-locale text.
///
/// ```
/// use ludion_engine::DialogueConfig;
///
/// let config = DialogueConfig::default();
/// assert_eq!(config.default_locale, "en");
/// assert!(config.validate().is_ok());
/// ```
#[derive(Clone, Debug, PartialEq)]
pub struct DialogueConfig {
    /// The locale every key must exist in, and the fallback for the others (default `en`).
    pub default_locale: String,
    /// Reading time: seconds before the per-character part (default `1.0`).
    pub base_s: f32,
    /// Reading time: seconds per character (default `0.06`).
    pub per_char_s: f32,
    /// Reading time: the shortest a line lasts, in seconds (default `1.5`).
    pub min_s: f32,
    /// Ticks a voiced line outlasts its clip when the line sets no `tail_ticks` (default `12`).
    pub tail_ticks: u32,
}

impl Default for DialogueConfig {
    fn default() -> Self {
        Self {
            default_locale: "en".to_owned(),
            base_s: 1.0,
            per_char_s: 0.06,
            min_s: 1.5,
            tail_ticks: 12,
        }
    }
}

impl DialogueConfig {
    /// Checks the config.
    ///
    /// # Errors
    /// `L1710` for an empty `default_locale`, or a reading constant that is negative or not finite.
    pub fn validate(&self) -> Result<(), Diagnostic> {
        let bad_constant = [
            ("base_s", self.base_s),
            ("per_char_s", self.per_char_s),
            ("min_s", self.min_s),
        ]
        .into_iter()
        .find(|(_, value)| !(value.is_finite() && *value >= 0.0));
        let problem = if self.default_locale.trim().is_empty() {
            Some("`default_locale` is empty".to_owned())
        } else {
            bad_constant.map(|(name, value)| {
                format!("`{name}` is {value}; it must be finite and not negative")
            })
        };
        match problem {
            Some(problem) => Err(Diagnostic::new(
                code::DIALOGUE_CONFIG_INVALID,
                format!("the dialogue config is invalid: {problem}"),
            )
            .with_help("see `ludion docs dialogue`")),
            None => Ok(()),
        }
    }
}

/// How a line stands in the queue.
#[derive(Serialize, Deserialize, JsonSchema, Clone, Copy, Debug, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum LineState {
    /// Waiting on the previous line of its sequence.
    Pending,
    /// Waiting for its slot.
    #[default]
    Queued,
    /// On air.
    Playing,
}

/// Why a line ended.
#[derive(Serialize, Deserialize, JsonSchema, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum EndReason {
    /// It played its full duration.
    Completed,
    /// Cut while playing: by an interrupting line, [`interrupt`], or its speaker despawning.
    Interrupted,
    /// Its channel was blocked.
    Blocked,
    /// It never played: it waited past `max_wait_ticks`, was cancelled while queued, its sequence
    /// predecessor did not complete, or its speaker despawned.
    Dropped,
}

/// A named cue on a line.
#[derive(Serialize, Deserialize, JsonSchema, Clone, Debug, PartialEq, Eq)]
pub struct LineCue {
    /// The cue name.
    pub name: String,
    /// The tick, counted from the line's start, it fires on.
    pub at: u32,
}

/// One placeholder value of a line.
#[derive(Serialize, Deserialize, JsonSchema, Clone, Debug, PartialEq, Eq)]
pub struct LineArg {
    /// The `{name}` it fills.
    pub name: String,
    /// The text it becomes.
    pub value: String,
}

/// One said line: everything the queue needs, copied at [`say`] time so a later bank reload only
/// affects lines said after it. A runtime entity, hashed and snapshotted like any other; the engine
/// despawns it when the line ends. Read through [`line_state`] and [`DialogueEvents`] rather than
/// writing it.
///
/// ```
/// use ludion_engine::dialogue::DialogueLine;
///
/// let line: DialogueLine = serde_json::from_value(serde_json::json!({
///     "line": "intro", "speaker_id": "operator", "text": "radio.intro", "channel": "radio",
/// }))
/// .unwrap();
/// assert_eq!(line.duration, 0);
/// ```
#[derive(Serialize, Deserialize, JsonSchema, Clone, Debug, PartialEq)]
pub struct DialogueLine {
    /// The line id in its dialogue doc.
    pub line: String,
    /// The speaker id.
    pub speaker_id: String,
    /// The string-table key of the text.
    pub text: String,
    /// The slot.
    pub channel: Channel,
    /// The speaking entity; required on the `world` channel (default none).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(with = "Option<ludion_ecs::EntityRefSchema>")]
    pub speaker: Option<EntityId>,
    /// Queue priority.
    #[serde(default)]
    pub priority: Priority,
    /// Whether it cuts the slot's current line.
    #[serde(default)]
    pub interrupt: Interrupt,
    /// Whether it holds the `GameClock` while playing.
    #[serde(default, skip_serializing_if = "is_false")]
    pub holds_gameplay: bool,
    /// What a blocked channel does while it waits.
    #[serde(default)]
    pub when_blocked: WhenBlocked,
    /// Ticks it may wait before being dropped.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_wait_ticks: Option<u32>,
    /// Authored length in ticks.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub duration_ticks: Option<u32>,
    /// The voice WAV, `{locale}` unsubstituted.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub voice: Option<String>,
    /// Authored ticks past the voice clip.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tail_ticks: Option<u32>,
    /// Cues, by `at` then name.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub cues: Vec<LineCue>,
    /// Placeholder values.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub args: Vec<LineArg>,
    /// Enqueue order, from `DialogueControl::next_seq`.
    #[serde(default)]
    pub seq: u64,
    /// The queue order: this line's `seq`, or its sequence head's.
    #[serde(default)]
    pub order: u64,
    /// The `seq` of the sequence line before this one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub after: Option<u64>,
    /// Where it stands.
    #[serde(default)]
    pub state: LineState,
    /// Ticks spent queued.
    #[serde(default)]
    pub waited: u32,
    /// Ticks played.
    #[serde(default)]
    pub elapsed: u32,
    /// Length in ticks, fixed at the start.
    #[serde(default)]
    pub duration: u32,
    /// The sim frame it started on.
    #[serde(default)]
    pub started_frame: u64,
    /// A cancel waiting for the next [`dialogue_update`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cancel: Option<EndReason>,
}

/// The dialogue's shared state, one lazily spawned entity: the locale override, blocked channels,
/// the suspend flag and the next enqueue number.
///
/// ```
/// use ludion_engine::dialogue::DialogueControl;
///
/// let control: DialogueControl = serde_json::from_value(serde_json::json!({})).unwrap();
/// assert_eq!(control.next_seq, 0);
/// ```
#[derive(Serialize, Deserialize, JsonSchema, Clone, Debug, PartialEq, Eq, Default)]
pub struct DialogueControl {
    /// The active locale; none means the config's default.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub locale: Option<String>,
    /// Blocked channels, sorted and unique.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub blocked: Vec<Channel>,
    /// Whether progress is frozen.
    #[serde(default, skip_serializing_if = "is_false")]
    pub suspended: bool,
    /// The `seq` the next line takes.
    #[serde(default)]
    pub next_seq: u64,
}

fn is_false(value: &bool) -> bool {
    !*value
}

/// A said line's handle: the entity that carries it, valid until the line ends.
///
/// ```
/// use ludion_engine::dialogue::LineHandle;
/// use ludion_core::ids::EntityId;
///
/// let handle = LineHandle(EntityId::new(3, 0));
/// assert_eq!(handle.0, EntityId::new(3, 0));
/// ```
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct LineHandle(pub EntityId);

fn not_enabled() -> Diagnostic {
    Diagnostic::new(
        code::DIALOGUE_NOT_ENABLED,
        "a dialogue call was made, but the game did not opt in",
    )
    .with_help("add `.dialogue(DialogueConfig::default())` to the engine builder")
}

/// The control entity, spawned on first use.
fn control_entity(world: &mut World) -> EntityId {
    if let Some(id) = world
        .query_ids::<(&DialogueControl,)>()
        .map(|(id, _)| id)
        .min()
    {
        return id;
    }
    let id = world.spawn();
    let _ = world.insert(id, DialogueControl::default());
    id
}

fn control_mut(world: &mut World) -> Option<&mut DialogueControl> {
    let id = control_entity(world);
    world.get_mut::<DialogueControl>(id).ok().flatten()
}

fn control(world: &mut World) -> Option<DialogueControl> {
    world
        .query_ids::<(&DialogueControl,)>()
        .map(|(id, (control,))| (id, control.clone()))
        .min_by_key(|(id, _)| *id)
        .map(|(_, control)| control)
}

fn find_line(world: &World, line: &str) -> Option<LineDef> {
    world
        .resource::<crate::DocStore<DialogueDoc>>()?
        .iter()
        .find_map(|(_, doc)| doc.lines.get(line).cloned())
}

fn find_speaker(world: &World, speaker: &str) -> Option<Speaker> {
    world
        .resource::<crate::DocStore<DialogueDoc>>()?
        .iter()
        .find_map(|(_, doc)| doc.speakers.get(speaker).cloned())
}

/// Everything `say` and `say_sequence` check for one line, before anything is spawned.
fn prepare(
    world: &World,
    line: &str,
    speaker: Option<EntityId>,
) -> Result<(LineDef, Speaker), Diagnostic> {
    if world.resource::<DialogueConfig>().is_none() {
        return Err(not_enabled());
    }
    let def = find_line(world, line).ok_or_else(|| {
        Diagnostic::new(
            code::DIALOGUE_UNKNOWN_LINE,
            format!("no `kind: dialogue` doc defines a line `{line}`"),
        )
        .with_help("check the id against the `lines:` of your dialogue docs")
    })?;
    let who = find_speaker(world, &def.speaker).ok_or_else(|| {
        Diagnostic::new(
            code::DIALOGUE_UNKNOWN_LINE,
            format!(
                "line `{line}` names speaker `{}`, which no dialogue doc defines",
                def.speaker
            ),
        )
        .with_help("define the speaker under `speakers:`")
    })?;
    if who.channel == Channel::World && !speaker.is_some_and(|entity| world.is_alive(entity)) {
        return Err(Diagnostic::new(
            code::DIALOGUE_NO_SPEAKER,
            format!("line `{line}` is on the `world` channel, so it needs a live speaker entity"),
        )
        .with_help("pass `Some(entity)` to `say_with`, or put the speaker on `radio`/`ui`"));
    }
    Ok((def, who))
}

/// Where a sequence line sits behind its predecessor.
struct Chain {
    /// The predecessor's `seq`.
    after: u64,
    /// The sequence head's `seq`, the queue order of every line of it.
    order: u64,
    /// Every line of a sequence holds iff its head does.
    holds_gameplay: bool,
}

fn spawn_line(
    world: &mut World,
    id: &str,
    def: &LineDef,
    who: &Speaker,
    speaker: Option<EntityId>,
    args: &[(&str, &str)],
    chain: Option<Chain>,
) -> LineHandle {
    let seq = {
        let Some(control) = control_mut(world) else {
            unreachable!("the control entity was just ensured");
        };
        let seq = control.next_seq;
        control.next_seq += 1;
        seq
    };
    let mut cues: Vec<LineCue> = def
        .cues
        .iter()
        .map(|(name, at)| LineCue {
            name: name.clone(),
            at: *at,
        })
        .collect();
    cues.sort_by(|a, b| (a.at, &a.name).cmp(&(b.at, &b.name)));
    let component = DialogueLine {
        line: id.to_owned(),
        speaker_id: def.speaker.clone(),
        text: def.text.clone(),
        channel: who.channel,
        speaker,
        priority: def.priority,
        interrupt: def.interrupt,
        holds_gameplay: chain
            .as_ref()
            .map_or(def.holds_gameplay, |chain| chain.holds_gameplay),
        when_blocked: def.when_blocked,
        max_wait_ticks: def.max_wait_ticks,
        duration_ticks: def.duration_ticks,
        voice: def.voice.clone(),
        tail_ticks: def.tail_ticks,
        cues,
        args: args
            .iter()
            .map(|(name, value)| LineArg {
                name: (*name).to_owned(),
                value: (*value).to_owned(),
            })
            .collect(),
        seq,
        order: chain.as_ref().map_or(seq, |chain| chain.order),
        after: chain.as_ref().map(|chain| chain.after),
        state: if chain.is_some() {
            LineState::Pending
        } else {
            LineState::Queued
        },
        waited: 0,
        elapsed: 0,
        duration: 0,
        started_frame: 0,
        cancel: None,
    };
    let entity = world.spawn();
    let _ = world.insert(entity, component);
    LineHandle(entity)
}

/// Queues `line` (an id from a `kind: dialogue` doc) spoken by `speaker` (required for a `world`
/// speaker, `L5131`; `None` for radio/ui).
///
/// # Errors
/// `L5136` without `.dialogue()`, `L5130` for an unknown line or speaker, `L5131` for a `world`
/// line without a speaker.
///
/// # Example
/// ```
/// use ludion_engine::{DialogueConfig, Engine, dialogue};
///
/// let mut engine = Engine::builder().dialogue(DialogueConfig::default()).build().unwrap();
/// // No bank is loaded, so the line is unknown.
/// let refused = dialogue::say(engine.world_mut(), "intro", None).unwrap_err();
/// assert_eq!(refused.code.as_str(), "L5130");
/// ```
pub fn say(
    world: &mut World,
    line: &str,
    speaker: Option<EntityId>,
) -> Result<LineHandle, Diagnostic> {
    say_with(world, line, speaker, &[])
}

/// As [`say`], with `args` filling `{name}` placeholders.
///
/// # Errors
/// As [`say`].
pub fn say_with(
    world: &mut World,
    line: &str,
    speaker: Option<EntityId>,
    args: &[(&str, &str)],
) -> Result<LineHandle, Diagnostic> {
    let (def, who) = prepare(world, line, speaker)?;
    Ok(spawn_line(world, line, &def, &who, speaker, args, None))
}

/// Queues `lines` as one sequence: the next line starts the tick the previous one completes, with no
/// gap and no release of a hold; any other end of one line drops the rest. Every line holds
/// gameplay iff the first does. `speakers[i]` is line `i`'s speaker entity.
///
/// # Errors
/// `L5135` for an empty sequence or a `speakers` list of another length; otherwise as [`say`]
/// (nothing is queued when any line is refused).
pub fn say_sequence(
    world: &mut World,
    lines: &[&str],
    speakers: &[Option<EntityId>],
) -> Result<Vec<LineHandle>, Diagnostic> {
    if world.resource::<DialogueConfig>().is_none() {
        return Err(not_enabled());
    }
    if lines.is_empty() || lines.len() != speakers.len() {
        return Err(Diagnostic::new(
            code::DIALOGUE_SEQUENCE_INVALID,
            format!(
                "a sequence needs at least one line and one speaker slot per line; got {} lines and {} speakers",
                lines.len(),
                speakers.len()
            ),
        )
        .with_help("pass `None` for a line with no speaker entity"));
    }
    let mut prepared = Vec::with_capacity(lines.len());
    for (line, speaker) in lines.iter().zip(speakers) {
        prepared.push(prepare(world, line, *speaker)?);
    }
    let holds_gameplay = prepared[0].0.holds_gameplay;
    let mut handles: Vec<LineHandle> = Vec::with_capacity(lines.len());
    let mut chain: Option<Chain> = None;
    for (index, ((def, who), speaker)) in prepared.iter().zip(speakers).enumerate() {
        let handle = spawn_line(world, lines[index], def, who, *speaker, &[], chain.take());
        let seq = world
            .get::<DialogueLine>(handle.0)
            .ok()
            .flatten()
            .map_or(0, |line| line.seq);
        let order = world
            .get::<DialogueLine>(handle.0)
            .ok()
            .flatten()
            .map_or(0, |line| line.order);
        chain = Some(Chain {
            after: seq,
            order,
            holds_gameplay,
        });
        handles.push(handle);
    }
    Ok(handles)
}

fn cancel_line(world: &mut World, entity: EntityId) -> bool {
    let Ok(Some(line)) = world.get_mut::<DialogueLine>(entity) else {
        return false;
    };
    line.cancel = Some(if line.state == LineState::Playing {
        EndReason::Interrupted
    } else {
        EndReason::Dropped
    });
    true
}

/// Ends the line at the next [`dialogue_update`]: `Interrupted` if it is playing, `Dropped` if it
/// has not started. `false` when the handle names no live line.
///
/// # Example
/// ```
/// use ludion_engine::{DialogueConfig, Engine, dialogue};
///
/// let mut engine = Engine::builder().dialogue(DialogueConfig::default()).build().unwrap();
/// let gone = dialogue::LineHandle(ludion_core::ids::EntityId::new(9, 0));
/// assert!(!dialogue::interrupt(engine.world_mut(), gone));
/// ```
pub fn interrupt(world: &mut World, handle: LineHandle) -> bool {
    cancel_line(world, handle.0)
}

/// Cancels every line `speaker` has, playing or waiting. Returns how many.
pub fn interrupt_speaker(world: &mut World, speaker: EntityId) -> usize {
    let mut mine: Vec<(u64, EntityId)> = world
        .query_ids::<(&DialogueLine,)>()
        .filter(|(_, (line,))| line.speaker == Some(speaker))
        .map(|(id, (line,))| (line.seq, id))
        .collect();
    mine.sort_unstable();
    mine.into_iter()
        .filter(|(_, id)| cancel_line(world, *id))
        .count()
}

/// Blocks or unblocks `channel`: a playing line on it ends `Blocked`, and a queued one waits or
/// drops by its `when_blocked`.
pub fn block_channel(world: &mut World, channel: Channel, blocked: bool) {
    if world.resource::<DialogueConfig>().is_none() {
        return;
    }
    if let Some(control) = control_mut(world) {
        control.blocked.retain(|c| *c != channel);
        if blocked {
            control.blocked.push(channel);
            control.blocked.sort_unstable();
        }
    }
}

/// Freezes (or resumes) every line: no progress, no aging, no starts. Cancels and a holding line's
/// hold still apply.
pub fn suspend(world: &mut World, suspended: bool) {
    if world.resource::<DialogueConfig>().is_none() {
        return;
    }
    if let Some(control) = control_mut(world) {
        control.suspended = suspended;
    }
}

/// Sets the active locale; live subtitles re-resolve the next tick.
pub fn set_locale(world: &mut World, locale: &str) {
    if world.resource::<DialogueConfig>().is_none() {
        return;
    }
    if let Some(control) = control_mut(world) {
        control.locale = Some(locale.to_owned());
    }
}

/// The active locale: the one [`set_locale`] set, else the config's default (empty without
/// `.dialogue()`).
pub fn locale(world: &mut World) -> String {
    control(world)
        .and_then(|control| control.locale)
        .or_else(|| {
            world
                .resource::<DialogueConfig>()
                .map(|config| config.default_locale.clone())
        })
        .unwrap_or_default()
}

/// Whether the speaker with this id (`speakers:` key) has a line on air.
pub fn speaking(world: &mut World, speaker_id: &str) -> bool {
    world
        .query_ids::<(&DialogueLine,)>()
        .any(|(_, (line,))| line.state == LineState::Playing && line.speaker_id == speaker_id)
}

/// Whether a playing line holds the `GameClock`.
pub fn holding(world: &mut World) -> bool {
    world
        .query_ids::<(&DialogueLine,)>()
        .any(|(_, (line,))| line.state == LineState::Playing && line.holds_gameplay)
}

/// Where the line stands, or `None` once it ended (its entity is gone).
pub fn line_state(world: &World, handle: LineHandle) -> Option<LineState> {
    world
        .get::<DialogueLine>(handle.0)
        .ok()
        .flatten()
        .map(|line| line.state)
}
