//! The gallery `debug-shapes` scene's CPU proof: `common::debug_shapes_builder` draws every `DebugPen`
//! primitive under `shapes` in one tick, nothing else draws, and the pen's output is the pinned
//! segment count. CPU-only on purpose: the rendered proof is `gallery.rs`'s `debug-shapes-driven` golden.

mod common;

use std::path::PathBuf;

use ludion_engine::{DebugCategories, Engine};

fn gallery_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../examples/gallery")
}

fn build() -> Engine {
    let mut engine = common::debug_shapes_builder()
        .project_root(gallery_root())
        .build()
        .expect("the shapes engine builds");
    engine
        .load_inline_scene(
            &std::fs::read_to_string(gallery_root().join("debug-shapes.yaml"))
                .expect("the committed scene"),
        )
        .expect("the committed scene loads");
    engine
}

#[test]
fn one_tick_draws_every_primitive_under_shapes() {
    let mut engine = build();
    engine.advance_live();
    assert!(engine.debug_draw_drawn().contains(&"shapes"));

    engine.set_debug_draw(DebugCategories::parse("shapes"));
    let lines = engine.extract_present_scene().unwrap().0.debug_lines;
    assert_eq!(
        lines.len(),
        common::SHAPE_SEGMENTS,
        "the 14 primitive calls push a fixed segment count"
    );
    for (i, color) in common::SHAPE_COLORS.iter().enumerate() {
        // `axes` draws its own fixed red, green and blue, so its palette slot is never seen.
        if i != 4 {
            assert!(
                lines.iter().any(|line| line.color == *color),
                "primitive {i} drew no segment in its colour"
            );
        }
    }
}

#[test]
fn nothing_shows_without_the_category() {
    let mut engine = build();
    engine.advance_live();
    assert!(
        engine
            .extract_present_scene()
            .unwrap()
            .0
            .debug_lines
            .is_empty()
    );
}
