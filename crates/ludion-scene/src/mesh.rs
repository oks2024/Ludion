//! Mesh asset documents: `meshes/*.mesh.yaml` — one strict-YAML document per mesh.
//!
//! A mesh document is a flat, textual description of a triangle mesh: `positions` (3 floats per
//! vertex), optional `normals` (3 floats per vertex — omitted, `ludion-engine` computes faceted
//! normals at load, a pure function of `positions` + `indices`; this crate only stores/validates),
//! optional `uvs` (2 floats per vertex, stored losslessly), optional `tangents` (4 floats per
//! vertex, geometry a normal map needs), optional per-vertex material lists
//! `emissive`, `metallic`, `roughness` (1 float per vertex each — `roughness`/`metallic` are
//! perceptual values in `[0, 1]`, `emissive` is an HDR intensity multiplier on the vertex color,
//! `>= 0`), up to five optional texture references ([`MeshTextures`]), the optional skin keys
//! `joints`/`weights`/`bone_indices` ([`MeshSkin`], all three or none), and a required `indices`
//! triangle list. A generated mesh also carries an optional
//! `origin:` provenance block ([`crate::Origin`], owned by [`crate::generation`]), emitted straight
//! after `name` and only when present — so every mesh document written before it existed stays
//! byte-identical; a mesh a tool owns every byte of carries `generated_from:` beside `name`, the
//! same write-only marker a generated scene, atlas and voxel model carry. Every optional key follows
//! that same rule: a document that authors none of them
//! emits exactly the bytes it did before the key existed.
//!
//! This module owns mesh-document *structure* (shape/type checking, `L1110`) and *data
//! consistency* (counts, index range, finiteness; `L1111`), the skin keys' structure and data
//! (`L1190`, `L1191`), plus the non-fatal vertex-count style
//! budget (`L1112`). It reuses the crate's shared strict-YAML layer ([`crate::yaml`]) — there is
//! exactly one YAML parser in this crate — and the same canonical-writer primitives
//! ([`crate::scalar`]) the scene emitter uses, so a mesh document is ryu-float, byte-idempotent
//! canonical text for the same reasons a scene document is.
//!
//! Unlike scene documents (validated against the component registry's per-component JSON
//! Schemas), a mesh document's shape is fixed and small enough to check directly — there is no
//! registry involved here.

use std::collections::BTreeMap;

use ludion_core::codes::code;
use ludion_core::diagnostics::{Diagnostic, DiagnosticCode};
use ludion_core::math::glam::{DMat4, DQuat, DVec3};
use ludion_core::paths::sibling_refusal;
use ludion_core::suggest;
use serde_json::Value;

use crate::envelope;
use crate::generation::{self, Origin};
use crate::scalar::{format_float, scalar_string};
use crate::yaml::{self, MapEntry, Node, NodeValue};

/// The mesh document format version this crate reads and writes.
pub const MESH_FORMAT_VERSION: u64 = 1;

/// Vertex count above which [`crate::mesh_budget_warning`] warns (`L1112`) — a style guardrail,
/// not a hard limit. The built-in sphere primitive is ~600 vertices; 10,000 is already suspicious
/// for the HD-2D low-poly style.
pub const MESH_VERTEX_BUDGET: usize = 10_000;

/// The most joints one skinned mesh may carry — the size of the GPU bone palette a skinned draw
/// binds.
pub const MESH_JOINT_LIMIT: usize = 256;

/// How far a vertex's four skin weights may sum from 1 (`L1191`).
pub const SKIN_WEIGHT_SUM_TOLERANCE: f64 = 1e-4;

/// How far a rest rotation's length may differ from 1 (`L1191`).
const REST_ROTATION_TOLERANCE: f64 = 1e-3;

const JOINT_KEYS: [&str; 4] = ["name", "parent", "rest", "inverse_bind"];
const REST_KEYS: [&str; 3] = ["t", "r", "s"];

const ALPHA_MODE_NAMES: [&str; 2] = ["opaque", "mask"];

/// The default `alpha_cutoff` of a `mask` mesh (glTF's `alphaCutoff` default).
pub const DEFAULT_ALPHA_CUTOFF: f64 = 0.5;

/// How a mesh's coverage is resolved: drawn whole, or cut out where its alpha falls under a cutoff.
#[derive(Clone, Copy, Debug, PartialEq, Default)]
pub enum AlphaMode {
    /// Every fragment is drawn.
    #[default]
    Opaque,
    /// A fragment is discarded when the base-colour map's alpha times `Mesh.color.a` is under
    /// `cutoff`, a value in `[0, 1]`.
    Mask {
        /// The coverage threshold.
        cutoff: f64,
    },
}

const MESH_KEYS: [&str; 28] = [
    "format",
    "kind",
    "name",
    "generated_from",
    "origin",
    "positions",
    "normals",
    "uvs",
    "tangents",
    "emissive",
    "metallic",
    "roughness",
    "base_color_texture",
    "metallic_roughness_texture",
    "normal_texture",
    "occlusion_texture",
    "emissive_texture",
    "alpha_mode",
    "alpha_cutoff",
    "double_sided",
    "rim",
    "joints",
    "weights",
    "bone_indices",
    "lods",
    "surfaces",
    "triangle_surfaces",
    "indices",
];

const LOD_KEYS: [&str; 2] = ["mesh", "screen_size"];

/// The most coarser levels a mesh may chain after its own (four levels in all).
pub const MAX_MESH_LODS: usize = crate::lod::MAX_LODS;

/// A parsed, structurally valid mesh asset document.
///
/// Held as plain data (`f64` positions, not the renderer's `f32` — text round-trips losslessly;
/// the renderer narrows to `f32` when it builds GPU vertex buffers), so this crate never
/// needs to depend on a GPU type (`ludion-assets` never sees a wgpu type — `CLAUDE.md`).
#[derive(Clone, Debug, PartialEq, Default)]
pub struct MeshDocument {
    /// Optional human-readable mesh name.
    pub name: Option<String>,
    /// Optional derivation: the document this mesh was derived from, relative to this file. Present
    /// only on a mesh some tool owns every byte of, which is therefore write-only — the rule a
    /// generated scene, atlas and voxel model already follow. `None` for a hand-authored or
    /// imported mesh.
    pub generated_from: Option<String>,
    /// Optional provenance: which recipe, seed and workflow generated this mesh. `None` for an
    /// authored or imported mesh, which is every mesh that was not generated.
    pub origin: Option<Origin>,
    /// Flat vertex positions, 3 floats per vertex (`x0, y0, z0, x1, y1, z1, ...`).
    pub positions: Vec<f64>,
    /// Flat per-vertex normals, 3 floats per vertex. `None` means "compute faceted normals at
    /// load" — a pure function of `positions` + `indices`.
    pub normals: Option<Vec<f64>>,
    /// Flat per-vertex UVs, 2 floats per vertex, stored losslessly.
    pub uvs: Option<Vec<f64>>,
    /// Flat per-vertex tangents, 4 floats per vertex: `xyz` plus the handedness `w`, exactly `-1`
    /// or `1`. Written by the importer — imported from glTF `TANGENT`, or generated when a normal
    /// map needs one. `None` means "no normal map asked for tangents", the case for every mesh
    /// authored before textures existed.
    pub tangents: Option<Vec<f64>>,
    /// Flat per-vertex emissive intensity, 1 float per vertex. An HDR intensity multiplier on
    /// the vertex's own color (never a separate RGB), `>= 0`. `None` means "no authored
    /// emissive" — the default material value (`0.0`) applies.
    pub emissive: Option<Vec<f64>>,
    /// Flat per-vertex metallic, 1 float per vertex, perceptual `[0, 1]`. `None` means "no
    /// authored metallic" — the default material value (`0.0`) applies.
    pub metallic: Option<Vec<f64>>,
    /// Flat per-vertex roughness, 1 float per vertex, perceptual `[0, 1]`. `None` means "no
    /// authored roughness" — the default material value (`1.0`) applies.
    pub roughness: Option<Vec<f64>>,
    /// The texture maps this mesh references, each optional. Empty for every palette, primitive and
    /// untextured imported mesh.
    pub textures: MeshTextures,
    /// Whether the mesh is cut out by alpha. Render-only; a `Mask` cutoff tests the base-colour
    /// map's alpha times `Mesh.color.a`.
    pub alpha_mode: AlphaMode,
    /// Whether the mesh draws unculled, back faces shaded with a flipped normal. Render-only.
    pub double_sided: bool,
    /// One rim highlight for the whole mesh. It wins over the entity's `Mesh.rim` and any
    /// inherited one. Ranges are `L1111`.
    pub rim: Option<crate::rim::DocRim>,
    /// The skeleton and per-vertex skin weights of a skinned mesh. `None` for every unskinned mesh,
    /// which is every mesh authored before skins existed.
    pub skin: Option<MeshSkin>,
    /// Coarser levels of detail, finest first; empty for every mesh without a chain. The base is
    /// level 0.
    pub lods: Vec<MeshLodRef>,
    /// Triangle list: flat vertex indices, 3 per triangle.
    pub indices: Vec<u32>,
}

/// One coarser level of a mesh's chain: a level document and the screen size under which it draws.
#[derive(Clone, Debug, PartialEq)]
pub struct MeshLodRef {
    /// The level's `*.mesh.yaml`, **relative to the base document's own directory**.
    pub mesh: String,
    /// The level takes over when the mesh's bounding sphere covers less than this fraction of the
    /// viewport height. Finite, above zero, strictly decreasing along the chain.
    pub screen_size: f64,
}

/// The skeleton a skinned mesh binds to and its per-vertex influences — glTF's skin, flattened to
/// the text the document holds. Present only with all three keys (`joints`, `weights`,
/// `bone_indices`).
#[derive(Clone, Debug, PartialEq)]
pub struct MeshSkin {
    /// The joints in glTF order; a parent always precedes its children.
    pub joints: Vec<MeshJoint>,
    /// Flat per-vertex weights, 4 per vertex.
    pub weights: Vec<f64>,
    /// Flat per-vertex joint indices into `joints`, 4 per vertex.
    pub bone_indices: Vec<u32>,
}

/// One joint of a [`MeshSkin`].
#[derive(Clone, Debug, PartialEq)]
pub struct MeshJoint {
    /// The joint's name, unique within the skin.
    pub name: String,
    /// The parent's index in `joints`; `None` for a root (several roots are legal).
    pub parent: Option<u32>,
    /// The joint's local transform at bind.
    pub rest: JointRest,
    /// The inverse bind matrix, column-major.
    pub inverse_bind: [f64; 16],
}

/// A joint's local translation, rotation and scale at bind.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct JointRest {
    /// Local translation (`t`).
    pub translation: [f64; 3],
    /// Local rotation (`r`), a unit quaternion `x, y, z, w`.
    pub rotation: [f64; 4],
    /// Local scale (`s`).
    pub scale: [f64; 3],
}

/// Each joint's rest transform in model space, column-major (see [`MeshSkin::rest_model_matrices`]);
/// `joints` is parent-first. `None` unless it is, as every skin from `parse`/`check_mesh` is.
#[must_use]
pub fn rest_model_matrices(joints: &[MeshJoint]) -> Option<Vec<[f64; 16]>> {
    let mut model: Vec<DMat4> = Vec::with_capacity(joints.len());
    for joint in joints {
        let rest = &joint.rest;
        let local = DMat4::from_scale_rotation_translation(
            DVec3::from_array(rest.scale),
            DQuat::from_array(rest.rotation),
            DVec3::from_array(rest.translation),
        );
        let matrix = match joint.parent {
            Some(parent) => *model.get(parent as usize)? * local,
            None => local,
        };
        model.push(matrix);
    }
    Some(model.iter().map(DMat4::to_cols_array).collect())
}

impl MeshSkin {
    /// Each joint's rest transform in model space, column-major: a root's is its own TRS and a
    /// child's is its parent's times its own (`joints` is parent-first). A joint's head is columns
    /// 12–14. Pure `f64` arithmetic, so every platform gets the same bits. `None` unless `joints`
    /// is parent-first, as every skin from `parse`/`check_mesh` is.
    #[must_use]
    pub fn rest_model_matrices(&self) -> Option<Vec<[f64; 16]>> {
        rest_model_matrices(&self.joints)
    }

    /// Each joint's skinning matrix at bind, `rest_model × inverse_bind`, identity up to the
    /// export's precision; column-major. `None` unless `joints` is parent-first.
    #[must_use]
    pub fn rest_palette(&self) -> Option<Vec<[f64; 16]>> {
        Some(
            self.rest_model_matrices()?
                .iter()
                .zip(&self.joints)
                .map(|(model, joint)| {
                    (DMat4::from_cols_array(model) * DMat4::from_cols_array(&joint.inverse_bind))
                        .to_cols_array()
                })
                .collect(),
        )
    }
}

/// The optional texture maps a mesh document references — glTF's metallic-roughness set. Every
/// map is `None` for a document that authors none; the loader binds per-draw only what is present.
#[derive(Clone, Debug, PartialEq, Default)]
pub struct MeshTextures {
    /// Albedo, sRGB; multiplies the vertex color.
    pub base_color: Option<TextureRef>,
    /// glTF's packing: `G` = roughness, `B` = metallic, both linear.
    pub metallic_roughness: Option<TextureRef>,
    /// Tangent-space normals; the reference's `factor` is glTF's `scale`, multiplying the sampled
    /// `xy`. Needs `tangents`.
    pub normal: Option<TextureRef>,
    /// Ambient occlusion in `R`; the reference's `factor` is glTF's `strength`, lerping toward `1`.
    pub occlusion: Option<TextureRef>,
    /// Emission, sRGB; multiplies the per-vertex `emissive` intensity.
    pub emissive: Option<TextureRef>,
}

/// One texture reference: a PNG path relative to the mesh document's own directory, its sampler
/// wrap mode, and the glTF per-map scalar (`normalTexture.scale` / `occlusionTexture.strength`;
/// `1` for the other three maps, where it is never written).
#[derive(Clone, Debug, PartialEq)]
pub struct TextureRef {
    /// The PNG this map reads, **relative to the mesh document's own directory** — the same rule a
    /// sculpt part's `path` follows ([`crate::SculptShape::Voxel`]). Non-empty.
    pub path: String,
    /// The sampler wrap mode for both axes.
    pub wrap: TextureWrap,
    /// The map's own scalar: `scale` on a normal map, `strength` on an occlusion map, `1` (and
    /// unwritten) everywhere else.
    pub factor: f64,
}

impl TextureRef {
    /// A reference to `path` with glTF's defaults: `repeat` wrap, factor `1`.
    pub fn new(path: impl Into<String>) -> Self {
        Self {
            path: path.into(),
            wrap: TextureWrap::Repeat,
            factor: 1.0,
        }
    }
}

/// A texture's sampler wrap mode, both axes at once — glTF's per-axis `wrapS`/`wrapT` split is
/// rejected at import when the two disagree, so one mode is all a reference can hold.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum TextureWrap {
    /// glTF's default, and the only mode the canonical writer omits.
    #[default]
    Repeat,
    /// Clamp to the edge texel.
    Clamp,
    /// Mirror on every repeat.
    Mirror,
}

