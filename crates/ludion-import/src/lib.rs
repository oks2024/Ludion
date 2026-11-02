#![forbid(unsafe_code)]
#![warn(missing_docs)]

//! `ludion-import` — a deterministic glTF → canonical-text compiler.
//!
//! [`import`] reads a `.gltf`/`.glb` document and produces an [`Import`] plan: a
//! [`PrefabDocument`] (parts with `Transform`/`Parent`/`Mesh`/`PointLight`), a set of
//! `meshes/*.mesh.yaml` documents, and re-encoded `textures/*.png`. It is a *compiler into the canonical text formats*
//! (Pillar 2): the output is diffable, `check-scene`-validatable, agent-editable text — not an
//! opaque blob. The `ludion import` CLI command (`ludion-cli`) drives it, emits the prefab through
//! the canonical writer with the built-in registry, validates, and writes the tree.
//!
//! # Determinism
//!
//! Same input → byte-identical plan. Everything walks glTF **document order** (nodes, meshes,
//! primitives, images); name deduplication uses stable index suffixes; no hashed-map iteration
//! reaches the output. The scene and mesh docs go through the canonical writers, and textures
//! through [`ludion_assets::encode_png`] — all pure functions of their input.
//!
//! # The mapping (exhaustive — decision 3 of `Plan/phase-8-assets-and-sound.md`, materials per
//! # decision 7 of `Plan/phase-11-pbr.md`)
//!
//! | glTF                                   | Ludion                                  |
//! |----------------------------------------|-----------------------------------------|
//! | node tree (document order)             | parts, `Transform` (TRS) + `Parent`     |
//! | mesh primitive                         | a `meshes/*.mesh.yaml` doc              |
//! | material `baseColorFactor`             | `Mesh.color`                            |
//! | material `roughnessFactor`             | `Mesh.roughness`                        |
//! | material `metallicFactor`              | `Mesh.metallic`                         |
//! | material `emissiveFactor` (see below)  | `Mesh.emissive`                         |
//! | `KHR_materials_emissive_strength`      | multiplies `emissiveFactor` before that mapping |
//! | the five metallic-roughness maps       | the mesh doc's texture references       |
//! | `TANGENT`, or a generated tangent frame| the mesh doc's `tangents`               |
//! | skin (`JOINTS_0`/`WEIGHTS_0`, inverse binds) | the mesh doc's `joints`/`weights`/`bone_indices`; joint nodes folded, not entities |
//! | animation (joint or part channels)     | `clips/<name>.anim.yaml`; part clips listed in the prefab's `clips:` |
//! | `KHR_lights_punctual` point/spot light | `PointLight`/`SpotLight` (`cast_shadows: false`) |
//! | a *referenced* PNG image               | re-encoded `textures/<hash>.png`        |
//!
//! `Mesh.emissive` is a scalar HDR intensity on the surface's own color, not an independent tint
//! (`Plan/phase-11-pbr.md` decision 1), so `emissiveFactor` (× `KHR_materials_emissive_strength`)
//! maps straight only when it *is* one: `[0,0,0]` maps to `0.0`, and a non-zero factor maps to
//! scalar `k` only when it equals `k × baseColorFactor.rgb` within a few ULP per channel (`f32`
//! rounding means an authored-exact proportional factor need not satisfy `c * (e / c) == e`
//! bit-for-bit; see `proportional_emissive` for the bound).
//!
//! Any other factor is a tint, and the importer bakes it. The shader renders `Mesh.color ×
//! Mesh.emissive × emissive map`, so with `r = factor / baseColorFactor.rgb` per channel,
//! `Mesh.emissive` is `max r` and the emissive map is the glTF `emissiveTexture` (or one white
//! texel) times `r / max r` in linear light, at the texture's size. The baked map is an ordinary
//! `textures/<hash>.png`; the untinted source is not written for it. A channel with a zero base
//! and a non-zero factor, or a tint on a primitive without `TEXCOORD_0`, cannot be carried and is an
//! `L1202` naming the material; `--allow-lossy` skips just the emissive (the rest of the material
//! still imports) and warns.
//!
//! The five metallic-roughness maps (`baseColorTexture`, `metallicRoughnessTexture`,
//! `normalTexture`, `occlusionTexture`, `emissiveTexture`) are re-encoded to canonical PNGs named by
//! the content hash of those bytes — identical pixels are one committed file whatever the exporter
//! named them — and referenced from the mesh document, whose paths are relative to its own
//! directory. Only a *referenced* image is written. Tangents come from `TANGENT` when the document
//! carries one, are generated (`tangents.rs`) when a normal map needs one and it does not, and are
//! absent otherwise.
//!
//! A material's `MASK` alpha mode with its `alphaCutoff` (default 0.5), and `doubleSided`, map onto
//! the mesh document's `alpha_mode: mask` / `alpha_cutoff` and `double_sided: true`.
//!
//! Everything else — a texture on a second UV set or with per-axis wrap modes that disagree (a mesh
//! document holds one mode for both), a `BLEND` alpha mode (`MASK` is the mapped alternative), a camera (a prefab carries none), a directional light, a morph target, an
//! animation channel of morph weights, on a node under a skin joint, or mixing skin joints and prefab parts, a node `matrix` that shears, a sparse or non-triangle primitive, a
//! JPEG image, any other required extension — is an `L1202`, naming the offender. By default any `L1202` aborts the import (nothing is written);
//! `--allow-lossy` downgrades them to warnings and compiles the supported subset. Any other
//! *optional* extension in `extensionsUsed` is ignored with the warning `L1239`.
//!
//! A skin (`skin.rs`) maps onto the mesh document of the mesh its node wears: joints are
//! reordered parent-first, a root joint's rest is expressed in the skinned mesh's frame, and each
//! vertex's four weights are renormalized, sorted heaviest first and remapped to joint indices. A
//! vertex with no weight is bound to the root joint and reported as the warning `L1205`. The joint
//! nodes are folded into `joints:` and emitted as no entity, and a non-joint node between two joints
//! folds its transform into the rest of the joint below it. A skin the document cannot carry (over
//! 256 joints, a second joint/weight set, a mesh shared with another node, a node or a mesh, camera
//! or light on a joint) is an `L1202`.
//!
//! An animation (`animation.rs`) becomes one clip whose tracks are named after the skin's joints,
//! or, when its channels sit on nodes that became prefab parts, after those parts (the prefab then
//! lists the clip in its `clips:` map; one animation never mixes the two): keys are reduced,
//! rotations unwrapped to Euler degrees, and the result measured against the source in model space
//! — a clip over its bound is `L1206` (never downgraded), a bad `extras` key `L1207`. See
//! `dev-docs/ludion-import.md`.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use gltf::accessor::{DataType, Dimensions};
use gltf::mesh::{Mode, Semantic};
use gltf::texture::WrappingMode;
use ludion_core::diagnostics::{Diagnostic, Severity};
use ludion_core::math::glam::{DQuat, DVec3};
use ludion_core::math::quat_to_euler_xyz_degrees;
use ludion_scene::{
    AlphaMode, AnimationDocument, AssemblyClip, MeshDocument, MeshSkin, MeshTextures,
    PrefabDocument, SceneEntity, TextureRef, TextureWrap,
};
use serde_json::Value;

mod accessors;
mod animation;
mod convert;
mod diagnostics;
mod document;
mod provenance;
mod skin;
mod sources;
mod tangents;
mod tree;

use accessors::check_accessor;
pub use animation::{ClipReport, JointError, KeyCounts, SeamError, TrackReport};
use convert::{
    flatten2, flatten3, flatten4, mesh_document, mesh_value, parent_value, point_light_value,
    spot_light_value, transform_value,
};
use diagnostics::{
    blend_unsupported, camera_unsupported, compiled_mesh_problem, emissive_without_uvs,
    emissive_zero_base_channel, extension_ignored, invalid_gltf, malformed,
    node_inside_joint_chain, node_matrix_shears, node_under_joint, output_io, png_encode_failed,
    unsupported, unweighted_vertices,
};
use document::check_structure;
pub use provenance::relative_to;
use skin::{SkinBinding, bind_skin, map_weights};
use sources::{Buffers, resolve_image};
pub use tangents::compute_tangents;
use tangents::faceted_normals;
pub use tree::{GltfImported, import_gltf};

/// The glTF features that are an `L1202`, as short phrases. `ludion import --help`, the `L1202`
/// explain text and this crate's rustdoc each name exactly this set (a `ludion-cli` test pins it).
pub const UNSUPPORTED_FEATURES: &[&str] = &[
    "a texture on a second UV set",
    "a camera",
    "a directional light",
    "a BLEND alpha mode",
    "a morph target",
    "a JPEG image",
    "any other required extension",
];

/// A hash of the importer's dependency closure, derived at build time: `ludion sync` re-imports
/// when it changes. See `build/fingerprint.rs`.
pub const IMPORTER_FINGERPRINT: &str = env!("LUDION_IMPORTER_FINGERPRINT");

