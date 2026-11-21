//! Dialogue (`ludion docs dialogue`): the string tables, the line queue and the subtitles. Every
//! test builds a game from `doc_text` documents on an empty temp root and reads one trace per tick.

use ludion_core::codes::code;
use ludion_core::ids::EntityId;
use ludion_ecs::{SystemCtx, World};
use ludion_engine::dialogue::{
    self, Channel, DialogueControl, DialogueEvent, DialogueLine, EndReason, LineHandle, LineState,
};
use ludion_engine::localization;
use ludion_engine::{
    Diagnostics, DialogueConfig, DialogueEvents, Engine, EngineBuilder, Subtitles, dialogue_update,
};
use ludion_protocol::command::{EngineRestoreParams, SimStepParams};
use ludion_protocol::{Command, ResponseBody};
use serde_json::Value;
use std::path::PathBuf;

const STRINGS_EN: &str = "format: 1
kind: strings
locale: en
strings:
  speaker.operator: Operator
  speaker.guard: Guard
  speaker.hud: HUD
  t.line: Copy that.
  t.timed: Hello {name}
  t.fr: Copy that
  t.only_en: English only
  t.greet: Hello {name}
  t.esc: Use {{braces}} {name}
";

const STRINGS_FR: &str = "format: 1
kind: strings
locale: fr
strings:
  t.fr: Bien recu
  t.greet: Bonjour {name}
";

const SPEAKERS: &str = "format: 1
kind: dialogue
speakers:
  operator:
    name: speaker.operator
    channel: radio
  guard_a:
    name: speaker.guard
    channel: world
  guard_b:
    name: speaker.guard
    channel: world
  hud:
    name: speaker.hud
    channel: ui
    portrait: hud
    color:
      - 1.0
      - 0.0
      - 0.0
      - 1.0
lines:
";

/// `(id, speaker, text key, extra fields at four-space indent)`.
const LINES: &[(&str, &str, &str, &str)] = &[
    (
        "bark_low",
        "operator",
        "t.line",
        "    priority: low\n    duration_ticks: 20\n",
    ),
    (
        "bark_norm",
        "operator",
        "t.line",
        "    duration_ticks: 10\n",
    ),
    (
        "story_hi",
        "operator",
        "t.line",
        "    priority: high\n    duration_ticks: 10\n",
    ),
    (
        "cut_lower",
        "operator",
        "t.line",
        "    interrupt: lower\n    duration_ticks: 10\n",
    ),
    (
        "cut_any",
        "operator",
        "t.line",
        "    interrupt: any\n    duration_ticks: 10\n",
    ),
    (
        "stale",
        "operator",
        "t.line",
        "    max_wait_ticks: 0\n    duration_ticks: 10\n",
    ),
    (
        "stale3",
        "operator",
        "t.line",
        "    max_wait_ticks: 3\n    duration_ticks: 10\n",
    ),
    (
        "drop_blocked",
        "operator",
        "t.line",
        "    when_blocked: drop\n    duration_ticks: 10\n",
    ),
    (
        "seq_a",
        "operator",
        "t.line",
        "    holds_gameplay: true\n    duration_ticks: 5\n",
    ),
    ("seq_b", "operator", "t.line", "    duration_ticks: 5\n"),
    ("seq_c", "operator", "t.line", "    duration_ticks: 5\n"),
    (
        "hold_call",
        "operator",
        "t.line",
        "    holds_gameplay: true\n    duration_ticks: 6\n",
    ),
    ("bark_ui", "hud", "t.line", "    duration_ticks: 4\n"),
    (
        "crit_ui",
        "hud",
        "t.line",
        "    priority: critical\n    duration_ticks: 4\n",
    ),
    (
        "guard_a_line",
        "guard_a",
        "t.line",
        "    duration_ticks: 10\n",
    ),
    (
        "guard_a_next",
        "guard_a",
        "t.line",
        "    duration_ticks: 10\n",
    ),
    (
        "guard_b_line",
        "guard_b",
        "t.line",
        "    duration_ticks: 10\n",
    ),
    ("timed", "operator", "t.timed", ""),
    (
        "cued",
        "operator",
        "t.line",
        "    duration_ticks: 10\n    cues:\n      first: 0\n      mid: 4\n",
    ),
    ("fr_line", "operator", "t.fr", "    duration_ticks: 10\n"),
];

