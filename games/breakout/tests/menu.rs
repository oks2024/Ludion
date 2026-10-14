//! The pause menu, driven headlessly through scripted input — the non-roguelike proof that `UiList`
//! is a genre-generic widget.
//!
//! Covers the whole surface: `Escape` opens and closes it, every gameplay system freezes while it is
//! open (asserted twice — once on the visible state, once on the `WorldHash` itself), the cursor
//! wraps and skips nothing, `RESTART` re-instantiates the scene, and all three input devices —
//! keyboard, gamepad, mouse — reach the same two rows.

use std::path::PathBuf;

use breakout::{Ball, Brick, MENU_RESTART, MENU_RESUME, Paddle, PauseMenu};
use ludion::math::FontFace;
use ludion::prelude::*;
use ludion_testkit::GameplayDriver;

/// The game scene, embedded so these tests are self-contained.
const MAIN_SCENE: &str = include_str!("../scenes/main.yaml");

/// The ball's authored position — what a restart must put it back at, read off `main.yaml`.
const AUTHORED_BALL: [f32; 3] = [0.0, -2.5, 0.0];
/// The paddle's authored x — likewise.
const AUTHORED_PADDLE_X: f32 = 0.0;
/// The board's full brick count.
const BRICKS: usize = 15;

/// A fresh driver rooted at `scenes/`, so `RESTART`'s scene switch resolves `main.yaml` off disk
/// exactly as the live game does (the project root is the started scene's directory).
fn driver() -> GameplayDriver {
    GameplayDriver::new(
        breakout::game().project_root(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("scenes")),
        MAIN_SCENE,
    )
}

/// The pause menu's list, cloned out of the world.
fn menu(d: &mut GameplayDriver) -> UiList {
    d.engine_mut()
        .world_mut()
        .query::<(&PauseMenu, &UiList)>()
        .next()
        .map(|(_m, list)| list.clone())
        .expect("the scene has a PauseMenu entity")
}

/// The menu's item texts, top to bottom.
fn items(d: &mut GameplayDriver) -> Vec<String> {
    menu(d).items.into_iter().map(|item| item.text).collect()
}

/// The menu's cursor.
fn selected(d: &mut GameplayDriver) -> u32 {
    menu(d).selected
}

/// How many bricks are still alive.
fn brick_count(d: &mut GameplayDriver) -> usize {
    d.engine_mut().world_mut().query::<(&Brick,)>().count()
}

/// The ball's translation.
fn ball_position(d: &mut GameplayDriver) -> [f32; 3] {
    d.engine_mut()
        .world_mut()
        .query::<(&Ball, &Transform)>()
        .next()
        .map(|(_b, t)| t.translation)
        .expect("the scene has a Ball entity")
}

/// The paddle's x position.
fn paddle_x(d: &mut GameplayDriver) -> f32 {
    d.engine_mut()
        .world_mut()
        .query::<(&Paddle, &Transform)>()
        .next()
        .map(|(_p, t)| t.translation[0])
        .expect("the scene has a Paddle entity")
}

/// Injects one already-typed [`InputEvent`] (the mouse/gamepad seam the driver's keyboard-only
/// `inject` has no shorthand for).
fn inject(d: &mut GameplayDriver, event: InputEvent) {
    d.inject_value(serde_json::to_value(event).expect("an InputEvent serializes"));
}

/// Plays long enough that the board is provably chipped — the honest precondition for asserting a
/// restart rebuilt it.
fn play_until_bricks_break(d: &mut GameplayDriver) {
    d.step(600); // ten seconds; the serve aims at the bricks
    assert!(
        brick_count(d) < BRICKS,
        "the test needs a chipped board before restarting, still {} bricks",
        brick_count(d)
    );
}

/// Presses `Enter` and steps exactly one tick: the switch is performed at that tick's frame
/// boundary, so the world read afterwards is the freshly instantiated scene with no tick on it.
fn activate(d: &mut GameplayDriver) {
    d.inject("key_down", "Enter");
    d.step(1);
}

/// Asserts the world is the scene as authored, and that the switch itself raised nothing.
fn assert_restarted(d: &mut GameplayDriver) {
    assert_eq!(
        d.engine()
            .last_transition_error()
            .map(|e| e.code.to_string()),
        None,
        "the restart switch must be clean"
    );
    assert_eq!(brick_count(d), BRICKS, "a restart rebuilds the whole board");
    assert!(items(d).is_empty(), "the fresh scene's menu is closed");
    assert_eq!(ball_position(d), AUTHORED_BALL, "the ball is re-authored");
    assert_eq!(paddle_x(d), AUTHORED_PADDLE_X, "the paddle is re-authored");
}

