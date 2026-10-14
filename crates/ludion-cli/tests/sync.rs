//! Integration tests for `ludion sync`, driven against the built binary on a scratch project: two
//! kits compile into the derived tree byte-identically to a hand `ludion import`, an unchanged
//! second run skips, one changed source re-imports alone, a vanished source is removed, and a `to`
//! the sync does not own is refused. A deleted or partial output re-imports, a corrupt record is
//! refused, two concurrent syncs both succeed, and staging debris is swept without a report. A
//! sync waits for a first sync that has swapped a stem in but not yet written its record, a killed
//! first sync leaves a provisional record the next run recovers from, `--json` prints one object
//! when the manifest is refused, and a symbolic link under a source directory is refused by name.

use std::io::{BufRead, BufReader, Read};
use std::path::{Path, PathBuf};
use std::process::{Command as ProcCommand, Output};
use std::sync::mpsc::{Receiver, RecvTimeoutError, channel};
use std::time::Duration;

fn textured() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../examples/import-textured/scene.gltf")
}

/// A per-process scratch project root, removed when the test passes (kept on a panic for
/// debugging); the pid keeps concurrent test runs of this file off each other's trees.
struct Scratch(PathBuf);

impl std::ops::Deref for Scratch {
    type Target = Path;
    fn deref(&self) -> &Path {
        &self.0
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        if !std::thread::panicking() {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
}

fn scratch(name: &str) -> Scratch {
    let dir =
        PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(format!("{name}-{}", std::process::id()));
    if let Err(e) = std::fs::remove_dir_all(&dir) {
        assert!(
            e.kind() == std::io::ErrorKind::NotFound,
            "stale scratch dir {dir:?} could not be removed: {e}"
        );
    }
    std::fs::create_dir_all(&dir).unwrap();
    Scratch(dir)
}

fn run_ludion(args: &[&str]) -> Output {
    ProcCommand::new(env!("CARGO_BIN_EXE_ludion"))
        .args(args)
        .output()
        .expect("failed to run the ludion binary")
}

/// A project with kits `a` and `b`, each a copy of the textured fixture, and one `[[import]]`.
fn project(name: &str) -> Scratch {
    let dir = scratch(name);
    std::fs::write(
        dir.join("ludion.toml"),
        "name = \"probe\"\ndefault_scene = \"scenes/main.yaml\"\n\n[[import]]\nfrom = \"kits/*/export/*.gltf\"\nto = \"assets/derived\"\n",
    )
    .unwrap();
    for kit in ["a", "b"] {
        let export = dir.join("kits").join(kit).join("export");
        std::fs::create_dir_all(&export).unwrap();
        std::fs::copy(textured(), export.join(format!("{kit}.gltf"))).unwrap();
    }
    dir
}

/// Runs `ludion sync --json` and returns its exit success and the parsed stdout object.
fn sync(project: &Path) -> (bool, serde_json::Value, String) {
    let output = run_ludion(&["sync", "--project", project.to_str().unwrap(), "--json"]);
    let stdout = String::from_utf8_lossy(&output.stdout);
    let json = serde_json::from_str(stdout.trim())
        .unwrap_or_else(|e| panic!("stdout is not one JSON object ({e}): {stdout}"));
    (
        output.status.success(),
        json,
        String::from_utf8_lossy(&output.stderr).into_owned(),
    )
}

fn stems(json: &serde_json::Value, key: &str) -> Vec<String> {
    json[key]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap().to_string())
        .collect()
}

/// The `code` of every top-level diagnostic in the envelope, in order.
fn diagnostic_codes(json: &serde_json::Value) -> Vec<String> {
    json["diagnostics"]
        .as_array()
        .unwrap()
        .iter()
        .map(|d| d["code"].as_str().unwrap().to_string())
        .collect()
}

fn collect_files(root: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        for entry in std::fs::read_dir(&dir).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                stack.push(path);
            } else {
                out.push(path.strip_prefix(root).unwrap().to_path_buf());
            }
        }
    }
    out.sort();
    out
}

