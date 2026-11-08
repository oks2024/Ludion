//! Generated mesh LOD levels: meshoptimizer's `simplify` over a base mesh document.

use std::collections::{BTreeMap, BTreeSet};

use ludion_core::codes::code;
use ludion_core::diagnostics::Diagnostic;
use ludion_scene::{MAX_MESH_LODS, MeshDocument, MeshLodRef, MeshSkin, check_lod_level};
use meshopt::{SimplifyOptions, VertexDataAdapter, simplify, simplify_sloppy, typed_to_bytes};

use crate::skin::SkinBinding;
use crate::{Inputs, LodSpec, MeshArtifact, State, mesh_reference};

/// The relative error `simplify` may spend reaching the target (a fraction of the mesh's extent).
/// Recorded here because it shapes every generated level.
pub const SIMPLIFY_TARGET_ERROR: f32 = 1e-2;

/// The relative error the sloppy fallback may spend: loose enough that a coarse level reaches its
/// target on a mesh the plain pass cannot collapse.
pub const SLOPPY_TARGET_ERROR: f32 = 1.0;

/// Simplifies `base` to about `ratio` of its triangles (`ratio` in (0, 1), exclusive) and returns the
/// level as a document with no `lods`.
///
/// Vertices at one position are welded for the simplifier (a faceted mesh gives every corner its own
/// normal, which would pin every edge), and `simplify_sloppy` takes over when the plain pass leaves
/// more than 10% above the target. The level is faceted: every output triangle owns three vertices
/// (`indices` is `0..n`), `normals` is absent so the loader computes flat ones, and each corner takes
/// the uv, tangent, emissive, metallic, roughness, skin weights and bone indices of the first base
/// vertex at its position, so a texture may smear along a UV seam. `joints` are the base's. The
/// result may sit above the target when neither pass reaches it, and is empty only for a base with
/// no triangles.
#[must_use]
pub fn simplify_level(base: &MeshDocument, ratio: f64) -> MeshDocument {
    let vertex_count = base.positions.len() / 3;
    let positions: Vec<f32> = base.positions.iter().map(|&p| p as f32).collect();
    let target = ((ratio * base.indices.len() as f64).round() as usize / 3 * 3).max(3);

    let mut first_at: BTreeMap<[u32; 3], u32> = BTreeMap::new();
    let canonical: Vec<u32> = (0..vertex_count)
        .map(|v| {
            let key = [
                positions[v * 3].to_bits(),
                positions[v * 3 + 1].to_bits(),
                positions[v * 3 + 2].to_bits(),
            ];
            *first_at.entry(key).or_insert(v as u32)
        })
        .collect();
    let shadow: Vec<u32> = base
        .indices
        .iter()
        .map(|&index| canonical[index as usize])
        .collect();

    let kept = match VertexDataAdapter::new(typed_to_bytes(&positions), 12, 0) {
        Ok(adapter) => {
            let plain = simplify(
                &shadow,
                &adapter,
                target,
                SIMPLIFY_TARGET_ERROR,
                SimplifyOptions::None,
                None,
            );
            if plain.len() > target + target / 10 {
                simplify_sloppy(&shadow, &adapter, target, SLOPPY_TARGET_ERROR, None)
            } else {
                plain
            }
        }
        Err(_) => shadow,
    };
    let corners: Vec<usize> = kept
        .chunks_exact(3)
        .filter(|tri| tri[0] != tri[1] && tri[1] != tri[2] && tri[0] != tri[2])
        .flatten()
        .map(|&index| index as usize)
        .collect();

    let gather_f64 = |values: &Option<Vec<f64>>, width: usize| {
        values
            .as_ref()
            .map(|values| gather(values, &corners, width))
    };
    MeshDocument {
        positions: gather(&base.positions, &corners, 3),
        normals: None,
        uvs: gather_f64(&base.uvs, 2),
        tangents: gather_f64(&base.tangents, 4),
        emissive: gather_f64(&base.emissive, 1),
        metallic: gather_f64(&base.metallic, 1),
        roughness: gather_f64(&base.roughness, 1),
        skin: base.skin.as_ref().map(|skin| MeshSkin {
            joints: skin.joints.clone(),
            weights: gather(&skin.weights, &corners, 4),
            bone_indices: gather(&skin.bone_indices, &corners, 4),
        }),
        lods: Vec::new(),
        indices: (0..corners.len() as u32).collect(),
        ..base.clone()
    }
}