/// The prefab file written at the root of the output tree.
pub const PREFAB_FILE: &str = "model.prefab.yaml";
/// The subdirectory (under `--out`) mesh documents are written to.
pub const MESH_DIR: &str = "meshes";
/// The subdirectory (under `--out`) animation clips are written to.
pub const CLIP_DIR: &str = "clips";
/// The subdirectory (under `--out`) re-encoded textures are written to.
pub const TEXTURE_DIR: &str = "textures";

/// The extensions we map; any other *required* extension is `L1202`, any other optional one `L1239`.
const SUPPORTED_EXTENSIONS: &[&str] = &["KHR_lights_punctual", "KHR_materials_emissive_strength"];

/// A compiled import plan: the scene plus the asset files it references, ready to write.
///
/// Holding the plan (rather than writing directly) keeps `ludion-import` free of the component
/// registry — the CLI emits the scene through the canonical writer with `builtin_registry` and
/// writes the tree. `meshes`/`textures` are in first-encounter (document) order.
#[derive(Debug)]
pub struct Import {
    /// The prefab document (parts in node-document order, then multi-primitive child parts; a
    /// skin's joint nodes are not parts).
    pub prefab: PrefabDocument,
    /// The mesh asset documents, each with its `meshes/<name>.mesh.yaml` relative path.
    pub meshes: Vec<MeshArtifact>,
    /// The re-encoded textures, each with its `textures/<content hash>.png` relative path.
    pub textures: Vec<TextureArtifact>,
    /// The animation clips, one per importable glTF animation, in document order. The importer
    /// leaves each `generated_from` unset — it sees bytes, not paths.
    pub clips: Vec<ClipArtifact>,
    /// `L1202` diagnostics for features the plan left out (only present in `--allow-lossy` mode; a
    /// non-lossy import with any `L1202` returns `Err` instead), then the `L1205` notes about
    /// skinned vertices with no weight, which are present in both modes.
    pub warnings: Vec<Diagnostic>,
}

impl Import {
    /// Writes the whole output tree under `out_dir`: `model.prefab.yaml` (the caller's canonical
    /// prefab text — emitted through [`ludion_scene::emit_prefab`] with the built-in registry, since
    /// the registry lives above this crate), every `meshes/*.mesh.yaml`, and every `textures/*.png`.
    ///
    /// The plan is complete and internally validated before this is called, so a failure here is
    /// purely a filesystem one. Each file lands whole (temp-then-rename), and the prefab, which
    /// names the rest of the tree, goes last: a kill never leaves a prefab pointing at files that do
    /// not exist.
    ///
    /// # Errors
    /// `L1203` if a directory cannot be created or a file cannot be written.
    pub fn write(&self, out_dir: &Path, prefab_yaml: &str) -> Result<(), Diagnostic> {
        create_dir(out_dir)?;
        if !self.meshes.is_empty() {
            create_dir(&out_dir.join(MESH_DIR))?;
            for mesh in &self.meshes {
                write_file(
                    &out_dir.join(&mesh.rel_path),
                    ludion_scene::emit_mesh(&mesh.doc).as_bytes(),
                )?;
            }
        }
        if !self.clips.is_empty() {
            create_dir(&out_dir.join(CLIP_DIR))?;
            for clip in &self.clips {
                write_file(
                    &out_dir.join(&clip.rel_path),
                    ludion_scene::emit_animation(&clip.doc).as_bytes(),
                )?;
            }
        }
        if !self.textures.is_empty() {
            create_dir(&out_dir.join(TEXTURE_DIR))?;
            for texture in &self.textures {
                write_file(&out_dir.join(&texture.rel_path), &texture.png)?;
            }
        }
        write_file(&out_dir.join(PREFAB_FILE), prefab_yaml.as_bytes())
    }

    /// Files already under `out_dir`'s `meshes/`, `clips/` and `textures/` that this plan would not
    /// write (a previous import's leftovers), as `"<dir>/<name>"` with forward slashes, sorted. A
    /// missing directory contributes nothing.
    pub fn stale_outputs(&self, out_dir: &Path) -> Vec<String> {
        let planned: std::collections::BTreeSet<&str> = self
            .meshes
            .iter()
            .map(|mesh| mesh.rel_path.as_str())
            .chain(self.clips.iter().map(|clip| clip.rel_path.as_str()))
            .chain(
                self.textures
                    .iter()
                    .map(|texture| texture.rel_path.as_str()),
            )
            .collect();
        let mut stale = Vec::new();
        for dir in [MESH_DIR, CLIP_DIR, TEXTURE_DIR] {
            let Ok(entries) = std::fs::read_dir(out_dir.join(dir)) else {
                continue;
            };
            for entry in entries.flatten() {
                if !entry.file_type().is_ok_and(|kind| kind.is_file()) {
                    continue;
                }
                let rel = format!("{dir}/{}", entry.file_name().to_string_lossy());
                if !planned.contains(rel.as_str()) {
                    stale.push(rel);
                }
            }
        }
        stale.sort();
        stale
    }
}

/// Creates `dir` (and any missing parents), mapping failure to `L1203`.
fn create_dir(dir: &Path) -> Result<(), Diagnostic> {
    std::fs::create_dir_all(dir)
        .map_err(|error| output_io(format!("could not create `{}`: {error}", dir.display())))
}

/// Writes `bytes` to `path` atomically, mapping failure to `L1203`.
fn write_file(path: &Path, bytes: &[u8]) -> Result<(), Diagnostic> {
    ludion_core::atomic::write_atomic(path, bytes)
        .map_err(|error| output_io(format!("could not write `{}`: {error}", path.display())))
}

/// One `meshes/*.mesh.yaml` document plus its output-relative path.
#[derive(Debug)]
pub struct MeshArtifact {
    /// The output-relative path, e.g. `meshes/crystal.mesh.yaml`.
    pub rel_path: String,
    /// The parsed, validated mesh document (emit with [`ludion_scene::emit_mesh`]).
    pub doc: MeshDocument,
}

/// One `clips/*.anim.yaml` document plus its output-relative path and the reduction report.
#[derive(Debug)]
pub struct ClipArtifact {
    /// The output-relative path, e.g. `clips/walk.anim.yaml`.
    pub rel_path: String,
    /// The clip document (emit with [`ludion_scene::emit_animation`]).
    pub doc: AnimationDocument,
    /// What reduction did and how close the clip stays to the source.
    pub report: ClipReport,
    /// Whether the clip's tracks name prefab parts (listed in the prefab's `clips:`), not the
    /// joints of a skin.
    pub binds_parts: bool,
}

/// One re-encoded texture plus its output-relative path.
#[derive(Debug)]
pub struct TextureArtifact {
    /// The output-relative path, e.g. `textures/3f9a1c0b7e2d4a56.png` — the content hash of
    /// [`TextureArtifact::png`], so identical pixels are one file whatever the exporter named them.
    pub rel_path: String,
    /// The deterministic PNG bytes ([`ludion_assets::encode_png`]).
    pub png: Vec<u8>,
}

/// Where a glTF's external (non-`data:`, non-GLB-chunk) buffer and image URIs may be read from.
#[derive(Clone, Copy, Debug)]
pub enum ExternalFiles<'a> {
    /// At or below this directory: the input file's own directory.
    Beside(&'a Path),
    /// Nowhere: every source must be embedded.
    Refused,
}