fn assert_trees_identical(expected: &Path, actual: &Path) {
    let expected_files = collect_files(expected);
    assert_eq!(expected_files, collect_files(actual), "file sets differ");
    for rel in &expected_files {
        assert_eq!(
            std::fs::read(expected.join(rel)).unwrap(),
            std::fs::read(actual.join(rel)).unwrap(),
            "`{}` differs",
            rel.display()
        );
    }
}

#[test]
fn sync_imports_skips_reimports_changed_and_removes_vanished() {
    let dir = project("sync-lifecycle");
    let derived = dir.join("assets/derived");

    let (ok, first, stderr) = sync(&dir);
    assert!(ok, "{stderr}");
    assert_eq!(stems(&first, "imported"), ["a", "b"]);
    assert!(stems(&first, "skipped").is_empty());

    // A hand import at the same depth writes the same bytes.
    for stem in ["a", "b"] {
        let hand = dir.join("assets/hand").join(stem);
        let source = dir.join(format!("kits/{stem}/export/{stem}.gltf"));
        let output = run_ludion(&[
            "import",
            source.to_str().unwrap(),
            "--out",
            hand.to_str().unwrap(),
        ]);
        assert!(output.status.success());
        assert_trees_identical(&hand, &derived.join(stem));
    }

    let (ok, second, _) = sync(&dir);
    assert!(ok);
    assert!(stems(&second, "imported").is_empty());
    assert_eq!(stems(&second, "skipped"), ["a", "b"]);

    let b = dir.join("kits/b/export/b.gltf");
    let mut text = std::fs::read_to_string(&b).unwrap();
    text.push('\n');
    std::fs::write(&b, text).unwrap();
    let (ok, third, _) = sync(&dir);
    assert!(ok);
    assert_eq!(stems(&third, "imported"), ["b"]);
    assert_eq!(stems(&third, "skipped"), ["a"]);

    std::fs::remove_dir_all(dir.join("kits/a")).unwrap();
    let (ok, fourth, _) = sync(&dir);
    assert!(ok);
    assert_eq!(stems(&fourth, "removed"), ["a"]);
    assert!(!derived.join("a").exists());
    assert!(derived.join("b/model.prefab.yaml").is_file());
}

#[test]
fn a_to_the_sync_does_not_own_is_refused_and_untouched() {
    let dir = project("sync-unowned");
    let derived = dir.join("assets/derived");
    std::fs::create_dir_all(&derived).unwrap();
    std::fs::write(derived.join("hand.txt"), "mine").unwrap();

    let (ok, json, stderr) = sync(&dir);
    assert!(!ok);
    assert!(stderr.contains("L1245"), "{stderr}");
    assert_eq!(diagnostic_codes(&json), ["L1245"]);
    assert_eq!(
        std::fs::read_to_string(derived.join("hand.txt")).unwrap(),
        "mine"
    );
    assert_eq!(collect_files(&derived), [PathBuf::from("hand.txt")]);
}

#[test]
fn a_from_matching_nothing_is_refused() {
    let dir = project("sync-nomatch");
    std::fs::write(
        dir.join("ludion.toml"),
        "name = \"probe\"\ndefault_scene = \"scenes/main.yaml\"\n\n[[import]]\nfrom = \"kitz/*/export/*.gltf\"\nto = \"assets/derived\"\n",
    )
    .unwrap();
    let (ok, _, stderr) = sync(&dir);
    assert!(!ok);
    assert!(stderr.contains("L1245"), "{stderr}");
    assert!(!dir.join("assets/derived").exists());
}