/// The `width`-wide rows of `values` at `rows`, in that order.
fn gather<T: Copy>(values: &[T], rows: &[usize], width: usize) -> Vec<T> {
    rows.iter()
        .flat_map(|&row| values[row * width..(row + 1) * width].iter().copied())
        .collect()
}

fn chain_error(detail: impl Into<String>, help: impl Into<String>) -> Diagnostic {
    Diagnostic::new(code::MESH_LOD_CHAIN, detail.into()).with_help(help.into())
}

/// Checks the requested levels: at most [`MAX_MESH_LODS`], each `screen_size` finite, above zero and
/// strictly below the previous one, each `ratio` in (0, 1), and either every level or none carrying
/// a `ratio` (one source per chain).
///
/// # Errors
/// `L1395` naming the first offending level.
pub fn validate_specs(specs: &[LodSpec]) -> Result<(), Diagnostic> {
    if specs.len() > MAX_MESH_LODS {
        return Err(chain_error(
            format!(
                "{} LOD levels requested; a mesh chains at most {MAX_MESH_LODS} coarser levels",
                specs.len()
            ),
            "keep the first three `--lod` / `lod` entries",
        ));
    }
    let mut previous = f64::INFINITY;
    for (index, spec) in specs.iter().enumerate() {
        let n = index + 1;
        if !(spec.screen_size.is_finite() && spec.screen_size > 0.0 && spec.screen_size < previous)
        {
            return Err(chain_error(
                format!(
                    "LOD level {n} has screen size {}; screen sizes must be finite, above zero and \
                     strictly decreasing",
                    spec.screen_size
                ),
                "list the levels from the finest to the coarsest, e.g. `--lod 0.5 --lod 0.25`",
            ));
        }
        previous = spec.screen_size;
        if let Some(ratio) = spec.ratio
            && !(ratio > 0.0 && ratio < 1.0)
        {
            return Err(chain_error(
                format!(
                    "LOD level {n} has ratio {ratio}; a ratio is a fraction of the base's triangles in (0, 1)"
                ),
                "use e.g. `--lod 0.25:0.5` to keep half of the triangles below a quarter of the screen",
            ));
        }
    }
    let with_ratio = specs.iter().filter(|spec| spec.ratio.is_some()).count();
    if with_ratio != 0 && with_ratio != specs.len() {
        return Err(chain_error(
            "some LOD levels carry a ratio and some do not",
            "give every level a ratio (generated levels) or none (thresholds for the glTF's authored `_LOD<n>` nodes)",
        ));
    }
    Ok(())
}

/// Adds a generated level per spec to every mesh in `meshes`, after its base, as
/// `meshes/<base>.lod<n>.mesh.yaml`, and lists them in the base's `lods`. A base with no triangles
/// gets no levels. `specs` all carry a ratio ([`validate_specs`] and the caller guarantee it).
///
/// # Errors
/// `L1395` if a level's file name is already taken.
pub(crate) fn generate_levels(
    meshes: Vec<MeshArtifact>,
    specs: &[LodSpec],
) -> Result<Vec<MeshArtifact>, Vec<Diagnostic>> {
    let ratios: Vec<f64> = specs
        .iter()
        .map(|spec| spec.ratio.expect("generation runs only for ratio specs"))
        .collect();
    let taken: BTreeSet<String> = meshes.iter().map(|mesh| mesh.rel_path.clone()).collect();
    let mut out = Vec::new();
    for mut base in meshes {
        let (dir, stem) = split_path(&base.rel_path);
        let mut levels = Vec::new();
        if !base.doc.indices.is_empty() {
            for (index, (spec, ratio)) in specs.iter().zip(&ratios).enumerate() {
                let (file, rel_path) = level_names(&dir, &stem, index + 1);
                if taken.contains(&rel_path) {
                    return Err(vec![name_taken(&rel_path)]);
                }
                let mut doc = simplify_level(&base.doc, *ratio);
                doc.name = Some(file.trim_end_matches(".mesh.yaml").to_string());
                base.doc.lods.push(MeshLodRef {
                    mesh: file,
                    screen_size: spec.screen_size,
                });
                levels.push(MeshArtifact { rel_path, doc });
            }
        }
        out.push(base);
        out.extend(levels);
    }
    Ok(out)
}