/// Compiles the glTF in `bytes` (a `.gltf` or `.glb`) into an [`Import`] plan. `external` says
/// where external buffer/image URIs may be read from: [`ExternalFiles::Beside`] resolves relative
/// URIs at or below the input file's directory, regular files of at most 512 MiB.
///
/// # Errors
/// - `L1201` if the bytes are not a readable, valid glTF, or a buffer/image source cannot be
///   resolved, or the node graph is not a forest (a node listed as a child twice, or its own
///   ancestor).
/// - `L1111` for a compiled mesh whose data is inconsistent.
/// - `L1204` if an external URI leaves the allowed directory (absolute, prefixed, `..`), names a
///   non-regular or oversized file, or `external` is [`ExternalFiles::Refused`].
/// - `L1202`(s) if the document uses unsupported features **and** `allow_lossy` is false — the
///   returned vector names every offender. With `allow_lossy` true these become
///   [`Import::warnings`] instead and the supported subset compiles.
///
/// # Example
/// ```
/// use std::path::Path;
/// use ludion_import::{ExternalFiles, import};
///
/// // A one-node glTF (an identity transform, no mesh) compiles to a single part.
/// let gltf = br#"{"asset":{"version":"2.0"},"nodes":[{"name":"root"}]}"#;
/// let plan = import(gltf, ExternalFiles::Beside(Path::new(".")), false).unwrap();
/// assert_eq!(plan.prefab.entities.len(), 1);
/// assert!(plan.warnings.is_empty());
/// ```
pub fn import(
    bytes: &[u8],
    external: ExternalFiles<'_>,
    allow_lossy: bool,
) -> Result<Import, Vec<Diagnostic>> {
    let gltf::Gltf { document, blob } = gltf::Gltf::from_slice_without_validation(bytes)
        .map_err(|error| vec![invalid_gltf(format!("could not parse glTF: {error}"))])?;
    check_structure(document.as_json()).map_err(|d| vec![d])?;
    let document = gltf::Document::from_json(document.into_json())
        .map_err(|error| vec![invalid_gltf(format!("invalid glTF: {error}"))])?;
    let document = &document;
    let buffers = Buffers::resolve(document, external, blob.as_deref()).map_err(|d| vec![d])?;

    let mut state = State::default();

    // Document-level unsupported features (named for the rustc bar).
    for extension in document.extensions_required() {
        if !SUPPORTED_EXTENSIONS.contains(&extension) {
            state.lossy(unsupported(
                "a required extension",
                &format!("`{extension}`"),
            ));
        }
    }
    let required: Vec<&str> = document.extensions_required().collect();
    for extension in document.extensions_used() {
        if !SUPPORTED_EXTENSIONS.contains(&extension) && !required.contains(&extension) {
            state.notes.push(extension_ignored(extension));
        }
    }

    let inputs = Inputs {
        buffers: &buffers,
        external,
    };
    let (entities, bindings, part_names) = build_entities(document, &inputs, &mut state)?;
    let clips = match animation::import_animations(
        document,
        &bindings,
        &part_names,
        &buffers,
        &mut state.lossy,
    ) {
        Ok(clips) => clips,
        Err(errors) => {
            // A refused clip is never downgraded, so it fails the import in both modes.
            return Err(if allow_lossy {
                errors
            } else {
                state.lossy.into_iter().chain(errors).collect()
            });
        }
    };

    let prefab = PrefabDocument {
        name: document
            .default_scene()
            .and_then(|s| s.name())
            .map(str::to_string),
        // The importer sees bytes, not paths: the CLI sets the provenance marker.
        generated_from: None,
        clips: part_clip_bindings(&clips),
        entities,
    };

    if !allow_lossy && !state.lossy.is_empty() {
        return Err(state.lossy);
    }
    // `L1202` is the one code whose fatality is a property of the invocation rather than the class,
    // so it is registered fatal and downgraded here — the single point where `--allow-lossy` turns
    // "this import did not happen" into "this import left something out".
    Ok(Import {
        prefab,
        meshes: state.meshes,
        textures: state.textures,
        clips,
        warnings: state
            .lossy
            .into_iter()
            .map(|d| d.with_severity(Severity::Warning))
            .chain(state.notes)
            .collect(),
    })
}

/// The resolved input: every buffer's bytes plus where external URIs may be read from. They travel
/// together because reading a texture needs both (an image may live in a buffer view or at a URI
/// below the `.gltf`'s directory).
struct Inputs<'a> {
    buffers: &'a Buffers,
    external: ExternalFiles<'a>,
}

/// Mutable state threaded through the build: accumulated `L1202`s, and the mesh/texture registries.
#[derive(Default)]
struct State {
    lossy: Vec<Diagnostic>,
    /// Warnings that are not `L1202`s (`L1205`), kept in both modes.
    notes: Vec<Diagnostic>,
    mesh_names: BTreeSet<String>,
    /// Every primitive's resolved outcome, keyed by `(mesh index, primitive index)`. A mesh shared
    /// by several nodes resolves once, so each of its diagnostics is reported exactly once.
    primitives: BTreeMap<(usize, usize), Option<MeshRef>>,
    meshes: Vec<MeshArtifact>,
    /// Every glTF image's outcome, keyed by image index: the emitted texture's output-relative path,
    /// or `None` for an image no mesh document can reference (a JPEG, already reported).
    image_paths: BTreeMap<usize, Option<String>>,
    /// The content hashes already emitted — two images with identical pixels share one file.
    texture_hashes: BTreeSet<u64>,
    /// Baked tinted-emissive maps, keyed by the source image index (`None` for the 1×1 map) and the
    /// hue's bits.
    tinted_paths: BTreeMap<(Option<usize>, [u32; 3]), Option<String>>,
    textures: Vec<TextureArtifact>,
}

impl State {
    fn lossy(&mut self, diagnostic: Diagnostic) {
        self.lossy.push(diagnostic);
    }
}

/// The entities of a build, the skin bindings (keyed by mesh index) and each node's part name
/// (`None` for a node that is no part) the animation pass needs.
type Built = (
    Vec<SceneEntity>,
    BTreeMap<usize, SkinBinding>,
    Vec<Option<String>>,
);

/// The prefab's `clips:` map: one binding per clip whose tracks name parts, by file stem, sorted.
fn part_clip_bindings(clips: &[ClipArtifact]) -> Vec<AssemblyClip> {
    let mut bindings: Vec<AssemblyClip> = clips
        .iter()
        .filter(|clip| clip.binds_parts)
        .map(|clip| AssemblyClip {
            name: clip.doc.name.clone().expect("an imported clip is named"),
            file: clip.rel_path.clone(),
        })
        .collect();
    bindings.sort_by(|a, b| a.name.cmp(&b.name));
    bindings
}

/// Builds one entity per emitted node in document order, then appends one child entity per
/// primitive of every multi-primitive mesh, each parented to its node. A skin's joint nodes are
/// folded into its mesh document and emitted as no entity, so a node's entity position equals its
/// node index only when the document has no skin.
fn build_entities(
    document: &gltf::Document,
    inputs: &Inputs,
    state: &mut State,
) -> Result<Built, Vec<Diagnostic>> {
    let nodes: Vec<gltf::Node> = document.nodes().collect();

    // A node's parent is the node that lists it as a child; check_structure refused any node with two.
    let mut parent_of: Vec<Option<usize>> = vec![None; nodes.len()];
    for node in &nodes {
        for child in node.children() {
            parent_of[child.index()] = Some(node.index());
        }
    }

    let bindings = bind_skins(&nodes, &parent_of, inputs, state)?;
    let emitted = emitted_nodes(&nodes, &parent_of, &bindings, state);
    let mut entity_of: Vec<Option<usize>> = vec![None; nodes.len()];
    for (position, index) in (0..nodes.len()).filter(|&i| emitted[i]).enumerate() {
        entity_of[index] = Some(position);
    }
    let entity_at = |node: usize| entity_of[node].expect("an emitted node's ancestors are emitted");

    // Deduped entity names (documentation for humans/diffs).
    let mut name_used = BTreeSet::new();
    let entity_names: Vec<String> = nodes
        .iter()
        .map(|node| {
            if !emitted[node.index()] {
                return String::new();
            }
            // `/` composes instance paths, so a part name never holds one.
            let base = node
                .name()
                .map(|name| name.replace('/', "_"))
                .unwrap_or_else(|| format!("node-{}", node.index()));
            dedup(&mut name_used, &base, node.index())
        })
        .collect();

    let mut node_entities = Vec::with_capacity(nodes.len());
    let mut child_entities: Vec<SceneEntity> = Vec::new();

    for node in &nodes {
        let index = node.index();
        if !emitted[index] {
            continue;
        }
        let mut components: Vec<(String, Value)> = Vec::new();

        let offender = named("node", index, node.name());
        if let Some(matrix) = skin::node_matrix(node)
            && skin::shears(&matrix)
        {
            state.lossy(node_matrix_shears(&offender));
        }
        let (translation, rotation, scale) = node.transform().decomposed();
        let rotation_deg = quat_to_euler_xyz_degrees(rotation);
        let finite = |values: &[f32], what: &str| {
            require_finite(values, what, &offender).map_err(|d| vec![d])
        };
        finite(&translation, "translation")?;
        finite(&scale, "scale")?;
        finite(
            &rotation_deg,
            "rotation (the quaternion must be unit length)",
        )?;
        components.push((
            "Transform".into(),
            transform_value(translation, rotation_deg, scale),
        ));

        // A single-primitive mesh rides the node; a multi-primitive mesh fans out to children.
        if let Some(mesh) = node.mesh() {
            let skin = bindings.get(&mesh.index());
            let primitives: Vec<gltf::Primitive> = mesh.primitives().collect();
            if primitives.len() == 1 {
                if let Some(reference) = mesh_reference(&mesh, &primitives[0], skin, inputs, state)?
                {
                    components.push(("Mesh".into(), reference.into_mesh_value()));
                }
            } else {
                for primitive in &primitives {
                    if let Some(reference) = mesh_reference(&mesh, primitive, skin, inputs, state)?
                    {
                        let child = SceneEntity {
                            presentation: false,
                            name: Some(dedup(
                                &mut name_used,
                                &format!("{}-{}", entity_names[index], primitive.index()),
                                primitive.index(),
                            )),
                            components: vec![
                                (
                                    "Transform".into(),
                                    transform_value([0.0; 3], [0.0; 3], [1.0; 3]),
                                ),
                                ("Mesh".into(), reference.into_mesh_value()),
                                ("Parent".into(), parent_value(entity_at(index))),
                            ],
                            instance: None,
                        };
                        child_entities.push(child);
                    }
                }
            }
        }

        note_camera(node, state);
        if let Some((name, light)) =
            light_component(node, &nodes, &parent_of, state).map_err(|d| vec![d])?
        {
            components.push((name.into(), light));
        }

        if let Some(parent) = parent_of[index] {
            components.push(("Parent".into(), parent_value(entity_at(parent))));
        }

        node_entities.push(SceneEntity {
            presentation: false,
            name: Some(entity_names[index].clone()),
            components,
            instance: None,
        });
    }

    node_entities.extend(child_entities);
    let part_names = (0..nodes.len())
        .map(|index| emitted[index].then(|| entity_names[index].clone()))
        .collect();
    Ok((node_entities, bindings, part_names))
}

