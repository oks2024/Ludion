//! Lexical path confinement and spelling: does a client-supplied path stay below the directory it is
//! joined to, and what is the one spelling of a path (`normalize`, `sibling`)?
//!
//! Pure string logic, so the answer is the same on every platform. Both separators are checked
//! everywhere: text written on one machine is read on the other. Symlinks are not chased.
//!
//! # Example
//!
//! ```
//! use ludion_core::paths::{relative_refusal, PathRefusal};
//!
//! assert_eq!(relative_refusal("levels/arena.yaml"), None);
//! assert_eq!(relative_refusal("../arena.yaml"), Some(PathRefusal::ParentDir));
//! assert_eq!(relative_refusal("C:/arena.yaml"), Some(PathRefusal::Prefixed));
//! assert_eq!(relative_refusal("lit/nul.png"), Some(PathRefusal::Reserved));
//! ```

/// Why a path is not a plain relative path below its base.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PathRefusal {
    /// The path is `""`.
    Empty,
    /// The path starts with `/` or `\`, which covers `//host/share` and `\\?\`.
    Absolute,
    /// The path contains `:`, which covers drive letters (`C:`), URL schemes (`file:`, `http:`)
    /// and NTFS alternate streams (`name:stream`).
    Prefixed,
    /// A segment is `..`, or made only of dots and spaces (`...`, `.. `), which Win32 strips down
    /// to `..`.
    ParentDir,
    /// The path contains a NUL byte.
    Nul,
    /// A stem contains `/` or `\`.
    Separator,
    /// A segment is a Windows reserved device name (`CON`, `NUL`, `COM1`, …), with or without an
    /// extension.
    Reserved,
}

impl PathRefusal {
    /// The reason as a phrase that finishes "the path …".
    pub fn describe(self) -> &'static str {
        match self {
            Self::Empty => "is empty",
            Self::Absolute => "is absolute",
            Self::Prefixed => "carries a drive, scheme or stream prefix (`:`)",
            Self::ParentDir => "climbs out of its base directory with `..`",
            Self::Nul => "contains a NUL byte",
            Self::Separator => "contains a path separator; a stem names a file in its directory",
            Self::Reserved => {
                "names a Windows reserved device (`CON`, `NUL`, `COM1`, …), which no file can be called on Windows"
            }
        }
    }
}

/// `None` when `text` is a relative path that stays below its base; otherwise the reason it does
/// not. Empty segments (`a//b`) and a lone `.` segment are fine.
pub fn relative_refusal(text: &str) -> Option<PathRefusal> {
    if text.is_empty() {
        return Some(PathRefusal::Empty);
    }
    if text.contains('\0') {
        return Some(PathRefusal::Nul);
    }
    if text.starts_with(['/', '\\']) {
        return Some(PathRefusal::Absolute);
    }
    if text.contains(':') {
        return Some(PathRefusal::Prefixed);
    }
    let climbs = text.split(['/', '\\']).any(|segment| {
        segment != "." && segment.contains('.') && segment.chars().all(|c| c == '.' || c == ' ')
    });
    if climbs {
        return Some(PathRefusal::ParentDir);
    }
    text.split(['/', '\\'])
        .any(is_reserved_device)
        .then_some(PathRefusal::Reserved)
}

/// `None` when `text`, after [`normalize`]'s lexical `..` folding, is a plain relative path below
/// its base: the one rule for a path that came from content or a payload and is joined to the
/// content root. A `..` that cancels a segment is fine; one that escapes the base is refused.
///
/// ```
/// use ludion_core::paths::{content_refusal, PathRefusal};
///
/// assert_eq!(content_refusal("meshes/../t.png"), None);
/// assert_eq!(content_refusal("a/../../x"), Some(PathRefusal::ParentDir));
/// assert_eq!(content_refusal("C:x"), Some(PathRefusal::Prefixed));
/// assert_eq!(content_refusal("/dev/zero"), Some(PathRefusal::Absolute));
/// ```
pub fn content_refusal(text: &str) -> Option<PathRefusal> {
    relative_refusal(&normalize(text))
}

