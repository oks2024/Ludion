//! A nine-slice panel: a textured `UiRect` whose corners keep their size while its edges and centre
//! stretch, authored as a `slice` on the rect.
//!
//! `extract` emits the nine pieces itself; nothing is written per tick. The headless `main` loads the
//! authored scene, then prints the pieces `ludion::math::nine_slice` (the same split `extract` uses)
//! gives the panel at three sizes: the corners stay 8 px, and a rect smaller than its corners
//! shrinks them proportionally.
//!
//! ```text
//! cargo run -p ludion --example ui_nine_slice
//! ```

use ludion::math::nine_slice;
use ludion::prelude::*;
use ludion_engine::components::ui::{UiSlice, UiSliceCenter};
use std::process::ExitCode;

// --- The game (copyable) -------------------------------------------------------------------------

pub fn game() -> EngineBuilder {
    Engine::builder()
}

/// A 24 x 24 texel frame with 8 texel corners, drawn at 2x as a dialogue panel.
const SCENE: &str = "\
format: 1
entities:
  - name: panel
    components:
      UiRect:
        anchor: bottom_center
        offset:
          - 0.0
          - -24.0
        size:
          - 640.0
          - 160.0
        texture: ui/panel.png
        slice:
          border:
            - 8.0
            - 8.0
            - 8.0
            - 8.0
          scale: 2.0
";

// --- Headless demo `main` ------------------------------------------------------------------------

fn main() -> ExitCode {
    let mut engine = match game().build() {
        Ok(engine) => engine,
        Err(diagnostic) => {
            eprintln!("{diagnostic}");
            return ExitCode::FAILURE;
        }
    };
    if let Err(diagnostics) = engine.load_inline_scene(SCENE) {
        for diagnostic in diagnostics {
            eprintln!("{diagnostic}");
        }
        return ExitCode::FAILURE;
    }
    let slice = UiSlice {
        border: [8.0; 4],
        scale: 2.0,
        center: UiSliceCenter::Stretch,
    };
    for size in [[640.0, 160.0], [320.0, 96.0], [24.0, 24.0]] {
        let nine = nine_slice(
            [0.0, 0.0, size[0], size[1]],
            [0.0, 0.0, 1.0, 1.0],
            [24.0, 24.0],
            &slice,
            1.0,
        );
        println!(
            "{} x {}: {} pieces, clamped {}",
            size[0],
            size[1],
            nine.pieces.len(),
            nine.clamped
        );
        for (rect, uv) in &nine.pieces {
            println!("  rect {rect:?}  uv {uv:?}");
        }
    }
    ExitCode::SUCCESS
}