/// Binds every skinned mesh to its skin, keyed by mesh index. A skinned node without a mesh
/// ignores its skin; a mesh worn by several skinned nodes, or by both a skinned and an unskinned
/// one, cannot carry one skin (`L1202` naming the later node, which keeps the first's document).
fn bind_skins(
    nodes: &[gltf::Node],
    parent_of: &[Option<usize>],
    inputs: &Inputs,
    state: &mut State,
) -> Result<BTreeMap<usize, SkinBinding>, Vec<Diagnostic>> {
    let mut bindings = BTreeMap::new();
    let mut skinned_meshes = BTreeSet::new();
    let mut unskinned_meshes = BTreeSet::new();
    for node in nodes {
        let Some(mesh) = node.mesh() else { continue };
        let index = mesh.index();
        let conflict = match node.skin() {
            None => {
                let conflict = skinned_meshes.contains(&index);
                if !conflict {
                    unskinned_meshes.insert(index);
                }
                conflict
            }
            Some(_) => unskinned_meshes.contains(&index) || !skinned_meshes.insert(index),
        };
        if conflict {
            state.lossy(unsupported(
                "a mesh shared by several skinned nodes (or skinned and unskinned)",
                &named("node", node.index(), node.name()),
            ));
            continue;
        }
        if let Some(skin) = node.skin()
            && let Some(binding) = bind_skin(
                &skin,
                node,
                nodes,
                parent_of,
                inputs.buffers,
                &mut state.lossy,
            )?
        {
            bindings.insert(mesh.index(), binding);
        }
    }
    Ok(bindings)
}

/// Which nodes become entities: not a joint of a bound skin (those fold into the mesh document),
/// and not under one — a node parented to a joint has no entity to hang from, so it and its subtree
/// are dropped (`L1202`, once, on the node directly under the joint, as is a mesh, camera or light
/// on a joint).
fn emitted_nodes(
    nodes: &[gltf::Node],
    parent_of: &[Option<usize>],
    bindings: &BTreeMap<usize, SkinBinding>,
    state: &mut State,
) -> Vec<bool> {
    let mut folded = vec![false; nodes.len()];
    for binding in bindings.values() {
        for &joint in &binding.joint_nodes {
            folded[joint] = true;
        }
    }
    let mut emitted = vec![true; nodes.len()];
    for node in nodes {
        let index = node.index();
        let offender = named("node", index, node.name());
        if folded[index] {
            emitted[index] = false;
            for (present, what) in [
                (node.mesh().is_some(), "a mesh on a skin joint"),
                (node.camera().is_some(), "a camera on a skin joint"),
                (node.light().is_some(), "a light on a skin joint"),
            ] {
                if present {
                    state.lossy(unsupported(what, &offender));
                }
            }
            continue;
        }
        let mut current = index;
        while let Some(parent) = parent_of[current] {
            if folded[parent] {
                emitted[index] = false;
                if current == index {
                    state.lossy(if bindings.values().any(|b| b.between.contains(&index)) {
                        node_inside_joint_chain(&offender)
                    } else {
                        node_under_joint(&offender)
                    });
                }
                break;
            }
            current = parent;
        }
    }
    emitted
}

/// A resolved `meshes/*.mesh.yaml` reference plus the material data its `Mesh` component needs
/// (decision 7 of `Plan/phase-11-pbr.md`) — named rather than a tuple, since `clippy::type_
/// complexity` rightly balks at a five-element one threaded through two call sites.
#[derive(Clone)]
struct MeshRef {
    path: String,
    color: [f32; 4],
    roughness: f32,
    metallic: f32,
    emissive: f32,
}

impl MeshRef {
    fn into_mesh_value(self) -> Value {
        mesh_value(
            &self.path,
            self.color,
            self.roughness,
            self.metallic,
            self.emissive,
        )
    }
}

/// Resolves the mesh-asset path for one primitive, plus its material's flat color and the three
/// `Mesh` material scalars (decision 7 of `Plan/phase-11-pbr.md`). `Ok(None)` means the primitive is
/// unsupported (a lossy diagnostic was recorded) and should be skipped.
///
/// A mesh shared by several nodes resolves exactly once: the outcome — including "unsupported" — is
/// cached per primitive, so every diagnostic its material or geometry raises is reported once
/// however many nodes reference it.
fn mesh_reference(
    mesh: &gltf::Mesh,
    primitive: &gltf::Primitive,
    skin: Option<&SkinBinding>,
    inputs: &Inputs,
    state: &mut State,
) -> Result<Option<MeshRef>, Vec<Diagnostic>> {
    let key = (mesh.index(), primitive.index());
    if let Some(cached) = state.primitives.get(&key) {
        return Ok(cached.clone());
    }
    let reference = resolve_primitive(mesh, primitive, skin, inputs, state)?;
    state.primitives.insert(key, reference.clone());
    Ok(reference)
}

/// [`mesh_reference`] on a first encounter: reads the material, builds and registers the mesh
/// document, and names every feature that did not map.
fn resolve_primitive(
    mesh: &gltf::Mesh,
    primitive: &gltf::Primitive,
    skin: Option<&SkinBinding>,
    inputs: &Inputs,
    state: &mut State,
) -> Result<Option<MeshRef>, Vec<Diagnostic>> {
    let material = primitive.material();
    let label = primitive_label(mesh, primitive);
    let pbr = material.pbr_metallic_roughness();

    let color = pbr.base_color_factor();
    let roughness = pbr.roughness_factor();
    let metallic = pbr.metallic_factor();
    let offender = material_label(&material);
    let finite =
        |values: &[f32], what: &str| require_finite(values, what, &offender).map_err(|d| vec![d]);
    finite(&color, "baseColorFactor")?;
    let strength = material.emissive_strength().unwrap_or(1.0);
    finite(&[strength], "KHR_materials_emissive_strength")?;
    let emissive_factor = material.emissive_factor().map(|c| c * strength);
    finite(&emissive_factor, "emissiveFactor")?;
    let has_uvs = primitive.get(&Semantic::TexCoords(0)).is_some();
    let emissive_map = emissive_mapping(
        emissive_factor,
        [color[0], color[1], color[2]],
        has_uvs,
        &offender,
        state,
    );
    let tint = match emissive_map {
        EmissiveMap::Tinted { hue, .. } => Some(hue),
        _ => None,
    };

    let textures = material_textures(&material, &label, tint, inputs, state)?;
    let emissive = match emissive_map {
        EmissiveMap::Scalar(k) => k,
        // The baked map carries the hue; if its slot was refused (already reported) the uniform
        // intensity alone would glow the wrong colour, so the emissive drops with it.
        EmissiveMap::Tinted { intensity, .. } if textures.emissive.is_some() => intensity,
        EmissiveMap::Tinted { .. } | EmissiveMap::Refused => 0.0,
    };
    let alpha_mode = match material.alpha_mode() {
        gltf::material::AlphaMode::Opaque => AlphaMode::Opaque,
        gltf::material::AlphaMode::Blend => {
            state.lossy(blend_unsupported(&label));
            AlphaMode::Opaque
        }
        gltf::material::AlphaMode::Mask => {
            let cutoff = material.alpha_cutoff().unwrap_or(0.5);
            finite(&[cutoff], "alphaCutoff")?;
            if !(0.0..=1.0).contains(&cutoff) {
                return Err(vec![malformed(format!(
                    "{offender}'s alphaCutoff {cutoff} is outside [0, 1]"
                ))]);
            }
            AlphaMode::Mask {
                cutoff: f64::from(cutoff),
            }
        }
    };
    finite(&[roughness], "roughnessFactor")?;
    finite(&[metallic], "metallicFactor")?;

    let Some(doc) = build_mesh_document(mesh, primitive, textures, skin, inputs, state)? else {
        return Ok(None);
    };
    let base = doc
        .name
        .clone()
        .unwrap_or_else(|| format!("mesh-{}", mesh.index()));
    let name = dedup_file_stem(&mut state.mesh_names, &sanitize(&base), mesh.index());
    let rel_path = format!("{MESH_DIR}/{name}.mesh.yaml");
    state.meshes.push(MeshArtifact {
        rel_path: rel_path.clone(),
        doc: MeshDocument {
            // Re-name to the deduped, sanitized filename stem so the doc's `name:` and its path agree.
            name: Some(name),
            alpha_mode,
            double_sided: material.double_sided(),
            ..doc
        },
    });
    Ok(Some(MeshRef {
        path: rel_path,
        color,
        roughness,
        metallic,
        emissive,
    }))
}

