//! Cross-process integration test for `ludion dev` — the hot-reload supervisor, driven end to end.
//!
//! It scaffolds a real game with `ludion new`, starts `ludion dev` against it, and exercises the
//! whole loop against real process boundaries: a live client answers, a real source edit reloads, and
//! component state set over the wire *survives the restart* (snapshot fidelity through a process
//! boundary). A build-breaking edit leaves the last good child serving; a follow-up fix reloads
//! cleanly.
//!
//! The failure scenarios (a child that cannot boot, a refused restore, stale doc reloads, a wedged
//! child, a crashed child, a renamed component) share a second session,
//! `dev_keeps_the_snapshot_through_failed_boots`.
//!
//! Cost: this compiles a scaffold and rebuilds it several times, so it is slow — the same accepted
//! cost as `new.rs`'s `generated_project_passes_cargo_test`. `CARGO_TARGET_DIR` points at the
//! workspace target dir so the engine crates are reused warm rather than cold-built. The Wave 2
//! scenarios (state-survival + status sequence, and build-fail-keeps-child + fix-reloads) plus the
//! Wave 6 scenes-hint scenario (friction #5, `phase-7-sokoban-proof.md`) all run in **one dev session**
//! to pay the scaffold/first-build cost once.
//!
//! The failures test costs 70–165 s, inside the 10-minute ceiling and serialized by the `cargo-build`
//! nextest group. Trigger to split scenario I (the fresh `dev` resume) into its own test: check.mjs's
//! `test` step breaches its budget with this test on the slow list.

mod common;

use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::Duration;

use crossbeam_channel::{Receiver, RecvTimeoutError, unbounded};
use ludion_core::ids::EntityId;
use ludion_protocol::command::{ComponentGetParams, ComponentSetParams, QueryEntitiesParams};
use ludion_protocol::{Command as Wire, ResponseBody};

/// The engine workspace root (`.../crates/ludion-cli` -> hop up two).
fn engine_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("workspace root")
        .to_path_buf()
}

/// Owns a spawned `ludion dev` child and reaps its whole process tree on drop — including when a
/// `wait_line` timeout panics. On Windows `Child::kill` kills only the supervisor and orphans the
/// game grandchild (in `ludion-dev-session-<pid>/`); that grandchild keeps dev's piped stdout open and
/// wedges any parent waiting on the pipeline for many minutes. `taskkill /T` reaps the tree, so a
/// panicking run leaves no `ludion*` processes behind. Deref lets it stand in for the `Child`.
struct DevGuard(Child);

impl std::ops::Deref for DevGuard {
    type Target = Child;
    fn deref(&self) -> &Child {
        &self.0
    }
}

impl std::ops::DerefMut for DevGuard {
    fn deref_mut(&mut self) -> &mut Child {
        &mut self.0
    }
}

