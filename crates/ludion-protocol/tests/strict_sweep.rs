//! Every committed client of the wire still decodes under strict params: each committed `.lrp`
//! replay, and every request example written out in the docs.

use std::path::{Path, PathBuf};

use ludion_protocol::command::MAX_STEP_TICKS;
use ludion_protocol::replay::{ReplayReader, ReplayRecord};
use ludion_protocol::{Command, Request};

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn walk(dir: &Path, ext: &str, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            if path.file_name().is_some_and(|n| n == "target") {
                continue;
            }
            walk(&path, ext, out);
        } else if path.extension().is_some_and(|e| e == ext) {
            out.push(path);
        }
    }
}

#[test]
fn every_committed_replay_decodes_and_steps_within_the_cap() {
    let mut files = Vec::new();
    for dir in ["examples", "games"] {
        walk(&root().join(dir), "lrp", &mut files);
    }
    files.sort();
    assert!(
        !files.is_empty(),
        "no committed replay found under examples/ or games/"
    );
    let mut largest = 0;
    for file in &files {
        let bytes = std::fs::read(file).unwrap();
        let records = ReplayReader::new(&bytes)
            .and_then(|reader| reader.records())
            .unwrap_or_else(|d| panic!("{} does not decode: {d}", file.display()));
        for record in records {
            if let ReplayRecord::Command {
                command: Command::SimStep(step),
                ..
            } = record
            {
                largest = largest.max(step.ticks);
            }
        }
    }
    println!("{} replays; largest sim.step = {largest}", files.len());
    assert!(
        largest <= MAX_STEP_TICKS,
        "a committed replay steps {largest} > {MAX_STEP_TICKS}"
    );
}

#[test]
fn every_documented_request_passes_strict_decode() {
    let mut files = Vec::new();
    for dir in ["docs", "dev-docs"] {
        walk(&root().join(dir), "md", &mut files);
    }
    let mut checked = 0;
    for file in &files {
        let text = std::fs::read_to_string(file).unwrap();
        for (n, line) in text.lines().enumerate() {
            let Ok(value) = serde_json::from_str::<serde_json::Value>(line.trim()) else {
                continue;
            };
            if value.get("v").is_none() || value.get("method").is_none() {
                continue;
            }
            Request::from_value(value).unwrap_or_else(|d| {
                panic!(
                    "{}:{}: documented request is refused: {d}",
                    file.display(),
                    n + 1
                )
            });
            checked += 1;
        }
    }
    assert!(
        checked > 0,
        "no documented request found under docs/ or dev-docs/"
    );
    println!("{checked} documented requests decode");
}
