//! Resolving a `kind: generation` recipe against the filesystem.
//!
//! [`ludion_scene`] owns the recipe's text and reads no file: `workflow` and every `images` entry
//! are checked for shape there, never for existence. This module is the caller that resolution is
//! the job of — every path is taken relative to the recipe document's own directory, the rule a
//! sculpt's `shape: voxel` part already follows, so a recipe folder moves as a unit.

use std::path::{Path, PathBuf};

use ludion_core::codes::code;
use ludion_core::diagnostics::Diagnostic;
use ludion_scene::GenerationDocument;

use crate::workflow::{Graph, pointer_token};

/// The folder a project's generation recipes, workflows and run scratch live under. A recipe's
/// project root is the directory holding this one.
pub const GENERATION_DIR: &str = "generation";

/// A recipe read from disk with every path it names resolved.
#[derive(Clone, Debug)]
pub struct Resolved {
    /// The parsed, validated recipe.
    pub doc: GenerationDocument,
    /// The recipe file itself.
    pub recipe_path: PathBuf,
    /// A 16-hex content hash of the recipe file's bytes.
    pub recipe_hash: String,
    /// The workflow file the recipe names, resolved against the recipe's directory.
    pub workflow_path: PathBuf,
    /// A 16-hex content hash of the workflow file's bytes — what makes a later edit to it
    /// detectable from an accepted asset's provenance.
    pub workflow_hash: String,
    /// The parsed API-format graph, before any override is applied.
    pub graph: Graph,
    /// Every `images:` entry, `(name, resolved path)`, sorted by name as the document holds them.
    pub images: Vec<(String, PathBuf)>,
}

impl Resolved {
    /// The recipe's name: its `name:` key, or the file stem with the `.gen` half of `.gen.yaml`
    /// dropped, so `idol-portrait.gen.yaml` names its run folder `idol-portrait`. This is what the
    /// run folder is called.
    pub fn name(&self) -> String {
        self.doc.name.clone().unwrap_or_else(|| {
            self.recipe_path
                .file_stem()
                .map(|stem| {
                    let stem = stem.to_string_lossy();
                    stem.strip_suffix(".gen").unwrap_or(&stem).to_string()
                })
                .unwrap_or_else(|| "recipe".to_string())
        })
    }

    /// The project root: the directory holding the `generation/` folder the recipe sits under
    /// (resolved against the working directory when the spelling has none), or the recipe's own
    /// directory when it sits outside one.
    pub fn project_root(&self) -> PathBuf {
        project_root(&self.recipe_path)
    }
}

/// Reads and resolves a recipe.
///
/// # Errors
/// Everything [`ludion_scene::check_generation`] reports (`L1161` structure, `L1162` data), plus
/// [`code::GENERATION_DATA`] at `/workflow` or `/images/<name>` for a path that does not resolve,
/// and [`code::WORKFLOW_NOT_API`] when the workflow file is not an API-format graph.
pub fn resolve(recipe_path: &Path) -> Result<Resolved, Vec<Diagnostic>> {
    let source = read_to_string(recipe_path).map_err(|diagnostic| vec![diagnostic])?;
    let doc = ludion_scene::check_generation(&source)?;

    let directory = recipe_path.parent().unwrap_or_else(|| Path::new("."));
    let workflow_path = normalize(&directory.join(&doc.workflow));
    if relative_to(&project_root(recipe_path), &workflow_path).is_none() {
        return Err(vec![workflow_outside_root(&doc.workflow)]);
    }
    let workflow_source = read_to_string(&workflow_path).map_err(|diagnostic| {
        vec![diagnostic.with_path("/workflow").with_help(format!(
            "`workflow` is resolved against this document's directory (`{}`); commit the \
                     API-format graph there",
            directory.display()
        ))]
    })?;

    let mut diagnostics = Vec::new();
    let mut images = Vec::new();
    for (name, relative) in &doc.images {
        let path = normalize(&directory.join(relative));
        if path.is_file() {
            images.push((name.clone(), path));
        } else {
            diagnostics.push(
                Diagnostic::new(
                    code::GENERATION_DATA,
                    format!(
                        "the image `{name}` names `{relative}`, and no file is there (resolved to \
                         `{}`)",
                        path.display()
                    ),
                )
                .with_path(format!("/images/{}", pointer_token(name)))
                .with_help(
                    "an `images:` path is resolved against this document's directory, and the file \
                     must be committed — a scratch candidate goes through `gen run --image <path>` \
                     instead",
                ),
            );
        }
    }
    if !diagnostics.is_empty() {
        return Err(diagnostics);
    }

    let graph = Graph::parse(&workflow_source, &workflow_path.display().to_string())
        .map_err(|diagnostic| vec![diagnostic])?;

    Ok(Resolved {
        recipe_hash: content_hash(source.as_bytes()),
        workflow_hash: content_hash(workflow_source.as_bytes()),
        doc,
        recipe_path: recipe_path.to_path_buf(),
        workflow_path,
        graph,
        images,
    })
}

