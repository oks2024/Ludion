//! Loading a `Mesh.shape: asset` document into renderer-ready [`MeshData`].
//!
//! A mesh asset is a strict-YAML `meshes/*.mesh.yaml` document ([`ludion_scene::check_mesh`]).
//! This module is the engine-side seam that turns one into the plain vertex data the renderer
//! consumes: read the bytes ([`ludion_assets`]), parse + validate ([`ludion_scene`]), then narrow
//! `f64`→`f32` and — when the document omits `normals` — compute **faceted** normals as a pure
//! function of positions + indices. The renderer never sees YAML; `ludion-scene` never sees a wgpu
//! type; both invariants hold because the decode lives here, engine-side.
//!
//! The texture maps a document references are the one part that needs the filesystem, so they are
//! [`load`]'s half, not [`decode`]'s: a document alone cannot say what its PNGs contain.

use std::path::Path;
use std::sync::Arc;

use ludion_core::codes::code;
use ludion_core::collections::StableMap;
use ludion_core::diagnostics::Diagnostic;
use ludion_core::math::glam::{DMat4, DQuat, DVec3, Mat4, Quat, Vec3};
use ludion_renderer::{MeshData, MeshTexture, MeshTextures, SkinJoint, TextureImage, TextureWrap};
use ludion_scene::{MeshDocument, MeshSkin, TextureRef};

use crate::asset_cache::{SourceFiles, fold_source_hashes};
use crate::load_observer::{LoadObserver, LoadPhase, NoLoadObserver};

/// The tangent frame `(+X, handedness +1)` a vertex wears when the document authors none, mirroring
/// the renderer's own default.
const DEFAULT_TANGENT: [f32; 4] = [1.0, 0.0, 0.0, 1.0];

/// The skin weights `(1, 0, 0, 0)` an unskinned vertex wears, mirroring the renderer's own default.
const DEFAULT_WEIGHTS: [f32; 4] = [1.0, 0.0, 0.0, 0.0];

/// The bone indices `(0, 0, 0, 0)` an unskinned vertex wears, mirroring the renderer's own default.
const DEFAULT_BONES: [u32; 4] = [0, 0, 0, 0];

/// Reads, validates, and decodes the mesh document at `root`/`rel` into [`MeshData`].
///
/// # Errors
/// - `L1101` if the file, or one of its texture PNGs, cannot be read.
/// - `L1102` if a texture PNG is not decodable.
/// - `L1110` if the bytes are not valid UTF-8 text, or the document is structurally malformed.
/// - `L1111` if the document's vertex/index data is inconsistent.
///
/// All diagnostics carry a JSON Pointer into the *mesh document*; the caller ([`extract`](crate::extract))
/// re-points them at the referencing `Mesh` component.
pub(crate) fn load(root: &Path, rel: &str) -> Result<MeshData, Vec<Diagnostic>> {
    load_with_sources(root, rel, &mut NoLoadObserver).map(|(data, _)| data)
}

/// [`load`], additionally reporting **every file it read** — the mesh document first, then each
/// distinct texture PNG, as `root`-relative paths. That list is the mesh's reload dependency set
/// ([`crate::asset_cache`]), and the texture hashes are folded into [`MeshData::hash`] so an edited
/// PNG re-keys the GPU mesh (and its material bind group) even when the document is untouched.
///
/// `observer` is told when each map's decode starts and ends, so a map's time is attributed to the
/// texture phase rather than to the mesh.
pub(crate) fn load_with_sources(
    root: &Path,
    rel: &str,
    observer: &mut dyn LoadObserver,
) -> Result<(MeshData, Vec<String>), Vec<Diagnostic>> {
    let (doc, hash) = read_document(root, rel)?;
    let mut data = decode(&doc, hash);
    let (textures, files) = load_textures(root, rel, &doc, observer)?;
    data.textures = textures;
    let texture_hashes: Vec<u64> = files.iter().map(|(_, hash)| *hash).collect();
    data.hash = fold_source_hashes(hash, &texture_hashes);
    let sources = std::iter::once(rel.to_string())
        .chain(files.into_iter().map(|(path, _)| path))
        .collect();
    Ok((data, sources))
}

