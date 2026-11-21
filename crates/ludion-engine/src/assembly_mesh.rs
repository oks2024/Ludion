//! Meshing a `kind: assembly` document into per-part renderer-ready [`MeshData`] plus the rest pose
//! that places each part.
//!
//! An assembly is a strict-YAML `*.assembly.yaml` document ([`ludion_scene::check_assembly`]) that
//! owns one palette, places lean `kind: part` files into one assembly-global grid, and declares the
//! part hierarchy (`Plan/voxel-animation.md` decisions 3, 4 and 9). This module is the engine-side
//! seam that turns one into plain per-part vertex data: the assembly sibling of [`crate::voxel_mesh`],
//! with the same `load(root, rel)` shape — read the bytes ([`ludion_assets`]), parse + validate
//! ([`ludion_scene`]), then mesh **each part on its own** through the very same greedy mesher
//! ([`crate::voxel_mesh::mesh`]). One part is one [`MeshData`]; nothing is welded, because each part
//! must be able to move independently.
//!
//! [`mesh_assembly`] is pure and exact: the same documents always yield a bit-identical
//! [`AssemblyData`]. Reading files is [`load`]'s job alone, keeping `ludion-scene` IO-free.
//!
//! # Coordinate frames
//!
//! One voxel is one unit, and a meshed grid has a **bottom-center origin** — the [`crate::voxel_mesh`]
//! convention, pinned by its exact-stream fixtures: cell `(x, y, z)` of a grid sized `[sx, sy, sz]`
//! spans `[x − sx/2, x+1 − sx/2] × [y, y+1] × [z − sz/2, z+1 − sz/2]`. So X and Z are centered on
//! the grid and Y sits on the ground.
//!
//! That convention applies twice here, at two different extents, which is the whole subtlety:
//!
//! - the **assembly** frame is bottom-center of the assembled grid `[SX, SY, SZ]`
//!   ([`ludion_scene::AssemblyDocument::size`]) — the frame the entity's own `Transform` acts in;
//! - a **part mesh** is bottom-center of that part's own grid `[sx, sy, sz]`, because the mesher only
//!   ever sees the one part document.
//!
//! A part's cell `(0, 0, 0)` lands on assembly cell `place`, so the part mesh's origin sits at
//! [`AssemblyPartData::offset`] in the assembly frame:
//!
//! ```text
//! offset = [place.x + sx/2 − SX/2,  place.y,  place.z + sz/2 − SZ/2]
//! ```
//!
//! and a part whose grid *is* the assembled grid, placed at the origin, has `offset` `[0, 0, 0]` —
//! a single-part assembly renders exactly where the equivalent voxel model would.
//!
//! [`AssemblyPartData::pivot`] is the authored pivot expressed **relative to that same part mesh
//! origin**, so posing a part is `translate(offset) · rotate-about(pivot)` with no grid arithmetic
//! left to redo (the clip evaluator in `extract` subtracts the *parent's* offset from a child's, so
//! the chain composes without translating twice):
//!
//! ```text
//! pivot = [pv.x − sx/2,  pv.y,  pv.z − sz/2]
//! ```
//!
//! where `pv` is [`ludion_scene::AssemblyPartEntry::pivot_in_mesh_frame`] — the authored pivot
//! already carried into the mirrored frame for a `mirror: x` part, since
//! [`ludion_scene::assembly_part_voxel`] mirrors the grid it hands the mesher. Mirroring is a pure
//! row reversal, so a mirrored part is ordinary geometry with correct winding and outward normals:
//! there is no negative scale anywhere in this path, and nothing downstream may re-mirror.
//!
//! Halves of grid extents and small integers are exact in `f32`, so both formulas are exact for any
//! realistic model.

use std::sync::Arc;

use ludion_core::codes::code;
use ludion_core::diagnostics::Diagnostic;
use ludion_renderer::MeshData;
use ludion_scene::{
    AnimationDocument, AssemblyDocument, AssemblyJointKind, AssemblyPartEntry, IkHinge, IkKind,
    PartDocument,
};

use crate::asset_cache::{SourceFiles, fold_source_hashes};
use crate::blend_space::{BlendSpaceData, BoundClip, resolve_blends};

/// One meshed part of an assembly: its own geometry plus where that geometry rests and what it turns
/// about. Everything is in voxel units (1 voxel = 1 unit).
#[derive(Clone, Debug, PartialEq)]
pub struct AssemblyPartData {
    /// The part's name inside the assembly — what an animation clip's tracks address it by.
    pub name: String,
    /// The part's own greedy mesh: the assembly's palette over the part's grid, with `mirror: x`
    /// already applied as a grid transform. Its vertices are in the **part mesh frame**
    /// (bottom-center of the part's own grid), not the assembly frame.
    ///
    /// `Arc`-shared because [`extract`](crate::extract()) hands every part straight to
    /// `RenderScene::mesh_assets` on every frame: a clone must be a refcount bump, never a copy of
    /// buffers that run to megabytes (the engine's asset-cache rule for voxel models).
    pub mesh: Arc<MeshData>,
    /// The part's own grid extent in voxels — what bounds a lone `AssemblyPart` entity.
    pub size: [u32; 3],
    /// The rest-pose translation of the part mesh's origin, in the **assembly frame** — see the
    /// module doc's frames section. `[0, 0, 0]` for a part whose grid is the assembled grid placed
    /// at the origin.
    pub offset: [f32; 3],
    /// The rotation pivot, relative to the **part mesh origin** (not the assembly origin, and not
    /// the part's grid corner) — so a pose is `offset · rotate-about(pivot)`. Already in the
    /// mirrored frame for a `mirror: x` part: nothing downstream re-mirrors it.
    pub pivot: [f32; 3],
    /// The parent part's index into [`AssemblyData::parts`], or `None` for the one root part. On a
    /// validated assembly exactly one part is `None` and the links form a tree.
    pub parent: Option<usize>,
}

/// One resolved clip binding: the short name game code selects by, and the parsed clip document.
#[derive(Clone, Debug, PartialEq)]
pub struct AssemblyClipData {
    /// The short name an [`Animation`](crate::Animation) component names
    /// (`Plan/voxel-animation.md` decision 7) — model-agnostic, so one clip file serves every
    /// assembly sharing a part-naming convention.
    pub name: String,
    /// The clip document's path as authored, relative to the assembly file. Kept for diagnostics
    /// and for the reload dependency list; nothing resolves it a second time.
    pub file: String,
    /// The parsed, validated clip.
    pub clip: AnimationDocument,
}

/// A meshed assembly: every part's geometry and rest pose, plus its resolved clips.
#[derive(Clone, Debug, PartialEq)]
pub struct AssemblyData {
    /// The assembled grid's extent in voxels — what bounds the whole assembly.
    pub size: [u32; 3],
    /// The parts, in the assembly document's authored order — the order
    /// [`AssemblyPartData::parent`] indexes into.
    pub parts: Vec<AssemblyPartData>,
    /// The assembly's `clips:` bindings, resolved to parsed documents and sorted by short name (the
    /// assembly document's own canonical order).
    pub clips: Vec<AssemblyClipData>,
    /// The assembly's `joints:` bindings, carried through unresolved (they need no file of their
    /// own) and sorted by child part name (the assembly document's own canonical order). The
    /// ragdoll switch (`crate::ragdoll`) is the one consumer; a plain assembly ignores this field.
    pub joints: Vec<ludion_scene::AssemblyJoint>,
    /// The assembly's `masks:`, sorted by name, each resolved to one weight per part. Masks are pose
    /// data like clips and stay out of the mesh content hash.
    pub masks: Vec<AssemblyMaskData>,
    /// The assembly's `blends:`, sorted by name, each resolved against the bound clips (speed 1, a
    /// clip's own events). Pose data like clips: outside the mesh content hash.
    pub blends: Vec<BlendSpaceData>,
    /// The assembly's `ik:` chains and look-ats, sorted by name, resolved against the parts, masks
    /// and joints. Pose data like clips: outside the mesh content hash.
    pub ik: Vec<AssemblyIkData>,
    /// The `ground:` bounds for `ground: true` chains; `None` when no chain is grounded. Pose data:
    /// outside the mesh content hash.
    pub ground: Option<AssemblyGroundData>,
}