#[test]
fn stems_differing_only_in_case_are_refused_naming_both_sources() {
    let dir = project("sync-stem-case");
    std::fs::rename(
        dir.join("kits/a/export/a.gltf"),
        dir.join("kits/a/export/Well.gltf"),
    )
    .unwrap();
    std::fs::rename(
        dir.join("kits/b/export/b.gltf"),
        dir.join("kits/b/export/well.gltf"),
    )
    .unwrap();
    let (ok, _, stderr) = sync(&dir);
    assert!(!ok);
    assert!(stderr.contains("L1245"), "{stderr}");
    assert!(
        stderr.contains("Well.gltf") && stderr.contains("well.gltf"),
        "{stderr}"
    );
    assert!(!dir.join("assets/derived").exists());
}

/// Hand-imports kit `stem` of `dir` into `assets/hand/<stem>` and compares it to the derived tree.
fn assert_matches_hand_import(dir: &Path, stem: &str) {
    let hand = dir.join("assets/hand").join(stem);
    let _ = std::fs::remove_dir_all(&hand);
    let source = dir.join(format!("kits/{stem}/export/{stem}.gltf"));
    let output = run_ludion(&[
        "import",
        source.to_str().unwrap(),
        "--out",
        hand.to_str().unwrap(),
    ]);
    assert!(output.status.success());
    assert_trees_identical(&hand, &dir.join("assets/derived").join(stem));
}

fn debris(derived: &Path) -> Vec<String> {
    std::fs::read_dir(derived)
        .unwrap()
        .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
        .filter(|name| name.starts_with(".sync-") || name.starts_with(".old-"))
        .collect()
}

#[test]
fn a_deleted_or_partial_output_reimports() {
    let dir = project("sync-partial");
    let derived = dir.join("assets/derived");
    let (ok, _, stderr) = sync(&dir);
    assert!(ok, "{stderr}");

    std::fs::remove_dir_all(derived.join("a")).unwrap();
    let (ok, second, _) = sync(&dir);
    assert!(ok);
    assert_eq!(stems(&second, "imported"), ["a"]);
    assert_eq!(stems(&second, "skipped"), ["b"]);

    let meshes = derived.join("b/meshes");
    let victim = std::fs::read_dir(&meshes)
        .unwrap()
        .next()
        .expect("the import wrote a mesh")
        .unwrap()
        .path();
    std::fs::remove_file(victim).unwrap();
    let (ok, third, _) = sync(&dir);
    assert!(ok);
    assert_eq!(stems(&third, "imported"), ["b"]);
    assert_eq!(stems(&third, "skipped"), ["a"]);

    for stem in ["a", "b"] {
        assert_matches_hand_import(&dir, stem);
    }
}

#[test]
fn a_corrupt_record_is_refused_and_untouched() {
    let dir = project("sync-corrupt");
    let derived = dir.join("assets/derived");
    let (ok, _, stderr) = sync(&dir);
    assert!(ok, "{stderr}");

    std::fs::write(derived.join(".ludion-sync.json"), "{oops").unwrap();
    let (ok, json, stderr) = sync(&dir);
    assert!(!ok);
    assert!(stderr.contains("L1245"), "{stderr}");
    assert!(stderr.contains(".ludion-sync.json"), "{stderr}");
    assert_eq!(diagnostic_codes(&json), ["L1245"]);
    assert!(derived.join("a/model.prefab.yaml").is_file());
    assert!(derived.join("b/model.prefab.yaml").is_file());
}

#[test]
fn concurrent_syncs_both_succeed() {
    let dir = project("sync-concurrent");
    let lock = hold_lock(&dir.join("assets/derived"));
    // Both syncs are provably contending: each reports the wait before the lock is released.
    let mut children = [spawn_sync(&dir), spawn_sync(&dir)];
    let receivers = children.each_mut().map(stderr_lines);
    for rx in &receivers {
        wait_stderr(rx, WAITING, "both syncs to report the lock wait");
    }
    drop(lock);
    for (child, rx) in children.into_iter().zip(receivers) {
        let (ok, _, stderr) = finish(child, rx);
        assert!(ok, "{stderr}");
    }
    for stem in ["a", "b"] {
        assert_matches_hand_import(&dir, stem);
    }
    assert!(debris(&dir.join("assets/derived")).is_empty());
}

