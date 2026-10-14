//! `render.debug`: choose which debug-draw categories the host renders, and see which ones the
//! scene actually draws.
//!
//! One [`Command::RenderDebug`] verb, two wire shapes: `{}` reads, `{"categories": [...]}` sets
//! (`[]` none, `"all"` every category). Both answer with `enabled` (the set in force) and `drawn`
//! (the categories the last tick emitted lines for). The set is **host render state** — it moves
//! what the stream and `render.screenshot` draw, never the world, so it is never hashed, snapshotted
//! or recorded.
//!
//! Run it:
//!
//! ```text
//! cargo run -p ludion-engine --example render_debug
//! ```

use std::process::ExitCode;

use ludion_ecs::World;
use ludion_engine::{Engine, debug_draw, dispatch};
use ludion_protocol::command::RenderDebugParams;
use ludion_protocol::{Command, ResponseBody};

fn marker(world: &mut World) {
    debug_draw(world, "path").line([0.0; 3], [0.0, 1.0, 0.0], [0.0, 1.0, 0.0, 1.0]);
}

fn main() -> ExitCode {
    let mut engine = Engine::builder()
        .system("marker", |world, _| marker(world))
        .build()
        .expect("the engine builds");
    if let Err(diagnostics) = engine.load_inline_scene("format: 1\nentities: []\n") {
        for diagnostic in diagnostics {
            eprintln!("{diagnostic}");
        }
        return ExitCode::FAILURE;
    }
    engine.advance_live();

    // `{}` — a pure read: nothing is enabled, but the tick already drew under `path`.
    let (enabled, drawn) = render_debug(&mut engine, None);
    println!("read        -> enabled {enabled:?}, drawn {drawn:?}");
    assert!(enabled.is_empty());
    assert_eq!(drawn, ["path"]);

    // Set one category; naming one nobody draws is accepted and simply never shows in `drawn`.
    let (enabled, drawn) = render_debug(&mut engine, Some(&["path", "motor"]));
    println!("set         -> enabled {enabled:?}, drawn {drawn:?}");
    assert_eq!(enabled, ["motor", "path"]);
    assert!(!drawn.contains(&"motor".to_string()));

    // `[]` turns everything off again.
    let (enabled, _) = render_debug(&mut engine, Some(&[]));
    println!("cleared     -> enabled {enabled:?}");
    assert!(enabled.is_empty());

    ExitCode::SUCCESS
}

fn render_debug(engine: &mut Engine, categories: Option<&[&str]>) -> (Vec<String>, Vec<String>) {
    let categories = categories.map(|names| names.iter().map(|n| n.to_string()).collect());
    match dispatch(
        engine,
        Command::RenderDebug(RenderDebugParams { categories }),
    )
    .expect("render.debug is always accepted")
    .body
    {
        ResponseBody::RenderDebug { enabled, drawn } => (enabled, drawn),
        other => panic!("expected RenderDebug, got {other:?}"),
    }
}
