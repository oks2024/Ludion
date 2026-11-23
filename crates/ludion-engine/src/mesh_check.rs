//! The mesh-asset pass of `scene.check`: every mesh, collider-mesh, mesh-texture, decal-map and
//! grid-legend file a scene names must load. CPU only; each distinct file is opened once and the
//! fault is reported at every entity naming it, through the readers the loader itself calls.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Component, Path, PathBuf};

use ludion_assets::AssetRoot;
use ludion_components::{Decal, Mesh, MeshGrid, MeshSource};
use ludion_core::codes::code;
use ludion_core::diagnostics::Diagnostic;
use ludion_scene::Scene;
use ludion_spatial::{Collider, Shape};

use crate::extract::{MeshSite, mesh_error, suggest_asset};
use crate::manifest;

/// Which reader opens a document.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Doc {
    Mesh,
    Voxel,
    Sculpt,
    Assembly,
    Character,
}

/// One place a scene names a file, with the label and pointer its fault reports under.
struct Site {
    doc: Doc,
    rel: String,
    label: &'static str,
    pointer: String,
    /// Whether the mesh's texture maps are opened too (a collider reads geometry only).
    maps: bool,
}

/// What reading one distinct document found.
#[derive(Default)]
struct Read {
    faults: Vec<Diagnostic>,
    /// The mesh's distinct map paths, content-root relative.
    maps: Vec<String>,
    /// A character's mesh document.
    character_mesh: Option<String>,
    /// The local surface names a mesh document's `surfaces` key lists.
    surfaces: Vec<String>,
}

/// Every `L1101`/`L1102` (or document code) a checked scene's mesh-like files report: `Mesh`
/// shapes, `MeshGrid` legend entries used by its rows, `Collider` mesh shapes, the texture maps a
/// mesh document references and a `Decal`'s maps. A primitive shape names no file, and a legend
/// glyph no row uses is never loaded, so neither is opened here. Check-time only: nothing here
/// touches the sim, and loading the scene reports the same faults on its own.
///
/// `project_root` is the content root the paths resolve against.
///
/// # Example
/// ```
/// use ludion_engine::scene_mesh_asset_diagnostics;
///
/// let root = std::env::temp_dir().join(format!("ludion-mesh-diag-doc-{}", std::process::id()));
/// std::fs::create_dir_all(&root).unwrap();
/// let scene = ludion_scene::check(
///     "format: 1\nentities:\n  - name: rock\n    components:\n      Mesh:\n        shape:\n          asset: rock.mesh.yaml\n",
///     &ludion_engine::builtin_registry(),
/// )
/// .unwrap();
/// let found = scene_mesh_asset_diagnostics(&root, &scene);
/// assert_eq!(found[0].code.as_str(), "L1101");
/// assert_eq!(
///     found[0].path.as_deref(),
///     Some("/entities/0/components/Mesh/shape/asset")
/// );
/// # std::fs::remove_dir_all(&root).ok();
/// ```
pub fn scene_mesh_asset_diagnostics(project_root: &Path, scene: &Scene) -> Vec<Diagnostic> {
    let (sites, decal_maps) = walk(scene);
    if sites.is_empty() && decal_maps.is_empty() {
        return Vec::new();
    }
    let root = AssetRoot::content(project_root);

    // Characters first: their mesh documents join the mesh reads, so each file opens once.
    let character_docs: BTreeSet<(Doc, &str)> = sites
        .iter()
        .filter(|site| site.doc == Doc::Character)
        .map(|site| (site.doc, site.rel.as_str()))
        .collect();
    let characters = read_docs(root, &character_docs);

    let mut distinct: BTreeSet<(Doc, &str)> = sites
        .iter()
        .filter(|site| site.doc != Doc::Character)
        .map(|site| (site.doc, site.rel.as_str()))
        .collect();
    for read in characters.values() {
        if let Some(mesh) = &read.character_mesh {
            distinct.insert((Doc::Mesh, mesh));
        }
    }
    let reads = read_docs(root, &distinct);

    // A map is opened if a mesh that wants its maps (or a decal) names it.
    let mut map_paths: BTreeSet<&str> = decal_maps.iter().map(|(_, rel)| rel.as_str()).collect();
    for site in sites.iter().filter(|site| site.maps) {
        if let Some(read) = mesh_read(site, &characters, &reads) {
            map_paths.extend(read.maps.iter().map(String::as_str));
        }
    }
    let map_paths: Vec<&str> = map_paths.into_iter().collect();
    let png_faults: BTreeMap<&str, Diagnostic> = map_paths
        .iter()
        .copied()
        .zip(ludion_core::par::par_map_chunks(
            &map_paths,
            1,
            |_, chunk| ludion_assets::check_png_header(root, chunk[0]).err(),
        ))
        .filter_map(|(rel, fault)| fault.map(|fault| (rel, fault)))
        .collect();

    let mut manifest = None;
    let mut found = Vec::new();
    for site in &sites {
        let faults = site_faults(site, &characters, &reads, &png_faults);
        for (label, rel, fault) in faults {
            let fault = helped(project_root, fault, &mut manifest);
            found.push(mesh_error(&rel, label, &site.pointer, fault));
        }
    }
    found.extend(undeclared_mesh_surfaces(scene, &sites, &reads));
    for (pointer, rel) in &decal_maps {
        if let Some(fault) = png_faults.get(rel.as_str()) {
            let fault = helped(project_root, fault.clone(), &mut manifest);
            found.push(fault.with_path(pointer.clone()));
        }
    }
    found
}