fn bank() -> String {
    let mut text = SPEAKERS.to_owned();
    for (id, speaker, key, extra) in LINES {
        text.push_str(&format!(
            "  {id}:\n    speaker: {speaker}\n    text: {key}\n{extra}"
        ));
    }
    text
}

/// One entry per tick: the frame, whether gameplay time advanced into it, and its events.
#[derive(Default)]
struct Trace(Vec<(u64, bool, Vec<DialogueEvent>)>);

fn probe(world: &mut World, ctx: &SystemCtx) {
    let events: Vec<DialogueEvent> = world
        .resource::<DialogueEvents>()
        .map(|events| events.iter().cloned().collect())
        .unwrap_or_default();
    if world.resource::<Trace>().is_none() {
        world.insert_resource(Trace::default());
    }
    if let Some(trace) = world.resource_mut::<Trace>() {
        trace
            .0
            .push((ctx.frame.as_u64(), ctx.game_advanced(), events));
    }
}

fn fresh_dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("ludion-dialogue-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn game(name: &str) -> EngineBuilder {
    Engine::builder()
        .project_root(fresh_dir(name))
        .dialogue(DialogueConfig::default())
        .system("dialogue_update", dialogue_update)
        .query::<(&DialogueLine,)>()
        .query::<(&DialogueControl,)>()
        .system("probe", probe)
        .doc_text("strings/en.yaml", STRINGS_EN)
        .doc_text("strings/fr.yaml", STRINGS_FR)
        .doc_text("dialogue/main.yaml", &bank())
}

fn boot(name: &str) -> Engine {
    let mut engine = game(name).build().expect("the bank is valid");
    engine
        .load_inline_scene("format: 1\nseed: 1\nentities: []\n")
        .unwrap();
    engine
}

fn tick(engine: &mut Engine) -> Vec<DialogueEvent> {
    engine
        .execute(Command::SimStep(SimStepParams {
            ticks: 1,
            hash: false,
        }))
        .unwrap();
    engine
        .world()
        .resource::<DialogueEvents>()
        .unwrap()
        .iter()
        .cloned()
        .collect()
}

fn ticks(engine: &mut Engine, n: usize) -> Vec<Vec<DialogueEvent>> {
    (0..n).map(|_| tick(engine)).collect()
}

fn say(engine: &mut Engine, line: &str) -> LineHandle {
    dialogue::say(engine.world_mut(), line, None).unwrap()
}

fn say_by(engine: &mut Engine, line: &str, who: EntityId) -> LineHandle {
    dialogue::say(engine.world_mut(), line, Some(who)).unwrap()
}

fn started_names(events: &[DialogueEvent]) -> Vec<String> {
    events
        .iter()
        .filter_map(|event| match event {
            DialogueEvent::Started { line, .. } => Some(line.clone()),
            _ => None,
        })
        .collect()
}

fn ended_of(events: &[DialogueEvent], handle: LineHandle) -> Option<EndReason> {
    events.iter().find_map(|event| match event {
        DialogueEvent::Ended {
            handle: h, reason, ..
        } if *h == handle => Some(*reason),
        _ => None,
    })
}

fn started_of(events: &[DialogueEvent], handle: LineHandle) -> bool {
    events
        .iter()
        .any(|event| matches!(event, DialogueEvent::Started { handle: h, .. } if *h == handle))
}

/// The index of the first tick where `pred` holds.
fn first(
    per_tick: &[Vec<DialogueEvent>],
    pred: impl Fn(&[DialogueEvent]) -> bool,
) -> Option<usize> {
    per_tick.iter().position(|events| pred(events))
}

fn reported(engine: &Engine, code: ludion_core::diagnostics::DiagnosticCode) -> Vec<String> {
    engine
        .world()
        .resource::<Diagnostics>()
        .unwrap()
        .reported(code)
        .map(String::from)
        .collect()
}

fn subtitles(engine: &Engine) -> Vec<ludion_engine::dialogue::Subtitle> {
    engine
        .world()
        .resource::<Subtitles>()
        .unwrap()
        .iter()
        .cloned()
        .collect()
}

