//! `scene.load` / `scene.save` / `scene.check` accept only a content-root-relative path (`L3040`).

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use ludion_engine::{Engine, dispatch};
use ludion_protocol::command::{SceneCheckParams, SceneLoadParams, SceneSaveParams};
use ludion_protocol::{Command, ResponseBody};

const SCENE: &str = "format: 1\nseed: 7\nentities: []\n";

static COUNTER: AtomicU64 = AtomicU64::new(0);

/// A fresh, empty directory to use as a content root. Emptied first: nextest runs each test in its
/// own process and Windows reuses PIDs, so an earlier run's directory may share the name.
fn temp_root() -> PathBuf {
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!("ludion-scene-paths-{}-{n}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn engine_at(root: &Path) -> Engine {
    let mut engine = Engine::with_project_root(root);
    engine.load_inline_scene(SCENE).unwrap();
    engine
}

/// Paths that leave the content root: an absolute path to a real file, a `..` climb, a drive.
fn escaping_paths(outside: &Path) -> Vec<String> {
    vec![
        outside.to_string_lossy().into_owned(),
        "../x.yaml".to_string(),
        "sub\\..\\..\\x.yaml".to_string(),
        "C:/x.yaml".to_string(),
        "//host/share/x.yaml".to_string(),
        String::new(),
    ]
}

/// A real scene file outside the content root, standing in for any file a client could name.
fn outside_file() -> PathBuf {
    let dir = temp_root();
    let file = dir.join("outside.yaml");
    std::fs::write(&file, SCENE).unwrap();
    file
}

#[test]
fn load_refuses_paths_outside_the_root() {
    let root = temp_root();
    let outside = outside_file();
    let mut engine = engine_at(&root);
    for path in escaping_paths(&outside) {
        let error = dispatch(
            &mut engine,
            Command::SceneLoad(SceneLoadParams { path: path.clone() }),
        )
        .expect_err("an escaping path must be refused");
        assert_eq!(error.code.to_string(), "L3040", "{path:?}");
    }
}

#[test]
fn save_refuses_paths_outside_the_root_and_writes_nothing() {
    let root = temp_root();
    let outside = outside_file();
    let before = std::fs::read_to_string(&outside).unwrap();
    let mut engine = engine_at(&root);
    engine
        .load_inline_scene("format: 1\nseed: 9\nentities: []\n")
        .unwrap();
    for path in escaping_paths(&outside) {
        let error = dispatch(
            &mut engine,
            Command::SceneSave(SceneSaveParams {
                path: path.clone(),
                force: false,
            }),
        )
        .expect_err("an escaping path must be refused");
        assert_eq!(error.code.to_string(), "L3040", "{path:?}");
    }
    assert_eq!(std::fs::read_to_string(&outside).unwrap(), before);
    assert!(
        std::fs::read_dir(&root).unwrap().next().is_none(),
        "the root stays empty"
    );
}

#[test]
fn check_reports_an_invalid_result_rather_than_an_error() {
    let root = temp_root();
    let outside = outside_file();
    let mut engine = engine_at(&root);
    for path in escaping_paths(&outside) {
        let response = dispatch(
            &mut engine,
            Command::SceneCheck(SceneCheckParams { path: path.clone() }),
        )
        .expect("scene.check never errors");
        match response.body {
            ResponseBody::SceneChecked { valid, diagnostics } => {
                assert!(!valid, "{path:?}");
                assert_eq!(diagnostics.len(), 1, "{path:?}");
                assert_eq!(diagnostics[0].code.to_string(), "L3040", "{path:?}");
            }
            other => panic!("unexpected body: {other:?}"),
        }
    }
}

#[test]
fn save_refuses_a_generated_scene_and_writes_nothing() {
    let root = temp_root();
    let mut engine = engine_at(&root);
    engine
        .load_inline_scene("format: 1\ngenerated_from: town.map.yaml\nentities: []\n")
        .unwrap();
    let error = dispatch(
        &mut engine,
        Command::SceneSave(SceneSaveParams {
            path: "town.yaml".to_string(),
            force: false,
        }),
    )
    .expect_err("a generated scene must not save");
    assert_eq!(error.code.to_string(), "L1267");
    assert!(error.message.contains("town.map.yaml"), "{}", error.message);
    assert!(!root.join("town.yaml").exists(), "nothing was written");
}

#[test]
fn check_runs_the_engine_passes_a_missing_atlas_is_invalid() {
    let root = temp_root();
    std::fs::write(
        root.join("sprite.yaml"),
        "format: 1\nentities:\n  - name: s\n    components:\n      Sprite:\n        atlas: missing.atlas.yaml\n        frame: a\n",
    )
    .unwrap();
    let mut engine = engine_at(&root);
    let body = dispatch(
        &mut engine,
        Command::SceneCheck(SceneCheckParams {
            path: "sprite.yaml".to_string(),
        }),
    )
    .unwrap()
    .body;
    let ResponseBody::SceneChecked { valid, diagnostics } = body else {
        panic!("unexpected body");
    };
    assert!(!valid, "{diagnostics:?}");
    assert!(
        diagnostics.iter().any(|d| d.code.to_string() == "L1151"),
        "{diagnostics:?}"
    );
}

#[test]
fn a_relative_path_below_the_root_saves_checks_and_loads_back() {
    let root = temp_root();
    std::fs::create_dir_all(root.join("sub/dir")).unwrap();
    let mut engine = engine_at(&root);
    let path = "sub/dir/x.yaml".to_string();
    dispatch(
        &mut engine,
        Command::SceneSave(SceneSaveParams {
            path: path.clone(),
            force: false,
        }),
    )
    .expect("a relative save succeeds");
    assert!(root.join("sub/dir/x.yaml").is_file());
    match dispatch(
        &mut engine,
        Command::SceneCheck(SceneCheckParams { path: path.clone() }),
    )
    .unwrap()
    .body
    {
        ResponseBody::SceneChecked { valid, .. } => assert!(valid),
        other => panic!("unexpected body: {other:?}"),
    }
    dispatch(&mut engine, Command::SceneLoad(SceneLoadParams { path }))
        .expect("a relative load succeeds");
}