impl TextureWrap {
    /// The mode's document spelling (`repeat`, `clamp`, `mirror`).
    pub fn name(self) -> &'static str {
        match self {
            TextureWrap::Repeat => "repeat",
            TextureWrap::Clamp => "clamp",
            TextureWrap::Mirror => "mirror",
        }
    }
}

/// Every wrap spelling a document may use, in [`TextureWrap`] order.
const WRAP_NAMES: [&str; 3] = ["repeat", "clamp", "mirror"];

/// One of the five texture slots, resolved for a walk that must visit them in canonical order: the
/// document key, the key its per-map scalar spells (`None` for a map that has none), and the
/// reference itself when the document authors one.
struct TextureSlot<'a> {
    key: &'static str,
    scalar_key: Option<&'static str>,
    reference: Option<&'a TextureRef>,
}

impl MeshTextures {
    /// The five slots in canonical emit order — the one place that order is written down, shared by
    /// [`emit`] and [`validate`].
    fn slots(&self) -> [TextureSlot<'_>; 5] {
        [
            TextureSlot {
                key: "base_color_texture",
                scalar_key: None,
                reference: self.base_color.as_ref(),
            },
            TextureSlot {
                key: "metallic_roughness_texture",
                scalar_key: None,
                reference: self.metallic_roughness.as_ref(),
            },
            TextureSlot {
                key: "normal_texture",
                scalar_key: Some("scale"),
                reference: self.normal.as_ref(),
            },
            TextureSlot {
                key: "occlusion_texture",
                scalar_key: Some("strength"),
                reference: self.occlusion.as_ref(),
            },
            TextureSlot {
                key: "emissive_texture",
                scalar_key: None,
                reference: self.emissive.as_ref(),
            },
        ]
    }
}

impl MeshDocument {
    /// The vertex count implied by `positions` (3 floats per vertex).
    pub fn vertex_count(&self) -> usize {
        self.positions.len() / 3
    }

    /// The triangle count implied by `indices` (3 indices per triangle).
    pub fn triangle_count(&self) -> usize {
        self.indices.len() / 3
    }
}

/// The most local surface names one mesh document may list.
const MAX_MESH_SURFACES: usize = 256;

/// Per-triangle surface tags that travel beside a [`MeshDocument`] (the `surfaces` and
/// `triangle_surfaces` keys): local names, and `[count, local index]` runs in triangle order.
#[derive(Clone, Debug, PartialEq)]
pub struct MeshSurfaces {
    /// The local surface names, matched to the scene's `physics.surfaces` at load.
    pub names: Vec<String>,
    /// Run-length `[count, local index]` pairs over the triangles, in triangle order.
    pub runs: Vec<[u32; 2]>,
}

impl MeshSurfaces {
    /// The local name index of each triangle, the runs expanded.
    pub fn per_triangle(&self) -> Vec<u16> {
        let total: usize = self.runs.iter().map(|run| run[0] as usize).sum();
        let mut out = Vec::with_capacity(total);
        for [count, index] in &self.runs {
            out.extend(std::iter::repeat_n(*index as u16, *count as usize));
        }
        out
    }
}

/// Parses strict-subset YAML into a [`MeshDocument`], checking structure/shape only (`L1110`).
pub(crate) fn parse(source: &str) -> Result<MeshDocument, Vec<Diagnostic>> {
    parse_with_surfaces(source).map(|(doc, _)| doc)
}

/// [`parse`], also returning the `surfaces`/`triangle_surfaces` tags when the document has them.
pub(crate) fn parse_with_surfaces(
    source: &str,
) -> Result<(MeshDocument, Option<MeshSurfaces>), Vec<Diagnostic>> {
    let root = yaml::parse_document(source).map_err(|diagnostic| vec![diagnostic])?;
    let Some(root) = root else {
        return Err(vec![
            Diagnostic::new(
                code::MESH_DOCUMENT_STRUCTURE,
                "the mesh document is empty; a mesh needs at least `format: 1`, `kind: mesh`, \
                 `positions`, and `indices`",
            )
            .with_path("")
            .with_help(
                "a minimal mesh is:\n  format: 1\n  kind: mesh\n  positions: [...]\n  \
                 indices: [...]",
            ),
        ]);
    };
    let NodeValue::Mapping(entries) = root.value else {
        return Err(vec![
            Diagnostic::new(
                code::MESH_DOCUMENT_STRUCTURE,
                format!(
                    "the mesh document root must be a mapping, found {} (line {})",
                    root.value.type_phrase(),
                    root.line
                ),
            )
            .with_path("")
            .with_help("start the file with the top-level keys `format:` and `kind: mesh`"),
        ]);
    };

    let mut diagnostics = Vec::new();
    let mut doc = MeshDocument::default();
    let mut format_seen = false;
    let mut kind_seen = false;
    let mut positions_seen = false;
    let mut indices_seen = false;
    let mut skin_first: Option<&str> = None;
    let mut joints_seen = false;
    let mut weights_seen = false;
    let mut bone_indices_seen = false;
    let mut joints: Option<Vec<MeshJoint>> = None;
    let mut weights: Option<Vec<f64>> = None;
    let mut bone_indices: Option<Vec<u32>> = None;
    let mut lods: Vec<MeshLodRef> = Vec::new();
    let mut alpha_mode_seen = false;
    let mut alpha_mask = false;
    let mut alpha_cutoff: Option<f64> = None;
    let mut alpha_cutoff_line = 0;
    let mut surface_names: Option<Vec<String>> = None;
    let mut surface_runs: Option<Vec<[u32; 2]>> = None;
    let mut surface_key_seen: Option<&str> = None;

    for entry in &entries {
        match entry.key.as_str() {
            "format" => {
                format_seen = true;
                envelope::check_envelope(
                    entry,
                    code::MESH_DOCUMENT_STRUCTURE,
                    code::MESH_DOCUMENT_STRUCTURE,
                    MESH_FORMAT_VERSION,
                    "the mesh document",
                    "mesh",
                    "mesh",
                    &mut diagnostics,
                );
            }
            "kind" => {
                kind_seen = true;
                envelope::check_kind(
                    entry,
                    code::MESH_DOCUMENT_STRUCTURE,
                    "mesh",
                    "a mesh asset document",
                    &mut diagnostics,
                );
            }
            "name" => {
                doc.name = envelope::expect_string(
                    &entry.node,
                    "/name",
                    "the mesh name",
                    code::MESH_DOCUMENT_STRUCTURE,
                    &mut diagnostics,
                );
            }
            "generated_from" => {
                doc.generated_from = envelope::expect_string(
                    &entry.node,
                    "/generated_from",
                    "the document this mesh was derived from",
                    code::MESH_DOCUMENT_STRUCTURE,
                    &mut diagnostics,
                );
            }
            "origin" => {
                doc.origin = generation::parse_origin(
                    &entry.node,
                    "/origin",
                    code::MESH_DOCUMENT_STRUCTURE,
                    &mut diagnostics,
                );
            }
            "positions" => {
                positions_seen = true;
                if let Some(values) = expect_float_list(
                    &entry.node,
                    "/positions",
                    "positions",
                    code::MESH_DOCUMENT_STRUCTURE,
                    &mut diagnostics,
                ) {
                    doc.positions = values;
                }
            }
            "normals" => {
                doc.normals = expect_float_list(
                    &entry.node,
                    "/normals",
                    "normals",
                    code::MESH_DOCUMENT_STRUCTURE,
                    &mut diagnostics,
                );
            }
            "uvs" => {
                doc.uvs = expect_float_list(
                    &entry.node,
                    "/uvs",
                    "uvs",
                    code::MESH_DOCUMENT_STRUCTURE,
                    &mut diagnostics,
                );
            }
            "tangents" => {
                doc.tangents = expect_float_list(
                    &entry.node,
                    "/tangents",
                    "tangents",
                    code::MESH_DOCUMENT_STRUCTURE,
                    &mut diagnostics,
                );
            }
            "emissive" => {
                doc.emissive = expect_float_list(
                    &entry.node,
                    "/emissive",
                    "emissive",
                    code::MESH_DOCUMENT_STRUCTURE,
                    &mut diagnostics,
                );
            }
            "metallic" => {
                doc.metallic = expect_float_list(
                    &entry.node,
                    "/metallic",
                    "metallic",
                    code::MESH_DOCUMENT_STRUCTURE,
                    &mut diagnostics,
                );
            }
            "roughness" => {
                doc.roughness = expect_float_list(
                    &entry.node,
                    "/roughness",
                    "roughness",
                    code::MESH_DOCUMENT_STRUCTURE,
                    &mut diagnostics,
                );
            }
            "base_color_texture" => {
                doc.textures.base_color =
                    expect_texture_ref(&entry.node, "base_color_texture", None, &mut diagnostics);
            }
            "metallic_roughness_texture" => {
                doc.textures.metallic_roughness = expect_texture_ref(
                    &entry.node,
                    "metallic_roughness_texture",
                    None,
                    &mut diagnostics,
                );
            }
            "normal_texture" => {
                doc.textures.normal = expect_texture_ref(
                    &entry.node,
                    "normal_texture",
                    Some("scale"),
                    &mut diagnostics,
                );
            }
            "occlusion_texture" => {
                doc.textures.occlusion = expect_texture_ref(
                    &entry.node,
                    "occlusion_texture",
                    Some("strength"),
                    &mut diagnostics,
                );
            }
            "emissive_texture" => {
                doc.textures.emissive =
                    expect_texture_ref(&entry.node, "emissive_texture", None, &mut diagnostics);
            }
            "alpha_mode" => {
                alpha_mode_seen = true;
                if let Some(mask) = expect_alpha_mode(&entry.node, &mut diagnostics) {
                    alpha_mask = mask;
                }
            }
            "alpha_cutoff" => {
                alpha_cutoff = expect_number(
                    &entry.node,
                    "/alpha_cutoff",
                    "alpha_cutoff",
                    &mut diagnostics,
                );
                alpha_cutoff_line = entry.node.line;
            }
            "double_sided" => {
                if let NodeValue::Scalar(Value::Bool(flag)) = &entry.node.value {
                    doc.double_sided = *flag;
                } else {
                    diagnostics.push(
                        Diagnostic::new(
                            code::MESH_DOCUMENT_STRUCTURE,
                            format!(
                                "`double_sided` must be true or false, found {} (line {})",
                                entry.node.value.type_phrase(),
                                entry.node.line
                            ),
                        )
                        .with_path("/double_sided")
                        .with_help("write `double_sided: true`, or omit it"),
                    );
                }
            }
            "rim" => {
                doc.rim = crate::rim::parse_rim(
                    &entry.node,
                    "/rim",
                    code::MESH_DOCUMENT_STRUCTURE,
                    &mut diagnostics,
                );
            }
            "joints" => {
                skin_first.get_or_insert("/joints");
                joints = parse_joints(&entry.node, &mut diagnostics);
                joints_seen = true;
            }
            "weights" => {
                skin_first.get_or_insert("/weights");
                weights = expect_float_list(
                    &entry.node,
                    "/weights",
                    "weights",
                    code::MESH_SKIN_STRUCTURE,
                    &mut diagnostics,
                );
                weights_seen = true;
            }
            "bone_indices" => {
                skin_first.get_or_insert("/bone_indices");
                bone_indices = expect_index_list(
                    &entry.node,
                    "/bone_indices",
                    "bone_indices",
                    code::MESH_SKIN_STRUCTURE,
                    &mut diagnostics,
                );
                bone_indices_seen = true;
            }
            "lods" => {
                if let Some(parsed) = parse_lods(&entry.node, &mut diagnostics) {
                    lods = parsed;
                }
            }
            "surfaces" => {
                surface_key_seen.get_or_insert("/surfaces");
                surface_names = parse_surface_names(&entry.node, &mut diagnostics);
            }
            "triangle_surfaces" => {
                surface_key_seen.get_or_insert("/triangle_surfaces");
                surface_runs = parse_surface_runs(&entry.node, &mut diagnostics);
            }
            "indices" => {
                indices_seen = true;
                if let Some(values) = expect_index_list(
                    &entry.node,
                    "/indices",
                    "indices",
                    code::MESH_DOCUMENT_STRUCTURE,
                    &mut diagnostics,
                ) {
                    doc.indices = values;
                }
            }
            unknown => diagnostics.push(envelope::unknown_key(
                unknown,
                entry,
                "",
                "a mesh document",
                MESH_KEYS,
                code::MESH_DOCUMENT_STRUCTURE,
            )),
        }
    }

    if !format_seen {
        diagnostics.push(missing_key("format", "add `format: 1` as the first line"));
    }
    if !kind_seen {
        diagnostics.push(missing_key("kind", "add `kind: mesh`"));
    }
    if !positions_seen {
        diagnostics.push(missing_key(
            "positions",
            "add a flat `positions` list, 3 floats per vertex",
        ));
    }
    if !indices_seen {
        diagnostics.push(missing_key(
            "indices",
            "add a flat `indices` list — a triangle list, 3 indices per triangle",
        ));
    }

    if alpha_mask {
        doc.alpha_mode = AlphaMode::Mask {
            cutoff: alpha_cutoff.unwrap_or(DEFAULT_ALPHA_CUTOFF),
        };
    } else if alpha_cutoff.is_some() {
        let how = if alpha_mode_seen {
            "is `opaque`"
        } else {
            "is absent"
        };
        diagnostics.push(
            Diagnostic::new(
                code::MESH_DATA_CONSISTENCY,
                format!(
                    "`alpha_cutoff` only means something with `alpha_mode: mask`, but \
                     `alpha_mode` {how} (line {alpha_cutoff_line})"
                ),
            )
            .with_path("/alpha_cutoff")
            .with_help("add `alpha_mode: mask`, or remove `alpha_cutoff`"),
        );
    }

    doc.lods = lods;

    if let Some(first) = skin_first {
        let absent: Vec<&str> = [
            ("joints", joints_seen),
            ("weights", weights_seen),
            ("bone_indices", bone_indices_seen),
        ]
        .into_iter()
        .filter_map(|(key, seen)| (!seen).then_some(key))
        .collect();
        if absent.is_empty() {
            if let (Some(joints), Some(weights), Some(bone_indices)) =
                (joints, weights, bone_indices)
            {
                doc.skin = Some(MeshSkin {
                    joints,
                    weights,
                    bone_indices,
                });
            }
        } else {
            let list = absent
                .iter()
                .map(|key| format!("`{key}`"))
                .collect::<Vec<_>>()
                .join(", ");
            diagnostics.push(
                Diagnostic::new(
                    code::MESH_SKIN_STRUCTURE,
                    format!(
                        "a skinned mesh needs all three of `joints`, `weights` and \
                         `bone_indices`; missing {list}"
                    ),
                )
                .with_path(first)
                .with_help(format!(
                    "add {list}, or remove the skin keys for an unskinned mesh"
                )),
            );
        }
    }

    let has_names = entries.iter().any(|entry| entry.key == "surfaces");
    let has_runs = entries.iter().any(|entry| entry.key == "triangle_surfaces");
    if has_names != has_runs {
        let (present, absent) = if has_names {
            ("surfaces", "triangle_surfaces")
        } else {
            ("triangle_surfaces", "surfaces")
        };
        diagnostics.push(
            Diagnostic::new(
                code::MESH_SURFACES_INVALID,
                format!("`{present}` needs `{absent}` beside it; the two keys go together"),
            )
            .with_path(surface_key_seen.unwrap_or("/surfaces"))
            .with_help(format!("add `{absent}`, or remove `{present}`")),
        );
    }
    let surfaces = match (surface_names, surface_runs) {
        (Some(names), Some(runs)) => Some(MeshSurfaces { names, runs }),
        _ => None,
    };

    if diagnostics.is_empty() {
        Ok((doc, surfaces))
    } else {
        Err(diagnostics)
    }
}

