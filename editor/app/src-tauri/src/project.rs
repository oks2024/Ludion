//! The game project the editor is pointed at: a directory holding `ludion.toml`. Reading it
//! (`project_info`), finding the `--project` launch argument (`launch_project`) and adding the
//! missing stage scenes (`add_stage_scenes`) are the host-side pieces the frontend's boot probe
//! (`App.tsx`), `scripts/editor.mjs` and the stage views need.

use std::path::Path;

use serde::{Deserialize, Serialize};

/// What the frontend shows for the current project: its directory (as given, not canonicalized),
/// and the two `ludion.toml` fields the titlebar and the terminal drawer need.
#[derive(Debug, Clone, Serialize)]
pub struct ProjectInfo {
    dir: String,
    name: String,
    #[serde(rename = "defaultScene")]
    default_scene: String,
}

/// The subset of `ludion.toml` this reader needs. Unlike the engine's `ludion-cli` manifest
/// reader (`crates/ludion-cli/src/manifest.rs`, `deny_unknown_fields`), this one is lenient: an
/// `[ios]` table or any future key must pass, since the editor only ever displays `name` and
/// `default_scene`.
#[derive(Debug, Deserialize)]
struct Manifest {
    name: String,
    default_scene: String,
}

/// Reads `<dir>/ludion.toml`. A missing or unreadable file is refused with the decision-1 message
/// (kept byte-identical with `scripts/editor.mjs`'s copy); a malformed manifest, or one missing a
/// required key, is refused as an invalid manifest.
pub fn read_project(dir: &Path) -> Result<ProjectInfo, String> {
    let manifest_path = dir.join("ludion.toml");
    let text = std::fs::read_to_string(&manifest_path).map_err(|_| {
        format!(
            "`{}` has no ludion.toml — a Ludion project is what `ludion new` creates; the engine \
             checkout itself is not one.",
            dir.display()
        )
    })?;
    let manifest: Manifest = toml::from_str(&text).map_err(|err| {
        format!(
            "`{}` is not a valid manifest: {err}",
            manifest_path.display()
        )
    })?;
    Ok(ProjectInfo {
        dir: dir.display().to_string(),
        name: manifest.name,
        default_scene: manifest.default_scene,
    })
}

/// The `--project <dir>` / `--project=<dir>` value in `args`, or `None` when the flag is absent or
/// its value is missing/empty. Everything else in `args` is ignored.
fn launch_project_from(args: &[String]) -> Option<String> {
    let mut iter = args.iter();
    while let Some(arg) = iter.next() {
        if let Some(value) = arg.strip_prefix("--project=") {
            return (!value.is_empty()).then(|| value.to_string());
        }
        if arg == "--project" {
            return iter.next().filter(|v| !v.is_empty()).cloned();
        }
    }
    None
}

/// The `--project` value the process was launched with, from `std::env::args()`.
pub fn launch_project() -> Option<String> {
    launch_project_from(&std::env::args().collect::<Vec<_>>())
}

#[tauri::command]
pub fn project_info(dir: String) -> Result<ProjectInfo, String> {
    read_project(Path::new(&dir))
}

/// The three stage scenes every stage view needs (`Plan/editor-open-project.md` decision 6).
pub const STAGE_SCENES: [&str; 3] = [
    "model-stage.yaml",
    "model-stage-night.yaml",
    "effects-stage.yaml",
];

/// Copies every `STAGE_SCENES` file missing from `<project>/scenes/` out of `templates` (the
/// checkout's `crates/ludion-cli/templates/scenes/`), returning the `scenes/<file>` paths actually
/// written. Never overwrites: a destination that already exists is skipped, not replaced. Refuses a
/// directory that is not a project (`read_project`) before writing anything.
pub fn add_stage_scenes_from(templates: &Path, project: &Path) -> Result<Vec<String>, String> {
    read_project(project)?;
    let scenes_dir = project.join("scenes");
    std::fs::create_dir_all(&scenes_dir)
        .map_err(|e| format!("could not create `{}`: {e}", scenes_dir.display()))?;

    let mut written = Vec::new();
    for file in STAGE_SCENES {
        let dest = scenes_dir.join(file);
        if dest.exists() {
            continue;
        }
        let template_path = templates.join(file);
        let body = std::fs::read_to_string(&template_path)
            .map_err(|e| format!("could not read template `{}`: {e}", template_path.display()))?;
        match create_new_atomic(&dest, body.as_bytes()) {
            Ok(()) => written.push(format!("scenes/{file}")),
            // A concurrent writer won the race to create the file — treat it like the plain
            // already-exists skip above, not a failure.
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(e) => return Err(format!("could not write `{}`: {e}", dest.display())),
        }
    }
    Ok(written)
}

