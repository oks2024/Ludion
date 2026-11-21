//! The line queue: [`dialogue_update`] and the resources it fills.
//!
//! One pass per tick, in this order:
//!
//! 1. cancels (`interrupt`, `interrupt_speaker`) end their lines;
//! 2. unless suspended, playing lines gain a tick; a line past its duration ends `Completed`; a
//!    dead speaker ends it `Interrupted`; a blocked channel ends it `Blocked`;
//! 3. sequences: a pending line whose predecessor `Completed` becomes `Queued` (the same tick,
//!    so there is no gap), and one whose predecessor ended any other way is `Dropped`;
//! 4. unless suspended, each slot — `radio`, `ui`, then `world` by speaker entity — starts its head
//!    line `(priority desc, order, seq)`, subject to the hold gate and the interrupt rule;
//! 5. unless suspended, still-queued lines age and a stale one is `Dropped`; a queued `drop` line
//!    on a blocked channel ends `Blocked`; a line with a dead speaker is `Dropped`; sequences cascade;
//! 6. cues fire for lines that started or advanced this tick;
//! 7. ended lines are despawned in ascending `seq`;
//! 8. voices: a playing line cut any way but `Completed` stops its voice, a change of the suspend
//!    flag pauses or resumes the suspend buses, and a line that started pushes its voice
//!    (`push_voice`, or `push_on` its speaker on `world`); the first pass after a build or restore
//!    also restarts the voice of each playing line that has a `voice_file`, at its `elapsed`;
//! 9. `GameClock::hold` is called if a playing line holds gameplay;
//! 10. [`DialogueEvents`] and [`Subtitles`] are rewritten.
//!
//! A line with a `voice:` starts with the file read for the active locale (else the default's) and
//! lasts the clip plus its tail; the length is read once per path.

use std::cmp::Reverse;
use std::collections::BTreeMap;

use ludion_core::ids::EntityId;
use ludion_core::time::GameClock;
use ludion_ecs::{SystemCtx, World};

use crate::audio::{AudioEvent, AudioQueue, VoiceSpatial};
use crate::localization::{self, render};

use super::voice::{DialogueChannels, DialogueVoices, VoiceLibrary, resolve};
use super::{
    Channel, DialogueConfig, DialogueControl, DialogueLine, EndReason, Interrupt, LineHandle,
    LineState, Priority, WhenBlocked, control, find_speaker,
};

/// One thing that happened to a line this tick.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DialogueEvent {
    /// The line went on air.
    Started {
        /// The line's handle (its entity; gone once it ends).
        handle: LineHandle,
        /// The line id.
        line: String,
        /// The speaker entity; `None` for a radio or ui line said without one.
        speaker: Option<EntityId>,
        /// The channel the line plays on.
        channel: Channel,
    },
    /// The line ended.
    Ended {
        /// The line's handle.
        handle: LineHandle,
        /// The line id.
        line: String,
        /// Why.
        reason: EndReason,
    },
    /// A named cue fired.
    Cue {
        /// The line's handle.
        handle: LineHandle,
        /// The line id.
        line: String,
        /// The cue name.
        name: String,
    },
}

/// Every [`DialogueEvent`] the last [`dialogue_update`] produced: ends by `seq`, starts by slot,
/// cues by `seq`. Rewritten on every run, so a consumer reads it the same tick, after that system.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct DialogueEvents {
    events: Vec<DialogueEvent>,
}

impl DialogueEvents {
    /// The events of this tick, in order.
    pub fn iter(&self) -> impl Iterator<Item = &DialogueEvent> {
        self.events.iter()
    }

    /// Whether `handle`'s line started this tick.
    pub fn started(&self, handle: LineHandle) -> bool {
        self.events
            .iter()
            .any(|event| matches!(event, DialogueEvent::Started { handle: h, .. } if *h == handle))
    }

    /// Why `handle`'s line ended this tick, if it did.
    pub fn ended(&self, handle: LineHandle) -> Option<EndReason> {
        self.events.iter().find_map(|event| match event {
            DialogueEvent::Ended {
                handle: h, reason, ..
            } if *h == handle => Some(*reason),
            _ => None,
        })
    }
}