/// The `L1697` warning for each local surface name a collider mesh's document lists that the
/// scene's `physics.surfaces` does not declare; those triangles fall through to the entity's tag.
fn undeclared_mesh_surfaces(
    scene: &Scene,
    sites: &[Site],
    reads: &BTreeMap<(Doc, &str), Read>,
) -> Vec<Diagnostic> {
    let Some(physics) = &scene.physics else {
        return Vec::new();
    };
    let declared = physics.surfaces.iter().map(String::as_str);
    let mut found = Vec::new();
    for site in sites.iter().filter(|site| site.label == "collider mesh") {
        let Some(read) = reads.get(&(Doc::Mesh, site.rel.as_str())) else {
            continue;
        };
        for name in &read.surfaces {
            if declared.clone().any(|d| d == name) {
                continue;
            }
            let help = match ludion_core::suggest::nearest(name, declared.clone()) {
                Some(near) => format!("nearest match is `{near}`"),
                None => "declare it under `physics.surfaces`".to_string(),
            };
            found.push(
                Diagnostic::new(
                    code::MESH_SURFACE_UNDECLARED,
                    format!(
                        "mesh `{}` tags triangles `{name}`, which `physics.surfaces` does not \
                         declare; they fall through to the entity's surface",
                        site.rel
                    ),
                )
                .with_path(site.pointer.clone())
                .with_help(help),
            );
        }
    }
    found
}

