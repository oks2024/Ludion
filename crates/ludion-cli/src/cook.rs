//! The cook stage of `ludion sync`: compresses every PNG map an imported `*.mesh.yaml` references
//! into a KTX2 file of `<project>/.ludion/cooked/<target>/<key>.ktx2`, `<key>` being
//! [`ludion_assets::cooked::cook_key`] over the source file's hash and the cook settings.
//!
//! The stage is incremental by key (a file already in the store is reused, so a changed source, cap
//! or cooker is a new key and a new file) and parallel through `ludion_core::par` (the store is
//! byte-identical at any thread count). Afterwards every store entry no live mesh references is
//! deleted: the store is the sync's alone, and `write_atomic` temp debris from a killed run goes
//! with it. Only the import trees' own maps are cooked; a map that leaves its `<to>/<stem>/`
//! directory is skipped.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Component, Path, PathBuf};

use ludion_assets::cooked::{ColorSpace, CookSettings, CookTarget, cook_key, cooked_file_name};
use ludion_assets::import_sync::collect_files;
use ludion_assets::{Image, content_hash, decode_png};
use ludion_core::atomic::write_atomic;
use ludion_core::codes::code::{COOK_ENCODE_FAILED, IMPORT_OUTPUT_IO};
use ludion_core::diagnostics::Diagnostic;
use ludion_scene::{MeshTextures, parse_mesh};

use crate::sync::io_failure;

/// The store's location under the project root.
const STORE: &str = ".ludion/cooked";

/// What the caller asks of the stage beyond the manifest.
pub(crate) struct CookRequest {
    /// `false` skips the stage and its GC (`--no-cook`).
    pub enabled: bool,
    /// Targets to cook besides the manifest's.
    pub extra_targets: Vec<CookTarget>,
}

impl Default for CookRequest {
    fn default() -> Self {
        CookRequest {
            enabled: true,
            extra_targets: Vec::new(),
        }
    }
}

/// One map that did not cook.
#[derive(Debug)]
pub(crate) struct CookFailure {
    /// The map, or the mesh that could not be read: project-relative with `/` separators.
    pub map: String,
    /// The target it failed for; `all` when the failure does not depend on one.
    pub target: &'static str,
    pub diagnostics: Vec<Diagnostic>,
}

/// What one cook did. Counts are unique keys summed over the targets.
#[derive(Debug, Default)]
pub(crate) struct CookOutcome {
    pub cooked: usize,
    pub reused: usize,
    pub failed: Vec<CookFailure>,
    /// The bytes left in the cooked target directories.
    pub store_bytes: u64,
}

/// One `[[import]]` table's derived tree.
pub(crate) struct CookTree {
    /// The absolute derived root.
    pub to: PathBuf,
    /// The stems the table found: each `<to>/<stem>/` is walked for meshes.
    pub stems: Vec<String>,
    pub max_texture_size: Option<u32>,
}

impl CookTree {
    /// A tree whose stems are every non-dot subdirectory of `to`, sorted: what a sync left there.
    pub(crate) fn whole(to: PathBuf, max_texture_size: Option<u32>) -> std::io::Result<CookTree> {
        let mut stems = Vec::new();
        if to.is_dir() {
            for entry in to.read_dir()? {
                let entry = entry?;
                let name = entry.file_name().to_string_lossy().into_owned();
                if !name.starts_with('.') && entry.path().is_dir() {
                    stems.push(name);
                }
            }
        }
        stems.sort();
        Ok(CookTree {
            to,
            stems,
            max_texture_size,
        })
    }
}

/// A mesh document to read.
struct MeshFile {
    path: PathBuf,
    /// The stem directory the mesh and its maps must stay inside.
    stem_dir: PathBuf,
    max_texture_size: Option<u32>,
}

/// One map in one colour role, before the target is chosen.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord)]
struct MapUse {
    png: PathBuf,
    srgb: bool,
    max_texture_size: Option<u32>,
}

/// One store entry to produce.
struct Unit {
    map: MapUse,
    hash: u64,
}

/// The maps a mesh document names, in their colour role: colour maps are sRGB, data maps linear.
fn roles(textures: &MeshTextures) -> Vec<(ColorSpace, &str)> {
    [
        (ColorSpace::Srgb, &textures.base_color),
        (ColorSpace::Linear, &textures.metallic_roughness),
        (ColorSpace::Linear, &textures.normal),
        (ColorSpace::Linear, &textures.occlusion),
        (ColorSpace::Srgb, &textures.emissive),
    ]
    .into_iter()
    .filter_map(|(space, map)| map.as_ref().map(|map| (space, map.path.as_str())))
    .collect()
}