/// One line being shown: its resolved, placeholder-filled text and the frames it spans.
#[derive(Clone, Debug, PartialEq)]
pub struct Subtitle {
    /// The line's handle.
    pub handle: LineHandle,
    /// The line id.
    pub line: String,
    /// The speaker id.
    pub speaker_id: String,
    /// The speaker's display name in the active locale.
    pub speaker: String,
    /// The speaker's colour, linear RGBA.
    pub color: [f32; 4],
    /// The line's text in the active locale.
    pub text: String,
    /// The slot it plays on.
    pub channel: Channel,
    /// The speaker's portrait string, if any.
    pub portrait: Option<String>,
    /// The sim frame it started on.
    pub start_frame: u64,
    /// The sim frame it ends on, exclusive: the first frame it is no longer shown.
    pub end_frame: u64,
}

/// What is on air now, rewritten every [`dialogue_update`], oldest first. Text-only: the game
/// draws it.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Subtitles {
    active: Vec<Subtitle>,
}

impl Subtitles {
    /// The lines on air, by start frame then enqueue order.
    pub fn iter(&self) -> impl Iterator<Item = &Subtitle> {
        self.active.iter()
    }

    /// How many lines are on air.
    pub fn len(&self) -> usize {
        self.active.len()
    }

    /// Whether nothing is on air.
    pub fn is_empty(&self) -> bool {
        self.active.is_empty()
    }

    /// The subtitle of `handle`'s line, if it is on air.
    pub fn get(&self, handle: LineHandle) -> Option<&Subtitle> {
        self.active
            .iter()
            .find(|subtitle| subtitle.handle == handle)
    }
}

/// A slot: one line at a time. Orders `radio`, `ui`, then `world` by speaker entity.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum Slot {
    Radio,
    Ui,
    World(EntityId),
}

struct Row {
    id: EntityId,
    line: DialogueLine,
    before: DialogueLine,
    ended: Option<EndReason>,
    progressed: bool,
    /// The resolved voice file, set on the tick the line starts.
    voice_file: Option<String>,
}

impl Row {
    fn playing(&self) -> bool {
        self.ended.is_none() && self.line.state == LineState::Playing
    }

    fn queued(&self) -> bool {
        self.ended.is_none() && self.line.state == LineState::Queued
    }

    fn slot(&self) -> Option<Slot> {
        match self.line.channel {
            Channel::Radio => Some(Slot::Radio),
            Channel::Ui => Some(Slot::Ui),
            Channel::World => self.line.speaker.map(Slot::World),
        }
    }

    fn handle(&self) -> LineHandle {
        LineHandle(self.id)
    }
}

fn end(row: &mut Row, reason: EndReason, events: &mut Vec<DialogueEvent>) {
    row.ended = Some(reason);
    events.push(DialogueEvent::Ended {
        handle: row.handle(),
        line: row.line.line.clone(),
        reason,
    });
}

/// Seconds to whole ticks at `hz`, rounding up with the slack `ludion-ai`'s timers use, so a
/// duration authored in tenths lands on the tick a human expects.
fn seconds_to_ticks(seconds: f32, hz: u32) -> u64 {
    let x = f64::from(seconds.max(0.0)) * f64::from(hz);
    (x - x * 1e-6).ceil() as u64
}

fn reading_ticks(config: &DialogueConfig, chars: usize, hz: u32) -> u32 {
    let seconds = config
        .min_s
        .max(config.base_s + config.per_char_s * chars as f32);
    u32::try_from(seconds_to_ticks(seconds, hz).max(1)).unwrap_or(u32::MAX)
}

/// Promotes or drops pending sequence lines whose predecessor ended. Ascending `seq`, so a drop
/// cascades down the chain in one pass.
fn resolve_sequences(rows: &mut [Row], events: &mut Vec<DialogueEvent>) {
    let mut ended: BTreeMap<u64, EndReason> = rows
        .iter()
        .filter_map(|row| row.ended.map(|reason| (row.line.seq, reason)))
        .collect();
    for index in 0..rows.len() {
        if rows[index].ended.is_some() || rows[index].line.state != LineState::Pending {
            continue;
        }
        let Some(after) = rows[index].line.after else {
            rows[index].line.state = LineState::Queued;
            continue;
        };
        match ended.get(&after) {
            Some(EndReason::Completed) => rows[index].line.state = LineState::Queued,
            Some(_) => {
                end(&mut rows[index], EndReason::Dropped, events);
                ended.insert(rows[index].line.seq, EndReason::Dropped);
            }
            None if rows.iter().any(|row| row.line.seq == after) => {}
            None => {
                end(&mut rows[index], EndReason::Dropped, events);
                ended.insert(rows[index].line.seq, EndReason::Dropped);
            }
        }
    }
}