// --- Open / close / freeze --------------------------------------------------------------------------

/// `Escape` opens the menu and the whole sim stops; `Escape` again closes it and play resumes.
#[test]
fn escape_opens_the_menu_and_freezes_the_sim() {
    let mut d = driver();
    assert!(items(&mut d).is_empty(), "the boot frame draws no menu");

    d.tap("Escape");
    assert_eq!(items(&mut d), vec![MENU_RESUME, MENU_RESTART]);
    assert_eq!(selected(&mut d), 0, "the cursor opens on the first row");

    let ball = ball_position(&mut d);
    let paddle = paddle_x(&mut d);
    d.hold("Right", 20);
    assert_eq!(
        paddle_x(&mut d),
        paddle,
        "the paddle is frozen while paused"
    );
    assert_eq!(
        ball_position(&mut d),
        ball,
        "the ball is frozen while paused"
    );
    d.release("Right");

    d.tap("Escape");
    assert!(items(&mut d).is_empty(), "Escape closes the menu");
    d.step(10);
    assert_ne!(ball_position(&mut d), ball, "play resumes on close");
}

/// The freeze is total, not merely visible: an open menu churns no hashed state at all, so two
/// consecutive paused ticks produce the identical `WorldHash`.
#[test]
fn the_world_hash_holds_still_while_paused() {
    let mut d = driver();
    d.step(120); // play a little first, so the frozen state is not just the boot pose
    d.tap("Escape");
    assert_eq!(items(&mut d), vec![MENU_RESUME, MENU_RESTART]);

    let hash = |d: &mut GameplayDriver| d.engine_mut().world_hash().expect("the world hashes");
    let first = hash(&mut d);
    d.step(1);
    let second = hash(&mut d);
    d.step(1);
    let third = hash(&mut d);
    assert_eq!(first, second, "a paused tick must change nothing");
    assert_eq!(second, third, "and keep changing nothing");
}

// --- Activation, by every device --------------------------------------------------------------------

/// `Down` then `Enter` picks `RESTART`, and the scene is re-instantiated: full board, fresh ball,
/// fresh paddle, closed menu.
#[test]
fn down_then_enter_restarts_the_scene() {
    let mut d = driver();
    play_until_bricks_break(&mut d);

    d.tap("Escape");
    d.tap("Down");
    assert_eq!(selected(&mut d), 1, "Down lands on RESTART");
    activate(&mut d);
    assert_restarted(&mut d);
}

/// The same route over the controller alone — `Start`, `DpadDown`, `South` — the widget's
/// gamepad bindings driving a real game.
#[test]
fn the_gamepad_drives_the_menu() {
    let mut d = driver();
    play_until_bricks_break(&mut d);

    inject(
        &mut d,
        InputEvent::GamepadButtonDown {
            button: GamepadButton::Start,
        },
    );
    d.step(1);
    inject(
        &mut d,
        InputEvent::GamepadButtonUp {
            button: GamepadButton::Start,
        },
    );
    d.step(1);
    assert_eq!(items(&mut d), vec![MENU_RESUME, MENU_RESTART]);

    inject(
        &mut d,
        InputEvent::GamepadButtonDown {
            button: GamepadButton::DpadDown,
        },
    );
    d.step(1);
    inject(
        &mut d,
        InputEvent::GamepadButtonUp {
            button: GamepadButton::DpadDown,
        },
    );
    d.step(1);
    assert_eq!(selected(&mut d), 1, "DpadDown lands on RESTART");

    inject(
        &mut d,
        InputEvent::GamepadButtonDown {
            button: GamepadButton::South,
        },
    );
    d.step(1);
    assert_restarted(&mut d);
}

/// The cursor wraps at both ends, and `Space` activates `RESUME` — which hands play back untouched.
#[test]
fn menu_navigation_wraps_and_the_resume_row_resumes() {
    let mut d = driver();
    play_until_bricks_break(&mut d);
    let bricks = brick_count(&mut d);

    d.tap("Escape");
    d.tap("Up");
    assert_eq!(
        selected(&mut d),
        1,
        "Up from the first row wraps to the last"
    );
    d.tap("Up");
    assert_eq!(selected(&mut d), 0, "and wraps back around");

    d.inject("key_down", "Space");
    d.step(1);
    assert!(items(&mut d).is_empty(), "RESUME closes the menu");
    assert_eq!(brick_count(&mut d), bricks, "RESUME rebuilds nothing");
}