/// The directory holding the `generation/` folder `path` sits under, or `path`'s own directory when
/// it sits outside one. Every path `run.json` records is relative to this. A cwd-relative spelling
/// with no `generation/` in it (`idol.gen.yaml` run from inside `generation/recipes`) is resolved
/// against the working directory, so the root is then absolute.
pub fn project_root(path: &Path) -> PathBuf {
    generation_parent(path)
        .or_else(|| generation_parent(&absolute(path)))
        .unwrap_or_else(|| path.parent().unwrap_or(Path::new(".")).to_path_buf())
}

fn generation_parent(path: &Path) -> Option<PathBuf> {
    let mut current = path.parent();
    while let Some(directory) = current {
        if directory
            .file_name()
            .is_some_and(|name| name == GENERATION_DIR)
        {
            return Some(directory.parent().unwrap_or(directory).to_path_buf());
        }
        current = directory.parent();
    }
    None
}

/// `path` made absolute against the working directory and folded lexically; `""` reads as `.`.
/// Never `canonicalize`. The path as given when the working directory is unreadable.
fn absolute(path: &Path) -> PathBuf {
    let path = if path.as_os_str().is_empty() {
        Path::new(".")
    } else {
        path
    };
    std::path::absolute(path).map_or_else(|_| path.to_path_buf(), |p| normalize(&p))
}

/// Folds `.` and `..` out of a path lexically, so a resolved path reads as one a human would write
/// — `generation/recipes/../workflows/x.json` becomes `generation/workflows/x.json`. Lexical rather
/// than `canonicalize`, which would make every recorded path absolute and machine-specific. Not
/// `ludion_core::paths::normalize`: this folds the OS components of an absolute path, keeping its
/// Windows prefix, root and native separators, which that text normalizer would respell.
pub fn normalize(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for component in path.components() {
        match component {
            std::path::Component::CurDir => {}
            std::path::Component::ParentDir => {
                // Only a real directory name may be popped; a leading `..` has nothing above it.
                let pops = out
                    .file_name()
                    .is_some_and(|name| name != std::ffi::OsStr::new(".."));
                if pops {
                    out.pop();
                } else {
                    out.push("..");
                }
            }
            other => out.push(other.as_os_str()),
        }
    }
    out
}

/// `path` relative to `root` in forward-slash spelling — what `run.json` and a provenance block
/// record, so a report reads the same on both platforms. `None` when `path` is not under `root`, or
/// the remainder would climb out of it (`..`) or restart (a root or drive). Independent of how
/// either path is spelled (`.`, `./x`, relative or absolute).
pub fn relative_to(root: &Path, path: &Path) -> Option<String> {
    let relative = absolute(path)
        .strip_prefix(absolute(root))
        .ok()?
        .to_path_buf();
    let mut parts = Vec::new();
    for component in relative.components() {
        match component {
            std::path::Component::Normal(part) => parts.push(part.to_string_lossy().to_string()),
            std::path::Component::CurDir => {}
            _ => return None,
        }
    }
    Some(parts.join("/"))
}

/// The `L1162` for a `workflow` that resolves outside the project root: runs and accepted assets
/// record it project-relative, which a path above the root has no spelling for.
pub fn workflow_outside_root(workflow: &str) -> Diagnostic {
    Diagnostic::new(
        code::GENERATION_DATA,
        format!("the workflow `{workflow}` resolves outside the project root"),
    )
    .with_path("/workflow")
    .with_help(
        "runs and accepted assets record the workflow project-relative, so it belongs under the \
         project, e.g. `generation/workflows/`",
    )
}

/// A 16-hex content hash of some bytes — the one hash spelling this crate writes, shared with the
/// engine's asset hashing so a workflow file and a texture are hashed the same way.
pub fn content_hash(bytes: &[u8]) -> String {
    format!("{:016x}", ludion_assets::content_hash(bytes))
}

fn read_to_string(path: &Path) -> Result<String, Diagnostic> {
    std::fs::read_to_string(path).map_err(|e| {
        Diagnostic::new(
            code::GENERATION_DATA,
            format!("could not read `{}`: {e}", path.display()),
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn relative_to_is_none_outside_the_root_or_above_it() {
        let root = Path::new("/work/proj");
        assert_eq!(
            relative_to(root, Path::new("/work/proj/generation/a.json")).as_deref(),
            Some("generation/a.json")
        );
        assert_eq!(relative_to(root, Path::new("/elsewhere/a.json")), None);
        assert_eq!(relative_to(root, Path::new("/work/proj/../x")), None);
        assert_eq!(relative_to(Path::new("a"), Path::new("a/../x")), None);
    }

    #[test]
    fn relative_to_is_spelling_independent() {
        for root in [".", ""] {
            assert_eq!(
                relative_to(Path::new(root), Path::new("generation/w.json")).as_deref(),
                Some("generation/w.json")
            );
        }
        assert_eq!(
            relative_to(Path::new("."), Path::new("./generation/w.json")).as_deref(),
            Some("generation/w.json")
        );
    }
}
