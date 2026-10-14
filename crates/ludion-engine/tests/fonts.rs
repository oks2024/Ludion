//! The runtime font reporter (`Plan/font-assets.md` decision 8): a `font` naming a face the table
//! lacks is reported `L1254` once per name, and a scene without `font` reports nothing.

use std::path::{Path, PathBuf};
use std::time::Duration;

use crossbeam_channel::{bounded, unbounded};
use ludion_engine::live::{StampedCommand, run_live, shutdown_channel};
use ludion_engine::{Egress, EgressChannels, Engine};
use ludion_protocol::command::{SimStepParams, StreamSubscribeParams, SubscribeMode};
use ludion_protocol::event::{Event, LogEvent};
use ludion_protocol::frame::FrameFormat;
use ludion_protocol::{Channel, Command};

fn root(tag: &str, table: bool) -> PathBuf {
    let dir =
        std::env::temp_dir().join(format!("ludion-fonts-engine-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("fonts")).expect("a temp content root");
    if table {
        let gallery = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples/gallery/fonts");
        for file in ["inter.font.yaml", "inter.png"] {
            std::fs::copy(gallery.join(file), dir.join("fonts").join(file)).expect("copy face");
        }
        std::fs::write(
            dir.join("fonts.yaml"),
            "format: 1\nkind: fonts\nfonts:\n  body: fonts/inter.font.yaml\n",
        )
        .expect("table");
    }
    dir
}

/// Loads `scene`, then steps `ticks` and again `ticks` more on a live sim thread, returning the log
/// lines seen after each batch.
fn log_lines(root: &Path, scene: &str, ticks: u64) -> (Vec<LogEvent>, Vec<LogEvent>) {
    let mut engine = Engine::builder()
        .project_root(root)
        .build()
        .expect("builds");
    engine.load_inline_scene(scene).expect("the scene loads");
    engine.execute(Command::SimPause).expect("pauses");
    let (intake, intake_rx) = unbounded();
    let (shutdown, shutdown_rx) = shutdown_channel();
    let (frames, _frames_rx) = bounded::<Egress>(1);
    let (events, events_rx) = unbounded::<Egress>();
    let handle = std::thread::spawn(move || {
        run_live(
            engine,
            intake_rx,
            shutdown_rx,
            Some(EgressChannels { frames, events }),
        )
    });
    let send = |command: Command| {
        let (stamped, reply) = StampedCommand::new(command);
        intake.send(stamped).unwrap();
        reply
            .recv_timeout(Duration::from_secs(5))
            .expect("the sim thread replies")
            .expect("the command succeeds");
    };
    send(Command::StreamSubscribe(StreamSubscribeParams {
        channel: Channel::Log,
        mode: SubscribeMode::LatestWins { max_fps: None },
        format: FrameFormat::Qoi,
    }));
    let drain = || {
        let mut lines = Vec::new();
        let mut wait = Duration::from_secs(5);
        while let Ok(Egress::Event(event)) = events_rx.recv_timeout(wait) {
            if let Event::Log(log) = event.message.event {
                lines.push(log);
            }
            wait = Duration::from_millis(300);
        }
        lines
    };
    send(Command::SimStep(SimStepParams { ticks, hash: false }));
    let first = drain();
    send(Command::SimStep(SimStepParams { ticks, hash: false }));
    let second = drain();
    shutdown.signal();
    let _ = handle.join();
    (first, second)
}

const NOPE: &str = "format: 1\nentities:\n  - name: menu\n    components:\n      UiList:\n        font: nope\n        items:\n          - text: A\n";

#[test]
fn an_unknown_font_name_is_reported_once() {
    let dir = root("unknown", true);
    let (first, second) = log_lines(&dir, NOPE, 3);
    let count = |lines: &[LogEvent]| lines.iter().filter(|l| code_is(l, "L1254")).count();
    assert_eq!(count(&first), 1, "{first:?}");
    assert!(
        first.iter().any(|l| l.message.contains("nope")),
        "{first:?}"
    );
    assert_eq!(
        count(&second),
        0,
        "still once after a second step: {second:?}"
    );
}

#[test]
fn a_scene_without_font_reports_nothing() {
    let dir = root("plain", false);
    let scene =
        "format: 1\nentities:\n  - name: label\n    components:\n      UiText:\n        text: HI\n";
    let (first, _) = log_lines(&dir, scene, 3);
    assert!(
        !first
            .iter()
            .any(|l| code_is(l, "L1254") || code_is(l, "L1255")),
        "{first:?}"
    );
}

#[test]
fn a_malformed_table_is_reported_as_l1253_once() {
    let dir = root("badtable", false);
    std::fs::write(dir.join("fonts.yaml"), "format: 1\nkind: fonts\nfonts: 3\n").unwrap();
    let scene =
        "format: 1\nentities:\n  - name: label\n    components:\n      UiText:\n        text: HI\n";
    let (first, second) = log_lines(&dir, scene, 3);
    let count = |lines: &[LogEvent]| lines.iter().filter(|l| code_is(l, "L1253")).count();
    assert_eq!(count(&first), 1, "{first:?}");
    assert_eq!(count(&second), 0, "{second:?}");
}

/// The face a measurement helper is handed is the face it measures with: a named face's narrower
/// advances shrink both the text block and the `list_hit` region.
#[test]
fn a_named_face_reaches_measurement() {
    use ludion_engine::FontLibrary;
    use ludion_engine::components::{Anchor, UiList, UiListItem};
    use ludion_engine::ui::{FontFace, list_hit, measure_ui_text, resolve_ui_list};

    let dir = root("measure", true);
    let lib = FontLibrary::load(&dir);
    let embedded = FontFace::embedded();
    let body = lib.face(Some("body"));

    let named = measure_ui_text(body, "Wave To", 24.0);
    let plain = measure_ui_text(embedded, "Wave To", 24.0);
    assert!(named[0] < plain[0], "{named:?} vs {plain:?}");

    let list = UiList {
        anchor: Anchor::TopLeft,
        size: 24.0,
        items: vec![UiListItem {
            text: "Wave To".to_string(),
            enabled: true,
        }],
        ..UiList::default()
    };
    let viewport = (1920, 1080);
    let layout = resolve_ui_list(body, &list, viewport);
    let cursor = [
        layout.origin_px[0] + (named[0] + plain[0]) / 2.0,
        layout.origin_px[1] + 12.0,
    ];
    assert_eq!(list_hit(body, &list, viewport, cursor), None);
    let wide = resolve_ui_list(embedded, &list, viewport);
    let cursor = [
        wide.origin_px[0] + (named[0] + plain[0]) / 2.0,
        wide.origin_px[1] + 12.0,
    ];
    assert_eq!(list_hit(embedded, &list, viewport, cursor), Some(0));
}

fn gallery_scene() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples/gallery/ui-fonts.yaml")
}

/// A proportional list answers `list_hit` over exactly the rows `extract` draws: for every drawn
/// item the hit at the row's vertical middle is that item, and just right of the measured width
/// there is none.
#[test]
fn list_hit_matches_the_drawn_rows_of_a_proportional_list() {
    use ludion_engine::FontLibrary;
    use ludion_engine::components::UiList;
    use ludion_engine::ui::{list_hit, measure_ui_list, resolve_ui_list};
    use ludion_renderer::UiInstance;

    let scene = gallery_scene();
    let mut engine = Engine::builder()
        .load_scene_file(&scene)
        .expect("the gallery scene loads");
    let list = engine
        .world_mut()
        .query::<(&UiList,)>()
        .next()
        .map(|(list,)| list.clone())
        .expect("the scene has a UiList");
    let world = engine.world();
    let library = world
        .resource::<FontLibrary>()
        .expect("the world builds a FontLibrary");
    let face = library.face(list.font.as_deref());
    assert_eq!(list.font.as_deref(), Some("body"));

    let viewport = (1920, 1080);
    let render_scene =
        ludion_engine::extract(world, viewport, scene.parent().unwrap()).expect("extracts");
    let layout = resolve_ui_list(face, &list, viewport);
    let scale = viewport.1 as f32 / 1080.0;

    for (index, item) in list.items.iter().enumerate() {
        let quads = render_scene
            .ui
            .iter()
            .find_map(|instance| match instance {
                UiInstance::Text { text, quads, .. } if *text == item.text => Some(quads),
                _ => None,
            })
            .unwrap_or_else(|| panic!("item {index} is drawn as a text run"));
        let top = quads.iter().map(|q| q.pos[1]).fold(f32::MAX, f32::min);
        let bottom = quads.iter().map(|q| q.pos[3]).fold(f32::MIN, f32::max);
        let middle = (top + bottom) / 2.0;
        assert_eq!(
            list_hit(face, &list, viewport, [layout.origin_px[0] + 1.0, middle]),
            Some(index as u32),
            "item {index} at y = {middle}"
        );
    }

    let right_edge = layout.origin_px[0] + measure_ui_list(face, &list)[0] * scale;
    let middle_y = layout.origin_px[1] + layout.cell_px / 2.0;
    assert_eq!(
        list_hit(face, &list, viewport, [right_edge + 1.0, middle_y]),
        None
    );
}

/// The gallery scene's text layout is sim-visible through measurement only: two engines stepped the
/// same ticks hash identically.
#[test]
fn the_font_scene_hashes_identically_across_two_runs() {
    let hash = || {
        let mut engine = Engine::builder()
            .load_scene_file(&gallery_scene())
            .expect("the gallery scene loads");
        engine
            .execute(Command::SimStep(SimStepParams {
                ticks: 30,
                hash: false,
            }))
            .expect("steps");
        engine.world_hash().expect("hashes")
    };
    assert_eq!(hash(), hash());
}

/// An unknown `font` measures with the table's `default`, exactly as an unset one does.
#[test]
fn an_unknown_font_hits_like_the_default() {
    use ludion_engine::FontLibrary;
    use ludion_engine::components::{Anchor, UiList, UiListItem};
    use ludion_engine::ui::list_hit;

    let dir = root("typo-default", true);
    std::fs::write(
        dir.join("fonts.yaml"),
        "format: 1\nkind: fonts\nfonts:\n  default: fonts/inter.font.yaml\n",
    )
    .expect("table");
    let lib = FontLibrary::load(&dir);
    let list = |font: Option<&str>| UiList {
        anchor: Anchor::TopLeft,
        size: 24.0,
        font: font.map(str::to_string),
        items: vec![UiListItem {
            text: "Wave To".to_string(),
            enabled: true,
        }],
        ..UiList::default()
    };
    let viewport = (1920, 1080);
    for x in (0..400).step_by(8) {
        let cursor = [x as f32, 12.0];
        let typo = list(Some("typo"));
        let plain = list(None);
        assert_eq!(
            list_hit(lib.face(typo.font.as_deref()), &typo, viewport, cursor),
            list_hit(lib.face(plain.font.as_deref()), &plain, viewport, cursor),
            "x = {x}"
        );
    }
}

/// Whether a log record carries the diagnostic `code` (the structured field, not the rendered text).
fn code_is(log: &LogEvent, code: &str) -> bool {
    log.diagnostic
        .as_ref()
        .is_some_and(|d| d.code.as_str() == code)
}

/// An item is one row: a line break inside it folds to a space in measurement, in `list_hit` and in
/// the drawn quads alike. On the embedded face every char, space included, is one 10-unit cell.
#[test]
fn a_line_break_inside_a_ui_list_item_stays_on_its_row() {
    use ludion_ecs::World;
    use ludion_engine::components::{UiList, UiListItem};
    use ludion_engine::extract;
    use ludion_engine::ui::{list_hit, measure_ui_list};
    use ludion_font::FontFace;
    use ludion_renderer::UiInstance;

    let item = |text: String| UiListItem {
        text,
        enabled: true,
    };
    for brk in ["\n", "\r\n", "\r"] {
        let list = UiList {
            items: vec![item(format!("AB{brk}CD")), item("EF".to_string())],
            size: 10.0,
            ..UiList::default()
        };
        let face = FontFace::embedded();
        assert_eq!(measure_ui_list(face, &list), [50.0, 20.0], "{brk:?}");

        let mut world = World::new();
        let entity = world.spawn();
        world.insert(entity, list.clone()).unwrap();
        let scene = extract(&world, (1920, 1080), Path::new(".")).unwrap();
        let texts: Vec<_> = scene
            .ui
            .iter()
            .filter_map(|ui| match ui {
                UiInstance::Text { quads, .. } => Some(quads),
                _ => None,
            })
            .collect();
        assert_eq!(texts.len(), 2, "{brk:?}");
        let first: Vec<_> = texts[0]
            .iter()
            .map(|q| (q.ch, q.pos[0], q.pos[1], q.pos[3]))
            .collect();
        assert_eq!(
            first,
            [
                ('A', 0.0, 0.0, 10.0),
                ('B', 10.0, 0.0, 10.0),
                (' ', 20.0, 0.0, 10.0),
                ('C', 30.0, 0.0, 10.0),
                ('D', 40.0, 0.0, 10.0),
            ],
            "{brk:?}"
        );

        let viewport = (1920, 1080);
        assert_eq!(list_hit(face, &list, viewport, [35.0, 5.0]), Some(0));
        assert_eq!(list_hit(face, &list, viewport, [45.0, 15.0]), Some(1));
        assert_eq!(list_hit(face, &list, viewport, [5.0, 25.0]), None);
    }
}