#[test]
fn a_lock_and_staging_dir_without_a_record_still_syncs() {
    let dir = project("sync-first-run-race");
    let derived = dir.join("assets/derived");
    std::fs::create_dir_all(derived.join(".sync-a")).unwrap();
    std::fs::write(derived.join(".ludion-sync.lock"), "").unwrap();
    let (ok, json, stderr) = sync(&dir);
    assert!(ok, "{stderr}");
    assert_eq!(stems(&json, "imported"), ["a", "b"]);
}

#[test]
fn staging_debris_is_swept_silently() {
    let dir = project("sync-debris");
    let derived = dir.join("assets/derived");
    let (ok, _, stderr) = sync(&dir);
    assert!(ok, "{stderr}");

    for leftover in [".sync-a", ".old-b"] {
        std::fs::create_dir_all(derived.join(leftover)).unwrap();
        std::fs::write(derived.join(leftover).join("x"), "debris").unwrap();
    }
    let (ok, json, stderr) = sync(&dir);
    assert!(ok, "{stderr}");
    assert!(stems(&json, "removed").is_empty());
    assert!(debris(&derived).is_empty());
}

/// Starts `ludion sync --json` without waiting for it.
fn spawn_sync(project: &Path) -> std::process::Child {
    ProcCommand::new(env!("CARGO_BIN_EXE_ludion"))
        .args(["sync", "--project", project.to_str().unwrap(), "--json"])
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .expect("failed to run the ludion binary")
}

const PROVISIONAL: &str = r#"{"importer":"other","entries":{}}"#;

/// Reads a child's stderr on a thread, forwarding each line; the channel closes when the child's
/// stderr does.
fn stderr_lines(child: &mut std::process::Child) -> Receiver<String> {
    let stderr = child.stderr.take().expect("sync stderr piped");
    let (tx, rx) = channel();
    std::thread::spawn(move || {
        for line in BufReader::new(stderr).lines().map_while(Result::ok) {
            if tx.send(line).is_err() {
                break;
            }
        }
    });
    rx
}

/// Consumes lines until one contains `needle`. Panics (with everything seen) after a 60 s hang guard
/// or when stderr closes first.
fn wait_stderr(rx: &Receiver<String>, needle: &str, label: &str) {
    let mut polls = 600;
    let mut seen = Vec::new();
    loop {
        match rx.recv_timeout(Duration::from_millis(100)) {
            Ok(line) if line.contains(needle) => return,
            Ok(line) => seen.push(line),
            Err(RecvTimeoutError::Timeout) => {
                polls -= 1;
                assert!(polls > 0, "timed out waiting for {label}; saw: {seen:?}");
            }
            Err(RecvTimeoutError::Disconnected) => {
                panic!("stderr closed while waiting for {label}; saw: {seen:?}");
            }
        }
    }
}

/// Waits for the child, then returns its success, parsed stdout object and all stderr.
fn finish(
    mut child: std::process::Child,
    stderr: Receiver<String>,
) -> (bool, serde_json::Value, String) {
    let status = child.wait().unwrap();
    let mut stdout = String::new();
    child
        .stdout
        .take()
        .unwrap()
        .read_to_string(&mut stdout)
        .unwrap();
    let stderr = stderr.iter().collect::<Vec<_>>().join("\n");
    let json = serde_json::from_str(stdout.trim())
        .unwrap_or_else(|e| panic!("stdout is not one JSON object ({e}): {stdout}\n{stderr}"));
    (status.success(), json, stderr)
}

/// Creates and exclusively locks `<derived>/.ludion-sync.lock`, as a running sync would.
fn hold_lock(derived: &Path) -> std::fs::File {
    std::fs::create_dir_all(derived).unwrap();
    let lock = std::fs::File::options()
        .create(true)
        .write(true)
        .truncate(false)
        .open(derived.join(".ludion-sync.lock"))
        .unwrap();
    lock.lock().unwrap();
    lock
}