/// Reads and validates the mesh document at `root`/`rel`, returning it with its bytes' content hash.
pub(crate) fn read_document(
    root: &Path,
    rel: &str,
) -> Result<(MeshDocument, u64), Vec<Diagnostic>> {
    with_mesh_source(root, rel, ludion_scene::check_mesh)
}

/// Loads `root`/`rel`, decodes it as UTF-8 and hands the text to `read` without copying it, returning
/// what `read` made with the bytes' content hash.
pub(crate) fn with_mesh_source<T>(
    root: &Path,
    rel: &str,
    read: impl FnOnce(&str) -> Result<T, Vec<Diagnostic>>,
) -> Result<(T, u64), Vec<Diagnostic>> {
    let bytes = ludion_assets::load_bytes(root, rel).map_err(|diagnostic| vec![diagnostic])?;
    let source = std::str::from_utf8(&bytes.bytes).map_err(|_| {
        vec![
            Diagnostic::new(
                code::MESH_DOCUMENT_STRUCTURE,
                format!("mesh asset `{rel}` is not valid UTF-8 text"),
            )
            .with_help("a mesh document is a strict-YAML text file (`meshes/*.mesh.yaml`)"),
        ]
    })?;
    Ok((read(source)?, bytes.hash))
}

/// Reads the PNG behind every texture map `doc`, the mesh at `root`/`rel`, references, and binds them
/// as the renderer's [`MeshTextures`]. `None` when the document references no map at all — the
/// renderer's signal to take the untextured arithmetic exactly.
///
/// A reference's `path` resolves **relative to the mesh document's own directory** — the rule a
/// sculpt part's `path` and an assembly's `file:` parts follow — through the same
/// [`ludion_assets`] seam a scene's loose `texture:` PNG takes from its content root. Each distinct
/// path decodes once however many slots name it, and the slots that name it share one `Arc`.
///
/// Mesh maps are never premultiplied: the sprite path's premultiply is a blending convention, and a
/// metallic-roughness or normal map has no alpha to weight by.
///
/// Also returns the files it read — one `(resolved path, content hash)` per distinct authored path,
/// in slot order — for the caller's dependency list and hash fold.
///
/// # Errors
/// `L1101`/`L1102` for a map that cannot be read or decoded — **every** failing map, not just the
/// first, each re-pointed at its own document key so the author sees the whole list in one pass.
fn load_textures(
    root: &Path,
    rel: &str,
    doc: &MeshDocument,
    observer: &mut dyn LoadObserver,
) -> Result<(Option<MeshTextures>, SourceFiles), Vec<Diagnostic>> {
    let maps = &doc.textures;
    // The five slots in the document's canonical order, each paired with its own key.
    let slots: [(&str, Option<&TextureRef>); 5] = [
        ("/base_color_texture", maps.base_color.as_ref()),
        (
            "/metallic_roughness_texture",
            maps.metallic_roughness.as_ref(),
        ),
        ("/normal_texture", maps.normal.as_ref()),
        ("/occlusion_texture", maps.occlusion.as_ref()),
        ("/emissive_texture", maps.emissive.as_ref()),
    ];
    if slots.iter().all(|(_, reference)| reference.is_none()) {
        return Ok((None, Vec::new()));
    }

    // Keyed by the authored path: `None` records a path already reported as failing, so a second
    // slot naming it neither re-reads it nor duplicates its diagnostic.
    let mut images: StableMap<String, Option<Arc<TextureImage>>> = StableMap::new();
    let mut errors = Vec::new();
    let mut files = Vec::new();
    let mut resolved: [Option<MeshTexture>; 5] = Default::default();
    for (index, (key, reference)) in slots.iter().enumerate() {
        let Some(reference) = reference else {
            continue;
        };
        if !images.contains_key(&reference.path) {
            let resolved_path = ludion_core::paths::sibling(rel, &reference.path);
            observer.begin(LoadPhase::Texture);
            let loaded = ludion_assets::load_png(root, &resolved_path);
            observer.end(LoadPhase::Texture);
            let image = match loaded {
                Ok(image) => {
                    files.push((resolved_path, image.hash));
                    Some(Arc::new(TextureImage {
                        width: image.width,
                        height: image.height,
                        rgba: image.rgba,
                        hash: image.hash,
                        // A mesh map is sampled trilinear whatever this says; linear is the honest value.
                        filter_linear: true,
                        premultiplied_on_load: false,
                    }))
                }
                Err(diagnostic) => {
                    errors.push(diagnostic.with_path(*key));
                    None
                }
            };
            images.insert(reference.path.clone(), image);
        }
        resolved[index] = images
            .get(&reference.path)
            .and_then(Option::as_ref)
            .map(|image| MeshTexture {
                image: Arc::clone(image),
                wrap: wrap(reference.wrap),
            });
    }
    if !errors.is_empty() {
        return Err(errors);
    }

    let [base_color, metallic_roughness, normal, occlusion, emissive] = resolved;
    let textures = MeshTextures {
        base_color,
        metallic_roughness,
        normal,
        occlusion,
        emissive,
        // glTF's per-map scalars ride the set, not the map: the identity `1` when the map is absent.
        normal_scale: maps.normal.as_ref().map_or(1.0, |r| r.factor as f32),
        occlusion_strength: maps.occlusion.as_ref().map_or(1.0, |r| r.factor as f32),
    };
    Ok((Some(textures), files))
}