/// The resolved `ground:` block (`Plan/animation-layers.md` decisions 45–48).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct AssemblyGroundData {
    /// The largest foot move up or down, in the assembly's voxels.
    pub max_adjust: f64,
    /// Whether the whole assembly may lower so the lowest foot reaches.
    pub root_drop: bool,
}

/// One `ik:` entry resolved against the assembly (`Plan/animation-layers.md` decision 37).
#[derive(Clone, Debug, PartialEq)]
pub struct AssemblyIkData {
    /// The name an `IkTargets` target selects by.
    pub name: String,
    /// The chain or look-at.
    pub kind: AssemblyIkKind,
}

/// What a resolved `ik:` entry solves.
#[derive(Clone, Debug, PartialEq)]
pub enum AssemblyIkKind {
    /// A two-part chain.
    Chain {
        /// The upper and lower part indices into [`AssemblyData::parts`]; the lower is a child of
        /// the upper.
        parts: [usize; 2],
        /// The effector relative to the lower part's mesh origin (the [`AssemblyPartData::pivot`]
        /// frame, already mirrored for a `mirror: x` part).
        effector: [f64; 3],
        /// The flex axis in the upper part's frame.
        axis: [f64; 3],
        /// `1` or `-1`: the sign of the flex the target's distance asks for.
        bend: f64,
        /// The flex limits in degrees, `[min, max]`; `None` is unlimited.
        limits: Option<[f64; 2]>,
        /// Whether the effector is placed on the static ground under it.
        ground: bool,
    },
    /// A look-at.
    LookAt {
        /// One weight per part from the `aim` mask, in authored order.
        weights: Vec<f32>,
        /// The facing direction in the assembly frame.
        forward: [f64; 3],
        /// The yaw and pitch limits in degrees.
        limits_deg: [f64; 2],
    },
}

/// One part mask resolved against the assembly's parts (`Plan/animation-layers.md` decision 26).
#[derive(Clone, Debug, PartialEq)]
pub struct AssemblyMaskData {
    /// The short name an `AnimationLayer.mask` selects by.
    pub name: String,
    /// One weight per part, in authored order; part `i` is [`AssemblyData::parts`]`[i]`.
    pub weights: Vec<f32>,
}

impl AssemblyData {
    /// The resolved weights of the mask named `name` (one per part), or `None` when this assembly
    /// defines no such mask.
    pub fn mask(&self, name: &str) -> Option<&[f32]> {
        self.masks
            .iter()
            .find(|mask| mask.name == name)
            .map(|mask| mask.weights.as_slice())
    }

    /// The blend space named `name`, or `None` when this assembly defines no such blend.
    pub fn blend(&self, name: &str) -> Option<&BlendSpaceData> {
        self.blends.iter().find(|blend| blend.name == name)
    }

    /// The clip bound under the short `name`, or `None` when this assembly binds no such clip.
    ///
    /// An unbound name is the caller's cue to render the rest pose: a clip name is chosen at
    /// runtime by game logic, so "the name is not one of mine" is a normal state, not an error.
    pub fn clip(&self, name: &str) -> Option<&AnimationDocument> {
        self.clips
            .iter()
            .find(|binding| binding.name == name)
            .map(|binding| &binding.clip)
    }
}

/// Reads, validates, and meshes the assembly document at `root`/`rel`, together with every part file
/// and every clip file it names.
///
/// Each part entry's `file` (and each clip binding's) is resolved **relative to the assembly file's
/// own directory** — the `kit:`/`fragments:` precedent from `kind: map` — and every part is read,
/// parsed, and validated with its diagnostics **accumulated across all parts**, so one run reports
/// every broken part rather than the first. Only when the assembly, every part, and the cross-file
/// checks ([`ludion_scene::check_assembly_parts`]) are all clean is anything meshed; clips are read
/// after that, and accumulate their diagnostics the same way.
///
/// # Errors
/// - `L1101` if the assembly or any part or clip file cannot be read (one diagnostic per unreadable
///   file).
/// - `L1137`/`L1138` for a malformed or inconsistent assembly, including the cross-file unknown-glyph
///   and does-it-fit checks.
/// - `L1139`/`L1140` for a malformed or inconsistent part document, its message naming the part entry
///   and its path.
/// - `L1141`/`L1142` for a malformed or inconsistent clip document, its message naming the binding
///   and its path.
pub fn load(
    root: ludion_assets::AssetRoot<'_>,
    rel: &str,
) -> Result<AssemblyData, Vec<Diagnostic>> {
    load_with_sources(root, rel).map(|(data, _)| data)
}

/// [`load`], additionally reporting **every file it read** — the assembly document first, then each
/// part, then each clip, as `root`-relative paths, each with the content hash of the bytes it
/// decoded. That list is the assembly's reload dependency
/// set: a watcher reports the file that changed, and only something holding this list can tell which
/// assemblies were meshed out of it (see [`crate::asset_cache`]). Because a clip file is on the list,
/// editing a clip re-reads the assembly exactly as editing a part does.
pub(crate) fn load_with_sources(
    root: ludion_assets::AssetRoot<'_>,
    rel: &str,
) -> Result<(AssemblyData, SourceFiles), Vec<Diagnostic>> {
    let (assembly, parts, hash, mut sources) = load_documents(root, rel)?;
    let clips = read_clips(root, rel, &assembly, &mut sources)?;
    Ok((mesh_assembly(&assembly, &parts, &clips, hash), sources))
}

/// Reads, parses, and validates every clip the assembly binds, in the document's own (short-name
/// sorted) order, appending each file to `sources`.
///
/// Diagnostics accumulate across all clips — one run reports every broken clip — and a clip's path is
/// appended to `sources` only when it read, matching [`load_documents`]'s handling of parts.
///
/// Clip bytes deliberately do **not** feed the assembly's content hash: a clip edit changes the pose,
/// never the geometry, so leaving the per-part mesh keys where they are keeps the renderer's GPU
/// buffers instead of re-uploading every limb for a keyframe nudge.
fn read_clips(
    root: ludion_assets::AssetRoot<'_>,
    rel: &str,
    assembly: &AssemblyDocument,
    sources: &mut Vec<(String, u64)>,
) -> Result<Vec<AnimationDocument>, Vec<Diagnostic>> {
    let mut diagnostics = Vec::new();
    let mut clips = Vec::with_capacity(assembly.clips.len());
    for binding in &assembly.clips {
        let file = ludion_core::paths::sibling(rel, &binding.file);
        match read_clip(root, &file, &binding.name) {
            Ok((clip, hash)) => {
                clips.push(clip);
                sources.push((file, hash));
            }
            Err(mut errors) => diagnostics.append(&mut errors),
        }
    }
    if !diagnostics.is_empty() {
        return Err(diagnostics);
    }
    let cross = ludion_scene::check_assembly_blends(assembly, &clips);
    if !cross.is_empty() {
        return Err(cross);
    }
    Ok(clips)
}

/// Reads and validates one clip document at `root`/`rel`.
///
/// Every diagnostic is prefixed with the binding's short name and path, for [`read_part`]'s reason:
/// a clip document's own pointers (`/tracks/arm/rotation/1/t`) say *where in a file* but never
/// *which file*, and an assembly names several. Returns the document with its bytes' content hash.
pub(crate) fn read_clip(
    root: ludion_assets::AssetRoot<'_>,
    rel: &str,
    name: &str,
) -> Result<(AnimationDocument, u64), Vec<Diagnostic>> {
    let label = format!("clip `{name}` (`{rel}`)");
    let bytes = ludion_assets::load_bytes(root, rel).map_err(|diagnostic| vec![diagnostic])?;
    let source = std::str::from_utf8(&bytes.bytes).map_err(|_| {
        vec![
            Diagnostic::new(
                code::ANIMATION_DOCUMENT_STRUCTURE,
                format!("{label} is not valid UTF-8 text"),
            )
            .with_help("a clip document is a strict-YAML text file (`*.anim.yaml`)"),
        ]
    })?;
    let clip = ludion_scene::check_animation(source).map_err(|errors| {
        errors
            .into_iter()
            .map(|mut diagnostic| {
                diagnostic.message = format!("{label}: {}", diagnostic.message);
                diagnostic
            })
            .collect::<Vec<_>>()
    })?;
    Ok((clip, bytes.hash))
}

