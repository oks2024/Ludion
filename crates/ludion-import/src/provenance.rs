use std::path::{Component, Path, Prefix, PrefixComponent};

use ludion_core::diagnostics::{Diagnostic, DiagnosticCode};

/// `file` expressed relative to `dir` with forward slashes — the `generated_from:` provenance the
/// drift gate rediscovers the pair through. In the default layout (the pair written beside its
/// source) this is just the file name.
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
                    "`{}` is on a different drive from the output `{}`: `generated_from` cannot name it relatively",
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
}
