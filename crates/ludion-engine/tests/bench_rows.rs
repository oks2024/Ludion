//! Helpers the physics benches share: a per-system timing row read off a [`ProfileReport`], and
//! the pinned-box budget file lookup.

use std::path::Path;

use ludion_engine::{Budget, ProfileReport, load_budget};

#[derive(Clone, Copy, Default)]
pub struct Row {
    pub median: f64,
    pub mean: f64,
    pub p99: f64,
}

pub fn row(report: &ProfileReport, name: &str) -> Row {
    let timing = report
        .sim
        .systems
        .iter()
        .find(|s| s.name == name)
        .unwrap_or_else(|| panic!("no timing for `{name}`"));
    let samples = report
        .samples
        .as_ref()
        .expect("offline reports carry samples");
    let at = samples
        .system_names
        .iter()
        .position(|s| s == name)
        .expect("a sampled system");
    let column = &samples.system_ms[at];
    Row {
        median: timing.median_ms,
        mean: column.iter().sum::<f64>() / column.len() as f64,
        p99: timing.tail.as_ref().map_or(0.0, |t| t.p99_ms),
    }
}

/// The pinned box name the budget file carries, `None` where no pinned box exists.
pub fn pinned_box(platform: &str) -> Option<&'static str> {
    match platform {
        "macos" => Some("apple-m4"),
        "windows" => Some("rtx3090"),
        _ => None,
    }
}

/// `dir/pinned-<box>-<stem>.toml` when it exists; a present but broken file panics.
pub fn platform_budget(dir: &Path, platform: &str, stem: &str) -> Option<Budget> {
    let name = pinned_box(platform)?;
    let path = dir.join(format!("pinned-{name}-{stem}.toml"));
    if !path.exists() {
        return None;
    }
    Some(load_budget(&path).unwrap_or_else(|d| panic!("{}: {}", path.display(), d.message)))
}
