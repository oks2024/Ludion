//! Implementation of `ludion image-stats <PNG>... [--json] [--check <TOML>]`.
//!
//! The instrument half of the post-process pipeline's defect-gate arc (`Plan/post-process-pipeline.md`
//! decision 10): runs `ludion_testkit::image_stats` over one or more PNGs and prints the four
//! normalized defect metrics (`banding_score`, `edge_aliasing`, `clipped_low`, `clipped_high` — see
//! `ludion_testkit::ImageStats` for what each one measures), batching like `generate-map`/`map-check`
//! so an agent can record a whole gallery of goldens in one run. `--json` emits the same numbers as a
//! self-describing array (each object carries its own `path`); `--check <TOML>` gates every metric
//! against a ceiling file, mirroring `ludion profile --check`'s budget shape (`L4009` an invalid
//! threshold file, `L4010` one diagnostic per breach — see `ludion explain` on either).
//!
//! This is a defect *gate*, not a target: the four numbers exist to catch a regression a later
//! rendering change introduces (supersampling, output dither, exposure), never to be driven toward
//! zero — a still image legitimately carries some banding, some hard edges, some clipped pixels.
//!
//! `ludion-testkit` decodes its own committed goldens through a private, unexported local decoder
//! (its own doc comment explains why: the crate graph keeps testkit off `ludion-assets`, so it wraps
//! the same `png` crate independently). Since neither the decoder nor a public `path -> DecodedImage`
//! function is re-exported, this reuses `ludion-assets`'s public PNG decoder instead of adding a
//! second one — `ludion-cli` already depends on it, and it wraps the identical `png` crate.

use std::collections::BTreeMap;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use ludion_core::codes::code;
use ludion_core::diagnostics::Diagnostic;
use ludion_testkit::{DecodedImage, ImageStats, image_stats};
use serde::{Deserialize, Serialize};

use crate::diagnostic;

/// One image's report row: its path (as given on the command line) plus its four metrics, flattened
/// so the JSON form is `{"path": "...", "banding_score": ..., ...}` — self-describing per object,
/// which is what makes a batch report useful on its own.
#[derive(Debug, Serialize)]
struct ImageReport {
    path: String,
    #[serde(flatten)]
    stats: ImageStats,
}

/// One [`METRICS`] row: a metric's name, its accessor on a measured [`ImageStats`], and its accessor
/// on a [`Ceilings`] (base or per-image override).
type Metric = (
    &'static str,
    fn(&ImageStats) -> f64,
    fn(&Ceilings) -> Option<f64>,
);

/// The four metrics, in report/column/threshold order, each paired with a stats accessor and a
/// ceiling accessor — the one list every table column, JSON key, and `--check` lookup walks, so the
/// four names never drift against each other.
const METRICS: [Metric; 4] = [
    ("banding_score", |s| s.banding_score, |c| c.banding_score),
    ("edge_aliasing", |s| s.edge_aliasing, |c| c.edge_aliasing),
    ("clipped_low", |s| s.clipped_low, |c| c.clipped_low),
    ("clipped_high", |s| s.clipped_high, |c| c.clipped_high),
];

/// A `--check` threshold file: default ceilings applied to every image, plus optional per-image
/// overrides keyed by the PNG's bare file name (`[images."<file name>"]`) — the one key that survives
/// running the command from a different working directory, since a threshold file travels with the
/// repo while a caller's cwd does not. An omitted metric, at either level, is simply not checked.
/// Every metric is "higher is worse" ([`ImageStats`]'s own guarantee), so every entry here is a
/// ceiling, never a floor.
#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct ThresholdFile {
    #[serde(default)]
    banding_score: Option<f64>,
    #[serde(default)]
    edge_aliasing: Option<f64>,
    #[serde(default)]
    clipped_low: Option<f64>,
    #[serde(default)]
    clipped_high: Option<f64>,
    #[serde(default)]
    images: BTreeMap<String, Ceilings>,
}

impl ThresholdFile {
    /// The file's top-level ceilings, as a [`Ceilings`] — the same shape a per-image override takes,
    /// so both levels resolve through the same [`METRICS`] accessors.
    fn base(&self) -> Ceilings {
        Ceilings {
            banding_score: self.banding_score,
            edge_aliasing: self.edge_aliasing,
            clipped_low: self.clipped_low,
            clipped_high: self.clipped_high,
        }
    }
}

/// One image's four optional ceilings — the shape of both the threshold file's top level and each
/// `[images.*]` override.
#[derive(Debug, Default, Clone, Copy, Deserialize)]
#[serde(deny_unknown_fields)]
struct Ceilings {
    #[serde(default)]
    banding_score: Option<f64>,
    #[serde(default)]
    edge_aliasing: Option<f64>,
    #[serde(default)]
    clipped_low: Option<f64>,
    #[serde(default)]
    clipped_high: Option<f64>,
}