/// Parses `surfaces`: a sequence of local surface names.
fn parse_surface_names(node: &Node, diagnostics: &mut Vec<Diagnostic>) -> Option<Vec<String>> {
    let items = expect_sequence(node, "/surfaces", "surfaces", diagnostics)?;
    let before = diagnostics.len();
    let names: Vec<String> = items
        .iter()
        .enumerate()
        .filter_map(|(index, item)| {
            envelope::expect_string(
                item,
                &format!("/surfaces/{index}"),
                "a surface name",
                code::MESH_SURFACES_INVALID,
                diagnostics,
            )
        })
        .collect();
    (diagnostics.len() == before).then_some(names)
}

/// Parses `triangle_surfaces`: a sequence of `[count, local index]` integer pairs.
fn parse_surface_runs(node: &Node, diagnostics: &mut Vec<Diagnostic>) -> Option<Vec<[u32; 2]>> {
    let items = expect_sequence(node, "/triangle_surfaces", "triangle_surfaces", diagnostics)?;
    let before = diagnostics.len();
    let mut runs = Vec::with_capacity(items.len());
    for (index, item) in items.iter().enumerate() {
        let pointer = format!("/triangle_surfaces/{index}");
        let pair = match &item.value {
            NodeValue::Sequence(pair) if pair.len() == 2 => {
                let value = |node: &Node| match &node.value {
                    NodeValue::Scalar(Value::Number(n)) => n
                        .as_u64()
                        .filter(|v| *v <= u64::from(u32::MAX))
                        .map(|v| v as u32),
                    _ => None,
                };
                value(&pair[0]).zip(value(&pair[1]))
            }
            _ => None,
        };
        match pair {
            Some((count, local)) => runs.push([count, local]),
            None => diagnostics.push(
                Diagnostic::new(
                    code::MESH_SURFACES_INVALID,
                    format!(
                        "`triangle_surfaces[{index}]` must be a pair of non-negative integers, \
                         `[count, local index]` (line {})",
                        item.line
                    ),
                )
                .with_path(pointer)
                .with_help("write each run as two lines: `- - 12` then `  - 0`"),
            ),
        }
    }
    (diagnostics.len() == before).then_some(runs)
}

fn missing_key(key: &str, help: &str) -> Diagnostic {
    envelope::missing_key(key, help, code::MESH_DOCUMENT_STRUCTURE, "mesh document")
}

/// Parses a flat sequence of numbers (`positions`, `normals`, `uvs`). Returns `None` on any shape
/// error (a diagnostic is already pushed for each); the caller decides what "absent" means for
/// its field (required vs. optional).
fn expect_float_list(
    node: &Node,
    pointer: &str,
    what: &str,
    code: DiagnosticCode,
    diagnostics: &mut Vec<Diagnostic>,
) -> Option<Vec<f64>> {
    let NodeValue::Sequence(items) = &node.value else {
        diagnostics.push(
            Diagnostic::new(
                code,
                format!(
                    "`{what}` must be a flat sequence of numbers, found {} (line {})",
                    node.value.type_phrase(),
                    node.line
                ),
            )
            .with_path(pointer)
            .with_help(format!(
                "write `{what}` as one number per line, e.g.\n  {what}:\n    - 0.0\n    - 1.0"
            )),
        );
        return None;
    };
    let mut out = Vec::with_capacity(items.len());
    let mut ok = true;
    for (index, item) in items.iter().enumerate() {
        match &item.value {
            NodeValue::Scalar(Value::Number(n)) => out.push(crate::fields::number(n)),
            other => {
                diagnostics.push(
                    Diagnostic::new(
                        code,
                        format!(
                            "`{what}[{index}]` must be a number, found {} (line {})",
                            other.type_phrase(),
                            item.line
                        ),
                    )
                    .with_path(format!("{pointer}/{index}")),
                );
                ok = false;
            }
        }
    }
    ok.then_some(out)
}

/// Parses `indices`: a flat sequence of non-negative integers that fit in `u32`.
fn expect_index_list(
    node: &Node,
    pointer: &str,
    what: &str,
    code: DiagnosticCode,
    diagnostics: &mut Vec<Diagnostic>,
) -> Option<Vec<u32>> {
    let NodeValue::Sequence(items) = &node.value else {
        diagnostics.push(
            Diagnostic::new(
                code,
                format!(
                    "`{what}` must be a flat sequence of non-negative integers, found {} \
                     (line {})",
                    node.value.type_phrase(),
                    node.line
                ),
            )
            .with_path(pointer)
            .with_help(format!(
                "write `{what}` as one integer per line, e.g.\n  {what}:\n    - 0"
            )),
        );
        return None;
    };
    let mut out = Vec::with_capacity(items.len());
    let mut ok = true;
    for (index, item) in items.iter().enumerate() {
        let valid = match &item.value {
            NodeValue::Scalar(Value::Number(n)) => n.as_u64().filter(|v| *v <= u64::from(u32::MAX)),
            _ => None,
        };
        match valid {
            Some(v) => out.push(v as u32),
            None => {
                diagnostics.push(
                    Diagnostic::new(
                        code,
                        format!(
                            "`{what}[{index}]` must be a non-negative integer that fits in 32 \
                             bits, found {} (line {})",
                            item.value.type_phrase(),
                            item.line
                        ),
                    )
                    .with_path(format!("{pointer}/{index}")),
                );
                ok = false;
            }
        }
    }
    ok.then_some(out)
}

/// The skin joints of a mesh document, without parsing its vertex data; `None` when it has no skin.
///
/// The fast path slices the column-0 `joints:` block out of the source and parses only that. Any
/// irregularity — no or repeated `joints:`, a slice that does not parse, a joint diagnostic — falls
/// back to the full [`check_mesh`](crate::check_mesh), so every error is the full parse's, with true
/// line numbers.
///
/// # Errors
/// Everything `check_mesh` reports when the fast path cannot answer.
pub fn read_mesh_joints(source: &str) -> Result<Option<Vec<MeshJoint>>, Vec<Diagnostic>> {
    if let Some(joints) = fast_joints(source) {
        return Ok(Some(joints));
    }
    crate::check_mesh(source).map(|doc| doc.skin.map(|skin| skin.joints))
}

fn fast_joints(source: &str) -> Option<Vec<MeshJoint>> {
    let mut start = None;
    let mut is_mesh = false;
    for (offset, line) in line_offsets(source) {
        let line = line.trim_end();
        if line == "joints:" {
            if start.replace(offset).is_some() {
                return None;
            }
        } else if line == "kind: mesh" {
            is_mesh = true;
        }
    }
    let start = start.filter(|_| is_mesh)?;
    let rest = &source[start..];
    let mut len = rest.len();
    for (offset, line) in line_offsets(rest).skip(1) {
        let continues = line.trim().is_empty() || line.starts_with([' ', '-', '#']);
        if !continues {
            len = offset;
            break;
        }
    }
    let node = yaml::parse_document(&rest[..len]).ok()??;
    let NodeValue::Mapping(entries) = &node.value else {
        return None;
    };
    let entry = entries.iter().find(|entry| entry.key == "joints")?;
    let mut diagnostics = Vec::new();
    let joints = parse_joints(&entry.node, &mut diagnostics)?;
    check_joints(&joints, &mut diagnostics);
    diagnostics.is_empty().then_some(joints)
}

/// Each line of `text` with its byte offset.
fn line_offsets(text: &str) -> impl Iterator<Item = (usize, &str)> {
    let mut offset = 0;
    text.split('\n').map(move |line| {
        let at = offset;
        offset += line.len() + 1;
        (at, line)
    })
}

/// Parses `lods`: a sequence of `{mesh, screen_size}` mappings. Returns `None` if any entry has a
/// shape error (a diagnostic is already pushed for each).
fn parse_lods(node: &Node, diagnostics: &mut Vec<Diagnostic>) -> Option<Vec<MeshLodRef>> {
    let items = expect_sequence(node, "/lods", "lods", diagnostics)?;
    let before = diagnostics.len();
    let lods: Vec<MeshLodRef> = items
        .iter()
        .enumerate()
        .filter_map(|(index, item)| parse_lod(item, index, diagnostics))
        .collect();
    (diagnostics.len() == before).then_some(lods)
}

fn parse_lod(node: &Node, index: usize, diagnostics: &mut Vec<Diagnostic>) -> Option<MeshLodRef> {
    let pointer = format!("/lods/{index}");
    let entries = expect_mapping(node, &pointer, "a level of `lods`", diagnostics)?;
    let before = diagnostics.len();
    let mut mesh = None;
    let mut screen_size = None;
    for entry in entries {
        let field = format!("{pointer}/{}", entry.key);
        match entry.key.as_str() {
            "mesh" => {
                mesh = envelope::expect_string(
                    &entry.node,
                    &field,
                    "a level's `mesh` path",
                    code::MESH_LOD_CHAIN,
                    diagnostics,
                );
            }
            "screen_size" => {
                screen_size = expect_number(&entry.node, &field, "screen_size", diagnostics);
            }
            unknown => diagnostics.push(envelope::unknown_key(
                unknown,
                entry,
                &pointer,
                "a level of `lods`",
                LOD_KEYS,
                code::MESH_LOD_CHAIN,
            )),
        }
    }
    for key in LOD_KEYS {
        if !entries.iter().any(|entry| entry.key == key) {
            diagnostics.push(
                Diagnostic::new(
                    code::MESH_LOD_CHAIN,
                    format!("a level of `lods` is missing its required `{key}`"),
                )
                .with_path(pointer.clone())
                .with_help(format!("add `{key}:` to it")),
            );
        }
    }
    if diagnostics.len() != before {
        return None;
    }
    Some(MeshLodRef {
        mesh: mesh?,
        screen_size: screen_size?,
    })
}

/// Parses `joints`: a sequence of joint mappings. Returns `None` if any joint has a shape error (a
/// diagnostic is already pushed for each) — the document is rejected then anyway.
fn parse_joints(node: &Node, diagnostics: &mut Vec<Diagnostic>) -> Option<Vec<MeshJoint>> {
    let items = expect_sequence(node, "/joints", "joints", diagnostics)?;
    let before = diagnostics.len();
    let joints: Vec<MeshJoint> = items
        .iter()
        .enumerate()
        .filter_map(|(index, item)| parse_joint(item, index, diagnostics))
        .collect();
    (diagnostics.len() == before).then_some(joints)
}

fn parse_joint(node: &Node, index: usize, diagnostics: &mut Vec<Diagnostic>) -> Option<MeshJoint> {
    let pointer = format!("/joints/{index}");
    let entries = expect_mapping(node, &pointer, "a joint", diagnostics)?;
    let before = diagnostics.len();
    let mut name = None;
    let mut parent = None;
    let mut rest = None;
    let mut inverse_bind = None;
    for entry in entries {
        let field = format!("{pointer}/{}", entry.key);
        match entry.key.as_str() {
            "name" => {
                name = envelope::expect_string(
                    &entry.node,
                    &field,
                    "a joint `name`",
                    code::MESH_SKIN_STRUCTURE,
                    diagnostics,
                );
            }
            "parent" => parent = expect_joint_parent(&entry.node, &field, diagnostics),
            "rest" => rest = parse_rest(&entry.node, &field, diagnostics),
            "inverse_bind" => {
                inverse_bind = expect_float_array::<16>(
                    &entry.node,
                    &field,
                    "inverse_bind",
                    "a joint's inverse bind matrix is 16 floats, column-major",
                    diagnostics,
                );
            }
            unknown => diagnostics.push(envelope::unknown_key(
                unknown,
                entry,
                &pointer,
                "a joint",
                JOINT_KEYS,
                code::MESH_SKIN_STRUCTURE,
            )),
        }
    }
    for key in ["name", "rest", "inverse_bind"] {
        if !entries.iter().any(|entry| entry.key == key) {
            diagnostics.push(missing_nested(&pointer, "a joint", key));
        }
    }
    if diagnostics.len() != before {
        return None;
    }
    Some(MeshJoint {
        name: name?,
        parent,
        rest: rest?,
        inverse_bind: inverse_bind?,
    })
}

/// Parses a joint's `rest` mapping: `t` (3 floats), `r` (4, a quaternion) and `s` (3), all required.
fn parse_rest(node: &Node, pointer: &str, diagnostics: &mut Vec<Diagnostic>) -> Option<JointRest> {
    let entries = expect_mapping(node, pointer, "a joint's `rest`", diagnostics)?;
    let before = diagnostics.len();
    let mut translation = None;
    let mut rotation = None;
    let mut scale = None;
    for entry in entries {
        let field = format!("{pointer}/{}", entry.key);
        match entry.key.as_str() {
            "t" => {
                translation = expect_float_array::<3>(
                    &entry.node,
                    &field,
                    "t",
                    "a joint's rest translation `t` is 3 floats",
                    diagnostics,
                );
            }
            "r" => {
                rotation = expect_float_array::<4>(
                    &entry.node,
                    &field,
                    "r",
                    "a joint's rest rotation `r` is a quaternion, 4 floats x, y, z, w",
                    diagnostics,
                );
            }
            "s" => {
                scale = expect_float_array::<3>(
                    &entry.node,
                    &field,
                    "s",
                    "a joint's rest scale `s` is 3 floats",
                    diagnostics,
                );
            }
            unknown => diagnostics.push(envelope::unknown_key(
                unknown,
                entry,
                pointer,
                "a joint's `rest`",
                REST_KEYS,
                code::MESH_SKIN_STRUCTURE,
            )),
        }
    }
    for key in REST_KEYS {
        if !entries.iter().any(|entry| entry.key == key) {
            diagnostics.push(missing_nested(pointer, "a joint's `rest`", key));
        }
    }
    if diagnostics.len() != before {
        return None;
    }
    Some(JointRest {
        translation: translation?,
        rotation: rotation?,
        scale: scale?,
    })
}

