//! Procedural texture recipes for Ludion: a `kind: texture` document of named steps becomes the
//! PBR map set a mesh consumes — `base_color`, `metallic_roughness`, `normal`, `occlusion`,
//! `emissive` (only when authored) and a 16-bit linear `height` — as PNG bytes.
//!
//! Everything is expressed in tile units and tileable by construction (periodic generators,
//! wrap-around filters). Evaluation is CPU `f32`, a pure function of the recipe text, the files it
//! references and the size: the PNG bytes match across Windows x86-64 and macOS arm64 and across
//! thread counts, so the crate holds to the sim-side determinism rules (transcendentals through
//! [`ludion_core::math`], parallelism only through [`ludion_core::par`] over fixed row chunks, no
//! hash-order iteration).
//!
//! Module map, in pipeline order: [`recipe`] parses one document and resolves its step graph;
//! [`ops`] holds the typed op set and field reader; [`resolve`] loads sub-recipes through a
//! [`Resolver`] (or the embedded [`stdlib`]) and types every step; [`eval`] evaluates the graph over [`Image`]s built from
//! [`noise`], [`adjust`] and [`filter`]; [`mod@encode`] writes the PNGs. The crate reads no file on
//! its own — a [`Resolver`] does.
//!
//! # Example
//! ```
//! let recipe = "format: 1
//! kind: texture
//! steps:
//!   cells:
//!     op: noise
//!     type: cellular
//!     scale: 4
//!   tint:
//!     op: colorize
//!     input: cells
//!     stops:
//!       - at: 0.0
//!         color: \"#303030\"
//!       - at: 1.0
//!         color: \"#a09080\"
//! outputs:
//!   base_color: tint
//!   roughness: cells
//!   height: cells
//! ";
//! let resolver = ludion_texgen::FsResolver::new(".");
//! let maps = ludion_texgen::build(recipe, &resolver, 16).unwrap();
//! assert_eq!(&maps.base_color[1..4], b"PNG");
//! assert!(maps.emissive.is_none());
//! ```

#![forbid(unsafe_code)]
#![warn(missing_docs)]

pub mod adjust;
pub mod encode;
pub mod eval;
pub mod fill;
pub mod filter;
pub mod hash;
pub mod image;
pub mod noise;
pub mod ops;
pub mod picture;
pub mod recipe;
pub mod resolve;
pub mod scatter;
pub mod stdlib;
pub mod validate;

pub use crate::encode::{MapSet, encode};
pub use crate::eval::{Maps, evaluate};
pub use crate::image::Image;
pub use crate::recipe::{Recipe, parse};
pub use crate::resolve::{FsResolver, Loaded, Resolver, Source, load, load_with_seed};

use ludion_core::codes::code;
use ludion_core::diagnostics::Diagnostic;

/// The smallest and largest build size.
pub const SIZE_RANGE: std::ops::RangeInclusive<u32> = 8..=8192;

/// Checks that `size` lies in [`SIZE_RANGE`].
///
/// # Errors
/// `L1284` naming the range.
pub fn check_size(size: u32) -> Result<(), Vec<Diagnostic>> {
    if SIZE_RANGE.contains(&size) {
        return Ok(());
    }
    Err(vec![
        Diagnostic::new(
            code::TEXTURE_DATA,
            format!(
                "size {size} is outside {}..={}",
                SIZE_RANGE.start(),
                SIZE_RANGE.end()
            ),
        )
        .with_path("/size")
        .with_help("build at a size such as 512 or 1024"),
    ])
}

/// Checks that every sample of every evaluated map is finite.
///
/// Bounded inputs can still overflow in a step's arithmetic; the PNG encoder would write the
/// result as a black or saturated map.
///
/// # Errors
/// One `L1284` at `/outputs/<key>` naming the first non-finite sample.
pub fn check_finite(maps: &Maps) -> Result<(), Vec<Diagnostic>> {
    let Some((key, at)) = maps.first_non_finite() else {
        return Ok(());
    };
    Err(vec![
        Diagnostic::new(
            code::TEXTURE_DATA,
            format!(
                "the `{key}` map holds a non-finite value at sample {at}: a step's arithmetic overflowed"
            ),
        )
        .with_path(format!("/outputs/{key}"))
        .with_help("bring the extreme values of the steps feeding it into a smaller range"),
    ])
}

/// Loads `root`, evaluates it at `size`×`size` and encodes the maps.
///
/// # Errors
/// Every diagnostic of the recipe and its sub-recipes, or `L1284` for a `size` outside
/// [`SIZE_RANGE`].
pub fn build(root: &str, resolver: &dyn Resolver, size: u32) -> Result<MapSet, Vec<Diagnostic>> {
    check_size(size)?;
    let loaded = load(root, resolver)?;
    let maps = evaluate(&loaded, size, &mut |_, _| {});
    check_finite(&maps)?;
    encode(&maps)
        .map_err(|reason| vec![Diagnostic::new(code::TEXTURE_DATA, reason).with_path("/size")])
}
