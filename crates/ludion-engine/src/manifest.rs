//! The project manifest, `ludion.toml`: a game's name, its default scene, its per-platform
//! identity, its derived-import tables, its `[cook]` table and its `[audio]` voice caps.
//! `ludion export`, `ludion sync` and every game host read it.

use std::path::{Path, PathBuf};

use crate::Quality;
use ludion_assets::cooked::CookTarget;
use ludion_core::audio::VoiceCaps;
use ludion_core::codes::code::{AUDIO_VOICE_CAPS_INVALID, MANIFEST_INVALID};
use ludion_core::diagnostics::Diagnostic;
use ludion_core::paths;
use serde::Deserialize;

/// The parsed `ludion.toml`.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Manifest {
    /// The project's name; the Xcode project and app are named after it.
    pub name: String,
    /// The scene the game hosts by default, relative to the project (`scenes/camp.yaml`); its
    /// directory is the game's content.
    pub default_scene: PathBuf,
    /// The `[ios]` table, needed only for an iOS export.
    pub ios: Option<IosSection>,
    /// The `[[import]]` tables `ludion sync` compiles into derived trees.
    #[serde(default, rename = "import")]
    pub imports: Vec<ImportSection>,
    /// The `[audio]` table, read by [`Manifest::voice_caps`]; validated by `load`.
    #[serde(default)]
    pub audio: Option<toml::Table>,
    /// The `[cook]` table: what `ludion sync` compresses the imported maps into.
    pub cook: Option<CookSection>,
}

/// The `[cook]` table. Every key is optional.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CookSection {
    /// The GPU families to cook for, `desktop` (BC7) and `ios` (ASTC); absent is `["desktop"]`.
    #[serde(default = "default_cook_targets")]
    pub targets: Vec<String>,
    /// Cook the texture maps of the import trees and roots; `false` skips the stage like `ludion sync --no-cook`.
    #[serde(default = "default_true")]
    pub textures: bool,
    /// Binary mesh cooking. Not implemented: `true` is refused.
    #[serde(default)]
    pub meshes: bool,
    /// Project-relative directories whose `*.mesh.yaml` files `ludion sync` cooks the maps of,
    /// beside the `[[import]]` trees. Each is outside every `[[import]] to`, uncapped.
    #[serde(default)]
    pub roots: Vec<PathBuf>,
}

fn default_cook_targets() -> Vec<String> {
    vec!["desktop".to_string()]
}

fn default_true() -> bool {
    true
}

/// One `[[import]]` table: glTF sources and the derived tree they compile into.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ImportSection {
    /// A project-relative glob of `.gltf` files; `*` matches within one path segment. Each match
    /// `<dir>/<stem>.gltf` imports into `<to>/<stem>/`.
    pub from: String,
    /// The derived root, project-relative. Ignored by version control, never hand-edited.
    pub to: PathBuf,
    /// Halve every cooked map of this table until neither side exceeds this many texels; absent
    /// keeps the source size. Part of the cooked map's key.
    pub max_texture_size: Option<u32>,
}

/// The `[ios]` table.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct IosSection {
    /// The app's bundle identifier (`dev.ludion.breakout`).
    pub bundle_id: String,
    /// The render quality preset the app ships with (`medium`); absent is `full`. Whoever launches
    /// the app can still override it with `LUDION_QUALITY`.
    pub quality: Option<String>,
}