/// Reduces a glTF `emissiveFactor` to Ludion's scalar `Mesh.emissive`: an HDR intensity multiplier
/// on the surface's own color, never an independent tint (decision 1 of `Plan/phase-11-pbr.md`).
/// `[0, 0, 0]` maps to `0.0`. Otherwise `emissive` must equal `k × base_color` for some scalar `k`,
/// within [`EMISSIVE_REL_TOLERANCE`] per channel — `None` means no such `k` exists (an independent
/// emission tint, inexpressible here).
///
/// `k` is recovered from the base-color channel with the largest magnitude (dividing by a larger
/// denominator minimizes the relative error carried into `k`), then reapplied to every channel. A
/// zero base-color channel is exempt from that division — it demands an exactly-zero emissive
/// channel instead, since no finite `k` can produce a non-zero emissive from a zero base color.
fn proportional_emissive(emissive: [f32; 3], base_color: [f32; 3]) -> Option<f32> {
    if emissive == [0.0; 3] {
        return Some(0.0);
    }
    let pivot = (0..3)
        .filter(|&i| base_color[i] != 0.0)
        .max_by(|&a, &b| base_color[a].abs().total_cmp(&base_color[b].abs()))?;
    let k = emissive[pivot] / base_color[pivot];
    if !(k.is_finite() && k >= 0.0) {
        return None;
    }
    let exact = emissive
        .iter()
        .zip(base_color.iter())
        .all(|(&e, &c)| exact_channel(e, c, k));
    exact.then_some(k)
}

/// How a material's emissive reaches the mesh document.
#[derive(Clone, Copy)]
enum EmissiveMap {
    /// A scalar intensity on the surface's own colour: no map.
    Scalar(f32),
    /// An independent tint: `intensity` is the HDR scalar, `hue` (each channel in `[0, 1]`) goes
    /// into a baked emissive map.
    Tinted { intensity: f32, hue: [f32; 3] },
    /// Inexpressible (reported); the emissive imports as `0.0`.
    Refused,
}

/// A base-colour channel below one 8-bit step cannot carry a colour, so a tint over it is refused.
const TINT_BASE_EPSILON: f32 = 1.0 / 255.0;

/// Decides the [`EmissiveMap`] for `emissive` (factor × strength) over `base` (`baseColorFactor.rgb`).
///
/// The shader renders `Mesh.color × Mesh.emissive × emissive_map`, so a tinted factor `f` is carried
/// as per-channel ratio `r = f / base`: `emissive = max r`, map hue `= r / max r`. A channel with a
/// zero base and a non-zero factor has no `r`, and a map cannot be sampled without `TEXCOORD_0`.
fn emissive_mapping(
    emissive: [f32; 3],
    base: [f32; 3],
    has_uvs: bool,
    offender: &str,
    state: &mut State,
) -> EmissiveMap {
    if let Some(k) = proportional_emissive(emissive, base) {
        return EmissiveMap::Scalar(k);
    }
    let mut ratio = [0.0_f32; 3];
    for c in 0..3 {
        if emissive[c] == 0.0 {
            continue;
        }
        let r = emissive[c] / base[c];
        if base[c] < TINT_BASE_EPSILON || !(r.is_finite() && r > 0.0) {
            state.lossy(emissive_zero_base_channel(
                offender,
                ["red", "green", "blue"][c],
                emissive[c],
                base[c],
            ));
            return EmissiveMap::Refused;
        }
        ratio[c] = r;
    }
    if !has_uvs {
        state.lossy(emissive_without_uvs(offender));
        return EmissiveMap::Refused;
    }
    let intensity = ratio.into_iter().fold(0.0_f32, f32::max);
    if !(intensity.is_finite() && intensity > 0.0) {
        let c = (0..3)
            .max_by(|&a, &b| ratio[a].total_cmp(&ratio[b]))
            .unwrap_or(0);
        state.lossy(emissive_zero_base_channel(
            offender,
            ["red", "green", "blue"][c],
            emissive[c],
            base[c],
        ));
        return EmissiveMap::Refused;
    }
    EmissiveMap::Tinted {
        intensity,
        hue: ratio.map(|r| r / intensity),
    }
}

/// Bakes an emissive map: `src` (RGBA8; `None` is one opaque white texel) times `hue` per channel,
/// multiplied in linear light and re-encoded sRGB, alpha copied. Returns `(width, height, rgba)`.
fn bake_tinted(src: Option<(u32, u32, &[u8])>, hue: [f32; 3]) -> (u32, u32, Vec<u8>) {
    let (width, height, rgba) = src.unwrap_or((1, 1, &[255, 255, 255, 255]));
    let linear: [f32; 256] = std::array::from_fn(|i| ludion_assets::decode_srgb(i as u8));
    let mut out = Vec::with_capacity(rgba.len());
    for texel in rgba.chunks_exact(4) {
        for c in 0..3 {
            out.push(ludion_assets::encode_srgb(
                linear[usize::from(texel[c])] * hue[c],
            ));
        }
        out.push(texel[3]);
    }
    (width, height, out)
}

/// f32 does not generally satisfy `c * (e / c) == e`: recovering `k` from one channel and
/// reapplying it to another carries the source's own authoring-time rounding (`e = round(c ×
/// k_true)`) plus two more roundings here (the division that recovered `k`, then this
/// multiplication) — each up to 0.5 ULP. `EMISSIVE_REL_TOLERANCE` (8 ULP, i.e. `8 ×
/// f32::EPSILON` relative to the larger magnitude) comfortably covers that short, fixed-length
/// rounding chain while staying far below a genuinely different per-channel ratio: the crate's
/// `rejects_a_non_proportional_emissive` test disagrees by ~50%, ~6 orders of magnitude past this
/// bound.
const EMISSIVE_REL_TOLERANCE: f32 = 8.0 * f32::EPSILON;

fn exact_channel(e: f32, c: f32, k: f32) -> bool {
    if c == 0.0 {
        return e == 0.0;
    }
    let expected = c * k;
    (expected - e).abs() <= EMISSIVE_REL_TOLERANCE * expected.abs().max(e.abs())
}

/// `material N` or `material N ("name")` — the L1202 "where" for a material-level diagnostic (as
/// opposed to `primitive_label`, used for the mesh-pipeline-facing diagnostics that name the
/// primitive; the emissive-proportionality rule is purely a material property, so it names the
/// material directly).
fn material_label(material: &gltf::Material) -> String {
    match material.index() {
        Some(index) => named("material", index, material.name()),
        None => "the default material".to_string(),
    }
}

/// Resolves a material's five metallic-roughness map slots into mesh-document texture references,
/// emitting each referenced image once. A slot with no Ludion mapping is an `L1202` and stays `None`
/// — the rest of the material still imports (`--allow-lossy` is per-feature).
fn material_textures(
    material: &gltf::Material,
    label: &str,
    tint: Option<[f32; 3]>,
    inputs: &Inputs,
    state: &mut State,
) -> Result<MeshTextures, Vec<Diagnostic>> {
    let pbr = material.pbr_metallic_roughness();
    let offender = material_label(material);
    if let Some(info) = material.normal_texture() {
        require_finite(&[info.scale()], "normalTexture scale", &offender).map_err(|d| vec![d])?;
    }
    if let Some(info) = material.occlusion_texture() {
        require_finite(&[info.strength()], "occlusionTexture strength", &offender)
            .map_err(|d| vec![d])?;
    }
    // Canonical slot order (the mesh format's emit order), normalized to `(texture, UV set, the
    // map's own scalar)` — glTF spells the three slot types with three different info structs, and
    // only the normal and occlusion maps carry a scalar.
    let slots = [
        pbr.base_color_texture()
            .map(|info| (info.texture(), info.tex_coord(), 1.0)),
        pbr.metallic_roughness_texture()
            .map(|info| (info.texture(), info.tex_coord(), 1.0)),
        material
            .normal_texture()
            .map(|info| (info.texture(), info.tex_coord(), f64::from(info.scale()))),
        material
            .occlusion_texture()
            .map(|info| (info.texture(), info.tex_coord(), f64::from(info.strength()))),
        material
            .emissive_texture()
            .map(|info| (info.texture(), info.tex_coord(), 1.0)),
    ];

    let mut references: [Option<TextureRef>; 5] = Default::default();
    for (index, (slot, resolved)) in slots.into_iter().zip(references.iter_mut()).enumerate() {
        const EMISSIVE_SLOT: usize = 4;
        if let (EMISSIVE_SLOT, Some(hue)) = (index, tint) {
            let texture = slot.map(|(texture, tex_coord, _)| (texture, tex_coord));
            *resolved = tinted_reference(texture, hue, label, inputs, state)?;
        } else if let Some((texture, tex_coord, factor)) = slot {
            *resolved = texture_reference(&texture, tex_coord, factor, label, inputs, state)?;
        }
    }
    let [base_color, metallic_roughness, normal, occlusion, emissive] = references;
    Ok(MeshTextures {
        base_color,
        metallic_roughness,
        normal,
        occlusion,
        emissive,
    })
}