// --- Pointer ----------------------------------------------------------------------------------------

/// The session's live render extent — the space the cursor pixels these tests inject live in, read
/// from the world exactly as `menu_input` reads it.
fn viewport(d: &mut GameplayDriver) -> (u32, u32) {
    d.engine_mut()
        .world_mut()
        .resource::<RenderExtent>()
        .copied()
        .unwrap_or_default()
        .size()
}

/// The face the menu draws and hit-tests with, resolved through the world's `FontLibrary` exactly as
/// `menu_input` does.
fn menu_face(d: &mut GameplayDriver) -> FontFace {
    let list = menu(d);
    let fonts = d
        .engine_mut()
        .world_mut()
        .resource::<FontLibrary>()
        .cloned()
        .unwrap_or_default();
    fonts.face(list.font.as_deref()).clone()
}

/// The first cursor `y` (scanning down the canvas at `x`) that lands on `item`, at `viewport`.
fn first_row_y(face: &FontFace, list: &UiList, viewport: (u32, u32), x: f32, item: u32) -> f32 {
    (0..viewport.1)
        .map(|y| y as f32)
        .find(|&y| ludion::math::list_hit(face, list, viewport, [x, y]) == Some(item))
        .unwrap_or_else(|| panic!("item {item} is on screen somewhere"))
}

/// A click on the `RESTART` row activates it, and a click on the title line — inside the panel but
/// on nothing selectable — does not. The rows are located with the same `list_hit` the game calls,
/// so the test never hard-codes the layout.
#[test]
fn a_click_on_the_restart_row_restarts() {
    let mut d = driver();
    play_until_bricks_break(&mut d);
    d.tap("Escape");

    let view = viewport(&mut d);
    let list = menu(&mut d);
    let face = menu_face(&mut d);
    let row_resume = first_row_y(&face, &list, view, 640.0, 0);
    let row_restart = first_row_y(&face, &list, view, 640.0, 1);
    // Lines advance uniformly, so one advance above the first item is the title's own line.
    let title_y = row_resume - (row_restart - row_resume);
    assert!(title_y >= 0.0, "the title line is on screen");
    assert_eq!(
        ludion::math::list_hit(&face, &list, view, [640.0, title_y]),
        None,
        "the title is inside the panel but is not selectable"
    );

    // A click on the title changes nothing: menu still open, cursor still on RESUME.
    inject(
        &mut d,
        InputEvent::MouseMove {
            position: [640.0, title_y],
        },
    );
    inject(
        &mut d,
        InputEvent::MouseDown {
            button: MouseButton::Left,
        },
    );
    d.step(1);
    assert_eq!(
        items(&mut d),
        vec![MENU_RESUME, MENU_RESTART],
        "a click on the title leaves the menu open"
    );
    assert_eq!(selected(&mut d), 0, "and leaves the cursor where it was");
    inject(
        &mut d,
        InputEvent::MouseUp {
            button: MouseButton::Left,
        },
    );
    d.step(1);

    // A click on the RESTART row activates it, cursor included.
    inject(
        &mut d,
        InputEvent::MouseMove {
            position: [640.0, row_restart],
        },
    );
    inject(
        &mut d,
        InputEvent::MouseDown {
            button: MouseButton::Left,
        },
    );
    d.step(1);
    assert_restarted(&mut d);
}

/// A host at a non-default extent maps a click correctly: the `RESTART` row is where `list_hit`
/// resolved *at that extent* says it is, and the row's default-extent pixel is not it. The extent is
/// host state, so nothing about the sim moves with it — only the pointer mapping does.
#[test]
fn a_click_at_a_non_default_extent_resolves_against_that_extent() {
    let mut d = driver();
    d.engine_mut().set_render_size(640, 360);
    assert_eq!(viewport(&mut d), (640, 360), "the setter reaches the world");
    play_until_bricks_break(&mut d);

    d.tap("Escape");
    let list = menu(&mut d);
    let face = menu_face(&mut d);
    let resized = first_row_y(&face, &list, (640, 360), 320.0, 1);
    let default_extent = first_row_y(&face, &list, RenderExtent::default().size(), 640.0, 1);
    assert_ne!(
        resized, default_extent,
        "the two extents put the row on different pixels"
    );
    assert_eq!(
        ludion::math::list_hit(&face, &list, (640, 360), [320.0, default_extent]),
        None,
        "the default-extent row pixel is off the resized menu"
    );

    inject(
        &mut d,
        InputEvent::MouseMove {
            position: [320.0, resized],
        },
    );
    inject(
        &mut d,
        InputEvent::MouseDown {
            button: MouseButton::Left,
        },
    );
    d.step(1);
    assert_restarted(&mut d);
}