impl Drop for DevGuard {
    fn drop(&mut self) {
        #[cfg(windows)]
        {
            let _ = Command::new("taskkill")
                .args(["/F", "/T", "/PID", &self.0.id().to_string()])
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status();
        }
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

/// Reads dev's stdout on a thread, forwarding each line to a channel the test consumes with timeouts.
fn line_reader(child: &mut Child) -> Receiver<String> {
    let stdout = child.stdout.take().expect("dev stdout piped");
    let (tx, rx) = unbounded();
    std::thread::spawn(move || {
        for line in BufReader::new(stdout).lines() {
            match line {
                Ok(line) => {
                    if tx.send(line).is_err() {
                        break;
                    }
                }
                Err(_) => break,
            }
        }
    });
    rx
}

/// Consumes lines until one satisfies `pred`, returning it. Panics (with everything seen) if `timeout`
/// elapses first — a missing status line fails loudly rather than hanging.
///
/// The wait is a poll count over 100 ms `recv_timeout` slices rather than a wall-clock deadline: this
/// crate's determinism denylist bans `Instant::now` even in tests (the clock lives inside crossbeam's
/// `recv_timeout`, not here).
fn wait_line(
    rx: &Receiver<String>,
    label: &str,
    timeout: Duration,
    pred: impl Fn(&str) -> bool,
) -> String {
    let mut polls = (timeout.as_millis() / 100).max(1);
    let mut seen = Vec::new();
    loop {
        match rx.recv_timeout(Duration::from_millis(100)) {
            Ok(line) => {
                if pred(&line) {
                    return line;
                }
                seen.push(line);
            }
            Err(RecvTimeoutError::Timeout) => {
                polls -= 1;
                if polls == 0 {
                    panic!("timed out waiting for `{label}`; saw: {seen:?}");
                }
            }
            Err(RecvTimeoutError::Disconnected) => {
                panic!("dev stdout closed while waiting for `{label}`; saw: {seen:?}");
            }
        }
    }
}

/// Connects a client and returns the live `Player` entity's id (robust to scene ordering).
fn player_entity(url: &str) -> (ludion_client::Client, EntityId) {
    entity_with(url, "Player")
}

/// Connects a client and returns the first live entity carrying `component`.
fn entity_with(url: &str, component: &str) -> (ludion_client::Client, EntityId) {
    let mut client = ludion_client::Client::connect(url).expect("client connects to dev's child");
    let body = client
        .call(Wire::QueryEntities(QueryEntitiesParams {
            filter: vec![component.to_string()],
            include: Vec::new(),
            world: false,
            entities: None,
        }))
        .expect("query.entities")
        .body;
    let ResponseBody::QueryResult { entities } = body else {
        panic!("unexpected query body: {body:?}");
    };
    let entity = entities
        .first()
        .unwrap_or_else(|| panic!("a {component} entity exists"))
        .entity;
    (client, entity)
}

/// Reads a `Player` entity's `x` field over a fresh connection to `url`.
fn read_player_x(url: &str, entity: EntityId) -> f64 {
    let mut client = ludion_client::Client::connect(url).expect("client reconnects");
    let body = client
        .call(Wire::ComponentGet(ComponentGetParams {
            entity,
            component: "Player".to_string(),
        }))
        .expect("component.get Player")
        .body;
    let ResponseBody::ComponentValue { value } = body else {
        panic!("unexpected component.get body: {body:?}");
    };
    value
        .get("x")
        .and_then(serde_json::Value::as_f64)
        .expect("Player.x is a number")
}

/// Drains lines for `timeout`, panicking if any satisfies `pred` — proves a status line did NOT
/// appear (e.g. that a scenes-only edit never triggers a rebuild). The counterpart to [`wait_line`]:
/// same poll-count-over-`recv_timeout` shape, for the same determinism-denylist reason (no
/// `Instant::now` in this crate's tests either).
fn expect_no_line(
    rx: &Receiver<String>,
    label: &str,
    timeout: Duration,
    pred: impl Fn(&str) -> bool,
) {
    let mut polls = (timeout.as_millis() / 100).max(1);
    loop {
        match rx.recv_timeout(Duration::from_millis(100)) {
            Ok(line) => assert!(!pred(&line), "unexpected `{label}` line: {line}"),
            Err(RecvTimeoutError::Timeout) => {
                polls -= 1;
                if polls == 0 {
                    return;
                }
            }
            Err(RecvTimeoutError::Disconnected) => {
                panic!("dev stdout closed while confirming no `{label}`")
            }
        }
    }
}

/// Overwrites the scaffold's `PLAYER_SPEED` constant — a real code change that recompiles. The
/// tunable lives in the scaffold's `src/components.rs` (the multi-module layout).
fn set_player_speed(components: &Path, from: &str, to: &str) {
    let text = std::fs::read_to_string(components).expect("read components.rs");
    let updated = text.replace(
        &format!("PLAYER_SPEED: f32 = {from}"),
        &format!("PLAYER_SPEED: f32 = {to}"),
    );
    assert_ne!(updated, text, "expected to change PLAYER_SPEED from {from}");
    std::fs::write(components, updated).expect("write components.rs");
}

/// Builds the scaffold with the invocation, target dir and profile `ludion dev` uses, so the engine
/// dependencies are compiled before the "first listening" clock starts and dev's own build is left
/// with the game crate alone. A cold or contended build can take many minutes; the wait is unbounded
/// and a failed build fails the test with cargo's own stderr.
fn prebuild(project: &Path, target_dir: &Path) {
    let status = Command::new("cargo")
        .arg("build")
        .current_dir(project)
        .env("CARGO_TARGET_DIR", target_dir)
        .status()
        .expect("run `cargo build` on the scaffold");
    assert!(status.success(), "pre-building the scaffold failed");
}

#[test]
fn dev_reloads_and_preserves_state_across_a_real_edit() {
    let (base, _lock) = common::stable_scratch("dev_session");
    let status = Command::new(env!("CARGO_BIN_EXE_ludion"))
        .arg("new")
        .arg("dev_game")
        .current_dir(&base)
        .status()
        .expect("run `ludion new`");
    assert!(status.success(), "`ludion new dev_game` failed");

    let project = base.join("dev_game");
    let components = project.join("src").join("components.rs");
    let target_dir = engine_root().join("target");

    // A glTF kit and the `[[import]]` that derives it: dev syncs it before the first build.
    let export = project.join("art").join("kit").join("export");
    std::fs::create_dir_all(&export).expect("create the kit export dir");
    std::fs::copy(
        engine_root()
            .join("examples")
            .join("import-textured")
            .join("scene.gltf"),
        export.join("kit.gltf"),
    )
    .expect("copy the kit glTF");
    let manifest = project.join("ludion.toml");
    let mut text = std::fs::read_to_string(&manifest).expect("read the scaffold manifest");
    text.push_str("\n[[import]]\nfrom = \"art/*/export/*.gltf\"\nto = \"assets/derived\"\n");
    std::fs::write(&manifest, text).expect("write the scaffold manifest");

    prebuild(&project, &target_dir);

    // Start dev on an OS-chosen port; it builds the scaffold then serves. Wrapped in a
    // `DevGuard` so a `wait_line` panic still reaps the dev + game process tree (see `DevGuard`).
    let mut dev = DevGuard(
        Command::new(env!("CARGO_BIN_EXE_ludion"))
            .arg("dev")
            .arg("--addr")
            .arg("127.0.0.1:0")
            .current_dir(&project)
            .env("CARGO_TARGET_DIR", &target_dir)
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .expect("spawn `ludion dev`"),
    );

    let lines = line_reader(&mut dev);

    // First start: the child's listening line (relayed verbatim), then dev's watching line.
    let listening = wait_line(&lines, "first listening", Duration::from_secs(180), |l| {
        l.starts_with("listening on ws://")
    });
    let url = listening
        .trim()
        .strip_prefix("listening on ")
        .expect("a ws:// url")
        .to_string();
    wait_line(&lines, "watching", Duration::from_secs(10), |l| {
        l.starts_with("dev watching")
    });
    let derived = project.join("assets").join("derived");
    assert!(
        derived.join("kit").join("model.prefab.yaml").is_file(),
        "dev should have derived the kit before serving"
    );
    assert!(derived.join(".ludion-sync.json").is_file());

    // The live sim answers, and we can set component state over the wire.
    let (mut client, player) = player_entity(&url);
    client
        .call(Wire::ComponentSet(ComponentSetParams {
            entity: player,
            component: "Player".to_string(),
            value: serde_json::json!({ "x": 42.5 }),
        }))
        .expect("component.set Player.x");
    drop(client);

    // --- Scenario 1: a real source edit reloads, and the set state survives the restart. ---
    set_player_speed(&components, "6.0", "7.0");

    wait_line(&lines, "reload building", Duration::from_secs(30), |l| {
        l == "dev reload building"
    });
    // The new child rebinds the SAME port.
    let relisten = wait_line(&lines, "reload listening", Duration::from_secs(180), |l| {
        l.starts_with("listening on ws://")
    });
    assert!(
        relisten.contains(url.strip_prefix("ws://").unwrap()),
        "reload must rebind the same port: first {url}, reload {relisten}",
    );
    let restored = wait_line(&lines, "reload restored", Duration::from_secs(60), |l| {
        l.starts_with("dev reload restored frame=")
    });
    // Log the real touch->restored loop time (Wave 6 gates it; here it is data, not a gate).
    let ms: u64 = restored
        .rsplit(" in ")
        .next()
        .and_then(|tail| tail.trim_end_matches(" ms").parse().ok())
        .expect("restored line carries a ms number");
    eprintln!("MEASURED ludion dev loop time (touch -> restored, debug): {ms} ms");

    // Snapshot fidelity through a real process boundary: the value we set pre-reload is still there.
    let survived = read_player_x(&url, player);
    assert_eq!(survived, 42.5, "component state must survive the reload");

    // --- Scenario 2: a build-breaking edit leaves the old child serving; a fix reloads cleanly. ---
    let good = std::fs::read_to_string(&components).expect("read components.rs");
    std::fs::write(&components, format!("{good}\nfn __dev_build_break__( {{\n"))
        .expect("write a broken components.rs");

    wait_line(&lines, "break building", Duration::from_secs(30), |l| {
        l == "dev reload building"
    });
    // A build under load takes over a minute (measured 66 s for a successful reload with 32 busy loops).
    wait_line(&lines, "build failed", Duration::from_secs(240), |l| {
        l == "dev reload build failed"
    });
    // The last good binary is still serving on the same port, with the reloaded state intact.
    assert_eq!(
        read_player_x(&url, player),
        42.5,
        "a failed build must leave the old child serving its state",
    );

    // Fix the build: the loop recovers.
    std::fs::write(&components, &good).expect("restore a good components.rs");
    wait_line(&lines, "fix building", Duration::from_secs(30), |l| {
        l == "dev reload building"
    });
    wait_line(&lines, "fix listening", Duration::from_secs(180), |l| {
        l.starts_with("listening on ws://")
    });
    wait_line(&lines, "fix restored", Duration::from_secs(60), |l| {
        l.starts_with("dev reload restored frame=")
    });
    assert_eq!(
        read_player_x(&url, player),
        42.5,
        "state survives the recovery reload too",
    );

    // --- Scenario 3: a scenes/ edit prints the hint, never a rebuild (Wave 6, friction #5 fix). ---
    let scene_file = project.join("scenes").join("main.yaml");
    let scene_text = std::fs::read_to_string(&scene_file).expect("read scenes/main.yaml");
    std::fs::write(
        &scene_file,
        format!("{scene_text}\n# touched by the dev test\n"),
    )
    .expect("touch scenes/main.yaml");

    wait_line(&lines, "scenes hint", Duration::from_secs(10), |l| {
        l == "dev scenes changed (world state is preserved; assets and data-docs reload live, scene edits need scene.load or restart)"
    });
    // No rebuild follows: a scenes-only edit must never reach the src watcher's reload path.
    expect_no_line(&lines, "dev reload building", Duration::from_secs(3), |l| {
        l == "dev reload building"
    });
    // The world is untouched too — the same state set before scenario 1 is still there.
    assert_eq!(
        read_player_x(&url, player),
        42.5,
        "a scenes/ edit must not mutate the running world",
    );

    // Teardown: hard-kill dev alone (`Child::kill` on Windows spares the game grandchild — exactly the
    // orphan case) and require the game to notice and free its port by itself.
    let _ = dev.kill();
    let _ = dev.wait();
    let mut attempts = 0;
    while ludion_client::Client::connect(&url).is_ok() {
        attempts += 1;
        assert!(
            attempts <= 150,
            "the game child outlived `ludion dev`: {url} still answers ~15 s after dev was killed",
        );
        std::thread::sleep(Duration::from_millis(100));
    }
    let _ = std::io::stdout().flush();
}

/// Polls `dev-status.json` (dev writes it next to the matching stdout line) until its `state` is
/// `want`, returning the document. Panics with the last document seen.
fn wait_status(project: &Path, want: &str) -> serde_json::Value {
    wait_status_where(project, want, |_| true)
}

/// The `seq` of the status file's current document — taken before an edit, so a wait can demand a
/// document written after it.
fn status_seq(project: &Path) -> u64 {
    let text = std::fs::read_to_string(project.join(".ludion").join("dev-status.json"))
        .expect("dev wrote a status file");
    let doc: serde_json::Value = serde_json::from_str(&text).expect("the status file parses");
    doc["seq"].as_u64().expect("a status seq")
}

/// [`wait_status`] that also requires `pred` of the document: an earlier scenario's stale `ok` is
/// already in the file, so a later scenario names what only its own `ok` carries.
fn wait_status_where(
    project: &Path,
    want: &str,
    pred: impl Fn(&serde_json::Value) -> bool,
) -> serde_json::Value {
    let path = project.join(".ludion").join("dev-status.json");
    let mut last = String::new();
    for _ in 0..100 {
        if let Ok(text) = std::fs::read_to_string(&path) {
            if let Ok(doc) = serde_json::from_str::<serde_json::Value>(&text)
                && doc.get("state").and_then(serde_json::Value::as_str) == Some(want)
                && pred(&doc)
            {
                return doc;
            }
            last = text;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    panic!("dev-status.json never reached `{want}`; last: {last}");
}

/// Sets the live `Player.x` over a fresh connection — the trigger the scaffold's `test_triggers`
/// system reacts to.
fn set_player_x(url: &str, x: f64) {
    send_player_x(url, x).expect("component.set Player.x");
}

/// Sets `Player.x` to the crash trigger. The child may die before its reply is written, so a lost
/// connection (L3009) is the expected outcome as often as an `Ok` (measured under 32 busy loops: a
/// reset); a call timeout or any other error, such as a rejected `component.set`, fails the test.
fn trigger_crash(url: &str, x: f64) {
    match send_player_x(url, x) {
        Ok(()) => {}
        Err(e) if e.code.as_str() == "L3009" && !e.message.contains("call timeout") => {}
        Err(e) => panic!("the crash trigger failed for a reason other than the child dying: {e:?}"),
    }
}

fn send_player_x(url: &str, x: f64) -> Result<(), ludion_core::diagnostics::Diagnostic> {
    let (mut client, player) = player_entity(url);
    client
        .call(Wire::ComponentSet(ComponentSetParams {
            entity: player,
            component: "Player".to_string(),
            value: serde_json::json!({ "x": x }),
        }))
        .map(drop)
}

/// The first value stored under `key` anywhere in `value`, depth first: a layout-agnostic probe of a
/// snapshot document.
fn find_key<'a>(value: &'a serde_json::Value, key: &str) -> Option<&'a serde_json::Value> {
    match value {
        serde_json::Value::Object(map) => map
            .get(key)
            .or_else(|| map.values().find_map(|v| find_key(v, key))),
        serde_json::Value::Array(items) => items.iter().find_map(|v| find_key(v, key)),
        _ => None,
    }
}

/// An `engine.snapshot` of the child at `url`, on a fresh connection.
fn snapshot_of(url: &str) -> serde_json::Value {
    let mut client = ludion_client::Client::connect(url).expect("client connects");
    match client
        .call(Wire::EngineSnapshot)
        .expect("engine.snapshot")
        .body
    {
        ResponseBody::Snapshot { snapshot } => snapshot,
        other => panic!("unexpected engine.snapshot body: {other:?}"),
    }
}

/// The snapshot's logged data-doc reloads (empty when the key is absent).
fn doc_reloads(snapshot: &serde_json::Value) -> Vec<serde_json::Value> {
    snapshot
        .get("doc_reloads")
        .and_then(serde_json::Value::as_array)
        .cloned()
        .unwrap_or_default()
}

/// A reload whose child cannot boot keeps `dev` alive and the designer's snapshot on disk; the next
/// save retries it. A failed restore keeps the snapshot too, the session's live data-doc reloads
/// never outlive a recompile, and a component the boot scene uses may be renamed away: the child
/// boots from the snapshot, never the scene, and dev reports what the restore dropped.
#[test]
fn dev_keeps_the_snapshot_through_failed_boots() {
    let (base, _lock) = common::stable_scratch("dev_failures");
    let status = Command::new(env!("CARGO_BIN_EXE_ludion"))
        .arg("new")
        .arg("dev_failures_game")
        .current_dir(&base)
        .status()
        .expect("run `ludion new`");
    assert!(status.success(), "`ludion new dev_failures_game` failed");

    let project = base.join("dev_failures_game");
    let components = project.join("src").join("components.rs");
    let lib = project.join("src").join("lib.rs");
    let tuning = project.join("scenes").join("tuning.yaml");
    let kept = project.join(".ludion").join("dev-snapshot.json");
    let target_dir = engine_root().join("target");

    // A game data-doc kind, so a live doc reload exists to be logged in the snapshot.
    let mut text = std::fs::read_to_string(&components).expect("read components.rs");
    text.push_str(
        "\n/// A tuning doc.\n#[derive(Serialize, Deserialize, JsonSchema, Clone)]\npub struct Tuning {\n    /// Speed.\n    pub speed: f32,\n}\n",
    );
    std::fs::write(&components, text).expect("write components.rs");
    let good_lib = std::fs::read_to_string(&lib)
        .expect("read lib.rs")
        .replace(
            ".components(register)",
            ".components(register)\n        .doc_kind::<Tuning>(\"tuning\")",
        )
        .replace(
            ".system(\"move_player\", move_player)",
            ".system(\"move_player\", move_player)\n        .system(\"test_triggers\", test_triggers)",
        );
    assert!(
        good_lib.contains("doc_kind") && good_lib.contains("test_triggers"),
        "expected to add the doc kind and the trigger system"
    );
    // A system a test steers over the wire: it wedges the sim (a child that never answers) or kills
    // the process (a child that exits) when `Player.x` takes a sentinel value.
    let systems = project.join("src").join("systems.rs");
    let mut text = std::fs::read_to_string(&systems).expect("read systems.rs");
    text.push_str(
        "\n/// Wedges or ends the process on a sentinel `Player.x`, for the dev supervisor's tests.\npub fn test_triggers(world: &mut World, _ctx: &SystemCtx) {\n    for (player,) in world.query::<(&Player,)>() {\n        if player.x == -1.0 {\n            loop {\n                std::hint::spin_loop();\n            }\n        }\n        if player.x == -2.0 {\n            std::process::exit(3);\n        }\n    }\n}\n",
    );
    std::fs::write(&systems, text).expect("write systems.rs");
    std::fs::write(&lib, &good_lib).expect("write lib.rs");
    std::fs::write(&tuning, "format: 1\nkind: tuning\nspeed: 1.0\n").expect("write tuning.yaml");
    // A boot failure no snapshot can mend: `Transform` is already a built-in, so the engine build
    // itself refuses (L0005) whether or not the child boots from a snapshot. (A renamed component is
    // not one: the snapshot restores around it — scenario E.)
    let broken_lib = good_lib.replace(
        "register::<Player>(\"Player\")",
        "register::<Player>(\"Transform\")",
    );
    assert_ne!(broken_lib, good_lib, "expected to collide the registration");

    prebuild(&project, &target_dir);

    let mut dev = DevGuard(
        Command::new(env!("CARGO_BIN_EXE_ludion"))
            .arg("dev")
            .arg("--addr")
            .arg("127.0.0.1:0")
            .current_dir(&project)
            .env("CARGO_TARGET_DIR", &target_dir)
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .expect("spawn `ludion dev`"),
    );
    let lines = line_reader(&mut dev);

    let listening = wait_line(&lines, "first listening", Duration::from_secs(180), |l| {
        l.starts_with("listening on ws://")
    });
    let url = listening
        .trim()
        .strip_prefix("listening on ")
        .expect("a ws:// url")
        .to_string();
    wait_line(&lines, "watching", Duration::from_secs(10), |l| {
        l.starts_with("dev watching")
    });

    let (mut client, player) = player_entity(&url);
    client
        .call(Wire::ComponentSet(ComponentSetParams {
            entity: player,
            component: "Player".to_string(),
            value: serde_json::json!({ "x": 42.5 }),
        }))
        .expect("component.set Player.x");
    drop(client);

    // --- A: a rebuilt child that cannot boot keeps dev alive and the snapshot on disk. ---
    std::fs::write(&lib, &broken_lib).expect("write a lib.rs the engine rejects");
    wait_line(&lines, "A building", Duration::from_secs(30), |l| {
        l == "dev reload building"
    });
    wait_line(&lines, "A launch failed", Duration::from_secs(180), |l| {
        l.starts_with("dev reload launch failed; snapshot kept at")
    });
    assert!(
        dev.try_wait().expect("poll dev").is_none(),
        "a child that cannot boot must not end dev"
    );
    let failed = wait_status(&project, "failed");
    assert!(failed.get("snapshot").is_some(), "status: {failed}");
    let codes: Vec<&str> = failed["diagnostics"]
        .as_array()
        .expect("failed carries diagnostics")
        .iter()
        .filter_map(|d| d["code"].as_str())
        .collect();
    assert!(codes.contains(&"L0013"), "status diagnostics: {codes:?}");
    let doc: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&kept).expect("the kept snapshot exists"))
            .expect("the kept snapshot parses");
    let kept_player = find_key(&doc, "Player").expect("the kept snapshot holds a Player");
    assert_eq!(kept_player["x"], 42.5, "kept Player: {kept_player}");