/// One map slot's reference. `Ok(None)` (with an `L1202`) for a slot Ludion cannot express: a second
/// UV set (a mesh document has one `uvs` list), per-axis wrap modes that disagree (a reference holds
/// one mode for both axes), or a JPEG source.
fn texture_reference(
    texture: &gltf::texture::Texture,
    tex_coord: u32,
    factor: f64,
    label: &str,
    inputs: &Inputs,
    state: &mut State,
) -> Result<Option<TextureRef>, Vec<Diagnostic>> {
    if tex_coord != 0 {
        state.lossy(unsupported("a texture on a second UV set", label));
        return Ok(None);
    }
    let sampler = texture.sampler();
    if sampler.wrap_s() != sampler.wrap_t() {
        state.lossy(unsupported(
            "a sampler with different wrap modes per axis",
            label,
        ));
        return Ok(None);
    }
    let Some(rel_path) = emit_texture(&texture.source(), inputs, state).map_err(|d| vec![d])?
    else {
        return Ok(None);
    };
    Ok(Some(TextureRef {
        // A mesh document's texture path is relative to its own directory, and `MESH_DIR` and
        // `TEXTURE_DIR` are siblings under the output root.
        path: format!("../{rel_path}"),
        wrap: wrap_mode(sampler.wrap_s()),
        factor,
    }))
}

/// The emissive slot of a tinted material: the glTF `emissiveTexture` (when there is one) baked with
/// `hue`, or a 1×1 map of the hue alone. `Ok(None)` (with an `L1202`) under the same refusals as
/// [`texture_reference`].
fn tinted_reference(
    texture: Option<(gltf::texture::Texture, u32)>,
    hue: [f32; 3],
    label: &str,
    inputs: &Inputs,
    state: &mut State,
) -> Result<Option<TextureRef>, Vec<Diagnostic>> {
    let (image, wrap) = match &texture {
        Some((texture, tex_coord)) => {
            if *tex_coord != 0 {
                state.lossy(unsupported("a texture on a second UV set", label));
                return Ok(None);
            }
            let sampler = texture.sampler();
            if sampler.wrap_s() != sampler.wrap_t() {
                state.lossy(unsupported(
                    "a sampler with different wrap modes per axis",
                    label,
                ));
                return Ok(None);
            }
            (Some(texture.source()), wrap_mode(sampler.wrap_s()))
        }
        None => (None, TextureWrap::Repeat),
    };
    let Some(rel_path) =
        emit_tinted_texture(image.as_ref(), hue, inputs, state).map_err(|d| vec![d])?
    else {
        return Ok(None);
    };
    Ok(Some(TextureRef {
        path: format!("../{rel_path}"),
        wrap,
        factor: 1.0,
    }))
}

/// Bakes (once per image and hue) `image` times `hue` into the texture set; `None` bakes the 1×1
/// constant map. `Ok(None)` for a JPEG (`L1202`, reported once per image).
fn emit_tinted_texture(
    image: Option<&gltf::image::Image>,
    hue: [f32; 3],
    inputs: &Inputs,
    state: &mut State,
) -> Result<Option<String>, Diagnostic> {
    let key = (image.map(gltf::image::Image::index), hue.map(f32::to_bits));
    if let Some(cached) = state.tinted_paths.get(&key) {
        return Ok(cached.clone());
    }
    let baked = match image {
        None => bake_tinted(None, hue),
        Some(image) => {
            if let Some(None) = state.image_paths.get(&image.index()) {
                state.tinted_paths.insert(key, None);
                return Ok(None);
            }
            let resolved = resolve_image(image, inputs.buffers, inputs.external)?;
            if !resolved.is_png {
                state.lossy(unsupported(
                    "a JPEG image",
                    &named("image", image.index(), image.name()),
                ));
                state.image_paths.insert(image.index(), None);
                state.tinted_paths.insert(key, None);
                return Ok(None);
            }
            let label = format!("image {}", image.index());
            let (width, height, rgba) = ludion_assets::decode_png(&resolved.bytes, &label)?;
            bake_tinted(Some((width, height, &rgba)), hue)
        }
    };
    let (width, height, rgba) = baked;
    let png = ludion_assets::encode_png(width, height, &rgba)
        .map_err(|detail| png_encode_failed("a baked emissive map", &detail))?;
    let rel_path = register_png(png, state);
    state.tinted_paths.insert(key, Some(rel_path.clone()));
    Ok(Some(rel_path))
}

/// Adds canonical PNG bytes to the plan's texture set under their content hash (a repeat is the
/// same file) and returns the output-relative path.
fn register_png(png: Vec<u8>, state: &mut State) -> String {
    let hash = ludion_assets::content_hash(&png);
    let rel_path = format!("{TEXTURE_DIR}/{hash:016x}.png");
    if state.texture_hashes.insert(hash) {
        state.textures.push(TextureArtifact {
            rel_path: rel_path.clone(),
            png,
        });
    }
    rel_path
}

/// glTF's sampler wrap mode as a mesh document spells it.
fn wrap_mode(mode: WrappingMode) -> TextureWrap {
    match mode {
        WrappingMode::Repeat => TextureWrap::Repeat,
        WrappingMode::ClampToEdge => TextureWrap::Clamp,
        WrappingMode::MirroredRepeat => TextureWrap::Mirror,
    }
}

/// Re-encodes one glTF image into the plan's texture set and returns its output-relative path. The
/// name is the content hash of the canonical PNG bytes, so two images with identical pixels are one
/// committed file whatever the exporter called them. `Ok(None)` for a JPEG (`L1202`).
///
/// Only a *referenced* image ever reaches this: an image no supported map slot names is dead weight
/// in the glTF and is simply not written.
fn emit_texture(
    image: &gltf::image::Image,
    inputs: &Inputs,
    state: &mut State,
) -> Result<Option<String>, Diagnostic> {
    if let Some(cached) = state.image_paths.get(&image.index()) {
        return Ok(cached.clone());
    }
    let resolved = resolve_image(image, inputs.buffers, inputs.external)?;
    if !resolved.is_png {
        state.lossy(unsupported(
            "a JPEG image",
            &named("image", image.index(), image.name()),
        ));
        state.image_paths.insert(image.index(), None);
        return Ok(None);
    }
    let label = format!("image {}", image.index());
    let (width, height, rgba) = ludion_assets::decode_png(&resolved.bytes, &label)?;
    let png = ludion_assets::encode_png(width, height, &rgba)
        .map_err(|detail| png_encode_failed(&label, &detail))?;
    let rel_path = register_png(png, state);
    state
        .image_paths
        .insert(image.index(), Some(rel_path.clone()));
    Ok(Some(rel_path))
}