/// Stops the voice of every playing line that ended other than `Completed`, starts the voice of
/// every line that started this tick (and, on the first update after a build or restore, restarts
/// the voice of every playing line that has none, at its `elapsed`), then pauses or resumes the
/// suspend buses on a change of the suspend flag. All in ascending `seq`; a world with no voiced
/// line pushes nothing.
fn sync_voices(world: &mut World, rows: &[Row], suspended: bool, hz: u32) {
    let Some(channels) = world.resource::<DialogueChannels>().cloned() else {
        return;
    };
    let Some(mut voices) = world.resource_mut::<DialogueVoices>().map(std::mem::take) else {
        return;
    };
    let mut stops = Vec::new();
    for row in rows {
        if row.before.state != LineState::Playing {
            continue;
        }
        let Some(reason) = row.ended else {
            continue;
        };
        if let Some(id) = voices.by_line.remove(&row.id)
            && reason != EndReason::Completed
        {
            stops.push((id, channels.of(row.line.channel).cut_fade_ticks));
        }
    }
    // Starts this tick, and on the first update after a build the resumes of lines already on
    // air, each with the file it started with, in ascending `seq`.
    let mut pushes: Vec<(&Row, &str, u32)> = Vec::new();
    for row in rows {
        if let Some(file) = &row.voice_file {
            pushes.push((row, file, 0));
        } else if !voices.resumed
            && row.playing()
            && let Some(file) = &row.line.voice_file
            && !voices.by_line.contains_key(&row.id)
            && world
                .resource_mut::<VoiceLibrary>()
                .and_then(|library| library.clip_ticks(hz, file))
                .is_some_and(|clip| u64::from(row.line.elapsed) < clip)
        {
            pushes.push((row, file, row.line.elapsed));
        }
    }
    if let Some(queue) = world.resource_mut::<AudioQueue>() {
        for (id, fade) in stops {
            queue.voice_stop(id, fade);
        }
        if suspended && !voices.paused && !(voices.by_line.is_empty() && pushes.is_empty()) {
            queue.bus_pause(&channels.suspend_buses, channels.suspend_fade_ticks);
            voices.paused = true;
        } else if !suspended && voices.paused {
            queue.bus_resume(&channels.suspend_buses, channels.suspend_fade_ticks);
            voices.paused = false;
        }
        for (row, file, offset) in pushes {
            let audio = channels.of(row.line.channel);
            let event = AudioEvent {
                category: audio.category,
                spatial: VoiceSpatial {
                    filter: audio.filter,
                    send: 0.0,
                },
                ..AudioEvent::new(file, audio.gain)
            };
            let id = match (row.line.channel, row.line.speaker) {
                (Channel::World, Some(speaker)) => {
                    queue.push_on_from(speaker, event, audio.range, offset)
                }
                _ => queue.push_voice_from(event, offset),
            };
            voices.by_line.insert(row.id, id);
        }
    }
    voices.resumed = true;
    world.insert_resource(voices);
}