#[test]
fn the_queue_starts_by_priority_then_enqueue_order() {
    let mut engine = boot("order");
    let low = say(&mut engine, "bark_low");
    let norm_a = say(&mut engine, "bark_norm");
    let norm_b = say(&mut engine, "bark_norm");
    let hi = say(&mut engine, "story_hi");
    let run = ticks(&mut engine, 60);
    let at = |handle| first(&run, |events| started_of(events, handle)).unwrap();
    assert_eq!(at(hi), 0, "the high line starts first");
    assert_eq!(at(norm_a), 10, "then the older of the two normal lines");
    assert_eq!(at(norm_b), 20);
    assert_eq!(at(low), 30, "the low line waits for every higher one");
    // The same script again gives the same order.
    let mut again = boot("order-again");
    for line in ["bark_low", "bark_norm", "bark_norm", "story_hi"] {
        say(&mut again, line);
    }
    let rerun = ticks(&mut again, 60);
    assert_eq!(
        run.iter().map(|e| started_names(e)).collect::<Vec<_>>(),
        rerun.iter().map(|e| started_names(e)).collect::<Vec<_>>()
    );
}

#[test]
fn interrupt_rules_cut_by_priority_and_none_waits() {
    let mut engine = boot("interrupt");
    let low = say(&mut engine, "bark_low");
    ticks(&mut engine, 2);
    // `none` waits behind the low line even though it outranks it.
    let polite = say(&mut engine, "bark_norm");
    let run = ticks(&mut engine, 30);
    assert_eq!(first(&run, |e| ended_of(e, low).is_some()), Some(18));
    assert_eq!(ended_of(&run[18], low), Some(EndReason::Completed));
    assert!(
        started_of(&run[18], polite),
        "it starts when the low line completes"
    );

    // `lower` cuts a strictly lower line at once.
    let mut engine = boot("interrupt-lower");
    let low = say(&mut engine, "bark_low");
    ticks(&mut engine, 2);
    let cutter = say(&mut engine, "cut_lower");
    let events = tick(&mut engine);
    assert_eq!(ended_of(&events, low), Some(EndReason::Interrupted));
    assert!(started_of(&events, cutter));

    // `lower` does not cut an equal line.
    let mut engine = boot("interrupt-equal");
    let norm = say(&mut engine, "bark_norm");
    ticks(&mut engine, 2);
    let lower = say(&mut engine, "cut_lower");
    let events = tick(&mut engine);
    assert!(ended_of(&events, norm).is_none() && !started_of(&events, lower));
    assert_eq!(
        dialogue::line_state(engine.world(), lower),
        Some(LineState::Queued)
    );

    // `any` does.
    let mut engine = boot("interrupt-any");
    let norm = say(&mut engine, "bark_norm");
    ticks(&mut engine, 2);
    let any = say(&mut engine, "cut_any");
    let events = tick(&mut engine);
    assert_eq!(ended_of(&events, norm), Some(EndReason::Interrupted));
    assert!(started_of(&events, any));
}

#[test]
fn a_stale_bark_is_dropped_by_max_wait_ticks() {
    let mut engine = boot("stale");
    say(&mut engine, "story_hi");
    ticks(&mut engine, 1);
    let zero = say(&mut engine, "stale");
    let three = say(&mut engine, "stale3");
    let run = ticks(&mut engine, 6);
    assert_eq!(first(&run, |e| ended_of(e, zero).is_some()), Some(0));
    assert_eq!(ended_of(&run[0], zero), Some(EndReason::Dropped));
    // `max_wait_ticks: 3` survives three ticks of waiting and is dropped on the fourth.
    assert_eq!(first(&run, |e| ended_of(e, three).is_some()), Some(3));
    assert_eq!(ended_of(&run[3], three), Some(EndReason::Dropped));
}

#[test]
fn a_blocked_channel_holds_waits_drops_and_cuts() {
    let mut engine = boot("blocked");
    dialogue::block_channel(engine.world_mut(), Channel::Radio, true);
    let waits = say(&mut engine, "bark_norm");
    let drops = say(&mut engine, "drop_blocked");
    let events = tick(&mut engine);
    assert_eq!(ended_of(&events, drops), Some(EndReason::Blocked));
    ticks(&mut engine, 5);
    assert_eq!(
        dialogue::line_state(engine.world(), waits),
        Some(LineState::Queued)
    );
    dialogue::block_channel(engine.world_mut(), Channel::Radio, false);
    assert!(
        started_of(&tick(&mut engine), waits),
        "it starts on unblock"
    );
    // A playing line ends `Blocked` when its channel is blocked.
    ticks(&mut engine, 2);
    dialogue::block_channel(engine.world_mut(), Channel::Radio, true);
    assert_eq!(
        ended_of(&tick(&mut engine), waits),
        Some(EndReason::Blocked)
    );
}