/// Reads a primitive's geometry into a validated [`MeshDocument`], carrying `textures` and the
/// tangent frame a normal map needs. `Ok(None)` for an unsupported primitive
/// (sparse/non-triangle/morph/no-positions — a lossy diagnostic was recorded).
fn build_mesh_document(
    mesh: &gltf::Mesh,
    primitive: &gltf::Primitive,
    mut textures: MeshTextures,
    skin: Option<&SkinBinding>,
    inputs: &Inputs,
    state: &mut State,
) -> Result<Option<MeshDocument>, Vec<Diagnostic>> {
    let label = primitive_label(mesh, primitive);

    if primitive.mode() != Mode::Triangles {
        state.lossy(unsupported("a non-triangle-list primitive", &label));
        return Ok(None);
    }
    if primitive.morph_targets().len() > 0 {
        state.lossy(unsupported("a morph target", &label));
        return Ok(None);
    }
    if skin.is_some() {
        if primitive
            .attributes()
            .any(|(semantic, _)| matches!(semantic, Semantic::Joints(n) | Semantic::Weights(n) if n >= 1))
        {
            state.lossy(unsupported("a second joint/weight set", &label));
        }
        for (semantic, role) in [
            (Semantic::Joints(0), "JOINTS_0"),
            (Semantic::Weights(0), "WEIGHTS_0"),
        ] {
            if primitive.get(&semantic).is_none() {
                return Err(vec![malformed(format!(
                    "{label} is skinned but has no {role}"
                ))]);
            }
        }
    }
    // Every accessor the importer reads is checked before the reader touches it, which asserts on
    // a stride shorter than its element and has no answer for a component type it does not expect.
    let mut accessors = vec![
        (primitive.get(&Semantic::Positions), "POSITION", VEC3_F32),
        (primitive.get(&Semantic::Normals), "NORMAL", VEC3_F32),
        (
            primitive.get(&Semantic::TexCoords(0)),
            "TEXCOORD_0",
            TEXCOORD_0_TYPES,
        ),
        (primitive.get(&Semantic::Tangents), "TANGENT", VEC4_F32),
        (primitive.indices(), "indices", INDEX_TYPES),
    ];
    if skin.is_some() {
        accessors.push((
            primitive.get(&Semantic::Joints(0)),
            "JOINTS_0",
            JOINTS_0_TYPES,
        ));
        accessors.push((
            primitive.get(&Semantic::Weights(0)),
            "WEIGHTS_0",
            WEIGHTS_0_TYPES,
        ));
    }
    let present = || {
        accessors
            .iter()
            .filter_map(|(accessor, role, allowed)| Some((accessor.as_ref()?, *role, *allowed)))
    };
    if present().any(|(accessor, ..)| accessor.sparse().is_some()) {
        state.lossy(unsupported("a sparse accessor", &label));
        return Ok(None);
    }
    for (accessor, role, allowed) in present() {
        check_accessor(accessor, allowed, role, &label, inputs.buffers).map_err(|d| vec![d])?;
    }

    let reader = primitive.reader(|buffer| inputs.buffers.get(buffer.index()));
    let Some(positions) = reader
        .read_positions()
        .map(|p| flatten3(&p.collect::<Vec<_>>()))
    else {
        state.lossy(unsupported("a primitive without POSITION", &label));
        return Ok(None);
    };
    let normals = reader
        .read_normals()
        .map(|n| flatten3(&n.collect::<Vec<_>>()));
    let uvs = reader
        .read_tex_coords(0)
        .map(|u| flatten2(&u.into_f32().collect::<Vec<_>>()));
    let indices = match reader.read_indices() {
        Some(read) => read.into_u32().collect::<Vec<u32>>(),
        // Non-indexed primitive: a trivial 0..vertex_count triangle list.
        None => (0..(positions.len() / 3) as u32).collect(),
    };

    // A map is sampled at the vertex UVs, so without TEXCOORD_0 there is nowhere to sample from —
    // the maps drop (the rest of the material still imports), never the geometry.
    if uvs.is_none() && textures != MeshTextures::default() {
        state.lossy(unsupported(
            "a textured material on a primitive without TEXCOORD_0",
            &label,
        ));
        textures = MeshTextures::default();
    }

    // An authored TANGENT is imported verbatim (an artist may have baked it to match their normal
    // map).
    let tangents = reader
        .read_tangents()
        .map(|t| flatten4(&t.collect::<Vec<_>>()));

    let name = mesh
        .name()
        .map(str::to_string)
        .unwrap_or_else(|| format!("mesh-{}", mesh.index()));
    let mut doc = mesh_document(name, positions, normals, uvs, tangents, textures, indices);

    if let Some(binding) = skin {
        let joints: Vec<[u16; 4]> = reader
            .read_joints(0)
            .map(|joints| joints.into_u16().collect())
            .expect("JOINTS_0 was checked present");
        let weights: Vec<[f32; 4]> = reader
            .read_weights(0)
            .map(|weights| weights.into_f32().collect())
            .expect("WEIGHTS_0 was checked present");
        let mapped = map_weights(binding, &joints, &weights, &label).map_err(|d| vec![d])?;
        if mapped.unweighted > 0 {
            state.notes.push(unweighted_vertices(
                mapped.unweighted,
                &label,
                &binding.root_name,
            ));
        }
        doc.skin = Some(MeshSkin {
            joints: binding.joints.clone(),
            weights: mapped.weights,
            bone_indices: mapped.bone_indices,
        });
    }

    // The tangent pass indexes by the raw index list, so the data must be consistent before it runs.
    // A normal map with no tangents yet is the pass's own input, not a problem.
    let generating_tangents = doc.tangents.is_none() && doc.textures.normal.is_some();
    let problems: Vec<Diagnostic> = ludion_scene::validate_mesh(&doc)
        .into_iter()
        .filter(|d| !(generating_tangents && d.path.as_deref() == Some("/normal_texture")))
        .collect();
    if !problems.is_empty() {
        return Err(problems
            .into_iter()
            .map(|d| compiled_mesh_problem(d, &label))
            .collect());
    }

    // A normal map without an authored tangent gets a generated frame, since a tangent-space map is
    // unusable without it. No normal map asks for no tangents at all.
    if doc.tangents.is_none() && doc.textures.normal.is_some() {
        // The frame must be orthogonal to the normals this document commits, so a primitive
        // without NORMAL gets the faceted normals written out beside its tangents rather than
        // left to the loader.
        let normals = doc
            .normals
            .get_or_insert_with(|| faceted_normals(&doc.positions, &doc.indices));
        let uvs = doc
            .uvs
            .as_ref()
            .expect("the TEXCOORD_0 check above dropped every map on a primitive without uvs");
        doc.tangents = Some(compute_tangents(&doc.positions, normals, uvs, &doc.indices));
    }

    // The importer builds this; if it is somehow inconsistent that is a hard bug, not a lossy drop.
    let problems = ludion_scene::validate_mesh(&doc);
    if !problems.is_empty() {
        return Err(problems
            .into_iter()
            .map(|d| compiled_mesh_problem(d, &label))
            .collect());
    }
    Ok(Some(doc))
}

const VEC3_F32: &[(Dimensions, DataType)] = &[(Dimensions::Vec3, DataType::F32)];
const VEC4_F32: &[(Dimensions, DataType)] = &[(Dimensions::Vec4, DataType::F32)];
const TEXCOORD_0_TYPES: &[(Dimensions, DataType)] = &[
    (Dimensions::Vec2, DataType::F32),
    (Dimensions::Vec2, DataType::U8),
    (Dimensions::Vec2, DataType::U16),
];
const JOINTS_0_TYPES: &[(Dimensions, DataType)] = &[
    (Dimensions::Vec4, DataType::U8),
    (Dimensions::Vec4, DataType::U16),
];
const WEIGHTS_0_TYPES: &[(Dimensions, DataType)] = &[
    (Dimensions::Vec4, DataType::F32),
    (Dimensions::Vec4, DataType::U8),
    (Dimensions::Vec4, DataType::U16),
];
const INDEX_TYPES: &[(Dimensions, DataType)] = &[
    (Dimensions::Scalar, DataType::U8),
    (Dimensions::Scalar, DataType::U16),
    (Dimensions::Scalar, DataType::U32),
];

/// `L1201` unless every value is finite: a component builder cannot store a NaN or infinity.
/// `offender` names the glTF element and `what` its field.
pub(crate) fn require_finite(values: &[f32], what: &str, offender: &str) -> Result<(), Diagnostic> {
    if values.iter().all(|v| v.is_finite()) {
        Ok(())
    } else {
        Err(malformed(format!("{offender}'s {what} is not finite")))
    }
}

/// A node's camera is `L1202` and left out: a prefab carries no `Camera` (a scene has one), so the
/// node stays a part with its transform only.
fn note_camera(node: &gltf::Node, state: &mut State) {
    if node.camera().is_some() {
        let offender = named("node", node.index(), node.name());
        state.lossy(camera_unsupported(&offender));
    }
}

/// The light component (name and value) for a node's `KHR_lights_punctual` point or spot light
/// (directional → `L1202`, `None`).
fn light_component(
    node: &gltf::Node,
    nodes: &[gltf::Node],
    parent_of: &[Option<usize>],
    state: &mut State,
) -> Result<Option<(&'static str, Value)>, Diagnostic> {
    let Some(light) = node.light() else {
        return Ok(None);
    };
    let offender = named("node", node.index(), node.name());
    let kind = light.kind();
    if matches!(kind, gltf::khr_lights_punctual::Kind::Directional) {
        state.lossy(unsupported("a directional light", &offender));
        return Ok(None);
    }
    // glTF range is optional (infinite if absent); the engine needs a finite range, so an absent
    // one falls back to the components' default (10.0).
    let range = light.range().unwrap_or(10.0);
    let color = light.color();
    require_finite(&color, "light color", &offender)?;
    require_finite(
        &[light.intensity(), range],
        "light intensity or range",
        &offender,
    )?;
    let gltf::khr_lights_punctual::Kind::Spot {
        inner_cone_angle,
        outer_cone_angle,
    } = kind
    else {
        return Ok(Some((
            "PointLight",
            point_light_value(color, light.intensity(), range),
        )));
    };
    require_finite(
        &[inner_cone_angle, outer_cone_angle],
        "spot cone angle",
        &offender,
    )?;
    // glTF allows an outer angle of exactly 90 degrees, which `SpotLight` refuses (`[0, 90)`);
    // it and a wider inner angle are pulled to the nearest valid cone.
    let outer = radians_to_degrees(outer_cone_angle).clamp(0.0, SPOT_MAX_ANGLE_DEG);
    let inner = radians_to_degrees(inner_cone_angle).clamp(0.0, outer);
    let direction = spot_world_direction(node, nodes, parent_of);
    require_finite(&direction, "world rotation", &offender)?;
    Ok(Some((
        "SpotLight",
        spot_light_value(color, light.intensity(), range, direction, inner, outer),
    )))
}

/// The widest half-angle a `SpotLight` accepts, just under the exclusive 90.
const SPOT_MAX_ANGLE_DEG: f32 = 89.9;

fn radians_to_degrees(radians: f32) -> f32 {
    (f64::from(radians) * 180.0 / std::f64::consts::PI) as f32
}