/// Parses a joint's `parent`: a non-negative integer that fits in `u32`. Whether it names an
/// earlier joint is data consistency, checked in [`validate`].
fn expect_joint_parent(
    node: &Node,
    pointer: &str,
    diagnostics: &mut Vec<Diagnostic>,
) -> Option<u32> {
    let valid = match &node.value {
        NodeValue::Scalar(Value::Number(n)) => n.as_u64().and_then(|v| u32::try_from(v).ok()),
        _ => None,
    };
    if valid.is_none() {
        diagnostics.push(
            Diagnostic::new(
                code::MESH_SKIN_STRUCTURE,
                format!(
                    "a joint's `parent` must be a non-negative integer that fits in 32 bits, \
                     found {} (line {})",
                    node.value.type_phrase(),
                    node.line
                ),
            )
            .with_path(pointer)
            .with_help("`parent` is the index of an earlier joint in `joints`; omit it on a root"),
        );
    }
    valid
}

/// Parses a float list of exactly `N` entries; `purpose` says what the length means.
fn expect_float_array<const N: usize>(
    node: &Node,
    pointer: &str,
    what: &str,
    purpose: &str,
    diagnostics: &mut Vec<Diagnostic>,
) -> Option<[f64; N]> {
    let values = expect_float_list(node, pointer, what, code::MESH_SKIN_STRUCTURE, diagnostics)?;
    let len = values.len();
    match <[f64; N]>::try_from(values) {
        Ok(array) => Some(array),
        Err(_) => {
            diagnostics.push(
                Diagnostic::new(
                    code::MESH_SKIN_STRUCTURE,
                    format!("`{what}` has {len} entries; {purpose}"),
                )
                .with_path(pointer)
                .with_help(format!("supply exactly {N} numbers, one per line")),
            );
            None
        }
    }
}

fn missing_nested(pointer: &str, noun: &str, key: &str) -> Diagnostic {
    Diagnostic::new(
        code::MESH_SKIN_STRUCTURE,
        format!("{noun} is missing its required `{key}`"),
    )
    .with_path(pointer.to_string())
    .with_help(format!("add `{key}:` to it"))
}

fn expect_sequence<'a>(
    node: &'a Node,
    pointer: &str,
    what: &str,
    diagnostics: &mut Vec<Diagnostic>,
) -> Option<&'a [Node]> {
    match &node.value {
        NodeValue::Sequence(items) => Some(items),
        other => {
            diagnostics.push(
                Diagnostic::new(
                    code::MESH_SKIN_STRUCTURE,
                    format!(
                        "`{what}` must be a sequence, found {} (line {})",
                        other.type_phrase(),
                        node.line
                    ),
                )
                .with_path(pointer.to_string())
                .with_help(format!(
                    "write `{what}` as a list, one `- name: ...` entry per item"
                )),
            );
            None
        }
    }
}

fn expect_mapping<'a>(
    node: &'a Node,
    pointer: &str,
    what: &str,
    diagnostics: &mut Vec<Diagnostic>,
) -> Option<&'a [MapEntry]> {
    match &node.value {
        NodeValue::Mapping(entries) => Some(entries),
        other => {
            diagnostics.push(
                Diagnostic::new(
                    code::MESH_SKIN_STRUCTURE,
                    format!(
                        "{what} must be a mapping, found {} (line {})",
                        other.type_phrase(),
                        node.line
                    ),
                )
                .with_path(pointer.to_string())
                .with_help("write it as `key: value` lines"),
            );
            None
        }
    }
}

/// Parses one texture reference (`base_color_texture`, ...): a mapping with `path` (required, a
/// string), optional `wrap`, and the map's own scalar key when it has one (`scale` on a normal map,
/// `strength` on an occlusion map — spelling it on any other map is an unknown key). Returns `None`
/// on any shape error; whether the path is empty is data consistency, checked in [`validate`].
fn expect_texture_ref(
    node: &Node,
    key: &'static str,
    scalar_key: Option<&'static str>,
    diagnostics: &mut Vec<Diagnostic>,
) -> Option<TextureRef> {
    let pointer = format!("/{key}");
    let NodeValue::Mapping(entries) = &node.value else {
        diagnostics.push(
            Diagnostic::new(
                code::MESH_DOCUMENT_STRUCTURE,
                format!(
                    "`{key}` must be a mapping, found {} (line {})",
                    node.value.type_phrase(),
                    node.line
                ),
            )
            .with_path(&pointer)
            .with_help(format!(
                "write it as\n  {key}:\n    path: textures/0123456789abcdef.png"
            )),
        );
        return None;
    };

    let before = diagnostics.len();
    let mut reference = TextureRef::new(String::new());
    let mut path_seen = false;
    for entry in entries {
        let field = format!("{pointer}/{}", entry.key);
        match entry.key.as_str() {
            "path" => {
                path_seen = true;
                if let Some(path) = envelope::expect_string(
                    &entry.node,
                    &field,
                    format!("the `{key}` path").as_str(),
                    code::MESH_DOCUMENT_STRUCTURE,
                    diagnostics,
                ) {
                    reference.path = path;
                }
            }
            "wrap" => {
                if let Some(wrap) = expect_wrap(&entry.node, &field, diagnostics) {
                    reference.wrap = wrap;
                }
            }
            other if Some(other) == scalar_key => {
                if let Some(factor) = expect_number(&entry.node, &field, other, diagnostics) {
                    reference.factor = factor;
                }
            }
            unknown => {
                let context = format!("a `{key}` reference");
                diagnostics.push(match scalar_key {
                    Some(scalar) => envelope::unknown_key(
                        unknown,
                        entry,
                        &pointer,
                        &context,
                        ["path", "wrap", scalar],
                        code::MESH_DOCUMENT_STRUCTURE,
                    ),
                    None => envelope::unknown_key(
                        unknown,
                        entry,
                        &pointer,
                        &context,
                        ["path", "wrap"],
                        code::MESH_DOCUMENT_STRUCTURE,
                    ),
                });
            }
        }
    }

    if !path_seen {
        diagnostics.push(
            Diagnostic::new(
                code::MESH_DOCUMENT_STRUCTURE,
                format!("`{key}` is missing the required key `path`"),
            )
            .with_path(&pointer)
            .with_help(
                "point `path` at the PNG, written relative to this mesh document's own directory",
            ),
        );
    }
    (diagnostics.len() == before).then_some(reference)
}

/// Parses a texture's `wrap` mode from its closed set of spellings.
fn expect_wrap(
    node: &Node,
    pointer: &str,
    diagnostics: &mut Vec<Diagnostic>,
) -> Option<TextureWrap> {
    let NodeValue::Scalar(Value::String(name)) = &node.value else {
        diagnostics.push(
            Diagnostic::new(
                code::MESH_DOCUMENT_STRUCTURE,
                format!(
                    "`wrap` must be a string, found {} (line {})",
                    node.value.type_phrase(),
                    node.line
                ),
            )
            .with_path(pointer)
            .with_help(format!("`wrap` is one of: {}", WRAP_NAMES.join(", "))),
        );
        return None;
    };
    match name.as_str() {
        "repeat" => Some(TextureWrap::Repeat),
        "clamp" => Some(TextureWrap::Clamp),
        "mirror" => Some(TextureWrap::Mirror),
        unknown => {
            let help = match suggest::nearest(unknown, WRAP_NAMES) {
                Some(candidate) => format!("nearest match is `{candidate}`"),
                None => format!("`wrap` is one of: {}", WRAP_NAMES.join(", ")),
            };
            diagnostics.push(
                Diagnostic::new(
                    code::MESH_DOCUMENT_STRUCTURE,
                    format!("unknown texture wrap mode `{unknown}` (line {})", node.line),
                )
                .with_path(pointer)
                .with_help(help),
            );
            None
        }
    }
}

/// Parses `alpha_mode` from its closed set of spellings; `Some(true)` for `mask`.
fn expect_alpha_mode(node: &Node, diagnostics: &mut Vec<Diagnostic>) -> Option<bool> {
    let NodeValue::Scalar(Value::String(name)) = &node.value else {
        diagnostics.push(
            Diagnostic::new(
                code::MESH_DOCUMENT_STRUCTURE,
                format!(
                    "`alpha_mode` must be a string, found {} (line {})",
                    node.value.type_phrase(),
                    node.line
                ),
            )
            .with_path("/alpha_mode")
            .with_help(format!(
                "`alpha_mode` is one of: {}",
                ALPHA_MODE_NAMES.join(", ")
            )),
        );
        return None;
    };
    match name.as_str() {
        "opaque" => Some(false),
        "mask" => Some(true),
        unknown => {
            let help = if unknown == "blend" {
                "sorted alpha-blend is unsupported; use `mask` to cut the mesh out by alpha"
                    .to_string()
            } else {
                match suggest::nearest(unknown, ALPHA_MODE_NAMES) {
                    Some(candidate) => format!("nearest match is `{candidate}`"),
                    None => format!("`alpha_mode` is one of: {}", ALPHA_MODE_NAMES.join(", ")),
                }
            };
            diagnostics.push(
                Diagnostic::new(
                    code::MESH_DOCUMENT_STRUCTURE,
                    format!("unknown alpha mode `{unknown}` (line {})", node.line),
                )
                .with_path("/alpha_mode")
                .with_help(help),
            );
            None
        }
    }
}

/// Parses a single number (a texture's `scale`/`strength`). Finiteness is data consistency, checked
/// in [`validate`].
fn expect_number(
    node: &Node,
    pointer: &str,
    what: &str,
    diagnostics: &mut Vec<Diagnostic>,
) -> Option<f64> {
    match &node.value {
        NodeValue::Scalar(Value::Number(n)) => Some(crate::fields::number(n)),
        other => {
            diagnostics.push(
                Diagnostic::new(
                    code::MESH_DOCUMENT_STRUCTURE,
                    format!(
                        "`{what}` must be a number, found {} (line {})",
                        other.type_phrase(),
                        node.line
                    ),
                )
                .with_path(pointer)
                .with_help(format!(
                    "write `{what}: 1.0`, or omit the key for the default"
                )),
            );
            None
        }
    }
}

/// Checks that level `index` of `base.lods` (0-based, so level `index + 1` of the chain) draws through
/// the base's pipeline and bone palette (`L1396`): the same `alpha_mode`, `double_sided` and `rim`,
/// the same set of texture slots, and a skin on both with identical joints or on neither. A pure
/// function of two parsed documents; the loader that reads the level file calls it. A level that
/// carries its own `lods` is a chain error (`L1395`).
#[must_use]
pub fn check_lod_level(base: &MeshDocument, level: &MeshDocument, index: usize) -> Vec<Diagnostic> {
    let path = base.lods.get(index).map_or("?", |lod| lod.mesh.as_str());
    let mismatch = |key: &str, what: String| {
        Diagnostic::new(
            code::MESH_LOD_LEVEL_MISMATCH,
            format!(
                "level {} `{path}` of this mesh differs from the base in `{key}`: {what}",
                index + 1
            ),
        )
        .with_path(format!("/lods/{index}/mesh"))
        .with_help(format!(
            "make `{key}` in `{path}` match the base mesh, or drop the level from `lods`"
        ))
    };
    if !level.lods.is_empty() {
        return vec![
            Diagnostic::new(
                code::MESH_LOD_CHAIN,
                format!(
                    "level {} `{path}` has its own `lods`; chains do not nest",
                    index + 1
                ),
            )
            .with_path(format!("/lods/{index}/mesh"))
            .with_help("list every level in the base document and remove `lods` from the level"),
        ];
    }
    let mut out = Vec::new();
    if base.alpha_mode != level.alpha_mode {
        out.push(mismatch(
            "alpha_mode",
            format!("{:?} against {:?}", base.alpha_mode, level.alpha_mode),
        ));
    }
    if base.double_sided != level.double_sided {
        out.push(mismatch(
            "double_sided",
            format!("{} against {}", base.double_sided, level.double_sided),
        ));
    }
    if base.rim != level.rim {
        out.push(mismatch("rim", "the rim highlights differ".to_string()));
    }
    for (b, l) in base
        .textures
        .slots()
        .iter()
        .zip(level.textures.slots().iter())
    {
        if b.reference.is_some() != l.reference.is_some() {
            let (has, lacks) = if b.reference.is_some() {
                ("the base", "the level")
            } else {
                ("the level", "the base")
            };
            out.push(mismatch(
                b.key,
                format!("{has} sets it and {lacks} does not"),
            ));
        }
    }
    match (&base.skin, &level.skin) {
        (None, None) => {}
        (Some(_), None) => out.push(mismatch(
            "joints",
            "the base is skinned and the level is not".into(),
        )),
        (None, Some(_)) => out.push(mismatch(
            "joints",
            "the level is skinned and the base is not".into(),
        )),
        (Some(b), Some(l)) => {
            if b.joints != l.joints {
                let first = b
                    .joints
                    .iter()
                    .zip(&l.joints)
                    .position(|(x, y)| x != y)
                    .map_or_else(
                        || format!("{} joints against {}", b.joints.len(), l.joints.len()),
                        |at| format!("joint {at} (`{}`) differs", b.joints[at].name),
                    );
                out.push(mismatch("joints", first));
            }
        }
    }
    out
}

/// Checks the structure of a `lods` chain (`L1395`): at most [`MAX_MESH_LODS`] levels, each path a
/// relative `*.mesh.yaml` that stays inside the project, each `screen_size` finite, above zero and
/// strictly below the previous level's.
fn validate_lods(lods: &[MeshLodRef], diagnostics: &mut Vec<Diagnostic>) {
    crate::lod::check_count(lods.len(), "a mesh", diagnostics);
    let mut previous = f64::INFINITY;
    for (index, lod) in lods.iter().enumerate() {
        let pointer = format!("/lods/{index}");
        if !lod.mesh.ends_with(".mesh.yaml") {
            diagnostics.push(
                Diagnostic::new(
                    code::MESH_LOD_CHAIN,
                    format!(
                        "level {} path `{}` is not a `*.mesh.yaml` file",
                        index + 1,
                        lod.mesh
                    ),
                )
                .with_path(format!("{pointer}/mesh"))
                .with_help("point `mesh` at a mesh document, e.g. `crate.lod1.mesh.yaml`"),
            );
        }
        if let Some(refusal) = ludion_core::paths::sibling_refusal(&lod.mesh) {
            diagnostics.push(
                Diagnostic::new(
                    code::MESH_LOD_CHAIN,
                    format!(
                        "level {} path `{}` {}; a level is written relative to this document",
                        index + 1,
                        lod.mesh,
                        refusal.describe()
                    ),
                )
                .with_path(format!("{pointer}/mesh"))
                .with_help("write the level as a relative path, e.g. `crate.lod1.mesh.yaml`"),
            );
        }
        crate::lod::check_screen_size(index, lod.screen_size, &mut previous, diagnostics);
    }
}