#[test]
fn a_sequence_runs_without_a_gap_and_a_cut_drops_the_rest() {
    let mut engine = boot("sequence");
    let lines = dialogue::say_sequence(
        engine.world_mut(),
        &["seq_a", "seq_b", "seq_c"],
        &[None, None, None],
    )
    .unwrap();
    assert_eq!(
        dialogue::line_state(engine.world(), lines[1]),
        Some(LineState::Pending)
    );
    let run = ticks(&mut engine, 20);
    let a = first(&run, |e| started_of(e, lines[0])).unwrap();
    let b = first(&run, |e| started_of(e, lines[1])).unwrap();
    let c = first(&run, |e| started_of(e, lines[2])).unwrap();
    assert_eq!(
        (b - a, c - b),
        (5, 5),
        "each starts the tick the last completes"
    );
    assert_eq!(ended_of(&run[b], lines[0]), Some(EndReason::Completed));
    assert_eq!(ended_of(&run[c], lines[1]), Some(EndReason::Completed));

    let mut engine = boot("sequence-cut");
    let lines = dialogue::say_sequence(
        engine.world_mut(),
        &["seq_a", "seq_b", "seq_c"],
        &[None, None, None],
    )
    .unwrap();
    ticks(&mut engine, 2);
    assert!(dialogue::interrupt(engine.world_mut(), lines[0]));
    let events = tick(&mut engine);
    assert_eq!(ended_of(&events, lines[0]), Some(EndReason::Interrupted));
    assert_eq!(ended_of(&events, lines[1]), Some(EndReason::Dropped));
    assert_eq!(ended_of(&events, lines[2]), Some(EndReason::Dropped));
    assert!(
        !dialogue::holding(engine.world_mut()),
        "the hold ends with the head"
    );
}

#[test]
fn world_slots_are_per_speaker_and_a_dead_speaker_ends_its_lines() {
    let mut engine = boot("world");
    let a = engine.world_mut().spawn();
    let b = engine.world_mut().spawn();
    let first_a = say_by(&mut engine, "guard_a_line", a);
    let first_b = say_by(&mut engine, "guard_b_line", b);
    let second_a = say_by(&mut engine, "guard_a_next", a);
    let events = tick(&mut engine);
    assert!(started_of(&events, first_a) && started_of(&events, first_b));
    assert_eq!(
        dialogue::line_state(engine.world(), second_a),
        Some(LineState::Queued)
    );
    assert!(dialogue::speaking(engine.world_mut(), "guard_a"));
    assert!(!dialogue::speaking(engine.world_mut(), "operator"));

    engine.world_mut().despawn(a).unwrap();
    let events = tick(&mut engine);
    assert_eq!(ended_of(&events, first_a), Some(EndReason::Interrupted));
    assert_eq!(ended_of(&events, second_a), Some(EndReason::Dropped));
    assert!(ended_of(&events, first_b).is_none());

    // `interrupt_speaker` cuts what a live speaker has.
    assert_eq!(dialogue::interrupt_speaker(engine.world_mut(), b), 1);
    assert_eq!(
        ended_of(&tick(&mut engine), first_b),
        Some(EndReason::Interrupted)
    );
}