/// The `L1212` a scene's physics-claimed `Collider` mesh files earn when they cannot be opened,
/// each with the `L1101` as `related`: the faults that refuse a load, since the world would
/// otherwise run with no body for them. A scene with no `physics:` block never raises these, and a
/// document that opens but is malformed stays the runtime's `L1212`.
pub(crate) fn missing_collider_meshes(project_root: &Path, scene: &Scene) -> Vec<Diagnostic> {
    if scene.physics.is_none() {
        return Vec::new();
    }
    let mut sites = Vec::new();
    for (index, entity) in scene.entities.iter().enumerate() {
        let Some((_, value)) = entity
            .components
            .iter()
            .find(|(name, _)| name == "Collider")
        else {
            continue;
        };
        if let Ok(collider) = serde_json::from_value::<Collider>(value.clone())
            && collider.is_solver_owned()
            && let Shape::Mesh { path } = collider.shape
            && !path.is_empty()
        {
            sites.push((index, path));
        }
    }
    let root = AssetRoot::content(project_root);
    let mut faults: BTreeMap<&str, Diagnostic> = BTreeMap::new();
    for (_, rel) in &sites {
        if !faults.contains_key(rel.as_str())
            && let Err(fault) = ludion_assets::check_readable(root, rel)
            && fault.code == code::ASSET_FILE_IO
        {
            faults.insert(rel, fault);
        }
    }
    let mut manifest = None;
    let faults: BTreeMap<&str, Diagnostic> = faults
        .into_iter()
        .map(|(rel, fault)| (rel, helped(project_root, fault, &mut manifest)))
        .collect();
    sites
        .iter()
        .filter_map(|(index, rel)| {
            let fault = faults.get(rel.as_str())?;
            let named = crate::skin_bone::named(&scene.entities[*index], *index);
            Some(
                Diagnostic::new(
                    code::COLLIDER_MESH_UNAVAILABLE,
                    format!(
                        "{named}: the mesh `{rel}` cannot be read, so this scene would load with no body for it"
                    ),
                )
                .with_related(fault.clone())
                .with_path(format!(
                    "/entities/{index}/components/Collider/shape/mesh/path"
                )),
            )
        })
        .collect()
}

/// The faults a site reports, each with the label and document path it is reported under (a
/// character's mesh fault names the mesh, not the character).
fn site_faults<'a>(
    site: &Site,
    characters: &'a BTreeMap<(Doc, &str), Read>,
    reads: &'a BTreeMap<(Doc, &str), Read>,
    png_faults: &'a BTreeMap<&str, Diagnostic>,
) -> Vec<(&'static str, String, Diagnostic)> {
    let mut found = Vec::new();
    let mut push_read = |label: &'static str, rel: &str, read: &Read, maps: bool| {
        if !read.faults.is_empty() {
            found.extend(
                read.faults
                    .iter()
                    .map(|d| (label, rel.to_string(), d.clone())),
            );
        } else if maps {
            found.extend(
                read.maps
                    .iter()
                    .filter_map(|path| png_faults.get(path.as_str()))
                    .map(|d| (label, rel.to_string(), d.clone())),
            );
        }
    };
    if site.doc == Doc::Character {
        let Some(character) = characters.get(&(Doc::Character, site.rel.as_str())) else {
            return found;
        };
        if !character.faults.is_empty() {
            push_read("character", &site.rel, character, false);
        } else if let Some((rel, mesh)) = character
            .character_mesh
            .as_deref()
            .and_then(|rel| Some((rel, reads.get(&(Doc::Mesh, rel))?)))
        {
            push_read("character mesh", rel, mesh, site.maps);
        }
    } else if let Some(read) = reads.get(&(site.doc, site.rel.as_str())) {
        push_read(site.label, &site.rel, read, site.maps);
    }
    found
}

/// The mesh document whose maps `site` opens: its own, or the character's mesh.
fn mesh_read<'a>(
    site: &'a Site,
    characters: &'a BTreeMap<(Doc, &str), Read>,
    reads: &'a BTreeMap<(Doc, &str), Read>,
) -> Option<&'a Read> {
    if site.doc == Doc::Character {
        let mesh = characters
            .get(&(Doc::Character, site.rel.as_str()))?
            .character_mesh
            .as_deref()?;
        reads.get(&(Doc::Mesh, mesh))
    } else {
        reads.get(&(site.doc, site.rel.as_str()))
    }
}

