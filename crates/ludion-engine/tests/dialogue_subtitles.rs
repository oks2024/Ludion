//! Subtitle filtering (`ludion docs dialogue`, § Events and subtitles): `subtitle_range`, its
//! validation and the `Channel` serde. Every test builds from `doc_text` documents on a fresh temp
//! root.

use std::path::PathBuf;

use ludion_core::codes::code;
use ludion_core::ids::EntityId;
use ludion_engine::dialogue::{self, Channel, DialogueControl, DialogueLine, LineHandle};
use ludion_engine::{
    DialogueConfig, DialogueEvents, Engine, EngineBuilder, Subtitles, dialogue_update,
};
use ludion_protocol::Command;
use ludion_protocol::command::SimStepParams;

/// Entity 0 is the listener, entity 1 a world speaker two units away.
const SCENE: &str = "\
format: 1
entities:
  - name: ear
    components:
      Transform: {}
      AudioListener: {}
  - name: guard
    components:
      Transform:
        translation:
          - 2.0
          - 0.0
          - 0.0
";
const EAR: EntityId = EntityId::new(0, 0);
const GUARD: EntityId = EntityId::new(1, 0);

const STRINGS_EN: &str = "format: 1
kind: strings
locale: en
strings:
  speaker.guard: Guard
  speaker.operator: Operator
  t.line: Halt.
";

fn dialogue_doc(extra: &str, speaker: &str) -> String {
    format!(
        "format: 1
kind: dialogue
speakers:
  guard:
    name: speaker.guard
    channel: world
  operator:
    name: speaker.operator
    channel: radio
lines:
  near:
    speaker: {speaker}
    text: t.line
    duration_ticks: 20
{extra}"
    )
}

fn fresh_dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("ludion-subtitles-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn builder(name: &str, doc: &str) -> EngineBuilder {
    Engine::builder()
        .project_root(fresh_dir(name))
        .dialogue(DialogueConfig::default())
        .system("dialogue_update", dialogue_update)
        .query::<(&DialogueLine,)>()
        .query::<(&DialogueControl,)>()
        .doc_text("strings/en.yaml", STRINGS_EN)
        .doc_text("dialogue/main.yaml", doc)
}

fn boot(name: &str, scene: &str, extra: &str) -> Engine {
    let mut engine = builder(name, &dialogue_doc(extra, "guard"))
        .build()
        .expect("the bank is valid");
    engine.load_inline_scene(scene).unwrap();
    engine
}

fn tick(engine: &mut Engine) {
    engine
        .execute(Command::SimStep(SimStepParams {
            ticks: 1,
            hash: false,
        }))
        .unwrap();
}

fn shown(engine: &Engine) -> usize {
    engine.world().resource::<Subtitles>().unwrap().len()
}

fn move_guard(engine: &mut Engine, who: EntityId, x: f32) {
    engine
        .world_mut()
        .get_mut::<ludion_engine::Transform>(who)
        .unwrap()
        .unwrap()
        .translation = [x, 0.0, 0.0];
}

#[test]
fn a_world_line_is_subtitled_only_within_range() {
    let mut engine = boot("range", SCENE, "    subtitle_range: 5.0\n");
    let handle: LineHandle = dialogue::say(engine.world_mut(), "near", Some(GUARD)).unwrap();
    tick(&mut engine);
    assert_eq!(shown(&engine), 1, "two units away is within 5");

    move_guard(&mut engine, EAR, 20.0);
    tick(&mut engine);
    assert_eq!(shown(&engine), 0, "out of range: the subtitle is filtered");

    move_guard(&mut engine, EAR, 0.0);
    tick(&mut engine);
    assert_eq!(shown(&engine), 1, "back in range");

    // The filter never touches the line itself: it ends on its authored tick.
    let mut ended = None;
    for at in 0..30 {
        tick(&mut engine);
        if engine
            .world()
            .resource::<DialogueEvents>()
            .unwrap()
            .ended(handle)
            .is_some()
        {
            ended = Some(at);
            break;
        }
    }
    assert_eq!(
        ended,
        Some(17),
        "20 ticks from its start, as without a range"
    );
}

#[test]
fn no_listener_and_no_camera_shows_the_line() {
    let scene = "format: 1
entities:
  - name: guard
    components:
      Transform:
        translation:
          - 90.0
          - 0.0
          - 0.0
";
    let mut engine = boot("no-ear", scene, "    subtitle_range: 1.0\n");
    dialogue::say(engine.world_mut(), "near", Some(EntityId::new(0, 0))).unwrap();
    tick(&mut engine);
    assert_eq!(shown(&engine), 1);
}

#[test]
fn a_bad_subtitle_range_is_l1717() {
    for (name, extra, speaker) in [
        ("zero", "    subtitle_range: 0.0\n", "guard"),
        ("negative", "    subtitle_range: -2.0\n", "guard"),
        ("radio", "    subtitle_range: 4.0\n", "operator"),
    ] {
        let result = builder(name, &dialogue_doc(extra, speaker)).build();
        let message = match result {
            Ok(_) => panic!("{name}: the build should have failed"),
            Err(error) => format!("{error:?}"),
        };
        assert!(
            message.contains(code::DIALOGUE_LINE_INVALID.as_str()),
            "{name}: expected L1717 in {message}"
        );
        assert!(message.contains("subtitle_range"), "{name}: {message}");
    }
}

#[test]
fn channel_round_trips_through_a_line_as_snake_case() {
    assert_eq!(serde_json::to_value(Channel::Radio).unwrap(), "radio");
    let mut engine = boot("serde", SCENE, "");
    let handle = dialogue::say(engine.world_mut(), "near", Some(GUARD)).unwrap();
    let line = engine
        .world()
        .get::<DialogueLine>(handle.0)
        .unwrap()
        .unwrap()
        .clone();
    let value = serde_json::to_value(&line).unwrap();
    assert_eq!(value["channel"], "world");
    let back: DialogueLine = serde_json::from_value(value).unwrap();
    assert_eq!(back.channel, Channel::World);
}