/// Validates data-consistency invariants of an already-parsed [`MeshDocument`] (`L1111`): flat-list
/// counts are the right multiple, cross-list counts agree, every index is in range, and every
/// float is finite.
///
/// Independent of how the document was built — a hand-authored one via [`parse`], or one an
/// importer assembles — so this is the one place those invariants are enforced.
pub(crate) fn validate(doc: &MeshDocument) -> Vec<Diagnostic> {
    let mut diagnostics = Vec::new();

    if let Some(origin) = &doc.origin {
        generation::validate_origin_at(
            origin,
            "/origin",
            code::MESH_DATA_CONSISTENCY,
            &mut diagnostics,
        );
    }
    if let Some(rim) = &doc.rim {
        diagnostics.extend(crate::rim::validate_rim(
            rim,
            "/rim",
            code::MESH_DATA_CONSISTENCY,
        ));
    }
    validate_lods(&doc.lods, &mut diagnostics);
    check_finite(&doc.positions, "positions", &mut diagnostics);
    if !doc.positions.len().is_multiple_of(3) {
        diagnostics.push(
            Diagnostic::new(
                code::MESH_DATA_CONSISTENCY,
                format!(
                    "`positions` has {} entries, which is not divisible by 3 (3 floats per \
                     vertex)",
                    doc.positions.len()
                ),
            )
            .with_path("/positions")
            .with_help("add or remove entries so the count is a multiple of 3"),
        );
    }
    let vertex_count = doc.vertex_count();

    if let Some(normals) = &doc.normals {
        check_finite(normals, "normals", &mut diagnostics);
        if normals.len() != doc.positions.len() {
            diagnostics.push(
                Diagnostic::new(
                    code::MESH_DATA_CONSISTENCY,
                    format!(
                        "`normals` has {} entries but `positions` has {} — they must match \
                         one-for-one (3 floats per vertex)",
                        normals.len(),
                        doc.positions.len()
                    ),
                )
                .with_path("/normals")
                .with_help(
                    "supply exactly 3 normal floats per vertex, or omit `normals` entirely to \
                     compute faceted normals at load",
                ),
            );
        } else {
            push_degenerate_vectors(
                normals,
                "normals",
                "a normal",
                "re-import the model (the importer replaces a zero normal with a faceted one); for a hand-written mesh, drop `normals` and the loader computes faceted ones",
                &mut diagnostics,
            );
        }
    }

    if let Some(uvs) = &doc.uvs {
        check_finite(uvs, "uvs", &mut diagnostics);
        if !uvs.len().is_multiple_of(2) {
            diagnostics.push(
                Diagnostic::new(
                    code::MESH_DATA_CONSISTENCY,
                    format!(
                        "`uvs` has {} entries, which is not divisible by 2 (2 floats per vertex)",
                        uvs.len()
                    ),
                )
                .with_path("/uvs")
                .with_help("add or remove entries so the count is a multiple of 2"),
            );
        } else if uvs.len() / 2 != vertex_count {
            diagnostics.push(
                Diagnostic::new(
                    code::MESH_DATA_CONSISTENCY,
                    format!(
                        "`uvs` describes {} vertices' worth of entries but `positions` has {}",
                        uvs.len() / 2,
                        vertex_count
                    ),
                )
                .with_path("/uvs")
                .with_help("supply exactly one UV pair per vertex"),
            );
        }
    }

    if let Some(tangents) = &doc.tangents {
        check_finite(tangents, "tangents", &mut diagnostics);
        if tangents.len() != vertex_count * 4 {
            diagnostics.push(
                Diagnostic::new(
                    code::MESH_DATA_CONSISTENCY,
                    format!(
                        "`tangents` has {} entries but `positions` has {vertex_count} vertices — \
                         a tangent is 4 floats per vertex (xyz plus the handedness w)",
                        tangents.len()
                    ),
                )
                .with_path("/tangents")
                .with_help(
                    "supply exactly 4 tangent floats per vertex, or omit `tangents` entirely (only \
                     a normal map needs them)",
                ),
            );
        }
        if tangents.len() == vertex_count * 4 {
            let xyz: Vec<f64> = tangents
                .chunks_exact(4)
                .flat_map(|t| [t[0], t[1], t[2]])
                .collect();
            push_degenerate_vectors(
                &xyz,
                "tangents",
                "a tangent",
                "re-import the model (the importer repairs a zero tangent); for a hand-written mesh, drop `tangents` or give each a non-zero vector",
                &mut diagnostics,
            );
        }
        // The w component carries handedness as a sign, so anything between the two signs is a
        // silently wrong bitangent rather than an approximation.
        for (index, &w) in tangents.iter().enumerate().skip(3).step_by(4) {
            if w.is_finite() && w != -1.0 && w != 1.0 {
                diagnostics.push(
                    Diagnostic::new(
                        code::MESH_DATA_CONSISTENCY,
                        format!(
                            "`tangents[{index}]` is {w}, but a tangent's handedness `w` must be \
                             exactly -1 or 1"
                        ),
                    )
                    .with_path(format!("/tangents/{index}"))
                    .with_help("every 4th tangent entry is the handedness sign, not a magnitude"),
                );
            }
        }
    }

    check_material_list(
        doc.emissive.as_deref(),
        "emissive",
        vertex_count,
        &mut diagnostics,
    );
    check_material_list(
        doc.metallic.as_deref(),
        "metallic",
        vertex_count,
        &mut diagnostics,
    );
    check_material_list(
        doc.roughness.as_deref(),
        "roughness",
        vertex_count,
        &mut diagnostics,
    );
    check_textures(doc, &mut diagnostics);
    if let AlphaMode::Mask { cutoff } = doc.alpha_mode
        && (!cutoff.is_finite() || !(0.0..=1.0).contains(&cutoff))
    {
        diagnostics.push(
            Diagnostic::new(
                code::MESH_DATA_CONSISTENCY,
                format!("`alpha_cutoff` is {cutoff}, but it must be a finite value in [0, 1]"),
            )
            .with_path("/alpha_cutoff")
            .with_help("a fragment is cut out where its alpha is under the cutoff; 0.5 is typical"),
        );
    }
    check_skin(doc, vertex_count, &mut diagnostics);

    if !doc.indices.len().is_multiple_of(3) {
        diagnostics.push(
            Diagnostic::new(
                code::MESH_DATA_CONSISTENCY,
                format!(
                    "`indices` has {} entries, which is not divisible by 3 (a triangle list \
                     needs 3 indices per triangle)",
                    doc.indices.len()
                ),
            )
            .with_path("/indices")
            .with_help("add or remove entries so the count is a multiple of 3"),
        );
    }
    for (index, &value) in doc.indices.iter().enumerate() {
        if (value as usize) >= vertex_count {
            diagnostics.push(
                Diagnostic::new(
                    code::MESH_DATA_CONSISTENCY,
                    format!(
                        "`indices[{index}]` is {value}, but `positions` only has {vertex_count} \
                         vertices (valid indices are 0..{vertex_count})"
                    ),
                )
                .with_path(format!("/indices/{index}"))
                .with_help("check the index, or add the missing vertex to `positions`"),
            );
        }
    }

    diagnostics
}

/// Validates an optional per-vertex material list (`emissive`, `metallic`, `roughness`): finite,
/// exactly one entry per vertex, and in the field's valid range — `roughness`/`metallic` are
/// perceptual `[0, 1]`, `emissive` is a non-negative HDR intensity multiplier
/// (`Plan/phase-11-pbr.md` decision 1). A no-op when `values` is `None` — an absent list means
/// "use the default material value".
/// Squared length under which an authored normal or tangent counts as zero-length: normalizing it
/// in the shader yields NaN or an arbitrary frame.
pub const DEGENERATE_VECTOR_LENGTH_SQUARED: f64 = 1e-12;

/// L1111 for each finite 3-vector in `flat` (a flat `xyz` list) whose length is near zero.
fn push_degenerate_vectors(
    flat: &[f64],
    field: &str,
    noun: &str,
    help: &str,
    diagnostics: &mut Vec<Diagnostic>,
) {
    for (vertex, v) in flat.chunks_exact(3).enumerate() {
        if v.iter().all(|c| c.is_finite())
            && v.iter().map(|c| c * c).sum::<f64>() < DEGENERATE_VECTOR_LENGTH_SQUARED
        {
            diagnostics.push(
                Diagnostic::new(
                    code::MESH_DATA_CONSISTENCY,
                    format!("`{field}` vertex {vertex} is {noun} of near-zero length"),
                )
                .with_path(format!("/{field}/{}", vertex * 3))
                .with_help(help),
            );
        }
    }
}

/// Validates a document's surface tags (`L1696`): names non-empty, unique and at most 256, runs
/// non-zero with in-range local indices, and counts summing to the triangle count.
pub(crate) fn validate_surfaces(doc: &MeshDocument, surfaces: &MeshSurfaces) -> Vec<Diagnostic> {
    let invalid = |message: String, pointer: String, help: &str| {
        Diagnostic::new(code::MESH_SURFACES_INVALID, message)
            .with_path(pointer)
            .with_help(help.to_string())
    };
    let mut diagnostics = Vec::new();
    if surfaces.names.len() > MAX_MESH_SURFACES {
        diagnostics.push(invalid(
            format!(
                "`surfaces` lists {} names, more than the {MAX_MESH_SURFACES} a mesh may tag",
                surfaces.names.len()
            ),
            "/surfaces".to_string(),
            "merge surfaces so the mesh names at most 256",
        ));
    }
    for (index, name) in surfaces.names.iter().enumerate() {
        if name.is_empty() {
            diagnostics.push(invalid(
                format!("`surfaces[{index}]` is empty"),
                format!("/surfaces/{index}"),
                "name the surface, or remove the entry",
            ));
        } else if surfaces.names[..index].contains(name) {
            diagnostics.push(invalid(
                format!("`surfaces[{index}]` repeats `{name}`"),
                format!("/surfaces/{index}"),
                "list each surface once and point runs at its single index",
            ));
        }
    }
    let mut total: u64 = 0;
    for (index, [count, local]) in surfaces.runs.iter().enumerate() {
        total += u64::from(*count);
        if *count == 0 {
            diagnostics.push(invalid(
                format!("`triangle_surfaces[{index}]` has a zero count"),
                format!("/triangle_surfaces/{index}"),
                "drop the empty run",
            ));
        }
        if *local as usize >= surfaces.names.len() {
            diagnostics.push(invalid(
                format!(
                    "`triangle_surfaces[{index}]` names local surface {local}, but `surfaces` \
                     lists {}",
                    surfaces.names.len()
                ),
                format!("/triangle_surfaces/{index}"),
                "use an index below the length of `surfaces`",
            ));
        }
    }
    let triangles = doc.triangle_count() as u64;
    if total != triangles {
        diagnostics.push(invalid(
            format!(
                "`triangle_surfaces` counts sum to {total}, but the mesh has {triangles} triangles"
            ),
            "/triangle_surfaces".to_string(),
            "the run counts must cover every triangle exactly once, in index order",
        ));
    }
    diagnostics
}

fn check_material_list(
    values: Option<&[f64]>,
    field: &str,
    vertex_count: usize,
    diagnostics: &mut Vec<Diagnostic>,
) {
    let Some(values) = values else { return };
    check_finite(values, field, diagnostics);
    if values.len() != vertex_count {
        diagnostics.push(
            Diagnostic::new(
                code::MESH_DATA_CONSISTENCY,
                format!(
                    "`{field}` has {} entries but `positions` has {vertex_count} vertices — \
                     they must match one-for-one (1 float per vertex)",
                    values.len()
                ),
            )
            .with_path(format!("/{field}"))
            .with_help(format!(
                "supply exactly 1 `{field}` float per vertex, or omit `{field}` entirely to use \
                 the default material value"
            )),
        );
    }
    if field == "emissive" {
        check_non_negative(values, field, diagnostics);
    } else {
        check_unit_range(values, field, diagnostics);
    }
}

/// Validates the texture references (`L1111`): a map needs somewhere to sample from, so any texture
/// requires `uvs` and a normal map additionally requires `tangents`; a path is written
/// relative to the document's own directory and may climb with any number of `..`, but is refused
/// when absolute, drive-prefixed, NUL-bearing or a Windows device; a factor must be a number the sampler can use.
fn check_textures(doc: &MeshDocument, diagnostics: &mut Vec<Diagnostic>) {
    for slot in doc.textures.slots() {
        let Some(reference) = slot.reference else {
            continue;
        };
        let key = slot.key;
        if reference.path.is_empty() {
            diagnostics.push(
                Diagnostic::new(
                    code::MESH_DATA_CONSISTENCY,
                    format!("`{key}` has an empty `path`"),
                )
                .with_path(format!("/{key}/path"))
                .with_help(
                    "point `path` at the PNG, written relative to this mesh document's own \
                     directory",
                ),
            );
        } else if let Some(refusal) = sibling_refusal(&reference.path) {
            diagnostics.push(
                Diagnostic::new(
                    code::MESH_DATA_CONSISTENCY,
                    format!(
                        "`{key}` `path` (`{}`) {}",
                        reference.path,
                        refusal.describe()
                    ),
                )
                .with_path(format!("/{key}/path"))
                .with_help(
                    "point `path` at the PNG, written relative to this mesh document's own \
                     directory",
                ),
            );
        }
        if doc.uvs.is_none() {
            diagnostics.push(
                Diagnostic::new(
                    code::MESH_DATA_CONSISTENCY,
                    format!("`{key}` needs `uvs`, but this mesh has none"),
                )
                .with_path(format!("/{key}"))
                .with_help(
                    "a texture is sampled at the vertex UVs; add a `uvs` list (2 floats per \
                     vertex) or drop the texture",
                ),
            );
        }
        if let Some(scalar_key) = slot.scalar_key
            && !reference.factor.is_finite()
        {
            diagnostics.push(
                Diagnostic::new(
                    code::MESH_DATA_CONSISTENCY,
                    format!(
                        "`{key}` has a `{scalar_key}` of {}, which is not a finite number",
                        reference.factor
                    ),
                )
                .with_path(format!("/{key}/{scalar_key}")),
            );
        }
    }
    if doc.textures.normal.is_some() && doc.tangents.is_none() {
        diagnostics.push(
            Diagnostic::new(
                code::MESH_DATA_CONSISTENCY,
                "`normal_texture` needs `tangents`, but this mesh has none",
            )
            .with_path("/normal_texture")
            .with_help(
                "a tangent-space normal map is meaningless without a tangent frame; add a \
                 `tangents` list (4 floats per vertex) or drop the texture",
            ),
        );
    }
}