const WAITING: &str = "waiting for the lock on";

#[test]
fn a_sync_waits_for_a_first_sync_that_has_swapped_a_stem() {
    let dir = project("sync-waits-for-first");
    let derived = dir.join("assets/derived");
    std::fs::create_dir_all(derived.join("a")).unwrap();
    let lock = hold_lock(&derived);
    std::fs::write(derived.join("a/junk.txt"), "swapped in by the first sync").unwrap();

    let mut child = spawn_sync(&dir);
    let rx = stderr_lines(&mut child);
    wait_stderr(&rx, WAITING, "the sync to report the lock wait");
    assert!(
        child.try_wait().unwrap().is_none(),
        "the second sync exited instead of waiting for the lock"
    );

    std::fs::write(derived.join(".ludion-sync.json"), PROVISIONAL).unwrap();
    drop(lock);
    let (ok, json, stderr) = finish(child, rx);
    assert!(ok, "{stderr}");
    assert_eq!(stems(&json, "imported"), ["a", "b"]);
    for stem in ["a", "b"] {
        assert_matches_hand_import(&dir, stem);
    }
    assert!(!derived.join("a/junk.txt").exists());
}

/// True when a sync of `dir` was killed while still running, after its first stem was swapped in
/// and before its final record: the record still has no entries and a stem directory exists.
fn kill_between_first_swap_and_final_record(dir: &Path) -> bool {
    let derived = dir.join("assets/derived");
    let mut child = spawn_sync(dir);
    let mut polls = 0;
    let running = loop {
        let swapped = derived.read_dir().is_ok_and(|read| {
            read.filter_map(Result::ok)
                .any(|entry| !entry.file_name().to_string_lossy().starts_with('.'))
        });
        let running = child.try_wait().unwrap().is_none();
        if swapped || polls > 60_000 || !running {
            break running;
        }
        polls += 1;
        std::thread::sleep(Duration::from_millis(1));
    };
    let _ = child.kill();
    let _ = child.wait();
    let stem = derived.read_dir().is_ok_and(|read| {
        read.filter_map(Result::ok)
            .any(|entry| !entry.file_name().to_string_lossy().starts_with('.'))
    });
    let record = std::fs::read_to_string(derived.join(".ludion-sync.json"));
    assert!(
        !stem || record.is_ok(),
        "a stem was swapped in before any record existed"
    );
    let provisional = record
        .ok()
        .and_then(|text| serde_json::from_str::<serde_json::Value>(&text).ok())
        .is_some_and(|record| record["entries"].as_object().is_some_and(|e| e.is_empty()));
    running && provisional && stem
}

#[test]
fn a_killed_first_sync_leaves_a_record_and_the_next_sync_recovers() {
    // The kill is a race against a fast sync, so an attempt counts only when it provably landed
    // between the first swap and the final record; a miss retries on a fresh project.
    let dir = (0..5)
        .find_map(|attempt| {
            let dir = project(&format!("sync-killed-first-{attempt}"));
            for index in 0..48 {
                let export = dir.join(format!("kits/k{index:02}/export"));
                std::fs::create_dir_all(&export).unwrap();
                std::fs::copy(textured(), export.join(format!("k{index:02}.gltf"))).unwrap();
            }
            kill_between_first_swap_and_final_record(&dir).then_some(dir)
        })
        .expect("the kill never landed between the first swap and the final record");
    let derived = dir.join("assets/derived");

    let (ok, json, stderr) = sync(&dir);
    assert!(ok, "{stderr}");
    assert!(!stderr.contains("L1245"), "{stderr}");
    assert_eq!(
        stems(&json, "imported").len() + stems(&json, "skipped").len(),
        50
    );
    assert!(debris(&derived).is_empty());
}