fn name_taken(rel_path: &str) -> Diagnostic {
    chain_error(
        format!("the LOD level file `{rel_path}` is already a mesh of this import"),
        "rename the colliding node in the authoring tool",
    )
}

fn split_path(rel_path: &str) -> (String, String) {
    let (dir, file) = rel_path.rsplit_once('/').unwrap_or(("", rel_path));
    (
        dir.to_string(),
        file.trim_end_matches(".mesh.yaml").to_string(),
    )
}

fn level_names(dir: &str, stem: &str, n: usize) -> (String, String) {
    let file = format!("{stem}.lod{n}.mesh.yaml");
    (file.clone(), format!("{dir}/{file}"))
}

/// An authored level's number and the mesh path each of its primitives resolved to.
type LevelPaths = (usize, Vec<Option<String>>);

/// One authored level: glTF node `node` is level `level` (1-based) of the mesh on node `base`.
pub(crate) struct AuthoredLevel {
    pub node: usize,
    pub base: usize,
    pub level: usize,
}

/// Finds the `<stem>_LOD<n>` sibling nodes (`n` in 1..=3) and the node each one is a level of: the
/// node named `<stem>` or `<stem>_LOD0`. Names that merely end in `_LOD<non-number>` are ordinary
/// nodes. Returned sorted by base node, then level.
///
/// # Errors
/// `L1395` for a level with no base, a level or base without a mesh, a level with children, a level
/// above 3, a repeated level, a gap in a base's levels, or a level mesh also worn by another node.
pub(crate) fn authored_levels(nodes: &[gltf::Node]) -> Result<Vec<AuthoredLevel>, Vec<Diagnostic>> {
    let mut found: Vec<(usize, String, usize)> = Vec::new();
    for node in nodes {
        let Some(name) = node.name() else { continue };
        let Some((stem, digits)) = name.rsplit_once("_LOD") else {
            continue;
        };
        let Ok(n) = digits.parse::<usize>() else {
            continue;
        };
        if digits.len() > 1 && digits.starts_with('0') || n == 0 {
            continue;
        }
        if n > MAX_MESH_LODS {
            return Err(vec![chain_error(
                format!(
                    "node `{name}` is LOD level {n}; a mesh chains at most {MAX_MESH_LODS} coarser levels"
                ),
                "merge or drop the coarsest levels in the authoring tool",
            )]);
        }
        found.push((node.index(), stem.to_string(), n));
    }
    let mut levels: Vec<AuthoredLevel> = Vec::new();
    let mut errors = Vec::new();
    for (node_index, stem, n) in &found {
        let node = &nodes[*node_index];
        let label = node.name().unwrap_or_default();
        let base = nodes.iter().find(|candidate| {
            candidate.name() == Some(stem.as_str())
                || candidate
                    .name()
                    .is_some_and(|name| name.strip_prefix(stem.as_str()) == Some("_LOD0"))
        });
        let Some(base) = base else {
            errors.push(chain_error(
                format!("node `{label}` is a LOD level but no node `{stem}` (or `{stem}_LOD0`) exists"),
                "name the finest mesh `<name>` or `<name>_LOD0` beside its `_LOD1` .. `_LOD3` siblings",
            ));
            continue;
        };
        if node.mesh().is_none() || base.mesh().is_none() {
            errors.push(chain_error(
                format!("LOD node `{label}` and its base `{stem}` must both carry a mesh"),
                "give every node of the chain a mesh",
            ));
            continue;
        }
        if node.children().len() > 0 {
            errors.push(chain_error(
                format!("LOD node `{label}` has child nodes, which an import would drop"),
                "move the children under the base node",
            ));
            continue;
        }
        levels.push(AuthoredLevel {
            node: *node_index,
            base: base.index(),
            level: *n,
        });
    }
    levels.sort_by_key(|level| (level.base, level.level));
    let mut chains: BTreeMap<usize, Vec<usize>> = BTreeMap::new();
    for level in &levels {
        chains.entry(level.base).or_default().push(level.level);
    }
    for (base, wanted) in &chains {
        let expected: Vec<usize> = (1..=wanted.len()).collect();
        if *wanted != expected {
            errors.push(chain_error(
                format!(
                    "the LOD levels of `{}` are {wanted:?}; levels must be 1, 2, 3 in order, with no gap or repeat",
                    nodes[*base].name().unwrap_or_default()
                ),
                "add the missing `_LOD<n>` node or renumber the levels",
            ));
        }
    }
    let mut base_meshes: BTreeMap<usize, usize> = BTreeMap::new();
    for base in chains.keys() {
        let mesh = nodes[*base].mesh().expect("checked above").index();
        if let Some(other) = base_meshes.insert(mesh, *base) {
            errors.push(chain_error(
                format!(
                    "nodes `{}` and `{}` share one mesh but each has its own LOD levels",
                    nodes[other].name().unwrap_or("?"),
                    nodes[*base].name().unwrap_or("?")
                ),
                "give each node with LOD levels its own mesh",
            ));
        }
    }
    let level_meshes: BTreeSet<usize> = levels
        .iter()
        .filter_map(|level| nodes[level.node].mesh().map(|mesh| mesh.index()))
        .collect();
    for node in nodes {
        let is_level = levels.iter().any(|level| level.node == node.index());
        if !is_level
            && let Some(mesh) = node.mesh()
            && level_meshes.contains(&mesh.index())
        {
            errors.push(chain_error(
                format!(
                    "node `{}` wears a mesh that is also a LOD level",
                    node.name().unwrap_or("?")
                ),
                "give each LOD level node its own mesh",
            ));
        }
    }
    if errors.is_empty() {
        Ok(levels)
    } else {
        Err(errors)
    }
}