/// Maps a document's sampler wrap mode to the renderer's — the same three glTF modes, 1:1.
fn wrap(mode: ludion_scene::TextureWrap) -> TextureWrap {
    match mode {
        ludion_scene::TextureWrap::Repeat => TextureWrap::Repeat,
        ludion_scene::TextureWrap::Clamp => TextureWrap::Clamp,
        ludion_scene::TextureWrap::Mirror => TextureWrap::Mirror,
    }
}

/// Narrows a validated [`MeshDocument`] to renderer [`MeshData`], computing faceted normals when the
/// document carries none. `hash` is the source bytes' content hash (the GPU mesh-cache key).
///
/// Pure: `textures` stays `None`, since binding a map means reading a PNG — [`load`] fills it.
pub(crate) fn decode(doc: &MeshDocument, hash: u64) -> MeshData {
    let positions: Vec<f32> = doc.positions.iter().map(|&p| p as f32).collect();
    let indices = doc.indices.clone();
    let normals = match &doc.normals {
        Some(normals) => normals.iter().map(|&n| n as f32).collect(),
        None => compute_faceted_normals(&positions, &indices),
    };
    // Asset meshes carry no per-vertex color: fill opaque white so `vertex_color × instance_color`
    // is exactly the instance color (`Mesh.color`) — the whole point of the one-pipeline layout.
    let vertex_count = positions.len() / 3;
    let colors = vec![1.0_f32; vertex_count * 4];
    // Per-vertex material: authored channels narrow f64→f32, unauthored channels take the default
    // material. `authored_material` is set iff the document authors any of the three lists — the
    // extract-time selector between these values and the `Mesh` component's fields.
    let authored_material =
        doc.roughness.is_some() || doc.metallic.is_some() || doc.emissive.is_some();
    let channel = |list: &Option<Vec<f64>>, v: usize, default: f32| {
        list.as_ref().map_or(default, |values| values[v] as f32)
    };
    let mut materials = Vec::with_capacity(vertex_count * 3);
    for v in 0..vertex_count {
        materials.push(channel(&doc.roughness, v, 1.0));
        materials.push(channel(&doc.metallic, v, 0.0));
        materials.push(channel(&doc.emissive, v, 0.0));
    }
    // Texture coordinates narrow f64→f32 when authored; a document with none fills `(0, 0)`.
    let uvs: Vec<f32> = match &doc.uvs {
        Some(uvs) => uvs.iter().map(|&uv| uv as f32).collect(),
        None => vec![0.0; vertex_count * 2],
    };
    // A tangent frame is imported, never derived here: glTF carries the one the authoring tool
    // built, and a normal map is only meaningful against that frame. A document with none takes the
    // default.
    let tangents: Vec<f32> = match &doc.tangents {
        Some(tangents) => tangents.iter().map(|&t| t as f32).collect(),
        None => DEFAULT_TANGENT.repeat(vertex_count),
    };
    // A skin's weights/indices are imported, never derived: they come from the authoring tool's
    // rig. A document with none takes the unskinned default.
    let (weights, bone_indices, skin_joints): (Vec<f32>, Vec<u32>, Vec<SkinJoint>) = match &doc.skin
    {
        Some(skin) => (
            skin.weights.iter().map(|&w| w as f32).collect(),
            skin.bone_indices.clone(),
            decode_skin_joints(skin),
        ),
        None => (
            DEFAULT_WEIGHTS.repeat(vertex_count),
            DEFAULT_BONES.repeat(vertex_count),
            Vec::new(),
        ),
    };
    MeshData {
        positions,
        normals,
        colors,
        materials,
        uvs,
        tangents,
        weights,
        bone_indices,
        skin_joints,
        textures: None,
        authored_material,
        indices,
        hash,
    }
}