#[test]
fn a_holding_line_freezes_gameplay_for_a_documented_window() {
    let mut engine = boot("hold");
    let call = say(&mut engine, "hold_call");
    let bark = say(&mut engine, "bark_ui");
    let crit = say(&mut engine, "crit_ui");
    let run = ticks(&mut engine, 14);
    let start = first(&run, |e| started_of(e, call)).unwrap();
    assert_eq!(start, 0);
    assert!(
        started_of(&run[0], crit),
        "a critical line starts through the hold"
    );
    assert!(
        !started_of(&run[0], bark),
        "a non-critical one waits for it"
    );
    assert_eq!(first(&run, |e| ended_of(e, call).is_some()), Some(6));
    assert!(
        started_of(&run[6], bark),
        "and starts the tick the hold ends"
    );

    // The window: a line that starts on frame F (D ticks long) holds the clock on F..F+D-1, so
    // `game_advanced` is false on F+1..=F+D — D frames, the last being the frame it ends on.
    let trace = &engine.world().resource::<Trace>().unwrap().0;
    let f = trace[0].0;
    for (frame, advanced, _) in trace {
        let frozen = (f + 1..=f + 6).contains(frame);
        assert_eq!(*advanced, !frozen, "frame {frame} (line started on {f})");
    }
}

#[test]
fn timing_is_frame_exact_and_the_reading_rule_sets_the_length() {
    let mut engine = boot("timing");
    let line = say(&mut engine, "bark_norm");
    let started = tick(&mut engine);
    assert!(started_of(&started, line));
    let frame = engine.world().resource::<Trace>().unwrap().0[0].0;
    let shown = subtitles(&engine);
    assert_eq!(shown.len(), 1);
    assert_eq!(
        (shown[0].start_frame, shown[0].end_frame),
        (frame, frame + 10)
    );
    assert_eq!(shown[0].text, "Copy that.");
    assert_eq!(shown[0].speaker, "Operator");
    // Present on F..F+9, ended on F+10.
    let run = ticks(&mut engine, 9);
    assert!(run.iter().all(|events| events.is_empty()));
    assert_eq!(subtitles(&engine).len(), 1, "still shown on F+9");
    let last = tick(&mut engine);
    assert_eq!(subtitles(&engine).len(), 0, "gone on the frame it ends");
    assert_eq!(ended_of(&last, line), Some(EndReason::Completed));

    // No authored length: max(1.5, 1.0 + 0.06 * 10) = 1.6 s = 96 ticks at 60 Hz.
    let mut engine = boot("reading");
    dialogue::say_with(engine.world_mut(), "timed", None, &[("name", "Neon")]).unwrap();
    tick(&mut engine);
    let shown = subtitles(&engine);
    assert_eq!(shown[0].text, "Hello Neon");
    assert_eq!(shown[0].end_frame - shown[0].start_frame, 96);
}

#[test]
fn cues_fire_on_their_tick_and_suspend_freezes_the_line() {
    let mut engine = boot("cues");
    let line = say(&mut engine, "cued");
    let run = ticks(&mut engine, 5);
    let cue = |events: &[DialogueEvent], name: &str| {
        events
            .iter()
            .any(|e| matches!(e, DialogueEvent::Cue { name: n, .. } if n == name))
    };
    assert!(
        started_of(&run[0], line) && cue(&run[0], "first"),
        "`at: 0` fires on the start tick"
    );
    assert!(cue(&run[4], "mid") && !cue(&run[3], "mid"));
    dialogue::suspend(engine.world_mut(), true);
    let frozen = ticks(&mut engine, 4);
    assert!(
        frozen.iter().all(|events| events.is_empty()),
        "no cue repeats while suspended"
    );
    assert_eq!(
        dialogue::line_state(engine.world(), line),
        Some(LineState::Playing)
    );
    dialogue::suspend(engine.world_mut(), false);
    let rest = ticks(&mut engine, 8);
    assert_eq!(
        first(&rest, |e| ended_of(e, line).is_some()),
        Some(5),
        "the same line resumes"
    );
}

#[test]
fn strings_fall_back_and_report_once() {
    let mut engine = boot("strings");
    assert_eq!(localization::text(engine.world_mut(), "t.fr"), "Copy that");
    localization::set_locale(engine.world_mut(), "fr");
    assert_eq!(localization::locale(engine.world_mut()), "fr");
    assert_eq!(localization::text(engine.world_mut(), "t.fr"), "Bien recu");
    for _ in 0..3 {
        assert_eq!(
            localization::text(engine.world_mut(), "t.only_en"),
            "English only"
        );
    }
    assert_eq!(
        reported(&engine, code::STRING_FELL_BACK),
        ["string_fell_back:fr/t.only_en"]
    );
    for _ in 0..3 {
        assert_eq!(localization::text(engine.world_mut(), "no.such"), "no.such");
    }
    assert_eq!(
        reported(&engine, code::STRING_MISSING),
        ["string_missing:no.such"]
    );
}