/// Imports the mesh of every authored level node and chains them under their bases: each level
/// primitive `i` becomes `meshes/<base primitive i>.lod<n>.mesh.yaml` and is listed in that base
/// document's `lods`. `specs` supply the screen sizes (one per level, no ratios) or, empty,
/// `0.5^n` does.
///
/// # Errors
/// `L1395` for a threshold count that differs from a chain's levels, a level whose primitive count
/// differs from its base's, or a name collision; `L1396` for a level that does not match its base.
pub(crate) fn attach_authored_levels(
    nodes: &[gltf::Node],
    levels: &[AuthoredLevel],
    bindings: &BTreeMap<usize, SkinBinding>,
    specs: &[LodSpec],
    inputs: &Inputs,
    state: &mut State,
) -> Result<(), Vec<Diagnostic>> {
    // Import each level's primitives first (the shared material path), remembering where they went.
    let mut chains: BTreeMap<usize, Vec<LevelPaths>> = BTreeMap::new();
    for level in levels {
        let mesh = nodes[level.node]
            .mesh()
            .expect("checked by authored_levels");
        let skin = bindings.get(&mesh.index());
        let mut paths = Vec::new();
        for primitive in mesh.primitives() {
            let reference = mesh_reference(&mesh, &primitive, skin, inputs, state)?;
            paths.push(reference.map(|reference| reference.path));
        }
        chains
            .entry(level.base)
            .or_default()
            .push((level.level, paths));
    }
    for (base, per_level) in chains {
        let base_mesh = nodes[base].mesh().expect("checked by authored_levels");
        let base_label = nodes[base].name().unwrap_or("?").to_string();
        if !specs.is_empty() && specs.len() != per_level.len() {
            return Err(vec![chain_error(
                format!(
                    "`{base_label}` has {} authored LOD levels but {} `--lod` / `lod` thresholds were given",
                    per_level.len(),
                    specs.len()
                ),
                "give one threshold per authored level, or none for the default 0.5^n",
            )]);
        }
        for primitive in base_mesh.primitives() {
            let Some(base_path) = state
                .primitives
                .get(&(base_mesh.index(), primitive.index()))
                .cloned()
                .flatten()
                .map(|reference| reference.path)
            else {
                continue;
            };
            let base_at = state
                .meshes
                .iter()
                .position(|mesh| mesh.rel_path == base_path)
                .expect("a resolved primitive has an artifact");
            let (dir, stem) = split_path(&base_path);
            let mut refs: Vec<MeshLodRef> = Vec::new();
            let mut moved: Vec<MeshArtifact> = Vec::new();
            for (level, paths) in &per_level {
                let Some(Some(level_path)) = paths.get(primitive.index()) else {
                    return Err(vec![chain_error(
                        format!(
                            "LOD level {level} of `{base_label}` has no primitive {} to match the base's",
                            primitive.index()
                        ),
                        "give every level the same primitives (one per material) as the base",
                    )]);
                };
                let from = state
                    .meshes
                    .iter()
                    .position(|mesh| &mesh.rel_path == level_path)
                    .expect("a resolved primitive has an artifact");
                let mut artifact = state.meshes.remove(from);
                let (file, rel_path) = level_names(&dir, &stem, *level);
                artifact.rel_path = rel_path;
                artifact.doc.name = Some(file.trim_end_matches(".mesh.yaml").to_string());
                refs.push(MeshLodRef {
                    mesh: file,
                    screen_size: specs
                        .get(level - 1)
                        .map_or_else(|| 0.5_f64.powi(*level as i32), |spec| spec.screen_size),
                });
                moved.push(artifact);
            }
            for (index, level) in moved.iter().enumerate() {
                if let Some(taken) = state
                    .meshes
                    .iter()
                    .find(|mesh| mesh.rel_path == level.rel_path)
                {
                    return Err(vec![name_taken(&taken.rel_path)]);
                }
                let base_at = state
                    .meshes
                    .iter()
                    .position(|mesh| mesh.rel_path == base_path)
                    .unwrap_or(base_at);
                let doc = &mut state.meshes[base_at].doc;
                doc.lods.clone_from(&refs);
                let found = check_lod_level(doc, &level.doc, index);
                if !found.is_empty() {
                    return Err(found);
                }
            }
            state.meshes.extend(moved);
        }
    }
    Ok(())
}
#[cfg(test)]
mod tests {
    use ludion_scene::{JointRest, MeshJoint};

