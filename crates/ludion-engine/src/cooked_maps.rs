//! Cooked mesh maps: the engine half of `ludion sync`'s cook stage.
//!
//! Under [`TextureSource::Cooked`] a mesh map that an `[[import]]` table produced is read from the
//! content-addressed store `<project>/.ludion/cooked/<target>/<cook key>.ktx2` as BC7 or ASTC instead
//! of being decoded from its PNG. The key folds in the PNG's content hash, the map's colour role, the
//! target and the table's `max_texture_size`, so the lookup reads the PNG's bytes (to hash them),
//! never decodes them, and finds the file with no index. A map that is missing, corrupt or unusable
//! on this GPU loads from its PNG, and the load reports the fallbacks as one `L1271`.
//!
//! The axis is render-only: nothing here reaches the world, so `WorldHash`, replays and snapshots do
//! not see it.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use ludion_assets::AssetBytes;
use ludion_assets::cooked::{
    ColorSpace, CookSettings, CookTarget, Ktx2Error, cook_key, cooked_file_name, read_ktx2,
};
use ludion_core::codes::code;
use ludion_core::diagnostics::Diagnostic;
use ludion_renderer::{CompressedUploadError, CookedMap, check_compressed};

/// Which pixels a host wants for imported mesh maps.
///
/// # Example
/// ```
/// use ludion_engine::TextureSource;
/// assert_eq!(TextureSource::default(), TextureSource::Source);
/// assert_eq!(TextureSource::from_name("cooked"), Some(TextureSource::Cooked));
/// assert_eq!(TextureSource::Cooked.name(), "cooked");
/// ```
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum TextureSource {
    /// Decode every map from its PNG.
    #[default]
    Source,
    /// Read the cooked BC7/ASTC copy, falling back to the PNG where there is none.
    Cooked,
}

impl TextureSource {
    /// The axis value's name: `"source"` or `"cooked"`.
    pub fn name(self) -> &'static str {
        match self {
            TextureSource::Source => "source",
            TextureSource::Cooked => "cooked",
        }
    }

    /// The value a name spells; the inverse of [`TextureSource::name`].
    pub fn from_name(name: &str) -> Option<TextureSource> {
        match name {
            "source" => Some(TextureSource::Source),
            "cooked" => Some(TextureSource::Cooked),
            _ => None,
        }
    }
}

/// Why a map was not read from the store.
#[derive(Clone, PartialEq, Eq, Debug)]
pub(crate) enum CookMiss {
    /// This GPU offers neither BC7 nor ASTC.
    Unsupported,
    /// No file under the key (`None`), or one that could not be read (the I/O error).
    Missing(Option<String>),
    /// The file is not a valid KTX2 for the target and role.
    Corrupt(Ktx2Error),
    /// The file decodes but cannot be uploaded to the map's slot.
    Invalid(CompressedUploadError),
}

impl CookMiss {
    /// The reason in one clause.
    pub(crate) fn reason(&self) -> String {
        match self {
            CookMiss::Unsupported => "this GPU offers neither BC7 nor ASTC".to_string(),
            CookMiss::Missing(None) => "no cooked file for this map".to_string(),
            CookMiss::Missing(Some(error)) => format!("the cooked file is unreadable: {error}"),
            CookMiss::Corrupt(error) => format!("the cooked file is corrupt: {error}"),
            CookMiss::Invalid(error) => format!("the cooked file is unusable: {error}"),
        }
    }

    fn help(&self) -> &'static str {
        match self {
            CookMiss::Unsupported => "nothing to fix: this GPU samples the PNG",
            CookMiss::Missing(_) => "run `ludion sync`",
            CookMiss::Corrupt(_) | CookMiss::Invalid(_) => {
                "delete the file from `.ludion/cooked/`, then run `ludion sync`"
            }
        }
    }
}

/// The colour role a cooked map is read in: its colour space and, for a base-colour map bound by an
/// `alpha_mode: mask` mesh, the cutoff (as `f32::to_bits`) its mips preserve coverage at.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub(crate) struct CookRole {
    pub(crate) space: ColorSpace,
    pub(crate) alpha_cutoff: Option<u32>,
}

/// Where cooked maps live for one project and which maps they cover: resolved once per host setting
/// from the manifest, the renderer's supported [`CookTarget`]s and which store exists.
#[derive(Clone, PartialEq, Eq, Debug)]
pub(crate) struct CookLookup {
    /// `None`: the GPU supports neither format, so every covered map misses as unsupported.
    target: Option<CookTarget>,
    /// `<project>/.ludion/cooked/<target>`.
    store: PathBuf,
    /// Each `[[import]]` table's `to`, as a prefix of a content-root-relative map path, with its cap.
    tables: Vec<(String, Option<u32>)>,
}

