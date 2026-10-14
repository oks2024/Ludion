//! [`DiffReport`] — the structured, agent-legible artifact a golden-test failure produces.
//!
//! Per `CLAUDE.md`'s "errors are the UX budget" rule, a failing golden test must be diagnosable by
//! an agent with **no image viewer**: this JSON document names the golden and actual paths, the
//! global score, and — critically — exactly where in the image the damage is, in both pixel-rect
//! and coarse human terms ("top-left"). The report is the primary artifact; [`DiffReport::diagnostic`]
//! additionally renders it as a rustc-style [`Diagnostic`] for terminal output (`CLAUDE.md`: "keep
//! the JSON report the primary artifact").

use serde::{Deserialize, Serialize};

use ludion_core::codes::code;
use ludion_core::diagnostics::Diagnostic;

use crate::ssim::{Comparison, TileDiff, TileGrid};

/// The full structured result of one golden-test comparison.
///
/// Always produced on a completed comparison (pass *or* fail); only written to disk on failure
/// (see [`crate::golden`]) — a passing test needs no artifact.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DiffReport {
    /// Path to the committed golden image (as configured, not necessarily absolute).
    pub golden: String,
    /// Path the freshly rendered "actual" image would be (or was) written to.
    pub actual: String,
    /// The golden image's width.
    pub golden_width: u32,
    /// The golden image's height.
    pub golden_height: u32,
    /// The freshly rendered image's width.
    pub actual_width: u32,
    /// The freshly rendered image's height.
    pub actual_height: u32,
    /// The configured minimum passing SSIM.
    pub threshold: f64,
    /// The minimum passing SSIM of any single tile — a local defect fails even when the global mean
    /// passes. Never above `threshold`.
    pub tile_floor: f64,
    /// The maximum passing colour error of any single tile (mean absolute R, G or B channel
    /// difference, 0–255). SSIM is luma-only, so this is what fails a hue change.
    pub color_ceiling: f64,
    /// Whether this comparison passed.
    pub pass: bool,
    /// The global SSIM score. `None` when the images differ in size (nothing to compare pixel-wise).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ssim: Option<f64>,
    /// Mean absolute per-channel error (0–255 scale), the secondary signal. `None` on a dimension
    /// mismatch, same as `ssim`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mae: Option<f64>,
    /// The tile grid's shape.
    pub tile_grid: TileGrid,
    /// Every tile whose own SSIM fell below `threshold` or whose colour error exceeded
    /// `color_ceiling`, lowest SSIM first (ties: larger colour error first). Empty on a dimension
    /// mismatch or a passing comparison.
    pub damaged_tiles: Vec<TileDiff>,
    /// The single lowest-scoring tile, regardless of whether it is "damaged" by the threshold —
    /// useful even on a pass, to see how much margin remains. `None` on a dimension mismatch.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub worst_tile: Option<TileDiff>,
    /// The tile with the highest colour error, regardless of whether it breaches `color_ceiling`.
    /// `None` on a dimension mismatch.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub worst_color_tile: Option<TileDiff>,
    /// A one-line human summary — the first thing to read.
    pub summary: String,
    /// Classifies a failure by where it rendered (`crate::golden::provenance_label`): a red on the
    /// pinned reference adapter for this platform is a regression, anywhere else it is environment
    /// until re-run on the reference box. `None` when the candidate did not come from an in-process
    /// render with a known adapter (e.g. [`crate::golden::compare_png_bytes`]'s wire screenshots).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub provenance: Option<String>,
}

impl DiffReport {
    /// Builds a report for a dimension mismatch: no pixel comparison is possible, so `ssim`/`mae`/
    /// tiles are all absent.
    pub(crate) fn dimension_mismatch(
        golden_path: &str,
        actual_path: &str,
        golden_size: (u32, u32),
        actual_size: (u32, u32),
        threshold: f64,
        tile_floor: f64,
        color_ceiling: f64,
    ) -> DiffReport {
        let (golden_width, golden_height) = golden_size;
        let (actual_width, actual_height) = actual_size;
        DiffReport {
            golden: golden_path.to_string(),
            actual: actual_path.to_string(),
            golden_width,
            golden_height,
            actual_width,
            actual_height,
            threshold,
            tile_floor,
            color_ceiling,
            pass: false,
            ssim: None,
            mae: None,
            tile_grid: TileGrid { cols: 0, rows: 0 },
            damaged_tiles: Vec::new(),
            worst_tile: None,
            worst_color_tile: None,
            summary: format!(
                "actual render is {actual_width}x{actual_height} but the golden is \
                 {golden_width}x{golden_height} — re-bless if this resolution change is \
                 intentional"
            ),
            provenance: None,
        }
    }