/// The temp file a write to `dest` lands on first: `.{name}.{pid}.{n}.tmp` beside it, so two
/// concurrent writers never share (or delete) each other's temp.
fn temp_sibling(dest: &Path) -> std::path::PathBuf {
    static NEXT_TEMP: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let name = dest.file_name().unwrap_or_default().to_string_lossy();
    let n = NEXT_TEMP.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    dest.with_file_name(format!(".{name}.{}.{n}.tmp", std::process::id()))
}

/// Publishes `body` at `dest` only if `dest` is absent: a unique temp sibling is written whole, then
/// hard-linked into place — atomic and no-clobber, so `dest` never exists half-written.
fn create_new_atomic(dest: &Path, body: &[u8]) -> std::io::Result<()> {
    let tmp = temp_sibling(dest);
    if let Err(e) = std::fs::write(&tmp, body) {
        let _ = std::fs::remove_file(&tmp);
        return Err(e);
    }
    let linked = match std::fs::hard_link(&tmp, dest) {
        Err(e) if e.kind() == std::io::ErrorKind::Unsupported && !dest.exists() => {
            std::fs::rename(&tmp, dest)
        }
        other => other,
    };
    let _ = std::fs::remove_file(&tmp);
    linked
}

#[tauri::command]
pub fn add_stage_scenes(project: String) -> Result<Vec<String>, String> {
    add_stage_scenes_from(
        &crate::pty::repo_root()?.join("crates/ludion-cli/templates/scenes"),
        Path::new(&project),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[test]
    fn temp_names_for_one_dest_differ() {
        let dest = Path::new("d/a.yaml");
        assert_ne!(temp_sibling(dest), temp_sibling(dest));
    }

    fn temp_project_dir() -> std::path::PathBuf {
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let n = NEXT.fetch_add(1, Ordering::Relaxed);
        let dir =
            std::env::temp_dir().join(format!("ludion-project-test-{}-{n}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn a_valid_manifest_with_an_ios_table_reads_name_and_default_scene() {
        let dir = temp_project_dir();
        std::fs::write(
            dir.join("ludion.toml"),
            "name = \"vigil\"\ndefault_scene = \"scenes/camp.yaml\"\n[ios]\nbundle_id = \"dev.ludion.vigil\"\n",
        )
        .unwrap();
        let info = read_project(&dir).unwrap();
        assert_eq!(info.name, "vigil");
        assert_eq!(info.default_scene, "scenes/camp.yaml");
        assert_eq!(info.dir, dir.display().to_string());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_missing_manifest_names_the_dir_and_says_no_ludion_toml() {
        let dir = temp_project_dir();
        let err = read_project(&dir).unwrap_err();
        assert!(err.starts_with(&format!("`{}`", dir.display())), "{err}");
        assert!(err.contains("has no ludion.toml"), "{err}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_malformed_manifest_is_not_a_valid_manifest() {
        let dir = temp_project_dir();
        std::fs::write(dir.join("ludion.toml"), "not valid toml {{{").unwrap();
        let err = read_project(&dir).unwrap_err();
        assert!(err.contains("is not a valid manifest"), "{err}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_manifest_missing_name_is_not_a_valid_manifest() {
        let dir = temp_project_dir();
        std::fs::write(
            dir.join("ludion.toml"),
            "default_scene = \"scenes/camp.yaml\"\n",
        )
        .unwrap();
        let err = read_project(&dir).unwrap_err();
        assert!(err.contains("is not a valid manifest"), "{err}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn launch_project_from_reads_both_flag_forms() {
        assert_eq!(
            launch_project_from(&["--project".to_string(), "D:\\proj".to_string()]),
            Some("D:\\proj".to_string())
        );
        assert_eq!(
            launch_project_from(&["--project=D:\\proj".to_string()]),
            Some("D:\\proj".to_string())
        );
    }

    #[test]
    fn launch_project_from_is_none_when_absent_or_the_value_is_missing_or_empty() {
        assert_eq!(launch_project_from(&["exe".to_string()]), None);
        assert_eq!(launch_project_from(&["--project".to_string()]), None);
        assert_eq!(launch_project_from(&["--project=".to_string()]), None);
    }

    /// A fresh temp dir holding a `templates/` subdir (fake stand-ins for the three stage scenes)
    /// and a `project/` subdir to write into — a sibling layout inside one temp dir, so a test never
    /// depends on the checkout's real template tree.
    fn scratch_with_templates() -> (std::path::PathBuf, std::path::PathBuf) {
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let n = NEXT.fetch_add(1, Ordering::Relaxed);
        let base = std::env::temp_dir().join(format!(
            "ludion-stage-scenes-test-{}-{n}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&base);
        let templates = base.join("templates");
        let project = base.join("project");
        std::fs::create_dir_all(&templates).unwrap();
        std::fs::create_dir_all(&project).unwrap();
        for file in STAGE_SCENES {
            std::fs::write(templates.join(file), format!("format: 1\nname: {file}\n")).unwrap();
        }
        (templates, project)
    }

    #[test]
    fn writes_all_three_into_a_fresh_project_in_order() {
        let (templates, project) = scratch_with_templates();
        std::fs::write(
            project.join("ludion.toml"),
            "name = \"p\"\ndefault_scene = \"scenes/main.yaml\"\n",
        )
        .unwrap();
        let written = add_stage_scenes_from(&templates, &project).unwrap();
        assert_eq!(
            written,
            vec![
                "scenes/model-stage.yaml".to_string(),
                "scenes/model-stage-night.yaml".to_string(),
                "scenes/effects-stage.yaml".to_string(),
            ]
        );
        for file in STAGE_SCENES {
            assert!(project.join("scenes").join(file).is_file());
        }
        let _ = std::fs::remove_dir_all(templates.parent().unwrap());
    }

    #[test]
    fn never_overwrites_an_existing_stage_scene() {
        let (templates, project) = scratch_with_templates();
        std::fs::write(
            project.join("ludion.toml"),
            "name = \"p\"\ndefault_scene = \"scenes/main.yaml\"\n",
        )
        .unwrap();
        std::fs::create_dir_all(project.join("scenes")).unwrap();
        std::fs::write(
            project.join("scenes").join("model-stage.yaml"),
            "SENTINEL\n",
        )
        .unwrap();
        let written = add_stage_scenes_from(&templates, &project).unwrap();
        assert!(!written.contains(&"scenes/model-stage.yaml".to_string()));
        assert_eq!(written.len(), 2);
        assert_eq!(
            std::fs::read_to_string(project.join("scenes").join("model-stage.yaml")).unwrap(),
            "SENTINEL\n"
        );
        let _ = std::fs::remove_dir_all(templates.parent().unwrap());
    }

    #[test]
    fn a_failed_write_leaves_no_file_at_the_destination() {
        let (templates, project) = scratch_with_templates();
        // A regular file where the destination's parent should be makes the temp write fail.
        let blocker = project.join("blocker");
        std::fs::write(&blocker, "a file, not a directory").unwrap();
        let dest = blocker.join("model-stage.yaml");
        assert!(create_new_atomic(&dest, b"body").is_err());
        assert!(!dest.exists());
        assert_eq!(
            std::fs::read_dir(&project).unwrap().count(),
            1,
            "no temp left"
        );
        let _ = std::fs::remove_dir_all(templates.parent().unwrap());
    }

    #[test]
    fn refuses_a_non_project_and_creates_nothing() {
        let (templates, project) = scratch_with_templates();
        let err = add_stage_scenes_from(&templates, &project).unwrap_err();
        assert!(err.contains("has no ludion.toml"), "{err}");
        assert!(!project.join("scenes").exists());
        let _ = std::fs::remove_dir_all(templates.parent().unwrap());
    }

    #[test]
    fn the_real_template_dir_holds_all_three_scenes_with_no_placeholders() {
        let templates = crate::pty::repo_root()
            .unwrap()
            .join("crates/ludion-cli/templates/scenes");
        for file in STAGE_SCENES {
            let body = std::fs::read_to_string(templates.join(file))
                .unwrap_or_else(|e| panic!("missing template `{file}`: {e}"));
            assert!(
                !body.contains("{{"),
                "`{file}` still has a `{{{{` placeholder"
            );
        }
    }
}