/// The lookup for the project whose content root is `project_root`, or `None` when no manifest
/// governs it, the manifest does not load or it has no `[[import]]`: every map then uses its PNG
/// silently, because sync cooks nothing for it.
///
/// `supported` is the device's targets in preference order. The pick is the first whose store
/// directory exists, else the first supported: a bundle ships one target, and an Apple-silicon GPU
/// that reports BC must still read the ASTC store it was shipped.
pub(crate) fn lookup_for(project_root: &Path, supported: &[CookTarget]) -> Option<CookLookup> {
    let project = crate::manifest::find(project_root)?;
    let manifest = crate::manifest::load(&project).ok()?;
    if manifest.imports.is_empty() {
        return None;
    }
    let depth = project_root
        .strip_prefix(&project)
        .ok()?
        .components()
        .count();
    let tables = manifest
        .imports
        .iter()
        .map(|table| {
            let to = table.to.to_string_lossy().replace('\\', "/");
            (table_prefix(depth, &to), table.max_texture_size)
        })
        .collect();
    let target = pick_target(&project, supported);
    let store = project
        .join(".ludion/cooked")
        .join(target.map_or("", CookTarget::name));
    Some(CookLookup {
        target,
        store,
        tables,
    })
}

/// The first supported target whose store directory exists under `project`, else the first supported.
fn pick_target(project: &Path, supported: &[CookTarget]) -> Option<CookTarget> {
    supported
        .iter()
        .copied()
        .find(|target| project.join(".ludion/cooked").join(target.name()).is_dir())
        .or_else(|| supported.first().copied())
}

/// The prefix a map under the table `to` carries when the content root is `depth` directories below
/// the manifest: one `..` per directory, then `to` (vigil's `scenes/` root names its town maps
/// `../assets/town/…`).
fn table_prefix(depth: usize, to: &str) -> String {
    let climb = "../".repeat(depth);
    ludion_core::paths::normalize(&format!("{climb}{to}"))
}

impl CookLookup {
    /// The target this lookup reads, `None` when the GPU supports neither format.
    pub(crate) fn target(&self) -> Option<CookTarget> {
        self.target
    }

    /// The cap of the table that owns the map at the normalized `rel`; `None` when no table does,
    /// which is a map sync never cooks. Sync cooks only `<to>/<stem>/…` (a non-dot stem directory
    /// holding the map), so a file directly under `<to>/` is not covered.
    pub(crate) fn cap_for(&self, rel: &str) -> Option<Option<u32>> {
        self.tables
            .iter()
            .find(|(prefix, _)| {
                rel.strip_prefix(prefix.as_str())
                    .and_then(|rest| rest.strip_prefix('/'))
                    .and_then(|rest| rest.split_once('/'))
                    .is_some_and(|(stem, file)| {
                        !stem.is_empty() && !stem.starts_with('.') && !file.is_empty()
                    })
            })
            .map(|(_, cap)| *cap)
    }

    /// Reads the cooked copy of the map whose PNG bytes are `png`, in `role` under the table cap
    /// `cap`.
    pub(crate) fn read(
        &self,
        png: &AssetBytes,
        role: CookRole,
        cap: Option<u32>,
    ) -> Result<Arc<CookedMap>, CookMiss> {
        let target = self.target.ok_or(CookMiss::Unsupported)?;
        let key = cook_key(
            png.hash,
            &CookSettings {
                target,
                color_space: role.space,
                max_texture_size: cap,
                alpha_cutoff: role.alpha_cutoff.map(f32::from_bits),
            },
        );
        let bytes = std::fs::read(self.store.join(cooked_file_name(key))).map_err(|error| {
            match error.kind() {
                std::io::ErrorKind::NotFound => CookMiss::Missing(None),
                _ => CookMiss::Missing(Some(error.to_string())),
            }
        })?;
        let image = read_ktx2(&bytes, target, role.space).map_err(CookMiss::Corrupt)?;
        check_compressed(&image, role.space == ColorSpace::Srgb).map_err(CookMiss::Invalid)?;
        Ok(Arc::new(CookedMap { key, image }))
    }
}

/// How many maps one `L1271` lists in full.
const LISTED: usize = 8;