/// Whether one path segment names a Windows device (`con`, `NUL .png`, `com1.tar.gz`, `Aux.`),
/// which Win32 resolves to the device whatever the extension.
pub fn is_reserved_device(segment: &str) -> bool {
    let trimmed = segment.trim_end_matches(['.', ' ']);
    let base = trimmed
        .split('.')
        .next()
        .unwrap_or("")
        .trim_end_matches(' ');
    let upper = base.to_ascii_uppercase();
    if matches!(
        upper.as_str(),
        "CON" | "PRN" | "AUX" | "NUL" | "CONIN$" | "CONOUT$"
    ) {
        return true;
    }
    let mut chars = upper.chars();
    let prefix: String = chars.by_ref().take(3).collect();
    matches!(prefix.as_str(), "COM" | "LPT")
        && matches!(chars.next(), Some('0'..='9' | '¹' | '²' | '³'))
        && chars.next().is_none()
}

/// `stem` unchanged unless it names a Windows device ([`is_reserved_device`]), else with a `_`
/// before its first `.` (or at the end when it has none), which no longer resolves to the device.
///
/// ```
/// use ludion_core::paths::unreserved_stem;
///
/// assert_eq!(unreserved_stem("con"), "con_");
/// assert_eq!(unreserved_stem("NUL.tar"), "NUL_.tar");
/// assert_eq!(unreserved_stem("console"), "console");
/// ```
pub fn unreserved_stem(stem: &str) -> String {
    if !is_reserved_device(stem) {
        return stem.to_string();
    }
    match stem.find('.') {
        Some(dot) => format!("{}_{}", &stem[..dot], &stem[dot..]),
        None => format!("{stem}_"),
    }
}

/// Like [`relative_refusal`], for a text that is one file stem: also refuses a separator and a lone
/// `.`.
pub fn stem_refusal(text: &str) -> Option<PathRefusal> {
    if let Some(refusal) = relative_refusal(text) {
        return Some(refusal);
    }
    if text.contains(['/', '\\']) {
        return Some(PathRefusal::Separator);
    }
    (text == ".").then_some(PathRefusal::ParentDir)
}

/// `path` as text with forward slashes, so a diagnostic reads the same on every platform.
///
/// ```
/// use ludion_core::paths::slash;
///
/// assert_eq!(slash(std::path::Path::new("a\\b/c.yaml")), "a/b/c.yaml");
/// ```
pub fn slash(path: &std::path::Path) -> String {
    path.display().to_string().replace('\\', "/")
}

/// `text` in one lexical spelling: both separators become `/`, empty and `.` segments vanish and a
/// `..` cancels the plain segment before it. A leading `..` is kept; a leading separator stays as
/// `/` and a `..` directly under it is dropped. A drive segment such as `C:` is never popped.
/// Text that collapses to nothing is `.`; `""` stays `""`.
///
/// ```
/// use ludion_core::paths::normalize;
///
/// assert_eq!(normalize("meshes\\../textures/x.png"), "textures/x.png");
/// assert_eq!(normalize("a/../../x"), "../x");
/// assert_eq!(normalize("a/.."), ".");
/// ```
pub fn normalize(text: &str) -> String {
    let body = text.strip_prefix('/').unwrap_or(text);
    if !text.is_empty()
        && !text.contains('\\')
        && !body
            .split('/')
            .any(|s| s.is_empty() || s == "." || s == "..")
    {
        return text.to_owned();
    }
    let rooted = text.starts_with(['/', '\\']);
    let mut segments: Vec<&str> = Vec::new();
    for segment in text.split(['/', '\\']) {
        match segment {
            "" | "." => {}
            ".." => match segments.last() {
                Some(&last) if last != ".." && !(segments.len() == 1 && last.ends_with(':')) => {
                    segments.pop();
                }
                _ if rooted => {}
                _ => segments.push(".."),
            },
            plain => segments.push(plain),
        }
    }
    match (rooted, segments.is_empty()) {
        (true, _) => format!("/{}", segments.join("/")),
        (false, true) if !text.is_empty() => ".".to_owned(),
        _ => segments.join("/"),
    }
}

