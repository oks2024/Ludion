//! Measuring, wrapping and hit-testing text with a baked TrueType face — the copyable pattern for
//! every game that sets a `font:` on its text (`ludion docs fonts`).
//!
//! A face is a name in the content root's `fonts.yaml`; [`FontLibrary`] loads the table and hands
//! back the [`FontFace`] a component's `font` names. Every measurement helper takes that face first,
//! so a game measures with the face the renderer draws with. A face the table lacks (or one that
//! failed to load) resolves to the embedded 8x8 face, never an error — which is why `main` compares
//! widths to prove `body` really loaded.
//!
//! Everything above the `main` divider is copyable as-is. Run it headless:
//!
//! ```text
//! cargo run -p ludion --example font_text
//! ```

use ludion::math::{FontFace, list_hit, measure_ui_text, wrap_ui_text};
use ludion::prelude::*;
use std::path::Path;
use std::process::ExitCode;

// --- The game (copyable) -------------------------------------------------------------------------

/// The width of `text` at `size` in the named face, in canvas units.
fn text_width(library: &FontLibrary, font: Option<&str>, text: &str, size: f32) -> f32 {
    measure_ui_text(library.face(font), text, size)[0]
}

/// Wraps `text` to `wrap_width` canvas units with the named face — the same wrap `UiText` draws.
fn wrapped(library: &FontLibrary, font: &str, text: &str, size: f32, wrap_width: f32) -> String {
    wrap_ui_text(library.face(Some(font)), text, size, wrap_width).into_owned()
}

/// The item under `cursor` (render-target pixels) in a list drawn with `font`, at `viewport`.
fn hovered_item(
    library: &FontLibrary,
    list: &UiList,
    viewport: (u32, u32),
    cursor: [f32; 2],
) -> Option<u32> {
    let face: &FontFace = library.face(list.font.as_deref());
    list_hit(face, list, viewport, cursor)
}

// --- Headless demo `main` ------------------------------------------------------------------------

fn main() -> ExitCode {
    let root = Path::new(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../examples/gallery"
    ));
    let library = FontLibrary::load(root);

    let text = "Wave To Yo";
    let body = text_width(&library, Some("body"), text, 24.0);
    let embedded = text_width(&library, None, text, 24.0);
    println!("{text:?} at size 24: body {body:.1}, embedded {embedded:.1}");
    if (body - embedded).abs() < f32::EPSILON {
        eprintln!("the `body` face did not load from {}", root.display());
        return ExitCode::FAILURE;
    }

    let paragraph = "The mayor didn't pay: we never ordered a lamp-post, she said.";
    println!(
        "wrapped to 400 units:\n{}",
        wrapped(&library, "body", paragraph, 24.0, 400.0)
    );

    let item = |text: &str| UiListItem {
        text: text.to_string(),
        enabled: true,
    };
    let list = UiList {
        font: Some("body".to_string()),
        size: 24.0,
        items: vec![item("Glazier"), item("Lamp-post")],
        ..UiList::default()
    };
    // Top-left list, no title: item 0 fills y in [0, 24) and item 1 [24, 48) at this 1920x1080 scale.
    let viewport = (1920, 1080);
    println!(
        "cursor on row 1: {:?}, below the list: {:?}",
        hovered_item(&library, &list, viewport, [4.0, 30.0]),
        hovered_item(&library, &list, viewport, [4.0, 80.0]),
    );

    ExitCode::SUCCESS
}