#[test]
fn a_provisional_record_with_stems_and_debris_recovers() {
    let dir = project("sync-provisional");
    let derived = dir.join("assets/derived");
    for leftover in ["a", ".sync-b", "gone"] {
        std::fs::create_dir_all(derived.join(leftover)).unwrap();
    }
    std::fs::write(derived.join("a/junk.txt"), "x").unwrap();
    std::fs::write(derived.join(".sync-b/x"), "x").unwrap();
    std::fs::write(derived.join(".ludion-sync.lock"), "").unwrap();
    std::fs::write(derived.join(".ludion-sync.json"), PROVISIONAL).unwrap();

    let (ok, json, stderr) = sync(&dir);
    assert!(ok, "{stderr}");
    assert_eq!(stems(&json, "imported"), ["a", "b"]);
    assert_eq!(stems(&json, "removed"), ["gone"]);
    assert!(!derived.join("gone").exists());
    assert!(debris(&derived).is_empty());
    for stem in ["a", "b"] {
        assert_matches_hand_import(&dir, stem);
    }
}

#[test]
fn json_is_printed_when_the_manifest_is_refused() {
    let dir = project("sync-json-manifest");
    std::fs::write(
        dir.join("ludion.toml"),
        "name = \"probe\"\ndefault_scene = \"scenes/main.yaml\"\n\n[[import]]\nfrom = \"kits/*/export/*.gltf\"\nto = \"assets/derived\"\n\n[[import]]\nfrom = \"kits/*/export/*.gltf\"\nto = \"assets/derived/inner\"\n",
    )
    .unwrap();
    let (ok, json, stderr) = sync(&dir);
    assert!(!ok);
    assert!(stderr.contains("L0023"), "{stderr}");
    assert_eq!(json["ok"], false);
    assert_eq!(diagnostic_codes(&json), ["L0023"]);
}

#[test]
fn a_failed_entry_keeps_its_diagnostic_objects_in_the_envelope() {
    let dir = project("sync-json-failed");
    std::fs::write(dir.join("kits/a/export/a.gltf"), "not a gltf").unwrap();
    let (ok, json, stderr) = sync(&dir);
    assert!(!ok);
    assert_eq!(json["ok"], false);
    assert_eq!(stems(&json, "imported"), ["b"]);
    let failed = json["failed"].as_array().unwrap();
    assert_eq!(failed.len(), 1);
    assert_eq!(failed[0]["stem"], "a");
    let code = failed[0]["diagnostics"][0]["code"].as_str().unwrap();
    assert!(code.starts_with('L'), "{json}");
    assert!(stderr.contains(code), "{stderr}");
    assert!(
        diagnostic_codes(&json).contains(&code.to_string()),
        "{json}"
    );
}

/// A directory link `link` -> `target`; a junction on Windows, which needs no privilege.
fn link_dir(link: &Path, target: &Path) {
    #[cfg(unix)]
    std::os::unix::fs::symlink(target, link).unwrap();
    #[cfg(windows)]
    if std::os::windows::fs::symlink_dir(target, link).is_err() {
        let status = ProcCommand::new("cmd")
            .args(["/c", "mklink", "/J"])
            .arg(link)
            .arg(target)
            .output()
            .unwrap()
            .status;
        assert!(status.success(), "could not create a junction");
    }
}

#[test]
fn a_symlink_under_a_source_directory_is_refused() {
    let dir = project("sync-symlink");
    link_dir(&dir.join("kits/a/export/loop"), &dir.join("kits/a"));

    let (ok, json, stderr) = sync(&dir);
    assert!(!ok);
    assert!(stderr.contains("L1245"), "{stderr}");
    assert!(stderr.contains("loop"), "{stderr}");
    assert_eq!(diagnostic_codes(&json), ["L1245"]);
    let derived = dir.join("assets/derived");
    assert!(!derived.exists() || collect_files(&derived) == [PathBuf::from(".ludion-sync.lock")]);
}