    /// Builds a report from a completed [`Comparison`] (equal-sized images).
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn from_comparison(
        golden_path: &str,
        actual_path: &str,
        width: u32,
        height: u32,
        threshold: f64,
        tile_floor: f64,
        color_ceiling: f64,
        comparison: &Comparison,
    ) -> DiffReport {
        let mut damaged: Vec<TileDiff> = comparison
            .tiles
            .iter()
            .filter(|t| t.ssim < threshold || t.color_error > color_ceiling)
            .cloned()
            .collect();
        damaged.sort_by(|a, b| {
            a.ssim
                .total_cmp(&b.ssim)
                .then(b.color_error.total_cmp(&a.color_error))
        });

        let worst_tile = comparison
            .tiles
            .iter()
            .min_by(|a, b| a.ssim.total_cmp(&b.ssim))
            .cloned();
        let worst_color_tile = comparison
            .tiles
            .iter()
            .max_by(|a, b| a.color_error.total_cmp(&b.color_error))
            .cloned();

        let global_ok = comparison.ssim >= threshold;
        let floor_breach = worst_tile.as_ref().filter(|t| t.ssim < tile_floor);
        let color_breach = worst_color_tile
            .as_ref()
            .filter(|t| t.color_error > color_ceiling);
        let pass = global_ok && floor_breach.is_none() && color_breach.is_none();

        let summary = if pass {
            let worst = match &worst_tile {
                Some(w) => format!(
                    ", worst tile {:.4} at {} r{}c{}",
                    w.ssim, w.region, w.row, w.col
                ),
                None => String::new(),
            };
            let colour = worst_color_tile.as_ref().map_or(String::new(), |w| {
                format!(", worst colour {:.2}", w.color_error)
            });
            format!(
                "PASS: ssim {:.4} >= threshold {:.4}{worst}{colour} ({} tile{} below threshold)",
                comparison.ssim,
                threshold,
                damaged.len(),
                if damaged.len() == 1 { "" } else { "s" }
            )
        } else if !global_ok {
            let mut text = match &worst_tile {
                Some(worst) => format!(
                    "FAIL: ssim {:.4} < threshold {:.4}; worst damage at {} (tile r{}c{}, rect {},{} \
                     {}x{}, ssim {:.4})",
                    comparison.ssim,
                    threshold,
                    worst.region,
                    worst.row,
                    worst.col,
                    worst.rect.x,
                    worst.rect.y,
                    worst.rect.width,
                    worst.rect.height,
                    worst.ssim
                ),
                None => format!(
                    "FAIL: ssim {:.4} < threshold {:.4}",
                    comparison.ssim, threshold
                ),
            };
            if floor_breach.is_some() {
                text.push_str(&format!("; tile floor {tile_floor:.4} also breached"));
            }
            if color_breach.is_some() {
                text.push_str(&format!(
                    "; colour ceiling {color_ceiling:.2} also breached"
                ));
            }
            text
        } else if let Some(worst) = floor_breach {
            let mut text = format!(
                "FAIL: tile r{}c{} at {} scored {:.4} < tile floor {:.4} (global ssim {:.4} passes \
                 {:.4}); rect {},{} {}x{}",
                worst.row,
                worst.col,
                worst.region,
                worst.ssim,
                tile_floor,
                comparison.ssim,
                threshold,
                worst.rect.x,
                worst.rect.y,
                worst.rect.width,
                worst.rect.height
            );
            if color_breach.is_some() {
                text.push_str(&format!(
                    "; colour ceiling {color_ceiling:.2} also breached"
                ));
            }
            text
        } else {
            let worst = color_breach.expect("a passing structure with a failing report");
            format!(
                "FAIL: tile r{}c{} at {} colour error {:.2} > ceiling {:.2} (ssim {:.4} passes); \
                 rect {},{} {}x{}",
                worst.row,
                worst.col,
                worst.region,
                worst.color_error,
                color_ceiling,
                worst.ssim,
                worst.rect.x,
                worst.rect.y,
                worst.rect.width,
                worst.rect.height
            )
        };

        DiffReport {
            golden: golden_path.to_string(),
            actual: actual_path.to_string(),
            golden_width: width,
            golden_height: height,
            actual_width: width,
            actual_height: height,
            threshold,
            tile_floor,
            color_ceiling,
            pass,
            ssim: Some(comparison.ssim),
            mae: Some(comparison.mae),
            tile_grid: comparison.grid,
            damaged_tiles: damaged,
            worst_tile,
            worst_color_tile,
            summary,
            provenance: None,
        }
    }

