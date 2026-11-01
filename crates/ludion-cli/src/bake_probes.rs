//! Implementation of `ludion bake-probes <scene>`, and the scene discovery and warming `ludion sync`
//! shares with it.
//!
//! The bake itself is `Engine::bake_probes`; this module loads a scene quietly with the built-in
//! registry, bakes at this machine's quality, and prints the report. A game's own components are
//! unknown here (`L1013`), so a game scene is baked by the game binary's `--bake-probes`.

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use ludion_core::diagnostics::Diagnostic;
use ludion_engine::{Engine, ProbeBakeReport, ProbeOutcome, QualitySetting};

use crate::diagnostic;

/// The `.yaml` scenes under `dir` that can hold a probe, sorted. A tree with no `ReflectionProbe`
/// anywhere gives an empty list without parsing a document. When only scenes name it, just those are
/// listed; when a non-scene document (a prefab or any `kind:` doc) names it, a probe can reach any
/// scene indirectly, so every scene is listed. A document outside `dir` is not seen.
pub(crate) fn probe_scenes(dir: &Path) -> Vec<PathBuf> {
    let mut files = Vec::new();
    collect_yaml(dir, &mut files);
    let texts: Vec<(PathBuf, String)> = files
        .into_iter()
        .filter_map(|path| std::fs::read_to_string(&path).ok().map(|text| (path, text)))
        .collect();
    let names_probe = |text: &str| text.contains("ReflectionProbe");
    if !texts.iter().any(|(_, text)| names_probe(text)) {
        return Vec::new();
    }
    let indirect = texts
        .iter()
        .any(|(_, text)| !is_scene(text) && names_probe(text));
    let mut scenes: Vec<PathBuf> = texts
        .into_iter()
        .filter(|(_, text)| is_scene(text) && (indirect || names_probe(text)))
        .map(|(path, _)| path)
        .collect();
    scenes.sort();
    scenes
}

/// A scene has a top-level `entities:` line and no top-level `kind:` line.
fn is_scene(text: &str) -> bool {
    text.lines().any(|line| line.starts_with("entities:"))
        && !text.lines().any(|line| line.starts_with("kind:"))
}

/// Every `.yaml` under `dir`, recursively, skipping dot-directories (`.ludion` holds derived files).
fn collect_yaml(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let hidden = entry.file_name().to_string_lossy().starts_with('.');
        if path.is_dir() {
            if !hidden {
                collect_yaml(&path, out);
            }
        } else if path.extension().is_some_and(|ext| ext == "yaml") {
            out.push(path);
        }
    }
}

/// Loads `scene` into a built-in engine at `quality` without printing anything. An unknown component
/// (`L1013`) gains help naming the game's own bake mode.
pub(crate) fn load_quiet(scene: &Path, quality: &QualitySetting) -> Result<Engine, Diagnostic> {
    let mut engine = Engine::builder()
        .patterns()
        .load_scene_file(scene)
        .map_err(|diagnostic| {
            if diagnostic.code == ludion_core::codes::code::SCENE_UNKNOWN_COMPONENT {
                diagnostic.with_help(
                    "this scene uses a game's own components: bake it with the game's \
                     `--bake-probes` instead",
                )
            } else {
                diagnostic
            }
        })?;
    engine.set_quality(quality.clone());
    Ok(engine)
}

/// Runs `ludion bake-probes`. Exit 1 on a load error, an unknown `--probe` (`L1280`) or no GPU.
pub fn run(scene: &Path, probe: Option<&str>, quality: &QualitySetting, json: bool) -> ExitCode {
    let baked = load_quiet(scene, quality).and_then(|mut engine| engine.bake_probes(probe));
    match baked {
        Ok(report) => {
            if json {
                println!("{}", report.to_json(scene, true, &[]));
            } else if report.probes.is_empty() {
                println!("no reflection probes in {}", scene.display());
            } else {
                for line in report.lines() {
                    println!("{line}");
                }
            }
            ExitCode::SUCCESS
        }
        Err(error) => {
            diagnostic::print(&error);
            if json {
                let empty = ProbeBakeReport::default();
                println!("{}", empty.to_json(scene, false, &[error]));
            }
            ExitCode::FAILURE
        }
    }
}

/// What `ludion sync` warmed: counts over the scenes the CLI could load.
#[derive(Debug, Default)]
pub(crate) struct Warmed {
    pub captured: usize,
    pub stored: usize,
    pub scenes: usize,
    /// Scenes the CLI cannot load (a game's components), as project-relative paths.
    pub skipped: Vec<String>,
}