/// Runs the queue for one tick (the order is in the module docs) and rewrites [`DialogueEvents`]
/// and [`Subtitles`]. Register it after the systems that call `say`, and declare its queries:
///
/// ```
/// use ludion_engine::dialogue::{DialogueControl, DialogueLine};
/// use ludion_engine::{DialogueConfig, DialogueEvents, Engine, Subtitles, dialogue_update};
/// use ludion_protocol::{Command, command};
///
/// let mut engine = Engine::builder()
///     .dialogue(DialogueConfig::default())
///     .system("dialogue_update", dialogue_update)
///     .query::<(&DialogueLine,)>()
///     .query::<(&DialogueControl,)>()
///     .build()
///     .unwrap();
/// engine
///     .execute(Command::SimStep(command::SimStepParams { ticks: 1, hash: false }))
///     .unwrap();
/// assert_eq!(engine.world().resource::<DialogueEvents>().unwrap().iter().count(), 0);
/// assert!(engine.world().resource::<Subtitles>().unwrap().is_empty());
/// ```
///
/// A no-op for a game that did not opt in.
pub fn dialogue_update(world: &mut World, ctx: &SystemCtx) {
    let Some(config) = world.resource::<DialogueConfig>().cloned() else {
        return;
    };
    let frame = ctx.frame.as_u64();
    let hz = ctx.game_time.rate().hz();
    let control: DialogueControl = control(world).unwrap_or_default();
    let suspended = control.suspended;
    let blocked = |channel: Channel| control.blocked.contains(&channel);
    let active = localization::locale(world);
    let default_locale = localization::default_locale(world);

    let mut rows: Vec<Row> = world
        .query_ids::<(&DialogueLine,)>()
        .map(|(id, (line,))| Row {
            id,
            line: line.clone(),
            before: line.clone(),
            ended: None,
            progressed: false,
            voice_file: None,
        })
        .collect();
    rows.sort_by_key(|row| (row.line.seq, row.id));
    let mut events: Vec<DialogueEvent> = Vec::new();

    // 1. Cancels.
    for row in &mut rows {
        if let Some(reason) = row.line.cancel.take() {
            end(row, reason, &mut events);
        }
    }

    // 2. Progress, completion, dead speakers, blocked channels.
    for row in &mut rows {
        if !row.playing() {
            continue;
        }
        if !suspended {
            row.line.elapsed += 1;
            row.progressed = true;
        }
        let dead = row
            .line
            .speaker
            .is_some_and(|speaker| !world.is_alive(speaker));
        if row.line.elapsed >= row.line.duration {
            end(row, EndReason::Completed, &mut events);
        } else if dead {
            end(row, EndReason::Interrupted, &mut events);
        } else if blocked(row.line.channel) {
            end(row, EndReason::Blocked, &mut events);
        }
    }

    // 3. Sequences.
    resolve_sequences(&mut rows, &mut events);

    // 4. Starts.
    if !suspended {
        let mut hold_active = rows
            .iter()
            .any(|row| row.playing() && row.line.holds_gameplay);
        let mut slots: Vec<Slot> = rows
            .iter()
            .filter(|row| row.queued())
            .filter_map(Row::slot)
            .collect();
        slots.sort_unstable();
        slots.dedup();
        for slot in slots {
            let current = rows
                .iter()
                .position(|row| row.playing() && row.slot() == Some(slot));
            let mut candidates: Vec<usize> = (0..rows.len())
                .filter(|&i| {
                    let row = &rows[i];
                    row.queued()
                        && row.slot() == Some(slot)
                        && !blocked(row.line.channel)
                        && row
                            .line
                            .speaker
                            .is_none_or(|speaker| world.is_alive(speaker))
                        && (!hold_active
                            || row.line.priority == Priority::Critical
                            || row.line.holds_gameplay)
                })
                .collect();
            candidates.sort_by_key(|&i| {
                let line = &rows[i].line;
                (Reverse(line.priority), line.order, line.seq)
            });
            let Some(&head) = candidates.first() else {
                continue;
            };
            if let Some(current) = current {
                let cut = match rows[head].line.interrupt {
                    Interrupt::None => false,
                    Interrupt::Lower => rows[current].line.priority < rows[head].line.priority,
                    Interrupt::Any => true,
                };
                if !cut {
                    continue;
                }
                end(&mut rows[current], EndReason::Interrupted, &mut events);
            }
            let args: Vec<(String, String)> = rows[head]
                .line
                .args
                .iter()
                .map(|arg| (arg.name.clone(), arg.value.clone()))
                .collect();
            let args: Vec<(&str, &str)> =
                args.iter().map(|(n, v)| (n.as_str(), v.as_str())).collect();
            let voice = rows[head]
                .line
                .voice
                .clone()
                .and_then(|template| resolve(world, hz, &template, &active, &default_locale));
            let duration = match (rows[head].line.duration_ticks, &voice) {
                (Some(ticks), _) => ticks,
                (None, Some((_, clip))) => {
                    let tail = rows[head].line.tail_ticks.unwrap_or(config.tail_ticks);
                    u32::try_from(clip + u64::from(tail))
                        .unwrap_or(u32::MAX)
                        .max(1)
                }
                (None, None) => {
                    let text = render(world, &rows[head].line.text, &args);
                    reading_ticks(&config, text.chars().count(), hz)
                }
            };
            let row = &mut rows[head];
            row.voice_file = voice.map(|(file, _)| file);
            row.line.voice_file = row.voice_file.clone();
            row.line.state = LineState::Playing;
            row.line.elapsed = 0;
            row.line.duration = duration;
            row.line.started_frame = frame;
            row.progressed = true;
            hold_active |= row.line.holds_gameplay;
            events.push(DialogueEvent::Started {
                handle: row.handle(),
                line: row.line.line.clone(),
                speaker: row.line.speaker,
                channel: row.line.channel,
            });
        }
    }

    // 5. Ageing, blocked drops, dead speakers.
    for row in &mut rows {
        if !row.queued() {
            continue;
        }
        let dead = row
            .line
            .speaker
            .is_some_and(|speaker| !world.is_alive(speaker));
        if row.line.when_blocked == WhenBlocked::Drop && blocked(row.line.channel) {
            end(row, EndReason::Blocked, &mut events);
        } else if dead {
            end(row, EndReason::Dropped, &mut events);
        } else if !suspended {
            row.line.waited += 1;
            if row
                .line
                .max_wait_ticks
                .is_some_and(|max| row.line.waited > max)
            {
                end(row, EndReason::Dropped, &mut events);
            }
        }
    }
    resolve_sequences(&mut rows, &mut events);

    // 6. Cues.
    for row in &rows {
        if row.ended.is_some() || !row.progressed || row.line.state != LineState::Playing {
            continue;
        }
        for cue in row
            .line
            .cues
            .iter()
            .filter(|cue| cue.at == row.line.elapsed)
        {
            events.push(DialogueEvent::Cue {
                handle: row.handle(),
                line: row.line.line.clone(),
                name: cue.name.clone(),
            });
        }
    }

    // 7. Write back; ended lines despawn in ascending `seq`.
    let mut doomed = Vec::new();
    for row in &rows {
        if row.ended.is_some() {
            doomed.push(row.id);
        } else if row.line != row.before
            && let Ok(Some(stored)) = world.get_mut::<DialogueLine>(row.id)
        {
            *stored = row.line.clone();
        }
    }
    let _ = world.despawn_many(&doomed);

    // 8. Voices.
    sync_voices(world, &rows, suspended, hz);

    // 9. Holds.
    if rows
        .iter()
        .any(|row| row.playing() && row.line.holds_gameplay)
        && let Some(clock) = world.resource_mut::<GameClock>()
    {
        clock.hold();
    }

    // 10. Subtitles and events.
    let mut on_air: Vec<&Row> = rows.iter().filter(|row| row.playing()).collect();
    on_air.sort_by_key(|row| (row.line.started_frame, row.line.seq));
    let mut active = Vec::with_capacity(on_air.len());
    for row in on_air {
        let line = &row.line;
        let args: Vec<(&str, &str)> = line
            .args
            .iter()
            .map(|arg| (arg.name.as_str(), arg.value.as_str()))
            .collect();
        let text = render(world, &line.text, &args);
        let speaker = find_speaker(world, &line.speaker_id);
        let label = match &speaker {
            Some(speaker) => render(world, &speaker.name, &[]),
            None => line.speaker_id.clone(),
        };
        active.push(Subtitle {
            handle: row.handle(),
            line: line.line.clone(),
            speaker_id: line.speaker_id.clone(),
            speaker: label,
            color: speaker
                .as_ref()
                .and_then(|speaker| speaker.color)
                .unwrap_or([1.0; 4]),
            text,
            channel: line.channel,
            portrait: speaker.and_then(|speaker| speaker.portrait),
            start_frame: line.started_frame,
            end_frame: line.started_frame + u64::from(line.duration),
        });
    }
    world.insert_resource(DialogueEvents { events });
    world.insert_resource(Subtitles { active });
}