/// Narrows a [`MeshSkin`]'s joints to renderer [`SkinJoint`]s: `rest_local` is each joint's own bind
/// TRS (mirrors [`MeshSkin::rest_model_matrices`]'s per-joint `local`, narrowed here instead of
/// composed with a parent), `rest` is [`MeshSkin::rest_palette`] (`rest_model × inverse_bind`,
/// already `f64`-composed).
fn decode_skin_joints(skin: &MeshSkin) -> Vec<SkinJoint> {
    let narrow = |m: [f64; 16]| Mat4::from_cols_array(&m.map(|v| v as f32));
    skin.joints
        .iter()
        .zip(
            skin.rest_palette()
                .expect("a validated skin is parent-first"),
        )
        .map(|(joint, rest)| {
            let rest_local = DMat4::from_scale_rotation_translation(
                DVec3::from_array(joint.rest.scale),
                DQuat::from_array(joint.rest.rotation),
                DVec3::from_array(joint.rest.translation),
            );
            SkinJoint {
                name: joint.name.clone(),
                parent: joint.parent,
                rest_local: narrow(rest_local.to_cols_array()),
                rest_translation: Vec3::from_array(joint.rest.translation.map(|v| v as f32)),
                rest_rotation: Quat::from_array(joint.rest.rotation.map(|v| v as f32)),
                rest_scale: Vec3::from_array(joint.rest.scale.map(|v| v as f32)),
                inverse_bind: narrow(joint.inverse_bind),
                rest: narrow(rest),
            }
        })
        .collect()
}