/// The sites and decal maps `scene` names, in entity order. Components that do not deserialize
/// are the schema pass's to report.
fn walk(scene: &Scene) -> (Vec<Site>, Vec<(String, String)>) {
    let mut sites = Vec::new();
    let mut decals = Vec::new();
    for (index, entity) in scene.entities.iter().enumerate() {
        for (name, value) in &entity.components {
            match name.as_str() {
                "Mesh" => {
                    if let Ok(mesh) = serde_json::from_value::<Mesh>(value.clone()) {
                        mesh_sites(&mesh.shape, MeshSite::Component, index, &mut sites);
                    }
                }
                "MeshGrid" => {
                    let Ok(grid) = serde_json::from_value::<MeshGrid>(value.clone()) else {
                        continue;
                    };
                    let mut seen = BTreeSet::new();
                    for glyph in grid.rows.iter().flat_map(|row| row.chars()) {
                        let mut buf = [0u8; 4];
                        let Some(mesh) = grid.legend.get(glyph.encode_utf8(&mut buf) as &str)
                        else {
                            continue;
                        };
                        if seen.insert(glyph) {
                            mesh_sites(&mesh.shape, MeshSite::GridCell(glyph), index, &mut sites);
                        }
                    }
                }
                "Collider" => {
                    if let Ok(collider) = serde_json::from_value::<Collider>(value.clone())
                        && let Shape::Mesh { path } = collider.shape
                        && !path.is_empty()
                    {
                        sites.push(Site {
                            doc: Doc::Mesh,
                            rel: path,
                            label: "collider mesh",
                            pointer: format!(
                                "/entities/{index}/components/Collider/shape/mesh/path"
                            ),
                            maps: false,
                        });
                    }
                }
                "Decal" => {
                    let Ok(decal) = serde_json::from_value::<Decal>(value.clone()) else {
                        continue;
                    };
                    for (field, map) in [
                        ("base_color_texture", &decal.base_color_texture),
                        ("normal_texture", &decal.normal_texture),
                        (
                            "metallic_roughness_texture",
                            &decal.metallic_roughness_texture,
                        ),
                    ] {
                        if let Some(map) = map {
                            decals.push((
                                format!("/entities/{index}/components/Decal/{field}/path"),
                                ludion_core::paths::normalize(&map.path),
                            ));
                        }
                    }
                }
                _ => {}
            }
        }
    }
    (sites, decals)
}

fn mesh_sites(shape: &MeshSource, site: MeshSite, index: usize, sites: &mut Vec<Site>) {
    let (doc, rel, label, tail) = match shape {
        MeshSource::Cube | MeshSource::Plane | MeshSource::Sphere => return,
        MeshSource::Asset(path) => (Doc::Mesh, path, "mesh asset", "asset"),
        MeshSource::Voxel(path) => (Doc::Voxel, path, "voxel model", "voxel"),
        MeshSource::Sculpt(path) => (Doc::Sculpt, path, "sculpt model", "sculpt"),
        MeshSource::Character(path) => (Doc::Character, path, "character", "character"),
        MeshSource::Assembly(path) => (Doc::Assembly, path, "assembly", "assembly"),
        MeshSource::AssemblyPart { assembly, .. } => (
            Doc::Assembly,
            assembly,
            "assembly",
            "assembly_part/assembly",
        ),
    };
    sites.push(Site {
        doc,
        rel: rel.clone(),
        label,
        pointer: site.shape_pointer(index, tail),
        maps: doc == Doc::Mesh || doc == Doc::Character,
    });
}

/// Reads each distinct document once, in parallel; results are keyed, so order never shows.
fn read_docs<'a>(
    root: AssetRoot<'_>,
    docs: &BTreeSet<(Doc, &'a str)>,
) -> BTreeMap<(Doc, &'a str), Read> {
    let jobs: Vec<(Doc, &str)> = docs.iter().copied().collect();
    jobs.iter()
        .copied()
        .zip(ludion_core::par::par_map_chunks(&jobs, 1, |_, chunk| {
            read_one(root, chunk[0].0, chunk[0].1)
        }))
        .collect()
}