/// [`load_documents`]'s success value: the parsed assembly, its resolved parts (aligned by index with
/// `assembly.parts`), the assembly's content hash, and the reload dependency list as `(path, content
/// hash)` pairs — aliased so the signature stays readable and `clippy::type_complexity` stays quiet.
pub(crate) type LoadedAssemblyDocuments =
    (AssemblyDocument, Vec<PartDocument>, u64, Vec<(String, u64)>);

/// Reads and validates the assembly document at `root`/`rel` together with every part file it names,
/// stopping short of meshing — the read/parse/cross-validate half shared by `load_with_sources` and
/// [`load_meshed_documents`].
///
/// Returns the parsed assembly, its resolved parts (aligned by index with `assembly.parts`, the
/// [`ludion_scene::check_assembly_parts`] convention), the same content hash `load_with_sources`
/// derives its mesh keys from, and the same reload dependency list.
///
/// # Errors
/// Exactly [`load`]'s: `L1101` per unreadable file, `L1137`/`L1138` for the assembly,
/// `L1139`/`L1140` for a part.
pub(crate) fn load_documents(
    root: ludion_assets::AssetRoot<'_>,
    rel: &str,
) -> Result<LoadedAssemblyDocuments, Vec<Diagnostic>> {
    let bytes = ludion_assets::load_bytes(root, rel).map_err(|diagnostic| vec![diagnostic])?;
    let source = std::str::from_utf8(&bytes.bytes).map_err(|_| {
        vec![
            Diagnostic::new(
                code::ASSEMBLY_DOCUMENT_STRUCTURE,
                format!("assembly asset `{rel}` is not valid UTF-8 text"),
            )
            .with_help("an assembly document is a strict-YAML text file (`*.assembly.yaml`)"),
        ]
    })?;
    let assembly = ludion_scene::check_assembly(source)?;

    let mut diagnostics = Vec::new();
    let mut parts = Vec::with_capacity(assembly.parts.len());
    let mut part_hashes = Vec::with_capacity(assembly.parts.len());
    let mut sources = Vec::with_capacity(assembly.parts.len() + 1);
    sources.push((rel.to_string(), bytes.hash));
    for entry in &assembly.parts {
        let file = ludion_core::paths::sibling(rel, &entry.file);
        match read_part(root, &file, entry) {
            Ok((part, hash)) => {
                parts.push(part);
                part_hashes.push(hash);
                sources.push((file, hash));
            }
            Err(mut errors) => diagnostics.append(&mut errors),
        }
    }
    if !diagnostics.is_empty() {
        return Err(diagnostics);
    }

    let cross = ludion_scene::check_assembly_parts(&assembly, &parts);
    if !cross.is_empty() {
        return Err(cross);
    }

    let hash = fold_source_hashes(bytes.hash, &part_hashes);
    Ok((assembly, parts, hash, sources))
}

/// [`load_meshed_documents`]'s success value: the meshed assembly with the parsed assembly and its
/// resolved parts (aligned by index with `assembly.parts`) it was meshed from.
pub type MeshedAssemblyDocuments = (AssemblyData, AssemblyDocument, Vec<PartDocument>);

/// [`load`], additionally returning the documents the mesh was built from — for a caller that
/// analyses the assembled grid. Each file is read once, so the mesh and the documents cannot
/// disagree about a file that changed between two reads.
///
/// # Errors
/// Exactly [`load`]'s.
pub fn load_meshed_documents(
    root: ludion_assets::AssetRoot<'_>,
    rel: &str,
) -> Result<MeshedAssemblyDocuments, Vec<Diagnostic>> {
    let (assembly, parts, hash, mut sources) = load_documents(root, rel)?;
    let clips = read_clips(root, rel, &assembly, &mut sources)?;
    let data = mesh_assembly(&assembly, &parts, &clips, hash);
    Ok((data, assembly, parts))
}

/// Reads and validates one part document at `root`/`rel`, returning it with its bytes' content hash.
///
/// Every diagnostic is prefixed with the part's name and path, because a part document's own
/// pointers (`/slices/2/0`) say *where in a file* but never *which file*, and an assembly is many
/// files.
fn read_part(
    root: ludion_assets::AssetRoot<'_>,
    rel: &str,
    entry: &AssemblyPartEntry,
) -> Result<(PartDocument, u64), Vec<Diagnostic>> {
    let label = format!("part `{}` (`{rel}`)", entry.name);
    let bytes = ludion_assets::load_bytes(root, rel).map_err(|diagnostic| vec![diagnostic])?;
    let source = std::str::from_utf8(&bytes.bytes).map_err(|_| {
        vec![
            Diagnostic::new(
                code::PART_DOCUMENT_STRUCTURE,
                format!("{label} is not valid UTF-8 text"),
            )
            .with_help("a part document is a strict-YAML text file (`*.part.yaml`)"),
        ]
    })?;
    match ludion_scene::check_part(source) {
        Ok(part) => Ok((part, bytes.hash)),
        Err(errors) => Err(errors
            .into_iter()
            .map(|mut diagnostic| {
                diagnostic.message = format!("{label}: {}", diagnostic.message);
                diagnostic
            })
            .collect()),
    }
}

/// The mesh-cache key for part `index` of an assembly hashed `assembly_hash`.
///
/// Every part of an assembly needs its **own** key: the renderer dedups GPU uploads by
/// [`MeshData::hash`], and two parts of one assembly are legitimately identical documents (a limb
/// file used for both sides), which would otherwise collapse to one mesh. Mixing the index in keeps
/// the key stable per (assembly content, part slot) and distinct between slots.
fn part_mesh_hash(assembly_hash: u64, index: usize) -> u64 {
    let mut bytes = [0u8; 16];
    bytes[..8].copy_from_slice(&assembly_hash.to_le_bytes());
    bytes[8..].copy_from_slice(&(index as u64).to_le_bytes());
    ludion_assets::content_hash(&bytes)
}

/// Meshes every part of `assembly` and composes the rest pose, per the module doc's frames, binding
/// its resolved `clips` by short name.
///
/// `parts` is aligned by index with [`AssemblyDocument::parts`] and `clips` with
/// [`AssemblyDocument::clips`]; all are assumed **caller-validated** —
/// [`ludion_scene::check_assembly`] clean, each part [`ludion_scene::check_part`] clean, each clip
/// [`ludion_scene::check_animation`] clean, and [`ludion_scene::check_assembly_parts`] clean
/// ([`load`] is that caller). `hash` is the assembly's content hash, from which each part's mesh key
/// is derived.
///
/// Pure and exact: the same inputs always yield a bit-identical [`AssemblyData`].
///
/// # Panics
/// If `parts` or `clips` is not the same length as the matching list on `assembly` — silently
/// meshing (or binding) the shorter prefix would drop parts of the model, or clips a game selects
/// by name.
pub fn mesh_assembly(
    assembly: &AssemblyDocument,
    parts: &[PartDocument],
    clips: &[AnimationDocument],
    hash: u64,
) -> AssemblyData {
    assert_eq!(
        assembly.parts.len(),
        parts.len(),
        "one resolved part document per `parts` entry, in the assembly's own order"
    );
    assert_eq!(
        assembly.clips.len(),
        clips.len(),
        "one resolved clip document per `clips` binding, in the assembly's own order"
    );
    let parents = ludion_scene::assembly_parent_indices(assembly);
    let mut meshed = Vec::with_capacity(parts.len());
    for (index, (entry, part)) in assembly.parts.iter().zip(parts).enumerate() {
        let voxel = ludion_scene::assembly_part_voxel(assembly, index, part);
        meshed.push(AssemblyPartData {
            name: entry.name.clone(),
            mesh: Arc::new(crate::voxel_mesh::mesh(&voxel, part_mesh_hash(hash, index))),
            size: part.size,
            offset: rest_offset(assembly.size, entry.place, part.size),
            pivot: pivot_in_part_mesh(entry, part.size),
            parent: parents[index],
        });
    }
    let rig: Vec<(&str, Option<u32>)> = meshed
        .iter()
        .map(|part| (part.name.as_str(), part.parent.map(|p| p as u32)))
        .collect();
    let masks: Vec<AssemblyMaskData> = assembly
        .masks
        .iter()
        .map(|mask| AssemblyMaskData {
            name: mask.name.clone(),
            weights: ludion_scene::resolve_rig_mask(mask, &rig),
        })
        .collect();
    let bound: Vec<BoundClip<'_>> = assembly
        .clips
        .iter()
        .zip(clips)
        .map(|(binding, clip)| BoundClip {
            name: &binding.name,
            duration: clip.duration,
            speed: 1.0,
            events: &clip.events,
        })
        .collect();
    let blends = resolve_blends(&assembly.blends, &bound);
    AssemblyData {
        size: assembly.size,
        parts: meshed,
        clips: assembly
            .clips
            .iter()
            .zip(clips)
            .map(|(binding, clip)| AssemblyClipData {
                name: binding.name.clone(),
                file: binding.file.clone(),
                clip: clip.clone(),
            })
            .collect(),
        joints: assembly.joints.clone(),
        ik: resolve_ik(assembly, &masks, parts),
        ground: assembly.ground.as_ref().map(|g| AssemblyGroundData {
            max_adjust: g.max_adjust,
            root_drop: g.root_drop,
        }),
        masks,
        blends,
    }
}