/// The per-joint skin checks (`L1191`): a bounded joint count, unique non-empty names, parent-first
/// order and a finite, unit-rotation rest pose. Shared by the full validation and the joints-only
/// read.
fn check_joints(joints: &[MeshJoint], diagnostics: &mut Vec<Diagnostic>) {
    let joint_count = joints.len();
    if joint_count == 0 {
        diagnostics.push(
            Diagnostic::new(code::MESH_SKIN_DATA, "`joints` is empty")
                .with_path("/joints")
                .with_help("a skinned mesh needs at least one joint, or drop the skin keys"),
        );
    }
    if joint_count > MESH_JOINT_LIMIT {
        diagnostics.push(
            Diagnostic::new(
                code::MESH_SKIN_DATA,
                format!(
                    "`joints` has {joint_count} entries, above the {MESH_JOINT_LIMIT}-joint limit"
                ),
            )
            .with_path("/joints")
            .with_help("the GPU bone palette holds at most 256 joints; merge or drop joints"),
        );
    }

    let mut first_named: BTreeMap<&str, usize> = BTreeMap::new();
    for (index, joint) in joints.iter().enumerate() {
        if joint.name.is_empty() {
            diagnostics.push(
                Diagnostic::new(
                    code::MESH_SKIN_DATA,
                    format!("joint {index} has an empty `name`"),
                )
                .with_path(format!("/joints/{index}/name"))
                .with_help("name every joint; a `SkinBone` part finds its bone by name"),
            );
        } else if let Some(first) = first_named.insert(joint.name.as_str(), index) {
            diagnostics.push(
                Diagnostic::new(
                    code::MESH_SKIN_DATA,
                    format!(
                        "joint {index} is named `{}`, the name joint {first} already has",
                        joint.name
                    ),
                )
                .with_path(format!("/joints/{index}/name"))
                .with_help("joint names are unique within a skin; rename one"),
            );
        }
        if let Some(parent) = joint.parent
            && parent as usize >= index
        {
            diagnostics.push(
                Diagnostic::new(
                    code::MESH_SKIN_DATA,
                    format!(
                        "joint {index} has `parent` {parent}; a joint's parent must come earlier \
                         in `joints`"
                    ),
                )
                .with_path(format!("/joints/{index}/parent"))
                .with_help("reorder `joints` so every parent precedes its children"),
            );
        }
        check_finite_as(
            &joint.rest.translation,
            &format!("joints/{index}/rest/t"),
            code::MESH_SKIN_DATA,
            diagnostics,
        );
        check_finite_as(
            &joint.rest.rotation,
            &format!("joints/{index}/rest/r"),
            code::MESH_SKIN_DATA,
            diagnostics,
        );
        check_finite_as(
            &joint.rest.scale,
            &format!("joints/{index}/rest/s"),
            code::MESH_SKIN_DATA,
            diagnostics,
        );
        check_finite_as(
            &joint.inverse_bind,
            &format!("joints/{index}/inverse_bind"),
            code::MESH_SKIN_DATA,
            diagnostics,
        );
        let rotation = joint.rest.rotation;
        if rotation.iter().all(|v| v.is_finite()) {
            let length = rotation.iter().map(|v| v * v).sum::<f64>().sqrt();
            if (length - 1.0).abs() > REST_ROTATION_TOLERANCE {
                diagnostics.push(
                    Diagnostic::new(
                        code::MESH_SKIN_DATA,
                        format!(
                            "joint {index}'s rest rotation has length {length}, but a rest \
                             rotation is a unit quaternion"
                        ),
                    )
                    .with_path(format!("/joints/{index}/rest/r"))
                    .with_help("a rest rotation is a unit quaternion x, y, z, w"),
                );
            }
        }
    }
}

/// Validates the skin (`L1191`): a bounded, named, parent-first joint list with a sane rest pose,
/// and per-vertex weights and bone indices that are 4 per vertex, non-negative, summing to 1 and
/// naming real joints. The weight order is the importer's choice and is not checked.
fn check_skin(doc: &MeshDocument, vertex_count: usize, diagnostics: &mut Vec<Diagnostic>) {
    let Some(skin) = &doc.skin else { return };
    check_joints(&skin.joints, diagnostics);
    let joint_count = skin.joints.len();

    check_finite_as(&skin.weights, "weights", code::MESH_SKIN_DATA, diagnostics);
    let expected = vertex_count * 4;
    if skin.weights.len() != expected {
        diagnostics.push(
            Diagnostic::new(
                code::MESH_SKIN_DATA,
                format!(
                    "`weights` has {} entries but `positions` has {vertex_count} vertices — a \
                     skinned vertex has exactly 4 weights",
                    skin.weights.len()
                ),
            )
            .with_path("/weights")
            .with_help("supply exactly 4 weights per vertex, padding with 0"),
        );
    }
    if skin.bone_indices.len() != expected {
        diagnostics.push(
            Diagnostic::new(
                code::MESH_SKIN_DATA,
                format!(
                    "`bone_indices` has {} entries but `positions` has {vertex_count} vertices — \
                     a skinned vertex has exactly 4 bone indices",
                    skin.bone_indices.len()
                ),
            )
            .with_path("/bone_indices")
            .with_help("supply exactly 4 bone indices per vertex, padding with 0"),
        );
    }
    for (index, &weight) in skin.weights.iter().enumerate() {
        if weight.is_finite() && weight < 0.0 {
            diagnostics.push(
                Diagnostic::new(
                    code::MESH_SKIN_DATA,
                    format!("`weights[{index}]` is {weight}, but a skin weight must be >= 0"),
                )
                .with_path(format!("/weights/{index}"))
                .with_help("skin weights are non-negative influences that sum to 1 per vertex"),
            );
        }
    }
    if skin.weights.len() == expected {
        for (vertex, weights) in skin.weights.chunks_exact(4).enumerate() {
            let sum: f64 = weights.iter().sum();
            if sum.is_finite() && (sum - 1.0).abs() > SKIN_WEIGHT_SUM_TOLERANCE {
                diagnostics.push(
                    Diagnostic::new(
                        code::MESH_SKIN_DATA,
                        format!("vertex {vertex}'s 4 weights sum to {sum}, not 1"),
                    )
                    .with_path(format!("/weights/{}", vertex * 4))
                    .with_help("renormalize the vertex's weights so they sum to 1"),
                );
            }
        }
    }
    for (index, &bone) in skin.bone_indices.iter().enumerate() {
        if bone as usize >= joint_count {
            diagnostics.push(
                Diagnostic::new(
                    code::MESH_SKIN_DATA,
                    format!(
                        "`bone_indices[{index}]` is {bone}, but `joints` only has {joint_count} \
                         entries (valid indices are 0..{joint_count})"
                    ),
                )
                .with_path(format!("/bone_indices/{index}"))
                .with_help("point the influence at an existing joint"),
            );
        }
    }
}

/// `roughness`/`metallic` are perceptual values, valid only in `[0, 1]`.
fn check_unit_range(values: &[f64], field: &str, diagnostics: &mut Vec<Diagnostic>) {
    for (index, &value) in values.iter().enumerate() {
        if value.is_finite() && !(0.0..=1.0).contains(&value) {
            diagnostics.push(
                Diagnostic::new(
                    code::MESH_DATA_CONSISTENCY,
                    format!(
                        "`{field}[{index}]` is {value}, but `{field}` must be in the range \
                         [0, 1] (a perceptual value)"
                    ),
                )
                .with_path(format!("/{field}/{index}"))
                .with_help(format!("clamp `{field}` to the [0, 1] range")),
            );
        }
    }
}

/// `emissive` is an HDR intensity multiplier, valid only when non-negative.
fn check_non_negative(values: &[f64], field: &str, diagnostics: &mut Vec<Diagnostic>) {
    for (index, &value) in values.iter().enumerate() {
        if value.is_finite() && value < 0.0 {
            diagnostics.push(
                Diagnostic::new(
                    code::MESH_DATA_CONSISTENCY,
                    format!(
                        "`{field}[{index}]` is {value}, but `{field}` must be >= 0 (an HDR \
                         intensity multiplier on the vertex color)"
                    ),
                )
                .with_path(format!("/{field}/{index}"))
                .with_help(format!("`{field}` cannot be negative")),
            );
        }
    }
}

fn check_finite(values: &[f64], field: &str, diagnostics: &mut Vec<Diagnostic>) {
    check_finite_as(values, field, code::MESH_DATA_CONSISTENCY, diagnostics);
}

/// [`check_finite`] under a caller's code — the skin's finiteness is `L1191`, not `L1111`.
fn check_finite_as(
    values: &[f64],
    field: &str,
    code: DiagnosticCode,
    diagnostics: &mut Vec<Diagnostic>,
) {
    for (index, &value) in values.iter().enumerate() {
        if !value.is_finite() {
            diagnostics.push(
                Diagnostic::new(
                    code,
                    format!("`{field}[{index}]` is {value}, which is not a finite number"),
                )
                .with_path(format!("/{field}/{index}"))
                .with_help(
                    "mesh vertex data must be finite; check for a typo or a bad computation \
                     upstream",
                ),
            );
        }
    }
}

/// The non-fatal style-budget warning (`L1112`) for `doc`, if its vertex count exceeds
/// [`MESH_VERTEX_BUDGET`]. `None` when under budget.
pub(crate) fn budget_warning(doc: &MeshDocument) -> Option<Diagnostic> {
    let vertex_count = doc.vertex_count();
    if vertex_count <= MESH_VERTEX_BUDGET {
        return None;
    }
    Some(
        Diagnostic::new(
            code::MESH_VERTEX_BUDGET_EXCEEDED,
            format!(
                "this mesh has {vertex_count} vertices, above the {MESH_VERTEX_BUDGET}-vertex \
                 style budget (the built-in sphere primitive is ~600)"
            ),
        )
        .with_path("/positions")
        .with_help(
            "HD-2D low-poly content should stay well under this; if the density is \
             unintentional, retopologize or lower a generator's subdivision level — this is a \
             warning, not an error, and does not block validation",
        ),
    )
}

/// Writes `doc` in canonical form: `format`, `kind`, `name` (optional), `generated_from`
/// (optional, beside `name` as it is in a generated scene), `origin` (optional, the
/// provenance block, written where a diff shows it rather than below the bulk float lists),
/// `positions`, `normals`
/// (optional), `uvs` (optional), `tangents` (optional — geometry, so it sits with the other vertex
/// lists), `emissive`/`metallic`/`roughness` (each optional, alphabetical
/// order, emitted only when authored), the five texture references (optional, in
/// [`MeshTextures::slots`] order), `alpha_mode: mask` (with `alpha_cutoff` unless it is 0.5) and
/// `double_sided: true` (each only when set), the skin (`joints`, `weights`, `bone_indices` — optional, all
/// three together), `indices`, each flat list one entry per line, ryu
/// shortest-round-trip floats — the mesh analog of [`crate::emit`].
pub(crate) fn emit(doc: &MeshDocument) -> String {
    emit_with_surfaces(doc, None)
}

/// [`emit`] with the `surfaces`/`triangle_surfaces` keys, written right before `indices`.
pub(crate) fn emit_with_surfaces(doc: &MeshDocument, surfaces: Option<&MeshSurfaces>) -> String {
    let mut out = String::new();
    out.push_str("format: 1\n");
    out.push_str("kind: mesh\n");
    if let Some(name) = &doc.name {
        out.push_str("name: ");
        out.push_str(&scalar_string(name));
        out.push('\n');
    }
    if let Some(source) = &doc.generated_from {
        out.push_str("generated_from: ");
        out.push_str(&scalar_string(source));
        out.push('\n');
    }
    if let Some(origin) = &doc.origin {
        out.push_str("origin:\n");
        generation::emit_origin(&mut out, "  ", origin);
    }
    write_float_list(&mut out, "positions", &doc.positions);
    if let Some(normals) = &doc.normals {
        write_float_list(&mut out, "normals", normals);
    }
    if let Some(uvs) = &doc.uvs {
        write_float_list(&mut out, "uvs", uvs);
    }
    if let Some(tangents) = &doc.tangents {
        write_float_list(&mut out, "tangents", tangents);
    }
    if let Some(emissive) = &doc.emissive {
        write_float_list(&mut out, "emissive", emissive);
    }
    if let Some(metallic) = &doc.metallic {
        write_float_list(&mut out, "metallic", metallic);
    }
    if let Some(roughness) = &doc.roughness {
        write_float_list(&mut out, "roughness", roughness);
    }
    for slot in doc.textures.slots() {
        if let Some(reference) = slot.reference {
            write_texture_ref(&mut out, &slot, reference);
        }
    }
    if let AlphaMode::Mask { cutoff } = doc.alpha_mode {
        out.push_str("alpha_mode: mask\n");
        if cutoff != DEFAULT_ALPHA_CUTOFF {
            out.push_str("alpha_cutoff: ");
            out.push_str(&format_float(cutoff));
            out.push('\n');
        }
    }
    if doc.double_sided {
        out.push_str("double_sided: true\n");
    }
    if let Some(rim) = &doc.rim {
        crate::rim::emit_rim(&mut out, rim);
    }
    if let Some(skin) = &doc.skin {
        write_joints(&mut out, &skin.joints);
        write_float_list(&mut out, "weights", &skin.weights);
        write_index_list(&mut out, "bone_indices", &skin.bone_indices);
    }
    if !doc.lods.is_empty() {
        out.push_str("lods:\n");
        for lod in &doc.lods {
            out.push_str("  - mesh: ");
            out.push_str(&scalar_string(&lod.mesh));
            out.push_str("\n    screen_size: ");
            out.push_str(&format_float(lod.screen_size));
            out.push('\n');
        }
    }
    if let Some(surfaces) = surfaces {
        out.push_str(if surfaces.names.is_empty() {
            "surfaces: []\n"
        } else {
            "surfaces:\n"
        });
        for name in &surfaces.names {
            out.push_str("  - ");
            out.push_str(&scalar_string(name));
            out.push('\n');
        }
        out.push_str(if surfaces.runs.is_empty() {
            "triangle_surfaces: []\n"
        } else {
            "triangle_surfaces:\n"
        });
        for [count, local] in &surfaces.runs {
            out.push_str(&format!("  - - {count}\n    - {local}\n"));
        }
    }
    write_index_list(&mut out, "indices", &doc.indices);
    out
}

/// Writes `joints`: per joint `name`, `parent` (omitted on a root), `rest` (`t`, `r`, `s`) and
/// `inverse_bind`, every list one entry per line.
fn write_joints(out: &mut String, joints: &[MeshJoint]) {
    out.push_str("joints:\n");
    for joint in joints {
        out.push_str("  - name: ");
        out.push_str(&scalar_string(&joint.name));
        out.push('\n');
        if let Some(parent) = joint.parent {
            out.push_str("    parent: ");
            out.push_str(&parent.to_string());
            out.push('\n');
        }
        out.push_str("    rest:\n");
        write_nested_floats(out, "      ", "t", &joint.rest.translation);
        write_nested_floats(out, "      ", "r", &joint.rest.rotation);
        write_nested_floats(out, "      ", "s", &joint.rest.scale);
        write_nested_floats(out, "    ", "inverse_bind", &joint.inverse_bind);
    }
}

/// Writes `key:` at `indent`, then one float per line two columns deeper.
fn write_nested_floats(out: &mut String, indent: &str, key: &str, values: &[f64]) {
    out.push_str(indent);
    out.push_str(key);
    out.push_str(":\n");
    for value in values {
        out.push_str(indent);
        out.push_str("  - ");
        out.push_str(&format_float(*value));
        out.push('\n');
    }
}