/// `file` resolved against the directory of the document `doc` (the text up to its last separator),
/// in [`normalize`]d spelling.
///
/// ```
/// use ludion_core::paths::sibling;
///
/// assert_eq!(sibling("meshes/m.mesh.yaml", "../textures/x.png"), "textures/x.png");
/// assert_eq!(sibling("hero.character.yaml", "clips/run.anim.yaml"), "clips/run.anim.yaml");
/// ```
pub fn sibling(doc: &str, file: &str) -> String {
    let dir = doc.rfind(['/', '\\']).map_or("", |at| &doc[..=at]);
    normalize(&format!("{dir}{file}"))
}

/// The inverse of [`sibling`]: the spelling of `target` that `doc` resolves back to it, so
/// `sibling(doc, &relative_to(doc, target)?) == normalize(target)`. `None` when `doc`'s directory
/// holds a `..` that cannot be inverted lexically, or `target` is rooted.
///
/// ```
/// use ludion_core::paths::relative_to;
///
/// assert_eq!(
///     relative_to("level/render/model.prefab.yaml", "level/render/meshes/a.mesh.yaml").as_deref(),
///     Some("meshes/a.mesh.yaml")
/// );
/// assert_eq!(relative_to("a/p.yaml", "b/x").as_deref(), Some("../b/x"));
/// ```
pub fn relative_to(doc: &str, target: &str) -> Option<String> {
    let doc = normalize(doc);
    let target = normalize(target);
    if target.starts_with('/') || doc.starts_with('/') {
        return None;
    }
    let doc_segments: Vec<&str> = doc.split('/').collect();
    let doc_dir = &doc_segments[..doc_segments.len() - 1];
    let target_segments: Vec<&str> = target.split('/').collect();
    let common = doc_dir
        .iter()
        .zip(&target_segments[..target_segments.len() - 1])
        .take_while(|(a, b)| a == b)
        .count();
    let rest = &doc_dir[common..];
    if rest.contains(&"..") {
        return None;
    }
    let mut out: Vec<&str> = vec![".."; rest.len()];
    out.extend(&target_segments[common..]);
    Some(out.join("/"))
}