/// Resolves the `ik:` entries against the meshed parts, masks and joints. An entry naming an
/// unknown part or mask is skipped: validation reports it, and a resolver never sees one.
fn resolve_ik(
    assembly: &AssemblyDocument,
    masks: &[AssemblyMaskData],
    parts: &[PartDocument],
) -> Vec<AssemblyIkData> {
    let index = |name: &str| assembly.parts.iter().position(|p| p.name == name);
    assembly
        .ik
        .iter()
        .filter_map(|entry| {
            let kind = match &entry.kind {
                IkKind::Chain {
                    parts: names,
                    effector,
                    hinge,
                    ground,
                } => {
                    let upper = index(&names[0])?;
                    let lower = index(&names[1])?;
                    let (axis, bend, limits) =
                        resolve_hinge(assembly, hinge.as_ref(), &assembly.parts[lower].name);
                    AssemblyIkKind::Chain {
                        parts: [upper, lower],
                        effector: point_in_part_mesh(
                            &assembly.parts[lower],
                            *effector,
                            parts[lower].size,
                        )
                        .map(f64::from),
                        axis,
                        bend,
                        limits,
                        ground: *ground,
                    }
                }
                IkKind::LookAt {
                    aim,
                    forward,
                    limits_deg,
                } => AssemblyIkKind::LookAt {
                    weights: masks
                        .iter()
                        .find(|mask| &mask.name == aim)
                        .map(|mask| mask.weights.clone())?,
                    forward: *forward,
                    limits_deg: *limits_deg,
                },
            };
            Some(AssemblyIkData {
                name: entry.name.clone(),
                kind,
            })
        })
        .collect()
}

/// A chain's hinge: the authored `hinge:`, else the lower part's `joints:` hinge (its axis and
/// limits; the bend from the naming convention), else the humanoid convention (axis +X; knee −X,
/// elbow +X — `docs/voxel-assemblies.md`).
fn resolve_hinge(
    assembly: &AssemblyDocument,
    authored: Option<&IkHinge>,
    lower: &str,
) -> ([f64; 3], f64, Option<[f64; 2]>) {
    let convention_bend = if lower.starts_with("lower-leg-") {
        -1.0
    } else {
        1.0
    };
    if let Some(hinge) = authored {
        return (hinge.axis, f64::from(hinge.bend), None);
    }
    for joint in &assembly.joints {
        if joint.part == lower
            && let AssemblyJointKind::Hinge { axis, limits } = &joint.kind
        {
            return (*axis, convention_bend, *limits);
        }
    }
    ([1.0, 0.0, 0.0], convention_bend, None)
}

/// The part mesh origin's rest position in the assembly frame — the module doc's `offset` formula.
fn rest_offset(assembly_size: [u32; 3], place: [u32; 3], part_size: [u32; 3]) -> [f32; 3] {
    let half = |extent: u32| f64::from(extent) * 0.5;
    [
        f64::from(place[0]) + half(part_size[0]) - half(assembly_size[0]),
        f64::from(place[1]),
        f64::from(place[2]) + half(part_size[2]) - half(assembly_size[2]),
    ]
    .map(|component| component as f32)
}

/// Every part's socket rest in the assembly frame, by part name: [`AssemblyPartData::offset`] plus
/// [`AssemblyPartData::pivot`], built from the same two functions [`mesh_assembly`] uses, without
/// meshing. `parts` is aligned by index with `assembly.parts`.
pub(crate) fn part_socket_rests(
    assembly: &AssemblyDocument,
    parts: &[PartDocument],
) -> Vec<(String, [f32; 3])> {
    assembly
        .parts
        .iter()
        .zip(parts)
        .map(|(entry, part)| {
            let offset = rest_offset(assembly.size, entry.place, part.size);
            let pivot = pivot_in_part_mesh(entry, part.size);
            (
                entry.name.clone(),
                [
                    offset[0] + pivot[0],
                    offset[1] + pivot[1],
                    offset[2] + pivot[2],
                ],
            )
        })
        .collect()
}

/// The authored pivot relative to the part mesh origin — the module doc's `pivot` formula, taking
/// the mirrored frame from [`AssemblyPartEntry::pivot_in_mesh_frame`] rather than branching here.
fn pivot_in_part_mesh(entry: &AssemblyPartEntry, part_size: [u32; 3]) -> [f32; 3] {
    point_in_part_mesh(entry, entry.pivot, part_size)
}

/// A part-local, unmirrored voxel point relative to the part mesh origin, through
/// [`AssemblyPartEntry::point_in_mesh_frame`].
fn point_in_part_mesh(entry: &AssemblyPartEntry, point: [f64; 3], part_size: [u32; 3]) -> [f32; 3] {
    let [px, py, pz] = entry.point_in_mesh_frame(point, part_size[0]);
    [
        px - f64::from(part_size[0]) * 0.5,
        py,
        pz - f64::from(part_size[2]) * 0.5,
    ]
    .map(|component| component as f32)
}

#[cfg(test)]
mod tests {
    use super::*;
    use ludion_scene::{AnimationTrack, AssemblyClip, Ease, Keyframe, PaletteEntry, VoxelDocument};
    use std::path::PathBuf;

    const RED: [f64; 4] = [1.0, 0.0, 0.0, 1.0];
    const GREEN: [f64; 4] = [0.0, 1.0, 0.0, 1.0];
    const RED_F32: [f32; 4] = [1.0, 0.0, 0.0, 1.0];
    const GREEN_F32: [f32; 4] = [0.0, 1.0, 0.0, 1.0];

    /// A palette entry with a single glyph and an opaque color.
    fn entry(glyph: &str, color: [f64; 4]) -> PaletteEntry {
        PaletteEntry {
            glyph: glyph.to_string(),
            color,
            emissive: None,
            metallic: None,
            porosity: None,
            roughness: None,
        }
    }

    /// A part entry: name, place, and the defaults every fixture overrides explicitly when it cares.
    fn part_entry(name: &str, place: [u32; 3]) -> AssemblyPartEntry {
        AssemblyPartEntry {
            name: name.to_string(),
            file: format!("parts/{name}.part.yaml"),
            parent: None,
            place,
            pivot: [0.0; 3],
            mirror_x: false,
        }
    }

    /// A part document built directly from parts (the mesher is a pure function of the documents, so
    /// the unit fixtures skip the YAML layer). `slices[y][z]` is a row string; column `c` is `x = c`.
    fn part_doc(size: [u32; 3], slices: &[&[&str]]) -> PartDocument {
        PartDocument {
            name: None,
            size,
            slices: slices
                .iter()
                .map(|layer| layer.iter().map(|row| row.to_string()).collect())
                .collect(),
        }
    }

    /// A clip binding naming `anims/<name>.anim.yaml` — the assembly-side half of a `clips:` entry.
    fn clip_binding(name: &str) -> AssemblyClip {
        AssemblyClip {
            name: name.to_string(),
            file: format!("anims/{name}.anim.yaml"),
        }
    }