/// Reads and parses `<project_dir>/ludion.toml`.
///
/// # Errors
/// `L0023` for a missing or unreadable file, malformed TOML, an unknown or missing key, a `name`
/// outside `A-Z a-z 0-9 _ -`, or a `default_scene` that is not a project-relative path in its one
/// spelling (absolute, escaping, `.`, `..` or empty segments, reserved names) or has no parent
/// directory; `L5004` for an invalid `[audio]` table.
pub fn load(project_dir: &Path) -> Result<Manifest, Diagnostic> {
    let path = project_dir.join("ludion.toml");
    let invalid = |message: String, help: &str| {
        Diagnostic::new(MANIFEST_INVALID, message).with_help(help.to_string())
    };
    let text = std::fs::read_to_string(&path).map_err(|err| {
        invalid(
            format!("could not read `{}`: {err}", path.display()),
            "run from the game's project directory or pass `--project <dir>`; `ludion new` writes a \
             `ludion.toml`",
        )
    })?;
    let manifest: Manifest = toml::from_str(&text).map_err(|err| {
        invalid(
            format!("`{}` is not a valid manifest: {err}", path.display()),
            "the keys are `name`, `default_scene`, the `[ios]` table's `bundle_id` and `quality`, and \
             each `[[import]]` table's `from`, `to` and `max_texture_size`, the `[cook]` table's \
             `targets`, `textures`, `meshes` and `roots`, and the `[audio]` table's `voices_per_sound` and \
             `voices_total`",
        )
    })?;
    let plain_name = !manifest.name.is_empty() && manifest.name.chars().all(is_plain_name_char);
    if !plain_name {
        return Err(invalid(
            format!(
                "`name` = `{}` must be letters, digits, `_` or `-`",
                manifest.name
            ),
            "the name becomes the Xcode project and app name",
        ));
    }
    let scene = manifest.default_scene.to_string_lossy();
    if let Some(reason) = path_refusal(&scene) {
        return Err(invalid(
            format!("`default_scene` = `{scene}` {reason}"),
            "the scene's directory is copied into the app: write it as `scenes/main.yaml`",
        ));
    }
    let has_parent = manifest
        .default_scene
        .parent()
        .is_some_and(|parent| !parent.as_os_str().is_empty());
    if !has_parent {
        return Err(invalid(
            format!(
                "`default_scene` = `{}` has no directory",
                manifest.default_scene.display()
            ),
            "put the scene in a directory (`scenes/main.yaml`): that directory is the game's content",
        ));
    }
    validate_imports(&manifest.imports).map_err(|message| {
        invalid(
            message,
            "an `[[import]]` table is `from = \"art/*/export/*.gltf\"` and `to = \"assets/derived\"`: \
             relative paths inside the project in one spelling (no `.`, `..` or `\\`), one `to` per table, no `to` inside another, and no `to` inside a `from` directory",
        )
    })?;
    validate_cook(&manifest).map_err(|message| {
        invalid(
            message,
            "`[cook]` is `targets = [\"desktop\", \"ios\"]`, `textures = true` and \
             `roots = [\"scenes/assets\"]` (project-relative, none overlapping an `[[import]] to`); \
             `[[import]] max_texture_size` is a texel count of at least 4",
        )
    })?;
    manifest.voice_caps()?;
    Ok(manifest)
}

fn validate_cook(manifest: &Manifest) -> Result<(), String> {
    for (index, table) in manifest.imports.iter().enumerate() {
        if let Some(cap) = table.max_texture_size.filter(|cap| *cap < 4) {
            return Err(format!(
                "`[[import]]` #{} `max_texture_size` = {cap} is below one 4×4 block: use 4 or more, or drop the key to keep the source size",
                index + 1
            ));
        }
    }
    let Some(cook) = &manifest.cook else {
        return Ok(());
    };
    if let Some(bad) = cook
        .targets
        .iter()
        .find(|name| CookTarget::from_name(name).is_none())
    {
        return Err(format!(
            "`[cook] targets` names `{bad}`: the valid targets are `desktop` and `ios`"
        ));
    }
    if cook.meshes {
        return Err(
            "`[cook] meshes = true`: binary mesh cooking does not exist; set `meshes = false` or drop the key"
                .to_string(),
        );
    }
    validate_roots(&cook.roots, &manifest.imports)
}