    use super::*;

    /// A `n × n` quad grid (2·n² triangles) with every attribute and a two-joint skin whose weights
    /// and indices vary per vertex, with a mild bump so the simplifier has something to keep.
    fn grid(n: usize) -> MeshDocument {
        let side = n + 1;
        let mut positions = Vec::new();
        let mut uvs = Vec::new();
        let mut weights = Vec::new();
        let mut bones = Vec::new();
        for y in 0..side {
            for x in 0..side {
                let (fx, fy) = (x as f64 / n as f64, y as f64 / n as f64);
                positions.extend([fx, fy, 0.05 * (fx * 6.0).sin() * (fy * 5.0).cos()]);
                uvs.extend([fx, fy]);
                weights.extend([1.0 - fx, fx, 0.0, 0.0]);
                bones.extend([0, 1, 0, 0]);
            }
        }
        let mut indices = Vec::new();
        for y in 0..n {
            for x in 0..n {
                let a = (y * side + x) as u32;
                let (b, c, d) = (a + 1, a + side as u32, a + side as u32 + 1);
                indices.extend([a, b, c, b, d, c]);
            }
        }
        let vertices = side * side;
        let joint = |name: &str, parent| MeshJoint {
            name: name.into(),
            parent,
            rest: JointRest {
                translation: [0.0; 3],
                rotation: [0.0, 0.0, 0.0, 1.0],
                scale: [1.0; 3],
            },
            inverse_bind: [
                1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0,
            ],
        };
        MeshDocument {
            name: Some("grid".into()),
            normals: Some((0..vertices).flat_map(|_| [0.0, 0.0, 1.0]).collect()),
            uvs: Some(uvs),
            tangents: Some((0..vertices).flat_map(|_| [1.0, 0.0, 0.0, 1.0]).collect()),
            emissive: Some(vec![0.0; vertices]),
            metallic: Some(vec![0.25; vertices]),
            roughness: Some((0..vertices).map(|v| v as f64 / vertices as f64).collect()),
            skin: Some(MeshSkin {
                joints: vec![joint("root", None), joint("tip", Some(0))],
                weights,
                bone_indices: bones,
            }),
            positions,
            indices,
            ..MeshDocument::default()
        }
    }

    #[test]
    fn a_level_has_fewer_triangles_and_only_in_range_indices() {
        let base = grid(16);
        let level = simplify_level(&base, 0.25);
        let vertices = (level.positions.len() / 3) as u32;
        assert!(level.indices.len() <= base.indices.len() / 2);
        assert!(!level.indices.is_empty() && level.indices.len().is_multiple_of(3));
        assert!(level.indices.iter().all(|&i| i < vertices));
        assert!(level.lods.is_empty());
        ludion_scene::check_mesh(&ludion_scene::emit_mesh(&level)).expect("a valid level");
    }

