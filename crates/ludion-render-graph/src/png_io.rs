//! Deterministic PNG encoding of a [`RenderedImage`].
//!
//! The encoder itself lives in `ludion-assets` ([`ludion_assets::encode_png`]) — the GPU-free assets
//! crate — so `ludion import`'s texture re-encode shares the exact same deterministic writer without
//! depending on wgpu. This module is the thin `RenderedImage` façade over it: two runs of the same
//! scene write the same PNG because identical pixels encode to identical bytes, and nothing here
//! reaches for the wall clock.

use std::path::Path;

use ludion_core::codes::code;
use ludion_core::diagnostics::Diagnostic;

use crate::execute::RenderedImage;

impl RenderedImage {
    /// Encodes this image as an in-memory PNG (RGBA8). Deterministic for identical pixels.
    ///
    /// # Example
    /// ```
    /// use ludion_render_graph::RenderedImage;
    /// let image = RenderedImage { width: 2, height: 1, rgba: vec![255, 0, 0, 255, 0, 255, 0, 255] };
    /// let a = image.encode_png().unwrap();
    /// let b = image.encode_png().unwrap();
    /// assert_eq!(a, b, "PNG encoding is deterministic");
    /// assert_eq!(&a[1..4], b"PNG");
    /// ```
    pub fn encode_png(&self) -> Result<Vec<u8>, Diagnostic> {
        ludion_assets::encode_png(self.width, self.height, &self.rgba).map_err(png_failure)
    }

    /// Encodes this image as a PNG and writes it to `path`.
    ///
    /// # Errors
    /// `L2006` if the PNG cannot be encoded or the file cannot be written.
    pub fn write_png(&self, path: impl AsRef<Path>) -> Result<(), Diagnostic> {
        let path = path.as_ref();
        let bytes = self.encode_png()?;
        ludion_core::atomic::write_atomic(path, bytes)
            .map_err(|error| png_failure(format!("could not write `{}`: {error}", path.display())))
    }
}

fn png_failure(detail: String) -> Diagnostic {
    Diagnostic::new(code::RENDER_PNG_IO, detail)
        .with_help("check the output location is writable and that there is free disk space")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn write_png_replaces_the_target_and_never_rewrites_it_in_place() {
        let dir = std::env::temp_dir().join(format!("ludion-png-io-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let target = dir.join("out.png");
        let alias = dir.join("alias.png");
        std::fs::write(&target, b"old").unwrap();
        std::fs::hard_link(&target, &alias).unwrap();

        let image = RenderedImage {
            width: 2,
            height: 1,
            rgba: vec![255, 0, 0, 255, 0, 255, 0, 255],
        };
        image.write_png(&target).unwrap();

        assert_eq!(std::fs::read(&alias).unwrap(), b"old");
        let bytes = std::fs::read(&target).unwrap();
        assert_eq!(&bytes[1..4], b"PNG");
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