/// `reference` resolved against the directory of `mesh`, lexically, relative to `stem_dir`; `None`
/// when it is absolute or climbs out of `stem_dir`.
fn resolve_map(stem_dir: &Path, mesh: &Path, reference: &str) -> Option<PathBuf> {
    let mut parts: Vec<std::ffi::OsString> = mesh
        .strip_prefix(stem_dir)
        .ok()?
        .parent()?
        .components()
        .map(|part| part.as_os_str().to_owned())
        .collect();
    for part in Path::new(reference).components() {
        match part {
            Component::Normal(name) => parts.push(name.to_owned()),
            Component::CurDir => {}
            Component::ParentDir => {
                parts.pop()?;
            }
            Component::RootDir | Component::Prefix(_) => return None,
        }
    }
    (!parts.is_empty()).then(|| stem_dir.join(parts.iter().collect::<PathBuf>()))
}

/// `path` project-relative with `/` separators.
fn display(root: &Path, path: &Path) -> String {
    path.strip_prefix(root)
        .unwrap_or(path)
        .to_string_lossy()
        .replace('\\', "/")
}

fn failure(map: String, target: &'static str, diagnostic: Diagnostic) -> CookFailure {
    CookFailure {
        map,
        target,
        diagnostics: vec![diagnostic],
    }
}

/// What the trees' meshes reference, hashed.
struct Scan {
    uses: BTreeSet<MapUse>,
    hashes: BTreeMap<PathBuf, u64>,
    /// `false` when a mesh or PNG could not be read, so the referenced set is not the whole truth.
    complete: bool,
    failed: Vec<CookFailure>,
}

/// Walks the trees' meshes and hashes every map they reference.
fn scan(root: &Path, trees: &[CookTree]) -> Scan {
    let mut complete = true;
    let mut failed: Vec<CookFailure> = Vec::new();

    let mut meshes: Vec<MeshFile> = Vec::new();
    for tree in trees {
        for stem in &tree.stems {
            let stem_dir = tree.to.join(stem);
            let mut files = Vec::new();
            if stem_dir.is_dir()
                && let Err(error) = collect_files(&stem_dir, &stem_dir, &mut files)
            {
                complete = false;
                failed.push(failure(
                    display(root, &stem_dir),
                    "all",
                    io_failure("read", &stem_dir, &error),
                ));
                continue;
            }
            files.sort();
            meshes.extend(
                files
                    .iter()
                    .filter(|file| file.ends_with(".mesh.yaml"))
                    .map(|file| MeshFile {
                        path: stem_dir.join(file),
                        stem_dir: stem_dir.clone(),
                        max_texture_size: tree.max_texture_size,
                    }),
            );
        }
    }

    let parsed = ludion_core::par::par_map_chunks(&meshes, 1, |_, chunk| read_mesh(&chunk[0]));
    let mut uses: BTreeSet<MapUse> = BTreeSet::new();
    for result in parsed {
        match result {
            Ok(found) => uses.extend(found),
            Err((path, diagnostics)) => {
                complete = false;
                failed.push(CookFailure {
                    map: display(root, &path),
                    target: "all",
                    diagnostics,
                });
            }
        }
    }

    let pngs: Vec<&Path> = uses
        .iter()
        .map(|map| map.png.as_path())
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect();
    let hashed = ludion_core::par::par_map_chunks(&pngs, 1, |_, chunk| {
        std::fs::read(chunk[0]).map(|bytes| content_hash(&bytes))
    });
    let mut hashes: BTreeMap<PathBuf, u64> = BTreeMap::new();
    for (png, result) in pngs.iter().zip(hashed) {
        match result {
            Ok(hash) => {
                hashes.insert(png.to_path_buf(), hash);
            }
            Err(error) => {
                complete = false;
                failed.push(failure(
                    display(root, png),
                    "all",
                    io_failure("read", png, &error),
                ));
            }
        }
    }
    Scan {
        uses,
        hashes,
        complete,
        failed,
    }
}

/// The store entries `target` needs, by key.
fn units(scan: &Scan, target: CookTarget) -> BTreeMap<u64, Unit> {
    let mut units: BTreeMap<u64, Unit> = BTreeMap::new();
    for map in &scan.uses {
        let Some(&hash) = scan.hashes.get(&map.png) else {
            continue;
        };
        units
            .entry(cook_key(hash, &settings(target, map)))
            .or_insert(Unit {
                map: map.clone(),
                hash,
            });
    }
    units
}

