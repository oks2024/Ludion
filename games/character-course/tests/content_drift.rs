//! The committed meshes and paths are derived from `scenes/blender/build.py`: the meshes by
//! importing `course.glb`, the paths straight from the script. Nothing here is hand-edited; a
//! failure names the way back.

use std::path::PathBuf;

use ludion_import::{ExternalFiles, ImportOptions, import};
use ludion_scene::{check_path, emit_mesh, emit_path};

const REGENERATE: &str = "re-run `blender -b --factory-startup --python scenes/blender/build.py -- \
    scenes/blender/course.glb`, then `ludion import scenes/blender/course.glb --out <scratch>` and copy \
    its `meshes/` into `scenes/meshes/`";

fn scenes() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("scenes")
}

fn files_in(dir: &str, suffix: &str) -> Vec<String> {
    let mut names: Vec<String> = std::fs::read_dir(scenes().join(dir))
        .expect("the scenes directory reads")
        .map(|entry| entry.unwrap().file_name().into_string().unwrap())
        .filter(|name| name.ends_with(suffix))
        .collect();
    names.sort();
    names
}

#[test]
fn committed_meshes_are_what_the_glb_imports_to() {
    let bytes = std::fs::read(scenes().join("blender/course.glb")).expect("course.glb reads");
    let imported = import(&bytes, ExternalFiles::Refused, &ImportOptions::lossy(false))
        .unwrap_or_else(|diags| panic!("course.glb imports: {diags:?}"));
    let mut expected = Vec::new();
    for artifact in &imported.meshes {
        let name = artifact
            .rel_path
            .strip_prefix("meshes/")
            .unwrap()
            .to_string();
        let committed =
            std::fs::read_to_string(scenes().join(&artifact.rel_path)).unwrap_or_else(|e| {
                panic!("{} is not committed ({e}); {REGENERATE}", artifact.rel_path)
            });
        assert!(
            emit_mesh(&artifact.doc) == committed,
            "{} drifted from course.glb; {REGENERATE}",
            artifact.rel_path
        );
        expected.push(name);
    }
    expected.sort();
    assert_eq!(
        files_in("meshes", ".mesh.yaml"),
        expected,
        "a committed mesh has no artifact in course.glb (or one is missing); {REGENERATE}"
    );
}

#[test]
fn committed_paths_are_canonical() {
    let names = files_in("paths", ".path.yaml");
    assert!(
        !names.is_empty(),
        "the course commits its platform and press paths"
    );
    for name in names {
        let text = std::fs::read_to_string(scenes().join("paths").join(&name)).unwrap();
        let doc = check_path(&text).unwrap_or_else(|diags| panic!("{name} is invalid: {diags:?}"));
        assert!(
            emit_path(&doc) == text,
            "{name} is not the canonical spelling; build.py writes it, so re-run the script"
        );
    }
}