/// A path has one owner: a root is in its one spelling, repeats or nests no other root, and
/// neither holds nor sits inside an `[[import]] to`.
fn validate_roots(roots: &[PathBuf], imports: &[ImportSection]) -> Result<(), String> {
    for (index, root) in roots.iter().enumerate() {
        let text = root.to_string_lossy();
        if let Some(reason) = path_refusal(&text) {
            return Err(format!("`[cook] roots` #{} = `{text}` {reason}", index + 1));
        }
        for (other_index, other) in roots.iter().enumerate().take(index) {
            if starts_with_ignore_case(root, other) || starts_with_ignore_case(other, root) {
                return Err(format!(
                    "`[cook] roots` #{} = `{text}` repeats or nests #{} = `{}`: a map would be cooked twice",
                    index + 1,
                    other_index + 1,
                    other.display()
                ));
            }
        }
        for (table_index, table) in imports.iter().enumerate() {
            if starts_with_ignore_case(root, &table.to) || starts_with_ignore_case(&table.to, root)
            {
                return Err(format!(
                    "`[cook] roots` #{} = `{text}` overlaps `[[import]]` #{}'s `to` = `{}`: the table already cooks that tree",
                    index + 1,
                    table_index + 1,
                    table.to.display()
                ));
            }
        }
    }
    Ok(())
}

/// The nearest directory at or above `start` that holds a `ludion.toml`: the project root of a
/// game whose scene lives under `start`. `None` when no ancestor holds one.
pub fn find(start: &Path) -> Option<PathBuf> {
    start
        .ancestors()
        .find(|dir| dir.join("ludion.toml").is_file())
        .map(Path::to_path_buf)
}

/// One character a project name may hold — the charset `load` accepts and `ludion new` enforces.
pub fn is_plain_name_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '_' || c == '-'
}

/// The directory a `from` glob reads under: its segments before the first one holding `*`, never
/// the last (the file name). Empty when the sources sit at the project root.
fn from_directory(from: &str) -> PathBuf {
    let segments: Vec<&str> = from.split('/').collect();
    segments[..segments.len() - 1]
        .iter()
        .take_while(|segment| !segment.contains('*'))
        .collect()
}

/// `Path::starts_with` compares case-sensitively, but the derived trees live on filesystems that
/// do not.
pub(crate) fn starts_with_ignore_case(path: &Path, prefix: &Path) -> bool {
    let mut path = path.components();
    prefix.components().all(|part| {
        path.next()
            .is_some_and(|other| part.as_os_str().eq_ignore_ascii_case(other.as_os_str()))
    })
}

/// Why a manifest path is not a project-relative path in its one spelling, as a phrase that
/// finishes "… `x`": `paths::content_refusal`, plus `normalize`'s spelling (no `.`, empty
/// segment or `\`), and never the project directory itself.
fn path_refusal(text: &str) -> Option<String> {
    if let Some(refusal) = paths::content_refusal(text) {
        return Some(refusal.describe().to_string());
    }
    let canonical = paths::normalize(text);
    if canonical == "." {
        return Some("names the project directory itself".into());
    }
    (canonical != text).then(|| format!("is not in its one spelling: write `{canonical}`"))
}

fn validate_imports(imports: &[ImportSection]) -> Result<(), String> {
    for (index, table) in imports.iter().enumerate() {
        if let Some(reason) = path_refusal(&table.from) {
            return Err(format!(
                "`[[import]]` #{} `from` = `{}` {reason}",
                index + 1,
                table.from
            ));
        }
        let to = table.to.to_string_lossy();
        if let Some(reason) = path_refusal(&to) {
            return Err(format!(
                "`[[import]]` #{} `to` = `{to}` {reason}",
                index + 1
            ));
        }
        for source in imports {
            let prefix = from_directory(&source.from);
            let message = if starts_with_ignore_case(&table.to, &prefix) {
                "lies inside the `from` directory"
            } else if starts_with_ignore_case(&prefix, &table.to) {
                "holds the sources of the `from` directory"
            } else {
                continue;
            };
            return Err(format!(
                "`[[import]]` #{} `to` = `{}` {message} `{}` (`from` = `{}`): each sync rewrites files the next sync hashes, so the tree is never fresh",
                index + 1,
                table.to.display(),
                if prefix.as_os_str().is_empty() {
                    ".".into()
                } else {
                    prefix.display().to_string()
                },
                source.from
            ));
        }
        let same =
            |a: &Path, b: &Path| starts_with_ignore_case(a, b) && starts_with_ignore_case(b, a);
        if let Some(first) = imports[..index].iter().position(|t| same(&t.to, &table.to)) {
            return Err(format!(
                "`[[import]]` #{} and #{} share `to` = `{}`",
                first + 1,
                index + 1,
                table.to.display()
            ));
        }
        for (other_index, other) in imports.iter().enumerate().take(index) {
            let (inner, outer) = if starts_with_ignore_case(&table.to, &other.to) {
                (index, other_index)
            } else if starts_with_ignore_case(&other.to, &table.to) {
                (other_index, index)
            } else {
                continue;
            };
            return Err(format!(
                "`[[import]]` #{} `to` = `{}` lies inside #{}'s `to` = `{}`: one table's sync would sweep the other's tree",
                inner + 1,
                imports[inner].to.display(),
                outer + 1,
                imports[outer].to.display()
            ));
        }
    }
    Ok(())
}

