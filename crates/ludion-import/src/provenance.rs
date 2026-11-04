use std::path::{Component, Path, PathBuf, Prefix, PrefixComponent};

use ludion_core::diagnostics::{Diagnostic, DiagnosticCode};

/// `file` expressed relative to `dir` with forward slashes — the recorded source path (a
/// `generated_from:` the drift gate rediscovers the pair through, a sidecar's `recipe:`). In the
/// default layout (the pair written beside its source) this is just the file name. `.` and `..`
/// resolve lexically first, so the result does not depend on how the platform keeps them.
///
/// # Errors
/// `code` when `dir` and `file` are on different drives: no relative path exists, and an absolute
/// one would not be portable.
pub fn relative_to(dir: &Path, file: &Path, code: DiagnosticCode) -> Result<String, Diagnostic> {
    let file_name = || {
        file.file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default()
    };
    let (Ok(dir), Ok(file)) = (std::path::absolute(dir), std::path::absolute(file)) else {
        return Ok(file_name());
    };
    let (dir, file) = (normalized(&dir), normalized(&file));
    let mut left = dir.components().peekable();
    let mut right = file.components().peekable();
    if let (Some(Component::Prefix(left_prefix)), Some(Component::Prefix(right_prefix))) =
        (left.peek().copied(), right.peek().copied())
    {
        let drive = |prefix: PrefixComponent| match prefix.kind() {
            Prefix::Disk(letter) | Prefix::VerbatimDisk(letter) => {
                Some(letter.to_ascii_uppercase())
            }
            _ => None,
        };
        let same = match (drive(left_prefix), drive(right_prefix)) {
            (Some(a), Some(b)) => a == b,
            _ => left_prefix == right_prefix,
        };
        if !same {
            return Err(Diagnostic::new(
                code,
                format!(
                    "`{}` is on a different drive from the output `{}`: no relative path can name it",
                    file.display(),
                    dir.display()
                ),
            )
            .with_help(format!(
                "write the output on the source's drive (`{}`), e.g. `--out` inside the project",
                right_prefix.as_os_str().to_string_lossy()
            )));
        }
        // `\\?\C:` and `C:` name one drive but differ as components: step past both.
        left.next();
        right.next();
    }
    while left.peek().is_some() && left.peek() == right.peek() {
        left.next();
        right.next();
    }
    let mut parts: Vec<String> = vec!["..".to_string(); left.count()];
    parts.extend(right.map(|part| part.as_os_str().to_string_lossy().into_owned()));
    if parts.is_empty() {
        return Ok(file_name());
    }
    Ok(parts.join("/"))
}

/// `path` with `.` and `..` resolved lexically; a `..` never pops a prefix or the root.
fn normalized(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                if matches!(out.components().next_back(), Some(Component::Normal(_))) {
                    out.pop();
                }
            }
            other => out.push(other.as_os_str()),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use ludion_core::codes::code;

    #[test]
    fn a_source_on_another_drive_has_no_relative_path() {
        if !Path::new(r"C:\a").is_absolute() {
            return;
        }
        let err = relative_to(
            Path::new(r"Q:\out"),
            Path::new(r"C:\src\x.gltf"),
            code::OUTPUT_IO,
        )
        .unwrap_err();
        assert_eq!(err.code, code::OUTPUT_IO);
        assert!(err.message.contains("different drive"), "{}", err.message);
        assert_eq!(
            relative_to(
                Path::new(r"c:\out"),
                Path::new(r"C:\src\x.gltf"),
                code::OUTPUT_IO
            )
            .unwrap(),
            "../src/x.gltf"
        );
        assert_eq!(
            relative_to(
                Path::new(r"\\?\C:\out"),
                Path::new(r"C:\src\x.gltf"),
                code::OUTPUT_IO
            )
            .unwrap(),
            "../src/x.gltf"
        );
        assert!(
            relative_to(
                Path::new(r"\\?\Q:\out"),
                Path::new(r"C:\src\x.gltf"),
                code::OUTPUT_IO
            )
            .is_err()
        );
    }

    #[test]
    fn dot_dot_components_resolve_before_the_climb_is_counted() {
        if !Path::new("/a").is_absolute() {
            return;
        }
        let rel = |dir: &str, file: &str| {
            relative_to(Path::new(dir), Path::new(file), code::OUTPUT_IO).unwrap()
        };
        assert_eq!(rel("/a/b/../c", "/a/c/x"), "x");
        assert_eq!(rel("/a/tex/out", "/a/b/../tex/x.yaml"), "../x.yaml");
    }

    #[test]
    fn relative_paths_climb_out_of_the_out_dir() {
        if !Path::new("/a").is_absolute() {
            return;
        }
        let rel = |dir: &str, file: &str| {
            relative_to(Path::new(dir), Path::new(file), code::OUTPUT_IO).unwrap()
        };
        assert_eq!(
            rel("/a/b/stem", "/a/b/stem.texture.yaml"),
            "../stem.texture.yaml"
        );
        assert_eq!(rel("/a/out", "/a/b/x.texture.yaml"), "../b/x.texture.yaml");
    }
}