#[test]
fn placeholders_fill_escape_and_warn_once() {
    let mut engine = boot("placeholders");
    let world = engine.world_mut();
    assert_eq!(
        localization::format_text(world, "t.greet", &[("name", "Neon")]),
        "Hello Neon"
    );
    assert_eq!(
        localization::format_text(world, "t.esc", &[("name", "Neon")]),
        "Use {braces} Neon"
    );
    for _ in 0..3 {
        assert_eq!(
            localization::format_text(world, "t.greet", &[]),
            "Hello {name}"
        );
    }
    assert_eq!(
        reported(&engine, code::STRING_ARG_MISSING),
        ["string_arg:t.greet/name"]
    );
    localization::set_locale(engine.world_mut(), "fr");
    assert_eq!(
        localization::format_text(engine.world_mut(), "t.greet", &[("name", "Neon")]),
        "Bonjour Neon"
    );
}

#[test]
fn set_locale_re_resolves_a_live_subtitle_without_changing_its_length() {
    let mut engine = boot("locale-live");
    say(&mut engine, "fr_line");
    tick(&mut engine);
    let before = subtitles(&engine);
    assert_eq!(before[0].text, "Copy that");
    localization::set_locale(engine.world_mut(), "fr");
    tick(&mut engine);
    let after = subtitles(&engine);
    assert_eq!(after[0].text, "Bien recu");
    assert_eq!(
        (after[0].start_frame, after[0].end_frame),
        (before[0].start_frame, before[0].end_frame)
    );
}

#[test]
fn speakers_carry_their_colour_and_portrait_into_the_subtitle() {
    let mut engine = boot("speaker-fields");
    say(&mut engine, "bark_ui");
    tick(&mut engine);
    let shown = subtitles(&engine);
    assert_eq!(shown[0].color, [1.0, 0.0, 0.0, 1.0]);
    assert_eq!(shown[0].portrait.as_deref(), Some("hud"));
    assert_eq!(shown[0].channel, Channel::Ui);
}

// --- Validation -----------------------------------------------------------------------------

const BASE_DIALOGUE: &str = "format: 1
kind: dialogue
speakers:
  operator:
    name: speaker.operator
    channel: radio
lines:
  intro:
    speaker: operator
    text: t.line
";

const BASE_STRINGS: &str = "format: 1
kind: strings
locale: en
strings:
  speaker.operator: Operator
  t.line: Hello
";

fn bare(name: &str) -> EngineBuilder {
    Engine::builder()
        .project_root(fresh_dir(name))
        .dialogue(DialogueConfig::default())
}

fn failing(builder: EngineBuilder) -> String {
    match builder.build() {
        Ok(_) => panic!("the build should have failed"),
        Err(error) => format!("{error:?}"),
    }
}

#[test]
fn a_clean_bank_builds_with_no_warnings() {
    let engine = bare("clean")
        .doc_text("strings/en.yaml", BASE_STRINGS)
        .doc_text("dialogue/a.yaml", BASE_DIALOGUE)
        .build()
        .unwrap();
    assert!(engine.doc_validation_warnings().is_empty());
}