/// Warms the probe store for the project's default-scene directory. Costs nothing without probes:
/// no GPU is touched unless a scene holds a `ReflectionProbe`. Stops quietly at no adapter, and says
/// so once when the lane forbids a GPU (`quiet` silences the notes for `--json`).
pub(crate) fn warm(project: &Path, scene_dir: &Path, quiet: bool) -> Warmed {
    let mut warmed = Warmed::default();
    let scenes = probe_scenes(&project.join(scene_dir));
    if scenes.is_empty() {
        return warmed;
    }
    if ludion_core::require_gpu::gpu_forbidden() {
        if !quiet {
            println!("probes: skipped (no GPU on this lane)");
        }
        return warmed;
    }
    let quality = match ludion_engine::resolve_startup_quality(None, None) {
        Ok(quality) => quality,
        Err(invalid) => {
            diagnostic::print(&invalid);
            return warmed;
        }
    };
    for scene in scenes {
        let rel = scene.strip_prefix(project).unwrap_or(&scene);
        let rel = rel.to_string_lossy().replace('\\', "/");
        let mut engine = match load_quiet(&scene, &quality) {
            Ok(engine) => engine,
            Err(_) => {
                warmed.skipped.push(rel);
                continue;
            }
        };
        match engine.bake_probes(None) {
            Ok(report) => {
                warmed.scenes += 1;
                for probe in &report.probes {
                    match probe.outcome {
                        ProbeOutcome::Captured => warmed.captured += 1,
                        ProbeOutcome::Loaded => warmed.stored += 1,
                    }
                }
            }
            Err(error) => {
                if !ludion_core::require_gpu::is_no_adapter(std::slice::from_ref(&error)) {
                    diagnostic::print(&error);
                }
                if !quiet {
                    println!("probes: warming stopped at {rel} ({})", error.code);
                }
                break;
            }
        }
    }
    warmed
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tree(files: &[(&str, &str)]) -> tempfile_dir::Dir {
        let dir = tempfile_dir::Dir::new();
        for (name, text) in files {
            let path = dir.path().join(name);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, text).unwrap();
        }
        dir
    }

    /// A scratch directory removed on drop; the CLI has no `tempfile` dependency.
    mod tempfile_dir {
        use std::path::{Path, PathBuf};
        use std::sync::atomic::{AtomicU32, Ordering};

        pub struct Dir(PathBuf);

        impl Dir {
            pub fn new() -> Self {
                static NEXT: AtomicU32 = AtomicU32::new(0);
                let path = std::env::temp_dir().join(format!(
                    "ludion-bake-probes-{}-{}",
                    std::process::id(),
                    NEXT.fetch_add(1, Ordering::Relaxed)
                ));
                std::fs::create_dir_all(&path).unwrap();
                Dir(path)
            }

            pub fn path(&self) -> &Path {
                &self.0
            }
        }

        impl Drop for Dir {
            fn drop(&mut self) {
                std::fs::remove_dir_all(&self.0).ok();
            }
        }
    }

    const SCENE: &str = "format: 1\nentities: []\n";
    const PROBE_SCENE: &str = "format: 1\nentities:\n  - components:\n      ReflectionProbe: {}\n";

    #[test]
    fn a_tree_without_a_probe_lists_nothing() {
        let dir = tree(&[("a.yaml", SCENE), ("sub/b.yaml", SCENE)]);
        assert!(probe_scenes(dir.path()).is_empty());
    }

    #[test]
    fn documents_with_a_kind_are_excluded_and_the_rest_sorted() {
        let prefab =
            "format: 1\nkind: prefab\nentities:\n  - components:\n      ReflectionProbe: {}\n";
        let dir = tree(&[
            ("z.yaml", PROBE_SCENE),
            ("a/b.yaml", SCENE),
            ("prefab.yaml", prefab),
            (".ludion/hidden.yaml", PROBE_SCENE),
            ("notes.txt", PROBE_SCENE),
        ]);
        let found = probe_scenes(dir.path());
        let names: Vec<_> = found
            .iter()
            .map(|p| {
                p.strip_prefix(dir.path())
                    .unwrap()
                    .to_string_lossy()
                    .replace('\\', "/")
            })
            .collect();
        assert_eq!(names, ["a/b.yaml", "z.yaml"]);
    }

    fn relative(dir: &tempfile_dir::Dir) -> Vec<String> {
        probe_scenes(dir.path())
            .iter()
            .map(|p| {
                p.strip_prefix(dir.path())
                    .unwrap()
                    .to_string_lossy()
                    .replace('\\', "/")
            })
            .collect()
    }

    #[test]
    fn only_scenes_naming_a_probe_are_listed() {
        let dir = tree(&[
            ("a.yaml", PROBE_SCENE),
            ("b.yaml", SCENE),
            ("c/d.yaml", PROBE_SCENE),
        ]);
        assert_eq!(relative(&dir), ["a.yaml", "c/d.yaml"]);
    }

    #[test]
    fn a_probe_in_a_prefab_lists_every_scene() {
        let prefab =
            "format: 1\nkind: prefab\nentities:\n  - components:\n      ReflectionProbe: {}\n";
        let dir = tree(&[("p.yaml", prefab), ("a.yaml", SCENE), ("b.yaml", SCENE)]);
        assert_eq!(relative(&dir), ["a.yaml", "b.yaml"]);
    }

    #[test]
    fn a_non_yaml_mention_does_not_widen_the_list() {
        let dir = tree(&[("notes.txt", PROBE_SCENE), ("a.yaml", SCENE)]);
        assert!(relative(&dir).is_empty());
    }
}