    /// The resolved side of a `clips:` entry: a one-track clip, built directly (the binder is a pure
    /// function of the documents, so the unit fixtures skip the YAML layer).
    fn clip_doc() -> AnimationDocument {
        AnimationDocument {
            name: Some("fixture-clip".to_string()),
            duration: 1.0,
            looped: true,
            tracks: vec![AnimationTrack {
                target: "arm".to_string(),
                rotation: vec![Keyframe {
                    t: 0.0,
                    v: [0.0, 0.0, 0.0],
                    ease: Ease::Linear,
                }],
                translation: Vec::new(),
                scale: Vec::new(),
            }],
            ..Default::default()
        }
    }

    /// An assembly document built directly from parts, with the two-glyph fixture palette.
    fn assembly_doc(size: [u32; 3], parts: Vec<AssemblyPartEntry>) -> AssemblyDocument {
        AssemblyDocument {
            name: Some("fixture".to_string()),
            size,
            palette: vec![entry("A", RED), entry("B", GREEN)],
            parts,
            clips: Vec::new(),
            joints: Vec::new(),
            masks: Vec::new(),
            blends: Vec::new(),
            ik: Vec::new(),
            ground: None,
        }
    }

    /// One quad's expected data: outward normal, four corner positions (in emission order), color —
    /// the [`crate::voxel_mesh`] fixture shape, so the two modules' pinned streams read alike.
    type Quad = ([f32; 3], [[f32; 3]; 4], [f32; 4]);

    /// Expands hand-authored quads into flat [`MeshData`] arrays with the mesher's fixed indexing
    /// (`0,1,2` and `0,2,3` per quad). The quads carry no authored material, so every vertex takes
    /// the default `(1, 0, 0)`.
    fn expected(quads: &[Quad], hash: u64) -> MeshData {
        let mut positions = Vec::new();
        let mut normals = Vec::new();
        let mut colors = Vec::new();
        let mut materials = Vec::new();
        let mut uvs = Vec::new();
        let mut tangents = Vec::new();
        let mut indices = Vec::new();
        for (normal, corners, color) in quads {
            let base = (positions.len() / 3) as u32;
            for corner in corners {
                positions.extend_from_slice(corner);
                normals.extend_from_slice(normal);
                colors.extend_from_slice(color);
                materials.extend_from_slice(&[1.0, 0.0, 0.0]);
                uvs.extend_from_slice(&[0.0, 0.0]);
                tangents.extend_from_slice(&[1.0, 0.0, 0.0, 1.0]);
            }
            indices.extend_from_slice(&[base, base + 1, base + 2, base, base + 2, base + 3]);
        }
        let vertex_count = positions.len() / 3;
        MeshData {
            positions,
            normals,
            colors,
            materials,
            porosity: vec![ludion_renderer::DEFAULT_POROSITY; vertex_count],
            uvs,
            tangents,
            weights: [1.0, 0.0, 0.0, 0.0].repeat(vertex_count),
            bone_indices: vec![0; vertex_count * 4],
            skin_joints: Vec::new(),
            textures: None,
            authored_material: false,
            base_color_mean: [1.0; 4],
            rim: None,
            double_sided: false,
            indices,
            hash,
            lods: Vec::new(),
        }
    }

    /// The six faces of the unit box spanning `[x, x+1] × [y, y+1] × [z, z+1]`, in the mesher's
    /// pinned −X, +X, −Y, +Y, −Z, +Z order with its per-face winding. Hand-derived from
    /// `voxel_mesh::tests::single_voxel_exact_stream`'s corner values (which pin the box at
    /// `x, z ∈ ±0.5`, `y ∈ [0, 1]`), translated — so this helper is that fixture's arithmetic, not
    /// the mesher's.
    fn unit_box(origin: [f32; 3], color: [f32; 4]) -> Vec<Quad> {
        let [x, y, z] = origin;
        let (x1, y1, z1) = (x + 1.0, y + 1.0, z + 1.0);
        vec![
            (
                [-1.0, 0.0, 0.0],
                [[x, y, z], [x, y, z1], [x, y1, z1], [x, y1, z]],
                color,
            ),
            (
                [1.0, 0.0, 0.0],
                [[x1, y, z], [x1, y1, z], [x1, y1, z1], [x1, y, z1]],
                color,
            ),
            (
                [0.0, -1.0, 0.0],
                [[x, y, z], [x1, y, z], [x1, y, z1], [x, y, z1]],
                color,
            ),
            (
                [0.0, 1.0, 0.0],
                [[x, y1, z], [x, y1, z1], [x1, y1, z1], [x1, y1, z]],
                color,
            ),
            (
                [0.0, 0.0, -1.0],
                [[x, y, z], [x, y1, z], [x1, y1, z], [x1, y, z]],
                color,
            ),
            (
                [0.0, 0.0, 1.0],
                [[x, y, z1], [x1, y, z1], [x1, y1, z1], [x, y1, z1]],
                color,
            ),
        ]
    }

    /// The two-part rest pose, pinned exactly: each part's full vertex stream, and the `offset`,
    /// `pivot` and `parent` the wave-2 evaluator inherits.
    ///
    /// The assembly is 4×3×2; `body` is a 2×1×1 red bar at the origin, `arm` a single green cell
    /// placed at (3, 1, 1) with pivot (1.0, 0.5, 0.0) and parented to `body`.
    #[test]
    fn two_part_rest_pose_exact_stream() {
        let mut arm = part_entry("arm", [3, 1, 1]);
        arm.parent = Some("body".to_string());
        arm.pivot = [1.0, 0.5, 0.0];
        let assembly = assembly_doc([4, 3, 2], vec![part_entry("body", [0, 0, 0]), arm]);
        let parts = [
            part_doc([2, 1, 1], &[&["AA"]]),
            part_doc([1, 1, 1], &[&["B"]]),
        ];
        let data = mesh_assembly(&assembly, &parts, &[], 0x1234);

        assert_eq!(data.parts.len(), 2);
        assert_eq!(data.clips, Vec::new());

        // `body`: the 2×1×1 bar meshed in its own frame — bottom-center of a 2×1×1 grid, so the box
        // spans x ∈ [−1, 1] and the ±Y/±Z faces merge across both cells.
        let body = &data.parts[0];
        assert_eq!(body.name, "body");
        let bar: Vec<Quad> = vec![
            (
                [-1.0, 0.0, 0.0],
                [
                    [-1.0, 0.0, -0.5],
                    [-1.0, 0.0, 0.5],
                    [-1.0, 1.0, 0.5],
                    [-1.0, 1.0, -0.5],
                ],
                RED_F32,
            ),
            (
                [1.0, 0.0, 0.0],
                [
                    [1.0, 0.0, -0.5],
                    [1.0, 1.0, -0.5],
                    [1.0, 1.0, 0.5],
                    [1.0, 0.0, 0.5],
                ],
                RED_F32,
            ),
            (
                [0.0, -1.0, 0.0],
                [
                    [-1.0, 0.0, -0.5],
                    [1.0, 0.0, -0.5],
                    [1.0, 0.0, 0.5],
                    [-1.0, 0.0, 0.5],
                ],
                RED_F32,
            ),
            (
                [0.0, 1.0, 0.0],
                [
                    [-1.0, 1.0, -0.5],
                    [-1.0, 1.0, 0.5],
                    [1.0, 1.0, 0.5],
                    [1.0, 1.0, -0.5],
                ],
                RED_F32,
            ),
            (
                [0.0, 0.0, -1.0],
                [
                    [-1.0, 0.0, -0.5],
                    [-1.0, 1.0, -0.5],
                    [1.0, 1.0, -0.5],
                    [1.0, 0.0, -0.5],
                ],
                RED_F32,
            ),
            (
                [0.0, 0.0, 1.0],
                [
                    [-1.0, 0.0, 0.5],
                    [1.0, 0.0, 0.5],
                    [1.0, 1.0, 0.5],
                    [-1.0, 1.0, 0.5],
                ],
                RED_F32,
            ),
        ];
        assert_eq!(*body.mesh, expected(&bar, part_mesh_hash(0x1234, 0)));
        // place 0 + sx/2 (1) − SX/2 (2) = −1 on x; place 0 on y; 0 + sz/2 (0.5) − SZ/2 (1) = −0.5.
        assert_eq!(body.offset, [-1.0, 0.0, -0.5]);
        // The default pivot (the part's grid corner) is the mesh origin's own corner.
        assert_eq!(body.pivot, [-1.0, 0.0, -0.5]);
        assert_eq!(body.parent, None);

        // `arm`: one cell, so its mesh is the unit box centered on its own 1×1×1 grid.
        let arm = &data.parts[1];
        assert_eq!(arm.name, "arm");
        assert_eq!(
            *arm.mesh,
            expected(
                &unit_box([-0.5, 0.0, -0.5], GREEN_F32),
                part_mesh_hash(0x1234, 1)
            )
        );
        // place 3 + 0.5 − 2 = 1.5 on x; place 1 on y; 1 + 0.5 − 1 = 0.5 on z.
        assert_eq!(arm.offset, [1.5, 1.0, 0.5]);
        // pivot (1.0, 0.5, 0.0) − (sx/2, 0, sz/2) = (0.5, 0.5, −0.5).
        assert_eq!(arm.pivot, [0.5, 0.5, -0.5]);
        assert_eq!(arm.parent, Some(0));
    }