#[test]
fn each_validator_code_fires_as_specified() {
    let cases: Vec<(&str, EngineBuilder)> = vec![
        (
            "L1711",
            bare("l1711")
                .doc_text("strings/en.yaml", BASE_STRINGS)
                .doc_text("strings/en2.yaml", BASE_STRINGS)
                .doc_text("dialogue/a.yaml", BASE_DIALOGUE),
        ),
        (
            "L1712",
            bare("l1712")
                .doc_text("strings/en.yaml", BASE_STRINGS)
                .doc_text("dialogue/a.yaml", BASE_DIALOGUE)
                .doc_text(
                    "dialogue/b.yaml",
                    "format: 1\nkind: dialogue\nlines:\n  intro:\n    speaker: operator\n    text: t.line\n",
                ),
        ),
        (
            "L1713",
            bare("l1713")
                .doc_text("strings/en.yaml", BASE_STRINGS)
                .doc_text("dialogue/a.yaml", BASE_DIALOGUE)
                .doc_text(
                    "dialogue/b.yaml",
                    "format: 1\nkind: dialogue\nspeakers:\n  operator:\n    name: speaker.operator\n    channel: radio\n",
                ),
        ),
        (
            "L1714",
            bare("l1714")
                .doc_text("strings/en.yaml", BASE_STRINGS)
                .doc_text(
                    "dialogue/a.yaml",
                    "format: 1\nkind: dialogue\nlines:\n  intro:\n    speaker: ghost\n    text: t.line\n",
                ),
        ),
        (
            "L1715",
            bare("l1715")
                .doc_text(
                    "strings/en.yaml",
                    "format: 1\nkind: strings\nlocale: en\nstrings:\n  speaker.operator: Operator\n",
                )
                .doc_text("dialogue/a.yaml", BASE_DIALOGUE),
        ),
        (
            "L1717",
            bare("l1717-zero")
                .doc_text("strings/en.yaml", BASE_STRINGS)
                .doc_text(
                    "dialogue/a.yaml",
                    &format!("{BASE_DIALOGUE}    duration_ticks: 0\n"),
                ),
        ),
        (
            "L1717",
            bare("l1717-cue")
                .doc_text("strings/en.yaml", BASE_STRINGS)
                .doc_text(
                    "dialogue/a.yaml",
                    &format!("{BASE_DIALOGUE}    duration_ticks: 5\n    cues:\n      late: 5\n"),
                ),
        ),
        (
            "L1718",
            bare("l1718")
                .doc_text("strings/en.yaml", BASE_STRINGS)
                .doc_text(
                    "strings/none.yaml",
                    "format: 1\nkind: strings\nlocale: \"\"\nstrings:\n  a: b\n",
                )
                .doc_text("dialogue/a.yaml", BASE_DIALOGUE),
        ),
        (
            "L1120",
            bare("unknown-field")
                .doc_text("strings/en.yaml", BASE_STRINGS)
                .doc_text(
                    "dialogue/a.yaml",
                    &format!("{BASE_DIALOGUE}    loudness: 3\n"),
                ),
        ),
        (
            "L1710",
            Engine::builder()
                .project_root(fresh_dir("l1710"))
                .dialogue(DialogueConfig {
                    base_s: -1.0,
                    ..DialogueConfig::default()
                }),
        ),
        (
            "L1722",
            bare("l1722")
                .doc_text("strings/en.yaml", BASE_STRINGS)
                .doc_text("dialogue/a.yaml", BASE_DIALOGUE)
                .doc_text(
                    "locales/locales.yaml",
                    "format: 1
kind: locales
default: de
locales:
  en:
    name: English
",
                ),
        ),
    ];
    for (code, builder) in cases {
        let message = failing(builder);
        assert!(message.contains(code), "expected {code} in: {message}");
    }
}

#[test]
fn a_placeholder_mismatch_is_a_warning_not_a_refusal() {
    let engine = bare("l1716")
        .doc_text(
            "strings/en.yaml",
            &format!("{BASE_STRINGS}  t.hi: Hi {{name}}\n"),
        )
        .doc_text(
            "strings/fr.yaml",
            "format: 1\nkind: strings\nlocale: fr\nstrings:\n  t.hi: Salut\n",
        )
        .doc_text("dialogue/a.yaml", BASE_DIALOGUE)
        .build()
        .unwrap();
    let warnings: Vec<_> = engine
        .doc_validation_warnings()
        .iter()
        .map(|d| d.code)
        .collect();
    assert_eq!(warnings, [code::STRINGS_PLACEHOLDER_MISMATCH]);
}

#[test]
fn started_carries_the_speaker_and_channel() {
    let mut engine = boot("started-fields");
    let a = engine.world_mut().spawn();
    let radio = say(&mut engine, "bark_low");
    let world_line = say_by(&mut engine, "guard_a_line", a);
    let events = tick(&mut engine);
    let started = |handle: LineHandle| {
        events.iter().find_map(|event| match event {
            DialogueEvent::Started {
                handle: h,
                speaker,
                channel,
                ..
            } if *h == handle => Some((*speaker, *channel)),
            _ => None,
        })
    };
    assert_eq!(started(world_line), Some((Some(a), Channel::World)));
    assert_eq!(started(radio), Some((None, Channel::Radio)));
}