/// The menu draws a proportional face, so a click right of the measured text block misses a row the
/// 8x8 face's wider block would have hit — and the whole input session, recorded, replays to the same
/// `WorldHash` trace, so the proportional hit test is deterministic.
#[test]
fn a_click_lands_by_proportional_width_and_replays() {
    let mut probe = driver();
    probe.tap("Escape");
    let view = viewport(&mut probe);
    let list = menu(&mut probe);
    let face = menu_face(&mut probe);
    assert_ne!(
        face.document().image,
        FontFace::embedded().document().image,
        "the menu resolves to its baked Inter, not the embedded face"
    );
    let hit = |face: &FontFace, x: f32, y: f32| ludion::math::list_hit(face, &list, view, [x, y]);

    let y = first_row_y(&face, &list, view, 640.0, 1);
    let row_xs: Vec<f32> = (0..view.0)
        .map(|x| x as f32)
        .filter(|&x| hit(&face, x, y) == Some(1))
        .collect();
    let (left, right) = (row_xs[0], row_xs[row_xs.len() - 1]);
    let centre = f32::midpoint(left, right).floor();
    let x_out = (right as u32 + 1..view.0)
        .map(|x| x as f32)
        .find(|&x| hit(&face, x, y).is_none() && hit(FontFace::embedded(), x, y) == Some(1))
        .expect("the embedded face's wider block reaches a pixel Inter's measured block does not");

    let command = |event: InputEvent| {
        Command::InputInject(command::InputInjectParams {
            frame: None,
            event: serde_json::to_value(event).expect("an InputEvent serializes"),
        })
    };
    let key = |kind: &str| {
        Command::InputInject(command::InputInjectParams {
            frame: None,
            event: serde_json::json!({"kind": kind, "key": "Escape"}),
        })
    };
    let step = |ticks| Command::SimStep(command::SimStepParams { ticks, hash: true });
    let click = |x: f32| {
        vec![
            command(InputEvent::MouseMove { position: [x, y] }),
            command(InputEvent::MouseDown {
                button: MouseButton::Left,
            }),
            step(1),
            command(InputEvent::MouseUp {
                button: MouseButton::Left,
            }),
            step(1),
        ]
    };

    let scenes = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("scenes");
    let fresh = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("breakout-menu-click.lrp");
    let mut engine =
        ludion_testkit::loaded_engine(breakout::game().project_root(&scenes), MAIN_SCENE);
    engine
        .set_recording(std::fs::File::create(&fresh).expect("the recording is writable"))
        .expect("recording arms");

    let mut open = vec![key("key_down"), step(1), key("key_up"), step(1)];
    open.extend(click(x_out));
    let mut live = ludion_engine::drive_hashes(&mut engine, open).expect("the opening runs");
    let menu_items = |engine: &mut Engine| {
        engine
            .world_mut()
            .query::<(&PauseMenu, &UiList)>()
            .next()
            .map(|(_m, list)| list.items.len())
    };
    assert_eq!(
        menu_items(&mut engine),
        Some(2),
        "a click past the proportional block leaves the menu open"
    );
    assert_eq!(engine.world_mut().query::<(&Brick,)>().count(), BRICKS);

    live.extend(ludion_engine::drive_hashes(&mut engine, click(centre)).expect("the click runs"));
    assert_eq!(
        menu_items(&mut engine),
        Some(0),
        "a click on the measured centre restarts the scene, closing the menu"
    );
    engine.stop_recording().expect("recording flushes");
    // Each drive ends by sampling the frame it stops on, so the join repeats one entry.
    live.dedup();

    let bytes = std::fs::read(&fresh).expect("the recording reads back");
    let replayed = ludion_engine::replay_hashes_with(breakout::game(), &bytes, &scenes)
        .expect("the recording replays");
    assert_eq!(
        live, replayed,
        "the replayed session reproduces the live trace"
    );
}