/// The store entries the trees reference for one target.
pub(crate) struct Referenced {
    /// `cooked_file_name(key)` of every entry.
    pub files: BTreeSet<String>,
    pub failed: Vec<CookFailure>,
}

/// The file names `target`'s store holds for the trees' meshes.
pub(crate) fn referenced_files(root: &Path, trees: &[CookTree], target: CookTarget) -> Referenced {
    let scan = scan(root, trees);
    let files = units(&scan, target)
        .keys()
        .map(|&key| cooked_file_name(key))
        .collect();
    Referenced {
        files,
        failed: scan.failed,
    }
}

/// `target`'s store directory under the project root.
pub(crate) fn store_dir(root: &Path, target: CookTarget) -> PathBuf {
    root.join(STORE).join(target.name())
}

/// Cooks every map the trees' meshes reference for each target, then sweeps the store. `trees`
/// and `targets` are in manifest order; the result does not depend on the thread count.
pub(crate) fn cook_project(root: &Path, trees: &[CookTree], targets: &[CookTarget]) -> CookOutcome {
    let mut scan = scan(root, trees);
    let mut outcome = CookOutcome {
        failed: std::mem::take(&mut scan.failed),
        ..CookOutcome::default()
    };
    // A run that could not read every mesh or PNG does not know what the store should hold.
    let complete = scan.complete;

    for &target in targets {
        let store = store_dir(root, target);
        let units = units(&scan, target);
        let mut jobs: Vec<(u64, &Unit)> = Vec::new();
        for (&key, unit) in &units {
            if store.join(cooked_file_name(key)).is_file() {
                outcome.reused += 1;
            } else {
                jobs.push((key, unit));
            }
        }
        if !jobs.is_empty()
            && let Err(error) = std::fs::create_dir_all(&store)
        {
            outcome.failed.push(failure(
                display(root, &store),
                target.name(),
                io_failure("create", &store, &error),
            ));
            continue;
        }
        let encoded = ludion_core::par::par_map_chunks(&jobs, 1, |_, chunk| {
            let (key, unit) = chunk[0];
            encode(root, &store, target, key, unit)
        });
        for result in encoded {
            match result {
                Ok(()) => outcome.cooked += 1,
                Err(failed) => outcome.failed.push(failed),
            }
        }
        if complete {
            sweep(root, &store, &units, target, &mut outcome);
        }
        outcome.store_bytes += directory_bytes(&store);
    }
    outcome
        .failed
        .sort_by(|a, b| a.target.cmp(b.target).then_with(|| a.map.cmp(&b.map)));
    outcome
}

fn settings(target: CookTarget, map: &MapUse) -> CookSettings {
    CookSettings {
        target,
        color_space: if map.srgb {
            ColorSpace::Srgb
        } else {
            ColorSpace::Linear
        },
        max_texture_size: map.max_texture_size,
    }
}

/// Reads one mesh document and lists its in-tree maps; the error is the mesh and why.
fn read_mesh(mesh: &MeshFile) -> Result<Vec<MapUse>, (PathBuf, Vec<Diagnostic>)> {
    let text = std::fs::read_to_string(&mesh.path).map_err(|error| {
        (
            mesh.path.clone(),
            vec![io_failure("read", &mesh.path, &error)],
        )
    })?;
    let document = parse_mesh(&text).map_err(|diagnostics| (mesh.path.clone(), diagnostics))?;
    Ok(roles(&document.textures)
        .into_iter()
        .filter_map(|(space, reference)| {
            Some(MapUse {
                png: resolve_map(&mesh.stem_dir, &mesh.path, reference)?,
                srgb: space == ColorSpace::Srgb,
                max_texture_size: mesh.max_texture_size,
            })
        })
        .collect())
}