    // --- B: the next save relaunches and restores the kept snapshot. ---
    std::fs::write(&lib, &good_lib).expect("revert lib.rs");
    wait_line(&lines, "B building", Duration::from_secs(30), |l| {
        l == "dev reload building"
    });
    let relisten = wait_line(&lines, "B listening", Duration::from_secs(180), |l| {
        l.starts_with("listening on ws://")
    });
    assert_eq!(
        relisten.trim(),
        listening.trim(),
        "same port after a failed boot"
    );
    wait_line(&lines, "B restored", Duration::from_secs(60), |l| {
        l.starts_with("dev reload restored frame=")
    });
    let ok = wait_status(&project, "ok");
    assert!(ok.get("restored_frame").is_some(), "status: {ok}");
    let (client, player) = player_entity(&url);
    drop(client);
    assert_eq!(
        read_player_x(&url, player),
        42.5,
        "the retry restores the snapshot"
    );

    // --- C: a failed restore keeps the snapshot; deleting the file drops it. ---
    std::fs::write(&lib, &broken_lib).expect("break lib.rs again");
    wait_line(&lines, "C building", Duration::from_secs(30), |l| {
        l == "dev reload building"
    });
    wait_line(&lines, "C launch failed", Duration::from_secs(180), |l| {
        l.starts_with("dev reload launch failed; snapshot kept at")
    });
    let mut doc: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&kept).expect("the kept snapshot exists"))
            .expect("the kept snapshot parses");
    doc["format"] = serde_json::json!(999);
    std::fs::write(&kept, serde_json::to_string_pretty(&doc).unwrap())
        .expect("corrupt the snapshot");
    std::fs::write(&lib, &good_lib).expect("revert lib.rs");
    wait_line(&lines, "C restore failed", Duration::from_secs(240), |l| {
        l.starts_with("dev reload restore failed; running a fresh world")
    });
    wait_status(&project, "restore_failed");
    let (client, player) = player_entity(&url);
    drop(client);
    assert_eq!(
        read_player_x(&url, player),
        0.0,
        "a fresh world, not the snapshot"
    );
    std::fs::remove_file(&kept).expect("delete the kept snapshot");
    set_player_speed(&components, "6.0", "7.0");
    wait_line(&lines, "C dropped", Duration::from_secs(240), |l| {
        l.starts_with("dev reload restored frame=")
    });

    // --- D: a recompile restores the docs from disk, not the session's live-reload log. ---
    std::fs::write(&tuning, "format: 1\nkind: tuning\nspeed: 2.0\n").expect("edit tuning.yaml");
    let mut logged = false;
    for _ in 0..100 {
        if !doc_reloads(&snapshot_of(&url)).is_empty() {
            logged = true;
            break;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    assert!(
        logged,
        "the live doc reload was never logged in the snapshot"
    );
    // The old child rejects `armor`; the rebuilt one requires it.
    std::fs::write(&tuning, "format: 1\nkind: tuning\nspeed: 2.0\narmor: 5.0\n")
        .expect("add armor to tuning.yaml");
    let text = std::fs::read_to_string(&components).expect("read components.rs");
    std::fs::write(
        &components,
        text.replace(
            "pub speed: f32,",
            "pub speed: f32,\n    /// Armor.\n    pub armor: f32,",
        ),
    )
    .expect("add the armor field");
    wait_line(&lines, "D building", Duration::from_secs(30), |l| {
        l == "dev reload building"
    });
    let outcome = wait_line(&lines, "D outcome", Duration::from_secs(240), |l| {
        l.starts_with("dev reload restored frame=") || l.starts_with("dev reload restore failed")
    });
    assert!(
        outcome.starts_with("dev reload restored frame="),
        "the reload must restore, not replay the stale doc log: {outcome}"
    );
    let stale = doc_reloads(&snapshot_of(&url));
    assert!(
        stale.is_empty(),
        "stale doc reloads survived the recompile: {stale:?}"
    );

    // --- F: a wedged child is stopped and replaced, never left serving the old build. ---
    set_player_x(&url, -1.0);
    let seq_before = status_seq(&project);
    set_player_speed(&components, "7.0", "8.0");
    wait_line(&lines, "F building", Duration::from_secs(30), |l| {
        l == "dev reload building"
    });
    let failed = wait_line(&lines, "F snapshot failed", Duration::from_secs(240), |l| {
        l.starts_with("dev reload snapshot failed (")
    });
    assert!(failed.contains("session state lost"), "line: {failed}");
    let relisten = wait_line(&lines, "F listening", Duration::from_secs(240), |l| {
        l.starts_with("listening on ws://")
    });
    assert_eq!(relisten.trim(), listening.trim(), "same port after a wedge");
    let ok = wait_status_where(&project, "ok", |doc| {
        doc["seq"].as_u64() > Some(seq_before) && doc.get("message").is_some()
    });
    assert!(ok.get("restored_frame").is_none(), "status: {ok}");
    let (client, player) = player_entity(&url);
    drop(client);
    assert_eq!(
        read_player_x(&url, player),
        0.0,
        "a fresh world, and a child that answers"
    );

    // --- G: a crashed child relaunches fresh and the status ends on `ok`. ---
    trigger_crash(&url, -2.0);
    let mut gone = false;
    for _ in 0..100 {
        if ludion_client::Client::connect(&url).is_err() {
            gone = true;
            break;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    assert!(gone, "the child never exited");
    let seq_before = status_seq(&project);
    set_player_speed(&components, "8.0", "9.0");
    wait_line(&lines, "G building", Duration::from_secs(30), |l| {
        l == "dev reload building"
    });
    wait_line(&lines, "G child exited", Duration::from_secs(240), |l| {
        l == "dev reload child exited; state lost, relaunching"
    });
    wait_line(&lines, "G listening", Duration::from_secs(240), |l| {
        l.starts_with("listening on ws://")
    });
    let ok = wait_status_where(&project, "ok", |doc| {
        doc["seq"].as_u64() > Some(seq_before) && doc.get("message").is_some()
    });
    assert!(ok.get("restored_frame").is_none(), "status: {ok}");

    // --- E: a renamed component the boot scene uses restores around it, reported as L3015. ---
    let camera_translation = [1.5, 2.5, 6.0];
    let (mut client, camera) = entity_with(&url, "Camera");
    let ResponseBody::ComponentValue {
        value: mut transform,
    } = client
        .call(Wire::ComponentGet(ComponentGetParams {
            entity: camera,
            component: "Transform".to_string(),
        }))
        .expect("component.get Transform")
        .body
    else {
        panic!("component.get replies ComponentValue");
    };
    transform["translation"] = serde_json::json!(camera_translation);
    client
        .call(Wire::ComponentSet(ComponentSetParams {
            entity: camera,
            component: "Transform".to_string(),
            value: transform,
        }))
        .expect("component.set Transform");
    drop(client);
    let renamed_lib = good_lib.replace(
        "register::<Player>(\"Player\")",
        "register::<Player>(\"PlayerRenamed\")",
    );
    assert_ne!(renamed_lib, good_lib, "expected to rename the registration");
    let seq_before = status_seq(&project);
    std::fs::write(&lib, &renamed_lib).expect("rename the registration");
    wait_line(&lines, "E building", Duration::from_secs(30), |l| {
        l == "dev reload building"
    });
    wait_line(&lines, "E dropped Player", Duration::from_secs(240), |l| {
        l.contains("[L3015]") && l.contains("`Player`")
    });
    wait_line(&lines, "E restored", Duration::from_secs(60), |l| {
        l.starts_with("dev reload restored frame=")
    });
    let ok = wait_status_where(&project, "ok", |doc| {
        doc["seq"].as_u64() > Some(seq_before) && doc.get("restored_frame").is_some()
    });
    let entries = ok["report"]["entries"]
        .as_array()
        .unwrap_or_else(|| panic!("the ok status carries the full report: {ok}"));
    assert!(
        entries.iter().any(
            |entry| entry["kind"] == "component_unregistered" && entry["component"] == "Player"
        ),
        "report entries: {entries:?}"
    );
    let (mut client, camera) = entity_with(&url, "Camera");
    let ResponseBody::ComponentValue { value } = client
        .call(Wire::ComponentGet(ComponentGetParams {
            entity: camera,
            component: "Transform".to_string(),
        }))
        .expect("component.get Transform")
        .body
    else {
        panic!("component.get replies ComponentValue");
    };
    assert_eq!(
        value["translation"],
        serde_json::json!(camera_translation),
        "the snapshot's camera, not the scene's (a fresh world has [0, 0, 6])"
    );

    // --- H: a restored snapshot leaves no file, so a later start does not resume it. ---
    assert!(
        !kept.exists(),
        "the restored snapshot must be deleted: the file exists only while a snapshot is kept"
    );

    // --- I: a fresh `ludion dev` resumes a snapshot a previous session left kept. ---
    let resumed_translation = [3.5, 4.5, 6.0];
    set_camera_translation(&url, resumed_translation);
    let broken_renamed = renamed_lib.replace(
        "register::<Player>(\"PlayerRenamed\")",
        "register::<Player>(\"Transform\")",
    );
    assert_ne!(broken_renamed, renamed_lib, "expected to collide again");
    std::fs::write(&lib, &broken_renamed).expect("break lib.rs a last time");
    wait_line(&lines, "I building", Duration::from_secs(30), |l| {
        l == "dev reload building"
    });
    wait_line(&lines, "I launch failed", Duration::from_secs(240), |l| {
        l.starts_with("dev reload launch failed; snapshot kept at")
    });
    assert!(kept.is_file(), "the failed boot keeps the snapshot");
    drop(dev);

    std::fs::write(&lib, &renamed_lib).expect("restore the good source");
    let mut dev = DevGuard(
        Command::new(env!("CARGO_BIN_EXE_ludion"))
            .arg("dev")
            .arg("--addr")
            .arg("127.0.0.1:0")
            .current_dir(&project)
            .env("CARGO_TARGET_DIR", &target_dir)
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .expect("respawn `ludion dev`"),
    );
    let lines = line_reader(&mut dev);
    let listening = wait_line(&lines, "I listening", Duration::from_secs(240), |l| {
        l.starts_with("listening on ws://")
    });
    let url = listening
        .trim()
        .strip_prefix("listening on ")
        .expect("a ws:// url")
        .to_string();
    wait_line(&lines, "I resumed", Duration::from_secs(60), |l| {
        l.starts_with("dev resumed frame=")
    });
    let ok = wait_status_where(&project, "ok", |doc| doc.get("restored_frame").is_some());
    assert!(ok.get("message").is_some(), "status: {ok}");
    assert_eq!(
        camera_translation_of(&url),
        serde_json::json!(resumed_translation),
        "the kept snapshot's camera, not the scene's (a fresh world has [0, 0, 6])"
    );
    assert!(!kept.exists(), "the resumed snapshot is deleted");
}

/// The live camera's `Transform.translation`.
fn camera_translation_of(url: &str) -> serde_json::Value {
    let (mut client, camera) = entity_with(url, "Camera");
    let ResponseBody::ComponentValue { value } = client
        .call(Wire::ComponentGet(ComponentGetParams {
            entity: camera,
            component: "Transform".to_string(),
        }))
        .expect("component.get Transform")
        .body
    else {
        panic!("component.get replies ComponentValue");
    };
    value["translation"].clone()
}

/// Sets the live camera's `Transform.translation`.
fn set_camera_translation(url: &str, translation: [f64; 3]) {
    let (mut client, camera) = entity_with(url, "Camera");
    let ResponseBody::ComponentValue {
        value: mut transform,
    } = client
        .call(Wire::ComponentGet(ComponentGetParams {
            entity: camera,
            component: "Transform".to_string(),
        }))
        .expect("component.get Transform")
        .body
    else {
        panic!("component.get replies ComponentValue");
    };
    transform["translation"] = serde_json::json!(translation);
    client
        .call(Wire::ComponentSet(ComponentSetParams {
            entity: camera,
            component: "Transform".to_string(),
            value: transform,
        }))
        .expect("component.set Transform");
}