/// Like [`relative_refusal`], for a path a document resolves against its own directory: an exact `..`
/// segment is allowed; every other refusal stands, so `../nul.png` is still refused.
///
/// ```
/// use ludion_core::paths::{sibling_refusal, PathRefusal};
///
/// assert_eq!(sibling_refusal("../textures/x.png"), None);
/// assert_eq!(sibling_refusal("../nul.png"), Some(PathRefusal::Reserved));
/// ```
pub fn sibling_refusal(text: &str) -> Option<PathRefusal> {
    match relative_refusal(text) {
        Some(PathRefusal::ParentDir) => {
            let mut segments = text.split(['/', '\\']);
            if segments.clone().any(|s| {
                s != ".." && s != "." && s.contains('.') && s.chars().all(|c| c == '.' || c == ' ')
            }) {
                Some(PathRefusal::ParentDir)
            } else if segments.any(is_reserved_device) {
                Some(PathRefusal::Reserved)
            } else {
                None
            }
        }
        other => other,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn refuses_each_variant_on_both_separators() {
        let cases = [
            ("a/../b", PathRefusal::ParentDir),
            ("a\\..\\b", PathRefusal::ParentDir),
            ("..\\x", PathRefusal::ParentDir),
            ("../x", PathRefusal::ParentDir),
            (".. /x", PathRefusal::ParentDir),
            ("...", PathRefusal::ParentDir),
            ("a/...", PathRefusal::ParentDir),
            ("C:x", PathRefusal::Prefixed),
            ("C:/x", PathRefusal::Prefixed),
            ("file:///x", PathRefusal::Prefixed),
            ("a:stream", PathRefusal::Prefixed),
            ("//host/s", PathRefusal::Absolute),
            ("/etc/x", PathRefusal::Absolute),
            ("\\\\?\\C:\\x", PathRefusal::Absolute),
            ("\\x", PathRefusal::Absolute),
            ("", PathRefusal::Empty),
            ("a\0b", PathRefusal::Nul),
        ];
        for (text, expected) in cases {
            assert_eq!(relative_refusal(text), Some(expected), "{text:?}");
        }
    }

    #[test]
    fn content_refusal_folds_inside_dots_and_refuses_escapes() {
        let cases = [
            ("meshes/../t.png", None),
            ("a/../b.png", None),
            ("./a.png", None),
            ("../x", Some(PathRefusal::ParentDir)),
            ("a/../../x", Some(PathRefusal::ParentDir)),
            ("C:x", Some(PathRefusal::Prefixed)),
            ("//h/s", Some(PathRefusal::Absolute)),
            ("\\\\?\\x", Some(PathRefusal::Absolute)),
            ("/dev/zero", Some(PathRefusal::Absolute)),
            ("...", Some(PathRefusal::ParentDir)),
            ("nul.png", Some(PathRefusal::Reserved)),
            ("a/..", None),
            ("", Some(PathRefusal::Empty)),
        ];
        for (text, expected) in cases {
            assert_eq!(content_refusal(text), expected, "{text:?}");
        }
    }

    #[test]
    fn sibling_refusal_allows_only_exact_parent_segments() {
        let cases = [
            ("../textures/x.png", None),
            ("a/../b.png", None),
            ("../nul.png", Some(PathRefusal::Reserved)),
            ("a/.../b", Some(PathRefusal::ParentDir)),
            ("a/.. /b", Some(PathRefusal::ParentDir)),
            ("C:/x", Some(PathRefusal::Prefixed)),
            ("/x", Some(PathRefusal::Absolute)),
            ("a\0b", Some(PathRefusal::Nul)),
            ("", Some(PathRefusal::Empty)),
        ];
        for (text, expected) in cases {
            assert_eq!(sibling_refusal(text), expected, "{text:?}");
        }
    }

    #[test]
    fn relative_to_inverts_sibling() {
        let cases = [
            (
                "level/render/model.prefab.yaml",
                "level/render/meshes/a.mesh.yaml",
                Some("meshes/a.mesh.yaml"),
            ),
            (
                "prefabs/town/p.prefab.yaml",
                "../assets/x.mesh.yaml",
                Some("../../../assets/x.mesh.yaml"),
            ),
            ("p.prefab.yaml", "meshes/a.yaml", Some("meshes/a.yaml")),
            ("a/p.yaml", "b/x", Some("../b/x")),
            ("../a/p.yaml", "b/x", None),
            ("a/p.yaml", "/abs/x", None),
        ];
        for (doc, target, expected) in cases {
            assert_eq!(
                relative_to(doc, target).as_deref(),
                expected,
                "{doc} {target}"
            );
            if let Some(relative) = relative_to(doc, target) {
                assert_eq!(sibling(doc, &relative), normalize(target), "{doc} {target}");
            }
        }
    }

    #[test]
    fn refuses_reserved_devices() {
        for text in [
            "nul",
            "CON",
            "lit/nul",
            "a/Aux/b",
            "sub\\com1.png",
            "con.txt",
            "NUL .png",
            "com1.tar.gz",
            "Aux.",
            "LPT²",
        ] {
            assert_eq!(
                relative_refusal(text),
                Some(PathRefusal::Reserved),
                "{text:?}"
            );
            assert!(stem_refusal(text).is_some(), "{text:?}");
        }
    }

    #[test]
    fn reserved_device_predicate() {
        for yes in [
            "con", "PRN", "aux.x", "Nul ", "CONIN$", "conout$", "com0", "lpt9", "COM³",
        ] {
            assert!(is_reserved_device(yes), "{yes:?}");
        }
        for no in [
            "console", "com10", "lpt", "icon", "con-1", "nul_x", "", "com", "xcom1",
        ] {
            assert!(!is_reserved_device(no), "{no:?}");
        }
    }

    #[test]
    fn unreserved_stem_defuses_devices_only() {
        for (stem, expected) in [
            ("con", "con_"),
            ("Con.x", "Con_.x"),
            ("com1", "com1_"),
            ("lpt9.tar.gz", "lpt9_.tar.gz"),
            ("nul.", "nul_."),
            ("console", "console"),
            ("com10", "com10"),
            ("idol", "idol"),
        ] {
            let out = unreserved_stem(stem);
            assert_eq!(out, expected, "{stem:?}");
            assert!(!is_reserved_device(&out), "{stem:?}");
        }
    }

    #[test]
    fn accepts_plain_relative_paths() {
        for text in [
            "a.yaml",
            "./a.yaml",
            "sub/dir/a.yaml",
            "sub\\a.yaml",
            "a..b.yaml",
            ".hidden",
            "a//b.yaml",
            "my scene.yaml",
            "console",
            "com10",
            "lpt",
            "icon.png",
            "con-1",
            "nul_x",
        ] {
            assert_eq!(relative_refusal(text), None, "{text:?}");
        }
    }

    #[test]
    fn every_refusal_describes_itself() {
        for refusal in [
            PathRefusal::Empty,
            PathRefusal::Absolute,
            PathRefusal::Prefixed,
            PathRefusal::ParentDir,
            PathRefusal::Nul,
            PathRefusal::Separator,
            PathRefusal::Reserved,
        ] {
            assert!(!refusal.describe().is_empty());
        }
    }

    #[test]
    fn normalize_collapses_to_one_spelling() {
        for (text, expected) in [
            ("a/b", "a/b"),
            ("a\\b/c", "a/b/c"),
            ("a/./b", "a/b"),
            ("a//b", "a/b"),
            ("meshes\\../textures/x.png", "textures/x.png"),
            ("a/../../x", "../x"),
            ("../a", "../a"),
            ("/a/../b", "/b"),
            ("/../a", "/a"),
            ("a/..", "."),
            ("C:/..", "C:/.."),
            ("", ""),
            (".", "."),
        ] {
            assert_eq!(normalize(text), expected, "{text:?}");
        }
    }

    #[test]
    fn sibling_resolves_against_the_document_directory() {
        for (doc, file, expected) in [
            (
                "characters/hero.character.yaml",
                "../meshes/hero.mesh.yaml",
                "meshes/hero.mesh.yaml",
            ),
            (
                "hero.character.yaml",
                "clips/run.anim.yaml",
                "clips/run.anim.yaml",
            ),
            ("a/hero.character.yaml", "../../x.yaml", "../x.yaml"),
            (
                "models/player.assembly.yaml",
                "parts/arm.part.yaml",
                "models/parts/arm.part.yaml",
            ),
            ("player.assembly.yaml", "arm.part.yaml", "arm.part.yaml"),
            ("meshes\\m.mesh.yaml", "../textures/x.png", "textures/x.png"),
        ] {
            assert_eq!(sibling(doc, file), expected, "{doc:?} {file:?}");
        }
    }

    #[test]
    fn stems_are_plain_file_names() {
        for text in ["inter", "my font", "a..b", ".hidden"] {
            assert_eq!(stem_refusal(text), None, "{text:?}");
        }
        let cases = [
            ("a/b", PathRefusal::Separator),
            ("a\\b", PathRefusal::Separator),
            ("./a", PathRefusal::Separator),
            (".", PathRefusal::ParentDir),
            ("..", PathRefusal::ParentDir),
            ("", PathRefusal::Empty),
            ("C:x", PathRefusal::Prefixed),
        ];
        for (text, expected) in cases {
            assert_eq!(stem_refusal(text), Some(expected), "{text:?}");
        }
    }
}