/// Writes one texture reference: `path` always, `wrap` only when it is not glTF's `repeat` default,
/// and the map's scalar only when it is not the identity `1` — so a plain reference is one line of
/// diff, whatever the schema grows.
fn write_texture_ref(out: &mut String, slot: &TextureSlot<'_>, reference: &TextureRef) {
    out.push_str(slot.key);
    out.push_str(":\n  path: ");
    out.push_str(&scalar_string(&reference.path));
    out.push('\n');
    if reference.wrap != TextureWrap::Repeat {
        out.push_str("  wrap: ");
        out.push_str(reference.wrap.name());
        out.push('\n');
    }
    if let Some(scalar_key) = slot.scalar_key
        && reference.factor != 1.0
    {
        out.push_str("  ");
        out.push_str(scalar_key);
        out.push_str(": ");
        out.push_str(&format_float(reference.factor));
        out.push('\n');
    }
}

fn write_float_list(out: &mut String, key: &str, values: &[f64]) {
    out.push_str(key);
    out.push(':');
    if values.is_empty() {
        out.push_str(" []\n");
        return;
    }
    out.push('\n');
    for value in values {
        out.push_str("  - ");
        out.push_str(&format_float(*value));
        out.push('\n');
    }
}

fn write_index_list(out: &mut String, key: &str, values: &[u32]) {
    out.push_str(key);
    out.push(':');
    if values.is_empty() {
        out.push_str(" []\n");
        return;
    }
    out.push('\n');
    for value in values {
        out.push_str("  - ");
        out.push_str(&value.to_string());
        out.push('\n');
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const CUBE_TRI: &str = "\
format: 1
kind: mesh
positions:
  - 0.0
  - 0.0
  - 0.0
  - 1.0
  - 0.0
  - 0.0
  - 0.0
  - 1.0
  - 0.0
indices:
  - 0
  - 1
  - 2
";

    #[test]
    fn parses_a_minimal_triangle() {
        let doc = parse(CUBE_TRI).expect("mesh should parse");
        assert_eq!(doc.vertex_count(), 3);
        assert_eq!(doc.triangle_count(), 1);
        assert!(doc.normals.is_none());
        assert!(doc.uvs.is_none());
        assert!(validate(&doc).is_empty());
    }

    const CUBE_TRI_WITH_ORIGIN: &str = "\
format: 1
kind: mesh
name: idol
origin:
  tool: comfyui
  comfy_version: \"0.3.61\"
  recipe: generation/recipes/idol.gen.yaml
  run_id: idol-0007
  date: \"2026-09-17\"
  seed: 9
  workflow: generation/workflows/mesh.json
  workflow_hash: \"9f2c\"
  checkpoints:
    - hunyuan3d.safetensors
  prompt: a weathered stone idol
  inputs:
    Empty Latent:
      width: 1024
positions:
  - 0.0
  - 0.0
  - 0.0
  - 1.0
  - 0.0
  - 0.0
  - 0.0
  - 1.0
  - 0.0
indices:
  - 0
  - 1
  - 2
";

    #[test]
    fn emit_is_a_fixed_point() {
        let doc = parse(CUBE_TRI).expect("mesh should parse");
        assert_eq!(emit(&doc), CUBE_TRI);
    }

    /// A mesh document without an `origin:` or `generated_from:` is byte-identical to what it was
    /// before either key existed: each is written only when present.
    #[test]
    fn a_mesh_without_an_origin_is_byte_identical() {
        for source in [CUBE_TRI, CUBE_TRI_TEXTURED] {
            let doc = parse(source).expect("mesh should parse");
            assert!(doc.origin.is_none());
            assert!(doc.generated_from.is_none());
            let text = emit(&doc);
            assert_eq!(text, source);
            assert!(!text.contains("origin") && !text.contains("generated_from"));
        }
    }

    const CUBE_TRI_DERIVED: &str = "\
format: 1
kind: mesh
name: idol-lit
generated_from: \"../idol-lit.mesh-edit.yaml\"
positions:
  - 0.0
  - 0.0
  - 0.0
  - 1.0
  - 0.0
  - 0.0
  - 0.0
  - 1.0
  - 0.0
indices:
  - 0
  - 1
  - 2
";

    /// `generated_from:` sits between `name` and `origin` — the adjacency a generated scene has —
    /// so a derived mesh says what it was derived from before it says what its geometry came from.
    #[test]
    fn a_generated_from_round_trips_after_name() {
        let doc = parse(CUBE_TRI_DERIVED).expect("a derived mesh should parse");
        assert_eq!(
            doc.generated_from.as_deref(),
            Some("../idol-lit.mesh-edit.yaml")
        );
        assert!(validate(&doc).is_empty());
        assert_eq!(emit(&doc), CUBE_TRI_DERIVED);

        let both = CUBE_TRI_WITH_ORIGIN.replace(
            "origin:\n",
            "generated_from: \"../idol-lit.mesh-edit.yaml\"\norigin:\n",
        );
        let doc = parse(&both).expect("a derived, generated mesh should parse");
        assert!(doc.origin.is_some());
        assert_eq!(emit(&doc), both);
    }

    #[test]
    fn a_non_string_generated_from_is_l1110() {
        let source = CUBE_TRI_DERIVED.replace("\"../idol-lit.mesh-edit.yaml\"", "7");
        let diagnostics = parse(&source).unwrap_err();
        let d = diagnostics
            .iter()
            .find(|d| d.path.as_deref() == Some("/generated_from"))
            .expect("the wrong-typed key is reported");
        assert_eq!(d.code, code::MESH_DOCUMENT_STRUCTURE);
    }

    #[test]
    fn an_origin_block_round_trips_after_name() {
        let doc = parse(CUBE_TRI_WITH_ORIGIN).expect("mesh with an origin should parse");
        let origin = doc.origin.as_ref().expect("the origin block is parsed");
        assert_eq!(origin.tool, "comfyui");
        assert_eq!(origin.seed, Some(9));
        assert_eq!(
            origin.checkpoints,
            vec!["hunyuan3d.safetensors".to_string()]
        );
        assert!(validate(&doc).is_empty());
        assert_eq!(emit(&doc), CUBE_TRI_WITH_ORIGIN);
        let name_at = CUBE_TRI_WITH_ORIGIN.find("name:").expect("name is written");
        let origin_at = CUBE_TRI_WITH_ORIGIN
            .find("origin:")
            .expect("origin is written");
        let positions_at = CUBE_TRI_WITH_ORIGIN
            .find("positions:")
            .expect("positions are written");
        assert!(name_at < origin_at && origin_at < positions_at);
    }

    #[test]
    fn a_malformed_origin_reports_the_mesh_structure_code() {
        let source = CUBE_TRI_WITH_ORIGIN.replace("  tool: comfyui\n", "");
        let diagnostics = parse(&source).unwrap_err();
        let d = diagnostics
            .iter()
            .find(|d| d.message.contains("`tool`"))
            .expect("the missing origin key is reported");
        assert_eq!(d.code, code::MESH_DOCUMENT_STRUCTURE);
        assert_eq!(d.path.as_deref(), Some("/origin"));
    }

    const CUBE_TRI_TEXTURED: &str = "\
format: 1
kind: mesh
positions:
  - 0.0
  - 0.0
  - 0.0
  - 1.0
  - 0.0
  - 0.0
  - 0.0
  - 1.0
  - 0.0
uvs:
  - 0.0
  - 0.0
  - 1.0
  - 0.0
  - 0.0
  - 1.0
tangents:
  - 1.0
  - 0.0
  - 0.0
  - 1.0
  - 1.0
  - 0.0
  - 0.0
  - -1.0
  - 1.0
  - 0.0
  - 0.0
  - 1.0
base_color_texture:
  path: textures/0123456789abcdef.png
normal_texture:
  path: textures/fedcba9876543210.png
  wrap: clamp
  scale: 0.8
indices:
  - 0
  - 1
  - 2
";

    #[test]
    fn texture_keys_round_trip_in_canonical_order() {
        let doc = parse(CUBE_TRI_TEXTURED).expect("a textured mesh should parse");
        let base_color = doc
            .textures
            .base_color
            .as_ref()
            .expect("the base-color reference is parsed");
        assert_eq!(base_color.path, "textures/0123456789abcdef.png");
        assert_eq!(base_color.wrap, TextureWrap::Repeat);
        assert_eq!(base_color.factor, 1.0);
        let normal = doc
            .textures
            .normal
            .as_ref()
            .expect("the normal reference is parsed");
        assert_eq!(normal.wrap, TextureWrap::Clamp);
        assert_eq!(normal.factor, 0.8);
        assert!(doc.textures.metallic_roughness.is_none());
        assert!(validate(&doc).is_empty());
        assert_eq!(emit(&doc), CUBE_TRI_TEXTURED);
    }

    /// A mesh that references no texture and carries no tangents is byte-identical to what it was
    /// before either key existed.
    #[test]
    fn an_untextured_mesh_is_byte_identical() {
        let doc = parse(CUBE_TRI).expect("mesh should parse");
        assert!(doc.tangents.is_none());
        assert_eq!(doc.textures, MeshTextures::default());
        let text = emit(&doc);
        assert_eq!(text, CUBE_TRI);
        assert!(!text.contains("tangents") && !text.contains("texture"));
    }

    /// A wrap of `repeat` and a factor of `1` are the defaults, so neither is written — the emitted
    /// reference is one line.
    #[test]
    fn default_wrap_and_factor_are_not_written() {
        let mut doc = parse(CUBE_TRI_TEXTURED).expect("a textured mesh should parse");
        doc.textures.normal = Some(TextureRef::new("textures/fedcba9876543210.png"));
        let text = emit(&doc);
        assert!(text.contains("normal_texture:\n  path: textures/fedcba9876543210.png\nindices:"));
        assert!(!text.contains("wrap") && !text.contains("scale"));
    }

    fn with_keys(keys: &str) -> String {
        CUBE_TRI.replace("indices:", &format!("{keys}indices:"))
    }

    #[test]
    fn alpha_and_double_sided_keys_round_trip() {
        let source = with_keys("alpha_mode: mask\nalpha_cutoff: 0.25\ndouble_sided: true\n");
        let doc = parse(&source).expect("the keys parse");
        assert_eq!(doc.alpha_mode, AlphaMode::Mask { cutoff: 0.25 });
        assert!(doc.double_sided);
        assert!(validate(&doc).is_empty());
        assert_eq!(emit(&doc), source);
    }

    #[test]
    fn explicit_alpha_defaults_normalize_away() {
        let source = with_keys("alpha_mode: mask\nalpha_cutoff: 0.5\ndouble_sided: false\n");
        let doc = parse(&source).expect("the keys parse");
        assert_eq!(doc.alpha_mode, AlphaMode::Mask { cutoff: 0.5 });
        assert_eq!(emit(&doc), with_keys("alpha_mode: mask\n"));
        let opaque = parse(&with_keys("alpha_mode: opaque\n")).expect("opaque parses");
        assert_eq!(opaque.alpha_mode, AlphaMode::Opaque);
        assert_eq!(emit(&opaque), CUBE_TRI);
    }

    #[test]
    fn a_mask_without_a_base_map_is_valid() {
        let doc = parse(&with_keys("alpha_mode: mask\n")).expect("mask parses");
        assert!(doc.textures.base_color.is_none());
        assert!(validate(&doc).is_empty());
    }

    #[test]
    fn an_unknown_alpha_mode_is_l1110_with_a_suggestion() {
        let diagnostics = parse(&with_keys("alpha_mode: maks\n")).unwrap_err();
        let d = diagnostics
            .iter()
            .find(|d| d.path.as_deref() == Some("/alpha_mode"))
            .expect("the bad mode is reported");
        assert_eq!(d.code, code::MESH_DOCUMENT_STRUCTURE);
        assert!(
            d.help
                .as_deref()
                .is_some_and(|h| h.contains("nearest match"))
        );
        let blend = parse(&with_keys("alpha_mode: blend\n")).unwrap_err();
        assert!(
            blend
                .iter()
                .any(|d| d.help.as_deref().is_some_and(|h| h.contains("`mask`")))
        );
    }

    #[test]
    fn a_non_boolean_double_sided_is_l1110() {
        let diagnostics = parse(&with_keys("double_sided: 1\n")).unwrap_err();
        let d = diagnostics
            .iter()
            .find(|d| d.path.as_deref() == Some("/double_sided"))
            .expect("the bad flag is reported");
        assert_eq!(d.code, code::MESH_DOCUMENT_STRUCTURE);
    }

    #[test]
    fn an_out_of_range_cutoff_is_l1111() {
        for bad in ["1.5", "-0.1"] {
            let source = with_keys(&format!("alpha_mode: mask\nalpha_cutoff: {bad}\n"));
            let doc = parse(&source).expect("the structure is fine");
            let diagnostics = validate(&doc);
            let d = diagnostics
                .iter()
                .find(|d| d.path.as_deref() == Some("/alpha_cutoff"))
                .unwrap_or_else(|| panic!("`{bad}` is refused"));
            assert_eq!(d.code, code::MESH_DATA_CONSISTENCY, "{bad}");
        }
        let doc = MeshDocument {
            alpha_mode: AlphaMode::Mask { cutoff: f64::NAN },
            ..parse(CUBE_TRI).unwrap()
        };
        assert!(!validate(&doc).is_empty());
    }

    #[test]
    fn a_cutoff_without_mask_is_l1111() {
        for keys in [
            "alpha_cutoff: 0.3\n",
            "alpha_mode: opaque\nalpha_cutoff: 0.3\n",
        ] {
            let diagnostics = parse(&with_keys(keys)).unwrap_err();
            let d = diagnostics
                .iter()
                .find(|d| d.path.as_deref() == Some("/alpha_cutoff"))
                .expect("the stray cutoff is reported");
            assert_eq!(d.code, code::MESH_DATA_CONSISTENCY);
        }
    }

    /// A texture path may climb with `..` but not leave the content root by an absolute, drive or
    /// device spelling.
    #[test]
    fn a_texture_path_outside_the_content_root_is_l1111() {
        for bad in ["C:/x.png", "/x.png", "lit/nul.png"] {
            let mut doc = parse(CUBE_TRI_TEXTURED).expect("a textured mesh should parse");
            doc.textures.base_color = Some(TextureRef::new(bad));
            let diagnostics = validate(&doc);
            let d = diagnostics
                .iter()
                .find(|d| d.path.as_deref() == Some("/base_color_texture/path"))
                .unwrap_or_else(|| panic!("`{bad}` is refused"));
            assert_eq!(d.code, code::MESH_DATA_CONSISTENCY, "{bad}");
        }
        let mut doc = parse(CUBE_TRI_TEXTURED).expect("a textured mesh should parse");
        doc.textures.base_color = Some(TextureRef::new("../textures/x.png"));
        assert!(validate(&doc).is_empty());
        doc.textures.base_color = Some(TextureRef::new("../textures/nul.png"));
        let diagnostics = validate(&doc);
        assert!(
            diagnostics
                .iter()
                .any(|d| d.path.as_deref() == Some("/base_color_texture/path")),
            "a device name behind `..` is refused"
        );
    }

    #[test]
    fn an_unknown_wrap_mode_is_l1110() {
        let source = CUBE_TRI_TEXTURED.replace("wrap: clamp", "wrap: clmap");
        let diagnostics = parse(&source).unwrap_err();
        let d = diagnostics
            .iter()
            .find(|d| d.path.as_deref() == Some("/normal_texture/wrap"))
            .expect("the bad wrap mode is reported");
        assert_eq!(d.code, code::MESH_DOCUMENT_STRUCTURE);
        assert_eq!(d.help.as_deref(), Some("nearest match is `clamp`"));
    }

    #[test]
    fn a_scalar_key_on_the_wrong_map_is_l1110() {
        let source = CUBE_TRI_TEXTURED.replace("  scale: 0.8\n", "").replace(
            "base_color_texture:\n  path: textures/0123456789abcdef.png\n",
            "base_color_texture:\n  path: textures/0123456789abcdef.png\n  scale: 0.8\n",
        );
        let diagnostics = parse(&source).unwrap_err();
        let d = diagnostics
            .iter()
            .find(|d| d.path.as_deref() == Some("/base_color_texture/scale"))
            .expect("`scale` is not a base-color key");
        assert_eq!(d.code, code::MESH_DOCUMENT_STRUCTURE);
        assert_eq!(d.help.as_deref(), Some("valid keys are: path, wrap"));
    }

    #[test]
    fn a_texture_without_a_path_is_l1110() {
        let source =
            CUBE_TRI_TEXTURED.replace("  path: textures/0123456789abcdef.png\n", "  wrap: clamp\n");
        let diagnostics = parse(&source).unwrap_err();
        let d = diagnostics
            .iter()
            .find(|d| d.path.as_deref() == Some("/base_color_texture"))
            .expect("the missing `path` is reported");
        assert_eq!(d.code, code::MESH_DOCUMENT_STRUCTURE);
        assert!(d.message.contains("`path`"), "{}", d.message);
    }

    #[test]
    fn a_tangent_handedness_other_than_one_is_l1111() {
        let doc = MeshDocument {
            positions: vec![0.0, 0.0, 0.0],
            tangents: Some(vec![1.0, 0.0, 0.0, 0.5]),
            indices: vec![0, 0, 0],
            ..Default::default()
        };
        let diagnostics = validate(&doc);
        assert!(
            diagnostics
                .iter()
                .any(|d| d.code == code::MESH_DATA_CONSISTENCY
                    && d.path.as_deref() == Some("/tangents/3"))
        );
    }

    #[test]
    fn a_zero_length_tangent_or_normal_is_l1111() {
        let doc = MeshDocument {
            positions: vec![0.0, 0.0, 0.0],
            tangents: Some(vec![0.0, 0.0, 0.0, 1.0]),
            normals: Some(vec![0.0, 0.0, 0.0]),
            indices: vec![0, 0, 0],
            ..Default::default()
        };
        let diagnostics = validate(&doc);
        for path in ["/tangents/0", "/normals/0"] {
            assert!(
                diagnostics
                    .iter()
                    .any(|d| d.code == code::MESH_DATA_CONSISTENCY
                        && d.path.as_deref() == Some(path)),
                "{path}: {diagnostics:?}"
            );
        }
        let ok = MeshDocument {
            tangents: Some(vec![1.0, 0.0, 0.0, 1.0]),
            normals: Some(vec![0.0, 0.0, 1.0]),
            ..doc
        };
        assert!(validate(&ok).is_empty());
    }

    #[test]
    fn missing_positions_is_l1110() {
        let diagnostics = parse("format: 1\nkind: mesh\nindices: []\n").unwrap_err();
        assert!(
            diagnostics
                .iter()
                .any(|d| d.code == code::MESH_DOCUMENT_STRUCTURE
                    && d.message.contains("positions"))
        );
    }

    #[test]
    fn bad_kind_is_l1110() {
        let diagnostics =
            parse("format: 1\nkind: tilemap\npositions: []\nindices: []\n").unwrap_err();
        assert_eq!(diagnostics[0].code, code::MESH_DOCUMENT_STRUCTURE);
        assert_eq!(diagnostics[0].path.as_deref(), Some("/kind"));
    }

    #[test]
    fn unknown_key_suggests_nearest() {
        let diagnostics =
            parse("format: 1\nkind: mesh\nposition: []\npositions: []\nindices: []\n").unwrap_err();
        let d = diagnostics
            .iter()
            .find(|d| d.path.as_deref() == Some("/position"))
            .expect("unknown key diagnostic");
        assert_eq!(d.code, code::MESH_DOCUMENT_STRUCTURE);
        assert_eq!(d.help.as_deref(), Some("nearest match is `positions`"));
    }

    #[test]
    fn indices_not_divisible_by_three_is_l1111() {
        let doc = MeshDocument {
            positions: vec![0.0, 0.0, 0.0],
            indices: vec![0, 0],
            ..Default::default()
        };
        let diagnostics = validate(&doc);
        assert!(diagnostics.iter().any(
            |d| d.code == code::MESH_DATA_CONSISTENCY && d.path.as_deref() == Some("/indices")
        ));
    }

    #[test]
    fn out_of_range_index_is_l1111() {
        let doc = MeshDocument {
            positions: vec![0.0, 0.0, 0.0],
            indices: vec![0, 0, 5],
            ..Default::default()
        };
        let diagnostics = validate(&doc);
        assert!(
            diagnostics
                .iter()
                .any(|d| d.code == code::MESH_DATA_CONSISTENCY
                    && d.path.as_deref() == Some("/indices/2"))
        );
    }

    #[test]
    fn non_finite_float_is_l1111() {
        let doc = MeshDocument {
            positions: vec![0.0, 0.0, f64::NAN],
            indices: vec![0, 0, 0],
            ..Default::default()
        };
        let diagnostics = validate(&doc);
        assert!(
            diagnostics
                .iter()
                .any(|d| d.code == code::MESH_DATA_CONSISTENCY
                    && d.path.as_deref() == Some("/positions/2"))
        );
    }

    #[test]
    fn normals_count_mismatch_is_l1111() {
        let doc = MeshDocument {
            positions: vec![0.0, 0.0, 0.0, 1.0, 0.0, 0.0],
            normals: Some(vec![0.0, 1.0, 0.0]),
            indices: vec![0, 0, 1],
            ..Default::default()
        };
        let diagnostics = validate(&doc);
        assert!(diagnostics.iter().any(
            |d| d.code == code::MESH_DATA_CONSISTENCY && d.path.as_deref() == Some("/normals")
        ));
    }

    #[test]
    fn budget_warning_fires_above_threshold_only() {
        let small = MeshDocument {
            positions: vec![0.0; 9],
            indices: vec![0, 1, 2],
            ..Default::default()
        };
        assert!(budget_warning(&small).is_none());

        let big = MeshDocument {
            positions: vec![0.0; (MESH_VERTEX_BUDGET + 1) * 3],
            indices: vec![0, 1, 2],
            ..Default::default()
        };
        let warning = budget_warning(&big).expect("expected a budget warning");
        assert_eq!(warning.code, code::MESH_VERTEX_BUDGET_EXCEEDED);
    }

    #[test]
    fn rest_palette_is_identity_at_bind() {
        let mut identity = [0.0; 16];
        for i in 0..4 {
            identity[i * 5] = 1.0;
        }
        let mut inverse_translate_down = identity;
        inverse_translate_down[13] = -1.0;
        let skin = MeshSkin {
            joints: vec![
                MeshJoint {
                    name: "root".to_string(),
                    parent: None,
                    rest: JointRest {
                        translation: [0.0, 0.0, 0.0],
                        rotation: [0.0, 0.0, 0.0, 1.0],
                        scale: [1.0, 1.0, 1.0],
                    },
                    inverse_bind: identity,
                },
                MeshJoint {
                    name: "child".to_string(),
                    parent: Some(0),
                    rest: JointRest {
                        translation: [0.0, 1.0, 0.0],
                        rotation: [0.0, 0.0, 0.0, 1.0],
                        scale: [1.0, 1.0, 1.0],
                    },
                    inverse_bind: inverse_translate_down,
                },
            ],
            weights: vec![1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0],
            bone_indices: vec![0, 0, 0, 0, 1, 0, 0, 0, 0, 0, 0, 0],
        };
        let palette = skin.rest_palette().unwrap();
        assert_eq!(palette.len(), 2);
        let root = DMat4::from_cols_array(&palette[0]);
        assert!(root.abs_diff_eq(DMat4::IDENTITY, 1e-12), "root {palette:?}");
        let child = DMat4::from_cols_array(&palette[1]);
        assert!(
            child.abs_diff_eq(DMat4::IDENTITY, 1e-12),
            "child {palette:?}"
        );

        let mut skin = skin;
        skin.joints[1].inverse_bind = identity;
        let palette = skin.rest_palette().unwrap();
        let child = DMat4::from_cols_array(&palette[1]);
        let want = DMat4::from_translation(DVec3::new(0.0, 1.0, 0.0));
        assert!(
            child.abs_diff_eq(want, 1e-12),
            "child {palette:?}, want T(0,1,0)"
        );
    }

    const LODS_BLOCK: &str = "\
lods:
  - mesh: crate.lod1.mesh.yaml
    screen_size: 0.25
  - mesh: lods/crate.lod2.mesh.yaml
    screen_size: 0.08
";

    fn with_lods(block: &str) -> String {
        CUBE_TRI.replace("indices:", &format!("{block}indices:"))
    }

    fn lod_joint(name: &str) -> MeshJoint {
        MeshJoint {
            name: name.into(),
            parent: None,
            rest: JointRest {
                translation: [0.0; 3],
                rotation: [0.0, 0.0, 0.0, 1.0],
                scale: [1.0; 3],
            },
            inverse_bind: [
                1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0,
            ],
        }
    }

    #[test]
    fn lods_round_trip_as_a_fixed_point() {
        let source = with_lods(LODS_BLOCK);
        let doc = parse(&source).expect("mesh should parse");
        assert_eq!(doc.lods.len(), 2);
        assert_eq!(doc.lods[1].mesh, "lods/crate.lod2.mesh.yaml");
        assert_eq!(doc.lods[1].screen_size, 0.08);
        assert!(validate(&doc).is_empty());
        assert_eq!(emit(&doc), source);
        let again = parse(&emit(&doc)).expect("emitted mesh should parse");
        assert_eq!(again, doc);
    }

    #[test]
    fn a_mesh_without_lods_emits_no_lods_key() {
        let doc = parse(CUBE_TRI).expect("mesh should parse");
        assert!(doc.lods.is_empty());
        assert!(!emit(&doc).contains("lods"));
    }

    #[test]
    fn a_bad_lod_chain_reports_l1395() {
        let too_many = "lods:\n".to_string()
            + &(1..=4)
                .map(|n| {
                    format!(
                        "  - mesh: c.lod{n}.mesh.yaml\n    screen_size: {}\n",
                        0.5 / f64::from(n)
                    )
                })
                .collect::<String>();
        for bad in [
            too_many.as_str(),
            "lods:\n  - mesh: a.mesh.yaml\n    screen_size: 0.0\n",
            "lods:\n  - mesh: a.mesh.yaml\n    screen_size: -1.0\n",
            "lods:\n  - mesh: a.mesh.yaml\n    screen_size: 0.2\n  - mesh: b.mesh.yaml\n    screen_size: 0.2\n",
            "lods:\n  - mesh: a.mesh.yaml\n    screen_size: 0.2\n  - mesh: b.mesh.yaml\n    screen_size: 0.4\n",
            "lods:\n  - mesh: a.png\n    screen_size: 0.2\n",
            "lods:\n  - mesh: C:/a.mesh.yaml\n    screen_size: 0.2\n",
            "lods:\n  - mesh: /a.mesh.yaml\n    screen_size: 0.2\n",
        ] {
            let source = with_lods(bad);
            let diagnostics = match parse(&source) {
                Ok(doc) => validate(&doc),
                Err(diagnostics) => diagnostics,
            };
            assert!(
                diagnostics.iter().any(|d| d.code == code::MESH_LOD_CHAIN),
                "{bad}: {diagnostics:?}"
            );
        }
    }

    #[test]
    fn a_lod_entry_with_an_unknown_or_missing_key_is_structural() {
        for bad in [
            "lods:\n  - mesh: a.mesh.yaml\n",
            "lods:\n  - mesh: a.mesh.yaml\n    screen_size: 0.2\n    ratio: 0.5\n",
        ] {
            let diagnostics = parse(&with_lods(bad)).expect_err(bad);
            assert!(
                diagnostics.iter().any(|d| d.code == code::MESH_LOD_CHAIN),
                "{bad}"
            );
        }
    }

    #[test]
    fn check_lod_level_accepts_a_matching_level_and_names_each_mismatch() {
        let mut base = parse(CUBE_TRI).expect("mesh should parse");
        base.lods = vec![MeshLodRef {
            mesh: "c.lod1.mesh.yaml".into(),
            screen_size: 0.25,
        }];
        let level = parse(CUBE_TRI).expect("mesh should parse");
        assert!(check_lod_level(&base, &level, 0).is_empty());

        let skin = |joint: &str| MeshSkin {
            joints: vec![lod_joint(joint)],
            weights: [1.0, 0.0, 0.0, 0.0].repeat(3),
            bone_indices: vec![0; 12],
        };
        let mut masked = level.clone();
        masked.alpha_mode = AlphaMode::Mask { cutoff: 0.5 };
        let mut double = level.clone();
        double.double_sided = true;
        let mut rimmed = level.clone();
        rimmed.rim = Some(crate::rim::DocRim {
            color: [1.0, 1.0, 1.0],
            strength: 1.0,
            power: 2.0,
        });
        let mut textured = level.clone();
        textured.textures.base_color = Some(TextureRef::new("t.png"));
        let mut skinned = level.clone();
        skinned.skin = Some(skin("root"));
        let mut skinned_base = base.clone();
        skinned_base.skin = Some(skin("root"));
        let mut renamed = level.clone();
        renamed.skin = Some(skin("other"));

        let cases = [
            ("alpha_mode", &base, masked),
            ("double_sided", &base, double),
            ("rim", &base, rimmed),
            ("base_color_texture", &base, textured),
            ("joints", &base, skinned),
            ("joints", &skinned_base, renamed),
        ];
        for (key, against, bad) in &cases {
            let found = check_lod_level(against, bad, 0);
            assert!(
                found.iter().any(|d| d.code == code::MESH_LOD_LEVEL_MISMATCH
                    && d.message.contains(key)
                    && d.message.contains("c.lod1.mesh.yaml")),
                "{key}: {found:?}"
            );
        }
    }

    #[test]
    fn check_lod_level_refuses_a_nested_chain() {
        let mut base = parse(CUBE_TRI).expect("mesh should parse");
        base.lods = vec![MeshLodRef {
            mesh: "c.lod1.mesh.yaml".into(),
            screen_size: 0.25,
        }];
        let mut level = base.clone();
        level.lods = base.lods.clone();
        let found = check_lod_level(&base, &level, 0);
        assert!(found.iter().any(|d| d.code == code::MESH_LOD_CHAIN));
    }
}