    /// One asymmetric part file used twice — plain and `mirror: x` — pins that the mirrored part is
    /// ordinary geometry: the exact same six faces with the same winding and outward normals as the
    /// unmirrored one, only in the reflected grid cell. A negative scale would flip the winding, so
    /// this stream is the proof it never happens. The pivot reflects with the geometry.
    #[test]
    fn mirrored_part_is_ordinary_geometry() {
        let mut left = part_entry("limb-l", [0, 0, 0]);
        left.pivot = [0.5, 0.0, 0.5];
        let mut right = part_entry("limb-r", [2, 0, 0]);
        right.parent = Some("limb-l".to_string());
        right.pivot = [0.5, 0.0, 0.5];
        right.mirror_x = true;
        let assembly = assembly_doc([4, 1, 1], vec![left, right]);
        // The part is asymmetric: filled at x = 0, empty at x = 1.
        let limb = part_doc([2, 1, 1], &[&["A."]]);
        let data = mesh_assembly(&assembly, &[limb.clone(), limb], &[], 0x99);

        // Unmirrored: the filled cell is x = 0 of a 2-wide grid, so the box spans x ∈ [−1, 0].
        let plain = &data.parts[0];
        assert_eq!(
            *plain.mesh,
            expected(
                &unit_box([-1.0, 0.0, -0.5], RED_F32),
                part_mesh_hash(0x99, 0)
            )
        );
        assert_eq!(plain.offset, [-1.0, 0.0, 0.0]);
        // pivot 0.5 − sx/2 (1) = −0.5 on x.
        assert_eq!(plain.pivot, [-0.5, 0.0, 0.0]);
        assert_eq!(plain.parent, None);

        // Mirrored: the row reverses, so the filled cell is x = 1 and the box spans x ∈ [0, 1] —
        // same six quads, same order, same winding, shifted.
        let mirrored = &data.parts[1];
        assert_eq!(
            *mirrored.mesh,
            expected(
                &unit_box([0.0, 0.0, -0.5], RED_F32),
                part_mesh_hash(0x99, 1)
            )
        );
        assert_eq!(mirrored.offset, [1.0, 0.0, 0.0]);
        // The mirrored-frame pivot is sx − 0.5 = 1.5, so 1.5 − 1 = 0.5: the plain pivot reflected
        // about the part's own center, not the authored value reused.
        assert_eq!(mirrored.pivot, [0.5, 0.0, 0.0]);
        assert_eq!(mirrored.parent, Some(0));
    }

    fn ik_rig(lower: &str, mirror: bool, hinge: Option<IkHinge>) -> AssemblyDocument {
        let upper = part_entry("upper-arm-r", [0, 0, 0]);
        let mut low = part_entry(lower, [2, 0, 0]);
        low.parent = Some("upper-arm-r".to_string());
        low.mirror_x = mirror;
        let mut doc = assembly_doc([4, 1, 1], vec![upper, low]);
        doc.ik = vec![ludion_scene::IkEntry {
            name: "hand".to_string(),
            kind: IkKind::Chain {
                parts: ["upper-arm-r".to_string(), lower.to_string()],
                effector: [1.5, 0.0, 0.5],
                hinge,
                ground: false,
            },
        }];
        doc
    }

    fn resolved_chain(doc: &AssemblyDocument) -> ([f64; 3], [f64; 3], f64, Option<[f64; 2]>) {
        let limb = part_doc([2, 1, 1], &[&["AA"]]);
        let data = mesh_assembly(doc, &[limb.clone(), limb], &[], 1);
        let AssemblyIkKind::Chain {
            effector,
            axis,
            bend,
            limits,
            ..
        } = data.ik[0].kind.clone()
        else {
            panic!("a chain");
        };
        (effector, axis, bend, limits)
    }

    #[test]
    fn a_mirrored_part_resolves_its_effector_in_the_mesh_frame() {
        let (plain, ..) = resolved_chain(&ik_rig("lower-arm-r", false, None));
        // 1.5 − sx/2 (1) on x, 0.5 − sz/2 (0.5) on z.
        assert_eq!(plain, [0.5, 0.0, 0.0]);
        let (mirrored, ..) = resolved_chain(&ik_rig("lower-arm-r", true, None));
        // The mirrored-frame point is sx − 1.5 = 0.5, so 0.5 − 1 = −0.5.
        assert_eq!(mirrored, [-0.5, 0.0, 0.0]);
    }

    #[test]
    fn the_hinge_comes_from_the_entry_then_the_joint_then_the_convention() {
        let authored = IkHinge {
            axis: [0.0, 0.0, 1.0],
            bend: -1,
        };
        let (_, axis, bend, limits) = resolved_chain(&ik_rig("lower-arm-r", false, Some(authored)));
        assert_eq!((axis, bend, limits), ([0.0, 0.0, 1.0], -1.0, None));

        let mut jointed = ik_rig("lower-arm-r", false, None);
        jointed.joints = vec![ludion_scene::AssemblyJoint {
            part: "lower-arm-r".to_string(),
            kind: AssemblyJointKind::Hinge {
                axis: [0.0, 1.0, 0.0],
                limits: Some([0.0, 120.0]),
            },
            break_impulse: None,
        }];
        let (_, axis, bend, limits) = resolved_chain(&jointed);
        assert_eq!(
            (axis, bend, limits),
            ([0.0, 1.0, 0.0], 1.0, Some([0.0, 120.0]))
        );

        let (_, axis, bend, limits) = resolved_chain(&ik_rig("lower-arm-r", false, None));
        assert_eq!((axis, bend, limits), ([1.0, 0.0, 0.0], 1.0, None));
        let (_, axis, bend, _) = resolved_chain(&ik_rig("lower-leg-r", false, None));
        assert_eq!((axis, bend), ([1.0, 0.0, 0.0], -1.0));
    }

    /// A one-part assembly whose grid *is* the assembled grid, placed at the origin, meshes exactly
    /// like the equivalent voxel model and rests at the origin — so converting a voxel model into a
    /// one-part assembly changes nothing about what is drawn or where.
    #[test]
    fn single_part_assembly_equals_the_voxel_model() {
        let slices: &[&[&str]] = &[&["AB.", "BAA"], &[".A.", "B.B"]];
        let assembly = assembly_doc([3, 2, 2], vec![part_entry("body", [0, 0, 0])]);
        let part = part_doc([3, 2, 2], slices);
        let data = mesh_assembly(&assembly, &[part], &[], 0xFEED);

        let voxel = VoxelDocument {
            name: Some("body".to_string()),
            size: [3, 2, 2],
            scale: None,
            generated_from: None,
            budget_exempt: None,
            palette: assembly.palette.clone(),
            slices: slices
                .iter()
                .map(|layer| layer.iter().map(|row| row.to_string()).collect())
                .collect(),
            rim: None,
            lods: Vec::new(),
        };
        assert_eq!(
            *data.parts[0].mesh,
            crate::voxel_mesh::mesh(&voxel, part_mesh_hash(0xFEED, 0))
        );
        assert_eq!(data.parts[0].offset, [0.0, 0.0, 0.0]);
    }