/// Runs `ludion image-stats <images>... [--json] [--check <TOML>]`: measure every image in turn,
/// printing one aligned table (or, with `--json`, one JSON array) covering the whole batch, and
/// aggregating the exit code — non-zero if any image failed to decode or, with `--check`, breached a
/// ceiling. An invalid `--check` file is validated once, up front, before any image is measured. Stdout
/// is the table or JSON only; the `--check` verdict line goes to stderr.
pub fn run(images: &[PathBuf], json: bool, check: Option<&Path>) -> ExitCode {
    let thresholds = match check {
        Some(path) => match load_thresholds(path) {
            Ok(thresholds) => Some(thresholds),
            Err(diag) => {
                diagnostic::print(&diag);
                return ExitCode::FAILURE;
            }
        },
        None => None,
    };

    let mut reports = Vec::with_capacity(images.len());
    let mut failed = false;
    let mut breach_count = 0usize;

    for path in images {
        match decode(path) {
            Ok(decoded) => {
                let stats = image_stats(&decoded);
                if let Some(thresholds) = &thresholds {
                    let breaches = check_one(path, &stats, thresholds);
                    breach_count += breaches.len();
                    for breach in &breaches {
                        diagnostic::print(breach);
                    }
                }
                reports.push(ImageReport {
                    path: path.display().to_string(),
                    stats,
                });
            }
            Err(diag) => {
                diagnostic::report(path, std::slice::from_ref(&diag));
                failed = true;
            }
        }
    }
    if breach_count > 0 {
        failed = true;
    }

    if json {
        print!("{}", render_json(&reports));
    } else {
        print!("{}", render_table(&reports));
    }
    let _ = std::io::stdout().flush();

    if let Some(path) = check {
        if breach_count == 0 {
            eprintln!("{}: all images within thresholds", path.display());
        } else {
            eprintln!(
                "{}: {breach_count} threshold breach{} found",
                path.display(),
                if breach_count == 1 { "" } else { "es" },
            );
        }
    }

    if failed {
        ExitCode::FAILURE
    } else {
        ExitCode::SUCCESS
    }
}

/// Decodes the PNG at `path` into the [`DecodedImage`] [`image_stats`] takes. See the module doc for
/// why this goes through `ludion-assets` rather than testkit's own (unexported) decoder.
fn decode(path: &Path) -> Result<DecodedImage, Diagnostic> {
    let root = path.parent().unwrap_or_else(|| Path::new("."));
    let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
    let image = ludion_assets::load_png(
        ludion_assets::AssetRoot::local_document_unconfined(root),
        name,
    )?;
    Ok(DecodedImage {
        width: image.width,
        height: image.height,
        rgba: image.rgba,
    })
}

/// Reads and parses a `--check` threshold file. Any read error, TOML syntax error, or key outside the
/// four `ImageStats` metrics is one `L4009`.
fn load_thresholds(path: &Path) -> Result<ThresholdFile, Diagnostic> {
    let text = std::fs::read_to_string(path)
        .map_err(|error| threshold_invalid(path, format!("could not read the file: {error}")))?;
    toml::from_str(&text).map_err(|error| threshold_invalid(path, error.to_string()))
}

fn threshold_invalid(path: &Path, detail: String) -> Diagnostic {
    Diagnostic::new(
        code::STATS_THRESHOLD_FILE_INVALID,
        format!(
            "`{}` is not a valid image-stats threshold file: {detail}",
            path.display()
        ),
    )
    .with_help("see `ludion explain L4009` for the threshold-file shape")
}

/// The `--check` per-image lookup key: the PNG's bare file name, not its stem and not the path as
/// given — see [`ThresholdFile`]'s doc comment for why.
fn image_key(path: &Path) -> String {
    path.file_name()
        .and_then(|n| n.to_str())
        .map(str::to_string)
        .unwrap_or_else(|| path.display().to_string())
}

/// Compares one image's measured `stats` against `thresholds` (its per-image override, if any, else
/// the file's base ceilings), returning one `L4010` per breached metric.
fn check_one(path: &Path, stats: &ImageStats, thresholds: &ThresholdFile) -> Vec<Diagnostic> {
    let overrides = thresholds.images.get(&image_key(path));
    let base = thresholds.base();

    let mut breaches = Vec::new();
    for (name, measure, ceiling_of) in METRICS {
        let Some(ceiling) = overrides.and_then(ceiling_of).or_else(|| ceiling_of(&base)) else {
            continue;
        };
        let measured = measure(stats);
        if measured > ceiling {
            breaches.push(
                Diagnostic::new(
                    code::STATS_THRESHOLD_EXCEEDED,
                    format!(
                        "{}: {name} {measured:.4} exceeds ceiling {ceiling:.4}",
                        path.display(),
                    ),
                )
                .with_help(
                    "run `ludion image-stats` again to confirm the regression reproduces, then \
                     find the change that moved the metric — re-calibrate the threshold only if \
                     the new value is intended and permanent",
                ),
            );
        }
    }
    breaches
}

/// Renders the reports as compact one-line JSON with a trailing newline (empty array for a
/// wholly-failed batch).
fn render_json(reports: &[ImageReport]) -> String {
    let mut json =
        serde_json::to_string(reports).expect("image reports are plain data and always serialize");
    json.push('\n');
    json
}