/// Opens one document with the reader the loader uses, parsing and validating it without meshing.
fn read_one(root: AssetRoot<'_>, doc: Doc, rel: &str) -> Read {
    let mut read = Read::default();
    let outcome = match doc {
        Doc::Mesh => crate::mesh_asset::with_mesh_source(root, rel, ludion_scene::check_mesh)
            .and_then(|(document, _)| {
                read.surfaces = document
                    .surfaces
                    .as_ref()
                    .map(|tags| tags.names.clone())
                    .unwrap_or_default();
                let levels = crate::mesh_asset::read_levels(root, rel, &document)?;
                read.maps = crate::mesh_asset::map_paths(rel, &document)
                    .into_iter()
                    .chain(
                        levels
                            .iter()
                            .flat_map(|level| crate::mesh_asset::map_paths(&level.rel, &level.doc)),
                    )
                    .map(|(path, _, _)| path)
                    .collect::<BTreeSet<_>>()
                    .into_iter()
                    .collect();
                Ok(())
            }),
        Doc::Voxel => crate::voxel_mesh::read_document(root, rel).map(|_| ()),
        Doc::Sculpt => crate::sculpt_mesh::read_document(root, rel).map(|_| ()),
        Doc::Assembly => crate::assembly_mesh::load_documents(root, rel).map(|_| ()),
        Doc::Character => crate::character::mesh_path(root, rel).map(|mesh| {
            read.character_mesh = Some(mesh);
        }),
    };
    if let Err(faults) = outcome {
        read.faults = faults;
    }
    read
}

/// The path an `L1101` from `load_bytes`/`check_png_header` says it could not read: the file that
/// is missing, which for a map or an assembly part is not the document naming it.
fn unread_path(fault: &Diagnostic) -> Option<&str> {
    let rest = fault.message.split("could not read asset `").nth(1)?;
    rest.split('`').next()
}

/// A read failure's help: the derived-tree case, else the reader's own plus any nearest filename.
fn helped(
    project_root: &Path,
    mut fault: Diagnostic,
    manifest: &mut Option<Option<(PathBuf, manifest::Manifest)>>,
) -> Diagnostic {
    let Some(rel) = unread_path(&fault).map(str::to_owned) else {
        return fault;
    };
    if fault.code != code::ASSET_FILE_IO || project_root.join(&rel).is_file() {
        return fault;
    }
    let rel = rel.as_str();
    let loaded = manifest.get_or_insert_with(|| {
        let absolute = std::path::absolute(project_root).ok()?;
        let dir = manifest::find(&absolute)?;
        let loaded = manifest::load(&dir).ok()?;
        Some((dir, loaded))
    });
    if let Some((dir, manifest)) = loaded
        && let Ok(absolute) = std::path::absolute(project_root)
    {
        let target = collapse(&absolute.join(rel));
        let tree = manifest.imports.iter().find(|table| {
            manifest::starts_with_ignore_case(&target, &collapse(&dir.join(&table.to)))
        });
        if let Some(table) = tree {
            fault.help = Some(format!(
                "`{}` is this project's derived `[[import]]` tree, git-ignored: run `ludion sync --project {}`",
                table.to.display(),
                dir.display()
            ));
            return fault;
        }
    }
    if let Some(name) = suggest_asset(AssetRoot::content(project_root), rel) {
        fault = fault.append_help(format!("nearest match is `{name}`"));
    }
    fault
}