    /// Meshing is pure: the same documents yield a bit-identical [`AssemblyData`], whole struct.
    #[test]
    fn double_run_is_bit_identical() {
        let mut arm = part_entry("arm", [2, 1, 0]);
        arm.parent = Some("body".to_string());
        arm.pivot = [0.5, 1.5, 0.5];
        arm.mirror_x = true;
        let mut assembly = assembly_doc([4, 3, 2], vec![part_entry("body", [0, 0, 0]), arm]);
        assembly.clips = vec![clip_binding("walk")];
        let parts = [
            part_doc([2, 2, 2], &[&["AB", "BA"], &[".A", "B."]]),
            part_doc([2, 2, 1], &[&["A."], &[".B"]]),
        ];
        let clips = [clip_doc()];
        assert_eq!(
            mesh_assembly(&assembly, &parts, &clips, 0xC0FFEE),
            mesh_assembly(&assembly, &parts, &clips, 0xC0FFEE)
        );
    }

    /// The sim-side socket rest equals the meshed `offset + pivot` per part, mirrored part included.
    #[test]
    fn socket_rests_equal_the_meshed_offset_plus_pivot() {
        let mut arm = part_entry("arm", [2, 1, 0]);
        arm.parent = Some("body".to_string());
        arm.pivot = [0.5, 1.5, 0.5];
        arm.mirror_x = true;
        let assembly = assembly_doc([4, 3, 2], vec![part_entry("body", [0, 0, 0]), arm]);
        let parts = [
            part_doc([2, 2, 2], &[&["AB", "BA"], &[".A", "B."]]),
            part_doc([2, 2, 1], &[&["A."], &[".B"]]),
        ];
        let data = mesh_assembly(&assembly, &parts, &[], 1);
        let rests = part_socket_rests(&assembly, &parts);
        assert_eq!(rests.len(), data.parts.len());
        for ((name, rest), part) in rests.iter().zip(&data.parts) {
            assert_eq!(name, &part.name);
            let expected: [f32; 3] = std::array::from_fn(|i| part.offset[i] + part.pivot[i]);
            assert_eq!(*rest, expected, "part `{name}`");
        }
    }

    /// Two identical part documents in one assembly still get distinct mesh hashes — the renderer
    /// dedups GPU uploads by [`MeshData::hash`], so a shared key would collapse two parts into one
    /// drawable that cannot move independently. The clip bindings ride through, resolved, and
    /// `clip()` answers by short name (an unbound name is `None` — the rest-pose degrade).
    #[test]
    fn identical_parts_get_distinct_mesh_hashes() {
        let mut right = part_entry("limb-r", [2, 0, 0]);
        right.parent = Some("limb-l".to_string());
        let mut assembly = assembly_doc([4, 1, 1], vec![part_entry("limb-l", [0, 0, 0]), right]);
        assembly.clips = vec![clip_binding("attack")];
        let limb = part_doc([2, 1, 1], &[&["AA"]]);
        let data = mesh_assembly(&assembly, &[limb.clone(), limb], &[clip_doc()], 0x2024);

        assert_ne!(data.parts[0].mesh.hash, data.parts[1].mesh.hash);
        // Same geometry, different key: only the hash separates them.
        assert_eq!(data.parts[0].mesh.positions, data.parts[1].mesh.positions);
        assert_eq!(data.clips[0].name, "attack");
        assert_eq!(data.clips[0].file, "anims/attack.anim.yaml");
        assert_eq!(data.clip("attack"), Some(&clip_doc()));
        assert_eq!(data.clip("walk"), None, "an unbound name means rest pose");
    }

    /// A different part file changes the assembly hash, and with it every part's mesh key — what
    /// makes a hot-reloaded assembly invalidate the renderer's cache.
    #[test]
    fn a_changed_part_file_moves_every_mesh_key() {
        let base = fold_source_hashes(0x11, &[0xAA, 0xBB]);
        assert_eq!(base, fold_source_hashes(0x11, &[0xAA, 0xBB]));
        assert_ne!(base, fold_source_hashes(0x11, &[0xAA, 0xBC]));
        assert_ne!(base, fold_source_hashes(0x12, &[0xAA, 0xBB]));
        assert_ne!(part_mesh_hash(base, 0), part_mesh_hash(base, 1));
    }

    // --- Loader fixtures ------------------------------------------------------------------------