/// Renders the compact, aligned, greppable default table: one header row, then one row per
/// successfully measured image, columns sized to the widest header or value in each.
fn render_table(reports: &[ImageReport]) -> String {
    if reports.is_empty() {
        return String::new();
    }
    const COLUMNS: [&str; 4] = [
        "banding_score",
        "edge_aliasing",
        "clipped_low",
        "clipped_high",
    ];

    let cells: Vec<[String; 4]> = reports
        .iter()
        .map(|r| {
            [
                format!("{:.4}", r.stats.banding_score),
                format!("{:.4}", r.stats.edge_aliasing),
                format!("{:.4}", r.stats.clipped_low),
                format!("{:.4}", r.stats.clipped_high),
            ]
        })
        .collect();

    let path_w = reports
        .iter()
        .map(|r| r.path.len())
        .max()
        .unwrap_or(0)
        .max("image".len());
    let col_w: [usize; 4] = std::array::from_fn(|i| {
        cells
            .iter()
            .map(|row| row[i].len())
            .max()
            .unwrap_or(0)
            .max(COLUMNS[i].len())
    });

    let mut out = String::new();
    out.push_str(&format!("{:<path_w$}", "image"));
    for (name, w) in COLUMNS.iter().zip(col_w) {
        out.push_str(&format!("  {name:>w$}"));
    }
    out.push('\n');

    for (report, row) in reports.iter().zip(&cells) {
        out.push_str(&format!("{:<path_w$}", report.path));
        for (value, w) in row.iter().zip(col_w) {
            out.push_str(&format!("  {value:>w$}"));
        }
        out.push('\n');
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn stats(banding: f64, edge: f64, low: f64, high: f64) -> ImageStats {
        ImageStats {
            banding_score: banding,
            edge_aliasing: edge,
            clipped_low: low,
            clipped_high: high,
        }
    }

    #[test]
    fn table_has_a_row_per_image_and_the_four_columns() {
        let reports = vec![
            ImageReport {
                path: "a.png".to_string(),
                stats: stats(0.1, 0.2, 0.3, 0.4),
            },
            ImageReport {
                path: "b.png".to_string(),
                stats: stats(0.5, 0.6, 0.7, 0.8),
            },
        ];
        let table = render_table(&reports);
        let mut lines = table.lines();
        let header = lines.next().unwrap();
        for column in [
            "banding_score",
            "edge_aliasing",
            "clipped_low",
            "clipped_high",
        ] {
            assert!(header.contains(column), "header missing {column}: {header}");
        }
        assert!(table.contains("a.png") && table.contains("0.1000"));
        assert!(table.contains("b.png") && table.contains("0.8000"));
    }

    #[test]
    fn json_report_is_self_describing_per_image() {
        let reports = vec![ImageReport {
            path: "x.png".to_string(),
            stats: stats(0.1, 0.2, 0.3, 0.4),
        }];
        let json = render_json(&reports);
        let value: serde_json::Value = serde_json::from_str(&json).unwrap();
        let first = &value[0];
        assert_eq!(first["path"], "x.png");
        assert_eq!(first["banding_score"], 0.1);
        assert_eq!(first["clipped_high"], 0.4);
    }

    #[test]
    fn threshold_file_parses_base_and_per_image_overrides() {
        let text = "banding_score = 0.5\n\n[images.\"night-sky.png\"]\nbanding_score = 0.9\n";
        let file: ThresholdFile = toml::from_str(text).unwrap();
        assert_eq!(file.base().banding_score, Some(0.5));
        assert_eq!(
            file.images.get("night-sky.png").unwrap().banding_score,
            Some(0.9)
        );
    }

    #[test]
    fn an_unknown_metric_name_fails_to_parse() {
        let text = "banding_scoer = 0.5\n";
        assert!(toml::from_str::<ThresholdFile>(text).is_err());
    }

    #[test]
    fn per_image_override_wins_over_the_base_ceiling() {
        let text = "banding_score = 0.5\n\n[images.\"night-sky.png\"]\nbanding_score = 0.9\n";
        let file: ThresholdFile = toml::from_str(text).unwrap();

        let below_override_above_base = stats(0.8, 0.0, 0.0, 0.0);
        assert!(
            check_one(
                Path::new("night-sky.png"),
                &below_override_above_base,
                &file
            )
            .is_empty(),
            "the per-image override (0.9) must win over the base ceiling (0.5)"
        );
        assert_eq!(
            check_one(Path::new("other.png"), &below_override_above_base, &file).len(),
            1,
            "an unlisted image must fall back to the base ceiling"
        );
    }

    #[test]
    fn image_key_is_the_bare_file_name() {
        assert_eq!(
            image_key(Path::new("goldens/windows/night-sky.png")),
            "night-sky.png"
        );
        assert_eq!(
            image_key(Path::new("/abs/path/night-sky.png")),
            "night-sky.png"
        );
    }
}