    /// Asserts a "must differ" pair moved at least `min_tiles` tiles — below the threshold or past
    /// the colour ceiling — so one stray changed window (the worst-tile minimum) cannot satisfy it.
    pub fn assert_moved_tiles(&self, min_tiles: usize, what: &str) {
        assert!(
            !self.pass && self.damaged_tiles.len() >= min_tiles,
            "{what}: {} tile(s) moved, at least {min_tiles} required ({})",
            self.damaged_tiles.len(),
            self.summary
        );
    }

    /// Renders this report as an rustc-style [`Diagnostic`] for terminal output. The JSON report
    /// (written to disk by [`crate::golden`]) remains the primary artifact; this is a human-legible
    /// summary of it, distinguishing a dimension mismatch (`L2202`), a structural failure
    /// (`L2203`) and a colour-only failure (`L2207`). Only meaningful when [`DiffReport::pass`] is `false`.
    pub fn diagnostic(&self) -> Diagnostic {
        let structure_ok = self.ssim.is_some_and(|s| s >= self.threshold)
            && self
                .worst_tile
                .as_ref()
                .is_some_and(|t| t.ssim >= self.tile_floor);
        if self.ssim.is_none() {
            Diagnostic::new(code::RENDER_IMAGE_DIMENSION_MISMATCH, self.summary.clone())
                .with_help("re-bless the golden (`ludion test --bless`) if the resolution change is intentional")
        } else if structure_ok {
            Diagnostic::new(
                code::RENDER_GOLDEN_COLOR_ABOVE_CEILING,
                self.summary.clone(),
            )
            .with_help(format!(
                "see the diff report's `worst_color_tile` for the region, and the actual/golden \
                     PNGs at `{}` / `{}`; re-bless (`ludion test --bless`) if the change is \
                     intentional",
                self.actual, self.golden
            ))
        } else {
            Diagnostic::new(code::RENDER_SSIM_BELOW_THRESHOLD, self.summary.clone()).with_help(
                format!(
                    "see the diff report for the damaged region, and the actual/golden PNGs at \
                     `{}` / `{}`; re-bless (`ludion test --bless`) if the change is intentional",
                    self.actual, self.golden
                ),
            )
        }
    }