/// `path` with `.` and `..` folded away lexically.
fn collapse(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for part in path.components() {
        match part {
            Component::CurDir => {}
            Component::ParentDir => {
                out.pop();
            }
            other => out.push(other.as_os_str()),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const TRIANGLE: &str = "format: 1
kind: mesh
positions:
  - 0
  - 0
  - 0
  - 1
  - 0
  - 0
  - 0
  - 1
  - 0
uvs:
  - 0
  - 0
  - 1
  - 0
  - 0
  - 1
indices:
  - 0
  - 1
  - 2
";

    fn scratch(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("ludion-mesh-check-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("scratch dir");
        dir
    }

    fn scene(entities: &str) -> Scene {
        ludion_scene::check(
            &format!("format: 1\nseed: 1\nentities:\n{entities}"),
            &crate::builtin_registry(),
        )
        .expect("fixture scene")
    }

    fn mesh_entity(shape: &str) -> String {
        format!("  - components:\n      Mesh:\n        shape:\n          {shape}\n")
    }

    fn check(dir: &Path, entities: &str) -> Vec<Diagnostic> {
        scene_mesh_asset_diagnostics(dir, &scene(entities))
    }

    /// The `(code, pointer)` pairs, a document's repeated fault at one site collapsed.
    fn at(found: &[Diagnostic]) -> Vec<(&str, &str)> {
        found
            .iter()
            .map(|d| (d.code.as_str(), d.path.as_deref().unwrap_or("")))
            .collect::<Vec<_>>()
            .into_iter()
            .fold(Vec::new(), |mut kept, pair| {
                if kept.last() != Some(&pair) {
                    kept.push(pair);
                }
                kept
            })
    }

    #[test]
    fn a_missing_mesh_is_l1101_at_its_shape_and_a_bad_document_keeps_its_own_code() {
        let dir = scratch("doc");
        std::fs::write(
            dir.join("broken.mesh.yaml"),
            "format: 1\nkind: mesh\nnot yaml: [",
        )
        .unwrap();
        std::fs::write(
            dir.join("short.mesh.yaml"),
            "format: 1
kind: mesh
positions:
  - 0
  - 0
  - 0
indices:
  - 0
  - 1
  - 2
",
        )
        .unwrap();
        let entities = [
            mesh_entity("asset: gone.mesh.yaml"),
            mesh_entity("asset: broken.mesh.yaml"),
            mesh_entity("asset: short.mesh.yaml"),
        ]
        .concat();
        let found = check(&dir, &entities);
        assert_eq!(
            at(&found),
            [
                ("L1101", "/entities/0/components/Mesh/shape/asset"),
                ("L1001", "/entities/1/components/Mesh/shape/asset"),
                ("L1111", "/entities/2/components/Mesh/shape/asset"),
            ],
            "{found:?}"
        );
        assert!(found[0].message.starts_with("mesh asset `gone.mesh.yaml`"));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn two_entities_on_one_missing_file_report_twice() {
        let dir = scratch("twice");
        let entities = [
            mesh_entity("asset: gone.mesh.yaml"),
            mesh_entity("asset: gone.mesh.yaml"),
        ]
        .concat();
        let found = check(&dir, &entities);
        assert_eq!(found.len(), 2, "{found:?}");
        assert_ne!(found[0].path, found[1].path);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_collider_mesh_a_missing_map_and_a_junk_map_report_at_their_sites() {
        let dir = scratch("maps");
        std::fs::write(
            dir.join("t.mesh.yaml"),
            format!("{TRIANGLE}base_color_texture:\n  path: gone.png\n"),
        )
        .unwrap();
        std::fs::write(
            dir.join("j.mesh.yaml"),
            format!("{TRIANGLE}base_color_texture:\n  path: junk.png\n"),
        )
        .unwrap();
        std::fs::write(dir.join("junk.png"), b"nope").unwrap();
        let entities = [
            "  - components:\n      Collider:\n        shape:\n          mesh:\n            path: floor.mesh.yaml\n        body: static\n".to_string(),
            mesh_entity("asset: t.mesh.yaml"),
            mesh_entity("asset: j.mesh.yaml"),
        ]
        .concat();
        let found = check(&dir, &entities);
        assert_eq!(
            at(&found),
            [
                ("L1101", "/entities/0/components/Collider/shape/mesh/path"),
                ("L1101", "/entities/1/components/Mesh/shape/asset"),
                ("L1102", "/entities/2/components/Mesh/shape/asset"),
            ],
            "{found:?}"
        );
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_collider_does_not_open_the_maps_of_its_mesh() {
        let dir = scratch("collider-maps");
        std::fs::write(
            dir.join("t.mesh.yaml"),
            format!("{TRIANGLE}base_color_texture:\n  path: gone.png\n"),
        )
        .unwrap();
        let found = check(
            &dir,
            "  - components:\n      Collider:\n        shape:\n          mesh:\n            path: t.mesh.yaml\n        body: static\n",
        );
        assert!(found.is_empty(), "{found:?}");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_decal_map_reports_at_its_own_pointer() {
        let dir = scratch("decal");
        let found = check(
            &dir,
            "  - components:\n      Decal:\n        base_color_texture:\n          path: gone.png\n",
        );
        assert_eq!(
            at(&found),
            [(
                "L1101",
                "/entities/0/components/Decal/base_color_texture/path"
            )],
            "{found:?}"
        );
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn only_legend_glyphs_a_row_uses_are_opened() {
        let dir = scratch("grid");
        let found = check(
            &dir,
            "  - components:\n      MeshGrid:\n        cell_size: 1.0\n        legend:\n          A:\n            shape:\n              asset: used.mesh.yaml\n          B:\n            shape:\n              asset: unused.mesh.yaml\n        rows:\n          - AA\n",
        );
        assert_eq!(
            at(&found),
            [(
                "L1101",
                "/entities/0/components/MeshGrid/legend/A/shape/asset"
            )],
            "{found:?}"
        );
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn primitives_open_nothing() {
        let dir = scratch("primitive");
        let entities = [
            mesh_entity("cube"),
            mesh_entity("plane"),
            mesh_entity("sphere"),
        ]
        .concat();
        assert!(check(&dir, &entities).is_empty());
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_missing_voxel_sculpt_assembly_and_character_report_at_their_tails() {
        let dir = scratch("kinds");
        let entities = [
            mesh_entity("voxel: gone.voxel.yaml"),
            mesh_entity("sculpt: gone.sculpt.yaml"),
            mesh_entity("assembly: gone.assembly.yaml"),
            mesh_entity("character: gone.character.yaml"),
            mesh_entity(
                "assembly_part:\n            assembly: gone2.assembly.yaml\n            part: arm",
            ),
        ]
        .concat();
        let found = check(&dir, &entities);
        assert_eq!(
            at(&found),
            [
                ("L1101", "/entities/0/components/Mesh/shape/voxel"),
                ("L1101", "/entities/1/components/Mesh/shape/sculpt"),
                ("L1101", "/entities/2/components/Mesh/shape/assembly"),
                ("L1101", "/entities/3/components/Mesh/shape/character"),
                (
                    "L1101",
                    "/entities/4/components/Mesh/shape/assembly_part/assembly"
                ),
            ],
            "{found:?}"
        );
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_character_whose_mesh_is_missing_reports_the_mesh() {
        let dir = scratch("character");
        std::fs::write(
            dir.join("c.character.yaml"),
            "format: 1\nkind: assembly\nmesh: gone.mesh.yaml\nclips: {}\n",
        )
        .unwrap();
        let found = check(&dir, &mesh_entity("character: c.character.yaml"));
        assert_eq!(
            at(&found),
            [("L1101", "/entities/0/components/Mesh/shape/character")],
            "{found:?}"
        );
        assert!(
            found[0]
                .message
                .starts_with("character mesh `gone.mesh.yaml`")
        );
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_derived_tree_miss_names_sync_and_a_typo_gets_the_nearest_file() {
        let dir = scratch("help");
        std::fs::write(
            dir.join("ludion.toml"),
            "name = \"p\"\ndefault_scene = \"scenes/main.yaml\"\n\n[[import]]\nfrom = \"src/*.glb\"\nto = \"assets/town\"\n",
        )
        .unwrap();
        std::fs::create_dir_all(dir.join("scenes")).unwrap();
        std::fs::create_dir_all(dir.join("assets/town/a")).unwrap();
        std::fs::write(dir.join("scenes/lantern.mesh.yaml"), TRIANGLE).unwrap();
        let found = check(
            &dir.join("scenes"),
            &[
                mesh_entity("asset: ../assets/town/a/x.mesh.yaml"),
                mesh_entity("asset: lantren.mesh.yaml"),
            ]
            .concat(),
        );
        assert_eq!(found.len(), 2, "{found:?}");
        let sync = found[0].help.as_deref().unwrap();
        assert!(
            sync.contains("assets/town") && sync.contains("ludion sync"),
            "{sync}"
        );
        assert!(
            found[1]
                .help
                .as_deref()
                .unwrap()
                .contains("lantern.mesh.yaml")
        );
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_skin_bone_on_a_missing_skinned_mesh_reports_once() {
        let dir = scratch("skin");
        let entities = [
            mesh_entity("asset: gone.mesh.yaml"),
            "  - components:\n      SkinBone:\n        skin:\n          index: 0\n          generation: 0\n        bone: root\n".to_string(),
        ]
        .concat();
        let found = crate::scene_engine_diagnostics(&dir, &scene(&entities));
        let l1101: Vec<_> = found
            .iter()
            .filter(|d| d.code.as_str() == "L1101")
            .collect();
        assert_eq!(l1101.len(), 1, "{found:?}");
        assert_eq!(
            l1101[0].path.as_deref(),
            Some("/entities/0/components/Mesh/shape/asset")
        );
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_missing_map_gets_help_about_the_map_not_its_mesh_document() {
        let dir = scratch("map-help");
        std::fs::write(
            dir.join("t.mesh.yaml"),
            format!("{TRIANGLE}base_color_texture:\n  path: skin.png\n"),
        )
        .unwrap();
        std::fs::write(
            dir.join("skim.png"),
            ludion_assets::encode_png(1, 1, &[1, 2, 3, 255]).unwrap(),
        )
        .unwrap();
        let found = check(&dir, &mesh_entity("asset: t.mesh.yaml"));
        assert_eq!(found.len(), 1, "{found:?}");
        let help = found[0].help.as_deref().unwrap();
        assert!(
            help.contains("skim.png") && !help.contains("t.mesh.yaml"),
            "{help}"
        );

        std::fs::remove_file(dir.join("skim.png")).unwrap();
        let found = check(&dir, &mesh_entity("asset: t.mesh.yaml"));
        let help = found[0].help.as_deref().unwrap();
        assert!(!help.contains("t.mesh.yaml"), "{help}");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_missing_map_under_a_derived_tree_names_sync() {
        let dir = scratch("map-sync");
        std::fs::write(
            dir.join("ludion.toml"),
            "name = \"p\"\ndefault_scene = \"scenes/main.yaml\"\n\n[[import]]\nfrom = \"src/*.glb\"\nto = \"assets/town\"\n",
        )
        .unwrap();
        std::fs::create_dir_all(dir.join("scenes")).unwrap();
        std::fs::create_dir_all(dir.join("assets/town")).unwrap();
        std::fs::write(
            dir.join("assets/town/t.mesh.yaml"),
            format!("{TRIANGLE}base_color_texture:\n  path: t.png\n"),
        )
        .unwrap();
        let found = check(
            &dir.join("scenes"),
            &mesh_entity("asset: ../assets/town/t.mesh.yaml"),
        );
        assert_eq!(found.len(), 1, "{found:?}");
        assert!(found[0].help.as_deref().unwrap().contains("ludion sync"));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_missing_assembly_part_gets_help_about_the_part_not_the_assembly() {
        let dir = scratch("part-help");
        let assembly = include_str!("../../ludion-cli/tests/fixtures/assembly/good.assembly.yaml");
        std::fs::write(dir.join("good.assembly.yaml"), assembly).unwrap();
        std::fs::create_dir_all(dir.join("parts")).unwrap();
        std::fs::write(dir.join("parts/bod.part.yaml"), "x").unwrap();
        let found = check(&dir, &mesh_entity("assembly: good.assembly.yaml"));
        assert!(!found.is_empty(), "{found:?}");
        let help = found[0].help.as_deref().unwrap();
        assert!(help.contains("bod.part.yaml"), "{help}");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_good_scene_is_clean() {
        let dir = scratch("clean");
        std::fs::write(
            dir.join("ok.png"),
            ludion_assets::encode_png(1, 1, &[1, 2, 3, 255]).unwrap(),
        )
        .unwrap();
        std::fs::write(
            dir.join("ok.mesh.yaml"),
            format!("{TRIANGLE}base_color_texture:\n  path: ok.png\n"),
        )
        .unwrap();
        assert!(check(&dir, &mesh_entity("asset: ok.mesh.yaml")).is_empty());
        std::fs::remove_dir_all(&dir).ok();
    }
}