impl Manifest {
    /// The targets `ludion sync` cooks for: `[cook] targets` in manifest order without repeats,
    /// `desktop` alone when the table is absent. Names are valid once `load` has accepted them.
    pub fn cook_targets(&self) -> Vec<CookTarget> {
        let Some(cook) = &self.cook else {
            return vec![CookTarget::Desktop];
        };
        let mut targets: Vec<CookTarget> = Vec::new();
        for target in cook.targets.iter().filter_map(|n| CookTarget::from_name(n)) {
            if !targets.contains(&target) {
                targets.push(target);
            }
        }
        targets
    }

    /// The `[cook] roots`, project-relative; empty without a `[cook]` table.
    pub fn cook_roots(&self) -> &[PathBuf] {
        self.cook.as_ref().map_or(&[], |cook| &cook.roots)
    }

    /// The `[audio]` voice caps, the engine defaults where a key is absent.
    ///
    /// # Errors
    /// `L5004` for a key other than `voices_per_sound` / `voices_total`, a value that is not an
    /// integer in `1..=VoiceCaps::MAX_TOTAL`, or a per-sound cap above the total.
    pub fn voice_caps(&self) -> Result<VoiceCaps, Diagnostic> {
        let invalid = |message: String| {
            Diagnostic::new(AUDIO_VOICE_CAPS_INVALID, message).with_help(format!(
                "write `[audio]` with `voices_per_sound` and `voices_total` as integers from 1 to {}, \
                 per-sound no larger than total (`ludion explain L5004`)",
                VoiceCaps::MAX_TOTAL
            ))
        };
        let mut caps = VoiceCaps::default();
        let Some(table) = &self.audio else {
            return Ok(caps);
        };
        for (key, value) in table {
            let slot = match key.as_str() {
                "voices_per_sound" => &mut caps.per_sound,
                "voices_total" => &mut caps.total,
                _ => {
                    return Err(invalid(format!(
                        "`[audio] {key}` is not a key: the legal keys are `voices_per_sound` and `voices_total`"
                    )));
                }
            };
            let in_range = value
                .as_integer()
                .and_then(|n| usize::try_from(n).ok())
                .filter(|n| (1..=VoiceCaps::MAX_TOTAL).contains(n));
            let Some(n) = in_range else {
                return Err(invalid(format!(
                    "`[audio] {key}` = `{value}` must be an integer from 1 to {}",
                    VoiceCaps::MAX_TOTAL
                )));
            };
            *slot = n;
        }
        if caps.per_sound > caps.total {
            return Err(invalid(format!(
                "`[audio] voices_per_sound` = `{}` exceeds `voices_total` = `{}`, so it could never bind",
                caps.per_sound, caps.total
            )));
        }
        Ok(caps)
    }

