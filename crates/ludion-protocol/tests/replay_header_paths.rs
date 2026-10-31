//! No committed `.lrp` replay header names an absolute or machine-specific path.
//!
//! The privacy canary skips `.lrp` by extension and the header is zstd-compressed, so a replay
//! recorded with an absolute scene path would publish the recorder's home directory unseen. This
//! test decodes every committed replay's header and refuses such a path.

use std::path::{Path, PathBuf};

use ludion_protocol::replay::{ReplayReader, SceneRef};

/// Directories that hold committed replays, relative to the workspace root. Walked recursively;
/// untracked strays at the root (a local `demo.lrp`) are deliberately outside the walk.
const REPLAY_DIRS: &[&str] = &["crates", "examples", "games", "fuzz/corpus"];

/// Directory names never walked: build output and vendored trees.
const SKIP_DIRS: &[&str] = &["target", "node_modules", ".build", ".git"];

/// Replays that are hostile on purpose (a decompression bomb) and do not decode.
const HOSTILE: &[&str] = &["zstd-decompression-bomb.lrp"];

fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .expect("crates/ludion-protocol sits two levels below the workspace root")
        .to_path_buf()
}

fn collect(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let name = entry.file_name().to_string_lossy().into_owned();
        if path.is_dir() {
            if !SKIP_DIRS.contains(&name.as_str()) {
                collect(&path, out);
            }
        } else if path.extension().is_some_and(|e| e == "lrp") && !HOSTILE.contains(&name.as_str())
        {
            out.push(path);
        }
    }
}

/// The first absolute-path shape in `text`, if any.
fn absolute_path_shape(text: &str) -> Option<&'static str> {
    for needle in ["/Users/", "/home/", "C:\\", "c:\\", "\\\\"] {
        if text.contains(needle) {
            return Some(needle);
        }
    }
    // A drive letter: a letter not preceded by an alphanumeric, then `:` and a slash (so the
    // `p:/` inside `http://` does not match).
    let bytes = text.as_bytes();
    for i in 0..bytes.len().saturating_sub(2) {
        if bytes[i].is_ascii_alphabetic()
            && bytes[i + 1] == b':'
            && matches!(bytes[i + 2], b'\\' | b'/')
            && (i == 0 || !bytes[i - 1].is_ascii_alphanumeric())
        {
            return Some("a drive letter");
        }
    }
    None
}

/// Every string in `value` with its JSON Pointer-style location.
fn collect_strings<'v>(at: &str, value: &'v serde_json::Value, out: &mut Vec<(String, &'v str)>) {
    match value {
        serde_json::Value::String(text) => out.push((at.to_string(), text)),
        serde_json::Value::Array(items) => {
            for (i, item) in items.iter().enumerate() {
                collect_strings(&format!("{at}/{i}"), item, out);
            }
        }
        serde_json::Value::Object(fields) => {
            for (key, item) in fields {
                collect_strings(&format!("{at}/{key}"), item, out);
            }
        }
        _ => {}
    }
}

/// A scene path that is portable: relative, forward-slash, no `..` escape.
fn portable_scene_path(path: &str) -> Result<(), &'static str> {
    if path.starts_with('/') || path.starts_with('\\') {
        Err("is absolute")
    } else if path.contains('\\') {
        Err("contains a backslash")
    } else if path.split('/').any(|part| part == "..") {
        Err("escapes with `..`")
    } else if absolute_path_shape(path).is_some() {
        Err("carries a drive letter or absolute-path shape")
    } else {
        Ok(())
    }
}

#[test]
fn committed_replay_headers_carry_no_absolute_paths() {
    let root = workspace_root();
    let mut replays = Vec::new();
    for dir in REPLAY_DIRS {
        collect(&root.join(dir), &mut replays);
    }
    replays.sort();
    assert!(
        replays.len() >= 5,
        "the walk found only {} replays; the directory list is stale",
        replays.len()
    );

    let mut failures = Vec::new();
    for path in &replays {
        let shown = path.strip_prefix(&root).unwrap_or(path).display();
        let bytes = std::fs::read(path).unwrap_or_else(|e| panic!("{shown}: {e}"));
        let reader = ReplayReader::new(&bytes)
            .unwrap_or_else(|d| panic!("{shown}: header does not decode: {}", d.message));
        let header = reader.header();
        if let SceneRef::Path { path } = &header.scene
            && let Err(why) = portable_scene_path(path)
        {
            failures.push(format!("{shown}: header.scene.path {path:?} {why}"));
        }
        // Scan the decoded string values, not the serialized JSON, where an escaped newline
        // (`:\n`) would read as a drive letter.
        let value = serde_json::to_value(header).expect("a decoded header re-serializes");
        let mut strings = Vec::new();
        collect_strings("header", &value, &mut strings);
        for (field, text) in strings {
            if let Some(shape) = absolute_path_shape(text) {
                failures.push(format!("{shown}: {field} contains {shape}"));
            }
        }
    }
    assert!(
        failures.is_empty(),
        "committed replays with machine-specific paths (re-recording is a separate task):\n{}",
        failures.join("\n")
    );
}

#[test]
fn the_path_checks_catch_what_they_claim_to() {
    for bad in [
        "/Users/a/b.yaml",
        "C:\\x\\y.yaml",
        "scenes\\a.yaml",
        "../a.yaml",
        "/abs.yaml",
    ] {
        assert!(portable_scene_path(bad).is_err(), "{bad} should be refused");
    }
    assert!(portable_scene_path("scenes/main.yaml").is_ok());
    assert!(absolute_path_shape("see http://example.com/a").is_none());
    assert!(absolute_path_shape("E:/data/a").is_some());
}