/// Computes flat faceted normals for an indexed triangle mesh — a pure function of `positions`
/// (3 floats per vertex) and `indices` (a triangle list).
///
/// Each triangle's area-weighted geometric normal is accumulated into its three vertices, then each
/// vertex normal is normalized. For faceted low-poly content (vertices duplicated per face, the way
/// the built-in primitives and hand-authored crystal meshes are wound) every vertex belongs to a
/// single face, so its normal is exactly that face's normal — the flat HD-2D look. Deterministic:
/// the accumulation order is index order, and every op is plain `f32` arithmetic.
///
/// Assumes already-validated input (`positions.len()` a multiple of 3, indices in range) — the
/// caller runs [`ludion_scene::check_mesh`] first.
pub(crate) fn compute_faceted_normals(positions: &[f32], indices: &[u32]) -> Vec<f32> {
    let vertex_count = positions.len() / 3;
    let point = |i: u32| {
        let base = i as usize * 3;
        Vec3::new(positions[base], positions[base + 1], positions[base + 2])
    };

    let mut accum = vec![Vec3::ZERO; vertex_count];
    for tri in indices.chunks_exact(3) {
        let (a, b, c) = (point(tri[0]), point(tri[1]), point(tri[2]));
        let face = (b - a).cross(c - a); // area-weighted (un-normalized) face normal
        for &index in tri {
            accum[index as usize] += face;
        }
    }

    let mut normals = vec![0.0_f32; positions.len()];
    for (vertex, n) in accum.iter().enumerate() {
        let unit = n.normalize_or_zero();
        normals[vertex * 3] = unit.x;
        normals[vertex * 3 + 1] = unit.y;
        normals[vertex * 3 + 2] = unit.z;
    }
    normals
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A single CCW triangle in the XY plane has a +Z faceted normal on all three vertices.
    #[test]
    fn single_triangle_normal_is_the_face_normal() {
        let positions = vec![0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0, 0.0];
        let indices = vec![0, 1, 2];
        let normals = compute_faceted_normals(&positions, &indices);
        assert_eq!(normals, vec![0.0, 0.0, 1.0, 0.0, 0.0, 1.0, 0.0, 0.0, 1.0]);
    }

    /// Winding flips the normal: a CW triangle points −Z.
    #[test]
    fn winding_flips_the_normal() {
        let positions = vec![0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 1.0, 0.0, 0.0];
        let indices = vec![0, 1, 2];
        let normals = compute_faceted_normals(&positions, &indices);
        assert_eq!(&normals[0..3], &[0.0, 0.0, -1.0]);
    }

    /// A vertex shared by two coplanar faces keeps the shared plane's normal, and every normal is
    /// unit length. Faceted content that duplicates vertices per face never hits this averaging path.
    #[test]
    fn shared_vertices_average_and_stay_unit_length() {
        // A quad in the XY plane as two triangles sharing an edge — all faces face +Z.
        let positions = vec![
            0.0, 0.0, 0.0, // 0
            1.0, 0.0, 0.0, // 1
            1.0, 1.0, 0.0, // 2
            0.0, 1.0, 0.0, // 3
        ];
        let indices = vec![0, 1, 2, 0, 2, 3];
        let normals = compute_faceted_normals(&positions, &indices);
        for chunk in normals.chunks_exact(3) {
            let n = Vec3::new(chunk[0], chunk[1], chunk[2]);
            assert!((n.length() - 1.0).abs() < 1e-6, "normals are unit length");
            assert!(
                (n.z - 1.0).abs() < 1e-6,
                "coplanar faces keep the +Z normal"
            );
        }
    }

    /// `decode` narrows f64→f32 and fills normals from the pure function when the document has none.
    #[test]
    fn decode_computes_normals_when_absent() {
        let doc = MeshDocument {
            positions: vec![0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0, 0.0],
            normals: None,
            indices: vec![0, 1, 2],
            ..Default::default()
        };
        let data = decode(&doc, 0x1234);
        assert_eq!(data.hash, 0x1234);
        assert_eq!(
            data.normals,
            vec![0.0, 0.0, 1.0, 0.0, 0.0, 1.0, 0.0, 0.0, 1.0]
        );
    }

    /// `decode` always fills opaque-white per-vertex colors, 4 per vertex — the invariant that lets
    /// asset meshes share the one mesh pipeline with `vertex_color × instance_color` exact.
    #[test]
    fn decode_fills_opaque_white_colors() {
        let doc = MeshDocument {
            positions: vec![0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0, 0.0],
            normals: None,
            indices: vec![0, 1, 2],
            ..Default::default()
        };
        let data = decode(&doc, 0);
        assert_eq!(data.colors.len(), data.positions.len() / 3 * 4);
        assert!(data.colors.iter().all(|&c| c == 1.0));
    }

    /// `decode` copies an authored tangent frame verbatim (narrowed) — a frame is imported from the
    /// authoring tool, never rebuilt — and falls back to the `(+X, +1)` default when there is none.
    #[test]
    fn decode_keeps_authored_tangents_and_defaults_otherwise() {
        let authored: Vec<f32> = vec![
            0.0, 1.0, 0.0, 1.0, //
            0.0, 1.0, 0.0, -1.0, //
            0.0, 0.0, 1.0, 1.0,
        ];
        let doc = MeshDocument {
            positions: vec![0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0, 0.0],
            tangents: Some(authored.iter().map(|&t| f64::from(t)).collect()),
            indices: vec![0, 1, 2],
            ..Default::default()
        };
        assert_eq!(decode(&doc, 0).tangents, authored);

        let untangented = MeshDocument {
            tangents: None,
            ..doc
        };
        assert_eq!(decode(&untangented, 0).tangents, DEFAULT_TANGENT.repeat(3));
    }

    /// `decode` preserves authored normals verbatim (narrowed), never recomputing.
    #[test]
    fn decode_keeps_authored_normals() {
        let doc = MeshDocument {
            positions: vec![0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0, 0.0],
            normals: Some(vec![1.0, 0.0, 0.0, 1.0, 0.0, 0.0, 1.0, 0.0, 0.0]),
            indices: vec![0, 1, 2],
            ..Default::default()
        };
        let data = decode(&doc, 0);
        assert_eq!(
            data.normals,
            vec![1.0, 0.0, 0.0, 1.0, 0.0, 0.0, 1.0, 0.0, 0.0]
        );
    }

    /// `decode` narrows a skinned document's weights/indices verbatim; an unskinned document takes
    /// the unskinned defaults.
    #[test]
    fn decode_narrows_skin_weights_and_defaults_when_unskinned() {
        use ludion_scene::{JointRest, MeshJoint, MeshSkin};

        let joint = MeshJoint {
            name: "root".to_string(),
            parent: None,
            rest: JointRest {
                translation: [0.0, 0.0, 0.0],
                rotation: [0.0, 0.0, 0.0, 1.0],
                scale: [1.0, 1.0, 1.0],
            },
            inverse_bind: {
                let mut m = [0.0; 16];
                for i in 0..4 {
                    m[i * 4 + i] = 1.0;
                }
                m
            },
        };
        let doc = MeshDocument {
            positions: vec![0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0, 0.0],
            skin: Some(MeshSkin {
                joints: vec![joint],
                weights: vec![
                    1.0, 0.0, 0.0, 0.0, // v0
                    0.5, 0.5, 0.0, 0.0, // v1
                    0.0, 0.0, 1.0, 0.0, // v2
                ],
                bone_indices: vec![
                    0, 0, 0, 0, // v0
                    0, 0, 0, 0, // v1
                    0, 0, 0, 0, // v2
                ],
            }),
            indices: vec![0, 1, 2],
            ..Default::default()
        };
        let data = decode(&doc, 0);
        assert_eq!(
            data.weights,
            vec![
                1.0, 0.0, 0.0, 0.0, //
                0.5, 0.5, 0.0, 0.0, //
                0.0, 0.0, 1.0, 0.0,
            ]
        );
        assert_eq!(data.bone_indices, vec![0; 12]);
        assert_eq!(data.skin_joints.len(), 1);
        assert_eq!(data.skin_joints[0].rest, Mat4::IDENTITY);

        let unskinned = MeshDocument { skin: None, ..doc };
        let unskinned_data = decode(&unskinned, 0);
        assert_eq!(unskinned_data.weights, DEFAULT_WEIGHTS.repeat(3));
        assert_eq!(unskinned_data.bone_indices, DEFAULT_BONES.repeat(3));
        assert!(unskinned_data.skin_joints.is_empty());
    }

    /// A 1×1 RGBA PNG's bytes, `red` distinguishing one file's content hash from another's.
    fn png(red: u8) -> Vec<u8> {
        ludion_assets::encode_png(1, 1, &[red, 20, 30, 255]).expect("a 1x1 RGBA PNG encodes")
    }

    /// A one-triangle mesh document carrying `uvs` and `tangents` — both required before a texture,
    /// respectively a normal map, validates — plus whatever texture keys `textures` spells.
    fn textured_doc(textures: &str) -> String {
        let list = |key: &str, values: &[f64]| {
            let mut out = format!("{key}:\n");
            for value in values {
                out.push_str(&format!("  - {value:?}\n"));
            }
            out
        };
        format!(
            "format: 1\nkind: mesh\n{}{}{}{textures}indices:\n  - 0\n  - 1\n  - 2\n",
            list("positions", &[0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0, 0.0]),
            list("uvs", &[0.0, 0.0, 1.0, 0.0, 0.0, 1.0]),
            list(
                "tangents",
                &[
                    1.0, 0.0, 0.0, 1.0, //
                    1.0, 0.0, 0.0, -1.0, //
                    1.0, 0.0, 0.0, 1.0,
                ],
            ),
        )
    }

    /// Lays out a project root the way a glTF import does — `meshes/x.mesh.yaml` beside a sibling
    /// `textures/` folder, which is why a reference reads `../textures/…` — and returns it.
    fn project(test: &str, doc: &str, textures: &[(&str, Vec<u8>)]) -> std::path::PathBuf {
        let dir =
            std::env::temp_dir().join(format!("ludion-mesh-maps-{}-{test}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("meshes")).unwrap();
        std::fs::create_dir_all(dir.join("textures")).unwrap();
        std::fs::write(dir.join("meshes/x.mesh.yaml"), doc).unwrap();
        for (name, bytes) in textures {
            std::fs::write(dir.join("textures").join(name), bytes).unwrap();
        }
        dir
    }

    /// `load` resolves each reference against the *mesh document's own directory*, decodes it once
    /// per distinct path, and carries the wrap mode and both glTF scalars onto the renderer set.
    #[test]
    fn load_binds_texture_maps_sharing_one_image_per_path() {
        let keys = "base_color_texture:\n  path: ../textures/t.png\n\
                    normal_texture:\n  path: ../textures/n.png\n  wrap: clamp\n  scale: 0.75\n\
                    occlusion_texture:\n  path: ../textures/t.png\n  strength: 0.5\n";
        let base = png(10);
        let dir = project(
            "bind",
            &textured_doc(keys),
            &[("t.png", base.clone()), ("n.png", png(200))],
        );

        let data = load(&dir, "meshes/x.mesh.yaml").expect("a textured mesh loads");
        let textures = data.textures.expect("the document references maps");
        let base_color = textures
            .base_color
            .as_ref()
            .expect("the base-color map is bound");
        assert_eq!(base_color.image.hash, ludion_assets::content_hash(&base));
        assert_eq!((base_color.image.width, base_color.image.height), (1, 1));
        assert!(base_color.image.filter_linear);
        assert_eq!(base_color.wrap, TextureWrap::Repeat);
        let normal = textures.normal.as_ref().expect("the normal map is bound");
        assert_eq!(normal.wrap, TextureWrap::Clamp);
        assert_eq!(textures.normal_scale, 0.75);
        assert_eq!(textures.occlusion_strength, 0.5);
        assert!(textures.metallic_roughness.is_none());
        assert!(textures.emissive.is_none());
        // Two slots naming one path decode once and share the `Arc`; a different path does not.
        let occlusion = textures
            .occlusion
            .as_ref()
            .expect("the occlusion map is bound");
        assert!(Arc::ptr_eq(&base_color.image, &occlusion.image));
        assert!(!Arc::ptr_eq(&base_color.image, &normal.image));
        // The authored vertex data the maps are sampled against rides through.
        assert_eq!(data.uvs, vec![0.0, 0.0, 1.0, 0.0, 0.0, 1.0]);
        assert_eq!(data.tangents[7], -1.0);

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Every unreadable map is reported, not just the first, each at its own document key — and a
    /// path named twice is reported once.
    #[test]
    fn load_reports_every_missing_texture_once() {
        let keys = "base_color_texture:\n  path: ../textures/missing.png\n\
                    normal_texture:\n  path: ../textures/absent.png\n\
                    occlusion_texture:\n  path: ../textures/missing.png\n";
        let dir = project("missing", &textured_doc(keys), &[]);

        let errors = load(&dir, "meshes/x.mesh.yaml").expect_err("a missing map fails the load");
        assert_eq!(errors.len(), 2, "one diagnostic per distinct failing path");
        assert!(errors.iter().all(|d| d.code == code::ASSET_FILE_IO));
        assert_eq!(errors[0].path.as_deref(), Some("/base_color_texture"));
        assert!(errors[0].message.contains("textures/missing.png"));
        assert_eq!(errors[1].path.as_deref(), Some("/normal_texture"));

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A document referencing no map binds none: `textures: None` is the renderer's signal to take
    /// the untextured arithmetic exactly.
    #[test]
    fn load_of_an_untextured_mesh_binds_no_textures() {
        let dir = project("untextured", &textured_doc(""), &[]);
        let data = load(&dir, "meshes/x.mesh.yaml").expect("an untextured mesh loads");
        assert!(data.textures.is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