    /// The `[ios]` bundle identifier.
    ///
    /// # Errors
    /// `L0023` when the table or its `bundle_id` is absent, or the identifier holds a character
    /// outside `A-Z a-z 0-9 . -`.
    pub fn ios_bundle_id(&self) -> Result<&str, Diagnostic> {
        let Some(ios) = &self.ios else {
            return Err(Diagnostic::new(
                MANIFEST_INVALID,
                "`ludion.toml` has no `[ios]` table with a `bundle_id`",
            )
            .with_help(
                "add `[ios]` and `bundle_id = \"com.example.mygame\"` (reverse-DNS; keep it stable \
                 once a device has installed the app)",
            ));
        };
        let id = ios.bundle_id.as_str();
        let valid = !id.is_empty()
            && id
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '-');
        if valid {
            Ok(id)
        } else {
            Err(Diagnostic::new(
                MANIFEST_INVALID,
                format!("`[ios] bundle_id` = `{id}` is empty or holds a character outside A-Z a-z 0-9 . -"),
            )
            .with_help("use letters, digits, `.` and `-` only (a `_` in the project name becomes `-`)"))
        }
    }

    /// The `[ios]` quality preset, lowercased; `None` when the manifest names none.
    ///
    /// # Errors
    /// `L0023` when the value is not one of the presets. An override file is not accepted: the app
    /// carries only a name.
    pub fn ios_quality(&self) -> Result<Option<&'static str>, Diagnostic> {
        let Some(quality) = self.ios.as_ref().and_then(|ios| ios.quality.as_deref()) else {
            return Ok(None);
        };
        let lowered = quality.trim().to_ascii_lowercase();
        match Quality::PRESET_NAMES.iter().find(|name| **name == lowered) {
            Some(name) => Ok(Some(name)),
            None => Err(Diagnostic::new(
                MANIFEST_INVALID,
                format!("`[ios] quality` = `{quality}` is not a quality preset"),
            )
            .with_help(format!(
                "use one of {} (`ludion docs quality`)",
                Quality::PRESET_NAMES.join(", ")
            ))),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(text: &str) -> Result<Manifest, Diagnostic> {
        static NEXT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        let n = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!("ludion-manifest-{}-{n}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("ludion.toml"), text).unwrap();
        let result = load(&dir);
        let _ = std::fs::remove_dir_all(&dir);
        result
    }

    #[test]
    fn a_good_manifest_loads() {
        let m = parse(
            "name = \"vigil\"\ndefault_scene = \"scenes/camp.yaml\"\n[ios]\nbundle_id = \"dev.ludion.vigil\"\n",
        )
        .unwrap();
        assert_eq!(m.name, "vigil");
        assert_eq!(m.ios_bundle_id().unwrap(), "dev.ludion.vigil");
    }

    #[test]
    fn the_ios_quality_is_a_preset_name() {
        let with = |quality: &str| {
            parse(&format!(
                "name = \"a\"\ndefault_scene = \"s/a.yaml\"\n[ios]\nbundle_id = \"a.b\"\nquality = \"{quality}\"\n"
            ))
            .unwrap()
        };
        assert_eq!(with("Medium").ios_quality().unwrap(), Some("medium"));
        let err = with("laptop.toml").ios_quality().unwrap_err();
        assert_eq!(err.code, MANIFEST_INVALID);
        assert!(err.message.contains("laptop.toml"), "{}", err.message);
        let none =
            parse("name = \"a\"\ndefault_scene = \"s/a.yaml\"\n[ios]\nbundle_id = \"a.b\"\n")
                .unwrap();
        assert_eq!(none.ios_quality().unwrap(), None);
    }

    #[test]
    fn an_unknown_key_is_refused() {
        let err = parse("name = \"a\"\ndefault_scene = \"s/a.yaml\"\nbogus = 1\n").unwrap_err();
        assert_eq!(err.code, MANIFEST_INVALID);
    }

    #[test]
    fn the_cook_table_defaults_and_validates() {
        let base = "name = \"a\"\ndefault_scene = \"s/a.yaml\"\n";
        let none = parse(base).unwrap();
        assert!(none.cook.is_none());
        assert_eq!(none.cook_targets(), [CookTarget::Desktop]);
        let empty = parse(&format!("{base}[cook]\n")).unwrap();
        let cook = empty.cook.as_ref().unwrap();
        assert!(cook.textures && !cook.meshes);
        assert_eq!(empty.cook_targets(), [CookTarget::Desktop]);
        let both = parse(&format!(
            "{base}[cook]\ntargets = [\"ios\", \"desktop\", \"ios\"]\n"
        ))
        .unwrap();
        assert_eq!(both.cook_targets(), [CookTarget::Ios, CookTarget::Desktop]);

        let err = parse(&format!("{base}[cook]\nbogus = 1\n")).unwrap_err();
        assert_eq!(err.code, MANIFEST_INVALID);
        let err = parse(&format!("{base}[cook]\ntargets = [\"web\"]\n")).unwrap_err();
        assert_eq!(err.code, MANIFEST_INVALID);
        assert!(err.message.contains("web") && err.message.contains("ios"));
        let err = parse(&format!("{base}[cook]\nmeshes = true\n")).unwrap_err();
        assert_eq!(err.code, MANIFEST_INVALID);
        assert!(err.message.contains("meshes"), "{}", err.message);
        assert!(parse(&format!("{base}[cook]\nmeshes = false\ntextures = false\n")).is_ok());
    }

    #[test]
    fn cook_roots_load_and_one_owner_per_path_is_enforced() {
        let base = "name = \"a\"\ndefault_scene = \"s/a.yaml\"\n";
        assert!(parse(base).unwrap().cook_roots().is_empty());
        assert!(
            parse(&format!("{base}[cook]\n"))
                .unwrap()
                .cook_roots()
                .is_empty()
        );
        let good = parse(&format!(
            "{base}[cook]\nroots = [\"scenes/assets\", \"extra\"]\n"
        ))
        .unwrap();
        assert_eq!(
            good.cook_roots(),
            [PathBuf::from("scenes/assets"), PathBuf::from("extra")]
        );
        for root in ["../x", "/x", "a/", ".", "a/./b", "a\\\\b"] {
            let err = parse(&format!("{base}[cook]\nroots = [\"{root}\"]\n")).unwrap_err();
            assert_eq!(err.code, MANIFEST_INVALID, "{root}");
        }
        for roots in [
            "\"a\", \"a\"",
            "\"a\", \"A\"",
            "\"a\", \"a/b\"",
            "\"a/b\", \"a\"",
        ] {
            let err = parse(&format!("{base}[cook]\nroots = [{roots}]\n")).unwrap_err();
            assert!(err.message.contains("nests"), "{roots}: {}", err.message);
        }
        let with_table = |to: &str| {
            format!(
                "{base}[[import]]\nfrom = \"k/*.gltf\"\nto = \"{to}\"\n[cook]\nroots = [\"assets/derived\"]\n"
            )
        };
        for to in ["assets/derived", "assets/derived/sub", "assets"] {
            let err = parse(&with_table(to)).unwrap_err();
            assert!(err.message.contains("overlaps"), "{to}: {}", err.message);
        }
        assert!(parse(&with_table("assets/other")).is_ok());
    }

    #[test]
    fn an_import_max_texture_size_loads_and_zero_is_refused() {
        let table = |extra: &str| {
            format!(
                "name = \"a\"\ndefault_scene = \"s/a.yaml\"\n[[import]]\nfrom = \"k/*.gltf\"\nto = \"out\"\n{extra}"
            )
        };
        assert_eq!(parse(&table("")).unwrap().imports[0].max_texture_size, None);
        let capped = parse(&table("max_texture_size = 512\n")).unwrap();
        assert_eq!(capped.imports[0].max_texture_size, Some(512));
        let err = parse(&table("max_texture_size = 0\n")).unwrap_err();
        assert_eq!(err.code, MANIFEST_INVALID);
        assert!(err.message.contains("max_texture_size"), "{}", err.message);
        let err = parse(&table("max_texture_size = 3\n")).unwrap_err();
        assert_eq!(err.code, MANIFEST_INVALID);
        assert!(
            err.message.contains("below one 4×4 block"),
            "{}",
            err.message
        );
        assert!(parse(&table("max_texture_size = 4\n")).is_ok());
        let err = parse(&table("max_size = 4\n")).unwrap_err();
        assert_eq!(err.code, MANIFEST_INVALID);
    }

    #[test]
    fn a_scene_without_a_directory_is_refused() {
        let err = parse("name = \"a\"\ndefault_scene = \"a.yaml\"\n").unwrap_err();
        assert_eq!(err.code, MANIFEST_INVALID);
    }

    #[test]
    fn a_scene_path_that_leaves_or_widens_the_project_is_refused() {
        for scene in [
            "/abs/scenes/a.yaml",
            "../scenes/a.yaml",
            "s/../a.yaml",
            "./s/a.yaml",
            "C:x/a.yaml",
            "s/nul.yaml",
            "s//a.yaml",
            "s\\\\a.yaml",
        ] {
            let err = parse(&format!("name = \"a\"\ndefault_scene = \"{scene}\"\n")).unwrap_err();
            assert_eq!(err.code, MANIFEST_INVALID, "{scene}");
            let shown = scene.replace("\\\\", "\\");
            assert!(err.message.contains(&shown), "{}", err.message);
        }
    }

    #[test]
    fn a_name_outside_the_plain_set_is_refused() {
        for name in ["", "my game", "a/b", "a.b"] {
            let err = parse(&format!(
                "name = \"{name}\"\ndefault_scene = \"s/a.yaml\"\n"
            ))
            .unwrap_err();
            assert_eq!(err.code, MANIFEST_INVALID, "{name:?}");
        }
    }

    #[test]
    fn import_tables_load_in_order() {
        let m = parse(
            "name = \"a\"\ndefault_scene = \"s/a.yaml\"\n[[import]]\nfrom = \"art/*/x/*.gltf\"\nto = \"assets/town\"\n[[import]]\nfrom = \"b/*.gltf\"\nto = \"assets/b\"\n",
        )
        .unwrap();
        assert_eq!(m.imports.len(), 2);
        assert_eq!(m.imports[0].from, "art/*/x/*.gltf");
        assert_eq!(m.imports[1].to, PathBuf::from("assets/b"));
    }

    #[test]
    fn a_bad_import_table_is_refused() {
        let table = |from: &str, to: &str| {
            format!(
                "name = \"a\"\ndefault_scene = \"s/a.yaml\"\n[[import]]\nfrom = \"{from}\"\nto = \"{to}\"\n"
            )
        };
        for (from, to) in [
            ("k/*.gltf", "../out"),
            ("k/*.gltf", "./b"),
            ("k/*.gltf", ""),
            ("/abs/*.gltf", "out"),
            ("../k/*.gltf", "out"),
            ("", "out"),
            ("k/*.gltf", "..."),
            ("k/*.gltf", "nul"),
            ("k/*.gltf", "."),
            ("k/*.gltf", "a/../b"),
            ("k/*.gltf", "C:x"),
        ] {
            let err = parse(&table(from, to)).unwrap_err();
            assert_eq!(err.code, MANIFEST_INVALID, "{from} -> {to}");
        }
        let twice = format!(
            "{}[[import]]\nfrom = \"o/*.gltf\"\nto = \"out\"\n",
            table("k/*.gltf", "out")
        );
        let err = parse(&twice).unwrap_err();
        assert!(err.message.contains("share"), "{}", err.message);
        let err = parse(&format!(
            "{}[[import]]\nfrom = \"o/*.gltf\"\nto = \"Out\"\n",
            table("k/*.gltf", "out")
        ))
        .unwrap_err();
        assert!(err.message.contains("share"), "{}", err.message);
        let both = |first: &str, second: &str| {
            format!(
                "{}[[import]]\nfrom = \"o/*.gltf\"\nto = \"{second}\"\n",
                table("k/*.gltf", first)
            )
        };
        for (first, second) in [
            ("assets", "assets/town"),
            ("assets/town", "assets"),
            ("assets", "Assets/derived"),
            ("assets/models", "Assets/models/rocks"),
        ] {
            let err = parse(&both(first, second)).unwrap_err();
            assert!(err.message.contains("lies inside"), "{}", err.message);
        }
        assert!(parse(&both("assets/town", "assets/townx")).is_ok());
        let unknown = format!("{}glob = 1\n", table("k/*.gltf", "out"));
        assert_eq!(parse(&unknown).unwrap_err().code, MANIFEST_INVALID);
    }

    #[test]
    fn a_to_inside_a_from_directory_is_refused() {
        let table = |from: &str, to: &str| {
            format!(
                "name = \"a\"\ndefault_scene = \"s/a.yaml\"\n[[import]]\nfrom = \"{from}\"\nto = \"{to}\"\n"
            )
        };
        for (from, to) in [
            ("kits/*.gltf", "kits/out"),
            ("kits/*.gltf", "kits"),
            ("art/kits/*.gltf", "art"),
            ("*.gltf", "out"),
            ("kits/well.gltf", "kits"),
            ("kits/*.gltf", "Kits/out"),
        ] {
            let err = parse(&table(from, to)).unwrap_err();
            assert_eq!(err.code, MANIFEST_INVALID, "{from} -> {to}");
            assert!(
                err.message.contains(from) && err.message.contains(to),
                "{from} -> {to}: {}",
                err.message
            );
        }
        for (from, to) in [
            ("art/town/kits/*/export/*.gltf", "assets/town"),
            ("kits/well.gltf", "derived"),
        ] {
            assert!(parse(&table(from, to)).is_ok(), "{from} -> {to}");
        }
        let other_table = format!(
            "{}[[import]]\nfrom = \"o/*.gltf\"\nto = \"kits/out\"\n",
            table("kits/*.gltf", "derived")
        );
        assert_eq!(parse(&other_table).unwrap_err().code, MANIFEST_INVALID);
    }

    fn audio(body: &str) -> Result<Manifest, Diagnostic> {
        parse(&format!(
            "name = \"a\"\ndefault_scene = \"s/a.yaml\"\n[audio]\n{body}\n"
        ))
    }

    #[test]
    fn the_voice_caps_default_without_an_audio_table() {
        let m = parse("name = \"a\"\ndefault_scene = \"s/a.yaml\"\n").unwrap();
        assert_eq!(m.voice_caps().unwrap(), VoiceCaps::default());
    }

    #[test]
    fn the_audio_table_overrides_the_voice_caps() {
        let both = audio("voices_per_sound = 6\nvoices_total = 96").unwrap();
        assert_eq!(
            both.voice_caps().unwrap(),
            VoiceCaps {
                per_sound: 6,
                total: 96
            }
        );
        let one = audio("voices_total = 32").unwrap();
        assert_eq!(
            one.voice_caps().unwrap(),
            VoiceCaps {
                per_sound: 4,
                total: 32
            }
        );
    }

    #[test]
    fn a_bad_audio_table_is_refused_as_l5004() {
        for body in [
            "voices_per_sound = 0",
            "voices_total = 257",
            "voices_total = \"4\"",
            "voices_per_sound = 2.5",
            "voices = 4",
            "voices_per_sound = 5\nvoices_total = 4",
            "voices_per_sound = 100",
        ] {
            let err = audio(body).unwrap_err();
            assert_eq!(err.code, AUDIO_VOICE_CAPS_INVALID, "{body}");
            assert!(err.message.contains("[audio]"), "{}", err.message);
        }
    }

    #[test]
    fn find_walks_up_to_the_nearest_manifest() {
        let root =
            std::env::temp_dir().join(format!("ludion-manifest-find-{}", std::process::id()));
        let nested = root.join("scenes").join("deep");
        std::fs::create_dir_all(&nested).unwrap();
        assert_eq!(find(&nested).filter(|dir| dir.starts_with(&root)), None);
        std::fs::write(root.join("ludion.toml"), "").unwrap();
        assert_eq!(find(&nested), Some(root.clone()));
        std::fs::write(root.join("scenes").join("ludion.toml"), "").unwrap();
        assert_eq!(find(&nested), Some(root.join("scenes")));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_missing_ios_table_names_bundle_id() {
        let m = parse("name = \"a\"\ndefault_scene = \"s/a.yaml\"\n").unwrap();
        let err = m.ios_bundle_id().unwrap_err();
        assert_eq!(err.code, MANIFEST_INVALID);
        assert!(err.message.contains("bundle_id"), "{}", err.message);
    }

    #[test]
    fn a_bad_bundle_id_is_refused() {
        let m = parse(
            "name = \"a\"\ndefault_scene = \"s/a.yaml\"\n[ios]\nbundle_id = \"com.my_game\"\n",
        )
        .unwrap();
        assert_eq!(m.ios_bundle_id().unwrap_err().code, MANIFEST_INVALID);
    }
}
