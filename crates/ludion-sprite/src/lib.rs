//! Sprite baking for Ludion: `kind: sprite` documents → pixels, pixels → a packed atlas, and PNGs
//! → `kind: sprite` documents.
//!
//! This crate is **offline tooling**. It turns text into the binary artifacts a repository commits
//! (`<atlas>.png` beside its `kind: atlas` document) and back again; nothing here runs in a frame.
//! It reads no files and writes none — the CLI resolves paths, loads inputs and writes results, so
//! everything below is a pure function of its arguments.
//!
//! Three entry points, in pipeline order:
//!
//! 1. [`bake()`] — a [`SpriteDocument`](ludion_scene::SpriteDocument) becomes one [`FrameImage`] per
//!    frame: the glyph grid painted from the palette, then the `effects:` list applied in order.
//! 2. [`pack()`] — those frames (plus any loose PNGs the caller decoded) become one packed image and
//!    the derived [`AtlasDocument`](ludion_scene::AtlasDocument) that indexes it.
//! 3. [`import_png()`] — the inverse of the first step for art authored elsewhere: a decoded PNG
//!    becomes an editable sprite document, losslessly when its colours fit the glyph budget.
//!
//! # Bit-reproducibility
//!
//! A packed atlas is a **committed** artifact that CI regenerates on Windows x86-64 and macOS arm64
//! and byte-compares (`generate-atlas --check`). Every function here is therefore held to the
//! determinism rules the sim path uses and then some: `f32`/`f64` `+ - * /` and integer arithmetic
//! only, every transcendental through [`ludion_core::math`], no `mul_add`, no hash-container
//! iteration, no reduction whose order depends on anything but texel order, and noise from an
//! explicit integer hash of the texel's address rather than a sequential generator. Sorts carry an
//! explicit total order with a name tie-break. A change to any of that re-bakes every committed
//! atlas, which is why the packer's layout and the noise mixer are pinned by tests.
//!
//! # Example
//!
//! ```
//! let source = "\
//! format: 1
//! kind: sprite
//! size:
//!   - 2
//!   - 2
//! palette:
//!   A:
//!     color:
//!       - 1.0
//!       - 0.0
//!       - 0.0
//!       - 1.0
//! frames:
//!   idle:
//!     rows:
//!       - \"A.\"
//!       - \".A\"
//! ";
//! let doc = ludion_scene::check_sprite(source).expect("the sprite is valid");
//! let frames = ludion_sprite::bake(&doc).expect("the sprite bakes");
//! assert_eq!(frames.len(), 1);
//! assert_eq!(frames[0].name, "idle");
//! assert_eq!(frames[0].pivot, [1.0, 1.0]);
//! assert_eq!(frames[0].image.get(0, 0), [1.0, 0.0, 0.0, 1.0]);
//! assert_eq!(frames[0].image.get(1, 0), [0.0, 0.0, 0.0, 0.0]);
//! ```

#![forbid(unsafe_code)]
#![warn(missing_docs)]

mod bake;
mod image;
mod import;
mod pack;

pub use bake::{BakeStep, BakedFrame, FrameSteps, bake, bake_steps};
pub use image::{FrameImage, srgb_decode, srgb_decode_f64, srgb_encode};
pub use import::{IMPORT_GLYPHS, ImportedSprite, QuantizeReport, import_png};
pub use pack::{AtlasBake, AtlasInput, AtlasInputFrame, pack};

use ludion_core::codes::code;
use ludion_core::diagnostics::Diagnostic;

/// The `L1153` this crate raises for a defect `ludion-scene` cannot see — one only the bake knows,
/// because only the bake has read the inputs (a duplicate region name, an unresolvable flipbook
/// frame, an image too large to be a sprite).
fn structure_error(
    message: impl Into<String>,
    path: impl Into<String>,
    help: impl Into<String>,
) -> Diagnostic {
    Diagnostic::new(code::SPRITE_DOCUMENT_STRUCTURE, message)
        .with_path(path)
        .with_help(help)
}