    /// A fresh, empty temp directory unique to this process + `name` (removed first, so a re-run
    /// starts clean) — the project root a fixture assembly is written into.
    fn fresh_dir(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("ludion-assembly-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// Writes `contents` to `root`/`rel`, creating parent directories.
    fn write(root: &std::path::Path, rel: &str, contents: &str) {
        let path = root.join(rel);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, contents).unwrap();
    }

    /// A two-part assembly source naming `parts/body.part.yaml` and `parts/arm.part.yaml`.
    const ASSEMBLY_SOURCE: &str = "\
format: 1
kind: assembly
name: fixture
size:
  - 4
  - 2
  - 1
palette:
  A:
    color:
      - 1.0
      - 0.0
      - 0.0
      - 1.0
parts:
  - name: body
    file: parts/body.part.yaml
    place:
      - 0
      - 0
      - 0
  - name: arm
    file: parts/arm.part.yaml
    parent: body
    place:
      - 2
      - 1
      - 0
clips:
  walk: anims/walk.anim.yaml
";

    /// A part source of `size` with one row string per y layer.
    fn part_source(size: [u32; 3], rows: &[&str]) -> String {
        let mut out = format!(
            "format: 1\nkind: part\nsize:\n  - {}\n  - {}\n  - {}\nslices:\n",
            size[0], size[1], size[2]
        );
        for row in rows {
            out.push_str("  -\n");
            out.push_str(&format!("    - \"{row}\"\n"));
        }
        out
    }

    /// The `walk` clip `ASSEMBLY_SOURCE` binds: one rotation track on `arm`, block-style keyframes
    /// (the only spelling the strict-YAML subset accepts).
    const CLIP_SOURCE: &str = "\
format: 1
kind: animation
name: walk
duration: 1.0
loop: true
tracks:
  arm:
    rotation:
      - t: 0.0
        v:
          - 0.0
          - 0.0
          - 0.0
      - t: 1.0
        v:
          - 90.0
          - 0.0
          - 0.0
";

    /// Writes the two part files and the clip file `ASSEMBLY_SOURCE` names.
    fn write_fixture_sources(root: &std::path::Path) {
        write(
            root,
            "models/parts/body.part.yaml",
            &part_source([2, 1, 1], &["AA"]),
        );
        write(
            root,
            "models/parts/arm.part.yaml",
            &part_source([1, 1, 1], &["A"]),
        );
        write(root, "models/anims/walk.anim.yaml", CLIP_SOURCE);
    }

    /// The happy path end to end: the assembly, its parts, and its clips are read from disk with each
    /// `file` resolved against the assembly's own directory, and the result is the rest pose the pure
    /// path produces with the clips bound by short name.
    #[test]
    fn load_reads_parts_relative_to_the_assembly_file() {
        let root = fresh_dir("load-ok");
        write(&root, "models/fixture.assembly.yaml", ASSEMBLY_SOURCE);
        write_fixture_sources(&root);

        let data = load(
            ludion_assets::AssetRoot::content(&root),
            "models/fixture.assembly.yaml",
        )
        .expect("the assembly loads");
        assert_eq!(
            data.parts
                .iter()
                .map(|p| p.name.as_str())
                .collect::<Vec<_>>(),
            ["body", "arm"]
        );
        assert_eq!(data.parts[0].offset, [-1.0, 0.0, 0.0]);
        assert_eq!(data.parts[0].parent, None);
        assert_eq!(data.parts[1].offset, [0.5, 1.0, 0.0]);
        assert_eq!(data.parts[1].parent, Some(0));
        assert_ne!(data.parts[0].mesh.hash, data.parts[1].mesh.hash);
        assert_eq!(data.clips[0].name, "walk");
        assert_eq!(data.clips[0].file, "anims/walk.anim.yaml");
        let walk = data.clip("walk").expect("the `walk` binding resolves");
        assert_eq!(walk.duration, 1.0);
        assert!(walk.looped);
        assert_eq!(walk.tracks[0].target, "arm");
        assert_eq!(data.clip("attack"), None);

        // Loading again reads the same bytes, so the whole result — mesh keys included — is stable.
        assert_eq!(
            load(
                ludion_assets::AssetRoot::content(&root),
                "models/fixture.assembly.yaml"
            )
            .unwrap(),
            data
        );
    }

    /// A part or clip file that climbs out of the content root is refused with the general content
    /// rule, even when a real file sits at the escaped location.
    #[test]
    fn a_part_file_climbing_out_of_the_root_is_refused() {
        let base = fresh_dir("load-escape");
        let root = base.join("content");
        write(
            &root,
            "models/parts/arm.part.yaml",
            &part_source([1, 1, 1], &["A"]),
        );
        write(&root, "models/anims/walk.anim.yaml", CLIP_SOURCE);
        write(&base, "outside.part.yaml", &part_source([2, 1, 1], &["AA"]));
        let source = ASSEMBLY_SOURCE.replace("parts/body.part.yaml", "../../../outside.part.yaml");
        write(&root, "models/fixture.assembly.yaml", &source);

        let errors = load(
            ludion_assets::AssetRoot::content(&root),
            "models/fixture.assembly.yaml",
        )
        .expect_err("the part escapes");
        assert!(
            errors
                .iter()
                .any(|d| d.code == code::CONTENT_PATH_OUTSIDE_ROOT),
            "{errors:?}"
        );
    }

    /// Every source file the assembly names — its own document, each part, each clip — is on the
    /// reload dependency list, in that order. A clip file being on it is what makes an `asset.reload`
    /// of a clip re-read the assembly exactly as a part edit does.
    #[test]
    fn the_source_list_carries_the_clip_files() {
        let root = fresh_dir("load-sources");
        write(&root, "models/fixture.assembly.yaml", ASSEMBLY_SOURCE);
        write_fixture_sources(&root);

        let (_, sources) = load_with_sources(
            ludion_assets::AssetRoot::content(&root),
            "models/fixture.assembly.yaml",
        )
        .expect("the assembly loads");
        let sources: Vec<&str> = sources.iter().map(|(path, _)| path.as_str()).collect();
        assert_eq!(
            sources,
            [
                "models/fixture.assembly.yaml",
                "models/parts/body.part.yaml",
                "models/parts/arm.part.yaml",
                "models/anims/walk.anim.yaml",
            ]
        );
    }

    /// A clip file that is not there is the ordinary asset-read diagnostic — a bound clip is a
    /// declared dependency, not an optional one.
    #[test]
    fn a_missing_clip_file_is_an_asset_diagnostic() {
        let root = fresh_dir("load-missing-clip");
        write(&root, "models/fixture.assembly.yaml", ASSEMBLY_SOURCE);
        write(
            &root,
            "models/parts/body.part.yaml",
            &part_source([2, 1, 1], &["AA"]),
        );
        write(
            &root,
            "models/parts/arm.part.yaml",
            &part_source([1, 1, 1], &["A"]),
        );

        let errors = load(
            ludion_assets::AssetRoot::content(&root),
            "models/fixture.assembly.yaml",
        )
        .expect_err("the clip is gone");
        assert_eq!(errors.len(), 1);
        assert_eq!(errors[0].code, code::ASSET_FILE_IO);
        assert!(errors[0].message.contains("models/anims/walk.anim.yaml"));
    }

    /// A clip that fails its own validation surfaces those diagnostics, named by the binding — a
    /// clip pointer alone (`/tracks/arm/rotation/1/t`) never says which of an assembly's files it
    /// means.
    #[test]
    fn an_invalid_clip_surfaces_its_own_diagnostics() {
        let root = fresh_dir("load-invalid-clip");
        write(&root, "models/fixture.assembly.yaml", ASSEMBLY_SOURCE);
        write_fixture_sources(&root);
        // A zero duration: L1142.
        write(
            &root,
            "models/anims/walk.anim.yaml",
            &CLIP_SOURCE.replace("duration: 1.0", "duration: 0.0"),
        );

        let errors = load(
            ludion_assets::AssetRoot::content(&root),
            "models/fixture.assembly.yaml",
        )
        .expect_err("the clip is invalid");
        assert!(
            errors
                .iter()
                .any(|d| d.code == code::ANIMATION_DATA_CONSISTENCY),
            "expected an L1142: {errors:?}"
        );
        assert!(
            errors[0].message.contains("clip `walk`")
                && errors[0].message.contains("models/anims/walk.anim.yaml"),
            "the diagnostic names the failing binding and its path: {}",
            errors[0].message
        );
    }

    /// Every missing part is reported, each naming its resolved path — one pass, one full answer,
    /// rather than a fix-one-rerun loop.
    #[test]
    fn missing_part_files_are_reported_per_entry() {
        let root = fresh_dir("load-missing");
        write(&root, "models/fixture.assembly.yaml", ASSEMBLY_SOURCE);

        let errors = load(
            ludion_assets::AssetRoot::content(&root),
            "models/fixture.assembly.yaml",
        )
        .expect_err("both parts are gone");
        assert_eq!(errors.len(), 2, "one diagnostic per unreadable part file");
        assert!(errors.iter().all(|d| d.code == code::ASSET_FILE_IO));
        assert!(errors[0].message.contains("models/parts/body.part.yaml"));
        assert!(errors[1].message.contains("models/parts/arm.part.yaml"));
    }

    /// A part that fails its own validation surfaces those diagnostics, named by the part entry —
    /// a part pointer alone (`/slices/0/0`) never says which of an assembly's files it means.
    #[test]
    fn an_invalid_part_surfaces_its_own_diagnostics() {
        let root = fresh_dir("load-invalid-part");
        write(&root, "models/fixture.assembly.yaml", ASSEMBLY_SOURCE);
        // The row is one glyph wide where `size.x` says two: L1140.
        write(
            &root,
            "models/parts/body.part.yaml",
            &part_source([2, 1, 1], &["A"]),
        );
        write(
            &root,
            "models/parts/arm.part.yaml",
            &part_source([1, 1, 1], &["A"]),
        );

        let errors = load(
            ludion_assets::AssetRoot::content(&root),
            "models/fixture.assembly.yaml",
        )
        .expect_err("the part is invalid");
        assert!(errors.iter().any(|d| d.code == code::PART_DATA_CONSISTENCY));
        assert!(
            errors[0].message.contains("part `body`")
                && errors[0].message.contains("models/parts/body.part.yaml"),
            "the diagnostic names the failing part and its path: {}",
            errors[0].message
        );
    }

    /// A glyph the assembly's palette does not define is the cross-file check's `L1138` — the
    /// decision-4 lint, which only exists once both documents are in hand.
    #[test]
    fn an_unknown_glyph_is_an_assembly_diagnostic() {
        let root = fresh_dir("load-unknown-glyph");
        write(&root, "models/fixture.assembly.yaml", ASSEMBLY_SOURCE);
        write(
            &root,
            "models/parts/body.part.yaml",
            &part_source([2, 1, 1], &["AZ"]),
        );
        write(
            &root,
            "models/parts/arm.part.yaml",
            &part_source([1, 1, 1], &["A"]),
        );

        let errors = load(
            ludion_assets::AssetRoot::content(&root),
            "models/fixture.assembly.yaml",
        )
        .expect_err("`Z` is undefined");
        assert!(
            errors
                .iter()
                .any(|d| d.code == code::ASSEMBLY_DATA_CONSISTENCY
                    && d.message.contains('Z')
                    && d.message.contains("body")),
            "expected an L1138 naming the glyph and the part: {errors:?}"
        );
    }

    /// A missing assembly file is the ordinary asset-read diagnostic, before any part is looked at.
    #[test]
    fn a_missing_assembly_is_one_asset_diagnostic() {
        let root = fresh_dir("load-missing-assembly");
        let errors = load(
            ludion_assets::AssetRoot::content(&root),
            "models/absent.assembly.yaml",
        )
        .expect_err("nothing is there");
        assert_eq!(errors.len(), 1);
        assert_eq!(errors[0].code, code::ASSET_FILE_IO);
    }
}