    /// The grid with every triangle given its own three vertices (a faceted base).
    fn faceted(base: &MeshDocument) -> MeshDocument {
        let rows: Vec<usize> = base.indices.iter().map(|&i| i as usize).collect();
        let skin = base.skin.as_ref().unwrap();
        MeshDocument {
            positions: gather(&base.positions, &rows, 3),
            normals: None,
            uvs: base.uvs.as_ref().map(|v| gather(v, &rows, 2)),
            tangents: base.tangents.as_ref().map(|v| gather(v, &rows, 4)),
            emissive: base.emissive.as_ref().map(|v| gather(v, &rows, 1)),
            metallic: base.metallic.as_ref().map(|v| gather(v, &rows, 1)),
            roughness: base.roughness.as_ref().map(|v| gather(v, &rows, 1)),
            skin: Some(MeshSkin {
                joints: skin.joints.clone(),
                weights: gather(&skin.weights, &rows, 4),
                bone_indices: gather(&skin.bone_indices, &rows, 4),
            }),
            indices: (0..rows.len() as u32).collect(),
            ..base.clone()
        }
    }

    #[test]
    fn a_faceted_base_reaches_its_target_with_unshared_corners() {
        let base = faceted(&grid(16));
        let level = simplify_level(&base, 0.25);
        assert!(level.indices.len() as f64 <= base.indices.len() as f64 * 0.3);
        assert!(!level.indices.is_empty() && level.indices.len().is_multiple_of(3));
        let expected: Vec<u32> = (0..level.indices.len() as u32).collect();
        assert_eq!(level.indices, expected);
        assert!(level.normals.is_none());
        ludion_scene::check_mesh(&ludion_scene::emit_mesh(&level)).expect("a valid level");
    }

    #[test]
    fn a_skinned_corner_carries_its_canonical_source_vertexs_data() {
        let base = faceted(&grid(16));
        let level = simplify_level(&base, 0.25);
        let (base_skin, skin) = (base.skin.as_ref().unwrap(), level.skin.as_ref().unwrap());
        assert_eq!(skin.joints, base_skin.joints);
        let (base_uvs, uvs) = (base.uvs.as_ref().unwrap(), level.uvs.as_ref().unwrap());
        for (v, position) in level.positions.chunks(3).enumerate() {
            let source = (0..base.positions.len() / 3)
                .find(|&s| base.positions[s * 3..s * 3 + 3] == *position)
                .expect("a corner sits at a base position");
            assert_eq!(
                skin.weights[v * 4..v * 4 + 4],
                base_skin.weights[source * 4..source * 4 + 4]
            );
            assert_eq!(
                skin.bone_indices[v * 4..v * 4 + 4],
                base_skin.bone_indices[source * 4..source * 4 + 4]
            );
            assert_eq!(uvs[v * 2..v * 2 + 2], base_uvs[source * 2..source * 2 + 2]);
        }
    }

    #[test]
    fn the_sloppy_fallback_reaches_a_target_the_plain_pass_cannot() {
        let base = faceted(&grid(16));
        let level = simplify_level(&base, 0.02);
        let target = (0.02 * base.indices.len() as f64) as usize;
        assert!(level.indices.len() <= target + target / 10 + 3 * 4);
    }

    #[test]
    fn a_base_without_triangles_gets_no_levels_and_the_others_still_do() {
        let empty = MeshDocument {
            positions: vec![0.0; 3],
            ..MeshDocument::default()
        };
        let artifact = |path: &str, doc: MeshDocument| MeshArtifact {
            rel_path: path.into(),
            doc,
        };
        let specs = [LodSpec {
            screen_size: 0.25,
            ratio: Some(0.5),
        }];
        let out = generate_levels(
            vec![
                artifact("meshes/a.mesh.yaml", empty),
                artifact("meshes/b.mesh.yaml", grid(8)),
            ],
            &specs,
        )
        .unwrap();
        let paths: Vec<&str> = out.iter().map(|m| m.rel_path.as_str()).collect();
        assert_eq!(
            paths,
            [
                "meshes/a.mesh.yaml",
                "meshes/b.mesh.yaml",
                "meshes/b.lod1.mesh.yaml"
            ]
        );
        assert!(out[0].doc.lods.is_empty());
        assert_eq!(out[1].doc.lods.len(), 1);
    }

    #[test]
    fn two_runs_emit_identical_bytes() {
        let base = grid(16);
        let emit = || ludion_scene::emit_mesh(&simplify_level(&base, 0.1));
        assert_eq!(emit(), emit());
    }
}
