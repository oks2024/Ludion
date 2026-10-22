#![forbid(unsafe_code)]
#![warn(missing_docs)]

//! `ludion-testkit` — the golden-image harness: SSIM compare, structured diff reports, bless flow,
//! and sim assertions (build-order step 7, `engine-architecture.md` §10; Phase 2 exit criterion in
//! `implementation-guide.md`).
//!
//! Per `CLAUDE.md`'s tiered-determinism rule, CPU-side state is bit-exact but **images gate on
//! perceptual comparison** (SSIM + tolerance) with a structured diff report, because bit-exact
//! image goldens only hold on a pinned GPU. This crate is the machinery that makes that claim
//! checkable by an agent with no eyes: [`ssim::compare`] scores two images and localizes any damage
//! to a tile of the image (with a coarse human name, e.g. `"top-left"`), [`report::DiffReport`] is
//! the JSON artifact that carries the answer, and [`golden::GoldenTest`] / [`suite`] wire that up to
//! real scene files and committed golden PNGs — the same thing `ludion test` (`ludion-cli`) drives.
//! [`sim_assert`] separately wraps the two hand-written patterns in `ludion-sim`'s own determinism
//! suite (predicate-until-true, hash-sequence-equality) for headless gameplay tests. [`driver`] is
//! the game-project counterpart at the `Engine` layer: [`loaded_engine`] is the "build this game's
//! `EngineBuilder`, load its scene inline" step every headless gameplay/replay/audio test starts
//! from, and [`GameplayDriver`] adds a frame-cursor-tracked `hold`/`release`/`tap` on top for tests
//! that script key presses. [`golden::GoldenTest::single`] is the matching one-call recipe for a
//! game's own golden-image test (resolve the per-OS path, skip/bless/compare) — see its doc example.
//!
//! [`stats::image_stats`] answers the other image question — not "did this frame change?" but "does
//! this frame carry a *defect*?", with no reference image at all: [`ImageStats`] scores quantization
//! contouring, un-antialiased edges, and channels pinned to the black/white rails as four
//! normalized, resolution-stable numbers. They are defect detectors, never quality scores, so an
//! imaging change proves itself by moving one of them with no human at the screen.
//!
//! # Example — one golden test
//!
//! ```no_run
//! use ludion_testkit::GoldenTest;
//!
//! // Goldens are blessed per pinned OS: `goldens/windows/…` or `goldens/macos/…`.
//! let golden = format!("goldens/{}/scene.png", ludion_testkit::platform_dir().map_err(|d| vec![d])?);
//! let test = GoldenTest::new("scene.yaml", golden);
//! match test.run(std::path::Path::new("target/testkit"))? {
//!     ludion_testkit::GoldenOutcome::Passed { report } => {
//!         println!("pass: ssim {}", report.ssim.unwrap());
//!     }
//!     ludion_testkit::GoldenOutcome::Failed { report, .. } => {
//!         eprintln!("{}", report.summary);
//!     }
//!     ludion_testkit::GoldenOutcome::Blessed { .. } => unreachable!("run() never blesses"),
//! }
//! # Ok::<(), Vec<ludion_core::diagnostics::Diagnostic>>(())
//! ```
//!
//! # Example — comparing two images directly
//!
//! ```
//! use ludion_testkit::ssim;
//!
//! # fn solid(w: u32, h: u32, px: [u8; 4]) -> ludion_testkit::DecodedImage {
//! #     let mut rgba = Vec::new();
//! #     for _ in 0..(w * h) { rgba.extend_from_slice(&px); }
//! #     ludion_testkit::DecodedImage { width: w, height: h, rgba }
//! # }
//! let a = solid(64, 64, [200, 200, 200, 255]);
//! let b = solid(64, 64, [200, 200, 200, 255]);
//! let result = ssim::compare(&a, &b);
//! assert_eq!(result.ssim, 1.0); // identical images score exactly 1.0
//! ```

mod image;

pub mod audio;
pub mod drift;
pub mod driver;
pub mod golden;
pub mod imports;
pub mod report;
pub mod sim_assert;
pub mod spelling;
pub mod ssim;
pub mod stats;
pub mod suite;

pub use drift::assert_components_drift_safe;
pub use driver::{GameplayDriver, loaded_engine};
pub use golden::{
    GoldenOutcome, GoldenTest, MACOS_REFERENCE_ADAPTER, WINDOWS_REFERENCE_ADAPTER, bless_requested,
    compare_png_bytes, missing_golden, platform_dir, provenance_label, unverifiable,
};
pub use image::DecodedImage;
pub use imports::{missing_derived_imports, require_derived_imports};
pub use report::DiffReport;
pub use sim_assert::{assert_hash_sequences_match, assert_reaches, hash_sequence, run_until};
pub use spelling::assert_saves_respell_nothing;
pub use ssim::{Comparison, MaskedSsim, TileDiff, TileGrid, TileRect, compare, masked_ssim};
pub use stats::{ImageStats, image_stats};
pub use suite::{DiscoveredTest, SuiteDefaults, SuiteTestResult, discover, run_suite};