#[test]
fn the_api_refuses_with_its_codes() {
    let mut engine = boot("api-errors");
    let world = engine.world_mut();
    assert_eq!(
        dialogue::say(world, "nope", None).unwrap_err().code,
        code::DIALOGUE_UNKNOWN_LINE
    );
    assert_eq!(
        dialogue::say(world, "guard_a_line", None).unwrap_err().code,
        code::DIALOGUE_NO_SPEAKER
    );
    assert_eq!(
        dialogue::say_sequence(world, &[], &[]).unwrap_err().code,
        code::DIALOGUE_SEQUENCE_INVALID
    );
    assert_eq!(
        dialogue::say_sequence(world, &["seq_a", "seq_b"], &[None])
            .unwrap_err()
            .code,
        code::DIALOGUE_SEQUENCE_INVALID
    );
    // Nothing was queued by any refusal.
    assert_eq!(world.query_ids::<(&DialogueLine,)>().count(), 0);

    let mut plain = Engine::builder()
        .project_root(fresh_dir("not-enabled"))
        .build()
        .unwrap();
    assert_eq!(
        dialogue::say(plain.world_mut(), "intro", None)
            .unwrap_err()
            .code,
        code::DIALOGUE_NOT_ENABLED
    );
}

// --- Snapshots and hashing ------------------------------------------------------------------

fn snapshot_of(engine: &mut Engine) -> Value {
    match engine.execute(Command::EngineSnapshot).unwrap().body {
        ResponseBody::Snapshot { snapshot } => snapshot,
        other => panic!("expected a snapshot, got {other:?}"),
    }
}

fn restore(engine: &mut Engine, snapshot: Value) {
    match engine
        .execute(Command::EngineRestore(EngineRestoreParams { snapshot }))
        .unwrap()
        .body
    {
        ResponseBody::Restored { .. } => {}
        other => panic!("expected a restore, got {other:?}"),
    }
}

fn script(engine: &mut Engine) {
    dialogue::say_sequence(
        engine.world_mut(),
        &["seq_a", "seq_b", "seq_c"],
        &[None, None, None],
    )
    .unwrap();
    say(engine, "cued");
}

fn stream(engine: &mut Engine, n: usize) -> Vec<Vec<(String, String)>> {
    (0..n)
        .map(|_| {
            tick(engine)
                .iter()
                .map(|event| match event {
                    DialogueEvent::Started { line, .. } => ("started".to_owned(), line.clone()),
                    DialogueEvent::Ended { line, reason, .. } => {
                        (format!("ended {reason:?}"), line.clone())
                    }
                    DialogueEvent::Cue { line, name, .. } => (format!("cue {name}"), line.clone()),
                })
                .collect()
        })
        .collect()
}

#[test]
fn a_restore_mid_line_gives_the_same_event_stream() {
    let mut engine = boot("restore");
    script(&mut engine);
    stream(&mut engine, 7);
    let snapshot = snapshot_of(&mut engine);
    let reference = stream(&mut engine, 30);
    assert!(reference.iter().any(|events| !events.is_empty()));
    restore(&mut engine, snapshot);
    assert_eq!(stream(&mut engine, 30), reference);
}

#[test]
fn dialogue_runs_hash_identically_and_an_unused_opt_in_hashes_like_none() {
    let run = |name: &str| {
        let mut engine = boot(name);
        script(&mut engine);
        (0..40)
            .map(|_| {
                tick(&mut engine);
                engine.world_hash().unwrap().as_u64()
            })
            .collect::<Vec<_>>()
    };
    assert_eq!(run("hash-a"), run("hash-b"));

    let quiet = |opted_in: bool| {
        let builder = if opted_in {
            game("quiet-in")
        } else {
            Engine::builder().project_root(fresh_dir("quiet-out"))
        };
        let mut engine = builder.build().unwrap();
        engine
            .load_inline_scene("format: 1\nseed: 1\nentities: []\n")
            .unwrap();
        (0..10)
            .map(|_| {
                engine
                    .execute(Command::SimStep(SimStepParams {
                        ticks: 1,
                        hash: false,
                    }))
                    .unwrap();
                engine.world_hash().unwrap().as_u64()
            })
            .collect::<Vec<_>>()
    };
    assert_eq!(quiet(true), quiet(false));
}