/// Decodes, cooks and stores one map, so a worker holds one map at a time.
fn encode(
    root: &Path,
    store: &Path,
    target: CookTarget,
    key: u64,
    unit: &Unit,
) -> Result<(), CookFailure> {
    let map = display(root, &unit.map.png);
    let fail = |diagnostic| failure(map.clone(), target.name(), diagnostic);
    let bytes = std::fs::read(&unit.map.png)
        .map_err(|error| fail(io_failure("read", &unit.map.png, &error)))?;
    let (width, height, rgba) = decode_png(&bytes, &map).map_err(fail)?;
    let image = Image {
        width,
        height,
        rgba,
        hash: unit.hash,
    };
    let cooked = ludion_cook::cook(&image, &settings(target, &unit.map)).map_err(|error| {
        fail(
            Diagnostic::new(
                COOK_ENCODE_FAILED,
                format!("could not cook `{map}` for {}: {error}", target.name()),
            )
            .with_help("re-export the PNG, or report the encoder error"),
        )
    })?;
    let path = store.join(cooked_file_name(key));
    write_atomic(&path, ludion_cook::write_ktx2(&cooked))
        .map_err(|error| fail(io_failure("write", &path, &error)))
}

/// Deletes every entry of `store` that is not a referenced `<key>.ktx2`.
fn sweep(
    root: &Path,
    store: &Path,
    units: &BTreeMap<u64, Unit>,
    target: CookTarget,
    outcome: &mut CookOutcome,
) {
    let Ok(entries) = store.read_dir() else {
        return;
    };
    let keep: BTreeSet<String> = units.keys().map(|&key| cooked_file_name(key)).collect();
    let mut stale: Vec<PathBuf> = entries
        .filter_map(Result::ok)
        .filter(|entry| !keep.contains(entry.file_name().to_string_lossy().as_ref()))
        .map(|entry| entry.path())
        .collect();
    stale.sort();
    for path in stale {
        let removed = if path.is_dir() {
            std::fs::remove_dir_all(&path)
        } else {
            std::fs::remove_file(&path)
        };
        if let Err(error) = removed {
            outcome.failed.push(CookFailure {
                map: display(root, &path),
                target: target.name(),
                diagnostics: vec![Diagnostic::new(
                    IMPORT_OUTPUT_IO,
                    format!("could not remove `{}`: {error}", path.display()),
                )],
            });
        }
    }
}

fn directory_bytes(dir: &Path) -> u64 {
    dir.read_dir().map_or(0, |entries| {
        entries
            .filter_map(Result::ok)
            .filter_map(|entry| entry.metadata().ok())
            .filter(std::fs::Metadata::is_file)
            .map(|meta| meta.len())
            .sum()
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_map_resolves_against_its_meshs_directory_and_stays_inside_the_stem() {
        let stem = Path::new("/p/to/kit");
        let mesh = stem.join("meshes").join("a.mesh.yaml");
        let resolved = |reference| resolve_map(stem, &mesh, reference);
        assert_eq!(
            resolved("../textures/x.png"),
            Some(stem.join("textures").join("x.png"))
        );
        assert_eq!(resolved("./x.png"), Some(stem.join("meshes").join("x.png")));
        assert_eq!(resolved("../../x.png"), None);
        assert_eq!(resolved("../../../outside.png"), None);
        assert_eq!(resolved("/abs/x.png"), None);
    }

    #[test]
    fn a_whole_tree_lists_its_sorted_non_dot_directories() {
        let to = std::env::temp_dir().join(format!("ludion-cook-whole-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&to);
        for dir in ["b", "a", ".ludion"] {
            std::fs::create_dir_all(to.join(dir)).unwrap();
        }
        std::fs::write(to.join("file.txt"), b"x").unwrap();
        let tree = CookTree::whole(to.clone(), Some(512)).unwrap();
        assert_eq!(tree.stems, ["a", "b"]);
        assert_eq!(tree.max_texture_size, Some(512));
        std::fs::remove_dir_all(&to).unwrap();
        assert!(CookTree::whole(to, None).unwrap().stems.is_empty());
    }

    #[test]
    fn colour_slots_are_srgb_and_data_slots_linear() {
        use ludion_scene::TextureRef;
        let map = |path: &str| Some(TextureRef::new(path));
        let textures = MeshTextures {
            base_color: map("b.png"),
            metallic_roughness: map("m.png"),
            normal: map("n.png"),
            occlusion: map("o.png"),
            emissive: map("e.png"),
        };
        let found: Vec<(bool, &str)> = roles(&textures)
            .into_iter()
            .map(|(space, path)| (space == ColorSpace::Srgb, path))
            .collect();
        assert_eq!(
            found,
            [
                (true, "b.png"),
                (false, "m.png"),
                (false, "n.png"),
                (false, "o.png"),
                (true, "e.png")
            ]
        );
        assert!(roles(&MeshTextures::default()).is_empty());
    }
}