/// The one warning for a load's fallbacks: how many maps came from their PNG, and why for the first
/// few.
pub(crate) fn not_cooked_warning(
    target: Option<CookTarget>,
    misses: &[(String, CookMiss)],
) -> Diagnostic {
    let for_target = target.map_or("this GPU", |target| target.name());
    let more = misses.len().saturating_sub(LISTED);
    let tail = if more > 0 {
        format!(" (and {more} more)")
    } else {
        String::new()
    };
    let mut diagnostic = Diagnostic::new(
        code::TEXTURE_NOT_COOKED,
        format!(
            "{} mesh maps are not cooked for {for_target}; they load from their PNG sources{tail}",
            misses.len()
        ),
    )
    .with_help("run `ludion sync`");
    for (path, miss) in misses.iter().take(LISTED) {
        diagnostic = diagnostic.with_related(
            Diagnostic::new(code::TEXTURE_NOT_COOKED, miss.reason())
                .with_path(path.clone())
                .with_help(miss.help()),
        );
    }
    diagnostic
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lookup(depth: usize, tables: &[(&str, Option<u32>)]) -> CookLookup {
        CookLookup {
            target: Some(CookTarget::Desktop),
            store: PathBuf::new(),
            tables: tables
                .iter()
                .map(|(to, cap)| (table_prefix(depth, to), *cap))
                .collect(),
        }
    }

    #[test]
    fn a_content_root_at_the_manifest_dir_prefixes_the_table_directly() {
        let lookup = lookup(0, &[("assets/kit", Some(512)), ("assets/town", None)]);
        assert_eq!(
            lookup.cap_for("assets/kit/k/textures/x.png"),
            Some(Some(512))
        );
        assert_eq!(lookup.cap_for("assets/town/t/x.png"), Some(None));
    }

    #[test]
    fn a_content_root_below_the_manifest_climbs_to_the_table() {
        let lookup = lookup(1, &[("assets/town", Some(256))]);
        assert_eq!(
            lookup.cap_for("../assets/town/k/textures/x.png"),
            Some(Some(256))
        );
        // The same file spelled from the wrong depth is under no table.
        assert_eq!(lookup.cap_for("assets/town/k/x.png"), None);
    }

    #[test]
    fn a_map_outside_every_table_or_sharing_only_a_name_prefix_is_not_covered() {
        let lookup = lookup(0, &[("assets/kit", None)]);
        assert_eq!(lookup.cap_for("textures/x.png"), None);
        assert_eq!(lookup.cap_for("assets/kitchen/x.png"), None);
        assert_eq!(lookup.cap_for("assets/kit"), None);
    }

    #[test]
    fn a_map_outside_a_stem_directory_is_not_covered() {
        let lookup = lookup(0, &[("assets/kit", None)]);
        assert_eq!(lookup.cap_for("assets/kit/x.png"), None);
        assert_eq!(lookup.cap_for("assets/kit/.hidden/x.png"), None);
        assert_eq!(lookup.cap_for("assets/kit/stem/x.png"), Some(None));
    }

    #[test]
    fn no_manifest_means_no_lookup() {
        let dir =
            std::env::temp_dir().join(format!("ludion-cook-no-manifest-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        // An ancestor of the temp dir may carry a manifest on a developer box, so only assert the
        // negative when none does.
        if crate::manifest::find(&dir).is_none() {
            assert!(lookup_for(&dir, &[CookTarget::Desktop]).is_none());
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_target_is_the_first_supported_one_that_has_a_store() {
        let dir = std::env::temp_dir().join(format!("ludion-cook-pick-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("ludion.toml"),
            "name = \"pick\"\ndefault_scene = \"scenes/main.yaml\"\n\n[[import]]\nfrom = \"art/*.gltf\"\nto = \"assets/kit\"\n",
        )
        .unwrap();
        let both = [CookTarget::Desktop, CookTarget::Ios];
        let target = |supported: &[CookTarget]| lookup_for(&dir, supported).unwrap().target();
        let store = |name: &str| std::fs::create_dir_all(dir.join(".ludion/cooked").join(name));

        assert_eq!(target(&both), Some(CookTarget::Desktop), "no store");
        store("ios").unwrap();
        assert_eq!(target(&both), Some(CookTarget::Ios), "a bundle's ios store");
        store("desktop").unwrap();
        assert_eq!(target(&both), Some(CookTarget::Desktop), "both stores");
        std::fs::remove_dir_all(dir.join(".ludion/cooked/ios")).unwrap();
        assert_eq!(
            target(&[CookTarget::Ios]),
            Some(CookTarget::Ios),
            "no ios store"
        );
        assert_eq!(target(&[]), None, "no supported target");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_warning_lists_eight_maps_and_counts_the_rest() {
        let misses: Vec<(String, CookMiss)> = (0..11)
            .map(|i| (format!("a/{i}.png"), CookMiss::Missing(None)))
            .collect();
        let warning = not_cooked_warning(Some(CookTarget::Ios), &misses);
        assert_eq!(warning.code, code::TEXTURE_NOT_COOKED);
        assert!(
            warning
                .message
                .starts_with("11 mesh maps are not cooked for ios")
        );
        assert!(warning.message.contains("and 3 more"));
        assert_eq!(warning.related.len(), 8);
        assert_eq!(warning.related[0].path.as_deref(), Some("a/0.png"));
    }
}