    /// Serializes this report as pretty-printed JSON.
    pub fn to_json(&self) -> String {
        // A `DiffReport` is always representable as JSON (plain data, no maps with non-string
        // keys), so this cannot fail in practice; unwrap keeps the API ergonomic for callers that
        // already handle `Diagnostic`-shaped errors everywhere else.
        serde_json::to_string_pretty(self).expect("DiffReport always serializes")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ssim::TileRect;

    fn tile(row: usize, col: usize, ssim: f64, region: &str) -> TileDiff {
        TileDiff {
            row,
            col,
            rect: TileRect {
                x: (col as u32) * 8,
                y: (row as u32) * 8,
                width: 8,
                height: 8,
            },
            ssim,
            color_error: 0.0,
            region: region.to_string(),
        }
    }

    #[test]
    fn passing_comparison_has_no_damaged_tiles_and_pass_is_true() {
        let comparison = Comparison {
            ssim: 1.0,
            mae: 0.0,
            max_color_error: 0.0,
            grid: TileGrid { cols: 1, rows: 1 },
            tiles: vec![tile(0, 0, 1.0, "center")],
        };
        let report =
            DiffReport::from_comparison("g.png", "a.png", 8, 8, 0.99, 0.95, 4.0, &comparison);
        assert!(report.pass);
        assert!(report.damaged_tiles.is_empty());
        assert_eq!(report.worst_tile.unwrap().ssim, 1.0);
    }

    #[test]
    fn failing_comparison_sorts_damaged_tiles_worst_first() {
        let comparison = Comparison {
            ssim: 0.5,
            mae: 40.0,
            max_color_error: 0.0,
            grid: TileGrid { cols: 2, rows: 1 },
            tiles: vec![tile(0, 0, 0.9, "top-left"), tile(0, 1, 0.1, "top-right")],
        };
        let report =
            DiffReport::from_comparison("g.png", "a.png", 16, 8, 0.99, 0.95, 4.0, &comparison);
        assert!(!report.pass);
        assert_eq!(report.damaged_tiles.len(), 2);
        assert_eq!(
            report.damaged_tiles[0].col, 1,
            "worst (lowest ssim) tile must be first"
        );
        assert_eq!(report.worst_tile.unwrap().col, 1);
        assert!(report.summary.contains("top-right"));
    }

    fn flat_with_block(block: Option<(u32, u32, u32)>) -> crate::DecodedImage {
        let (w, h) = (160_u32, 160_u32);
        let mut rgba = Vec::new();
        for y in 0..h {
            for x in 0..w {
                let inside = block.is_some_and(|(bx, by, side)| {
                    x >= bx && x < bx + side && y >= by && y < by + side
                });
                let v = if inside { 255 } else { 20 };
                rgba.extend_from_slice(&[v, v, v, 255]);
            }
        }
        crate::DecodedImage {
            width: w,
            height: h,
            rgba,
        }
    }

    fn report_of(a: &crate::DecodedImage, b: &crate::DecodedImage) -> DiffReport {
        let comparison = crate::ssim::compare(a, b);
        DiffReport::from_comparison(
            "g.png",
            "a.png",
            a.width,
            a.height,
            0.99,
            0.95,
            4.0,
            &comparison,
        )
    }

    #[test]
    #[should_panic(expected = "required")]
    fn a_single_changed_window_does_not_satisfy_must_move() {
        let clean = flat_with_block(None);
        let one_window = flat_with_block(Some((30, 30, 11)));
        report_of(&clean, &one_window).assert_moved_tiles(5, "one window");
    }

    #[test]
    fn a_broad_change_satisfies_must_move() {
        let clean = flat_with_block(None);
        let broad = flat_with_block(Some((0, 0, 140)));
        report_of(&clean, &broad).assert_moved_tiles(5, "broad change");
    }

    fn grid_of_64(bad: f64) -> Comparison {
        let mut tiles: Vec<TileDiff> = (0..64).map(|i| tile(i / 8, i % 8, 1.0, "center")).collect();
        tiles[9].ssim = bad;
        let ssim = tiles.iter().map(|t| t.ssim).sum::<f64>() / 64.0;
        Comparison {
            ssim,
            mae: 1.0,
            max_color_error: 0.0,
            grid: TileGrid { cols: 8, rows: 8 },
            tiles,
        }
    }

    #[test]
    fn one_bad_tile_fails_the_floor_while_the_mean_passes() {
        let comparison = grid_of_64(0.36);
        assert!(comparison.ssim >= 0.99, "the mean alone would pass");
        let report =
            DiffReport::from_comparison("g.png", "a.png", 64, 64, 0.99, 0.95, 4.0, &comparison);
        assert!(!report.pass);
        assert!(report.summary.contains("r1c1"), "{}", report.summary);
        assert!(report.summary.contains("tile floor"), "{}", report.summary);
        assert_eq!(report.diagnostic().code.as_str(), "L2203");
    }

    #[test]
    fn a_tile_exactly_at_the_floor_passes() {
        let comparison = grid_of_64(0.95);
        let report =
            DiffReport::from_comparison("g.png", "a.png", 64, 64, 0.99, 0.95, 4.0, &comparison);
        assert!(report.pass, "{}", report.summary);
        assert!(
            report.summary.contains("worst tile 0.9500"),
            "{}",
            report.summary
        );
    }

    #[test]
    fn a_global_failure_also_names_a_breached_floor() {
        let comparison = Comparison {
            ssim: 0.5,
            mae: 40.0,
            max_color_error: 0.0,
            grid: TileGrid { cols: 1, rows: 1 },
            tiles: vec![tile(0, 0, 0.5, "center")],
        };
        let report =
            DiffReport::from_comparison("g.png", "a.png", 8, 8, 0.99, 0.95, 4.0, &comparison);
        assert!(report.summary.starts_with("FAIL: ssim 0.5000 < threshold"));
        assert!(report.summary.ends_with("tile floor 0.9500 also breached"));
    }

    #[test]
    fn a_colour_only_failure_is_l2207_and_names_the_tile() {
        let mut tiles = vec![tile(0, 0, 1.0, "top-left"), tile(0, 1, 1.0, "top-right")];
        tiles[1].color_error = 30.0;
        let comparison = Comparison {
            ssim: 1.0,
            mae: 10.0,
            max_color_error: 30.0,
            grid: TileGrid { cols: 2, rows: 1 },
            tiles,
        };
        let report =
            DiffReport::from_comparison("g.png", "a.png", 16, 8, 0.99, 0.95, 4.0, &comparison);
        assert!(!report.pass);
        assert!(
            report.summary.contains("colour error 30.00 > ceiling 4.00"),
            "{}",
            report.summary
        );
        assert!(report.summary.contains("r0c1"), "{}", report.summary);
        assert_eq!(report.diagnostic().code.as_str(), "L2207");
        assert_eq!(report.damaged_tiles.len(), 1);
        assert_eq!(report.worst_color_tile.as_ref().unwrap().col, 1);
        let value: serde_json::Value = serde_json::from_str(&report.to_json()).unwrap();
        assert_eq!(value["color_ceiling"], 4.0);
        assert_eq!(value["worst_color_tile"]["color_error"], 30.0);
    }

    #[test]
    fn a_structure_failure_that_also_breaches_colour_says_so_and_stays_l2203() {
        let mut tiles = vec![tile(0, 0, 0.5, "center")];
        tiles[0].color_error = 30.0;
        let comparison = Comparison {
            ssim: 0.5,
            mae: 40.0,
            max_color_error: 30.0,
            grid: TileGrid { cols: 1, rows: 1 },
            tiles,
        };
        let report =
            DiffReport::from_comparison("g.png", "a.png", 8, 8, 0.99, 0.95, 4.0, &comparison);
        assert!(
            report
                .summary
                .ends_with("colour ceiling 4.00 also breached"),
            "{}",
            report.summary
        );
        assert_eq!(report.diagnostic().code.as_str(), "L2203");
    }

    #[test]
    fn dimension_mismatch_report_has_no_ssim_and_fails() {
        let report = DiffReport::dimension_mismatch(
            "g.png",
            "a.png",
            (100, 50),
            (200, 100),
            0.99,
            0.95,
            4.0,
        );
        assert!(!report.pass);
        assert!(report.ssim.is_none());
        assert!(report.mae.is_none());
        assert_eq!(report.diagnostic().code.as_str(), "L2202");
    }

    #[test]
    fn ssim_failure_diagnostic_is_l2203() {
        let comparison = Comparison {
            ssim: 0.5,
            mae: 40.0,
            max_color_error: 0.0,
            grid: TileGrid { cols: 1, rows: 1 },
            tiles: vec![tile(0, 0, 0.5, "center")],
        };
        let report =
            DiffReport::from_comparison("g.png", "a.png", 8, 8, 0.99, 0.95, 4.0, &comparison);
        assert_eq!(report.diagnostic().code.as_str(), "L2203");
    }

    #[test]
    fn json_round_trips_and_has_the_documented_shape() {
        let comparison = Comparison {
            ssim: 0.8,
            mae: 12.0,
            max_color_error: 0.0,
            grid: TileGrid { cols: 1, rows: 1 },
            tiles: vec![tile(0, 0, 0.8, "center")],
        };
        let report =
            DiffReport::from_comparison("g.png", "a.png", 8, 8, 0.99, 0.95, 4.0, &comparison);
        let json = report.to_json();
        let value: serde_json::Value = serde_json::from_str(&json).unwrap();
        for key in [
            "golden",
            "actual",
            "golden_width",
            "golden_height",
            "actual_width",
            "actual_height",
            "threshold",
            "tile_floor",
            "color_ceiling",
            "pass",
            "ssim",
            "mae",
            "tile_grid",
            "damaged_tiles",
            "worst_tile",
            "worst_color_tile",
            "summary",
        ] {
            assert!(value.get(key).is_some(), "report JSON is missing `{key}`");
        }
        let back: DiffReport = serde_json::from_str(&json).unwrap();
        assert_eq!(back.pass, report.pass);
    }
}