/// The world direction a node's glTF light shines: its world rotation applied to local -Z.
/// `SpotLight.direction` is world-space and no parent turns it, so the whole ancestor chain is
/// composed here (scale ignored; the direction is normalized at extract).
fn spot_world_direction(
    node: &gltf::Node,
    nodes: &[gltf::Node],
    parent_of: &[Option<usize>],
) -> [f32; 3] {
    let local = |n: &gltf::Node| {
        let (_, r, _) = n.transform().decomposed();
        DQuat::from_xyzw(r[0].into(), r[1].into(), r[2].into(), r[3].into()).normalize()
    };
    let mut world = local(node);
    let mut current = node.index();
    while let Some(parent) = parent_of[current] {
        world = local(&nodes[parent]) * world;
        current = parent;
    }
    let d = world * DVec3::NEG_Z;
    // `+ 0.0` turns a -0.0 component into 0.0 so the text stays canonical.
    [d.x as f32 + 0.0, d.y as f32 + 0.0, d.z as f32 + 0.0]
}

/// A stable label for a primitive: its mesh (named or indexed) and, when the mesh has several, the
/// primitive index — the "where" of an `L1202`.
fn primitive_label(mesh: &gltf::Mesh, primitive: &gltf::Primitive) -> String {
    format!(
        "{} primitive {}",
        named("mesh", mesh.index(), mesh.name()),
        primitive.index()
    )
}

/// `kind index` or `kind index (\"name\")` — the human-facing name of a glTF element.
pub(crate) fn named(kind: &str, index: usize, name: Option<&str>) -> String {
    match name {
        Some(name) => format!("{kind} {index} (\"{name}\")"),
        None => format!("{kind} {index}"),
    }
}

/// A unique name from `base`, appending a stable `-<index>` suffix (then `-<index>-<n>`) on
/// collision. Deterministic because callers iterate in document order.
pub(crate) fn dedup(used: &mut BTreeSet<String>, base: &str, index: usize) -> String {
    let base = if base.is_empty() { "unnamed" } else { base };
    if used.insert(base.to_string()) {
        return base.to_string();
    }
    let mut candidate = format!("{base}-{index}");
    let mut n = 2;
    while !used.insert(candidate.clone()) {
        candidate = format!("{base}-{index}-{n}");
        n += 1;
    }
    candidate
}

/// [`dedup`] for a name that becomes a file stem: stems differing only in case collide on
/// case-insensitive file systems, so `used` holds lowercased keys and the result keeps `base`'s spelling.
pub(crate) fn dedup_file_stem(used: &mut BTreeSet<String>, base: &str, index: usize) -> String {
    let base = if base.is_empty() { "unnamed" } else { base };
    if used.insert(base.to_ascii_lowercase()) {
        return base.to_string();
    }
    let mut candidate = format!("{base}-{index}");
    let mut n = 2;
    while !used.insert(candidate.to_ascii_lowercase()) {
        candidate = format!("{base}-{index}-{n}");
        n += 1;
    }
    candidate
}

/// Sanitizes a glTF name into a filesystem-safe stem: anything outside `[A-Za-z0-9._-]` becomes `_`,
/// and a Windows device name gains a `_` before its first dot.
fn sanitize(name: &str) -> String {
    let folded: String = name
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-') {
                c
            } else {
                '_'
            }
        })
        .collect();
    ludion_core::paths::unreserved_stem(&folded)
}

#[cfg(test)]
mod tests {
    use super::{Import, PREFAB_FILE, TextureArtifact, proportional_emissive, sanitize};

    #[test]
    fn a_device_name_never_comes_out_of_sanitize() {
        for (name, expected) in [
            ("CON", "CON_"),
            ("nul.001", "nul_.001"),
            ("Com1", "Com1_"),
            ("Cube", "Cube"),
            ("a b", "a_b"),
        ] {
            assert_eq!(sanitize(name), expected, "{name:?}");
        }
    }

    /// The prefab names the rest of the tree, so it lands last: a texture that cannot be written
    /// leaves no prefab, and nothing is left half-written through `fs::write`.
    #[test]
    fn a_failed_texture_write_leaves_no_prefab() {
        let dir = std::env::temp_dir().join(format!("ludion-import-order-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let blocked = "textures/0123456789abcdef.png";
        std::fs::create_dir_all(dir.join(blocked)).unwrap();
        let import = Import {
            prefab: Default::default(),
            meshes: Vec::new(),
            textures: vec![TextureArtifact {
                rel_path: blocked.to_string(),
                png: vec![1, 2, 3],
            }],
            clips: Vec::new(),
            warnings: Vec::new(),
        };
        import
            .write(&dir, "format: 1\n")
            .expect_err("the texture path is a directory");
        assert!(!dir.join(PREFAB_FILE).exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn stale_outputs_are_the_files_the_plan_would_not_write() {
        let dir = std::env::temp_dir().join(format!("ludion-import-stale-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let import = Import {
            prefab: Default::default(),
            meshes: Vec::new(),
            textures: vec![TextureArtifact {
                rel_path: "textures/kept.png".to_string(),
                png: vec![1],
            }],
            clips: Vec::new(),
            warnings: Vec::new(),
        };
        assert!(
            import.stale_outputs(&dir).is_empty(),
            "missing dirs are fine"
        );
        std::fs::create_dir_all(dir.join("meshes")).unwrap();
        std::fs::create_dir_all(dir.join("textures")).unwrap();
        std::fs::write(dir.join("textures/kept.png"), [1]).unwrap();
        std::fs::write(dir.join("textures/dead.png"), [2]).unwrap();
        std::fs::write(dir.join("meshes/old.mesh.yaml"), "x").unwrap();
        assert_eq!(
            import.stale_outputs(&dir),
            ["meshes/old.mesh.yaml", "textures/dead.png"]
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn zero_emissive_is_zero_regardless_of_base_color() {
        assert_eq!(
            proportional_emissive([0.0, 0.0, 0.0], [0.3, 0.55, 0.9]),
            Some(0.0)
        );
    }

    #[test]
    fn accepts_an_exact_scalar_multiple_of_base_color() {
        // Scaling by an exact power of two (here 0.5) is exact for any f32 base color — the
        // rounding of the decimal literal and the halving commute, so this holds for arbitrary
        // `base_color`, not just "clean" binary fractions (the fixture relies on this fact).
        let base_color = [0.3_f32, 0.55, 0.9];
        let emissive = base_color.map(|c| c * 0.5);
        assert_eq!(proportional_emissive(emissive, base_color), Some(0.5));
    }

    #[test]
    fn accepts_zero_base_color_channels_when_emissive_matches() {
        // A base color of 0 in some channel demands emissive 0 in that channel too.
        let base_color = [0.0_f32, 0.4, 0.0];
        let emissive = [0.0_f32, 0.2, 0.0];
        assert_eq!(proportional_emissive(emissive, base_color), Some(0.5));
    }

    #[test]
    fn rejects_a_non_proportional_emissive() {
        // No single `k` reproduces both channels: 0.2/0.4 = 0.5 but 0.2/0.6 ≈ 0.333.
        assert_eq!(
            proportional_emissive([0.2, 0.2, 0.0], [0.4, 0.6, 0.0]),
            None
        );
    }

    #[test]
    fn rejects_an_independent_tint_against_a_zero_base_channel() {
        // The base color has a zero channel, but the emissive doesn't — no `k` can produce it.
        assert_eq!(
            proportional_emissive([0.1, 0.2, 0.0], [0.0, 0.4, 0.0]),
            None
        );
    }

    #[test]
    fn accepts_a_non_power_of_two_scalar_multiple_of_base_color() {
        // k = 0.3 is not an exact power of two: unlike 0.5, `c * (e / c) == e` fails bit-for-bit
        // for at least one channel even though the factor genuinely is proportional (audit finding
        // 1.7 — the bug this test guards against). All three channels must still recover the same
        // k within tolerance.
        let base_color = [0.3_f32, 0.55, 0.9];
        let emissive = base_color.map(|c| c * 0.3);
        let k = proportional_emissive(emissive, base_color).expect("k = 0.3 is proportional");
        assert!(
            (k - 0.3).abs() < 1e-6,
            "recovered k should be close to 0.3, got {k}"
        );
    }

    #[test]
    fn rejects_a_genuinely_non_proportional_emissive_far_past_tolerance() {
        // Distinguishes "rounding slop" from "actually a different color": these channels differ
        // by ~10%, many orders of magnitude past the few-ULP tolerance for proportional input.
        assert_eq!(
            proportional_emissive([0.30, 0.33, 0.0], [1.0, 1.0, 0.0]),
            None
        );
    }

    #[test]
    fn all_zero_base_color_with_nonzero_emissive_is_rejected() {
        // Every base-color channel is zero, so no finite k can produce the nonzero emissive —
        // this must not divide by zero or otherwise panic.
        assert_eq!(
            proportional_emissive([0.1, 0.0, 0.0], [0.0, 0.0, 0.0]),
            None
        );
    }

    #[test]
    fn picks_the_largest_magnitude_channel_as_the_pivot() {
        // Recovering k from the smallest nonzero base-color channel would amplify rounding error;
        // picking the largest (here index 2, 0.9) keeps the non-power-of-two case exact enough.
        let base_color = [0.01_f32, 0.02, 0.9];
        let emissive = base_color.map(|c| c * 0.3);
        let k = proportional_emissive(emissive, base_color).expect("k = 0.3 is proportional");
        assert!(
            (k - 0.3).abs() < 1e-6,
            "recovered k should be close to 0.3, got {k}"
        );
    }
}
